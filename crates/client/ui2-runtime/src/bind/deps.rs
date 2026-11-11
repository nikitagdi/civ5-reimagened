//! What bindings read, and who reads what.
//!
//! A [`Plan`] resolves the paths of a package's expressions against the model schema once (when the runtime
//! first sees a model store, and again if the schema changes): a model path becomes its [`FieldId`], a skin
//! state path (`state.*`) that the schema does not carry yet becomes an index into the package's state
//! defaults. The [`Index`] maps every field and state path to the binding rows that read it, so a frame delta
//! reaches exactly the rows whose inputs changed.
//!
//! Fields are numbered in preorder with the end of each subtree in the path table, so a change of a struct, a
//! list or a whole root is a range of fields: the rows reading anything inside it.

use std::collections::HashMap;

use ui2_model::{FieldId, Key, Schema, Type};
use ui2_package::profile::expr::{Expr, Value};
use ui2_package::{Lit, Package};

/// What a path of an expression reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Slot {
    /// A field of the model.
    Field(FieldId),
    /// A skin state path not in the schema: index into the package's state defaults.
    State(u32),
    /// A path that names nothing (reported once, reads `null`).
    Missing,
}

/// A state default as an expression value.
fn lit_value(l: &Lit) -> Value {
    match l {
        Lit::Bool(b) => Value::Bool(*b),
        Lit::Int(i) => Value::Int(*i),
        Lit::Float(f) => Value::Float(*f),
        Lit::Str(s) => Value::Str(s.clone()),
        Lit::List(items) => Value::List(items.len()),
    }
}

/// Paths resolved against one schema.
#[derive(Debug)]
pub(crate) struct Plan {
    /// The schema the fields belong to.
    pub schema: &'static Schema,
    /// Path text (`top.gold`, `state.tab`) to what it reads.
    slots: HashMap<String, Slot>,
    /// The values of the state paths, by `Slot::State` index.
    pub state: Vec<Value>,
    /// The names of the state paths (without `state.`), by index.
    state_names: Vec<String>,
}

impl Plan {
    /// An empty plan for a schema; paths are resolved on first use.
    pub fn new(schema: &'static Schema, pkg: &Package) -> Self {
        let mut state = Vec::new();
        let mut state_names = Vec::new();
        for (name, lit) in pkg.state_defaults() {
            state_names.push(name.to_string());
            state.push(lit_value(lit));
        }
        Self {
            schema,
            slots: HashMap::new(),
            state,
            state_names,
        }
    }

    /// The number of fields of the schema.
    pub fn fields(&self) -> usize {
        self.schema.paths().len()
    }

    /// What a path (segments joined by `.`) reads.
    pub fn slot(&mut self, path: &str) -> Slot {
        if let Some(s) = self.slots.get(path) {
            return *s;
        }
        let slot = match self.schema.resolve(path) {
            Ok(f) => Slot::Field(f.id),
            Err(_) => path
                .strip_prefix("state.")
                .and_then(|rest| self.state_names.iter().position(|n| n == rest))
                .map_or(Slot::Missing, |i| Slot::State(i as u32)),
        };
        self.slots.insert(path.to_string(), slot);
        slot
    }

    /// A slot already resolved (evaluation never resolves: every path of a row was resolved when the row was
    /// made).
    pub fn lookup(&self, path: &str) -> Slot {
        self.slots.get(path).copied().unwrap_or(Slot::Missing)
    }

    /// The fields inside a field's subtree, itself included.
    pub fn subtree(&self, f: FieldId) -> std::ops::Range<u32> {
        let end = self
            .schema
            .paths()
            .get(f.0 as usize)
            .map_or(f.0 + 1, |e| e.end.max(f.0 + 1));
        f.0..end
    }

    /// Whether a field holds localizable text (its shown value depends on the language).
    pub fn is_text(&self, f: FieldId) -> bool {
        let ty = self.schema.paths().get(f.0 as usize).map(|e| &e.ty);
        let scalar = |t: &Type| matches!(t, Type::Scalar(ui2_model::Scalar::Text));
        match ty {
            Some(Type::Option(inner)) => scalar(inner),
            Some(t) => scalar(t),
            None => false,
        }
    }
}

/// The paths of an expression, joined.
pub(crate) fn paths_of(e: &Expr, out: &mut Vec<String>) {
    e.for_each_path(&mut |segs| out.push(segs.join(".")));
}

/// Whether an expression calls `loc(...)`.
pub(crate) fn calls_loc(e: &Expr) -> bool {
    use ui2_package::profile::expr::Func;
    match e {
        Expr::Call(Func::Loc, _) => true,
        Expr::Call(_, args) => args.iter().any(calls_loc),
        Expr::Unary(_, a) => calls_loc(a),
        Expr::Binary(_, a, b) => calls_loc(a) || calls_loc(b),
        Expr::Cond(a, b, c) => calls_loc(a) || calls_loc(b) || calls_loc(c),
        _ => false,
    }
}

/// Rows by what they read.
#[derive(Debug, Default)]
pub(crate) struct Index {
    by_field: Vec<Vec<u32>>,
    by_state: Vec<Vec<u32>>,
}

impl Index {
    /// An empty index for a plan.
    pub fn new(plan: &Plan) -> Self {
        Self {
            by_field: vec![Vec::new(); plan.fields()],
            by_state: vec![Vec::new(); plan.state.len()],
        }
    }

    fn list(&mut self, s: Slot) -> Option<&mut Vec<u32>> {
        match s {
            Slot::Field(f) => self.by_field.get_mut(f.0 as usize),
            Slot::State(i) => self.by_state.get_mut(i as usize),
            Slot::Missing => None,
        }
    }

    /// Registers a row as a reader of a slot.
    pub fn add(&mut self, s: Slot, row: u32) {
        if let Some(l) = self.list(s)
            && !l.contains(&row)
        {
            l.push(row);
        }
    }

    /// Forgets a row as a reader of a slot.
    pub fn remove(&mut self, s: Slot, row: u32) {
        if let Some(l) = self.list(s) {
            l.retain(|r| *r != row);
        }
    }

    /// The rows reading any field in a range.
    pub fn readers_of_fields(&self, range: std::ops::Range<u32>) -> impl Iterator<Item = u32> + '_ {
        let lo = (range.start as usize).min(self.by_field.len());
        let hi = (range.end as usize).min(self.by_field.len());
        self.by_field[lo..hi].iter().flatten().copied()
    }
}

/// Whether the keys of a change cover the keys of a row: the change's keys are a prefix of the row's (a
/// change outside lists reaches every row; a change inside one list element reaches the rows of that
/// element only).
pub(crate) fn keys_cover(change: &[Key], row: &[Key]) -> bool {
    change.len() <= row.len() && change.iter().zip(row).all(|(a, b)| a == b)
}
