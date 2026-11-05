//! The `blend` pipeline: alpha-blended geometry over the map. Rivers,
//! borders and the hex grid use it now; decals and highlights of `entity-render` will too.
//!
//! WebGL2-compatible: uniform buffers and one non-filtered `R8Uint` texture, no compute, no
//! storage buffers, instancing not used. Fog is read per vertex cell from the fog-state texture
//! (SIMP-013).
//!
//! The CPU half ([`BlendMesh`]) has no GPU types and is tested on its own.

use bytemuck::{Pod, Zeroable};
use glam::Vec3;

/// `across` value for geometry without feathered edges (borders, grid).
pub const NO_FEATHER: f32 = -1.0;

/// Position (0), `across` (12), colour RGBA linear (16), cell index (32); stride 36. `across` is
/// the position over the width of a strip, 0..1; the shader fades the alpha towards both ends
/// unless it is [`NO_FEATHER`]. `cell` indexes the fog-state texture.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct BlendVertex {
    pub position: [f32; 3],
    pub across: f32,
    pub color: [f32; 4],
    pub cell: u32,
}

/// Which draw a quad belongs to; the layers are drawn in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Rivers,
    Borders,
    Grid,
}

/// CPU geometry for the blend pipeline: one vertex list, one index list per layer. Triangles are
/// not culled.
#[derive(Debug, Clone, Default)]
pub struct BlendMesh {
    pub vertices: Vec<BlendVertex>,
    pub rivers: Vec<u32>,
    pub borders: Vec<u32>,
    pub grid: Vec<u32>,
}

impl BlendMesh {
    /// A quad `[a, b, c, d]` going round its outline; `across` and `color` are per corner / shared.
    pub fn quad(
        &mut self,
        layer: Layer,
        corners: [Vec3; 4],
        across: [f32; 4],
        color: [f32; 4],
        cell: u32,
    ) {
        let base = self.vertices.len() as u32;
        for (p, a) in corners.iter().zip(across) {
            self.vertices.push(BlendVertex {
                position: p.to_array(),
                across: a,
                color,
                cell,
            });
        }
        let list = match layer {
            Layer::Rivers => &mut self.rivers,
            Layer::Borders => &mut self.borders,
            Layer::Grid => &mut self.grid,
        };
        list.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// Quads of one layer, as their four corners.
    pub fn quads(&self, layer: Layer) -> impl Iterator<Item = [Vec3; 4]> + '_ {
        let list = match layer {
            Layer::Rivers => &self.rivers,
            Layer::Borders => &self.borders,
            Layer::Grid => &self.grid,
        };
        list.chunks_exact(6).map(|q| {
            [q[0], q[1], q[2], q[5]].map(|i| Vec3::from(self.vertices[i as usize].position))
        })
    }

    pub fn is_empty(&self) -> bool {
        self.rivers.is_empty() && self.borders.is_empty() && self.grid.is_empty()
    }

    pub fn indices_valid(&self) -> bool {
        let n = self.vertices.len() as u32;
        [&self.rivers, &self.borders, &self.grid]
            .into_iter()
            .all(|l| l.len().is_multiple_of(3) && l.iter().all(|&i| i < n))
    }
}

/// sRGB bytes to linear light: the scene shaders work in linear light and apply gamma themselves.
pub fn srgb_to_linear(rgb: [u8; 3]) -> [f32; 3] {
    rgb.map(|c| (f32::from(c) / 255.0).powf(2.2))
}

/// Uniform data the blend shader needs besides the globals.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BlendTargets {
    pub view_format: wgpu::TextureFormat,
    pub depth_format: wgpu::TextureFormat,
    /// Samples of the main pass (1 or 4).
    pub samples: u32,
}

/// Pipeline and the layout of the fog-state texture (group 2). Cheap to clone: the overlay
/// subsystems of `world-overlays` hold clones.
#[derive(Clone)]
pub(crate) struct BlendPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub fog_layout: wgpu::BindGroupLayout,
}

impl BlendPipeline {
    /// Groups: 0 globals, 1 object (model matrix), 2 fog-state texture.
    pub fn new(
        device: &wgpu::Device,
        globals_layout: &wgpu::BindGroupLayout,
        object_layout: &wgpu::BindGroupLayout,
        targets: BlendTargets,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("blend.wgsl"));
        let fog_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fog layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Uint,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blend pipeline layout"),
            bind_group_layouts: &[Some(globals_layout), Some(object_layout), Some(&fog_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blend pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<BlendVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3, 1 => Float32, 2 => Float32x4, 3 => Uint32
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            // Depth-tested against the terrain, never written; a small bias towards the camera
            // keeps strips lying on a slope from z-fighting.
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
        Self {
            pipeline,
            fog_layout,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_layout() {
        assert_eq!(std::mem::size_of::<BlendVertex>(), 36);
    }

    #[test]
    fn quad_adds_two_triangles_to_its_layer() {
        let mut m = BlendMesh::default();
        let q = [Vec3::ZERO, Vec3::X, Vec3::new(1.0, 0.0, 1.0), Vec3::Z];
        m.quad(Layer::Borders, q, [0.0; 4], [1.0; 4], 3);
        m.quad(Layer::Grid, q, [NO_FEATHER; 4], [1.0; 4], 4);
        assert_eq!(m.vertices.len(), 8);
        assert_eq!(m.borders, vec![0, 1, 2, 0, 2, 3]);
        assert_eq!(m.grid, vec![4, 5, 6, 4, 6, 7]);
        assert!(m.rivers.is_empty());
        assert!(m.indices_valid());
        assert_eq!(m.quads(Layer::Borders).next(), Some(q));
    }
}
