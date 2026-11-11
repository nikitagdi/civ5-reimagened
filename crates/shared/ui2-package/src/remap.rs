//! Provisional ids to final ids.
//!
//! The builder hands out ids in insertion order, but sorted tables (strings, pictures, game asset
//! references) get their final positions only at `finish`. Every section that holds such an id
//! implements `Remapped`, and `finish` applies the map before encoding, so the insertion order of
//! sorted things never reaches the bytes.

use crate::ids::{ARefId, ImgId, StrId};

/// The maps from provisional to final ids; an empty map is the identity.
#[derive(Debug, Default)]
pub(crate) struct Remap {
    pub strs: Vec<u32>,
    pub imgs: Vec<u32>,
    pub arefs: Vec<u32>,
}

fn map(table: &[u32], id: u32) -> u32 {
    table.get(id as usize).copied().unwrap_or(id)
}

impl Remap {
    pub fn str(&self, id: &mut StrId) {
        id.0 = map(&self.strs, id.0);
    }

    pub fn opt_str(&self, id: &mut Option<StrId>) {
        if let Some(id) = id {
            self.str(id);
        }
    }

    pub fn img(&self, id: &mut ImgId) {
        id.0 = map(&self.imgs, id.0);
    }

    pub fn aref(&self, id: &mut ARefId) {
        id.0 = map(&self.arefs, id.0);
    }
}

/// A part of a package that holds provisional ids.
pub(crate) trait Remapped {
    /// Rewrites every provisional id with its final one.
    fn remap(&mut self, r: &Remap);
}
