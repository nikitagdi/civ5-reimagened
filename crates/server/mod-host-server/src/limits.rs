//! Repeated-violation tracking (D7). `mod_api::Limiter` decides whether one message passes; this decides when a
//! seat that keeps failing is to be disconnected. The host acts on the signal, this module only counts.

use std::collections::{BTreeMap, VecDeque};

use mod_api::Seat;

/// Violations of one seat inside [`STRIKE_WINDOW_MS`] that trip the disconnect signal.
pub const MAX_STRIKES: usize = 10;
/// The sliding window of the count, in milliseconds.
pub const STRIKE_WINDOW_MS: u64 = 10_000;

/// Violation times per seat. A violation is a message the host dropped before the mod saw it (unknown mod, channel
/// or oversize payload, empty token bucket).
#[derive(Debug, Clone, Default)]
pub struct Strikes {
    times: BTreeMap<Seat, VecDeque<u64>>,
}

impl Strikes {
    /// Records one violation at `now_ms`. Returns true when the seat reached [`MAX_STRIKES`] inside the window; its
    /// count then starts again, so a host that keeps the seat connected gets the signal again after another
    /// [`MAX_STRIKES`] violations.
    pub fn hit(&mut self, seat: Seat, now_ms: u64) -> bool {
        let q = self.times.entry(seat).or_default();
        while q
            .front()
            .is_some_and(|t| now_ms.saturating_sub(*t) > STRIKE_WINDOW_MS)
        {
            q.pop_front();
        }
        q.push_back(now_ms);
        if q.len() >= MAX_STRIKES {
            self.times.remove(&seat);
            true
        } else {
            false
        }
    }

    /// The seat left the game.
    pub fn forget(&mut self, seat: Seat) {
        self.times.remove(&seat);
    }

    /// Violations currently counted for a seat.
    pub fn count(&self, seat: Seat) -> usize {
        self.times.get(&seat).map_or(0, VecDeque::len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trips_at_the_limit_and_restarts() {
        let mut s = Strikes::default();
        let seat = Seat(1);
        for i in 0..MAX_STRIKES - 1 {
            assert!(!s.hit(seat, i as u64), "strike {i}");
        }
        assert_eq!(s.count(seat), MAX_STRIKES - 1);
        assert!(s.hit(seat, 100));
        assert_eq!(s.count(seat), 0);
        assert!(!s.hit(seat, 101));
    }

    #[test]
    fn old_violations_expire() {
        let mut s = Strikes::default();
        let seat = Seat(2);
        for i in 0..MAX_STRIKES - 1 {
            s.hit(seat, i as u64);
        }
        // Far later the old ones no longer count.
        assert!(!s.hit(seat, STRIKE_WINDOW_MS + 1_000));
        assert_eq!(s.count(seat), 1);
    }

    #[test]
    fn seats_are_independent_and_forgettable() {
        let mut s = Strikes::default();
        for i in 0..MAX_STRIKES - 1 {
            s.hit(Seat(0), i as u64);
        }
        assert!(!s.hit(Seat(1), 0));
        s.forget(Seat(0));
        assert_eq!(s.count(Seat(0)), 0);
        assert!(!s.hit(Seat(0), 1));
    }
}
