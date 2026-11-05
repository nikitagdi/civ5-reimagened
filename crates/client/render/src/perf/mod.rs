//! Instruments of the client (build B3, `perf-hud`): frame time, GPU and RAM accounting, draw
//! statistics and the snapshot that the HUD, `window.__civ5stats` and the desktop log all read.

pub mod draw;
pub mod frame;
pub mod gpu_mem;
pub mod groups;
pub mod ram;
pub mod slow;
pub mod snapshot;
pub mod state;

pub use draw::{DrawCounts, PassCount};
pub use frame::{FrameRing, FrameSample, FrameSummary, Verdict};
pub use gpu_mem::{Buf, Cluster, DeviceTrack, GpuMemSnapshot, Tex, Tracked};
pub use groups::{Group, GroupRow, GroupTally};
pub use ram::{CountingAlloc, RamScope, RamSnapshot, RamTag, ram_scope};
pub use slow::SlowFrame;
pub use snapshot::{PassRow, PerfSnapshot};
pub use state::PassState;

use std::collections::BTreeMap;
use std::sync::Mutex;

/// Counters that other layers publish by name (`net.ws_in_bps`, `assets.inflight`,
/// `storage.used`, `ram.js_heap_used`): the prefix says the group. They appear in the HUD and in
/// `window.__civ5stats.counters` next to the counters of the renderer.
static EXTRA: Mutex<BTreeMap<String, f64>> = Mutex::new(BTreeMap::new());

/// Sets the counter `name` (replacing the old value).
pub fn set_counter(name: &str, value: f64) {
    if let Ok(mut m) = EXTRA.lock() {
        match m.get_mut(name) {
            Some(v) => *v = value,
            None => {
                m.insert(name.to_owned(), value);
            }
        }
    }
}

/// All counters set by [`set_counter`], sorted by name.
pub fn extra_counters() -> Vec<(String, f64)> {
    EXTRA
        .lock()
        .map(|m| m.iter().map(|(k, v)| (k.clone(), *v)).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_counters_replace_and_sort() {
        set_counter("zz_test.b", 2.0);
        set_counter("zz_test.a", 1.0);
        set_counter("zz_test.b", 3.0);
        let all: Vec<_> = extra_counters()
            .into_iter()
            .filter(|(k, _)| k.starts_with("zz_test."))
            .collect();
        assert_eq!(
            all,
            vec![("zz_test.a".to_owned(), 1.0), ("zz_test.b".to_owned(), 3.0)]
        );
    }

    /// AC-H9, the always-on part (hidden panel): what one frame adds with the ring, the command
    /// counters and the once-per-second snapshot. Run: `cargo test -p render --release --lib
    /// hidden_overhead -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement, not a check"]
    fn hidden_overhead_per_frame() {
        use std::time::Instant;
        let mut ring = FrameRing::default();
        let frames = 200_000u32;
        let t = Instant::now();
        for i in 0..frames {
            // One frame: ~400 draw calls worth of counters, one ring sample, two counts() reads.
            let before = draw::counts();
            for _ in 0..400 {
                std::hint::black_box(draw::counts());
            }
            let _ = std::hint::black_box(draw::counts().since(before));
            ring.push(FrameSample {
                at_ms: f64::from(i) * 6.9,
                dt_ms: 6.9,
                cpu_ms: 3.0,
                gpu_ms: Some(4.0),
            });
        }
        let per_frame_us = t.elapsed().as_secs_f64() * 1e6 / f64::from(frames);
        let t = Instant::now();
        for _ in 0..1000 {
            std::hint::black_box(ring.summary());
        }
        let summary_us = t.elapsed().as_secs_f64() * 1e6 / 1000.0;
        println!(
            "hidden panel: ring + counters {per_frame_us:.2} us/frame; summary() {summary_us:.0} us once a second"
        );
        assert!(per_frame_us < 100.0, "AC-H9: hidden < 0.1 ms per frame");
    }
}
