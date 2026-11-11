//! Frame deltas: the operations that turn the model of one frame into the next.

use std::ops::Deref;

use crate::schema::{FieldId, MAX_LIST_DEPTH, RootId};
use crate::value::{Key, Value};

/// The keys of the lists that enclose a field, outermost first. A fixed array: the schema limits list nesting
/// to [`MAX_LIST_DEPTH`], so building one never allocates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Keys {
    len: u8,
    keys: [Key; MAX_LIST_DEPTH],
}

impl Keys {
    /// No keys.
    #[must_use]
    pub const fn new() -> Keys {
        Keys {
            len: 0,
            keys: [Key(0); MAX_LIST_DEPTH],
        }
    }

    /// From a slice; `None` if it is longer than [`MAX_LIST_DEPTH`].
    #[must_use]
    pub fn from_slice(keys: &[Key]) -> Option<Keys> {
        if keys.len() > MAX_LIST_DEPTH {
            return None;
        }
        let mut k = Keys::new();
        k.keys[..keys.len()].copy_from_slice(keys);
        k.len = keys.len() as u8;
        Some(k)
    }

    /// Add the key of an enclosing list.
    ///
    /// # Panics
    /// If more than [`MAX_LIST_DEPTH`] keys are pushed, which a schema never produces.
    pub fn push(&mut self, key: Key) {
        assert!(
            usize::from(self.len) < MAX_LIST_DEPTH,
            "list nesting exceeds the schema limit"
        );
        self.keys[usize::from(self.len)] = key;
        self.len += 1;
    }

    /// Drop the innermost key.
    pub fn pop(&mut self) {
        self.len = self.len.saturating_sub(1);
    }

    /// The keys.
    #[must_use]
    pub fn as_slice(&self) -> &[Key] {
        &self.keys[..usize::from(self.len)]
    }
}

impl Deref for Keys {
    type Target = [Key];

    fn deref(&self) -> &[Key] {
        self.as_slice()
    }
}

/// One change. An update of a list element is a `Set` of its fields with the chain of keys; there is no
/// separate `Update`.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// A scalar, an optional, or a struct that appears or disappears (`None` and `Some`).
    Set {
        field: FieldId,
        keys: Keys,
        value: Value,
    },
    /// A new list element at position `at` (the list order is the order of the new frame).
    Insert {
        list: FieldId,
        keys: Keys,
        at: u32,
        key: Key,
        value: Value,
    },
    /// An element that is gone.
    Remove { list: FieldId, keys: Keys, key: Key },
    /// An element that changed position: take it out and put it so that it ends up at index `to`.
    Move {
        list: FieldId,
        keys: Keys,
        key: Key,
        to: u32,
    },
    /// A whole root: the first frame, a new skin, a new source, a failed delta.
    Reset { root: RootId, value: Value },
}

/// The operations list of one frame.
pub type Ops = Vec<Op>;

/// The changes of one frame. `revision` grows with every non-empty frame.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameDelta {
    pub revision: u64,
    pub ops: Vec<Op>,
}

/// Where a diff is: the node whose children are being compared and the keys of the enclosing lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    /// The path slot of the current node (a root, a struct field or a list).
    pub field: FieldId,
    pub keys: Keys,
}

impl Cursor {
    /// A cursor at a root.
    #[must_use]
    pub fn at(field: FieldId) -> Cursor {
        Cursor {
            field,
            keys: Keys::new(),
        }
    }
}
