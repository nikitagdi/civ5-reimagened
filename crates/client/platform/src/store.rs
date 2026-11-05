//! Small persistent storage and page events (net-client 5, `platform`).
//!
//! `KeyValueStore` keeps the session token and the last nick (`localStorage` in the browser, memory in
//! tests and bots). `HostEvent` is what the page tells the network layer about itself.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

/// String storage by key. Never fails loudly: a store that cannot be read or written (a private window, a
/// blocked `localStorage`) behaves as an empty one, and the caller carries on without it.
pub trait KeyValueStore {
    fn get(&self, key: &str) -> Option<String>;

    fn set(&self, key: &str, value: &str);

    fn remove(&self, key: &str);

    /// The keys that start with `prefix` (the account store uploads the settings the device already has). A store that
    /// cannot list its keys answers nothing.
    fn keys_with_prefix(&self, _prefix: &str) -> Vec<String> {
        Vec::new()
    }

    /// Whether the device token may be kept here (`civ5.token.<nick>`). The web page keeps none: its session is an
    /// HttpOnly cookie that script cannot read (SIMP-exe-854); a native client keeps the token in its store.
    fn keeps_tokens(&self) -> bool {
        true
    }
}

/// What the page tells about itself: the network, the visibility of the tab, leaving the page. The client
/// core has its own copy of this type (`client_core::HostEvent`); the shell maps one to the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostEvent {
    Online,
    Offline,
    Visible,
    Hidden,
    PageHide,
}

/// A store in memory. Clones share the contents, so a test keeps a handle and looks into what the code
/// under test wrote.
#[derive(Debug, Clone, Default)]
pub struct MemoryStore {
    items: Rc<RefCell<BTreeMap<String, String>>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.items.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.borrow().is_empty()
    }
}

impl KeyValueStore for MemoryStore {
    fn get(&self, key: &str) -> Option<String> {
        self.items.borrow().get(key).cloned()
    }

    fn set(&self, key: &str, value: &str) {
        self.items
            .borrow_mut()
            .insert(key.to_owned(), value.to_owned());
    }

    fn remove(&self, key: &str) {
        self.items.borrow_mut().remove(key);
    }

    fn keys_with_prefix(&self, prefix: &str) -> Vec<String> {
        self.items
            .borrow()
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_the_contents() {
        let a = MemoryStore::new();
        let b = a.clone();
        assert!(a.is_empty());
        a.set("k", "v");
        assert_eq!(b.get("k").as_deref(), Some("v"));
        b.set("k", "w");
        assert_eq!(a.get("k").as_deref(), Some("w"));
        a.remove("k");
        assert_eq!((b.get("k"), b.len()), (None, 0));
        a.remove("absent");
    }

    #[test]
    fn keys_are_listed_by_prefix() {
        let a = MemoryStore::new();
        a.set("civ5.options.A", "1");
        a.set("civ5.options.B", "2");
        a.set("civ5.token.x", "t");
        assert_eq!(
            a.keys_with_prefix("civ5.options."),
            ["civ5.options.A", "civ5.options.B"]
        );
        assert!(a.keys_with_prefix("none").is_empty());
    }
}
