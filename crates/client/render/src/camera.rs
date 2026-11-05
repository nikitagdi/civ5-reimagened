//! Orbit camera (showcase only; the game uses `GameCamera`). World: right-handed, Y up, map in XZ.

use glam::{Mat4, Vec3};

/// What the renderer needs from a camera: the eye and the clip-space transform.
pub trait CameraView {
    fn eye(&self) -> Vec3;
    /// World to view space, in our basis.
    fn view(&self) -> Mat4;
    /// View to clip space for a screen of `aspect`; depth 0..1 (wgpu convention).
    fn projection(&self, aspect: f32) -> Mat4;
    /// `projection * view`; an implementation may compute it faster.
    fn view_proj(&self, aspect: f32) -> Mat4 {
        self.projection(aspect) * self.view()
    }
}

/// The camera of a capture for `golden`: the matrices of the original's frame brought into our
/// basis through `ours_to_orig` (render-core 4.6, 4.14), and the eye. The aspect is the
/// capture's own, so `projection` ignores the one asked for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FixedCamera {
    pub view: Mat4,
    pub proj: Mat4,
    pub eye: Vec3,
}

impl FixedCamera {
    /// The camera whose `projection * view` is `view_proj`, a perspective matrix of our basis (right-handed,
    /// Y up, depth 0..1), read back into the eye, a view and a projection (`golden`: the capture's matrix
    /// is all that is known of its camera). `None` when `view_proj` is not a perspective of that kind: the
    /// rebuilt matrix is checked against it (0.01 % of its largest element).
    ///
    /// Row 3 of the matrix is the view direction (clip `w` is the depth along it), rows 0 and 1 are the right
    /// and up axes scaled by the projection, row 2 is depth: `z = a + b / w`.
    pub fn from_view_proj(view_proj: Mat4) -> Option<Self> {
        use glam::{DMat3, DMat4, DVec3, DVec4};
        let m = view_proj.as_dmat4();
        let (r0, r1, r2, r3) = (m.row(0), m.row(1), m.row(2), m.row(3));
        // The eye is where clip x, y and w are all zero.
        let rows = DMat3::from_cols(r0.truncate(), r1.truncate(), r3.truncate()).transpose();
        if rows.determinant().abs() < 1e-12 {
            return None;
        }
        let eye = rows.inverse() * -DVec3::new(r0.w, r1.w, r3.w);
        let len = r3.truncate().length();
        let (right_len, up_len) = (r0.truncate().length(), r1.truncate().length());
        if len < 1e-12 || right_len < 1e-12 || up_len < 1e-12 {
            return None;
        }
        let fwd = r3.truncate() / len;
        let up = r1.truncate() / up_len;
        let aspect = up_len / right_len;
        let fov_y = 2.0 * (len / up_len).atan();
        let a = r2.truncate().dot(fwd) / len;
        let b = r2.w / len + a * fwd.dot(eye);
        let (near, far) = (-b / a, b / (1.0 - a));
        if !(near > 0.0 && far > near) {
            return None;
        }
        // The view looks down -Z of its own space; the projection is Direct3D's (depth 0..1).
        let right = fwd.cross(up).normalize();
        let up = right.cross(fwd);
        let view = DMat4::from_cols(
            DVec4::new(right.x, up.x, -fwd.x, 0.0),
            DVec4::new(right.y, up.y, -fwd.y, 0.0),
            DVec4::new(right.z, up.z, -fwd.z, 0.0),
            DVec4::new(-right.dot(eye), -up.dot(eye), fwd.dot(eye), 1.0),
        );
        let f = 1.0 / (fov_y * 0.5).tan();
        let proj = DMat4::from_cols(
            DVec4::new(f / aspect, 0.0, 0.0, 0.0),
            DVec4::new(0.0, f, 0.0, 0.0),
            DVec4::new(0.0, 0.0, far / (near - far), -1.0),
            DVec4::new(0.0, 0.0, near * far / (near - far), 0.0),
        );
        let scale = m.to_cols_array().iter().fold(0.0f64, |x, v| x.max(v.abs()));
        let err = (proj * view)
            .to_cols_array()
            .iter()
            .zip(m.to_cols_array())
            .fold(0.0f64, |x, (p, q)| x.max((p - q).abs()));
        (err <= 1e-4 * scale).then(|| Self {
            view: view.as_mat4(),
            proj: proj.as_mat4(),
            eye: eye.as_vec3(),
        })
    }
}

impl CameraView for FixedCamera {
    fn eye(&self) -> Vec3 {
        self.eye
    }

    fn view(&self) -> Mat4 {
        self.view
    }

    fn projection(&self, _aspect: f32) -> Mat4 {
        self.proj
    }
}

impl CameraView for Camera {
    fn eye(&self) -> Vec3 {
        Camera::eye(self)
    }

    fn view(&self) -> Mat4 {
        Camera::view(self)
    }

    fn projection(&self, aspect: f32) -> Mat4 {
        Camera::projection(self, aspect)
    }

    fn view_proj(&self, aspect: f32) -> Mat4 {
        Camera::view_proj(self, aspect)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub fov_y: f32,
}

impl Camera {
    /// Keeps the camera above the ground.
    pub const MIN_PITCH: f32 = 0.2;
    /// Just under π/2, so `look_at` stays valid.
    pub const MAX_PITCH: f32 = 1.45;
    pub const MIN_DISTANCE: f32 = 3.0;
    pub const MAX_DISTANCE: f32 = 120.0;
    pub const NEAR: f32 = 0.1;
    pub const FAR: f32 = 500.0;

    pub fn new(target: Vec3) -> Self {
        Self {
            target,
            yaw: 0.0,
            pitch: 0.9,
            distance: 28.0,
            fov_y: 45f32.to_radians(),
        }
    }

    pub fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.target + Vec3::new(cp * sy, sp, cp * cy) * self.distance
    }

    pub fn ground_forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        Vec3::new(-sy, 0.0, -cy)
    }

    pub fn ground_right(&self) -> Vec3 {
        self.ground_forward().cross(Vec3::Y)
    }

    pub fn view(&self) -> Mat4 {
        glam::camera::rh::view::look_at_mat4(self.eye(), self.target, Vec3::Y)
    }

    /// Depth 0..1 (wgpu convention).
    pub fn projection(&self, aspect: f32) -> Mat4 {
        glam::camera::rh::proj::directx::perspective(
            self.fov_y,
            aspect.max(1e-3),
            Self::NEAR,
            Self::FAR,
        )
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        self.projection(aspect) * self.view()
    }

    pub fn orbit(&mut self, d_yaw: f32, d_pitch: f32) {
        self.yaw = (self.yaw + d_yaw).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + d_pitch).clamp(Self::MIN_PITCH, Self::MAX_PITCH);
    }

    pub fn zoom(&mut self, factor: f32) {
        if factor.is_finite() && factor > 0.0 {
            self.distance = (self.distance * factor).clamp(Self::MIN_DISTANCE, Self::MAX_DISTANCE);
        }
    }

    pub fn world_per_pixel(&self, viewport_height: f32) -> f32 {
        2.0 * self.distance * (self.fov_y * 0.5).tan() / viewport_height.max(1.0)
    }

    /// Map drag: the point under the cursor follows it. `dy` is down-positive.
    pub fn pan_pixels(&mut self, dx: f32, dy: f32, viewport_height: f32) {
        let k = self.world_per_pixel(viewport_height);
        // Compensate for foreshortening.
        let k_forward = k / self.pitch.sin().max(0.2);
        self.target -= self.ground_right() * (dx * k);
        self.target += self.ground_forward() * (dy * k_forward);
    }
}

impl Default for Camera {
    fn default() -> Self {
        Self::new(Vec3::ZERO)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn eye_is_at_distance_from_target() {
        let mut c = Camera::new(Vec3::new(3.0, 0.0, -2.0));
        c.yaw = 1.0;
        c.pitch = 0.7;
        c.distance = 10.0;
        assert!(approx(c.eye().distance(c.target), 10.0));
        assert!(c.eye().y > c.target.y);
    }

    #[test]
    fn default_yaw_looks_along_negative_z() {
        let c = Camera::default();
        assert!(c.eye().z > 0.0);
        assert!(approx(c.ground_forward().z, -1.0));
        assert!(approx(c.ground_right().x, 1.0));
    }

    #[test]
    fn orbit_clamps_pitch_and_wraps_yaw() {
        let mut c = Camera::default();
        c.orbit(0.0, 10.0);
        assert_eq!(c.pitch, Camera::MAX_PITCH);
        c.orbit(0.0, -10.0);
        assert_eq!(c.pitch, Camera::MIN_PITCH);
        c.orbit(std::f32::consts::TAU + 0.5, 0.0);
        assert!(approx(c.yaw, 0.5));
    }

    #[test]
    fn zoom_clamps_and_ignores_garbage() {
        let mut c = Camera::default();
        c.zoom(1000.0);
        assert_eq!(c.distance, Camera::MAX_DISTANCE);
        c.zoom(0.0001);
        assert_eq!(c.distance, Camera::MIN_DISTANCE);
        c.zoom(f32::NAN);
        c.zoom(-1.0);
        assert_eq!(c.distance, Camera::MIN_DISTANCE);
    }

    #[test]
    fn pan_moves_target_against_drag() {
        let mut c = Camera::default();
        c.pan_pixels(100.0, 0.0, 800.0);
        assert!(c.target.x < 0.0);
        assert!(approx(c.target.y, 0.0));
        let mut c = Camera::default();
        c.pan_pixels(0.0, 100.0, 800.0);
        assert!(c.target.z < 0.0);
    }

    #[test]
    fn view_proj_is_projection_times_view_for_every_camera() {
        let c = Camera::new(Vec3::new(5.0, 0.0, 5.0));
        let aspect = 16.0 / 9.0;
        let cam: &dyn CameraView = &c;
        assert_eq!(cam.view_proj(aspect), cam.projection(aspect) * cam.view());
        // The default of the trait, through a camera that only gives view and projection.
        let fixed = FixedCamera {
            view: c.view(),
            proj: c.projection(aspect),
            eye: c.eye(),
        };
        let cam: &dyn CameraView = &fixed;
        assert_eq!(
            cam.view_proj(1.0),
            c.view_proj(aspect),
            "the aspect is the capture's"
        );
        assert_eq!(cam.eye(), c.eye());
    }

    #[test]
    fn a_fixed_camera_is_read_back_from_a_view_projection() {
        let aspect = 1914.0 / 1051.0;
        let c = crate::GameCamera::new(glam::Vec2::new(40.0, 25.0));
        let vp = c.view_proj(aspect);
        let f = FixedCamera::from_view_proj(vp).expect("a perspective matrix");
        assert!(
            (f.eye - c.eye()).length() < 1e-3,
            "{:?} {:?}",
            f.eye,
            c.eye()
        );
        let again = f.view_proj(1.0);
        let scale = vp
            .to_cols_array()
            .iter()
            .fold(0.0f32, |a, v| a.max(v.abs()));
        for (p, q) in again.to_cols_array().iter().zip(vp.to_cols_array()) {
            assert!((p - q).abs() <= 1e-4 * scale, "{p} {q}");
        }
        // The matrix of a capture is in the basis of the original, shifted: `P_ours * ours_to_orig⁻¹`.
        let to_orig =
            Mat4::from_translation(Vec3::new(-300.0, 200.0, 0.0)) * crate::frame::basis_rotation();
        let orig = vp * to_orig.inverse();
        let f = FixedCamera::from_view_proj(orig).expect("the same camera in another basis");
        assert!((f.eye - to_orig.transform_point3(c.eye())).length() < 1e-2);
        // Not a perspective: it has no view direction.
        assert!(FixedCamera::from_view_proj(Mat4::IDENTITY).is_none());
    }

    #[test]
    fn view_proj_maps_target_to_screen_center() {
        let c = Camera::new(Vec3::new(5.0, 0.0, 5.0));
        let p = c.view_proj(16.0 / 9.0).project_point3(c.target);
        assert!(approx(p.x, 0.0) && approx(p.y, 0.0));
        assert!(p.z > 0.0 && p.z < 1.0);
    }
}
