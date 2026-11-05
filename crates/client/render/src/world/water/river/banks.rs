//! The bank strips of `cvFloodplainsSystem` (world-water WW-4, RW-12): the sims (the runs of river sides
//! that have a marked hex on one bank), the outline of the bank around the hexes
//! (`ExtrudeControlPoints`, `CountInternalEdges`), the end caps of the control points (`AddEndcaps`)
//! and the strip of quads along the river (`BuildBankVis`).
//!
//! Everything is in the texel plane of the terrain field until the last step, which turns the
//! vertices into our world.

// The constants and the comparisons are those of the original (NaN included), kept as they are.
#![allow(clippy::excessive_precision, clippy::needless_range_loop)]
// re: cvFloodplainsSystem.cpp (ExtrudeControlPoints 0x08663ce8, CountInternalEdges 0x08664806,
// BuildBankVis 0x0866217c, AddEndcaps<...> 0x08664cc2 / 0x08664e23)

use glam::{Vec2, Vec3};

use super::hex::{HexEdge, RiverGrid};
use crate::hexgrid::{HEX_WIDTH, ROW_STEP};
use crate::world::terrain::sampler::TerrainFrame;
use crate::world::water::coast::bspline;

/// Which bank of the river a sim is on.
// re: cvFloodplainsSim.h `FloodplainsRiverSide`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BankSide {
    Left,
    Right,
}

/// A run of sides of one river that have a marked hex on `side`, in the owned form, from the source
/// down.
// re: cvFloodplainsSim.h `cvFloodplainsSim`
#[derive(Clone, Debug, PartialEq)]
pub struct BankSim {
    pub side: BankSide,
    pub edges: Vec<HexEdge>,
}

impl BankSim {
    pub fn new(side: BankSide) -> Self {
        Self {
            side,
            edges: Vec::new(),
        }
    }
}

/// Rotation of the bank pattern per side, in sixths of a turn.
// re: cvFloodplainsSystem.cpp `kSideRotation`
pub const SIDE_ROTATION: [f32; 6] = [1.0, 3.0, 5.0, 4.0, 0.0, 2.0];
/// A sixth of a turn as the original has it (not exactly pi / 3).
// re: cvFloodplainsSystem.cpp `kSixthTurn`
pub const SIXTH_TURN: f32 = 1.047_166_7;
/// Bank width in texels for a run of 1..5 sides around one hex.
// re: cvFloodplainsSystem.cpp `kRunWidth`
pub const RUN_WIDTH: [f32; 5] = [35.0, 30.0, 25.0, 20.0, 15.0];
/// Spline parameter step of the strip.
// re: cvFloodplainsSystem.cpp `kSplineStep`
pub const SPLINE_STEP: f32 = 0.25;
/// Texels to `u` of the strip.
// re: cvFloodplainsSystem.cpp `kTexelToU`
pub const TEXEL_TO_U: f32 = 0.003_906_25;
/// The end caps lie this far (of the last step) beyond the ends.
// re: AddEndcaps<...> 0x08664cc2 (the float at [ebx-0x11da618] = 0.5)
pub const ENDCAP_FRACTION: f32 = 0.5;
/// Six triangles fanning round the centre vertex (3) of each quad, relative to the quad's first vertex.
// re: cvFloodplainsSystem.cpp `kStripIndexOffsets`
pub const STRIP_INDEX_OFFSETS: [i32; 18] =
    [-4, 3, -3, -3, 3, -2, -4, 0, 3, -2, 3, 2, 3, 0, 1, 3, 1, 2];

/// Left-bank outline in cube space for a run of 1..5 sides; unused points stay zero.
// re: ExtrudeControlPoints 0x08663ce8 (`aaLeftPattern`)
const LEFT_PATTERN: [[[f32; 3]; 6]; 5] = [
    [
        [0.5, -0.5, 0.0],
        [0.0, 0.5, -0.5],
        [0.0; 3],
        [0.0; 3],
        [0.0; 3],
        [0.0; 3],
    ],
    [
        [0.5, -0.5, 0.0],
        [0.099_999_994, 0.099_999_994, -0.199_999_99],
        [-0.5, 0.5, 0.0],
        [0.0; 3],
        [0.0; 3],
        [0.0; 3],
    ],
    [
        [0.5, -0.5, 0.0],
        [0.154_166_67, -0.008_333_355_2, -0.145_833_31],
        [-0.154_166_67, 0.145_833_28, 0.008_333_385],
        [-0.5, 0.0, 0.5],
        [0.0; 3],
        [0.0; 3],
    ],
    [
        [0.5, -0.5, 0.0],
        [0.154_166_67, -0.008_333_355_2, -0.145_833_31],
        [-0.099_999_994, 0.199_999_99, -0.099_999_994],
        [-0.145_833_28, -0.008_333_355_2, 0.154_166_64],
        [0.0, -0.5, 0.5],
        [0.0; 3],
    ],
    [
        [0.5, -0.5, 0.0],
        [0.154_166_67, -0.008_333_355_2, -0.145_833_31],
        [-0.099_999_994, 0.199_999_99, -0.099_999_994],
        [-0.199_999_99, 0.099_999_994, 0.099_999_994],
        [0.008_333_355_2, -0.154_166_67, 0.145_833_31],
        [0.5, -0.5, 0.0],
    ],
];

/// The right pattern: the left one through `HEXMatrix::REFLECTIONS[6]`.
// TODO(RW-12): the table is `.bss` data (zero in the dump). By the geometry the reflection is the
// mirror across the north-south axis through the hex: the pattern, put next to the hex on the
// right (`+ E`), then runs along the same river side in the same direction. In cube space that is
// `(x, y, z) -> (z, y, x)`.
fn mirror(p: [f32; 3]) -> [f32; 3] {
    [p[2], p[1], p[0]]
}

/// `1 / sqrt(3)`, the double the rotation of the original reads.
// re: HEXMatRotate 0x0868df1d (double at [ebx-0x11bb600] = 0x3fe279a74590331d)
const INV_SQRT3: f64 = 0.577_350_269_189_625_7;

/// The turn of cube vectors by `angle` radians (the matrix of `HEXMatRotate`), for a row vector: a
/// positive angle turns counter-clockwise on the map, north up; a sixth of a turn takes E to NE.
// re: HEXMatRotate 0x0868df1d
pub fn rotate_cube(p: [f32; 3], angle: f32) -> [f32; 3] {
    let a = f64::from(angle);
    let (c, s) = (a.cos(), a.sin());
    // Entries as the original computes them in double and stores them as float.
    let minus_c_plus = (-(c - s * INV_SQRT3)) as f32; // m01, m06, m08
    let minus_c_minus = (-c - s * INV_SQRT3) as f32; // m02, m04, m09
    let [x, y, z] = p;
    [
        y * minus_c_minus + z * minus_c_plus,
        x * minus_c_plus + z * minus_c_minus,
        x * minus_c_minus + y * minus_c_plus,
    ]
}

/// The point of the texel plane at the cube point `(x, y, z)` (floats): the float generalisation of
/// `hexgrid::cell_center` for a cube hex (`x = col - (row >> 1)`, `y = row`), and the frame.
// re: WorldGlobals::g_HexToTex (a matrix in the original, RW-6)
pub fn cube_to_tex(frame: &TerrainFrame, p: Vec3) -> Vec2 {
    frame.world_to_tex(Vec2::new(HEX_WIDTH * (p.x + 0.5 * p.y), -ROW_STEP * p.y))
}

/// The same side named from the hex across it (`EdgeAcross`).
fn across(e: HexEdge) -> HexEdge {
    e.twin()
}

/// Number of sides from `start` on that turn round one hex on the sim's side; at least 1.
// re: cvFloodplainsSystem::CountInternalEdges 0x08664806
pub fn count_internal_edges(grid: &RiverGrid, sim: &BankSim, start: usize) -> usize {
    let Some(last) = sim.edges.len().checked_sub(1) else {
        return 1;
    };
    if start >= last {
        return 1;
    }
    let mut e = start;
    loop {
        let next = match sim.side {
            BankSide::Left => grid.next_left_edge(sim.edges[e]),
            BankSide::Right => grid.next_right_edge(sim.edges[e]),
        };
        let Some(next) = next else { break };
        let following = sim.edges[e + 1];
        if following != next && following != across(next) {
            break;
        }
        e += 1;
        if e >= last {
            break;
        }
    }
    e - start + 1
}

/// The outline of the bank of a sim: points and widths cut at the corners (`points`, `widths`) and
/// the uncut ones.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outline {
    pub points: Vec<Vec2>,
    pub widths: Vec<f32>,
    pub raw_points: Vec<Vec2>,
    pub raw_widths: Vec<f32>,
}

/// Puts the pattern of a run on each run of sides round one hex, then cuts the corners (1/4 and 3/4).
// re: cvFloodplainsSystem::ExtrudeControlPoints 0x08663ce8
pub fn extrude_control_points(grid: &RiverGrid, sim: &BankSim, frame: &TerrainFrame) -> Outline {
    let mut out = Outline::default();
    let n_edges = sim.edges.len();
    let mut i = 0usize;
    while i < n_edges {
        let mut edge = sim.edges[i];
        // The hex on the left of the flow.
        if edge.code % 12 <= 5 {
            edge = across(edge);
        }
        let angle = SIDE_ROTATION[edge.side()] * SIXTH_TURN;
        let run = count_internal_edges(grid, sim, i);
        // Later runs share their first point with the previous run's last.
        let first = usize::from(i != 0);
        let hex = [edge.x as f32, edge.y as f32, (-(edge.x + edge.y)) as f32];
        let side_offset = match sim.side {
            // TRANSLATIONS[4] = E
            BankSide::Right => [1.0, 0.0, -1.0],
            BankSide::Left => [0.0; 3],
        };
        let pattern_run = run.clamp(1, 5);
        for point in first..run + 1 {
            if point > pattern_run {
                // More than five sides round one hex is out of the table of the original.
                continue;
            }
            let base = LEFT_PATTERN[pattern_run - 1][point];
            let p = match sim.side {
                BankSide::Left => base,
                BankSide::Right => mirror(base),
            };
            let shifted = [
                p[0] + side_offset[0],
                p[1] + side_offset[1],
                p[2] + side_offset[2],
            ];
            let r = rotate_cube(shifted, angle);
            let cube = Vec3::new(r[0] + hex[0], r[1] + hex[1], r[2] + hex[2]);
            out.raw_points.push(cube_to_tex(frame, cube));
            out.raw_widths.push(RUN_WIDTH[pattern_run - 1]);
        }
        i += run;
    }
    for k in 1..out.raw_points.len() {
        let (a, b) = (out.raw_points[k - 1], out.raw_points[k]);
        out.points
            .push(Vec2::new(b.x * 0.25 + a.x * 0.75, b.y * 0.25 + a.y * 0.75));
        out.points
            .push(Vec2::new(b.x * 0.75 + a.x * 0.25, b.y * 0.75 + a.y * 0.25));
        let (wa, wb) = (out.raw_widths[k - 1], out.raw_widths[k]);
        out.widths.push(wb * 0.25 + wa * 0.75);
        out.widths.push(wb * 0.75 + wa * 0.25);
    }
    out
}

/// A point extended past each end: the first by half the first step backwards, the last by half the
/// last step onwards.
// re: AddEndcaps<GameSpline::ControlPoint, FFastVector<FGXVector2,...>> 0x08664cc2
pub fn add_endcaps(points: &mut Vec<Vec2>) {
    if points.len() < 2 {
        return;
    }
    let n = points.len();
    let (p0, p1) = (points[0], points[1]);
    let (q1, q0) = (points[n - 2], points[n - 1]);
    let head = Vec2::new(
        (p0.x - p1.x) * ENDCAP_FRACTION + p0.x,
        (p0.y - p1.y) * ENDCAP_FRACTION + p0.y,
    );
    let tail = Vec2::new(
        (q0.x - q1.x) * ENDCAP_FRACTION + q0.x,
        (q0.y - q1.y) * ENDCAP_FRACTION + q0.y,
    );
    points.insert(0, head);
    points.push(tail);
}

/// [`add_endcaps`] for the widths.
// re: AddEndcaps<float, FFastVector<float,...>> 0x08664e23
pub fn add_endcaps_scalar(values: &mut Vec<f32>) {
    if values.len() < 2 {
        return;
    }
    let n = values.len();
    let head = (values[0] - values[1]) * ENDCAP_FRACTION + values[0];
    let tail = (values[n - 1] - values[n - 2]) * ENDCAP_FRACTION + values[n - 1];
    values.insert(0, head);
    values.push(tail);
}

/// The uniform quadratic B-spline of floats (the weights of the points).
// re: B_SPLINE::EvalScalingFunc<float, float*, 0, 0> 0x08665067
fn eval_scalar(values: &[f32], t: f32) -> f32 {
    let (i, f) = bspline::locate(values.len(), t);
    let f2 = f * f;
    let w = [0.5 * f2 - f + 0.5, -f2 + f + 0.5, 0.5 * f2];
    let at = |k: usize| values[(i + k).min(values.len() - 1)];
    at(1) * w[1] + at(0) * w[0] + at(2) * w[2]
}

/// The point `distance` from `from` toward `toward` (`D3DXVec2Normalize`: a zero vector stays zero).
fn extrude_toward(from: Vec2, toward: Vec2, distance: f32) -> Vec2 {
    let (dx, dy) = (toward.x - from.x, toward.y - from.y);
    let len = (dx * dx + dy * dy).sqrt();
    let (nx, ny) = if len == 0.0 {
        (0.0, 0.0)
    } else {
        (dx / len, dy / len)
    };
    Vec2::new(nx * distance + from.x, distance * ny + from.y)
}

fn dist(a: Vec2, b: Vec2) -> f32 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    (dy * dy + dx * dx).sqrt()
}

/// A vertex of the strip in the texel plane: place and `(u, v)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StripVertex {
    pub pos: Vec2,
    pub uv: Vec2,
}

/// The strip of a sim and where it is cut into start, middle and end.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Strip {
    pub verts: Vec<StripVertex>,
    pub indices: Vec<u16>,
    /// Index in `indices` where the start part ends, and where the end part starts.
    pub start_end: usize,
    pub tail_start: usize,
}

impl Strip {
    /// The index ranges of the start, the middle and the end.
    pub fn parts(&self) -> [(usize, usize); 3] {
        let n = self.indices.len();
        [
            (0, self.start_end),
            (self.start_end, self.tail_start),
            (self.tail_start, n),
        ]
    }
}

/// The strip of quads from the river to the bank: four vertices per sample (river, middle, bank,
/// quad centre), six triangles per quad. `river` and `bank` are the control points with their end
/// caps, `widths` the widths of the bank at them.
// re: cvFloodplainsSystem::BuildBankVis 0x0866217c (the two passes over the parameter)
pub fn build_strip(river: &[Vec2], bank: &[Vec2], widths: &[f32]) -> Strip {
    let n = river.len();
    debug_assert!(n >= 2 && bank.len() == n && widths.len() == n);
    let last_t = n as f32 - 2.0;
    let end_t = last_t + 0.001;

    // First pass: the mean length of the river side and the bank side, in u.
    let mut prev_river = bspline::value(river, 0.0);
    let mut prev_bank = bspline::value(bank, 0.0);
    let mut river_length = 0.0f32;
    let mut bank_length = 0.0f32;
    let mut t = 0.0f32;
    while end_t > t {
        let ct = if last_t < t { last_t } else { t };
        let r = bspline::value(river, ct);
        let toward = bspline::value(bank, ct);
        let w = eval_scalar(widths, ct);
        let b = extrude_toward(r, toward, w);
        river_length += dist(r, prev_river);
        bank_length += dist(b, prev_bank);
        prev_river = r;
        prev_bank = b;
        t = ct + SPLINE_STEP;
    }
    let end_u = (river_length * TEXEL_TO_U + bank_length * TEXEL_TO_U) * 0.5 - 0.1;

    // Second pass.
    let mut strip = Strip::default();
    prev_river = bspline::value(river, 0.0);
    prev_bank = bspline::value(bank, 0.0);
    let mut river_u = 0.0f32;
    let mut bank_u = 0.0f32;
    let mut prev_u = 0.0f32;
    let mut start_end = 0usize;
    let mut tail_start = 0usize;
    t = 0.0;
    while end_t > t {
        let ct = if last_t < t { last_t } else { t };
        let r = bspline::value(river, ct);
        let toward = bspline::value(bank, ct);
        let w = eval_scalar(widths, ct);
        let b = extrude_toward(r, toward, w);
        river_u += dist(r, prev_river) * TEXEL_TO_U;
        bank_u += TEXEL_TO_U * dist(b, prev_bank);
        let u = river_u * 0.5 + bank_u * 0.5;

        let first = strip.verts.len() as i32;
        strip.verts.push(StripVertex {
            pos: r,
            uv: Vec2::new(u, 0.98),
        });
        strip.verts.push(StripVertex {
            pos: Vec2::new(r.x * 0.5 + b.x * 0.5, r.y * 0.5 + b.y * 0.5),
            uv: Vec2::new(u, 0.49),
        });
        strip.verts.push(StripVertex {
            pos: b,
            uv: Vec2::new(u, 0.0),
        });
        strip.verts.push(StripVertex {
            pos: Vec2::new(
                ((prev_bank.x * 0.25 + b.x * 0.25) + r.x * 0.25) + prev_river.x * 0.25,
                ((prev_bank.y * 0.25 + b.y * 0.25) + r.y * 0.25) + prev_river.y * 0.25,
            ),
            uv: Vec2::new(prev_u * 0.5 + u * 0.5, 0.48),
        });

        let next_t = ct + SPLINE_STEP;
        // The first sample only opens the strip.
        if next_t >= 0.2501 {
            let count = strip.indices.len();
            if start_end == 0 && prev_u > 0.1 {
                start_end = count;
            }
            let prev_u8 = 8.0 * prev_u;
            if tail_start == 0 && prev_u > end_u && (prev_u8 - (prev_u8 as i32) as f32).abs() < 0.5
            {
                tail_start = count;
            }
            for o in STRIP_INDEX_OFFSETS {
                strip.indices.push((first + o) as u16);
            }
        }
        prev_river = r;
        prev_bank = b;
        prev_u = u;
        t = next_t;
    }
    let count = strip.indices.len();
    if tail_start == 0 {
        tail_start = count;
    }
    if start_end > tail_start {
        tail_start = start_end;
    }
    strip.start_end = start_end;
    strip.tail_start = tail_start;
    strip
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hexgrid::{Dir, cell_center};

    fn frame() -> TerrainFrame {
        TerrainFrame::new(24, 24, false)
    }

    #[test]
    fn rotating_east_by_the_side_rotation_gives_the_direction_of_the_side() {
        use super::super::hex::SIDE_DIR;
        let east = [1.0, 0.0, -1.0];
        for side in 0..6 {
            let r = rotate_cube(east, SIDE_ROTATION[side] * SIXTH_TURN);
            let (dx, dy) = SIDE_DIR[side].hex_delta();
            let want = [dx as f32, dy as f32, -(dx + dy) as f32];
            for k in 0..3 {
                assert!(
                    (r[k] - want[k]).abs() < 1e-3,
                    "side {side}: {r:?} vs {want:?}"
                );
            }
        }
        // A whole step of sixths takes NE to NW (counter-clockwise).
        let ne = [0.0, 1.0, -1.0];
        let nw = rotate_cube(ne, SIXTH_TURN);
        assert!((nw[0] + 1.0).abs() < 1e-3 && (nw[1] - 1.0).abs() < 1e-3 && nw[2].abs() < 1e-3);
        assert_eq!(Dir::East.hex_delta(), (1, 0));
    }

    #[test]
    fn the_cube_to_texel_map_reproduces_the_hex_centres_and_is_linear() {
        let f = frame();
        for (col, row) in [(0, 0), (3, 0), (3, 1), (5, 4), (7, 7), (2, 11), (0, 13)] {
            let want = f.world_to_tex(cell_center(col, row));
            let x = col - (row >> 1);
            let got = cube_to_tex(&f, Vec3::new(x as f32, row as f32, -(x + row) as f32));
            assert!(got.distance(want) < 1e-2, "{col},{row}: {got} {want}");
        }
        let a = cube_to_tex(&f, Vec3::new(1.0, 2.0, -3.0));
        let b = cube_to_tex(&f, Vec3::new(3.0, -2.0, -1.0));
        let mid = cube_to_tex(&f, Vec3::new(2.0, 0.0, -2.0));
        assert!(mid.distance((a + b) * 0.5) < 1e-2);
    }

    #[test]
    fn endcaps_extend_each_end_by_half_a_step() {
        let mut p = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(2.0, 4.0),
        ];
        add_endcaps(&mut p);
        assert_eq!(p.len(), 5);
        assert_eq!(p[0], Vec2::new(-1.0, 0.0));
        assert_eq!(p[4], Vec2::new(2.0, 6.0));
        let mut w = vec![10.0, 20.0, 40.0];
        add_endcaps_scalar(&mut w);
        assert_eq!(w, vec![5.0, 10.0, 20.0, 40.0, 50.0]);
    }

    #[test]
    fn a_strip_has_four_vertices_per_sample_and_eighteen_indices_per_quad() {
        // 8 control points -> t in 0..=6 in steps of a quarter: 25 samples.
        let river: Vec<Vec2> = (0..8).map(|i| Vec2::new(i as f32 * 20.0, 0.0)).collect();
        let bank: Vec<Vec2> = (0..8).map(|i| Vec2::new(i as f32 * 20.0, 30.0)).collect();
        let widths = vec![20.0; 8];
        let s = build_strip(&river, &bank, &widths);
        assert_eq!(s.verts.len(), 4 * 25);
        assert_eq!(s.indices.len(), 18 * 24);
        assert!(s.indices.iter().all(|&i| usize::from(i) < s.verts.len()));
        // The river vertices carry v = 0.98, the bank ones 0.
        assert!(
            s.verts
                .chunks(4)
                .all(|q| q[0].uv.y == 0.98 && q[2].uv.y == 0.0)
        );
        // The bank is 20 texels from the river toward the points of the bank.
        for q in s.verts.chunks(4) {
            assert!((q[2].pos.y - q[0].pos.y - 20.0).abs() < 1e-3);
        }
        // The three parts tile the indices, in order.
        let parts = s.parts();
        assert_eq!(parts[0].0, 0);
        assert_eq!(parts[0].1, parts[1].0);
        assert_eq!(parts[1].1, parts[2].0);
        assert_eq!(parts[2].1, s.indices.len());
        assert!(parts.iter().all(|(a, b)| a <= b && (b - a) % 3 == 0));
        // u grows along the strip.
        assert!(
            s.verts
                .chunks(4)
                .collect::<Vec<_>>()
                .windows(2)
                .all(|w| w[1][0].uv.x >= w[0][0].uv.x)
        );
    }

    #[test]
    fn the_first_quad_refers_back_to_the_first_sample() {
        let river: Vec<Vec2> = (0..4).map(|i| Vec2::new(i as f32 * 20.0, 0.0)).collect();
        let bank: Vec<Vec2> = (0..4).map(|i| Vec2::new(i as f32 * 20.0, 5.0)).collect();
        let s = build_strip(&river, &bank, &[10.0; 4]);
        // The first quad is made at the second sample (vertices 4..8) from -4..: 0..3 of the first.
        assert_eq!(&s.indices[..6], &[0, 7, 1, 1, 7, 2]);
    }
}
