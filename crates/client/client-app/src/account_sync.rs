//! The account store on the client (protocol v12): the options-screen settings and the in-game toggles
//! (`civ5.options.*` of the page's key-value store) follow the player's account on the server.
//!
//! * [`AccountKv`] wraps the page's store for the original UI: every write or removal of a `civ5.options.<Key>`
//!   goes to the local store as before (the local fallback) and is noted in [`AccountSync`] as a change of the
//!   account's `<Key>`.
//! * [`AccountSync::on_loaded`] runs when `ServerMsg::Account` arrives: the server's values are written into the
//!   local store (the server wins), and the keys only the device has are sent up (the first login after the store
//!   came, a setting made while off line).
//! * [`AccountSync::flush`] sends what changed, at most once per [`SEND_EVERY_MS`] and at most
//!   [`protocol::MAX_SAVE_ENTRIES`] keys a message, so a volume slider is one message, not one per pixel.
//!
//! Without a login (or on a server without the store) nothing is sent and the local store carries on alone.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use client_core::ClientCore;
use platform::KeyValueStore;
use protocol::{AccountData, AccountSave, MAX_SAVE_ENTRIES, key_ok, value_ok};

// SIMPLIFIED(SIMP-exe-1086): only the `civ5.options.*` keys follow the account.
// SIMPLIFIED(SIMP-exe-1089): server wins at login, last writer wins per key, one message a second at most.
/// Prefix of the keys in the local store that the account follows (`exe_api::options::KEY_PREFIX`).
pub const LOCAL_PREFIX: &str = "civ5.options.";

/// The shortest time between two messages.
pub const SEND_EVERY_MS: u64 = 1_000;

#[derive(Default)]
pub struct AccountSync {
    /// Changes not sent yet, by account key: `None` is a removal.
    pending: RefCell<BTreeMap<String, Option<String>>>,
    /// Our own writes of the server's values: not changes of the player.
    applying: Cell<bool>,
    last_sent_ms: Cell<Option<u64>>,
}

/// The account key of a local key, if the account follows it.
pub fn account_key(local: &str) -> Option<&str> {
    local.strip_prefix(LOCAL_PREFIX).filter(|k| key_ok(k))
}

impl AccountSync {
    pub fn new() -> Rc<Self> {
        Rc::new(Self::default())
    }

    fn note(&self, local_key: &str, value: Option<&str>) {
        if self.applying.get() {
            return;
        }
        let Some(key) = account_key(local_key) else {
            return;
        };
        match value {
            Some(v) if !value_ok(v) => {}
            v => {
                self.pending
                    .borrow_mut()
                    .insert(key.to_owned(), v.map(str::to_owned));
            }
        }
    }

    /// Changes waiting to be sent.
    pub fn pending_len(&self) -> usize {
        self.pending.borrow().len()
    }

    /// The server's store came: its values go into the local store, the keys only the device has go up.
    pub fn on_loaded(&self, data: &AccountData, local: &dyn KeyValueStore) {
        // A store of a newer schema is not read, and not written over: the device keeps its own.
        if data.version > protocol::ACCOUNT_VERSION {
            return;
        }
        self.applying.set(true);
        for (k, v) in &data.prefs {
            local.set(&format!("{LOCAL_PREFIX}{k}"), v);
        }
        self.applying.set(false);
        // What the server does not have and the device has: uploaded (merged; the server's value won above).
        let mut pending = self.pending.borrow_mut();
        for lk in local.keys_with_prefix(LOCAL_PREFIX) {
            let Some(k) = account_key(&lk) else {
                continue;
            };
            if data.prefs.iter().any(|(sk, _)| sk == k) {
                // A change made before the store came wins over the server's value: the pending entry stays and
                // `local` already holds the server's, so write the pending one back.
                match pending.get(k) {
                    Some(Some(v)) => {
                        self.applying.set(true);
                        local.set(&lk, v);
                        self.applying.set(false);
                    }
                    Some(None) => {
                        self.applying.set(true);
                        local.remove(&lk);
                        self.applying.set(false);
                    }
                    None => {}
                }
                continue;
            }
            if let Some(v) = local.get(&lk).filter(|v| value_ok(v)) {
                pending.entry(k.to_owned()).or_insert(Some(v));
            }
        }
    }

    /// Sends the changes if it is time. Returns whether a message went.
    pub fn flush(&self, core: &mut ClientCore, now_ms: u64) -> bool {
        if self.pending.borrow().is_empty() {
            return false;
        }
        if self
            .last_sent_ms
            .get()
            .is_some_and(|t| now_ms.saturating_sub(t) < SEND_EVERY_MS)
        {
            return false;
        }
        // Nothing is sent before the store came: the first message would overwrite what the server holds with a
        // partial picture (`on_loaded` merges first).
        if core.account().is_none() {
            return false;
        }
        let mut save = AccountSave::default();
        let mut taken = Vec::new();
        {
            let pending = self.pending.borrow();
            for (k, v) in pending.iter().take(MAX_SAVE_ENTRIES) {
                match v {
                    Some(v) => save.set.push((k.clone(), v.clone())),
                    None => save.remove.push(k.clone()),
                }
                taken.push(k.clone());
            }
        }
        if !core.save_account(save) {
            return false;
        }
        let mut pending = self.pending.borrow_mut();
        for k in taken {
            pending.remove(&k);
        }
        self.last_sent_ms.set(Some(now_ms));
        true
    }
}

/// The page's store seen by the original UI: writes are noted for the account.
pub struct AccountKv {
    inner: Rc<dyn KeyValueStore>,
    sync: Rc<AccountSync>,
}

impl AccountKv {
    pub fn new(inner: Rc<dyn KeyValueStore>, sync: Rc<AccountSync>) -> Self {
        Self { inner, sync }
    }
}

impl KeyValueStore for AccountKv {
    fn get(&self, key: &str) -> Option<String> {
        self.inner.get(key)
    }

    fn set(&self, key: &str, value: &str) {
        if self.inner.get(key).as_deref() != Some(value) {
            self.sync.note(key, Some(value));
        }
        self.inner.set(key, value);
    }

    fn remove(&self, key: &str) {
        if self.inner.get(key).is_some() {
            self.sync.note(key, None);
        }
        self.inner.remove(key);
    }

    fn keys_with_prefix(&self, prefix: &str) -> Vec<String> {
        self.inner.keys_with_prefix(prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform::MemoryStore;

    fn kv() -> (MemoryStore, AccountKv, Rc<AccountSync>) {
        let mem = MemoryStore::new();
        let sync = AccountSync::new();
        let kv = AccountKv::new(Rc::new(mem.clone()), Rc::clone(&sync));
        (mem, kv, sync)
    }

    fn data(prefs: &[(&str, &str)]) -> AccountData {
        AccountData {
            version: protocol::ACCOUNT_VERSION,
            prefs: prefs
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
            last_room: None,
        }
    }

    #[test]
    fn option_writes_are_kept_locally_and_noted() {
        let (mem, kv, sync) = kv();
        kv.set("civ5.options.GridOn", "0");
        assert_eq!(mem.get("civ5.options.GridOn").as_deref(), Some("0"));
        assert_eq!(sync.pending_len(), 1);
        // The same value again, and keys the account does not follow: not a change.
        kv.set("civ5.options.GridOn", "0");
        kv.set("civ5.token.bob", "t");
        kv.set("civ5.userdata.x", "{}");
        kv.set("civ5.options.bad key", "1");
        assert_eq!(sync.pending_len(), 1);
        kv.remove("civ5.options.GridOn");
        assert_eq!(
            sync.pending.borrow().get("GridOn"),
            Some(&None),
            "a removal is noted"
        );
    }

    #[test]
    fn the_servers_values_land_in_the_local_store_without_being_sent_back() {
        let (mem, kv, sync) = kv();
        kv.set("civ5.options.YieldOn", "1");
        sync.pending.borrow_mut().clear();
        sync.on_loaded(&data(&[("GridOn", "0"), ("Volume.0", "0.25")]), &kv);
        assert_eq!(mem.get("civ5.options.GridOn").as_deref(), Some("0"));
        assert_eq!(mem.get("civ5.options.Volume.0").as_deref(), Some("0.25"));
        // Only the key the server lacks goes up.
        let pending = sync.pending.borrow();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending.get("YieldOn"), Some(&Some("1".to_string())));
    }

    #[test]
    fn a_change_made_before_the_store_came_wins() {
        let (mem, kv, sync) = kv();
        kv.set("civ5.options.GridOn", "1");
        sync.on_loaded(&data(&[("GridOn", "0")]), &kv);
        assert_eq!(mem.get("civ5.options.GridOn").as_deref(), Some("1"));
        assert_eq!(
            sync.pending.borrow().get("GridOn"),
            Some(&Some("1".to_string()))
        );
    }

    #[test]
    fn a_store_of_a_newer_schema_is_left_alone() {
        let (mem, kv, sync) = kv();
        let mut d = data(&[("GridOn", "0")]);
        d.version = protocol::ACCOUNT_VERSION + 1;
        sync.on_loaded(&d, &kv);
        assert!(mem.get("civ5.options.GridOn").is_none());
        assert_eq!(sync.pending_len(), 0);
    }
}
