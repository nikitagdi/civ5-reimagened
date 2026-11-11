//! A model as plain values, driven by deltas. The store keeps one as the mirror of its typed roots; a consumer
//! that only receives deltas (a test, a remote client) keeps one as its model.

use thiserror::Error;

use crate::delta::Op;
use crate::schema::{FieldId, PathEntry, Root as RootDef, RootId, RootShape, Schema, Type, TypeId};
use crate::value::{Key, Value, ValueError, default_value, validate};

/// A delta that does not fit the model.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ApplyError {
    #[error("field id {0} is not in the schema")]
    UnknownField(u32),
    #[error("root id {0} is not in the schema")]
    UnknownRoot(u16),
    #[error("{path}: {msg}")]
    Shape { path: String, msg: &'static str },
    #[error("{path}: no list element with key {key}")]
    NoElement { path: String, key: Key },
    #[error("{path}: list key {key} exists already")]
    DuplicateKey { path: String, key: Key },
    #[error("{path}: position {pos} is past the end of the list ({len} elements)")]
    Position { path: String, pos: u32, len: usize },
    #[error("{path}: {source}")]
    Value { path: String, source: ValueError },
}

/// Why a walk down a path failed.
enum Walk {
    Shape(&'static str),
    NoElement(Key),
}

/// The values of all roots of a schema.
#[derive(Clone, Debug)]
pub struct ModelState {
    schema: &'static Schema,
    roots: Vec<Value>,
}

impl ModelState {
    /// All roots at their default values.
    #[must_use]
    pub fn new(schema: &'static Schema) -> ModelState {
        let roots = schema
            .roots()
            .iter()
            .map(|r| default_value(schema, &r.value_type()))
            .collect();
        ModelState { schema, roots }
    }

    /// The schema.
    #[must_use]
    pub fn schema(&self) -> &'static Schema {
        self.schema
    }

    /// The value of a root.
    ///
    /// # Panics
    /// If `id` is not from the schema.
    #[must_use]
    pub fn root(&self, id: RootId) -> &Value {
        &self.roots[usize::from(id.0)]
    }

    pub(crate) fn set_root(&mut self, id: RootId, value: Value) {
        self.roots[usize::from(id.0)] = value;
    }

    /// Apply the operations of a frame, all or nothing.
    ///
    /// # Errors
    /// The first operation that does not fit (unknown field, wrong shape, missing element, bad value). The
    /// state is left as it was.
    pub fn apply(&mut self, ops: &[Op]) -> Result<(), ApplyError> {
        let mut touched: Vec<usize> = Vec::new();
        for op in ops {
            let root = match op {
                Op::Reset { root, .. } => usize::from(root.0),
                Op::Set { field, .. } => self.entry(*field)?.root.0.into(),
                Op::Insert { list: f, .. }
                | Op::Remove { list: f, .. }
                | Op::Move { list: f, .. } => self.entry(*f)?.root.0.into(),
            };
            if root >= self.roots.len() {
                return Err(ApplyError::UnknownRoot(root as u16));
            }
            if !touched.contains(&root) {
                touched.push(root);
            }
        }
        let backup: Vec<(usize, Value)> = touched
            .iter()
            .map(|&r| (r, self.roots[r].clone()))
            .collect();
        let result = self.apply_in_place(ops);
        if result.is_err() {
            for (r, v) in backup {
                self.roots[r] = v;
            }
        }
        result
    }

    /// Apply the operations one by one; on an error the earlier ones stay applied.
    pub(crate) fn apply_in_place(&mut self, ops: &[Op]) -> Result<(), ApplyError> {
        for op in ops {
            self.apply_one(op)?;
        }
        Ok(())
    }

    fn entry(&self, f: FieldId) -> Result<&'static PathEntry, ApplyError> {
        self.schema
            .paths()
            .get(f.0 as usize)
            .ok_or(ApplyError::UnknownField(f.0))
    }

    fn apply_one(&mut self, op: &Op) -> Result<(), ApplyError> {
        let schema = self.schema;
        let root = match op {
            Op::Reset { root, .. } => usize::from(root.0),
            Op::Set { field: f, .. }
            | Op::Insert { list: f, .. }
            | Op::Remove { list: f, .. }
            | Op::Move { list: f, .. } => usize::from(self.entry(*f)?.root.0),
        };
        let slot = self
            .roots
            .get_mut(root)
            .ok_or(ApplyError::UnknownRoot(root as u16))?;
        apply_op(schema, slot, op)
    }

    /// The value of a field. `keys` are the keys of the lists that enclose the field, outermost first. `None`
    /// if the field, an element or an optional on the way is not there.
    #[must_use]
    pub fn value(&self, field: FieldId, keys: &[Key]) -> Option<&Value> {
        let e = self.schema.paths().get(field.0 as usize)?;
        read_at(self.schema, &self.roots[usize::from(e.root.0)], e, keys)
    }

    /// The keys of a list, in list order.
    #[must_use]
    pub fn list_keys(
        &self,
        field: FieldId,
        keys: &[Key],
    ) -> Option<impl Iterator<Item = Key> + '_> {
        match self.value(field, keys)? {
            Value::List(items) => Some(items.iter().map(|(k, _)| *k)),
            _ => None,
        }
    }
}

fn walk_err(e: &PathEntry, w: Walk) -> ApplyError {
    match w {
        Walk::Shape(msg) => ApplyError::Shape {
            path: e.path.clone(),
            msg,
        },
        Walk::NoElement(key) => ApplyError::NoElement {
            path: e.path.clone(),
            key,
        },
    }
}

fn entry_of(schema: &Schema, f: FieldId) -> Result<&PathEntry, ApplyError> {
    schema
        .paths()
        .get(f.0 as usize)
        .ok_or(ApplyError::UnknownField(f.0))
}

/// The value of the field `e` inside the value of its root; `None` if an element or an optional on the way is
/// not there.
pub(crate) fn read_at<'a>(
    schema: &Schema,
    root: &'a Value,
    e: &PathEntry,
    keys: &[Key],
) -> Option<&'a Value> {
    if e.steps.is_empty() {
        return keys.is_empty().then_some(root);
    }
    slot_ref(schema, root, schema.root(e.root), &e.steps, keys).ok()
}

/// The list the field `e` is, inside the value of its root.
fn list_slot<'a>(
    schema: &Schema,
    root: &'a mut Value,
    e: &PathEntry,
    keys: &[Key],
) -> Result<&'a mut Vec<(Key, Value)>, ApplyError> {
    // The list at the top of a list root is the root value itself.
    let slot = if e.steps.is_empty() {
        if !keys.is_empty() {
            return Err(ApplyError::Shape {
                path: e.path.clone(),
                msg: "too many keys for the path",
            });
        }
        root
    } else {
        slot_mut(schema, root, schema.root(e.root), &e.steps, keys).map_err(|w| walk_err(e, w))?
    };
    match slot {
        Value::List(items) => Ok(items),
        _ => Err(ApplyError::Shape {
            path: e.path.clone(),
            msg: "the field is not a list",
        }),
    }
}

/// Apply one operation to the value of the root it belongs to.
pub(crate) fn apply_op(schema: &Schema, root: &mut Value, op: &Op) -> Result<(), ApplyError> {
    match op {
        Op::Reset { root: rid, value } => {
            let r = schema
                .roots()
                .get(usize::from(rid.0))
                .ok_or(ApplyError::UnknownRoot(rid.0))?;
            validate(schema, &r.value_type(), value).map_err(|source| ApplyError::Value {
                path: r.name.clone(),
                source,
            })?;
            *root = value.clone();
            Ok(())
        }
        Op::Set { field, keys, value } => {
            let e = entry_of(schema, *field)?;
            validate(schema, &e.ty, value).map_err(|source| ApplyError::Value {
                path: e.path.clone(),
                source,
            })?;
            if e.steps.is_empty() {
                // The whole root (an optional root appearing or going away).
                if !keys.is_empty() {
                    return Err(ApplyError::Shape {
                        path: e.path.clone(),
                        msg: "too many keys for the path",
                    });
                }
                *root = value.clone();
                return Ok(());
            }
            let slot = slot_mut(schema, root, schema.root(e.root), &e.steps, keys)
                .map_err(|w| walk_err(e, w))?;
            *slot = value.clone();
            Ok(())
        }
        Op::Insert {
            list,
            keys,
            at,
            key,
            value,
        } => {
            let e = entry_of(schema, *list)?;
            let l = list_type(e)?;
            validate(schema, &Type::Struct(l.elem), value).map_err(|source| ApplyError::Value {
                path: e.path.clone(),
                source,
            })?;
            if let Value::Struct(fs) = value
                && fs[l.key_index].as_key() != Some(*key)
            {
                return Err(ApplyError::Value {
                    path: e.path.clone(),
                    source: ValueError::KeyMismatch {
                        listed: *key,
                        actual: fs[l.key_index].as_key().unwrap_or(Key(0)),
                    },
                });
            }
            let items = list_slot(schema, root, e, keys)?;
            if items.iter().any(|(k, _)| k == key) {
                return Err(ApplyError::DuplicateKey {
                    path: e.path.clone(),
                    key: *key,
                });
            }
            if *at as usize > items.len() {
                return Err(ApplyError::Position {
                    path: e.path.clone(),
                    pos: *at,
                    len: items.len(),
                });
            }
            items.insert(*at as usize, (*key, value.clone()));
            Ok(())
        }
        Op::Remove { list, keys, key } => {
            let e = entry_of(schema, *list)?;
            let items = list_slot(schema, root, e, keys)?;
            let i =
                items
                    .iter()
                    .position(|(k, _)| k == key)
                    .ok_or_else(|| ApplyError::NoElement {
                        path: e.path.clone(),
                        key: *key,
                    })?;
            items.remove(i);
            Ok(())
        }
        Op::Move {
            list,
            keys,
            key,
            to,
        } => {
            let e = entry_of(schema, *list)?;
            let items = list_slot(schema, root, e, keys)?;
            let i =
                items
                    .iter()
                    .position(|(k, _)| k == key)
                    .ok_or_else(|| ApplyError::NoElement {
                        path: e.path.clone(),
                        key: *key,
                    })?;
            if *to as usize >= items.len() {
                return Err(ApplyError::Position {
                    path: e.path.clone(),
                    pos: *to,
                    len: items.len(),
                });
            }
            let item = items.remove(i);
            items.insert(*to as usize, item);
            Ok(())
        }
    }
}

/// A write by path, as a writer makes it.
#[derive(Clone, Debug)]
pub(crate) enum Edit {
    /// Replace the value of the field.
    Set(Value),
    /// Replace the element with the key (keeping its place) or add it at the end.
    Put(Key, Value),
    /// Remove the element with the key.
    Remove(Key),
    /// Remove every element.
    Clear,
}

/// Apply a write by path to the value of the root the field belongs to; nothing changes on an error.
pub(crate) fn edit_root(
    schema: &Schema,
    root: &mut Value,
    field: FieldId,
    keys: &[Key],
    edit: Edit,
) -> Result<(), ApplyError> {
    let e = entry_of(schema, field)?;
    match edit {
        Edit::Set(value) => apply_op(
            schema,
            root,
            &Op::Set {
                field,
                keys: crate::delta::Keys::from_slice(keys).ok_or_else(|| ApplyError::Shape {
                    path: e.path.clone(),
                    msg: "too many keys for the path",
                })?,
                value,
            },
        ),
        Edit::Put(key, value) => {
            let l = list_type(e)?;
            validate(schema, &Type::Struct(l.elem), &value).map_err(|source| {
                ApplyError::Value {
                    path: e.path.clone(),
                    source,
                }
            })?;
            let actual = match &value {
                Value::Struct(fs) => fs[l.key_index].as_key().unwrap_or(Key(0)),
                _ => Key(0),
            };
            if actual != key {
                return Err(ApplyError::Value {
                    path: e.path.clone(),
                    source: ValueError::KeyMismatch {
                        listed: key,
                        actual,
                    },
                });
            }
            let items = list_slot(schema, root, e, keys)?;
            match items.iter_mut().find(|(k, _)| *k == key) {
                Some(slot) => slot.1 = value,
                None => items.push((key, value)),
            }
            Ok(())
        }
        Edit::Remove(key) => {
            list_type(e)?;
            let items = list_slot(schema, root, e, keys)?;
            let i =
                items
                    .iter()
                    .position(|(k, _)| *k == key)
                    .ok_or_else(|| ApplyError::NoElement {
                        path: e.path.clone(),
                        key,
                    })?;
            items.remove(i);
            Ok(())
        }
        Edit::Clear => {
            list_type(e)?;
            list_slot(schema, root, e, keys)?.clear();
            Ok(())
        }
    }
}

fn list_type(e: &PathEntry) -> Result<&crate::schema::ListType, ApplyError> {
    match &e.ty {
        Type::List(l) => Ok(l),
        _ => Err(ApplyError::Shape {
            path: e.path.clone(),
            msg: "the field is not a list",
        }),
    }
}

/// The struct and the field a step leads to; `None` for a step that does not exist.
fn next_type(schema: &Schema, ty: TypeId, step: u16) -> Option<&Type> {
    schema
        .type_def(ty)
        .fields
        .get(usize::from(step))
        .map(|f| &f.ty)
}

fn slot_mut<'a>(
    schema: &Schema,
    root: &'a mut Value,
    def: &RootDef,
    steps: &[u16],
    keys: &[Key],
) -> Result<&'a mut Value, Walk> {
    let mut cur: &'a mut Value = root;
    let mut ty = def.ty;
    let mut ki = 0;
    if matches!(def.shape, RootShape::List(_)) {
        // The first key picks the element of the list root.
        let key = *keys.first().ok_or(Walk::Shape("missing list key"))?;
        ki = 1;
        let Value::List(items) = cur else {
            return Err(Walk::Shape("expected a list"));
        };
        cur = &mut items
            .iter_mut()
            .find(|(k, _)| *k == key)
            .ok_or(Walk::NoElement(key))?
            .1;
    }
    for (si, &step) in steps.iter().enumerate() {
        let Value::Struct(fields) = cur else {
            return Err(Walk::Shape("expected a struct on the way"));
        };
        let fty = next_type(schema, ty, step).ok_or(Walk::Shape("no such field"))?;
        let next = fields
            .get_mut(usize::from(step))
            .ok_or(Walk::Shape("the struct has fewer fields"))?;
        if si + 1 == steps.len() {
            if ki != keys.len() {
                return Err(Walk::Shape("too many keys for the path"));
            }
            return Ok(next);
        }
        match fty {
            Type::Struct(t) => {
                cur = next;
                ty = *t;
            }
            Type::Option(inner) => match inner.as_ref() {
                Type::Struct(t) if !matches!(next, Value::None) => {
                    cur = next;
                    ty = *t;
                }
                Type::Struct(_) => return Err(Walk::Shape("an optional on the way is absent")),
                _ => return Err(Walk::Shape("a scalar on the way")),
            },
            Type::List(l) => {
                let key = *keys.get(ki).ok_or(Walk::Shape("missing list key"))?;
                ki += 1;
                let Value::List(items) = next else {
                    return Err(Walk::Shape("expected a list"));
                };
                let item = items
                    .iter_mut()
                    .find(|(k, _)| *k == key)
                    .ok_or(Walk::NoElement(key))?;
                cur = &mut item.1;
                ty = l.elem;
            }
            _ => return Err(Walk::Shape("a scalar on the way")),
        }
    }
    Err(Walk::Shape("a root has no slot, use Reset"))
}

fn slot_ref<'a>(
    schema: &Schema,
    root: &'a Value,
    def: &RootDef,
    steps: &[u16],
    keys: &[Key],
) -> Result<&'a Value, Walk> {
    let mut cur: &'a Value = root;
    let mut ty = def.ty;
    let mut ki = 0;
    if matches!(def.shape, RootShape::List(_)) {
        let key = *keys.first().ok_or(Walk::Shape("missing list key"))?;
        ki = 1;
        let Value::List(items) = cur else {
            return Err(Walk::Shape("expected a list"));
        };
        cur = &items
            .iter()
            .find(|(k, _)| *k == key)
            .ok_or(Walk::NoElement(key))?
            .1;
    }
    for (si, &step) in steps.iter().enumerate() {
        let Value::Struct(fields) = cur else {
            return Err(Walk::Shape("expected a struct on the way"));
        };
        let fty = next_type(schema, ty, step).ok_or(Walk::Shape("no such field"))?;
        let next = fields
            .get(usize::from(step))
            .ok_or(Walk::Shape("the struct has fewer fields"))?;
        if si + 1 == steps.len() {
            if ki != keys.len() {
                return Err(Walk::Shape("too many keys for the path"));
            }
            return Ok(next);
        }
        match fty {
            Type::Struct(t) => {
                cur = next;
                ty = *t;
            }
            Type::Option(inner) => match inner.as_ref() {
                Type::Struct(t) if !matches!(next, Value::None) => {
                    cur = next;
                    ty = *t;
                }
                _ => return Err(Walk::Shape("an optional on the way is absent")),
            },
            Type::List(l) => {
                let key = *keys.get(ki).ok_or(Walk::Shape("missing list key"))?;
                ki += 1;
                let Value::List(items) = next else {
                    return Err(Walk::Shape("expected a list"));
                };
                let item = items
                    .iter()
                    .find(|(k, _)| *k == key)
                    .ok_or(Walk::NoElement(key))?;
                cur = &item.1;
                ty = l.elem;
            }
            _ => return Err(Walk::Shape("a scalar on the way")),
        }
    }
    Err(Walk::Shape("a root has no slot"))
}
