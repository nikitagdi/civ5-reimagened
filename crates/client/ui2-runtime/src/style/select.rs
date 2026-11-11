//! Condition masks and the reverse index.
//!
//! Condition `i` of a node's set tests a state or a class on a *subject*: the node itself (`up = 0`) or its
//! `up`-th ancestor element. A text leaf is not an element: its conditions are tested from its parent element,
//! so it always selects the same variant as the parent whose set it carries.
//!
//! When a node is made, each condition registers the node in its subject's `deps` and sets a sensitivity bit
//! on the subject. A state or class change on X then:
//! 1. flips the bit on X (cost: one store, also for nodes nobody tests: hover on a chain of ancestors),
//! 2. only if X is sensitive to that kind, queues the dependents of that kind for the style pass,
//! 3. the style pass recomputes each queued node's mask (at most four conditions, each a walk of `up`
//!    parents), takes `set.get(cell, mask)` and, when the record changed, sets the dirty bits of [`diff`].

use ui2_package::{CondKind, StrId, StyleSet, Styles};

use crate::arena::{Arena, Dep, Dirty, NodeClass, NodeId, StateBits};
use crate::style::{Overrides, StyleEnv, StyleIndex, diff};

/// Sensitivity bits: which kinds of conditions are tested on a node.
pub(crate) mod sens {
    pub const HOVER: u8 = 1;
    pub const ACTIVE: u8 = 2;
    pub const FOCUS: u8 = 4;
    pub const FOCUS_VISIBLE: u8 = 8;
    pub const DISABLED: u8 = 16;
    pub const CLASS: u8 = 32;
}

fn sens_of(kind: CondKind) -> u8 {
    match kind {
        CondKind::Hover => sens::HOVER,
        CondKind::Active => sens::ACTIVE,
        CondKind::Focus => sens::FOCUS,
        CondKind::FocusVisible => sens::FOCUS_VISIBLE,
        CondKind::Disabled => sens::DISABLED,
        CondKind::Class(_) => sens::CLASS,
    }
}

/// The condition kind a state bit is tested by (`CHECKED` has none).
fn kind_of_state(bit: StateBits) -> Option<CondKind> {
    match bit {
        StateBits::HOVER => Some(CondKind::Hover),
        StateBits::ACTIVE => Some(CondKind::Active),
        StateBits::FOCUS => Some(CondKind::Focus),
        StateBits::FOCUS_VISIBLE => Some(CondKind::FocusVisible),
        StateBits::DISABLED => Some(CondKind::Disabled),
        _ => None,
    }
}

/// The node conditions are tested from: the node, or a text leaf's parent element.
fn base(arena: &Arena, id: NodeId) -> Option<NodeId> {
    let node = arena.get(id)?;
    if node.class == NodeClass::Text {
        node.parent
    } else {
        Some(id)
    }
}

fn subject(arena: &Arena, id: NodeId, up: u8) -> Option<NodeId> {
    arena.ancestor(base(arena, id)?, up)
}

fn holds(arena: &Arena, subject: NodeId, kind: CondKind) -> bool {
    let Some(n) = arena.get(subject) else {
        return false;
    };
    match kind {
        CondKind::Hover => n.state.contains(StateBits::HOVER),
        CondKind::Active => n.state.contains(StateBits::ACTIVE),
        CondKind::Focus => n.state.contains(StateBits::FOCUS),
        CondKind::FocusVisible => n.state.contains(StateBits::FOCUS_VISIBLE),
        CondKind::Disabled => n.state.contains(StateBits::DISABLED),
        CondKind::Class(c) => n.classes.binary_search(&c).is_ok(),
    }
}

fn mask(arena: &Arena, set: &StyleSet, id: NodeId) -> u8 {
    let mut m = 0u8;
    for (i, c) in set.conds.iter().enumerate() {
        if subject(arena, id, c.up).is_some_and(|s| holds(arena, s, c.kind)) {
            m |= 1 << i;
        }
    }
    m
}

/// Registers a new node's conditions with their subjects and selects its first record.
pub(crate) fn attach(arena: &mut Arena, styles: &Styles, env: &StyleEnv, id: NodeId) {
    let Some(set) = arena
        .get(id)
        .and_then(|n| n.set)
        .and_then(|s| styles.sets.get(s.index()))
    else {
        return;
    };
    for c in &set.conds {
        if let Some(s) = subject(arena, id, c.up)
            && let Some(sn) = arena.get_mut(s)
        {
            sn.deps.push(Dep {
                node: id,
                kind: c.kind,
            });
            sn.sensitive |= sens_of(c.kind);
        }
    }
    let m = mask(arena, set, id);
    if let Some(node) = arena.get_mut(id) {
        node.cond_mask = m;
        node.style = set.get(env.cell, m);
    }
}

/// Selects a node's record again. Returns the passes the change needs (none when the record is the same).
pub(crate) fn reselect(
    arena: &mut Arena,
    styles: &Styles,
    index: &StyleIndex,
    env: &StyleEnv,
    id: NodeId,
) -> Dirty {
    let Some(set) = arena
        .get(id)
        .and_then(|n| n.set)
        .and_then(|s| styles.sets.get(s.index()))
    else {
        return Dirty::NONE;
    };
    let m = mask(arena, set, id);
    let new = set.get(env.cell, m);
    let Some(node) = arena.get_mut(id) else {
        return Dirty::NONE;
    };
    node.cond_mask = m;
    let old = std::mem::replace(&mut node.style, new);
    if old == new {
        return Dirty::NONE;
    }
    // Properties set by bindings stay on top of the new record.
    if let Some(over) = node.over.as_mut() {
        let rebuilt = Overrides::new(index.record(styles, new), std::mem::take(&mut over.props));
        let bits = diff(&over.record, &rebuilt.record);
        **over = rebuilt;
        return bits;
    }
    diff(index.record(styles, old), index.record(styles, new))
}

/// Queues for the style pass the dependents of `subject` whose condition kind `matches`. Stale dependents
/// (removed nodes) are dropped on the way.
fn notify(arena: &mut Arena, subject: NodeId, sens: u8, matches: impl Fn(CondKind) -> bool) {
    let Some(n) = arena.get_mut(subject) else {
        return;
    };
    if n.sensitive & sens == 0 {
        return;
    }
    let mut deps = std::mem::take(&mut n.deps);
    deps.retain(|d| arena.contains(d.node));
    for d in &deps {
        if matches(d.kind) {
            arena.mark(d.node, Dirty::STYLE);
        }
    }
    if let Some(n) = arena.get_mut(subject) {
        n.deps = deps;
    }
}

/// Sets or clears state bits on a node; dependents of each bit that changed are queued.
pub(crate) fn set_state(arena: &mut Arena, id: NodeId, bits: StateBits, on: bool) {
    for i in 0..8 {
        let bit = StateBits(1 << i);
        if !bits.contains(bit) {
            continue;
        }
        let Some(n) = arena.get_mut(id) else {
            return;
        };
        if n.state.contains(bit) == on {
            continue;
        }
        n.state = if on { n.state | bit } else { n.state & !bit };
        if let Some(kind) = kind_of_state(bit) {
            notify(arena, id, sens_of(kind), |k| k == kind);
        }
    }
}

/// Switches a run-time class on a node; dependents testing that class are queued.
pub(crate) fn set_class(arena: &mut Arena, id: NodeId, class: StrId, on: bool) {
    let Some(n) = arena.get_mut(id) else {
        return;
    };
    match (n.classes.binary_search(&class), on) {
        (Err(at), true) => n.classes.insert(at, class),
        (Ok(at), false) => {
            n.classes.remove(at);
        }
        _ => return,
    }
    notify(arena, id, sens::CLASS, |k| k == CondKind::Class(class));
}

/// What one style pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PassStats {
    pub restyled: u32,
    pub visited: u32,
}

/// Environment changes waiting for the next style pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct EnvChange {
    /// The viewport cell changed: nodes whose set varies by cell reselect.
    pub cell: bool,
    /// The theme changed: nodes with token colors repaint.
    pub theme: bool,
    /// The logical viewport changed: the roots lay out again.
    pub viewport: bool,
    /// The device scale changed: every chunk repaints.
    pub k: bool,
}

impl EnvChange {
    pub fn any(self) -> bool {
        self.cell || self.theme || self.viewport || self.k
    }
}

/// The style pass: applies environment changes, then reselects the queued nodes.
pub(crate) fn pass(
    arena: &mut Arena,
    styles: &Styles,
    index: &StyleIndex,
    env: &StyleEnv,
    change: EnvChange,
) -> PassStats {
    let mut stats = PassStats::default();
    if change.any() {
        let ids: Vec<NodeId> = arena.ids().collect();
        for id in ids {
            stats.visited += 1;
            let Some(n) = arena.get(id) else {
                continue;
            };
            let mut bits = Dirty::NONE;
            if change.cell && n.set.is_some_and(|s| index.varies_by_cell(s)) {
                bits |= Dirty::STYLE;
            }
            if change.theme && index.has_tokens(n.style) {
                bits |= Dirty::PAINT;
            }
            if change.viewport && n.parent.is_none() {
                bits |= Dirty::LAYOUT;
            }
            if change.k && n.chunk == id.index() {
                bits |= Dirty::PAINT;
            }
            arena.mark(id, bits);
        }
    }
    if arena.pending(Dirty::STYLE) == 0 {
        return stats;
    }
    for id in arena.take_dirty(Dirty::STYLE) {
        stats.visited += 1;
        stats.restyled += 1;
        let bits = reselect(arena, styles, index, env, id);
        arena.mark(id, bits);
    }
    stats
}
