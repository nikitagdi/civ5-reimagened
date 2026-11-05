//! The river simulation of `world-water`: `cvRiverSimSystem::StartupGame`
//! up to, not including, `CreateStencilMeshes` (that is `stencil.rs`). The rivers are gathered from
//! the grid of river edges, their control points are smoothed and jittered, the segments go into
//! the spline database, the rivers get their end caps, mouths and heights, and the two banks are
//! made.
//!
//! Order of the original, with the draws of `rand()` of each step:
//!
//! | step                              | draws                                                  |
//! |-----------------------------------|--------------------------------------------------------|
//! | `GatherControlPointsFromMap`      | none                                                   |
//! | `SmoothLowFrequency`              | none                                                   |
//! | `RandomizeLowFrequency`           | per river of 8+ points: see [`randomize_low_frequency`]|
//! | `AddSegmentsToGameSplineDatabase` | none                                                   |
//! | `AddRiverEndcaps`                 | none                                                   |
//! | `MarkRiverMouths`                 | none                                                   |
//! | `CalculateRiverHeights`           | none                                                   |
//! | `GenerateRiverBankControlPoints`  | per river: see [`generate_river_bank_control_points`]  |
//!
//! `SmoothRivers` (0x087f7c1a) is not called by `StartupGame` and is not ported.

use std::collections::HashSet;

use glam::{Vec2, Vec3};

use super::hex::{HexEdge, RiverGrid};
use super::spline_db::SplineDb;
use super::wavelet::RiverWavelet;
use super::{RiverSim, RiverSpline};
use crate::world::terrain::build::rand::MsvcRand;
use crate::world::terrain::sampler::TerrainFrame;

/// Edge codes of a hex: six sides in both directions.
// re: cvRiverSimSystem.cpp `kNumEdgeCodes`
const NUM_EDGE_CODES: u8 = 12;
/// Sides of a hex.
// re: cvRiverSimSystem.cpp `kNumHexSides`
const NUM_HEX_SIDES: u8 = 6;

/// Endcap points past a river mouth.
// re: cvRiverSimSystem.cpp `kNumEndcapPoints`
const NUM_ENDCAP_POINTS: usize = 4;
/// Texels between the endcap points, along the last hex side.
// re: cvRiverSimSystem.cpp `kEndcapSpacing`
const ENDCAP_SPACING: f32 = 16.0;

/// Coarse wavelet level offsets are `rand() % 10 - 5` texels.
// re: cvRiverSimSystem.cpp `kCoarseJitterRange`
const COARSE_JITTER_RANGE: u32 = 10;
// re: cvRiverSimSystem.cpp `kCoarseJitterBias`
const COARSE_JITTER_BIAS: f32 = -5.0;
/// Control point offsets are `rand() % 15 - 7.5` texels.
// re: cvRiverSimSystem.cpp `kFineJitterRange`
const FINE_JITTER_RANGE: u32 = 15;
// re: cvRiverSimSystem.cpp `kFineJitterBias`
const FINE_JITTER_BIAS: f32 = -7.5;
/// Rivers with fewer (even-rounded) control points are left alone by the smoothing and the
/// jitter (`6 < n` in `SmoothLowFrequency`, `n < 7` in `RandomizeLowFrequency`).
// re: cvRiverSimSystem.cpp `kMinRandomizedPoints`
const MIN_RANDOMIZED_POINTS: usize = 7;
/// The smoothing of the coarse level: `0.125 0.75 0.125`.
// re: SmoothLowFrequency 0x087f30e0 (DAT_0914e0a4 = 0.125, DAT_09149c84 = 0.75)
const SMOOTH_SIDE: f32 = 0.125;
const SMOOTH_MIDDLE: f32 = 0.75;

/// The start of the running minimum of the heights.
// re: CalculateRiverHeights 0x087f3b2c (DAT_0912c9f4 = -1.0)
const START_HEIGHT: f32 = -1.0;
/// `CalculateRiverHeights` wraps `x` with `(width * 100 + x) % width`.
// re: CalculateRiverHeights 0x087f3b2c
const WRAP_WIDTHS: i32 = 100;

/// Bank half widths in texels: base plus the taper, then by control point index.
// re: cvRiverSimSystem.cpp `kBankBaseWidth`
const BANK_BASE_WIDTH: f32 = 0.405;
// re: cvRiverSimSystem.cpp `kBankTaperStep`
const BANK_TAPER_STEP: f32 = 0.75;
// re: cvRiverSimSystem.cpp `kBankTaperMax`
const BANK_TAPER_MAX: f32 = 1.0;
// re: cvRiverSimSystem.cpp `kBankWidthAtEnds`
const BANK_WIDTH_AT_ENDS: f32 = 0.0;
// re: cvRiverSimSystem.cpp `kBankWidthSecondPair`
const BANK_WIDTH_SECOND_PAIR: f32 = 0.3;
// re: cvRiverSimSystem.cpp `kBankWidthThirdPair`
const BANK_WIDTH_THIRD_PAIR: f32 = 1.2;
// re: cvRiverSimSystem.cpp `kBankWidthRandomRange`
const BANK_WIDTH_RANDOM_RANGE: u32 = 5;
// re: cvRiverSimSystem.cpp `kBankEndRandomRange`
const BANK_END_RANDOM_RANGE: u32 = 3;
/// Points within this many of the end of a river that ends at a mouth flare out.
// re: cvRiverSimSystem.cpp `kMouthFlarePoints`
const MOUTH_FLARE_POINTS: usize = 4;
// re: cvRiverSimSystem.cpp `kRiverEndPoints`
const RIVER_END_POINTS: usize = 6;
// re: cvRiverSimSystem.cpp `kMouthFlareScale`
const MOUTH_FLARE_SCALE: f32 = 0.35;
// re: cvRiverSimSystem.cpp `kMouthFlareDivisor`
const MOUTH_FLARE_DIVISOR: f32 = 1.2;
/// A river with fewer control points gets no banks.
// re: cvRiverSimSystem.cpp `kMinBankPoints`
const MIN_BANK_POINTS: usize = 4;

/// `StartupGame` without the stencil: the rivers of the grid with their banks and heights.
///
/// `height_at(u, v)` is the raw height of the texel `(u, v)` of the field (`u` already wrapped,
/// `v` inside the field).
// re: cvRiverSimSystem::StartupGame 0x087f2a2a
pub fn build(
    grid: &RiverGrid,
    frame: &TerrainFrame,
    rand: &mut MsvcRand,
    height_at: &dyn Fn(i32, i32) -> u16,
) -> RiverSim {
    let mut db = SplineDb::new(*frame);
    let (edges, mut rivers) = gather_control_points_from_map(grid, &db);
    smooth_low_frequency(&mut rivers);
    randomize_low_frequency(&mut rivers, rand);
    add_segments_to_game_spline_database(&mut db, &rivers, &edges);
    add_river_endcaps(grid, &db, &mut rivers, &edges);
    mark_river_mouths(grid, &mut rivers, &edges);
    let heights = calculate_river_heights(&rivers, frame, height_at);
    let (left_banks, right_banks) = generate_river_bank_control_points(&rivers, rand);
    RiverSim {
        edges,
        rivers,
        left_banks,
        right_banks,
        heights,
        db,
    }
}

/// The edge, or its twin from the hex across it, is in the visited set (both wrapped).
// re: the lambda `IsEdgeVisited` of GatherControlPointsFromMap 0x087f2b8c (inlined there)
fn is_edge_visited(grid: &RiverGrid, visited: &HashSet<HexEdge>, edge: HexEdge) -> bool {
    grid.both_names(edge)
        .into_iter()
        .flatten()
        .any(|k| visited.contains(&k))
}

/// Walks up the river from `start` to its source, stopping below a stretch another river already
/// claimed. `None` if `start` is no river or was visited.
// re: cvRiverSimSystem::TraverseToRiverSource 0x087f5194
fn traverse_to_river_source(
    grid: &RiverGrid,
    start: HexEdge,
    visited: &HashSet<HexEdge>,
) -> Option<HexEdge> {
    if !grid.is_river(start) || is_edge_visited(grid, visited, start) {
        return None;
    }
    // Not in the original: a ring of river edges would walk for ever.
    let mut walked: HashSet<Option<HexEdge>> = HashSet::new();
    walked.insert(grid.key(start));
    let mut edge = start;
    loop {
        let Some(prev) = grid.prev_edge(edge) else {
            return Some(edge);
        };
        if is_edge_visited(grid, visited, prev) || !walked.insert(grid.key(prev)) {
            return Some(edge);
        }
        edge = prev;
    }
}

/// `AddControlPointsAlongRiver`: the two generated points of every side of the river.
// re: cvRiverSimSystem::AddControlPointsAlongRiver 0x087f7a6e
fn control_points_along_river(db: &SplineDb, edges: &[HexEdge]) -> RiverSpline {
    let mut river = RiverSpline::default();
    for e in edges {
        for c in db.query_generated_segment(*e) {
            river.points.push(c.pos.extend(c.extra));
        }
    }
    river
}

/// The rivers of the grid, from the source down, and their control points (two per edge).
// re: cvRiverSimSystem::GatherControlPointsFromMap 0x087f2b8c
fn gather_control_points_from_map(
    grid: &RiverGrid,
    db: &SplineDb,
) -> (Vec<Vec<HexEdge>>, Vec<RiverSpline>) {
    let mut edges: Vec<Vec<HexEdge>> = Vec::new();
    let mut rivers = Vec::new();
    let mut visited: HashSet<HexEdge> = HashSet::new();
    let shape = grid.shape();
    for x in 0..shape.width {
        for y in 0..shape.height {
            for code in 0..NUM_EDGE_CODES {
                // Sides 0..2 in both directions name every side of the map once.
                if code % NUM_HEX_SIDES >= 3 {
                    continue;
                }
                let start = HexEdge::of_cell(x, y, code);
                if !grid.is_river(start) || is_edge_visited(grid, &visited, start) {
                    continue;
                }
                let Some(source) = traverse_to_river_source(grid, start, &visited) else {
                    continue;
                };
                let mut river = Vec::new();
                let mut edge = source;
                loop {
                    edge = edge.owned();
                    if is_edge_visited(grid, &visited, edge) {
                        break;
                    }
                    river.push(edge);
                    if let Some(k) = grid.wrapped(edge) {
                        visited.insert(k);
                    }
                    match grid.next_edge(edge) {
                        Some(next) => edge = next,
                        None => break,
                    }
                }
                rivers.push(control_points_along_river(db, &river));
                edges.push(river);
            }
        }
    }
    (edges, rivers)
}

/// `n` (even) points of a river as the texel plane points the wavelet works on.
fn plane_points(river: &RiverSpline, n: usize) -> Vec<Vec2> {
    river.points[..n].iter().map(|p| p.truncate()).collect()
}

/// Puts the points of the wavelet back into the `x`, `y` of the river (`z` stays).
fn store_plane_points(river: &mut RiverSpline, pts: &[Vec2]) {
    for (p, q) in river.points.iter_mut().zip(pts) {
        p.x = q.x;
        p.y = q.y;
    }
}

/// Smooths the coarse level of every river with the filter `0.125 0.75 0.125`; the ends of the
/// coarse level and the detail stay. Draws nothing.
// re: cvRiverSimSystem::SmoothLowFrequency 0x087f30e0
fn smooth_low_frequency(rivers: &mut [RiverSpline]) {
    for river in rivers {
        let n = river.points.len() & !1;
        if n < MIN_RANDOMIZED_POINTS {
            continue;
        }
        // The original decomposes two copies: one to change, one to read the unchanged coarse
        // level from.
        let source = RiverWavelet::decompose(&plane_points(river, n));
        let mut smooth = source.clone();
        let c = &source.coarse;
        for i in 1..c.len() - 1 {
            smooth.coarse[i] =
                (c[i] * SMOOTH_MIDDLE + c[i - 1] * SMOOTH_SIDE) + c[i + 1] * SMOOTH_SIDE;
        }
        store_plane_points(river, &smooth.reconstruct());
    }
}

/// Moves the coarse level and then the points of every river by random offsets.
///
/// Draws per river of `n >= 8` (even-rounded) points: for each of the `n / 2 - 1` inner coarse
/// points one draw, and two more if it was even; then for each of the `n` points one draw, and two
/// more if it was even.
// re: cvRiverSimSystem::RandomizeLowFrequency 0x087f332e
// SIMPLIFIED(SIMP-040): river wiggle form is ours on the MSVC rand stream; not confirmed equal to the original's (D-3).
fn randomize_low_frequency(rivers: &mut [RiverSpline], rand: &mut MsvcRand) {
    for river in rivers {
        let n = river.points.len() & !1;
        if n < MIN_RANDOMIZED_POINTS {
            continue;
        }
        let mut wavelet = RiverWavelet::decompose(&plane_points(river, n));
        let count = wavelet.coarse.len();
        for p in &mut wavelet.coarse[1..count - 1] {
            if rand.next() & 1 == 0 {
                p.x += (rand.next() % COARSE_JITTER_RANGE) as f32 + COARSE_JITTER_BIAS;
                p.y += (rand.next() % COARSE_JITTER_RANGE) as f32 + COARSE_JITTER_BIAS;
            }
        }
        store_plane_points(river, &wavelet.reconstruct());
        for p in &mut river.points[..n] {
            if rand.next() & 1 == 0 {
                p.x += (rand.next() % FINE_JITTER_RANGE) as f32 + FINE_JITTER_BIAS;
                p.y += (rand.next() % FINE_JITTER_RANGE) as f32 + FINE_JITTER_BIAS;
            }
        }
    }
}

/// Saves the final points of every side of the rivers in the spline database: the points `2i` and
/// `2i + 1` of a river belong to its edge `i`.
// re: cvRiverSimSystem::AddSegmentsToGameSplineDatabase 0x087f35c8
fn add_segments_to_game_spline_database(
    db: &mut SplineDb,
    rivers: &[RiverSpline],
    edges: &[Vec<HexEdge>],
) {
    for (river, river_edges) in rivers.iter().zip(edges) {
        let n = river.points.len() & !1;
        for (i, pair) in river.points[..n].chunks_exact(2).enumerate() {
            if let Some(edge) = river_edges.get(i) {
                db.submit(*edge, &[pair[0].truncate(), pair[1].truncate()]);
            }
        }
    }
}

/// A tributary runs on along the side it flows into; a river that ends at a mouth runs on into
/// the water along its last side.
// re: cvRiverSimSystem::AddRiverEndcaps 0x087f36ba
fn add_river_endcaps(
    grid: &RiverGrid,
    db: &SplineDb,
    rivers: &mut [RiverSpline],
    edges: &[Vec<HexEdge>],
) {
    for (river, river_edges) in rivers.iter_mut().zip(edges) {
        let Some(&last) = river_edges.last() else {
            continue;
        };
        if let Some(next) = grid.next_edge(last) {
            for c in db.query_segment(next) {
                river.points.push(c.pos.extend(c.extra));
            }
            continue;
        }
        let side = db.query_generated_segment(last);
        if side.len() < 2 || river.points.len() < 2 {
            continue;
        }
        let end = river.points[river.points.len() - 1];
        let step = (side[1].pos - side[0].pos).normalize_or_zero() * ENDCAP_SPACING;
        let (mut x, mut y) = (end.x, end.y);
        for _ in 0..NUM_ENDCAP_POINTS {
            x += step.x;
            y += step.y;
            river.points.push(Vec3::new(x, y, 0.0));
        }
    }
}

/// A river whose last edge has no river downstream ends at a mouth.
// re: cvRiverSimSystem::MarkRiverMouths 0x087f3a8a
fn mark_river_mouths(grid: &RiverGrid, rivers: &mut [RiverSpline], edges: &[Vec<HexEdge>]) {
    for (river, river_edges) in rivers.iter_mut().zip(edges) {
        if let Some(&last) = river_edges.last() {
            river.ends_at_mouth = grid.next_edge(last).is_none();
        }
    }
}

/// The height of every control point: the running minimum of the field heights along the river,
/// so a river never runs uphill. Points outside the rows of the field get no height.
// re: cvRiverSimSystem::CalculateRiverHeights 0x087f3b2c
fn calculate_river_heights(
    rivers: &[RiverSpline],
    frame: &TerrainFrame,
    height_at: &dyn Fn(i32, i32) -> u16,
) -> Vec<Vec<f32>> {
    let (w, h) = frame.texel_size();
    let (width, height) = (w as i32, h as i32);
    rivers
        .iter()
        .map(|river| {
            let mut heights = Vec::with_capacity(river.points.len() + 16);
            let mut running = START_HEIGHT;
            for p in &river.points {
                let v = p.y as i32;
                if v < 0 || v >= height {
                    continue;
                }
                let u = ((p.x as i32).wrapping_add(width * WRAP_WIDTHS)) % width;
                let raw = f32::from(height_at(u, v));
                let low = if running < 0.0 || running > raw {
                    raw
                } else {
                    running
                };
                running = low;
                heights.push(low.min(raw));
            }
            heights
        })
        .collect()
}

/// `D3DXVec3Normalize`: the zero vector stays zero.
fn normalize3(v: Vec3) -> Vec3 {
    v.normalize_or_zero()
}

/// The banks of every river: the control points moved to the left and to the right by a width
/// that grows from the source and flares at a mouth.
///
/// Draws per river of `n >= 4` points: two for every point `i >= 6` that is not in the first
/// three branches (all of them but the flaring end of a mouth), and, for a river that does not end
/// at a mouth, two more for every `i >= n - 6` with `i != 0`.
// re: cvRiverSimSystem::GenerateRiverBankControlPoints 0x087f3d52
fn generate_river_bank_control_points(
    rivers: &[RiverSpline],
    rand: &mut MsvcRand,
) -> (Vec<RiverSpline>, Vec<RiverSpline>) {
    let mut lefts: Vec<RiverSpline> = Vec::with_capacity(rivers.len());
    let mut rights: Vec<RiverSpline> = Vec::with_capacity(rivers.len());
    for river in rivers {
        let copy = RiverSpline {
            points: river.points.clone(),
            ends_at_mouth: false,
        };
        lefts.push(copy.clone());
        rights.push(copy);
    }

    let up = Vec3::Z;
    for (r, river) in rivers.iter().enumerate() {
        let pts = &river.points;
        let n = pts.len();
        let mut taper = 0.0f32;
        for i in 0..n {
            if n < MIN_BANK_POINTS {
                continue;
            }
            let dir = if i == 0 {
                Vec3::new(pts[1].x - pts[0].x, pts[1].y - pts[0].y, 0.0)
            } else if i == n - 1 {
                Vec3::new(pts[i].x - pts[i - 1].x, pts[i].y - pts[i - 1].y, 0.0)
            } else {
                let center = Vec3::new(pts[i].x, pts[i].y, 0.0);
                let to_prev = normalize3(Vec3::new(
                    pts[i - 1].x - center.x,
                    pts[i - 1].y - center.y,
                    0.0,
                ));
                let to_next = normalize3(Vec3::new(
                    pts[i + 1].x - center.x,
                    pts[i + 1].y - center.y,
                    0.0,
                ));
                Vec3::new(
                    (center.x + to_next.x) - (to_prev.x + center.x),
                    (center.y + to_next.y) - (to_prev.y + center.y),
                    (to_next.z + center.z) - (to_prev.z + center.z),
                )
            };
            let dir = normalize3(dir);
            // dir x up: the right-hand side of the flow.
            let side = normalize3(Vec3::new(
                dir.y * up.z - dir.z * up.y,
                dir.z * up.x - dir.x * up.z,
                dir.x * up.y - dir.y * up.x,
            ));

            taper = (taper + BANK_TAPER_STEP).min(BANK_TAPER_MAX);
            let mouth = river.ends_at_mouth;
            let base = BANK_BASE_WIDTH + taper;
            let (mut left_w, mut right_w);
            if i < 2 || (i + MOUTH_FLARE_POINTS >= n && mouth) {
                left_w = base + BANK_WIDTH_AT_ENDS;
                right_w = left_w;
            } else if i < 4 {
                left_w = base + BANK_WIDTH_SECOND_PAIR;
                right_w = left_w;
            } else if i < 6 {
                left_w = base + BANK_WIDTH_THIRD_PAIR;
                right_w = left_w;
            } else {
                left_w = (rand.next() % BANK_WIDTH_RANDOM_RANGE) as f32 + base;
                right_w = (rand.next() % BANK_WIDTH_RANDOM_RANGE) as f32 + base;
            }
            if !mouth {
                // The unsigned `n - 6` of the original wraps for a short river: no end points.
                if (i as u32) >= (n as u32).wrapping_sub(RIVER_END_POINTS as u32) && i != 0 {
                    left_w = (rand.next() % BANK_END_RANDOM_RANGE) as f32 + base;
                    right_w = (rand.next() % BANK_END_RANDOM_RANGE) as f32 + base;
                }
            } else if i + MOUTH_FLARE_POINTS >= n {
                let from_flare_start = (i + 2 * MOUTH_FLARE_POINTS - n) as f32;
                let flare =
                    from_flare_start * from_flare_start * MOUTH_FLARE_SCALE / MOUTH_FLARE_DIVISOR;
                left_w += flare;
                right_w += flare;
            }

            let center = pts[i];
            lefts[r].points[i].x = center.x - left_w * side.x;
            lefts[r].points[i].y = center.y - left_w * side.y;
            rights[r].points[i].x = side.x * right_w + center.x;
            rights[r].points[i].y = side.y * right_w + center.y;
        }
    }
    (lefts, rights)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAP: u32 = 24;

    fn frame() -> TerrainFrame {
        TerrainFrame::new(MAP, MAP, false)
    }

    fn noisy(u: i32, v: i32) -> u16 {
        (20_000 + ((u * 37 + v * 91) % 1000)) as u16
    }

    /// `steps` edges from `start`, alternating the left and right continuation.
    fn chain(start: HexEdge, steps: usize) -> Vec<HexEdge> {
        let mut v = vec![start];
        let mut e = start;
        for k in 1..steps {
            e = if k % 2 == 0 {
                e.next_left()
            } else {
                e.next_right()
            };
            v.push(e);
        }
        v
    }

    fn grid_of(chains: &[&[HexEdge]]) -> RiverGrid {
        let mut g = RiverGrid::new(frame().grid_shape());
        for c in chains {
            for e in *c {
                g.add(*e);
            }
        }
        g
    }

    fn owned(c: &[HexEdge]) -> Vec<HexEdge> {
        c.iter().map(|e| e.owned()).collect()
    }

    fn xy(p: Vec3) -> Vec2 {
        p.truncate()
    }

    fn build_with(grid: &RiverGrid, seed: u32) -> (RiverSim, u64) {
        let mut rand = MsvcRand::new(seed);
        let sim = build(grid, &frame(), &mut rand, &noisy);
        (sim, rand.calls())
    }

    fn straight_chain() -> Vec<HexEdge> {
        chain(HexEdge::of_cell(12, 12, 0), 6)
    }

    #[test]
    fn no_rivers_make_nothing_and_draw_nothing() {
        let g = RiverGrid::new(frame().grid_shape());
        let (sim, calls) = build_with(&g, 1);
        assert!(sim.edges.is_empty() && sim.rivers.is_empty() && sim.heights.is_empty());
        assert!(sim.left_banks.is_empty() && sim.right_banks.is_empty() && sim.db.is_empty());
        assert_eq!(calls, 0);
    }

    #[test]
    fn a_river_to_the_sea_runs_from_the_source_with_two_points_per_edge() {
        let c = straight_chain();
        let g = grid_of(&[&c]);
        let (sim, _) = build_with(&g, 3);
        assert_eq!(sim.rivers.len(), 1);
        assert_eq!(sim.edges[0], owned(&c));
        let r = &sim.rivers[0];
        assert!(r.ends_at_mouth);
        assert_eq!(r.points.len(), 2 * c.len() + NUM_ENDCAP_POINTS);
        // The endcap points are 16 texels apart in a line along the last side.
        let n = r.points.len();
        let d = xy(r.points[n - 1]) - xy(r.points[n - 2]);
        for k in 1..NUM_ENDCAP_POINTS {
            let step = xy(r.points[n - k]) - xy(r.points[n - k - 1]);
            assert!((step.length() - ENDCAP_SPACING).abs() < 1e-3, "{k}: {step}");
            assert!(step.distance(d) < 1e-3);
        }
        // The line follows the generated side of the last edge.
        let side = sim.db.query_generated(*c.last().unwrap());
        let along = (side[1] - side[0]).normalize();
        assert!(d.normalize().distance(along) < 1e-3);
        // Heights: one per point, never rising downstream, never above the field.
        let h = &sim.heights[0];
        assert_eq!(h.len(), n);
        assert!(h.windows(2).all(|w| w[1] <= w[0]), "{h:?}");
        for (p, hh) in r.points.iter().zip(h) {
            assert!(*hh <= f32::from(noisy(p.x as i32, p.y as i32)));
        }
        assert_eq!(
            h[0],
            f32::from(noisy(r.points[0].x as i32, r.points[0].y as i32))
        );
    }

    #[test]
    fn the_banks_lie_on_both_sides_at_the_widths_of_the_formulas() {
        let c = straight_chain();
        let (sim, _) = build_with(&grid_of(&[&c]), 3);
        let river = &sim.rivers[0];
        let (left, right) = (&sim.left_banks[0], &sim.right_banks[0]);
        assert_eq!(left.points.len(), river.points.len());
        assert_eq!(right.points.len(), river.points.len());
        let base = BANK_BASE_WIDTH;
        // The first six points have fixed widths: taper 0.75 then 1.
        let widths = [
            base + 0.75,
            base + 1.0,
            base + 1.0 + BANK_WIDTH_SECOND_PAIR,
            base + 1.0 + BANK_WIDTH_SECOND_PAIR,
            base + 1.0 + BANK_WIDTH_THIRD_PAIR,
            base + 1.0 + BANK_WIDTH_THIRD_PAIR,
        ];
        let n = river.points.len();
        for (i, w) in widths.iter().enumerate() {
            let c = xy(river.points[i]);
            let (l, r) = (xy(left.points[i]), xy(right.points[i]));
            assert!((l.distance(c) - w).abs() < 1e-3, "{i}: {}", l.distance(c));
            assert!((r.distance(c) - w).abs() < 1e-3, "{i}");
            // Opposite sides of the centre line, across the flow.
            assert!(((l + r) * 0.5).distance(c) < 1e-3);
            let dir = if i == 0 {
                xy(river.points[1]) - xy(river.points[0])
            } else {
                let to_next = (xy(river.points[i + 1]) - c).normalize();
                let to_prev = (xy(river.points[i - 1]) - c).normalize();
                to_next - to_prev
            };
            assert!((r - l).normalize().dot(dir.normalize()).abs() < 1e-3, "{i}");
        }
        // The right bank is on the right of the flow: dir x up = (dir.y, -dir.x).
        let dir = xy(river.points[1]) - xy(river.points[0]);
        let right_of_flow = Vec2::new(dir.y, -dir.x);
        assert!((xy(right.points[0]) - xy(river.points[0])).dot(right_of_flow) > 0.0);
        // The mouth flares: the last point is wider by (7 * 7) * 0.35 / 1.2.
        let c = xy(river.points[n - 1]);
        let flare = 49.0 * MOUTH_FLARE_SCALE / MOUTH_FLARE_DIVISOR;
        let w = base + 1.0 + flare;
        assert!((xy(left.points[n - 1]).distance(c) - w).abs() < 1e-3);
        // z is the river's.
        assert_eq!(left.points[2].z, river.points[2].z);
    }

    /// The edges that lead into `target`, found among the edges of the hexes around it.
    fn predecessors(target: HexEdge) -> Vec<HexEdge> {
        let mut out: Vec<HexEdge> = Vec::new();
        for dx in -2..=2 {
            for dy in -2..=2 {
                for code in 0..12u8 {
                    let x = HexEdge::new(target.x + dx, target.y + dy, code);
                    let leads = x.next_left().owned() == target.owned()
                        || x.next_right().owned() == target.owned();
                    if leads && !out.iter().any(|o| o.owned() == x.owned()) {
                        out.push(x);
                    }
                }
            }
        }
        out
    }

    fn middle(e: HexEdge) -> Vec2 {
        let (a, b) = e.world_ends();
        (a + b) * 0.5
    }

    /// A branch of `steps` edges that ends in a feeder of `target` and walks away from it, never
    /// over an edge of `main`.
    fn branch_into(target: HexEdge, steps: usize, main: &[HexEdge]) -> Vec<HexEdge> {
        let from = middle(target);
        let mut avoid: HashSet<HexEdge> = main.iter().map(|e| e.owned()).collect();
        let (mut out, mut t) = (Vec::new(), target);
        for _ in 0..steps {
            let p = predecessors(t)
                .into_iter()
                .filter(|p| !avoid.contains(&p.owned()))
                .max_by(|a, b| {
                    middle(*a)
                        .distance(from)
                        .total_cmp(&middle(*b).distance(from))
                })
                .expect("a feeder");
            avoid.insert(p.owned());
            out.push(p);
            t = p;
        }
        out.reverse();
        out
    }

    /// Every edge of the branch leads into the next one and the last into `target`.
    fn assert_leads(g: &RiverGrid, branch: &[HexEdge], target: HexEdge) {
        for w in branch.windows(2) {
            assert_eq!(g.next_edge(w[0]).map(|e| e.owned()), Some(w[1].owned()));
        }
        let last = *branch.last().unwrap();
        assert_eq!(g.next_edge(last).map(|e| e.owned()), Some(target.owned()));
    }

    #[test]
    fn a_tributary_runs_on_along_the_side_it_flows_into() {
        let main = chain(HexEdge::of_cell(12, 12, 0), 8);
        let join = main[4];
        let trib = branch_into(join, 4, &main);
        let g = grid_of(&[&main, &trib]);
        assert_leads(&g, &main[..5], main[5]);
        assert_leads(&g, &trib, join);
        let (sim, _) = build_with(&g, 5);
        assert_eq!(sim.rivers.len(), 2);
        // Every edge once, no loop.
        let all: Vec<HexEdge> = sim.edges.iter().flatten().copied().collect();
        let set: HashSet<HexEdge> = all.iter().copied().collect();
        assert_eq!(all.len(), set.len());
        assert_eq!(set.len(), main.len() + trib.len());
        // One river ends at the sea, the other flows into it.
        assert_eq!(sim.rivers.iter().filter(|r| r.ends_at_mouth).count(), 1);
        let (i, river) = sim
            .rivers
            .iter()
            .enumerate()
            .find(|(_, r)| !r.ends_at_mouth)
            .unwrap();
        let last = *sim.edges[i].last().unwrap();
        let next = g.next_edge(last).expect("flows on");
        let seg = sim.db.query_segment(next);
        let n = river.points.len();
        assert_eq!(n, 2 * sim.edges[i].len() + seg.len());
        for (k, c) in seg.iter().enumerate() {
            let p = river.points[n - seg.len() + k];
            assert_eq!(xy(p), c.pos);
            assert_eq!(p.z, c.extra);
        }
        // The side it runs along is the one of the edge it joins, and was saved by the other river
        // (the float is 1) unless that river was gathered after it.
        assert!(seg.iter().all(|c| c.extra == 0.0 || c.extra == 1.0));
    }

    #[test]
    fn a_confluence_has_no_loop_and_every_river_is_a_chain() {
        let a = chain(HexEdge::of_cell(12, 12, 0), 7);
        let b = branch_into(a[3], 3, &a);
        let g = grid_of(&[&a, &b]);
        assert_leads(&g, &b, a[3]);
        let (sim, _) = build_with(&g, 9);
        let count: usize = sim.edges.iter().map(Vec::len).sum();
        assert_eq!(count, g.edges().count());
        for river in &sim.edges {
            for w in river.windows(2) {
                assert_eq!(g.next_edge(w[0]).map(|e| e.owned()), Some(w[1].owned()));
            }
        }
        let seen: HashSet<HexEdge> = sim.edges.iter().flatten().copied().collect();
        assert_eq!(seen.len(), count);
    }
    #[test]
    fn the_saved_segments_are_the_final_points_of_each_edge() {
        let c = straight_chain();
        let (sim, _) = build_with(&grid_of(&[&c]), 11);
        for (i, e) in sim.edges[0].iter().enumerate() {
            let seg = sim.db.query_saved(*e).expect("saved");
            assert_eq!(seg.len(), 2);
            assert!(seg[0].pos.distance(xy(sim.rivers[0].points[2 * i])) < 1e-2);
            assert!(seg[1].pos.distance(xy(sim.rivers[0].points[2 * i + 1])) < 1e-2);
            let back = sim.db.query_saved(e.reversed()).expect("saved reversed");
            assert!(back[0].pos.distance(seg[1].pos) < 1e-2);
        }
    }

    #[test]
    fn the_same_seed_gives_the_same_rivers() {
        let c = straight_chain();
        let g = grid_of(&[&c]);
        let (a, ca) = build_with(&g, 77);
        let (b, cb) = build_with(&g, 77);
        assert_eq!(ca, cb);
        assert_eq!(a.edges, b.edges);
        assert_eq!(a.rivers, b.rivers);
        assert_eq!(a.left_banks, b.left_banks);
        assert_eq!(a.right_banks, b.right_banks);
        assert_eq!(a.heights, b.heights);
        let (other, _) = build_with(&g, 78);
        assert_ne!(a.rivers, other.rivers);
    }

    /// A river of `n` points with a bend.
    fn bent(n: usize, mouth: bool) -> RiverSpline {
        RiverSpline {
            points: (0..n)
                .map(|i| {
                    Vec3::new(
                        100.0 + 9.0 * i as f32,
                        200.0 + 6.0 * (i as f32 * 0.8).sin(),
                        0.0,
                    )
                })
                .collect(),
            ends_at_mouth: mouth,
        }
    }

    /// The draws a stream of this seed would make for `count` bits, each followed by two more
    /// draws if even.
    fn draws_with_even_extras(seed: u32, count: usize) -> u64 {
        let mut s = MsvcRand::new(seed);
        for _ in 0..count {
            if s.next() & 1 == 0 {
                s.skip(2);
            }
        }
        s.calls()
    }

    #[test]
    fn randomize_draws_one_per_point_and_two_more_for_the_even_ones() {
        for n in [8usize, 12, 20] {
            let mut rivers = vec![bent(n, false)];
            let mut rand = MsvcRand::new(4);
            randomize_low_frequency(&mut rivers, &mut rand);
            // Coarse inner points, then the points: both walk the same stream.
            let expected = draws_with_even_extras(4, n / 2 - 1 + n);
            assert_eq!(rand.calls(), expected, "n = {n}");
        }
    }

    #[test]
    fn randomize_and_smooth_leave_short_rivers_alone() {
        for n in [0usize, 2, 4, 6] {
            let mut rivers = vec![bent(n, false)];
            let before = rivers.clone();
            let mut rand = MsvcRand::new(4);
            randomize_low_frequency(&mut rivers, &mut rand);
            smooth_low_frequency(&mut rivers);
            assert_eq!(rand.calls(), 0);
            assert_eq!(rivers[0], before[0]);
        }
        // An odd count is rounded down: 9 points are 8.
        let mut rivers = vec![bent(9, false)];
        let before = rivers[0].points[8];
        let mut rand = MsvcRand::new(4);
        randomize_low_frequency(&mut rivers, &mut rand);
        assert_eq!(rivers[0].points[8], before);
        assert_eq!(rand.calls(), draws_with_even_extras(4, 3 + 8));
    }

    #[test]
    fn smoothing_draws_nothing_keeps_a_line_and_flattens_a_wiggle() {
        let mut line = RiverSpline {
            points: (0..12)
                .map(|i| Vec3::new(i as f32 * 5.0, 30.0, 0.0))
                .collect(),
            ends_at_mouth: false,
        };
        let before = line.clone();
        smooth_low_frequency(std::slice::from_mut(&mut line));
        for (a, b) in line.points.iter().zip(&before.points) {
            assert!(a.distance(*b) < 1e-3);
        }
        // The coarse level is what is smoothed.
        let roughness = |r: &RiverSpline| -> f32 {
            let coarse = RiverWavelet::decompose(&plane_points(r, r.points.len())).coarse;
            coarse
                .windows(3)
                .map(|w| (w[0] - 2.0 * w[1] + w[2]).length())
                .sum()
        };
        let mut wiggle = RiverSpline {
            points: (0..16)
                .map(|i| {
                    Vec3::new(
                        i as f32 * 5.0,
                        30.0 + if i % 4 < 2 { 6.0 } else { -6.0 },
                        0.0,
                    )
                })
                .collect(),
            ends_at_mouth: false,
        };
        let before = roughness(&wiggle);
        smooth_low_frequency(std::slice::from_mut(&mut wiggle));
        assert!(roughness(&wiggle) < before);
    }

    #[test]
    fn the_banks_draw_by_the_rules_for_rivers_with_and_without_a_mouth() {
        // A mouth: only the points 6 .. n - 4 draw (two each).
        let (n, seed) = (16usize, 21);
        let mut rand = MsvcRand::new(seed);
        generate_river_bank_control_points(&[bent(n, true)], &mut rand);
        assert_eq!(rand.calls() as usize, 2 * (n - MOUTH_FLARE_POINTS - 6));
        // No mouth: the points 6 .. n draw, and the last six draw again.
        let mut rand = MsvcRand::new(seed);
        generate_river_bank_control_points(&[bent(n, false)], &mut rand);
        assert_eq!(rand.calls() as usize, 2 * (n - 6) + 2 * RIVER_END_POINTS);
        // Six points without a mouth: every point but the first draws for the end. Five points:
        // the unsigned `n - 6` wraps and nothing draws.
        let mut rand = MsvcRand::new(seed);
        generate_river_bank_control_points(&[bent(6, false)], &mut rand);
        assert_eq!(rand.calls() as usize, 2 * 5);
        let mut rand = MsvcRand::new(seed);
        generate_river_bank_control_points(&[bent(5, false)], &mut rand);
        assert_eq!(rand.calls(), 0);
        // Fewer than four points: nothing.
        let mut rand = MsvcRand::new(seed);
        let (l, r) = generate_river_bank_control_points(&[bent(3, false)], &mut rand);
        assert_eq!(rand.calls(), 0);
        assert_eq!(l[0].points, bent(3, false).points);
        assert_eq!(r[0].points, bent(3, false).points);
    }

    #[test]
    fn the_draws_of_a_whole_build_are_the_sum_of_the_steps() {
        let c = straight_chain();
        let g = grid_of(&[&c]);
        let (sim, calls) = build_with(&g, 13);
        let n0 = 2 * c.len();
        // The randomizing, replayed on the stream.
        let mut s = MsvcRand::new(13);
        for _ in 0..(n0 / 2 - 1 + n0) {
            if s.next() & 1 == 0 {
                s.skip(2);
            }
        }
        let n = sim.rivers[0].points.len();
        assert!(sim.rivers[0].ends_at_mouth);
        let banks = 2 * (n - MOUTH_FLARE_POINTS - 6) as u64;
        assert_eq!(calls, s.calls() + banks);
    }

    #[test]
    fn heights_wrap_x_and_skip_points_outside_the_rows() {
        let river = RiverSpline {
            points: vec![
                Vec3::new(-3.0, 10.0, 0.0),
                Vec3::new(5.0, -4.0, 0.0),
                Vec3::new(7.0, 10.0, 0.0),
                Vec3::new(9.0, 5000.0, 0.0),
                Vec3::new(11.0, 12.0, 0.0),
            ],
            ends_at_mouth: false,
        };
        let seen = std::cell::RefCell::new(Vec::new());
        let at = |u: i32, v: i32| -> u16 {
            seen.borrow_mut().push((u, v));
            match seen.borrow().len() {
                1 => 500,
                2 => 800,
                _ => 300,
            }
        };
        let h = calculate_river_heights(&[river], &frame(), &at);
        assert_eq!(h, vec![vec![500.0, 500.0, 300.0]]);
        let (w, _) = frame().texel_size();
        assert_eq!(seen.borrow()[0], (w as i32 - 3, 10));
        assert_eq!(seen.borrow().len(), 3);
    }
}
