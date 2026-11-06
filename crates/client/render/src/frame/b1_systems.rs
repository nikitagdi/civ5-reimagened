//! B1 world drawing as adapter subsystems. Until a `world-*`
//! module moves its part, B1 code draws through a [`RenderSystem`] in the main-pass slot nearest
//! to its meaning; the executor in `renderer.rs` runs them like any other subsystem, once per
//! visible map copy. An adapter takes the place of the stub of the same [`SystemId`] in the frame
//! plan; the module that ports the subsystem deletes the adapter and its facade input.
//!
//! The adapters own no data. `Renderer` keeps B1's buffers and pipelines (the facade inputs
//! `set_terrain`, `set_entities`, ... fill them) and lends them to the adapters for one frame as a
//! [`B1Draw`].
//!
//! Slots used (B1 draws in one pass, so everything on the map sits in `Main.Terrain` in B1's own
//! order; table 4.2 gives the `world-*` slot of each):
//!
//! | adapter | `SystemId` | B1 slot | draws |
//! |---|---|---|---|
//! | `B1Terrain` | `Terrain` | `Main.Terrain` | land and the seabed |
//! | `B1Water` | `Water` | `Main.Terrain` | water surface |
//! | `B1Decal` | `Decal` | `Main.Terrain` (B1 has no bake) | feature decals |
//! | `B1Icons` | `YieldIcons` | `Main.Terrain` (table gives no slot) | resource and improvement icons |
//! | `B1Landmark` | `Landmark` | `Main.Landmark` | roads |
//! | `B1Forest` | `Forest` | `Main.Forest` | trees |
//! | `B1Unit` | `Unit` | `Main.Unit` | units and cities (one batch set), skinned showcase |
//!
//! `B1Unit` also holds the place of the units of `assets-b2` (WOB2) in the plan: it names both slots
//! of the units, and the renderer calls the `Unit` subsystem's own `record` in them after it when
//! those units are on. B1's batch set then holds the cities and the showcase only.
//!
//! `Overlay` (rivers, borders, grid, highlights), `MovementPath` and `FluidFow` (fog state and
//! clouds) have bodies of their own (`world-overlays` WOV0) and are no adapters any more: `Overlay`
//! draws the ground lines in `Main.Terrain` after the adapters of that slot and the highlights in
//! `Main.Landmark` after the roads, and the adapters lend them nothing but the fog-state texture.
//!
//! The copies of a wrapping map: `render-core` calls `record` once per visible copy
//! ([`crate::frame::copy_shifts`]) with `DrawContext::copy_shift`; the adapter binds that copy's
//! object group (B1 shaders still shift the copy through the object matrix, which keeps the
//! texture coordinates of the ground continuous across the seam). Group 0 is B1's own `Globals`.

use crate::perf::Buf;
use crate::perf::PassCount;
use std::cell::Cell;

use super::graph::MainSlot;
use super::mask::{MaskPipelineDesc, MaskShader, class};
use super::system::{
    DrawContext, InitContext, MaskContext, PassSlot, PrepareContext, RenderSystem, SystemId,
};
use crate::blend::BlendPipeline;
use crate::decal::DecalPipeline;
use crate::model::ModelRenderer;
use crate::static_model::{BatchSet, StaticRenderer};
use crate::terrain_pipeline::{TerrainMaterial, TerrainPipeline};
use crate::water_pipeline::WaterPipeline;
use crate::world::terrain::{ClassRanges, TerrainM1};

/// The map: one vertex set drawn once, or three times shifted by the world width on a wrapping map
pub(crate) struct GpuTerrain {
    pub vertices: Buf,
    pub indices: Buf,
    pub index_count: u32,
    /// Indices of the land and skirts, which come first; the water indices follow.
    pub land_index_count: u32,
    /// The triangles of the land and of the water, each region sorted by continent style: the
    /// ground of the milestone M1 is lit per style (`world::terrain::m1`).
    pub land_classes: ClassRanges,
    pub water_classes: ClassRanges,
    /// One object uniform and bind group per copy.
    pub copies: Vec<wgpu::BindGroup>,
    /// X shift of each copy, in the same order; `width` is the map's world width (0 without wrap).
    pub shifts: Vec<f32>,
    pub width: f32,
}

/// Decal geometry over the map (decal pipeline), drawn once per terrain copy.
pub(crate) struct GpuDecals {
    pub vertices: Buf,
    pub indices: Buf,
    pub index_count: u32,
}

/// Roads over the map (blend pipeline), drawn once per terrain copy.
pub(crate) struct GpuLines {
    pub vertices: Buf,
    pub indices: Buf,
    pub index_count: u32,
}

/// The map and the fog-state texture its shaders read (group 2, `FluidFow`'s); present once
/// `set_terrain` has been called.
pub(crate) struct MapDraw<'a> {
    pub terrain: &'a GpuTerrain,
    pub fog: &'a wgpu::BindGroup,
}

/// Class mask pipelines of B1's ground (R0.17): the vertex stage of `terrain.wgsl`, one pipeline
/// for the land and one for the water surface. B1 draws everything in one slot, so these are all
/// the classes it can tell apart; the rest come with the `world-*` modules.
pub(crate) struct B1Masks {
    land: wgpu::RenderPipeline,
    water: wgpu::RenderPipeline,
}

impl B1Masks {
    /// Groups: 0 globals, 1 object, as in the terrain pipeline.
    pub fn new(
        device: &wgpu::Device,
        shader: &MaskShader,
        globals_layout: &wgpu::BindGroupLayout,
        object_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let vertex = device.create_shader_module(wgpu::include_wgsl!("../terrain.wgsl"));
        let attributes = wgpu::vertex_attr_array![
            0 => Float32x3, 1 => Float32x3, 2 => Uint32, 3 => Uint32
        ];
        let buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<crate::terrain::texture_array::TerrainGpuVertex>()
                as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &attributes,
        })];
        let layouts = [Some(globals_layout), Some(object_layout)];
        let make = |label, class| {
            shader.pipeline(
                device,
                &MaskPipelineDesc {
                    label,
                    vertex: &vertex,
                    vertex_entry: "vs_main",
                    vertex_buffers: &buffers,
                    bind_group_layouts: &layouts,
                    cull: Some(wgpu::Face::Back),
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    class,
                },
            )
        };
        Self {
            land: make("b1 mask land", class::LAND),
            water: make("b1 mask water", class::WATER),
        }
    }
}

/// What B1 draws, lent by `Renderer` for one frame.
pub(crate) struct B1Draw<'a> {
    pub masks: &'a B1Masks,
    pub map: Option<MapDraw<'a>>,
    pub terrain_pipeline: &'a TerrainPipeline,
    pub terrain_material: &'a TerrainMaterial,
    /// The light of the original on B1's ground (WT-0); B1's pipeline draws while it is not on.
    pub terrain_m1: &'a TerrainM1,
    pub water: &'a WaterPipeline,
    pub decal_pipeline: &'a DecalPipeline,
    pub blend: &'a BlendPipeline,
    /// Feature decals and their material.
    pub feature_decals: Option<(&'a GpuDecals, &'a wgpu::BindGroup)>,
    /// Resource and improvement icons and their material.
    pub icons: Option<(&'a GpuDecals, &'a wgpu::BindGroup)>,
    pub lines: Option<&'a GpuLines>,
    pub statics: &'a StaticRenderer,
    pub models: &'a ModelRenderer,
}

impl<'a> B1Draw<'a> {
    /// The map and the object group of the copy shifted by `shift`.
    fn copy(&self, shift: f32) -> Option<(&MapDraw<'a>, &'a wgpu::BindGroup)> {
        let map = self.map.as_ref()?;
        let at = map.terrain.shifts.iter().position(|&s| s == shift)?;
        Some((map, &map.terrain.copies[at]))
    }

    /// The cities in the reflection pass, for each map copy of `shifts`: B1's batch set of the
    /// entities through the building shader's reflection pipeline.
    pub(crate) fn draw_reflection(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        shifts: &[f32],
        stats: &mut crate::renderer::FrameStats,
    ) {
        for &shift in shifts {
            if let Some((map, copy)) = self.copy(shift) {
                self.statics
                    .draw_reflection(BatchSet::Entities, pass, copy, map.fog, stats);
            }
        }
    }
}

fn draw_decals(
    d: &B1Draw<'_>,
    decals: (&GpuDecals, &wgpu::BindGroup),
    cx: &mut DrawContext<'_, '_>,
) {
    let Some((map, copy)) = d.copy(cx.copy_shift) else {
        return;
    };
    let (decals, material) = decals;
    let pass = &mut *cx.pass;
    pass.set_pipeline_c(&d.decal_pipeline.pipeline);
    pass.set_bind_group_c(0, cx.view_group, &[]);
    pass.set_bind_group_c(2, map.fog, &[]);
    pass.set_bind_group_c(3, material, &[]);
    pass.set_vertex_buffer(0, decals.vertices.slice(..));
    pass.set_index_buffer(decals.indices.slice(..), wgpu::IndexFormat::Uint32);
    pass.set_bind_group_c(1, copy, &[]);
    pass.draw_indexed_c(0..decals.index_count, 0, 0..1);
    cx.stats.record_draw(decals.index_count);
}

fn pass_globals(cx: &mut DrawContext<'_, '_>) {
    cx.pass.set_bind_group_c(0, cx.view_group, &[]);
}

/// For an adapter that has no class mask.
fn no_mask<T>(_: &T, _: &mut MaskContext<'_, '_>) {}

macro_rules! adapter {
    ($name:ident, $id:expr, $slots:expr) => {
        adapter!($name, $id, $slots, no_mask);
    };
    ($name:ident, $id:expr, $slots:expr, $mask:expr) => {
        pub(crate) struct $name<'a> {
            d: &'a B1Draw<'a>,
            /// For what is not drawn per copy: set by the first copy of the frame.
            once: Cell<bool>,
        }

        impl<'a> $name<'a> {
            fn new(d: &'a B1Draw<'a>) -> Self {
                Self {
                    d,
                    once: Cell::new(false),
                }
            }

            /// True for the first call of the frame, false after.
            #[allow(dead_code)]
            fn first(&self) -> bool {
                !self.once.replace(true)
            }
        }

        impl RenderSystem for $name<'_> {
            fn id(&self) -> SystemId {
                $id
            }

            fn passes(&self) -> &'static [PassSlot] {
                $slots
            }

            fn init(&mut self, _cx: &mut InitContext<'_>) {}

            fn prepare(&mut self, _cx: &mut PrepareContext<'_>) {}

            fn record(&self, _slot: PassSlot, cx: &mut DrawContext<'_, '_>) {
                self.draw(cx);
            }

            fn record_mask(&self, _slot: PassSlot, cx: &mut MaskContext<'_, '_>) {
                ($mask)(self, cx);
            }
        }
    };
}

const TERRAIN_SLOT: &[PassSlot] = &[PassSlot::Main(MainSlot::Terrain)];

adapter!(
    B1Terrain,
    SystemId::Terrain,
    TERRAIN_SLOT,
    B1Terrain::draw_mask
);
adapter!(B1Water, SystemId::Water, TERRAIN_SLOT, B1Water::draw_mask);
adapter!(B1Decal, SystemId::Decal, TERRAIN_SLOT);
adapter!(B1Icons, SystemId::YieldIcons, TERRAIN_SLOT);
// Roads: after the rivers, borders and grid of `Overlay` (`Main.Terrain`), before its highlights
// (`Main.Landmark`, after the adapters of the slot).
adapter!(
    B1Landmark,
    SystemId::Landmark,
    &[PassSlot::Main(MainSlot::Landmark)]
);
adapter!(
    B1Forest,
    SystemId::Forest,
    &[PassSlot::Main(MainSlot::Forest)]
);

/// The units: B1's batch set in `Main.Unit` (cities, markers, the showcase). `Main.UnitZFail` is
/// named for the units of `assets-b2`, which the renderer records after this adapter.
pub(crate) struct B1Unit<'a> {
    d: &'a B1Draw<'a>,
    once: Cell<bool>,
}

impl<'a> B1Unit<'a> {
    fn new(d: &'a B1Draw<'a>) -> Self {
        Self {
            d,
            once: Cell::new(false),
        }
    }

    /// True for the first call of the frame, false after.
    fn first(&self) -> bool {
        !self.once.replace(true)
    }
}

impl RenderSystem for B1Unit<'_> {
    fn id(&self) -> SystemId {
        SystemId::Unit
    }

    fn passes(&self) -> &'static [PassSlot] {
        &[
            PassSlot::Main(MainSlot::UnitZFail),
            PassSlot::Main(MainSlot::Unit),
        ]
    }

    fn init(&mut self, _cx: &mut InitContext<'_>) {}

    fn prepare(&mut self, _cx: &mut PrepareContext<'_>) {}

    fn record(&self, slot: PassSlot, cx: &mut DrawContext<'_, '_>) {
        if slot == PassSlot::Main(MainSlot::Unit) {
            self.draw(cx);
        }
    }

    fn record_mask(&self, _slot: PassSlot, _cx: &mut MaskContext<'_, '_>) {}
}

impl B1Terrain<'_> {
    fn draw(&self, cx: &mut DrawContext<'_, '_>) {
        let d = self.d;
        let Some((map, copy)) = d.copy(cx.copy_shift) else {
            return;
        };
        let t = map.terrain;
        let pass = &mut *cx.pass;
        if let (true, Some(pipeline)) = (d.terrain_m1.on(), d.terrain_m1.pipeline()) {
            // The light of the original: the groups 1..3 are B1's, group 0 is the style's.
            pass.set_pipeline_c(pipeline);
            pass.set_bind_group_c(1, copy, &[]);
            pass.set_bind_group_c(2, map.fog, &[]);
            pass.set_bind_group_c(3, &d.terrain_material.bind_group, &[]);
            pass.set_vertex_buffer(0, t.vertices.slice(..));
            pass.set_index_buffer(t.indices.slice(..), wgpu::IndexFormat::Uint32);
            for region in [&t.land_classes, &t.water_classes] {
                for (style, range) in region.ranges.iter().enumerate() {
                    let Some(group) = d.terrain_m1.group(style) else {
                        continue;
                    };
                    if range.is_empty() {
                        continue;
                    }
                    pass.set_bind_group_c(0, group, &[]);
                    pass.draw_indexed_c(range.clone(), 0, 0..1);
                    cx.stats.record_draw(range.end - range.start);
                }
            }
            return;
        }
        pass.set_pipeline_c(&d.terrain_pipeline.pipeline);
        pass.set_bind_group_c(0, cx.view_group, &[]);
        pass.set_bind_group_c(2, map.fog, &[]);
        pass.set_bind_group_c(3, &d.terrain_material.bind_group, &[]);
        pass.set_vertex_buffer(0, t.vertices.slice(..));
        pass.set_index_buffer(t.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.set_bind_group_c(1, copy, &[]);
        pass.draw_indexed_c(0..t.index_count, 0, 0..1);
        cx.stats.record_draw(t.index_count);
    }
}

impl B1Terrain<'_> {
    /// The land and the seabed: indices `0..land_index_count`.
    fn draw_mask(&self, cx: &mut MaskContext<'_, '_>) {
        let d = self.d;
        let Some((map, copy)) = d.copy(cx.copy_shift) else {
            return;
        };
        let t = map.terrain;
        let pass = &mut *cx.pass;
        pass.set_pipeline_c(&d.masks.land);
        pass.set_bind_group_c(0, cx.view_group, &[]);
        pass.set_bind_group_c(1, copy, &[]);
        pass.set_vertex_buffer(0, t.vertices.slice(..));
        pass.set_index_buffer(t.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed_c(0..t.land_index_count, 0, 0..1);
    }
}

impl B1Water<'_> {
    /// The water surface: the indices after the land.
    fn draw_mask(&self, cx: &mut MaskContext<'_, '_>) {
        let d = self.d;
        let Some((map, copy)) = d.copy(cx.copy_shift) else {
            return;
        };
        let t = map.terrain;
        if t.index_count <= t.land_index_count {
            return;
        }
        let pass = &mut *cx.pass;
        pass.set_pipeline_c(&d.masks.water);
        pass.set_bind_group_c(0, cx.view_group, &[]);
        pass.set_bind_group_c(1, copy, &[]);
        pass.set_vertex_buffer(0, t.vertices.slice(..));
        pass.set_index_buffer(t.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed_c(t.land_index_count..t.index_count, 0, 0..1);
    }

    fn draw(&self, cx: &mut DrawContext<'_, '_>) {
        let d = self.d;
        let Some((map, copy)) = d.copy(cx.copy_shift) else {
            return;
        };
        let t = map.terrain;
        // The water surface over the seabed: the water indices come after the land.
        if t.index_count <= t.land_index_count {
            return;
        }
        let pass = &mut *cx.pass;
        pass.set_pipeline_c(&d.water.pipeline);
        pass.set_bind_group_c(0, cx.view_group, &[]);
        pass.set_bind_group_c(2, map.fog, &[]);
        pass.set_bind_group_c(3, &d.water.bind_group, &[]);
        pass.set_vertex_buffer(0, t.vertices.slice(..));
        pass.set_index_buffer(t.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.set_bind_group_c(1, copy, &[]);
        pass.draw_indexed_c(t.land_index_count..t.index_count, 0, 0..1);
        cx.stats.record_draw(t.index_count - t.land_index_count);
    }
}

impl B1Decal<'_> {
    fn draw(&self, cx: &mut DrawContext<'_, '_>) {
        if let Some(decals) = self.d.feature_decals {
            draw_decals(self.d, decals, cx);
        }
    }
}

impl B1Icons<'_> {
    fn draw(&self, cx: &mut DrawContext<'_, '_>) {
        if let Some(icons) = self.d.icons {
            draw_decals(self.d, icons, cx);
        }
    }
}

impl B1Landmark<'_> {
    fn draw(&self, cx: &mut DrawContext<'_, '_>) {
        let d = self.d;
        let (Some(l), Some((map, copy))) = (d.lines, d.copy(cx.copy_shift)) else {
            return;
        };
        // Roads over rivers and borders, under the highlights and the units.
        let pass = &mut *cx.pass;
        pass.set_pipeline_c(&d.blend.pipeline);
        pass.set_bind_group_c(0, cx.view_group, &[]);
        pass.set_bind_group_c(2, map.fog, &[]);
        pass.set_vertex_buffer(0, l.vertices.slice(..));
        pass.set_index_buffer(l.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.set_bind_group_c(1, copy, &[]);
        pass.draw_indexed_c(0..l.index_count, 0, 0..1);
        cx.stats.record_draw(l.index_count);
    }
}

impl B1Forest<'_> {
    fn draw(&self, cx: &mut DrawContext<'_, '_>) {
        let d = self.d;
        let Some((map, copy)) = d.copy(cx.copy_shift) else {
            return;
        };
        pass_globals(cx);
        d.statics.draw_set(
            BatchSet::Trees,
            cx.pass,
            cx.view_group,
            copy,
            map.fog,
            cx.stats,
        );
    }
}

impl B1Unit<'_> {
    fn draw(&self, cx: &mut DrawContext<'_, '_>) {
        let d = self.d;
        pass_globals(cx);
        if let Some((map, copy)) = d.copy(cx.copy_shift) {
            d.statics.draw_set(
                BatchSet::Entities,
                cx.pass,
                cx.view_group,
                copy,
                map.fog,
                cx.stats,
            );
        }
        // The skinned showcase models know nothing of map copies.
        if self.first() {
            d.models.draw(cx.pass, cx.view_group, cx.stats);
        }
    }
}

/// All adapters of one frame, in B1's draw order: the order inside a slot.
pub(crate) struct B1Adapters<'a> {
    terrain: B1Terrain<'a>,
    water: B1Water<'a>,
    decal: B1Decal<'a>,
    icons: B1Icons<'a>,
    landmark: B1Landmark<'a>,
    forest: B1Forest<'a>,
    unit: B1Unit<'a>,
    /// The terrain draws itself through the cache of the original: B1's terrain stays out of the
    /// plan and the subsystem takes its place.
    terrain_native: bool,
    /// The same for the water: the `Water` subsystem draws the surface once it is ready. B1's
    /// water then stays out of the plan, except in the class mask (`water_mask`).
    water_native: bool,
    /// The `Forest` subsystem draws the trees through `ForestShader`: B1's trees leave the plan.
    forest_native: bool,
}

impl<'a> B1Adapters<'a> {
    pub fn new(d: &'a B1Draw<'a>) -> Self {
        Self {
            terrain: B1Terrain::new(d),
            water: B1Water::new(d),
            decal: B1Decal::new(d),
            icons: B1Icons::new(d),
            landmark: B1Landmark::new(d),
            forest: B1Forest::new(d),
            unit: B1Unit::new(d),
            terrain_native: false,
            water_native: false,
            forest_native: false,
        }
    }

    /// With `true` the terrain subsystem draws the ground and B1's terrain is not in the list.
    pub fn with_native_terrain(mut self, native: bool) -> Self {
        self.terrain_native = native;
        self
    }

    /// With `true` the `Water` subsystem draws the surface and B1's water is not in the list.
    pub fn with_native_water(mut self, native: bool) -> Self {
        self.water_native = native;
        self
    }

    /// With `true` the `Forest` subsystem draws the trees and B1's trees are not in the list.
    pub fn with_native_forest(mut self, native: bool) -> Self {
        self.forest_native = native;
        self
    }

    /// B1's water for the class mask alone, while the subsystem draws the surface (the subsystem
    /// has no mask of its own yet).
    pub fn water_mask(&self) -> Option<&dyn RenderSystem> {
        self.water_native
            .then_some(&self.water as &dyn RenderSystem)
    }

    /// In B1's draw order.
    pub fn list(&self) -> Vec<&dyn RenderSystem> {
        let terrain: [&dyn RenderSystem; 1] = [&self.terrain];
        terrain
            .into_iter()
            .filter(|_| !self.terrain_native)
            .chain(self.rest())
            .collect()
    }

    fn rest(&self) -> Vec<&dyn RenderSystem> {
        let water: [&dyn RenderSystem; 1] = [&self.water];
        water
            .into_iter()
            .filter(|_| !self.water_native)
            .chain([
                &self.decal as &dyn RenderSystem,
                &self.icons,
                &self.landmark,
            ])
            .chain(
                [&self.forest as &dyn RenderSystem]
                    .into_iter()
                    .filter(|_| !self.forest_native),
            )
            .chain([&self.unit as &dyn RenderSystem])
            .collect()
    }

    pub fn get(&self, id: SystemId) -> Option<&dyn RenderSystem> {
        self.list().into_iter().find(|s| s.id() == id)
    }
}

#[cfg(test)]
mod tests {
    //! B1 regression ("Регрессия B1"): frames of a test world, drawn before and
    //! after a change of the frame (adapters, R0.10; gamma space and MSAA, R0.11), agree within
    //! ΔE2000 ≤ 1, except at the edges that MSAA smooths.
    //!
    //! The "before" frames are not in git. `B1_FRAME_DIR=<dir>` makes the test write a frame that is
    //! not in `<dir>` yet and compare against one that is, so the baseline is made with the code
    //! to be compared against. Without the variable the test checks that the frames are not blank
    //! and that drawing twice gives the same bytes.
    //!
    //! The baseline of RC-2 is B1 as the browser showed it: the shaders apply the gamma themselves
    //! (`params.x = 1`) into a surface without sRGB, with the B1 clear colour (0.02, 0.03, 0.06).
    //! The frames here are drawn with the same clear. (Against a frame from a hardware-sRGB target
    //! the picture differs by up to ΔE2000 17: the shaders' `pow(1/2.2)` is not the sRGB curve,
    //! and alpha blending happens in gamma space.)

    use std::collections::HashMap;
    use std::path::PathBuf;

    use draw_list::UiDrawList;
    use glam::{Mat4, Vec3};

    use crate::entities::roads::RouteKind;
    use crate::entities::{
        CityEntity, EntityPlayer, EntitySnapshot, Highlights, PlotDecor, UnitEntity,
        highlight::PathStep,
    };
    use crate::game_camera::GameCamera;
    use crate::gpu_test::{Gpu, gpu, read_texture};
    use crate::hexgrid::cell_center;
    use crate::layers::LayerSwitches;
    use crate::renderer::Renderer;
    use crate::scene::Scene;
    use crate::synthetic;
    use crate::terrain::{
        FeatureKind, Fog, Relief, TerrainCell, TerrainKind, TerrainMesh, TerrainSnapshot,
    };

    use wgpu::TextureFormat as F;

    /// Off the edges MSAA smooths, a frame may differ from the baseline by this much in this
    /// many pixels: faint lines thinner than a pixel (the grid in the far rows of the wide view)
    /// that the edge mask misses. Everything else is within ΔE2000 1.
    const OFF_EDGE_MAX: f64 = 3.5;
    const OFF_EDGE_PIXELS: usize = 100;

    const SIZE: (u32, u32) = (512, 288);
    /// Clear colour of the baseline frames (B1's, stored as it was).
    const BASELINE_CLEAR: [f64; 3] = [0.02, 0.03, 0.06];
    const MAP: (u32, u32) = (16, 10);

    fn cell(x: i32, y: i32) -> TerrainCell {
        let (relief, kind) = match (x, y) {
            (_, 0) | (_, 9) => (Relief::Water, TerrainKind::Ocean),
            (3, 1..=3) | (4, 2) => (Relief::Water, TerrainKind::Coast),
            (6, 4..=6) => (Relief::Hill, TerrainKind::Plains),
            (9, 5) | (9, 6) => (Relief::Mountain, TerrainKind::Snow),
            (x, _) if x % 5 == 2 => (Relief::Flat, TerrainKind::Desert),
            _ => (Relief::Flat, TerrainKind::Grass),
        };
        let fog = match x {
            12..=13 => Fog::Unrevealed,
            8..=10 => Fog::Hidden,
            _ => Fog::Visible,
        };
        TerrainCell {
            relief,
            kind,
            fog,
            rivers: if (x + y) % 4 == 0 && relief == Relief::Flat {
                0b000_0110
            } else {
                0
            },
            owner: match x {
                0..=4 => 0,
                11..=15 => 1,
                _ => -1,
            },
            feat: if x == 7 && y == 5 {
                FeatureKind::Forest
            } else {
                FeatureKind::None
            },
            ..TerrainCell::default()
        }
    }

    fn terrain() -> TerrainSnapshot {
        let cells = (0..MAP.1 as i32)
            .flat_map(|y| (0..MAP.0 as i32).map(move |x| cell(x, y)))
            .collect();
        TerrainSnapshot::new(MAP.0, MAP.1, true, cells)
            .unwrap()
            .with_owner_colors(vec![[200, 60, 60], [60, 110, 200]])
    }

    fn entities() -> EntitySnapshot {
        let unit = |id, x, y, civilian| UnitEntity {
            id,
            owner: (id % 2) as i8,
            type_name: "UNIT_WARRIOR".into(),
            x,
            y,
            hp: 60,
            max_hp: 100,
            civilian,
            needs_orders: id == 1,
        };
        let city = |id, x, y| CityEntity {
            id,
            owner: (id % 2) as i8,
            x,
            y,
            name: format!("City {id}"),
            population: 3,
            capital: id == 0,
            hp: 200,
            max_hp: 200,
            ..CityEntity::default()
        };
        let player = |id: i8, color| EntityPlayer {
            id,
            color,
            team_color: color,
            culture: None,
            era: None,
            industrial: false,
            art_techs: 0,
            era_index: None,
        };
        EntitySnapshot {
            players: vec![player(0, [200, 60, 60]), player(1, [60, 110, 200])],
            units: vec![
                unit(1, 5, 4, false),
                unit(2, 14, 5, false),
                unit(3, 1, 6, true),
            ],
            cities: vec![city(0, 2, 5), city(1, 15, 3)],
            plots: vec![
                PlotDecor {
                    x: 4,
                    y: 5,
                    route: Some(RouteKind::Road),
                    ..PlotDecor::default()
                },
                PlotDecor {
                    x: 5,
                    y: 5,
                    route: Some(RouteKind::Road),
                    ..PlotDecor::default()
                },
            ],
            ..EntitySnapshot::default()
        }
    }

    fn highlights() -> Highlights {
        Highlights {
            selected: Some((5, 4)),
            reachable: vec![(5, 5), (6, 3), (4, 4)],
            targets: vec![(6, 4)],
            path: vec![
                PathStep {
                    x: 5,
                    y: 4,
                    turn: 0,
                    flags: 0,
                },
                PathStep {
                    x: 5,
                    y: 5,
                    turn: 0,
                    flags: 0,
                },
                PathStep {
                    x: 6,
                    y: 6,
                    turn: 1,
                    flags: 0,
                },
            ],
        }
    }

    /// The test world in a renderer: wrapping map with fog, rivers, borders and the grid, units,
    /// cities (markers), roads, highlights and one skinned showcase model.
    fn world(gpu: &Gpu, format: F, samples: u32) -> Renderer {
        fill_world(Renderer::offscreen(gpu, SIZE, format, samples))
    }

    /// The test world in `r`.
    fn fill_world(mut r: Renderer) -> Renderer {
        let snapshot = terrain();
        let mesh = TerrainMesh::build(&snapshot);
        r.set_terrain(&snapshot, &mesh);
        r.set_entity_art_unavailable();
        r.set_entities(entities());
        r.set_highlights(highlights());
        r.set_grid_visible(true);
        r.set_animation_time(1.5);
        r.set_time(0.5);
        let demo = synthetic::skinned_cylinder();
        let model = r.create_model(&demo.model, &demo.textures);
        let at = cell_center(4, 6);
        r.add_model_instance(
            model,
            Mat4::from_scale_rotation_translation(
                Vec3::splat(20.0),
                glam::Quat::IDENTITY,
                Vec3::new(at.x, 4.0, at.y),
            ),
        );
        r
    }

    /// Camera views: the middle of the map, just right of the seam (left copy visible), just left
    /// of it (right copy visible).
    fn cameras() -> Vec<(&'static str, GameCamera)> {
        let at = |x, y, level| {
            let mut c = GameCamera::new(cell_center(x, y));
            c.set_level(level);
            c
        };
        vec![
            ("mid", at(8, 5, 14.0)),
            ("seam_right", at(1, 5, 13.0)),
            ("seam_left", at(14, 5, 13.0)),
            // A wide view that reaches the sea rows.
            ("wide", at(8, 5, 18.0)),
        ]
    }

    fn baseline_scene() -> Scene {
        Scene {
            clear_color: BASELINE_CLEAR,
            ..Scene::test(0.0)
        }
    }

    /// One frame of the world through the whole plan, `Present` included, into a surface of
    /// `format`; the bytes as stored.
    fn frame(gpu: &Gpu, r: &mut Renderer, camera: &GameCamera, format: F) -> Vec<u8> {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("b1 regression target"),
            size: wgpu::Extent3d {
                width: SIZE.0,
                height: SIZE.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let commands = r.encode_frame(
            &view,
            &baseline_scene(),
            camera,
            &UiDrawList::default(),
            None,
        );
        gpu.queue.submit([commands]);
        r.frame_submitted();
        read_texture(gpu, &texture, 0, 0, SIZE, 4)
    }

    // ---- ΔE2000 (Sharma, Wu, Dalal 2005), sRGB D65 ----

    fn lab(px: &[u8]) -> [f64; 3] {
        let lin = |c: u8| {
            let c = f64::from(c) / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = (lin(px[0]), lin(px[1]), lin(px[2]));
        let x = (0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / 0.95047;
        let y = 0.2126729 * r + 0.7151522 * g + 0.0721750 * b;
        let z = (0.0193339 * r + 0.119192 * g + 0.9503041 * b) / 1.08883;
        let f = |t: f64| {
            if t > 216.0 / 24389.0 {
                t.cbrt()
            } else {
                (24389.0 / 27.0 * t + 16.0) / 116.0
            }
        };
        let (fx, fy, fz) = (f(x), f(y), f(z));
        [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
    }

    fn de2000(a: [f64; 3], b: [f64; 3]) -> f64 {
        use std::f64::consts::PI;
        let (l1, a1, b1) = (a[0], a[1], a[2]);
        let (l2, a2, b2) = (b[0], b[1], b[2]);
        let c1 = a1.hypot(b1);
        let c2 = a2.hypot(b2);
        let cbar7 = (((c1 + c2) / 2.0).powi(7)).max(0.0);
        let g = 0.5 * (1.0 - (cbar7 / (cbar7 + 25f64.powi(7))).sqrt());
        let (ap1, ap2) = ((1.0 + g) * a1, (1.0 + g) * a2);
        let (cp1, cp2) = (ap1.hypot(b1), ap2.hypot(b2));
        let hue = |b: f64, a: f64| {
            if a == 0.0 && b == 0.0 {
                0.0
            } else {
                let h = b.atan2(a).to_degrees();
                if h < 0.0 { h + 360.0 } else { h }
            }
        };
        let (hp1, hp2) = (hue(b1, ap1), hue(b2, ap2));
        let dl = l2 - l1;
        let dc = cp2 - cp1;
        let dh = if cp1 * cp2 == 0.0 {
            0.0
        } else if (hp2 - hp1).abs() <= 180.0 {
            hp2 - hp1
        } else if hp2 - hp1 > 180.0 {
            hp2 - hp1 - 360.0
        } else {
            hp2 - hp1 + 360.0
        };
        let dh_big = 2.0 * (cp1 * cp2).sqrt() * (dh.to_radians() / 2.0).sin();
        let lbar = (l1 + l2) / 2.0;
        let cbar = (cp1 + cp2) / 2.0;
        let hbar = if cp1 * cp2 == 0.0 {
            hp1 + hp2
        } else if (hp1 - hp2).abs() <= 180.0 {
            (hp1 + hp2) / 2.0
        } else if hp1 + hp2 < 360.0 {
            (hp1 + hp2 + 360.0) / 2.0
        } else {
            (hp1 + hp2 - 360.0) / 2.0
        };
        let t = 1.0 - 0.17 * (hbar - 30.0).to_radians().cos()
            + 0.24 * (2.0 * hbar).to_radians().cos()
            + 0.32 * (3.0 * hbar + 6.0).to_radians().cos()
            - 0.20 * (4.0 * hbar - 63.0).to_radians().cos();
        let dtheta = 30.0 * (-(((hbar - 275.0) / 25.0).powi(2))).exp();
        let rc = 2.0 * (cbar.powi(7) / (cbar.powi(7) + 25f64.powi(7))).sqrt();
        let sl = 1.0 + 0.015 * (lbar - 50.0).powi(2) / (20.0 + (lbar - 50.0).powi(2)).sqrt();
        let sc = 1.0 + 0.045 * cbar;
        let sh = 1.0 + 0.015 * cbar * t;
        let rt = -(2.0 * dtheta * PI / 180.0).sin() * rc;
        let (x, y, z) = (dl / sl, dc / sc, dh_big / sh);
        (x * x + y * y + z * z + rt * y * z).sqrt()
    }

    #[test]
    fn de2000_matches_published_pairs() {
        // Pairs 1 and 4 of the table of Sharma et al. (2005).
        let d = de2000([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485]);
        assert!((d - 2.0425).abs() < 1e-3, "{d}");
        let d = de2000([50.0, -1.3802, -84.2814], [50.0, 0.0, -82.7485]);
        assert!((d - 1.0).abs() < 1e-3, "{d}");
    }

    /// (max ΔE, pixels with ΔE > 1) between two frames.
    fn compare(a: &[u8], b: &[u8]) -> (f64, usize) {
        assert_eq!(a.len(), b.len());
        let mut max = 0f64;
        let mut over = 0;
        for (p, q) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
            if p == q {
                continue;
            }
            let d = de2000(lab(p), lab(q));
            max = max.max(d);
            over += usize::from(d > 1.0);
        }
        (max, over)
    }

    /// Pixels at a visible edge of `base` (a right or lower neighbour differs clearly), widened by
    /// two rings: where MSAA changes the picture.
    fn edge_mask(base: &[u8]) -> Vec<bool> {
        let (w, h) = (SIZE.0 as usize, SIZE.1 as usize);
        let px = |x: usize, y: usize| &base[(y * w + x) * 4..][..4];
        let differs = |a: &[u8], b: &[u8]| {
            a.iter()
                .zip(b)
                .map(|(&p, &q)| (i32::from(p) - i32::from(q)).abs())
                .sum::<i32>()
                > 2
        };
        let mut edge = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                let here = px(x, y);
                if (x + 1 < w && differs(here, px(x + 1, y)))
                    || (y + 1 < h && differs(here, px(x, y + 1)))
                {
                    edge[y * w + x] = true;
                    if x + 1 < w {
                        edge[y * w + x + 1] = true;
                    }
                    if y + 1 < h {
                        edge[(y + 1) * w + x] = true;
                    }
                }
            }
        }
        for _ in 0..2 {
            let before = edge.clone();
            for y in 0..h {
                for x in 0..w {
                    if before[y * w + x] {
                        for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                            if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
                                edge[ny as usize * w + nx as usize] = true;
                            }
                        }
                    }
                }
            }
        }
        edge
    }

    /// (max ΔE, pixels over 1) off the `exempt` pixels, then on them.
    fn compare_outside(a: &[u8], b: &[u8], exempt: &[bool]) -> ((f64, usize), (f64, usize)) {
        let mut outside = (0f64, 0usize);
        let mut inside = (0f64, 0usize);
        for (i, (p, q)) in a.chunks_exact(4).zip(b.chunks_exact(4)).enumerate() {
            if p == q {
                continue;
            }
            let d = de2000(lab(p), lab(q));
            let slot = if exempt[i] { &mut inside } else { &mut outside };
            slot.0 = slot.0.max(d);
            slot.1 += usize::from(d > 1.0);
        }
        (outside, inside)
    }

    /// A surface that only offers sRGB gets `srgb_to_linear` from `Present`, so the bytes in the
    /// surface are those of a surface without sRGB (to rounding).
    #[test]
    fn an_srgb_only_surface_gets_the_same_bytes() {
        let Some(gpu) = gpu(
            "an_srgb_only_surface_gets_the_same_bytes",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let camera = &cameras()[0].1;
        let mut plain = world(&gpu, F::Rgba8Unorm, 1);
        let mut srgb = world(&gpu, F::Rgba8UnormSrgb, 1);
        let a = frame(&gpu, &mut plain, camera, F::Rgba8Unorm);
        let b = frame(&gpu, &mut srgb, camera, F::Rgba8UnormSrgb);
        let worst = a
            .iter()
            .zip(&b)
            .map(|(&p, &q)| (i32::from(p) - i32::from(q)).abs())
            .max()
            .unwrap();
        let (max, over) = compare(&a, &b);
        eprintln!("sRGB-only surface: worst byte {worst}, max dE2000 {max:.3}, {over} px over 1");
        assert!(max <= 1.0, "max dE2000 {max}, {over} px over 1");
    }

    /// The targets of the main pass follow the sample count.
    #[test]
    fn main_targets_follow_the_sample_count() {
        use crate::frame::{
            DEPTH_FORMAT, SCENE, SCENE_DEPTH, SCENE_MSAA, TargetPool, WORLD_FORMAT,
            declare_main_targets,
        };
        for samples in [1, 4] {
            let mut pool = TargetPool::new(false, (640, 360));
            declare_main_targets(&mut pool, samples).unwrap();
            assert_eq!(pool.size_of(SCENE), Some((640, 360)));
            assert_eq!(pool.size_of(SCENE_MSAA).is_some(), samples > 1);
            assert_eq!(pool.size_of(SCENE_DEPTH), Some((640, 360)));
        }
        assert_eq!(WORLD_FORMAT, F::Rgba8Unorm);
        assert!(!WORLD_FORMAT.is_srgb());
        assert_eq!(DEPTH_FORMAT, F::Depth24PlusStencil8);
    }

    #[test]
    fn b1_frames_match_the_baseline() {
        let Some(gpu) = gpu(
            "b1_frames_match_the_baseline",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let dir = std::env::var_os("B1_FRAME_DIR").map(PathBuf::from);
        // The frame of RC-2: a gamma-space world, a surface without sRGB, MSAA 4x. One sample
        // isolates the change of gamma from the smoothing of the edges.
        let mut r = world(&gpu, F::Rgba8Unorm, 4);
        let mut r1 = world(&gpu, F::Rgba8Unorm, 1);
        let mut seen: HashMap<&str, Vec<u8>> = HashMap::new();
        for (name, camera) in cameras() {
            let a = frame(&gpu, &mut r, &camera, F::Rgba8Unorm);
            let b = frame(&gpu, &mut r, &camera, F::Rgba8Unorm);
            assert!(a == b, "{name}: two frames of the same world differ");
            let distinct = a.chunks_exact(4).filter(|p| *p != &a[..4]).count();
            assert!(distinct > 1000, "{name}: the frame is blank");
            if let Some(dir) = &dir {
                let file = dir.join(format!("{name}.rgba"));
                match std::fs::read(&file) {
                    Ok(base) => {
                        let one = frame(&gpu, &mut r1, &camera, F::Rgba8Unorm);
                        let (max1, over1) = compare(&base, &one);
                        let near_edge = edge_mask(&base);
                        let ((max, over), (edge_max, edge_over)) =
                            compare_outside(&base, &a, &near_edge);
                        let edges = near_edge.iter().filter(|&&e| e).count();
                        eprintln!(
                            "{name}: 1 sample: max dE2000 {max1:.3}, {over1} px over 1; \
                             4 samples: off the edges max {max:.3}, {over} px over 1, \
                             on the edges ({edges} px) max {edge_max:.3}, {edge_over} px over 1"
                        );
                        assert!(max1 <= 1.0, "{name}: 1 sample, max dE2000 {max1}");
                        assert!(
                            max <= OFF_EDGE_MAX && over <= OFF_EDGE_PIXELS,
                            "{name}: off the edges max dE2000 {max}, {over} px over 1"
                        );
                    }
                    Err(_) => {
                        std::fs::create_dir_all(dir).unwrap();
                        std::fs::write(&file, &a).unwrap();
                        eprintln!("{name}: baseline written to {}", file.display());
                    }
                }
            }
            seen.insert(name, a);
        }
        // The views differ from each other (the camera does move the picture).
        assert!(seen["mid"] != seen["seam_right"]);
    }

    /// `FrameStats::passes` lists every pass of table 4.1 in order, with the draws of the frame
    /// shared out among them and the CPU time of the passes that did work.
    #[test]
    fn frame_stats_list_every_pass_of_the_table() {
        let Some(gpu) = gpu(
            "frame_stats_list_every_pass_of_the_table",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let mut r = world(&gpu, F::Rgba8Unorm, 4);
        frame(&gpu, &mut r, &cameras()[0].1, F::Rgba8Unorm);
        let stats = r.last_frame_stats().clone();
        let order: Vec<_> = stats.passes.iter().map(|p| p.pass).collect();
        assert_eq!(order, crate::frame::PassId::ORDER);
        assert!(stats.draws > 0);
        assert_eq!(
            stats.passes.iter().map(|p| p.draws).sum::<u32>(),
            stats.draws,
            "every draw belongs to a pass"
        );
        let main = stats
            .passes
            .iter()
            .find(|p| p.pass == crate::frame::PassId::Main)
            .unwrap();
        // B1 draws in the main pass; the other passes that draw are the fog mask's.
        let mask = stats
            .passes
            .iter()
            .find(|p| p.pass == crate::frame::PassId::FowMask)
            .unwrap();
        assert!(mask.draws > 0, "the fog mask draws the hexes of the map");
        // And the minimap's (WOV3: it draws every frame once it has a map).
        let minimap = stats
            .passes
            .iter()
            .find(|p| p.pass == crate::frame::PassId::Minimap)
            .map_or(0, |p| p.draws);
        assert_eq!(
            main.draws + mask.draws + minimap,
            stats.draws,
            "the main pass, the fog mask and the minimap draw"
        );
        assert!(main.cpu_ms > 0.0, "recording the main pass takes time");
        // Not a pass of its own on the GPU.
        let resolve = stats
            .passes
            .iter()
            .find(|p| p.pass == crate::frame::PassId::Resolve)
            .unwrap();
        assert_eq!(resolve.gpu_ms, None);
        let table = stats.pass_table();
        for pass in crate::frame::PassId::ORDER {
            assert!(table.contains(crate::frame::pass_name(pass)), "{table}");
        }
    }

    /// With `TIMESTAMP_QUERY` the passes the renderer opens get GPU times a few frames later.
    #[test]
    fn gpu_times_arrive_a_few_frames_later() {
        let Some(gpu) = gpu(
            "gpu_times_arrive_a_few_frames_later",
            wgpu::Backends::PRIMARY,
            wgpu::Features::TIMESTAMP_QUERY,
        ) else {
            return;
        };
        let mut r = world(&gpu, F::Rgba8Unorm, 4);
        assert!(r.gpu_pass_times());
        let camera = &cameras()[0].1;
        let mut seen = None;
        for _ in 0..30 {
            frame(&gpu, &mut r, camera, F::Rgba8Unorm);
            let get = |pass| {
                r.last_frame_stats()
                    .passes
                    .iter()
                    .find(|p| p.pass == pass)
                    .and_then(|p| p.gpu_ms)
            };
            if let (Some(main), Some(present)) = (
                get(crate::frame::PassId::Main),
                get(crate::frame::PassId::Present),
            ) {
                seen = Some((main, present));
                break;
            }
        }
        let (main, present) = seen.expect("a GPU time arrived within 30 frames");
        assert!((0.0..1000.0).contains(&main), "main {main} ms");
        assert!((0.0..1000.0).contains(&present), "present {present} ms");
    }

    /// `set_time_override` pins the time the shaders see; `set_constant_overrides` replaces blocks
    /// on upload (for `golden`).
    #[test]
    fn overrides_pin_the_time_and_the_constants() {
        use crate::shaders::UniformBlock;
        let Some(gpu) = gpu(
            "overrides_pin_the_time_and_the_constants",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let mut r = world(&gpu, F::Rgba8Unorm, 1);
        r.set_animation_time(40.0);
        assert_eq!(r.shader_time(), 40.0);
        r.set_time_override(Some(1.5));
        assert_eq!(r.shader_time(), 1.5);
        r.set_time_override(None);
        assert_eq!(r.shader_time(), 40.0);

        // A block of `uniform_block!` is replaced by CTAB name on upload.
        crate::uniform_block! {
            pub struct Probe {
                pub f3FogColor: [f32; 3],
                pub g_ShadowFade: f32,
            }
        }
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("probe"),
            size: std::mem::size_of::<Probe>() as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let block = Probe {
            f3FogColor: [0.5, 0.5, 0.5],
            g_ShadowFade: 1.0,
        };
        assert_eq!(Probe::FIELDS.len(), 2);
        r.upload_uniform(&buffer, 0, &block);
        r.set_constant_overrides(HashMap::from([("g_ShadowFade".to_owned(), vec![0.125])]));
        r.upload_uniform(&buffer, 0, &block);
        assert_eq!(
            r.constant_overrides().get("g_ShadowFade"),
            Some(&[0.125][..])
        );
        let read = |buffer: &wgpu::Buffer| {
            let staging = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("staging"),
                size: 16,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut e = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            e.copy_buffer_to_buffer(buffer, 0, &staging, 0, 16);
            gpu.queue.submit([e.finish()]);
            staging.map_async(wgpu::MapMode::Read, .., |r| r.expect("map"));
            gpu.device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("poll");
            let v: Vec<f32> =
                bytemuck::cast_slice(&staging.get_mapped_range(..).expect("range")[..]).to_vec();
            v
        };
        let got = read(&buffer);
        assert_eq!(got[3], 0.125, "overridden");
        assert_eq!(&got[0..3], &[0.5, 0.5, 0.5], "the rest as it was");
        r.set_constant_overrides(HashMap::new());
        r.upload_uniform(&buffer, 0, &block);
        assert_eq!(read(&buffer)[3], 1.0);
    }

    /// The minimap stub (R0.16): the view appears with the first frame, keeps its generation,
    /// is offered to the overlay every frame and is gone while `Minimap` is switched off.
    #[test]
    fn the_minimap_is_offered_to_the_overlay() {
        use crate::overlay::{MinimapInfo, Overlay, OverlayTarget, OverlayTextures};
        #[derive(Default)]
        struct Spy {
            offers: Vec<Option<MinimapInfo>>,
        }
        impl Overlay for Spy {
            fn offer(&mut self, t: &OverlayTextures<'_>) {
                assert_eq!(t.minimap.is_some(), t.minimap_info.is_some());
                self.offers.push(t.minimap_info);
            }
            fn prepare(
                &mut self,
                _: &wgpu::Device,
                _: &wgpu::Queue,
                _: &mut wgpu::CommandEncoder,
                _: &OverlayTarget,
            ) {
            }
            fn draw(&mut self, _: &mut wgpu::RenderPass<'static>) {}
        }
        let Some(gpu) = gpu(
            "the_minimap_is_offered_to_the_overlay",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let mut r = world(&gpu, F::Rgba8Unorm, 1);
        assert!(
            r.minimap_view().is_none(),
            "no target before the first frame"
        );
        let mut spy = Spy::default();
        let camera = &cameras()[0].1;
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("target"),
            size: wgpu::Extent3d {
                width: SIZE.0,
                height: SIZE.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: F::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        for _ in 0..3 {
            let c = r.encode_frame(
                &view,
                &baseline_scene(),
                camera,
                &UiDrawList::default(),
                Some(&mut spy),
            );
            gpu.queue.submit([c]);
        }
        let (_, info) = r.minimap_view().expect("the target exists after a frame");
        assert_eq!(info.size, (320, 190));
        assert_eq!(spy.offers.len(), 3);
        assert!(
            spy.offers.iter().all(|o| *o == Some(info)),
            "one generation"
        );
        // The test world has a map: a click lands on it (WOV3).
        assert!(r.minimap_to_world(100, 100).is_some());
        // Switched off: nothing is offered, and a click goes nowhere.
        r.set_layers(LayerSwitches::from_names("Minimap").0);
        assert!(r.minimap_view().is_none());
        assert_eq!(r.minimap_to_world(100, 100), None);
        let c = r.encode_frame(
            &view,
            &baseline_scene(),
            camera,
            &UiDrawList::default(),
            Some(&mut spy),
        );
        gpu.queue.submit([c]);
        assert_eq!(spy.offers.last(), Some(&None));
        r.set_layers(LayerSwitches::new());
        // The fog targets follow FOWLevel.
        let mut s = r.settings().clone();
        s.set("FOWLevel", 1).unwrap();
        r.set_settings(s);
        assert_eq!(
            r.minimap_view().map(|(_, i)| i),
            Some(info),
            "the minimap is not remade"
        );
    }

    /// A cloud texture of a few texels with a varying alpha, so that the layers show a pattern.
    fn cloud_blob(seed: u8) -> asset_format::TextureBlob {
        let n = 8usize;
        let data: Vec<u8> = (0..n * n)
            .flat_map(|i| {
                let a = 110 + ((i as u32 * 37 + u32::from(seed) * 53) % 146) as u8;
                [235, 238, 245, a]
            })
            .collect();
        asset_format::TextureBlob {
            format: asset_format::TextureFormat::Rgba8,
            dim: asset_format::TexDim::D2,
            srgb: false,
            width: n as u32,
            height: n as u32,
            depth: 1,
            mips: vec![data],
        }
    }

    fn cloud_textures()
    -> std::collections::HashMap<asset_format::LogicalId, asset_format::TextureBlob> {
        crate::terrain::fog::CLOUD_LAYERS
            .iter()
            .enumerate()
            .map(|(i, l)| (asset_format::LogicalId::new(l.texture), cloud_blob(i as u8)))
            .collect()
    }

    /// WOV1: the mask has the classes of the cells in its channels, the clouds show over the
    /// unrevealed cells only (and not where the cull leaves them out), the level 0 draws the
    /// classic fog and no clouds. With `FOW_DEBUG_DIR` the frames and the mask are written there
    /// as raw RGBA for a look.
    #[test]
    fn the_fog_mask_and_the_clouds_follow_the_fog() {
        use crate::frame::{FOW_MASK, FrameSource, RenderSystem};
        use crate::gpu_test::block_on;
        use crate::renderer::BackendPref;
        use crate::world::fow::mask::{FowView, MaskView};

        let Ok(r) = block_on(Renderer::new_headless(SIZE, BackendPref::Auto)) else {
            eprintln!("SKIP the_fog_mask_and_the_clouds_follow_the_fog: no adapter");
            return;
        };
        let mut r = fill_world(r);
        let (_, camera) = cameras().into_iter().find(|(n, _)| *n == "wide").unwrap();
        let ui = UiDrawList::default();
        let scene = baseline_scene();
        let read = |r: &mut Renderer, what: FrameSource| {
            let readback = r.read_frame(what);
            r.render(&scene, &camera, &ui, None).unwrap();
            readback.wait().unwrap()
        };
        let dump = |name: &str, image: &crate::frame::Image| {
            if let Ok(dir) = std::env::var("FOW_DEBUG_DIR") {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(
                    std::path::Path::new(&dir).join(format!("{name}.rgba")),
                    &image.bytes,
                )
                .unwrap();
            }
        };
        let aspect = SIZE.0 as f32 / SIZE.1 as f32;
        let vp = camera.view_proj(aspect);
        // The screen pixel over the ground of a cell.
        let screen = |x: i32, y: i32| {
            let c = cell_center(x, y);
            let p = vp.project_point3(Vec3::new(c.x, 12.0, c.y));
            (
                ((p.x * 0.5 + 0.5) * SIZE.0 as f32) as u32,
                ((0.5 - p.y * 0.5) * SIZE.1 as f32) as u32,
            )
        };

        // ---- the mask ----
        let mask = read(&mut r, FrameSource::Target(FOW_MASK));
        assert_eq!(mask.size, (1024, 1024), "FOWLevel 3 works at level 2");
        dump("mask", &mask);
        let view = MaskView::of(&FowView {
            view_proj: vp,
            eye: camera.eye(),
            ours_to_orig: crate::frame::basis_rotation(),
        });
        let in_mask = |x: i32, y: i32| {
            let c = cell_center(x, y);
            let uv = view.uv(glam::Vec2::new(c.x, -c.y));
            let at = mask.pixel((uv.x * 1024.0) as u32, (uv.y * 1024.0) as u32);
            [at[0], at[1], at[2], at[3]]
        };
        // x: revealed, y: cloud coverage, z: visible.
        assert_eq!(in_mask(4, 5), [255, 0, 255, 255], "a visible cell");
        assert_eq!(
            in_mask(9, 5),
            [255, 0, 0, 255],
            "a revealed cell that is hidden"
        );
        assert_eq!(in_mask(12, 5), [0, 255, 0, 255], "an unrevealed cell");
        assert_eq!(
            in_mask(13, 3),
            [0, 255, 0, 255],
            "the other unrevealed column"
        );
        let stats = r.world().fluid_fow.mask_counters();
        assert!(
            stats.unrevealed >= 20 && stats.visible > 0 && stats.revealed > 0,
            "{stats:?}"
        );

        // ---- no cloud textures: nothing over the map ----
        let plain = read(&mut r, FrameSource::World);
        dump("plain", &plain);
        assert_eq!(
            r.world().fluid_fow.cloud_counters().draws,
            0,
            "no textures, no draws"
        );

        // ---- the clouds ----
        assert!(r.set_fog_clouds(&cloud_textures()));
        let clouded = read(&mut r, FrameSource::World);
        dump("clouds", &clouded);
        let k = r.world().fluid_fow.cloud_counters();
        assert!(k.tiles > 0 && k.draws >= 3, "{k:?}");
        assert!(
            k.tiles_culled > 0,
            "tiles over revealed ground are left out: {k:?}"
        );
        let differs = |x: i32, y: i32| {
            let (px, py) = screen(x, y);
            plain.pixel(px, py) != clouded.pixel(px, py)
        };
        assert!(
            differs(12, 5) && differs(13, 4),
            "clouds over the unrevealed cells"
        );
        assert!(!differs(4, 5), "none over a visible cell");
        assert!(!differs(9, 5), "none over a hidden cell");
        let changed = plain
            .bytes
            .chunks_exact(4)
            .zip(clouded.bytes.chunks_exact(4))
            .filter(|(a, b)| a != b)
            .count();
        assert!(
            changed > 200,
            "the clouds change the picture ({changed} px)"
        );

        // ---- time moves the clouds ----
        r.set_animation_time(40.0);
        let later = read(&mut r, FrameSource::World);
        assert!(later != clouded, "the layers scroll");

        // ---- level 0: the classic fog, no clouds ----
        let mut s = r.settings().clone();
        s.set("FOWLevel", 0).unwrap();
        r.set_settings(s);
        let classic = read(&mut r, FrameSource::World);
        dump("classic", &classic);
        assert_eq!(
            r.world().fluid_fow.cloud_counters().draws,
            0,
            "no clouds at level 0"
        );
        let (px, py) = screen(12, 5);
        let at = classic.pixel(px, py);
        assert!(
            at[..3].iter().all(|&c| c < 30),
            "an unrevealed cell is black: {at:?}"
        );
        let (hx, hy) = screen(4, 5);
        assert_eq!(
            classic.pixel(hx, hy),
            plain.pixel(hx, hy),
            "visible cells are clear"
        );
        let counters = r.world().fow_classic.counters();
        assert!(
            counters
                .iter()
                .any(|(n, v)| *n == "fow.classic_opaque_hexes" && *v == 20)
        );
        // The mask is 512 at level 1 and 0.
        let small = read(&mut r, FrameSource::Target(FOW_MASK));
        assert_eq!(small.size, (512, 512));
    }

    /// Client-hooks R1 to R3: a mod's render system is prepared and recorded in the main pass, sees the frame's
    /// constants, and a system that draws nothing changes no pixel.
    #[test]
    fn a_mod_system_joins_the_main_pass_and_an_inert_one_changes_no_pixel() {
        use crate::frame::{
            DrawContext, FrameSource, InitContext, MainSlot, PassSlot, PrepareContext,
            RenderSystem, SystemId,
        };
        use crate::gpu_test::block_on;
        use crate::renderer::BackendPref;
        use std::cell::Cell;
        use std::rc::Rc;

        #[derive(Default)]
        struct Seen {
            prepared: Cell<u32>,
            recorded: Cell<u32>,
            frame_group: Cell<bool>,
            time_finite: Cell<bool>,
        }
        struct Inert(Rc<Seen>);
        impl RenderSystem for Inert {
            fn id(&self) -> SystemId {
                SystemId::Mod(1)
            }
            fn passes(&self) -> &'static [PassSlot] {
                &[PassSlot::Main(MainSlot::Mod)]
            }
            fn init(&mut self, _cx: &mut InitContext<'_>) {}
            fn prepare(&mut self, cx: &mut PrepareContext<'_>) {
                self.0.prepared.set(self.0.prepared.get() + 1);
                // None here: B1's ground has no sampler; the native terrain's is `Some` once a map is set.
                let _ = cx.terrain();
            }
            fn record(&self, _slot: PassSlot, cx: &mut DrawContext<'_, '_>) {
                self.0.recorded.set(self.0.recorded.get() + 1);
                let frame = &cx.services().frame;
                self.0.frame_group.set(frame.bind_group().is_some());
                self.0.time_finite.set(frame.time().is_finite());
            }
        }

        let Ok(r) = block_on(Renderer::new_headless(SIZE, BackendPref::Auto)) else {
            eprintln!("SKIP a_mod_system_joins_the_main_pass: no adapter");
            return;
        };
        let mut r = fill_world(r);
        let (_, camera) = cameras().into_iter().find(|(n, _)| *n == "wide").unwrap();
        let ui = UiDrawList::default();
        let scene = baseline_scene();
        let read = |r: &mut Renderer| {
            let readback = r.read_frame(FrameSource::World);
            r.render(&scene, &camera, &ui, None).unwrap();
            readback.wait().unwrap()
        };
        let before = read(&mut r);
        let seen = Rc::new(Seen::default());
        r.add_mod_system(Box::new(Inert(Rc::clone(&seen)))).unwrap();
        assert!(r.mod_gpu().samples >= 1);
        let after = read(&mut r);
        assert!(
            before == after,
            "a mod system that draws nothing changes no pixel"
        );
        assert!(seen.prepared.get() >= 1 && seen.recorded.get() >= 1);
        assert!(seen.frame_group.get() && seen.time_finite.get());
    }

    /// A camera of a mod (client-hooks C2): the frame draws, the fluid fog is off while the camera holds the
    /// screen and the switches the player set are as they were when the strategic camera is back, and the
    /// strategic frame after it is the frame before it.
    #[test]
    fn a_mod_camera_hides_the_fog_clouds_and_gives_the_frame_back() {
        use crate::camera::{CameraView, FirstPerson, ModCamera, ModCameraView};
        use crate::frame::{FrameSource, SystemId};
        use crate::gpu_test::block_on;
        use crate::renderer::BackendPref;

        let Ok(r) = block_on(Renderer::new_headless(SIZE, BackendPref::Auto)) else {
            eprintln!(
                "SKIP a_mod_camera_hides_the_fog_clouds_and_gives_the_frame_back: no adapter"
            );
            return;
        };
        let mut r = fill_world(r);
        let (_, camera) = cameras().into_iter().find(|(n, _)| *n == "wide").unwrap();
        let ui = UiDrawList::default();
        let scene = baseline_scene();
        let read = |r: &mut Renderer, cam: &dyn CameraView| {
            let readback = r.read_frame(FrameSource::World);
            r.render(&scene, cam, &ui, None).unwrap();
            readback.wait().unwrap()
        };
        let (mut player, _) = LayerSwitches::from_names("Decal");
        r.set_layers(player.clone());
        let before = read(&mut r, &camera);
        assert!(!r.layers().is_disabled(SystemId::FluidFow));

        let mut fp = FirstPerson::new(glam::Vec3::new(200.0, 12.0, -150.0));
        fp.yaw = 0.7;
        let low: &dyn ModCamera = &fp;
        let shown = read(&mut r, &ModCameraView(low));
        assert!(shown.bytes.iter().any(|&b| b != 0), "a frame was drawn");
        assert!(r.layers().is_disabled(SystemId::FluidFow));
        assert!(
            r.layers().is_disabled(SystemId::Decal),
            "the player's switch stays"
        );
        // The player changes a switch while the camera holds the screen: it is the base from then on.
        player.disable("Waves").ok();
        r.set_layers(player.clone());
        assert!(r.layers().is_disabled(SystemId::FluidFow));
        let again = read(&mut r, &camera);
        assert!(!r.layers().is_disabled(SystemId::FluidFow));
        assert_eq!(*r.layers(), player);
        let (mut want, _) = LayerSwitches::from_names("Decal");
        want.disable("Waves").ok();
        assert_eq!(*r.layers(), want);
        // The frame of the strategic camera does not depend on the visit of the other one.
        r.set_layers(LayerSwitches::from_names("Decal").0);
        let after = read(&mut r, &camera);
        assert!(before == after, "the strategic frame is the one it was");
        let _ = again;
    }

    /// A renderer without a window (R0.17): the same world twice gives the same bytes, the frame
    /// with the UI is the frame of the world here, `FixedCamera` with the matrices of a camera
    /// draws what the camera draws, and the class mask has land and water where they are.
    #[test]
    fn headless_frames_repeat_and_the_class_mask_has_the_classes() {
        use crate::camera::{CameraView, FixedCamera};
        use crate::frame::FrameSource;
        use crate::frame::mask::class;
        use crate::gpu_test::block_on;
        use crate::renderer::BackendPref;

        let Ok(r) = block_on(Renderer::new_headless(SIZE, BackendPref::Auto)) else {
            eprintln!("SKIP headless_frames_repeat_and_the_class_mask_has_the_classes: no adapter");
            return;
        };
        let mut r = fill_world(r);
        let (_, camera) = cameras().into_iter().find(|(n, _)| *n == "wide").unwrap();
        let ui = UiDrawList::default();
        let scene = baseline_scene();
        let read = |r: &mut Renderer, what: FrameSource, cam: &dyn CameraView| {
            let readback = r.read_frame(what);
            r.render(&scene, cam, &ui, None).unwrap();
            readback.wait().unwrap()
        };

        let a = read(&mut r, FrameSource::World, &camera);
        let b = read(&mut r, FrameSource::World, &camera);
        assert_eq!(a.size, SIZE);
        assert_eq!(a.format, F::Rgba8Unorm);
        assert!(a == b, "the same world twice gives the same bytes");
        let distinct = a
            .bytes
            .chunks_exact(4)
            .filter(|p| *p != &a.bytes[..4])
            .count();
        assert!(distinct > 1000, "the frame is not blank");

        // `Present` and the UI run: the frame that goes to the output is that of the world.
        let with_ui = read(&mut r, FrameSource::WithUi, &camera);
        assert!(with_ui.bytes == a.bytes, "no UI, no change by Present");

        // A fixed camera with the matrices of the game camera draws the same frame.
        let aspect = SIZE.0 as f32 / SIZE.1 as f32;
        let fixed = FixedCamera {
            view: camera.view(),
            proj: camera.projection(aspect),
            eye: camera.eye(),
        };
        let from_fixed = read(&mut r, FrameSource::World, &fixed);
        assert!(
            from_fixed == a,
            "FixedCamera gives the frame of its matrices"
        );

        // A target of the pool by name; a multisampled one cannot be read.
        let scene_img = read(&mut r, FrameSource::Target(crate::frame::SCENE), &camera);
        assert!(scene_img == a);
        let msaa = r.read_frame(FrameSource::Target(crate::frame::SCENE_MSAA));
        r.render(&scene, &camera, &ui, None).unwrap();
        assert!(msaa.wait().is_err(), "SceneMsaa is multisampled");

        // The class mask: the same twice, only the classes B1 can tell apart, in the right places.
        let m1 = read(&mut r, FrameSource::ClassMask, &camera);
        let m2 = read(&mut r, FrameSource::ClassMask, &camera);
        assert_eq!(m1.format, F::R8Uint);
        assert_eq!(m1.size, SIZE);
        assert!(m1 == m2, "the mask of the same world twice is the same");
        let has = |c: u32| m1.bytes.iter().any(|&b| u32::from(b) == c);
        assert!(has(class::LAND) && has(class::WATER));
        // The overlay's bit (the lines) is on top of the classes; what is under it is not told.
        let bits = (class::FOG_BIT | class::OVERLAY_BIT) as u8;
        assert!(
            m1.bytes
                .iter()
                .all(|&b| u32::from(b & !bits) <= class::WATER)
        );
        let vp = camera.view_proj(aspect);
        let class_at = |x: i32, y: i32| {
            let c = cell_center(x, y);
            let p = vp.project_point3(Vec3::new(c.x, 0.0, c.y));
            let (px, py) = (
                (p.x * 0.5 + 0.5) * SIZE.0 as f32,
                (0.5 - p.y * 0.5) * SIZE.1 as f32,
            );
            assert!(
                (0.0..SIZE.0 as f32).contains(&px) && (0.0..SIZE.1 as f32).contains(&py),
                "cell ({x}, {y}) is on screen at ({px}, {py})"
            );
            u32::from(m1.pixel(px as u32, py as u32)[0] & !bits)
        };
        assert_eq!(class_at(8, 5), class::LAND, "a grass cell");
        assert_eq!(class_at(8, 0), class::WATER, "an ocean cell");
        // Switching the terrain off empties the mask.
        r.set_layers(LayerSwitches::from_names("Terrain").0);
        let none = read(&mut r, FrameSource::ClassMask, &camera);
        assert!(
            none.bytes.iter().all(|&b| b & !bits == 0),
            "nothing drawn, class 0 (the overlay's and the fog's bits stay)"
        );
    }

    #[test]
    fn a_disabled_subsystem_is_not_drawn() {
        let Some(gpu) = gpu(
            "a_disabled_subsystem_is_not_drawn",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let mut r = world(&gpu, F::Rgba8Unorm, 4);
        let camera = &cameras()[0].1;
        let all = frame(&gpu, &mut r, camera, F::Rgba8Unorm);
        let draws_all = r.last_frame_stats().draws;
        assert!(r.settled());
        // The names of `?disable=`.
        for (names, what) in [
            ("Terrain", "terrain and water"),
            ("Overlay", "borders and grid"),
            ("LandmarkSystem", "roads and highlights"),
            ("Unit", "units"),
        ] {
            let (off, warnings) = LayerSwitches::from_names(names);
            assert!(warnings.is_empty());
            r.set_layers(off);
            let without = frame(&gpu, &mut r, camera, F::Rgba8Unorm);
            assert!(
                r.last_frame_stats().draws < draws_all,
                "{what}: switching {names} off draws less"
            );
            assert!(without != all, "{what}: the picture changes");
            r.set_layers(LayerSwitches::new());
        }
        assert!(
            frame(&gpu, &mut r, camera, F::Rgba8Unorm) == all,
            "back on: the same picture"
        );
    }

    /// WOV8: the toggles of the overlay subsystems (`[DisableSystems]` of the original, RC 9): a
    /// subsystem that is switched off draws nothing and reports nothing, the others go on.
    #[test]
    fn the_overlay_toggles_leave_their_subsystems_out() {
        let Some(gpu) = gpu(
            "the_overlay_toggles_leave_their_subsystems_out",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let mut r = world(&gpu, F::Rgba8Unorm, 1);
        assert!(r.set_fog_clouds(&cloud_textures()));
        // Level 0 for the classic fog, which draws only there; set below for the others.
        let camera = cameras().into_iter().find(|(n, _)| *n == "wide").unwrap().1;
        let counters_of = |r: &Renderer| -> Vec<&'static str> {
            r.last_frame_stats()
                .counters
                .iter()
                .map(|(n, _)| *n)
                .collect()
        };
        let pass_draws = |r: &Renderer, pass: crate::frame::PassId| {
            r.last_frame_stats()
                .passes
                .iter()
                .find(|p| p.pass == pass)
                .map_or(0, |p| p.draws)
        };
        // (name, a counter of the subsystem that must be gone, what must draw less)
        let cases = [
            ("FluidFOW", "fow.mask_visible"),
            ("Overlay", "overlay.ground_builds"),
            ("MovementPath", "path.builds"),
            ("Arrows", "arrows.updates"),
            ("Minimap", "minimap.fills"),
            ("FOW", "fow.classic_draws"),
        ];
        for classic in [false, true] {
            let mut s = r.settings().clone();
            s.set("FOWLevel", if classic { 0 } else { 3 }).unwrap();
            r.set_settings(s);
            r.set_layers(LayerSwitches::new());
            frame(&gpu, &mut r, &camera, F::Rgba8Unorm);
            let on_draws = r.last_frame_stats().draws;
            let on_mask = pass_draws(&r, crate::frame::PassId::FowMask);
            let on = counters_of(&r);
            for (name, counter) in cases {
                assert!(on.contains(&counter), "{name} is on: {counter} is reported");
                let (off, warnings) = LayerSwitches::from_names(name);
                assert!(warnings.is_empty(), "{name} is a known name");
                r.set_layers(off);
                frame(&gpu, &mut r, &camera, F::Rgba8Unorm);
                assert!(
                    !counters_of(&r).contains(&counter),
                    "{name} is off: {counter} is gone"
                );
                let draws = r.last_frame_stats().draws;
                match name {
                    "FluidFOW" => {
                        assert_eq!(pass_draws(&r, crate::frame::PassId::FowMask), 0, "no mask");
                        assert!(on_mask > 0);
                        if !classic {
                            assert!(draws < on_draws, "no clouds either");
                        }
                    }
                    "Overlay" => {
                        assert!(
                            draws < on_draws,
                            "{name}: draws less ({draws} < {on_draws})"
                        );
                    }
                    "FOW" => {
                        assert_eq!(
                            draws < on_draws,
                            classic,
                            "the classic fog draws at level 0"
                        );
                    }
                    _ => {}
                }
                r.set_layers(LayerSwitches::new());
            }
        }
    }

    /// WOV8: the class mask has the bits 0x40 (the fog clouds) and 0x80 (the overlay) where those
    /// are, and none where their subsystems are off.
    #[test]
    fn the_class_mask_has_the_fog_and_overlay_bits() {
        use crate::frame::FrameSource;
        use crate::frame::mask::class::{FOG_BIT, OVERLAY_BIT};
        use crate::gpu_test::block_on;
        use crate::renderer::BackendPref;
        let Ok(r) = block_on(Renderer::new_headless(SIZE, BackendPref::Auto)) else {
            eprintln!("SKIP the_class_mask_has_the_fog_and_overlay_bits: no adapter");
            return;
        };
        let mut r = fill_world(r);
        assert!(r.set_fog_clouds(&cloud_textures()));
        let (_, camera) = cameras().into_iter().find(|(n, _)| *n == "wide").unwrap();
        let ui = UiDrawList::default();
        let scene = baseline_scene();
        let mask = |r: &mut Renderer| {
            let readback = r.read_frame(FrameSource::ClassMask);
            r.render(&scene, &camera, &ui, None).unwrap();
            readback.wait().unwrap()
        };
        let count =
            |m: &crate::frame::Image, bit: u8| m.bytes.iter().filter(|&&b| b & bit != 0).count();
        let on = mask(&mut r);
        let (fog, overlay) = (count(&on, FOG_BIT as u8), count(&on, OVERLAY_BIT as u8));
        assert!(fog > 500, "the clouds mark their pixels ({fog})");
        assert!(overlay > 100, "the lines mark theirs ({overlay})");
        // The bits do not collide with the classes of what is drawn.
        assert!(
            on.bytes
                .iter()
                .all(|&b| b == 0 || b & !(FOG_BIT as u8 | OVERLAY_BIT as u8) <= 10)
        );
        r.set_layers(LayerSwitches::from_names("FluidFOW").0);
        let no_fog = mask(&mut r);
        assert_eq!(
            count(&no_fog, FOG_BIT as u8),
            0,
            "FluidFOW off: no cloud bit"
        );
        assert!(count(&no_fog, OVERLAY_BIT as u8) > 0);
        r.set_layers(LayerSwitches::from_names("Overlay").0);
        let no_overlay = mask(&mut r);
        assert_eq!(
            count(&no_overlay, OVERLAY_BIT as u8),
            0,
            "Overlay off: no overlay bit"
        );
        assert!(count(&no_overlay, FOG_BIT as u8) > 0);
    }

    /// R5.9: the trade routes of the active player go into the map of the overlay, over the grid and
    /// the borders; the option and the owner switch them off.
    #[test]
    fn the_trade_routes_go_into_the_map_of_the_overlay() {
        use crate::frame::{FrameSource, OVERLAY_MAP};
        use crate::gpu_test::block_on;
        use crate::renderer::BackendPref;
        use crate::world::overlay::cells::CellGeometry;
        use crate::world::trade::{TradeKind, TradeRoute};
        let Ok(r) = block_on(Renderer::new_headless(SIZE, BackendPref::Auto)) else {
            eprintln!("SKIP the_trade_routes_go_into_the_map_of_the_overlay: no adapter");
            return;
        };
        let mut r = fill_world(r);
        let (_, camera) = cameras().into_iter().find(|(n, _)| *n == "wide").unwrap();
        let ui = UiDrawList::default();
        let scene = baseline_scene();
        let w = MAP.0 as f32 * 64.0;
        let sheet = CellGeometry::flat(
            [-64.0, -60.0 * MAP.1 as f32 - 40.0],
            [w + 64.0, 80.0],
            24,
            &[0.0, -w, w],
        );
        r.world_mut().overlay.set_cell_geometry(&sheet);
        let lit =
            |img: &crate::frame::Image| img.bytes.chunks_exact(4).filter(|p| p[3] > 8).count();
        let read = |r: &mut Renderer| {
            let readback = r.read_frame(FrameSource::Target(OVERLAY_MAP));
            r.render(&scene, &camera, &ui, None).unwrap();
            readback.wait().unwrap()
        };
        let counter = |r: &Renderer, name: &str| {
            r.last_frame_stats()
                .counters
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| *v)
        };
        let route = |owner: i8, y: i32| TradeRoute {
            owner,
            color: [255, 0, 0],
            kind: TradeKind::International,
            plots: (1..8).map(|x| (x, y)).collect(),
        };
        let base = lit(&read(&mut r));
        // A route of the active player, one of another player.
        r.set_trade_routes(0, vec![route(0, 2), route(1, 4)]);
        let with = read(&mut r);
        assert!(counter(&r, "trade.indices") > Some(0));
        let drawn = lit(&with);
        assert!(
            drawn > base + 150,
            "the route is in the map ({base} -> {drawn})"
        );
        // The ribbon is the owner's red, at the alpha 0.4 of the active player's routes.
        let red = with
            .bytes
            .chunks_exact(4)
            .filter(|p| p[0] > 60 && p[0] <= 110 && p[1] < 10 && p[2] < 10 && p[3] >= 90)
            .count();
        assert!(red > 100, "red pixels at alpha 0.4: {red}");
        // The other player's route is not drawn: with only that one nothing is added.
        r.set_trade_routes(0, vec![route(1, 4)]);
        assert_eq!(lit(&read(&mut r)), base);
        // The option off hides the routes of the active player; on again brings them back.
        r.set_trade_routes(0, vec![route(0, 2)]);
        assert!(lit(&read(&mut r)) > base + 100);
        r.set_show_trade(false);
        assert_eq!(lit(&read(&mut r)), base);
        r.set_show_trade(true);
        assert!(lit(&read(&mut r)) > base + 100);
        // The `Arrows` layer switch takes them with it.
        r.set_layers(LayerSwitches::from_names("Arrows").0);
        assert_eq!(lit(&read(&mut r)), base);
    }

    /// WOV2: with the cells of the ground the grid goes into the map of the overlay and from there,
    /// laid over the cells, into `ScreenOverlay`; B1's line is not drawn then. `RENDER_DUMP_DIR`
    /// writes the target as a PPM for a look.
    #[test]
    fn the_grid_goes_through_the_map_of_the_overlay_into_the_screen_overlay() {
        use crate::frame::{FrameSource, OVERLAY_MAP, SCREEN_OVERLAY};
        use crate::gpu_test::block_on;
        use crate::renderer::BackendPref;
        use crate::world::overlay::cells::CellGeometry;
        let Ok(r) = block_on(Renderer::new_headless(SIZE, BackendPref::Auto)) else {
            eprintln!("SKIP the_grid_goes_through_the_map_of_the_overlay: no adapter");
            return;
        };
        let mut r = fill_world(r);
        let (_, camera) = cameras().into_iter().find(|(n, _)| *n == "wide").unwrap();
        let ui = UiDrawList::default();
        let scene = baseline_scene();
        let w = MAP.0 as f32 * 64.0;
        let sheet = CellGeometry::flat(
            [-64.0, -60.0 * MAP.1 as f32 - 40.0],
            [w + 64.0, 80.0],
            24,
            &[0.0, -w, w],
        );
        let lit =
            |img: &crate::frame::Image| img.bytes.chunks_exact(4).filter(|p| p[3] > 8).count();
        // The grid is white (grey over a clear target); the borders are the players' colours.
        let is_grey = |p: &[u8]| p[0].abs_diff(p[1]) < 3 && p[1].abs_diff(p[2]) < 3;
        let grey = |img: &crate::frame::Image| {
            img.bytes
                .chunks_exact(4)
                .filter(|p| p[3] >= 30 && is_grey(p))
                .count()
        };
        let read = |r: &mut Renderer, what| {
            let readback = r.read_frame(FrameSource::Target(what));
            r.render(&scene, &camera, &ui, None).unwrap();
            readback.wait().unwrap()
        };
        let counter = |r: &Renderer, name: &str| {
            r.last_frame_stats()
                .counters
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| *v)
        };

        // Without the cells nothing goes through the new pass: B1 draws the grid over the ground.
        assert!(!r.world().overlay.screen_overlay_active());
        let b1 = read(&mut r, SCREEN_OVERLAY);
        assert_eq!(lit(&b1), 0);
        assert_eq!(counter(&r, "overlay.map_passes"), Some(0));
        let b1_grid = r.world().overlay.stats().grid_indices;
        assert!(b1_grid > 0);

        r.world_mut().overlay.set_cell_geometry(&sheet);
        assert!(r.world().overlay.screen_overlay_active());
        let on = read(&mut r, SCREEN_OVERLAY);
        let map = read(&mut r, OVERLAY_MAP);
        assert_eq!(on.size, SIZE, "the screen's size");
        assert_eq!(map.size, (2048, 2048), "OverlayLevel 2");
        assert!(counter(&r, "overlay.map_passes") >= Some(1));
        assert!(counter(&r, "overlay.grid_hexes") > Some(20));
        assert!(
            counter(&r, "overlay.cell_draws") >= Some(3),
            "a draw per copy"
        );
        let lines = lit(&on);
        assert!(lines > 300, "the grid is in the screen overlay ({lines})");
        assert!(lit(&map) > 300, "and in the map ({})", lit(&map));
        // The colour is the file's white over a clear target, premultiplied by the texture's own
        // alpha (the white texel here), not by the file's 75 (the S1a capture, ev 741).
        let px = on
            .bytes
            .chunks_exact(4)
            .filter(|p| p[3] >= 30 && is_grey(p))
            .max_by_key(|p| p[3])
            .unwrap();
        assert!(px[3] > 200 && px[0] <= px[3].saturating_add(2), "{px:?}");
        // The borders of the players went through the same path (WOV4): solid bands of their
        // colours, and the meshes of the pass are the ones of the line over the ground.
        let solid = on
            .bytes
            .chunks_exact(4)
            .filter(|p| p[3] > 150 && !is_grey(p))
            .count();
        assert!(
            solid > 100,
            "the borders are in the screen overlay ({solid})"
        );
        assert!(counter(&r, "overlay.border_indices") > Some(0));
        assert_eq!(
            counter(&r, "overlay.border_builds"),
            Some(2),
            "a mesh per player"
        );
        if let Ok(dir) = std::env::var("RENDER_DUMP_DIR") {
            let mut ppm = format!("P6\n{} {}\n255\n", on.size.0, on.size.1).into_bytes();
            ppm.extend(on.bytes.chunks_exact(4).flat_map(|p| [p[3], p[3], p[3]]));
            std::fs::write(std::path::Path::new(&dir).join("screen_overlay.ppm"), ppm).unwrap();
            // And the colours (premultiplied, over black).
            let mut rgb = format!("P6\n{} {}\n255\n", on.size.0, on.size.1).into_bytes();
            rgb.extend(on.bytes.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]));
            std::fs::write(
                std::path::Path::new(&dir).join("screen_overlay_rgb.ppm"),
                rgb,
            )
            .unwrap();
        }

        // The grid off: nothing in either.
        r.set_grid_visible(false);
        let off = read(&mut r, SCREEN_OVERLAY);
        assert_eq!(grey(&off), 0, "the borders stay, the grid goes");
        assert!(lit(&off) > 100);
        r.set_grid_visible(true);
        // Level 1 reads the map in the final shader and has no cell pass: B1 draws the grid again.
        let mut s = r.settings().clone();
        s.set("OverlayLevel", 1).unwrap();
        r.set_settings(s);
        assert!(!r.world().overlay.screen_overlay_active());
        let _ = read(&mut r, SCREEN_OVERLAY);
        let level1 = read(&mut r, OVERLAY_MAP);
        assert_eq!(level1.size, (2048, 2048), "level 1 keeps the 2048 map");
    }

    /// WOV5: the highlights of the styles go into the map of the overlay: a fill, a contour, a spline
    /// border and a texture each make their own draw; the same set again builds nothing.
    #[test]
    fn the_styled_highlights_go_into_the_map_of_the_overlay() {
        use crate::frame::{FrameSource, OVERLAY_MAP};
        use crate::gpu_test::block_on;
        use crate::renderer::BackendPref;
        use crate::world::overlay::cells::CellGeometry;
        use crate::world::overlay::highlights::{HighlightCell, HighlightSet};
        use asset_format::{HighlightKind, HighlightStyle, LogicalId};
        let Ok(r) = block_on(Renderer::new_headless(SIZE, BackendPref::Auto)) else {
            eprintln!("SKIP the_styled_highlights: no adapter");
            return;
        };
        let mut r = fill_world(r);
        let (_, camera) = cameras().into_iter().find(|(n, _)| *n == "wide").unwrap();
        let ui = UiDrawList::default();
        let scene = baseline_scene();
        let w = MAP.0 as f32 * 64.0;
        r.world_mut().overlay.set_cell_geometry(&CellGeometry::flat(
            [-64.0, -60.0 * MAP.1 as f32 - 40.0],
            [w + 64.0, 80.0],
            24,
            &[0.0, -w, w],
        ));
        let style = |name: &str, kind, width, tex: Option<&str>, color: &[u8]| HighlightStyle {
            name: name.into(),
            kind,
            width,
            texture: tex.map(LogicalId::new),
            model: None,
            color: color.to_vec(),
            extra: Default::default(),
        };
        r.world_mut().overlay.set_highlight_styles(&[
            style(
                "Fill",
                HighlightKind::FilledHex,
                Some(1.0),
                None,
                &[255, 0, 0, 255],
            ),
            style(
                "Ring",
                HighlightKind::HexContour,
                Some(0.3),
                None,
                &[0, 255, 0, 255],
            ),
            style(
                "Border",
                HighlightKind::SplineBorder,
                Some(6.0),
                None,
                &[0, 0, 255, 255],
            ),
        ]);
        let cell = |style: &str, x, y| HighlightCell {
            style: style.into(),
            x,
            y,
            color: None,
        };
        let set = HighlightSet {
            cells: vec![
                cell("Fill", 3, 3),
                cell("Ring", 6, 3),
                cell("Border", 9, 3),
                cell("Border", 10, 3),
            ],
        };
        assert!(r.set_highlight_set(set.clone()));
        let readback = r.read_frame(FrameSource::Target(OVERLAY_MAP));
        r.render(&scene, &camera, &ui, None).unwrap();
        let map = readback.wait().unwrap();
        let counter = |r: &Renderer, name: &str| {
            r.last_frame_stats()
                .counters
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| *v)
        };
        assert_eq!(counter(&r, "overlay.style_builds"), Some(1));
        assert!(counter(&r, "overlay.style_indices") > Some(0));
        // A plain-coloured style makes no texture lookup: the pixels are its colour, premultiplied.
        let count = |f: fn(&[u8]) -> bool| map.bytes.chunks_exact(4).filter(|p| f(p)).count();
        assert!(
            count(|p| p[0] > 200 && p[1] < 20 && p[3] > 200) > 100,
            "the red fill"
        );
        assert!(
            count(|p| p[1] > 200 && p[0] < 20 && p[3] > 200) > 100,
            "the green ring"
        );
        assert!(
            count(|p| p[2] > 200 && p[0] < 20 && p[3] > 200) > 100,
            "the blue border"
        );
        // The same set again: no new mesh.
        assert!(!r.set_highlight_set(set));
        r.render(&scene, &camera, &ui, None).unwrap();
        assert_eq!(counter(&r, "overlay.style_builds"), Some(1));
    }

    /// WOV5: the selection ring of the selected cell goes into the map of the overlay in its colour
    /// (golden S2: the ring of `PSValidMoves` around the selected stack).
    #[test]
    fn the_selection_ring_goes_into_the_map_of_the_overlay() {
        use crate::entities::Highlights;
        use crate::frame::{FrameSource, OVERLAY_MAP};
        use crate::gpu_test::block_on;
        use crate::renderer::BackendPref;
        use crate::world::overlay::cells::CellGeometry;
        use asset_format::{LogicalId, TextureBlob, TextureFormat};
        let Ok(r) = block_on(Renderer::new_headless(SIZE, BackendPref::Auto)) else {
            eprintln!("SKIP the_selection_ring: no adapter");
            return;
        };
        let mut r = fill_world(r);
        let (_, camera) = cameras().into_iter().find(|(n, _)| *n == "wide").unwrap();
        let ui = UiDrawList::default();
        let scene = baseline_scene();
        let w = MAP.0 as f32 * 64.0;
        r.world_mut().overlay.set_cell_geometry(&CellGeometry::flat(
            [-64.0, -60.0 * MAP.1 as f32 - 40.0],
            [w + 64.0, 80.0],
            24,
            &[0.0, -w, w],
        ));
        // A ring of full alpha between radii 0.15 and 0.25 of the texture, a glow of none.
        let n = 64usize;
        let ring: Vec<u8> = (0..n * n)
            .flat_map(|i| {
                let (x, y) = ((i % n) as f32 + 0.5, (i / n) as f32 + 0.5);
                let d = ((x / n as f32 - 0.5).powi(2) + (y / n as f32 - 0.5).powi(2)).sqrt();
                let a = if (0.15..0.25).contains(&d) { 255 } else { 0 };
                [255, 255, 255, a]
            })
            .collect();
        let blob = |px: Vec<u8>| {
            TextureBlob::new_2d(TextureFormat::Rgba8, false, n as u32, n as u32, vec![px])
        };
        let textures = std::collections::HashMap::from([
            (
                LogicalId::new(crate::world::overlay::ring::TEXTURES[0]),
                blob(ring),
            ),
            (
                LogicalId::new(crate::world::overlay::ring::TEXTURES[1]),
                blob(
                    vec![255; n * n * 4]
                        .chunks(4)
                        .flat_map(|_| [255, 255, 255, 0])
                        .collect(),
                ),
            ),
        ]);
        assert_eq!(r.set_ring_textures(&textures), 2);
        r.set_highlights(Highlights {
            selected: Some((4, 3)),
            ..Default::default()
        });
        let mut yellow_at = |angle: f32| {
            r.set_ring_pin(Some((angle, 0.0)));
            // The first frame takes the ring's state, the second draws it.
            r.render(&scene, &camera, &ui, None).unwrap();
            let readback = r.read_frame(FrameSource::Target(OVERLAY_MAP));
            r.render(&scene, &camera, &ui, None).unwrap();
            let map = readback.wait().unwrap();
            // The colour of `UnitSelectionHighlight` (0.9, 0.9, 0.45), premultiplied.
            map.bytes
                .chunks_exact(4)
                .filter(|p| p[3] > 200 && p[0] > 200 && p[1] > 200 && p[2] > 90 && p[2] < 140)
                .count()
        };
        let still = yellow_at(0.0);
        assert!(still > 100, "the ring: {still} pixels");
        // A round ring turned about the middle of its texture covers the same pixels (golden S2: the
        // turn of -1.21 rad left a sliver while the registers were read transposed).
        let turned = yellow_at(-1.21);
        let ratio = turned as f32 / still as f32;
        assert!((0.9..1.1).contains(&ratio), "{still} against {turned}");
    }

    /// WOV3: the minimap target holds the explored map: water, land, a fog over what is unexplored,
    /// and the rectangle of the camera. `RENDER_DUMP_DIR` writes the picture as a PPM for a look.
    #[test]
    fn the_minimap_draws_the_explored_map() {
        use crate::frame::{FrameSource, MINIMAP};
        use crate::gpu_test::block_on;
        use crate::renderer::BackendPref;
        let Ok(r) = block_on(Renderer::new_headless(SIZE, BackendPref::Auto)) else {
            eprintln!("SKIP the_minimap_draws_the_explored_map: no adapter");
            return;
        };
        let mut r = fill_world(r);
        let (_, camera) = cameras().into_iter().find(|(n, _)| *n == "wide").unwrap();
        let ui = UiDrawList::default();
        let scene = baseline_scene();
        let readback = r.read_frame(FrameSource::Target(MINIMAP));
        r.render(&scene, &camera, &ui, None).unwrap();
        let img = readback.wait().unwrap();
        assert_eq!(img.size, (320, 190));
        if let Ok(dir) = std::env::var("RENDER_DUMP_DIR") {
            let mut ppm = format!(
                "P6
{} {}
255
",
                img.size.0, img.size.1
            )
            .into_bytes();
            ppm.extend(img.bytes.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]));
            std::fs::write(std::path::Path::new(&dir).join("minimap.ppm"), ppm).unwrap();
        }
        let px = |x: u32, y: u32| {
            let p = img.pixel(x, y);
            [p[0], p[1], p[2], p[3]]
        };
        let mut seen = std::collections::HashSet::new();
        for y in (0..190).step_by(7) {
            for x in (0..320).step_by(7) {
                seen.insert(px(x, y));
            }
        }
        assert!(seen.len() > 3, "more than one flat colour ({})", seen.len());
        // Opaque everywhere: the target is cleared with the fog's alpha and the layers cover it.
        assert!(seen.iter().all(|p| p[3] == 255));
    }

    /// WOB0: the keyed inputs. A changed unit is placed alone, the cities and the plots are not
    /// touched, the other units keep their instances, and the counters of `?stats=1` say so.
    #[test]
    fn a_changed_unit_is_placed_alone() {
        use crate::frame::SystemId;
        use crate::world::objects::input::EntityChanges;
        let Some(gpu) = gpu(
            "a_changed_unit_is_placed_alone",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let mut r = world(&gpu, F::Rgba8Unorm, 1);
        let stat = |r: &Renderer, id: SystemId| {
            r.object_stats()
                .into_iter()
                .find(|o| o.system == id)
                .unwrap()
                .counters
        };
        let (units, cities, plots) = (
            stat(&r, SystemId::Unit),
            stat(&r, SystemId::City),
            stat(&r, SystemId::Landmark),
        );
        assert_eq!((units.records, cities.records, plots.records), (3, 2, 2));
        assert!(units.instances > 0 && cities.instances > 0);

        let mut moved = entities().units[0].clone();
        moved.x += 1;
        r.apply_entity_changes(EntityChanges {
            units: vec![moved.clone()],
            ..EntityChanges::default()
        });
        let after = stat(&r, SystemId::Unit);
        assert_eq!(after.placed_last, 1);
        assert_eq!(after.placed_total, units.placed_total + 1);
        assert_eq!(
            after.instances, units.instances,
            "the same squads, elsewhere"
        );
        let city_after = stat(&r, SystemId::City);
        assert_eq!(city_after.placed_total, cities.placed_total);
        assert_eq!(city_after.placed_last, 0);
        assert_eq!(
            stat(&r, SystemId::Landmark).placed_total,
            plots.placed_total
        );

        // The same record again, and the same snapshot again, change nothing.
        r.apply_entity_changes(EntityChanges {
            units: vec![moved],
            ..EntityChanges::default()
        });
        r.set_entities({
            let mut s = entities();
            s.units[0].x += 1;
            s
        });
        let again = stat(&r, SystemId::Unit);
        assert_eq!(again.placed_total, after.placed_total);
        assert!(r.last_frame_stats().objects.is_empty(), "no frame yet");
        let camera = &cameras()[0].1;
        frame(&gpu, &mut r, camera, F::Rgba8Unorm);
        assert_eq!(r.last_frame_stats().objects.len(), 5);
        assert!(r.last_frame_stats().object_table().contains("Unit"));
    }

    /// New settings reach a running renderer: the keys are kept, the frame goes on, and the main
    /// pass keeps the samples it was made for (an MSAA change waits for the next start).
    #[test]
    fn new_settings_reach_a_running_renderer() {
        use crate::settings::{Preset, RenderSettings};
        let Some(gpu) = gpu(
            "new_settings_reach_a_running_renderer",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let mut r = world(&gpu, F::Rgba8Unorm, 4);
        let camera = &cameras()[0].1;
        let before = frame(&gpu, &mut r, camera, F::Rgba8Unorm);
        assert_eq!(r.settings().effective_msaa(), 4);
        r.set_settings(RenderSettings::preset(Preset::Low));
        assert_eq!(r.settings(), &RenderSettings::preset(Preset::Low));
        assert_eq!(r.settings().effective_msaa(), 1);
        let after = frame(&gpu, &mut r, camera, F::Rgba8Unorm);
        // The subsystems have no body for these keys yet; the picture stays and does not fail.
        assert!(before == after);
        r.set_settings(RenderSettings::preset(Preset::High));
        assert_eq!(r.settings().msaa_samples, 4);
    }

    /// WOV0: the fog and the owners go to the subsystems that own them; the same input again does
    /// nothing, one changed cell is one texel, and a changed owner builds the borders again.
    #[test]
    fn fog_and_owners_do_only_what_changed() {
        use crate::world::fow::FogInput;
        use crate::world::overlay::OwnersInput;
        let Some(gpu) = gpu(
            "fog_and_owners_do_only_what_changed",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let mut r = world(&gpu, F::Rgba8Unorm, 1);
        let camera = &cameras()[0].1;
        let first = frame(&gpu, &mut r, camera, F::Rgba8Unorm);
        let fog0 = r.world().fluid_fow.stats();
        let own0 = r.world().overlay.stats();
        assert_eq!(fog0.updates, 1, "set_terrain seeded the fog once");
        assert_eq!(own0.ground_builds, 1);

        // The feed's own input is the seed: nothing happens.
        let snapshot = terrain();
        let fog = FogInput::from_snapshot(&snapshot);
        r.set_fog(fog.clone());
        r.set_owners(OwnersInput::from_snapshot(&snapshot));
        assert_eq!(r.world().fluid_fow.stats().updates, 1);
        assert_eq!(r.world().overlay.stats().owner_updates, own0.owner_updates);

        // One cell is revealed: one texel.
        let mut next = fog.clone();
        // The mountain at (9, 5) in the middle of the hidden columns.
        let at = 5 * MAP.0 as usize + 9;
        assert_eq!(next.cells[at], Fog::Hidden);
        next.cells[at] = Fog::Visible;
        r.set_fog(next);
        let fog1 = r.world().fluid_fow.stats();
        assert_eq!((fog1.updates, fog1.last_texels), (2, 1));
        let second = frame(&gpu, &mut r, camera, F::Rgba8Unorm);
        assert!(second != first, "the cell looks different");

        // One cell changes hands: the borders are built again.
        let mut owners = OwnersInput::from_snapshot(&snapshot);
        let free = owners.owners.iter().position(|&o| o == -1).unwrap();
        owners.owners[free] = 1;
        r.set_owners(owners);
        frame(&gpu, &mut r, camera, F::Rgba8Unorm);
        assert_eq!(
            r.world().overlay.stats().ground_builds,
            own0.ground_builds + 1
        );
    }

    /// WOV0: `?stats=1` lists what the overlay subsystems did, and only for those that are on.
    #[test]
    fn the_overlay_subsystems_report_counters() {
        let Some(gpu) = gpu(
            "the_overlay_subsystems_report_counters",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let mut r = world(&gpu, F::Rgba8Unorm, 1);
        let camera = &cameras()[0].1;
        frame(&gpu, &mut r, camera, F::Rgba8Unorm);
        let stats = r.last_frame_stats().clone();
        let get = |name: &str| {
            stats
                .counters
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| *v)
        };
        assert_eq!(get("fog.updates"), Some(1));
        assert!(
            get("overlay.ground_indices").unwrap() > 0,
            "rivers and borders"
        );
        assert!(get("overlay.grid_indices").unwrap() > 0, "the grid");
        assert_eq!(get("path.steps"), Some(3));
        assert_eq!(get("minimap.fills"), Some(1));
        assert!(stats.pass_table().contains("counters"));
        // Switched off: no counters.
        r.set_layers(LayerSwitches::from_names("Overlay,MovementPath").0);
        frame(&gpu, &mut r, camera, F::Rgba8Unorm);
        let off = r.last_frame_stats().clone();
        assert!(
            off.counters
                .iter()
                .all(|(n, _)| !n.starts_with("overlay.") && !n.starts_with("path."))
        );
        assert!(off.counters.iter().any(|(n, _)| n.starts_with("fog.")));
    }

    /// The path has its own input: its labels come from `set_path` alone, and the selected plot
    /// from `set_highlights`.
    #[test]
    fn the_path_has_its_own_input() {
        use crate::world::path::PathInput;
        let Some(gpu) = gpu(
            "the_path_has_its_own_input",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let mut r = world(&gpu, F::Rgba8Unorm, 1);
        let camera = &cameras()[0].1;
        frame(&gpu, &mut r, camera, F::Rgba8Unorm);
        assert_eq!(
            r.path_labels().len(),
            2,
            "a marker where the turn changes, and at the end"
        );
        r.set_path(PathInput::default());
        frame(&gpu, &mut r, camera, F::Rgba8Unorm);
        assert!(r.path_labels().is_empty());
        // The selected plot stayed.
        assert_eq!(r.world().overlay.highlights().selected, Some((5, 4)));
        r.set_path(PathInput {
            steps: highlights().path,
        });
        frame(&gpu, &mut r, camera, F::Rgba8Unorm);
        assert_eq!(r.path_labels().len(), 2);
    }
}
