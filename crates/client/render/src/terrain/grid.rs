//! Hex grid overlay: the outline of every hex, white at alpha 75/255. The
//! renderer draws it only while the grid is switched on (G key).

use super::mesh::{LAND_HEIGHT, WATER_HEIGHT};
use super::{Relief, TerrainSnapshot, edge, lift};
use crate::blend::{BlendMesh, Layer, NO_FEATHER, srgb_to_linear};
use crate::hexgrid::Dir;

/// RGBA 255,255,255,75.
// re: HexGrid.xml:43
pub const GRID_RGBA: [u8; 4] = [255, 255, 255, 75];
/// Line width in world units: about a pixel at the default zoom.
// SIMPLIFIED(SIMP-020): width chosen by eye, the original draws a pixel-wide line
pub const GRID_WIDTH: f32 = 1.2;
/// Lift over the surface the line sits on.
const GRID_LIFT: f32 = 0.3;

pub fn build(snapshot: &TerrainSnapshot, out: &mut BlendMesh) {
    let shape = snapshot.shape();
    let [r, g, b] = srgb_to_linear([GRID_RGBA[0], GRID_RGBA[1], GRID_RGBA[2]]);
    let color = [r, g, b, f32::from(GRID_RGBA[3]) / 255.0];
    let half = GRID_WIDTH * 0.5;
    for y in 0..shape.height {
        for x in 0..shape.width {
            let Some(index) = snapshot.index(x, y) else {
                continue;
            };
            let cell = &snapshot.cells()[index];
            for dir in Dir::ALL {
                let neighbor = shape.neighbor(x, y, dir);
                // A shared edge belongs to the cell that sees it as NE, E or SE; edges on the
                // map outline belong to their only cell.
                let owns = matches!(dir, Dir::NorthEast | Dir::East | Dir::SouthEast)
                    || neighbor.is_none();
                if !owns {
                    continue;
                }
                let neighbor_land = neighbor
                    .and_then(|(nx, ny)| snapshot.cell(nx, ny))
                    .is_some_and(|n| n.drawn_relief() != Relief::Water);
                // The rim of land stays at the land height, also next to water.
                let any_land = cell.drawn_relief() != Relief::Water || neighbor_land;
                let h = GRID_LIFT + if any_land { LAND_HEIGHT } else { WATER_HEIGHT };
                let e = edge(x, y, dir);
                let step = e.inward * half;
                out.quad(
                    Layer::Grid,
                    [
                        lift(e.a - step, h),
                        lift(e.b - step, h),
                        lift(e.b + step, h),
                        lift(e.a + step, h),
                    ],
                    [NO_FEATHER; 4],
                    color,
                    index as u32,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::terrain::TerrainSnapshot;
    use crate::terrain::test_util::{land, map};

    fn grid_of(snap: &TerrainSnapshot) -> BlendMesh {
        let mut m = BlendMesh::default();
        build(snap, &mut m);
        assert!(m.indices_valid());
        m
    }

    /// Distinct edges of the map by rounded midpoint; on a wrapping map X is taken modulo the
    /// world width.
    fn unique_edges(snap: &TerrainSnapshot) -> usize {
        let shape = snap.shape();
        let period = (shape.world_width() * 2.0).round() as i64;
        let mut set = HashSet::new();
        for y in 0..shape.height {
            for x in 0..shape.width {
                for d in Dir::ALL {
                    let e = edge(x, y, d);
                    let mid = (e.a + e.b) * 0.5;
                    let mut key = [(mid.x * 2.0).round() as i64, (mid.y * 2.0).round() as i64];
                    if snap.wrap_x() {
                        key[0] = key[0].rem_euclid(period);
                    }
                    set.insert(key);
                }
            }
        }
        set.len()
    }

    #[test]
    fn every_edge_is_drawn_exactly_once() {
        let snap = map(6, 5, |_, _| land());
        let m = grid_of(&snap);
        assert_eq!(m.grid.len() / 6, unique_edges(&snap));
        assert!(m.rivers.is_empty() && m.borders.is_empty());
    }

    #[test]
    fn wrapping_map_has_no_duplicate_seam_edges() {
        let snap = TerrainSnapshot::new(6, 4, true, vec![land(); 6 * 4]).unwrap();
        assert_eq!(grid_of(&snap).grid.len() / 6, unique_edges(&snap));
    }

    #[test]
    fn colour_is_white_with_the_original_alpha() {
        let m = grid_of(&map(2, 2, |_, _| land()));
        for v in &m.vertices {
            assert!(v.color[..3].iter().all(|&c| (c - 1.0).abs() < 1e-5));
            assert!((v.color[3] - 75.0 / 255.0).abs() < 1e-5);
        }
    }
}
