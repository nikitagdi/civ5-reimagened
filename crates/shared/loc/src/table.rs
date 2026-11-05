//! The text dictionary: `Language_<type>(Tag, Text, Gender, Plurality)` as a trait, and a map implementation.

use std::collections::HashMap;

/// One row of the text table. `gender` and `plurality` are the raw column strings
/// (`neuter:an`, `masculine|feminine`, `1|2`); absent columns mean the defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextEntry<'a> {
    pub text: &'a str,
    pub gender: Option<&'a str>,
    pub plurality: Option<&'a str>,
}

/// `StringDictionary` of one language. Implementations must be cheap to query: the engine calls
/// `get` for every nested text key.
pub trait TextTable: Send + Sync {
    fn get(&self, tag: &str) -> Option<TextEntry<'_>>;
}

/// An owned row of [`MapTable`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OwnedEntry {
    pub text: String,
    pub gender: Option<String>,
    pub plurality: Option<String>,
}

/// A hash-map text table: used by tests, tools and the C ABI.
#[derive(Debug, Clone, Default)]
pub struct MapTable {
    rows: HashMap<String, OwnedEntry>,
}

impl MapTable {
    pub fn new() -> MapTable {
        MapTable::default()
    }

    /// Adds or replaces a row (later rows win, as `Replace` rows do in the game data).
    pub fn insert(&mut self, tag: &str, text: &str, gender: Option<&str>, plurality: Option<&str>) {
        self.rows.insert(
            tag.to_string(),
            OwnedEntry {
                text: text.to_string(),
                gender: gender.map(str::to_string),
                plurality: plurality.map(str::to_string),
            },
        );
    }

    /// A row with only text.
    pub fn insert_text(&mut self, tag: &str, text: &str) {
        self.insert(tag, text, None, None);
    }

    pub fn remove(&mut self, tag: &str) {
        self.rows.remove(tag);
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &OwnedEntry)> {
        self.rows.iter().map(|(k, v)| (k.as_str(), v))
    }
}

impl TextTable for MapTable {
    fn get(&self, tag: &str) -> Option<TextEntry<'_>> {
        self.rows.get(tag).map(|e| TextEntry {
            text: &e.text,
            gender: e.gender.as_deref(),
            plurality: e.plurality.as_deref(),
        })
    }
}

/// `Languages(Type, DisplayName, PluralRule)`; the language is a value, not a constant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageInfo {
    pub type_: String,
    pub display_name: String,
    /// `PluralRule` column, 1..=16 (`Localization::GetPluralityRule`).
    pub plural_rule: u32,
}

impl LanguageInfo {
    pub fn new(type_: &str, display_name: &str, plural_rule: u32) -> LanguageInfo {
        LanguageInfo {
            type_: type_.to_string(),
            display_name: display_name.to_string(),
            plural_rule,
        }
    }

    /// en_US as the installed `Languages` table has it (`PluralRule` 2).
    pub fn en_us() -> LanguageInfo {
        LanguageInfo::new("en_US", "English", 2)
    }
}
