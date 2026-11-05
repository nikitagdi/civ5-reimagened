//! The Fork particle simulation on the CPU (world-objects WOB8b), by the reading of the SDK in
//! `civ5-re` answers-b updates 6 and 10 (RW-29): emit loop `frkPEmitterEmit` 0x08ffb183, the
//! twelve birth volumes, `DefaultFullSim` 0x09002203 and `DefaultPhysics` 0x09001421.
//!
//! Space: the original's basis without the map translation (x east, y north, z up), so a point
//! of ours is `basis_rotation() * p` and nothing depends on the frame. An effect's matrix is the
//! visual effect's transform (scale, rotation, translation) then the map position; the emitter's
//! is the property's matrix then that (row-vector `P * effect`, column `effect * P`).
//!
//! The `rand()` calls are made in the order of the binary, with the C runtime's generator.

use std::rc::Rc;

use asset_format::{FxEffect, FxEmitter, FxPath, FxProps, fx_flag as flag, fx_flag2 as flag2};
use glam::{Mat4, Vec3, Vec4};

/// Most particles one emitter emits in a frame (PreProcess 0x09000e6a).
pub const MAX_EMIT_PER_FRAME: usize = 0x400;
/// Most particles alive in the whole system (the Fork pool is 0x2000 per system).
pub const MAX_PARTICLES: usize = 0x2000;
/// An effect that never ends by itself is dropped after this long without particles, and a
/// released one after this long at most.
pub const MAX_LINGER_SECONDS: f32 = 30.0;
/// Longest prewarm simulated at spawn, in steps of [`PREWARM_STEP`].
pub const MAX_PREWARM_STEPS: usize = 90;
pub const PREWARM_STEP: f32 = 1.0 / 30.0;

/// The C runtime's `rand()` (MSVC: `x = x * 214013 + 2531011`, `(x >> 16) & 0x7fff`).
#[derive(Debug, Clone)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Self(seed)
    }

    pub fn rand(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(214_013).wrapping_add(2_531_011);
        (self.0 >> 16) & 0x7fff
    }

    /// `(rand() % 100) * 0.01`: 0..0.99 in steps of 0.01.
    pub fn u(&mut self) -> f32 {
        (self.rand() % 100) as f32 * 0.01
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    std::array::from_fn(|i| lerp(a[i], b[i], t))
}

fn v3(a: [f32; 3]) -> Vec3 {
    Vec3::from_array(a)
}

/// The emitter matrix of the property (rows of the row-vector convention are glam's columns).
pub fn property_matrix(p: &FxProps) -> Mat4 {
    Mat4::from_cols_array(&p.matrix)
}

/// The matrix of a visual effect placed at `at` (the original's basis, no map translation):
/// scale, rotation in degrees (x, then y, then z) and translation of the effect, then `at`.
pub fn effect_matrix(at: Vec3, translation: [f32; 3], rotation_deg: [f32; 3], scale: f32) -> Mat4 {
    let r = rotation_deg.map(f32::to_radians);
    Mat4::from_translation(at + v3(translation))
        * Mat4::from_rotation_z(r[2])
        * Mat4::from_rotation_y(r[1])
        * Mat4::from_rotation_x(r[0])
        * Mat4::from_scale(Vec3::splat(scale))
}

/// One particle (the 0x84-byte record, less the links).
#[derive(Debug, Clone, PartialEq)]
pub struct Particle {
    pub sizes: [f32; 3],
    pub spin: f32,
    pub start_frame: f32,
    pub life: f32,
    pub pos: Vec3,
    pub vel: Vec3,
    pub color: [f32; 4],
    pub age: f32,
    pub size: f32,
    pub rotation: f32,
    pub frame: u32,
    pub extra: f32,
    /// Bezier offset and its phase.
    pub offset: Vec3,
    pub phase: f32,
}

/// The state of one emitter of a live effect.
#[derive(Debug, Clone)]
pub struct EmitterState {
    /// Index in [`FxEffect::emitters`].
    pub index: usize,
    /// The property now (the track moves it).
    pub props: FxProps,
    pub matrix: Mat4,
    pub prev: Mat4,
    pub particles: Vec<Particle>,
    carry: f32,
    age: f32,
    delay: f32,
    burst_clock: f32,
    burst_interval: f32,
    burst_done: bool,
    path_dist: f32,
}

/// A live effect: one `.psb` placed on the map.
#[derive(Debug, Clone)]
pub struct Instance {
    pub key: u64,
    pub fx: Rc<FxEffect>,
    /// The effect's matrix (see [`effect_matrix`]).
    pub matrix: Mat4,
    /// Emitter scale (`E+0x330`): the visual effect's scale times the request's.
    pub scale: f32,
    pub emitters: Vec<EmitterState>,
    pub age: f32,
    /// Age at which the effect stops emitting (`fBaseDuration`); `None` lives until released.
    pub deadline: Option<f32>,
    /// It emits no more; it goes when its particles are gone.
    pub released: bool,
    /// Drawn (the fog may hide an effect without stopping it).
    pub visible: bool,
}

impl Instance {
    pub fn live(&self) -> usize {
        self.emitters.iter().map(|e| e.particles.len()).sum()
    }
}

/// Every live effect.
#[derive(Debug)]
pub struct Sim {
    pub instances: Vec<Instance>,
    pub rng: Rng,
    /// Direction of the gravity (`v += g * gravityScale * dt`), the original's basis.
    pub gravity: Vec3,
}

impl Default for Sim {
    fn default() -> Self {
        Self {
            instances: Vec::new(),
            rng: Rng::new(1),
            // SIMPLIFIED(SIMP-048): Fork's default gravity is (0, 1, 0) in its own y-up space and
            // Civ5's call that sets it was not read; the map is z up and the flames of the city
            // damage (`fxp_d2_fire_01`, no rotation in its visual effect) rise, so it is +z here.
            gravity: Vec3::Z,
        }
    }
}

impl Sim {
    pub fn total(&self) -> usize {
        self.instances.iter().map(Instance::live).sum()
    }

    /// Places an effect (`Spawn` with `duration: None`, `Trigger`/`OneShot` with the visual
    /// effect's duration). The same `key` again moves the live effect and refreshes its deadline
    /// (`SetTransform` 0x088cacfe), as a re-fired trigger does.
    pub fn spawn(
        &mut self,
        key: u64,
        fx: Rc<FxEffect>,
        matrix: Mat4,
        scale: f32,
        duration: Option<f32>,
    ) {
        if let Some(i) = self
            .instances
            .iter_mut()
            .find(|i| i.key == key && !i.released)
        {
            i.matrix = matrix;
            i.deadline = duration.map(|d| i.age + d);
            return;
        }
        let emitters = fx
            .emitters
            .iter()
            .enumerate()
            // A referenced emitter is only a sub-emitter's template.
            .filter(|(_, e)| e.props.flags & flag::REFERENCED == 0)
            .map(|(index, e)| {
                let delay = lerp(e.props.start_delay[0], e.props.start_delay[1], self.rng.u());
                let m = matrix * property_matrix(&e.props);
                EmitterState {
                    index,
                    props: e.props.clone(),
                    matrix: m,
                    prev: m,
                    particles: Vec::new(),
                    carry: 0.0,
                    age: 0.0,
                    delay,
                    burst_clock: 0.0,
                    burst_interval: 0.0,
                    burst_done: false,
                    path_dist: 0.0,
                }
            })
            .collect();
        let mut inst = Instance {
            key,
            fx,
            matrix,
            scale,
            emitters,
            age: 0.0,
            deadline: duration,
            released: false,
            visible: true,
        };
        let prewarm = inst
            .emitters
            .iter()
            .map(|e| e.props.prewarm)
            .fold(0.0f32, f32::max);
        if prewarm > 0.0 {
            let steps = ((prewarm / PREWARM_STEP) as usize).min(MAX_PREWARM_STEPS);
            for _ in 0..steps {
                step_instance(
                    &mut inst,
                    PREWARM_STEP,
                    &mut self.rng,
                    self.gravity,
                    usize::MAX,
                );
            }
        }
        self.instances.push(inst);
    }

    /// Stops the effect of `key` (`DeSpawn`): it emits no more and goes when its particles die.
    pub fn release(&mut self, key: u64) {
        for i in self.instances.iter_mut().filter(|i| i.key == key) {
            i.released = true;
        }
    }

    /// Moves the effect of `key`.
    pub fn move_to(&mut self, key: u64, matrix: Mat4) {
        for i in self.instances.iter_mut().filter(|i| i.key == key) {
            i.matrix = matrix;
        }
    }

    pub fn set_visible(&mut self, key: u64, visible: bool) {
        for i in self.instances.iter_mut().filter(|i| i.key == key) {
            i.visible = visible;
        }
    }

    pub fn has(&self, key: u64) -> bool {
        self.instances.iter().any(|i| i.key == key && !i.released)
    }

    /// Advances every effect by `dt` seconds and drops the finished ones.
    pub fn step(&mut self, dt: f32) {
        if dt <= 0.0 {
            return;
        }
        let mut room = MAX_PARTICLES.saturating_sub(self.total());
        for inst in &mut self.instances {
            let before = inst.live();
            step_instance(inst, dt, &mut self.rng, self.gravity, room);
            room = (room + before).saturating_sub(inst.live());
        }
        self.instances.retain(|i| {
            let quiet = i.released || i.deadline.is_some_and(|d| i.age >= d);
            !(quiet && (i.live() == 0 || i.age > i.deadline.unwrap_or(0.0) + MAX_LINGER_SECONDS))
        });
    }
}

/// One step of one effect. `room`: particles the system may still add.
fn step_instance(inst: &mut Instance, dt: f32, rng: &mut Rng, gravity: Vec3, mut room: usize) {
    inst.age += dt;
    if inst.deadline.is_some_and(|d| inst.age >= d) {
        inst.released = true;
    }
    let fx = inst.fx.clone();
    for em in &mut inst.emitters {
        let def = &fx.emitters[em.index];
        let edt = dt * def.props.time_scale.max(0.0);
        em.age += edt;
        if let Some(t) = &def.track {
            em.props = animate(t, em.age);
        }
        em.prev = em.matrix;
        em.matrix = inst.matrix * property_matrix(&em.props);
        if em.props.flags & flag::EMITTER_PATH != 0
            && let Some(p) = &def.path
        {
            em.path_dist += edt * em.props.path_speed;
            let at = path_point(p, em.path_dist) - v3(p.points[0]) + v3(p.origin);
            em.matrix *= Mat4::from_translation(at);
        }
        let before = em.particles.len();
        update_particles(em, def, edt, gravity, rng);
        room = (room + before).saturating_sub(em.particles.len());
        if !inst.released && em.age >= em.delay {
            let n = emit_count(em, edt, rng).min(room);
            for _ in 0..n {
                let p = birth(em, def, inst.scale, rng);
                em.particles.push(p);
            }
            room -= n;
        }
    }
}

/// How many particles to emit this step: the rate with its fraction carried, and the bursts;
/// capped by the most alive and per frame.
fn emit_count(em: &mut EmitterState, dt: f32, rng: &mut Rng) -> usize {
    let p = &em.props;
    let exact = p.rate.max(0.0) * dt + em.carry;
    let mut n = exact.floor();
    em.carry = exact - n;
    if p.burst > 0 {
        if !em.burst_done {
            n += p.burst as f32;
            em.burst_done = true;
            em.burst_interval = lerp(p.burst_interval[0], p.burst_interval[1], rng.u());
        } else if em.burst_interval > 0.0 {
            em.burst_clock += dt;
            if em.burst_clock >= em.burst_interval {
                em.burst_clock -= em.burst_interval;
                n += p.burst as f32;
                em.burst_interval = lerp(p.burst_interval[0], p.burst_interval[1], rng.u());
            }
        }
    }
    let alive = em.particles.len() as f32;
    let cap = (p.max_live - alive).max(0.0);
    (n.min(cap) as usize).min(MAX_EMIT_PER_FRAME)
}

/// The "up" direction of the birth volumes 0, 2, 4, 6 and 8: `0xE0` is the spread.
fn up_dir(spread: f32, rng: &mut Rng) -> Vec3 {
    let a = std::f32::consts::TAU * rng.u();
    let r = rng.u() * spread;
    Vec3::new(a.cos() * r, 10.0, a.sin() * r).normalize()
}

/// Birth position (local) and direction by the volume (`g_aDefaultBirthCB`).
pub fn birth_volume(p: &FxProps, rng: &mut Rng) -> (Vec3, Vec3) {
    use std::f32::consts::{PI, TAU};
    let [dx, dy, dz, spread] = p.volume;
    let norm = |v: Vec3| v.try_normalize().unwrap_or(Vec3::Y);
    match p.volume_kind {
        // Plane (0) and its volume (4), the custom slot (9) that falls back to it.
        0 | 4 | 9 => {
            let x = rng.u() * dx - dx * 0.5;
            let z = rng.u() * dy - dy * 0.5;
            let y = rng.u() * dz - dz * 0.5;
            (Vec3::new(x, y, z), up_dir(spread, rng))
        }
        1 => {
            let th = PI * rng.u();
            let ph = TAU * rng.u();
            let pos = Vec3::new(
                dx * ph.sin() * th.cos(),
                dz * ph.cos(),
                dy * ph.sin() * th.sin(),
            );
            (pos, norm(pos))
        }
        5 => {
            let ux = rng.u();
            let uz = rng.u();
            let uy = rng.u();
            let th = PI * rng.u();
            let ph = TAU * rng.u();
            let pos = Vec3::new(
                dx * ux * ph.sin() * th.cos(),
                dz * uy * ph.cos(),
                dy * uz * ph.sin() * th.sin(),
            );
            (pos, norm(pos))
        }
        2 => {
            let h_max = dy;
            let h = rng.u() * h_max;
            let a = TAU * rng.u();
            let (x, z) = if h_max.abs() > 1e-4 {
                (
                    (dx - dx * h / h_max) * a.cos(),
                    (dz - dz * h / h_max) * a.sin(),
                )
            } else {
                (0.0, 0.0)
            };
            (Vec3::new(x, h, z), up_dir(spread, rng))
        }
        6 => {
            let rx = rng.u() * dx;
            let rz = rng.u() * dz;
            let h_max = rng.u() * dy + 1e-5;
            let h = rng.u() * h_max;
            let a = TAU * rng.u();
            let pos = Vec3::new(
                (rx - rx * h / h_max) * a.cos(),
                h,
                (rz - rz * h / h_max) * a.sin(),
            );
            (pos, up_dir(spread, rng))
        }
        3 => {
            let y = rng.u() * dy;
            let a = TAU * rng.u();
            let pos = Vec3::new(dx * a.cos(), y, dz * a.sin());
            (pos, norm(pos))
        }
        7 => {
            let u1 = rng.u();
            let u2 = rng.u();
            let u3 = rng.u();
            let u4 = rng.u();
            let a = TAU * rng.u();
            let pos = Vec3::new(u1 * dx * a.cos(), u4 * u3 * dy, u2 * dz * a.sin());
            (pos, norm(pos))
        }
        10 | 11 => {
            let (rx, ry, rz) = if p.volume_kind == 11 {
                (rng.u() * dx, rng.u() * dx, rng.u() * dx)
            } else {
                (dx, dx, dx)
            };
            let az = dy.to_radians() * rng.u();
            let pol = dz.to_radians() * rng.u();
            let pos = Vec3::new(
                rx * pol.sin() * az.cos(),
                ry * pol.cos(),
                rz * pol.sin() * az.sin(),
            );
            (pos, norm(pos))
        }
        // 8 point, and anything else.
        _ => (Vec3::ZERO, up_dir(spread, rng)),
    }
}

/// The bezier offset of a particle: angle, radius with the sign rule.
fn tube_offset(p: &FxProps, rng: &mut Rng) -> Vec3 {
    let a = rng.u() * 6.282;
    let mut r = lerp(p.tube_radius[0], p.tube_radius[1], rng.u());
    if ((100.0 * r) as i32) & 1 == 0 {
        r = -r;
    }
    Vec3::new(a.sin() * r, 0.0, a.cos() * r)
}

/// `_frkParticleBirthBezier` 0x0900084f.
fn birth_bezier(p: &FxProps, path: Option<&FxPath>, rng: &mut Rng) -> (Vec3, Vec3) {
    let Some(path) = path else {
        return (Vec3::ZERO, Vec3::ZERO);
    };
    let n = path.points.len();
    let i = rng.rand() as usize % n;
    let j = (i + 1).min(n - 1);
    let pos = if path.mode & 1 != 0 {
        let t = rng.u();
        let off = tube_offset(p, rng);
        // SIMPLIFIED(SIMP-048): the offset is not turned by the Euler angles of the delta.
        v3(path.points[i]) + v3(path.deltas[j]) * t + off
    } else {
        v3(path.points[i]) + v3(path.deltas[j]) * rng.u()
    };
    let dir = Vec3::new(
        0.0,
        0.1 * p.speed[0] + rng.u() * (p.speed[1] - p.speed[0]),
        0.0,
    );
    (pos, dir)
}

/// A new particle (Emit steps 1 to 11).
fn birth(em: &EmitterState, def: &FxEmitter, scale: f32, rng: &mut Rng) -> Particle {
    let p = &em.props;
    let (mut pos, dir) = if p.flags & flag::BEZIER_BIRTH != 0 {
        birth_bezier(p, def.path.as_ref(), rng)
    } else {
        birth_volume(p, rng)
    };
    if let Some(path) = &def.path
        && path.mode & 3 != 0
        && p.flags & flag::EMITTER_PATH != 0
    {
        pos += v3(path.points[1]);
    }
    let speed = lerp(p.speed[0], p.speed[1], rng.u());
    let rot = Mat4::from_cols(
        em.matrix.x_axis,
        em.matrix.y_axis,
        em.matrix.z_axis,
        Vec4::W,
    );
    let mut vel = rot.transform_vector3(dir * speed);
    let mut spin;
    if p.flags & flag::NEGATIVE_SPIN == 0 {
        spin = lerp(p.spin[0], p.spin[1], rng.u());
        if p.flags & flag::NO_SPIN_SIGN == 0 && rng.rand() & 1 == 0 {
            spin = -spin;
        }
    } else {
        spin = -lerp(p.spin[0], p.spin[1], rng.u());
    }
    let rotation = spin;
    if p.flags2 & flag2::NO_SPIN != 0 {
        spin = 0.0;
    }
    let mut life = lerp(p.life[0], p.life[1], rng.u());
    if life <= 0.0 {
        life = p.life[1] * 0.5;
    }
    let s0 = lerp(p.size_min[0], p.size_max[0], rng.u()) * scale;
    let sizes = if p.flags2 & flag2::UNIFORM_SIZE == 0 {
        let s1 = lerp(p.size_min[1], p.size_max[1], rng.u()) * scale;
        let s2 = lerp(p.size_min[2], p.size_max[2], rng.u()) * scale;
        [s0, s1, s2]
    } else {
        [s0; 3]
    };
    let extra = lerp(p.extra[0], p.extra[1], rng.u()) * scale;
    let mut world = em.matrix.transform_point3(pos);
    let frames = def.frames.len();
    let start_frame =
        if frames > 1 && p.flags & flag::FLIPBOOK != 0 && p.flags & flag::RANDOM_FRAME != 0 {
            (frames as f32 * rng.u()).floor()
        } else {
            0.0
        };
    let mut offset = Vec3::ZERO;
    if p.flags & flag::BEZIER_PATH != 0 {
        offset = tube_offset(p, rng);
        if let Some(path) = &def.path {
            world = v3(path.points[0]) + offset + em.matrix.w_axis.truncate();
            let d = v3(path.deltas[1]).try_normalize().unwrap_or(Vec3::Y);
            vel = d * vel.length();
        }
    }
    Particle {
        sizes,
        spin,
        start_frame,
        life,
        pos: world,
        vel,
        color: p.colors[0],
        age: 0.0,
        size: s0,
        rotation,
        frame: start_frame as u32,
        extra,
        offset,
        phase: 0.0,
    }
}

/// A point at distance `d` along the path (wrapped to its length).
fn path_point(path: &FxPath, d: f32) -> Vec3 {
    let n = path.points.len();
    let mut s = if path.length > 0.0 {
        d / path.length
    } else {
        0.0
    };
    if s > 1.0 {
        s = s.fract();
    }
    let f = n as f32 * s.max(0.0);
    let mut i = f as usize;
    if i + 2 > n {
        i = 0;
    }
    v3(path.points[i]) + v3(path.deltas[i + 1]) * f.fract()
}

/// Ages, colours, sizes, spins, flipbooks and moves the particles; drops the dead.
fn update_particles(em: &mut EmitterState, def: &FxEmitter, dt: f32, gravity: Vec3, rng: &mut Rng) {
    let p = &em.props;
    let follow = if p.flags & flag::FOLLOW_EMITTER != 0 {
        Some(em.matrix * em.prev.inverse())
    } else {
        None
    };
    let frames = def.frames.len().max(1) as f32;
    let forces: Vec<(Vec3, f32, f32, u8)> = def
        .forces
        .iter()
        .map(|f| {
            let at = if f.kind == 4 {
                em.matrix.transform_vector3(v3(f.position))
            } else {
                em.matrix.transform_point3(v3(f.position))
            };
            (at, f.strength, f.radius, f.kind)
        })
        .collect();
    let bezier = p.flags & flag::BEZIER_PATH != 0;
    let matrix = em.matrix;
    for q in &mut em.particles {
        if p.flags & flag::FREEZE_AGE == 0 {
            q.age += dt;
        }
        if q.age >= q.life {
            continue;
        }
        let tn = q.age / q.life;
        let [t1, t2] = p.times;
        q.color = if tn <= t1 {
            lerp4(
                p.colors[0],
                p.colors[1],
                if t1 > 0.0 { tn / t1 } else { 1.0 },
            )
        } else if tn <= t2 {
            lerp4(p.colors[1], p.colors[2], (tn - t1) / (t2 - t1).max(1e-6))
        } else {
            lerp4(p.colors[2], p.colors[3], (tn - t2) / (1.0 - t2).max(1e-6))
        };
        q.size = if tn <= p.mid_time {
            lerp(
                q.sizes[0],
                q.sizes[1],
                if p.mid_time > 0.0 {
                    tn / p.mid_time
                } else {
                    1.0
                },
            )
        } else {
            lerp(
                q.sizes[1],
                q.sizes[2],
                (tn - p.mid_time) / (1.0 - p.mid_time).max(1e-6),
            )
        };
        q.rotation += q.spin * dt;
        if p.flags & flag::FLIPBOOK != 0 && frames > 1.0 {
            q.frame = ((q.age * p.flipbook_fps + q.start_frame) % frames) as u32;
        }
        if let Some(m) = follow {
            q.pos = m.transform_point3(q.pos);
            q.vel = m.transform_vector3(q.vel);
        }
        if bezier && let Some(path) = &def.path {
            // `_frkParticlePathBezier` 0x090026e6, without the phase modes.
            let spd = q.vel.length();
            let local = path_point(path, q.age * spd) + q.offset;
            q.pos = matrix.transform_point3(local);
            continue;
        }
        let mut force = Vec3::ZERO;
        for (at, strength, radius, kind) in &forces {
            match kind {
                4 => force += *at * *strength,
                0 | 1 => {
                    let d = *at - q.pos;
                    let r2 = d.length_squared().max(1e-4);
                    if *radius <= 0.0 || r2 <= radius * radius {
                        let sign = if *kind == 1 { 1.0 } else { -1.0 };
                        force += d.normalize_or_zero() * (sign * strength / r2);
                    }
                }
                2 => {
                    force +=
                        Vec3::new(rng.u() - 0.5, rng.u() - 0.5, rng.u() - 0.5) * (2.0 * strength);
                }
                _ => {}
            }
        }
        // SIMPLIFIED(SIMP-048): the reading gives `v += (v * drag + F) * dt`; a positive drag
        // would speed every particle up, so drag slows here (`v -= v * drag * dt`).
        q.vel += (force - q.vel * p.drag) * dt + gravity * p.gravity * dt;
        q.pos += q.vel * dt;
    }
    let loop_age = p.flags & 0x10_0000 != 0;
    if loop_age {
        for q in &mut em.particles {
            if q.age >= q.life {
                q.age %= q.life.max(1e-3);
            }
        }
    }
    em.particles.retain(|q| q.age < q.life);
}

/// The property of a track at emitter age `age`: the animated fields between the two keys, the
/// others from the key at or before (answers-b update 6: "the rest steps").
pub fn animate(track: &asset_format::FxTrack, age: f32) -> FxProps {
    let keys = &track.keys;
    let last = keys.last().map_or(0, |k| k.0) as f32;
    let mut f = age * track.fps;
    if track.looped && last > 0.0 {
        f %= last;
    }
    let i = keys
        .partition_point(|k| (k.0 as f32) <= f)
        .saturating_sub(1);
    let (fa, a) = &keys[i];
    let Some((fb, b)) = keys.get(i + 1) else {
        return a.clone();
    };
    // SIMPLIFIED(SIMP-048): linear between keys; the Hermite tangents (key + 0x204) are not read.
    let t = ((f - *fa as f32) / (*fb as f32 - *fa as f32).max(1e-6)).clamp(0.0, 1.0);
    let mut out = a.clone();
    let l = |x: f32, y: f32| lerp(x, y, t);
    out.volume = lerp4(a.volume, b.volume, t);
    out.gravity = l(a.gravity, b.gravity);
    out.rate = l(a.rate, b.rate);
    out.life = [l(a.life[0], b.life[0]), l(a.life[1], b.life[1])];
    out.spin = [l(a.spin[0], b.spin[0]), l(a.spin[1], b.spin[1])];
    out.drag = l(a.drag, b.drag);
    out.max_live = l(a.max_live, b.max_live);
    out.rotation = std::array::from_fn(|k| l(a.rotation[k], b.rotation[k]));
    out.size_scale = std::array::from_fn(|k| l(a.size_scale[k], b.size_scale[k]));
    out.tint = lerp4(a.tint, b.tint, t);
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use asset_format::{FxFrame, FxShader, FxTrack, LogicalId};

    pub fn props() -> FxProps {
        FxProps {
            colors: [
                [1.0, 0.0, 0.0, 1.0],
                [0.0, 1.0, 0.0, 1.0],
                [0.0, 0.0, 1.0, 1.0],
                [0.0; 4],
            ],
            times: [0.25, 0.5],
            life: [1.0, 1.0],
            speed: [0.0, 0.0],
            flags: 0,
            size_max: [2.0, 4.0, 6.0],
            mid_time: 0.5,
            spin: [0.0, 0.0],
            drag: 0.0,
            size_scale: [1.0; 3],
            rotation: [0.0; 3],
            tint: [1.0; 4],
            size_min: [2.0, 4.0, 6.0],
            prewarm: 0.0,
            rate: 10.0,
            gravity: 0.0,
            volume: [0.0; 4],
            max_live: 250.0,
            volume_kind: 8,
            shape: 1,
            burst: 0,
            flipbook_fps: 0.0,
            alpha: 1,
            time_scale: 1.0,
            tube_radius: [0.0; 2],
            path_speed: 0.0,
            start_delay: [0.0; 2],
            matrix: Mat4::IDENTITY.to_cols_array(),
            burst_interval: [0.0; 2],
            extra: [0.0; 2],
            flags2: 0,
        }
    }

    pub fn effect(p: FxProps) -> Rc<FxEffect> {
        Rc::new(FxEffect {
            name: "t".into(),
            emitters: vec![FxEmitter {
                name: "e".into(),
                props: p,
                frames: vec![FxFrame {
                    texture: Some(LogicalId::new("tx:a.dds")),
                    uv: [0.0, 0.0, 1.0, 1.0],
                }],
                forces: vec![],
                shader: FxShader::Default,
                track: None,
                path: None,
                sub_emitter: None,
            }],
        })
    }

    #[test]
    fn rand_is_the_msvc_generator() {
        let mut r = Rng::new(1);
        // srand(1): the first values of MSVC's rand().
        assert_eq!([r.rand(), r.rand(), r.rand()], [41, 18467, 6334]);
        let mut r = Rng::new(1);
        assert_eq!(r.u(), 0.41);
    }

    #[test]
    fn the_rate_emits_with_the_fraction_carried_and_life_ends_them() {
        let mut s = Sim::default();
        s.spawn(1, effect(props()), Mat4::IDENTITY, 1.0, None);
        for _ in 0..10 {
            s.step(0.05);
        }
        // 10 per second for 0.5 s; the first life ends at 1 s.
        assert_eq!(s.total(), 5);
        for _ in 0..30 {
            s.step(0.05);
        }
        assert!(s.total() <= 10 + 1, "a particle lives 1 s: {}", s.total());
        s.release(1);
        for _ in 0..30 {
            s.step(0.05);
        }
        assert!(s.instances.is_empty(), "released and empty: gone");
    }

    #[test]
    fn colour_and_size_follow_their_keys() {
        let mut s = Sim::default();
        let mut p = props();
        p.rate = 0.0;
        p.burst = 1;
        s.spawn(1, effect(p), Mat4::IDENTITY, 2.0, None);
        s.step(1e-4);
        let q = |s: &Sim| s.instances[0].emitters[0].particles[0].clone();
        assert_eq!(
            q(&s).sizes,
            [4.0, 8.0, 12.0],
            "sizes times the emitter scale"
        );
        s.step(0.25 - 1e-4);
        let a = q(&s);
        assert!(
            (a.color[1] - 1.0).abs() < 1e-3,
            "key 1 at T1: {:?}",
            a.color
        );
        assert!(
            (a.size - 6.0).abs() < 1e-3,
            "half way to the middle size: {}",
            a.size
        );
        s.step(0.25);
        assert!((q(&s).color[2] - 1.0).abs() < 1e-3);
        assert!((q(&s).size - 8.0).abs() < 1e-3);
    }

    #[test]
    fn a_burst_fires_once_without_an_interval_and_respects_max_live() {
        let mut s = Sim::default();
        let mut p = props();
        p.rate = 0.0;
        p.burst = 4;
        p.life = [10.0, 10.0];
        s.spawn(1, effect(p.clone()), Mat4::IDENTITY, 1.0, None);
        for _ in 0..20 {
            s.step(0.05);
        }
        assert_eq!(s.total(), 4);
        p.max_live = 3.0;
        let mut s = Sim::default();
        s.spawn(1, effect(p), Mat4::IDENTITY, 1.0, None);
        s.step(0.05);
        assert_eq!(s.total(), 3);
    }

    #[test]
    fn gravity_lifts_and_the_matrix_places() {
        let mut s = Sim::default();
        let mut p = props();
        p.gravity = 10.0;
        p.rate = 0.0;
        p.burst = 1;
        let m = effect_matrix(Vec3::new(100.0, 50.0, 0.0), [0.0; 3], [0.0; 3], 1.0);
        s.spawn(1, effect(p), m, 1.0, None);
        // Born at the end of the first step, moved by the second.
        s.step(0.01);
        s.step(0.5);
        let q = &s.instances[0].emitters[0].particles[0];
        assert!((q.pos.x - 100.0).abs() < 1e-4 && (q.pos.y - 50.0).abs() < 1e-4);
        assert!(q.pos.z > 0.0, "rises: {:?}", q.pos);
    }

    #[test]
    fn birth_volumes_stay_in_their_bounds() {
        let mut rng = Rng::new(7);
        for kind in 0..=11u8 {
            let mut p = props();
            p.volume_kind = kind;
            p.volume = [4.0, 6.0, 8.0, 1.0];
            for _ in 0..200 {
                let (pos, dir) = birth_volume(&p, &mut rng);
                assert!(pos.is_finite() && dir.is_finite(), "kind {kind}");
                assert!(pos.length() <= 16.0, "kind {kind}: {pos:?}");
            }
        }
        let mut p = props();
        p.volume_kind = 0;
        p.volume = [4.0, 6.0, 8.0, 0.0];
        let (pos, dir) = birth_volume(&p, &mut Rng::new(1));
        // rand order x, z, y: 0.41, 0.67, 0.34.
        assert!((pos.x - (0.41 * 4.0 - 2.0)).abs() < 1e-5);
        assert!((pos.z - (0.67 * 6.0 - 3.0)).abs() < 1e-5);
        assert!((pos.y - (0.34 * 8.0 - 4.0)).abs() < 1e-5);
        assert_eq!(dir, Vec3::Y, "no spread: straight up");
    }

    #[test]
    fn a_trigger_lives_its_duration_and_a_refire_refreshes_it() {
        let mut s = Sim::default();
        s.spawn(9, effect(props()), Mat4::IDENTITY, 1.0, Some(0.5));
        s.step(0.4);
        s.spawn(9, effect(props()), Mat4::IDENTITY, 1.0, Some(0.5));
        s.step(0.4);
        assert!(!s.instances[0].released, "refreshed at 0.4 to 0.9");
        s.step(0.2);
        assert!(s.instances[0].released);
        assert_eq!(s.instances.len(), 1);
    }

    #[test]
    fn a_track_moves_the_animated_fields() {
        let mut a = props();
        a.rate = 0.0;
        let mut b = props();
        b.rate = 100.0;
        b.alpha = 6;
        let t = FxTrack {
            fps: 10.0,
            looped: false,
            keys: vec![(0, a), (10, b)],
        };
        let m = animate(&t, 0.5);
        assert!((m.rate - 50.0).abs() < 1e-4);
        assert_eq!(m.alpha, 1, "steps with the key before");
        assert_eq!(animate(&t, 5.0).alpha, 6);
    }

    #[test]
    fn the_follow_flag_carries_particles_with_the_emitter() {
        let mut p = props();
        p.rate = 0.0;
        p.burst = 1;
        p.life = [10.0; 2];
        p.flags = flag::FOLLOW_EMITTER;
        let mut s = Sim::default();
        s.spawn(1, effect(p), Mat4::IDENTITY, 1.0, None);
        s.step(0.1);
        s.move_to(1, Mat4::from_translation(Vec3::X * 5.0));
        s.step(0.1);
        let q = &s.instances[0].emitters[0].particles[0];
        assert!((q.pos.x - 5.0).abs() < 1e-4, "{:?}", q.pos);
    }
}
