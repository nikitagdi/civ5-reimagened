//! The map of the overlay (world-overlays WOV2, RW-3, `OverlaySystem`).
//!
//! The original draws the grid, the borders and the highlights into one flat texture of the map, the
//! target `g_TexHandles[0]` ("Terrain Overlay": 2048 x 2048 at `OverlayLevel` 1 and 2, 1024 x 1024 at 0),
//! seen through `m_mOverlayProj`: an orthographic projection of the ground that the camera sees,
//! cut from the clip planes, snapped to 64 world units and padded by 64 (`SetupFrameGlobals`).
//! At `OverlayLevel` 2 (and `Use Screen Space Overlay` 1) a second pass lays that map over the
//! relief, cell by cell, into a target of the screen's size ("Screen Overlay", `cells.rs`), which the
//! final shader of the ground reads.
//!
//! This file is the geometry of the map and the order of the two steps; the grid is in `grid.rs`,
//! the cell pass in `cells.rs`.

use glam::{Mat4, Vec2, Vec3, Vec4};

/// The projection is snapped to this many world units.
// re: OverlaySystem.cpp:26 (kProjectionSnapMask = ~63)
pub const SNAP: i32 = 64;
/// And padded by one snap cell before the snap.
// re: OverlaySystem.cpp:27 (kProjectionPadding)
pub const PADDING: f32 = 64.0;

/// Side of the map of the overlay for an `OverlayLevel`.
// re: OverlaySystem::Startup 0x088c5dec (RW-3): 0x800 at levels 1 and 2, 0x400 at 0
pub fn map_side(overlay_level: i64) -> u32 {
    if overlay_level >= 1 { 2048 } else { 1024 }
}

/// The cell pass runs: `OverlayLevel` is 2 and `Use Screen Space Overlay` is 1 (world-terrain 4.8:
/// "только при OverlayLevel = 2 и Use Screen Space Overlay = 1"; the target "Screen Overlay" exists
/// only at level 2).
pub fn screen_overlay_used(overlay_level: i64, use_screen_space_overlay: i64) -> bool {
    overlay_level >= 2 && use_screen_space_overlay != 0
}

/// `static_cast<float>(static_cast<int>(bound) & ~63)`.
fn snap(bound: f32) -> f32 {
    ((bound as i32) & !(SNAP - 1)) as f32
}

/// The rectangle of the ground the map covers, original basis (x east, y north).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapView {
    pub min: Vec2,
    pub max: Vec2,
}

impl MapView {
    /// `SetupFrameGlobals`: the rectangle `lo`..`hi` the camera sees (the corners of its side planes
    /// on the ground and on the plane 127.5 above it), padded and snapped.
    pub fn of(lo: Vec2, hi: Vec2) -> Self {
        Self {
            min: Vec2::new(snap(lo.x - PADDING), snap(lo.y - PADDING)),
            max: Vec2::new(snap(hi.x + PADDING), snap(hi.y + PADDING)),
        }
    }

    pub fn size(&self) -> Vec2 {
        self.max - self.min
    }

    /// `D3DXMatrixOrthoOffCenterRH`: a point of the ground (original basis) to the clip space of the
    /// map (x right, y up, the plane at depth 0.5).
    pub fn projection(&self) -> Mat4 {
        let s = Vec2::splat(2.0) / self.size();
        Mat4::from_cols(
            Vec4::new(s.x, 0.0, 0.0, 0.0),
            Vec4::new(0.0, s.y, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 0.0),
            Vec4::new(-1.0 - s.x * self.min.x, -1.0 - s.y * self.min.y, 0.5, 1.0),
        )
    }

    /// Where a point of the ground is in the map: (0, 0) top left, (1, 1) bottom right.
    pub fn uv(&self, p: Vec2) -> Vec2 {
        let t = (p - self.min) / self.size();
        Vec2::new(t.x, 1.0 - t.y)
    }

    /// A point of our world to the clip space of the map.
    pub fn world_to_map(&self, ours_to_orig: &Mat4) -> Mat4 {
        self.projection() * *ours_to_orig
    }

    /// The rectangle in our basis (`x` and `z`): smallest and largest corner.
    pub fn ours_rect(&self, ours_to_orig: &Mat4) -> (Vec2, Vec2) {
        let to_ours = ours_to_orig.inverse();
        let mut lo = Vec2::splat(f32::INFINITY);
        let mut hi = Vec2::splat(f32::NEG_INFINITY);
        for c in [
            self.min,
            Vec2::new(self.max.x, self.min.y),
            Vec2::new(self.min.x, self.max.y),
            self.max,
        ] {
            let p = to_ours.transform_point3(Vec3::new(c.x, c.y, 0.0));
            lo = lo.min(Vec2::new(p.x, p.z));
            hi = hi.max(Vec2::new(p.x, p.z));
        }
        (lo, hi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_map_is_2048_squared_from_level_1_and_1024_below() {
        assert_eq!([0, 1, 2].map(map_side), [1024, 2048, 2048]);
    }

    #[test]
    fn the_cell_pass_needs_level_2_and_the_screen_space_key() {
        assert!(screen_overlay_used(2, 1));
        assert!(!screen_overlay_used(1, 1));
        assert!(!screen_overlay_used(2, 0));
        assert!(!screen_overlay_used(0, 1));
    }

    #[test]
    fn the_bounds_are_padded_by_64_and_snapped_to_64() {
        let v = MapView::of(Vec2::new(100.5, -30.0), Vec2::new(300.2, 410.0));
        // 100.5 - 64 = 36.5 -> 36 -> 0; -30 - 64 = -94 -> -94 & ~63 = -128 (two's complement);
        // 300.2 + 64 = 364.2 -> 364 -> 320; 410 + 64 = 474 -> 448.
        assert_eq!(v.min, Vec2::new(0.0, -128.0));
        assert_eq!(v.max, Vec2::new(320.0, 448.0));
        for b in [v.min, v.max] {
            assert_eq!(b.x % 64.0, 0.0);
            assert_eq!(b.y % 64.0, 0.0);
        }
        // The rectangle holds what was asked for.
        assert!(v.min.x <= 100.5 && v.max.x >= 300.2 && v.min.y <= -30.0 && v.max.y >= 410.0);
    }

    #[test]
    fn the_projection_maps_the_rectangle_to_the_clip_square_and_north_is_the_top() {
        let v = MapView::of(Vec2::new(0.0, 0.0), Vec2::new(640.0, 320.0));
        let p = v.projection();
        let at = |x: f32, y: f32| p.project_point3(Vec3::new(x, y, 9.0));
        assert!((at(v.min.x, v.min.y) - Vec3::new(-1.0, -1.0, 0.5)).length() < 1e-6);
        assert!((at(v.max.x, v.max.y) - Vec3::new(1.0, 1.0, 0.5)).length() < 1e-6);
        assert_eq!(v.uv(Vec2::new(v.min.x, v.max.y)), Vec2::new(0.0, 0.0));
        assert_eq!(v.uv(Vec2::new(v.max.x, v.min.y)), Vec2::new(1.0, 1.0));
    }

    #[test]
    fn our_rectangle_holds_the_whole_map() {
        let v = MapView::of(Vec2::new(0.0, 0.0), Vec2::new(640.0, 320.0));
        let (lo, hi) = v.ours_rect(&crate::frame::basis_rotation());
        // Our z is the south: the northern edge is the smaller z.
        assert_eq!(lo.x, v.min.x);
        assert_eq!(hi.x, v.max.x);
        assert!((lo.y + v.max.y).abs() < 1e-3 && (hi.y + v.min.y).abs() < 1e-3);
    }
}
