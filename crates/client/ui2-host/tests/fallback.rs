//! Fatal skin failures with the broken packages of `tests/fixtures` (also served to the `ui2-host` playtest):
//!
//! - `badhash`: a good package whose index entry names another hash;
//! - `api9`: a package for script API 9 (its index entry says so too: refused before the download);
//! - `scriptfail`: a package whose script throws while it loads (fatal on the skin's first start).
//!
//! Each one is refused and falls back to the default skin, then to the classic UI, and no skin is tried twice.
//! `cargo test -p ui2-host --test fallback -- --ignored write_the_fixtures` writes the files again.

use std::path::PathBuf;
use std::sync::Arc;

use ui2_host::{Fallback, LoadFailure, SkinRegistry, V2Frame, V2Init, V2Ui, hash_hex};
use ui2_package::style::StyleRecord;
use ui2_package::{Layer, Node, NodeKind, Package, PackageBuilder, StyleSet, Template};
use ui2_runtime::testkit::Kit;
use ui2_script::testing::MockScriptHost;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// A skin of one empty HUD layer with the id `id`, the script API `api` and the scripts `files`.
fn skin(id: &str, api: (u16, u16), files: &[(&str, &str)]) -> Vec<u8> {
    let mut b = PackageBuilder::new();
    let record = b.style_record(StyleRecord::initial());
    let style = b.style_set(StyleSet::plain(vec![record]));
    let name = b.str("doc");
    b.template(Template {
        name,
        nodes: vec![Node {
            kind: NodeKind::Layer(Layer::Hud),
            subtree: 1,
            style,
            part: None,
            flags: 0,
        }],
    });
    for (path, code) in files {
        b.script(path, code.as_bytes());
    }
    let mut manifest = Kit::manifest();
    manifest.id = id.into();
    manifest.api = api;
    manifest.default = id == "scriptfail";
    b.finish(manifest).expect("package")
}

/// The fixture packages: `(id, bytes)`.
fn packages() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("badhash", skin("badhash", (1, 0), &[])),
        ("api9", skin("api9", (9, 0), &[])),
        (
            "scriptfail",
            skin(
                "scriptfail",
                (1, 0),
                &[(
                    "js/main.js",
                    "throw new Error('the fixture skin fails while it loads');",
                )],
            ),
        ),
    ]
}

/// The index of the fixtures: `badhash` names a hash no file has, `api9` says API 9.0, `scriptfail` is the
/// default skin.
fn index(packages: &[(&str, Vec<u8>)]) -> String {
    let entries: Vec<serde_json::Value> = packages
        .iter()
        .map(|(id, bytes)| {
            let pkg = Package::read(Arc::from(bytes.clone())).expect("readable");
            let full = hash_hex(&pkg.hash());
            let hash = if *id == "badhash" {
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned()
            } else {
                full.clone()
            };
            serde_json::json!({
                "id": id,
                "file": format!("{id}-{}.ui2", &full[..16]),
                "hash": hash,
                "title_key": format!("TXT_KEY_SKIN_{}", id.to_uppercase()),
                "themes": ["dark", "light"],
                "api": if *id == "api9" { "9.0" } else { "1.0" },
                "default": *id == "scriptfail",
            })
        })
        .collect();
    serde_json::to_string_pretty(&entries).expect("json") + "\n"
}

/// The packages by skin id.
type Files = Vec<(String, Arc<[u8]>)>;

/// The fixture files as they are on disk: the index and each package by id.
fn on_disk() -> (String, Files) {
    let read = |name: &str| {
        std::fs::read(dir().join(name)).unwrap_or_else(|e| panic!("tests/fixtures/{name}: {e}"))
    };
    let idx = String::from_utf8(read("skins.json")).expect("utf-8");
    let reg = SkinRegistry::parse(&idx).expect("the fixture index");
    let files = reg
        .entries()
        .iter()
        .map(|e| (e.id.clone(), Arc::from(read(&e.file))))
        .collect();
    (idx, files)
}

#[test]
#[ignore = "writes the fixtures"]
fn write_the_fixtures() {
    let packages = packages();
    std::fs::create_dir_all(dir()).expect("dir");
    let idx = index(&packages);
    std::fs::write(dir().join("skins.json"), &idx).expect("index");
    let reg = SkinRegistry::parse(&idx).expect("index");
    for (id, bytes) in &packages {
        let file = &reg.get(id).expect("entry").file;
        std::fs::write(dir().join(file), bytes).expect("package");
    }
}

#[test]
fn the_fixtures_are_current() {
    let packages = packages();
    let (idx, files) = on_disk();
    assert_eq!(idx, index(&packages), "run write_the_fixtures");
    for (id, bytes) in &packages {
        let disk = files.iter().find(|(i, _)| i == id).expect("on disk");
        assert_eq!(&*disk.1, bytes.as_slice(), "{id}: run write_the_fixtures");
    }
}

fn file_of(files: &Files, id: &str) -> Arc<[u8]> {
    Arc::clone(&files.iter().find(|(i, _)| i == id).expect("fixture").1)
}

#[test]
fn a_broken_hash_is_refused_and_falls_back_to_the_default_skin() {
    let (idx, files) = on_disk();
    let mut reg = SkinRegistry::parse(&idx).expect("index");
    let e = reg
        .open("badhash", file_of(&files, "badhash"))
        .expect_err("refused");
    assert!(matches!(e.failure, LoadFailure::HashMismatch { .. }), "{e}");
    assert_eq!(reg.fail("badhash"), Fallback::Skin("scriptfail".into()));
}

#[test]
fn a_skin_for_api_9_is_refused_by_its_entry_and_by_its_package() {
    let (idx, files) = on_disk();
    let reg = SkinRegistry::parse(&idx).expect("index");
    let entry = reg.get("api9").expect("entry");
    let e = entry.check_api().expect_err("api 9");
    assert!(matches!(e.failure, LoadFailure::Incompatible(_)), "{e}");
    assert!(e.to_string().contains("script API 9.0"), "{e}");
    assert!(reg.open("api9", file_of(&files, "api9")).is_err());
    // The package alone (no index: samples, dev reload) is refused too.
    let pkg = Package::read(file_of(&files, "api9")).expect("readable");
    let e = V2Ui::new(Arc::new(pkg), V2Init::default())
        .err()
        .expect("refused");
    assert!(matches!(e.failure, LoadFailure::Incompatible(_)), "{e}");
}

#[test]
fn a_script_that_fails_while_loading_is_fatal_once_then_classic_without_a_loop() {
    let (idx, files) = on_disk();
    let mut reg = SkinRegistry::parse(&idx).expect("index");
    let pkg = reg
        .open("scriptfail", file_of(&files, "scriptfail"))
        .expect("the package itself is good");
    let host = MockScriptHost::new();
    let mut ui = V2Ui::new(
        pkg,
        V2Init {
            scripts: Some(Box::new(host.clone())),
            ..V2Init::default()
        },
    )
    .expect("loads");
    let w = host.live()[0];
    host.reply(
        w,
        r#"{"t":"fail","ctx":"skin","error":{"message":"Error: the fixture skin fails while it loads","file":"js/main.js","line":1}}"#,
    );
    let frame = |ui: &mut V2Ui| {
        ui.frame(V2Frame {
            replica: None,
            steps: &[],
            net: None,
            events: &[],
            mods: None,
            now_ms: 16.0,
        })
    };
    let fatal = frame(&mut ui).fatal.expect("fatal");
    assert_eq!(fatal.skin, "scriptfail");
    assert!(matches!(fatal.failure, LoadFailure::Scripts(_)), "{fatal}");
    assert_eq!(frame(&mut ui).fatal, None, "reported once");
    // The chain of the playtest: badhash, then the default scriptfail, then the classic UI; nothing again.
    assert_eq!(reg.fail("badhash"), Fallback::Skin("scriptfail".into()));
    assert_eq!(reg.fail("scriptfail"), Fallback::Classic);
    for want in [None, Some("badhash"), Some("scriptfail")] {
        assert!(reg.resolve(want).is_none(), "{want:?}");
    }
    // api9 is still in the index and not broken: asking for it gives it (its entry refuses it).
    assert_eq!(
        reg.resolve(Some("api9")).map(|e| e.id.as_str()),
        Some("api9")
    );
}
