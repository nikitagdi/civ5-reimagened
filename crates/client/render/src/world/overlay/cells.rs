//! The cell pass (world-overlays WOV2, `TerrainSystem::RenderOverlay`, `TerrainShader::ScreenSpaceOverlayPS`).
//!
//! After the map of the overlay (`map.rs`, `grid.rs`) the original draws the cells of the ground with
//! their relief, once per visible copy of the map, into the screen-sized target `ScreenOverlay`:
//! every pixel reads the map at the place its vertex shader worked out from the world position, so the
//! flat map lies over hills and valleys and the final shader of the ground finds the lines where the
//! eye sees them (`TerrainOverlay`). The pass runs at `OverlayLevel` 2 with `Use Screen Space Overlay` 1.
//!
//! The geometry is the ground's: `world-terrain` gives it as `cell_geometry()` (WT-10): the grids,
//! the height tiles and the group of the vertex shader `TerrainShaderVS`. In production the pass draws
//! through them ([`CellsGpu::draw_terrain`]): the lattices of the visible cells with the vertex stage of
//! the terrain, and a pixel stage that reads the map at the place worked out from the point in the
//! world (`cells_ps.wgsl`). [`CellGeometry`], plain triangles in our world, is the other source: what a
//! test or a caller without the native terrain can hand over (`Overlay::set_cell_geometry`).

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use glam::Mat4;

use crate::frame::{DEPTH_FORMAT, ResourceId, WORLD_FORMAT};
use crate::world::terrain::CellGeometry as TerrainCells;

/// The depth of the cell pass, the size of the screen.
pub const OVERLAY_DEPTH: ResourceId = ResourceId("OverlayDepth");

/// One copy of the ground: its triangles in our world, and the shift of the copy in X.
#[derive(Debug, Clone, PartialEq)]
pub struct CellCopy {
    pub x_shift: f32,
    pub vertices: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

/// The cells the pass draws: a copy per visible copy of the map (one, or the map and its neighbours
/// on a wrapping one).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CellGeometry {
    pub copies: Vec<CellCopy>,
}

impl CellGeometry {
    pub fn is_empty(&self) -> bool {
        self.copies.iter().all(|c| c.indices.is_empty())
    }

    /// The ground as one flat sheet at height 0 over `[min, max]` (our `x` and `z`), cut into `n`
    /// by `n` quads, once per shift in `shifts`.
    pub fn flat(min: [f32; 2], max: [f32; 2], n: u32, shifts: &[f32]) -> Self {
        let mut vertices = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                let t = (i as f32 / n as f32, j as f32 / n as f32);
                vertices.push([
                    min[0] + (max[0] - min[0]) * t.0,
                    0.0,
                    min[1] + (max[1] - min[1]) * t.1,
                ]);
            }
        }
        let mut indices = Vec::new();
        for j in 0..n {
            for i in 0..n {
                let a = j * (n + 1) + i;
                let b = a + 1;
                let c = a + n + 1;
                let d = c + 1;
                indices.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
        Self {
            copies: shifts
                .iter()
                .map(|&x_shift| CellCopy {
                    x_shift,
                    vertices: vertices.clone(),
                    indices: indices.clone(),
                })
                .collect(),
        }
    }
}

crate::uniform_block! {
    /// The constants of the vertex stage: a point of our world to the screen and to the map of the
    /// overlay, and the shift of the copy.
    pub struct CellConsts {
        pub world_to_clip: [[f32; 4]; 4],
        pub world_to_map: [[f32; 4]; 4],
        pub shift: [f32; 4],
    }
}

/// Copies at most (the map and a neighbour on each side).
const MAX_COPIES: usize = 3;
/// Stride of a constant block (`min_uniform_buffer_offset_alignment`).
const SLOT: u64 = 256;

struct CopyBuffers {
    vertices: Buf,
    indices: Buf,
    index_count: u32,
    shift: f32,
}

/// The pipeline and buffers of the cell pass.
pub struct CellsGpu {
    pipeline: wgpu::RenderPipeline,
    consts: Buf,
    consts_group: wgpu::BindGroup,
    map_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// The group of the map of the overlay, and the generation of the target it was made for.
    map_group: Option<(u64, wgpu::BindGroup)>,
    copies: Vec<CopyBuffers>,
    /// The projection of the map for the cells of `world-terrain`, and their pipeline, made at the
    /// first geometry (it needs the layout of the terrain's group 2).
    map_proj: Buf,
    map_proj_layout: wgpu::BindGroupLayout,
    map_proj_group: wgpu::BindGroup,
    terrain_pipeline: Option<wgpu::RenderPipeline>,
}

impl CellsGpu {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = crate::shaders::create_module(
            device,
            "overlay cells",
            crate::wgsl_module!("overlay/screen_space_ps.wgsl", "overlay/screen_space.wgsl"),
        );
        let consts_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay cells consts"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<CellConsts>() as u64
                    ),
                },
                count: None,
            }],
        });
        let map_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay cells map"),
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
            label: Some("overlay cells"),
            bind_group_layouts: &[Some(&consts_layout), Some(&map_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay cells"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("cell_vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 12,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            // The nearest cell wins where the copies of the relief overlap on the screen.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("screenspaceoverlayps"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: WORLD_FORMAT,
                    // The pixel shader writes what it reads; the target was cleared.
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let consts = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("overlay cells consts"),
                size: SLOT * MAX_COPIES as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        let consts_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay cells consts"),
            layout: &consts_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &consts,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<CellConsts>() as u64),
                }),
            }],
        });
        // `s1` of the original: clamped to the edge.
        // re: cache/golden/facts.md (sampler table): `ScreenSpaceOverlayPS` s1 U, V = ClampEdge, linear
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("overlay cells"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let map_proj_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay terrain cells map projection"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(64),
                },
                count: None,
            }],
        });
        let map_proj = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("overlay terrain cells map projection"),
                size: 64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        let map_proj_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay terrain cells map projection"),
            layout: &map_proj_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: map_proj.as_entire_binding(),
            }],
        });
        Self {
            pipeline,
            consts,
            consts_group,
            map_layout,
            sampler,
            map_group: None,
            copies: Vec::new(),
            map_proj,
            map_proj_layout,
            map_proj_group,
            terrain_pipeline: None,
        }
    }

    /// The pipeline of the cells of `world-terrain`: their vertex stage (`TerrainShaderVS`, group 2 of
    /// its layout, the lattice and the origin of the cell as the two vertex buffers) and the pixel
    /// stage of the overlay.
    fn make_terrain_pipeline(
        &self,
        device: &wgpu::Device,
        cells: &TerrainCells<'_>,
    ) -> wgpu::RenderPipeline {
        let module = crate::shaders::catalog()
            .iter()
            .find(|m| m.name == "overlay_cells")
            .expect("the overlay_cells module is in the catalog");
        let shader =
            crate::shaders::create_module(device, "overlay terrain cells", &module.source());
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay terrain cells"),
            bind_group_layouts: &[
                Some(&self.map_proj_layout),
                Some(&self.map_layout),
                Some(cells.vs_layout()),
            ],
            immediate_size: 0,
        });
        let (lattice, origin) = TerrainCells::vertex_attributes();
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay terrain cells"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("terrain_shader_vs"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: TerrainCells::VERTEX_STRIDE,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &lattice,
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 16,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &origin,
                    }),
                ],
            },
            // A heightfield seen from above: no face is hidden, and the nearest wins on the depth.
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("screenspaceoverlay_cells_ps"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: WORLD_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    }

    /// The projection of the map (`MapView::projection`) for the cells of `world-terrain`.
    pub fn set_map_projection(&self, queue: &wgpu::Queue, projection: &Mat4) {
        queue.write_buffer(
            &self.map_proj,
            0,
            bytemuck::bytes_of(&projection.to_cols_array_2d()),
        );
    }

    /// Draws the cells of `world-terrain` of every copy; returns how many cells it drew per copy.
    /// `bind_map` must have been called.
    pub fn draw_terrain(
        &mut self,
        device: &wgpu::Device,
        pass: &mut wgpu::RenderPass<'_>,
        cells: &TerrainCells<'_>,
    ) -> Vec<u32> {
        if self.map_group.is_none() {
            return Vec::new();
        }
        if self.terrain_pipeline.is_none() {
            self.terrain_pipeline = Some(self.make_terrain_pipeline(device, cells));
        }
        let (Some(pipeline), Some((_, map))) = (&self.terrain_pipeline, &self.map_group) else {
            return Vec::new();
        };
        pass.set_pipeline_c(pipeline);
        pass.set_bind_group_c(0, &self.map_proj_group, &[]);
        pass.set_bind_group_c(1, map, &[]);
        cells
            .copies()
            .iter()
            .map(|&shift| cells.draw(pass, shift, 2) as u32)
            .collect()
    }

    /// The geometry of the cells; an empty one stops the pass.
    pub fn set_geometry(&mut self, device: &wgpu::Device, geometry: &CellGeometry) {
        self.copies = geometry
            .copies
            .iter()
            .take(MAX_COPIES)
            .filter(|c| !c.indices.is_empty())
            .map(|c| CopyBuffers {
                vertices: device.buf_init(
                    &wgpu::util::BufferInitDescriptor {
                        label: Some("overlay cells vertices"),
                        contents: bytemuck::cast_slice(&c.vertices),
                        usage: wgpu::BufferUsages::VERTEX,
                    },
                    Cluster::Other,
                ),
                indices: device.buf_init(
                    &wgpu::util::BufferInitDescriptor {
                        label: Some("overlay cells indices"),
                        contents: bytemuck::cast_slice(&c.indices),
                        usage: wgpu::BufferUsages::INDEX,
                    },
                    Cluster::Other,
                ),
                index_count: c.indices.len() as u32,
                shift: c.x_shift,
            })
            .collect();
    }

    pub fn has_geometry(&self) -> bool {
        !self.copies.is_empty()
    }

    /// The constants of a frame: `world_to_clip` is the camera's `view * projection`, `world_to_map`
    /// the map's (`MapView::world_to_map`).
    pub fn set_frame(&self, queue: &wgpu::Queue, world_to_clip: &Mat4, world_to_map: &Mat4) {
        let mut bytes = vec![0u8; SLOT as usize * self.copies.len()];
        for (k, c) in self.copies.iter().enumerate() {
            let block = CellConsts {
                world_to_clip: world_to_clip.to_cols_array_2d(),
                world_to_map: world_to_map.to_cols_array_2d(),
                shift: [c.shift, 0.0, 0.0, 0.0],
            };
            let at = k * SLOT as usize;
            bytes[at..at + std::mem::size_of::<CellConsts>()]
                .copy_from_slice(bytemuck::bytes_of(&block));
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.consts, 0, &bytes);
        }
    }

    /// The group that reads the map of the overlay, made again when the target is.
    pub fn bind_map(&mut self, device: &wgpu::Device, map: &crate::frame::Target) {
        if self
            .map_group
            .as_ref()
            .is_some_and(|(generation, _)| *generation == map.generation)
        {
            return;
        }
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay cells map"),
            layout: &self.map_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&map.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.map_group = Some((map.generation, group));
    }

    /// Draws every copy; returns the index counts of the draws. `bind_map` must have been called.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) -> Vec<u32> {
        let Some((_, map)) = &self.map_group else {
            return Vec::new();
        };
        pass.set_pipeline_c(&self.pipeline);
        pass.set_bind_group_c(1, map, &[]);
        let mut sizes = Vec::new();
        for (k, c) in self.copies.iter().enumerate() {
            pass.set_bind_group_c(0, &self.consts_group, &[(k as u64 * SLOT) as u32]);
            pass.set_vertex_buffer(0, c.vertices.slice(..));
            pass.set_index_buffer(c.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed_c(0..c.index_count, 0, 0..1);
            sizes.push(c.index_count);
        }
        sizes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_sheet_has_two_triangles_per_quad_and_one_copy_per_shift() {
        let g = CellGeometry::flat([0.0, 0.0], [100.0, 50.0], 4, &[0.0, -640.0]);
        assert_eq!(g.copies.len(), 2);
        let c = &g.copies[0];
        assert_eq!(c.vertices.len(), 25);
        assert_eq!(c.indices.len(), 4 * 4 * 6);
        assert!(c.indices.iter().all(|&i| (i as usize) < c.vertices.len()));
        assert_eq!(c.vertices[0], [0.0, 0.0, 0.0]);
        assert_eq!(c.vertices[24], [100.0, 0.0, 50.0]);
        assert_eq!(g.copies[1].x_shift, -640.0);
        assert!(!g.is_empty());
        assert!(CellGeometry::default().is_empty());
    }

    #[test]
    fn the_constant_block_is_two_matrices_and_a_shift() {
        assert_eq!(std::mem::size_of::<CellConsts>(), 144);
        assert!(std::mem::size_of::<CellConsts>() as u64 <= SLOT);
    }
}
