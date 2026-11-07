//! The draw of the models of `assets-b2` (WOB1/WOB2): the GPU side of
//! the units. A model is uploaded once (`add_model`): a vertex and an index buffer per mesh as the
//! pipeline stored them (`MeshV2.layout` is the layout of the vertex), a bind group per material
//! (group 3: the base texture and `SREFMap`). A frame is one instance arena (`ObjectInstance`, 64
//! bytes), one draw per mesh and copy of the map (WO-D4), and four passes' worth of calls:
//!
//! | pass | slot | what | pipeline |
//! |---|---|---|---|
//! | main | `Main.Unit` | every mesh that is not invisible | the family's entries, opaque first, then the sorted ones from the farthest |
//! | z-fail | `Main.UnitZFail` | the meshes with `ZFAIL`, where the depth test fails | the same entries under `depth_compare: Greater`, no write |
//! | shadow | `ShadowMap` | the meshes with `SHADOWS` | the `*_Shadow` entries into the raw shadow map |
//! | reflection | `Reflection` | the meshes with `REFLECTIONS`, from the reflection camera, cut at the water plane | the main entries as `*_reflect_ps` twins that cut at the plane (the catalog has no reflection entry of the units, RW-14 (g)) |
//!
//! Pipelines come from the cache of render-core, one per material row (`material::resolve_object`),
//! made at the first frame that draws the mesh. A mesh whose pipeline cannot be made is left out and
//! the cache has said why, once (`Skip`).
//!
//! Group 0 is the frame constants and the light cubes (`unit_family::frame_layout`); the shadow
//! pass has its own constants (the light's view) in the same layout. Group 1 is the bone texture,
//! group 2 the fog of war (`FowMask` and the rectangle it covers), group 3 the material.

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use std::collections::{BTreeMap, HashMap};
use std::ops::Range;

use asset_format::{
    AttrFormat, MeshV2, ModelBlobV2, Semantic as BlobSemantic, TexSource, TextureBlob,
};
use glam::{Vec2, Vec3};

use crate::entities::unit_family::frame_layout;
use crate::frame::{DrawContext, MainSlot, PassSlot};
use crate::lighting::LightingEnv;
use crate::material::families::{UNIT_CODE_PS, UNIT_CODE_REFLECT_PS, resolve_object};
use crate::material::{
    BlendMode, MaterialDesc, PassKind, PipelineCache, PipelineRequest, RenderState, VertexAttr,
    VertexLayout, blends, stencil_only,
};
use crate::perf::PassState;
use crate::renderer::FrameStats;
use crate::shaders::Semantic;
use crate::skin::BoneTexture;
use crate::texture::{UploadOptions, upload_blob};
use crate::world::objects::instance::{ObjectInstance, instance_layout};

use super::cull::{self, CullSet};

/// Pipelines of a mesh by pass: main, z-fail, shadow, reflection.
const PASSES: [PassKind; 4] = [
    PassKind::Main,
    PassKind::ZFail,
    PassKind::Shadow,
    PassKind::Reflection,
];

fn pass_index(pass: PassKind) -> usize {
    match pass {
        PassKind::Main => 0,
        PassKind::ZFail => 1,
        PassKind::Reflection => 3,
        _ => 2,
    }
}

/// Format of the shadow map's raw target.
use crate::shadow::raw_format;

// ---- the layout of a vertex ----

fn semantic(s: BlobSemantic) -> Semantic {
    match s {
        BlobSemantic::Position0 => Semantic::Position0,
        BlobSemantic::Normal0 => Semantic::Normal0,
        BlobSemantic::Texcoord(n) => Semantic::Texcoord(n),
        BlobSemantic::Color(n) => Semantic::Color(n),
        BlobSemantic::Tangent0 => Semantic::Tangent0,
        BlobSemantic::Binormal0 => Semantic::Binormal0,
        BlobSemantic::BlendIndices0 => Semantic::BlendIndices0,
        BlobSemantic::BlendWeight0 => Semantic::BlendWeight0,
    }
}

fn format(f: AttrFormat) -> wgpu::VertexFormat {
    use wgpu::VertexFormat as F;
    match f {
        AttrFormat::F32x3 => F::Float32x3,
        AttrFormat::OctSnorm16x2 => F::Snorm16x2,
        AttrFormat::F16x2 => F::Float16x2,
        AttrFormat::F32x2 => F::Float32x2,
        AttrFormat::U8x4 => F::Uint8x4,
        AttrFormat::U16x4 => F::Uint16x4,
        AttrFormat::Unorm8x4 => F::Unorm8x4,
        AttrFormat::Snorm16x4 => F::Snorm16x4,
    }
}

/// The layout of the vertex rows of `mesh` for the pipeline cache; the `@location`s come from the
/// semantics (render-core 4.6).
pub fn vertex_layout_of(mesh: &MeshV2) -> Result<VertexLayout, String> {
    let stride = mesh.stride().ok_or("the vertex is too wide")? as u64;
    let mut offset = 0;
    let mut attrs = Vec::with_capacity(mesh.layout.len());
    for a in &mesh.layout {
        attrs.push(VertexAttr {
            semantic: semantic(a.semantic),
            format: format(a.format),
            offset,
        });
        offset += a.format.size() as u64;
    }
    VertexLayout::new(stride, attrs).map_err(|e| e.to_string())
}

/// True if the mesh has joints and weights: it is skinned by its vertices.
fn has_skin(mesh: &MeshV2) -> bool {
    mesh.layout
        .iter()
        .any(|a| a.semantic == BlobSemantic::BlendIndices0)
}

/// The sphere the culling uses for a model: the box of its vertices, the one the blob says and the
/// origin of the instance; for an animated model also the rest positions of the bones (a mesh
/// attached to a bone is in the space of the bone) with the reach of such meshes around them.
fn model_sphere(blob: &ModelBlobV2) -> cull::ModelSphere {
    let mut lo = Vec3::ZERO;
    let mut hi = Vec3::ZERO;
    let mut grow = |p: Vec3| {
        lo = lo.min(p);
        hi = hi.max(p);
    };
    let computed = blob.compute_bounds();
    for b in [computed, blob.bounds] {
        if b.iter().all(|v| v.is_finite()) {
            grow(Vec3::new(b[0], b[1], b[2]));
            grow(Vec3::new(b[3], b[4], b[5]));
        }
    }
    let animated = blob.meshes.iter().any(|m| has_skin(m) || m.bind.is_some());
    if animated && !blob.skeleton.bones.is_empty() {
        let rest = blob.skeleton.rest_pose();
        let mut world = vec![glam::Mat4::IDENTITY; rest.len()];
        blob.skeleton.world_matrices(&rest, &mut world);
        let bones: Vec<Vec3> = world.iter().map(|m| m.w_axis.truncate()).collect();
        for p in &bones {
            grow(*p);
        }
        for m in &blob.meshes {
            let (Some(k), Some(points)) = (m.bind, m.positions()) else {
                continue;
            };
            let reach = points
                .iter()
                .fold(0.0f32, |r, p| r.max(Vec3::from(*p).length()));
            if let Some(&at) = bones.get(usize::from(k)) {
                grow(at - Vec3::splat(reach));
                grow(at + Vec3::splat(reach));
            }
        }
    }
    cull::ModelSphere::of_box([lo.x, lo.y, lo.z, hi.x, hi.y, hi.z], animated)
}

// ---- a model on the GPU ----

struct MaterialGpu {
    desc: MaterialDesc,
    /// An `FXShader` effect: its pipeline has the empty fog group ([`Layouts::fx_fow`]).
    fx: bool,
    group: wgpu::BindGroup,
    /// The base, the SREF and the cube of a material that is not an effect, to make the group again
    /// with another colour volume ([`UnitDraw::set_color_key`]).
    views: Option<[wgpu::TextureView; 3]>,
    /// The `Irradiance` cube the material names, for a draw that honours it
    /// ([`UnitDraw::set_material_irradiance`]): its groups 0 are in [`UnitDraw::irradiance_groups`].
    irradiance: Option<asset_format::LogicalId>,
}

struct MeshGpu {
    vertices: Buf,
    indices: Buf,
    index_count: u32,
    index_format: wgpu::IndexFormat,
    layout: VertexLayout,
    material: usize,
    bind: Option<u16>,
    /// The part of the model the mesh belongs to: a landmark draws its parts one by one.
    part: u16,
    /// `MeshV2::states`.
    states: u8,
    skinned: bool,
    /// Main, z-fail, shadow. `None` until asked for; `failed` when the cache refused.
    pipes: [Option<wgpu::RenderPipeline>; 4],
    failed: [bool; 4],
}

struct ModelGpu {
    meshes: Vec<MeshGpu>,
    materials: Vec<MaterialGpu>,
    /// For the culling of the instances.
    sphere: cull::ModelSphere,
}

/// Where the textures of the models are: the views, by the id of the blob.
type TextureViews = HashMap<asset_format::LogicalId, wgpu::TextureView>;

// ---- the arena of instances ----

/// One buffer of `ObjectInstance`s for the frame. The frame stages its instances, `flush` writes
/// the runs that differ from the last frame's and grows the buffer when it is too small.
struct InstanceArena {
    buffer: Buf,
    capacity: u32,
    staged: Vec<ObjectInstance>,
    /// What the buffer holds from the last flush (the first `uploaded.len()` instances).
    uploaded: Vec<ObjectInstance>,
    /// Instances written by the last flush.
    written: u32,
}

/// Runs closer than this many instances are written as one.
const UPLOAD_GAP: usize = 16;
/// More runs than this are written as one span from the first change to the last.
const UPLOAD_RUNS: usize = 32;

/// The runs of `new` that differ from `old` (compared byte by byte; instances past the end of `old`
/// are all new). Runs less than `gap` apart are merged; with more than `max_runs` of them the
/// result is one span.
#[allow(clippy::single_range_in_vec_init)]
fn dirty_runs(
    old: &[ObjectInstance],
    new: &[ObjectInstance],
    gap: usize,
    max_runs: usize,
) -> Vec<Range<usize>> {
    let mut runs: Vec<Range<usize>> = Vec::new();
    for (i, item) in new.iter().enumerate() {
        let same = old
            .get(i)
            .is_some_and(|o| bytemuck::bytes_of(o) == bytemuck::bytes_of(item));
        if same {
            continue;
        }
        match runs.last_mut() {
            Some(last) if i - last.end < gap => last.end = i + 1,
            _ => runs.push(i..i + 1),
        }
    }
    if runs.len() > max_runs.max(1)
        && let (Some(first), Some(last)) = (runs.first(), runs.last())
    {
        return vec![first.start..last.end];
    }
    runs
}

impl InstanceArena {
    const MIN: u32 = 256;

    fn new(device: &wgpu::Device) -> Self {
        Self {
            buffer: Self::make(device, Self::MIN),
            capacity: Self::MIN,
            staged: Vec::new(),
            uploaded: Vec::new(),
            written: 0,
        }
    }

    fn make(device: &wgpu::Device, capacity: u32) -> Buf {
        device.buf(
            &wgpu::BufferDescriptor {
                label: Some("object instances"),
                size: u64::from(capacity) * ObjectInstance::STRIDE,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Models,
        )
    }

    fn begin(&mut self) {
        self.staged.clear();
    }

    fn flush(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let need = self.staged.len() as u32;
        if need > self.capacity {
            self.capacity = need.next_power_of_two();
            self.buffer = Self::make(device, self.capacity);
            // A new buffer holds nothing: everything is written.
            self.uploaded.clear();
        }
        let runs = dirty_runs(&self.uploaded, &self.staged, UPLOAD_GAP, UPLOAD_RUNS);
        self.written = 0;
        self.uploaded
            .resize(self.staged.len(), ObjectInstance::default());
        for run in runs {
            let data = &self.staged[run.clone()];
            queue.write_buffer(
                &self.buffer,
                run.start as u64 * ObjectInstance::STRIDE,
                bytemuck::cast_slice(data),
            );
            self.uploaded[run.clone()].copy_from_slice(data);
            self.written += data.len() as u32;
        }
    }

    fn slice(&self, range: &Range<u32>) -> wgpu::BufferSlice<'_> {
        let stride = ObjectInstance::STRIDE;
        self.buffer
            .slice(u64::from(range.start) * stride..u64::from(range.end) * stride)
    }
}

// ---- the calls of a frame ----

/// One mesh of one model, with the instances it is drawn for in each map copy.
struct Call {
    model: usize,
    mesh: usize,
    /// The instances by the X shift of the copy that holds them.
    ranges: Vec<(f32, Range<u32>)>,
    /// From the eye to the middle of the instances, for the sort of the transparent ones.
    distance: f32,
    transparent: bool,
}

#[derive(Default)]
struct Lists {
    main: Vec<Call>,
    zfail: Vec<Call>,
    shadow: Vec<Call>,
    reflection: Vec<Call>,
}

/// Instances of single parts of models, by (model, part).
pub type PartBatches = BTreeMap<(asset_format::LogicalId, u16), Vec<ObjectInstance>>;

/// Instances of models in a node state other than the intact one, by (model, desired-state byte):
/// a damaged ship (R5.11). Only the meshes whose state mask has a bit of the byte are drawn.
pub type StateBatches = BTreeMap<(asset_format::LogicalId, u8), Vec<ObjectInstance>>;

/// Whether the meshes of a material go into the shadow map. The `Shadows` property of the material
/// says so for the buildings and the landmarks; a unit's is 0 in the gr2 and the capture's shadow
/// pass draws the unit meshes all the same (`UnitSkinnedVS_Shadow`: 181 draws of the 830 instances
/// of the main pass in S3), so the `UnitShader` family always casts.
// re: S3 capture, events 466..: the `*_Shadow` draws of `UnitShader` run for every unit mesh.
fn casts_shadow(desc: &MaterialDesc) -> bool {
    desc.has(MaterialDesc::SHADOWS) || desc.package.starts_with("UnitShader")
}

/// Instances of single parts of models in a node state other than the intact one, by (model, part,
/// desired-state byte): a landmark whose parts stand one by one on an improved plot.
pub type PartStateBatches = BTreeMap<(asset_format::LogicalId, u16, u8), Vec<ObjectInstance>>;

/// The meshes of a model drawn for the desired-state byte `desired` (`FGXModelNodeStateSorter`).
// re: FGXModelNodeStateSorter::IsInCorrectState 0x08cf4532: `(desired[member] & node.stateMask) != 0`
pub fn shows_in(mesh_states: u8, desired: u8) -> bool {
    mesh_states & desired != 0
}

/// What `UnitDraw::build` reads of the frame.
pub struct BuildInputs<'a> {
    /// The instances by model (`Unit::frame_instances`).
    pub batches: &'a BTreeMap<asset_format::LogicalId, Vec<ObjectInstance>>,
    /// The instances of single parts of a model, by model and part: a landmark whose parts stand on
    /// the plot one by one (`Landmark::frame_instances`). Only the meshes of that part are drawn.
    pub parts: &'a PartBatches,
    /// The instances of models in another node state (the batches above are the intact state).
    pub states: &'a StateBatches,
    /// The same for single parts of a model (a landmark's parts on an improved plot).
    pub part_states: &'a PartStateBatches,
    /// The X shifts of the copies of the map that reach the screen.
    pub shifts: &'a [f32],
    pub eye: Vec3,
    /// Whether a model has clips: the meshes that need the bone texture are drawn for these only.
    pub animated: &'a dyn Fn(&asset_format::LogicalId) -> bool,
    /// Keep a copy of the instances and skip the next build when they, the views and the draw are
    /// the same (B3 packet F): for the callers whose instances rarely change (landmarks, cities;
    /// with clocked references an animated one does not change from frame to frame). The units
    /// change somewhere nearly every frame and leave it off.
    pub remember: bool,
    /// Changes when the models of the animation change (what `animated` says).
    pub models_revision: u64,
}

/// What a remembered build was made of, besides the instances.
#[derive(Clone, PartialEq)]
struct BuildKey {
    revision: u64,
    models_revision: u64,
    cull: CullSet,
    shifts: Vec<f32>,
    eye: Vec3,
    bones: Option<u64>,
}

/// The inputs of the last build, kept when the caller asked to `remember`.
struct Remembered {
    key: BuildKey,
    batches: BTreeMap<asset_format::LogicalId, Vec<ObjectInstance>>,
    parts: PartBatches,
    states: StateBatches,
    part_states: PartStateBatches,
}

/// What `build` did, for the counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BuildReport {
    /// Instances the draw was given (times the copies of the map), before the culling.
    pub instances: u32,
    /// Instances staged for some pass; the rest were culled from all of them.
    pub staged: u32,
    /// Instances each pass draws (per mesh the draw repeats them): main and z-fail, shadow, reflection.
    pub in_main: u32,
    pub in_shadow: u32,
    pub in_reflection: u32,
    pub calls: u32,
    /// Instances the flush wrote to the arena (those that changed since the last frame); a
    /// instance is `ObjectInstance::STRIDE` bytes.
    pub uploaded: u32,
    /// Meshes left out because their pipeline cannot be made.
    pub failed: u32,
}

/// Where the fog mask is and what it covers; `None` for no mask (the models are not dimmed).
#[derive(Clone, Copy)]
pub struct FowSource<'a> {
    pub view: &'a wgpu::TextureView,
    /// The corner and the size of the rectangle, in the original's basis.
    pub min: Vec2,
    pub size: Vec2,
}

/// Group 3: `BaseTextureMap` and `SREFMap` with their samplers (0..3), and the cube of the material,
/// `IrradianceCubeMap` of the `UIShader` models (4, 5). A material with no cube binds a black one.
/// An `FXShader` effect has its own layout (`fx`): the base and its sampler (0, 1) and the textures
/// of its layers ([`FX_LAYER_BINDINGS`], 6..14, no sampler of their own: the file reads all of them
/// with the sampler 1), no `SREFMap` and no cube. Apart, and with a group 2 of no entries
/// ([`Layouts::fx_fow`]: the effect reads no fog), so that no pipeline holds more than the 16
/// textures of WebGL2 (the GL backend numbers the textures of the whole pipeline layout, a 17th
/// one is an index out of its table of 16 slots and a panic): the frame holds 4, the bones 1, the
/// fog 3, the effect's material 9. [`pipeline_textures`] counts them, a test holds the limit.
fn material_layout(device: &wgpu::Device, fx: bool) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(if fx {
            "object fx material layout"
        } else {
            "object material layout"
        }),
        entries: &material_entries(fx),
    })
}

/// The entries of [`material_layout`].
fn material_entries(fx: bool) -> Vec<wgpu::BindGroupLayoutEntry> {
    let texture = |binding, dim| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: dim,
            multisampled: false,
        },
        count: None,
    };
    let sampler = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    };
    use wgpu::TextureViewDimension::{Cube, D2};
    if fx {
        [texture(0, D2), sampler(1)]
            .into_iter()
            .chain(
                FX_LAYER_BINDINGS
                    .iter()
                    .map(|&(binding, _)| texture(binding, D2)),
            )
            .collect()
    } else {
        vec![
            texture(0, D2),
            sampler(1),
            texture(2, D2),
            sampler(3),
            texture(4, Cube),
            sampler(5),
            // `ColorCorrection` of `BuildingPS_CODE_Opaque`: the colour volume of the city's style.
            texture(6, wgpu::TextureViewDimension::D3),
            sampler(7),
        ]
    }
}

/// The textures of the layers 0 (scroll and alpha), 1 and 2 of `FXShader` in the material group, by
/// binding: `fx.wgsl` names them. The base of the layer 0 is the binding 0.
const FX_LAYER_BINDINGS: [(u32, &str); 8] = [
    (6, "UVScrollingMap0"),
    (7, "AlphaLookupMap0"),
    (8, "BaseTextureMap1"),
    (9, "UVScrollingMap1"),
    (10, "AlphaLookupMap1"),
    (11, "BaseTextureMap2"),
    (12, "UVScrollingMap2"),
    (13, "AlphaLookupMap2"),
];

/// The bind group layouts the draws share.
struct Layouts {
    frame: wgpu::BindGroupLayout,
    bones: wgpu::BindGroupLayout,
    fow: wgpu::BindGroupLayout,
    /// The group 2 of an effect pipeline: no entries (`fx.wgsl` reads no fog), which keeps the
    /// effect's nine textures within the 16 of WebGL2 ([`pipeline_textures`]).
    fx_fow: wgpu::BindGroupLayout,
    material: wgpu::BindGroupLayout,
    fx_material: wgpu::BindGroupLayout,
}

/// Textures of the pipeline layout `frame`, `bones`, `fow`, `material`: the GL backend gives every
/// texture of the layout a slot of its own, 16 of them (`downlevel_webgl2_defaults`).
#[cfg(test)]
fn pipeline_textures(fx: bool) -> usize {
    let count = |e: &[wgpu::BindGroupLayoutEntry]| {
        e.iter()
            .filter(|e| matches!(e.ty, wgpu::BindingType::Texture { .. }))
            .count()
    };
    count(&crate::entities::unit_family::frame_entries())
        + count(&[BoneTexture::layout_entry()])
        + if fx { 0 } else { count(&fow_entries()) }
        + count(&material_entries(fx))
}

/// The entries of the fog group of the objects.
fn fow_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    let texture = |binding, dim| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: dim,
            multisampled: false,
        },
        count: None,
    };
    let sampler = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    };
    let uniform = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    use wgpu::TextureViewDimension::{D2, D3};
    vec![
        texture(1, D2),
        sampler(2),
        uniform(3),
        // The resources of `BuildingPS_CODE_Opaque` (the buildings of the cities; its
        // `FOWMask` is the binding 1): the base layer, the colour key of the fog of war, the
        // constants of the stage and `f4FOWBaseScale`.
        texture(6, D2),
        sampler(7),
        texture(8, D3),
        sampler(9),
        uniform(10),
        uniform(11),
    ]
}

impl Layouts {
    fn new(device: &wgpu::Device) -> Self {
        let fow = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("object fow layout"),
            entries: &fow_entries(),
        });
        let fx_fow = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("object fx fow layout"),
            entries: &[],
        });
        let bones = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("object bones layout"),
            entries: &[BoneTexture::layout_entry()],
        });
        Self {
            frame: frame_layout(device),
            bones,
            fow,
            fx_fow,
            material: material_layout(device, false),
            fx_material: material_layout(device, true),
        }
    }
}

/// The formats the pipelines are made for.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Targets {
    colour: wgpu::TextureFormat,
    depth: wgpu::TextureFormat,
    samples: u32,
}

/// The GPU side of the units: models, pipelines, the arena, the groups and the calls of the frame.
pub struct UnitDraw {
    layouts: Layouts,
    sampler: wgpu::Sampler,
    white: wgpu::TextureView,
    black: wgpu::TextureView,
    black_cube: wgpu::TextureView,
    textures: TextureViews,
    models: Vec<ModelGpu>,
    index: HashMap<asset_format::LogicalId, usize>,
    cache: PipelineCache,
    targets: Targets,
    arena: InstanceArena,
    /// The frame constants of the main view and of the light, and the light cubes.
    frame_group: Option<wgpu::BindGroup>,
    shadow_group: Option<wgpu::BindGroup>,
    shadow_globals: Buf,
    /// Group 0 of the reflection pass: the frame constants of the reflection camera.
    reflection_group: Option<wgpu::BindGroup>,
    bones_group: Option<(u64, wgpu::BindGroup)>,
    fow_group: Option<wgpu::BindGroup>,
    /// The group 2 of the effects' pipelines: empty.
    fx_fow_group: wgpu::BindGroup,
    fow_rect: Buf,
    /// What `fow_rect` holds and the views `fow_group` binds: written and made again on a change.
    fow_rect_last: Option<[f32; 4]>,
    fow_group_key: Option<FowGroupKey>,
    lists: Lists,
    report: BuildReport,
    /// The volumes the passes cull the instances against ([`UnitDraw::set_cull`]).
    cull: std::cell::Cell<CullSet>,
    /// Scratch of `build`: the passes each instance of a model is in.
    masks: Vec<u8>,
    /// The slot of the main pass the draw records: the units' or the landmarks'.
    slot: MainSlot,
    /// For the pipelines of the class mask, made at the first mask frame (`golden` only).
    device: wgpu::Device,
    mask_cache: std::cell::RefCell<PipelineCache>,
    /// The `BuildingShader` meshes draw with the ported `BuildingPS_CODE_Opaque` (the cities).
    building_code: bool,
    code: CodeResources,
    /// The `BuildingShader` meshes read the `Irradiance` cube of their material instead of the unit's
    /// one (the landmarks).
    material_irradiance: bool,
    /// The models whose ground meshes ([`crate::material::ground_decal`], rigid) the terrain bakes
    /// as decals: they are not drawn here (the landmarks of the plots).
    ground_decal_models: std::collections::HashSet<asset_format::LogicalId>,
    /// The textures that are cubes, by id (the material cube of a building is one of them).
    cubes: std::collections::HashSet<asset_format::LogicalId>,
    /// What the groups 0 of a material cube are made of (from `bind_light`): the dull and the sharp
    /// cube, the fog ramp, the constants of the main and of the reflection view.
    light: Option<LightViews>,
    /// The groups 0 of the main and of the reflection pass with the material cube `id` in the place
    /// of the unit's irradiance.
    irradiance_groups: HashMap<asset_format::LogicalId, (wgpu::BindGroup, wgpu::BindGroup)>,
    /// The groups 0 of the main and of the reflection pass of a model with cubes of its own
    /// ([`UnitDraw::set_model_light`]), by the index of the model.
    model_light: HashMap<usize, (wgpu::BindGroup, wgpu::BindGroup)>,
    /// Bumped by what changes the lists a build makes (models, targets, switches, groups).
    revision: u64,
    /// The last build's inputs when its caller asked to remember them and it was complete.
    remembered: Option<Remembered>,
}

type FowGroupKey = (
    wgpu::TextureView,
    Option<wgpu::TextureView>,
    Option<wgpu::TextureView>,
);

struct LightViews {
    dull: wgpu::TextureView,
    sharp: wgpu::TextureView,
    ramp: wgpu::TextureView,
    frame_globals: wgpu::Buffer,
    reflection_globals: wgpu::Buffer,
}

/// What `BuildingPS_CODE_Opaque` reads besides the frame and the material: the colour volumes
/// (`ColorCorrection` by model, `FOWColorKey`), the base layer of the fog of war and the constants.
struct CodeResources {
    /// A colour volume that gives back what it is given, for a model without a style of its own.
    identity: wgpu::TextureView,
    sampler: wgpu::Sampler,
    /// `ColorCorrection` of the models that have one.
    keys: HashMap<asset_format::LogicalId, wgpu::TextureView>,
    /// The sRGB copies of the textures of the building models.
    srgb: TextureViews,
    fow_key: Option<wgpu::TextureView>,
    fow_base: Option<wgpu::TextureView>,
    /// `BuildingCodeConsts` (`v3CameraWorld`, `fZBias`) and `BuildingFowBase` (`f4FOWBaseScale`).
    consts: Buf,
    fow_base_scale: Buf,
}

/// The side of [`CodeResources::identity`].
const IDENTITY_SIDE: u32 = 16;

/// What the frame gives the ported colour pass of the buildings (`UnitDraw::set_code_frame`).
pub struct CodeFrame<'a> {
    /// The base layer of the fog of war (the clouds); white without one.
    pub fow_base: Option<&'a wgpu::TextureView>,
    /// `f4FOWBaseScale`: the scroll and the repeat of the base layer.
    pub fow_base_scale: [f32; 4],
    /// `FOWColorKey`; the identity without one.
    pub fow_key: Option<&'a wgpu::TextureView>,
    /// `v3CameraWorld`: the eye in the original's basis.
    pub eye_orig: Vec3,
}

/// `fZBias` of `BuildingPS_CODE_Opaque` (the scale of the height in the distance fog): the value the
/// game sets is not read; 1 keeps the height as it is.
// TODO: the `fZBias` of the captures (the constants of the S3 draws of `BuildingPS_CODE_Opaque`).
const Z_BIAS: f32 = 1.0;

/// A colour volume of `side`³ whose texel (r, g, b) is its own place.
fn identity_volume(device: &wgpu::Device, queue: &wgpu::Queue, side: u32) -> wgpu::TextureView {
    let n = side as usize;
    let level = |i: usize| (i * 255 / (n - 1)) as u8;
    let mut data = Vec::with_capacity(n * n * n * 4);
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                data.extend_from_slice(&[level(r), level(g), level(b), 255]);
            }
        }
    }
    device
        .tex_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("object identity volume"),
                size: wgpu::Extent3d {
                    width: side,
                    height: side,
                    depth_or_array_layers: side,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &data,
            Cluster::Models,
        )
        .create_view(&wgpu::TextureViewDescriptor::default())
}

/// A cube of one black texel a face: what a material with no cube samples.
fn black_cube(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
    device
        .tex_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("object black cube"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 6,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &[0, 0, 0, 255].repeat(6),
            Cluster::Models,
        )
        .create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::Cube),
            ..Default::default()
        })
}

fn solid(device: &wgpu::Device, queue: &wgpu::Queue, rgba: [u8; 4]) -> wgpu::TextureView {
    device
        .tex_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("object solid"),
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

impl UnitDraw {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        colour: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        samples: u32,
    ) -> Self {
        // SIMPLIFIED(SIMP-exe-1031): the god ray reads its layers with this sampler too.
        // Linear filter and repeat, as B1's units (the table of sampler states of the original has
        // no row for them yet: `material::sampler::ORIGINAL_SAMPLERS`).
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("object sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let shadow_globals = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("object shadow globals"),
                size: std::mem::size_of::<crate::lighting::globals::FrameGlobals>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        let fow_rect = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("object fow rect"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        let small = |label| {
            device.buf(
                &wgpu::BufferDescriptor {
                    label: Some(label),
                    size: 16,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                },
                Cluster::Other,
            )
        };
        let code = CodeResources {
            identity: identity_volume(device, queue, IDENTITY_SIDE),
            // The colour volumes are looked up by colour: clamped, filtered.
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("object colour volume sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            keys: HashMap::new(),
            srgb: HashMap::new(),
            fow_key: None,
            fow_base: None,
            consts: small("object building code consts"),
            fow_base_scale: small("object fow base scale"),
        };
        let layouts = Layouts::new(device);
        Self {
            fx_fow_group: device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("object fx fow"),
                layout: &layouts.fx_fow,
                entries: &[],
            }),
            layouts,
            sampler,
            white: solid(device, queue, [255; 4]),
            black: solid(device, queue, [0, 0, 0, 255]),
            black_cube: black_cube(device, queue),
            textures: HashMap::new(),
            models: Vec::new(),
            index: HashMap::new(),
            cache: PipelineCache::new(),
            targets: Targets {
                colour,
                depth,
                samples,
            },
            arena: InstanceArena::new(device),
            frame_group: None,
            shadow_group: None,
            shadow_globals,
            reflection_group: None,
            bones_group: None,
            fow_group: None,
            fow_rect,
            fow_rect_last: None,
            fow_group_key: None,
            lists: Lists::default(),
            report: BuildReport::default(),
            cull: std::cell::Cell::new(CullSet::default()),
            masks: Vec::new(),
            slot: MainSlot::Unit,
            device: device.clone(),
            mask_cache: std::cell::RefCell::new(PipelineCache::new()),
            building_code: false,
            code,
            material_irradiance: false,
            ground_decal_models: std::collections::HashSet::new(),
            cubes: std::collections::HashSet::new(),
            light: None,
            irradiance_groups: HashMap::new(),
            model_light: HashMap::new(),
            revision: 0,
            remembered: None,
        }
    }

    /// The model `id` is lit by cubes of its own instead of the unit's: `irradiance` (binding 2),
    /// `soft` (binding 4, `SpecSoft` of `BuildingShader`) and `sharp` (binding 6, `SpecSharp`), for
    /// all its meshes (the houses of the cities). Does nothing before [`Self::bind_light`] or before
    /// the model is here.
    pub fn set_model_light(
        &mut self,
        device: &wgpu::Device,
        id: &asset_format::LogicalId,
        irradiance: &wgpu::TextureView,
        soft: &wgpu::TextureView,
        sharp: &wgpu::TextureView,
    ) {
        self.revision += 1;
        let (Some(&mi), Some(light)) = (self.index.get(id), &self.light) else {
            return;
        };
        let group = |label: &str, globals: &wgpu::Buffer| {
            let tex = |binding, view| wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(view),
            };
            let smp = |binding| wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::Sampler(&self.sampler),
            };
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &self.layouts.frame,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: globals.as_entire_binding(),
                    },
                    tex(2, irradiance),
                    smp(3),
                    tex(4, soft),
                    smp(5),
                    tex(6, sharp),
                    smp(7),
                    tex(8, &light.ramp),
                    smp(9),
                ],
            })
        };
        let groups = (
            group("object frame (model light)", &light.frame_globals),
            group(
                "object reflection frame (model light)",
                &light.reflection_globals,
            ),
        );
        self.model_light.insert(mi, groups);
    }

    /// True if the model `id` has cubes of its own ([`Self::set_model_light`]).
    pub fn has_model_light(&self, id: &asset_format::LogicalId) -> bool {
        self.index
            .get(id)
            .is_some_and(|mi| self.model_light.contains_key(mi))
    }

    /// The `BuildingShader` meshes of this draw are lit by the `Irradiance` cube their material
    /// names (`Improvements_Cubelight` of the wheat, `Unit_Light` of the camps), as the FGX block of
    /// the model sets it, instead of the unit's cube. Call it before the models come.
    pub fn set_material_irradiance(&mut self, on: bool) {
        self.revision += 1;
        self.material_irradiance = on;
    }

    /// The ground meshes of the model `id` are decals of the terrain from now on (`true`) and are
    /// not drawn here, or are drawn again (`false`).
    pub fn set_ground_decals(&mut self, id: &asset_format::LogicalId, on: bool) {
        self.revision += 1;
        if on {
            self.ground_decal_models.insert(id.clone());
        } else {
            self.ground_decal_models.remove(id);
        }
    }

    /// The `BuildingShader` meshes of this draw take the ported colour pass of the D3D11 build,
    /// `BuildingPS_CODE_Opaque` (the draw of the cities); the pipelines made so far are made again.
    pub fn set_building_code(&mut self, on: bool) {
        self.revision += 1;
        if self.building_code == on {
            return;
        }
        self.building_code = on;
        for m in &mut self.models {
            for mesh in &mut m.meshes {
                mesh.pipes = Default::default();
                mesh.failed = [false; 4];
            }
        }
    }

    /// `ColorCorrection` of the model `id` (the colour volume of its style), for its materials now and
    /// when it comes again.
    pub fn set_color_key(
        &mut self,
        device: &wgpu::Device,
        id: &asset_format::LogicalId,
        view: &wgpu::TextureView,
    ) {
        self.revision += 1;
        self.code.keys.insert(id.clone(), view.clone());
        let Some(&mi) = self.index.get(id) else {
            return;
        };
        let groups: Vec<Option<wgpu::BindGroup>> = self.models[mi]
            .materials
            .iter()
            .map(|m| {
                m.views
                    .as_ref()
                    .map(|[base, sref, cube]| self.material_group(device, base, sref, cube, view))
            })
            .collect();
        for (m, g) in self.models[mi].materials.iter_mut().zip(groups) {
            if let Some(g) = g {
                m.group = g;
            }
        }
    }

    /// True if the model `id` has its colour volume.
    pub fn has_color_key(&self, id: &asset_format::LogicalId) -> bool {
        self.code.keys.contains_key(id)
    }

    /// The inputs of the frame for `BuildingPS_CODE_Opaque`; the next [`Self::bind_fow`] binds them.
    pub fn set_code_frame(&mut self, queue: &wgpu::Queue, f: &CodeFrame<'_>) {
        let consts = [f.eye_orig.x, f.eye_orig.y, f.eye_orig.z, Z_BIAS];
        queue.write_buffer(&self.code.consts, 0, bytemuck::cast_slice(&consts));
        queue.write_buffer(
            &self.code.fow_base_scale,
            0,
            bytemuck::cast_slice(&f.fow_base_scale),
        );
        self.code.fow_base = f.fow_base.cloned();
        self.code.fow_key = f.fow_key.cloned();
    }

    /// The group 3 of a material that is not an effect.
    fn material_group(
        &self,
        device: &wgpu::Device,
        base: &wgpu::TextureView,
        sref: &wgpu::TextureView,
        cube: &wgpu::TextureView,
        key: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        let tex = |binding, view| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::TextureView(view),
        };
        let smp = |binding, s| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::Sampler(s),
        };
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("object material"),
            layout: &self.layouts.material,
            entries: &[
                tex(0, base),
                smp(1, &self.sampler),
                tex(2, sref),
                smp(3, &self.sampler),
                tex(4, cube),
                smp(5, &self.sampler),
                tex(6, key),
                smp(7, &self.code.sampler),
            ],
        })
    }

    /// The slot of the main pass this draw records (`Unit` unless it is set): a second draw of the
    /// same models and shaders serves `Main.Landmark`.
    pub fn set_main_slot(&mut self, slot: MainSlot) {
        self.revision += 1;
        self.slot = slot;
    }

    /// The buffer of the frame constants for the shadow pass: the renderer writes
    /// `FrameGlobals` of the light's view into it every frame.
    pub fn shadow_globals(&self) -> &wgpu::Buffer {
        &self.shadow_globals
    }

    pub fn has_model(&self, id: &asset_format::LogicalId) -> bool {
        self.index.contains_key(id)
    }

    pub fn models(&self) -> usize {
        self.models.len()
    }

    pub fn pipelines(&self) -> usize {
        self.cache.len()
    }

    /// The volumes the next `build` culls the instances against (the renderer says them every
    /// frame; the default keeps everything).
    pub fn set_cull(&self, cull: CullSet) {
        self.cull.set(cull);
    }

    pub fn report(&self) -> BuildReport {
        self.report
    }

    /// The pass formats changed (MSAA): the pipelines are made again at their next use.
    pub fn set_targets(
        &mut self,
        colour: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        samples: u32,
    ) {
        self.revision += 1;
        let now = Targets {
            colour,
            depth,
            samples,
        };
        if now == self.targets {
            return;
        }
        self.targets = now;
        self.cache = PipelineCache::new();
        for m in &mut self.models {
            for mesh in &mut m.meshes {
                mesh.pipes = Default::default();
                mesh.failed = [false; 4];
            }
        }
    }

    /// The textures a model needs: the files its materials name.
    pub fn textures_of(blob: &ModelBlobV2) -> Vec<asset_format::LogicalId> {
        let mut ids: Vec<_> = blob
            .materials
            .iter()
            .flat_map(|m| m.texture_files().cloned())
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// Uploads a model with the textures its materials name (`textures`; a missing one is white,
    /// and the log says so). A model with the same id is replaced.
    pub fn add_model(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        id: asset_format::LogicalId,
        blob: &ModelBlobV2,
        textures: &HashMap<asset_format::LogicalId, TextureBlob>,
    ) {
        self.revision += 1;
        for (tid, t) in textures {
            if self.textures.contains_key(tid) {
                continue;
            }
            match upload_blob(device, queue, t, tid.as_str(), UploadOptions::WORLD) {
                Ok(up) => {
                    let view = up.texture.create_view(&wgpu::TextureViewDescriptor {
                        dimension: Some(up.view_dimension),
                        ..Default::default()
                    });
                    if up.view_dimension == wgpu::TextureViewDimension::Cube {
                        self.cubes.insert(tid.clone());
                    }
                    self.textures.insert(tid.clone(), view);
                }
                Err(why) => {
                    tracing::warn!("texture {}: {why}; the material is white", tid.as_str())
                }
            }
        }
        // The D3D11 build samples the diffuse and the SREF of `BuildingPS_CODE` through sRGB views
        // (S3 ev 13046: `BC1_SRGB`, `BC2_SRGB`), so the ported stage reads them decoded: a second,
        // sRGB copy of the textures of a building model in the draw of the cities.
        let building = |d: &MaterialDesc| d.package.starts_with("BuildingShader");
        if self.building_code && blob.materials.iter().any(building) {
            for (tid, t) in textures {
                if self.code.srgb.contains_key(tid) {
                    continue;
                }
                let mut s = t.clone();
                s.srgb = true;
                let label = format!("{} (srgb)", tid.as_str());
                if let Ok(up) = upload_blob(device, queue, &s, &label, UploadOptions::default()) {
                    let view = up.texture.create_view(&wgpu::TextureViewDescriptor {
                        dimension: Some(up.view_dimension),
                        ..Default::default()
                    });
                    self.code.srgb.insert(tid.clone(), view);
                }
            }
        }
        let materials = blob
            .materials
            .iter()
            .map(|desc| {
                // The building materials of the cities read the sRGB copies (raw where there is none).
                let decoded = self.building_code && building(desc);
                let file = |id: &asset_format::LogicalId| {
                    decoded
                        .then(|| self.code.srgb.get(id))
                        .flatten()
                        .or_else(|| self.textures.get(id))
                };
                let view_of = |name: &str, fallback: &wgpu::TextureView| -> wgpu::TextureView {
                    let id = match desc.texture(name).map(|t| &t.src) {
                        Some(TexSource::File(id)) => Some(id),
                        _ => None,
                    };
                    id.and_then(|id| self.textures.get(id))
                        .unwrap_or(fallback)
                        .clone()
                };
                // The base is `BaseTextureMap` (`BaseSampler`, `Diffuse` in the other families),
                // the first file of the material when the name is not one of these.
                let first = desc.texture_files().next();
                let base = [
                    "BaseTextureMap",
                    "BaseTextureMap0",
                    "BaseSampler",
                    "Diffuse",
                ]
                .iter()
                .find_map(|n| match desc.texture(n).map(|t| &t.src) {
                    Some(TexSource::File(id)) => file(id),
                    _ => None,
                })
                .or_else(|| first.and_then(file))
                .unwrap_or(&self.white)
                .clone();
                // `SREFMap` of the units, `BuildingSREF` of the building materials of the landmarks.
                let sref = ["SREFMap", "BuildingSREF"]
                    .iter()
                    .find_map(|n| match desc.texture(n).map(|t| &t.src) {
                        Some(TexSource::File(id)) => file(id),
                        _ => None,
                    })
                    .unwrap_or(&self.black)
                    .clone();
                let cube = view_of("IrradianceCubeMap", &self.black_cube);
                // The textures of the layers of an effect; a material without them binds white.
                let fx_views: Vec<wgpu::TextureView> = FX_LAYER_BINDINGS
                    .iter()
                    .map(|&(_, name)| view_of(name, &self.white))
                    .collect();
                let tex = |binding, view| wgpu::BindGroupEntry {
                    binding,
                    resource: wgpu::BindingResource::TextureView(view),
                };
                let smp = |binding| wgpu::BindGroupEntry {
                    binding,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                };
                // The cube the material names (`Irradiance` of `BuildingShader`), for a draw that
                // honours it.
                let irradiance = match desc.texture("Irradiance").map(|t| &t.src) {
                    Some(TexSource::File(id))
                        if self.material_irradiance
                            && building(desc)
                            && self.cubes.contains(id) =>
                    {
                        Some(id.clone())
                    }
                    _ => None,
                };
                if !crate::material::desc::is_fx(desc) {
                    let key = self.code.keys.get(&id).unwrap_or(&self.code.identity);
                    return MaterialGpu {
                        desc: desc.clone(),
                        fx: false,
                        group: self.material_group(device, &base, &sref, &cube, key),
                        views: Some([base, sref, cube]),
                        irradiance,
                    };
                }
                let mut entries = vec![tex(0, &base), smp(1)];
                entries.extend(
                    FX_LAYER_BINDINGS
                        .iter()
                        .zip(&fx_views)
                        .map(|(&(binding, _), view)| tex(binding, view)),
                );
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("object material"),
                    layout: &self.layouts.fx_material,
                    entries: &entries,
                });
                MaterialGpu {
                    desc: desc.clone(),
                    fx: true,
                    group,
                    views: None,
                    irradiance: None,
                }
            })
            .collect::<Vec<_>>();
        let mut meshes = Vec::with_capacity(blob.meshes.len());
        for (i, mesh) in blob.meshes.iter().enumerate() {
            let layout = match vertex_layout_of(mesh) {
                Ok(l) => l,
                Err(why) => {
                    tracing::warn!("model {} mesh {i}: {why}; not drawn", id.as_str());
                    continue;
                }
            };
            if mesh.vertices.is_empty() || mesh.indices.is_empty() {
                continue;
            }
            if usize::from(mesh.material) >= materials.len() {
                tracing::warn!(
                    "model {} mesh {i}: material {} is not there",
                    id.as_str(),
                    mesh.material
                );
                continue;
            }
            let vertices = device.buf_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some(mesh.name.as_str()),
                    contents: &mesh.vertices,
                    usage: wgpu::BufferUsages::VERTEX,
                },
                Cluster::Models,
            );
            // `create_buffer_init` pads to the copy alignment; the count is what the mesh says.
            let indices = device.buf_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some(mesh.name.as_str()),
                    contents: &mesh.indices.data,
                    usage: wgpu::BufferUsages::INDEX,
                },
                Cluster::Models,
            );
            meshes.push(MeshGpu {
                vertices,
                indices,
                index_count: mesh.indices.len() as u32,
                index_format: if mesh.indices.wide {
                    wgpu::IndexFormat::Uint32
                } else {
                    wgpu::IndexFormat::Uint16
                },
                layout,
                material: usize::from(mesh.material),
                bind: mesh.bind,
                part: mesh.part,
                states: mesh.states,
                skinned: has_skin(mesh),
                pipes: Default::default(),
                failed: [false; 4],
            });
        }
        let model = ModelGpu {
            meshes,
            materials,
            sphere: model_sphere(blob),
        };
        match self.index.get(&id) {
            Some(&at) => self.models[at] = model,
            None => {
                self.index.insert(id, self.models.len());
                self.models.push(model);
            }
        }
    }

    /// The light cubes are in: makes the groups 0 of the main view, of the shadow pass and of the
    /// reflection pass (`reflection_globals` is the buffer of the reflection camera's constants). Does
    /// nothing before, or once they are made.
    pub fn bind_light(
        &mut self,
        device: &wgpu::Device,
        env: &LightingEnv,
        frame_globals: &wgpu::Buffer,
        reflection_globals: &wgpu::Buffer,
    ) {
        self.revision += 1;
        if self.frame_group.is_some() || !env.ready() {
            return;
        }
        let (Some(irradiance), Some(dull), Some(sharp), Some(ramp)) = (
            env.unit_irradiance(),
            env.unit_environment_dull(),
            env.unit_environment_sharp(),
            env.fog_ramp(),
        ) else {
            return;
        };
        // The draw of the cities: `BuildingPS_CODE` reads its cubes decoded (sRGB views).
        let decoded = |file: &str, raw| {
            if self.building_code {
                env.model_cube_srgb(file).unwrap_or(raw)
            } else {
                raw
            }
        };
        use crate::lighting::env::{
            UNIT_ENVIRONMENT_DULL, UNIT_ENVIRONMENT_SHARP, UNIT_IRRADIANCE,
        };
        let irradiance = decoded(UNIT_IRRADIANCE, irradiance);
        let dull = decoded(UNIT_ENVIRONMENT_DULL, dull);
        let sharp = decoded(UNIT_ENVIRONMENT_SHARP, sharp);
        let group = |label: &str, globals: &wgpu::Buffer| {
            let tex = |binding, view| wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(view),
            };
            let smp = |binding| wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::Sampler(&self.sampler),
            };
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &self.layouts.frame,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: globals.as_entire_binding(),
                    },
                    tex(2, irradiance),
                    smp(3),
                    tex(4, dull),
                    smp(5),
                    tex(6, sharp),
                    smp(7),
                    tex(8, ramp),
                    smp(9),
                ],
            })
        };
        self.frame_group = Some(group("object frame", frame_globals));
        self.shadow_group = Some(group("object shadow frame", &self.shadow_globals));
        self.reflection_group = Some(group("object reflection frame", reflection_globals));
        self.light = Some(LightViews {
            dull: dull.clone(),
            sharp: sharp.clone(),
            ramp: ramp.clone(),
            frame_globals: frame_globals.clone(),
            reflection_globals: reflection_globals.clone(),
        });
    }

    /// Makes the groups 0 of the material cubes that have none yet (after `bind_light`).
    fn ensure_irradiance_groups(&mut self, device: &wgpu::Device) {
        let Some(light) = &self.light else {
            return;
        };
        let wanted: Vec<asset_format::LogicalId> = self
            .models
            .iter()
            .flat_map(|m| &m.materials)
            .filter_map(|m| m.irradiance.clone())
            .filter(|id| !self.irradiance_groups.contains_key(id))
            .collect();
        for id in wanted {
            if self.irradiance_groups.contains_key(&id) {
                continue;
            }
            // `BuildingPS_CODE` reads its cubes decoded (the sRGB copy), `BuildingPS` raw.
            let cube = self
                .building_code
                .then(|| self.code.srgb.get(&id))
                .flatten()
                .or_else(|| self.textures.get(&id));
            let Some(cube) = cube else {
                continue;
            };
            let group = |label: &str, globals: &wgpu::Buffer| {
                let tex = |binding, view| wgpu::BindGroupEntry {
                    binding,
                    resource: wgpu::BindingResource::TextureView(view),
                };
                let smp = |binding| wgpu::BindGroupEntry {
                    binding,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                };
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(label),
                    layout: &self.layouts.frame,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: globals.as_entire_binding(),
                        },
                        tex(2, cube),
                        smp(3),
                        tex(4, &light.dull),
                        smp(5),
                        tex(6, &light.sharp),
                        smp(7),
                        tex(8, &light.ramp),
                        smp(9),
                    ],
                })
            };
            let groups = (
                group("object frame (material cube)", &light.frame_globals),
                group(
                    "object reflection frame (material cube)",
                    &light.reflection_globals,
                ),
            );
            self.irradiance_groups.insert(id, groups);
        }
    }

    /// The fog mask of the frame (group 2). Without one the rectangle is empty and the pixel
    /// entries read white.
    pub fn bind_fow(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mask: Option<FowSource<'_>>,
    ) {
        self.revision += 1;
        let (view, rect) = match mask {
            Some(m) => {
                let inv = Vec2::ONE / m.size.max(Vec2::splat(1.0));
                (m.view, [m.min.x, m.min.y, inv.x, inv.y])
            }
            None => (&self.white, [0.0; 4]),
        };
        if self.fow_rect_last != Some(rect) {
            queue.write_buffer(&self.fow_rect, 0, bytemuck::cast_slice(&rect));
            self.fow_rect_last = Some(rect);
        }
        // The group is made again only when a view it binds changed: it was one `createBindGroup`
        // a frame for each of the three draws.
        let key = (
            view.clone(),
            self.code.fow_base.clone(),
            self.code.fow_key.clone(),
        );
        if self.fow_group.is_some() && self.fow_group_key.as_ref() == Some(&key) {
            return;
        }
        self.fow_group_key = Some(key);
        let tex = |binding, view| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::TextureView(view),
        };
        let smp = |binding, s| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::Sampler(s),
        };
        let code = &self.code;
        self.fow_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("object fow"),
            layout: &self.layouts.fow,
            entries: &[
                tex(1, view),
                smp(2, &self.sampler),
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.fow_rect.as_entire_binding(),
                },
                tex(6, code.fow_base.as_ref().unwrap_or(&self.white)),
                smp(7, &self.sampler),
                tex(8, code.fow_key.as_ref().unwrap_or(&code.identity)),
                smp(9, &code.sampler),
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: code.consts.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: code.fow_base_scale.as_entire_binding(),
                },
            ],
        }));
    }

    /// True once everything a draw needs is bound.
    pub fn ready(&self) -> bool {
        self.frame_group.is_some() && self.bones_group.is_some() && self.fow_group.is_some()
    }

    /// Stages the instances of the frame and lists the calls of the passes. `bones` is the bone
    /// texture of the world (the group 1 is made again when it was).
    pub fn build(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bones: Option<&BoneTexture>,
        inputs: &BuildInputs<'_>,
    ) -> BuildReport {
        let key = BuildKey {
            revision: self.revision,
            models_revision: inputs.models_revision,
            cull: self.cull.get(),
            shifts: inputs.shifts.to_vec(),
            eye: inputs.eye,
            bones: bones.map(BoneTexture::generation),
        };
        if inputs.remember
            && let Some(r) = &self.remembered
            && r.key == key
            && r.batches == *inputs.batches
            && r.parts == *inputs.parts
            && r.states == *inputs.states
            && r.part_states == *inputs.part_states
        {
            // The same instances seen the same way: the lists and the arena of the last build.
            self.report.uploaded = 0;
            return self.report;
        }
        let complete = std::cell::Cell::new(true);
        self.lists = Lists::default();
        self.arena.begin();
        let mut report = BuildReport::default();
        if let Some(b) = bones
            && self.bones_group.as_ref().map(|(g, _)| *g) != Some(b.generation())
        {
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("object bones"),
                layout: &self.layouts.bones,
                entries: &[wgpu::BindGroupEntry {
                    binding: crate::skin::BONE_TEXTURE_BINDING,
                    resource: wgpu::BindingResource::TextureView(b.view()),
                }],
            });
            self.bones_group = Some((b.generation(), group));
        }
        if self.frame_group.is_none() || self.bones_group.is_none() {
            self.arena.flush(device, queue);
            report.uploaded = self.arena.written;
            self.report = report;
            self.remembered = None;
            return report;
        }
        self.ensure_irradiance_groups(device);
        let cull = self.cull.get();
        let jobs = inputs
            .batches
            .iter()
            .map(|(id, items)| (id, None, asset_format::STATE_INTACT, items))
            .chain(
                inputs
                    .parts
                    .iter()
                    .map(|((id, p), items)| (id, Some(*p), asset_format::STATE_INTACT, items)),
            )
            .chain(
                inputs
                    .states
                    .iter()
                    .map(|((id, s), items)| (id, None, *s, items)),
            )
            .chain(
                inputs
                    .part_states
                    .iter()
                    .map(|((id, p, s), items)| (id, Some(*p), *s, items)),
            );
        for (id, part, desired, items) in jobs {
            let Some(&mi) = self.index.get(id) else {
                // Its model is not here yet: the next build must look again.
                complete.set(false);
                continue;
            };
            if items.is_empty() {
                continue;
            }
            // One copy of the instances for each copy of the map that reaches the screen, and in
            // each copy only the instances that a pass can see (`ranges`: main, shadow, reflection).
            let sphere = self.models[mi].sphere;
            let mut ranges: [Vec<(f32, Range<u32>)>; 3] = Default::default();
            let mut centre = Vec3::ZERO;
            let mut seen = 0u32;
            for &shift in inputs.shifts {
                let Staged {
                    ranges: copy,
                    main_centre,
                    main_seen,
                    counts,
                } = stage_copy(
                    &mut self.arena.staged,
                    &mut self.masks,
                    items,
                    shift,
                    &sphere,
                    &cull,
                );
                for (all, one) in ranges.iter_mut().zip(copy) {
                    all.extend(one.into_iter().map(|r| (shift, r)));
                }
                centre += main_centre;
                seen += main_seen;
                report.staged += counts.staged;
                report.in_main += counts.in_pass[0];
                report.in_shadow += counts.in_pass[1];
                report.in_reflection += counts.in_pass[2];
            }
            report.instances += items.len() as u32 * inputs.shifts.len() as u32;
            let distance = if seen > 0 {
                (centre / seen as f32).distance(inputs.eye)
            } else {
                0.0
            };
            let animated = (inputs.animated)(id);
            let grounds_baked = self.ground_decal_models.contains(id);
            let model = &mut self.models[mi];
            for k in 0..model.meshes.len() {
                let mesh = &model.meshes[k];
                let desc = &model.materials[mesh.material].desc;
                if desc.has(MaterialDesc::INVISIBLE)
                    || stencil_only(desc)
                    || part.is_some_and(|p| mesh.part != p)
                    || (grounds_baked
                        && !mesh.skinned
                        && mesh.bind.is_none()
                        && crate::material::ground_decal(desc))
                {
                    continue;
                }
                // The node state of the instances: a damaged ship shows its damaged hull.
                if !shows_in(mesh.states, desired) {
                    continue;
                }
                // The bones of the instance are the zero pose of a model with no clips.
                if (mesh.skinned || mesh.bind.is_some()) && !animated {
                    continue;
                }
                let transparent = blends(desc);
                let wants = [
                    true,
                    desc.has(MaterialDesc::ZFAIL),
                    casts_shadow(desc) && !transparent,
                    desc.has(MaterialDesc::REFLECTIONS),
                ];
                for (pi, pass) in PASSES.into_iter().enumerate() {
                    if !wants[pi] {
                        continue;
                    }
                    if !ensure_pipeline(
                        &mut self.cache,
                        device,
                        &self.layouts,
                        self.targets,
                        &mut model.meshes[k],
                        &model.materials,
                        pass,
                        self.building_code,
                    ) {
                        if pi == 0 {
                            report.failed += 1;
                        }
                        complete.set(false);
                        continue;
                    }
                    // Main and z-fail draw what the main view sees; a pass with nothing to draw
                    // gets no call (its pipeline is made all the same: a pan must not stall).
                    let pass_ranges = &ranges[match pass {
                        PassKind::Main | PassKind::ZFail => 0,
                        PassKind::Shadow => 1,
                        _ => 2,
                    }];
                    if pass_ranges.is_empty() {
                        continue;
                    }
                    let call = Call {
                        model: mi,
                        mesh: k,
                        ranges: pass_ranges.clone(),
                        distance,
                        transparent,
                    };
                    match pass {
                        PassKind::Main => self.lists.main.push(call),
                        PassKind::ZFail => self.lists.zfail.push(call),
                        PassKind::Reflection => self.lists.reflection.push(call),
                        _ => self.lists.shadow.push(call),
                    }
                    report.calls += 1;
                }
            }
        }
        order_by_pipeline(
            &mut self.lists.main,
            &self.models,
            pass_index(PassKind::Main),
        );
        order(&mut self.lists.zfail);
        order_by_pipeline(
            &mut self.lists.reflection,
            &self.models,
            pass_index(PassKind::Reflection),
        );
        order_by_pipeline(
            &mut self.lists.shadow,
            &self.models,
            pass_index(PassKind::Shadow),
        );
        self.arena.flush(device, queue);
        report.uploaded = self.arena.written;
        self.report = report;
        if inputs.remember && complete.get() {
            // `clone_from` keeps the allocations of the copy of the frame before.
            let r = self.remembered.get_or_insert_with(|| Remembered {
                key: key.clone(),
                batches: BTreeMap::new(),
                parts: PartBatches::new(),
                states: StateBatches::new(),
                part_states: PartStateBatches::new(),
            });
            r.key = key;
            r.batches.clone_from(inputs.batches);
            r.parts.clone_from(inputs.parts);
            r.states.clone_from(inputs.states);
            r.part_states.clone_from(inputs.part_states);
        } else {
            self.remembered = None;
        }
        report
    }

    /// `frame` is the group 0 the pass bound; a material with a cube of its own binds its group 0
    /// for its call and the next call binds `frame` back.
    fn draw_calls(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        calls: &[Call],
        pass_kind: PassKind,
        shift: f32,
        stats: &mut FrameStats,
        groups: Option<(&wgpu::BindGroup, &wgpu::BindGroup)>,
    ) {
        let (frame, fow) = groups.unzip();
        let pi = pass_index(pass_kind);
        // What the caller bound is known: the frame (0) and the fog (2) of a colour pass, the
        // light's constants (0) of the shadow pass; the sets that change nothing are skipped.
        let mut state = PassState::new();
        let (frame0, bones) = match (&self.bones_group, pass_kind) {
            (Some((_, b)), PassKind::Shadow) => (self.shadow_group.as_ref(), Some(b)),
            (Some((_, b)), _) => (frame, Some(b)),
            _ => (None, None),
        };
        if let Some(g) = frame0 {
            state.assume_group(0, g);
        }
        if let Some(g) = bones {
            state.assume_group(1, g);
        }
        if let Some(g) = fow {
            state.assume_group(2, g);
        }
        let stride = ObjectInstance::STRIDE;
        for call in calls {
            let mut ranges = call
                .ranges
                .iter()
                .filter(|(s, _)| (s - shift).abs() < 1e-3)
                .map(|(_, r)| r)
                .peekable();
            if ranges.peek().is_none() {
                continue;
            }
            let model = &self.models[call.model];
            let mesh = &model.meshes[call.mesh];
            let Some(pipeline) = &mesh.pipes[pi] else {
                continue;
            };
            state.pipeline(pass, pipeline);
            if pass_kind != PassKind::Shadow {
                let material = &model.materials[mesh.material];
                // The effects' pipelines have an empty group 2; the pass bound the fog group.
                if let Some(fow) = fow {
                    state.group(pass, 2, if material.fx { &self.fx_fow_group } else { fow });
                }
                if let Some(frame) = frame {
                    let own = self.model_light.get(&call.model).or_else(|| {
                        material
                            .irradiance
                            .as_ref()
                            .and_then(|id| self.irradiance_groups.get(id))
                    });
                    let g = match own {
                        Some((_, reflection)) if pass_kind == PassKind::Reflection => reflection,
                        Some((main, _)) => main,
                        None => frame,
                    };
                    state.group(pass, 0, g);
                }
                state.group(pass, 3, &material.group);
            }
            state.vertex(pass, 0, &mesh.vertices, 0..mesh.vertices.size());
            state.index(
                pass,
                &mesh.indices,
                0..mesh.indices.size(),
                mesh.index_format,
            );
            for range in ranges {
                state.vertex(
                    pass,
                    1,
                    &self.arena.buffer,
                    u64::from(range.start) * stride..u64::from(range.end) * stride,
                );
                pass.draw_indexed_c(0..mesh.index_count, 0, 0..range.end - range.start);
                stats.record_instanced(mesh.index_count / 3 * (range.end - range.start));
            }
        }
    }

    /// The calls of a slot of the main pass, for the map copy shifted by `shift`.
    pub fn record(&self, slot: PassSlot, cx: &mut DrawContext<'_, '_>) {
        let (calls, kind) = match slot {
            PassSlot::Main(MainSlot::UnitZFail) if self.slot == MainSlot::Unit => {
                (&self.lists.zfail, PassKind::ZFail)
            }
            PassSlot::Main(s) if s == self.slot => (&self.lists.main, PassKind::Main),
            _ => return,
        };
        let (Some(frame), Some((_, bones)), Some(fow)) =
            (&self.frame_group, &self.bones_group, &self.fow_group)
        else {
            return;
        };
        if calls.is_empty() {
            return;
        }
        cx.pass.set_bind_group_c(0, frame, &[]);
        cx.pass.set_bind_group_c(1, bones, &[]);
        cx.pass.set_bind_group_c(2, fow, &[]);
        let shift = cx.copy_shift;
        self.draw_calls(cx.pass, calls, kind, shift, cx.stats, Some((frame, fow)));
    }

    /// The main calls of the slot into the class mask: the units, the
    /// cities or the landmarks, by the slot of this draw. The vertex stage and the depth state are
    /// the main pass's; the effects (`FXShader`) are particles to the mask and are left out, as are
    /// the blended meshes.
    pub fn record_mask(&self, slot: PassSlot, pass: &mut wgpu::RenderPass<'_>, shift: f32) {
        use crate::frame::mask::class;
        if slot != PassSlot::Main(self.slot) || self.lists.main.is_empty() {
            return;
        }
        let class = match self.slot {
            MainSlot::City => class::CITY,
            MainSlot::Landmark => class::LANDMARK,
            _ => class::UNIT,
        };
        let (Some(frame), Some((_, bones)), Some(fow)) =
            (&self.frame_group, &self.bones_group, &self.fow_group)
        else {
            return;
        };
        pass.set_bind_group_c(0, frame, &[]);
        pass.set_bind_group_c(1, bones, &[]);
        pass.set_bind_group_c(2, fow, &[]);
        let mut cache = self.mask_cache.borrow_mut();
        let instance = instance_layout();
        for call in &self.lists.main {
            if !call.ranges.iter().any(|(s, _)| (s - shift).abs() < 1e-3) {
                continue;
            }
            let model = &self.models[call.model];
            let mesh = &model.meshes[call.mesh];
            let desc = &model.materials[mesh.material].desc;
            // The blended meshes (a city's ground, glass, the fade of a decal) have no alpha test
            // in the mask and would cover what shows through them.
            if call.transparent || crate::material::desc::is_fx(desc) {
                continue;
            }
            let Ok((family, selection)) =
                resolve_object(desc, PassKind::Main, mesh.bind.filter(|_| !mesh.skinned))
            else {
                continue;
            };
            let layouts = [
                &self.layouts.frame,
                &self.layouts.bones,
                &self.layouts.fow,
                &self.layouts.material,
            ];
            let request = PipelineRequest {
                family,
                selection: &selection,
                pass: PassKind::Main,
                state: RenderState::of_material(desc, PassKind::Main),
                target: self.targets.colour,
                depth: Some(self.targets.depth),
                samples: 1,
                vertex: &mesh.layout,
                instance: Some(&instance),
                layouts: &layouts,
            };
            let Ok(pipeline) = cache.get_mask(&self.device, &request, class) else {
                continue;
            };
            pass.set_pipeline_c(&pipeline);
            pass.set_bind_group_c(3, &model.materials[mesh.material].group, &[]);
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), mesh.index_format);
            for (_, range) in call.ranges.iter().filter(|(s, _)| (s - shift).abs() < 1e-3) {
                pass.set_vertex_buffer(1, self.arena.slice(range));
                pass.draw_indexed_c(0..mesh.index_count, 0, 0..range.end - range.start);
            }
        }
    }

    /// The casters of the units into the pass of the shadow map (the copy in the middle of the
    /// map: the map copies of a wrapping map are not shadowed twice).
    pub fn record_shadow(&self, pass: &mut wgpu::RenderPass<'_>, stats: &mut FrameStats) {
        let (Some(group), Some((_, bones))) = (&self.shadow_group, &self.bones_group) else {
            return;
        };
        if self.lists.shadow.is_empty() {
            return;
        }
        pass.set_bind_group_c(0, group, &[]);
        pass.set_bind_group_c(1, bones, &[]);
        self.draw_calls(pass, &self.lists.shadow, PassKind::Shadow, 0.0, stats, None);
    }

    /// The meshes of the reflection pass, for the map copy shifted by `shift`: the frame constants
    /// are the reflection camera's (the renderer writes them every frame), the pipelines cut at the
    /// water plane. The bone texture and the fog mask are the main view's.
    pub fn record_reflection(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        shift: f32,
        stats: &mut FrameStats,
    ) {
        let (Some(frame), Some((_, bones)), Some(fow)) =
            (&self.reflection_group, &self.bones_group, &self.fow_group)
        else {
            return;
        };
        if self.lists.reflection.is_empty() {
            return;
        }
        pass.set_bind_group_c(0, frame, &[]);
        pass.set_bind_group_c(1, bones, &[]);
        pass.set_bind_group_c(2, fow, &[]);
        self.draw_calls(
            pass,
            &self.lists.reflection,
            PassKind::Reflection,
            shift,
            stats,
            Some((frame, fow)),
        );
    }

    /// Calls of the last build by pass: main, z-fail, shadow.
    pub fn call_counts(&self) -> (usize, usize, usize) {
        (
            self.lists.main.len(),
            self.lists.zfail.len(),
            self.lists.shadow.len(),
        )
    }

    /// Calls of the reflection pass in the last build.
    pub fn reflection_calls(&self) -> usize {
        self.lists.reflection.len()
    }
}

/// The order the groups of instances of one copy are staged in, by the passes they are in (bits of
/// [`cull`]): the groups the main pass draws come first and are one run, the shadow groups are two
/// runs at most, so a pass draws a few ranges and not one per instance.
const STAGE_ORDER: [u8; 7] = [7, 3, 5, 1, 6, 2, 4];

/// What `stage_copy` counted.
#[derive(Default)]
struct StageCounts {
    staged: u32,
    /// Instances in main, shadow, reflection.
    in_pass: [u32; 3],
}

struct Staged {
    /// The ranges of the arena by pass: main, shadow, reflection.
    ranges: [Vec<Range<u32>>; 3],
    /// The sum of the places of the instances the main view sees, and how many.
    main_centre: Vec3,
    main_seen: u32,
    counts: StageCounts,
}

/// Puts the instances of one model in the arena for the copy of the map moved by `shift`: only
/// those that a pass sees, grouped by the passes that see them.
fn stage_copy(
    staged: &mut Vec<ObjectInstance>,
    masks: &mut Vec<u8>,
    items: &[ObjectInstance],
    shift: f32,
    sphere: &cull::ModelSphere,
    cull: &CullSet,
) -> Staged {
    let mut counts = [0usize; 8];
    let mut main_centre = Vec3::ZERO;
    let mut main_seen = 0u32;
    sphere.masks_into(items, shift, cull, masks);
    for (item, &m) in items.iter().zip(masks.iter()) {
        counts[usize::from(m)] += 1;
        if m & cull::MAIN != 0 {
            main_centre += Vec3::from(item.transform_point([0.0; 3]));
            main_seen += 1;
        }
    }
    let total: usize = counts[1..].iter().sum();
    let mut out = Staged {
        ranges: Default::default(),
        main_centre,
        main_seen,
        counts: StageCounts {
            staged: total as u32,
            in_pass: [cull::MAIN, cull::SHADOW, cull::REFLECTION].map(|bit| {
                (1..8)
                    .filter(|m| m & bit != 0)
                    .map(|m| counts[m as usize] as u32)
                    .sum()
            }),
        },
    };
    if total == 0 {
        return out;
    }
    let start = staged.len() as u32;
    staged.resize(staged.len() + total, ObjectInstance::default());
    let slots = &mut staged[start as usize..];
    // Where the group of each mask starts in `slots`.
    let mut at = [0usize; 8];
    let mut next = 0;
    for m in STAGE_ORDER {
        at[usize::from(m)] = next;
        next += counts[usize::from(m)];
    }
    let mut fill = at;
    for (item, &m) in items.iter().zip(masks.iter()) {
        if m == 0 {
            continue;
        }
        let slot = &mut fill[usize::from(m)];
        // A low eye draws the instance smaller about its origin (the sphere of the masks was the scaled one).
        let mut o = match &cull.near {
            Some(near) => near.scaled(item),
            None => *item,
        };
        o.world[0][3] += shift;
        slots[*slot] = o;
        *slot += 1;
    }
    for (bit, ranges) in [cull::MAIN, cull::SHADOW, cull::REFLECTION]
        .into_iter()
        .zip(&mut out.ranges)
    {
        let mut run: Option<Range<u32>> = None;
        for m in STAGE_ORDER {
            let n = counts[usize::from(m)];
            if n == 0 {
                continue;
            }
            let first = start + at[usize::from(m)] as u32;
            let group = first..first + n as u32;
            if m & bit == 0 {
                ranges.extend(run.take());
            } else if let Some(r) = run.as_mut().filter(|r| r.end == group.start) {
                r.end = group.end;
            } else {
                ranges.extend(run.replace(group));
            }
        }
        ranges.extend(run);
    }
    out
}

/// Opaque calls first, in the order of the models; the transparent ones from the farthest.
fn order(calls: &mut [Call]) {
    calls.sort_by(|a, b| {
        a.transparent.cmp(&b.transparent).then_with(|| {
            if a.transparent {
                b.distance.total_cmp(&a.distance)
            } else {
                (a.model, a.mesh).cmp(&(b.model, b.mesh))
            }
        })
    });
}

/// As [`order`], and the opaque calls of one pipeline together (then the model, the mesh), so that
/// the pass sets a pipeline once for all of them. Opaque calls are tested against the depth and
/// write no blend, so their order is not in the picture; the transparent ones keep theirs.
fn order_by_pipeline(calls: &mut [Call], models: &[ModelGpu], pass: usize) {
    let pipeline = |c: &Call| models[c.model].meshes[c.mesh].pipes[pass].as_ref();
    calls.sort_by(|a, b| {
        a.transparent.cmp(&b.transparent).then_with(|| {
            if a.transparent {
                b.distance.total_cmp(&a.distance)
            } else {
                pipeline(a)
                    .cmp(&pipeline(b))
                    .then_with(|| (a.model, a.mesh).cmp(&(b.model, b.mesh)))
            }
        })
    });
}

/// Makes the pipeline of `mesh` for `pass` if there is none. False when the family does not draw it
/// in the pass or the cache refused. `building_code`: the `BuildingShader` meshes take the ported
/// colour pass (`UnitDraw::set_building_code`).
#[allow(clippy::too_many_arguments)]
fn ensure_pipeline(
    cache: &mut PipelineCache,
    device: &wgpu::Device,
    layouts: &Layouts,
    targets: Targets,
    mesh: &mut MeshGpu,
    materials: &[MaterialGpu],
    pass: PassKind,
    building_code: bool,
) -> bool {
    let pi = pass_index(pass);
    if mesh.pipes[pi].is_some() {
        return true;
    }
    if mesh.failed[pi] {
        return false;
    }
    let desc = &materials[mesh.material].desc;
    let resolved = resolve_object(desc, pass, mesh.bind.filter(|_| !mesh.skinned));
    let Ok((family, mut selection)) = resolved else {
        mesh.failed[pi] = true;
        return false;
    };
    // The buildings of the cities: the ported colour pass, the same `override`s.
    if building_code
        && std::ptr::eq(family, &crate::material::families::BUILDING_OBJECT)
        && let Some(s) = crate::material::families::building_code_select(desc, pass)
    {
        selection.vs = s.vs;
        selection.fs = s.fs;
    }
    // The group 0 of this draw is the unit's (light cubes and the fog ramp); the building family has its
    // own (binding 10, no ramp) and is drawn by `static_model`. A pipeline made here for it would be
    // invalid and take the whole frame with it.
    if family.name == crate::entities::building_family::PACKAGE {
        mesh.failed[pi] = true;
        return false;
    }
    let instance = instance_layout();
    let shadow = pass == PassKind::Shadow;
    let fx = crate::material::desc::is_fx(desc);
    let all = [
        &layouts.frame,
        &layouts.bones,
        if fx { &layouts.fx_fow } else { &layouts.fow },
        if fx {
            &layouts.fx_material
        } else {
            &layouts.material
        },
    ];
    let mut state = RenderState::of_material(desc, pass);
    if [UNIT_CODE_PS, UNIT_CODE_REFLECT_PS].contains(&selection.fs) {
        state.blend = BlendMode::UnitCode;
    }
    let request = PipelineRequest {
        family,
        selection: &selection,
        pass,
        state,
        target: if shadow { raw_format() } else { targets.colour },
        depth: (!shadow).then_some(targets.depth),
        // The reflection target is a plain texture, never multisampled.
        samples: if shadow || pass == PassKind::Reflection {
            1
        } else {
            targets.samples
        },
        vertex: &mesh.layout,
        instance: Some(&instance),
        // The shadow entries read the frame constants and the bones only.
        layouts: if shadow { &all[..2] } else { &all },
    };
    match cache.get(device, &request) {
        Ok(p) => {
            mesh.pipes[pi] = Some(p);
            true
        }
        Err(_) => {
            mesh.failed[pi] = true;
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::data::fixtures;
    use crate::entities::{EntityPlayer, EntitySnapshot, UnitEntity};
    use crate::game_camera::GameCamera;
    use crate::gpu_test::{gpu, read_texture};
    use crate::hexgrid::cell_center;
    use crate::lighting::tests::{boot_blobs_with, settings};
    use crate::renderer::Renderer;
    use crate::scene::Scene;
    use crate::terrain::{Fog, TerrainCell, TerrainMesh, TerrainSnapshot};
    use crate::world::terrain::TerrainBasis;
    use asset_format::{
        ArtTable, FigureB2, LogicalId, MeshArrays, ModelBlobV2, ParamSet, Part, TexDim, TexKind,
        TexParam, TextureFormat, Transform, UnitArtB2, UnitRecordB2,
    };
    use draw_list::UiDrawList;

    /// The GL backend of WebGL2 numbers the textures of a whole pipeline layout and has 16 slots for
    /// them: the 17th made `wgpu-hal` panic (gles/device.rs, index 16 of 16) as soon as the full set
    /// drew an effect. Holds for the layouts of both kinds of pipeline, with the fog group each uses.
    #[test]
    fn no_pipeline_layout_of_the_objects_has_more_textures_than_webgl2() {
        let limit = wgpu::Limits::downlevel_webgl2_defaults().max_sampled_textures_per_shader_stage
            as usize;
        assert_eq!(limit, 16);
        for fx in [false, true] {
            let n = pipeline_textures(fx);
            assert!(n <= limit, "{n} textures in a pipeline (effect: {fx})");
        }
        // The effect's material is the big one: with the fog group of the others it would not fit.
        let fog = fow_entries()
            .iter()
            .filter(|e| matches!(e.ty, wgpu::BindingType::Texture { .. }))
            .count();
        assert!(pipeline_textures(true) + fog > limit);
    }

    fn quad_mesh(material: u16) -> MeshV2 {
        quad(material, true)
    }

    fn quad(material: u16, skinned: bool) -> MeshV2 {
        let a = MeshArrays {
            positions: vec![
                [-20.0, 0.0, 0.0],
                [20.0, 0.0, 0.0],
                [20.0, 40.0, 0.0],
                [-20.0, 40.0, 0.0],
            ],
            normals: vec![[0.0, 0.0, 1.0]; 4],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            joints: if skinned { vec![[0; 4]; 4] } else { vec![] },
            weights: if skinned {
                vec![[255, 0, 0, 0]; 4]
            } else {
                vec![]
            },
            indices: vec![0, 1, 2, 0, 2, 3],
        };
        MeshV2::pack("quad", 0, material, &a, false, false).unwrap()
    }

    fn material(texture: &LogicalId, flags: u16) -> MaterialDesc {
        MaterialDesc {
            package: "UnitShader_Skinned".into(),
            version: 4,
            flags,
            skin_bones: 2,
            sets: vec![ParamSet {
                blocks: vec![],
                textures: vec![TexParam {
                    name: "BaseTextureMap".into(),
                    kind: TexKind::Tex2D,
                    src: TexSource::File(texture.clone()),
                    semantic: 0,
                    reg: 0,
                    usage: 0,
                    flags: 0,
                    active: true,
                }],
                constants: vec![],
            }],
            ..MaterialDesc::default()
        }
    }

    /// A model of two bones with the idle clips and one skinned quad of 40 x 40 standing on the
    /// ground; bone 0 stays where it is, so the quad is where its vertices say.
    fn model(texture: &LogicalId, flags: u16) -> (ModelBlobV2, asset_format::AnimPack) {
        let (mut m, pack) = fixtures::model("warrior", 2);
        m.parts = vec![Part {
            name: "warrior".into(),
            first_mesh: 0,
            mesh_count: 1,
            placement: Transform::IDENTITY,
        }];
        m.meshes = vec![quad_mesh(0)];
        m.materials = vec![material(texture, flags)];
        (m, pack)
    }

    fn base_texture(rgba: [u8; 4]) -> TextureBlob {
        TextureBlob {
            format: TextureFormat::Rgba8,
            dim: TexDim::D2,
            srgb: true,
            width: 1,
            height: 1,
            depth: 1,
            mips: vec![rgba.to_vec()],
        }
    }

    fn art_table() -> ArtTable {
        let mut t = ArtTable::new();
        t.insert_unit_b2(
            "UNIT_WARRIOR",
            UnitRecordB2 {
                base: UnitArtB2 {
                    art_info: "ART_WARRIOR".into(),
                    figures: vec![FigureB2 {
                        member: "M".into(),
                        model: LogicalId::new("md:warrior"),
                        count: 1,
                        scale: Some(1.0),
                        z_offset: None,
                        domain: None,
                    }],
                    damage_states: None,
                    formation: None,
                },
                style_variants: vec![],
                era_variation: false,
                era_variants: vec![],
            },
        );
        t
    }

    struct World {
        gpu: crate::gpu_test::Gpu,
    }

    /// A renderer with the light of the boot group (every cube grey 200), a map of 6 x 4 that the
    /// player sees as `fog` says, and one warrior at cell (3, 2) of the player `team`.
    fn world(
        gpu: crate::gpu_test::Gpu,
        size: (u32, u32),
        team: [u8; 3],
        fog: crate::terrain::Fog,
    ) -> (World, Renderer) {
        let mut r = Renderer::offscreen(&gpu, size, wgpu::TextureFormat::Rgba8Unorm, 1);
        r.set_boot_required(true);
        for (file, b) in boot_blobs_with(&[], &|_, _| [200, 200, 200, 255]) {
            r.add_lighting_texture(&file, &b).unwrap();
        }
        r.set_world_settings(settings());
        let cell = TerrainCell {
            fog,
            ..TerrainCell::default()
        };
        let snapshot = TerrainSnapshot::new(6, 4, false, vec![cell; 24]).unwrap();
        r.set_terrain(&snapshot, &TerrainMesh::build(&snapshot));
        r.set_terrain_basis(TerrainBasis::from_snapshot(&snapshot, 1).unwrap());
        r.set_unit_art_b2(art_table());
        r.set_entities(EntitySnapshot {
            players: vec![EntityPlayer {
                id: 0,
                color: team,
                team_color: team,
                culture: None,
                era: None,
                industrial: false,
                art_techs: 0,
                era_index: None,
            }],
            units: vec![UnitEntity {
                id: 1,
                owner: 0,
                type_name: "UNIT_WARRIOR".into(),
                x: 3,
                y: 2,
                hp: 100,
                max_hp: 100,
                civilian: false,
                needs_orders: false,
            }],
            cities: vec![],
            era: Default::default(),
            plots: vec![],
        });
        (World { gpu }, r)
    }

    /// Draws `frames` frames of the unit cell from above and gives the last one.
    fn frames(w: &World, r: &mut Renderer, size: (u32, u32), frames: usize) -> Vec<u8> {
        let camera = {
            let mut c = GameCamera::new(cell_center(3, 2));
            c.set_level(10.0);
            c
        };
        frames_as(w, r, size, frames, &camera)
    }

    /// Draws `frames` frames through `camera` and gives the last one.
    fn frames_as(
        w: &World,
        r: &mut Renderer,
        size: (u32, u32),
        frames: usize,
        camera: &dyn crate::camera::CameraView,
    ) -> Vec<u8> {
        let target = w.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("unit target"),
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
        let mut last = Vec::new();
        for i in 0..frames {
            let commands = r.encode_frame(
                &view,
                &Scene::test(i as f32 / 30.0),
                camera,
                &UiDrawList::default(),
                None,
            );
            w.gpu.queue.submit([commands]);
            r.frame_submitted();
            last = read_texture(&w.gpu, &target, 0, 0, size, 4);
        }
        last
    }

    /// The pixel that is most the colour of the team.
    fn most(px: &[u8], team: [u8; 3]) -> [u8; 4] {
        px.chunks(4)
            .map(|p| [p[0], p[1], p[2], p[3]])
            .max_by_key(|p| {
                let want = |c: u8, t: u8| if t > 128 { i32::from(c) } else { -i32::from(c) };
                (0..3).map(|k| want(p[k], team[k])).sum::<i32>()
            })
            .unwrap()
    }

    /// WOB2/WAN3: a skinned figure of `assets-b2` is on the screen in the colour of its owner,
    /// lit by the cubes, through the bone texture of the world, in the main, z-fail and shadow
    /// calls (no validation error in any pipeline).
    #[test]
    fn a_skinned_figure_of_assets_b2_is_drawn_in_the_colour_of_its_team() {
        let Some(gpu) = gpu(
            "a_skinned_figure_of_assets_b2_is_drawn_in_the_colour_of_its_team",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let size = (96u32, 96u32);
        let (w, mut r) = world(gpu, size, [255, 0, 0], Fog::Visible);
        let mut settings = r.settings().clone();
        settings.set("ShadowLevel", 2).unwrap();
        r.set_settings(settings);
        let tex = LogicalId::new("tx:unit_base");
        // Nothing is drawn before the model is there, and the renderer asks for it.
        frames(&w, &mut r, size, 1);
        assert!(r.world().unit.b2_active(), "the light and the art are in");
        assert_eq!(r.unit_models_b2_wanted(), [LogicalId::new("md:warrior")]);
        // A base texture with alpha 0: all team colour.
        let (m, pack) = model(
            &tex,
            MaterialDesc::SKINNED | MaterialDesc::ZFAIL | MaterialDesc::SHADOWS,
        );
        let textures = [(tex, base_texture([255, 255, 255, 0]))].into();
        r.add_unit_model_b2(
            LogicalId::new("md:warrior"),
            &m,
            Some(pack),
            None,
            &textures,
        );
        assert!(r.unit_models_b2_wanted().is_empty());
        let px = frames(&w, &mut r, size, 4);
        let draw = r.world().unit.b2_draw().unwrap();
        // One mesh: a call in the main pass, in the z-fail pass and in the shadow map.
        assert_eq!(draw.call_counts(), (1, 1, 1), "{:?}", draw.report());
        // The caster went into the shadow map: counted under `ShadowMap` (the blur has its own row).
        let stats = r.last_frame_stats();
        let shadow = stats
            .passes
            .iter()
            .find(|p| p.pass == crate::frame::PassId::ShadowMap)
            .unwrap();
        assert!(
            shadow.draws >= 1 && shadow.triangles >= 1,
            "{}",
            stats.pass_table()
        );
        // Gamma 2.0 of the whole chain: sqrt(1 * team^2 * (200/255)^2) = team * 200/255.
        let red = most(&px, [255, 0, 0]);
        assert!((i32::from(red[0]) - 200).abs() <= 4, "{red:?}");
        assert!(red[1] <= 4 && red[2] <= 4, "{red:?}");
    }

    /// RC-12: a unit above the water is drawn into `ReflectionColor` (the reflection camera, the
    /// main entries cut at the water plane) when the material reflects; a material that does not
    /// leaves the target black; and at `ReflectionLevel` 0 there is no target and no pass.
    #[test]
    fn a_unit_above_the_water_changes_the_reflection_target() {
        let size = (96u32, 96u32);
        let tex = LogicalId::new("tx:unit_base");
        let reflected = |flags: u16, level: i64| {
            let gpu = gpu(
                "a_unit_above_the_water_changes_the_reflection_target",
                wgpu::Backends::PRIMARY,
                wgpu::Features::empty(),
            )?;
            let (w, mut r) = world(gpu, size, [255, 0, 0], Fog::Visible);
            let mut settings = r.settings().clone();
            settings.set("ReflectionLevel", level).unwrap();
            r.set_settings(settings);
            frames(&w, &mut r, size, 1);
            let (m, pack) = model(&tex, flags);
            let textures = [(tex.clone(), base_texture([255, 255, 255, 0]))].into();
            r.add_unit_model_b2(
                LogicalId::new("md:warrior"),
                &m,
                Some(pack),
                None,
                &textures,
            );
            frames(&w, &mut r, size, 4);
            let drawn = r
                .last_frame_stats()
                .passes
                .iter()
                .find(|p| p.pass == crate::frame::PassId::Reflection)
                .map_or(0, |p| p.draws);
            let lit = r.target(crate::frame::REFLECTION_COLOR).map(|t| {
                read_texture(&w.gpu, &t.texture, 0, 0, t.size, 4)
                    .chunks(4)
                    .filter(|p| p[0] > 8 || p[1] > 8 || p[2] > 8)
                    .count()
            });
            Some((
                drawn,
                lit,
                r.world().unit.b2_draw().unwrap().reflection_calls(),
            ))
        };
        let Some((drawn, lit, calls)) =
            reflected(MaterialDesc::SKINNED | MaterialDesc::REFLECTIONS, 1)
        else {
            return;
        };
        assert_eq!(calls, 1, "the mesh has a call in the reflection pass");
        assert!(drawn >= 1, "the pass drew the mesh: {drawn}");
        assert!(lit.unwrap() > 20, "the unit is in the target: {lit:?}");
        let (_, black, calls) = reflected(MaterialDesc::SKINNED, 1).unwrap();
        assert_eq!(
            calls, 0,
            "a material without UseReflections is not reflected"
        );
        assert_eq!(black, Some(0), "the target stays black");
        let (drawn, none, _) =
            reflected(MaterialDesc::SKINNED | MaterialDesc::REFLECTIONS, 0).unwrap();
        assert_eq!((drawn, none), (0, None), "level 0: no pass and no target");
    }

    /// AC-R8.1 on the real passes: under the Low preset a unit that casts a shadow and reflects is in
    /// neither the shadow map nor the reflection target, and there is no reflection target; High
    /// draws both, and the switch back to High brings both back without a new renderer.
    #[test]
    fn the_low_preset_has_no_object_shadows_and_no_reflection() {
        use crate::settings::{Preset, RenderSettings};
        let size = (96u32, 96u32);
        let Some(gpu) = gpu(
            "the_low_preset_has_no_object_shadows_and_no_reflection",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let (w, mut r) = world(gpu, size, [255, 0, 0], Fog::Visible);
        let tex = LogicalId::new("tx:unit_base");
        r.set_settings(RenderSettings::preset(Preset::High));
        frames(&w, &mut r, size, 1);
        let (m, pack) = model(
            &tex,
            MaterialDesc::SKINNED
                | MaterialDesc::ZFAIL
                | MaterialDesc::SHADOWS
                | MaterialDesc::REFLECTIONS,
        );
        let textures = [(tex, base_texture([255, 255, 255, 0]))].into();
        r.add_unit_model_b2(
            LogicalId::new("md:warrior"),
            &m,
            Some(pack),
            None,
            &textures,
        );
        let draws = |r: &Renderer| {
            let stats = r.last_frame_stats();
            let of = |id| {
                stats
                    .passes
                    .iter()
                    .find(|p| p.pass == id)
                    .map_or(0, |p| p.draws)
            };
            (
                of(crate::frame::PassId::Reflection),
                of(crate::frame::PassId::ShadowMap),
            )
        };
        let mut seen = Vec::new();
        for preset in [Preset::High, Preset::Low, Preset::Medium, Preset::High] {
            r.set_settings(RenderSettings::preset(preset));
            frames(&w, &mut r, size, 4);
            let (reflection, shadow) = draws(&r);
            let has_target = r.target(crate::frame::REFLECTION_COLOR).is_some();
            seen.push((preset, reflection, shadow, has_target));
            let s = RenderSettings::preset(preset);
            match preset {
                Preset::Low => {
                    assert_eq!((reflection, shadow, has_target), (0, 0, false), "Low");
                    assert_eq!(s.effective_msaa(), 1, "Low has no MSAA");
                    assert!(s.fow_level <= 1, "Low has no cloud fog");
                }
                Preset::High => {
                    assert!(reflection >= 1 && has_target, "High: {seen:?}");
                    assert!(shadow >= 1, "High: {seen:?}");
                }
                // Medium keeps the shadows, loses the reflection (the menu of the original).
                Preset::Medium => {
                    assert_eq!((reflection, has_target), (0, false), "Medium: {seen:?}");
                    assert!(shadow >= 1, "Medium: {seen:?}");
                }
            }
        }
    }

    /// The same figure of another player is in the colour of that player: the colour is the
    /// instance data.
    #[test]
    fn the_colour_of_a_figure_is_the_colour_of_its_owner() {
        let Some(gpu) = gpu(
            "the_colour_of_a_figure_is_the_colour_of_its_owner",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let size = (96u32, 96u32);
        let (w, mut r) = world(gpu, size, [0, 0, 255], Fog::Visible);
        frames(&w, &mut r, size, 1);
        let tex = LogicalId::new("tx:unit_base");
        let (m, pack) = model(&tex, MaterialDesc::SKINNED);
        let textures = [(tex, base_texture([255, 255, 255, 0]))].into();
        r.add_unit_model_b2(
            LogicalId::new("md:warrior"),
            &m,
            Some(pack),
            None,
            &textures,
        );
        let px = frames(&w, &mut r, size, 4);
        let blue = most(&px, [0, 0, 255]);
        assert!((i32::from(blue[2]) - 200).abs() <= 4, "{blue:?}");
        assert!(blue[0] <= 4 && blue[1] <= 4, "{blue:?}");
        let draw = r.world().unit.b2_draw().unwrap();
        assert_eq!(
            draw.call_counts(),
            (1, 0, 1),
            "no z-fail flag; the unit family always casts a shadow"
        );
    }

    /// A first-person camera of a mod (client-hooks C2), the eye 6.5 above the ground, in front of a figure 40
    /// tall: at the map's scale (`objects: None`) the figure fills the view when the eye walks up to it. With
    /// [`ObjectsNearEye::FIRST_PERSON`](crate::camera::ObjectsNearEye) the figure seen from 80 away is about a
    /// quarter of its pixels (half its size), and up close it is not drawn at all.
    #[test]
    fn a_low_eye_sees_a_figure_at_half_its_size_and_not_when_it_reaches_the_eye() {
        use crate::camera::{FirstPerson, ObjectsNearEye};
        let Some(gpu) = gpu(
            "a_low_eye_sees_a_figure_at_half_its_size_and_not_when_it_reaches_the_eye",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let size = (128u32, 96u32);
        let (w, mut r) = world(gpu, size, [0, 0, 255], Fog::Visible);
        frames(&w, &mut r, size, 1);
        let tex = LogicalId::new("tx:unit_base");
        let (m, pack) = model(&tex, MaterialDesc::SKINNED);
        let textures = [(tex, base_texture([255, 255, 255, 0]))].into();
        r.add_unit_model_b2(
            LogicalId::new("md:warrior"),
            &m,
            Some(pack),
            None,
            &textures,
        );
        frames(&w, &mut r, size, 4);
        // The figure stands at the middle of its cell, facing south (+Z): the eye looks north at it.
        let at = cell_center(3, 2);
        let ground = r
            .plot_center_heights()
            .map_or(0.0, |(width, _, h)| h[(2 * width + 3) as usize]);
        let blue = |px: &[u8]| {
            px.chunks(4)
                .filter(|p| p[2] > 100 && p[0] < 40 && p[1] < 40)
                .count()
        };
        let mut look = |distance: f32, objects: Option<ObjectsNearEye>| {
            let mut cam = FirstPerson::new(glam::Vec3::new(at.x, ground + 6.5, at.y + distance));
            cam.objects = objects;
            blue(&frames_as(&w, &mut r, size, 3, &cam))
        };
        let all = (size.0 * size.1) as usize;
        let far_map = look(80.0, None);
        let far_fp = look(80.0, Some(ObjectsNearEye::FIRST_PERSON));
        let near_map = look(8.0, None);
        let near_fp = look(8.0, Some(ObjectsNearEye::FIRST_PERSON));
        eprintln!(
            "figure px: 80 away {far_map} / {far_fp}, 8 away {near_map} / {near_fp} of {all}"
        );
        assert!(
            far_map > 100,
            "the figure is seen from 80 away ({far_map} px)"
        );
        // Half the size: a quarter of the pixels, give or take the edges.
        assert!(
            far_fp > 0 && far_fp * 3 < far_map && far_fp * 6 > far_map,
            "half the size: {far_fp} of {far_map} px"
        );
        assert!(
            near_map * 2 > all,
            "at the map's scale it fills the view ({near_map} of {all})"
        );
        assert_eq!(near_fp, 0, "nothing at the eye");
        // The strategic camera after it draws the figure as before.
        let again = frames(&w, &mut r, size, 2);
        assert!(blue(&again) > 0);
    }

    /// The class mask has the unit class where the figure stands.
    #[test]
    fn a_figure_is_the_unit_class_in_the_class_mask() {
        use crate::frame::mask::class;
        let Some(gpu) = gpu(
            "a_figure_is_the_unit_class_in_the_class_mask",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let size = (96u32, 96u32);
        let (w, mut r) = world(gpu, size, [0, 0, 255], Fog::Visible);
        frames(&w, &mut r, size, 1);
        let tex = LogicalId::new("tx:unit_base");
        let (m, pack) = model(&tex, MaterialDesc::SKINNED);
        let textures = [(tex, base_texture([255, 255, 255, 0]))].into();
        r.add_unit_model_b2(
            LogicalId::new("md:warrior"),
            &m,
            Some(pack),
            None,
            &textures,
        );
        frames(&w, &mut r, size, 3);
        let mask = r.read_frame(crate::frame::FrameSource::ClassMask);
        let px = frames(&w, &mut r, size, 1);
        let mask = mask.wait().expect("the class mask");
        let units = mask
            .bytes
            .iter()
            .filter(|&&b| u32::from(b) == class::UNIT)
            .count();
        // The pixels of the figure (in the team's blue) are about the pixels of the class.
        let blue = px
            .chunks(4)
            .filter(|p| p[2] > 100 && p[0] < 40 && p[1] < 40)
            .count();
        assert!(units > 0, "no unit class in the mask");
        assert!(
            units.abs_diff(blue) * 4 <= blue.max(1),
            "unit class {units} px, figure {blue} px"
        );
    }

    /// The fog of war on a figure (the mask `FowMask`): never dimmed on a revealed and hidden cell
    /// (the original's unit shaders have no fog), not drawn on a cell nobody has seen.
    #[test]
    fn a_figure_is_not_dimmed_on_a_hidden_cell_and_not_drawn_on_an_unrevealed_one() {
        let Some(first) = gpu(
            "a_figure_is_not_dimmed_on_a_hidden_cell_and_not_drawn_on_an_unrevealed_one",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let size = (96u32, 96u32);
        let tex = LogicalId::new("tx:unit_base");
        let textures: HashMap<_, _> = [(tex.clone(), base_texture([255, 255, 255, 0]))].into();
        let pixel = |fog: Fog, gpu: crate::gpu_test::Gpu| {
            let (w, mut r) = world(gpu, size, [255, 0, 0], fog);
            frames(&w, &mut r, size, 1);
            let (m, pack) = model(&tex, MaterialDesc::SKINNED);
            r.add_unit_model_b2(
                LogicalId::new("md:warrior"),
                &m,
                Some(pack),
                None,
                &textures,
            );
            let px = frames(&w, &mut r, size, 4);
            most(&px, [255, 0, 0])
        };
        let hidden = pixel(Fog::Hidden, first);
        // The colour of the figure in the light, as on a seen cell.
        assert!((i32::from(hidden[0]) - 200).abs() <= 8, "{hidden:?}");
        assert!(hidden[1] <= 16 && hidden[2] <= 16, "{hidden:?}");
        let Some(second) = gpu(
            "a_figure_is_not_dimmed_on_a_hidden_cell_and_not_drawn_on_an_unrevealed_one",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let unseen = pixel(Fog::Unrevealed, second);
        assert!(unseen[0] < 150 || unseen[1] > 60, "nothing red: {unseen:?}");
    }

    /// The rigid `UnitShader` of the spec goes to `unit_object_vs` (the instance of the spec, not
    /// B1's), also for a mesh that sits on a bone (`HAS_BIND`, `BIND_BONE` as `override`s).
    #[test]
    fn a_rigid_mesh_and_a_rigid_mesh_on_a_bone_are_drawn_with_the_object_vertex_stage() {
        let size = (96u32, 96u32);
        for bind in [None, Some(0u16)] {
            let Some(gpu) = gpu(
                "a_rigid_mesh_and_a_rigid_mesh_on_a_bone_are_drawn_with_the_object_vertex_stage",
                wgpu::Backends::PRIMARY,
                wgpu::Features::empty(),
            ) else {
                return;
            };
            let (w, mut r) = world(gpu, size, [255, 0, 0], Fog::Visible);
            frames(&w, &mut r, size, 1);
            let tex = LogicalId::new("tx:unit_base");
            let (mut m, pack) = model(&tex, 0);
            m.materials[0].package = "UnitShader".into();
            m.meshes = vec![MeshV2 {
                bind,
                ..quad(0, false)
            }];
            let textures = [(tex, base_texture([255, 255, 255, 0]))].into();
            r.add_unit_model_b2(
                LogicalId::new("md:warrior"),
                &m,
                Some(pack),
                None,
                &textures,
            );
            let px = frames(&w, &mut r, size, 4);
            let red = most(&px, [255, 0, 0]);
            assert!(
                (i32::from(red[0]) - 200).abs() <= 4,
                "bind {bind:?}: {red:?}"
            );
            assert!(red[1] <= 4 && red[2] <= 4, "bind {bind:?}: {red:?}");
        }
    }

    /// The models of the movement path are meshes of the package `UIShader`: they get a pipeline
    /// (`ui_vs`, `ui_ps`) and a call, rigid or on a bone, with no cube in the material (the black one).
    #[test]
    fn a_uishader_mesh_of_the_path_is_drawn_rigid_and_on_a_bone() {
        let size = (96u32, 96u32);
        for bind in [None, Some(0u16)] {
            let Some(gpu) = gpu(
                "a_uishader_mesh_of_the_path_is_drawn_rigid_and_on_a_bone",
                wgpu::Backends::PRIMARY,
                wgpu::Features::empty(),
            ) else {
                return;
            };
            let (w, mut r) = world(gpu, size, [255, 0, 0], Fog::Visible);
            frames(&w, &mut r, size, 1);
            let tex = LogicalId::new("tx:unit_base");
            let (mut m, pack) = model(&tex, 0);
            m.materials[0].package = "UIShader".into();
            m.meshes = vec![MeshV2 {
                bind,
                ..quad(0, false)
            }];
            let textures = [(tex, base_texture([255, 255, 255, 255]))].into();
            r.add_unit_model_b2(
                LogicalId::new("md:warrior"),
                &m,
                Some(pack),
                None,
                &textures,
            );
            frames(&w, &mut r, size, 4);
            let draw = r.world().unit.b2_draw().expect("the draw is there");
            assert_eq!(draw.report().failed, 0, "bind {bind:?}: a pipeline failed");
            assert!(draw.call_counts().0 >= 1, "bind {bind:?}: no call");
        }
    }

    /// The effect models are meshes of the package `FXShader` (`FXShader_Skinned`): they get the
    /// premultiplied pipeline of the module `fx` and a call in the transparent list.
    #[test]
    fn an_fxshader_mesh_gets_its_pipeline_and_a_transparent_call() {
        let size = (96u32, 96u32);
        for (package, skinned) in [("FXShader", false), ("FXShader_Skinned", true)] {
            let Some(gpu) = gpu(
                "an_fxshader_mesh_gets_its_pipeline_and_a_transparent_call",
                wgpu::Backends::PRIMARY,
                wgpu::Features::empty(),
            ) else {
                return;
            };
            let (w, mut r) = world(gpu, size, [255, 0, 0], Fog::Visible);
            frames(&w, &mut r, size, 1);
            let tex = LogicalId::new("tx:unit_base");
            let (mut m, pack) = model(&tex, if skinned { MaterialDesc::SKINNED } else { 0 });
            m.materials[0].package = package.into();
            m.meshes = vec![quad(0, skinned)];
            let textures = [(tex, base_texture([255, 255, 255, 255]))].into();
            r.add_unit_model_b2(
                LogicalId::new("md:warrior"),
                &m,
                Some(pack),
                None,
                &textures,
            );
            frames(&w, &mut r, size, 4);
            let draw = r.world().unit.b2_draw().expect("the draw is there");
            assert_eq!(draw.report().failed, 0, "{package}: a pipeline failed");
            assert!(draw.call_counts().0 >= 1, "{package}: no call");
        }
    }

    /// Item 31: the god ray is an effect model that stands over the hex of the selected unit. The renderer
    /// asks for it only while a unit is selected, draws it (a transparent call, no pipeline failure) and
    /// draws nothing once the selection is cleared; with the model missing nothing is drawn and nothing fails.
    #[test]
    fn the_god_ray_is_asked_for_and_drawn_over_the_selected_unit_only() {
        use crate::entities::highlight::Highlights;
        let size = (96u32, 96u32);
        let Some(gpu) = gpu(
            "the_god_ray_is_asked_for_and_drawn_over_the_selected_unit_only",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let (w, mut r) = world(gpu, size, [255, 0, 0], Fog::Visible);
        let ray = crate::world::god_ray::model();
        let select = |r: &mut Renderer, cell| {
            r.set_highlights(Highlights {
                selected: cell,
                ..Highlights::default()
            });
        };
        // Nothing selected: the beam is not asked for (an older set without it is never asked).
        frames(&w, &mut r, size, 2);
        assert!(!r.unit_models_b2_wanted().contains(&ray));
        // Selected, the model not there: asked for, nothing drawn, nothing fails.
        select(&mut r, Some((3, 2)));
        frames(&w, &mut r, size, 2);
        assert!(r.unit_models_b2_wanted().contains(&ray));
        let draw = r.world().unit.b2_draw().expect("the draw is there");
        assert_eq!((draw.call_counts().0, draw.report().failed), (0, 0));
        // The model arrives: one call over the unit's hex.
        let tex = LogicalId::new("tx:unit_base");
        let (mut m, pack) = model(&tex, 0);
        m.materials[0].package = "FXShader".into();
        m.meshes = vec![quad(0, false)];
        let textures = [(tex, base_texture([255, 255, 255, 255]))].into();
        r.add_unit_model_b2(ray.clone(), &m, Some(pack), None, &textures);
        frames(&w, &mut r, size, 3);
        assert!(!r.unit_models_b2_wanted().contains(&ray));
        let draw = r.world().unit.b2_draw().expect("the draw is there");
        assert_eq!(draw.report().failed, 0, "a pipeline failed");
        assert!(draw.call_counts().0 >= 1, "the beam is not drawn");
        // The selection clears: the beam goes.
        select(&mut r, None);
        frames(&w, &mut r, size, 2);
        let draw = r.world().unit.b2_draw().expect("the draw is there");
        assert_eq!(draw.call_counts().0, 0, "the beam stays after the deselect");
    }

    /// The material of the god ray with constant 1x1 layer textures (the files are named as the set's, which
    /// is what makes the draw take the "God Ray" state), and its textures.
    fn god_ray_material(
        layers: &[crate::world::god_ray::reference::Layer; 3],
    ) -> (MaterialDesc, Vec<(LogicalId, TextureBlob)>) {
        let linear = |rgba: [u8; 4]| TextureBlob {
            srgb: false,
            ..base_texture(rgba)
        };
        let byte = |v: f32| (v * 255.0).round() as u8;
        let names = [
            ("BaseTextureMap0", "base_01"),
            ("UVScrollingMap0", "uv_01"),
            ("AlphaLookupMap0", "alpha_01"),
            ("BaseTextureMap1", "base_02"),
            ("UVScrollingMap1", "uv_03"),
            ("AlphaLookupMap1", "alpha_01"),
            ("BaseTextureMap2", "base_02"),
            ("UVScrollingMap2", "uv_02"),
            ("AlphaLookupMap2", "alpha_01"),
        ];
        let mut textures = Vec::new();
        let mut params = Vec::new();
        for (i, (slot, file)) in names.into_iter().enumerate() {
            let l = &layers[i / 3];
            let rgba = match i % 3 {
                0 => l.base.map(byte),
                1 => l.scroll.map(byte),
                _ => {
                    let r = byte(l.alpha_scroll_x);
                    [r, r, r, byte(l.alpha_uv_w)]
                }
            };
            // base_02 and alpha_01 are one file for two layers in the set; here each layer has its own
            // texel, so the file name carries the layer.
            let id = LogicalId::new(format!("tx:fx_god_ray_{file}_l{}.dds", i / 3));
            textures.push((id.clone(), linear(rgba)));
            params.push(TexParam {
                name: slot.into(),
                kind: TexKind::Tex2D,
                src: TexSource::File(id),
                semantic: 0,
                reg: 0,
                usage: 0,
                flags: 0,
                active: true,
            });
        }
        let desc = MaterialDesc {
            package: "FXShader".into(),
            version: 4,
            sets: vec![ParamSet {
                blocks: vec![],
                textures: params,
                constants: vec![],
            }],
            ..MaterialDesc::default()
        };
        (desc, textures)
    }

    /// A quad of the god ray model, counter-clockwise from +z (`flip` reverses the winding).
    fn god_quad(flip: bool) -> MeshV2 {
        let mut m = quad(0, false);
        let mut a = MeshArrays {
            positions: vec![
                [-20.0, 0.0, 0.0],
                [20.0, 0.0, 0.0],
                [20.0, 40.0, 0.0],
                [-20.0, 40.0, 0.0],
            ],
            normals: vec![[0.0, 0.0, 1.0]; 4],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            joints: vec![],
            weights: vec![],
            indices: vec![0, 1, 2, 0, 2, 3],
        };
        if flip {
            a.indices = vec![0, 2, 1, 0, 3, 2];
        }
        m = MeshV2::pack("quad", 0, 0, &a, false, false).unwrap_or(m);
        m
    }

    /// The pixels the god ray changes are the original's blend of its shader: `sqrt(C) + behind * sqrt(T)`
    /// in the gamma space of the target (colour `ONE`/`SRCALPHA`), with the squared texture decode and the
    /// three layers put over each other (`FXPS_CODE3`); the winding that faces the camera is drawn and the
    /// reverse one is culled (`CULL_CW` of the exe's state).
    #[test]
    fn the_god_ray_pixels_are_the_original_blend_of_the_three_layer_shader() {
        use crate::entities::highlight::Highlights;
        use crate::world::god_ray::reference::{Layer, shade};
        let size = (96u32, 96u32);
        let layers = [
            Layer {
                scroll: [0.5; 4],
                base: [0.95, 0.92, 0.6, 0.9],
                alpha_uv_w: 0.9,
                alpha_scroll_x: 0.8,
            },
            Layer {
                scroll: [0.5; 4],
                base: [0.9, 0.9, 0.5, 0.8],
                alpha_uv_w: 0.8,
                alpha_scroll_x: 0.7,
            },
            Layer {
                scroll: [0.5; 4],
                base: [0.8, 0.9, 0.9, 0.7],
                alpha_uv_w: 0.7,
                alpha_scroll_x: 0.9,
            },
        ];
        // What the 8-bit textures hold, as the shader reads them.
        let q = |v: f32| (v * 255.0).round() / 255.0;
        let stored = layers.map(|l| Layer {
            scroll: l.scroll.map(q),
            base: l.base.map(q),
            alpha_uv_w: q(l.alpha_uv_w),
            alpha_scroll_x: q(l.alpha_scroll_x),
        });
        let [sr, sg, sb, st] = shade(&stored);
        let mut results = Vec::new();
        for flip in [false, true] {
            let Some(gpu) = gpu(
                "the_god_ray_pixels_are_the_original_blend_of_the_three_layer_shader",
                wgpu::Backends::PRIMARY,
                wgpu::Features::empty(),
            ) else {
                return;
            };
            let (w, mut r) = world(gpu, size, [255, 0, 0], Fog::Visible);
            r.set_highlights(Highlights {
                selected: Some((3, 2)),
                ..Highlights::default()
            });
            let before = frames(&w, &mut r, size, 3);
            let ray = crate::world::god_ray::model();
            let (desc, textures) = god_ray_material(&layers);
            let tex = LogicalId::new("tx:unit_base");
            let (mut m, pack) = model(&tex, 0);
            m.materials = vec![desc];
            m.meshes = vec![god_quad(flip)];
            let textures = textures.into_iter().collect();
            r.add_unit_model_b2(ray, &m, Some(pack), None, &textures);
            let after = frames(&w, &mut r, size, 3);
            let draw = r.world().unit.b2_draw().expect("the draw is there");
            assert_eq!(draw.report().failed, 0, "a pipeline failed");
            let mut changed = 0;
            for (b, a) in before.chunks(4).zip(after.chunks(4)) {
                if b == a {
                    continue;
                }
                changed += 1;
                let want = [
                    sr + f32::from(b[0]) / 255.0 * st,
                    sg + f32::from(b[1]) / 255.0 * st,
                    sb + f32::from(b[2]) / 255.0 * st,
                ];
                for k in 0..3 {
                    let got = f32::from(a[k]) / 255.0;
                    assert!(
                        (got - want[k].min(1.0)).abs() <= 3.0 / 255.0,
                        "flip {flip} channel {k}: {got} vs {} (behind {b:?}, got {a:?})",
                        want[k]
                    );
                }
            }
            results.push(changed);
        }
        assert!(results[0] > 100, "the facing side is drawn: {results:?}");
        assert_eq!(results[1], 0, "the reverse side is culled: {results:?}");
    }

    /// The buildings of the cities: a `BuildingShader` mesh of a draw that takes the ported colour pass
    /// (`BuildingPS_CODE_Opaque`) gets its pipelines in the frame's layouts, rigid and skinned, before
    /// and after its colour volume is set.
    #[test]
    fn a_building_mesh_gets_the_pipeline_of_the_ported_colour_pass() {
        let size = (96u32, 96u32);
        for (package, skinned) in [("BuildingShader", false), ("BuildingShader_Skinned", true)] {
            let Some(gpu) = gpu(
                "a_building_mesh_gets_the_pipeline_of_the_ported_colour_pass",
                wgpu::Backends::PRIMARY,
                wgpu::Features::empty(),
            ) else {
                return;
            };
            let device = gpu.device.clone();
            let queue = gpu.queue.clone();
            let (w, mut r) = world(gpu, size, [255, 0, 0], Fog::Visible);
            frames(&w, &mut r, size, 1);
            let draw = r.world_mut().unit.b2_draw_mut().expect("the draw is there");
            draw.set_building_code(true);
            draw.set_code_frame(
                &queue,
                &CodeFrame {
                    fow_base: None,
                    fow_base_scale: [1.0, 1.0, 0.0, 0.0],
                    fow_key: None,
                    eye_orig: Vec3::new(0.0, 0.0, 100.0),
                },
            );
            let tex = LogicalId::new("tx:unit_base");
            let (mut m, pack) = model(&tex, if skinned { MaterialDesc::SKINNED } else { 0 });
            m.materials[0].package = package.into();
            m.meshes = vec![quad(0, skinned)];
            let textures = [(tex, base_texture([200, 200, 200, 255]))].into();
            r.add_unit_model_b2(
                LogicalId::new("md:warrior"),
                &m,
                Some(pack),
                None,
                &textures,
            );
            frames(&w, &mut r, size, 4);
            let draw = r.world_mut().unit.b2_draw_mut().expect("the draw is there");
            assert_eq!(draw.report().failed, 0, "{package}: a pipeline failed");
            assert!(draw.call_counts().0 >= 1, "{package}: no call");
            let id = LogicalId::new("md:warrior");
            let volume = identity_volume(&device, &queue, 4);
            draw.set_color_key(&device, &id, &volume);
            assert!(draw.has_color_key(&id));
            frames(&w, &mut r, size, 2);
            let draw = r.world().unit.b2_draw().expect("the draw is there");
            assert_eq!(draw.report().failed, 0, "{package}: a pipeline failed");
        }
    }

    /// A transparent material is drawn after the opaque ones, from the farthest.
    #[test]
    fn the_transparent_calls_follow_the_opaque_ones_from_the_farthest() {
        let call = |model, mesh, distance, transparent| Call {
            model,
            mesh,
            ranges: vec![],
            distance,
            transparent,
        };
        let mut list = vec![
            call(1, 0, 5.0, true),
            call(2, 0, 0.0, false),
            call(0, 0, 9.0, true),
            call(0, 1, 0.0, false),
        ];
        order(&mut list);
        let got: Vec<(usize, usize)> = list.iter().map(|c| (c.model, c.mesh)).collect();
        assert_eq!(got, [(0, 1), (2, 0), (0, 0), (1, 0)]);
    }

    /// A figure whose model has no clips is not drawn when its mesh needs the bones.
    #[test]
    fn a_skinned_mesh_of_a_model_with_no_clips_is_left_out() {
        let Some(gpu) = gpu(
            "a_skinned_mesh_of_a_model_with_no_clips_is_left_out",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let size = (64u32, 64u32);
        let (w, mut r) = world(gpu, size, [255, 0, 0], Fog::Visible);
        frames(&w, &mut r, size, 1);
        let tex = LogicalId::new("tx:unit_base");
        let (mut m, _pack) = model(&tex, MaterialDesc::SKINNED);
        m.clips.clear();
        r.add_unit_model_b2(
            LogicalId::new("md:warrior"),
            &m,
            None,
            None,
            &HashMap::new(),
        );
        frames(&w, &mut r, size, 3);
        let draw = r.world().unit.b2_draw().unwrap();
        assert_eq!(draw.call_counts(), (0, 0, 0));
    }

    /// The layout of a vertex is that of the blob, and the shader locations are the semantics.
    #[test]
    fn the_vertex_layout_is_the_layout_of_the_blob() {
        let layout = vertex_layout_of(&quad_mesh(0)).unwrap();
        // position f32x3, normal snorm16x2, uv f16x2, joints u8x4, weights unorm8x4
        assert_eq!(layout.stride(), 12 + 4 + 4 + 4 + 4);
        let at: Vec<(u32, u64)> = layout
            .wgpu_attributes()
            .iter()
            .map(|a| (a.shader_location, a.offset))
            .collect();
        assert_eq!(at, [(0, 0), (1, 12), (2, 16), (14, 20), (15, 24)]);
        // The instance takes the free TEXCOORD locations after the uv: 3..8 (six fields).
        let instance = instance_layout().attributes(&layout).unwrap();
        let locations: Vec<u32> = instance.iter().map(|a| a.shader_location).collect();
        assert_eq!(locations, [3, 4, 5, 6, 7, 8]);
    }
}

/// The runs of the arena that a flush writes (no device).
#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod upload_tests {
    use super::*;

    fn items(n: usize) -> Vec<ObjectInstance> {
        (0..n)
            .map(|k| ObjectInstance {
                world: [[1.0, 0.0, 0.0, k as f32], [0.0; 4], [0.0; 4]],
                ..Default::default()
            })
            .collect()
    }

    fn touch(v: &mut [ObjectInstance], at: usize) {
        v[at].color = [1, 2, 3, 4];
    }

    #[test]
    fn nothing_changed_writes_nothing() {
        let a = items(100);
        assert!(dirty_runs(&a, &a.clone(), 16, 32).is_empty());
        assert!(dirty_runs(&[], &[], 16, 32).is_empty());
        // A shorter list has nothing new.
        assert!(dirty_runs(&a, &a[..40], 16, 32).is_empty());
    }

    #[test]
    fn one_change_is_one_run() {
        let a = items(100);
        let mut b = a.clone();
        touch(&mut b, 37);
        assert_eq!(dirty_runs(&a, &b, 16, 32), [37..38]);
    }

    #[test]
    fn close_runs_merge_and_far_ones_do_not() {
        let a = items(200);
        let mut b = a.clone();
        touch(&mut b, 10);
        touch(&mut b, 20);
        touch(&mut b, 100);
        assert_eq!(dirty_runs(&a, &b, 16, 32), vec![10..21, 100..101]);
        assert_eq!(dirty_runs(&a, &b, 5, 32), vec![10..11, 20..21, 100..101]);
    }

    #[test]
    fn a_longer_list_has_a_dirty_tail() {
        let a = items(10);
        let b = items(25);
        assert_eq!(dirty_runs(&a, &b, 16, 32), [10..25]);
        assert_eq!(dirty_runs(&[], &b, 16, 32), [0..25]);
        let mut c = b.clone();
        touch(&mut c, 2);
        assert_eq!(dirty_runs(&a, &c, 16, 32), [2..25]);
    }

    #[test]
    fn too_many_runs_are_one_span() {
        let a = items(500);
        let mut b = a.clone();
        for k in 0..5 {
            touch(&mut b, 10 + k * 100);
        }
        assert_eq!(dirty_runs(&a, &b, 16, 32).len(), 5);
        assert_eq!(dirty_runs(&a, &b, 16, 4), [10..411]);
    }

    #[test]
    fn nan_and_negative_zero_are_compared_by_bytes() {
        let a = items(3);
        let mut b = a.clone();
        b[1].world[1][0] = f32::NAN;
        assert_eq!(dirty_runs(&a, &b, 16, 32), [1..2]);
        // The same NaN twice is no change; 0.0 and -0.0 are one.
        assert!(dirty_runs(&b, &b.clone(), 16, 32).is_empty());
        let mut c = a.clone();
        c[2].world[2][0] = -0.0;
        assert_eq!(dirty_runs(&a, &c, 16, 32), [2..3]);
    }
}

/// The culling of the instances, on the CPU (no device): a synthetic scene of many instances.
#[cfg(test)]
mod culling_tests {
    use super::*;
    use glam::{Mat4, Vec4};

    /// A top view over x and z in `-half..half` (our basis), the height 100 to -100 as depth 0 to 1.
    fn view(half: f32) -> Mat4 {
        Mat4::from_cols(
            Vec4::new(1.0 / half, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, -0.005, 0.0),
            Vec4::new(0.0, -1.0 / half, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.5, 1.0),
        )
    }

    fn grid(n: usize, step: f32) -> Vec<ObjectInstance> {
        (0..n * n)
            .map(|k| {
                let (x, z) = ((k % n) as f32 * step, (k / n) as f32 * step);
                ObjectInstance {
                    world: [[1.0, 0.0, 0.0, x], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, z]],
                    ..Default::default()
                }
            })
            .collect()
    }

    fn sphere() -> cull::ModelSphere {
        cull::ModelSphere::of_box([-1.0, 0.0, -1.0, 1.0, 3.0, 1.0], false)
    }

    fn drawn(ranges: &[Range<u32>]) -> usize {
        ranges.iter().map(|r| (r.end - r.start) as usize).sum()
    }

    /// The ranges of a pass hold exactly the instances the pass sees, and every instance whose
    /// sphere touches a view is in them: a visible one never disappears.
    #[test]
    fn a_big_scene_stages_only_what_the_passes_see() {
        // 10 000 instances 10 apart, centred on the origin (x and z from -450 to 540).
        let items: Vec<ObjectInstance> = grid(100, 10.0)
            .into_iter()
            .map(|mut i| {
                i.world[0][3] -= 450.0;
                i.world[2][3] -= 450.0;
                i
            })
            .collect();
        let cull = CullSet::new(Some(view(50.0)), Some(view(60.0)), Some(view(70.0)));
        let s = sphere();
        let mut staged = Vec::new();
        let mut masks = Vec::new();
        let out = stage_copy(&mut staged, &mut masks, &items, 0.0, &s, &cull);
        let want = |bit: u8| {
            items
                .iter()
                .filter(|i| s.passes(i, 0.0, &cull) & bit != 0)
                .count()
        };
        assert_eq!(drawn(&out.ranges[0]), want(cull::MAIN));
        assert_eq!(drawn(&out.ranges[1]), want(cull::SHADOW));
        assert_eq!(drawn(&out.ranges[2]), want(cull::REFLECTION));
        assert_eq!(out.counts.in_pass[0] as usize, want(cull::MAIN));
        // The main view sees about 10 x 10 of the 100 x 100 grid, the others a little more: most
        // of the scene is not staged at all.
        assert!(
            (90..=200).contains(&want(cull::MAIN)),
            "{}",
            want(cull::MAIN)
        );
        assert!(want(cull::SHADOW) > want(cull::MAIN) && want(cull::SHADOW) < 400);
        assert!(staged.len() < 450, "{} of {}", staged.len(), items.len());
        assert_eq!(out.counts.staged as usize, staged.len());
        // The main ranges hold the instances the main view sees, and no other.
        let place = |i: &ObjectInstance| (i.world[0][3] as i32, i.world[2][3] as i32);
        let mut at: Vec<(i32, i32)> = out.ranges[0]
            .iter()
            .flat_map(|r| staged[r.start as usize..r.end as usize].iter())
            .map(place)
            .collect();
        let mut expect: Vec<(i32, i32)> = items
            .iter()
            .filter(|i| s.passes(i, 0.0, &cull) & cull::MAIN != 0)
            .map(place)
            .collect();
        at.sort();
        expect.sort();
        assert_eq!(at, expect);
        // Whatever stands inside the box of the view is seen, by any rule.
        for i in &items {
            let (x, z) = (i.world[0][3], i.world[2][3]);
            if x.abs() <= 50.0 && z.abs() <= 50.0 {
                assert_ne!(s.passes(i, 0.0, &cull) & cull::MAIN, 0, "{x} {z}");
            }
        }
    }

    /// A low eye (a mod's first-person camera): the arena holds the instances scaled about their origin, in
    /// every pass, and the one at the eye is in the shadow ranges but not in the view's.
    #[test]
    fn a_low_eye_stages_the_instances_scaled_and_leaves_the_one_at_the_eye_out_of_the_view() {
        let items = grid(5, 20.0);
        let near = cull::NearEye {
            eye: Vec3::new(40.0, 2.0, 40.0),
            scale: 0.5,
            clearance: 2.0,
        };
        let cull = CullSet::new(None, None, None).with_near(near);
        let s = sphere();
        let mut staged = Vec::new();
        let mut masks = Vec::new();
        let out = stage_copy(&mut staged, &mut masks, &items, 0.0, &s, &cull);
        assert_eq!(drawn(&out.ranges[0]), 24, "all but the one at the eye");
        assert_eq!(drawn(&out.ranges[1]), 25, "every shadow");
        let in_range = |ranges: &[Range<u32>], at: (f32, f32)| {
            ranges
                .iter()
                .flat_map(|r| staged[r.start as usize..r.end as usize].iter())
                .any(|i| (i.world[0][3], i.world[2][3]) == at)
        };
        assert!(!in_range(&out.ranges[0], (40.0, 40.0)));
        assert!(in_range(&out.ranges[1], (40.0, 40.0)));
        assert!(in_range(&out.ranges[0], (60.0, 40.0)));
        // Every staged matrix is the scaled one: the origin where it was, the axes half as long.
        for i in &staged {
            assert_eq!(
                [i.world[0][0], i.world[1][1], i.world[2][2]],
                [0.5, 0.5, 0.5]
            );
        }
        // Without it, the instances as they are.
        let plain = CullSet::new(None, None, None);
        let mut staged2 = Vec::new();
        let out = stage_copy(&mut staged2, &mut masks, &items, 0.0, &s, &plain);
        assert_eq!(drawn(&out.ranges[0]), 25);
        assert_eq!(staged2, items);
    }

    /// The groups of a pass are few ranges, a copy of the map moved by a shift is staged by its
    /// own place, and the shadow map is of the middle copy.
    #[test]
    fn the_ranges_of_a_pass_merge_and_a_shifted_copy_has_no_shadow() {
        let items = grid(20, 10.0);
        let cull = CullSet::new(Some(view(1000.0)), Some(view(1000.0)), Some(view(40.0)));
        let s = sphere();
        let mut staged = Vec::new();
        let mut masks = Vec::new();
        let out = stage_copy(&mut staged, &mut masks, &items, 0.0, &s, &cull);
        // All in main and shadow: one range each; the reflection sees a part.
        assert_eq!(out.ranges[0].len(), 1);
        assert_eq!(out.ranges[1].len(), 1);
        assert_eq!(drawn(&out.ranges[0]), 400);
        assert!(out.ranges[2].len() <= 2);
        assert!(drawn(&out.ranges[2]) < 400);
        let before = staged.len();
        let out = stage_copy(&mut staged, &mut masks, &items, -2000.0, &s, &cull);
        assert!(out.ranges[1].is_empty(), "no shadow for a copy of the map");
        // The ranges start where this copy starts in the arena.
        assert!(out.ranges[0].iter().all(|r| r.start as usize >= before));
        // Nothing in view: nothing staged.
        let none = CullSet::new(Some(view(1.0)), Some(view(1.0)), Some(view(1.0)));
        let before = staged.len();
        let out = stage_copy(&mut staged, &mut masks, &items, 5000.0, &s, &none);
        assert_eq!(staged.len(), before);
        assert!(out.ranges.iter().all(Vec::is_empty));
    }
}
