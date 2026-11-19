//! The UI v2 frame ring: where the main-thread time of each UI v2 frame went, phase by phase, for the playtest's
//! budget scenario (`ui2-perf`, `window.__civ5test.ui2Frames()`) and for anyone reading the counters on a live page.
//!
//! One record per UI v2 frame, filled in two steps: [`begin`] after `V2Ui::frame` (the host's report and the test
//! probe's time), [`finish`] after the renderer took the scene (the anchors' placement and the renderer's
//! `prepare_ui2`). The ring keeps the last [`CAPACITY`] frames; [`take`] reads it (and empties it on request). Cost: a
//! few stores per frame, no allocation once the ring is full.
//!
//! The phases (ms): `cpu` is the whole `V2Ui::frame` (it contains `source`, `runtime`, `resolve` and the scripts'
//! main-thread steps `js`); `runtime` is `Runtime::frame` (its passes `bind`, `style`, `text`, `layout`, `keys`, `paint`);
//! `input` is the time the runtime spent on the input events since the frame before (hit-testing, hover, dispatch: outside
//! `cpu`); `place` the projection of the map anchors, `prepare` the renderer's take of the scene (uploads), `probe` the
//! test hook's snapshot (0 without `?test=1`): all three outside `cpu`. `app` is the whole app frame before this one
//! (the renderer's CPU time of a frame: update, UI, encoding), for the comparison with the classic UI.
//!
//! The GPU side ([`Ui2Gpu`]): what the `Ui2` pass draws (draw calls, instances, the pixels its quads cover per frame
//! and of them the blurred box shadows and the textured quads) and the pass's GPU time from timestamp queries (a few
//! frames old; `None` where the device has none, e.g. WebGL2).

use std::cell::RefCell;
use std::collections::VecDeque;

use ui2_host::V2FrameReport;

/// Frames the ring keeps (about 17 s at 60 fps).
pub const CAPACITY: usize = 1024;

/// One UI v2 frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Ui2FrameTimes {
    /// The app's whole frame before this one, ms (`None` before the renderer has timed one).
    pub app: Option<f32>,
    pub cpu: f32,
    pub source: f32,
    pub runtime: f32,
    pub bind: f32,
    pub style: f32,
    pub text: f32,
    pub layout: f32,
    pub keys: f32,
    pub paint: f32,
    pub resolve: f32,
    pub js: f32,
    pub input: f32,
    pub place: f32,
    pub prepare: f32,
    pub probe: f32,
    /// Work counters of the frame.
    pub binds: u32,
    pub restyled: u32,
    pub layout_runs: u32,
    pub chunks: u32,
    pub visited: u32,
    pub ops: u32,
    pub input_events: u32,
    /// Bytes the renderer wrote to the GPU for UI v2 in this frame.
    pub upload_bytes: u64,
    /// What the pass drew.
    pub gpu: Ui2Gpu,
}

/// The GPU side of a UI v2 frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Ui2Gpu {
    /// Draw calls and instances of the `Ui2` pass.
    pub draws: u32,
    pub instances: u32,
    /// Thousands of pixels the quads cover (overdraw included), and of them blurred box shadows and textured quads
    /// (pictures, glyphs, icons).
    pub fill_kpx: u32,
    pub shadow_kpx: u32,
    pub texture_kpx: u32,
    /// GPU time of the pass, ms (timestamp queries; `None` without them).
    pub gpu_ms: Option<f32>,
}

thread_local! {
    static RING: RefCell<VecDeque<Ui2FrameTimes>> = RefCell::new(VecDeque::with_capacity(CAPACITY));
    /// The frame between `begin` and `finish`.
    static OPEN: RefCell<Option<Ui2FrameTimes>> = const { RefCell::new(None) };
}

/// The first half of a frame's record: the host's report and the probe's time.
pub fn begin(report: &V2FrameReport, probe_ms: f32) {
    let r = &report.runtime;
    let t = Ui2FrameTimes {
        cpu: report.cpu_ms,
        source: report.source_ms,
        runtime: r.cpu_ms,
        bind: r.bind_ms,
        style: r.style_ms,
        text: r.text_ms,
        layout: r.layout_ms,
        keys: r.keys_ms,
        paint: r.paint_ms,
        resolve: report.resolve_ms,
        js: report.js_main_ms,
        input: r.input_ms,
        probe: probe_ms,
        binds: r.binds_evaluated,
        restyled: r.restyled,
        layout_runs: r.layout_runs,
        chunks: r.chunks_rebuilt,
        visited: r.visited,
        ops: report.delta_ops,
        input_events: r.input_events,
        ..Ui2FrameTimes::default()
    };
    open(t);
}

/// Starts a frame's record from its first half.
fn open(t: Ui2FrameTimes) {
    OPEN.with(|o| *o.borrow_mut() = Some(t));
}

/// The second half: the placement, the renderer's take of the scene, the bytes it uploaded, what the pass draws and
/// the app's last frame. The record goes into the ring.
pub fn finish(place_ms: f32, prepare_ms: f32, upload_bytes: u64, gpu: Ui2Gpu, app_ms: Option<f32>) {
    let Some(mut t) = OPEN.with(|o| o.borrow_mut().take()) else {
        return;
    };
    t.place = place_ms;
    t.prepare = prepare_ms;
    t.upload_bytes = upload_bytes;
    t.gpu = gpu;
    t.app = app_ms;
    RING.with(|r| {
        let mut r = r.borrow_mut();
        if r.len() == CAPACITY {
            r.pop_front();
        }
        r.push_back(t);
    });
}

/// The frames in the ring, oldest first; `reset` empties it.
pub fn take(reset: bool) -> Vec<Ui2FrameTimes> {
    RING.with(|r| {
        let mut r = r.borrow_mut();
        if reset {
            r.drain(..).collect()
        } else {
            r.iter().copied().collect()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_needs_both_halves_and_the_ring_keeps_the_newest() {
        take(true);
        // A finish without a begin records nothing.
        finish(1.0, 1.0, 0, Ui2Gpu::default(), None);
        assert!(take(false).is_empty());
        for i in 0..CAPACITY + 5 {
            open(Ui2FrameTimes {
                cpu: i as f32,
                probe: 0.5,
                ..Ui2FrameTimes::default()
            });
            finish(
                0.25,
                0.75,
                96,
                Ui2Gpu {
                    draws: 3,
                    fill_kpx: 2073,
                    gpu_ms: Some(0.4),
                    ..Ui2Gpu::default()
                },
                Some(3.0),
            );
        }
        let all = take(true);
        assert_eq!(all.len(), CAPACITY);
        assert_eq!(all[0].cpu, 5.0, "the oldest frames left the ring");
        let last = all[CAPACITY - 1];
        assert_eq!(
            (
                last.probe,
                last.place,
                last.prepare,
                last.upload_bytes,
                last.app
            ),
            (0.5, 0.25, 0.75, 96, Some(3.0))
        );
        assert_eq!(
            (last.gpu.draws, last.gpu.fill_kpx, last.gpu.gpu_ms),
            (3, 2073, Some(0.4))
        );
        assert!(take(false).is_empty(), "reset emptied the ring");
    }
}
