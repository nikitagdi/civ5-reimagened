//! The fog mask (world-overlays WOV1, `FluidFOWSystem::RenderFOWMask`): the target `FowMask`, drawn
//! every frame from the state of the cells.
//!
//! The mask covers the rectangle of the ground the camera sees ([`MaskView`]), snapped to whole
//! tiles. Every hex of that rectangle is drawn as a small fan with a soft rim (`VSFowMask`, 24 to a
//! draw, `shaders/fow/mask.wgsl`), in three passes with the class of the hex:
//!
//! | pass | hexes | rim | pixel shader | blend |
//! |---|---|---|---|---|
//! | 0 | revealed, not visible | 1.33 | `Transparent` (w,0,0,1) | add |
//! | 1 | visible | 1.33 | `Visible` (w,0,w,1) | max |
//! | 2 | unrevealed, or off the map | 1.4 | `VisibleFluid` (0,w,0,1) | max |
//!
//! The channel contract that follows: **x** is how far a point is revealed (1 over every revealed
//! or visible hex, falling to 0 across the rim), **y** the coverage of the clouds (1 over the
//! unrevealed hexes and 0.4 or so of a hex beyond), **z** how far it is visible. A mask of white
//! (all channels 1) is what the original writes when the system is off.
// S1 (cache/golden/facts.md): FOW::Visible writes R and B, VisibleFluid writes G, both blend Max
// (applied above). Still open:
// TODO(S1): the order of the three passes and the pixel shader of each are read from
// `FluidFOWSystem.cpp` (subsets 2, 1, 0 of the two surfaces) and from the channels the pixel
// shaders write; which channel the terrain shaders read for what is not in the code. The capture
// of the mask settles it.

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3};

use super::state::FogInput;
use crate::frame::{EncoderContext, FOW_MASK, WORLD_FORMAT};
use crate::hexgrid::{CORNER_RADIUS, HEX_WIDTH, ROW_STEP, cell_center};
use crate::shaders::create_module;
use crate::shadow::visible_rect;
use crate::terrain::Fog;
use crate::world::overlay::map::MapView;

/// Hexes to a draw: the size of `WVP[24]`.
pub const BATCH: usize = 24;
/// Hexes of one pass at most (`kMaxFogTiles`).
pub const MAX_INSTANCES: usize = 0x800;
/// Outer radius of the rim in corner radii: revealed and visible hexes.
pub const REVEALED_RIM: f32 = 1.33;
/// Outer radius of the rim in corner radii: unrevealed hexes.
pub const UNREVEALED_RIM: f32 = 1.4;
/// A tile of the ground in world units: the rectangle is snapped to it.
pub const TILE: f32 = 64.0;
/// Bytes of one batch of matrices in the uniform buffer (24 `float4x4`).
pub const BATCH_BYTES: u64 = (BATCH * 64) as u64;
/// Vertices of one hex: the middle, the inner ring, the outer ring.
const HEX_VERTICES: usize = 13;
/// Indices of one hex: six triangles in the middle, twelve in the rim.
const HEX_INDICES: usize = 54;
/// Half-size of the square seen around the eye when no edge of the frustum reaches the ground.
const FALLBACK_HALF: f32 = 1024.0;

/// What the camera shows, in the units and the basis the subsystem works in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FowView {
    /// Our `view * projection` (`clip = view_proj * point`).
    pub view_proj: Mat4,
    pub eye: Vec3,
    /// Our basis to the original's.
    pub ours_to_orig: Mat4,
}

impl Default for FowView {
    fn default() -> Self {
        Self {
            view_proj: Mat4::IDENTITY,
            eye: Vec3::ZERO,
            ours_to_orig: crate::frame::basis_rotation(),
        }
    }
}

/// The rectangle of the ground the mask covers, in the original's basis (x east, y north).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaskView {
    pub min: Vec2,
    pub size: Vec2,
}

impl MaskView {
    /// The rectangle of the map of the overlay (`m_mOverlayProj`, [`MapView`]): the original reads
    /// the mask, the clouds' `FOWOverlay`, the overlay map and the cells' `mtxCompositeVP` through
    /// that one matrix (S1a ev 741, 819, 2194: the same rows, x -64..832, y -512..192, which is
    /// `MapView` of our camera to the unit).
    pub fn of(view: &FowView) -> Self {
        let (lo, hi) = visible_rect(&view.view_proj, view.eye, &view.ours_to_orig, FALLBACK_HALF);
        let map = MapView::of(lo.truncate(), hi.truncate());
        Self {
            min: map.min,
            size: map.size().max(Vec2::splat(TILE)),
        }
    }

    /// The smallest rectangle of whole tiles that holds `lo`..`hi`.
    pub fn snapped(lo: Vec2, hi: Vec2) -> Self {
        let min = (lo / TILE).floor() * TILE;
        let max = (hi / TILE).ceil() * TILE;
        Self {
            min,
            size: (max - min).max(Vec2::splat(TILE)),
        }
    }

    pub fn max(&self) -> Vec2 {
        self.min + self.size
    }

    /// The orthographic projection of the rectangle: original-basis point to clip space (x right,
    /// y up, the plane at depth 0.5). `FOWOverlay` of the cloud layers is the same matrix.
    pub fn projection(&self) -> Mat4 {
        let s = Vec2::new(2.0, 2.0) / self.size;
        Mat4::from_cols(
            glam::Vec4::new(s.x, 0.0, 0.0, 0.0),
            glam::Vec4::new(0.0, s.y, 0.0, 0.0),
            glam::Vec4::new(0.0, 0.0, 0.0, 0.0),
            glam::Vec4::new(-1.0 - s.x * self.min.x, -1.0 - s.y * self.min.y, 0.5, 1.0),
        )
    }

    /// Where a point of the ground is in the mask: (0, 0) top left, (1, 1) bottom right.
    pub fn uv(&self, p: Vec2) -> Vec2 {
        let t = (p - self.min) / self.size;
        Vec2::new(t.x, 1.0 - t.y)
    }

    /// The rectangle in our basis (`x` and `z`): smallest and largest corner.
    pub fn ours_rect(&self, ours_to_orig: &Mat4) -> (Vec2, Vec2) {
        let to_ours = ours_to_orig.inverse();
        let mut lo = Vec2::splat(f32::INFINITY);
        let mut hi = Vec2::splat(f32::NEG_INFINITY);
        let max = self.max();
        for c in [
            self.min,
            Vec2::new(max.x, self.min.y),
            Vec2::new(self.min.x, max.y),
            max,
        ] {
            let p = to_ours.transform_point3(c.extend(0.0));
            lo = lo.min(Vec2::new(p.x, p.z));
            hi = hi.max(Vec2::new(p.x, p.z));
        }
        (lo, hi)
    }
}

/// The matrix of a column-vector `G` as the original's `row_major float4x4` constant: the layout
/// of a WGSL `mat4x4` in which column `j` holds component `j` of every register.
pub fn row_major(g: &Mat4) -> [[f32; 4]; 4] {
    g.transpose().to_cols_array_2d()
}

/// The cells (virtual ones too, outside the map) whose centre lies in the rectangle (`lo`, `hi`,
/// our `x` and `z`) grown by `margin`.
pub fn cells_in_rect(lo: Vec2, hi: Vec2, margin: f32) -> Vec<(i32, i32)> {
    // Row y is at z = -ROW_STEP * y.
    let y0 = ((-(hi.y + margin)) / ROW_STEP).ceil() as i32;
    let y1 = ((-(lo.y - margin)) / ROW_STEP).floor() as i32;
    let mut cells = Vec::new();
    for y in y0..=y1 {
        let shift = 0.5 * (y & 1) as f32;
        let x0 = ((lo.x - margin) / HEX_WIDTH - shift).ceil() as i32;
        let x1 = ((hi.x + margin) / HEX_WIDTH - shift).floor() as i32;
        for x in x0..=x1 {
            cells.push((x, y));
        }
    }
    cells
}

/// The class of a hex for the mask. A hex off the map is unrevealed; on a wrapping map the east
/// and west copies of the map are the map.
pub fn class_of(fog: &FogInput, x: i32, y: i32) -> MaskClass {
    match fog
        .shape()
        .normalize(x, y)
        .and_then(|(nx, ny)| fog.cell(nx, ny))
    {
        Some(Fog::Visible) => MaskClass::Visible,
        Some(Fog::Hidden) => MaskClass::Revealed,
        Some(Fog::Unrevealed) | None => MaskClass::Unrevealed,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskClass {
    /// Revealed and not visible (`kMaskClassRevealed`).
    Revealed,
    Visible,
    /// Unrevealed or off the map.
    Unrevealed,
}

/// The centres of the hexes of each pass, in the original's basis.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct MaskInstances {
    pub revealed: Vec<Vec2>,
    pub visible: Vec<Vec2>,
    pub unrevealed: Vec<Vec2>,
}

impl MaskInstances {
    pub fn total(&self) -> usize {
        self.revealed.len() + self.visible.len() + self.unrevealed.len()
    }
}

/// The hexes the mask draws: those of the rectangle, by class.
pub fn collect_instances(fog: &FogInput, view: &MaskView, ours_to_orig: &Mat4) -> MaskInstances {
    let (lo, hi) = view.ours_rect(ours_to_orig);
    let mut out = MaskInstances::default();
    for (x, y) in cells_in_rect(lo, hi, CORNER_RADIUS * UNREVEALED_RIM) {
        let c = cell_center(x, y);
        let p = ours_to_orig
            .transform_point3(Vec3::new(c.x, 0.0, c.y))
            .truncate();
        let list = match class_of(fog, x, y) {
            MaskClass::Revealed => &mut out.revealed,
            MaskClass::Visible => &mut out.visible,
            MaskClass::Unrevealed => &mut out.unrevealed,
        };
        if list.len() < MAX_INSTANCES {
            list.push(p);
        }
    }
    out
}

/// A vertex of the mask fan: `dcl_position`, `dcl_normal` (x: the hex among the 24 of a draw),
/// `dcl_texcoord` (x: the weight).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct MaskVertex {
    pub position: [f32; 4],
    pub slot: [f32; 4],
    pub weight: [f32; 4],
}

/// The 24 fans of a draw. A fan has weight 1 over the hex (up to the corner radius) and falls to 0
/// at `rim` corner radii; fans of neighbouring hexes of one class overlap in the full part.
pub fn hex_fans(rim: f32) -> (Vec<MaskVertex>, Vec<u16>) {
    let mut vertices = Vec::with_capacity(HEX_VERTICES * BATCH);
    let mut indices = Vec::with_capacity(HEX_INDICES * BATCH);
    for slot in 0..BATCH {
        let base = vertices.len() as u16;
        let vertex = |p: Vec2, w: f32| MaskVertex {
            position: [p.x, p.y, 0.0, 1.0],
            slot: [slot as f32, 0.0, slot as f32, 0.0],
            weight: [w, 0.0, 0.0, 0.0],
        };
        vertices.push(vertex(Vec2::ZERO, 1.0));
        // Pointy top: corners at 30 + 60 i degrees, y north.
        let ring = |r: f32, i: usize| {
            let a = f32::to_radians(30.0 + 60.0 * i as f32);
            Vec2::new(a.cos(), a.sin()) * r
        };
        for i in 0..6 {
            vertices.push(vertex(ring(CORNER_RADIUS, i), 1.0));
        }
        for i in 0..6 {
            vertices.push(vertex(ring(CORNER_RADIUS * rim, i), 0.0));
        }
        for i in 0..6u16 {
            let j = (i + 1) % 6;
            let (a, b) = (base + 1 + i, base + 1 + j);
            let (c, d) = (base + 7 + i, base + 7 + j);
            indices.extend_from_slice(&[base, a, b, a, c, d, a, d, b]);
        }
    }
    (vertices, indices)
}

/// The matrices of a draw as the constant block `WVP[24]`: `projection * translation(centre)` of
/// each hex, in the layout of `row_major float4x4`; the unused ones are zero.
pub fn batch_block(projection: &Mat4, centres: &[Vec2]) -> Vec<u8> {
    let mut out = vec![0u8; BATCH_BYTES as usize];
    for (i, c) in centres.iter().take(BATCH).enumerate() {
        let g = *projection * Mat4::from_translation(c.extend(0.0));
        out[i * 64..(i + 1) * 64].copy_from_slice(bytemuck::bytes_of(&row_major(&g)));
    }
    out
}

/// Batches needed for `n` hexes.
pub fn batches_for(n: usize) -> usize {
    n.div_ceil(BATCH)
}

/// Batches of the largest mask: three passes of [`MAX_INSTANCES`].
const MAX_BATCHES: usize = 3 * (MAX_INSTANCES.div_ceil(BATCH));

struct Mesh {
    vertices: Buf,
    indices: Buf,
}

impl Mesh {
    fn new(device: &wgpu::Device, rim: f32) -> Self {
        let (v, i) = hex_fans(rim);
        Self {
            vertices: device.buf_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("fow mask vertices"),
                    contents: bytemuck::cast_slice(&v),
                    usage: wgpu::BufferUsages::VERTEX,
                },
                Cluster::Other,
            ),
            indices: device.buf_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("fow mask indices"),
                    contents: bytemuck::cast_slice(&i),
                    usage: wgpu::BufferUsages::INDEX,
                },
                Cluster::Other,
            ),
        }
    }
}

/// One pass of the mask on the GPU: which hexes, with which mesh, pipeline and batches.
#[derive(Clone, Copy)]
struct Draw {
    /// Index into the pipelines (0 revealed, 1 visible, 2 unrevealed).
    pass: usize,
    wide: bool,
    first_batch: u32,
    hexes: u32,
}

/// What the last `upload` made of the frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MaskCounters {
    pub revealed: u32,
    pub visible: u32,
    pub unrevealed: u32,
    pub draws: u32,
}

/// The pipelines and buffers of the mask pass.
pub struct FowMask {
    pipelines: [wgpu::RenderPipeline; 3],
    narrow: Mesh,
    wide: Mesh,
    uniform: Buf,
    group: wgpu::BindGroup,
    draws: Vec<Draw>,
    counters: MaskCounters,
}

impl FowMask {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = create_module(device, "fow mask", crate::wgsl_module!("fow/mask.wgsl"));
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fow mask"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(BATCH_BYTES),
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fow mask"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let add = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let max = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Max,
        };
        let attributes = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4];
        let pipeline = |label: &str, fragment: &str, blend: wgpu::BlendComponent| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("fowmask_vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<MaskVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
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
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: WORLD_FORMAT,
                        blend: Some(wgpu::BlendState {
                            color: blend,
                            alpha: blend,
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipelines = [
            pipeline("fow mask revealed", "transparent_ps", add),
            pipeline("fow mask visible", "visible_ps", max),
            pipeline("fow mask unrevealed", "visiblefluid_ps", max),
        ];
        let uniform = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("fow mask matrices"),
                size: BATCH_BYTES * MAX_BATCHES as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fow mask"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniform,
                    offset: 0,
                    size: wgpu::BufferSize::new(BATCH_BYTES),
                }),
            }],
        });
        Self {
            pipelines,
            narrow: Mesh::new(device, REVEALED_RIM),
            wide: Mesh::new(device, UNREVEALED_RIM),
            uniform,
            group,
            draws: Vec::new(),
            counters: MaskCounters::default(),
        }
    }

    pub fn counters(&self) -> MaskCounters {
        self.counters
    }

    /// Writes the matrices of the frame and plans the draws.
    pub fn upload(&mut self, queue: &wgpu::Queue, view: &MaskView, instances: &MaskInstances) {
        let projection = view.projection();
        let mut bytes: Vec<u8> = Vec::new();
        self.draws.clear();
        for (pass, wide, centres) in [
            (0, false, &instances.revealed),
            (1, false, &instances.visible),
            (2, true, &instances.unrevealed),
        ] {
            if centres.is_empty() {
                continue;
            }
            let first_batch = (bytes.len() as u64 / BATCH_BYTES) as u32;
            for chunk in centres.chunks(BATCH) {
                bytes.extend_from_slice(&batch_block(&projection, chunk));
            }
            self.draws.push(Draw {
                pass,
                wide,
                first_batch,
                hexes: centres.len() as u32,
            });
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.uniform, 0, &bytes);
        }
        self.counters = MaskCounters {
            revealed: instances.revealed.len() as u32,
            visible: instances.visible.len() as u32,
            unrevealed: instances.unrevealed.len() as u32,
            draws: self
                .draws
                .iter()
                .map(|d| batches_for(d.hexes as usize) as u32)
                .sum(),
        };
    }

    /// Draws the mask into `FowMask`: cleared to black, then the three passes. `white` clears to
    /// white and draws nothing (what the original writes when the fluid fog is off).
    pub fn record(&self, cx: &mut EncoderContext<'_>, white: bool) {
        let Some(target) = cx.targets.get(FOW_MASK) else {
            return;
        };
        let timestamps = cx.timestamps();
        let clear = if white {
            wgpu::Color::WHITE
        } else {
            wgpu::Color::TRANSPARENT
        };
        let mut pass = cx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("FowMask"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: timestamps,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if white {
            return;
        }
        let mut sizes: Vec<u32> = Vec::new();
        for d in &self.draws {
            let mesh = if d.wide { &self.wide } else { &self.narrow };
            pass.set_pipeline_c(&self.pipelines[d.pass]);
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint16);
            let mut left = d.hexes as usize;
            let mut batch = d.first_batch;
            while left > 0 {
                let n = left.min(BATCH);
                pass.set_bind_group_c(0, &self.group, &[(u64::from(batch) * BATCH_BYTES) as u32]);
                pass.draw_indexed_c(0..(n * HEX_INDICES) as u32, 0, 0..1);
                sizes.push((n * HEX_INDICES) as u32);
                left -= n;
                batch += 1;
            }
        }
        drop(pass);
        for n in sizes {
            cx.stats.record_draw(n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::Fog;

    fn fog(width: u32, height: u32, wrap: bool, f: impl Fn(u32, u32) -> Fog) -> FogInput {
        FogInput::new(
            width,
            height,
            wrap,
            (0..width * height)
                .map(|i| f(i % width, i / width))
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn the_rectangle_is_snapped_to_whole_tiles_and_holds_what_was_asked() {
        let v = MaskView::snapped(Vec2::new(-10.0, 5.0), Vec2::new(70.0, 130.0));
        assert_eq!(v.min, Vec2::new(-64.0, 0.0));
        assert_eq!(v.max(), Vec2::new(128.0, 192.0));
        assert_eq!(v.size % TILE, Vec2::ZERO);
        // A point-sized rectangle is still a tile.
        let p = MaskView::snapped(Vec2::new(10.0, 10.0), Vec2::new(10.0, 10.0));
        assert_eq!(p.size, Vec2::splat(TILE));
    }

    #[test]
    fn the_projection_maps_the_rectangle_to_the_clip_square() {
        let v = MaskView::snapped(Vec2::new(0.0, 0.0), Vec2::new(256.0, 128.0));
        let p = v.projection();
        let at = |x: f32, y: f32| p.project_point3(Vec3::new(x, y, 7.0));
        assert!((at(0.0, 0.0) - Vec3::new(-1.0, -1.0, 0.5)).length() < 1e-6);
        assert!((at(256.0, 128.0) - Vec3::new(1.0, 1.0, 0.5)).length() < 1e-6);
        assert!((at(128.0, 64.0).truncate()).length() < 1e-6);
        // The top of the rectangle (north) is the top of the texture: v = 0.
        assert_eq!(v.uv(Vec2::new(0.0, 128.0)), Vec2::new(0.0, 0.0));
        assert_eq!(v.uv(Vec2::new(256.0, 0.0)), Vec2::new(1.0, 1.0));
    }

    #[test]
    fn the_constant_block_has_the_registers_of_a_row_major_matrix() {
        // `v * M` in WGSL with this block's matrix is `G * v`.
        let g = Mat4::from_translation(Vec3::new(3.0, 5.0, 0.0))
            * Mat4::from_scale(Vec3::new(2.0, 4.0, 1.0));
        let m = row_major(&g);
        let wgsl = glam::Mat4::from_cols_array_2d(&m);
        let v = glam::Vec4::new(1.0, 1.0, 0.0, 1.0);
        let by_wgsl = glam::Vec4::new(
            v.dot(wgsl.col(0)),
            v.dot(wgsl.col(1)),
            v.dot(wgsl.col(2)),
            v.dot(wgsl.col(3)),
        );
        assert!((by_wgsl - g * v).length() < 1e-6, "{by_wgsl:?}");
        // Register 3 (the translation) is the fourth row of the original's matrix: component j of
        // it is `m[j][3]`.
        assert_eq!([m[0][3], m[1][3], m[2][3], m[3][3]], [3.0, 5.0, 0.0, 1.0]);
    }

    #[test]
    fn a_batch_holds_up_to_24_hexes_and_zeroes_the_rest() {
        let block = batch_block(
            &Mat4::IDENTITY,
            &[Vec2::new(64.0, 0.0), Vec2::new(0.0, 64.0)],
        );
        assert_eq!(block.len() as u64, BATCH_BYTES);
        assert!(block[2 * 64..].iter().all(|&b| b == 0));
        let first: &[[f32; 4]; 4] = bytemuck::from_bytes(&block[..64]);
        assert_eq!(
            first[0][3], 64.0,
            "register 3, component x: the translation"
        );
        assert_eq!(batches_for(0), 0);
        assert_eq!(batches_for(24), 1);
        assert_eq!(batches_for(25), 2);
    }

    #[test]
    fn the_fans_have_a_full_middle_and_a_rim_that_falls_to_nothing() {
        let (v, i) = hex_fans(REVEALED_RIM);
        assert_eq!(v.len(), HEX_VERTICES * BATCH);
        assert_eq!(i.len(), HEX_INDICES * BATCH);
        assert!(i.iter().all(|&k| (k as usize) < v.len()));
        // Hex 5: its slot is in every vertex, the weights go 1 (middle and corners), 0 (outer ring).
        let hex = &v[5 * HEX_VERTICES..6 * HEX_VERTICES];
        assert!(hex.iter().all(|p| p.slot[0] == 5.0 && p.slot[2] == 5.0));
        assert_eq!(hex[0].weight[0], 1.0);
        assert!(hex[1..7].iter().all(|p| p.weight[0] == 1.0));
        assert!(hex[7..].iter().all(|p| p.weight[0] == 0.0));
        let r = |p: &MaskVertex| (p.position[0].powi(2) + p.position[1].powi(2)).sqrt();
        assert!((r(&hex[1]) - CORNER_RADIUS).abs() < 1e-3);
        assert!((r(&hex[7]) - CORNER_RADIUS * REVEALED_RIM).abs() < 1e-3);
        // The indices of a hex stay in its own 13 vertices.
        let own = &i[5 * HEX_INDICES..6 * HEX_INDICES];
        assert!(
            own.iter()
                .all(|&k| (5 * HEX_VERTICES..6 * HEX_VERTICES).contains(&(k as usize)))
        );
        let (_, wide) = hex_fans(UNREVEALED_RIM);
        assert_eq!(wide, i);
    }

    #[test]
    fn the_class_of_a_hex_follows_the_fog_and_the_map_edge() {
        let f = fog(4, 3, false, |x, _| match x {
            0 => Fog::Unrevealed,
            1 => Fog::Hidden,
            _ => Fog::Visible,
        });
        assert_eq!(class_of(&f, 0, 1), MaskClass::Unrevealed);
        assert_eq!(class_of(&f, 1, 1), MaskClass::Revealed);
        assert_eq!(class_of(&f, 2, 1), MaskClass::Visible);
        // Off the map: unrevealed, on a flat map in x as in y.
        assert_eq!(class_of(&f, -1, 1), MaskClass::Unrevealed);
        assert_eq!(class_of(&f, 4, 1), MaskClass::Unrevealed);
        assert_eq!(class_of(&f, 2, -1), MaskClass::Unrevealed);
        assert_eq!(class_of(&f, 2, 3), MaskClass::Unrevealed);
        // A wrapping map continues in x.
        let w = fog(4, 3, true, |x, _| match x {
            0 => Fog::Unrevealed,
            _ => Fog::Visible,
        });
        assert_eq!(class_of(&w, 4, 1), MaskClass::Unrevealed);
        assert_eq!(class_of(&w, -1, 1), MaskClass::Visible);
        assert_eq!(class_of(&w, 2, 3), MaskClass::Unrevealed);
    }

    #[test]
    fn the_cells_of_a_rectangle_are_those_with_their_centre_in_it() {
        let (lo, hi) = (Vec2::new(0.0, -200.0), Vec2::new(300.0, 0.0));
        let cells = cells_in_rect(lo, hi, 0.0);
        assert!(!cells.is_empty());
        for &(x, y) in &cells {
            let c = cell_center(x, y);
            assert!(
                c.x >= lo.x - 1e-3
                    && c.x <= hi.x + 1e-3
                    && c.y >= lo.y - 1e-3
                    && c.y <= hi.y + 1e-3,
                "{x},{y} at {c:?}"
            );
        }
        // Every cell with its centre inside is there: 5 columns on the even rows, 4 or 5 on odd.
        let inside = (-6..10)
            .flat_map(|y| (-3..10).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let c = cell_center(x, y);
                c.x >= lo.x && c.x <= hi.x && c.y >= lo.y && c.y <= hi.y
            })
            .count();
        assert_eq!(cells.len(), inside);
    }

    #[test]
    fn the_hexes_of_the_mask_are_sorted_by_class_and_cover_the_rectangle() {
        let f = fog(8, 8, false, |x, y| match (x + y) % 3 {
            0 => Fog::Unrevealed,
            1 => Fog::Hidden,
            _ => Fog::Visible,
        });
        let to_orig = crate::frame::basis_rotation();
        let view = MaskView::snapped(Vec2::new(0.0, 0.0), Vec2::new(512.0, 400.0));
        let inst = collect_instances(&f, &view, &to_orig);
        assert!(
            !inst.revealed.is_empty() && !inst.visible.is_empty() && !inst.unrevealed.is_empty()
        );
        // Every hex of the map that is inside is drawn once, by its class.
        let (lo, hi) = view.ours_rect(&to_orig);
        let inside = cells_in_rect(lo, hi, CORNER_RADIUS * UNREVEALED_RIM).len();
        assert_eq!(inst.total(), inside);
        // Centres are in the original's basis: y north is -z of ours.
        let c = cell_center(2, 2);
        let want = Vec2::new(c.x, -c.y);
        assert!(
            [&inst.revealed, &inst.visible, &inst.unrevealed]
                .iter()
                .any(|l| l.iter().any(|p| (*p - want).length() < 1e-3))
        );
    }

    #[test]
    fn a_pass_stops_at_the_limit() {
        let f = fog(2, 2, false, |_, _| Fog::Unrevealed);
        let to_orig = crate::frame::basis_rotation();
        let view = MaskView::snapped(Vec2::new(-20000.0, -20000.0), Vec2::new(20000.0, 20000.0));
        let inst = collect_instances(&f, &view, &to_orig);
        assert_eq!(inst.unrevealed.len(), MAX_INSTANCES);
    }
}
