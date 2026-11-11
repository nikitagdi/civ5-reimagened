//! The top strip end to end: `skins/engraved` built to a package, the package run in `ui2-runtime` with the
//! text engine on its own font subsets and the `hud` sample, and the runtime's rectangle dump compared with
//! Chrome's (`node/rects.mjs`) at the reference viewports: the same records, every edge within 0.5 px, text
//! leaves included. The work is `ui2_convert::verify` (the `cargo xtask ui verify` command); this test runs it,
//! and also runs the comparison on the stored fixture, which needs no browser.
//!
//! The chrome test needs `CIV5_UI2_CHROME=1`, Node with the Playwright install and Python with fontTools; it
//! skips otherwise. With `CIV5_UI2_WRITE_FIXTURE=1` it also writes `tests/fixtures/top-strip/{top.ui2,
//! chrome-rects.json}`.

use std::path::{Path, PathBuf};

use serde_json::Value;
use ui2_convert::verify::{self, TOLERANCE};

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

#[test]
fn layout_top_strip_package_matches_chrome() {
    if !chrome() {
        return;
    }
    let skin = crate_dir().join("../../skins/engraved");
    let o = match verify::verify(&skin, "hud") {
        Ok(o) => o,
        Err(e) => panic!("{e:?}"),
    };
    for l in o.report.lines() {
        println!("{l}");
    }
    assert!(o.report.ok(), "the layout differs from Chrome");

    if std::env::var("CIV5_UI2_WRITE_FIXTURE").ok().as_deref() == Some("1") {
        let dir = crate_dir().join("tests/fixtures/top-strip");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("top.ui2"), &o.package).expect("top.ui2");
        let dumps = &o.dumps;
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

/// The stored package and Chrome's stored dumps: no browser needed.
#[test]
fn layout_top_strip_fixture_matches_stored_chrome_dumps() {
    let dir = crate_dir().join("tests/fixtures/top-strip");
    let package = std::fs::read(dir.join("top.ui2")).expect("top.ui2");
    let dumps: Vec<Value> = serde_json::from_str(
        &std::fs::read_to_string(dir.join("chrome-rects.json")).expect("dumps"),
    )
    .expect("json");
    let sample = std::fs::read_to_string(crate_dir().join("../../skins/engraved/sample/hud.json"))
        .expect("sample");
    let report = verify::compare(&package, &sample, "engraved", "hud", &dumps).expect("compare");
    for l in report.lines() {
        println!("{l}");
    }
    assert_eq!(report.viewports.len(), verify::VIEWPORTS.len());
    for v in &report.viewports {
        assert!(v.mismatch.is_none(), "{:?}", v.mismatch);
        assert!(!v.rows.is_empty());
        assert!(
            v.rows.iter().any(|r| !r.part.is_empty()),
            "records carry their data-part"
        );
    }
    assert!(report.ok(), "tolerance {TOLERANCE} px");
}
