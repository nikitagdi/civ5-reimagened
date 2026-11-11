//! Settings of skins and mods: small JSON values kept on the device, under keys that start with `civ5.ui2.`
//! (never `civ5.options.*`, which the account sync takes). The module keeps the values in memory, hands them
//! to the script worker at start ([`settings_snapshot`](crate::UiActions::settings_snapshot)), and writes
//! changes through to the host's [`SettingsStore`] at most once per second, all pending keys together.
//!
//! Limits, per skin or mod: a key is 1 to 64 characters of `A-Z a-z 0-9 . _ -`, a value is JSON of at most 4
//! KiB, at most 64 keys. A change that breaks a limit is logged and dropped.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use crate::state::{SkinId, StateError};

/// The longest key.
pub const MAX_KEY_LEN: usize = 64;
/// The largest value, in bytes of JSON text.
pub const MAX_VALUE_LEN: usize = 4096;
/// The most keys of one skin or mod.
pub const MAX_KEYS: usize = 64;
/// The shortest time between two writes to the store, in milliseconds.
pub const WRITE_INTERVAL_MS: u64 = 1000;

/// The device storage the host gives: a string key-value store (`platform::KeyValueStore` in the app).
pub trait SettingsStore {
    /// The value under a key.
    fn get(&self, key: &str) -> Option<String>;
    /// Write a value.
    fn set(&mut self, key: &str, value: &str);
    /// Remove a key.
    fn remove(&mut self, key: &str);
}

/// Whose settings: a skin's (by skin id) or a mod's (by mod id).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SettingsNs {
    Skin(SkinId),
    Mod(Arc<str>),
}

impl SettingsNs {
    fn id(&self) -> &str {
        match self {
            SettingsNs::Skin(id) | SettingsNs::Mod(id) => id,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            SettingsNs::Skin(_) => "skin",
            SettingsNs::Mod(_) => "mod",
        }
    }

    /// The store key of a setting.
    fn key(&self, key: &str) -> String {
        format!("civ5.ui2.{}.{}.{key}", self.kind(), self.id())
    }

    /// The store key of the list of the keys that are set (the store cannot list its keys).
    fn index_key(&self) -> String {
        format!("civ5.ui2.index.{}.{}", self.kind(), self.id())
    }

    fn check(&self) -> Result<(), StateError> {
        let id = self.id();
        if (1..=MAX_KEY_LEN).contains(&id.len())
            && id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            Ok(())
        } else {
            Err(StateError::BadNamespace(id.to_string()))
        }
    }
}

fn valid_key(key: &str) -> bool {
    (1..=MAX_KEY_LEN).contains(&key.len())
        && key
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

#[derive(Default)]
struct NsData {
    values: BTreeMap<String, String>,
    /// Keys changed since the last write (set or removed).
    pending: BTreeSet<String>,
    /// The set of keys changed, so the index is written too.
    index_dirty: bool,
}

/// The settings of all skins and mods of the session.
pub(crate) struct Settings {
    store: Box<dyn SettingsStore>,
    loaded: HashMap<SettingsNs, NsData>,
    last_write_ms: Option<u64>,
}

impl Settings {
    pub(crate) fn new(store: Box<dyn SettingsStore>) -> Settings {
        Settings {
            store,
            loaded: HashMap::new(),
            last_write_ms: None,
        }
    }

    fn data(&mut self, ns: &SettingsNs) -> Result<&mut NsData, StateError> {
        ns.check()?;
        Ok(match self.loaded.entry(ns.clone()) {
            Entry::Occupied(o) => o.into_mut(),
            Entry::Vacant(v) => {
                let mut data = NsData::default();
                let keys: Vec<String> = self
                    .store
                    .get(&ns.index_key())
                    .and_then(|t| serde_json::from_str(&t).ok())
                    .unwrap_or_default();
                for key in keys.into_iter().filter(|k| valid_key(k)) {
                    if let Some(v) = self.store.get(&ns.key(&key)) {
                        data.values.insert(key, v);
                    }
                }
                v.insert(data)
            }
        })
    }

    /// Every setting of a namespace, by key.
    pub(crate) fn snapshot(&mut self, ns: &SettingsNs) -> Vec<(String, String)> {
        match self.data(ns) {
            Ok(d) => d
                .values
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Set (`Some(json)`) or remove (`None`) a setting and write what is due.
    pub(crate) fn set(
        &mut self,
        ns: &SettingsNs,
        key: &str,
        json: Option<&str>,
        now_ms: u64,
    ) -> Result<(), StateError> {
        let result = self.set_checked(ns, key, json);
        if let Err(e) = &result {
            tracing::warn!("ui2-actions: setting {key:?} dropped: {e}");
        }
        self.flush(now_ms);
        result
    }

    fn set_checked(
        &mut self,
        ns: &SettingsNs,
        key: &str,
        json: Option<&str>,
    ) -> Result<(), StateError> {
        if !valid_key(key) {
            return Err(StateError::BadKey(key.to_string()));
        }
        if let Some(text) = json {
            if text.len() > MAX_VALUE_LEN {
                return Err(StateError::ValueTooBig {
                    key: key.to_string(),
                    len: text.len(),
                });
            }
            if let Err(e) = serde_json::from_str::<serde_json::Value>(text) {
                return Err(StateError::BadJson {
                    key: key.to_string(),
                    msg: e.to_string(),
                });
            }
        }
        let data = self.data(ns)?;
        match json {
            Some(text) => {
                let new_key = !data.values.contains_key(key);
                if new_key && data.values.len() >= MAX_KEYS {
                    return Err(StateError::TooManyKeys(ns.id().to_string()));
                }
                if data.values.get(key).map(String::as_str) == Some(text) {
                    return Ok(());
                }
                data.values.insert(key.to_string(), text.to_string());
                data.index_dirty |= new_key;
            }
            None => {
                if data.values.remove(key).is_none() {
                    return Ok(());
                }
                data.index_dirty = true;
            }
        }
        data.pending.insert(key.to_string());
        Ok(())
    }

    /// Write the pending changes if the last write is at least [`WRITE_INTERVAL_MS`] old (or there was none).
    pub(crate) fn flush(&mut self, now_ms: u64) {
        if self.loaded.values().all(|d| d.pending.is_empty()) {
            return;
        }
        if let Some(last) = self.last_write_ms
            && now_ms < last.saturating_add(WRITE_INTERVAL_MS)
        {
            return;
        }
        for (ns, data) in &mut self.loaded {
            if data.pending.is_empty() {
                continue;
            }
            for key in std::mem::take(&mut data.pending) {
                match data.values.get(&key) {
                    Some(v) => self.store.set(&ns.key(&key), v),
                    None => self.store.remove(&ns.key(&key)),
                }
            }
            if std::mem::take(&mut data.index_dirty) {
                let keys: Vec<&String> = data.values.keys().collect();
                // A list of strings always serializes.
                let text = serde_json::to_string(&keys).unwrap_or_else(|_| "[]".to_string());
                self.store.set(&ns.index_key(), &text);
            }
        }
        self.last_write_ms = Some(now_ms);
    }
}

/// A settings store in memory, for tests and for hosts without a device store. Clones share the contents, so
/// a test can keep a handle after giving one to the module.
#[derive(Clone, Debug, Default)]
pub struct MemorySettings {
    inner: Arc<Mutex<MemoryInner>>,
}

#[derive(Debug, Default)]
struct MemoryInner {
    map: BTreeMap<String, String>,
    writes: usize,
}

impl MemorySettings {
    fn lock(&self) -> std::sync::MutexGuard<'_, MemoryInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Every key and value now in the store.
    #[must_use]
    pub fn contents(&self) -> BTreeMap<String, String> {
        self.lock().map.clone()
    }

    /// How many times `set` or `remove` was called.
    #[must_use]
    pub fn writes(&self) -> usize {
        self.lock().writes
    }
}

impl SettingsStore for MemorySettings {
    fn get(&self, key: &str) -> Option<String> {
        self.lock().map.get(key).cloned()
    }

    fn set(&mut self, key: &str, value: &str) {
        let mut g = self.lock();
        g.writes += 1;
        g.map.insert(key.to_string(), value.to_string());
    }

    fn remove(&mut self, key: &str) {
        let mut g = self.lock();
        g.writes += 1;
        g.map.remove(key);
    }
}
