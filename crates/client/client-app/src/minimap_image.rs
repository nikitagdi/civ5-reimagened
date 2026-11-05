//! The stand-in minimap (lobby-ui 4.8, LU-D10): a picture built from the shown view, one pixel per
//! plot, for the panel's `Image` source. The renderer's own minimap texture replaces it when its
//! projection is real (`Renderer::minimap_to_world`).
//!
//! SIMPLIFIED(SIMP-060): no meshes of hexes, no layers; the colour is the kind of plot, tinted by
//! the owner, darkened where the plot is only remembered (fog), black where it is unknown.

use std::sync::Arc;

use glam::Vec2;
use interim_ui::screens::minimap::MinimapImage;
use protocol::{MapView, PlayerView, PlotFlags, PlotType, PlotView, WorldView};
use render::GameCamera;
use render::hexgrid;

/// Builds the picture and keeps its version: it rises with every picture that differs.
#[derive(Debug, Default)]
pub struct MinimapBuilder {
    version: u64,
    /// What the picture was built for: the revision of the view the caller named.
    built_for: Option<u64>,
    image: Option<MinimapImage>,
}

impl MinimapBuilder {
    /// The picture of `world`; it is built again when `revision` (the shell's counter of changes of
    /// the view) or the map size differs from the last call.
    pub fn update(&mut self, world: &WorldView, revision: u64) -> Option<&MinimapImage> {
        let map = &world.map;
        if map.width == 0 || map.plots.len() != usize::from(map.width) * usize::from(map.height) {
            self.image = None;
            self.built_for = None;
            return None;
        }
        let same_size = self
            .image
            .as_ref()
            .is_some_and(|i| i.size == [usize::from(map.width), usize::from(map.height)]);
        if self.built_for != Some(revision) || !same_size {
            self.version += 1;
            self.built_for = Some(revision);
            self.image = Some(MinimapImage {
                size: [usize::from(map.width), usize::from(map.height)],
                rgba: Arc::new(pixels(map, &world.players)),
                version: self.version,
            });
        }
        self.image.as_ref()
    }
}

/// RGBA, row 0 the northern edge (plot rows count from the south).
pub fn pixels(map: &MapView, players: &[PlayerView]) -> Vec<u8> {
    let (w, h) = (usize::from(map.width), usize::from(map.height));
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let at = ((h - 1 - y) * w + x) * 4;
            out[at..at + 4].copy_from_slice(&plot_color(&map.plots[y * w + x], players));
        }
    }
    out
}

fn plot_color(p: &PlotView, players: &[PlayerView]) -> [u8; 4] {
    if !p.flags.contains(PlotFlags::REVEALED) {
        return [0, 0, 0, 255];
    }
    let base: [u8; 3] = match p.plot_type {
        PlotType::Water => [30, 70, 140],
        PlotType::Land => [90, 140, 60],
        PlotType::Hill => [130, 105, 60],
        PlotType::Mountain => [135, 135, 135],
    };
    let mut c = base.map(f32::from);
    if let Some(owner) = players.iter().find(|pl| pl.id == p.owner && p.owner >= 0) {
        let [r, g, b, _] = owner.color_primary.to_be_bytes();
        for (v, t) in c.iter_mut().zip([r, g, b]) {
            *v = *v * 0.55 + f32::from(t) * 0.45;
        }
    }
    if !p.flags.contains(PlotFlags::VISIBLE) {
        c = c.map(|v| v * 0.55);
    }
    [c[0] as u8, c[1] as u8, c[2] as u8, 255]
}

/// The plot at the position `(u, v)` (0..=65535 from the top left corner of the picture) of a map
/// of `width` x `height` plots: linear in both directions (SIMP-060).
pub fn cell_at(u: u16, v: u16, width: u16, height: u16) -> (i32, i32) {
    let f = |a: u16, n: u16| (f32::from(a) / 65535.0 * f32::from(n.max(1) - 1)).round() as i32;
    let x = f(u, width);
    // The picture's top is the northern edge.
    let y = i32::from(height.max(1)) - 1 - f(v, height);
    (x, y)
}

/// The camera's view as a frame on the picture: left, top, right, bottom in 0..=65535. `None`
/// while the corners of the screen do not reach the ground. `aspect` is the screen's width / height.
pub fn camera_frame(camera: &GameCamera, aspect: f32, width: u16, height: u16) -> Option<[u16; 4]> {
    if width == 0 || height == 0 {
        return None;
    }
    let world_w = hexgrid::cell_center(i32::from(width), 0).x;
    let world_h = -hexgrid::cell_center(0, i32::from(height)).y;
    let mut min = Vec2::splat(f32::MAX);
    let mut max = Vec2::splat(f32::MIN);
    // The top of the screen may look past the horizon: stay a little below it.
    for ndc in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 0.8), (1.0, 0.8)] {
        let p = camera.ground_point(Vec2::new(ndc.0, ndc.1), aspect)?;
        let p = Vec2::new(p.x, p.z);
        min = min.min(p);
        max = max.max(p);
    }
    let fx = |x: f32| (x / world_w).clamp(0.0, 1.0);
    // World Z grows to the south; the picture's v grows to the south as well.
    let fy = |z: f32| (1.0 + z / world_h).clamp(0.0, 1.0);
    let q = |f: f32| (f * 65535.0).round() as u16;
    Some([q(fx(min.x)), q(fy(min.y)), q(fx(max.x)), q(fy(max.y))])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(w: u16, h: u16) -> MapView {
        MapView {
            width: w,
            height: h,
            plots: vec![PlotView::default(); usize::from(w) * usize::from(h)],
            ..MapView::default()
        }
    }

    fn seen(plot_type: PlotType, visible: bool) -> PlotView {
        let mut flags = PlotFlags::REVEALED;
        if visible {
            flags |= PlotFlags::VISIBLE;
        }
        PlotView {
            flags: PlotFlags(flags),
            plot_type,
            owner: -1,
            ..PlotView::default()
        }
    }

    fn world(map: MapView) -> WorldView {
        WorldView {
            turn: 1,
            year: -4000,
            human: 0,
            human_team: 0,
            status: protocol::TurnStatus::HumanTurn,
            blocker: None,
            map,
            units: vec![],
            cities: vec![],
            players: vec![],
            top_bar: protocol::TopBar::default(),
            notifications: vec![],
            peace_offers: vec![],
            trade_routes: vec![],
        }
    }

    #[test]
    fn the_picture_has_one_pixel_per_plot() {
        let m = map(5, 3);
        assert_eq!(pixels(&m, &[]).len(), 5 * 3 * 4);
    }

    #[test]
    fn colours_follow_the_kind_of_plot_and_the_fog() {
        let mut m = map(3, 1);
        m.plots[0] = seen(PlotType::Water, true);
        m.plots[1] = seen(PlotType::Land, true);
        m.plots[2] = seen(PlotType::Land, false);
        let px = pixels(&m, &[]);
        let at = |i: usize| &px[i * 4..i * 4 + 4];
        assert_eq!(at(0), [30, 70, 140, 255]);
        assert_eq!(at(1), [90, 140, 60, 255]);
        assert!(at(2)[1] < at(1)[1], "fog darkens");
        let unknown = pixels(&map(1, 1), &[]);
        assert_eq!(unknown, [0, 0, 0, 255]);
    }

    #[test]
    fn an_owner_tints_the_plot() {
        let mut m = map(1, 1);
        m.plots[0] = PlotView {
            owner: 2,
            ..seen(PlotType::Land, true)
        };
        let owner = PlayerView {
            id: 2,
            color_primary: 0xFF00_00FF,
            ..PlayerView::default()
        };
        let px = pixels(&m, &[owner]);
        assert!(px[0] > 130 && px[1] < 90, "reddish: {px:?}");
    }

    #[test]
    fn the_northern_row_is_on_top() {
        let mut m = map(1, 2);
        m.plots[1] = seen(PlotType::Water, true); // y = 1, north
        let px = pixels(&m, &[]);
        assert_eq!(&px[0..4], [30, 70, 140, 255]);
        assert_eq!(&px[4..8], [0, 0, 0, 255]);
    }

    #[test]
    fn the_builder_rebuilds_on_a_new_revision_only() {
        let mut world = world(map(4, 3));
        let mut b = MinimapBuilder::default();
        let v1 = b.update(&world, 1).expect("image").version;
        assert_eq!(b.update(&world, 1).expect("image").version, v1);
        let v2 = b.update(&world, 2).expect("image").version;
        assert!(v2 > v1);
        world.map = MapView::default();
        assert!(b.update(&world, 3).is_none());
    }

    #[test]
    fn a_click_gives_the_plot_back() {
        assert_eq!(cell_at(0, 0, 10, 6), (0, 5), "top left is the north west");
        assert_eq!(cell_at(65535, 65535, 10, 6), (9, 0));
        assert_eq!(cell_at(32768, 32768, 11, 7), (5, 3));
        assert_eq!(cell_at(1000, 1000, 0, 0), (0, 0));
        // The picture's pixel of a plot gives that plot back at its centre.
        let (w, h) = (20u16, 12u16);
        for (x, y) in [(0u16, 0u16), (7, 3), (19, 11)] {
            let u = (f32::from(x) / f32::from(w - 1) * 65535.0).round() as u16;
            let v = ((f32::from(h - 1 - y) / f32::from(h - 1)) * 65535.0).round() as u16;
            assert_eq!(cell_at(u, v, w, h), (i32::from(x), i32::from(y)));
        }
    }

    #[test]
    fn the_camera_frame_is_a_box_around_the_focus() {
        let mut cam = GameCamera::new(hexgrid::cell_center(10, 6));
        cam.set_level(0.0);
        let f = camera_frame(&cam, 16.0 / 9.0, 20, 12).expect("frame");
        assert!(f[0] < f[2] && f[1] < f[3], "{f:?}");
        let centre = |a: u16, b: u16| (u32::from(a) + u32::from(b)) / 2;
        assert!(centre(f[0], f[2]).abs_diff(32768) < 12_000, "{f:?}");
        assert_eq!(camera_frame(&cam, 1.0, 0, 0), None);
    }
}
