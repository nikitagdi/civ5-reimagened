//! The UI v2 pass: draws the retained layers of a [`ui2_draw::Scene`] (map, HUD, windows, tooltips, in that
//! order) over the finished frame, before the original UI and egui.
//!
//! One pipeline and one bind group for everything: each primitive is an instance of 96 bytes
//! ([`ui2_draw::Ui2Instance`]) drawn as a quad of six vertices without an index buffer; the shader computes the
//! coverage of boxes from distance functions and reads transforms and clips from a table texture. Each layer
//! keeps its own vertex buffer, written only when the layer's revision changed (the changed instance ranges
//! when the count stayed the same), and each draw call ([`ui2_draw::Op`]) binds a slice of it, so no base
//! instance is needed (WebGL2 has none).
//!
//! [`Ui2Pass::prepare`] does every upload; [`Ui2Pass::draw`] only records. A frame without changes uploads
//! nothing.

mod gpu_atlas;
#[cfg(test)]
mod tests;

use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use ui2_draw::instance::{ATTRIBUTES, AttrFormat, INSTANCE_SIZE};
use ui2_draw::tables::{HEIGHT as TABLE_HEIGHT, TEXEL_BYTES, WIDTH as TABLE_WIDTH};
use ui2_draw::{LayerChange, LayerId, Scene};

use crate::perf::gpu_mem::{Buf, Cluster, DeviceTrack, Tex};
use crate::ui_pass::UiTarget;

pub use gpu_atlas::{MIN_LAYERS, MaskArray};

/// Smallest layer buffer; buffers grow by 1.5x from here.
const MIN_BUFFER: u64 = 64 * 1024;
/// Vertices of the quad of one instance.
const QUAD_VERTICES: u32 = 6;

/// The counters of the last [`Ui2Pass::prepare`] (`ui2.*`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ui2Stats {
    /// Draw calls of the pass.
    pub draws: u32,
    /// Draw calls per layer, in [`LayerId::ALL`] order.
    pub layer_draws: [u32; 4],
    /// Instances drawn.
    pub instances: u64,
    /// Bytes written to the GPU: instances, table segments, glyph pages, the frame constants.
    pub upload_bytes: u64,
    /// Layers whose instances were written (in full or in ranges).
    pub layers_uploaded: u32,
    /// Bytes of the glyph mask array on the GPU.
    pub glyph_bytes: u64,
    /// Draw calls skipped for an external texture the pass cannot bind yet.
    pub ext_missing: u32,
}

impl Ui2Stats {
    /// Names as `__civ5stats.counters` and the stats table show them.
    pub fn counters(&self) -> [(&'static str, u64); 10] {
        let [map, hud, window, tooltip] = self.layer_draws.map(u64::from);
        [
            ("ui2.draws", u64::from(self.draws)),
            ("ui2.draws.map", map),
            ("ui2.draws.hud", hud),
            ("ui2.draws.window", window),
            ("ui2.draws.tooltip", tooltip),
            ("ui2.instances", self.instances),
            ("ui2.upload_bytes", self.upload_bytes),
            ("ui2.layers_uploaded", u64::from(self.layers_uploaded)),
            ("ui2.atlas.glyph_mb", self.glyph_bytes >> 20),
            ("ui2.ext_missing", u64::from(self.ext_missing)),
        ]
    }
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Pod, Zeroable)]
struct FrameUniform {
    to_ndc: [f32; 2],
    target_srgb: f32,
    pad: f32,
}

/// The GPU copy of one layer's instances.
#[derive(Default)]
struct LayerBuf {
    buffer: Option<Buf>,
    capacity: u64,
    /// Instances in the buffer.
    len: usize,
    /// Scene revision of the layer the buffer holds.
    revision: u64,
}

impl LayerBuf {
    /// Writes all of `bytes`, growing the buffer by 1.5x steps from [`MIN_BUFFER`] when they do not fit.
    fn write_all(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8]) {
        let need = bytes.len() as u64;
        if need > self.capacity || self.buffer.is_none() {
            let mut cap = self.capacity.max(MIN_BUFFER);
            while cap < need {
                cap = (cap + cap / 2).next_multiple_of(INSTANCE_SIZE as u64);
            }
            self.buffer = Some(device.buf(
                &wgpu::BufferDescriptor {
                    label: Some("ui2 layer instances"),
                    size: cap,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                },
                Cluster::Ui,
            ));
            self.capacity = cap;
        }
        if let (Some(b), false) = (&self.buffer, bytes.is_empty()) {
            queue.write_buffer(b, 0, bytes);
        }
    }
}

/// One draw call: instances `range` of layer `layer`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DrawOp {
    layer: usize,
    range: Range<u32>,
}

/// The UI v2 pass.
pub struct Ui2Pass {
    format: wgpu::TextureFormat,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: Buf,
    /// What `uniform` holds.
    frame: Option<FrameUniform>,
    tables: Tex,
    tables_view: wgpu::TextureView,
    mask: MaskArray,
    group: wgpu::BindGroup,
    layers: [LayerBuf; 4],
    ops: Vec<DrawOp>,
    /// Nothing was uploaded yet: the first `prepare` writes everything the scene holds.
    fresh: bool,
    stats: Ui2Stats,
}

fn vertex_format(f: AttrFormat) -> wgpu::VertexFormat {
    match f {
        AttrFormat::Float32x4 => wgpu::VertexFormat::Float32x4,
        AttrFormat::Float16x4 => wgpu::VertexFormat::Float16x4,
        AttrFormat::Unorm8x4 => wgpu::VertexFormat::Unorm8x4,
        AttrFormat::Uint32x3 => wgpu::VertexFormat::Uint32x3,
    }
}

/// The instance attributes of the pipeline, from the layout table of `ui2-draw`.
fn vertex_attributes() -> Vec<wgpu::VertexAttribute> {
    ATTRIBUTES
        .iter()
        .map(|a| wgpu::VertexAttribute {
            format: vertex_format(a.format),
            offset: u64::from(a.offset),
            shader_location: a.location,
        })
        .collect()
}

impl Ui2Pass {
    /// Makes the pipeline for a target of `format`, the table texture and an empty mask array.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("ui2.wgsl"));
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui2 layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<FrameUniform>() as u64,
                        ),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ui2 pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attributes = vertex_attributes();
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ui2 pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: INSTANCE_SIZE as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attributes,
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ui2 mask sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let uniform = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("ui2 frame"),
                size: std::mem::size_of::<FrameUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Ui,
        );
        let tables = device.tex(
            &wgpu::TextureDescriptor {
                label: Some("ui2 tables"),
                size: wgpu::Extent3d {
                    width: TABLE_WIDTH,
                    height: TABLE_HEIGHT,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            Cluster::Ui,
        );
        let tables_view = tables.create_view(&wgpu::TextureViewDescriptor::default());
        let mask = MaskArray::new(device);
        let group = Self::make_group(device, &layout, &uniform, &tables_view, &mask, &sampler);
        Self {
            format,
            pipeline,
            layout,
            sampler,
            uniform,
            frame: None,
            tables,
            tables_view,
            mask,
            group,
            layers: Default::default(),
            ops: Vec::new(),
            fresh: true,
            stats: Ui2Stats::default(),
        }
    }

    fn make_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        uniform: &wgpu::Buffer,
        tables: &wgpu::TextureView,
        mask: &MaskArray,
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ui2 group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(tables),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(mask.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }

    /// The counters of the last `prepare`.
    pub fn stats(&self) -> Ui2Stats {
        self.stats
    }

    /// True when `draw` has something to record; the pass is not opened otherwise.
    pub fn has_work(&self) -> bool {
        !self.ops.is_empty()
    }

    /// Draw calls and triangles of the last `prepare`.
    pub fn draw_totals(&self) -> (u32, u32) {
        (self.stats.draws, (self.stats.instances * 2) as u32)
    }

    /// Uploads what changed in `scene` since the last call (everything on the first call) and takes its draw
    /// calls for this frame. Call before the pass opens.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &mut Scene,
        target: &UiTarget,
    ) {
        let mut stats = Ui2Stats::default();
        let full = self.fresh;
        self.fresh = false;

        let frame = FrameUniform {
            to_ndc: [
                2.0 / target.size.0.max(1) as f32,
                2.0 / target.size.1.max(1) as f32,
            ],
            // An sRGB target makes the shader linearise its output (see `finish` in `ui2.wgsl`).
            target_srgb: if target.format.is_srgb() { 1.0 } else { 0.0 },
            pad: 0.0,
        };
        if self.frame != Some(frame) {
            queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&frame));
            self.frame = Some(frame);
            stats.upload_bytes += std::mem::size_of::<FrameUniform>() as u64;
        }

        stats.upload_bytes += self.upload_tables(queue, scene, full);

        let dirty = scene.take_glyph_dirty();
        let (bytes, remade) = self
            .mask
            .sync(device, queue, scene.glyph_atlas(), &dirty, full);
        stats.upload_bytes += bytes;
        if remade {
            self.group = Self::make_group(
                device,
                &self.layout,
                &self.uniform,
                &self.tables_view,
                &self.mask,
                &self.sampler,
            );
        }
        stats.glyph_bytes = self.mask.bytes();

        self.ops.clear();
        for layer in LayerId::ALL {
            let li = layer.index();
            let (bytes, written) = self.upload_layer(device, queue, scene, layer, full);
            stats.upload_bytes += bytes;
            stats.layers_uploaded += u32::from(written);
            for op in scene.layer_ops(layer) {
                if op.range.is_empty() {
                    continue;
                }
                if op.ext != 0 {
                    stats.ext_missing += 1;
                    continue;
                }
                stats.draws += 1;
                stats.layer_draws[li] += 1;
                stats.instances += u64::from(op.range.end - op.range.start);
                self.ops.push(DrawOp {
                    layer: li,
                    range: op.range.clone(),
                });
            }
        }
        self.stats = stats;
    }

    /// Writes the changed segments of the table texture (all of it when `full`). Returns the bytes written.
    fn upload_tables(&mut self, queue: &wgpu::Queue, scene: &mut Scene, full: bool) -> u64 {
        let segments = scene.take_table_uploads();
        let write = |row: u32, x: u32, texels: &[[f32; 4]]| {
            let width = texels.len() as u32;
            let rows = texels.len() as u32 / TABLE_WIDTH;
            let (width, height) = if rows > 1 {
                (TABLE_WIDTH, rows)
            } else {
                (width, 1)
            };
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.tables,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y: row, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(texels),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * TEXEL_BYTES),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
            u64::from(width * height * TEXEL_BYTES)
        };
        if full {
            return write(0, 0, scene.tables().all_texels());
        }
        segments
            .iter()
            .filter(|s| !s.texels.is_empty())
            .map(|s| write(s.row, s.x, &s.texels))
            .sum()
    }

    /// Writes a layer's instances when its revision changed: the changed ranges when the count stayed, the
    /// whole layer otherwise. Returns the bytes written and whether anything was.
    fn upload_layer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &mut Scene,
        layer: LayerId,
        full: bool,
    ) -> (u64, bool) {
        let change = scene.take_layer_change(layer);
        let revision = scene.layer_revision(layer);
        let instances = scene.layer_instances(layer);
        let lb = &mut self.layers[layer.index()];
        let ranges = match change {
            LayerChange::Ranges(r) if !full && lb.len == instances.len() => Some(r),
            LayerChange::None if !full && lb.revision == revision => None,
            // A count change, a revision this buffer does not hold (another scene) or the first upload.
            _ => {
                let bytes: &[u8] = bytemuck::cast_slice(instances);
                lb.write_all(device, queue, bytes);
                lb.len = instances.len();
                lb.revision = revision;
                return (bytes.len() as u64, !bytes.is_empty());
            }
        };
        lb.revision = revision;
        let (Some(ranges), Some(buffer)) = (ranges, &lb.buffer) else {
            return (0, false);
        };
        let mut written = 0u64;
        for r in ranges {
            let Some(slice) = instances.get(r.clone()) else {
                continue;
            };
            let bytes: &[u8] = bytemuck::cast_slice(slice);
            if bytes.is_empty() {
                continue;
            }
            queue.write_buffer(buffer, (r.start * INSTANCE_SIZE) as u64, bytes);
            written += bytes.len() as u64;
        }
        (written, written > 0)
    }

    /// Records the frame's draw calls into `pass`, which loads the colour and has no depth.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'static>) {
        if self.ops.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.group, &[]);
        let stride = INSTANCE_SIZE as u64;
        for op in &self.ops {
            let Some(buffer) = &self.layers[op.layer].buffer else {
                continue;
            };
            let bytes = u64::from(op.range.start) * stride..u64::from(op.range.end) * stride;
            pass.set_vertex_buffer(0, buffer.slice(bytes));
            pass.draw(0..QUAD_VERTICES, 0..op.range.end - op.range.start);
        }
    }
}

/// The pass as the renderer holds it: made on the first [`Ui2Slot::prepare`], so a renderer that never sees a
/// UI v2 scene makes no GPU object for it, and armed only for the frame after a `prepare`.
#[derive(Default)]
pub(crate) struct Ui2Slot {
    pub pass: Option<Ui2Pass>,
    /// `prepare` was called since the last frame: the step runs.
    pub armed: bool,
    /// The step ran in the frame being encoded: its counters go into the frame's.
    pub ran: bool,
    /// CPU time of the `prepare` calls since the last frame, counted to the pass's row.
    pub prepare_ms: f64,
}

impl Ui2Slot {
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &mut Scene,
        target: &UiTarget,
    ) {
        // The pipeline is made for one target format; another one starts the pass again.
        if self
            .pass
            .as_ref()
            .is_some_and(|p| p.format != target.format)
        {
            self.pass = None;
        }
        let pass = self
            .pass
            .get_or_insert_with(|| Ui2Pass::new(device, target.format));
        pass.prepare(device, queue, scene, target);
        self.armed = true;
    }

    pub fn stats(&self) -> Ui2Stats {
        self.pass.as_ref().map(Ui2Pass::stats).unwrap_or_default()
    }
}
