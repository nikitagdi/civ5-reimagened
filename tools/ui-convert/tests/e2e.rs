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

fn engraved() -> PathBuf {
    crate_dir()
        .join("..")
        .join("..")
        .join("skins")
        .join("engraved")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for e in std::fs::read_dir(from).expect("read_dir").flatten() {
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()));
        } else {
            std::fs::copy(&p, to.join(e.file_name())).expect("copy");
        }
    }
}

/// Two builds of the skin, and a build of a copy in another folder, give the same bytes.
#[test]
fn engraved_builds_and_the_package_is_deterministic() {
    if !chrome() {
        return;
    }
    let a = ui2_convert::build::build(&engraved(), true);
    assert!(a.run.errors.is_empty(), "{:?}", a.run.errors);
    let a = a.written.expect("package");
    let b = ui2_convert::build::build(&engraved(), true)
        .written
        .expect("package");
    assert_eq!(a.hash, b.hash);
    assert_eq!(a.bytes, b.bytes);
    let copy = std::env::temp_dir().join(format!("ui2-e2e-copy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&copy);
    copy_dir(&engraved(), &copy.join("engraved"));
    let c = ui2_convert::build::build(&copy.join("engraved"), true);
    let _ = std::fs::remove_dir_all(&copy);
    assert!(c.run.errors.is_empty(), "{:?}", c.run.errors);
    assert_eq!(c.written.expect("package").bytes, a.bytes);
    assert_eq!(a.file_name, format!("engraved-{}.ui2", &a.hash[..16]));
    // The package reads back.
    let pkg = ui2_package::Package::read(std::sync::Arc::from(a.bytes.as_slice())).expect("read");
    assert_eq!(pkg.fonts().len(), 3);
    pictures_are_atlas_pages(&pkg, &a.bytes);
}

/// The icons are mask sprites at scale 1 and 2 on A8 pages; no PNG, WebP or SVG file is inside.
fn pictures_are_atlas_pages(pkg: &ui2_package::Package, bytes: &[u8]) {
    use ui2_package::PageFormat;
    for magic in [&b"\x89PNG"[..], b"RIFF", b"<svg", b"<?xml"] {
        assert!(
            !bytes.windows(magic.len()).any(|w| w == magic),
            "{:?} in the package",
            String::from_utf8_lossy(magic)
        );
    }
    let images = pkg.images();
    assert!(images.pages.iter().all(|p| p.format == PageFormat::A8));
    assert_eq!(
        images.pages.iter().map(|p| p.scale).collect::<Vec<_>>(),
        [1, 2]
    );
    // The science flask: 16 x 16 at scale 1 (the research ring and the science yield).
    let sci = images
        .imgs
        .iter()
        .find(|i| {
            pkg.str(i.path)
                .is_some_and(|p| p.starts_with("icons/i-sci@16x16"))
        })
        .expect("science icon");
    assert!(sci.mask);
    assert_eq!(sci.x1.rect[2..], [18, 18]);
    assert_eq!(sci.x2.expect("2x").rect[2..], [34, 34]);
    let page = &images.pages[usize::from(sci.x1.page)];
    let px = pkg.blob_owned(page.blob).expect("page");
    let w = usize::from(page.size[0]);
    let [x, y, sw, sh] = sci.x1.rect.map(usize::from);
    let mut art = String::new();
    let mut covered = 0;
    for row in y..y + sh {
        for col in x..x + sw {
            let a = px[row * w + col];
            let edge = row == y || col == x || row == y + sh - 1 || col == x + sw - 1;
            assert!(!edge || a == 0, "the margin is transparent");
            covered += usize::from(a > 0);
            art.push(match a {
                0 => '.',
                1..=127 => '+',
                _ => '#',
            });
        }
        art.push('\n');
    }
    println!("{art}");
    assert!(
        covered > 30 && covered < 16 * 16,
        "{covered} pixels covered"
    );
}
