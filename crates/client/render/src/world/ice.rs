//! `Ice` subsystem (world-water WW-8, card R5.8): the polar ice,
//! `cvTerrainIceSystem` of the original. The floes are `water/ice.rs`; this draws them with the
//! ported `IceShader` (`shaders/water/ice.wgsl`, `RenderSceneVS` and `PS_Ice`) in `Main.Terrain`
//! after the water, one draw per copy of the map.
//!
//! The renderer feeds it ([`Ice::feed`]) with what the rivers take from the water surface (the
//! camera, the ramp of the fog, the colour key and the mask of the fog of war), the pack and the
//! sea level; [`Ice::set_map`] makes the floes once a map. Nothing is drawn until all of that is in.
//!
//! Constants (rw-a6-answers RW-22 (b) 5, `IceGlobalConstants`): the colours of `IceSettings.xml`
//! go in as `pow(v / 255, 2.2)`, the ramps as `1 / v`; `g_fLeanGradientScale` is the
//! `-output_scale` of `ice_bump.lean`; the sun and the fog are the water's.
//!
//! SIMPLIFIED(SIMP-043): the reflection pass of the original draws the ice again with the camera
//! of the reflection (`RenderReflections` 0x08675a18); ours leaves it out of the reflection. The
//! base layer of the fog of war (`FOWBaseLayer`) is black.

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

use crate::frame::{
    DEPTH_FORMAT, DrawContext, InitContext, MainSlot, PassId, PassSlot, PrepareContext,
    RenderSystem, SystemId, WORLD_FORMAT, multisample,
};
use crate::settings::{KeySet, RenderSettings};
use crate::shaders::create_module;
use crate::texture::{UploadOptions, upload_blob};
use crate::world::terrain::basis::{BasisFeature, TerrainBasis};
use crate::world::terrain::sampler::TerrainFrame;
use crate::world::water::data::{
    ICE_BUMP, ICE_BUMP_SIDES, ICE_BUMP_SIDES_CROSS, ICE_CUBE, ICE_DIFFUSE, ICE_DIFFUSE_SIDES,
    ICE_DISPLACEMENT, WaterData,
};
use crate::world::water::ice::{IceBuild, IceMesh, IceVertex, params_for};
use crate::world::water::ocean::{
    FOG_COLOR, OVERLAY_ROWS, SUN_DIR, copy_slot, fog_consts_for_eye_z, one_texel, sampler_entry,
    texture_entry,
};
use crate::world::water::river::draw::RiverFeed;

const TERRAIN_DETAIL: &str = "TerrainDetailLevel";
const COPIES: usize = 3;
/// The time a frame gives to making the floes.
const BUILD_BUDGET_MS: f64 = 2.5;
const STRIDE: u64 = 512;

/// `IceVsConsts` of ice.wgsl (c0..c13).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct IceVsConsts {
    view_proj: [f32; 16],
    bbox_min: [f32; 4],
    bbox_max: [f32; 4],
    height_scale: [f32; 4],
    height_bias: [f32; 4],
    overlay: [f32; 16],
    eye: [f32; 4],
    fow_base_scale: [f32; 4],
}

/// `IcePsConsts` of ice.wgsl (c0..c17).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct IcePsConsts {
    regs: [[f32; 4]; 18],
}

/// `pow(v / 255, 2.2)` of a colour of `IceSettings.xml`.
// re: IceSettings::ReadXML 0x0890a86c (RW-22 (b) 1: colours are stored as pow(v/255, 2.2))
fn linear(c: [u8; 3]) -> [f32; 4] {
    let l = |v: u8| (f32::from(v) / 255.0).powf(2.2);
    [l(c[0]), l(c[1]), l(c[2]), 0.0]
}

/// `1 / v` (the ramps of the settings); zero stays zero.
fn inverse(v: f32) -> f32 {
    if v != 0.0 { 1.0 / v } else { 0.0 }
}

/// The constants of the pixel shader, in the order of `IcePsConsts`.
fn ps_consts(p: &asset_format::IceParams, eye: Vec3, sea: f32, gradient_scale: f32) -> IcePsConsts {
    let s = |v: f32| [v, 0.0, 0.0, 0.0];
    IcePsConsts {
        regs: [
            [eye.x, eye.y, eye.z, 0.0],
            [0.0; 4],
            [SUN_DIR[0], SUN_DIR[1], SUN_DIR[2], 0.0],
            [SUN_DIR[0], SUN_DIR[1], SUN_DIR[2], 0.0],
            s(gradient_scale),
            s(p.texture_repeat),
            s(p.bump_repeat),
            s(p.specular_power.max(1e-3)),
            linear(p.subsurface),
            s(sea),
            [p.side_texture_repeat_u, p.side_texture_repeat_v, 0.0, 0.0],
            [p.side_bump_repeat_u, p.side_bump_repeat_v, 0.0, 0.0],
            linear(p.underwater_1),
            linear(p.underwater_2),
            s(inverse(p.underwater_color_ramp)),
            s(inverse(p.underwater_alpha_ramp)),
            FOG_COLOR,
            fog_consts_for_eye_z(eye.z),
        ],
    }
}

/// What the renderer gives the ice each frame.
#[derive(Clone)]
pub struct IceFeed {
    /// What the rivers take from the surface: camera, map, fog ramp, colour key, mask.
    pub water: RiverFeed,
    pub data: Arc<WaterData>,
    /// The sea level of the original's world.
    pub sea_level: f32,
}

/// The textures of the pack, in the order of the bindings 2, 4, 6, 8, 10, 12.
const PACK: [&str; 6] = [
    ICE_CUBE,
    ICE_DIFFUSE,
    ICE_BUMP,
    ICE_DIFFUSE_SIDES,
    ICE_BUMP_SIDES,
    ICE_BUMP_SIDES_CROSS,
];

/// The colour textures of the pack the original samples through sampler 2 (wrap, sRGB decode): the
/// environment cube and the two diffuse maps; the bumps go up raw (sampler 0).
// re: cvTerrainIceSystem::BuildIceMeshes 0x0867456e (slots 0, 1, 4 with sampler 2; re-b16 I1)
fn srgb_in_original(file: &str) -> bool {
    matches!(file, ICE_CUBE | ICE_DIFFUSE | ICE_DIFFUSE_SIDES)
}

/// The blob as it goes up: marked sRGB where the original decodes it, with the options that keep
/// the mark.
fn pack_upload(
    file: &str,
    blob: &asset_format::TextureBlob,
) -> (asset_format::TextureBlob, UploadOptions) {
    if srgb_in_original(file) {
        let mut b = blob.clone();
        b.srgb = true;
        (
            b,
            UploadOptions {
                linear: false,
                ..UploadOptions::WORLD
            },
        )
    } else {
        (blob.clone(), UploadOptions::WORLD)
    }
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    wrap: wgpu::Sampler,
    clamp: wgpu::Sampler,
    vs: Buf,
    ps: Buf,
    vertices: Option<Buf>,
    indices: Option<Buf>,
    index_count: u32,
    /// The revision of the data and the views of [`PACK`].
    pack: Option<(u64, Vec<wgpu::TextureView>)>,
    black: wgpu::TextureView,
    visible: wgpu::TextureView,
    group: Option<(u64, wgpu::BindGroup)>,
}

impl Gpu {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, samples: u32) -> Self {
        let module = create_module(
            device,
            "water ice",
            crate::wgsl_module!("common/sm3.wgsl", "water/ice.wgsl"),
        );
        let uniform = |binding, size: u64| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: wgpu::BufferSize::new(size),
            },
            count: None,
        };
        let mut entries = vec![
            uniform(0, std::mem::size_of::<IceVsConsts>() as u64),
            uniform(1, std::mem::size_of::<IcePsConsts>() as u64),
        ];
        use wgpu::TextureViewDimension as D;
        for (binding, dim) in [
            (2, D::Cube),
            (4, D::D2),
            (6, D::D2),
            (8, D::D2),
            (10, D::D2),
            (12, D::D2),
            (14, D::D2),
            (16, D::D2),
            (18, D::D2),
            (20, D::D3),
        ] {
            entries.push(texture_entry(binding, dim));
            entries.push(sampler_entry(binding + 1));
        }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ice"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ice"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];
        // re: RW-22 (b) 5: command set "Ice", ONE / INVSRCALPHA, ADD.
        let blend = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ice"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_ice"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<IceVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: multisample(samples),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("ps_ice"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: WORLD_FORMAT,
                    blend: Some(wgpu::BlendState {
                        color: blend,
                        alpha: blend,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = |label, mode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: mode,
                address_mode_v: mode,
                address_mode_w: mode,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            })
        };
        let buffer = |label| {
            device.buf(
                &wgpu::BufferDescriptor {
                    label: Some(label),
                    size: STRIDE * COPIES as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                },
                Cluster::Other,
            )
        };
        let rgba = wgpu::TextureFormat::Rgba8Unorm;
        let d2 = wgpu::TextureDimension::D2;
        Self {
            device: device.clone(),
            queue: queue.clone(),
            layout,
            pipeline,
            wrap: sampler("ice wrap", wgpu::AddressMode::Repeat),
            clamp: sampler("ice clamp", wgpu::AddressMode::ClampToEdge),
            vs: buffer("ice vs"),
            ps: buffer("ice ps"),
            vertices: None,
            indices: None,
            index_count: 0,
            pack: None,
            black: one_texel(device, queue, "ice black", rgba, [0, 0, 0, 255], d2),
            visible: one_texel(device, queue, "ice visible", rgba, [255, 0, 255, 255], d2),
            group: None,
        }
    }

    fn set_mesh(&mut self, mesh: &IceMesh) {
        self.index_count = mesh.indices.len() as u32;
        if mesh.is_empty() {
            self.vertices = None;
            self.indices = None;
            return;
        }
        let init = |label, contents: &[u8], usage| {
            self.device.buf_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents,
                    usage,
                },
                Cluster::Other,
            )
        };
        let vertices = init(
            "ice vertices",
            bytemuck::cast_slice(&mesh.vertices),
            wgpu::BufferUsages::VERTEX,
        );
        let indices = init(
            "ice indices",
            bytemuck::cast_slice(&mesh.indices),
            wgpu::BufferUsages::INDEX,
        );
        self.vertices = Some(vertices);
        self.indices = Some(indices);
    }

    /// The textures of the pack, uploaded again when the data moved.
    fn set_pack(&mut self, data: &WaterData) -> bool {
        if self.pack.as_ref().map(|p| p.0) == Some(data.revision()) {
            return true;
        }
        let mut views = Vec::with_capacity(PACK.len());
        for file in PACK {
            let Some(blob) = data.texture(file) else {
                return false;
            };
            let (blob, options) = pack_upload(file, blob);
            match upload_blob(&self.device, &self.queue, &blob, file, options) {
                Ok(u) => views.push(u.texture.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(u.view_dimension),
                    ..Default::default()
                })),
                Err(e) => {
                    tracing::warn!(error = %e, file, "ice: texture not uploaded");
                    return false;
                }
            }
        }
        self.pack = Some((data.revision(), views));
        self.group = None;
        true
    }

    fn bind(&mut self, feed: &IceFeed, key: u64) -> bool {
        if self.group.as_ref().map(|(k, _)| *k) == Some(key) {
            return true;
        }
        let Some((_, pack)) = &self.pack else {
            return false;
        };
        let t = &feed.water.textures;
        let views: [(&wgpu::TextureView, bool); 10] = [
            (&pack[0], false),
            (&pack[1], false),
            (&pack[2], false),
            (&pack[3], false),
            (&pack[4], false),
            (&pack[5], false),
            (&t.fog_ramp, true),
            (t.fow.as_ref().unwrap_or(&self.visible), false),
            (&self.black, false),
            (&t.lut, true),
        ];
        let part = |buffer, size: usize| {
            wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset: 0,
                size: wgpu::BufferSize::new(size as u64),
            })
        };
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: part(&self.vs, std::mem::size_of::<IceVsConsts>()),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: part(&self.ps, std::mem::size_of::<IcePsConsts>()),
            },
        ];
        for (i, (view, clamped)) in views.iter().enumerate() {
            let binding = 2 + 2 * i as u32;
            entries.push(wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: binding + 1,
                resource: wgpu::BindingResource::Sampler(if *clamped {
                    &self.clamp
                } else {
                    &self.wrap
                }),
            });
        }
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ice"),
            layout: &self.layout,
            entries: &entries,
        });
        self.group = Some((key, group));
        true
    }
}

/// The polar ice.
pub struct Ice {
    gpu: Option<Gpu>,
    feed: Option<IceFeed>,
    mesh: IceMesh,
    made_for: Option<u64>,
    /// The floes being made for this fingerprint.
    building: Option<(u64, IceBuild)>,
    mesh_dirty: bool,
    /// `TerrainDetailLevel`: below 2 the low settings.
    terrain_detail: i64,
    drawn: std::cell::Cell<u32>,
}

impl Default for Ice {
    fn default() -> Self {
        Self {
            gpu: None,
            feed: None,
            mesh: IceMesh::default(),
            made_for: None,
            building: None,
            mesh_dirty: false,
            terrain_detail: 3,
            drawn: std::cell::Cell::new(0),
        }
    }
}

impl Ice {
    /// Takes the device (once, after it exists).
    pub fn bind_device(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, samples: u32) {
        self.gpu = Some(Gpu::new(device, queue, samples));
        self.mesh_dirty = true;
    }

    /// What the frame gives the ice; `None` while the water surface cannot share its inputs.
    pub fn feed(&mut self, feed: Option<IceFeed>) {
        self.feed = feed;
    }

    /// `TerrainDetailLevel` before the first `on_settings`.
    pub fn set_terrain_detail(&mut self, level: i64) {
        if self.terrain_detail != level {
            self.terrain_detail = level;
            self.made_for = None;
        }
    }

    /// The map: the floes are made again when its ice cells, its frame or the settings changed
    /// (`BuildIce` runs once a map in the original). Call after [`Self::feed`].
    pub fn set_map(&mut self, basis: &TerrainBasis, frame: &TerrainFrame) {
        let Some(feed) = &self.feed else { return };
        if !feed.data.has_settings() {
            return;
        }
        let params = params_for(feed.data.ice(), self.terrain_detail);
        let disp = feed.data.texture(ICE_DISPLACEMENT);
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (basis.width, basis.height, basis.wrap_x).hash(&mut h);
            for p in &basis.plots {
                (p.feature == BasisFeature::Ice).hash(&mut h);
            }
            (frame.width_hexes(), frame.height_hexes()).hash(&mut h);
            format!("{params:?}").hash(&mut h);
            feed.sea_level.to_bits().hash(&mut h);
            disp.is_some().hash(&mut h);
            h.finish()
        };
        if self.made_for == Some(key) {
            return;
        }
        // The floes are made a few milliseconds a frame; the old ones stay until the new are whole.
        let building = match self.building.take() {
            Some((k, b)) if k == key => b,
            _ => IceBuild::new(basis, frame, params, feed.sea_level, disp),
        };
        let mut building = building;
        if building.step(BUILD_BUDGET_MS) {
            self.mesh = building.finish();
            self.made_for = Some(key);
            self.mesh_dirty = true;
        } else {
            self.building = Some((key, building));
        }
    }

    /// The floes of the map, for the tests.
    pub fn mesh(&self) -> &IceMesh {
        &self.mesh
    }

    pub fn drawn(&self) -> u32 {
        self.drawn.get()
    }
}

impl RenderSystem for Ice {
    fn id(&self) -> SystemId {
        SystemId::Ice
    }

    fn passes(&self) -> &'static [PassSlot] {
        &[
            PassSlot::Pass(PassId::Reflection),
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
        if changed.contains(TERRAIN_DETAIL) {
            self.set_terrain_detail(settings.get(TERRAIN_DETAIL).unwrap_or(3));
        }
    }

    fn prepare(&mut self, _cx: &mut PrepareContext<'_>) {
        self.drawn.set(0);
        let (Some(gpu), Some(feed)) = (&mut self.gpu, &self.feed) else {
            return;
        };
        if self.mesh_dirty {
            gpu.set_mesh(&self.mesh);
            self.mesh_dirty = false;
        }
        if gpu.index_count == 0 || !feed.data.missing_for_ice().is_empty() {
            return;
        }
        if !gpu.set_pack(&feed.data) {
            return;
        }
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            feed.data.revision().hash(&mut h);
            let t = &feed.water.textures;
            format!("{:?}{:?}{:?}", t.fog_ramp, t.lut, t.fow).hash(&mut h);
            h.finish()
        };
        if !gpu.bind(feed, key) {
            return;
        }
        let params = params_for(feed.data.ice(), self.terrain_detail);
        let gradient_scale = feed.data.lean_scale(ICE_BUMP).unwrap_or(1.0);
        let f = &feed.water.frame;
        let orig_to_ours = f.ours_to_orig.inverse();
        let size = feed.water.map.world_size().max(glam::Vec2::ONE);
        let half = feed.water.map.half_world;
        let overlay = f
            .overlay_proj
            .unwrap_or_else(|| Mat4::from_cols_array_2d(&OVERLAY_ROWS));
        for shift in feed.water.map.shifts() {
            let slot = copy_slot(shift).min(COPIES - 1);
            // As the ocean's `OverlayProjection`: the mask of the fog is read where the copy draws the point.
            let overlay: [f32; 16] = bytemuck::cast(crate::lighting::globals::row_major(
                overlay * crate::world::water::ocean::copy_translation(f, shift),
            ));
            let vp = f.view_proj * Mat4::from_translation(Vec3::X * shift) * orig_to_ours;
            let eye = f.ours_to_orig.transform_point3(f.eye - Vec3::X * shift);
            let m = &self.mesh;
            let vs = IceVsConsts {
                // `row_major float4x4`: register k is column k of the plain matrix.
                view_proj: vp.transpose().to_cols_array(),
                bbox_min: [m.bbox_min.x, m.bbox_min.y, 0.0, 0.0],
                bbox_max: [m.bbox_max.x, m.bbox_max.y, 0.0, 0.0],
                height_scale: [m.height_scale, 0.0, 0.0, 0.0],
                height_bias: [m.height_bias, 0.0, 0.0, 0.0],
                overlay,
                eye: [eye.x, eye.y, eye.z, 0.0],
                fow_base_scale: [1.0 / size.x, -1.0 / size.y, half.x, -half.y],
            };
            let ps = ps_consts(params, eye, feed.sea_level, gradient_scale);
            gpu.queue
                .write_buffer(&gpu.vs, slot as u64 * STRIDE, bytemuck::bytes_of(&vs));
            gpu.queue
                .write_buffer(&gpu.ps, slot as u64 * STRIDE, bytemuck::bytes_of(&ps));
        }
    }

    fn record(&self, slot: PassSlot, cx: &mut DrawContext<'_, '_>) {
        if slot != PassSlot::Main(MainSlot::Terrain) || self.feed.is_none() {
            return;
        }
        let Some(gpu) = &self.gpu else { return };
        let (Some((_, group)), Some(vertices), Some(indices)) =
            (&gpu.group, &gpu.vertices, &gpu.indices)
        else {
            return;
        };
        if gpu.index_count == 0 {
            return;
        }
        let offset = (copy_slot(cx.copy_shift).min(COPIES - 1) as u64 * STRIDE) as u32;
        cx.pass.set_pipeline_c(&gpu.pipeline);
        cx.pass.set_bind_group_c(0, group, &[offset, offset]);
        cx.pass.set_vertex_buffer(0, vertices.slice(..));
        cx.pass
            .set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        cx.pass.draw_indexed_c(0..gpu.index_count, 0, 0..1);
        cx.stats.record_draw(1);
        self.drawn.set(self.drawn.get() + 1);
    }

    fn counters(&self) -> Vec<(&'static str, u64)> {
        vec![
            ("ice.floes", self.mesh.floes as u64),
            ("ice.drawn", u64::from(self.drawn.get())),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// re-b16 I1: the cube and the two diffuse maps (BC1 and BC3 in the pack) go up in an sRGB
    /// format, as the original's sampler 2 decodes them; the LEAN bumps stay raw.
    #[test]
    fn the_ice_colour_maps_are_sampled_through_srgb_views_and_the_bumps_raw() {
        let blob = |format| asset_format::TextureBlob {
            format,
            dim: asset_format::TexDim::D2,
            srgb: false,
            width: 4,
            height: 4,
            depth: 1,
            mips: vec![vec![0; 16]],
        };
        let format_of = |file: &str, f| {
            let (b, o) = pack_upload(file, &blob(f));
            crate::texture::wgpu_format(b.format, b.srgb && !o.linear)
        };
        use asset_format::TextureFormat::{Bc1, Bc3, Rgba8};
        assert_eq!(
            format_of(ICE_CUBE, Bc1),
            wgpu::TextureFormat::Bc1RgbaUnormSrgb
        );
        assert_eq!(
            format_of(ICE_DIFFUSE, Bc3),
            wgpu::TextureFormat::Bc3RgbaUnormSrgb
        );
        assert_eq!(
            format_of(ICE_DIFFUSE_SIDES, Bc3),
            wgpu::TextureFormat::Bc3RgbaUnormSrgb
        );
        for bump in [ICE_BUMP, ICE_BUMP_SIDES, ICE_BUMP_SIDES_CROSS] {
            assert_eq!(format_of(bump, Rgba8), wgpu::TextureFormat::Rgba8Unorm);
        }
    }

    #[test]
    fn the_colours_go_in_linear_and_the_ramps_inverted() {
        let p = asset_format::IceParams {
            subsurface: [255, 0, 255],
            underwater_color_ramp: 10.0,
            underwater_alpha_ramp: 0.0,
            specular_power: 150.0,
            ..Default::default()
        };
        let c = ps_consts(&p, Vec3::new(1.0, 2.0, 3.0), 10.12, 0.5);
        assert_eq!(c.regs[8][..3], [1.0, 0.0, 1.0]);
        assert!((c.regs[14][0] - 0.1).abs() < 1e-6);
        assert_eq!(c.regs[15][0], 0.0);
        assert_eq!(c.regs[4][0], 0.5);
        assert_eq!(c.regs[9][0], 10.12);
        assert_eq!(c.regs[0][..3], [1.0, 2.0, 3.0]);
        assert_eq!(std::mem::size_of::<IcePsConsts>(), 18 * 16);
        assert!(std::mem::size_of::<IceVsConsts>() as u64 <= STRIDE);
    }

    #[test]
    fn a_lower_terrain_detail_makes_the_floes_again() {
        let mut ice = Ice {
            made_for: Some(1),
            ..Ice::default()
        };
        ice.set_terrain_detail(1);
        assert_eq!(ice.made_for, None);
        ice.made_for = Some(1);
        ice.set_terrain_detail(1);
        assert_eq!(ice.made_for, Some(1));
    }
}
