//! Hex grid in world space.
//!
//! World units are Civ5 units (a hex is 64 wide). Y is up, north is -Z, east is +X. Cell (x, y) has
//! y growing north; odd rows are shifted half a hex east. Pure math, no GPU.

use glam::Vec2;

/// Hex width (flat-to-flat, pointy-top), world units.
// re: TerrainSystem.cpp:681-684
pub const HEX_WIDTH: f32 = 64.0;
/// Distance between rows: 64 * sqrt(3) / 2.
// re: TerrainSystem.cpp:48
pub const ROW_STEP: f32 = 55.425_625;
/// Centre-to-corner radius: 64 / sqrt(3).
pub const CORNER_RADIUS: f32 = 36.950_417;

/// The six neighbour directions, in the order of the gamecore `DirectionTypes`; the discriminant is
/// both the direction index and the bit index in `PlotView.rivers`.
// re: Lekmod CvGameCoreDLL_Expansion2/CvGlobals.cpp:2095-2113 (aiPlotDirectionX/Y)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Dir {
    NorthEast = 0,
    East = 1,
    SouthEast = 2,
    SouthWest = 3,
    West = 4,
    NorthWest = 5,
}

impl Dir {
    pub const ALL: [Dir; 6] = [
        Dir::NorthEast,
        Dir::East,
        Dir::SouthEast,
        Dir::SouthWest,
        Dir::West,
        Dir::NorthWest,
    ];

    pub const fn index(self) -> usize {
        self as usize
    }

    /// Hex-space step `(dhx, dy)` where `hx = x - floor(y / 2)`.
    // re: Lekmod CvGameCoreDLL_Expansion2/CvGlobals.cpp:2095-2113
    pub const fn hex_delta(self) -> (i32, i32) {
        match self {
            Dir::NorthEast => (0, 1),
            Dir::East => (1, 0),
            Dir::SouthEast => (1, -1),
            Dir::SouthWest => (0, -1),
            Dir::West => (-1, 0),
            Dir::NorthWest => (-1, 1),
        }
    }

    pub const fn opposite(self) -> Dir {
        match self {
            Dir::NorthEast => Dir::SouthWest,
            Dir::East => Dir::West,
            Dir::SouthEast => Dir::NorthWest,
            Dir::SouthWest => Dir::NorthEast,
            Dir::West => Dir::East,
            Dir::NorthWest => Dir::SouthEast,
        }
    }

    /// Bit of this edge in a 6-bit river mask.
    pub const fn edge_bit(self) -> u8 {
        1 << (self as u8)
    }

    /// Angle (radians, counter-clockwise seen from above with north up) of the edge normal.
    pub fn normal_angle(self) -> f32 {
        let deg = match self {
            Dir::NorthEast => 60.0,
            Dir::East => 0.0,
            Dir::SouthEast => -60.0,
            Dir::SouthWest => -120.0,
            Dir::West => 180.0,
            Dir::NorthWest => 120.0,
        };
        f32::to_radians(deg)
    }
}

/// Directions whose bit is set in a 6-bit river mask.
pub fn river_dirs(mask: u8) -> impl Iterator<Item = Dir> {
    Dir::ALL
        .into_iter()
        .filter(move |d| mask & d.edge_bit() != 0)
}

/// Map extent for neighbour lookup and picking. Only X wrapping exists in B1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridShape {
    pub width: i32,
    pub height: i32,
    pub wrap_x: bool,
}

impl GridShape {
    /// Wrap X (if the map wraps) and bounds-check Y; `None` if the cell is off the map.
    pub fn normalize(&self, x: i32, y: i32) -> Option<(i32, i32)> {
        if y < 0 || y >= self.height {
            return None;
        }
        if self.wrap_x {
            Some((x.rem_euclid(self.width), y))
        } else if (0..self.width).contains(&x) {
            Some((x, y))
        } else {
            None
        }
    }

    /// World-space period in X when wrapping.
    pub fn world_width(&self) -> f32 {
        HEX_WIDTH * self.width as f32
    }

    /// Neighbour of `(x, y)` in direction `dir`, or `None` past the map edge.
    // re: Lekmod CvGameCoreDLL_Expansion2/CvGameCoreUtils.h:217-235 (plotDirection), :84-92
    pub fn neighbor(&self, x: i32, y: i32, dir: Dir) -> Option<(i32, i32)> {
        let (dx, dy) = dir.hex_delta();
        let ny = y + dy;
        // `>> 1` on i32 is floor division, matching the hex-space shift for y >= 0.
        let nhx = x - (y >> 1) + dx;
        let nx = nhx + (ny >> 1);
        self.normalize(nx, ny)
    }

    /// Cell under a world point (nearest centre), `None` if it falls off the map.
    pub fn world_to_cell(&self, p: Vec2) -> Option<(i32, i32)> {
        let (x, y) = nearest_cell(p);
        self.normalize(x, y)
    }

    /// Bring a world X into `[0, 64 * width)`.
    pub fn wrap_world_x(&self, world_x: f32) -> f32 {
        world_x.rem_euclid(self.world_width())
    }
}

/// Unwrapped cell whose centre is nearest to the world point `(X, Z)`; no map bounds, no wrap.
pub fn nearest_cell(p: Vec2) -> (i32, i32) {
    let row = (-p.y / ROW_STEP).round() as i32;
    let mut best = (0, 0);
    let mut best_d = f32::INFINITY;
    for y in (row - 1)..=(row + 1) {
        let x = (p.x / HEX_WIDTH - 0.5 * (y & 1) as f32).round() as i32;
        let d = cell_center(x, y).distance_squared(p);
        if d < best_d {
            best = (x, y);
            best_d = d;
        }
    }
    best
}

/// World `(X, Z)` of the centre of cell `(x, y)`, unwrapped.
pub fn cell_center(x: i32, y: i32) -> Vec2 {
    Vec2::new(
        HEX_WIDTH * (x as f32 + 0.5 * (y & 1) as f32),
        -ROW_STEP * y as f32,
    )
}

/// Corner `i` (0..6) offset from the centre: angle 30 + 60 * i degrees, pointy-top.
pub fn corner_offset(i: usize) -> Vec2 {
    let a = f32::to_radians(30.0 + 60.0 * i as f32);
    Vec2::new(CORNER_RADIUS * a.cos(), -CORNER_RADIUS * a.sin())
}

/// Corner indices `(a, b)` of the edge facing `dir`, going counter-clockwise (a is the lower angle).
pub fn edge_corners(dir: Dir) -> (usize, usize) {
    let a = match dir {
        Dir::East => 5, // -30 deg
        Dir::NorthEast => 0,
        Dir::NorthWest => 1,
        Dir::West => 2,
        Dir::SouthWest => 3,
        Dir::SouthEast => 4,
    };
    (a, (a + 1) % 6)
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: i32 = 40;
    const H: i32 = 24;

    fn shape() -> GridShape {
        GridShape {
            width: W,
            height: H,
            wrap_x: true,
        }
    }

    #[test]
    fn constants_match_formulas() {
        assert!((ROW_STEP - 64.0 * 3f32.sqrt() / 2.0).abs() < 1e-3);
        assert!((CORNER_RADIUS - 64.0 / 3f32.sqrt()).abs() < 1e-3);
        assert!((corner_offset(0).length() - CORNER_RADIUS).abs() < 1e-3);
    }

    #[test]
    fn center_roundtrip_all_cells() {
        let s = shape();
        for y in 0..H {
            for x in 0..W {
                let c = cell_center(x, y);
                assert_eq!(s.world_to_cell(c), Some((x, y)));
                // Near a corner but still inside.
                for i in 0..6 {
                    let p = c + corner_offset(i) * 0.9;
                    assert_eq!(s.world_to_cell(p), Some((x, y)), "cell {x},{y} corner {i}");
                }
            }
        }
    }

    #[test]
    fn wrapped_world_x_picks_same_cell() {
        let s = shape();
        for y in 0..H {
            for x in 0..W {
                let c = cell_center(x, y);
                let shifted = c + Vec2::new(s.world_width(), 0.0);
                assert_eq!(s.world_to_cell(shifted), Some((x, y)));
                let shifted = c - Vec2::new(s.world_width(), 0.0);
                assert_eq!(s.world_to_cell(shifted), Some((x, y)));
            }
        }
        let w = s.wrap_world_x(-10.0);
        assert!((w - (s.world_width() - 10.0)).abs() < 1e-3);
    }

    #[test]
    fn off_map_is_none() {
        let s = shape();
        assert_eq!(s.world_to_cell(cell_center(0, -1)), None);
        assert_eq!(s.world_to_cell(cell_center(0, H)), None);
        let flat = GridShape { wrap_x: false, ..s };
        assert_eq!(flat.world_to_cell(cell_center(-1, 4)), None);
        assert_eq!(flat.world_to_cell(cell_center(W, 4)), None);
    }

    /// Offset tables derived by hand from the gamecore hex-space tables
    /// (CvGlobals.cpp:2095-2113 with hx = x - floor(y/2)), order NE, E, SE, SW, W, NW.
    const EVEN: [(i32, i32); 6] = [(0, 1), (1, 0), (0, -1), (-1, -1), (-1, 0), (-1, 1)];
    const ODD: [(i32, i32); 6] = [(1, 1), (1, 0), (1, -1), (0, -1), (-1, 0), (0, 1)];

    #[test]
    fn neighbors_match_gamecore_tables() {
        // Unbounded shape so no cell is clipped; interior cells only.
        let s = GridShape {
            width: 1000,
            height: 1000,
            wrap_x: false,
        };
        for y in 10..30 {
            for x in 10..30 {
                let table = if y & 1 == 0 { &EVEN } else { &ODD };
                for d in Dir::ALL {
                    let (dx, dy) = table[d.index()];
                    assert_eq!(s.neighbor(x, y, d), Some((x + dx, y + dy)), "{x},{y} {d:?}");
                }
            }
        }
    }

    #[test]
    fn hex_delta_matches_gamecore() {
        let x = [0, 1, 1, 0, -1, -1];
        let y = [1, 0, -1, -1, 0, 1];
        for d in Dir::ALL {
            assert_eq!(d.hex_delta(), (x[d.index()], y[d.index()]));
        }
    }

    #[test]
    fn neighbors_are_geometrically_adjacent_and_oriented() {
        let s = shape();
        for y in 0..H {
            for x in 0..W {
                for d in Dir::ALL {
                    let Some((nx, ny)) = s.neighbor(x, y, d) else {
                        continue;
                    };
                    // Pick the wrapped image of the neighbour closest to this cell.
                    let c = cell_center(x, y);
                    let n = cell_center(nx, ny);
                    let n = [-1.0f32, 0.0, 1.0]
                        .map(|k| n + Vec2::new(k * s.world_width(), 0.0))
                        .into_iter()
                        .min_by(|a, b| a.distance(c).total_cmp(&b.distance(c)))
                        .unwrap();
                    let v = n - c;
                    assert!((v.length() - HEX_WIDTH).abs() < 1e-2, "{x},{y} {d:?}");
                    let a = d.normal_angle();
                    let expect = Vec2::new(a.cos(), -a.sin()) * HEX_WIDTH;
                    assert!(v.distance(expect) < 1e-2, "{x},{y} {d:?}");
                    // Symmetry.
                    assert_eq!(s.neighbor(nx, ny, d.opposite()), Some((x, y)));
                }
            }
        }
    }

    #[test]
    fn seam_neighbors() {
        let s = shape();
        assert_eq!(s.neighbor(0, 4, Dir::West), Some((W - 1, 4)));
        assert_eq!(s.neighbor(W - 1, 4, Dir::East), Some((0, 4)));
        // Odd row: NE of the last column wraps to column 0.
        assert_eq!(s.neighbor(W - 1, 3, Dir::NorthEast), Some((0, 4)));
        // Even row: SW of column 0 wraps to the last column.
        assert_eq!(s.neighbor(0, 4, Dir::SouthWest), Some((W - 1, 3)));
        assert_eq!(s.neighbor(3, 0, Dir::SouthEast), None);
        assert_eq!(s.neighbor(3, H - 1, Dir::NorthWest), None);
    }

    #[test]
    fn edges_face_their_neighbors() {
        for d in Dir::ALL {
            let (a, b) = edge_corners(d);
            let mid = (corner_offset(a) + corner_offset(b)) * 0.5;
            let ang = d.normal_angle();
            let normal = Vec2::new(ang.cos(), -ang.sin());
            // Edge midpoint lies on the normal at half the hex width.
            assert!(mid.distance(normal * HEX_WIDTH * 0.5) < 1e-2, "{d:?}");
        }
    }

    #[test]
    fn river_bits_map_to_directions() {
        let mask = Dir::East.edge_bit() | Dir::NorthWest.edge_bit();
        assert_eq!(mask, 0b10_0010);
        let got: Vec<Dir> = river_dirs(mask).collect();
        assert_eq!(got, vec![Dir::East, Dir::NorthWest]);
        assert_eq!(river_dirs(0).count(), 0);
        assert_eq!(river_dirs(0x3F).count(), 6);
    }
}
