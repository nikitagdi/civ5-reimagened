//! Bindings: the package's binding table turned into rows of live nodes, and the work a model change causes.
//!
//! Every node instance with bindings owns a contiguous range of *rows* (`Node::binds`): one per scalar binding
//! of its template node (`data-bind-value`, `data-l10n`, `data-bind-class-toggle`, `data-bind-style-*`,
//! `data-bind-disabled`), and one per step of `data-on-*` that reads something (the arguments of `intent`
//! and `call`, the value of `set`, the path of `toggle`): those rows only keep their values, which input
//! reads when the event happens. A row knows what it reads (the [`deps::Slot`]s of its expressions) and is
//! registered in the dependency [`deps::Index`] under each of them.
//!
//! Work is driven by two queues. New nodes are queued for the BIND pass by instantiation; the pass makes their
//! rows and evaluates them once. A frame delta ([`Binder::apply_delta`]) queues exactly the rows reading a
//! changed field (a struct, list or root change is the range of fields inside it, narrowed by list keys); the
//! next pass evaluates each queued row once. A row whose result did not change has no effect; otherwise:
//! a text writes its leaf and queues it for shaping, a class or the disabled state goes through style
//! selection, a style property becomes an override of the node's record (layout or paint, by what changed).
//!
//! The rows of `for` items carry the keys of their instance; list bindings (`for`, `if`, sorting) build on
//! this and are not here yet.

pub(crate) mod deps;
pub(crate) mod eval;
pub(crate) mod style;

use std::sync::Arc;

use ui2_model::{FrameDelta, Key, ModelStore, Op, Scalar as MScalar, Type, Value as MValue};
use ui2_package::profile::action::Action;
use ui2_package::profile::expr::{Expr, Value, eval, truthy};
use ui2_package::{BindKind, ExprId, NodeKind, NodeRef, Package, PropId, StrId, TextSrc, TplId};

use crate::arena::{Arena, Dirty, NodeClass, NodeId, StateBits};
use crate::error::{ErrorCode, ErrorSink, UiError};
use crate::instantiate;
use crate::loc::SkinLoc;
use crate::style::{self as styles, OverValue, StyleIndex};
use deps::{Index, Plan, Slot};
use eval::Host;

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
}

/// The last result of a row (a new result equal to it has no effect).
#[derive(Debug, Clone, PartialEq)]
enum Out {
    Text(Arc<str>),
    Bool(bool),
    Style(Option<OverValue>),
    Values(Vec<MValue>),
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
pub(crate) fn action_of(pkg: &Package, tmpl: NodeRef, bind: u16, action: u16) -> Option<&Action> {
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

/// The value of an action argument: a bare model path gives the field as the model holds it (a key, an enum,
/// a whole struct such as a plot); anything else is evaluated and converted to the expected type.
fn arg_value(host: &mut Host<'_>, e: &Expr, ty: Option<&Type>) -> MValue {
    if let Expr::Path(segs) = e
        && let Slot::Field(f) = host.plan.lookup(&segs.join("."))
    {
        return host.raw(f);
    }
    to_model(&eval(e, host), ty)
}

#[derive(Debug)]
struct Row {
    /// The node the binding belongs to.
    node: NodeId,
    kind: RowKind,
    /// What the row reads.
    deps: Vec<Slot>,
    /// The list keys of the node's instances, outermost first.
    keys: Vec<Key>,
    /// The result depends on the language.
    localized: bool,
    queued: bool,
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
    /// Localization.
    pub loc: SkinLoc,
}

/// What the pass reads besides the arena.
pub(crate) struct Ctx<'a> {
    pub pkg: &'a Package,
    pub index: &'a StyleIndex,
    pub store: &'a ModelStore,
}

/// The keys of the `for` instances above a node, outermost first.
fn instance_keys(arena: &Arena, id: NodeId) -> Vec<Key> {
    let mut keys = Vec::new();
    let mut at = Some(id);
    while let Some(n) = at.and_then(|i| arena.get(i)) {
        if let Some(k) = n.key() {
            keys.push(k);
        }
        at = n.parent();
    }
    keys.reverse();
    keys
}

/// The text leaf a text row writes: the node itself, or the first text leaf among an element's children.
fn text_target(arena: &Arena, id: NodeId) -> Option<NodeId> {
    let n = arena.get(id)?;
    if n.class() == NodeClass::Text {
        return Some(id);
    }
    n.children()
        .iter()
        .copied()
        .find(|&c| arena.get(c).is_some_and(|c| c.class() == NodeClass::Text))
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
            loc,
        }
    }

    /// Makes sure the paths are resolved against the store's schema. A new schema (another store) frees every
    /// row and queues every node for new rows.
    fn ensure_plan(&mut self, arena: &mut Arena, pkg: &Package, store: &ModelStore) {
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
            self.rows.clear();
            self.free.clear();
            self.queue.clear();
            let ids: Vec<NodeId> = arena.ids().collect();
            for id in ids {
                if let Some(n) = arena.get_mut(id) {
                    n.binds = 0..0;
                }
                arena.mark(id, Dirty::BIND);
            }
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

    /// Queues the rows that read what a frame delta changed. Nothing is evaluated here.
    pub fn apply_delta(
        &mut self,
        arena: &mut Arena,
        pkg: &Package,
        d: &FrameDelta,
        store: &ModelStore,
    ) {
        self.ensure_plan(arena, pkg, store);
        let Some(plan) = self.plan.as_ref() else {
            return;
        };
        for op in &d.ops {
            let (field, keys): (_, &[Key]) = match op {
                Op::Set { field, keys, .. } => (*field, keys),
                Op::Insert { list, keys, .. }
                | Op::Remove { list, keys, .. }
                | Op::Move { list, keys, .. } => (*list, keys),
                Op::Reset { root, .. } => (plan.schema.root(*root).field, &[]),
            };
            for r in self.index.readers_of_fields(plan.subtree(field)) {
                let covered = self
                    .rows
                    .get(r as usize)
                    .and_then(Option::as_ref)
                    .is_some_and(|row| deps::keys_cover(keys, &row.keys));
                if covered {
                    Self::enqueue(&mut self.rows, &mut self.queue, r);
                }
            }
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
                for s in row.deps {
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
    fn kinds_of(pkg: &Package, arena: &Arena, errors: &mut ErrorSink, id: NodeId) -> Vec<RowKind> {
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
                // Lists and conditions are built by the list bindings, not by rows.
                NodeKind::For { .. } | NodeKind::If { .. } => return kinds,
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
                // Pictures, controls, events, anchors and fitting are handled by their own passes.
                _ => {}
            }
        }
        kinds
    }

    /// The paths a row reads.
    fn paths_of(pkg: &Package, tmpl: NodeRef, kind: &RowKind, out: &mut Vec<String>) {
        if let RowKind::Action { bind, action } = kind {
            match action_of(pkg, tmpl, *bind, *action) {
                Some(Action::Intent { args, .. } | Action::Call { args, .. }) => {
                    for a in args {
                        deps::paths_of(&a.value, out);
                    }
                }
                Some(Action::Set { value, .. }) => deps::paths_of(value, out),
                Some(Action::Toggle { path }) => out.push(path.clone()),
                Some(Action::Stop) | None => {}
            }
            return;
        }
        for e in Self::exprs_of(pkg, kind) {
            deps::paths_of(e, out);
        }
    }

    /// The expressions a row evaluates.
    fn exprs_of<'a>(pkg: &'a Package, kind: &RowKind) -> Vec<&'a ui2_package::profile::expr::Expr> {
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
            RowKind::Class { cond: e, .. } | RowKind::Disabled(e) => {
                pkg.expr(*e).into_iter().collect()
            }
            // Read through `paths_of`: the expressions live in the binding's actions.
            RowKind::Action { .. } => Vec::new(),
        }
    }

    /// Makes the rows of a node and queues them.
    fn make_rows(&mut self, arena: &mut Arena, pkg: &Package, errors: &mut ErrorSink, id: NodeId) {
        if arena.get(id).is_none_or(|n| !n.binds.is_empty()) {
            return;
        }
        let kinds = Self::kinds_of(pkg, arena, errors, id);
        if kinds.is_empty() {
            return;
        }
        let keys = instance_keys(arena, id);
        let tmpl = arena.get(id).map(|n| n.tmpl());
        let count = kinds.len() as u32;
        let start = self.alloc(kinds.len());
        for (i, kind) in kinds.into_iter().enumerate() {
            let r = start + i as u32;
            let mut paths = Vec::new();
            let mut localized = matches!(kind, RowKind::L10n { .. });
            for e in Self::exprs_of(pkg, &kind) {
                localized |= deps::calls_loc(e);
            }
            if let Some(tmpl) = tmpl {
                Self::paths_of(pkg, tmpl, &kind, &mut paths);
            }
            let mut slots: Vec<Slot> = Vec::new();
            if let Some(plan) = self.plan.as_mut() {
                for p in &paths {
                    let s = plan.slot(p);
                    if s == Slot::Missing {
                        let path = instantiate::node_path(arena, pkg, id);
                        errors.report(UiError::at(
                            &path,
                            ErrorCode::Bind,
                            format!("the model has no path {p}"),
                        ));
                    }
                    if let Slot::Field(f) = s {
                        localized |= plan.is_text(f);
                    }
                    if !slots.contains(&s) {
                        slots.push(s);
                    }
                }
            }
            for &s in &slots {
                self.index.add(s, r);
            }
            self.rows[r as usize] = Some(Row {
                node: id,
                kind,
                deps: slots,
                keys: keys.clone(),
                localized,
                queued: false,
                last: None,
            });
            Self::enqueue(&mut self.rows, &mut self.queue, r);
        }
        if let Some(n) = arena.get_mut(id) {
            n.binds = start..start + count;
        }
    }

    /// The binding pass: frees the rows of removed nodes, makes the rows of new nodes, evaluates the queued
    /// rows and applies what changed. An idle frame checks three counters.
    pub fn pass(&mut self, arena: &mut Arena, cx: &Ctx<'_>, errors: &mut ErrorSink) -> PassStats {
        let mut stats = PassStats::default();
        self.ensure_plan(arena, cx.pkg, cx.store);
        if arena.has_removed_binds() {
            for range in arena.take_removed_binds() {
                self.free_rows(range);
            }
        }
        if arena.pending(Dirty::BIND) > 0 {
            for id in arena.take_dirty(Dirty::BIND) {
                stats.visited += 1;
                self.make_rows(arena, cx.pkg, errors, id);
            }
        }
        if self.queue.is_empty() {
            return stats;
        }
        let queue = std::mem::take(&mut self.queue);
        for r in queue {
            stats.visited += 1;
            let Some(mut row) = self.rows.get_mut(r as usize).and_then(Option::take) else {
                continue;
            };
            row.queued = false;
            if arena.contains(row.node) {
                stats.evaluated += 1;
                self.evaluate(arena, cx, errors, &mut row);
            }
            self.rows[r as usize] = Some(row);
        }
        stats
    }

    fn evaluate(&self, arena: &mut Arena, cx: &Ctx<'_>, errors: &mut ErrorSink, row: &mut Row) {
        let Some(plan) = self.plan.as_ref() else {
            return;
        };
        let pkg = cx.pkg;
        let tmpl = arena.get(row.node).map(|n| n.tmpl());
        let mut host = Host::new(pkg, plan, cx.store, &row.keys, &self.loc);
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
            RowKind::Class { cond, .. } | RowKind::Disabled(cond) => {
                Out::Bool(truthy(&host.expr(*cond)))
            }
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
            _ => {}
        }
        row.last = Some(out);
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

    /// The number of live rows.
    pub fn len(&self) -> usize {
        self.rows.iter().filter(|r| r.is_some()).count()
    }
}

/// Writes a text leaf's string and queues it for shaping when it changed.
fn set_text(arena: &mut Arena, leaf: NodeId, s: &Arc<str>) {
    let Some(n) = arena.get_mut(leaf) else {
        return;
    };
    if n.text.as_deref() == Some(&**s) {
        return;
    }
    n.text = Some(Arc::clone(s));
    arena.mark(leaf, Dirty::TEXT);
}
