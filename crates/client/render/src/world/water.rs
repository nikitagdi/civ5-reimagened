//! `Water` subsystem (world-water WW-0): the ocean and the lakes.
//! The surface is the ported `WaterShader` (`water/ocean.rs`), the map of the currents is drawn
//! once in `WaterPrep` (`water/currents.rs`), the files come from the pack as `WaterData`
//! (`water/data.rs`).
//!
//! The subsystem draws nothing until it has everything it reads: the textures of the pack
//! ([`WaterData::ocean_ready`]), the heights of a map ([`Water::set_terrain`]), the ramp of the fog
//! and the lookup of the fog of war ([`Water::set_lighting`]) and a device ([`Water::bind`]).
//! [`Water::ready`] says so; the renderer keeps B1's water in the frame until then.

pub mod coast;
pub mod currents;
pub mod data;
pub mod ice;
pub mod ocean;
pub mod river;
pub mod waves;

#[cfg(test)]
mod gpu_tests;

use std::sync::Arc;

use self::currents::{CURRENTS, Currents};
use self::data::WaterData;
use self::ocean::{
    HeightBuild, HeightState, OCEAN_HEIGHT, Ocean, OceanInputs, OceanMap, SHELF_RADIUS, TexelRect,
    WaterFrame, height_max_side,
};
use self::river::draw::{RiverFeed, RiverTextures};
use super::b1_env::B1Env;
use super::terrain::CellRect;
use super::terrain::sampler::{ORIG_EXTRA_ROWS, TerrainFrame, TerrainSampler};
use crate::frame::{
    DrawContext, EncoderContext, InitContext, MainSlot, MaskContext, PassId, PassSlot,
    PrepareContext, RenderSystem, SystemId,
};
use crate::settings::{KeySet, RenderSettings};

const WATER_QUALITY: &str = "TerrainWaterQuality";
const REFLECTION_LEVEL: &str = "ReflectionLevel";

/// The level of the water in the world of the original (`fOceanHeight`): the number the other
/// subsystems ask (`fWaterHeight` of the units).
pub fn water_level() -> f32 {
    OCEAN_HEIGHT
}

/// Rows of texels of the height map of the water for a field of `field_rows`: the map is stretched
/// over the whole world of the original, which is [`ORIG_EXTRA_ROWS`] longer than our field along y
/// (`quad`, `g_WorldToTerrainTextureSpace`), the extra rows being its north end. Made from the field
/// alone the map was stretched by that much, and the water of a hex was drawn up to half a row of
/// hexes north of it (Lake Victoria's water north of its bowl). The rows past the field read as the
/// deep sea, the height 0 outside the map.
// SIMPLIFIED(SIMP-exe-1074): the extra rows of the world are deep sea, the original's padding rows
// are not read from the exe.
pub fn height_rows(field_rows: u32) -> u32 {
    field_rows + ORIG_EXTRA_ROWS
}

/// Cells of the field the heights of the water make again in a frame, and the texels round a cell
/// that are made with it.
const HEIGHT_CELLS_PER_FRAME: usize = 12;
const HEIGHT_CELL_MARGIN: i32 = 8;
/// Units of work (a read of the field, a texel blurred or averaged) the first build of the heights of
/// a map does in a frame.
const HEIGHT_BUILD_UNITS_PER_FRAME: usize = 150_000;

/// The blur of the shelf of the heights, in texels. With the styles of the coast (`coaststyles.xml`)
/// the shore is the height field the coast meshes write (`BlendAllCoastMeshes`), as in the original,
/// whose water has no blur of its own: a blur on top widens the transparent shallow strip of the
/// depth ramp into a soft green band of the sea floor along the coast. Without styles (the assets of B1)
/// the blur stands in for the shelf.
pub fn shelf_radius(data: &WaterData) -> u32 {
    if data.coast().styles.is_empty() {
        SHELF_RADIUS
    } else {
        0
    }
}

pub struct Water {
    gpu: Option<Gpu>,
    data: Option<Arc<WaterData>>,
    map: Option<OceanMap>,
    /// The frame of the map, for the rivers.
    terrain_frame: Option<TerrainFrame>,
    /// The heights, made cell by cell as the field builds.
    height: Option<HeightState>,
    /// The revision of the field the heights are being made for, the cells still to make, and what
    /// the texture is yet to be given: all of it, or these rectangles by level.
    height_target: Option<u64>,
    height_pending: std::collections::BTreeSet<(i32, i32)>,
    height_upload_all: bool,
    /// The cells whose field was not built when the heights were made: their depth is made again,
    /// unbudgeted, the frame the field gets them.
    height_unbuilt: std::collections::BTreeSet<(i32, i32)>,
    /// The blur of the shelf in texels ([`shelf_radius`]).
    shelf_radius: u32,
    /// The heights being made over frames, and the cells that changed meanwhile (looked at when
    /// they are done).
    building: Option<HeightBuild>,
    build_late: std::collections::BTreeSet<(i32, i32)>,
    height_dirty: Option<TexelRect>,
    /// The longest side of a texture on the device (`u32::MAX` until one is bound).
    device_max_side: u32,
    /// Heights changed since the map of the currents was made, and the frames since.
    currents_due: bool,
    currents_wait: u32,
    cells_made: u64,
    /// The padded game grid in hexes, for the currents.
    grid: [f32; 2],
    frame: WaterFrame,
    inputs: OceanInputs,
    inputs_revision: u64,
    /// `TerrainWaterQuality`: 0 low, 1 medium, 2 high.
    quality: usize,
    reflection_level: i64,
    /// The map of the currents is stale (new heights, or its target was made again).
    currents_stale: bool,
    currents_generation: Option<u64>,
    bound: Option<(u64, u64)>,
    drawn: std::cell::Cell<u32>,
}

struct Gpu {
    ocean: Ocean,
    currents: Currents,
}

impl Default for Water {
    fn default() -> Self {
        Self {
            gpu: None,
            data: None,
            map: None,
            terrain_frame: None,
            height: None,
            height_target: None,
            height_pending: Default::default(),
            height_upload_all: true,
            height_unbuilt: Default::default(),
            shelf_radius: SHELF_RADIUS,
            building: None,
            build_late: Default::default(),
            height_dirty: None,
            device_max_side: u32::MAX,
            currents_due: false,
            currents_wait: 0,
            cells_made: 0,
            grid: [0.0; 2],
            frame: WaterFrame::default(),
            inputs: OceanInputs::default(),
            inputs_revision: 0,
            quality: 2,
            reflection_level: 1,
            currents_stale: true,
            currents_generation: None,
            bound: None,
            drawn: std::cell::Cell::new(0),
        }
    }
}

impl Water {
    /// Takes the device (once, after it exists).
    pub(crate) fn bind(&mut self, env: &B1Env) {
        self.bind_device(&env.device, &env.queue, env.samples);
    }

    /// The same with the parts of the device alone.
    pub fn bind_device(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, samples: u32) {
        self.gpu = Some(Gpu {
            ocean: Ocean::new(device, queue, samples),
            currents: Currents::new(device),
        });
        let before = self.height_max_side();
        self.device_max_side = device.limits().max_texture_dimension_2d;
        if self.height_max_side() != before {
            self.height_target = None;
        }
        self.height_upload_all = true;
        self.bound = None;
    }

    /// The textures and settings of the pack.
    pub fn set_data(&mut self, data: Arc<WaterData>) {
        let radius = shelf_radius(&data);
        if radius != self.shelf_radius {
            // The heights are made again with the other blur: the next `set_terrain` starts over.
            self.shelf_radius = radius;
            self.height_target = None;
        }
        self.data = Some(data);
        self.bound = None;
    }

    /// The longest side of the height texture for the quality and the device ([`height_max_side`]).
    pub fn height_max_side(&self) -> u32 {
        height_max_side(self.quality, self.device_max_side)
    }

    /// Texels of blur of the shelf the heights are made with.
    pub fn shelf_radius(&self) -> u32 {
        self.shelf_radius
    }

    /// The pack and the settings, for the waves and the ice.
    pub(crate) fn data(&self) -> Option<&Arc<WaterData>> {
        self.data.as_ref()
    }

    /// The map: its frame and its heights. The heights are made again only where the revision of
    /// the field moved. `grid` is the padded game grid in hexes (`GridSize` of the currents).
    ///
    /// `dirty` are the cells of the field that changed since [`Self::height_target`] (`None`: the
    /// map is another one, or nothing is known); only they are made again, a few a frame
    /// ([`HEIGHT_CELLS_PER_FRAME`]), and the old depth stays on the texture until they are.
    pub fn set_terrain(
        &mut self,
        sampler: &TerrainSampler,
        grid: [f32; 2],
        dirty: Option<&[(i32, i32)]>,
        unbuilt: &dyn Fn() -> Vec<(i32, i32)>,
    ) {
        let frame = sampler.frame();
        let map = OceanMap {
            half_world: frame.half_world(),
            ocean_height: OCEAN_HEIGHT,
            wrap_x: frame.wrap_x(),
        };
        self.terrain_frame = Some(*frame);
        if self.map != Some(map) || self.grid != grid {
            self.map = Some(map);
            self.grid = grid;
            self.currents_stale = true;
        }
        let revision = sampler.height_revision();
        let raw = |u: i32, v: i32| f32::from(sampler.raw_height(u, v));
        // The map covers the whole world, the rows north of our field too ([`height_rows`]).
        let (fw, fh_field) = (sampler.field().width(), sampler.field().height());
        let fh = height_rows(fh_field);
        let mut first = Vec::new();
        if self.height_target != Some(revision) {
            let size = self
                .building
                .as_ref()
                .map(HeightBuild::field_size)
                .or_else(|| self.height.as_ref().map(HeightState::field_size));
            let same_size = size == Some((fw, fh));
            match dirty {
                Some(cells) if same_size => {
                    for &cell in cells {
                        if self.building.is_some() {
                            // The build is still reading the field: the cell is looked at when it
                            // is done.
                            self.build_late.insert(cell);
                        } else if self.height_unbuilt.remove(&cell) {
                            // The heights of a cell whose field was not built yet were made from
                            // nothing (below the water): they are made again at once, or the sea
                            // draws over the land until the budget reaches them.
                            first.push(cell);
                        } else {
                            self.height_pending.insert(cell);
                        }
                    }
                }
                _ => {
                    // The old depth is drawn while the new is made only if every cell of it came
                    // from a built field; else no sea is drawn until the new heights are there.
                    let keep = same_size
                        && self.height.is_some()
                        && (self.building.is_some() || self.height_unbuilt.is_empty());
                    if !keep {
                        self.height = None;
                        self.bound = None;
                    }
                    self.height_unbuilt = unbuilt().into_iter().collect();
                    self.building = Some(HeightBuild::new(
                        fw,
                        fh,
                        self.height_max_side(),
                        self.shelf_radius,
                    ));
                    self.build_late.clear();
                    self.height_pending.clear();
                    self.height_dirty = None;
                }
            }
            self.height_target = Some(revision);
        }
        // The first build of a map is made over frames, a budget of work a frame.
        let done = self
            .building
            .as_mut()
            .is_some_and(|b| b.step(HEIGHT_BUILD_UNITS_PER_FRAME, &raw));
        if done && let Some(build) = self.building.take() {
            self.height = Some(build.finish());
            self.height_dirty = None;
            self.height_upload_all = true;
            self.currents_stale = true;
            for cell in std::mem::take(&mut self.build_late) {
                if self.height_unbuilt.remove(&cell) {
                    first.push(cell);
                } else {
                    self.height_pending.insert(cell);
                }
            }
        } // A few cells of the field a frame; the rest wait (the depth on the texture is the old one).
        if self.building.is_none()
            && let Some(state) = &mut self.height
        {
            let mut made = false;
            let mut remake = |(cx, cy): (i32, i32)| {
                let rect = CellRect::cell(cx, cy).expand(HEIGHT_CELL_MARGIN);
                let (u0, v0) = (rect.u0.max(0) as u32, rect.v0.max(0) as u32);
                let (u1, v1) = (
                    (rect.u1.max(0) as u32).min(fw),
                    (rect.v1.max(0) as u32).min(fh_field),
                );
                if u1 <= u0 || v1 <= v0 {
                    return;
                }
                let rect = state.update((u0, v0, u1, v1), &raw);
                if !self.height_upload_all {
                    HeightState::merge(&mut self.height_dirty, rect);
                }
                self.cells_made += 1;
                made = true;
            };
            // The cells that got their field now, all of them; then a few of the edited ones.
            first.into_iter().for_each(&mut remake);
            for _ in 0..HEIGHT_CELLS_PER_FRAME {
                let Some(cell) = self.height_pending.pop_first() else {
                    break;
                };
                remake(cell);
            }
            self.currents_due |= made;
            self.currents_wait += u32::from(self.currents_due);
            // The map of the currents follows the depth once it has settled, or every so often.
            if self.currents_due && (self.height_pending.is_empty() || self.currents_wait >= 60) {
                self.currents_stale = true;
                self.currents_due = false;
                self.currents_wait = 0;
            }
        }
    }

    /// The revision of the field the heights were last given (made, or being made).
    pub fn height_target(&self) -> Option<u64> {
        self.height_target
    }

    /// Cells of the field still to make, and made so far (`water.height_cells`).
    pub fn height_work(&self) -> (usize, u64) {
        (self.height_pending.len(), self.cells_made)
    }

    /// The ramp of the distance fog and the lookup of the fog of war (`LightingEnv::fog_ramp`,
    /// `terrain_color_key(2, class)`).
    pub fn set_lighting(
        &mut self,
        fog_ramp: Option<wgpu::TextureView>,
        lut: Option<wgpu::TextureView>,
    ) {
        if self.inputs.fog_ramp == fog_ramp && self.inputs.lut == lut {
            return;
        }
        self.inputs.fog_ramp = fog_ramp;
        self.inputs.lut = lut;
        self.inputs_revision += 1;
    }

    /// The rivers' colour key (`LightingEnv::rivers_color_key`).
    pub fn set_river_key(&mut self, key: Option<wgpu::TextureView>) {
        if self.inputs.river_key == key {
            return;
        }
        self.inputs.river_key = key;
        self.inputs_revision += 1;
    }

    /// The fog of war's mask the surface reads (`set_maps`); `None`: everything visible.
    #[cfg(test)]
    pub(crate) fn fow_mask_input(&self) -> Option<&wgpu::TextureView> {
        self.inputs.fow.as_ref()
    }

    /// The maps other subsystems make: the reflection target, the fog of war's mask, the overlay
    /// maps and the sun's shadow map. `None` stands for black, everything visible, transparent
    /// and no shadow. The views are the pool's; a frame that gives the same ones changes nothing.
    pub fn set_maps(
        &mut self,
        reflection: Option<wgpu::TextureView>,
        fow: Option<wgpu::TextureView>,
        overlay: Option<wgpu::TextureView>,
        shadow: Option<wgpu::TextureView>,
    ) {
        if self.inputs.reflection == reflection
            && self.inputs.fow == fow
            && self.inputs.overlay == overlay
            && self.inputs.shadow == shadow
        {
            return;
        }
        self.inputs.reflection = reflection;
        self.inputs.fow = fow;
        self.inputs.overlay = overlay;
        self.inputs.shadow = shadow;
        self.inputs_revision += 1;
    }

    /// The camera of the frame.
    pub fn set_frame(&mut self, frame: WaterFrame) {
        self.frame = frame;
    }

    pub fn set_quality(&mut self, quality: i64) {
        let before = self.height_max_side();
        self.quality = quality.clamp(0, 2) as usize;
        self.currents_stale = true;
        if self.height_max_side() != before {
            // The depth of the other preset is made again at its own size (the next `set_terrain`).
            self.height_target = None;
        }
    }

    /// `ReflectionLevel`, before the first `on_settings`.
    pub fn set_reflection_level(&mut self, level: i64) {
        self.reflection_level = level;
    }

    pub fn quality(&self) -> usize {
        self.quality
    }

    /// The reflection target is 1024 x 1024 only at `ReflectionLevel` 1 and quality 1 or more
    /// (RW-11); the camera of the reflection is `RenderSystem`'s business (R3.7).
    pub fn wants_reflection(&self) -> bool {
        self.reflection_level == 1 && self.quality >= 1
    }

    /// Everything the surface reads is in and the frame draws it.
    pub fn ready(&self) -> bool {
        self.bound.is_some()
    }

    /// What the rivers share with the surface (textures, inputs, camera, map), once the surface is
    /// [`ready`](Self::ready); `River::feed` takes it.
    pub(crate) fn river_feed(&self) -> Option<RiverFeed> {
        if !self.ready() {
            return None;
        }
        let ocean = &self.gpu.as_ref()?.ocean;
        let views = ocean.shared_views()?;
        let (river_color, river_blend) = ocean.river_views();
        Some(RiverFeed {
            map: self.map?,
            terrain: self.terrain_frame?,
            frame: self.frame,
            quality: self.quality,
            textures: RiverTextures::new(views, &self.inputs)?.with_own(river_color, river_blend),
        })
    }

    /// The pack, the heights and the lighting are in, so the subsystem can make its group in the
    /// next `WaterPrep`: the renderer puts it in the plan in place of B1's water (it is
    /// [`ready`](Self::ready) after that pass).
    pub fn eligible(&self) -> bool {
        self.gpu.is_some()
            && self.data_ready()
            && self.inputs.fog_ramp.is_some()
            && self.inputs.lut.is_some()
    }

    /// `ReflectionColor` and its depth for the level (R3.7); the pass that fills them is another
    /// subsystem's, until it runs the water reads black from the target.
    fn declare_reflection(&self, cx: &mut InitContext<'_>) {
        if let Err(e) = crate::reflection::declare_targets(cx.targets, self.reflection_level) {
            tracing::warn!(error = %e, "water: reflection targets not declared");
        }
    }

    fn data_ready(&self) -> bool {
        self.data.as_ref().is_some_and(|d| d.ocean_ready())
            && self.map.is_some()
            && self.height.is_some()
    }

    /// What the last frame drew, for the tests.
    pub fn drawn(&self) -> u32 {
        self.drawn.get()
    }
}

impl RenderSystem for Water {
    fn id(&self) -> SystemId {
        SystemId::Water
    }

    /// The cells of the field whose heights wait to be made again: a frame captured before they are
    /// has the old depth on the texture.
    fn pending_work(&self) -> u32 {
        self.height_pending.len() as u32
            + self
                .building
                .as_ref()
                .map_or(0, |b| b.remaining().div_ceil(1000).max(1) as u32)
    }

    fn passes(&self) -> &'static [PassSlot] {
        &[
            PassSlot::Pass(PassId::WaterPrep),
            PassSlot::Main(MainSlot::Terrain),
        ]
    }

    fn init(&mut self, cx: &mut InitContext<'_>) {
        // The map of the currents only exists above quality 0 (RW-11).
        if let Err(e) = currents::declare_target(cx.targets, self.quality > 0) {
            tracing::warn!(error = %e, "water: currents target not declared");
        }
        self.declare_reflection(cx);
    }

    fn on_settings(
        &mut self,
        settings: &RenderSettings,
        changed: &KeySet,
        cx: &mut InitContext<'_>,
    ) {
        if changed.contains(WATER_QUALITY) {
            self.set_quality(settings.get(WATER_QUALITY).unwrap_or(2));
            if let Err(e) = currents::declare_target(cx.targets, self.quality > 0) {
                tracing::warn!(error = %e, "water: currents target not declared");
            }
            self.bound = None;
        }
        if changed.contains(REFLECTION_LEVEL) {
            self.reflection_level = settings.get(REFLECTION_LEVEL).unwrap_or(1);
            self.declare_reflection(cx);
        }
    }

    fn prepare(&mut self, _cx: &mut PrepareContext<'_>) {
        self.drawn.set(0);
        let quality = self.quality;
        let (Some(gpu), Some(data)) = (&mut self.gpu, &self.data) else {
            return;
        };
        let has_textures = gpu.ocean.set_data(data);
        if let Some(state) = &self.height {
            if self.height_upload_all || !gpu.ocean.has_height() {
                gpu.ocean.set_height(&state.map);
                self.height_upload_all = false;
                self.height_dirty = None;
                self.bound = None;
            } else if let Some(rect) = self.height_dirty.take()
                && !gpu.ocean.update_height(&state.map, rect)
            {
                gpu.ocean.set_height(&state.map);
                self.bound = None;
            }
        }
        if !has_textures || !gpu.ocean.has_height() {
            return;
        }
        if let Some(map) = &self.map {
            gpu.ocean.prepare(map, &self.frame);
            gpu.ocean.ensure_pipeline(quality);
        }
    }

    fn record_offscreen(&mut self, pass: PassId, cx: &mut EncoderContext<'_>) {
        if pass != PassId::WaterPrep || !self.data_ready() {
            return;
        }
        let quality = self.quality;
        let Some(gpu) = &mut self.gpu else { return };
        let target = if quality > 0 {
            cx.targets.get(CURRENTS)
        } else {
            None
        };
        let generation = target.map(|t| t.generation);
        if generation != self.currents_generation {
            self.currents_generation = generation;
            self.currents_stale = true;
        }
        if let Some(target) = target {
            // RW-11: the map of the currents is made once, not every frame.
            if self.currents_stale
                && let Some(height) = gpu.ocean.height_view()
            {
                let n = gpu.currents.record(
                    cx.device,
                    cx.queue,
                    cx.encoder,
                    height,
                    &target.view,
                    self.grid,
                );
                cx.stats.record_draw(n);
                self.currents_stale = false;
            }
        }
        let key = (generation.unwrap_or(0), self.inputs_revision);
        if self.bound != Some(key) || !gpu.ocean.is_bound() {
            let mut inputs = self.inputs.clone();
            inputs.currents = target.map(|t| t.view.clone());
            self.bound = if gpu.ocean.bind(&inputs) {
                Some(key)
            } else {
                None
            };
        }
    }

    fn record(&self, slot: PassSlot, cx: &mut DrawContext<'_, '_>) {
        if slot != PassSlot::Main(MainSlot::Terrain) || !self.ready() {
            return;
        }
        let Some(gpu) = &self.gpu else { return };
        let n = gpu.ocean.draw(cx.pass, self.quality, cx.copy_shift);
        if n > 0 {
            cx.stats.record_draw(n);
            self.drawn.set(self.drawn.get() + 1);
        }
    }

    fn record_mask(&self, slot: PassSlot, cx: &mut MaskContext<'_, '_>) {
        if slot != PassSlot::Main(MainSlot::Terrain) || !self.ready() {
            return;
        }
        if let Some(gpu) = &self.gpu {
            gpu.ocean.draw_mask(cx.pass, cx.copy_shift);
        }
    }

    fn counters(&self) -> Vec<(&'static str, u64)> {
        vec![
            ("water.drawn", u64::from(self.drawn.get())),
            ("water.height_cells_made", self.cells_made),
            (
                "water.height_build_pending",
                self.building.as_ref().map_or(0, |b| b.remaining() as u64),
            ),
            (
                "water.height_cells_pending",
                self.height_pending.len() as u64,
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_water_level_is_the_ocean_height_of_the_capture() {
        assert!((water_level() - 10.12).abs() < 0.01);
    }

    /// The shore of the original is the height field of the coast meshes alone: with the styles of the
    /// coast the heights get no blur on top (the soft green band of the report), without them the
    /// blur stands in for the shelf; a change of the settings makes the heights again.
    #[test]
    fn the_heights_are_not_blurred_where_the_coast_meshes_make_the_shore() {
        use crate::world::terrain::sampler::{TerrainFrame, TerrainSampler};
        let mut plain = WaterData::new();
        plain.set_settings(&asset_format::WorldSettings::default());
        assert_eq!(shelf_radius(&plain), SHELF_RADIUS);
        let styled = {
            let mut d = WaterData::new();
            d.set_settings(&asset_format::WorldSettings {
                coast: crate::world::water::coast::styles::synthetic_settings(false),
                ..Default::default()
            });
            d
        };
        assert_eq!(shelf_radius(&styled), 0);

        // A step from the land to the water: the byte at the foot of the step is the water's own
        // with no blur, and the blur of the stand-in softens it.
        let mut s = TerrainSampler::new(TerrainFrame::new(2, 1, false));
        for v in 0..64 {
            for u in 0..128 {
                s.field_mut()
                    .set_height(u, v, if u < 64 { 15000 } else { 6000 });
            }
        }
        let made = |data: WaterData| {
            let mut w = Water::default();
            w.set_data(Arc::new(data));
            while w.height.is_none() {
                w.set_terrain(&s, [2.0, 1.0], None, &Vec::new);
            }
            w.height.unwrap().map.bytes
        };
        let sharp = made(styled);
        let soft = made(plain);
        // The first rows of the map are the sea past the north edge of the field (128 wide, one
        // field texel to a map texel).
        let north = ORIG_EXTRA_ROWS as usize * 128;
        let (water_byte, land_byte) = (sharp[sharp.len() - 1], sharp[north]);
        assert!(land_byte > water_byte + 20);
        // Beside the step (two map texels, within the blur) the sharp map is still the land's or the
        // water's byte; the blurred one is in between.
        let row = &sharp[north..north + 128];
        let blurred = &soft[north..north + 128];
        let edge = row.iter().position(|&b| b == water_byte).unwrap();
        assert!(row[edge.saturating_sub(1)] >= land_byte.saturating_sub(1));
        assert!(blurred[edge.saturating_sub(1)] < land_byte.saturating_sub(1));
    }

    /// The field builds cell by cell: the cell that gets its heights after the first map was made
    /// has them in the map the frame it appears, not 12 cells a frame later (the sea drew over the
    /// land meanwhile), and the map is the one a full build of the same field makes.
    #[test]
    fn a_cell_that_builds_after_the_first_map_is_made_again_at_once() {
        use crate::world::terrain::sampler::{TerrainFrame, TerrainSampler};
        let mut s = TerrainSampler::new(TerrainFrame::new(14, 1, false));
        // Fourteen cells of 64 x 64 texels, more than a frame remakes; only the west one is built.
        for v in 0..64 {
            for u in 0..64 {
                s.field_mut().set_height(u, v, 20000);
            }
        }
        let mut w = Water::default();
        w.set_terrain(&s, [14.0, 1.0], None, &|| {
            (1..14).map(|cx| (cx, 0)).collect()
        });
        while w.height.is_none() {
            w.set_terrain(&s, [14.0, 1.0], None, &Vec::new);
        }
        let low = w.height.as_ref().unwrap().map.bytes.clone();
        assert!(low[low.len() - 1] < 40, "the east is below the water");
        // The rest builds: 13 cells, one more than the budget of a frame.
        for v in 0..64 {
            for u in 64..896 {
                s.field_mut().set_height(u, v, 20000);
            }
        }
        let built: Vec<(i32, i32)> = (1..14).map(|cx| (cx, 0)).collect();
        w.set_terrain(&s, [14.0, 1.0], Some(&built), &Vec::new);
        assert_eq!(w.height_work().0, 0);
        let (fw, fh) = (s.field().width(), height_rows(s.field().height()));
        let raw = |u: i32, v: i32| f32::from(s.raw_height(u, v));
        let full = HeightState::build(fw, fh, w.height_max_side(), SHELF_RADIUS, &raw);
        assert_eq!(w.height.as_ref().unwrap().map.bytes, full.map.bytes);
    }

    /// Lake Victoria's water was drawn north of its bowl (feedback of 2026-11-02): the depth map held
    /// only the field, the quad and the shader stretched it over the whole world, which is
    /// `ORIG_EXTRA_ROWS` longer, so a pit of the field was drawn up to a row of hexes north of itself.
    /// The water of a pit lands on the pit, at the south, the middle and the north of the map.
    #[test]
    fn the_water_of_a_pit_is_drawn_where_the_pit_is() {
        use crate::world::terrain::sampler::{TerrainFrame, TerrainSampler};
        use glam::Vec2;
        let frame = TerrainFrame::new(4, 3, false);
        let mut s = TerrainSampler::new(frame);
        let (fw, fh) = (s.field().width() as i32, s.field().height() as i32);
        for v in 0..fh {
            for u in 0..fw {
                s.field_mut().set_height(u, v, 30000);
            }
        }
        let half = frame.half_world();
        let scale = frame.terrain_scale();
        let size = half * 2.0;
        for (cu, cv) in [(100i32, 20i32), (128, 96), (60, 170)] {
            for v in cv - 6..=cv + 6 {
                for u in cu - 6..=cu + 6 {
                    s.field_mut().set_height(u, v, 0);
                }
            }
            let mut w = Water::default();
            w.set_terrain(&s, [4.0, 3.0], None, &Vec::new);
            while w.height.is_none() {
                w.set_terrain(&s, [4.0, 3.0], None, &Vec::new);
            }
            let map = &w.height.as_ref().unwrap().map;
            // The rows past the north edge of the field are the deep sea by design.
            let f = w.height.as_ref().unwrap().texels_per_texel();
            let north = ORIG_EXTRA_ROWS / f;
            let (mut sum, mut sy, mut n) = (0.0f32, 0.0f32, 0.0f32);
            for row in north..map.height {
                for col in 0..map.width {
                    if map.bytes[(row * map.width + col) as usize] < 15 {
                        sum += (col as f32 + 0.5) / map.width as f32;
                        sy += (row as f32 + 0.5) / map.height as f32;
                        n += 1.0;
                    }
                }
            }
            assert!(n > 0.0, "the pit has no water");
            // The point of the world the quad draws these texture coordinates at (see `ocean::quad`),
            // against the point of the original's world the pit's texel is at.
            let (tu, tv) = (sum / n, sy / n);
            let drawn = Vec2::new(tu * size.x - half.x, (1.0 - tv) * size.y - half.y);
            let want = Vec2::new(
                (cu as f32 + 0.5) * scale.x - half.x,
                (cv as f32 + 0.5) * scale.y - half.y,
            );
            assert!(
                (drawn - want).abs().max_element() < 3.0,
                "pit at texel ({cu}, {cv}): water at {drawn}, pit at {want}"
            );
            for v in cv - 6..=cv + 6 {
                for u in cu - 6..=cu + 6 {
                    s.field_mut().set_height(u, v, 30000);
                }
            }
        }
    }

    #[test]
    fn a_water_without_a_device_is_not_ready_and_draws_nothing() {
        let w = Water::default();
        assert!(!w.ready());
        assert_eq!(w.quality(), 2);
        assert!(w.wants_reflection());
    }

    /// AC-R8.1 for the water: a preset reaches `on_settings` and changes what the keys say, with no
    /// reload: the quality of the surface, the map of the currents (above quality 0) and the
    /// reflection target (`ReflectionLevel` 1).
    #[test]
    fn the_presets_change_the_quality_the_currents_and_the_reflection() {
        use crate::frame::{InitContext, REFLECTION_COLOR, TargetPool};
        use crate::gpu_test::gpu;
        use crate::settings::Preset;
        let Some(g) = gpu(
            "water presets",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let mut pool = TargetPool::new(false, (64, 64));
        let mut water = Water::default();
        let mut now = RenderSettings::preset(Preset::High);
        water.init(&mut InitContext {
            device: &g.device,
            targets: &mut pool,
        });
        for preset in [
            Preset::High,
            Preset::Low,
            Preset::Medium,
            Preset::Low,
            Preset::High,
        ] {
            let next = RenderSettings::preset(preset);
            let changed = now.changed(&next);
            water.on_settings(
                &next,
                &changed,
                &mut InitContext {
                    device: &g.device,
                    targets: &mut pool,
                },
            );
            now = next;
            let quality = now.get(WATER_QUALITY).unwrap() as usize;
            let reflect = now.get(REFLECTION_LEVEL).unwrap() == 1;
            assert_eq!(water.quality(), quality, "{preset:?}");
            assert_eq!(
                pool.size_of(CURRENTS).is_some(),
                quality > 0,
                "{preset:?}: the currents exist above quality 0"
            );
            assert_eq!(
                pool.size_of(REFLECTION_COLOR).is_some(),
                reflect,
                "{preset:?}: the reflection target"
            );
            assert_eq!(
                water.wants_reflection(),
                reflect && quality >= 1,
                "{preset:?}"
            );
        }
        // Low has neither reflection nor currents; High has both.
        let low = RenderSettings::preset(Preset::Low);
        assert_eq!(low.get(REFLECTION_LEVEL), Some(0));
        assert_eq!(low.get(WATER_QUALITY), Some(0));
    }

    #[test]
    fn the_reflection_needs_the_level_and_quality() {
        let mut w = Water::default();
        w.set_quality(0);
        assert!(!w.wants_reflection());
        w.set_quality(1);
        assert!(w.wants_reflection());
        w.reflection_level = 0;
        assert!(!w.wants_reflection());
    }
}
