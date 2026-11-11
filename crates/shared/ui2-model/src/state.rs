//! A model as plain values, driven by deltas. The store keeps one as the mirror of its typed roots; a consumer
//! that only receives deltas (a test, a remote client) keeps one as its model.

use thiserror::Error;

use crate::delta::Op;
use crate::schema::{FieldId, PathEntry, RootId, Schema, Type, TypeId};
use crate::value::{Key, Value, ValueError, default_value, validate};

/// A delta that does not fit the model.
#[derive(Clone, Debug, PartialEq, Error)]
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
            .map(|r| default_value(schema, &Type::Struct(r.ty)))
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
        match op {
            Op::Reset { root, value } => {
                let r = schema
                    .roots()
                    .get(usize::from(root.0))
                    .ok_or(ApplyError::UnknownRoot(root.0))?;
                validate(schema, &Type::Struct(r.ty), value).map_err(|source| {
                    ApplyError::Value {
                        path: r.name.clone(),
                        source,
                    }
                })?;
                self.roots[usize::from(root.0)] = value.clone();
                Ok(())
            }
            Op::Set { field, keys, value } => {
                let e = self.entry(*field)?;
                validate(schema, &e.ty, value).map_err(|source| ApplyError::Value {
                    path: e.path.clone(),
                    source,
                })?;
                let slot = self.slot_mut(e, keys)?;
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
                let e = self.entry(*list)?;
                let l = list_type(e)?;
                validate(schema, &Type::Struct(l.elem), value).map_err(|source| {
                    ApplyError::Value {
                        path: e.path.clone(),
                        source,
                    }
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
                let items = self.list_mut(e, keys)?;
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
                let e = self.entry(*list)?;
                let items = self.list_mut(e, keys)?;
                let i = items.iter().position(|(k, _)| k == key).ok_or_else(|| {
                    ApplyError::NoElement {
                        path: e.path.clone(),
                        key: *key,
                    }
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
                let e = self.entry(*list)?;
                let items = self.list_mut(e, keys)?;
                let i = items.iter().position(|(k, _)| k == key).ok_or_else(|| {
                    ApplyError::NoElement {
                        path: e.path.clone(),
                        key: *key,
                    }
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

    fn slot_mut(&mut self, e: &PathEntry, keys: &[Key]) -> Result<&mut Value, ApplyError> {
        let schema = self.schema;
        let root_ty = schema.root(e.root).ty;
        let root = &mut self.roots[usize::from(e.root.0)];
        slot_mut(schema, root, root_ty, &e.steps, keys).map_err(|w| Self::walk_err(e, w))
    }

    fn list_mut(
        &mut self,
        e: &PathEntry,
        keys: &[Key],
    ) -> Result<&mut Vec<(Key, Value)>, ApplyError> {
        match self.slot_mut(e, keys)? {
            Value::List(items) => Ok(items),
            _ => Err(ApplyError::Shape {
                path: e.path.clone(),
                msg: "the field is not a list",
            }),
        }
    }

    /// The value of a field. `keys` are the keys of the lists that enclose the field, outermost first. `None`
    /// if the field, an element or an optional on the way is not there.
    #[must_use]
    pub fn value(&self, field: FieldId, keys: &[Key]) -> Option<&Value> {
        let e = self.schema.paths().get(field.0 as usize)?;
        let root = &self.roots[usize::from(e.root.0)];
        if e.steps.is_empty() {
            return keys.is_empty().then_some(root);
        }
        slot_ref(
            self.schema,
            root,
            self.schema.root(e.root).ty,
            &e.steps,
            keys,
        )
        .ok()
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
    root_ty: TypeId,
    steps: &[u16],
    keys: &[Key],
) -> Result<&'a mut Value, Walk> {
    let mut cur: &'a mut Value = root;
    let mut ty = root_ty;
    let mut ki = 0;
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
    root_ty: TypeId,
    steps: &[u16],
    keys: &[Key],
) -> Result<&'a Value, Walk> {
    let mut cur: &'a Value = root;
    let mut ty = root_ty;
    let mut ki = 0;
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
