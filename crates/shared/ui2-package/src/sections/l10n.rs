//! `L10N`: the skin's own texts by language: key to a `loc` template.

use serde::{Deserialize, Serialize};

use crate::container::SectionTag;
use crate::error::PackageError;
use crate::ids::StrId;
use crate::sections::manifest::Manifest;

/// The texts of one language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lang {
    /// The language tag (`en_US`).
    pub tag: String,
    /// Key (a string of `STRS`) to the template in the syntax of the game's text engine; strictly
    /// ascending by key.
    pub entries: Vec<(StrId, String)>,
}

/// The section: one entry per language of the manifest, in the manifest's order (the first is the
/// fallback).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct L10n {
    /// The languages.
    pub langs: Vec<Lang>,
}

impl L10n {
    /// The template of a key in a language, `None` when the language or the key is missing.
    pub fn get(&self, lang: &str, key: StrId) -> Option<&str> {
        let l = self.langs.iter().find(|l| l.tag == lang)?;
        let at = l.entries.binary_search_by_key(&key, |(k, _)| *k).ok()?;
        Some(l.entries[at].1.as_str())
    }

    pub(crate) fn validate(
        &self,
        at: SectionTag,
        strs: usize,
        manifest: &Manifest,
    ) -> Result<(), PackageError> {
        let tags: Vec<&String> = self.langs.iter().map(|l| &l.tag).collect();
        if tags.len() != manifest.languages.len()
            || tags.iter().zip(&manifest.languages).any(|(a, b)| *a != b)
        {
            return Err(PackageError::invalid(
                at,
                "the languages differ from the manifest's",
            ));
        }
        for lang in &self.langs {
            let ordered = lang.entries.iter().all(|(k, _)| k.index() < strs)
                && lang.entries.windows(2).all(|w| w[0].0 < w[1].0);
            if !ordered {
                return Err(PackageError::invalid(
                    at,
                    format!(
                        "{}: keys are not ascending names of the string table",
                        lang.tag
                    ),
                ));
            }
        }
        Ok(())
    }
}
