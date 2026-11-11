//! Painting the top strip into a scene: the items of its chunk (a snapshot), and frames without changes that
//! leave the scene untouched.
//!
//! The snapshot is `tests/fixtures/paint-top.txt`; run with `UI2_BLESS=1` to write it again after a deliberate
//! change, and review the diff.

mod support;

use std::fmt::Write as _;

use serde_json::json;
use ui2_draw::{ChunkItems, Clip, Fill, Item, LayerChange, LayerId, Scene};
use ui2_model::{FrameDelta, ModelStore, Sample, Schema};
use ui2_runtime::{Env, Runtime, Services, load_faces};
use ui2_text::TextEngine;

fn env(vp: [u32; 2], ui_scale: f32) -> Env {
    Env {
        viewport_px: (vp[0], vp[1]),
        dpr: 1.0,
        ui_scale,
        theme: 0,
    }
}

fn frame_of(store: &mut ModelStore, sample: &serde_json::Value) -> FrameDelta {
    let s = Sample::parse(Schema::v1(), &sample.to_string()).expect("sample");
    s.apply(store).expect("apply");
    store.take_frame().expect("a change")
}

struct Strip {
    rt: Runtime,
    engine: TextEngine,
    store: ModelStore,
    scene: Scene,
    sample: serde_json::Value,
}

/// The bound strip after its first frame on the sample state.
fn strip(vp: [u32; 2], ui_scale: f32) -> (Strip, ui2_runtime::FrameStats) {
    let fx = support::load();
    let pkg = support::package_with(&fx, support::Texts::Bound);
    let mut engine = TextEngine::new();
    load_faces(&pkg, &mut engine).expect("fonts");
    let rt = Runtime::new(pkg, env(vp, ui_scale), Services::default()).expect("runtime");
    let mut s = Strip {
        rt,
        engine,
        store: ModelStore::new(Schema::v1()),
        scene: Scene::new(1.0),
        sample: support::sample_json(),
    };
    let d = frame_of(&mut s.store, &s.sample);
    s.rt.apply_delta(&d, &s.store);
    let stats = s.rt.update(&s.store, &mut s.engine, &mut s.scene);
    (s, stats)
}

fn hex(c: ui2_draw::Color) -> String {
    let [r, g, b, a] = c.0;
    format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
}

fn f(v: f32) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn rect(r: ui2_draw::Rect) -> String {
    format!("{} {} {} {}", f(r.x), f(r.y), f(r.w), f(r.h))
}

fn clip(c: &Option<Clip>) -> String {
    match c.and_then(|c| c.rect.map(|r| (r, c.radius))) {
        Some((r, radius)) => format!(" clip=[{} r{}]", rect(r), f(radius)),
        None => String::new(),
    }
}

/// One line per item.
fn snapshot(items: &ChunkItems) -> String {
    let mut out = String::new();
    for item in &items.items {
        match item {
            Item::Box(b) => {
                let fill = match b.fill {
                    Fill::Solid(c) => hex(c),
                    _ => "none".into(),
                };
                let bd = &b.border;
                let _ = writeln!(
                    out,
                    "box {} fill={} border={},{},{},{} {} radii={},{},{},{}{}",
                    rect(b.rect),
                    fill,
                    f(bd.top),
                    f(bd.right),
                    f(bd.bottom),
                    f(bd.left),
                    hex(bd.color),
                    f(b.radii.tl),
                    f(b.radii.tr),
                    f(b.radii.br),
                    f(b.radii.bl),
                    clip(&b.clip)
                );
            }
            Item::Mask(m) => {
                let _ = writeln!(
                    out,
                    "mask {} layer={} uv={} {} {} {} {}{}",
                    rect(m.rect),
                    m.mask.layer,
                    f(m.mask.uv[0]),
                    f(m.mask.uv[1]),
                    f(m.mask.uv[2]),
                    f(m.mask.uv[3]),
                    hex(m.color),
                    clip(&m.clip)
                );
            }
            Item::Glyphs(g) => {
                let first = g.glyphs.first().map_or((0.0, 0.0), |p| (p.x, p.y));
                let last = g.glyphs.last().map_or(0.0, |p| p.x);
                let _ = writeln!(
                    out,
                    "glyphs face={} size={} wght={} {} n={} x={}..{} y={}{}",
                    g.face.0,
                    f(g.size_px),
                    f(g.wght),
                    hex(g.color),
                    g.glyphs.len(),
                    f(first.0),
                    f(last),
                    f(first.1),
                    clip(&g.clip)
                );
            }
            other => {
                let _ = writeln!(out, "{other:?}");
            }
        }
    }
    out
}

#[test]
fn paint_top_strip_items_match_the_snapshot() {
    let (mut s, stats) = strip([1920, 1200], 1.0);
    assert_eq!(stats.chunks_rebuilt, 2, "the layer root and the strip");
    let top = s.rt.find_part("top").expect("top");
    let chunk = s.rt.node(top).expect("top").chunk();
    let (layer, items) = s.rt.chunk_items(chunk).expect("chunk");
    assert_eq!(layer, LayerId::Hud);
    assert!(s.rt.take_errors().is_empty());

    let count = |pred: fn(&Item) -> bool| items.items.iter().filter(|i| pred(i)).count();
    let boxes = count(|i| matches!(i, Item::Box(_)));
    let masks = count(|i| matches!(i, Item::Mask(_)));
    let runs = count(|i| matches!(i, Item::Glyphs(_)));
    let glyphs: usize = items
        .items
        .iter()
        .map(|i| match i {
            Item::Glyphs(g) => g.glyphs.len(),
            _ => 0,
        })
        .sum();
    println!(
        "items {}: boxes {boxes}, masks {masks}, glyph runs {runs}, glyphs {glyphs}",
        items.items.len()
    );
    assert_eq!(masks, 16, "every icon of the strip");
    // The strip's background, then its bottom hairline (an inset shadow without blur).
    match (&items.items[0], &items.items[1]) {
        (Item::Box(bg), Item::Box(line)) => {
            assert_eq!(rect(bg.rect), "0 0 1920 48");
            assert_eq!(bg.fill, Fill::Solid(ui2_draw::Color([27, 24, 21, 255])));
            assert_eq!(
                (line.border.top, line.border.bottom, line.border.left),
                (0.0, 1.0, 0.0)
            );
            assert_eq!(line.border.color, ui2_draw::Color([77, 64, 50, 255]));
        }
        other => panic!("{other:?}"),
    }

    // The same draw list was set on the scene.
    assert_eq!(s.scene.stats().chunks, 2);
    assert!(
        s.scene
            .chunk_instances(chunk)
            .is_some_and(|i| !i.is_empty())
    );

    let text = snapshot(&items);
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/paint-top.txt");
    if std::env::var_os("UI2_BLESS").is_some() {
        std::fs::write(&path, &text).expect("write snapshot");
    }
    let want = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        text, want,
        "the top strip's items changed (UI2_BLESS=1 writes them)"
    );
}

#[test]
fn paint_device_scale_multiplies_positions_and_sizes() {
    let (mut one, _) = strip([1920, 1200], 1.0);
    let (mut big, _) = strip([2880, 1800], 1.5);
    let items = |s: &mut Strip| {
        let top = s.rt.find_part("top").expect("top");
        let chunk = s.rt.node(top).expect("top").chunk();
        s.rt.chunk_items(chunk).expect("chunk").1
    };
    let (a, b) = (items(&mut one), items(&mut big));
    assert_eq!(a.items.len(), b.items.len());
    match (&a.items[0], &b.items[0]) {
        (Item::Box(x), Item::Box(y)) => {
            assert_eq!(rect(x.rect), "0 0 1920 48");
            assert_eq!(rect(y.rect), "0 0 2880 72");
        }
        other => panic!("{other:?}"),
    }
    let glyph_size = |c: &ChunkItems| {
        c.items.iter().find_map(|i| match i {
            Item::Glyphs(g) => Some(g.size_px),
            _ => None,
        })
    };
    assert_eq!(glyph_size(&a).map(|s| s * 1.5), glyph_size(&b));
    assert_eq!(big.scene.scale(), 1.5, "the frame set the scene's scale");
}

/// Stacking order, clips, opacity and transforms on a small panel.
#[test]
fn paint_stacking_clips_opacity_and_transforms() {
    use ui2_package::style::{
        ColorV, Display, IntAuto, Len, LenAuto, Overflow, Position, Radius, Shadow, StyleRecord,
        TFn,
    };
    use ui2_package::{Layer, PropId};
    use ui2_runtime::testkit::{Kit, TNode, TplPiece, path};

    let mut kit = Kit::new();
    let block = |w: f32, h: f32, c: [u8; 4]| {
        let mut r = StyleRecord::initial();
        r.display = Display::Block;
        r.width = LenAuto::Len(Len::px(w));
        r.height = LenAuto::Len(Len::px(h));
        r.background_color = ColorV::Lit(c);
        r
    };
    let mut layer = StyleRecord::initial();
    layer.display = Display::Block;
    let mut panel = block(100.0, 60.0, [10, 10, 10, 255]);
    panel.position = Position::Relative;
    panel.overflow_x = Overflow::Hidden;
    panel.overflow_y = Overflow::Hidden;
    let r8 = Radius {
        h: Len::px(8.0),
        v: Len::px(8.0),
    };
    panel.border_top_left_radius = r8;
    panel.border_top_right_radius = r8;
    panel.border_bottom_right_radius = r8;
    panel.border_bottom_left_radius = r8;
    let mut over = block(20.0, 10.0, [255, 0, 0, 255]);
    over.position = Position::Absolute;
    over.left = LenAuto::Len(Len::px(5.0));
    over.top = LenAuto::Len(Len::px(5.0));
    over.z_index = IntAuto::Int(2);
    let mut faded = block(30.0, 10.0, [0, 255, 0, 255]);
    faded.opacity = 0.5;
    let mut under = block(30.0, 10.0, [0, 0, 255, 255]);
    under.position = Position::Relative;
    under.z_index = IntAuto::Int(-1);
    let mut turned = block(10.0, 10.0, [255, 255, 0, 255]);
    turned.transform = vec![TFn::Translate(Len::px(10.0), Len::ZERO)];
    turned.box_shadow = vec![Shadow {
        inset: false,
        x: 0.0,
        y: 2.0,
        blur: 4.0,
        spread: 0.0,
        color: ColorV::Lit([0, 0, 0, 128]),
    }];
    let plain = |kit: &mut Kit, r: StyleRecord| {
        let id = kit.record(r);
        kit.plain(id)
    };
    let sets = [layer, panel, over, faded, under, turned].map(|r| plain(&mut kit, r));
    let width = kit.tpl_of(vec![
        TplPiece::Expr(path("top.happiness")),
        TplPiece::Lit("px".into()),
    ]);
    let names = ["panel", "over", "faded", "under", "turned"].map(|p| kit.str(p));
    kit.template(
        "doc",
        vec![
            TNode::layer(Layer::Hud, sets[0]).child(
                TNode::el(sets[1]).part(names[0]).children([
                    TNode::el(sets[2]).part(names[1]),
                    TNode::el(sets[3]).part(names[2]),
                    TNode::el(sets[4]).part(names[3]),
                    TNode::el(sets[5])
                        .part(names[4])
                        .bind(ui2_package::BindKind::Style {
                            prop: PropId::Width,
                            tpl: width,
                        }),
                ]),
            ),
        ],
    );
    let pkg = kit.finish().expect("package");
    let mut engine = TextEngine::new();
    let mut rt = Runtime::new(pkg, env([800, 600], 2.0), Services::default()).expect("runtime");
    let mut store = ModelStore::new(Schema::v1());
    let mut scene = Scene::new(1.0);
    let mut sample = support::sample_json();
    let d = frame_of(&mut store, &sample);
    rt.apply_delta(&d, &store);
    rt.update(&store, &mut engine, &mut scene);
    let panel = rt.find_part("panel").expect("panel");
    let chunk = rt.node(panel).expect("panel").chunk();
    let (_, items) = rt.chunk_items(chunk).expect("chunk");
    let boxes: Vec<&ui2_draw::BoxItem> = items
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Box(b) => Some(b),
            _ => None,
        })
        .collect();
    let fills: Vec<[u8; 4]> = boxes
        .iter()
        .map(|b| match b.fill {
            Fill::Solid(c) => c.0,
            _ => [0; 4],
        })
        .collect();
    // The panel, the negative context, the flow child (half transparent), the transformed context, the
    // positive context.
    assert_eq!(
        fills,
        vec![
            [10, 10, 10, 255],
            [0, 0, 255, 255],
            [0, 255, 0, 128],
            [255, 255, 0, 255],
            [255, 0, 0, 255]
        ]
    );
    // Device pixels (scale 2), the panel's rounded overflow clip on its children only.
    assert_eq!(rect(boxes[0].rect), "0 0 200 120");
    assert_eq!(boxes[0].radii.tl, 16.0);
    assert!(boxes[0].clip.is_none());
    for b in &boxes[1..] {
        let c = b.clip.expect("clipped by the panel");
        assert_eq!(c.rect.map(rect).as_deref(), Some("0 0 200 120"));
        assert_eq!(c.radius, 16.0);
    }
    // The transform is carried by the item: translate 10 CSS px = 20 device px.
    let xf = boxes[3].transform.expect("transform");
    assert_eq!(xf.translation, glam::Vec2::new(20.0, 0.0));
    // The bound width (7 px from the sample) went through layout into the item.
    assert_eq!(boxes[3].rect.w, 14.0);
    // The blurred shadow is reported once, also after the chunk is built again.
    let errors = rt.take_errors();
    assert_eq!(errors.len(), 1, "{errors:?}");
    sample["model"]["top"]["happiness"] = json!(9);
    let d = frame_of(&mut store, &sample);
    rt.apply_delta(&d, &store);
    let st = rt.update(&store, &mut engine, &mut scene);
    assert_eq!(
        (st.binds_evaluated, st.layout_runs, st.chunks_rebuilt),
        (1, 1, 1)
    );
    assert!(rt.take_errors().is_empty());
}

/// Test 7 of the runtime spec: once settled, frames without changes do not touch the scene.
#[test]
fn paint_empty_frames_leave_the_scene_untouched() {
    let (mut s, _) = strip([1920, 1200], 1.0);
    let top = s.rt.find_part("top").expect("top");
    let chunk = s.rt.node(top).expect("top").chunk();
    s.scene.take_layer_change(LayerId::Hud);
    s.scene.reset_frame_stats();
    let rev = s.scene.layer_revision(LayerId::Hud);
    let chunk_rev = s.scene.chunk_revision(chunk);
    for _ in 0..100 {
        let st = s.rt.update(&s.store, &mut s.engine, &mut s.scene);
        assert_eq!(
            (
                st.chunks_rebuilt,
                st.layout_runs,
                st.binds_evaluated,
                st.shaped_runs
            ),
            (0, 0, 0, 0)
        );
    }
    assert_eq!(s.scene.stats().chunks_encoded, 0);
    assert_eq!(s.scene.layer_revision(LayerId::Hud), rev);
    assert_eq!(s.scene.chunk_revision(chunk), chunk_rev);
    assert_eq!(s.scene.take_layer_change(LayerId::Hud), LayerChange::None);

    // One field: one chunk is built again and encoded once.
    s.sample["model"]["top"]["gold"] = json!(313);
    let d = frame_of(&mut s.store, &s.sample);
    s.rt.apply_delta(&d, &s.store);
    let st = s.rt.update(&s.store, &mut s.engine, &mut s.scene);
    assert_eq!(st.chunks_rebuilt, 1);
    assert_eq!(s.scene.stats().chunks_encoded, 1);

    // Removing the strip removes its chunk from the scene.
    s.rt.remove(top);
    let st = s.rt.update(&s.store, &mut s.engine, &mut s.scene);
    assert_eq!(
        st.chunks_rebuilt, 1,
        "the layer root, whose child went away"
    );
    assert_eq!(s.scene.stats().chunks, 1);
    assert!(s.scene.chunk_instances(chunk).is_none());
}
