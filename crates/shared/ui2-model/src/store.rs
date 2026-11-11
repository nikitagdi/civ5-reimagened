//! The model store: typed roots with change tracking. Sources write whole roots; the store diffs them against
//! the value at the start of the frame and hands the host one [`FrameDelta`] per frame.
//!
//! Next to the typed roots of the schema the store holds the *mounted fragments* of the skin and of the mods
//! (`state`, `mods.<id>`, `modstate.<id>`, see [`Fragment`]). Their paths and ids live in the same id space:
//! the fields of a fragment get ids after the last path of the schema (and its root an id after the last root),
//! so a [`FrameDelta`] and the [`ModelStore::value`] lookup treat both alike. An id of a fragment is valid
//! until the fragment is unmounted; mounting again gives new ids.

use std::any::Any;
use std::sync::Arc;

use thiserror::Error;

use crate::delta::{Cursor, FrameDelta, Op, Ops};
use crate::fragment::{Fragment, Mount, Namespace};
use crate::model::Root;
use crate::schema::{FieldId, FieldRef, RootId, RootKind, Schema, SchemaError};
use crate::state::{ApplyError, Edit, ModelState, edit_root, read_at};
use crate::value::{Key, Value, ValueError, ValueRef, default_value};
use crate::watch::WatchSet;

/// Who writes a root. Every root has exactly one owner.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Owner {
    /// Game data roots: the replica adapter, on stage 3 the server.
    Source,
    /// The `ui` root: the actions module.
    Actions,
    /// `state.*` of the active skin.
    Skin,
    /// `mods.<id>.*`: the Rust half of a mod.
    Mod(Arc<str>),
    /// `modstate.<id>.*`: the script and markup of a mod.
    ModScript(Arc<str>),
}

/// A write the store refused.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum StoreError {
    #[error("root `{0}` is not in the schema")]
    UnknownRoot(&'static str),
    #[error("root `{root}` belongs to {expected:?}, not {writer:?}")]
    WrongOwner {
        root: &'static str,
        writer: Owner,
        expected: Owner,
    },
    #[error("root `{0}` was stored with another Rust type")]
    TypeMismatch(&'static str),
    /// A write by path to a place the writer does not own: another mod's namespace, a data root.
    #[error("`{path}` belongs to {expected:?}, not {writer:?}")]
    PathOwner {
        path: String,
        writer: Owner,
        expected: Owner,
    },
    #[error("namespace `{0}` is not mounted")]
    NotMounted(String),
    #[error("root `{0}` has no value yet: write the whole root first")]
    Unwritten(String),
    #[error(transparent)]
    Path(#[from] SchemaError),
    #[error(transparent)]
    Apply(#[from] ApplyError),
    #[error("{path}: {source}")]
    Value { path: String, source: ValueError },
}

/// A root of one type, the value at the start of the frame, and whether the consumers have seen it at all.
trait Slot: Send {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    /// The operations since the last frame (a `Reset` if the root was never sent); clears the frame state.
    fn take_ops(&mut self, root: RootId, field: FieldId, out: &mut Ops);
    fn to_value(&self) -> Value;
    /// Write one field or list by path, through the dynamic value of the root.
    fn edit(
        &mut self,
        schema: &Schema,
        field: FieldId,
        keys: &[Key],
        edit: Edit,
    ) -> Result<(), StoreError>;
}

struct TypedSlot<R> {
    cur: R,
    /// The value at the start of the frame, set by the first `put` of the frame.
    base: Option<R>,
    /// Consumers have received this root (as a `Reset`) at least once.
    sent: bool,
}

impl<R: Root> Slot for TypedSlot<R> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn take_ops(&mut self, root: RootId, field: FieldId, out: &mut Ops) {
        let base = self.base.take();
        if !self.sent {
            self.sent = true;
            out.push(Op::Reset {
                root,
                value: self.cur.to_value(),
            });
        } else if let Some(base) = base {
            let mut at = Cursor::at(field);
            self.cur.diff(&base, &mut at, out);
        }
    }

    fn to_value(&self) -> Value {
        self.cur.to_value()
    }

    fn edit(
        &mut self,
        schema: &Schema,
        field: FieldId,
        keys: &[Key],
        edit: Edit,
    ) -> Result<(), StoreError> {
        let mut v = self.cur.to_value();
        edit_root(schema, &mut v, field, keys, edit)?;
        let new = R::from_value(&v).map_err(|source| StoreError::Value {
            path: schema.path(field).path.clone(),
            source,
        })?;
        let old = std::mem::replace(&mut self.cur, new);
        if self.base.is_none() {
            self.base = Some(old);
        }
        Ok(())
    }
}

/// The model: typed roots, a plain-value mirror for dynamic reads, the revision and the roots changed in the
/// current frame. Owned by the host; skins get `&ModelStore` only.
pub struct ModelStore {
    schema: &'static Schema,
    slots: Vec<Option<Box<dyn Slot>>>,
    /// The state as of the last `take_frame`.
    mirror: ModelState,
    dirty: Vec<RootId>,
    revision: u64,
    /// Mounted fragments, by ascending `field_base`.
    mounts: Vec<Mount>,
    /// A mounted fragment was written or mounted since the last frame.
    mounts_dirty: bool,
    /// `Reset`s of fragments unmounted since the last frame.
    retired: Ops,
    /// The next free store-wide field and root ids (they only grow, so an id is never reused).
    next_field: u32,
    next_root: u32,
}

impl ModelStore {
    /// An empty store: every root at its default value, nothing sent yet.
    #[must_use]
    pub fn new(schema: &'static Schema) -> ModelStore {
        ModelStore {
            schema,
            slots: (0..schema.roots().len()).map(|_| None).collect(),
            mirror: ModelState::new(schema),
            dirty: Vec::new(),
            revision: 0,
            mounts: Vec::new(),
            mounts_dirty: false,
            retired: Ops::new(),
            next_field: schema.paths().len() as u32,
            next_root: schema.roots().len() as u32,
        }
    }

    /// The schema.
    #[must_use]
    pub fn schema(&self) -> &'static Schema {
        self.schema
    }

    /// A writer that may write the roots of `owner`.
    pub fn writer(&mut self, owner: Owner) -> ModelWriter<'_> {
        ModelWriter { store: self, owner }
    }

    /// Mount a fragment into a namespace: its root starts at the default values and reaches the consumers as
    /// a `Reset` in the next frame.
    ///
    /// # Errors
    /// The namespace is mounted already, the id of a mod is not a valid one, the store ran out of ids, or the
    /// fragment does not lay out under the namespace's names.
    pub fn mount(&mut self, ns: Namespace, fragment: &Fragment) -> Result<(), SchemaError> {
        ns.check()?;
        if self.mounts.iter().any(|m| m.ns == ns) {
            return Err(SchemaError::Path {
                path: ns.root_name(),
                msg: "the namespace is mounted already; unmount it first".to_string(),
            });
        }
        let schema = fragment.schema_for(&ns)?;
        let too_many = || SchemaError::Path {
            path: ns.root_name(),
            msg: "too many mounts in this store".to_string(),
        };
        let root = u16::try_from(self.next_root).map_err(|_| too_many())?;
        let fields = u32::try_from(schema.paths().len()).map_err(|_| too_many())?;
        let next_field = self.next_field.checked_add(fields).ok_or_else(too_many)?;
        let cur = default_value(schema, &schema.root_type(RootId(0)));
        self.mounts.push(Mount {
            ns,
            schema,
            field_base: self.next_field,
            root: RootId(root),
            cur,
            base: None,
            sent: false,
        });
        self.next_field = next_field;
        self.next_root += 1;
        self.mounts_dirty = true;
        Ok(())
    }

    /// Unmount a fragment. The consumers get a `Reset` of its root to the default values in the next frame
    /// (if they ever saw it); its ids are retired. `false` if the namespace was not mounted.
    pub fn unmount(&mut self, ns: &Namespace) -> bool {
        let Some(i) = self.mounts.iter().position(|m| m.ns == *ns) else {
            return false;
        };
        let m = self.mounts.remove(i);
        if m.sent {
            self.retired.push(Op::Reset {
                root: m.root,
                value: default_value(m.schema, &m.schema.root_type(RootId(0))),
            });
        }
        true
    }

    /// Whether a namespace is mounted.
    #[must_use]
    pub fn is_mounted(&self, ns: &Namespace) -> bool {
        self.mounts.iter().any(|m| m.ns == *ns)
    }

    /// The schema of a mounted fragment (its own, with paths written as `mods.<id>.<field>`); the types of
    /// [`FieldRef`]s of the fragment refer to it.
    #[must_use]
    pub fn mount_schema(&self, ns: &Namespace) -> Option<&'static Schema> {
        self.mounts.iter().find(|m| m.ns == *ns).map(|m| m.schema)
    }

    /// Resolve a path of the schema or of a mounted fragment to its store-wide id.
    ///
    /// # Errors
    /// The path is not in the schema, its namespace is not mounted, or the fragment has no such field.
    pub fn resolve(&self, path: &str) -> Result<FieldRef, SchemaError> {
        let Some(ns) = Namespace::of_path(path) else {
            return self.schema.resolve(path);
        };
        let m = self
            .mounts
            .iter()
            .find(|m| m.ns == ns)
            .ok_or_else(|| SchemaError::Path {
                path: path.to_string(),
                msg: format!("namespace `{}` is not mounted", ns.root_name()),
            })?;
        let mut r = m.schema.resolve(path)?;
        r.id = FieldId(r.id.0 + m.field_base);
        Ok(r)
    }

    /// The mount that owns a store-wide field id, and the id inside its schema.
    fn mount_of(&self, field: FieldId) -> Option<(&Mount, FieldId)> {
        let i = self.mounts.partition_point(|m| m.fields().end <= field.0);
        let m = self.mounts.get(i)?;
        m.fields()
            .contains(&field.0)
            .then(|| (m, FieldId(field.0 - m.field_base)))
    }

    /// The typed root, as last written (including writes of the current frame). `None` if the root was never
    /// written.
    #[must_use]
    pub fn get<R: Root>(&self) -> Option<&R> {
        let rid = self.schema.root_by_name(R::NAME)?;
        let slot = self.slots[usize::from(rid.0)].as_ref()?;
        slot.as_any().downcast_ref::<TypedSlot<R>>().map(|s| &s.cur)
    }

    /// The value of a field as of the last [`take_frame`](Self::take_frame). `keys` are the keys of the lists
    /// that enclose the field, outermost first.
    #[must_use]
    pub fn value(&self, field: FieldId, keys: &[Key]) -> Option<ValueRef<'_>> {
        if (field.0 as usize) < self.schema.paths().len() {
            return self.mirror.value(field, keys);
        }
        let (m, local) = self.mount_of(field)?;
        read_at(m.schema, m.frame_value(), m.schema.path(local), keys)
    }

    /// The keys of a list as of the last frame, in list order.
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

    /// The revision: grows with every non-empty frame.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The changes since the previous call, or `None` if nothing changed (constant time, no allocation: the
    /// caller does no work for the frame).
    pub fn take_frame(&mut self) -> Option<FrameDelta> {
        if self.dirty.is_empty() && !self.mounts_dirty && self.retired.is_empty() {
            return None;
        }
        let mut dirty = std::mem::take(&mut self.dirty);
        dirty.sort_unstable();
        let mut ops = Ops::new();
        for &rid in &dirty {
            let field = self.schema.root(rid).field;
            if let Some(slot) = self.slots[usize::from(rid.0)].as_mut() {
                slot.take_ops(rid, field, &mut ops);
            }
        }
        if self.mirror.apply_in_place(&ops).is_err() {
            // Cannot happen for ops built by the typed diff; if it does, rebuild the mirror from the roots.
            debug_assert!(false, "typed diff produced ops the mirror refuses");
            for &rid in &dirty {
                if let Some(slot) = self.slots[usize::from(rid.0)].as_ref() {
                    self.mirror.set_root(rid, slot.to_value());
                }
            }
        }
        // The fragments: the roots that went away, then the ones that changed.
        ops.append(&mut self.retired);
        if std::mem::take(&mut self.mounts_dirty) {
            for m in &mut self.mounts {
                m.take_ops(&mut ops);
            }
        }
        if ops.is_empty() {
            return None;
        }
        self.revision += 1;
        Some(FrameDelta {
            revision: self.revision,
            ops,
        })
    }

    /// Every root as a `Reset`, as of the last frame: for a new skin or a restarted script worker. The
    /// mounted fragments the consumers have seen are included.
    #[must_use]
    pub fn snapshot(&self) -> FrameDelta {
        let mut ops: Ops = self
            .schema
            .roots()
            .iter()
            .enumerate()
            .map(|(i, _)| Op::Reset {
                root: RootId(i as u16),
                value: self.mirror.root(RootId(i as u16)).clone(),
            })
            .collect();
        ops.extend(self.mounts.iter().filter(|m| m.sent).map(|m| Op::Reset {
            root: m.root,
            value: m.frame_value().clone(),
        }));
        FrameDelta {
            revision: self.revision,
            ops,
        }
    }

    /// The UMD1 text of a snapshot of the roots a watch set touches, with the schema hash as the first op.
    #[must_use]
    pub fn snapshot_umd1(&self, watch: &WatchSet) -> String {
        use std::fmt::Write as _;

        let mut out = String::new();
        let _ = write!(
            out,
            "[{},[5,\"{:016x}\"]",
            self.revision,
            self.schema.hash()
        );
        let mut reset = |root: RootId, value: &Value| {
            out.push(',');
            crate::umd1::op(
                &mut out,
                &Op::Reset {
                    root,
                    value: value.clone(),
                },
            );
        };
        for (i, r) in self.schema.roots().iter().enumerate() {
            if watch.contains(r.field) {
                reset(RootId(i as u16), self.mirror.root(RootId(i as u16)));
            }
        }
        for m in self.mounts.iter().filter(|m| m.sent) {
            if watch.contains(FieldId(m.field_base)) {
                reset(m.root, m.frame_value());
            }
        }
        out.push(']');
        out
    }

    /// The schema that holds a store-wide field id and the id of its first path slot, for building a
    /// [`WatchSet`].
    pub(crate) fn locate(&self, field: FieldId) -> Option<(&'static Schema, u32)> {
        if (field.0 as usize) < self.schema.paths().len() {
            return Some((self.schema, 0));
        }
        self.mount_of(field).map(|(m, _)| (m.schema, m.field_base))
    }

    /// The number of field ids in use (the schema's paths and every fragment ever mounted).
    pub(crate) fn field_ids(&self) -> u32 {
        self.next_field
    }

    /// The path slot of every root id in use: the schema's roots, then the fragments' (`None` for one that
    /// was unmounted).
    pub(crate) fn root_fields(&self) -> Vec<Option<FieldId>> {
        let mut out: Vec<Option<FieldId>> =
            self.schema.roots().iter().map(|r| Some(r.field)).collect();
        out.resize(self.next_root as usize, None);
        for m in &self.mounts {
            out[usize::from(m.root.0)] = Some(FieldId(m.field_base));
        }
        out
    }
}

/// Write access for one owner. Only the host, which owns the `&mut ModelStore`, can make one.
pub struct ModelWriter<'a> {
    store: &'a mut ModelStore,
    owner: Owner,
}

impl ModelWriter<'_> {
    /// Replace a root. The change is diffed against the value at the start of the frame when the host takes
    /// the frame, so several writes in one frame merge into one delta (and a write that goes back gives none).
    ///
    /// # Panics
    /// In a debug build, if the root belongs to another owner (a programming error).
    ///
    /// # Errors
    /// The root is unknown, belongs to another owner (release builds), or holds another type.
    pub fn put<R: Root>(&mut self, value: R) -> Result<(), StoreError> {
        let r = self.try_put(value);
        if cfg!(debug_assertions)
            && let Err(StoreError::WrongOwner { .. }) = &r
        {
            panic!(
                "{}",
                r.as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_default()
            );
        }
        r
    }

    /// Like [`put`](Self::put) but a wrong owner is an error in every build.
    ///
    /// # Errors
    /// See [`put`](Self::put).
    pub fn try_put<R: Root>(&mut self, value: R) -> Result<(), StoreError> {
        let schema = self.store.schema;
        let rid = schema
            .root_by_name(R::NAME)
            .ok_or(StoreError::UnknownRoot(R::NAME))?;
        let expected = match schema.root(rid).kind {
            RootKind::Data => Owner::Source,
            RootKind::Ui => Owner::Actions,
        };
        if self.owner != expected {
            return Err(StoreError::WrongOwner {
                root: R::NAME,
                writer: self.owner.clone(),
                expected,
            });
        }
        let slot = &mut self.store.slots[usize::from(rid.0)];
        match slot {
            None => {
                *slot = Some(Box::new(TypedSlot {
                    cur: value,
                    base: None,
                    sent: false,
                }));
            }
            Some(s) => {
                let s = s
                    .as_any_mut()
                    .downcast_mut::<TypedSlot<R>>()
                    .ok_or(StoreError::TypeMismatch(R::NAME))?;
                let old = std::mem::replace(&mut s.cur, value);
                if s.base.is_none() {
                    s.base = Some(old);
                }
            }
        }
        if !self.store.dirty.contains(&rid) {
            self.store.dirty.push(rid);
        }
        Ok(())
    }

    /// Set the field at `path` (`mods.hello.count`, `state.prod.sort`, `ui.windows.menu.open`) to a value. The
    /// path must not run through a list (see [`set_value_at`](Self::set_value_at)).
    ///
    /// # Errors
    /// The path is not in the schema or a mounted fragment, is not owned by this writer, or the value does not
    /// have the type of the field; nothing is written then.
    pub fn set_value(&mut self, path: &str, value: Value) -> Result<(), StoreError> {
        self.edit(path, &[], Edit::Set(value))
    }

    /// Like [`set_value`](Self::set_value) for a field inside list elements: `keys` are the keys of the
    /// enclosing lists, outermost first.
    ///
    /// # Errors
    /// See [`set_value`](Self::set_value); also a key with no element.
    pub fn set_value_at(
        &mut self,
        path: &str,
        keys: &[Key],
        value: Value,
    ) -> Result<(), StoreError> {
        self.edit(path, keys, Edit::Set(value))
    }

    /// Put an element into the list at `path`: replace the element with this key (it keeps its place) or add it
    /// at the end. `value` is the whole element, with the key in its key field.
    ///
    /// # Errors
    /// The path is not a list of a mounted fragment or of the schema, is not owned by this writer, or the
    /// element does not have the type of the list or a different key.
    pub fn list_put(&mut self, path: &str, key: Key, value: Value) -> Result<(), StoreError> {
        self.edit(path, &[], Edit::Put(key, value))
    }

    /// [`list_put`](Self::list_put) for a list inside list elements.
    ///
    /// # Errors
    /// See [`list_put`](Self::list_put).
    pub fn list_put_at(
        &mut self,
        path: &str,
        keys: &[Key],
        key: Key,
        value: Value,
    ) -> Result<(), StoreError> {
        self.edit(path, keys, Edit::Put(key, value))
    }

    /// Remove the element with this key from the list at `path`.
    ///
    /// # Errors
    /// As for [`list_put`](Self::list_put); also there is no such element.
    pub fn list_remove(&mut self, path: &str, key: Key) -> Result<(), StoreError> {
        self.edit(path, &[], Edit::Remove(key))
    }

    /// [`list_remove`](Self::list_remove) for a list inside list elements.
    ///
    /// # Errors
    /// See [`list_remove`](Self::list_remove).
    pub fn list_remove_at(&mut self, path: &str, keys: &[Key], key: Key) -> Result<(), StoreError> {
        self.edit(path, keys, Edit::Remove(key))
    }

    /// Remove every element of the list at `path`.
    ///
    /// # Errors
    /// As for [`list_put`](Self::list_put).
    pub fn list_clear(&mut self, path: &str) -> Result<(), StoreError> {
        self.edit(path, &[], Edit::Clear)
    }

    /// [`list_clear`](Self::list_clear) for a list inside list elements.
    ///
    /// # Errors
    /// See [`list_clear`](Self::list_clear).
    pub fn list_clear_at(&mut self, path: &str, keys: &[Key]) -> Result<(), StoreError> {
        self.edit(path, keys, Edit::Clear)
    }

    fn edit(&mut self, path: &str, keys: &[Key], edit: Edit) -> Result<(), StoreError> {
        let store = &mut *self.store;
        if let Some(ns) = Namespace::of_path(path) {
            // A fragment: the namespace must be mounted and be the writer's.
            let expected = ns.owner();
            if self.owner != expected {
                return Err(StoreError::PathOwner {
                    path: path.to_string(),
                    writer: self.owner.clone(),
                    expected,
                });
            }
            let m = store
                .mounts
                .iter_mut()
                .find(|m| m.ns == ns)
                .ok_or_else(|| StoreError::NotMounted(ns.root_name()))?;
            let field = m.schema.resolve(path)?.id;
            // Writes are all or nothing, so the value at the start of the frame is kept only on success.
            let backup = m.base.is_none().then(|| m.cur.clone());
            edit_root(m.schema, &mut m.cur, field, keys, edit)?;
            if m.base.is_none() {
                m.base = backup;
            }
            store.mounts_dirty = true;
            return Ok(());
        }
        let schema = store.schema;
        let field = schema.resolve(path)?.id;
        let rid = schema.path(field).root;
        let expected = match schema.root(rid).kind {
            RootKind::Data => Owner::Source,
            RootKind::Ui => Owner::Actions,
        };
        if self.owner != expected {
            return Err(StoreError::PathOwner {
                path: path.to_string(),
                writer: self.owner.clone(),
                expected,
            });
        }
        let slot = store.slots[usize::from(rid.0)]
            .as_mut()
            .ok_or_else(|| StoreError::Unwritten(schema.root(rid).name.clone()))?;
        slot.edit(schema, field, keys, edit)?;
        if !store.dirty.contains(&rid) {
            store.dirty.push(rid);
        }
        Ok(())
    }
}
