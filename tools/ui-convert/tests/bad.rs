//! Bad mockups: each folder under `tests/bad/` is a minimal skin with one mistake, the codes it
//! must produce (`expect.txt`: `<code> <file>:<line>` per line) and its stored scan (`ir.json`).
//! This test runs the checks on the stored scan (no browser); `e2e.rs` scans the folders again
//! in Chromium and checks that the stored scans are current.

use std::path::{Path, PathBuf};

use ui2_convert::{Input, check_skin};

fn bad_dirs() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("bad");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("tests/bad exists")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("expect.txt").is_file())
        .collect();
    dirs.sort();
    dirs
}

/// Whether the mockup's error is found only past the checks (rasterizing, writing): `# stage: build` in
/// `expect.txt`. The stored scan then runs the checks without errors; `e2e.rs` builds it.
fn build_stage(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("expect.txt"))
        .expect("expect.txt")
        .lines()
        .any(|l| l.trim().starts_with("# stage: build"))
}

fn expected(dir: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(dir.join("expect.txt")).expect("expect.txt");
    let mut v: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect();
    v.sort();
    v
}

fn got(run: &ui2_convert::Run) -> Vec<String> {
    let mut v: Vec<String> = run
        .errors
        .iter()
        .map(|e| match e.place.file.as_str() {
            "" => format!("{} (skin)", e.code),
            f => format!("{} {f}:{}", e.code, e.place.line),
        })
        .collect();
    v.sort();
    v
}

#[test]
fn bad_mockups_give_the_expected_codes() {
    let dirs = bad_dirs();
    assert_eq!(dirs.len(), 41, "the whole set of bad mockups is present");
    for dir in dirs {
        let ir = std::fs::read_to_string(dir.join("ir.json")).expect("ir.json");
        let run = check_skin(&dir, Input::Dump(&ir));
        let want = if build_stage(&dir) {
            Vec::new()
        } else {
            expected(&dir)
        };
        assert_eq!(
            got(&run),
            want,
            "{}:\n{}",
            dir.display(),
            run.errors
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

#[test]
fn error_texts_carry_no_machine_paths() {
    for dir in bad_dirs() {
        let ir = std::fs::read_to_string(dir.join("ir.json")).expect("ir.json");
        let run = check_skin(&dir, Input::Dump(&ir));
        let all = ui2_convert::errors::to_json(&run.errors);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .to_string_lossy()
            .replace('\\', "/");
        assert!(!all.replace('\\', "/").contains(&root), "{all}");
        assert!(!all.contains(":\\"), "{all}");
    }
}
