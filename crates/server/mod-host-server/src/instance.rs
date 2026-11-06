//! The mods of one game: one instance per mod with a server half, in load order, behind the D7 checks.
//!
//! Every call into a mod goes through [`ModInstance::guarded`]: the payload was already checked (channel, size,
//! rate), the call runs under `catch_unwind`, and what the mod returns is filtered (capability, size). A panic
//! disables that mod for the rest of the game; the game, the other mods and the seat that triggered it go on.
//!
//! Isolation needs `panic = "unwind"`. See [`PANIC_ISOLATION`].

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

use mod_api::{
    Audience, Caps, ChannelId, HostServices, Limiter, LogLevel, Outgoing, Seat, ServerCtx,
    ServerMod, Verdict, WorldFacts,
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
    /// A monotonic millisecond clock for the token buckets.
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

impl ModInstance {
    pub(crate) fn new(
        id: ModId,
        load_idx: u16,
        caps: Caps,
        limiter: Limiter,
        module: Box<dyn ServerMod>,
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

    fn active(&self) -> bool {
        self.status == ModStatus::Active
    }

    /// Runs `f` on the mod under `catch_unwind`. `None`: the mod is off, or it panicked in this call (and is off
    /// now). The mod's `ServerCtx` logs are kept in `self.logs`.
    fn guarded<R>(
        &mut self,
        env: CallEnv<'_>,
        what: &str,
        f: impl FnOnce(&mut dyn ServerMod, &mut ServerCtx<'_>) -> R,
    ) -> Option<R> {
        if !self.active() {
            return None;
        }
        let mut host = CallHost {
            facts: env.facts,
            limiter: &mut self.limiter,
            now_ms: env.now_ms,
            logs: &mut self.logs,
        };
        let mut cx = ServerCtx::new(&self.caps, &mut host);
        let module = self.module.as_mut();
        match catch_unwind(AssertUnwindSafe(|| f(module, &mut cx))) {
            Ok(r) => Some(r),
            Err(p) => {
                let reason = panic_text(p.as_ref());
                self.logs.push((
                    LogLevel::Error,
                    format!("panicked in {what}: {reason}; the mod is disabled for this game"),
                ));
                self.status = ModStatus::Disabled { reason };
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

    fn start(&mut self, env: CallEnv<'_>, saved: Option<&[u8]>) {
        if let Some(s) = saved {
            self.last_state = s.to_vec();
            self.have_state = true;
        }
        let _ = self.guarded(env, "start", |m, cx| m.start(cx, saved));
    }

    /// The state to save: a fresh `persist()` when the mod is dirty (or never saved), else the last good one. A
    /// disabled mod, a panic, or a blob over `state_max` keeps the last good state.
    fn persist(&mut self, env: CallEnv<'_>) -> Vec<u8> {
        if !self.active() {
            return self.last_state.clone();
        }
        if self.have_state && self.guarded(env, "is_dirty", |m, _| m.is_dirty()) == Some(false) {
            return self.last_state.clone();
        }
        if let Some(bytes) = self.guarded(env, "persist", |m, _| m.persist()) {
            if self.limiter.check_state(bytes.len()) {
                self.last_state = bytes;
                self.have_state = true;
                let _ = self.guarded(env, "mark_saved", |m, _| m.mark_saved());
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
}

impl GameMods {
    pub(crate) fn new(slots: Vec<Option<ModInstance>>) -> Self {
        Self {
            slots,
            strikes: Strikes::default(),
        }
    }

    /// No instance at all: the game behaves exactly as without the mod system.
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }

    /// The size of the load order (including mods without a server half).
    pub fn mod_count(&self) -> usize {
        self.slots.len()
    }

    fn live(&mut self) -> impl Iterator<Item = &mut ModInstance> {
        self.slots.iter_mut().flatten()
    }

    /// Starts every mod, in load order. `saved` holds the states from the save's `MODS` section by mod id; a mod
    /// without an entry starts fresh.
    pub fn start(&mut self, env: CallEnv<'_>, saved: &[(ModId, Vec<u8>)]) {
        for m in self.live() {
            let state = saved
                .iter()
                .find(|(id, _)| id == &m.id)
                .map(|(_, s)| s.as_slice());
            m.start(env, state);
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
        let Some(Some(inst)) = self.slots.get_mut(usize::from(mod_idx)) else {
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
        let out = inst
            .guarded(env, "on_client", |m, cx| m.on_client(cx, from, ch, payload))
            .map(|out| inst.route(out, Some(from)))
            .unwrap_or_default();
        ClientOutcome {
            outputs: out,
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

    /// A seat attached (Fresh attach or reconnect): the snapshot pages of every mod, in load order.
    pub fn on_attach(&mut self, env: CallEnv<'_>, seat: Seat) -> Vec<ModOutput> {
        let mut all = Vec::new();
        for m in self.live() {
            if let Some(out) = m.guarded(env, "on_attach", |m, cx| m.on_attach(cx, seat)) {
                all.extend(m.route(out, None));
            }
        }
        all
    }

    /// A new turn began.
    pub fn on_turn(&mut self, env: CallEnv<'_>, turn: i32) {
        for m in self.live() {
            let _ = m.guarded(env, "on_turn", |m, cx| m.on_turn(cx, turn));
        }
    }

    /// The states for the save's `MODS` section: one entry per mod with a server half, in load order. A mod that
    /// is off keeps the state it had.
    pub fn persist(&mut self, env: CallEnv<'_>) -> Vec<(ModId, Vec<u8>)> {
        self.live()
            .map(|m| (m.id.clone(), m.persist(env)))
            .collect()
    }

    /// The seat left the game: its buckets and violation count go.
    pub fn forget_seat(&mut self, seat: Seat) {
        self.strikes.forget(seat);
        for m in self.live() {
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

    /// Takes the log lines the mods (and the host for them) produced since the last call, in load order.
    pub fn take_logs(&mut self) -> Vec<LogLine> {
        let mut out = Vec::new();
        for m in self.live() {
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
