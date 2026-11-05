//! The static list of world subsystems: one field per subsystem of
//! table 4.2, plus the shared services. The field order is the order inside a slot (`graph::plan`).
//!
//! [`wire_build_stages`] puts the stages of the subsystems that write the terrain build
//! (today the five of the coast) into [`Terrain`].

use glam::Mat4;

use super::graph::PassId;
use super::system::{
    EncoderContext, InitContext, PrepareContext, RenderSystem, SystemId, WorldServices,
};
use super::targets::TargetPool;
use crate::renderer::FrameStats;
use crate::settings::{KeySet, RenderSettings, readers_of};
use crate::world::{
    arrows::Arrows, city::City, decal::Decal, distance_fog::DistanceFog, forest::Forest,
    fow::FluidFow, fow::FowClassic, ice::Ice, landmark::Landmark, minimap::Minimap,
    overlay::Overlay, particle::Particle, path::MovementPath, river::River, shadows::Shadows,
    terrain::Terrain, trade::Trade, unit::Unit, water::Water, waves::Waves,
    yield_icons::YieldIcons,
};

/// Every subsystem of table 4.2, and the shared services (`services`, no slots, no switches).
#[derive(Default)]
pub struct WorldSystems {
    pub services: WorldServices,
    pub terrain: Terrain,
    pub decal: Decal,
    pub water: Water,
    pub river: River,
    pub ice: Ice,
    pub waves: Waves,
    pub city: City,
    pub arrows: Arrows,
    pub trade: Trade,
    pub forest: Forest,
    pub landmark: Landmark,
    pub unit: Unit,
    pub particle: Particle,
    pub fow_classic: FowClassic,
    pub fluid_fow: FluidFow,
    pub overlay: Overlay,
    pub movement_path: MovementPath,
    pub minimap: Minimap,
    pub yield_icons: YieldIcons,
    pub distance_fog: DistanceFog,
    pub shadows: Shadows,
}

impl WorldSystems {
    /// The services and every subsystem mutably (`init` and `prepare` take `&mut self`); the
    /// services are separate so a `PrepareContext` can borrow them beside a subsystem.
    pub fn split_mut(&mut self) -> (&WorldServices, [&mut dyn RenderSystem; 21]) {
        let Self {
            services,
            terrain,
            decal,
            water,
            river,
            ice,
            waves,
            city,
            arrows,
            trade,
            forest,
            landmark,
            unit,
            particle,
            fow_classic,
            fluid_fow,
            overlay,
            movement_path,
            minimap,
            yield_icons,
            distance_fog,
            shadows,
        } = self;
        (
            services,
            [
                terrain,
                decal,
                water,
                river,
                ice,
                waves,
                city,
                arrows,
                trade,
                forest,
                landmark,
                unit,
                particle,
                fow_classic,
                fluid_fow,
                overlay,
                movement_path,
                minimap,
                yield_icons,
                distance_fog,
                shadows,
            ],
        )
    }

    /// Every subsystem mutably, in field order.
    pub fn list_mut(&mut self) -> [&mut dyn RenderSystem; 21] {
        self.split_mut().1
    }

    /// The subsystem with `id`.
    pub fn get(&self, id: SystemId) -> &dyn RenderSystem {
        self.list()
            .into_iter()
            .find(|s| s.id() == id)
            .expect("every SystemId has a field in WorldSystems")
    }

    /// Hands B1's pipelines to the subsystems that still draw with them (once, after the device
    /// exists; `world-overlays` WOV0).
    pub(crate) fn bind_b1(&mut self, env: &crate::world::b1_env::B1Env) {
        self.water.bind(env);
        self.river.bind_device(&env.device, &env.queue, env.samples);
        self.forest
            .bind_device(&env.device, &env.queue, env.samples);
        self.particle
            .bind_device(&env.device, &env.queue, env.samples);
        self.fluid_fow.bind(env);
        self.fow_classic.bind(env);
        self.overlay.bind(env);
        self.waves.bind_device(&env.device, &env.queue, env.samples);
        self.ice.bind_device(&env.device, &env.queue, env.samples);
        self.arrows
            .bind_device(&env.device, &env.queue, env.samples);
        self.trade.bind_device(&env.device, &env.queue);
    }

    /// The camera of the frame for the two fogs.
    pub(crate) fn set_fow_frame(&mut self, view: crate::world::fow::mask::FowView) {
        self.fluid_fow.set_frame(view);
        self.fow_classic.set_frame(view);
        self.minimap.set_frame(view);
        self.overlay.set_frame(view);
    }

    /// `FOWLevel` for the two fogs.
    pub(crate) fn set_fow_level(&mut self, level: i64) {
        self.fluid_fow.set_level(level);
        self.fow_classic.set_level(level);
    }

    /// The counters of the subsystems that are on, for `?stats=1`.
    pub fn counters(&self, enabled: &dyn Fn(SystemId) -> bool) -> Vec<(&'static str, u64)> {
        self.list()
            .into_iter()
            .filter(|s| enabled(s.id()))
            .flat_map(|s| s.counters())
            .collect()
    }

    /// `RenderSystem::init` of every subsystem (once, after the device exists).
    pub fn init(&mut self, device: &wgpu::Device, targets: &mut TargetPool) {
        for system in self.list_mut() {
            system.init(&mut InitContext { device, targets });
        }
    }

    /// `RenderSystem::on_settings` of the subsystems that read a key of `changed`; returns who was
    /// called.
    pub fn on_settings(
        &mut self,
        device: &wgpu::Device,
        targets: &mut TargetPool,
        settings: &RenderSettings,
        changed: &KeySet,
    ) -> Vec<SystemId> {
        notify_readers(self.list_mut(), device, targets, settings, changed)
    }

    /// `RenderSystem::prepare` of the subsystem `id`.
    pub fn prepare_system(
        &mut self,
        id: SystemId,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world_frame: &mut Option<Mat4>,
    ) {
        if id == SystemId::Overlay {
            // The pass of the cells runs on the terrain's own cells when it has some on the screen.
            let drawn = self
                .terrain
                .cell_geometry()
                .is_some_and(|g| g.copies().iter().any(|&shift| g.cells(shift) > 0));
            self.overlay.set_terrain_cells(drawn);
        }
        let (services, systems) = self.split_mut();
        if let Some(system) = systems.into_iter().find(|s| s.id() == id) {
            system.prepare(&mut PrepareContext::new(
                device,
                queue,
                services,
                world_frame,
            ));
        }
    }

    /// `RenderSystem::record_offscreen` of the subsystem `id` for the pass `pass`. True if the
    /// subsystem used the timestamp writes it was offered (its pass is timed).
    #[allow(clippy::too_many_arguments)]
    pub fn offscreen_system(
        &mut self,
        id: SystemId,
        pass: PassId,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        targets: &TargetPool,
        stats: &mut FrameStats,
        timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) -> bool {
        let had = timestamps.is_some();
        if id == SystemId::Overlay && pass == PassId::ScreenOverlay {
            // The cells of the ground come from the terrain, which the overlay cannot reach alone.
            let Self {
                services,
                terrain,
                overlay,
                trade,
                ..
            } = self;
            let geometry = terrain.cell_geometry();
            let mut cx = EncoderContext::new(device, queue, encoder, targets, stats, services)
                .with_timestamps(timestamps);
            overlay.encode_screen_overlay(&mut cx, geometry.as_ref(), Some(&*trade));
            return had && cx.timestamps().is_none();
        }
        let (services, systems) = self.split_mut();
        let Some(system) = systems.into_iter().find(|s| s.id() == id) else {
            return false;
        };
        let mut cx = EncoderContext::new(device, queue, encoder, targets, stats, services)
            .with_timestamps(timestamps);
        system.record_offscreen(pass, &mut cx);
        had && cx.timestamps().is_none()
    }

    /// Every subsystem in field order, the list `graph::plan` and `graph::settled` take.
    pub fn list(&self) -> [&dyn RenderSystem; 21] {
        [
            &self.terrain,
            &self.decal,
            &self.water,
            &self.river,
            &self.ice,
            &self.waves,
            &self.city,
            &self.arrows,
            &self.trade,
            &self.forest,
            &self.landmark,
            &self.unit,
            &self.particle,
            &self.fow_classic,
            &self.fluid_fow,
            &self.overlay,
            &self.movement_path,
            &self.minimap,
            &self.yield_icons,
            &self.distance_fog,
            &self.shadows,
        ]
    }
}

/// Installs the stages the other subsystems add to the build of the terrain. The coast of
/// `world-water` (`CoastSplines`, `CoastalRoads`, `CoastStencil`, `BlendSplines`, `CoastBlendMeshes`)
/// needs the styles of `tb:world-b2`: without them (the assets of B1) nothing is installed and the
/// build is what `TerrainBuild::new` makes. The stages take their place in the order of the table
/// whatever the order they are installed in, so the stream of `rand` runs as the original's:
/// the noise of the lines first, then the matcher of `Pieces`, then the two draws of the meshes.
/// Call it before the basis is set or before `Terrain::set_defs`, which builds a map again.
pub fn wire_build_stages(terrain: &mut Terrain, settings: &asset_format::WorldSettings) {
    if settings.coast.styles.is_empty() {
        return;
    }
    let coast = std::sync::Arc::new(settings.coast.clone());
    for stage in crate::world::water::coast::stages(coast) {
        terrain.install_stage(stage);
    }
}

/// Calls `on_settings` on each system that reads a key of `changed`, and returns their ids.
fn notify_readers<'a>(
    systems: impl IntoIterator<Item = &'a mut dyn RenderSystem>,
    device: &wgpu::Device,
    targets: &mut TargetPool,
    settings: &RenderSettings,
    changed: &KeySet,
) -> Vec<SystemId> {
    let mut called = Vec::new();
    for system in systems {
        let id = system.id();
        if readers_of(id).intersects(changed) {
            system.on_settings(settings, changed, &mut InitContext { device, targets });
            called.push(id);
        }
    }
    called
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{MainSlot, PassId, PassSlot, SystemId};
    use crate::gpu_test::gpu;
    use std::collections::HashSet;

    /// Counts the `on_settings` calls it gets, under the id it is given.
    struct Spy {
        id: SystemId,
        calls: u32,
    }

    impl RenderSystem for Spy {
        fn id(&self) -> SystemId {
            self.id
        }
        fn passes(&self) -> &'static [PassSlot] {
            &[]
        }
        fn init(&mut self, _cx: &mut InitContext<'_>) {}
        fn on_settings(&mut self, _s: &RenderSettings, _c: &KeySet, _cx: &mut InitContext<'_>) {
            self.calls += 1;
        }
        fn prepare(&mut self, _cx: &mut PrepareContext<'_>) {}
        fn record(&self, _slot: PassSlot, _cx: &mut crate::frame::DrawContext<'_, '_>) {}
    }

    /// The coast joins the build of the terrain in the places of the table, around `Pieces`; the
    /// assets of B1 have no styles, and the build stays what `TerrainBuild::new` makes.
    #[test]
    fn wire_build_stages_adds_the_coast_where_the_settings_have_styles() {
        use crate::world::terrain::build::StageId;
        let mut terrain = Terrain::default();
        wire_build_stages(&mut terrain, &asset_format::WorldSettings::default());
        assert_eq!(terrain.build().stage_ids(), [StageId::Pieces]);

        let settings = asset_format::WorldSettings {
            coast: crate::world::water::coast::styles::synthetic_settings(false),
            ..Default::default()
        };
        wire_build_stages(&mut terrain, &settings);
        assert_eq!(
            terrain.build().stage_ids(),
            [
                StageId::CoastSplines,
                StageId::CoastalRoads,
                StageId::CoastStencil,
                StageId::BlendSplines,
                StageId::Pieces,
                StageId::CoastBlendMeshes
            ]
        );
        // Again with the same settings: the stages are replaced, not doubled.
        wire_build_stages(&mut terrain, &settings);
        assert_eq!(terrain.build().stage_ids().len(), 6);
    }

    #[test]
    fn system_id_all_is_the_field_list() {
        let ws = WorldSystems::default();
        let ids: Vec<SystemId> = ws.list().iter().map(|s| s.id()).collect();
        assert_eq!(ids, SystemId::ALL);
    }

    /// `on_settings` goes only to the subsystems that read a changed key.
    #[test]
    fn on_settings_reaches_only_the_readers_of_changed_keys() {
        let Some(gpu) = gpu(
            "on_settings_reaches_only_the_readers_of_changed_keys",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let mut targets = TargetPool::new(false, (64, 64));
        let high = RenderSettings::preset(crate::settings::Preset::High);
        let mut now = high.clone();
        now.set("FOWLevel", 1).unwrap();
        let changed = high.changed(&now);
        let mut spies: Vec<Spy> = SystemId::ALL
            .into_iter()
            .map(|id| Spy { id, calls: 0 })
            .collect();
        let called = notify_readers(
            spies.iter_mut().map(|s| s as &mut dyn RenderSystem),
            &gpu.device,
            &mut targets,
            &now,
            &changed,
        );
        assert_eq!(called, [SystemId::FowClassic, SystemId::FluidFow]);
        for s in &spies {
            let reads = matches!(s.id, SystemId::FluidFow | SystemId::FowClassic);
            assert_eq!(s.calls, u32::from(reads), "{:?}", s.id);
        }

        // MSAA is read by render-core only: no subsystem hears of it.
        let mut off = high.clone();
        off.set("MSAASamples", 1).unwrap();
        let called = notify_readers(
            spies.iter_mut().map(|s| s as &mut dyn RenderSystem),
            &gpu.device,
            &mut targets,
            &off,
            &high.changed(&off),
        );
        assert!(called.is_empty());
        // Nothing changed, nobody is called.
        let called = notify_readers(
            spies.iter_mut().map(|s| s as &mut dyn RenderSystem),
            &gpu.device,
            &mut targets,
            &high,
            &high.changed(&high),
        );
        assert!(called.is_empty());
    }

    /// Table 4.2: id, `[DisableSystems]` name, passes and slots.
    #[test]
    fn every_system_has_disable_name_and_slots_from_table() {
        let ws = WorldSystems::default();
        let systems = ws.list();
        let table: [(SystemId, Option<&str>, &[PassSlot]); 21] = [
            (
                SystemId::Terrain,
                Some("Terrain"),
                &[
                    PassSlot::Pass(PassId::TerrainBake),
                    PassSlot::Pass(PassId::ShadowMap),
                    PassSlot::Main(MainSlot::Terrain),
                ],
            ),
            (
                SystemId::Decal,
                Some("Decal"),
                &[PassSlot::Pass(PassId::TerrainBake)],
            ),
            (
                SystemId::Water,
                Some("Terrain"),
                &[
                    PassSlot::Pass(PassId::WaterPrep),
                    PassSlot::Main(MainSlot::Terrain),
                ],
            ),
            (
                SystemId::River,
                Some("River"),
                &[
                    PassSlot::Pass(PassId::TerrainBake),
                    PassSlot::Main(MainSlot::Terrain),
                ],
            ),
            (
                SystemId::Ice,
                Some("Terrain"),
                &[
                    PassSlot::Pass(PassId::Reflection),
                    PassSlot::Main(MainSlot::Terrain),
                ],
            ),
            (
                SystemId::Waves,
                Some("Waves"),
                &[PassSlot::Main(MainSlot::Waves)],
            ),
            (
                SystemId::City,
                Some("City"),
                &[
                    PassSlot::Pass(PassId::ShadowMap),
                    PassSlot::Pass(PassId::Reflection),
                    PassSlot::Main(MainSlot::City),
                ],
            ),
            (
                SystemId::Arrows,
                Some("Arrows"),
                &[PassSlot::Main(MainSlot::Arrows)],
            ),
            // No slot: `Overlay` draws the mesh in its pass into the map of the overlay.
            (SystemId::Trade, Some("Arrows"), &[]),
            (
                SystemId::Forest,
                Some("Forest"),
                &[
                    PassSlot::Pass(PassId::ShadowMap),
                    PassSlot::Pass(PassId::Reflection),
                    PassSlot::Main(MainSlot::Forest),
                ],
            ),
            (
                SystemId::Landmark,
                Some("LandmarkSystem"),
                &[
                    PassSlot::Pass(PassId::ShadowMap),
                    PassSlot::Pass(PassId::Reflection),
                    PassSlot::Main(MainSlot::Landmark),
                ],
            ),
            (
                SystemId::Unit,
                Some("Unit"),
                &[
                    PassSlot::Pass(PassId::ShadowMap),
                    PassSlot::Pass(PassId::Reflection),
                    PassSlot::Main(MainSlot::UnitZFail),
                    PassSlot::Main(MainSlot::Unit),
                ],
            ),
            (
                SystemId::Particle,
                Some("Particle"),
                &[PassSlot::Main(MainSlot::Particle)],
            ),
            (
                SystemId::FowClassic,
                Some("FOW"),
                &[PassSlot::Main(MainSlot::FowClassic)],
            ),
            (
                SystemId::FluidFow,
                Some("FluidFOW"),
                &[
                    PassSlot::Pass(PassId::FowMask),
                    PassSlot::Main(MainSlot::FluidFow),
                ],
            ),
            // The path draws as models through the unit draw: no pass of its own.
            (SystemId::MovementPath, Some("MovementPath"), &[]),
            // WOV0: B1 draws in the main pass, so the ground lines sit in `Main.Terrain` until WOV2
            // moves them into `ScreenOverlay`.
            (
                SystemId::Overlay,
                Some("Overlay"),
                &[
                    PassSlot::Pass(PassId::ScreenOverlay),
                    PassSlot::Main(MainSlot::Terrain),
                ],
            ),
            (
                SystemId::Minimap,
                Some("Minimap"),
                &[PassSlot::Pass(PassId::Minimap)],
            ),
            (SystemId::YieldIcons, Some("YieldIcons"), &[]),
            (SystemId::DistanceFog, Some("DistanceFog"), &[]),
            (
                SystemId::Shadows,
                None,
                &[PassSlot::Pass(PassId::ShadowBlur)],
            ),
        ];

        assert_eq!(systems.len(), table.len());
        let ids: HashSet<SystemId> = systems.iter().map(|s| s.id()).collect();
        assert_eq!(ids.len(), table.len(), "every subsystem id appears once");
        for (id, name, slots) in table {
            let sys = systems
                .iter()
                .find(|s| s.id() == id)
                .unwrap_or_else(|| panic!("{id:?} is in WorldSystems"));
            assert_eq!(id.disable_name(), name, "[DisableSystems] name of {id:?}");
            assert_eq!(sys.passes(), slots, "passes and slots of {id:?}");
        }
    }
}
