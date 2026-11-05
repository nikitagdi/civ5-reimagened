//! The classic fog of war (world-overlays WOV1, `FOWLevel` 0, `FowClassic`): a hex over every cell
//! the player cannot see. The fluid fog draws nothing at this level (`FluidFOWSystem::Render`
//! returns), so this is the fog.
//!
//! An unrevealed cell gets an opaque hex (`VSFog` + `PSFog`), a revealed one that is not visible a
//! half transparent one (`VSFogTrans` + `PSFogTrans`, `shaders/fow/classic.wgsl`); the colour is the
//! vertex's. The mesh is made once per fog and drawn over every copy of the map.
// TODO(S1): the meshes, heights and colours of the original's classic fog are not in the code. The
// hexes here are flat, at one height each (above the mountains for the opaque ones; the
// translucent ones at the height `VSFogTrans` brings down to just over the plains), black. The
// terrain shaders still darken the hidden cells themselves (the B1 look, SIMP-013), so the
// translucent hexes darken them twice.

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3};

use super::clouds::TERRAIN_SCALE_Z;
use super::state::FogInput;
use crate::frame::{DEPTH_FORMAT, WORLD_FORMAT};
use crate::hexgrid::{CORNER_RADIUS, HEX_WIDTH, cell_center};
use crate::shaders::create_module;
use crate::terrain::Fog;

/// Height of the opaque hexes: just over the highest terrain (`65535 / 255` terrain units).
pub const OPAQUE_HEIGHT: f32 = 65535.0 / 255.0 * TERRAIN_SCALE_Z + 1.0;
/// Height of the translucent hexes in the mesh; `VSFogTrans` scales it by 0.66.
pub const TRANSLUCENT_HEIGHT: f32 = 20.0;
/// Colour of both kinds of hex.
pub const CLASSIC_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
/// Bytes between the transforms of the three map copies in the uniform buffer.
const TRANSFORM_STRIDE: u64 = 256;

/// A vertex of the classic fog: `dcl_position`, `dcl_color`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct ClassicVertex {
    pub position: [f32; 4],
    pub color: [f32; 4],
}

/// The hexes of a fog: opaque ones for the unrevealed cells, translucent ones for the hidden.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ClassicMesh {
    pub opaque: Vec<ClassicVertex>,
    pub opaque_indices: Vec<u32>,
    pub translucent: Vec<ClassicVertex>,
    pub translucent_indices: Vec<u32>,
}

fn push_hex(vertices: &mut Vec<ClassicVertex>, indices: &mut Vec<u32>, centre: Vec3, z: f32) {
    let base = vertices.len() as u32;
    let vertex = |p: Vec2| ClassicVertex {
        position: [p.x, p.y, z, 1.0],
        color: CLASSIC_COLOR,
    };
    vertices.push(vertex(centre.truncate()));
    for i in 0..6 {
        let a = f32::to_radians(30.0 + 60.0 * i as f32);
        vertices.push(vertex(
            centre.truncate() + Vec2::new(a.cos(), a.sin()) * CORNER_RADIUS,
        ));
    }
    for i in 0..6u32 {
        indices.extend_from_slice(&[base, base + 1 + i, base + 1 + (i + 1) % 6]);
    }
}

/// The mesh of a fog, in the original's basis.
pub fn classic_mesh(fog: &FogInput, ours_to_orig: &Mat4) -> ClassicMesh {
    let mut mesh = ClassicMesh::default();
    for y in 0..fog.height as i32 {
        for x in 0..fog.width as i32 {
            let c = cell_center(x, y);
            let p = ours_to_orig.transform_point3(Vec3::new(c.x, 0.0, c.y));
            match fog.cell(x, y) {
                Some(Fog::Unrevealed) => {
                    push_hex(&mut mesh.opaque, &mut mesh.opaque_indices, p, OPAQUE_HEIGHT);
                }
                Some(Fog::Hidden) => push_hex(
                    &mut mesh.translucent,
                    &mut mesh.translucent_indices,
                    p,
                    TRANSLUCENT_HEIGHT,
                ),
                _ => {}
            }
        }
    }
    mesh
}

struct Buffers {
    vertices: Buf,
    indices: Buf,
    count: u32,
}

fn upload(device: &wgpu::Device, vertices: &[ClassicVertex], indices: &[u32]) -> Option<Buffers> {
    if indices.is_empty() {
        return None;
    }
    Some(Buffers {
        vertices: device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("fow classic vertices"),
                contents: bytemuck::cast_slice(vertices),
                usage: wgpu::BufferUsages::VERTEX,
            },
            Cluster::Other,
        ),
        indices: device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("fow classic indices"),
                contents: bytemuck::cast_slice(indices),
                usage: wgpu::BufferUsages::INDEX,
            },
            Cluster::Other,
        ),
        count: indices.len() as u32,
    })
}

/// What the last frame drew.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClassicCounters {
    pub opaque_hexes: u32,
    pub translucent_hexes: u32,
}

/// The pipelines and meshes of the classic fog.
pub struct FowClassicGpu {
    opaque: wgpu::RenderPipeline,
    translucent: wgpu::RenderPipeline,
    uniform: Buf,
    group: wgpu::BindGroup,
    opaque_buffers: Option<Buffers>,
    translucent_buffers: Option<Buffers>,
    mesh: ClassicMesh,
    counters: ClassicCounters,
}

impl FowClassicGpu {
    pub fn new(device: &wgpu::Device, samples: u32) -> Self {
        let module = create_module(
            device,
            "fow classic",
            crate::wgsl_module!("fow/classic.wgsl"),
        );
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fow classic"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(64),
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fow classic"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];
        let pipeline = |label: &str,
                        vs: &str,
                        fs: &str,
                        blend: Option<wgpu::BlendState>,
                        depth_write: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<ClassicVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(depth_write),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: crate::frame::multisample(samples),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: WORLD_FORMAT,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let opaque = pipeline("fow classic opaque", "vsfog", "psfog", None, true);
        let translucent = pipeline(
            "fow classic translucent",
            "vsfogtrans",
            "psfogtrans",
            Some(wgpu::BlendState::ALPHA_BLENDING),
            false,
        );
        let uniform = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("fow classic transforms"),
                size: TRANSFORM_STRIDE * 3,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fow classic"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniform,
                    offset: 0,
                    size: wgpu::BufferSize::new(64),
                }),
            }],
        });
        Self {
            opaque,
            translucent,
            uniform,
            group,
            opaque_buffers: None,
            translucent_buffers: None,
            mesh: ClassicMesh::default(),
            counters: ClassicCounters::default(),
        }
    }

    pub fn counters(&self) -> ClassicCounters {
        self.counters
    }

    /// Makes the meshes of a fog (nothing is drawn until `set_fog`).
    pub fn set_fog(&mut self, device: &wgpu::Device, fog: Option<(&FogInput, &Mat4)>) {
        self.mesh = match fog {
            Some((fog, to_orig)) => classic_mesh(fog, to_orig),
            None => ClassicMesh::default(),
        };
        self.opaque_buffers = upload(device, &self.mesh.opaque, &self.mesh.opaque_indices);
        self.translucent_buffers = upload(
            device,
            &self.mesh.translucent,
            &self.mesh.translucent_indices,
        );
        self.counters = ClassicCounters {
            opaque_hexes: (self.mesh.opaque.len() / 7) as u32,
            translucent_hexes: (self.mesh.translucent.len() / 7) as u32,
        };
    }

    /// Writes the transforms of the three copies of the map (the shifts 0, -W and +W in our x):
    /// `view_proj_orig` is the camera in the original's basis.
    pub fn prepare(&self, queue: &wgpu::Queue, view_proj_orig: &Mat4, world_width: f32) {
        for (i, dx) in [0.0, -world_width, world_width].into_iter().enumerate() {
            // Our x is the original's x.
            let g = *view_proj_orig * Mat4::from_translation(Vec3::new(dx, 0.0, 0.0));
            queue.write_buffer(
                &self.uniform,
                i as u64 * TRANSFORM_STRIDE,
                bytemuck::bytes_of(&super::mask::row_major(&g)),
            );
        }
    }

    /// Draws the hexes over the copy shifted by `shift` (0, or ± the world width); the sizes of
    /// the indexed draws are returned for the counters.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, shift: f32) -> Vec<u32> {
        let mut sizes = Vec::new();
        let at = if shift == 0.0 {
            0
        } else if shift < 0.0 {
            1
        } else {
            2
        };
        let offset = [(at * TRANSFORM_STRIDE) as u32];
        for (pipeline, buffers) in [
            (&self.opaque, &self.opaque_buffers),
            (&self.translucent, &self.translucent_buffers),
        ] {
            let Some(b) = buffers else { continue };
            pass.set_pipeline_c(pipeline);
            pass.set_bind_group_c(0, &self.group, &offset);
            pass.set_vertex_buffer(0, b.vertices.slice(..));
            pass.set_index_buffer(b.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed_c(0..b.count, 0, 0..1);
            sizes.push(b.count);
        }
        sizes
    }
}

/// The world width of a fog, in units: the period of a wrapping map.
pub fn world_width(fog: &FogInput) -> f32 {
    HEX_WIDTH * fog.width as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fog(f: impl Fn(u32, u32) -> Fog) -> FogInput {
        FogInput::new(6, 4, false, (0..24).map(|i| f(i % 6, i / 6)).collect()).unwrap()
    }

    #[test]
    fn a_hex_is_a_fan_of_seven_vertices_and_six_triangles() {
        let f = fog(|x, y| {
            if (x, y) == (2, 1) {
                Fog::Unrevealed
            } else {
                Fog::Visible
            }
        });
        let m = classic_mesh(&f, &crate::frame::basis_rotation());
        assert_eq!(m.opaque.len(), 7);
        assert_eq!(m.opaque_indices.len(), 18);
        assert!(m.translucent.is_empty() && m.translucent_indices.is_empty());
        assert!(m.opaque_indices.iter().all(|&i| i < 7));
        // The middle is the cell's centre, in the original's basis (y north), at the opaque height.
        let c = cell_center(2, 1);
        assert_eq!(m.opaque[0].position, [c.x, -c.y, OPAQUE_HEIGHT, 1.0]);
        assert!(m.opaque.iter().all(|v| v.position[2] == OPAQUE_HEIGHT));
    }

    #[test]
    fn unrevealed_cells_are_opaque_and_hidden_ones_translucent_and_visible_ones_clear() {
        let f = fog(|x, _| match x % 3 {
            0 => Fog::Unrevealed,
            1 => Fog::Hidden,
            _ => Fog::Visible,
        });
        let m = classic_mesh(&f, &crate::frame::basis_rotation());
        assert_eq!(m.opaque.len(), 2 * 4 * 7);
        assert_eq!(m.translucent.len(), 2 * 4 * 7);
        assert!(
            m.translucent
                .iter()
                .all(|v| v.position[2] == TRANSLUCENT_HEIGHT)
        );
    }

    #[test]
    fn the_opaque_hexes_are_above_the_mountains() {
        let highest = std::hint::black_box(65535.0f32) / 255.0 * TERRAIN_SCALE_Z;
        assert!(OPAQUE_HEIGHT > highest);
        // `VSFogTrans` brings the translucent ones to 0.66 of their height: over the plains (12).
        let lowered = TRANSLUCENT_HEIGHT * 0.66;
        assert!(lowered > 12.0 && lowered < 16.0, "{lowered}");
    }

    #[test]
    fn a_hex_fills_the_cell() {
        let f = fog(|_, _| Fog::Unrevealed);
        let m = classic_mesh(&f, &crate::frame::basis_rotation());
        // The corners of a hex are at the corner radius from its centre.
        let mid = m.opaque[0].position;
        for v in &m.opaque[1..7] {
            let d = ((v.position[0] - mid[0]).powi(2) + (v.position[1] - mid[1]).powi(2)).sqrt();
            assert!((d - CORNER_RADIUS).abs() < 1e-3);
        }
    }
}
