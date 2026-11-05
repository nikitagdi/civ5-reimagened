//! The blend of the coast meshes into the field:
//! `BlendAllCoastMeshes` and `CoastMeshBlendJob`. Every triangle of a mesh is rasterized over the
//! texels it covers; the height of a texel moves toward the height of the mesh by a quadratic
//! weight, the terrain id of the first vertex is written, and a texel left low by the blend takes
//! the beach texture.
//!
//! The original runs the batches of a pass at once, so where two meshes overlap the result is
//! schedule-dependent. Here the order is fixed: the even batches of all meshes in the order of the
//! meshes, then the odd ones. The texels are independent of each other, so a cell of the field
//! can be written alone by the triangles that touch it, in the same order, with the same result as
//! the whole map at once ([`BlendWork::apply_cell`] against [`BlendWork::apply_all`]).

use std::collections::HashMap;

use super::levels::NO_TERRAIN;
use super::mesh::{Batch, CoastMesh};
use crate::world::terrain::build::FieldWindow;
use crate::world::terrain::build::rand::MsvcRand;
use crate::world::terrain::field::{CELL_SIZE, CellRect, TerrainField};
use crate::world::terrain::library::TerrainDefs;
use crate::world::terrain::sampler::TerrainFrame;

/// A texel at or below this height after the blend takes the beach texture.
// re: CoastMeshBlendJob 0x088eaf32 (`if (usNewHeight > 35000) continue;`)
const BEACH_HEIGHT: u16 = 35_000;

/// Id of the terrain of the beach (`prefix | 0x35`) and of the sand it becomes (`prefix | 0x10`).
// re: BlendAllCoastMeshes 0x088e8ff4 (`uVar14 & g_mTextureMask | ~g_mTextureMask & 0x35` / `& 0x10`)
const BEACH_ID: u32 = 0x35;
const SAND_ID: u32 = 0x10;

/// What the blend reads and writes of the field: the whole field, or a window on one cell.
pub trait Texels {
    /// Whether the texel is one this writer owns.
    fn in_scope(&self, u: i32, v: i32) -> bool;
    fn height(&self, u: i32, v: i32) -> u16;
    fn id(&self, u: i32, v: i32) -> u8;
    fn set_height(&mut self, u: i32, v: i32, raw: u16);
    fn set_id(&mut self, u: i32, v: i32, id: u8);
}

impl Texels for TerrainField {
    fn in_scope(&self, u: i32, v: i32) -> bool {
        self.contains(u, v)
    }
    fn height(&self, u: i32, v: i32) -> u16 {
        self.get_height(u, v)
    }
    fn id(&self, u: i32, v: i32) -> u8 {
        self.get_id(u, v)
    }
    fn set_height(&mut self, u: i32, v: i32, raw: u16) {
        TerrainField::set_height(self, u, v, raw);
    }
    fn set_id(&mut self, u: i32, v: i32, id: u8) {
        TerrainField::set_id(self, u, v, id);
    }
}

impl Texels for FieldWindow<'_> {
    fn in_scope(&self, u: i32, v: i32) -> bool {
        self.rect().contains(u, v)
    }
    fn height(&self, u: i32, v: i32) -> u16 {
        FieldWindow::height(self, u, v)
    }
    fn id(&self, u: i32, v: i32) -> u8 {
        FieldWindow::id(self, u, v)
    }
    fn set_height(&mut self, u: i32, v: i32, raw: u16) {
        FieldWindow::set_height(self, u, v, raw);
    }
    fn set_id(&mut self, u: i32, v: i32, id: u8) {
        FieldWindow::set_id(self, u, v, id);
    }
}

/// The terrain rows of the beach remap: texels of the beach texture (id 0x35 of the continent)
/// that the blend leaves low become the sand (id 0x10), per continent 1..4. A continent that lacks
/// either row has no remap.
// re: BlendAllCoastMeshes 0x088e8ff4 (the loop over `g_aContinentPrefix[1..=4]`)
pub fn remap_pairs(defs: &TerrainDefs) -> [(u8, u8); 4] {
    let find = |c: u8, id: u32| {
        defs.rows()
            .iter()
            .position(|r| r.continent == c && r.id == id)
            .map(|i| i as u8)
    };
    std::array::from_fn(|k| {
        let c = k as u8 + 1;
        let from = find(c, BEACH_ID).unwrap_or(NO_TERRAIN);
        match find(c, SAND_ID) {
            Some(to) if to != NO_TERRAIN => (from, to),
            _ => (NO_TERRAIN, NO_TERRAIN),
        }
    })
}

/// A triangle of a batch, by place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TriRef {
    mesh: u32,
    batch: u32,
    tri: u32,
}

/// A triangle prepared for the rasterizer: the texels it can cover and the constants of the
/// barycentric weights.
// re: Fun_088eaf32 0x088eaf32 (CoastMeshBlendJob.cpp:21-58)
struct Plan {
    /// The vertices relative to the first covered texel.
    v: [(f32, f32); 3],
    first_x: i32,
    end_x: i32,
    first_y: i32,
    end_y: i32,
    inv_area2: f32,
    c: [f32; 3],
}

impl Plan {
    fn new(p: [glam::Vec2; 3]) -> Option<Self> {
        let origin_x = (p[0].x.min(p[1].x.min(p[2].x)) + 0.5).floor();
        let origin_y = (p[0].y.min(p[1].y.min(p[2].y)) + 0.5).floor();
        let max_x = p[0].x.max(p[1].x.max(p[2].x));
        let max_y = p[0].y.max(p[1].y.max(p[2].y));
        let v = p.map(|q| (q.x - origin_x, q.y - origin_y));
        let ((x0, y0), (x1, y1), (x2, y2)) = (v[0], v[1], v[2]);
        let edge0 = x1 * y2 - y1 * x2;
        let area2 = x0 * (y1 - y2) + y0 * (x2 - x1) + edge0;
        if f64::from(area2.abs()) < 1.0e-5 {
            return None;
        }
        let first_y = origin_y as i32;
        let end_y = max_y as i32 + 1;
        if first_y >= end_y {
            return None;
        }
        let inv_area2 = 1.0 / area2;
        Some(Self {
            v,
            first_x: origin_x as i32,
            end_x: max_x as i32 + 1,
            first_y,
            end_y,
            inv_area2,
            c: [
                edge0 * inv_area2,
                (x2 * y0 - x0 * y2) * inv_area2,
                (x0 * y1 - y0 * x1) * inv_area2,
            ],
        })
    }

    fn rect(&self) -> CellRect {
        CellRect::new(self.first_x, self.first_y, self.end_x, self.end_y)
    }
}

/// The meshes, ready to be blended: the order of the triangles and, per cell of the field, the
/// triangles that touch it.
pub struct BlendWork {
    meshes: Vec<CoastMesh>,
    pairs: [(u8, u8); 4],
    map_w: i32,
    map_h: i32,
    wrap: bool,
    order: Vec<TriRef>,
    buckets: HashMap<(i32, i32), Vec<u32>>,
}

impl BlendWork {
    /// Prepares the meshes. Draws twice from `rng` when there is a mesh: the original picks a mesh
    /// and a batch to run as a debug job that tramples a block of the heap; the numbers take no
    /// part in the result, but the stream must advance.
    // re: cvCoastMeshEngine::BlendAllCoastMeshes 0x088e8ff4 (the two `rand_wincompatible` at 0x088e91e2, 0x088e9229)
    pub fn new(
        meshes: Vec<CoastMesh>,
        frame: &TerrainFrame,
        defs: &TerrainDefs,
        rng: &mut MsvcRand,
    ) -> Self {
        if !meshes.is_empty() {
            let picked = rng.next() as usize % meshes.len();
            // The batch of the picked mesh that the debug job runs. The original divides by half
            // the number of batches, which is never 0 for a mesh of 4 or more points.
            let _debug_batch = (rng.next() as usize)
                .checked_rem(meshes[picked].batches.len() / 2)
                .map(|b| b * 2);
        }
        let (w, h) = frame.texel_size();
        let mut work = Self {
            pairs: remap_pairs(defs),
            map_w: w as i32,
            map_h: h as i32,
            wrap: frame.wrap_x(),
            order: Vec::new(),
            buckets: HashMap::new(),
            meshes,
        };
        // The even batches of every mesh, then the odd ones.
        for parity in [0usize, 1] {
            for (m, mesh) in work.meshes.iter().enumerate() {
                for b in (parity..mesh.batches.len()).step_by(2) {
                    for t in 0..mesh.batches[b].num_triangles {
                        work.order.push(TriRef {
                            mesh: m as u32,
                            batch: b as u32,
                            tri: t as u32,
                        });
                    }
                }
            }
        }
        let (cells_x, cells_y) = (w as i32 / CELL_SIZE as i32, h as i32 / CELL_SIZE as i32);
        for (n, r) in work.order.iter().enumerate() {
            let Some(plan) = work.plan(r) else {
                continue;
            };
            for cell in plan.rect().cells(cells_x, cells_y, work.wrap) {
                work.buckets.entry(cell).or_default().push(n as u32);
            }
        }
        work
    }

    fn corners(&self, r: &TriRef) -> (&CoastMesh, &Batch, [usize; 3]) {
        let mesh = &self.meshes[r.mesh as usize];
        let batch = &mesh.batches[r.batch as usize];
        let ix = &mesh.batch_indices(batch)[r.tri as usize * 3..r.tri as usize * 3 + 3];
        (
            mesh,
            batch,
            [usize::from(ix[0]), usize::from(ix[1]), usize::from(ix[2])],
        )
    }

    fn plan(&self, r: &TriRef) -> Option<Plan> {
        let (mesh, batch, ix) = self.corners(r);
        let vs = mesh.batch_vertices(batch);
        Plan::new(ix.map(|i| vs[i].pos))
    }

    /// Triangles to blend.
    pub fn triangles(&self) -> usize {
        self.order.len()
    }

    pub fn meshes(&self) -> &[CoastMesh] {
        &self.meshes
    }

    /// Blends all triangles.
    pub fn apply_all(&self, t: &mut impl Texels) {
        for r in &self.order {
            self.blend_triangle(r, t);
        }
    }

    /// Blends the triangles that touch the cell, writing only the texels `t` owns.
    pub fn apply_cell(&self, cell: CellRect, t: &mut impl Texels) {
        let s = CELL_SIZE as i32;
        let key = (cell.u0.div_euclid(s), cell.v0.div_euclid(s));
        let Some(list) = self.buckets.get(&key) else {
            return;
        };
        for &n in list {
            self.blend_triangle(&self.order[n as usize], t);
        }
    }

    /// One triangle of `CoastMeshBlendJob`.
    // re: Fun_088eaf32 0x088eaf32 (CoastMeshBlendJob.cpp:17-129)
    fn blend_triangle(&self, r: &TriRef, t: &mut impl Texels) {
        let (mesh, batch, ix) = self.corners(r);
        let vs = mesh.batch_vertices(batch);
        let (v0, v1, v2) = (vs[ix[0]], vs[ix[1]], vs[ix[2]]);
        let Some(p) = self.plan(r) else {
            return;
        };
        let ((x0, y0), (x1, y1), (x2, y2)) = (p.v[0], p.v[1], p.v[2]);
        // Barycentric weight of vertex k at (dx, dy): (A_k dx + B_k dy + C_k) / area2; the step is
        // 1 / area2 per texel, so A_k and B_k need no division.
        // The weights of the whole mesh are indexed by the number of the triangle inside the
        // batch: every batch holds whole segments, so the pattern of eight is the same.
        // re: BlendAllCoastMeshes 0x088e8ff4 (`*(a3 + 0x18)` is the base of the weights of the mesh, not of the batch)
        let edge = mesh.edge_weights[r.tri as usize];
        let mut dy = 0.0f32;
        for y in p.first_y..p.end_y {
            let cur_dy = dy;
            dy += p.inv_area2;
            let row0 = (x2 - x1) * cur_dy;
            let row1 = (x0 - x2) * cur_dy;
            let row2 = (x1 - x0) * cur_dy;
            let mut dx = 0.0f32;
            for x in p.first_x..p.end_x {
                let cur_dx = dx;
                dx += p.inv_area2;
                let w0 = (y1 - y2) * cur_dx + row0 + p.c[0];
                if w0 < 0.0 {
                    continue;
                }
                let w1 = (y2 - y0) * cur_dx + row1 + p.c[1];
                if w1 < 0.0 {
                    continue;
                }
                let w2 = (y0 - y1) * cur_dx + row2 + p.c[2];
                if w2 < 0.0 {
                    continue;
                }
                if y < 0 || y >= self.map_h {
                    continue;
                }
                // The position of the texel, wrapped along X when the map wraps.
                // re: cvTerrainSampler::GetWrappedPosition (cvTerrainSampler.cpp:150)
                let wx = if self.wrap {
                    x.rem_euclid(self.map_w)
                } else {
                    x
                };
                if !t.in_scope(wx, y) {
                    continue;
                }
                let old_height = t.height(wx, y);
                let old_terrain = t.id(wx, y);
                // The first vertex alone decides the terrain of the triangle.
                if v0.terrain != NO_TERRAIN {
                    t.set_id(wx, y, v0.terrain);
                }
                // The quadratic patch: the weights of the vertices on the corners, those of the
                // edges doubled on the mixed terms.
                let mut blend = v0.weight * w0 * w0
                    + v1.weight * w1 * w1
                    + v2.weight * w2 * w2
                    + 2.0 * (edge[0] * w0 * w1)
                    + 2.0 * (edge[1] * w1 * w2)
                    + 2.0 * (edge[2] * w2 * w0);
                if blend < 0.0 {
                    blend = 0.0;
                }
                let mesh_height = v0.height * w0 + v1.height * w1 + v2.height * w2;
                let old = f32::from(old_height) / 65535.0;
                // No clamp: a blend above 1 or a height outside [0, 1] wraps in the 16-bit store.
                let new_height =
                    (((1.0 - blend) * old + mesh_height * blend) * 65535.0) as i32 as u16;
                t.set_height(wx, y, new_height);
                if new_height > BEACH_HEIGHT {
                    continue;
                }
                // Compares the terrain read before the vertex terrain was written.
                for &(from, to) in &self.pairs {
                    if old_terrain == from {
                        t.set_id(wx, y, to);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::levels::tests::{defs_with_ids, island_levels};
    use super::super::mesh::generate_final_mesh;
    use super::super::testkit::{frame_of, island};
    use super::*;
    use crate::world::terrain::sampler::SEA_LEVEL_RAW;

    fn island_work(rng: &mut MsvcRand) -> (BlendWork, TerrainFrame, Vec<usize>) {
        let (splines, levels, set) = island_levels(false);
        let b = island(20, 20, false, 2);
        let frame = frame_of(&b);
        let meshes: Vec<CoastMesh> = levels
            .iter()
            .map(|l| generate_final_mesh(l, &set))
            .collect();
        let sizes = splines.iter().map(|s| s.points.len()).collect();
        let work = BlendWork::new(meshes, &frame, &defs_with_ids(&[5, 16, 53]), rng);
        (work, frame, sizes)
    }

    fn flat_field(frame: &TerrainFrame) -> TerrainField {
        let (w, h) = frame.texel_size();
        let mut field = TerrainField::with_texels(w, h);
        for v in 0..h as i32 {
            for u in 0..w as i32 {
                field.set(u, v, 0x3c3f, 1);
            }
        }
        field
    }

    #[test]
    fn the_remap_pairs_need_both_rows_of_a_continent() {
        let both = remap_pairs(&defs_with_ids(&[0x35, 0x10]));
        assert!(
            both.iter()
                .all(|&(f, t)| f != NO_TERRAIN && t != NO_TERRAIN)
        );
        // Continent 1 is row 2 (0x35) and 3 (0x10) of the table: 2 rows per continent.
        assert_eq!(both[0], (2, 3));
        let only_from = remap_pairs(&defs_with_ids(&[0x35]));
        assert!(only_from.iter().all(|&p| p == (NO_TERRAIN, NO_TERRAIN)));
        // The sand without the beach: nothing matches the beach, so nothing is remapped.
        let only_to = remap_pairs(&defs_with_ids(&[0x10]));
        assert!(only_to.iter().all(|&(f, _)| f == NO_TERRAIN));
    }

    #[test]
    fn no_mesh_means_no_draw_and_a_mesh_means_two() {
        let frame = TerrainFrame::new(1, 1, false);
        let mut rng = MsvcRand::new(1);
        let work = BlendWork::new(Vec::new(), &frame, &defs_with_ids(&[]), &mut rng);
        assert_eq!(rng.calls(), 0);
        assert_eq!(work.triangles(), 0);
        let mut rng = MsvcRand::new(1);
        let (work, _, _) = island_work(&mut rng);
        assert_eq!(rng.calls(), 2);
        assert!(work.triangles() > 1000);
    }

    #[test]
    fn the_blend_carves_the_beach_into_a_flat_field_and_leaves_the_far_texels_alone() {
        let mut rng = MsvcRand::new(3);
        let (work, frame, sizes) = island_work(&mut rng);
        let mut field = flat_field(&frame);
        work.apply_all(&mut field);
        // A texel in the middle of the island is far from every line: untouched.
        let c = frame.hex_to_tex(10, 10);
        assert_eq!(field.get_height(c.x as i32, c.y as i32), 0x3c3f);
        assert_eq!(field.get_id(c.x as i32, c.y as i32), 1);
        // Near the waterline of the shore (the shorter of the two lines) the field is lowered
        // toward the water, and not below it.
        let shore = sizes.iter().position(|&n| n < 70).unwrap();
        let line = work.meshes()[shore]
            .vertices
            .chunks(8)
            .map(|s| s[3].pos)
            .collect::<Vec<_>>();
        let mut lowest = u16::MAX;
        for p in line.iter().step_by(11) {
            lowest = lowest.min(field.get_height(p.x.round() as i32, p.y.round() as i32));
        }
        // (The low beach of the synthetic styles is at 6553; the texel is up to half a texel off
        // the line, on the steep side.)
        assert!(lowest < 0x3c3f, "{lowest}");
        assert!(lowest >= 6500, "{lowest}");
        assert!(SEA_LEVEL_RAW > lowest);
    }

    #[test]
    fn a_cell_alone_gets_what_the_whole_map_gets() {
        let mut rng = MsvcRand::new(3);
        let (work, frame, _) = island_work(&mut rng);
        let mut whole = flat_field(&frame);
        work.apply_all(&mut whole);
        // Every cell of the map, one by one, on a field that started the same way.
        let (w, h) = frame.texel_size();
        let mut by_cell = flat_field(&frame);
        let mut stencil = crate::world::terrain::stencil::TerrainStencil::new(w, h);
        for cy in 0..(h / CELL_SIZE) as i32 {
            for cx in 0..(w / CELL_SIZE) as i32 {
                let rect = CellRect::cell(cx, cy);
                let mut window = FieldWindow::new(&mut by_cell, &mut stencil, rect);
                work.apply_cell(rect, &mut window);
            }
        }
        assert!(whole.same_content(&by_cell));
        assert!(whole.digest() != flat_field(&frame).digest());
    }
}
