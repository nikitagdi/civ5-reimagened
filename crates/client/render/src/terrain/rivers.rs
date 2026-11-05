//! Rivers (SIMP-007): a strip along each hex edge that has a river bit.
//!
//! Each cell draws the half of the strip lying inside itself, and, when the neighbour is land and
//! does not draw its own half, also the half on the neighbour's side. So an edge flagged on both
//! cells and an edge flagged on one are both a full-width strip, with no overlap. Strips are drawn
//! only over land: none between two water cells, and none over water.

use super::{Relief, TerrainSnapshot, edge, lift};
use crate::blend::{BlendMesh, Layer, srgb_to_linear};
use crate::hexgrid::river_dirs;

/// Width of the strip over the edge.
// SIMPLIFIED(SIMP-007): a flat strip instead of the original procedural river geometry
pub const RIVER_WIDTH: f32 = 8.0;
/// Height of the strip: just over the land (12.0).
// SIMPLIFIED(SIMP-007): one height for every river, also over hills
pub const RIVER_HEIGHT: f32 = 12.3;
/// Stand-in for `river_diffuse` / `river_alpha` until the assets exist: sRGB colour and alpha.
// SIMPLIFIED(SIMP-007): flat blue with feathered edges instead of the river textures
const RIVER_FALLBACK: ([u8; 3], f32) = ([70, 130, 200], 0.9);

pub fn build(snapshot: &TerrainSnapshot, out: &mut BlendMesh) {
    let shape = snapshot.shape();
    let (rgb, alpha) = RIVER_FALLBACK;
    let [r, g, b] = srgb_to_linear(rgb);
    let color = [r, g, b, alpha];
    let half = RIVER_WIDTH * 0.5;
    for y in 0..shape.height {
        for x in 0..shape.width {
            let Some(index) = snapshot.index(x, y) else {
                continue;
            };
            let cell = &snapshot.cells()[index];
            if !cell.is_revealed() {
                continue;
            }
            let self_land = cell.drawn_relief() != Relief::Water;
            for dir in river_dirs(cell.rivers) {
                let neighbor = shape
                    .neighbor(x, y, dir)
                    .and_then(|(nx, ny)| snapshot.cell(nx, ny));
                let e = edge(x, y, dir);
                let mut side = |across_far: f32, sign: f32| {
                    let step = e.inward * (sign * half);
                    out.quad(
                        Layer::Rivers,
                        [
                            lift(e.a, RIVER_HEIGHT),
                            lift(e.b, RIVER_HEIGHT),
                            lift(e.b + step, RIVER_HEIGHT),
                            lift(e.a + step, RIVER_HEIGHT),
                        ],
                        [0.5, 0.5, across_far, across_far],
                        color,
                        index as u32,
                    );
                };
                if self_land {
                    side(1.0, 1.0);
                }
                let outward_land = neighbor.is_some_and(|n| {
                    n.is_revealed()
                        && n.drawn_relief() != Relief::Water
                        && n.rivers & dir.opposite().edge_bit() == 0
                });
                if outward_land {
                    side(0.0, -1.0);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec2;

    use super::*;
    use crate::hexgrid::{Dir, cell_center};
    use crate::terrain::test_util::{land, map, water};
    use crate::terrain::{TerrainCell, TerrainSnapshot};

    fn rivers_of(snap: &TerrainSnapshot) -> BlendMesh {
        let mut m = BlendMesh::default();
        build(snap, &mut m);
        assert!(m.indices_valid());
        m
    }

    /// 5x5 land map, the cell (2, 2) carries the given rivers.
    fn one_river(mask: u8) -> TerrainSnapshot {
        map(5, 5, |x, y| TerrainCell {
            rivers: if (x, y) == (2, 2) { mask } else { 0 },
            ..land()
        })
    }

    fn normal(dir: Dir) -> Vec2 {
        let a = dir.normal_angle();
        Vec2::new(a.cos(), -a.sin())
    }

    #[test]
    fn strip_lies_along_the_flagged_edge_for_every_direction() {
        for dir in Dir::ALL {
            let m = rivers_of(&one_river(dir.edge_bit()));
            // Land on both sides, the neighbour has no river bit: inside and outside half.
            assert_eq!(m.rivers.len(), 12, "{dir:?}");
            let c = cell_center(2, 2);
            let n = normal(dir);
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for v in &m.vertices {
                assert!((v.position[1] - RIVER_HEIGHT).abs() < 1e-4);
                // Signed distance from the edge line along its outward normal.
                let d = (Vec2::new(v.position[0], v.position[2]) - c).dot(n) - 32.0;
                lo = lo.min(d);
                hi = hi.max(d);
            }
            // Together the two halves span the full width, centred on the edge.
            assert!((lo + RIVER_WIDTH * 0.5).abs() < 1e-3, "{dir:?}: {lo}");
            assert!((hi - RIVER_WIDTH * 0.5).abs() < 1e-3, "{dir:?}: {hi}");
        }
    }

    #[test]
    fn no_river_without_a_bit_and_one_strip_per_bit() {
        assert!(rivers_of(&one_river(0)).is_empty());
        let m = rivers_of(&one_river(Dir::East.edge_bit() | Dir::SouthWest.edge_bit()));
        assert_eq!(m.rivers.len(), 2 * 12);
    }

    #[test]
    fn edge_flagged_on_both_cells_is_drawn_once_across() {
        let snap = map(3, 3, |x, y| TerrainCell {
            rivers: match (x, y) {
                (1, 2) => Dir::East.edge_bit(),
                (2, 2) => Dir::West.edge_bit(),
                _ => 0,
            },
            ..land()
        });
        let m = rivers_of(&snap);
        // Two inside halves, no outside ones.
        assert_eq!(m.rivers.len(), 2 * 6);
        let edge_x = cell_center(1, 2).x + 32.0;
        let xs: Vec<f32> = m.vertices.iter().map(|v| v.position[0]).collect();
        assert!(xs.iter().any(|&x| x < edge_x - 1.0) && xs.iter().any(|&x| x > edge_x + 1.0));
    }

    #[test]
    fn none_between_two_water_cells() {
        let snap = map(5, 5, |x, y| {
            let base = if (x, y) == (2, 2) || (x, y) == (3, 2) {
                water()
            } else {
                land()
            };
            TerrainCell {
                rivers: if (x, y) == (2, 2) {
                    Dir::East.edge_bit()
                } else {
                    0
                },
                ..base
            }
        });
        assert!(rivers_of(&snap).is_empty());
    }

    #[test]
    fn river_at_a_coast_is_drawn_on_the_land_side_only() {
        let snap = map(5, 5, |x, y| {
            let base = if (x, y) == (3, 2) { water() } else { land() };
            TerrainCell {
                rivers: if (x, y) == (2, 2) {
                    Dir::East.edge_bit()
                } else {
                    0
                },
                ..base
            }
        });
        let m = rivers_of(&snap);
        assert_eq!(m.rivers.len(), 6);
        let edge_x = cell_center(2, 2).x + 32.0;
        assert!(m.vertices.iter().all(|v| v.position[0] <= edge_x + 1e-3));
    }

    #[test]
    fn unrevealed_cells_draw_nothing() {
        let snap = map(3, 3, |x, y| {
            if (x, y) == (1, 1) {
                TerrainCell {
                    rivers: 0x3F,
                    ..TerrainCell::default()
                }
            } else {
                land()
            }
        });
        assert!(rivers_of(&snap).is_empty());
    }
}
