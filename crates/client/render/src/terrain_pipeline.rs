//! The `terrain` pipeline: opaque map surface, one texture array,
//! fog from the fog-state texture of the blend pipeline.
//!
//! WebGL2-compatible: uniform buffers, one 2D array texture, one non-filtered `R8Uint` texture,
//! no compute, no storage buffers, at most 4 bind groups (globals, object, fog, material).
//!
//! The material (group 3) is the texture array and the slot table. Without assets it is a 1x1
//! array and every slot a flat colour, so the pipeline is the same with and without textures.

use crate::perf::{Cluster, DeviceTrack, Tex};
use std::collections::HashMap;

use asset_format::{LogicalId, TextureBlob};
use bytemuck::{Pod, Zeroable};

use crate::blend::srgb_to_linear;
use crate::terrain::fog::FOG_UNREVEALED_RGB;
use crate::terrain::texture_array::{LayerPlan, SLOT_COUNT, TerrainGpuVertex};
use crate::texture;

/// Length of the slot array in `terrain.wgsl`.
const SHADER_SLOTS: usize = 64;
const _: () = assert!(SLOT_COUNT <= SHADER_SLOTS);
/// Anisotropic filtering of the ground, which is seen at 45 degrees.
const ANISOTROPY: u16 = 4;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct SlotUniform {
    color: [f32; 4],
    tex: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct MaterialUniform {
    fog_color: [f32; 4],
    slots: [SlotUniform; SHADER_SLOTS],
}

impl MaterialUniform {
    fn new(plan: &LayerPlan) -> Self {
        let [r, g, b] = srgb_to_linear(FOG_UNREVEALED_RGB);
        let mut slots = [SlotUniform::zeroed(); SHADER_SLOTS];
        for (dst, src) in slots.iter_mut().zip(&plan.slots) {
            let [cr, cg, cb] = src.color;
            *dst = SlotUniform {
                color: [cr, cg, cb, 1.0],
                tex: [src.layer.map_or(-1.0, |l| l as f32), src.scale, 0.0, 0.0],
            };
        }
        Self {
            fog_color: [r, g, b, 1.0],
            slots,
        }
    }
}

/// Texture array, slot table and the bind group over them.
pub(crate) struct TerrainMaterial {
    pub bind_group: wgpu::BindGroup,
    /// Array layers (0 for the flat-colour material).
    pub layers: usize,
}

pub(crate) struct TerrainTargets {
    pub view_format: wgpu::TextureFormat,
    pub depth_format: wgpu::TextureFormat,
    /// Samples of the main pass (1 or 4).
    pub samples: u32,
}

pub(crate) struct TerrainPipeline {
    pub pipeline: wgpu::RenderPipeline,
    material_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl TerrainPipeline {
    /// Groups: 0 globals, 1 object (model matrix), 2 fog-state texture, 3 material.
    pub fn new(
        device: &wgpu::Device,
        globals_layout: &wgpu::BindGroupLayout,
        object_layout: &wgpu::BindGroupLayout,
        fog_layout: &wgpu::BindGroupLayout,
        targets: TerrainTargets,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("terrain.wgsl"));
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain material layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain pipeline layout"),
            bind_group_layouts: &[
                Some(globals_layout),
                Some(object_layout),
                Some(fog_layout),
                Some(&material_layout),
            ],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<TerrainGpuVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3, 1 => Float32x3, 2 => Uint32, 3 => Uint32
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: targets.depth_format,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: crate::frame::multisample(targets.samples),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: targets.view_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terrain sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: ANISOTROPY,
            ..Default::default()
        });
        Self {
            pipeline,
            material_layout,
            sampler,
        }
    }

    /// Layout of group 3 (the slot table and the texture array); the ground of the milestone M1
    /// (`world::terrain::m1`) draws with the same material.
    pub fn material_layout(&self) -> &wgpu::BindGroupLayout {
        &self.material_layout
    }

    /// Flat colours for every slot.
    pub fn flat_material(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> TerrainMaterial {
        let white = TextureBlob {
            format: asset_format::TextureFormat::Rgba8,
            dim: asset_format::TexDim::D2,
            srgb: true,
            width: 1,
            height: 1,
            depth: 1,
            mips: vec![vec![255; 4]],
        };
        // A one-layer RGBA8 array always uploads.
        let texture = texture::upload_array(device, queue, &[&white], (1, 1), 1, "terrain flat")
            .expect("1x1 RGBA8 array");
        self.material(device, texture, &LayerPlan::fallback())
    }

    /// The array of `plan`'s layers (taken from `blobs`) and its slot table. If the device cannot
    /// make the array the material is flat colours and the error is returned too.
    pub fn textured_material(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        plan: &LayerPlan,
        blobs: &HashMap<LogicalId, TextureBlob>,
    ) -> (TerrainMaterial, Option<String>) {
        let layers: Option<Vec<&TextureBlob>> =
            plan.layers.iter().map(|id| blobs.get(id)).collect();
        let result = match (layers, plan.size) {
            (Some(l), Some(size)) if !l.is_empty() => {
                texture::upload_array(device, queue, &l, size, plan.mip_count, "terrain textures")
            }
            _ => Err("no textures".to_owned()),
        };
        match result {
            Ok(t) => {
                let mut m = self.material(device, t, plan);
                m.layers = plan.layers.len();
                (m, None)
            }
            Err(e) => (self.flat_material(device, queue), Some(e)),
        }
    }

    fn material(&self, device: &wgpu::Device, texture: Tex, plan: &LayerPlan) -> TerrainMaterial {
        let uniform = device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("terrain material"),
                contents: bytemuck::bytes_of(&MaterialUniform::new(plan)),
                usage: wgpu::BufferUsages::UNIFORM,
            },
            Cluster::Other,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain material"),
            layout: &self.material_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        TerrainMaterial {
            bind_group,
            layers: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::texture_array::{LayerType, STYLES};

    #[test]
    fn uniform_layout_matches_the_shader() {
        // fog colour 16 + 64 slots of 32 bytes.
        assert_eq!(std::mem::size_of::<MaterialUniform>(), 16 + 64 * 32);
        // Within the WebGL2 uniform block limit (16 KiB).
        assert!(std::mem::size_of::<MaterialUniform>() <= 16 * 1024);
    }

    #[test]
    fn flat_plan_marks_every_slot_without_layer() {
        let u = MaterialUniform::new(&LayerPlan::fallback());
        for s in &u.slots[..SLOT_COUNT] {
            assert!(s.tex[0] < 0.0);
        }
        // Fog colour is the unrevealed colour, linearised like the other colours.
        let want = srgb_to_linear(FOG_UNREVEALED_RGB);
        assert_eq!(&u.fog_color[..3], &want);
        let ocean = u.slots[LayerType::Ocean.index() * STYLES].color;
        assert!(ocean[2] > ocean[0]);
    }
}
