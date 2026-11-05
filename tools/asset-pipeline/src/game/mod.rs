//! game-source: reading a Civ5 install (FPK packs, loose files, DLC overlays). Read-only.

pub mod fpk;
pub mod source;
pub mod steam;

pub use source::{Entry, GameError, GameSource, Layer, Origin};
