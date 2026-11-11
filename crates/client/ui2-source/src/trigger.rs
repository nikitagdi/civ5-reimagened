//! When to rebuild: the replica's change signals of this frame against the previous frame's.
//!
//! The comparison is a handful of integers; nothing here reads the game. A root marked here stays pending until
//! the replica can be read (live and caught up), then it is built once, however many batches came meanwhile.

use replica::{ReplicaStep, State};

/// A set of model roots, as bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Roots(u8);

impl Roots {
    pub const NONE: Roots = Roots(0);
    pub const GAME: Roots = Roots(1);
    pub const TOP: Roots = Roots(1 << 1);
    pub const RESEARCH: Roots = Roots(1 << 2);
    /// Every root built from the replica's game.
    pub const GAME_DATA: Roots = Roots(Self::GAME.0 | Self::TOP.0 | Self::RESEARCH.0);

    #[must_use]
    pub const fn union(self, other: Roots) -> Roots {
        Roots(self.0 | other.0)
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

/// The roots this frame's signals ask to rebuild.
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
        assert!(Roots::NONE.is_empty());
    }
}
