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
//! Pictures of game assets and the minimap sample external textures ([`ui2_draw::ExtTexture`]) bound as a
//! second bind group of up to [`EXT_SLOTS`] textures; each picture finds its texture there by its slot. A layer
//! is drawn in its own order (chunks in order, items in order) as consecutive instance ranges: a range grows over
//! the scene's runs until it would need a texture more than the group holds, and only then a new draw starts. So
//! a layer whose pictures come from at most [`EXT_SLOTS`] textures is one draw, the paint order is never changed,
//! and the pixels are those of one draw per run. [`Ui2ExtTextures`] resolves the ids every frame (the renderer's
//! implementation asks the original UI's texture registry and loads missing assets through its wanted list); a
//! group is made only when a set of textures is new or one of them is uploaded again. A picture whose texture is
//! not available gets no area in the vertex shader.
//!
//! [`Ui2Pass::prepare`] does every upload; [`Ui2Pass::draw`] only records. A frame without changes uploads
//! nothing.

mod gpu_atlas;
#[cfg(test)]
mod tests;

use std::ops::Range;

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use draw_list::TextureId;
use ui2_draw::instance::{ATTRIBUTES, AttrFormat, INSTANCE_SIZE};
use ui2_draw::tables::{HEIGHT as TABLE_HEIGHT, TEXEL_BYTES, WIDTH as TABLE_WIDTH};
use ui2_draw::{LayerChange, LayerId, Scene};

use crate::perf::gpu_mem::{Buf, Cluster, DeviceTrack, Tex};
use crate::ui_pass::{UiExternal, UiTarget, UiTextures};

pub use gpu_atlas::{ImageArray, MIN_LAYERS, MaskArray, MaskSource, glyph_room};

/// Smallest layer buffer; buffers grow by 1.5x from here.
const MIN_BUFFER: u64 = 64 * 1024;
/// Vertices of the quad of one instance.
const QUAD_VERTICES: u32 = 6;
/// External textures one draw binds (`EXT_SLOTS` in `ui2.wgsl`). With the coverage and image arrays that is 10
/// sampled textures in the fragment stage, within the 16 of WebGL2.
pub const EXT_SLOTS: usize = 8;
/// The slot of an unused entry of the external texture group (`NO_SLOT` in `ui2.wgsl`).
const NO_SLOT: u32 = u32::MAX;

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
    /// Of `upload_bytes`: the transform and clip table segments (anchors moved by the camera land here).
    pub table_bytes: u64,
    /// Layers whose instances were written (in full or in ranges).
    pub layers_uploaded: u32,
    /// Bytes of the glyph layers of the coverage array on the GPU.
    pub glyph_bytes: u64,
    /// Bytes of the layers that hold the skin package's picture pages (coverage and image arrays).
    pub page_bytes: u64,
    /// Runs of pictures of an external texture the pass cannot bind yet (drawn without area).
    pub ext_missing: u32,
    /// Bind groups made for sets of external textures (a new set, or a new upload of one of its textures).
    pub ext_groups_made: u32,
}

impl Ui2Stats {
    /// Names as `__civ5stats.counters` and the stats table show them.
    pub fn counters(&self) -> [(&'static str, u64); 12] {
        let [map, hud, window, tooltip] = self.layer_draws.map(u64::from);
        [
            ("ui2.draws", u64::from(self.draws)),
            ("ui2.draws.map", map),
            ("ui2.draws.hud", hud),
            ("ui2.draws.window", window),
            ("ui2.draws.tooltip", tooltip),
            ("ui2.instances", self.instances),
            ("ui2.upload_bytes", self.upload_bytes),
            ("ui2.table_bytes", self.table_bytes),
            ("ui2.layers_uploaded", u64::from(self.layers_uploaded)),
            ("ui2.atlas.glyph_mb", self.glyph_bytes >> 20),
            ("ui2.atlas.pages_mb", self.page_bytes >> 20),
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

/// One draw call: instances `range` of layer `layer`, with the external texture group `ext` (an index into
/// `Ui2Pass::ext_bound`; `None`: the placeholder group).
#[derive(Debug, Clone, PartialEq, Eq)]
struct DrawOp {
    layer: usize,
    range: Range<u32>,
    ext: Option<usize>,
}

/// One entry of an external texture group: the scene slot pictures name it by, the texture id, its serial.
type ExtKey = (u16, u64, u64);

/// A texture of the draw being gathered: its key, view and UV scale.
struct ExtEntry {
    key: ExtKey,
    view: wgpu::TextureView,
    scale: [f32; 2],
}

/// The draw being gathered: consecutive instances of a layer and the external textures they sample.
#[derive(Default)]
struct Gather {
    range: Option<Range<u32>>,
    entries: Vec<ExtEntry>,
}

/// Where the pass finds the renderer's textures that pictures name by [`ui2_draw::ExtTexture`].
pub trait Ui2ExtTextures {
    /// The texture `id` for this frame: a serial that changes whenever the texture behind the id changes, its
    /// view, and the UV scale that maps the picture's 0..1 onto the stored texture. `None` while it is not
    /// available (a game asset not loaded yet is asked for here).
    // SIMPLIFIED(SIMP-282): an external texture is sampled as straight RGBA8 colour; one stored as one or two channels (R8, RG8) is not interpreted, so it must be converted to RGBA before it is offered here.
    fn resolve(&mut self, id: u64) -> Option<(u64, &wgpu::TextureView, [f32; 2])>;
}

/// No external textures at all.
pub struct NoExtTextures;

impl Ui2ExtTextures for NoExtTextures {
    fn resolve(&mut self, _id: u64) -> Option<(u64, &wgpu::TextureView, [f32; 2])> {
        None
    }
}

/// The renderer's textures: the game assets of the original UI's texture registry (loaded on demand through
/// its wanted list) and the minimap target.
pub(crate) struct RendererTextures<'a> {
    pub textures: &'a mut UiTextures,
    pub external: UiExternal<'a>,
}

impl Ui2ExtTextures for RendererTextures<'_> {
    fn resolve(&mut self, id: u64) -> Option<(u64, &wgpu::TextureView, [f32; 2])> {
        let tid = TextureId(id);
        if tid == TextureId::MINIMAP {
            let (view, info) = self.external.minimap?;
            return Some((info.generation, view, [1.0, 1.0]));
        }
        if tid.is_external() {
            return None;
        }
        let serial = self.textures.touch(tid)?;
        let scale = self.textures.uv_scale(tid);
        Some((serial, self.textures.view(tid)?, scale))
    }
}

/// `ExtParams` of `ui2.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Pod, Zeroable)]
struct ExtUniform {
    /// xy: the UV scale of entry i.
    uv_scale: [[f32; 4]; EXT_SLOTS],
    /// The scene slot of each entry ([`NO_SLOT`]: unused).
    slots: [u32; EXT_SLOTS],
}

impl ExtUniform {
    fn of(entries: &[ExtEntry]) -> Self {
        let mut u = ExtUniform {
            uv_scale: [[1.0, 1.0, 0.0, 0.0]; EXT_SLOTS],
            slots: [NO_SLOT; EXT_SLOTS],
        };
        for (i, e) in entries.iter().take(EXT_SLOTS).enumerate() {
            u.uv_scale[i] = [e.scale[0], e.scale[1], 0.0, 0.0];
            u.slots[i] = u32::from(e.key.0);
        }
        u
    }
}

/// The bind group of a set of external textures (made when the set is new or one of its textures is uploaded
/// again: the serial is part of the key).
struct ExtGroup {
    group: wgpu::BindGroup,
    _uniform: Buf,
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
    image: ImageArray,
    /// `Pages::revision` of the package pages on the GPU.
    pages_revision: u64,
    group: wgpu::BindGroup,
    /// Group 1: up to [`EXT_SLOTS`] external textures, their slots and UV scales.
    ext_layout: wgpu::BindGroupLayout,
    /// Group 1 of draws without an external texture (white texels, no slot).
    ext_default: wgpu::BindGroup,
    _ext_white: Tex,
    /// The placeholder of the unused entries of a group.
    ext_white_view: wgpu::TextureView,
    _ext_default_uniform: Buf,
    /// Groups of the sets of external textures in use, by their entries.
    ext_groups: HashMap<Vec<ExtKey>, ExtGroup>,
    /// The groups this frame's draws bind (`DrawOp::ext`), with their entries.
    ext_bound: Vec<(Vec<ExtKey>, wgpu::BindGroup)>,
    /// Tests: one draw per run of the scene (the draws before batching), the reference of the batched pixels.
    #[cfg(test)]
    unbatched: bool,
    layers: [LayerBuf; 4],
    ops: Vec<DrawOp>,
    /// Nothing was uploaded yet: the first `prepare` writes everything the scene holds.
    fresh: bool,
    /// The scene's solid background, as the clear value of the target.
    clear: Option<wgpu::Color>,
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
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let mut ext_entries: Vec<wgpu::BindGroupLayoutEntry> = (0..EXT_SLOTS as u32)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            })
            .collect();
        // The slots are read by the vertex shader (a picture finds its entry there), the scales by the fragment one.
        ext_entries.push(wgpu::BindGroupLayoutEntry {
            binding: EXT_SLOTS as u32,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<ExtUniform>() as u64),
            },
            count: None,
        });
        let ext_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui2 external texture layout"),
            entries: &ext_entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ui2 pipeline layout"),
            bind_group_layouts: &[Some(&layout), Some(&ext_layout)],
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
        let image = ImageArray::new(device);
        let group = Self::make_group(
            device,
            &layout,
            &uniform,
            &tables_view,
            [mask.view(), image.view()],
            &sampler,
        );
        // Draws without an external texture never sample group 1: a zeroed texel is enough, and no entry has a slot.
        let ext_white = device.tex(
            &wgpu::TextureDescriptor {
                label: Some("ui2 no external texture"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            Cluster::Ui,
        );
        let ext_white_view = ext_white.create_view(&wgpu::TextureViewDescriptor::default());
        let ext_default_uniform = Self::ext_uniform(device, &ExtUniform::of(&[]));
        let ext_default = Self::make_ext_group(
            device,
            &ext_layout,
            &[],
            &ext_white_view,
            &ext_default_uniform,
        );
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
            image,
            pages_revision: 0,
            group,
            ext_layout,
            ext_default,
            _ext_white: ext_white,
            ext_white_view,
            _ext_default_uniform: ext_default_uniform,
            ext_groups: HashMap::new(),
            ext_bound: Vec::new(),
            #[cfg(test)]
            unbatched: false,
            layers: Default::default(),
            ops: Vec::new(),
            fresh: true,
            clear: None,
            stats: Ui2Stats::default(),
        }
    }

    fn make_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        uniform: &wgpu::Buffer,
        tables: &wgpu::TextureView,
        [mask, image]: [&wgpu::TextureView; 2],
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
                    resource: wgpu::BindingResource::TextureView(mask),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(image),
                },
            ],
        })
    }

    /// A uniform buffer holding `u` (written at creation).
    fn ext_uniform(device: &wgpu::Device, u: &ExtUniform) -> Buf {
        let buf = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("ui2 external textures"),
                size: std::mem::size_of::<ExtUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM,
                mapped_at_creation: true,
            },
            Cluster::Ui,
        );
        // A buffer mapped at creation always has its range.
        if let Ok(mut range) = buf.slice(..).get_mapped_range_mut() {
            range.copy_from_slice(bytemuck::bytes_of(u));
        }
        buf.unmap();
        buf
    }

    /// The group of `entries` (the rest of the [`EXT_SLOTS`] textures are `white`).
    fn make_ext_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        entries: &[ExtEntry],
        white: &wgpu::TextureView,
        uniform: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        let mut bindings: Vec<wgpu::BindGroupEntry> = (0..EXT_SLOTS)
            .map(|i| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: wgpu::BindingResource::TextureView(
                    entries.get(i).map_or(white, |e| &e.view),
                ),
            })
            .collect();
        bindings.push(wgpu::BindGroupEntry {
            binding: EXT_SLOTS as u32,
            resource: uniform.as_entire_binding(),
        });
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ui2 external texture group"),
            layout,
            entries: &bindings,
        })
    }

    /// The group of the gathered draw's textures for this frame (made when the set is new or one of its textures
    /// was uploaded again), as an index into `ext_bound`; `None` for a draw without external textures.
    fn bind_ext(
        &mut self,
        device: &wgpu::Device,
        entries: &[ExtEntry],
        stats: &mut Ui2Stats,
    ) -> Option<usize> {
        if entries.is_empty() {
            return None;
        }
        let key: Vec<ExtKey> = entries.iter().map(|e| e.key).collect();
        if let Some(i) = self.ext_bound.iter().position(|(bound, _)| *bound == key) {
            return Some(i);
        }
        if !self.ext_groups.contains_key(&key) {
            let uniform = Self::ext_uniform(device, &ExtUniform::of(entries));
            stats.upload_bytes += std::mem::size_of::<ExtUniform>() as u64;
            stats.ext_groups_made += 1;
            let group = Self::make_ext_group(
                device,
                &self.ext_layout,
                entries,
                &self.ext_white_view,
                &uniform,
            );
            self.ext_groups.insert(
                key.clone(),
                ExtGroup {
                    group,
                    _uniform: uniform,
                },
            );
        }
        let group = self.ext_groups.get(&key)?.group.clone();
        self.ext_bound.push((key, group));
        Some(self.ext_bound.len() - 1)
    }

    /// Ends the gathered draw of layer `li`: records it with its group.
    fn flush(&mut self, device: &wgpu::Device, li: usize, g: &mut Gather, stats: &mut Ui2Stats) {
        let entries = std::mem::take(&mut g.entries);
        let Some(range) = g.range.take() else {
            return;
        };
        let ext = self.bind_ext(device, &entries, stats);
        stats.draws += 1;
        stats.layer_draws[li] += 1;
        stats.instances += u64::from(range.end - range.start);
        self.ops.push(DrawOp {
            layer: li,
            range,
            ext,
        });
    }

    /// The counters of the last `prepare`.
    pub fn stats(&self) -> Ui2Stats {
        self.stats
    }

    /// True when `draw` has something to record or the frame is to be cleared; the pass is not opened otherwise.
    pub fn has_work(&self) -> bool {
        !self.ops.is_empty() || self.clear.is_some()
    }

    /// How the pass opens its target: cleared with the scene's solid background (`Scene::set_background`, the
    /// reference images of the UI alone), loaded otherwise (the UI over the frame).
    pub fn load_op(&self) -> wgpu::LoadOp<wgpu::Color> {
        match self.clear {
            Some(c) => wgpu::LoadOp::Clear(c),
            None => wgpu::LoadOp::Load,
        }
    }

    /// Draw calls and triangles of the last `prepare`.
    pub fn draw_totals(&self) -> (u32, u32) {
        (self.stats.draws, (self.stats.instances * 2) as u32)
    }

    /// Uploads what changed in `scene` since the last call (everything on the first call) and takes its draw
    /// calls for this frame; `textures` resolves the external textures pictures name. Call before the pass
    /// opens.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &mut Scene,
        target: &UiTarget,
        textures: &mut dyn Ui2ExtTextures,
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

        self.clear = scene
            .background()
            .map(|c| clear_color(c, target.format.is_srgb()));
        stats.table_bytes = self.upload_tables(queue, scene, full);
        stats.upload_bytes += stats.table_bytes;

        let dirty = scene.take_glyph_dirty();
        let pages = scene.pages();
        let pages_changed = pages.revision() != self.pages_revision;
        self.pages_revision = pages.revision();
        let src = MaskSource {
            pages: pages.mask(),
            pages_changed,
            side: scene.mask_side(),
            glyphs: scene.glyphs(),
            dirty: &dirty,
        };
        let (mask_bytes, mask_remade) = self.mask.sync(device, queue, &src, full);
        let (image_bytes, image_remade) = self.image.sync(
            device,
            queue,
            pages.image(),
            pages.image_side(),
            pages_changed,
            full,
        );
        stats.upload_bytes += mask_bytes + image_bytes;
        if mask_remade || image_remade {
            self.group = Self::make_group(
                device,
                &self.layout,
                &self.uniform,
                &self.tables_view,
                [self.mask.view(), self.image.view()],
                &self.sampler,
            );
        }
        stats.page_bytes = self.mask.page_bytes() + self.image.page_bytes();
        stats.glyph_bytes = self.mask.bytes().saturating_sub(self.mask.page_bytes());

        self.ops.clear();
        self.ext_bound.clear();
        for layer in LayerId::ALL {
            let li = layer.index();
            let (bytes, written) = self.upload_layer(device, queue, scene, layer, full);
            stats.upload_bytes += bytes;
            stats.layers_uploaded += u32::from(written);
            // The scene's runs (instances by external texture, in paint order) gathered into as few draws as the
            // texture group allows; a run of an external texture that is not available stays in the draw and gets
            // no area in the vertex shader.
            let mut g = Gather::default();
            let ops = scene.layer_ops(layer).to_vec();
            for op in ops {
                if op.range.is_empty() {
                    continue;
                }
                let entry = match scene.ext_texture(op.ext) {
                    None if op.ext == 0 => None,
                    None => {
                        stats.ext_missing += 1;
                        None
                    }
                    Some(t) => match textures.resolve(t.0) {
                        Some((serial, view, scale)) => Some(ExtEntry {
                            key: (op.ext, t.0, serial),
                            view: view.clone(),
                            scale,
                        }),
                        None => {
                            stats.ext_missing += 1;
                            None
                        }
                    },
                };
                #[cfg(test)]
                if self.unbatched {
                    self.flush(device, li, &mut g, &mut stats);
                }
                if let Some(e) = entry
                    && !g.entries.iter().any(|x| x.key == e.key)
                {
                    if g.entries.len() == EXT_SLOTS {
                        self.flush(device, li, &mut g, &mut stats);
                    }
                    g.entries.push(e);
                }
                g.range = Some(match g.range.take() {
                    Some(r) => r.start..op.range.end,
                    None => op.range.clone(),
                });
            }
            self.flush(device, li, &mut g, &mut stats);
        }
        // Groups of texture sets no draw uses any more are dropped.
        let bound = &self.ext_bound;
        self.ext_groups
            .retain(|id, _| bound.iter().any(|(b, _)| b == id));
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

    /// Writes a layer's instances when its revision changed: the changed ranges when the count stayed, the tail
    /// from the first changed chunk when the count changed and the buffer has room, the whole layer otherwise.
    /// Returns the bytes written and whether anything was.
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
        let fits = (instances.len() * INSTANCE_SIZE) as u64 <= lb.capacity && lb.buffer.is_some();
        let ranges = match change {
            LayerChange::Ranges(r) if !full && lb.len == instances.len() => Some(r),
            // The instances before `start` are the ones the buffer holds: only the rest is written.
            LayerChange::Tail(start) if !full && fits && start <= lb.len.min(instances.len()) => {
                lb.len = instances.len();
                Some(std::iter::once(start..instances.len()).collect())
            }
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
        // `Some(None)`: the placeholder group is bound.
        let mut bound: Option<Option<usize>> = None;
        for op in &self.ops {
            let Some(buffer) = &self.layers[op.layer].buffer else {
                continue;
            };
            if bound != Some(op.ext) {
                let group = match op.ext.and_then(|i| self.ext_bound.get(i)) {
                    Some((_, g)) => g,
                    None => &self.ext_default,
                };
                pass.set_bind_group(1, group, &[]);
                bound = Some(op.ext);
            }
            let bytes = u64::from(op.range.start) * stride..u64::from(op.range.end) * stride;
            pass.set_vertex_buffer(0, buffer.slice(bytes));
            pass.draw(0..QUAD_VERTICES, 0..op.range.end - op.range.start);
        }
    }
}

/// The clear value that stores sRGB colour `c` in a target: the colour itself in a plain target, its linear
/// value in an sRGB one (which encodes on store).
fn clear_color(c: ui2_draw::Color, srgb_target: bool) -> wgpu::Color {
    let ch = |v: u8| {
        let x = f64::from(v) / 255.0;
        if !srgb_target {
            x
        } else if x <= 0.04045 {
            x / 12.92
        } else {
            ((x + 0.055) / 1.055).powf(2.4)
        }
    };
    wgpu::Color {
        r: ch(c.0[0]),
        g: ch(c.0[1]),
        b: ch(c.0[2]),
        a: 1.0,
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
        textures: &mut dyn Ui2ExtTextures,
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
        pass.prepare(device, queue, scene, target, textures);
        self.armed = true;
    }

    pub fn stats(&self) -> Ui2Stats {
        self.pass.as_ref().map(Ui2Pass::stats).unwrap_or_default()
    }
}
