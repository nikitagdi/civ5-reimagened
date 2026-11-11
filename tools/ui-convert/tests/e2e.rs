//! Tests with Node and Chromium. They run only with `CIV5_UI2_CHROME=1` (and Node plus the
//! Playwright install of `tools/playtest`); otherwise each prints why it skipped.
//!
//! `$env:CIV5_UI2_CHROME=1; cargo test -p ui2-convert --test e2e -- --test-threads=1`

use std::path::{Path, PathBuf};

use ui2_convert::{Input, check_skin, node};

fn chrome() -> bool {
    if std::env::var("CIV5_UI2_CHROME").ok().as_deref() != Some("1") {
        eprintln!("skipped: CIV5_UI2_CHROME is not set");
        return false;
    }
    match node::available() {
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

#[test]
fn bad_mockup_scans_are_current() {
    if !chrome() {
        return;
    }
    let root = crate_dir().join("tests").join("bad");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("tests/bad")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("expect.txt").is_file())
        .collect();
    dirs.sort();
    for dir in dirs {
        let run = check_skin(&dir, Input::Chrome);
        let fresh = run.dump().expect("the scan ran");
        let stored = std::fs::read_to_string(dir.join("ir.json")).expect("ir.json");
        assert_eq!(
            fresh,
            stored,
            "{}: the stored ir.json is stale; regenerate it with `ui2-convert check <dir> --dump-ir <dir>/ir.json`",
            dir.display()
        );
    }
}

#[test]
fn layout_fixture_is_current() {
    if !chrome() {
        return;
    }
    let fixtures = crate_dir().join("tests").join("fixtures");
    let run = check_skin(&fixtures.join("layout"), Input::Chrome);
    assert!(run.errors.is_empty(), "{:?}", run.errors);
    let stored = std::fs::read_to_string(fixtures.join("layout.ir.json")).expect("fixture");
    assert_eq!(
        run.dump().expect("dump"),
        stored,
        "layout.ir.json is stale; see tests/fixtures/README.md"
    );
}

#[test]
fn engraved_checks_run_in_chrome() {
    if !chrome() {
        return;
    }
    let dir = crate_dir()
        .join("..")
        .join("..")
        .join("skins")
        .join("engraved");
    let run = check_skin(&dir, Input::Chrome);
    assert!(run.scanned.is_some());
    // The checks ran; the codes depend on the state of the skin and the profile.
    for e in &run.errors {
        assert!(!e.code.starts_with("E9-00") || e.code == "E9-005", "{e}");
    }
}
