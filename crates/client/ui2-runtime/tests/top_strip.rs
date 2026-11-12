//! The top strip through the frame as the host runs it: `drain_output`, `take_frame` → `apply_delta`,
//! `frame`, `Scene::resolve`. Once settled, frames without model changes do no work: no layout, no draw
//! list, no binding, no shaping, and the scene encodes nothing.

mod support;

use std::cell::Cell;
use std::rc::Rc;

use serde_json::json;
use ui2_draw::{LayerId, Scene};
use ui2_model::{ModelStore, Sample, Schema};
use ui2_runtime::{Env, FrameOutput, Runtime, Services, load_faces};
use ui2_text::TextEngine;

struct Host {
    rt: Runtime,
    engine: TextEngine,
    store: ModelStore,
    scene: Scene,
    now: f64,
}

impl Host {
    fn new() -> Self {
        let fx = support::load();
        let pkg = support::package_with(&fx, support::Texts::Bound);
        let mut engine = TextEngine::new();
        load_faces(&pkg, &mut engine).expect("fonts");
        let env = Env {
            viewport_px: (1920, 1200),
            ..Env::default()
        };
        Self {
            rt: Runtime::new(pkg, env, Services::default()).expect("runtime"),
            engine,
            store: ModelStore::new(Schema::v1()),
            scene: Scene::new(1.0),
            now: 0.0,
        }
    }

    /// Writes a sample state as the source would.
    fn write(&mut self, sample: &serde_json::Value) {
        let s = Sample::parse(Schema::v1(), &sample.to_string()).expect("sample");
        s.apply(&mut self.store).expect("apply");
    }

    /// One frame in the host's order.
    fn frame(&mut self) -> FrameOutput {
        self.now += 16.0;
        self.scene.reset_frame_stats();
        assert!(self.rt.drain_output().is_empty(), "no input, no output");
        if let Some(d) = self.store.take_frame() {
            self.rt.apply_delta(&d, &self.store);
        }
        let out = self
            .rt
            .frame(self.now, &self.store, &mut self.engine, &mut self.scene);
        self.scene.resolve(&mut self.engine, 1.5);
        out
    }
}

#[test]
fn top_strip_idle_frames_do_no_work() {
    let mut h = Host::new();
    let mut sample = support::sample_json();
    h.write(&sample);
    let first = h.frame();
    let s = first.stats;
    println!(
        "first frame: nodes {}, binds {}, shaped {}, layout runs {}, chunks {}, restyled {}",
        s.nodes, s.binds_evaluated, s.shaped_runs, s.layout_runs, s.chunks_rebuilt, s.restyled
    );
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    assert_eq!(s.binds_evaluated, 16);
    assert_eq!(s.layout_runs, 1);
    assert_eq!(s.chunks_rebuilt, 2);
    assert_eq!(s.shaped_runs, 15, "every text leaf once");
    assert!(!first.animating);

    // The glyphs reach the atlas within the budget of a few frames; the runtime does nothing meanwhile.
    for _ in 0..10 {
        let out = h.frame();
        assert_eq!(out.stats.chunks_rebuilt, 0);
        if h.scene.stats().raster_deferred == 0 {
            break;
        }
    }
    h.frame();
    let rev = h.scene.layer_revision(LayerId::Hud);
    for _ in 0..100 {
        let out = h.frame();
        let s = out.stats;
        assert_eq!(s.layout_runs, 0);
        assert_eq!(s.chunks_rebuilt, 0);
        assert_eq!(s.binds_evaluated, 0);
        assert_eq!(s.shaped_runs, 0);
        assert_eq!((s.restyled, s.visited), (0, 0));
        assert_eq!(s.nodes, first.stats.nodes);
        assert!(out.errors.is_empty());
        let scene = h.scene.stats();
        assert_eq!((scene.chunks_encoded, scene.raster_glyphs), (0, 0));
    }
    assert_eq!(h.scene.layer_revision(LayerId::Hud), rev);

    // One field of the model: one binding, one shaped run, no layout, one chunk.
    sample["model"]["top"]["gold"] = json!(313);
    h.write(&sample);
    let s = h.frame().stats;
    assert_eq!(
        (
            s.binds_evaluated,
            s.shaped_runs,
            s.layout_runs,
            s.chunks_rebuilt
        ),
        (1, 1, 0, 1)
    );
    // A write that changes nothing gives no frame of the store, and no work here.
    h.write(&sample);
    let s = h.frame().stats;
    assert_eq!((s.binds_evaluated, s.chunks_rebuilt, s.visited), (0, 0, 0));
}

#[test]
fn top_strip_frame_times_its_passes_with_the_host_clock() {
    let mut h = Host::new();
    let t = Rc::new(Cell::new(0.0f64));
    let tick = Rc::clone(&t);
    // Every reading advances the clock by a quarter millisecond.
    h.rt.set_clock(Box::new(move || {
        tick.set(tick.get() + 0.25);
        tick.get()
    }));
    h.write(&support::sample_json());
    let s = h.frame().stats;
    let parts = s.bind_ms + s.style_ms + s.text_ms + s.layout_ms + s.paint_ms;
    assert_eq!(s.cpu_ms, 1.25, "five passes");
    assert_eq!(parts, s.cpu_ms);
    // Without a clock the times stay zero.
    let mut plain = Host::new();
    plain.write(&support::sample_json());
    assert_eq!(plain.frame().stats.cpu_ms, 0.0);
    assert!(t.get() > 0.0);
}
