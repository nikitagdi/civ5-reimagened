//! Texts: the skin's tables (`l10n/<lang>.toml`), the keys the markup uses, and the characters
//! the font subsets must keep.
//!
//! A table is TOML: `"key" = "template"` (a nested table flattens to dotted keys). Templates use
//! the game's syntax (`{1}`, `{1_Num}`, `{1: plural 1?turn; other?turns;}`) and are parsed by the
//! `loc` crate. Every used key must exist in every language (E6-002); a template may not use more
//! arguments than the element passes (E6-004).
//!
//! The glyph set is the named ranges of `[fonts] subset` plus every character of every table,
//! of the literal parts of bound templates, of the static text the markup keeps, and of the
//! strings in the samples. It is a superset (template syntax characters are in it too), never a
//! subset.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::bindings::Bound;
use crate::errors::{ConvError, Place, codes};
use crate::skin::Skin;

/// One language table.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lang {
    /// The tag (`en_US`), the file name without `.toml`.
    pub tag: String,
    /// Key -> template.
    pub entries: BTreeMap<String, String>,
}

/// The texts of a skin.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Texts {
    /// The languages, by tag.
    pub langs: Vec<Lang>,
    /// The characters the font subsets keep.
    pub glyphs: Glyphs,
}

/// The characters of the font subsets.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Glyphs {
    /// The named ranges of `[fonts] subset`, inclusive.
    pub ranges: Vec<(u32, u32)>,
    /// Characters outside the ranges that the skin shows.
    pub extra: BTreeSet<char>,
}

impl Glyphs {
    fn add_text(&mut self, s: &str) {
        for c in s.chars() {
            if c.is_control() {
                continue;
            }
            let cp = c as u32;
            if !self.ranges.iter().any(|&(a, b)| a <= cp && cp <= b) {
                self.extra.insert(c);
            }
        }
    }

    /// Every code point, merged into ascending inclusive ranges.
    pub fn codepoints(&self) -> Vec<(u32, u32)> {
        let mut all: Vec<(u32, u32)> = self.ranges.clone();
        all.extend(self.extra.iter().map(|&c| (c as u32, c as u32)));
        all.sort();
        let mut out: Vec<(u32, u32)> = Vec::new();
        for (a, b) in all {
            match out.last_mut() {
                Some(last) if a <= last.1.saturating_add(1) => last.1 = last.1.max(b),
                _ => out.push((a, b)),
            }
        }
        out
    }

    /// Whether a character is in the set.
    pub fn contains(&self, c: char) -> bool {
        let cp = c as u32;
        self.extra.contains(&c) || self.ranges.iter().any(|&(a, b)| a <= cp && cp <= b)
    }
}

fn flatten(
    prefix: &str,
    t: &toml::Table,
    out: &mut BTreeMap<String, String>,
    bad: &mut Vec<String>,
) {
    for (k, v) in t {
        let key = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match v {
            toml::Value::String(s) => {
                out.insert(key, s.clone());
            }
            toml::Value::Table(inner) => flatten(&key, inner, out, bad),
            other => bad.push(format!("`{key}` is a {}, not a text", other.type_str())),
        }
    }
}

/// The highest argument number a template uses (0 for none).
fn max_arg(params: &[loc::Param]) -> i32 {
    params
        .iter()
        .map(|p| p.arg_index.max(max_arg(&p.injected)))
        .max()
        .unwrap_or(0)
}

/// Reads the tables, checks the used keys and builds the glyph set.
pub fn check(
    skin_dir: &Path,
    skin: &Skin,
    bound: &Bound,
    samples: &[String],
    static_text: &[String],
) -> (Texts, Vec<ConvError>) {
    let mut errs = Vec::new();
    let mut texts = Texts::default();
    let dir = skin_dir.join("l10n");
    // A mod fragment has no text tables (mod API 1.3): its text is written in the markup or bound.
    let fragment = skin.fragment.is_some();
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.ends_with(".toml") && !fragment)
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    let mut args_of: BTreeMap<(String, String), i32> = BTreeMap::new();
    for f in &files {
        let rel = format!("l10n/{f}");
        let tag = f.trim_end_matches(".toml").to_string();
        let raw = match std::fs::read_to_string(dir.join(f)) {
            Ok(t) => t.replace("\r\n", "\n"),
            Err(e) => {
                errs.push(ConvError::new(
                    codes::L10N_FILE,
                    Place::file(&rel),
                    e.to_string(),
                ));
                continue;
            }
        };
        let table: toml::Table = match toml::from_str(&raw) {
            Ok(t) => t,
            Err(e) => {
                errs.push(ConvError::new(
                    codes::L10N_FILE,
                    Place::file(&rel),
                    e.message().to_string(),
                ));
                continue;
            }
        };
        let mut entries = BTreeMap::new();
        let mut bad = Vec::new();
        flatten("", &table, &mut entries, &mut bad);
        for b in bad {
            errs.push(ConvError::new(codes::L10N_FILE, Place::file(&rel), b));
        }
        for (k, v) in &entries {
            match loc::parse::parse(v) {
                Ok(params) => {
                    args_of.insert((tag.clone(), k.clone()), max_arg(&params));
                }
                Err(e) => errs.push(
                    ConvError::new(codes::L10N_FILE, Place::file(&rel), format!("`{k}`: {e}"))
                        .prop(k.clone()),
                ),
            }
        }
        texts.langs.push(Lang { tag, entries });
    }

    // Every used key in every language, with enough arguments.
    let mut reported = BTreeSet::new();
    for u in &bound.keys {
        if texts.langs.is_empty() {
            let why = if fragment {
                format!(
                    "`{}`: a mod panel has no text tables; write the text in the markup or bind it",
                    u.key
                )
            } else {
                format!("`{}`: the skin has no l10n/<lang>.toml", u.key)
            };
            errs.push(
                ConvError::new(codes::L10N_MISSING, u.place.clone(), why)
                    .element(u.element.clone())
                    .prop(u.key.clone()),
            );
            continue;
        }
        for lang in &texts.langs {
            if !lang.entries.contains_key(&u.key) {
                if reported.insert((u.key.clone(), lang.tag.clone(), u.place.clone())) {
                    errs.push(
                        ConvError::new(
                            codes::L10N_MISSING,
                            u.place.clone(),
                            format!("`{}` is not in l10n/{}.toml", u.key, lang.tag),
                        )
                        .element(u.element.clone())
                        .prop(u.key.clone()),
                    );
                }
                continue;
            }
            let need = args_of
                .get(&(lang.tag.clone(), u.key.clone()))
                .copied()
                .unwrap_or(0);
            if let Some(n) = u.args
                && need > 0
                && need as usize > n
            {
                errs.push(
                    ConvError::new(
                        codes::L10N_ARGS,
                        u.place.clone(),
                        format!(
                            "`{}` in {} uses {{{need}}}, the element passes {n} argument(s)",
                            u.key, lang.tag
                        ),
                    )
                    .element(u.element.clone())
                    .prop(u.key.clone()),
                );
            }
        }
    }

    // The glyph set.
    let mut g = Glyphs::default();
    for name in &skin.fonts.subset {
        if let Some((_, ranges)) = crate::SUBSETS.iter().find(|(n, _)| n == name) {
            g.ranges.extend_from_slice(ranges);
        }
    }
    g.ranges.sort();
    for lang in &texts.langs {
        for v in lang.entries.values() {
            g.add_text(v);
        }
    }
    for l in &bound.literals {
        g.add_text(l);
    }
    for t in static_text {
        g.add_text(t);
    }
    for s in samples {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
            sample_strings(&v, &mut g);
        }
    }
    texts.glyphs = g;
    (texts, errs)
}

fn sample_strings(v: &serde_json::Value, g: &mut Glyphs) {
    match v {
        serde_json::Value::String(s) => g.add_text(s),
        serde_json::Value::Array(a) => a.iter().for_each(|x| sample_strings(x, g)),
        serde_json::Value::Object(m) => m.values().for_each(|x| sample_strings(x, g)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codepoints_merge_ranges_and_extras() {
        let mut g = Glyphs {
            ranges: vec![(0x20, 0x7e), (0x400, 0x45f)],
            ..Glyphs::default()
        };
        g.add_text("Ab\u{7f}\u{a0}\u{2212}\u{401}");
        assert_eq!(
            g.extra.iter().copied().collect::<Vec<_>>(),
            ['\u{a0}', '\u{2212}']
        );
        assert_eq!(
            g.codepoints(),
            [(0x20, 0x7e), (0xa0, 0xa0), (0x400, 0x45f), (0x2212, 0x2212)]
        );
        assert!(g.contains('A') && g.contains('\u{2212}') && !g.contains('\u{3b1}'));
    }

    #[test]
    fn template_arguments_are_counted() {
        let p = loc::parse::parse("{1_Num} {1_Num: plural 1?turn; other?turns;}").unwrap();
        assert_eq!(max_arg(&p), 1);
        assert_eq!(max_arg(&loc::parse::parse("{2} of {1}").unwrap()), 2);
        assert_eq!(max_arg(&loc::parse::parse("Engraved").unwrap()), 0);
    }
}
