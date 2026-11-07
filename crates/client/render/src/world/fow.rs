//! `FowClassic` and `FluidFow` subsystems (world-overlays 4.2).
//!
//! `FluidFow` owns the fog-state texture that the B1 shaders of terrain, water, decals, trees and
//! units read in group 2 (WOV0), and the fluid fog of the original (WOV1): the mask of the cells
//! ([`mask`], pass `FowMask`), the cull of the clouds ([`cull`]) and the three cloud layers
//! ([`clouds`], `Main.FluidFow`) with the puffs over them ([`puffs`]). `FowClassic` draws the
//! classic fog at `FOWLevel` 0 ([`classic`]). The input is [`FogInput`] (`set_fog`).

pub mod classic;
pub mod clouds;
pub mod cull;
pub mod mask;
pub mod puffs;
pub mod state;

use crate::perf::{Cluster, DeviceTrack, Tex};
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use asset_format::{LogicalId, TextureBlob};

use classic::FowClassicGpu;
use clouds::{CloudFrame, FowClouds, view_proj_orig, visible_tiles};
use cull::{CullGrid, cull_grid};
use mask::{FowMask, FowView, MaskInstances, MaskView, collect_instances};
use puffs::{FowPuffs, PuffFrame};

pub use state::{
    CLOUD_LAYERS, CloudLayer, FOG_HIDDEN_FACTOR, FOG_HIDDEN_SATURATION, FOG_UNREVEALED_RGB,
    FogInput, cloud_uv_scale, fog_texel, fog_texels,
};

use super::b1_env::B1Env;
use crate::frame::{
    DrawContext, EncoderContext, FOW_MASK, InitContext, MainSlot, MaskContext, PassId, PassSlot,
    PrepareContext, RenderSystem, SystemId,
};
use crate::settings::{KeySet, RenderSettings};

/// More changed cells than this are uploaded as the whole texture.
const WHOLE_TEXTURE_FROM: usize = 64;

/// The key of the level in the settings.
const FOW_LEVEL: &str = "FOWLevel";

/// The fog-state texture of B1 and its bind group (group 2 of every B1 pipeline). One byte per cell:
/// 0 unrevealed, 1 revealed and hidden, 2 visible. `FluidFow` owns it; the subsystems that draw
/// over the map hold a clone of the `Rc` and bind it.
pub struct FogGpu {
    pub(crate) bind_group: wgpu::BindGroup,
    texture: Tex,
    pub width: u32,
    pub height: u32,
}

impl FogGpu {
    fn new(env: &B1Env, input: &FogInput) -> Self {
        let (width, height) = (input.width, input.height);
        let texture = env.device.tex(
            &wgpu::TextureDescriptor {
                label: Some("fog state"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            Cluster::Other,
        );
        let bind_group = env.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fog state"),
            layout: &env.blend.fog_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(
                    &texture.create_view(&wgpu::TextureViewDescriptor::default()),
                ),
            }],
        });
        let gpu = Self {
            bind_group,
            texture,
            width,
            height,
        };
        gpu.write(env, input, None);
        gpu
    }

    /// Writes the states of `cells` (indices), or of every cell.
    fn write(&self, env: &B1Env, input: &FogInput, cells: Option<&[u32]>) {
        let texels = fog_texels(input);
        match cells {
            None => env.queue.write_texture(
                self.texture.as_image_copy(),
                &texels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.width),
                    rows_per_image: Some(self.height),
                },
                wgpu::Extent3d {
                    width: self.width,
                    height: self.height,
                    depth_or_array_layers: 1,
                },
            ),
            Some(cells) => {
                for &i in cells {
                    let (x, y) = (i % self.width, i / self.width);
                    let at = i as usize;
                    env.queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &self.texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d { x, y, z: 0 },
                            aspect: wgpu::TextureAspect::All,
                        },
                        &texels[at..at + 1],
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(1),
                            rows_per_image: Some(1),
                        },
                        wgpu::Extent3d {
                            width: 1,
                            height: 1,
                            depth_or_array_layers: 1,
                        },
                    );
                }
            }
        }
    }
}

/// Counters of `?stats=1`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FogCounters {
    /// `set_fog` calls that changed something.
    pub updates: u64,
    /// Texels written by the last update (the whole map for a new map).
    pub last_texels: u64,
    /// Cells that are not unrevealed.
    pub revealed: u64,
}

/// The level the fluid fog works at: `min(FOWLevel, 2)` (`FluidFOWSystem::Startup`). 0 draws no
/// clouds (the classic fog is the fog), 1 has a 512 mask, 2 a 1024 one.
// re: FluidFOWSystem.h m_uFogLevel: min(GeneralSettings::g_uFOWLevel, 2)
pub fn fluid_level(fow_level: i64) -> i64 {
    fow_level.clamp(0, 2)
}

/// Classic fog of war (`[DisableSystems] FOW`): the fog at `FOWLevel` 0, where the fluid fog draws
/// nothing.
pub struct FowClassic {
    env: Option<B1Env>,
    level: i64,
    input: Option<FogInput>,
    view: FowView,
    gpu: Option<FowClassicGpu>,
    /// The meshes are those of the fog of this revision seen in this frame of the world.
    built_for: Option<(u64, glam::Mat4)>,
    revision: u64,
    drawn: Cell<u32>,
}

impl Default for FowClassic {
    fn default() -> Self {
        Self {
            env: None,
            level: 3,
            input: None,
            view: FowView::default(),
            gpu: None,
            built_for: None,
            revision: 0,
            drawn: Cell::new(0),
        }
    }
}

impl FowClassic {
    pub(crate) fn bind(&mut self, env: &B1Env) {
        self.gpu = Some(FowClassicGpu::new(&env.device, env.samples));
        self.env = Some(env.clone());
    }

    /// The fog to cover (the same as `FluidFow`'s).
    pub fn set_fog(&mut self, fog: Option<FogInput>) {
        if self.input != fog {
            self.input = fog;
            self.revision += 1;
        }
    }

    pub fn set_level(&mut self, fow_level: i64) {
        self.level = fow_level;
    }

    pub fn set_frame(&mut self, view: FowView) {
        self.view = view;
    }

    /// Whether the classic fog is the one drawn.
    pub fn active(&self) -> bool {
        self.level <= 0
    }
}

impl RenderSystem for FowClassic {
    fn id(&self) -> SystemId {
        SystemId::FowClassic
    }

    fn passes(&self) -> &'static [PassSlot] {
        &[PassSlot::Main(MainSlot::FowClassic)]
    }

    fn init(&mut self, _cx: &mut InitContext<'_>) {}

    fn on_settings(
        &mut self,
        settings: &RenderSettings,
        changed: &KeySet,
        _cx: &mut InitContext<'_>,
    ) {
        if changed.contains(FOW_LEVEL) {
            self.level = settings.get(FOW_LEVEL).unwrap_or(3);
        }
    }

    fn prepare(&mut self, cx: &mut PrepareContext<'_>) {
        self.drawn.set(0);
        if !self.active() {
            return;
        }
        let Some(gpu) = &mut self.gpu else { return };
        let key = Some((self.revision, self.view.ours_to_orig));
        if self.built_for != key {
            gpu.set_fog(
                cx.device,
                self.input.as_ref().map(|f| (f, &self.view.ours_to_orig)),
            );
            self.built_for = key;
        }
        if let Some(fog) = &self.input {
            gpu.prepare(
                cx.queue,
                &view_proj_orig(&self.view.view_proj, &self.view.ours_to_orig),
                classic::world_width(fog),
            );
        }
    }

    fn record(&self, slot: PassSlot, cx: &mut DrawContext<'_, '_>) {
        if slot != PassSlot::Main(MainSlot::FowClassic) || !self.active() {
            return;
        }
        let Some(gpu) = &self.gpu else { return };
        let sizes = gpu.draw(cx.pass, cx.copy_shift);
        self.drawn.set(self.drawn.get() + sizes.len() as u32);
        for n in sizes {
            cx.stats.record_draw(n);
        }
    }

    fn counters(&self) -> Vec<(&'static str, u64)> {
        let c = self.gpu.as_ref().map(|g| g.counters()).unwrap_or_default();
        vec![
            ("fow.classic_opaque_hexes", u64::from(c.opaque_hexes)),
            (
                "fow.classic_translucent_hexes",
                u64::from(c.translucent_hexes),
            ),
            ("fow.classic_draws", u64::from(self.drawn.get())),
        ]
    }
}

/// Fluid fog of war (`[DisableSystems] FluidFOW`): the state of the cells, the mask the terrain
/// and the clouds read, and the clouds over the unrevealed cells.
pub struct FluidFow {
    env: Option<B1Env>,
    input: Option<FogInput>,
    fog: Option<Rc<FogGpu>>,
    level: i64,
    view: FowView,
    time: f32,
    mask: Option<FowMask>,
    clouds: Option<FowClouds>,
    puffs: Option<FowPuffs>,
    /// Where the clouds need not be drawn.
    cull: Option<CullGrid>,
    /// Frames prepared, and the frame the clouds were last drawn in: the clouds cover the world
    /// plane, whatever the number of map copies.
    frame: u64,
    drawn: Cell<u64>,
    /// Draws of the clouds in the last frame.
    cloud_draws: Cell<u32>,
    counters: FogCounters,
    /// Changes whenever the fog given (or its absence) does: the mask is a function of it.
    fog_rev: u64,
    /// What the mask instances on the GPU were collected from, and whether any of them was
    /// unrevealed (the only class that shows puffs).
    mask_key: Option<MaskKey>,
    unrevealed_any: bool,
    mask_rebuilds: u64,
    /// The clouds and the puffs are not drawn (`CameraHints::hide_fow_clouds` of a mod's camera); the mask
    /// still is, for the current view.
    clouds_hidden: bool,
}

/// What the instances of the mask depend on.
#[derive(Debug, Clone, Copy, PartialEq)]
struct MaskKey {
    fog_rev: u64,
    has_fog: bool,
    view: MaskView,
    to_orig: glam::Mat4,
}

/// The mask is collected and uploaded again only when its key changed.
fn mask_stale(last: Option<&MaskKey>, now: &MaskKey) -> bool {
    last != Some(now)
}

/// The puffs are simulated and built only when the clouds are on and some hex of the mask is
/// unrevealed: a puff is drawn through the unrevealed channel of the mask (`textureps` kills it
/// where that is 0), so over a revealed map none can be seen.
fn puffs_possible(clouds_on: bool, unrevealed_hexes: bool) -> bool {
    clouds_on && unrevealed_hexes
}

impl Default for FluidFow {
    fn default() -> Self {
        Self {
            env: None,
            input: None,
            fog: None,
            level: 3,
            view: FowView::default(),
            time: 0.0,
            mask: None,
            clouds: None,
            puffs: None,
            cull: None,
            frame: 0,
            drawn: Cell::new(0),
            cloud_draws: Cell::new(0),
            counters: FogCounters::default(),
            fog_rev: 0,
            mask_key: None,
            unrevealed_any: false,
            mask_rebuilds: 0,
            clouds_hidden: false,
        }
    }
}

impl FluidFow {
    /// Hands over the device (`Renderer` does it once).
    pub(crate) fn bind(&mut self, env: &B1Env) {
        self.mask = Some(FowMask::new(&env.device));
        self.clouds = Some(FowClouds::new(&env.device, env.samples));
        self.puffs = Some(FowPuffs::new(&env.device, env.samples));
        self.env = Some(env.clone());
    }

    /// The fog of the map. Replaces the whole fog; the same fog again does nothing, a map of the
    /// same size changes only the cells that differ. Returns true if the fog texture is a new one
    /// (the subsystems that hold it must take [`Self::fog_gpu`] again).
    pub fn set_fog(&mut self, input: FogInput) -> bool {
        let Some(env) = &self.env else {
            self.fog_rev += 1;
            self.input = Some(input);
            return false;
        };
        if self.input.as_ref() == Some(&input) && self.fog.is_some() {
            return false;
        }
        self.fog_rev += 1;
        let revealed = input
            .cells
            .iter()
            .filter(|&&c| c != crate::terrain::Fog::Unrevealed)
            .count() as u64;
        let changed = self
            .input
            .as_ref()
            .and_then(|old| input.changed_cells(old))
            .filter(|cells| cells.len() < WHOLE_TEXTURE_FROM);
        let reuse = self
            .fog
            .as_ref()
            .is_some_and(|f| (f.width, f.height) == (input.width, input.height));
        let new_texture = !reuse;
        match (&self.fog, &changed) {
            (Some(gpu), Some(cells)) if reuse => {
                gpu.write(env, &input, Some(cells));
                self.counters.last_texels = cells.len() as u64;
            }
            (Some(gpu), _) if reuse => {
                gpu.write(env, &input, None);
                self.counters.last_texels = u64::from(gpu.width) * u64::from(gpu.height);
            }
            _ => {
                let gpu = FogGpu::new(env, &input);
                self.counters.last_texels = u64::from(gpu.width) * u64::from(gpu.height);
                self.fog = Some(Rc::new(gpu));
            }
        }
        self.cull = Some(cull_grid(&input));
        self.counters.updates += 1;
        self.counters.revealed = revealed;
        self.input = Some(input);
        new_texture
    }

    /// The fog last given.
    pub fn fog(&self) -> Option<&FogInput> {
        self.input.as_ref()
    }

    /// The state texture and its bind group, for what draws over the map.
    pub fn fog_gpu(&self) -> Option<Rc<FogGpu>> {
        self.fog.clone()
    }

    /// Back to no map.
    pub fn clear(&mut self) {
        self.fog_rev += 1;
        self.input = None;
        self.fog = None;
        self.cull = None;
    }

    /// Takes the three cloud textures of the fog ([`CLOUD_LAYERS`]) from `textures`. Without them
    /// (or on a device that cannot sample them) no clouds are drawn.
    pub fn set_cloud_textures(
        &mut self,
        textures: &HashMap<LogicalId, TextureBlob>,
    ) -> Result<(), String> {
        let (Some(env), Some(clouds)) = (&self.env, &mut self.clouds) else {
            return Err("the fog is not bound to a device".to_owned());
        };
        // The puffs are drawn when their atlas is there too; the clouds do not wait for it.
        if let Some(puffs) = &mut self.puffs
            && let Err(e) = puffs.set_texture(&env.device, &env.queue, textures)
        {
            tracing::info!("fog puffs unavailable ({e})");
        }
        clouds.set_textures(&env.device, &env.queue, textures)
    }

    /// Seconds since the start: the scrolling of the clouds.
    pub fn set_time(&mut self, seconds: f32) {
        self.time = seconds;
    }

    /// The camera of the frame about to be drawn.
    pub fn set_frame(&mut self, view: FowView) {
        self.view = view;
    }

    /// `FOWLevel` as the settings give it.
    pub fn set_level(&mut self, fow_level: i64) {
        self.level = fow_level;
    }

    /// The clouds and the puffs are left out of the frame (a low camera of a mod sees them edge-on, a flat
    /// strip) while the mask goes on being drawn for the current view: the ground, the water, the objects
    /// and the overlay read it. Switching the whole system off left them a mask of an older view, read
    /// through the rectangle of this one, and the seen ground went the flat colour of the clouds
    /// (client-hooks C2, 2026-11-07).
    pub fn set_clouds_hidden(&mut self, hidden: bool) {
        self.clouds_hidden = hidden;
    }

    pub fn clouds_hidden(&self) -> bool {
        self.clouds_hidden
    }

    /// The clouds are drawn at this level.
    pub fn clouds_active(&self) -> bool {
        fluid_level(self.level) >= 1
    }

    /// The base layer the ground and the trees show where nothing is known (`FOWBaseCached`), when
    /// the fluid fog draws its mask from a map's fog and has its cloud textures.
    pub fn base_view(&self) -> Option<&wgpu::TextureView> {
        if !self.clouds_active() || self.input.is_none() || self.mask.is_none() {
            return None;
        }
        self.clouds.as_ref()?.base_view()
    }

    pub fn stats(&self) -> FogCounters {
        self.counters
    }

    /// The hexes the mask of the last frame was drawn from.
    pub fn mask_counters(&self) -> mask::MaskCounters {
        self.mask.as_ref().map(|m| m.counters()).unwrap_or_default()
    }

    /// The work of the puffs and of the mask collection so far (for `fow.puffs_*` counters).
    pub fn puff_stats(&self) -> puffs::PuffStats {
        let (skipped, uploads) = self.puffs.as_ref().map_or((0, 0), FowPuffs::work);
        puffs::PuffStats {
            steps: self.puffs.as_ref().map_or(0, FowPuffs::steps),
            skipped,
            uploads,
            mask_rebuilds: self.mask_rebuilds,
        }
    }

    pub fn cloud_counters(&self) -> clouds::CloudCounters {
        self.clouds
            .as_ref()
            .map(|c| c.counters())
            .unwrap_or_default()
    }
}

impl RenderSystem for FluidFow {
    fn id(&self) -> SystemId {
        SystemId::FluidFow
    }

    fn passes(&self) -> &'static [PassSlot] {
        &[
            PassSlot::Pass(PassId::FowMask),
            PassSlot::Main(MainSlot::FluidFow),
        ]
    }

    fn init(&mut self, _cx: &mut InitContext<'_>) {}

    fn on_settings(
        &mut self,
        settings: &RenderSettings,
        changed: &KeySet,
        _cx: &mut InitContext<'_>,
    ) {
        if changed.contains(FOW_LEVEL) {
            self.level = settings.get(FOW_LEVEL).unwrap_or(3);
        }
    }

    fn prepare(&mut self, cx: &mut PrepareContext<'_>) {
        self.frame += 1;
        self.cloud_draws.set(0);
        let (Some(mask), Some(clouds)) = (&mut self.mask, &mut self.clouds) else {
            return;
        };
        let to_orig = self.view.ours_to_orig;
        let view = MaskView::of(&self.view);
        let key = MaskKey {
            fog_rev: self.fog_rev,
            has_fog: self.input.is_some(),
            view,
            to_orig,
        };
        let stale = mask_stale(self.mask_key.as_ref(), &key);
        let Some(fog) = &self.input else {
            if stale {
                mask.upload(cx.queue, &view, &MaskInstances::default());
                self.mask_key = Some(key);
                self.unrevealed_any = false;
                self.mask_rebuilds += 1;
            }
            if let Some(puffs) = &mut self.puffs {
                puffs.skip();
            }
            return;
        };
        // The instances are a function of the fog and the rectangle: a still view over an
        // unchanged fog keeps the ones on the GPU.
        if stale {
            let s = crate::perf::slow::scope("fow.collect");
            let instances = collect_instances(fog, &view, &to_orig);
            drop(s);
            self.unrevealed_any = !instances.unrevealed.is_empty();
            if let Some(puffs) = &mut self.puffs {
                let _s = crate::perf::slow::scope("fow.puffs_set_active");
                puffs.set_active(self.mask_rebuilds + 1, &instances.unrevealed);
            }
            let _s = crate::perf::slow::scope("fow.mask_upload");
            mask.upload(cx.queue, &view, &instances);
            drop(_s);
            self.mask_key = Some(key);
            self.mask_rebuilds += 1;
        }
        // Hidden clouds: no tile is planned (as at level 0); the mask above is made all the same.
        let (tiles, culled) = if fluid_level(self.level) >= 1 && !self.clouds_hidden {
            visible_tiles(&view, &to_orig, self.cull.as_ref())
        } else {
            (Vec::new(), 0)
        };
        let view_proj_orig = view_proj_orig(&self.view.view_proj, &to_orig);
        let _s = crate::perf::slow::scope("fow.clouds_upload");
        clouds.upload(
            cx.queue,
            &CloudFrame {
                view_proj_orig,
                view,
                tiles: &tiles,
                culled,
                world_width: classic::world_width(fog),
                time: self.time,
            },
        );
        drop(_s);
        if let Some(puffs) = &mut self.puffs {
            let _s = crate::perf::slow::scope("fow.puffs_upload");
            let frame = PuffFrame {
                view_proj_orig,
                view,
                terrain_scale_z: clouds::TERRAIN_SCALE_Z,
                time: self.time,
            };
            // No unrevealed hex in the mask rectangle: no puff can be seen, so none is simulated
            // or built (the fluid waits where it is until the fog is back).
            if puffs_possible(
                fluid_level(self.level) >= 1 && !self.clouds_hidden,
                self.unrevealed_any,
            ) {
                puffs.upload(cx.queue, Some(&frame));
            } else {
                puffs.skip();
            }
        }
    }

    fn record(&self, slot: PassSlot, cx: &mut DrawContext<'_, '_>) {
        if slot != PassSlot::Main(MainSlot::FluidFow)
            || !self.clouds_active()
            || self.clouds_hidden
            || self.drawn.replace(self.frame) == self.frame
        {
            return;
        }
        // Over everything the map shows; the layers cover the plane, whatever the copies.
        let Some(clouds) = &self.clouds else { return };
        let sizes = clouds.draw(cx.pass);
        self.cloud_draws.set(sizes.len() as u32);
        for n in sizes {
            cx.stats.record_draw(n);
        }
        // The puffs last, over the clouds and over what stands at the edge (S1a ev 2280).
        if let Some(n) = self.puffs.as_ref().and_then(|p| p.draw(cx.pass)) {
            cx.stats.record_draw(n);
        }
    }

    fn record_offscreen(&mut self, pass: PassId, cx: &mut EncoderContext<'_>) {
        if pass != PassId::FowMask {
            return;
        }
        if let Some(mask) = &self.mask {
            // Without a map the mask is white, as the original writes it when the fog is off.
            mask.record(cx, self.input.is_none());
        }
        if let (Some(clouds), Some(target)) = (&mut self.clouds, cx.targets.get(FOW_MASK)) {
            clouds.bind_mask(cx.device, target);
        }
        if let (Some(puffs), Some(target)) = (&mut self.puffs, cx.targets.get(FOW_MASK)) {
            puffs.bind_mask(cx.device, target);
        }
    }

    fn record_mask(&self, slot: PassSlot, cx: &mut MaskContext<'_, '_>) {
        // `FOG_BIT` where the clouds show. The pass has one byte per pixel and no blend: the bit
        // replaces the class of what lies under the clouds.
        if slot != PassSlot::Main(MainSlot::FluidFow) || !self.clouds_active() || self.clouds_hidden
        {
            return;
        }
        if let Some(clouds) = &self.clouds {
            clouds.draw_mask(cx.pass);
        }
    }

    fn counters(&self) -> Vec<(&'static str, u64)> {
        let c = self.counters;
        let m = self.mask_counters();
        let k = self.cloud_counters();
        vec![
            ("fog.updates", c.updates),
            ("fog.texels_written", c.last_texels),
            ("fog.revealed_cells", c.revealed),
            ("fow.mask_revealed", u64::from(m.revealed)),
            ("fow.mask_visible", u64::from(m.visible)),
            ("fow.mask_unrevealed", u64::from(m.unrevealed)),
            ("fow.mask_draws", u64::from(m.draws)),
            ("fow.cloud_tiles", u64::from(k.tiles)),
            ("fow.cloud_tiles_culled", u64::from(k.tiles_culled)),
            ("fow.cloud_draws", u64::from(self.cloud_draws.get())),
            (
                "fow.puffs",
                u64::from(self.puffs.as_ref().map_or(0, FowPuffs::count)),
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::test_util::{land, map};
    use crate::terrain::{Fog, TerrainCell};

    #[test]
    fn the_mask_is_collected_again_only_when_its_key_changes() {
        let view = MaskView::snapped(glam::Vec2::ZERO, glam::Vec2::splat(300.0));
        let key = MaskKey {
            fog_rev: 1,
            has_fog: true,
            view,
            to_orig: glam::Mat4::IDENTITY,
        };
        assert!(mask_stale(None, &key));
        assert!(!mask_stale(Some(&key), &key));
        let moved = MaskKey {
            view: MaskView::snapped(glam::Vec2::splat(100.0), glam::Vec2::splat(500.0)),
            ..key
        };
        assert!(mask_stale(Some(&key), &moved));
        assert!(mask_stale(Some(&key), &MaskKey { fog_rev: 2, ..key }));
        assert!(mask_stale(
            Some(&key),
            &MaskKey {
                has_fog: false,
                ..key
            }
        ));
    }

    #[test]
    fn puffs_need_clouds_and_an_unrevealed_hex() {
        assert!(puffs_possible(true, true));
        assert!(!puffs_possible(true, false), "a revealed map shows no puff");
        assert!(!puffs_possible(false, true), "no clouds, no puffs");
        assert!(!puffs_possible(false, false));
    }

    #[test]
    fn stats_start_empty() {
        assert_eq!(
            FluidFow::default().puff_stats(),
            puffs::PuffStats::default()
        );
    }

    #[test]
    fn counters_start_empty() {
        let f = FluidFow::default();
        assert_eq!(f.stats(), FogCounters::default());
        assert!(f.fog().is_none() && f.fog_gpu().is_none());
    }

    #[test]
    fn an_unbound_fog_keeps_the_last_input() {
        let snap = map(2, 2, |x, _| TerrainCell {
            fog: if x == 0 { Fog::Hidden } else { Fog::Visible },
            ..land()
        });
        let mut f = FluidFow::default();
        assert!(!f.set_fog(FogInput::from_snapshot(&snap)));
        assert_eq!(f.fog().map(|i| i.cells.len()), Some(4));
    }

    #[test]
    fn the_fluid_fog_works_at_most_at_level_two_and_none_at_zero() {
        assert_eq!(
            [0, 1, 2, 3].map(fluid_level),
            [0, 1, 2, 2],
            "level 3 is never reached (Startup clamps it)"
        );
        let mut f = FluidFow::default();
        assert!(f.clouds_active(), "the default level is 3");
        f.set_level(0);
        assert!(!f.clouds_active());
        f.set_level(1);
        assert!(f.clouds_active());
        let mut c = FowClassic::default();
        assert!(!c.active());
        c.set_level(0);
        assert!(c.active());
    }
}
