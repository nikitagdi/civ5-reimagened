//! Structural bindings: `for` lists (keyed instances, sorting and filtering, virtual lists) and `if`
//! conditions.
//!
//! **Lists.** A `for` node keeps a [`ForState`]: the model keys of its list in model order, per item the sort
//! key and the filter result, and the *shown* order (filtered, then stably sorted; without sort and filter it
//! is the model order). The list row of the node reads the list field, so an `Insert`, `Remove`, `Move` or
//! `Reset` of the list (or of anything above it) evaluates it once: the items are reconciled by key and the
//! shown order is rebuilt. A sorted or filtered list has one *item row* per model item that reads what the
//! item's sort key and filter read (fields of the item, `state.*`): a change of one item's key evaluates one
//! row, finds the item's new place by binary search and moves one instance.
//!
//! The instances of a list follow its shown order ([`Binder::sync_lists`]): instances are matched by key and
//! reused, missing ones are made, extra ones removed, and the instances' top-level nodes are put right after
//! the `for` node in shown order (siblings: CSS conditions on ancestors and layout see the real parent).
//!
//! **Virtual lists** (`data-virtual`): only the rows near the visible part of the scroll container live, the
//! visible ones plus four above and four below. The rows have the fixed height of the body's record; the
//! `for` node itself stands for the rows that do not live: a block as high as all rows, pulled back by a
//! negative bottom margin to where the first live row starts, so the content keeps its full height. A scroll
//! gives the instances that left the window to the items that entered it (their bindings are made again;
//! no node is made).
//!
//! **Conditions.** An `if` node makes its body the first time the condition holds. When it stops holding, the
//! body is detached (out of the tree, its bindings asleep: what they read may change, they are only marked
//! stale); when it holds again, the body is put back and only the stale rows are evaluated.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use ui2_model::Key;
use ui2_package::profile::expr::{Expr, Value, truthy};
use ui2_package::style::{Display, Len, LenAuto, StyleRecord};
use ui2_package::{ExprId, NodeKind, TemplateId};

use super::deps::Slot;
use super::eval::{Host, Resolved, Var, VarVal, resolve_path};
use super::{Binder, Ctx, Out, Reads, Row, RowKind};
use crate::arena::{Arena, Dirty, Instance, NodeId};
use crate::error::{ErrorCode, ErrorSink, UiError};
use crate::instantiate::{self, Spawn};
use crate::style::Overrides;

/// Rows of a virtual list made above and below the visible ones.
pub const VIRTUAL_MARGIN: usize = 4;

/// A sort key: numbers, then strings (by code points), then `null`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SortKey {
    Num(f64),
    Str(String),
    Null,
}

impl SortKey {
    fn of(v: &Value) -> SortKey {
        match v {
            Value::Int(i) => SortKey::Num(*i as f64),
            Value::Float(f) if f.is_finite() => SortKey::Num(*f),
            Value::Str(s) | Value::Text(s) => SortKey::Str(s.clone()),
            _ => SortKey::Null,
        }
    }

    /// The order of two keys; descending reverses numbers and strings, `null` stays last.
    fn cmp(&self, other: &SortKey, desc: bool) -> Ordering {
        let rank = |k: &SortKey| match k {
            SortKey::Num(_) => 0,
            SortKey::Str(_) => 1,
            SortKey::Null => 2,
        };
        let o = match (self, other) {
            (SortKey::Num(a), SortKey::Num(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
            (SortKey::Str(a), SortKey::Str(b)) => a.cmp(b),
            (SortKey::Null, SortKey::Null) => return Ordering::Equal,
            (SortKey::Null, _) | (_, SortKey::Null) => return rank(self).cmp(&rank(other)),
            _ => rank(self).cmp(&rank(other)),
        };
        if desc { o.reverse() } else { o }
    }
}

/// One model item of a list.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    /// The position in model order (ties of the sort keep it).
    pos: u32,
    sort: SortKey,
    /// The filter holds (always without a filter).
    pass: bool,
    /// The item row (sorted or filtered lists only).
    row: Option<u32>,
}

/// The window of a virtual list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Virtual {
    /// The height of one row, CSS px.
    row_h: f32,
    /// The shown items that live: `start..end`.
    start: usize,
    end: usize,
    /// The spacer as last set: rows in all and the first live one.
    spacer: Option<(usize, usize)>,
}

/// The state of one `for` node.
#[derive(Debug, Clone)]
pub(crate) struct ForState {
    body: TemplateId,
    item: String,
    index: Option<String>,
    /// The list field and the keys of the lists around it.
    list: Slot,
    base_keys: Vec<Key>,
    /// The model path of the elements (`top.resources[]`), when the list is a model path.
    origin: Option<String>,
    sort: Option<ExprId>,
    sort_desc: Option<ExprId>,
    filter: Option<ExprId>,
    desc: bool,
    virt: Option<Virtual>,
    entries: HashMap<Key, Entry>,
    /// Model order.
    model: Vec<Key>,
    /// Shown order: filtered and sorted.
    display: Vec<Key>,
    /// The shown index each live instance had when its rows were last evaluated (index variables).
    shown_index: HashMap<Key, usize>,
}

impl ForState {
    /// The item rows.
    pub fn item_rows(&self) -> impl Iterator<Item = u32> + '_ {
        self.entries.values().filter_map(|e| e.row)
    }

    /// The list row's reads: the list (with its keys) and the direction of the sort.
    pub fn list_reads(&self, pkg: &crate::Pkg, reads: &mut Reads<'_>) {
        if self.list != Slot::Missing && !reads.out.iter().any(|(s, _)| *s == self.list) {
            reads.out.push((self.list, self.base_keys.clone()));
        }
        if let Some(e) = self.sort_desc.and_then(|e| pkg.expr(e)) {
            reads.expr(e);
        }
    }

    fn elem_keys(&self, key: Key) -> Vec<Key> {
        let mut k = self.base_keys.clone();
        k.push(key);
        k
    }

    /// The index of an item in the shown order (`-1` when it is not shown).
    fn position(&self, key: Key) -> i64 {
        self.display
            .iter()
            .position(|k| *k == key)
            .map_or(-1, |p| p as i64)
    }

    /// The variables of one item: the item and the index (shown position for instances).
    fn vars(&self, key: Key, index: i64) -> Vec<Var> {
        let mut v = vec![Var {
            name: self.item.clone(),
            val: VarVal::Item {
                origin: self.origin.clone(),
                keys: self.elem_keys(key),
            },
        }];
        if let Some(ix) = &self.index {
            v.push(Var {
                name: ix.clone(),
                val: VarVal::Index(index),
            });
        }
        v
    }

    fn order(&self, a: Key, b: Key) -> Ordering {
        match (self.entries.get(&a), self.entries.get(&b)) {
            (Some(x), Some(y)) => x.sort.cmp(&y.sort, self.desc).then(x.pos.cmp(&y.pos)),
            _ => Ordering::Equal,
        }
    }

    /// Builds the shown order from the model order.
    fn rebuild(&mut self) {
        let mut d: Vec<Key> = self
            .model
            .iter()
            .copied()
            .filter(|k| self.entries.get(k).is_some_and(|e| e.pass))
            .collect();
        if self.sort.is_some() {
            d.sort_by(|a, b| self.order(*a, *b));
        }
        self.display = d;
    }

    /// Puts one item where its sort key and filter say: out of the shown order, then back at the place a
    /// binary search finds.
    fn reposition(&mut self, key: Key) {
        if let Some(i) = self.display.iter().position(|k| *k == key) {
            self.display.remove(i);
        }
        if self.entries.get(&key).is_some_and(|e| e.pass) {
            let at = self
                .display
                .partition_point(|k| self.order(*k, key) == Ordering::Less);
            self.display.insert(at, key);
        }
    }

    /// The shown items that should have instances.
    fn wanted(&self) -> &[Key] {
        match self.virt {
            Some(v) => {
                let n = self.display.len();
                &self.display[v.start.min(n)..v.end.min(n)]
            }
            None => &self.display,
        }
    }
}

/// The loop variables in scope at a node, outermost first: the items (and indices) of the `for` instances the
/// node is in.
pub(crate) fn scope_of(arena: &Arena, lists: &HashMap<NodeId, ForState>, id: NodeId) -> Vec<Var> {
    let mut out: Vec<Var> = Vec::new();
    let mut at = Some(id);
    while let Some(n) = at.and_then(|i| arena.get(i)) {
        if let (Some(owner), Some(key)) = (n.owner(), n.key())
            && let Some(st) = lists.get(&owner)
        {
            let mut v = st.vars(key, st.position(key));
            v.reverse();
            out.extend(v);
        }
        at = n.parent();
    }
    out.reverse();
    out
}

/// Whether a node is inside a detached `if` body.
pub(crate) fn in_detached(arena: &Arena, id: NodeId) -> bool {
    let mut at = Some(id);
    while let Some(n) = at.and_then(|i| arena.get(i)) {
        if n.detached {
            return true;
        }
        at = n.parent();
    }
    false
}

/// The nodes of the subtrees of `roots` (children only: detached bodies further down are not included).
fn subtree(arena: &Arena, roots: &[NodeId]) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = roots.to_vec();
    while let Some(id) = stack.pop() {
        if let Some(n) = arena.get(id) {
            out.push(id);
            stack.extend(n.children().iter().copied());
        }
    }
    out
}

/// The height a row of a virtual list takes: its record's fixed height and vertical margins, in px.
fn row_height(pkg: &crate::Pkg, cx: &Ctx<'_>, body: TemplateId) -> Option<f32> {
    let template = pkg.templates().get(body.index())?;
    let root = template
        .nodes
        .first()
        .filter(|n| n.subtree as usize == template.nodes.len())?;
    let set = pkg.styles().sets.get(root.style.index())?;
    let r = cx.index.record(pkg.styles(), set.get(cx.env.cell, 0));
    let px = |l: LenAuto| match l {
        LenAuto::Len(Len { px, pct: 0.0 }) => Some(px),
        _ => None,
    };
    let margin = |l: LenAuto| match l {
        LenAuto::Auto => Some(0.0),
        l => px(l),
    };
    let h = px(r.height)? + margin(r.margin_top)? + margin(r.margin_bottom)?;
    (h > 0.0).then_some(h)
}

impl Binder {
    /// Makes (or makes again) the state of a `for` node.
    pub(super) fn make_list(
        &mut self,
        arena: &mut Arena,
        cx: &Ctx<'_>,
        errors: &mut ErrorSink,
        id: NodeId,
    ) {
        let pkg = cx.pkg;
        let Some(tmpl) = arena.get(id).map(|n| n.tmpl()) else {
            return;
        };
        let Some(NodeKind::For {
            tpl,
            item,
            index,
            list,
            virt,
            sort,
            sort_desc,
            filter,
            ..
        }) = instantiate::template_node(pkg, tmpl).map(|t| t.kind.clone())
        else {
            return;
        };
        if let Some(old) = self.lists.remove(&id) {
            for r in old.item_rows() {
                self.free_rows(r..r + 1);
            }
        }
        let vars = scope_of(arena, &self.lists, id);
        let mut problem = None;
        let (mut slot, mut base_keys, mut origin) = (Slot::Missing, Vec::new(), None);
        match pkg.expr(list) {
            Some(Expr::Path(segs)) => {
                let mut buf = String::new();
                if let Resolved::Path(keys) = resolve_path(&vars, segs, &mut buf)
                    && let Some(plan) = self.plan.as_mut()
                {
                    slot = plan.slot(&buf, cx.store);
                    let depth = match slot {
                        Slot::Field(f) => plan
                            .entry(f)
                            .map_or(0, |(_, e)| e.path.matches("[]").count()),
                        _ => 0,
                    };
                    base_keys = keys[..depth.min(keys.len())].to_vec();
                    if slot == Slot::Missing {
                        problem = Some(format!("the model has no list {buf}"));
                    }
                    origin = Some(format!("{buf}[]"));
                }
            }
            _ => problem = Some("the list of a for is not a model path".to_string()),
        }
        let row_h = if virt {
            let h = row_height(pkg, cx, tpl);
            if h.is_none() {
                problem = Some(
                    "a virtual list needs a body of one element with a fixed height in px; every row is made"
                        .to_string(),
                );
            }
            h
        } else {
            None
        };
        if let Some(msg) = problem {
            let path = instantiate::node_path(arena, pkg, id);
            errors.report(UiError::at(&path, ErrorCode::Bind, msg));
        }
        let name = |s| pkg.str(s).unwrap_or("").to_string();
        self.lists.insert(
            id,
            ForState {
                body: tpl,
                item: name(item),
                index: index.map(name),
                list: slot,
                base_keys,
                origin,
                sort,
                sort_desc,
                filter,
                desc: false,
                virt: row_h.map(|row_h| Virtual {
                    row_h,
                    start: 0,
                    end: VIRTUAL_MARGIN,
                    spacer: None,
                }),
                entries: HashMap::new(),
                model: Vec::new(),
                display: Vec::new(),
                shown_index: HashMap::new(),
            },
        );
    }

    /// The sort key and the filter of one item.
    fn item_values(
        &self,
        cx: &Ctx<'_>,
        st: &ForState,
        vars: &[Var],
    ) -> (SortKey, bool, Vec<String>) {
        let Some(plan) = self.plan.as_ref() else {
            return (SortKey::Null, true, Vec::new());
        };
        let mut host = Host::new(cx.pkg, plan, cx.store, vars, &self.loc);
        let sort = st
            .sort
            .map_or(SortKey::Null, |e| SortKey::of(&host.expr(e)));
        let pass = st.filter.is_none_or(|e| truthy(&host.expr(e)));
        (sort, pass, std::mem::take(&mut host.warnings))
    }

    /// Makes the item row of a new item and gives its first values.
    fn make_item(
        &mut self,
        cx: &Ctx<'_>,
        st: &ForState,
        outer: &[Var],
        service: NodeId,
        key: Key,
        pos: u32,
    ) -> Entry {
        let mut vars = outer.to_vec();
        vars.extend(st.vars(key, i64::from(pos)));
        let mut entry = Entry {
            pos,
            sort: SortKey::Null,
            pass: true,
            row: None,
        };
        if st.sort.is_none() && st.filter.is_none() {
            return entry;
        }
        let Some(plan) = self.plan.as_mut() else {
            return entry;
        };
        let mut reads = Reads {
            plan,
            store: cx.store,
            vars: &vars,
            out: Vec::new(),
            reads_index: false,
            missing: Vec::new(),
        };
        for e in [st.sort, st.filter].into_iter().flatten() {
            if let Some(e) = cx.pkg.expr(e) {
                reads.expr(e);
            }
        }
        let deps = reads.out;
        // The paths are resolved now: the first values can be read.
        (entry.sort, entry.pass, _) = self.item_values(cx, st, &vars);
        let r = self.alloc(1);
        for (s, _) in &deps {
            self.index.add(*s, r);
        }
        self.rows[r as usize] = Some(Row {
            node: service,
            kind: RowKind::Item,
            deps,
            item: Some(key),
            localized: false,
            reads_index: false,
            queued: false,
            asleep: false,
            stale: false,
            last: Some(Out::Done),
        });
        entry.row = Some(r);
        entry
    }

    /// The list row: the model keys are read again, items are added and dropped, the shown order rebuilt.
    pub(super) fn eval_list(
        &mut self,
        arena: &mut Arena,
        cx: &Ctx<'_>,
        errors: &mut ErrorSink,
        service: NodeId,
    ) {
        let Some(mut st) = self.lists.remove(&service) else {
            return;
        };
        let keys: Vec<Key> = match st.list {
            Slot::Field(f) => cx
                .store
                .list_keys(f, &st.base_keys)
                .map(|it| it.collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let outer = scope_of(arena, &self.lists, service);
        if let (Some(e), Some(plan)) = (st.sort_desc, self.plan.as_ref()) {
            let mut host = Host::new(cx.pkg, plan, cx.store, &outer, &self.loc);
            st.desc = truthy(&host.expr(e));
        }
        let live: HashSet<Key> = keys.iter().copied().collect();
        let gone: Vec<Key> = st
            .entries
            .keys()
            .copied()
            .filter(|k| !live.contains(k))
            .collect();
        for k in gone {
            if let Some(r) = st.entries.remove(&k).and_then(|e| e.row) {
                self.free_rows(r..r + 1);
            }
        }
        for (i, &k) in keys.iter().enumerate() {
            match st.entries.get_mut(&k) {
                Some(e) => e.pos = i as u32,
                None => {
                    let e = self.make_item(cx, &st, &outer, service, k, i as u32);
                    st.entries.insert(k, e);
                }
            }
        }
        let _ = errors;
        st.model = keys;
        st.rebuild();
        self.lists.insert(service, st);
        self.mark_list(service);
    }

    /// An item row: the item's sort key or filter changed; it moves to its new place.
    pub(super) fn eval_item(
        &mut self,
        arena: &mut Arena,
        cx: &Ctx<'_>,
        errors: &mut ErrorSink,
        service: NodeId,
        key: Key,
    ) {
        let Some(mut st) = self.lists.remove(&service) else {
            return;
        };
        if let Some(pos) = st.entries.get(&key).map(|e| e.pos) {
            let mut vars = scope_of(arena, &self.lists, service);
            vars.extend(st.vars(key, i64::from(pos)));
            let (sort, pass, warnings) = self.item_values(cx, &st, &vars);
            if let Some(w) = warnings.first() {
                let path = instantiate::node_path(arena, cx.pkg, service);
                errors.report(UiError::at(&path, ErrorCode::Bind, w.clone()));
            }
            let changed = st
                .entries
                .get(&key)
                .is_some_and(|e| e.sort != sort || e.pass != pass);
            if changed {
                if let Some(e) = st.entries.get_mut(&key) {
                    e.sort = sort;
                    e.pass = pass;
                }
                st.reposition(key);
                self.lists.insert(service, st);
                self.mark_list(service);
                return;
            }
        }
        self.lists.insert(service, st);
    }

    fn mark_list(&mut self, service: NodeId) {
        if !self.dirty_lists.contains(&service) {
            self.dirty_lists.push(service);
        }
        if self.lists.get(&service).is_some_and(|s| s.virt.is_some()) {
            self.windows_dirty = true;
        }
    }

    /// Makes the instances of the lists that changed follow their shown order.
    pub(super) fn sync_lists(&mut self, arena: &mut Arena, cx: &Ctx<'_>, errors: &mut ErrorSink) {
        // A list in a detached body is synced when the body comes back (waking marks it again).
        for service in std::mem::take(&mut self.dirty_lists) {
            if arena.contains(service) && !in_detached(arena, service) {
                self.sync(arena, cx, errors, service);
            }
        }
    }

    /// Frees the rows of a subtree and queues its nodes for new ones (an instance given to another item).
    fn rebind_subtree(&mut self, arena: &mut Arena, roots: &[NodeId]) {
        let mut stack: Vec<NodeId> = roots.to_vec();
        while let Some(id) = stack.pop() {
            let Some(n) = arena.get_mut(id) else {
                continue;
            };
            let range = std::mem::replace(&mut n.binds, 0..0);
            stack.extend(n.children.iter().copied());
            let owned: Vec<NodeId> = n
                .instances
                .iter()
                .flat_map(|i| i.roots.iter().copied())
                .collect();
            // Detached bodies of conditions inside are bound again too.
            stack.extend(
                owned
                    .into_iter()
                    .filter(|r| arena.get(*r).is_some_and(|rn| rn.detached)),
            );
            if !range.is_empty() {
                self.free_rows(range);
            }
            if let Some(st) = self.lists.remove(&id) {
                for r in st.item_rows() {
                    self.free_rows(r..r + 1);
                }
            }
            arena.mark(id, Dirty::BIND);
        }
    }

    fn sync(&mut self, arena: &mut Arena, cx: &Ctx<'_>, errors: &mut ErrorSink, service: NodeId) {
        let Some(st) = self.lists.get(&service) else {
            return;
        };
        let wanted: Vec<Key> = st.wanted().to_vec();
        let virtual_list = st.virt.is_some();
        let body = st.body;
        let Some(node) = arena.get_mut(service) else {
            return;
        };
        let parent = node.parent;
        let old = std::mem::take(&mut node.instances);
        let same =
            old.len() == wanted.len() && old.iter().zip(&wanted).all(|(i, k)| i.key == Some(*k));
        let mut new: Vec<Option<Instance>> = Vec::with_capacity(wanted.len());
        if same {
            new.extend(old.into_iter().map(Some));
        } else {
            let mut by_key: HashMap<Key, Instance> = HashMap::new();
            let mut spare: Vec<Instance> = Vec::new();
            for inst in old {
                match inst.key {
                    Some(k) if !by_key.contains_key(&k) => {
                        by_key.insert(k, inst);
                    }
                    _ => spare.push(inst),
                }
            }
            for k in &wanted {
                new.push(by_key.remove(k));
            }
            spare.extend(by_key.into_values());
            for (slot, k) in new.iter_mut().zip(&wanted) {
                if slot.is_some() {
                    continue;
                }
                // A virtual list gives a spare instance to the new item instead of making one.
                if virtual_list && let Some(mut inst) = spare.pop() {
                    for &r in &inst.roots {
                        if let Some(n) = arena.get_mut(r) {
                            n.key = Some(*k);
                        }
                        arena.mark(r, Dirty::LAYOUT | Dirty::PAINT);
                    }
                    inst.key = Some(*k);
                    let roots = inst.roots.clone();
                    self.rebind_subtree(arena, &roots);
                    *slot = Some(inst);
                    continue;
                }
                let at = arena
                    .siblings(parent)
                    .iter()
                    .position(|c| *c == service)
                    .map_or(0, |p| p + 1);
                let roots = instantiate::instantiate(
                    arena,
                    cx.pkg,
                    cx.env,
                    errors,
                    Spawn {
                        tpl: body,
                        parent,
                        at,
                        key: Some(*k),
                        owner: Some(service),
                    },
                );
                if let Some(n) = arena.get_mut(service) {
                    n.instances.clear();
                }
                *slot = Some(Instance {
                    key: Some(*k),
                    roots,
                });
            }
            for inst in spare {
                for r in inst.roots {
                    arena.remove(r);
                }
            }
        }
        let new: Vec<Instance> = new.into_iter().flatten().collect();
        let order: Vec<NodeId> = new.iter().flat_map(|i| i.roots.iter().copied()).collect();
        if let Some(n) = arena.get_mut(service) {
            n.instances = new;
        }
        if arena.place_after(service, &order)
            && let Some(p) = parent
        {
            arena.mark(p, Dirty::LAYOUT | Dirty::TEXT | Dirty::PAINT);
        }
        self.update_indices(arena, service);
        self.update_spacer(arena, service);
    }

    /// Queues the rows that read the index of instances whose shown position changed.
    fn update_indices(&mut self, arena: &Arena, service: NodeId) {
        let Some(st) = self.lists.get_mut(&service) else {
            return;
        };
        if st.index.is_none() {
            return;
        }
        let pos: HashMap<Key, usize> = st
            .display
            .iter()
            .enumerate()
            .map(|(i, k)| (*k, i))
            .collect();
        let mut moved: Vec<NodeId> = Vec::new();
        let instances = arena.get(service).map_or(&[][..], |n| n.instances());
        let mut seen = HashMap::new();
        for inst in instances {
            let Some(k) = inst.key else { continue };
            let p = pos.get(&k).copied().unwrap_or(usize::MAX);
            if st.shown_index.get(&k).is_some_and(|old| *old != p) {
                moved.extend(inst.roots.iter().copied());
            }
            seen.insert(k, p);
        }
        st.shown_index = seen;
        for id in subtree(arena, &moved) {
            let Some(range) = arena.get(id).map(|n| n.binds.clone()) else {
                continue;
            };
            for r in range {
                if self
                    .rows
                    .get(r as usize)
                    .and_then(Option::as_ref)
                    .is_some_and(|row| row.reads_index)
                {
                    Self::enqueue(&mut self.rows, &mut self.queue, r);
                }
            }
        }
    }

    /// Sets the box that stands for the rows of a virtual list that do not live.
    fn update_spacer(&mut self, arena: &mut Arena, service: NodeId) {
        let Some(st) = self.lists.get_mut(&service) else {
            return;
        };
        let total = st.display.len();
        let Some(v) = st.virt.as_mut() else {
            return;
        };
        let start = v.start.min(total);
        if v.spacer == Some((total, start)) {
            return;
        }
        v.spacer = Some((total, start));
        let mut record = StyleRecord::initial();
        record.display = Display::Block;
        record.height = LenAuto::Len(Len::px(total as f32 * v.row_h));
        record.margin_bottom = LenAuto::Len(Len::px(-((total - start) as f32 * v.row_h)));
        let taffy = crate::layout::style::taffy_style(&record);
        if let Some(n) = arena.get_mut(service) {
            n.over = Some(Box::new(Overrides {
                props: Vec::new(),
                record,
                taffy,
            }));
        }
        arena.mark(service, Dirty::LAYOUT);
    }

    /// Asks the next frame to check the windows of virtual lists (a scroll or a layout happened).
    pub fn windows_changed(&mut self) {
        if self.lists.values().any(|s| s.virt.is_some()) {
            self.windows_dirty = true;
        }
    }

    /// Moves the windows of virtual lists to what their scroll containers show. Returns whether instances
    /// changed (the frame then binds, styles and lays out again).
    pub fn update_windows(
        &mut self,
        arena: &mut Arena,
        cx: &Ctx<'_>,
        errors: &mut ErrorSink,
    ) -> bool {
        if !std::mem::take(&mut self.windows_dirty) {
            return false;
        }
        let services: Vec<NodeId> = self
            .lists
            .iter()
            .filter(|(_, s)| s.virt.is_some())
            .map(|(id, _)| *id)
            .collect();
        let mut changed = false;
        for service in services {
            let Some(window) = self.window_of(arena, service) else {
                continue;
            };
            let Some(v) = self.lists.get_mut(&service).and_then(|s| s.virt.as_mut()) else {
                continue;
            };
            if (v.start, v.end) == window {
                continue;
            }
            (v.start, v.end) = window;
            self.sync(arena, cx, errors, service);
            changed = true;
        }
        changed
    }

    /// The live window of a virtual list from its scroll container's box and offset.
    fn window_of(&self, arena: &Arena, service: NodeId) -> Option<(usize, usize)> {
        let st = self.lists.get(&service)?;
        let v = st.virt?;
        let total = st.display.len();
        let sn = arena.get(service)?;
        if in_detached(arena, service) {
            return None;
        }
        let pn = arena.get(sn.parent()?)?;
        let l = pn.layout().unrounded;
        let view_h = (l.size.height - l.border.top - l.border.bottom).max(0.0);
        let top = sn.layout().unrounded.location.y;
        let y0 = pn.scroll()[1] + l.border.top - top;
        let y1 = y0 + view_h;
        let first = (y0 / v.row_h).floor().max(0.0) as usize;
        let last = ((y1 / v.row_h).ceil().max(0.0) as usize).min(total);
        let first = first.min(last);
        Some((
            first.saturating_sub(VIRTUAL_MARGIN),
            (last + VIRTUAL_MARGIN).min(total),
        ))
    }

    /// Applies an `if` condition: makes the body the first time, detaches it when false, puts it back when
    /// true again.
    pub(super) fn set_cond(
        &mut self,
        arena: &mut Arena,
        cx: &Ctx<'_>,
        errors: &mut ErrorSink,
        service: NodeId,
        on: bool,
    ) {
        let Some(node) = arena.get(service) else {
            return;
        };
        let parent = node.parent();
        let inst = node.instances().first().cloned();
        let detached = inst
            .as_ref()
            .and_then(|i| i.roots.first())
            .and_then(|r| arena.get(*r))
            .is_some_and(|n| n.detached);
        match (on, inst) {
            (true, None) => {
                let Some(tpl) =
                    instantiate::template_node(cx.pkg, node.tmpl()).and_then(|t| match t.kind {
                        NodeKind::If { tpl, .. } => Some(tpl),
                        _ => None,
                    })
                else {
                    return;
                };
                let at = arena
                    .siblings(parent)
                    .iter()
                    .position(|c| *c == service)
                    .map_or(0, |p| p + 1);
                instantiate::instantiate(
                    arena,
                    cx.pkg,
                    cx.env,
                    errors,
                    Spawn {
                        tpl,
                        parent,
                        at,
                        key: None,
                        owner: Some(service),
                    },
                );
            }
            (true, Some(inst)) if detached => {
                arena.place_after(service, &inst.roots);
                for &r in &inst.roots {
                    if let Some(n) = arena.get_mut(r) {
                        n.detached = false;
                    }
                    arena.mark(r, Dirty::LAYOUT | Dirty::PAINT);
                }
                if let Some(p) = parent {
                    arena.mark(p, Dirty::LAYOUT | Dirty::TEXT | Dirty::PAINT);
                }
                self.set_asleep(arena, &inst.roots, false);
            }
            (false, Some(inst)) if !detached => {
                arena.unlink(parent, &inst.roots);
                for &r in &inst.roots {
                    if let Some(n) = arena.get_mut(r) {
                        n.detached = true;
                        if n.chunk == r.index() {
                            let c = n.chunk();
                            arena.hide_chunk(c);
                        }
                    }
                }
                if let Some(p) = parent {
                    arena.mark(p, Dirty::LAYOUT | Dirty::TEXT | Dirty::PAINT);
                }
                self.set_asleep(arena, &inst.roots, true);
            }
            _ => {}
        }
    }

    /// Puts the rows of a subtree to sleep or wakes them; a woken row that went stale is queued.
    fn set_asleep(&mut self, arena: &Arena, roots: &[NodeId], asleep: bool) {
        let mut rows: Vec<u32> = Vec::new();
        let mut services = Vec::new();
        for id in subtree(arena, roots) {
            if let Some(n) = arena.get(id) {
                rows.extend(n.binds.clone());
            }
            if let Some(st) = self.lists.get(&id) {
                rows.extend(st.item_rows());
                services.push(id);
            }
        }
        for r in rows {
            let Some(Some(row)) = self.rows.get_mut(r as usize) else {
                continue;
            };
            row.asleep = asleep;
            if !asleep && row.stale {
                row.stale = false;
                Self::enqueue(&mut self.rows, &mut self.queue, r);
            }
        }
        if !asleep {
            // Lists that changed while asleep follow their items now.
            for s in services {
                self.mark_list(s);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_keys_put_numbers_then_strings_then_null() {
        let n = SortKey::Num(2.0);
        let s = SortKey::Str("a".into());
        assert_eq!(n.cmp(&s, false), Ordering::Less);
        assert_eq!(s.cmp(&SortKey::Null, false), Ordering::Less);
        assert_eq!(SortKey::Num(1.0).cmp(&n, true), Ordering::Greater);
        // Descending keeps `null` last.
        assert_eq!(SortKey::Null.cmp(&n, true), Ordering::Greater);
        assert_eq!(SortKey::of(&Value::Bool(true)), SortKey::Null);
    }
}
