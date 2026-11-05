//! The cull grid of the fluid fog (world-overlays WOV1, `FluidFOWSystem::Update`): where the clouds
//! need not be drawn.
//!
//! A cell is culled when none of the 21 cells around it (two steps) is unrevealed: the mask's
//! cloud channel is 0 there (its rim reaches 1.4 hex radii, less than a step), so a cloud tile over
//! only such cells shows nothing. The original draws a quad per culled cell with the clip depth
//! forced to 0, so that the cloud layers fail the depth test. The product leaves the tiles out
//! instead (`CullGrid::tile_is_culled`): the depth buffer of the main pass is shared with what is
//! drawn after the fog (the path, the arrows), so nothing can be written to it. The picture is the
//! same; the shaders of the cull layer (`shaders/fow/cull.wgsl`) are ported and kept for the
//! reference.
//!
//! The original wraps the neighbours around the edge of the grid in both directions and culls no
//! cell of the first two columns; here a hex off the map is unrevealed (the mask draws it so), so
//! a cell next to the edge is never culled.

use glam::Vec2;

use super::mask::cells_in_rect;
use super::state::FogInput;
use crate::hexgrid::CORNER_RADIUS;
use crate::terrain::Fog;

/// Cells that decide a cell's cull, itself included (`kNumNearOffsets`).
pub const NEAR_HEXES: usize = 21;

/// The cells `Update` looks at: a 5 x 5 block of the offset grid without its four corners (the
/// original's `kNearOffsets`), `(dx, dy)` from the cell. It holds every hex within two steps (19)
/// and two more.
pub fn near_offsets() -> Vec<(i32, i32)> {
    let mut offsets = Vec::with_capacity(NEAR_HEXES);
    for dy in -2..=2i32 {
        for dx in -2..=2i32 {
            if dx.abs() == 2 && dy.abs() == 2 {
                continue;
            }
            offsets.push((dx, dy));
        }
    }
    offsets
}

/// The 21 cells around `(x, y)` that decide whether it is culled.
pub fn near_hexes(x: i32, y: i32) -> Vec<(i32, i32)> {
    near_offsets()
        .into_iter()
        .map(|(dx, dy)| (x + dx, y + dy))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CullGrid {
    width: u32,
    height: u32,
    wrap_x: bool,
    /// Row-major from y = 0.
    culled: Vec<bool>,
}

/// The cull grid of a fog.
pub fn cull_grid(fog: &FogInput) -> CullGrid {
    let shape = fog.shape();
    let mut culled = Vec::with_capacity(fog.cells.len());
    for y in 0..fog.height as i32 {
        for x in 0..fog.width as i32 {
            let clear = near_hexes(x, y).into_iter().all(|(nx, ny)| {
                shape
                    .normalize(nx, ny)
                    .and_then(|(cx, cy)| fog.cell(cx, cy))
                    .is_some_and(|f| f != Fog::Unrevealed)
            });
            culled.push(clear);
        }
    }
    CullGrid {
        width: fog.width,
        height: fog.height,
        wrap_x: fog.wrap_x,
        culled,
    }
}

impl CullGrid {
    /// Whether the cell is culled; a cell off the map never is.
    pub fn at(&self, x: i32, y: i32) -> bool {
        let shape = crate::hexgrid::GridShape {
            width: self.width as i32,
            height: self.height as i32,
            wrap_x: self.wrap_x,
        };
        shape
            .normalize(x, y)
            .is_some_and(|(cx, cy)| self.culled[cy as usize * self.width as usize + cx as usize])
    }

    pub fn culled_cells(&self) -> usize {
        self.culled.iter().filter(|&&c| c).count()
    }

    /// Whether a tile (`lo`..`hi`, our `x` and `z`) lies over culled cells only: every cell that
    /// has a point of the tile is culled.
    pub fn tile_is_culled(&self, lo: Vec2, hi: Vec2) -> bool {
        let cells = cells_in_rect(lo, hi, CORNER_RADIUS);
        !cells.is_empty() && cells.into_iter().all(|(x, y)| self.at(x, y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fog(width: u32, height: u32, wrap: bool, f: impl Fn(u32, u32) -> Fog) -> FogInput {
        FogInput::new(
            width,
            height,
            wrap,
            (0..width * height)
                .map(|i| f(i % width, i / width))
                .collect(),
        )
        .unwrap()
    }

    use crate::hexgrid::Dir;

    /// The cell `dir` of `(x, y)`, without a map: the same arithmetic as `GridShape::neighbor`.
    fn step(x: i32, y: i32, dir: Dir) -> (i32, i32) {
        let (dx, dy) = dir.hex_delta();
        let ny = y + dy;
        let nhx = x - (y >> 1) + dx;
        (nhx + (ny >> 1), ny)
    }

    #[test]
    fn the_block_is_21_cells_without_corners() {
        for (x, y) in [(5, 4), (5, 5), (0, 0), (3, 1)] {
            let cells = near_hexes(x, y);
            assert_eq!(cells.len(), NEAR_HEXES, "{x},{y}");
            assert!(cells.contains(&(x, y)), "the cell itself is in the block");
            let mut sorted = cells.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(sorted.len(), NEAR_HEXES);
            assert!(!cells.contains(&(x + 2, y + 2)) && !cells.contains(&(x - 2, y - 2)));
        }
    }

    #[test]
    fn every_hex_within_two_steps_is_in_the_block_and_three_steps_away_is_not() {
        for start in [(10, 10), (10, 11)] {
            let near = near_hexes(start.0, start.1);
            let mut ring = vec![start];
            let mut within_two = vec![start];
            for _ in 0..2 {
                let from = ring.clone();
                ring.clear();
                for (x, y) in from {
                    for d in Dir::ALL {
                        let n = step(x, y, d);
                        if !within_two.contains(&n) {
                            within_two.push(n);
                            ring.push(n);
                        }
                    }
                }
            }
            assert_eq!(within_two.len(), 19);
            for h in &within_two {
                assert!(near.contains(h), "{h:?} of {start:?}");
            }
            // Three steps along one direction is out of the block.
            let mut p = start;
            for _ in 0..3 {
                p = step(p.0, p.1, Dir::NorthEast);
            }
            assert!(!near.contains(&p), "{p:?}");
        }
        // The same arithmetic as `GridShape::neighbor` on a map that does not touch an edge.
        let shape = crate::hexgrid::GridShape {
            width: 40,
            height: 40,
            wrap_x: false,
        };
        assert_eq!(
            shape.neighbor(10, 10, Dir::SouthEast),
            Some(step(10, 10, Dir::SouthEast))
        );
    }

    #[test]
    fn nothing_is_culled_near_an_unrevealed_cell() {
        let f = fog(12, 12, false, |x, y| {
            if (x, y) == (6, 6) {
                Fog::Unrevealed
            } else {
                Fog::Visible
            }
        });
        let g = cull_grid(&f);
        for (x, y) in near_hexes(6, 6) {
            assert!(!g.at(x, y), "{x},{y} is within two steps");
        }
        // Three steps away is culled, if it is not at an edge.
        assert!(g.at(6, 9) || g.at(2, 6), "{:?}", g.culled_cells());
        assert!(g.culled_cells() > 0);
        assert!(!g.at(-1, 3) && !g.at(3, 99));
    }

    #[test]
    fn the_edge_of_a_flat_map_is_never_culled_and_that_of_a_wrapping_one_in_x_is() {
        let all = |wrap| fog(12, 12, wrap, |_, _| Fog::Visible);
        let flat = cull_grid(&all(false));
        assert!(!flat.at(0, 6), "the west edge touches hexes off the map");
        assert!(!flat.at(6, 0));
        assert!(!flat.at(6, 11));
        assert!(!flat.at(1, 6));
        assert!(flat.at(6, 6));
        let wrapped = cull_grid(&all(true));
        assert!(wrapped.at(0, 6), "no edge in x");
        assert!(wrapped.at(11, 6));
        assert!(!wrapped.at(6, 0), "the north and south edges stay");
    }

    #[test]
    fn a_fully_revealed_map_is_culled_in_the_middle_only() {
        let g = cull_grid(&fog(20, 20, false, |_, _| Fog::Hidden));
        let n = g.culled_cells();
        assert!(n > 0 && n < 400, "{n}");
        assert_eq!(n, 16 * 16, "all but a ring two cells wide ({n})");
    }

    #[test]
    fn a_tile_over_culled_cells_is_culled_and_one_over_the_fog_is_not() {
        let f = fog(24, 24, false, |x, y| {
            if x < 4 && y < 4 {
                Fog::Unrevealed
            } else {
                Fog::Visible
            }
        });
        let g = cull_grid(&f);
        // A tile in the middle of the revealed part (our x east, z south).
        let mid = crate::hexgrid::cell_center(14, 14);
        assert!(g.tile_is_culled(mid - Vec2::splat(32.0), mid + Vec2::splat(32.0)));
        // A tile over the unrevealed corner.
        let corner = crate::hexgrid::cell_center(1, 1);
        assert!(!g.tile_is_culled(corner - Vec2::splat(32.0), corner + Vec2::splat(32.0)));
        // A tile off the map is drawn.
        assert!(!g.tile_is_culled(Vec2::new(-500.0, 500.0), Vec2::new(-400.0, 600.0)));
    }
}
