//! `dump_rects` under transforms: Chrome's dump (`getBoundingClientRect`, a `Range`'s rectangle for text) is
//! the bounding box of the transformed box, so the runtime's dump maps each box's corners through the
//! transforms above it and around each one's `transform-origin`. Boxes without a transform stay exactly as
//! laid out.

use ui2_draw::Scene;
use ui2_model::{ModelStore, Schema};
use ui2_package::Layer;
use ui2_package::style::{Display, Len, LenAuto, Origin, StyleRecord, TFn};
use ui2_runtime::testkit::{Kit, TNode};
use ui2_runtime::{ElementRect, Env, Runtime, Services, load_faces};
use ui2_text::TextEngine;

fn fira() -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../third_party/fonts/fira-sans/FiraSans-Regular.ttf"),
    )
    .expect("font")
}

/// Four 100 x 20 blocks stacked from the top of the page: `rot` (turned a quarter about its centre), `big`
/// (scaled twice about its top left corner, with the text `Esc`), `moved` (translated by 5 px and half its
/// height, with a 10 x 10 child) and `plain`. With `transformed` false none has a transform.
fn dump(transformed: bool) -> Vec<ElementRect> {
    let mut kit = Kit::new();
    kit.font("Fira Sans", 400, false, &fira());
    let family = kit.str("Fira Sans");
    let mut block = StyleRecord::initial();
    block.display = Display::Block;
    block.font_family = vec![family];
    block.font_size = Len::px(14.0);
    let mut bar = block.clone();
    bar.width = LenAuto::Len(Len::px(100.0));
    bar.height = LenAuto::Len(Len::px(20.0));
    let mut small = block.clone();
    small.width = LenAuto::Len(Len::px(10.0));
    small.height = LenAuto::Len(Len::px(10.0));
    let (mut rot, mut big, mut moved) = (bar.clone(), bar.clone(), bar.clone());
    if transformed {
        rot.transform = vec![TFn::Rotate(90.0)];
        big.transform = vec![TFn::Scale(2.0, 2.0)];
        big.transform_origin = Origin {
            x: Len::px(0.0),
            y: Len::px(0.0),
        };
        moved.transform = vec![TFn::Translate(Len::px(5.0), Len::pct(50.0))];
    }
    let sets: Vec<_> = [block, rot, big, moved, bar, small]
        .into_iter()
        .map(|r| {
            let id = kit.record(r);
            kit.plain(id)
        })
        .collect();
    let names = ["rot", "big", "moved", "plain", "inner"].map(|n| kit.str(n));
    let text = kit.str("Esc");
    kit.template(
        "doc",
        vec![
            TNode::layer(Layer::Hud, sets[0])
                .child(TNode::el(sets[1]).part(names[0]))
                .child(
                    TNode::el(sets[2])
                        .part(names[1])
                        .child(TNode::text(text, sets[2])),
                )
                .child(
                    TNode::el(sets[3])
                        .part(names[2])
                        .child(TNode::el(sets[5]).part(names[4])),
                )
                .child(TNode::el(sets[4]).part(names[3])),
        ],
    );
    let pkg = kit.finish().expect("package");
    let mut engine = TextEngine::new();
    load_faces(&pkg, &mut engine).expect("fonts");
    let env = Env {
        viewport_px: (800, 600),
        ..Env::default()
    };
    let mut rt = Runtime::new(pkg, env, Services::default()).expect("runtime");
    let store = ModelStore::new(Schema::v1());
    let mut scene = Scene::new(1.0);
    rt.frame(0.0, &store, &mut engine, &mut scene);
    rt.dump_rects(None)
}

fn find<'a>(all: &'a [ElementRect], node: &str) -> &'a ElementRect {
    all.iter()
        .find(|r| r.node == node)
        .unwrap_or_else(|| panic!("no {node} in {all:?}"))
}

fn xywh(r: &ElementRect) -> (f32, f32, f32, f32) {
    (r.x, r.y, r.w, r.h)
}

#[test]
fn transformed_boxes_dump_their_bounding_boxes() {
    let flat = dump(false);
    let all = dump(true);
    // Elements in preorder (text leaves are named after their element): layer 0, rot 1, big 2, moved 3,
    // inner 4, plain 5.
    assert_eq!(xywh(find(&flat, "0:1")), (0.0, 0.0, 100.0, 20.0));
    // A quarter turn about the centre (50, 10): 20 wide, 100 high.
    assert_eq!(xywh(find(&all, "0:1")), (40.0, -40.0, 20.0, 100.0));
    // Twice the size about the top left corner (0, 20); the text follows its element.
    assert_eq!(xywh(find(&all, "0:2")), (0.0, 20.0, 200.0, 40.0));
    let (t0, t) = (find(&flat, "0:2#0"), find(&all, "0:2#0"));
    assert!(t0.w > 0.0 && t0.h > 0.0, "{t0:?}");
    let close = |a: f32, b: f32| (a - b).abs() <= 0.002;
    assert!(close(t.x, 2.0 * t0.x), "{t:?} vs {t0:?}");
    assert!(close(t.y, 20.0 + 2.0 * (t0.y - 20.0)), "{t:?} vs {t0:?}");
    assert!(close(t.w, 2.0 * t0.w) && close(t.h, 2.0 * t0.h), "{t:?}");
    // A translation by (5 px, 50 % of 20 px) moves the box and its child.
    assert_eq!(xywh(find(&all, "0:3")), (5.0, 50.0, 100.0, 20.0));
    assert_eq!(xywh(find(&flat, "0:4")), (0.0, 40.0, 10.0, 10.0));
    assert_eq!(xywh(find(&all, "0:4")), (5.0, 50.0, 10.0, 10.0));
    // A box without a transform is listed exactly as before.
    assert_eq!(find(&all, "0:5"), find(&flat, "0:5"));
    assert_eq!(xywh(find(&all, "0:5")), (0.0, 60.0, 100.0, 20.0));
}
