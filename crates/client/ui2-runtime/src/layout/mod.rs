//! Box layout with taffy over the node arena.
//!
//! The arena implements taffy's low-level tree traits itself (no `TaffyTree`): the cache and the result live in
//! each node's [`LayoutSlot`](crate::arena::LayoutSlot), children are the arena's children, and a text leaf
//! answers its own measurement (with baselines, which `TaffyTree`'s measure function cannot return). Rounding
//! is never run: positions stay fractional and are snapped to device pixels when painting.

pub mod tree;

pub use tree::{LayoutStyles, compute_root};
