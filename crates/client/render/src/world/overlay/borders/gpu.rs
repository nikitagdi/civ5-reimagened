//! The borders into the map of the overlay (`CultureBorders::Render`; world-overlays WOV4).
//!
//! One draw per visible copy of the map with the whole mesh (every player's strips), through
//! `VSOverlayCountry` and `PSCountryBorder` of `country.wgsl`. The state is the original's: blend
//! ONE, INV_SRC_ALPHA, depth test `Less` with writes (the depth of every vertex is the same, so the
//! first triangle at a pixel wins and the fill does not tint the solid band), no culling.

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec4};

use super::{BorderVertex, OwnerMesh};
use crate::frame::{DEPTH_FORMAT, WORLD_FORMAT};

/// Copies of the map at most (the map and a neighbour on each side).
pub const MAX_COPIES: usize = 3;
/// Stride of a constant block (`min_uniform_buffer_offset_alignment`).
const SLOT: u64 = 256;

/// The constants of `VSOverlayCountry`: `WVP` and `Local`, rows as registers.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct CountryConsts {
    pub wvp: [[f32; 4]; 4],
    pub local: [[f32; 4]; 4],
}

/// `Local` for one copy of the map: a point `(X, Z)` of the ground plane, given as the vertex's `x`
/// and `y`, to the original's basis; `shift` moves the copy along X.
pub fn local_matrix(ours_to_orig: &Mat4, shift: f32) -> Mat4 {
    // (x, y, 0, 1) -> our world (x, 0, y, 1).
    let plane = Mat4::from_cols(
        Vec4::new(1.0, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(0.0, 1.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, 1.0),
    );
    *ours_to_orig * Mat4::from_translation(glam::Vec3::new(shift, 0.0, 0.0)) * plane
}

pub struct BordersGpu {
    pipeline: wgpu::RenderPipeline,
    consts: Buf,
    consts_group: wgpu::BindGroup,
    color_group: wgpu::BindGroup,
    vertices: Option<Buf>,
    indices: Option<Buf>,
    index_count: u32,
    copies: usize,
    /// The depth of the pass and the size it was made for.
    depth: Option<((u32, u32), wgpu::TextureView)>,
}

impl BordersGpu {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let module = crate::shaders::create_module(
            device,
            "overlay borders",
            crate::wgsl_module!("overlay/country.wgsl"),
        );
        let consts_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay borders consts"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<CountryConsts>() as u64
                    ),
                },
                count: None,
            }],
        });
        let color_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay borders color"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(16),
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay borders"),
            bind_group_layouts: &[Some(&consts_layout), Some(&color_layout)],
            immediate_size: 0,
        });
        let premultiplied = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay borders"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vsoverlaycountry"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<BorderVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Unorm8x4],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("pscountryborder"),
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
                label: Some("overlay borders consts"),
                size: SLOT * MAX_COPIES as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        let consts_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay borders consts"),
            layout: &consts_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &consts,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<CountryConsts>() as u64),
                }),
            }],
        });
        // `DrawColor`: white, the colour is the vertex's.
        let color = device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("overlay borders color"),
                contents: bytemuck::cast_slice(&[1.0f32; 4]),
                usage: wgpu::BufferUsages::UNIFORM,
            },
            Cluster::Other,
        );
        let color_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay borders color"),
            layout: &color_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: color.as_entire_binding(),
            }],
        });
        let _ = queue;
        Self {
            pipeline,
            consts,
            consts_group,
            color_group,
            vertices: None,
            indices: None,
            index_count: 0,
            copies: 0,
            depth: None,
        }
    }

    /// Uploads a new mesh (all players').
    pub fn set_mesh(&mut self, device: &wgpu::Device, mesh: &OwnerMesh) {
        self.index_count = mesh.indices.len() as u32;
        if mesh.indices.is_empty() {
            self.vertices = None;
            self.indices = None;
            return;
        }
        self.vertices = Some(device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("overlay borders vertices"),
                contents: bytemuck::cast_slice(&mesh.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            },
            Cluster::Other,
        ));
        self.indices = Some(device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("overlay borders indices"),
                contents: bytemuck::cast_slice(&mesh.indices),
                usage: wgpu::BufferUsages::INDEX,
            },
            Cluster::Other,
        ));
    }

    /// The constants of a frame: the projection of the map and a copy of the mesh per shift.
    pub fn set_frame(
        &mut self,
        queue: &wgpu::Queue,
        projection: &Mat4,
        ours_to_orig: &Mat4,
        shifts: &[f32],
    ) {
        self.copies = shifts.len().min(MAX_COPIES);
        for (i, &shift) in shifts.iter().take(MAX_COPIES).enumerate() {
            let consts = CountryConsts {
                wvp: crate::world::fow::mask::row_major(projection),
                local: crate::world::fow::mask::row_major(&local_matrix(ours_to_orig, shift)),
            };
            queue.write_buffer(&self.consts, i as u64 * SLOT, bytemuck::bytes_of(&consts));
        }
    }

    /// Indices drawn per copy by the last frame (0 with no mesh).
    pub fn index_count(&self) -> u32 {
        if self.vertices.is_some() {
            self.index_count
        } else {
            0
        }
    }

    pub fn copies(&self) -> usize {
        self.copies
    }

    /// The depth view of the last [`Self::ensure_depth`].
    pub fn depth(&self) -> Option<&wgpu::TextureView> {
        self.depth.as_ref().map(|(_, v)| v)
    }

    /// Makes the depth texture for a map of `size` if it is not there.
    pub fn ensure_depth(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.depth.as_ref().is_none_or(|(s, _)| *s != size) {
            let texture = device.tex(
                &wgpu::TextureDescriptor {
                    label: Some("overlay borders depth"),
                    size: wgpu::Extent3d {
                        width: size.0,
                        height: size.1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: DEPTH_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                },
                Cluster::Other,
            );
            self.depth = Some((
                size,
                texture.create_view(&wgpu::TextureViewDescriptor::default()),
            ));
        }
    }

    /// Draws the copies; returns the indices of every draw (for the counters).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) -> Vec<u32> {
        let (Some(vertices), Some(indices)) = (&self.vertices, &self.indices) else {
            return Vec::new();
        };
        pass.set_pipeline_c(&self.pipeline);
        pass.set_bind_group_c(1, &self.color_group, &[]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        let mut sizes = Vec::new();
        for i in 0..self.copies {
            pass.set_bind_group_c(0, &self.consts_group, &[(i as u64 * SLOT) as u32]);
            pass.draw_indexed_c(0..self.index_count, 0, 0..1);
            sizes.push(self.index_count);
        }
        sizes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn the_local_matrix_takes_a_plane_point_to_the_original_basis_with_the_shift() {
        let basis = crate::frame::basis_rotation();
        let p = glam::Vec3::new(10.0, 20.0, 0.0);
        let plain = local_matrix(&basis, 0.0).transform_point3(p);
        let ours = basis.transform_point3(Vec3::new(10.0, 0.0, 20.0));
        assert!((plain - ours).length() < 1e-4, "{plain:?} {ours:?}");
        let shifted = local_matrix(&basis, 64.0).transform_point3(p);
        let ours = basis.transform_point3(Vec3::new(74.0, 0.0, 20.0));
        assert!((shifted - ours).length() < 1e-4);
    }

    #[test]
    fn the_constants_are_two_matrices() {
        assert_eq!(std::mem::size_of::<CountryConsts>(), 128);
    }
}
