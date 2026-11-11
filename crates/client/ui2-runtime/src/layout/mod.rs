//! Box layout with taffy over the node arena.
//!
//! The arena implements taffy's low-level tree traits itself (no `TaffyTree`): the cache and the result live in
//! each node's [`LayoutSlot`](crate::arena::LayoutSlot), children are the arena's children, and an inline
//! formatting root answers its own measurement (with baselines, which `TaffyTree`'s measure function cannot
//! return). Rounding is never run: positions stay fractional and are snapped to device pixels when painting.
//!
//! The pass is driven by the LAYOUT queue: each queued node clears its cache and its ancestors' up to the
//! document root, and every root above a queued node is laid out again (clean subtrees answer from their
//! caches). Nodes whose box changed are queued for paint. No queued node, no work.

pub mod style;
pub mod tree;

use std::collections::HashSet;

use taffy::{AvailableSpace, Size};
use ui2_text::TextMeasure;

use crate::arena::{Arena, Dirty, NodeId};

pub use tree::{LayoutStyles, compute_root};

/// What one layout pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PassStats {
    /// Roots laid out.
    pub runs: u32,
    /// Nodes whose cache was cleared.
    pub visited: u32,
}

/// The layout pass over the queued nodes, in a viewport of CSS px.
pub(crate) fn pass(
    arena: &mut Arena,
    styles: &LayoutStyles,
    text: &dyn TextMeasure,
    viewport: [f32; 2],
) -> PassStats {
    let mut stats = PassStats::default();
    if arena.pending(Dirty::LAYOUT) == 0 {
        return stats;
    }
    let mut cleared: HashSet<u32> = HashSet::new();
    let mut roots: Vec<NodeId> = Vec::new();
    for id in arena.take_dirty(Dirty::LAYOUT) {
        let mut at = Some(id);
        while let Some(n) = at {
            if !cleared.insert(n.index()) {
                break;
            }
            let Some(node) = arena.get_mut(n) else {
                break;
            };
            stats.visited += 1;
            node.layout.cache.clear();
            at = node.parent;
            if at.is_none() {
                roots.push(n);
            }
        }
    }
    roots.sort();
    roots.dedup();
    let available = Size {
        width: AvailableSpace::Definite(viewport[0]),
        height: AvailableSpace::Definite(viewport[1]),
    };
    for root in roots {
        let changed = compute_root(arena, styles, text, root, available);
        stats.runs += 1;
        for c in changed {
            arena.mark(c, Dirty::PAINT);
        }
    }
    stats
}
