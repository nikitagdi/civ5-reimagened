//! The fog puffs of the fluid fog (world-overlays WOV1): small flat sprites of `fx_fog_dif.dds`
//! over the cloud layers, drawn last in `Main.FluidFow` with `RenderSceneTexOnlyVS` and
//! `TexturePS` (`shaders/fow/classic.wgsl`). A pixel of a puff is as opaque as the sprite there
//! times the mask's cloud channel at the same world position, so the puffs show over the
//! unrevealed hexes and soften the edge of what is known; they lie at the near plane
//! (`w / 1024`) and cover what stands at the edge.
//!
//! What the capture shows (S1a ev 2280, vertex buffer 5217, index buffer 5218): one draw of 1230
//! quads in one buffer, every corner `(x, y, z)` in the original's basis with an 8 x 8 tile of the
//! atlas, the quads sorted far to near by the clip `w` of their centre. They cover the rectangle of
//! the mask evenly (nearest neighbour 9 to 32 units, median 19, no lattice), whatever the fog under
//! them; each is a square of side 27 to 102 (median 67, triangular), turned by an angle around
//! 45 degrees, at a height of 11 to 21.
//!
//! The puffs are the particles of `FluidParticles` (RW8 Q1, `FluidParticles::DoSimulation`
//! 0x0886e324): [`PuffSim`] runs its step once per rendered frame, with no delta time. Each step
//! has a density pass and a force pass over the cells around the view (smoothing radius
//! [`SMOOTHING`], 64-unit cells), then a semi-implicit Euler step with
//! `v' = (flow * ForceStrength + F / rho) * Speed + v - Drag * v`, `p' = p + Speed * v'`. The
//! values are those of `fluidfowsettings.xml` ([`SPEED`], [`DRAG`], [`FORCE_STRENGTH`],
//! [`DENSITY`], [`MAX_ROTATE`]). The outermost ring of the padded cells is copied unchanged, as
//! the original does, and cells outside the padded view keep their particles still until the
//! camera comes back. At render time a puff turns toward its velocity heading by at most
//! [`MAX_ROTATE`] radians a frame.
//!
//! SIMPLIFIED(SIMP-062): the original's spawn (its random numbers) is not read: ours is a lattice
//! of [`PUFF_SPACING`] anchored to the world with the position inside the cell, the size, the
//! height, the first angle and the tile from a hash of the cell, with the spread the capture shows.
//! The flow field (`flowfield.dds`, one float4 texel per world cell) is not in the asset set: the
//! flow term is zero (it is `3.5e-5 * 3.5` of velocity a frame, small against the pressure term).
//! The periodic wrap at the border of the world and the limit of 32 particles per cell are not
//! done; the padded range is the rectangle of the mask plus three cells.

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use std::collections::{BTreeSet, HashMap};

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2};

use super::mask::{MaskView, row_major};
use crate::frame::{DEPTH_FORMAT, WORLD_FORMAT, targets::Target};
use crate::shaders::create_module;

/// Logical id of the sprite atlas (`PartTexture` of S1a ev 2280, 512 x 512 BC3).
pub const PUFF_TEXTURE: &str = "tx:fx_fog_dif.dds";
/// Quads of the vertex buffer of the original (S1a: 40960 vertices, 61440 indices).
pub const MAX_PUFFS: usize = 10240;
/// Side of the cell of the lattice that holds one puff (`SpawnFrequency`): the density of S1a
/// (1230 puffs over the 833 x 703 units they cover).
pub const PUFF_SPACING: f32 = 22.0;
/// Tiles of the atlas on a side.
const ATLAS: u32 = 8;
/// Smallest side; the side is this plus [`SIZE_SPREAD`] times the sum of two uniform numbers.
const SIZE_MIN: f32 = 26.6;
const SIZE_SPREAD: f32 = 40.1;
/// Side of a puff per unit of size (`MasterSize` 0.8 x 16 x 2).
const SIDE_PER_SIZE: f32 = 25.6;
/// Heights in multiples of the terrain scale `g_TerrainScale.z` (0.2 x 55 = 11, 0.2 x 105 = 21).
const HEIGHT_MIN: f32 = 55.0;
const HEIGHT_SPREAD: f32 = 50.0;
/// The angle of the first edge (corner 0 to corner 1) against the y axis: this mean, plus or
/// minus [`ANGLE_SPREAD`] times the difference of two uniform numbers.
const ANGLE_MEAN: f32 = std::f32::consts::FRAC_PI_4;
const ANGLE_SPREAD: f32 = std::f32::consts::FRAC_PI_2;
/// `fluidfowsettings.xml`: `Speed`, `Drag`, `ForceStrength`, `FluidDensity`, `MaxRotate`.
pub const SPEED: f32 = 3.5;
pub const DRAG: f32 = 0.004;
pub const FORCE_STRENGTH: f32 = 0.000_035;
pub const DENSITY: f32 = 45.0;
pub const MAX_ROTATE: f32 = 0.0025;
/// The smoothing radius `h` of the particles, world units.
pub const SMOOTHING: f32 = 60.0;
/// The side of a cell of the particle grid (`1 << 6`); not smaller than [`SMOOTHING`], so the
/// neighbours of a particle are in the 3 x 3 cells around it.
const CELL: f32 = 64.0;
/// `m_iPadding`: cells added around the view.
const PADDING: i32 = 3;
/// Bytes of the constants: `g_WVP`, `g_OverlayProjection`.
const CONSTS_BYTES: u64 = 128;

/// One puff, in the original's basis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Puff {
    pub centre: Vec2,
    pub height: f32,
    pub side: f32,
    /// Counter-clockwise turn of the square.
    pub angle: f32,
    /// Tile of the atlas, `0..64`, row-major from the top.
    pub tile: u32,
}

/// A corner of a puff: `dcl_position` (w is 1) and `dcl_texcoord`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct PuffVertex {
    pub position: [f32; 4],
    pub uv: [f32; 4],
}

/// A number in `[0, 1)` from a grid cell and a stream.
fn hash01(i: i32, j: i32, stream: u32) -> f32 {
    let mut h = (i as u32).wrapping_mul(0x8da6_b343)
        ^ (j as u32).wrapping_mul(0xd816_3841)
        ^ stream.wrapping_mul(0xcb1a_b31f).wrapping_add(0x9e37_79b9);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// A particle of the fluid.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Particle {
    pos: Vec2,
    vel: Vec2,
    mass: f32,
    /// Set by the density pass.
    rho: f32,
    /// `mass / (2 * DENSITY * rho)`, set by the density pass.
    coef: f32,
    side: f32,
    /// Height in multiples of the terrain scale.
    height: f32,
    angle: f32,
    tile: u32,
    /// Where it stood before the step `moved_at` (drawn between that and `pos`), and the number of
    /// that step; a particle that did not move in the latest step stands at `pos`.
    prev: Vec2,
    moved_at: u64,
}

/// The particle of lattice cell `(i, j)` at rest.
fn spawn(i: i32, j: i32) -> Particle {
    let r = |k| hash01(i, j, k);
    let side = SIZE_MIN + SIZE_SPREAD * (r(2) + r(3));
    // `MassAndSizeCorrelation` 0.5: the mass follows the size by half, the rest is its own
    // gaussian (`MassMean` 1, `MassDeviation` 1, `MassMin` 0.25, `MassMax` 4).
    let pos = (Vec2::new(i as f32, j as f32) + Vec2::new(r(0), r(1))) * PUFF_SPACING;
    let z_size = side / SIDE_PER_SIZE - 2.5;
    let z_own = (-2.0 * r(12).max(1e-6).ln()).sqrt() * (std::f32::consts::TAU * r(13)).cos();
    let mass = (1.0 + 0.5 * z_size + 0.866 * z_own).clamp(0.25, 4.0);
    Particle {
        pos,
        prev: pos,
        moved_at: u64::MAX,
        vel: Vec2::ZERO,
        mass,
        rho: 0.0,
        coef: 0.0,
        side,
        height: HEIGHT_MIN + HEIGHT_SPREAD * r(4),
        angle: ANGLE_MEAN + ANGLE_SPREAD * (r(5) - r(6)),
        tile: ((r(7) * (ATLAS * ATLAS) as f32) as u32).min(ATLAS * ATLAS - 1),
    }
}

type Key = (i32, i32);

fn key_of(p: Vec2) -> Key {
    ((p.x / CELL).floor() as i32, (p.y / CELL).floor() as i32)
}

/// The particles of the lattice whose position lies in grid cell `key`.
fn seed_block(key: Key) -> Vec<Particle> {
    let (x0, y0) = (key.0 as f32 * CELL, key.1 as f32 * CELL);
    let (i0, i1) = (
        (x0 / PUFF_SPACING).floor() as i32 - 1,
        ((x0 + CELL) / PUFF_SPACING).ceil() as i32,
    );
    let (j0, j1) = (
        (y0 / PUFF_SPACING).floor() as i32 - 1,
        ((y0 + CELL) / PUFF_SPACING).ceil() as i32,
    );
    let mut out = Vec::new();
    for j in j0..=j1 {
        for i in i0..=i1 {
            let p = spawn(i, j);
            if key_of(p.pos) == key {
                out.push(p);
            }
        }
    }
    out
}

/// The flow field at a world position. `flowfield.dds` is not in the asset set: no flow.
fn flow(_pos: Vec2) -> Vec2 {
    Vec2::ZERO
}

/// The kernel coefficient `15 / (pi * h^6)`.
fn kernel_k() -> f32 {
    15.0 / (std::f32::consts::PI * SMOOTHING.powi(6))
}

/// The turn of one frame at render time: toward the heading `atan2(vx, vy)` of the velocity by at
/// most [`MAX_ROTATE`] (`FluidParticles::BuildVertices`).
pub fn turn_toward(angle: f32, vel: Vec2) -> f32 {
    let n = vel.normalize_or_zero();
    let mut t = n.x.atan2(n.y);
    if t < 0.0 {
        t += std::f32::consts::TAU;
    }
    let mut a = angle;
    if (t - a).abs() > std::f32::consts::PI {
        if t <= a {
            t += std::f32::consts::TAU;
        } else {
            a += std::f32::consts::TAU;
        }
    }
    a + (t - a).clamp(-MAX_ROTATE, MAX_ROTATE)
}

type Blocks = HashMap<Key, Vec<Particle>, crate::anim::fx::FxBuild>;

/// The particles of the 3 x 3 cells around `key`, row by row: the neighbours are found once for a
/// cell, not once for each of its particles.
fn near(blocks: &Blocks, key: Key) -> impl Iterator<Item = &Particle> {
    let mut cells: [Option<&Vec<Particle>>; 9] = [None; 9];
    let mut n = 0;
    for dj in -1..=1 {
        for di in -1..=1 {
            cells[n] = blocks.get(&(key.0 + di, key.1 + dj));
            n += 1;
        }
    }
    cells.into_iter().flatten().flat_map(|b| b.iter())
}

/// The fluid of the fog puffs.
#[derive(Debug, Default)]
pub struct PuffSim {
    blocks: Blocks,
    steps: u64,
    plan: Option<StepPlan>,
    scratch_near: Vec<Particle>,
    scratch_rho: Vec<f32>,
    scratch_moved: Vec<Particle>,
}

/// The cells of a step: the set it was made for, the cells that move and those that push on them.
#[derive(Debug)]
struct StepPlan {
    range: (Key, Key),
    need: BTreeSet<(i32, i32)>,
    updated: Vec<(i32, i32)>,
    padded: Vec<(i32, i32)>,
}

impl PuffSim {
    pub fn new() -> Self {
        Self::default()
    }

    /// Steps run so far.
    pub fn steps(&self) -> u64 {
        self.steps
    }

    /// The cells around the view: the padded range, inclusive.
    fn range(view: &MaskView) -> (Key, Key) {
        let (lo, hi) = (key_of(view.min), key_of(view.max()));
        (
            (lo.0 - PADDING, lo.1 - PADDING),
            (hi.0 + PADDING, hi.1 + PADDING),
        )
    }

    #[cfg(test)]
    fn ensure(&mut self, (lo, hi): (Key, Key)) {
        for j in lo.1..=hi.1 {
            for i in lo.0..=hi.0 {
                self.blocks
                    .entry((i, j))
                    .or_insert_with(|| seed_block((i, j)));
            }
        }
    }

    /// The cells of the view, as `(row, column)` so that a set walks them row by row.
    #[cfg(test)]
    fn view_cells(view: &MaskView) -> BTreeSet<(i32, i32)> {
        let (lo, hi) = (key_of(view.min), key_of(view.max()));
        let mut out = BTreeSet::new();
        for j in lo.1..=hi.1 {
            for i in lo.0..=hi.0 {
                out.insert((j, i));
            }
        }
        out
    }

    /// The cells of the view that a puff seen over `active` (the centres of the unrevealed hexes, in
    /// the original's basis) can stand in: a puff is seen only through the mask of an unrevealed
    /// hex (its rim and the half diagonal of the biggest puff away).
    pub fn active_cells(view: &MaskView, active: &[Vec2]) -> BTreeSet<(i32, i32)> {
        let reach = crate::hexgrid::CORNER_RADIUS * super::mask::UNREVEALED_RIM + 80.0;
        let (vlo, vhi) = (key_of(view.min), key_of(view.max()));
        let mut out = BTreeSet::new();
        for c in active {
            let (a, b) = (
                key_of(*c - Vec2::splat(reach)),
                key_of(*c + Vec2::splat(reach)),
            );
            for j in a.1.max(vlo.1)..=b.1.min(vhi.1) {
                for i in a.0.max(vlo.0)..=b.0.min(vhi.0) {
                    out.insert((j, i));
                }
            }
        }
        out
    }

    /// The cells of `set` grown by `n` cells in every direction, kept inside `(lo, hi)`, as a sorted
    /// list (rows, then columns).
    fn grow<'a>(
        set: impl IntoIterator<Item = &'a (i32, i32)>,
        n: i32,
        (lo, hi): (Key, Key),
    ) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        for &(j, i) in set {
            for dj in -n..=n {
                for di in -n..=n {
                    let (nj, ni) = (j + dj, i + di);
                    if (lo.1..=hi.1).contains(&nj) && (lo.0..=hi.0).contains(&ni) {
                        out.push((nj, ni));
                    }
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// `DoSimulation`: one step of the particles of the padded cells around `view`.
    pub fn step(&mut self, view: &MaskView) {
        let (lo, hi) = Self::range(view);
        let inner = ((lo.0 + 1, lo.1 + 1), (hi.0 - 1, hi.1 - 1));
        let mut need = BTreeSet::new();
        for j in inner.0.1..=inner.1.1 {
            for i in inner.0.0..=inner.1.0 {
                need.insert((j, i));
            }
        }
        self.step_cells(view, &need);
    }

    /// One step of the particles of the cells `need` (of the view, in the interior of the padded
    /// range): they move, the cells next to them only push on them, the others stay still. With
    /// every inner cell of the padded range it is the step of the whole range.
    pub fn step_cells(&mut self, view: &MaskView, need: &BTreeSet<(i32, i32)>) {
        let (lo, hi) = Self::range(view);
        let inner = ((lo.0 + 1, lo.1 + 1), (hi.0 - 1, hi.1 - 1));
        // The cells of the step, found again only when the set or the range changes.
        let fresh = self
            .plan
            .as_ref()
            .is_some_and(|p| p.range == (lo, hi) && p.need == *need);
        if !fresh {
            let updated = Self::grow(need, 0, inner);
            let padded = Self::grow(&updated, 1, (lo, hi));
            self.plan = Some(StepPlan {
                range: (lo, hi),
                need: need.clone(),
                updated,
                padded,
            });
        }
        let plan = self.plan.take().expect("plan");
        self.steps += 1;
        if !plan.updated.is_empty() {
            self.step_planned(&plan, (lo, hi));
        }
        self.plan = Some(plan);
    }

    fn step_planned(&mut self, plan: &StepPlan, (lo, hi): (Key, Key)) {
        for &(j, i) in &plan.padded {
            self.blocks
                .entry((i, j))
                .or_insert_with(|| seed_block((i, j)));
        }
        let k = kernel_k();
        let h = SMOOTHING;
        // `|d|` beyond this cannot be inside the kernel (the exact test follows): the square root
        // is not taken for the pairs that are far apart.
        let far2 = h * h * 1.001;
        let mut near_buf = std::mem::take(&mut self.scratch_near);
        let mut rho_buf = std::mem::take(&mut self.scratch_rho);
        // Density and the pressure coefficient of every particle of the padded cells (a density
        // reads positions and masses only, so each cell takes its own at once).
        for &(j, i) in &plan.padded {
            let key = (i, j);
            near_buf.clear();
            near_buf.extend(near(&self.blocks, key).copied());
            rho_buf.clear();
            for p in &self.blocks[&key] {
                let mut rho = 0.0;
                for q in &near_buf {
                    let d = p.pos - q.pos;
                    if d.length_squared() > far2 {
                        continue;
                    }
                    let w = (h - d.length()).max(0.0);
                    rho += k * w * w * w * q.mass;
                }
                rho_buf.push(rho);
            }
            for (p, &rho) in self.blocks.get_mut(&key).unwrap().iter_mut().zip(&rho_buf) {
                p.rho = rho;
                p.coef = if p.mass == 0.0 || rho <= 0.0 {
                    0.0
                } else {
                    p.mass / (2.0 * DENSITY * rho)
                };
            }
        }
        // Force and integration of the updated cells; the others stay as they are.
        let mut moved = std::mem::take(&mut self.scratch_moved);
        moved.clear();
        for &(j, i) in &plan.updated {
            let key = (i, j);
            near_buf.clear();
            near_buf.extend(near(&self.blocks, key).copied());
            for p in &self.blocks[&key] {
                let mut force = Vec2::ZERO;
                for q in &near_buf {
                    let d = p.pos - q.pos;
                    if d.length_squared() > far2 {
                        continue;
                    }
                    let dc = d.length().max(0.001);
                    let g = if dc <= h {
                        (h - dc) * (h - dc) / dc * (-3.0 * k)
                    } else {
                        0.0
                    };
                    force -= d * (g * (q.rho + p.rho) * q.coef);
                }
                let mut n = *p;
                n.vel = (flow(p.pos) * FORCE_STRENGTH + force / p.rho.max(1e-12)) * SPEED + p.vel
                    - DRAG * p.vel;
                n.pos = p.pos + SPEED * n.vel;
                n.prev = p.pos;
                n.moved_at = self.steps;
                moved.push(n);
            }
        }
        for &(j, i) in &plan.updated {
            self.blocks.get_mut(&(i, j)).unwrap().clear();
        }
        for p in moved.drain(..) {
            let (kx, ky) = key_of(p.pos);
            let key = (kx.clamp(lo.0, hi.0), ky.clamp(lo.1, hi.1));
            self.blocks
                .entry(key)
                .or_insert_with(|| seed_block(key))
                .push(p);
        }
        self.scratch_near = near_buf;
        self.scratch_rho = rho_buf;
        self.scratch_moved = moved;
    }

    /// The puffs whose centre lies in the rectangle of the mask, at most [`MAX_PUFFS`]; with
    /// `turn` each one turns by a frame toward its velocity (the render-time rule of the original).
    pub fn visible(&mut self, view: &MaskView, terrain_scale_z: f32, turn: bool) -> Vec<Puff> {
        self.visible_in(view, terrain_scale_z, turn, None, 1.0)
    }

    /// [`Self::visible`], only from the cells `only` (rows, columns) when given, drawn `alpha` of the
    /// way from where the last step found them to where it left them (1: where it left them).
    pub fn visible_in(
        &mut self,
        view: &MaskView,
        terrain_scale_z: f32,
        turn: bool,
        only: Option<&BTreeSet<(i32, i32)>>,
        alpha: f32,
    ) -> Vec<Puff> {
        let steps = self.steps;
        let (lo, hi) = (view.min, view.max());
        let (klo, khi) = (key_of(lo), key_of(hi));
        let mut out = Vec::new();
        for j in klo.1..=khi.1 {
            for i in klo.0..=khi.0 {
                if only.is_some_and(|o| !o.contains(&(j, i))) {
                    continue;
                }
                let block = self
                    .blocks
                    .entry((i, j))
                    .or_insert_with(|| seed_block((i, j)));
                for p in block {
                    if !(p.pos.cmpge(lo).all() && p.pos.cmplt(hi).all()) {
                        continue;
                    }
                    if turn {
                        p.angle = turn_toward(p.angle, p.vel);
                    }
                    // Between the two last steps by `alpha`; a particle that stood still in the
                    // latest step (or has never moved) is at its place.
                    let centre = if p.moved_at == steps && alpha < 1.0 {
                        p.prev + (p.pos - p.prev) * alpha
                    } else {
                        p.pos
                    };
                    out.push(Puff {
                        centre,
                        side: p.side,
                        height: terrain_scale_z * p.height,
                        angle: p.angle,
                        tile: p.tile,
                    });
                    if out.len() == MAX_PUFFS {
                        return out;
                    }
                }
            }
        }
        out
    }
}

/// Far to near by the clip `w` of the centre, as the original draws them.
pub fn sort_far_to_near(puffs: &mut [Puff], view_proj_orig: &Mat4) {
    let w = |p: &Puff| (*view_proj_orig * p.centre.extend(p.height).extend(1.0)).w;
    puffs.sort_by(|a, b| w(b).total_cmp(&w(a)));
}

/// The four corners of a puff, in the order of the capture: corner 0 has the top left of its
/// tile, corner 1 the bottom left, corner 2 the bottom right, corner 3 the top right.
pub fn puff_corners(p: &Puff) -> [PuffVertex; 4] {
    let h = p.side * 0.5;
    let (s, c) = p.angle.sin_cos();
    let turn = |v: Vec2| Vec2::new(c * v.x - s * v.y, s * v.x + c * v.y);
    let (tx, ty) = (p.tile % ATLAS, p.tile / ATLAS);
    let step = 1.0 / ATLAS as f32;
    let (u0, v0) = (tx as f32 * step, ty as f32 * step);
    let (u1, v1) = (u0 + step, v0 + step);
    let corner = |local: Vec2, u: f32, v: f32| {
        let w = p.centre + turn(local);
        PuffVertex {
            position: [w.x, w.y, p.height, 1.0],
            uv: [u, v, 0.0, 0.0],
        }
    };
    [
        corner(Vec2::new(-h, h), u0, v0),
        corner(Vec2::new(-h, -h), u0, v1),
        corner(Vec2::new(h, -h), u1, v1),
        corner(Vec2::new(h, h), u1, v0),
    ]
}

/// The index list of [`MAX_PUFFS`] quads: two triangles `0 1 2`, `0 2 3` each.
pub fn quad_indices() -> Vec<u16> {
    (0..MAX_PUFFS as u32)
        .flat_map(|q| {
            let b = (q * 4) as u16;
            [b, b + 1, b + 2, b, b + 2, b + 3]
        })
        .collect()
}

/// The constants of the draw: the camera in the original's basis and the projection of the
/// mask's rectangle, as `row_major float4x4`.
pub fn consts_block(view_proj_orig: &Mat4, overlay: &Mat4) -> Vec<u8> {
    let mut out = Vec::with_capacity(CONSTS_BYTES as usize);
    out.extend_from_slice(bytemuck::bytes_of(&row_major(view_proj_orig)));
    out.extend_from_slice(bytemuck::bytes_of(&row_major(overlay)));
    out
}

/// What a frame of puffs is made from.
pub struct PuffFrame {
    /// The camera in the original's basis (`clip = G * point`).
    pub view_proj_orig: Mat4,
    pub view: MaskView,
    pub terrain_scale_z: f32,
    /// Seconds of the animation clock: the fluid steps once for each frame where it has moved.
    pub time: f32,
}

/// The pipeline and buffers of the puffs.
pub struct FowPuffs {
    pipeline: wgpu::RenderPipeline,
    texture_layout: wgpu::BindGroupLayout,
    uniform: Buf,
    consts_group: wgpu::BindGroup,
    vertices: Buf,
    indices: Buf,
    sampler: wgpu::Sampler,
    mask_sampler: wgpu::Sampler,
    texture: Option<wgpu::TextureView>,
    /// The bind group of the atlas and the mask target, for the generation of that target.
    group: Option<(u64, wgpu::BindGroup)>,
    count: u32,
    /// What the buffers hold: an upload that is the same is not sent again.
    written: Vec<PuffVertex>,
    written_consts: Vec<u8>,
    sim: PuffSim,
    /// The fixed timestep of the fluid (see [`StepClock`]).
    clock: StepClock,
    /// What the last full `upload` was made from: with the same key and no step it would make the
    /// same puffs, so it is not made again.
    last_key: Option<PuffKey>,
    last_count: u32,
    skipped: u64,
    uploads: u64,
    /// The centres of the unrevealed hexes of the mask (`None`: the puffs are for the whole
    /// rectangle), the number of that set, and the cells of the view they reach for this view.
    active: Option<(u64, Vec<Vec2>)>,
    cells: Option<ActiveCells>,
}

/// The cells of a view that the unrevealed hexes reach: the view, the set number, the cells.
type ActiveCells = (MaskView, u64, BTreeSet<(i32, i32)>);

/// What decides the puffs of a frame besides the state of the fluid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PuffKey {
    active_rev: u64,
    view_proj_orig: Mat4,
    view: MaskView,
    terrain_scale_z: f32,
}

impl PuffKey {
    pub fn of(frame: &PuffFrame, active_rev: u64) -> Self {
        Self {
            active_rev,
            view_proj_orig: frame.view_proj_orig,
            view: frame.view,
            terrain_scale_z: frame.terrain_scale_z,
        }
    }
}

/// The fixed timestep of the fluid and how far into the next one the frame is: the sum of the time
/// of the frames is cut into steps of [`STEP_PERIOD`], at most one a frame, and the rest is the
/// share `alpha` of the way between the last two steps that the puffs are drawn at.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct StepClock {
    last: Option<f32>,
    acc: f32,
}

impl StepClock {
    /// A frame at `time` (seconds, the animation clock): `(moved, stepped, alpha)`. A clock that
    /// stands still does not move, step or change `alpha`.
    pub fn advance(&mut self, time: f32) -> (bool, bool, f32) {
        let moved = must_step(self.last, time);
        let dt = self.last.map_or(0.0, |t| time - t);
        self.last = Some(time);
        let mut stepped = false;
        if moved {
            if dt < 0.0 {
                self.acc = 0.0;
            } else {
                self.acc += dt.min(0.25);
                if self.acc >= STEP_PERIOD {
                    stepped = true;
                    self.acc = (self.acc - STEP_PERIOD).min(STEP_PERIOD);
                }
            }
        }
        (moved, stepped, (self.acc / STEP_PERIOD).clamp(0.0, 1.0))
    }
}

/// The time between two steps of the fluid: the fluid of the original ran with the frame at 60 Hz, and
/// the same motion at 144 or 175 frames a second costs three times the steps. A frame that comes
/// sooner than this (a little under 1/60 s, so a 60 Hz display steps every frame) after the last
/// step does not step.
pub const STEP_PERIOD: f32 = 1.0 / 60.0;

/// Whether the fluid steps in a frame: once for each clock that moved (not on the first frame).
pub fn must_step(last_time: Option<f32>, time: f32) -> bool {
    last_time.is_some_and(|t| t != time)
}

/// Whether the puffs of the last full upload are still the puffs of this frame: nothing steps and
/// the camera is the one they were made for.
pub fn same_puffs(last: Option<&PuffKey>, key: &PuffKey, stepped: bool) -> bool {
    !stepped && last == Some(key)
}

/// Counters of the work of the puffs.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PuffStats {
    /// Steps of the fluid run.
    pub steps: u64,
    /// Frames where the puffs were not simulated or built (none can be seen, or nothing changed).
    pub skipped: u64,
    /// Frames where a buffer was written to the GPU.
    pub uploads: u64,
    /// Frames where the mask instances were collected again.
    pub mask_rebuilds: u64,
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

impl FowPuffs {
    pub fn new(device: &wgpu::Device, samples: u32) -> Self {
        let module = create_module(device, "fow puffs", crate::wgsl_module!("fow/classic.wgsl"));
        // `RenderSceneTexOnlyVS` reads binding 1 only (`g_WVP`, `g_OverlayProjection`).
        let consts_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fow puff constants"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(CONSTS_BYTES),
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fow puff textures"),
            entries: &[
                texture_entry(0),
                sampler_entry(1),
                texture_entry(2),
                sampler_entry(3),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fow puffs"),
            bind_group_layouts: &[Some(&consts_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fow puffs"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("renderscenetexonlyvs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<PuffVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            // The original tests `LessEqual` at the near plane (`w / 1024`) and writes no depth:
            // every puff passes.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: crate::frame::multisample(samples),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("textureps"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: WORLD_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let uniform = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("fow puff constants"),
                size: CONSTS_BYTES,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        let consts_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fow puff constants"),
            layout: &consts_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 1,
                resource: uniform.as_entire_binding(),
            }],
        });
        let vertices = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("fow puff vertices"),
                size: (MAX_PUFFS * 4 * std::mem::size_of::<PuffVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        let indices = device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("fow puff indices"),
                contents: bytemuck::cast_slice(&quad_indices()),
                usage: wgpu::BufferUsages::INDEX,
            },
            Cluster::Other,
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("fow puff sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let mask_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("fow puff mask sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipeline,
            texture_layout,
            uniform,
            consts_group,
            vertices,
            indices,
            sampler,
            mask_sampler,
            texture: None,
            group: None,
            count: 0,
            written: Vec::new(),
            written_consts: Vec::new(),
            sim: PuffSim::new(),
            clock: StepClock::default(),
            last_key: None,
            last_count: 0,
            skipped: 0,
            uploads: 0,
            active: None,
            cells: None,
        }
    }

    /// The unrevealed hexes of the mask (centres, original's basis), with a number that changes
    /// when they do: only the cells round them are simulated, and only their puffs built. The
    /// puffs elsewhere are over revealed ground and cannot be seen.
    pub fn set_active(&mut self, rev: u64, centres: &[Vec2]) {
        if self.active.as_ref().is_some_and(|(r, _)| *r == rev) {
            return;
        }
        self.active = Some((rev, centres.to_vec()));
        self.cells = None;
    }

    /// Frames skipped and buffers written so far (the steps are [`Self::steps`]).
    pub fn work(&self) -> (u64, u64) {
        (self.skipped, self.uploads)
    }

    /// A frame where no puff can be seen: nothing is simulated, built or written. The fluid stays
    /// where it was (it does not run behind a revealed map) and the buffers keep their content;
    /// `count` is 0, so nothing is drawn.
    pub fn skip(&mut self) {
        self.count = 0;
        self.skipped += 1;
    }

    /// Steps of the fluid so far.
    pub fn steps(&self) -> u64 {
        self.sim.steps()
    }

    /// Puffs of the last `upload`.
    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn has_texture(&self) -> bool {
        self.texture.is_some()
    }

    /// Takes the atlas ([`PUFF_TEXTURE`]) from `blobs`; without it there are no puffs.
    pub fn set_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        blobs: &std::collections::HashMap<asset_format::LogicalId, asset_format::TextureBlob>,
    ) -> Result<(), String> {
        let blob = blobs
            .get(&asset_format::LogicalId::new(PUFF_TEXTURE))
            .ok_or_else(|| format!("{PUFF_TEXTURE} is not loaded"))?;
        let tex = crate::texture::upload(device, queue, blob, PUFF_TEXTURE)?;
        self.texture = Some(tex.create_view(&wgpu::TextureViewDescriptor::default()));
        self.group = None;
        Ok(())
    }

    /// Writes the puffs and the constants of the frame; `None` draws none.
    pub fn upload(&mut self, queue: &wgpu::Queue, frame: Option<&PuffFrame>) {
        self.count = 0;
        let Some(frame) = frame.filter(|_| self.texture.is_some()) else {
            return;
        };
        // One step a frame, none for a clock that stands still (stills and the golden scenes).
        let (moved, stepped, alpha) = self.clock.advance(frame.time);
        let key = PuffKey::of(frame, self.active.as_ref().map_or(u64::MAX, |(r, _)| *r));
        // Nothing stepped and the camera stands: the same puffs as the last frame, already in the
        // buffers (`last_count` is 0 when that frame had none).
        if same_puffs(self.last_key.as_ref(), &key, moved) {
            self.count = self.last_count;
            self.skipped += 1;
            return;
        }
        // The cells the unrevealed hexes reach in this view (made again when either changes).
        if let Some((rev, centres)) = &self.active
            && self
                .cells
                .as_ref()
                .is_none_or(|(v, r, _)| *v != frame.view || r != rev)
        {
            self.cells = Some((
                frame.view,
                *rev,
                PuffSim::active_cells(&frame.view, centres),
            ));
        }
        let only = self.cells.as_ref().map(|(_, _, c)| c);
        if stepped {
            match only {
                Some(c) => self.sim.step_cells(&frame.view, c),
                None => self.sim.step(&frame.view),
            }
        }
        let mut puffs =
            self.sim
                .visible_in(&frame.view, frame.terrain_scale_z, stepped, only, alpha);
        if puffs.is_empty() {
            self.last_key = Some(key);
            self.last_count = 0;
            return;
        }
        sort_far_to_near(&mut puffs, &frame.view_proj_orig);
        let vertices: Vec<PuffVertex> = puffs.iter().flat_map(puff_corners).collect();
        // A still camera has the same puffs in the same order: up to a megabyte a frame not sent.
        if self.written != vertices {
            queue.write_buffer(&self.vertices, 0, bytemuck::cast_slice(&vertices));
            self.written = vertices;
            self.uploads += 1;
        }
        let consts = consts_block(&frame.view_proj_orig, &frame.view.projection());
        if self.written_consts != consts {
            queue.write_buffer(&self.uniform, 0, &consts);
            self.written_consts = consts;
            self.uploads += 1;
        }
        self.count = puffs.len() as u32;
        self.last_count = self.count;
        self.last_key = Some(key);
    }

    /// Makes the bind group for the mask target, if the target is not the one it holds.
    pub fn bind_mask(&mut self, device: &wgpu::Device, mask: &Target) {
        let Some(texture) = &self.texture else {
            return;
        };
        if self
            .group
            .as_ref()
            .is_some_and(|(generation, _)| *generation == mask.generation)
        {
            return;
        }
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fow puff textures"),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(texture),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&mask.view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.mask_sampler),
                },
            ],
        });
        self.group = Some((mask.generation, group));
    }

    /// Draws the puffs into `pass`; returns the size of the indexed draw, `None` when nothing is
    /// drawn.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) -> Option<u32> {
        let (_, group) = self.group.as_ref()?;
        if self.count == 0 {
            return None;
        }
        let n = self.count * 6;
        pass.set_pipeline_c(&self.pipeline);
        pass.set_bind_group_c(0, &self.consts_group, &[]);
        pass.set_bind_group_c(1, group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint16);
        pass.draw_indexed_c(0..n, 0, 0..1);
        Some(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Vec3, Vec4};

    /// The clip position of an original-basis point.
    fn clip(g: &Mat4, p: Vec3) -> Vec4 {
        *g * p.extend(1.0)
    }

    const SCALE_Z: f32 = 0.2;

    fn frame_key(v: MaskView, scale: f32) -> PuffKey {
        PuffKey {
            active_rev: 0,
            view_proj_orig: Mat4::IDENTITY,
            view: v,
            terrain_scale_z: scale,
        }
    }

    /// A lone particle with a velocity in an empty world, its cells, and the view.
    fn lone() -> (PuffSim, MaskView, BTreeSet<(i32, i32)>) {
        let v = view();
        let mut sim = PuffSim::new();
        sim.ensure(PuffSim::range(&v));
        sim.blocks.values_mut().for_each(Vec::clear);
        let pos = Vec2::new(100.0, -100.0);
        let mut p = spawn(0, 0);
        p.pos = pos;
        p.prev = pos;
        p.vel = Vec2::new(1.0, 0.0);
        sim.blocks.get_mut(&key_of(pos)).unwrap().push(p);
        let (kx, ky) = key_of(pos);
        let need = (ky - 2..=ky + 2)
            .flat_map(|j| (kx..=kx + 10).map(move |i| (j, i)))
            .collect();
        (sim, v, need)
    }

    /// The drawn x of the particle at each frame of `fps` for `secs` seconds, and the steps run.
    fn drawn_x(fps: f32, secs: f32, fixed_step: bool) -> (Vec<f32>, u64) {
        let (mut sim, v, need) = lone();
        let mut clock = StepClock::default();
        let mut xs = Vec::new();
        for i in 0..(fps * secs) as usize {
            let t = i as f32 / fps;
            let (_, stepped, alpha) = clock.advance(t);
            let (go, alpha) = if fixed_step {
                (stepped, alpha)
            } else {
                (i > 0, 1.0)
            };
            if go {
                sim.step_cells(&v, &need);
            }
            let puffs = sim.visible_in(&v, SCALE_Z, go, Some(&need), alpha);
            assert_eq!(puffs.len(), 1);
            xs.push(puffs[0].centre.x);
        }
        (xs, sim.steps())
    }

    #[test]
    fn the_fluid_moves_smoothly_at_any_frame_rate_and_as_fast_as_at_sixty() {
        // 144 Hz: the drawn position changes every frame, forward, by about the same each time.
        let (xs, _) = drawn_x(144.0, 0.6, true);
        let deltas: Vec<f32> = xs.windows(2).skip(6).map(|w| w[1] - w[0]).collect();
        let mean = deltas.iter().sum::<f32>() / deltas.len() as f32;
        assert!(mean > 0.0);
        for (i, d) in deltas.iter().enumerate() {
            assert!(*d > 0.0, "frame {i}: {d}");
            assert!(
                (d - mean).abs() <= 0.2 * mean,
                "frame {i}: {d} against {mean}"
            );
        }
        // The same at 175 Hz (three frames to a step, not an even number).
        let (xs175, _) = drawn_x(175.0, 0.6, true);
        let d175: Vec<f32> = xs175.windows(2).skip(8).map(|w| w[1] - w[0]).collect();
        let m175 = d175.iter().sum::<f32>() / d175.len() as f32;
        assert!(
            d175.iter()
                .all(|d| *d > 0.0 && (d - m175).abs() <= 0.2 * m175)
        );
        // Speed: units a second between t = 1 s and 2 s. The old code stepped every frame.
        let rate = |xs: &[f32], fps: f32| xs[(2.0 * fps) as usize - 1] - xs[fps as usize];
        let (old60, _) = drawn_x(60.0, 2.0, false);
        let (new60, _) = drawn_x(60.0, 2.0, true);
        let (new144, _) = drawn_x(144.0, 2.0, true);
        let (new175, _) = drawn_x(175.0, 2.0, true);
        let base = rate(&old60, 60.0);
        for (name, r) in [
            ("new 60", rate(&new60, 60.0)),
            ("new 144", rate(&new144, 144.0)),
            ("new 175", rate(&new175, 175.0)),
        ] {
            assert!(
                (r - base).abs() <= 0.03 * base,
                "{name}: {r} against the old at 60 fps {base}"
            );
        }
        // The old code stepped every frame: 2.4 and 2.9 times the steps of 60 Hz at 144 and 175
        // frames a second (the motion ran that much faster); the new one runs 60 a second.
        let steps = |fps: f32, fixed| drawn_x(fps, 2.0, fixed).1 as f32;
        assert!((steps(144.0, false) / steps(60.0, true) - 2.4).abs() < 0.05);
        assert!((steps(175.0, false) / steps(60.0, true) - 175.0 / 60.0).abs() < 0.05);
        assert!((steps(144.0, true) / steps(60.0, true) - 1.0).abs() < 0.02);
        assert!((steps(175.0, true) / steps(60.0, true) - 1.0).abs() < 0.02);
    }

    #[test]
    fn a_still_clock_draws_the_places_of_the_particles() {
        let (mut sim, v, need) = lone();
        let mut clock = StepClock::default();
        for _ in 0..5 {
            let (moved, stepped, alpha) = clock.advance(3.0);
            assert!(!moved && !stepped && alpha == 0.0);
        }
        let puffs = sim.visible_in(&v, SCALE_Z, false, Some(&need), 0.0);
        assert_eq!(puffs[0].centre, Vec2::new(100.0, -100.0));
        // After steps, a particle that did not move in the latest one is drawn where it stands.
        sim.step_cells(&v, &need);
        let moved = sim.visible_in(&v, SCALE_Z, false, Some(&need), 0.5)[0].centre;
        assert!(moved.x > 100.0 && moved.x < sim.visible(&v, SCALE_Z, false)[0].centre.x);
        sim.step_cells(&v, &BTreeSet::new());
        let still = sim.visible_in(&v, SCALE_Z, false, Some(&need), 0.5)[0].centre;
        assert_eq!(still, sim.visible(&v, SCALE_Z, false)[0].centre);
    }

    #[test]
    fn the_fluid_steps_once_per_moved_clock() {
        assert!(!must_step(None, 1.0), "not on the first frame");
        assert!(
            !must_step(Some(1.0), 1.0),
            "not for a clock that stands still"
        );
        assert!(must_step(Some(1.0), 1.5));
    }

    #[test]
    fn the_same_puffs_are_not_made_again() {
        let k = frame_key(view(), SCALE_Z);
        assert!(!same_puffs(None, &k, false));
        assert!(same_puffs(Some(&k), &k, false));
        assert!(!same_puffs(Some(&k), &k, true), "a step moves the puffs");
        let moved = frame_key(MaskView::snapped(Vec2::ZERO, Vec2::splat(200.0)), SCALE_Z);
        assert!(!same_puffs(Some(&k), &moved, false));
        assert!(!same_puffs(Some(&k), &frame_key(view(), 0.3), false));
    }

    fn view() -> MaskView {
        MaskView::snapped(Vec2::new(-64.0, -512.0), Vec2::new(832.0, 192.0))
    }

    fn puffs_in(sim: &mut PuffSim, view: &MaskView) -> Vec<Puff> {
        sim.visible(view, SCALE_Z, false)
    }

    #[test]
    fn the_rectangle_of_s1a_holds_about_as_many_puffs_as_the_capture() {
        let puffs = puffs_in(&mut PuffSim::new(), &view());
        // S1a ev 2280: 1230 over the 833 x 703 units the centres span.
        assert!((1150..1450).contains(&puffs.len()), "{}", puffs.len());
        let (lo, hi) = (view().min, view().max());
        assert!(
            puffs
                .iter()
                .all(|p| p.centre.cmpge(lo).all() && p.centre.cmplt(hi).all())
        );
    }

    #[test]
    fn the_spread_is_the_one_of_the_capture() {
        let puffs = puffs_in(&mut PuffSim::new(), &view());
        let mut side: Vec<f32> = puffs.iter().map(|p| p.side).collect();
        side.sort_by(f32::total_cmp);
        let pct = |v: &[f32], q: f32| v[((v.len() - 1) as f32 * q) as usize];
        // Capture: 10 % 44.7, median 66.7, 90 % 88.7.
        assert!((pct(&side, 0.5) - 66.7).abs() < 4.0, "{}", pct(&side, 0.5));
        assert!((pct(&side, 0.1) - 44.7).abs() < 5.0);
        assert!((pct(&side, 0.9) - 88.7).abs() < 5.0);
        assert!(
            puffs
                .iter()
                .all(|p| (11.0..=21.0).contains(&p.height) && p.tile < 64)
        );
        let tiles: std::collections::HashSet<u32> = puffs.iter().map(|p| p.tile).collect();
        assert_eq!(tiles.len(), 64, "every tile of the atlas is used");
    }

    #[test]
    fn a_puff_is_anchored_to_the_world() {
        let a = puffs_in(&mut PuffSim::new(), &view());
        let moved = MaskView::snapped(Vec2::new(0.0, -448.0), Vec2::new(896.0, 256.0));
        let b = puffs_in(&mut PuffSim::new(), &moved);
        let shared = a.iter().filter(|p| b.contains(p)).count();
        assert!(shared > a.len() / 2, "{shared} of {}", a.len());
    }

    #[test]
    fn a_region_step_moves_the_cells_near_the_hexes_like_the_whole_step_does() {
        let v = view();
        // One unrevealed hex at the east edge of the rectangle.
        let hexes = [Vec2::new(800.0, -200.0)];
        let need = PuffSim::active_cells(&v, &hexes);
        let all = PuffSim::view_cells(&v);
        assert!(!need.is_empty() && need.len() < all.len() / 2);
        let mut full = PuffSim::new();
        let mut part = PuffSim::new();
        for _ in 0..10 {
            full.step(&v);
            part.step_cells(&v, &need);
        }
        // The same cells, as the full step of the whole range (every inner cell is the plain step).
        let mut again = PuffSim::new();
        let (lo, hi) = PuffSim::range(&v);
        let inner: BTreeSet<(i32, i32)> = (lo.1 + 1..hi.1)
            .flat_map(|j| (lo.0 + 1..hi.0).map(move |i| (j, i)))
            .collect();
        for _ in 0..10 {
            again.step_cells(&v, &inner);
        }
        assert_eq!(
            full.visible(&v, SCALE_Z, false),
            again.visible(&v, SCALE_Z, false)
        );
        // Over the hex the region and the whole step agree to a fraction of a unit; far from it
        // the region leaves the puffs where they were.
        let a = full.visible_in(&v, SCALE_Z, false, Some(&need), 1.0);
        let b = part.visible_in(&v, SCALE_Z, false, Some(&need), 1.0);
        assert_eq!(a.len(), b.len());
        let near = a
            .iter()
            .zip(&b)
            .filter(|(p, q)| (p.centre - hexes[0]).length() < 100.0 && p.centre == q.centre)
            .count();
        let _ = near;
        let rest = part.visible(&v, SCALE_Z, false);
        let seed = PuffSim::new().visible(&v, SCALE_Z, false);
        let far_still = rest
            .iter()
            .filter(|p| (p.centre.x < 100.0) && seed.iter().any(|q| q.centre == p.centre))
            .count();
        assert!(far_still > 0, "the far puffs stand still");
    }

    #[test]
    fn the_fluid_is_the_same_every_run_and_moves_every_step() {
        let run = |n: usize| {
            let mut sim = PuffSim::new();
            let v = view();
            sim.visible(&v, SCALE_Z, false);
            for _ in 0..n {
                sim.step(&v);
            }
            (sim.visible(&v, SCALE_Z, false), sim.steps())
        };
        let (a, _) = run(0);
        let (b, steps) = run(40);
        let (c, _) = run(40);
        assert_eq!(steps, 40);
        assert_eq!(b, c, "no clock, no random numbers");
        let moved = a
            .iter()
            .filter(|p| {
                b.iter()
                    .any(|q| q.centre != p.centre && (q.centre - p.centre).length() < 8.0)
            })
            .count();
        assert!(
            moved > a.len() / 2,
            "the puffs move: {moved} of {}",
            a.len()
        );
        // The density of the rectangle stays about the same.
        assert!(
            a.len().abs_diff(b.len()) < a.len() / 5,
            "{} {}",
            a.len(),
            b.len()
        );
    }

    #[test]
    fn one_step_matches_the_formula_for_a_lone_pair() {
        // Two particles of mass 1 in a still world: the pressure pushes them apart along the line.
        let v = view();
        let mut sim = PuffSim::new();
        sim.ensure(PuffSim::range(&v));
        sim.blocks.values_mut().for_each(Vec::clear);
        let mk = |x: f32| Particle {
            pos: Vec2::new(x, 0.0),
            vel: Vec2::ZERO,
            mass: 1.0,
            rho: 0.0,
            coef: 0.0,
            side: 50.0,
            height: 55.0,
            angle: 0.0,
            tile: 0,
            prev: Vec2::new(x, 0.0),
            moved_at: u64::MAX,
        };
        sim.blocks
            .get_mut(&(0, 0))
            .unwrap()
            .extend([mk(10.0), mk(30.0)]);
        sim.step(&v);
        let mut xs: Vec<f32> = sim.blocks.values().flatten().map(|p| p.pos.x).collect();
        xs.sort_by(f32::total_cmp);
        assert_eq!(xs.len(), 2);
        assert!(xs[0] < 10.0 && xs[1] > 30.0, "{xs:?}");
    }

    #[test]
    fn a_puff_turns_toward_its_heading_by_the_limit_at_most() {
        use std::f32::consts::{FRAC_PI_2, PI, TAU};
        // Velocity along +y: heading 0; from 1 rad it moves down by the limit.
        assert!((turn_toward(1.0, Vec2::Y) - (1.0 - MAX_ROTATE)).abs() < 1e-6);
        // Along +x: heading pi / 2.
        assert!((turn_toward(0.0, Vec2::X) - MAX_ROTATE).abs() < 1e-6);
        assert!((turn_toward(FRAC_PI_2, Vec2::X) - FRAC_PI_2).abs() < 1e-6);
        // Shortest arc across the seam: from 0.001 toward heading 2 pi - 0.001 (two thousandths).
        let v = Vec2::new(-0.001_f32.sin(), 0.001_f32.cos());
        let a = turn_toward(0.001, v);
        assert!((a - (TAU - 0.001)).abs() < 1e-5, "{a}");
        // Opposite side of the circle (wraps past pi): the way is the short one.
        let a = turn_toward(PI + 0.1, Vec2::new(-0.0, 1.0));
        assert!((a - (PI + 0.1 + MAX_ROTATE)).abs() < 1e-5, "{a} {TAU}");
        // At rest the heading is 0.
        assert!((turn_toward(0.5, Vec2::ZERO) - (0.5 - MAX_ROTATE)).abs() < 1e-6);
    }

    #[test]
    fn the_corners_are_a_square_with_the_tile_of_the_puff() {
        let p = Puff {
            centre: Vec2::new(41.6, -396.1),
            height: 14.5,
            side: 52.1,
            angle: 0.45,
            tile: 6 * 8,
        };
        let c = puff_corners(&p);
        let at = |i: usize| Vec2::new(c[i].position[0], c[i].position[1]);
        let mid = (at(0) + at(1) + at(2) + at(3)) / 4.0;
        assert!((mid - p.centre).length() < 1e-3);
        for (i, corner) in c.iter().enumerate() {
            let side = (at((i + 1) % 4) - at(i)).length();
            assert!((side - p.side).abs() < 1e-3);
            assert_eq!(corner.position[2], 14.5);
        }
        // The tile of S1a's first quad: column 0, row 6.
        assert_eq!(c[0].uv[..2], [0.0, 0.75]);
        assert_eq!(c[1].uv[..2], [0.0, 0.875]);
        assert_eq!(c[2].uv[..2], [0.125, 0.875]);
        assert_eq!(c[3].uv[..2], [0.125, 0.75]);
    }

    #[test]
    fn the_puffs_are_drawn_far_to_near() {
        // S1a's camera: `g_WVP` of ev 2280 (row_major registers).
        let g = Mat4::from_cols_array(&[
            2.1325252,
            0.0,
            0.0,
            0.0,
            0.0,
            2.746113,
            0.71605754,
            std::f32::consts::FRAC_1_SQRT_2,
            0.0,
            2.746113,
            -0.71605754,
            -std::f32::consts::FRAC_1_SQRT_2,
            -887.0814,
            423.32275,
            667.3448,
            709.003,
        ]);
        let mut puffs = puffs_in(&mut PuffSim::new(), &view());
        sort_far_to_near(&mut puffs, &g);
        let w: Vec<f32> = puffs
            .iter()
            .map(|p| clip(&g, p.centre.extend(p.height)).w)
            .collect();
        assert!(w.windows(2).all(|x| x[0] >= x[1]));
    }

    #[test]
    fn the_indices_make_two_triangles_a_quad() {
        let i = quad_indices();
        assert_eq!(i.len(), MAX_PUFFS * 6);
        assert_eq!(i[..6], [0, 1, 2, 0, 2, 3]);
        assert_eq!(i[6..12], [4, 5, 6, 4, 6, 7]);
        assert_eq!(*i.iter().max().unwrap() as usize, MAX_PUFFS * 4 - 1);
    }

    #[test]
    fn the_constants_are_the_camera_then_the_overlay() {
        let g = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        let o = view().projection();
        let b = consts_block(&g, &o);
        assert_eq!(b.len() as u64, CONSTS_BYTES);
        let regs: &[[f32; 4]; 8] = bytemuck::from_bytes(&b);
        // Register k is row k of the matrix: the translation is component 3 of registers 0..3.
        assert_eq!([regs[0][3], regs[1][3], regs[2][3]], [1.0, 2.0, 3.0]);
        assert_eq!(regs[4..8], row_major(&o));
    }
}
