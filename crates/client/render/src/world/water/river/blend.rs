//! The bed of the rivers cut into the field: `cvRiverSimSystem::BlendIntoTerrain`
//! with the meshes of `CreateSimMeshes`. Every river makes a bed (the quads between the two bank
//! splines, one per step of 0.1 of the spline parameter, and a cap of 15 texels past the mouth) and
//! a skirt (the bed again with weight 1, a band of 15 texels outside each bank, and a cap of 30
//! texels before the source and past the mouth). The skirt pass pulls the field toward the mesh
//! height and retextures the low texels; the bed pass then cuts the bed lower and writes the bed
//! terrain.
//!
//! The original rasterizes over the whole map. A texel only depends on the triangles that touch it,
//! in list order, so a cell of the field can be written alone ([`RiverBlend::raster`]) with the
//! result of the whole map at once. The mask of the skirt pass ("the first skirt triangle that
//! reaches a texel wins") is local to the cell for the same reason.

// The constants and the comparisons are those of the original (NaN included), kept as they are.
#![allow(
    clippy::excessive_precision,
    clippy::approx_constant,
    clippy::neg_cmp_op_on_partial_ord,
    clippy::manual_clamp
)]

use std::collections::HashMap;

use glam::Vec3;

use super::RiverSim;
use super::stencil::{gradient, value};
use crate::world::terrain::build::FieldWindow;
use crate::world::terrain::field::{CELL_SIZE, CellRect};
use crate::world::terrain::library::TerrainDefs;
use crate::world::water::coast::blend::Texels;

/// The original has no terrain here: the pass does nothing for the id.
// re: BlendIntoTerrain 0x087f5508 (`ucFrom = ucTo = ucBedTerrain = 0xff` when no row has the code)
pub const NO_TERRAIN: u8 = 0xff;

/// A raw height is this many steps for a normalised one.
// re: cvRiverSimSystem.cpp `kHeightScale`
const HEIGHT_SCALE: f32 = 65535.0;
/// Normalised height of the floor of the bed, and the offset from it.
// re: cvRiverSimSystem.cpp `kBedFloor`, `kBedFloorOffset`
pub const BED_FLOOR: f32 = 0.23533989;
pub const BED_FLOOR_OFFSET: f32 = -0.06;
/// The bed dip: the across factor, the half pi and pi of the sine, the depth.
// re: Fun_087fa59c 0x087fa59c (`kBedAcrossScale`, `kBedHalfPi`, `kBedPi`, `kBedDipDepth`)
const BED_ACROSS_SCALE: f32 = 3.0;
const BED_HALF_PI: f32 = 1.57075;
const BED_PI: f32 = 3.1415;
const BED_DIP_DEPTH: f32 = -0.05;
/// A skirt texel at or below this height after the blend is retextured.
// re: Fun_087fa960 0x087fa960 (`kSkirtRetextureHeight`)
const SKIRT_RETEXTURE_HEIGHT: u32 = 35_000;
/// Triangles under this area (twice, signed) are dropped.
// re: Fun_087f8220 0x087f8220, Fun_087f875c 0x087f875c (`kMinTriangleArea`)
const MIN_TRIANGLE_AREA: f64 = 1.0e-5;

/// A river with fewer control points makes no mesh.
// re: CreateSimMeshes 0x087f5a70 (`3 < uVar21`)
const MIN_POINTS: usize = 4;
/// The step of the spline parameter, and the slack of the end of the loop.
// re: CreateSimMeshes 0x087f5a70 (DAT_09149c7c = 0.1, DAT_0914b1b4 = 1e-4)
const STEP: f32 = 0.1;
const END_EPSILON: f32 = 1.0e-4;
/// How far the skirt goes out of the bank and the bed cap past the mouth.
// re: CreateSimMeshes 0x087f5a70 (DAT_0920c6f4 = 15.0)
const SKIRT_WIDTH: f32 = 15.0;
/// How far the skirt caps go along the river past the first and the last point.
// re: CreateSimMeshes 0x087f5a70 (DAT_0914df80 = 30.0)
const CAP_LENGTH: f32 = 30.0;

/// Terrain codes the original finds by hex code: the continent (1..=4) of the skirt, the
/// terrain the skirt takes from and gives, and the bed.
// re: BlendIntoTerrain 0x087f5508 (`kSkirtFromPostfix = 0x35`, `kSkirtToPostfix = 0x10`,
// `kBedPostfix = 0x30`, `kBedContinent = 4`, `kNumBlendContinents = 4`)
const SKIRT_FROM_ID: u32 = 0x35;
const SKIRT_TO_ID: u32 = 0x10;
const BED_ID: u32 = 0x30;
const BED_CONTINENT: u8 = 4;

/// The indices of `TerrainDefs` the passes write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RiverIds {
    /// Per continent 1..=4: the terrain a skirt texel must have to be retextured.
    pub skirt_from: [u8; 4],
    /// Per continent 1..=4: the terrain it becomes.
    pub skirt_to: [u8; 4],
    /// The terrain of the bed.
    pub bed: u8,
}

impl RiverIds {
    /// No terrain found: the skirt retextures nothing and the bed keeps the ids.
    pub const NONE: Self = Self {
        skirt_from: [NO_TERRAIN; 4],
        skirt_to: [NO_TERRAIN; 4],
        bed: NO_TERRAIN,
    };

    /// The rows of the defs that have the hex codes of the original. A continent without the
    /// target row has no skirt remap.
    // re: BlendIntoTerrain 0x087f5508 (the loop over `g_aContinentPrefix[c + 1]`, the bed search)
    pub fn from_defs(defs: &TerrainDefs) -> Self {
        let find = |continent: u8, id: u32| {
            defs.rows()
                .iter()
                .position(|r| r.continent == continent && r.id == id)
                .map(|i| i as u8)
        };
        let mut ids = Self::NONE;
        for c in 0..4 {
            let continent = c as u8 + 1;
            let from = find(continent, SKIRT_FROM_ID).unwrap_or(NO_TERRAIN);
            // Without a target the source is disabled too.
            if let Some(to) = find(continent, SKIRT_TO_ID).filter(|&to| to != NO_TERRAIN) {
                ids.skirt_from[c] = from;
                ids.skirt_to[c] = to;
            }
        }
        ids.bed = find(BED_CONTINENT, BED_ID).unwrap_or(NO_TERRAIN);
        ids
    }
}

/// A vertex of the meshes.
// re: cvRiverSimSystem.h `RiverMeshVertex`
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshVertex {
    /// `x`, `y` in texels, `z` a raw height.
    pub pos: Vec3,
    /// Bed: 1 on the centre line, 0 on the banks (the skirt does not set it).
    pub across: f32,
    /// Blend weight toward the mesh height: 1 inside the banks, 0 at the outer edge of the skirt.
    pub weight: f32,
}

fn vtx(pos: Vec3, across: f32, weight: f32) -> MeshVertex {
    MeshVertex {
        pos,
        across,
        weight,
    }
}

/// The triangle lists (three vertices per triangle) of all rivers.
// re: cvRiverSimSystem.h `RiverSimSplineMesh`
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RiverMesh {
    pub bed: Vec<MeshVertex>,
    pub skirts: Vec<MeshVertex>,
}

/// up x gradient (up is +z), normalized: the left-hand side of the flow.
fn side_of(g: Vec3) -> Vec3 {
    Vec3::new(-g.y, g.x, 0.0).normalize_or_zero()
}

/// Gradient of the centre line at `t`, normalized (D3DXVec3Normalize: zero stays zero).
fn flow_at(center: &[Vec3], t: f32) -> Vec3 {
    gradient(center, t).normalize_or_zero()
}

/// The same with the height gradient cleared first.
fn flow_flat_at(center: &[Vec3], t: f32) -> Vec3 {
    let mut g = gradient(center, t);
    g.z = 0.0;
    g.normalize_or_zero()
}

/// The meshes of every river.
// re: cvRiverSimSystem::CreateSimMeshes 0x087f5a70 (no C++ in the dump; from the decompilation).
// The three splines of a river (centre, `m_aLeftBanks`, `m_aRightBanks`) are walked together in
// steps of 0.1; every step is a quad of the bed (4 triangles from the left bank over the centre to
// the right bank) and a quad of the skirt (6 triangles: the outer band of the left bank, the bed at
// weight 1, the outer band of the right bank). After the loop: the cap of the bed past the mouth
// (15 texels along the flow) and the skirt caps before the first point and past the last
// parameter reached by the loop (30 texels along the flow, 15 out of the banks).
pub fn create_sim_meshes(sim: &RiverSim) -> RiverMesh {
    let mut mesh = RiverMesh::default();
    for (river, spline) in sim.rivers.iter().enumerate() {
        let n = spline.points.len();
        if n < MIN_POINTS {
            continue;
        }
        let (Some(left), Some(right)) = (sim.left_banks.get(river), sim.right_banks.get(river))
        else {
            continue;
        };
        if left.points.len() < n || right.points.len() < n {
            continue;
        }
        let heights = sim.heights.get(river);
        let lift = |pts: &[Vec3]| -> Vec<Vec3> {
            (0..n)
                .map(|i| {
                    let z = heights.and_then(|h| h.get(i)).copied().unwrap_or(0.0);
                    Vec3::new(pts[i].x, pts[i].y, z)
                })
                .collect()
        };
        let (c, l, r) = (
            lift(&spline.points),
            lift(&left.points),
            lift(&right.points),
        );

        let last = (n - 2) as f32;
        let end_t = last + END_EPSILON;
        let mut t_next = STEP;
        let mut t_last = 0.0f32;
        while t_next <= end_t {
            let t = if t_next <= last { t_next } else { last };
            t_last = t;
            let tp = -STEP + t;
            let (ct, lt, rt) = (value(&c, t), value(&l, t), value(&r, t));
            let (cp, lp, rp) = (value(&c, tp), value(&l, tp), value(&r, tp));
            // The side vectors of the two ends of the step, 15 texels long.
            let side_t = side_of(flow_at(&c, t)) * SKIRT_WIDTH;
            let side_p = side_of(flow_at(&c, tp)) * SKIRT_WIDTH;

            // Bed: (A,D,E) (A,E,B) (B,E,F) (B,F,C) with A/D/E on the left, B/E/F on the right.
            let a = vtx(lp, 0.0, 1.0);
            let d = vtx(lt, 0.0, 1.0);
            let e = vtx(ct, 1.0, 1.0);
            let b = vtx(cp, 1.0, 1.0);
            let f = vtx(rt, 0.0, 1.0);
            let cc = vtx(rp, 0.0, 1.0);
            mesh.bed
                .extend_from_slice(&[a, d, e, a, e, b, b, e, f, b, f, cc]);

            // Skirt of the step.
            let out_lp = vtx(lp + side_p, 0.0, 0.0);
            let out_lt = vtx(lt + side_t, 0.0, 0.0);
            let in_lt = vtx(lt, 0.0, 1.0);
            let in_lp = vtx(lp, 0.0, 1.0);
            let in_rt = vtx(rt, 0.0, 1.0);
            let in_rp = vtx(rp, 0.0, 1.0);
            let out_rt = vtx(rt - side_t, 0.0, 0.0);
            let out_rp = vtx(rp - side_p, 0.0, 0.0);
            mesh.skirts.extend_from_slice(&[
                out_lp, out_lt, in_lt, //
                out_lp, in_lt, in_lp, //
                in_lp, in_lt, in_rt, //
                in_lp, in_rt, in_rp, //
                in_rp, in_rt, out_rt, //
                in_rp, out_rt, out_rp,
            ]);

            t_next = STEP + t;
        }

        // The bed cap past the mouth, at the end of the spline domain.
        let (ce, le, re) = (value(&c, last), value(&l, last), value(&r, last));
        let flow = flow_flat_at(&c, last);
        let ahead = flow * SKIRT_WIDTH;
        let l0 = vtx(le, 0.0, 1.0);
        let c0 = vtx(ce, 1.0, 1.0);
        let r0 = vtx(re, 0.0, 1.0);
        let l1 = vtx(le + ahead, 0.0, 0.0);
        let c1 = vtx(ce + ahead, 1.0, 0.0);
        let r1 = vtx(re + ahead, 0.0, 0.0);
        mesh.bed
            .extend_from_slice(&[l0, l1, c1, l0, c1, c0, c0, c1, r1, c0, r1, r0]);

        // The skirt caps: before the first point (backward) and at the last parameter (forward).
        for (at, forward) in [(0.0f32, false), (t_last, true)] {
            let (lb, rb) = (value(&l, at), value(&r, at));
            let flow = flow_flat_at(&c, at);
            let side = side_of(flow) * SKIRT_WIDTH;
            let along = flow * CAP_LENGTH * if forward { 1.0 } else { -1.0 };
            let p1 = vtx(lb + side, 0.0, 0.0);
            let p2 = vtx(lb + side + along, 0.0, 0.0);
            let p3 = vtx(lb + along, 0.0, 0.0);
            let p4 = vtx(lb, 0.0, 1.0);
            let p5 = vtx(rb + along, 0.0, 0.0);
            let p6 = vtx(rb - side + along, 0.0, 0.0);
            let p7 = vtx(rb, 0.0, 1.0);
            let p8 = vtx(rb - side, 0.0, 0.0);
            mesh.skirts.extend_from_slice(&[
                p1, p2, p3, //
                p1, p3, p4, //
                p4, p3, p5, //
                p4, p5, p7, //
                p7, p5, p6, //
                p7, p6, p8,
            ]);
        }
    }
    mesh
}

/// A triangle prepared for the rasterizer.
// re: Fun_087f8220 0x087f8220 / Fun_087f875c 0x087f875c (the setup is the same in both)
struct Plan {
    v: [MeshVertex; 3],
    /// The corners relative to the first covered texel.
    x: [f32; 3],
    y: [f32; 3],
    first_x: i32,
    end_x: i32,
    first_y: i32,
    end_y: i32,
    inv_area: f32,
    c: [f32; 3],
}

impl Plan {
    fn new(v: [MeshVertex; 3]) -> Option<Self> {
        let p = v.map(|q| q.pos);
        // `fMinX12 = k2 <= k1 ? k2 : k1; min = fMinX12 <= k0 ? fMinX12 : k0`
        let min_x = p[2].x.min(p[1].x).min(p[0].x);
        let min_y = p[2].y.min(p[1].y).min(p[0].y);
        let origin_x = (min_x + 0.5).floor();
        let origin_y = (min_y + 0.5).floor();
        let max_x = p[1].x.max(p[2].x).max(p[0].x);
        let max_y = p[1].y.max(p[2].y).max(p[0].y);
        let x = p.map(|q| q.x - origin_x);
        let y = p.map(|q| q.y - origin_y);
        let c0 = x[1] * y[2] - y[1] * x[2];
        let area = y[0] * (x[2] - x[1]) + x[0] * (y[1] - y[2]) + c0;
        if !(f64::from(area.abs()) >= MIN_TRIANGLE_AREA) {
            return None;
        }
        let inv_area = 1.0 / area;
        Some(Self {
            v,
            x,
            y,
            first_x: origin_x as i32,
            end_x: max_x as i32 + 1,
            first_y: origin_y as i32,
            end_y: max_y as i32 + 1,
            inv_area,
            c: [
                c0 * inv_area,
                (x[2] * y[0] - x[0] * y[2]) * inv_area,
                (x[0] * y[1] - y[0] * x[1]) * inv_area,
            ],
        })
    }

    fn rect(&self) -> CellRect {
        CellRect::new(self.first_x, self.first_y, self.end_x, self.end_y)
    }

    /// The covered texels of the cell, in the order of the original, with their barycentric
    /// weights. The accumulated steps are those of the original, so the weights are the same.
    fn walk(&self, cell: CellRect, map_w: i32, map_h: i32, mut f: impl FnMut(i32, i32, [f32; 3])) {
        let (x, y) = (&self.x, &self.y);
        let mut fv = 0.0f32;
        for iy in self.first_y..self.end_y {
            let cur_v = fv;
            fv += self.inv_area;
            if iy < 0 || iy >= map_h || iy < cell.v0 || iy >= cell.v1 {
                continue;
            }
            let row0 = (x[2] - x[1]) * cur_v;
            let row1 = (x[0] - x[2]) * cur_v;
            let row2 = (x[1] - x[0]) * cur_v;
            let mut fu = 0.0f32;
            for ix in self.first_x..self.end_x {
                let cur_u = fu;
                fu += self.inv_area;
                // re: `(iWidth * 100 + iX) % iWidth`: the map always wraps along X.
                let wx = ix.rem_euclid(map_w);
                if wx < cell.u0 || wx >= cell.u1 {
                    continue;
                }
                let w0 = (y[1] - y[2]) * cur_u + row0 + self.c[0];
                if !(w0 >= 0.0) {
                    continue;
                }
                let w1 = (y[2] - y[0]) * cur_u + row1 + self.c[1];
                if !(w1 >= 0.0) {
                    continue;
                }
                let w2 = (y[0] - y[1]) * cur_u + row2 + self.c[2];
                if !(w2 >= 0.0) {
                    continue;
                }
                f(wx, iy, [w0, w1, w2]);
            }
        }
    }
}

/// The triangles of one pass, with the cells they touch.
struct Pass {
    plans: Vec<Plan>,
    buckets: HashMap<(i32, i32), Vec<u32>>,
}

impl Pass {
    fn new(vertices: &[MeshVertex], cells: (i32, i32)) -> Self {
        let plans: Vec<Plan> = vertices
            .chunks_exact(3)
            .filter_map(|t| Plan::new([t[0], t[1], t[2]]))
            .collect();
        let mut buckets: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (n, plan) in plans.iter().enumerate() {
            for key in plan.rect().cells(cells.0, cells.1, true) {
                buckets.entry(key).or_default().push(n as u32);
            }
        }
        Self { plans, buckets }
    }

    /// The triangles that touch the rectangle, in the order of the list.
    fn touching(&self, cell: CellRect, cells: (i32, i32)) -> Vec<u32> {
        let mut out: Vec<u32> = Vec::new();
        for key in cell.cells(cells.0, cells.1, true) {
            if let Some(list) = self.buckets.get(&key) {
                out.extend_from_slice(list);
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// The meshes of the rivers, ready to be blended into the field.
pub struct RiverBlend {
    ids: RiverIds,
    mesh: RiverMesh,
    map: (i32, i32),
    cells: (i32, i32),
    skirt: Pass,
    bed: Pass,
}

impl RiverBlend {
    /// `CreateSimMeshes` for the rivers of `sim`. The size of the map comes with
    /// [`with_map_size`](Self::with_map_size): until then nothing is rasterized.
    // re: cvRiverSimSystem::BlendIntoTerrain 0x087f5508 (`CalculateRiverHeights`, `CreateSimMeshes`)
    pub fn new(sim: &RiverSim, ids: &RiverIds) -> Self {
        let mut blend = Self {
            ids: *ids,
            mesh: create_sim_meshes(sim),
            map: (0, 0),
            cells: (0, 0),
            skirt: Pass {
                plans: Vec::new(),
                buckets: HashMap::new(),
            },
            bed: Pass {
                plans: Vec::new(),
                buckets: HashMap::new(),
            },
        };
        blend.plan();
        blend
    }

    /// The size of the field in texels (`m_iHeightMapWidth`, `m_iHeightMapHeight`); the width is
    /// where the texels wrap.
    pub fn with_map_size(mut self, width: u32, height: u32) -> Self {
        self.map = (width as i32, height as i32);
        self.cells = (
            width as i32 / CELL_SIZE as i32,
            height as i32 / CELL_SIZE as i32,
        );
        self.plan();
        self
    }

    fn plan(&mut self) {
        self.skirt = Pass::new(&self.mesh.skirts, self.cells);
        self.bed = Pass::new(&self.mesh.bed, self.cells);
    }

    pub fn mesh(&self) -> &RiverMesh {
        &self.mesh
    }

    /// The skirt pass, then the bed pass, on the texels of the cell.
    // re: cvRiverSimSystem::BlendIntoTerrain 0x087f5508 (the skirts first, the bed on top)
    pub fn raster(&self, cell: CellRect, window: &mut FieldWindow<'_>) {
        self.raster_into(cell, window);
    }

    /// The same for any field (a window or a whole field).
    pub fn raster_into(&self, cell: CellRect, t: &mut impl Texels) {
        let (map_w, map_h) = self.map;
        if map_w <= 0 || map_h <= 0 {
            return;
        }
        // Texels outside the map are not part of any cell.
        let cell = CellRect::new(
            cell.u0.max(0),
            cell.v0.max(0),
            cell.u1.min(map_w),
            cell.v1.min(map_h),
        );
        if cell.is_empty() {
            return;
        }
        // The mask of the skirt pass: the texels a skirt triangle already wrote.
        let mut mask = vec![false; (cell.width() * cell.height()) as usize];
        for n in self.skirt.touching(cell, self.cells) {
            let plan = &self.skirt.plans[n as usize];
            plan.walk(cell, map_w, map_h, |u, v, bary| {
                let slot = ((v - cell.v0) * cell.width() + (u - cell.u0)) as usize;
                self.skirt_texel(t, &mut mask[slot], u, v, bary, &plan.v);
            });
        }
        for n in self.bed.touching(cell, self.cells) {
            let plan = &self.bed.plans[n as usize];
            plan.walk(cell, map_w, map_h, |u, v, bary| {
                self.bed_texel(t, u, v, bary, &plan.v);
            });
        }
    }

    /// One texel of a skirt triangle.
    // re: Fun_087fa960 0x087fa960
    fn skirt_texel(
        &self,
        t: &mut impl Texels,
        written: &mut bool,
        u: i32,
        v: i32,
        b: [f32; 3],
        vs: &[MeshVertex; 3],
    ) {
        let old_height = t.height(u, v);
        let old_terrain = t.id(u, v);
        let weight = vs[0].weight * b[0] + vs[1].weight * b[1] + vs[2].weight * b[2];
        let h0 = b[0] * vs[0].pos.z;
        let h1 = b[1] * vs[1].pos.z;
        let h2 = b[2] * vs[2].pos.z;
        let blend = if weight < 0.0 {
            0.0
        } else if 1.0 < weight {
            1.0
        } else {
            weight
        };
        if *written {
            return;
        }
        let mesh_height = (h0 + h1 + h2) / HEIGHT_SCALE;
        *written = true;
        let old = f32::from(old_height) / HEIGHT_SCALE;
        let new_height = (((1.0 - blend) * old + mesh_height * blend) * HEIGHT_SCALE) as i32;
        if u32::from(new_height as u16) <= SKIRT_RETEXTURE_HEIGHT {
            for c in 0..4 {
                if old_terrain == self.ids.skirt_from[c] && old_terrain != NO_TERRAIN {
                    let to = self.ids.skirt_to[c];
                    if to != NO_TERRAIN {
                        t.set_id(u, v, to);
                    }
                }
            }
        }
        t.set_height(u, v, new_height as u16);
    }

    /// One texel of a bed triangle.
    // re: Fun_087fa59c 0x087fa59c
    fn bed_texel(&self, t: &mut impl Texels, u: i32, v: i32, b: [f32; 3], vs: &[MeshVertex; 3]) {
        let weight = vs[0].weight * b[0] + vs[1].weight * b[1] + vs[2].weight * b[2];
        let mesh_height =
            (b[0] * vs[0].pos.z + b[1] * vs[1].pos.z + b[2] * vs[2].pos.z) / HEIGHT_SCALE;
        let across =
            (vs[0].across * b[0] + vs[1].across * b[1] + vs[2].across * b[2]) * BED_ACROSS_SCALE;
        let mut angle = 0.0f32;
        if !(0.0 > across) {
            angle = if 1.0 < across {
                BED_HALF_PI
            } else {
                0.5 * across * BED_PI
            };
        }
        let dip = angle.sin() * BED_DIP_DEPTH / BED_ACROSS_SCALE;
        let old = f32::from(t.height(u, v)) / HEIGHT_SCALE;
        let mut new = (1.0 - weight) * old + (mesh_height + dip) * weight;
        new = if new < old { new } else { old };
        let floor = BED_FLOOR + BED_FLOOR_OFFSET;
        if floor > new {
            new = if old < floor { old } else { floor };
        }
        t.set_height(u, v, (new * HEIGHT_SCALE) as i32 as u16);
        // TODO(RW-12): a missing bed row writes 0xff in the original; 0xff is no terrain here, so
        // the id stays.
        if self.ids.bed != NO_TERRAIN {
            t.set_id(u, v, self.ids.bed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::RiverSpline;
    use super::*;
    use crate::world::terrain::field::TerrainField;
    use crate::world::terrain::stencil::TerrainStencil;

    const W: u32 = 256;
    const H: u32 = 128;
    const Y: f32 = 64.0;
    const FLAT: u16 = 15_000;
    const MESH: f32 = 14_000.0;

    fn spline(points: Vec<Vec3>) -> RiverSpline {
        RiverSpline {
            points,
            ends_at_mouth: false,
        }
    }

    /// A straight river along +x, 8 points 6 texels apart from x = 60, banks 3 texels away
    /// (the left one on +y, the left of the flow).
    fn straight(height: f32) -> RiverSim {
        let at = |dy: f32| {
            spline(
                (0..8)
                    .map(|i| Vec3::new(60.0 + 6.0 * i as f32, Y + dy, 0.0))
                    .collect(),
            )
        };
        RiverSim {
            rivers: vec![at(0.0)],
            left_banks: vec![at(3.0)],
            right_banks: vec![at(-3.0)],
            heights: vec![(0..8).map(|i| height - 10.0 * i as f32).collect()],
            ..RiverSim::default()
        }
    }

    const IDS: RiverIds = RiverIds {
        skirt_from: [3, NO_TERRAIN, NO_TERRAIN, NO_TERRAIN],
        skirt_to: [7, NO_TERRAIN, NO_TERRAIN, NO_TERRAIN],
        bed: 9,
    };

    fn field(height: u16) -> TerrainField {
        let mut f = TerrainField::with_texels(W, H);
        for v in 0..H as i32 {
            for u in 0..W as i32 {
                f.set(u, v, height, 3);
            }
        }
        f
    }

    fn blend_of(sim: &RiverSim, ids: &RiverIds) -> RiverBlend {
        RiverBlend::new(sim, ids).with_map_size(W, H)
    }

    /// All cells of the field, in the given order, through the windows.
    fn raster_cells(blend: &RiverBlend, field: &mut TerrainField, reverse: bool) {
        let mut cells: Vec<CellRect> = Vec::new();
        for cy in 0..(H / CELL_SIZE) as i32 {
            for cx in 0..(W / CELL_SIZE) as i32 {
                cells.push(CellRect::cell(cx, cy));
            }
        }
        if reverse {
            cells.reverse();
        }
        let mut stencil = TerrainStencil::new(W, H);
        for cell in cells {
            let mut win = FieldWindow::new(field, &mut stencil, cell);
            blend.raster(cell, &mut win);
        }
    }

    fn snapshot(f: &TerrainField) -> Vec<(u16, u8)> {
        let mut out = Vec::new();
        for v in 0..H as i32 {
            for u in 0..W as i32 {
                out.push((f.get_height(u, v), f.get_id(u, v)));
            }
        }
        out
    }

    #[test]
    fn the_mesh_has_the_triangles_of_the_steps_and_the_caps() {
        let sim = straight(MESH);
        let mesh = create_sim_meshes(&sim);
        // The loop of the original in single precision: t = 0.1, 0.2, ... up to 6 + 1e-4.
        let mut steps = 0;
        let mut t = 0.1f32;
        while t <= 6.0 + 1.0e-4 {
            steps += 1;
            t = if t <= 6.0 { t } else { 6.0 } + 0.1;
        }
        assert_eq!(mesh.bed.len(), (4 * steps + 4) * 3);
        assert_eq!(mesh.skirts.len(), (6 * steps + 12) * 3);
        // The bed is full weight; across is 1 on the centre line and 0 on the banks.
        assert!(mesh.bed.iter().take(12 * steps).all(|v| v.weight == 1.0));
        assert!(mesh.bed.iter().any(|v| v.across == 1.0));
        assert!(mesh.skirts.iter().any(|v| v.weight == 0.0));
        // A river with three points makes nothing.
        let mut short = straight(MESH);
        short.rivers[0].points.truncate(3);
        assert_eq!(create_sim_meshes(&short), RiverMesh::default());
    }

    #[test]
    fn the_bed_is_cut_below_the_field_and_smooth_across() {
        let sim = straight(MESH);
        let blend = blend_of(&sim, &IDS);
        let mut f = field(FLAT);
        raster_cells(&blend, &mut f, false);
        let x = 80;
        let at = |dy: i32| f.get_height(x, Y as i32 + dy);
        // Never above the original, lower than it on the centre line.
        for v in 0..H as i32 {
            for u in 0..W as i32 {
                assert!(f.get_height(u, v) <= FLAT, "{u},{v}");
            }
        }
        assert!(at(0) < FLAT - 1000, "{}", at(0));
        // Centre lowest, rising to the sides, smooth (no jump over 3000 between neighbours).
        let profile: Vec<u16> = (-14..=14).map(at).collect();
        for pair in profile.windows(2) {
            assert!(
                (i32::from(pair[0]) - i32::from(pair[1])).abs() < 3000,
                "{profile:?}"
            );
        }
        for dy in 1..14 {
            assert!(at(dy) >= at(dy - 1), "up {dy}: {profile:?}");
            assert!(at(-dy) >= at(-dy + 1), "down {dy}: {profile:?}");
        }
        // The bed terrain is on the bed, far away nothing changed.
        assert_eq!(f.get_id(x, Y as i32), 9);
        for (u, v) in [(x, 10), (x, 120), (5, Y as i32), (250, Y as i32)] {
            assert_eq!((f.get_height(u, v), f.get_id(u, v)), (FLAT, 3), "{u},{v}");
        }
    }

    #[test]
    fn the_bed_never_goes_below_the_floor() {
        let floor = ((BED_FLOOR + BED_FLOOR_OFFSET) * 65535.0) as u16;
        let lowest = |f: &TerrainField| {
            (0..W as i32)
                .flat_map(|u| (0..H as i32).map(move |v| (u, v)))
                .map(|(u, v)| f.get_height(u, v))
                .min()
                .unwrap()
        };
        // A mesh a little below the floor: the skirt pulls the field to the mesh, the bed stops
        // at the floor (the skirt pass has no floor, the bed pass keeps what is lower).
        let sim = straight(f32::from(floor) + 400.0);
        let blend = blend_of(&sim, &IDS);
        let mut f = field(FLAT);
        raster_cells(&blend, &mut f, false);
        let low = lowest(&f);
        assert!(low >= floor - 1, "{low} < {floor}");
        assert!(low <= floor + 1, "{low} {floor}");
        // A field that is below the floor already is not raised by the bed.
        let start = floor - 500;
        let sim = straight(f32::from(start) + 70.0);
        let blend = blend_of(&sim, &IDS);
        let mut lower = field(start);
        raster_cells(&blend, &mut lower, false);
        assert!(lowest(&lower) >= start - 2, "{}", lowest(&lower));
        assert!(lower.get_height(80, Y as i32) <= start + 80);
    }

    #[test]
    fn cells_in_any_order_equal_one_big_cell() {
        let sim = straight(MESH);
        let blend = blend_of(&sim, &IDS);
        let mut a = field(FLAT);
        raster_cells(&blend, &mut a, false);
        let mut b = field(FLAT);
        raster_cells(&blend, &mut b, true);
        assert_eq!(snapshot(&a), snapshot(&b));

        let mut whole = field(FLAT);
        let all = CellRect::new(0, 0, W as i32, H as i32);
        let mut stencil = TerrainStencil::new(W, H);
        let mut win = FieldWindow::new(&mut whole, &mut stencil, all);
        blend.raster(all, &mut win);
        assert_eq!(snapshot(&a), snapshot(&whole));
        assert_ne!(snapshot(&whole), snapshot(&field(FLAT)));
    }

    #[test]
    fn the_skirt_retextures_low_texels_of_the_source_terrain_only() {
        let sim = straight(MESH);
        let blend = blend_of(&sim, &IDS);
        let mut f = field(FLAT);
        raster_cells(&blend, &mut f, false);
        // Inside the bed: the bed terrain. In the skirt, 10 texels from the centre line: the
        // retextured terrain. Far away: untouched.
        assert_eq!(f.get_id(80, Y as i32), 9);
        assert_eq!(f.get_id(80, Y as i32 + 10), 7);
        assert_eq!(f.get_id(80, Y as i32 - 10), 7);
        assert_eq!(f.get_id(80, Y as i32 + 30), 3);

        // Another source terrain stays.
        let mut other = field(FLAT);
        for v in 0..H as i32 {
            for u in 0..W as i32 {
                other.set_id(u, v, 4);
            }
        }
        raster_cells(&blend, &mut other, false);
        assert_eq!(other.get_id(80, Y as i32 + 10), 4);

        // A high field and high mesh: new height above 35000, no retexture; the bed still marks.
        let high = straight(40_000.0);
        let blend = blend_of(&high, &IDS);
        let mut f = field(40_000);
        raster_cells(&blend, &mut f, false);
        assert_eq!(f.get_id(80, Y as i32 + 10), 3);
        assert_eq!(f.get_id(80, Y as i32), 9);
    }

    #[test]
    fn missing_ids_write_nothing() {
        let sim = straight(MESH);
        let blend = blend_of(&sim, &RiverIds::NONE);
        let mut f = field(FLAT);
        raster_cells(&blend, &mut f, false);
        assert!(f.get_height(80, Y as i32) < FLAT - 1000);
        for v in 0..H as i32 {
            for u in 0..W as i32 {
                assert_eq!(f.get_id(u, v), 3);
            }
        }
    }

    #[test]
    fn a_river_over_the_seam_wraps() {
        // The river starts left of x = 0 and the texels wrap to the right edge of the map.
        let at = |dy: f32| {
            spline(
                (0..8)
                    .map(|i| Vec3::new(-20.0 + 6.0 * i as f32, Y + dy, 0.0))
                    .collect(),
            )
        };
        let sim = RiverSim {
            rivers: vec![at(0.0)],
            left_banks: vec![at(3.0)],
            right_banks: vec![at(-3.0)],
            heights: vec![vec![MESH; 8]],
            ..RiverSim::default()
        };
        let blend = blend_of(&sim, &IDS);
        let mut f = field(FLAT);
        raster_cells(&blend, &mut f, false);
        assert!(f.get_height(W as i32 - 10, Y as i32) < FLAT - 1000);
        assert!(f.get_height(10, Y as i32) < FLAT - 1000);
        assert_eq!(f.get_height(W as i32 / 2, Y as i32), FLAT);
    }
}
