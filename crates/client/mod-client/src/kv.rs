//! The per-mod key-value store (K1) over the platform's `KeyValueStore`.

use std::rc::Rc;

use mod_manifest::ModId;
use platform::KeyValueStore;

use crate::ctx::ModKv;

/// Longest key a mod may use, in bytes.
pub const KV_KEY_MAX: usize = 128;
/// Longest value a mod may store, in bytes.
pub const KV_VALUE_MAX: usize = 64 * 1024;

/// A mod's view of the platform store: every key is prefixed `civ5.mod.<id>.`, so mods cannot read or overwrite
/// each other's (or the game's) entries. Like the platform store it never fails loudly: a key or value over the
/// limit is dropped.
pub struct NamespacedKv {
    store: Rc<dyn KeyValueStore>,
    prefix: String,
}

impl NamespacedKv {
    pub fn new(store: Rc<dyn KeyValueStore>, id: &ModId) -> Self {
        Self {
            store,
            prefix: format!("civ5.mod.{id}."),
        }
    }

    fn key(&self, key: &str) -> Option<String> {
        (!key.is_empty() && key.len() <= KV_KEY_MAX).then(|| format!("{}{key}", self.prefix))
    }
}

impl ModKv for NamespacedKv {
    fn get(&self, key: &str) -> Option<String> {
        self.store.get(&self.key(key)?)
    }

    fn set(&mut self, key: &str, value: &str) {
        if value.len() > KV_VALUE_MAX {
            return;
        }
        if let Some(k) = self.key(key) {
            self.store.set(&k, value);
        }
    }

    fn remove(&mut self, key: &str) {
        if let Some(k) = self.key(key) {
            self.store.remove(&k);
        }
    }
}
