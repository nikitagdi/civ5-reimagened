//! The texts of the real game (`Localization-Merged.db`, the DB snapshot the game data is pinned to) against the texts the
//! UI set builds from the XML files.
//!
//! The XML files are an approximation of what the game merges: the game loads the text files of the base game, the DLCs and
//! Lekmod in an order of its own (a Lekmod `Override` file's `<Replace>` beats the No Quitters Mod's `<Row>` of the same
//! tag), which a layer order cannot copy for every tag. The snapshot holds the result, the same file the gameplay side's
//! `data:loc-en_us` is cut from, so the UI takes its texts from it too.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::gamedata::GameDataError;
use crate::gamedata::db::{Db, quote};

use super::text::{TextRow, TextSet};

/// One language of the snapshot: tag, text, `Gender`, `Plurality` (the text of a NULL row is empty).
pub fn read_language(loc_db: &Path, lang: &str) -> Result<Vec<TextRow>, GameDataError> {
    let db = Db::open_read_only(loc_db)?;
    let rows = db.rows(
        &format!(
            "SELECT Tag, Text, Gender, Plurality FROM LocalizedText \
             WHERE Language = {} AND Tag IS NOT NULL ORDER BY Tag",
            quote(lang)
        ),
        4,
    )?;
    Ok(rows
        .into_iter()
        .filter_map(|mut r| {
            let plurality = r.pop()?.unwrap_or_default();
            let gender = r.pop()?.unwrap_or_default();
            let text = r.pop()?.unwrap_or_default();
            let tag = r.pop()??;
            Some(TextRow {
                lang: lang.to_owned(),
                tag,
                text,
                gender: gender.trim().to_owned(),
                plurality: plurality.trim().to_owned(),
            })
        })
        .collect())
}

/// How a built language differs from the snapshot's.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LocDiff {
    pub lang: String,
    /// Rows of the snapshot.
    pub oracle_rows: usize,
    /// Rows the build has.
    pub our_rows: usize,
    /// Tag, our text, the snapshot's text.
    pub differing: Vec<(String, String, String)>,
    /// Tags the snapshot has and the build lacks, with the snapshot's text.
    pub missing: Vec<(String, String)>,
    /// Tags the build has and the snapshot lacks, with our text.
    pub extra: Vec<(String, String)>,
    /// Tags whose `Gender` / `Plurality` differ.
    pub grammar: Vec<String>,
}

impl LocDiff {
    pub fn total(&self) -> usize {
        self.differing.len() + self.missing.len() + self.extra.len() + self.grammar.len()
    }
}

/// Compares the language `lang` of `texts` with the snapshot rows.
pub fn diff(texts: &TextSet, oracle: &[TextRow], lang: &str) -> LocDiff {
    let empty = BTreeMap::new();
    let ours = texts.table(lang).unwrap_or(&empty);
    let grammar = texts.grammar_table(lang);
    let mut d = LocDiff {
        lang: lang.to_owned(),
        oracle_rows: oracle.len(),
        our_rows: ours.len(),
        ..LocDiff::default()
    };
    let mut seen = BTreeSet::new();
    for r in oracle {
        seen.insert(r.tag.as_str());
        match ours.get(&r.tag) {
            None => d.missing.push((r.tag.clone(), r.text.clone())),
            Some(t) if t != &r.text => {
                d.differing.push((r.tag.clone(), t.clone(), r.text.clone()));
            }
            Some(_) => {}
        }
        let ours_g = grammar
            .and_then(|g| g.get(&r.tag))
            .cloned()
            .unwrap_or_default();
        if ours.contains_key(&r.tag) && ours_g != (r.gender.clone(), r.plurality.clone()) {
            d.grammar.push(r.tag.clone());
        }
    }
    for (tag, t) in ours {
        if !seen.contains(tag.as_str()) {
            d.extra.push((tag.clone(), t.clone()));
        }
    }
    d
}

/// Replaces the language `lang` of `texts` with the snapshot's rows, text and grammar. Tags only the build has are kept
/// when `keep_extra`, else dropped.
pub fn apply(texts: &mut TextSet, oracle: Vec<TextRow>, lang: &str, keep_extra: bool) {
    let key = lang.to_ascii_lowercase();
    let old_text = texts.langs.remove(&key).map(|(_, m)| m).unwrap_or_default();
    let old_grammar = texts.grammar.remove(&key).unwrap_or_default();
    let mut map = BTreeMap::new();
    let mut grammar = BTreeMap::new();
    if keep_extra {
        let oracle_tags: BTreeSet<&str> = oracle.iter().map(|r| r.tag.as_str()).collect();
        for (tag, t) in old_text {
            if !oracle_tags.contains(tag.as_str()) {
                if let Some(g) = old_grammar.get(&tag) {
                    grammar.insert(tag.clone(), g.clone());
                }
                map.insert(tag, t);
            }
        }
    }
    for r in oracle {
        if !(r.gender.is_empty() && r.plurality.is_empty()) {
            grammar.insert(r.tag.clone(), (r.gender, r.plurality));
        }
        map.insert(r.tag, r.text);
    }
    texts.langs.insert(key.clone(), (lang.to_owned(), map));
    texts.grammar.insert(key, grammar);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(lang: &str, tag: &str, text: &str) -> TextRow {
        TextRow {
            lang: lang.into(),
            tag: tag.into(),
            text: text.into(),
            gender: String::new(),
            plurality: String::new(),
        }
    }

    /// The case of the Scientific Revolution: the file that loads later in the layer order wins over the one the game ends
    /// with. The snapshot's text replaces it, and the diff counts the row before and not after.
    #[test]
    fn the_snapshot_text_replaces_a_wrong_winner_and_the_diff_goes_to_zero() {
        const RIGHT: &str = "Grants a free [ICON_GREAT_SCIENTIST] Great Scientist.";
        let mut t = TextSet::default();
        t.add("mongol.xml", [row("en_US", "TXT_POLICY", RIGHT)]);
        t.add(
            "no-quitters.xml",
            [
                row("en_US", "TXT_POLICY", "Grants a free Technology."),
                row("en_US", "TXT_ONLY_OURS", "ours"),
                row("en_US", "TXT_SAME", "same"),
            ],
        );
        let mut oracle = vec![
            row("en_US", "TXT_POLICY", RIGHT),
            row("en_US", "TXT_SAME", "same"),
            row("en_US", "TXT_ONLY_ORACLE", "oracle"),
        ];
        oracle[1].gender = "M".into();
        let before = diff(&t, &oracle, "en_US");
        assert_eq!(before.differing.len(), 1);
        assert_eq!(
            before.missing,
            [("TXT_ONLY_ORACLE".to_owned(), "oracle".to_owned())]
        );
        assert_eq!(before.extra.len(), 1);
        assert_eq!(before.grammar, ["TXT_SAME".to_owned()]);
        apply(&mut t, oracle.clone(), "en_US", false);
        assert_eq!(diff(&t, &oracle, "en_US").total(), 0);
        assert_eq!(t.table("en_US").unwrap()["TXT_POLICY"], RIGHT);
        // Another language is not touched; the build's own tags stay when asked.
        t.add("ru.xml", [row("ru_RU", "TXT_POLICY", "x")]);
        t.add("own.xml", [row("en_US", "TXT_OWN", "own")]);
        apply(&mut t, oracle, "en_US", true);
        assert_eq!(t.table("ru_RU").unwrap()["TXT_POLICY"], "x");
        assert_eq!(t.table("en_US").unwrap()["TXT_OWN"], "own");
    }

    /// The real snapshot, when `CIV5_DB_SNAPSHOT` names it: the policy of the report reads as the real game's.
    #[test]
    fn the_snapshot_has_the_great_scientist_text() {
        let Some(dir) = std::env::var_os("CIV5_DB_SNAPSHOT") else {
            return;
        };
        let rows = read_language(
            &std::path::PathBuf::from(dir).join(crate::gamedata::LOC_DB_FILE),
            "en_US",
        )
        .unwrap();
        let r = rows
            .iter()
            .find(|r| r.tag == "TXT_KEY_POLICY_SCIENTIFIC_REVOLUTION_HELP")
            .unwrap();
        assert!(
            r.text.contains("[ICON_GREAT_SCIENTIST] Great Scientist"),
            "{}",
            r.text
        );
    }
}
