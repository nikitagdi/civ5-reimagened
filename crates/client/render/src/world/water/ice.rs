//! The polar ice (world-water 4.8, WW-8, card R5.8): `cvTerrainIceSystem` of the original without
//! the GPU. `world/ice.rs` draws what this makes.
//!
//! What is the original's (RW-22 (b)):
//! - **Which cells:** exactly the plots with `FEATURE_ICE` (`MarkIceHex` 0x086744c8), no terrain or
//!   latitude test.
//! - **Settings:** `IceSettings.xml`, or `IceSettings_Low.xml` when `TerrainDetailLevel` is below 2
//!   (`BuildIce` 0x086744f4; the other test, less than 64 MB of the temporary heap, has no meaning
//!   here). The low file differs in the samples and in `LatticeVertsPerHex` (8 for 12).
//! - **Vertex:** the place in the box of the mesh in `[-1, 1]` and the normal and the height in
//!   `[0, 1]` of `maxHeight + maxDepth` (`RenderSceneVS`: `z = v1.w * scale + bias`, `scale =
//!   LargePolyHeight + DisplaceMax + maxDepth`, `bias = sea level - maxDepth`).
//! - **Displacement:** `icedisplacement.dds` (`R8`) by `DisplaceFrequency`, `DisplaceMin`,
//!   `DisplaceMax`, `DisplaceExponent`.
//!
//! SIMPLIFIED(SIMP-043): the fracture of the original (`BuildIceMeshes` 0x0867456e: outline,
//! splines, a KD-tree, Voronoi, crack propagation by potentials, `TriangulateIce`) has no body in
//! the dump. Ours: `LatticeVertsPerHex` seeds a hex at places of our own, each floe the Voronoi
//! cell of a seed inside its hex, the cracks the cell walls opened by a width between
//! `CrackMinClip` and `CrackMaxClip`, the shore pulled in by `EdgePullAmount`, small floes
//! (`SmallPolyShrinkAmount` of a hex or less) at `SmallPolyHeight*` and the others at
//! `LargePolyHeight`, every floe with a skirt down to its depth. The displacement and the shading
//! are the original's.

use glam::{Vec2, Vec3};

use asset_format::{IceParams, IceSettings, TextureBlob, TextureFormat};

use crate::hexgrid::{CORNER_RADIUS, corner_offset};
use crate::world::terrain::basis::{BasisFeature, TerrainBasis};
use crate::world::terrain::sampler::TerrainFrame;

/// `TerrainDetailLevel` below which the low settings are read.
// re: cvTerrainIceSystem::BuildIce 0x086744f4 (`g_uTerrainDetailLevel <= 1` -> IceSettings_Low)
pub const LOW_DETAIL_BELOW: i64 = 2;

/// The settings the ice is built with at a `TerrainDetailLevel`.
pub fn params_for(settings: &IceSettings, terrain_detail: i64) -> &IceParams {
    if terrain_detail < LOW_DETAIL_BELOW {
        &settings.low
    } else {
        &settings.full
    }
}

/// The vertex of `RenderSceneVS`: `v0.xy` the place in the box in `[-1, 1]`, `v1` the normal and
/// the height, both in `[0, 1]`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct IceVertex {
    pub v0: [f32; 4],
    pub v1: [f32; 4],
}

/// The ice of a map, in the original's world.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IceMesh {
    pub vertices: Vec<IceVertex>,
    pub indices: Vec<u32>,
    /// `g_vBBoxMin`, `g_vBBoxMax` (x, y).
    pub bbox_min: Vec2,
    pub bbox_max: Vec2,
    /// `g_fHeightScale`, `g_fHeightBias`.
    pub height_scale: f32,
    pub height_bias: f32,
    /// The ice cells, `(col, row)`, for the mini-map.
    pub cells: Vec<(i32, i32)>,
    /// Floes made.
    pub floes: usize,
}

impl IceMesh {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// A deterministic number in `[0, 1)` of a cell and an index.
fn hash01(col: i32, row: i32, k: u32, salt: u32) -> f32 {
    let mut h = (col as u32).wrapping_mul(0x9e37_79b1)
        ^ (row as u32).wrapping_mul(0x85eb_ca77)
        ^ k.wrapping_mul(0xc2b2_ae3d)
        ^ salt.wrapping_mul(0x27d4_eb2f);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// The centre of a cell in the original's world.
fn centre(frame: &TerrainFrame, col: i32, row: i32) -> Vec2 {
    frame.orig_point(frame.hex_to_tex(col, row), 0.0).truncate()
}

/// Our corner offset `(X, Z)` as an offset of the original's world (`y = -Z`).
fn corner(i: usize) -> Vec2 {
    let c = corner_offset(i);
    Vec2::new(c.x, -c.y)
}

/// A half-plane `n . p <= d`.
#[derive(Clone, Copy)]
struct Half {
    n: Vec2,
    d: f32,
}

/// Cuts the convex polygon `poly` by `h`.
fn clip(poly: &[Vec2], h: Half) -> Vec<Vec2> {
    let mut out = Vec::with_capacity(poly.len() + 1);
    let n = poly.len();
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        let (da, db) = (h.n.dot(a) - h.d, h.n.dot(b) - h.d);
        if da <= 0.0 {
            out.push(a);
        }
        if (da <= 0.0) != (db <= 0.0) {
            let t = da / (da - db);
            out.push(a + (b - a) * t);
        }
    }
    out
}

fn area(poly: &[Vec2]) -> f32 {
    let n = poly.len();
    0.5 * (0..n)
        .map(|i| poly[i].perp_dot(poly[(i + 1) % n]))
        .sum::<f32>()
}

/// Reads the displacement map (`R8` level 0) at the world point `p` with the repeat `freq`.
fn displacement(tex: Option<&TextureBlob>, p: Vec2, freq: f32) -> f32 {
    let Some(t) = tex else { return 0.0 };
    let Some(level) = t.mips.first() else {
        return 0.0;
    };
    if t.width == 0 || t.height == 0 {
        return 0.0;
    }
    let bpp = match t.format {
        TextureFormat::R8 => 1,
        TextureFormat::Rgba8 => 4,
        _ => return 0.0,
    };
    let u = (p.x * freq).rem_euclid(1.0);
    let v = (p.y * freq).rem_euclid(1.0);
    let x = ((u * t.width as f32) as u32).min(t.width - 1);
    let y = ((v * t.height as f32) as u32).min(t.height - 1);
    let i = ((y * t.width + x) as usize) * bpp;
    level.get(i).map_or(0.0, |&b| f32::from(b) / 255.0)
}

/// The ice of `basis`: the floes of every `FEATURE_ICE` plot. `sea` is the sea level of the
/// original's world; `disp` the displacement map, if it is in.
pub fn build_ice(
    basis: &TerrainBasis,
    frame: &TerrainFrame,
    p: &IceParams,
    sea: f32,
    disp: Option<&TextureBlob>,
) -> IceMesh {
    let mut build = IceBuild::new(basis, frame, p, sea, disp);
    while !build.step(f64::INFINITY) {}
    build.finish()
}

/// Where an [`IceBuild`] is.
enum IcePhase {
    /// Making the floes of the cells, `next` cells done.
    Floes,
    /// Making the vertices of the floes, `next` floes done.
    Mesh,
    Done,
}

/// The ice made in steps that each stop at a time budget: the same mesh as [`build_ice`], so a big
/// map spends its frames a little at a time (the work of one cell or one floe is never split).
pub struct IceBuild {
    frame: TerrainFrame,
    params: IceParams,
    sea: f32,
    disp: Option<TextureBlob>,
    width: i32,
    height: i32,
    /// `is_ice` of every plot of the basis, row by row.
    ice: Vec<bool>,
    cells: Vec<(i32, i32)>,
    mesh: IceMesh,
    phase: IcePhase,
    next: usize,
    seeds_per_hex: u32,
    hex_area: f32,
    floes: Vec<(Vec<Vec2>, f32, f32)>,
    lo: Vec2,
    size: Vec2,
}

impl IceBuild {
    pub fn new(
        basis: &TerrainBasis,
        frame: &TerrainFrame,
        p: &IceParams,
        sea: f32,
        disp: Option<&TextureBlob>,
    ) -> Self {
        let ice: Vec<bool> = basis
            .plots
            .iter()
            .map(|pl| pl.feature == BasisFeature::Ice)
            .collect();
        let mut this = Self {
            frame: *frame,
            params: p.clone(),
            sea,
            disp: disp.cloned(),
            width: i32::from(basis.width),
            height: i32::from(basis.height),
            ice,
            cells: Vec::new(),
            mesh: IceMesh::default(),
            phase: IcePhase::Floes,
            next: 0,
            seeds_per_hex: p.lattice_verts_per_hex.round().max(1.0) as u32,
            hex_area: 1.5 * 3f32.sqrt() * CORNER_RADIUS * CORNER_RADIUS,
            floes: Vec::new(),
            lo: Vec2::ZERO,
            size: Vec2::ONE,
        };
        let mut cells = Vec::new();
        for row in 0..this.height {
            for col in 0..this.width {
                if this.is_ice(frame, col, row) {
                    cells.push((col, row));
                }
            }
        }
        this.mesh.cells = cells.clone();
        this.cells = cells;
        let max_depth = p
            .large_poly_depth
            .max(p.small_poly_depth_min)
            .max(p.small_poly_depth_max)
            .max(0.0);
        let max_height = p.large_poly_height + p.displace_max.max(0.0);
        // re: IceSettings::ReadXML 0x0890a86c derived values (maxHeight, maxDepth) and the constants
        // 0x60 / 0x70 of IceGlobalConstants.
        this.mesh.height_scale = (max_height + max_depth).max(1e-3);
        this.mesh.height_bias = sea - max_depth;
        if this.cells.is_empty() {
            this.phase = IcePhase::Done;
        }
        this
    }

    fn is_ice(&self, frame: &TerrainFrame, col: i32, row: i32) -> bool {
        frame
            .grid_shape()
            .normalize(col, row)
            .and_then(|(x, y)| {
                if x < 0 || y < 0 || x >= self.width || y >= self.height {
                    None
                } else {
                    self.ice.get((y * self.width + x) as usize).copied()
                }
            })
            .unwrap_or(false)
    }

    /// Works until `budget_ms` of the recorder's clock has passed (at least one cell or floe a
    /// call); `true` when the mesh is done.
    pub fn step(&mut self, budget_ms: f64) -> bool {
        let start = crate::perf::slow::now_ms();
        let over =
            |start: f64| budget_ms.is_finite() && crate::perf::slow::now_ms() - start >= budget_ms;
        loop {
            match self.phase {
                IcePhase::Done => return true,
                IcePhase::Floes => {
                    self.make_cell(self.next);
                    self.next += 1;
                    if self.next == self.cells.len() {
                        self.begin_mesh();
                    }
                }
                IcePhase::Mesh => {
                    self.make_floe(self.next);
                    self.next += 1;
                    if self.next == self.floes.len() {
                        self.floes = Vec::new();
                        self.phase = IcePhase::Done;
                    }
                }
            }
            if over(start) {
                return matches!(self.phase, IcePhase::Done);
            }
        }
    }

    /// The mesh, whole once [`Self::step`] gave `true`.
    pub fn finish(self) -> IceMesh {
        self.mesh
    }

    /// The floes of the cell `at` of the list.
    fn make_cell(&mut self, at: usize) {
        let (col, row) = self.cells[at];
        let frame = self.frame;
        let p = &self.params;
        let c = centre(&frame, col, row);
        let hexagon: Vec<Vec2> = (0..6).map(|i| c + corner(i)).collect();
        let seeds: Vec<Vec2> = (0..self.seeds_per_hex)
            .map(|k| {
                let a = hash01(col, row, k, 1) * std::f32::consts::TAU;
                let r = hash01(col, row, k, 2).sqrt() * CORNER_RADIUS * 0.85;
                c + Vec2::new(a.cos(), a.sin()) * r
            })
            .collect();
        // The sides that face no ice are the shore: pulled in.
        let mut shore: Vec<Half> = Vec::new();
        for i in 0..6 {
            let (a, b) = (hexagon[i], hexagon[(i + 1) % 6]);
            let mid = (a + b) * 0.5;
            let out = (mid - c).normalize_or_zero();
            let probe = mid + out * 4.0;
            // The neighbour across the side: the cell nearest the probe point.
            let (nc, nr) = nearest_cell(&frame, probe);
            let pull = if self.is_ice(&frame, nc, nr) {
                0.0
            } else {
                p.edge_pull_amount * 4.0
            };
            shore.push(Half {
                n: out,
                d: out.dot(mid) - pull,
            });
        }
        let mut made = Vec::new();
        for (k, &s) in seeds.iter().enumerate() {
            let mut poly = hexagon.clone();
            for h in &shore {
                poly = clip(&poly, *h);
            }
            for (j, &o) in seeds.iter().enumerate() {
                if j == k || poly.len() < 3 {
                    continue;
                }
                let n = (o - s).normalize_or_zero();
                if n == Vec2::ZERO {
                    continue;
                }
                let crack = p.crack_min_clip
                    + (p.crack_max_clip - p.crack_min_clip)
                        * hash01(col, row, (k.min(j) * 64 + k.max(j)) as u32, 3);
                let mid = (s + o) * 0.5;
                poly = clip(
                    &poly,
                    Half {
                        n,
                        d: n.dot(mid) - crack,
                    },
                );
            }
            if poly.len() < 3 || area(&poly).abs() < 1.0 {
                continue;
            }
            let small = area(&poly).abs() < p.small_poly_shrink_amount * self.hex_area / 4.0;
            let r = hash01(col, row, k as u32, 4);
            let (height, depth) = if small {
                (
                    p.small_poly_height_min
                        + (p.small_poly_height_max - p.small_poly_height_min) * r,
                    p.small_poly_depth_min + (p.small_poly_depth_max - p.small_poly_depth_min) * r,
                )
            } else {
                let edge = p.large_poly_edge_height_min
                    + (p.large_poly_edge_height_max - p.large_poly_edge_height_min) * r;
                (p.large_poly_height + edge, p.large_poly_depth)
            };
            made.push((poly, height, depth));
        }
        self.floes.extend(made);
    }

    /// All floes are made: the box of the mesh, and on to the vertices.
    fn begin_mesh(&mut self) {
        let mut lo = Vec2::splat(f32::MAX);
        let mut hi = Vec2::splat(f32::MIN);
        for (poly, _, _) in &self.floes {
            for q in poly {
                lo = lo.min(*q);
                hi = hi.max(*q);
            }
        }
        self.mesh.bbox_min = lo;
        self.mesh.bbox_max = hi;
        self.lo = lo;
        self.size = (hi - lo).max(Vec2::splat(1e-3));
        self.next = 0;
        self.phase = if self.floes.is_empty() {
            IcePhase::Done
        } else {
            IcePhase::Mesh
        };
    }

    fn push(&mut self, q: Vec3, n: Vec3) -> u32 {
        let m = &mut self.mesh;
        let uv = (q.truncate() - self.lo) / self.size * 2.0 - Vec2::ONE;
        let n = n.normalize_or(Vec3::Z) * 0.5 + Vec3::splat(0.5);
        let h = ((q.z - m.height_bias) / m.height_scale).clamp(0.0, 1.0);
        m.vertices.push(IceVertex {
            v0: [uv.x, uv.y, 0.0, 1.0],
            v1: [n.x, n.y, n.z, h],
        });
        m.vertices.len() as u32 - 1
    }

    /// The vertices and the triangles of the floe `at`.
    fn make_floe(&mut self, at: usize) {
        let (poly, height, depth) = std::mem::take(&mut self.floes[at]);
        let sea = self.sea;
        let p = displace_of(&self.params);
        let top_z = |disp: Option<&TextureBlob>, q: Vec2, height: f32| {
            let d = displacement(disp, q, p.frequency);
            let d = p.min + (p.max - p.min) * d.powf(p.exponent.max(0.01));
            sea + height + d
        };
        self.mesh.floes += 1;
        let poly: Vec<Vec2> = if area(&poly) < 0.0 {
            poly.iter().rev().copied().collect()
        } else {
            poly
        };
        let mid: Vec2 = poly.iter().copied().sum::<Vec2>() / poly.len() as f32;
        // The top: a fan with one ring between the middle and the rim, so the displacement shows.
        let inner: Vec<Vec2> = poly.iter().map(|q| mid + (*q - mid) * 0.55).collect();
        let disp = self.disp.take();
        let at_top = |q: Vec2| Vec3::new(q.x, q.y, top_z(disp.as_ref(), q, height));
        let m0 = at_top(mid);
        let rim: Vec<Vec3> = poly.iter().map(|&q| at_top(q)).collect();
        let ring: Vec<Vec3> = inner.iter().map(|&q| at_top(q)).collect();
        self.disp = disp;
        let n = poly.len();
        let tri_normal = |a: Vec3, b: Vec3, c: Vec3| (b - a).cross(c - a);
        let ci = self.push(m0, Vec3::Z);
        let ri: Vec<u32> = ring.iter().map(|&q| self.push(q, Vec3::Z)).collect();
        let oi: Vec<u32> = rim.iter().map(|&q| self.push(q, Vec3::Z)).collect();
        // Normals of the top from its faces (counter-clockwise seen from above: up).
        let mut normals = vec![Vec3::ZERO; 1 + 2 * n];
        let mut tris: Vec<[usize; 3]> = Vec::new();
        for i in 0..n {
            let j = (i + 1) % n;
            tris.push([0, 1 + i, 1 + j]);
            tris.push([1 + i, 1 + n + i, 1 + n + j]);
            tris.push([1 + i, 1 + n + j, 1 + j]);
        }
        let pos = |k: usize| {
            if k == 0 {
                m0
            } else if k <= n {
                ring[k - 1]
            } else {
                rim[k - 1 - n]
            }
        };
        for t in &tris {
            let nn = tri_normal(pos(t[0]), pos(t[1]), pos(t[2]));
            for &k in t {
                normals[k] += nn;
            }
        }
        let index = |k: usize| {
            if k == 0 {
                ci
            } else if k <= n {
                ri[k - 1]
            } else {
                oi[k - 1 - n]
            }
        };
        for (k, nn) in normals.iter().enumerate() {
            let v = &mut self.mesh.vertices[index(k) as usize];
            let e = nn.normalize_or(Vec3::Z) * 0.5 + Vec3::splat(0.5);
            v.v1[0] = e.x;
            v.v1[1] = e.y;
            v.v1[2] = e.z;
        }
        for t in &tris {
            // Clockwise seen from above, as the rest of the world's meshes.
            self.mesh
                .indices
                .extend([index(t[0]), index(t[2]), index(t[1])]);
        }
        // The skirt: a flat quad a side, from the rim down to the depth under the sea.
        let bottom = sea - depth;
        for i in 0..n {
            let j = (i + 1) % n;
            let (a, b) = (rim[i], rim[j]);
            let out = Vec3::new(b.y - a.y, -(b.x - a.x), 0.0).normalize_or_zero();
            let ab = Vec3::new(a.x, a.y, bottom);
            let bb = Vec3::new(b.x, b.y, bottom);
            let k0 = self.push(a, out);
            let k1 = self.push(b, out);
            let k2 = self.push(bb, out);
            let k3 = self.push(ab, out);
            self.mesh.indices.extend([k0, k1, k2, k0, k2, k3]);
        }
    }
}

/// The numbers of the displacement of the top of a floe.
struct Displace {
    frequency: f32,
    min: f32,
    max: f32,
    exponent: f32,
}

fn displace_of(p: &IceParams) -> Displace {
    Displace {
        frequency: p.displace_frequency,
        min: p.displace_min,
        max: p.displace_max,
        exponent: p.displace_exponent,
    }
}

/// The cell under a point of the original's world.
fn nearest_cell(frame: &TerrainFrame, p: Vec2) -> (i32, i32) {
    let half = frame.half_world();
    let tex = Vec2::new(
        (p.x + half.x) / frame.terrain_scale().x,
        (p.y + half.y) / frame.terrain_scale().y,
    );
    frame.tex_to_hex(tex)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::terrain::TerrainKind;
    use crate::world::terrain::basis::PlotBasis;

    /// The numbers of the game's `IceSettings.xml` that matter here.
    pub(crate) fn params() -> IceParams {
        IceParams {
            near_samples: 16.0,
            far_samples: 48.0,
            lattice_verts_per_hex: 12.0,
            crack_min_clip: 0.05,
            crack_max_clip: 0.8,
            edge_pull_amount: 0.25,
            small_poly_shrink_amount: 0.35,
            small_poly_height_min: 0.6,
            small_poly_height_max: 0.6,
            large_poly_height: 10.0,
            large_poly_depth: 8.0,
            small_poly_depth_min: 2.0,
            small_poly_depth_max: 3.0,
            displace_frequency: 0.01,
            displace_min: -1.0,
            displace_max: 2.0,
            displace_exponent: 1.0,
            large_poly_edge_height_freq: 0.5,
            ..IceParams::default()
        }
    }

    fn polar() -> (TerrainBasis, TerrainFrame) {
        let water = PlotBasis {
            terrain: TerrainKind::Ocean,
            ..PlotBasis::default()
        };
        let mut b = TerrainBasis::uniform(6, 4, false, 1, water);
        for col in 0..6 {
            let i = b.index(col, 0).unwrap();
            b.plots[i].feature = BasisFeature::Ice;
        }
        // One ice hex on land: the original tests the feature only.
        let i = b.index(2, 2).unwrap();
        b.plots[i].terrain = TerrainKind::Snow;
        b.plots[i].feature = BasisFeature::Ice;
        (b, TerrainFrame::new(6, 4, false))
    }

    #[test]
    fn the_low_settings_go_with_a_terrain_detail_below_two() {
        let mut s = IceSettings::default();
        s.full.lattice_verts_per_hex = 12.0;
        s.low.lattice_verts_per_hex = 8.0;
        assert_eq!(params_for(&s, 3).lattice_verts_per_hex, 12.0);
        assert_eq!(params_for(&s, 2).lattice_verts_per_hex, 12.0);
        assert_eq!(params_for(&s, 1).lattice_verts_per_hex, 8.0);
        assert_eq!(params_for(&s, 0).lattice_verts_per_hex, 8.0);
    }

    #[test]
    fn only_the_ice_plots_get_floes_inside_the_box_of_heights() {
        let (b, f) = polar();
        let m = build_ice(&b, &f, &params(), 10.12, None);
        assert_eq!(m.cells.len(), 7);
        assert!(m.floes >= 7 * 6, "{}", m.floes);
        assert!(!m.is_empty());
        assert_eq!(m.indices.len() % 3, 0);
        assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
        for v in &m.vertices {
            assert!((-1.0..=1.0).contains(&v.v0[0]) && (-1.0..=1.0).contains(&v.v0[1]));
            assert!((0.0..=1.0).contains(&v.v1[3]));
            for c in &v.v1[..3] {
                assert!((0.0..=1.0).contains(c));
            }
        }
        // The top of a large floe lies LargePolyHeight above the sea (with the displacement), the
        // skirt goes below it.
        let z = |v: &IceVertex| v.v1[3] * m.height_scale + m.height_bias;
        let top = m.vertices.iter().map(z).fold(f32::MIN, f32::max);
        let low = m.vertices.iter().map(z).fold(f32::MAX, f32::min);
        assert!((top - (10.12 + 10.0 - 1.0)).abs() < 0.1, "{top}");
        assert!(low < 10.12, "{low}");
        // The low settings make fewer floes.
        let mut lowp = params();
        lowp.lattice_verts_per_hex = 8.0;
        assert!(build_ice(&b, &f, &lowp, 10.12, None).floes < m.floes);
    }

    #[test]
    fn a_map_without_ice_has_no_mesh() {
        let (mut b, f) = polar();
        for p in &mut b.plots {
            p.feature = BasisFeature::None;
        }
        assert!(build_ice(&b, &f, &params(), 10.12, None).is_empty());
    }
}
