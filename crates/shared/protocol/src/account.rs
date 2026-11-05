//! The per-account store (v12): a small persistent bag of the player's settings that lives on the server, keyed
//! by nick. The client loads it at login (`ServerMsg::Account`) and sends what changed (`ClientMsg::Account`).
//! Field and variant order is the wire format.
//!
//! What it holds: the options-screen settings and the in-game toggles (text values under short keys, the client
//! decides the keys: `GridOn`, `YieldOn`, `Volume.0`, ...) and the settings of the room the player hosted last
//! (saved by the server when that game starts, the defaults of the next room the player creates).
//!
//! Bounded and validated on both sides by the same functions below, so a client never sends what the server
//! would refuse: at most [`MAX_PREFS`] entries, a key of 1..=[`MAX_KEY_CHARS`] characters of `[A-Za-z0-9._-]`, a
//! value of at most [`MAX_VALUE_CHARS`] characters without control characters, and [`MAX_PREFS_BYTES`] bytes of
//! keys and values together.

use serde::{Deserialize, Serialize};

use crate::lobby::GameSettings;

/// Version of the stored data (`AccountData.version`). A newer client reads an older store; a client that finds a
/// version above its own ignores the store and keeps its local settings.
pub const ACCOUNT_VERSION: u16 = 1;

pub const MAX_PREFS: usize = 128;
pub const MAX_KEY_CHARS: usize = 64;
pub const MAX_VALUE_CHARS: usize = 256;
/// Bytes of all keys and values of one account.
pub const MAX_PREFS_BYTES: usize = 16 * 1024;
/// Entries of one `ClientMsg::Account`.
pub const MAX_SAVE_ENTRIES: usize = 64;

/// `ServerMsg::Account`: the whole store of the signed-in nick, once after login and again when the server changes
/// `last_room`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountData {
    pub version: u16,
    /// Settings and toggles, sorted by key.
    pub prefs: Vec<(String, String)>,
    /// The settings of the room the player started a game from last.
    pub last_room: Option<GameSettings>,
}

/// `ClientMsg::Account`: a change of the store. `set` entries are added or replaced, `remove` keys are dropped
/// (removals first, then sets).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSave {
    pub set: Vec<(String, String)>,
    pub remove: Vec<String>,
}

/// Why a store change was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountError {
    BadKey(String),
    BadValue(String),
    TooManyEntries,
    TooBig,
}

impl std::fmt::Display for AccountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AccountError::BadKey(k) => write!(f, "bad settings key {k:?}"),
            AccountError::BadValue(k) => write!(f, "bad settings value for {k:?}"),
            AccountError::TooManyEntries => f.write_str("too many settings"),
            AccountError::TooBig => f.write_str("the settings are too big"),
        }
    }
}

pub fn key_ok(key: &str) -> bool {
    let n = key.chars().count();
    (1..=MAX_KEY_CHARS).contains(&n)
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

pub fn value_ok(value: &str) -> bool {
    value.chars().count() <= MAX_VALUE_CHARS && !value.chars().any(char::is_control)
}

impl AccountSave {
    /// Checks the shape of one message (not the size of the store it lands in).
    pub fn validate(&self) -> Result<(), AccountError> {
        if self.set.len() + self.remove.len() > MAX_SAVE_ENTRIES {
            return Err(AccountError::TooManyEntries);
        }
        for (k, v) in &self.set {
            if !key_ok(k) {
                return Err(AccountError::BadKey(k.clone()));
            }
            if !value_ok(v) {
                return Err(AccountError::BadValue(k.clone()));
            }
        }
        for k in &self.remove {
            if !key_ok(k) {
                return Err(AccountError::BadKey(k.clone()));
            }
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.set.is_empty() && self.remove.is_empty()
    }
}

/// Bytes of keys and values.
pub fn prefs_bytes(prefs: &[(String, String)]) -> usize {
    prefs.iter().map(|(k, v)| k.len() + v.len()).sum()
}

/// Applies `save` to `prefs` (kept sorted by key) and checks the bounds of the result. `prefs` is unchanged on an
/// error.
pub fn apply_save(
    prefs: &mut Vec<(String, String)>,
    save: &AccountSave,
) -> Result<(), AccountError> {
    save.validate()?;
    let mut next: std::collections::BTreeMap<String, String> = prefs.iter().cloned().collect();
    for k in &save.remove {
        next.remove(k);
    }
    for (k, v) in &save.set {
        next.insert(k.clone(), v.clone());
    }
    let list: Vec<(String, String)> = next.into_iter().collect();
    if list.len() > MAX_PREFS {
        return Err(AccountError::TooManyEntries);
    }
    if prefs_bytes(&list) > MAX_PREFS_BYTES {
        return Err(AccountError::TooBig);
    }
    *prefs = list;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn save(set: &[(&str, &str)], remove: &[&str]) -> AccountSave {
        AccountSave {
            set: set
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
            remove: remove.iter().map(|k| (*k).into()).collect(),
        }
    }

    #[test]
    fn keys_and_values_are_checked() {
        assert!(key_ok("GridOn") && key_ok("Volume.0") && key_ok("a-b_c"));
        assert!(!key_ok("") && !key_ok("a b") && !key_ok("ключ") && !key_ok(&"k".repeat(65)));
        assert!(value_ok("") && value_ok("0.5") && value_ok(&"v".repeat(256)));
        assert!(!value_ok(&"v".repeat(257)) && !value_ok("a\nb") && !value_ok("a\u{0}b"));
    }

    #[test]
    fn a_save_sets_removes_and_keeps_the_list_sorted() {
        let mut p = Vec::new();
        apply_save(&mut p, &save(&[("b", "2"), ("a", "1")], &[])).unwrap();
        assert_eq!(p, [("a".into(), "1".into()), ("b".into(), "2".into())]);
        apply_save(&mut p, &save(&[("c", "3")], &["a"])).unwrap();
        assert_eq!(p, [("b".into(), "2".into()), ("c".into(), "3".into())]);
        apply_save(&mut p, &save(&[("b", "9")], &[])).unwrap();
        assert_eq!(p[0], ("b".into(), "9".into()));
    }

    #[test]
    fn a_refused_save_changes_nothing() {
        let mut p = vec![("a".to_string(), "1".to_string())];
        let before = p.clone();
        assert_eq!(
            apply_save(&mut p, &save(&[("bad key", "1")], &[])),
            Err(AccountError::BadKey("bad key".into()))
        );
        assert_eq!(
            apply_save(&mut p, &save(&[("k", &"v".repeat(300))], &[])),
            Err(AccountError::BadValue("k".into()))
        );
        let many: Vec<(String, String)> = (0..65).map(|i| (format!("k{i}"), "1".into())).collect();
        assert_eq!(
            apply_save(
                &mut p,
                &AccountSave {
                    set: many,
                    remove: vec![]
                }
            ),
            Err(AccountError::TooManyEntries)
        );
        assert_eq!(p, before);
    }

    #[test]
    fn the_store_is_bounded_in_entries_and_bytes() {
        let mut p = Vec::new();
        for chunk in 0..2 {
            let set: Vec<(String, String)> = (0..64)
                .map(|i| (format!("key{chunk}-{i}"), "x".into()))
                .collect();
            apply_save(
                &mut p,
                &AccountSave {
                    set,
                    remove: vec![],
                },
            )
            .unwrap();
        }
        assert_eq!(p.len(), 128);
        assert_eq!(
            apply_save(&mut p, &save(&[("one-more", "x")], &[])),
            Err(AccountError::TooManyEntries)
        );
        // Bytes: 64 values of 256 characters is 16 KiB with the keys, over the bound.
        let mut p = Vec::new();
        let mut last = Ok(());
        for chunk in 0..2 {
            let set: Vec<(String, String)> = (0..32)
                .map(|i| (format!("k{chunk}-{i}"), "v".repeat(256)))
                .collect();
            last = apply_save(
                &mut p,
                &AccountSave {
                    set,
                    remove: vec![],
                },
            );
        }
        assert_eq!(last, Err(AccountError::TooBig));
        assert!(prefs_bytes(&p) <= MAX_PREFS_BYTES);
    }
}
