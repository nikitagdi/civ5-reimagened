//! Culling of the instances of the objects (units, landmarks, cities) against the volumes of the
//! passes (B3, packet D). Each instance is a sphere: the centre of the model's box through the
//! instance matrix and the radius of the box (a little more for a model that animates) times the
//! largest scale of the matrix. The test is conservative: a sphere that touches a volume is kept.
//!
//! The volumes, all in our basis:
//! * the main view: the six planes of the perspective frustum;
//! * the shadow map and the reflection target: the light's (and the reflection camera's) view is
//!   orthographic over the visible rectangle, a sphere is out only when it is wholly outside one of
//!   the four side planes. The map is the silhouette of the casters seen along the light, so what
//!   is outside the rectangle in the light's own x and y cannot land in the map; no depth is tested
//!   (the near and far of the light are 30 000 units), so the shadow that falls into the view from
//!   a caster out of it is kept: the caster is in the rectangle when its shadow is.
//!
//! A low eye (a mod's first-person camera, [`crate::camera::ObjectsNearEye`]) adds [`NearEye`]: every
//! instance is drawn smaller about its origin, and one whose sphere comes near the eye leaves the main view.

use glam::{Mat4, Vec3, Vec4};

use crate::world::objects::instance::ObjectInstance;
use crate::world::terrain::draw::frustum_planes;

/// Planes of a convex volume, normalised, pointing in.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Planes {
    planes: [Vec4; 6],
    count: usize,
}

impl Planes {
    /// The six planes of a perspective view (`projection * view`, depth 0 to 1).
    pub fn frustum(view_proj: &Mat4) -> Self {
        Self {
            planes: frustum_planes(view_proj),
            count: 6,
        }
    }

    /// The four side planes of an orthographic view (the near and the far are not tested).
    pub fn sides(view_proj: &Mat4) -> Self {
        Self {
            planes: frustum_planes(view_proj),
            count: 4,
        }
    }

    /// False only when the sphere is wholly outside one plane.
    pub fn sees(&self, centre: Vec3, radius: f32) -> bool {
        self.planes[..self.count]
            .iter()
            .all(|p| p.truncate().dot(centre) + p.w >= -radius)
    }
}

impl Planes {
    /// False only when the box is wholly outside one plane.
    pub fn sees_box(&self, min: Vec3, max: Vec3) -> bool {
        self.planes[..self.count].iter().all(|p| {
            // The corner of the box that is furthest along the normal.
            let c = Vec3::new(
                if p.x >= 0.0 { max.x } else { min.x },
                if p.y >= 0.0 { max.y } else { min.y },
                if p.z >= 0.0 { max.z } else { min.z },
            );
            p.truncate().dot(c) + p.w >= 0.0
        })
    }
}

impl Planes {}

/// Where a box of sphere centres lies against a volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    /// Every sphere of the box is wholly outside one plane: none is in the pass.
    Outside,
    /// Every centre is on the inner side of every plane: all are in the pass.
    Inside,
    Straddle,
}

impl Planes {
    /// Classifies the spheres whose centres lie in `[min, max]` and whose radii lie in `0..=rmax`.
    /// The margin `tol` is far above the rounding of the dot products, so a box called `Outside` or
    /// `Inside` gives what the test of each sphere gives; anything unclear is `Straddle`.
    fn classify(&self, min: Vec3, max: Vec3, rmax: f32) -> Side {
        let mut inside = true;
        for p in &self.planes[..self.count] {
            let n = p.truncate();
            if !(n.is_finite() && p.w.is_finite()) {
                return Side::Straddle;
            }
            // The corners of the box that are furthest along the normal and furthest against it.
            let hi = n.max(Vec3::ZERO);
            let lo = n.min(Vec3::ZERO);
            let top = hi.dot(max) + lo.dot(min) + p.w;
            let bottom = hi.dot(min) + lo.dot(max) + p.w;
            let reach = n.abs().dot(min.abs().max(max.abs())) + p.w.abs() + rmax;
            let tol = reach * 1e-5 + 1e-6;
            if top < -rmax - tol {
                return Side::Outside;
            }
            inside &= bottom >= tol;
        }
        if inside { Side::Inside } else { Side::Straddle }
    }
}

/// Items per box of the hierarchical test of [`ModelSphere::masks_into`].
const CHUNK: usize = 32;

/// The widest box of centres (along one axis) that gets the test of the whole box.
const COMPACT: f32 = 24.0;

/// What the passes of a frame cull against. `None` for a pass keeps everything in it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CullSet {
    pub main: Option<Planes>,
    pub shadow: Option<Planes>,
    pub reflection: Option<Planes>,
    /// The far-view LOD (`FarLOD`, [`crate::world::far_lod`]): the eye and the distance beyond
    /// which an instance leaves the shadow map and the reflection. `None` keeps them.
    pub far: Option<(Vec3, f32)>,
    /// A low eye: the instances are scaled about their origin and the ones at the eye leave the main
    /// view. `None`: the strategic frame, every instance as it is.
    pub near: Option<NearEye>,
}

/// The objects around a low eye ([`crate::camera::ObjectsNearEye`], in our basis).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NearEye {
    pub eye: Vec3,
    /// Every instance is drawn `scale` times its size about its origin (the translation of its matrix).
    pub scale: f32,
    /// An instance whose sphere (scaled) comes nearer `eye` than this is not in the main view.
    pub clearance: f32,
}

impl NearEye {
    /// The instance drawn `scale` times its size about its origin: `M * S(scale)`.
    pub fn scaled(&self, item: &ObjectInstance) -> ObjectInstance {
        let mut o = *item;
        for row in &mut o.world {
            for v in &mut row[..3] {
                *v *= self.scale;
            }
        }
        o
    }

    /// Whether a sphere (already scaled) comes nearer the eye than the clearance.
    fn at_eye(&self, centre: Vec3, radius: f32) -> bool {
        centre.distance(self.eye) - radius < self.clearance
    }
}

impl CullSet {
    /// From the matrices that take a point of our basis to clip space.
    pub fn new(main: Option<Mat4>, shadow: Option<Mat4>, reflection: Option<Mat4>) -> Self {
        Self {
            main: main.as_ref().map(Planes::frustum),
            shadow: shadow.as_ref().map(Planes::sides),
            reflection: reflection.as_ref().map(Planes::sides),
            far: None,
            near: None,
        }
    }

    /// The same volumes, for a low eye: see [`NearEye`].
    pub fn with_near(self, near: NearEye) -> Self {
        Self {
            near: Some(near),
            ..self
        }
    }

    /// The same volumes; what is wholly farther than `distance` from `eye` casts no shadow and is
    /// not in the reflection.
    pub fn with_far(self, eye: Vec3, distance: f32) -> Self {
        Self {
            far: Some((eye, distance)),
            ..self
        }
    }

    /// True when nothing is culled.
    pub fn is_off(&self) -> bool {
        self.main.is_none()
            && self.shadow.is_none()
            && self.reflection.is_none()
            && self.far.is_none()
            && self.near.is_none()
    }
}

/// Whether a sphere is wholly farther than the far-view distance.
fn is_far(far: Option<(Vec3, f32)>, centre: Vec3, radius: f32) -> bool {
    far.is_some_and(|(eye, d)| centre.distance(eye) - radius > d)
}

/// The bits a far instance loses.
const FAR_DROPS: u8 = SHADOW | REFLECTION;

/// The bits an instance at a low eye loses: the view (its shadow and its reflection stay).
const NEAR_DROPS: u8 = MAIN;

/// The passes an instance is in: bits of the mask.
pub const MAIN: u8 = 1;
pub const SHADOW: u8 = 2;
pub const REFLECTION: u8 = 4;

/// The sphere of a model in its own space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelSphere {
    pub centre: Vec3,
    pub radius: f32,
}

impl ModelSphere {
    /// The centre and the radius of the sphere of `item` moved by `shift` in X; with a low eye, of the instance
    /// scaled about its origin (`near`).
    fn place(&self, item: &ObjectInstance, shift: f32, near: Option<&NearEye>) -> (Vec3, f32) {
        let c = item.transform_point(self.centre.to_array());
        let radius = self.radius * item_scale(item);
        match near {
            None => (Vec3::new(c[0] + shift, c[1], c[2]), radius),
            Some(n) => {
                let o = Vec3::new(item.world[0][3], item.world[1][3], item.world[2][3]);
                let c = o + (Vec3::from(c) - o) * n.scale;
                (Vec3::new(c.x + shift, c.y, c.z), radius * n.scale.abs())
            }
        }
    }

    /// An infinite sphere keeps the instance in every pass.
    pub const ALL: ModelSphere = ModelSphere {
        centre: Vec3::ZERO,
        radius: f32::INFINITY,
    };

    /// The sphere of a box `[min xyz, max xyz]`. `animated` models grow it: a skinned vertex or a
    /// bone-attached mesh leaves the box of the bind pose.
    pub fn of_box(bounds: [f32; 6], animated: bool) -> Self {
        let lo = Vec3::new(bounds[0], bounds[1], bounds[2]);
        let hi = Vec3::new(bounds[3], bounds[4], bounds[5]);
        if !(lo.is_finite() && hi.is_finite()) || lo.cmpgt(hi).any() {
            return Self::ALL;
        }
        let centre = (lo + hi) * 0.5;
        let half = (hi - lo).length() * 0.5;
        // A margin for what the vertex stage moves, and for the poses of the clips.
        let radius = if animated {
            half * 2.0 + 0.5
        } else {
            half * 1.1 + 0.25
        };
        Self { centre, radius }
    }

    /// The passes `item` (moved by `shift` in X) is in.
    pub fn passes(&self, item: &ObjectInstance, shift: f32, cull: &CullSet) -> u8 {
        if self.radius.is_infinite() {
            return MAIN | SHADOW | REFLECTION;
        }
        let (centre, radius) = self.place(item, shift, cull.near.as_ref());
        let mut m = 0;
        if cull.main.is_none_or(|p| p.sees(centre, radius)) {
            m |= MAIN;
        }
        // The shadow map is drawn for the middle copy of the map only.
        if shift.abs() < 1e-3 && cull.shadow.is_none_or(|p| p.sees(centre, radius)) {
            m |= SHADOW;
        }
        if cull.reflection.is_none_or(|p| p.sees(centre, radius)) {
            m |= REFLECTION;
        }
        if is_far(cull.far, centre, radius) {
            m &= !FAR_DROPS;
        }
        if cull.near.is_some_and(|n| n.at_eye(centre, radius)) {
            m &= !NEAR_DROPS;
        }
        m
    }
}

impl ModelSphere {
    /// The masks `passes` gives for every item, into `out` (cleared first). The centre and the
    /// radius are made once per item; the items go in boxes of [`CHUNK`], and a box wholly out of a
    /// volume (or wholly in it) settles its items without a test of their own.
    pub fn masks_into(
        &self,
        items: &[ObjectInstance],
        shift: f32,
        cull: &CullSet,
        out: &mut Vec<u8>,
    ) {
        self.volume_masks_into(items, shift, cull, out);
        if let Some(far) = cull.far {
            self.drop_far(items, shift, far, cull.near.as_ref(), out);
        }
        if let Some(near) = &cull.near {
            self.drop_near(items, shift, near, out);
        }
    }

    /// A low eye on the masks of `items`: the instances whose sphere comes near it leave the main view. Only the
    /// items in the view are tested (a low eye's neighbourhood is a few of them).
    fn drop_near(&self, items: &[ObjectInstance], shift: f32, near: &NearEye, out: &mut [u8]) {
        if self.radius.is_infinite() {
            return;
        }
        for (item, m) in items.iter().zip(out.iter_mut()) {
            if *m & NEAR_DROPS == 0 {
                continue;
            }
            let (centre, radius) = self.place(item, shift, Some(near));
            if near.at_eye(centre, radius) {
                *m &= !NEAR_DROPS;
            }
        }
    }

    /// The far-view LOD on the masks of `items`: the far ones lose their shadow and reflection.
    /// Boxes of [`CHUNK`] items wholly near (or wholly far) settle without a test of their own.
    fn drop_far(
        &self,
        items: &[ObjectInstance],
        shift: f32,
        far: (Vec3, f32),
        near: Option<&NearEye>,
        out: &mut [u8],
    ) {
        if self.radius.is_infinite() {
            return;
        }
        let (eye, d) = far;
        let mut centres = [Vec3::ZERO; CHUNK];
        let mut radii = [0.0f32; CHUNK];
        for (n, chunk) in items.chunks(CHUNK).enumerate() {
            let masks = &mut out[n * CHUNK..n * CHUNK + chunk.len()];
            if masks.iter().all(|m| m & FAR_DROPS == 0) {
                continue;
            }
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            let (mut rmax, mut sane) = (0.0f32, true);
            for (k, item) in chunk.iter().enumerate() {
                (centres[k], radii[k]) = self.place(item, shift, near);
                lo = lo.min(centres[k]);
                hi = hi.max(centres[k]);
                rmax = rmax.max(radii[k]);
                sane &= centres[k].is_finite() && radii[k].is_finite();
            }
            let tol = 1e-3 * d.abs() + 1e-3;
            let near = crate::world::far_lod::box_distance(eye, lo, hi) - rmax;
            let reach = crate::world::far_lod::box_reach(eye, lo, hi);
            if !(sane && near.is_finite() && reach.is_finite())
                || (reach >= d - tol && near <= d + tol)
            {
                for (k, m) in masks.iter_mut().enumerate() {
                    if is_far(Some(far), centres[k], radii[k]) {
                        *m &= !FAR_DROPS;
                    }
                }
            } else if near > d + tol {
                for m in masks.iter_mut() {
                    *m &= !FAR_DROPS;
                }
            }
        }
    }

    /// The masks of the volumes alone (the far-view LOD aside).
    fn volume_masks_into(
        &self,
        items: &[ObjectInstance],
        shift: f32,
        cull: &CullSet,
        out: &mut Vec<u8>,
    ) {
        out.clear();
        if self.radius.is_infinite() {
            out.resize(items.len(), MAIN | SHADOW | REFLECTION);
            return;
        }
        // The volumes to test, with their bit; a pass with no volume keeps every item.
        let shadow_on = shift.abs() < 1e-3;
        let mut fixed = 0;
        let mut tests: [(u8, Planes); 3] = [(0, Planes::default()); 3];
        let mut n = 0;
        for (bit, planes, on) in [
            (MAIN, cull.main, true),
            (SHADOW, cull.shadow, shadow_on),
            (REFLECTION, cull.reflection, true),
        ] {
            if !on {
                continue;
            }
            match planes {
                None => fixed |= bit,
                Some(p) => {
                    tests[n] = (bit, p);
                    n += 1;
                }
            }
        }
        out.reserve(items.len());
        if n == 0 {
            out.resize(items.len(), fixed);
            return;
        }
        let tests = &tests[..n];
        let mut centres = [Vec3::ZERO; CHUNK];
        let mut radii = [0.0f32; CHUNK];
        for chunk in items.chunks(CHUNK) {
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            let (mut rmax, mut sane) = (0.0f32, true);
            for (k, item) in chunk.iter().enumerate() {
                let (centre, radius) = self.place(item, shift, cull.near.as_ref());
                centres[k] = centre;
                radii[k] = radius;
                // Huge or odd values: the boxes lose their meaning, test the items one by one.
                sane &= centre.is_finite()
                    && centre.abs().max_element() < 1e12
                    && (0.0..1e12).contains(&radius);
                lo = lo.min(centre);
                hi = hi.max(centre);
                rmax = rmax.max(radius);
            }
            // A box that spreads over the map straddles every volume: do not spend the test on it.
            let compact = sane && (hi - lo).max_element() <= COMPACT;
            let mut all = fixed;
            let mut open: [(u8, &Planes); 3] = [(0, &tests[0].1); 3];
            let mut opens = 0;
            for (bit, planes) in tests {
                let side = if compact {
                    planes.classify(lo, hi, rmax)
                } else {
                    Side::Straddle
                };
                match side {
                    Side::Outside => {}
                    Side::Inside => all |= bit,
                    Side::Straddle => {
                        open[opens] = (*bit, planes);
                        opens += 1;
                    }
                }
            }
            if opens == 0 {
                out.resize(out.len() + chunk.len(), all);
                continue;
            }
            for k in 0..chunk.len() {
                let mut m = all;
                for (bit, planes) in &open[..opens] {
                    if planes.sees(centres[k], radii[k]) {
                        m |= bit;
                    }
                }
                out.push(m);
            }
        }
    }
}

/// The largest scale of the matrix of an instance.
fn item_scale(i: &ObjectInstance) -> f32 {
    let col = |c: usize| {
        let (a, b, d) = (i.world[0][c], i.world[1][c], i.world[2][c]);
        a * a + b * b + d * d
    };
    col(0).max(col(1)).max(col(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item_at(x: f32, y: f32, z: f32, scale: f32) -> ObjectInstance {
        ObjectInstance {
            world: [
                [scale, 0.0, 0.0, x],
                [0.0, scale, 0.0, y],
                [0.0, 0.0, scale, z],
            ],
            ..Default::default()
        }
    }

    /// A view from above over x and z in -10..10 (our basis, Y up).
    fn top_view() -> Mat4 {
        // x and z over -10..10 to clip x and y; the height y from 100 (depth 0) to -100 (depth 1).
        Mat4::from_cols(
            Vec4::new(0.1, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, -0.005, 0.0),
            Vec4::new(0.0, -0.1, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.5, 1.0),
        )
    }

    #[test]
    fn a_sphere_that_touches_the_view_is_kept_and_one_clear_of_it_is_not() {
        let cull = CullSet::new(Some(top_view()), None, None);
        let s = ModelSphere::of_box([-1.0, 0.0, -1.0, 1.0, 2.0, 1.0], false);
        let inside = s.passes(&item_at(0.0, 0.0, 0.0, 1.0), 0.0, &cull);
        assert_eq!(inside & MAIN, MAIN);
        // Centre 11 out: the radius (about 1.9) reaches over the edge at 10.
        let touching = s.passes(&item_at(11.0, 0.0, 0.0, 1.0), 0.0, &cull);
        assert_eq!(touching & MAIN, MAIN);
        let outside = s.passes(&item_at(14.0, 0.0, 0.0, 1.0), 0.0, &cull);
        assert_eq!(outside & MAIN, 0);
        // A scale makes the sphere bigger: at 3 times it reaches back in.
        let big = s.passes(&item_at(14.0, 0.0, 0.0, 3.0), 0.0, &cull);
        assert_eq!(big & MAIN, MAIN);
        // The shift of a map copy moves the instance.
        let shifted = s.passes(&item_at(14.0, 0.0, 0.0, 1.0), -10.0, &cull);
        assert_eq!(shifted & MAIN, MAIN);
    }

    #[test]
    fn the_side_planes_of_an_orthographic_view_ignore_the_depth() {
        let cull = CullSet::new(None, Some(top_view()), Some(top_view()));
        let s = ModelSphere::of_box([-1.0, 0.0, -1.0, 1.0, 2.0, 1.0], true);
        // Far above and far below the near and far planes, but inside in x and z.
        for y in [-5000.0, 5000.0] {
            let m = s.passes(&item_at(0.0, y, 0.0, 1.0), 0.0, &cull);
            assert_eq!(m, MAIN | SHADOW | REFLECTION, "y {y}");
        }
        let out = s.passes(&item_at(30.0, 0.0, 0.0, 1.0), 0.0, &cull);
        assert_eq!(out, MAIN, "no main volume keeps it in the main pass only");
        // The shadow map is for the middle copy.
        let copy = s.passes(&item_at(0.0, 0.0, 0.0, 1.0), 25.0, &cull);
        assert_eq!(copy & SHADOW, 0);
    }

    /// The far-view LOD: a far instance stays in the main view and leaves the shadow map and the
    /// reflection; one the distance cuts through keeps them.
    #[test]
    fn a_far_instance_keeps_its_model_and_loses_its_shadow_and_reflection() {
        let cull = CullSet::new(None, None, None).with_far(Vec3::ZERO, 100.0);
        let s = ModelSphere::of_box([-1.0, 0.0, -1.0, 1.0, 2.0, 1.0], false);
        assert_eq!(
            s.passes(&item_at(50.0, 0.0, 0.0, 1.0), 0.0, &cull),
            MAIN | SHADOW | REFLECTION
        );
        assert_eq!(s.passes(&item_at(150.0, 0.0, 0.0, 1.0), 0.0, &cull), MAIN);
        // The sphere (radius about 1.9) reaches back over the distance.
        assert_eq!(
            s.passes(&item_at(101.0, 0.0, 0.0, 1.0), 0.0, &cull),
            MAIN | SHADOW | REFLECTION
        );
        let mut out = Vec::new();
        let items: Vec<ObjectInstance> =
            (0..200).map(|k| item_at(k as f32, 0.0, 0.0, 1.0)).collect();
        s.masks_into(&items, 0.0, &cull, &mut out);
        assert!(out[..100].iter().all(|&m| m == MAIN | SHADOW | REFLECTION));
        assert!(out[110..].iter().all(|&m| m == MAIN));
    }

    /// A small deterministic generator for the tests.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }

        fn range(&mut self, lo: f32, hi: f32) -> f32 {
            lo + (hi - lo) * self.next()
        }
    }

    fn random_item(rng: &mut Rng, at: Vec3, spread: f32) -> ObjectInstance {
        let pos = at
            + Vec3::new(
                rng.range(-1.0, 1.0),
                rng.range(-1.0, 1.0),
                rng.range(-1.0, 1.0),
            ) * spread;
        let axis = Vec3::new(
            rng.range(-1.0, 1.0),
            rng.range(-1.0, 1.0),
            rng.range(-1.0, 1.0),
        );
        let rot = glam::Quat::from_axis_angle(axis.normalize_or(Vec3::Y), rng.range(0.0, 6.3));
        let kind = rng.next();
        let scale = if kind < 0.03 {
            0.0
        } else if kind < 0.06 {
            1e30
        } else if kind < 0.08 {
            f32::NAN
        } else {
            rng.range(0.1, 4.0)
        };
        let s = Vec3::new(
            scale,
            scale * rng.range(0.5, 1.5),
            scale * rng.range(0.5, 1.5),
        );
        let m = glam::Mat4::from_scale_rotation_translation(s, rot, pos);
        let mut w = [[0.0; 4]; 3];
        for (r, row) in w.iter_mut().enumerate() {
            for (c, v) in row.iter_mut().enumerate() {
                *v = m.col(c)[r];
            }
        }
        if rng.next() < 0.01 {
            w[0][3] = f32::NAN;
        }
        ObjectInstance {
            world: w,
            ..Default::default()
        }
    }

    fn random_items(rng: &mut Rng, n: usize, area: f32) -> Vec<ObjectInstance> {
        let mut items = Vec::new();
        while items.len() < n {
            // Groups that sit together, so that whole boxes land in or out of a volume.
            let at = Vec3::new(
                rng.range(-area, area),
                rng.range(-5.0, 5.0),
                rng.range(-area, area),
            );
            let spread = if rng.next() < 0.5 { 0.5 } else { area * 0.4 };
            for _ in 0..CHUNK {
                items.push(random_item(rng, at, spread));
            }
        }
        items.truncate(n);
        items
    }

    /// A view from above over the rectangle of half sizes `hx`, `hz` around `(cx, cz)`.
    fn top_ortho(cx: f32, cz: f32, hx: f32, hz: f32) -> Mat4 {
        Mat4::from_cols(
            Vec4::new(1.0 / hx, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, -0.005, 0.0),
            Vec4::new(0.0, -1.0 / hz, 0.0, 0.0),
            Vec4::new(-cx / hx, cz / hz, 0.5, 1.0),
        )
    }

    /// A random perspective (depth 0 to 1, looking down at the ground) or orthographic view.
    fn random_view(rng: &mut Rng) -> Mat4 {
        let turn = Mat4::from_rotation_y(rng.range(0.0, 6.3));
        if rng.next() < 0.5 {
            let eye = Vec3::new(
                rng.range(-30.0, 30.0),
                rng.range(20.0, 60.0),
                rng.range(-30.0, 30.0),
            );
            let view =
                Mat4::from_rotation_x(rng.range(0.5, 1.4)) * turn * Mat4::from_translation(-eye);
            let f = 1.0 / (rng.range(0.4, 1.2) * 0.5).tan();
            let aspect = rng.range(0.8, 2.0);
            let (near, far) = (1.0f32, 300.0f32);
            let proj = Mat4::from_cols(
                Vec4::new(f / aspect, 0.0, 0.0, 0.0),
                Vec4::new(0.0, f, 0.0, 0.0),
                Vec4::new(0.0, 0.0, far / (near - far), -1.0),
                Vec4::new(0.0, 0.0, near * far / (near - far), 0.0),
            );
            proj * view
        } else {
            let (w, h) = (rng.range(5.0, 60.0), rng.range(5.0, 40.0));
            top_ortho(rng.range(-20.0, 20.0), rng.range(-20.0, 20.0), w, h) * turn
        }
    }

    #[test]
    fn the_batched_masks_are_the_masks_of_each_item() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut out = Vec::new();
        let mut seen = [0usize; 8];
        for round in 0..60 {
            let items = random_items(&mut rng, 700, 60.0);
            let cull = match round % 4 {
                0 => CullSet::new(
                    Some(random_view(&mut rng)),
                    Some(random_view(&mut rng)),
                    Some(random_view(&mut rng)),
                ),
                1 => CullSet::new(
                    Some(random_view(&mut rng)),
                    None,
                    Some(random_view(&mut rng)),
                ),
                2 => CullSet::new(None, Some(random_view(&mut rng)), None),
                _ => CullSet::new(
                    Some(random_view(&mut rng)),
                    Some(random_view(&mut rng)),
                    None,
                ),
            };
            // Half the rounds with the far-view LOD, its distance inside the spread of the items.
            let cull = if round % 8 >= 4 {
                let eye = Vec3::new(rng.range(-30.0, 30.0), 40.0, rng.range(-30.0, 30.0));
                cull.with_far(eye, rng.range(10.0, 90.0))
            } else {
                cull
            };
            // A low eye in some rounds (with and without the far LOD): scaled instances, the ones at the eye
            // out of the view.
            let cull = if round % 3 == 1 {
                cull.with_near(NearEye {
                    eye: Vec3::new(rng.range(-30.0, 30.0), 3.0, rng.range(-30.0, 30.0)),
                    scale: rng.range(0.2, 1.5),
                    clearance: rng.range(0.0, 20.0),
                })
            } else {
                cull
            };
            for bounds in [
                [-1.0, 0.0, -1.0, 1.0, 2.0, 1.0],
                [0.0; 6],
                [-3.0, -1.0, 0.0, 5.0, 9.0, 2.0],
            ] {
                for animated in [false, true] {
                    let s = ModelSphere::of_box(bounds, animated);
                    for shift in [0.0, 100.0, -100.0, 5e-4, f32::NAN] {
                        s.masks_into(&items, shift, &cull, &mut out);
                        assert_eq!(out.len(), items.len());
                        for (i, item) in items.iter().enumerate() {
                            let want = s.passes(item, shift, &cull);
                            assert_eq!(out[i], want, "round {round} item {i} shift {shift}");
                            seen[usize::from(want)] += 1;
                        }
                    }
                }
            }
        }
        // The test has seen items in and out of the passes.
        assert!(seen[0] > 100 && seen[7] > 100, "{seen:?}");
        // A model with no bounds keeps all.
        let all = ModelSphere::ALL;
        let items = random_items(&mut rng, 70, 60.0);
        all.masks_into(&items, 0.0, &CullSet::default(), &mut out);
        assert!(out.iter().all(|&m| m == 7) && out.len() == 70);
    }

    #[test]
    fn the_batched_masks_agree_at_the_edge_of_a_volume() {
        let cull = CullSet::new(Some(top_view()), Some(top_view()), Some(top_view()));
        let mut out = Vec::new();
        for scale in [1.0, 0.37, 3.3, 0.0] {
            for s in [
                ModelSphere::of_box([-1.0, 0.0, -1.0, 1.0, 2.0, 1.0], false),
                ModelSphere {
                    centre: Vec3::new(0.3, 0.1, -0.2),
                    radius: 0.0,
                },
                ModelSphere {
                    centre: Vec3::ZERO,
                    radius: -1.0,
                },
            ] {
                // The edge at x = 10 (and the same on z): the centre goes over it one step at a time.
                let edge = 10.0 + s.radius * scale;
                let mut x = edge - 4e-5 * edge.abs().max(1.0);
                let mut items = Vec::new();
                for _ in 0..300 {
                    items.push(item_at(x, 0.0, 0.0, scale));
                    items.push(item_at(0.0, 0.0, -x, scale));
                    x = f32::from_bits(x.to_bits() + 1);
                }
                s.masks_into(&items, 0.0, &cull, &mut out);
                for (i, item) in items.iter().enumerate() {
                    assert_eq!(out[i], s.passes(item, 0.0, &cull), "scale {scale} item {i}");
                }
            }
        }
    }

    /// `cargo test -p render --release --lib bench_cull_masks -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_cull_masks() {
        let mut rng = Rng(0x1234_5678_9abc_def1);
        let mut items: Vec<ObjectInstance> = (0..14_000)
            .map(|_| {
                let (x, z) = (rng.range(-200.0, 200.0), rng.range(-125.0, 125.0));
                item_at(x, 0.0, z, rng.range(0.5, 3.0))
            })
            .collect();
        // A view over x -200..0 and z -62..62 is a quarter of the area.
        let view = top_ortho(-100.0, 0.0, 100.0, 62.5);
        let cull = CullSet::new(Some(view), Some(view), Some(view));
        let s = ModelSphere::of_box([-1.0, 0.0, -1.0, 1.0, 2.0, 1.0], true);
        let mut out = Vec::new();
        let mut old = Vec::new();
        // Items in the order they were made (no order in space), then in order of the place.
        for label in ["scattered", "ordered"] {
            if label == "ordered" {
                items.sort_by(|a, b| {
                    let key = |i: &ObjectInstance| {
                        (i.world[0][3] / 20.0).floor() * 1000.0 + i.world[2][3]
                    };
                    key(a).total_cmp(&key(b))
                });
            }
            let (mut t_old, mut t_new) = (f64::MAX, f64::MAX);
            for _ in 0..200 {
                let t = std::time::Instant::now();
                old.clear();
                old.extend(items.iter().map(|i| s.passes(i, 0.0, &cull)));
                t_old = t_old.min(t.elapsed().as_secs_f64() * 1e6);
                std::hint::black_box(&old);
                let t = std::time::Instant::now();
                s.masks_into(&items, 0.0, &cull, &mut out);
                t_new = t_new.min(t.elapsed().as_secs_f64() * 1e6);
                std::hint::black_box(&out);
            }
            assert_eq!(old, out);
            let seen = out.iter().filter(|&&m| m & MAIN != 0).count();
            println!(
                "cull masks, 14000 items {label} ({seen} in main): per-item {t_old:.0} us, batched {t_new:.0} us"
            );
        }
    }

    /// A low eye (a mod's first-person camera): an instance is scaled about its origin, the point it stands on;
    /// one whose scaled sphere comes nearer the eye than the clearance leaves the main view and keeps its
    /// shadow and reflection; a big one that would reach the eye at the map's scale stays at half of it.
    #[test]
    fn a_low_eye_scales_the_instances_about_their_origin_and_leaves_out_the_ones_at_it() {
        let near = NearEye {
            eye: Vec3::new(0.0, 2.0, 0.0),
            scale: 0.5,
            clearance: 2.0,
        };
        // The scaled matrix: the origin stays, every other point comes half the way to it.
        let item = item_at(30.0, 4.0, -10.0, 3.0);
        let s = near.scaled(&item);
        assert_eq!(s.transform_point([0.0; 3]), [30.0, 4.0, -10.0]);
        assert_eq!(s.transform_point([2.0, 4.0, 0.0]), [33.0, 10.0, -10.0]);
        assert_eq!(s.color, item.color);

        let cull = CullSet::new(None, None, None).with_near(near);
        assert!(!cull.is_off());
        // A box 2 wide and 2 tall: sphere radius about 2.2 at scale 1.
        let sphere = ModelSphere::of_box([-1.0, 0.0, -1.0, 1.0, 2.0, 1.0], false);
        let all = MAIN | SHADOW | REFLECTION;
        // At the eye's feet: out of the view, its shadow and reflection stay.
        assert_eq!(
            sphere.passes(&item_at(0.5, 0.0, 0.5, 1.0), 0.0, &cull),
            SHADOW | REFLECTION
        );
        // Ten units off: in.
        assert_eq!(
            sphere.passes(&item_at(10.0, 0.0, 0.0, 1.0), 0.0, &cull),
            all
        );
        // A figure 4 times the size 9 off: its sphere (about 8.6) reaches the eye's clearance at the map's
        // scale; scaled by a half (4.3, its centre half as high) it does not.
        let big = item_at(9.0, 0.0, 0.0, 4.0);
        assert_eq!(sphere.passes(&big, 0.0, &cull), all);
        let unscaled = CullSet::new(None, None, None).with_near(NearEye { scale: 1.0, ..near });
        assert_eq!(sphere.passes(&big, 0.0, &unscaled), SHADOW | REFLECTION);
        // A copy of the map: the shift moves the instance away from the eye.
        assert_eq!(
            sphere.passes(&item_at(0.5, 0.0, 0.5, 1.0), 100.0, &cull) & MAIN,
            MAIN
        );
        // The batched masks agree.
        let items: Vec<ObjectInstance> = (0..100)
            .map(|k| item_at(k as f32 * 0.25 - 12.0, 0.0, 0.0, 1.0 + (k % 5) as f32))
            .collect();
        let mut out = Vec::new();
        sphere.masks_into(&items, 0.0, &cull, &mut out);
        for (i, item) in items.iter().enumerate() {
            assert_eq!(out[i], sphere.passes(item, 0.0, &cull), "item {i}");
        }
        assert!(out.contains(&(SHADOW | REFLECTION)));
        assert!(out.contains(&all));
        // No near eye: the strategic masks, every instance as it is.
        let plain = CullSet::new(None, None, None);
        assert_eq!(
            sphere.passes(&item_at(0.5, 0.0, 0.5, 1.0), 0.0, &plain),
            all
        );
    }

    #[test]
    fn no_volume_and_a_model_without_bounds_keep_everything() {
        let off = CullSet::default();
        assert!(off.is_off());
        let s = ModelSphere::of_box([0.0; 6], false);
        assert_eq!(
            s.passes(&item_at(1e6, 0.0, 0.0, 1.0), 0.0, &off),
            MAIN | SHADOW | REFLECTION
        );
        let bad = ModelSphere::of_box([f32::NAN; 6], false);
        assert!(bad.radius.is_infinite());
    }
}
