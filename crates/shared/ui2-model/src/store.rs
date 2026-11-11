//! The model store: typed roots with change tracking. Sources write whole roots; the store diffs them against
//! the value at the start of the frame and hands the host one [`FrameDelta`] per frame.

use std::any::Any;
use std::sync::Arc;

use thiserror::Error;

use crate::delta::{Cursor, FrameDelta, Op, Ops};
use crate::model::Root;
use crate::schema::{FieldId, RootId, RootKind, Schema};
use crate::state::ModelState;
use crate::value::{Key, Value, ValueRef};
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
}

/// A root of one type, the value at the start of the frame, and whether the consumers have seen it at all.
trait Slot: Send {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    /// The operations since the last frame (a `Reset` if the root was never sent); clears the frame state.
    fn take_ops(&mut self, root: RootId, field: FieldId, out: &mut Ops);
    fn to_value(&self) -> Value;
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
        self.mirror.value(field, keys)
    }

    /// The keys of a list as of the last frame, in list order.
    #[must_use]
    pub fn list_keys(
        &self,
        field: FieldId,
        keys: &[Key],
    ) -> Option<impl Iterator<Item = Key> + '_> {
        self.mirror.list_keys(field, keys)
    }

    /// The revision: grows with every non-empty frame.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The changes since the previous call, or `None` if nothing changed (constant time, no allocation: the
    /// caller does no work for the frame).
    pub fn take_frame(&mut self) -> Option<FrameDelta> {
        if self.dirty.is_empty() {
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
        if ops.is_empty() {
            return None;
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
        self.revision += 1;
        Some(FrameDelta {
            revision: self.revision,
            ops,
        })
    }

    /// Every root as a `Reset`, as of the last frame: for a new skin or a restarted script worker.
    #[must_use]
    pub fn snapshot(&self) -> FrameDelta {
        FrameDelta {
            revision: self.revision,
            ops: self
                .schema
                .roots()
                .iter()
                .enumerate()
                .map(|(i, _)| Op::Reset {
                    root: RootId(i as u16),
                    value: self.mirror.root(RootId(i as u16)).clone(),
                })
                .collect(),
        }
    }

    /// The UMD1 text of a snapshot of the roots a watch set touches, with the schema hash as the first op.
    #[must_use]
    pub fn snapshot_umd1(&self, watch: &WatchSet) -> String {
        let mut out = format!("[{},[5,\"{:016x}\"]", self.revision, self.schema.hash());
        for (i, r) in self.schema.roots().iter().enumerate() {
            if watch.contains(r.field) {
                out.push(',');
                crate::umd1::op(
                    &mut out,
                    &Op::Reset {
                        root: RootId(i as u16),
                        value: self.mirror.root(RootId(i as u16)).clone(),
                    },
                );
            }
        }
        out.push(']');
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
}
