//! The node arena: every live node of the skin, its tree links, style selection state, dirty flags and the
//! per-node layout storage.
//!
//! Nodes live in a `Vec` of slots addressed by [`NodeId`] (index plus generation); a freed slot is reused with
//! the next generation, so a stale handle never reaches a new node. Work is driven by dirty flags: marking a
//! node pushes it onto the queue of each newly set flag, and the passes (style, layout, text, paint, bindings)
//! drain only their own queue. A frame without changes finds every queue empty and touches no node.
//!
//! `for` and `if` nodes of a template become *service* nodes: they have no box (their style is
//! [`HIDDEN_STYLE`], `display: none` for layout) and their instances are inserted as siblings right after them,
//! in the same parent. An element of an instance therefore has its real parent element as `parent`, which is
//! what CSS conditions on ancestors (`up = k`) and layout expect.

use std::ops::{BitAnd, BitOr, BitOrAssign, Not};
use std::sync::Arc;

use crate::text::Ifc;

use ui2_draw::ChunkId;
use ui2_model::Key;
use ui2_package::{CondKind, NodeRef, StrId, StyleId, StyleSetId};

/// The style of service nodes (`for`, `if`): no box, `display: none` for layout.
pub const HIDDEN_STYLE: StyleId = StyleId(u32::MAX);
/// The style of placeholders that replace what could not be built.
pub const PLACEHOLDER_STYLE: StyleId = StyleId(u32::MAX - 1);

/// A handle to a node: slot index and generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId {
    index: u32,
    generation: u32,
}

impl NodeId {
    /// The slot index (also the node's id in the layout tree).
    pub const fn index(self) -> u32 {
        self.index
    }

    /// The generation of the slot when the node was made.
    pub const fn generation(self) -> u32 {
        self.generation
    }

    /// The node's id in the layout tree (the slot index; generations do not matter during one layout run).
    pub fn taffy(self) -> taffy::NodeId {
        taffy::NodeId::from(self.index as usize)
    }
}

macro_rules! bits_type {
    ($(#[$doc:meta])* $name:ident { $($(#[$cdoc:meta])* $c:ident = $v:expr;)* }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
        pub struct $name(pub u8);

        impl $name {
            /// No bit.
            pub const NONE: $name = $name(0);
            $($(#[$cdoc])* pub const $c: $name = $name($v);)*

            /// Whether every bit of `other` is set.
            pub const fn contains(self, other: $name) -> bool {
                self.0 & other.0 == other.0
            }

            /// Whether any bit of `other` is set.
            pub const fn intersects(self, other: $name) -> bool {
                self.0 & other.0 != 0
            }

            /// Whether no bit is set.
            pub const fn is_empty(self) -> bool {
                self.0 == 0
            }
        }

        impl BitOr for $name {
            type Output = $name;
            fn bitor(self, rhs: $name) -> $name {
                $name(self.0 | rhs.0)
            }
        }

        impl BitOrAssign for $name {
            fn bitor_assign(&mut self, rhs: $name) {
                self.0 |= rhs.0;
            }
        }

        impl BitAnd for $name {
            type Output = $name;
            fn bitand(self, rhs: $name) -> $name {
                $name(self.0 & rhs.0)
            }
        }

        impl Not for $name {
            type Output = $name;
            fn not(self) -> $name {
                $name(!self.0)
            }
        }
    };
}

bits_type! {
    /// Interaction states of a node (what `:hover`, `:active`, ... test).
    StateBits {
        /// Under the pointer, or an ancestor of the node under it.
        HOVER = 1;
        /// Pressed.
        ACTIVE = 2;
        /// Has keyboard focus.
        FOCUS = 4;
        /// Has focus that came from the keyboard.
        FOCUS_VISIBLE = 8;
        /// Disabled by a binding.
        DISABLED = 16;
        /// A checked control.
        CHECKED = 32;
    }
}

bits_type! {
    /// What must be recomputed for a node.
    Dirty {
        /// The style record must be selected again (a condition, the cell or the set changed).
        STYLE = 1;
        /// The box must be laid out again.
        LAYOUT = 2;
        /// The text must be shaped again.
        TEXT = 4;
        /// The node's chunk must be painted again.
        PAINT = 8;
        /// The node's bindings must be evaluated.
        BIND = 16;
    }
}

impl Dirty {
    /// The number of flags (and queues).
    pub const COUNT: usize = 5;
    /// Every flag.
    pub const ALL: Dirty = Dirty(31);

    /// The queue of a single flag.
    fn queue(self) -> Option<usize> {
        (self.0.count_ones() == 1).then(|| self.0.trailing_zeros() as usize)
    }
}

/// What a node is in the arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeClass {
    /// An element box (also pictures and text inputs).
    Element,
    /// The root element of a layer (`data-layer`); its children are chunk roots.
    Layer,
    /// An anonymous text leaf: styled like its parent, conditions are tested on the parent.
    Text,
    /// A `for` or `if` node: no box, its instances follow it as siblings.
    Service,
    /// A box that stands in for something that could not be built.
    Placeholder,
}

impl NodeClass {
    /// Whether the node is an element (counts for `up = k` conditions and has a box).
    pub const fn is_element(self) -> bool {
        matches!(
            self,
            NodeClass::Element | NodeClass::Layer | NodeClass::Placeholder
        )
    }
}

/// A node whose style depends on a condition tested on another node (its *subject*).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Dep {
    /// The dependent node.
    pub node: NodeId,
    /// What the condition tests.
    pub kind: CondKind,
}

/// One instance of a `for` or `if` body: its key and its top-level nodes (siblings after the service node).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    /// The list key of the item (`None` for an `if` body).
    pub key: Option<Key>,
    /// The instance's top-level nodes, in order.
    pub roots: Vec<NodeId>,
}

/// Per-node storage of the layout pass: the result, the cache and the absolutely positioned boxes the node is
/// the containing block of.
#[derive(Debug, Clone)]
pub struct LayoutSlot {
    /// The layout as computed (never rounded: snapping happens at paint).
    pub unrounded: taffy::Layout,
    /// The layout cache of the node.
    pub cache: taffy::Cache,
    /// Out-of-flow boxes whose containing block is this node.
    pub hoisted: Vec<taffy::NodeId>,
}

impl Default for LayoutSlot {
    fn default() -> Self {
        Self {
            unrounded: taffy::Layout::with_order(0),
            cache: taffy::Cache::new(),
            hoisted: Vec::new(),
        }
    }
}

/// One node.
#[derive(Debug, Clone)]
pub struct Node {
    pub(crate) tmpl: NodeRef,
    pub(crate) class: NodeClass,
    pub(crate) parent: Option<NodeId>,
    pub(crate) children: Vec<NodeId>,
    /// The list key, on the top-level nodes of a `for` instance.
    pub(crate) key: Option<Key>,
    /// The service node whose instance this node is a top-level node of.
    pub(crate) owner: Option<NodeId>,
    /// The slot index of the chunk root.
    pub(crate) chunk: u32,
    pub(crate) set: Option<StyleSetId>,
    pub(crate) style: StyleId,
    pub(crate) cond_mask: u8,
    pub(crate) state: StateBits,
    /// Kinds of conditions that some node tests on this one (`style::select` sensitivity bits).
    pub(crate) sensitive: u8,
    /// Classes switched on at run time (sorted).
    pub(crate) classes: Vec<StrId>,
    /// Nodes whose style depends on a condition tested on this node (itself included).
    pub(crate) deps: Vec<Dep>,
    pub(crate) dirty: Dirty,
    /// Service nodes: the instances of the body.
    pub(crate) instances: Vec<Instance>,
    pub(crate) layout: LayoutSlot,
    /// Text leaves: the current string.
    pub(crate) text: Option<Arc<str>>,
    /// Inline formatting roots: shaped runs and lines (see [`crate::text`]).
    pub(crate) ifc: Option<Box<Ifc>>,
    // Extension point (bindings): the range of this instance's rows in the binding table.
}

impl Node {
    pub(crate) fn new(tmpl: NodeRef, class: NodeClass, parent: Option<NodeId>) -> Self {
        Self {
            tmpl,
            class,
            parent,
            children: Vec::new(),
            key: None,
            owner: None,
            chunk: 0,
            set: None,
            style: HIDDEN_STYLE,
            cond_mask: 0,
            state: StateBits::NONE,
            sensitive: 0,
            classes: Vec::new(),
            deps: Vec::new(),
            dirty: Dirty::NONE,
            instances: Vec::new(),
            layout: LayoutSlot::default(),
            text: None,
            ifc: None,
        }
    }

    /// The string of a text leaf.
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// The inline layout of an inline formatting root.
    pub fn ifc(&self) -> Option<&Ifc> {
        self.ifc.as_deref()
    }

    /// The template node it was made from.
    pub fn tmpl(&self) -> NodeRef {
        self.tmpl
    }

    /// What the node is.
    pub fn class(&self) -> NodeClass {
        self.class
    }

    /// The parent element (`None` at the top of the document).
    pub fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    /// The children in document order (service nodes and their instances included).
    pub fn children(&self) -> &[NodeId] {
        &self.children
    }

    /// The list key, on the top-level nodes of a `for` instance.
    pub fn key(&self) -> Option<Key> {
        self.key
    }

    /// The service node this node is a top-level node of an instance of.
    pub fn owner(&self) -> Option<NodeId> {
        self.owner
    }

    /// The chunk (retained draw list) the node paints into: the slot index of its chunk root.
    pub fn chunk(&self) -> ChunkId {
        ChunkId(self.chunk)
    }

    /// The node's style set (`None` for service nodes and placeholders).
    pub fn style_set(&self) -> Option<StyleSetId> {
        self.set
    }

    /// The selected style record (or [`HIDDEN_STYLE`] / [`PLACEHOLDER_STYLE`]).
    pub fn style(&self) -> StyleId {
        self.style
    }

    /// Bit `i` says that condition `i` of the node's style set holds.
    pub fn cond_mask(&self) -> u8 {
        self.cond_mask
    }

    /// Interaction states.
    pub fn state(&self) -> StateBits {
        self.state
    }

    /// Classes switched on at run time, sorted.
    pub fn classes(&self) -> &[StrId] {
        &self.classes
    }

    /// Pending work.
    pub fn dirty(&self) -> Dirty {
        self.dirty
    }

    /// The instances of a service node's body.
    pub fn instances(&self) -> &[Instance] {
        &self.instances
    }

    /// The layout storage.
    pub fn layout(&self) -> &LayoutSlot {
        &self.layout
    }
}

#[derive(Debug, Clone)]
struct Slot {
    generation: u32,
    node: Option<Node>,
}

/// All nodes of a runtime.
#[derive(Debug, Default)]
pub struct Arena {
    slots: Vec<Slot>,
    free: Vec<u32>,
    roots: Vec<NodeId>,
    live: usize,
    revision: u64,
    queues: [Vec<NodeId>; Dirty::COUNT],
    removed_chunks: Vec<ChunkId>,
}

impl Arena {
    /// An empty arena.
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of live nodes.
    pub fn len(&self) -> usize {
        self.live
    }

    /// Whether no node is alive.
    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// The number of slots, live or free (layout ids are below it).
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Bumped by every structural change (nodes made, removed or moved).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// The top-level nodes of the document, in order.
    pub fn roots(&self) -> &[NodeId] {
        &self.roots
    }

    /// A live node.
    pub fn get(&self, id: NodeId) -> Option<&Node> {
        let slot = self.slots.get(id.index as usize)?;
        if slot.generation != id.generation {
            return None;
        }
        slot.node.as_ref()
    }

    pub(crate) fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.generation != id.generation {
            return None;
        }
        slot.node.as_mut()
    }

    /// Whether the handle names a live node.
    pub fn contains(&self, id: NodeId) -> bool {
        self.get(id).is_some()
    }

    /// The live node in a slot, by index (the layout tree addresses nodes this way).
    pub fn at(&self, index: usize) -> Option<&Node> {
        self.slots.get(index)?.node.as_ref()
    }

    pub(crate) fn at_mut(&mut self, index: usize) -> Option<&mut Node> {
        self.slots.get_mut(index)?.node.as_mut()
    }

    /// The handle of the live node in a slot.
    pub fn id_at(&self, index: usize) -> Option<NodeId> {
        let slot = self.slots.get(index)?;
        slot.node.as_ref().map(|_| NodeId {
            index: index as u32,
            generation: slot.generation,
        })
    }

    /// Every live node, in slot order.
    pub fn ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.slots.iter().enumerate().filter_map(|(i, s)| {
            s.node.as_ref().map(|_| NodeId {
                index: i as u32,
                generation: s.generation,
            })
        })
    }

    /// Every live node in document order.
    pub fn preorder(&self) -> Vec<NodeId> {
        let mut out = Vec::with_capacity(self.live);
        let mut stack: Vec<NodeId> = self.roots.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            if let Some(node) = self.get(id) {
                out.push(id);
                stack.extend(node.children.iter().rev().copied());
            }
        }
        out
    }

    /// The children of `parent`, or the top-level nodes for `None`.
    pub fn siblings(&self, parent: Option<NodeId>) -> &[NodeId] {
        match parent {
            None => &self.roots,
            Some(p) => self.get(p).map_or(&[], |n| n.children.as_slice()),
        }
    }

    fn siblings_mut(&mut self, parent: Option<NodeId>) -> Option<&mut Vec<NodeId>> {
        match parent {
            None => Some(&mut self.roots),
            Some(p) => self.get_mut(p).map(|n| &mut n.children),
        }
    }

    pub(crate) fn alloc(&mut self, node: Node) -> NodeId {
        self.live += 1;
        if let Some(index) = self.free.pop()
            && let Some(slot) = self.slots.get_mut(index as usize)
        {
            slot.node = Some(node);
            return NodeId {
                index,
                generation: slot.generation,
            };
        }
        let index = self.slots.len() as u32;
        self.slots.push(Slot {
            generation: 0,
            node: Some(node),
        });
        NodeId {
            index,
            generation: 0,
        }
    }

    /// Inserts `child` into the children of `parent` (or the top level) at `at`, clamped to the end.
    pub(crate) fn attach(&mut self, parent: Option<NodeId>, at: usize, child: NodeId) {
        if let Some(list) = self.siblings_mut(parent) {
            let at = at.min(list.len());
            list.insert(at, child);
        }
    }

    /// The `up`-th ancestor (0 is the node itself).
    pub fn ancestor(&self, id: NodeId, up: u8) -> Option<NodeId> {
        let mut at = id;
        for _ in 0..up {
            at = self.get(at)?.parent?;
        }
        self.contains(at).then_some(at)
    }

    /// Removes a node and its subtree (for a service node, its instances too). Returns the number of nodes
    /// freed; a stale handle frees nothing.
    pub fn remove(&mut self, id: NodeId) -> usize {
        let Some(node) = self.get(id) else {
            return 0;
        };
        let parent = node.parent;
        let owner = node.owner;
        if let Some(list) = self.siblings_mut(parent) {
            list.retain(|c| *c != id);
        }
        if let Some(owner) = owner.and_then(|o| self.get_mut(o)) {
            for inst in &mut owner.instances {
                inst.roots.retain(|r| *r != id);
            }
            owner.instances.retain(|i| !i.roots.is_empty());
        }
        if let Some(p) = parent {
            self.mark(p, Dirty::LAYOUT | Dirty::PAINT);
        }
        let freed = self.free_subtree(id);
        self.bump_revision();
        freed
    }

    fn free_subtree(&mut self, id: NodeId) -> usize {
        let mut freed = 0;
        let mut stack = vec![id];
        while let Some(id) = stack.pop() {
            let Some(slot) = self.slots.get_mut(id.index as usize) else {
                continue;
            };
            if slot.generation != id.generation {
                continue;
            }
            let Some(node) = slot.node.take() else {
                continue;
            };
            slot.generation = slot.generation.wrapping_add(1);
            self.free.push(id.index);
            self.live -= 1;
            freed += 1;
            if node.chunk == id.index {
                self.removed_chunks.push(ChunkId(id.index));
            }
            stack.extend(node.children.iter().copied());
            // The instances of a service node are its siblings: detach them from the parent that may survive.
            for inst in &node.instances {
                for &root in &inst.roots {
                    if let Some(list) = self.siblings_mut(node.parent) {
                        list.retain(|c| *c != root);
                    }
                    stack.push(root);
                }
            }
        }
        freed
    }

    /// Sets dirty flags; each flag that was not set yet queues the node for its pass.
    pub fn mark(&mut self, id: NodeId, bits: Dirty) {
        let Some(node) = self.get_mut(id) else {
            return;
        };
        let new = bits & !node.dirty;
        if new.is_empty() {
            return;
        }
        node.dirty |= new;
        for q in 0..Dirty::COUNT {
            if new.0 & (1 << q) != 0 {
                self.queues[q].push(id);
            }
        }
    }

    /// Clears one flag on its queued nodes and returns those that are still alive (the pass's work list).
    /// `bit` must be a single flag; any other value returns nothing.
    pub fn take_dirty(&mut self, bit: Dirty) -> Vec<NodeId> {
        let Some(q) = bit.queue() else {
            return Vec::new();
        };
        let mut queued = std::mem::take(&mut self.queues[q]);
        queued.retain(|&id| match self.get_mut(id) {
            Some(node) if node.dirty.contains(bit) => {
                node.dirty = node.dirty & !bit;
                true
            }
            _ => false,
        });
        queued
    }

    /// How many nodes wait for one flag's pass (stale handles included). An idle frame checks this only.
    pub fn pending(&self, bit: Dirty) -> usize {
        bit.queue().map_or(0, |q| self.queues[q].len())
    }

    /// The chunks whose root was removed since the last call (the paint pass drops them from the scene).
    pub fn take_removed_chunks(&mut self) -> Vec<ChunkId> {
        std::mem::take(&mut self.removed_chunks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ui2_package::TemplateId;

    fn leaf(arena: &mut Arena, parent: Option<NodeId>) -> NodeId {
        let id = arena.alloc(Node::new(
            NodeRef(TemplateId(0), 0),
            NodeClass::Element,
            parent,
        ));
        arena.attach(parent, usize::MAX, id);
        id
    }

    #[test]
    fn slots_are_reused_with_a_new_generation() {
        let mut a = Arena::new();
        let root = leaf(&mut a, None);
        let child = leaf(&mut a, Some(root));
        assert_eq!(a.len(), 2);
        assert_eq!(a.remove(child), 1);
        assert!(!a.contains(child));
        let again = leaf(&mut a, Some(root));
        assert_eq!(again.index(), child.index());
        assert_ne!(again.generation(), child.generation());
        assert!(
            a.get(child).is_none(),
            "a stale handle never reaches the new node"
        );
        assert_eq!(a.remove(child), 0);
        assert_eq!(a.get(root).map(|n| n.children().len()), Some(1));
    }

    #[test]
    fn marking_queues_once_per_flag() {
        let mut a = Arena::new();
        let n = leaf(&mut a, None);
        a.mark(n, Dirty::STYLE | Dirty::PAINT);
        a.mark(n, Dirty::PAINT);
        assert_eq!(a.pending(Dirty::PAINT), 1);
        assert_eq!(a.take_dirty(Dirty::PAINT), vec![n]);
        assert_eq!(a.pending(Dirty::PAINT), 0);
        assert!(a.get(n).is_some_and(|n| n.dirty() == Dirty::STYLE));
        // A removed node drops out of the work list.
        a.remove(n);
        assert!(a.take_dirty(Dirty::STYLE).is_empty());
        assert!(a.take_dirty(Dirty::STYLE | Dirty::PAINT).is_empty());
    }

    #[test]
    fn a_node_stays_within_its_memory_budget() {
        // The runtime budget is 1 KiB per node with the layout cache.
        assert!(
            std::mem::size_of::<Node>() <= 1024,
            "{}",
            std::mem::size_of::<Node>()
        );
    }
}
