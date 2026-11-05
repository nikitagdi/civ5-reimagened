//! Game camera: fixed pitch, focus point on the ground, continuous zoom
//! level. Pure math, no GPU. World: Y up, north is -Z, east is +X (see `hexgrid`).

use glam::{Mat4, Vec2, Vec3};

use crate::camera::CameraView;
use crate::camera_params as p;
use crate::hexgrid::{GridShape, HEX_WIDTH, ROW_STEP};

/// Where the focus may go: the padded world of the map and, once the fog is known, the box of the
/// plots the player has revealed grown by one hex. With `wrap_width` X is wrapped, otherwise
/// clamped.
// re: CvCamera::StartupGame 0x088339e8 (bounds = g_WorldMin/Max), CvCamera::OnFOWChanged 0x088b2f9a
// (a revealed plot grows the bounds to its world point +-64 in x, +-55.424 in y, clamped to the
// world), PositionIsWithinBounds 0x088afe22 (the test is on the focus, the view axis at z = 0)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapBounds {
    /// Smallest allowed focus `(X, Z)` of the world.
    pub min: Vec2,
    /// Largest allowed focus `(X, Z)` of the world.
    pub max: Vec2,
    /// World period in X, if the map wraps.
    pub wrap_width: Option<f32>,
    /// The box `(min, max)` of the revealed plots grown by [`EXPLORED_MARGIN`], inside the world;
    /// `None` before the fog is known.
    pub explored: Option<(Vec2, Vec2)>,
}

/// How far the bounds reach past the centre of a revealed plot: a hex across, a row along.
// re: CvCamera::OnFOWChanged 0x088b2f9a (64.0, 55.424 = 64 * 0.866)
pub const EXPLORED_MARGIN: Vec2 = Vec2::new(64.0, 55.424);

impl MapBounds {
    /// The world of the map: its cells and the padding of the original, two hexes on each side
    /// across (when the map does not wrap) and two rows at each end.
    // re: WorldGlobals::GetPaddedMapSize 0x086725b4 ((W, H + 4) wrapping, (W + 4, H + 4) not),
    // SetupWorldGlobals 0x08672ce8 (g_WorldMin/Max = -+size / 2, centred on the padded map)
    pub fn from_grid(shape: &GridShape) -> Self {
        let w = shape.width.max(0) as f32;
        let h = shape.height.max(0) as f32;
        Self {
            min: Vec2::new(-2.0 * HEX_WIDTH, -ROW_STEP * (h + 2.0)),
            max: Vec2::new(HEX_WIDTH * (w + 2.0), 2.0 * ROW_STEP),
            wrap_width: shape.wrap_x.then(|| shape.world_width()),
            explored: None,
        }
    }

    /// The same world with the box of the revealed plots whose centres are `revealed`; no plot:
    /// `None` (nothing known yet, the world bounds the camera).
    pub fn with_explored(mut self, revealed: impl IntoIterator<Item = Vec2>) -> Self {
        let mut boxed: Option<(Vec2, Vec2)> = None;
        for c in revealed {
            let (lo, hi) = (c - EXPLORED_MARGIN, c + EXPLORED_MARGIN);
            boxed = Some(boxed.map_or((lo, hi), |(a, b)| (a.min(lo), b.max(hi))));
        }
        self.explored = boxed.map(|(lo, hi)| {
            let lo = lo.clamp(self.min, self.max);
            let hi = hi.clamp(self.min, self.max);
            (lo, hi)
        });
        self
    }

    /// The X range allowed: the explored one, else the world's.
    fn x_range(&self) -> (f32, f32) {
        self.explored
            .map_or((self.min.x, self.max.x), |(a, b)| (a.x, b.x))
    }

    fn y_range(&self) -> (f32, f32) {
        self.explored
            .map_or((self.min.y, self.max.y), |(a, b)| (a.y, b.y))
    }

    /// `x` (wrapped into the period) inside `lo..=hi`, trying the copies one period off.
    fn wrapped_in(x: f32, w: f32, lo: f32, hi: f32) -> Option<f32> {
        [x, x - w, x + w]
            .into_iter()
            .find(|v| (lo..=hi).contains(v))
    }

    /// True if the focus may stand at `focus`.
    pub fn contains(&self, focus: Vec2) -> bool {
        let (xl, xh) = self.x_range();
        let (yl, yh) = self.y_range();
        let x_ok = match self.wrap_width {
            Some(_) if self.explored.is_none() => true,
            Some(w) => Self::wrapped_in(focus.x.rem_euclid(w), w, xl, xh).is_some(),
            None => (xl..=xh).contains(&focus.x),
        };
        x_ok && (yl..=yh).contains(&focus.y)
    }

    fn apply(&self, mut focus: Vec2) -> Vec2 {
        let (xl, xh) = self.x_range();
        let (yl, yh) = self.y_range();
        focus.x = match self.wrap_width {
            Some(w) => {
                let x = focus.x.rem_euclid(w);
                if self.explored.is_none() || Self::wrapped_in(x, w, xl, xh).is_some() {
                    x
                } else {
                    // The nearer edge across the seam.
                    let gap = |e: f32| {
                        let d = (x - e).rem_euclid(w);
                        d.min(w - d)
                    };
                    let e = if gap(xl) <= gap(xh) { xl } else { xh };
                    e.rem_euclid(w)
                }
            }
            None => focus.x.clamp(xl, xh),
        };
        focus.y = focus.y.clamp(yl, yh);
        focus
    }
}
/// A world-space ray with a unit direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

impl Ray {
    /// Point where the ray crosses the plane `y = plane_y`, if in front of the origin.
    pub fn at_height(&self, plane_y: f32) -> Option<Vec3> {
        if self.dir.y.abs() < 1e-6 {
            return None;
        }
        let t = (plane_y - self.origin.y) / self.dir.y;
        (t >= 0.0).then(|| self.origin + self.dir * t)
    }
}

/// Pixel position (Y down) to normalised device coordinates (Y up, -1..1).
pub fn pixels_to_ndc(x: f32, y: f32, width: f32, height: f32) -> Vec2 {
    Vec2::new(
        2.0 * x / width.max(1.0) - 1.0,
        1.0 - 2.0 * y / height.max(1.0),
    )
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GameCamera {
    /// Ground point `(X, Z)` in the middle of the screen, at height `FOCUS_HEIGHT`.
    focus: Vec2,
    level: f32,
    /// Where the camera is going: a zoom or a look-at sets this at once and `update` follows it
    /// (`CvCamera::m_vPosition`, the real camera eases toward it).
    target_focus: Vec2,
    target_level: f32,
    bounds: Option<MapBounds>,
    /// Glide after a drag, ground units per second at the start (`CvCamera::m_vVelocity`).
    velocity: Vec2,
    /// Seconds since the glide started and its length (`m_fInertiaStartTime`, `m_fInertiaDuration`).
    glide_elapsed: f32,
    glide_duration: f32,
}

impl GameCamera {
    pub fn new(focus: Vec2) -> Self {
        Self {
            focus,
            level: p::START_LEVEL,
            target_focus: focus,
            target_level: p::START_LEVEL,
            bounds: None,
            velocity: Vec2::ZERO,
            glide_elapsed: 0.0,
            glide_duration: 0.0,
        }
    }

    pub fn focus(&self) -> Vec2 {
        self.focus
    }

    pub fn level(&self) -> f32 {
        self.level
    }

    /// Camera height above the focus.
    pub fn height(&self) -> f32 {
        p::height_for_level(self.level)
    }

    pub fn bounds(&self) -> Option<MapBounds> {
        self.bounds
    }

    /// The world of a map; a new world forgets the revealed box of the old one, the same world
    /// keeps it.
    pub fn set_bounds(&mut self, bounds: Option<MapBounds>) {
        self.bounds = match (bounds, self.bounds) {
            (Some(mut new), Some(old))
                if (new.min, new.max, new.wrap_width) == (old.min, old.max, old.wrap_width)
                    && new.explored.is_none() =>
            {
                new.explored = old.explored;
                Some(new)
            }
            (b, _) => b,
        };
        self.clamp_focus();
    }

    /// The box of the revealed plots (their centres `(X, Z)`) bounds the focus from now on; the
    /// fog of every change of the plots gives it again (revealed plots stay revealed, so it only
    /// grows). world is [MapBounds::from_grid] of the map.
    pub fn set_explored(&mut self, world: MapBounds, revealed: impl IntoIterator<Item = Vec2>) {
        self.bounds = Some(world.with_explored(revealed));
        self.clamp_focus();
    }

    /// The level the camera is heading for (`level` while it is not following).
    pub fn target_level(&self) -> f32 {
        self.target_level
    }

    /// The focus the camera is heading for.
    pub fn target_focus(&self) -> Vec2 {
        self.target_focus
    }

    /// The camera is still on its way to a target (farther than `FOLLOW_MIN_DISTANCE`).
    pub fn is_following(&self) -> bool {
        self.follow_gap()
            .is_some_and(|(d, ..)| d > p::FOLLOW_MIN_DISTANCE)
    }

    /// Level now, no easing (also the target).
    pub fn set_level(&mut self, level: f32) {
        if level.is_finite() {
            self.level = level.clamp(p::LEVEL_MIN, p::LEVEL_MAX);
            self.target_level = self.level;
        }
    }

    /// Put `world` (X, Z) in the middle of the screen at once, no easing (start of a game, the
    /// minimap; the exe's `MoveCameraSpaceInstantly`).
    pub fn center_on(&mut self, world: Vec2) {
        self.focus = world;
        self.velocity = Vec2::ZERO;
        self.clamp_focus();
        self.sync_target();
    }

    /// Go to `world` (X, Z) at the current height: the target is set at once and `update` eases
    /// the camera to it (every `lookAt` of the exe).
    // re: CvCamera::OnCameraCenter -> MoveAbsoluteWorldSpaceXY (night answers 1b, E4)
    pub fn look_at(&mut self, world: Vec2) {
        self.velocity = Vec2::ZERO;
        self.target_focus = match &self.bounds {
            Some(b) => b.apply(world),
            None => world,
        };
    }

    /// The target is where the camera is.
    fn sync_target(&mut self) {
        self.target_focus = self.focus;
        self.target_level = self.level;
    }

    fn clamp_focus(&mut self) {
        if let Some(b) = &self.bounds {
            self.focus = b.apply(self.focus);
            self.target_focus = b.apply(self.target_focus);
        }
    }

    /// Camera position `(x, height, z)` for a focus and a level.
    fn eye_of(focus: Vec2, level: f32) -> Vec3 {
        let h = p::height_for_level(level);
        Vec3::new(focus.x, h, focus.y + h / p::PITCH.tan())
    }

    /// Distance to the target in camera positions, the vector from the target to the camera and
    /// the target position (its X on the nearer side of the wrap seam).
    fn follow_gap(&self) -> Option<(f32, Vec3, Vec3)> {
        let mut tf = self.target_focus;
        if let Some(w) = self.bounds.and_then(|b| b.wrap_width) {
            let dx = tf.x - self.focus.x;
            tf.x = self.focus.x + (dx + 0.5 * w).rem_euclid(w) - 0.5 * w;
        }
        let tgt = Self::eye_of(tf, self.target_level);
        let diff = Self::eye_of(self.focus, self.level) - tgt;
        let d = diff.length();
        (d.is_finite() && d > 0.0).then_some((d, diff, tgt))
    }

    /// One frame of following the target by `dt` seconds.
    // re: CvCamera::Update follow block (CvCamera.cpp:317-361) (night answers 1a/1b)
    fn follow(&mut self, dt: f32) {
        let Some((d, diff, tgt)) = self.follow_gap() else {
            return;
        };
        let nd = p::follow_distance(d, dt);
        if nd == d {
            return;
        }
        let at = tgt + diff / d * nd;
        self.level = p::level_for_height(at.y).clamp(p::LEVEL_MIN, p::LEVEL_MAX);
        self.focus = Vec2::new(at.x, at.z - at.y / p::PITCH.tan());
        self.clamp_focus();
    }

    pub fn eye(&self) -> Vec3 {
        let h = self.height();
        Vec3::new(
            self.focus.x,
            p::FOCUS_HEIGHT + h,
            self.focus.y + h / p::PITCH.tan(),
        )
    }

    fn forward() -> Vec3 {
        let (s, c) = p::PITCH.sin_cos();
        Vec3::new(0.0, -s, -c)
    }

    pub fn view(&self) -> Mat4 {
        let focus = Vec3::new(self.focus.x, p::FOCUS_HEIGHT, self.focus.y);
        glam::camera::rh::view::look_at_mat4(self.eye(), focus, Vec3::Y)
    }

    /// Depth 0..1 (wgpu convention). FOV depends on `aspect` (width / height).
    pub fn projection(&self, aspect: f32) -> Mat4 {
        let aspect = aspect.max(1e-3);
        glam::camera::rh::proj::directx::perspective(
            p::fov_y_for_aspect(aspect),
            aspect,
            p::NEAR,
            p::FAR,
        )
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        self.projection(aspect) * self.view()
    }

    /// Ray through the point `ndc` (X right, Y up, -1..1).
    pub fn unproject(&self, ndc: Vec2, aspect: f32) -> Ray {
        let aspect = aspect.max(1e-3);
        let t = (p::fov_y_for_aspect(aspect) * 0.5).tan();
        let fwd = Self::forward();
        let right = Vec3::X;
        let up = right.cross(fwd);
        let dir = fwd + right * (ndc.x * t * aspect) + up * (ndc.y * t);
        Ray {
            origin: self.eye(),
            dir: dir.normalize(),
        }
    }

    /// Ground point (height `FOCUS_HEIGHT`) under `ndc`.
    pub fn ground_point(&self, ndc: Vec2, aspect: f32) -> Option<Vec3> {
        self.unproject(ndc, aspect).at_height(p::FOCUS_HEIGHT)
    }

    /// Change the level of the target by `delta` (negative zooms in), keeping the ground point
    /// under `ndc` fixed in the target view; the camera follows with `update`. The pick ray of
    /// the exe has the same direction for the camera and for the target (the pitch is fixed).
    // re: CvCamera::OnCameraIn/Out -> ZoomAlongVector -> MoveAlongVector (night answers 1a)
    pub fn zoom_at(&mut self, delta: f32, ndc: Vec2, aspect: f32) {
        let mut t = *self;
        t.focus = self.target_focus;
        t.level = self.target_level;
        let before = t.ground_point(ndc, aspect);
        let level = t.level + delta;
        t.level = if level.is_finite() {
            level.clamp(p::LEVEL_MIN, p::LEVEL_MAX)
        } else {
            p::LEVEL_MIN
        };
        if let (Some(a), Some(b)) = (before, t.ground_point(ndc, aspect)) {
            t.focus += Vec2::new(a.x - b.x, a.z - b.z);
        }
        self.target_level = t.level;
        self.target_focus = match &self.bounds {
            Some(b) => b.apply(t.focus),
            None => t.focus,
        };
    }

    /// Like `zoom_at` but at once (a pinch follows the fingers).
    // SIMPLIFIED(SIMP-uic-30): touch pinch zoom is direct, the exe has no touch input
    pub fn zoom_at_now(&mut self, delta: f32, ndc: Vec2, aspect: f32) {
        let before = self.ground_point(ndc, aspect);
        self.set_level(self.level + delta);
        if let (Some(a), Some(b)) = (before, self.ground_point(ndc, aspect)) {
            self.focus += Vec2::new(a.x - b.x, a.z - b.z);
        }
        self.clamp_focus();
        self.sync_target();
    }

    /// Drag: the ground point that was under `from` ends up under `to`. Returns the focus shift.
    pub fn drag(&mut self, from: Vec2, to: Vec2, aspect: f32) -> Vec2 {
        let (Some(a), Some(b)) = (
            self.ground_point(from, aspect),
            self.ground_point(to, aspect),
        ) else {
            return Vec2::ZERO;
        };
        let shift = Vec2::new(a.x - b.x, a.z - b.z);
        self.focus += shift;
        self.clamp_focus();
        // A drag sets the camera, not a target: it ends any easing.
        // SIMPLIFIED(SIMP-uic-31): what the exe does with its target under a drag is not traced
        self.sync_target();
        shift
    }

    /// Keyboard or edge scroll. `dir`: X east, Y north, each -1..1. Speed is `h` units per second.
    pub fn scroll(&mut self, dir: Vec2, dt: f32) {
        if dir == Vec2::ZERO {
            return;
        }
        self.velocity = Vec2::ZERO;
        let step = self.height() * p::SCROLL_SPEED_PER_HEIGHT * dt;
        let to = self.focus + Vec2::new(dir.x, -dir.y) * step;
        // A step that would leave the bounds is not taken (not clamped): the camera stops at
        // the edge.
        // re: CvCamera::MoveCameraSpaceXY 0x088b06e4, MoveAlongVector 0x088b0936 (commit only if
        // PositionIsWithinBounds)
        if self.bounds.is_none_or(|b| b.contains(to)) {
            self.focus = to;
            self.clamp_focus();
        }
        self.sync_target();
    }

    /// Start the glide after a drag; `velocity` is the already scaled release velocity (ground
    /// units per second). Duration comes from the speed and the current camera height.
    // re: CvCamera::Update, release branch (CvCamera.cpp:242-251)
    pub fn fling(&mut self, velocity: Vec2) {
        if !p::INERTIA_ENABLED || velocity == Vec2::ZERO || !velocity.is_finite() {
            return;
        }
        self.velocity = velocity;
        self.glide_elapsed = 0.0;
        self.glide_duration = p::inertia_duration(velocity.length(), self.height());
    }

    pub fn stop_inertia(&mut self) {
        self.velocity = Vec2::ZERO;
    }

    pub fn is_gliding(&self) -> bool {
        self.velocity != Vec2::ZERO
    }

    /// Advance the glide by `dt` seconds: the speed factor falls from 1 to 0 over the duration.
    // re: CvCamera::Update, inertia branch (CvCamera.cpp:260-274)
    pub fn update(&mut self, dt: f32) {
        if self.velocity == Vec2::ZERO {
            self.follow(dt);
            return;
        }
        let t = self.glide_elapsed / self.glide_duration;
        if t > 1.0 {
            self.velocity = Vec2::ZERO;
            return;
        }
        self.focus += self.velocity * (p::inertia_factor(t) * dt);
        self.glide_elapsed += dt;
        self.clamp_focus();
        self.sync_target();
    }
}

impl Default for GameCamera {
    fn default() -> Self {
        Self::new(Vec2::ZERO)
    }
}

impl CameraView for GameCamera {
    fn eye(&self) -> Vec3 {
        GameCamera::eye(self)
    }

    fn view(&self) -> Mat4 {
        GameCamera::view(self)
    }

    fn projection(&self, aspect: f32) -> Mat4 {
        GameCamera::projection(self, aspect)
    }

    fn view_proj(&self, aspect: f32) -> Mat4 {
        GameCamera::view_proj(self, aspect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASPECT_16_9: f32 = 16.0 / 9.0;

    fn approx(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn height_at_level_ends() {
        assert!(approx(p::height_for_level(11.0), 260.0, 0.5));
        assert!(approx(p::height_for_level(20.5), 1083.0, 1.0));
    }

    #[test]
    fn wheel_step_is_a_135_factor() {
        let r = p::height_for_level(13.0) / p::height_for_level(11.0);
        assert!(approx(r, 1.35, 0.005), "{r}");
    }

    #[test]
    fn fov_at_16_9() {
        let deg = p::fov_y_for_aspect(ASPECT_16_9).to_degrees();
        assert!(approx(deg, 29.4, 0.1), "{deg}");
        // A very wide window hits the lower bound.
        assert!(approx(p::fov_y_for_aspect(4.0).to_degrees(), 26.5, 1e-3));
    }

    #[test]
    fn level_is_clamped() {
        let mut c = GameCamera::default();
        c.set_level(100.0);
        assert_eq!(c.level(), p::LEVEL_MAX);
        c.set_level(-5.0);
        assert_eq!(c.level(), p::LEVEL_MIN);
        c.set_level(f32::NAN);
        assert_eq!(c.level(), p::LEVEL_MIN);
    }

    /// Ground width in hexes across the middle row of a 1920x1080 screen.
    fn hexes_across(level: f32) -> f32 {
        let mut c = GameCamera::default();
        c.set_level(level);
        let a = 1920.0 / 1080.0;
        let l = c
            .ground_point(pixels_to_ndc(0.0, 540.0, 1920.0, 1080.0), a)
            .unwrap();
        let r = c
            .ground_point(pixels_to_ndc(1920.0, 540.0, 1920.0, 1080.0), a)
            .unwrap();
        (r.x - l.x) / HEX_WIDTH
    }

    #[test]
    fn visible_hexes_at_extreme_levels() {
        let near = hexes_across(p::LEVEL_MIN);
        let far = hexes_across(p::LEVEL_MAX);
        assert!(approx(near, 5.4, 0.5), "{near}");
        assert!(approx(far, 22.0, 2.0), "{far}");
    }

    #[test]
    fn pitch_is_45_degrees_and_center_hits_focus() {
        let mut c = GameCamera::new(Vec2::new(300.0, -200.0));
        c.set_level(15.0);
        let eye = c.eye();
        let f = Vec3::new(300.0, p::FOCUS_HEIGHT, -200.0);
        let d = f - eye;
        assert!(approx(d.y.abs(), (d.x * d.x + d.z * d.z).sqrt(), 1e-2));
        let g = c.ground_point(Vec2::ZERO, ASPECT_16_9).unwrap();
        assert!(g.distance(f) < 1e-2, "{g}");
        let p = c.view_proj(ASPECT_16_9).project_point3(f);
        assert!(approx(p.x, 0.0, 1e-4) && approx(p.y, 0.0, 1e-4));
        assert!((0.0..1.0).contains(&p.z));
    }

    #[test]
    fn unproject_inverts_projection() {
        let c = GameCamera::new(Vec2::new(100.0, -80.0));
        for ndc in [
            Vec2::new(-0.8, 0.7),
            Vec2::new(0.5, -0.9),
            Vec2::new(0.1, 0.2),
        ] {
            let g = c.ground_point(ndc, ASPECT_16_9).unwrap();
            let q = c.view_proj(ASPECT_16_9).project_point3(g);
            assert!(
                approx(q.x, ndc.x, 1e-3) && approx(q.y, ndc.y, 1e-3),
                "{ndc} {q}"
            );
        }
    }

    #[test]
    fn zoom_to_cursor_keeps_ground_point() {
        let mut c = GameCamera::new(Vec2::new(1000.0, -500.0));
        let ndc = Vec2::new(0.6, -0.4);
        for delta in [-2.0, -2.0, 2.0, 2.0, 2.0, -2.0, 2.0, 2.0, 2.0] {
            let before = c.ground_point(ndc, ASPECT_16_9).unwrap();
            c.zoom_at_now(delta, ndc, ASPECT_16_9);
            let after = c.ground_point(ndc, ASPECT_16_9).unwrap();
            let d = Vec2::new(after.x - before.x, after.z - before.z).length();
            assert!(d < 0.5, "moved {d} at level {}", c.level());
        }
    }

    const DT: f32 = 1.0 / 60.0;

    /// Frames until the camera stops following (at most `max`).
    fn run_follow(c: &mut GameCamera, max: usize) -> usize {
        let mut n = 0;
        while c.is_following() && n < max {
            c.update(DT);
            n += 1;
        }
        n
    }

    #[test]
    fn follow_distance_is_the_quartic_root_rule() {
        // D = 100, dt = 0.016: 1.025 * (3.1428 / 1.025 - 0.016)^4 = 88.7 (night answers 1a, 6).
        assert!(approx(p::follow_distance(100.0, 0.016), 88.7, 0.1));
        // Without dt the ratio is 1 / k^4 whatever the distance.
        for d in [5.0, 100.0, 1000.0] {
            assert!(
                approx(p::follow_distance(d, 0.0) / d, 0.905951, 1e-4),
                "{d}"
            );
        }
        // A big dt reaches the target at once.
        assert_eq!(p::follow_distance(50.0, 10.0), 0.0);
        // Within 1.0: no move, no snap.
        assert_eq!(p::follow_distance(1.0, DT), 1.0);
        assert_eq!(p::follow_distance(0.4, DT), 0.4);
        // The distance halves in about 7 frames at a small dt.
        let mut d = 400.0;
        for _ in 0..7 {
            d = p::follow_distance(d, 0.0);
        }
        assert!(approx(d / 400.0, 0.5, 0.02), "{d}");
    }

    #[test]
    fn look_at_eases_and_never_jumps() {
        let mut c = GameCamera::new(Vec2::new(1000.0, -500.0));
        c.set_level(15.0);
        let start = c.focus();
        let goal = Vec2::new(1000.0 + 3000.0, -500.0 - 1200.0);
        c.look_at(goal);
        assert_eq!(c.focus(), start, "the camera has not moved yet");
        assert_eq!(c.target_focus(), goal);
        assert!(c.is_following());
        let mut last = c.focus().distance(goal);
        let mut frames = 0;
        while c.is_following() {
            c.update(DT);
            let d = c.focus().distance(goal);
            assert!(d < last, "closer every frame");
            // Each frame covers at most 1 - 1/k^4 (+ the dt term) of what is left, never a jump.
            // (the dt term bites only in the last few units)
            assert!(last < 10.0 || d > last * 0.85, "{last} -> {d}");
            last = d;
            frames += 1;
            assert!(frames < 400);
        }
        assert!(
            c.focus().distance(goal) < 1.0,
            "stops within one unit, no snap"
        );
        assert!(frames > 30, "a long way takes many frames: {frames}");
        assert!(approx(c.level(), 15.0, 1e-3), "a look-at keeps the height");
    }

    #[test]
    fn wheel_zoom_eases_to_the_target_level() {
        let mut c = GameCamera::new(Vec2::new(1000.0, -500.0));
        c.set_level(15.0);
        let ndc = Vec2::new(0.6, -0.4);
        let pin = c.ground_point(ndc, ASPECT_16_9).unwrap();
        c.zoom_at(-2.0, ndc, ASPECT_16_9);
        c.zoom_at(-2.0, ndc, ASPECT_16_9);
        assert_eq!(c.level(), 15.0, "not at once");
        assert!(approx(c.target_level(), 11.0, 1e-4), "{}", c.target_level());
        let mut prev = c.level();
        let n = run_follow(&mut c, 400);
        assert!((5..200).contains(&n), "{n} frames");
        assert!(c.level() < prev);
        prev = c.level();
        assert!(approx(prev, 11.0, 0.05), "{prev}");
        // The ground point under the cursor ends where it was.
        let now = c.ground_point(ndc, ASPECT_16_9).unwrap();
        assert!(Vec2::new(now.x - pin.x, now.z - pin.z).length() < 2.0);
        // The clamp holds for the target too.
        c.zoom_at(-2.0, ndc, ASPECT_16_9);
        assert_eq!(c.target_level(), p::LEVEL_MIN);
        c.zoom_at(f32::NAN, ndc, ASPECT_16_9);
        assert_eq!(c.target_level(), p::LEVEL_MIN);
    }

    #[test]
    fn zoom_steps_add_up_before_the_camera_arrives() {
        let mut c = GameCamera::default();
        c.set_level(12.0);
        c.zoom_at(2.0, Vec2::ZERO, ASPECT_16_9);
        for _ in 0..3 {
            c.update(DT);
        }
        // A second notch while the first is under way continues from the target.
        c.zoom_at(2.0, Vec2::ZERO, ASPECT_16_9);
        assert!(approx(c.target_level(), 16.0, 1e-4));
        run_follow(&mut c, 400);
        assert!(approx(c.level(), 16.0, 0.05), "{}", c.level());
    }

    #[test]
    fn look_at_crosses_the_wrap_seam_the_short_way() {
        let mut c = bounds_camera(true);
        let w = 64.0 * 40.0;
        c.center_on(Vec2::new(w - 20.0, -100.0));
        c.look_at(Vec2::new(30.0, -100.0));
        c.update(DT);
        let x = c.focus().x;
        assert!(
            x > w - 20.0 || x < 30.0,
            "went over the seam, not across the map: {x}"
        );
        let n = run_follow(&mut c, 400);
        assert!(n < 200);
        let off = (c.focus().x - 30.0).rem_euclid(w);
        assert!(off < 1.0 || w - off < 1.0, "{}", c.focus().x);
    }

    #[test]
    fn a_drag_or_a_scroll_ends_the_easing() {
        let mut c = GameCamera::new(Vec2::new(1000.0, -500.0));
        c.look_at(Vec2::new(2000.0, -500.0));
        c.update(DT);
        c.drag(Vec2::ZERO, Vec2::new(0.1, 0.0), ASPECT_16_9);
        assert!(!c.is_following());
        c.look_at(Vec2::new(2000.0, -500.0));
        c.scroll(Vec2::new(1.0, 0.0), DT);
        assert!(!c.is_following());
        // center_on is the teleport of the exe's startup.
        c.look_at(Vec2::new(0.0, 0.0));
        c.center_on(Vec2::new(5.0, -5.0));
        assert!(!c.is_following());
        assert_eq!(c.focus(), Vec2::new(5.0, -5.0));
    }

    #[test]
    fn drag_carries_the_ground_point() {
        let mut c = GameCamera::new(Vec2::new(1000.0, -500.0));
        let (from, to) = (Vec2::new(0.2, 0.1), Vec2::new(-0.3, -0.25));
        let grabbed = c.ground_point(from, ASPECT_16_9).unwrap();
        c.drag(from, to, ASPECT_16_9);
        let now = c.ground_point(to, ASPECT_16_9).unwrap();
        assert!(Vec2::new(now.x - grabbed.x, now.z - grabbed.z).length() < 0.05);
    }

    #[test]
    fn scroll_speed_is_height_per_second() {
        let mut c = GameCamera::default();
        let h = c.height();
        c.scroll(Vec2::new(1.0, 0.0), 1.0);
        assert!(approx(c.focus().x, h, 1e-2));
        c.scroll(Vec2::new(0.0, 1.0), 0.5);
        assert!(approx(c.focus().y, -h * 0.5, 1e-2), "north is -Z");
    }

    fn bounds_camera(wrap: bool) -> GameCamera {
        let shape = GridShape {
            width: 40,
            height: 24,
            wrap_x: wrap,
        };
        let mut c = GameCamera::new(Vec2::ZERO);
        c.set_bounds(Some(MapBounds::from_grid(&shape)));
        c
    }

    #[test]
    fn a_scroll_that_would_leave_the_world_is_not_taken() {
        let mut c = bounds_camera(false);
        let b = c.bounds().unwrap();
        // The padded world: two hexes past the cells across, two rows at each end.
        assert_eq!(b.min, Vec2::new(-128.0, -ROW_STEP * 26.0));
        assert_eq!(b.max, Vec2::new(64.0 * 42.0, ROW_STEP * 2.0));
        c.scroll(Vec2::new(-1.0, 0.0), 10.0);
        assert_eq!(c.focus(), Vec2::ZERO, "stopped, not slid to the edge");
        // Small steps get to the edge and stop there.
        for _ in 0..100 {
            c.scroll(Vec2::new(-1.0, 0.0), 0.01);
        }
        assert!(c.focus().x >= b.min.x && c.focus().x < b.min.x + 10.0);
        // A drag is clamped.
        c.center_on(Vec2::new(1.0e6, 1.0e6));
        assert_eq!(c.focus(), b.max * Vec2::new(1.0, 1.0));
    }

    #[test]
    fn the_revealed_plots_grown_by_a_hex_bound_the_focus() {
        let mut c = bounds_camera(false);
        let world = c.bounds().unwrap();
        let cells = [Vec2::new(640.0, -554.0), Vec2::new(704.0, -609.0)];
        c.set_explored(world, cells);
        let (lo, hi) = c.bounds().unwrap().explored.unwrap();
        assert_eq!(lo, Vec2::new(640.0 - 64.0, -609.0 - 55.424));
        assert_eq!(hi, Vec2::new(704.0 + 64.0, -554.0 + 55.424));
        // The camera is pulled in, and a scroll out of the box is not taken.
        assert_eq!(c.focus(), Vec2::new(lo.x, hi.y));
        c.scroll(Vec2::new(-1.0, 0.0), 0.1);
        assert_eq!(c.focus(), Vec2::new(lo.x, hi.y));
        // The terrain giving the same world again keeps the box; nothing revealed: the world.
        c.set_bounds(Some(MapBounds::from_grid(&GridShape {
            width: 40,
            height: 24,
            wrap_x: false,
        })));
        assert!(c.bounds().unwrap().explored.is_some());
        c.set_explored(world, []);
        assert!(c.bounds().unwrap().explored.is_none());
        // A box at the edge stays inside the world.
        c.set_explored(world, [Vec2::ZERO]);
        assert_eq!(c.bounds().unwrap().explored.unwrap().1.y, 55.424);
        assert_eq!(c.bounds().unwrap().explored.unwrap().0.x, -64.0);
    }

    #[test]
    fn wrap_carries_x_over_the_seam_without_a_jump() {
        let mut c = bounds_camera(true);
        let w = 64.0 * 40.0;
        c.center_on(Vec2::new(w - 10.0, -100.0));
        let before = c.eye();
        c.scroll(Vec2::new(1.0, 0.0), 0.05);
        let moved = c.focus().x;
        assert!(moved < 100.0, "wrapped to the start: {moved}");
        // Unwrapped distance travelled is small, modulo the period.
        let travelled = (moved - (w - 10.0)).rem_euclid(w);
        assert!(travelled < 100.0, "{travelled}");
        assert!(c.eye().x >= 0.0 && before.x > 0.0);
        c.scroll(Vec2::new(-1.0, 0.0), 0.2);
        assert!(c.focus().x > w - 100.0, "wrapped back: {}", c.focus().x);
        // A scroll past the end of the world along Z is not taken, even when wrapping.
        let y = c.focus().y;
        c.scroll(Vec2::new(0.0, 1.0), 1000.0);
        assert_eq!(c.focus().y, y);
        // A revealed box across the seam: the copy one period off is inside it.
        let world = c.bounds().unwrap();
        c.set_explored(world, [Vec2::new(10.0, -100.0)]);
        assert!(c.bounds().unwrap().contains(Vec2::new(w - 20.0, -100.0)));
        assert!(!c.bounds().unwrap().contains(Vec2::new(w * 0.5, -100.0)));
    }
    #[test]
    fn inertia_follows_the_original_curve() {
        // Speed 200 at h = 424.26: d = 200 * 0.5 / 424.26 + 0.125 = 0.3607, glide 0.1803 s.
        assert!(approx(p::inertia_duration(200.0, 424.26), 0.1803, 1e-3));
        assert!(approx(p::inertia_duration(0.0, 424.26), 0.0625, 1e-6));
        assert_eq!(
            p::inertia_duration(1.0e6, 424.26),
            1.0,
            "d >= 2 caps at 1 s"
        );
        assert!(approx(p::inertia_factor(0.0), 1.0, 1e-6));
        assert!(approx(p::inertia_factor(0.5), 0.5, 1e-6));
        assert!(approx(p::inertia_factor(1.0), 0.0, 1e-6));

        let mut c = GameCamera::default();
        let h = c.height();
        let v = Vec2::new(2000.0, 0.0);
        c.fling(v);
        assert!(c.is_gliding());
        let dur = p::inertia_duration(v.length(), h);
        let mut last = 0.0;
        let mut frames = 0;
        while c.is_gliding() {
            c.update(1.0 / 240.0);
            assert!(c.focus().x >= last);
            last = c.focus().x;
            frames += 1;
            assert!(frames < 1000);
        }
        // The distance is the integral of v * factor over the duration: v * d / 2.
        assert!(approx(last, v.x * dur * 0.5, v.x * dur * 0.02), "{last}");
        assert!(approx(frames as f32 / 240.0, dur, 0.02), "{frames} frames");
        c.fling(Vec2::ZERO);
        assert!(!c.is_gliding());
    }
}
