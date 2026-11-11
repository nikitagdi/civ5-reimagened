//! The top strip against Chrome: every element and text leaf within 0.5 px at the four reference viewports.
//!
//! TEMPORARY fixture (`tests/fixtures/top-strip-chrome.json`, made by `tests/support/top_strip_chrome.mjs`);
//! the converter's top strip package and rectangle dump replace it.

mod support;

use ui2_model::{ModelStore, Schema};
use ui2_runtime::{Env, Runtime, Services, load_faces};
use ui2_text::TextEngine;

const TOLERANCE: f64 = 0.5;

fn env(vp: [u32; 2]) -> Env {
    Env {
        viewport_px: (vp[0], vp[1]),
        dpr: 1.0,
        ui_scale: 1.0,
        theme: 0,
    }
}

#[test]
fn layout_top_strip_matches_chrome() {
    let fx = support::load();
    let pkg = support::package(&fx);
    let mut engine = TextEngine::new();
    load_faces(&pkg, &mut engine).expect("fonts");
    let mut rt = Runtime::new(pkg, env(fx.viewports[0].0), Services::default()).expect("runtime");
    let store = ModelStore::new(Schema::v1());

    let mut worst = (0.0f64, String::new());
    let mut failures = Vec::new();
    for (vp, chrome) in &fx.viewports {
        rt.set_env(env(*vp));
        let stats = rt.update(&store, &mut engine);
        assert_eq!(stats.layout_runs, 1, "{vp:?}");
        let ours = rt.dump_rects(None);
        let names = |v: Vec<String>| v.join(" ");
        assert_eq!(
            names(ours.iter().map(|r| r.node.clone()).collect()),
            names(chrome.iter().map(|r| r.0.clone()).collect()),
            "the same nodes at {vp:?}"
        );
        for (o, (node, keys, c)) in ours.iter().zip(chrome) {
            assert_eq!(&o.keys, keys);
            let ov = [o.x, o.y, o.w, o.h].map(f64::from);
            let d = (0..4).map(|i| (ov[i] - c[i]).abs()).fold(0.0, f64::max);
            if d > worst.0 {
                worst = (d, format!("{node} at {vp:?}: ours {ov:?}, Chrome {c:?}"));
            }
            if d > TOLERANCE {
                failures.push(format!("{node} at {vp:?}: ours {ov:?}, Chrome {c:?}"));
            }
        }
    }
    println!("worst delta {:.4} px: {}", worst.0, worst.1);
    assert!(
        failures.is_empty(),
        "{} rects off by more than {TOLERANCE} px:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(rt.take_errors().is_empty());

    // A part's dump is the subtree of that part.
    let research = rt.dump_rects(Some("top.research"));
    assert_eq!(research.len(), 16, "12 elements and 4 text leaves");
    assert!(research.iter().all(|r| r.node != "0:1" && r.node != "0:14"));
    assert!(rt.dump_rects(Some("no.such.part")).is_empty());

    // Laid out: idle frames do no layout and shape nothing.
    for _ in 0..100 {
        let s = rt.update(&store, &mut engine);
        assert_eq!(
            (s.layout_runs, s.shaped_runs, s.restyled, s.visited),
            (0, 0, 0, 0)
        );
    }
}

/// A text leaf in a narrow box wraps at spaces; with `white-space: nowrap` it stays on one line.
#[test]
fn layout_text_wraps_unless_nowrap() {
    use ui2_package::style::{Display, Len, LenAuto, LineHeight, StyleRecord, TextWrapMode};
    use ui2_package::{Layer, StrId};
    use ui2_runtime::testkit::{Kit, TNode};

    let fira = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../third_party/fonts/fira-sans/FiraSans-Regular.ttf"),
    )
    .expect("font");
    let mut kit = Kit::new();
    kit.font("Fira Sans", 400, false, &fira);
    let family: Vec<StrId> = vec![kit.str("Fira Sans")];
    let mut block = StyleRecord::initial();
    block.display = Display::Block;
    block.font_family = family;
    block.font_size = Len::px(14.0);
    block.line_height = LineHeight::Px(20.0);
    let root = kit.record(block.clone());
    let root = kit.plain(root);
    let mut narrow = block.clone();
    narrow.width = LenAuto::Len(Len::px(60.0));
    let mut nowrap = narrow.clone();
    nowrap.text_wrap_mode = TextWrapMode::Nowrap;
    let narrow = kit.record(narrow);
    let narrow = kit.plain(narrow);
    let nowrap = kit.record(nowrap);
    let nowrap = kit.plain(nowrap);
    let text = kit.str("Turns production into gold");
    let (a, b) = (kit.str("wrap"), kit.str("nowrap"));
    kit.template(
        "doc",
        vec![TNode::layer(Layer::Hud, root).children([
            TNode::el(narrow).part(a).child(TNode::text(text, narrow)),
            TNode::el(nowrap).part(b).child(TNode::text(text, nowrap)),
        ])],
    );
    let pkg = kit.finish().expect("package");
    let mut engine = TextEngine::new();
    load_faces(&pkg, &mut engine).expect("fonts");
    let mut rt = Runtime::new(pkg, env([800, 600]), Services::default()).expect("runtime");
    let stats = rt.update(&ModelStore::new(Schema::v1()), &mut engine);
    assert_eq!(stats.shaped_runs, 2);
    let rect = |part: &str, i: usize| {
        let r = &rt.dump_rects(Some(part))[i];
        (r.w, r.h)
    };
    // The box keeps its width; the text wraps into three lines of 20 px: "Turns", "production" (wider than
    // the box: a word is never cut), "into gold".
    assert_eq!(rect("wrap", 0), (60.0, 60.0));
    let (w, h) = rect("wrap", 1);
    assert!(w > 60.0 && h == 57.0, "wrapped text {w} x {h}");
    // Nowrap: one line overflowing the box.
    assert_eq!(rect("nowrap", 0), (60.0, 20.0));
    let (w, h) = rect("nowrap", 1);
    assert!(w > 150.0 && h == 17.0, "one line {w} x {h}");
    assert!(rt.take_errors().is_empty());
}
