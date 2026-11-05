//! The UI's texts against the real game's `Localization-Merged.db` (the report of 2026-10-31: Scientific Revolution read
//! "Grants a free Technology." where Lekmod says "Grants a free [ICON_GREAT_SCIENTIST] Great Scientist.").
//!
//! Runs only when `CIV5_ROOT` (the game) and `CIV5_DB_SNAPSHOT` (the snapshot folder) are set.

use std::path::{Path, PathBuf};

use asset_pipeline::gamedata::LOC_DB_FILE;
use asset_pipeline::ui::{UiOptions, oracle, xml_texts};

const TAG: &str = "TXT_KEY_POLICY_SCIENTIFIC_REVOLUTION_HELP";

fn real() -> Option<(PathBuf, PathBuf)> {
    let root = std::env::var_os("CIV5_ROOT")?;
    let snap = std::env::var_os("CIV5_DB_SNAPSHOT")?;
    Some((PathBuf::from(root), Path::new(&snap).join(LOC_DB_FILE)))
}

/// The XML merge alone is wrong for this policy and for hundreds of other texts (the cause: a layer order is not the
/// game's loader order); with the snapshot applied the build equals the snapshot, in every language the UI ships.
#[test]
fn the_ui_texts_equal_the_real_games_after_the_snapshot_is_applied() {
    let Some((root, loc_db)) = real() else {
        eprintln!("CIV5_ROOT / CIV5_DB_SNAPSHOT not set: skipped");
        return;
    };
    let opts = UiOptions::default();
    let mut texts = xml_texts(&root, &opts).unwrap();
    let en = oracle::read_language(&loc_db, "en_US").unwrap();
    let before = oracle::diff(&texts, &en, "en_US");
    assert!(
        before
            .differing
            .iter()
            .any(|(t, ours, _)| t == TAG && ours.ends_with("a free Technology.")),
        "the XML merge is expected to have the wrong text (if Lekmod's files changed, this guard can go)"
    );
    for lang in opts.langs.clone().unwrap() {
        let rows = oracle::read_language(&loc_db, &lang).unwrap();
        oracle::apply(&mut texts, rows.clone(), &lang, false);
        let d = oracle::diff(&texts, &rows, &lang);
        assert_eq!(d.total(), 0, "{lang}: {d:?}");
    }
    assert_eq!(
        texts.table("en_US").unwrap()[TAG],
        "[COLOR_POSITIVE_TEXT]Scientific Revolution[ENDCOLOR][NEWLINE]Grants a free [ICON_GREAT_SCIENTIST] Great Scientist."
    );
}
