//! Wrapping along X: which map copies reach the screen and
//! which copy of an object is nearest the camera. Pure math, no GPU.
//!
//! Drawing no longer loops over copies by hand: a copy is a view (`frame::views`), and
//! `frame::copy_shifts` asks [`visible_x_range`] and [`copy_visible`] which copies to call the
//! subsystems for. [`nearest_copy_x`] stays for labels.

use glam::{Mat4, Vec3};

use crate::hexgrid::HEX_WIDTH;

/// Slack added to the visible X range: relief and trees lean over the ground line, and the
/// copy's own edge cell reaches half a hex past its last centre.
const RANGE_MARGIN: f32 = 2.0 * HEX_WIDTH + 64.0;

/// World X range of the ground (height 0) seen through `view_proj`, widened by a margin. A corner
/// of the screen that does not reach the ground contributes its point on the far plane.
pub fn visible_x_range(view_proj: &Mat4) -> (f32, f32) {
    let inv = view_proj.inverse();
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for (nx, ny) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let near = inv.project_point3(Vec3::new(nx, ny, 0.0));
        let far = inv.project_point3(Vec3::new(nx, ny, 1.0));
        let dir = far - near;
        let t = if dir.y < -1e-6 { -near.y / dir.y } else { 1.0 };
        let p = near + dir * t.clamp(0.0, 1.0);
        lo = lo.min(p.x);
        hi = hi.max(p.x);
    }
    if lo > hi {
        return (f32::NEG_INFINITY, f32::INFINITY);
    }
    (lo - RANGE_MARGIN, hi + RANGE_MARGIN)
}

/// Does the copy of the map shifted by `shift` (cells from `-HEX_WIDTH / 2` to
/// `width + HEX_WIDTH / 2`) overlap the visible X `range`?
pub fn copy_visible(shift: f32, width: f32, range: (f32, f32)) -> bool {
    shift + width + 0.5 * HEX_WIDTH >= range.0 && shift - 0.5 * HEX_WIDTH <= range.1
}

/// The copy of `x` (in `x + k * width`) nearest to `near_x`, e.g. the camera's X.
pub fn nearest_copy_x(x: f32, near_x: f32, width: f32) -> f32 {
    if width <= 0.0 {
        return x;
    }
    x + width * ((near_x - x) / width).round()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_camera::GameCamera;
    use glam::Vec2;

    const W: f32 = 64.0 * 40.0;

    #[test]
    fn nearest_copy_picks_the_closest_period() {
        assert_eq!(nearest_copy_x(100.0, 120.0, W), 100.0);
        // The camera just past the seam sees a label from the right edge on its left.
        assert_eq!(nearest_copy_x(W - 30.0, 20.0, W), -30.0);
        // And the other way round.
        assert_eq!(nearest_copy_x(30.0, W - 20.0, W), W + 30.0);
        assert_eq!(nearest_copy_x(5.0, 7.0, 0.0), 5.0);
    }

    #[test]
    fn copies_follow_the_camera_across_the_seam() {
        let shifts = [0.0, -W, W];
        let seen = |fx: f32| -> Vec<bool> {
            let mut c = GameCamera::new(Vec2::new(fx, -400.0));
            c.set_level(20.0);
            let range = visible_x_range(&c.view_proj(16.0 / 9.0));
            shifts.iter().map(|&s| copy_visible(s, W, range)).collect()
        };
        assert_eq!(seen(W * 0.5), [true, false, false], "middle: one copy");
        assert_eq!(
            seen(40.0),
            [true, true, false],
            "just right of the seam: the left copy"
        );
        assert_eq!(
            seen(W - 40.0),
            [true, false, true],
            "just left of it: the right copy"
        );
    }

    #[test]
    fn range_contains_the_focus() {
        let mut c = GameCamera::new(Vec2::new(1000.0, -300.0));
        c.set_level(14.0);
        let (lo, hi) = visible_x_range(&c.view_proj(1.5));
        assert!(lo < 1000.0 && hi > 1000.0 && hi - lo < 4000.0, "{lo} {hi}");
    }
}
