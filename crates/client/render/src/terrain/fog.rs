//! The fog of war moved to `world::fow::state` (WOV0): its state texture, cloud mask and constants
//! belong to the `FluidFow` subsystem. This path stays for the files that still import it; they
//! move to the new path with their own streams, and then this file goes.

pub use crate::world::fow::state::*;
