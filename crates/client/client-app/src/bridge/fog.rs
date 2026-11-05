//! The `fog` ribbon of the bridge (world-overlays WOV0): `MapView` -> the render crate's
//! [`FogInput`] for `Renderer::set_fog` (the `FluidFow` subsystem), as the [`FogFeed`] (R4.10).
//!
//! The feed reads the section `PLOTS` and hands the whole fog over after every change of the
//! plots; the renderer compares it with the last one and does nothing when they are equal.

use client_core::ViewChange;
use protocol::{Catalog, MapView, PlotFlags, PlotView, SectionMask, WorldView};
use render::Renderer;
use render::world::fow::FogInput;

use super::{FeedCx, ViewFeed, mask, section};

use render::terrain::Fog;

/// The fog state of one plot: unrevealed, revealed but not visible, visible.
pub fn plot_fog(p: &PlotView) -> Fog {
    if !p.flags.contains(PlotFlags::REVEALED) {
        Fog::Unrevealed
    } else if p.flags.contains(PlotFlags::VISIBLE) {
        Fog::Visible
    } else {
        Fog::Hidden
    }
}

/// The fog of the whole map. `None` if the plot count does not match the size.
pub fn fog_input(map: &MapView) -> Option<FogInput> {
    FogInput::new(
        u32::from(map.width),
        u32::from(map.height),
        map.wrap_x,
        map.plots.iter().map(plot_fog).collect(),
    )
}

/// The feed of the fog: the fog of the view's map, to `Renderer::set_fog`.
#[derive(Debug, Default)]
pub struct FogFeed {
    pending: Option<FogInput>,
}

impl FogFeed {
    /// The fog that waits for the next `flush` (tests).
    pub fn pending(&self) -> Option<&FogInput> {
        self.pending.as_ref()
    }
}

impl ViewFeed for FogFeed {
    fn sections(&self) -> SectionMask {
        mask(&[section::PLOTS])
    }

    fn on_catalog(&mut self, _catalog: &Catalog) {}

    fn apply(&mut self, view: &WorldView, _change: &ViewChange, cx: &mut FeedCx<'_>) {
        self.pending = fog_input(&view.map);
        if self.pending.is_some() {
            // The camera pans over the revealed plots grown by a hex (`CvCamera::OnFOWChanged`).
            let shape = render::hexgrid::GridShape {
                width: i32::from(view.map.width),
                height: i32::from(view.map.height),
                wrap_x: view.map.wrap_x,
            };
            let width = usize::from(view.map.width).max(1);
            let revealed = view
                .map
                .plots
                .iter()
                .enumerate()
                .filter(|(_, p)| plot_fog(p) != Fog::Unrevealed)
                .map(|(i, _)| render::hexgrid::cell_center((i % width) as i32, (i / width) as i32));
            cx.camera
                .set_explored(render::MapBounds::from_grid(&shape), revealed);
        }
    }

    fn flush(&mut self, renderer: &mut Renderer) {
        if let Some(fog) = self.pending.take() {
            renderer.set_fog(fog);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::tests::fixture_world;

    #[test]
    fn the_fog_follows_the_flags_of_the_plots() {
        let (world, _) = fixture_world();
        let fog = fog_input(&world.map).expect("the fixture map is whole");
        assert_eq!(fog.cells.len(), world.map.plots.len());
        for (p, c) in world.map.plots.iter().zip(&fog.cells) {
            let want = match (
                p.flags.contains(PlotFlags::REVEALED),
                p.flags.contains(PlotFlags::VISIBLE),
            ) {
                (false, _) => Fog::Unrevealed,
                (true, true) => Fog::Visible,
                (true, false) => Fog::Hidden,
            };
            assert_eq!(*c, want);
        }
        assert!(
            fog.cells.contains(&Fog::Visible),
            "the fixture shows something"
        );
    }

    #[test]
    fn the_camera_pans_over_the_revealed_plots_grown_by_a_hex() {
        let (world, _) = fixture_world();
        let mut camera = render::GameCamera::default();
        let mut feed = FogFeed::default();
        feed.apply(
            &world,
            &ViewChange::all(),
            &mut FeedCx {
                basis: None,
                camera: &mut camera,
            },
        );
        let (lo, hi) = camera.bounds().unwrap().explored.expect("revealed plots");
        let w = usize::from(world.map.width);
        for (i, p) in world.map.plots.iter().enumerate() {
            let c = render::hexgrid::cell_center((i % w) as i32, (i / w) as i32);
            if plot_fog(p) != Fog::Unrevealed {
                assert!(
                    c.cmpge(lo).all() && c.cmple(hi).all(),
                    "{c} outside {lo}..{hi}"
                );
            }
        }
    }

    #[test]
    fn a_map_of_the_wrong_size_gives_no_fog() {
        let (mut world, _) = fixture_world();
        world.map.plots.pop();
        assert!(fog_input(&world.map).is_none());
    }

    #[test]
    fn the_fog_of_the_snapshot_and_of_the_function_agree() {
        // The terrain feed and `Renderer::set_terrain` both give the renderer a fog; it is the
        // same one, so that the second `set_fog` does nothing.
        use crate::bridge::{TerrainTable, terrain_snapshot};
        let (world, catalog) = fixture_world();
        let snapshot = terrain_snapshot(&world.map, &TerrainTable::from_catalog(&catalog))
            .expect("the fixture map is whole");
        assert_eq!(
            fog_input(&world.map),
            Some(FogInput::from_snapshot(&snapshot))
        );
    }
}
