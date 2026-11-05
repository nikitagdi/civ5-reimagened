//! Hex edges of the rivers (world-water WW-4, RW-12): the `HexEdge` of the original, its twin and
//! flip tables, the grid of river bits (`cvRiverSimSystem::IsRiver`, `GetNextEdge` and its
//! relatives) and the control points of a hex side (`GameSplineDatabase::GenerateHexSide`).
//!
//! # Numbering of the sides (our fixed choice)
//!
//! The original numbers the six sides of a hex with `HEXVec_i::TRANSLATIONS`, a `.bss` table the
//! dump does not hold. What the code fixes is this: side `s` and `s + 3` are opposite
//! (`DAT_09149c44`); `kSideRotation` in `cvFloodplainsSystem` says the sides, in turn, are
//! `4, 0, 5, 1, 3, 2` at 60 degrees apart, and the bank pattern of side 4 is centred on the
//! direction `(1, 0, -1)`. Rotating the other way round or starting at another side is the same
//! drawing under other names, so we take the one that fits our `hexgrid` (cube `x = x - (y >> 1)`,
//! `y`): `TRANSLATIONS = [NE, W, SE, SW, E, NW]`, turning counter-clockwise from side 4 (east).
//!
//! A code is `side + 6 * d` with `d = 0` for the edge travelled clockwise round its hex (the hex is
//! on the right of the flow) and `d = 1` counter-clockwise (the hex is on the left). The corner
//! labels of `DAT_089e8` map to our corner indices by `k - 1` (corner 0 of the table is our 5).
// re: cvRiverSimSystem.h `HexEdge`; FUN_0865f1be, FUN_0865f23e, FUN_087c97f2, FUN_087c988e,
// FUN_087f801a, FUN_087d5344; tables DAT_09149c44 (twin), DAT_0920c180 (flip), the table at
// 0x0a30f9e8 (corner of a code)

use std::collections::HashSet;

use glam::Vec2;

use crate::hexgrid::{Dir, GridShape, cell_center, corner_offset};
use crate::world::terrain::basis::TerrainBasis;
use crate::world::terrain::sampler::TerrainFrame;

/// The same edge seen from the hex across it, in the same direction.
// re: DAT_09149c44
pub const TWIN: [u8; 12] = [9, 10, 11, 6, 7, 8, 3, 4, 5, 0, 1, 2];
/// The next edge round the same hex (the one that starts where this one ends).
// re: DAT_0920c180
pub const FLIP: [u8; 12] = [4, 5, 3, 1, 2, 0, 11, 9, 10, 8, 6, 7];
/// The corner label (0..6, one step is 60 degrees) an edge starts at.
// re: the table at 0x0a30f9e8 read by FUN_087d5344
const START_CORNER: [u8; 12] = [2, 4, 0, 5, 1, 3, 1, 3, 5, 4, 0, 2];
/// The direction of the neighbour across side `s`.
pub const SIDE_DIR: [Dir; 6] = [
    Dir::NorthEast,
    Dir::West,
    Dir::SouthEast,
    Dir::SouthWest,
    Dir::East,
    Dir::NorthWest,
];

/// An edge of a hex: the hex in cube coordinates `(x, y)` (`z = -(x + y)`, `x = col - (row >> 1)`)
/// and the code 0..12.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HexEdge {
    pub x: i32,
    pub y: i32,
    pub code: u8,
}

impl HexEdge {
    pub fn new(x: i32, y: i32, code: u8) -> Self {
        Self { x, y, code }
    }

    /// The edge of the cell `(col, row)`.
    pub fn of_cell(col: i32, row: i32, code: u8) -> Self {
        Self::new(col - (row >> 1), row, code)
    }

    /// The offset cell of the hex, unwrapped.
    pub fn cell(&self) -> (i32, i32) {
        (self.x + (self.y >> 1), self.y)
    }

    pub fn side(&self) -> usize {
        usize::from(self.code % 6)
    }

    /// The neighbour across the side of the code.
    fn across_hex(&self) -> (i32, i32) {
        let (dx, dy) = SIDE_DIR[self.side()].hex_delta();
        (self.x + dx, self.y + dy)
    }

    /// The same edge named from the hex across it.
    // re: the stepping in FUN_087c97f2 / the `EdgeAcross` of cvFloodplainsSystem.cpp
    pub fn twin(&self) -> HexEdge {
        let (x, y) = self.across_hex();
        HexEdge::new(x, y, TWIN[usize::from(self.code)])
    }

    /// The same side, travelled the other way, on the same hex.
    pub fn reversed(&self) -> HexEdge {
        HexEdge::new(self.x, self.y, (self.code + 6) % 12)
    }

    /// Sides 3..5 are named from the hex across.
    // re: FUN_0865f1be
    pub fn owned(&self) -> HexEdge {
        if self.code % 6 < 3 {
            *self
        } else {
            self.twin()
        }
    }

    /// The edge in the form with the hex on its left (`code >= 6`).
    fn left_form(&self) -> HexEdge {
        if self.code < 6 { self.twin() } else { *self }
    }

    /// The edge in the form with the hex on its right (`code < 6`).
    fn right_form(&self) -> HexEdge {
        if self.code > 5 { self.twin() } else { *self }
    }

    /// The edge that goes on along the hex on the left of this one.
    // re: FUN_087c97f2
    pub fn next_left(&self) -> HexEdge {
        let l = self.left_form();
        HexEdge::new(l.x, l.y, FLIP[usize::from(l.code)])
    }

    /// The edge that goes on along the hex on the right of this one.
    // re: FUN_087c988e
    pub fn next_right(&self) -> HexEdge {
        let r = self.right_form();
        HexEdge::new(r.x, r.y, FLIP[usize::from(r.code)])
    }

    /// The two edges that come into the start of this one.
    // TODO(RW-12): `FUN_087fb18e` / `FUN_087fb22a` (the candidates of `GetPrevEdge`) are not in the
    // dump; these are the reverse of the continuations of the reverse, in the order of the next ones.
    pub fn prev_candidates(&self) -> (HexEdge, HexEdge) {
        let r = self.reversed();
        (r.next_left().reversed(), r.next_right().reversed())
    }

    /// The corner index (our `hexgrid::corner_offset`) the edge starts at, and the one it ends at.
    pub fn corners(&self) -> (usize, usize) {
        let s = usize::from(START_CORNER[usize::from(self.code)]);
        let e = usize::from(START_CORNER[usize::from(FLIP[usize::from(self.code)])]);
        ((s + 5) % 6, (e + 5) % 6)
    }

    /// The two ends of the edge as points of the world `(X, Z)`.
    pub fn world_ends(&self) -> (Vec2, Vec2) {
        let (col, row) = self.cell();
        let c = cell_center(col, row);
        let (a, b) = self.corners();
        (c + corner_offset(a), c + corner_offset(b))
    }

    /// The control points of the side in the texel plane: the points at a quarter and three quarters
    /// of it, in the direction of the flow.
    // re: GameSplineDatabase::GenerateHexSide 0x087d4f28
    pub fn side_points(&self, frame: &TerrainFrame) -> [Vec2; 2] {
        let e = self.owned_form();
        let (a, b) = e.world_ends();
        let (a, b) = (frame.world_to_tex(a), frame.world_to_tex(b));
        [a * 0.75 + b * 0.25, a * 0.25 + b * 0.75]
    }

    /// The form with a code below 6 (`GenerateHexSide` takes this one).
    fn owned_form(&self) -> HexEdge {
        self.right_form()
    }
}

/// The grid of the edges that carry a river, with the flow: `OnAddRiverTile` sets bit `code` of
/// the hex. Edges are kept by their owned form.
#[derive(Clone, Debug, Default)]
pub struct RiverGrid {
    shape: Option<GridShape>,
    edges: HashSet<HexEdge>,
}

impl RiverGrid {
    pub fn new(shape: GridShape) -> Self {
        Self {
            shape: Some(shape),
            edges: HashSet::new(),
        }
    }

    pub fn shape(&self) -> GridShape {
        self.shape.expect("a river grid has a shape")
    }

    /// The edge with the hex put inside the map (`FUN_0865f23e`), `None` off the map.
    pub fn wrapped(&self, e: HexEdge) -> Option<HexEdge> {
        let (col, row) = e.cell();
        let (c, r) = self.shape().normalize(col, row)?;
        Some(HexEdge::of_cell(c, r, e.code))
    }

    /// The key an edge is stored under: the owned form, wrapped.
    pub fn key(&self, e: HexEdge) -> Option<HexEdge> {
        self.wrapped(e.owned())
    }

    /// The same edge under both names, wrapped (`IsEdgeVisited` of the original tests both).
    pub fn both_names(&self, e: HexEdge) -> [Option<HexEdge>; 2] {
        [self.wrapped(e), self.wrapped(e.twin())]
    }

    pub fn add(&mut self, e: HexEdge) {
        if let Some(k) = self.key(e) {
            self.edges.insert(k);
        }
    }

    // re: cvRiverSimSystem::IsRiver 0x087f4fb0
    pub fn is_river(&self, e: HexEdge) -> bool {
        self.key(e).is_some_and(|k| self.edges.contains(&k))
    }

    /// Every edge of the grid, in the owned form.
    pub fn edges(&self) -> impl Iterator<Item = HexEdge> + '_ {
        self.edges.iter().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    // re: cvRiverSimSystem::GetNextLeftEdge 0x087f50e4
    pub fn next_left_edge(&self, e: HexEdge) -> Option<HexEdge> {
        let n = e.next_left();
        self.is_river(n).then_some(n)
    }

    // re: cvRiverSimSystem::GetNextRightEdge 0x087f513c
    pub fn next_right_edge(&self, e: HexEdge) -> Option<HexEdge> {
        let n = e.next_right();
        self.is_river(n).then_some(n)
    }

    /// The left continuation if it is a river, else the right one.
    // re: cvRiverSimSystem::GetNextEdge 0x087f504c (FUN_087f7e66)
    pub fn next_edge(&self, e: HexEdge) -> Option<HexEdge> {
        self.next_left_edge(e).or_else(|| self.next_right_edge(e))
    }

    // re: cvRiverSimSystem::GetPrevEdge 0x087f5098 (FUN_087f7f40)
    pub fn prev_edge(&self, e: HexEdge) -> Option<HexEdge> {
        let (a, b) = e.prev_candidates();
        [a, b].into_iter().find(|c| self.is_river(*c))
    }

    /// The grid of a basis. Flow comes from `river_flow`; an edge without it is oriented toward the
    /// water.
    pub fn from_basis(basis: &TerrainBasis) -> Self {
        let shape = GridShape {
            width: i32::from(basis.width),
            height: i32::from(basis.height),
            wrap_x: basis.wrap_x,
        };
        let mut grid = Self::new(shape);
        let mut water = None;
        let w = i32::from(basis.width);
        for y in 0..i32::from(basis.height) {
            for x in 0..w {
                let Some(p) = basis.plot(x, y) else { continue };
                for (slot, dir) in [Dir::East, Dir::SouthEast, Dir::SouthWest]
                    .into_iter()
                    .enumerate()
                {
                    let across = shape.neighbor(x, y, dir);
                    let other = across.and_then(|(nx, ny)| basis.plot(nx, ny));
                    let has = p.rivers & dir.edge_bit() != 0
                        || other.is_some_and(|o| o.rivers & dir.opposite().edge_bit() != 0);
                    if !has {
                        continue;
                    }
                    let side = SIDE_DIR.iter().position(|d| *d == dir).unwrap_or(0) as u8;
                    let flow = p.river_flow[slot];
                    let code = if flow >= 0 {
                        code_for_flow(side, flow)
                    } else {
                        // TODO(dev-fallback): the B1 snapshot and the fixture carry no flow; turn
                        // the edge toward the water.
                        let d = water.get_or_insert_with(|| WaterDistance::new(basis));
                        d.code_toward_water(x, y, side)
                    };
                    grid.add(HexEdge::of_cell(x, y, code));
                }
            }
        }
        grid
    }
}

/// The compass angle of a `FlowDirectionTypes` (north, north-east, south-east, south, south-west,
/// north-west), in degrees, north up.
fn flow_angle(flow: i8) -> f32 {
    [90.0, 30.0, -30.0, -90.0, -150.0, 150.0][usize::from(flow.rem_euclid(6) as u8)]
}

/// The direction of travel of the edge `code` as an angle in degrees, north up, east right.
fn edge_angle(code: u8) -> f32 {
    let (a, b) = HexEdge::new(0, 0, code).world_ends();
    let d = b - a;
    // World Z points south.
    (-d.y).atan2(d.x).to_degrees()
}

/// The code of side `side` whose direction of travel is the nearest to the compass flow.
pub fn code_for_flow(side: u8, flow: i8) -> u8 {
    let want = flow_angle(flow);
    let diff = |c: u8| {
        let mut d = (edge_angle(c) - want).rem_euclid(360.0);
        if d > 180.0 {
            d = 360.0 - d;
        }
        d
    };
    if diff(side) <= diff(side + 6) {
        side
    } else {
        side + 6
    }
}

/// The distance of the hexes to the nearest water, for the flow fallback.
struct WaterDistance {
    shape: GridShape,
    dist: Vec<u16>,
}

impl WaterDistance {
    fn new(basis: &TerrainBasis) -> Self {
        let shape = GridShape {
            width: i32::from(basis.width),
            height: i32::from(basis.height),
            wrap_x: basis.wrap_x,
        };
        let n = basis.plots.len();
        let mut dist = vec![u16::MAX; n];
        let mut queue = std::collections::VecDeque::new();
        for (i, p) in basis.plots.iter().enumerate() {
            if p.relief == crate::terrain::Relief::Water {
                dist[i] = 0;
                queue.push_back(i);
            }
        }
        while let Some(i) = queue.pop_front() {
            let (x, y) = basis.xy(i as u32);
            for d in Dir::ALL {
                if let Some((nx, ny)) = shape.neighbor(x, y, d) {
                    let j = ny as usize * usize::from(basis.width) + nx as usize;
                    if dist[j] == u16::MAX {
                        dist[j] = dist[i] + 1;
                        queue.push_back(j);
                    }
                }
            }
        }
        Self { shape, dist }
    }

    fn at(&self, x: i32, y: i32) -> f32 {
        match self.shape.normalize(x, y) {
            Some((x, y)) => {
                let d = self.dist[y as usize * self.shape.width as usize + x as usize];
                if d == u16::MAX { 1.0e4 } else { f32::from(d) }
            }
            None => 1.0e4,
        }
    }

    /// The distance at a point of the world: the nearest centres within a corner radius.
    fn at_point(&self, p: Vec2) -> f32 {
        let (cx, cy) = crate::hexgrid::nearest_cell(p);
        let mut best = f32::MAX;
        let mut sum = 0.0;
        let mut n = 0.0;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (x, y) = (cx + dx, cy + dy);
                if cell_center(x, y).distance(p) < crate::hexgrid::CORNER_RADIUS * 1.05 {
                    let v = self.at(x, y);
                    best = best.min(v);
                    sum += v;
                    n += 1.0;
                }
            }
        }
        if n == 0.0 {
            self.at(cx, cy)
        } else {
            0.5 * best + 0.5 * sum / n
        }
    }

    fn code_toward_water(&self, x: i32, y: i32, side: u8) -> u8 {
        let e = HexEdge::of_cell(x, y, side);
        let (a, b) = e.world_ends();
        if self.at_point(b) <= self.at_point(a) {
            side
        } else {
            side + 6
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::{Relief, TerrainKind};
    use crate::world::terrain::basis::PlotBasis;

    #[test]
    fn twin_is_an_involution_with_the_same_ends() {
        for code in 0..12u8 {
            let e = HexEdge::new(3, 4, code);
            let t = e.twin();
            assert_eq!(t.twin(), e);
            let (a, b) = e.world_ends();
            let (c, d) = t.world_ends();
            // Wrapping the cube hex into a cell moves nothing in the plane.
            assert!(a.distance(c) < 1e-2 && b.distance(d) < 1e-2, "{code}");
        }
    }

    #[test]
    fn flip_walks_round_the_hex() {
        let mut c = 0u8;
        let mut seen = HashSet::new();
        for _ in 0..6 {
            assert!(seen.insert(c));
            let e = HexEdge::new(0, 0, c);
            let n = HexEdge::new(0, 0, FLIP[usize::from(c)]);
            assert!(e.world_ends().1.distance(n.world_ends().0) < 1e-2);
            c = FLIP[usize::from(c)];
        }
        assert_eq!(c, 0);
    }

    #[test]
    fn the_hex_is_on_the_right_of_a_low_code() {
        for code in 0..6u8 {
            let e = HexEdge::new(0, 0, code);
            let (a, b) = e.world_ends();
            let c = cell_center(0, 0);
            // Z points south, so cross products flip: clockwise seen from above means the centre is to
            // the right of the travel.
            let d = b - a;
            let to_c = c - a;
            let cross_north_up = d.x * (-to_c.y) - (-d.y) * to_c.x;
            assert!(cross_north_up < 0.0, "{code}");
        }
    }

    #[test]
    fn continuations_start_where_the_edge_ends() {
        for code in 0..12u8 {
            let e = HexEdge::new(2, 2, code);
            for n in [e.next_left(), e.next_right()] {
                assert!(e.world_ends().1.distance(n.world_ends().0) < 1e-2, "{code}");
                assert_ne!(n.owned(), e.owned());
            }
            let (l, r) = (e.next_left(), e.next_right());
            assert_ne!(l.owned(), r.owned());
        }
    }

    #[test]
    fn flow_picks_the_code_that_travels_that_way() {
        // East side: flows north (0) or south (3).
        let n = code_for_flow(4, 0);
        let (a, b) = HexEdge::new(0, 0, n).world_ends();
        assert!(b.y < a.y, "north is -Z");
        let s = code_for_flow(4, 3);
        let (a, b) = HexEdge::new(0, 0, s).world_ends();
        assert!(b.y > a.y);
        assert_ne!(n, s);
    }

    fn basis_with(rivers: &[(i32, i32, u8, i8)], water: &[(i32, i32)]) -> TerrainBasis {
        let mut b = TerrainBasis::uniform(
            8,
            8,
            false,
            1,
            PlotBasis {
                relief: Relief::Flat,
                terrain: TerrainKind::Grass,
                river_flow: PlotBasis::NO_FLOW,
                ..PlotBasis::default()
            },
        );
        for &(x, y) in water {
            b.plots[(y * 8 + x) as usize].relief = Relief::Water;
        }
        for &(x, y, dir, flow) in rivers {
            let i = (y * 8 + x) as usize;
            b.plots[i].rivers |= 1 << dir;
            let slot = [Dir::East, Dir::SouthEast, Dir::SouthWest]
                .iter()
                .position(|d| *d as u8 == dir)
                .unwrap();
            b.plots[i].river_flow[slot] = flow;
        }
        b
    }

    #[test]
    fn a_chain_of_edges_is_found_from_the_grid() {
        // East side of (3, 3) flowing north, then the next edge round the corner.
        let b = basis_with(&[(3, 3, Dir::East as u8, 0)], &[]);
        let g = RiverGrid::from_basis(&b);
        assert_eq!(g.edges().count(), 1);
        let e = g.edges().next().unwrap();
        assert!(g.is_river(e) && g.is_river(e.twin()));
        assert!(!g.is_river(e.reversed()));
        assert!(g.next_edge(e).is_none());
    }

    #[test]
    fn two_edges_in_a_row_are_chained() {
        // The east side of (3, 3) flowing north ends at its north-east corner. The north-east side of
        // (3, 3) is the east side of (3, 4)'s... use the geometry: find an edge that starts there.
        let b = basis_with(&[(3, 3, Dir::East as u8, 0)], &[]);
        let g = RiverGrid::from_basis(&b);
        let e = g.edges().next().unwrap();
        let end = e.world_ends().1;
        // Add the continuation by hand: the right continuation, owned form.
        let n = e.next_right();
        let mut g2 = g.clone();
        g2.add(n);
        assert_eq!(g2.next_edge(e).map(|x| x.owned()), Some(n.owned()));
        assert!(n.world_ends().0.distance(end) < 1e-2);
        assert_eq!(g2.prev_edge(n).map(|x| x.owned()), Some(e.owned()));
    }

    #[test]
    fn without_flow_the_edge_turns_toward_the_water() {
        // Water at the west, the river on an east side far from it flowing... fallback.
        let b = basis_with(
            &[(4, 4, Dir::East as u8, -1)],
            &[(7, 0), (7, 1), (7, 2), (7, 3), (7, 4)],
        );
        let g = RiverGrid::from_basis(&b);
        let e = g.edges().next().unwrap();
        let (a, c) = e.world_ends();
        let water = cell_center(7, 4);
        assert!(c.distance(water) <= a.distance(water));
    }

    #[test]
    fn side_points_lie_between_the_corners_in_the_flow_order() {
        let frame = TerrainFrame::new(8, 8, false);
        let e = HexEdge::new(1, 1, 3);
        let [p, q] = e.side_points(&frame);
        let (a, b) = e.world_ends();
        let (a, b) = (frame.world_to_tex(a), frame.world_to_tex(b));
        assert!(p.distance(a) < q.distance(a));
        assert!(p.distance(b) > q.distance(b));
    }
}
