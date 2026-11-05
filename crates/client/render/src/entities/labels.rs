//! Labels for `play-ui`: where unit flags and city banners go. Drawing is the UI's
//! (SIMP-014, task F15); this module gives world positions and the projection to the screen.

use glam::{Mat4, Vec2, Vec3, Vec4};

use crate::terrain::{TerrainSnapshot, terrain_height_at};
use crate::wrap::nearest_copy_x;

/// A flag or banner hangs this far over the ground point of its cell.
// re: `UnitFlagManager.lua:33`, `CityBannerManager.lua:15` (plot position + (0, 35, 0))
pub const LABEL_HEIGHT: f32 = 35.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitFlag {
    pub unit: i32,
    pub owner: i8,
    pub type_name: String,
    pub hp: i32,
    pub max_hp: i32,
    pub needs_orders: bool,
    pub civilian: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CityBanner {
    pub city: i32,
    pub owner: i8,
    pub name: String,
    pub population: i32,
    pub capital: bool,
    pub hp: i32,
    pub max_hp: i32,
}

/// The number of a turn on a path, at the end of that turn's stretch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathTurn {
    pub turn: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelData {
    UnitFlag(UnitFlag),
    CityBanner(CityBanner),
    PathTurn(PathTurn),
}

/// One label: the cell, the world point it hangs at (unwrapped; a wrapping map draws it in every
/// copy) and its data.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldLabel {
    pub cell: (i32, i32),
    pub position: Vec3,
    pub data: LabelData,
}

/// A label on the screen: pixel position (origin top left), NDC depth, and whether it can be
/// seen (in front of the camera, on screen, not behind terrain).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenLabel {
    pub pos: Vec2,
    pub depth: f32,
    pub visible: bool,
}

/// Samples along the line of sight tested against the terrain.
const SIGHT_SAMPLES: u32 = 24;
/// The last part of the line of sight (next to the label) is not tested: the label's own cell.
const SIGHT_END: f32 = 0.9;

/// Projects `world` with `view_proj` into a viewport of `viewport` pixels. `terrain` and the
/// camera `eye` enable the check for relief in the way.
pub fn project_label(
    world: Vec3,
    view_proj: &Mat4,
    viewport: Vec2,
    eye: Vec3,
    terrain: Option<&TerrainSnapshot>,
) -> ScreenLabel {
    let clip = *view_proj * Vec4::new(world.x, world.y, world.z, 1.0);
    if clip.w <= 1e-6 {
        return ScreenLabel {
            pos: Vec2::ZERO,
            depth: 1.0,
            visible: false,
        };
    }
    let ndc = clip.truncate() / clip.w;
    let pos = Vec2::new(
        (ndc.x * 0.5 + 0.5) * viewport.x,
        (0.5 - ndc.y * 0.5) * viewport.y,
    );
    let on_screen = ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0 && (0.0..=1.0).contains(&ndc.z);
    let visible = on_screen && terrain.is_none_or(|t| line_of_sight(t, eye, world));
    ScreenLabel {
        pos,
        depth: ndc.z,
        visible,
    }
}

/// [`project_label`] for a map that may wrap: `wrap_width` is the world width (`None`: no wrap).
/// The label is projected at the copy nearest the camera's X (`eye.x`; the game camera looks along
/// -Z, so the eye X is the focus X), so a flag over the seam appears on the side the camera is on.
pub fn project_wrapped_label(
    world: Vec3,
    wrap_width: Option<f32>,
    view_proj: &Mat4,
    viewport: Vec2,
    eye: Vec3,
    terrain: Option<&TerrainSnapshot>,
) -> ScreenLabel {
    let mut at = world;
    if let Some(w) = wrap_width {
        at.x = nearest_copy_x(world.x, eye.x, w);
    }
    project_label(at, view_proj, viewport, eye, terrain)
}

fn line_of_sight(terrain: &TerrainSnapshot, eye: Vec3, target: Vec3) -> bool {
    (1..=SIGHT_SAMPLES).all(|i| {
        let t = SIGHT_END * i as f32 / SIGHT_SAMPLES as f32;
        let p = eye.lerp(target, t);
        p.y >= terrain_height_at(terrain, Vec2::new(p.x, p.z))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::Relief;
    use crate::terrain::TerrainCell;
    use crate::terrain::test_util::{land, map};

    fn view(eye: Vec3, at: Vec3, up: Vec3) -> Mat4 {
        glam::camera::rh::view::look_at_mat4(eye, at, up)
    }

    fn proj() -> Mat4 {
        glam::camera::rh::proj::directx::perspective(1.0, 1.0, 1.0, 1000.0)
    }

    fn camera() -> (Mat4, Vec3) {
        let eye = Vec3::new(0.0, 100.0, 100.0);
        let view = view(eye, Vec3::ZERO, Vec3::Y);
        (proj() * view, eye)
    }

    #[test]
    fn point_above_the_target_lands_in_the_middle_of_the_screen() {
        let (vp, eye) = camera();
        let s = project_label(Vec3::ZERO, &vp, Vec2::new(800.0, 800.0), eye, None);
        assert!(s.visible);
        assert!(
            (s.pos - Vec2::new(400.0, 400.0)).length() < 1e-2,
            "{:?}",
            s.pos
        );
        let up = project_label(
            Vec3::new(0.0, 35.0, 0.0),
            &vp,
            Vec2::new(800.0, 800.0),
            eye,
            None,
        );
        assert!(up.pos.y < 400.0);
    }

    #[test]
    fn behind_the_camera_and_off_screen_are_invisible() {
        let (vp, eye) = camera();
        let behind = project_label(
            Vec3::new(0.0, 100.0, 500.0),
            &vp,
            Vec2::splat(800.0),
            eye,
            None,
        );
        assert!(!behind.visible);
        let off = project_label(
            Vec3::new(5000.0, 0.0, 0.0),
            &vp,
            Vec2::splat(800.0),
            eye,
            None,
        );
        assert!(!off.visible);
    }

    #[test]
    fn a_label_over_the_seam_is_shown_at_the_copy_nearest_the_camera() {
        let w = 6400.0;
        let eye = Vec3::new(30.0, 100.0, 100.0);
        let vp = proj() * view(eye, Vec3::new(30.0, 0.0, 0.0), Vec3::Y);
        let label = Vec3::new(w - 10.0, 0.0, 0.0);
        // Plain projection puts it 6360 units away: off screen.
        assert!(!project_label(label, &vp, Vec2::splat(800.0), eye, None).visible);
        // Wrapped, it is the copy at x = -10: next to the middle of the screen.
        let s = project_wrapped_label(label, Some(w), &vp, Vec2::splat(800.0), eye, None);
        assert!(
            s.visible && s.pos.x < 400.0 && s.pos.x > 100.0,
            "{:?}",
            s.pos
        );
        // Same label, camera on the other side: the copy stays where it is.
        let eye2 = Vec3::new(w - 40.0, 100.0, 100.0);
        let vp2 = proj() * view(eye2, Vec3::new(w - 40.0, 0.0, 0.0), Vec3::Y);
        let s2 = project_wrapped_label(label, Some(w), &vp2, Vec2::splat(800.0), eye2, None);
        assert!(s2.visible && s2.pos.x > 400.0);
        // A map without wrap does not move it.
        assert!(!project_wrapped_label(label, None, &vp, Vec2::splat(800.0), eye, None).visible);
    }

    #[test]
    fn a_label_behind_a_mountain_is_invisible() {
        // A mountain cell at the origin; the eye is low on one side, the label on the other.
        let s = map(5, 5, |x, y| {
            if (x, y) == (1, 0) {
                TerrainCell {
                    relief: Relief::Mountain,
                    ..land()
                }
            } else {
                land()
            }
        });
        let c = crate::hexgrid::cell_center(1, 0);
        let eye = Vec3::new(c.x - 120.0, 30.0, c.y);
        let label = Vec3::new(c.x + 60.0, 20.0, c.y);
        let vp = proj() * view(eye, label, Vec3::Y);
        let hidden = project_label(label, &vp, Vec2::splat(800.0), eye, Some(&s));
        assert!(!hidden.visible);
        let shown = project_label(label, &vp, Vec2::splat(800.0), eye, None);
        assert!(shown.visible);
        // From high above the same label is visible.
        let high = Vec3::new(c.x - 120.0, 200.0, c.y);
        let vp = proj() * view(high, label, Vec3::Y);
        assert!(project_label(label, &vp, Vec2::splat(800.0), high, Some(&s)).visible);
    }
}
