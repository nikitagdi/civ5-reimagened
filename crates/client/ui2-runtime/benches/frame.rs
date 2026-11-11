//! Runtime budgets, measured natively: a turn on the HUD and a window, an idle frame, a hover, 200 map
//! anchors placed by the camera, and the heap of a late-game tree.
//!
//! Run with `cargo test -p ui2-runtime --release --test frame_bench -- --ignored --nocapture --test-threads 1`.
//! The budgets of a release build on a desktop machine are asserted (the turn against its 1.5 ms runtime
//! share of the UI's 3 ms); a debug build only prints its numbers.
//! Rasterizing glyphs is not part of the runtime (it is the draw side's), so the scene is never resolved.

#[path = "../tests/support/mod.rs"]
mod support;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use glam::Vec2;
use serde_json::json;
use ui2_draw::Scene;
use ui2_model::{ModelStore, Sample, Schema};
use ui2_package::profile::action::AnchorKind;
use ui2_package::profile::expr::parse;
use ui2_package::style::{
    ColorV, Display, FlexDirection, FlexWrap, Len, LenAuto, Position, StyleRecord, TFn,
};
use ui2_package::{
    BindKind, Cond, CondKind, Layer, NodeKind, StyleId, StyleSetId, TemplateId, node_flags,
};
use ui2_runtime::testkit::{Kit, TNode, TplPiece, path};
use ui2_runtime::{AnchorId, AnchorScreen, Env, FrameStats, Runtime, Services, load_faces};
use ui2_text::TextEngine;

/// Counts the bytes the process holds on the heap.
struct Counting;

static HELD: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards to the system allocator and only counts sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        HELD.fetch_add(l.size(), Ordering::Relaxed);
        // SAFETY: same layout as asked.
        unsafe { System.alloc(l) }
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        HELD.fetch_sub(l.size(), Ordering::Relaxed);
        // SAFETY: `p` came from `alloc` with this layout.
        unsafe { System.dealloc(p, l) }
    }

    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        HELD.fetch_add(new, Ordering::Relaxed);
        HELD.fetch_sub(l.size(), Ordering::Relaxed);
        // SAFETY: `p` came from this allocator with layout `l`.
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn held() -> usize {
    HELD.load(Ordering::Relaxed)
}

const RELEASE: bool = !cfg!(debug_assertions);

fn fira() -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../third_party/fonts/fira-sans/FiraSans-Regular.ttf"),
    )
    .expect("font")
}

fn ms(t: std::time::Duration) -> f64 {
    t.as_secs_f64() * 1e3
}

fn for_node(kit: &mut Kit, body: u32, item: &str, list: &str) -> TNode {
    let list = kit.expr(parse(list).expect("list"));
    let key = kit.expr(parse(&format!("{item}.id")).expect("key"));
    let item = kit.str(item);
    TNode::new(
        NodeKind::For {
            tpl: TemplateId(body),
            item,
            index: None,
            list,
            key,
            virt: false,
            sort: None,
            sort_desc: None,
            filter: None,
        },
        StyleSetId(0),
    )
}

/// The HUD (20 bound numbers of `top`) and a window with one row per resource: five bound cells, two class
/// toggles and a hover variant per row.
fn hud_and_window() -> (Runtime, TextEngine) {
    let mut kit = Kit::new();
    kit.font("Fira Sans", 400, false, &fira());
    let fira = kit.str("Fira Sans");
    let text = |display| {
        let mut r = StyleRecord::initial();
        r.display = display;
        r.font_family = vec![fira];
        r.font_size = Len::px(14.0);
        r
    };
    let block = kit.record(text(Display::Block));
    let block = kit.plain(block);
    let mut hud = text(Display::Flex);
    hud.flex_wrap = FlexWrap::Wrap;
    hud.column_gap = ui2_package::style::LenNormal::Len(Len::px(8.0));
    let hud = kit.record(hud);
    let hud = kit.plain(hud);
    let mut window = text(Display::Flex);
    window.flex_direction = FlexDirection::Column;
    window.width = LenAuto::Len(Len::px(640.0));
    let window = kit.record(window);
    let window = kit.plain(window);
    let mut cell = text(Display::Block);
    cell.width = LenAuto::Len(Len::px(110.0));
    let cell = kit.record(cell);
    let cell = kit.plain(cell);
    // The row: classes `neg` and `big`, and hover.
    let mut row = text(Display::Flex);
    row.column_gap = ui2_package::style::LenNormal::Len(Len::px(4.0));
    let mut table = Vec::new();
    for mask in 0u8..8 {
        let mut r = row.clone();
        if mask & 1 != 0 {
            r.color = ColorV::Lit([214, 96, 77, 255]);
        }
        if mask & 2 != 0 {
            r.background_color = ColorV::Lit([40, 40, 60, 255]);
        }
        if mask & 4 != 0 {
            r.outline_color = ColorV::Lit([255, 255, 255, 255]);
        }
        table.push(kit.record(r));
    }
    let (neg, big) = (kit.str("neg"), kit.str("big"));
    let row = kit.set(
        vec![
            Cond {
                up: 0,
                kind: CondKind::Class(neg),
            },
            Cond {
                up: 0,
                kind: CondKind::Class(big),
            },
            Cond {
                up: 0,
                kind: CondKind::Hover,
            },
        ],
        table,
    );
    let fields = [
        "top.gold",
        "top.gold_rate",
        "top.happiness",
        "top.faith",
        "top.faith_rate",
        "top.tourism",
        "top.science_rate",
        "top.culture_rate",
    ];
    let hud_cells: Vec<TNode> = (0..20)
        .map(|i| {
            let t = kit.tpl_of(vec![
                TplPiece::Lit(format!("{i}: ")),
                TplPiece::Expr(path(fields[i % fields.len()])),
            ]);
            TNode::el(block).child(TNode::bound_text(t, block))
        })
        .collect();
    let list = for_node(&mut kit, 1, "r", "top.resources");
    let (hud_part, window_part) = (kit.str("hud"), kit.str("window"));
    kit.template(
        "doc",
        vec![
            TNode::layer(Layer::Hud, block)
                .child(TNode::el(hud).part(hud_part).children(hud_cells)),
            TNode::layer(Layer::Window, block)
                .child(TNode::el(window).part(window_part).child(list)),
        ],
    );
    let cells: Vec<TNode> = [
        "{{r.name}}",
        "{{r.available}}",
        "{{r.used}}",
        "{{r.total}}",
        "{{r.total - r.used}} left",
    ]
    .iter()
    .map(|src| {
        let t = ui2_package::profile::expr::parse_template(src).expect("template");
        let pieces = t
            .segs
            .into_iter()
            .map(|s| match s {
                ui2_package::profile::expr::TplSeg::Lit(l) => TplPiece::Lit(l),
                ui2_package::profile::expr::TplSeg::Expr(e) => TplPiece::Expr(e),
            })
            .collect();
        let t = kit.tpl_of(pieces);
        TNode::el(cell).child(TNode::bound_text(t, cell))
    })
    .collect();
    let neg_cond = kit.expr(parse("r.available < r.used").expect("neg"));
    let big_cond = kit.expr(parse("r.total > 10").expect("big"));
    kit.template(
        "row",
        vec![
            TNode::el(row)
                .bind(BindKind::ClassToggle {
                    class: neg,
                    cond: neg_cond,
                })
                .bind(BindKind::ClassToggle {
                    class: big,
                    cond: big_cond,
                })
                .children(cells),
        ],
    );
    let pkg = kit.finish().expect("package");
    let mut engine = TextEngine::new();
    load_faces(&pkg, &mut engine).expect("fonts");
    let env = Env {
        viewport_px: (1920, 1080),
        ..Env::default()
    };
    (
        Runtime::new(pkg, env, Services::default()).expect("runtime"),
        engine,
    )
}

/// Resource rows of a turn: `phase` flips every number and every class of the rows.
fn resources(count: u32, phase: u32) -> serde_json::Value {
    (0..count)
        .map(|i| {
            let k = i + phase * 7;
            let (available, used) = if (i + phase).is_multiple_of(2) { (k, k + 3) } else { (k + 5, k) };
            let total = if (i + phase).is_multiple_of(2) { 20 + k } else { 4 };
            json!({ "id": format!("0:{i}"), "name": format!("Resource {i} {phase}"), "icon": "icon:R:1",
                    "available": available, "used": used, "total": total })
        })
        .collect()
}

struct Rig {
    rt: Runtime,
    engine: TextEngine,
    store: ModelStore,
    scene: Scene,
    sample: serde_json::Value,
    now: f64,
}

impl Rig {
    fn write(&mut self) {
        let s = Sample::parse(Schema::v1(), &self.sample.to_string()).expect("sample");
        s.apply(&mut self.store).expect("apply");
    }

    /// One frame as the host runs it (without the draw side), timed.
    fn frame(&mut self) -> (f64, FrameStats) {
        self.now += 16.0;
        let t = Instant::now();
        if let Some(d) = self.store.take_frame() {
            self.rt.apply_delta(&d, &self.store);
        }
        let out = self
            .rt
            .frame(self.now, &self.store, &mut self.engine, &mut self.scene);
        let took = ms(t.elapsed());
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        (took, out.stats)
    }
}

fn rig(rows: u32) -> Rig {
    let (mut rt, engine) = hud_and_window();
    let start = Instant::now();
    rt.set_clock(Box::new(move || start.elapsed().as_secs_f64() * 1e3));
    let mut r = Rig {
        rt,
        engine,
        store: ModelStore::new(Schema::v1()),
        scene: Scene::new(1.0),
        sample: support::sample_json(),
        now: 0.0,
    };
    r.sample["model"]["top"]["resources"] = resources(rows, 0);
    r.write();
    r.frame();
    r.frame();
    r
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

#[test]
#[ignore = "timing; run in release with --nocapture"]
fn bench_turn_idle_and_hover() {
    let mut r = rig(40);
    let gold = r.sample["model"]["top"]["gold"].as_i64().unwrap_or(0);
    let mut turns = Vec::new();
    let mut stats = Vec::new();
    for phase in 1..=40u32 {
        // A turn as the budget counts it: the numbers of the HUD and of 40 rows (5 texts and 2 classes each:
        // 300 bindings), the classes of every row, and 10 rows in or out (50 new text lines).
        let rows = if phase % 2 == 1 { 50 } else { 40 };
        r.sample["model"]["top"]["resources"] = resources(rows, phase);
        for f in [
            "gold",
            "gold_rate",
            "happiness",
            "faith",
            "faith_rate",
            "tourism",
        ] {
            r.sample["model"]["top"][f] = json!(gold + i64::from(phase) * 3);
        }
        r.write();
        let (t, s) = r.frame();
        turns.push(t);
        if phase % 2 == 1 {
            stats.push(s);
        }
    }
    let turn = median(turns.clone());
    let s = stats[stats.len() / 2];
    println!(
        "turn passes: bind {:.3}, style {:.3}, text {:.3}, layout {:.3}, paint {:.3} ms",
        s.bind_ms, s.style_ms, s.text_ms, s.layout_ms, s.paint_ms
    );
    println!(
        "turn: median {turn:.3} ms, max {:.3} ms; binds {}, restyled {}, shaped {}, layout runs {}, chunks {}, nodes {}",
        turns.iter().copied().fold(0.0, f64::max),
        s.binds_evaluated,
        s.restyled,
        s.shaped_runs,
        s.layout_runs,
        s.chunks_rebuilt,
        s.nodes
    );
    assert!(s.binds_evaluated >= 300, "{s:?}");

    let mut idle = Vec::new();
    for _ in 0..200 {
        let (t, s) = r.frame();
        assert_eq!((s.layout_runs, s.chunks_rebuilt, s.visited), (0, 0, 0));
        idle.push(t);
    }
    let idle = median(idle);
    println!("idle frame: median {:.4} ms", idle);

    let rows: Vec<_> = {
        let window = r.rt.find_part("window").expect("window");
        r.rt.node(window).expect("window").children()[1..].to_vec()
    };
    let mut hover = Vec::new();
    for i in 0..60 {
        r.rt.set_hover(Some(rows[i % rows.len()]));
        let (t, s) = r.frame();
        assert_eq!(s.layout_runs, 0);
        hover.push(t);
    }
    let hover = median(hover);
    println!("hover: median {hover:.4} ms");

    // The runtime's share of a turn is 1.5 ms (the whole UI's turn is 3 ms on average).
    if turn > 1.5 {
        println!(
            "turn: OVER the runtime budget of 1.5 ms by {:.3} ms",
            turn - 1.5
        );
    }
    if RELEASE {
        assert!(
            turn <= 1.5,
            "turn {turn:.3} ms over the runtime budget of 1.5 ms"
        );
        assert!(idle <= 0.02, "idle frame {idle:.4} ms over 0.02 ms");
        assert!(hover <= 0.15, "hover {hover:.4} ms over 0.15 ms");
    }
}

#[test]
#[ignore = "timing; run in release with --nocapture"]
fn bench_200_anchors() {
    let mut kit = Kit::new();
    let mut layer = StyleRecord::initial();
    layer.display = Display::Block;
    let mut banner = layer.clone();
    banner.position = Position::Absolute;
    banner.width = LenAuto::Len(Len::px(20.0));
    banner.height = LenAuto::Len(Len::px(10.0));
    banner.transform = vec![TFn::Translate(Len::pct(-50.0), Len::pct(-100.0))];
    banner.background_color = ColorV::Lit([200, 0, 0, 255]);
    let ids: Vec<StyleId> = [layer, banner].into_iter().map(|r| kit.record(r)).collect();
    let (layer, banner) = (kit.plain(ids[0]), kit.plain(ids[1]));
    let list = for_node(&mut kit, 1, "r", "top.resources");
    kit.template("doc", vec![TNode::layer(Layer::Map, layer).child(list)]);
    let id = kit.expr(parse("r.id").expect("anchor"));
    kit.template(
        "banner",
        vec![
            TNode::el(banner)
                .flags(node_flags::ANCHOR)
                .bind(BindKind::Anchor {
                    kind: AnchorKind::City,
                    id,
                }),
        ],
    );
    let pkg = kit.finish().expect("package");
    let mut r = Rig {
        rt: Runtime::new(pkg, Env::default(), Services::default()).expect("runtime"),
        engine: TextEngine::new(),
        store: ModelStore::new(Schema::v1()),
        scene: Scene::new(1.0),
        sample: support::sample_json(),
        now: 0.0,
    };
    r.sample["model"]["top"]["resources"] = resources(200, 0);
    r.write();
    r.now += 16.0;
    let d = r.store.take_frame().expect("frame");
    r.rt.apply_delta(&d, &r.store);
    let out = r.rt.frame(r.now, &r.store, &mut r.engine, &mut r.scene);
    let anchors: Vec<AnchorId> = out.anchor_changes.iter().map(|c| c.0).collect();
    assert_eq!(anchors.len(), 200);
    let mut times = Vec::new();
    for round in 0..100 {
        let upd: Vec<AnchorScreen> = anchors
            .iter()
            .enumerate()
            .map(|(i, &id)| AnchorScreen {
                id,
                pos: Vec2::new(
                    (i % 20) as f32 * 40.0 + round as f32,
                    (i / 20) as f32 * 30.0,
                ),
                scale: 1.0,
                visible: true,
            })
            .collect();
        let t = Instant::now();
        r.rt.set_anchor_screen(&upd, &mut r.scene);
        let (_, s) = r.frame();
        times.push(ms(t.elapsed()));
        assert_eq!((s.layout_runs, s.chunks_rebuilt), (0, 0));
    }
    let t = median(times);
    println!("200 anchors placed + frame: median {t:.4} ms");
    if RELEASE {
        // Placing is a table write per anchor: well inside an idle frame's share of the UI budget.
        assert!(t <= 0.45, "200 anchors {t:.4} ms over 0.45 ms");
    }
}

#[test]
#[ignore = "timing; run in release with --nocapture"]
fn bench_late_game_heap() {
    // About 7300 nodes: the HUD and 660 rows of eleven nodes (a row, five cells, five text leaves).
    let base = held();
    let (rt, engine) = hud_and_window();
    let after_load = held();
    let mut r = Rig {
        rt,
        engine,
        store: ModelStore::new(Schema::v1()),
        scene: Scene::new(1.0),
        sample: support::sample_json(),
        now: 0.0,
    };
    r.sample["model"]["top"]["resources"] = resources(660, 0);
    r.write();
    let before_frame = held();
    let (t, s) = r.frame();
    let after = held();
    let tree = after.saturating_sub(before_frame);
    let mib = |b: usize| b as f64 / (1024.0 * 1024.0);
    println!(
        "late game: {} nodes; first frame {t:.2} ms; skin and fonts {:.2} MiB, tree and caches after the first frame {:.2} MiB ({} B per node)",
        s.nodes,
        mib(after_load - base),
        mib(tree),
        tree / s.nodes.max(1) as usize
    );
    assert!((7000..8000).contains(&s.nodes), "{}", s.nodes);
    // The runtime heap (16 MiB) and the text caches (8 MiB) together.
    assert!(mib(tree) <= 24.0, "{:.2} MiB", mib(tree));
}
