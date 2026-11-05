//! The reflection pass: the camera, the clip plane and the targets.
//!
//! The original does not mirror the main camera. `TerrainSystem::SetupShadowFrameGlobals`
//! (`TerrainSystem.cpp:1147-1158`) makes a second orthographic view of the same visible rectangle,
//! looking along `TerrainSystem::m_vReflectDir` from 5000 units away, with the projection of the
//! shadow camera; the result is `g_ReflectionViewProj` (`g_ReflectVP` here), which the water reads
//! to find its reflection in `ReflectionColor`. The pass runs with `ReflectionLevel` 1; level 0
//! has no pass and no target.
//!
//! The D3D9 pass cuts at the water with a clip plane (`SetClipPlane`); WebGL2 has none, so the
//! reflection variants of the shaders `discard` what lies below the water plane
//! (`below_water_clip` of `frame.wgsl`, the plane in `ViewGlobals::water_clip`).
//!
//! The matrices are D3DX's (row vectors), in the basis of the original (x east, y north, z up), as
//! [`ShadowCamera`](crate::shadow::ShadowCamera) keeps them; [`ReflectionCamera::view_proj_ours`]
//! turns the view into ours for the draws of this renderer.

use glam::{Mat4, Vec3};

use crate::frame::targets::{
    DEPTH_FORMAT, REFLECTION_COLOR, REFLECTION_DEPTH, ResourceDesc, TargetError, TargetPool,
    TargetSize, WORLD_FORMAT,
};
use crate::frame::views::ViewGlobals;
use crate::lighting::globals::{REFLECT_CAMERA_DIR, WATER_HEIGHT};
use crate::shadow::camera::{
    D3dMatrix, LIGHT_DISTANCE, ORTHO_FAR, ORTHO_NEAR, UP, look_at_rh, ortho_off_center_rh,
};

/// The plane the reflection is cut at: the level of the water in the original's basis (z up).
// re: `water_level()` = 10.12; the plane of `RenderReflections`' clip
// TODO(RE-RW-2): `GameViewState::RenderReflections` (0x0883d62c) has no body in civ5-re; the plane
// is the water level until the disassembly says otherwise.
pub const CLIP_HEIGHT: f32 = WATER_HEIGHT;

/// `ReflectionLevel` at which the pass runs.
// re: GraphicsSettings: `ReflectionLevel` 0 off, 1 on (settings.rs: range 0..1)
pub const LEVEL_ON: i64 = 1;

/// Whether the pass runs at `level`.
pub fn is_on(level: i64) -> bool {
    level >= LEVEL_ON
}

/// Size of `ReflectionColor` and its depth: the screen.
// TODO(S1): the original's size of `ReflectionColor` and its format come with the capture S3 and
// `re-world` (`RenderReflections`); until then the screen's size and the format of the world.
pub const TARGET_SIZE: TargetSize = TargetSize::Screen { num: 1, den: 1 };

/// Declares `ReflectionColor` and its depth for `level`, or drops them at level 0. Redeclared when
/// `ReflectionLevel` changes: the pool recreates only what changed.
pub fn declare_targets(pool: &mut TargetPool, level: i64) -> Result<(), TargetError> {
    if !is_on(level) {
        pool.remove(REFLECTION_COLOR);
        pool.remove(REFLECTION_DEPTH);
        return Ok(());
    }
    pool.declare(
        REFLECTION_COLOR,
        ResourceDesc::color(WORLD_FORMAT, TARGET_SIZE),
    )?;
    pool.declare(
        REFLECTION_DEPTH,
        ResourceDesc::color(DEPTH_FORMAT, TARGET_SIZE),
    )
}

/// The reflection camera of a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReflectionCamera {
    /// `g_ReflectionView`.
    pub view: D3dMatrix,
    /// `g_ReflectionProj`: the projection of the shadow camera.
    pub projection: D3dMatrix,
    /// `g_ReflectionViewProj`: `view * projection`; `g_ReflectVP`.
    pub view_proj: D3dMatrix,
    /// The eye, in the original's basis.
    pub eye: Vec3,
}

impl ReflectionCamera {
    /// `SetupShadowFrameGlobals` for the visible rectangle `top_left`..`bottom_right` (the minimum
    /// and the maximum corner, as for [`ShadowCamera::new`](crate::shadow::ShadowCamera::new)) and
    /// the direction the camera looks along (`m_vReflectDir`).
    pub fn new(top_left: Vec3, bottom_right: Vec3, reflect_dir: Vec3) -> Self {
        let center = (top_left + bottom_right) * 0.5;
        let width = top_left.x - bottom_right.x;
        let height = top_left.y - bottom_right.y;
        // The eye lies against the direction it looks along: the shadow camera adds its direction
        // (that of the sun), this one subtracts.
        let eye = center - reflect_dir * LIGHT_DISTANCE;
        let view = look_at_rh(eye, center, Vec3::from_array(UP));
        let projection = ortho_off_center_rh(
            width * 0.5,
            width * -0.5,
            height * 0.5,
            height * -0.5,
            ORTHO_NEAR,
            ORTHO_FAR,
        );
        Self {
            view,
            projection,
            view_proj: view.mul(&projection),
            eye,
        }
    }

    /// With the direction of the original, `m_vReflectDir`.
    pub fn standard(top_left: Vec3, bottom_right: Vec3) -> Self {
        Self::new(top_left, bottom_right, Vec3::from_array(REFLECT_CAMERA_DIR))
    }

    /// `g_ReflectVP` for glam's column vectors, in the original's basis: `clip = M * p_orig`.
    pub fn view_proj(&self) -> Mat4 {
        self.view_proj.to_glam()
    }

    /// The view for points in our basis: `VP_orig * ours_to_orig`.
    pub fn view_proj_ours(&self, ours_to_orig: &Mat4) -> Mat4 {
        self.view_proj() * *ours_to_orig
    }

    /// Group 0 of the reflection pass, in our basis, with the water clip on.
    pub fn view_globals(&self, ours_to_orig: &Mat4) -> ViewGlobals {
        let eye = ours_to_orig.inverse().transform_point3(self.eye);
        ViewGlobals::new(self.view_proj_ours(ours_to_orig), eye).with_water_clip(CLIP_HEIGHT)
    }
}

/// What the reflection variants of the shaders discard: a point of the original's basis below the
/// plane. Mirrors `below_water_clip` of `frame.wgsl`.
pub fn below_clip(point_orig: Vec3, plane: f32) -> bool {
    point_orig.z < plane
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::basis_rotation;
    use crate::shadow::ShadowCamera;

    // A visible rectangle in the original's basis.
    const TL: Vec3 = Vec3::new(-300.0, -200.0, 0.0);
    const BR: Vec3 = Vec3::new(500.0, 400.0, 0.0);
    const CENTER: Vec3 = Vec3::new(100.0, 100.0, 0.0);

    fn dir() -> Vec3 {
        Vec3::from_array(REFLECT_CAMERA_DIR)
    }

    #[test]
    fn the_camera_is_the_formula_of_setup_shadow_frame_globals() {
        let c = ReflectionCamera::standard(TL, BR);
        // vEye = vCenter - m_vReflectDir * 5000 (TerrainSystem.cpp:1152-1154).
        let eye = CENTER - dir() * 5000.0;
        assert!(c.eye.abs_diff_eq(eye, 1e-3), "{:?} {eye:?}", c.eye);
        // LookAtRH: the eye goes to the origin and the centre to -z at the distance of 5000.
        let at_eye = c.view.transform(eye);
        assert!(at_eye[..3].iter().all(|v| v.abs() < 1e-2), "{at_eye:?}");
        let at_center = c.view.transform(CENTER);
        assert!(at_center[0].abs() < 1e-3 && at_center[1].abs() < 1e-3);
        assert!((at_center[2] + 5000.0).abs() < 1e-2, "{at_center:?}");
        // The camera looks along the direction: a step that way is a step into -z.
        let ahead = c.view.transform(CENTER + dir());
        assert!((ahead[2] + 5001.0).abs() < 1e-2, "{ahead:?}");
        // And the view is glam's look-at for the same eye, centre and up (0, 1, 0).
        let glam = glam::camera::rh::view::look_at_mat4(eye, CENTER, Vec3::Y);
        assert!(c.view.to_glam().abs_diff_eq(glam, 5e-3));
        // view * projection, view first (D3DXMatrixMultiply(&g_ReflectionViewProj, &mView, &mProj)).
        assert_eq!(c.view_proj, c.view.mul(&c.projection));
        // The middle of the rectangle is the middle of the screen.
        let ndc = c.view_proj.transform(CENTER);
        assert!(ndc[0].abs() < 1e-4 && ndc[1].abs() < 1e-4, "{ndc:?}");
        assert!((ndc[2] - (0.5 - 5000.0 / 60000.0)).abs() < 1e-5, "{ndc:?}");
    }

    #[test]
    fn the_projection_is_the_one_of_the_shadow_camera() {
        let r = ReflectionCamera::standard(TL, BR);
        let s = ShadowCamera::new(
            TL,
            BR,
            Vec3::from_array(crate::lighting::globals::SHADOW_DIR),
        );
        assert_eq!(r.projection, s.projection, "g_ReflectionProj = mProjection");
        // But the views differ: they look from opposite sides of the sky along other directions.
        assert_ne!(r.view, s.view);
    }

    #[test]
    fn the_direction_is_what_the_original_starts_with() {
        let d = dir();
        assert_eq!(
            [d.x, d.y, d.z],
            [-0.12278783, -0.6963642, -0.70710677],
            "m_vReflectDir (TerrainSystem::Startup)"
        );
        assert!((d.length() - 1.0).abs() < 1e-5);
        // It points down: the eye is above the map, at 5000 * 0.707 = 3535 over the centre.
        let c = ReflectionCamera::standard(TL, BR);
        assert!((c.eye.z - 3535.53).abs() < 0.1, "{}", c.eye.z);
    }

    #[test]
    fn glam_gets_the_same_transform_and_ours_follows_the_basis() {
        let c = ReflectionCamera::standard(TL, BR);
        let m = c.view_proj();
        let p = Vec3::new(10.0, 20.0, 5.0);
        let d3d = c.view_proj.transform(p);
        let g = m * p.extend(1.0);
        for k in 0..4 {
            assert!((d3d[k] - g[k]).abs() < 1e-4, "{k}: {d3d:?} {g}");
        }
        // A point of ours (X east, Y up, Z south) lands where its point of the original does.
        let b = basis_rotation();
        let ours = Vec3::new(10.0, 5.0, -20.0);
        let want = m * b.transform_point3(ours).extend(1.0);
        let got = c.view_proj_ours(&b) * ours.extend(1.0);
        assert!((want - got).abs().max_element() < 1e-4, "{want} {got}");
        // The eye of the view is the eye in our basis: x east, y up, z south.
        let v = c.view_globals(&b);
        let eye = Vec3::new(c.eye.x, c.eye.z, -c.eye.y);
        assert!(Vec3::new(v.eye[0], v.eye[1], v.eye[2]).abs_diff_eq(eye, 1e-2));
    }

    #[test]
    fn the_view_of_the_pass_has_the_plane_on_and_the_main_view_has_it_off() {
        let b = basis_rotation();
        let pass = ReflectionCamera::standard(TL, BR).view_globals(&b);
        assert_eq!(pass.water_clip, [10.12, 1.0, 0.0, 0.0]);
        let main = ViewGlobals::new(Mat4::IDENTITY, Vec3::ZERO);
        assert_eq!(main.water_clip[1], 0.0, "the main pass does not clip");
        // Copies of a wrapping map keep the plane.
        assert_eq!(pass.for_copy(640.0).water_clip, pass.water_clip);
        assert!(below_clip(Vec3::new(0.0, 0.0, 10.0), CLIP_HEIGHT));
        assert!(!below_clip(Vec3::new(0.0, 0.0, 10.12), CLIP_HEIGHT));
        assert!(!below_clip(Vec3::new(0.0, 0.0, 30.0), CLIP_HEIGHT));
    }

    #[test]
    fn the_targets_exist_with_the_level() {
        let mut pool = TargetPool::new(false, (1280, 720));
        declare_targets(&mut pool, 1).unwrap();
        assert_eq!(pool.size_of(REFLECTION_COLOR), Some((1280, 720)));
        assert_eq!(pool.size_of(REFLECTION_DEPTH), Some((1280, 720)));
        assert_eq!(
            pool.desc_of(REFLECTION_COLOR).unwrap().format,
            WORLD_FORMAT,
            "gamma space, not sRGB"
        );
        // The screen changes: the targets follow it.
        pool.set_screen((800, 600));
        assert_eq!(pool.size_of(REFLECTION_COLOR), Some((800, 600)));
        // Level 0: no pass, no targets.
        declare_targets(&mut pool, 0).unwrap();
        assert_eq!(pool.size_of(REFLECTION_COLOR), None);
        assert_eq!(pool.size_of(REFLECTION_DEPTH), None);
        assert!(!is_on(0) && is_on(1));
    }
}
