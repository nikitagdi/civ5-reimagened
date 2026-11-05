//! `Arrows` subsystem (world-overlays 4.7, card R5.9): the arrow of a
//! ranged attack, `ArrowVisSystem` of the original.
//!
//! The arrow is a mesh made on the CPU every frame it grows (`ArrowVis::UpdateArrowBuffers`
//! 0x0884d1ce, RW-22 (g)) and drawn with the ported `ArrowShader` (`shaders/overlay/arrow.wgsl`):
//! a B-spline from the attacker to the target with four control points, a shaft of boxes whose
//! width grows from `min_width` to `max_width`, a collar and a head, and around all of it an
//! outline box with the winding turned over (an inverted hull: only its back faces are seen). The
//! numbers are the root attributes of `ArrowSettings.xml` (`C5WS.arrows`).
//!
//! The arrow grows from the attacker to the target in `anim_speed` seconds once it is shown
//! (`ArrowVis::Update` 0x08851158); a new list of arrows starts the growth again (event 165
//! `SpawnArrow`), an empty list hides it (event 166).
//!
//! The original draws the arrow twice, with the alpha states 2 and 8 of its state table
//! (`CreateDefaultAlphaStates` 0x08d14182, 9 blocks of 0x2c, RA6c): state 2 has no blend and
//! writes the alpha channel only (so the first draw leaves the depth of the arrow), state 8 blends
//! the colour `SrcAlpha` / `InvSrcAlpha` and the alpha `One` / `One`, over that depth: the
//! translucent arrow shows its front surface only. Both draws test `LessEqual`, write the depth and
//! cull counter-clockwise faces (z state 0).

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use asset_format::Arrows as ArrowSettings;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

use crate::frame::{
    DEPTH_FORMAT, DrawContext, InitContext, MainSlot, PassSlot, PrepareContext, RenderSystem,
    SystemId, WORLD_FORMAT, multisample,
};
use crate::shaders::create_module;

/// One arrow, as `Events.SpawnArrowEvent(from, to)` of the original gives it: the plot of the
/// attacker and the plot of the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrowSpec {
    pub from: (i32, i32),
    pub to: (i32, i32),
}

/// Counters of `?stats=1`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArrowCounters {
    /// `set_arrows` calls that changed something.
    pub updates: u64,
    /// Arrows shown.
    pub arrows: u32,
}

/// The vertex of the original (`ArrowVis::ArrowVertex`, 0x28 bytes): position, normal (0 on the
/// outline), colour `0xAABBGGRR`, flag (all ones on the body and the head, 0 on the outline) and
/// eight bytes never written.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct ArrowVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [u8; 4],
    pub flag: [u8; 4],
    pub unused: [u32; 2],
}

/// The vertices and indices of one arrow, in the original's world.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArrowMesh {
    pub vertices: Vec<ArrowVertex>,
    pub indices: Vec<u32>,
    /// The shaft reached the head: the arrow is whole.
    pub done: bool,
}

// re: ArrowVis::UpdateArrowBuffers 0x0884d1ce (RW-22 (g), rw-a6b-arrow 4)
/// Step of the parameter along the shaft.
const T_STEP: f32 = 0.04;
/// `normalize(T.xy).x` above which the frame banks.
const BANK_LIMIT: f32 = 0.867;
/// Bank angle per unit of `d`.
// The original's own 3.1415, not pi.
#[allow(clippy::approx_constant)]
const BANK_ANGLE: f32 = 0.175 * 3.1415;
/// The growth stops just below 1.
// re: ArrowVis::Update 0x08851158 (0x3f7ff972)
const PROGRESS_MAX: f32 = 0.9999;
/// Capacity of the buffers of the original (`CreateBuffers` 0x0884d104).
const MAX_VERTICES: usize = 4000;
const MAX_INDICES: usize = 3000;

/// The colour of an `ArrowSettings.xml` attribute (`"255,0,0"`) as the vertex bytes `R, G, B, A`
/// (the original packs `0xff << 24 | b << 16 | g << 8 | r`).
fn color_of(rgb: &[u8]) -> [u8; 4] {
    let at = |i: usize| rgb.get(i).copied().unwrap_or(0);
    [at(0), at(1), at(2), 255]
}

/// The quadratic B-spline of the arrow through its four control points: the value and the
/// gradient at `t` in `0 ..= 2`, with the ends on `S` and `E` (knots `0 0 0 1 2 2 2`).
// TODO(RW-10): the weights of `B_SPLINE` are `.bss` data zero in the dump. The textbook uniform
// weights of `water/coast/bspline.rs` end the curve at the middle of the last two points, at least
// half the arc height from `E`, so the shaft would never meet the stop test of
// `UpdateArrowBuffers` 0x0884d1ce (`2*sqrt(3)*w + outline_width >= |E - centre|`) and the arrow would
// never get a head. The clamped quadratic has the same parameter range (`0 ..= n - 2`) and reaches
// both ends, so it is the reading that agrees with the code.
fn spline(pts: &[Vec3; 4], t: f32) -> (Vec3, Vec3) {
    let t = t.clamp(0.0, 2.0);
    if t < 1.0 {
        let u = t;
        let value =
            pts[0] * (1.0 - u).powi(2) + pts[1] * (2.0 * u - 1.5 * u * u) + pts[2] * (0.5 * u * u);
        let grad = pts[0] * (-2.0 * (1.0 - u)) + pts[1] * (2.0 - 3.0 * u) + pts[2] * u;
        (value, grad)
    } else {
        let v = t - 1.0;
        let value = pts[1] * (0.5 * (1.0 - v).powi(2))
            + pts[2] * (0.5 + v - 1.5 * v * v)
            + pts[3] * (v * v);
        let grad = pts[1] * (v - 1.0) + pts[2] * (1.0 - 3.0 * v) + pts[3] * (2.0 * v);
        (value, grad)
    }
}

/// `v` turned by `angle` about the unit axis `axis` (Rodrigues).
fn rotate(v: Vec3, axis: Vec3, angle: f32) -> Vec3 {
    let (s, c) = angle.sin_cos();
    v * c + axis.cross(v) * s + axis * axis.dot(v) * (1.0 - c)
}

/// The frame of the spline at a point: the tangent, the side and the up, banked where the arrow
/// runs east or west (or north).
fn frame(grad: Vec3) -> (Vec3, Vec3, Vec3) {
    let t = grad.normalize_or(Vec3::X);
    let mut side = t.cross(Vec3::Z).normalize_or(Vec3::NEG_Y);
    let d = glam::Vec2::new(t.x, t.y).normalize_or_zero().x;
    if d.abs() > BANK_LIMIT {
        side = rotate(side, t, d * BANK_ANGLE);
    } else if t.y > 0.0 {
        side = rotate(side, t, -d * BANK_ANGLE);
    }
    let up = side.cross(t).normalize_or(Vec3::Z);
    (t, side, up)
}

struct Builder {
    vertices: Vec<ArrowVertex>,
    indices: Vec<u32>,
    body: [u8; 4],
    outline: [u8; 4],
}

const LIT: [u8; 4] = [255; 4];
const FLAT: [u8; 4] = [0; 4];

/// The parts of a head: base, tangent, side, up, inner and outer half-widths, half-height, length.
struct Head {
    base: Vec3,
    t: Vec3,
    side: Vec3,
    up: Vec3,
    inner: f32,
    outer: f32,
    hh: f32,
    length: f32,
}

impl Builder {
    fn style(&self, outline: bool) -> ([u8; 4], [u8; 4]) {
        if outline {
            (self.outline, FLAT)
        } else {
            (self.body, LIT)
        }
    }

    fn push(&mut self, p: Vec3, n: Vec3, outline: bool) -> u32 {
        let (color, flag) = self.style(outline);
        self.vertices.push(ArrowVertex {
            position: p.to_array(),
            normal: if outline { [0.0; 3] } else { n.to_array() },
            color,
            flag,
            unused: [0; 2],
        });
        self.vertices.len() as u32 - 1
    }

    /// A box ring of 8 vertices around `c`: TL TR | TR BR | BR BL | BL TL, with the normals up,
    /// side, -up, -side (zero on the outline).
    fn ring(&mut self, c: Vec3, side: Vec3, up: Vec3, hw: f32, hh: f32, outline: bool) -> u32 {
        let tl = c - side * hw + up * hh;
        let tr = c + side * hw + up * hh;
        let br = c + side * hw - up * hh;
        let bl = c - side * hw - up * hh;
        let first = self.vertices.len() as u32;
        for (a, b, n) in [(tl, tr, up), (tr, br, side), (br, bl, -up), (bl, tl, -side)] {
            self.push(a, n, outline);
            self.push(b, n, outline);
        }
        first
    }

    /// The four faces between the rings at `p` and `c` (24 indices); the outline with the winding
    /// turned over.
    fn join(&mut self, p: u32, c: u32, outline: bool) {
        for f in 0..4 {
            let (p, c) = (p + 2 * f, c + 2 * f);
            if outline {
                self.indices.extend([p, p + 1, c, p + 1, c + 1, c]);
            } else {
                self.indices.extend([p, c, p + 1, p + 1, c, c + 1]);
            }
        }
    }

    /// A flat face: a fan of `pts`, wound to face away from the solid (turned on the outline).
    fn face(&mut self, pts: &[Vec3], n: Vec3, outline: bool) {
        let first = self.vertices.len() as u32;
        for &p in pts {
            self.push(p, n, outline);
        }
        for k in 1..pts.len() as u32 - 1 {
            let (a, b, c) = (first, first + k, first + k + 1);
            if outline {
                self.indices.extend([a, b, c]);
            } else {
                self.indices.extend([a, c, b]);
            }
        }
    }

    /// The head: a flat-shaded prism whose cap has the corners `inner` (where the shaft meets
    /// it), `outer` (the barbs) and the tip; 26 vertices and 14 triangles.
    fn head(&mut self, h: &Head, outline: bool) {
        let tip = h.base + h.t * h.length;
        let il = h.base - h.side * h.inner;
        let ol = h.base - h.side * h.outer;
        let ir = h.base + h.side * h.inner;
        let or = h.base + h.side * h.outer;
        let lift = |p: Vec3, s: f32| p + h.up * (h.hh * s);
        let up = |p: Vec3| lift(p, 1.0);
        let dn = |p: Vec3| lift(p, -1.0);
        // The top and the bottom: five points each, three triangles.
        self.face(&[up(ol), up(il), up(ir), up(or), up(tip)], h.up, outline);
        self.face(&[dn(tip), dn(or), dn(ir), dn(il), dn(ol)], -h.up, outline);
        // The two slanted sides.
        let left_n = h.up.cross(tip - ol).normalize_or_zero();
        self.face(&[up(ol), up(tip), dn(tip), dn(ol)], left_n, outline);
        let right_n = (tip - or).cross(h.up).normalize_or_zero();
        self.face(&[up(or), dn(or), dn(tip), up(tip)], right_n, outline);
        // The back between the barbs and the shaft.
        self.face(&[up(ol), dn(ol), dn(il), up(il)], -h.t, outline);
        self.face(&[up(ir), dn(ir), dn(or), up(or)], -h.t, outline);
    }
}

/// The mesh of an arrow from `s` to `e` (the original's world: x east, y north, z up) grown to
/// `progress` (0..1).
// re: ArrowVis::UpdateArrowBuffers 0x0884d1ce (rw-a6b-arrow 4)
pub fn build_arrow(s: Vec3, e: Vec3, cfg: &ArrowSettings, progress: f32) -> ArrowMesh {
    let arc = Vec3::Z * cfg.arc_height;
    let pts = [s, e * 0.25 + s * 0.75 + arc, s * 0.25 + e * 0.75 + arc, e];
    let mut b = Builder {
        vertices: Vec::new(),
        indices: Vec::new(),
        body: color_of(&cfg.body_color),
        outline: color_of(&cfg.outline_color),
    };
    let progress = progress.clamp(0.0, PROGRESS_MAX);
    let width = |t: f32| cfg.min_width + (cfg.max_width - cfg.min_width) * t;
    let (ow, ow2, hh) = (cfg.outline_width, cfg.outline_width2, cfg.width2);
    let mut prev: Option<(u32, u32)> = None;
    let mut head = None;
    let mut t = 0.0f32;
    while t < progress + 0.039 {
        let tt = t.min(progress);
        let (c, grad) = spline(&pts, 2.0 * tt);
        let (tan, side, up) = frame(grad);
        let w = width(tt);
        // The shaft stops where the head reaches the target.
        if 2.0 * 3f32.sqrt() * w + ow >= (e - c).length() {
            head = Some((c, tan, side, up, w));
            break;
        }
        let body = b.ring(c, side, up, w, hh, false);
        let outline = b.ring(c, side, up, w + ow, hh + ow2, true);
        if let Some((pb, po)) = prev {
            b.join(pb, body, false);
            b.join(po, outline, true);
        }
        prev = Some((body, outline));
        t += T_STEP;
    }
    let done = head.is_some();
    if let Some((c, tan, side, up, w)) = head {
        // The collar: the body box at the head's start and a copy `outline_width` along T.
        let a = b.ring(c, side, up, w, hh, false);
        let z = b.ring(c + tan * ow, side, up, w, hh, false);
        b.join(a, z, false);
        if let Some((pb, po)) = prev {
            b.join(pb, a, false);
            let o = b.ring(c, side, up, w + ow, hh + ow2, true);
            b.join(po, o, true);
        }
        b.head(
            &Head {
                base: c + tan * ow,
                t: tan,
                side,
                up,
                inner: w,
                outer: 2.0 * w,
                hh,
                length: 2.0 * w * 3f32.sqrt(),
            },
            false,
        );
        let outer = 2.0 * w + ow;
        b.head(
            &Head {
                base: c,
                t: tan,
                side,
                up,
                inner: w + ow,
                outer,
                hh: hh + ow2,
                length: outer * 3f32.sqrt(),
            },
            true,
        );
    }
    if b.vertices.len() > MAX_VERTICES || b.indices.len() > MAX_INDICES {
        tracing::debug!(
            vertices = b.vertices.len(),
            indices = b.indices.len(),
            "arrow: more than the buffers of the original hold"
        );
    }
    ArrowMesh {
        vertices: b.vertices,
        indices: b.indices,
        done,
    }
}

/// The growth of a shown arrow over `dt` seconds: `progress += dt / anim_speed` up to 0.9999.
// re: ArrowVis::Update 0x08851158 (rate = 1 / anim_speed)
pub fn grow(progress: f32, dt: f32, anim_speed: f32) -> f32 {
    if anim_speed <= 0.0 {
        return PROGRESS_MAX;
    }
    (progress + dt / anim_speed).min(PROGRESS_MAX)
}

/// `ArrowShaderConstants`: +0 the view-projection, +0x40 `alpha`, +0x50 `ambient_light_level`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct ArrowConsts {
    view_proj: [f32; 16],
    alpha: [f32; 4],
    ambient: [f32; 4],
}

/// One slot of the constants per copy of the map (the middle, west, east).
const COPIES: usize = 3;
const CONSTS_STRIDE: u64 = 256;

/// The frame of the arrows: the camera in the original's basis, the copies of the map, the time.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrowFrame {
    /// `clip = view_proj_orig * p` for a point of the original's world.
    pub view_proj_orig: Mat4,
    /// The X shifts of the map copies that reach the screen.
    pub shifts: Vec<f32>,
    /// Seconds since the start.
    pub time: f32,
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    group: wgpu::BindGroup,
    consts: Buf,
    /// The draws with the alpha states 2 and 8.
    pipelines: [wgpu::RenderPipeline; 2],
    vertices: Option<Buf>,
    indices: Option<Buf>,
    index_count: u32,
}

impl Gpu {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, samples: u32) -> Self {
        let module = create_module(
            device,
            "overlay arrow",
            crate::wgsl_module!("common/sm3.wgsl", "overlay/arrow.wgsl"),
        );
        let size = wgpu::BufferSize::new(std::mem::size_of::<ArrowConsts>() as u64);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("arrow"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: size,
                },
                count: None,
            }],
        });
        let consts = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("arrow consts"),
                size: CONSTS_STRIDE * COPIES as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("arrow"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &consts,
                    offset: 0,
                    size,
                }),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("arrow"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![
            0 => Float32x3, 1 => Float32x3, 2 => Unorm8x4, 3 => Unorm8x4
        ];
        let pipeline = |label: &str, blend: Option<wgpu::BlendState>, writes| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_arrow"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<ArrowVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    })],
                },
                // z state 0: `LESSEQUAL`, z write, cull counter-clockwise (the outward faces of
                // the mesh are clockwise seen from outside).
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Cw,
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: multisample(samples),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("ps_arrow"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: WORLD_FORMAT,
                        blend,
                        write_mask: writes,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        // re: CreateDefaultAlphaStates 0x08d14182 (RA6c): state 2 blend off, write mask 8 (alpha);
        // state 8 colour SRCALPHA / INVSRCALPHA add, alpha ONE / ONE add.
        let pipelines = [
            pipeline("arrow alpha state 2", None, wgpu::ColorWrites::ALPHA),
            pipeline(
                "arrow alpha state 8",
                Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::SrcAlpha,
                        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                }),
                wgpu::ColorWrites::ALL,
            ),
        ];
        Self {
            device: device.clone(),
            queue: queue.clone(),
            group,
            consts,
            pipelines,
            vertices: None,
            indices: None,
            index_count: 0,
        }
    }

    fn upload(&mut self, mesh: &ArrowMesh) {
        self.index_count = mesh.indices.len() as u32;
        if mesh.indices.is_empty() {
            self.vertices = None;
            self.indices = None;
            return;
        }
        let buffer = |label, contents: &[u8], usage| {
            self.device.buf_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents,
                    usage,
                },
                Cluster::Other,
            )
        };
        self.vertices = Some(buffer(
            "arrow vertices",
            bytemuck::cast_slice(&mesh.vertices),
            wgpu::BufferUsages::VERTEX,
        ));
        self.indices = Some(buffer(
            "arrow indices",
            bytemuck::cast_slice(&mesh.indices),
            wgpu::BufferUsages::INDEX,
        ));
    }

    fn write_consts(&self, frame: &ArrowFrame, cfg: &ArrowSettings) {
        for &shift in &frame.shifts {
            let slot = crate::world::water::ocean::copy_slot(shift).min(COPIES - 1);
            let vp = frame.view_proj_orig * Mat4::from_translation(Vec3::X * shift);
            let c = ArrowConsts {
                // A `row_major float4x4`: register k is column k of the plain matrix.
                view_proj: vp.transpose().to_cols_array(),
                alpha: [cfg.alpha, 0.0, 0.0, 0.0],
                ambient: [cfg.ambient_light_level, 0.0, 0.0, 0.0],
            };
            self.queue.write_buffer(
                &self.consts,
                slot as u64 * CONSTS_STRIDE,
                bytemuck::bytes_of(&c),
            );
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, shift: f32) -> u32 {
        let (Some(v), Some(i)) = (&self.vertices, &self.indices) else {
            return 0;
        };
        let slot = crate::world::water::ocean::copy_slot(shift).min(COPIES - 1);
        for pipeline in &self.pipelines {
            pass.set_pipeline_c(pipeline);
            pass.set_bind_group_c(0, &self.group, &[(slot as u64 * CONSTS_STRIDE) as u32]);
            pass.set_vertex_buffer(0, v.slice(..));
            pass.set_index_buffer(i.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed_c(0..self.index_count, 0, 0..1);
        }
        self.pipelines.len() as u32
    }
}

/// The arrows of the ranged attack.
#[derive(Default)]
pub struct Arrows {
    arrows: Vec<ArrowSpec>,
    counters: ArrowCounters,
    settings: Option<ArrowSettings>,
    /// The ends of each arrow in the original's world, from the renderer.
    ends: Vec<(Vec3, Vec3)>,
    progress: f32,
    last_time: Option<f32>,
    frame: Option<ArrowFrame>,
    /// The mesh is to be made again (new ends, still growing).
    dirty: bool,
    gpu: Option<Gpu>,
    drawn: std::cell::Cell<u32>,
}

impl Arrows {
    /// The arrows to show; an empty list clears them. The same list again does nothing; a new one
    /// starts the growth again (`SpawnArrow`).
    pub fn set_arrows(&mut self, arrows: Vec<ArrowSpec>) {
        if self.arrows == arrows {
            return;
        }
        self.counters.updates += 1;
        self.counters.arrows = arrows.len() as u32;
        self.arrows = arrows;
        self.progress = 0.0;
        self.dirty = true;
    }

    pub fn arrows(&self) -> &[ArrowSpec] {
        &self.arrows
    }

    pub fn stats(&self) -> ArrowCounters {
        self.counters
    }

    /// `ArrowSettings.xml` from `tb:world-b2`.
    pub fn set_settings(&mut self, settings: ArrowSettings) {
        self.settings = Some(settings);
        self.dirty = true;
    }

    pub fn has_settings(&self) -> bool {
        self.settings.is_some()
    }

    /// Takes the device (once, after it exists).
    pub fn bind_device(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, samples: u32) {
        self.gpu = Some(Gpu::new(device, queue, samples));
        self.dirty = true;
    }

    /// The ends of the arrows: the start on the ground of the attacker's plot, the end
    /// `target_height` above the world's zero at the target's (`OnSpawnArrow` 0x08851fc2). `ground`
    /// gives a plot's centre in the original's world at the height of the ground; `world_width` is
    /// the period of a wrapping map (0 for none): the start is taken from the copy nearest the end,
    /// as the original takes the one on the screen.
    pub fn set_ends(&mut self, ground: &dyn Fn(i32, i32) -> Option<Vec3>, world_width: f32) {
        let target_height = self.settings.as_ref().map_or(20.0, |s| s.target_height);
        let ends: Vec<(Vec3, Vec3)> = self
            .arrows
            .iter()
            .filter_map(|a| {
                let mut s = ground(a.from.0, a.from.1)?;
                let mut e = ground(a.to.0, a.to.1)?;
                e.z = target_height;
                if world_width > 0.0 {
                    let k = ((e.x - s.x) / world_width).round();
                    s.x += k * world_width;
                }
                Some((s, e))
            })
            .collect();
        if ends != self.ends {
            self.ends = ends;
            self.dirty = true;
        }
    }

    /// The camera and the time of the frame; the arrow grows by the time since the last frame.
    pub fn set_frame(&mut self, frame: ArrowFrame) {
        let dt = self
            .last_time
            .map_or(0.0, |t| (frame.time - t).clamp(0.0, 0.25));
        self.last_time = Some(frame.time);
        if !self.arrows.is_empty() && self.progress < PROGRESS_MAX {
            let speed = self.settings.as_ref().map_or(0.5, |s| s.anim_speed);
            self.progress = grow(self.progress, dt, speed);
            self.dirty = true;
        }
        self.frame = Some(frame);
    }

    /// The mesh of this frame (every shown arrow).
    pub fn mesh(&self) -> ArrowMesh {
        let Some(cfg) = &self.settings else {
            return ArrowMesh::default();
        };
        let mut out = ArrowMesh {
            done: true,
            ..ArrowMesh::default()
        };
        for &(s, e) in &self.ends {
            let m = build_arrow(s, e, cfg, self.progress);
            let base = out.vertices.len() as u32;
            out.vertices.extend(m.vertices);
            out.indices.extend(m.indices.iter().map(|i| i + base));
            out.done &= m.done;
        }
        out
    }
}

impl RenderSystem for Arrows {
    fn id(&self) -> SystemId {
        SystemId::Arrows
    }

    fn passes(&self) -> &'static [PassSlot] {
        &[PassSlot::Main(MainSlot::Arrows)]
    }

    fn init(&mut self, _cx: &mut InitContext<'_>) {}

    fn prepare(&mut self, _cx: &mut PrepareContext<'_>) {
        self.drawn.set(0);
        if self.gpu.is_none() || self.settings.is_none() {
            return;
        }
        if self.dirty {
            let mesh = if self.arrows.is_empty() {
                ArrowMesh::default()
            } else {
                self.mesh()
            };
            if let Some(gpu) = &mut self.gpu {
                gpu.upload(&mesh);
            }
            self.dirty = false;
        }
        if let (Some(gpu), Some(frame), Some(cfg)) = (&self.gpu, &self.frame, &self.settings) {
            gpu.write_consts(frame, cfg);
        }
    }

    fn record(&self, slot: PassSlot, cx: &mut DrawContext<'_, '_>) {
        if slot != PassSlot::Main(MainSlot::Arrows) || self.arrows.is_empty() {
            return;
        }
        let Some(gpu) = &self.gpu else { return };
        let n = gpu.draw(cx.pass, cx.copy_shift);
        if n > 0 {
            cx.stats.record_draw(n);
            self.drawn.set(self.drawn.get() + n);
        }
    }

    fn counters(&self) -> Vec<(&'static str, u64)> {
        let c = self.counters;
        vec![
            ("arrows.updates", c.updates),
            ("arrows.shown", u64::from(c.arrows)),
            ("arrows.drawn", u64::from(self.drawn.get())),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn settings() -> ArrowSettings {
        // The values of the game's `ArrowSettings.xml` (rw-a6-answers RW-22 (g) 1).
        ArrowSettings {
            min_width: 3.0,
            max_width: 5.0,
            width2: 1.5,
            arc_height: 75.0,
            outline_width: 0.65,
            outline_width2: 0.65,
            body_color: vec![255, 0, 0],
            outline_color: vec![255, 255, 255],
            alpha: 0.5,
            target_height: 20.0,
            anim_speed: 0.5,
            ambient_light_level: 0.7,
            ..ArrowSettings::default()
        }
    }

    #[test]
    fn arrows_are_kept_and_counted() {
        let mut a = Arrows::default();
        let list = vec![ArrowSpec {
            from: (1, 1),
            to: (3, 2),
        }];
        a.set_arrows(list.clone());
        a.set_arrows(list);
        assert_eq!(
            a.stats(),
            ArrowCounters {
                updates: 1,
                arrows: 1
            }
        );
        a.set_arrows(Vec::new());
        assert_eq!(a.stats().arrows, 0);
        assert!(a.arrows().is_empty());
    }

    #[test]
    fn the_vertex_is_the_original_s_forty_bytes() {
        assert_eq!(std::mem::size_of::<ArrowVertex>(), 0x28);
    }

    /// The whole arrow: shaft rings of 16 vertices and 48 indices a step after the first, the
    /// collar (16, 24) and the two heads of 26 vertices and 14 triangles; the colours of the body
    /// and the outline.
    #[test]
    fn a_grown_arrow_has_the_shaft_the_collar_and_the_heads() {
        let s = Vec3::new(0.0, 0.0, 5.0);
        let e = Vec3::new(200.0, 0.0, 20.0);
        let m = build_arrow(s, e, &settings(), PROGRESS_MAX);
        assert!(m.done);
        assert_eq!(m.indices.len() % 3, 0);
        assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
        // Rings: n pairs of 16; then the collar 16, the last outline ring 8 and two heads of 26.
        let rings = (m.vertices.len() - 16 - 8 - 52) / 16;
        assert_eq!((m.vertices.len() - 16 - 8 - 52) % 16, 0);
        assert_eq!(
            m.indices.len(),
            (rings - 1) * 48 + 24 + 48 + 2 * 14 * 3,
            "{rings} rings"
        );
        let lit: Vec<_> = m.vertices.iter().filter(|v| v.flag == LIT).collect();
        let flat: Vec<_> = m.vertices.iter().filter(|v| v.flag == FLAT).collect();
        assert!(!lit.is_empty() && !flat.is_empty());
        assert!(lit.iter().all(|v| v.color == [255, 0, 0, 255]));
        assert!(
            flat.iter()
                .all(|v| v.normal == [0.0; 3] && v.color == [255; 4])
        );
        // The arc rises well above both ends.
        let top = m
            .vertices
            .iter()
            .map(|v| v.position[2])
            .fold(f32::MIN, f32::max);
        assert!(top > 40.0, "{top}");
        // The tip of the head is near the target (the head is straight along the last tangent of
        // a falling curve, so it lands a little short).
        let near = m
            .vertices
            .iter()
            .map(|v| (Vec3::from(v.position) - e).length())
            .fold(f32::MAX, f32::min);
        assert!(near < 6.0, "{near}");
    }

    #[test]
    fn the_arrow_grows_in_half_a_second() {
        let mut p = 0.0;
        for _ in 0..15 {
            p = grow(p, 1.0 / 60.0, 0.5);
        }
        assert!((p - 0.5).abs() < 1e-3);
        for _ in 0..15 {
            p = grow(p, 1.0 / 60.0, 0.5);
        }
        assert!((p - PROGRESS_MAX).abs() < 1e-6);
        let e = Vec3::new(200.0, 0.0, 20.0);
        let early = build_arrow(Vec3::ZERO, e, &settings(), 0.2);
        let whole = build_arrow(Vec3::ZERO, e, &settings(), 1.0);
        assert!(!early.done);
        assert!(early.vertices.len() < whole.vertices.len());
    }

    #[test]
    fn a_new_arrow_restarts_the_growth_and_the_start_takes_the_nearest_copy() {
        let mut a = Arrows::default();
        a.set_settings(settings());
        a.set_arrows(vec![ArrowSpec {
            from: (0, 0),
            to: (1, 0),
        }]);
        let frame = |time| ArrowFrame {
            view_proj_orig: Mat4::IDENTITY,
            shifts: vec![0.0],
            time,
        };
        a.set_frame(frame(0.0));
        a.set_frame(frame(0.2));
        assert!(a.progress > 0.3);
        a.set_arrows(vec![ArrowSpec {
            from: (2, 0),
            to: (1, 0),
        }]);
        assert_eq!(a.progress, 0.0);
        // A map 1000 wide: the attacker at x 990 is 20 from the target at x 10 across the seam.
        let ground = |x: i32, _y: i32| Some(Vec3::new(if x == 2 { 990.0 } else { 10.0 }, 0.0, 3.0));
        a.set_ends(&ground, 1000.0);
        assert_eq!(a.ends[0].0.x, -10.0);
        assert_eq!(a.ends[0].1.z, 20.0);
    }
}
