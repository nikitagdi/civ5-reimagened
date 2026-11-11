//! When to rebuild: the change signals of this frame (replica, session, the UI's own state) against the previous
//! frame's.
//!
//! The comparisons are a handful of integers and a few small values; nothing here reads the game. A root marked
//! here stays pending until it can be built (the replica live and caught up), then it is built once, however
//! many signals came meanwhile.

use protocol::{PlayerId, SeatView, TurnState};
use replica::{ReplicaStep, State};
use ui2_model::r#gen::Timer;

use crate::session::{SessionRead, timer};

/// A set of model roots (and the timer patch of `turn`), as bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Roots(u16);

impl Roots {
    pub const NONE: Roots = Roots(0);
    pub const GAME: Roots = Roots(1);
    pub const TOP: Roots = Roots(1 << 1);
    pub const RESEARCH: Roots = Roots(1 << 2);
    pub const PLAYERS: Roots = Roots(1 << 3);
    pub const TURN: Roots = Roots(1 << 4);
    /// Only `turn.timer` (a new second): written from the last `turn`, without reading the game.
    pub const TIMER: Roots = Roots(1 << 5);
    /// Every root built from the replica's game: what a journal batch or a new snapshot rebuilds.
    pub const GAME_DATA: Roots =
        Roots(Self::GAME.0 | Self::TOP.0 | Self::RESEARCH.0 | Self::PLAYERS.0 | Self::TURN.0);
    /// What a change of the session's turn state or seats rebuilds.
    pub const SESSION_TURN: Roots = Roots(Self::GAME.0 | Self::TURN.0);

    /// The roots that need the replica's game, in the order a frame builds them (the budget queue).
    pub const QUEUE: [Roots; 5] = [
        Self::TOP,
        Self::RESEARCH,
        Self::GAME,
        Self::TURN,
        Self::PLAYERS,
    ];

    #[must_use]
    pub const fn union(self, other: Roots) -> Roots {
        Roots(self.0 | other.0)
    }

    #[must_use]
    pub const fn without(self, other: Roots) -> Roots {
        Roots(self.0 & !other.0)
    }

    #[must_use]
    pub const fn contains(self, other: Roots) -> bool {
        self.0 & other.0 == other.0
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The number of roots in the set.
    #[must_use]
    pub const fn len(self) -> u32 {
        self.0.count_ones()
    }
}

/// What the source remembers of the replica between frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    pub state: State,
    pub catching_up: bool,
    pub generation: u64,
    pub applied_jseq: u64,
}

impl Seen {
    /// The game may be read now: the replica follows the journal and has no batches waiting.
    #[must_use]
    pub fn readable(&self) -> bool {
        self.state == State::Live && !self.catching_up
    }
}

/// The roots the replica's signals of this frame ask to rebuild.
///
/// - a first look at a replica, a new generation (snapshot load, resync) or a `Loaded`/`TurnSwitched` step:
///   everything;
/// - a new applied batch (`applied_jseq` moved, or an `Applied` step): every game-data root, since the core does
///   not say what a batch changed;
/// - otherwise nothing.
#[must_use]
pub fn changes(prev: Option<&Seen>, now: &Seen, steps: &[ReplicaStep]) -> Roots {
    let Some(prev) = prev else {
        return Roots::GAME_DATA;
    };
    let mut roots = Roots::NONE;
    if prev.generation != now.generation {
        roots = roots.union(Roots::GAME_DATA);
    }
    if prev.applied_jseq != now.applied_jseq {
        roots = roots.union(Roots::GAME_DATA);
    }
    for s in steps {
        match s {
            ReplicaStep::Loaded { .. }
            | ReplicaStep::TurnSwitched { .. }
            | ReplicaStep::Applied { .. } => roots = roots.union(Roots::GAME_DATA),
            ReplicaStep::Idle | ReplicaStep::Working | ReplicaStep::Resyncing => {}
        }
    }
    roots
}

/// What the source remembers of the session between frames. A value is cloned only when it changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionSeen {
    pub me: Option<PlayerId>,
    pub turn: Option<TurnState>,
    pub seats: Vec<SeatView>,
    pub timer: Option<Timer>,
}

impl SessionSeen {
    /// Compares the session with what was seen, remembers the new values and says what to rebuild:
    ///
    /// - our seat, the turn state (phase, mode, who ended the turn, the timers) or the seats' connections changed:
    ///   `game` and `turn`;
    /// - only the second shown on our timer changed: the timer patch of `turn`.
    pub fn update(&mut self, s: &dyn SessionRead, now_ms: u64) -> Roots {
        let mut roots = Roots::NONE;
        if self.me != s.me() {
            self.me = s.me();
            roots = roots.union(Roots::SESSION_TURN);
        }
        if self.turn.as_ref() != s.turn_state() {
            self.turn = s.turn_state().cloned();
            roots = roots.union(Roots::SESSION_TURN);
        }
        if self.seats.as_slice() != s.seats() {
            self.seats = s.seats().to_vec();
            roots = roots.union(Roots::TURN);
        }
        let t = timer(s, now_ms);
        if self.timer != t {
            self.timer = t;
            roots = roots.union(Roots::TIMER);
        }
        roots
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(generation: u64, applied_jseq: u64) -> Seen {
        Seen {
            state: State::Live,
            catching_up: false,
            generation,
            applied_jseq,
        }
    }

    #[test]
    fn nothing_changed_is_no_root() {
        let a = seen(1, 7);
        assert_eq!(changes(Some(&a), &a, &[]), Roots::NONE);
        assert_eq!(
            changes(Some(&a), &a, &[ReplicaStep::Idle, ReplicaStep::Working]),
            Roots::NONE
        );
    }

    #[test]
    fn every_signal_rebuilds_the_game_data() {
        let a = seen(1, 7);
        assert_eq!(changes(None, &a, &[]), Roots::GAME_DATA);
        assert_eq!(changes(Some(&a), &seen(2, 7), &[]), Roots::GAME_DATA);
        assert_eq!(changes(Some(&a), &seen(1, 8), &[]), Roots::GAME_DATA);
        for step in [
            ReplicaStep::Loaded { turn: 3 },
            ReplicaStep::TurnSwitched { turn: 4 },
            ReplicaStep::Applied { jseq: 8 },
        ] {
            assert_eq!(changes(Some(&a), &a, &[step]), Roots::GAME_DATA, "{step:?}");
        }
    }

    #[test]
    fn roots_set_operations() {
        let r = Roots::TOP.union(Roots::GAME);
        assert!(r.contains(Roots::TOP));
        assert!(!r.contains(Roots::RESEARCH));
        assert_eq!(r.len(), 2);
        assert_eq!(r.without(Roots::TOP), Roots::GAME);
        assert!(Roots::NONE.is_empty());
        let queued = Roots::QUEUE.iter().fold(Roots::NONE, |a, r| a.union(*r));
        assert_eq!(
            queued,
            Roots::GAME_DATA,
            "every game-data root has a place in the queue"
        );
    }
}
