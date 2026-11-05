//! `localStorage` behind `KeyValueStore` (net-client 5): the session token and the last nick.
//!
//! Every access is in a `try`: `localStorage` may be missing, blocked (a private window, site data off) or
//! full, and a read or write may throw. Then the value lives in memory for the life of the page and the
//! player is asked for the password again after a reload; nothing else changes.

use std::cell::Cell;

use platform::{KeyValueStore, MemoryStore};
use web_sys::Storage;

use crate::transport::js_error_text;

pub struct LocalStorage {
    storage: Option<Storage>,
    /// What could not be written to the real storage, and everything when there is none.
    memory: MemoryStore,
    warned: Cell<bool>,
}

impl LocalStorage {
    pub fn new() -> Self {
        let storage = match web_sys::window().map(|w| w.local_storage()) {
            Some(Ok(s)) => s,
            Some(Err(e)) => {
                tracing::warn!("localStorage is not available: {}", js_error_text(&e));
                None
            }
            None => None,
        };
        LocalStorage {
            storage,
            memory: MemoryStore::new(),
            warned: Cell::new(false),
        }
    }

    /// `false`: nothing survives a reload.
    pub fn is_persistent(&self) -> bool {
        self.storage.is_some()
    }

    /// One line in the log for the first failure, not one per call.
    fn failed(&self, what: &str, e: &wasm_bindgen::JsValue) {
        if !self.warned.replace(true) {
            tracing::warn!("localStorage {what} failed: {}", js_error_text(e));
        }
    }
}

impl Default for LocalStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyValueStore for LocalStorage {
    fn get(&self, key: &str) -> Option<String> {
        if let Some(s) = &self.storage {
            match s.get_item(key) {
                Ok(Some(v)) => return Some(v),
                Ok(None) => {}
                Err(e) => self.failed("read", &e),
            }
        }
        self.memory.get(key)
    }

    fn set(&self, key: &str, value: &str) {
        if let Some(s) = &self.storage {
            match s.set_item(key, value) {
                Ok(()) => {
                    self.memory.remove(key);
                    return;
                }
                Err(e) => self.failed("write", &e),
            }
        }
        self.memory.set(key, value);
    }

    fn keeps_tokens(&self) -> bool {
        false
    }

    fn remove(&self, key: &str) {
        if let Some(s) = &self.storage
            && let Err(e) = s.remove_item(key)
        {
            self.failed("remove", &e);
        }
        self.memory.remove(key);
    }

    fn keys_with_prefix(&self, prefix: &str) -> Vec<String> {
        let mut keys = self.memory.keys_with_prefix(prefix);
        if let Some(s) = &self.storage
            && let Ok(n) = s.length()
        {
            for i in 0..n {
                if let Ok(Some(k)) = s.key(i)
                    && k.starts_with(prefix)
                    && !keys.contains(&k)
                {
                    keys.push(k);
                }
            }
        }
        keys
    }
}
