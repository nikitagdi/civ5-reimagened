//! When to rebuild: the change signals of this frame (replica, session, the UI's own state) against the previous
//! frame's.
//!
//! The comparisons are a handful of integers and a few small values; nothing here reads the game. A root marked
//! here stays pending until it can be built (the world live and caught up), then it is built once, however
//! many signals came meanwhile.

use protocol::{PlayerId, SeatView, TurnState};
use ui2_model::r#gen::{Pending, Timer, Ui, WinCity};
use ui2_model::{Key, KeyedList};

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
    pub const MAP: Roots = Roots(1 << 6);
    pub const NOTES: Roots = Roots(1 << 7);
    /// Built from the session only.
    pub const CHAT: Roots = Roots(1 << 8);
    pub const UNIT: Roots = Roots(1 << 9);
    pub const PRODUCTION: Roots = Roots(1 << 10);
    /// Every root built from the world's game: what a journal batch or a new snapshot rebuilds.
    pub const GAME_DATA: Roots = Roots(
        Self::GAME.0
            | Self::TOP.0
            | Self::RESEARCH.0
            | Self::PLAYERS.0
            | Self::TURN.0
            | Self::MAP.0
            | Self::NOTES.0
            | Self::UNIT.0
            | Self::PRODUCTION.0,
    );
    /// What a change of the session's turn state or seats rebuilds.
    pub const SESSION_TURN: Roots = Roots(Self::GAME.0 | Self::TURN.0);

    /// The roots that need the world's game, in the order a frame builds them (the budget queue): the
    /// [`PRIORITY`](Self::PRIORITY) roots, then the others (a frame takes those the longest waiting first, ties in
    /// this order).
    pub const QUEUE: [Roots; 9] = [
        Self::TURN,
        Self::GAME,
        Self::TOP,
        Self::UNIT,
        Self::RESEARCH,
        Self::NOTES,
        Self::MAP,
        Self::PLAYERS,
        Self::PRODUCTION,
    ];

    /// The roots always on screen and cheap to build (the turn dial and button, the top strip, the selected
    /// unit): built in the frame of their change with all their reads, whatever is left of the budget.
    pub const PRIORITY: [Roots; 4] = [Self::TURN, Self::GAME, Self::TOP, Self::UNIT];

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

    /// The set as bits (the constants above).
    #[must_use]
    pub const fn bits(self) -> u16 {
        self.0
    }

    /// The bit number of a single root (its slot in per-root tables).
    #[must_use]
    pub const fn index(self) -> usize {
        self.0.trailing_zeros() as usize
    }
}

/// The last world revision read by the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    pub revision: u64,
}

/// A new shown-world revision invalidates the game-data roots.
#[must_use]
pub fn changes(prev: Option<&Seen>, now: &Seen) -> Roots {
    if prev == Some(now) {
        Roots::NONE
    } else {
        Roots::GAME_DATA
    }
}

/// What the source remembers of the session between frames. A value is cloned only when it changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionSeen {
    pub me: Option<PlayerId>,
    pub turn: Option<TurnState>,
    pub seats: Vec<SeatView>,
    pub timer: Option<Timer>,
    pub chat_total: u64,
}

impl SessionSeen {
    /// Compares the session with what was seen, remembers the new values and says what to rebuild:
    ///
    /// - our seat, the turn state (phase, mode, who ended the turn, the timers) or the seats' connections changed:
    ///   `game` and `turn`;
    /// - a chat line came (or our seat changed: "mine" is ours): `chat`;
    /// - only the second shown on our timer changed: the timer patch of `turn`.
    pub fn update(&mut self, s: &dyn SessionRead, now_ms: u64) -> Roots {
        let mut roots = Roots::NONE;
        if self.me != s.me() {
            self.me = s.me();
            roots = roots.union(Roots::SESSION_TURN).union(Roots::CHAT);
        }
        if self.chat_total != s.chat_total() {
            self.chat_total = s.chat_total();
            roots = roots.union(Roots::CHAT);
        }
        if self.turn.as_ref() != s.turn_state() {
            self.turn = s.turn_state().cloned();
            roots = roots.union(Roots::SESSION_TURN);
        }
        if self.seats.as_slice() != s.seats() {
            if self
                .seats
                .iter()
                .map(|s| (s.player, &s.control))
                .ne(s.seats().iter().map(|s| (s.player, &s.control)))
            {
                roots = roots.union(Roots::PLAYERS);
            }
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

/// What the source remembers of the UI's own root between frames: the parts its roots depend on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiSeen {
    pub unit: Option<Key>,
    pub production: WinCity,
    pub focus: Option<Key>,
    pub pending: KeyedList<Pending>,
}

impl UiSeen {
    /// Compares the UI root with what was seen, remembers the new values and says what to rebuild:
    ///
    /// - a new selected unit: `unit`;
    /// - the production window opened, closed or shows another city, or another option is focused: `production`;
    /// - the orders waiting for an answer changed (`unit.pending`, `Opt.pending`): `unit` and `production`.
    pub fn update(&mut self, ui: &Ui) -> Roots {
        let mut roots = Roots::NONE;
        if self.unit != ui.selection.unit {
            self.unit = ui.selection.unit;
            roots = roots.union(Roots::UNIT);
        }
        if self.production != ui.windows.production {
            self.production.clone_from(&ui.windows.production);
            roots = roots.union(Roots::PRODUCTION);
        }
        if self.focus != ui.production.focus {
            self.focus = ui.production.focus;
            roots = roots.union(Roots::PRODUCTION);
        }
        if self.pending != ui.pending {
            self.pending.clone_from(&ui.pending);
            roots = roots.union(Roots::UNIT).union(Roots::PRODUCTION);
        }
        roots
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revisions_trigger_once() {
        let a = Seen { revision: 7 };
        assert_eq!(changes(None, &a), Roots::GAME_DATA);
        assert_eq!(changes(Some(&a), &a), Roots::NONE);
        assert_eq!(changes(Some(&a), &Seen { revision: 8 }), Roots::GAME_DATA);
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
