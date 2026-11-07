//! Renderer: backend selection, surface, test-scene pipeline (§5.6).

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack, Tex};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use draw_list::UiDrawList;
use glam::{Mat4, Vec2, Vec3, Vec4};

use asset_format::{LogicalId, ModelBlob, PlatformProfile, Skeleton, TextureBlob};

use crate::blend::{BlendPipeline, BlendTargets};
use crate::camera::{CameraHints, CameraView};
use crate::decal::DecalPipeline;
use crate::entities::lines::LineMesh;
use crate::entities::marker::{CITY_MARKER, UNIT_MARKER, pillar};
use crate::entities::{
    ArtState, EntityPlan, EntitySnapshot, Highlights, ModelRef, ModelStatus, Owners, PlanContext,
    WorldLabel, icons, plot_key, roads,
};
use crate::frame::b1_systems::{
    B1Adapters, B1Draw, B1Masks, GpuDecals, GpuLines, GpuTerrain, MapDraw,
};
use crate::frame::mask::{
    CLASS_MASK, CLASS_MASK_DEPTH, MASK_FORMAT, MaskShader, declare_mask_targets,
};
use crate::frame::readback::Request;
use crate::frame::{
    CpuClock, DEPTH_FORMAT, DrawContext, GpuTimer, MINIMAP, PassId, PassStat, PassTally, Present,
    RenderSystem, SCENE, SCENE_DEPTH, SCENE_MSAA, Step, SystemId, TargetPool, WORLD_FORMAT,
    WorldSystems, copy_shifts, declare_main_targets, plan, settled,
};
use crate::frame::{FrameSource, MaskContext, Readback};
use crate::layers::LayerSwitches;
use crate::lighting::{FrameGlobals, FrameInputs, LightingEnv, LightingError, world_drawn};
use crate::model::{GpuModel, InstanceId, ModelRenderer};
use crate::overlay::{MinimapInfo, Overlay, OverlayTarget, OverlayTextures, run_overlay};
use crate::scene::{self, Mesh, Scene, SceneVertex};
use crate::settings::RenderSettings;
use crate::shaders::{ConstantOverrides, UniformBlock};
use crate::shadow::{ShadowCamera, ShadowMap, fixed_rect, visible_rect};
use crate::static_model::{
    BatchSet, Look, StaticInstance, StaticModelId, StaticRenderer, StaticTargets, Variants,
};
use crate::terrain::features::{DecalRef, FeatureArtSet, decal_mesh};
use crate::terrain::texture_array::{
    LayerPlan, STYLES as ART_STYLES, TerrainArtSlots, gpu_vertices,
};
use crate::terrain::{TerrainMesh, TerrainSnapshot};
use crate::terrain_pipeline::{TerrainMaterial, TerrainPipeline, TerrainTargets};
use crate::texture;
use crate::ui_pass::{UiExternal, UiPass, UiStats, UiTarget, UiTextures, UiTimes};
use crate::water_pipeline::{WaterPipeline, WaterTargets};
use crate::world::arrows::ArrowSpec;
use crate::world::b1_env::B1Env;
use crate::world::forest::scatter::{ForestMask, ForestTiles};
use crate::world::fow::{FOG_HIDDEN_FACTOR, FogInput};
use crate::world::objects::input::{EntityChanges, ObjectStat};
use crate::world::overlay::OwnersInput;
use crate::world::path::PathInput;
use crate::world::terrain::{GroundShare, PlotBasis, TerrainBasis, TerrainM1, partition_by_class};

/// Web `Auto` tries WebGPU, then WebGL2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendPref {
    #[default]
    Auto,
    /// Forces WebGL2 (`?gfx=webgl2`).
    WebGl2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    WebGpu,
    WebGl2,
    Vulkan,
    Dx12,
    Metal,
    Gl,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextureCompression {
    pub bc: bool,
    pub etc2: bool,
    pub astc: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendInfo {
    pub kind: BackendKind,
    pub adapter: String,
    pub driver: String,
    pub compression: TextureCompression,
}

/// What the device is, for the performance report (`Renderer::device_report`): the adapter as wgpu
/// names it, the features and the limits that matter to this renderer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceReport {
    pub adapter: String,
    pub vendor: u32,
    pub device_id: u32,
    pub device_type: String,
    pub backend: String,
    pub driver: String,
    pub driver_info: String,
    /// The features this client looks at, by name, for those the device has.
    pub features: Vec<&'static str>,
    /// Every feature the device has, as wgpu prints them.
    pub features_all: String,
    /// `(name, value)` of the limits of interest.
    pub limits: Vec<(&'static str, u64)>,
    /// The texture variants the client asks the asset server for, most preferred first.
    pub texture_variants: Vec<String>,
    pub msaa_samples: u32,
}

/// What the platform does when the GPU device is lost (the page shows an error); set once.
static DEVICE_LOST_HOOK: std::sync::OnceLock<fn(&str)> = std::sync::OnceLock::new();

/// Sets what runs when the device is lost (not destroyed by us): the message of the device. The
/// first call wins.
pub fn set_device_lost_hook(hook: fn(&str)) {
    let _ = DEVICE_LOST_HOOK.set(hook);
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("no surface target (canvas or window)")]
    NoSurfaceTarget,
    #[error("failed to create surface: {0}")]
    CreateSurface(#[from] wgpu::CreateSurfaceError),
    #[error("no suitable GPU adapter: {0}")]
    RequestAdapter(#[from] wgpu::RequestAdapterError),
    #[error("failed to create device: {0}")]
    RequestDevice(#[from] wgpu::RequestDeviceError),
    #[error("surface not supported by adapter")]
    UnsupportedSurface,
    #[error("validation error while acquiring frame")]
    SurfaceValidation,
}

/// Yields a fresh target per call (a canvas with a `webgpu` context cannot switch to WebGL2).
pub type SurfaceTargetSource<'a> = dyn FnMut() -> Option<wgpu::SurfaceTarget<'static>> + 'a;

/// Group 0 uniform, 112 bytes (WebGL2 limit is 16 KiB).
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    light_dir: [f32; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct ObjectUniform {
    pub model: [[f32; 4]; 4],
    pub tint: [f32; 4],
}

struct GpuMesh {
    vertices: Buf,
    indices: Buf,
    index_count: u32,
    uniform: Buf,
    bind_group: wgpu::BindGroup,
}

/// The art of the features: tree models in the static pipeline and the decal texture array.
struct FeatureGpu {
    art: FeatureArtSet,
    /// The tree lists of `forests.xml`, scattered once per art set.
    forest_tiles: ForestTiles,
    tree_models: HashMap<LogicalId, StaticModelId>,
    /// Decal bind group; `None` when no decal texture could be used.
    decal_material: Option<wgpu::BindGroup>,
    decals: HashMap<LogicalId, DecalRef>,
}

/// Frames between two placings again of the landmarks and cities for new heights of the terrain.
const HEIGHT_REPLACE_FRAMES: u64 = 20;
/// CPU time (ms) a frame spends on making the native trees again (cells and blocks), whatever
/// changed; the work goes on in the next frames, and the old trees stay drawn until it is done.
const FOREST_BUDGET_MS: f64 = 1.0;
/// Frames between two looks at the cuts of the landmarks.
const FOREST_CUTS_FRAMES: u64 = 10;
/// Placings whose cuts change the index of the cuts in one frame.
const FOREST_CUT_CHANGES_PER_FRAME: usize = 32;
const GRID_COLS: u32 = 20;
const GRID_ROWS: u32 = 12;
const GRID_SEED: u64 = 0x5EED_C1F5;

/// Draw calls and triangles of the last rendered frame, and the time of each pass (`?stats=1`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FrameStats {
    pub draws: u32,
    pub triangles: u32,
    /// One row per pass of table 4.1 in frame order, also for passes that did nothing; filled when
    /// the frame is encoded. GPU times lag the frame by two or three.
    pub passes: Vec<PassStat>,
    /// What the record steps of `Main` did, by draw group (`perf::groups`).
    pub groups: crate::perf::GroupTally,
    /// What the subsystems that are on did in the frame, by name (`fog.updates`): the counters of
    /// `?stats=1` (`RenderSystem::counters`).
    pub counters: Vec<(&'static str, u64)>,
    /// What the object subsystems hold and placed (WO-D13), filled with the passes.
    pub objects: Vec<ObjectStat>,
    /// The `ui.*` counters of the original-UI pass (they are also in `counters`).
    pub ui: UiStats,
    /// `ui.cpu_ms`, `ui.gpu_ms` and `ui.overlay_ms`, taken from the rows of `passes`.
    pub ui_times: UiTimes,
}

impl FrameStats {
    /// The table of the object subsystems of `?stats=1`: one line per subsystem.
    pub fn object_table(&self) -> String {
        let mut text = String::from("objects");
        for o in &self.objects {
            text += &format!("\n{:<10} {}", format!("{:?}", o.system), o.counters.line());
        }
        // WOB9: what the subsystems hand to the draw, per subsystem and in all.
        let rows: Vec<(String, _)> = self
            .objects
            .iter()
            .map(|o| (format!("{:?}", o.system), o.counters))
            .collect();
        text += "\n";
        text += &crate::anim::stats::budget_text(&crate::anim::stats::budget(&rows));
        text
    }

    /// One indexed draw of `index_count` indices, one instance.
    pub(crate) fn record_draw(&mut self, index_count: u32) {
        self.draws += 1;
        self.triangles += index_count / 3;
    }

    /// The table of `?stats=1`: one line per pass with GPU time ("-" where the device gives none),
    /// CPU time, draws and triangles.
    pub fn pass_table(&self) -> String {
        let mut text = String::from("pass            gpu ms   cpu ms  draws  triangles");
        for p in &self.passes {
            let gpu = p
                .gpu_ms
                .map_or_else(|| "     -".to_owned(), |ms| format!("{ms:6.2}"));
            text += &format!(
                "\n{:<14} {gpu}   {:6.2}  {:5}  {:9}",
                crate::frame::pass_name(p.pass),
                p.cpu_ms,
                p.draws,
                p.triangles
            );
        }
        if !self.counters.is_empty() {
            text += "\ncounters";
            for (name, value) in &self.counters {
                text += &format!(" {name}={value}");
            }
        }
        text
    }

    /// One instanced draw that shows `triangles` triangles in all.
    pub(crate) fn record_instanced(&mut self, triangles: u32) {
        self.draws += 1;
        self.triangles += triangles;
    }

    /// Adds `n` to the counter `key` of the frame (made at the first add).
    pub(crate) fn add_counter(&mut self, key: &'static str, n: u32) {
        add_tris(&mut self.counters, key, n);
    }
}

/// The passes the triangles of the objects are told apart in (`tris.<pass>.<system>`).
#[derive(Clone, Copy)]
enum TriPass {
    Main,
    Shadow,
    Reflection,
}

/// The name of the counter of the triangles `id` drew in `pass` (B3 far LOD): the systems with many
/// triangles by name, the rest together.
fn tri_key(pass: TriPass, id: SystemId) -> &'static str {
    use SystemId as S;
    match (pass, id) {
        (TriPass::Main, S::Forest) => "tris.main.forest",
        (TriPass::Main, S::Unit) => "tris.main.unit",
        (TriPass::Main, S::Landmark) => "tris.main.landmark",
        (TriPass::Main, S::City) => "tris.main.city",
        (TriPass::Main, S::Terrain) => "tris.main.terrain",
        (TriPass::Main, S::Water | S::River | S::Ice | S::Waves) => "tris.main.water",
        (TriPass::Main, _) => "tris.main.other",
        (TriPass::Shadow, S::Unit) => "tris.shadow.unit",
        (TriPass::Shadow, S::Landmark) => "tris.shadow.landmark",
        (TriPass::Shadow, S::City) => "tris.shadow.city",
        (TriPass::Shadow, _) => "tris.shadow.other",
        (TriPass::Reflection, S::Forest) => "tris.reflection.forest",
        (TriPass::Reflection, S::Unit) => "tris.reflection.unit",
        (TriPass::Reflection, S::Landmark) => "tris.reflection.landmark",
        (TriPass::Reflection, _) => "tris.reflection.other",
    }
}

/// Adds `n` to the counter `key` of the frame's triangles by system.
fn add_tris(tris: &mut Vec<(&'static str, u64)>, key: &'static str, n: u32) {
    match tris.iter_mut().find(|(k, _)| *k == key) {
        Some((_, v)) => *v += u64::from(n),
        None => tris.push((key, u64::from(n))),
    }
}

/// Units and cities (entity-render): the art, the uploaded models and the last snapshot.
#[derive(Default)]
struct EntityState {
    art: ArtState,
    /// The art of `assets-b2` (`tb:art-b2`): when it holds unit records the units draw through the
    /// models of `assets-b2` (WOB2) and B1's units are out of the plan (SIMP-011, as the terrain).
    art_b2: Option<asset_format::ArtTable>,
    /// Uploaded models; `None` is a model that failed to load.
    models: HashMap<LogicalId, Option<StaticModelId>>,
    /// Pillars of the units and of the cities (SIMP-011), made on first use.
    markers: Option<(StaticModelId, StaticModelId)>,
    /// Colours, cultures and eras of the owners; the units, cities and plots themselves are
    /// kept by the subsystems (`world::{unit, city, landmark}`, WOB0).
    owners: Owners,
    labels: Vec<WorldLabel>,
    wanted: Vec<LogicalId>,
    /// Models the last plan used for figures of units, and for the composed models of cities.
    unit_models: std::collections::BTreeSet<LogicalId>,
    city_models: std::collections::BTreeSet<LogicalId>,
    /// Instances and batches of the last rebuild.
    last: (usize, usize),
}

/// Decals of resources, improvements, encampments and ruins (icons from the SV art), and the
/// lines over the map.
#[derive(Default)]
struct DecorState {
    /// Icon textures that arrived, and those that will not come.
    icon_blobs: HashMap<LogicalId, TextureBlob>,
    icon_failed: HashSet<LogicalId>,
    /// Layer of each icon texture in the icon array.
    icon_layers: HashMap<LogicalId, DecalRef>,
    icon_material: Option<wgpu::BindGroup>,
    icons: Option<GpuDecals>,
    /// The roads. (Highlights and the path moved to `Overlay` and `MovementPath`.)
    lines: Option<GpuLines>,
}

pub struct Renderer {
    // Drop order: surface before device. `None` for an offscreen renderer (tests).
    surface: Option<wgpu::Surface<'static>>,
    config: wgpu::SurfaceConfiguration,
    /// Format of the surface view: not sRGB when the surface offers one.
    view_format: wgpu::TextureFormat,
    scale: f32,
    /// The 3D scene is drawn at this many times the size of the surface, 0.5 to 1 (`render_scale`);
    /// `Present` stretches it over the surface and the UI passes draw on the surface itself.
    render_scale: f32,
    /// How big the shadow map is (`ShadowQuality`): the player's pick on top of `ShadowLevel`.
    shadow_quality: crate::shadow::ShadowQuality,
    /// The quality was picked by the player (`set_shadow_quality`): then `Low`..`High` are real maps
    /// whatever `ShadowLevel` says (`ShadowQuality::frame_level`).
    shadow_quality_chosen: bool,
    /// The preset of the graphics choices for the report and the panel (`GfxPreset::index_of`).
    gfx_preset: u32,
    /// The draw groups the `Main` pass does not record (`perf::groups`, the debug switches of the
    /// performance panel); 0 is the normal state. Picking and the class mask do not look at it.
    disabled_groups: u32,
    /// Samples of the main pass (1 or 4): `RenderSettings::effective_msaa` when the renderer was
    /// made. The pipelines are made for this count.
    samples: u32,
    present: Present,
    /// The original UI's draw list (`ui-render`) and the textures it names.
    ui_pass: UiPass,
    ui_textures: UiTextures,
    device: wgpu::Device,
    queue: wgpu::Queue,
    info: BackendInfo,
    pipeline: wgpu::RenderPipeline,
    globals: Buf,
    globals_bind_group: wgpu::BindGroup,
    grid: GpuMesh,
    model: GpuMesh,
    object_layout: wgpu::BindGroupLayout,
    /// Replaces the test grid and sphere once the world arrives.
    terrain: Option<GpuTerrain>,
    blend: BlendPipeline,
    terrain_pipeline: TerrainPipeline,
    /// The light of the original on B1's ground (WT-0); on once the boot group has arrived.
    terrain_m1: TerrainM1,
    water: WaterPipeline,
    /// Seconds since the start: scrolling of the clouds and the waves.
    anim_time: f32,
    terrain_material: TerrainMaterial,
    /// The revision of the cutout mask the material group was made over.
    cutout_revision: u64,
    /// The snapshot last given to [`Renderer::set_terrain`], for the feature layers.
    snapshot: Option<TerrainSnapshot>,
    /// The hexes a mod lets the player draw the ground of without revealing them (mod API 1.2).
    ground_share: GroundShare,
    /// The fog as the caller gave it (`set_fog`, or the snapshot's): the minimap gets it as it is, the ground with
    /// the share.
    raw_fog: Option<FogInput>,
    decal_pipeline: DecalPipeline,
    statics: StaticRenderer,
    features: Option<FeatureGpu>,
    /// The placement masks of the trees (`forest_tile1..N`, R8), set by `set_forest_masks`.
    forest_masks: Vec<ForestMask>,
    /// The trees of the native draw are made again at the next frame (new cells, art or masks).
    forest_dirty: bool,
    /// The basis the native trees were made for, the cells of the terrain the stencil had then, and
    /// the frame of that rebuild.
    forest_orig: Option<Mat4>,
    forest_gate_cells: usize,
    /// The height revision of the terrain the landmarks and the cities were last placed on: they
    /// stand on its heights, so a cell that builds later places them again.
    landmark_height_rev: u64,
    /// The cells whose heights changed and are not final yet (a cell around them is still to build):
    /// the landmarks and the cities on them are looked at when they are, and the trees.
    height_cells_waiting: Vec<(i32, i32)>,
    forest_cells_waiting: Vec<(i32, i32)>,
    /// The frame (`frame_no`) they were last placed again for a new revision.
    landmark_replaced: u64,
    /// WOB6: how many coastal roads the terrain had when the decals of the roads were last made.
    coastal_roads_seen: usize,
    forest_rebuilt: u64,
    /// Native tree rebuilds since the start, and the time of the last one (ms), for the counters.
    forest_rebuilds: u64,
    /// The generation of the map (`Terrain::generation`) whose first full view was drawn.
    first_view_gen: std::cell::Cell<Option<u32>>,
    forest_ms: f64,
    /// The height revision the native trees stand on, and whether cells changed since that are
    /// outside the view (the trees are made again when the view comes to them).
    forest_height_rev: u64,
    /// The cuts of the landmarks the native trees were made against, and whether the landmarks
    /// changed them since.
    forest_cuts: crate::world::forest::batches::TreeCuts,
    forest_cuts_dirty: bool,
    /// The ground the roads clear of trees (the ribbon of every road), by plot: part of `forest_cuts`.
    road_cuts: std::collections::BTreeMap<u32, Vec<[glam::Vec2; 3]>>,
    /// The cuts that single placings changed, waiting to go into `forest_cuts`.
    forest_cut_queue: std::collections::VecDeque<crate::world::landmark::CutChange>,
    /// Frames the cells waited while blocks were due, and blocks uploaded since the start.
    forest_starved: u32,
    forest_cuts_at: u64,
    /// Pending cells and blocks in or near the view, and the rest (the counters).
    forest_view_pending: u64,
    forest_background: u64,
    forest_blocks_rebuilt: u64,
    /// Cells reported changed by the terrain since the start, and tree rebuilds the view skipped.
    height_cells_checked: u64,
    forest_skipped: u64,
    /// CPU time of the last `b2_frame` of the cities and of the landmarks (ms).
    city_ms: f64,
    landmark_ms: f64,
    /// Bumped when `snapshot` is replaced; with the epochs of the records it gates the per-frame
    /// feeds of the city and landmark placing (fog sets, plot grid, net).
    snapshot_gen: u64,
    feed_key: Option<(u64, u64, u64, u64, bool)>,
    frame_no: u64,
    decals: Option<GpuDecals>,
    entities: EntityState,
    decor: DecorState,
    /// Width of the map in cells, for the fog lookup in the blend shader.
    map_width: u32,
    models: ModelRenderer,
    frame_stats: FrameStats,
    /// The subsystems of the world and their services (table 4.2). B1 code draws through the
    /// adapters of `frame::b1_systems`, which stand in for the stubs here.
    world: WorldSystems,
    /// The beam of light over the selected unit (item 31, `world::god_ray`).
    god_ray: crate::world::god_ray::GodRay,
    /// The targets of the main pass (`Scene`, `SceneMsaa`, `SceneDepth`) and those the
    /// subsystems declare.
    targets: TargetPool,
    /// `ours_to_orig` of the map, set by a subsystem's `prepare`.
    world_frame: Option<Mat4>,
    /// What the water takes from the pack (group `water`) and from `tb:world-b2`; the subsystem
    /// gets a copy when it changed.
    water_data: crate::world::water::data::WaterData,
    water_data_dirty: bool,
    /// Frames the subsystem was eligible without being ready; after a few B1's water stays.
    water_unbound_frames: u32,
    /// The light cubes, colour keys and fog ramp of the `boot` group.
    lighting: LightingEnv,
    /// The world waits for the boot group: no world is drawn before `lighting.ready()`.
    boot_required: bool,
    /// The shadow map: the camera of the light and the `ShadowMap` and `ShadowBlur` passes. It runs
    /// when the world needs the boot group (the ported families read it) or the debug view is on.
    shadow: ShadowMap,
    /// `?debug=shadow`: the shadow map is shown instead of the picture.
    debug_shadow: bool,
    /// `FrameGlobals` of the frame being drawn (group 0, binding 1 of the ported families).
    frame_globals: Buf,
    /// The same for the reflection pass, with the matrix of the reflection camera (group 0 of the
    /// objects and the buildings drawn into `ReflectionColor`).
    reflection_globals: Buf,
    /// Device, layouts and formats for the render systems of mods (client-hooks R1 to R3).
    mod_gpu: crate::frame::ModGpu,
    /// Subsystems switched off; `plan` leaves them out.
    layers: LayerSwitches,
    /// What the camera of this frame asks besides its matrices; default (nothing asked) for the strategic one.
    cam_hints: CameraHints,
    /// The quality keys in force. `samples` follows them at start-up only.
    settings: RenderSettings,
    /// GPU time of the passes (timestamp queries), when the device has them.
    timer: GpuTimer,
    /// Milliseconds for the CPU time of the passes.
    cpu_clock: CpuClock,
    /// The frames of the last ~10 s (`perf::FrameRing`): the HUD graph, 1% low, verdict.
    frame_ring: crate::perf::FrameRing,
    /// Clock readings (ms) of the end of the last frame and of the start of the one being made.
    frame_clock: (Option<f64>, Option<f64>),
    /// The CPU time of the last frame sampled (the terrain's background stops past the target).
    last_frame_cpu_ms: Option<f32>,
    /// Constants that replace the renderer's own when a block is uploaded (`golden`, debugging).
    constant_overrides: ConstantOverrides,
    /// Time that replaces the animation time (`golden`: the time of a capture).
    time_override: Option<f32>,
    /// The class mask pass (`golden`): the fragment and B1's pipelines.
    mask_shader: MaskShader,
    b1_masks: B1Masks,
    /// A renderer without a window draws into this instead of a surface.
    headless: Option<Headless>,
    /// The texture of the frame being encoded, for `FrameSource::WithUi`; `None` if it cannot be
    /// copied (no `COPY_SRC` on the surface).
    output: Option<wgpu::Texture>,
    /// Frames asked for with `read_frame`, waiting for the next frame, and those whose copy is in
    /// the commands being submitted.
    requests: Vec<Request>,
    in_flight: Vec<Request>,
    /// Unit tests only: the test's right to the GPU, held until the renderer (and its device, which drops before this
    /// last field) is gone, so parallel tests do not create and poll devices at once (hangs on Windows).
    #[cfg(test)]
    _gpu_lock: crate::gpu_test::GpuLock,
}

/// The target of a renderer without a window.
struct Headless {
    texture: Tex,
    view: wgpu::TextureView,
}

impl Headless {
    fn new(device: &wgpu::Device, size: (u32, u32)) -> Self {
        let texture = device.tex(
            &wgpu::TextureDescriptor {
                label: Some("headless frame"),
                size: wgpu::Extent3d {
                    width: size.0.max(1),
                    height: size.1.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: WORLD_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            },
            Cluster::RenderTargets,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { texture, view }
    }
}

impl Renderer {
    /// Single target; use [`Self::with_targets`] for the WebGPU fallback.
    pub async fn new(
        target: wgpu::SurfaceTarget<'static>,
        size: (u32, u32),
        pref: BackendPref,
    ) -> Result<Self, RenderError> {
        let mut target = Some(target);
        Self::with_targets(&mut || target.take(), size, pref).await
    }

    /// The High preset (4 samples).
    pub async fn with_targets(
        targets: &mut SurfaceTargetSource<'_>,
        size: (u32, u32),
        pref: BackendPref,
    ) -> Result<Self, RenderError> {
        Self::with_settings(targets, size, pref, RenderSettings::default()).await
    }

    /// Web `Auto`: WebGPU first, device created before the surface so a failure leaves the canvas
    /// free; otherwise WebGL2 with the next target. `settings` are in force from the first frame;
    /// the samples of the main pass come from `effective_msaa`.
    pub async fn with_settings(
        targets: &mut SurfaceTargetSource<'_>,
        size: (u32, u32),
        pref: BackendPref,
        settings: RenderSettings,
    ) -> Result<Self, RenderError> {
        #[cfg(target_arch = "wasm32")]
        if pref == BackendPref::Auto {
            if wgpu::util::is_browser_webgpu_supported().await {
                match Self::try_webgpu(targets, size, &settings).await {
                    Ok(r) => return Ok(r),
                    Err(e) => {
                        tracing::warn!("WebGPU failed to start ({e}), falling back to WebGL2")
                    }
                }
            } else {
                tracing::info!("WebGPU unavailable in this browser, using WebGL2");
            }
        }
        #[cfg(target_arch = "wasm32")]
        if pref == BackendPref::WebGl2 {
            tracing::info!("WebGL2 forced (gfx=webgl2)");
        }
        let backends = Self::fallback_backends(pref);
        Self::try_surface_first(targets, size, backends, settings).await
    }

    #[cfg(target_arch = "wasm32")]
    fn fallback_backends(_pref: BackendPref) -> wgpu::Backends {
        wgpu::Backends::GL
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn fallback_backends(pref: BackendPref) -> wgpu::Backends {
        match pref {
            BackendPref::Auto => wgpu::Backends::PRIMARY,
            BackendPref::WebGl2 => wgpu::Backends::GL,
        }
    }

    fn instance(backends: wgpu::Backends) -> wgpu::Instance {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = backends;
        wgpu::Instance::new(desc)
    }

    #[cfg(target_arch = "wasm32")]
    async fn try_webgpu(
        targets: &mut SurfaceTargetSource<'_>,
        size: (u32, u32),
        settings: &RenderSettings,
    ) -> Result<Self, RenderError> {
        let instance = Self::instance(wgpu::Backends::BROWSER_WEBGPU);
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                ..Default::default()
            })
            .await?;
        let (device, queue) = Self::request_device(&adapter).await?;
        let target = targets().ok_or(RenderError::NoSurfaceTarget)?;
        let surface = instance.create_surface(target)?;
        Self::finish(surface, &adapter, device, queue, size, settings.clone())
    }

    /// The surface is needed to pick a compatible adapter.
    async fn try_surface_first(
        targets: &mut SurfaceTargetSource<'_>,
        size: (u32, u32),
        backends: wgpu::Backends,
        settings: RenderSettings,
    ) -> Result<Self, RenderError> {
        let instance = Self::instance(backends);
        let target = targets().ok_or(RenderError::NoSurfaceTarget)?;
        let surface = instance.create_surface(target)?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await?;
        let (device, queue) = Self::request_device(&adapter).await?;
        Self::finish(surface, &adapter, device, queue, size, settings)
    }

    async fn request_device(
        adapter: &wgpu::Adapter,
    ) -> Result<(wgpu::Device, wgpu::Queue), RenderError> {
        // WebGL2 limits on every backend, so one code path runs everywhere.
        let limits = wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());
        // Compressed textures are optional: take whatever the adapter offers.
        let features = adapter.features()
            & (wgpu::Features::TEXTURE_COMPRESSION_BC
                | wgpu::Features::TEXTURE_COMPRESSION_ASTC
                // Only if the adapter gives it: the time of the passes.
                | wgpu::Features::TIMESTAMP_QUERY);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("civ5 device"),
                required_features: features,
                required_limits: limits,
                ..Default::default()
            })
            .await?;
        // Log instead of panicking: under panic=abort a panic kills the tab.
        device.on_uncaptured_error(Arc::new(|e: wgpu::Error| {
            tracing::error!("wgpu error: {e}");
        }));
        // A lost device draws nothing more: say so (the hook shows it on the page) instead of a black canvas.
        device.set_device_lost_callback(|reason, message| {
            if reason == wgpu::DeviceLostReason::Destroyed {
                return;
            }
            let text = format!("wgpu device lost ({reason:?}): {message}");
            tracing::error!("{text}");
            if let Some(hook) = DEVICE_LOST_HOOK.get() {
                hook(&text);
            }
        });
        Ok((device, queue))
    }

    fn finish(
        surface: wgpu::Surface<'static>,
        adapter: &wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
        size: (u32, u32),
        settings: RenderSettings,
    ) -> Result<Self, RenderError> {
        let info = backend_info(adapter);
        // The shadow maps in half floats where the window's device has them (the headless renderer
        // of the tests keeps the 8-bit maps of its goldens).
        crate::shadow::detect_precision(adapter);
        let (width, height) = (size.0.max(1), size.1.max(1));
        let caps = surface.get_capabilities(adapter);
        if caps.formats.is_empty() {
            return Err(RenderError::UnsupportedSurface);
        }
        // The world is in gamma space, so a surface without sRGB takes it as it is. An adapter
        // that offers only sRGB gets `srgb_to_linear` in `Present`.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let view_format = format;
        let config = wgpu::SurfaceConfiguration {
            // COPY_SRC where the surface allows it: `read_frame(WithUi)` copies the frame out.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | (caps.usages & wgpu::TextureUsages::COPY_SRC),
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width,
            height,
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        tracing::info!(
            backend = ?info.kind,
            adapter = %info.adapter,
            driver = %info.driver,
            format = ?format,
            srgb_present = format.is_srgb(),
            compression = ?info.compression,
            "renderer: selected backend {:?}",
            info.kind
        );

        Ok(Self::assemble(
            Some(surface),
            config,
            view_format,
            device,
            queue,
            info,
            settings,
        ))
    }

    /// Pipelines and buffers for a target of `config`'s size and `view_format`; with a surface
    /// (the real renderer) or without (offscreen, tests).
    fn assemble(
        surface: Option<wgpu::Surface<'static>>,
        config: wgpu::SurfaceConfiguration,
        view_format: wgpu::TextureFormat,
        device: wgpu::Device,
        queue: wgpu::Queue,
        info: BackendInfo,
        settings: RenderSettings,
    ) -> Self {
        let samples = settings.effective_msaa();
        tracing::info!(
            requested = settings.msaa_samples,
            effective = samples,
            "renderer: MSAA samples"
        );
        let (width, height) = (config.width, config.height);
        let shader = device.create_shader_module(wgpu::include_wgsl!("scene.wgsl"));
        let uniform_layout = |label| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            })
        };
        let globals_layout = uniform_layout("globals layout");
        let object_layout = uniform_layout("object layout");
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene pipeline layout"),
            bind_group_layouts: &[Some(&globals_layout), Some(&object_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<SceneVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3],
                })],
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
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: crate::frame::multisample(samples),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: WORLD_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let globals = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("globals"),
                size: std::mem::size_of::<Globals>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });

        let grid = upload_mesh(
            &device,
            &object_layout,
            "hex grid",
            &scene::hex_grid(GRID_COLS, GRID_ROWS, GRID_SEED),
            ObjectUniform {
                model: Mat4::IDENTITY.to_cols_array_2d(),
                tint: [1.0, 1.0, 1.0, 0.15],
            },
        );
        let model = upload_mesh(
            &device,
            &object_layout,
            "icosphere",
            &scene::icosphere(3, Vec3::new(0.8, 0.25, 0.05)),
            ObjectUniform {
                model: Mat4::IDENTITY.to_cols_array_2d(),
                tint: [1.0, 1.0, 1.0, 0.8],
            },
        );
        let blend = BlendPipeline::new(
            &device,
            &globals_layout,
            &object_layout,
            BlendTargets {
                view_format: WORLD_FORMAT,
                depth_format: DEPTH_FORMAT,
                samples,
            },
        );
        let terrain_pipeline = TerrainPipeline::new(
            &device,
            &globals_layout,
            &object_layout,
            &blend.fog_layout,
            TerrainTargets {
                view_format: WORLD_FORMAT,
                depth_format: DEPTH_FORMAT,
                samples,
            },
        );
        // The cutout mask of the terrain (client-hooks D13) is made before the first material, which carries it.
        let mut cutout = crate::cutout::CutoutShared::default();
        cutout.init(&device, &queue);
        let terrain_material = terrain_pipeline.flat_material(
            &device,
            &queue,
            &cutout.refs().expect("the cutout mask was just made"),
        );
        let terrain_m1 = TerrainM1::new(&device, WORLD_FORMAT, DEPTH_FORMAT, samples);
        let water = WaterPipeline::new(
            &device,
            &queue,
            &globals_layout,
            &object_layout,
            &blend.fog_layout,
            WaterTargets {
                view_format: WORLD_FORMAT,
                depth_format: DEPTH_FORMAT,
                samples,
            },
        );
        let decal_pipeline = DecalPipeline::new(
            &device,
            &globals_layout,
            &object_layout,
            &blend.fog_layout,
            BlendTargets {
                view_format: WORLD_FORMAT,
                depth_format: DEPTH_FORMAT,
                samples,
            },
        );
        let shadow = ShadowMap::new(&device, &queue, WORLD_FORMAT);
        let statics = StaticRenderer::new(
            &device,
            &queue,
            &globals_layout,
            &object_layout,
            &blend.fog_layout,
            StaticTargets {
                view_format: WORLD_FORMAT,
                depth_format: DEPTH_FORMAT,
                samples,
            },
        );
        let models = ModelRenderer::new(
            &device,
            &queue,
            &globals_layout,
            WORLD_FORMAT,
            DEPTH_FORMAT,
            samples,
        );
        let present = Present::new(&device, view_format);
        let ui_pass = UiPass::new(&device, view_format);
        let ui_textures = UiTextures::new(&device, &queue);
        let mask_shader = MaskShader::new(&device);
        let b1_masks = B1Masks::new(&device, &mask_shader, &globals_layout, &object_layout);
        let timer = GpuTimer::new(&device, &queue);
        tracing::info!(gpu_pass_times = timer.available(), "renderer: pass timing");
        let mut world = WorldSystems::default();
        let cutout_revision = cutout.revision();
        world.services.cutout = cutout;
        world.terrain.set_settings(&settings);
        world
            .water
            .set_quality(settings.get("TerrainWaterQuality").unwrap_or(2));
        world
            .water
            .set_reflection_level(settings.get("ReflectionLevel").unwrap_or(1));
        let mut targets = TargetPool::new(false, (width, height));
        declare_main_targets(&mut targets, samples).expect("main pass targets are valid");
        crate::frame::declare_overlay_targets(
            &mut targets,
            settings.get("FOWLevel").unwrap_or(3),
            Some(crate::world::overlay::map::map_side(
                settings.get("OverlayLevel").unwrap_or(2),
            )),
        )
        .expect("overlay targets are valid");
        world.init(&device, &mut targets);
        // The overlay subsystems draw with B1's pipelines until their own passes land.
        world.bind_b1(&B1Env {
            device: device.clone(),
            queue: queue.clone(),
            object_layout: object_layout.clone(),
            blend: blend.clone(),
            samples,
        });
        world.set_fow_level(settings.get("FOWLevel").unwrap_or(3));
        world.overlay.set_overlay_settings(
            settings.get("OverlayLevel").unwrap_or(2),
            settings.get("Use Screen Space Overlay").unwrap_or(1),
        );
        let frame_globals = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("frame globals"),
                size: std::mem::size_of::<FrameGlobals>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        let reflection_globals = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("reflection frame globals"),
                size: std::mem::size_of::<FrameGlobals>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );

        // What a mod's render system needs to build its pipeline and to read the frame (client-hooks R3).
        let frame_layout = crate::frame::FrameShared::make_layout(&device);
        world
            .services
            .frame
            .bind(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mod frame globals"),
                layout: &frame_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: frame_globals.as_entire_binding(),
                }],
            }));
        let mod_gpu = crate::frame::ModGpu {
            device: device.clone(),
            queue: queue.clone(),
            view_layout: globals_layout.clone(),
            frame_layout,
            color_format: WORLD_FORMAT,
            depth_format: DEPTH_FORMAT,
            samples,
        };

        Self {
            surface,
            config,
            view_format,
            scale: 1.0,
            render_scale: 1.0,
            shadow_quality: crate::shadow::ShadowQuality::High,
            shadow_quality_chosen: false,
            gfx_preset: 0,
            disabled_groups: 0,
            samples,
            present,
            ui_pass,
            ui_textures,
            device,
            queue,
            info,
            pipeline,
            globals,
            globals_bind_group,
            grid,
            model,
            object_layout,
            terrain: None,
            blend,
            terrain_pipeline,
            terrain_m1,
            water,
            anim_time: 0.0,
            terrain_material,
            cutout_revision,
            snapshot: None,
            ground_share: GroundShare::default(),
            raw_fog: None,
            decal_pipeline,
            statics,
            features: None,
            forest_masks: Vec::new(),
            forest_dirty: true,
            forest_rebuilds: 0,
            first_view_gen: std::cell::Cell::new(None),
            forest_height_rev: u64::MAX,
            forest_cuts: Default::default(),
            forest_cuts_dirty: false,
            road_cuts: Default::default(),
            forest_cut_queue: Default::default(),
            forest_starved: 0,
            forest_cuts_at: 0,
            forest_view_pending: 0,
            forest_background: 0,
            forest_blocks_rebuilt: 0,
            height_cells_checked: 0,
            forest_skipped: 0,
            forest_ms: 0.0,
            city_ms: 0.0,
            landmark_ms: 0.0,
            snapshot_gen: 0,
            feed_key: None,
            forest_orig: None,
            forest_gate_cells: 0,
            landmark_height_rev: u64::MAX,
            height_cells_waiting: Vec::new(),
            forest_cells_waiting: Vec::new(),
            landmark_replaced: 0,
            coastal_roads_seen: 0,
            forest_rebuilt: 0,
            frame_no: 0,
            decals: None,
            entities: EntityState::default(),
            decor: DecorState::default(),
            map_width: 0,
            models,
            frame_stats: FrameStats::default(),
            world,
            targets,
            world_frame: None,
            water_data: Default::default(),
            water_data_dirty: false,
            water_unbound_frames: 0,
            lighting: LightingEnv::new(),
            boot_required: false,
            shadow,
            debug_shadow: false,
            frame_globals,
            reflection_globals,
            mod_gpu,
            layers: LayerSwitches::new(),
            cam_hints: CameraHints::default(),
            settings,
            timer,
            cpu_clock: crate::frame::timing::default_clock(),
            frame_ring: crate::perf::FrameRing::default(),
            god_ray: Default::default(),
            frame_clock: (None, None),
            last_frame_cpu_ms: None,
            constant_overrides: ConstantOverrides::default(),
            time_override: None,
            mask_shader,
            b1_masks,
            headless: None,
            output: None,
            requests: Vec::new(),
            in_flight: Vec::new(),
            #[cfg(test)]
            _gpu_lock: crate::gpu_test::GpuLock::none(),
        }
    }

    pub fn backend(&self) -> BackendInfo {
        self.info.clone()
    }

    /// Draw calls, triangles and the time of each pass of the most recent `render` call.
    pub fn last_frame_stats(&self) -> &FrameStats {
        &self.frame_stats
    }

    /// The clock for the CPU time of the passes, in milliseconds. A native renderer has one; the
    /// browser has no `Instant`, so the app gives `performance.now()`.
    pub fn set_cpu_clock(&mut self, clock: CpuClock) {
        self.cpu_clock = clock;
    }

    /// True until the first frame whose view is fully drawn (`terrain.view_ready`): the load, where
    /// the app may spend more of a frame on arriving models.
    pub fn map_loading(&self) -> bool {
        let generation = self.world.terrain.generation();
        let view_ready = self.world.terrain.live().is_some_and(|l| l.view_ready());
        let (loading, ready) = map_loading_of(self.first_view_gen.get(), generation, view_ready);
        self.first_view_gen.set(ready);
        loading
    }

    /// The CPU clock in milliseconds (the one of [`Self::set_cpu_clock`]), for budgets of the app.
    pub fn clock_ms(&self) -> f64 {
        (self.cpu_clock)()
    }

    /// True if the passes are timed on the GPU (`TIMESTAMP_QUERY`); otherwise `?stats=1` shows
    /// their CPU time.
    pub fn gpu_pass_times(&self) -> bool {
        self.timer.available()
    }

    /// Shows the map instead of the test grid, with the terrain pipeline: flat colours per layer
    /// until [`Self::set_terrain_textures`] brings the textures. Call again after any change of
    /// the snapshot. `mesh` must be built from `snapshot` (or from one with the same geometry, see
    /// `TerrainSnapshot::same_geometry`).
    pub fn set_terrain(&mut self, snapshot: &TerrainSnapshot, mesh: &TerrainMesh) {
        let vertex_data = gpu_vertices(snapshot, mesh);
        // Each region (land and skirts, water) is sorted by continent style so the ground of the
        // milestone M1 can be lit per style; B1's own pipeline draws the whole region as before.
        let style_of = |v: u32| vertex_data[v as usize].slot as usize % ART_STYLES;
        let (land, land_classes) = partition_by_class(&mesh.land, style_of);
        let (water, water_classes) = partition_by_class(&mesh.water, style_of);
        let land_classes_len = land.len() as u32;
        let water_classes = crate::world::terrain::m1::shifted(&water_classes, land_classes_len);
        let index_data: Vec<u32> = land.into_iter().chain(water).collect();
        let vertices = self.device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("terrain vertices"),
                contents: bytemuck::cast_slice(&vertex_data),
                usage: wgpu::BufferUsages::VERTEX,
            },
            Cluster::TerrainTex,
        );
        let indices = self.device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("terrain indices"),
                contents: bytemuck::cast_slice(&index_data),
                usage: wgpu::BufferUsages::INDEX,
            },
            Cluster::TerrainTex,
        );
        let shifts: &[f32] = if snapshot.wrap_x() {
            let w = snapshot.shape().world_width();
            &[0.0, -w, w]
        } else {
            &[0.0]
        };
        let copies = shifts
            .iter()
            .map(|&dx| {
                let uniform = ObjectUniform {
                    model: Mat4::from_translation(Vec3::new(dx, 0.0, 0.0)).to_cols_array_2d(),
                    tint: [1.0, 1.0, 1.0, 0.0],
                };
                let buffer = self.device.buf_init(
                    &wgpu::util::BufferInitDescriptor {
                        label: Some("terrain object"),
                        contents: bytemuck::bytes_of(&uniform),
                        usage: wgpu::BufferUsages::UNIFORM,
                    },
                    Cluster::Other,
                );
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("terrain object"),
                    layout: &self.object_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    }],
                })
            })
            .collect();
        self.terrain = Some(GpuTerrain {
            vertices,
            indices,
            index_count: index_data.len() as u32,
            land_index_count: mesh.land.len() as u32,
            land_classes,
            water_classes,
            copies,
            shifts: shifts.to_vec(),
            width: if snapshot.wrap_x() {
                snapshot.shape().world_width()
            } else {
                0.0
            },
        });
        self.set_ground_overlays(snapshot);
        // The fog of the cells goes to the terrain subsystem too: it picks the cells to build. A hex a mod shares is
        // built and drawn like fogged revealed land (mod API 1.2).
        let mut fog: Vec<_> = snapshot.cells().iter().map(|c| c.fog).collect();
        self.ground_share.ground_fog(&mut fog);
        self.world.terrain.set_fog(&fog);
        self.snapshot = Some(snapshot.clone());
        self.snapshot_gen += 1;
        // The column grid of the cutout mask follows the map (the mask stays while the grid does).
        self.world
            .services
            .cutout
            .set_map(snapshot.width(), snapshot.height(), snapshot.wrap_x());
        self.rebind_cutout();
        self.rebuild_features();
        // New ground (heights, fog): every object is placed again.
        self.invalidate_objects();
        self.rebuild_entities();
    }

    /// The column grid of the cutout mask, `None` before the first map. Columns are 4 units, 16 to the
    /// width of a hex; see [`crate::cutout`].
    pub fn cutout_grid(&self) -> Option<crate::cutout::CutoutGrid> {
        self.world.services.cutout.grid()
    }

    /// Writes the bytes of `region` into the cutout mask (client-hooks D13): one byte per column, row by
    /// row, above 0 the terrain over the column is not drawn. The first call makes the mask (all 0
    /// elsewhere); a later one changes the columns of its region and nothing else. No bake is redone.
    pub fn set_cutout_mask(
        &mut self,
        region: crate::cutout::CutoutRegion,
        bytes: &[u8],
    ) -> Result<(), crate::cutout::CutoutError> {
        self.world.services.cutout.set_mask(region, bytes)?;
        self.rebind_cutout();
        Ok(())
    }

    /// Removes the cutout mask: the terrain is drawn whole, byte for byte as without the hook.
    pub fn clear_cutout_mask(&mut self) {
        self.world.services.cutout.clear();
        self.rebind_cutout();
    }

    /// The material group of B1's ground (and M1's) is made again when the mask's texture changed.
    fn rebind_cutout(&mut self) {
        let cutout = &self.world.services.cutout;
        if self.cutout_revision == cutout.revision() {
            return;
        }
        self.cutout_revision = cutout.revision();
        if let Some(refs) = cutout.refs() {
            self.terrain_pipeline
                .rebind_cutout(&self.device, &mut self.terrain_material, &refs);
        }
    }

    /// A new basis of the map (`MapBasis`, or what B1's snapshot knows of it): the terrain
    /// subsystem builds its field again (`set_basis`).
    pub fn set_terrain_basis(&mut self, basis: TerrainBasis) {
        self.world.terrain.set_basis(basis);
    }

    /// The pieces of a style of the terrain (the blob `pc:<style>` of the group `style:<style>`).
    pub fn add_terrain_piece_set(&mut self, set: asset_format::TerrainPieceSet) {
        self.world.terrain.add_piece_set(set);
    }

    /// The terrain builds the map of a basis over several frames (about 8 ms each) instead of in the
    /// call that sets it.
    pub fn set_terrain_incremental(&mut self, on: bool) {
        self.world.terrain.set_incremental(on);
    }

    /// Several piece sets at once: the map is built again once, not per set.
    pub fn add_terrain_piece_sets(&mut self, sets: Vec<asset_format::TerrainPieceSet>) {
        self.world.terrain.add_piece_sets(sets);
    }

    /// The layer of decals the terrain bakes into its pages: `create`, `destroy`,
    /// `set_object_decals` for the producers (road, farms, trees, rivers).
    pub fn terrain_decals_mut(&mut self) -> &mut crate::world::terrain::decals::DecalLayer {
        self.world.terrain.decals_mut()
    }

    /// The ids the terrain still lacks for the map it has (pieces of the styles, textures of the
    /// table); empty when it has all.
    pub fn terrain_wanted(&self) -> Vec<LogicalId> {
        self.world.terrain.wanted()
    }

    /// A texture of the table `TerrainDef` (the diffuse, height or specular of a row), by its id
    /// (`tx:...`).
    pub fn add_terrain_def_texture(&mut self, id: LogicalId, blob: TextureBlob) {
        self.world.terrain.add_texture(id, blob);
    }

    /// The records of a `basis` diff: only the cells they touch are built again (`patch_basis`).
    pub fn patch_terrain_basis(&mut self, changes: &[(u32, PlotBasis)]) {
        self.world.terrain.patch_basis(changes);
    }

    /// The terrain subsystem: its sampler, frame and progress, for the other modules and the tests.
    pub fn terrain_system(&self) -> &crate::world::terrain::Terrain {
        &self.world.terrain
    }

    #[cfg(test)]
    pub(crate) fn terrain_system_mut(&mut self) -> &mut crate::world::terrain::Terrain {
        &mut self.world.terrain
    }

    // ---- entity-render (units and cities through the static pipeline) ----

    /// The art table has arrived: units and cities are drawn from it. Models come later through
    /// [`Self::add_entity_model`]; until then an entity waits (or is a marker if its model failed).
    pub fn set_entity_art(&mut self, table: asset_format::ArtTable) {
        self.entities.art = ArtState::Ready(table);
        self.invalidate_objects();
        self.rebuild_entities();
    }

    /// The art of `assets-b2` (`tb:art-b2`) has arrived. When it has unit records the units draw
    /// through the models of `assets-b2` as soon as the light data is in (WOB2); B1's art keeps the
    /// cities, and the units if this table has none.
    pub fn set_unit_art_b2(&mut self, table: asset_format::ArtTable) {
        let has_units = table
            .iter()
            .any(|(_, r)| matches!(r, asset_format::ArtRecord::UnitB2(_)));
        if has_units {
            self.world.unit.b2_attach(
                &self.device,
                &self.queue,
                WORLD_FORMAT,
                DEPTH_FORMAT,
                self.samples,
            );
        }
        // WOB4: the landmarks draw through a second draw of the same kind, in `Main.Landmark`.
        let has_landmarks = table.landmarks().next().is_some();
        if has_landmarks {
            self.world.landmark.b2_attach(
                &self.device,
                &self.queue,
                WORLD_FORMAT,
                DEPTH_FORMAT,
                self.samples,
            );
        }
        // WOB5: the cities draw through a third one, in `Main.City`.
        let has_cities = table
            .iter()
            .any(|(_, r)| matches!(r, asset_format::ArtRecord::CityB2(_)));
        if has_cities {
            self.world.city.b2_attach(
                &self.device,
                &self.queue,
                WORLD_FORMAT,
                DEPTH_FORMAT,
                self.samples,
            );
        }
        self.entities.art_b2 = (has_units || has_landmarks || has_cities).then_some(table);
        self.invalidate_objects();
    }

    /// The technologies of the landmark art list (`Catalog.art_techs`, by type name, in the bit
    /// order of `PlayerView.art_techs`): the choice of a landmark can depend on one (WOB4).
    pub fn set_landmark_art_techs(&mut self, names: Vec<String>) {
        self.world.landmark.set_art_techs(names);
    }

    /// True if the landmarks draw through the models of `assets-b2` in the next frame.
    fn landmarks_b2_ready(&self, world_drawn: bool) -> bool {
        self.boot_required
            && world_drawn
            && self.entities.art_b2.is_some()
            && self.world.landmark.b2_draw().is_some()
            && self.world.terrain.sampler().is_some()
    }

    /// True if the cities draw through the models of `assets-b2` in the next frame.
    fn cities_b2_ready(&self, world_drawn: bool) -> bool {
        self.boot_required
            && world_drawn
            && self.entities.art_b2.is_some()
            && self.world.city.b2_draw().is_some()
            && self.world.terrain.sampler().is_some()
    }

    /// The far-view LOD is on (`FarLOD`, [`crate::world::far_lod`]).
    fn far_lod(&self) -> bool {
        self.settings.get("FarLOD") == Some(1)
    }

    /// True if the units draw through the models of `assets-b2` in the next frame.
    fn units_b2_ready(&self, world_drawn: bool) -> bool {
        self.boot_required
            && world_drawn
            && self.entities.art_b2.is_some()
            && self.world.unit.b2_draw().is_some()
            && self.world.terrain.sampler().is_some()
    }

    /// The models of `assets-b2` the units' figures need and that have not been given yet.
    /// The figures of the units within `radius` of `near` (X, Z) and the clips they play now: the sounds of the units'
    /// animations are timed by them (re-b15 ITEM 4).
    pub fn sounding_figures(&self, near: Vec2, radius: f32) -> Vec<crate::world::unit::FigureClip> {
        let mut out = Vec::new();
        self.world
            .unit
            .sounding_figures(&self.world.services.anim, near, radius, &mut out);
        out
    }

    pub fn unit_models_b2_wanted(&self) -> Vec<LogicalId> {
        let mut wanted = self.world.unit.b2_models_wanted();
        // The models of the landmarks come the same way (WOB4).
        for id in self
            .world
            .landmark
            .b2_models_wanted()
            .into_iter()
            .chain(self.world.city.b2_models_wanted())
        {
            if !wanted.contains(&id) {
                wanted.push(id);
            }
        }
        wanted
    }

    /// Gives a model of `assets-b2` with the animation data it names (`anim_pack`, `fsm` of the
    /// blob) and the textures of its materials (`UnitDraw::textures_of`). A model that has no clips,
    /// or whose animation data is bad, is drawn in the pose of its meshes; the log says why.
    pub fn add_unit_model_b2(
        &mut self,
        id: LogicalId,
        model: &asset_format::ModelBlobV2,
        pack: Option<asset_format::AnimPack>,
        fsm: Option<asset_format::IdleFsm>,
        textures: &HashMap<LogicalId, TextureBlob>,
    ) {
        match crate::anim::ModelAnim::new(model, pack, fsm.as_ref()) {
            Ok(anim) => self.world.services.anim.register_model(id.clone(), anim),
            Err(why) => tracing::warn!("the animation of {}: {why}; it stands still", id.as_str()),
        }
        // A model a city waits for is the city's (WOB5): its catalog, wall, ruins or a wonder. Every
        // mesh is a part of its own, so the ground and the building of a lot are drawn apart.
        if self.world.city.wants_model(&id) {
            self.world.city.register_model(id.clone(), model);
            if let Some(draw) = self.world.city.b2_draw_mut() {
                let split = crate::world::city::compose::split_by_mesh(model);
                draw.add_model(&self.device, &self.queue, id, &split, textures);
            }
        // A model a plot waits for is a landmark's; the others are the figures of the units.
        } else if self.world.landmark.wants_model(&id) {
            self.world.landmark.register_model(id.clone(), model);
            if let Some(draw) = self.world.landmark.b2_draw_mut() {
                draw.add_model(&self.device, &self.queue, id, model, textures);
            }
        } else if let Some(draw) = self.world.unit.b2_draw_mut() {
            draw.add_model(&self.device, &self.queue, id, model, textures);
        }
    }

    /// There is no art table (no assets): every unit and city is a marker.
    pub fn set_entity_art_unavailable(&mut self) {
        self.entities.art = ArtState::Unavailable;
        self.invalidate_objects();
        self.rebuild_entities();
    }

    /// Every unit and city is placed again at the next rebuild (WO-D2: new ground, art or owners).
    fn invalidate_objects(&mut self) {
        self.world.unit.invalidate_all();
        self.world.city.invalidate_all();
        self.world.landmark.invalidate_all();
    }

    /// Models the current units and cities need and that have not been given yet.
    pub fn entity_models_wanted(&self) -> &[LogicalId] {
        &self.entities.wanted
    }

    /// Uploads one static model (all meshes are one object) with its textures.
    pub fn add_entity_model(
        &mut self,
        id: LogicalId,
        blob: &ModelBlob,
        textures: &HashMap<LogicalId, TextureBlob>,
    ) {
        // The figures of units are drawn with the ported `UnitShader` when the world waits for the
        // light data (the assets of B2); with B1's assets they keep B1's lambert.
        // The composed city model is drawn with the ported `BuildingShader` the same way (WOB1).
        let look = match self.boot_required {
            true if self.entities.unit_models.contains(&id) => Look::Unit,
            true if self.entities.city_models.contains(&id) => Look::Building,
            _ => Look::Plain,
        };
        let model = self.statics.create_model_as(
            &self.device,
            &self.queue,
            blob,
            textures,
            Variants::Single,
            false,
            look,
        );
        self.entities.models.insert(id.clone(), Some(model));
        self.model_changed(&id);
    }

    /// The model will not come; what uses it becomes a marker.
    pub fn fail_entity_model(&mut self, id: LogicalId) {
        self.entities.models.insert(id.clone(), None);
        self.model_changed(&id);
    }

    /// A model arrived or failed: the units and cities whose squad or city model it is are placed
    /// again, the others keep their placement.
    fn model_changed(&mut self, id: &LogicalId) {
        self.world.unit.invalidate_model(id);
        self.world.city.invalidate_model(id);
        self.rebuild_entity_batches();
    }

    /// Shows the units and cities of `snapshot`, replacing the previous ones. Call on every new
    /// world. The snapshot is compared with the records held and only the differences are applied
    /// (the keyed inputs of WO-D2, [`Self::apply_entity_changes`]).
    pub fn set_entities(&mut self, snapshot: EntitySnapshot) {
        let changes = self.diff_entities(&snapshot);
        self.apply_entity_changes(changes);
    }

    /// What `snapshot` differs in from the records held.
    fn diff_entities(&self, snapshot: &EntitySnapshot) -> EntityChanges {
        let owners = snapshot.owners();
        let mut changes = EntityChanges {
            owners: (owners != self.entities.owners).then_some(owners),
            ..EntityChanges::default()
        };
        let units = &self.world.unit;
        let mut seen: HashSet<_> = HashSet::with_capacity(snapshot.units.len());
        for u in &snapshot.units {
            let key = crate::world::unit::Unit::key_of(u);
            seen.insert(key);
            if units.get(&key) != Some(u) {
                changes.units.push(u.clone());
            }
        }
        changes.units_removed = units
            .units()
            .map(crate::world::unit::Unit::key_of)
            .filter(|k| !seen.contains(k))
            .collect();
        let cities = &self.world.city;
        let mut seen: HashSet<_> = HashSet::with_capacity(snapshot.cities.len());
        for c in &snapshot.cities {
            let key = crate::world::city::City::key_of(c);
            seen.insert(key);
            if cities.get(&key) != Some(c) {
                changes.cities.push(c.clone());
            }
        }
        changes.cities_removed = cities
            .cities()
            .map(crate::world::city::City::key_of)
            .filter(|k| !seen.contains(k))
            .collect();
        let landmark = &self.world.landmark;
        let mut seen: HashSet<_> = HashSet::with_capacity(snapshot.plots.len());
        for p in &snapshot.plots {
            let key = plot_key(p.x, p.y);
            seen.insert(key);
            if landmark.get(&key) != Some(p) {
                changes.plots.push((key, Some(p.clone())));
            }
        }
        changes.plots.extend(
            landmark
                .plots()
                .map(|p| plot_key(p.x, p.y))
                .filter(|k| !seen.contains(k))
                .map(|k| (k, None)),
        );
        changes
    }

    /// The keyed inputs of the object subsystems (WO-D2): only the units, cities and plots that
    /// changed are placed again; the owners (colours, cultures, eras) change every placement.
    pub fn apply_entity_changes(&mut self, changes: EntityChanges) {
        if changes.is_empty() {
            return;
        }
        let mut owners_changed = false;
        if let Some(owners) = changes.owners {
            owners_changed = owners != self.entities.owners;
            self.entities.owners = owners;
        }
        if let Some(names) = changes.art_techs {
            self.world.landmark.set_art_techs(names);
        }
        if changes.clear {
            self.world.unit.clear();
            self.world.city.clear();
            self.world.landmark.clear();
        }
        for key in changes.units_removed {
            self.world.unit.remove(key);
        }
        for u in changes.units {
            self.world.unit.upsert(u);
        }
        for (key, extra) in changes.unit_extras {
            self.world.unit.set_extra(key, extra);
        }
        let mut cities_changed = changes.clear;
        if changes.clear {
            self.world.particle.cities_cleared();
        }
        for key in changes.cities_removed {
            self.world.particle.city_removed(key);
            cities_changed |= self.world.city.remove(key);
        }
        for c in changes.cities {
            cities_changed |= self.world.city.upsert(c);
        }
        let plots_changed = self.world.landmark.set_plots(&changes.plots) || changes.clear;
        if owners_changed {
            self.invalidate_objects();
        }
        self.rebuild_entity_batches();
        // The icons and the roads depend on the plots, the cities (roads join them), the owners'
        // art through the table, and the ground; a move of a unit changes none of them.
        if plots_changed || cities_changed || owners_changed {
            self.rebuild_decor();
        }
    }

    /// The counters of the object subsystems (`?stats=1`, WO-D13).
    pub fn object_stats(&self) -> Vec<ObjectStat> {
        let w = &self.world;
        [
            (SystemId::Unit, w.unit.counters()),
            (SystemId::City, w.city.counters()),
            (SystemId::Landmark, w.landmark.counters()),
            (SystemId::Forest, w.forest.counters()),
            (SystemId::Particle, w.particle.counters()),
        ]
        .into_iter()
        .map(|(system, counters)| ObjectStat { system, counters })
        .collect()
    }

    /// Flags and banners of the shown units and cities, for the UI.
    pub fn entity_labels(&self) -> &[WorldLabel] {
        &self.entities.labels
    }

    /// The terrain last given to [`Self::set_terrain`].
    pub fn terrain_snapshot(&self) -> Option<&TerrainSnapshot> {
        self.snapshot.as_ref()
    }

    /// The revision of the relief's heights (grows at every change); `None` without a map.
    pub fn ground_revision(&self) -> Option<u64> {
        self.world.terrain.sampler().map(|s| s.height_revision())
    }

    /// The relief's height (our `Y`) at the centre of every plot, row by row from `y = 0`, as the
    /// exe's `GridToWorld` gives it (the terrain sampler's height at the hex centre, but not under the
    /// sea: a unit flag over a boat or an embarked unit stands on the water, not on the sea floor, where it
    /// was off the unit and out of reach of the mouse); `None` without a map. With [`Self::ground_revision`] the caller knows when to ask again.
    pub fn plot_center_heights(&self) -> Option<(u32, u32, Vec<f32>)> {
        let sampler = self.world.terrain.sampler()?;
        let snap = self.snapshot.as_ref()?;
        let (w, h) = (snap.width(), snap.height());
        let mut out = Vec::with_capacity((w * h) as usize);
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                out.push(sampler.surface_height(crate::hexgrid::cell_center(x, y)));
            }
        }
        Some((w, h, out))
    }

    /// Instances and batches of the units and cities now shown.
    pub fn entity_counts(&self) -> (usize, usize) {
        self.entities.last
    }

    fn entity_marker_models(&mut self) -> (StaticModelId, StaticModelId) {
        if let Some(m) = self.entities.markers {
            return m;
        }
        let mut make = |size| {
            self.statics.create_model(
                &self.device,
                &self.queue,
                &pillar(size),
                &HashMap::new(),
                Variants::Single,
                false,
            )
        };
        let m = (make(UNIT_MARKER), make(CITY_MARKER));
        self.entities.markers = Some(m);
        m
    }

    /// Units and cities, then what lies on the plots and the lines over the map.
    fn rebuild_entities(&mut self) {
        self.rebuild_entity_batches();
        self.rebuild_decor();
    }

    /// Places the units and cities that changed (the others keep their placement, WO-D2) and gives
    /// the whole to B1's static pipeline.
    fn rebuild_entity_batches(&mut self) {
        let Some(terrain) = &self.snapshot else {
            self.statics
                .replace_set(&self.device, BatchSet::Entities, &[]);
            self.entities.labels.clear();
            self.entities.wanted.clear();
            self.entities.last = (0, 0);
            return;
        };
        let models = &self.entities.models;
        let status = |id: &LogicalId| match models.get(id) {
            None => ModelStatus::Pending,
            Some(None) => ModelStatus::Failed,
            Some(Some(_)) => ModelStatus::Ready,
        };
        let ctx = PlanContext {
            terrain,
            art: &self.entities.art,
            status: &status,
        };
        let owners = &self.entities.owners;
        let units_b2 = self.world.unit.b2_active();
        if !units_b2 {
            self.world.unit.refresh(owners, &ctx);
        }
        self.world.city.refresh(owners, &ctx);
        let mut plan = EntityPlan::default();
        if !units_b2 {
            self.world.unit.assemble(&mut plan);
        }
        let unit_instances = plan.instance_count();
        let unit_batches = plan.batches.len();
        self.world.city.assemble(&mut plan);
        let EntityPlan {
            batches,
            labels,
            wanted,
            unit_models,
            city_models,
        } = plan;
        let city_instances = batches.values().map(Vec::len).sum::<usize>() - unit_instances;
        if !units_b2 {
            self.world.unit.set_assembled(unit_instances, unit_batches);
        }
        self.world
            .city
            .set_assembled(city_instances, batches.len().saturating_sub(unit_batches));
        let needs_markers = batches
            .keys()
            .any(|m| matches!(m, ModelRef::UnitMarker | ModelRef::CityMarker));
        let markers = needs_markers.then(|| self.entity_marker_models());
        let sets: Vec<(StaticModelId, &[StaticInstance])> = batches
            .iter()
            .filter_map(|(m, v)| {
                let id = match m {
                    ModelRef::Asset(id) => (*self.entities.models.get(id)?)?,
                    ModelRef::UnitMarker => markers?.0,
                    ModelRef::CityMarker => markers?.1,
                };
                Some((id, v.as_slice()))
            })
            .collect();
        self.entities.last = (batches.values().map(Vec::len).sum(), sets.len());
        tracing::debug!(
            instances = self.entities.last.0,
            batches = sets.len(),
            wanted = wanted.len(),
            labels = labels.len(),
            "entities rebuilt"
        );
        self.statics
            .replace_set(&self.device, BatchSet::Entities, &sets);
        self.entities.labels = labels;
        self.entities.wanted = wanted.into_iter().collect();
        self.entities.unit_models = unit_models;
        self.entities.city_models = city_models;
    }

    // ---- decals, roads and highlights ----

    /// Textures of the icons the current plots need (resources, improvements, encampments, ruins)
    /// that have not been given yet: the SV icons of `sv/<art define>` in the art table.
    pub fn icon_textures_wanted(&self) -> Vec<LogicalId> {
        let ArtState::Ready(table) = &self.entities.art else {
            return Vec::new();
        };
        let mut out: Vec<LogicalId> = icons::wanted_tags_of(self.world.landmark.plots())
            .iter()
            .filter_map(|tag| table.sv(tag).map(|sv| sv.icon.clone()))
            .filter(|id| {
                !self.decor.icon_blobs.contains_key(id) && !self.decor.icon_failed.contains(id)
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Icon textures that arrived. The icon array is rebuilt from everything that has come so far.
    pub fn add_icon_textures(&mut self, textures: HashMap<LogicalId, TextureBlob>) {
        if textures.is_empty() {
            return;
        }
        self.decor.icon_blobs.extend(textures);
        self.rebuild_icon_material();
        self.rebuild_decor();
    }

    /// The texture will not come; its icon is left out.
    pub fn fail_icon_texture(&mut self, id: LogicalId) {
        self.decor.icon_failed.insert(id);
    }

    fn rebuild_icon_material(&mut self) {
        let mut ids: Vec<LogicalId> = self.decor.icon_blobs.keys().cloned().collect();
        ids.sort();
        let plan = LayerPlan::build(
            &TerrainArtSlots::from_textures(&ids),
            &self.decor.icon_blobs,
        );
        for (id, why) in &plan.rejected {
            tracing::warn!(id = id.as_str(), "icon texture left out: {why}");
        }
        self.decor.icon_material = match self.decal_pipeline.material(
            &self.device,
            &self.queue,
            &plan,
            &self.decor.icon_blobs,
        ) {
            Ok(m) => Some(m),
            Err(e) => {
                tracing::warn!("icon textures unavailable ({e}); no icons");
                None
            }
        };
        self.decor.icon_layers = ids
            .iter()
            .zip(&plan.slots)
            .filter_map(|(id, slot)| {
                Some((
                    id.clone(),
                    DecalRef {
                        layer: slot.layer?,
                        uv_scale: slot.scale,
                        alpha: 1.0,
                    },
                ))
            })
            .collect();
        tracing::info!(layers = plan.layers.len(), "icon textures in use");
    }

    /// What to highlight: the selected plot and the path. Replaces the previous ones; an empty
    /// [`Highlights`] clears everything.
    ///
    /// The selected plot goes to `Overlay` (the ring of the original), the path to `MovementPath`
    /// (`set_path`). The reachable cells and the targets of `highlights` are not drawn here: they
    /// go through the styles of `Highlights.xml` (`set_highlight_set`).
    pub fn set_highlights(&mut self, highlights: Highlights) {
        self.world.overlay.set_highlights(&highlights);
        self.world.movement_path.set_path(PathInput {
            steps: highlights.path,
        });
    }

    /// The cell the selection ring was drawn on in the last frame (a cell and the ring textures), for
    /// the stats.
    pub fn selection_ring(&self) -> Option<(i32, i32)> {
        self.world.overlay.ring_drawn()
    }

    /// Pins the turn (radians) and `fAlpha` of the selection ring (golden: the capture's); `None`
    /// follows the clock.
    pub fn set_ring_pin(&mut self, pin: Option<(f32, f32)>) {
        self.world.overlay.set_ring_pin(pin);
    }

    /// The path to show (`MovementPath`); an empty one clears it.
    pub fn set_path(&mut self, path: PathInput) {
        self.world.movement_path.set_path(path);
    }

    /// The arrows of the ranged attack and the trade routes (`Arrows`); an empty list clears them.
    pub fn set_arrows(&mut self, arrows: Vec<ArrowSpec>) {
        self.world.arrows.set_arrows(arrows);
    }

    /// The trade route lines of the active player (`Trade`); `routes` of the other players are not
    /// drawn. The same list again does nothing.
    pub fn set_trade_routes(&mut self, active: i8, routes: Vec<crate::world::trade::TradeRoute>) {
        self.world.trade.set_routes(active, routes);
    }

    /// The popup route (`TradeVisuals_ActivatePopupRoute`), drawn whoever owns it; `None` takes it away.
    pub fn set_trade_popup(&mut self, popup: Option<crate::world::trade::TradeRoute>) {
        self.world.trade.set_popup(popup);
    }

    /// `g_bShowTradeRoutes`, the "Show Trade" option of the minimap. On at the start.
    pub fn set_show_trade(&mut self, show: bool) {
        self.world.trade.set_show(show);
    }

    pub fn show_trade(&self) -> bool {
        self.world.trade.show()
    }

    /// The textures of the trade routes (`world::trade::TEXTURES`) for the loader.
    pub fn trade_texture_ids(&self) -> Vec<LogicalId> {
        crate::world::trade::TEXTURES
            .iter()
            .map(|t| LogicalId::new(*t))
            .collect()
    }

    /// Takes the textures of the trade routes from `textures`; a type without one keeps a plain
    /// ribbon. Returns how many were taken.
    pub fn set_trade_textures(&mut self, textures: &HashMap<LogicalId, TextureBlob>) -> usize {
        self.world.trade.set_textures(textures)
    }

    /// The fog of the map (`FluidFow`). The same fog again does nothing; a change of a few cells
    /// writes a few texels. `set_terrain` seeds it from the snapshot until the caller feeds it.
    pub fn set_fog(&mut self, fog: FogInput) {
        self.feed_fog(fog);
        self.share_fog_state();
    }

    /// The fog to both fogs: the fluid one and the classic one (`FOWLevel` 0). They cover the ground, so they get it
    /// with the hexes a mod shares revealed and not seen; the minimap tells the player what was revealed and gets the
    /// fog as it is.
    fn feed_fog(&mut self, fog: FogInput) {
        self.world.minimap.set_fog(&fog);
        self.raw_fog = Some(fog.clone());
        let ground = self.ground_share.ground_input(fog);
        self.world.fow_classic.set_fog(Some(ground.clone()));
        self.world.fluid_fow.set_fog(ground);
    }

    /// The hexes a mod lets the player draw the ground of although the player has not revealed them (mod API 1.2,
    /// `GameMsg::TerrainShare`), with their basis records: the terrain builds and draws them like fogged revealed
    /// land (relief, ground, water, the trees of their features), so a first-person camera there stands on the
    /// relief every client sees. The minimap, the snapshot (units, cities, improvements, routes, resources) and the
    /// camera bounds keep the real fog. A shared hex that is unrevealed takes the record when the basis differs (a
    /// basis made from a snapshot that knew nothing of it); with the game's `MapBasis` they are the same. The same
    /// share again does nothing; an empty one ends it.
    pub fn set_ground_share(&mut self, records: Vec<(u32, PlotBasis)>) {
        let share = GroundShare::new(records);
        if share == self.ground_share {
            return;
        }
        self.ground_share = share;
        // The records of the unrevealed shared hexes the basis does not have.
        let unrevealed = |i: u32| {
            self.snapshot
                .as_ref()
                .and_then(|s| s.cells().get(i as usize))
                .is_none_or(|c| c.fog == crate::terrain::Fog::Unrevealed)
        };
        let patch: Vec<(u32, PlotBasis)> = match self.world.terrain.basis() {
            Some(b) => self
                .ground_share
                .records()
                .iter()
                .filter(|(i, r)| unrevealed(*i) && b.plots.get(*i as usize).is_some_and(|p| p != r))
                .copied()
                .collect(),
            None => Vec::new(),
        };
        if !patch.is_empty() {
            self.world.terrain.patch_basis(&patch);
        }
        if let Some(s) = self.snapshot.as_ref() {
            let mut fog: Vec<_> = s.cells().iter().map(|c| c.fog).collect();
            self.ground_share.ground_fog(&mut fog);
            self.world.terrain.set_fog(&fog);
        }
        if let Some(raw) = self.raw_fog.clone() {
            self.feed_fog(raw);
            self.share_fog_state();
        }
    }

    /// The hexes of the ground share in force (`set_ground_share`).
    pub fn ground_share(&self) -> &GroundShare {
        &self.ground_share
    }

    /// The fog as the caller gave it, before the ground share (what the minimap has).
    #[cfg(test)]
    pub(crate) fn fog_as_given(&self) -> Option<&FogInput> {
        self.raw_fog.as_ref()
    }

    /// The owner of every cell and the colour of every owner (`Overlay`: the borders). The same
    /// owners again do nothing.
    pub fn set_owners(&mut self, owners: OwnersInput) {
        self.world.minimap.set_owners(&owners);
        self.world.overlay.set_owners(owners);
    }

    /// The viewer's bridges over rivers (the original's `GameplayBridgeChanged`): whether the viewer's team knows the
    /// technology with `BridgeBuilding`, and whether the bridges are iron. The routes are placed again when it changes.
    pub fn set_bridges(&mut self, bridge: bool, iron: bool) {
        self.world
            .landmark
            .set_bridges(Some(crate::world::landmark::routes::BridgeState {
                bridge,
                iron,
            }));
    }

    /// The subsystems that draw over the map read the fog-state texture `FluidFow` owns.
    fn share_fog_state(&mut self) {
        let fog = self.world.fluid_fow.fog_gpu();
        self.world.overlay.set_fog_state(fog);
    }

    /// Time in seconds; the selection ring pulses with it. Call once per frame.
    pub fn set_time(&mut self, seconds: f32) {
        self.world.overlay.set_time(seconds);
        self.world.trade.set_time(seconds);
    }

    /// Turn numbers on the shown path, for the UI (like [`Self::entity_labels`]).
    pub fn path_labels(&self) -> &[WorldLabel] {
        self.world.movement_path.labels()
    }

    /// What a render system of a mod builds its pipeline from: device, queue, the layouts of group 0 and of
    /// `FrameGlobals`, the formats and the sample count of the main pass.
    pub fn mod_gpu(&self) -> &crate::frame::ModGpu {
        &self.mod_gpu
    }

    /// Registers a render system of a mod (`SystemId::Mod(_)`, client-hooks R1). It is initialised at once and
    /// joins the frame in its slots, after the engine's systems. Without any, the frame is the engine's own.
    pub fn add_mod_system(
        &mut self,
        system: Box<dyn RenderSystem>,
    ) -> Result<(), crate::frame::ModSystemError> {
        self.world
            .register_mod(system, &self.device, &mut self.targets)
    }

    /// The subsystems of the world, for the inputs that have no facade of their own.
    pub fn world_mut(&mut self) -> &mut WorldSystems {
        &mut self.world
    }

    /// The subsystems of the world, for their counters and state.
    pub fn world(&self) -> &WorldSystems {
        &self.world
    }

    fn rebuild_decor(&mut self) {
        self.rebuild_icons();
        self.rebuild_lines();
    }

    /// The fields of the farms go to the layer of decals of the terrain, which bakes them into its
    /// pages (WO-D7, `landmark::decals`). They wait for the types of the layer and the size of the
    /// map.
    fn apply_farm_decals(&mut self) {
        // A turn that changes many farms gives them to the layer over a few frames.
        const FARM_PLOTS_PER_FRAME: usize = 48;
        let Some((farms, touched)) = self.world.landmark.farm_update_of(FARM_PLOTS_PER_FRAME)
        else {
            return;
        };
        let Some(width) = self.snapshot.as_ref().map(|s| s.width() as i32) else {
            return;
        };
        if self.world.terrain.decals().types().is_empty() {
            return;
        }
        let Some(basis) = self.world.terrain.basis() else {
            return;
        };
        let Some(heading) = self
            .world
            .terrain
            .frame()
            .map(crate::world::landmark::heading::HeadingField::new)
        else {
            return;
        };
        // The continent art of each hex picks the crops of its style.
        let continent: std::collections::HashMap<(i32, i32), u8> = farms
            .iter()
            .map(|f| {
                let c = basis.plot(f.x, f.y).map_or(0, |p| p.continent);
                ((f.x, f.y), c)
            })
            .collect();
        crate::world::landmark::decals::apply(
            self.world.terrain.decals_mut(),
            &farms,
            &touched,
            width,
            &heading,
            &|x, y| continent.get(&(x, y)).copied().unwrap_or(0),
        );
        self.world.landmark.farms_applied(&farms, &touched);
    }

    /// The ground meshes of the landmarks and of the cities (`LandmarkShader`) go to the layer of
    /// decals of the terrain, so they lie on the relief (`landmark::ground`) under the fog of the
    /// terrain; a plot whose ground changed is given again.
    fn apply_ground_decals(&mut self) {
        let Some(width) = self.snapshot.as_ref().map(|s| s.width() as i32) else {
            return;
        };
        if let Some((decals, cells)) = self.world.landmark.take_ground_update(width) {
            self.world.terrain.decals_mut().set_mesh_decals(
                crate::world::terrain::decals::DecalSource::Landmarks,
                &cells,
                decals,
            );
        }
        // The ground of the cities, as the original's `cvCityVisSystem::InitMeshDecals`.
        if let Some((decals, cells)) = self.world.city.take_ground_update(width) {
            self.world.terrain.decals_mut().set_mesh_decals(
                crate::world::terrain::decals::DecalSource::Cities,
                &cells,
                decals,
            );
        }
    }

    /// The surface of the roads goes to the layer of decals of the terrain (WO-D7,
    /// `objects::decals`), with the strips of the coastal roads of the terrain build.
    fn apply_route_decals(&mut self) {
        let coastal_n = self.world.terrain.products().coastal_roads.len();
        if coastal_n != self.coastal_roads_seen {
            self.coastal_roads_seen = coastal_n;
            self.world.landmark.redo_route_decals();
        }
        let Some((links, touched)) = self.world.landmark.route_decals() else {
            return;
        };
        let Some(width) = self.snapshot.as_ref().map(|s| s.width() as i32) else {
            return;
        };
        if self.world.terrain.decals().types().is_empty() {
            return;
        }
        // The roads are the pieces of the road models laid as decals: wait for the model.
        if !links.is_empty() && self.world.landmark.route_skins().is_empty() {
            return;
        }
        let landmark = &self.world.landmark;
        let skins = landmark.route_skins();
        let has_route = |x: i32, y: i32| {
            landmark
                .get(&plot_key(x, y))
                .is_some_and(|p| p.route.is_some())
        };
        let coastal = match self.world.terrain.frame() {
            Some(frame) => {
                let roads: Vec<Vec<glam::Vec2>> = self
                    .world
                    .terrain
                    .products()
                    .coastal_roads
                    .iter()
                    .map(|r| {
                        r.iter()
                            .map(|&p| crate::world::objects::decals::from_layout(p, frame))
                            .collect()
                    })
                    .collect();
                crate::world::objects::decals::coastal_decals(&roads, width, skins, &has_route)
            }
            None => Vec::new(),
        };
        let skins = skins.clone();
        let cities: std::collections::BTreeSet<(i32, i32)> = links
            .iter()
            .map(|(p, _)| *p)
            .filter(|&(x, y)| landmark.route_city(x, y))
            .collect();
        self.apply_road_cuts(&links, width, &cities);
        crate::world::objects::decals::apply(
            self.world.terrain.decals_mut(),
            &links,
            &coastal,
            &touched,
            width,
            &skins,
            &|x, y| cities.contains(&(x, y)),
        );
        self.world.landmark.route_decals_applied(&links);
    }

    fn rebuild_icons(&mut self) {
        self.decor.icons = None;
        let (Some(terrain), ArtState::Ready(table)) = (&self.snapshot, &self.entities.art) else {
            return;
        };
        if self.decor.icon_material.is_none() {
            return;
        }
        let layers = &self.decor.icon_layers;
        let resolve = |tag: &str| layers.get(&table.sv(tag)?.icon).copied();
        // With the models of the landmarks the improvements, camps and ruins are models; only the
        // resources keep an icon (WO-D11, `landmark::icons`).
        let mesh = if self.world.landmark.b2_active() {
            let resources =
                crate::world::landmark::icons::resource_only(self.world.landmark.plots());
            icons::build_plots(&resources, terrain, &resolve)
        } else {
            icons::build_plots(self.world.landmark.plots(), terrain, &resolve)
        };
        let placed = self.world.landmark.changed_plots();
        self.world
            .landmark
            .set_assembled(placed, mesh.indices.len() / 6);
        if mesh.indices.is_empty() {
            return;
        }
        self.decor.icons = Some(GpuDecals {
            vertices: self.device.buf_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("icon vertices"),
                    contents: bytemuck::cast_slice(&mesh.vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                },
                Cluster::Decals,
            ),
            indices: self.device.buf_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("icon indices"),
                    contents: bytemuck::cast_slice(&mesh.indices),
                    usage: wgpu::BufferUsages::INDEX,
                },
                Cluster::Decals,
            ),
            index_count: mesh.indices.len() as u32,
        });
    }

    /// The roads over the map (highlights and the path are `Overlay`'s and `MovementPath`'s).
    fn rebuild_lines(&mut self) {
        self.decor.lines = None;
        let Some(terrain) = &self.snapshot else {
            return;
        };
        let mut mesh = LineMesh::default();
        {
            let routes = self
                .world
                .landmark
                .plots()
                .filter_map(|p| Some(((p.x, p.y), p.route?)))
                .collect();
            let cities = self.world.city.cities().map(|c| (c.x, c.y)).collect();
            roads::build(terrain, &routes, &cities, &mut mesh);
        }
        if mesh.is_empty() {
            return;
        }
        self.decor.lines = Some(GpuLines {
            vertices: self.device.buf_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("line vertices"),
                    contents: bytemuck::cast_slice(&mesh.vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                },
                Cluster::Other,
            ),
            indices: self.device.buf_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("line indices"),
                    contents: bytemuck::cast_slice(&mesh.indices),
                    usage: wgpu::BufferUsages::INDEX,
                },
                Cluster::Other,
            ),
            index_count: mesh.indices.len() as u32,
        });
    }

    /// The native trees, made again from the cells the forest holds: through the stencil of the
    /// terrain (the forest bits `world-water` writes), in the original's basis.
    ///
    /// Incremental: the cells marked (changed ground, stencil, cuts, placing) are made again nearest
    /// the view first, and the blocks of the map they put trees in are uploaded again, within
    /// [`FOREST_BUDGET_MS`] a frame (at least one cell and one block when there is work). A block
    /// keeps its old instances until its new ones are ready.
    fn pump_forest(&mut self, to_orig: Mat4, fow_view: &crate::world::fow::mask::FowView) {
        use crate::world::forest::blocks::BlockId;
        use crate::world::forest::scatter::TreeKind;
        let Some(features) = &self.features else {
            return;
        };
        let started = (self.cpu_clock)();
        let (lo, hi) = crate::world::fow::mask::MaskView::of(fow_view).ours_rect(&to_orig);
        let centre = (lo + hi) * 0.5;
        // The model, overlay and variants of each draw (kind and style), from the art.
        let mut table: HashMap<
            (TreeKind, usize),
            (
                asset_format::LogicalId,
                Option<asset_format::LogicalId>,
                u32,
            ),
        > = HashMap::new();
        if let Some(gpu) = self.world.forest.gpu() {
            for kind in TreeKind::ALL {
                for style in 0..16 {
                    let Some(entry) = features.art.entry(kind.feature(), style) else {
                        continue;
                    };
                    let Some(model) = &entry.tree_model else {
                        continue;
                    };
                    let variants = gpu.variant_count(model);
                    if variants > 0 {
                        table.insert(
                            (kind, style),
                            (model.clone(), entry.decal.clone(), variants),
                        );
                    }
                }
            }
        }
        // TODO(RW-16): the capture's answer says "TerrainStencil channel 7"; the forest layout
        // stage of `world-water` writes bits 0 and 3 (0x09). The text of the spec (WOB 4.6) is
        // followed: the trees keep out of the layout bits.
        let mask = crate::world::water::coast::stencil::FOREST_LAYOUT_BITS;
        let ground = self.world.terrain.sampler();
        let stencil = self.world.terrain.stencil();
        let stencil_gate = match (stencil, self.world.terrain.frame()) {
            (Some(stencil), Some(frame)) => Some(crate::world::forest::batches::StencilGate {
                stencil,
                frame,
                mask,
            }),
            _ => None,
        };
        let inner: &dyn crate::world::forest::batches::ForestGate = match &stencil_gate {
            Some(g) => g,
            None => &crate::world::forest::batches::OpenGate,
        };
        // The `Tree Cut` meshes of the landmarks close the ground they cover too.
        let cuts = &self.forest_cuts;
        let gate = crate::world::forest::batches::CutGate { inner, cuts };
        let variants = |c: &crate::world::forest::batches::TreeCell| {
            table.get(&(c.kind, c.style)).map_or(0, |t| t.2)
        };
        let shadow_fn = ground.map(|sampler| {
            move |instances: &[StaticInstance]| {
                crate::world::forest::batches::tree_shadow_quads(instances, sampler, stencil, cuts)
            }
        });
        let cx = crate::world::forest::CellCx {
            to_orig: &to_orig,
            gate: &gate,
            ground,
            variants: &variants,
            shadow: shadow_fn
                .as_ref()
                .map(|f| f as &crate::world::forest::ShadowFn<'_>),
        };
        // The cells, the nearest to the view first. The shadows of the trees are decals of the
        // `Tree_Shadow` category in the macro shadow of the pages
        // (`ForestVisSystem::CreateTreeShadows`), made again for the cells that changed.
        let cell_rank = |c: u32| {
            let w = self.snapshot.as_ref().map_or(1, |s| s.width().max(1));
            crate::hexgrid::cell_center((c % w) as i32, (c / w) as i32).distance(centre)
        };
        let mut shadow_changes = Vec::new();
        let mut cells_done = 0usize;
        let cells_budget = FOREST_BUDGET_MS * 0.6;
        for index in self.world.forest.pending_by(&cell_rank) {
            if let Some(change) = self.world.forest.update_cell(index, &cx) {
                shadow_changes.push(change);
            }
            cells_done += 1;
            if (self.cpu_clock)() - started >= cells_budget {
                break;
            }
        }
        if !shadow_changes.is_empty()
            && let Some(category) = crate::world::decal::DecalCategory::of_name("Tree_Shadow")
        {
            let cells: Vec<u32> = shadow_changes.iter().map(|(i, _)| *i).collect();
            let decals = shadow_changes
                .into_iter()
                .filter(|(_, q)| !q.is_empty())
                .map(|(cell, quads)| crate::world::terrain::decals::ObjectDecal {
                    category,
                    texture: None,
                    shape: crate::world::terrain::decals::DecalShape::Quads(quads),
                    cell,
                    casts_shadow: true,
                })
                .collect();
            self.world.terrain.decals_mut().set_object_decals(
                crate::world::terrain::decals::DecalSource::Trees,
                &cells,
                decals,
            );
        }
        // The blocks: after the cells settle (a block is made once for the cells that fill it), or
        // after a few frames of waiting.
        if self.world.forest.pending_cells() > 0 {
            self.forest_starved += 1;
        }
        if self.world.forest.pending_cells() == 0 || self.forest_starved >= 8 {
            self.forest_starved = 0;
            let inv = to_orig.inverse();
            let block_rank = |id: &BlockId| {
                let (std::cmp::Reverse(row), col) = id.1;
                let side = crate::world::forest::cull::CHUNK;
                let c = Vec3::new((col as f32 + 0.5) * side, (row as f32 + 0.5) * side, 0.0);
                let p = inv.transform_point3(c);
                Vec2::new(p.x, p.z).distance(centre)
            };
            for id in self.world.forest.dirty_blocks_by(&block_rank) {
                let trees = self.world.forest.build_block(&id);
                let (group, block) = id;
                if let Some(gpu) = self.world.forest.gpu_mut() {
                    match table.get(&group) {
                        Some((model, colorize, _))
                            if !trees.is_empty()
                                && gpu.ensure_set(
                                    &self.device,
                                    group,
                                    model,
                                    colorize.as_ref(),
                                    group.1,
                                ) =>
                        {
                            gpu.set_block(&self.device, group, block, &trees);
                        }
                        _ => gpu.remove_block(group, block),
                    }
                }
                self.world.forest.block_done(&id);
                self.forest_blocks_rebuilt += 1;
                if (self.cpu_clock)() - started >= FOREST_BUDGET_MS {
                    break;
                }
            }
        }
        // What is left, in or near the view and elsewhere.
        let (half, margin) = ((hi - lo) * 0.5, crate::hexgrid::HEX_WIDTH * 4.0);
        let near = |p: Vec2, extra: f32| {
            let d = (p - centre).abs();
            d.x <= half.x + margin + extra && d.y <= half.y + margin + extra
        };
        let w = self.snapshot.as_ref().map_or(1, |s| s.width().max(1));
        let mut view_n = self
            .world
            .forest
            .pending_iter()
            .filter(|&c| {
                near(
                    crate::hexgrid::cell_center((c % w) as i32, (c / w) as i32),
                    0.0,
                )
            })
            .count() as u64;
        let mut all_n = self.world.forest.pending_cells() as u64;
        let inv = to_orig.inverse();
        let side = crate::world::forest::cull::CHUNK;
        for id in self.world.forest.dirty_ids() {
            let (std::cmp::Reverse(row), col) = id.1;
            let c = Vec3::new((col as f32 + 0.5) * side, (row as f32 + 0.5) * side, 0.0);
            let p = inv.transform_point3(c);
            all_n += 1;
            view_n += u64::from(near(Vec2::new(p.x, p.z), side));
        }
        self.forest_view_pending = view_n;
        self.forest_background = all_n - view_n;
        self.forest_rebuilds += 1;
        self.forest_ms = (self.cpu_clock)() - started;
        tracing::debug!(
            cells = cells_done,
            pending = self.world.forest.pending_cells(),
            blocks_dirty = self.world.forest.dirty_blocks(),
            ms = self.forest_ms,
            "native trees stepped"
        );
    }

    /// The cell at `(x, y)` of the snapshot's map (wrapped), if it is on the map.
    fn forest_cell_index(&self, x: i32, y: i32) -> Option<u32> {
        let s = self.snapshot.as_ref()?;
        let (w, h) = (s.width() as i32, s.height() as i32);
        if y < 0 || y >= h || w <= 0 {
            return None;
        }
        let x = if s.wrap_x() {
            x.rem_euclid(w)
        } else if (0..w).contains(&x) {
            x
        } else {
            return None;
        };
        Some((y * w + x) as u32)
    }

    /// Marks the cells whose trees can stand in the world rectangle `lo..hi` (`(X, Z)`), a margin
    /// round it for the reach of a cell's trees.
    fn forest_mark_world_rect(&mut self, lo: Vec2, hi: Vec2) {
        let m = 70.0;
        let x0 = ((lo.x - m) / crate::hexgrid::HEX_WIDTH).floor() as i32 - 1;
        let x1 = ((hi.x + m) / crate::hexgrid::HEX_WIDTH).ceil() as i32;
        let y0 = (-(hi.y + m) / crate::hexgrid::ROW_STEP).floor() as i32;
        let y1 = (-(lo.y - m) / crate::hexgrid::ROW_STEP).ceil() as i32;
        if i64::from(x1 - x0 + 1) * i64::from(y1 - y0 + 1) > 8192 {
            self.world.forest.mark_every_cell();
            return;
        }
        let mut cells = Vec::new();
        for y in y0..=y1 {
            for x in x0..=x1 {
                cells.extend(self.forest_cell_index(x, y));
            }
        }
        self.world.forest.mark_cells(cells);
    }

    /// Every cut the trees are gated by: the ground of the `Tree Cut` meshes of the landmarks, the
    /// ribbons of the roads and the squares under the houses of the cities.
    fn cuts_whole(&self) -> crate::world::forest::batches::TreeCuts {
        let mut cuts = self.world.landmark.tree_cuts();
        for t in self.road_cuts.values().flatten() {
            cuts.add(*t);
        }
        for t in self.world.city.tree_cuts() {
            cuts.add(t);
        }
        cuts
    }

    /// The ribbons of the roads clear the trees under them (the stencil bit 0x02 of
    /// `UpdateRoadGeometry`, `objects::decals::road_cuts`): the plots whose ribbons changed go to the queue
    /// of the single changes of the cuts, a plot that has no road now gives its old ground back.
    fn apply_road_cuts(
        &mut self,
        links: &[crate::world::landmark::routes::DrawnLink],
        width: i32,
        cities: &std::collections::BTreeSet<(i32, i32)>,
    ) {
        let new = crate::world::objects::decals::road_cuts(links, width, &|x, y| {
            cities.contains(&(x, y))
        });
        let old = std::mem::replace(&mut self.road_cuts, new);
        let cells: std::collections::BTreeSet<u32> =
            old.keys().chain(self.road_cuts.keys()).copied().collect();
        for cell in cells {
            let (was, is) = (old.get(&cell), self.road_cuts.get(&cell));
            if was != is {
                self.forest_cut_queue.push_back((
                    was.cloned().unwrap_or_default(),
                    is.cloned().unwrap_or_default(),
                ));
            }
        }
    }

    /// Looks at what changed under the native trees since the last frame and marks the cells to make
    /// again: placing, the landmarks' cuts, the heights and the stencil of cells that built.
    fn forest_invalidate(&mut self, ours_to_orig: Mat4) {
        let cells = self.world.terrain.cells_done();
        let height_rev = self.world.terrain.height_revision();
        if self.forest_dirty || self.forest_orig != Some(ours_to_orig) {
            // New art or masks, or another basis: everything is made again.
            self.world.forest.mark_all();
            self.forest_orig = Some(ours_to_orig);
            self.forest_dirty = false;
            self.forest_cuts = self.cuts_whole();
            self.forest_cuts_dirty = false;
            self.forest_cut_queue.clear();
            self.forest_height_rev = height_rev;
            self.forest_cells_waiting.clear();
            self.forest_gate_cells = cells;
            self.forest_rebuilt = self.frame_no;
        }
        let touched = self.world.forest.take_touched();
        self.world.forest.mark_cells(touched);
        // The cut index is rebuilt whole at most every few frames (it costs tens of ms on a full
        // map); it is rebuilt whole only when everything changed (new art, the models).
        if self.forest_cuts_dirty && self.frame_no - self.forest_cuts_at >= FOREST_CUTS_FRAMES {
            let _s = crate::perf::slow::scope("forest.cuts");
            self.forest_cuts_dirty = false;
            self.forest_cuts_at = self.frame_no;
            self.forest_cut_queue.clear();
            let new = self.cuts_whole();
            let boxes = self.forest_cuts.changed_boxes(&new);
            self.forest_cuts = new;
            for (lo, hi) in boxes {
                self.forest_mark_world_rect(lo, hi);
            }
        } else if !self.forest_cuts_dirty && !self.forest_cut_queue.is_empty() {
            // The placings that changed cuts: each takes its triangles out of the index and puts its
            // new ones in, a few at a time; the trees on the ground they cover are made again.
            let _s = crate::perf::slow::scope("forest.cut_changes");
            let n = self
                .forest_cut_queue
                .len()
                .min(FOREST_CUT_CHANGES_PER_FRAME);
            let mut boxes = Vec::new();
            for (out, put) in self.forest_cut_queue.drain(..n) {
                let rect = |t: &[Vec2; 3]| (t[0].min(t[1]).min(t[2]), t[0].max(t[1]).max(t[2]));
                for t in &out {
                    if self.forest_cuts.remove(t) {
                        boxes.push(rect(t));
                    }
                }
                for t in put {
                    self.forest_cuts.add(t);
                    boxes.push(rect(&t));
                }
            }
            for (lo, hi) in boxes {
                self.forest_mark_world_rect(lo, hi);
            }
        }
        // The stencil grows and the heights change as the terrain builds; the cells that changed are
        // looked at a few times a second, and the cells next to them with them (a tree reads the
        // heights a little round it).
        if (cells != self.forest_gate_cells || height_rev != self.forest_height_rev)
            && self.frame_no - self.forest_rebuilt >= 30
        {
            let _s = crate::perf::slow::scope("forest.heights");
            match self
                .world
                .terrain
                .height_dirty_cells(self.forest_height_rev)
            {
                None => {
                    self.forest_cells_waiting.clear();
                    self.world.forest.mark_every_cell();
                }
                Some(dirty) => {
                    // The trees of a cell are placed again once its heights are final.
                    let dirty = self
                        .world
                        .terrain
                        .final_height_cells(dirty, &mut self.forest_cells_waiting);
                    let mut marks = Vec::new();
                    for (x, y) in dirty {
                        for dy in -1..=1 {
                            for dx in -1..=1 {
                                marks.extend(self.forest_cell_index(x + dx, y + dy));
                            }
                        }
                    }
                    self.world.forest.mark_cells(marks);
                }
            }
            self.forest_height_rev = height_rev;
            self.forest_gate_cells = cells;
            self.forest_rebuilt = self.frame_no;
        }
    }

    /// The `Particle` subsystem's frame (R5.12): the camera, the animation time, the ground under
    /// a cell and its fog state (2 visible, 1 fogged, 0 unseen; all visible without a fog feed).
    fn feed_particles(&mut self, camera: &dyn CameraView) {
        use crate::terrain::Fog;
        let world = &mut self.world;
        let sampler = world.terrain.sampler();
        let cell_world = |x: i32, y: i32| {
            let c = crate::hexgrid::cell_center(x, y);
            let h = sampler.map_or(0.0, |s| s.surface_height(c));
            Some(Vec3::new(c.x, h, c.y))
        };
        let fog = world.fluid_fow.fog();
        let cell_fog = |x: i32, y: i32| match fog.map(|f| f.cell(x, y)) {
            None => 2,
            Some(Some(Fog::Visible)) => 2,
            Some(Some(Fog::Hidden)) => 1,
            Some(_) => 0,
        };
        let frame = crate::world::particle::ParticleFrame {
            view: camera.view(),
            time: self.time_override.unwrap_or(self.anim_time),
            cell_world: &cell_world,
            cell_fog: &cell_fog,
        };
        // The terrain and the fog are read while the particles change: split the borrow.
        let mut particle = std::mem::take(&mut world.particle);
        particle.frame(&frame);
        world.particle = particle;
    }

    /// The particles of the map (`world-objects` 5: `particles_mut().look_at(x, y)`).
    pub fn particles_mut(&mut self) -> &mut crate::world::particle::Particle {
        &mut self.world.particle
    }

    /// The `Forest` subsystem draws the trees through `ForestShader` when the fog of war's mask, the
    /// light tables and the art are in; B1's trees stay until then. Returns whether it does.
    fn feed_forest(
        &mut self,
        camera: &dyn CameraView,
        aspect: f32,
        ours_to_orig: Mat4,
        fow_view: crate::world::fow::mask::FowView,
        shifts: &[f32],
        world_drawn: bool,
    ) -> bool {
        self.frame_no += 1;
        let fow_on = self.world.fluid_fow.clouds_active()
            && !self.layers.is_disabled(SystemId::FluidFow)
            && self.targets.get(crate::frame::FOW_MASK).is_some();
        if !(world_drawn
            && self.boot_required
            && fow_on
            && self.features.is_some()
            && self.world.forest.gpu().is_some())
        {
            // Nothing takes the single changes of the cuts while the native trees are not drawn: a
            // long queue gives way to one whole rebuild.
            if self.forest_cut_queue.len() > 4096 {
                self.forest_cut_queue.clear();
                self.forest_cuts_dirty = true;
            }
            return false;
        }
        // `FarLOD` switched: the blocks are made again with (or without) their far trees.
        let far_on = self.far_lod();
        if self
            .world
            .forest
            .gpu_mut()
            .is_some_and(|g| g.set_far_enabled(far_on))
        {
            self.world.forest.mark_all();
        }
        // The cells that changed are marked and made again a few at a time, nearest the view first.
        let _s = crate::perf::slow::scope("forest.invalidate");
        self.forest_invalidate(ours_to_orig);
        drop(_s);
        if self.world.forest.pending_cells() > 0 || self.world.forest.dirty_blocks() > 0 {
            let _slow21 = crate::perf::slow::scope("render.rebuild_forest");
            self.pump_forest(ours_to_orig, &fow_view);
            drop(_slow21);
        } else {
            self.forest_view_pending = 0;
            self.forest_background = 0;
        }
        let reflect_vp_orig = self
            .reflection_camera(camera, aspect, &ours_to_orig)
            .map(|c| c.view_proj());
        let mask = crate::world::fow::mask::MaskView::of(&fow_view);
        let world_width = self
            .world
            .terrain
            .frame()
            .map_or(1.0, |f| f.half_world().x * 2.0);
        let layer = &crate::world::fow::state::CLOUD_LAYERS[0];
        let overlay_alpha = self
            .features
            .as_ref()
            .and_then(|f| f.art.entry(crate::terrain::FeatureKind::Forest, 0))
            .map_or(0.6, |e| e.alpha);
        // `FarLOD`: the distance in the original's basis (`ours_to_orig` may scale).
        let far_lod = self.far_lod().then(|| {
            self.cam_hints
                .far_lod_distance
                .unwrap_or(crate::world::far_lod::FAR_LOD_DISTANCE)
                * ours_to_orig.transform_vector3(glam::Vec3::X).length()
        });
        let scene = self.scene_size();
        let (Some(fow), Some(gpu)) = (
            self.targets.get(crate::frame::FOW_MASK),
            self.world.forest.gpu_mut(),
        ) else {
            return false;
        };
        let view = |id| self.targets.get(id).map(|t| &t.view);
        let frame = crate::world::forest::draw::ForestFrame {
            globals: &self.frame_globals,
            lighting: &self.lighting,
            fow_mask: &fow.view,
            fow_base: self
                .world
                .fluid_fow
                .base_view()
                .or_else(|| view(crate::frame::FOW_BASE)),
            screen_overlay: view(crate::frame::SCREEN_OVERLAY),
            overlay_map: view(crate::frame::OVERLAY_MAP),
            view_proj_orig: camera.view_proj(aspect) * ours_to_orig.inverse(),
            overlay_projection: mask.projection(),
            fow_base_scale: crate::world::fow::clouds::base_scale(
                world_width,
                self.anim_time,
                layer,
            ),
            eye_orig: ours_to_orig.transform_point3(camera.eye()),
            screen: (scene.0 as f32, scene.1 as f32),
            shifts,
            overlay_alpha,
            reflect_vp_orig,
            far_lod,
            // A low eye (a mod's first-person camera): the trees smaller, none around the eye. The
            // clearance in the original's basis, as the distance of `FarLOD`.
            near: self
                .cam_hints
                .objects
                .filter(|o| !o.is_identity())
                .map(|o| crate::world::forest::draw::TreesNearEye {
                    scale: o.scale,
                    clearance: o.clearance * ours_to_orig.transform_vector3(glam::Vec3::X).length(),
                }),
        };
        let _s = crate::perf::slow::scope("forest.gpu_frame");
        gpu.frame(&self.device, &self.queue, &frame)
    }

    /// Takes the fields of the city layout system (WOB5): the textures `tx:df_*` and `tx:tf_*` of
    /// `b2-city` by file name ([`crate::world::city::fields::file_names`]), `Rgba8` 128x128. The
    /// cities are laid out again with them; an incomplete set keeps the stand-in fields.
    pub fn set_city_layout_fields(&mut self, images: &[(String, TextureBlob)]) {
        let rgba: std::collections::HashMap<&str, &[u8]> = images
            .iter()
            .filter(|(_, b)| {
                b.format == asset_format::TextureFormat::Rgba8 && b.width == 128 && b.height == 128
            })
            .filter_map(|(n, b)| Some((n.as_str(), b.mips.first()?.as_slice())))
            .collect();
        match crate::world::city::fields::LayoutFields::from_images(&|n| {
            rgba.get(n).map(|b| b.to_vec())
        }) {
            Some(fields) => self.world.city.set_layout_fields(fields),
            None => tracing::warn!("the city layout fields are incomplete; the stand-in is used"),
        }
    }

    /// Takes the placement masks of the trees (`forest_tile1..N`, `R8` 128x128; WOB3): the lists of
    /// trees are scattered with them (list `i` takes mask `i % N`), and the trees of the map are
    /// placed again. A texture of any other shape is left out with a warning; none left means the
    /// hex outline (SIMP-009).
    pub fn set_forest_masks(&mut self, masks: &[TextureBlob]) {
        self.forest_masks = masks
            .iter()
            .filter_map(|b| {
                let m = ForestMask::from_blob(b);
                if m.is_none() {
                    tracing::warn!("a forest mask is not an R8 texture of 128x128, left out");
                }
                m
            })
            .collect();
        if let Some(features) = self.features.as_mut() {
            features.forest_tiles =
                ForestTiles::with_masks(features.art.forest_params(), &self.forest_masks);
            self.world.forest.invalidate_all();
            self.rebuild_features();
        }
        self.forest_dirty = true;
    }

    /// Takes the art of the features: the tree models become static models, the
    /// decal textures one array. `models` and `textures` hold what `art` names; whatever is
    /// missing leaves that feature without trees or decal. Draws the features of the current map.
    pub fn set_feature_art(
        &mut self,
        art: FeatureArtSet,
        models: &HashMap<LogicalId, ModelBlob>,
        textures: &HashMap<LogicalId, TextureBlob>,
    ) {
        if let Some(forest) = self.world.forest.gpu_mut() {
            // The native draw keeps its own copies: the diffuse of the trees and the overlays.
            for (id, t) in textures {
                forest.add_texture(&self.device, &self.queue, id, t);
            }
            for id in art.tree_models() {
                if let Some(blob) = models.get(&id) {
                    forest.add_model(&self.device, &id, blob);
                }
            }
        }
        self.forest_dirty = true;
        let mut tree_models = HashMap::new();
        for id in art.tree_models() {
            let Some(blob) = models.get(&id) else {
                tracing::warn!(id = id.as_str(), "tree model not loaded, no trees");
                continue;
            };
            let model = self.statics.create_model(
                &self.device,
                &self.queue,
                blob,
                textures,
                Variants::PerMesh,
                true,
            );
            tracing::info!(
                id = id.as_str(),
                variants = self.statics.variant_count(model),
                "tree model in use"
            );
            tree_models.insert(id, model);
        }
        let ids = art.decal_textures();
        let decoded = crate::decal::uniform_decal_blobs(&self.device, &ids, textures);
        let decal_blobs = decoded.as_ref().unwrap_or(textures);
        let plan = LayerPlan::build(&TerrainArtSlots::from_textures(&ids), decal_blobs);
        for (id, why) in &plan.rejected {
            tracing::warn!(id = id.as_str(), "decal texture left out: {why}");
        }
        let decal_material =
            match self
                .decal_pipeline
                .material(&self.device, &self.queue, &plan, decal_blobs)
            {
                Ok(m) => Some(m),
                Err(e) => {
                    if !ids.is_empty() {
                        tracing::warn!("decal textures unavailable ({e}); no feature decals");
                    }
                    None
                }
            };
        let decals = ids
            .iter()
            .zip(&plan.slots)
            .filter_map(|(id, slot)| {
                Some((
                    id.clone(),
                    DecalRef {
                        layer: slot.layer?,
                        uv_scale: slot.scale,
                        alpha: 1.0,
                    },
                ))
            })
            .collect();
        tracing::info!(
            tree_models = tree_models.len(),
            decal_layers = plan.layers.len(),
            "feature art in use"
        );
        // New lists: every tree is placed again.
        self.world.forest.invalidate_all();
        self.features = Some(FeatureGpu {
            forest_tiles: ForestTiles::with_masks(art.forest_params(), &self.forest_masks),
            art,
            tree_models,
            decal_material,
            decals,
        });
        self.rebuild_features();
    }

    /// Trees and decals of the current snapshot with the current feature art.
    fn rebuild_features(&mut self) {
        let (Some(snapshot), Some(features)) = (&self.snapshot, &self.features) else {
            self.statics.replace_set(&self.device, BatchSet::Trees, &[]);
            self.decals = None;
            return;
        };
        // The forest keeps the trees of each cell: only the cells that changed are placed again
        // (a change of fog or owner moves no tree), and the lists of a new art set start over.
        let forest = &mut self.world.forest;
        forest.sync_with(snapshot);
        forest.refresh(&features.forest_tiles);
        // One batch per model: styles that share a model (Generic and Europe jungle) are merged.
        let mut per_model: HashMap<StaticModelId, Vec<StaticInstance>> = HashMap::new();
        for batch in forest.batches() {
            let model = features
                .art
                .entry(batch.kind.feature(), batch.style)
                .and_then(|e| e.tree_model.as_ref())
                .and_then(|id| features.tree_models.get(id));
            let Some(&model) = model else { continue };
            let variants = self.statics.variant_count(model).max(1);
            let out = per_model.entry(model).or_default();
            out.extend(batch.instances.into_iter().map(|mut i| {
                i.variant %= variants;
                i
            }));
        }
        let mut sets: Vec<(StaticModelId, Vec<StaticInstance>)> = per_model.into_iter().collect();
        sets.sort_by_key(|(m, _)| *m);
        tracing::info!(
            tree_instances = sets.iter().map(|(_, v)| v.len()).sum::<usize>(),
            tree_batches = sets.len(),
            "trees rebuilt"
        );
        let sets: Vec<(StaticModelId, &[StaticInstance])> =
            sets.iter().map(|(m, v)| (*m, v.as_slice())).collect();
        self.statics
            .replace_set(&self.device, BatchSet::Trees, &sets);

        self.decals = None;
        if features.decal_material.is_none() {
            return;
        }
        let mesh = decal_mesh(snapshot, &|kind, style| {
            let entry = features.art.entry(kind, style)?;
            let decal = features.decals.get(entry.decal.as_ref()?)?;
            Some(DecalRef {
                alpha: entry.alpha,
                ..*decal
            })
        });
        if mesh.indices.is_empty() {
            return;
        }
        self.decals = Some(GpuDecals {
            vertices: self.device.buf_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("decal vertices"),
                    contents: bytemuck::cast_slice(&mesh.vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                },
                Cluster::Decals,
            ),
            indices: self.device.buf_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("decal indices"),
                    contents: bytemuck::cast_slice(&mesh.indices),
                    usage: wgpu::BufferUsages::INDEX,
                },
                Cluster::Decals,
            ),
            index_count: mesh.indices.len() as u32,
        });
    }

    /// Builds the texture array of `plan` from `blobs` and uses it for the terrain. A plan without
    /// layers (or an array the device cannot make) leaves the flat colours. Returns the number of
    /// array layers in use.
    pub fn set_terrain_textures(
        &mut self,
        plan: &LayerPlan,
        blobs: &HashMap<LogicalId, TextureBlob>,
    ) -> usize {
        let (material, error) = self.terrain_pipeline.textured_material(
            &self.device,
            &self.queue,
            plan,
            blobs,
            &self
                .world
                .services
                .cutout
                .refs()
                .expect("made with the renderer"),
        );
        if let Some(e) = error
            && !plan.layers.is_empty()
        {
            tracing::warn!("terrain textures unavailable ({e}); using flat colours");
        }
        for (id, why) in &plan.rejected {
            tracing::warn!(id = id.as_str(), "terrain texture left out: {why}");
        }
        self.terrain_material = material;
        self.cutout_revision = self.world.services.cutout.revision();
        self.terrain_material.layers
    }

    /// Takes the three cloud textures of the fog (`terrain::fog::CLOUD_LAYERS`) from `textures`.
    /// Without them (or on a device that cannot sample them) the unrevealed cells stay the flat
    /// fog colour. Returns whether the clouds will be drawn.
    pub fn set_fog_clouds(&mut self, textures: &HashMap<LogicalId, TextureBlob>) -> bool {
        match self.world.fluid_fow.set_cloud_textures(textures) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("fog clouds unavailable ({e}); the unrevealed cells stay flat");
                false
            }
        }
    }

    /// Takes the bump map of the water surface (`terrain::water::WATER_BUMP_TEXTURE`) from
    /// `textures`. Without it (or on a device that cannot sample it) the water is still drawn,
    /// without ripples. Returns whether the ripples will be drawn.
    pub fn set_water_texture(&mut self, textures: &HashMap<LogicalId, TextureBlob>) -> bool {
        let Some(blob) = textures.get(&LogicalId::new(crate::terrain::water::WATER_BUMP_TEXTURE))
        else {
            return false;
        };
        match self.water.set_bump(&self.device, &self.queue, blob) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("water bump map unavailable ({e}); the water stays flat");
                false
            }
        }
    }

    /// Seconds since the start, for the scrolling of the clouds and the waves, and the clock of the
    /// animation service (the clips of the figures, the landmarks and the cities): without it the
    /// service stood at 0 and every model held the first frame of its clip.
    pub fn set_animation_time(&mut self, seconds: f32) {
        self.anim_time = seconds;
        self.world.fluid_fow.set_time(seconds);
        self.world.services.anim.set_time(seconds);
    }

    /// Layers of the terrain texture array in use; 0 while the terrain is flat colours.
    pub fn terrain_texture_layers(&self) -> usize {
        self.terrain_material.layers
    }

    /// What the overlay subsystems need of a new map: the relief the lines lie on, and, as the B1
    /// adapter, the fog and the owners of the snapshot (the feed's own `set_fog` and `set_owners`
    /// give the same and do nothing again). The fog-state texture goes to whoever draws over the map.
    fn set_ground_overlays(&mut self, snapshot: &TerrainSnapshot) {
        let ground = Rc::new(snapshot.clone());
        self.world.minimap.set_ground(ground.clone());
        self.world.overlay.set_ground(ground.clone());
        self.world.movement_path.set_ground(ground);
        self.world.trade.set_wrap_width(if snapshot.wrap_x() {
            snapshot.shape().world_width()
        } else {
            0.0
        });
        self.feed_fog(FogInput::from_snapshot(snapshot));
        self.world
            .overlay
            .set_owners(OwnersInput::from_snapshot(snapshot));
        self.share_fog_state();
        self.map_width = snapshot.width();
    }

    /// Back to the test grid.
    pub fn clear_terrain(&mut self) {
        self.terrain = None;
        self.world.minimap.clear();
        self.world.overlay.clear();
        self.world.trade.set_wrap_width(0.0);
        self.world.movement_path.clear();
        self.world.fluid_fow.clear();
        self.world.fow_classic.set_fog(None);
        self.share_fog_state();
        self.snapshot = None;
        self.snapshot_gen += 1;
        self.rebuild_features();
        self.rebuild_decor();
    }

    /// The game was left: the map, its terrain, fog, units, cities and landmarks go; the next game
    /// sets them up again from nothing.
    pub fn clear_world(&mut self) {
        self.clear_terrain();
        self.ground_share = GroundShare::default();
        self.raw_fog = None;
        self.world.terrain.unload();
        self.world.unit.clear();
        self.world.city.clear();
        self.world.landmark.clear();
        // What the last game left on the map besides the terrain and the entities: the animation handles of its
        // units, the route lines and arrows, the effects and beams.
        self.world
            .unit
            .release_handles(&mut self.world.services.anim);
        self.world.arrows.set_arrows(Vec::new());
        self.world.trade.set_routes(0, Vec::new());
        self.world.trade.set_popup(None);
        self.world.particle.clear_effects();
        self.rebuild_entities();
    }

    /// Shows or hides the hex grid (G key).
    pub fn set_grid_visible(&mut self, visible: bool) {
        self.world.overlay.set_grid_visible(visible);
    }

    pub fn grid_visible(&self) -> bool {
        self.world.overlay.grid_visible()
    }

    /// Texture variants this device can sample, to pass to `AssetLoader::request`.
    pub fn platform_profile(&self) -> PlatformProfile {
        texture::profile_for_features(self.device.features())
    }

    /// Top-center of the ground tile nearest the scene center, where units stand.
    pub fn scene_center(&self) -> Vec3 {
        scene::center_tile_top(GRID_COLS, GRID_ROWS, GRID_SEED)
    }

    /// Uploads a model and the textures its materials reference (missing ones render white).
    pub fn create_model(
        &self,
        blob: &ModelBlob,
        textures: &HashMap<LogicalId, TextureBlob>,
    ) -> Rc<GpuModel> {
        self.models
            .create_model(&self.device, &self.queue, blob, textures)
    }

    /// The instance starts in the identity pose; see [`Self::set_pose`].
    pub fn add_model_instance(&mut self, model: Rc<GpuModel>, transform: Mat4) -> InstanceId {
        self.models.add_instance(&self.device, model, transform)
    }

    pub fn set_instance_transform(&self, id: InstanceId, transform: Mat4) {
        self.models.set_transform(&self.queue, id, transform);
    }

    /// Skin matrices (`world * inverse_bind`) per bone; at most 255 are used. They go to the bone
    /// texture with the next frame, within its upload limit.
    pub fn set_pose(&self, id: InstanceId, skin: &[Mat4]) {
        self.models.set_pose(id, skin);
    }

    /// Rows of the bone texture in use and bytes the last frame uploaded to it (`?stats=1`).
    pub fn bone_stats(&self) -> (u32, usize) {
        self.models.bone_stats()
    }

    pub fn remove_model_instance(&mut self, id: InstanceId) {
        self.models.remove_instance(id);
    }

    pub fn instance_skeleton(&self, id: InstanceId) -> Option<&Skeleton> {
        self.models.skeleton(id)
    }

    /// Size of the surface, which the UI draws at.
    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Size of the 3D scene targets: the surface times the render scale.
    pub fn scene_size(&self) -> (u32, u32) {
        crate::render_scale::scene_size(self.size(), self.render_scale)
    }

    /// The render scale in force (0.5 to 1).
    pub fn render_scale(&self) -> f32 {
        self.render_scale
    }

    /// The adapter, features and limits of the device, and the texture variants in use: the device
    /// part of the performance report.
    pub fn device_report(&self) -> DeviceReport {
        let ai = self.device.adapter_info();
        let f = self.device.features();
        let l = self.device.limits();
        let known = [
            (
                wgpu::Features::TEXTURE_COMPRESSION_BC,
                "TEXTURE_COMPRESSION_BC",
            ),
            (
                wgpu::Features::TEXTURE_COMPRESSION_ETC2,
                "TEXTURE_COMPRESSION_ETC2",
            ),
            (
                wgpu::Features::TEXTURE_COMPRESSION_ASTC,
                "TEXTURE_COMPRESSION_ASTC",
            ),
            (wgpu::Features::TIMESTAMP_QUERY, "TIMESTAMP_QUERY"),
        ];
        DeviceReport {
            adapter: ai.name.clone(),
            vendor: ai.vendor,
            device_id: ai.device,
            device_type: format!("{:?}", ai.device_type),
            backend: format!("{:?}", ai.backend),
            driver: ai.driver.clone(),
            driver_info: ai.driver_info.clone(),
            features: known
                .into_iter()
                .filter(|(flag, _)| f.contains(*flag))
                .map(|(_, name)| name)
                .collect(),
            features_all: format!("{f:?}"),
            limits: vec![
                (
                    "max_texture_dimension_2d",
                    u64::from(l.max_texture_dimension_2d),
                ),
                (
                    "max_texture_array_layers",
                    u64::from(l.max_texture_array_layers),
                ),
                ("max_bind_groups", u64::from(l.max_bind_groups)),
                (
                    "max_storage_buffers_per_shader_stage",
                    u64::from(l.max_storage_buffers_per_shader_stage),
                ),
                (
                    "max_uniform_buffer_binding_size",
                    l.max_uniform_buffer_binding_size,
                ),
                ("max_buffer_size", l.max_buffer_size),
                ("max_color_attachments", u64::from(l.max_color_attachments)),
            ],
            texture_variants: self
                .platform_profile()
                .texture_formats
                .iter()
                .map(|v| format!("{v:?}"))
                .collect(),
            msaa_samples: self.samples,
        }
    }

    /// The draw groups `Main` skips (bit = `perf::Group::bit`), as set by [`Self::set_disabled_groups`].
    pub fn disabled_groups(&self) -> u32 {
        self.disabled_groups
    }

    /// Debug: `Main` records no more of the groups in `mask` from the next frame on (`perf::Group`;
    /// 0 draws all). It is not saved and touches neither picking nor the class mask, so the panel's
    /// checkboxes show what a group costs.
    pub fn set_disabled_groups(&mut self, mask: u32) {
        self.disabled_groups = crate::perf::groups::sanitize(mask);
    }

    /// The size of the shadow map in force.
    pub fn shadow_quality(&self) -> crate::shadow::ShadowQuality {
        self.shadow_quality
    }

    /// Picks the size of the shadow map (or none) from the next frame on, whatever `ShadowLevel` is
    /// (`Low` is a 512 map even where the world's detail says level 0); the maps are made again at
    /// the new size, nothing else.
    pub fn set_shadow_quality(&mut self, quality: crate::shadow::ShadowQuality) {
        self.shadow_quality_chosen = true;
        if quality != self.shadow_quality {
            tracing::info!(from = ?self.shadow_quality, to = ?quality, "renderer: shadow quality");
            self.shadow_quality = quality;
        }
    }

    /// Samples of the main pass in force (1 or 4); a new `MSAASamples` waits for the next start.
    pub fn msaa_in_force(&self) -> u32 {
        self.samples
    }

    /// Tells the renderer which preset the graphics choices are (`GfxPreset::index_of`, 0 custom),
    /// for the performance report and the panel; it draws nothing differently.
    pub fn set_gfx_preset(&mut self, index: u32) {
        self.gfx_preset = index;
    }

    /// Draws the scene at `scale` times the size of the surface from the next frame on (snapped to
    /// the steps of `render_scale`); no restart. Only the targets that follow the screen are made
    /// again.
    pub fn set_render_scale(&mut self, scale: f32) {
        let scale = crate::render_scale::snap(scale);
        if scale == self.render_scale {
            return;
        }
        tracing::info!(
            from = self.render_scale,
            to = scale,
            "renderer: render scale"
        );
        self.render_scale = scale;
        self.targets.set_screen(self.scene_size());
    }

    /// The terrain detail in force.
    pub fn terrain_detail(&self) -> crate::terrain_detail::TerrainDetail {
        self.world.terrain.detail()
    }

    /// Picks how finely the ground is meshed and how big its pages may be, from the next frame on. The
    /// render scale has no part in it. A new level makes the lattices and the pages again.
    pub fn set_terrain_detail(&mut self, detail: crate::terrain_detail::TerrainDetail) {
        if detail != self.terrain_detail() {
            tracing::info!(from = ?self.terrain_detail(), to = ?detail, "renderer: terrain detail");
            self.world.terrain.set_detail(detail);
        }
    }

    /// Replaces constants of the uniform blocks by name, as the CTAB calls them, when the blocks
    /// are uploaded (`upload_uniform`): for `golden` (the time and the cloud scrolls of a capture)
    /// and for debugging. An empty map ends it.
    pub fn set_constant_overrides(&mut self, values: HashMap<String, Vec<f32>>) {
        self.constant_overrides = ConstantOverrides::new(values);
    }

    pub fn constant_overrides(&self) -> &ConstantOverrides {
        &self.constant_overrides
    }

    /// The time the shaders see instead of the animation time, for `golden`; `None` ends it.
    pub fn set_time_override(&mut self, seconds: Option<f32>) {
        self.time_override = seconds;
    }

    /// The time the shaders get: the override if there is one.
    pub fn shader_time(&self) -> f32 {
        self.time_override.unwrap_or(self.anim_time)
    }

    /// Writes `block` to `buffer` at `offset` with the constant overrides applied: the one way a
    /// block of `uniform_block!` reaches the GPU.
    pub fn upload_uniform<T: UniformBlock>(&self, buffer: &wgpu::Buffer, offset: u64, block: &T) {
        let mut block = *block;
        self.constant_overrides.apply(&mut block);
        self.queue
            .write_buffer(buffer, offset, bytemuck::bytes_of(&block));
    }

    /// The minimap texture and its size and generation, for the UI panel; `None` until the target
    /// exists (the first frame) and while `Minimap` is switched off. `Minimap` draws it every
    /// frame: the explored map and the rectangle of the camera.
    pub fn minimap_view(&self) -> Option<(&wgpu::TextureView, MinimapInfo)> {
        minimap_of(&self.layers, &self.targets)
    }

    /// The registry of the original UI's textures: the client takes the ids the draw list asked
    /// for (`take_wanted`), loads the bytes from `ui-assets` and uploads them.
    pub fn ui_textures_mut(&mut self) -> &mut UiTextures {
        &mut self.ui_textures
    }

    /// The point of our world (X, Z) under the minimap position (`u`, `v`), in 1/65535 of the
    /// width and height from the top left, for the camera focus (the click of the panel). `None`
    /// while the minimap has no map, or is switched off.
    pub fn minimap_to_world(&self, u: u16, v: u16) -> Option<glam::Vec2> {
        if self.layers.is_disabled(crate::frame::SystemId::Minimap) {
            return None;
        }
        self.world.minimap.to_world(u, v)
    }

    /// Takes `hexgrid.dds` (`world::overlay::grid::HEX_GRID_TEXTURE`) from `textures`. Without it
    /// the ring of every hex is the line. Returns whether it was taken.
    pub fn set_hex_grid_texture(&mut self, textures: &HashMap<LogicalId, TextureBlob>) -> bool {
        textures
            .get(&LogicalId::new(
                crate::world::overlay::grid::HEX_GRID_TEXTURE,
            ))
            .is_some_and(|blob| self.world.overlay.set_hex_grid_texture(blob))
    }

    /// Takes the textures the highlight styles name (`HighlightLibrary::texture_ids`) from
    /// `textures`. A missing one is a flat colour. Returns how many were taken.
    pub fn set_highlight_textures(&mut self, textures: &HashMap<LogicalId, TextureBlob>) -> usize {
        let ids = self.world.overlay.highlight_library().texture_ids();
        ids.iter()
            .filter(|id| {
                textures
                    .get(*id)
                    .is_some_and(|b| self.world.overlay.set_highlight_texture(id.as_str(), b))
            })
            .count()
    }

    /// True once the styles of `Highlights.xml` are in (`set_world_settings`).
    pub fn has_highlight_styles(&self) -> bool {
        !self.world.overlay.highlight_library().is_empty()
    }

    /// The ids of the textures the highlight styles and the selection ring name, for the loader.
    pub fn highlight_texture_ids(&self) -> Vec<LogicalId> {
        let mut ids: Vec<LogicalId> = self
            .world
            .overlay
            .highlight_library()
            .texture_ids()
            .into_iter()
            .collect();
        ids.extend(
            crate::world::overlay::ring::TEXTURES
                .iter()
                .map(|t| LogicalId::new(*t)),
        );
        ids
    }

    /// Takes the two textures of the selection ring (`world::overlay::ring::TEXTURES`) from
    /// `textures`. Without the ring and its glow the ring stays B1's outline. Returns how many.
    pub fn set_ring_textures(&mut self, textures: &HashMap<LogicalId, TextureBlob>) -> usize {
        self.world.overlay.set_ring_textures(textures)
    }

    /// What to highlight through the styles of `Highlights.xml` (the table of calls of
    /// `client-app` makes it). The same set again builds nothing.
    pub fn set_highlight_set(
        &mut self,
        set: crate::world::overlay::highlights::HighlightSet,
    ) -> bool {
        self.world.overlay.set_highlight_set(set)
    }

    /// Takes the textures of the minimap (group `minimap`, `world::minimap::TEXTURES`) from
    /// `textures`. A missing one keeps a flat colour. Returns how many were taken.
    pub fn set_minimap_textures(&mut self, textures: &HashMap<LogicalId, TextureBlob>) -> usize {
        self.world
            .minimap
            .set_textures(&self.device, &self.queue, textures)
    }

    /// The light resources of the world and what is still missing from the boot group.
    pub fn lighting(&self) -> &LightingEnv {
        &self.lighting
    }

    /// Hands over one texture of the `boot` group by its file name (`unit_irradiance.dds`).
    pub fn add_lighting_texture(
        &mut self,
        file: &str,
        blob: &TextureBlob,
    ) -> Result<(), LightingError> {
        self.lighting
            .add_texture(&self.device, &self.queue, file, blob)
    }

    /// Hands over one texture of the group `water` by its file name (`waterbumps16.lean`,
    /// `waterbumps16.lean#cross`, `waterdepthcolor_new.dds`, `watersky.dds`).
    pub fn add_water_texture(&mut self, file: &str, blob: TextureBlob) {
        if file.eq_ignore_ascii_case(crate::world::water::data::MARSH_STAMP) {
            // The stamp of the marsh is a mask of the terrain build (stage `Marsh`).
            if let Some(stamp) = crate::world::water::river::marsh::MarshStamp::from_blob(&blob) {
                self.world
                    .terrain
                    .install_stage(crate::world::water::river::marsh::stage(Arc::new(stamp)));
            }
        }
        self.water_data.add_texture(file, blob);
        self.water_data_dirty = true;
    }

    /// The camera of the reflection pass for this view, or `None` when the pass is off:
    /// `ReflectionLevel` 0 has no `ReflectionColor`, and nothing is drawn before the world is.
    fn reflection_camera(
        &self,
        camera: &dyn CameraView,
        aspect: f32,
        ours_to_orig: &Mat4,
    ) -> Option<crate::reflection::ReflectionCamera> {
        self.targets.get(crate::frame::REFLECTION_COLOR)?;
        let (top_left, bottom_right) = view_rect(camera, aspect, ours_to_orig, 256.0);
        Some(crate::reflection::ReflectionCamera::standard(
            top_left,
            bottom_right,
        ))
    }

    /// The `Water` subsystem: its pack, the heights of the map, the lighting, the maps of the other
    /// subsystems and the camera. Returns whether it draws the surface in this frame (the
    /// adapter of B1's water leaves the plan); until it can, B1's water stays.
    fn feed_water(
        &mut self,
        camera: &dyn CameraView,
        aspect: f32,
        ours_to_orig: Mat4,
        fow_view: crate::world::fow::mask::FowView,
    ) -> bool {
        if !self.world_drawn() {
            return false;
        }
        if self.water_data_dirty {
            self.world.water.set_data(Arc::new(self.water_data.clone()));
            self.water_data_dirty = false;
        }
        let overlay_map = self
            .world
            .overlay
            .screen_overlay_active()
            .then(|| self.targets.get(crate::frame::OVERLAY_MAP))
            .flatten()
            .map(|t| t.view.clone());
        {
            let WorldSystems { terrain, water, .. } = &mut self.world;
            // The heights wait for the cells the fog opens: a field half built is a coast that
            // moves, and the texture of the depth is made again at each revision.
            if terrain.cells_wanted() > 0
                && terrain.cells_done() >= terrain.cells_wanted()
                && let (Some(sampler), Some(basis)) = (terrain.sampler(), terrain.basis())
            {
                let (w, h) = crate::world::terrain::build::bridge::WrapMode::of(basis.wrap_x)
                    .padded_size(i32::from(basis.width), i32::from(basis.height));
                let revision = sampler.height_revision();
                let dirty = if water.height_target() == Some(revision) {
                    Some(Vec::new())
                } else {
                    water
                        .height_target()
                        .and_then(|since| terrain.height_dirty_cells(since))
                };
                water.set_terrain(sampler, [w as f32, h as f32], dirty.as_deref(), &|| {
                    terrain.unbuilt_cells()
                });
            }
            water.set_lighting(
                self.lighting.fog_ramp().cloned(),
                // The fog of war of the generic class (`TransFOWColor`).
                self.lighting.terrain_color_key(2, 0).cloned(),
            );
            water.set_river_key(self.lighting.rivers_color_key().cloned());
            water.set_maps(
                self.targets
                    .get(crate::frame::REFLECTION_COLOR)
                    .map(|t| t.view.clone()),
                // The fog's mask while the fluid fog is in the plan (it draws it for this view); switched
                // off, the target holds the mask of an older view: the water is drawn as seen instead.
                (!self.layers.is_disabled(SystemId::FluidFow))
                    .then(|| self.targets.get(crate::frame::FOW_MASK))
                    .flatten()
                    .map(|t| t.view.clone()),
                // The map of the overlay (the grid, the borders): its rectangle is the mask's
                // (`MaskView::of` is `MapView` of the camera), the one `OverlayProjection` of the
                // water, so the ocean and the rivers carry the grid like the ground does.
                overlay_map,
                self.shadow.active().then(|| self.shadow.map_view().clone()),
            );
        }
        let fade = self.settings.get("FadeShadows") != Some(0) && self.shadow.active();
        // The camera of the pass is the camera the water reads the target through; the matrix is
        // the same one even when there is no target (black reflection).
        let reflect_vp = {
            let (top_left, bottom_right) = view_rect(camera, aspect, &ours_to_orig, 256.0);
            crate::reflection::ReflectionCamera::standard(top_left, bottom_right).view_proj()
        };
        let time = self.shader_time();
        self.world
            .water
            .set_frame(crate::world::water::ocean::WaterFrame {
                view_proj: camera.view_proj(aspect),
                eye: camera.eye(),
                ours_to_orig,
                time,
                shadow_vp: self.shadow.shadow_vp(),
                shadow_fade: if fade { 1.0 } else { 0.0 },
                reflect_vp,
                overlay_proj: Some(crate::world::fow::mask::MaskView::of(&fow_view).projection()),
            });
        // The group is made in the first `WaterPrep` after the subsystem can draw; if that does not
        // work in a few frames B1's water stays.
        if self.world.water.ready() {
            self.water_unbound_frames = 0;
            true
        } else if self.world.water.eligible() && self.water_unbound_frames < 3 {
            self.water_unbound_frames += 1;
            true
        } else {
            false
        }
    }

    /// The surf, the ice and the arrow of the ranged attack (card R5.8, R5.9): the camera and the
    /// clock of the frame, the map, and what the water surface shares.
    fn feed_extras(&mut self, camera: &dyn CameraView, aspect: f32, ours_to_orig: Mat4) {
        let time = self.shader_time();
        let view_proj = camera.view_proj(aspect);
        let shifts = match &self.terrain {
            Some(t) => copy_shifts(t.width, &view_proj),
            None => vec![0.0],
        };
        let view_proj_orig = view_proj * ours_to_orig.inverse();
        let WorldSystems {
            terrain,
            water,
            ice,
            waves,
            arrows,
            ..
        } = &mut self.world;
        if let (Some(data), Some(sampler)) = (water.data().cloned(), terrain.sampler()) {
            let sea_level = sampler.sea_level();
            let _s = crate::perf::slow::scope("extras.waves_ice_feed");
            waves.feed(crate::world::waves::WavesFeed {
                sea_level,
                data: data.clone(),
                view_proj_orig,
                shifts: shifts.clone(),
                time,
            });
            ice.feed(water.river_feed().map(|water| crate::world::ice::IceFeed {
                water,
                data,
                sea_level,
            }));
            if let Some(basis) = terrain.basis() {
                let frame = *sampler.frame();
                let s = crate::perf::slow::scope("extras.waves_set_map");
                waves.set_map(basis, &frame, &terrain.products().coast_segments);
                drop(s);
                let _s = crate::perf::slow::scope("extras.ice_set_map");
                ice.set_map(basis, &frame);
            }
        }
        if let Some(sampler) = terrain.sampler() {
            let frame = sampler.frame();
            let world_width = if frame.wrap_x() {
                frame.half_world().x * 2.0
            } else {
                0.0
            };
            let _s = crate::perf::slow::scope("extras.arrows_ends");
            let ground = |x: i32, y: i32| {
                let (x, y) = frame.grid_shape().normalize(x, y)?;
                Some(ours_to_orig.transform_point3(sampler.hex_location(x, y)))
            };
            arrows.set_ends(&ground, world_width);
        }
        arrows.set_frame(crate::world::arrows::ArrowFrame {
            view_proj_orig,
            shifts,
            time,
        });
    }

    /// Hands over the world settings (`tb:world-b2`).
    pub fn set_world_settings(&mut self, settings: asset_format::WorldSettings) {
        self.world.arrows.set_settings(settings.arrows.clone());
        self.world.overlay.set_hex_grid_style(
            crate::world::overlay::grid::HexGridStyle::from_settings(&settings.hex_grid),
        );
        self.world.overlay.set_border_style(
            crate::world::overlay::borders::BorderStyle::from_settings(&settings.culture_borders),
        );
        self.world
            .overlay
            .set_highlight_styles(&settings.highlights);
        self.world.overlay.set_ring_style(&settings.overlay_defs);
        crate::frame::wire_build_stages(&mut self.world.terrain, &settings);
        self.world.terrain.install_stage(self.world.river.stage());
        self.world.terrain.decals_mut().set_types(
            &crate::world::landmark::decals::with_field_types(&settings.decals),
        );
        {
            let types = self.world.terrain.decals().types();
            let mut counts = [0usize; 10];
            for (i, c) in counts.iter_mut().enumerate() {
                *c = types.of(crate::world::decal::DecalCategory(i as u8)).len();
            }
            self.world.river.shared().set_decal_type_counts(counts);
        }
        self.world.terrain.set_defs(settings.terrain_defs.clone());
        self.water_data.set_settings(&settings);
        self.water_data_dirty = true;
        self.world.decal.set_types(&settings.decals);
        self.lighting.set_world_settings(settings);
    }

    /// With `true` the world is not drawn until the boot group is complete
    /// (`LightingEnv::ready`); `client-app` sets it when the manifest has a boot group and shows the
    /// loading screen meanwhile. Off by default: the assets of B1 have no boot group.
    pub fn set_boot_required(&mut self, required: bool) {
        self.boot_required = required;
    }

    /// `?debug=shadow`: shows the shadow map (grey, white where something shades) in place of the
    /// picture, and runs the shadow passes even without a boot group.
    pub fn set_debug_shadow(&mut self, on: bool) {
        self.debug_shadow = on;
    }

    /// The shadow map of the last frame.
    pub fn shadow(&self) -> &ShadowMap {
        &self.shadow
    }

    /// The world is drawn in this frame (see [`Self::set_boot_required`]).
    pub fn world_drawn(&self) -> bool {
        world_drawn(self.boot_required, &self.lighting)
    }

    /// What the terrain's pages and draw did in the last frame (`terrain-scroll`).
    pub fn terrain_live_stats(&self) -> crate::world::terrain::live::LiveStats {
        self.world.terrain.live_stats()
    }

    /// The turn of the view shown: when it changes, the counters of the turn (`terrain.rebakes_turn`)
    /// start again.
    pub fn note_turn(&mut self, turn: i32) {
        crate::perf::slow::set_turn(turn);
        self.world.terrain.note_turn(turn);
    }

    /// The quality keys in force.
    /// A target of the pool (`ReflectionColor` for the tests of the reflection pass).
    #[cfg(test)]
    pub(crate) fn target(&self, id: crate::frame::ResourceId) -> Option<&crate::frame::Target> {
        self.targets.get(id)
    }

    pub fn settings(&self) -> &RenderSettings {
        &self.settings
    }

    /// Takes new quality keys without a restart of the game (AC-R8.1). `on_settings` goes to the
    /// subsystems that read a changed key and to no others. The state and the log keep the value the
    /// player chose; what is in force for the main pass is the next line.
    ///
    /// A new `MSAASamples` that changes the effective count (1 and 4) is not applied yet: the main
    /// pass and every pipeline are made for one count, and rebuilding them without losing the
    /// uploaded models needs changes in the B1 pipeline files that RC-3 does not own. It takes
    /// effect at the next start (`Self::with_settings`). Nothing else waits for a restart.
    pub fn set_settings(&mut self, settings: RenderSettings) {
        let changed = self.settings.changed(&settings);
        if changed.is_empty() {
            return;
        }
        tracing::info!(keys = ?changed.names().collect::<Vec<_>>(), "render settings changed");
        self.settings = settings;
        // SIMPLIFIED(SIMP-066): an MSAA change waits for the next start (pipelines and targets keep their count)
        if self.settings.effective_msaa() != self.samples {
            tracing::warn!(
                requested = self.settings.msaa_samples,
                in_force = self.samples,
                "MSAA change takes effect at the next start"
            );
        }
        let called =
            self.world
                .on_settings(&self.device, &mut self.targets, &self.settings, &changed);
        tracing::debug!(?called, "on_settings");
        if changed.contains("FOWLevel") || changed.contains("OverlayLevel") {
            // The fog targets change size with the level, and so does the map of the overlay; the
            // pool makes only those again.
            crate::frame::declare_overlay_targets(
                &mut self.targets,
                self.settings.get("FOWLevel").unwrap_or(3),
                Some(crate::world::overlay::map::map_side(
                    self.settings.get("OverlayLevel").unwrap_or(2),
                )),
            )
            .expect("overlay targets are valid");
        }
        if changed.contains("OverlayLevel") || changed.contains("Use Screen Space Overlay") {
            self.world.overlay.set_overlay_settings(
                self.settings.get("OverlayLevel").unwrap_or(2),
                self.settings.get("Use Screen Space Overlay").unwrap_or(1),
            );
        }
    }

    /// The subsystems switched off now.
    pub fn layers(&self) -> &LayerSwitches {
        &self.layers
    }

    /// Switches subsystems off or on for the next frames; `render-core` leaves the switched-off
    /// ones out of the plan, so their `prepare` and `record` are not called.
    pub fn set_layers(&mut self, layers: LayerSwitches) {
        if layers == self.layers {
            return;
        }
        tracing::info!(off = ?layers.names().collect::<Vec<_>>(), "layer switches changed");
        self.layers = layers;
    }

    /// Takes in what the camera of this frame asks (client-hooks C2): the hints for the passes, and the clouds
    /// of the fluid fog hidden while the camera asks it and drawn again when it stops (the fog's mask is drawn
    /// all the while: [`crate::world::fow::FluidFow::set_clouds_hidden`]). The strategic camera asks nothing:
    /// no change.
    fn apply_camera_hints(&mut self, hints: CameraHints) {
        self.cam_hints = hints;
        self.world
            .fluid_fow
            .set_clouds_hidden(hints.hide_fow_clouds);
    }

    /// True when no subsystem has baking or loading left, so a captured frame is final
    /// (`golden`). B1 draws synchronously, so only the stubs and the services count.
    pub fn settled(&self) -> bool {
        self.world.services.pending_work() == 0
            && settled(&self.world.list_all(), &|id| !self.layers.is_disabled(id))
            && !self.forest_draws_stale()
            && !self.heights_stale()
    }

    /// The native trees were placed on a terrain that has built more cells since (they stand on its
    /// heights and keep out of its stencil, and are placed again only a few times a second).
    /// The landmarks and cities stand on heights older than the terrain's (placed again a few
    /// times a second, see `HEIGHT_REPLACE_FRAMES`).
    fn heights_stale(&self) -> bool {
        self.landmark_height_rev != u64::MAX
            && (self.world.terrain.height_revision() != self.landmark_height_rev
                || !self.height_cells_waiting.is_empty()
                || self.world.landmark.placing_pending()
                || self.world.city.placing_pending())
    }

    fn forest_draws_stale(&self) -> bool {
        !self.layers.is_disabled(SystemId::Forest)
            && self.world.forest.is_native()
            && (self.forest_dirty
                || self.forest_cuts_dirty
                || !self.forest_cut_queue.is_empty()
                || self.world.forest.pending_cells() > 0
                || self.world.forest.dirty_blocks() > 0
                || !self.forest_cells_waiting.is_empty()
                || self.world.terrain.cells_done() != self.forest_gate_cells)
    }

    /// What keeps [`Renderer::settled`] false: the shared services and every enabled system with pending
    /// work, as `name: count` (for the error of a scene that does not settle).
    pub fn pending_work(&self) -> String {
        let mut out = Vec::new();
        let services = self.world.services.pending_work();
        if services > 0 {
            out.push(format!("services: {services}"));
        }
        if self.forest_draws_stale() {
            out.push("trees placed on an older terrain".into());
        }
        if self.heights_stale() {
            out.push("landmarks and cities placed on an older terrain".into());
        }
        for s in self.world.list_all() {
            if !self.layers.is_disabled(s.id()) && s.pending_work() > 0 {
                out.push(format!("{:?}: {}", s.id(), s.pending_work()));
            }
        }
        out.join(", ")
    }

    /// Ignores zero sizes (minimized). `scale` is passed to overlays.
    pub fn resize(&mut self, size: (u32, u32), scale: f32) {
        if scale > 0.0 {
            self.scale = scale;
        }
        let max = self.device.limits().max_texture_dimension_2d;
        let (w, h) = (size.0.min(max), size.1.min(max));
        if w == 0 || h == 0 || (w, h) == (self.config.width, self.config.height) {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.config);
        }
        if self.headless.is_some() {
            self.headless = Some(Headless::new(&self.device, (w, h)));
        }
        self.targets.set_screen(self.scene_size());
    }

    /// Skipped frames are not errors; the surface is reconfigured.
    pub fn render(
        &mut self,
        scene: &Scene,
        camera: &dyn CameraView,
        ui: &UiDrawList,
        overlay: Option<&mut dyn Overlay>,
    ) -> Result<(), RenderError> {
        if self.frame_clock.1.is_none() {
            self.mark_frame_start();
        }
        let Some(surface) = &self.surface else {
            return self.render_headless(scene, camera, ui, overlay);
        };
        let _slow1 = crate::perf::slow::scope("render.acquire");
        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                // Draw this frame; reconfigure for the next.
                surface.configure(&self.device, &self.config);
                t
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                surface.configure(&self.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => return Err(RenderError::SurfaceValidation),
        };
        drop(_slow1);
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.view_format),
            ..Default::default()
        });
        self.output = self
            .config
            .usage
            .contains(wgpu::TextureUsages::COPY_SRC)
            .then(|| frame.texture.clone());
        let _slow2 = crate::perf::slow::scope("render.encode_frame");
        let commands = self.encode_frame(&view, scene, camera, ui, overlay);
        drop(_slow2);
        self.output = None;
        let _slow3 = crate::perf::slow::scope("render.submit");
        self.queue.submit([commands]);
        drop(_slow3);
        let _slow4 = crate::perf::slow::scope("render.frame_submitted");
        self.frame_submitted();
        drop(_slow4);
        let _slow5 = crate::perf::slow::scope("render.present");
        self.queue.present(frame);
        drop(_slow5);
        Ok(())
    }

    /// A frame of a renderer without a window, into its own texture (`Present` and the UI run as
    /// for a surface, so `FrameSource::WithUi` is the frame a window would show).
    fn render_headless(
        &mut self,
        scene: &Scene,
        camera: &dyn CameraView,
        ui: &UiDrawList,
        overlay: Option<&mut dyn Overlay>,
    ) -> Result<(), RenderError> {
        let Some(h) = &self.headless else {
            return Ok(());
        };
        let (view, texture) = (h.view.clone(), h.texture.clone());
        self.output = Some(texture);
        let commands = self.encode_frame(&view, scene, camera, ui, overlay);
        self.output = None;
        self.queue.submit([commands]);
        self.frame_submitted();
        Ok(())
    }

    /// A renderer without a window or a surface, drawing into its own `Rgba8Unorm` texture: for
    /// `golden` and for tests. Native only. `render` works as with a window; read the pixels with
    /// [`Self::read_frame`].
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn new_headless(size: (u32, u32), pref: BackendPref) -> Result<Self, RenderError> {
        Self::new_headless_with(size, pref, RenderSettings::default()).await
    }

    /// [`Self::new_headless`] with the quality keys given: the samples of the main pass are fixed when the
    /// renderer is made (`golden` draws with `MSAASamples` 4).
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn new_headless_with(
        size: (u32, u32),
        pref: BackendPref,
        settings: RenderSettings,
    ) -> Result<Self, RenderError> {
        #[cfg(test)]
        let gpu_lock = crate::gpu_test::GpuLock::acquire();
        let instance = Self::instance(Self::fallback_backends(pref));
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                ..Default::default()
            })
            .await?;
        let (device, queue) = Self::request_device(&adapter).await?;
        let (width, height) = (size.0.max(1), size.1.max(1));
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            format: WORLD_FORMAT,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width,
            height,
            present_mode: wgpu::PresentMode::AutoNoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        };
        let info = backend_info(&adapter);
        tracing::info!(backend = ?info.kind, adapter = %info.adapter, "renderer: headless");
        let mut r = Self::assemble(None, config, WORLD_FORMAT, device, queue, info, settings);
        r.headless = Some(Headless::new(&r.device, (width, height)));
        #[cfg(test)]
        {
            r._gpu_lock = gpu_lock;
        }
        Ok(r)
    }

    /// `ours_to_orig` of the map now in use: the move of the map into the basis of the original
    /// (`TerrainFrame`), or only the rotation `B` until a map has set it. `golden` brings the matrices of a
    /// capture into our basis with it (`P_ours = P_orig * ours_to_orig`).
    pub fn ours_to_orig(&self) -> Mat4 {
        self.world_frame
            .unwrap_or_else(crate::frame::basis_rotation)
    }

    /// The fragment of the class mask, for subsystems that make mask pipelines in `init`.
    pub fn mask_shader(&self) -> &MaskShader {
        &self.mask_shader
    }

    /// Asks for a copy of `what` in the next frame that is rendered. Poll the [`Readback`] (or
    /// `wait` for it natively) after rendering. Asking for the class mask makes that frame run the
    /// mask pass too.
    pub fn read_frame(&mut self, what: FrameSource) -> Readback {
        if what == FrameSource::ClassMask {
            declare_mask_targets(&mut self.targets).expect("mask targets are valid");
        }
        let (readback, request) = Readback::new(&self.device, what);
        self.requests.push(request);
        readback
    }

    /// A renderer without a surface for native tests: draws into textures of `view_format`
    /// through [`Self::encode_frame`]. Use `Rgba8Unorm` (a surface without sRGB) or
    /// `Rgba8UnormSrgb` (`Present` linearises); `samples` is 1 or 4.
    #[cfg(test)]
    pub(crate) fn offscreen(
        gpu: &crate::gpu_test::Gpu,
        size: (u32, u32),
        view_format: wgpu::TextureFormat,
        samples: u32,
    ) -> Self {
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            format: view_format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.0,
            height: size.1,
            present_mode: wgpu::PresentMode::AutoNoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        };
        let info = describe_backend(gpu.info.clone(), gpu.device.features());
        let settings = RenderSettings {
            msaa_samples: samples,
            ..RenderSettings::default()
        };
        let mut r = Self::assemble(
            None,
            config,
            view_format,
            gpu.device.clone(),
            gpu.queue.clone(),
            info,
            settings,
        );
        // The tests read exact colours: no haze over them (`set_layers` of a test may bring it back).
        r.layers = LayerSwitches::from_names("DistanceFog").0;
        r
    }

    #[cfg(test)]
    pub(crate) fn statics_unit_family_on(&self) -> bool {
        self.statics.unit_family_on()
    }

    /// The ground is drawn with the light of the original (WT-0).
    #[cfg(test)]
    pub(crate) fn terrain_m1_on(&self) -> bool {
        self.terrain_m1.on()
    }

    /// A model drawn with the ported `UnitShader`, in the entity batches (after `set_terrain`,
    /// which would clear them).
    #[cfg(test)]
    pub(crate) fn test_unit(
        &mut self,
        blob: &ModelBlob,
        textures: &HashMap<LogicalId, TextureBlob>,
        instances: &[StaticInstance],
    ) {
        self.test_model(Look::Unit, blob, textures, instances);
    }

    /// As [`Self::test_unit`], for a model of any look (the buildings of WOB1).
    #[cfg(test)]
    pub(crate) fn test_model(
        &mut self,
        look: Look,
        blob: &ModelBlob,
        textures: &HashMap<LogicalId, TextureBlob>,
        instances: &[StaticInstance],
    ) {
        let id = self.statics.create_model_as(
            &self.device,
            &self.queue,
            blob,
            textures,
            Variants::Single,
            false,
            look,
        );
        self.statics
            .replace_set(&self.device, BatchSet::Entities, &[(id, instances)]);
    }

    /// The ported building shader is drawing cities (tests).
    #[cfg(test)]
    pub(crate) fn statics_building_family_on(&self) -> bool {
        self.statics.building_family_on()
    }

    /// The app calls it at the start of its frame, so that the CPU time of the frame covers the
    /// update too and not only the encoding (`perf-hud`). Without it the frame starts in `render`.
    pub fn mark_frame_start(&mut self) {
        self.frame_clock.1 = Some((self.cpu_clock)());
    }

    /// One frame into the ring of the HUD: period, CPU time, and the GPU time of the frame as the
    /// sum of the timed passes (a frame two or three back).
    fn sample_frame(&mut self) {
        let now = (self.cpu_clock)();
        let (last_end, start) = self.frame_clock;
        self.frame_clock = (Some(now), None);
        let Some(last_end) = last_end else { return };
        // The clock of a web renderer reads 0 until the app gives it one.
        if now <= last_end {
            return;
        }
        let gpu: f32 = self
            .frame_stats
            .passes
            .iter()
            .filter_map(|p| p.gpu_ms)
            .sum();
        let any_gpu = self.frame_stats.passes.iter().any(|p| p.gpu_ms.is_some());
        self.last_frame_cpu_ms = start.map(|s| (now - s.max(last_end)) as f32);
        self.frame_ring.push(crate::perf::FrameSample {
            at_ms: now,
            dt_ms: (now - last_end) as f32,
            cpu_ms: start.map_or(0.0, |s| (now - s.max(last_end)) as f32),
            gpu_ms: any_gpu.then_some(gpu),
        });
    }

    /// Everything the HUD, `window.__civ5stats` and the desktop log show, from the last frame.
    /// `graph` also fills the frame-time series (only the HUD wants it).
    pub fn perf_snapshot(&self, graph: bool) -> crate::perf::PerfSnapshot {
        use crate::perf::{PassRow, snapshot};
        let frame = self.frame_ring.summary();
        let passes = self
            .frame_stats
            .passes
            .iter()
            .map(|p| PassRow {
                name: crate::frame::pass_name(p.pass),
                gpu_ms: p.gpu_ms,
                cpu_ms: p.cpu_ms,
                draw_calls: p.counts.draw_calls,
                triangles: p.counts.triangles,
                instances: p.counts.instances,
                pipeline_sets: p.counts.pipeline_sets,
                bind_group_sets: p.counts.bind_group_sets,
            })
            .collect();
        let mut counters: Vec<(String, f64)> = vec![
            ("frame.fps".into(), f64::from(frame.fps)),
            ("frame.p50_ms".into(), f64::from(frame.dt_p50_ms)),
            ("frame.p99_ms".into(), f64::from(frame.dt_p99_ms)),
            ("frame.low1_fps".into(), f64::from(frame.low1_fps)),
            ("frame.cpu_ms".into(), f64::from(frame.cpu_ms)),
            // The workers of the jobs pool that run (B3 render-mt): 0 is the single-threaded build or
            // a client without threads.
            ("app.threads".into(), jobs::workers() as f64),
        ];
        if let Some(g) = frame.gpu_ms {
            counters.push(("frame.gpu_ms".into(), f64::from(g)));
        }
        // A long frame that repeats at the same distance (0: none), and by how much it is longer.
        let dts: Vec<f32> = self.frame_ring.dt_series().collect();
        let cycle = crate::perf::snapshot::stall_cycle(&dts);
        counters.push((
            "frame.stall_every".into(),
            cycle.map_or(0.0, |c| c.every as f64),
        ));
        counters.push((
            "frame.stall_ms".into(),
            cycle.map_or(0.0, |c| f64::from(c.excess_ms)),
        ));
        // The render scale and the sizes it makes: the scene is drawn at `scene`, the UI at `screen`.
        let (scene, screen) = (self.scene_size(), self.size());
        counters.push((
            "render.scale_pct".into(),
            f64::from(crate::render_scale::scale_to_percent(self.render_scale)),
        ));
        counters.push((
            "render.terrain_detail".into(),
            f64::from(self.terrain_detail().index()),
        ));
        counters.push(("render.scene_w".into(), f64::from(scene.0)));
        counters.push(("render.scene_h".into(), f64::from(scene.1)));
        counters.push(("render.screen_w".into(), f64::from(screen.0)));
        counters.push(("render.screen_h".into(), f64::from(screen.1)));
        counters.push((
            "render.shadow_quality".into(),
            f64::from(self.shadow_quality.index()),
        ));
        counters.push((
            "render.shadow_map_side".into(),
            f64::from(self.shadow.map_side()),
        ));
        // The multisampling in force, and the preset of the graphics choices (0 custom, 1 low .. 3 high).
        counters.push(("render.msaa".into(), f64::from(self.samples)));
        counters.push(("render.gfx_preset".into(), f64::from(self.gfx_preset)));
        // The raw map the casters are drawn into, and whether the blur makes the map from it.
        let raw = self.shadow.raw_px();
        counters.push(("render.shadow_raw_w".into(), f64::from(raw.0)));
        counters.push(("render.shadow_raw_h".into(), f64::from(raw.1)));
        counters.push((
            "render.shadow_blur".into(),
            f64::from(u8::from(self.shadow.blurs())),
        ));
        let mem = crate::perf::gpu_mem::snapshot();
        counters.push(("gpu_mem.total".into(), mem.total as f64));
        for (c, live, peak, n) in &mem.clusters {
            counters.push((format!("gpu_mem.{}", c.name()), *live as f64));
            counters.push((format!("gpu_mem.{}_peak", c.name()), *peak as f64));
            counters.push((format!("gpu_mem.{}_count", c.name()), *n as f64));
        }
        let ram = crate::perf::ram::snapshot();
        counters.push(("ram.heap_live".into(), ram.live as f64));
        counters.push(("ram.heap_peak".into(), ram.peak as f64));
        counters.push(("ram.allocs".into(), ram.allocs as f64));
        counters.push(("ram.alloc_bytes".into(), ram.alloc_bytes as f64));
        counters.push(("ram.wasm_linear".into(), ram.wasm_linear as f64));
        for (t, live) in &ram.by_tag {
            counters.push((format!("ram.{}", t.name()), *live as f64));
        }
        for (k, v) in &self.frame_stats.counters {
            counters.push(((*k).to_owned(), *v as f64));
        }
        counters.extend(crate::perf::extra_counters());
        let main = self
            .frame_stats
            .passes
            .iter()
            .find(|p| p.pass == PassId::Main)
            .map(|p| p.counts)
            .unwrap_or_default();
        crate::perf::PerfSnapshot {
            main_groups: self.frame_stats.groups.rows(main),
            groups_off: self.disabled_groups,
            graph: if graph {
                snapshot::decimate(self.frame_ring.dt_series(), snapshot::GRAPH_POINTS)
            } else {
                Vec::new()
            },
            frame,
            passes,
            counters,
            slow_frames: crate::perf::slow::ring(),
            frame_trace: crate::perf::slow::trace(),
        }
    }

    /// Starts the read of the pass timestamps of the frame just submitted. `render`
    /// calls it; a caller that submits `encode_frame` itself does too.
    pub(crate) fn frame_submitted(&mut self) {
        self.sample_frame();
        self.timer.after_submit();
        for request in self.in_flight.drain(..) {
            request.submitted();
        }
    }

    /// The commands of one frame into `view`, which has `view_format` and the surface size.
    pub(crate) fn encode_frame(
        &mut self,
        view: &wgpu::TextureView,
        scene: &Scene,
        camera: &dyn CameraView,
        ui: &UiDrawList,
        overlay: Option<&mut dyn Overlay>,
    ) -> wgpu::CommandBuffer {
        self.apply_camera_hints(camera.hints());
        // The build of a new basis goes on a few milliseconds a frame (when the terrain builds in pieces).
        if self.world.terrain.building() {
            let _s = crate::perf::slow::scope("terrain.build_job");
            self.world
                .terrain
                .step_job(crate::world::terrain::live::BASIS_JOB_MS);
        }
        let aspect = self.config.width as f32 / self.config.height as f32;
        let light = scene.light_dir.normalize_or(Vec3::Y);
        let globals = Globals {
            view_proj: camera.view_proj(aspect).to_cols_array_2d(),
            camera_pos: camera.eye().extend(1.0).to_array(),
            light_dir: light.extend(0.0).to_array(),
            params: Vec4::new(
                // The world is in gamma space: the shaders apply the gamma themselves.
                1.0,
                self.map_width as f32,
                FOG_HIDDEN_FACTOR,
                self.time_override.unwrap_or(self.anim_time),
            )
            .to_array(),
        };
        self.queue
            .write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
        // The ground of the original needs the move of the map into its basis from the first frame.
        if self.boot_required
            && self.world.terrain.textures_ready()
            && let Some(m) = self.world.terrain.take_frame_update()
        {
            self.world_frame = Some(m);
        }
        let ours_to_orig = self
            .world_frame
            .unwrap_or_else(crate::frame::basis_rotation);
        // `Off` of the shadow quality is level 0: no pass, a cleared map, everything lit.
        let shadow_divisor = self.shadow_quality.divisor();
        let shadow_level = self.shadow_quality.frame_level(
            self.settings.get("ShadowLevel").unwrap_or(0),
            self.shadow_quality_chosen,
        );
        let shadow_camera =
            ((self.boot_required || self.debug_shadow) && self.world_drawn() && shadow_level > 0)
                .then(|| {
                    // re: SetupShadowFrameGlobals: the light looks at the middle of the visible rectangle.
                    let (top_left, bottom_right) = view_rect(camera, aspect, &ours_to_orig, 256.0);
                    let light = ShadowCamera::new(
                        top_left,
                        bottom_right,
                        Vec3::from_array(crate::lighting::globals::SHADOW_DIR),
                    );
                    let caster_vp = light.view_proj() * ours_to_orig;
                    (light, caster_vp)
                });
        let unit_caster_vp = shadow_camera.as_ref().map(|(_, vp)| *vp);
        self.shadow.prepare(
            &self.device,
            &self.queue,
            shadow_level,
            self.scene_size(),
            shadow_divisor.unwrap_or(1),
            shadow_camera,
        );
        // A pass that is not encoded has no new time: the last one it had must not stay in the report.
        if !self.shadow.active() {
            self.timer.forget(PassId::ShadowMap);
        }
        if !self.shadow.blurs() {
            self.timer.forget(PassId::ShadowBlur);
        }
        let frame_globals = FrameGlobals::new(&FrameInputs {
            shadow_vp: self.shadow.shadow_vp(),
            distance_fog: !self.layers.is_disabled(crate::frame::SystemId::DistanceFog),
            fade_shadows: self.settings.get("FadeShadows") != Some(0),
            ..FrameInputs::new(
                camera.view_proj(aspect),
                camera.eye(),
                self.world_frame
                    .unwrap_or_else(crate::frame::basis_rotation),
                self.shader_time(),
            )
        });
        self.upload_uniform(&self.frame_globals, 0, &frame_globals);
        // The same constants for the render systems of mods (client-hooks R3).
        let mut shared = frame_globals;
        self.constant_overrides.apply(&mut shared);
        self.world.services.frame.set_globals(shared);
        let world_drawn = self.world_drawn();
        // The reflection pass (RC-12): the camera along `m_vReflectDir` and the frame constants
        // with its matrix. No target at `ReflectionLevel` 0, so no camera and no pass.
        let reflection_camera = (self.boot_required && world_drawn)
            .then(|| self.reflection_camera(camera, aspect, &ours_to_orig))
            .flatten();
        if let Some(cam) = &reflection_camera {
            let globals = FrameGlobals::new(&FrameInputs {
                shadow_vp: self.shadow.shadow_vp(),
                distance_fog: !self.layers.is_disabled(crate::frame::SystemId::DistanceFog),
                fade_shadows: self.settings.get("FadeShadows") != Some(0),
                ..FrameInputs::new(
                    cam.view_proj_ours(&ours_to_orig),
                    camera.eye(),
                    ours_to_orig,
                    self.shader_time(),
                )
            });
            self.upload_uniform(&self.reflection_globals, 0, &globals);
        }
        if self.boot_required {
            self.statics.prepare_units(
                &self.device,
                &self.lighting,
                &self.frame_globals,
                &self.reflection_globals,
            );
            // The light of the original on the ground: the pipeline once the boot group is in, the
            // groups of the styles every frame (the shadow map's target changes with its level).
            self.terrain_m1.prepare(
                &self.device,
                &self.lighting,
                &self.frame_globals,
                self.shadow.map_view(),
                [
                    &self.object_layout,
                    &self.blend.fog_layout,
                    self.terrain_pipeline.material_layout(),
                ],
            );
        }
        // The ground through the cache of the original, when the map, its textures and the light are
        // in: the work of the frame on the CPU is done here, the bake and the draw are in the plan.
        let terrain_native = self.boot_required
            && world_drawn
            && match crate::world::terrain::LiveLights::from_env(&self.lighting) {
                Some(lights) => {
                    let orig_to_ours = ours_to_orig.inverse();
                    let view_proj = camera.view_proj(aspect);
                    let eye = camera.eye();
                    // The fluid fog's mask (drawn this frame from the same view) and its lowest
                    // cloud layer, as the original's cells read them. Only while the fluid fog is in the
                    // plan: switched off (`[DisableSystems] FluidFOW`, the player's switch) it does not
                    // draw the mask, and the target holds the mask of an older view, which the cells would
                    // read through this view's rectangle (the seen ground the flat colour of the clouds,
                    // 2026-11-07). The cells read B1's fog then, as the units do. A mod's camera that
                    // hides the clouds keeps the mask drawn (`FluidFow::set_clouds_hidden`).
                    let fow = match (
                        self.targets.get(crate::frame::FOW_MASK),
                        self.world.fluid_fow.base_view(),
                        self.layers.is_disabled(SystemId::FluidFow),
                    ) {
                        (Some(mask), Some(base), false) => {
                            let world_width = self
                                .world
                                .terrain
                                .frame()
                                .map_or(1.0, |f| f.half_world().x * 2.0);
                            let view = crate::world::fow::mask::MaskView::of(
                                &crate::world::fow::mask::FowView {
                                    view_proj,
                                    eye,
                                    ours_to_orig,
                                },
                            );
                            let overlay = self
                                .world
                                .overlay
                                .screen_overlay_active()
                                .then(|| self.targets.get(crate::frame::OVERLAY_MAP))
                                .flatten()
                                .map(|t| &t.view);
                            Some(crate::world::terrain::draw::CellFow {
                                mask: &mask.view,
                                base,
                                overlay,
                                projection: view.projection(),
                                base_scale: crate::world::fow::clouds::base_scale(
                                    world_width,
                                    self.anim_time,
                                    &crate::world::fow::state::CLOUD_LAYERS[0],
                                ),
                                key: (
                                    std::ptr::from_ref(&mask.view) as usize,
                                    std::ptr::from_ref(base) as usize,
                                    overlay.map_or(0, |v| std::ptr::from_ref(v) as usize),
                                ),
                            })
                        }
                        _ => None,
                    };
                    let _slow_live = crate::perf::slow::scope("terrain.live_frame");
                    let inputs = crate::world::terrain::LiveInputs {
                        lights: &lights,
                        frame_globals: &self.frame_globals,
                        shadow_map: self.shadow.map_view(),
                        view: crate::world::terrain::LiveView {
                            view_proj_orig: view_proj * orig_to_ours,
                            eye_orig: ours_to_orig.transform_point3(eye),
                            // The size of the surface, not of the scene: the render scale must not
                            // choose the pages of the ground (`terrain_detail`).
                            screen_width: crate::terrain_detail::lod_screen_width(self.size()),
                            shifts: match &self.terrain {
                                Some(t) => copy_shifts(t.width, &view_proj),
                                None => vec![0.0],
                            },
                            near: self.cam_hints.near,
                            focus: self.cam_hints.terrain_focus,
                        },
                        decals: !self.layers.is_disabled(crate::frame::SystemId::Decal),
                        bake_gpu_ms: self.timer.gpu_ms(PassId::TerrainBake),
                        fow,
                        clock: Some(&*self.cpu_clock),
                        frame_cpu_ms: self.last_frame_cpu_ms,
                        bake_cpu_ms: self
                            .frame_stats
                            .passes
                            .iter()
                            .find(|p| p.pass == PassId::TerrainBake)
                            .map(|p| p.cpu_ms),
                    };
                    let cfg = crate::world::terrain::LiveConfig {
                        target: WORLD_FORMAT,
                        depth: DEPTH_FORMAT,
                        samples: self.samples,
                        float_targets: self.targets.has_float_targets(),
                    };
                    self.world
                        .terrain
                        .live_frame(&self.device, &self.queue, &cfg, &inputs)
                }
                None => false,
            };
        let model = ObjectUniform {
            model: scene.model_transform.to_cols_array_2d(),
            tint: [1.0, 1.0, 1.0, 0.8],
        };
        self.queue
            .write_buffer(&self.model.uniform, 0, bytemuck::bytes_of(&model));

        let _slow18 = crate::perf::slow::scope("render.models_prepare");
        self.models.prepare(&self.device, &self.queue);
        drop(_slow18);
        let fow_view = crate::world::fow::mask::FowView {
            view_proj: camera.view_proj(aspect),
            eye: camera.eye(),
            ours_to_orig,
        };
        self.world.set_fow_frame(fow_view);
        let _slow6 = crate::perf::slow::scope("render.feed_water");
        let water_native = self.feed_water(camera, aspect, ours_to_orig, fow_view);
        drop(_slow6);
        {
            // The rivers of the last frame replace B1's strips in this one.
            let native = self.world.river.drawn_natively();
            self.world.overlay.set_rivers_native(native);
            let WorldSystems { river, water, .. } = &mut self.world;
            river.feed(water);
        }
        let _slow7 = crate::perf::slow::scope("render.feed_extras");
        self.feed_extras(camera, aspect, ours_to_orig);
        drop(_slow7);
        self.targets.sync(&self.device);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        let mut stats = FrameStats::default();
        let mut tally = PassTally::new();
        let mut group_tally = crate::perf::GroupTally::default();
        let groups_off = self.disabled_groups;
        self.timer.begin_frame(&self.device);
        // Only the map copies that reach the screen are drawn: one in the middle of a
        // wrapping map, two near the seam. Each is a view; the subsystems are called once per copy.
        let shifts = match &self.terrain {
            Some(t) => copy_shifts(t.width, &camera.view_proj(aspect)),
            None => vec![0.0],
        };
        // WOB2: the units of `assets-b2` draw through their own path once the art and the light are
        // in; B1's units leave its plan when that starts, and come back when it stops.
        let units_native = self.units_b2_ready(world_drawn);
        if !units_native {
            self.world
                .unit
                .set_path_pieces(Vec::new(), Default::default());
        }
        if self.world.unit.set_b2_active(units_native) {
            self.rebuild_entity_batches();
        }
        // WOB4: the same for the landmarks. B1's icons of the improvements go with them, and the
        // resources keep theirs (`rebuild_icons`).
        let landmarks_native = self.landmarks_b2_ready(world_drawn);
        let height_rev = self.world.terrain.height_revision();
        // The models were placed on the heights of the cells built then (resources on an empty
        // field stood under the ground). With the whole map revealed the cells build by the
        // thousand and the revision changes every frame; placing every plot and every tree again
        // each time took a second a frame (the gallery ran at 0.3 fps), so the placing follows the
        // heights a few times a second at most, as the trees follow the stencil.
        if height_rev != self.landmark_height_rev
            && (self.landmark_height_rev == u64::MAX
                || self.frame_no.saturating_sub(self.landmark_replaced) >= HEIGHT_REPLACE_FRAMES)
        {
            // Only the plots and cities on the cells that changed are looked at, and placed again
            // when the fingerprint of their ground really differs.
            let dirty = if self.landmark_height_rev == u64::MAX {
                None
            } else {
                self.world
                    .terrain
                    .height_dirty_cells(self.landmark_height_rev)
            };
            self.landmark_height_rev = height_rev;
            self.landmark_replaced = self.frame_no;
            match dirty {
                Some(cells) => {
                    // A plot is looked at once the heights round it are final, not at each cell
                    // around it the field builds (eight revisions of its ground, a placing each).
                    let cells = self
                        .world
                        .terrain
                        .final_height_cells(cells, &mut self.height_cells_waiting);
                    self.height_cells_checked += cells.len() as u64;
                    self.world.landmark.heights_dirty(&cells);
                    self.world.city.heights_dirty(&cells);
                    self.world.unit.heights_dirty(&cells);
                }
                None => {
                    self.height_cells_waiting.clear();
                    self.world.landmark.heights_changed();
                    self.world.city.heights_changed();
                    self.world.unit.invalidate_all();
                }
            }
        }
        if self.world.landmark.set_b2_active(landmarks_native) {
            self.rebuild_decor();
        }
        // WOB5: the cities too; B1's composed model leaves its plan, the banners stay.
        let cities_native = self.cities_b2_ready(world_drawn);
        if self.world.city.set_b2_active(cities_native) {
            self.rebuild_entity_batches();
        }
        if units_native || landmarks_native || cities_native {
            let view = crate::world::fow::mask::FowView {
                view_proj: camera.view_proj(aspect),
                eye: camera.eye(),
                ours_to_orig,
            };
            let mask = crate::world::fow::mask::MaskView::of(&view);
            // The mask is read where the fluid fog draws it; the classic fog is an overlay of its own.
            let fow = (self.world.fluid_fow.clouds_active()
                && !self.layers.is_disabled(SystemId::FluidFow))
            .then(|| self.targets.get(crate::frame::FOW_MASK))
            .flatten()
            .map(|t| crate::world::unit::draw::FowSource {
                view: &t.view,
                min: mask.min,
                size: mask.size,
            });
            // The instances of the objects are culled per pass: the main view, the light's
            // rectangle (what lands in the shadow map) and the reflection camera's.
            let cull = crate::world::unit::cull::CullSet::new(
                Some(camera.view_proj(aspect)),
                unit_caster_vp,
                reflection_camera
                    .as_ref()
                    .map(|c| c.view_proj_ours(&ours_to_orig)),
            );
            // `FarLOD`: what is far casts no shadow and is not in the reflection.
            let cull = if self.far_lod() {
                cull.with_far(
                    camera.eye(),
                    self.cam_hints
                        .far_lod_distance
                        .unwrap_or(crate::world::far_lod::FAR_LOD_DISTANCE),
                )
            } else {
                cull
            };
            // A low eye (a mod's first-person camera): the objects smaller about where they stand, and
            // none that comes up to the eye in the view.
            let cull = match self.cam_hints.objects.filter(|o| !o.is_identity()) {
                Some(o) => cull.with_near(crate::world::unit::cull::NearEye {
                    eye: camera.eye(),
                    scale: o.scale,
                    clearance: o.clearance,
                }),
                None => cull,
            };
            for draw in [
                self.world.unit.b2_draw(),
                self.world.landmark.b2_draw(),
                self.world.city.b2_draw(),
            ]
            .into_iter()
            .flatten()
            {
                draw.set_cull(cull);
            }
            if let Some(vp) = unit_caster_vp {
                let globals = FrameGlobals::new(&FrameInputs::new(
                    vp,
                    camera.eye(),
                    ours_to_orig,
                    self.shader_time(),
                ));
                for draw in [
                    self.world.unit.b2_draw(),
                    self.world.landmark.b2_draw(),
                    self.world.city.b2_draw(),
                ]
                .into_iter()
                .flatten()
                {
                    self.upload_uniform(draw.shadow_globals(), 0, &globals);
                }
            }
            let sampler = self.world.terrain.sampler();
            let ground = |p: glam::Vec2| sampler.map_or(0.0, |s| s.world_height(p));
            // WOV6: the pieces of the path draw with the figures.
            {
                self.world
                    .movement_path
                    .sync_models(&mut self.world.services.anim);
                let mut wanted = self.world.movement_path.wanted_models();
                let mut batches = self
                    .world
                    .movement_path
                    .model_batches(&self.world.services.anim, &ground);
                // The god ray stands over the hex of the selected unit, like the ring; it is an
                // effect model drawn with the pieces (a set without the model draws nothing).
                let ray_time = self.shader_time();
                let selected = self.world.overlay.highlights().selected;
                self.god_ray.update(selected, ray_time);
                if let Some(ray) = self.god_ray.instance(&ground) {
                    wanted.extend(self.god_ray.wanted());
                    batches
                        .entry(crate::world::god_ray::model())
                        .or_default()
                        .push(ray);
                }
                self.world.unit.set_path_pieces(wanted, batches);
            }
            if let Some(art) = &self.entities.art_b2 {
                let frame = crate::world::unit::B2Frame {
                    art,
                    owners: &self.entities.owners,
                    ground: &ground,
                    water_level: Some(crate::world::water::water_level()),
                    lights: &self.lighting,
                    frame_globals: &self.frame_globals,
                    reflection_globals: &self.reflection_globals,
                    fow,
                    shifts: &shifts,
                    eye: camera.eye(),
                };
                let _slow8 = crate::perf::slow::scope("unit.b2_frame");
                self.world.unit.b2_frame(
                    &self.device,
                    &self.queue,
                    &mut self.world.services.anim,
                    &frame,
                );
                drop(_slow8);
                // The smoke of a pillaged improvement shows on visible plots only. The sets below
                // follow the snapshot and the records; they are made again only when one changed.
                let snap = self.snapshot.as_ref();
                let key = (
                    self.snapshot_gen,
                    self.world.city.epoch(),
                    self.world.landmark.epoch(),
                    self.world.overlay.owners_revision(),
                    sampler.is_some(),
                );
                let feeds_changed = self.feed_key != Some(key);
                if feeds_changed {
                    self.feed_key = Some(key);
                    let _slow16 = crate::perf::slow::scope("render.feed_set_seen");
                    self.world.landmark.set_seen(|x, y| {
                        snap.and_then(|s| crate::entities::fog_of(s, x, y))
                            .is_some_and(|(_, fog)| fog == crate::terrain::Fog::Visible)
                    });
                    drop(_slow16);
                    let shape = self.snapshot.as_ref().map(|s| s.shape());
                    if let Some(shape) = shape {
                        let cities = self.world.city.cities().map(|c| (c.x, c.y)).collect();
                        self.world.landmark.set_net(shape, cities);
                    }
                }
                // The ported `BuildingPS_CODE_Opaque` of the city buildings and of the
                // `BuildingShader` meshes of the landmarks: the base layer of the fog of war (as
                // the trees read it), its scroll, the colour key of the fog of war and the eye.
                let world_width = self
                    .world
                    .terrain
                    .frame()
                    .map_or(1.0, |f| f.half_world().x * 2.0);
                let code = crate::world::unit::draw::CodeFrame {
                    fow_base: self
                        .world
                        .fluid_fow
                        .base_view()
                        .or_else(|| self.targets.get(crate::frame::FOW_BASE).map(|t| &t.view)),
                    fow_base_scale: crate::world::fow::clouds::base_scale(
                        world_width,
                        self.anim_time,
                        &crate::world::fow::state::CLOUD_LAYERS[0],
                    ),
                    // TODO(RW-4 g): the class of the fog-of-war key of the buildings; the generic one.
                    fow_key: self
                        .lighting
                        .terrain_color_key(2, crate::lighting::STYLES.len() - 1),
                    eye_orig: ours_to_orig.transform_point3(camera.eye()),
                };
                self.world.landmark.set_code_frame(&self.queue, &code);
                if let Some(frame) = self.world.terrain.frame() {
                    let heading = crate::world::landmark::heading::HeadingField::new(frame);
                    self.world.landmark.set_heading(heading);
                }
                let started = (self.cpu_clock)();
                let _slow9 = crate::perf::slow::scope("landmark.b2_frame");
                self.world.landmark.b2_frame(
                    &self.device,
                    &self.queue,
                    &mut self.world.services.anim,
                    &frame,
                );
                drop(_slow9);
                self.landmark_ms = (self.cpu_clock)() - started;
                // R5.11: the Great Wall follows the owners of the borders.
                if feeds_changed {
                    self.world
                        .city
                        .set_territory(self.world.overlay.owners(), sampler.map(|s| *s.frame()));
                }
                // WOB5: the hex-edge pattern of each city reads the water, mountains and rivers.
                if feeds_changed {
                    let _slow17 = crate::perf::slow::scope("render.feed_city_plots");
                    self.world.city.set_plots(snap.map(|s| {
                        use crate::terrain::{Relief, TerrainKind};
                        crate::world::city::PlotGrid {
                            width: s.width() as i32,
                            height: s.height() as i32,
                            wrap_x: s.wrap_x(),
                            cells: s
                                .cells()
                                .iter()
                                .map(|c| crate::world::city::layout::PlotEdges {
                                    water: c.relief == Relief::Water
                                        || matches!(
                                            c.kind,
                                            TerrainKind::Coast | TerrainKind::Ocean
                                        ),
                                    mountain: c.relief == Relief::Mountain,
                                    rivers: c.rivers,
                                })
                                .collect(),
                        }
                    }));
                    drop(_slow17);
                    // re: the VEFFECTs of the city damage have no FOWMask (= visible plots only).
                    self.world.city.set_seen(|x, y| {
                        snap.and_then(|s| crate::entities::fog_of(s, x, y))
                            .is_some_and(|(_, fog)| fog == crate::terrain::Fog::Visible)
                    });
                }
                let loading = self.map_loading();
                self.world.city.set_loading(loading);
                self.world.city.set_code_frame(&self.queue, &code);
                let started = (self.cpu_clock)();
                let _slow10 = crate::perf::slow::scope("city.b2_frame");
                self.world.city.b2_frame(
                    &self.device,
                    &self.queue,
                    &mut self.world.services.anim,
                    &frame,
                );
                drop(_slow10);
                self.city_ms = (self.cpu_clock)() - started;
                // R5.12: the fire particles follow the damage models of the cities.
                for (key, fires) in self.world.city.take_fire_changes() {
                    self.world.particle.city_fires(key, &fires);
                }
            }
        }
        if landmarks_native {
            let _slow11 = crate::perf::slow::scope("render.apply_farm_decals");
            self.apply_farm_decals();
            drop(_slow11);
            let _slow12 = crate::perf::slow::scope("render.apply_route_decals");
            self.apply_route_decals();
            drop(_slow12);
        }
        let _slow13 = crate::perf::slow::scope("render.apply_ground_decals");
        self.apply_ground_decals();
        drop(_slow13);
        // Both are asked every frame (each forgets its single changes when it is taken whole).
        let landmarks_all = self.world.landmark.take_cuts_dirty();
        let cities_all = self.world.city.take_cuts_dirty();
        if landmarks_all || cities_all {
            self.forest_cuts_dirty = true;
            self.forest_cut_queue.clear();
        }
        self.forest_cut_queue
            .extend(self.world.landmark.take_cut_changes());
        self.forest_cut_queue
            .extend(self.world.city.take_cut_changes());
        let _slow14 = crate::perf::slow::scope("render.feed_forest");
        let forest_native =
            self.feed_forest(camera, aspect, ours_to_orig, fow_view, &shifts, world_drawn);
        drop(_slow14);
        self.world.forest.set_native(forest_native);
        let _slow15 = crate::perf::slow::scope("render.feed_particles");
        self.feed_particles(camera);
        drop(_slow15);
        let fog_state = self.world.fluid_fow.fog_gpu();
        let b1 = B1Draw {
            masks: &self.b1_masks,
            map: match (&self.terrain, &fog_state) {
                (Some(terrain), Some(fog)) => Some(MapDraw {
                    terrain,
                    fog: &fog.bind_group,
                }),
                _ => None,
            },
            terrain_pipeline: &self.terrain_pipeline,
            terrain_material: &self.terrain_material,
            terrain_m1: &self.terrain_m1,
            water: &self.water,
            decal_pipeline: &self.decal_pipeline,
            blend: &self.blend,
            feature_decals: match (&self.decals, &self.features) {
                (Some(d), Some(f)) => f.decal_material.as_ref().map(|m| (d, m)),
                _ => None,
            },
            icons: match (&self.decor.icons, &self.decor.icon_material) {
                (Some(d), Some(m)) => Some((d, m)),
                _ => None,
            },
            // The models of the roads take the lines' place (WOB6). B1's lines lie on B1's heights (12 the
            // land, 25 a hill top, plus 1), not on the native ground the frame draws and a mod's eye stands on:
            // from above that is lost in the view, but to a low eye (a mod's camera, `CameraHints::near`) a
            // line over a hill hangs in the sky as a flat brown band (2026-11-07). Such a camera does not
            // draw them.
            lines: if (landmarks_native && self.world.landmark.routes_native())
                || self.cam_hints.near.is_some()
            {
                None
            } else {
                self.decor.lines.as_ref()
            },
            statics: &self.statics,
            models: &self.models,
        };
        let _slow19 = crate::perf::slow::scope("render.plan_adapters");
        let adapters = B1Adapters::new(&b1)
            .with_native_terrain(terrain_native)
            .with_native_water(water_native)
            .with_native_forest(forest_native);
        drop(_slow19);
        // The adapters stand in for the stubs of their ids; the other stubs stay in the plan.
        let mut systems: Vec<&dyn RenderSystem> = adapters.list().to_vec();
        systems.extend(
            self.world
                .list_all()
                .into_iter()
                .filter(|s| adapters.get(s.id()).is_none()),
        );
        let layers = &self.layers;
        // `Overlay` draws the routes in its own pass, so the switch of `Arrows` reaches them here.
        self.world
            .trade
            .set_layer(!layers.is_disabled(SystemId::Trade));
        let _slow20 = crate::perf::slow::scope("render.plan");
        let frame_plan = plan(&systems, &|id| !layers.is_disabled(id));
        drop(_slow20);

        let scene_target = self.targets.get(SCENE).expect("Scene target");
        let scene_msaa = self.targets.get(SCENE_MSAA);
        let depth_target = self.targets.get(SCENE_DEPTH).expect("SceneDepth target");
        let mut overlay = overlay;
        let mut pass: Option<wgpu::RenderPass<'static>> = None;
        let mut reflection_pass: Option<wgpu::RenderPass<'static>> = None;
        for step in &frame_plan.steps {
            let mut started = (self.cpu_clock)();
            let mut counted = (stats.draws, stats.triangles);
            let mut cmds_before = crate::perf::draw::counts();
            match *step {
                Step::PrepareServices => self.world.services.prepare(&self.device, &self.queue),
                Step::Prepare(id) => {
                    if adapters.get(id).is_none()
                        || (id == SystemId::Unit && units_native)
                        || (id == SystemId::Landmark && landmarks_native)
                        || (id == SystemId::City && cities_native)
                    {
                        self.world.prepare_system(
                            id,
                            &self.device,
                            &self.queue,
                            &mut self.world_frame,
                        );
                    }
                }
                // Passes into targets (bake, water, fog mask, overlay, minimap): the subsystem
                // encodes them. No subsystem has a body yet, and B1 draws everything in the main
                // pass; the adapters of B1 have no offscreen pass.
                Step::Offscreen { pass, system } => {
                    if system == crate::frame::SystemId::Shadows {
                        // The casters are the static models, which the renderer owns: it runs
                        // both passes of the subsystem.
                        if self.shadow.active() {
                            let map_span = self.timer.reserve(PassId::ShadowMap);
                            let blur_span = self.timer.reserve(PassId::ShadowBlur);
                            let casters = self.shadow.encode_map(
                                &mut encoder,
                                &self.statics,
                                &mut stats,
                                map_span.map(|s| self.timer.writes(s)),
                                self.shadow_quality.statics_cast(),
                                &|pass, stats| {
                                    let mut tris = Vec::new();
                                    let mut at = stats.triangles;
                                    let mut count = |stats: &mut FrameStats, id| {
                                        add_tris(
                                            &mut tris,
                                            tri_key(TriPass::Shadow, id),
                                            stats.triangles - at,
                                        );
                                        at = stats.triangles;
                                    };
                                    if units_native {
                                        self.world.unit.record_shadow(pass, stats);
                                        count(stats, SystemId::Unit);
                                    }
                                    if landmarks_native {
                                        self.world.landmark.record_shadow(pass, stats);
                                        count(stats, SystemId::Landmark);
                                    }
                                    if cities_native {
                                        self.world.city.record_shadow(pass, stats);
                                        count(stats, SystemId::City);
                                    }
                                    for (k, v) in tris {
                                        add_tris(&mut stats.counters, k, v as u32);
                                    }
                                },
                            );
                            // What the casters drew, by source: the panel tells a pass that only clears
                            // the map from one that draws (`shadow.*`).
                            stats.counters.extend([
                                ("shadow.static_draws", u64::from(casters.statics.draw_calls)),
                                ("shadow.static_tris", casters.statics.triangles),
                                ("shadow.model_draws", u64::from(casters.models.draw_calls)),
                                ("shadow.model_tris", casters.models.triangles),
                            ]);
                            // The step is filed under the slot's pass (`ShadowBlur`): book the
                            // map's own work under `ShadowMap` and let the blur have the rest.
                            let now = (self.cpu_clock)();
                            tally.add(
                                PassId::ShadowMap,
                                now - started,
                                stats.draws - counted.0,
                                stats.triangles - counted.1,
                            );
                            let cmds_now = crate::perf::draw::counts();
                            tally.add_counts(PassId::ShadowMap, cmds_now.since(cmds_before));
                            started = now;
                            counted = (stats.draws, stats.triangles);
                            cmds_before = cmds_now;
                            if self.shadow.blurs() {
                                self.shadow.encode_blur(
                                    &mut encoder,
                                    &mut stats,
                                    blur_span.map(|s| self.timer.writes(s)),
                                );
                            } else if let Some(s) = blur_span {
                                self.timer.release(s);
                            }
                        }
                    } else if adapters.get(system).is_none() {
                        let span = self.timer.reserve(pass);
                        let taken = self.world.offscreen_system(
                            system,
                            pass,
                            &self.device,
                            &self.queue,
                            &mut encoder,
                            &self.targets,
                            &mut stats,
                            span.map(|s| self.timer.writes(s)),
                        );
                        // A subsystem that did not time its pass leaves the queries unwritten.
                        if let (false, Some(span)) = (taken, span) {
                            self.timer.release(span);
                        }
                    }
                }
                Step::BeginPass(PassId::Main) => {
                    let [r, g, b] = self.cam_hints.sky_color.unwrap_or(scene.clear_color);
                    let span = self.timer.reserve(PassId::Main);
                    // With MSAA the pass draws into `SceneMsaa` and resolves into `Scene` when
                    // it ends; with one sample it draws into `Scene` itself.
                    let (colour, resolve) = if self.samples > 1 {
                        (
                            &scene_msaa.expect("SceneMsaa").view,
                            Some(&scene_target.view),
                        )
                    } else {
                        (&scene_target.view, None)
                    };
                    let mut p = encoder
                        .begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("scene"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: colour,
                                depth_slice: None,
                                resolve_target: resolve,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a: 1.0 }),
                                    // The multisampled colour is only needed for the resolve.
                                    store: if resolve.is_some() {
                                        wgpu::StoreOp::Discard
                                    } else {
                                        wgpu::StoreOp::Store
                                    },
                                },
                            })],
                            depth_stencil_attachment: Some(
                                wgpu::RenderPassDepthStencilAttachment {
                                    view: &depth_target.view,
                                    depth_ops: Some(wgpu::Operations {
                                        load: wgpu::LoadOp::Clear(1.0),
                                        store: wgpu::StoreOp::Discard,
                                    }),
                                    stencil_ops: Some(wgpu::Operations {
                                        load: wgpu::LoadOp::Clear(0),
                                        store: wgpu::StoreOp::Discard,
                                    }),
                                },
                            ),
                            timestamp_writes: span.map(|s| self.timer.writes(s)),
                            occlusion_query_set: None,
                            multiview_mask: None,
                        })
                        .forget_lifetime();
                    p.set_pipeline_c(&self.pipeline);
                    p.set_bind_group_c(0, &self.globals_bind_group, &[]);
                    if self.terrain.is_none() && world_drawn {
                        // No world yet: the test grid and the sphere of the showcase.
                        for mesh in [&self.grid, &self.model] {
                            p.set_bind_group_c(1, &mesh.bind_group, &[]);
                            p.set_vertex_buffer(0, mesh.vertices.slice(..));
                            p.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                            p.draw_indexed_c(0..mesh.index_count, 0, 0..1);
                            stats.record_draw(mesh.index_count);
                        }
                    }
                    pass = Some(p);
                }
                Step::EndPass(PassId::Main) => pass = None,
                // The reflection (RC-12): the objects that reflect, from the reflection camera, into
                // `ReflectionColor`. Without the target (`ReflectionLevel` 0) or the camera the
                // pass is not begun and the records below do nothing.
                Step::BeginPass(PassId::Reflection) => {
                    if let (Some(_), Some(colour), Some(depth)) = (
                        reflection_camera.as_ref(),
                        self.targets.get(crate::frame::REFLECTION_COLOR),
                        self.targets.get(crate::frame::REFLECTION_DEPTH),
                    ) {
                        let span = self.timer.reserve(PassId::Reflection);
                        let mut p = encoder
                            .begin_render_pass(&wgpu::RenderPassDescriptor {
                                label: Some("reflection"),
                                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                    view: &colour.view,
                                    depth_slice: None,
                                    resolve_target: None,
                                    ops: wgpu::Operations {
                                        // Black adds nothing to the water: only what stands in the
                                        // picture reflects.
                                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                        store: wgpu::StoreOp::Store,
                                    },
                                })],
                                depth_stencil_attachment: Some(
                                    wgpu::RenderPassDepthStencilAttachment {
                                        view: &depth.view,
                                        depth_ops: Some(wgpu::Operations {
                                            load: wgpu::LoadOp::Clear(1.0),
                                            store: wgpu::StoreOp::Discard,
                                        }),
                                        stencil_ops: Some(wgpu::Operations {
                                            load: wgpu::LoadOp::Clear(0),
                                            store: wgpu::StoreOp::Discard,
                                        }),
                                    },
                                ),
                                timestamp_writes: span.map(|s| self.timer.writes(s)),
                                occlusion_query_set: None,
                                multiview_mask: None,
                            })
                            .forget_lifetime();
                        // The cities of B1 and the figures and objects of `assets-b2`; the trees
                        // follow as the records of their subsystem.
                        b1.draw_reflection(&mut p, &shifts, &mut stats);
                        let at = stats.triangles;
                        if units_native {
                            self.world.unit.record_reflection(&mut p, &mut stats);
                        }
                        let unit_tris = stats.triangles - at;
                        add_tris(
                            &mut stats.counters,
                            tri_key(TriPass::Reflection, SystemId::Unit),
                            unit_tris,
                        );
                        let at = stats.triangles;
                        self.world.landmark.record_reflection(&mut p, &mut stats);
                        let landmark_tris = stats.triangles - at;
                        add_tris(
                            &mut stats.counters,
                            tri_key(TriPass::Reflection, SystemId::Landmark),
                            landmark_tris,
                        );
                        reflection_pass = Some(p);
                    }
                }
                Step::EndPass(PassId::Reflection) => reflection_pass = None,
                Step::BeginPass(_) | Step::EndPass(_) => {}
                Step::Record {
                    pass: PassId::Reflection,
                    slot,
                    system,
                } => {
                    if let Some(p) = reflection_pass.as_mut() {
                        // The adapters of B1 have no reflection slot; the subsystems that do
                        // (the native trees) draw here.
                        let sys = self.world.get(system);
                        let at = stats.triangles;
                        for &shift in &shifts {
                            let mut cx = DrawContext::new(
                                p,
                                &self.globals_bind_group,
                                None,
                                shift,
                                &mut stats,
                                &self.world.services,
                            )
                            .with_terrain(self.world.terrain.sampler());
                            sys.record(slot, &mut cx);
                        }
                        let n = stats.triangles - at;
                        add_tris(&mut stats.counters, tri_key(TriPass::Reflection, system), n);
                    }
                }
                Step::Record {
                    pass: PassId::Main,
                    slot,
                    system,
                } => {
                    // The boot group has not arrived: nothing of the world, only the clear colour.
                    // A group the operator switched off in the panel is not recorded (debug only).
                    let group = crate::perf::Group::of(system);
                    if let (true, Some(p), false) = (
                        world_drawn,
                        pass.as_mut(),
                        crate::perf::groups::is_off(groups_off, group),
                    ) {
                        let sys = adapters
                            .get(system)
                            .unwrap_or_else(|| self.world.get(system));
                        let at = stats.triangles;
                        for &shift in &shifts {
                            let mut cx = DrawContext::new(
                                p,
                                &self.globals_bind_group,
                                None,
                                shift,
                                &mut stats,
                                &self.world.services,
                            )
                            .with_terrain(self.world.terrain.sampler());
                            sys.record(slot, &mut cx);
                            // The units of `assets-b2` after what B1 draws in the slot.
                            if units_native && system == SystemId::Unit {
                                self.world.unit.record(slot, &mut cx);
                            }
                            // The models of the landmarks after B1's roads in the slot.
                            if landmarks_native && system == SystemId::Landmark {
                                self.world.landmark.record(slot, &mut cx);
                            }
                            // The cities of `assets-b2` after B1's models in the slot.
                            if cities_native && system == SystemId::City {
                                self.world.city.record(slot, &mut cx);
                            }
                        }
                        let n = stats.triangles - at;
                        add_tris(&mut stats.counters, tri_key(TriPass::Main, system), n);
                        group_tally.add(group, crate::perf::draw::counts().since(cmds_before));
                    }
                }
                Step::Record { .. } => {}
                // The resolve is the `resolve_target` of the main pass.
                Step::Core(PassId::Resolve) => {
                    if self.debug_shadow && self.shadow.active() {
                        self.shadow
                            .encode_debug(&self.device, &mut encoder, &scene_target.view);
                    }
                }
                Step::Core(PassId::Present) => {
                    let span = self.timer.reserve(PassId::Present);
                    self.present.draw(
                        &self.device,
                        &mut encoder,
                        &scene_target.view,
                        view,
                        scene_target.size != (self.config.width, self.config.height),
                        span.map(|s| self.timer.writes(s)),
                    );
                }
                // The original UI (`ui-render`): over the frame, under egui. An empty list, or one
                // with nothing to draw yet, opens no pass.
                Step::Core(PassId::UiGame) => {
                    let ui_target = UiTarget {
                        format: self.view_format,
                        size: (self.config.width, self.config.height),
                    };
                    let external = UiExternal {
                        minimap: minimap_of(&self.layers, &self.targets),
                    };
                    self.ui_pass.prepare(
                        &self.device,
                        &self.queue,
                        ui,
                        &mut self.ui_textures,
                        &external,
                        &ui_target,
                    );
                    if self.ui_pass.has_work() {
                        let span = self.timer.reserve(PassId::UiGame);
                        let mut p = encoder
                            .begin_render_pass(&wgpu::RenderPassDescriptor {
                                label: Some("ui game"),
                                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                    view,
                                    depth_slice: None,
                                    resolve_target: None,
                                    ops: wgpu::Operations {
                                        load: wgpu::LoadOp::Load,
                                        store: wgpu::StoreOp::Store,
                                    },
                                })],
                                depth_stencil_attachment: None,
                                timestamp_writes: span.map(|s| self.timer.writes(s)),
                                occlusion_query_set: None,
                                multiview_mask: None,
                            })
                            .forget_lifetime();
                        self.ui_pass.draw(&mut p);
                        let (draws, triangles) = self.ui_pass.draw_totals();
                        stats.draws += draws;
                        stats.triangles += triangles;
                    }
                }
                Step::Core(_) => {
                    if let Some(overlay) = overlay.take() {
                        let minimap = self.minimap_view();
                        overlay.offer(&OverlayTextures {
                            minimap: minimap.map(|(v, _)| v),
                            minimap_info: minimap.map(|(_, i)| i),
                        });
                        let span = self.timer.reserve(PassId::Ui);
                        let target = OverlayTarget {
                            format: self.view_format,
                            size: (self.config.width, self.config.height),
                            scale: self.scale,
                        };
                        run_overlay(
                            overlay,
                            &self.device,
                            &self.queue,
                            &mut encoder,
                            view,
                            &target,
                            span.map(|s| self.timer.writes(s)),
                        );
                    }
                }
            }
            let step_ms = (self.cpu_clock)() - started;
            crate::perf::slow::record(step_label(step), step_ms);
            tally.add(
                step.pass(),
                step_ms,
                stats.draws - counted.0,
                stats.triangles - counted.1,
            );
            tally.add_counts(step.pass(), crate::perf::draw::counts().since(cmds_before));
        }
        if world_drawn
            && self
                .requests
                .iter()
                .any(|r| r.what() == FrameSource::ClassMask)
        {
            let started = (self.cpu_clock)();
            if let (Some(color), Some(depth)) = (
                self.targets.get(CLASS_MASK),
                self.targets.get(CLASS_MASK_DEPTH),
            ) {
                let mut p = crate::frame::mask::begin_pass(&mut encoder, &color.view, &depth.view);
                for step in &frame_plan.steps {
                    let Step::Record {
                        pass: PassId::Main,
                        slot,
                        system,
                    } = *step
                    else {
                        continue;
                    };
                    let adapter = adapters.get(system);
                    let sys = adapter.unwrap_or_else(|| self.world.get(system));
                    // B1's water in the plan has no class mask of its own: B1's mask stands in. The
                    // B2 water writes its class itself.
                    let b1_water = (system == crate::frame::SystemId::Water && adapter.is_some())
                        .then(|| adapters.water_mask())
                        .flatten();
                    // The models of `assets-b2` after the adapter of their slot, as in the main pass.
                    let native: Option<&dyn RenderSystem> = match system {
                        SystemId::Unit if units_native => Some(&self.world.unit),
                        SystemId::Landmark if landmarks_native => Some(&self.world.landmark),
                        SystemId::City if cities_native && adapter.is_some() => {
                            Some(&self.world.city)
                        }
                        _ => None,
                    };
                    for &shift in &shifts {
                        for sys in std::iter::once(sys).chain(b1_water).chain(native) {
                            sys.record_mask(
                                slot,
                                &mut MaskContext::new(
                                    &mut p,
                                    &self.globals_bind_group,
                                    shift,
                                    &self.world.services,
                                ),
                            );
                        }
                    }
                }
            }
            tally.add(PassId::ClassMask, (self.cpu_clock)() - started, 0, 0);
        }
        // Frames asked for with `read_frame`: copied now, mapped after the submit.
        for request in std::mem::take(&mut self.requests) {
            match request.what() {
                FrameSource::World => request.encode(
                    &self.device,
                    &mut encoder,
                    &scene_target.texture,
                    scene_target.size,
                    WORLD_FORMAT,
                ),
                FrameSource::ClassMask => match self.targets.get(CLASS_MASK) {
                    Some(t) => {
                        request.encode(&self.device, &mut encoder, &t.texture, t.size, MASK_FORMAT)
                    }
                    None => request.fail("the mask target does not exist yet"),
                },
                FrameSource::WithUi => match &self.output {
                    Some(texture) => request.encode(
                        &self.device,
                        &mut encoder,
                        texture,
                        (self.config.width, self.config.height),
                        self.view_format,
                    ),
                    None => request.fail("this frame has no copyable output (no COPY_SRC)"),
                },
                FrameSource::Target(id) => match (self.targets.get(id), self.targets.desc_of(id)) {
                    (Some(t), Some(desc)) if desc.samples == 1 => {
                        request.encode(&self.device, &mut encoder, &t.texture, t.size, desc.format)
                    }
                    _ => request.fail(format!("target {} is not there or is multisampled", id.0)),
                },
            }
            self.in_flight.push(request);
        }
        self.timer.end_frame(&mut encoder);
        stats.passes = tally.finish(&self.timer);
        stats.groups = group_tally;
        let layers = &self.layers;
        // The triangles by system the passes counted (`tris.*`) go after the subsystems' counters.
        let tris = std::mem::replace(
            &mut stats.counters,
            self.world.counters(&|id| !layers.is_disabled(id)),
        );
        stats.counters.extend(tris);
        // The clip cache (`anim.baked_total`, `anim.failed_total`, ...), whatever the layers.
        stats
            .counters
            .extend(self.world.services.anim.stats().counters());
        stats.ui = self.ui_pass.stats();
        stats.counters.extend(stats.ui.counters());
        stats.ui_times = ui_times(&stats.passes);
        {
            let (built, deferred, checked) = self.world.city.layout_counters();
            let (lm_checked, lm_deferred) = self.world.landmark.budget_counters();
            let puffs = self.world.fluid_fow.puff_stats();
            stats.counters.extend([
                ("city.layouts_built", built),
                ("city.layouts_deferred", deferred),
                ("city.ground_checks", checked),
                ("city.layout_ms", self.city_ms as u64),
                ("landmark.ground_checks", lm_checked),
                ("landmark.deferred", lm_deferred),
                ("landmark.refresh_ms", self.landmark_ms as u64),
                ("forest.rebuilds", self.forest_rebuilds),
                ("forest.rebuilds_skipped", self.forest_skipped),
                ("heights.cells_reported", self.height_cells_checked),
                ("city.instances_reused", self.world.city.instances_reused()),
                (
                    "landmark.instances_reused",
                    self.world.landmark.instances_reused(),
                ),
                ("forest.rebuild_ms", self.forest_ms as u64),
                ("forest.rebuild_us", (self.forest_ms * 1000.0) as u64),
                (
                    "forest.blocks_dirty",
                    self.world.forest.dirty_blocks() as u64,
                ),
                ("forest.view_pending", self.forest_view_pending),
                ("forest.background", self.forest_background),
                ("forest.blocks_rebuilt", self.forest_blocks_rebuilt),
                ("fow.puffs_steps", puffs.steps),
                ("fow.puffs_skipped", puffs.skipped),
                ("fow.puffs_uploads", puffs.uploads),
            ]);
        }
        stats.objects = self.object_stats();
        self.frame_stats = stats;
        encoder.finish()
    }
}

/// The minimap target, its size and generation; `None` while it does not exist or `Minimap` is off.
/// The rectangle of the original's ground the shadow and reflection maps cover: what the view frustum cuts
/// out of it (`fallback`, the half side of a square around the eye, when it cuts none), or for a camera that
/// asks for a range (`CameraHints::shadow_range`) the square of that half side around the eye.
fn view_rect(
    camera: &dyn CameraView,
    aspect: f32,
    ours_to_orig: &Mat4,
    fallback: f32,
) -> (Vec3, Vec3) {
    match camera.hints().shadow_range {
        Some(half) => fixed_rect(camera.eye(), ours_to_orig, half),
        None => visible_rect(
            &camera.view_proj(aspect),
            camera.eye(),
            ours_to_orig,
            fallback,
        ),
    }
}

fn minimap_of<'a>(
    layers: &LayerSwitches,
    targets: &'a crate::frame::TargetPool,
) -> Option<(&'a wgpu::TextureView, MinimapInfo)> {
    if layers.is_disabled(crate::frame::SystemId::Minimap) {
        return None;
    }
    let t = targets.get(MINIMAP)?;
    Some((
        &t.view,
        MinimapInfo {
            size: t.size,
            generation: t.generation,
        },
    ))
}

/// `ui.cpu_ms`, `ui.gpu_ms` and `ui.overlay_ms` from the rows of the pass table.
fn ui_times(passes: &[PassStat]) -> UiTimes {
    let row = |pass| passes.iter().find(|p| p.pass == pass);
    let game = row(PassId::UiGame);
    UiTimes {
        cpu_ms: game.map_or(0.0, |p| p.cpu_ms),
        gpu_ms: game.and_then(|p| p.gpu_ms),
        overlay_ms: row(PassId::Ui).map_or(0.0, PassStat::shown_ms),
    }
}

/// The name a plan step goes under in the slow-frame record (`prepare.city`, `record.terrain`).
/// Whether the map is loading, and the generation whose first full view has been drawn: a new map
/// (another generation) loads again.
fn map_loading_of(ready: Option<u32>, generation: u32, view_ready: bool) -> (bool, Option<u32>) {
    if ready == Some(generation) {
        (false, ready)
    } else if view_ready {
        (false, Some(generation))
    } else {
        (true, ready)
    }
}

fn step_label(step: &Step) -> &'static str {
    match *step {
        Step::PrepareServices => "step.prepare_services",
        Step::Prepare(id) => match id {
            SystemId::Terrain => "prepare.terrain",
            SystemId::Decal => "prepare.decal",
            SystemId::Water => "prepare.water",
            SystemId::River => "prepare.river",
            SystemId::Ice => "prepare.ice",
            SystemId::Waves => "prepare.waves",
            SystemId::City => "prepare.city",
            SystemId::Arrows => "prepare.arrows",
            SystemId::Trade => "prepare.trade",
            SystemId::Forest => "prepare.forest",
            SystemId::Landmark => "prepare.landmark",
            SystemId::Unit => "prepare.unit",
            SystemId::Particle => "prepare.particle",
            SystemId::FowClassic => "prepare.fowclassic",
            SystemId::FluidFow => "prepare.fluidfow",
            SystemId::Overlay => "prepare.overlay",
            SystemId::MovementPath => "prepare.movementpath",
            SystemId::Minimap => "prepare.minimap",
            SystemId::YieldIcons => "prepare.yieldicons",
            SystemId::DistanceFog => "prepare.distancefog",
            SystemId::Shadows => "prepare.shadows",
            SystemId::Mod(_) => "prepare.mod",
        },
        Step::Offscreen { system, .. } => match system {
            SystemId::Terrain => "offscreen.terrain",
            SystemId::Decal => "offscreen.decal",
            SystemId::Water => "offscreen.water",
            SystemId::River => "offscreen.river",
            SystemId::Ice => "offscreen.ice",
            SystemId::Waves => "offscreen.waves",
            SystemId::City => "offscreen.city",
            SystemId::Arrows => "offscreen.arrows",
            SystemId::Trade => "offscreen.trade",
            SystemId::Forest => "offscreen.forest",
            SystemId::Landmark => "offscreen.landmark",
            SystemId::Unit => "offscreen.unit",
            SystemId::Particle => "offscreen.particle",
            SystemId::FowClassic => "offscreen.fowclassic",
            SystemId::FluidFow => "offscreen.fluidfow",
            SystemId::Overlay => "offscreen.overlay",
            SystemId::MovementPath => "offscreen.movementpath",
            SystemId::Minimap => "offscreen.minimap",
            SystemId::YieldIcons => "offscreen.yieldicons",
            SystemId::DistanceFog => "offscreen.distancefog",
            SystemId::Shadows => "offscreen.shadows",
            SystemId::Mod(_) => "offscreen.mod",
        },
        Step::Record { system, .. } => match system {
            SystemId::Terrain => "record.terrain",
            SystemId::Decal => "record.decal",
            SystemId::Water => "record.water",
            SystemId::River => "record.river",
            SystemId::Ice => "record.ice",
            SystemId::Waves => "record.waves",
            SystemId::City => "record.city",
            SystemId::Arrows => "record.arrows",
            SystemId::Trade => "record.trade",
            SystemId::Forest => "record.forest",
            SystemId::Landmark => "record.landmark",
            SystemId::Unit => "record.unit",
            SystemId::Particle => "record.particle",
            SystemId::FowClassic => "record.fowclassic",
            SystemId::FluidFow => "record.fluidfow",
            SystemId::Overlay => "record.overlay",
            SystemId::MovementPath => "record.movementpath",
            SystemId::Minimap => "record.minimap",
            SystemId::YieldIcons => "record.yieldicons",
            SystemId::DistanceFog => "record.distancefog",
            SystemId::Shadows => "record.shadows",
            SystemId::Mod(_) => "record.mod",
        },
        Step::BeginPass(_) | Step::EndPass(_) => "step.pass_edge",
        Step::Core(_) => "step.core",
    }
}

fn upload_mesh(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    label: &str,
    mesh: &Mesh,
    uniform: ObjectUniform,
) -> GpuMesh {
    let vertices = device.buf_init(
        &wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        },
        Cluster::Models,
    );
    let indices = device.buf_init(
        &wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        },
        Cluster::Models,
    );
    let uniform = device.buf_init(
        &wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        },
        Cluster::Other,
    );
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform.as_entire_binding(),
        }],
    });
    GpuMesh {
        vertices,
        indices,
        index_count: mesh.indices.len() as u32,
        uniform,
        bind_group,
    }
}

fn backend_info(adapter: &wgpu::Adapter) -> BackendInfo {
    describe_backend(adapter.get_info(), adapter.features())
}

fn describe_backend(ai: wgpu::AdapterInfo, f: wgpu::Features) -> BackendInfo {
    let kind = match ai.backend {
        wgpu::Backend::BrowserWebGpu => BackendKind::WebGpu,
        wgpu::Backend::Gl if cfg!(target_arch = "wasm32") => BackendKind::WebGl2,
        wgpu::Backend::Gl => BackendKind::Gl,
        wgpu::Backend::Vulkan => BackendKind::Vulkan,
        wgpu::Backend::Dx12 => BackendKind::Dx12,
        wgpu::Backend::Metal => BackendKind::Metal,
        _ => BackendKind::Other,
    };
    BackendInfo {
        kind,
        adapter: ai.name,
        driver: format!("{} {}", ai.driver, ai.driver_info)
            .trim()
            .to_owned(),
        compression: TextureCompression {
            bc: f.contains(wgpu::Features::TEXTURE_COMPRESSION_BC),
            etc2: f.contains(wgpu::Features::TEXTURE_COMPRESSION_ETC2),
            astc: f.contains(wgpu::Features::TEXTURE_COMPRESSION_ASTC),
        },
    }
}

#[cfg(test)]
mod loading_tests {
    use super::map_loading_of;

    #[test]
    fn a_new_map_loads_again() {
        // The first map: loading until its view is ready, then not.
        let (l, r) = map_loading_of(None, 1, false);
        assert!(l && r.is_none());
        let (l, r) = map_loading_of(r, 1, true);
        assert!(!l && r == Some(1));
        // The view may stop being ready (the camera moves) without loading again.
        let (l, r) = map_loading_of(r, 1, false);
        assert!(!l && r == Some(1));
        // Another map: loading again until its own first full view.
        let (l, r) = map_loading_of(r, 2, false);
        assert!(l && r == Some(1));
        let (l, r) = map_loading_of(r, 2, true);
        assert!(!l && r == Some(2));
    }
}
