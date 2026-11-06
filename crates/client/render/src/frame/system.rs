//! Subsystems of the world and the contexts they get. A subsystem names
//! the passes and slots it draws into; `render-core` calls it, it never checks switches itself.
//!
//! `on_settings` reaches only the subsystems that read a changed key (`settings::KEYS`).

use glam::Mat4;

use super::graph::{MainSlot, PassId};
use super::open::FrameShared;
use super::targets::TargetPool;
use crate::renderer::FrameStats;
use crate::settings::{KeySet, RenderSettings};
use crate::skin::BoneTexture;
use crate::world::terrain::TerrainSampler;

/// Subsystems of table 4.2, and the render systems of mods.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SystemId {
    Terrain,
    Decal,
    Water,
    River,
    Ice,
    Waves,
    City,
    Arrows,
    Trade,
    Forest,
    Landmark,
    Unit,
    Particle,
    FowClassic,
    FluidFow,
    Overlay,
    MovementPath,
    Minimap,
    YieldIcons,
    DistanceFog,
    Shadows,
    /// A render system of a mod (client-hooks R1), numbered by the registry of mods. Not in
    /// [`SystemId::ALL`]: those are the engine's own.
    Mod(u16),
}

impl SystemId {
    /// Every subsystem of the engine, in the order of `WorldSystems`. The systems of mods follow them
    /// (`WorldSystems::mods`).
    pub const ALL: [SystemId; 21] = [
        SystemId::Terrain,
        SystemId::Decal,
        SystemId::Water,
        SystemId::River,
        SystemId::Ice,
        SystemId::Waves,
        SystemId::City,
        SystemId::Arrows,
        SystemId::Trade,
        SystemId::Forest,
        SystemId::Landmark,
        SystemId::Unit,
        SystemId::Particle,
        SystemId::FowClassic,
        SystemId::FluidFow,
        SystemId::Overlay,
        SystemId::MovementPath,
        SystemId::Minimap,
        SystemId::YieldIcons,
        SystemId::DistanceFog,
        SystemId::Shadows,
    ];

    /// The name in `config.ini` `[DisableSystems]` that switches this subsystem off (table 4.2).
    /// `Shadows` has none: `ShadowLevel` 0 turns it off.
    pub fn disable_name(self) -> Option<&'static str> {
        Some(match self {
            SystemId::Terrain | SystemId::Water | SystemId::Ice => "Terrain",
            SystemId::Decal => "Decal",
            SystemId::River => "River",
            SystemId::Waves => "Waves",
            SystemId::City => "City",
            SystemId::Arrows | SystemId::Trade => "Arrows",
            SystemId::Forest => "Forest",
            SystemId::Landmark => "LandmarkSystem",
            SystemId::Unit => "Unit",
            SystemId::Particle => "Particle",
            SystemId::FowClassic => "FOW",
            SystemId::FluidFow => "FluidFOW",
            SystemId::MovementPath => "MovementPath",
            SystemId::Overlay => "Overlay",
            SystemId::Minimap => "Minimap",
            SystemId::YieldIcons => "YieldIcons",
            SystemId::DistanceFog => "DistanceFog",
            // Neither has a `[DisableSystems]` name; a mod is switched off by not registering it.
            SystemId::Shadows | SystemId::Mod(_) => return None,
        })
    }
}

/// Where a subsystem draws: a whole pass opened by `render-core` (or encoded by the subsystem
/// for [`super::graph::PassMode::Encoder`] passes), or a slot of the main pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PassSlot {
    Pass(PassId),
    Main(MainSlot),
}

/// Declare targets and shader families, once.
pub struct InitContext<'a> {
    pub device: &'a wgpu::Device,
    pub targets: &'a mut TargetPool,
}

/// The CPU phase: uploads, instances, constants.
pub struct PrepareContext<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    services: &'a WorldServices,
    world_frame: &'a mut Option<Mat4>,
    terrain: Option<&'a TerrainSampler>,
}

impl<'a> PrepareContext<'a> {
    pub fn new(
        device: &'a wgpu::Device,
        queue: &'a wgpu::Queue,
        services: &'a WorldServices,
        world_frame: &'a mut Option<Mat4>,
    ) -> Self {
        Self {
            device,
            queue,
            services,
            world_frame,
            terrain: None,
        }
    }

    /// The ground's sampler comes with the context (client-hooks R3).
    pub fn with_terrain(mut self, terrain: Option<&'a TerrainSampler>) -> Self {
        self.terrain = terrain;
        self
    }

    /// Heights, `hex_at` and `height_revision` of the ground; `None` until a map is set.
    pub fn terrain(&self) -> Option<&TerrainSampler> {
        self.terrain
    }

    pub fn services(&self) -> &WorldServices {
        self.services
    }

    /// `ours_to_orig` of the new map (`TerrainFrame::ours_to_orig()`); until a map is set only
    /// the basis rotation applies.
    pub fn set_world_frame(&mut self, ours_to_orig: Mat4) {
        *self.world_frame = Some(ours_to_orig);
    }
}

/// Inside a pass that `render-core` has begun. `view_group` is group 0 of the current view and
/// map copy; `copy_shift` is the copy's X shift, for subsystems that must pick per-copy data.
pub struct DrawContext<'a, 'p> {
    pub pass: &'a mut wgpu::RenderPass<'p>,
    pub view_group: &'a wgpu::BindGroup,
    /// Group 1 (pass inputs), if the pass has any.
    pub pass_group: Option<&'a wgpu::BindGroup>,
    pub copy_shift: f32,
    pub stats: &'a mut FrameStats,
    services: &'a WorldServices,
    terrain: Option<&'a TerrainSampler>,
}

impl<'a, 'p> DrawContext<'a, 'p> {
    pub fn new(
        pass: &'a mut wgpu::RenderPass<'p>,
        view_group: &'a wgpu::BindGroup,
        pass_group: Option<&'a wgpu::BindGroup>,
        copy_shift: f32,
        stats: &'a mut FrameStats,
        services: &'a WorldServices,
    ) -> Self {
        Self {
            pass,
            view_group,
            pass_group,
            copy_shift,
            stats,
            services,
            terrain: None,
        }
    }

    /// The ground's sampler comes with the context (client-hooks R3).
    pub fn with_terrain(mut self, terrain: Option<&'a TerrainSampler>) -> Self {
        self.terrain = terrain;
        self
    }

    /// Heights, `hex_at` and `height_revision` of the ground; `None` until a map is set.
    pub fn terrain(&self) -> Option<&TerrainSampler> {
        self.terrain
    }

    pub fn services(&self) -> &WorldServices {
        self.services
    }
}

/// For passes a subsystem encodes whole (`record_offscreen`).
pub struct EncoderContext<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub targets: &'a TargetPool,
    pub stats: &'a mut FrameStats,
    services: &'a WorldServices,
    timestamps: Option<wgpu::RenderPassTimestampWrites<'a>>,
}

impl<'a> EncoderContext<'a> {
    pub fn new(
        device: &'a wgpu::Device,
        queue: &'a wgpu::Queue,
        encoder: &'a mut wgpu::CommandEncoder,
        targets: &'a TargetPool,
        stats: &'a mut FrameStats,
        services: &'a WorldServices,
    ) -> Self {
        Self {
            device,
            queue,
            encoder,
            targets,
            stats,
            services,
            timestamps: None,
        }
    }

    /// The pass is timed on the GPU when the device allows.
    pub fn with_timestamps(
        mut self,
        timestamps: Option<wgpu::RenderPassTimestampWrites<'a>>,
    ) -> Self {
        self.timestamps = timestamps;
        self
    }

    /// Timestamp writes for the subsystem's first render pass of this step, to put in its
    /// `RenderPassDescriptor::timestamp_writes`; `None` if the device has none or the pass is
    /// already taken. Only the pass that takes it is timed, so a subsystem with several passes
    /// in one step gives the first one (the pass shows that pass's GPU time alone).
    pub fn timestamps(&mut self) -> Option<wgpu::RenderPassTimestampWrites<'a>> {
        self.timestamps.take()
    }

    pub fn services(&self) -> &WorldServices {
        self.services
    }
}

/// The same calls as [`DrawContext`] into the `ClassMask` target (`golden` only); no statistics.
pub struct MaskContext<'a, 'p> {
    pub pass: &'a mut wgpu::RenderPass<'p>,
    pub view_group: &'a wgpu::BindGroup,
    pub copy_shift: f32,
    services: &'a WorldServices,
}

impl<'a, 'p> MaskContext<'a, 'p> {
    pub fn new(
        pass: &'a mut wgpu::RenderPass<'p>,
        view_group: &'a wgpu::BindGroup,
        copy_shift: f32,
        services: &'a WorldServices,
    ) -> Self {
        Self {
            pass,
            view_group,
            copy_shift,
            services,
        }
    }

    pub fn services(&self) -> &WorldServices {
        self.services
    }
}

pub trait RenderSystem {
    /// Name in `[DisableSystems]`, flag of the original.
    fn id(&self) -> SystemId;
    /// Passes and slots it draws into.
    fn passes(&self) -> &'static [PassSlot];
    /// Declare targets and shader families.
    fn init(&mut self, cx: &mut InitContext<'_>);
    /// A key this subsystem reads has changed (`changed` lists them all, including keys of other
    /// subsystems): recreate what depends on it. Not called when none of its keys changed.
    fn on_settings(
        &mut self,
        _settings: &RenderSettings,
        _changed: &KeySet,
        _cx: &mut InitContext<'_>,
    ) {
    }
    /// CPU phase: uploads, instances, constants.
    fn prepare(&mut self, cx: &mut PrepareContext<'_>);
    /// Inside a pass already begun by `render-core`, once per visible map copy.
    fn record(&self, slot: PassSlot, cx: &mut DrawContext<'_, '_>);
    /// Its own passes, encoded whole.
    fn record_offscreen(&mut self, _pass: PassId, _cx: &mut EncoderContext<'_>) {}
    /// For `golden`.
    fn record_mask(&self, _slot: PassSlot, _cx: &mut MaskContext<'_, '_>) {}
    /// Unfinished work (baking, loads); `Renderer::settled()` waits for 0.
    fn pending_work(&self) -> u32 {
        0
    }
    /// Counters for `?stats=1`: what the subsystem did, by name (`fog.updates`). Read once per
    /// frame, for the subsystems that are on.
    fn counters(&self) -> Vec<(&'static str, u64)> {
        Vec::new()
    }
}

/// Animation service of `world-anim`: bone references for `Unit`, `Landmark`, `City`,
/// `MovementPath`, `Particle` (the body is `crate::anim`).
pub use crate::anim::AnimService;

/// Shared services of the world: no slots, no switches. Their `prepare` runs first in the
/// `Prepare` pass, before the subsystems'.
pub struct WorldServices {
    pub anim: AnimService,
    /// The constants of the frame for systems outside the engine (client-hooks R3).
    pub frame: FrameShared,
    /// The GPU half of the bones `anim` bakes (render-core 4.12); made at the first `prepare`, when
    /// the device is there.
    bones: Option<BoneTexture>,
}

impl Default for WorldServices {
    /// The steady instances of the world are drawn with clocked references (B3 packet F): the
    /// vertex shader runs their time, the CPU makes a reference only when a play changes.
    fn default() -> Self {
        let mut anim = AnimService::default();
        anim.set_gpu_clock(true);
        Self {
            anim,
            frame: FrameShared::default(),
            bones: None,
        }
    }
}

impl WorldServices {
    /// The animation first (it bakes clips into its store), then the changes of the store to the
    /// bone texture, at most the upload limit a frame.
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.anim.prepare(device, queue);
        let bones = self.bones.get_or_insert_with(|| BoneTexture::new(device));
        bones.flush_from(self.anim.cache_mut().store_mut(), device, queue);
    }

    /// The bone texture; `None` before the first `prepare`.
    pub fn bones(&self) -> Option<&BoneTexture> {
        self.bones.as_ref()
    }

    pub fn pending_work(&self) -> u32 {
        self.anim.pending_work()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disable_names_come_from_table_4_2() {
        assert_eq!(SystemId::Water.disable_name(), Some("Terrain"));
        assert_eq!(SystemId::Landmark.disable_name(), Some("LandmarkSystem"));
        assert_eq!(SystemId::Trade.disable_name(), Some("Arrows"));
        assert_eq!(SystemId::Shadows.disable_name(), None);
    }

    #[test]
    fn services_have_no_backlog_at_start() {
        assert_eq!(WorldServices::default().pending_work(), 0);
    }

    /// WAN2/RC-10: what the animation bakes into its store is in the bone texture after the
    /// `prepare` of the services, slot for slot.
    #[test]
    fn the_services_send_the_baked_clips_to_the_bone_texture() {
        use crate::anim::data::{ModelAnim, fixtures};
        use crate::anim::{AnimState, AnimTarget};
        use crate::gpu_test::{gpu, read_texture};
        use crate::skin::{TEXTURE_WIDTH, bone_texel};
        use asset_format::LogicalId;

        let Some(g) = gpu(
            "the_services_send_the_baked_clips_to_the_bone_texture",
            wgpu::Backends::all(),
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let mut s = WorldServices::default();
        assert!(s.bones().is_none(), "no texture before the first prepare");
        let id = LogicalId::new("md:warrior");
        let (m, pack) = fixtures::model("warrior", 3);
        s.anim
            .register_model(id.clone(), ModelAnim::new(&m, Some(pack), None).unwrap());
        let target = AnimTarget::Figure {
            unit: (0, 1),
            index: 0,
        };
        let h = s.anim.attach(&id, target).unwrap();
        s.anim.set_state(h, AnimState::default());
        for _ in 0..8 {
            s.prepare(&g.device, &g.queue);
            if s.anim.is_ready(h) && s.pending_work() == 0 {
                break;
            }
        }
        assert!(s.anim.is_ready(h));
        let bones = s.bones().unwrap();
        assert!(bones.uploaded_last() > 0 || bones.store().pending_bytes() == 0);
        let data: Vec<f32> = bytemuck::cast_slice(&read_texture(
            &g,
            bones.texture(),
            0,
            0,
            (TEXTURE_WIDTH, bones.rows()),
            16,
        ))
        .to_vec();
        // The frames of the moment (the reference may be a clocked one; the clock is at 0).
        let r = s.anim.resolved_ref(h, 0.0);
        for k in 0..3u32 {
            let slot = r.base + k;
            let (x, y) = bone_texel(slot);
            let at = ((y * TEXTURE_WIDTH + x) * 4) as usize;
            let want = s.anim.cache().store().bone(slot);
            assert_eq!(data[at..at + 12], want, "bone {k} of the first frame");
        }
        // The swing of the clip moves a bone: the baked frames are not all the rest pose.
        let rest = s.anim.cache().store().bone(r.base);
        assert!((0..60).any(|f| s.anim.cache().store().bone(r.base + f * 3 + 1) != rest));
    }
}
