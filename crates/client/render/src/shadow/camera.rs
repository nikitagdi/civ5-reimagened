//! The camera of the shadow map: a port of `TerrainSystem::SetupShadowFrameGlobals` and of the
//! sizes of `TerrainSystem::BuildShadowBuffers` (civ5-re `TerrainSystem.cpp:279`, `:1112`).
//!
//! The light looks at the middle of the visible part of the world from 5000 units away along the
//! direction of the sun; the view is orthographic over the visible rectangle. The matrices are
//! D3DX's: row vectors, `v * View * Projection`. [`D3dMatrix`] keeps them as D3DX does, so
//! `g_ShadowVP` of a capture and ours compare element by element; [`ShadowCamera::view_proj`]
//! gives the same matrix for glam's column vectors (the transpose).
//!
//! Everything here is in the basis of the original (x east, y north, z up); the world frame of
//! `render` (`ours_to_orig`) turns it into ours where a pass needs that.

use glam::{Mat4, Vec2, Vec3};

/// The light's distance from the middle of the visible rectangle.
// re: TerrainSystem::SetupShadowFrameGlobals (TerrainSystem.cpp:1124): kLightDistance = 5000
pub const LIGHT_DISTANCE: f32 = 5000.0;

/// Near and far of the orthographic projection: `OrthoOffCenterRH(.., 30000, -30000)`.
// re: TerrainSystem::SetupShadowFrameGlobals (TerrainSystem.cpp:1143): zn = 30000, zf = -30000
pub const ORTHO_NEAR: f32 = 30000.0;
pub const ORTHO_FAR: f32 = -30000.0;

/// The up vector of the light's view.
// re: TerrainSystem::SetupShadowFrameGlobals: vUp = (0, 1, 0)
pub const UP: [f32; 3] = [0.0, 1.0, 0.0];

/// The two heights the visible rectangle is measured at (the frustum's edges meet these planes):
/// the ground and the top of the mountains.
// re: GameViewState::CalcVisibleRegion (0x0883beb8): the planes (0, 0, 1, 0) and (0, 0, 1, -127.5)
pub const VISIBLE_HEIGHTS: [f32; 2] = [0.0, 127.5];

/// A D3DX matrix: `m[row][column]`, applied to row vectors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct D3dMatrix(pub [[f32; 4]; 4]);

impl D3dMatrix {
    pub const IDENTITY: D3dMatrix = D3dMatrix([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]);

    /// `a * b` as `D3DXMatrixMultiply`: `a` first.
    pub fn mul(&self, b: &D3dMatrix) -> D3dMatrix {
        let mut out = [[0.0f32; 4]; 4];
        for (r, row) in out.iter_mut().enumerate() {
            for (c, v) in row.iter_mut().enumerate() {
                *v = (0..4).map(|k| self.0[r][k] * b.0[k][c]).sum();
            }
        }
        D3dMatrix(out)
    }

    /// The same transform for glam's column vectors.
    pub fn to_glam(&self) -> Mat4 {
        // glam's `from_cols_array_2d` takes columns: the rows of the D3DX matrix are the columns
        // of its transpose, which is the matrix for column vectors.
        Mat4::from_cols_array_2d(&self.0)
    }

    /// Transforms the point `p` (row vector with w = 1), without the divide by w.
    pub fn transform(&self, p: Vec3) -> [f32; 4] {
        let v = [p.x, p.y, p.z, 1.0];
        std::array::from_fn(|c| (0..4).map(|k| v[k] * self.0[k][c]).sum())
    }
}

fn normalize(v: Vec3) -> Vec3 {
    v / v.length()
}

/// `D3DXMatrixLookAtRH`.
pub fn look_at_rh(eye: Vec3, at: Vec3, up: Vec3) -> D3dMatrix {
    let z = normalize(eye - at);
    let x = normalize(up.cross(z));
    let y = z.cross(x);
    D3dMatrix([
        [x.x, y.x, z.x, 0.0],
        [x.y, y.y, z.y, 0.0],
        [x.z, y.z, z.z, 0.0],
        [-x.dot(eye), -y.dot(eye), -z.dot(eye), 1.0],
    ])
}

/// `D3DXMatrixOrthoOffCenterRH`.
pub fn ortho_off_center_rh(l: f32, r: f32, b: f32, t: f32, zn: f32, zf: f32) -> D3dMatrix {
    D3dMatrix([
        [2.0 / (r - l), 0.0, 0.0, 0.0],
        [0.0, 2.0 / (t - b), 0.0, 0.0],
        [0.0, 0.0, 1.0 / (zn - zf), 0.0],
        [(l + r) / (l - r), (t + b) / (b - t), zn / (zn - zf), 1.0],
    ])
}

/// The shadow camera of a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowCamera {
    pub view: D3dMatrix,
    pub projection: D3dMatrix,
    /// `g_ShadowViewProj`: `view * projection`.
    pub view_proj: D3dMatrix,
}

impl ShadowCamera {
    /// `SetupShadowFrameGlobals` for the visible rectangle `top_left`..`bottom_right`
    /// (`FrameGlobals::g_WorldTopLeftVisibleUnclamped` and `g_WorldBottomRightVisibleUnclamped`:
    /// the minimum and the maximum corner) and the direction of the light.
    pub fn new(top_left: Vec3, bottom_right: Vec3, shadow_dir: Vec3) -> Self {
        let center = (top_left + bottom_right) * 0.5;
        let width = top_left.x - bottom_right.x;
        let height = top_left.y - bottom_right.y;
        let eye = shadow_dir * LIGHT_DISTANCE + center;
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
        }
    }

    /// `g_ShadowVP` for glam: `clip = view_proj() * point`.
    pub fn view_proj(&self) -> Mat4 {
        self.view_proj.to_glam()
    }
}

/// The visible rectangle in the original's basis: the minimum and the maximum corner of the
/// points where the four edges of the view frustum meet the two planes of [`VISIBLE_HEIGHTS`].
/// `view_proj` and `eye` are ours, `ours_to_orig` takes the points over. An edge that does not
/// reach a plane (it looks above the horizon) is left out; with no edge left the rectangle is a
/// square of `fallback` around the eye.
// re: GameViewState::CalcVisibleRegion (0x0883beb8): 8 plane intersections, min and max of x, y
pub fn visible_rect(
    view_proj: &Mat4,
    eye: Vec3,
    ours_to_orig: &Mat4,
    fallback: f32,
) -> (Vec3, Vec3) {
    let inverse = view_proj.inverse();
    let mut min = Vec2::splat(f32::INFINITY);
    let mut max = Vec2::splat(f32::NEG_INFINITY);
    for corner in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        // A point of the edge: the far end of the corner's ray.
        let far = inverse.project_point3(Vec3::new(corner.0, corner.1, 1.0));
        let dir = far - eye;
        for height in VISIBLE_HEIGHTS {
            // Our Y is the original's z.
            if dir.y.abs() < 1e-6 {
                continue;
            }
            let t = (height - eye.y) / dir.y;
            if t <= 0.0 {
                continue;
            }
            // Past the far plane nothing is drawn: an edge that meets the plane beyond it is bounded by its far
            // end. A strategic camera always meets the ground first. A low eye's edge just under the horizon
            // meets it thousands of times farther: the rectangle was millions of units wide, the overlay map
            // and the fog mask stretched over it (the ground went white with the pitch), and the grid of the
            // overlay walked its cells until a vector overflowed (first-person crash, 2026-11-07).
            let t = t.min(1.0);
            let p = ours_to_orig.transform_point3(eye + dir * t);
            min = min.min(p.truncate());
            max = max.max(p.truncate());
        }
    }
    if min.x > max.x {
        return fixed_rect(eye, ours_to_orig, fallback);
    }
    (min.extend(0.0), max.extend(0.0))
}

/// The square of side `2 * half` around the eye in the original's basis: the rectangle of a camera that
/// asks for a fixed range (client-hooks C2, a low eye whose frustum cuts only a strip out of the ground), and
/// the fallback of [`visible_rect`] when no edge reaches the ground.
pub fn fixed_rect(eye: Vec3, ours_to_orig: &Mat4, half: f32) -> (Vec3, Vec3) {
    let c = ours_to_orig.transform_point3(eye).truncate();
    let (min, max) = (c - Vec2::splat(half), c + Vec2::splat(half));
    (min.extend(0.0), max.extend(0.0))
}

/// What a `ShadowLevel` makes of the targets (`BuildShadowBuffers`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShadowSize {
    /// Side of the blurred map, `DynamicShadowTexture`.
    pub side: u32,
    /// The blur pass runs; without it the raw texture is the map.
    pub blur: bool,
}

impl ShadowSize {
    /// `None` for level 0: the original makes a cleared 4 x 4 placeholder and has no pass.
    // re: TerrainSystem::BuildShadowBuffers (0x0867ef40): level 1 0x400 without blur, 2 0x400 with,
    // 3 0x800 with, else the placeholder
    pub fn of_level(level: i64) -> Option<ShadowSize> {
        match level {
            1 => Some(ShadowSize {
                side: 0x400,
                blur: false,
            }),
            2 => Some(ShadowSize {
                side: 0x400,
                blur: true,
            }),
            l if l >= 3 => Some(ShadowSize {
                side: 0x800,
                blur: true,
            }),
            _ => None,
        }
    }
}

/// The smallest side the quality may give the blurred map.
const MIN_SIDE: u32 = 128;

// SIMPLIFIED(SIMP-exe-1113): sizes by a divisor of the level's size; the original has the level only.
/// How big the shadow map is, as the player picks it (ours, not a key of the original): `High` is
/// the size `ShadowLevel` makes, `Medium` half and `Low` a quarter of it in each direction (the
/// raw map and the blurred one both), `Off` draws no shadows at all (everything is lit).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ShadowQuality {
    Off,
    Low,
    Medium,
    #[default]
    High,
}

impl ShadowQuality {
    pub const ALL: [ShadowQuality; 4] = [Self::Off, Self::Low, Self::Medium, Self::High];

    /// The default of a device: `Medium` on a touch device, else `High` (the original's size).
    pub fn default_for(touch: bool) -> Self {
        if touch { Self::Medium } else { Self::High }
    }

    /// The number the key-value store keeps (0 Off, 1 Low, 2 Medium, 3 High).
    pub fn index(self) -> u32 {
        self as u32
    }

    /// The quality of a stored number; `None` for one that is none of them.
    pub fn from_index(index: u32) -> Option<Self> {
        Self::ALL.get(index as usize).copied()
    }

    /// By how much each side of the maps is divided; `None` for `Off`.
    pub fn divisor(self) -> Option<u32> {
        match self {
            Self::Off => None,
            Self::Low => Some(4),
            Self::Medium => Some(2),
            Self::High => Some(1),
        }
    }

    // SIMPLIFIED(SIMP-exe-1131): at Low the ground and the static models do not cast, only the units,
    // landmarks and cities do; the original draws every caster at every level.
    /// Whether the static models (the ground's trees and props, the biggest casters) are drawn into
    /// the map: not at `Low`, where the map is small and the cost of the pass is what the player saves.
    pub fn statics_cast(self) -> bool {
        matches!(self, Self::Medium | Self::High)
    }

    /// The `ShadowLevel` the frame is drawn with. `Off` is level 0. A player's pick (`chosen`) of
    /// `Low`, `Medium` or `High` is a real map even where the quality keys of the world say
    /// `ShadowLevel` 0 (the Low preset of the world's detail): the biggest map, cut by the divisor.
    /// Without a pick (the goldens, `?gfx=ShadowLevel:0`) the level is the key.
    pub fn frame_level(self, key_level: i64, chosen: bool) -> i64 {
        match self.divisor() {
            None => 0,
            Some(_) if chosen && key_level <= 0 => 3,
            Some(_) => key_level,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }
}

impl ShadowSize {
    /// The size with each side divided by `divisor` (the blurred map keeps at least [`MIN_SIDE`]).
    pub fn divided(self, divisor: u32) -> ShadowSize {
        ShadowSize {
            side: (self.side / divisor.max(1)).max(MIN_SIDE.min(self.side)),
            blur: self.blur,
        }
    }
}

/// The size of the raw map: the screen with each side divided by `divisor`, one pixel at least.
pub fn raw_size(screen: (u32, u32), divisor: u32) -> (u32, u32) {
    let d = divisor.max(1);
    ((screen.0 / d).max(1), (screen.1 / d).max(1))
}

/// `g_ScreenPixelSize` of the blur: `(1/w, 1/h, -1/w, -1/h)` of the screen.
// re: TerrainSystem::BuildShadowBuffers (TerrainSystem.cpp:329-336): the texel size of the screen
pub fn screen_pixel_size(width: u32, height: u32) -> [f32; 4] {
    let (w, h) = (width.max(1) as f32, height.max(1) as f32);
    [1.0 / w, 1.0 / h, -1.0 / w, -1.0 / h]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lighting::globals::SHADOW_DIR;

    fn camera() -> ShadowCamera {
        ShadowCamera::new(
            Vec3::new(-300.0, -200.0, 0.0),
            Vec3::new(500.0, 400.0, 0.0),
            Vec3::from_array(SHADOW_DIR),
        )
    }

    #[test]
    fn the_matrices_are_the_ones_of_the_d3dx_formulas() {
        let c = camera();
        let center = Vec3::new(100.0, 100.0, 0.0);
        let dir = Vec3::from_array(SHADOW_DIR);
        let eye = dir * 5000.0 + center;
        // LookAtRH: the eye goes to the origin, the centre to the minus z axis.
        let at_eye = c.view.transform(eye);
        assert!(at_eye[..3].iter().all(|v| v.abs() < 1e-2), "{at_eye:?}");
        let at_center = c.view.transform(center);
        assert!(at_center[0].abs() < 1e-3 && at_center[1].abs() < 1e-3);
        assert!((at_center[2] + 5000.0).abs() < 1e-2, "{at_center:?}");
        // OrthoOffCenterRH(w/2, -w/2, h/2, -h/2, 30000, -30000), w = -800, h = -600:
        // 2/(r-l) = 2/(-(w/2) - w/2) = -2/w.
        let p = c.projection.0;
        assert!((p[0][0] - 2.0 / 800.0).abs() < 1e-9);
        assert!((p[1][1] - 2.0 / 600.0).abs() < 1e-9);
        assert!((p[2][2] - 1.0 / 60000.0).abs() < 1e-12);
        assert!((p[3][2] - 0.5).abs() < 1e-6);
        assert_eq!((p[3][0], p[3][1], p[3][3]), (0.0, 0.0, 1.0));
        // The middle of the rectangle is the middle of the screen, at depth 0.5 + 5000/60000.
        let ndc = c.view_proj.transform(center);
        assert!(ndc[0].abs() < 1e-4 && ndc[1].abs() < 1e-4, "{ndc:?}");
        assert!((ndc[2] - (0.5 - 5000.0 / 60000.0)).abs() < 1e-5, "{ndc:?}");
        assert_eq!(ndc[3], 1.0);
    }

    #[test]
    fn glam_gets_the_same_transform() {
        let c = camera();
        let m = c.view_proj();
        for p in [Vec3::new(10.0, 20.0, 5.0), Vec3::new(-250.0, 380.0, 60.0)] {
            let d3d = c.view_proj.transform(p);
            let g = m * p.extend(1.0);
            for k in 0..4 {
                assert!((d3d[k] - g[k]).abs() < 1e-4, "{k}: {d3d:?} {g}");
            }
        }
        // And it is the product the original takes: view, then projection.
        assert_eq!(c.view_proj, c.view.mul(&c.projection));
    }

    #[test]
    fn the_view_agrees_with_glams_look_at() {
        let eye = Vec3::new(2500.0, -1200.0, 4300.0);
        let at = Vec3::new(100.0, 100.0, 0.0);
        let ours = look_at_rh(eye, at, Vec3::Y).to_glam();
        let glam = glam::camera::rh::view::look_at_mat4(eye, at, Vec3::Y);
        assert!(ours.abs_diff_eq(glam, 5e-3), "{ours:?} {glam:?}");
    }

    #[test]
    fn the_size_follows_the_level() {
        assert_eq!(ShadowSize::of_level(0), None);
        assert_eq!(
            ShadowSize::of_level(1),
            Some(ShadowSize {
                side: 1024,
                blur: false
            })
        );
        assert_eq!(
            ShadowSize::of_level(2),
            Some(ShadowSize {
                side: 1024,
                blur: true
            })
        );
        assert_eq!(
            ShadowSize::of_level(3),
            Some(ShadowSize {
                side: 2048,
                blur: true
            })
        );
        let p = screen_pixel_size(1920, 1080);
        assert_eq!(p[0], -p[2]);
        assert_eq!(p[1], -p[3]);
        assert!((p[0] - 1.0 / 1920.0).abs() < 1e-9);
    }

    #[test]
    fn high_is_the_size_of_the_level_and_the_lower_ones_halve_it() {
        let level3 = ShadowSize::of_level(3).unwrap();
        for (q, side, raw) in [
            (ShadowQuality::High, 2048, (1920, 1080)),
            (ShadowQuality::Medium, 1024, (960, 540)),
            (ShadowQuality::Low, 512, (480, 270)),
        ] {
            let d = q.divisor().unwrap();
            assert_eq!(level3.divided(d).side, side, "{q:?}");
            assert_eq!(raw_size((1920, 1080), d), raw, "{q:?}");
        }
        assert_eq!(level3.divided(1), level3, "High keeps the original size");
        assert!(level3.divided(4).blur);
        assert_eq!(ShadowQuality::Off.divisor(), None);
        // The small maps never go under the floor, nor to nothing.
        assert_eq!(ShadowSize::of_level(2).unwrap().divided(64).side, 128);
        assert_eq!(raw_size((3, 1), 4), (1, 1));
    }

    #[test]
    fn the_blur_texel_follows_the_raw_map() {
        let raw = raw_size((1920, 1080), 2);
        let p = screen_pixel_size(raw.0, raw.1);
        assert!((p[0] - 1.0 / 960.0).abs() < 1e-9 && (p[1] - 1.0 / 540.0).abs() < 1e-9);
    }

    #[test]
    fn a_touch_device_gets_medium_shadows_and_a_desktop_high() {
        assert_eq!(ShadowQuality::default_for(true), ShadowQuality::Medium);
        assert_eq!(ShadowQuality::default_for(false), ShadowQuality::High);
    }

    #[test]
    fn the_stored_number_names_the_quality() {
        for q in ShadowQuality::ALL {
            assert_eq!(ShadowQuality::from_index(q.index()), Some(q));
        }
        assert_eq!(ShadowQuality::from_index(4), None);
    }

    /// The bug of the phone report: the world's Low detail has `ShadowLevel` 0, and a pick of Low
    /// shadows gave side 4 (no map) because the key won.
    #[test]
    fn a_picked_quality_other_than_off_is_a_real_map_even_where_the_key_says_zero() {
        // Each quality, with the key at 0 and a pick: the side the frame makes.
        let side = |q: ShadowQuality, key: i64, chosen: bool| {
            ShadowSize::of_level(q.frame_level(key, chosen))
                .map(|s| s.divided(q.divisor().unwrap_or(1)).side)
        };
        assert_eq!(side(ShadowQuality::High, 0, true), Some(2048));
        assert_eq!(side(ShadowQuality::Medium, 0, true), Some(1024));
        assert_eq!(side(ShadowQuality::Low, 0, true), Some(512));
        assert_eq!(side(ShadowQuality::Off, 3, true), None, "Off is no map");
        assert_eq!(side(ShadowQuality::Off, 0, true), None);
        // Without a pick the key is the level: the goldens and `?gfx=ShadowLevel:0` keep it.
        assert_eq!(side(ShadowQuality::High, 0, false), None);
        assert_eq!(side(ShadowQuality::High, 3, false), Some(2048));
        // A key above 0 stays what it is: ShadowLevel 2 is a 1024 map before the divisor.
        assert_eq!(side(ShadowQuality::Medium, 2, true), Some(512));
    }

    #[test]
    fn only_medium_and_high_draw_the_static_models_into_the_map() {
        let casts: Vec<_> = ShadowQuality::ALL
            .iter()
            .map(|q| (q.name(), q.statics_cast()))
            .collect();
        assert_eq!(
            casts,
            [
                ("Off", false),
                ("Low", false),
                ("Medium", true),
                ("High", true)
            ]
        );
    }

    #[test]
    fn the_visible_rectangle_of_a_downward_view_is_the_footprint() {
        // A camera 600 above (0, 0) looking straight down, in our basis: Y up.
        let eye = Vec3::new(0.0, 600.0, 0.0);
        let view = glam::camera::rh::view::look_at_mat4(eye, Vec3::ZERO, -Vec3::Z);
        let proj = glam::camera::rh::proj::directx::perspective(
            std::f32::consts::FRAC_PI_2,
            1.0,
            1.0,
            5000.0,
        );
        let b = crate::frame::basis_rotation();
        let (tl, br) = visible_rect(&(proj * view), eye, &b, 100.0);
        // At the ground (600 down) a 90 degree view sees +-600; at 127.5 it sees +-472.5. The
        // rectangle is the larger one, in the original's basis (x east, y north).
        assert!(
            (tl.x + 600.0).abs() < 1.0 && (br.x - 600.0).abs() < 1.0,
            "{tl} {br}"
        );
        assert!(
            (tl.y + 600.0).abs() < 1.0 && (br.y - 600.0).abs() < 1.0,
            "{tl} {br}"
        );
        // Looking at the sky leaves nothing: the fallback square around the eye.
        let sky = glam::camera::rh::view::look_at_mat4(eye, eye + Vec3::Y, Vec3::Z);
        let (tl, br) = visible_rect(&(proj * sky), eye, &b, 50.0);
        assert_eq!((br - tl).x, 100.0);
    }

    #[test]
    fn a_fixed_range_is_a_square_around_the_eye_whatever_the_view_cuts() {
        let b = crate::frame::basis_rotation();
        let eye = Vec3::new(40.0, 6.5, -25.0);
        let (tl, br) = fixed_rect(eye, &b, 300.0);
        let c = b.transform_point3(eye).truncate();
        assert_eq!((tl.truncate(), br.truncate()), (c - 300.0, c + 300.0));
        // The same square is the fallback of the view that sees no ground.
        let proj = glam::camera::rh::proj::directx::perspective(1.3, 1.5, 0.5, 3000.0);
        let high = Vec3::new(40.0, 600.0, -25.0);
        let sky = glam::camera::rh::view::look_at_mat4(high, high + Vec3::Y, Vec3::Z);
        assert_eq!(
            visible_rect(&(proj * sky), high, &b, 300.0),
            fixed_rect(high, &b, 300.0)
        );
        // A level look from 6.5 up cuts a long strip, not a square: that is what the range replaces.
        let level = glam::camera::rh::view::look_at_mat4(eye, eye - Vec3::Z, Vec3::Y);
        let (lo, hi) = visible_rect(&(proj * level), eye, &b, 300.0);
        let (w, h) = ((hi - lo).x, (hi - lo).y);
        assert!(w < 40.0 || h < 40.0 || (w - h).abs() > 1.0, "{lo} {hi}");
    }

    #[test]
    fn a_low_eye_near_the_horizon_sees_no_farther_than_its_far_plane() {
        let b = crate::frame::basis_rotation();
        let far = 3000.0;
        let proj =
            glam::camera::rh::proj::directx::perspective(75f32.to_radians(), 16.0 / 9.0, 0.5, far);
        let eye = Vec3::new(40.0, 6.5, -25.0);
        let c = b.transform_point3(eye).truncate();
        // Pitches around the horizon, where an edge meets the ground (or the plane above it) almost parallel.
        for deg in [
            -60.0f32, -40.0, -37.6, -37.5, -37.4, -20.0, -1.0, 0.0, 0.5, 1.0, 20.0, 37.4, 37.6,
            60.0,
        ] {
            let pitch = deg.to_radians();
            let dir = Vec3::new(0.0, pitch.sin(), -pitch.cos());
            let view = glam::camera::rh::view::look_at_mat4(eye, eye + dir, Vec3::Y);
            let (lo, hi) = visible_rect(&(proj * view), eye, &b, 300.0);
            for p in [lo.truncate(), hi.truncate()] {
                assert!(p.is_finite(), "pitch {deg}: {lo} {hi}");
                // The far corners lie within far / cos(half the diagonal field) of the eye: well under 2 far.
                assert!((p - c).length() < 2.0 * far, "pitch {deg}: {lo} {hi}");
            }
        }
    }
}
