//! The mesh of the river water: `RiverVisSystem::TessellateMesh`, a
//! ribbon of triangles between the two bank splines of every river, in the world of the original
//! (x east, y north, z up) so that `VSWaterRiver` runs with `World = identity`.
//!
//! re: `RiverVisSystem::TessellateMesh` 0x088d97be (rw-a6 "RW-12 (g)", rw-a6b-river, dump
//! `088d97be.txt`). Per river of at least four bank points:
//!
//! * both banks get the river's height of each control point as `z` and are evaluated as
//!   B-splines (`EvalScalingFunc<FGXVector3,1,0>`) at `p = end - t`, `end = n - 2`, for
//!   `t = 0, 0.1, ...` while `t <= end + 1e-4`: the mesh runs from the downstream end to the source;
//! * a sample is one pair of vertices, left bank then right bank; every pair after the first adds the
//!   two triangles `(b, b+2, b+3)`, `(b, b+3, b+1)` with the previous pair (`b` = count - 4);
//! * the point is `(x - offX, 0.866 y - offY, z / 255 * TerrainScale.z)`, lifted by 0.25 when
//!   `TerrainTessLevel` is 0, by 0.25 more where it is under 10.9, and by 0.5 at the downstream end
//!   (`t < 1.1`) of a river that does not end at a mouth;
//! * the vertex (`VStruct<10>`, 0x20 bytes) carries the flow `U` (the length along the middle of the
//!   banks in texels / 100, from 6.4 for a river without a mouth, from 0 at a mouth), the place
//!   across `V` (the texture has five columns, the body of the river takes four), the fade and the
//!   side (0 left, 1 right).

use bytemuck::{Pod, Zeroable};
use glam::{Vec2, Vec3};

use super::RiverSim;
use super::stencil::value;
use crate::world::terrain::sampler::{HEIGHT_TO_UNIT, TerrainFrame};

/// A river with fewer bank points makes no mesh.
// re: TessellateMesh 0x088d97be (`3 < count`)
const MIN_POINTS: usize = 4;
/// The step of the spline parameter.
// re: DAT_09149c7c = 0.1
const STEP: f32 = 0.1;
/// The loop runs while the parameter is at most the last segment plus this.
// re: DAT_0914b1b4 = 1e-4
const END_EPSILON: f32 = 1.0e-4;
/// The lift of the water when the terrain is not tessellated (`g_uTerrainTessLevel == 0`), and the
/// lift of a bank point under [`LOW_WATER`].
// re: DAT_09149c6c = 0.25
const LIFT: f32 = 0.25;
/// Under this height (world units) a bank point is lifted by [`LIFT`].
// re: DAT_09213ae4 = 10.9
const LOW_WATER: f32 = 10.9;
/// The lift of the downstream end of a river that does not end at a mouth, for `t <` [`TAIL`].
// re: DAT_0912c9e8 = 0.5, DAT_09213aec = 1.1
const TAIL_LIFT: f32 = 0.5;
const TAIL: f32 = 1.1;
/// The flow length a river starts with, in texels, by `ends_at_mouth` (no mouth, mouth).
// re: DAT_09213af0 = {640.0, 0.0}
const FLOW_START: [f32; 2] = [640.0, 0.0];
/// Texels per unit of the flow coordinate `U`.
// re: DAT_0920e8b8 = 100.0
const FLOW_UNIT: f32 = 100.0;
/// The width between the banks that spans the whole body of the texture (`w = D / 20 * 0.5`).
// re: DAT_0920cb2c = 20.0
const WIDTH_UNIT: f32 = 20.0;
/// The fade grows by the flow length over this many texels, up to 0.5.
// re: DAT_09213ae8 = 122.0
const FADE_LENGTH: f32 = 122.0;

/// The vertex of `VSWaterRiver` (`VStruct<10>`): `v0` the point, `v1` (flow `U`, across `V`),
/// `v2` (fade, side); the last four bytes (`COLOR` in the DX11 layout) are not read.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct RiverVertex {
    pub position: [f32; 3],
    pub flow: [f32; 2],
    pub fade_side: [f32; 2],
    pub unused: u32,
}

/// The triangles of every river.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RiverMesh {
    pub vertices: Vec<RiverVertex>,
    /// Triangle list.
    pub indices: Vec<u32>,
}

impl RiverMesh {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }
}

/// The raw height of every control point of river `i`; a missing height repeats the one before
/// (a river never rises downstream), the first one the height of the sea.
fn control_heights(sim: &RiverSim, i: usize, n: usize) -> Vec<f32> {
    let given = sim.heights.get(i).map(Vec::as_slice).unwrap_or(&[]);
    let mut last = f32::from(crate::world::terrain::sampler::SEA_LEVEL_RAW);
    (0..n)
        .map(|k| {
            if let Some(&h) = given.get(k) {
                last = h;
            }
            last
        })
        .collect()
}

/// A bank sample in the world of the original, lifted as `TessellateMesh` does before the width.
fn bank_point(frame: &TerrainFrame, p: Vec3, tess_level: u32) -> Vec3 {
    let mut z = p.z * HEIGHT_TO_UNIT * frame.terrain_scale().z;
    if tess_level == 0 {
        z += LIFT;
    }
    if z < LOW_WATER {
        z += LIFT;
    }
    frame.orig_point(Vec2::new(p.x, p.y), z)
}

/// The ribbon of every river (`TessellateMesh`); `tess_level` is `TerrainTessLevel`.
// re: RiverVisSystem::TessellateMesh 0x088d97be
pub fn build(sim: &RiverSim, frame: &TerrainFrame, tess_level: u32) -> RiverMesh {
    let mut mesh = RiverMesh::default();
    for i in 0..sim.left_banks.len() {
        build_river(sim, i, frame, tess_level, &mut mesh);
    }
    mesh
}

/// The ribbon made a river at a time, in steps that each stop at a time budget: the same mesh as
/// [`build`].
pub struct RiverBuild {
    sim: std::sync::Arc<RiverSim>,
    frame: TerrainFrame,
    tess_level: u32,
    next: usize,
    mesh: RiverMesh,
}

impl RiverBuild {
    pub fn new(sim: std::sync::Arc<RiverSim>, frame: &TerrainFrame, tess_level: u32) -> Self {
        Self {
            sim,
            frame: *frame,
            tess_level,
            next: 0,
            mesh: RiverMesh::default(),
        }
    }

    /// Makes rivers until `budget_ms` of the recorder's clock has passed (at least one a call);
    /// the mesh when it is whole.
    pub fn step(&mut self, budget_ms: f64) -> Option<RiverMesh> {
        let start = crate::perf::slow::now_ms();
        while self.next < self.sim.left_banks.len() {
            build_river(
                &self.sim,
                self.next,
                &self.frame,
                self.tess_level,
                &mut self.mesh,
            );
            self.next += 1;
            if crate::perf::slow::now_ms() - start >= budget_ms {
                break;
            }
        }
        (self.next >= self.sim.left_banks.len()).then(|| std::mem::take(&mut self.mesh))
    }

    pub fn sim(&self) -> &std::sync::Arc<RiverSim> {
        &self.sim
    }

    pub fn frame(&self) -> &TerrainFrame {
        &self.frame
    }

    pub fn tess_level(&self) -> u32 {
        self.tess_level
    }
}

/// The ribbon of river `i` appended to `mesh`.
fn build_river(
    sim: &RiverSim,
    i: usize,
    frame: &TerrainFrame,
    tess_level: u32,
    mesh: &mut RiverMesh,
) {
    {
        let Some(left) = sim.left_banks.get(i) else {
            return;
        };
        let n = left.points.len();
        let (Some(right), Some(river)) = (sim.right_banks.get(i), sim.rivers.get(i)) else {
            return;
        };
        if n < MIN_POINTS || right.points.len() < n {
            return;
        }
        let heights = control_heights(sim, i, n);
        let with_height = |points: &[Vec3]| -> Vec<Vec3> {
            points[..n]
                .iter()
                .zip(&heights)
                .map(|(p, &h)| Vec3::new(p.x, p.y, h))
                .collect()
        };
        let (l, r) = (with_height(&left.points), with_height(&right.points));
        let end = (n - 2) as f32;
        let limit = end + END_EPSILON;
        let mouth = river.ends_at_mouth;
        let mut flow = FLOW_START[usize::from(mouth)];
        let mid = |a: Vec3, b: Vec3| (a.truncate() + b.truncate()) * 0.5;
        let mut previous = mid(value(&l, end), value(&r, end));
        let mut t = 0.0f32;
        while t <= limit {
            let p = (end - t).clamp(0.0, end);
            let (sl, sr) = (value(&l, p), value(&r, p));
            let mut wl = bank_point(frame, sl, tess_level);
            let mut wr = bank_point(frame, sr, tess_level);
            // The loop variable itself is clamped to the parameter range (`fVar31`).
            t = t.clamp(0.0, end);
            let m = mid(sl, sr);
            flow += (m - previous).length();
            previous = m;
            let w = (wl - wr).length() / WIDTH_UNIT * 0.5;
            let across_left = (0.5 - w) * 4.0 / 5.0;
            let across_right = ((0.5 + w) * 4.0 + 1.0) / 5.0;
            let u = flow / FLOW_UNIT;
            let to_source = limit - t;
            let fade = if to_source < 1.0 {
                1.0 - 0.5 * to_source
            } else {
                let f = flow / FADE_LENGTH;
                if to_source > 1.0 && f > 0.5 { 0.5 } else { f }
            };
            if !mouth && t < TAIL {
                wl.z += TAIL_LIFT;
                wr.z += TAIL_LIFT;
            }
            for (pos, across, side) in [(wl, across_left, 0.0), (wr, across_right, 1.0)] {
                mesh.vertices.push(RiverVertex {
                    position: pos.to_array(),
                    flow: [u, across],
                    fade_side: [fade, side],
                    unused: 0,
                });
            }
            if t >= STEP {
                let c = mesh.vertices.len() as u32;
                mesh.indices
                    .extend_from_slice(&[c - 4, c - 2, c - 1, c - 4, c - 1, c - 3]);
            }
            t += STEP;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::RiverSpline;
    use super::*;

    /// A river along +u of `n` control points 4 texels apart from u = 20 at v = 40, the banks 6
    /// texels to each side.
    fn straight(n: usize, mouth: bool) -> RiverSim {
        let line = |dv: f32| RiverSpline {
            points: (0..n)
                .map(|i| Vec3::new(20.0 + 4.0 * i as f32, 40.0 + dv, 0.0))
                .collect(),
            ends_at_mouth: mouth,
        };
        RiverSim {
            rivers: vec![line(0.0)],
            left_banks: vec![line(6.0)],
            right_banks: vec![line(-6.0)],
            heights: vec![(0..n).map(|i| 14000.0 - 10.0 * i as f32).collect()],
            ..RiverSim::default()
        }
    }

    fn frame() -> TerrainFrame {
        TerrainFrame::new(4, 4, false)
    }

    fn h(f: &TerrainFrame, raw: f32) -> f32 {
        raw * HEIGHT_TO_UNIT * f.terrain_scale().z
    }

    #[test]
    fn a_pair_per_step_of_a_tenth_and_two_triangles_per_pair_after_the_first() {
        // 8 points: end = 6, t = 0 .. 6 by 0.1: 61 pairs, 60 quads.
        let m = build(&straight(8, false), &frame(), 2);
        assert_eq!(m.vertices.len(), 61 * 2);
        assert_eq!(m.triangles(), 60 * 2);
        assert_eq!(&m.indices[..6], &[0, 2, 3, 0, 3, 1]);
        assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
    }

    #[test]
    fn the_mesh_runs_from_the_mouth_up_and_the_sides_alternate() {
        let f = frame();
        let m = build(&straight(8, true), &f, 2);
        // Pair 0 is the downstream end: the end of the last segment (the middle of the last two
        // points), on the left bank (+6 texels).
        let first = Vec3::from(m.vertices[0].position);
        let want = f.orig_point(Vec2::new(20.0 + 4.0 * 6.5, 46.0), 0.0);
        assert!((first.x - want.x).abs() < 1e-3 && (first.y - want.y).abs() < 1e-3);
        // The last pair is the source: the first control point.
        let last = Vec3::from(m.vertices[m.vertices.len() - 2].position);
        let src = f.orig_point(Vec2::new(20.0, 46.0), 0.0);
        assert!((last.x - src.x).abs() < 1e-3 && (last.y - src.y).abs() < 1e-3);
        for pair in m.vertices.chunks(2) {
            assert_eq!(pair[0].fade_side[1], 0.0);
            assert_eq!(pair[1].fade_side[1], 1.0);
            assert_eq!(pair[0].flow[0], pair[1].flow[0]);
            assert_eq!(pair[0].fade_side[0], pair[1].fade_side[0]);
        }
    }

    #[test]
    fn the_flow_starts_at_zero_at_a_mouth_and_at_six_point_four_otherwise_and_grows_upstream() {
        let mouth = build(&straight(8, true), &frame(), 2);
        let open = build(&straight(8, false), &frame(), 2);
        assert_eq!(mouth.vertices[0].flow[0], 0.0);
        assert!((open.vertices[0].flow[0] - 6.4).abs() < 1e-5);
        let u: Vec<f32> = mouth
            .vertices
            .iter()
            .step_by(2)
            .map(|v| v.flow[0])
            .collect();
        assert!(u.windows(2).all(|w| w[1] > w[0]));
        // A step of 0.1 of a 4-texel segment is 0.4 texels: U grows by 0.004 inside the river.
        assert!((u[30] - u[29] - 0.004).abs() < 1e-4, "{}", u[30] - u[29]);
    }

    #[test]
    fn across_takes_the_middle_four_fifths_by_the_width() {
        let m = build(&straight(8, false), &frame(), 2);
        let v = &m.vertices[40..42];
        let d = (Vec3::from(v[0].position) - Vec3::from(v[1].position)).length();
        let w = d / 40.0;
        assert!((v[0].flow[1] - (0.5 - w) * 0.8).abs() < 1e-5);
        assert!((v[1].flow[1] - ((0.5 + w) * 4.0 + 1.0) / 5.0).abs() < 1e-5);
        assert!(v[0].flow[1] < 0.5 && v[1].flow[1] > 0.5);
    }

    #[test]
    fn the_fade_is_one_at_the_source_and_grows_from_the_mouth() {
        let mouth = build(&straight(8, true), &frame(), 2);
        let fades: Vec<f32> = mouth
            .vertices
            .iter()
            .step_by(2)
            .map(|v| v.fade_side[0])
            .collect();
        assert_eq!(fades[0], 0.0);
        assert!((fades.last().unwrap() - 1.0).abs() < 1e-3);
        // In the middle: the flow length / 122, at most 0.5.
        let u = mouth.vertices[20].flow[0];
        assert!((fades[10] - (u * 100.0 / 122.0).min(0.5)).abs() < 1e-5);
        let open = build(&straight(8, false), &frame(), 2);
        assert_eq!(open.vertices[0].fade_side[0], 0.5);
    }

    #[test]
    fn the_height_is_lifted_by_the_rules_of_the_original() {
        let f = frame();
        let sim = straight(8, true);
        // At the mouth the height is the middle of the last two (13935 raw: 10.93, over 10.9).
        let base = h(&f, 13935.0);
        assert!(base > LOW_WATER);
        let z = build(&sim, &f, 2).vertices[0].position[2];
        assert!((z - base).abs() < 1e-3, "{z} {base}");
        // TerrainTessLevel 0 lifts by a quarter.
        let z0 = build(&sim, &f, 0).vertices[0].position[2];
        assert!((z0 - base - LIFT).abs() < 1e-3);
        // Low water is lifted by a quarter.
        let mut low = sim.clone();
        low.heights = vec![vec![1000.0; 8]];
        let z = build(&low, &f, 2).vertices[0].position[2];
        assert!((z - (h(&f, 1000.0) + LIFT)).abs() < 1e-4);
        // The downstream end of a river without a mouth is lifted by a half while t < 1.1.
        low.rivers[0].ends_at_mouth = false;
        let m = build(&low, &f, 2);
        let z_at = |pair: usize| m.vertices[2 * pair].position[2];
        assert!((z_at(0) - (h(&f, 1000.0) + LIFT + TAIL_LIFT)).abs() < 1e-4);
        assert!((z_at(10) - (h(&f, 1000.0) + LIFT + TAIL_LIFT)).abs() < 1e-4);
        assert!((z_at(12) - (h(&f, 1000.0) + LIFT)).abs() < 1e-4);
    }

    #[test]
    fn short_rivers_and_missing_banks_emit_nothing() {
        assert!(build(&straight(3, false), &frame(), 2).is_empty());
        assert!(!build(&straight(4, false), &frame(), 2).is_empty());
        assert!(build(&RiverSim::default(), &frame(), 2).vertices.is_empty());
        let mut sim = straight(6, false);
        sim.heights.clear();
        assert!(!build(&sim, &frame(), 2).is_empty());
        sim.right_banks.clear();
        assert!(build(&sim, &frame(), 2).is_empty());
    }

    #[test]
    fn the_vertex_is_the_32_bytes_of_vstruct_10() {
        assert_eq!(std::mem::size_of::<RiverVertex>(), 0x20);
    }
}
