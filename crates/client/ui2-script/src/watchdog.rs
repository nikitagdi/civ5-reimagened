//! The watchdog of a script context: how long its worker has kept a frame unanswered, and how often it failed.
//!
//! A frame the worker has not acknowledged for [`BUSY_AFTER_MS`] makes the context busy: further frames are held
//! back and merged. Unanswered for [`KILL_AFTER_MS`], the worker is killed and a new one starts from a fresh
//! snapshot. A worker that has not said `ready` within [`READY_TIMEOUT_MS`] of its start is killed the same way
//! (loading includes fetching the loader, so it gets longer). [`MAX_FAILURES`] kills or failed loads within
//! [`FAILURE_WINDOW_MS`] switch the context off until the skin changes.
//!
//! SIMPLIFIED(SIMP-262): a hanging handler is seen only by the missing acknowledgement of its frame (250 ms, then
//! the worker is killed and restarted), not by the time of the call itself: a worker cannot interrupt a handler
//! without being killed.

use std::collections::VecDeque;

/// Unanswered this long, a frame makes its context busy.
pub const BUSY_AFTER_MS: f64 = 250.0;
/// Unanswered this long, a frame gets its worker killed and restarted.
pub const KILL_AFTER_MS: f64 = 1000.0;
/// A started worker that has not said `ready` this long after its start is killed.
pub const READY_TIMEOUT_MS: f64 = 10_000.0;
/// This many kills or failed loads within [`FAILURE_WINDOW_MS`] switch a context off.
pub const MAX_FAILURES: usize = 3;
pub const FAILURE_WINDOW_MS: f64 = 60_000.0;

/// What the watchdog says about a context now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Verdict {
    /// Nothing waits, or not for long.
    Fine,
    /// The oldest unanswered frame has waited this long (at least [`BUSY_AFTER_MS`]).
    Busy(f64),
    /// Kill the worker: a frame waited this long, or the start did (`loading`).
    Kill { waited_ms: f64, loading: bool },
}

#[derive(Debug, Default)]
pub(crate) struct Watchdog {
    /// `(seq, sent at)` of the frames not acknowledged yet, oldest first.
    inflight: VecDeque<(u64, f64)>,
    /// The worker was started and has not said `ready`: since when (set by the first look at the clock).
    loading: bool,
    loading_since: Option<f64>,
    /// When the recent kills and failed loads happened, oldest first.
    failures: VecDeque<f64>,
}

impl Watchdog {
    /// A new worker was started: nothing of the old one is waited for.
    pub fn started(&mut self) {
        self.inflight.clear();
        self.loading = true;
        self.loading_since = None;
    }

    /// The worker said `ready`. Frames sent while it loaded count from now: they could not be answered before.
    pub fn ready(&mut self, now_ms: f64) {
        self.loading = false;
        self.loading_since = None;
        for f in &mut self.inflight {
            f.1 = f.1.max(now_ms);
        }
    }

    /// A frame was sent.
    pub fn sent(&mut self, seq: u64, now_ms: f64) {
        self.inflight.push_back((seq, now_ms));
    }

    /// The worker answered every frame up to `ack`.
    pub fn acked(&mut self, ack: u64) {
        while self.inflight.front().is_some_and(|f| f.0 <= ack) {
            self.inflight.pop_front();
        }
    }

    /// Whether frames wait for an answer.
    pub fn waiting(&self) -> bool {
        !self.inflight.is_empty()
    }

    /// The verdict at `now_ms`.
    pub fn check(&mut self, now_ms: f64) -> Verdict {
        if self.loading {
            let since = *self.loading_since.get_or_insert(now_ms);
            let waited_ms = now_ms - since;
            return if waited_ms >= READY_TIMEOUT_MS {
                Verdict::Kill {
                    waited_ms,
                    loading: true,
                }
            } else {
                Verdict::Fine
            };
        }
        let Some(&(_, sent)) = self.inflight.front() else {
            return Verdict::Fine;
        };
        let waited_ms = now_ms - sent;
        if waited_ms >= KILL_AFTER_MS {
            Verdict::Kill {
                waited_ms,
                loading: false,
            }
        } else if waited_ms >= BUSY_AFTER_MS {
            Verdict::Busy(waited_ms)
        } else {
            Verdict::Fine
        }
    }

    /// A kill or a failed load at `now_ms`; `true` when the context must be switched off.
    pub fn failure(&mut self, now_ms: f64) -> bool {
        while self
            .failures
            .front()
            .is_some_and(|t| now_ms - *t >= FAILURE_WINDOW_MS)
        {
            self.failures.pop_front();
        }
        self.failures.push_back(now_ms);
        self.failures.len() >= MAX_FAILURES
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_then_kill_by_the_oldest_unanswered_frame() {
        let mut w = Watchdog::default();
        w.ready(0.0);
        w.sent(1, 0.0);
        w.sent(2, 100.0);
        assert_eq!(w.check(249.0), Verdict::Fine);
        assert_eq!(w.check(250.0), Verdict::Busy(250.0));
        w.acked(1);
        assert_eq!(w.check(300.0), Verdict::Fine, "frame 2 waited 200 ms");
        assert_eq!(
            w.check(1100.0),
            Verdict::Kill {
                waited_ms: 1000.0,
                loading: false
            }
        );
        w.acked(2);
        assert!(!w.waiting());
        assert_eq!(w.check(5000.0), Verdict::Fine);
    }

    #[test]
    fn loading_has_its_own_timeout_and_frames_count_from_ready() {
        let mut w = Watchdog::default();
        w.started();
        w.sent(1, 10.0);
        assert_eq!(w.check(20.0), Verdict::Fine);
        assert_eq!(w.check(5000.0), Verdict::Fine);
        w.ready(6000.0);
        assert_eq!(w.check(6100.0), Verdict::Fine);
        assert_eq!(w.check(6300.0), Verdict::Busy(300.0));
        w.started();
        assert_eq!(w.check(7000.0), Verdict::Fine);
        assert!(matches!(
            w.check(17_000.0),
            Verdict::Kill { loading: true, .. }
        ));
    }

    #[test]
    fn three_failures_within_a_minute_switch_off() {
        let mut w = Watchdog::default();
        assert!(!w.failure(0.0));
        assert!(!w.failure(30_000.0));
        // The first one is out of the window.
        assert!(!w.failure(61_000.0));
        assert!(w.failure(62_000.0));
    }
}
