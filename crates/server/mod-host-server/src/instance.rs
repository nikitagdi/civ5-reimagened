//! The mods of one game: one instance per mod with a server half, in load order, behind the D7 checks.
//!
//! Every call into a mod goes through [`ModInstance::guarded`]: the payload was already checked (channel, size,
//! rate), the call runs under `catch_unwind`, and what the mod returns is filtered (capability, size). A panic
//! disables that mod for the rest of the game; the game, the other mods and the seat that triggered it go on.
//!
//! API 1.1 adds, on the same path: the tick (`[rust] tick_hz`, scheduled here on the host clock), `on_detach`, and
//! bodies and hits (`world.bodies`). Each mod's bodies live on its instance; a call sees the other mods' bodies as
//! they were when it began. The hits a call queued are handed, in order, to the owners' `on_hit` right after it
//! returns, one level deep.
//!
//! API 1.2 adds the cosmetic terrain share (`world.share_terrain`): the requests of a call are applied after it returns
//! to the mod's [`TerrainShares`] (what each seat was given), and what is new for a seat waits in the instance until the
//! host takes it ([`GameMods::take_terrain_shares`]). A seat whose client goes away, or that attaches again, is forgotten.
//!
//! Isolation needs `panic = "unwind"`. See [`PANIC_ISOLATION`].

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

use mod_api::{
    Audience, Body, BodyRef, Cap, Caps, ChannelId, Hit, HitQueue, HostServices, Limiter, LogLevel,
    Outgoing, PlotIdx, Seat, ServerCtx, ServerMod, ShareOut, ShareQueue, TerrainShares, Verdict,
    WorldFacts, prepare_bodies,
};
use mod_manifest::ModId;
use protocol::ModS2C;

use crate::limits::Strikes;

/// Whether this build can isolate a panicking mod. True under `panic = "unwind"` (the distribution server build,
/// plan decision 7). Under `abort` a mod panic ends the process, so a host that loads mods should check this at
/// start-up and say so loudly.
pub const PANIC_ISOLATION: bool = cfg!(panic = "unwind");

/// What the host knows at the moment of a call.
#[derive(Clone, Copy)]
pub struct CallEnv<'a> {
    /// Whole-truth map facts (a mod sees them only with `world.read`).
    pub facts: &'a dyn WorldFacts,
    /// A monotonic millisecond clock: the token buckets, `ServerCtx::now_ms`, and the tick schedule.
    pub now_ms: u64,
}

/// A line a mod (or the host on its behalf) wants in the server log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub mod_id: ModId,
    pub level: LogLevel,
    pub msg: String,
}

/// A message from a mod's server half, routed to the game actor. The actor turns `to` into seats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModOutput {
    pub mod_idx: u16,
    pub to: Audience,
    pub ch: ChannelId,
    /// The seat whose message caused it: set for the answers of `on_client`, `None` otherwise.
    pub from: Option<Seat>,
    pub payload: Vec<u8>,
}

impl ModOutput {
    /// The `GameMsg::Mod` body (the audience stays with the caller).
    pub fn to_wire(&self) -> ModS2C {
        ModS2C {
            mod_idx: self.mod_idx,
            ch: self.ch,
            from: self.from.map(Seat::player),
            payload: self.payload.clone(),
        }
    }
}

/// Why `on_client` did not reach the mod.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    /// No server half at that index (out of range, or a client-only mod). Counts as a violation.
    NoSuchMod,
    /// The mod panicked earlier and is off for this game. Not a violation: the client cannot know.
    Disabled,
    /// Unknown channel, oversize payload or empty bucket. Counts as a violation.
    Limit(Verdict),
}

/// The result of one client message.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClientOutcome {
    pub outputs: Vec<ModOutput>,
    pub dropped: Option<DropReason>,
    /// The seat has repeated violations: the host should disconnect it.
    pub disconnect: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModStatus {
    Active,
    /// Off for this game; `reason` is the panic message.
    Disabled {
        reason: String,
    },
}

struct CallHost<'a> {
    facts: &'a dyn WorldFacts,
    limiter: &'a mut Limiter,
    now_ms: u64,
    logs: &'a mut Vec<(LogLevel, String)>,
    others: &'a [Body],
    new_bodies: &'a mut Option<Vec<Body>>,
    queue: &'a mut HitQueue,
    shares: &'a mut ShareQueue,
}

impl HostServices for CallHost<'_> {
    fn facts(&self) -> &dyn WorldFacts {
        self.facts
    }
    fn allow(&mut self, seat: Seat, ch: ChannelId, cost: u32) -> bool {
        self.limiter.allow(seat, ch, cost, self.now_ms)
    }
    fn log(&mut self, level: LogLevel, msg: &str) {
        self.logs.push((level, msg.to_string()));
    }
    fn now_ms(&self) -> u64 {
        self.now_ms
    }
    fn bodies(&self) -> &[Body] {
        self.others
    }
    fn set_bodies(&mut self, bodies: Vec<Body>) {
        *self.new_bodies = Some(bodies);
    }
    fn hit(&mut self, target: BodyRef, hit: Hit) {
        self.queue.push(target, hit);
    }
    fn share_terrain(&mut self, seat: Seat, plots: &[PlotIdx]) {
        self.shares.share(seat, plots);
    }
    fn unshare_terrain(&mut self, seat: Seat) {
        self.shares.unshare(seat);
    }
}

/// When the next ticks of a mod are due: `base + k * 1000 / hz` ms, so 30 Hz does not drift.
#[derive(Debug, Clone, Copy)]
struct TickSched {
    hz: u32,
    base_ms: u64,
    k: u64,
}

impl TickSched {
    fn new(hz: u32, now_ms: u64) -> Self {
        Self {
            hz,
            base_ms: now_ms,
            k: 1,
        }
    }

    fn due_ms(&self) -> u64 {
        self.base_ms + self.k * 1000 / u64::from(self.hz)
    }

    /// The tick at `now_ms` ran. A late one is not caught up: if the next is already due, the schedule starts again
    /// from now.
    fn ran(&mut self, now_ms: u64) {
        self.k += 1;
        if self.due_ms() <= now_ms {
            *self = Self::new(self.hz, now_ms);
        }
    }
}

/// One mod of one game.
pub struct ModInstance {
    id: ModId,
    load_idx: u16,
    caps: Caps,
    limiter: Limiter,
    module: Box<dyn ServerMod>,
    status: ModStatus,
    logs: Vec<(LogLevel, String)>,
    /// The last `persist()` that passed the size check, or the bytes restored from the save.
    last_state: Vec<u8>,
    have_state: bool,
    /// `[rust] tick_hz` (0: never).
    tick_hz: u32,
    /// Set by `start`; `None` for a mod that does not tick (or is off).
    tick: Option<TickSched>,
    /// What the mod published (`set_bodies`), after the limits.
    bodies: Vec<Body>,
    /// The bodies of the other mods, as of `others_gen` (the snapshot its calls see).
    others: Vec<Body>,
    others_gen: u64,
    /// `bodies` changed in the last call.
    bodies_changed: bool,
    /// What the mod shared of the terrain to each seat (`world.share_terrain`).
    shares: TerrainShares,
    /// The shares the host has not taken yet, in order.
    share_out: Vec<ShareOut>,
}

fn panic_text(p: &(dyn Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// Whether hits queued in a call are delivered.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hits {
    Deliver,
    /// Inside `on_hit`: dropped (one level deep).
    Nested,
}

impl ModInstance {
    pub(crate) fn new(
        id: ModId,
        load_idx: u16,
        caps: Caps,
        limiter: Limiter,
        module: Box<dyn ServerMod>,
        tick_hz: u32,
    ) -> Self {
        Self {
            id,
            load_idx,
            caps,
            limiter,
            module,
            status: ModStatus::Active,
            logs: Vec::new(),
            last_state: Vec::new(),
            have_state: false,
            tick_hz,
            tick: None,
            bodies: Vec::new(),
            others: Vec::new(),
            others_gen: u64::MAX,
            bodies_changed: false,
            shares: TerrainShares::new(),
            share_out: Vec::new(),
        }
    }

    pub fn id(&self) -> &ModId {
        &self.id
    }

    pub fn load_idx(&self) -> u16 {
        self.load_idx
    }

    pub fn status(&self) -> &ModStatus {
        &self.status
    }

    /// The bodies the mod published (owner set, within the limits).
    pub fn bodies(&self) -> &[Body] {
        &self.bodies
    }

    fn active(&self) -> bool {
        self.status == ModStatus::Active
    }

    /// Runs `f` on the mod under `catch_unwind`. `None`: the mod is off, or it panicked in this call (and is off
    /// now). The mod's `ServerCtx` logs are kept in `self.logs`; the bodies it set replace its list; the hits it
    /// queued come back with the result (none for [`Hits::Nested`]).
    fn guarded<R>(
        &mut self,
        env: CallEnv<'_>,
        what: &str,
        hits: Hits,
        f: impl FnOnce(&mut dyn ServerMod, &mut ServerCtx<'_>) -> R,
    ) -> Option<(R, Vec<(BodyRef, Hit)>)> {
        if !self.active() {
            return None;
        }
        let mut queue = match hits {
            Hits::Deliver => HitQueue::new(),
            Hits::Nested => HitQueue::nested(),
        };
        let mut new_bodies = None;
        let mut shares = ShareQueue::new();
        let mut host = CallHost {
            facts: env.facts,
            limiter: &mut self.limiter,
            now_ms: env.now_ms,
            logs: &mut self.logs,
            others: &self.others,
            new_bodies: &mut new_bodies,
            queue: &mut queue,
            shares: &mut shares,
        };
        let mut cx = ServerCtx::new(&self.caps, &mut host);
        let module = self.module.as_mut();
        match catch_unwind(AssertUnwindSafe(|| f(module, &mut cx))) {
            Ok(r) => {
                if let Some(b) = new_bodies {
                    let (b, warn) = prepare_bodies(b, &self.id);
                    if let Some(w) = warn {
                        self.logs.push((LogLevel::Warn, w));
                    }
                    self.bodies = b;
                    self.bodies_changed = true;
                }
                let (queued, warn) = queue.finish();
                if let Some(w) = warn {
                    self.logs.push((LogLevel::Warn, w));
                }
                let (ops, warn) = shares.finish();
                if let Some(w) = warn {
                    self.logs.push((LogLevel::Warn, w));
                }
                if !ops.is_empty() {
                    let plot_count = env.facts.map().plot_count();
                    let out = self.shares.apply(ops, plot_count);
                    self.share_out.extend(out);
                }
                Some((r, queued))
            }
            Err(p) => {
                let reason = panic_text(p.as_ref());
                self.logs.push((
                    LogLevel::Error,
                    format!("panicked in {what}: {reason}; the mod is disabled for this game"),
                ));
                self.status = ModStatus::Disabled { reason };
                self.tick = None;
                if !self.bodies.is_empty() {
                    self.bodies.clear();
                    self.bodies_changed = true;
                }
                None
            }
        }
    }

    /// Capability and size filter on what the mod returned; refused messages are logged.
    fn route(&mut self, out: Vec<Outgoing>, from: Option<Seat>) -> Vec<ModOutput> {
        let f = self.limiter.filter_outgoing(&self.caps, out);
        for d in &f.dropped {
            self.logs.push((
                LogLevel::Warn,
                format!(
                    "dropped an outgoing message on channel {}: {:?}",
                    d.ch, d.reason
                ),
            ));
        }
        f.kept
            .into_iter()
            .map(|o| ModOutput {
                mod_idx: self.load_idx,
                to: o.to,
                ch: o.ch,
                from,
                payload: o.payload,
            })
            .collect()
    }

    /// The state to save: a fresh `persist()` when the mod is dirty (or never saved), else the last good one. A
    /// disabled mod, a panic, or a blob over `state_max` keeps the last good state.
    fn persist(&mut self, env: CallEnv<'_>) -> Vec<u8> {
        if !self.active() {
            return self.last_state.clone();
        }
        let plain = Hits::Nested;
        if self.have_state
            && self
                .guarded(env, "is_dirty", plain, |m, _| m.is_dirty())
                .map(|(d, _)| d)
                == Some(false)
        {
            return self.last_state.clone();
        }
        if let Some((bytes, _)) = self.guarded(env, "persist", plain, |m, _| m.persist()) {
            if self.limiter.check_state(bytes.len()) {
                self.last_state = bytes;
                self.have_state = true;
                let _ = self.guarded(env, "mark_saved", plain, |m, _| m.mark_saved());
            } else {
                self.logs.push((
                    LogLevel::Error,
                    format!(
                        "persist() returned {} bytes, state_max is {}; the previous state is kept",
                        bytes.len(),
                        self.limiter.state_max()
                    ),
                ));
            }
        }
        self.last_state.clone()
    }
}

/// The server-side mods of one game. Empty for a game without mods; then every method does nothing.
#[derive(Default)]
pub struct GameMods {
    /// Indexed by load order; `None` for a mod without a server half.
    slots: Vec<Option<ModInstance>>,
    strikes: Strikes,
    /// Bumped whenever some mod's bodies change: the snapshots of the others are rebuilt lazily.
    bodies_gen: u64,
}

impl GameMods {
    pub(crate) fn new(slots: Vec<Option<ModInstance>>) -> Self {
        Self {
            slots,
            strikes: Strikes::default(),
            bodies_gen: 0,
        }
    }

    /// No instance at all: the game behaves exactly as without the mod system.
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }

    /// Some mod with a server half declared `world.read`: the host keeps the table of owners for them (T22). A game
    /// where no mod does never asks the worker for it.
    pub fn wants_world(&self) -> bool {
        self.slots
            .iter()
            .flatten()
            .any(|m| m.caps.has(Cap::WorldRead))
    }

    /// The size of the load order (including mods without a server half).
    pub fn mod_count(&self) -> usize {
        self.slots.len()
    }

    /// One call into the mod at `i` (it exists): its snapshot of the other bodies is brought up to date first, a
    /// change of its own bodies is noted after.
    fn call<R>(
        &mut self,
        i: usize,
        env: CallEnv<'_>,
        what: &str,
        hits: Hits,
        f: impl FnOnce(&mut dyn ServerMod, &mut ServerCtx<'_>) -> R,
    ) -> Option<(R, Vec<(BodyRef, Hit)>)> {
        self.refresh_others(i);
        let inst = self.slots.get_mut(i)?.as_mut()?;
        let r = inst.guarded(env, what, hits, f);
        if std::mem::take(&mut inst.bodies_changed) {
            self.bodies_gen += 1;
        }
        r
    }

    fn refresh_others(&mut self, i: usize) {
        let generation = self.bodies_gen;
        let stale = matches!(self.slots.get(i), Some(Some(m))
            if m.caps.has(Cap::WorldBodies) && m.others_gen != generation);
        if !stale {
            return;
        }
        let others: Vec<Body> = self
            .slots
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .filter_map(|(_, s)| s.as_ref())
            .flat_map(|m| m.bodies.iter().cloned())
            .collect();
        if let Some(Some(m)) = self.slots.get_mut(i) {
            m.others = others;
            m.others_gen = generation;
        }
    }

    /// A call that returns messages: the call, its routed answers, then the `on_hit` answers of the hits it queued.
    fn call_out(
        &mut self,
        i: usize,
        env: CallEnv<'_>,
        what: &str,
        from: Option<Seat>,
        f: impl FnOnce(&mut dyn ServerMod, &mut ServerCtx<'_>) -> Vec<Outgoing>,
    ) -> Vec<ModOutput> {
        let Some((out, hits)) = self.call(i, env, what, Hits::Deliver, f) else {
            return Vec::new();
        };
        let mut all = match self.slots.get_mut(i) {
            Some(Some(inst)) => inst.route(out, from),
            _ => Vec::new(),
        };
        all.extend(self.deliver_hits(i, env, hits));
        all
    }

    /// Hands the hits mod `from` queued to the owners' `on_hit`, in order. An owner that is not in the game, has no
    /// server half, is off, or has no body with that id: the hit is dropped with a line in the hitter's log.
    fn deliver_hits(
        &mut self,
        from: usize,
        env: CallEnv<'_>,
        hits: Vec<(BodyRef, Hit)>,
    ) -> Vec<ModOutput> {
        let mut all = Vec::new();
        if hits.is_empty() {
            return all;
        }
        let Some(Some(hitter)) = self.slots.get(from) else {
            return all;
        };
        let from_id = hitter.id.clone();
        for (target, hit) in hits {
            let owner = self
                .slots
                .iter()
                .position(|s| s.as_ref().is_some_and(|m| m.id == target.owner));
            let why = match owner.and_then(|o| self.slots[o].as_ref()) {
                None => Some("no such mod with a server half in this game"),
                Some(m) if !m.active() => Some("the mod is off"),
                Some(m) if !m.bodies.iter().any(|b| b.id == target.id) => {
                    Some("the mod has no such body")
                }
                Some(_) => None,
            };
            let Some(o) = owner.filter(|_| why.is_none()) else {
                if let Some(Some(h)) = self.slots.get_mut(from) {
                    h.logs.push((
                        LogLevel::Warn,
                        format!(
                            "a hit on body {} of `{}` was dropped: {}",
                            target.id,
                            target.owner,
                            why.unwrap_or_default()
                        ),
                    ));
                }
                continue;
            };
            let id = target.id;
            if let Some((out, _)) = self.call(o, env, "on_hit", Hits::Nested, |m, cx| {
                m.on_hit(cx, &from_id, id, &hit)
            }) && let Some(Some(inst)) = self.slots.get_mut(o)
            {
                all.extend(inst.route(out, None));
            }
        }
        all
    }

    /// Starts every mod, in load order. `saved` holds the states from the save's `MODS` section by mod id; a mod
    /// without an entry starts fresh. The tick schedule starts at `env.now_ms`. Hits queued in `start` are dropped
    /// (the other mods may not have started yet).
    pub fn start(&mut self, env: CallEnv<'_>, saved: &[(ModId, Vec<u8>)]) {
        for i in 0..self.slots.len() {
            let Some(Some(m)) = self.slots.get_mut(i) else {
                continue;
            };
            let state = saved
                .iter()
                .find(|(id, _)| id == &m.id)
                .map(|(_, s)| s.clone());
            if let Some(s) = &state {
                m.last_state.clone_from(s);
                m.have_state = true;
            }
            m.tick = (m.tick_hz > 0).then(|| TickSched::new(m.tick_hz, env.now_ms));
            let r = self.call(i, env, "start", Hits::Deliver, |m, cx| {
                m.start(cx, state.as_deref());
            });
            if let Some(((), hits)) = r
                && !hits.is_empty()
                && let Some(Some(m)) = self.slots.get_mut(i)
            {
                m.logs.push((
                    LogLevel::Warn,
                    format!("{} hits queued in start were dropped", hits.len()),
                ));
            }
        }
    }

    /// One message from a client. The payload is checked first (channel, size, rate); the mod never sees a
    /// refused one. Repeated violations of a seat raise `disconnect`.
    pub fn on_client(
        &mut self,
        env: CallEnv<'_>,
        from: Seat,
        mod_idx: u16,
        ch: ChannelId,
        payload: &[u8],
    ) -> ClientOutcome {
        let i = usize::from(mod_idx);
        let Some(Some(inst)) = self.slots.get_mut(i) else {
            return self.violation(from, env.now_ms, DropReason::NoSuchMod);
        };
        if !inst.active() {
            return ClientOutcome {
                dropped: Some(DropReason::Disabled),
                ..ClientOutcome::default()
            };
        }
        let verdict = inst.limiter.check_c2s(from, ch, payload.len(), env.now_ms);
        if !verdict.is_ok() {
            return self.violation(from, env.now_ms, DropReason::Limit(verdict));
        }
        let outputs = self.call_out(i, env, "on_client", Some(from), |m, cx| {
            m.on_client(cx, from, ch, payload)
        });
        ClientOutcome {
            outputs,
            dropped: None,
            disconnect: false,
        }
    }

    fn violation(&mut self, seat: Seat, now_ms: u64, why: DropReason) -> ClientOutcome {
        ClientOutcome {
            outputs: Vec::new(),
            dropped: Some(why),
            disconnect: self.strikes.hit(seat, now_ms),
        }
    }

    /// A seat attached (Fresh attach or reconnect): the snapshot pages of every mod, in load order. The seat's fresh
    /// stream holds no terrain share: what the mods shared to it is forgotten first, so a share made here goes whole.
    pub fn on_attach(&mut self, env: CallEnv<'_>, seat: Seat) -> Vec<ModOutput> {
        self.forget_shares(seat);
        let mut all = Vec::new();
        for i in 0..self.slots.len() {
            all.extend(self.call_out(i, env, "on_attach", None, |m, cx| m.on_attach(cx, seat)));
        }
        all
    }

    /// The client of a seat went away (connection lost, left, kicked): every mod, in load order. Then what the mods
    /// shared of the terrain to the seat is forgotten (nothing goes to a seat that is away).
    pub fn on_detach(&mut self, env: CallEnv<'_>, seat: Seat) -> Vec<ModOutput> {
        let mut all = Vec::new();
        for i in 0..self.slots.len() {
            all.extend(self.call_out(i, env, "on_detach", None, |m, cx| m.on_detach(cx, seat)));
        }
        self.forget_shares(seat);
        all
    }

    /// Every mod forgets what it shared of the terrain to the seat; the shares not taken yet for it are dropped.
    fn forget_shares(&mut self, seat: Seat) {
        for m in self.slots.iter_mut().flatten() {
            m.shares.forget_seat(seat);
            m.share_out.retain(|s| s.seat != seat);
        }
    }

    /// Takes the terrain shares the mods made since the last call (`world.share_terrain`, API 1.2): `(load index,
    /// share)`, in load order and, per mod, in the order made. The host sends each to its seat.
    pub fn take_terrain_shares(&mut self) -> Vec<(u16, ShareOut)> {
        let mut out = Vec::new();
        for m in self.slots.iter_mut().flatten() {
            let idx = m.load_idx;
            out.extend(m.share_out.drain(..).map(|s| (idx, s)));
        }
        out
    }

    /// The plots the mod at `mod_idx` shares to the seat now, ascending (tests, logs).
    pub fn shared_terrain(&self, mod_idx: u16, seat: Seat) -> Vec<PlotIdx> {
        self.slots
            .get(usize::from(mod_idx))
            .and_then(Option::as_ref)
            .map_or_else(Vec::new, |m| m.shares.shared(seat).collect())
    }

    /// A new turn began. `on_turn` returns nothing, but the hits it queued are delivered: their answers come back.
    pub fn on_turn(&mut self, env: CallEnv<'_>, turn: i32) -> Vec<ModOutput> {
        let mut all = Vec::new();
        for i in 0..self.slots.len() {
            all.extend(self.call_out(i, env, "on_turn", None, |m, cx| {
                m.on_turn(cx, turn);
                Vec::new()
            }));
        }
        all
    }

    /// The earliest time a mod's tick is due (`None`: no mod ticks). The actor wakes then and calls
    /// [`on_tick`](Self::on_tick).
    pub fn next_tick_ms(&self) -> Option<u64> {
        self.slots
            .iter()
            .flatten()
            .filter(|m| m.active())
            .filter_map(|m| m.tick.map(|t| t.due_ms()))
            .min()
    }

    /// Ticks every mod whose tick is due at `env.now_ms`, in load order. A mod is ticked at most once per call; one
    /// that fell behind skips the missed ticks.
    pub fn on_tick(&mut self, env: CallEnv<'_>) -> Vec<ModOutput> {
        let now = env.now_ms;
        let mut all = Vec::new();
        for i in 0..self.slots.len() {
            let due = matches!(self.slots.get(i), Some(Some(m))
                if m.active() && m.tick.is_some_and(|t| t.due_ms() <= now));
            if !due {
                continue;
            }
            if let Some(Some(m)) = self.slots.get_mut(i)
                && let Some(t) = &mut m.tick
            {
                t.ran(now);
            }
            all.extend(self.call_out(i, env, "on_tick", None, |m, cx| m.on_tick(cx, now)));
        }
        all
    }

    /// The states for the save's `MODS` section: one entry per mod with a server half, in load order. A mod that
    /// is off keeps the state it had.
    pub fn persist(&mut self, env: CallEnv<'_>) -> Vec<(ModId, Vec<u8>)> {
        let mut changed = false;
        let states = self
            .slots
            .iter_mut()
            .flatten()
            .map(|m| {
                let s = (m.id.clone(), m.persist(env));
                // A panic in persist() switched the mod off and took its bodies away.
                changed |= std::mem::take(&mut m.bodies_changed);
                s
            })
            .collect();
        if changed {
            self.bodies_gen += 1;
        }
        states
    }

    /// The seat left the game: its buckets and violation count go.
    pub fn forget_seat(&mut self, seat: Seat) {
        self.forget_shares(seat);
        self.strikes.forget(seat);
        for m in self.slots.iter_mut().flatten() {
            m.limiter.forget_seat(seat);
        }
    }

    /// Status of every mod with a server half, in load order.
    pub fn statuses(&self) -> Vec<(ModId, ModStatus)> {
        self.slots
            .iter()
            .flatten()
            .map(|m| (m.id.clone(), m.status.clone()))
            .collect()
    }

    /// The bodies every mod with a server half published, in load order.
    pub fn bodies(&self) -> Vec<(ModId, &[Body])> {
        self.slots
            .iter()
            .flatten()
            .map(|m| (m.id.clone(), m.bodies.as_slice()))
            .collect()
    }

    /// Takes the log lines the mods (and the host for them) produced since the last call, in load order.
    pub fn take_logs(&mut self) -> Vec<LogLine> {
        let mut out = Vec::new();
        for m in self.slots.iter_mut().flatten() {
            for (level, msg) in m.logs.drain(..) {
                out.push(LogLine {
                    mod_id: m.id.clone(),
                    level,
                    msg,
                });
            }
        }
        out
    }
}
