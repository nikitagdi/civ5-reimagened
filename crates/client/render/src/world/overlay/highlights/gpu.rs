//! The highlights into the map of the overlay (world-overlays WOV5): one draw per [`Draw`] of the
//! mesh and per visible copy of the map, through `VSOverlay` (`overlay.wgsl`) with the pixel shader of
//! the draw's kind: `PSContour`, `PSOverlay`, `PSNoTexture` (`highlight.wgsl`), or `PSSimpleTexture`
//! times the colour (`grid_ps`). Blend ONE, INV_SRC_ALPHA, no depth.
//!
//! Groups: 0 the constants of `VSOverlay` (a block per copy, dynamic offset), 1 the texture and its
//! sampler (a white texel where a style has none or it is not there), 2 `DrawColor` (a block per
//! draw, dynamic offset).

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use std::collections::HashMap;

use glam::Mat4;

use super::{Draw, DrawKind, HighlightMesh};
use crate::frame::WORLD_FORMAT;
use crate::world::overlay::borders::gpu::local_matrix;
use crate::world::overlay::grid::GridVertex;

const SLOT: u64 = 256;
/// Copies of the map at most.
const MAX_COPIES: usize = 3;
/// Draws at most (the styles in use at a time are few).
pub const MAX_DRAWS: usize = 64;

pub struct HighlightGpu {
    pipelines: [wgpu::RenderPipeline; 4],
    vs_consts: Buf,
    vs_group: wgpu::BindGroup,
    colors: Buf,
    color_group: wgpu::BindGroup,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    white: wgpu::BindGroup,
    textures: HashMap<String, wgpu::BindGroup>,
    vertices: Option<Buf>,
    indices: Option<Buf>,
    draws: Vec<Draw>,
    copies: usize,
}

fn kind_index(k: DrawKind) -> usize {
    match k {
        DrawKind::Contour => 0,
        DrawKind::Plain => 1,
        DrawKind::Filled => 2,
        DrawKind::Static => 3,
    }
}

impl HighlightGpu {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let vs_module = crate::shaders::create_module(
            device,
            "overlay vs",
            crate::wgsl_module!("overlay/overlay.wgsl"),
        );
        let ps_module = crate::shaders::create_module(
            device,
            "overlay highlight",
            crate::wgsl_module!("overlay/highlight.wgsl"),
        );
        let uniform = |binding: u32, vis: wgpu::ShaderStages, dynamic: bool, size: u64| {
            wgpu::BindGroupLayoutEntry {
                binding,
                visibility: vis,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: dynamic,
                    min_binding_size: wgpu::BufferSize::new(size),
                },
                count: None,
            }
        };
        let vs_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("highlight vs"),
            entries: &[uniform(0, wgpu::ShaderStages::VERTEX, true, 256)],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("highlight texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let color_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("highlight color"),
            entries: &[uniform(0, wgpu::ShaderStages::FRAGMENT, true, 16)],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("highlight"),
            bind_group_layouts: &[Some(&vs_layout), Some(&texture_layout), Some(&color_layout)],
            immediate_size: 0,
        });
        let premultiplied = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let make = |label: &str, fs: (&wgpu::ShaderModule, &str)| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &vs_module,
                    entry_point: Some("vsoverlay"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<GridVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Float32x4, 1 => Float32x4, 2 => Float32x2
                        ],
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: fs.0,
                    entry_point: Some(fs.1),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: WORLD_FORMAT,
                        blend: Some(wgpu::BlendState {
                            color: premultiplied,
                            alpha: premultiplied,
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipelines = [
            make("highlight contour", (&ps_module, "pscontour")),
            make("highlight plain", (&ps_module, "psoverlay")),
            make("highlight filled", (&ps_module, "psnotexture")),
            make("highlight static", (&vs_module, "grid_ps")),
        ];
        let vs_consts = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("highlight vs consts"),
                size: SLOT * MAX_COPIES as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        let vs_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("highlight vs"),
            layout: &vs_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &vs_consts,
                    offset: 0,
                    size: wgpu::BufferSize::new(256),
                }),
            }],
        });
        let colors = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("highlight colors"),
                size: SLOT * MAX_DRAWS as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        let color_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("highlight color"),
            layout: &color_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &colors,
                    offset: 0,
                    size: wgpu::BufferSize::new(16),
                }),
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("highlight"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        let white_view = device
            .tex_with_data(
                queue,
                &wgpu::TextureDescriptor {
                    label: Some("highlight white"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: WORLD_FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &[255, 255, 255, 255],
                Cluster::Other,
            )
            .create_view(&wgpu::TextureViewDescriptor::default());
        let white = Self::group(device, &texture_layout, &white_view, &sampler);
        Self {
            pipelines,
            vs_consts,
            vs_group,
            colors,
            color_group,
            texture_layout,
            sampler,
            white,
            textures: HashMap::new(),
            vertices: None,
            indices: None,
            draws: Vec::new(),
            copies: 0,
        }
    }

    fn group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("highlight texture"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }

    /// A texture a style names (`id` as the style has it). Returns false if it cannot be used.
    pub fn set_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        id: &str,
        blob: &asset_format::TextureBlob,
    ) -> bool {
        match crate::texture::upload_blob(
            device,
            queue,
            blob,
            id,
            crate::texture::UploadOptions::WORLD,
        ) {
            Ok(up) => {
                let view = up
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                let g = Self::group(device, &self.texture_layout, &view, &self.sampler);
                self.textures.insert(id.to_string(), g);
                true
            }
            Err(e) => {
                tracing::warn!("{id} left out: {e}");
                false
            }
        }
    }

    pub fn has_texture(&self, id: &str) -> bool {
        self.textures.contains_key(id)
    }

    /// Uploads a new mesh and the colours of its draws.
    pub fn set_mesh(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, mesh: &HighlightMesh) {
        self.draws = mesh.draws.iter().take(MAX_DRAWS).cloned().collect();
        if mesh.indices.is_empty() {
            self.vertices = None;
            self.indices = None;
            return;
        }
        self.vertices = Some(device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("highlight vertices"),
                contents: bytemuck::cast_slice(&mesh.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            },
            Cluster::Other,
        ));
        self.indices = Some(device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("highlight indices"),
                contents: bytemuck::cast_slice(&mesh.indices),
                usage: wgpu::BufferUsages::INDEX,
            },
            Cluster::Other,
        ));
        for (i, d) in self.draws.iter().enumerate() {
            queue.write_buffer(
                &self.colors,
                i as u64 * SLOT,
                bytemuck::cast_slice(&d.color),
            );
        }
    }

    /// The constants of a frame: the projection of the map, a block per copy of the map.
    pub fn set_frame(
        &mut self,
        queue: &wgpu::Queue,
        projection: &Mat4,
        ours_to_orig: &Mat4,
        shifts: &[f32],
    ) {
        self.copies = shifts.len().min(MAX_COPIES);
        let id = Mat4::IDENTITY.to_cols_array_2d();
        for (i, &shift) in shifts.iter().take(MAX_COPIES).enumerate() {
            let rows = crate::world::fow::mask::row_major;
            // `VSOverlay`: `WVP`, `Local`, `TransScaleRot`, `TransScale`.
            let block = [
                rows(projection),
                rows(&local_matrix(ours_to_orig, shift)),
                id,
                id,
            ];
            queue.write_buffer(
                &self.vs_consts,
                i as u64 * SLOT,
                bytemuck::cast_slice(&block),
            );
        }
    }

    /// Indices of the draws of one copy (0 with no mesh).
    pub fn index_count(&self) -> u32 {
        if self.vertices.is_some() {
            self.draws.iter().map(|d| d.index_count).sum()
        } else {
            0
        }
    }

    pub fn draw_count(&self) -> usize {
        self.draws.len()
    }

    /// Draws every copy; returns the indices of every draw call.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) -> Vec<u32> {
        let (Some(vertices), Some(indices)) = (&self.vertices, &self.indices) else {
            return Vec::new();
        };
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        let mut sizes = Vec::new();
        for copy in 0..self.copies {
            pass.set_bind_group_c(0, &self.vs_group, &[(copy as u64 * SLOT) as u32]);
            for (i, d) in self.draws.iter().enumerate() {
                pass.set_pipeline_c(&self.pipelines[kind_index(d.kind)]);
                let tex = d
                    .texture
                    .as_ref()
                    .and_then(|t| self.textures.get(t.as_str()))
                    .unwrap_or(&self.white);
                pass.set_bind_group_c(1, tex, &[]);
                pass.set_bind_group_c(2, &self.color_group, &[(i as u64 * SLOT) as u32]);
                pass.draw_indexed_c(d.first_index..d.first_index + d.index_count, 0, 0..1);
                sizes.push(d.index_count);
            }
        }
        sizes
    }
}
