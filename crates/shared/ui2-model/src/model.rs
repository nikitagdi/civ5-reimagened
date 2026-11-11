//! The typed side of the model: the [`Model`] trait that generated structs implement, [`KeyedList`], and the
//! implementations for scalars and optionals.

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;

use crate::delta::{Cursor, Op, Ops};
use crate::diff::{ListStep, list_steps};
use crate::value::{AssetRef, Key, Text, Value, ValueError};

/// A value that knows its place in the model: it diffs itself against an older value without building
/// [`Value`]s (only the payload of a changed field is converted), and converts to and from [`Value`].
pub trait Model: Sized {
    /// Append the operations that turn `old` into `self`. `at` is the node that holds `self`: for a struct the
    /// children are the path slots after it; for a scalar it is the field itself. `at` is restored on return.
    fn diff(&self, old: &Self, at: &mut Cursor, out: &mut Ops);

    /// The dynamic form.
    fn to_value(&self) -> Value;

    /// Rebuild from the dynamic form.
    ///
    /// # Errors
    /// The value does not have the shape of the type.
    fn from_value(v: &Value) -> Result<Self, ValueError>;
}

/// A generated root struct: one per `[model.<name>]`.
pub trait Root: Model + Clone + Default + Send + 'static {
    /// The name of the root in the schema (`top`).
    const NAME: &'static str;
}

/// A struct that can be a list element: it knows its key.
pub trait Keyed {
    /// The key of this element (derived from the key field).
    fn key(&self) -> Key;
}

fn set(at: &Cursor, value: Value, out: &mut Ops) {
    out.push(Op::Set {
        field: at.field,
        keys: at.keys,
        value,
    });
}

impl Model for bool {
    fn diff(&self, old: &Self, at: &mut Cursor, out: &mut Ops) {
        if self != old {
            set(at, self.to_value(), out);
        }
    }

    fn to_value(&self) -> Value {
        Value::Bool(*self)
    }

    fn from_value(v: &Value) -> Result<Self, ValueError> {
        match v {
            Value::Bool(b) => Ok(*b),
            other => Err(ValueError::mismatch("bool", other)),
        }
    }
}

macro_rules! small_int {
    ($t:ty, $name:literal) => {
        impl Model for $t {
            fn diff(&self, old: &Self, at: &mut Cursor, out: &mut Ops) {
                if self != old {
                    set(at, self.to_value(), out);
                }
            }

            fn to_value(&self) -> Value {
                Value::I32(i32::from(*self))
            }

            fn from_value(v: &Value) -> Result<Self, ValueError> {
                match v {
                    Value::I32(n) => <$t>::try_from(*n).map_err(|_| ValueError::Range {
                        value: i64::from(*n),
                        ty: $name,
                    }),
                    other => Err(ValueError::mismatch("i32", other)),
                }
            }
        }
    };
}

small_int!(i8, "i8");
small_int!(i16, "i16");
small_int!(i32, "i32");

impl Model for u32 {
    fn diff(&self, old: &Self, at: &mut Cursor, out: &mut Ops) {
        if self != old {
            set(at, self.to_value(), out);
        }
    }

    fn to_value(&self) -> Value {
        Value::U32(*self)
    }

    fn from_value(v: &Value) -> Result<Self, ValueError> {
        match v {
            Value::U32(n) => Ok(*n),
            other => Err(ValueError::mismatch("u32", other)),
        }
    }
}

impl Model for f32 {
    fn diff(&self, old: &Self, at: &mut Cursor, out: &mut Ops) {
        if self.to_bits() != old.to_bits() {
            set(at, self.to_value(), out);
        }
    }

    fn to_value(&self) -> Value {
        Value::F32(*self)
    }

    fn from_value(v: &Value) -> Result<Self, ValueError> {
        match v {
            Value::F32(n) => Ok(*n),
            other => Err(ValueError::mismatch("f32", other)),
        }
    }
}

impl Model for Arc<str> {
    fn diff(&self, old: &Self, at: &mut Cursor, out: &mut Ops) {
        if self != old {
            set(at, self.to_value(), out);
        }
    }

    fn to_value(&self) -> Value {
        Value::Str(self.clone())
    }

    fn from_value(v: &Value) -> Result<Self, ValueError> {
        match v {
            Value::Str(s) => Ok(s.clone()),
            other => Err(ValueError::mismatch("str", other)),
        }
    }
}

impl Model for Text {
    fn diff(&self, old: &Self, at: &mut Cursor, out: &mut Ops) {
        if self != old {
            set(at, self.to_value(), out);
        }
    }

    fn to_value(&self) -> Value {
        Value::Text(self.clone())
    }

    fn from_value(v: &Value) -> Result<Self, ValueError> {
        match v {
            Value::Text(t) => Ok(t.clone()),
            other => Err(ValueError::mismatch("text", other)),
        }
    }
}

impl Model for AssetRef {
    fn diff(&self, old: &Self, at: &mut Cursor, out: &mut Ops) {
        if self != old {
            set(at, self.to_value(), out);
        }
    }

    fn to_value(&self) -> Value {
        Value::Asset(self.clone())
    }

    fn from_value(v: &Value) -> Result<Self, ValueError> {
        match v {
            Value::Asset(a) => Ok(a.clone()),
            other => Err(ValueError::mismatch("asset", other)),
        }
    }
}

impl Model for Key {
    fn diff(&self, old: &Self, at: &mut Cursor, out: &mut Ops) {
        if self != old {
            set(at, self.to_value(), out);
        }
    }

    fn to_value(&self) -> Value {
        Value::Key(*self)
    }

    fn from_value(v: &Value) -> Result<Self, ValueError> {
        match v {
            Value::Key(k) => Ok(*k),
            other => Err(ValueError::mismatch("key", other)),
        }
    }
}

impl<T: Model> Model for Option<T> {
    fn diff(&self, old: &Self, at: &mut Cursor, out: &mut Ops) {
        match (self, old) {
            (None, None) => {}
            (Some(a), Some(b)) => a.diff(b, at, out),
            _ => set(at, self.to_value(), out),
        }
    }

    fn to_value(&self) -> Value {
        match self {
            Some(v) => v.to_value(),
            None => Value::None,
        }
    }

    fn from_value(v: &Value) -> Result<Self, ValueError> {
        match v {
            Value::None => Ok(None),
            other => T::from_value(other).map(Some),
        }
    }
}

/// A list of elements with unique keys, in display order. Updates are per element: the diff emits `Insert`,
/// `Remove`, `Move` for the order and `Set` for fields of elements that stay.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyedList<T> {
    items: Vec<T>,
}

impl<T> Default for KeyedList<T> {
    fn default() -> Self {
        KeyedList { items: Vec::new() }
    }
}

impl<T> Deref for KeyedList<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.items
    }
}

impl<T: Keyed> KeyedList<T> {
    /// An empty list.
    #[must_use]
    pub fn new() -> Self {
        KeyedList { items: Vec::new() }
    }

    /// From elements in display order.
    ///
    /// # Errors
    /// Two elements have the same key.
    pub fn from_vec(items: Vec<T>) -> Result<Self, ValueError> {
        let mut seen = std::collections::HashSet::with_capacity(items.len());
        for it in &items {
            if !seen.insert(it.key()) {
                return Err(ValueError::DuplicateKey(it.key()));
            }
        }
        Ok(KeyedList { items })
    }

    /// Add at the end.
    ///
    /// # Errors
    /// The key exists already.
    pub fn try_push(&mut self, item: T) -> Result<(), ValueError> {
        let k = item.key();
        if self.items.iter().any(|i| i.key() == k) {
            return Err(ValueError::DuplicateKey(k));
        }
        self.items.push(item);
        Ok(())
    }

    /// The element with a key.
    #[must_use]
    pub fn get(&self, key: Key) -> Option<&T> {
        self.items.iter().find(|i| i.key() == key)
    }

    /// The keys in list order.
    pub fn keys(&self) -> impl Iterator<Item = Key> + '_ {
        self.items.iter().map(Keyed::key)
    }

    /// The elements as a vector.
    #[must_use]
    pub fn into_vec(self) -> Vec<T> {
        self.items
    }
}

impl<T: Model + Keyed> Model for KeyedList<T> {
    fn diff(&self, old: &Self, at: &mut Cursor, out: &mut Ops) {
        let same_shape = self.items.len() == old.items.len()
            && self
                .items
                .iter()
                .zip(&old.items)
                .all(|(a, b)| a.key() == b.key());
        if !same_shape {
            let ok: Vec<Key> = old.keys().collect();
            let nk: Vec<Key> = self.keys().collect();
            for step in list_steps(&ok, &nk) {
                out.push(match step {
                    ListStep::Remove(key) => Op::Remove {
                        list: at.field,
                        keys: at.keys,
                        key,
                    },
                    ListStep::Insert {
                        at: pos,
                        key,
                        new_index,
                    } => Op::Insert {
                        list: at.field,
                        keys: at.keys,
                        at: pos,
                        key,
                        value: self.items[new_index].to_value(),
                    },
                    ListStep::Move { key, to } => Op::Move {
                        list: at.field,
                        keys: at.keys,
                        key,
                        to,
                    },
                });
            }
        }
        if same_shape {
            for (a, b) in self.items.iter().zip(&old.items) {
                element(a, b, at, out);
            }
        } else {
            let by_key: HashMap<Key, &T> = old.items.iter().map(|i| (i.key(), i)).collect();
            for a in &self.items {
                if let Some(b) = by_key.get(&a.key()) {
                    element(a, b, at, out);
                }
            }
        }
    }

    fn to_value(&self) -> Value {
        Value::List(self.items.iter().map(|i| (i.key(), i.to_value())).collect())
    }

    fn from_value(v: &Value) -> Result<Self, ValueError> {
        let Value::List(items) = v else {
            return Err(ValueError::mismatch("list", v));
        };
        let mut out = Vec::with_capacity(items.len());
        for (k, ev) in items {
            let item = T::from_value(ev)?;
            if item.key() != *k {
                return Err(ValueError::KeyMismatch {
                    listed: *k,
                    actual: item.key(),
                });
            }
            out.push(item);
        }
        KeyedList::from_vec(out)
    }
}

/// Why a root given as a dynamic value could not be written.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum PutError {
    /// The value does not have the shape of the root.
    #[error("{0}")]
    Value(ValueError),
    /// The store refused the write (wrong owner, unknown root).
    #[error("{0}")]
    Store(crate::store::StoreError),
}

/// Convert a dynamic value to the typed root `R` and write it. Used by the generated `put_root` dispatch.
///
/// # Errors
/// The value has the wrong shape, or the writer does not own the root.
pub fn put_value<R: Root>(
    w: &mut crate::store::ModelWriter<'_>,
    v: &Value,
) -> Result<(), PutError> {
    let typed = R::from_value(v).map_err(PutError::Value)?;
    w.try_put(typed).map_err(PutError::Store)
}

fn element<T: Model + Keyed>(a: &T, b: &T, at: &mut Cursor, out: &mut Ops) {
    let keys = at.keys;
    at.keys.push(a.key());
    a.diff(b, at, out);
    at.keys = keys;
}
