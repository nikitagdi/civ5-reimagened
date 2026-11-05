//! The draw of the trees through `ForestShader`: `ForestVS` and
//! `ForestSSOverlayPS` of the original on the models of `assets-b2`.
//!
//! A tree is a position and a scale in the original's basis (z up): the instance buffer holds one
//! `{x, y, z, scale}` register per tree, what `treeLocation` holds in the original (RW-16), and the
//! vertex of the model is put into the original's basis when the model is uploaded, because
//! `ForestVS` adds it to the position as it is.
//!
//! The leaves are blended with the depth written, so the original draws them back to front: the
//! cells nearest the top of the screen first (`ForestVisSystem::InsertIntoVisList` keeps the vis
//! list in that order) and the trees of a cell in the order of their sorted list (S1a ev 1383: the
//! `treeLocation` registers by falling `y`). Out of order, a near tree's fringe hides the trees
//! behind it and the forest edge turns into a hard dark wall (golden S2). Here the trees of one
//! model go in one instanced draw in that order, by falling `y` in the original's basis: each
//! instance runs every variant (mesh) of the model, and the vertex stage folds the triangles of
//! the other variants to nothing.
//!
//! Group 0 is everything the shader reads but the tree: the frame constants, the ramp of the distance
//! fog, the two colour tables of the style (`ColorCorrection` is the table of the terrain, resource
//! 1920 in the capture; `TransFOWColor` the table of the fog of war, 1935), the fog-of-war maps and
//! the overlay maps, and the constants of the two stages. It is made every frame (the views of the
//! targets change when a target is made again; the tables with the style). Group 3 is the material of
//! a draw: `BaseSampler` (the diffuse of the model) and `ColorizeSampler` (the overlay of the style).
//!
//! The pixel stage gets `vPos` as the pixel centre (the convention of DX11, where the captures were
//! made), not the corner as D3D9 does.

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use std::collections::{BTreeMap, HashMap};

use asset_format::{LogicalId, ModelBlob, TextureBlob};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};

use super::blocks::Group;
use super::cull::{BlockKey, bounds_of, outside};
use crate::frame::{DEPTH_FORMAT, WORLD_FORMAT, multisample};
use crate::lighting::{LightingEnv, STYLES, row_major};
use crate::renderer::FrameStats;
use crate::shaders::create_module;
use crate::static_model::{StaticVertex, Variants, build_geometry};
use crate::texture::{UploadOptions, upload_blob};
use crate::world::terrain::draw::frustum_planes;

/// Bytes of the constants of the vertex stage: two matrices, `g_FOWBaseScale` and the
/// `treeLocation[90]` that the instance buffer replaces.
const VS_BYTES: u64 = 64 * 2 + 16 + 16 * 90;
/// Distance between the constants of two copies of the map (a dynamic offset).
const VS_STRIDE: u64 = 2048;
/// Copies of a wrapping map a frame draws at most.
pub const COPIES: usize = 3;
/// Bytes of the constants of the pixel stage.
const PS_BYTES: u64 = 32;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct VsConsts {
    mtx_view_proj: [[f32; 4]; 4],
    overlay_projection: [[f32; 4]; 4],
    fow_base_scale: [f32; 4],
    tree_location: [[f32; 4]; 90],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PsConsts {
    screen_res: [f32; 2],
    overlay_multiplier: f32,
    _pad0: f32,
    cam_pos: [f32; 3],
    _pad1: f32,
}

/// A vertex of the model in the original's basis: our `(x, y, z)` is the original's `(x, -z, y)`.
pub fn to_original(p: [f32; 3]) -> [f32; 3] {
    [p[0], -p[2], p[1]]
}

/// One tree of a draw: `treeLocation` of the original (`{x, y, z, scale}` in its basis) and the
/// variant (mesh) of the model it shows.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct TreeInstance {
    pub location: [f32; 4],
    pub variant: u32,
}

/// Puts the trees in the order the original draws them: back to front, by falling `y` of the
/// original's basis (its camera looks north and never turns).
pub fn sort_back_to_front(trees: &mut [TreeInstance]) {
    trees.sort_by(|a, b| {
        b.location[1]
            .total_cmp(&a.location[1])
            .then(a.location[0].total_cmp(&b.location[0]))
    });
}

struct MeshGpu {
    vertices: Buf,
    indices: Buf,
    /// Indices of the mesh: every variant of the model.
    index_count: u32,
    /// The texture of the material: the diffuse of the model.
    base: wgpu::TextureView,
    /// The mesh for `forest_vs_pulled` (WebGPU and native): its group 1 and the run of each variant.
    pulled: Option<PulledMesh>,
}

/// The storage buffers of `forest_vs_pulled` for one mesh: the vertices and the indices by variant
/// ([`pulled_indices`]), in its group 1.
struct PulledMesh {
    _vertices: Buf,
    _indices: Buf,
    group: wgpu::BindGroup,
    /// Vertices a tree runs: the indices of the longest variant.
    stride: u32,
}

/// The padding of a variant's run that is shorter than the longest (`PAD` of `forest_pulled.wgsl`).
const PULLED_PAD: u32 = u32::MAX;

/// The indices of a mesh for `forest_vs_pulled`: the run length (the indices of the longest variant)
/// first, then the indices of each variant in their order, padded with [`PULLED_PAD`] to that length.
/// A triangle is the variant of its first vertex (a mesh of a tree model is one variant).
pub fn pulled_indices(vertices: &[StaticVertex], indices: &[u32], variants: u32) -> Vec<u32> {
    let mut runs: Vec<Vec<u32>> = vec![Vec::new(); variants.max(1) as usize];
    for t in indices.chunks_exact(3) {
        let v = (vertices[t[0] as usize].variant as usize).min(runs.len() - 1);
        runs[v].extend_from_slice(t);
    }
    let stride = runs.iter().map(Vec::len).max().unwrap_or(0);
    let mut out = Vec::with_capacity(1 + stride * runs.len());
    out.push(stride as u32);
    for r in runs {
        let pad = stride - r.len();
        out.extend(r);
        out.extend(std::iter::repeat_n(PULLED_PAD, pad));
    }
    out
}

/// A vertex for `forest_vs_pulled`: `PulledVertex` of the shader, 20 bytes.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PulledVertex {
    position: [f32; 3],
    /// The uv of the f16 pair, widened as `Float16x2` widens it (exactly).
    uv: [f32; 2],
}

/// An f16 (its bits) as f32, exactly: what the vertex fetch of `Float16x2` gives.
fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((h >> 10) & 0x1F);
    let mant = f32::from(h & 0x3FF);
    match exp {
        0 => sign * mant * (2.0f32).powi(-24),
        31 if mant == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        e => sign * (1.0 + mant / 1024.0) * (2.0f32).powi(e - 15),
    }
}

struct ModelGpu {
    meshes: Vec<MeshGpu>,
    variants: u32,
    /// How far a tree goes from its place at scale 1, in the original's basis: across (x, y) and up
    /// (z), for the bounds the view culling uses ([`super::cull`]).
    reach: (f32, f32),
    /// The far-view LOD ([`crate::world::far_lod`]): the variant every far tree shows and its run of
    /// the indices of each mesh. `None` when the indices of a variant are not one run.
    lod: Option<FarMesh>,
}

/// One variant of a tree model for the trees far away: a far tree runs the indices of that variant
/// alone, not those of every variant folded away but one.
struct FarMesh {
    variant: u32,
    /// Per mesh of the model.
    ranges: Vec<std::ops::Range<u32>>,
}

/// The trees of one block of the map in one draw, in the order they are drawn
/// ([`sort_back_to_front`]), with the box that holds them.
struct BlockGpu {
    instances: Buf,
    /// The same trees, in the same order, all of them the variant of the model's [`FarMesh`].
    far: Option<Buf>,
    count: u32,
    min: Vec3,
    max: Vec3,
}

/// The variant of a model the far trees show, and its run of the indices of each mesh: the variant
/// whose height is nearest the median of the variants' (a typical tree of the model), the one with
/// fewer triangles of two as near. `None` if a variant's indices in a mesh are not one run.
fn far_mesh(geometry: &crate::static_model::StaticGeometry) -> Option<FarMesh> {
    let variants = geometry.variant_count() as usize;
    if variants < 2 {
        return None;
    }
    let mut height = vec![0.0f32; variants];
    for v in geometry.materials.iter().flat_map(|g| &g.vertices) {
        let h = &mut height[(v.variant as usize).min(variants - 1)];
        *h = h.max(to_original(v.position)[2]);
    }
    let mut sorted = height.clone();
    sorted.sort_by(f32::total_cmp);
    let median = sorted[variants / 2];
    let variant = (0..variants).min_by(|&a, &b| {
        (height[a] - median)
            .abs()
            .total_cmp(&(height[b] - median).abs())
            .then(geometry.variant_triangles[a].cmp(&geometry.variant_triangles[b]))
    })? as u32;
    let mut ranges = Vec::with_capacity(geometry.materials.len());
    for g in &geometry.materials {
        let of = |t: &[u32]| g.vertices[t[0] as usize].variant;
        let mut run: Option<std::ops::Range<u32>> = None;
        for (k, t) in g.indices.chunks_exact(3).enumerate() {
            if of(t) != variant {
                continue;
            }
            let at = (k * 3) as u32;
            run = match run {
                None => Some(at..at + 3),
                Some(r) if r.end == at => Some(r.start..at + 3),
                Some(_) => return None,
            };
        }
        ranges.push(run.unwrap_or(0..0));
    }
    Some(FarMesh { variant, ranges })
}

/// The trees of one model for one style, in blocks of the map (each its own buffer, so a change
/// makes again the blocks it touches and the others stay as they were).
struct SetGpu {
    model: LogicalId,
    /// The overlay of the feature and style (`ColorizeSampler`).
    colorize: Option<LogicalId>,
    style: usize,
    /// In the order they are drawn: the rows from the top down, west to east.
    blocks: BTreeMap<BlockKey, BlockGpu>,
    /// Group 3 of each mesh of the model.
    groups: Vec<wgpu::BindGroup>,
}

/// What the frame gives the draw.
pub struct ForestFrame<'a> {
    pub globals: &'a wgpu::Buffer,
    pub lighting: &'a LightingEnv,
    /// `FowMask`: without it the trees are not drawn natively.
    pub fow_mask: &'a wgpu::TextureView,
    pub fow_base: Option<&'a wgpu::TextureView>,
    pub screen_overlay: Option<&'a wgpu::TextureView>,
    pub overlay_map: Option<&'a wgpu::TextureView>,
    /// Clip space from the original's basis.
    pub view_proj_orig: Mat4,
    /// `OverlayProjection`: the rectangle the mask covers (`MaskView::projection`).
    pub overlay_projection: Mat4,
    pub fow_base_scale: [f32; 4],
    pub eye_orig: Vec3,
    pub screen: (f32, f32),
    /// The shift in x of each copy of the map the frame draws (at most [`COPIES`]).
    pub shifts: &'a [f32],
    /// `overlay_alpha` of `forests.xml`.
    pub overlay_alpha: f32,
    /// `g_ReflectVP` for points of the original's basis: the trees are drawn into the reflection
    /// with it. `None` when the reflection pass is off.
    pub reflect_vp_orig: Option<Mat4>,
    /// The far-view LOD (`FarLOD`): a block whose box is wholly farther than this from `eye_orig`
    /// (the original's basis) draws its trees as the far mesh of the model. `None`: off.
    pub far_lod: Option<f32>,
}

pub struct ForestDraw {
    samples: u32,
    group0_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    module: wgpu::ShaderModule,
    pipeline: Option<wgpu::RenderPipeline>,
    /// `ForestSSReflectPS` into `ReflectionColor` (never multisampled); made with the first frame
    /// that has a reflection camera.
    reflection_pipeline: Option<wgpu::RenderPipeline>,
    /// `forest_mask_ps` into the class mask (`golden`), made with `pipeline`.
    mask_pipeline: Option<wgpu::RenderPipeline>,
    /// Whether the constants of the reflection camera are written (the slots after the copies').
    reflection_on: bool,
    wrap: wgpu::Sampler,
    clamp: wgpu::Sampler,
    vs: Buf,
    /// The copies of the map the constants were written for, by their shift in x.
    shifts: Vec<f32>,
    /// The planes of the view of each slot of the constants (the copies', then the reflection's), for
    /// the culling of the blocks of trees; a slot with none draws everything.
    planes: Vec<[Vec4; 6]>,
    ps: Buf,
    white: wgpu::TextureView,
    clear: wgpu::TextureView,
    textures: HashMap<LogicalId, wgpu::TextureView>,
    models: HashMap<LogicalId, ModelGpu>,
    sets: BTreeMap<Group, SetGpu>,
    /// Group 0 of each style used, made by [`Self::frame`].
    group0: HashMap<usize, wgpu::BindGroup>,
    /// What each group of `group0` was made of.
    group0_keys: HashMap<usize, GroupKey>,
    ready: bool,
    drawn: std::cell::Cell<u32>,
    /// The eye in the original's basis and the distance of the far-view LOD of the last frame.
    far: Option<(Vec3, f32)>,
    /// The far-view LOD is on ([`Self::set_far_enabled`]): the blocks are uploaded with their far
    /// trees too. Off, nothing of the LOD is built.
    far_enabled: bool,
    /// The device has storage buffers in the vertex stage (WebGPU, native): the pipelines run
    /// `forest_vs_pulled` of this module through this layout, and every mesh has its
    /// [`PulledMesh`]. `None` (WebGL2): `forest_vs` and every variant per tree.
    pulled: Option<Pulled>,
    /// The pulled path is used where it exists ([`Self::set_pulled`]).
    pull: bool,
}

/// What `forest_vs_pulled` needs besides the meshes.
struct Pulled {
    module: wgpu::ShaderModule,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
}

impl Pulled {
    /// `None` where the vertex stage has no storage buffers (WebGL2), or when `CIV5_FOREST_PULLED=0`
    /// (natively, to compare with the old path).
    fn new(
        device: &wgpu::Device,
        group0: &wgpu::BindGroupLayout,
        material: &wgpu::BindGroupLayout,
    ) -> Option<Self> {
        if device.limits().max_storage_buffers_per_shader_stage < 2 {
            return None;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if std::env::var("CIV5_FOREST_PULLED").is_ok_and(|v| v == "0") {
            return None;
        }
        let storage = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("forest pulled mesh"),
            entries: &[storage(0), storage(1)],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("forest pulled"),
            bind_group_layouts: &[Some(group0), Some(&layout), None, Some(material)],
            immediate_size: 0,
        });
        let module = create_module(
            device,
            "forest pulled",
            crate::wgsl_module!(
                "common/frame.wgsl",
                "common/sm3.wgsl",
                "forest/forest.wgsl",
                "forest/forest_pulled.wgsl"
            ),
        );
        Some(Self {
            module,
            layout,
            pipeline_layout,
        })
    }
}

fn one_texel(device: &wgpu::Device, queue: &wgpu::Queue, rgba: [u8; 4]) -> wgpu::TextureView {
    device
        .tex_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("forest texel"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &rgba,
            Cluster::Models,
        )
        .create_view(&wgpu::TextureViewDescriptor::default())
}

fn tex_entry(
    binding: u32,
    dim: wgpu::TextureViewDimension,
    stage: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: stage,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: dim,
            multisampled: false,
        },
        count: None,
    }
}

fn smp_entry(binding: u32, stage: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: stage,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn uniform_entry(binding: u32, stage: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: stage,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: binding == 11,
            min_binding_size: None,
        },
        count: None,
    }
}

impl ForestDraw {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, samples: u32) -> Self {
        use wgpu::TextureViewDimension::{D2, D3};
        let frag = wgpu::ShaderStages::FRAGMENT;
        let mut entries = vec![
            uniform_entry(1, wgpu::ShaderStages::VERTEX_FRAGMENT),
            uniform_entry(11, wgpu::ShaderStages::VERTEX),
            uniform_entry(12, frag),
        ];
        // FogRamp 2, ColorCorrection 4 (3D), TransFOWColor 6 (3D), FOWMask 8, FOWBaseLayer 13,
        // SSOverlaySampler 15, OverlaySampler 17: each a texture and the sampler after it.
        for (binding, dim) in [
            (2, D2),
            (4, D3),
            (6, D3),
            (8, D2),
            (13, D2),
            (15, D2),
            (17, D2),
        ] {
            entries.push(tex_entry(binding, dim, frag));
            entries.push(smp_entry(binding + 1, frag));
        }
        let group0_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("forest frame"),
            entries: &entries,
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("forest material"),
            entries: &[
                tex_entry(0, D2, frag),
                smp_entry(1, frag),
                tex_entry(2, D2, frag),
                smp_entry(3, frag),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("forest"),
            bind_group_layouts: &[Some(&group0_layout), None, None, Some(&material_layout)],
            immediate_size: 0,
        });
        let sampler = |mode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("forest sampler"),
                address_mode_u: mode,
                address_mode_v: mode,
                address_mode_w: mode,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            })
        };
        let buffer = |label, size| {
            device.buf(
                &wgpu::BufferDescriptor {
                    label: Some(label),
                    size,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                },
                Cluster::Models,
            )
        };
        let pulled = Pulled::new(device, &group0_layout, &material_layout);
        Self {
            samples,
            group0_layout,
            material_layout,
            pipeline_layout,
            module: create_module(
                device,
                "forest",
                crate::wgsl_module!("common/frame.wgsl", "common/sm3.wgsl", "forest/forest.wgsl"),
            ),
            pipeline: None,
            reflection_pipeline: None,
            mask_pipeline: None,
            reflection_on: false,
            wrap: sampler(wgpu::AddressMode::Repeat),
            clamp: sampler(wgpu::AddressMode::ClampToEdge),
            vs: buffer("forest vs consts", VS_STRIDE * 2 * COPIES as u64),
            shifts: vec![0.0],
            planes: Vec::new(),
            ps: buffer("forest ps consts", PS_BYTES),
            white: one_texel(device, queue, [255; 4]),
            clear: one_texel(device, queue, [0; 4]),
            textures: HashMap::new(),
            models: HashMap::new(),
            sets: BTreeMap::new(),
            group0: HashMap::new(),
            group0_keys: HashMap::new(),
            ready: false,
            drawn: std::cell::Cell::new(0),
            far: None,
            far_enabled: false,
            pulled,
            pull: true,
        }
    }

    /// Turns the far-view LOD of the trees on or off; true when that changed, and the caller makes
    /// every block again (a block made with it off has no far trees, and one made with it on drops
    /// them).
    pub fn set_far_enabled(&mut self, on: bool) -> bool {
        let changed = self.far_enabled != on;
        self.far_enabled = on;
        changed
    }

    /// The pipelines run `forest_vs_pulled` ([`Pulled`]).
    pub fn is_pulled(&self) -> bool {
        self.pulled.is_some() && self.pull
    }

    /// Whether the pipelines pull the vertices where the device can (the default) or run every
    /// variant per tree as WebGL2 does; the pipelines are made again for the next frame. For the
    /// comparison of the two paths.
    pub fn set_pulled(&mut self, on: bool) {
        if self.pull != on {
            self.pull = on;
            self.pipeline = None;
            self.reflection_pipeline = None;
            self.mask_pipeline = None;
        }
    }

    /// A texture the models or the draws name (the diffuse of a tree, the overlay of a style).
    pub fn add_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        id: &LogicalId,
        blob: &TextureBlob,
    ) {
        match upload_blob(device, queue, blob, id.as_str(), UploadOptions::WORLD) {
            Ok(up) if up.view_dimension == wgpu::TextureViewDimension::D2 => {
                self.textures.insert(
                    id.clone(),
                    up.texture
                        .create_view(&wgpu::TextureViewDescriptor::default()),
                );
            }
            Ok(_) => tracing::warn!(id = id.as_str(), "forest texture is not 2D, left out"),
            Err(e) => tracing::warn!(id = id.as_str(), "forest texture: {e}"),
        }
    }

    /// A tree model: one mesh per material with every variant of the model (each vertex says
    /// which), the vertices in the original's basis.
    pub fn add_model(&mut self, device: &wgpu::Device, id: &LogicalId, blob: &ModelBlob) {
        let geometry = build_geometry(blob, Variants::PerMesh);
        let variants = geometry.variant_count();
        let lod = far_mesh(&geometry);
        let mut reach = (0.0f32, 0.0f32);
        for v in geometry.materials.iter().flat_map(|g| &g.vertices) {
            let p = to_original(v.position);
            reach.0 = reach.0.max(p[0].hypot(p[1]));
            reach.1 = reach.1.max(p[2].abs());
        }
        let meshes = geometry
            .materials
            .iter()
            .map(|g| {
                let vertices: Vec<StaticVertex> = g
                    .vertices
                    .iter()
                    .map(|v| StaticVertex {
                        position: to_original(v.position),
                        ..*v
                    })
                    .collect();
                let base = blob
                    .materials
                    .get(g.material)
                    .and_then(|m| m.diffuse.as_ref())
                    .and_then(|d| self.textures.get(d))
                    .unwrap_or(&self.white)
                    .clone();
                let pulled = self.pulled.as_ref().map(|p| {
                    let pv: Vec<PulledVertex> = vertices
                        .iter()
                        .map(|v| PulledVertex {
                            position: v.position,
                            uv: [f16_to_f32(v.uv[0]), f16_to_f32(v.uv[1])],
                        })
                        .collect();
                    let pi = pulled_indices(&vertices, &g.indices, variants);
                    let storage = |label, contents: &[u8]| {
                        device.buf_init(
                            &wgpu::util::BufferInitDescriptor {
                                label: Some(label),
                                // A binding of an empty array is not allowed: one element at least.
                                contents: if contents.is_empty() {
                                    &[0; 16]
                                } else {
                                    contents
                                },
                                usage: wgpu::BufferUsages::STORAGE,
                            },
                            Cluster::Models,
                        )
                    };
                    let vb = storage("forest pulled vertices", bytemuck::cast_slice(&pv));
                    let ib = storage("forest pulled indices", bytemuck::cast_slice(&pi));
                    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("forest pulled mesh"),
                        layout: &p.layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: vb.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: ib.as_entire_binding(),
                            },
                        ],
                    });
                    PulledMesh {
                        _vertices: vb,
                        _indices: ib,
                        group,
                        stride: pi[0],
                    }
                });
                MeshGpu {
                    vertices: device.buf_init(
                        &wgpu::util::BufferInitDescriptor {
                            label: Some("forest vertices"),
                            contents: bytemuck::cast_slice(&vertices),
                            usage: wgpu::BufferUsages::VERTEX,
                        },
                        Cluster::Models,
                    ),
                    indices: device.buf_init(
                        &wgpu::util::BufferInitDescriptor {
                            label: Some("forest indices"),
                            contents: bytemuck::cast_slice(&g.indices),
                            usage: wgpu::BufferUsages::INDEX,
                        },
                        Cluster::Models,
                    ),
                    index_count: g.indices.len() as u32,
                    base,
                    pulled,
                }
            })
            .collect();
        self.models.insert(
            id.clone(),
            ModelGpu {
                meshes,
                variants,
                reach,
                lod,
            },
        );
    }

    pub fn has_model(&self, id: &LogicalId) -> bool {
        self.models.contains_key(id)
    }

    /// Variants of a model, 0 if it is not on the GPU.
    pub fn variant_count(&self, id: &LogicalId) -> u32 {
        self.models.get(id).map_or(0, |m| m.variants)
    }

    /// Makes sure the draw of `key` shows `model` with the overlay `colorize`; a draw of another model
    /// starts over (its blocks are dropped). False if the model is not on the GPU.
    pub fn ensure_set(
        &mut self,
        device: &wgpu::Device,
        key: Group,
        model_id: &LogicalId,
        colorize: Option<&LogicalId>,
        style: usize,
    ) -> bool {
        if self
            .sets
            .get(&key)
            .is_some_and(|s| &s.model == model_id && s.colorize.as_ref() == colorize)
        {
            return true;
        }
        let Some(model) = self.models.get(model_id) else {
            return false;
        };
        let overlay = colorize
            .and_then(|c| self.textures.get(c))
            .unwrap_or(&self.white);
        let groups = model
            .meshes
            .iter()
            .map(|m| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("forest material"),
                    layout: &self.material_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&m.base),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.wrap),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(overlay),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::Sampler(&self.wrap),
                        },
                    ],
                })
            })
            .collect();
        self.sets.insert(
            key,
            SetGpu {
                model: model_id.clone(),
                colorize: colorize.cloned(),
                style,
                blocks: BTreeMap::new(),
                groups,
            },
        );
        true
    }

    /// Replaces the trees of one block of a draw (`trees` back to front); the old buffer stays in use
    /// until this call. Empty `trees` take the block away. The set must exist ([`Self::ensure_set`]).
    pub fn set_block(
        &mut self,
        device: &wgpu::Device,
        key: Group,
        block: BlockKey,
        trees: &[TreeInstance],
    ) {
        if trees.is_empty() {
            self.remove_block(key, block);
            return;
        }
        let Some(set) = self.sets.get_mut(&key) else {
            return;
        };
        let model = self.models.get(&set.model);
        let reach = model.map_or((0.0, 0.0), |m| m.reach);
        let (min, max) = bounds_of(trees, reach);
        // The far trees: the same places and scales, the variant of the far mesh; only with the
        // far-view LOD on.
        let far = model
            .and_then(|m| m.lod.as_ref())
            .filter(|_| self.far_enabled)
            .map(|lod| {
                let far: Vec<TreeInstance> = trees
                    .iter()
                    .map(|t| TreeInstance {
                        variant: lod.variant,
                        ..*t
                    })
                    .collect();
                device.buf_init(
                    &wgpu::util::BufferInitDescriptor {
                        label: Some("forest far trees"),
                        contents: bytemuck::cast_slice(&far),
                        usage: wgpu::BufferUsages::VERTEX,
                    },
                    Cluster::Models,
                )
            });
        set.blocks.insert(
            block,
            BlockGpu {
                instances: device.buf_init(
                    &wgpu::util::BufferInitDescriptor {
                        label: Some("forest trees"),
                        contents: bytemuck::cast_slice(trees),
                        usage: wgpu::BufferUsages::VERTEX,
                    },
                    Cluster::Models,
                ),
                far,
                count: trees.len() as u32,
                min,
                max,
            },
        );
    }

    /// Takes a block away; a draw left without blocks goes too.
    pub fn remove_block(&mut self, key: Group, block: BlockKey) {
        if let Some(set) = self.sets.get_mut(&key) {
            set.blocks.remove(&block);
            if set.blocks.is_empty() {
                self.sets.remove(&key);
            }
        }
    }

    /// Trees on the GPU.
    pub fn tree_count(&self) -> u32 {
        self.sets
            .values()
            .flat_map(|s| s.blocks.values())
            .map(|b| b.count)
            .sum()
    }

    /// Blocks on the GPU.
    pub fn block_count(&self) -> usize {
        self.sets.values().map(|s| s.blocks.len()).sum()
    }

    /// True once the last [`Self::frame`] could bind everything and there is something to draw.
    pub fn ready(&self) -> bool {
        self.ready && !self.sets.is_empty()
    }

    pub fn drawn(&self) -> u32 {
        self.drawn.get()
    }

    /// Forgets the count of the last frame.
    pub fn begin_frame(&self) {
        self.drawn.set(0);
    }

    /// The trees into the class mask: `forest_vs` and `forest_mask_ps`,
    /// one sample, the mask's own depth.
    fn make_mask_pipeline(&self, device: &wgpu::Device) -> wgpu::RenderPipeline {
        let (layout, module, entry) = self.vertex_stage();
        let buffers = self.buffers();
        let constants = [("MASK_CLASS", f64::from(crate::frame::mask::class::FOREST))];
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("forest mask"),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
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
            multisample: multisample(1),
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some("forest_mask_ps"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &constants,
                    ..Default::default()
                },
                targets: &[Some(wgpu::ColorTargetState {
                    format: crate::frame::mask::MASK_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    }

    const VERTEX: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Snorm16x2,
        2 => Float16x2,
        3 => Uint32,
    ];
    const INSTANCE: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![4 => Float32x4, 5 => Uint32];
    const INSTANCE_STRIDE: u64 = std::mem::size_of::<TreeInstance>() as u64;

    /// The layout, the module and the vertex entry of the pipelines: `forest_vs_pulled` where the
    /// device has it, else `forest_vs`. The pixel entries are the same in both modules.
    fn vertex_stage(&self) -> (&wgpu::PipelineLayout, &wgpu::ShaderModule, &'static str) {
        match self.pulled.as_ref().filter(|_| self.pull) {
            Some(p) => (&p.pipeline_layout, &p.module, "forest_vs_pulled"),
            None => (&self.pipeline_layout, &self.module, "forest_vs"),
        }
    }

    /// The vertex buffers of the pipelines: the model's vertices (none when they are pulled) and the
    /// trees, always in slot 1.
    fn buffers(&self) -> [Option<wgpu::VertexBufferLayout<'static>>; 2] {
        let trees = Some(wgpu::VertexBufferLayout {
            array_stride: Self::INSTANCE_STRIDE,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::INSTANCE,
        });
        if self.is_pulled() {
            return [None, trees];
        }
        [
            Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<StaticVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &Self::VERTEX,
            }),
            trees,
        ]
    }

    fn make_pipeline(&self, device: &wgpu::Device, reflection: bool) -> wgpu::RenderPipeline {
        let (layout, module, entry) = self.vertex_stage();
        let buffers = self.buffers();
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(if reflection {
                "forest reflection"
            } else {
                "forest"
            }),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // The capture (S1a ForestShader draws): back faces culled, front counter-clockwise.
                // The reflection's mirrored camera turns the winding over, so it culls nothing.
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: if reflection {
                    None
                } else {
                    Some(wgpu::Face::Back)
                },
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: if reflection {
                multisample(1)
            } else {
                multisample(self.samples)
            },
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some(if reflection {
                    "forest_ss_reflect_ps"
                } else {
                    "forest_ss_overlay_ps"
                }),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: WORLD_FORMAT,
                    // The capture blends the leaves: colour SrcAlpha / InvSrcAlpha, alpha One + One.
                    blend: Some(wgpu::BlendState {
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
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    }

    /// The constants and the groups 0 of the frame. Returns whether the trees can be drawn: the
    /// light tables of every style in use are in.
    pub fn frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        f: &ForestFrame<'_>,
    ) -> bool {
        self.ready = false;
        if self.sets.is_empty() {
            return false;
        }
        let Some(ramp) = f.lighting.fog_ramp() else {
            return false;
        };
        self.shifts = f.shifts.iter().copied().take(COPIES).collect();
        if self.shifts.is_empty() {
            self.shifts.push(0.0);
        }
        self.far = f.far_lod.map(|d| (f.eye_orig, d));
        self.planes.clear();
        self.planes.resize(COPIES * 2, [Vec4::ZERO; 6]);
        for (i, &shift) in self.shifts.iter().enumerate() {
            // A copy of the map lies `shift` east: the trees move with it.
            let vp = f.view_proj_orig * Mat4::from_translation(Vec3::X * shift);
            self.planes[i] = frustum_planes(&vp);
            let vs = VsConsts {
                mtx_view_proj: row_major(vp),
                overlay_projection: row_major(f.overlay_projection),
                fow_base_scale: f.fow_base_scale,
                tree_location: [[0.0; 4]; 90],
            };
            queue.write_buffer(&self.vs, i as u64 * VS_STRIDE, bytemuck::bytes_of(&vs));
        }
        // The reflection camera: the same trees through its matrix, in the slots after the copies'.
        self.reflection_on = f.reflect_vp_orig.is_some();
        if let Some(reflect_vp) = f.reflect_vp_orig {
            for (i, &shift) in self.shifts.iter().enumerate() {
                let vp = reflect_vp * Mat4::from_translation(Vec3::X * shift);
                self.planes[COPIES + i] = frustum_planes(&vp);
                let vs = VsConsts {
                    mtx_view_proj: row_major(vp),
                    overlay_projection: row_major(f.overlay_projection),
                    fow_base_scale: f.fow_base_scale,
                    tree_location: [[0.0; 4]; 90],
                };
                queue.write_buffer(
                    &self.vs,
                    (COPIES + i) as u64 * VS_STRIDE,
                    bytemuck::bytes_of(&vs),
                );
            }
        }
        let ps = PsConsts {
            screen_res: [f.screen.0, f.screen.1],
            overlay_multiplier: f.overlay_alpha,
            _pad0: 0.0,
            cam_pos: f.eye_orig.to_array(),
            _pad1: 0.0,
        };
        queue.write_buffer(&self.ps, 0, bytemuck::bytes_of(&ps));
        let mut styles: Vec<usize> = self.sets.values().map(|s| s.style).collect();
        styles.sort_unstable();
        styles.dedup();
        for style in styles {
            let class = style.min(STYLES.len() - 1);
            // t5 of the capture (resource 1920): the table of the terrain; t8 (1935): the table of
            // the fog of war.
            let (Some(correction), Some(trans)) = (
                f.lighting.terrain_color_key(0, class),
                f.lighting.terrain_color_key(2, class),
            ) else {
                return false;
            };
            // The group of a style is made again only when a resource it binds changed.
            let key = GroupKey {
                globals: f.globals.clone(),
                views: [
                    ramp.clone(),
                    correction.clone(),
                    trans.clone(),
                    f.fow_mask.clone(),
                ],
                optional: [
                    f.fow_base.cloned(),
                    f.screen_overlay.cloned(),
                    f.overlay_map.cloned(),
                ],
            };
            if self.group0.contains_key(&style) && self.group0_keys.get(&style) == Some(&key) {
                continue;
            }
            self.group0_keys.insert(style, key);
            let tex = |binding, view| wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(view),
            };
            let smp = |binding, s| wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::Sampler(s),
            };
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("forest frame"),
                layout: &self.group0_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: f.globals.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 11,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &self.vs,
                            offset: 0,
                            size: wgpu::BufferSize::new(VS_BYTES),
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 12,
                        resource: self.ps.as_entire_binding(),
                    },
                    tex(2, ramp),
                    smp(3, &self.clamp),
                    tex(4, correction),
                    smp(5, &self.clamp),
                    tex(6, trans),
                    smp(7, &self.clamp),
                    tex(8, f.fow_mask),
                    // `ForestVS` makes the mask coordinate as `m * (0.5, -0.5) - 0.5`, one whole
                    // texture off the usual `+ 0.5`: it reads the mask through the wrap of D3D9's
                    // default sampler state. Clamped, every tree reads the corner and is killed.
                    smp(9, &self.wrap),
                    tex(13, f.fow_base.unwrap_or(&self.clear)),
                    smp(14, &self.wrap),
                    tex(15, f.screen_overlay.unwrap_or(&self.clear)),
                    smp(16, &self.clamp),
                    tex(17, f.overlay_map.unwrap_or(&self.clear)),
                    smp(18, &self.wrap),
                ],
            });
            self.group0.insert(style, group);
        }
        if self.pipeline.is_none() {
            self.pipeline = Some(self.make_pipeline(device, false));
            self.mask_pipeline = Some(self.make_mask_pipeline(device));
        }
        if self.reflection_on && self.reflection_pipeline.is_none() {
            self.reflection_pipeline = Some(self.make_pipeline(device, true));
        }
        self.ready = true;
        true
    }

    /// Records the trees into the main pass.
    pub fn record(&self, pass: &mut wgpu::RenderPass<'_>, shift: f32, stats: &mut FrameStats) {
        let (true, Some(pipeline)) = (self.ready(), &self.pipeline) else {
            return;
        };
        self.record_with(pass, pipeline, shift, 0, stats);
    }

    /// Records the trees into the class mask.
    pub fn record_mask(&self, pass: &mut wgpu::RenderPass<'_>, shift: f32) {
        let (true, Some(pipeline)) = (self.ready(), &self.mask_pipeline) else {
            return;
        };
        let drawn = self.drawn.get();
        self.record_with(pass, pipeline, shift, 0, &mut FrameStats::default());
        // The count is the main pass's.
        self.drawn.set(drawn);
    }

    /// Records the trees into the reflection pass: `ForestSSReflectPS` under the reflection
    /// camera's matrix.
    pub fn record_reflection(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        shift: f32,
        stats: &mut FrameStats,
    ) {
        let (true, true, Some(pipeline)) =
            (self.ready(), self.reflection_on, &self.reflection_pipeline)
        else {
            return;
        };
        self.record_with(pass, pipeline, shift, COPIES, stats);
    }

    fn record_with(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        pipeline: &wgpu::RenderPipeline,
        shift: f32,
        first_slot: usize,
        stats: &mut FrameStats,
    ) {
        let Some(copy) = self.shifts.iter().position(|&s| (s - shift).abs() < 1e-3) else {
            return;
        };
        let offset = (first_slot + copy) as u32 * VS_STRIDE as u32;
        let mut state = crate::perf::PassState::new();
        state.pipeline(pass, pipeline);
        let planes = self.planes.get(first_slot + copy);
        let tree_bytes = std::mem::size_of::<TreeInstance>() as u64;
        for set in self.sets.values() {
            let (Some(group0), Some(model)) =
                (self.group0.get(&set.style), self.models.get(&set.model))
            else {
                continue;
            };
            // The blocks of the map in view (all of them without the planes of the slot), each its
            // own buffer, in the order they are drawn; with the far-view LOD, whether the block is
            // wholly far (the box of the copy of the map it is drawn in).
            let visible: Vec<(&BlockGpu, bool)> = set
                .blocks
                .values()
                .filter(|b| planes.is_none_or(|p| !outside(p, b.min, b.max)))
                .map(|b| {
                    let far = self.far.is_some_and(|(eye, d)| {
                        let at = Vec3::X * shift;
                        b.far.is_some()
                            && crate::world::far_lod::box_distance(eye, b.min + at, b.max + at) > d
                    });
                    (b, far && model.lod.is_some())
                })
                .collect();
            if visible.is_empty() {
                continue;
            }
            state.group_offsets(pass, 0, group0, &[offset]);
            for (k, (mesh, material)) in model.meshes.iter().zip(&set.groups).enumerate() {
                if mesh.index_count == 0 {
                    continue;
                }
                state.group(pass, 3, material);
                // The pulled path reads the mesh from its group 1; the old one from its buffers.
                let pulled = mesh.pulled.as_ref().filter(|_| self.is_pulled());
                match pulled {
                    Some(p) => state.group(pass, 1, &p.group),
                    None => {
                        state.vertex(pass, 0, &mesh.vertices, 0..mesh.vertices.size());
                        state.index(
                            pass,
                            &mesh.indices,
                            0..mesh.indices.size(),
                            wgpu::IndexFormat::Uint32,
                        );
                    }
                }
                let lod = model.lod.as_ref().and_then(|l| l.ranges.get(k));
                for &(block, far) in &visible {
                    let count = block.count;
                    let (buffer, indices) = match (far, &block.far, lod) {
                        // A far tree: the indices of the far variant alone.
                        (true, Some(b), Some(range)) => (b, range.clone()),
                        _ => (&block.instances, 0..mesh.index_count),
                    };
                    if indices.is_empty() {
                        continue;
                    }
                    let shown = if far {
                        indices.end - indices.start
                    } else {
                        mesh.index_count / model.variants.max(1)
                    };
                    state.vertex(pass, 1, buffer, 0..u64::from(count) * tree_bytes);
                    match pulled {
                        // Each tree runs the run of its own variant (the far ones the far
                        // variant's), padded to the longest.
                        Some(p) => {
                            if p.stride == 0 {
                                continue;
                            }
                            stats.add_counter("tris.forest.submitted", p.stride / 3 * count);
                            pass.draw_c(0..p.stride, 0..count);
                        }
                        None => {
                            // What the vertex stage runs (every variant of a near tree, the far
                            // variant of a far one).
                            stats.add_counter(
                                "tris.forest.submitted",
                                (indices.end - indices.start) / 3 * count,
                            );
                            pass.draw_indexed_c(indices, 0, 0..count);
                        }
                    }
                    // What shows: one variant of each tree.
                    stats.record_draw(shown * count);
                    self.drawn.set(self.drawn.get() + count);
                }
            }
        }
    }
}

/// The resources a style's group 0 binds (besides the buffers of the draw itself), to make the group
/// again only when one of them is another.
#[derive(PartialEq)]
struct GroupKey {
    globals: wgpu::Buffer,
    /// The ramp, the two tables of the style and the fog mask.
    views: [wgpu::TextureView; 4],
    /// The base of the fog, the screen overlay and the map of the overlay.
    optional: [Option<wgpu::TextureView>; 3],
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trees go back to front: the northmost (largest `y` of the original's basis) first.
    #[test]
    fn the_trees_are_drawn_back_to_front() {
        let t = |x: f32, y: f32| TreeInstance {
            location: [x, y, 0.0, 1.0],
            variant: 0,
        };
        let mut trees = vec![t(0.0, 1.0), t(5.0, 9.0), t(2.0, -3.0), t(1.0, 9.0)];
        sort_back_to_front(&mut trees);
        let order: Vec<[f32; 2]> = trees
            .iter()
            .map(|t| [t.location[0], t.location[1]])
            .collect();
        assert_eq!(order, vec![[1.0, 9.0], [5.0, 9.0], [0.0, 1.0], [2.0, -3.0]]);
        assert_eq!(std::mem::size_of::<TreeInstance>(), 20);
    }

    /// The far mesh of a tree model is the variant of the median height, and its indices are one
    /// run of the mesh: the indices of the other variants are not run for a far tree.
    #[test]
    fn the_far_mesh_is_one_typical_variant() {
        use asset_format::{Indices, Material, Mesh, Skeleton};
        // Three variants 10, 30 and 50 tall; the middle one has two triangles.
        let quad = |h: f32, tris: usize| Mesh {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, h, 0.0],
                [0.0, h, 0.0],
            ],
            normals: vec![[0, 0]; 4],
            uvs: vec![[0, 0]; 4],
            joints: vec![],
            weights: vec![],
            indices: Indices::U16([0, 1, 2, 0, 2, 3][..tris * 3].to_vec()),
            material: 0,
        };
        let blob = ModelBlob {
            skeleton: Skeleton::default(),
            meshes: vec![quad(10.0, 1), quad(30.0, 2), quad(50.0, 1)],
            materials: vec![Material {
                diffuse: None,
                specular: None,
            }],
            animations: vec![],
        };
        let g = build_geometry(&blob, Variants::PerMesh);
        let far = far_mesh(&g).expect("the variants are runs");
        assert_eq!(far.variant, 1);
        assert_eq!(far.ranges, vec![3..9]);
        // A model of one variant has nothing to save.
        let one = ModelBlob {
            meshes: vec![quad(10.0, 2)],
            ..blob
        };
        assert!(far_mesh(&build_geometry(&one, Variants::PerMesh)).is_none());
    }

    /// The indices of the pulled path: the longest run first, then each variant's indices in their
    /// order, padded to it.
    #[test]
    fn the_pulled_indices_are_one_padded_run_per_variant() {
        let v = |variant| StaticVertex {
            position: [0.0; 3],
            normal: [0; 2],
            uv: [0; 2],
            variant,
        };
        let vertices = [v(0), v(0), v(0), v(1), v(1), v(1), v(1), v(2), v(2), v(2)];
        let indices = [0, 1, 2, 3, 4, 5, 3, 5, 6, 7, 8, 9];
        let p = pulled_indices(&vertices, &indices, 4);
        let pad = PULLED_PAD;
        assert_eq!(
            p,
            vec![
                6, 0, 1, 2, pad, pad, pad, 3, 4, 5, 3, 5, 6, 7, 8, 9, pad, pad, pad, pad, pad, pad,
                pad, pad, pad
            ]
        );
        assert_eq!(std::mem::size_of::<PulledVertex>(), 20);
        // The f16 of the uv widen exactly: 0, 1, 0.5, the smallest subnormal, -2.
        for (h, f) in [
            (0u16, 0.0f32),
            (0x3C00, 1.0),
            (0x3800, 0.5),
            (1, 2f32.powi(-24)),
            (0xC000, -2.0),
        ] {
            assert_eq!(f16_to_f32(h), f);
        }
    }

    /// `forest_vs_pulled` with the rest of the forest module parses and validates.
    #[test]
    fn the_pulled_module_validates() {
        let text = crate::wgsl_module!(
            "common/frame.wgsl",
            "common/sm3.wgsl",
            "forest/forest.wgsl",
            "forest/forest_pulled.wgsl"
        );
        let parsed = naga::front::wgsl::parse_str(text)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(text)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&parsed)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(text)));
        assert!(
            parsed
                .entry_points
                .iter()
                .any(|e| e.name == "forest_vs_pulled")
        );
    }

    #[test]
    fn a_vertex_goes_to_the_basis_of_the_original() {
        // Up (y) is the original's z; our south (+z) is its -y.
        assert_eq!(to_original([1.0, 2.0, 3.0]), [1.0, -3.0, 2.0]);
        let m = crate::frame::basis_rotation();
        let p = m.transform_point3(Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(p.to_array(), to_original([1.0, 2.0, 3.0]));
    }

    #[cfg(test)]
    mod gpu {
        use super::*;
        use crate::game_camera::GameCamera;
        use crate::gpu_test::{gpu, read_texture};
        use crate::hexgrid::cell_center;
        use crate::lighting::tests::{boot_blobs_with, settings};
        use crate::renderer::Renderer;
        use crate::scene::Scene;
        use crate::terrain::features::FeatureArtSet;
        use crate::terrain::{FeatureKind, Fog, TerrainCell, TerrainMesh, TerrainSnapshot};
        use crate::world::terrain::TerrainBasis;
        use asset_format::{
            ArtTable, FeatureArt, Indices, Material, Mesh, Skeleton, TexDim, TextureFormat,
        };
        use draw_list::UiDrawList;

        fn texel(rgba: [u8; 4]) -> TextureBlob {
            TextureBlob {
                format: TextureFormat::Rgba8,
                dim: TexDim::D2,
                srgb: false,
                width: 1,
                height: 1,
                depth: 1,
                mips: vec![rgba.to_vec()],
            }
        }

        /// A tree of two variants: a standing quad each, 20 wide and 30 tall.
        fn tree() -> ModelBlob {
            let quad = |dx: f32| Mesh {
                positions: vec![
                    [dx - 10.0, 0.0, 0.0],
                    [dx + 10.0, 0.0, 0.0],
                    [dx + 10.0, 30.0, 0.0],
                    [dx - 10.0, 30.0, 0.0],
                ],
                normals: vec![[0, 0]; 4],
                // f16 0 and 1.
                uvs: vec![[0, 0], [0x3C00, 0], [0x3C00, 0x3C00], [0, 0x3C00]],
                joints: vec![],
                weights: vec![],
                indices: Indices::U16(vec![0, 1, 2, 0, 2, 3]),
                material: 0,
            };
            ModelBlob {
                skeleton: Skeleton::default(),
                meshes: vec![quad(0.0), quad(3.0)],
                materials: vec![Material {
                    diffuse: Some(LogicalId::new("tx:tree")),
                    specular: None,
                }],
                animations: vec![],
            }
        }

        /// WOB3: the trees of a forest cell go through `ForestShader` once the fog mask, the light
        /// tables and the art are in: the subsystem says it is native, draws every tree of the
        /// cell, and no pipeline fails validation.
        #[test]
        fn a_forest_cell_is_drawn_through_the_forest_shader() {
            let Some(gpu) = gpu(
                "a_forest_cell_is_drawn_through_the_forest_shader",
                wgpu::Backends::PRIMARY,
                wgpu::Features::empty(),
            ) else {
                return;
            };
            let size = (128u32, 128u32);
            let mut r = Renderer::offscreen(&gpu, size, wgpu::TextureFormat::Rgba8Unorm, 1);
            r.set_boot_required(true);
            // The table of the terrain is red (the ground reads it too); the trees go through it.
            let colour = |file: &str, _: usize| {
                if file.starts_with("terrain_") && file.ends_with("_colorkey.tga") {
                    [255, 0, 0, 255]
                } else {
                    [200, 200, 200, 255]
                }
            };
            for (file, b) in boot_blobs_with(&[], &colour) {
                r.add_lighting_texture(&file, &b).unwrap();
            }
            r.set_world_settings(settings());
            let cell = |feat| TerrainCell {
                fog: Fog::Visible,
                feat,
                ..TerrainCell::default()
            };
            let cells: Vec<TerrainCell> = (0..24)
                .map(|i| {
                    cell(if i == 2 * 6 + 3 {
                        FeatureKind::Forest
                    } else {
                        FeatureKind::None
                    })
                })
                .collect();
            let snapshot = TerrainSnapshot::new(6, 4, false, cells).unwrap();
            r.set_terrain(&snapshot, &TerrainMesh::build(&snapshot));
            r.set_terrain_basis(TerrainBasis::from_snapshot(&snapshot, 1).unwrap());
            let mut table = ArtTable::new();
            table.insert_feature(
                "FEATURE_FOREST",
                0,
                FeatureArt {
                    texture: Some(LogicalId::new("tx:overlay")),
                    tree_model: Some(LogicalId::new("md:tree")),
                    placement: Default::default(),
                },
            );
            let models = [(LogicalId::new("md:tree"), tree())].into();
            let textures = [
                (LogicalId::new("tx:tree"), texel([40, 160, 40, 255])),
                (LogicalId::new("tx:overlay"), texel([128, 128, 128, 0])),
            ]
            .into();
            r.set_feature_art(FeatureArtSet::from_table(&table), &models, &textures);
            let camera = {
                let mut c = GameCamera::new(cell_center(3, 2));
                c.set_level(10.0);
                c
            };
            let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("forest target"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = target.create_view(&wgpu::TextureViewDescriptor::default());
            for i in 0..4 {
                let commands = r.encode_frame(
                    &view,
                    &Scene::test(i as f32 / 30.0),
                    &camera,
                    &UiDrawList::default(),
                    None,
                );
                gpu.queue.submit([commands]);
                r.frame_submitted();
            }
            let forest = &r.world().forest;
            assert!(forest.is_native(), "the trees take the native path");
            let draw = forest.gpu().unwrap();
            assert_eq!(draw.tree_count(), 351, "one hex, 351 trees");
            assert!(draw.drawn() > 0);
            let with = read_texture(&gpu, &target, 0, 0, size, 4);
            // B3: the pulled vertex stage (every tree its own variant's indices) draws the frame
            // of the old one (every variant per tree, the others folded), byte for byte.
            if r.world().forest.gpu().unwrap().is_pulled() {
                for pulled in [false, true] {
                    r.world_mut().forest.gpu_mut().unwrap().set_pulled(pulled);
                    for _ in 0..2 {
                        let commands = r.encode_frame(
                            &view,
                            &Scene::test(3.0 / 30.0),
                            &camera,
                            &UiDrawList::default(),
                            None,
                        );
                        gpu.queue.submit([commands]);
                        r.frame_submitted();
                    }
                    let again = read_texture(&gpu, &target, 0, 0, size, 4);
                    assert!(
                        again == with,
                        "pulled {pulled}: {} bytes differ",
                        again.iter().zip(&with).filter(|(a, b)| a != b).count()
                    );
                }
            }
            // RC-12: the trees are in the reflection too (`ForestSSReflectPS`, the camera of the
            // reflection).
            let mirror = r
                .target(crate::frame::REFLECTION_COLOR)
                .expect("ReflectionColor");
            let lit = read_texture(&gpu, &mirror.texture, 0, 0, mirror.size, 4)
                .chunks(4)
                .filter(|p| p[0] > 8 || p[1] > 8 || p[2] > 8)
                .count();
            assert!(lit > 20, "the trees are in ReflectionColor: {lit} pixels");
            // The same map without the forest: the trees are what differs.
            let plain =
                TerrainSnapshot::new(6, 4, false, vec![cell(FeatureKind::None); 24]).unwrap();
            r.set_terrain(&plain, &TerrainMesh::build(&plain));
            for i in 4..8 {
                let commands = r.encode_frame(
                    &view,
                    &Scene::test(i as f32 / 30.0),
                    &camera,
                    &UiDrawList::default(),
                    None,
                );
                gpu.queue.submit([commands]);
                r.frame_submitted();
            }
            let without = read_texture(&gpu, &target, 0, 0, size, 4);
            let differing = with
                .chunks(4)
                .zip(without.chunks(4))
                .filter(|(a, b)| a != b)
                .count();
            assert!(differing > 200, "the trees changed {differing} pixels");
        }
    }

    #[test]
    fn the_constants_have_the_size_of_the_shader_blocks() {
        assert_eq!(std::mem::size_of::<VsConsts>() as u64, VS_BYTES);
        assert_eq!(std::mem::size_of::<PsConsts>() as u64, PS_BYTES);
    }
}
