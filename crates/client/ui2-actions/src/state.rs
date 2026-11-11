//! Writes to the local state of the UI: `ui.*` (only the fields the schema marks `js`), `state.*` of the skin
//! and `modstate.<id>.*` of a mod, as `StateOp`s from scripts and markup. Also what happens to `state.*` when
//! the skin changes: it is set aside under the skin's id and comes back when that skin is selected again.

use std::sync::Arc;

use ui2_model::generated::Ui;
use ui2_model::schema::{Schema, Type, TypeId};
use ui2_model::wire::StateOp;
use ui2_model::{
    Fragment, ModelStore, Namespace, Owner, RootId, RootShape, StoreError, Value, validate,
};

use crate::UiActions;
use crate::intent::coerce;

/// The id of a skin.
pub type SkinId = Arc<str>;

/// Whose state an op writes: the skin's own, or a mod's. For both, `state.<path>` in an op means their own
/// `state.*` / `modstate.<id>.*`; `ui.<path>` reaches the `js` fields of the `ui` root.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum StateNs {
    Skin,
    Mod(Arc<str>),
}

/// A write the module refused. The ops of a frame are independent: one error does not stop the others.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StateError {
    #[error("{path}: {msg}")]
    UnknownPath { path: String, msg: String },
    #[error("{0}: this path cannot be written (only `state.*` and the `js` fields of `ui.*` can)")]
    NotWritable(String),
    #[error("{path}: another mod's namespace")]
    OutsideNamespace { path: String },
    #[error("{0}: the state is not mounted (no skin or mod state of this kind is active)")]
    NotMounted(String),
    #[error("{path}: {msg}")]
    BadValue { path: String, msg: String },
    #[error("{path}: {source}")]
    Store { path: String, source: StoreError },
    #[error("setting key {0:?}: use 1 to 64 characters of A-Z a-z 0-9 . _ -")]
    BadKey(String),
    #[error("setting {key:?}: {len} bytes is over the 4096 byte limit")]
    ValueTooBig { key: String, len: usize },
    #[error("setting {key:?}: not JSON: {msg}")]
    BadJson { key: String, msg: String },
    #[error("`{0}` has the most settings it may have (64)")]
    TooManyKeys(String),
    #[error("{0:?} cannot be the id of a skin or mod here")]
    BadNamespace(String),
    #[error("mounting the state failed: {0}")]
    Mount(String),
}

/// What a set-aside state belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Keep {
    Skin(SkinId),
    Mod(Arc<str>),
}

/// One value of a set-aside state: the path it had, the name of its type, the value.
#[derive(Clone, Debug)]
pub(crate) struct Leaf {
    path: String,
    ty: String,
    value: Value,
}

impl UiActions {
    /// Apply the writes of scripts and markup to the local state, in order. A write that is refused is
    /// reported in the result (the host logs it) and changes nothing.
    ///
    /// `ui.<path>` must be a `js` field of the `ui` root. `state.<path>` goes to the state of `ns`: the skin's
    /// `state.*`, or `modstate.<id>.*` of the mod. A mod may also write `modstate.<its id>.<path>` by that
    /// name, and nothing of another mod.
    pub fn state_ops(
        &mut self,
        ns: StateNs,
        ops: &[StateOp],
        store: &mut ModelStore,
    ) -> Vec<StateError> {
        let mut errors = Vec::new();
        for op in ops {
            if let Err(e) = self.state_op(&ns, op, store) {
                errors.push(e);
            }
        }
        errors
    }

    fn state_op(
        &mut self,
        ns: &StateNs,
        op: &StateOp,
        store: &mut ModelStore,
    ) -> Result<(), StateError> {
        let path: &str = &op.path;
        let head = path.split(['.', '[']).next().unwrap_or("");
        let bad_path = |e: ui2_model::SchemaError| StateError::UnknownPath {
            path: path.to_string(),
            msg: e.to_string(),
        };
        let bad_value = |msg: String| StateError::BadValue {
            path: path.to_string(),
            msg,
        };
        match head {
            "ui" => {
                let schema = self.schema;
                let field = schema.resolve(path).map_err(bad_path)?;
                if !schema.path(field.id).js {
                    return Err(StateError::NotWritable(path.to_string()));
                }
                let value = coerce(schema, &field.ty, &op.value).map_err(bad_value)?;
                // The module's copy and the store's agree before and after the write.
                self.publish(store);
                store
                    .writer(Owner::Actions)
                    .set_value(path, value)
                    .map_err(|source| StateError::Store {
                        path: path.to_string(),
                        source,
                    })?;
                self.state.resync(store);
                Ok(())
            }
            "state" | "modstate" => {
                let (target, full) = match (ns, head) {
                    (StateNs::Skin, "state") => (Namespace::State, path.to_string()),
                    (StateNs::Mod(id), "state") => (
                        Namespace::ModState(id.clone()),
                        format!("modstate.{id}{}", &path["state".len()..]),
                    ),
                    (StateNs::Mod(id), "modstate")
                        if Namespace::of_path(path) == Some(Namespace::ModState(id.clone())) =>
                    {
                        (Namespace::ModState(id.clone()), path.to_string())
                    }
                    _ => {
                        return Err(StateError::OutsideNamespace {
                            path: path.to_string(),
                        });
                    }
                };
                let schema = store
                    .mount_schema(&target)
                    .ok_or_else(|| StateError::NotMounted(target.root_name()))?;
                let field = store.resolve(&full).map_err(bad_path)?;
                let value = coerce(schema, &field.ty, &op.value).map_err(bad_value)?;
                store
                    .writer(target.owner())
                    .set_value(&full, value)
                    .map_err(|source| StateError::Store {
                        path: path.to_string(),
                        source,
                    })
            }
            _ => Err(StateError::NotWritable(path.to_string())),
        }
    }

    /// Switch the skin: set aside the `state.*` of the skin that was active (under its id), mount the state
    /// fragment of the new one and bring back what was set aside for it, by path and type. A skin that is
    /// selected again finds its state as it left it; the same skin loaded again after a reload keeps every
    /// value whose path and type still exist. `None` leaves no skin: `state.*` is empty. Returns what could
    /// not be brought back or mounted.
    pub fn set_skin(
        &mut self,
        skin: Option<(SkinId, &Fragment)>,
        store: &mut ModelStore,
    ) -> Vec<StateError> {
        self.set_skin_with_defaults(skin, &[], store)
    }

    /// [`set_skin`](Self::set_skin) for a skin whose state starts from values of its own (`defaults`: full
    /// paths `state.<path>` and their values, as the skin's package declares them) rather than the zero values
    /// of its types. The defaults are written after the mount and before what was set aside comes back, so a
    /// value the player changed wins over the default. A default that does not fit its path is reported and
    /// left at zero.
    pub fn set_skin_with_defaults(
        &mut self,
        skin: Option<(SkinId, &Fragment)>,
        defaults: &[(String, Value)],
        store: &mut ModelStore,
    ) -> Vec<StateError> {
        let old = self.active_skin.take().map(Keep::Skin);
        let new = skin.map(|(id, f)| (Keep::Skin(id), f));
        if let Some((Keep::Skin(id), _)) = &new {
            self.active_skin = Some(id.clone());
        }
        let errors = self.remount(old, Namespace::State, new, defaults, store);
        if !errors.is_empty() && !store.is_mounted(&Namespace::State) {
            self.active_skin = None;
        }
        errors
    }

    /// The same for the script state of a mod: `modstate.<id>.*` is set aside when the fragment goes
    /// (`None`: the mod is switched off) and comes back when it is mounted again.
    pub fn set_mod_state(
        &mut self,
        id: &str,
        fragment: Option<&Fragment>,
        store: &mut ModelStore,
    ) -> Vec<StateError> {
        let id: Arc<str> = Arc::from(id);
        let keep = Keep::Mod(id.clone());
        let ns = Namespace::ModState(id);
        let old = store.is_mounted(&ns).then(|| keep.clone());
        self.remount(old, ns, fragment.map(|f| (keep, f)), &[], store)
    }

    fn remount(
        &mut self,
        old: Option<Keep>,
        ns: Namespace,
        new: Option<(Keep, &Fragment)>,
        defaults: &[(String, Value)],
        store: &mut ModelStore,
    ) -> Vec<StateError> {
        let mut errors = Vec::new();
        if let (Some(keep), Some(schema), Some(value)) = (
            old,
            store.mount_schema(&ns),
            store.fragment_value(&ns).cloned(),
        ) {
            self.saved.insert(keep, save_leaves(schema, &value));
        }
        store.unmount(&ns);
        let Some((keep, fragment)) = new else {
            return errors;
        };
        if let Err(e) = store.mount(ns.clone(), fragment) {
            errors.push(StateError::Mount(e.to_string()));
            return errors;
        }
        for (path, value) in defaults {
            if let Err(source) = store.writer(ns.owner()).set_value(path, value.clone()) {
                errors.push(StateError::Store {
                    path: path.clone(),
                    source,
                });
            }
        }
        for leaf in self.saved.remove(&keep).unwrap_or_default() {
            restore_leaf(store, &ns, &leaf);
        }
        errors
    }
}

/// The values of a mounted state that can be carried over: every field that is not a struct (a struct is
/// walked), at its path, with the name of its type. Elements of lists travel inside their list.
fn save_leaves(schema: &Schema, root: &Value) -> Vec<Leaf> {
    fn walk(schema: &Schema, prefix: &str, t: TypeId, v: &Value, out: &mut Vec<Leaf>) {
        let Value::Struct(values) = v else {
            return;
        };
        for (f, fv) in schema.type_def(t).fields.iter().zip(values) {
            let path = format!("{prefix}.{}", f.name);
            match &f.ty {
                Type::Struct(inner) => walk(schema, &path, *inner, fv, out),
                ty => out.push(Leaf {
                    ty: schema.type_name(ty),
                    path,
                    value: fv.clone(),
                }),
            }
        }
    }
    let r = schema.root(RootId(0));
    let mut out = Vec::new();
    match r.shape {
        RootShape::Struct => walk(schema, &r.name, r.ty, root, &mut out),
        _ => {
            let ty = schema.root_type(RootId(0));
            out.push(Leaf {
                path: r.name.clone(),
                ty: schema.type_name(&ty),
                value: root.clone(),
            });
        }
    }
    out
}

/// Put a set-aside value back if its path exists in the new fragment with the same type and the value still
/// fits it.
fn restore_leaf(store: &mut ModelStore, ns: &Namespace, leaf: &Leaf) {
    let Some(schema) = store.mount_schema(ns) else {
        return;
    };
    let Ok(field) = store.resolve(&leaf.path) else {
        return;
    };
    if schema.type_name(&field.ty) != leaf.ty || validate(schema, &field.ty, &leaf.value).is_err() {
        return;
    }
    // Fits by the checks above; a refusal could only mean the namespace is not ours, and it is.
    let _ = store
        .writer(ns.owner())
        .set_value(&leaf.path, leaf.value.clone());
}

impl crate::ui::UiState {
    /// Take the value of the `ui` root from the store, after a write by path went to the store directly.
    pub(crate) fn resync(&mut self, store: &ModelStore) {
        if let Some(ui) = store.get::<Ui>() {
            self.replace(ui.clone());
        }
    }
}
