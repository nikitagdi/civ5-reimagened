//! The `decal` pipeline: the textured variant of `blend`.
//! Alpha-blended stickers over the map from one texture array, the layer in the vertex.
//!
//! WebGL2-compatible: uniform buffers, one 2D array texture, one `R8Uint` texture (the fog
//! state), no storage buffers, no instancing. The CPU half is `terrain::features::decal_mesh`.

use std::collections::HashMap;

use asset_format::{LogicalId, TextureBlob};

use crate::blend::BlendTargets;
use crate::terrain::features::DecalVertex;
use crate::terrain::texture_array::LayerPlan;
use crate::texture;

/// The blobs of the decal textures `ids` as one array can hold them. The game mixes formats (the
/// `*_overlay_*_2.dds` are BC2, the rest BC3) and an array has one; a device without BC samples
/// none of them. Then every BC1-BC3 blob is decoded to RGBA8 on the CPU (the decals are a few
/// small textures), instead of leaving the odd ones out. `None` when they can go up as they are.
pub(crate) fn uniform_decal_blobs(
    device: &wgpu::Device,
    ids: &[LogicalId],
    blobs: &HashMap<LogicalId, TextureBlob>,
) -> Option<HashMap<LogicalId, TextureBlob>> {
    let present: Vec<(&LogicalId, &TextureBlob)> = ids
        .iter()
        .filter_map(|id| Some((id, blobs.get(id)?)))
        .collect();
    let first = present.first()?.1;
    let same = present
        .iter()
        .all(|(_, b)| (b.format, b.srgb) == (first.format, first.srgb));
    if same
        && device
            .features()
            .contains(texture::required_features(first.format))
    {
        return None;
    }
    let mut out = HashMap::new();
    for (id, blob) in present {
        let decoded = texture::decode_blob(blob).unwrap_or_else(|| blob.clone());
        out.insert(id.clone(), decoded);
    }
    tracing::info!(
        textures = out.len(),
        "decal textures decoded to RGBA8 (mixed formats or no BC on the device)"
    );
    Some(out)
}

pub(crate) struct DecalPipeline {
    pub pipeline: wgpu::RenderPipeline,
    material_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl DecalPipeline {
    /// Groups: 0 globals, 1 object (model matrix of the map copy), 2 fog-state texture, 3 decal
    /// array and sampler.
    pub fn new(
        device: &wgpu::Device,
        globals_layout: &wgpu::BindGroupLayout,
        object_layout: &wgpu::BindGroupLayout,
        fog_layout: &wgpu::BindGroupLayout,
        targets: BlendTargets,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("decal.wgsl"));
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("decal material layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
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
            label: Some("decal pipeline layout"),
            bind_group_layouts: &[
                Some(globals_layout),
                Some(object_layout),
                Some(fog_layout),
                Some(&material_layout),
            ],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("decal pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<DecalVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3, 1 => Float32x2, 2 => Uint32, 3 => Float32, 4 => Uint32
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            // Like blend: tested against the terrain, never written, biased towards the camera.
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
            label: Some("decal sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipeline,
            material_layout,
            sampler,
        }
    }

    /// The decal array of `plan`'s layers (taken from `blobs`) and the bind group over it.
    pub fn material(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        plan: &LayerPlan,
        blobs: &HashMap<LogicalId, TextureBlob>,
    ) -> Result<wgpu::BindGroup, String> {
        let layers: Option<Vec<&TextureBlob>> =
            plan.layers.iter().map(|id| blobs.get(id)).collect();
        let (Some(layers), Some(size)) = (layers, plan.size) else {
            return Err("no decal textures".to_owned());
        };
        let texture = texture::upload_array(
            device,
            queue,
            &layers,
            size,
            plan.mip_count,
            "decal textures",
        )?;
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        Ok(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("decal material"),
            layout: &self.material_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        }))
    }
}
