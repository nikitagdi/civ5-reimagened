//! Scalar bindings: texts, skin texts, class toggles, style properties and the dependency index.
//!
//! The top strip runs with the bindings of `parts/top.html` and the sample state `sample/hud.json`: the bound
//! texts must be the sample texts Chrome laid out, the boxes must match Chrome again, and a change of one
//! model field must evaluate exactly the bindings that read it.

mod support;

use std::collections::BTreeMap;

use serde_json::json;
use ui2_model::{FrameDelta, ModelStore, Sample, Schema};
use ui2_package::style::{ColorV, Display, Len, LenAuto, StyleRecord};
use ui2_package::{BindKind, Layer, PropId};
use ui2_runtime::testkit::{Kit, TNode, TplPiece, path};
use ui2_runtime::{Env, ErrorCode, NodeClass, NodeId, Runtime, Services, StateBits, load_faces};
use ui2_text::TextEngine;

fn env(vp: [u32; 2]) -> Env {
    Env {
        viewport_px: (vp[0], vp[1]),
        dpr: 1.0,
        ui_scale: 1.0,
        theme: 0,
    }
}

/// Writes a sample state into the store and takes the frame.
fn frame_of(store: &mut ModelStore, sample: &serde_json::Value) -> FrameDelta {
    let s = Sample::parse(Schema::v1(), &sample.to_string()).expect("sample");
    s.apply(store).expect("apply");
    store.take_frame().expect("a change")
}

/// The text leaves by node name, with their strings.
fn texts(rt: &Runtime) -> BTreeMap<String, String> {
    rt.arena()
        .preorder()
        .into_iter()
        .filter_map(|id| {
            let n = rt.node(id)?;
            (n.class() == NodeClass::Text)
                .then(|| (rt.path(id).label, n.text().unwrap_or("").to_string()))
        })
        .collect()
}

fn node_named(rt: &Runtime, label: &str) -> NodeId {
    rt.arena()
        .preorder()
        .into_iter()
        .find(|&id| rt.path(id).label == label)
        .unwrap_or_else(|| panic!("no node {label}"))
}

fn rect(rt: &Runtime, label: &str) -> [f32; 4] {
    let r = rt
        .dump_rects(None)
        .into_iter()
        .find(|r| r.node == label)
        .unwrap_or_else(|| panic!("no rect {label}"));
    [r.x, r.y, r.w, r.h]
}

#[test]
fn bind_top_strip_shows_the_sample_and_reevaluates_only_readers() {
    let fx = support::load();
    let pkg = support::package_with(&fx, support::Texts::Bound);
    let mut engine = TextEngine::new();
    load_faces(&pkg, &mut engine).expect("fonts");
    let (vp, chrome) = &fx.viewports[1];
    let mut rt = Runtime::new(pkg, env(*vp), Services::default()).expect("runtime");
    let mut store = ModelStore::new(Schema::v1());
    let mut sample = support::sample_json();

    // The whole sample: every binding is evaluated once.
    let d = frame_of(&mut store, &sample);
    rt.apply_delta(&d, &store);
    let s = rt.update(&store, &mut engine);
    assert_eq!(rt.binding_rows(), 16, "13 texts, 2 class toggles, 1 width");
    assert_eq!(s.binds_evaluated, 16);
    assert_eq!(s.layout_runs, 1);
    let errors = rt.take_errors();
    assert!(errors.is_empty(), "{errors:?}");

    // The texts are the sample texts of the mockup.
    let ours = texts(&rt);
    for n in fx.nodes.iter().filter(|n| n.text.is_some()) {
        assert_eq!(ours.get(&n.node), n.text.as_ref(), "{}", n.node);
    }
    assert_eq!(ours["0:38#0"], "312");
    assert_eq!(ours["0:9#0"], "3 turns");
    assert_eq!(ours["0:47#0"], "6 turns");
    assert_eq!(ours["0:43#0"], "412/650");

    // Laid out as Chrome laid out the static sample, the research fill sized by its binding (62 %).
    let dump = rt.dump_rects(None);
    for (o, (node, _, c)) in dump.iter().zip(chrome) {
        assert_eq!(&o.node, node);
        let ov = [o.x, o.y, o.w, o.h].map(f64::from);
        let d = (0..4).map(|i| (ov[i] - c[i]).abs()).fold(0.0, f64::max);
        assert!(d <= 0.5, "{node}: ours {ov:?}, Chrome {c:?}");
    }

    // Idle: nothing is evaluated, shaped or laid out.
    for _ in 0..100 {
        let s = rt.update(&store, &mut engine);
        assert_eq!(
            (
                s.binds_evaluated,
                s.shaped_runs,
                s.layout_runs,
                s.restyled,
                s.visited
            ),
            (0, 0, 0, 0, 0)
        );
    }

    // One field: one binding, one run, no layout (tabular figures keep the width).
    sample["model"]["top"]["gold"] = json!(313);
    let d = frame_of(&mut store, &sample);
    assert_eq!(d.ops.len(), 1);
    rt.apply_delta(&d, &store);
    let s = rt.update(&store, &mut engine);
    assert_eq!(
        (s.binds_evaluated, s.shaped_runs, s.layout_runs, s.restyled),
        (1, 1, 0, 0)
    );
    assert_eq!(texts(&rt)["0:38#0"], "313");

    // A negative rate: its text and its class toggle; the record with `.neg` is selected, the width changed.
    sample["model"]["top"]["gold_rate"] = json!(-5);
    let d = frame_of(&mut store, &sample);
    rt.apply_delta(&d, &store);
    let s = rt.update(&store, &mut engine);
    assert_eq!(s.binds_evaluated, 2);
    assert_eq!(s.shaped_runs, 1);
    assert_eq!(s.layout_runs, 1);
    assert_eq!(s.restyled, 2, "the span and its text leaf");
    assert_eq!(texts(&rt)["0:39#0"], "-5");
    let span = node_named(&rt, "0:39");
    assert_eq!(
        rt.style(span).map(|r| r.color),
        Some(ColorV::Lit(support::NEG_COLOR))
    );

    // The research fraction: only the fill's width binding; the fill is half the scale.
    sample["model"]["research"]["current"]["fraction"] = json!(0.5);
    let d = frame_of(&mut store, &sample);
    rt.apply_delta(&d, &store);
    let s = rt.update(&store, &mut engine);
    assert_eq!((s.binds_evaluated, s.shaped_runs), (1, 0));
    assert_eq!(s.layout_runs, 1);
    let (fill, scale) = (rect(&rt, "0:12"), rect(&rt, "0:11"));
    assert!(
        (fill[2] - scale[2] * 0.5).abs() < 0.01,
        "{fill:?} in {scale:?}"
    );
    assert!(rt.take_errors().is_empty());
}

/// A small skin: a bound text, a skin text, a class toggle, a disabled binding and a style width.
fn kit_runtime(game: Option<std::rc::Rc<loc::Localizer>>) -> (Runtime, TextEngine) {
    let fira = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../third_party/fonts/fira-sans/FiraSans-Regular.ttf"),
    )
    .expect("font");
    let mut kit = Kit::new();
    kit.font("Fira Sans", 400, false, &fira);
    let mut rec = StyleRecord::initial();
    rec.display = Display::Block;
    rec.font_family = vec![kit.str("Fira Sans")];
    rec.font_size = Len::px(14.0);
    let mut hot = rec.clone();
    hot.color = ColorV::Lit([255, 0, 0, 255]);
    let (plain, hot) = (kit.record(rec), kit.record(hot));
    let set = kit.plain(plain);
    let class = kit.str("hot");
    let toggled = kit.set(
        vec![ui2_package::Cond {
            up: 0,
            kind: ui2_package::CondKind::Class(class),
        }],
        vec![plain, hot],
    );
    let gold = kit.tpl_of(vec![
        TplPiece::Lit("Gold ".into()),
        TplPiece::Expr(path("top.gold")),
    ]);
    let width = kit.tpl_of(vec![
        TplPiece::Expr(path("top.happiness")),
        TplPiece::Lit("px".into()),
    ]);
    let missing = kit.tpl_of(vec![TplPiece::Expr(path("top.no_such_field"))]);
    let neg = kit.expr(ui2_package::profile::expr::parse("top.gold_rate < 0").expect("expr"));
    let year = kit.expr(path("game.year"));
    kit.l10n("en_US", "x.year", "Year {1_Num}");
    kit.l10n("en_US", "x.unused", "never shown");
    let key = kit.str("x.year");
    let absent = kit.str("x.absent");
    let parts = ["gold", "year", "bar", "absent", "broken"].map(|p| kit.str(p));
    kit.template(
        "doc",
        vec![
            TNode::layer(Layer::Hud, set).children([
                TNode::el(toggled)
                    .part(parts[0])
                    .bind(BindKind::ClassToggle { class, cond: neg })
                    .bind(BindKind::Disabled(neg))
                    .child(TNode::bound_text(gold, toggled)),
                TNode::el(set)
                    .part(parts[1])
                    .child(TNode::l10n_text(key, vec![year], set)),
                TNode::el(set).part(parts[2]).bind(BindKind::Style {
                    prop: PropId::Width,
                    tpl: width,
                }),
                TNode::el(set)
                    .part(parts[3])
                    .child(TNode::l10n_text(absent, vec![], set)),
                TNode::el(set)
                    .part(parts[4])
                    .child(TNode::bound_text(missing, set)),
            ]),
        ],
    );
    let pkg = kit.finish().expect("package");
    let mut engine = TextEngine::new();
    load_faces(&pkg, &mut engine).expect("fonts");
    let rt = Runtime::new(pkg, env([800, 600]), Services { loc: game }).expect("runtime");
    (rt, engine)
}

fn text_of(rt: &Runtime, part: &str) -> String {
    let id = rt.find_part(part).expect("part");
    let leaf = rt.node(id).expect("node").children()[0];
    rt.node(leaf)
        .and_then(|n| n.text())
        .unwrap_or("")
        .to_string()
}

#[test]
fn bind_scalars_toggle_classes_states_and_styles() {
    let (mut rt, mut engine) = kit_runtime(None);
    let mut store = ModelStore::new(Schema::v1());
    let mut sample = support::sample_json();
    let d = frame_of(&mut store, &sample);
    rt.apply_delta(&d, &store);
    let s = rt.update(&store, &mut engine);
    assert_eq!(rt.binding_rows(), 7);
    assert_eq!(s.binds_evaluated, 7);
    assert_eq!(text_of(&rt, "gold"), "Gold 312");
    // Composed by the game's text engine: `{1_Num}` groups digits.
    assert_eq!(text_of(&rt, "year"), "Year 1,250");
    // A missing skin text shows its key; a missing path reads null. Each is reported once.
    assert_eq!(text_of(&rt, "absent"), "x.absent");
    assert_eq!(text_of(&rt, "broken"), "");
    let mut errors = rt.take_errors();
    errors.sort_by_key(|e| e.path.clone());
    let codes: Vec<ErrorCode> = errors.iter().map(|e| e.code).collect();
    assert_eq!(codes, vec![ErrorCode::L10n, ErrorCode::Bind], "{errors:?}");
    let bar = rt.find_part("bar").expect("bar");
    assert_eq!(
        rt.style(bar).map(|r| r.width),
        Some(LenAuto::Len(Len::px(7.0)))
    );
    assert_eq!(rt.dump_rects(Some("bar"))[0].w, 7.0);
    let gold = rt.find_part("gold").expect("gold");
    assert!(
        !rt.node(gold)
            .expect("gold")
            .state()
            .contains(StateBits::DISABLED)
    );

    // A negative rate turns the class and the disabled state on: two rows, the node and its leaf restyle.
    sample["model"]["top"]["gold_rate"] = json!(-3);
    let d = frame_of(&mut store, &sample);
    rt.apply_delta(&d, &store);
    let s = rt.update(&store, &mut engine);
    assert_eq!((s.binds_evaluated, s.restyled), (2, 2));
    assert_eq!(s.layout_runs, 0, "a color does not move anything");
    let n = rt.node(gold).expect("gold");
    assert!(n.state().contains(StateBits::DISABLED));
    assert_eq!(
        rt.style(gold).map(|r| r.color),
        Some(ColorV::Lit([255, 0, 0, 255]))
    );

    // The bound width follows the model; the old value is replaced, not stacked.
    sample["model"]["top"]["happiness"] = json!(12);
    let d = frame_of(&mut store, &sample);
    rt.apply_delta(&d, &store);
    let s = rt.update(&store, &mut engine);
    assert_eq!((s.binds_evaluated, s.layout_runs), (1, 1));
    assert_eq!(rt.dump_rects(Some("bar"))[0].w, 12.0);
    assert_eq!(
        rt.node(bar)
            .and_then(|n| n.overrides())
            .map(|o| o.props.len()),
        Some(1)
    );

    // Errors are not reported again; idle frames evaluate nothing.
    for _ in 0..10 {
        let s = rt.update(&store, &mut engine);
        assert_eq!((s.binds_evaluated, s.visited), (0, 0));
    }
    assert!(rt.take_errors().is_empty());
}

#[test]
fn bind_a_new_localizer_reevaluates_the_localized_rows_only() {
    let (mut rt, mut engine) = kit_runtime(None);
    let mut store = ModelStore::new(Schema::v1());
    let d = frame_of(&mut store, &support::sample_json());
    rt.apply_delta(&d, &store);
    rt.update(&store, &mut engine);
    rt.take_errors();
    let game = loc::Localizer::new(loc::LanguageInfo::en_us(), Box::new(loc::MapTable::new()));
    rt.set_localizer(std::rc::Rc::new(game));
    let s = rt.update(&store, &mut engine);
    // The two skin texts; the bound gold text, the toggles and the width do not depend on the language.
    assert_eq!(s.binds_evaluated, 2);
    assert_eq!(s.shaped_runs, 0, "the same texts");
    assert_eq!(text_of(&rt, "year"), "Year 1,250");
}

#[test]
fn bind_removed_nodes_free_their_rows() {
    let (mut rt, mut engine) = kit_runtime(None);
    let mut store = ModelStore::new(Schema::v1());
    let mut sample = support::sample_json();
    let d = frame_of(&mut store, &sample);
    rt.apply_delta(&d, &store);
    rt.update(&store, &mut engine);
    let gold = rt.find_part("gold").expect("gold");
    rt.remove(gold);
    sample["model"]["top"]["gold"] = json!(1);
    sample["model"]["top"]["gold_rate"] = json!(-1);
    let d = frame_of(&mut store, &sample);
    rt.apply_delta(&d, &store);
    let s = rt.update(&store, &mut engine);
    assert_eq!(rt.binding_rows(), 4);
    assert_eq!(s.binds_evaluated, 0, "the readers were removed");
}
