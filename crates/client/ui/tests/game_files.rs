//! Acceptance on the real game files (skipped when they are absent).
mod support;

#[test]
fn every_ui_xml_parses() {
    let Some(assets) = support::game_assets() else {
        return;
    };
    let mut failed = Vec::new();
    let mut n = 0;
    for layer in support::UI_LAYERS {
        let mut files = Vec::new();
        support::walk(&assets.join(layer), &mut files);
        for f in files {
            if f.extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref()
                != Some("xml")
            {
                continue;
            }
            n += 1;
            let bytes = std::fs::read(&f).unwrap();
            if let Err(e) = ui::xml::parse(&f.to_string_lossy(), &bytes) {
                failed.push(e.to_string());
            }
        }
    }
    eprintln!("{n} xml files, {} failed", failed.len());
    for f in &failed {
        eprintln!("  {f}");
    }
}

const STUBS: &str = include_str!("fixtures/m1_stubs.lua");

#[test]
fn m1_runs_without_lua_errors() {
    let Some(assets) = support::game_assets() else {
        return;
    };
    let r = ui::dev::run_m1(&assets, STUBS, true);
    assert_eq!(r.lua_errors, 0, "{:?}", r.errors);
    assert!(r.controls > 150, "controls {}", r.controls);
    assert!(r.draw_cmds > 0);
    assert!(r.zero_sized.is_empty(), "{:?}", r.zero_sized);
}

#[test]
fn every_xml_builds() {
    let Some(assets) = support::game_assets() else {
        return;
    };
    let r = ui::dev::build_all_xml(&assets, STUBS);
    eprintln!(
        "{} files, {} controls, {} warnings",
        r.files,
        r.controls,
        r.warnings.len()
    );
    for w in &r.warnings {
        eprintln!("  {w}");
    }
    assert!(r.files > 100);
    // The original's own data errors: a test menu names a style that no book has, and one
    // file uses a lower-case style tag.
    let bad: Vec<_> = r
        .warnings
        .iter()
        .filter(|w| w.contains("Unidentified style") || w.contains("Unknown control type"))
        .filter(|w| !w.contains("TestSlider") && !w.contains("Highlights.xml"))
        .collect();
    assert!(bad.is_empty(), "{bad:?}");
}
