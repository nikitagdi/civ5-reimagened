//! Firaxis localization engine (`CvLocalization`) for the original UI and for the game DLL.
//!
//! One implementation of the text grammar serves Lua (`Locale.*`) and, through the C ABI in `capi`, the
//! Lekmod fork. Rules come from the Civ5XP reverse; every rule carries the address of the exe function.
//!
//! ```
//! use loc::{Arg, LanguageInfo, Localizer, MapTable};
//!
//! let mut table = MapTable::new();
//! table.insert_text("TXT_KEY_UNITS", "{1_Num} {1_Num: plural 1?unit; other?units;}");
//! let loc = Localizer::new(LanguageInfo::en_us(), Box::new(table));
//! assert_eq!(loc.convert("TXT_KEY_UNITS", &[Arg::Double(3.0)]), "3 units");
//! ```
//!
//! Markup (`[ICON_*]`, `[COLOR_*]`, `[NEWLINE]`) is left in the result; the text layout parses it.

mod compare;
mod eval;
pub mod number;
pub mod parse;
pub mod plural;
pub mod strings;
mod table;
mod text;

#[cfg(feature = "capi")]
pub mod capi;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub use compare::compare;
pub use eval::{Arg, EvalError};
pub use number::{DecimalFormat, PatternError, Symbols};
pub use parse::Param;
pub use table::{LanguageInfo, MapTable, OwnedEntry, TextEntry, TextTable};
pub use text::{Form, Gender, GenderSpec, LocString};

use eval::Value;

/// Parsed templates kept per `Localizer`; the cache is emptied when it grows beyond this.
const CACHE_LIMIT: usize = 8192;
/// Distinct warnings kept for [`Localizer::take_warnings`].
const WARNING_LIMIT: usize = 256;

struct LangEntry {
    info: LanguageInfo,
    table: Box<dyn TextTable>,
    symbols: Symbols,
}

/// Counters behind `loc.missing` and `loc.bad_template`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// `TXT_KEY_*` keys looked up and not found.
    pub missing: u64,
    /// Templates that failed to parse or evaluate.
    pub bad_template: u64,
}

/// The text dictionaries and the composition engine. Immutable after setup, so it can be shared.
pub struct Localizer {
    langs: Vec<LangEntry>,
    current: usize,
    cache: Mutex<HashMap<String, Arc<Vec<Param>>>>,
    missing: AtomicU64,
    bad_template: AtomicU64,
    warned: Mutex<HashSet<String>>,
    warnings: Mutex<Vec<String>>,
}

/// A `Localizer` bound to one language index (`lookup_language` evaluates in another language).
pub(crate) struct View<'a> {
    loc: &'a Localizer,
    lang: usize,
}

impl View<'_> {
    pub(crate) fn current_language(&self) -> &LanguageInfo {
        &self.loc.langs[self.lang].info
    }

    pub(crate) fn symbols(&self) -> &Symbols {
        &self.loc.langs[self.lang].symbols
    }

    pub(crate) fn cache_get(&self, template: &str) -> Option<Arc<Vec<Param>>> {
        self.loc.cache.lock().ok()?.get(template).cloned()
    }

    pub(crate) fn cache_put(&self, template: &str, params: Arc<Vec<Param>>) {
        if let Ok(mut c) = self.loc.cache.lock() {
            if c.len() >= CACHE_LIMIT {
                c.clear();
            }
            c.insert(template.to_string(), params);
        }
    }

    /// `StringDictionary::TryLookupLanguage`: the row's text with its Plurality and Gender columns applied.
    pub(crate) fn try_lookup_string(&self, key: &str) -> Option<LocString> {
        // re: 0x08c5fb2a StringDictionary::TryLookupLanguageUncached
        let entry = &self.loc.langs[self.lang];
        let row = entry.table.get(key)?;
        let mut s = LocString::new(row.text);
        if let Some(p) = row.plurality {
            // an unknown plural rule has no rule object: SetPlurality fails and the default stays
            if let Some(max) = plural::max_forms(entry.info.plural_rule) {
                s.set_plurality(p, max);
            }
        }
        if let Some(g) = row.gender {
            s.set_gender(g);
        }
        Some(s)
    }

    /// `StringDictionary::Lookup`: a missing key is the string of the key itself.
    pub(crate) fn lookup_string(&self, key: &str) -> LocString {
        // re: 0x08c5f86c StringDictionary::LookupLanguage(char const*, unsigned int, unsigned int)
        if key.is_empty() {
            return LocString::default();
        }
        match self.try_lookup_string(key) {
            Some(s) => s,
            None => {
                if key.starts_with("TXT_KEY") {
                    self.loc.missing.fetch_add(1, Ordering::Relaxed);
                    self.loc.warn_once(format!("missing text key {key}"));
                }
                LocString::new(key)
            }
        }
    }

    pub(crate) fn note_bad_template(&self, template: &str, err: &EvalError) {
        self.loc.bad_template.fetch_add(1, Ordering::Relaxed);
        self.loc
            .warn_once(format!("bad template \"{template}\": {err}"));
    }
}

impl Localizer {
    /// A localizer with one language, which becomes the current one.
    pub fn new(info: LanguageInfo, table: Box<dyn TextTable>) -> Localizer {
        let mut l = Localizer {
            langs: Vec::new(),
            current: 0,
            cache: Mutex::new(HashMap::new()),
            missing: AtomicU64::new(0),
            bad_template: AtomicU64::new(0),
            warned: Mutex::new(HashSet::new()),
            warnings: Mutex::new(Vec::new()),
        };
        l.add_language(info, table);
        l
    }

    /// Adds a language; one with the same type replaces the old.
    pub fn add_language(&mut self, info: LanguageInfo, table: Box<dyn TextTable>) {
        let symbols = Symbols::for_language(&info.type_);
        let entry = LangEntry {
            info,
            table,
            symbols,
        };
        match self
            .langs
            .iter()
            .position(|l| l.info.type_ == entry.info.type_)
        {
            Some(i) => self.langs[i] = entry,
            None => self.langs.push(entry),
        }
        if let Ok(mut c) = self.cache.lock() {
            c.clear();
        }
    }

    /// `Locale.SetCurrentLanguage`: false for an unknown type.
    pub fn set_language(&mut self, type_: &str) -> bool {
        match self.langs.iter().position(|l| l.info.type_ == type_) {
            Some(i) => {
                self.current = i;
                true
            }
            None => false,
        }
    }

    /// `Locale.GetSupportedLanguages`.
    pub fn languages(&self) -> impl Iterator<Item = &LanguageInfo> {
        self.langs.iter().map(|l| &l.info)
    }

    /// `Locale.GetCurrentLanguage`.
    pub fn current_language(&self) -> &LanguageInfo {
        &self.langs[self.current].info
    }

    /// `Locale.GetSupportedSpokenLanguages`.
    pub fn spoken_languages(&self) -> Vec<LanguageInfo> {
        // SIMPLIFIED(SIMP-txt-5): the spoken language is a stub; it always reports en_US
        vec![LanguageInfo::en_us()]
    }

    /// `Locale.GetCurrentSpokenLanguage`.
    pub fn current_spoken_language(&self) -> LanguageInfo {
        // SIMPLIFIED(SIMP-txt-5)
        LanguageInfo::en_us()
    }

    /// Number formatting symbols of the current language.
    pub fn symbols(&self) -> &Symbols {
        &self.langs[self.current].symbols
    }

    fn view(&self) -> View<'_> {
        View {
            loc: self,
            lang: self.current,
        }
    }

    fn warn_once(&self, msg: String) {
        let first = self
            .warned
            .lock()
            .map(|mut w| w.insert(msg.clone()))
            .unwrap_or(false);
        if first
            && let Ok(mut w) = self.warnings.lock()
            && w.len() < WARNING_LIMIT
        {
            w.push(msg);
        }
    }

    /// Counters since creation.
    pub fn stats(&self) -> Stats {
        Stats {
            missing: self.missing.load(Ordering::Relaxed),
            bad_template: self.bad_template.load(Ordering::Relaxed),
        }
    }

    /// New warnings (one per distinct problem) since the last call; the caller logs them.
    pub fn take_warnings(&self) -> Vec<String> {
        self.warnings
            .lock()
            .map(|mut w| std::mem::take(&mut *w))
            .unwrap_or_default()
    }

    /// `Locale.HasTextKey`.
    pub fn has_key(&self, key: &str) -> bool {
        // re: 0x08951b92 cvLuaLocaleLibrary::lHasTextKey
        self.langs[self.current].table.get(key).is_some()
    }

    /// The looked-up string of a key (`Localization::Lookup`), with Gender and Plurality applied.
    pub fn lookup_string(&self, key: &str) -> LocString {
        self.view().lookup_string(key)
    }

    /// `Locale.ConvertTextKey` and `Locale.Lookup`: look the key up (a missing key is its own text),
    /// push the arguments, compose the first form. A template error gives the empty string.
    pub fn convert(&self, key_or_text: &str, args: &[Arg]) -> String {
        // re: 0x08951a12 cvLuaLocaleLibrary::lConvertTextKey, 0x08951d6c lLookup
        self.convert_in(self.current, key_or_text, args)
    }

    /// `convert` that reports the error instead of the empty string.
    pub fn try_convert(&self, key_or_text: &str, args: &[Arg]) -> Result<String, EvalError> {
        let v = self.view();
        let s = v.lookup_string(key_or_text);
        let values: Vec<Value> = args.iter().map(Value::from_arg).collect();
        match s.form(1) {
            Some(f) => v.compose_form(&f.text, &values, 0),
            None => Ok(String::new()),
        }
    }

    fn convert_in(&self, lang: usize, key_or_text: &str, args: &[Arg]) -> String {
        let v = View { loc: self, lang };
        let s = v.lookup_string(key_or_text);
        let values: Vec<Value> = args.iter().map(Value::from_arg).collect();
        v.string_text(&s, 1, &values, 0)
    }

    /// `Locale.LookupLanguage(type, key, ...)`: `None` for an unknown language.
    pub fn lookup_language(&self, language_type: &str, key: &str, args: &[Arg]) -> Option<String> {
        // re: 0x08951eec cvLuaLocaleLibrary::lLookupLanguage
        let lang = self
            .langs
            .iter()
            .position(|l| l.info.type_ == language_type)?;
        Some(self.convert_in(lang, key, args))
    }

    /// Composes a template string that is not a key (`String(template)` + arguments + `toUTF8`).
    pub fn compose(&self, template: &str, args: &[Arg]) -> String {
        let v = self.view();
        let values: Vec<Value> = args.iter().map(Value::from_arg).collect();
        v.string_text(&LocString::new(template), 1, &values, 0)
    }

    /// `Locale.ToNumber(v, pattern)`: the template `{1: number "<pattern>"}`.
    pub fn to_number(&self, v: f64, pattern: &str) -> String {
        // re: 0x0895275e cvLuaLocaleLibrary::lToNumber
        self.compose(&format!("{{1: number \"{pattern}\"}}"), &[Arg::Double(v)])
    }

    /// `Locale.ToPercent`.
    pub fn to_percent(&self, v: f64) -> String {
        // re: 0x08952846 cvLuaLocaleLibrary::lToPercent
        self.compose("{1: percent}", &[Arg::Double(v)])
    }

    /// `Locale.ToCurrency`.
    pub fn to_currency(&self, v: f64) -> String {
        // re: 0x089526ac cvLuaLocaleLibrary::lToCurrency
        self.compose("{1: currency}", &[Arg::Double(v)])
    }

    /// `Locale.ToRomanNumeral`.
    pub fn to_roman(&self, v: f64) -> String {
        // re: 0x089528f8 cvLuaLocaleLibrary::lToRomanNumeral
        self.compose("{1: roman}", &[Arg::Double(v)])
    }

    /// `Locale.ToSpellout`.
    pub fn to_spellout(&self, v: f64) -> String {
        // re: 0x089529aa cvLuaLocaleLibrary::lToSpellout
        self.compose("{1: spellout}", &[Arg::Double(v)])
    }

    /// `Locale.ToOrdinal`.
    pub fn to_ordinal(&self, v: f64) -> String {
        // re: 0x08952a5c cvLuaLocaleLibrary::lToOrdinal
        self.compose("{1: ordinal}", &[Arg::Double(v)])
    }

    /// `Locale.ToUpper`: `{1: upper}` over the argument, so a text key is translated first and only the
    /// first `|` form is converted.
    pub fn to_upper(&self, s: &str) -> String {
        // re: 0x08952558 cvLuaLocaleLibrary::lToUpper
        self.compose("{1: upper}", &[Arg::Str(s.to_string())])
    }

    /// `Locale.ToLower`.
    pub fn to_lower(&self, s: &str) -> String {
        // re: 0x08952602 cvLuaLocaleLibrary::lToLower
        self.compose("{1: lower}", &[Arg::Str(s.to_string())])
    }

    /// `Locale.Compare`.
    pub fn compare(&self, a: &str, b: &str) -> i32 {
        // re: 0x08952d40 cvLuaLocaleLibrary::lCompare
        compare::compare(a, b)
    }
}

impl std::fmt::Debug for Localizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Localizer")
            .field(
                "languages",
                &self.langs.iter().map(|l| &l.info.type_).collect::<Vec<_>>(),
            )
            .field("current", &self.langs[self.current].info.type_)
            .finish()
    }
}
