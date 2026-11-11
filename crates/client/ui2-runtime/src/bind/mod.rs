//! Bindings: the package's binding table turned into rows of live nodes, and the work a model change causes.
//!
//! Every node instance with bindings owns a contiguous range of *rows* (`Node::binds`): one per scalar binding
//! of its template node (`data-bind-value`, `data-l10n`, `data-bind-class-toggle`, `data-bind-style-*`,
//! `data-bind-disabled`, `data-bind-model`, `data-bind-src`, `data-anchor`), and one per step of `data-on-*` that reads something (the arguments of `intent`
//! and `call`, the value of `set`, the path of `toggle`): those rows only keep their values, which input
//! reads when the event happens. A `for` node has one row for its list, an `if` node one for its condition;
//! a sorted or filtered list also has one row per model item for its sort key and filter (see [`list`]).
//! A row knows what it reads (the [`deps::Slot`]s of its expressions, with the list keys of each read) and
//! is registered in the dependency [`deps::Index`] under each of them.
//!
//! Work is driven by two queues. New nodes are queued for the BIND pass by instantiation; the pass makes their
//! rows and evaluates them once. A frame delta ([`Binder::apply_delta`]) queues exactly the rows reading a
//! changed field (a struct, list or root change is the range of fields inside it, narrowed by list keys); the
//! next pass evaluates each queued row once, structural rows (lists, conditions) first. A row whose result
//! did not change has no effect; otherwise: a text writes its leaf and queues it for shaping, a class or the
//! disabled state goes through style selection, a style property becomes an override of the node's record
//! (layout or paint, by what changed), a list or condition makes, moves or removes instances.
//!
//! Loop variables: the expressions of a `for` body name the item (and the index) by the variable of the
//! loop; the rows of an instance read them through the instance's key ([`eval::Var`]).

pub(crate) mod deps;
pub(crate) mod eval;
pub(crate) mod list;
pub(crate) mod style;

use std::collections::HashMap;
use std::sync::Arc;

use ui2_model::{FrameDelta, Key, ModelStore, Op, Scalar as MScalar, Type, Value as MValue};
use ui2_package::profile::action::{Action, AnchorKind, ControlKind};
use ui2_package::profile::expr::{Expr, Value, eval, render, truthy};
use ui2_package::{BindKind, ExprId, ImageSrc, NodeKind, NodeRef, PropId, StrId, TextSrc, TplId};

use crate::anchor::AnchorUpdate;
use crate::arena::{Arena, Dirty, NodeClass, NodeId, StateBits};
use crate::error::{ErrorCode, ErrorSink, UiError};
use crate::instantiate;
use crate::loc::SkinLoc;
use crate::style::{self as styles, OverValue, StyleEnv, StyleIndex};
use deps::{Index, Plan, Slot};
use eval::{Host, Resolved, Var, resolve_path};
use list::ForState;

/// What a row does.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RowKind {
    /// A text template (`data-bind-value`, a bound text leaf); the text goes to the row's text leaf.
    Text(TplId),
    /// A skin text (`data-l10n` with `data-l10n-args`).
    L10n { key: StrId, args: Vec<ExprId> },
    /// A class on while the condition holds.
    Class { class: StrId, cond: ExprId },
    /// The disabled state.
    Disabled(ExprId),
    /// A style property.
    Style { prop: PropId, tpl: TplId },
    /// The values a markup action needs (`data-on-*`): the arguments of an `intent` or `call`, the value of a
    /// `set`, the current value of a `toggle`'s path. `bind` is the binding's index among the node's
    /// bindings, `action` the step's index in it. Evaluated like any row (when what it reads changes) and
    /// kept: an event reads the values as the player saw them, without the model at hand.
    Action { bind: u16, action: u16 },
    /// The value a control edits (`data-bind-model`, the path below `state.`): kept as the model holds it
    /// and as text, for the control's behaviour and its checked state.
    Model(StrId),
    /// The items of a `for` node (its list, and whether the sort is descending).
    List,
    /// The condition of an `if` node.
    Cond(ExprId),
    /// The sort key and the filter of one item of a sorted or filtered `for` (the row's `item`).
    Item,
    /// The game asset reference a picture shows (`data-bind-src`).
    Src(ExprId),
    /// The map object a node follows (`data-anchor`): its kind and the expression of its key.
    Anchor { kind: AnchorKind, id: ExprId },
}

impl RowKind {
    /// Rows that make, move or remove nodes: evaluated before the others of a batch.
    fn structural(&self) -> bool {
        matches!(self, RowKind::List | RowKind::Cond(_) | RowKind::Item)
    }
}

/// The last result of a row (a new result equal to it has no effect).
#[derive(Debug, Clone, PartialEq)]
enum Out {
    Text(Arc<str>),
    Bool(bool),
    Style(Option<OverValue>),
    Values(Vec<MValue>),
    /// Structural rows apply their result themselves.
    Done,
}

/// The key of an anchor's object as the model holds keys (the expression language sees keys as integers).
fn anchor_key(v: &Value) -> MValue {
    match v {
        Value::Int(i) => MValue::Key(Key(*i as u64)),
        Value::Str(s) | Value::Text(s) => {
            Key::parse_js(s).map_or(MValue::Str(s.as_str().into()), MValue::Key)
        }
        other => to_model(other, None),
    }
}

/// Whether a markup action reads anything (and so needs a row).
fn action_reads(a: &Action) -> bool {
    match a {
        Action::Intent { args, .. } | Action::Call { args, .. } => !args.is_empty(),
        Action::Set { .. } | Action::Toggle { .. } => true,
        Action::Stop => false,
    }
}

/// The step `action` of the event binding `bind` of a template node.
pub(crate) fn action_of(
    pkg: &crate::Pkg,
    tmpl: NodeRef,
    bind: u16,
    action: u16,
) -> Option<&Action> {
    match &pkg.bindings_of(tmpl.0, tmpl.1).get(usize::from(bind))?.kind {
        BindKind::Event { actions, .. } => actions.get(usize::from(action)),
        _ => None,
    }
}

/// An expression value as a model value of the type the receiver expects (when known).
pub(crate) fn to_model(v: &Value, ty: Option<&Type>) -> MValue {
    let ty = match ty {
        Some(Type::Option(inner)) => Some(inner.as_ref()),
        t => t,
    };
    let int_scalar = |t: Option<&Type>| {
        matches!(
            t,
            Some(Type::Scalar(MScalar::I8 | MScalar::I16 | MScalar::I32))
        )
    };
    match v {
        Value::Null | Value::List(_) | Value::Struct => MValue::None,
        Value::Bool(b) => MValue::Bool(*b),
        Value::Int(i) => match ty {
            Some(Type::Key(_)) => MValue::Key(Key(*i as u64)),
            Some(Type::Scalar(MScalar::U32 | MScalar::Color)) => {
                u32::try_from(*i).map_or(MValue::None, MValue::U32)
            }
            Some(Type::Scalar(MScalar::F32)) => MValue::F32(*i as f32),
            Some(Type::Enum(_)) => u16::try_from(*i).map_or(MValue::None, MValue::Enum),
            // Integers beyond `i32` are keys (the expression language sees keys as integers).
            _ => i32::try_from(*i).map_or(MValue::Key(Key(*i as u64)), MValue::I32),
        },
        Value::Float(f) if int_scalar(ty) && f.fract() == 0.0 => MValue::I32(*f as i32),
        Value::Float(f) => MValue::F32(*f as f32),
        Value::Str(s) | Value::Text(s) => match ty {
            Some(Type::Key(_)) => {
                Key::parse_js(s).map_or(MValue::Str(s.as_str().into()), MValue::Key)
            }
            _ => MValue::Str(s.as_str().into()),
        },
        Value::Color(c) => MValue::U32(*c),
    }
}

/// The segments of a control's model path (`data-bind-model` keeps the path below the state root of the
/// template's owner: `state.` for the skin, `modstate.<id>.` for a mod).
pub(crate) fn model_segs(pkg: &crate::Pkg, tmpl: NodeRef, path: StrId) -> Vec<String> {
    pkg.state_path(tmpl.0, pkg.str(path).unwrap_or(""))
        .split('.')
        .map(str::to_string)
        .collect()
}

/// Whether a control is checked with its model at `v` (the value and its text): a checkbox or a toggle whose
/// value is true, a radio whose `data-value` is the value. Other nodes are never checked.
fn checked(pkg: &crate::Pkg, tmpl: NodeRef, v: &[MValue]) -> bool {
    let control = pkg
        .bindings_of(tmpl.0, tmpl.1)
        .iter()
        .find_map(|b| match &b.kind {
            BindKind::Control { kind, value, .. } => Some((*kind, *value)),
            _ => None,
        });
    match (control, v) {
        (Some((ControlKind::Checkbox | ControlKind::Toggle, _)), [raw, _]) => {
            *raw == MValue::Bool(true)
        }
        (Some((ControlKind::Radio, Some(value))), [_, MValue::Str(text)]) => {
            pkg.str(value) == Some(&**text)
        }
        _ => false,
    }
}

/// The value of an action argument: a bare model path gives the field as the model holds it (a key, an enum,
/// a whole struct such as a plot); anything else is evaluated and converted to the expected type.
fn arg_value(host: &mut Host<'_>, e: &Expr, ty: Option<&Type>) -> MValue {
    if let Expr::Path(segs) = e {
        let mut buf = String::new();
        if let Resolved::Path(_) = resolve_path(host.vars, segs, &mut buf)
            && let Slot::Field(_) = host.plan.lookup(&buf)
        {
            return host.raw_path(segs);
        }
    }
    to_model(&eval(e, host), ty)
}

#[derive(Debug)]
struct Row {
    /// The node the binding belongs to (the `for` node for list and item rows).
    node: NodeId,
    kind: RowKind,
    /// What the row reads, with the list keys of each read.
    deps: Vec<(Slot, Vec<Key>)>,
    /// The item of an item row.
    item: Option<Key>,
    /// The result depends on the language.
    localized: bool,
    /// The row reads a loop index (it is evaluated again when its instance moves).
    reads_index: bool,
    queued: bool,
    /// The node is in a detached `if` body: the row is not evaluated.
    asleep: bool,
    /// Something the row reads changed while it was asleep.
    stale: bool,
    last: Option<Out>,
}

/// What one binding pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PassStats {
    /// Rows evaluated.
    pub evaluated: u32,
    /// Nodes and rows looked at.
    pub visited: u32,
}

/// The binding rows of a runtime.
#[derive(Debug)]
pub(crate) struct Binder {
    plan: Option<Plan>,
    index: Index,
    rows: Vec<Option<Row>>,
    /// Free row ranges by length: `free[n]` holds the starts of free ranges of `n` rows.
    free: Vec<Vec<u32>>,
    queue: Vec<u32>,
    /// The state of each `for` node.
    lists: HashMap<NodeId, ForState>,
    /// `for` nodes whose instances must follow their shown items.
    dirty_lists: Vec<NodeId>,
    /// A virtual list may show other items: a scroll, a layout or a change of its items.
    windows_dirty: bool,
    /// Localization.
    pub loc: SkinLoc,
    /// The asset references of bound pictures, by node.
    srcs: HashMap<NodeId, Arc<str>>,
    /// Anchor bindings whose object changed since the frame took them.
    anchor_updates: Vec<AnchorUpdate>,
    /// The loop variables of the last row evaluated, by the innermost list instance it is in (`None`:
    /// outside lists). Rows of one instance follow each other in the queue and share them; the scope is
    /// forgotten when a round starts and after a row that makes, moves or removes nodes.
    scope: Option<(Option<NodeId>, Vec<Var>)>,
}

/// What the pass reads besides the arena.
pub(crate) struct Ctx<'a> {
    pub pkg: &'a crate::Pkg,
    pub index: &'a StyleIndex,
    pub store: &'a ModelStore,
    pub env: &'a StyleEnv,
}

/// The text leaf a text row writes: the node itself, or the first text leaf among an element's children.
pub(crate) fn text_target(arena: &Arena, id: NodeId) -> Option<NodeId> {
    let n = arena.get(id)?;
    if n.class() == NodeClass::Text {
        return Some(id);
    }
    n.children()
        .iter()
        .copied()
        .find(|&c| arena.get(c).is_some_and(|c| c.class() == NodeClass::Text))
}

/// The paths an expression reads once loop variables are replaced, with the keys of each read.
pub(crate) struct Reads<'a> {
    pub plan: &'a mut Plan,
    pub store: &'a ModelStore,
    pub vars: &'a [Var],
    pub out: Vec<(Slot, Vec<Key>)>,
    pub reads_index: bool,
    /// Paths that name nothing.
    pub missing: Vec<String>,
}

impl Reads<'_> {
    /// Adds the reads of one path.
    pub fn path(&mut self, segs: &[String]) {
        let mut buf = String::new();
        match resolve_path(self.vars, segs, &mut buf) {
            Resolved::Path(keys) => {
                let slot = self.plan.slot(&buf, self.store);
                let depth = match slot {
                    Slot::Field(f) => self
                        .plan
                        .entry(f)
                        .map_or(0, |(_, e)| e.path.matches("[]").count()),
                    _ => 0,
                };
                if slot == Slot::Missing && !self.missing.contains(&buf) {
                    self.missing.push(buf);
                }
                let keys = keys[..depth.min(keys.len())].to_vec();
                if !self.out.iter().any(|(s, k)| *s == slot && *k == keys) {
                    self.out.push((slot, keys));
                }
            }
            Resolved::Index(_) => self.reads_index = true,
            Resolved::Null => {}
        }
    }

    /// Adds the reads of an expression.
    pub fn expr(&mut self, e: &Expr) {
        let mut paths: Vec<Vec<String>> = Vec::new();
        e.for_each_path(&mut |segs| paths.push(segs.to_vec()));
        for p in paths {
            self.path(&p);
        }
    }
}

impl Binder {
    /// No rows yet; paths are resolved when the first model store is seen.
    pub fn new(loc: SkinLoc) -> Self {
        Self {
            plan: None,
            index: Index::default(),
            rows: Vec::new(),
            free: Vec::new(),
            queue: Vec::new(),
            lists: HashMap::new(),
            dirty_lists: Vec::new(),
            windows_dirty: false,
            loc,
            srcs: HashMap::new(),
            anchor_updates: Vec::new(),
            scope: None,
        }
    }

    /// Forgets every row and queues every node for new rows (paths resolve differently now).
    fn rebind_all(&mut self, arena: &mut Arena) {
        self.rows.clear();
        self.free.clear();
        self.queue.clear();
        self.lists.clear();
        self.dirty_lists.clear();
        if let Some(plan) = self.plan.as_ref() {
            self.index = Index::new(plan);
        }
        let ids: Vec<NodeId> = arena.ids().collect();
        for id in ids {
            if let Some(n) = arena.get_mut(id) {
                n.binds = 0..0;
            }
            arena.mark(id, Dirty::BIND);
        }
    }

    /// Makes sure the paths are resolved against the store's schema. A new schema (another store) frees every
    /// row and queues every node for new rows.
    fn ensure_plan(&mut self, arena: &mut Arena, pkg: &crate::Pkg, store: &ModelStore) {
        let schema = store.schema();
        if self
            .plan
            .as_ref()
            .is_some_and(|p| std::ptr::eq(p.schema, schema))
        {
            return;
        }
        let had_rows = !self.rows.is_empty();
        let plan = Plan::new(schema, pkg);
        self.index = Index::new(&plan);
        self.plan = Some(plan);
        if had_rows {
            self.rebind_all(arena);
        }
    }

    fn enqueue(rows: &mut [Option<Row>], queue: &mut Vec<u32>, r: u32) {
        if let Some(Some(row)) = rows.get_mut(r as usize)
            && !row.queued
        {
            row.queued = true;
            queue.push(r);
        }
    }

    /// Queues the rows that read a field in `range` under `keys`.
    fn queue_readers(&mut self, range: std::ops::Range<u32>, keys: &[Key]) {
        for r in self.index.readers_of_fields(range.clone()) {
            let covered = self
                .rows
                .get(r as usize)
                .and_then(Option::as_ref)
                .is_some_and(|row| {
                    row.deps.iter().any(|(s, k)| match s {
                        Slot::Field(f) => range.contains(&f.0) && deps::keys_cover(keys, k),
                        _ => false,
                    })
                });
            if covered {
                Self::enqueue(&mut self.rows, &mut self.queue, r);
            }
        }
    }

    /// Queues the rows that read what a frame delta changed. Nothing is evaluated here.
    pub fn apply_delta(
        &mut self,
        arena: &mut Arena,
        pkg: &crate::Pkg,
        d: &FrameDelta,
        store: &ModelStore,
    ) {
        self.ensure_plan(arena, pkg, store);
        for op in &d.ops {
            let Some(plan) = self.plan.as_mut() else {
                return;
            };
            let (field, keys): (_, &[Key]) = match op {
                Op::Set { field, keys, .. } => (*field, keys),
                // The other elements of the list do not change: only what reads the list itself (the list
                // rows, `len()`); the rows of a new element are new, those of a removed one go.
                Op::Insert { list, keys, .. }
                | Op::Remove { list, keys, .. }
                | Op::Move { list, keys, .. } => {
                    self.queue_readers(list.0..list.0 + 1, keys);
                    continue;
                }
                Op::Reset { root, .. } => match plan.schema.roots().get(usize::from(root.0)) {
                    Some(r) => (r.field, &[]),
                    None => {
                        // A fragment of the store (`state`, a mod) was mounted or mounted again.
                        if plan.refresh(store) {
                            self.rebind_all(arena);
                            return;
                        }
                        let fields: Vec<u32> = plan.mounted_fields().collect();
                        for f in fields {
                            let range = self
                                .plan
                                .as_ref()
                                .map_or(f..f + 1, |p| p.subtree(ui2_model::FieldId(f)));
                            self.queue_readers(range, &[]);
                        }
                        continue;
                    }
                },
            };
            let range = plan.subtree(field);
            self.queue_readers(range, keys);
        }
    }

    /// Queues every row whose result depends on the language.
    pub fn queue_localized(&mut self) {
        for r in 0..self.rows.len() as u32 {
            if self.rows[r as usize]
                .as_ref()
                .is_some_and(|row| row.localized)
            {
                Self::enqueue(&mut self.rows, &mut self.queue, r);
            }
        }
    }

    fn free_rows(&mut self, range: std::ops::Range<u32>) {
        let len = (range.end - range.start) as usize;
        for r in range.clone() {
            if let Some(row) = self.rows.get_mut(r as usize).and_then(Option::take) {
                for (s, _) in row.deps {
                    self.index.remove(s, r);
                }
            }
        }
        if self.free.len() <= len {
            self.free.resize_with(len + 1, Vec::new);
        }
        self.free[len].push(range.start);
    }

    fn alloc(&mut self, n: usize) -> u32 {
        if let Some(start) = self.free.get_mut(n).and_then(Vec::pop) {
            return start;
        }
        let start = self.rows.len() as u32;
        self.rows.resize_with(self.rows.len() + n, || None);
        start
    }

    /// The binding kinds of a node.
    fn kinds_of(
        pkg: &crate::Pkg,
        arena: &Arena,
        errors: &mut ErrorSink,
        id: NodeId,
    ) -> Vec<RowKind> {
        let Some(node) = arena.get(id) else {
            return Vec::new();
        };
        let mut kinds = Vec::new();
        if let Some(t) = instantiate::template_node(pkg, node.tmpl()) {
            match &t.kind {
                NodeKind::Text(TextSrc::Bound(tpl)) => kinds.push(RowKind::Text(*tpl)),
                NodeKind::Text(TextSrc::L10n { key, args }) => kinds.push(RowKind::L10n {
                    key: *key,
                    args: args.clone(),
                }),
                NodeKind::For { .. } => return vec![RowKind::List],
                NodeKind::If { cond, .. } => return vec![RowKind::Cond(*cond)],
                NodeKind::Image(ImageSrc::Bound(e)) => kinds.push(RowKind::Src(*e)),
                _ => {}
            }
        }
        let r = node.tmpl();
        for (bi, b) in pkg.bindings_of(r.0, r.1).iter().enumerate() {
            match &b.kind {
                BindKind::Event { actions, .. } => {
                    for (ai, a) in actions.iter().enumerate() {
                        if action_reads(a) {
                            kinds.push(RowKind::Action {
                                bind: bi as u16,
                                action: ai as u16,
                            });
                        }
                    }
                }
                BindKind::Text(tpl) => kinds.push(RowKind::Text(*tpl)),
                BindKind::ClassToggle { class, cond } => kinds.push(RowKind::Class {
                    class: *class,
                    cond: *cond,
                }),
                BindKind::Disabled(e) => kinds.push(RowKind::Disabled(*e)),
                BindKind::Src(e) if !kinds.contains(&RowKind::Src(*e)) => {
                    kinds.push(RowKind::Src(*e))
                }
                BindKind::Anchor { kind, id } => kinds.push(RowKind::Anchor {
                    kind: *kind,
                    id: *id,
                }),
                BindKind::Model(p) => kinds.push(RowKind::Model(*p)),
                BindKind::Style { prop, tpl } if styles::bindable(*prop) => {
                    kinds.push(RowKind::Style {
                        prop: *prop,
                        tpl: *tpl,
                    })
                }
                BindKind::Style { prop, .. } => {
                    let path = instantiate::node_path(arena, pkg, id);
                    errors.report(UiError::at(
                        &path,
                        ErrorCode::Bind,
                        format!("{} cannot be bound in this runtime", prop.name()),
                    ));
                }
                // Controls, hotkey labels and fitting are handled by their own passes.
                _ => {}
            }
        }
        kinds
    }

    /// Adds the reads of a row of a node.
    fn reads_of(pkg: &crate::Pkg, tmpl: NodeRef, kind: &RowKind, reads: &mut Reads<'_>) {
        if let RowKind::Model(p) = kind {
            reads.path(&model_segs(pkg, tmpl, *p));
            return;
        }
        if let RowKind::Action { bind, action } = kind {
            match action_of(pkg, tmpl, *bind, *action) {
                Some(Action::Intent { args, .. } | Action::Call { args, .. }) => {
                    for a in args {
                        reads.expr(&a.value);
                    }
                }
                Some(Action::Set { value, .. }) => reads.expr(value),
                Some(Action::Toggle { path }) => {
                    let segs: Vec<String> = path.split('.').map(str::to_string).collect();
                    reads.path(&segs);
                }
                Some(Action::Stop) | None => {}
            }
            return;
        }
        for e in Self::exprs_of(pkg, kind) {
            reads.expr(e);
        }
    }

    /// The expressions a row evaluates (list rows: see [`list`]).
    fn exprs_of<'a>(pkg: &'a crate::Pkg, kind: &RowKind) -> Vec<&'a Expr> {
        let tpl_exprs = |t: TplId| {
            pkg.tpl(t)
                .unwrap_or(&[])
                .iter()
                .filter_map(|s| match s {
                    ui2_package::Seg::Expr(e) => pkg.expr(*e),
                    ui2_package::Seg::Lit(_) => None,
                })
                .collect::<Vec<_>>()
        };
        match kind {
            RowKind::Text(t) | RowKind::Style { tpl: t, .. } => tpl_exprs(*t),
            RowKind::L10n { args, .. } => args.iter().filter_map(|e| pkg.expr(*e)).collect(),
            RowKind::Class { cond: e, .. }
            | RowKind::Disabled(e)
            | RowKind::Cond(e)
            | RowKind::Src(e)
            | RowKind::Anchor { id: e, .. } => pkg.expr(*e).into_iter().collect(),
            // Read through `reads_of` (actions) or the list's state (lists, items).
            RowKind::Action { .. } | RowKind::Model(_) | RowKind::List | RowKind::Item => {
                Vec::new()
            }
        }
    }

    /// Reports the paths of a row that name nothing.
    fn report_missing(
        arena: &Arena,
        pkg: &crate::Pkg,
        errors: &mut ErrorSink,
        id: NodeId,
        missing: &[String],
    ) {
        for p in missing {
            let path = instantiate::node_path(arena, pkg, id);
            errors.report(UiError::at(
                &path,
                ErrorCode::Bind,
                format!("the model has no path {p}"),
            ));
        }
    }

    /// Puts a new row in place: registers its reads and queues it (a row in a sleeping subtree is stale).
    fn place_row(&mut self, r: u32, mut row: Row) {
        for (s, _) in &row.deps {
            self.index.add(*s, r);
        }
        if row.asleep {
            row.stale = true;
            self.rows[r as usize] = Some(row);
        } else {
            self.rows[r as usize] = Some(row);
            Self::enqueue(&mut self.rows, &mut self.queue, r);
        }
    }

    /// Makes the rows of a node and queues them.
    fn make_rows(&mut self, arena: &mut Arena, cx: &Ctx<'_>, errors: &mut ErrorSink, id: NodeId) {
        let pkg = cx.pkg;
        if arena.get(id).is_none_or(|n| !n.binds.is_empty()) {
            return;
        }
        let kinds = Self::kinds_of(pkg, arena, errors, id);
        if kinds.is_empty() {
            return;
        }
        if kinds == [RowKind::List] {
            self.make_list(arena, cx, errors, id);
        }
        let vars = list::scope_of(arena, &self.lists, id);
        let asleep = list::in_detached(arena, id);
        let Some(tmpl) = arena.get(id).map(|n| n.tmpl()) else {
            return;
        };
        let count = kinds.len() as u32;
        let start = self.alloc(kinds.len());
        for (i, kind) in kinds.into_iter().enumerate() {
            let r = start + i as u32;
            let mut localized = matches!(kind, RowKind::L10n { .. });
            for e in Self::exprs_of(pkg, &kind) {
                localized |= deps::calls_loc(e);
            }
            let Some(plan) = self.plan.as_mut() else {
                return;
            };
            let mut reads = Reads {
                plan,
                store: cx.store,
                vars: &vars,
                out: Vec::new(),
                reads_index: false,
                missing: Vec::new(),
            };
            if kind == RowKind::List {
                if let Some(st) = self.lists.get(&id) {
                    st.list_reads(pkg, &mut reads);
                }
            } else {
                Self::reads_of(pkg, tmpl, &kind, &mut reads);
            }
            let (deps, reads_index, missing) = (reads.out, reads.reads_index, reads.missing);
            for (s, _) in &deps {
                if let Slot::Field(f) = s {
                    localized |= self.plan.as_ref().is_some_and(|p| p.is_text(*f));
                }
            }
            Self::report_missing(arena, pkg, errors, id, &missing);
            self.place_row(
                r,
                Row {
                    node: id,
                    kind,
                    deps,
                    item: None,
                    localized,
                    reads_index,
                    queued: false,
                    asleep,
                    stale: false,
                    last: None,
                },
            );
        }
        if let Some(n) = arena.get_mut(id) {
            n.binds = start..start + count;
        }
    }

    /// Frees the rows of removed nodes and the state of removed lists.
    fn collect_removed(&mut self, arena: &mut Arena) {
        if !arena.has_removed_binds() {
            return;
        }
        for range in arena.take_removed_binds() {
            self.free_rows(range);
        }
        if !self.srcs.is_empty() {
            self.srcs.retain(|id, _| arena.contains(*id));
        }
        let dead: Vec<NodeId> = self
            .lists
            .keys()
            .copied()
            .filter(|&id| !arena.contains(id))
            .collect();
        for id in dead {
            if let Some(st) = self.lists.remove(&id) {
                for r in st.item_rows() {
                    self.free_rows(r..r + 1);
                }
            }
        }
    }

    /// The binding pass: frees the rows of removed nodes, makes the rows of new nodes, evaluates the queued
    /// rows and applies what changed; lists and conditions make nodes whose rows are made and evaluated in
    /// the same pass. An idle frame checks three counters.
    pub fn pass(&mut self, arena: &mut Arena, cx: &Ctx<'_>, errors: &mut ErrorSink) -> PassStats {
        let mut stats = PassStats::default();
        self.ensure_plan(arena, cx.pkg, cx.store);
        // Each round makes the rows of new nodes and evaluates the queue; lists and conditions may make new
        // nodes for the next round. The depth of nested lists bounds the rounds.
        for _ in 0..64 {
            self.scope = None;
            self.collect_removed(arena);
            if arena.pending(Dirty::BIND) > 0 {
                for id in arena.take_dirty(Dirty::BIND) {
                    stats.visited += 1;
                    self.make_rows(arena, cx, errors, id);
                }
            }
            if self.queue.is_empty() && self.dirty_lists.is_empty() {
                break;
            }
            let mut queue = std::mem::take(&mut self.queue);
            // Structural rows first: they may remove nodes whose other rows are queued too.
            queue.sort_by_key(|&r| {
                self.rows
                    .get(r as usize)
                    .and_then(Option::as_ref)
                    .is_none_or(|row| !row.kind.structural())
            });
            for r in queue {
                stats.visited += 1;
                let Some(mut row) = self.rows.get_mut(r as usize).and_then(Option::take) else {
                    continue;
                };
                row.queued = false;
                if row.asleep {
                    row.stale = true;
                } else if arena.contains(row.node) {
                    stats.evaluated += 1;
                    if row.kind.structural() {
                        self.scope = None;
                    }
                    self.evaluate(arena, cx, errors, &mut row);
                }
                if let Some(slot) = self.rows.get_mut(r as usize)
                    && slot.is_none()
                {
                    *slot = Some(row);
                }
            }
            self.sync_lists(arena, cx, errors);
        }
        stats
    }

    fn evaluate(&mut self, arena: &mut Arena, cx: &Ctx<'_>, errors: &mut ErrorSink, row: &mut Row) {
        match row.kind {
            RowKind::List => {
                self.eval_list(arena, cx, errors, row.node);
                row.last = Some(Out::Done);
                return;
            }
            RowKind::Item => {
                if let Some(key) = row.item {
                    self.eval_item(arena, cx, errors, row.node, key);
                }
                row.last = Some(Out::Done);
                return;
            }
            _ => {}
        }
        if self.plan.is_none() {
            return;
        }
        // The loop variables in scope: the same as the last row's when it is in the same list instance.
        let top = list::instance_top(arena, row.node);
        if self.scope.as_ref().is_none_or(|(t, _)| *t != top) {
            let vars = match top {
                Some(_) => list::scope_of(arena, &self.lists, row.node),
                None => Vec::new(),
            };
            self.scope = Some((top, vars));
        }
        let Some(plan) = self.plan.as_ref() else {
            return;
        };
        let vars = self.scope.as_ref().map_or(&[][..], |(_, v)| v.as_slice());
        let pkg = cx.pkg;
        let tmpl = arena.get(row.node).map(|n| n.tmpl());
        let mut host = Host::new(pkg, plan, cx.store, vars, &self.loc);
        let mut problem: Option<(ErrorCode, String)> = None;
        let out = match &row.kind {
            RowKind::Action { bind, action } => {
                let act = tmpl.and_then(|t| action_of(pkg, t, *bind, *action));
                Out::Values(match act {
                    Some(Action::Intent { name, args }) => {
                        let def = plan.schema.intents().iter().find(|d| d.name == *name);
                        args.iter()
                            .map(|a| {
                                let ty = def
                                    .zip(a.name.as_ref())
                                    .and_then(|(d, n)| d.args.iter().find(|f| f.name == *n))
                                    .map(|f| &f.ty);
                                arg_value(&mut host, &a.value, ty)
                            })
                            .collect()
                    }
                    Some(Action::Call { args, .. }) => args
                        .iter()
                        .map(|a| arg_value(&mut host, &a.value, None))
                        .collect(),
                    Some(Action::Set { path, value }) => {
                        let ty = plan.schema.resolve(path).ok().map(|f| f.ty);
                        vec![arg_value(&mut host, value, ty.as_ref())]
                    }
                    Some(Action::Toggle { path }) => {
                        let segs: Vec<String> = path.split('.').map(str::to_string).collect();
                        let now = ui2_package::profile::expr::EvalHost::path(&mut host, &segs);
                        vec![MValue::Bool(truthy(&now))]
                    }
                    Some(Action::Stop) | None => Vec::new(),
                })
            }
            RowKind::Model(p) => {
                let segs = tmpl.map(|t| model_segs(pkg, t, *p)).unwrap_or_default();
                let raw = host.raw_path(&segs);
                let seen = ui2_package::profile::expr::EvalHost::path(&mut host, &segs);
                // A state path the store does not carry reads the package's default.
                let raw = if raw == MValue::None {
                    to_model(&seen, None)
                } else {
                    raw
                };
                let text = match &raw {
                    MValue::Key(k) => k.to_js(),
                    _ => render(&seen),
                };
                Out::Values(vec![raw, MValue::Str(text.into())])
            }
            RowKind::Text(tpl) => Out::Text(host.template(*tpl).into()),
            RowKind::L10n { key, args } => {
                let values: Vec<Value> = args.iter().map(|e| host.expr(*e)).collect();
                let key = pkg.str(*key).unwrap_or("");
                match self.loc.skin_text(pkg, key, &values) {
                    Ok(s) => Out::Text(s.into()),
                    Err(k) => {
                        problem = Some((ErrorCode::L10n, format!("the skin has no text {k}")));
                        Out::Text(k.into())
                    }
                }
            }
            RowKind::Class { cond, .. } | RowKind::Disabled(cond) | RowKind::Cond(cond) => {
                Out::Bool(truthy(&host.expr(*cond)))
            }
            RowKind::Anchor { id, .. } => Out::Values(vec![anchor_key(&host.expr(*id))]),
            RowKind::Src(e) => match host.expr(*e) {
                Value::Null => Out::Text("".into()),
                v => Out::Text(render(&v).into()),
            },
            RowKind::Style { prop, tpl } => {
                let text = host.template(*tpl);
                match style::parse(pkg, *prop, &text) {
                    Some(v) => Out::Style(Some(v)),
                    // Nothing to show: the record's own value.
                    None if text.trim().is_empty() => Out::Style(None),
                    None => {
                        problem = Some((
                            ErrorCode::Bind,
                            format!("{text:?} is not a value of {}", prop.name()),
                        ));
                        row.last.clone().unwrap_or(Out::Style(None))
                    }
                }
            }
            RowKind::List | RowKind::Item => Out::Done,
        };
        let warnings = std::mem::take(&mut host.warnings);
        drop(host);
        if problem.is_none()
            && let Some(w) = warnings.first()
        {
            problem = Some((ErrorCode::Bind, w.clone()));
        }
        if let Some((code, msg)) = problem {
            let path = instantiate::node_path(arena, pkg, row.node);
            errors.report(UiError::at(&path, code, msg));
        }
        if row.last.as_ref() == Some(&out) {
            return;
        }
        match (&row.kind, &out) {
            (RowKind::Text(_) | RowKind::L10n { .. }, Out::Text(s)) => {
                if let Some(leaf) = text_target(arena, row.node) {
                    set_text(arena, leaf, s);
                } else {
                    let path = instantiate::node_path(arena, pkg, row.node);
                    errors.report(UiError::at(
                        &path,
                        ErrorCode::Bind,
                        "a text binding on a node without a text leaf",
                    ));
                }
            }
            (RowKind::Class { class, .. }, Out::Bool(on)) => {
                styles::select::set_class(arena, row.node, *class, *on);
            }
            (RowKind::Disabled(_), Out::Bool(on)) => {
                styles::select::set_state(arena, row.node, StateBits::DISABLED, *on);
            }
            (RowKind::Style { prop, .. }, Out::Style(v)) => {
                let bits =
                    styles::set_override(arena, pkg.styles(), cx.index, row.node, *prop, v.clone());
                arena.mark(row.node, bits);
            }
            (RowKind::Cond(_), Out::Bool(on)) => {
                self.set_cond(arena, cx, errors, row.node, *on);
            }
            (RowKind::Anchor { kind, .. }, Out::Values(v)) => {
                let key = v.first().cloned().unwrap_or(MValue::None);
                self.anchor_updates.push((row.node, *kind, key));
            }
            (RowKind::Src(_), Out::Text(s)) => {
                self.srcs.insert(row.node, Arc::clone(s));
                arena.mark(row.node, Dirty::PAINT);
            }
            (RowKind::Model(_), Out::Values(v)) => {
                let on = tmpl.is_some_and(|t| checked(pkg, t, v));
                styles::select::set_state(arena, row.node, StateBits::CHECKED, on);
                // A text field shows its value: its text is shaped again.
                if tmpl
                    .and_then(|t| instantiate::template_node(pkg, t))
                    .is_some_and(|t| matches!(t.kind, ui2_package::NodeKind::Input { .. }))
                {
                    arena.mark(row.node, Dirty::TEXT);
                }
            }
            _ => {}
        }
        row.last = Some(out);
    }

    /// The anchor bindings whose object changed (new anchor nodes included) since the last call.
    pub fn take_anchor_updates(&mut self) -> Vec<AnchorUpdate> {
        std::mem::take(&mut self.anchor_updates)
    }

    /// The asset references bound pictures show (`data-bind-src`), by node.
    pub fn srcs(&self) -> &HashMap<NodeId, Arc<str>> {
        &self.srcs
    }

    /// The values the row of a node's markup action holds (`rows` is the node's row range): `None` while it
    /// was not evaluated yet, or when the action reads nothing.
    pub fn action_values(
        &self,
        rows: std::ops::Range<u32>,
        bind: u16,
        action: u16,
    ) -> Option<&[MValue]> {
        rows.filter_map(|r| self.rows.get(r as usize)?.as_ref())
            .find(|row| row.kind == RowKind::Action { bind, action })
            .and_then(|row| match &row.last {
                Some(Out::Values(v)) => Some(v.as_slice()),
                _ => None,
            })
    }

    /// The value of a control's model as its row last read it (`rows` is the node's row range): the value as
    /// the model holds it and as text. `None` while it was not evaluated yet.
    pub fn model_values(&self, rows: std::ops::Range<u32>) -> Option<(&MValue, &str)> {
        let row = rows
            .filter_map(|r| self.rows.get(r as usize)?.as_ref())
            .find(|row| matches!(row.kind, RowKind::Model(_)))?;
        match &row.last {
            Some(Out::Values(v)) => match v.as_slice() {
                [raw, MValue::Str(text)] => Some((raw, &**text)),
                _ => None,
            },
            _ => None,
        }
    }

    /// The type of the value at a path a row reads (an optional value gives its inner type); a skin state
    /// path the store does not carry has the type of its default.
    pub fn model_type(&self, path: &str) -> Option<Type> {
        let plan = self.plan.as_ref()?;
        match plan.lookup(path) {
            Slot::Field(f) => plan.entry(f).map(|(_, e)| match &e.ty {
                Type::Option(inner) => (**inner).clone(),
                t => t.clone(),
            }),
            Slot::State(i) => match plan.state.get(i as usize)? {
                Value::Bool(_) => Some(Type::Scalar(MScalar::Bool)),
                Value::Int(_) => Some(Type::Scalar(MScalar::I32)),
                Value::Float(_) => Some(Type::Scalar(MScalar::F32)),
                Value::Str(_) => Some(Type::Scalar(MScalar::Str)),
                _ => None,
            },
            Slot::Missing => None,
        }
    }

    /// The number of live rows.
    pub fn len(&self) -> usize {
        self.rows.iter().filter(|r| r.is_some()).count()
    }
}

/// Writes a text leaf's string and queues it for shaping when it changed.
pub(crate) fn set_text(arena: &mut Arena, leaf: NodeId, s: &Arc<str>) {
    let Some(n) = arena.get_mut(leaf) else {
        return;
    };
    if n.text.as_deref() == Some(&**s) {
        return;
    }
    n.text = Some(Arc::clone(s));
    arena.mark(leaf, Dirty::TEXT);
}
