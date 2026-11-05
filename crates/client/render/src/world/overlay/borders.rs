//! The borders of the civilizations (world-overlays WOV4, RW-19, `CultureBorders`).
//!
//! The original follows the edges between the cells of a player and the rest, chains them into
//! closed lines (`TraverseBorder`, `AddControlPointsFromBorderList`), turns them into control
//! points (`GenerateControlPoints`, `GenerateHexSegment`), rounds the corners (`SmoothControlPoints`,
//! `ExpandControlPoints`), shakes them a little (`RandomizeControlPoints`) and builds a strip along
//! the spline: `GenerateMajorCivMesh` for a civilization (a solid band of the player's colour with a
//! wide soft fill of the second colour inside it), `GenerateMinorCivMesh` for a city state (a dashed
//! band). The strip goes into the map of the overlay through `VSOverlayCountry` and
//! `PSCountryBorder` (`country.wgsl`), once per player.
//!
//! The widths and alphas of the cross section are those of the `OverlayShaders::VSOverlayCountry`
//! draws of the capture S3 and of `CultureBorders.xml` (0.5 of soft edge, 1.4 solid at alpha 200,
//! 0.3 of soft edge, the fill of 20 at alpha 90 fading to nothing); the control points, their
//! smoothing, expansion and shake, the B-spline and the city state's band are the exe's (RE-B18 item
//! 3 BRD-1..3).
//!
//! The files: `points.rs` (edges to loops), `culture.rs` (the control points, their passes and the
//! B-spline), `smooth.rs` (the Catmull-Rom of the highlights), `mesh.rs` (the strips). This file holds the style, the vertex and the per-player meshes,
//! rebuilt only for the players whose cells or colours changed.

pub mod culture;
pub mod gpu;
pub mod mesh;
pub mod points;
pub mod smooth;

use std::collections::{BTreeMap, BTreeSet};

use bytemuck::{Pod, Zeroable};

use super::OwnersInput;
use crate::blend::{BlendMesh, BlendVertex, NO_FEATHER, srgb_to_linear};
use crate::hexgrid::GridShape;
use crate::terrain::mesh::{LAND_HEIGHT, WATER_HEIGHT};
use crate::terrain::{Relief, TerrainSnapshot};

/// The look of the borders: `CultureBorders.xml`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BorderStyle {
    pub primary_width: f32,
    pub secondary_width: f32,
    pub primary_aa_width: f32,
    pub secondary_aa_width: f32,
    /// Alphas are bytes, as in the file.
    pub primary_alpha: f32,
    pub secondary_alpha: f32,
    pub corner_bias: f32,
    pub cs_primary_width: f32,
    pub cs_outline_width: f32,
    pub cs_aa_width: f32,
    pub cs_primary_alpha: f32,
    pub cs_outline_alpha: f32,
    pub cs_dash_width: f32,
    pub randomness: f32,
    pub expansion: f32,
}

impl Default for BorderStyle {
    /// What the game ships.
    // re: Assets/Overlays/CultureBorders.xml:17-28
    fn default() -> Self {
        Self {
            primary_width: 1.4,
            secondary_width: 20.0,
            primary_aa_width: 0.5,
            secondary_aa_width: 0.3,
            primary_alpha: 200.0,
            secondary_alpha: 90.0,
            corner_bias: 0.105,
            cs_primary_width: 2.8,
            cs_outline_width: 0.4,
            cs_aa_width: 0.5,
            cs_primary_alpha: 130.0,
            cs_outline_alpha: 160.0,
            cs_dash_width: 8.0,
            randomness: 6.0,
            expansion: 2.0,
        }
    }
}

impl BorderStyle {
    /// From the settings of `tb:world-b2`; a value the file does not give (zero) keeps the default.
    pub fn from_settings(file: &asset_format::CultureBorders) -> Self {
        let d = Self::default();
        let pick = |v: f32, default: f32| if v > 0.0 { v } else { default };
        Self {
            primary_width: pick(file.primary_width, d.primary_width),
            secondary_width: pick(file.secondary_width, d.secondary_width),
            primary_aa_width: pick(file.primary_aa_width, d.primary_aa_width),
            secondary_aa_width: pick(file.secondary_aa_width, d.secondary_aa_width),
            primary_alpha: pick(file.primary_alpha, d.primary_alpha),
            secondary_alpha: pick(file.secondary_alpha, d.secondary_alpha),
            corner_bias: pick(file.corner_bias, d.corner_bias),
            cs_primary_width: pick(file.cs_primary_width, d.cs_primary_width),
            cs_outline_width: pick(file.cs_outline_width, d.cs_outline_width),
            cs_aa_width: pick(file.cs_aa_width, d.cs_aa_width),
            cs_primary_alpha: pick(file.cs_primary_alpha, d.cs_primary_alpha),
            cs_outline_alpha: pick(file.cs_outline_alpha, d.cs_outline_alpha),
            cs_dash_width: pick(file.cs_dash_width, d.cs_dash_width),
            // Zero randomness is a valid value: no shake.
            randomness: file.randomness.max(0.0),
            expansion: pick(file.expansion, d.expansion),
        }
    }
}

/// A vertex of a border mesh: `POSITION` float2 and `COLOR` four bytes, the layout of the original's
/// buffers (the draws of S3: stride 12). The position is in our world's ground plane, `(X, Z)`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct BorderVertex {
    pub position: [f32; 2],
    pub color: [u8; 4],
}

/// The mesh of one player: the vertices, the triangles, and the cell each vertex belongs to (the
/// fog reads it in the line that B1 draws over the ground when the pass `ScreenOverlay` does not run).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OwnerMesh {
    pub vertices: Vec<BorderVertex>,
    pub indices: Vec<u32>,
    pub cells: Vec<u32>,
}

impl OwnerMesh {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// All the owners' meshes, kept between frames.
#[derive(Debug, Default)]
pub struct Borders {
    style: BorderStyle,
    meshes: BTreeMap<i8, OwnerMesh>,
    /// Meshes built so far (each owner rebuilt counts one).
    builds: u64,
}

impl Borders {
    pub fn style(&self) -> &BorderStyle {
        &self.style
    }

    /// Sets the style; true if it differs (every mesh is then out of date).
    pub fn set_style(&mut self, style: BorderStyle) -> bool {
        let changed = self.style != style;
        self.style = style;
        changed
    }

    /// Meshes built so far.
    pub fn builds(&self) -> u64 {
        self.builds
    }

    /// Indices of all the meshes.
    pub fn index_count(&self) -> u32 {
        self.meshes.values().map(|m| m.indices.len() as u32).sum()
    }

    pub fn mesh(&self, owner: i8) -> Option<&OwnerMesh> {
        self.meshes.get(&owner)
    }

    pub fn clear(&mut self) {
        self.meshes.clear();
    }

    /// Builds the meshes of `which` (or of every owner of the input with `None`) and forgets those of
    /// owners that have no cell any more. Returns how many meshes were built.
    pub fn rebuild(
        &mut self,
        input: &OwnersInput,
        shape: &GridShape,
        which: Option<&BTreeSet<i8>>,
    ) -> usize {
        let present: BTreeSet<i8> = input.owners.iter().copied().filter(|&o| o >= 0).collect();
        self.meshes.retain(|o, _| present.contains(o));
        let todo: Vec<i8> = match which {
            None => present.iter().copied().collect(),
            Some(set) => set
                .iter()
                .copied()
                .filter(|o| present.contains(o))
                .collect(),
        };
        for &owner in &todo {
            let mesh = mesh::owner_mesh(input, shape, owner, &self.style);
            self.meshes.insert(owner, mesh);
        }
        self.builds += todo.len() as u64;
        todo.len()
    }

    /// Every mesh in one: vertices, triangles (owner by owner) and the cell of each vertex.
    pub fn merged(&self) -> OwnerMesh {
        let mut out = OwnerMesh::default();
        for m in self.meshes.values() {
            let base = out.vertices.len() as u32;
            out.vertices.extend_from_slice(&m.vertices);
            out.cells.extend_from_slice(&m.cells);
            out.indices.extend(m.indices.iter().map(|i| i + base));
        }
        out
    }
}

/// Height of the line over the ground where the pass `ScreenOverlay` does not run; hills and
/// mountains rise there, so the line is lifted by a fixed amount per relief.
// SIMPLIFIED(SIMP-006): fixed lift per relief instead of sampling the own dome and cone; only the
// line that B1 draws over the ground (no cells of the terrain for the pass) has it, the borders of
// the pass lie in the map of the overlay
fn fallback_height(relief: Relief) -> f32 {
    match relief {
        Relief::Flat => LAND_HEIGHT + 0.3,
        Relief::Hill => LAND_HEIGHT + 5.0,
        Relief::Mountain => LAND_HEIGHT + 9.0,
        Relief::Water => WATER_HEIGHT + 0.3,
    }
}

/// The mesh as triangles of the blend pipeline, lifted onto the relief of `ground`: the line over
/// the ground that B1 draws where the pass `ScreenOverlay` does not run. Appends to `out.borders`.
pub fn blend_fallback(merged: &OwnerMesh, ground: &TerrainSnapshot, out: &mut BlendMesh) {
    let base = out.vertices.len() as u32;
    for (v, &cell) in merged.vertices.iter().zip(&merged.cells) {
        let relief = ground
            .cells()
            .get(cell as usize)
            .map_or(Relief::Flat, |c| c.drawn_relief());
        let [r, g, b] = srgb_to_linear([v.color[0], v.color[1], v.color[2]]);
        out.vertices.push(BlendVertex {
            position: [v.position[0], fallback_height(relief), v.position[1]],
            across: NO_FEATHER,
            color: [r, g, b, f32::from(v.color[3]) / 255.0],
            cell,
        });
    }
    out.borders.extend(merged.indices.iter().map(|i| i + base));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fallback_lifts_the_mesh_onto_the_relief_with_the_alphas() {
        let mesh = OwnerMesh {
            vertices: vec![
                BorderVertex {
                    position: [1.0, 2.0],
                    color: [255, 0, 0, 0],
                },
                BorderVertex {
                    position: [3.0, 4.0],
                    color: [255, 0, 0, 200],
                },
                BorderVertex {
                    position: [5.0, 6.0],
                    color: [255, 0, 0, 200],
                },
            ],
            indices: vec![0, 1, 2],
            cells: vec![0, 0, 1],
        };
        let ground = crate::terrain::test_util::map(2, 1, |x, _| crate::terrain::TerrainCell {
            relief: if x == 0 {
                Relief::Flat
            } else {
                Relief::Mountain
            },
            ..crate::terrain::test_util::land()
        });
        let mut out = BlendMesh::default();
        out.vertices
            .push(out.vertices.first().copied().unwrap_or(BlendVertex {
                position: [0.0; 3],
                across: 0.0,
                color: [0.0; 4],
                cell: 0,
            }));
        blend_fallback(&mesh, &ground, &mut out);
        assert_eq!(out.borders, vec![1, 2, 3]);
        assert!(out.indices_valid());
        // x and z are the plane's, y the relief's.
        assert_eq!(&out.vertices[1].position[0..1], &[1.0]);
        assert_eq!(out.vertices[1].position[2], 2.0);
        assert!(out.vertices[3].position[1] > out.vertices[1].position[1]);
        assert_eq!(out.vertices[1].color[3], 0.0);
        assert!((out.vertices[2].color[3] - 200.0 / 255.0).abs() < 1e-6);
        assert_eq!(out.vertices[3].cell, 1);
    }

    #[test]
    fn the_style_is_what_the_game_ships_and_the_file_overrides_it() {
        let d = BorderStyle::default();
        assert_eq!(
            (d.primary_width, d.secondary_width, d.primary_alpha),
            (1.4, 20.0, 200.0)
        );
        assert_eq!((d.cs_dash_width, d.randomness), (8.0, 6.0));
        let file = asset_format::CultureBorders {
            primary_width: 3.0,
            ..Default::default()
        };
        let s = BorderStyle::from_settings(&file);
        assert_eq!(s.primary_width, 3.0);
        // A value the file does not give is the default.
        assert_eq!(s.secondary_width, 20.0);
    }

    #[test]
    fn the_vertex_is_the_originals_twelve_bytes() {
        assert_eq!(std::mem::size_of::<BorderVertex>(), 12);
    }
}
