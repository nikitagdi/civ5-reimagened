//! Painting leaves out what a scroll container's clip hides wholly: the rows of a long list cost no item until they
//! are scrolled into view, and an idle frame or a scroll touches only what it must.

use std::sync::Arc;

use platform::{InputEvent, Modifiers, PhysicalPosition, WheelDelta, WheelEvent};
use ui2_draw::{Item, Scene};
use ui2_model::{ModelStore, Schema};
use ui2_package::style::{ColorV, Display, Len, LenAuto, Overflow, Position, StyleRecord};
use ui2_package::{Layer, Package};
use ui2_runtime::testkit::{Kit, TNode};
use ui2_runtime::{Env, Runtime, Services};
use ui2_text::TextEngine;

/// Rows of the list, each `ROW` px high, in a view `VIEW` px high.
const ROWS: usize = 12;
const ROW: f32 = 40.0;
const VIEW: f32 = 100.0;

fn at(x: f32, y: f32, w: f32, h: f32) -> StyleRecord {
    let mut r = StyleRecord::initial();
    r.position = Position::Absolute;
    r.left = LenAuto::Len(Len::px(x));
    r.top = LenAuto::Len(Len::px(y));
    r.width = LenAuto::Len(Len::px(w));
    r.height = LenAuto::Len(Len::px(h));
    r
}

/// A HUD with `view` (a 200 x 100 scroll container at 10, 10) holding `ROWS` coloured rows one under the other.
fn package() -> Arc<Package> {
    let mut kit = Kit::new();
    let mut layer = StyleRecord::initial();
    layer.display = Display::Block;
    let layer = kit.record(layer);
    let layer = kit.plain(layer);
    let mut view = at(10.0, 10.0, 200.0, VIEW);
    view.overflow_x = Overflow::Hidden;
    view.overflow_y = Overflow::Auto;
    let view = kit.record(view);
    let view = kit.plain(view);
    let name = kit.str("view");
    let mut list = TNode::el(view).part(name);
    for i in 0..ROWS {
        // Rows in the flow, one under the other (they make the content the container scrolls).
        let mut row = StyleRecord::initial();
        row.display = Display::Block;
        row.width = LenAuto::Len(Len::px(200.0));
        row.height = LenAuto::Len(Len::px(ROW));
        row.flex_shrink = 0.0;
        row.background_color = ColorV::Lit([10 + i as u8, 20, 30, 255]);
        let row = kit.record(row);
        let row = kit.plain(row);
        let name = kit.str(&format!("row{i}"));
        list = list.child(TNode::el(row).part(name));
    }
    kit.template("doc", vec![TNode::layer(Layer::Hud, layer).child(list)]);
    kit.finish().expect("package")
}

struct Rig {
    rt: Runtime,
    engine: TextEngine,
    store: ModelStore,
    scene: Scene,
}

impl Rig {
    fn new() -> Self {
        let mut r = Self {
            rt: Runtime::new(package(), Env::default(), Services::default()).expect("runtime"),
            engine: TextEngine::new(),
            store: ModelStore::new(Schema::v1()),
            scene: Scene::new(1.0),
        };
        r.frame();
        r
    }

    fn frame(&mut self) -> ui2_runtime::FrameStats {
        let out = self
            .rt
            .frame(0.0, &self.store, &mut self.engine, &mut self.scene);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        out.stats
    }

    /// The rows the chunk draws (by their fill colour's red channel).
    fn drawn_rows(&mut self) -> Vec<usize> {
        let id = self.rt.find_part("view").expect("view");
        let chunk = self.rt.node(id).expect("node").chunk();
        let items = self.rt.chunk_items(chunk).expect("chunk").1.items;
        let mut rows: Vec<usize> = items
            .iter()
            .filter_map(|i| match i {
                Item::Box(b) => match b.fill {
                    ui2_draw::Fill::Solid(c) => Some(usize::from(c.0[0] - 10)),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        rows.sort_unstable();
        rows
    }

    fn wheel(&mut self, lines: f32) {
        self.rt.handle_input(
            &InputEvent::Wheel(WheelEvent {
                delta: WheelDelta::Lines { x: 0.0, y: lines },
                position: PhysicalPosition { x: 50.0, y: 50.0 },
                modifiers: Modifiers::default(),
            }),
            0.0,
        );
    }
}

#[test]
fn rows_outside_the_view_are_left_out_until_scrolled_in() {
    let mut r = Rig::new();
    // 100 px of 40 px rows: rows 0, 1 and 2 (cut) are seen; the nine others cost nothing.
    assert_eq!(r.drawn_rows(), vec![0, 1, 2]);
    // An idle frame does nothing.
    let idle = r.frame();
    assert_eq!((idle.chunks_rebuilt, idle.visited), (0, 0));
    // Scrolled down: the rows now in view are drawn, those above are left out.
    r.wheel(-3.0);
    let s = r.frame();
    assert_eq!(s.chunks_rebuilt, 1);
    let rows = r.drawn_rows();
    assert!(!rows.contains(&0) && !rows.is_empty(), "{rows:?}");
    let first = rows[0];
    assert!(
        rows.iter().all(|&i| i < first + 4),
        "only what the view shows: {rows:?}"
    );
    // Every row the view shows is there (no gap): consecutive indices.
    assert!(rows.windows(2).all(|w| w[1] == w[0] + 1), "{rows:?}");
}
