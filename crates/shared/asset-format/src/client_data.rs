//! Small data blobs the client reads once at start, produced by `asset-pipeline` (set `client-data`)
//! from the user's own install: the mouse cursors (`CursorSet`, from the `.ani` files of
//! `Assets/UI/Cursors`) and the unit animation blend graphs (`data:dge-graphs`, the `.dge` files of
//! `Resource/Common/UnitModels.fpk` as one text table, read by `render::anim::dge`).
//!
//! Neither is in the repository: without them the client shows the browser's default cursor and the
//! units play the clips of their action (the fallback of `render::anim::graph`).

use serde::{Deserialize, Serialize};

use crate::{FormatError, blob};

/// Manifest id of the cursor set ([`CursorSet`]).
pub const CURSORS_ID: &str = "data:cursors";
/// Manifest id of the blend graph table (UTF-8 text, the format `render::anim::dge` parses).
pub const DGE_GRAPHS_ID: &str = "data:dge-graphs";

pub const CURSORS_MAGIC: [u8; 4] = *b"C5CU";

/// One animated cursor: frames of `size`×`size` RGBA8, top row first, one after another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorArt {
    /// `pointer`, `waiting`, `go_to`, ... (the names the client looks the cursors up by).
    pub name: String,
    pub hot: (u16, u16),
    /// Milliseconds a frame shows.
    pub frame_ms: u16,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorSet {
    /// Side of every frame in pixels.
    pub size: u16,
    pub cursors: Vec<CursorArt>,
}

impl CursorSet {
    pub const VERSION: u16 = 1;

    pub fn frame_bytes(&self) -> usize {
        usize::from(self.size) * usize::from(self.size) * 4
    }

    pub fn encode(&self) -> Vec<u8> {
        blob::encode_v(CURSORS_MAGIC, Self::VERSION, self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (_, set): (u16, CursorSet) =
            blob::decode_range(CURSORS_MAGIC, Self::VERSION, Self::VERSION, bytes)?;
        set.validate()?;
        Ok(set)
    }

    /// Every cursor has at least one whole frame.
    pub fn validate(&self) -> Result<(), FormatError> {
        let fb = self.frame_bytes();
        if fb == 0 {
            return Err(FormatError::invalid("cursor size 0"));
        }
        for c in &self.cursors {
            if c.rgba.is_empty() || c.rgba.len() % fb != 0 {
                return Err(FormatError::invalid(format!(
                    "cursor {}: {} bytes are not whole frames of {fb}",
                    c.name,
                    c.rgba.len()
                )));
            }
            if c.frame_ms == 0 {
                return Err(FormatError::invalid(format!(
                    "cursor {}: frame of 0 ms",
                    c.name
                )));
            }
        }
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&CursorArt> {
        self.cursors.iter().find(|c| c.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cursor_set_round_trips_and_checks_its_frames() {
        let set = CursorSet {
            size: 2,
            cursors: vec![CursorArt {
                name: "pointer".into(),
                hot: (1, 0),
                frame_ms: 100,
                rgba: vec![7; 2 * 2 * 4 * 3],
            }],
        };
        let back = CursorSet::decode(&set.encode()).unwrap();
        assert_eq!(back, set);
        assert_eq!(
            back.get("pointer").unwrap().rgba.len() / back.frame_bytes(),
            3
        );
        let mut bad = set.clone();
        bad.cursors[0].rgba.pop();
        assert!(CursorSet::decode(&bad.encode()).is_err());
    }
}
