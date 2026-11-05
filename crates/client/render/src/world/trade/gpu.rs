//! The trade routes into the map of the overlay (`OverlaySystem::Render` 0x088c659e, `TradeArrowShader`).
//!
//! One draw per type of route and copy of the map, production first, then food, then international,
//! with the constants of the shader: the projection of the overlay at +0, the scroll phase at +0x40.
//! The state is the overlay's own (`m_hAlphaState`): ONE, INV_SRC_ALPHA, the pixel shader writes
//! the colour times its alpha; no depth, no culling.

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use std::cell::Cell;
use std::collections::HashMap;
use std::ops::Range;

use asset_format::{LogicalId, TextureBlob};
use bytemuck::{Pod, Zeroable};
use glam::Mat4;

use super::{TEXTURES, TradeKind, TradeMesh, TradeVertex};
use crate::frame::WORLD_FORMAT;
use crate::world::overlay::borders::gpu::local_matrix;

/// Copies of the map at most (the map and a neighbour on each side).
pub const MAX_COPIES: usize = 3;
/// Stride of a constant block (`min_uniform_buffer_offset_alignment`).
const SLOT: u64 = 256;
/// The draw order of the batches: production, food, international.
const ORDER: [usize; 6] = [
    TradeKind::Production.index(),
    TradeKind::Food.index(),
    TradeKind::International.index(),
    // Then the popup route, last (`Render` 0x089f2ec8: surfaces 3, 2, 1, 0).
    TradeKind::Production.popup_index(),
    TradeKind::Food.popup_index(),
    TradeKind::International.popup_index(),
];

/// The constants of `TradeArrowShader`: the projection and `Local` (rows as registers), the scroll
/// phase in `.x`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct TradeConsts {
    pub projection: [[f32; 4]; 4],
    pub local: [[f32; 4]; 4],
    pub phase: [f32; 4],
}

/// What a frame draws the routes with.
#[derive(Clone, Copy)]
pub struct TradeFrame<'a> {
    pub projection: &'a Mat4,
    pub ours_to_orig: &'a Mat4,
    /// The X shifts of the copies of the map.
    pub shifts: &'a [f32],
    /// The scroll phase in `[0, 1)`.
    pub phase: f32,
}

pub struct TradeGpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    consts: Buf,
    consts_group: wgpu::BindGroup,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// The texture of each type; a white texel until it arrives, so the strip still shows.
    groups: [wgpu::BindGroup; 6],
    vertices: Option<Buf>,
    indices: Option<Buf>,
    batches: [Range<u32>; 6],
    copies: Cell<usize>,
}

impl TradeGpu {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let module = crate::shaders::create_module(
            device,
            "overlay trade",
            crate::wgsl_module!("overlay/trade.wgsl"),
        );
        let consts_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay trade consts"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<TradeConsts>() as u64
                    ),
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay trade texture"),
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
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay trade"),
            bind_group_layouts: &[Some(&consts_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let premultiplied = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay trade"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_trade"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<TradeVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3, 1 => Unorm8x4, 2 => Float32x2
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
                module: &module,
                entry_point: Some("ps_trade"),
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
        });
        let consts = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("overlay trade consts"),
                size: SLOT * MAX_COPIES as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        let consts_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay trade consts"),
            layout: &consts_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &consts,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<TradeConsts>() as u64),
                }),
            }],
        });
        // `ArrowTexture` repeats along the route (`v` runs past 1) and stops at the sides.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("overlay trade"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let white = device
            .tex_with_data(
                queue,
                &wgpu::TextureDescriptor {
                    label: Some("overlay trade white"),
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
        let groups = std::array::from_fn(|_| group(device, &texture_layout, &white, &sampler));
        Self {
            device: device.clone(),
            queue: queue.clone(),
            pipeline,
            consts,
            consts_group,
            texture_layout,
            sampler,
            groups,
            vertices: None,
            indices: None,
            batches: [0..0, 0..0, 0..0, 0..0, 0..0, 0..0],
            copies: Cell::new(0),
        }
    }

    /// The textures of the three types from `textures`; returns how many were taken. A type
    /// without one keeps the white texel.
    pub fn set_textures(&mut self, textures: &HashMap<LogicalId, TextureBlob>) -> usize {
        let mut taken = 0;
        for (slot, id) in TEXTURES.into_iter().enumerate() {
            let Some(blob) = textures.get(&LogicalId::new(id)) else {
                continue;
            };
            let view = match crate::texture::upload_blob(
                &self.device,
                &self.queue,
                blob,
                id,
                crate::texture::UploadOptions::WORLD,
            ) {
                Ok(up) => {
                    taken += 1;
                    up.texture
                        .create_view(&wgpu::TextureViewDescriptor::default())
                }
                Err(e) => {
                    tracing::warn!("{id} left out: {e}");
                    continue;
                }
            };
            self.groups[slot] = group(&self.device, &self.texture_layout, &view, &self.sampler);
        }
        taken
    }

    /// Uploads a new mesh.
    pub fn set_mesh(&mut self, device: &wgpu::Device, mesh: &TradeMesh) {
        self.batches = mesh.batches.clone();
        if mesh.indices.is_empty() {
            self.vertices = None;
            self.indices = None;
            return;
        }
        self.vertices = Some(device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("overlay trade vertices"),
                contents: bytemuck::cast_slice(&mesh.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            },
            Cluster::Other,
        ));
        self.indices = Some(device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("overlay trade indices"),
                contents: bytemuck::cast_slice(&mesh.indices),
                usage: wgpu::BufferUsages::INDEX,
            },
            Cluster::Other,
        ));
    }

    /// The constants of a frame: one block per copy of the map.
    pub fn set_frame(&self, frame: &TradeFrame<'_>) {
        self.copies.set(frame.shifts.len().min(MAX_COPIES));
        for (i, &shift) in frame.shifts.iter().take(MAX_COPIES).enumerate() {
            let consts = TradeConsts {
                projection: crate::world::fow::mask::row_major(frame.projection),
                local: crate::world::fow::mask::row_major(&local_matrix(frame.ours_to_orig, shift)),
                phase: [frame.phase, 0.0, 0.0, 0.0],
            };
            self.queue
                .write_buffer(&self.consts, i as u64 * SLOT, bytemuck::bytes_of(&consts));
        }
    }

    /// Draws every batch on every copy; returns the index counts of the draws.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) -> Vec<u32> {
        let (Some(v), Some(i)) = (&self.vertices, &self.indices) else {
            return Vec::new();
        };
        let mut sizes = Vec::new();
        pass.set_pipeline_c(&self.pipeline);
        pass.set_vertex_buffer(0, v.slice(..));
        pass.set_index_buffer(i.slice(..), wgpu::IndexFormat::Uint32);
        for copy in 0..self.copies.get() {
            pass.set_bind_group_c(0, &self.consts_group, &[(copy as u64 * SLOT) as u32]);
            for slot in ORDER {
                let range = self.batches[slot].clone();
                if range.is_empty() {
                    continue;
                }
                pass.set_bind_group_c(1, &self.groups[slot], &[]);
                sizes.push(range.end - range.start);
                pass.draw_indexed_c(range, 0, 0..1);
            }
        }
        sizes
    }
}

fn group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("overlay trade texture"),
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
