//! `clip-path: polygon()`: the polygon cuts its node and the subtree in the same clip as the overflow
//! rectangle above, hits outside it miss, a polygon without area hides its subtree, idle frames do nothing.

use glam::Vec2;
use ui2_draw::{Item, Scene};
use ui2_model::{ModelStore, Schema};
use ui2_package::Layer;
use ui2_package::style::{ColorV, Display, Len, LenAuto, Overflow, Point, Position, StyleRecord};
use ui2_runtime::testkit::{Kit, TNode};
use ui2_runtime::{Env, Runtime, Services};
use ui2_text::TextEngine;

fn abs(x: f32, y: f32, w: f32, h: f32) -> StyleRecord {
    let mut r = StyleRecord::initial();
    r.display = Display::Block;
    r.position = Position::Absolute;
    r.left = LenAuto::Len(Len::px(x));
    r.top = LenAuto::Len(Len::px(y));
    r.width = LenAuto::Len(Len::px(w));
    r.height = LenAuto::Len(Len::px(h));
    r.background_color = ColorV::Lit([200, 0, 0, 255]);
    r
}

fn pt(x: Len, y: Len) -> Point {
    Point { x, y }
}

fn diamond() -> Vec<Point> {
    vec![
        pt(Len::pct(50.0), Len::ZERO),
        pt(Len::pct(100.0), Len::pct(50.0)),
        pt(Len::pct(50.0), Len::pct(100.0)),
        pt(Len::ZERO, Len::pct(50.0)),
    ]
}

struct Rig {
    rt: Runtime,
    store: ModelStore,
    engine: TextEngine,
    scene: Scene,
}

impl Rig {
    fn tick(&mut self) -> ui2_runtime::FrameStats {
        let out = self
            .rt
            .frame(0.0, &self.store, &mut self.engine, &mut self.scene);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        out.stats
    }

    fn items(&mut self, part: &str) -> Vec<Item> {
        let id = self.rt.find_part(part).expect("part");
        let chunk = self.rt.node(id).expect("node").chunk();
        self.rt.chunk_items(chunk).expect("chunk").1.items
    }
}

/// A clipping frame (overflow hidden, 80 x 40 at 10, 10) holding a 100 x 50 diamond with a child box; a
/// flat polygon elsewhere. Device scale 2.
fn rig() -> Rig {
    let mut kit = Kit::new();
    let mut layer = StyleRecord::initial();
    layer.display = Display::Block;
    let mut frame = abs(10.0, 10.0, 80.0, 40.0);
    frame.overflow_x = Overflow::Hidden;
    frame.overflow_y = Overflow::Hidden;
    frame.background_color = ColorV::Lit([0, 0, 0, 0]);
    let mut shape = abs(0.0, 0.0, 100.0, 50.0);
    shape.clip_path = diamond();
    let inner = abs(40.0, 20.0, 20.0, 10.0);
    let mut flat = abs(200.0, 0.0, 20.0, 20.0);
    flat.clip_path = vec![
        pt(Len::ZERO, Len::ZERO),
        pt(Len::pct(50.0), Len::pct(50.0)),
        pt(Len::pct(100.0), Len::pct(100.0)),
    ];
    let flat_child = abs(0.0, 0.0, 5.0, 5.0);
    let mut sets = Vec::new();
    for r in [layer, frame, shape, inner, flat, flat_child] {
        let id = kit.record(r);
        sets.push(kit.plain(id));
    }
    let names = ["frame", "shape", "inner", "flat"].map(|n| kit.str(n));
    kit.template(
        "doc",
        vec![
            TNode::layer(Layer::Hud, sets[0]).children([
                TNode::el(sets[1]).part(names[0]).child(
                    TNode::el(sets[2])
                        .part(names[1])
                        .child(TNode::el(sets[3]).part(names[2])),
                ),
                TNode::el(sets[4]).part(names[3]).child(TNode::el(sets[5])),
            ]),
        ],
    );
    let pkg = kit.finish().expect("package");
    let env = Env {
        viewport_px: (800, 600),
        dpr: 1.0,
        ui_scale: 2.0,
        theme: 0,
    };
    let mut rig = Rig {
        rt: Runtime::new(pkg, env, Services::default()).expect("runtime"),
        store: ModelStore::new(Schema::v1()),
        engine: TextEngine::new(),
        scene: Scene::new(1.0),
    };
    rig.tick();
    rig
}

#[test]
fn clip_path_cuts_the_node_and_its_subtree_with_the_overflow_rectangle() {
    let mut rig = rig();
    let items = rig.items("frame");
    let boxes: Vec<&ui2_draw::BoxItem> = items
        .iter()
        .filter_map(|i| match i {
            Item::Box(b) => Some(b),
            _ => None,
        })
        .collect();
    assert_eq!(boxes.len(), 2, "{items:?}");
    // The diamond of the 100 x 50 box at (10, 10) CSS px, in device px.
    let want = [
        Vec2::new(120.0, 20.0),
        Vec2::new(220.0, 70.0),
        Vec2::new(120.0, 120.0),
        Vec2::new(20.0, 70.0),
    ];
    for b in &boxes {
        let clip = b.clip.expect("clipped");
        assert_eq!(
            clip.rect,
            Some(ui2_draw::Rect::new(20.0, 20.0, 160.0, 80.0)),
            "the frame's overflow rectangle"
        );
        let poly = clip.polygon.expect("the polygon");
        assert_eq!(poly.points(), want);
    }
}

#[test]
fn clip_path_without_area_hides_the_subtree() {
    let mut rig = rig();
    assert!(rig.items("flat").is_empty());
}

#[test]
fn hits_outside_the_polygon_miss() {
    let rig = rig();
    let frame = rig.rt.find_part("frame");
    let shape = rig.rt.find_part("shape");
    let inner = rig.rt.find_part("inner");
    // Physical px (device scale 2). The middle of the diamond is the child box.
    assert_eq!(rig.rt.node_at(120.0, 70.0), inner);
    // Inside the diamond, outside the child.
    assert_eq!(rig.rt.node_at(70.0, 70.0), shape);
    // The top left corner of the diamond's box is cut off: the frame under it takes the hit.
    assert_eq!(rig.rt.node_at(30.0, 30.0), frame);
    // The flat polygon hides its box and its child from input too.
    assert_eq!(rig.rt.node_at(405.0, 5.0), None);
    assert_eq!(rig.rt.node_at(420.0, 20.0), None);
}

#[test]
fn idle_frames_after_a_clip_path_do_nothing() {
    let mut rig = rig();
    for _ in 0..20 {
        let st = rig.tick();
        assert_eq!((st.chunks_rebuilt, st.layout_runs, st.visited), (0, 0, 0));
    }
}
