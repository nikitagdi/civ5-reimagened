//! Text placed straight in a flex container (a keycap, a button's own text): Chrome wraps it in an anonymous
//! flex item, which the container aligns by `align-items` and `justify-content`. The expected boxes are
//! computed by hand from the font's metrics and the shaped width.

use ui2_draw::Scene;
use ui2_model::{ModelStore, Schema};
use ui2_package::Layer;
use ui2_package::style::{
    ContentAlign, Display, FlexDirection, ItemsAlign, Len, LenAuto, StyleRecord, TextAlign,
};
use ui2_runtime::testkit::{Kit, TNode};
use ui2_runtime::{ElementRect, Env, Runtime, Services, load_faces};
use ui2_text::{FaceId, RunStyle, TextEngine};

const W: f32 = 200.0;
const H: f32 = 40.0;
const SIZE: f32 = 14.0;
const TEXT: &str = "Esc";

fn fira() -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../third_party/fonts/fira-sans/FiraSans-Regular.ttf"),
    )
    .expect("font")
}

/// The rectangles of a 200 x 40 flex container holding only `Esc`, styled by `f`, and of its text.
fn rects(f: impl Fn(&mut StyleRecord)) -> (ElementRect, ElementRect, TextEngine) {
    let mut kit = Kit::new();
    kit.font("Fira Sans", 400, false, &fira());
    let family = kit.str("Fira Sans");
    let mut block = StyleRecord::initial();
    block.display = Display::Block;
    block.font_family = vec![family];
    block.font_size = Len::px(SIZE);
    let mut keycap = block.clone();
    keycap.display = Display::Flex;
    keycap.width = LenAuto::Len(Len::px(W));
    keycap.height = LenAuto::Len(Len::px(H));
    f(&mut keycap);
    let (block, keycap) = (kit.record(block), kit.record(keycap));
    let (block, keycap) = (kit.plain(block), kit.plain(keycap));
    let (part, text) = (kit.str("key"), kit.str(TEXT));
    kit.template(
        "doc",
        vec![
            TNode::layer(Layer::Hud, block).child(
                TNode::el(keycap)
                    .part(part)
                    .child(TNode::text(text, keycap)),
            ),
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
    let all = rt.dump_rects(Some("key"));
    let el = all
        .iter()
        .find(|r| !r.node.contains('#'))
        .cloned()
        .expect("box");
    let leaf = all
        .iter()
        .find(|r| r.node.contains('#'))
        .cloned()
        .expect("text");
    (el, leaf, engine)
}

/// The text's height (ascent + descent), its top inside a `normal` line (half the line gap, floored), the
/// line's height, and the shaped width.
fn text_metrics(engine: &mut TextEngine) -> (f32, f32, f32, f32) {
    let m = engine.metrics(FaceId(0), SIZE, 400.0).expect("metrics");
    let run = engine
        .shape(&RunStyle::new(FaceId(0), SIZE).with_wght(400.0), TEXT)
        .expect("shape");
    let content = m.ascent + m.descent;
    let line = m.normal_line_height();
    (content, (line - content) / 2.0, line, run.width)
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 0.01
}

#[test]
fn flex_text_align_items_center_centers_the_anonymous_item() {
    let (el, leaf, mut engine) = rects(|r| r.align_items = ItemsAlign::Center);
    let (content, lead, line, width) = text_metrics(&mut engine);
    assert_eq!((el.w, el.h), (W, H));
    let top = (H - line) / 2.0 + lead.floor();
    assert!(close(leaf.y - el.y, top), "{} vs {top}", leaf.y - el.y);
    assert!(close(leaf.h, content));
    assert!(close(leaf.x - el.x, 0.0));
    assert!(close(leaf.w, width), "{} vs {width}", leaf.w);
}

#[test]
fn flex_text_justify_content_center_and_end() {
    let (el, leaf, mut engine) = rects(|r| r.justify_content = ContentAlign::Center);
    let (_, lead, _, width) = text_metrics(&mut engine);
    assert!(close(leaf.x - el.x, (W - width) / 2.0), "{}", leaf.x - el.x);
    // `align-items: normal` stretches the item: the text stays at the top.
    assert!(close(leaf.y - el.y, lead.floor()));

    let (el, leaf, _) = rects(|r| {
        r.justify_content = ContentAlign::FlexEnd;
        r.align_items = ItemsAlign::FlexEnd;
    });
    let (_, lead, line, width) = text_metrics(&mut engine);
    assert!(close(leaf.x - el.x, W - width));
    assert!(close(leaf.y - el.y, H - line + lead.floor()));
}

#[test]
fn flex_text_in_a_column_and_text_align_inside_the_item() {
    // A column stretches the item across: `text-align` works inside it; `justify-content` moves it down.
    let (el, leaf, mut engine) = rects(|r| {
        r.flex_direction = FlexDirection::Column;
        r.justify_content = ContentAlign::Center;
        r.text_align = TextAlign::Center;
    });
    let (_, lead, line, width) = text_metrics(&mut engine);
    assert!(close(leaf.x - el.x, (W - width) / 2.0));
    assert!(close(leaf.y - el.y, (H - line) / 2.0 + lead.floor()));
    // In a row the item is only as wide as the text: `text-align` has nothing to move.
    let (el, leaf, _) = rects(|r| r.text_align = TextAlign::Center);
    assert!(close(leaf.x - el.x, 0.0));
}
