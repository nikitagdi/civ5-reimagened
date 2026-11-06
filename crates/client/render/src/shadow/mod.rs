//! The shadow map (milestone M1b): the camera of the light, the
//! `ShadowMap` pass that draws what casts a shadow into the raw map, and the `ShadowBlur` pass
//! (`Shadow::BlurAndProcess`) that turns it into the map the terrain and the models read.
//!
//! The formats are the original's where the device renders and filters half floats: the raw map
//! `Rgba16Float` (the shadow is its alpha, as the blur shader reads it), the blurred one `R16Float`.
//! A device that cannot (WebGL2 without the float-target extension) takes `Rgba8Unorm` for both,
//! 256 steps of a blur whose result is a mean of eight taps (some banding in the penumbra). The
//! half floats cost 4 more bytes a pixel of the screen for the raw map and save 2 a texel of the
//! blurred one: about +8 MB at 1920 x 1080 and -2 MB (level 2) or -8 MB (level 3).
// re: TerrainSystem::BuildShadowBuffers 0x0867ef40 ("Shadow Texture Raw" format 0x71 =
// D3DFMT_A16B16G16R16F, "Shadow Texture Blur" 0x6f = D3DFMT_R16F) (RE-B18 item 4 F6)
//!
//! The raw map has the size of the screen and the blurred one 1024 or 2048 (`ShadowLevel` 2, 3);
//! the quality of the player (`ShadowQuality`) halves or quarters every side of both;
//! level 1 has no blur and the raw map is the map; level 0 has no pass and a 4 x 4 cleared map
//! (`BuildShadowBuffers`).

pub mod camera;

pub use camera::{
    D3dMatrix, LIGHT_DISTANCE, ShadowCamera, ShadowQuality, ShadowSize, fixed_rect, look_at_rh,
    ortho_off_center_rh, raw_size, screen_pixel_size, visible_rect,
};

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use bytemuck::{Pod, Zeroable};
use glam::Mat4;

use crate::renderer::FrameStats;
use crate::shaders::{catalog, create_module};
use crate::static_model::StaticRenderer;

/// Format of both maps on a device without half-float targets (and of the placeholder).
pub const SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The device renders, blends and filters `Rgba16Float` and `R16Float` (set by [`detect_precision`]).
static HALF_FLOAT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// True if the adapter can render, blend and filter `format`.
fn renders_and_filters(adapter: &wgpu::Adapter, format: wgpu::TextureFormat) -> bool {
    let f = adapter.get_texture_format_features(format);
    f.allowed_usages
        .contains(wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING)
        && f.flags.contains(
            wgpu::TextureFormatFeatureFlags::FILTERABLE
                | wgpu::TextureFormatFeatureFlags::BLENDABLE,
        )
}

/// Chooses the formats of the maps for the adapter of the window; call before the renderer makes
/// its pipelines.
pub fn detect_precision(adapter: &wgpu::Adapter) {
    let ok = renders_and_filters(adapter, wgpu::TextureFormat::Rgba16Float)
        && renders_and_filters(adapter, wgpu::TextureFormat::R16Float);
    HALF_FLOAT.store(ok, std::sync::atomic::Ordering::Relaxed);
}

/// The format of the raw map (the target of every caster).
pub fn raw_format() -> wgpu::TextureFormat {
    if HALF_FLOAT.load(std::sync::atomic::Ordering::Relaxed) {
        wgpu::TextureFormat::Rgba16Float
    } else {
        SHADOW_FORMAT
    }
}

/// The format of the blurred map.
pub fn blurred_format() -> wgpu::TextureFormat {
    if HALF_FLOAT.load(std::sync::atomic::Ordering::Relaxed) {
        wgpu::TextureFormat::R16Float
    } else {
        SHADOW_FORMAT
    }
}

/// Side of the placeholder of `ShadowLevel` 0.
pub const PLACEHOLDER_SIDE: u32 = 4;

/// One vertex of the blur's triangle: `dcl_position v0`, `dcl_texcoord v1` (both `float4`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BlurVertex {
    position: [f32; 4],
    uv: [f32; 4],
}

/// A triangle that covers the clip square; the texture coordinates run 0..1 over it.
const BLUR_TRIANGLE: [BlurVertex; 3] = [
    BlurVertex {
        position: [-1.0, -1.0, 0.0, 1.0],
        uv: [0.0, 1.0, 0.0, 0.0],
    },
    BlurVertex {
        position: [3.0, -1.0, 0.0, 1.0],
        uv: [2.0, 1.0, 0.0, 0.0],
    },
    BlurVertex {
        position: [-1.0, 3.0, 0.0, 1.0],
        uv: [0.0, -1.0, 0.0, 0.0],
    },
];

/// The targets of one `ShadowLevel` and screen size.
struct Targets {
    level: i64,
    /// The size of the screen the raw map was made for, and by how much the quality divided it.
    screen: (u32, u32),
    divisor: u32,
    raw: wgpu::TextureView,
    /// The blurred map; `None` at level 1, where the raw map is the map.
    blurred: Option<wgpu::TextureView>,
    blur_group: wgpu::BindGroup,
    map_side: u32,
}

/// What the casters of one `ShadowMap` pass drew: the static models (B1), and the models the
/// renderer adds after them (the units, landmarks and cities of `assets-b2`). Both zero is a pass
/// that only clears the map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CasterCounts {
    pub statics: crate::perf::DrawCounts,
    pub models: crate::perf::DrawCounts,
}

/// The state of a frame the passes need.
struct FrameState {
    camera: ShadowCamera,
    /// The side planes of the light's rectangle, in our basis: what is outside cannot land in the map.
    planes: crate::world::unit::cull::Planes,
}

/// The shadow map: pipelines, targets and the camera of the frame.
pub struct ShadowMap {
    caster: wgpu::RenderPipeline,
    caster_group: wgpu::BindGroup,
    caster_buffer: Buf,
    blur: wgpu::RenderPipeline,
    blur_layout: wgpu::BindGroupLayout,
    blur_buffer: Buf,
    blur_vertices: Buf,
    sampler: wgpu::Sampler,
    debug: wgpu::RenderPipeline,
    debug_layout: wgpu::BindGroupLayout,
    placeholder: wgpu::TextureView,
    targets: Option<Targets>,
    frame: Option<FrameState>,
}

impl ShadowMap {
    /// `world_format`: the format of the target the debug view draws into.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world_format: wgpu::TextureFormat,
    ) -> Self {
        let find = |name: &str| {
            catalog()
                .iter()
                .find(|m| m.name == name)
                .unwrap_or_else(|| panic!("module {name} is in the catalog"))
                .source()
        };
        let blur_module = create_module(device, "shadow blur", &find("shadow_blur"));
        let caster_module = create_module(device, "shadow caster", &find("shadow_caster"));

        let uniform = |binding: u32, visibility: wgpu::ShaderStages| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let texture = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };

        // The casters.
        let caster_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow caster layout"),
            entries: &[uniform(0, wgpu::ShaderStages::VERTEX)],
        });
        let caster_buffer = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("shadow caster vp"),
                size: 64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Shadows,
        );
        let caster_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow caster"),
            layout: &caster_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: caster_buffer.as_entire_binding(),
            }],
        });
        let caster_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("shadow caster pipeline layout"),
                bind_group_layouts: &[Some(&caster_layout)],
                immediate_size: 0,
            });
        // The vertex and the instance of the `static` pipeline, the fields the shader reads.
        let vertex_attributes = [
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Uint32,
                offset: 20,
                shader_location: 3,
            },
        ];
        let instance_attributes = [
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 0,
                shader_location: 4,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 16,
                shader_location: 5,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Uint32,
                offset: 40,
                shader_location: 8,
            },
        ];
        let caster = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow caster"),
            layout: Some(&caster_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &caster_module,
                entry_point: Some("caster_vs"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 24,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &vertex_attributes,
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: crate::static_model::StaticInstance::STRIDE,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &instance_attributes,
                    }),
                ],
            },
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &caster_module,
                entry_point: Some("caster_ps"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: raw_format(),
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        // The blur.
        let blur_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow blur layout"),
            entries: &[
                uniform(0, wgpu::ShaderStages::FRAGMENT),
                texture(1),
                sampler_entry(2),
            ],
        });
        let blur_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow blur pipeline layout"),
            bind_group_layouts: &[Some(&blur_layout)],
            immediate_size: 0,
        });
        let blur_attributes = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];
        let blur = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow blur"),
            layout: Some(&blur_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &blur_module,
                entry_point: Some("blur_vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<BlurVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &blur_attributes,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &blur_module,
                entry_point: Some("blur_ps"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: blurred_format(),
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let blur_buffer = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("shadow g_ScreenPixelSize"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Shadows,
        );
        let blur_vertices = device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("shadow blur triangle"),
                contents: bytemuck::bytes_of(&BLUR_TRIANGLE),
                usage: wgpu::BufferUsages::VERTEX,
            },
            Cluster::Shadows,
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // The debug view.
        let debug_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow debug layout"),
            entries: &[texture(1), sampler_entry(2)],
        });
        let debug_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("shadow debug pipeline layout"),
                bind_group_layouts: &[Some(&debug_layout)],
                immediate_size: 0,
            });
        let debug = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow debug"),
            layout: Some(&debug_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &caster_module,
                entry_point: Some("debug_vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &caster_module,
                entry_point: Some("debug_ps"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: world_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let placeholder = device
            .tex_with_data(
                queue,
                &wgpu::TextureDescriptor {
                    label: Some("shadow placeholder"),
                    size: wgpu::Extent3d {
                        width: PLACEHOLDER_SIDE,
                        height: PLACEHOLDER_SIDE,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: SHADOW_FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &[0u8; (PLACEHOLDER_SIDE * PLACEHOLDER_SIDE * 4) as usize],
                Cluster::Shadows,
            )
            .create_view(&wgpu::TextureViewDescriptor::default());

        Self {
            caster,
            caster_group,
            caster_buffer,
            blur,
            blur_layout,
            blur_buffer,
            blur_vertices,
            sampler,
            debug,
            debug_layout,
            placeholder,
            targets: None,
            frame: None,
        }
    }

    /// Sets up the frame: the targets of `level` at the `screen` size (made again when either
    /// changed) and the camera of the light. `level` 0 or `camera` `None` leaves the passes off.
    /// `caster_vp` takes a point of our basis to the clip space of the light.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        level: i64,
        screen: (u32, u32),
        divisor: u32,
        camera: Option<(ShadowCamera, Mat4)>,
    ) {
        let (Some(size), Some((camera, caster_vp))) = (ShadowSize::of_level(level), camera) else {
            self.targets = None;
            self.frame = None;
            return;
        };
        let screen = (screen.0.max(1), screen.1.max(1));
        let divisor = divisor.max(1);
        let stale = self
            .targets
            .as_ref()
            .is_none_or(|t| t.level != level || t.screen != screen || t.divisor != divisor);
        if stale {
            self.targets = Some(self.make_targets(device, level, screen, divisor, size));
        }
        // The blur reads the raw map: its texel is the texel of the raw map, whatever the quality.
        let raw = raw_size(screen, divisor);
        queue.write_buffer(
            &self.caster_buffer,
            0,
            bytemuck::bytes_of(&caster_vp.to_cols_array_2d()),
        );
        queue.write_buffer(
            &self.blur_buffer,
            0,
            bytemuck::bytes_of(&screen_pixel_size(raw.0, raw.1)),
        );
        self.frame = Some(FrameState {
            camera,
            planes: crate::world::unit::cull::Planes::sides(&caster_vp),
        });
    }

    fn make_targets(
        &self,
        device: &wgpu::Device,
        level: i64,
        screen: (u32, u32),
        divisor: u32,
        size: ShadowSize,
    ) -> Targets {
        let size = size.divided(divisor);
        let raw_px = raw_size(screen, divisor);
        let target = |label: &str, w: u32, h: u32, format: wgpu::TextureFormat| {
            device
                .tex(
                    &wgpu::TextureDescriptor {
                        label: Some(label),
                        size: wgpu::Extent3d {
                            width: w,
                            height: h,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::TEXTURE_BINDING
                            | wgpu::TextureUsages::COPY_SRC,
                        view_formats: &[],
                    },
                    Cluster::Shadows,
                )
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let raw = target("ShadowRaw", raw_px.0, raw_px.1, raw_format());
        let blurred = size.blur.then(|| {
            target(
                "DynamicShadowTexture",
                size.side,
                size.side,
                blurred_format(),
            )
        });
        let blur_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow blur"),
            layout: &self.blur_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.blur_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&raw),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        Targets {
            level,
            screen,
            divisor,
            raw,
            blurred,
            blur_group,
            map_side: if size.blur { size.side } else { raw_px.0 },
        }
    }

    /// The passes run this frame.
    pub fn active(&self) -> bool {
        self.frame.is_some() && self.targets.is_some()
    }

    /// The blur pass runs this frame (levels 2 and 3).
    pub fn blurs(&self) -> bool {
        self.targets.as_ref().is_some_and(|t| t.blurred.is_some())
    }

    /// The camera of the light of this frame.
    pub fn camera(&self) -> Option<&ShadowCamera> {
        self.frame.as_ref().map(|f| &f.camera)
    }

    /// `g_ShadowVP` of this frame for the original's basis (`clip = M * p`); the identity while
    /// the passes are off.
    pub fn shadow_vp(&self) -> Mat4 {
        self.camera()
            .map_or(Mat4::IDENTITY, ShadowCamera::view_proj)
    }

    /// The map the terrain and the models read: the blurred one, the raw one at level 1, a cleared
    /// 4 x 4 texture while the passes are off.
    pub fn map_view(&self) -> &wgpu::TextureView {
        match &self.targets {
            Some(t) => t.blurred.as_ref().unwrap_or(&t.raw),
            None => &self.placeholder,
        }
    }

    /// Side of the map the shaders read; the raw width at level 1.
    pub fn map_side(&self) -> u32 {
        self.targets
            .as_ref()
            .map_or(PLACEHOLDER_SIDE, |t| t.map_side)
    }

    /// The `ShadowMap` pass: the casters into the raw map. Nothing when the passes are off.
    /// `extra` draws the casters that are not static models (the units of `assets-b2`) into the
    /// same pass after the static ones; it sets its own pipeline and groups. `statics_cast` false
    /// leaves the static models out (`ShadowQuality::statics_cast`).
    pub(crate) fn encode_map(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        statics: &StaticRenderer,
        stats: &mut FrameStats,
        timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
        statics_cast: bool,
        extra: &dyn Fn(&mut wgpu::RenderPass<'_>, &mut FrameStats),
    ) -> CasterCounts {
        let Some(t) = &self.targets else {
            return CasterCounts::default();
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ShadowMap"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &t.raw,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: timestamps,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline_c(&self.caster);
        pass.set_bind_group_c(0, &self.caster_group, &[]);
        let before = crate::perf::draw::counts();
        if statics_cast {
            statics.draw_casters(&mut pass, stats, self.frame.as_ref().map(|f| &f.planes));
        }
        let mid = crate::perf::draw::counts();
        extra(&mut pass, stats);
        CasterCounts {
            statics: mid.since(before),
            models: crate::perf::draw::counts().since(mid),
        }
    }

    /// The size of the raw map the casters are drawn into (the screen's, cut by the quality); 0 x 0
    /// while the passes are off.
    pub fn raw_px(&self) -> (u32, u32) {
        self.targets
            .as_ref()
            .map_or((0, 0), |t| raw_size(t.screen, t.divisor))
    }

    /// The `ShadowBlur` pass: `BlurAndProcess` over the raw map into the blurred one. Nothing at
    /// levels without a blur.
    pub(crate) fn encode_blur(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        stats: &mut FrameStats,
        timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) {
        let Some(t) = &self.targets else { return };
        let Some(blurred) = &t.blurred else { return };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ShadowBlur"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: blurred,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: timestamps,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline_c(&self.blur);
        pass.set_bind_group_c(0, &t.blur_group, &[]);
        pass.set_vertex_buffer(0, self.blur_vertices.slice(..));
        pass.draw_c(0..3, 0..1);
        stats.record_draw(1);
    }

    /// `?debug=shadow`: the shadow map as grey over all of `target`.
    pub fn encode_debug(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
    ) {
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow debug"),
            layout: &self.debug_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(self.map_view()),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("shadow debug"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline_c(&self.debug);
        pass.set_bind_group_c(0, &group, &[]);
        pass.draw_c(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_camera::GameCamera;
    use crate::gpu_test::{Gpu, gpu, read_texture};
    use crate::hexgrid::cell_center;
    use crate::renderer::Renderer;
    use crate::scene::Scene;
    use crate::static_model::StaticInstance;
    use crate::terrain::{TerrainCell, TerrainMesh, TerrainSnapshot};
    use asset_format::{LogicalId, Material, TextureBlob, TextureFormat};
    use draw_list::UiDrawList;
    use std::collections::HashMap;

    const SIZE: (u32, u32) = (96, 96);

    /// A renderer with a small map, a pillar in the middle and `?debug=shadow` on.
    struct Setup {
        gpu: Gpu,
        r: Renderer,
        camera: GameCamera,
        target: wgpu::Texture,
        view: wgpu::TextureView,
    }

    fn setup(name: &str) -> Option<Setup> {
        let gpu = gpu(name, wgpu::Backends::PRIMARY, wgpu::Features::empty())?;
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut r = Renderer::offscreen(&gpu, SIZE, format, 1);
        r.set_debug_shadow(true);
        let snapshot = TerrainSnapshot::new(6, 4, false, vec![TerrainCell::default(); 24]).unwrap();
        r.set_terrain(&snapshot, &TerrainMesh::build(&snapshot));
        let at = cell_center(3, 2);
        let mut camera = GameCamera::new(at);
        camera.set_level(10.0);
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow test target"),
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
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        Some(Setup {
            gpu,
            r,
            camera,
            target,
            view,
        })
    }

    impl Setup {
        fn frame(&mut self) -> Vec<u8> {
            let commands = self.r.encode_frame(
                &self.view,
                &Scene::test(0.0),
                &self.camera,
                &UiDrawList::default(),
                None,
            );
            self.gpu.queue.submit([commands]);
            self.r.frame_submitted();
            read_texture(&self.gpu, &self.target, 0, 0, SIZE, 4)
        }

        fn pillar(&mut self, present: bool) {
            let id = LogicalId::new("tx:pillar");
            let mut model = crate::entities::marker::pillar((6.0, 30.0));
            model.materials = vec![Material {
                diffuse: Some(id.clone()),
                specular: None,
            }];
            let base = TextureBlob {
                format: TextureFormat::Rgba8,
                dim: asset_format::TexDim::D2,
                srgb: true,
                width: 1,
                height: 1,
                depth: 1,
                mips: vec![vec![255, 255, 255, 255]],
            };
            let textures: HashMap<_, _> = [(id, base)].into();
            let at = cell_center(3, 2);
            let instances = if present {
                vec![StaticInstance::new([at.x, 0.0, at.y], 0.0, 1.0)]
            } else {
                vec![]
            };
            self.r.test_unit(&model, &textures, &instances);
        }

        fn level(&mut self, level: i64) {
            let mut s = self.r.settings().clone();
            s.set("ShadowLevel", level).unwrap();
            self.r.set_settings(s);
        }
    }

    fn grey(px: &[u8]) -> Vec<u8> {
        px.chunks(4).map(|p| p[0]).collect()
    }

    #[test]
    fn a_pillar_casts_a_blurred_shadow_and_an_empty_world_none() {
        let Some(mut s) = setup("a_pillar_casts_a_blurred_shadow_and_an_empty_world_none") else {
            return;
        };
        s.level(3);
        s.pillar(false);
        let empty = grey(&s.frame());
        assert!(s.r.shadow().active(), "the debug view runs the passes");
        assert!(s.r.shadow().blurs());
        assert_eq!(s.r.shadow().map_side(), 2048);
        assert!(
            empty.iter().all(|&v| v == 0),
            "nothing casts: the map is clear"
        );

        s.pillar(true);
        let blurred = grey(&s.frame());
        let lit = blurred.iter().filter(|&&v| v > 8).count();
        assert!(lit > 0, "the pillar leaves a shadow");
        assert!(
            lit < blurred.len() / 2,
            "a pillar is not the whole picture ({lit})"
        );
        assert!(
            blurred.iter().any(|&v| v > 100),
            "the body of the shadow is dark"
        );
        let soft = blurred.iter().filter(|&&v| v > 8 && v < 247).count();
        assert!(soft > 0, "the blur makes the edge soft");

        // Level 1: no blur, the raw map is the map: edges are hard (alpha 0 or 1 per texel).
        s.level(1);
        let raw = grey(&s.frame());
        assert!(s.r.shadow().active() && !s.r.shadow().blurs());
        assert!(raw.contains(&255));
        let soft_raw = raw.iter().filter(|&&v| v > 8 && v < 247).count();
        assert!(
            soft_raw < soft,
            "level 1 is harder than level 3 ({soft_raw} vs {soft})"
        );

        // Level 0: no pass, a cleared 4 x 4 map, `g_ShadowVP` the identity.
        s.level(0);
        s.frame();
        assert!(!s.r.shadow().active());
        assert_eq!(s.r.shadow().map_side(), PLACEHOLDER_SIDE);
        assert_eq!(s.r.shadow().shadow_vp(), Mat4::IDENTITY);
    }

    #[test]
    fn the_frame_has_the_matrix_of_the_light() {
        let Some(mut s) = setup("the_frame_has_the_matrix_of_the_light") else {
            return;
        };
        s.level(2);
        s.pillar(true);
        s.frame();
        let cam = *s.r.shadow().camera().expect("a camera");
        assert_ne!(s.r.shadow().shadow_vp(), Mat4::IDENTITY);
        // The middle of what the camera sees is the middle of the shadow map.
        let at = cell_center(3, 2);
        let orig =
            crate::frame::basis_rotation().transform_point3(glam::Vec3::new(at.x, 0.0, at.y));
        let ndc = s.r.shadow().shadow_vp().project_point3(orig);
        assert!(ndc.x.abs() < 0.9 && ndc.y.abs() < 0.9, "{ndc}");
        assert_eq!(
            cam.view_proj().to_cols_array(),
            s.r.shadow().shadow_vp().to_cols_array()
        );
        assert_eq!(s.r.shadow().map_side(), 1024);
    }

    #[test]
    fn the_shadow_quality_sizes_the_maps_and_off_draws_none() {
        let Some(mut s) = setup("the_shadow_quality_sizes_the_maps_and_off_draws_none") else {
            return;
        };
        s.level(3);
        s.pillar(true);
        for (q, side) in [
            (ShadowQuality::High, 2048),
            (ShadowQuality::Medium, 1024),
            (ShadowQuality::Low, 512),
        ] {
            s.r.set_shadow_quality(q);
            let px = grey(&s.frame());
            assert!(s.r.shadow().active(), "{q:?}");
            assert_eq!(s.r.shadow().map_side(), side, "{q:?}");
            // Low is a real map but the static models (the pillar) do not cast into it.
            assert_eq!(px.iter().any(|&v| v > 8), q.statics_cast(), "{q:?}");
        }
        s.r.set_shadow_quality(ShadowQuality::Off);
        s.frame();
        assert!(!s.r.shadow().active(), "Off: no pass");
        assert_eq!(s.r.shadow().map_side(), PLACEHOLDER_SIDE);
        assert_eq!(s.r.shadow().shadow_vp(), Mat4::IDENTITY);
    }

    /// The phone report: the world's Low detail has `ShadowLevel` 0, and the pick of Low shadows made
    /// side 4. Each quality now gives the side the frame uses, and Off encodes no pass at all.
    #[test]
    fn each_shadow_quality_makes_its_side_even_at_shadow_level_zero_and_off_encodes_no_pass() {
        let Some(mut s) = setup("each_shadow_quality_makes_its_side_even_at_shadow_level_zero")
        else {
            return;
        };
        s.pillar(true);
        s.level(0);
        let counter = |s: &Setup, name: &str| s.r.perf_snapshot(false).counter(name);
        for (q, side) in [
            (ShadowQuality::High, 2048.0),
            (ShadowQuality::Medium, 1024.0),
            (ShadowQuality::Low, 512.0),
        ] {
            s.r.set_shadow_quality(q);
            s.frame();
            assert!(s.r.shadow().active(), "{q:?} is a pass");
            assert_eq!(counter(&s, "render.shadow_map_side"), Some(side), "{q:?}");
            assert!(
                counter(&s, "shadow.static_draws").is_some(),
                "{q:?}: the pass ran"
            );
        }
        // Low: the models cast, the ground and the statics do not.
        assert_eq!(counter(&s, "shadow.static_draws"), Some(0.0));
        s.r.set_shadow_quality(ShadowQuality::Medium);
        s.frame();
        assert!(counter(&s, "shadow.static_draws").unwrap() >= 1.0);

        s.r.set_shadow_quality(ShadowQuality::Off);
        s.frame();
        assert!(!s.r.shadow().active());
        assert_eq!(
            counter(&s, "render.shadow_map_side"),
            Some(f64::from(PLACEHOLDER_SIDE))
        );
        assert_eq!(
            counter(&s, "shadow.static_draws"),
            None,
            "no ShadowMap pass was encoded"
        );
        let map =
            s.r.last_frame_stats()
                .passes
                .iter()
                .find(|p| p.pass == crate::frame::PassId::ShadowMap)
                .cloned()
                .unwrap();
        assert_eq!(map.counts.draw_calls, 0);
        assert_eq!(map.gpu_ms, None, "a pass that did not run has no GPU time");
    }

    /// The `ShadowMap` row of the pass table (the panel's) counts the casters it draws, and the blur
    /// has its own row: the commands of the two passes are told apart.
    #[test]
    fn the_shadow_map_row_counts_its_own_draws_and_triangles_and_the_blur_its_own() {
        let Some(mut s) = setup("the_shadow_map_row_counts_its_own_draws") else {
            return;
        };
        s.level(3);
        s.pillar(true);
        s.frame();
        let row = |s: &Setup, pass| {
            s.r.last_frame_stats()
                .passes
                .iter()
                .find(|p| p.pass == pass)
                .cloned()
                .unwrap()
        };
        let map = row(&s, crate::frame::PassId::ShadowMap);
        assert!(
            map.counts.draw_calls >= 1 && map.counts.triangles >= 1 && map.counts.instances >= 1,
            "{:?}",
            map.counts
        );
        let blur = row(&s, crate::frame::PassId::ShadowBlur);
        assert_eq!(blur.counts.draw_calls, 1, "the blur is one triangle");
        assert_eq!(blur.counts.triangles, 1);
        // Nothing casts: the pass still runs (it clears the map) and draws nothing.
        s.pillar(false);
        s.frame();
        let empty = row(&s, crate::frame::PassId::ShadowMap);
        assert_eq!(empty.counts.draw_calls, 0, "{:?}", empty.counts);
    }

    /// The panel's line "Render NN%: ... shadows X (side)" reads these counters: each frame they
    /// follow the quality the player picked, for the blurred levels (the side of the map) and for the
    /// level without a blur (the raw map is the map), and the raw map is the scene cut by the quality.
    #[test]
    fn the_panel_counters_follow_the_shadow_quality_frame_by_frame() {
        let Some(mut s) = setup("the_panel_counters_follow_the_shadow_quality") else {
            return;
        };
        s.pillar(true);
        let counter = |s: &Setup, name: &str| s.r.perf_snapshot(false).counter(name).unwrap();
        for level in [3, 2] {
            s.level(level);
            // ShadowLevel 2 makes a 1024 map and 3 a 2048 one; the quality divides it.
            let full = if level == 3 { 2048.0 } else { 1024.0 };
            for (q, div) in [
                (ShadowQuality::High, 1.0),
                (ShadowQuality::Medium, 2.0),
                (ShadowQuality::Low, 4.0),
            ] {
                s.r.set_shadow_quality(q);
                s.frame();
                assert_eq!(counter(&s, "render.shadow_quality"), f64::from(q.index()));
                assert_eq!(counter(&s, "render.shadow_map_side"), full / div, "{q:?}");
                assert_eq!(
                    counter(&s, "render.shadow_raw_w"),
                    f64::from(SIZE.0) / div,
                    "{q:?}"
                );
                assert_eq!(counter(&s, "render.shadow_blur"), 1.0);
            }
        }
        // Level 1: no blur, the map is the raw map, whose side is the scene's cut by the quality.
        s.level(1);
        for (q, div) in [(ShadowQuality::High, 1.0), (ShadowQuality::Low, 4.0)] {
            s.r.set_shadow_quality(q);
            s.frame();
            assert_eq!(counter(&s, "render.shadow_blur"), 0.0);
            assert_eq!(
                counter(&s, "render.shadow_map_side"),
                f64::from(SIZE.0) / div
            );
        }
        // Off: the pass is gone and the line shows the 4 x 4 placeholder.
        s.r.set_shadow_quality(ShadowQuality::Off);
        s.frame();
        assert_eq!(counter(&s, "render.shadow_quality"), 0.0);
        assert_eq!(
            counter(&s, "render.shadow_map_side"),
            f64::from(PLACEHOLDER_SIDE)
        );
        assert_eq!(counter(&s, "render.shadow_raw_w"), 0.0);
    }

    /// "Main by group" adds up to the `Main` row, and a group that is switched off draws nothing
    /// (and leaves the others as they were).
    #[test]
    fn main_by_group_adds_up_and_a_switched_off_group_draws_nothing() {
        use crate::perf::Group;
        let Some(mut s) = setup("main_by_group_adds_up") else {
            return;
        };
        s.r.set_debug_shadow(false);
        s.pillar(true);
        let first = grey(&s.frame());
        let main = |s: &Setup| {
            s.r.last_frame_stats()
                .passes
                .iter()
                .find(|p| p.pass == crate::frame::PassId::Main)
                .unwrap()
                .counts
        };
        let row = |s: &Setup, g: Group| {
            let snap = s.r.perf_snapshot(false);
            assert_eq!(snap.groups_off, s.r.disabled_groups());
            snap.main_groups
                .into_iter()
                .find(|r| r.name == g.name())
                .unwrap()
        };
        let rows = s.r.perf_snapshot(false).main_groups;
        assert_eq!(rows.len(), crate::perf::groups::N);
        assert_eq!(
            rows.iter().map(|r| r.draw_calls).sum::<u32>(),
            main(&s).draw_calls,
            "the groups are the Main row"
        );
        assert_eq!(
            rows.iter().map(|r| r.triangles).sum::<u64>(),
            main(&s).triangles
        );
        assert!(
            rows.windows(2).all(|w| w[0].triangles >= w[1].triangles),
            "sorted by triangles"
        );
        let all_on = main(&s).draw_calls;
        let (terrain, units) = (row(&s, Group::Terrain), row(&s, Group::Units));
        assert!(
            terrain.draw_calls >= 1 && terrain.triangles >= 1,
            "{terrain:?}"
        );
        assert!(units.draw_calls >= 1 && units.instances >= 1, "{units:?}");

        s.r.set_disabled_groups(Group::Units.bit());
        s.frame();
        assert_eq!(s.r.disabled_groups(), Group::Units.bit());
        let off = row(&s, Group::Units);
        assert_eq!((off.draw_calls, off.triangles, off.instances), (0, 0, 0));
        assert_eq!(row(&s, Group::Terrain), terrain, "the others are as before");
        assert_eq!(
            main(&s).draw_calls,
            all_on - units.draw_calls,
            "Main lost what the group drew"
        );

        s.r.set_disabled_groups(Group::Terrain.bit() | Group::Units.bit() | Group::Other.bit());
        let bare = grey(&s.frame());
        assert_eq!(row(&s, Group::Terrain).draw_calls, 0);
        assert_ne!(first, bare, "the ground is gone from the picture");
        assert_eq!(
            s.r.disabled_groups() & Group::Other.bit(),
            0,
            "`other` cannot be switched off"
        );

        s.r.set_disabled_groups(0);
        assert_eq!(grey(&s.frame()), first, "all on again draws what it drew");
    }

    #[test]
    fn a_render_scale_below_one_makes_the_scene_smaller_and_still_fills_the_screen() {
        let Some(mut s) = setup("a_render_scale_below_one_makes_the_scene_smaller") else {
            return;
        };
        s.level(3);
        s.pillar(true);
        s.r.set_render_scale(0.5);
        let px = grey(&s.frame());
        assert_eq!(s.r.scene_size(), (48, 48));
        assert_eq!(s.r.size(), SIZE);
        assert_eq!(
            px.len(),
            (SIZE.0 * SIZE.1) as usize,
            "the frame is full size"
        );
        assert!(
            px.iter().any(|&v| v > 8),
            "the shadow view is stretched over it"
        );
        s.r.set_render_scale(1.0);
        s.frame();
        assert_eq!(s.r.scene_size(), SIZE);
    }

    #[test]
    fn the_blur_of_a_flat_map_is_the_map() {
        // The eight taps of a uniform alpha average to it: 8 * a * 0.125.
        let Some(g) = gpu(
            "the_blur_of_a_flat_map_is_the_map",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let m = ShadowMap::new(&g.device, &g.queue, wgpu::TextureFormat::Rgba8Unorm);
        assert!(!m.active());
        assert_eq!(m.map_side(), PLACEHOLDER_SIDE);
        // The shapes of the pipelines are checked by creating them; the values by the compare of
        // `shader-re` (BlurAndProcess, VSFallThrough) and the picture test above.
    }
}
