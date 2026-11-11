//! Painting: the draw lists of the chunks, handed to the scene of `ui2-draw`.
//!
//! A *chunk* is a retained draw list with its own revision: one per top-level node and per child of a layer
//! root (a panel, a screen; later a tooltip or a map anchor), identified by the slot index of its root node
//! ([`Node::chunk`](crate::arena::Node::chunk)) and drawn in the layer of its layer root. The paint pass is
//! driven by the PAINT queue: every queued node names its chunk, each named chunk is built again once
//! ([`emit`]) and set on the scene, and chunks whose root was removed are removed from the scene. An idle
//! frame finds the queue empty and does not touch the scene.
//!
//! The order of items inside a chunk follows CSS stacking ([`stacking`]); coordinates are device pixels.
//! What is not drawn (blurred inset shadows, text shadows, corners of different ellipses, borders of several
//! colors, game pictures the host cannot name) is reported once per node and left out.

pub(crate) mod emit;
pub(crate) mod stacking;

use ui2_draw::{ChunkId, ChunkItems, Scene};

use crate::arena::{Arena, Dirty, NodeId};
use crate::error::ErrorSink;
pub(crate) use emit::PaintCtx;

/// What one paint pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PassStats {
    /// Chunks built and set on the scene.
    pub chunks: u32,
    /// Chunks removed from the scene.
    pub removed: u32,
    /// Items of the chunks built.
    pub items: u32,
    /// Glyphs of those items.
    pub glyphs: u32,
    /// Queued nodes looked at.
    pub visited: u32,
}

/// The root of a chunk, if it is alive, still the root and in the tree (not a detached `if` body).
fn chunk_root(arena: &Arena, chunk: u32) -> Option<NodeId> {
    let id = arena.id_at(chunk as usize)?;
    let n = arena.get(id)?;
    (n.chunk == chunk && !n.detached).then_some(id)
}

/// The items of one chunk, built from the current state (also used by tests and tools).
pub(crate) fn build(
    arena: &Arena,
    cx: &PaintCtx<'_>,
    errors: &mut ErrorSink,
    root: NodeId,
) -> (ChunkItems, emit::Emitted) {
    emit::Emitter::new(arena, cx, errors).chunk(root)
}

/// The paint pass.
pub(crate) fn pass(
    arena: &mut Arena,
    cx: &PaintCtx<'_>,
    scene: &mut Scene,
    errors: &mut ErrorSink,
) -> PassStats {
    let mut stats = PassStats::default();
    if arena.pending(Dirty::PAINT) == 0 && !arena.has_removed_chunks() {
        return stats;
    }
    for c in arena.take_removed_chunks() {
        scene.remove_chunk(c);
        stats.removed += 1;
    }
    let mut chunks: Vec<u32> = Vec::new();
    for id in arena.take_dirty(Dirty::PAINT) {
        stats.visited += 1;
        if let Some(n) = arena.get(id) {
            chunks.push(n.chunk);
        }
    }
    chunks.sort_unstable();
    chunks.dedup();
    for c in chunks {
        let Some(root) = chunk_root(arena, c) else {
            continue;
        };
        let layer = emit::layer_of(arena, cx.pkg, root);
        let (items, emitted) = build(arena, cx, errors, root);
        scene.set_chunk(layer, ChunkId(c), items);
        stats.chunks += 1;
        stats.items += emitted.items;
        stats.glyphs += emitted.glyphs;
    }
    stats
}
