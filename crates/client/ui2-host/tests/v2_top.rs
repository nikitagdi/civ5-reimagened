//! The whole first-slice chain through `V2Ui::frame`: fake core values → source → model → bound text → glyph
//! runs in the scene; then frames without a change do no work anywhere, and one changed value redoes one binding.

mod support;

use std::cell::Cell;
use std::rc::Rc;

use replica::testing::{FakeRead, catalog_entry};
use replica::{CatalogTable, ReplicaStep, State};
use ui2_draw::{Item, LayerId};
use ui2_host::{Env, ReplicaView, V2Frame, V2FrameReport, V2Init, V2Ui};
use ui2_text::{FaceId, RunStyle, TextEngine};

const MEDIEVAL: i16 = 2;
const EDUCATION: i16 = 20;

/// Turn 87, 312 gold, +48 science, researching Education (3 turns).
fn game() -> FakeRead {
    let mut f = FakeRead::new(0, 0);
    f.info.turn = 87;
    f.info.year = 1250;
    let t = &mut f.top;
    t.science_per_turn = 48;
    t.gold = 312;
    t.gold_per_turn = 21;
    t.era = MEDIEVAL;
    t.research_tech = Some(EDUCATION);
    t.research_turns = 3;
    t.research_progress = 310;
    t.research_cost = 500;
    f.catalogs = vec![
        (
            CatalogTable::Eras,
            vec![catalog_entry(
                MEDIEVAL,
                "ERA_MEDIEVAL",
                "Medieval",
                "ERA_ATLAS",
                2,
            )],
        ),
        (
            CatalogTable::Technologies,
            vec![catalog_entry(
                EDUCATION,
                "TECH_EDUCATION",
                "Education",
                "TECH_ATLAS_1",
                24,
            )],
        ),
    ];
    f
}

fn env() -> Env {
    Env {
        viewport_px: (1280, 720),
        ..Env::default()
    }
}

struct Rig {
    ui: V2Ui,
    game: FakeRead,
    generation: u64,
    jseq: u64,
    now: f64,
}

impl Rig {
    fn new() -> Rig {
        let mut ui = V2Ui::new(
            support::strip(),
            V2Init {
                env: env(),
                ..V2Init::default()
            },
        )
        .expect("the skin loads");
        // These tests look at one frame's model: every root is built in the first frame.
        ui.set_read_budget(u32::MAX);
        Rig {
            ui,
            game: game(),
            generation: 1,
            jseq: 0,
            now: 0.0,
        }
    }

    fn frame(&mut self, steps: &[ReplicaStep]) -> V2FrameReport {
        self.now += 16.0;
        let view = ReplicaView {
            state: State::Live,
            catching_up: false,
            generation: self.generation,
            applied_jseq: self.jseq,
            read: Some(&mut self.game),
        };
        self.ui.frame(V2Frame {
            replica: Some(view),
            steps,
            net: None,
            events: &[],
            mods: None,
            now_ms: self.now,
        })
    }

    /// The glyph ids of every glyph run the runtime paints, in paint order.
    fn painted_runs(&mut self) -> Vec<Vec<u32>> {
        let rt = self.ui.runtime_mut();
        let mut chunks: Vec<_> = rt
            .arena()
            .preorder()
            .into_iter()
            .filter_map(|id| rt.node(id).map(|n| n.chunk()))
            .collect();
        chunks.dedup();
        let mut runs = Vec::new();
        for c in chunks {
            let Some((layer, items)) = rt.chunk_items(c) else {
                continue;
            };
            assert_eq!(layer, LayerId::Hud);
            for item in items.items {
                if let Item::Glyphs(g) = item {
                    runs.push(g.glyphs.iter().map(|p| p.glyph).collect());
                }
            }
        }
        runs
    }
}

/// The glyph ids of a string in the strip's face.
fn shaped(texts: &[&str]) -> Vec<Vec<u32>> {
    let mut engine = TextEngine::new();
    ui2_runtime::load_faces(&support::strip(), &mut engine).expect("fonts");
    texts
        .iter()
        .map(|t| {
            engine
                .shape(&RunStyle::new(FaceId(0), 14.0), t)
                .expect("shape")
                .glyphs
                .iter()
                .map(|g| g.glyph_id)
                .collect()
        })
        .collect()
}

#[test]
fn fake_core_values_reach_the_scene_as_glyphs() {
    let mut rig = Rig::new();
    let load = rig.ui.load_stats();
    assert_eq!((load.fonts, load.image_pages), (1, 0));
    assert!(load.nodes > 0);
    assert_eq!(rig.ui.skin_id(), "kit");

    let first = rig.frame(&[ReplicaStep::Loaded { turn: 87 }]);
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    assert!(
        first.source.roots_built >= 4,
        "at least meta, top, research, game"
    );
    assert!(first.delta_ops > 0);
    assert_eq!(first.runtime.binds_evaluated, 5);
    assert_eq!(first.runtime.layout_runs, 1);
    assert_eq!(first.dropped_output, 0);

    let want = ["Turn 87", "312", "+48", "Education", "3 turns"];
    assert_eq!(rig.ui.texts(), want);
    assert_eq!(rig.painted_runs(), shaped(&want));

    // The glyphs are rasterized within the frame's budget and the HUD layer holds them.
    let scene = first.scene;
    assert_eq!(scene.raster_deferred, 0);
    let glyphs: usize = want
        .iter()
        .map(|t| t.chars().filter(|c| *c != ' ').count())
        .sum();
    assert!(scene.raster_glyphs > 0 && scene.atlas_glyphs > 0);
    assert!(
        rig.ui.scene().layer_instances(LayerId::Hud).len() >= glyphs,
        "one instance per glyph at least"
    );
}

#[test]
fn frames_without_a_change_do_no_work() {
    let mut rig = Rig::new();
    rig.frame(&[ReplicaStep::Loaded { turn: 87 }]);
    let reads = rig.game.call_count();
    let rev = rig.ui.scene().layer_revision(LayerId::Hud);
    let model = rig.ui.store().revision();

    for _ in 0..50 {
        let r = rig.frame(&[ReplicaStep::Idle]);
        assert_eq!(r.source, Default::default(), "no read, no root");
        assert_eq!(r.delta_ops, 0);
        assert_eq!(r.model_revision, model);
        let s = r.runtime;
        assert_eq!(
            (
                s.binds_evaluated,
                s.restyled,
                s.shaped_runs,
                s.layout_runs,
                s.chunks_rebuilt,
                s.visited
            ),
            (0, 0, 0, 0, 0, 0)
        );
        assert_eq!((r.scene.chunks_encoded, r.scene.raster_glyphs), (0, 0));
        assert!(r.errors.is_empty());
    }
    assert_eq!(rig.game.call_count(), reads, "the core was not read");
    assert_eq!(rig.ui.scene().layer_revision(LayerId::Hud), rev);
    assert_eq!(rig.ui.frames(), 51);

    // One applied batch that changes the gold: one model op, one binding, one shaped run (the strip's digits are
    // proportional, so the row may lay out again and repaint more than one chunk).
    rig.game.top.gold = 333;
    rig.jseq = 1;
    let r = rig.frame(&[ReplicaStep::Applied { jseq: 1 }]);
    assert_eq!(r.delta_ops, 1);
    let s = r.runtime;
    assert_eq!((s.binds_evaluated, s.shaped_runs), (1, 1));
    assert!(s.chunks_rebuilt >= 1);
    // Back to idle.
    let s = rig.frame(&[]).runtime;
    assert_eq!((s.binds_evaluated, s.chunks_rebuilt, s.visited), (0, 0, 0));
    assert_eq!(rig.ui.texts()[1], "333");
    assert_eq!(rig.painted_runs()[1], shaped(&["333"])[0]);
}

#[test]
fn the_host_clock_times_the_load_and_the_frame() {
    let t = Rc::new(Cell::new(0.0f64));
    let tick = Rc::clone(&t);
    let clock: ui2_host::HostClock = Rc::new(move || {
        tick.set(tick.get() + 0.5);
        tick.get()
    });
    let mut ui = V2Ui::new(
        support::strip(),
        V2Init {
            env: env(),
            clock: Some(clock),
            ..V2Init::default()
        },
    )
    .expect("the skin loads");
    assert!(ui.load_stats().build_ms > 0.0);
    let r = ui.frame(V2Frame {
        replica: None,
        steps: &[],
        net: None,
        events: &[],
        mods: None,
        now_ms: 0.0,
    });
    assert!(r.cpu_ms > 0.0 && r.runtime.cpu_ms > 0.0);
    assert!(r.cpu_ms >= r.runtime.cpu_ms + r.source_ms + r.resolve_ms);
    let names: Vec<_> = r.counters().into_iter().map(|(n, _)| n).collect();
    assert!(names.contains(&"ui2.cpu_ms") && names.iter().all(|n| n.starts_with("ui2.")));
}

#[test]
fn a_new_environment_lays_out_again() {
    let mut rig = Rig::new();
    rig.frame(&[ReplicaStep::Loaded { turn: 87 }]);
    rig.frame(&[]);
    rig.ui.set_env(Env {
        viewport_px: (1920, 1080),
        dpr: 2.0,
        ..env()
    });
    assert_eq!(rig.ui.env().dpr, 2.0);
    let r = rig.frame(&[]);
    assert_eq!(r.runtime.layout_runs, 1);
    assert!(r.runtime.chunks_rebuilt > 0);
    assert_eq!(rig.ui.scene().scale(), 2.0);
    // The same environment again changes nothing.
    rig.ui.set_env(rig.ui.env());
    rig.frame(&[]);
    assert_eq!(rig.frame(&[]).runtime.layout_runs, 0);
}
