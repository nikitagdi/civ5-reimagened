//! The top strip end to end: `skins/engraved` built to a package, the package run in `ui2-runtime` with the
//! text engine on the package's own font subsets and the `hud` sample, and the runtime's rectangle dump compared
//! with Chrome's (`node/rects.mjs`) at the reference viewports: the same records, every edge within 0.5 px,
//! text leaves included.
//!
//! The runtime does not instantiate `for` and `if` bodies from the model yet (list bindings come later); the
//! test instantiates the bodies Chrome shows for the sample (an `if` that Chrome drew, a `for` item per key
//! of Chrome's dump, in Chrome's order) and the runtime binds them like any instance.
//!
//! Needs `CIV5_UI2_CHROME=1`, Node with the Playwright install and Python with fontTools; skips otherwise.
//! With `CIV5_UI2_WRITE_FIXTURE=1` it also writes `tests/fixtures/top-strip/{top.ui2,chrome-rects.json}`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use ui2_draw::Scene;
use ui2_model::{Key, ModelStore, Sample, Schema};
use ui2_package::{NodeKind, Package};
use ui2_runtime::{Env, NodeClass, Runtime, Services, load_faces};
use ui2_text::TextEngine;

const TOLERANCE: f64 = 0.5;

/// The reference viewports: CSS size and device scale.
const VIEWPORTS: &[([u32; 2], f32)] = &[
    ([1280, 720], 1.0),
    ([1920, 1200], 1.0),
    ([3440, 1440], 1.0),
    ([5120, 1440], 1.0),
    ([1280, 720], 1.5),
];

fn chrome() -> bool {
    if std::env::var("CIV5_UI2_CHROME").ok().as_deref() != Some("1") {
        eprintln!("skipped: CIV5_UI2_CHROME is not set");
        return false;
    }
    match ui2_convert::node::available() {
        Ok(()) => true,
        Err(why) => {
            eprintln!("skipped: {why}");
            false
        }
    }
}

fn crate_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// One record of a dump.
#[derive(Debug, Clone, PartialEq)]
struct Rect {
    node: String,
    keys: String,
    r: [f64; 4],
}

fn rects_of(dump: &Value) -> Vec<Rect> {
    dump["rects"]
        .as_array()
        .expect("rects")
        .iter()
        .map(|r| Rect {
            node: r[0].as_str().unwrap_or_default().to_string(),
            keys: r[1].as_str().unwrap_or_default().to_string(),
            r: [2, 3, 4, 5].map(|i| r[i].as_f64().expect("number")),
        })
        .collect()
}

/// Instantiates the `for` and `if` bodies that Chrome's dump shows, until nothing is left to add.
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
            let kind = &pkg.templates()[r.0.index()].nodes[r.1 as usize].kind;
            let (body, list) = match kind {
                NodeKind::For { tpl, .. } => (*tpl, true),
                NodeKind::If { tpl, .. } => (*tpl, false),
                _ => continue,
            };
            let root = format!("{}:0", body.0);
            let keys: Vec<String> = chrome
                .iter()
                .filter(|c| c.node == root)
                .map(|c| c.keys.clone())
                .collect();
            todo.push((id, list, keys));
        }
        if todo.is_empty() {
            return;
        }
        for (id, list, keys) in todo {
            done.insert(id);
            for k in keys {
                let key = if list {
                    let last = k.rsplit('/').next().unwrap_or_default();
                    Some(Key::parse_js(last).expect("a key of the dump"))
                } else {
                    None
                };
                rt.instantiate_body(id, key);
            }
        }
    }
}

#[test]
fn layout_top_strip_package_matches_chrome() {
    if !chrome() {
        return;
    }
    let skin = crate_dir().join("../../skins/engraved");
    let out = ui2_convert::build::build(&skin, true);
    assert!(out.run.errors.is_empty(), "{:?}", out.run.errors);
    let written = out.written.expect("package");

    // Chrome's dumps.
    let src = out.run.source.as_ref().expect("source");
    let mut node = ui2_convert::node::NodeClient::start().expect("node");
    let opts = ui2_convert::Opts {
        fonts_dir: Some(out.out_dir.join("fonts")),
    };
    ui2_convert::open(&mut node, src, &opts).expect("open");
    let mut dumps = Vec::new();
    for (vp, dsf) in VIEWPORTS {
        let d = node
            .call(
                "rects",
                json!({ "state": "hud", "viewport": vp, "dsf": dsf, "skin": "engraved" }),
            )
            .expect("rects");
        dumps.push(d);
    }
    node.close();

    // The runtime on the package.
    let pkg = Arc::new(Package::read(Arc::from(written.bytes.as_slice())).expect("read"));
    let mut engine = TextEngine::new();
    load_faces(&pkg, &mut engine).expect("fonts");
    let sample_text = std::fs::read_to_string(skin.join("sample/hud.json")).expect("sample");
    let sample = Sample::parse(Schema::v1(), &sample_text).expect("sample");
    let env = |vp: [u32; 2], dsf: f32| Env {
        viewport_px: ((vp[0] as f32 * dsf) as u32, (vp[1] as f32 * dsf) as u32),
        dpr: dsf,
        ui_scale: 1.0,
        theme: 0,
    };
    let mut rt =
        Runtime::new(pkg.clone(), env(VIEWPORTS[0].0, 1.0), Services::default()).expect("runtime");
    let mut store = ModelStore::new(Schema::v1());
    sample.apply(&mut store).expect("apply");
    let mut scene = Scene::new(1.0);
    expand(&mut rt, &pkg, &rects_of(&dumps[0]));
    if let Some(d) = store.take_frame() {
        rt.apply_delta(&d, &store);
    }

    let mut report = Vec::new();
    let mut failures = Vec::new();
    for ((vp, dsf), dump) in VIEWPORTS.iter().zip(&dumps) {
        rt.set_env(env(*vp, *dsf));
        rt.frame(0.0, &store, &mut engine, &mut scene);
        let ours = rt.dump_rects(None);
        let theirs = rects_of(dump);
        let names = |v: Vec<String>| v.join(" ");
        assert_eq!(
            names(
                ours.iter()
                    .map(|r| format!("{}|{}", r.node, r.keys))
                    .collect()
            ),
            names(
                theirs
                    .iter()
                    .map(|r| format!("{}|{}", r.node, r.keys))
                    .collect()
            ),
            "the same records at {vp:?} x{dsf}"
        );
        let mut worst = (0.0f64, String::new());
        for (o, c) in ours.iter().zip(&theirs) {
            let ov = [o.x, o.y, o.w, o.h].map(f64::from);
            let d = (0..4).map(|i| (ov[i] - c.r[i]).abs()).fold(0.0, f64::max);
            let line = format!(
                "{} [{}] at {vp:?} x{dsf}: ours {ov:?}, Chrome {:?}",
                c.node, c.keys, c.r
            );
            if d > worst.0 {
                worst = (d, line.clone());
            }
            if d > TOLERANCE {
                failures.push(line);
            }
        }
        report.push(format!(
            "{}x{} dsf {dsf}: {} records, worst {:.4} px ({})",
            vp[0],
            vp[1],
            ours.len(),
            worst.0,
            worst.1
        ));
    }
    for l in &report {
        println!("{l}");
    }
    let errors = rt.take_errors();
    assert!(
        failures.is_empty(),
        "{} records off by more than {TOLERANCE} px:\n{}",
        failures.len(),
        failures.join("\n")
    );
    // Painting reports what it cannot draw yet (the gradient of the research scale, outlines); nothing else.
    for e in &errors {
        let s = format!("{e:?}");
        assert!(
            s.contains("gradient") || s.contains("outline") || s.contains("game asset"),
            "{s}"
        );
    }

    if std::env::var("CIV5_UI2_WRITE_FIXTURE").ok().as_deref() == Some("1") {
        let dir = crate_dir().join("tests/fixtures/top-strip");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("top.ui2"), &written.bytes).expect("top.ui2");
        let mut text = String::from("[\n");
        for (i, d) in dumps.iter().enumerate() {
            text.push_str(&format!(
                "{{\"v\": 1, \"skin\": {}, \"state\": {}, \"viewport\": {}, \"dsf\": {}, \"rects\": [\n",
                d["skin"], d["state"], d["viewport"], d["dsf"]
            ));
            let rows: Vec<String> = d["rects"]
                .as_array()
                .expect("rects")
                .iter()
                .map(|r| r.to_string())
                .collect();
            text.push_str(&rows.join(",\n"));
            text.push_str(if i + 1 < dumps.len() {
                "\n]},\n"
            } else {
                "\n]}\n"
            });
        }
        text.push_str("]\n");
        std::fs::write(dir.join("chrome-rects.json"), text).expect("chrome-rects.json");
    }
}
