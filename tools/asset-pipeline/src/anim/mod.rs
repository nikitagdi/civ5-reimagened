//! Animation conversion for the B2 models: the clip packs `C5A2` and the
//! idle state machines `C5FM` v2.
//!
//! SIMPLIFIED(SIMP-029): clips are nodes with a tolerance (0.1 degree, 0.05 % of the model size,
//! 0.1 %) checked against the reference sampler on the 30 Hz grid, not the Granny splines.

/// Largest angle between a packed rotation and the reference, degrees (see `Tolerance`). The
/// spec says 0.1; at 0.1 the units alone are 18.9 MB against a ceiling of 17 MB for every clip of
/// the game (the tolerance is relaxed and the report says so), so it is 0.3.
pub const DEFAULT_ROT_DEG: f64 = 0.3;

pub mod dge;
pub mod fsm;
pub mod idle;
pub mod resample;
