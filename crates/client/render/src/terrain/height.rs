//! Terrain height at a point: the single source for everything that stands on the ground (units,
//! cities, labels). It follows the mesh of `mesh.rs`: flat land, the two-ring hill dome, the
//! mountain cone and flat water.

use glam::Vec2;

use super::mesh::{HILL_RING, HILL_TOP, LAND_HEIGHT, MOUNTAIN_TOP, WATER_HEIGHT};
use super::{Relief, TerrainSnapshot};
use crate::hexgrid::{HEX_WIDTH, cell_center, nearest_cell};

/// Distance of the cell edge from the centre.
const APOTHEM: f32 = HEX_WIDTH / 2.0;

/// Distance of a point (offset from the cell centre) from the centre in units of the cell: 0 at
/// the centre, 1 on the edge (the edge, not the corner, is at 1: hex rings scale with it).
pub fn hex_norm(offset: Vec2) -> f32 {
    const COS60: f32 = 0.5;
    const SIN60: f32 = 0.866_025_4;
    let a = offset.x.abs();
    let b = (offset.x * COS60 + offset.y * SIN60).abs();
    let c = (offset.x * COS60 - offset.y * SIN60).abs();
    a.max(b).max(c) / APOTHEM
}

/// Terrain height at an offset from the centre of a cell of this relief, following the mesh:
/// flat land, the two-ring hill, flat water. Mountains are not forested; they get the land height.
pub fn ground_height(relief: Relief, offset: Vec2) -> f32 {
    match relief {
        Relief::Water => WATER_HEIGHT,
        Relief::Hill => {
            let ring_height =
                LAND_HEIGHT + (HILL_TOP - LAND_HEIGHT) * (1.0 - HILL_RING * HILL_RING);
            let r = hex_norm(offset);
            if r <= HILL_RING {
                HILL_TOP + (ring_height - HILL_TOP) * (r / HILL_RING)
            } else {
                let t = ((r - HILL_RING) / (1.0 - HILL_RING)).min(1.0);
                ring_height + (LAND_HEIGHT - ring_height) * t
            }
        }
        Relief::Flat | Relief::Mountain => LAND_HEIGHT,
    }
}

/// Height over a cell of this relief at `offset` from its centre. Unlike
/// [`ground_height`] (trees), a mountain is its cone: linear from the peak to the rim.
pub fn relief_height(relief: Relief, offset: Vec2) -> f32 {
    match relief {
        Relief::Mountain => {
            LAND_HEIGHT + (MOUNTAIN_TOP - LAND_HEIGHT) * (1.0 - hex_norm(offset).min(1.0))
        }
        other => ground_height(other, offset),
    }
}

/// Height at `offset` from the centre of cell `(x, y)`; a cell off the map is plain land.
pub fn terrain_height(snapshot: &TerrainSnapshot, x: i32, y: i32, offset: Vec2) -> f32 {
    let relief = snapshot
        .shape()
        .normalize(x, y)
        .and_then(|(nx, ny)| snapshot.cell(nx, ny))
        .map_or(Relief::Flat, |c| c.drawn_relief());
    relief_height(relief, offset)
}

/// Height under the world point `(X, Z)` (unwrapped, as `cell_center` gives them).
pub fn terrain_height_at(snapshot: &TerrainSnapshot, p: Vec2) -> f32 {
    let (x, y) = nearest_cell(p);
    terrain_height(snapshot, x, y, p - cell_center(x, y))
}

#[cfg(test)]
mod tests {
    use super::super::mesh::{HILL_TOP, WATER_HEIGHT};
    use super::super::test_util::{land, map, water};
    use super::*;

    #[test]
    fn hex_norm_is_one_on_edges_and_not_more_at_corners() {
        assert!((hex_norm(Vec2::new(32.0, 0.0)) - 1.0).abs() < 1e-5);
        assert!((hex_norm(Vec2::new(-32.0, 0.0)) - 1.0).abs() < 1e-5);
        // Centre of the north-east edge.
        let ne = Vec2::new(16.0, 0.866_025_4 * 32.0);
        assert!((hex_norm(ne) - 1.0).abs() < 1e-4);
        // Corner at angle 30 degrees: on two edges, still 1.
        let corner = Vec2::new(32.0, -crate::hexgrid::CORNER_RADIUS * 0.5);
        assert!((hex_norm(corner) - 1.0).abs() < 1e-4);
        assert_eq!(hex_norm(Vec2::ZERO), 0.0);
    }

    #[test]
    fn heights_follow_the_mesh() {
        assert_eq!(
            ground_height(Relief::Flat, Vec2::new(5.0, 3.0)),
            LAND_HEIGHT
        );
        assert_eq!(ground_height(Relief::Water, Vec2::ZERO), WATER_HEIGHT);
        assert_eq!(ground_height(Relief::Hill, Vec2::ZERO), HILL_TOP);
        // On the rim the hill meets the land.
        assert!((ground_height(Relief::Hill, Vec2::new(32.0, 0.0)) - LAND_HEIGHT).abs() < 1e-4);
        // Falls monotonically from the top to the rim.
        let h = |x: f32| ground_height(Relief::Hill, Vec2::new(x, 0.0));
        assert!(h(0.0) > h(8.0) && h(8.0) > h(16.0) && h(16.0) > h(24.0) && h(24.0) > h(32.0));
    }

    #[test]
    fn heights_match_the_mesh_at_the_centre() {
        let s = map(3, 3, |x, y| match (x, y) {
            (0, 0) => TerrainCellExt::with(Relief::Hill),
            (1, 0) => TerrainCellExt::with(Relief::Mountain),
            (2, 0) => water(),
            _ => land(),
        });
        assert_eq!(terrain_height(&s, 0, 0, Vec2::ZERO), HILL_TOP);
        assert_eq!(terrain_height(&s, 1, 0, Vec2::ZERO), MOUNTAIN_TOP);
        assert_eq!(terrain_height(&s, 2, 0, Vec2::ZERO), WATER_HEIGHT);
        assert_eq!(terrain_height(&s, 0, 1, Vec2::ZERO), LAND_HEIGHT);
        // Off the map: plain land.
        assert_eq!(terrain_height(&s, 9, 9, Vec2::ZERO), LAND_HEIGHT);
    }

    #[test]
    fn mountain_falls_linearly_to_the_rim() {
        let h = |x: f32| relief_height(Relief::Mountain, Vec2::new(x, 0.0));
        assert_eq!(h(0.0), MOUNTAIN_TOP);
        assert!((h(32.0) - LAND_HEIGHT).abs() < 1e-3);
        assert!(h(8.0) > h(16.0) && h(16.0) > h(24.0));
    }

    #[test]
    fn point_lookup_agrees_with_the_cell_lookup() {
        let s = map(3, 3, |_, _| TerrainCellExt::with(Relief::Hill));
        let c = cell_center(1, 1);
        assert_eq!(terrain_height_at(&s, c), HILL_TOP);
        assert_eq!(
            terrain_height_at(&s, c + Vec2::new(10.0, 0.0)),
            terrain_height(&s, 1, 1, Vec2::new(10.0, 0.0))
        );
    }

    struct TerrainCellExt;
    impl TerrainCellExt {
        fn with(relief: Relief) -> super::super::TerrainCell {
            super::super::TerrainCell { relief, ..land() }
        }
    }
}
