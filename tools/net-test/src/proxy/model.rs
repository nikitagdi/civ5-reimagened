//! The channel model of `netproxy`: when a chunk of bytes is let out. Pure functions on a
//! virtual clock in microseconds, so the model is testable without sockets and without waiting.

use serde::{Deserialize, Serialize};

/// The profile of the channel; every field can change on the fly (`POST /profile`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
    /// Delay in each direction (RTT = 2 x delay).
    pub delay_ms: u32,
    /// Jitter in each direction: the delay of a chunk is `delay_ms` plus a uniform value in `[-jitter, +jitter]`.
    pub jitter_ms: u32,
    /// Bandwidth per direction; 0 is unlimited.
    pub rate_kbit: u32,
    /// The loss model: with probability `stall_p` a chunk is held for a time in `stall_ms` (like a TCP retransmit).
    pub stall_p: f64,
    pub stall_ms: [u32; 2],
    /// The wait before the first byte of a new connection (TCP and TLS handshake); `None` is 2 x delay.
    pub connect_delay_ms: Option<u32>,
}

impl Default for Profile {
    fn default() -> Self {
        Profile {
            delay_ms: 0,
            jitter_ms: 0,
            rate_kbit: 0,
            stall_p: 0.0,
            stall_ms: [200, 1000],
            connect_delay_ms: None,
        }
    }
}

impl Profile {
    pub fn connect_delay(&self) -> u32 {
        self.connect_delay_ms.unwrap_or(self.delay_ms * 2)
    }

    /// The profile of the current one with the fields of `patch` (a JSON object) laid over it.
    pub fn patched(&self, patch: &serde_json::Value) -> Result<Profile, String> {
        let mut v = serde_json::to_value(self).map_err(|e| e.to_string())?;
        let (Some(dst), Some(src)) = (v.as_object_mut(), patch.as_object()) else {
            return Err("the profile is a JSON object".into());
        };
        for (k, x) in src {
            dst.insert(k.clone(), x.clone());
        }
        let p: Profile = serde_json::from_value(v).map_err(|e| e.to_string())?;
        if !(0.0..=1.0).contains(&p.stall_p) {
            return Err("stall_p is a probability in 0..1".into());
        }
        if p.stall_ms[0] > p.stall_ms[1] {
            return Err("stall_ms is [a, b] with a <= b".into());
        }
        Ok(p)
    }
}

/// SplitMix64: a small seeded generator, so a run with `--seed` repeats.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// A float in `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// An integer in `[a, b]`.
    pub fn range(&mut self, a: u64, b: u64) -> u64 {
        if b <= a {
            return a;
        }
        a + self.next_u64() % (b - a + 1)
    }
}

/// The queue of one direction. The release time of a chunk is
/// `max(previous release, now + delay + jitter [+ stall], the time the bandwidth bucket lets it out)`:
/// the order is kept, a late chunk holds the ones behind it (as TCP does).
#[derive(Debug, Clone, Default)]
pub struct Shaper {
    last_due_us: u64,
    /// When the line is free again (the token bucket at the exit).
    line_free_us: u64,
}

impl Shaper {
    /// The release time (microseconds on the clock of `now_us`) of a chunk of `bytes` that arrived at `now_us`.
    pub fn schedule(&mut self, now_us: u64, bytes: usize, p: &Profile, rng: &mut Rng) -> u64 {
        let mut delay_us = i64::from(p.delay_ms) * 1000;
        if p.jitter_ms > 0 {
            let j = i64::from(p.jitter_ms) * 1000;
            delay_us += rng.range(0, 2 * j as u64) as i64 - j;
        }
        let mut delay_us = delay_us.max(0) as u64;
        if p.stall_p > 0.0 && rng.unit() < p.stall_p {
            delay_us += rng.range(u64::from(p.stall_ms[0]), u64::from(p.stall_ms[1])) * 1000;
        }
        let mut due = (now_us + delay_us).max(self.last_due_us);
        if p.rate_kbit > 0 {
            // kbit/s is bit/ms: a byte takes 8000 / rate microseconds.
            let tx_us = (bytes as u64 * 8000) / u64::from(p.rate_kbit);
            self.line_free_us = self.line_free_us.max(now_us) + tx_us;
            due = due.max(self.line_free_us);
        }
        self.last_due_us = due;
        due
    }
}

/// The histogram of the delay a chunk got in the proxy (milliseconds), `GET /stats`.
pub const DELAY_EDGES_MS: [u64; 11] = [1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000];

pub fn delay_bucket(ms: u64) -> usize {
    DELAY_EDGES_MS
        .iter()
        .position(|e| ms < *e)
        .unwrap_or(DELAY_EDGES_MS.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(delay: u32) -> Profile {
        Profile {
            delay_ms: delay,
            ..Profile::default()
        }
    }

    #[test]
    fn delay_is_added_and_order_is_kept() {
        let mut s = Shaper::default();
        let mut rng = Rng::new(1);
        let prof = Profile {
            delay_ms: 100,
            jitter_ms: 50,
            ..Profile::default()
        };
        let mut last = 0;
        for i in 0..200 {
            let t = s.schedule(i * 1000, 100, &prof, &mut rng);
            assert!(t >= last, "order kept");
            assert!(t >= i * 1000 + 50_000, "at least delay - jitter");
            last = t;
        }
    }

    #[test]
    fn rate_limits_the_line() {
        let mut s = Shaper::default();
        let mut rng = Rng::new(1);
        let prof = Profile {
            rate_kbit: 800,
            ..p(0)
        };
        // 100 chunks of 1000 bytes at 800 kbit/s = 100 bytes/ms: 10 ms each, 1 s for all.
        let mut end = 0;
        for _ in 0..100 {
            end = s.schedule(0, 1000, &prof, &mut rng);
        }
        assert_eq!(end, 1_000_000);
    }

    #[test]
    fn stall_holds_the_chunks_behind() {
        let mut s = Shaper::default();
        let mut rng = Rng::new(7);
        let prof = Profile {
            stall_p: 1.0,
            stall_ms: [300, 300],
            ..p(0)
        };
        let a = s.schedule(0, 10, &prof, &mut rng);
        let calm = Profile {
            stall_p: 0.0,
            ..p(0)
        };
        let b = s.schedule(1000, 10, &calm, &mut rng);
        assert_eq!(a, 300_000);
        assert_eq!(b, 300_000, "a chunk behind a stalled one waits for it");
    }

    #[test]
    fn patch_overlays_fields() {
        let base = p(50);
        let q = base.patched(&serde_json::json!({"jitter_ms": 7})).unwrap();
        assert_eq!((q.delay_ms, q.jitter_ms), (50, 7));
        assert!(base.patched(&serde_json::json!({"nonsense": 1})).is_err());
        assert!(base.patched(&serde_json::json!({"stall_p": 2.0})).is_err());
        assert_eq!(q.connect_delay(), 100);
    }

    #[test]
    fn buckets() {
        assert_eq!(delay_bucket(0), 0);
        assert_eq!(delay_bucket(7), 3);
        assert_eq!(delay_bucket(5000), DELAY_EDGES_MS.len());
    }
}
