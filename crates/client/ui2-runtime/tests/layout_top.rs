//! The top strip against Chrome: every element and text leaf within 0.5 px at the reference viewports.
//!
//! Inputs are the converter's top strip fixture (`tools/ui-convert/tests/fixtures/top-strip/`): the package
//! `skins/engraved` builds to (`top.ui2`, font subsets included) and Chrome's rectangle dump of the page with
//! the `hud` sample (`chrome-rects.json`, `node/rects.mjs`). The converter's `tests/layout_top.rs` makes both
//! again in Chromium and checks them the same way.
//!
//! List bindings are not in the runtime yet: the test instantiates the `for` and `if` bodies Chrome drew for
//! the sample (an `if` Chrome shows, one `for` item per key of the dump, in the dump's order).

use std::path::PathBuf;
use std::sync::Arc;

use ui2_draw::Scene;
use ui2_model::{Key, ModelStore, Sample, Schema};
use ui2_package::{NodeKind, Package};
use ui2_runtime::{Env, NodeClass, Runtime, Services, load_faces};
use ui2_text::TextEngine;

const TOLERANCE: f64 = 0.5;

fn env(vp: [u32; 2], dsf: f32) -> Env {
    Env {
        viewport_px: ((vp[0] as f32 * dsf) as u32, (vp[1] as f32 * dsf) as u32),
        dpr: dsf,
        ui_scale: 1.0,
        theme: 0,
    }
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// One record of a dump: node, keys, x y w h.
type Rect = (String, String, [f64; 4]);

/// Chrome's dump of one viewport and device scale.
type Dump = ([u32; 2], f32, Vec<Rect>);

/// The fixture: the package and, per viewport and device scale, Chrome's records.
fn fixture() -> (Arc<Package>, Vec<Dump>) {
    let dir = repo().join("tools/ui-convert/tests/fixtures/top-strip");
    let bytes = std::fs::read(dir.join("top.ui2")).expect("top.ui2");
    let pkg = Arc::new(Package::read(Arc::from(bytes)).expect("package"));
    let text = std::fs::read_to_string(dir.join("chrome-rects.json")).expect("chrome-rects.json");
    let json: serde_json::Value = serde_json::from_str(&text).expect("JSON");
    let dumps = json
        .as_array()
        .expect("dumps")
        .iter()
        .map(|d| {
            let vp = [0, 1].map(|i| d["viewport"][i].as_u64().unwrap_or(0) as u32);
            let dsf = d["dsf"].as_f64().unwrap_or(1.0) as f32;
            let rects = d["rects"]
                .as_array()
                .expect("rects")
                .iter()
                .map(|r| {
                    let s = |i: usize| r[i].as_str().unwrap_or_default().to_string();
                    (
                        s(0),
                        s(1),
                        [2, 3, 4, 5].map(|i| r[i].as_f64().expect("number")),
                    )
                })
                .collect();
            (vp, dsf, rects)
        })
        .collect();
    (pkg, dumps)
}

/// Instantiates the `for` and `if` bodies Chrome drew, until nothing is left to add.
fn expand(rt: &mut Runtime, pkg: &Package, chrome: &[Rect]) {
    let mut done = std::collections::BTreeSet::new();
    loop {
        let mut todo = Vec::new();
        for id in rt.arena().preorder() {
            let Some(n) = rt.node(id) else { continue };
            if n.class() != NodeClass::Service || done.contains(&id) {
                continue;
            }
            let r = n.tmpl();
            let (body, list) = match &pkg.templates()[r.0.index()].nodes[r.1 as usize].kind {
                NodeKind::For { tpl, .. } => (*tpl, true),
                NodeKind::If { tpl, .. } => (*tpl, false),
                _ => continue,
            };
            let root = format!("{}:0", body.0);
            let keys: Vec<String> = chrome
                .iter()
                .filter(|c| c.0 == root)
                .map(|c| c.1.clone())
                .collect();
            todo.push((id, list, keys));
        }
        if todo.is_empty() {
            return;
        }
        for (id, list, keys) in todo {
            done.insert(id);
            for k in keys {
                let key = list
                    .then(|| Key::parse_js(k.rsplit('/').next().unwrap_or_default()).expect("key"));
                rt.instantiate_body(id, key);
            }
        }
    }
}

#[test]
fn layout_top_strip_matches_chrome() {
    let (pkg, dumps) = fixture();
    let mut engine = TextEngine::new();
    load_faces(&pkg, &mut engine).expect("fonts");
    let mut rt = Runtime::new(
        pkg.clone(),
        env(dumps[0].0, dumps[0].1),
        Services::default(),
    )
    .expect("runtime");
    let sample =
        std::fs::read_to_string(repo().join("skins/engraved/sample/hud.json")).expect("sample");
    let mut store = ModelStore::new(Schema::v1());
    Sample::parse(Schema::v1(), &sample)
        .expect("sample")
        .apply(&mut store)
        .expect("apply");
    expand(&mut rt, &pkg, &dumps[0].2);
    if let Some(d) = store.take_frame() {
        rt.apply_delta(&d, &store);
    }
    let mut scene = Scene::new(1.0);

    let mut worst = (0.0f64, String::new());
    let mut failures = Vec::new();
    for (vp, dsf, chrome) in &dumps {
        rt.set_env(env(*vp, *dsf));
        rt.frame(0.0, &store, &mut engine, &mut scene);
        let ours = rt.dump_rects(None);
        let names = |v: Vec<String>| v.join(" ");
        assert_eq!(
            names(
                ours.iter()
                    .map(|r| format!("{}|{}", r.node, r.keys))
                    .collect()
            ),
            names(chrome.iter().map(|r| format!("{}|{}", r.0, r.1)).collect()),
            "the same records at {vp:?} x{dsf}"
        );
        for (o, (node, keys, c)) in ours.iter().zip(chrome) {
            let ov = [o.x, o.y, o.w, o.h].map(f64::from);
            let d = (0..4).map(|i| (ov[i] - c[i]).abs()).fold(0.0, f64::max);
            let line = format!("{node} [{keys}] at {vp:?} x{dsf}: ours {ov:?}, Chrome {c:?}");
            if d > worst.0 {
                worst = (d, line.clone());
            }
            if d > TOLERANCE {
                failures.push(line);
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

    // A part's dump is the subtree of that part.
    let research = rt.dump_rects(Some("top.research"));
    assert!(research.iter().any(|r| r.node.ends_with("#0")));
    assert!(research.iter().all(|r| r.node != "0:1" && r.node != "0:3"));
    assert!(rt.dump_rects(Some("no.such.part")).is_empty());

    // Laid out: idle frames do no layout and shape nothing.
    for _ in 0..100 {
        let s = rt.frame(0.0, &store, &mut engine, &mut scene).stats;
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
    let mut rt = Runtime::new(pkg, env([800, 600], 1.0), Services::default()).expect("runtime");
    let stats = rt
        .frame(
            0.0,
            &ModelStore::new(Schema::v1()),
            &mut engine,
            &mut Scene::new(1.0),
        )
        .stats;
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
