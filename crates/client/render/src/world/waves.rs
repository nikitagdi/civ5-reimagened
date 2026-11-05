//! `Waves` subsystem (world-water WW-7, card R5.8): the surf of the coast,
//! `cvWaveSystem` of the original. The waves themselves are `water/waves.rs`; this draws them with
//! the ported `WaveShader` (`shaders/water/waves.wgsl`) in `Main.Waves`, one call per batch of 248
//! waves and per copy of the map, the texture `wave_coastline_all.dds` in slot 0.
//!
//! The renderer feeds it ([`Waves::feed`]) with the basis and the frame of the map (the waves are
//! made once a map, as `SpawnWaves` at the start of a game), the settings and the texture of the
//! pack, and the camera. Nothing is drawn until all of that is in.
//!
//! Constants of `VS_Wave` (rw-a6-answers RW-22 (a) 4): `g_vWaveData = (scale * StartDistance,
//! scale * EndDistance, 1 / CycleTime, sea level + 1)`, `g_vWaveFadeParams = (FadeInStart,
//! 1 / (FadeInEnd - FadeInStart), FadeOutStart, 1 / (FadeOutEnd - FadeOutStart))`,
//! `g_vTexturePageSize = (1 / PagesU, 1 / PagesV)`; `g_vColor` of `PS_Wave` is the linear colour.
//!
//! SIMPLIFIED(SIMP-042): the command set "Waves" uses the alpha state 4 and the z state 10 of the
//! engine's tables, whose blocks are not read; the stand-in adds the colour (the shader writes an
//! alpha of 1 and scales the colour by the fade) and tests the depth without writing it.

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

use crate::frame::{
    DEPTH_FORMAT, DrawContext, InitContext, MainSlot, PassSlot, PrepareContext, RenderSystem,
    SystemId, WORLD_FORMAT, multisample,
};
use crate::shaders::create_module;
use crate::texture::{UploadOptions, upload_blob};
use crate::world::terrain::basis::TerrainBasis;
use crate::world::terrain::sampler::TerrainFrame;
use crate::world::water::data::WaterData;
use crate::world::water::waves::{BATCH, INDICES_PER_WAVE, VERTICES_PER_WAVE, WaveField, linear};

/// The crest atlas `PS_Wave` samples (`g_WaveTexture`, slot 0).
// re: cvWaveSystem (rw-a6-answers RW-22 (a) 4: "Wave_Coastline_ALL.dds")
pub const WAVE_TEXTURE: &str = "wave_coastline_all.dds";

/// The crest atlas is read through an sRGB view: `PS_Wave` lights the decoded texel and encodes the
/// result with `1 / 2.2`, so a soft texel stays soft. Read raw, `t ^ (1 / 2.2)` made the surf milky and
/// wide (a texel of 0.25 came out 0.53).
// re: cvWaveSystem 0x0893e0de (`field_38[0] = 3`): sampler 3 of the engine's table is CLAMP with
// SRGBTEXTURE 1 (re-b16 W1, O3).
const CREST_UPLOAD: UploadOptions = UploadOptions {
    linear: false,
    ..UploadOptions::WORLD
};

/// The blob of the crest atlas marked sRGB, so it is uploaded as an `*Srgb` format.
fn crest_blob(blob: &asset_format::TextureBlob) -> asset_format::TextureBlob {
    let mut b = blob.clone();
    b.srgb = true;
    b
}

/// `WaveVsConsts` of waves.wgsl.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct WaveVsConsts {
    view_proj: [f32; 16],
    wave_data: [f32; 4],
    fade: [f32; 4],
    page_size: [f32; 4],
}

const COPIES: usize = 3;
const VS_STRIDE: u64 = 256;
/// `WaveData[248]`: 3968 bytes, a batch every 4096.
const DATA_STRIDE: u64 = 4096;

/// What the renderer gives the waves each frame.
#[derive(Clone)]
pub struct WavesFeed {
    /// The sea level of the original's world (`cvTerrainSampler` +4).
    pub sea_level: f32,
    pub data: Arc<WaterData>,
    /// `clip = view_proj_orig * p` for a point of the original's world.
    pub view_proj_orig: Mat4,
    pub shifts: Vec<f32>,
    /// Seconds.
    pub time: f32,
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    vs: Buf,
    ps: Buf,
    indices: Buf,
    data: Option<Buf>,
    vertices: Option<Buf>,
    texture: Option<(u64, wgpu::TextureView)>,
    group: Option<wgpu::BindGroup>,
}

impl Gpu {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, samples: u32) -> Self {
        let module = create_module(
            device,
            "water waves",
            crate::wgsl_module!("common/sm3.wgsl", "water/waves.wgsl"),
        );
        let uniform = |binding, dynamic, size: u64| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: wgpu::BufferSize::new(size),
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("waves"),
            entries: &[
                uniform(0, true, std::mem::size_of::<WaveVsConsts>() as u64),
                uniform(1, true, (BATCH * 16) as u64),
                uniform(2, false, 16),
                crate::world::water::ocean::texture_entry(3, wgpu::TextureViewDimension::D2),
                crate::world::water::ocean::sampler_entry(4),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("waves"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float16x2, 1 => Unorm8x4, 2 => Uint8x4];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("waves"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_wave"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 12,
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
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: multisample(samples),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("ps_wave"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: WORLD_FORMAT,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Zero,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let buffer = |label, size| {
            device.buf(
                &wgpu::BufferDescriptor {
                    label: Some(label),
                    size,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                },
                Cluster::Other,
            )
        };
        Self {
            device: device.clone(),
            queue: queue.clone(),
            layout,
            pipeline,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("waves"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            }),
            vs: buffer("waves vs", VS_STRIDE * COPIES as u64),
            ps: buffer("waves ps", 16),
            indices: device.buf_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("waves indices"),
                    contents: bytemuck::cast_slice(&WaveField::batch_indices()),
                    usage: wgpu::BufferUsages::INDEX,
                },
                Cluster::Other,
            ),
            data: None,
            vertices: None,
            texture: None,
            group: None,
        }
    }

    fn set_field(&mut self, field: &WaveField) {
        self.group = None;
        if field.waves.is_empty() {
            self.vertices = None;
            self.data = None;
            return;
        }
        self.vertices = Some(self.device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("waves vertices"),
                contents: bytemuck::cast_slice(&field.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            },
            Cluster::Other,
        ));
        self.data = Some(self.device.buf(
            &wgpu::BufferDescriptor {
                label: Some("waves data"),
                size: DATA_STRIDE * field.batches() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        ));
    }

    fn set_texture(&mut self, data: &WaterData) -> bool {
        if self.texture.as_ref().map(|(r, _)| *r) == Some(data.revision()) {
            return true;
        }
        let Some(blob) = data.texture(WAVE_TEXTURE) else {
            return false;
        };
        match upload_blob(
            &self.device,
            &self.queue,
            &crest_blob(blob),
            "waves crest",
            CREST_UPLOAD,
        ) {
            Ok(u) => {
                let view = u.texture.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(u.view_dimension),
                    ..Default::default()
                });
                self.texture = Some((data.revision(), view));
                self.group = None;
                true
            }
            Err(e) => {
                tracing::warn!(error = %e, "waves: crest texture not uploaded");
                false
            }
        }
    }

    fn bind(&mut self) -> bool {
        if self.group.is_some() {
            return true;
        }
        let (Some(data), Some((_, view))) = (&self.data, &self.texture) else {
            return false;
        };
        let part = |buffer, size: u64| {
            wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset: 0,
                size: wgpu::BufferSize::new(size),
            })
        };
        self.group = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("waves"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: part(&self.vs, std::mem::size_of::<WaveVsConsts>() as u64),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: part(data, (BATCH * 16) as u64),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.ps.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        }));
        true
    }
}

/// The surf.
#[derive(Default)]
pub struct Waves {
    gpu: Option<Gpu>,
    feed: Option<WavesFeed>,
    field: WaveField,
    /// The fingerprint of the basis, the frame and the settings the waves were made for.
    made_for: Option<u64>,
    frame: Option<TerrainFrame>,
    /// The field changed: the vertices go up again.
    field_dirty: bool,
    last_time: Option<f32>,
    drawn: std::cell::Cell<u32>,
}

impl Waves {
    /// Takes the device (once, after it exists).
    pub fn bind_device(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, samples: u32) {
        self.gpu = Some(Gpu::new(device, queue, samples));
        self.field_dirty = true;
    }

    /// What the frame gives the waves.
    pub fn feed(&mut self, feed: WavesFeed) {
        self.feed = Some(feed);
    }

    /// The map: the waves are made again when its land and water, its frame, the shore the coast
    /// saved (`BuildProducts::coast_segments`) or the settings changed (`SpawnWaves` runs once a
    /// game in the original). Call after [`Self::feed`].
    pub fn set_map(
        &mut self,
        basis: &TerrainBasis,
        frame: &TerrainFrame,
        shore: &[(crate::world::water::river::hex::HexEdge, [glam::Vec2; 2])],
    ) {
        let Some(feed) = &self.feed else { return };
        if !feed.data.has_settings() {
            return;
        }
        let cfg = feed.data.waves();
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (basis.width, basis.height, basis.wrap_x).hash(&mut h);
            for p in &basis.plots {
                p.terrain.hash(&mut h);
            }
            (frame.width_hexes(), frame.height_hexes()).hash(&mut h);
            for (e, pts) in shore {
                e.hash(&mut h);
                for p in pts {
                    (p.x.to_bits(), p.y.to_bits()).hash(&mut h);
                }
            }
            format!("{cfg:?}").hash(&mut h);
            h.finish()
        };
        if self.made_for == Some(key) {
            return;
        }
        self.field = crate::world::water::waves::spawn_waves(basis, frame, cfg, shore);
        self.made_for = Some(key);
        self.frame = Some(*frame);
        self.field_dirty = true;
        self.last_time = None;
    }

    /// The waves of the map, for the tests.
    pub fn field(&self) -> &WaveField {
        &self.field
    }

    pub fn drawn(&self) -> u32 {
        self.drawn.get()
    }
}

impl RenderSystem for Waves {
    fn id(&self) -> SystemId {
        SystemId::Waves
    }

    fn passes(&self) -> &'static [PassSlot] {
        &[PassSlot::Main(MainSlot::Waves)]
    }

    fn init(&mut self, _cx: &mut InitContext<'_>) {}

    fn prepare(&mut self, _cx: &mut PrepareContext<'_>) {
        self.drawn.set(0);
        let (Some(gpu), Some(feed)) = (&mut self.gpu, &self.feed) else {
            return;
        };
        if !feed.data.has_settings() {
            return;
        }
        let cfg = feed.data.waves();
        let Some(frame) = self.frame else { return };
        if self.field_dirty {
            gpu.set_field(&self.field);
            self.field_dirty = false;
        }
        let dt = self
            .last_time
            .map_or(0.0, |t| (feed.time - t).clamp(0.0, 0.25));
        self.last_time = Some(feed.time);
        self.field.update(cfg, dt);
        if !gpu.set_texture(&feed.data) || !gpu.bind() {
            return;
        }
        let scale = crate::world::water::waves::world_scale(&frame);
        let inv = |a: f32, b: f32| if b > a { 1.0 / (b - a) } else { 0.0 };
        for &shift in &feed.shifts {
            let slot = crate::world::water::ocean::copy_slot(shift).min(COPIES - 1);
            let c = WaveVsConsts {
                // A `row_major float4x4`: register k is column k of the plain matrix.
                view_proj: (feed.view_proj_orig * Mat4::from_translation(Vec3::X * shift))
                    .transpose()
                    .to_cols_array(),
                wave_data: [
                    scale * cfg.start_distance,
                    scale * cfg.end_distance,
                    if cfg.cycle_time > 0.0 {
                        1.0 / cfg.cycle_time
                    } else {
                        0.0
                    },
                    feed.sea_level + 1.0,
                ],
                fade: [
                    cfg.fade_in_start,
                    inv(cfg.fade_in_start, cfg.fade_in_end),
                    cfg.fade_out_start,
                    inv(cfg.fade_out_start, cfg.fade_out_end),
                ],
                page_size: [
                    1.0 / cfg.crest_pages_u.max(1) as f32,
                    1.0 / cfg.crest_pages_v.max(1) as f32,
                    0.0,
                    0.0,
                ],
            };
            gpu.queue
                .write_buffer(&gpu.vs, slot as u64 * VS_STRIDE, bytemuck::bytes_of(&c));
        }
        let color = linear(cfg.color);
        gpu.queue.write_buffer(
            &gpu.ps,
            0,
            bytemuck::bytes_of(&[color.x, color.y, color.z, 1.0f32]),
        );
        if let Some(data) = &gpu.data {
            for b in 0..self.field.batches() {
                let d = self.field.batch_data(cfg, b);
                gpu.queue
                    .write_buffer(data, b as u64 * DATA_STRIDE, bytemuck::cast_slice(&d));
            }
        }
    }

    fn record(&self, slot: PassSlot, cx: &mut DrawContext<'_, '_>) {
        if slot != PassSlot::Main(MainSlot::Waves) {
            return;
        }
        let Some(gpu) = &self.gpu else { return };
        let (Some(group), Some(vertices)) = (&gpu.group, &gpu.vertices) else {
            return;
        };
        let copy = crate::world::water::ocean::copy_slot(cx.copy_shift).min(COPIES - 1);
        cx.pass.set_pipeline_c(&gpu.pipeline);
        cx.pass
            .set_index_buffer(gpu.indices.slice(..), wgpu::IndexFormat::Uint16);
        for b in 0..self.field.batches() {
            let n = self.field.batch_len(b) as u32;
            // Each batch from its own offset of the vertex buffer: WebGL2 has no base vertex.
            let first = (b * BATCH * VERTICES_PER_WAVE) as u64 * 12;
            cx.pass.set_vertex_buffer(0, vertices.slice(first..));
            cx.pass.set_bind_group_c(
                0,
                group,
                &[
                    (copy as u64 * VS_STRIDE) as u32,
                    (b as u64 * DATA_STRIDE) as u32,
                ],
            );
            cx.pass
                .draw_indexed(0..n * INDICES_PER_WAVE as u32, 0, 0..1);
            cx.stats.record_draw(1);
            self.drawn.set(self.drawn.get() + 1);
        }
    }

    fn counters(&self) -> Vec<(&'static str, u64)> {
        vec![
            ("waves.waves", self.field.waves.len() as u64),
            ("waves.drawn", u64::from(self.drawn.get())),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// re-b16 W1: the crest atlas (BC1 in the pack) goes up in an sRGB format, so the sampler decodes
    /// it as the original's sampler 3 does before `PS_Wave` lights it and encodes `1 / 2.2`.
    #[test]
    fn the_crest_atlas_is_sampled_through_an_srgb_view() {
        let raw = asset_format::TextureBlob {
            format: asset_format::TextureFormat::Bc1,
            dim: asset_format::TexDim::D2,
            srgb: false,
            width: 4,
            height: 4,
            depth: 1,
            mips: vec![vec![0; 8]],
        };
        let b = crest_blob(&raw);
        assert_eq!(
            crate::texture::wgpu_format(b.format, b.srgb && !CREST_UPLOAD.linear),
            wgpu::TextureFormat::Bc1RgbaUnormSrgb
        );
    }
}
