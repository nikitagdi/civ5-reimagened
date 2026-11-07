//! Test doubles for mod unit tests: [`MockFacts`] (a settable map) and [`MockCtx`] (a host stand-in that applies the
//! same checks the real host does: channel, size, rate, capabilities, and the limits on bodies and hits).
//!
//! API 1.1: [`MockCtx::tick`], [`MockCtx::detach`] and [`MockCtx::hit`] drive the new calls; the bodies and hits of
//! the mock world ([`MockBodies`]) live in [`MockFacts`], so `MockHost` and `MockCtx` keep their 1.0 fields and a test
//! that builds them with a struct literal still compiles.

use mod_manifest::{Channel, ModId};

use crate::bodies::{Body, BodyRef, Hit, HitQueue, prepare_bodies};
use crate::caps::Caps;
use crate::facts::{MapShape, Owner, PlotClass, PlotIdx, WorldFacts};
use crate::ids::{ChannelId, PlayerId, Seat};
use crate::limits::{Dropped, Limiter, Verdict};
use crate::server::{HostServices, LogLevel, Outgoing, ServerCtx, ServerMod};

/// The bodies of the mock world (API 1.1): the other mods' bodies the test sets, what the mod under test published,
/// and the hits it queued.
#[derive(Debug, Clone)]
pub struct MockBodies {
    /// The id of the mod under test (the owner of the bodies it publishes). `mock` unless set.
    pub mod_id: ModId,
    /// The bodies of the other mods, what `ServerCtx::bodies` returns. The owners stay as the test wrote them.
    pub others: Vec<Body>,
    /// What the mod published last with `ServerCtx::set_bodies`, after the host's limits (owner set, cut to size).
    pub published: Vec<Body>,
    /// Every hit the mod queued, in order, over all calls (a [`Delivery`] also has the hits of its own call).
    pub hits: Vec<(BodyRef, Hit)>,
    queue: HitQueue,
}

impl Default for MockBodies {
    fn default() -> Self {
        Self {
            mod_id: ModId::new("mock").unwrap_or_else(|_| unreachable!("a valid id")),
            others: Vec::new(),
            published: Vec::new(),
            hits: Vec::new(),
            queue: HitQueue::new(),
        }
    }
}

/// A map with settable owners and classes. Every plot starts as unowned `Land`. Also holds the [`MockBodies`].
#[derive(Debug, Clone)]
pub struct MockFacts {
    shape: MapShape,
    turn: i32,
    owners: Vec<Owner>,
    classes: Vec<PlotClass>,
    seats: Vec<(Seat, PlayerId)>,
    bodies: MockBodies,
}

impl MockFacts {
    pub fn new(width: u16, height: u16) -> Self {
        let shape = MapShape {
            width,
            height,
            wrap_x: true,
            wrap_y: false,
        };
        let n = shape.plot_count() as usize;
        Self {
            shape,
            turn: 0,
            owners: vec![Owner::None; n],
            classes: vec![PlotClass::Land; n],
            seats: Vec::new(),
            bodies: MockBodies::default(),
        }
    }

    pub fn with_wrap(mut self, wrap_x: bool, wrap_y: bool) -> Self {
        self.shape.wrap_x = wrap_x;
        self.shape.wrap_y = wrap_y;
        self
    }

    pub fn with_turn(mut self, turn: i32) -> Self {
        self.turn = turn;
        self
    }

    /// Adds a human seat that plays the player with the same id.
    pub fn with_seat(mut self, seat: Seat) -> Self {
        self.seats.push((seat, seat.player()));
        self
    }

    pub fn set_turn(&mut self, turn: i32) {
        self.turn = turn;
    }

    pub fn set_owner(&mut self, p: PlotIdx, owner: Owner) {
        if let Some(o) = self.owners.get_mut(p as usize) {
            *o = owner;
        }
    }

    pub fn set_class(&mut self, p: PlotIdx, class: PlotClass) {
        if let Some(c) = self.classes.get_mut(p as usize) {
            *c = class;
        }
    }

    /// Marks every plot `Unknown` (the host's table is not current).
    pub fn invalidate_owners(&mut self) {
        self.owners.fill(Owner::Unknown);
    }

    /// The bodies and hits of the mock world.
    pub fn bodies(&self) -> &MockBodies {
        &self.bodies
    }

    pub fn bodies_mut(&mut self) -> &mut MockBodies {
        &mut self.bodies
    }
}

impl WorldFacts for MockFacts {
    fn map(&self) -> MapShape {
        self.shape
    }
    fn turn(&self) -> i32 {
        self.turn
    }
    fn plot_at(&self, x: i16, y: i16) -> Option<PlotIdx> {
        self.shape.index(x, y)
    }
    fn owner(&self, p: PlotIdx) -> Owner {
        self.owners.get(p as usize).copied().unwrap_or(Owner::None)
    }
    fn class(&self, p: PlotIdx) -> PlotClass {
        self.classes
            .get(p as usize)
            .copied()
            .unwrap_or(PlotClass::Water)
    }
    fn seat_player(&self, seat: Seat) -> Option<PlayerId> {
        self.seats.iter().find(|(s, _)| *s == seat).map(|(_, p)| *p)
    }
}

/// The [`HostServices`] of [`MockCtx`]: public fields so tests can set the clock and read the log. The bodies are in
/// `facts` ([`MockFacts::bodies`]).
pub struct MockHost {
    pub facts: MockFacts,
    pub limiter: Limiter,
    pub now_ms: u64,
    pub logs: Vec<(LogLevel, String)>,
}

impl HostServices for MockHost {
    fn facts(&self) -> &dyn WorldFacts {
        &self.facts
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
        &self.facts.bodies.others
    }
    fn set_bodies(&mut self, bodies: Vec<Body>) {
        let b = &mut self.facts.bodies;
        let (bodies, warn) = prepare_bodies(bodies, &b.mod_id);
        b.published = bodies;
        if let Some(w) = warn {
            self.logs.push((LogLevel::Warn, w));
        }
    }
    fn hit(&mut self, target: BodyRef, hit: Hit) {
        self.facts.bodies.queue.push(target, hit);
    }
}

/// What the mock host delivered for one call.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Delivery {
    /// The host refused the message before the mod saw it.
    pub refused: Option<Verdict>,
    /// Messages that passed the capability and size checks.
    pub sent: Vec<Outgoing>,
    pub dropped: Vec<Dropped>,
    /// The hits the call queued (the real host hands each to the owner's `on_hit`; the mock only collects them).
    pub hits: Vec<(BodyRef, Hit)>,
}

/// A host stand-in: owns the facts, the clock, the limiter and the log, and hands out a [`ServerCtx`].
pub struct MockCtx {
    pub caps: Caps,
    pub host: MockHost,
}

impl MockCtx {
    /// All capabilities, no channels declared.
    pub fn new(facts: MockFacts) -> Self {
        Self::with_channels(facts, Caps::all(), &[])
    }

    pub fn with_channels(facts: MockFacts, caps: Caps, channels: &[Channel]) -> Self {
        Self {
            caps,
            host: MockHost {
                facts,
                limiter: Limiter::new(channels, 1024 * 1024),
                now_ms: 0,
                logs: Vec::new(),
            },
        }
    }

    /// The id the mod under test has (the owner of its bodies).
    pub fn with_mod_id(mut self, id: &str) -> Self {
        self.host.facts.bodies.mod_id =
            ModId::new(id).unwrap_or_else(|e| panic!("mod id `{id}`: {e}"));
        self
    }

    pub fn cx(&mut self) -> ServerCtx<'_> {
        ServerCtx::new(&self.caps, &mut self.host)
    }

    pub fn advance_ms(&mut self, ms: u64) {
        self.host.now_ms += ms;
    }

    pub fn facts_mut(&mut self) -> &mut MockFacts {
        &mut self.host.facts
    }

    /// The bodies the other mods of the game have (what `ServerCtx::bodies` returns). The owners stay as given.
    pub fn set_other_bodies(&mut self, bodies: Vec<Body>) {
        self.host.facts.bodies.others = bodies;
    }

    /// What the mod published with `ServerCtx::set_bodies`, after the host's limits.
    pub fn published_bodies(&self) -> &[Body] {
        &self.host.facts.bodies.published
    }

    /// Every hit the mod queued so far, in order.
    pub fn hits(&self) -> &[(BodyRef, Hit)] {
        &self.host.facts.bodies.hits
    }

    /// Calls `start` with the saved bytes. Hits it queues land in [`hits`](Self::hits).
    pub fn start(&mut self, m: &mut dyn ServerMod, saved: Option<&[u8]>) {
        self.host.facts.bodies.queue = HitQueue::new();
        m.start(&mut ServerCtx::new(&self.caps, &mut self.host), saved);
        let _ = self.take_queue();
    }

    /// Delivers a client message the way the host does: limits first, then the mod, then the output filter.
    pub fn deliver(
        &mut self,
        m: &mut dyn ServerMod,
        from: Seat,
        ch: ChannelId,
        payload: &[u8],
    ) -> Delivery {
        let v = self
            .host
            .limiter
            .check_c2s(from, ch, payload.len(), self.host.now_ms);
        if !v.is_ok() {
            self.log_line(
                LogLevel::Warn,
                format!("refused message on channel {ch}: {v:?}"),
            );
            return Delivery {
                refused: Some(v),
                ..Delivery::default()
            };
        }
        self.host.facts.bodies.queue = HitQueue::new();
        let out = m.on_client(
            &mut ServerCtx::new(&self.caps, &mut self.host),
            from,
            ch,
            payload,
        );
        self.finish(out)
    }

    /// Calls `on_attach` and filters the output.
    pub fn attach(&mut self, m: &mut dyn ServerMod, seat: Seat) -> Delivery {
        self.host.facts.bodies.queue = HitQueue::new();
        let out = m.on_attach(&mut ServerCtx::new(&self.caps, &mut self.host), seat);
        self.finish(out)
    }

    /// Calls `on_detach` (the seat's client went away) and filters the output.
    pub fn detach(&mut self, m: &mut dyn ServerMod, seat: Seat) -> Delivery {
        self.host.facts.bodies.queue = HitQueue::new();
        let out = m.on_detach(&mut ServerCtx::new(&self.caps, &mut self.host), seat);
        self.finish(out)
    }

    /// One tick of the game actor at the current clock ([`advance_ms`](Self::advance_ms) moves it).
    pub fn tick(&mut self, m: &mut dyn ServerMod) -> Delivery {
        self.host.facts.bodies.queue = HitQueue::new();
        let now = self.host.now_ms;
        let out = m.on_tick(&mut ServerCtx::new(&self.caps, &mut self.host), now);
        self.finish(out)
    }

    /// Delivers a hit of mod `from_mod` on the mod's body `target_id`, as the host does after the hitting call.
    /// Hits queued inside are dropped with a warning (one level deep), as by the host.
    pub fn hit(
        &mut self,
        m: &mut dyn ServerMod,
        from_mod: &str,
        target_id: u32,
        hit: &Hit,
    ) -> Delivery {
        let from = ModId::new(from_mod).unwrap_or_else(|e| panic!("mod id `{from_mod}`: {e}"));
        self.host.facts.bodies.queue = HitQueue::nested();
        let out = m.on_hit(
            &mut ServerCtx::new(&self.caps, &mut self.host),
            &from,
            target_id,
            hit,
        );
        self.finish(out)
    }

    fn take_queue(&mut self) -> Vec<(BodyRef, Hit)> {
        let b = &mut self.host.facts.bodies;
        let (hits, warn) = std::mem::take(&mut b.queue).finish();
        b.hits.extend(hits.iter().cloned());
        if let Some(w) = warn {
            self.log_line(LogLevel::Warn, w);
        }
        hits
    }

    fn finish(&mut self, out: Vec<Outgoing>) -> Delivery {
        let hits = self.take_queue();
        let f = self.host.limiter.filter_outgoing(&self.caps, out);
        for d in &f.dropped {
            self.log_line(
                LogLevel::Warn,
                format!("dropped outgoing on channel {}: {:?}", d.ch, d.reason),
            );
        }
        Delivery {
            refused: None,
            sent: f.kept,
            dropped: f.dropped,
            hits,
        }
    }

    fn log_line(&mut self, level: LogLevel, msg: String) {
        self.host.logs.push((level, msg));
    }
}
