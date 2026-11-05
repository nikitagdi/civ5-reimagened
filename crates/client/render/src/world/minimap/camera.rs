//! The camera of the minimap and the rectangle of the game camera (world-overlays WOV3, RW-21).
//!
//! `DeriveCameraParams` (`cvMinimapRenderer.cpp:257`) fits the explored region to the minimap's
//! aspect, pads it by the gutter and works out how finely the hex edges are cut; on a wrapping map
//! a region that leaves the world on one side gets a second, shifted view. `SetupCamera` is an
//! offset and a scale (no matrix): the centre of the view and 2 / its size.
//!
//! World units are the original's (x east, y north).

use glam::Vec2;

use crate::hexgrid::{CORNER_RADIUS, HEX_WIDTH};

/// Thickness of the camera rectangle in pixels.
// re: config.ini [MiniMap] CameraRectThickness (the user's file; RW-21 4: read straight from the INI)
pub const CAMERA_RECT_THICKNESS: f32 = 10.0;
/// The rectangle is drawn when the explored region is this many times the camera's view, on at least
/// one axis.
// re: config.ini [MiniMap] CameraRectThreshold
pub const CAMERA_RECT_THRESHOLD: f32 = 1.0;
/// Pixels left free round the explored region.
// re: config.ini [MiniMap] ExploredRegionGutter
pub const EXPLORED_REGION_GUTTER: f32 = 20.0;
/// Alpha of the fog and of the cleared target.
// re: config.ini [MiniMap] FogAlpha
pub const FOG_ALPHA: f32 = 1.0;

/// A rectangle of the world, the window of the minimap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewRect {
    pub min: Vec2,
    pub max: Vec2,
}

impl ViewRect {
    pub fn size(&self) -> Vec2 {
        self.max - self.min
    }

    pub fn center(&self) -> Vec2 {
        (self.min + self.max) * 0.5
    }

    pub fn intersects(&self, other: &ViewRect) -> bool {
        self.min.x <= other.max.x
            && other.min.x <= self.max.x
            && self.min.y <= other.max.y
            && other.min.y <= self.max.y
    }

    /// `SetupCamera`: `position * scale - offset` is in clip space (-1..1).
    pub fn offset_scale(&self) -> (Vec2, Vec2) {
        let scale = Vec2::splat(2.0) / self.size();
        (self.center() * scale, scale)
    }
}

/// What the camera is fitted to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraParams {
    pub world_min: Vec2,
    pub world_max: Vec2,
    pub wrap_x: bool,
    /// The viewport in pixels (the whole target; the original letterboxes it to the explored
    /// region's aspect, ours fits the region to the whole target by padding).
    pub viewport: (u32, u32),
    pub gutter: f32,
}

/// The views of the minimap: one, or two on a wrapping map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MinimapCamera {
    pub views: [ViewRect; 2],
    pub num_views: usize,
    /// Cuts of a hex edge: a quarter of the pixels one hex radius covers, at least 2. The original
    /// follows the edges of a cell with splines at this resolution.
    // TODO(RW-21): `PushTessellatedHexToMesh` and `GetSplineForEdge` are not in the dump; the meshes
    // here are plain hexes with straight edges, so the value is worked out and not used yet.
    pub edge_subdivisions: u32,
}

impl MinimapCamera {
    /// The view the meshes are made for.
    pub fn main(&self) -> &ViewRect {
        &self.views[0]
    }
}

/// `DeriveCameraParams`: `region_min` and `region_max` are the positions of the centres of the
/// hexes at the corners of the explored region.
pub fn derive_camera(region_min: Vec2, region_max: Vec2, p: &CameraParams) -> MinimapCamera {
    let half = Vec2::new(HEX_WIDTH * 0.5, CORNER_RADIUS);
    let rmin = region_min - half;
    let rmax = region_max + half;
    let rsize = rmax - rmin;
    let (vw, vh) = (p.viewport.0.max(1) as f32, p.viewport.1.max(1) as f32);
    let aspect = vw / vh;

    // Pads the short axis by the gutter in pixels, but not past the world.
    let padded = |region: f32, pixels: f32, world: f32| {
        let pad = p.gutter * (region / pixels);
        if world <= pad + pad + region {
            let room = (world - region) * 0.5;
            if room >= 0.0 { room } else { 0.0 }
        } else {
            pad
        }
    };
    let (left, right, mut bottom, mut top);
    if rsize.x / rsize.y <= aspect {
        let cx = (rmin.x + rmax.x) * 0.5;
        let pad = padded(rsize.y, vh, p.world_max.y - p.world_min.y);
        top = rmax.y + pad;
        bottom = rmin.y - pad;
        let hw = (top - bottom) * aspect * 0.5;
        left = cx - hw;
        right = cx + hw;
    } else {
        let cy = (rmax.y + rmin.y) * 0.5;
        let pad = padded(rsize.x, vw, p.world_max.x - p.world_min.x);
        right = rmax.x + pad;
        left = rmin.x - pad;
        let hh = ((right - left) / aspect) * 0.5;
        bottom = cy - hh;
        top = cy + hh;
    }
    if p.world_max.y <= top && bottom <= p.world_min.y {
        let shift = (top - bottom) * 0.5 - (p.world_max.y - p.world_min.y) * 0.5;
        bottom -= shift;
        top -= shift;
    }

    let ppu_y = vh / (top - bottom);
    let width = right - left;
    let ppu_x = vw / width;
    let ppu = ppu_y.max(ppu_x);
    let subdivisions = (ppu * CORNER_RADIUS * 0.25).ceil();
    let edge_subdivisions = if subdivisions > 2.0 {
        subdivisions as u32
    } else {
        2
    };

    let main = ViewRect {
        min: Vec2::new(left, bottom),
        max: Vec2::new(right, top),
    };
    let mut camera = MinimapCamera {
        views: [main; 2],
        num_views: 1,
        edge_subdivisions,
    };
    // A view that leaves the world on one side is drawn a second time one world width over.
    if p.wrap_x {
        let world_left = p.world_min.x;
        let shifted_left = if world_left <= left {
            if right <= p.world_max.x {
                return camera;
            }
            world_left - (p.world_max.x - left)
        } else {
            p.world_max.x - (world_left - left)
        };
        camera.views[1] = ViewRect {
            min: Vec2::new(shifted_left, bottom),
            max: Vec2::new(shifted_left + width, top),
        };
        camera.num_views = 2;
    }
    camera
}

/// The world position under the point `(u, v)` of the target, `u` and `v` in 0..=65535 from the
/// top left (the click of the panel). `None` for the corner of a target without a view.
pub fn target_to_world(view: &ViewRect, u: u16, v: u16, wrap_width: Option<f32>) -> Vec2 {
    let fu = f32::from(u) / 65535.0;
    let fv = f32::from(v) / 65535.0;
    let x = view.min.x + fu * (view.max.x - view.min.x);
    let y = view.max.y - fv * (view.max.y - view.min.y);
    Vec2::new(wrap_width.map_or(x, |w| x.rem_euclid(w)), y)
}

/// Where a world position lands in the target, `0..1` from the top left (the inverse of
/// [`target_to_world`] for a position inside the view).
pub fn world_to_target(view: &ViewRect, p: Vec2) -> Vec2 {
    let s = view.size();
    Vec2::new((p.x - view.min.x) / s.x, (view.max.y - p.y) / s.y)
}

/// The shifts in X, among `-W, 0, W`, at which the rectangle `[min, max]` shows in `view`: one
/// piece normally, two when it crosses the seam of a wrapping map and the view holds both sides.
pub fn camera_rect_shifts(
    rect_min: Vec2,
    rect_max: Vec2,
    view: &ViewRect,
    wrap_width: Option<f32>,
) -> Vec<f32> {
    let shifts: &[f32] = match wrap_width {
        Some(_) => &[-1.0, 0.0, 1.0],
        None => &[0.0],
    };
    let w = wrap_width.unwrap_or(0.0);
    shifts
        .iter()
        .map(|k| k * w)
        .filter(|dx| {
            view.intersects(&ViewRect {
                min: rect_min + Vec2::new(*dx, 0.0),
                max: rect_max + Vec2::new(*dx, 0.0),
            })
        })
        .collect()
}

/// Whether the rectangle is drawn at all: the explored view over the camera's, on the larger axis,
/// exceeds [`CAMERA_RECT_THRESHOLD`] (`RenderMiniMap`).
pub fn camera_rect_visible(view: &ViewRect, rect_min: Vec2, rect_max: Vec2) -> bool {
    let r = view.size() / (rect_max - rect_min).max(Vec2::splat(1e-3));
    r.x.max(r.y) > CAMERA_RECT_THRESHOLD
}

/// A vertex of the camera rectangle: `dcl_position`, and `dcl_color` whose `x` is the weight
/// (`VS_CameraRect`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RectVertex {
    pub position: [f32; 4],
    pub weight: [f32; 4],
}

/// The outline of the camera's view on the ground: a band `thickness` wide (world units) centred
/// on the quadrilateral, the weight 0 at both rims and 1 along the middle. The pixel shader makes
/// the weight the colour and its root the alpha.
// TODO(RW-21): `CreateCameraRectGeometry` is not in the dump; the band is a symmetric ring.
pub fn camera_rect_mesh(corners: &[Vec2; 4], thickness: f32) -> (Vec<RectVertex>, Vec<u32>) {
    let centre = corners.iter().copied().sum::<Vec2>() * 0.25;
    let ring = |offset: f32, weight: f32| -> Vec<RectVertex> {
        corners
            .iter()
            .map(|c| {
                let out = (*c - centre).normalize_or_zero();
                let p = *c + out * offset;
                RectVertex {
                    position: [p.x, p.y, 0.0, 1.0],
                    weight: [weight, 0.0, 0.0, 1.0],
                }
            })
            .collect()
    };
    let mut vertices = ring(thickness * 0.5, 0.0);
    vertices.extend(ring(0.0, 1.0));
    vertices.extend(ring(-thickness * 0.5, 0.0));
    let mut indices = Vec::new();
    for band in 0..2u32 {
        for i in 0..4u32 {
            let j = (i + 1) % 4;
            let (a, b) = (band * 4 + i, band * 4 + j);
            let (c, d) = (a + 4, b + 4);
            indices.extend_from_slice(&[a, b, d, a, d, c]);
        }
    }
    (vertices, indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(world_w: f32, wrap: bool) -> CameraParams {
        CameraParams {
            world_min: Vec2::new(-32.0, -37.0),
            world_max: Vec2::new(world_w, 1500.0),
            wrap_x: wrap,
            viewport: (320, 190),
            gutter: EXPLORED_REGION_GUTTER,
        }
    }

    #[test]
    fn the_view_has_the_aspect_of_the_target_and_holds_the_region_and_the_gutter() {
        let p = params(6400.0, false);
        let (lo, hi) = (Vec2::new(1000.0, 400.0), Vec2::new(2200.0, 900.0));
        let c = derive_camera(lo, hi, &p);
        let v = c.main();
        let aspect = v.size().x / v.size().y;
        assert!((aspect - 320.0 / 190.0).abs() < 1e-3, "{aspect}");
        // The region and half a hex round it are inside.
        assert!(v.min.x < lo.x - 32.0 && v.max.x > hi.x + 32.0);
        assert!(v.min.y < lo.y - CORNER_RADIUS && v.max.y > hi.y + CORNER_RADIUS);
        assert_eq!(c.num_views, 1);
    }

    #[test]
    fn a_wide_region_pads_the_height_and_a_tall_one_the_width() {
        let p = params(100_000.0, false);
        let wide = derive_camera(Vec2::new(0.0, 100.0), Vec2::new(4000.0, 300.0), &p);
        // Wide: fitted to the width, the height grows to the aspect.
        let w = wide.main();
        assert!((w.size().x - (4000.0 + 64.0 + 2.0 * pad_of(4000.0 + 64.0, 320.0))).abs() < 1.0);
        let tall = derive_camera(Vec2::new(0.0, 100.0), Vec2::new(300.0, 1400.0), &p);
        let t = tall.main();
        assert!(t.size().y > 1300.0);
        assert!((t.size().x / t.size().y - 320.0 / 190.0).abs() < 1e-3);
    }

    /// The gutter in world units at `region` units over `pixels`.
    fn pad_of(region: f32, pixels: f32) -> f32 {
        EXPLORED_REGION_GUTTER * region / pixels
    }

    #[test]
    fn the_second_view_appears_when_the_region_leaves_a_wrapping_world() {
        let w = 6400.0;
        let p = params(w, true);
        // Inside the world: one view.
        let inside = derive_camera(Vec2::new(2000.0, 300.0), Vec2::new(3000.0, 800.0), &p);
        assert_eq!(inside.num_views, 1);
        // Across the east seam.
        let across = derive_camera(Vec2::new(5800.0, 300.0), Vec2::new(6900.0, 800.0), &p);
        assert_eq!(across.num_views, 2);
        let v = across.views;
        assert!((v[1].size() - v[0].size()).length() < 1e-2);
        assert!(
            v[1].max.x <= p.world_max.x + 1e-3,
            "the copy lies on the other side"
        );
        // Not wrapping: never.
        let flat = derive_camera(
            Vec2::new(5800.0, 300.0),
            Vec2::new(6900.0, 800.0),
            &params(w, false),
        );
        assert_eq!(flat.num_views, 1);
    }

    #[test]
    fn edge_subdivisions_are_at_least_two() {
        let p = params(6400.0, false);
        let c = derive_camera(Vec2::new(0.0, 0.0), Vec2::new(6000.0, 1400.0), &p);
        assert_eq!(c.edge_subdivisions, 2);
        let close = derive_camera(Vec2::new(100.0, 100.0), Vec2::new(200.0, 160.0), &p);
        assert!(close.edge_subdivisions > 2);
    }

    #[test]
    fn the_camera_is_the_centre_and_two_over_the_size() {
        let v = ViewRect {
            min: Vec2::new(100.0, 200.0),
            max: Vec2::new(300.0, 300.0),
        };
        let (offset, scale) = v.offset_scale();
        assert_eq!(scale, Vec2::new(0.01, 0.02));
        // `position * scale - offset`: the corners are -1 and 1.
        assert!((v.min * scale - offset + Vec2::ONE).length() < 1e-5);
        assert!((v.max * scale - offset - Vec2::ONE).length() < 1e-5);
    }

    #[test]
    fn target_points_map_to_the_world_and_back() {
        let v = ViewRect {
            min: Vec2::new(0.0, 0.0),
            max: Vec2::new(1000.0, 500.0),
        };
        // The top left is the north west corner, the bottom right the south east one.
        assert_eq!(target_to_world(&v, 0, 0, None), Vec2::new(0.0, 500.0));
        let br = target_to_world(&v, 65535, 65535, None);
        assert!((br - Vec2::new(1000.0, 0.0)).length() < 1e-3);
        let mid = target_to_world(&v, 32768, 32768, None);
        assert!((mid - Vec2::new(500.0, 250.0)).length() < 0.1);
        let back = world_to_target(&v, mid);
        assert!((back - Vec2::splat(0.5)).length() < 1e-3);
        // A wrapping world brings X back into one period.
        let seam = target_to_world(&v, 65535, 0, Some(800.0));
        assert!((seam.x - 200.0).abs() < 1e-2);
    }

    #[test]
    fn the_rectangle_crossing_the_seam_shows_in_two_pieces() {
        let w = 6400.0;
        // The view holds the whole world and a gutter on each side.
        let view = ViewRect {
            min: Vec2::new(-200.0, 0.0),
            max: Vec2::new(w + 200.0, 1500.0),
        };
        let min = Vec2::new(w - 150.0, 600.0);
        let max = Vec2::new(w + 150.0, 900.0);
        let pieces = camera_rect_shifts(min, max, &view, Some(w));
        assert_eq!(pieces, vec![-w, 0.0], "two pieces");
        // In the middle of the world: one.
        let mid = camera_rect_shifts(
            Vec2::new(3000.0, 600.0),
            Vec2::new(3300.0, 900.0),
            &view,
            Some(w),
        );
        assert_eq!(mid, vec![0.0]);
        // Without wrapping the shifted copies do not exist.
        let flat = camera_rect_shifts(min, max, &view, None);
        assert_eq!(flat, vec![0.0]);
    }

    #[test]
    fn the_rectangle_is_drawn_only_while_the_view_is_larger_than_the_camera() {
        let view = ViewRect {
            min: Vec2::ZERO,
            max: Vec2::new(1000.0, 600.0),
        };
        assert!(camera_rect_visible(
            &view,
            Vec2::ZERO,
            Vec2::new(500.0, 300.0)
        ));
        assert!(!camera_rect_visible(
            &view,
            Vec2::ZERO,
            Vec2::new(1000.0, 600.0)
        ));
        assert!(!camera_rect_visible(
            &view,
            Vec2::ZERO,
            Vec2::new(2000.0, 2000.0)
        ));
    }

    #[test]
    fn the_rectangle_band_has_weight_one_in_the_middle_and_zero_at_the_rims() {
        let corners = [
            Vec2::new(0.0, 0.0),
            Vec2::new(100.0, 0.0),
            Vec2::new(100.0, 50.0),
            Vec2::new(0.0, 50.0),
        ];
        let (v, i) = camera_rect_mesh(&corners, 6.0);
        assert_eq!(v.len(), 12);
        assert_eq!(i.len(), 2 * 4 * 6);
        assert!(i.iter().all(|&k| (k as usize) < v.len()));
        let weights: Vec<f32> = v.iter().map(|x| x.weight[0]).collect();
        assert_eq!(&weights[..4], &[0.0; 4]);
        assert_eq!(&weights[4..8], &[1.0; 4]);
        // The middle ring is the quadrilateral.
        assert_eq!(v[4].position[..2], [0.0, 0.0]);
        // The outer ring lies outside it.
        assert!(v[0].position[0] < 0.0 && v[0].position[1] < 0.0);
    }
}
