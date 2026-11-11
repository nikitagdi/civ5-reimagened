//! Painting order inside a stacking context (CSS 2 appendix E, as far as the profile goes).
//!
//! A context paints: its own background and border; descendant contexts with a negative `z-index`; the
//! backgrounds of its in-flow, non-positioned descendants in tree order; their content (text, pictures);
//! positioned descendants with `z-index: auto` or `0` in tree order; descendant contexts with a positive
//! `z-index`, lowest first (ties in tree order). A node makes a context when it is positioned with an integer
//! `z-index`, has `opacity < 1`, a `transform` or a `clip-path`; the chunk root is always one. A positioned node with
//! `z-index: auto` is painted with its subtree at its place in the zero list (its positioned descendants
//! are not lifted out of it).

use ui2_package::style::{Display, IntAuto, Position, StyleRecord};

use crate::arena::{Arena, NodeClass, NodeId};

/// Whether a record makes its node a stacking context.
pub(crate) fn makes_context(r: &StyleRecord) -> bool {
    (r.position != Position::Static && matches!(r.z_index, IntAuto::Int(_)))
        || r.opacity < 1.0
        || !r.transform.is_empty()
        || !r.clip_path.is_empty()
}

/// The `z-index` a context or positioned node is ordered by (`auto` is 0).
pub(crate) fn z_of(r: &StyleRecord) -> i32 {
    match r.z_index {
        IntAuto::Int(z) => z,
        IntAuto::Auto => 0,
    }
}

/// The descendants of a context, sorted into its painting phases. Each entry is a node and the state its
/// parent passes down (`S`: transform, clip, opacity), computed while walking.
#[derive(Debug)]
pub(crate) struct Phases<S> {
    /// Contexts with a negative `z-index`, lowest first.
    pub negative: Vec<(NodeId, S)>,
    /// In-flow, non-positioned descendants in tree order (their boxes, then their content).
    pub flow: Vec<(NodeId, S)>,
    /// Positioned descendants with `z-index` `auto` or `0`, in tree order.
    pub zero: Vec<(NodeId, S)>,
    /// Contexts with a positive `z-index`, lowest first.
    pub positive: Vec<(NodeId, S)>,
}

/// Walks the descendants of `root` that paint into the same chunk and sorts them into phases.
/// `record` gives a node's effective record; `child_state(node, state)` the state a node passes to its
/// children; `content_root(node)` whether a node paints its children itself (an inline formatting root).
pub(crate) fn phases<'a, S: Clone>(
    arena: &Arena,
    root: NodeId,
    state: S,
    record: &dyn Fn(NodeId) -> Option<&'a StyleRecord>,
    child_state: &mut dyn FnMut(NodeId, &S) -> S,
    content_root: &dyn Fn(NodeId) -> bool,
) -> Phases<S> {
    let mut out = Phases {
        negative: Vec::new(),
        flow: Vec::new(),
        zero: Vec::new(),
        positive: Vec::new(),
    };
    let chunk = arena.get(root).map(|n| n.chunk);
    let mut stack: Vec<(NodeId, S)> = Vec::new();
    let push_children = |stack: &mut Vec<(NodeId, S)>, id: NodeId, s: S| {
        if let Some(n) = arena.get(id) {
            for &c in n.children().iter().rev() {
                stack.push((c, s.clone()));
            }
        }
    };
    if !content_root(root) {
        push_children(&mut stack, root, state);
    }
    // (z-index, tree order, node, state) of the contexts and positioned nodes.
    let mut lifted: Vec<(i32, usize, NodeId, S)> = Vec::new();
    let mut seq = 0usize;
    while let Some((id, s)) = stack.pop() {
        seq += 1;
        let Some(n) = arena.get(id) else {
            continue;
        };
        // Service nodes have no box; text leaves are painted by their inline formatting root (a leaf that is
        // its own root, next to blocks, paints its text in the flow); the children of a layer root are
        // chunks of their own.
        let inline_leaf = n.class() == NodeClass::Text && !content_root(id);
        if n.class() == NodeClass::Service || inline_leaf || Some(n.chunk) != chunk {
            continue;
        }
        let Some(r) = record(id) else {
            continue;
        };
        if r.display == Display::None {
            continue;
        }
        if makes_context(r) {
            lifted.push((z_of(r), seq, id, s));
            continue;
        }
        if r.position != Position::Static {
            lifted.push((0, seq, id, s));
            continue;
        }
        let below = child_state(id, &s);
        out.flow.push((id, s));
        if !content_root(id) {
            push_children(&mut stack, id, below);
        }
    }
    lifted.sort_by_key(|(z, order, _, _)| (*z, *order));
    for (z, _, id, s) in lifted {
        match z.cmp(&0) {
            std::cmp::Ordering::Less => out.negative.push((id, s)),
            std::cmp::Ordering::Greater => out.positive.push((id, s)),
            std::cmp::Ordering::Equal => out.zero.push((id, s)),
        }
    }
    out
}
