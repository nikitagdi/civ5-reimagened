//! The house lots of a city (world-objects 4.4, WOB5): a function-by-function port of
//! `cvCityBuildingLayoutGen`, a particle relaxation.
//!
//! re: `InitializePopBuildingPlacementSet` (0x08a034d8), `SolveForFinalPlacements` (0x08a036f0),
//! `Tick` (0x08a0374c), `FinalizePlacementSet` (0x08a040c6), the constants of the constructor
//! (0x087b9bc0) and the mask rasterisers of `GeneratePopBuildingPlacements` (0x087b73a4:
//! `FUN_087bb7c0`, `FUN_087bb886`).
//!
//! Particles are seeded on a lattice turned by 45 degrees over the 128-unit square of the city,
//! pushed apart for 40 ticks under the distance of the tensor field, and pushed off the cells of
//! a blocked mask (the wall line, the wonders). The solver draws no random numbers.
//!
//! Positions are in the original's world plane (x east, y up the rows: ours `(X, -Z)`).

use glam::Vec2;

use super::fields::{TENSOR_SIDE, TensorField};

/// Side of the blocked mask and of the layout square.
pub const MASK_SIDE: usize = 128;

/// Hash bins of 4 units, 32 a side.
const BINS: usize = 32;
const NO_PARTICLE: u16 = 0xffff;

/// The force class of a wall cell and of a wonder cell in the mask.
pub const MASK_WALL: u8 = 2;
pub const MASK_WONDER: u8 = 3;

/// `(r0, range, A, B)` of a force: type 0 (particle on particle) and the mask classes 1..3.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForceProfile {
    pub radius: f32,
    pub range: f32,
    pub strength_a: f32,
    pub strength_b: f32,
}

const fn profile(radius: f32, range: f32, strength_a: f32) -> ForceProfile {
    ForceProfile {
        radius,
        range,
        strength_a,
        strength_b: 0.0,
    }
}

/// The table of the constructor (0x087b9bc0).
pub const PROFILES: [ForceProfile; 4] = [
    profile(5.0, 2.0, 0.1),
    profile(2.0, 2.0, 0.3),
    profile(4.0, 2.0, 0.4),
    profile(2.0, 2.0, 0.2),
];
pub const SPACING: f32 = 6.0;
pub const MIN_NEIGHBOUR_DISTANCE: f32 = 5.0;
pub const HASH_RADIUS: i32 = 2;
pub const MASK_RADIUS: i32 = 4;

/// An axis-aligned rectangle of the world plane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub min: Vec2,
    pub max: Vec2,
}

impl Rect {
    fn contains(&self, p: Vec2) -> bool {
        self.min.x <= p.x && self.max.x > p.x && self.min.y <= p.y && self.max.y > p.y
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Particle {
    free: bool,
    pos: Vec2,
    tensor: Vec2,
    force: Vec2,
    hash_next: u16,
}

/// A lot as the solver leaves it: the place and the heading vector (the tensor at the place, not
/// normalised).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub index: usize,
    pub pos: Vec2,
    pub dir: Vec2,
}

/// The ticks of `SolveForFinalPlacements`.
pub const SOLVE_TICKS: u32 = 40;

/// The solver of one city.
pub struct LayoutGen {
    rect: Rect,
    /// `m_afFrameMatrix`, identity.
    frame: [f32; 4],
    tensor: TensorField,
    /// The frame of the tensor field: the layout rect, `63 / span` cells per unit.
    tensor_scale: Vec2,
    heads: [[u16; BINS]; BINS],
    /// The blocked mask, row-major, rect-local cells.
    pub mask: Box<[u8; MASK_SIDE * MASK_SIDE]>,
    particles: Vec<Particle>,
}

/// `forceMagnitude` of `Tick`.
fn force_magnitude(p: &ForceProfile, distance: f32) -> f32 {
    let band = (distance - p.radius) / p.range;
    let t = if band < 1.0 { band } else { 1.0 };
    let t = if 0.0 > t { 0.0 } else { t };
    let u = 1.0 - t;
    let outer = ((u * 3.0 + t) * (t * t)) * p.strength_b;
    let inner = ((t * 3.0 + u) * (u * u)) * p.strength_a;
    inner + outer
}

/// `interpolateChannel`: two truncations to a signed byte, then the vertical weighting.
fn interpolate_channel(c00: i8, c01: i8, c10: i8, c11: i8, fx: f32, fy: f32) -> i8 {
    let top = f32::from((f32::from(c00) * (1.0 - fx) + f32::from(c01) * fx) as i32 as i8);
    let bottom = f32::from((f32::from(c10) * (1.0 - fx) + f32::from(c11) * fx) as i32 as i8);
    (top * (1.0 - fy) + bottom * fy) as i32 as i8
}

impl LayoutGen {
    /// `InitializePopBuildingPlacementSet`: the lattice over the rect, the tensor field placed on it.
    pub fn new(rect: Rect, tensor: &TensorField) -> Self {
        let span = rect.max - rect.min;
        let mut g = Self {
            rect,
            frame: [1.0, 0.0, 0.0, 1.0],
            tensor: tensor.clone(),
            tensor_scale: Vec2::new(
                (TENSOR_SIDE - 1) as f32 / span.x,
                (TENSOR_SIDE - 1) as f32 / span.y,
            ),
            heads: [[NO_PARTICLE; BINS]; BINS],
            mask: Box::new([0; MASK_SIDE * MASK_SIDE]),
            particles: Vec::new(),
        };
        let side = (181.019_33f32 / SPACING) as i32;
        if side <= 0 {
            return g;
        }
        let step_x = 128.0 / side as f32;
        let step_y = -128.0 / side as f32;
        for i in 0..side {
            let base = i as f32 * step_x;
            for j in 0..side {
                let x = step_x * j as f32 + (base + -64.0);
                let y = j as f32 * step_y + (base + 64.0);
                let bx = (x * 0.25) as i32;
                let by = (y * 0.25) as i32;
                if (bx | by) < 0 || bx > 0x1f || by > 0x1f {
                    continue;
                }
                let index = g.particles.len() as u16;
                g.particles.push(Particle {
                    free: false,
                    pos: Vec2::new(x + rect.min.x, y + rect.min.y),
                    tensor: Vec2::ZERO,
                    force: Vec2::ZERO,
                    hash_next: g.heads[by as usize][bx as usize],
                });
                g.heads[by as usize][bx as usize] = index;
            }
        }
        g
    }

    /// The number of particles still alive.
    pub fn live(&self) -> usize {
        self.particles.iter().filter(|p| !p.free).count()
    }

    fn tensor_in_frame(&self, p: Vec2) -> bool {
        self.rect.contains(p)
    }

    /// The tensor at a world point (bilinear on the cells with the fraction of the world
    /// coordinate, as the original does), divided by 128.
    fn sample_tensor(&self, p: Vec2) -> Vec2 {
        let row = ((p.y - self.rect.min.y) * self.tensor_scale.y) as usize;
        let col = ((p.x - self.rect.min.x) * self.tensor_scale.x) as usize;
        let fx = p.x - p.x.floor();
        let fy = p.y - p.y.floor();
        let t = &self.tensor;
        let (c00, c01, c10, c11) = (
            t.cell(col, row),
            t.cell(col + 1, row),
            t.cell(col, row + 1),
            t.cell(col + 1, row + 1),
        );
        let x = interpolate_channel(c00[0], c01[0], c10[0], c11[0], fx, fy);
        let y = interpolate_channel(c00[1], c01[1], c10[1], c11[1], fx, fy);
        Vec2::new(f32::from(x), f32::from(y)) * (1.0 / 128.0)
    }

    /// `Tick(flags)`: bit 0 prunes close neighbours, bit 1 kills particles on a blocked cell.
    pub fn tick(&mut self, flags: u8) {
        let prune = flags & 1 != 0;
        let kill_on_mask = flags & 2 != 0;
        let count = self.particles.len();
        if count == 0 {
            self.heads = [[NO_PARTICLE; BINS]; BINS];
            return;
        }
        let min = self.rect.min;
        for index in 0..count {
            if self.particles[index].free {
                continue;
            }
            let pos = self.particles[index].pos;
            if !self.rect.contains(pos) {
                self.particles[index].free = true;
                continue;
            }
            let bin_x = ((pos.x - min.x) * 0.25) as i32;
            let bin_y = ((pos.y - min.y) * 0.25) as i32;
            let x0 = (bin_x - HASH_RADIUS).max(0);
            let y0 = (bin_y - HASH_RADIUS).max(0);
            let x_end = if bin_x + HASH_RADIUS + 1 >= 32 {
                0x1f
            } else {
                bin_x + HASH_RADIUS + 1
            };
            let y_end = if bin_y + HASH_RADIUS + 1 >= 32 {
                0x1f
            } else {
                bin_y + HASH_RADIUS + 1
            };
            for row in y0..y_end {
                for col in x0..x_end {
                    let mut other = self.heads[row as usize][col as usize];
                    while other != NO_PARTICLE && (other as usize) < count {
                        let o = other as usize;
                        if self.particles[o].free {
                            break;
                        }
                        let current = o;
                        other = self.particles[o].hash_next;
                        if current == index {
                            continue;
                        }
                        let me = self.particles[index].pos;
                        let them = self.particles[current].pos;
                        // No wrap: the city works in its own unwrapped coordinates.
                        let mid = Vec2::new((them.x + me.x) * 0.5, (them.y + me.y) * 0.5);
                        if !self.tensor_in_frame(mid) {
                            continue;
                        }
                        let delta = me - them;
                        let t = self.sample_tensor(mid);
                        self.particles[index].tensor = t;
                        let len = (t.y * t.y + t.x * t.x).sqrt();
                        let (ux, uy) = if len != 0.0 || len.is_nan() {
                            (t.x / len, t.y / len)
                        } else {
                            (0.0, 0.0)
                        };
                        let f = &self.frame;
                        let fx = f[1] * uy + f[0] * ux;
                        let fy = uy * f[3] + ux * f[2];
                        let flen = (fy * fy + fx * fx).sqrt();
                        let ax = fx / flen;
                        let ay = fy / flen;
                        // The along-axis term adds dy and the axis y instead of multiplying (as in
                        // the binary).
                        let across = (ay * delta.x - ax * delta.y).abs();
                        let along = (delta.x * ax + delta.y + ay).abs();
                        let distance = across + along;
                        if prune && MIN_NEIGHBOUR_DISTANCE > distance {
                            self.particles[current].free = true;
                            continue;
                        }
                        let mag = force_magnitude(&PROFILES[0], distance);
                        let force = Vec2::new(delta.x / distance * mag, delta.y / distance * mag);
                        self.particles[index].force += force;
                        self.particles[current].force -= force;
                    }
                }
            }
            let pos = self.particles[index].pos;
            let cell_x = (pos.x - min.x) as i32;
            let cell_y = (pos.y - min.y) as i32;
            let at = |x: i32, y: i32| (y as usize) * MASK_SIDE + x as usize;
            if kill_on_mask && self.mask[at(cell_x, cell_y)] != 0 {
                self.particles[index].free = true;
                continue;
            }
            let cx0 = (cell_x - MASK_RADIUS).max(0);
            let cy0 = (cell_y - MASK_RADIUS).max(0);
            let cx_end = if cell_x + MASK_RADIUS + 1 >= 0x80 {
                0x7f
            } else {
                cell_x + MASK_RADIUS + 1
            };
            let cy_end = if cell_y + MASK_RADIUS + 1 >= 0x80 {
                0x7f
            } else {
                cell_y + MASK_RADIUS + 1
            };
            for row in cy0..cy_end {
                for col in cx0..cx_end {
                    let class = self.mask[at(col, row)];
                    if class == 0 {
                        continue;
                    }
                    let dx = (pos.x - min.x) - col as f32;
                    let dy = (pos.y - min.y) - row as f32;
                    let distance = (dy * dy + dx * dx).sqrt();
                    let mag = force_magnitude(&PROFILES[usize::from(class.min(3))], distance);
                    let p = &mut self.particles[index];
                    p.force.x += dx / distance * mag;
                    p.force.y += dy / distance * mag;
                }
            }
        }
        // Integrate and rebuild the hash; the newest particle heads its bin.
        self.heads = [[NO_PARTICLE; BINS]; BINS];
        for index in 0..count {
            let p = &mut self.particles[index];
            if p.free {
                continue;
            }
            p.pos += p.force;
            p.force = Vec2::ZERO;
            let bx = (((p.pos.x - min.x) * 0.25) as i32).clamp(0, 0x1f) as usize;
            let by = (((p.pos.y - min.y) * 0.25) as i32).clamp(0, 0x1f) as usize;
            p.hash_next = self.heads[by][bx];
            self.heads[by][bx] = index as u16;
        }
    }

    /// `SolveForFinalPlacements`: 40 ticks; pruning off every 5th, the mask kill off every 10th.
    pub fn solve(&mut self) {
        self.solve_ticks(0, SOLVE_TICKS);
    }

    /// The ticks `from..from + count` of [`LayoutGen::solve`] (cut at the last); returns the tick
    /// the solve is at, [`SOLVE_TICKS`] once it is done.
    pub fn solve_ticks(&mut self, from: u32, count: u32) -> u32 {
        let end = from.saturating_add(count).min(SOLVE_TICKS);
        for tick in from..end {
            let mut flags = 0;
            if tick % 5 != 0 {
                flags |= 1;
            }
            if tick % 10 != 0 {
                flags |= 2;
            }
            self.tick(flags);
        }
        end
    }

    /// `FinalizePlacementSet`: frees the particles outside the rect, stores the tensor at each,
    /// and returns the live ones in index order.
    pub fn finalize(&mut self) -> Vec<Placement> {
        let mut out = Vec::new();
        for index in 0..self.particles.len() {
            if self.particles[index].free {
                continue;
            }
            let pos = self.particles[index].pos;
            if !(self.rect.contains(pos) && self.tensor_in_frame(pos)) {
                self.particles[index].free = true;
                continue;
            }
            let t = self.sample_tensor(pos);
            self.particles[index].tensor = t;
            out.push(Placement { index, pos, dir: t });
        }
        out
    }

    fn put(&mut self, x: i32, y: i32, value: u8) {
        if (0..MASK_SIDE as i32).contains(&x) && (0..MASK_SIDE as i32).contains(&y) {
            self.mask[y as usize * MASK_SIDE + x as usize] = value;
        }
    }

    /// `FUN_087bb7c0` with `RasterizeLine`: the polyline, each segment drawn with Bresenham into
    /// the rect-local cells (truncated toward zero).
    // SIMPLIFIED(SIMP-021): `RasterizeLine` (0x087bc32e) is not read; ours draws both ends.
    pub fn rasterize_polyline(&mut self, points: &[Vec2], value: u8) {
        let min = self.rect.min;
        for w in points.windows(2) {
            let (a, b) = (w[0] - min, w[1] - min);
            let (mut x0, mut y0) = (a.x as i32, a.y as i32);
            let (x1, y1) = (b.x as i32, b.y as i32);
            let dx = (x1 - x0).abs();
            let dy = -(y1 - y0).abs();
            let sx = if x0 < x1 { 1 } else { -1 };
            let sy = if y0 < y1 { 1 } else { -1 };
            let mut err = dx + dy;
            loop {
                self.put(x0, y0, value);
                if x0 == x1 && y0 == y1 {
                    break;
                }
                let e2 = 2 * err;
                if e2 >= dy {
                    err += dy;
                    x0 += sx;
                }
                if e2 <= dx {
                    err += dx;
                    y0 += sy;
                }
            }
        }
    }

    /// `FUN_087bb886`: a convex polygon (the oriented box of a wonder) filled into the mask.
    // SIMPLIFIED(SIMP-021): the triangle fill (0x087c2d10) is not read; ours takes the cells whose
    // middle is inside.
    pub fn fill_polygon(&mut self, points: &[Vec2], value: u8) {
        if points.len() < 3 {
            return;
        }
        let local: Vec<Vec2> = points.iter().map(|p| *p - self.rect.min).collect();
        let lo = local.iter().fold(Vec2::splat(f32::MAX), |a, p| a.min(*p));
        let hi = local.iter().fold(Vec2::splat(f32::MIN), |a, p| a.max(*p));
        let inside = |q: Vec2| {
            let mut sign = 0.0f32;
            for i in 0..local.len() {
                let a = local[i];
                let b = local[(i + 1) % local.len()];
                let c = (b - a).perp_dot(q - a);
                if c != 0.0 {
                    if sign != 0.0 && c.signum() != sign {
                        return false;
                    }
                    sign = c.signum();
                }
            }
            true
        };
        let y0 = lo.y.floor().max(0.0) as i32;
        let y1 = hi.y.ceil().min(MASK_SIDE as f32 - 1.0) as i32;
        let x0 = lo.x.floor().max(0.0) as i32;
        let x1 = hi.x.ceil().min(MASK_SIDE as f32 - 1.0) as i32;
        for y in y0..=y1 {
            for x in x0..=x1 {
                if inside(Vec2::new(x as f32 + 0.5, y as f32 + 0.5)) {
                    self.put(x, y, value);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Rect {
        Rect {
            min: Vec2::new(1000.0 - 64.0, 500.0 - 55.425_625),
            max: Vec2::new(1000.0 + 64.0, 500.0 + 55.425_625),
        }
    }

    #[test]
    fn the_lattice_is_turned_45_degrees_at_spacing_6() {
        let t = TensorField::uniform(64, 0);
        let g = LayoutGen::new(rect(), &t);
        // 30 x 30 lattice points, those of the 128 square (bins 0..31) kept.
        assert!(g.live() > 400 && g.live() < 520, "{}", g.live());
        // The nearest neighbour is a diagonal step of 128/30 each way.
        let p0 = g.particles[40].pos;
        let d = g
            .particles
            .iter()
            .map(|p| p.pos - p0)
            .filter(|d| d.length() > 1e-3)
            .min_by(|a, b| a.length().total_cmp(&b.length()))
            .unwrap();
        assert!(
            (d.length() - 128.0 / 30.0 * 2f32.sqrt()).abs() < 1e-3,
            "{d:?}"
        );
        assert!((d.x.abs() - d.y.abs()).abs() < 1e-3);
    }

    #[test]
    fn the_force_profile_is_the_original_cubic() {
        let p = PROFILES[0];
        assert!((force_magnitude(&p, 5.0) - 0.1).abs() < 1e-6);
        assert_eq!(force_magnitude(&p, 7.0), 0.0);
        assert!((force_magnitude(&p, 6.0) - 0.05).abs() < 1e-6);
    }

    #[test]
    fn the_solver_relaxes_without_random_numbers_and_keeps_the_wall_free() {
        let t = TensorField::uniform(64, 0);
        let run = |wall: bool| {
            let mut g = LayoutGen::new(rect(), &t);
            if wall {
                let c = Vec2::new(1000.0, 500.0);
                let ring: Vec<Vec2> = (0..=48)
                    .map(|k| {
                        let a = k as f32 / 48.0 * std::f32::consts::TAU;
                        c + Vec2::new(a.cos(), a.sin()) * 30.0
                    })
                    .collect();
                g.rasterize_polyline(&ring, MASK_WALL);
            }
            g.solve();
            g.finalize()
        };
        let a = run(true);
        assert_eq!(a, run(true), "deterministic");
        assert!(a.len() > 100, "{}", a.len());
        // Nobody stands on the wall line.
        for p in &a {
            let r = (p.pos - Vec2::new(1000.0, 500.0)).length();
            assert!((r - 30.0).abs() > 1.0, "{r}");
        }
        // The heading is the tensor (64/128 along x).
        assert!((a[0].dir - Vec2::new(0.5, 0.0)).length() < 1e-6);
    }

    #[test]
    fn a_wonder_box_is_filled() {
        let t = TensorField::uniform(64, 0);
        let mut g = LayoutGen::new(rect(), &t);
        let m = rect().min;
        g.fill_polygon(
            &[
                m + Vec2::new(10.0, 10.0),
                m + Vec2::new(20.0, 10.0),
                m + Vec2::new(20.0, 20.0),
                m + Vec2::new(10.0, 20.0),
            ],
            MASK_WONDER,
        );
        let n = g.mask.iter().filter(|c| **c == MASK_WONDER).count();
        assert_eq!(n, 100);
    }
}
