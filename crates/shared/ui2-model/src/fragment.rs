//! Fragments: the part of the model a skin or a mod brings. A fragment is written in the language of the
//! schema, but declares one `[root]` (its fields) plus its own `[type.*]` and `[enum]`; `[model.*]` and
//! `[intent.*]` are errors. The host mounts it into a namespace of the store ([`ModelStore::mount`]): the root
//! takes the name of the namespace, so its fields are `state.<field>`, `mods.<id>.<field>` or
//! `modstate.<id>.<field>`; the names of its types get the id in front (`hello.Counter`).
//!
//! ```toml
//! [type.Counter]
//! n = "i32"
//!
//! [root]
//! count = "i32"
//! history = { list = "Counter", key = "n" }
//! ```
//!
//! [`ModelStore::mount`]: crate::store::ModelStore::mount

use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock};

use crate::delta::{Op, Ops};
use crate::diff::diff_root;
use crate::schema::{FieldId, FragmentDraft, RootId, Schema, SchemaError};
use crate::store::Owner;
use crate::value::Value;

/// The longest id of a mod.
const MAX_ID: usize = 64;

/// A parsed, checked fragment. It has no name until it is mounted, so one fragment can be mounted in several
/// namespaces.
#[derive(Clone, Debug)]
pub struct Fragment {
    draft: FragmentDraft,
}

impl Fragment {
    /// Read a fragment source.
    ///
    /// # Errors
    /// The first problem found, with its line and column: a `[model.*]` or `[intent.*]` table, a missing
    /// `[root]`, an unknown type, a type that contains itself.
    pub fn parse(src: &str) -> Result<Fragment, SchemaError> {
        Ok(Fragment {
            draft: FragmentDraft::parse(src)?,
        })
    }

    /// The schema of the fragment as mounted in `ns`, shared between mounts of equal content.
    pub(crate) fn schema_for(&self, ns: &Namespace) -> Result<&'static Schema, SchemaError> {
        let root = ns.root_name();
        let prefix = ns.prefix();
        let schema = self.draft.schema_for_mount(&root, prefix)?;
        Ok(intern(root, schema))
    }
}

/// A fragment's schema outlives its mount (the typed paths and the store's id tables borrow it), so schemas
/// are kept for the life of the process, once per `(root name, content)`: switching a skin or toggling a mod
/// back and forth does not grow the memory.
fn intern(root: String, schema: Schema) -> &'static Schema {
    static CACHE: OnceLock<Mutex<HashMap<(String, u64), &'static Schema>>> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache
        .entry((root, schema.hash()))
        .or_insert_with(|| Box::leak(Box::new(schema)))
}

/// A place in the model where a fragment can be mounted.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Namespace {
    /// `state.*`: the state of the active skin.
    State,
    /// `mods.<id>.*`: the Rust half of a mod.
    Mod(Arc<str>),
    /// `modstate.<id>.*`: the script and markup of a mod.
    ModState(Arc<str>),
}

impl Namespace {
    /// The root name the fragment gets: `state`, `mods.hello`, `modstate.hello`.
    #[must_use]
    pub fn root_name(&self) -> String {
        match self {
            Namespace::State => "state".to_string(),
            Namespace::Mod(id) => format!("mods.{id}"),
            Namespace::ModState(id) => format!("modstate.{id}"),
        }
    }

    /// What goes in front of the type names of the fragment.
    pub(crate) fn prefix(&self) -> &str {
        match self {
            Namespace::State => "state",
            Namespace::Mod(id) | Namespace::ModState(id) => id,
        }
    }

    /// Who writes the namespace.
    #[must_use]
    pub fn owner(&self) -> Owner {
        match self {
            Namespace::State => Owner::Skin,
            Namespace::Mod(id) => Owner::Mod(id.clone()),
            Namespace::ModState(id) => Owner::ModScript(id.clone()),
        }
    }

    /// The namespace a path lies in (`state.prod.sort`, `mods.hello.count`, `modstate.hello.open`); `None` for a
    /// path that is not in a namespace or has an id that cannot be one.
    #[must_use]
    pub fn of_path(path: &str) -> Option<Namespace> {
        let root = |s: &str| s.split(['.', '[']).next().map(str::to_string);
        let head = root(path)?;
        if head == "state" {
            return Some(Namespace::State);
        }
        let rest = path.strip_prefix(head.as_str())?.strip_prefix('.')?;
        let id = root(rest)?;
        if !valid_id(&id) {
            return None;
        }
        match head.as_str() {
            "mods" => Some(Namespace::Mod(Arc::from(id.as_str()))),
            "modstate" => Some(Namespace::ModState(Arc::from(id.as_str()))),
            _ => None,
        }
    }

    pub(crate) fn check(&self) -> Result<(), SchemaError> {
        match self {
            Namespace::State => Ok(()),
            Namespace::Mod(id) | Namespace::ModState(id) if valid_id(id) => Ok(()),
            Namespace::Mod(id) | Namespace::ModState(id) => Err(SchemaError::Path {
                path: self.root_name(),
                msg: format!(
                    "`{id}` cannot be a mod id here: use 1 to {MAX_ID} characters of `a-z`, `A-Z`, `0-9`, `_`, `-`"
                ),
            }),
        }
    }
}

fn valid_id(id: &str) -> bool {
    (1..=MAX_ID).contains(&id.len())
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

/// A mounted fragment: its schema, the ids it occupies and its value.
pub(crate) struct Mount {
    pub ns: Namespace,
    pub schema: &'static Schema,
    /// The store-wide id of the first path slot of the fragment (the root itself).
    pub field_base: u32,
    /// The store-wide id of the root.
    pub root: RootId,
    pub cur: Value,
    /// The value at the start of the frame, set by the first write of the frame.
    pub base: Option<Value>,
    /// Consumers have received the root (as a `Reset`) at least once.
    pub sent: bool,
}

impl Mount {
    /// The store-wide ids of the path slots of the fragment.
    pub fn fields(&self) -> Range<u32> {
        self.field_base..self.field_base + self.schema.paths().len() as u32
    }

    /// The value as of the last frame.
    pub fn frame_value(&self) -> &Value {
        self.base.as_ref().unwrap_or(&self.cur)
    }

    /// The operations since the last frame (a `Reset` if the root was never sent); clears the frame state.
    pub fn take_ops(&mut self, out: &mut Ops) {
        let base = self.base.take();
        if !self.sent {
            self.sent = true;
            out.push(Op::Reset {
                root: self.root,
                value: self.cur.clone(),
            });
        } else if let Some(base) = base {
            let first = out.len();
            diff_root(self.schema, RootId(0), &base, &self.cur, out);
            for op in &mut out[first..] {
                self.globalize(op);
            }
        }
    }

    /// Turn the ids of an operation of the fragment's own schema into store-wide ids.
    fn globalize(&self, op: &mut Op) {
        let shift = |f: &mut FieldId| f.0 += self.field_base;
        match op {
            Op::Set { field, .. } => shift(field),
            Op::Insert { list, .. } | Op::Remove { list, .. } | Op::Move { list, .. } => {
                shift(list);
            }
            Op::Reset { root, .. } => *root = self.root,
        }
    }
}
