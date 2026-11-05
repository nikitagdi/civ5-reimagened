//! Gateway clock (GS-D7). All timers, deadlines and `server_ms` stamps read time through
//! [`Clock`], so tests drive them with [`TestClock`] instead of sleeping.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Monotonic milliseconds since an arbitrary start. Not a calendar time.
pub trait Clock: Send + Sync + 'static {
    /// Current monotonic time in milliseconds.
    fn now_ms(&self) -> u64;
}

/// Real clock: milliseconds since construction.
#[derive(Debug, Clone, Copy)]
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// Manually driven clock for tests. Starts at 0 and only moves on [`TestClock::advance`] or
/// [`TestClock::set`]; it never goes backwards.
#[derive(Debug, Default)]
pub struct TestClock {
    now: AtomicU64,
}

impl TestClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// Moves time forward by `ms`.
    pub fn advance(&self, ms: u64) {
        self.now.fetch_add(ms, Ordering::SeqCst);
    }

    /// Jumps to `ms`. A value in the past is ignored (the clock is monotonic).
    pub fn set(&self, ms: u64) {
        self.now.fetch_max(ms, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
}

/// Calendar time in Unix milliseconds. Only for data that outlives the process (identity
/// `last_seen_ms`); deadlines and timers use [`Clock`].
pub trait WallClock: Send + Sync + 'static {
    /// Milliseconds since the Unix epoch.
    fn unix_ms(&self) -> u64;
}

/// Real calendar time.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemWallClock;

impl WallClock for SystemWallClock {
    fn unix_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
    }
}

/// Manually driven calendar time for tests.
#[derive(Debug, Default)]
pub struct TestWallClock {
    now: AtomicU64,
}

impl TestWallClock {
    pub fn new(start_ms: u64) -> Self {
        Self {
            now: AtomicU64::new(start_ms),
        }
    }

    /// Moves time forward by `ms`.
    pub fn advance(&self, ms: u64) {
        self.now.fetch_add(ms, Ordering::SeqCst);
    }
}

impl WallClock for TestWallClock {
    fn unix_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clock_moves_only_forward() {
        let c = TestClock::new();
        assert_eq!(c.now_ms(), 0);
        c.advance(1500);
        assert_eq!(c.now_ms(), 1500);
        c.set(1000);
        assert_eq!(c.now_ms(), 1500);
        c.set(4000);
        assert_eq!(c.now_ms(), 4000);
    }

    #[test]
    fn system_clock_is_monotonic() {
        let c = SystemClock::new();
        let a = c.now_ms();
        let b = c.now_ms();
        assert!(b >= a);
    }

    #[test]
    fn usable_as_trait_object() {
        let c: std::sync::Arc<dyn Clock> = std::sync::Arc::new(TestClock::new());
        assert_eq!(c.now_ms(), 0);
    }
}
