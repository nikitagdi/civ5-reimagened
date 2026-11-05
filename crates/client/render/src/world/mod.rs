//! Subsystem stubs of render-core. One file per subsystem; the `world-*` modules
//! write the bodies. `WorldSystems` in `frame/systems.rs` holds them.

pub mod arrows;
pub(crate) mod b1_env;
pub mod city;
pub mod decal;
pub mod distance_fog;
pub mod far_lod;
pub mod forest;
pub mod fow;
pub mod god_ray;
pub mod ice;
pub mod landmark;
pub mod minimap;
pub mod objects;
pub mod overlay;
pub mod particle;
pub mod path;
pub mod river;
pub mod shadows;
pub mod terrain;
pub mod trade;
pub mod unit;
pub mod water;
pub mod waves;
pub mod yield_icons;
