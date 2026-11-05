//! The main pass of the terrain: the cells that have a baked page are
//! drawn from it. `TerrainShaderVS` makes the lattice of a cell from the tile of heights
//! (`cell_vs.wgsl`) and `TerrainRenderCachedDiffusePS` reads the page and puts the shadow of the
//! objects, the screen overlay, the fog of war and the fog of the distance over it (`screen_ps.wgsl`).
//!
//! One draw a cell: the pixel stage samples the page as a 2D texture, as the original's does, so the
//! layer of the array is bound as a view, not given by an instance (spec 4.8 draws the cells of an
//! array in one call; that needs a variant of the pixel stage that reads an array, which `shader-re`
//! would not compare). The lattices are shared by size. A cell of a map copy (a wrapping map shows up to
//! three) is drawn once a copy: the move of the copy is in `World`.
//!
//! Until `world-overlays` fills `FowMask`, `FowBase` and `ScreenOverlay`, the end of the chain reads
//! what this file makes of B1's fog ([`FogBridge`]): the mask of the hexes the fog knows, one colour for
//! the cells nobody has seen, and an empty overlay ("до `FowMask` ... маску тумана заполняет
//! переходник B1").
//!
//! Group 0 is the frame and the light of the style (`frame_globals`, the shadow map, the colour
//! volumes of the shadow and of the fog of war, the ramp of the distance fog), group 1 the cell (the
//! page and the maps of the screen), group 2 the tile of heights and the constants of the vertex stage.

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack, Tex};
use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3, Vec4};

use super::bake::BakeLight;
use super::cells::CellBox;
use super::heights_gpu::{HeightTiles, TILE_SIDE, tile_origin};
use super::m1::env_class;
use super::pages::{PageArrays, PageId};
use super::sampler::TerrainFrame;
use crate::frame::mask::{MaskPipelineDesc, MaskShader, class};
use crate::frame::{DrawContext, MaskContext, multisample};
use crate::lighting::{LightingEnv, STYLES as ENV_STYLES, row_major};
use crate::shaders::{catalog, create_module};
use crate::terrain::Fog;

/// Bytes a slot of the constants of the vertex stage takes: the offset of a dynamic binding.
pub const VS_SLOT_BYTES: u64 = 256;
/// Slots of the constants: a tile a copy. A Huge map has 60 tiles and three copies at most.
pub const MAX_VS_SLOTS: usize = 192;
/// Texels of the field a texel of the fog mask covers.
pub const MASK_DIV: u32 = 16;
/// Colour classes of the styles of B1's art table (Generic, America, Asia, Africa, Europe).
pub const ART_CLASSES: usize = 5;

/// What the draw needs to make its pipelines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveConfig {
    /// The format of the main pass, and its depth.
    pub target: wgpu::TextureFormat,
    pub depth: wgpu::TextureFormat,
    /// Samples of the main pass.
    pub samples: u32,
    /// The device renders to float targets (the temporary maps of the bake).
    pub float_targets: bool,
}

/// The light of one continent style: what the bake and the end of the chain read.
#[derive(Clone)]
pub struct ClassLight {
    pub bake: BakeLight,
    pub shadow_color: wgpu::TextureView,
    pub fow_color: wgpu::TextureView,
}

/// The light of every style, taken from the environment, and the ramp of the fog of the distance.
#[derive(Clone)]
pub struct LiveLights {
    /// By the class of the art table (Generic .. Europe); the generic one stands for a style that
    /// has no light of its own.
    pub classes: [Option<ClassLight>; ART_CLASSES],
    pub ramp: wgpu::TextureView,
    /// `specscreenmap`: the mask of the highlight over the screen (`SpecMaskCached`); white when the
    /// environment has none.
    pub spec_mask: Option<wgpu::TextureView>,
}

/// `TerrainSystem::m_vReflectDir` of the original: the direction of the light the highlight mirrors,
/// as it travels (yaw 80, pitch 135: from the north, 45 degrees up). In the basis of the original.
// re: TerrainSystem::TerrainSystem (TerrainSystem.cpp:205-209): m_fReflectYaw = 80, m_fReflectPitch = 135,
// m_vReflectDir = (-0.12278783, -0.6963642, -0.70710677)
pub const REFLECT_DIR: [f32; 3] = [-0.122_787_83, -0.696_364_2, -0.707_106_77];

/// The half vector of the highlight that the bake uses (`g_Half`, register c5 of the cache shader): the
/// original sends `normalize(camera z axis + m_vReflectDir)` (`Terrain::SetGlobals`,
/// `m_vAxisZPlusReflectDir`); with the camera axis pointing into the scene that is minus the half vector
/// between the eye and the light, which is what the shader's `dot(g_Half, n)` needs. The camera is the
/// usual one of the game, looking north 35 degrees from the vertical (the bake of the pages is before the
/// frame of every capture, so the camera of the bake is not known).
// re: Terrain::SetGlobals (Terrain.cpp:833-841)
// SIMPLIFIED(SIMP-063): a fixed camera stands for the camera of the bake
pub fn half_vector() -> [f32; 3] {
    let to_light = -Vec3::from_array(REFLECT_DIR);
    let to_eye = Vec3::new(0.0, -0.5736, 0.8192);
    (to_light + to_eye).normalize().to_array()
}

impl LiveLights {
    /// `None` until the environment is complete.
    pub fn from_env(env: &LightingEnv) -> Option<Self> {
        if !env.ready() {
            return None;
        }
        let ramp = env.fog_ramp()?.clone();
        let make = |art: usize| -> Option<ClassLight> {
            let class = env_class(art);
            Some(ClassLight {
                bake: BakeLight {
                    // sRGB views: the bake reads the cubes decoded, as the D3D11 build binds them.
                    sun_cube: env.terrain_sunlight_srgb(class)?.clone(),
                    shadow_cube: env
                        .terrain_shadowlight_srgb(class, true)
                        .or_else(|| env.terrain_shadowlight_srgb(class, false))?
                        .clone(),
                    color_correct: env.terrain_color_key(0, class)?.clone(),
                    half: half_vector(),
                },
                shadow_color: env.terrain_color_key(1, class)?.clone(),
                fow_color: env.terrain_color_key(2, class)?.clone(),
            })
        };
        let classes: [Option<ClassLight>; ART_CLASSES] = std::array::from_fn(make);
        classes[0].as_ref()?;
        Some(Self {
            classes,
            ramp,
            spec_mask: env.spec_screen_map().cloned(),
        })
    }

    /// The light of a class; the generic one for a class that has none.
    pub fn class(&self, art: usize) -> Option<&ClassLight> {
        self.classes
            .get(art)
            .and_then(Option::as_ref)
            .or(self.classes[0].as_ref())
    }
}

/// The names of the styles are those of the light files; the class of the art table maps onto them
/// by [`env_class`].
pub fn env_style_name(art: usize) -> &'static str {
    ENV_STYLES[env_class(art)]
}

// ---- the constants ----

/// The constants of the vertex stage of a tile and a copy: the layout of `CellVsConsts`
/// (`cell_vs.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct CellVsData {
    pub mtx_view_proj: [[f32; 4]; 4],
    pub mtx_composite_vp: [[f32; 4]; 4],
    pub world: [[f32; 4]; 4],
    pub fow_base_scale: [f32; 4],
    pub height_texture_scale: [f32; 2],
    pub _pad0: [f32; 2],
    pub world_scales: [f32; 3],
    pub _pad1: f32,
}

/// The constants of the pixel stage: `TerrainConsts` of `screen_ps.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct CellPsData {
    pub cam_pos: [f32; 3],
    pub apply_diffuse: f32,
    pub apply_specular: f32,
    pub _pad: [f32; 3],
}

/// Our `ours_to_orig` frame of a map, as the vertex stage needs it: the plain matrices of the
/// constants, with the copy's shift.
pub fn vs_data(
    frame: &TerrainFrame,
    view_proj_orig: &Mat4,
    shift: f32,
    tile_scale: f32,
) -> CellVsData {
    let scale = frame.terrain_scale();
    let half = frame.half_world();
    let (tw, th) = frame.texel_size();
    let (world_w, world_h) = (tw as f32 * scale.x, th as f32 * scale.y);
    // The world to the place on a map that covers the whole of the field: x 0..1 east, y 0..1
    // north (`o3.y` flips the clip y).
    let composite = Mat4::from_cols_array(&[
        2.0 / world_w,
        0.0,
        0.0,
        0.0,
        0.0,
        -2.0 / world_h,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ]);
    CellVsData {
        mtx_view_proj: row_major(*view_proj_orig),
        mtx_composite_vp: row_major(composite),
        world: row_major(Mat4::from_translation(Vec3::new(
            shift - half.x,
            -half.y,
            0.0,
        ))),
        fow_base_scale: [1.0 / world_w, 1.0 / world_h, half.x, half.y],
        height_texture_scale: [tile_scale, tile_scale],
        _pad0: [0.0; 2],
        world_scales: [scale.x, scale.y, scale.z],
        _pad1: 0.0,
    }
}

/// The six planes of a view (left, right, bottom, top, near, far), normalised, pointing in. The
/// matrix is a plain one with clip depth 0 to 1.
pub fn frustum_planes(m: &Mat4) -> [Vec4; 6] {
    let (r0, r1, r2, r3) = (m.row(0), m.row(1), m.row(2), m.row(3));
    let raw = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2];
    raw.map(|p| {
        let len = p.truncate().length();
        if len > 0.0 { p / len } else { p }
    })
}

// ---- the lattice of a cell ----

/// A vertex of the lattice: the texel inside the cell, the normal and the place in the page.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct LatticeVertex {
    pub pos: [f32; 4],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

/// The columns (or rows) of a lattice of `n` texels with the step `step`: 0, step, 2 step ... and the
/// last texel.
pub fn lattice_axis(n: i32, step: i32) -> Vec<i32> {
    let step = step.max(1);
    let mut v: Vec<i32> = (0..n.max(1)).step_by(step as usize).collect();
    if v.last() != Some(&(n - 1)) && n > 0 {
        v.push(n - 1);
    }
    v
}

/// The vertices and the triangles of the lattice of a cell of `w` x `h` texels. The place in the
/// page of the texel `(x, y)` is `((x + 1) / (w + 1), (y + 1) / (h + 1))`: the page covers the cell
/// and a border of one texel (`bake.rs`).
pub fn lattice(w: i32, h: i32, step: i32) -> (Vec<LatticeVertex>, Vec<u32>) {
    let xs = lattice_axis(w, step);
    let ys = lattice_axis(h, step);
    let (fw, fh) = ((w + 1) as f32, (h + 1) as f32);
    let mut vertices = Vec::with_capacity(xs.len() * ys.len());
    for &y in &ys {
        for &x in &xs {
            vertices.push(LatticeVertex {
                pos: [x as f32, y as f32, 0.0, 1.0],
                normal: [0.0, 0.0, 1.0],
                uv: [(x + 1) as f32 / fw, (y + 1) as f32 / fh],
            });
        }
    }
    let cols = xs.len() as u32;
    let mut indices = Vec::new();
    for j in 0..ys.len().saturating_sub(1) as u32 {
        for i in 0..cols.saturating_sub(1) {
            let a = j * cols + i;
            let (b, c, d) = (a + 1, a + cols, a + cols + 1);
            // Counter-clockwise seen from above (x east, y north).
            indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }
    (vertices, indices)
}

struct Lattice {
    vertices: Buf,
    indices: Buf,
    index_count: u32,
}

impl Lattice {
    fn new(device: &wgpu::Device, w: i32, h: i32, step: i32) -> Self {
        let (v, i) = lattice(w, h, step);
        let buffer = |label: &str, usage, bytes: &[u8]| {
            let b = device.buf(
                &wgpu::BufferDescriptor {
                    label: Some(label),
                    size: (bytes.len() as u64).next_multiple_of(4).max(4),
                    usage,
                    mapped_at_creation: true,
                },
                Cluster::TerrainTex,
            );
            if let Ok(mut m) = b.get_mapped_range_mut(..) {
                let mut padded = bytes.to_vec();
                padded.resize(m.len(), 0);
                m.copy_from_slice(&padded);
            }
            b.unmap();
            b
        };
        Self {
            vertices: buffer(
                "terrain lattice vertices",
                wgpu::BufferUsages::VERTEX,
                bytemuck::cast_slice(&v),
            ),
            indices: buffer(
                "terrain lattice indices",
                wgpu::BufferUsages::INDEX,
                bytemuck::cast_slice(&i),
            ),
            index_count: i.len() as u32,
        }
    }
}

// ---- the maps of the screen, from B1's fog ----

/// The fog mask: `x` the hex is known, `z` it is seen now (`FOWMaskCached`), one texel for
/// [`MASK_DIV`] texels of the field, row `j` is the row `j * MASK_DIV` of the field. Hexes the fog does
/// not list are not known.
pub fn fog_mask_bytes(frame: &TerrainFrame, fog: &[Fog], size: (u32, u32)) -> Vec<u8> {
    let shape = frame.grid_shape();
    let width = shape.width.max(1) as usize;
    let mut out = Vec::with_capacity((size.0 * size.1 * 4) as usize);
    for j in 0..size.1 {
        for i in 0..size.0 {
            let tex = Vec2::new(
                (i as f32 + 0.5) * MASK_DIV as f32,
                (j as f32 + 0.5) * MASK_DIV as f32,
            );
            let (hx, hy) = frame.tex_to_hex(tex);
            let state = shape
                .normalize(hx, hy)
                .and_then(|(x, y)| fog.get(y as usize * width + x as usize))
                .copied()
                .unwrap_or(Fog::Unrevealed);
            let (known, seen) = match state {
                Fog::Unrevealed => (0, 0),
                Fog::Hidden => (255, 0),
                Fog::Visible => (255, 255),
            };
            out.extend_from_slice(&[known, 0, seen, 0]);
        }
    }
    out
}

/// The size of the fog mask of a map.
pub fn fog_mask_size(frame: &TerrainFrame) -> (u32, u32) {
    let (w, h) = frame.texel_size();
    (w.div_ceil(MASK_DIV).max(1), h.div_ceil(MASK_DIV).max(1))
}

/// The textures the pixel stage reads besides the page.
pub struct FogBridge {
    mask: Tex,
    pub mask_view: wgpu::TextureView,
    pub base_view: wgpu::TextureView,
    pub overlay_view: wgpu::TextureView,
    pub spec_black: wgpu::TextureView,
    /// `spec_black` as an array of one layer, for the batched draw.
    pub spec_black_array: wgpu::TextureView,
    pub spec_white: wgpu::TextureView,
    size: (u32, u32),
}

fn small_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    size: (u32, u32),
    data: &[u8],
    usage: wgpu::TextureUsages,
) -> Tex {
    let t = device.tex(
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: usage | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        Cluster::TerrainTex,
    );
    queue.write_texture(
        t.as_image_copy(),
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size.0 * 4),
            rows_per_image: Some(size.1),
        },
        wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
    );
    t
}

impl FogBridge {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, size: (u32, u32)) -> Self {
        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let none = wgpu::TextureUsages::empty();
        let mask = small_texture(
            device,
            queue,
            "terrain fog mask",
            size,
            &vec![0u8; (size.0 * size.1 * 4) as usize],
            wgpu::TextureUsages::COPY_SRC,
        );
        // The colour of the cells nobody has seen: B1's flat colour, as the gamma of the page.
        let lin = crate::blend::srgb_to_linear(crate::terrain::fog::FOG_UNREVEALED_RGB);
        let g = |c: f32| (c.powf(1.0 / 2.2) * 255.0).round() as u8;
        let base = small_texture(
            device,
            queue,
            "terrain fog base",
            (1, 1),
            &[g(lin[0]), g(lin[1]), g(lin[2]), 255],
            none,
        );
        let overlay = small_texture(device, queue, "terrain overlay", (1, 1), &[0; 4], none);
        let black = small_texture(
            device,
            queue,
            "terrain spec page",
            (1, 1),
            &[0, 0, 0, 0],
            none,
        );
        let white = small_texture(device, queue, "terrain spec mask", (1, 1), &[255; 4], none);
        Self {
            mask_view: view(&mask),
            mask,
            base_view: view(&base),
            overlay_view: view(&overlay),
            spec_black: view(&black),
            spec_black_array: black.create_view(&wgpu::TextureViewDescriptor {
                label: Some("terrain spec page array"),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            }),
            spec_white: view(&white),
            size,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The mask texture, for the tests.
    pub fn mask_texture(&self) -> &wgpu::Texture {
        &self.mask
    }

    pub fn write_mask(&self, queue: &wgpu::Queue, bytes: &[u8]) {
        queue.write_texture(
            self.mask.as_image_copy(),
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.size.0 * 4),
                rows_per_image: Some(self.size.1),
            },
            wgpu::Extent3d {
                width: self.size.0,
                height: self.size.1,
                depth_or_array_layers: 1,
            },
        );
    }
}

// ---- the drawer ----

/// A cell to draw this frame.
#[derive(Clone, Copy, Debug)]
pub struct DrawCell {
    /// The copy of the map (the index into the shifts).
    pub copy: usize,
    pub cell: usize,
    pub rect: CellBox,
    pub page: PageId,
    /// The highlight page of the cell, when it has one: then the cell draws with
    /// `TerrainRenderCachedDiffuseAndSpecPS`.
    pub spec: Option<PageId>,
    /// The class of the art table the light is taken from.
    pub art: usize,
}

/// What one frame's draws read.
pub struct FrameDraw<'a> {
    pub frame: &'a TerrainFrame,
    pub view_proj_orig: Mat4,
    pub eye_orig: Vec3,
    /// X shifts of the copies of the map in our basis (the middle one is 0).
    pub shifts: &'a [f32],
    pub frame_globals: &'a wgpu::Buffer,
    pub shadow_map: &'a wgpu::TextureView,
    pub lights: &'a LiveLights,
    pub arrays: &'a PageArrays,
    pub tiles: &'a HeightTiles,
    pub cells: &'a [DrawCell],
    /// The step of the lattice (`TerrainSettings::lattice_step`).
    pub lattice_step: i32,
    /// A new map or new fog textures: the groups are made again.
    pub generation: u64,
    /// The fluid fog's own maps, when it draws: then they replace [`FogBridge`]'s.
    pub fow: Option<CellFow<'a>>,
}

/// What the original's `TerrainRenderCachedDiffusePS` reads of the fluid fog (S1a ev 819): the mask
/// `FowMask` (t4, through the mask's projection `mtxCompositeVP`, cb1[4..7]) and the cloud texture
/// of the lowest layer as `FOWBaseCached` (t5 is the same resource as the first `FOW::BasePS` draw's
/// t0, through `g_FOWBaseScale`, cb1[9]).
#[derive(Clone, Copy)]
pub struct CellFow<'a> {
    pub mask: &'a wgpu::TextureView,
    pub base: &'a wgpu::TextureView,
    /// The map of the overlay (`TerrainOverlay`, t1: the grid, the borders, the highlights), when
    /// the overlay draws it this frame; it is seen through the same projection.
    pub overlay: Option<&'a wgpu::TextureView>,
    /// The world (original's basis) to the mask's clip space ([`crate::world::fow::mask::MaskView`]).
    pub projection: Mat4,
    /// `g_FOWBaseScale` of the lowest cloud layer.
    pub base_scale: [f32; 4],
    /// Changes when the views do (the groups are made again).
    pub key: (usize, usize, usize),
}

/// The resources the groups of the style classes bind.
#[derive(PartialEq)]
struct ClassKey {
    shadow_map: wgpu::TextureView,
    globals: wgpu::Buffer,
    ramp: wgpu::TextureView,
    classes: [Option<(wgpu::TextureView, wgpu::TextureView)>; ART_CLASSES],
}

/// An array of pages: the size of its pages and its index among the arrays of that size.
type ArrayRef = (u32, u32);

/// What group 1 of a draw binds: the page of the cell (one draw a cell), or the arrays of the pages
/// (every cell of those arrays in one draw).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum GroupKey {
    Page(PageId, Option<PageId>),
    Array(ArrayRef, Option<ArrayRef>),
}

#[derive(Clone, Copy)]
struct Draw {
    art: usize,
    spec: bool,
    group: GroupKey,
    /// The order of the group in the sort: the size and layer of the page (the size and array of
    /// the arrays), and of the highlight page.
    order: [u32; 4],
    tile: (i32, i32),
    slot: u32,
    /// Width, height and step of the lattice.
    lattice: (i32, i32, i32),
    /// The origin of the cell and the move into the tile (instance `location(3)`).
    offset: [f32; 4],
    /// The layers of the page and of the highlight page (instance `location(4)`, batched draw).
    layers: [u32; 2],
}

/// Cells next to each other in the instances that draw with the same state: one draw call.
#[derive(Clone, Copy)]
struct Batch {
    first: u32,
    count: u32,
    art: usize,
    spec: bool,
    group: GroupKey,
    tile: (i32, i32),
    slot: u32,
    lattice: (i32, i32, i32),
}

/// Bytes of the layers of an instance (`Uint32x2`).
const LAYER_STRIDE: u64 = 8;

/// The pipelines, the lattices and the groups of the draw.
pub struct CellDrawer {
    layout0: wgpu::BindGroupLayout,
    layout1: wgpu::BindGroupLayout,
    layout2: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    /// The cells with a highlight page: `TerrainRenderCachedDiffuseAndSpecPS`.
    spec_pipeline: wgpu::RenderPipeline,
    mask_pipeline: wgpu::RenderPipeline,
    page_sampler: wgpu::Sampler,
    mask_sampler: wgpu::Sampler,
    shadow_sampler: wgpu::Sampler,
    clamp_sampler: wgpu::Sampler,
    nearest: wgpu::Sampler,
    lattices: HashMap<(i32, i32, i32), Lattice>,
    vs_buffer: Buf,
    instances: Buf,
    instance_capacity: u64,
    ps_consts: Buf,
    class_groups: [Option<wgpu::BindGroup>; ART_CLASSES],
    /// What `class_groups` were made of, to make them again only when it changes.
    class_key: Option<ClassKey>,
    /// The pipelines of the batched draw (the pages read from their arrays), and their group 1.
    layout1_array: wgpu::BindGroupLayout,
    array_pipeline: Option<wgpu::RenderPipeline>,
    array_spec_pipeline: Option<wgpu::RenderPipeline>,
    /// Cells drawn instanced from the arrays of the pages: not on GL, whose pages are plain 2D
    /// textures (`pages::layers_per_array_for`).
    batched: bool,
    /// The layers of the instances (vertex buffer 2 of the batched draw).
    layers: Buf,
    layer_capacity: u64,
    /// Group 1 of the cells, by [`GroupKey`].
    page_groups: HashMap<GroupKey, wgpu::BindGroup>,
    /// The group of each tile of heights, with the texture it binds: a tile made again (a new live map)
    /// gets a new group even when the generation did not say so.
    tile_groups: HashMap<(i32, i32), (wgpu::Texture, wgpu::BindGroup)>,
    generation: u64,
    /// [`CellFow::key`] the page groups were made with.
    fow_key: Option<(usize, usize, usize)>,
    /// The draws of the main pass of the frame, by copy.
    batches: Vec<Vec<Batch>>,
    /// The draws of the lattices only (the class mask, `draw_geometry`), by copy: cells of one tile
    /// and one lattice together.
    geometry: Vec<Vec<Batch>>,
    /// Cells of each copy.
    cells: Vec<usize>,
    shifts: Vec<f32>,
    /// Cells of the last frame (`?stats=1`, tests).
    last_draws: usize,
    /// Draw calls of the main pass of the last frame, over all the copies.
    last_batches: usize,
}

fn texture_entry(
    binding: u32,
    dim: wgpu::TextureViewDimension,
    stages: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: stages,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: dim,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32, stages: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: stages,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn uniform_entry(
    binding: u32,
    stages: wgpu::ShaderStages,
    dynamic: bool,
    size: u64,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: stages,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: dynamic,
            min_binding_size: wgpu::BufferSize::new(size),
        },
        count: None,
    }
}

impl CellDrawer {
    pub fn new(device: &wgpu::Device, cfg: &LiveConfig) -> Self {
        use wgpu::TextureViewDimension::{D2, D3};
        let frag = wgpu::ShaderStages::FRAGMENT;
        let vert = wgpu::ShaderStages::VERTEX;
        // Group 0: the frame and the light of the style.
        let mut e0 = vec![uniform_entry(
            1,
            wgpu::ShaderStages::VERTEX_FRAGMENT,
            false,
            std::mem::size_of::<crate::lighting::FrameGlobals>() as u64,
        )];
        for (binding, dim) in [(8, D2), (10, D3), (12, D3), (14, D2)] {
            e0.push(texture_entry(binding, dim, frag));
            e0.push(sampler_entry(binding + 1, frag));
        }
        let layout0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain cell frame"),
            entries: &e0,
        });
        // Group 1: the page and the maps of the screen.
        // The batched variant reads the page (0) and the highlight page (8) from their arrays.
        let page_layout = |label: &str, pages: wgpu::TextureViewDimension| {
            let mut e1 = Vec::new();
            for k in 0..6 {
                let dim = if k == 0 || k == 4 { pages } else { D2 };
                e1.push(texture_entry(k * 2, dim, frag));
                e1.push(sampler_entry(k * 2 + 1, frag));
            }
            e1.push(uniform_entry(
                12,
                frag,
                false,
                std::mem::size_of::<CellPsData>() as u64,
            ));
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries: &e1,
            })
        };
        let layout1 = page_layout("terrain cell page", D2);
        let layout1_array = page_layout(
            "terrain cell page array",
            wgpu::TextureViewDimension::D2Array,
        );
        // Group 2: the tile of heights and the constants of the vertex stage.
        let layout2 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain cell tile"),
            entries: &[
                texture_entry(0, D2, vert),
                sampler_entry(1, vert),
                uniform_entry(2, vert, true, std::mem::size_of::<CellVsData>() as u64),
            ],
        });

        let module = catalog()
            .iter()
            .find(|m| m.name == "terrain_cell")
            .expect("the terrain_cell module is in the catalog");
        let shader = create_module(device, "terrain cell", &module.source());
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain cell"),
            bind_group_layouts: &[Some(&layout0), Some(&layout1), Some(&layout2)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x3, 2 => Float32x2
        ];
        let instance_attributes = wgpu::vertex_attr_array![3 => Float32x4];
        let buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<LatticeVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attributes,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: 16,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &instance_attributes,
            }),
        ];
        let primitive = wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        };
        let layer_attributes = wgpu::vertex_attr_array![4 => Uint32x2];
        let array_buffers = [
            buffers[0].clone(),
            buffers[1].clone(),
            Some(wgpu::VertexBufferLayout {
                array_stride: LAYER_STRIDE,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &layer_attributes,
            }),
        ];
        let batched = device.adapter_info().backend != wgpu::Backend::Gl;
        let array_module = catalog()
            .iter()
            .find(|m| m.name == "terrain_cell_array")
            .expect("the terrain_cell_array module is in the catalog");
        // GL draws a cell at a time and never makes the batched pipelines (its pages are plain 2D
        // textures); the module is only compiled where it is used.
        let array_shader =
            batched.then(|| create_module(device, "terrain cell array", &array_module.source()));
        let array_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain cell array"),
            bind_group_layouts: &[Some(&layout0), Some(&layout1_array), Some(&layout2)],
            immediate_size: 0,
        });
        let make_with = |label: &str,
                         layout: &wgpu::PipelineLayout,
                         shader: &wgpu::ShaderModule,
                         vs: &str,
                         buffers: &[Option<wgpu::VertexBufferLayout<'_>>],
                         entry: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers,
                },
                primitive,
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: cfg.depth,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: multisample(cfg.samples),
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: cfg.target,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let make = |label: &str, entry: &str| {
            make_with(
                label,
                &layout,
                &shader,
                "terrain_shader_vs",
                &buffers,
                entry,
            )
        };
        let pipeline = make("terrain cell", "terrain_render_cached_diffuse_ps");
        let spec_pipeline = make(
            "terrain cell with highlight",
            "terrain_render_cached_diffuse_and_spec_ps",
        );
        let make_array = |label: &str, entry: &str| {
            array_shader.as_ref().map(|s| {
                make_with(
                    label,
                    &array_layout,
                    s,
                    "terrain_shader_vs_array",
                    &array_buffers,
                    entry,
                )
            })
        };
        let array_pipeline = make_array(
            "terrain cells batched",
            "terrain_render_cached_diffuse_array_ps",
        );
        let array_spec_pipeline = make_array(
            "terrain cells batched with highlight",
            "terrain_render_cached_diffuse_and_spec_array_ps",
        );
        // The class mask draws the same cells with the same vertex stage.
        let mask_layouts = [None, None, Some(&layout2)];
        let mask_pipeline = MaskShader::new(device).pipeline(
            device,
            &MaskPipelineDesc {
                label: "terrain cell mask",
                vertex: &shader,
                vertex_entry: "terrain_shader_vs",
                vertex_buffers: &buffers,
                bind_group_layouts: &mask_layouts,
                cull: Some(wgpu::Face::Back),
                topology: wgpu::PrimitiveTopology::TriangleList,
                class: class::LAND,
            },
        );

        let sampler = |label: &str, filter, mip: wgpu::MipmapFilterMode, u, v| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: u,
                address_mode_v: v,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: filter,
                min_filter: filter,
                mipmap_filter: mip,
                ..Default::default()
            })
        };
        use wgpu::AddressMode::{ClampToEdge as Clamp, Repeat};
        use wgpu::FilterMode::{Linear, Nearest};
        use wgpu::MipmapFilterMode as Mip;
        let buffer = |label: &str, size: u64, usage| {
            device.buf(
                &wgpu::BufferDescriptor {
                    label: Some(label),
                    size,
                    usage,
                    mapped_at_creation: false,
                },
                Cluster::TerrainTex,
            )
        };
        let instance_capacity = 1024 * 16;
        Self {
            layout0,
            layout1,
            layout2,
            pipeline,
            spec_pipeline,
            mask_pipeline,
            page_sampler: sampler("terrain page", Linear, Mip::Linear, Clamp, Clamp),
            // The mask repeats along X: a copy of a wrapping map is a unit to the side.
            mask_sampler: sampler("terrain fog mask", Linear, Mip::Nearest, Repeat, Clamp),
            // The coordinates of the shadow map are negative.
            shadow_sampler: sampler("terrain shadow", Linear, Mip::Linear, Repeat, Repeat),
            clamp_sampler: sampler("terrain clamp", Linear, Mip::Linear, Clamp, Clamp),
            nearest: sampler("terrain tile", Nearest, Mip::Nearest, Clamp, Clamp),
            lattices: HashMap::new(),
            vs_buffer: buffer(
                "terrain cell vs",
                VS_SLOT_BYTES * MAX_VS_SLOTS as u64,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            instances: buffer(
                "terrain cell instances",
                instance_capacity,
                wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            ),
            instance_capacity,
            ps_consts: buffer(
                "terrain cell ps",
                std::mem::size_of::<CellPsData>() as u64,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            class_groups: Default::default(),
            class_key: None,
            page_groups: HashMap::new(),
            fow_key: None,
            tile_groups: HashMap::new(),
            generation: u64::MAX,
            layout1_array,
            array_pipeline,
            array_spec_pipeline,
            batched,
            layers: buffer(
                "terrain cell layers",
                instance_capacity / 2,
                wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            ),
            layer_capacity: instance_capacity / 2,
            batches: Vec::new(),
            geometry: Vec::new(),
            cells: Vec::new(),
            shifts: Vec::new(),
            last_draws: 0,
            last_batches: 0,
        }
    }

    /// Cells drawn in the last frame, over all the copies.
    pub fn last_draws(&self) -> usize {
        self.last_draws
    }

    /// Draw calls of the main pass in the last frame, over all the copies: one a cell when the
    /// cells are not batched.
    pub fn last_batches(&self) -> usize {
        self.last_batches
    }

    /// Whether the cells are drawn instanced from the arrays of the pages.
    pub fn batched(&self) -> bool {
        self.batched
    }

    /// Draws a cell at a time, the page bound as a 2D view, as on GL (tests compare the two). Takes
    /// effect at the next `prepare`; a device without the batched pipelines stays per cell.
    pub fn set_batched(&mut self, on: bool) {
        let on = on && self.array_pipeline.is_some();
        if on != self.batched {
            self.batched = on;
            self.page_groups.clear();
        }
    }

    /// The texture of heights the group of a tile binds (tests).
    #[cfg(test)]
    pub fn tile_group_texture(&self, tile: (i32, i32)) -> Option<&wgpu::Texture> {
        self.tile_groups.get(&tile).map(|(t, _)| t)
    }

    /// The tiles that have a group (tests).
    #[cfg(test)]
    pub fn tile_group_keys(&self) -> Vec<(i32, i32)> {
        self.tile_groups.keys().copied().collect()
    }

    /// Forgets the groups that hold textures of an old map.
    pub fn reset(&mut self) {
        self.page_groups.clear();
        self.tile_groups.clear();
        self.lattices.clear();
    }

    #[allow(clippy::too_many_arguments)]
    fn page_group(
        &self,
        device: &wgpu::Device,
        arrays: &PageArrays,
        fog: &FogBridge,
        fow: Option<CellFow<'_>>,
        key: GroupKey,
        spec_mask: Option<&wgpu::TextureView>,
    ) -> Option<wgpu::BindGroup> {
        // The highlight page and the mask of the screen; a cell without one reads black (the pipeline
        // without the highlight does not read them).
        let (view, spec_view, black, layout) = match key {
            GroupKey::Page(page, spec) => (
                arrays.page_view_all_mips(page)?,
                spec.and_then(|s| arrays.page_view_all_mips(s)),
                &fog.spec_black,
                &self.layout1,
            ),
            GroupKey::Array((size, array), spec) => (
                arrays.array_view(size, array as usize)?,
                spec.and_then(|(s, a)| arrays.array_view(s, a as usize)),
                &fog.spec_black_array,
                &self.layout1_array,
            ),
        };
        let spec_page = spec_view.as_ref().unwrap_or(black);
        let spec_mask = spec_mask.unwrap_or(&fog.spec_white);
        // The fluid fog's maps repeat (s4, s5 of the capture: Wrap); the bridge's are per field.
        let (mask, base, fow_sampler) = match fow {
            Some(f) => (f.mask, f.base, &self.shadow_sampler),
            None => (&fog.mask_view, &fog.base_view, &self.mask_sampler),
        };
        let overlay = fow.and_then(|f| f.overlay).unwrap_or(&fog.overlay_view);
        fn tex(binding: u32, v: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(v),
            }
        }
        fn smp(binding: u32, s: &wgpu::Sampler) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::Sampler(s),
            }
        }
        Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain cell page"),
            layout,
            entries: &[
                tex(0, &view),
                smp(1, &self.page_sampler),
                tex(2, overlay),
                smp(3, &self.clamp_sampler),
                tex(4, mask),
                smp(5, fow_sampler),
                tex(6, base),
                smp(7, fow_sampler),
                tex(8, spec_page),
                smp(9, &self.page_sampler),
                tex(10, spec_mask),
                smp(11, &self.clamp_sampler),
                wgpu::BindGroupEntry {
                    binding: 12,
                    resource: self.ps_consts.as_entire_binding(),
                },
            ],
        }))
    }

    /// Makes what the frame draws: the constants, the groups, the list of draws per copy.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        fog: &FogBridge,
        input: &FrameDraw<'_>,
    ) {
        if self.generation != input.generation {
            self.reset();
            self.generation = input.generation;
        }
        let fow_key = input.fow.map(|f| f.key);
        if self.fow_key != fow_key {
            self.page_groups.clear();
            self.fow_key = fow_key;
        }
        queue.write_buffer(
            &self.ps_consts,
            0,
            bytemuck::bytes_of(&CellPsData {
                cam_pos: input.eye_orig.to_array(),
                apply_diffuse: 1.0,
                apply_specular: 1.0,
                _pad: [0.0; 3],
            }),
        );
        // The light of the styles, with the shadow map of this frame.
        fn tex(binding: u32, v: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(v),
            }
        }
        fn smp(binding: u32, s: &wgpu::Sampler) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::Sampler(s),
            }
        }
        // The groups are made again only when a resource they bind changed (the shadow map of a
        // level, the frame constants, a style's volumes); they were five `createBindGroup` a frame.
        let key = ClassKey {
            shadow_map: input.shadow_map.clone(),
            globals: input.frame_globals.clone(),
            ramp: input.lights.ramp.clone(),
            classes: std::array::from_fn(|art| {
                input
                    .lights
                    .class(art)
                    .map(|l| (l.shadow_color.clone(), l.fow_color.clone()))
            }),
        };
        let same = self.class_key.as_ref() == Some(&key);
        self.class_key = Some(key);
        for art in 0..ART_CLASSES {
            if same {
                break;
            }
            self.class_groups[art] = input.lights.class(art).map(|light| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("terrain cell frame"),
                    layout: &self.layout0,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: input.frame_globals.as_entire_binding(),
                        },
                        tex(8, input.shadow_map),
                        smp(9, &self.shadow_sampler),
                        tex(10, &light.shadow_color),
                        smp(11, &self.clamp_sampler),
                        tex(12, &light.fow_color),
                        smp(13, &self.clamp_sampler),
                        tex(14, &input.lights.ramp),
                        smp(15, &self.clamp_sampler),
                    ],
                })
            });
        }

        // The slots of the constants of the vertex stage: a tile a copy.
        let mut slots: HashMap<((i32, i32), usize), u32> = HashMap::new();
        let mut slot_data: Vec<CellVsData> = Vec::new();
        let tile_scale = 1.0 / TILE_SIDE as f32;
        let copies = input.shifts.len().max(1);
        let mut draws: Vec<Vec<Draw>> = vec![Vec::new(); copies];
        self.shifts.clear();
        self.shifts.extend_from_slice(input.shifts);
        let batched = self.batched;
        for c in input.cells {
            let tile =
                super::heights_gpu::tile_of_cell(c.rect.x.div_euclid(64), c.rect.y.div_euclid(64));
            if input.tiles.tile(tile).is_none() {
                continue;
            }
            let Some(shift) = input.shifts.get(c.copy).copied() else {
                continue;
            };
            let slot = match slots.get(&(tile, c.copy)) {
                Some(&s) => s,
                None => {
                    if slot_data.len() >= MAX_VS_SLOTS {
                        continue;
                    }
                    let s = slot_data.len() as u32;
                    let mut d = vs_data(input.frame, &input.view_proj_orig, shift, tile_scale);
                    if let Some(f) = &input.fow {
                        d.mtx_composite_vp = row_major(f.projection);
                        d.fow_base_scale = f.base_scale;
                    }
                    slot_data.push(d);
                    slots.insert((tile, c.copy), s);
                    s
                }
            };
            let (group, order, layers) = if batched {
                let Some((array, layer)) = input.arrays.locate(c.page) else {
                    continue;
                };
                let spec = c
                    .spec
                    .and_then(|s| input.arrays.locate(s).map(|l| (s.size, l)));
                (
                    GroupKey::Array((c.page.size, array), spec.map(|(s, (a, _))| (s, a))),
                    [
                        c.page.size,
                        array,
                        spec.map_or(0, |(s, _)| s),
                        spec.map_or(0, |(_, (a, _))| a),
                    ],
                    [layer, spec.map_or(0, |(_, (_, l))| l)],
                )
            } else {
                (
                    GroupKey::Page(c.page, c.spec),
                    [c.page.size, c.page.layer, 0, 0],
                    [0, 0],
                )
            };
            if !self.page_groups.contains_key(&group) {
                if let Some(g) = self.page_group(
                    device,
                    input.arrays,
                    fog,
                    input.fow,
                    group,
                    input.lights.spec_mask.as_ref(),
                ) {
                    self.page_groups.insert(group, g);
                } else {
                    continue;
                }
            }
            if let Some(t) = input.tiles.tile(tile)
                && self
                    .tile_groups
                    .get(&tile)
                    .is_none_or(|(bound, _)| *bound != *t.texture)
            {
                let g = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("terrain cell tile"),
                    layout: &self.layout2,
                    entries: &[
                        tex(0, &t.view),
                        smp(1, &self.nearest),
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &self.vs_buffer,
                                offset: 0,
                                size: wgpu::BufferSize::new(
                                    std::mem::size_of::<CellVsData>() as u64
                                ),
                            }),
                        },
                    ],
                });
                self.tile_groups.insert(tile, (t.texture.clone(), g));
            }
            let key = (c.rect.w, c.rect.h, input.lattice_step);
            self.lattices
                .entry(key)
                .or_insert_with(|| Lattice::new(device, c.rect.w, c.rect.h, input.lattice_step));
            let (ox, oy) = tile_origin(tile);
            draws[c.copy].push(Draw {
                art: c.art,
                spec: c.spec.is_some(),
                group,
                order,
                tile,
                slot,
                lattice: (c.rect.w, c.rect.h, input.lattice_step),
                offset: [c.rect.x as f32, c.rect.y as f32, -(ox as f32), -(oy as f32)],
                layers,
            });
        }
        // Cells of the same style, tile, lattice and page (array) together: fewer groups to set, and
        // in the batched draw one call for each run. The page layer last: the order of the cells of
        // one call.
        let mut offsets: Vec<[f32; 4]> = Vec::with_capacity(input.cells.len());
        let mut layers: Vec<[u32; 2]> = Vec::with_capacity(input.cells.len());
        self.batches.resize_with(copies, Vec::new);
        self.batches.truncate(copies);
        self.geometry.resize_with(copies, Vec::new);
        self.geometry.truncate(copies);
        self.cells.clear();
        for (copy, list) in draws.iter_mut().enumerate() {
            list.sort_by_key(|d| (d.spec, d.art, d.tile, d.lattice, d.order, d.layers));
            let (batches, geometry) = (&mut self.batches[copy], &mut self.geometry[copy]);
            batches.clear();
            geometry.clear();
            for d in list.iter() {
                let first = offsets.len() as u32;
                offsets.push(d.offset);
                layers.push(d.layers);
                let batch = Batch {
                    first,
                    count: 1,
                    art: d.art,
                    spec: d.spec,
                    group: d.group,
                    tile: d.tile,
                    slot: d.slot,
                    lattice: d.lattice,
                };
                // GL draws a cell at a time (no first instance there): the runs are of one cell.
                let joins = |b: &Batch| {
                    batched
                        && b.first + b.count == first
                        && b.tile == d.tile
                        && b.slot == d.slot
                        && b.lattice == d.lattice
                };
                match batches.last_mut() {
                    Some(b)
                        if joins(b) && b.art == d.art && b.spec == d.spec && b.group == d.group =>
                    {
                        b.count += 1;
                    }
                    _ => batches.push(batch),
                }
                match geometry.last_mut() {
                    Some(b) if joins(b) => b.count += 1,
                    _ => geometry.push(batch),
                }
            }
            self.cells.push(list.len());
        }
        self.last_draws = offsets.len();
        self.last_batches = self.batches.iter().map(Vec::len).sum();

        let mut bytes = vec![0u8; slot_data.len() * VS_SLOT_BYTES as usize];
        for (i, d) in slot_data.iter().enumerate() {
            let at = i * VS_SLOT_BYTES as usize;
            bytes[at..at + std::mem::size_of::<CellVsData>()]
                .copy_from_slice(bytemuck::bytes_of(d));
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.vs_buffer, 0, &bytes);
        }
        let need = (offsets.len() as u64 * 16).max(16);
        if need > self.instance_capacity {
            self.instance_capacity = need.next_power_of_two();
            self.instances = device.buf(
                &wgpu::BufferDescriptor {
                    label: Some("terrain cell instances"),
                    size: self.instance_capacity,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                },
                Cluster::TerrainTex,
            );
        }
        if !offsets.is_empty() {
            queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&offsets));
        }
        if batched {
            let need = (layers.len() as u64 * LAYER_STRIDE).max(LAYER_STRIDE);
            if need > self.layer_capacity {
                self.layer_capacity = need.next_power_of_two();
                self.layers = device.buf(
                    &wgpu::BufferDescriptor {
                        label: Some("terrain cell layers"),
                        size: self.layer_capacity,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    },
                    Cluster::TerrainTex,
                );
            }
            if !layers.is_empty() {
                queue.write_buffer(&self.layers, 0, bytemuck::cast_slice(&layers));
            }
        }
    }

    fn copy_index(&self, shift: f32) -> Option<usize> {
        self.shifts.iter().position(|&s| s == shift)
    }

    /// The two vertex buffers of a cell's lattice for a pipeline of another pass (the cells of
    /// `world-overlays`): the vertices (`location 0..2`) and the origin of the cell (`location 3`, one
    /// per instance). Their stride and attributes are those of `terrain_shader_vs`.
    pub fn vertex_layouts() -> ([wgpu::VertexAttribute; 3], [wgpu::VertexAttribute; 1]) {
        (
            wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x3, 2 => Float32x2],
            wgpu::vertex_attr_array![3 => Float32x4],
        )
    }

    /// Bytes of a vertex of the lattice.
    pub const VERTEX_STRIDE: u64 = std::mem::size_of::<LatticeVertex>() as u64;

    /// The layout of group 2 of the vertex stage (the tile of heights, its sampler and the constants
    /// of the cell, with a dynamic offset), the group `draw_geometry` binds.
    pub fn vs_layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout2
    }

    /// The copies the last `prepare` made a list for, as the shifts of the map in our X.
    pub fn copy_shifts(&self) -> &[f32] {
        &self.shifts
    }

    /// Cells in the list of a copy.
    pub fn cells_in_copy(&self, copy_shift: f32) -> usize {
        self.copy_index(copy_shift)
            .and_then(|c| self.cells.get(c).copied())
            .unwrap_or(0)
    }

    /// Sets the lattice and the instances of a run of cells and draws it: the whole instance
    /// buffers and a range of instances when batched, the cell's own instance on GL (which has no
    /// first instance). Returns the index count of one cell.
    fn draw_run<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'_>,
        state: &mut crate::perf::PassState<'a>,
        lattice: &'a Lattice,
        b: &Batch,
        with_layers: bool,
    ) {
        state.vertex(pass, 0, &lattice.vertices, 0..lattice.vertices.size());
        state.index(
            pass,
            &lattice.indices,
            0..lattice.indices.size(),
            wgpu::IndexFormat::Uint32,
        );
        if self.batched {
            state.vertex(pass, 1, &self.instances, 0..self.instances.size());
            if with_layers {
                state.vertex(pass, 2, &self.layers, 0..self.layers.size());
            }
            pass.draw_indexed_c(0..lattice.index_count, 0, b.first..b.first + b.count);
        } else {
            let at = u64::from(b.first) * 16;
            state.vertex(pass, 1, &self.instances, at..at + 16);
            pass.draw_indexed_c(0..lattice.index_count, 0, 0..1);
        }
    }

    /// Draws the lattices of the visible cells of one copy with the pipeline the caller set: group
    /// `vs_group` holds the heights and the constants of the cell, the vertex buffers 0 and 1 the
    /// lattice and the origin. The pixel stage and the other groups are the caller's. Returns how many
    /// cells were drawn.
    pub fn draw_geometry(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        copy_shift: f32,
        vs_group: u32,
    ) -> usize {
        let Some(copy) = self.copy_index(copy_shift) else {
            return 0;
        };
        let mut drawn = 0;
        let mut state = crate::perf::PassState::new();
        let mut tile_slot: Option<((i32, i32), u32)> = None;
        for b in &self.geometry[copy] {
            let (Some(tile), Some(lattice)) = (
                self.tile_groups.get(&b.tile).map(|g| &g.1),
                self.lattices.get(&b.lattice),
            ) else {
                continue;
            };
            if tile_slot != Some((b.tile, b.slot)) {
                state.group_offsets(pass, vs_group, tile, &[b.slot * VS_SLOT_BYTES as u32]);
                tile_slot = Some((b.tile, b.slot));
            }
            self.draw_run(pass, &mut state, lattice, b, false);
            drawn += b.count as usize;
        }
        drawn
    }

    /// Draws the cells of the copy shifted by `copy_shift` into the main pass.
    pub fn draw(&self, cx: &mut DrawContext<'_, '_>) {
        let Some(copy) = self.copy_index(cx.copy_shift) else {
            return;
        };
        let batches = &self.batches[copy];
        if batches.is_empty() {
            return;
        }
        let pass = &mut *cx.pass;
        // What the last run set stays set: runs of one lattice size share the vertex and index
        // buffers, neighbours of a tile share the tile group and its slot, runs of one page (array)
        // the page group (the cells are sorted by that).
        let mut state = crate::perf::PassState::new();
        let mut with_spec = None;
        let mut art = usize::MAX;
        let mut tile_slot: Option<((i32, i32), u32)> = None;
        for b in batches {
            let (Some(page), Some(tile), Some(lattice)) = (
                self.page_groups.get(&b.group),
                self.tile_groups.get(&b.tile).map(|g| &g.1),
                self.lattices.get(&b.lattice),
            ) else {
                continue;
            };
            let pipeline = match (self.batched, b.spec) {
                (false, false) => Some(&self.pipeline),
                (false, true) => Some(&self.spec_pipeline),
                (true, false) => self.array_pipeline.as_ref(),
                (true, true) => self.array_spec_pipeline.as_ref(),
            };
            let Some(pipeline) = pipeline else {
                continue;
            };
            if with_spec != Some(b.spec) {
                // A new pipeline: the groups are set again.
                with_spec = Some(b.spec);
                state.reset();
                tile_slot = None;
                state.pipeline(pass, pipeline);
                art = usize::MAX;
            }
            if b.art != art {
                let Some(g) = self.class_groups[b.art].as_ref() else {
                    continue;
                };
                state.group(pass, 0, g);
                art = b.art;
            }
            state.group(pass, 1, page);
            if tile_slot != Some((b.tile, b.slot)) {
                state.group_offsets(pass, 2, tile, &[b.slot * VS_SLOT_BYTES as u32]);
                tile_slot = Some((b.tile, b.slot));
            }
            self.draw_run(pass, &mut state, lattice, b, true);
            cx.stats.record_draw(lattice.index_count * b.count);
        }
    }

    /// Draws the same cells into the class mask.
    pub fn draw_mask(&self, cx: &mut MaskContext<'_, '_>) {
        let Some(copy) = self.copy_index(cx.copy_shift) else {
            return;
        };
        let runs = &self.geometry[copy];
        if runs.is_empty() {
            return;
        }
        let pass = &mut *cx.pass;
        pass.set_pipeline_c(&self.mask_pipeline);
        let mut state = crate::perf::PassState::new();
        let mut tile_slot: Option<((i32, i32), u32)> = None;
        for b in runs {
            let (Some(tile), Some(lattice)) = (
                self.tile_groups.get(&b.tile).map(|g| &g.1),
                self.lattices.get(&b.lattice),
            ) else {
                continue;
            };
            if tile_slot != Some((b.tile, b.slot)) {
                state.group_offsets(pass, 2, tile, &[b.slot * VS_SLOT_BYTES as u32]);
                tile_slot = Some((b.tile, b.slot));
            }
            self.draw_run(pass, &mut state, lattice, b, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hexgrid::cell_center;

    #[test]
    fn the_constants_have_the_size_of_their_wgsl_structs() {
        assert_eq!(std::mem::size_of::<CellVsData>(), 240);
        assert!(std::mem::size_of::<CellVsData>() as u64 <= VS_SLOT_BYTES);
        assert_eq!(std::mem::size_of::<CellPsData>(), 32);
        assert_eq!(std::mem::size_of::<LatticeVertex>(), 36);
    }

    #[test]
    fn the_lattice_has_a_vertex_every_step_and_the_last_texel() {
        assert_eq!(lattice_axis(65, 1).len(), 65);
        assert_eq!(lattice_axis(65, 2), (0..65).step_by(2).collect::<Vec<_>>());
        // 64 texels with a step of 3: 0, 3 .. 63, and the last texel 63 is there already.
        let a = lattice_axis(64, 3);
        assert_eq!((a[0], *a.last().unwrap()), (0, 63));
        // 65 texels with a step of 3: 0 .. 63, then 64.
        let b = lattice_axis(65, 3);
        assert_eq!(&b[b.len() - 2..], &[63, 64]);
        let (v, i) = lattice(65, 65, 1);
        assert_eq!(v.len(), 65 * 65);
        assert_eq!(i.len(), 64 * 64 * 6);
        assert!(i.iter().all(|&k| (k as usize) < v.len()));
        // The page covers the cell and a border: the first texel is one 66th in, the last 65 of 66.
        assert!((v[0].uv[0] - 1.0 / 66.0).abs() < 1e-6);
        assert!((v[v.len() - 1].uv[1] - 65.0 / 66.0).abs() < 1e-6);
    }

    #[test]
    fn the_triangles_of_the_lattice_turn_counter_clockwise_from_above() {
        let (v, i) = lattice(4, 4, 1);
        for t in i.chunks(3) {
            let p = |k: u32| Vec2::new(v[k as usize].pos[0], v[k as usize].pos[1]);
            let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
            let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
            assert!(area > 0.0, "{a:?} {b:?} {c:?}");
        }
    }

    #[test]
    fn a_view_gives_six_planes_that_keep_the_points_in_front() {
        // Looking along z: x and y in [-1, 1], z in [-50, 50] to depth 1..0.
        let m = Mat4::from_cols(
            Vec4::X,
            Vec4::Y,
            Vec4::new(0.0, 0.0, -0.01, 0.0),
            Vec4::new(0.0, 0.0, 0.5, 1.0),
        );
        let planes = frustum_planes(&m);
        let inside = |p: Vec3| planes.iter().all(|pl| pl.truncate().dot(p) + pl.w >= 0.0);
        assert!(inside(Vec3::ZERO));
        assert!(inside(Vec3::new(1.0, 1.0, 0.0)));
        assert!(!inside(Vec3::new(500.0, 0.0, 0.0)));
        assert!(!inside(Vec3::new(0.0, 0.0, 200.0)));
        // The planes are normalised.
        for p in planes {
            assert!((p.truncate().length() - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn the_fog_mask_knows_the_hexes_the_fog_lists() {
        let frame = TerrainFrame::new(6, 6, false);
        let mut fog = vec![Fog::Unrevealed; 36];
        fog[2 * 6 + 3] = Fog::Visible;
        fog[2 * 6 + 2] = Fog::Hidden;
        let size = fog_mask_size(&frame);
        assert_eq!(size, (24, 24));
        let bytes = fog_mask_bytes(&frame, &fog, size);
        let at = |hex: (i32, i32)| {
            let t = frame.hex_to_tex(hex.0, hex.1);
            let (i, j) = (
                (t.x / MASK_DIV as f32) as usize,
                (t.y / MASK_DIV as f32) as usize,
            );
            let k = (j * size.0 as usize + i) * 4;
            [bytes[k], bytes[k + 2]]
        };
        assert_eq!(at((3, 2)), [255, 255], "seen");
        assert_eq!(at((2, 2)), [255, 0], "known, not seen");
        assert_eq!(at((0, 0)), [0, 0], "not known");
        let _ = cell_center(0, 0);
    }

    #[test]
    fn the_vertex_constants_put_the_map_in_the_middle_of_the_world() {
        let frame = TerrainFrame::new(6, 6, false);
        let d = vs_data(&frame, &Mat4::IDENTITY, 0.0, 1.0 / 1025.0);
        let half = frame.half_world();
        // The world matrix is a move by -half, read back as the register 3 of the original: the
        // last row of the `row_major` matrix is the move.
        let w = Mat4::from_cols_array_2d(&d.world).transpose();
        let p = w.transform_point3(Vec3::ZERO);
        assert!((p.x + half.x).abs() < 1e-3 && (p.y + half.y).abs() < 1e-3);
        // A copy to the east moves by the shift.
        let c = vs_data(&frame, &Mat4::IDENTITY, 384.0, 1.0 / 1025.0);
        let p = Mat4::from_cols_array_2d(&c.world)
            .transpose()
            .transform_point3(Vec3::ZERO);
        assert!((p.x - (384.0 - half.x)).abs() < 1e-3);
        assert_eq!(d.world_scales[2], frame.terrain_scale().z);
        // The map spans the whole of the mask: the east edge of the world is the u of 1.
        let comp = Mat4::from_cols_array_2d(&d.mtx_composite_vp).transpose();
        let (tw, _) = frame.texel_size();
        let east = comp.transform_point3(Vec3::new(tw as f32 * frame.terrain_scale().x, 0.0, 0.0));
        assert!((east.x - 2.0).abs() < 1e-4, "{east:?}");
    }
}
