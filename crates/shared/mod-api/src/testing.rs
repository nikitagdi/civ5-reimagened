//! Test doubles for mod unit tests: [`MockFacts`] (a settable map) and [`MockCtx`] (a host stand-in that applies the
//! same checks the real host does: channel, size, rate, capabilities).

use mod_manifest::Channel;

use crate::caps::Caps;
use crate::facts::{MapShape, Owner, PlotClass, PlotIdx, WorldFacts};
use crate::ids::{ChannelId, PlayerId, Seat};
use crate::limits::{Dropped, Limiter, Verdict};
use crate::server::{HostServices, LogLevel, Outgoing, ServerCtx, ServerMod};

/// A map with settable owners and classes. Every plot starts as unowned `Land`.
#[derive(Debug, Clone)]
pub struct MockFacts {
    shape: MapShape,
    turn: i32,
    owners: Vec<Owner>,
    classes: Vec<PlotClass>,
    seats: Vec<(Seat, PlayerId)>,
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

/// The [`HostServices`] of [`MockCtx`]: public fields so tests can set the clock and read the log.
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
}

/// What the mock host delivered for one client message.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Delivery {
    /// The host refused the message before the mod saw it.
    pub refused: Option<Verdict>,
    /// Messages that passed the capability and size checks.
    pub sent: Vec<Outgoing>,
    pub dropped: Vec<Dropped>,
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

    pub fn cx(&mut self) -> ServerCtx<'_> {
        ServerCtx::new(&self.caps, &mut self.host)
    }

    pub fn advance_ms(&mut self, ms: u64) {
        self.host.now_ms += ms;
    }

    pub fn facts_mut(&mut self) -> &mut MockFacts {
        &mut self.host.facts
    }

    /// Calls `start` with the saved bytes.
    pub fn start(&mut self, m: &mut dyn ServerMod, saved: Option<&[u8]>) {
        m.start(&mut ServerCtx::new(&self.caps, &mut self.host), saved);
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
        let out = m.on_attach(&mut ServerCtx::new(&self.caps, &mut self.host), seat);
        self.finish(out)
    }

    fn finish(&mut self, out: Vec<Outgoing>) -> Delivery {
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
        }
    }

    fn log_line(&mut self, level: LogLevel, msg: String) {
        self.host.logs.push((level, msg));
    }
}
