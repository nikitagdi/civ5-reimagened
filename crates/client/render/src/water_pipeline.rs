//! The `water` pipeline (SIMP-008): the water cells of the terrain mesh
//! drawn a second time, alpha-blended over the seabed the terrain pass drew. Same vertex buffer and
//! index buffer as the terrain, only the water index range; groups 0 globals, 1 object, 2 fog-state
//! texture (as the terrain pass), 3 the bump map of the surface.
//!
//! WebGL2-compatible: uniform buffers, one 2D texture, one non-filtered `R8Uint` texture.

use asset_format::TextureBlob;

use crate::terrain::texture_array::TerrainGpuVertex;
use crate::texture;

/// Byte offsets of the vertex fields used here (`TerrainGpuVertex`).
const CELL_OFFSET: u64 = 24;
const DEPTH_OFFSET: u64 = 32;
const SHORE_OFFSET: u64 = 36;

pub(crate) struct WaterTargets {
    pub view_format: wgpu::TextureFormat,
    pub depth_format: wgpu::TextureFormat,
    /// Samples of the main pass (1 or 4).
    pub samples: u32,
}

pub(crate) struct WaterPipeline {
    pub pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Group 3: the bump map (a flat grey 1x1 until the real one arrives: no ripples).
    pub bind_group: wgpu::BindGroup,
}

impl WaterPipeline {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        globals_layout: &wgpu::BindGroupLayout,
        object_layout: &wgpu::BindGroupLayout,
        fog_layout: &wgpu::BindGroupLayout,
        targets: WaterTargets,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("water.wgsl"));
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water layout"),
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water pipeline layout"),
            bind_group_layouts: &[
                Some(globals_layout),
                Some(object_layout),
                Some(fog_layout),
                Some(&layout),
            ],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("water pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<TerrainGpuVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32,
                            offset: CELL_OFFSET,
                            shader_location: 2,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32,
                            offset: DEPTH_OFFSET,
                            shader_location: 4,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32,
                            offset: SHORE_OFFSET,
                            shader_location: 5,
                        },
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            // The same triangles as the seabed, so equal depth passes; the small bias keeps the
            // surface in front whatever the rounding of the two shaders.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: targets.depth_format,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState {
                    constant: -2,
                    slope_scale: -1.0,
                    clamp: 0.0,
                },
            }),
            multisample: crate::frame::multisample(targets.samples),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: targets.view_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("water sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        // No ripples until the bump map arrives: a 1x1 grey one.
        let grey = TextureBlob {
            format: asset_format::TextureFormat::Rgba8,
            dim: asset_format::TexDim::D2,
            srgb: false,
            width: 1,
            height: 1,
            depth: 1,
            mips: vec![vec![128, 128, 128, 255]],
        };
        let flat = texture::upload(device, queue, &grey, "water flat bump").expect("1x1 RGBA8");
        let bind_group = group(device, &layout, &sampler, &flat);
        Self {
            pipeline,
            layout,
            sampler,
            bind_group,
        }
    }

    /// Uses `blob` as the bump map; on error (a format the device cannot sample) the surface stays
    /// flat.
    pub fn set_bump(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        blob: &TextureBlob,
    ) -> Result<(), String> {
        let t = texture::upload(device, queue, blob, "water bump")?;
        self.bind_group = group(device, &self.layout, &self.sampler, &t);
        Ok(())
    }
}

fn group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    texture: &wgpu::Texture,
) -> wgpu::BindGroup {
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("water"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}
