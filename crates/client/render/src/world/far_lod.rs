//! The far-view LOD (the setting `FarLOD`): what is
//! farther from the eye than [`FAR_LOD_DISTANCE`] is drawn the cheap way. The trees of a block of
//! the map show one variant of their model ([`super::forest::draw`]), and the units, landmarks and
//! cities leave the shadow map and the reflection ([`super::unit::cull`]); their models in the main
//! view stay as they are.
//!
//! The distance is beyond anything the game's camera sees at the zoom the game starts at (the top
//! corners of a 16:9 view are 925 units away at level 14.25), so at the zoom of play nothing
//! changes; at the farthest zoom (level 20.5) the bottom of the screen is 1254 units away and most
//! of the view is far.
//!
//! The setting is ours, not a key of the original: no LOD switch is known in `ForestVisSystem` or
//! in the unit draw of the original (not reversed); its answer to the far view is the strategic
//! view. With `FarLOD = 0` the frame is the original's way at every distance.

use glam::Vec3;

/// Distance from the eye (our basis, world units) beyond which a thing is far.
pub const FAR_LOD_DISTANCE: f32 = 1400.0;

/// The smallest distance from `eye` to a point of the box `[min, max]` (0 inside it).
pub fn box_distance(eye: Vec3, min: Vec3, max: Vec3) -> f32 {
    (eye.clamp(min, max) - eye).length()
}

/// The largest distance from `eye` to a point of the box `[min, max]`.
pub fn box_reach(eye: Vec3, min: Vec3, max: Vec3) -> f32 {
    (eye - min).abs().max((eye - max).abs()).length()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera_params as p;
    use crate::game_camera::GameCamera;

    /// The farthest ground point the game camera sees, over the corners and the middle of the edges
    /// of a 16:9 view.
    fn farthest_seen(level: f32) -> (f32, f32) {
        let mut c = GameCamera::new(glam::Vec2::ZERO);
        c.set_level(level);
        let aspect = 16.0 / 9.0;
        let eye = c.eye();
        let (mut near, mut far) = (f32::MAX, 0.0f32);
        for (x, y) in [
            (-1.0, 1.0),
            (0.0, 1.0),
            (1.0, 1.0),
            (-1.0, -1.0),
            (0.0, -1.0),
            (1.0, -1.0),
        ] {
            let g = c
                .ground_point(glam::Vec2::new(x, y), aspect)
                .expect("the view meets the ground");
            let d = g.distance(eye);
            near = near.min(d);
            far = far.max(d);
        }
        (near, far)
    }

    #[test]
    fn nothing_is_far_at_the_zoom_of_play_and_most_is_at_the_farthest() {
        let (_, start) = farthest_seen(p::START_LEVEL);
        assert!(start < FAR_LOD_DISTANCE * 0.7, "start level sees {start}");
        // A notch of the wheel out of the start is still all near.
        let (_, notch) = farthest_seen(p::START_LEVEL + p::WHEEL_STEP_LEVELS);
        assert!(notch < FAR_LOD_DISTANCE, "a notch out sees {notch}");
        let (near, far) = farthest_seen(p::LEVEL_MAX);
        assert!(
            near < FAR_LOD_DISTANCE && far > FAR_LOD_DISTANCE * 1.4,
            "{near} {far}"
        );
    }

    #[test]
    fn the_distances_of_a_box() {
        let (lo, hi) = (Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 10.0, 10.0));
        assert_eq!(box_distance(Vec3::new(5.0, 5.0, 5.0), lo, hi), 0.0);
        assert_eq!(box_distance(Vec3::new(13.0, 5.0, 14.0), lo, hi), 5.0);
        assert_eq!(box_reach(Vec3::new(5.0, 5.0, 5.0), lo, hi), 75f32.sqrt());
        assert_eq!(
            box_reach(Vec3::new(-3.0, 0.0, 0.0), lo, hi),
            13f32.hypot(10f32.hypot(10.0))
        );
    }
}
