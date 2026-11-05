//! `River` subsystem (world-water WW-4): the stage `Rivers` of the terrain
//! build makes the simulation, this draws the water of the rivers (`water/river/draw.rs`) in
//! `Main.Terrain` after the ocean, over the ribbon of `RiverVisSystem::TessellateMesh`
//! (`water/river/mesh.rs`, WW-5).
//!
//! The draw shares the constants, the LEAN maps and the maps of the surface with `Water`: the
//! renderer calls [`River::feed`] with the water once per frame, before the pass. Until the water
//! is ready (or there is no simulation) nothing is drawn and [`River::drawn_natively`] is false, so
//! B1's strips stay.

use std::cell::Cell;
use std::sync::Arc;

use crate::frame::{
    DrawContext, InitContext, MainSlot, MaskContext, PassId, PassSlot, PrepareContext,
    RenderSystem, SystemId,
};
use crate::settings::{KeySet, RenderSettings};
use crate::world::terrain::build::BuildStage;
use crate::world::terrain::sampler::TerrainFrame;
use crate::world::water::Water;
use crate::world::water::river::RiverSim;
use crate::world::water::river::draw::{RiverFeed, RiverGpu};
use crate::world::water::river::mesh;
use crate::world::water::river::stage::RiverShared;

/// The setting the mesh lifts its water by (`g_uTerrainTessLevel`).
const TESS_LEVEL: &str = "TerrainTessLevel";
/// The time a frame gives to making the ribbon.
const BUILD_BUDGET_MS: f64 = 2.5;

/// The rivers: the stage `Rivers` of the terrain build makes the simulation, this draws it.
#[derive(Default)]
pub struct River {
    shared: RiverShared,
    gpu: Option<RiverGpu>,
    /// A simulation set by hand (tests, tools) in place of the stage's.
    injected: Option<Arc<RiverSim>>,
    feed: Option<RiverFeed>,
    /// The simulation and the frame the buffers were made from.
    meshed: Option<(Arc<RiverSim>, TerrainFrame)>,
    /// The ribbon being made for a simulation and a frame.
    building: Option<mesh::RiverBuild>,
    /// `TerrainTessLevel`; 0 lifts the water (`TessellateMesh`). The preset High until the
    /// settings come.
    tess_level: Option<u32>,
    /// This frame's draw: set in `prepare` from what can be drawn, cleared in `record` if the
    /// draw gave nothing.
    drawn: Cell<bool>,
}

impl River {
    /// The stage `Rivers` for the build of the terrain; its result lands in [`Self::shared`].
    pub fn stage(&self) -> Box<dyn BuildStage> {
        self.shared.stage()
    }

    /// The result of the stage.
    pub fn shared(&self) -> &RiverShared {
        &self.shared
    }

    /// Takes the device (once, after it exists).
    pub fn bind_device(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, samples: u32) {
        self.gpu = Some(RiverGpu::new(device, queue, samples));
        self.meshed = None;
    }

    /// A simulation in place of the one of the stage (tests and tools); `None` returns to the stage.
    pub fn set_sim(&mut self, sim: Option<Arc<RiverSim>>) {
        self.injected = sim;
    }

    /// What the surface gives the rivers in this frame. The renderer calls it after the water is fed
    /// and before `prepare`; without a ready water the rivers draw nothing.
    pub fn feed(&mut self, water: &Water) {
        self.feed = water.river_feed();
    }

    /// The rivers are drawn by this subsystem in this frame (at least one river): B1's strips can
    /// go. Known from `prepare` on, false again if the draw gave nothing.
    pub fn drawn_natively(&self) -> bool {
        self.drawn.get()
    }

    fn sim(&self) -> Option<Arc<RiverSim>> {
        self.injected.clone().or_else(|| self.shared.sim())
    }
}

impl RenderSystem for River {
    fn id(&self) -> SystemId {
        SystemId::River
    }

    fn passes(&self) -> &'static [PassSlot] {
        &[
            PassSlot::Pass(PassId::TerrainBake),
            PassSlot::Main(MainSlot::Terrain),
        ]
    }

    fn init(&mut self, _cx: &mut InitContext<'_>) {}

    fn on_settings(
        &mut self,
        settings: &RenderSettings,
        changed: &KeySet,
        _cx: &mut InitContext<'_>,
    ) {
        let level = settings.get(TESS_LEVEL).and_then(|v| u32::try_from(v).ok());
        if changed.contains(TESS_LEVEL) && level != self.tess_level {
            self.tess_level = level;
            // The ribbon is made again with the new lift.
            self.meshed = None;
        }
    }

    fn prepare(&mut self, _cx: &mut PrepareContext<'_>) {
        self.drawn.set(false);
        let sim = self.sim();
        let (Some(gpu), Some(feed), Some(sim)) = (&mut self.gpu, &self.feed, sim) else {
            return;
        };
        // The ribbon is made again when the simulation (the pointer) or the frame changed.
        let current = matches!(
            &self.meshed,
            Some((s, f)) if Arc::ptr_eq(s, &sim) && *f == feed.terrain
        );
        if !current {
            // The ribbon is made a few milliseconds a frame; the old one stays until the new is whole.
            let level = self.tess_level.unwrap_or(2);
            let fresh = self.building.as_ref().is_some_and(|b| {
                Arc::ptr_eq(b.sim(), &sim) && *b.frame() == feed.terrain && b.tess_level() == level
            });
            if !fresh {
                self.building = Some(mesh::RiverBuild::new(sim.clone(), &feed.terrain, level));
            }
            let s = crate::perf::slow::scope("river.mesh_build");
            let built = self.building.as_mut().and_then(|b| b.step(BUILD_BUDGET_MS));
            drop(s);
            if let Some(built) = built {
                self.building = None;
                let _s = crate::perf::slow::scope("river.set_mesh");
                gpu.set_mesh(&built);
                self.meshed = Some((sim, feed.terrain));
            }
        }
        if !gpu.has_mesh() {
            return;
        }
        gpu.bind(&feed.textures);
        gpu.prepare(feed);
        gpu.ensure_pipeline(feed.quality);
        self.drawn.set(true);
    }

    fn record(&self, slot: PassSlot, cx: &mut DrawContext<'_, '_>) {
        if slot != PassSlot::Main(MainSlot::Terrain) || !self.drawn.get() {
            return;
        }
        let (Some(gpu), Some(feed)) = (&self.gpu, &self.feed) else {
            return;
        };
        let n = gpu.draw(cx.pass, feed.quality, cx.copy_shift);
        if n > 0 {
            cx.stats.record_draw(n);
        } else {
            self.drawn.set(false);
        }
    }

    fn record_mask(&self, slot: PassSlot, cx: &mut MaskContext<'_, '_>) {
        if slot != PassSlot::Main(MainSlot::Terrain) || !self.drawn.get() {
            return;
        }
        if let Some(gpu) = &self.gpu {
            gpu.draw_mask(cx.pass, cx.copy_shift);
        }
    }

    fn counters(&self) -> Vec<(&'static str, u64)> {
        vec![("river.drawn", u64::from(self.drawn.get()))]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_river_without_water_or_device_draws_nothing() {
        let r = River::default();
        assert!(!r.drawn_natively());
        assert!(r.sim().is_none());
    }
}
