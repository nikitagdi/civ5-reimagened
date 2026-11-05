//! Every number of the original game camera in one place.
//!
//! `// re:` marks a value confirmed by reverse (source from the spec table). A SIMP-020 marker
//! marks what is still ours: keyboard zoom, touchpad wheel, the zoom after `lookAt`, the inertia option.

use std::f32::consts::FRAC_PI_4;

// --- Projection and pose ---

/// Camera pitch above the horizon, constant, no rotation.
// re: StartupGame -> Pitch(pi/4)
pub const PITCH: f32 = FRAC_PI_4;
/// Vertical FOV numerator, degrees: `fov = max(MIN_FOV_DEG, VIEW_FOV_DEG / sqrt(1 + aspect^2))`.
// re: StartupGame, ViewFOV() = 60
pub const VIEW_FOV_DEG: f32 = 60.0;
/// Lower bound of the vertical FOV, degrees.
// re: StartupGame
pub const MIN_FOV_DEG: f32 = 26.5;
/// Near clip plane, world units.
// re: ViewMax() = 4000 / StartupGame
pub const NEAR: f32 = 50.0;
/// Far clip plane, world units.
// re: ViewMax() = 4000
pub const FAR: f32 = 4000.0;
/// Height of the ground plane the focus sits on (plain terrain).
// re: plain height 12.0, Africa chunk height maps
pub const FOCUS_HEIGHT: f32 = 12.0;

// --- Zoom ---

/// `h = ZOOM_BASE_HEIGHT * exp(ZOOM_RATE * level)`.
// re: CvCamera::ZoomAlongVector
pub const ZOOM_BASE_HEIGHT: f32 = 50.0;
/// See `ZOOM_BASE_HEIGHT`.
// re: CvCamera::ZoomAlongVector
pub const ZOOM_RATE: f32 = 0.15;
/// Closest zoom level (h = 260).
// re: ZoomAlongVector
pub const LEVEL_MIN: f32 = 11.0;
/// Farthest zoom level (h = 1083).
// re: ZoomAlongVector
pub const LEVEL_MAX: f32 = 20.5;
/// Zoom levels per wheel notch (height x1.35).
// re: ProcessMouse
pub const WHEEL_STEP_LEVELS: f32 = 2.0;
/// Level at game start: the camera after `StartupGame` sits at (0, -424.26, 424.26), h = 424.26.
// re: StartupGame, level = ln(424.26 / 50) / 0.15 = 14.25
// SIMPLIFIED(SIMP-020): the zoom that `lookAt` / event 130 set after `StartupGame` is not reversed
pub const START_LEVEL: f32 = 14.25;
/// Zoom levels per `+` / `-` / PgUp / PgDn press; the exe C++ has no keyboard zoom.
// SIMPLIFIED(SIMP-020): keys and step are ours, the original zoom keys (if Lua has any) are unknown
pub const KEY_ZOOM_STEP_LEVELS: f32 = 2.0;
/// Touchpad pixels that count as one wheel notch; a wheel event of lines is exactly one notch.
// SIMPLIFIED(SIMP-020): pixel-mode wheels (touchpads) do not exist in the original
pub const PIXELS_PER_WHEEL_LINE: f64 = 50.0;

// --- Scrolling and dragging ---

/// Arrow-key scroll speed in ground units per second per unit of camera height `h`.
// re: CvCamera::Update, "h units per second"
pub const SCROLL_SPEED_PER_HEIGHT: f32 = 1.0;
/// Width of the screen-edge scroll strip, pixels (fullscreen only).
// re: InGame.lua:1155-1199
pub const EDGE_SCROLL_PX: f32 = 2.0;
/// A press turns into a drag once the pointer is more than this far from the press point along
/// X or along Y (strict, per axis), pixels. Also the click limit of the right button.
// re: UIManager::ProcessCameraDrag, kDragStartPixels = 4.0 (UIManager.cpp:16, :53-57)
pub const DRAG_THRESHOLD_PX: f32 = 4.0;
/// Release velocity = time-weighted mean of the last two drag frames times this.
// re: kReleaseSpeedScale (CvCamera.cpp:22, :243-245)
pub const INERTIA_RELEASE_SCALE: f32 = 0.9;
/// Drag frames kept for the release velocity (two-slot ring in the original).
// re: CvCamera::m_aMoveSamples (CvCamera.cpp:218-227)
pub const INERTIA_SAMPLES: usize = 2;
/// Glide duration `d = |v| * INERTIA_SPEED_PER_HEIGHT / h + INERTIA_BASE_S`; `d < 2 ? d / 2 : 1`.
// re: CvCamera::Update (CvCamera.cpp:249-250)
pub const INERTIA_SPEED_PER_HEIGHT: f32 = 0.5;
/// See `INERTIA_SPEED_PER_HEIGHT`, seconds.
// re: CvCamera::Update (CvCamera.cpp:249)
pub const INERTIA_BASE_S: f32 = 0.125;
/// Whether the glide is on. The original reads option byte 0x14 of the game options store.
// SIMPLIFIED(SIMP-020): the default of option byte 0x14 (drag inertia) is not reversed, taken as on
pub const INERTIA_ENABLED: bool = true;

/// Glide duration in seconds for a release speed `speed` (ground units per second) at camera
/// height `h`: 0.0625 .. 1 s.
pub fn inertia_duration(speed: f32, h: f32) -> f32 {
    let d = speed * INERTIA_SPEED_PER_HEIGHT / h + INERTIA_BASE_S;
    if d < 2.0 { d * 0.5 } else { 1.0 }
}

/// Speed factor at glide progress `t` (0..1): `0.5 * cos(pi * t) + 0.5`.
// re: CvCamera::Update, sin(t * pi + pi / 2) * 0.5 + 0.5 (CvCamera.cpp:266-267)
pub fn inertia_factor(t: f32) -> f32 {
    0.5 * (std::f32::consts::PI * t).cos() + 0.5
}

/// Camera height above the focus for a zoom level.
pub fn height_for_level(level: f32) -> f32 {
    ZOOM_BASE_HEIGHT * (ZOOM_RATE * level).exp()
}

/// Zoom level of a camera height (`RecalculateCurrentZoomLevel`).
pub fn level_for_height(height: f32) -> f32 {
    (height / ZOOM_BASE_HEIGHT).ln() / ZOOM_RATE
}

// --- Following the target (zoom and every look-at) ---

/// The `k` of the follow easing.
// re: CvCamera::Update follow block, DAT_09212f14 = 1.025 (night answers 1a/1b, E6)
pub const FOLLOW_K: f32 = 1.025;
/// The camera stops following within this distance of the target (no snap).
// re: CvCamera::Update, d > 1.0 (DAT_0912c9e0)
pub const FOLLOW_MIN_DISTANCE: f32 = 1.0;

/// Distance to the target after one frame of `dt` seconds, from the distance `d` now:
/// `k * max(0, (d / k)^0.25 / k - dt)^4`, and no move at all within `FOLLOW_MIN_DISTANCE`.
// re: CvCamera::Update follow block (CvCamera.cpp:317-361), exponent 0.25 = 0x3E800000
pub fn follow_distance(d: f32, dt: f32) -> f32 {
    if d.is_nan() || d <= FOLLOW_MIN_DISTANCE {
        return d;
    }
    let t = ((d / FOLLOW_K).powf(0.25) / FOLLOW_K - dt).max(0.0);
    FOLLOW_K * t.powi(4)
}

/// Vertical FOV in radians for a viewport aspect (width / height).
pub fn fov_y_for_aspect(aspect: f32) -> f32 {
    let deg = (VIEW_FOV_DEG / (1.0 + aspect * aspect).sqrt()).max(MIN_FOV_DEG);
    deg.to_radians()
}
