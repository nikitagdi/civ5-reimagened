//! Rivers of `world-water`: the stage `Rivers` of the terrain build, the water of
//! the rivers and their current.
//!
//! The simulation (`cvRiverSimSystem`) works in the texel plane of the terrain field: `x`, `y` of a
//! point are texels, `z` is a raw height (0..65535) once [`sim`] has set it.

pub mod banks;
pub mod blend;
pub mod draw;
pub mod floodplains;
pub mod hex;
pub mod marsh;
pub mod mesh;
pub mod sim;
pub mod spline_db;
pub mod stage;
pub mod stencil;
pub mod wavelet;

use glam::Vec3;

use self::hex::HexEdge;
use self::spline_db::SplineDb;

/// Control points of one river, or of one of its banks.
// re: cvRiverSimSystem.h `RiverSimSpline`
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RiverSpline {
    /// `x`, `y` in texels; `z` is 0 until the heights are set.
    pub points: Vec<Vec3>,
    /// Set by `MarkRiverMouths` when the last edge has no river downstream.
    pub ends_at_mouth: bool,
}

/// What `cvRiverSimSystem::StartupGame` makes.
#[derive(Clone, Debug, Default)]
pub struct RiverSim {
    /// The edges of every river, from the source down (`aRiverEdges`).
    pub edges: Vec<Vec<HexEdge>>,
    pub rivers: Vec<RiverSpline>,
    pub left_banks: Vec<RiverSpline>,
    pub right_banks: Vec<RiverSpline>,
    /// The height (raw, as a float) of every control point of every river, never rising downstream.
    pub heights: Vec<Vec<f32>>,
    /// The spline segments of the hex sides (`GameSplineDatabase`), for the floodplains.
    pub db: SplineDb,
}
