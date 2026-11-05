//! The bake of the cache pages: the passes "HeightMap", "Normal
//! Calculation" and "Terrain Cache" of the original, one cell and one page at a time, then the mips
//! and the copy into the layer of the page array.
//!
//! ```text
//! "HeightMap"           TerrainShaderScreenVS + TerrainShaderScreenPS    -> albedo, height   (HeightTemp)
//! "Normal Calculation"  VSFallThrough + MakeNormalPS                     -> normal           (NormalTemp)
//! "Terrain Cache"       TerrainRenderVSScreenSpace + ...CacheDiffuse     -> the page
//! mips                  `mips::generate` of render-core
//! ```
//!
//! A cell with a highlight (`TerrainCell::has_spec`) gets a second page from the same pool: the
//! highlight composite blends the specular maps of the rows into `SpecTemp` (`terrain_spec_composite_ps`,
//! a reconstruction: the original's DX11-only composite has no bytecode), and
//! `TerrainRenderCacheDiffuseAndSpec` writes the page and the highlight page at once.
//!
//! Not in this file (other cards): the decals ("Decal Layer", R2.14; the highlight of a decal,
//! `DecalPSSpec`, is not drawn yet), the macro shadow ("Macro Shadow", R2.15).
//!
//! The original renders for the vertex stage with the half-texel correction of Direct3D 9
//! (`g_TerrainScreenTexelOffsets`); wgpu's pixel centres need none, so the offsets are zero.
//! The temporary targets have the size of the page: the original makes them `g_iMaxTileSize`
//! square (open question, RW-5).

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack, Tex};
use std::collections::HashMap;

use glam::Vec2;

use super::cache::{BakeJob, PageKind, TerrainCache};
use super::cells::{
    BlendKernel, CellBox, CellGrid, CellVertices, Projections, TextureSet, VertexInputs,
    build_cell_vertices, snapped_scale,
};
use super::decal_pass::{DecalBake, DecalPass};
use super::pages::{PAGE_FORMAT, PageArrays, PageId, mip_count};
use super::sampler::TerrainSampler;
use super::settings_map::TerrainSettings;
use super::shadow_pass::{SHADOW_FORMAT, ShadowBake, ShadowPass};
use crate::mips::MipGenerator;
use crate::shaders::{catalog, create_module};

/// Textures the composite blends at once.
const SLOTS: usize = 8;
/// Entries of `g_fTextureMultiplierAndProjMode`.
const PROJECTION_REGISTERS: usize = 96;
/// Bytes of the constants of the vertex stage of the composite: a `vec2` at 0, the array at 16.
const SCREEN_VS_BYTES: usize = 16 + PROJECTION_REGISTERS * 16;
/// Bytes of a vertex of the composite (six `vec4`) and of the other two passes (two `vec4`).
const COMPOSITE_STRIDE: u64 = 6 * 16;
const SIMPLE_STRIDE: u64 = 2 * 16;

/// What can go wrong with a job. The job is dropped and reported; the page stays unready.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BakeError {
    /// The array texture of the page was not made (`PageArrays::ensure`).
    NoPageArray(PageId),
    /// `TerrainDef` has no GPU textures for the row.
    NoDefTexture(u32),
}

impl std::fmt::Display for BakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoPageArray(p) => write!(f, "no array texture for the page {p:?}"),
            Self::NoDefTexture(t) => write!(f, "no GPU texture for the TerrainDef row {t}"),
        }
    }
}

impl std::error::Error for BakeError {}

/// The textures of a `TerrainDef` row that the composite reads: the diffuse (its alpha is the
/// dominance), the height and the specular map (`None`: the row has no highlight). Made by the caller
/// from the textures of the library.
pub struct DefTexture {
    pub albedo: wgpu::TextureView,
    pub height: wgpu::TextureView,
    pub spec: Option<wgpu::TextureView>,
}

/// What lights the page of a style (the class of the cell): the sun and shadow cubes, the colour
/// volume, and the half vector of the highlight.
#[derive(Clone)]
pub struct BakeLight {
    /// The cubes through an sRGB format (`LightingEnv::terrain_sunlight_srgb`): the shader reads
    /// them as they come, the sampler decodes them.
    pub sun_cube: wgpu::TextureView,
    pub shadow_cube: wgpu::TextureView,
    pub color_correct: wgpu::TextureView,
    /// Direction between the sun and the eye (`g_Half`, register c5 of the cache shader).
    pub half: [f32; 3],
}

/// Settings of the baker.
#[derive(Clone, Copy, Debug)]
pub struct BakeConfig {
    /// The device renders to float targets (`EXT_color_buffer_float` on WebGL2; always on native).
    pub float_targets: bool,
    /// `HeightScale` of `MakeNormalPS`: how strong the height of the textures bends the normal
    /// ([`MAKE_NORMAL_HEIGHT_SCALE`] in the original).
    pub height_scale: f32,
}

/// `HeightScale` of `MakeNormalPS` (`NormalGlobsConstants::field_10`, register c1.x): the bump of the
/// height textures and decals in the page.
// re: TerrainCell.cpp:929 (`pNormalGlobs->field_10 = 16.0f`)
pub const MAKE_NORMAL_HEIGHT_SCALE: f32 = 16.0;

/// What a bake reads of the world.
pub struct BakeScene<'a> {
    pub sampler: &'a TerrainSampler,
    pub projections: &'a Projections,
    pub kernel: &'a BlendKernel,
    pub grid: &'a CellGrid,
    /// The textures of `TerrainDef`, indexed by the global row.
    pub defs: &'a [DefTexture],
    /// The light of the class of the cell (class 0..=4).
    pub light: &'a dyn Fn(u8) -> Option<&'a BakeLight>,
    /// The decals baked into the pages ("Decal Layer"); `None` draws none (`[DisableSystems] Decal`).
    pub decals: Option<DecalBake<'a>>,
    /// The macro shadow of the relief ("Macro Shadow"); `None` leaves the page without one.
    pub shadow: Option<ShadowBake<'a>>,
}

/// Outcome of a call of [`TerrainBaker::run`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BakeStats {
    pub baked: usize,
    pub failed: Vec<(BakeJob, BakeError)>,
}

/// Bytes of page vertices a frame of [`TerrainBaker::encode_jobs`] writes at most when it has a time
/// bound (about four pages of 0.56 MB): past it only the pages of holes are baked, the others wait a
/// frame drawn from the page they have.
pub const FRAME_VERTEX_BYTES: usize = 2 << 20;

/// The most pages a frame launches (launches at most 8).
pub const MAX_LAUNCHES_PER_FRAME: usize = 8;

/// The jobs a frame bakes: `TerrainPageinSpeedStill` when the camera stands, `...Moving` when it
/// moves, at most [`MAX_LAUNCHES_PER_FRAME`]; `MinimizeGrayTiles` gives the full budget of the frame.
// re: Terrain::LaunchJobs (Terrain.cpp:965-1130: the budgets `g_iPageinSpeed*`, Terrain.cpp:1046-1097)
pub fn bake_budget(settings: &TerrainSettings, moving: bool) -> usize {
    if settings.minimize_gray_tiles {
        return MAX_LAUNCHES_PER_FRAME;
    }
    (if moving {
        settings.pagein_moving
    } else {
        settings.pagein_still
    } as usize)
        .min(MAX_LAUNCHES_PER_FRAME)
}

/// The temporary targets of one page size.
struct Temps {
    albedo: wgpu::TextureView,
    height: wgpu::TextureView,
    normal: wgpu::TextureView,
    /// `ShadowTemp`: the macro shadow of the page.
    shadow: wgpu::TextureView,
    /// The page with its mips, before the copy into the array.
    scratch: Tex,
    /// `SpecTemp`: the blended specular maps of the cell.
    spec: wgpu::TextureView,
    /// The highlight page with its mips, before the copy into the array.
    spec_scratch: Tex,
    /// Level 0 of the two scratch pages, as render targets (made once with the pages).
    scratch_level0: wgpu::TextureView,
    spec_level0: wgpu::TextureView,
}

/// A buffer that the queue fills at growing offsets: every page of a frame gets a region of its own,
/// and the next frame writes over them again (the queue orders the writes before the submit that
/// reads them). When it is too small it is replaced by a bigger one; the commands already recorded
/// keep the old one alive.
///
/// The bytes of the pushes are kept on the CPU and written in one `write_buffer` at
/// [`Self::flush`] (before the submit of the commands that read them): two writes of half a megabyte
/// for every page of a far pan were frames of 30 to 90 ms in the browser.
struct Arena {
    buf: Option<Buf>,
    usage: wgpu::BufferUsages,
    min_size: u64,
    cursor: u64,
    /// Counts the buffers made, so that what was cached about the old one can be dropped.
    generation: u64,
    /// The bytes pushed since the last flush, from the offset `pending_at` of the buffer on (the gaps of
    /// the alignment are zeros).
    pending: Vec<u8>,
    pending_at: u64,
}

impl Arena {
    fn new(usage: wgpu::BufferUsages, min_size: u64) -> Self {
        Self {
            buf: None,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            min_size,
            cursor: 0,
            generation: 0,
            pending: Vec::new(),
            pending_at: 0,
        }
    }

    /// Writes the bytes pushed since the last flush into the buffer.
    fn flush(&mut self, queue: &wgpu::Queue) {
        if self.pending.is_empty() {
            return;
        }
        if let Some(buf) = self.buf.as_deref() {
            queue.write_buffer(buf, self.pending_at, &self.pending);
        }
        self.pending.clear();
    }

    /// Makes room for `len` more bytes (with the padding to `align`): when the buffer is too small it
    /// is replaced by a bigger one and the cursor starts over (what was pushed into the old one is
    /// written into it first). Call it before a group of pushes whose offsets must stay valid together.
    fn ensure(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, len: u64, align: u64) {
        let capacity = self.buf.as_ref().map_or(0, |b| b.size());
        let start = self.cursor.next_multiple_of(align);
        if self.buf.is_none() || start + len > capacity {
            let _s = crate::perf::slow::scope("bake.arena_grow");
            self.flush(queue);
            let size = self
                .min_size
                .max((len + align).next_power_of_two())
                .max(capacity * 2);
            self.buf = Some(device.buf(
                &wgpu::BufferDescriptor {
                    label: Some("terrain bake"),
                    size,
                    usage: self.usage,
                    mapped_at_creation: false,
                },
                Cluster::TerrainTex,
            ));
            self.generation += 1;
            self.cursor = 0;
        }
    }

    /// Writes `bytes` (a multiple of four long) at the next offset of `align`; returns the offset.
    fn push(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bytes: &[u8],
        align: u64,
    ) -> u64 {
        debug_assert!(bytes.len().is_multiple_of(4));
        let len = bytes.len() as u64;
        self.ensure(device, queue, len, align);
        let start = self.cursor.next_multiple_of(align);
        if self.pending.is_empty() {
            self.pending_at = start;
        }
        let gap = (start - self.pending_at) as usize - self.pending.len();
        self.pending.resize(self.pending.len() + gap, 0);
        self.pending.extend_from_slice(bytes);
        self.cursor = start + len;
        start
    }

    /// A region of `len` bytes at the next offset of `align` that the GPU writes (nothing is pushed);
    /// returns the offset.
    fn reserve(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, len: u64, align: u64) -> u64 {
        self.ensure(device, queue, len, align);
        let start = self.cursor.next_multiple_of(align);
        self.cursor = start + len;
        start
    }

    /// Starts the regions over; what was pushed is written first.
    fn reset(&mut self, queue: &wgpu::Queue) {
        self.flush(queue);
        self.cursor = 0;
    }

    fn buffer(&self) -> &wgpu::Buffer {
        self.buf.as_deref().expect("the arena was written")
    }
}

/// The uniform data of the pages of a frame, one 256-aligned slot per distinct value.
struct UniformRing {
    arena: Arena,
    align: u64,
    slots: HashMap<Vec<u8>, u32>,
    /// `arena.generation` when `slots` was filled.
    slots_generation: u64,
}

impl UniformRing {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            arena: Arena::new(wgpu::BufferUsages::UNIFORM, 64 * 1024),
            align: u64::from(device.limits().min_uniform_buffer_offset_alignment).max(16),
            slots: HashMap::new(),
            slots_generation: 0,
        }
    }

    fn reset(&mut self, queue: &wgpu::Queue) {
        self.arena.reset(queue);
        self.slots.clear();
    }

    /// The dynamic offset of a slot that holds `bytes` (written when the frame has none yet).
    fn offset(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8]) -> u32 {
        if self.slots_generation != self.arena.generation {
            self.slots.clear();
        }
        if let Some(&offset) = self.slots.get(bytes) {
            return offset;
        }
        let offset = self.arena.push(device, queue, bytes, self.align) as u32;
        // A new buffer drops the slots of the old one.
        if self.slots_generation != self.arena.generation {
            self.slots.clear();
            self.slots_generation = self.arena.generation;
        }
        self.slots.insert(bytes.to_vec(), offset);
        offset
    }
}

/// The bind groups that bind the uniform ring (dropped with its buffer).
#[derive(Default)]
struct RingGroups {
    generation: u64,
    /// Group 2 of the composite and of the highlight composite.
    vs: Option<(wgpu::BindGroup, wgpu::BindGroup)>,
    /// "Normal Calculation", by page size.
    normal: HashMap<u32, wgpu::BindGroup>,
    /// "Terrain Cache", by (page size, with the shadow, with the highlight, class).
    cache: HashMap<(u32, bool, bool, u8), CacheGroup>,
}

struct CacheGroup {
    /// The light the group was made for: the sun cube, the shadow cube, the colour volume.
    lights: [wgpu::TextureView; 3],
    group: wgpu::BindGroup,
}

/// Bind groups of the 16 textures of a composite, by the textures of the eight slots; each keeps the
/// views it was made of, to see that the textures were not made again.
type SlotGroups = HashMap<[u32; SLOTS], (Vec<wgpu::TextureView>, wgpu::BindGroup)>;

/// The vertices of the pages made on the GPU (render-opt GB, `bake_vertices.wgsl`): the CPU sends the
/// raw heights and ids around the cell (about 38 KB a page instead of 0.56 MB of vertices) and a
/// compute pass writes the composite and cache vertices into a buffer the passes read as their vertex
/// buffer. `None` (the CPU vertices of [`build_cell_vertices`]) where there is no compute stage with
/// storage buffers (WebGL2), or natively with `CIV5_BAKE_GPU_VERTICES=0`.
struct GpuVertices {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    /// The heights and ids of the pages of a frame (`u32` each).
    inputs: Arena,
    /// The vertices the compute pass writes: the composite and the cache vertices of each page.
    out: Arena,
    /// The bind group, by the generations of the uniform ring, `inputs` and `out`.
    group: Option<((u64, u64, u64), wgpu::BindGroup)>,
    /// The answer of the validation scope around the shader and the pipeline: until it says they are
    /// valid the pages are baked from the CPU vertices (the same pixels); an error makes the bake go
    /// back to the CPU for good, with a warning. Natively it answers at once; on the web it is a promise,
    /// polled once a page.
    check: Option<std::pin::Pin<Box<dyn std::future::Future<Output = Option<wgpu::Error>>>>>,
    verified: bool,
    /// The error the scope gave before the first page.
    failed: Option<String>,
}

/// What [`GpuVertices::poll`] says.
enum GpuCheck {
    Ready,
    Pending,
    Failed(String),
}

/// Bytes of the per-page constants of `bake_vertices.wgsl` (`Params`) and of the table (`Table`).
const GPU_PARAMS_BYTES: usize = 96;
const GPU_TABLE_BYTES: usize = 2256;
/// The side of the workgroup of `bake_vertices`.
const GPU_GROUP: u32 = 8;

impl GpuVertices {
    /// Whether the device has what the pass needs: a compute stage with two storage buffers.
    fn supported(device: &wgpu::Device) -> bool {
        let l = device.limits();
        l.max_storage_buffers_per_shader_stage >= 2
            && l.max_compute_workgroup_size_x >= GPU_GROUP
            && l.max_compute_workgroup_size_y >= GPU_GROUP
            && l.max_compute_invocations_per_workgroup >= GPU_GROUP * GPU_GROUP
    }

    /// The default of the device: on where it is supported, unless `CIV5_BAKE_GPU_VERTICES=0`
    /// (natively, to compare with the CPU vertices).
    fn wanted(device: &wgpu::Device) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if std::env::var("CIV5_BAKE_GPU_VERTICES").is_ok_and(|v| v == "0") {
            return false;
        }
        Self::supported(device)
    }

    /// The answer of the validation scope, if it came.
    fn poll(&mut self) -> GpuCheck {
        use std::task::{Context, Poll, Waker};
        if let Some(e) = self.failed.take() {
            return GpuCheck::Failed(e);
        }
        if self.verified {
            return GpuCheck::Ready;
        }
        let Some(check) = self.check.as_mut() else {
            self.verified = true;
            return GpuCheck::Ready;
        };
        let mut cx = Context::from_waker(Waker::noop());
        match check.as_mut().poll(&mut cx) {
            Poll::Pending => GpuCheck::Pending,
            Poll::Ready(None) => {
                self.check = None;
                self.verified = true;
                GpuCheck::Ready
            }
            Poll::Ready(Some(e)) => {
                self.check = None;
                GpuCheck::Failed(e.to_string())
            }
        }
    }

    fn new(device: &wgpu::Device) -> Self {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut made = Self::make(device);
        made.check = Some(Box::pin(scope.pop()));
        // Natively the scope answers here; the answer is told at the first page.
        if let GpuCheck::Failed(e) = made.poll() {
            made.failed = Some(e);
        }
        made
    }

    fn make(device: &wgpu::Device) -> Self {
        let module = create_module(
            device,
            "terrain_bake_vertices",
            include_str!("../../shaders/terrain/bake_vertices.wgsl"),
        );
        let buffer = |binding, ty, dynamic| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: dynamic,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain bake vertices"),
            entries: &[
                buffer(0, wgpu::BufferBindingType::Uniform, true),
                buffer(1, wgpu::BufferBindingType::Uniform, true),
                buffer(
                    2,
                    wgpu::BufferBindingType::Storage { read_only: true },
                    false,
                ),
                buffer(
                    3,
                    wgpu::BufferBindingType::Storage { read_only: false },
                    false,
                ),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain bake vertices"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("terrain bake vertices"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("bake_vertices"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self {
            pipeline,
            layout,
            inputs: Arena::new(wgpu::BufferUsages::STORAGE, 1 << 20),
            // As the vertex arena of the CPU path: the size of a busy frame at once.
            out: Arena::new(
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::VERTEX
                    | wgpu::BufferUsages::COPY_SRC,
                16 << 20,
            ),
            group: None,
            check: None,
            verified: false,
            failed: None,
        }
    }

    /// The table of `bake_vertices.wgsl` (`Table`): the projections of the ids, the blend kernel, the
    /// bytes of the weights as floats, the constants of the normal filter.
    fn table(projections: &Projections, kernel: &BlendKernel) -> Vec<u8> {
        use super::sampler::{
            DERIVATIVE_KERNEL, EXP_SCALE, RANGE_SCALE, SMOOTH_KERNEL, WEIGHT_NORM,
        };
        let mut words: Vec<u32> = Vec::with_capacity(GPU_TABLE_BYTES / 4);
        for id in 0..=255u8 {
            words.push(projections.first(id) | (u32::from(projections.mode(id)) << 16));
        }
        let mut floats = |values: &[f32], len: usize| {
            for i in 0..len {
                words.push(values.get(i).copied().unwrap_or(0.0).to_bits());
            }
        };
        floats(&kernel.weights, 28);
        let lut: Vec<f32> = (0..=255u8).map(|w| f32::from(w) / 255.0).collect();
        floats(&lut, 256);
        floats(&SMOOTH_KERNEL, 8);
        floats(&DERIVATIVE_KERNEL, 8);
        floats(
            &[
                RANGE_SCALE,
                EXP_SCALE,
                WEIGHT_NORM,
                super::cells::FLAT_NORMAL_Z,
            ],
            4,
        );
        // `f64::from(sum) < MIN_WEIGHT_SUM` is `sum < MIN_WEIGHT_SUM as f32` for every f32 `sum`.
        floats(
            &[
                super::cells::WEIGHT_SCALE,
                super::cells::MIN_WEIGHT_SUM as f32,
            ],
            4,
        );
        debug_assert_eq!(words.len() * 4, GPU_TABLE_BYTES);
        bytemuck::cast_slice(&words).to_vec()
    }

    /// What the CPU sends for a cell: the columns and rows (clip, place in the page through a half,
    /// world X / Y; four `u32` each), the window of taps (raw height | id << 16, two texels around the
    /// vertices, wrapped and clamped as `normals_block` and `calc_blend_vector` read them), and the raw
    /// heights of the vertices (wrapped as `build_cell_vertices` reads them). Returns the words and the
    /// offsets (columns, rows, window, heights) in them.
    fn inputs(sampler: &TerrainSampler, rect: &CellBox) -> (Vec<u32>, [u32; 4]) {
        let field = sampler.field();
        let (mw, mh) = (field.width() as i32, field.height() as i32);
        let wrap_x = sampler.frame().wrap_x();
        let scale = sampler.frame().terrain_scale();
        let (width, height) = (rect.w + 2, rect.h + 2);
        let (fw, fh) = ((rect.w + 1) as f32, (rect.h + 1) as f32);
        let through_half = |v: f32| asset_format::f16_bits_to_f32(asset_format::f32_to_f16_bits(v));
        let (ww, wh) = (width + 4, height + 4);
        let cols_at = 0u32;
        let rows_at = cols_at + width as u32 * 4;
        let window_at = rows_at + height as u32 * 4;
        let heights_at = window_at + (ww * wh) as u32;
        let mut words = Vec::with_capacity((heights_at as i32 + width * height) as usize);
        for ix in -1..=rect.w {
            let u = (ix + 1) as f32 / fw;
            let hx = rect.x + ix;
            words.extend_from_slice(&[
                (u + u - 1.0).to_bits(),
                through_half(u).to_bits(),
                (scale.x * hx as f32).to_bits(),
                0,
            ]);
        }
        for iy in -1..=rect.h {
            let v = (iy + 1) as f32 / fh;
            let hy = rect.y + iy;
            words.extend_from_slice(&[
                (-2.0 * v + 1.0).to_bits(),
                through_half(v).to_bits(),
                (scale.y * hy as f32).to_bits(),
                0,
            ]);
        }
        // The window starts two texels before the first vertex (texel `rect.x - 1`).
        let (x0, y0) = (rect.x - 3, rect.y - 3);
        let cols: Vec<(i32, i32)> = (0..ww)
            .map(|x| {
                let u = x0 + x;
                let wrapped = if wrap_x { u.rem_euclid(mw) } else { u };
                // `normal_window`: wrapped, then clamped; `blend_tap`: wrapped, or unsigned.
                let id_u = if wrap_x {
                    u.rem_euclid(mw)
                } else {
                    (u as u32 % mw as u32) as i32
                };
                (wrapped.clamp(0, mw - 1), id_u)
            })
            .collect();
        for y in 0..wh {
            let v = y0 + y;
            let (hv, id_v) = (v.clamp(0, mh - 1), (v as u32 % mh as u32) as i32);
            for &(hu, id_u) in &cols {
                words.push(
                    u32::from(field.get_height(hu, hv))
                        | (u32::from(field.get_id(id_u, id_v)) << 16),
                );
            }
        }
        for iy in -1..=rect.h {
            let wy = (mh + rect.y + iy).rem_euclid(mh);
            for ix in -1..=rect.w {
                let wx = (mw + rect.x + ix).rem_euclid(mw);
                words.push(u32::from(sampler.raw_height(wx, wy)));
            }
        }
        (words, [cols_at, rows_at, window_at, heights_at])
    }
}

/// The passes of the bake.
pub struct TerrainBaker {
    config: BakeConfig,
    height_format: wgpu::TextureFormat,
    normal_format: wgpu::TextureFormat,
    temps: HashMap<u32, Temps>,
    mips: MipGenerator,
    repeat: wgpu::Sampler,
    clamp: wgpu::Sampler,
    composite: Pipeline,
    /// The highlight composite: the albedo of the slots and their specular maps into `SpecTemp`.
    spec_composite: Pipeline,
    normal: Pipeline,
    cache: Pipeline,
    /// `TerrainRenderCacheDiffuseAndSpec`: the page and the highlight page, with the layout of `cache`.
    cache_spec: wgpu::RenderPipeline,
    decals: DecalPass,
    shadow: ShadowPass,
    /// 1x1 black: the macro shadow and the highlight that are not baked here.
    black: wgpu::TextureView,
    /// The vertex data of the pages of a frame (composite and cache vertices).
    verts: Arena,
    uniforms: UniformRing,
    ring_groups: RingGroups,
    composite_groups: SlotGroups,
    spec_groups: SlotGroups,
    /// The index buffers of the grids, by (width, height) of the vertices, with their index count.
    grids: HashMap<(i32, i32), (Buf, u32)>,
    /// The quad of "Normal Calculation": vertices and indices.
    quad: (Buf, Buf),
    /// The vertices made on the GPU; `None`: on the CPU.
    gpu: Option<GpuVertices>,
}

struct Pipeline {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    /// The layout of group 2 (composite only).
    layout2: Option<wgpu::BindGroupLayout>,
}

fn texture_entry(binding: u32, dim: wgpu::TextureViewDimension) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: dim,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: true,
            min_binding_size: None,
        },
        count: None,
    }
}

fn float4_attributes(count: u32) -> Vec<wgpu::VertexAttribute> {
    (0..count)
        .map(|i| wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: u64::from(i) * 16,
            shader_location: i,
        })
        .collect()
}

impl TerrainBaker {
    /// Makes the pipelines. The render targets of the temporary maps are `Rgba16Float` when the
    /// device renders to float, else `Rgba8Unorm`.
    // SIMPLIFIED(SIMP-038): 8-bit bake targets on WebGL2 without EXT_color_buffer_float (the height
    // loses precision for the normals); native and WebGPU keep Rgba16Float
    pub fn new(device: &wgpu::Device, config: BakeConfig) -> Self {
        Self::with_gpu_vertices(device, config, GpuVertices::wanted(device))
    }

    /// As [`Self::new`], with the vertices of the pages made on the GPU or not (`gpu_vertices` is
    /// ignored where the device has no compute stage with storage buffers).
    pub fn with_gpu_vertices(
        device: &wgpu::Device,
        config: BakeConfig,
        gpu_vertices: bool,
    ) -> Self {
        let gpu =
            (gpu_vertices && GpuVertices::supported(device)).then(|| GpuVertices::new(device));
        let height_format = if config.float_targets {
            wgpu::TextureFormat::Rgba16Float
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };
        let normal_format = wgpu::TextureFormat::Rgba8Unorm;
        let sampler = |mode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("terrain bake"),
                address_mode_u: mode,
                address_mode_v: mode,
                address_mode_w: mode,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            })
        };
        let black = device
            .tex(
                &wgpu::TextureDescriptor {
                    label: Some("terrain bake black"),
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
                Cluster::TerrainTex,
            )
            .create_view(&Default::default());
        let (cache, cache_spec) = Self::cache_pipeline(device, PAGE_FORMAT);
        let (quad_vertices, quad_indices) = Self::full_quad();
        let quad = (
            Self::buffer(
                device,
                wgpu::BufferUsages::VERTEX,
                bytemuck::cast_slice(&quad_vertices),
            ),
            Self::buffer(
                device,
                wgpu::BufferUsages::INDEX,
                bytemuck::cast_slice(&quad_indices),
            ),
        );
        Self {
            // A page is about 0.56 MB of vertices (66 x 66 of 128 bytes) and a frame bakes up to eight
            // cells with their near and highlight pages: made at the size of a busy frame at once, not
            // doubled four times in the middle of a far pan.
            verts: Arena::new(wgpu::BufferUsages::VERTEX, 16 << 20),
            uniforms: UniformRing::new(device),
            ring_groups: RingGroups::default(),
            composite_groups: HashMap::new(),
            spec_groups: HashMap::new(),
            grids: HashMap::new(),
            quad,
            config,
            height_format,
            normal_format,
            temps: HashMap::new(),
            mips: MipGenerator::new(device),
            repeat: sampler(wgpu::AddressMode::Repeat),
            clamp: sampler(wgpu::AddressMode::ClampToEdge),
            composite: Self::composite_pipeline(
                device,
                "terrain bake composite",
                "terrain_shader_screen_ps",
                &[height_format, height_format],
            ),
            spec_composite: Self::composite_pipeline(
                device,
                "terrain bake highlight composite",
                "terrain_spec_composite_ps",
                &[height_format],
            ),
            normal: Self::normal_pipeline(device, normal_format),
            cache,
            cache_spec,
            decals: DecalPass::new(device, height_format),
            shadow: ShadowPass::new(device),
            black,
            gpu,
        }
    }

    pub fn config(&self) -> &BakeConfig {
        &self.config
    }

    /// Whether the vertices of the pages are made on the GPU (`terrain.vertex_path`): the compute
    /// pipeline is there and the device said it is valid.
    pub fn gpu_vertices(&self) -> bool {
        self.gpu.as_ref().is_some_and(|g| g.verified)
    }

    /// Whether this page takes the GPU vertices: the answer of the validation scope came and was
    /// good. A bad one drops the compute pipeline for good, with a warning that tells why.
    fn gpu_ready(&mut self) -> bool {
        let Some(g) = self.gpu.as_mut() else {
            return false;
        };
        match g.poll() {
            GpuCheck::Ready => true,
            GpuCheck::Pending => false,
            GpuCheck::Failed(e) => {
                tracing::warn!(
                    "terrain bake: page vertices on the CPU: the compute pipeline failed: {e}"
                );
                self.gpu = None;
                false
            }
        }
    }

    /// The format of the temporary maps.
    pub fn height_format(&self) -> wgpu::TextureFormat {
        self.height_format
    }

    fn module(device: &wgpu::Device, name: &str) -> wgpu::ShaderModule {
        let m = catalog()
            .iter()
            .find(|m| m.name == name)
            .unwrap_or_else(|| panic!("the module {name} is not in the catalog"));
        create_module(device, name, &m.source())
    }

    /// A pipeline of the composite (vertex stage `TerrainShaderScreenVS`, the group of 16 textures):
    /// the albedo and the height, or the highlight.
    fn composite_pipeline(
        device: &wgpu::Device,
        label: &str,
        entry: &str,
        targets: &[wgpu::TextureFormat],
    ) -> Pipeline {
        let module = Self::module(device, "terrain_bake_composite");
        // Group 1: eight albedo and eight height textures, each with a sampler.
        let mut entries = Vec::new();
        for k in 0..16u32 {
            entries.push(texture_entry(k * 2, wgpu::TextureViewDimension::D2));
            entries.push(sampler_entry(k * 2 + 1));
        }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(label),
            entries: &entries,
        });
        let layout2 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain bake composite vs"),
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX)],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: &[None, Some(&layout), Some(&layout2)],
            immediate_size: 0,
        });
        let attributes = float4_attributes(6);
        let targets: Vec<Option<wgpu::ColorTargetState>> =
            targets.iter().map(|&f| Some(f.into())).collect();
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("terrain_shader_screen_vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: COMPOSITE_STRIDE,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        });
        Pipeline {
            pipeline,
            layout,
            layout2: Some(layout2),
        }
    }

    fn normal_pipeline(device: &wgpu::Device, target: wgpu::TextureFormat) -> Pipeline {
        let module = Self::module(device, "terrain_bake_normal");
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain bake normal"),
            entries: &[
                texture_entry(0, wgpu::TextureViewDimension::D2),
                sampler_entry(1),
                uniform_entry(2, wgpu::ShaderStages::FRAGMENT),
                uniform_entry(3, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain bake normal"),
            bind_group_layouts: &[None, Some(&layout)],
            immediate_size: 0,
        });
        let attributes = float4_attributes(2);
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain bake normal"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("terrain_vs_fall_through"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: SIMPLE_STRIDE,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("make_normal_ps"),
                compilation_options: Default::default(),
                targets: &[Some(target.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        Pipeline {
            pipeline,
            layout,
            layout2: None,
        }
    }

    /// The pipelines of "Terrain Cache": the page alone (`TerrainRenderCacheDiffuse`), and the page with
    /// the highlight page (`TerrainRenderCacheDiffuseAndSpec`, two targets), on one layout.
    fn cache_pipeline(
        device: &wgpu::Device,
        target: wgpu::TextureFormat,
    ) -> (Pipeline, wgpu::RenderPipeline) {
        use wgpu::TextureViewDimension::{Cube, D2, D3};
        let module = Self::module(device, "terrain_bake_cache");
        // The order of `bake_cache.wgsl`: normal, albedo, shadow, spec, sun cube, shadow cube,
        // colour volume (each with its sampler), then `g_Half`.
        let mut entries = Vec::new();
        for (i, dim) in [D2, D2, D2, D2, Cube, Cube, D3].into_iter().enumerate() {
            entries.push(texture_entry(i as u32 * 2, dim));
            entries.push(sampler_entry(i as u32 * 2 + 1));
        }
        entries.push(uniform_entry(14, wgpu::ShaderStages::FRAGMENT));
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain bake cache"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain bake cache"),
            bind_group_layouts: &[None, Some(&layout)],
            immediate_size: 0,
        });
        let attributes = float4_attributes(2);
        let make = |label: &str, entry: &str, targets: &[Option<wgpu::ColorTargetState>]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("terrain_render_vs_screen_space"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: SIMPLE_STRIDE,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    })],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline = make(
            "terrain bake cache",
            "terrain_cache_diffuse_ps",
            &[Some(target.into())],
        );
        let with_spec = make(
            "terrain bake cache and highlight",
            "terrain_cache_diffuse_and_spec_ps",
            &[Some(target.into()), Some(target.into())],
        );
        (
            Pipeline {
                pipeline,
                layout,
                layout2: None,
            },
            with_spec,
        )
    }

    fn temps(&mut self, device: &wgpu::Device, size: u32) -> &Temps {
        let height_format = self.height_format;
        let normal_format = self.normal_format;
        self.temps.entry(size).or_insert_with(|| {
            let target = |label: &str, format| {
                device
                    .tex(
                        &wgpu::TextureDescriptor {
                            label: Some(label),
                            size: wgpu::Extent3d {
                                width: size,
                                height: size,
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format,
                            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                                | wgpu::TextureUsages::TEXTURE_BINDING,
                            view_formats: &[],
                        },
                        Cluster::TerrainTex,
                    )
                    .create_view(&Default::default())
            };
            let scratch = |label: &str| {
                device.tex(
                    &wgpu::TextureDescriptor {
                        label: Some(label),
                        size: wgpu::Extent3d {
                            width: size,
                            height: size,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: mip_count(size),
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: PAGE_FORMAT,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::TEXTURE_BINDING
                            | wgpu::TextureUsages::COPY_SRC,
                        view_formats: &[],
                    },
                    Cluster::TerrainTex,
                )
            };
            let level0 = |t: &Tex, label: &str| {
                t.create_view(&wgpu::TextureViewDescriptor {
                    label: Some(label),
                    base_mip_level: 0,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            };
            let scratch_tex = scratch("terrain bake scratch");
            let spec_scratch_tex = scratch("terrain bake highlight scratch");
            Temps {
                albedo: target("terrain bake albedo", height_format),
                height: target("terrain bake height", height_format),
                normal: target("terrain bake normal", normal_format),
                shadow: target("terrain bake shadow", SHADOW_FORMAT),
                scratch_level0: level0(&scratch_tex, "terrain bake scratch"),
                scratch: scratch_tex,
                spec: target("terrain bake highlight", height_format),
                spec_level0: level0(&spec_scratch_tex, "terrain bake highlight scratch"),
                spec_scratch: spec_scratch_tex,
            }
        })
    }

    /// The constants of the vertex stage of the composite: the half-texel offsets (zero here) and
    /// for each projection the multiplier of its scale (snapped to a whole number of tiles over the
    /// world) and its axis.
    // re: TerrainCell::BuildTexture (TerrainCell.cpp:826-845): the constant table of the composite
    pub fn screen_vs_constants(projections: &Projections, world_width: f32) -> Vec<u8> {
        let mut floats = vec![0.0f32; SCREEN_VS_BYTES / 4];
        for p in 0..projections.count().min(PROJECTION_REGISTERS) {
            let axis = projections.axis_of(p as u32).unwrap_or(0);
            let scale = snapped_scale(projections.scale_of(p as u32), axis, world_width);
            let at = 4 + p * 4;
            floats[at] = scale;
            floats[at + 1] = f32::from(axis);
        }
        floats.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    /// The vertex data of the composite: six `vec4` a vertex (corner, world point, weights 0..3
    /// and 4..7, slots 0..3 and 4..7).
    pub fn composite_vertices(v: &CellVertices) -> Vec<f32> {
        let mut out = Vec::with_capacity(v.composite.len() * 24);
        for c in &v.composite {
            out.extend_from_slice(&[c.clip.x, c.clip.y, c.clip.z, 1.0]);
            out.extend_from_slice(&[c.world.x, c.world.y, c.world.z, 1.0]);
            out.extend(c.weights[..4].iter().map(|&w| f32::from(w) / 255.0));
            out.extend(c.weights[4..].iter().map(|&w| f32::from(w) / 255.0));
            out.extend(c.textures[..4].iter().map(|&t| f32::from(t)));
            out.extend(c.textures[4..].iter().map(|&t| f32::from(t)));
        }
        out
    }

    /// The vertex data of the cache pass: the normal and the place in the page (two `vec4`).
    pub fn cache_vertices(v: &CellVertices) -> Vec<f32> {
        let mut out = Vec::with_capacity(v.compressed.len() * 8);
        for c in &v.compressed {
            let half = |bits: u32| asset_format::f16_bits_to_f32(bits as u16);
            out.extend_from_slice(&[
                half(c.normal_xy),
                half(c.normal_xy >> 16),
                half(c.normal_zw),
                0.0,
            ]);
            out.extend_from_slice(&[half(c.texcoord), half(c.texcoord >> 16), 0.0, 0.0]);
        }
        out
    }

    /// Two triangles for each of the `(w + 1) x (h + 1)` quads of the `(w + 2) x (h + 2)` vertices.
    pub fn grid_indices(width: i32, height: i32) -> Vec<u32> {
        let mut out = Vec::with_capacity(((width - 1) * (height - 1) * 6).max(0) as usize);
        for y in 0..height - 1 {
            for x in 0..width - 1 {
                let i = (y * width + x) as u32;
                let (a, b, c, d) = (i, i + 1, i + width as u32, i + width as u32 + 1);
                out.extend_from_slice(&[a, b, c, b, d, c]);
            }
        }
        out
    }

    /// A triangle pair that covers the target, for the normal pass: (corner, texcoord).
    fn full_quad() -> ([f32; 32], [u32; 6]) {
        (
            [
                -1.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, //
                1.0, 1.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, //
                -1.0, -1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, //
                1.0, -1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 0.0,
            ],
            [0, 1, 2, 1, 3, 2],
        )
    }

    fn buffer(device: &wgpu::Device, usage: wgpu::BufferUsages, bytes: &[u8]) -> Buf {
        let size = (bytes.len() as u64).next_multiple_of(4).max(16);
        let b = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("terrain bake"),
                size,
                usage,
                mapped_at_creation: true,
            },
            Cluster::TerrainTex,
        );
        if let Ok(mut m) = b.get_mapped_range_mut(..) {
            let mut padded = bytes.to_vec();
            padded.resize(size as usize, 0);
            m.copy_from_slice(&padded);
        }
        b.unmap();
        b
    }

    /// The bind group of the 16 textures of a composite (view `i` at binding `2 i`, its sampler at
    /// `2 i + 1`), made once for the textures of a set of slots and kept while the views are the same.
    fn slot_group<'c>(
        device: &wgpu::Device,
        cache: &'c mut SlotGroups,
        layout: &wgpu::BindGroupLayout,
        label: &str,
        sampler: &wgpu::Sampler,
        ids: [u32; SLOTS],
        views: &[&wgpu::TextureView; 2 * SLOTS],
    ) -> &'c wgpu::BindGroup {
        let fresh = cache
            .get(&ids)
            .is_some_and(|(old, _)| old.iter().zip(views).all(|(a, b)| a == *b));
        if !fresh {
            if cache.len() >= 512 {
                cache.clear();
            }
            let mut entries: Vec<wgpu::BindGroupEntry<'_>> = Vec::with_capacity(views.len() * 2);
            for (i, view) in views.iter().enumerate() {
                entries.push(wgpu::BindGroupEntry {
                    binding: i as u32 * 2,
                    resource: wgpu::BindingResource::TextureView(view),
                });
                entries.push(wgpu::BindGroupEntry {
                    binding: i as u32 * 2 + 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                });
            }
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout,
                entries: &entries,
            });
            cache.insert(ids, (views.iter().map(|v| (*v).clone()).collect(), group));
        }
        &cache[&ids].1
    }

    /// Bakes one page of one cell: the three passes, the mips, and the copy into the array, in
    /// commands of its own that are submitted at once.
    #[allow(clippy::too_many_arguments)]
    pub fn bake_page(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        arrays: &PageArrays,
        page: PageId,
        rect: &CellBox,
        class: u8,
        textures: &TextureSet,
        scene: &BakeScene<'_>,
    ) -> Result<(), BakeError> {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("terrain bake"),
        });
        self.begin_frame(queue);
        self.encode_page(
            device,
            queue,
            &mut encoder,
            arrays,
            page,
            rect,
            class,
            textures,
            scene,
        )?;
        queue.submit([encoder.finish()]);
        Ok(())
    }

    /// The commands that bake one page of one cell into `encoder`: the three passes, the mips and
    /// the copy into the layer of the array (the pass "TerrainBake" of the frame).
    #[allow(clippy::too_many_arguments)]
    pub fn encode_page(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        arrays: &PageArrays,
        page: PageId,
        rect: &CellBox,
        class: u8,
        textures: &TextureSet,
        scene: &BakeScene<'_>,
    ) -> Result<(), BakeError> {
        self.encode_pages(
            device, queue, encoder, arrays, page, None, rect, class, textures, scene,
        )
    }

    /// Starts the per-frame regions of the vertex and uniform data over. Only when the commands of the
    /// frame before were submitted: the queue orders the writes of this frame after that submit.
    fn begin_frame(&mut self, queue: &wgpu::Queue) {
        self.verts.reset(queue);
        self.uniforms.reset(queue);
        if let Some(g) = self.gpu.as_mut() {
            g.inputs.reset(queue);
            g.out.reset(queue);
        }
    }

    /// Writes the vertex and uniform data the pages recorded so far read (before their submit).
    fn flush(&mut self, queue: &wgpu::Queue) {
        self.verts.flush(queue);
        self.uniforms.arena.flush(queue);
        if let Some(g) = self.gpu.as_mut() {
            g.inputs.flush(queue);
        }
    }

    /// Bytes of page data pushed since the last flush (vertices or, with the GPU vertices, heights).
    fn pending_bytes(&self) -> usize {
        self.verts.pending.len() + self.gpu.as_ref().map_or(0, |g| g.inputs.pending.len())
    }

    /// As [`Self::encode_page`], with the highlight page of the cell when it has one (`spec`, a page of
    /// the same size): the highlight composite and `TerrainRenderCacheDiffuseAndSpec` fill it too.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_pages(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        arrays: &PageArrays,
        page: PageId,
        spec: Option<PageId>,
        rect: &CellBox,
        class: u8,
        textures: &TextureSet,
        scene: &BakeScene<'_>,
    ) -> Result<(), BakeError> {
        let result = self.record_pages(
            device, queue, encoder, arrays, page, spec, rect, class, textures, scene, None,
        );
        self.flush(queue);
        result
    }

    /// [`Self::encode_pages`] without the write of its data: [`Self::encode_jobs`] writes the data of
    /// all the pages of a frame at once.
    #[allow(clippy::too_many_arguments)]
    fn record_pages(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        arrays: &PageArrays,
        page: PageId,
        spec: Option<PageId>,
        rect: &CellBox,
        class: u8,
        textures: &TextureSet,
        scene: &BakeScene<'_>,
        prebuilt: Option<CellVertices>,
    ) -> Result<(), BakeError> {
        let size = page.size;
        let target_array = arrays.texture(page).ok_or(BakeError::NoPageArray(page))?;
        let layer = arrays.layer(page);
        // The highlight page: only of the size of the page, and only with an array to copy into.
        let spec = spec
            .filter(|s| s.size == size)
            .and_then(|s| Some((arrays.texture(s)?, arrays.layer(s))));
        // The rows of `TerrainDef` of the eight slots; an empty slot repeats the first.
        let mut ids = [0u32; SLOTS];
        for (k, id) in ids.iter_mut().enumerate() {
            let projection = textures
                .ids
                .get(k)
                .or(textures.ids.first())
                .copied()
                .unwrap_or(0);
            let texture = scene.projections.texture_of(projection).unwrap_or(0);
            if scene.defs.get(texture as usize).is_none() {
                return Err(BakeError::NoDefTexture(texture));
            }
            *id = texture;
        }
        let light = (scene.light)(class).ok_or(BakeError::NoDefTexture(u32::MAX))?;

        let (grid_w, grid_h) = (rect.w + 2, rect.h + 2);
        let vertex_count = (grid_w * grid_h).max(0) as u64;
        // With the GPU vertices: the heights and ids of the cell go into the input buffer of the
        // frame, and a region of the output buffer is kept for the vertices the compute pass writes.
        let mut gpu_job: Option<([u32; 4], u64, u64)> = None;
        let gpu_ready = self.gpu_ready();
        let (composite_range, cache_range) =
            if let Some(g) = self.gpu.as_mut().filter(|_| gpu_ready) {
                let slow_vertices = crate::perf::slow::scope("bake.vertices");
                let (words, at) = GpuVertices::inputs(scene.sampler, rect);
                drop(slow_vertices);
                let _s = crate::perf::slow::scope("bake.upload");
                let input_at = g
                    .inputs
                    .push(device, queue, bytemuck::cast_slice(&words), 16);
                let input_word = (input_at / 4) as u32;
                let composite_len = vertex_count * COMPOSITE_STRIDE;
                let cache_len = vertex_count * SIMPLE_STRIDE;
                let composite_at = g.out.reserve(device, queue, composite_len + cache_len, 16);
                let cache_at = composite_at + composite_len;
                gpu_job = Some((at.map(|a| a + input_word), composite_at, cache_at));
                (
                    composite_at..composite_at + composite_len,
                    cache_at..cache_at + cache_len,
                )
            } else {
                let slow_vertices = crate::perf::slow::scope("bake.vertices");
                // Made on the pool already ([`Self::encode_jobs`]): the same function of the same inputs.
                let vertices = prebuilt.unwrap_or_else(|| {
                    build_cell_vertices(
                        &VertexInputs {
                            sampler: scene.sampler,
                            projections: scene.projections,
                            kernel: scene.kernel,
                            textures,
                            page_size: size,
                            gpu_weights: false,
                        },
                        rect,
                    )
                });
                debug_assert_eq!((vertices.width, vertices.height), (grid_w, grid_h));
                // The vertex data goes into the vertex buffer of the frame, a region for each pass; the
                // buffer is made big enough for both before either is written, so that the offsets stay
                // valid.
                let composite_data = Self::composite_vertices(&vertices);
                let cache_data = Self::cache_vertices(&vertices);
                drop(slow_vertices);
                let _s = crate::perf::slow::scope("bake.upload");
                let composite_bytes: &[u8] = bytemuck::cast_slice(&composite_data);
                let cache_bytes: &[u8] = bytemuck::cast_slice(&cache_data);
                self.verts.ensure(
                    device,
                    queue,
                    (composite_bytes.len() + cache_bytes.len()) as u64 + 32,
                    16,
                );
                let composite_at = self.verts.push(device, queue, composite_bytes, 16);
                let cache_at = self.verts.push(device, queue, cache_bytes, 16);
                (
                    composite_at..composite_at + composite_bytes.len() as u64,
                    cache_at..cache_at + cache_bytes.len() as u64,
                )
            };
        let slow_upload = crate::perf::slow::scope("bake.upload");

        // The index buffer of the grid, made once for each size of grid.
        let grid_key = (grid_w, grid_h);
        let index_count = self
            .grids
            .entry(grid_key)
            .or_insert_with(|| {
                let _s = crate::perf::slow::scope("bake.grid_new");
                let indices = Self::grid_indices(grid_key.0, grid_key.1);
                (
                    Self::buffer(
                        device,
                        wgpu::BufferUsages::INDEX,
                        bytemuck::cast_slice(&indices),
                    ),
                    indices.len() as u32,
                )
            })
            .1;

        // Constants: one slot of the uniform ring each, bound with a dynamic offset.
        let vs_bytes =
            Self::screen_vs_constants(scene.projections, scene.sampler.frame().world_width());
        // `NormalGlobsConstants`: the inverse of the page size in x and y, zero in z (so the taps of
        // `MakeNormalPS` that step by z are on the middle row and column: the plain Sobel filter).
        // re: TerrainCell.cpp:928-934 (`fInvTileSizeX/Y = 1 / fMaxTileSize`, `field_8 = 0`)
        let texel = 1.0 / size as f32;
        let texel_bytes: [u8; 16] = bytemuck::cast([texel, texel, 0.0, 0.0]);
        let scale_bytes: [u8; 16] = bytemuck::cast([self.config.height_scale, 0.0, 0.0, 0.0]);
        let half_bytes: [u8; 16] =
            bytemuck::cast([light.half[0], light.half[1], light.half[2], 0.0]);
        // The constants of the GPU vertices (`Params`, `Table` of `bake_vertices.wgsl`).
        let gpu_uniforms = gpu_job.map(|(at, composite_at, cache_at)| {
            let mut words = [0u32; GPU_PARAMS_BYTES / 4];
            words[..4].copy_from_slice(&[grid_w as u32, grid_h as u32, at[0], at[1]]);
            words[4..8].copy_from_slice(&[
                at[2],
                at[3],
                (composite_at / 16) as u32,
                (cache_at / 16) as u32,
            ]);
            words[8] = textures.ids.len().min(SLOTS) as u32;
            words[9] = scene.projections.count() as u32;
            for (k, &p) in textures.ids.iter().take(SLOTS).enumerate() {
                words[12 + k] = p;
            }
            let scale = scene.sampler.frame().terrain_scale();
            words[20..24].copy_from_slice(&[
                scale.x.to_bits(),
                scale.y.to_bits(),
                scale.z.to_bits(),
                super::sampler::HEIGHT_TO_UNIT.to_bits(),
            ]);
            (
                bytemuck::cast_slice::<u32, u8>(&words).to_vec(),
                GpuVertices::table(scene.projections, scene.kernel),
            )
        });
        let gpu_bytes = if gpu_uniforms.is_some() {
            (GPU_PARAMS_BYTES as u64).next_multiple_of(self.uniforms.align)
                + (GPU_TABLE_BYTES as u64).next_multiple_of(self.uniforms.align)
        } else {
            0
        };
        self.uniforms.arena.ensure(
            device,
            queue,
            3 * self.uniforms.align
                + (SCREEN_VS_BYTES as u64).next_multiple_of(self.uniforms.align)
                + gpu_bytes,
            self.uniforms.align,
        );
        let vs_offset = self.uniforms.offset(device, queue, &vs_bytes);
        let texel_offset = self.uniforms.offset(device, queue, &texel_bytes);
        let scale_offset = self.uniforms.offset(device, queue, &scale_bytes);
        let half_offset = self.uniforms.offset(device, queue, &half_bytes);
        let gpu_offsets = gpu_uniforms.map(|(params, table)| {
            (
                self.uniforms.offset(device, queue, &params),
                self.uniforms.offset(device, queue, &table),
            )
        });
        drop(slow_upload);

        {
            let _s = crate::perf::slow::scope("bake.temps");
            self.temps(device, size);
        }
        let temps = &self.temps[&size];
        let slow_groups = crate::perf::slow::scope("bake.groups");

        // Bind groups: the ones that bind the uniform ring are kept until it is made again.
        if self.ring_groups.generation != self.uniforms.arena.generation {
            self.ring_groups = RingGroups {
                generation: self.uniforms.arena.generation,
                ..RingGroups::default()
            };
        }
        let ring = self.uniforms.arena.buffer();
        let ring_binding = |bytes: usize| {
            wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: ring,
                offset: 0,
                size: std::num::NonZeroU64::new(bytes as u64),
            })
        };
        let groups = &mut self.ring_groups;
        let (vs_group, spec_vs_group) = groups.vs.get_or_insert_with(|| {
            let make = |label: &str, layout: &Option<wgpu::BindGroupLayout>| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(label),
                    layout: layout.as_ref().expect("group 2"),
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: ring_binding(SCREEN_VS_BYTES),
                    }],
                })
            };
            (
                make("terrain bake composite vs", &self.composite.layout2),
                make(
                    "terrain bake highlight composite vs",
                    &self.spec_composite.layout2,
                ),
            )
        });
        let normal_group = groups.normal.entry(size).or_insert_with(|| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("terrain bake normal"),
                layout: &self.normal.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&temps.height),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.clamp),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: ring_binding(16),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: ring_binding(16),
                    },
                ],
            })
        });
        let cache_key = (size, scene.shadow.is_some(), spec.is_some(), class);
        let lights = [&light.sun_cube, &light.shadow_cube, &light.color_correct];
        let stale = groups
            .cache
            .get(&cache_key)
            .is_none_or(|g| !g.lights.iter().zip(lights).all(|(a, b)| a == b));
        if stale {
            let views: [&wgpu::TextureView; 7] = [
                &temps.normal,
                &temps.albedo,
                if cache_key.1 {
                    &temps.shadow
                } else {
                    &self.black
                },
                if cache_key.2 {
                    &temps.spec
                } else {
                    &self.black
                },
                lights[0],
                lights[1],
                lights[2],
            ];
            let mut entries: Vec<wgpu::BindGroupEntry<'_>> = Vec::with_capacity(15);
            for (i, view) in views.iter().enumerate() {
                entries.push(wgpu::BindGroupEntry {
                    binding: i as u32 * 2,
                    resource: wgpu::BindingResource::TextureView(view),
                });
                entries.push(wgpu::BindGroupEntry {
                    binding: i as u32 * 2 + 1,
                    resource: wgpu::BindingResource::Sampler(&self.clamp),
                });
            }
            entries.push(wgpu::BindGroupEntry {
                binding: 14,
                resource: ring_binding(16),
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("terrain bake cache"),
                layout: &self.cache.layout,
                entries: &entries,
            });
            groups.cache.insert(
                cache_key,
                CacheGroup {
                    lights: lights.map(Clone::clone),
                    group,
                },
            );
        }
        let cache_group = &groups.cache[&cache_key].group;

        // The composite: the albedo and the height of the slots.
        let composite_views: [&wgpu::TextureView; 2 * SLOTS] = std::array::from_fn(|i| {
            let slot = &scene.defs[ids[i % SLOTS] as usize];
            if i < SLOTS {
                &slot.albedo
            } else {
                &slot.height
            }
        });
        let composite_group = Self::slot_group(
            device,
            &mut self.composite_groups,
            &self.composite.layout,
            "terrain bake composite",
            &self.repeat,
            ids,
            &composite_views,
        );
        // The highlight composite: the albedo of the slots (their dominance), and the specular maps
        // where the composite has the heights; a row without one is black (the original binds
        // `m_hBlankTexture` there). The decompiled `TerrainCell::BuildTexture` clears `field_38[0..2]`
        // after filling the slots; read as "no specular map in the first three slots" it would leave
        // the marsh of S1a without the highlight the capture shows, so it is not followed.
        // re: TerrainCell::BuildTexture (TerrainCell.cpp:878-913)
        let spec_group = spec.map(|_| {
            let views: [&wgpu::TextureView; 2 * SLOTS] = std::array::from_fn(|i| {
                let slot = &scene.defs[ids[i % SLOTS] as usize];
                if i < SLOTS {
                    &slot.albedo
                } else {
                    slot.spec.as_ref().unwrap_or(&self.black)
                }
            });
            Self::slot_group(
                device,
                &mut self.spec_groups,
                &self.spec_composite.layout,
                "terrain bake highlight composite",
                &self.repeat,
                ids,
                &views,
            )
        });

        let scratch_level0 = &temps.scratch_level0;
        drop(slow_groups);
        let _slow_passes = crate::perf::slow::scope("bake.passes");

        let clear = |c: [f64; 4]| {
            wgpu::LoadOp::Clear(wgpu::Color {
                r: c[0],
                g: c[1],
                b: c[2],
                a: c[3],
            })
        };
        let attachment = |view, load| wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        };
        // The vertices of the page on the GPU: one invocation a vertex, into the region of the
        // output buffer the passes below read.
        if let (Some(g), Some((params_offset, table_offset))) = (self.gpu.as_mut(), gpu_offsets) {
            let _s = crate::perf::slow::scope("bake.gpu_vertices");
            let ring = self.uniforms.arena.buffer();
            let key = (
                self.uniforms.arena.generation,
                g.inputs.generation,
                g.out.generation,
            );
            if g.group.as_ref().is_none_or(|(k, _)| *k != key) {
                let uniform = |bytes: usize| {
                    wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: ring,
                        offset: 0,
                        size: std::num::NonZeroU64::new(bytes as u64),
                    })
                };
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("terrain bake vertices"),
                    layout: &g.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: uniform(GPU_PARAMS_BYTES),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: uniform(GPU_TABLE_BYTES),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: g.inputs.buffer().as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: g.out.buffer().as_entire_binding(),
                        },
                    ],
                });
                g.group = Some((key, group));
            }
            let group = &g.group.as_ref().expect("made above").1;
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("terrain bake vertices"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&g.pipeline);
            pass.set_bind_group(0, group, &[params_offset, table_offset]);
            pass.dispatch_workgroups(
                (grid_w as u32).div_ceil(GPU_GROUP),
                (grid_h as u32).div_ceil(GPU_GROUP),
                1,
            );
        }
        let vertex_buffer = match self.gpu.as_ref().filter(|_| gpu_job.is_some()) {
            Some(g) => g.out.buffer(),
            None => self.verts.buffer(),
        };
        let grid_ib = &self.grids[&grid_key].0;
        // "HeightMap".
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("HeightMap"),
                color_attachments: &[
                    Some(attachment(&temps.albedo, clear([0.0; 4]))),
                    Some(attachment(&temps.height, clear([0.0; 4]))),
                ],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline_c(&self.composite.pipeline);
            pass.set_bind_group_c(1, composite_group, &[]);
            pass.set_bind_group_c(2, &*vs_group, &[vs_offset]);
            pass.set_vertex_buffer(0, vertex_buffer.slice(composite_range.clone()));
            pass.set_index_buffer(grid_ib.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed_c(0..index_count, 0, 0..1);
        }
        // The highlight of "HeightMap" (`SpecTemp`): no highlight where the cell does not reach.
        if let Some(group) = spec_group {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("HeightMap highlight"),
                color_attachments: &[Some(attachment(&temps.spec, clear([0.0; 4])))],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline_c(&self.spec_composite.pipeline);
            pass.set_bind_group_c(1, group, &[]);
            pass.set_bind_group_c(2, &*spec_vs_group, &[vs_offset]);
            pass.set_vertex_buffer(0, vertex_buffer.slice(composite_range));
            pass.set_index_buffer(grid_ib.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed_c(0..index_count, 0, 0..1);
        }
        // "Decal Layer": the decals of the area, over the composite.
        if let Some(bake) = &scene.decals {
            let _s = crate::perf::slow::scope("bake.decals");
            self.decals.encode(
                device,
                encoder,
                &temps.albedo,
                &temps.height,
                scene.sampler.frame(),
                rect,
                bake,
            );
        }
        // "Normal Calculation".
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Normal Calculation"),
                color_attachments: &[Some(attachment(&temps.normal, clear([0.5, 0.5, 1.0, 0.0])))],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline_c(&self.normal.pipeline);
            pass.set_bind_group_c(1, &*normal_group, &[texel_offset, scale_offset]);
            pass.set_vertex_buffer(0, self.quad.0.slice(..));
            pass.set_index_buffer(self.quad.1.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed_c(0..6, 0, 0..1);
        }
        // "Macro Shadow", then "Decal Macro Shadow" over it.
        if let Some(shadow) = &scene.shadow {
            let _s = crate::perf::slow::scope("bake.shadow");
            self.shadow
                .encode(device, encoder, &temps.shadow, size, rect, shadow);
            if let Some(bake) = &scene.decals {
                self.decals.encode_shadow(
                    device,
                    encoder,
                    &temps.shadow,
                    scene.sampler.frame(),
                    rect,
                    bake,
                );
            }
        }
        // "Terrain Cache": the page is cleared to (1, 1, 0, 0), where the cell does not reach; with the
        // highlight page both are cleared to (1, 0.5, 0, 0).
        // re: TerrainCell::BuildTexture (TerrainCell.cpp:985-995): vClearDiffuse, vClearDiffuseSpec
        {
            let page_clear = if spec.is_some() {
                [1.0, 0.5, 0.0, 0.0]
            } else {
                [1.0, 1.0, 0.0, 0.0]
            };
            let targets = [
                Some(attachment(scratch_level0, clear(page_clear))),
                spec.map(|_| attachment(&temps.spec_level0, clear(page_clear))),
            ];
            let targets = &targets[..if spec.is_some() { 2 } else { 1 }];
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Terrain Cache"),
                color_attachments: targets,
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline_c(if spec.is_some() {
                &self.cache_spec
            } else {
                &self.cache.pipeline
            });
            pass.set_bind_group_c(1, cache_group, &[half_offset]);
            pass.set_vertex_buffer(0, vertex_buffer.slice(cache_range));
            pass.set_index_buffer(grid_ib.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed_c(0..index_count, 0, 0..1);
        }
        // The mips of the page, then the whole chain into the layer of the array; the same for the
        // highlight page.
        let levels = mip_count(size);
        let mut chains = vec![(&temps.scratch, target_array, layer)];
        if let Some((spec_array, spec_layer)) = spec {
            chains.push((&temps.spec_scratch, spec_array, spec_layer));
        }
        for (scratch, array, layer) in chains {
            let _ = self.mips.generate(device, encoder, scratch, 1, levels);
            for level in 0..levels {
                let side = (size >> level).max(1);
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: scratch,
                        mip_level: level,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: array,
                        mip_level: level,
                        origin: wgpu::Origin3d {
                            x: 0,
                            y: 0,
                            z: layer,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width: side,
                        height: side,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
        Ok(())
    }

    /// One frame of the bake: at most `budget` jobs of the cache, in the order the automaton made
    /// them. A job that cannot be baked is reported and dropped; its page stays unready.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        arrays: &PageArrays,
        cache: &mut TerrainCache,
        cells: &super::cells::TerrainCells,
        scene: &BakeScene<'_>,
        budget: usize,
    ) -> BakeStats {
        let mut stats = BakeStats::default();
        for _ in 0..budget {
            let Some(job) = cache.take_job() else {
                break;
            };
            let cell = cells.cell(job.cell);
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("terrain bake"),
            });
            self.begin_frame(queue);
            let result = self.encode_pages(
                device,
                queue,
                &mut encoder,
                arrays,
                job.page,
                job.spec,
                &cell.rect,
                cell.class(),
                &cell.textures,
                scene,
            );
            if result.is_ok() {
                queue.submit([encoder.finish()]);
            }
            match result {
                Ok(()) => {
                    cache.complete(job);
                    stats.baked += 1;
                }
                Err(e) => stats.failed.push((job, e)),
            }
        }
        stats
    }
}

impl TerrainBaker {
    /// As [`Self::run`], into the commands of the frame: one encoder holds the passes of every page.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_jobs(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        arrays: &PageArrays,
        cache: &mut TerrainCache,
        cells: &super::cells::TerrainCells,
        scene: &BakeScene<'_>,
        budget: usize,
        cpu_ms: Option<f64>,
    ) -> BakeStats {
        let mut stats = BakeStats::default();
        // The commands of the frame before were submitted: its regions of the buffers are free.
        self.begin_frame(queue);
        let started = crate::perf::slow::now_ms();
        // The budget is in cells: the cache page and the near page of a cell are one (B3).
        let mut taken: Vec<usize> = Vec::new();
        // The vertices of the next jobs, made side by side on the pool (empty without workers).
        let mut ahead: Vec<((usize, u32), CellVertices)> = Vec::new();
        loop {
            let Some((next, kind)) = cache.jobs().next().map(|j| (j.cell, j.kind)) else {
                break;
            };
            // Past `cpu_ms` only the cache page of a cell with nothing to draw is baked (a hole
            // otherwise); a cell drawn from its old or its coarser page waits for the next frame.
            // A cell of the ring without a page goes too: made to wait, it came on the screen without
            // one (holes on a far pan).
            let hole = kind == PageKind::Cache && !cache.entry(next).is_drawable();
            // The same past [`FRAME_VERTEX_BYTES`] of vertices: one write of many megabytes stalled the
            // browser's queue for 50 ms now and then on a far pan.
            if !hole
                && (cpu_ms.is_some_and(|ms| crate::perf::slow::now_ms() - started >= ms)
                    || (cpu_ms.is_some() && self.pending_bytes() >= FRAME_VERTEX_BYTES))
            {
                break;
            }
            if !taken.contains(&next) {
                if taken.len() >= budget {
                    break;
                }
                taken.push(next);
            }
            let Some(job) = cache.take_job() else {
                break;
            };
            let cell = cells.cell(job.cell);
            let _s = crate::perf::slow::scope(match job.page.size {
                64 => "bake.page64",
                128 => "bake.page128",
                256 => "bake.page256",
                _ => "bake.page512",
            });
            let t0 = crate::perf::slow::now_ms();
            // The GPU makes the vertices (render-opt): nothing to make ahead on the pool.
            let prebuilt = if self.gpu_vertices() {
                None
            } else {
                Self::vertices_ahead(&mut ahead, cache, cells, scene, &job)
            };
            let result = self.record_pages(
                device,
                queue,
                encoder,
                arrays,
                job.page,
                job.spec,
                &cell.rect,
                cell.class(),
                &cell.textures,
                scene,
                prebuilt,
            );
            // The pages over 4 ms apart from the sum of the pages of a size.
            let ms = crate::perf::slow::now_ms() - t0;
            if ms > 4.0 {
                crate::perf::slow::record("bake.page.over_4ms", ms);
            }
            if ms > 20.0 {
                tracing::warn!(
                    cell = job.cell,
                    size = job.page.size,
                    spec = job.spec.is_some(),
                    ms,
                    "a page of the terrain took long to encode"
                );
            }
            match result {
                Ok(()) => {
                    cache.complete(job);
                    stats.baked += 1;
                }
                Err(e) => stats.failed.push((job, e)),
            }
        }
        // The data of every page of the frame, in one write each for the vertices and the constants.
        let _s = crate::perf::slow::scope("bake.flush");
        self.flush(queue);
        stats
    }

    /// The vertices of the page of `job` made on the pool of `jobs` (B3 render-mt), with those of the
    /// jobs after it in the queue, one per thread; kept in `ahead` for when their jobs come (the field
    /// does not change during a frame of the bake: they are what the job would make). `None` without
    /// workers: the job makes its own.
    fn vertices_ahead(
        ahead: &mut Vec<((usize, u32), CellVertices)>,
        cache: &TerrainCache,
        cells: &super::cells::TerrainCells,
        scene: &BakeScene<'_>,
        job: &BakeJob,
    ) -> Option<CellVertices> {
        let n = jobs::workers() + 1;
        if n < 2 {
            return None;
        }
        let key = (job.cell, job.page.size);
        if let Some(i) = ahead.iter().position(|(k, _)| *k == key) {
            return Some(ahead.swap_remove(i).1);
        }
        let mut keys = vec![key];
        for j in cache.jobs() {
            if keys.len() >= n {
                break;
            }
            let k = (j.cell, j.page.size);
            if !keys.contains(&k) && !ahead.iter().any(|(a, _)| *a == k) {
                keys.push(k);
            }
        }
        let (sampler, projections, kernel) = (scene.sampler, scene.projections, scene.kernel);
        let keys_ref = &keys;
        let mut made = jobs::map(keys.len(), |i| {
            let (c, size) = keys_ref[i];
            let cell = cells.cell(c);
            build_cell_vertices(
                &VertexInputs {
                    sampler,
                    projections,
                    kernel,
                    textures: &cell.textures,
                    page_size: size,
                    gpu_weights: false,
                },
                &cell.rect,
            )
        })
        .into_iter();
        let first = made.next();
        ahead.extend(keys[1..].iter().copied().zip(made));
        first
    }
}

/// Which kind of page a job bakes: both go through the same passes.
pub fn page_kind_name(kind: PageKind) -> &'static str {
    match kind {
        PageKind::Cache => "cache",
        PageKind::Near => "near",
    }
}

/// The corner of a cell on the page, in texels of the page, for a texel of the field: the cell
/// covers the page edge to edge with its `(w + 1)` texels.
pub fn texel_in_page(rect: &CellBox, size: u32, field_texel: Vec2) -> Vec2 {
    let u = (field_texel.x - rect.x as f32 + 1.0) / (rect.w as f32 + 1.0);
    let v = (field_texel.y - rect.y as f32 + 1.0) / (rect.h as f32 + 1.0);
    Vec2::new(u, v) * size as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_test::{Gpu, gpu, read_texture};
    use crate::terrain::{Relief, TerrainKind};
    use crate::world::terrain::Terrain;
    use crate::world::terrain::basis::{PlotBasis, TerrainBasis};
    use crate::world::terrain::cache::TerrainCache;
    use crate::world::terrain::cells::{TerrainCells, texture_contributions};
    use crate::world::terrain::library::TerrainLibrary;
    use crate::world::terrain::pages::PagePools;
    use std::sync::Arc;

    fn plot(relief: Relief, terrain: TerrainKind) -> PlotBasis {
        PlotBasis {
            relief,
            terrain,
            continent: 0,
            river_flow: PlotBasis::NO_FLOW,
            ..PlotBasis::default()
        }
    }

    fn built() -> Terrain {
        built_with(false)
    }

    fn built_with(wrap_x: bool) -> Terrain {
        let mut basis =
            TerrainBasis::uniform(6, 6, wrap_x, 3, plot(Relief::Flat, TerrainKind::Grass));
        for (x, y, r, k) in [
            (1, 1, Relief::Hill, TerrainKind::Grass),
            (2, 2, Relief::Mountain, TerrainKind::Plains),
            (3, 3, Relief::Water, TerrainKind::Coast),
        ] {
            let i = basis.index(x, y).unwrap();
            basis.plots[i] = plot(r, k);
        }
        let mut t = Terrain::default();
        t.set_library(Arc::new(TerrainLibrary::synthetic()));
        t.set_basis(basis);
        t.build_all();
        t
    }

    fn texture_2d(g: &Gpu, texels: &[[u8; 4]], side: u32) -> wgpu::TextureView {
        let t = g.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bake test"),
            size: wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let bytes: Vec<u8> = texels.iter().flatten().copied().collect();
        g.queue.write_texture(
            t.as_image_copy(),
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(side * 4),
                rows_per_image: Some(side),
            },
            wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: 1,
            },
        );
        t.create_view(&Default::default())
    }

    fn cube(g: &Gpu, rgb: [u8; 3]) -> wgpu::TextureView {
        let t = g.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bake test cube"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 6,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // As `LightingEnv` hands the cubes to the bake.
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let face = [rgb[0], rgb[1], rgb[2], 255];
        let bytes: Vec<u8> = face.iter().copied().cycle().take(6 * 4).collect();
        g.queue.write_texture(
            t.as_image_copy(),
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 6,
            },
        );
        t.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::Cube),
            ..Default::default()
        })
    }

    fn volume(g: &Gpu) -> wgpu::TextureView {
        let t = g.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bake test volume"),
            size: wgpu::Extent3d {
                width: 2,
                height: 2,
                depth_or_array_layers: 2,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut bytes = Vec::new();
        for z in 0..2u8 {
            for y in 0..2u8 {
                for x in 0..2u8 {
                    bytes.extend_from_slice(&[x * 255, y * 255, z * 255, 255]);
                }
            }
        }
        g.queue.write_texture(
            t.as_image_copy(),
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(8),
                rows_per_image: Some(2),
            },
            wgpu::Extent3d {
                width: 2,
                height: 2,
                depth_or_array_layers: 2,
            },
        );
        t.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D3),
            ..Default::default()
        })
    }

    /// One albedo and one height texture per row of the synthetic `TerrainDef`: a colour of its own,
    /// alpha 0.8 (the dominance), a gradient for the height.
    fn defs(g: &Gpu, rows: usize) -> Vec<DefTexture> {
        (0..rows)
            .map(|r| {
                let c = [
                    (40 + (r * 25) % 160) as u8,
                    (200 - (r * 12) % 150) as u8,
                    (90 + (r * 7) % 100) as u8,
                    204,
                ];
                let albedo: Vec<[u8; 4]> = (0..16)
                    .map(|i| {
                        let m = 0.8 + (i % 4) as f32 * 0.05;
                        [
                            (f32::from(c[0]) * m) as u8,
                            (f32::from(c[1]) * m) as u8,
                            c[2],
                            c[3],
                        ]
                    })
                    .collect();
                let height: Vec<[u8; 4]> = (0..16).map(|i| [(i * 16) as u8, 0, 0, 255]).collect();
                // The odd rows have a highlight: a grey specular map, exponent 0.5 * 64 + 1.
                let spec: Vec<[u8; 4]> = vec![[200, 200, 200, 128]; 16];
                DefTexture {
                    albedo: texture_2d(g, &albedo, 4),
                    height: texture_2d(g, &height, 4),
                    spec: (r % 2 == 1).then(|| texture_2d(g, &spec, 4)),
                }
            })
            .collect()
    }

    fn light(g: &Gpu) -> BakeLight {
        BakeLight {
            sun_cube: cube(g, [200, 190, 160]),
            shadow_cube: cube(g, [60, 70, 110]),
            color_correct: volume(g),
            half: [0.0, 0.6, 0.8],
        }
    }

    fn setup() -> Option<(Gpu, TerrainBaker)> {
        let g = gpu(
            "terrain bake",
            wgpu::Backends::all(),
            wgpu::Features::empty(),
        )?;
        let baker = TerrainBaker::new(
            &g.device,
            BakeConfig {
                float_targets: true,
                height_scale: 1.0,
            },
        );
        Some((g, baker))
    }

    fn page_bytes_of(g: &Gpu, arrays: &PageArrays, page: PageId, mip: u32) -> Vec<u8> {
        let side = (page.size >> mip).max(1);
        read_texture(
            g,
            arrays.texture(page).unwrap(),
            mip,
            arrays.layer(page),
            (side, side),
            4,
        )
    }

    #[test]
    fn the_constants_of_the_vertex_stage_hold_the_snapped_scale_and_the_axis() {
        let rows: Vec<_> = TerrainLibrary::synthetic().defs().rows().to_vec();
        let p = Projections::from_defs(&rows);
        let bytes = TerrainBaker::screen_vs_constants(&p, 2560.0);
        assert_eq!(bytes.len(), SCREEN_VS_BYTES);
        let f = |i: usize| f32::from_le_bytes(bytes[i * 4..][..4].try_into().unwrap());
        // The offsets of the half texel are zero.
        assert_eq!((f(0), f(1)), (0.0, 0.0));
        let scale = snapped_scale(p.scale_of(0), 0, 2560.0);
        assert_eq!((f(4), f(5)), (scale, 0.0));
        // A vertical texture has its axes 0, 1, 2 in consecutive entries.
        let vertical = rows.iter().position(|r| r.projection == 1);
        if let Some(t) = vertical {
            let first = p.first(t as u8) as usize;
            assert_eq!(f(4 + (first + 1) * 4 + 1), 1.0);
            assert_eq!(f(4 + (first + 2) * 4 + 1), 2.0);
        }
    }

    #[test]
    fn the_vertex_data_of_the_composite_is_six_vec4_a_vertex() {
        let t = built();
        let s = t.sampler().unwrap();
        let p = Projections::from_defs(t.library().defs().rows());
        let k = BlendKernel::new();
        let b = CellGrid::new(s.frame()).cell_box(1, 1);
        let set = texture_contributions(s, &p, &b);
        let v = build_cell_vertices(
            &VertexInputs {
                sampler: s,
                projections: &p,
                kernel: &k,
                textures: &set,
                page_size: 64,
                gpu_weights: false,
            },
            &b,
        );
        let data = TerrainBaker::composite_vertices(&v);
        assert_eq!(data.len(), v.composite.len() * 24);
        // The first vertex: the corner, the world point, the weights in 0..1, the slots.
        assert_eq!(&data[0..4], &[-1.0, 1.0, 0.5, 1.0]);
        let w: f32 = data[8..16].iter().sum();
        assert!(w > 0.9 && w <= 1.0 + 1e-6, "{w}");
        assert!(data[16..24].iter().all(|&x| x == x.trunc()));
        let cache = TerrainBaker::cache_vertices(&v);
        assert_eq!(cache.len(), v.compressed.len() * 8);
        // The place in the page of the first vertex is (0, 0), of the last (1, 1).
        assert_eq!(&cache[4..6], &[0.0, 0.0]);
        assert_eq!(&cache[cache.len() - 4..cache.len() - 2], &[1.0, 1.0]);
        let idx = TerrainBaker::grid_indices(v.width, v.height);
        assert_eq!(idx.len() as i32, (v.width - 1) * (v.height - 1) * 6);
        assert!(idx.iter().all(|&i| (i as usize) < v.composite.len()));
    }

    #[test]
    fn the_budget_follows_the_keys() {
        let s = TerrainSettings {
            pagein_still: 2,
            pagein_moving: 1,
            ..TerrainSettings::default()
        };
        assert_eq!(bake_budget(&s, false), 2);
        assert_eq!(bake_budget(&s, true), 1);
        // The launches of a frame stop at 8 whatever the key says.
        let fast = TerrainSettings {
            pagein_still: 16,
            ..s.clone()
        };
        assert_eq!(bake_budget(&fast, false), MAX_LAUNCHES_PER_FRAME);
    }

    /// `MinimizeGrayTiles` (table 4.12): the full budget of the frame, still or moving.
    #[test]
    fn minimize_gray_tiles_gives_the_full_budget() {
        let s = TerrainSettings {
            pagein_still: 2,
            pagein_moving: 1,
            minimize_gray_tiles: true,
            ..TerrainSettings::default()
        };
        assert_eq!(bake_budget(&s, false), MAX_LAUNCHES_PER_FRAME);
        assert_eq!(bake_budget(&s, true), MAX_LAUNCHES_PER_FRAME);
    }

    #[test]
    fn a_page_in_the_page_covers_the_cell_edge_to_edge() {
        let r = CellBox {
            x: 64,
            y: 0,
            w: 65,
            h: 65,
        };
        assert_eq!(texel_in_page(&r, 128, Vec2::new(63.0, -1.0)), Vec2::ZERO);
        let end = texel_in_page(&r, 128, Vec2::new(129.0, 65.0));
        assert!((end - Vec2::splat(128.0)).length() < 1e-4, "{end:?}");
        assert_eq!(page_kind_name(PageKind::Near), "near");
    }

    #[test]
    fn baking_a_cell_twice_gives_equal_pages() {
        let Some((g, mut baker)) = setup() else {
            return;
        };
        let t = built();
        let s = t.sampler().unwrap();
        let rows = t.library().defs().rows().to_vec();
        let projections = Projections::from_defs(&rows);
        let kernel = BlendKernel::new();
        let grid = CellGrid::new(s.frame());
        let mut cells = TerrainCells::new(s.frame(), t.basis().unwrap());
        cells.prune_work(s, &projections, false);
        let def_textures = defs(&g, rows.len());
        let lit = light(&g);
        let lookup = |_class: u8| Some(&lit);
        let scene = BakeScene {
            sampler: s,
            projections: &projections,
            kernel: &kernel,
            grid: &grid,
            defs: &def_textures,
            light: &lookup,
            decals: None,
            shadow: None,
        };
        let mut pools = PagePools::new([4, 4, 4, 4]);
        let first = pools.get(64).unwrap();
        let second = pools.get(64).unwrap();
        let mut arrays = PageArrays::new(4);
        arrays.ensure(&g.device, &pools);
        let i = grid.index(1, 1);
        let cell = cells.cell(i);
        assert!(!cell.textures.is_empty());
        for page in [first, second] {
            baker
                .bake_page(
                    &g.device,
                    &g.queue,
                    &arrays,
                    page,
                    &cell.rect,
                    cell.class(),
                    &cell.textures,
                    &scene,
                )
                .expect("bake");
        }
        for mip in 0..mip_count(64) {
            assert_eq!(
                page_bytes_of(&g, &arrays, first, mip),
                page_bytes_of(&g, &arrays, second, mip),
                "mip {mip}"
            );
        }
        // The page is not blank: it has the light of the cell, not the clear colour, and it is not
        // one flat colour.
        let bytes = page_bytes_of(&g, &arrays, first, 0);
        assert!(bytes.chunks(4).any(|t| t[3] == 255), "no texel was drawn");
        let first_texel = &bytes[..4];
        assert!(bytes.chunks(4).any(|t| t != first_texel), "one flat colour");
        // The last mip is a single texel: the average of the page.
        assert_eq!(
            page_bytes_of(&g, &arrays, first, mip_count(64) - 1).len(),
            4
        );
    }

    #[test]
    fn another_cell_gives_another_page() {
        let Some((g, mut baker)) = setup() else {
            return;
        };
        let t = built();
        let s = t.sampler().unwrap();
        let rows = t.library().defs().rows().to_vec();
        let projections = Projections::from_defs(&rows);
        let kernel = BlendKernel::new();
        let grid = CellGrid::new(s.frame());
        let mut cells = TerrainCells::new(s.frame(), t.basis().unwrap());
        cells.prune_work(s, &projections, false);
        let def_textures = defs(&g, rows.len());
        let lit = light(&g);
        let lookup = |_class: u8| Some(&lit);
        let scene = BakeScene {
            sampler: s,
            projections: &projections,
            kernel: &kernel,
            grid: &grid,
            defs: &def_textures,
            light: &lookup,
            decals: None,
            shadow: None,
        };
        let mut pools = PagePools::new([4, 4, 4, 4]);
        let a = pools.get(64).unwrap();
        let b = pools.get(64).unwrap();
        let mut arrays = PageArrays::new(4);
        arrays.ensure(&g.device, &pools);
        // The cell of the mountain and a cell of plain grass far from it.
        for (page, (cx, cy)) in [(a, (2, 2)), (b, (0, 4))] {
            let cell = cells.cell(grid.index(cx, cy));
            baker
                .bake_page(
                    &g.device,
                    &g.queue,
                    &arrays,
                    page,
                    &cell.rect,
                    cell.class(),
                    &cell.textures,
                    &scene,
                )
                .expect("bake");
        }
        assert_ne!(
            page_bytes_of(&g, &arrays, a, 0),
            page_bytes_of(&g, &arrays, b, 0)
        );
    }

    /// A cell with a highlight bakes its highlight page with the page: the page is the one baked without
    /// it, and the highlight page has the glint of the specular maps; rows without one give black.
    #[test]
    fn a_cell_with_a_highlight_fills_its_highlight_page() {
        let Some((g, mut baker)) = setup() else {
            return;
        };
        let t = built();
        let s = t.sampler().unwrap();
        let rows = t.library().defs().rows().to_vec();
        let projections = Projections::from_defs(&rows);
        let kernel = BlendKernel::new();
        let grid = CellGrid::new(s.frame());
        let mut cells = TerrainCells::new(s.frame(), t.basis().unwrap());
        cells.prune_work(s, &projections, false);
        let mut shiny = defs(&g, rows.len());
        let grey: Vec<[u8; 4]> = vec![[200, 200, 200, 128]; 16];
        for d in &mut shiny {
            d.spec = Some(texture_2d(&g, &grey, 4));
        }
        let mut dull = defs(&g, rows.len());
        for d in &mut dull {
            d.spec = None;
        }
        let lit = light(&g);
        let lookup = |_class: u8| Some(&lit);
        let scene = |defs| BakeScene {
            sampler: s,
            projections: &projections,
            kernel: &kernel,
            grid: &grid,
            defs,
            light: &lookup,
            decals: None,
            shadow: None,
        };
        let mut pools = PagePools::new([8, 4, 4, 4]);
        let plain = pools.get(64).unwrap();
        let page = pools.get(64).unwrap();
        let spec = pools.get(64).unwrap();
        let dull_page = pools.get(64).unwrap();
        let dull_spec = pools.get(64).unwrap();
        let mut arrays = PageArrays::new(8);
        arrays.ensure(&g.device, &pools);
        let cell = cells.cell(grid.index(1, 1));
        let bake = |baker: &mut TerrainBaker, page, spec, defs| {
            let mut encoder = g.device.create_command_encoder(&Default::default());
            baker
                .encode_pages(
                    &g.device,
                    &g.queue,
                    &mut encoder,
                    &arrays,
                    page,
                    spec,
                    &cell.rect,
                    cell.class(),
                    &cell.textures,
                    &scene(defs),
                )
                .expect("bake");
            g.queue.submit([encoder.finish()]);
        };
        bake(&mut baker, plain, None, &shiny);
        bake(&mut baker, page, Some(spec), &shiny);
        bake(&mut baker, dull_page, Some(dull_spec), &dull);
        assert_eq!(
            page_bytes_of(&g, &arrays, plain, 0),
            page_bytes_of(&g, &arrays, page, 0),
            "the page does not change with the highlight"
        );
        let glint = page_bytes_of(&g, &arrays, spec, 0);
        assert!(
            glint.chunks(4).any(|t| t[0] > 0),
            "the highlight page has a glint"
        );
        // A highlight page of another size is left out; without specular maps the highlight is black
        // wherever the cell is.
        let mut encoder = g.device.create_command_encoder(&Default::default());
        let other = pools.get(128).unwrap();
        arrays.ensure(&g.device, &pools);
        baker
            .encode_pages(
                &g.device,
                &g.queue,
                &mut encoder,
                &arrays,
                plain,
                Some(other),
                &cell.rect,
                cell.class(),
                &cell.textures,
                &scene(&dull),
            )
            .expect("a highlight page of another size is left out");
        g.queue.submit([encoder.finish()]);
        let dark = page_bytes_of(&g, &arrays, dull_spec, 0);
        assert!(dark.chunks(4).all(|t| t[..3] == [0, 0, 0]), "no glint");
    }

    #[test]
    fn the_baker_runs_the_jobs_of_the_cache_within_the_budget() {
        let Some((g, mut baker)) = setup() else {
            return;
        };
        let t = built();
        let s = t.sampler().unwrap();
        let rows = t.library().defs().rows().to_vec();
        let projections = Projections::from_defs(&rows);
        let kernel = BlendKernel::new();
        let grid = CellGrid::new(s.frame());
        let mut cells = TerrainCells::new(s.frame(), t.basis().unwrap());
        cells.prune_work(s, &projections, false);
        // All hexes open.
        cells.set_fog(&[crate::terrain::Fog::Visible; 36]);
        let def_textures = defs(&g, rows.len());
        let lit = light(&g);
        let lookup = |_class: u8| Some(&lit);
        let scene = BakeScene {
            sampler: s,
            projections: &projections,
            kernel: &kernel,
            grid: &grid,
            defs: &def_textures,
            light: &lookup,
            decals: None,
            shadow: None,
        };
        let settings = TerrainSettings {
            chunks: [64, 64, 64, 64],
            pagein_still: 3,
            pagein_moving: 1,
            ..TerrainSettings::default()
        };
        let mut cache = TerrainCache::new(cells.len(), &settings, 64, true);
        let list: Vec<_> = cells.iter().cloned().collect();
        while !cache.init_step(&list) {}
        let queued = cache.jobs().count();
        assert!(queued > 3, "{queued}");
        let mut arrays = PageArrays::new(16);
        arrays.ensure(&g.device, cache.pools());
        let stats = baker.run(
            &g.device,
            &g.queue,
            &arrays,
            &mut cache,
            &cells,
            &scene,
            bake_budget(&settings, false),
        );
        assert_eq!(stats.baked, 3);
        assert!(stats.failed.is_empty(), "{:?}", stats.failed);
        assert_eq!(cache.jobs().count(), queued - 3);
        // Run until the work is done: the progress reaches 1.
        let mut frames = 0;
        while cache.pending_work(&list) > 0 {
            baker.run(&g.device, &g.queue, &arrays, &mut cache, &cells, &scene, 3);
            frames += 1;
            assert!(frames < 100);
        }
        assert_eq!(cache.load_progress(&list), 1.0);
    }

    #[test]
    fn a_decal_changes_the_page_under_it_and_nowhere_else() {
        use crate::world::terrain::decal_pass::DecalBake;
        use crate::world::terrain::decals::{DecalLayer, DecalVertex, PASS_LAYER};
        use asset_format::{DecalDef, Extra, LogicalId};

        let Some((g, mut baker)) = setup() else {
            return;
        };
        let t = built();
        let s = t.sampler().unwrap();
        let rows = t.library().defs().rows().to_vec();
        let projections = Projections::from_defs(&rows);
        let kernel = BlendKernel::new();
        let grid = CellGrid::new(s.frame());
        let mut cells = TerrainCells::new(s.frame(), t.basis().unwrap());
        cells.prune_work(s, &projections, false);
        let def_textures = defs(&g, rows.len());
        let lit = light(&g);
        let lookup = |_class: u8| Some(&lit);
        let i = grid.index(1, 1);
        let cell = cells.cell(i);
        let rect = cell.rect;

        // A patch of red over the texels 16..32 of the cell, in both directions.
        let id = LogicalId::new("tx:test/crops.dds");
        let mut layer = DecalLayer::new();
        layer.set_types(&[DecalDef {
            kind: "Crops".into(),
            name: "Crops_01".into(),
            albedo: Some(id.clone()),
            heightmap: None,
            priority: None,
            extra: Extra::default(),
        }]);
        let corner = |dx: i32, dy: i32| {
            s.frame()
                .tex_to_world(glam::Vec2::new((rect.x + dx) as f32, (rect.y + dy) as f32))
        };
        let verts: Vec<DecalVertex> = [
            (16, 16, 0.0, 0.0),
            (32, 16, 1.0, 0.0),
            (32, 32, 1.0, 1.0),
            (16, 32, 0.0, 1.0),
        ]
        .iter()
        .map(|&(dx, dy, u, v)| DecalVertex {
            pos: corner(dx, dy),
            uv: glam::Vec2::new(u, v),
        })
        .collect();
        layer
            .create_typed(
                crate::world::decal::DecalCategory::of_name("Crops").unwrap(),
                0,
                verts,
                vec![[0, 1, 2], [0, 2, 3]],
                PASS_LAYER,
            )
            .unwrap();
        let red = texture_2d(&g, &[[255, 0, 0, 255]; 16], 4);
        let mut gpu_textures = HashMap::new();
        gpu_textures.insert(id, red);

        let plain = BakeScene {
            sampler: s,
            projections: &projections,
            kernel: &kernel,
            grid: &grid,
            defs: &def_textures,
            light: &lookup,
            decals: None,
            shadow: None,
        };
        let with = BakeScene {
            sampler: s,
            projections: &projections,
            kernel: &kernel,
            grid: &grid,
            defs: &def_textures,
            light: &lookup,
            decals: Some(DecalBake {
                layer: &layer,
                textures: &gpu_textures,
            }),
            shadow: None,
        };
        let mut pools = PagePools::new([4, 4, 4, 4]);
        let first = pools.get(64).unwrap();
        let second = pools.get(64).unwrap();
        let mut arrays = PageArrays::new(4);
        arrays.ensure(&g.device, &pools);
        for (page, scene) in [(first, &plain), (second, &with)] {
            baker
                .bake_page(
                    &g.device,
                    &g.queue,
                    &arrays,
                    page,
                    &rect,
                    cell.class(),
                    &cell.textures,
                    scene,
                )
                .expect("bake");
        }
        let a = page_bytes_of(&g, &arrays, first, 0);
        let b = page_bytes_of(&g, &arrays, second, 0);
        let at = |bytes: &[u8], x: usize, y: usize| bytes[(y * 64 + x) * 4..][..4].to_vec();
        // The page covers the cell and a texel before it: the patch is at 17..33 of 65, 64 pixels wide.
        assert_ne!(at(&a, 24, 24), at(&b, 24, 24), "the patch is on the page");
        assert_eq!(
            at(&a, 50, 50),
            at(&b, 50, 50),
            "far from the patch the page is the same"
        );
        assert_eq!(at(&a, 4, 55), at(&b, 4, 55));
        // The red decal makes the patch redder than the ground it covers.
        let (pa, pb) = (at(&a, 24, 24), at(&b, 24, 24));
        let red_share = |p: &[u8]| {
            f32::from(p[0]) / (f32::from(p[0]) + f32::from(p[1]) + f32::from(p[2]) + 1.0)
        };
        assert!(red_share(&pb) > red_share(&pa), "{pa:?} -> {pb:?}");
    }

    /// The sum of the colour of the texels of a page.
    fn light_of(bytes: &[u8]) -> u64 {
        bytes
            .chunks(4)
            .map(|t| u64::from(t[0]) + u64::from(t[1]) + u64::from(t[2]))
            .sum()
    }

    #[test]
    fn the_relief_shades_the_pages_next_to_it_and_never_lights_one() {
        use crate::world::terrain::heights_gpu::HeightTiles;
        use crate::world::terrain::shadow_pass::ShadowBake;

        let Some((g, mut baker)) = setup() else {
            return;
        };
        let t = built();
        let s = t.sampler().unwrap();
        let rows = t.library().defs().rows().to_vec();
        let projections = Projections::from_defs(&rows);
        let kernel = BlendKernel::new();
        let grid = CellGrid::new(s.frame());
        let mut cells = TerrainCells::new(s.frame(), t.basis().unwrap());
        cells.prune_work(s, &projections, false);
        let def_textures = defs(&g, rows.len());
        let lit = light(&g);
        let lookup = |_class: u8| Some(&lit);
        // The heights of every cell on the GPU.
        let mut tiles = HeightTiles::new();
        for cy in 0..grid.cells_y {
            for cx in 0..grid.cells_x {
                tiles.upload_cell(&g.device, &g.queue, s, (cx, cy), &grid.cell_box(cx, cy));
            }
        }
        let scale = s.frame().terrain_scale();
        let scene = |quality: u32, shadow: bool| BakeScene {
            sampler: s,
            projections: &projections,
            kernel: &kernel,
            grid: &grid,
            defs: &def_textures,
            light: &lookup,
            decals: None,
            shadow: shadow.then_some(ShadowBake {
                tiles: &tiles,
                sun_dir: crate::lighting::globals::SHADOW_DIR,
                quality,
                world_scales: [scale.x, scale.y, scale.z],
            }),
        };
        let mut pools = PagePools::new([4, 4, 4, 4]);
        let page = pools.get(64).unwrap();
        let mut arrays = PageArrays::new(4);
        arrays.ensure(&g.device, &pools);
        let mut bake = |cx: i32, cy: i32, scene: &BakeScene<'_>| -> Vec<u8> {
            let cell = cells.cell(grid.index(cx, cy));
            baker
                .bake_page(
                    &g.device,
                    &g.queue,
                    &arrays,
                    page,
                    &cell.rect,
                    cell.class(),
                    &cell.textures,
                    scene,
                )
                .expect("bake");
            page_bytes_of(&g, &arrays, page, 0)
        };
        let (none, full, empty) = (scene(3, false), scene(3, true), scene(0, true));
        let (mut darker, mut equal) = (0, 0);
        for cy in 0..grid.cells_y {
            for cx in 0..grid.cells_x {
                if cells.cell(grid.index(cx, cy)).textures.is_empty() {
                    continue;
                }
                let without = bake(cx, cy, &none);
                let with = bake(cx, cy, &full);
                let quiet = bake(cx, cy, &empty);
                // Quality 0 draws no pass: the page is the one without the shadow.
                assert_eq!(without, quiet, "cell {cx},{cy} at quality 0");
                let (a, b) = (light_of(&without), light_of(&with));
                assert!(b <= a, "cell {cx},{cy} got lighter: {a} -> {b}");
                if b < a {
                    darker += 1;
                } else {
                    equal += 1;
                }
            }
        }
        assert!(darker > 0, "the hill and the mountain shade some page");
        assert!(equal > 0, "the plain far from them keeps its light");
    }

    #[test]
    fn a_decal_that_casts_a_shadow_darkens_the_page_under_it() {
        use crate::world::terrain::decal_pass::DecalBake;
        use crate::world::terrain::decals::{DecalLayer, DecalVertex, PASS_SHADOW};
        use crate::world::terrain::heights_gpu::HeightTiles;
        use crate::world::terrain::shadow_pass::ShadowBake;
        use asset_format::{DecalDef, Extra, LogicalId};

        let Some((g, mut baker)) = setup() else {
            return;
        };
        let t = built();
        let s = t.sampler().unwrap();
        let rows = t.library().defs().rows().to_vec();
        let projections = Projections::from_defs(&rows);
        let kernel = BlendKernel::new();
        let grid = CellGrid::new(s.frame());
        let mut cells = TerrainCells::new(s.frame(), t.basis().unwrap());
        cells.prune_work(s, &projections, false);
        let def_textures = defs(&g, rows.len());
        let lit = light(&g);
        let lookup = |_class: u8| Some(&lit);
        let i = grid.index(1, 1);
        let cell = cells.cell(i);
        let rect = cell.rect;
        let mut tiles = HeightTiles::new();
        tiles.upload_cell(&g.device, &g.queue, s, (1, 1), &rect);

        let id = LogicalId::new("tx:test/tree_shadow.dds");
        let mut layer = DecalLayer::new();
        layer.set_types(&[DecalDef {
            kind: "Tree_Shadow".into(),
            name: "Tree_Shadow_01".into(),
            albedo: Some(id.clone()),
            heightmap: None,
            priority: None,
            extra: Extra::default(),
        }]);
        let corner = |dx: i32, dy: i32| {
            s.frame()
                .tex_to_world(glam::Vec2::new((rect.x + dx) as f32, (rect.y + dy) as f32))
        };
        let verts: Vec<DecalVertex> = [
            (16, 16, 0.0, 0.0),
            (32, 16, 1.0, 0.0),
            (32, 32, 1.0, 1.0),
            (16, 32, 0.0, 1.0),
        ]
        .iter()
        .map(|&(dx, dy, u, v)| DecalVertex {
            pos: corner(dx, dy),
            uv: glam::Vec2::new(u, v),
        })
        .collect();
        let category = crate::world::decal::DecalCategory::of_name("Tree_Shadow").unwrap();
        // A decal that only casts a shadow: the layer of the albedo does not draw it.
        layer
            .create_typed(category, 0, verts, vec![[0, 1, 2], [0, 2, 3]], PASS_SHADOW)
            .unwrap();
        let white = texture_2d(&g, &[[255, 255, 255, 255]; 16], 4);
        let mut gpu_textures = HashMap::new();
        gpu_textures.insert(id, white);
        let scale = s.frame().terrain_scale();
        let scene = |with_layer: bool| BakeScene {
            sampler: s,
            projections: &projections,
            kernel: &kernel,
            grid: &grid,
            defs: &def_textures,
            light: &lookup,
            decals: with_layer.then_some(DecalBake {
                layer: &layer,
                textures: &gpu_textures,
            }),
            // Quality 0: no macro shadow of the relief, only the decals.
            shadow: Some(ShadowBake {
                tiles: &tiles,
                sun_dir: crate::lighting::globals::SHADOW_DIR,
                quality: 0,
                world_scales: [scale.x, scale.y, scale.z],
            }),
        };
        let mut pools = PagePools::new([4, 4, 4, 4]);
        let first = pools.get(64).unwrap();
        let second = pools.get(64).unwrap();
        let mut arrays = PageArrays::new(4);
        arrays.ensure(&g.device, &pools);
        for (page, scene) in [(first, scene(false)), (second, scene(true))] {
            baker
                .bake_page(
                    &g.device,
                    &g.queue,
                    &arrays,
                    page,
                    &rect,
                    cell.class(),
                    &cell.textures,
                    &scene,
                )
                .expect("bake");
        }
        let a = page_bytes_of(&g, &arrays, first, 0);
        let b = page_bytes_of(&g, &arrays, second, 0);
        let at = |bytes: &[u8], x: usize, y: usize| bytes[(y * 64 + x) * 4..][..3].to_vec();
        let sum = |p: Vec<u8>| p.iter().map(|&v| u32::from(v)).sum::<u32>();
        assert!(
            sum(at(&b, 24, 24)) < sum(at(&a, 24, 24)),
            "{:?} -> {:?}",
            at(&a, 24, 24),
            at(&b, 24, 24)
        );
        assert_eq!(at(&a, 50, 50), at(&b, 50, 50), "far from the decal");
    }

    /// A map with hills, mountains and water on its edges (the taps there wrap and clamp).
    fn rough(wrap_x: bool) -> Terrain {
        let mut basis =
            TerrainBasis::uniform(8, 6, wrap_x, 7, plot(Relief::Flat, TerrainKind::Grass));
        for (x, y, r, k) in [
            (0, 0, Relief::Mountain, TerrainKind::Plains),
            (7, 0, Relief::Hill, TerrainKind::Grass),
            (0, 3, Relief::Hill, TerrainKind::Plains),
            (7, 3, Relief::Mountain, TerrainKind::Grass),
            (2, 2, Relief::Mountain, TerrainKind::Plains),
            (3, 2, Relief::Hill, TerrainKind::Grass),
            (4, 4, Relief::Water, TerrainKind::Coast),
            (5, 4, Relief::Water, TerrainKind::Coast),
            (6, 5, Relief::Hill, TerrainKind::Plains),
            (1, 5, Relief::Water, TerrainKind::Coast),
        ] {
            let i = basis.index(x, y).unwrap();
            basis.plots[i] = plot(r, k);
        }
        let mut t = Terrain::default();
        t.set_library(Arc::new(TerrainLibrary::synthetic()));
        t.set_basis(basis);
        t.build_all();
        t
    }

    fn read_buffer(g: &Gpu, buffer: &wgpu::Buffer, len: u64) -> Vec<u8> {
        let staging = g.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: len,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = g
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, len);
        g.queue.submit([encoder.finish()]);
        staging.map_async(wgpu::MapMode::Read, .., |r| r.expect("map"));
        g.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        staging.get_mapped_range(..).expect("range").to_vec()
    }

    /// The equality of the GPU vertices (render-opt GB): every vertex of every cell of two maps (one
    /// wrapping) that the compute pass writes is, bit for bit, the vertex of `build_cell_vertices`
    /// packed by `composite_vertices` and `cache_vertices`.
    #[test]
    fn the_gpu_vertices_are_the_cpu_vertices() {
        let Some(g) = gpu(
            "terrain bake gpu vertices",
            wgpu::Backends::all(),
            wgpu::Features::empty(),
        ) else {
            return;
        };
        if !GpuVertices::supported(&g.device) {
            return;
        }
        let config = BakeConfig {
            float_targets: true,
            height_scale: 1.0,
        };
        let mut baker = TerrainBaker::with_gpu_vertices(&g.device, config, true);
        assert!(baker.gpu_vertices());
        let mut checked = 0;
        for wrap_x in [false, true] {
            let t = rough(wrap_x);
            let s = t.sampler().unwrap();
            let rows = t.library().defs().rows().to_vec();
            let projections = Projections::from_defs(&rows);
            let kernel = BlendKernel::new();
            let grid = CellGrid::new(s.frame());
            let mut cells = TerrainCells::new(s.frame(), t.basis().unwrap());
            cells.prune_work(s, &projections, false);
            let def_textures = defs(&g, rows.len());
            let lit = light(&g);
            let lookup = |_class: u8| Some(&lit);
            let scene = BakeScene {
                sampler: s,
                projections: &projections,
                kernel: &kernel,
                grid: &grid,
                defs: &def_textures,
                light: &lookup,
                decals: None,
                shadow: None,
            };
            let mut pools = PagePools::new([4, 4, 4, 4]);
            let page = pools.get(64).unwrap();
            let mut arrays = PageArrays::new(4);
            arrays.ensure(&g.device, &pools);
            for i in 0..cells.len() {
                let cell = cells.cell(i);
                baker
                    .bake_page(
                        &g.device,
                        &g.queue,
                        &arrays,
                        page,
                        &cell.rect,
                        cell.class(),
                        &cell.textures,
                        &scene,
                    )
                    .expect("bake");
                let v = build_cell_vertices(
                    &VertexInputs {
                        sampler: s,
                        projections: &projections,
                        kernel: &kernel,
                        textures: &cell.textures,
                        page_size: 64,
                        gpu_weights: false,
                    },
                    &cell.rect,
                );
                let mut want: Vec<u32> = TerrainBaker::composite_vertices(&v)
                    .iter()
                    .map(|f| f.to_bits())
                    .collect();
                want.extend(TerrainBaker::cache_vertices(&v).iter().map(|f| f.to_bits()));
                // `bake_page` starts the frame over: the page's vertices are at the start.
                let out = baker.gpu.as_ref().unwrap().out.buffer();
                let bytes = read_buffer(&g, out, want.len() as u64 * 4);
                let got: &[u32] = bytemuck::cast_slice(&bytes);
                let composite = v.composite.len() * 24;
                let differ: Vec<(usize, u32, u32)> = got
                    .iter()
                    .zip(&want)
                    .enumerate()
                    .filter(|(_, (a, b))| a != b)
                    .map(|(k, (a, b))| (k, *a, *b))
                    .collect();
                assert!(
                    differ.is_empty(),
                    "wrap {wrap_x}, cell {i} {:?}: {} of {} words differ (composite {composite}), \
                     first {:?}",
                    cell.rect,
                    differ.len(),
                    want.len(),
                    differ
                        .iter()
                        .take(6)
                        .map(|&(k, a, b)| {
                            let (vertex, word) = if k < composite {
                                (k / 24, k % 24)
                            } else {
                                ((k - composite) / 8, 100 + (k - composite) % 8)
                            };
                            (vertex, word, f32::from_bits(a), f32::from_bits(b))
                        })
                        .collect::<Vec<_>>()
                );
                checked += 1;
            }
        }
        assert!(checked >= 2 * 12, "{checked}");
    }

    /// A page baked from the GPU vertices is the page baked from the CPU vertices, byte for byte, on
    /// every mip (with the highlight page where the cell has one).
    #[test]
    fn a_page_from_the_gpu_vertices_is_the_page_from_the_cpu_vertices() {
        let Some(g) = gpu(
            "terrain bake gpu pages",
            wgpu::Backends::all(),
            wgpu::Features::empty(),
        ) else {
            return;
        };
        if !GpuVertices::supported(&g.device) {
            return;
        }
        let config = BakeConfig {
            float_targets: true,
            height_scale: MAKE_NORMAL_HEIGHT_SCALE,
        };
        let mut cpu_baker = TerrainBaker::with_gpu_vertices(&g.device, config, false);
        let mut gpu_baker = TerrainBaker::with_gpu_vertices(&g.device, config, true);
        assert!(!cpu_baker.gpu_vertices() && gpu_baker.gpu_vertices());
        let t = rough(true);
        let s = t.sampler().unwrap();
        let rows = t.library().defs().rows().to_vec();
        let projections = Projections::from_defs(&rows);
        let kernel = BlendKernel::new();
        let grid = CellGrid::new(s.frame());
        let mut cells = TerrainCells::new(s.frame(), t.basis().unwrap());
        cells.prune_work(s, &projections, false);
        let def_textures = defs(&g, rows.len());
        let lit = light(&g);
        let lookup = |_class: u8| Some(&lit);
        let scene = BakeScene {
            sampler: s,
            projections: &projections,
            kernel: &kernel,
            grid: &grid,
            defs: &def_textures,
            light: &lookup,
            decals: None,
            shadow: None,
        };
        let mut pools = PagePools::new([4, 4, 4, 4]);
        let pages: Vec<PageId> = (0..4).map(|_| pools.get(64).unwrap()).collect();
        let mut arrays = PageArrays::new(4);
        arrays.ensure(&g.device, &pools);
        let mut drawn = 0;
        for i in 0..cells.len() {
            let cell = cells.cell(i);
            for (baker, page, spec) in [
                (&mut cpu_baker, pages[0], pages[1]),
                (&mut gpu_baker, pages[2], pages[3]),
            ] {
                let mut encoder = g
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
                baker.begin_frame(&g.queue);
                baker
                    .encode_pages(
                        &g.device,
                        &g.queue,
                        &mut encoder,
                        &arrays,
                        page,
                        Some(spec),
                        &cell.rect,
                        cell.class(),
                        &cell.textures,
                        &scene,
                    )
                    .expect("bake");
                g.queue.submit([encoder.finish()]);
            }
            for mip in 0..mip_count(64) {
                for (a, b) in [(pages[0], pages[2]), (pages[1], pages[3])] {
                    assert!(
                        page_bytes_of(&g, &arrays, a, mip) == page_bytes_of(&g, &arrays, b, mip),
                        "cell {i} {:?}, mip {mip}, highlight {}",
                        cell.rect,
                        a == pages[1]
                    );
                }
            }
            let bytes = page_bytes_of(&g, &arrays, pages[2], 0);
            drawn += usize::from(bytes.chunks(4).any(|t| t[3] == 255));
        }
        assert!(drawn > cells.len() / 2, "{drawn} of {}", cells.len());
    }
}
