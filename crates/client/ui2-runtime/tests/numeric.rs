//! `font-variant-numeric` reaches the shaper: tabular figures (`tnum`) share one advance, proportional ones
//! (`pnum`) do not, and kerning stays on with the figure features.

use ui2_draw::Scene;
use ui2_model::{ModelStore, Schema};
use ui2_package::Layer;
use ui2_package::style::{
    Display, Len, LenAuto, NumericFeatures, Position, StyleRecord, TextWrapMode,
};
use ui2_runtime::testkit::{Kit, TNode};
use ui2_runtime::{Env, Runtime, Services, load_faces};
use ui2_text::TextEngine;

fn font(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../third_party/fonts/fira-sans")
            .join(name),
    )
    .expect("font")
}

/// The width of each text, laid out in its own nowrap element with the given figure features.
fn widths(cases: &[(&str, NumericFeatures)]) -> Vec<f32> {
    let mut kit = Kit::new();
    kit.font("Fira Sans", 400, false, &font("FiraSans-Regular.ttf"));
    let family = kit.str("Fira Sans");
    let mut layer = StyleRecord::initial();
    layer.display = Display::Block;
    let layer = {
        let id = kit.record(layer);
        kit.plain(id)
    };
    let mut nodes = Vec::new();
    for (i, (text, numeric)) in cases.iter().enumerate() {
        let mut r = StyleRecord::initial();
        r.display = Display::Block;
        r.position = Position::Absolute;
        r.top = LenAuto::Len(Len::px(30.0 * i as f32));
        r.left = LenAuto::Len(Len::ZERO);
        r.font_family = vec![family];
        r.font_size = Len::px(20.0);
        r.text_wrap_mode = TextWrapMode::Nowrap;
        r.font_variant_numeric = *numeric;
        let id = kit.record(r);
        let set = kit.plain(id);
        let s = kit.str(text);
        let part = kit.str(&format!("t{i}"));
        nodes.push(TNode::el(set).part(part).child(TNode::text(s, set)));
    }
    kit.template("doc", vec![TNode::layer(Layer::Hud, layer).children(nodes)]);
    let pkg = kit.finish().expect("package");
    let mut engine = TextEngine::new();
    load_faces(&pkg, &mut engine).expect("faces");
    let mut rt = Runtime::new(pkg, Env::default(), Services::default()).expect("runtime");
    let store = ModelStore::new(Schema::v1());
    let out = rt.frame(0.0, &store, &mut engine, &mut Scene::new(1.0));
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    (0..cases.len())
        .map(|i| {
            let rects = rt.dump_rects(Some(&format!("t{i}")));
            // The element, then its text leaf.
            rects.iter().find(|r| r.node.contains('#')).expect("text").w
        })
        .collect()
}

#[test]
fn numeric_features_reach_the_shaper() {
    let tabular = NumericFeatures {
        tabular: true,
        lining: true,
        ..NumericFeatures::default()
    };
    let proportional = NumericFeatures {
        proportional: true,
        ..NumericFeatures::default()
    };
    let w = widths(&[
        ("1111", tabular),
        ("8888", tabular),
        ("1111", proportional),
        ("8888", proportional),
        ("AV", tabular),
        ("A", tabular),
        ("V", tabular),
    ]);
    assert_eq!(w[0], w[1], "tabular figures share one advance: {w:?}");
    assert!(w[2] < w[3], "proportional figures differ: {w:?}");
    assert!(
        w[2] < w[0],
        "a proportional one is narrower than a tabular one: {w:?}"
    );
    assert!(
        w[4] < w[5] + w[6],
        "AV is kerned with the figure features on: {w:?}"
    );
}
