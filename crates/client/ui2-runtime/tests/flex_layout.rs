//! Flex layout cases that real skin screens met, with boxes computed by hand from the CSS rules Chrome
//! follows.

use ui2_draw::Scene;
use ui2_model::{ModelStore, Schema};
use ui2_package::Layer;
use ui2_package::style::{Display, FlexDirection, Len, LenAuto, Overflow, StyleRecord};
use ui2_runtime::testkit::{Kit, TNode};
use ui2_runtime::{ElementRect, Env, Runtime, Services};
use ui2_text::TextEngine;

fn block() -> StyleRecord {
    let mut r = StyleRecord::initial();
    r.display = Display::Block;
    r
}

fn sized(w: Option<f32>, h: Option<f32>) -> StyleRecord {
    let mut r = block();
    if let Some(w) = w {
        r.width = LenAuto::Len(Len::px(w));
    }
    if let Some(h) = h {
        r.height = LenAuto::Len(Len::px(h));
    }
    r
}

/// Lays the document out on an 800 x 600 viewport and returns the rectangles of the parts, in order.
fn layout(kit: Kit, parts: &[&str]) -> Vec<ElementRect> {
    let pkg = kit.finish().expect("package");
    let mut engine = TextEngine::new();
    let env = Env {
        viewport_px: (800, 600),
        ..Env::default()
    };
    let mut rt = Runtime::new(pkg, env, Services::default()).expect("runtime");
    let store = ModelStore::new(Schema::v1());
    let mut scene = Scene::new(1.0);
    rt.frame(0.0, &store, &mut engine, &mut scene);
    parts
        .iter()
        .map(|p| {
            rt.dump_rects(Some(p))
                .into_iter()
                .find(|r| !r.node.contains('#'))
                .unwrap_or_else(|| panic!("no box for {p}"))
        })
        .collect()
}

fn xywh(r: &ElementRect) -> (f32, f32, f32, f32) {
    (r.x, r.y, r.w, r.h)
}

/// `flex: 1` is `flex: 1 1 0%`. A percentage flex basis against a column container of indefinite height
/// behaves as `content`, so the list keeps its content height and the panel grows around it (the research
/// window's list). A column of definite height resolves the same basis to zero and shares out the space.
#[test]
fn flex_1_in_a_column_of_auto_height_keeps_the_content_height() {
    for panel_h in [None, Some(200.0)] {
        let mut kit = Kit::new();
        let mut panel = sized(Some(300.0), panel_h);
        panel.display = Display::Flex;
        panel.flex_direction = FlexDirection::Column;
        let header = sized(None, Some(20.0));
        let mut list = block();
        list.flex_grow = 1.0;
        list.flex_shrink = 1.0;
        list.flex_basis = LenAuto::Len(Len::pct(0.0));
        list.min_height = LenAuto::Len(Len::px(0.0));
        list.overflow_x = Overflow::Auto;
        list.overflow_y = Overflow::Auto;
        let row = sized(None, Some(50.0));
        let sets: Vec<_> = [block(), panel, header, list, row]
            .into_iter()
            .map(|r| {
                let id = kit.record(r);
                kit.plain(id)
            })
            .collect();
        let names = ["panel", "header", "list"].map(|n| kit.str(n));
        kit.template(
            "doc",
            vec![
                TNode::layer(Layer::Window, sets[0]).child(
                    TNode::el(sets[1])
                        .part(names[0])
                        .child(TNode::el(sets[2]).part(names[1]))
                        .child(
                            TNode::el(sets[3])
                                .part(names[2])
                                .child(TNode::el(sets[4]))
                                .child(TNode::el(sets[4])),
                        ),
                ),
            ],
        );
        let r = layout(kit, &["panel", "header", "list"]);
        match panel_h {
            None => {
                assert_eq!(xywh(&r[0]), (0.0, 0.0, 300.0, 120.0), "panel");
                assert_eq!(xywh(&r[1]), (0.0, 0.0, 300.0, 20.0), "header");
                assert_eq!(xywh(&r[2]), (0.0, 20.0, 300.0, 100.0), "list");
            }
            Some(_) => {
                assert_eq!(xywh(&r[0]), (0.0, 0.0, 300.0, 200.0), "panel");
                assert_eq!(xywh(&r[2]), (0.0, 20.0, 300.0, 180.0), "list");
            }
        }
    }
}
