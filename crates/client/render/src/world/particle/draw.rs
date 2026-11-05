//! The draw of the Fork particles (answers-b update 10, RB-6): the CPU expands each particle into
//! a quad (`Submit<shape>` 0x08cb0c88..), the quads go into one vertex buffer in submit order
//! (emitters in list order, particles in live order, no sort), and a batch closes on any change
//! of texture or alpha state. Depth test LESSEQUAL, no depth write, no culling (z state 10); the
//! eight alpha states of `InitForkAlphaStates` 0x08cafa42, the alpha channel ONE/ZERO/ADD.

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use std::collections::HashMap;

use asset_format::{FxShader, LogicalId, fx_flag as flag};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

use super::sim::{Instance, Sim};
use crate::frame::{DEPTH_FORMAT, ViewBuffers, WORLD_FORMAT, basis_rotation, multisample};
use crate::renderer::FrameStats;
use crate::shaders::create_module;

/// Most quads in one frame (the system's particle pool).
pub const MAX_QUADS: usize = super::sim::MAX_PARTICLES;

/// Flag bit 4 of the property: the quad turns about the emitter's y only (the beams).
// SIMPLIFIED(SIMP-048): the facing of `SetFacing` per emitter was not read; by the shipped files
// 0x20 is the flat quads on the ground (`FX_Floor_*`), 0x10 the pillars of light.
pub const FACING_AXIAL: u32 = 0x10;
/// Flag bit 5: the quad lies in the emitter's x-z plane.
pub const FACING_FLAT: u32 = 0x20;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct Vertex {
    /// Our basis.
    pub position: [f32; 3],
    pub uv: [f32; 2],
    /// Particle colour (`COLOR0`).
    pub color0: [u8; 4],
    /// Tint of the property (`COLOR1`).
    pub color1: [u8; 4],
    /// Shader kind and its float (see `fork.wgsl`).
    pub mode: [f32; 2],
}

/// One draw: a run of quads with one texture, gradient and alpha state.
#[derive(Clone, Debug, PartialEq)]
pub struct Batch {
    pub alpha: u8,
    pub texture: LogicalId,
    pub gradient: Option<LogicalId>,
    /// First quad and the number of quads.
    pub first: u32,
    pub quads: u32,
}

/// The camera of the frame, our basis.
#[derive(Clone, Copy, Debug)]
pub struct CameraAxes {
    pub right: Vec3,
    pub up: Vec3,
    /// From the eye into the screen.
    pub forward: Vec3,
}

impl CameraAxes {
    /// From a view matrix (world to view, right-handed: the camera looks down -Z).
    pub fn from_view(view: Mat4) -> Self {
        let inv = view.inverse();
        Self {
            right: inv.x_axis.truncate().normalize_or_zero(),
            up: inv.y_axis.truncate().normalize_or_zero(),
            forward: -inv.z_axis.truncate().normalize_or_zero(),
        }
    }
}

fn unorm(c: [f32; 4]) -> [u8; 4] {
    c.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
}

/// Expands every visible particle into `out` and the batches; `has` says whether a texture is on
/// the GPU (a particle whose texture is not is not drawn).
pub fn build(
    sim: &Sim,
    cam: CameraAxes,
    has: &dyn Fn(&LogicalId) -> bool,
) -> (Vec<Vertex>, Vec<Batch>) {
    let to_ours = basis_rotation().inverse();
    let b = basis_rotation();
    // The camera in the simulation's basis.
    let cr = b.transform_vector3(cam.right);
    let cu = b.transform_vector3(cam.up);
    let cf = b.transform_vector3(cam.forward);
    let mut verts = Vec::new();
    let mut batches: Vec<Batch> = Vec::new();
    for inst in sim.instances.iter().filter(|i| i.visible) {
        build_instance(inst, (cr, cu, cf), &to_ours, has, &mut verts, &mut batches);
        if verts.len() / 4 >= MAX_QUADS {
            break;
        }
    }
    (verts, batches)
}

fn build_instance(
    inst: &Instance,
    (cr, cu, cf): (Vec3, Vec3, Vec3),
    to_ours: &Mat4,
    has: &dyn Fn(&LogicalId) -> bool,
    verts: &mut Vec<Vertex>,
    batches: &mut Vec<Batch>,
) {
    for em in &inst.emitters {
        let def = &inst.fx.emitters[em.index];
        let p = &em.props;
        let (mode, gradient) = match &def.shader {
            FxShader::FakeVolume { gradient: Some(g) } if has(g) => ([1.0, 0.0], Some(g.clone())),
            FxShader::Brightness(f) => ([2.0, *f], None),
            _ => ([0.0, 0.0], None),
        };
        let tint = unorm(p.tint);
        let m = em.matrix;
        let ex = m.transform_vector3(Vec3::X).normalize_or_zero();
        let ey = m.transform_vector3(Vec3::Y).normalize_or_zero();
        let ez = m.transform_vector3(Vec3::Z).normalize_or_zero();
        let angle0 = p.rotation[0].to_radians();
        for q in &em.particles {
            if verts.len() / 4 >= MAX_QUADS {
                return;
            }
            let Some(frame) = def.frames.get(q.frame as usize).or(def.frames.first()) else {
                continue;
            };
            let Some(tex) = frame.texture.as_ref().filter(|t| has(t)) else {
                continue;
            };
            // Axes of the quad (SetFacing; see FACING_*).
            let (mut right, mut up) = if p.flags & flag::VELOCITY_ALIGNED != 0 {
                let v = q.vel - cf * q.vel.dot(cf);
                match v.try_normalize() {
                    Some(u) => (u.cross(cf).normalize_or_zero(), u),
                    None => (cr, cu),
                }
            } else if p.flags & FACING_FLAT != 0 {
                (ex, ez)
            } else if p.flags & FACING_AXIAL != 0 {
                (ey.cross(cf).normalize_or(cr), ey)
            } else {
                (cr, cu)
            };
            let a = angle0 + q.rotation;
            if a != 0.0 {
                let (s, c) = a.sin_cos();
                (right, up) = (right * c + up * s, up * c - right * s);
            }
            let hx = q.size * 0.5 * p.size_scale[0];
            let hy = q.size * 0.5 * p.size_scale[1];
            let corners: [Vec3; 4] = if p.shape == 0 {
                // Diamond: +up, -right, +right, -up.
                [up * hy, -right * hx, right * hx, -up * hy]
            } else {
                // Rectangle (string and triangle drawn as it): normalize(+-right hx +- up hy)
                // times (hx + hy) / sqrt 2.
                let e = (hx + hy) * std::f32::consts::FRAC_1_SQRT_2;
                let d =
                    |sx: f32, sy: f32| (right * (sx * hx) + up * (sy * hy)).normalize_or_zero() * e;
                [d(-1.0, 1.0), d(-1.0, -1.0), d(1.0, 1.0), d(1.0, -1.0)]
            };
            let [u0, v0, u1, v1] = frame.uv;
            let uvs = [[u0, v0], [u0, v1], [u1, v0], [u1, v1]];
            let color0 = unorm(q.color);
            for (c, uv) in corners.iter().zip(uvs) {
                verts.push(Vertex {
                    position: to_ours.transform_point3(q.pos + *c).to_array(),
                    uv,
                    color0,
                    color1: tint,
                    mode,
                });
            }
            let quad = (verts.len() / 4 - 1) as u32;
            match batches.last_mut() {
                Some(bt)
                    if bt.alpha == p.alpha
                        && bt.texture == *tex
                        && bt.gradient == gradient
                        && bt.first + bt.quads == quad =>
                {
                    bt.quads += 1
                }
                _ => batches.push(Batch {
                    alpha: p.alpha,
                    texture: tex.clone(),
                    gradient: gradient.clone(),
                    first: quad,
                    quads: 1,
                }),
            }
        }
    }
}

/// The blend of an alpha state (D3D enums of `InitForkAlphaStates`).
pub fn blend_state(alpha: u8) -> wgpu::BlendState {
    use wgpu::{BlendComponent, BlendFactor as F, BlendOperation as O};
    let c = |src, dst, operation| BlendComponent {
        src_factor: src,
        dst_factor: dst,
        operation,
    };
    let color = match alpha {
        0 => c(F::SrcAlpha, F::OneMinusSrcAlpha, O::Add),
        1 => c(F::One, F::One, O::Add),
        // D3DBLENDOP_REVSUBTRACT: dst - src.
        2 => c(F::SrcAlpha, F::One, O::ReverseSubtract),
        3 => c(F::Zero, F::SrcAlpha, O::Add),
        4 => c(F::Zero, F::Src, O::Add),
        5 => c(F::One, F::OneMinusSrc, O::Add),
        6 => c(F::SrcAlpha, F::One, O::Add),
        _ => c(F::One, F::OneMinusSrcAlpha, O::Add),
    };
    wgpu::BlendState {
        color,
        alpha: c(F::One, F::Zero, O::Add),
    }
}

/// The GPU side: the pipelines of the eight alpha states, the textures, the buffers of a frame.
pub struct ParticleDraw {
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    tex_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pipelines: Vec<Option<wgpu::RenderPipeline>>,
    samples: u32,
    white: wgpu::TextureView,
    textures: HashMap<LogicalId, wgpu::TextureView>,
    groups: HashMap<(LogicalId, Option<LogicalId>), wgpu::BindGroup>,
    vertices: Option<Buf>,
    indices: Buf,
    batches: Vec<Batch>,
}

impl ParticleDraw {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, samples: u32) -> Self {
        let module = create_module(
            device,
            "fork",
            crate::wgsl_module!("common/frame.wgsl", "fork/fork.wgsl"),
        );
        let view_layout = device.create_bind_group_layout(&ViewBuffers::layout_desc());
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fork textures"),
            entries: &[
                tex_entry(0),
                tex_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fork"),
            bind_group_layouts: &[Some(&view_layout), Some(&tex_layout)],
            immediate_size: 0,
        });
        // re: rw-a6 RA6d item 3: one sampler for BaseSampler (slot 0, the batch texture) and
        // AuxSampler0 (slot 1, the material texture +0x38): wrap, linear, no sRGB decode.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("fork sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let white = one_texel(device, queue);
        // Quad i: 4i + (0, 1, 2, 2, 1, 3).
        let idx: Vec<u16> = (0..MAX_QUADS as u32)
            .flat_map(|q| [0, 1, 2, 2, 1, 3].map(|k| (q * 4 + k) as u16))
            .collect();
        let indices = buffer_init(
            device,
            "fork indices",
            bytemuck::cast_slice(&idx),
            wgpu::BufferUsages::INDEX,
        );
        Self {
            module,
            layout,
            tex_layout,
            sampler,
            pipelines: (0..8).map(|_| None).collect(),
            samples,
            white,
            textures: HashMap::new(),
            groups: HashMap::new(),
            vertices: None,
            indices,
            batches: Vec::new(),
        }
    }

    pub fn has_texture(&self, id: &LogicalId) -> bool {
        self.textures.contains_key(id)
    }

    pub fn add_texture(&mut self, id: LogicalId, view: wgpu::TextureView) {
        self.groups
            .retain(|(t, g), _| *t != id && g.as_ref() != Some(&id));
        self.textures.insert(id, view);
    }

    fn pipeline(&mut self, device: &wgpu::Device, alpha: u8) {
        let slot = &mut self.pipelines[usize::from(alpha.min(7))];
        if slot.is_some() {
            return;
        }
        let attributes = wgpu::vertex_attr_array![
            0 => Float32x3,
            1 => Float32x2,
            2 => Unorm8x4,
            3 => Unorm8x4,
            4 => Float32x2,
        ];
        *slot = Some(
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("fork"),
                layout: Some(&self.layout),
                vertex: wgpu::VertexState {
                    module: &self.module,
                    entry_point: Some("fork_vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
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
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: multisample(self.samples),
                fragment: Some(wgpu::FragmentState {
                    module: &self.module,
                    entry_point: Some("fork_ps"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: WORLD_FORMAT,
                        blend: Some(blend_state(alpha)),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            }),
        );
    }

    /// Uploads the quads of the frame and makes what the batches need.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        verts: &[Vertex],
        batches: Vec<Batch>,
    ) {
        self.batches = batches;
        if verts.is_empty() {
            return;
        }
        let bytes = std::mem::size_of_val(verts) as u64;
        if self.vertices.as_ref().is_none_or(|b| b.size() < bytes) {
            self.vertices = Some(device.buf(
                &wgpu::BufferDescriptor {
                    label: Some("fork vertices"),
                    size: bytes.next_power_of_two().max(4096),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                },
                Cluster::Other,
            ));
        }
        if let Some(b) = &self.vertices {
            queue.write_buffer(b, 0, bytemuck::cast_slice(verts));
        }
        let wanted: Vec<(u8, LogicalId, Option<LogicalId>)> = self
            .batches
            .iter()
            .map(|b| (b.alpha, b.texture.clone(), b.gradient.clone()))
            .collect();
        for (alpha, tex, grad) in wanted {
            self.pipeline(device, alpha);
            let key = (tex, grad);
            if self.groups.contains_key(&key) {
                continue;
            }
            let (Some(t), g) = (
                self.textures.get(&key.0),
                key.1.as_ref().and_then(|g| self.textures.get(g)),
            ) else {
                continue;
            };
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fork textures"),
                layout: &self.tex_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(t),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(g.unwrap_or(&self.white)),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            self.groups.insert(key, group);
        }
    }

    /// Draws the batches of the frame in order.
    pub fn record(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        view_group: &wgpu::BindGroup,
        stats: &mut FrameStats,
    ) {
        let Some(vb) = &self.vertices else {
            return;
        };
        if self.batches.is_empty() {
            return;
        }
        pass.set_bind_group_c(0, view_group, &[]);
        pass.set_vertex_buffer(0, vb.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint16);
        let mut bound: Option<u8> = None;
        for b in &self.batches {
            let (Some(pipe), Some(group)) = (
                self.pipelines[usize::from(b.alpha.min(7))].as_ref(),
                self.groups.get(&(b.texture.clone(), b.gradient.clone())),
            ) else {
                continue;
            };
            if bound != Some(b.alpha) {
                pass.set_pipeline_c(pipe);
                bound = Some(b.alpha);
            }
            pass.set_bind_group_c(1, group, &[]);
            let first = b.first * 6;
            let count = b.quads * 6;
            pass.draw_indexed_c(first..first + count, 0, 0..1);
            stats.record_draw(count);
        }
    }
}

fn buffer_init(device: &wgpu::Device, label: &str, bytes: &[u8], usage: wgpu::BufferUsages) -> Buf {
    device.buf_init(
        &wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytes,
            usage,
        },
        Cluster::Other,
    )
}

fn one_texel(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
    let tex = device.tex(
        &wgpu::TextureDescriptor {
            label: Some("fork white"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        Cluster::Other,
    );
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &[255; 4],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    tex.create_view(&Default::default())
}

#[cfg(test)]
mod tests {
    use super::super::sim::tests::{effect, props};
    use super::*;

    fn cam() -> CameraAxes {
        CameraAxes {
            right: Vec3::X,
            up: Vec3::Y,
            forward: -Vec3::Z,
        }
    }

    #[test]
    fn quads_batch_by_texture_and_alpha_in_submit_order() {
        let mut sim = Sim::default();
        let mut p = props();
        p.rate = 0.0;
        p.burst = 3;
        sim.spawn(1, effect(p.clone()), Mat4::IDENTITY, 1.0, None);
        p.alpha = 6;
        sim.spawn(2, effect(p), Mat4::IDENTITY, 1.0, None);
        sim.step(0.01);
        let (v, b) = build(&sim, cam(), &|_| true);
        assert_eq!(v.len(), 24);
        assert_eq!(b.len(), 2);
        assert_eq!((b[0].alpha, b[0].first, b[0].quads), (1, 0, 3));
        assert_eq!((b[1].alpha, b[1].first, b[1].quads), (6, 3, 3));
        let (v, _) = build(&sim, cam(), &|_| false);
        assert!(v.is_empty(), "no texture on the GPU: nothing drawn");
    }

    #[test]
    fn a_camera_quad_faces_the_camera_at_its_size() {
        let mut sim = Sim::default();
        let mut p = props();
        p.rate = 0.0;
        p.burst = 1;
        p.size_min = [2.0; 3];
        p.size_max = [2.0; 3];
        let at = basis_rotation().transform_point3(Vec3::new(10.0, 5.0, -20.0));
        sim.spawn(1, effect(p), Mat4::from_translation(at), 1.0, None);
        sim.step(0.01);
        let (v, _) = build(&sim, cam(), &|_| true);
        // Rectangle corners at (+-1, +-1) around the particle, in the camera's plane (our z).
        for q in &v {
            assert!((q.position[2] + 20.0).abs() < 1e-4, "{:?}", q.position);
            assert!(((q.position[0] - 10.0).abs() - 1.0).abs() < 1e-4);
            assert!(((q.position[1] - 5.0).abs() - 1.0).abs() < 1e-4);
        }
        assert_eq!(v[0].color0, [255, 0, 0, 255]);
    }

    #[test]
    fn the_alpha_states_are_the_d3d_ones() {
        use wgpu::BlendFactor as F;
        assert_eq!(blend_state(1).color.dst_factor, F::One);
        assert_eq!(
            blend_state(2).color.operation,
            wgpu::BlendOperation::ReverseSubtract
        );
        assert_eq!(blend_state(7).color.src_factor, F::One);
        assert_eq!(blend_state(4).color.dst_factor, F::Src);
        assert_eq!(blend_state(0).alpha.dst_factor, F::Zero);
    }
}
