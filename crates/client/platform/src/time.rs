//! Clock.

/// Monotonic client clock.
pub trait Clock {
    /// Milliseconds since an arbitrary origin. Never decreases; not calendar time.
    fn now_ms(&self) -> u64;

    /// The same clock with the fraction of a millisecond, for timing a few microseconds of work
    /// (the pass times of `?stats=1`). The default has whole milliseconds only.
    fn now_ms_f64(&self) -> f64 {
        self.now_ms() as f64
    }
}
