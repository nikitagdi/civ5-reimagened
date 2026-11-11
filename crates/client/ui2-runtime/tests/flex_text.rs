//! Text placed straight in a flex container (a keycap, a button's own text): Chrome wraps it in an anonymous
//! flex item, which the container aligns by `align-items` and `justify-content`. The expected boxes are
//! computed by hand from the font's metrics and the shaped width.

use ui2_draw::Scene;
use ui2_model::{ModelStore, Sample, Schema};
use ui2_package::style::{
    ContentAlign, Display, FlexDirection, ItemsAlign, Len, LenAuto, LenNormal, StyleRecord,
    TextAlign,
};
use ui2_package::{Layer, NodeKind, Tag};
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
    rects_in(Tag::Div, f)
}

/// The same as `rects` for an element of another tag.
fn rects_in(tag: Tag, f: impl Fn(&mut StyleRecord)) -> (ElementRect, ElementRect, TextEngine) {
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
                TNode::new(NodeKind::Box { tag }, keycap)
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

/// The boxes of a 300 x 40 flex container (gap 8 px) holding the text `Gold`, a `span` with `12` and the
/// text `per turn`, styled by `f`: the container, the span, and the three text rectangles in order. With
/// `inline` the span's record says `display: inline` (a flex item is blockified: it is laid out as a
/// block either way); otherwise `block`, as Chrome computes it for a flex item.
fn mixed(
    inline: bool,
    f: impl Fn(&mut StyleRecord),
) -> (ElementRect, ElementRect, [ElementRect; 3]) {
    let mut kit = Kit::new();
    kit.font("Fira Sans", 400, false, &fira());
    let family = kit.str("Fira Sans");
    let mut block = StyleRecord::initial();
    block.display = Display::Block;
    block.font_family = vec![family];
    block.font_size = Len::px(SIZE);
    let mut flex = block.clone();
    flex.display = Display::Flex;
    flex.width = LenAuto::Len(Len::px(300.0));
    flex.height = LenAuto::Len(Len::px(H));
    flex.column_gap = LenNormal::Len(Len::px(8.0));
    flex.row_gap = LenNormal::Len(Len::px(8.0));
    f(&mut flex);
    let mut span = block.clone();
    if inline {
        span.display = Display::Inline;
    }
    let (block, flex, span) = (kit.record(block), kit.record(flex), kit.record(span));
    let (block, flex, span) = (kit.plain(block), kit.plain(flex), kit.plain(span));
    let part = kit.str("mix");
    let texts = ["Gold", "12", "per turn"].map(|t| kit.str(t));
    kit.template(
        "doc",
        vec![
            TNode::layer(Layer::Hud, block).child(
                TNode::el(flex)
                    .part(part)
                    .child(TNode::text(texts[0], flex))
                    .child(TNode::el(span).child(TNode::text(texts[1], span)))
                    .child(TNode::text(texts[2], flex)),
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
    let all = rt.dump_rects(Some("mix"));
    let get = |n: &str| {
        all.iter()
            .find(|r| r.node == n)
            .cloned()
            .unwrap_or_else(|| panic!("no {n} in {all:?}"))
    };
    // Elements in preorder: layer 0, the container 1, the span 2; text leaves are named after their element.
    (
        get("0:1"),
        get("0:2"),
        [get("0:1#0"), get("0:2#0"), get("0:1#1")],
    )
}

fn width_of(engine: &mut TextEngine, text: &str) -> f32 {
    engine
        .shape(&RunStyle::new(FaceId(0), SIZE).with_wght(400.0), text)
        .expect("shape")
        .width
}

/// Text mixed with elements in a flex container: each run of text is its own anonymous flex item, in
/// document order, and the element between them is a flex item too; `gap`, `justify-content` and
/// `align-items` place all three.
#[test]
fn flex_text_mixed_with_elements_makes_one_item_per_run() {
    let mut engine = TextEngine::new();
    engine
        .load_face(FaceId(0), std::sync::Arc::from(fira()))
        .expect("font");
    let (content, lead, line, _) = text_metrics(&mut engine);
    let w = ["Gold", "12", "per turn"].map(|t| width_of(&mut engine, t));
    for inline in [false, true] {
        let (el, span, t) = mixed(inline, |r| {
            r.justify_content = ContentAlign::Center;
            r.align_items = ItemsAlign::Center;
        });
        let total = w[0] + w[1] + w[2] + 16.0;
        let x0 = (300.0 - total) / 2.0;
        let xs = [x0, x0 + w[0] + 8.0, x0 + w[0] + w[1] + 16.0];
        let top = (H - line) / 2.0;
        for i in 0..3 {
            assert!(
                close(t[i].x - el.x, xs[i]),
                "{inline} {i}: {:?} vs {}",
                t[i],
                xs[i]
            );
            assert!(close(t[i].w, w[i]), "{inline} {i}: {:?} vs {}", t[i], w[i]);
            assert!(
                close(t[i].y - el.y, top + lead.floor()),
                "{inline} {i}: {:?}",
                t[i]
            );
            assert!(close(t[i].h, content), "{inline} {i}: {:?}", t[i]);
        }
        assert!(
            close(span.x - el.x, xs[1]) && close(span.w, w[1]),
            "{span:?}"
        );
        assert!(close(span.y - el.y, top) && close(span.h, line), "{span:?}");

        // `flex-start` and `flex-end`, 3 px apart: the runs keep their order and the gap.
        let (el, _, t) = mixed(inline, |r| {
            r.align_items = ItemsAlign::FlexEnd;
            r.column_gap = LenNormal::Len(Len::px(3.0));
        });
        let xs = [0.0, w[0] + 3.0, w[0] + w[1] + 6.0];
        for i in 0..3 {
            assert!(
                close(t[i].x - el.x, xs[i]),
                "{inline} {i}: {:?} vs {}",
                t[i],
                xs[i]
            );
            assert!(
                close(t[i].y - el.y, H - line + lead.floor()),
                "{inline} {i}: {:?}",
                t[i]
            );
        }
    }
}

/// A `button` that is not a flex container: Chrome centres its text down the content box anyway (and
/// `text-align` across it). A `div` styled the same keeps its text at the top; a button whose text does
/// not fit starts it at the top.
#[test]
fn button_text_is_centred_down_its_content_box() {
    let style = |r: &mut StyleRecord| {
        r.display = Display::Block;
        r.text_align = TextAlign::Center;
        r.padding_left = Len::px(6.0);
        r.padding_right = Len::px(6.0);
        r.padding_top = Len::px(2.0);
        r.border_top_width = Len::px(1.0);
        r.border_bottom_width = Len::px(1.0);
        r.border_left_width = Len::px(1.0);
        r.border_right_width = Len::px(1.0);
    };
    let (el, leaf, mut engine) = rects_in(Tag::Button, style);
    let (content, lead, line, width) = text_metrics(&mut engine);
    assert_eq!((el.w, el.h), (W + 14.0, H + 4.0));
    // The content box: 1 px border and 2 px padding at the top, 40 px high.
    let top = 3.0 + (H - line) / 2.0 + lead.floor();
    assert!(close(leaf.y - el.y, top), "{} vs {top}", leaf.y - el.y);
    assert!(close(leaf.h, content));
    assert!(
        close(leaf.x - el.x, 7.0 + (W - width) / 2.0),
        "{}",
        leaf.x - el.x
    );

    let (el, leaf, _) = rects_in(Tag::Div, style);
    assert!(
        close(leaf.y - el.y, 3.0 + lead.floor()),
        "{}",
        leaf.y - el.y
    );

    let (el, leaf, _) = rects_in(Tag::Button, |r| {
        style(r);
        r.height = LenAuto::Len(Len::px(8.0));
    });
    assert!(
        close(leaf.y - el.y, 3.0 + lead.floor()),
        "{}",
        leaf.y - el.y
    );
}

/// The engraved sample with the gold rate set: a negative rate turns the class `row` on below.
fn sample_with_rate(rate: i64) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../skins/engraved/sample/hud.json");
    let src = std::fs::read_to_string(&path).expect("sample");
    let mut v: serde_json::Value = serde_json::from_str(&src).expect("sample is JSON");
    v["model"]["top"]["gold_rate"] = serde_json::json!(rate);
    v.to_string()
}

/// A runtime over a container whose class `row` (on while `top.gold_rate < 0`) turns it from a block into
/// a flex container, holding the text `Gold`, an inline `span` with `12` and the text `per turn`, with the
/// store already written from the sample of `rate`.
fn switching(rate: i64) -> (Runtime, ModelStore, TextEngine, Scene) {
    let mut kit = Kit::new();
    kit.font("Fira Sans", 400, false, &fira());
    let family = kit.str("Fira Sans");
    let mut block = StyleRecord::initial();
    block.display = Display::Block;
    block.font_family = vec![family];
    block.font_size = Len::px(SIZE);
    let mut col = block.clone();
    col.width = LenAuto::Len(Len::px(300.0));
    let mut row = col.clone();
    row.display = Display::Flex;
    row.column_gap = LenNormal::Len(Len::px(8.0));
    row.align_items = ItemsAlign::Center;
    row.height = LenAuto::Len(Len::px(H));
    let mut span = block.clone();
    span.display = Display::Inline;
    span.padding_left = Len::px(3.0);
    let (block, col, row, span) = (
        kit.record(block),
        kit.record(col),
        kit.record(row),
        kit.record(span),
    );
    let (block, span) = (kit.plain(block), kit.plain(span));
    let class = kit.str("row");
    let toggled = kit.set(
        vec![ui2_package::Cond {
            up: 0,
            kind: ui2_package::CondKind::Class(class),
        }],
        vec![col, row],
    );
    let neg = kit.expr(ui2_package::profile::expr::parse("top.gold_rate < 0").expect("expr"));
    let part = kit.str("mix");
    let texts = ["Gold", "12", "per turn"].map(|t| kit.str(t));
    kit.template(
        "doc",
        vec![
            TNode::layer(Layer::Hud, block).child(
                TNode::el(toggled)
                    .part(part)
                    .bind(ui2_package::BindKind::ClassToggle { class, cond: neg })
                    .child(TNode::text(texts[0], toggled))
                    .child(TNode::el(span).child(TNode::text(texts[1], span)))
                    .child(TNode::text(texts[2], toggled)),
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
    let rt = Runtime::new(pkg, env, Services::default()).expect("runtime");
    let mut store = ModelStore::new(Schema::v1());
    Sample::parse(Schema::v1(), &sample_with_rate(rate))
        .expect("sample")
        .apply(&mut store)
        .expect("apply");
    (rt, store, engine, Scene::new(1.0))
}

/// Writes the sample of `rate` and runs a frame.
fn run(rt: &mut Runtime, store: &mut ModelStore, engine: &mut TextEngine, scene: &mut Scene) {
    if let Some(d) = store.take_frame() {
        rt.apply_delta(&d, store);
    }
    rt.frame(0.0, store, engine, scene);
}

/// The rectangles of the container's subtree, text leaves included, as a fresh runtime of `rate` lays
/// them out.
fn fresh(rate: i64) -> Vec<ElementRect> {
    let (mut rt, mut store, mut engine, mut scene) = switching(rate);
    run(&mut rt, &mut store, &mut engine, &mut scene);
    rt.dump_rects(Some("mix"))
}

/// A container that turns from a block into a flex container and back at run time: its inline span and
/// the texts beside it are pieces of one line while it is a block, and blockified items (each text its own
/// anonymous item) while it is a flex container. The text roots are rebuilt on each switch, so the boxes
/// equal a fresh build of the same state both ways; frames after the switch do no work.
#[test]
fn display_switch_rebuilds_the_text_roots() {
    let as_block = fresh(5);
    let as_flex = fresh(-5);
    assert_ne!(
        as_block, as_flex,
        "the two displays lay the text out differently"
    );
    // In the flex container the span is an item between two anonymous items.
    let get = |rs: &[ElementRect], n: &str| {
        rs.iter()
            .find(|r| r.node == n)
            .cloned()
            .unwrap_or_else(|| panic!("no {n} in {rs:?}"))
    };
    assert!(get(&as_flex, "0:1#1").x > get(&as_flex, "0:2").x + get(&as_flex, "0:2").w);
    // In the block the three are one line: the span sits on the texts' baseline, inside the line.
    assert_eq!(get(&as_block, "0:1#0").y, get(&as_block, "0:1#1").y);

    for start in [5i64, -5] {
        let (mut rt, mut store, mut engine, mut scene) = switching(start);
        run(&mut rt, &mut store, &mut engine, &mut scene);
        for rate in [-start, start, -start] {
            Sample::parse(Schema::v1(), &sample_with_rate(rate))
                .expect("sample")
                .apply(&mut store)
                .expect("apply");
            run(&mut rt, &mut store, &mut engine, &mut scene);
            let want = if rate < 0 { &as_flex } else { &as_block };
            assert_eq!(&rt.dump_rects(Some("mix")), want, "from {start} to {rate}");
            for _ in 0..3 {
                let s = rt.frame(0.0, &store, &mut engine, &mut scene).stats;
                assert_eq!(s.visited, 0, "an idle frame after the switch to {rate}");
            }
        }
        assert!(rt.take_errors().is_empty());
    }
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
