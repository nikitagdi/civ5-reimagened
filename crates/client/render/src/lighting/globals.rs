//! `FrameGlobals`: the constants of one frame that every ported shader family reads (render-core
//! spec 4.6 group 0, 4.9): the sun, shadow and reflection vectors, the distance fog, the time and
//! the matrices of the main view in the basis of the original.
//!
//! Field names are the CTAB names of the constants where they are known (`f3CameraWorld`,
//! `g_FogConsts`, `f3FogColor`, `ViewProjMatrix`, `ViewInverseMatrix`, `g_ShadowVP`,
//! `g_ReflectVP`); the others carry the name of the field of the original they come from and are
//! renamed when RW-4 (b, d) gives the CTAB names. Every number is a named constant with a `// re:`.
//!
//! **Matrices.** A matrix of the original is a `row_major float4x4` read as `Σ v_k · row_k`, which
//! in WGSL is `v * M` with `M = P^T` for the usual (column vector) matrix `P`. So every matrix
//! of the block except `ours_to_orig` is stored transposed ([`row_major`]); `ours_to_orig` is the
//! plain matrix the vertex entries multiply a column vector with.

use glam::{Mat4, Vec3, Vec4};

use crate::uniform_block;

// ---- the numbers of the original ----

/// `TerrainSystem::m_vShadowDir` (`TerrainSystem::Startup`, `TerrainSystem.cpp:194-196`): the
/// direction of the sun of the terrain, also the direction of the shadow camera. In the basis of
/// the original (x east, y north, z up). `Terrain::m_vSunDir` takes this value.
// re: TerrainSystem::Startup (0x0867e9be): m_vShadowDir = (0.4330127, -0.25000003, 0.8660254)
pub const SHADOW_DIR: [f32; 3] = [0.4330127, -0.25000003, 0.8660254];

/// `TerrainSystem::m_vSunDir`: goes to `Terrain::m_vReflectDir` (`Terrain.cpp:1466-1469`).
// re: TerrainSystem::Startup: m_vSunDir = (1.8473232e-08, -0.42261827, 0.90630776)
pub const REFLECT_DIR_TERRAIN: [f32; 3] = [1.847_323_2e-8, -0.42261827, 0.90630776];

/// `TerrainSystem::m_vReflectDir`: the direction of the reflection camera
/// (`SetupShadowFrameGlobals`, `TerrainSystem.cpp:1147-1158`).
// re: TerrainSystem::Startup: m_vReflectDir = (-0.12278783, -0.6963642, -0.70710677)
pub const REFLECT_CAMERA_DIR: [f32; 3] = [-0.12278783, -0.6963642, -0.70710677];

/// `TerrainSystem::m_fReflectAmount`.
// re: TerrainSystem::Startup: m_fReflectAmount = 0.5
pub const REFLECT_AMOUNT: f32 = 0.5;

/// `g_FogConsts` with the distance fog off: height and distance factors 0 make the fog factor of
/// `UnitPS` exactly 0 (`exp_sat(0) = 1`, `1 - 1 = 0`); `w` is the row of `FogRampCached` the
/// shader reads.
// re: UnitShader::UnitPS (sha1 cb0f37374433): g_FogConsts.x height, .y distance, .w ramp row
pub const FOG_CONSTS_OFF: [f32; 4] = [0.0, 0.0, 0.0, 0.5];

/// `cvDistanceFog::m_fHeightFalloff`: the density falls with `exp(-falloff * height)`.
// re: cvDistanceFog::Startup (0x088bb128): m_fHeightFalloff = 0.05; the captures S1a, S2, S3 read
// `g_FogConsts.x = -0.05` everywhere.
pub const FOG_HEIGHT_FALLOFF: f32 = 0.05;

/// `cvDistanceFog::m_fDensity`.
// measured: 2026-10-27, S1a/S2/S3: `g_FogConsts.y / exp(-0.05 * eye.z)` = -1.0000e-3 in all three
// (eye.z 424.26, 261.04, 899.33), so `density / falloff` = 1e-3. `cvDistanceFog::Startup` is
// decompiled with 1.0e-4; the captures say 5e-5.
pub const FOG_DENSITY: f32 = 5.0e-5;

/// `f3FogColor`: `assets/DistanceFog/FogColor.csv`, the row (146, 182, 243). The alpha is not read.
// re: cvDistanceFog::Startup (channels / 255); measured: 2026-10-27, `f3FogColor` of S1a, S2, S3
// = (0.572549, 0.7137255, 0.9529412).
pub const FOG_COLOR_DEFAULT: [f32; 4] = [0.572549, 0.7137255, 0.9529412, 1.0];

/// `g_FogConsts` of a frame whose eye is at height `eye_z` (the original's basis, z up): the height
/// falloff negated, the density integrated along the height of the eye, and the zoom of the camera
/// as the row of the ramp.
// re: TerrainSystem::SetupFrameGlobals (TerrainSystem.cpp:1630-1633) and Terrain.cpp:863-866:
//   x = -falloff, y = -(density * exp(-falloff * eye.z)) / falloff, w = fCurrZoomNorm
// with `fCurrZoomNorm = (level - min) / (max - min)` and `level = ln(h / 50) / 0.15`
// (CvCamera::RecalculateCurrentZoomLevel; the levels are those of `camera_params`).
pub fn fog_consts(eye_z: f32) -> [f32; 4] {
    use crate::camera_params as cp;
    let level = (eye_z.max(1.0) / cp::ZOOM_BASE_HEIGHT).ln() / cp::ZOOM_RATE;
    let zoom_norm = ((level - cp::LEVEL_MIN) / (cp::LEVEL_MAX - cp::LEVEL_MIN)).clamp(0.0, 1.0);
    [
        -FOG_HEIGHT_FALLOFF,
        -(FOG_DENSITY * (-FOG_HEIGHT_FALLOFF * eye_z).exp()) / FOG_HEIGHT_FALLOFF,
        0.0,
        zoom_norm,
    ]
}

/// `fWaterHeight` of `UnitAlphaPS`: the level of the water, the same number `world-water` calls
/// `water_level()`. Units are lit differently under it.
// re: `water_level()` = 10.12
pub const WATER_HEIGHT: f32 = 10.12;

/// `g_ShadowFade` of a frame with `FadeShadows` on.
// TODO(S1): the fade of `Shadow::BlurAndProcess`; 1 = the shadow at full strength.
pub const SHADOW_FADE_FULL: f32 = 1.0;

uniform_block! {
    /// Group 0, binding 1: one per frame. 416 bytes.
    pub struct FrameGlobals {
        /// Our basis to the original's: rotation `B` and the translation `T` of the map. Plain
        /// (column vector) matrix.
        pub ours_to_orig: [[f32; 4]; 4],
        /// `row_major float4x4`: the main view's matrix in the basis of the original.
        pub ViewProjMatrix: [[f32; 4]; 4],
        /// `row_major float4x4`: the camera's placement; its row 3 is the eye, in the original's
        /// basis.
        pub ViewInverseMatrix: [[f32; 4]; 4],
        /// The sun's orthographic view (R1.16), `row_major`.
        pub g_ShadowVP: [[f32; 4]; 4],
        /// The reflection camera's view (RC-12), `row_major`.
        pub g_ReflectVP: [[f32; 4]; 4],
        /// The eye, in the original's basis.
        pub f3CameraWorld: [f32; 3],
        /// Seconds; `Renderer::set_time_override` and the constant overrides replace it.
        pub g_Time: f32,
        pub g_FogConsts: [f32; 4],
        pub f3FogColor: [f32; 4],
        /// `TerrainSystem::m_vShadowDir`: the sun of the terrain.
        pub g_SunDir: [f32; 3],
        pub g_ShadowFade: f32,
        /// `TerrainSystem::m_vSunDir`: `Terrain::m_vReflectDir`.
        pub g_ReflectDir: [f32; 3],
        pub g_ReflectAmount: f32,
        /// `TerrainSystem::m_vReflectDir`: the direction of the reflection camera.
        pub g_ReflectCameraDir: [f32; 3],
        /// The water level, in the original's basis (`UnitAlphaPS`: a unit below it is dimmed).
        /// This was the reserved word of the block (WOB1).
        pub fWaterHeight: f32,
    }
}

/// `P` as the shader reads a `row_major float4x4`: transposed.
pub fn row_major(p: Mat4) -> [[f32; 4]; 4] {
    p.transpose().to_cols_array_2d()
}

/// What a frame knows when its constants are made.
#[derive(Clone, Copy, Debug)]
pub struct FrameInputs {
    /// The main view's matrix in our basis (`CameraView::view_proj`).
    pub view_proj: Mat4,
    /// The eye in our basis.
    pub eye: Vec3,
    /// `ours_to_orig` of the map (`PrepareContext::set_world_frame`), or only the rotation.
    pub ours_to_orig: Mat4,
    pub time: f32,
    /// `DistanceFog` not switched off.
    pub distance_fog: bool,
    /// `FadeShadows`.
    pub fade_shadows: bool,
    /// The sun's view in the original's basis (plain matrix; [`Mat4::IDENTITY`] without shadows).
    pub shadow_vp: Mat4,
    /// The reflection camera's view in the original's basis.
    pub reflect_vp: Mat4,
}

impl FrameInputs {
    /// The inputs of a frame without shadows or reflections.
    pub fn new(view_proj: Mat4, eye: Vec3, ours_to_orig: Mat4, time: f32) -> Self {
        Self {
            view_proj,
            eye,
            ours_to_orig,
            time,
            distance_fog: true,
            fade_shadows: true,
            shadow_vp: Mat4::IDENTITY,
            reflect_vp: Mat4::IDENTITY,
        }
    }
}

impl FrameGlobals {
    /// The constants of a frame. `VP_orig = VP_ours · orig_to_ours`: a point of the
    /// original's basis lands where the same point lands in ours.
    pub fn new(i: &FrameInputs) -> Self {
        let orig_to_ours = i.ours_to_orig.inverse();
        let view_proj = i.view_proj * orig_to_ours;
        let eye = i.ours_to_orig.transform_point3(i.eye);
        // `DistanceFog` switched off: the density is 0 in the constants.
        let fog = if i.distance_fog {
            fog_consts(eye.z)
        } else {
            FOG_CONSTS_OFF
        };
        Self {
            ours_to_orig: i.ours_to_orig.to_cols_array_2d(),
            ViewProjMatrix: row_major(view_proj),
            ViewInverseMatrix: row_major(Mat4::from_translation(eye)),
            g_ShadowVP: row_major(i.shadow_vp),
            g_ReflectVP: row_major(i.reflect_vp),
            f3CameraWorld: eye.to_array(),
            g_Time: i.time,
            g_FogConsts: fog,
            f3FogColor: FOG_COLOR_DEFAULT,
            g_SunDir: SHADOW_DIR,
            g_ShadowFade: if i.fade_shadows {
                SHADOW_FADE_FULL
            } else {
                0.0
            },
            g_ReflectDir: REFLECT_DIR_TERRAIN,
            g_ReflectAmount: REFLECT_AMOUNT,
            g_ReflectCameraDir: REFLECT_CAMERA_DIR,
            fWaterHeight: WATER_HEIGHT,
        }
    }

    /// The eye as a `Vec4`, for tests.
    pub fn eye(&self) -> Vec4 {
        Vec4::new(
            self.f3CameraWorld[0],
            self.f3CameraWorld[1],
            self.f3CameraWorld[2],
            1.0,
        )
    }
}

impl Default for FrameGlobals {
    /// A frame at the origin, without distance fog (an eye at height 0 would stand in the thick of it).
    fn default() -> Self {
        Self::new(&FrameInputs {
            distance_fog: false,
            ..FrameInputs::new(
                Mat4::IDENTITY,
                Vec3::ZERO,
                crate::frame::views::basis_rotation(),
                0.0,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shaders::UniformBlock;

    fn ours_to_orig() -> Mat4 {
        // The rotation B and a translation T.
        Mat4::from_translation(Vec3::new(100.0, -40.0, 7.0)) * crate::frame::views::basis_rotation()
    }

    /// `v * M` for the shader's `row_major` reading.
    fn row_times(v: Vec4, m: [[f32; 4]; 4]) -> Vec4 {
        let col = |j: usize| Vec4::from_array(m[j]);
        Vec4::new(v.dot(col(0)), v.dot(col(1)), v.dot(col(2)), v.dot(col(3)))
    }

    #[test]
    fn the_block_has_the_names_and_the_size_of_the_spec() {
        assert_eq!(std::mem::size_of::<FrameGlobals>(), 416);
        for name in [
            "ViewProjMatrix",
            "ViewInverseMatrix",
            "g_ShadowVP",
            "g_ReflectVP",
            "f3CameraWorld",
            "g_FogConsts",
            "f3FogColor",
        ] {
            assert!(FrameGlobals::field(name).is_some(), "{name}");
        }
        assert_eq!(FrameGlobals::field("f3CameraWorld").unwrap().offset, 320);
        // A scalar shares the vec4 of the vec3 before it.
        assert_eq!(FrameGlobals::field("g_Time").unwrap().offset, 332);
    }

    #[test]
    fn the_view_projection_of_the_original_projects_like_ours() {
        let m = ours_to_orig();
        let vp = glam::camera::rh::proj::directx::perspective(1.0, 1.5, 1.0, 5000.0)
            * glam::camera::rh::view::look_at_mat4(
                Vec3::new(30.0, 90.0, 20.0),
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::Y,
            );
        let eye = Vec3::new(30.0, 90.0, 20.0);
        let g = FrameGlobals::new(&FrameInputs::new(vp, eye, m, 1.5));
        for p in [Vec3::new(3.0, 2.0, -9.0), Vec3::new(-12.0, 0.5, 4.0)] {
            let want = vp * p.extend(1.0);
            // The same point in the original's basis, read the way the shader reads the matrix.
            let orig = m.transform_point3(p);
            let got = row_times(orig.extend(1.0), g.ViewProjMatrix);
            assert!((want - got).abs().max_element() < 1e-3, "{want} {got}");
        }
        assert_eq!(g.g_Time, 1.5);
    }

    #[test]
    fn the_eye_is_in_the_original_basis_in_both_places() {
        let m = ours_to_orig();
        let eye = Vec3::new(30.0, 90.0, 20.0);
        let g = FrameGlobals::new(&FrameInputs::new(Mat4::IDENTITY, eye, m, 0.0));
        let want = m.transform_point3(eye);
        assert!(
            (Vec3::from_array(g.f3CameraWorld) - want)
                .abs()
                .max_element()
                < 1e-4
        );
        // `ViewInverseMatrix`: column j holds the eye's component j in its row 3.
        for j in 0..3 {
            assert!((g.ViewInverseMatrix[j][3] - want[j]).abs() < 1e-4, "{j}");
        }
    }

    #[test]
    fn the_sun_vectors_are_the_numbers_of_the_original() {
        let g = FrameGlobals::default();
        assert_eq!(g.g_SunDir, [0.4330127, -0.25000003, 0.8660254]);
        assert_eq!(g.g_ReflectCameraDir, [-0.12278783, -0.6963642, -0.70710677]);
        let len = |v: [f32; 3]| Vec3::from_array(v).length();
        for v in [g.g_SunDir, g.g_ReflectDir, g.g_ReflectCameraDir] {
            assert!((len(v) - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn the_distance_fog_constants_are_those_of_the_captures() {
        // (eye height, y, w) read from `g_FogConsts` of S1a, S2 and S3.
        for (z, y, w) in [
            (424.264_07_f32, -6.126_644e-13_f32, 0.342_689_8_f32),
            (261.042_3, -2.145_549e-9, 0.0),
            (899.332_6, -2.959_653e-23, 0.870_436_2),
        ] {
            let c = fog_consts(z);
            assert_eq!(c[0], -0.05);
            assert!(((c[1] - y) / y).abs() < 2e-3, "y at {z}: {} vs {y}", c[1]);
            assert_eq!(c[2], 0.0);
            // The zoom is that of the levels of `camera_params` (11 and 20.5); the captures' own
            // zoom state differs from the height of the eye by up to 2e-3.
            assert!((c[3] - w).abs() < 3e-3, "w at {z}: {} vs {w}", c[3]);
        }
        // Switched off: the density is 0.
        let mut i = FrameInputs::new(Mat4::IDENTITY, Vec3::ZERO, ours_to_orig(), 0.0);
        i.distance_fog = false;
        assert_eq!(&FrameGlobals::new(&i).g_FogConsts[..3], &[0.0; 3]);
    }
}
