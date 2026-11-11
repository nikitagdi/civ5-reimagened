//! Fragment mode: the panels of a mod as a package of kind `Fragment` (`ui2-convert fragment`).
//!
//! The fixture is the `[ui]` test mod of `cargo xtask mods` (`tools/xtask/tests/fixtures/ui-mod`). Reading its
//! manifest needs no browser; building it needs `CIV5_UI2_CHROME=1` (Node and the Playwright Chromium), and the
//! built fragment is mounted in the stored engraved package (`tests/fixtures/top-strip/top.ui2`) with the
//! runtime, as the game does.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ui2_convert::fragment::{self, ENTRY_NAME, REFERENCE_TOKENS};
use ui2_draw::Scene;
use ui2_model::{Fragment, ModelStore, Namespace, Owner, Schema, Value};
use ui2_package::{Kind, NodeKind, Package};
use ui2_runtime::{Env, MountMode, NodeClass, NodeId, Runtime, Services, load_faces};
use ui2_text::TextEngine;

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

fn fixture() -> PathBuf {
    crate_dir().join("../xtask/tests/fixtures/ui-mod")
}

/// A scratch folder that goes away with the value.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let p = std::env::temp_dir().join(format!("ui2-fragment-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("mkdir");
        Scratch(p)
    }

    fn file(&self, rel: &str, text: &str) {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        std::fs::write(p, text).expect("write");
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn codes(errors: &[ui2_convert::errors::ConvError]) -> Vec<String> {
    let mut v: Vec<String> = errors
        .iter()
        .map(|e| format!("{} {}", e.code, e.place.file))
        .collect();
    v.sort();
    v.dedup();
    v
}

#[test]
fn the_fixture_manifest_becomes_a_fragment_source() {
    let src = fragment::load(&fixture()).unwrap_or_else(|e| panic!("{e:?}"));
    let f = src.skin.fragment.as_ref().expect("fragment");
    assert_eq!(f.mod_id, "uifixture");
    assert_eq!(f.version, "0.2.0");
    assert_eq!(f.folder, "ui");
    assert_eq!(f.points(), ["hud.right"]);
    assert_eq!(f.class_prefix(), "m-uifixture-");
    assert_eq!(src.skin.skin.entry, format!("ui/{ENTRY_NAME}"));
    assert_eq!(src.skin.skin.api, "1.0");
    assert_eq!(src.skin.scripts.files, ["ui/fixture.js"]);
    assert_eq!(src.skin.scripts.reads, ["game.turn"]);
    // The page: the panel included, the catalogue's tokens with reference values.
    let files = f.virtual_files();
    let page = &files[&f.entry()];
    assert!(
        page.contains("<div data-include=\"ui/echo.html\"></div>"),
        "{page}"
    );
    let css = &files[&f.tokens_file()];
    for (name, value) in REFERENCE_TOKENS {
        assert!(css.contains(&format!("{name}: {value};")), "{name}");
    }
}

#[test]
fn a_manifest_the_converter_cannot_use_is_reported() {
    let s = Scratch::new("manifest");
    s.file(
        "mod.toml",
        "[mod]\nid = \"two\"\nname = \"two\"\nversion = \"1.0.0\"\napi = \"^1.3\"\nlicense = \"MIT\"\nsync = \"client\"\n\
         parts = [\"ui\"]\n[ui]\napi = \"^1\"\n[[ui.panels]]\nid = \"a\"\nsource = \"ui/a.html\"\npoint = \"mods\"\nmode = \"append\"\n\
         [[ui.panels]]\nid = \"b\"\nsource = \"other/b.html\"\npoint = \"mods\"\nmode = \"append\"\n[[ui.panels]]\nid = \"c\"\n\
         source = \"ui/c.html\"\npoint = \"mods\"\nmode = \"append\"\n",
    );
    s.file("ui/a.html", "<div class=\"m-two-a\"></div>\n");
    s.file("other/b.html", "<div class=\"m-two-b\"></div>\n");
    let errs = fragment::load(&s.0).expect_err("refused");
    let got = codes(&errs);
    assert_eq!(got, ["E1-009 mod.toml", "E7-008 mod.toml"], "{errs:?}");
    assert!(errs.iter().any(|e| e.why.contains("ui/c.html")));
    assert!(errs.iter().any(|e| e.why.contains("other, ui")));
    let errs = fragment::load(&s.0.join("ui")).expect_err("no mod.toml");
    assert_eq!(codes(&errs), ["E9-004 mod.toml"]);
}

/// The texts of the subtree of a node, in order.
fn texts(rt: &Runtime, id: NodeId, out: &mut Vec<String>) {
    let Some(n) = rt.node(id) else { return };
    if n.class() == NodeClass::Text
        && let Some(t) = n.text()
    {
        out.push(t.to_string());
    }
    for &c in n.children() {
        texts(rt, c, out);
    }
}

/// The fixture through the converter: two builds give the same bytes, and the package mounts in the skin's `mods`
/// point with its texts bound to the mod's fields.
#[test]
fn the_fixture_builds_and_mounts_in_engraved() {
    if !chrome() {
        return;
    }
    let s = Scratch::new("build");
    let a = fragment::build(&fixture(), &s.0.join("a"));
    for w in &a.run.warnings {
        println!("warning: {w}");
    }
    assert!(a.run.errors.is_empty(), "{:#?}", a.run.errors);
    let wa = a.written.expect("package");
    let b = fragment::build(&fixture(), &s.0.join("b"));
    let wb = b.written.expect("package");
    assert_eq!(wa.file_name, wb.file_name);
    assert_eq!(wa.bytes, wb.bytes, "deterministic");
    assert_eq!(
        wa.file_name,
        format!("uifixture-0.2.0-{}.ui2", &wa.hash[..16])
    );
    assert_eq!(
        std::fs::read(s.0.join("a").join(&wa.file_name)).expect("written"),
        wa.bytes
    );
    let text = String::from_utf8_lossy(&wa.bytes);
    assert!(
        !text.contains(":\\") && !text.contains("/Users/"),
        "no machine paths"
    );

    let frag = Arc::new(Package::read(Arc::from(wa.bytes.as_slice())).expect("reads"));
    let m = frag.manifest();
    assert_eq!(
        m.kind,
        Kind::Fragment {
            slots: vec!["hud.right".into()]
        }
    );
    assert_eq!((m.id.as_str(), m.version.as_str()), ("uifixture", "0.2.0"));
    assert_eq!(m.api, (1, 0));
    assert!(frag.fonts().is_empty());
    // The script, then the model fragment under its reserved name.
    let scripts: Vec<&str> = frag.scripts().map(|(p, _)| p).collect();
    assert_eq!(scripts, ["ui/fixture.js", fragment::MODEL_FILE]);
    let packed_model = frag
        .scripts()
        .find(|(p, _)| *p == fragment::MODEL_FILE)
        .map(|(_, b)| String::from_utf8(b.to_vec()).expect("text"))
        .expect("the model is in the package");
    // Template 0 holds the panel, named by its id.
    let doc = &frag.templates()[0];
    assert_eq!(
        doc.nodes[0].subtree as usize,
        doc.nodes.len(),
        "one panel root"
    );
    assert_eq!(doc.nodes[0].part.and_then(|p| frag.str(p)), Some("echo"));
    assert!(matches!(doc.nodes[0].kind, NodeKind::Box { .. }));
    // The mod's own fields are not paths of the shared model.
    assert!(
        frag.bindings()
            .used_paths
            .iter()
            .all(|(p, _)| !frag.str(*p).unwrap_or("").starts_with("mods."))
    );
    // Its colors are the catalogue's tokens, by name.
    let tokens: Vec<&str> = frag
        .tokens()
        .roots
        .iter()
        .filter_map(|(n, _)| frag.str(*n))
        .collect();
    assert!(tokens.contains(&"--ui-text"), "{tokens:?}");

    // Mounted in the stored engraved package, in the `mods` point.
    let skin_bytes =
        std::fs::read(crate_dir().join("tests/fixtures/top-strip/top.ui2")).expect("top.ui2");
    let skin = Arc::new(Package::read(Arc::from(skin_bytes.as_slice())).expect("skin"));
    assert!(
        skin.slots().any(|(n, _)| n == "mods"),
        "the skin has `mods`"
    );
    let mut engine = TextEngine::new();
    load_faces(&skin, &mut engine).expect("faces");
    let mut rt = Runtime::new(
        skin,
        Env {
            viewport_px: (1920, 1080),
            ..Env::default()
        },
        Services::default(),
    )
    .expect("runtime");
    let mut store = ModelStore::new(Schema::v1());
    let model = std::fs::read_to_string(fixture().join("ui/model.toml")).expect("model.toml");
    assert_eq!(packed_model, model.replace("\r\n", "\n"));
    store
        .mount(
            Namespace::Mod(Arc::from("uifixture")),
            &Fragment::parse(&model.replace("\r\n", "\n")).expect("model fragment"),
        )
        .expect("mount");
    let panel = rt
        .mount_fragment(frag, "echo", "mods", MountMode::Append, 0)
        .unwrap_or_else(|e| panic!("{e}"));
    store
        .writer(Owner::Mod(Arc::from("uifixture")))
        .set_value("mods.uifixture.count", Value::I32(7))
        .expect("count");
    let mut scene = Scene::new(1.0);
    for i in 0..3 {
        if let Some(d) = store.take_frame() {
            rt.apply_delta(&d, &store);
        }
        rt.frame(16.0 * f64::from(i), &store, &mut engine, &mut scene);
    }
    let (root, at) = rt.panel_root(panel).expect("the panel root exists");
    assert_eq!(at.1, 0, "the panel is the fragment's root node");
    let mut shown = Vec::new();
    texts(&rt, root, &mut shown);
    assert_eq!(shown, ["7", "Say"]);
    let status = rt.panels();
    let p = status.iter().find(|p| p.id == panel).expect("status");
    assert_eq!(
        (p.name.as_str(), p.slot.as_deref(), p.failed.as_deref()),
        ("echo", Some("mods"), None)
    );
    assert!(
        rt.node(root)
            .is_some_and(|n| n.layout().unrounded.size.height > 0.0)
    );
}

/// The rules of a fragment, each at its place.
#[test]
fn fragment_rules_are_errors_at_their_place() {
    if !chrome() {
        return;
    }
    let s = Scratch::new("rules");
    s.file(
        "mod.toml",
        "[mod]\nid = \"bad\"\nname = \"bad\"\nversion = \"1.0.0\"\napi = \"^1.3\"\nlicense = \"MIT\"\nsync = \"client\"\n\
         parts = [\"ui\"]\n[ui]\napi = \"^1\"\n[[ui.panels]]\nid = \"one\"\nsource = \"ui/one.html\"\npoint = \"mods\"\nmode = \"append\"\n\
         [[ui.panels]]\nid = \"two\"\nsource = \"ui/two.html\"\npoint = \"hud.right\"\nmode = \"append\"\n",
    );
    s.file(
        "ui/one.html",
        "<link rel=\"stylesheet\" href=\"one.css\">\n<div class=\"m-bad-one\" data-part=\"first\">\n  <span class=\"plain\">Hi</span>\n  <div class=\"m-bad-x\" data-slot=\"mods\"></div>\n</div>\n",
    );
    s.file(
        "ui/two.html",
        "<div class=\"m-bad-a\"></div>\n<div class=\"m-bad-b\"></div>\n",
    );
    s.file(
        "ui/one.css",
        "@font-face {\n  font-family: \"X\";\n  src: url(\"x.ttf\");\n}\n\n:root {\n  --ui-text: #fff;\n}\n\n.m-bad-one {\n  display: flex;\n  border-top: 1px solid var(--ui-nope);\n}\n\n.plain {\n  font-family: Arial;\n}\n",
    );
    let o = fragment::build(&s.0, &s.0.join("out"));
    assert!(o.written.is_none());
    let mut got: Vec<String> = o
        .run
        .errors
        .iter()
        .map(|e| format!("{} {}:{}", e.code, e.place.file, e.place.line))
        .collect();
    got.sort();
    got.dedup();
    assert_eq!(
        got,
        [
            "E5-009 ui/one.css:1",
            "E5-009 ui/one.html:3",
            "E7-008 ui/one.css:12",
            "E7-008 ui/one.css:7",
            "E7-008 ui/one.html:2",
            "E7-008 ui/one.html:4",
            "E7-008 ui/two.html:0",
            "E7-009 ui/one.css:15",
            "E7-009 ui/one.html:3",
        ],
        "{:#?}",
        o.run.errors
    );
}
