//! Water surface (SIMP-008): what the water pass needs from the map, without a GPU.
//!
//! The water cells are drawn twice: first as the seabed (the coast or ocean texture of the terrain
//! pass, at the water plane [`WATER_HEIGHT`](super::mesh::WATER_HEIGHT)), then again by the water
//! pass as a tinted, semi-transparent surface over it. How opaque and how dark that surface is
//! depends on a depth value `0..=1` carried by every water vertex: [`SHORE_DEPTH`] on the shore, [`COAST_DEPTH`]
//! in the coast cells, 1 in the open ocean. The values are a function of the corner (or of the
//! cell for its centre) only, so neighbouring cells agree along a shared edge and the colour
//! changes smoothly across the map.
//!
//! SIMPLIFIED(SIMP-008): the original water is `WaterSystem` with its own shaders (reflection of
//! the sky, ocean currents, coast splines of `coaststyles.xml`), which are only in the exe. Ours
//! is one flat plane: the colour ramp of the game's `waterdepthcolor.dds` (sampled at a few depths
//! and written into `water.wgsl`), the game's `waterbumps.dds` as two scrolling bump layers, a
//! sun glint and a foam line on the shore.

use glam::Vec2;

use super::mesh::TerrainVertex;
use super::{Relief, TerrainKind, TerrainSnapshot};
use crate::hexgrid::{Dir, cell_center, corner_offset};

/// Logical id of the bump map of the water surface (`TerrainTextures.fpk`, set `b1-terrain`).
pub const WATER_BUMP_TEXTURE: &str = "tx:waterbumps.dds";

/// Depth at a corner that touches land. Not 0: a coast cell next to land has most of its corners
/// there, and the seabed would show unchanged; the colour ramp of the water is transparent below
/// about 0.2 of its length.
pub const SHORE_DEPTH: f32 = 0.45;

/// Depth of the coast cells (and of the corners where a coast cell meets only water). Open ocean
/// is 1, a corner that touches land is [`SHORE_DEPTH`] (and has `shore` 1, for the foam).
pub const COAST_DEPTH: f32 = 0.6;

/// Dir whose edge starts at corner `i` (see `hexgrid::edge_corners`).
const EDGE_STARTING_AT: [Dir; 6] = [
    Dir::NorthEast,
    Dir::NorthWest,
    Dir::West,
    Dir::SouthWest,
    Dir::SouthEast,
    Dir::East,
];

fn is_land(snapshot: &TerrainSnapshot, x: i32, y: i32) -> bool {
    snapshot
        .cell(x, y)
        .is_some_and(|c| c.drawn_relief() != Relief::Water)
}

fn is_coast(snapshot: &TerrainSnapshot, x: i32, y: i32) -> bool {
    snapshot
        .cell(x, y)
        .is_some_and(|c| c.drawn_relief() == Relief::Water && matches!(c.kind, TerrainKind::Coast))
}

/// Depth of the water at the centre of water cell `(x, y)`.
pub fn centre_depth(snapshot: &TerrainSnapshot, x: i32, y: i32) -> f32 {
    if is_coast(snapshot, x, y) {
        COAST_DEPTH
    } else {
        1.0
    }
}

/// Depth of the water at corner `corner` of cell `(x, y)`: [`SHORE_DEPTH`] where it touches land, else
/// [`COAST_DEPTH`] where it touches a coast cell, else 1. The two other cells at a corner are
/// the neighbours across the two edges that meet there.
pub fn corner_depth(snapshot: &TerrainSnapshot, x: i32, y: i32, corner: usize) -> f32 {
    let shape = snapshot.shape();
    let dirs = [
        EDGE_STARTING_AT[corner % 6],
        EDGE_STARTING_AT[(corner + 5) % 6],
    ];
    let mut depth = centre_depth(snapshot, x, y);
    for dir in dirs {
        let Some((nx, ny)) = shape.neighbor(x, y, dir) else {
            continue;
        };
        if is_land(snapshot, nx, ny) {
            return SHORE_DEPTH;
        }
        if is_coast(snapshot, nx, ny) {
            depth = depth.min(COAST_DEPTH);
        }
    }
    depth
}

/// Depth and shore weight (1 at a corner that touches land, 0 elsewhere) of mesh vertex `v` of the
/// water cell it belongs to; `(0, 0)` for anything else (land and skirts do not use them).
pub fn vertex_depth(snapshot: &TerrainSnapshot, v: &TerrainVertex) -> (f32, f32) {
    let w = snapshot.width() as usize;
    let idx = v.cell as usize;
    let (x, y) = ((idx % w) as i32, (idx / w) as i32);
    match snapshot.cell(x, y) {
        Some(c) if c.drawn_relief() == Relief::Water => {}
        _ => return (0.0, 0.0),
    }
    let off = Vec2::new(v.position[0], v.position[2]) - cell_center(x, y);
    if off.length() < 1.0 {
        return (centre_depth(snapshot, x, y), 0.0);
    }
    let corner = (0..6)
        .min_by(|&a, &b| {
            let da = off.distance_squared(corner_offset(a));
            let db = off.distance_squared(corner_offset(b));
            da.total_cmp(&db)
        })
        .unwrap_or(0);
    let depth = corner_depth(snapshot, x, y, corner);
    (depth, if depth == SHORE_DEPTH { 1.0 } else { 0.0 })
}

#[cfg(test)]
mod tests {
    use super::super::mesh::TerrainMesh;
    use super::super::test_util::{land, map, water};
    use super::*;
    use crate::terrain::TerrainCell;

    fn coast() -> TerrainCell {
        TerrainCell {
            kind: TerrainKind::Coast,
            ..water()
        }
    }

    #[test]
    fn open_ocean_is_deep_and_the_shore_is_shallow() {
        // 5x1: land at x=0, coast at 1, ocean at 2..4.
        let s = map(5, 1, |x, _| match x {
            0 => land(),
            1 => coast(),
            _ => water(),
        });
        assert_eq!(centre_depth(&s, 1, 0), COAST_DEPTH);
        assert_eq!(centre_depth(&s, 3, 0), 1.0);
        // The corners of the coast cell facing the land touch it.
        let (a, b) = crate::hexgrid::edge_corners(Dir::West);
        assert_eq!(corner_depth(&s, 1, 0, a), SHORE_DEPTH);
        assert_eq!(corner_depth(&s, 1, 0, b), SHORE_DEPTH);
        // The far side of an ocean cell next to the coast is the coast value, deeper ocean is 1.
        let (a, _) = crate::hexgrid::edge_corners(Dir::West);
        assert_eq!(corner_depth(&s, 2, 0, a), COAST_DEPTH);
        let (a, _) = crate::hexgrid::edge_corners(Dir::East);
        assert_eq!(corner_depth(&s, 3, 0, a), 1.0);
    }

    #[test]
    fn neighbours_agree_at_shared_corners() {
        let s = map(6, 6, |x, y| match (x + 2 * y) % 5 {
            0 => land(),
            1 | 2 => coast(),
            _ => water(),
        });
        let mesh = TerrainMesh::build(&s);
        // Water vertices at the same world position get the same depth.
        let mut seen: std::collections::HashMap<[i32; 2], f32> = Default::default();
        for v in &mesh.vertices {
            let (x, y) = (v.cell as i32 % 6, v.cell as i32 / 6);
            if s.cell(x, y).unwrap().drawn_relief() != Relief::Water {
                continue;
            }
            let key = [
                (v.position[0] * 10.0).round() as i32,
                (v.position[2] * 10.0).round() as i32,
            ];
            let d = vertex_depth(&s, v).0;
            if let Some(&old) = seen.get(&key) {
                assert_eq!(old, d, "depth differs at {key:?}");
            }
            seen.insert(key, d);
        }
        assert!(!seen.is_empty());
    }

    #[test]
    fn land_vertices_have_no_depth() {
        let s = map(2, 2, |_, _| land());
        let mesh = TerrainMesh::build(&s);
        assert!(
            mesh.vertices
                .iter()
                .all(|v| vertex_depth(&s, v) == (0.0, 0.0))
        );
    }
}
