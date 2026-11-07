//! The terrain through the cache, live: what ties the cells, the
//! cache, the bake and the draw to a frame. `Terrain::live_frame` does the CPU work of a frame (the
//! cells the field has finished, the visible cells and the pages they ask for, the tiles of heights, the
//! draw list); the bake and the draw record into the passes the frame plan gives them
//! (`record_offscreen(TerrainBake)` and `record(Main.Terrain)`).
//!
//! Two parts. [`LiveGpu`] stays while the device and the formats do: the pipelines of the bake and of the
//! draw, and the textures of `TerrainDef` made from the library. [`LiveMap`] is made for a map (a basis,
//! a library): the cells, the cache with its pools of pages, the tiles of heights, the textures of the
//! screen. A key that changes the pages (`on_settings`) or a new map makes a new `LiveMap`; the field and
//! the build do not change with it.
//!
//! A cell is built in the field first (`Terrain::pump`). It is *ready* for the cache when it and the
//! eight cells around it are built: the normals and the heights of its border come from them. Until
//! then its statistics say "no ground" and the cache does not ask for a page.

use crate::perf::{Cluster, DeviceTrack};
use std::collections::{HashMap, VecDeque};

use asset_format::LogicalId;
use glam::{Mat4, Vec2, Vec3};

use super::bake::{BakeConfig, BakeLight, BakeScene, DefTexture, TerrainBaker, bake_budget};
use super::cache::{CellView, Rebake, TerrainCache};
use super::cells::{BlendKernel, CellGrid, CellStats, Projections, StatsClass, TerrainCells};
use super::decal_pass::DecalBake;
use super::decals::DecalLayer;
use super::draw::{
    CellDrawer, DrawCell, FogBridge, FrameDraw, LiveConfig, LiveLights, fog_mask_bytes,
    fog_mask_size,
};
use super::heights_gpu::HeightTiles;
use super::library::TerrainLibrary;
use super::pages::{PageArrays, layers_per_array_for};
use super::settings_map::TerrainSettings;
use super::shadow_pass::ShadowBake;
use crate::terrain::Fog;

/// The size of the cache pages (`g_iCacheSize`). The pool of 64 is the largest one at the High preset:
/// the cells far from the camera have the small page, the near ones a second, larger one.
// TODO(RW-5): the original's `g_iCacheSize` by `TerrainDetailLevel`
pub const CACHE_PAGE_SIZE: u32 = 64;
/// Cells of the field the frame finishes at most (`Terrain::pump`).
pub const FIELD_CELLS_PER_FRAME: usize = 16;
/// CPU time the field may build in a frame: past [FIELD_CELLS_PER_FRAME] cells it goes on while the
/// frame's share lasts, up to [FIELD_CELLS_MAX_PER_FRAME] (the build of a huge map that is all open
/// is ten thousand cells: at sixteen a frame it took as many frames as the load had, and the terrain
/// was not done long after the map was shown).
pub const FIELD_FRAME_MS: f64 = 8.0;
/// CPU time a frame may give to the build of a basis in pieces ([`super::Terrain::step_job`]); a
/// stage or a cell of the prior field is not cut, so a frame can go over by that.
pub const BASIS_JOB_MS: f64 = 8.0;
/// CPU time the field may build in a frame for cells in view once a view was complete (hexes a unit
/// opened): not the load, the frame is held to a few milliseconds.
pub const FIELD_VIEW_MS: f64 = 3.0;
/// Cells of the field a frame builds at most.
pub const FIELD_CELLS_MAX_PER_FRAME: usize = 96;
/// CPU time the field may take in a frame once the view and its ring are built: the rest of the map is
/// built in the background, at least one cell a frame (the frame of the target is 6.9 ms in all).
pub const FIELD_BACKGROUND_MS: f64 = 1.5;
/// Cells a frame builds in the background without a clock.
pub const FIELD_BACKGROUND_CELLS: usize = 2;
/// CPU time the field may take in a frame while only cells of the ring around the view wait for it, and
/// that the cells of the ring the field has built take to be made ready.
pub const FIELD_RING_MS: f64 = 2.0;
/// The CPU time of a frame the background of the field fills up to (the frame of the target is 6.9 ms):
/// the field and the cells it built take what the rest of the frame leaves, within
/// [`FIELD_SLACK_MIN_MS`] and [`FIELD_SLACK_MAX_MS`].
pub const FRAME_FILL_MS: f64 = 5.5;
pub const FIELD_SLACK_MIN_MS: f64 = 2.0;
pub const FIELD_SLACK_MAX_MS: f64 = 4.0;
/// Cells that are made ready (statistics, tile of heights) in a frame at most.
pub const READY_PER_FRAME: usize = 24;
/// What a drawn cell costs of the share of a frame ([READY_COST_PER_FRAME]); a cell that is not drawn
/// costs one.
const READY_COST_DRAWN: usize = 4;
/// The share of a frame: [READY_PER_FRAME] drawn cells.
const READY_COST_PER_FRAME: usize = READY_PER_FRAME * READY_COST_DRAWN;
/// The share of a frame for the cells out of view: two drawn cells.
/// CPU time a frame gives the background work of the terrain on average (making the cells out of view
/// ready, the normals and the bakes of their base pages); it saves up at most [`BACKGROUND_CREDIT_MS`].
pub const BACKGROUND_MS: f64 = 0.7;
const BACKGROUND_CREDIT_MS: f64 = 4.0 * BACKGROUND_MS;
/// The frame of the target: past it the background does nothing that frame.
pub const FRAME_TARGET_MS: f32 = 6.9;
/// Rows of a block of normals between two readings of the clock.
const BACKGROUND_ROWS: i32 = 4;
/// What the CPU part of a background bake costs before one is measured.
const BACKGROUND_PAGE_MS: f64 = 0.6;
/// Pages baked per frame while the first load runs (the loader of the original takes four cells a
/// call, `NonBlockingTextureInit`).
pub const LOAD_BAKES_PER_FRAME: usize = 8;
/// Calls of the loader of the first frame a frame makes.
pub const LOAD_STEPS_PER_FRAME: usize = 4;
/// GPU time the page bakes of a frame may take, when the device times its passes: the count of the
/// budget is cut to the pages that fit (one at least). Eight pages at once on a slow GPU (a big page,
/// the shadow pass) are a long frame every frame for as long as grey cells are on screen.
pub const BAKE_FRAME_MS: f32 = 4.0;
/// Frames between a bake and its GPU time (the ring of the timer is two or three frames deep).
const BAKE_TIMING_LAG: usize = 3;
/// CPU time the page bakes of a frame may take while only the ring around the view has cells without
/// a page (the view is drawn): the vertices and the normals of a page are a millisecond or more, and the
/// full budget of a frame for the ring of a far pan was 30 ms of bake a frame.
pub const RING_BAKE_MS: f32 = 3.0;
/// CPU time the page bakes of a frame may take at most: past it only the cache pages of the cells with
/// nothing to draw are baked (no hole), the others wait drawn from the page they have (old or coarser).
/// A frame of a far pan that baked eight cells with their near and highlight pages took 30 to 70 ms.
pub const BAKE_CPU_MS: f64 = 5.0;

/// The pages of a count that fit [`BAKE_FRAME_MS`] at `ms_per_page` (0: not measured, the count).
pub fn time_bounded(count: usize, ms_per_page: f32) -> usize {
    if ms_per_page <= 0.0 || !ms_per_page.is_finite() {
        return count;
    }
    let fit = (BAKE_FRAME_MS / ms_per_page).floor() as usize;
    count.min(fit.max(1))
}

/// The view of a frame, in the original's basis.
#[derive(Clone, Debug)]
pub struct LiveView {
    /// `P_ours * orig_to_ours`: the plain matrix, the clip depth 0 to 1.
    pub view_proj_orig: Mat4,
    pub eye_orig: Vec3,
    /// Width of the screen in pixels.
    pub screen_width: f32,
    /// X shifts of the copies of the map in our basis, the middle one first.
    pub shifts: Vec<f32>,
    /// A camera that can stand in the boxes of the cells (client-hooks T27b): its near plane. The width of a
    /// cell on the screen is then measured on its box cut at that plane, and the pages under the eye get
    /// their full size. `None` for the strategic camera: the widths are the original's.
    pub near: Option<f32>,
    /// The point (our X and Z) the pages stream around instead of the middle of the screen.
    pub focus: Option<Vec2>,
}

/// What a frame brings in.
pub struct LiveInputs<'a> {
    pub lights: &'a LiveLights,
    pub frame_globals: &'a wgpu::Buffer,
    pub shadow_map: &'a wgpu::TextureView,
    pub view: LiveView,
    /// The decals are baked into the pages (`[DisableSystems] Decal` turns it off).
    pub decals: bool,
    /// The newest GPU time of the bake pass (`TerrainBake`), when the device times its passes.
    pub bake_gpu_ms: Option<f32>,
    /// The fluid fog's mask and base layer, when the fog draws its clouds.
    pub fow: Option<super::draw::CellFow<'a>>,
    /// Milliseconds of a monotonic clock (the renderer's CPU clock), for the share of the frame the
    /// field may build in ([FIELD_FRAME_MS]); None or a clock that does not move: the count only.
    pub clock: Option<&'a dyn Fn() -> f64>,
    /// CPU time of the last frame, and of its bake pass (`TerrainBake`): the background work of the
    /// terrain stops when the frame is over [`FRAME_TARGET_MS`], and learns what a page costs.
    pub frame_cpu_ms: Option<f32>,
    pub bake_cpu_ms: Option<f32>,
}

/// What the pages and the draw of a map cost, for `?stats=1` and the tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LiveStats {
    /// Cells the draw list has this frame (over all the copies).
    pub drawn: usize,
    /// Pages baked so far.
    pub baked: u64,
    /// Jobs of the bake that failed (a cell stays grey).
    pub failed: u64,
    /// Cells ready for the cache.
    pub ready: usize,
    /// Distinct cells in view this frame, and of them the ones with no page to draw yet (state 0,
    /// grey in the original): `terrain-scroll` (AC-R1.5).
    pub visible: usize,
    pub unpaged: usize,
    /// Open cells in view the field has not made ready yet: they are not drawn either.
    pub unbuilt: usize,
    /// The sum of `unpaged` over the frames, and the frames with one at least (`terrain.holes_total`,
    /// `terrain.hole_frames_total`).
    pub holes_total: u64,
    pub hole_frames: u64,
    /// The pages the bake may take this frame.
    pub budget: usize,
    /// Cells of the ring around the view without a page (baked before they scroll in).
    pub ring_unpaged: usize,
    /// Open cells of the ring around the view the field has not made ready yet.
    pub ring_unbuilt: usize,
    /// Cells whose statistics were made (again) so far, and of them the ones whose textures were made
    /// too (a drawn cell; the contributions read a block of normals): `terrain.cells.recomputed_total`,
    /// `terrain.cells.textured_total`.
    pub recomputed: u64,
    pub textured: u64,
    /// Cells built in the field that are not made ready yet (a neighbour is not built, or the share of
    /// the frame is spent): `terrain.cells.waiting`.
    pub waiting: usize,
}

/// Frames ahead the view is guessed at for the order of the jobs: the cells the camera moves towards are
/// baked before the ones it leaves.
const LOOKAHEAD_FRAMES: f32 = 12.0;
/// The farthest the guess moves the middle of the view, in clip units.
const LOOKAHEAD_MAX: f32 = 0.6;
/// How much wider than the view the ring of cells is that get their cache page before they are seen.
const RING_WIDEN: f32 = 1.5;
/// How far the ring reaches ahead of a moving camera: the widened view moved by this many times the
/// guess of where the view goes ([`LOOKAHEAD_FRAMES`] ahead).
const RING_AHEAD: f32 = 2.0;
/// Cells of the order of the background a frame looks at, at most.
const BACKGROUND_SCAN: usize = 512;
/// How far (in cells) the camera moves before the order of the background is made again.
pub const BACKGROUND_REFOCUS: i32 = 4;

/// The pipelines and the textures of `TerrainDef`: they stay while the library and the formats do.
pub struct LiveGpu {
    pub cfg: LiveConfig,
    pub baker: TerrainBaker,
    pub drawer: CellDrawer,
    /// The textures of the rows of `TerrainDef`, by the index of the table, and the epoch of the library
    /// they were made from.
    pub defs: Vec<DefTexture>,
    pub defs_epoch: u64,
    /// The textures of the decals on the GPU, by id; taken from the library when a decal needs one.
    pub decal_textures: HashMap<LogicalId, wgpu::TextureView>,
    decal_seen: (u64, u64),
}

fn placeholder(device: &wgpu::Device, queue: &wgpu::Queue, rgba: [u8; 4]) -> wgpu::TextureView {
    let t = device.tex(
        &wgpu::TextureDescriptor {
            label: Some("terrain placeholder"),
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
    );
    queue.write_texture(
        t.as_image_copy(),
        &rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    t.create_view(&wgpu::TextureViewDescriptor::default())
}

/// The textures of every row of the table, from the library. A row without a texture, or whose
/// texture is not in the library (a style the map does not use), gets a placeholder that nothing reads.
pub fn make_defs(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    library: &TerrainLibrary,
) -> Vec<DefTexture> {
    let upload = |blob: &asset_format::TextureBlob, label: &str| match crate::texture::upload_blob(
        device,
        queue,
        blob,
        label,
        crate::texture::UploadOptions::WORLD,
    ) {
        Ok(u) => Some(
            u.texture
                .create_view(&wgpu::TextureViewDescriptor::default()),
        ),
        Err(why) => {
            tracing::warn!("terrain texture {label} could not be uploaded: {why}");
            None
        }
    };
    library
        .defs()
        .rows()
        .iter()
        .map(|row| {
            let t = library.textures_of(row);
            let label = row
                .diffuse
                .as_ref()
                .map_or("terrain row", |d| d.as_str())
                .to_owned();
            DefTexture {
                albedo: t
                    .diffuse
                    .and_then(|b| upload(b, &label))
                    .unwrap_or_else(|| placeholder(device, queue, [128, 128, 128, 255])),
                height: t
                    .height
                    .and_then(|b| upload(b, &label))
                    .unwrap_or_else(|| placeholder(device, queue, [128, 128, 128, 255])),
                spec: t.specular.and_then(|b| upload(b, &label)),
            }
        })
        .collect()
}

impl LiveGpu {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cfg: &LiveConfig,
        library: &TerrainLibrary,
        epoch: u64,
    ) -> Self {
        Self {
            cfg: *cfg,
            baker: TerrainBaker::new(
                device,
                BakeConfig {
                    float_targets: cfg.float_targets,
                    height_scale: super::bake::MAKE_NORMAL_HEIGHT_SCALE,
                },
            ),
            drawer: CellDrawer::new(device, cfg),
            defs: make_defs(device, queue, library),
            defs_epoch: epoch,
            decal_textures: HashMap::new(),
            decal_seen: (u64::MAX, u64::MAX),
        }
    }

    /// Uploads the textures the decals of the layer name and the library holds (once each). A texture
    /// that arrives later is picked up when the library changes.
    pub fn ensure_decal_textures(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        library: &TerrainLibrary,
        layer: &DecalLayer,
        epoch: u64,
    ) {
        if self.decal_seen == (layer.revision(), epoch) {
            return;
        }
        self.decal_seen = (layer.revision(), epoch);
        for d in layer.iter() {
            for id in [&d.albedo, &d.height].into_iter().flatten() {
                if self.decal_textures.contains_key(id) {
                    continue;
                }
                let Some(blob) = library.texture(id) else {
                    continue;
                };
                let _s = crate::perf::slow::scope("bake.decal_upload");
                match crate::texture::upload_blob(
                    device,
                    queue,
                    blob,
                    id.as_str(),
                    crate::texture::UploadOptions::WORLD,
                ) {
                    Ok(u) => {
                        self.decal_textures.insert(
                            id.clone(),
                            u.texture
                                .create_view(&wgpu::TextureViewDescriptor::default()),
                        );
                    }
                    Err(why) => {
                        tracing::warn!(
                            "decal texture {} could not be uploaded: {why}",
                            id.as_str()
                        );
                    }
                }
            }
        }
    }
}

/// The map-bound part.
pub struct LiveMap {
    pub generation: u32,
    pub library_epoch: u64,
    pub settings: TerrainSettings,
    projections: Projections,
    kernel: BlendKernel,
    grid: CellGrid,
    pub cells: TerrainCells,
    /// The revision of each cell of the field this saw.
    seen_rev: Vec<u32>,
    ready: Vec<bool>,
    dirty: VecDeque<usize>,
    in_dirty: Vec<bool>,
    /// The ready cells whose statistics, textures and pages a decal changed, each once, made again a
    /// few a frame ([`READY_PER_FRAME`]): a decal over a cell is no reason to read its 69 x 69
    /// normals again in the frame it came, and with a thousand decals a frame that did took seconds.
    stale: VecDeque<usize>,
    in_stale: Vec<bool>,
    pub cache: TerrainCache,
    arrays: PageArrays,
    heights: HeightTiles,
    fog: FogBridge,
    fog_epoch: u64,
    lights: Option<LiveLights>,
    last_view: Option<Mat4>,
    moving: bool,
    init_done: bool,
    decals_on: bool,
    pub stats: LiveStats,
    /// The open cells in view and in the ring of the last frame that are not ready yet: the field builds
    /// them (and the cells around them) first, with the share of a frame of the load.
    pub(super) unbuilt_cells: Vec<usize>,
    /// The cell nearest the middle of the screen in the last frame: the background work goes out from
    /// it.
    pub(super) focus: Option<usize>,
    /// Where the view goes (clip units, [`LOOKAHEAD_FRAMES`] ahead), from the last frame: the ring
    /// reaches further that way.
    ahead: Vec2,
    /// The cells nearest first from [`Self::bg_focus`], for the base pages baked in the background, and
    /// how far along it the cells are all served.
    bg_order: Vec<usize>,
    bg_cursor: usize,
    bg_focus: Option<usize>,
    bg: Background,
    /// The groups of the draw this map's textures go into: one number per live map, never reused, so
    /// the drawer (which outlives the map) forgets the groups of the map before. With one number for
    /// every map the groups of the old height tiles and page arrays stayed bound: a tile whose group was
    /// made before a rebuild (a basis again, a texture of the library that came late) drew the old
    /// heights next to a tile with the new ones, a long straight step at the tile border.
    draw_generation: u64,
    /// Pages baked in the last frames (a ring, by frame), and the GPU time of a page they gave.
    baked_ring: [usize; BAKE_TIMING_LAG + 1],
    frame_no: usize,
    ms_per_page: f32,
    /// The culling of the last view, and what it was made from.
    cull: cull::Cull,
    blocks: cull::Blocks,
    /// Bumped where a cell becomes ready, or its openness or class may change: the culling is made again.
    cull_epoch: u64,
    /// Per cell: the stamp of the frame whose draws had the cell.
    paged: Vec<u32>,
    paged_stamp: u32,
    /// The CPU time the cells the field built out of view may take to be made ready in this frame (what
    /// the field left of the background's share, `Terrain::live_frame`), and what they took.
    pub(super) ready_share_ms: f64,
    pub(super) ready_spent_ms: f64,
    /// Pages the last frame baked, and the CPU time of a page the bake pass gave (0: not measured).
    last_baked: usize,
    cpu_ms_per_page: f32,
    /// A view was complete once ([`Self::view_ready`]): the load is over.
    pub(super) was_ready: bool,
}

/// The background work of a live map ([`LiveMap::background_step`]): a credit of CPU time, the unit
/// of work in hand and what a base page costs to bake.
#[derive(Default)]
struct Background {
    credit: f64,
    /// The CPU time of a background page bake (the pass's time over its pages, when only background
    /// pages were baked); 0: not measured yet.
    page_ms: f64,
    /// The block of normals being made for a unit, and the unit.
    prep: Option<(Unit, super::sampler::NormalsInProgress)>,
    /// A base page whose normals are made, waiting for the credit of its bake.
    bake_next: Option<usize>,
    /// Background pages asked for in the last frame, and whether the view had jobs then.
    pushed: usize,
    view_jobs: bool,
    baked_seen: u64,
    /// Spent so far (`terrain.background.ms_total`).
    spent_total: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Unit {
    /// Make a cell out of view ready (its contributions read the block).
    Ready(usize),
    /// Bake the base page of a cell out of view (its vertices read the block).
    Bake(usize),
}

/// The source of [`LiveMap::draw_generation`].
static NEXT_DRAW_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The eight cells around `(cx, cy)` and itself that exist.
fn around(cx: i32, cy: i32, cells_x: i32, cells_y: i32) -> impl Iterator<Item = (i32, i32)> {
    (-1..=1).flat_map(move |dy| {
        (-1..=1).filter_map(move |dx| {
            let (x, y) = (cx + dx, cy + dy);
            ((0..cells_x).contains(&x) && (0..cells_y).contains(&y)).then_some((x, y))
        })
    })
}

impl LiveMap {
    pub(super) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        map: &super::MapState,
        library: &TerrainLibrary,
        settings: &TerrainSettings,
        generation: u32,
        library_epoch: u64,
    ) -> Self {
        let frame = *map.sampler.frame();
        let grid = CellGrid::new(&frame);
        let mut cells = TerrainCells::new(&frame, &map.basis);
        // Nothing is ground until the cell is ready.
        for i in 0..cells.len() {
            cells.cell_mut(i).stats = CellStats {
                class: StatsClass::Empty,
                ..CellStats::default()
            };
        }
        let n = cells.len();
        let mut me = Self {
            generation,
            library_epoch,
            settings: settings.clone(),
            projections: Projections::from_defs(library.defs().rows()),
            kernel: BlendKernel::new(),
            grid,
            cells,
            seen_rev: vec![0; n],
            ready: vec![false; n],
            dirty: VecDeque::new(),
            in_dirty: vec![false; n],
            stale: VecDeque::new(),
            in_stale: vec![false; n],
            cache: TerrainCache::new(n, settings, CACHE_PAGE_SIZE, true),
            arrays: PageArrays::new(layers_per_array_for(device)),
            heights: HeightTiles::new(),
            fog: FogBridge::new(device, queue, fog_mask_size(&frame)),
            fog_epoch: u64::MAX,
            lights: None,
            last_view: None,
            moving: false,
            init_done: false,
            decals_on: true,
            stats: LiveStats::default(),
            unbuilt_cells: Vec::new(),
            focus: None,
            ahead: Vec2::ZERO,
            bg_order: Vec::new(),
            bg_cursor: 0,
            bg_focus: None,
            bg: Background::default(),
            draw_generation: NEXT_DRAW_GENERATION
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            baked_ring: [0; BAKE_TIMING_LAG + 1],
            frame_no: 0,
            ms_per_page: 0.0,
            cull: cull::Cull::new(n),
            blocks: cull::Blocks::default(),
            cull_epoch: 0,
            paged: vec![0; n],
            paged_stamp: 0,
            ready_share_ms: 0.0,
            ready_spent_ms: 0.0,
            last_baked: 0,
            cpu_ms_per_page: 0.0,
            was_ready: false,
        };
        let cells = &me.cells;
        me.blocks = cull::Blocks::build(n, me.grid.cells_x as usize, |i| cells.cell(i).bounds);
        // The pools hold the base layer of the whole map and what the view asks for (B3).
        me.cache.set_auto_size(true);
        // The cells the field has finished before this was made.
        for i in 0..n {
            if map.done[i] {
                me.seen_rev[i] = map.cell_rev[i];
                me.mark(i);
            }
        }
        me
    }

    fn mark(&mut self, index: usize) {
        let (cx, cy) = self.grid.cell_xy(index);
        for (x, y) in around(cx, cy, self.grid.cells_x, self.grid.cells_y) {
            let i = self.grid.index(x, y);
            if !self.in_dirty[i] {
                self.in_dirty[i] = true;
                self.dirty.push_back(i);
            }
        }
    }

    /// The cell and the eight around it are built in the field.
    fn bake_ready(&self, map: &super::MapState, index: usize) -> bool {
        let (cx, cy) = self.grid.cell_xy(index);
        around(cx, cy, self.grid.cells_x, self.grid.cells_y).all(|(x, y)| {
            // A cell nobody wants is not waited for: it is outside the ring of the open hexes, and
            // the border it gives is the empty field of the original.
            let i = map.index(x, y);
            map.done[i] || !map.wanted[i]
        })
    }

    /// The cells the field finished since the last frame; the cells whose fog changed.
    fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        map: &super::MapState,
        fog: &[Fog],
        fog_epoch: u64,
        clock: Option<&dyn Fn() -> f64>,
    ) {
        for i in 0..self.cells.len() {
            if map.done[i] && self.seen_rev[i] != map.cell_rev[i] {
                self.seen_rev[i] = map.cell_rev[i];
                self.mark(i);
            }
        }
        if self.fog_epoch != fog_epoch {
            self.fog_epoch = fog_epoch;
            if !fog.is_empty() {
                if !self.cells.set_fog(fog).is_empty() {
                    self.cull_epoch += 1;
                }
                let bytes = fog_mask_bytes(map.sampler.frame(), fog, self.fog.size());
                self.fog.write_mask(queue, &bytes);
                self.cache.restart_init();
                self.init_done = false;
            }
        }
        // Make the cells ready: the statistics and the textures of the cell, the heights on the GPU,
        // a new bake of the pages it has.
        // A cell that is not drawn costs a quarter of one that is (no contributions, no normals).
        let mut made = 0;
        let mut waiting = VecDeque::new();
        // A cell the background was making ready (its normals a few rows a frame) goes back to the
        // queue when the background stops: out of the queue and not ready, it waited for a background
        // that does not run while the view or the ring has cells to make ready, and when the camera came
        // to it, it stayed a hole over the ocean plane for good (the grey block of the goldens; nothing
        // counted it as work, so the renderer was settled).
        if !self.background_mode()
            && let Some((Unit::Ready(i), _)) = self.bg.prep
        {
            self.bg.prep = None;
            if !self.in_dirty[i] {
                self.in_dirty[i] = true;
                self.dirty.push_front(i);
            }
        }
        // The cells the last frame saw in view or in the ring without ground come first, in that order:
        // behind the thousands of cells the field builds out of view (one queue, in the order they were
        // built) a cell of the ring waited seconds, and the field kept the share of the load for it.
        // While cells in view wait, the share of the load ([`READY_COST_PER_FRAME`]); while only the
        // ring's do, [`FIELD_RING_MS`] (one cell at least) and nothing of the queue.
        let loading = self.stats.unbuilt > 0 || self.frame_no == 0;
        let ring_ms = FIELD_RING_MS.max(self.ready_share_ms);
        let start = clock.map(|c| c());
        let in_time = |made: usize| {
            made == 0 || loading || start.zip(clock).is_none_or(|(t0, c)| c() - t0 < ring_ms)
        };
        let first = std::mem::take(&mut self.unbuilt_cells);
        for &i in &first {
            if made >= READY_COST_PER_FRAME || !in_time(made) {
                break;
            }
            // A cell of the view that is built and not ready is made ready, in the queue or not.
            if self.ready[i] || !map.done[i] || !self.bake_ready(map, i) {
                continue;
            }
            if self.in_dirty[i]
                && let Some(at) = self.dirty.iter().position(|&d| d == i)
            {
                self.dirty.remove(at);
            }
            self.in_dirty[i] = false;
            let textured = self.make_ready(device, queue, map, i);
            made += if textured { READY_COST_DRAWN } else { 1 };
        }
        self.unbuilt_cells = first;
        // The queue keeps the cells the field has built: a cell around a built one that the field has
        // not built (one nobody wants, at the edge of the open land) is marked again when it is built.
        // Left in the queue it never got out of it once the view was complete (the background takes only
        // built cells), and the renderer never settled ("pending: Terrain: 4" in the goldens).
        let in_dirty = &mut self.in_dirty;
        self.dirty.retain(|&i| {
            let keep = map.done[i];
            in_dirty[i] = keep;
            keep
        });
        // Once the view and its ring are ready the cells the field builds are out of view: they are
        // made ready by the background, in its time ([`Self::background_step`]).
        let share = if loading { READY_COST_PER_FRAME } else { 0 };
        while made < share {
            let Some(i) = self.dirty.pop_front() else {
                break;
            };
            self.in_dirty[i] = false;
            if !map.done[i] {
                continue;
            }
            if !self.bake_ready(map, i) {
                waiting.push_back(i);
                continue;
            }
            let textured = self.make_ready(device, queue, map, i);
            made += if textured { READY_COST_DRAWN } else { 1 };
        }
        // A cell whose neighbours are not built yet waits for them: marked again when they are.
        for i in waiting {
            self.in_dirty[i] = true;
            self.dirty.push_back(i);
        }
        self.stats.waiting = self.dirty.len();
        // The cells a decal changed, with what is left of the frame's share.
        while made < READY_COST_PER_FRAME {
            let Some(i) = self.stale.pop_front() else {
                break;
            };
            self.in_stale[i] = false;
            if self.ready[i] {
                let textured = self.cells.recompute_live(
                    i,
                    &map.sampler,
                    &self.projections,
                    self.settings.has_spec(),
                );
                self.stats.recomputed += 1;
                self.stats.textured += u64::from(textured);
                self.cull_epoch += 1;
                self.cache.invalidate(i, Rebake::Decal);
                made += if textured { READY_COST_DRAWN } else { 1 };
            }
        }
    }

    /// The cell is built with the cells around it: its statistics and textures (they read the field
    /// around the cell; the ones not wanted are zero), its heights on the GPU, a new bake of the pages it
    /// has. Returns whether its textures were made (a drawn cell).
    fn make_ready(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        map: &super::MapState,
        i: usize,
    ) -> bool {
        let textured =
            self.cells
                .recompute_live(i, &map.sampler, &self.projections, self.settings.has_spec());
        self.stats.recomputed += 1;
        self.stats.textured += u64::from(textured);
        self.cull_epoch += 1;
        let (cx, cy) = self.grid.cell_xy(i);
        let rect = self.cells.cell(i).rect;
        self.heights
            .upload_cell(device, queue, &map.sampler, (cx, cy), &rect);
        self.cache.invalidate(i, Rebake::Field);
        if !self.ready[i] {
            self.ready[i] = true;
            self.stats.ready += 1;
            self.cache.restart_init();
            self.init_done = false;
        }
        textured
    }

    /// The view and its ring are built (as the last frame saw them): what is left is out of view.
    fn background_mode(&self) -> bool {
        self.unbuilt_cells.is_empty() && self.frame_no > 0
    }

    /// Visible cells with their width on the screen, per copy; the draw list. The culling is cached
    /// ([`cull::Cull`]): on an unchanged view only the page lookups of the cells in view are made.
    #[allow(clippy::too_many_arguments)]
    fn plan(
        &mut self,
        map: &super::MapState,
        gpu: &LiveGpu,
        view: &LiveView,
        prev: Option<Mat4>,
    ) -> (Vec<CellView>, Vec<DrawCell>) {
        let _ = gpu;
        if !self.cull.is_current(view, self.cull_epoch, self.ahead) {
            let (cells, ready) = (&self.cells, &self.ready);
            self.cull.rebuild(
                &self.blocks,
                |i| cells.cell(i).bounds,
                |i| {
                    let cell = cells.cell(i);
                    if !ready[i] {
                        if cell.is_open() {
                            cull::Kind::Unbuilt
                        } else {
                            cull::Kind::Skip
                        }
                    } else if cell.is_open() && cell.stats.class.is_drawn() {
                        cull::Kind::Drawn
                    } else {
                        cull::Kind::Skip
                    }
                },
                view,
                self.cull_epoch,
                self.ahead,
            );
        }
        let clip = |m: &Mat4, p: Vec3| {
            let c = *m * p.extend(1.0);
            (c.w > 1e-6).then(|| Vec2::new(c.x / c.w, c.y / c.w))
        };
        // How the picture moved since the last frame (the mean over the cells of the middle copy): the
        // camera moves the other way. With the view of the last frame the motion is exactly zero.
        let (mut motion, mut moved) = (Vec2::ZERO, 0u32);
        if let (Some(p), Some(&shift)) = (prev, view.shifts.first())
            && p != view.view_proj_orig
        {
            let was_m = p * Mat4::from_translation(Vec3::new(shift, 0.0, 0.0));
            for v in self.cull.in_view.iter().filter(|v| v.copy == 0) {
                let center = self.cells.cell(v.cell as usize).bounds.center;
                if let Some(was) = clip(&was_m, center) {
                    motion += v.at - was;
                    moved += 1;
                }
            }
        }
        let mut draws: Vec<DrawCell> = Vec::with_capacity(self.cull.in_view.len());
        for v in &self.cull.in_view {
            let i = v.cell as usize;
            let entry = self.cache.entry(i);
            // The page and its highlight page (baked in the same job, so ready with it; a highlight
            // page the cache page got back is drawn once its job is done).
            let page = if entry.near_ready {
                entry.near_page.map(|p| (p, entry.near_spec))
            } else if entry.cache_ready {
                entry
                    .cache_page
                    .map(|p| (p, entry.cache_spec.filter(|_| entry.cache_spec_ready)))
            } else {
                None
            };
            if let Some((page, spec)) = page {
                let cell = self.cells.cell(i);
                draws.push(DrawCell {
                    copy: v.copy as usize,
                    cell: i,
                    rect: cell.rect,
                    page,
                    spec,
                    art: usize::from(cell.class()),
                });
            }
        }
        let ahead = if moved > 0 {
            (-motion / moved as f32 * LOOKAHEAD_FRAMES).clamp_length_max(LOOKAHEAD_MAX)
        } else {
            Vec2::ZERO
        };
        self.ahead = ahead;
        // The cell nearest the middle of the screen, or the one a camera of a mod names (a low eye looks at the
        // horizon, and the middle of its screen is the farthest cell in view).
        self.focus = match view.focus {
            Some(at) => Some(
                self.grid
                    .index_of_texel(map.sampler.frame().world_to_tex(at)),
            ),
            None => self
                .cull
                .best
                .iter()
                .min_by(|a, b| {
                    a.at.length()
                        .total_cmp(&b.at.length())
                        .then(a.cell.cmp(&b.cell))
                })
                .map(|b| b.cell as usize)
                .or(self.focus),
        };
        let mut visible: Vec<CellView> = Vec::with_capacity(self.cull.best.len());
        visible.extend(self.cull.best.iter().map(|b| CellView {
            cell: b.cell as usize,
            width_px: b.width,
            dist: b.at.distance(ahead),
            ring: false,
        }));
        // The ring: the cells out of view in every copy, without a page yet, nearest to where the view
        // goes first (after the cells in view: their distance is past the edge of the screen).
        let mut ring_holes = 0;
        for r in &self.cull.ring {
            let cell = r.cell as usize;
            if self.cull.in_best(cell) || self.cache.entry(cell).is_drawable() {
                continue;
            }
            ring_holes += 1;
            visible.push(CellView {
                cell,
                width_px: 0.0,
                dist: r.at.distance(ahead),
                ring: true,
            });
        }
        self.stats.ring_unpaged = ring_holes;
        self.stats.unbuilt = self.cull.unbuilt.len();
        self.stats.ring_unbuilt = self.cull.ring_unbuilt.len();
        self.unbuilt_cells.clear();
        self.unbuilt_cells
            .extend(self.cull.unbuilt.iter().map(|&i| i as usize));
        self.unbuilt_cells
            .extend(self.cull.ring_unbuilt.iter().map(|&i| i as usize));
        // The order of the original is the widest first; the cache sorts them.
        visible.sort_by_key(|v| v.cell);
        (visible, draws)
    }

    /// The CPU work of a frame.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        gpu: &mut LiveGpu,
        map: &super::MapState,
        fog: &[Fog],
        fog_epoch: u64,
        inputs: &LiveInputs<'_>,
    ) {
        let now = || inputs.clock.map_or(0.0, |c| c());
        let t0 = now();
        self.sync(device, queue, map, fog, fog_epoch, inputs.clock);
        let mut spent = now() - t0;
        self.lights = Some(inputs.lights.clone());
        if self.decals_on != inputs.decals {
            // The pages that were baked with the other setting are made again.
            self.decals_on = inputs.decals;
            for i in 0..self.cells.len() {
                if self.ready[i] {
                    self.cache.invalidate(i, Rebake::Settings);
                }
            }
        }
        // Whether the camera moves: the budget of the pages follows it.
        let m = inputs.view.view_proj_orig;
        self.moving = self.last_view.is_some_and(|l| {
            l.to_cols_array()
                .iter()
                .zip(m.to_cols_array())
                .any(|(a, b)| (a - b).abs() > 1e-5)
        });
        let prev = self.last_view.replace(m);
        // The GPU time of the bake of a few frames back, per page it baked.
        self.frame_no += 1;
        let slot = self.frame_no % self.baked_ring.len();
        let then = self.baked_ring[(slot + 1) % self.baked_ring.len()];
        if let Some(ms) = inputs.bake_gpu_ms
            && then > 0
            && ms > 0.0
        {
            let per = ms / then as f32;
            self.ms_per_page = if self.ms_per_page > 0.0 {
                self.ms_per_page * 0.75 + per * 0.25
            } else {
                per
            };
        }
        self.baked_ring[slot] = 0;
        // The CPU time of a page, from the bake pass of the last frame.
        if let Some(ms) = inputs.bake_cpu_ms
            && self.last_baked > 0
            && ms > 0.0
        {
            let per = ms / self.last_baked as f32;
            self.cpu_ms_per_page = if self.cpu_ms_per_page > 0.0 {
                self.cpu_ms_per_page * 0.75 + per * 0.25
            } else {
                per
            };
        }
        self.last_baked = 0;

        let (visible, draws) = self.plan(map, gpu, &inputs.view, prev);
        let budget = self.page_budget();
        self.cache
            .launch_jobs(self.cells.as_slice(), &visible, budget);
        for _ in 0..LOAD_STEPS_PER_FRAME {
            if self.cache.init_step(self.cells.as_slice()) {
                self.init_done = true;
                break;
            }
        }
        self.arrays.ensure(device, self.cache.pools());
        self.stats.drawn = draws.len();
        self.stats.visible = visible.iter().filter(|v| !v.ring).count();
        {
            self.paged_stamp = self.paged_stamp.wrapping_add(1);
            if self.paged_stamp == 0 {
                self.paged.fill(0);
                self.paged_stamp = 1;
            }
            for d in &draws {
                self.paged[d.cell] = self.paged_stamp;
            }
            let stamp = self.paged_stamp;
            self.stats.unpaged = visible
                .iter()
                .filter(|v| !v.ring && self.paged[v.cell] != stamp)
                .count();
        }
        let t1 = now();
        self.background_step(device, queue, map, inputs);
        spent += now() - t1;
        self.ready_spent_ms = spent;
        self.stats.holes_total += self.stats.unpaged as u64;
        self.stats.hole_frames += u64::from(self.stats.unpaged > 0);
        self.stats.budget = budget;
        self.was_ready |= self.view_ready();
        gpu.drawer.prepare(
            device,
            queue,
            &self.fog,
            &FrameDraw {
                frame: map.sampler.frame(),
                view_proj_orig: inputs.view.view_proj_orig,
                eye_orig: inputs.view.eye_orig,
                shifts: &inputs.view.shifts,
                frame_globals: inputs.frame_globals,
                shadow_map: inputs.shadow_map,
                lights: inputs.lights,
                arrays: &self.arrays,
                tiles: &self.heights,
                cells: &draws,
                lattice_step: self.settings.lattice_step() as i32,
                generation: self.draw_generation,
                fow: inputs.fow,
            },
        );
    }

    /// The squared distance in cells between two cells, across the seam of a map that wraps.
    fn cell_distance(&self, a: usize, b: usize) -> i64 {
        let ((ax, ay), (bx, by)) = (self.grid.cell_xy(a), self.grid.cell_xy(b));
        let mut dx = i64::from((ax - bx).abs());
        if self.grid.wrap_x {
            dx = dx.min(i64::from(self.grid.cells_x) - dx);
        }
        let dy = i64::from(ay - by);
        dx * dx + dy * dy
    }

    /// The next cell of the background order (nearest to the camera first) whose base page is to be
    /// made: built, drawn, open, without a page.
    fn next_background_cell(&mut self) -> Option<usize> {
        let focus = self.focus?;
        // Moving, the order is made again only when the camera has gone four times as far (a sort of
        // the map is a millisecond).
        let r = i64::from(BACKGROUND_REFOCUS) * if self.moving { 4 } else { 1 };
        let refocus = self
            .bg_focus
            .is_none_or(|f| self.cell_distance(f, focus) >= r * r);
        if refocus {
            let mut order: Vec<usize> = (0..self.cells.len()).collect();
            order.sort_unstable_by_key(|&i| (self.cell_distance(i, focus), i));
            self.bg_order = order;
            self.bg_cursor = 0;
            self.bg_focus = Some(focus);
        }
        let cells = self.cells.as_slice();
        // A cell is served when it has its page, or is built and is no ground to draw.
        let served = |me: &Self, i: usize| {
            me.cache.entry(i).cache_page.is_some()
                || (me.ready[i] && !(cells[i].stats.class.is_drawn() && cells[i].is_open()))
        };
        while self.bg_cursor < self.bg_order.len() && served(self, self.bg_order[self.bg_cursor]) {
            self.bg_cursor += 1;
        }
        let end = (self.bg_cursor + BACKGROUND_SCAN).min(self.bg_order.len());
        self.bg_order[self.bg_cursor..end]
            .iter()
            .copied()
            .find(|&i| {
                self.ready[i]
                    && cells[i].stats.class.is_drawn()
                    && cells[i].is_open()
                    && self.cache.entry(i).cache_page.is_none()
            })
    }

    /// The first cell the field has built with its neighbours that waits to be made ready, taken out
    /// of the queue.
    fn take_ready_cell(&mut self, map: &super::MapState) -> Option<usize> {
        let at = self
            .dirty
            .iter()
            .position(|&i| map.done[i] && self.bake_ready(map, i))?;
        let i = self.dirty.remove(at)?;
        self.in_dirty[i] = false;
        Some(i)
    }

    /// The background work of a frame, once the view and its ring are built: the cells out of view are
    /// made ready, then the base pages of the rest of the map are baked, nearest to the camera first (a
    /// pan then finds them made; B3, AC-T2: the pages made only as the cells were seen left holes behind
    /// a far pan). It spends [`BACKGROUND_MS`] of CPU a frame on average, on the renderer's clock, and
    /// nothing in a frame after one over [`FRAME_TARGET_MS`]. The block of normals a unit reads is made
    /// a few rows at a time over the frames it takes (kept in the sampler's cache, so the recompute and
    /// the bake find it); a base page is asked for when the credit covers what its bake costs.
    fn background_step(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        map: &super::MapState,
        inputs: &LiveInputs<'_>,
    ) {
        // What a background page costs: the pass over its pages, in a frame that baked only those.
        let delta = self.stats.baked.saturating_sub(self.bg.baked_seen);
        self.bg.baked_seen = self.stats.baked;
        if self.bg.pushed > 0
            && !self.bg.view_jobs
            && delta > 0
            && let Some(ms) = inputs.bake_cpu_ms.filter(|ms| *ms > 0.0)
        {
            let per = f64::from(ms) / delta as f64;
            self.bg.page_ms = if self.bg.page_ms > 0.0 {
                self.bg.page_ms * 0.75 + per * 0.25
            } else {
                per
            };
        }
        self.bg.pushed = 0;
        self.bg.view_jobs = self.cache.jobs().count() > self.cache.jobs_out_of_view();
        if !self.background_mode() || self.cache.spec_budget().is_none() {
            return;
        }
        // Cells built in the field that wait to be made ready are work the renderer waits for (it is not
        // settled until they are ready): they go on in a frame over the target too, at a quarter of the
        // share. With nothing of the kind, such a frame does nothing.
        let readying = !self.dirty.is_empty() || matches!(self.bg.prep, Some((Unit::Ready(_), _)));
        let over = inputs.frame_cpu_ms.is_some_and(|ms| ms > FRAME_TARGET_MS);
        if over && !readying {
            return;
        }
        let page_ms = if self.bg.page_ms > 0.0 {
            self.bg.page_ms
        } else {
            BACKGROUND_PAGE_MS
        };
        // While the cells the field built wait to be made ready they have the time the field left too:
        // at the 0.7 ms of the background alone they fell behind the field by thousands, and the map
        // took tens of seconds more to be ready than to be built.
        let (share, extra) = match (over, readying) {
            (true, _) => (0.25 * BACKGROUND_MS, 0.0),
            (false, true) => (BACKGROUND_MS, self.ready_share_ms),
            (false, false) => (BACKGROUND_MS, 0.0),
        };
        self.bg.credit =
            (self.bg.credit + share + extra).min(BACKGROUND_CREDIT_MS.max(page_ms) + extra);
        // The clock, if it moves; otherwise what the work is guessed to cost.
        let clock = inputs.clock.filter(|c| {
            let a = c();
            c() > a || a > 0.0
        });
        let now = || clock.map_or(0.0, |c| c());
        let view_done = self.stats.unpaged == 0 && self.stats.ring_unpaged == 0;
        while self.bg.credit > 0.0 {
            // A frame over the target makes cells ready only.
            if !over && let Some(i) = self.bg.bake_next {
                if self.bg.credit < page_ms {
                    break;
                }
                self.bg.bake_next = None;
                if view_done
                    && self.cache.jobs_out_of_view() == 0
                    && self.ready[i]
                    && self.cache.request_background(i)
                {
                    self.bg.pushed += 1;
                    self.bg.credit -= page_ms;
                    self.bg.spent_total += page_ms;
                }
                // One page a frame at most: the bake pass is timed by the frame.
                break;
            }
            let t0 = now();
            if let Some((unit, p)) = self.bg.prep.as_mut() {
                let unit = *unit;
                let done = map.sampler.continue_normals(p, BACKGROUND_ROWS);
                if done {
                    self.bg.prep = None;
                    match unit {
                        Unit::Ready(i) => {
                            self.make_ready(device, queue, map, i);
                        }
                        Unit::Bake(i) => self.bg.bake_next = Some(i),
                    }
                }
            } else if let Some(i) = self.take_ready_cell(map) {
                let (origin, size) = super::cells::contribution_normals(&self.cells.cell(i).rect);
                match map.sampler.begin_normals(origin, size) {
                    Some(p) => self.bg.prep = Some((Unit::Ready(i), p)),
                    None => {
                        self.make_ready(device, queue, map, i);
                    }
                }
            } else if !over
                && view_done
                && self.cache.jobs_out_of_view() == 0
                && let Some(i) = self.next_background_cell()
            {
                let (origin, size) = super::cells::vertex_normals(&self.cells.cell(i).rect);
                match map.sampler.begin_normals(origin, size) {
                    Some(p) => self.bg.prep = Some((Unit::Bake(i), p)),
                    None => self.bg.bake_next = Some(i),
                }
            } else {
                break;
            }
            let spent = if clock.is_some() {
                now() - t0
            } else {
                // A step of a few rows of normals, or a cell made ready.
                0.1
            };
            self.bg.credit -= spent;
            self.bg.spent_total += spent;
        }
    }

    /// CPU the background has spent so far, in milliseconds (`terrain.background.ms_total`), and what
    /// a background page is measured to cost (`terrain.background.page_us`).
    pub fn background_spent(&self) -> (f64, f64) {
        (self.bg.spent_total, self.bg.page_ms)
    }
    /// Cells launched and baked this frame. While the first load runs it is [`LOAD_BAKES_PER_FRAME`];
    /// after it the camera's `TerrainPageinSpeed*`, except that cells in view that have no page yet
    /// take the full budget of a frame, the camera still or moving (`MinimizeGrayTiles` for as long
    /// as there is grey on the screen): a map that has just come in fills up in a few frames instead of
    /// one or two pages a frame (AC-R8.4, warm start), and a scroll leaves no hole behind (B3, AC-T2;
    /// the jobs of those cells come first in the queue).
    /// The count is bounded by the GPU time of a page ([`time_bounded`]).
    fn page_budget(&self) -> usize {
        let count = if !self.init_done {
            LOAD_BAKES_PER_FRAME
        } else if self.stats.unpaged > 0 {
            super::bake::MAX_LAUNCHES_PER_FRAME
        } else if self.stats.ring_unpaged > 0 {
            // Only the ring: as many as [`RING_BAKE_MS`] of CPU hold.
            let fit = if self.cpu_ms_per_page > 0.0 {
                (RING_BAKE_MS / self.cpu_ms_per_page).floor() as usize
            } else {
                super::bake::MAX_LAUNCHES_PER_FRAME
            };
            fit.clamp(1, super::bake::MAX_LAUNCHES_PER_FRAME)
        } else {
            bake_budget(&self.settings, self.moving).max(1)
        };
        time_bounded(count, self.ms_per_page)
    }

    /// The pages of this frame's jobs, into the commands of the bake pass.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn bake(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        gpu: &mut LiveGpu,
        map: &super::MapState,
        library: &TerrainLibrary,
        library_epoch: u64,
        layer: &DecalLayer,
        budget_override: Option<usize>,
    ) {
        let Some(lights) = self.lights.as_ref() else {
            return;
        };
        if self.decals_on {
            let _s = crate::perf::slow::scope("bake.decal_textures");
            gpu.ensure_decal_textures(device, queue, library, layer, library_epoch);
        }
        let decals = self.decals_on.then_some(DecalBake {
            layer,
            textures: &gpu.decal_textures,
        });
        {
            let _s = crate::perf::slow::scope("bake.arrays");
            self.arrays.ensure(device, self.cache.pools());
        }
        let budget = budget_override.unwrap_or_else(|| self.page_budget());
        let lookup =
            |class: u8| -> Option<&BakeLight> { lights.class(usize::from(class)).map(|c| &c.bake) };
        let scene = BakeScene {
            sampler: &map.sampler,
            projections: &self.projections,
            kernel: &self.kernel,
            grid: &self.grid,
            defs: &gpu.defs,
            light: &lookup,
            decals,
            shadow: Some(ShadowBake {
                tiles: &self.heights,
                sun_dir: crate::lighting::globals::SHADOW_DIR,
                quality: self.settings.shadow_quality,
                world_scales: {
                    let s = map.sampler.frame().terrain_scale();
                    [s.x, s.y, s.z]
                },
            }),
        };
        let stats = gpu.baker.encode_jobs(
            device,
            queue,
            encoder,
            &self.arrays,
            &mut self.cache,
            &self.cells,
            &scene,
            budget,
            // A budget the caller gives is its own (no time bound).
            budget_override.is_none().then_some(BAKE_CPU_MS),
        );
        self.stats.baked += stats.baked as u64;
        self.last_baked = stats.baked;
        let slot = self.frame_no % self.baked_ring.len();
        self.baked_ring[slot] += stats.baked;
        for (job, why) in &stats.failed {
            self.stats.failed += 1;
            tracing::warn!(
                cell = job.cell,
                "a page of the terrain was not baked: {why}"
            );
        }
    }

    /// The pages of the cells that an area of our world touches are made again (`InvalidateRegion`):
    /// a decal was created or destroyed there. A cell that is not ready has no page to lose.
    pub(super) fn invalidate_world_region(&mut self, map: &super::MapState, min: Vec2, max: Vec2) {
        let frame = map.sampler.frame();
        let a = frame.world_to_tex(min);
        let b = frame.world_to_tex(max);
        // The page has a texel of border: the neighbours see the decal too.
        let lo = a.min(b) - Vec2::ONE;
        let hi = a.max(b) + Vec2::ONE;
        let width = self.grid.tex_w as f32;
        let mut cells = self.cells.touch_region(lo.max(Vec2::ZERO), hi);
        if self.grid.wrap_x {
            cells.extend(self.cells.touch_region(
                (lo + Vec2::new(width, 0.0)).max(Vec2::ZERO),
                hi + Vec2::new(width, 0.0),
            ));
            cells.extend(self.cells.touch_region(
                (lo - Vec2::new(width, 0.0)).max(Vec2::ZERO),
                hi - Vec2::new(width, 0.0),
            ));
        }
        // Each cell once, however many decals touch it; made again in `sync`.
        for i in cells {
            if self.ready[i] && !self.in_stale[i] {
                self.in_stale[i] = true;
                self.stale.push_back(i);
            }
        }
    }

    /// Work that is left: pages to bake, cells to make ready (the one the background is making ready
    /// too), cells that have no page.
    pub fn pending_work(&self) -> usize {
        let preparing = usize::from(matches!(self.bg.prep, Some((Unit::Ready(_), _))));
        self.cache.pending_work(self.cells.as_slice())
            + self.dirty.len()
            + self.stale.len()
            + preparing
    }

    /// How much of what the fog opens is baked, 0 to 1.
    pub fn load_progress(&self) -> f32 {
        self.cache.load_progress(self.cells.as_slice())
    }

    pub fn arrays(&self) -> &PageArrays {
        &self.arrays
    }

    pub fn heights(&self) -> &HeightTiles {
        &self.heights
    }

    pub fn fog_bridge(&self) -> &FogBridge {
        &self.fog
    }

    /// The view of the last frame is complete: every open cell in view and in its ring is built and has a
    /// page (`terrain.view_ready`).
    pub fn view_ready(&self) -> bool {
        self.frame_no > 0
            && self.stats.unbuilt == 0
            && self.stats.ring_unbuilt == 0
            && self.stats.unpaged == 0
            && self.stats.ring_unpaged == 0
    }

    /// Open cells in view or in the ring of the last frame wait for the field.
    pub fn view_unbuilt(&self) -> bool {
        !self.unbuilt_cells.is_empty()
    }

    pub fn is_ready(&self, index: usize) -> bool {
        self.ready.get(index).copied().unwrap_or(false)
    }
}

/// The culling of the map against the view, cached: what is in the view, in the ring around it, and
/// what waits for the field. A coarse grid of blocks (merged spheres) skips whole groups of cells.
mod cull {
    use glam::{Mat4, Vec2, Vec3, Vec4};

    use super::LiveView;
    use crate::world::terrain::cells::CellBounds;
    use crate::world::terrain::draw::frustum_planes;

    /// Cells along a side of a block.
    const BLOCK: usize = 16;

    /// What a cell is to the culling.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Kind {
        /// Not open, or ready and not drawn.
        Skip,
        /// Open and not ready.
        Unbuilt,
        /// Open, ready and drawn.
        Drawn,
    }

    struct Block {
        center: Vec3,
        radius: f32,
        start: u32,
        end: u32,
    }

    /// Blocks of cells with a sphere that holds the spheres of all their cells; built once per cell set.
    #[derive(Default)]
    pub struct Blocks {
        blocks: Vec<Block>,
        order: Vec<u32>,
    }

    impl Blocks {
        pub fn build(n: usize, cells_x: usize, bounds: impl Fn(usize) -> CellBounds) -> Self {
            let cells_x = cells_x.max(1);
            let per_row = cells_x.div_ceil(BLOCK);
            let rows = n.div_ceil(cells_x).div_ceil(BLOCK);
            let mut groups: Vec<Vec<u32>> = vec![Vec::new(); per_row * rows.max(1)];
            for i in 0..n {
                let (x, y) = (i % cells_x, i / cells_x);
                groups[(y / BLOCK) * per_row + x / BLOCK].push(i as u32);
            }
            let mut out = Self::default();
            for g in groups.into_iter().filter(|g| !g.is_empty()) {
                let sum: Vec3 = g.iter().map(|&i| bounds(i as usize).center).sum();
                let center = sum / g.len() as f32;
                let radius = g
                    .iter()
                    .map(|&i| {
                        let b = bounds(i as usize);
                        b.center.distance(center) + b.radius
                    })
                    .fold(0.0f32, f32::max);
                let start = out.order.len() as u32;
                out.order.extend_from_slice(&g);
                out.blocks.push(Block {
                    center,
                    radius,
                    start,
                    end: out.order.len() as u32,
                });
            }
            out
        }
    }

    /// Whether a sphere is clearly outside one of the planes (with a margin: only a block that no cell of
    /// can pass is skipped).
    fn clearly_out(b: &Block, planes: &[Vec4; 6]) -> bool {
        let margin = b.radius * 1.01 + 0.5;
        planes
            .iter()
            .any(|p| p.truncate().dot(b.center) + p.w < -margin)
    }

    fn visible(b: &CellBounds, planes: &[Vec4; 6]) -> bool {
        b.is_visible(planes).0
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct InView {
        pub copy: u32,
        pub cell: u32,
        /// Width on the screen of the cell in this copy.
        pub width: f32,
        /// Where the middle of the cell is on the screen (clip x, y) in this copy.
        pub at: Vec2,
    }

    /// A cell in view: the widest copy.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Best {
        pub cell: u32,
        pub width: f32,
        pub at: Vec2,
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Ring {
        pub cell: u32,
        pub at: Vec2,
    }

    const F_BEST: u8 = 1;
    const F_RING: u8 = 2;
    const F_UNB: u8 = 4;
    const F_RUNB: u8 = 8;

    /// The culling of one view.
    pub struct Cull {
        valid: bool,
        view_proj: [f32; 16],
        shifts: Vec<f32>,
        screen_width: f32,
        near: Option<f32>,
        epoch: u64,
        /// How far the ring reaches ahead of the camera (clip units, quantised): see [`Self::rebuild`].
        ahead: Vec2,
        /// The cells in view, per copy (the middle one first), each copy by cell.
        pub in_view: Vec<InView>,
        /// The widest copy of each cell in view, by cell.
        pub best: Vec<Best>,
        /// The cells of the ring that are drawn (the first copy that has them), by discovery.
        pub ring: Vec<Ring>,
        /// Open cells in view that are not ready, by cell.
        pub unbuilt: Vec<u32>,
        /// Open cells in the ring only that are not ready, by cell.
        pub ring_unbuilt: Vec<u32>,
        /// Per cell: the `F_*` bits of the lists the cell is in; slot: where in `best`.
        flags: Vec<u8>,
        slot: Vec<u32>,
    }

    impl Cull {
        pub fn new(n: usize) -> Self {
            Self {
                valid: false,
                view_proj: [0.0; 16],
                shifts: Vec::new(),
                screen_width: 0.0,
                near: None,
                epoch: 0,
                ahead: Vec2::ZERO,
                in_view: Vec::new(),
                best: Vec::new(),
                ring: Vec::new(),
                unbuilt: Vec::new(),
                ring_unbuilt: Vec::new(),
                flags: vec![0; n],
                slot: vec![0; n],
            }
        }

        /// The ring ahead of a camera that moves by `ahead` (clip units), in steps of a quarter.
        pub fn quantise(ahead: Vec2) -> Vec2 {
            (ahead * 4.0).round() / 4.0
        }

        pub fn is_current(&self, view: &LiveView, epoch: u64, ahead: Vec2) -> bool {
            self.valid
                && self.epoch == epoch
                && self.ahead == Self::quantise(ahead)
                && self.screen_width == view.screen_width
                && self.near == view.near
                && self.shifts == view.shifts
                && self.view_proj == view.view_proj_orig.to_cols_array()
        }

        pub fn in_best(&self, cell: usize) -> bool {
            self.flags[cell] & F_BEST != 0
        }

        /// Culls again. `kind` and `bounds` are asked for the cells of the blocks the view reaches only.
        pub fn rebuild(
            &mut self,
            blocks: &Blocks,
            bounds: impl Fn(usize) -> CellBounds,
            kind: impl Fn(usize) -> Kind,
            view: &LiveView,
            epoch: u64,
            ahead: Vec2,
        ) {
            let ahead = Self::quantise(ahead);
            for v in &self.in_view {
                self.flags[v.cell as usize] = 0;
            }
            for r in &self.ring {
                self.flags[r.cell as usize] = 0;
            }
            for &i in self.unbuilt.iter().chain(&self.ring_unbuilt) {
                self.flags[i as usize] = 0;
            }
            self.in_view.clear();
            self.best.clear();
            self.ring.clear();
            self.unbuilt.clear();
            self.ring_unbuilt.clear();
            let clip = |m: &Mat4, p: Vec3| {
                let c = *m * p.extend(1.0);
                (c.w > 1e-6).then(|| Vec2::new(c.x / c.w, c.y / c.w))
            };
            for (copy, &shift) in view.shifts.iter().enumerate() {
                let m = view.view_proj_orig * Mat4::from_translation(Vec3::new(shift, 0.0, 0.0));
                let planes = frustum_planes(&m);
                // The view widened by [`RING_WIDEN`] on each side, for the ring.
                let wide = frustum_planes(
                    &(Mat4::from_scale(Vec3::new(
                        1.0 / super::RING_WIDEN,
                        1.0 / super::RING_WIDEN,
                        1.0,
                    )) * m),
                );
                // The ring ahead of a moving camera: the widened view moved the way the view goes
                // ([`super::RING_AHEAD`] times the guess of where it goes).
                let fore = (ahead != Vec2::ZERO).then(|| {
                    let a = ahead * super::RING_AHEAD;
                    frustum_planes(
                        &(Mat4::from_translation(Vec3::new(-a.x, -a.y, 0.0))
                            * Mat4::from_scale(Vec3::new(
                                1.0 / super::RING_WIDEN,
                                1.0 / super::RING_WIDEN,
                                1.0,
                            ))
                            * m),
                    )
                });
                let in_ring = |b: &CellBounds| {
                    visible(b, &wide) || fore.as_ref().is_some_and(|f| visible(b, f))
                };
                let first = self.in_view.len();
                for block in &blocks.blocks {
                    if clearly_out(block, &planes)
                        && clearly_out(block, &wide)
                        && fore.as_ref().is_none_or(|f| clearly_out(block, f))
                    {
                        continue;
                    }
                    for &i in &blocks.order[block.start as usize..block.end as usize] {
                        let iu = i as usize;
                        let k = kind(iu);
                        if k == Kind::Skip {
                            continue;
                        }
                        let b = bounds(iu);
                        let f = &mut self.flags[iu];
                        if k == Kind::Unbuilt {
                            if visible(&b, &planes) {
                                if *f & F_UNB == 0 {
                                    *f |= F_UNB;
                                    self.unbuilt.push(i);
                                }
                            } else if *f & F_RUNB == 0 && in_ring(&b) {
                                *f |= F_RUNB;
                                self.ring_unbuilt.push(i);
                            }
                            continue;
                        }
                        if !visible(&b, &planes) {
                            if *f & F_RING == 0 && in_ring(&b) {
                                *f |= F_RING;
                                self.ring.push(Ring {
                                    cell: i,
                                    at: clip(&m, b.center).unwrap_or(Vec2::ZERO),
                                });
                            }
                            continue;
                        }
                        let width = match view.near {
                            Some(near) => b.max_width_px_clipped(&m, view.screen_width, near),
                            None => b.max_width_px(&m, view.screen_width),
                        };
                        let at = clip(&m, b.center).unwrap_or(Vec2::ZERO);
                        if *f & F_BEST == 0 {
                            *f |= F_BEST;
                            self.slot[iu] = self.best.len() as u32;
                            self.best.push(Best { cell: i, width, at });
                        } else {
                            let e = &mut self.best[self.slot[iu] as usize];
                            if width > e.width {
                                e.width = width;
                                e.at = at;
                            }
                        }
                        self.in_view.push(InView {
                            copy: copy as u32,
                            cell: i,
                            width,
                            at,
                        });
                    }
                }
                self.in_view[first..].sort_unstable_by_key(|v| v.cell);
            }
            self.best.sort_unstable_by_key(|b| b.cell);
            self.unbuilt.sort_unstable();
            let flags = &self.flags;
            self.ring_unbuilt
                .retain(|&i| flags[i as usize] & F_UNB == 0);
            self.ring_unbuilt.sort_unstable();
            self.valid = true;
            self.epoch = epoch;
            self.screen_width = view.screen_width;
            self.near = view.near;
            self.shifts.clear();
            self.shifts.extend_from_slice(&view.shifts);
            self.view_proj = view.view_proj_orig.to_cols_array();
            self.ahead = ahead;
        }
    }

    #[cfg(test)]
    mod tests {
        use std::collections::{HashMap, HashSet};

        use super::*;

        const CX: usize = 40;
        const CY: usize = 30;
        const STEP: f32 = 64.0;

        fn bounds_of(i: usize) -> CellBounds {
            let (x, y) = ((i % CX) as f32, (i / CX) as f32);
            let min = Vec3::new(x * STEP, y * STEP, 0.0);
            let max = min + Vec3::new(STEP + 1.0, STEP + 1.0, 255.0);
            let center = (min + max) * 0.5;
            CellBounds {
                min,
                max,
                center,
                radius: 0.5 * (max - min).length(),
            }
        }

        fn kind_of(i: usize) -> Kind {
            // A fixed scramble: a third of the cells skipped, a fifth unbuilt.
            match ((i * 2_654_435_761usize) >> 7) % 15 {
                0..=4 => Kind::Skip,
                5..=7 => Kind::Unbuilt,
                _ => Kind::Drawn,
            }
        }

        /// The cull as it was before the cache: every cell, every copy.
        fn brute(view: &LiveView) -> Cull {
            let mut c = Cull::new(CX * CY);
            let mut best: HashMap<usize, (f32, Vec2)> = HashMap::new();
            let (mut unbuilt, mut ring_unbuilt) = (HashSet::new(), HashSet::new());
            let mut ring: HashMap<usize, Vec2> = HashMap::new();
            let clip = |m: &Mat4, p: Vec3| {
                let v = *m * p.extend(1.0);
                (v.w > 1e-6).then(|| Vec2::new(v.x / v.w, v.y / v.w))
            };
            for (copy, &shift) in view.shifts.iter().enumerate() {
                let m = view.view_proj_orig * Mat4::from_translation(Vec3::new(shift, 0.0, 0.0));
                let planes = frustum_planes(&m);
                let wide = frustum_planes(
                    &(Mat4::from_scale(Vec3::new(
                        1.0 / super::super::RING_WIDEN,
                        1.0 / super::super::RING_WIDEN,
                        1.0,
                    )) * m),
                );
                for i in 0..CX * CY {
                    let b = bounds_of(i);
                    match kind_of(i) {
                        Kind::Skip => {}
                        Kind::Unbuilt => {
                            if b.is_visible(&planes).0 {
                                unbuilt.insert(i);
                            } else if b.is_visible(&wide).0 {
                                ring_unbuilt.insert(i);
                            }
                        }
                        Kind::Drawn => {
                            if !b.is_visible(&planes).0 {
                                if b.is_visible(&wide).0 {
                                    ring.entry(i).or_insert_with(|| {
                                        clip(&m, b.center).unwrap_or(Vec2::ZERO)
                                    });
                                }
                                continue;
                            }
                            let width = match view.near {
                                Some(near) => b.max_width_px_clipped(&m, view.screen_width, near),
                                None => b.max_width_px(&m, view.screen_width),
                            };
                            let at = clip(&m, b.center).unwrap_or(Vec2::ZERO);
                            let e = best.entry(i).or_insert((0.0, at));
                            if width > e.0 {
                                *e = (width, at);
                            }
                            c.in_view.push(InView {
                                copy: copy as u32,
                                cell: i as u32,
                                width,
                                at,
                            });
                        }
                    }
                }
            }
            c.best = best
                .into_iter()
                .map(|(cell, (width, at))| Best {
                    cell: cell as u32,
                    width,
                    at,
                })
                .collect();
            c.best.sort_by_key(|b| b.cell);
            c.ring = ring
                .into_iter()
                .map(|(cell, at)| Ring {
                    cell: cell as u32,
                    at,
                })
                .collect();
            c.ring.sort_by_key(|r| r.cell);
            ring_unbuilt.retain(|i| !unbuilt.contains(i));
            c.unbuilt = unbuilt.into_iter().map(|i| i as u32).collect();
            c.unbuilt.sort_unstable();
            c.ring_unbuilt = ring_unbuilt.into_iter().map(|i| i as u32).collect();
            c.ring_unbuilt.sort_unstable();
            c
        }

        fn view(eye: Vec3, target: Vec3, fov: f32, shifts: Vec<f32>) -> LiveView {
            let (near, far, t) = (10.0f32, 20000.0f32, (fov * 0.5).tan());
            let proj = Mat4::from_cols_array(&[
                t.recip() * 9.0 / 16.0,
                0.0,
                0.0,
                0.0,
                0.0,
                t.recip(),
                0.0,
                0.0,
                0.0,
                0.0,
                far / (near - far),
                -1.0,
                0.0,
                0.0,
                near * far / (near - far),
                0.0,
            ]);
            let f = (target - eye).normalize();
            let s = f.cross(Vec3::Z).normalize();
            let u = s.cross(f);
            let look = Mat4::from_cols_array(&[
                s.x,
                u.x,
                -f.x,
                0.0,
                s.y,
                u.y,
                -f.y,
                0.0,
                s.z,
                u.z,
                -f.z,
                0.0,
                -s.dot(eye),
                -u.dot(eye),
                f.dot(eye),
                1.0,
            ]);
            LiveView {
                view_proj_orig: proj * look,
                eye_orig: eye,
                screen_width: 1920.0,
                shifts,
                near: None,
                focus: None,
            }
        }

        #[test]
        fn cached_cull_matches_the_brute_force_scan() {
            let w = CX as f32 * STEP;
            let blocks = Blocks::build(CX * CY, CX, bounds_of);
            let mut cached = Cull::new(CX * CY);
            let views = [
                view(
                    Vec3::new(1280.0, -400.0, 900.0),
                    Vec3::new(1280.0, 960.0, 0.0),
                    0.8,
                    vec![0.0],
                ),
                view(
                    Vec3::new(100.0, 500.0, 600.0),
                    Vec3::new(300.0, 700.0, 0.0),
                    0.6,
                    vec![0.0, -w, w],
                ),
                view(
                    Vec3::new(2500.0, 1500.0, 2500.0),
                    Vec3::new(1300.0, 900.0, 0.0),
                    1.1,
                    vec![0.0, -w, w],
                ),
                view(
                    Vec3::new(-50.0, 900.0, 300.0),
                    Vec3::new(500.0, 900.0, 0.0),
                    0.5,
                    vec![0.0, w],
                ),
                view(
                    Vec3::new(5000.0, 5000.0, 400.0),
                    Vec3::new(9000.0, 5000.0, 0.0),
                    0.4,
                    vec![0.0],
                ),
            ];
            for v in &views {
                cached.rebuild(&blocks, bounds_of, kind_of, v, 7, Vec2::ZERO);
                let want = brute(v);
                assert_eq!(cached.in_view, want.in_view);
                assert_eq!(cached.best, want.best);
                let mut ring = cached.ring.clone();
                ring.sort_by_key(|r| r.cell);
                assert_eq!(ring, want.ring);
                assert_eq!(cached.unbuilt, want.unbuilt);
                assert_eq!(cached.ring_unbuilt, want.ring_unbuilt);
                for r in &want.ring {
                    assert!(cached.flags[r.cell as usize] & F_RING != 0);
                }
                assert!(cached.is_current(v, 7, Vec2::ZERO));
                // Another reach ahead of the camera culls again.
                assert!(!cached.is_current(v, 7, Vec2::new(0.5, 0.0)));
                assert!(!cached.is_current(v, 8, Vec2::ZERO));
                // Reaching ahead adds to the ring and changes nothing in view.
                let mut ahead = Cull::new(CX * CY);
                ahead.rebuild(&blocks, bounds_of, kind_of, v, 7, Vec2::new(0.5, 0.0));
                assert_eq!(ahead.in_view, cached.in_view);
                assert_eq!(ahead.unbuilt, cached.unbuilt);
                let far: HashSet<u32> = ahead.ring.iter().map(|r| r.cell).collect();
                assert!(cached.ring.iter().all(|r| far.contains(&r.cell)));
                assert!(ahead.is_current(v, 7, Vec2::new(0.55, 0.05)));
            }
        }

        #[test]
        fn a_low_eye_gets_full_pages_for_the_cells_it_stands_in_and_beside() {
            let blocks = Blocks::build(CX * CY, CX, bounds_of);
            // A drawn cell in the middle of the map, and an eye 6.5 up inside its box, looking along +Y.
            let home = (0..CX * CY)
                .find(|&i| i % CX == 20 && i / CX > 8 && kind_of(i) == Kind::Drawn)
                .expect("a drawn cell");
            let min = bounds_of(home).min;
            let eye = Vec3::new(min.x + 30.0, min.y + 30.0, 6.5);
            let mut v = view(eye, eye + Vec3::Y * 100.0, 1.3, vec![0.0]);
            v.near = Some(10.0);
            let mut cull = Cull::new(CX * CY);
            cull.rebuild(&blocks, bounds_of, kind_of, &v, 1, Vec2::ZERO);
            let want = brute(&v);
            assert_eq!(cull.in_view, want.in_view);
            assert_eq!(cull.best, want.best);
            let at_home = cull
                .best
                .iter()
                .find(|b| b.cell as usize == home)
                .expect("the cell under the eye is in view");
            assert!(
                crate::world::terrain::cells::page_size_for_width(at_home.width, 64) == 512,
                "{}",
                at_home.width
            );
            // The cull of another near plane is another cull.
            let mut other = v.clone();
            other.near = None;
            assert!(!cull.is_current(&other, 1, Vec2::ZERO));
            assert!(cull.is_current(&v, 1, Vec2::ZERO));
        }

        #[test]
        fn the_cull_finds_cells_and_skips_blocks() {
            let w = CX as f32 * STEP;
            let v = view(
                Vec3::new(1280.0, -400.0, 900.0),
                Vec3::new(1280.0, 960.0, 0.0),
                0.8,
                vec![0.0, -w, w],
            );
            let want = brute(&v);
            assert!(!want.best.is_empty());
            assert!(
                want.best.len() < CX * CY / 2,
                "the view is a part of the map"
            );
        }
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[test]
    fn the_pages_of_a_frame_are_bounded_by_their_gpu_time() {
        // Not measured (WebGL2, no timestamps): the count.
        assert_eq!(time_bounded(8, 0.0), 8);
        assert_eq!(time_bounded(8, f32::NAN), 8);
        // Half a millisecond a page: all eight fit in 4 ms.
        assert_eq!(time_bounded(8, 0.5), 8);
        // 1.5 ms a page: two fit.
        assert_eq!(time_bounded(8, 1.5), 2);
        // A page that alone takes longer than the frame's share: still one a frame.
        assert_eq!(time_bounded(8, 30.0), 1);
        assert_eq!(time_bounded(1, 0.1), 1);
    }
}

#[cfg(test)]
mod gpu_tests {
    //! A small known map through the whole path: the field, the cache, the bake, the draw (packet
    //! goal of R2.13), and the decals baked into its pages (R2.14). The renderer is the real one with
    //! the boot group; the data is the synthetic library, handed over through the same calls the client
    //! uses.

    use asset_format::{DecalDef, Extra, LogicalId, PieceStyle, TextureBlob, TextureFormat};
    use draw_list::UiDrawList;
    use glam::Vec2;

    use crate::frame::mask::{MASK_FORMAT, class};
    use crate::frame::readback::FrameSource;
    use crate::game_camera::GameCamera;
    use crate::gpu_test::{Gpu, gpu, read_texture};
    use crate::hexgrid::cell_center;
    use crate::lighting::tests::{boot_blobs_with, settings};
    use crate::renderer::Renderer;
    use crate::scene::Scene;
    use crate::terrain::{Fog, TerrainCell, TerrainMesh, TerrainSnapshot};
    use crate::world::decal::DecalCategory;
    use crate::world::terrain::basis::{PlotBasis, TerrainBasis};
    use crate::world::terrain::cells::CellGrid;
    use crate::world::terrain::decals::{DecalVertex, PASS_LAYER};
    use crate::world::terrain::heights_gpu::{
        HeightTiles, TILE_SIDE, height_byte, tile_of_cell, tile_origin,
    };
    use crate::world::terrain::library::TerrainLibrary;

    const SIZE: (u32, u32) = (128, 128);

    /// The renderer with a 7 x 5 map of plain land, seen from above the hex (3, 2).
    struct Live {
        gpu: Gpu,
        r: Renderer,
        camera: GameCamera,
        target: wgpu::Texture,
        view: wgpu::TextureView,
        size: (u32, u32),
    }

    impl Live {
        fn new(
            name: &str,
            decals: Vec<DecalDef>,
            textures: Vec<(LogicalId, TextureBlob)>,
        ) -> Option<Self> {
            Self::with_wrap(name, decals, textures, false, SIZE)
        }

        fn with_wrap(
            name: &str,
            decals: Vec<DecalDef>,
            textures: Vec<(LogicalId, TextureBlob)>,
            wrap: bool,
            size: (u32, u32),
        ) -> Option<Self> {
            let gpu = gpu(name, wgpu::Backends::PRIMARY, wgpu::Features::empty())?;
            let format = wgpu::TextureFormat::Rgba8Unorm;
            let mut r = Renderer::offscreen(&gpu, size, format, 1);
            r.set_boot_required(true);
            for (file, b) in boot_blobs_with(&[], &|_, _| [120, 120, 120, 255]) {
                r.add_lighting_texture(&file, &b).unwrap();
            }
            // The data of the terrain, as the client hands it over.
            let lib = TerrainLibrary::synthetic();
            let mut ws = settings();
            ws.terrain_defs = lib.defs().rows().to_vec();
            ws.decals = decals;
            r.set_world_settings(ws);
            for style in PieceStyle::ALL {
                r.add_terrain_piece_set((**lib.piece_set(style).unwrap()).clone());
            }
            for row in lib.defs().rows() {
                for id in [&row.diffuse, &row.heightmap, &row.specmap]
                    .into_iter()
                    .flatten()
                {
                    if let Some(blob) = lib.texture(id) {
                        r.add_terrain_def_texture(id.clone(), blob.clone());
                    }
                }
            }
            for (id, blob) in textures {
                r.add_terrain_def_texture(id, blob);
            }
            let (w, h) = (7u32, 5u32);
            let cells = (0..w * h)
                .map(|_| TerrainCell {
                    fog: Fog::Visible,
                    ..TerrainCell::default()
                })
                .collect();
            let snapshot = TerrainSnapshot::new(w, h, wrap, cells).unwrap();
            r.set_terrain(&snapshot, &TerrainMesh::build(&snapshot));
            r.set_terrain_basis(TerrainBasis::uniform(
                w as u16,
                h as u16,
                wrap,
                1,
                PlotBasis::default(),
            ));
            let mut camera = GameCamera::new(cell_center(3, 2));
            camera.set_level(10.0);
            let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("terrain live target"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = target.create_view(&wgpu::TextureViewDescriptor::default());
            Some(Self {
                gpu,
                r,
                camera,
                target,
                view,
                size,
            })
        }

        fn frame(&mut self) {
            let commands = self.r.encode_frame(
                &self.view,
                &Scene::test(0.0),
                &self.camera,
                &UiDrawList::default(),
                None,
            );
            self.gpu.queue.submit([commands]);
            self.r.frame_submitted();
        }

        /// Frames until the pages of the visible cells are baked and nothing is left to do.
        fn settle(&mut self) {
            for _ in 0..120 {
                self.frame();
                let s = self.r.terrain_system().live_stats();
                if s.baked > 0
                    && s.drawn > 0
                    && self.r.terrain_system().pending_work() == 0
                    && self.jobs_left() == 0
                {
                    break;
                }
            }
            for _ in 0..3 {
                self.frame();
            }
        }

        /// Frames until the work the last change made is done.
        fn work_off(&mut self) {
            self.frame();
            for _ in 0..120 {
                if self.r.terrain_system().pending_work() == 0 && self.jobs_left() == 0 {
                    break;
                }
                self.frame();
            }
            self.frame();
        }

        /// Pages baked again after a change (`rebaked`), not counting first bakes or the background
        /// ones that the wall-clock credit of the frame makes (`LiveStats::baked` counts those too).
        fn rebakes(&self) -> u64 {
            self.r
                .terrain_system()
                .live()
                .expect("the live map")
                .cache
                .counters()
                .rebaked
        }

        /// Bake jobs still queued. The frame bakes against a CPU time bound (a loaded machine bakes
        /// fewer pages a frame), so "baked > 0" alone does not mean the cell under test has its page.
        fn jobs_left(&self) -> usize {
            self.r
                .terrain_system()
                .live()
                .map_or(0, |l| l.cache.jobs().count())
        }

        fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
            let px = read_texture(&self.gpu, &self.target, 0, 0, self.size, 4);
            let at = ((y * self.size.0) + x) as usize * 4;
            [px[at], px[at + 1], px[at + 2]]
        }
    }

    #[test]
    fn a_small_map_is_drawn_from_baked_pages_and_marked_as_land() {
        let Some(mut live) = Live::new("terrain live", vec![], vec![]) else {
            return;
        };
        live.settle();
        let stats = live.r.terrain_system().live_stats();
        assert!(stats.baked > 0, "pages were baked: {stats:?}");
        assert!(stats.drawn > 0, "cells were drawn: {stats:?}");
        assert_eq!(stats.failed, 0, "{stats:?}");
        // The cells the overlays draw again are the cells drawn.
        let geometry = live.r.terrain_system().cell_geometry().expect("geometry");
        assert_eq!(geometry.copies(), [0.0]);
        assert_eq!(geometry.cells(0.0), stats.drawn);
        assert!(!geometry.height_tiles().is_empty());
        let mask = live.r.read_frame(FrameSource::ClassMask);
        live.frame();
        let image = mask.wait().expect("the class mask");
        assert_eq!(image.format, MASK_FORMAT);
        let land = image
            .bytes
            .iter()
            .filter(|&&b| u32::from(b) & 0x3f == class::LAND)
            .count();
        assert!(
            land * 4 > image.bytes.len(),
            "the ground is the class land over the middle of the picture: {land} of {}",
            image.bytes.len()
        );
        // The picture is not the clear colour: the pages were sampled.
        assert_ne!(
            live.pixel(SIZE.0 / 2, SIZE.1 / 2),
            [5, 8, 15],
            "the middle is not the clear colour"
        );
    }

    /// The batched draw (render-opt B3: the cells of an array of pages in one instanced call, the
    /// layer from the instance) gives the picture of one draw a cell bit for bit, over hills (pages
    /// that differ), a wrapping map (copies) and a view wide enough for several page sizes.
    #[test]
    fn the_batched_cells_draw_the_picture_of_one_draw_a_cell() {
        use crate::terrain::Relief;
        let size = (256, 256);
        let Some(mut live) = Live::with_wrap("terrain batched", vec![], vec![], true, size) else {
            return;
        };
        let mut b = TerrainBasis::uniform(7, 5, true, 1, PlotBasis::default());
        for (i, p) in b.plots.iter_mut().enumerate() {
            if i % 3 != 1 {
                p.relief = Relief::Hill;
            }
        }
        live.r.set_terrain_basis(b);
        live.camera = GameCamera::new(cell_center(0, 2));
        live.camera.set_level(14.0);
        live.work_off();
        live.settle();
        let batched = live
            .r
            .terrain_system_mut()
            .cell_drawer_mut()
            .is_some_and(|d| d.batched());
        if !batched {
            // GL draws a cell at a time anyway.
            return;
        }
        let drawn = live.r.terrain_system().live_stats().drawn;
        live.frame();
        let with_batches = read_texture(&live.gpu, &live.target, 0, 0, size, 4);
        let counters = live.r.terrain_system().terrain_counters_for_test();
        let get = |k: &str| counters.iter().find(|(n, _)| *n == k).map(|(_, v)| *v);
        let (cells, calls) = (get("terrain.cells.drawn"), get("terrain.cells.draw_calls"));
        assert!(drawn > 1, "several cells: {drawn}");
        assert!(
            calls < cells && calls > Some(0),
            "fewer calls than cells: {calls:?} of {cells:?}"
        );
        live.r
            .terrain_system_mut()
            .cell_drawer_mut()
            .unwrap()
            .set_batched(false);
        live.frame();
        live.frame();
        let per_cell = read_texture(&live.gpu, &live.target, 0, 0, size, 4);
        let counters = live.r.terrain_system().terrain_counters_for_test();
        let get = |k: &str| counters.iter().find(|(n, _)| *n == k).map(|(_, v)| *v);
        assert_eq!(get("terrain.cells.drawn"), get("terrain.cells.draw_calls"));
        let differ = with_batches
            .chunks_exact(4)
            .zip(per_cell.chunks_exact(4))
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(differ, 0, "pixels that differ between the two draws");
    }

    /// Writes the picture as a PPM into `CIV5_DUMP_DIR` when it is set (the before/after crops).
    fn dump(live: &Live, name: &str) {
        let Some(dir) = std::env::var_os("CIV5_DUMP_DIR") else {
            return;
        };
        let px = read_texture(&live.gpu, &live.target, 0, 0, live.size, 4);
        let mut out = format!("P6\n{} {}\n255\n", live.size.0, live.size.1).into_bytes();
        for p in px.chunks_exact(4) {
            out.extend_from_slice(&p[..3]);
        }
        let _ = std::fs::write(std::path::Path::new(&dir).join(format!("{name}.ppm")), out);
    }

    /// The seam of a wrapping map (feedback of 2026-10-28: "a gap and an abyss where one end of the map
    /// joins the other"): the last column of cells reaches the first texel of the next copy, so the
    /// ground is closed across the seam and the clear colour does not show through it.
    #[test]
    fn the_ground_of_a_wrapping_map_is_closed_across_the_seam() {
        use crate::terrain::Relief;
        let size = (512, 512);
        let Some(mut live) = Live::with_wrap("terrain seam", vec![], vec![], true, size) else {
            return;
        };
        let mut b = TerrainBasis::uniform(7, 5, true, 1, PlotBasis::default());
        for (i, p) in b.plots.iter_mut().enumerate() {
            if i % 3 != 1 {
                p.relief = Relief::Hill;
            }
        }
        live.r.set_terrain_basis(b);
        // Over the seam: texel 0 is half a hex west of the centre of the first column.
        let c = cell_center(0, 2);
        live.camera = GameCamera::new(Vec2::new(c.x - 32.0, c.y));
        live.camera.set_level(11.0);
        live.work_off();
        live.settle();
        dump(&live, "seam");
        // A gap is a thin line of the colour behind the ground: a pixel unlike the two pixels three
        // to either side of it, which are like each other.
        let px = read_texture(&live.gpu, &live.target, 0, 0, size, 4);
        let grey = |x: u32, y: u32| i32::from(px[((y * size.0) + x) as usize * 4]);
        let mut holes = Vec::new();
        for y in size.1 / 2..size.1 * 3 / 4 {
            for x in 3..size.0 - 3 {
                let (l, m, r) = (grey(x - 3, y), grey(x, y), grey(x + 3, y));
                if (l - r).abs() <= 8 && (m - l).abs() > 20 && (m - r).abs() > 20 && holes.len() < 8
                {
                    holes.push((x, y));
                }
            }
        }
        assert!(holes.is_empty(), "the ground has a gap at {holes:?}");
    }

    /// AC-R8.1 for the ground: the presets switch while the map stays (no new map, no new base);
    /// the pages are made again under the new keys, and High comes back as it was.
    #[test]
    fn a_preset_change_makes_the_pages_again_and_high_comes_back_the_same() {
        use crate::settings::{Preset, RenderSettings};
        let Some(mut live) = Live::new("terrain presets", vec![], vec![]) else {
            return;
        };
        live.settle();
        let first = live.pixel(SIZE.0 / 2, SIZE.1 / 2);
        for preset in [Preset::Low, Preset::Medium, Preset::High] {
            live.r.set_settings(RenderSettings::preset(preset));
            assert_eq!(live.r.settings(), &RenderSettings::preset(preset));
            live.work_off();
            live.settle();
            let s = live.r.terrain_system().live_stats();
            assert!(s.drawn > 0 && s.failed == 0, "{preset:?}: {s:?}");
            // The map was not built again, the pages were: the live map starts its counters anew.
            assert!(s.baked > 0, "{preset:?}: pages were baked again: {s:?}");
            assert_ne!(
                live.pixel(SIZE.0 / 2, SIZE.1 / 2),
                [5, 8, 15],
                "{preset:?}: the middle is not the clear colour"
            );
        }
        assert_eq!(
            live.pixel(SIZE.0 / 2, SIZE.1 / 2),
            first,
            "High after Low and Medium is the High of the start"
        );
    }

    /// A map built again (another basis; the same happens when a texture of the library comes late)
    /// draws its own heights: the drawer outlives the live map and must not keep the groups of the
    /// tiles of the map before. They did (every live map had the same draw generation), and a tile
    /// whose group was made before the rebuild drew the old heights beside a tile made after it: a long
    /// straight step at the tile border with the void under it (the seam of game 18, 2026-10-28).
    #[test]
    fn a_map_built_again_draws_the_heights_of_the_new_build() {
        use crate::terrain::Relief;
        // Every other hex a hill: slopes the pages shade.
        let hills = || {
            let mut b = TerrainBasis::uniform(7, 5, false, 1, PlotBasis::default());
            for (i, p) in b.plots.iter_mut().enumerate() {
                if i % 2 == 0 {
                    p.relief = Relief::Hill;
                }
            }
            b
        };
        let Some(mut live) = Live::new("terrain rebuild", vec![], vec![]) else {
            return;
        };
        // The plain map is drawn first, so the drawer has the group of its tile; then the map is built
        // again with hills.
        live.settle();
        live.r.set_terrain_basis(hills());
        live.work_off();
        live.settle();
        let terrain = live.r.terrain_system();
        let geometry = terrain.cell_geometry().expect("geometry");
        let keys = geometry.drawer.tile_group_keys();
        assert!(!keys.is_empty());
        let sampler = terrain.sampler().expect("the field");
        let grid = CellGrid::new(sampler.frame());
        for tile in keys {
            let ours = &geometry
                .heights
                .tile(tile)
                .expect("the tile of the map")
                .texture;
            let bound = geometry.drawer.tile_group_texture(tile).unwrap();
            assert!(
                bound == &**ours,
                "the group of tile {tile:?} binds a texture of the map before"
            );
            // What the draw reads is the surface of the field: every texel of every cell, the
            // shared border rows and columns of two cells included.
            let bytes = read_texture(&live.gpu, bound, 0, 0, (TILE_SIDE, TILE_SIDE), 2);
            let (ox, oy) = tile_origin(tile);
            let mut wrong = Vec::new();
            for cy in 0..grid.cells_y {
                for cx in 0..grid.cells_x {
                    if tile_of_cell(cx, cy) != tile {
                        continue;
                    }
                    let b = grid.cell_box(cx, cy);
                    let surface = HeightTiles::cell_surface(sampler, &b);
                    for v in b.y..b.y + b.h {
                        for u in b.x..b.x + b.w {
                            let i = (((v - oy) * TILE_SIDE as i32 + (u - ox)) * 2) as usize;
                            let gpu = u16::from_le_bytes([bytes[i], bytes[i + 1]]);
                            let field = surface[((v - b.y) * b.w + (u - b.x)) as usize];
                            if gpu != field && wrong.len() < 8 {
                                wrong.push((u, v, gpu, field));
                            }
                        }
                    }
                }
            }
            assert!(
                wrong.is_empty(),
                "tile {tile:?}: (u, v, gpu, field) {wrong:?}"
            );
        }
        // The hills are in the field the draw reads (raw 255ths: a plain is 60, a hill near 100).
        let top = (0..grid.cells_x)
            .map(|cx| height_byte(sampler.raw_height(cx * 64 + 32, 32)))
            .max()
            .unwrap();
        assert!(top > 80, "{top}");
    }

    /// The pass of the cells of the overlay runs on the cells the terrain draws (no triangles from
    /// outside), one draw for each cell.
    #[test]
    fn the_screen_overlay_pass_draws_through_the_cells_of_the_terrain() {
        use crate::frame::RenderSystem;
        let Some(mut live) = Live::new("terrain overlay cells", vec![], vec![]) else {
            return;
        };
        live.settle();
        live.r.set_grid_visible(true);
        for _ in 0..3 {
            live.frame();
        }
        assert!(live.r.world().overlay.screen_overlay_active());
        let counters: std::collections::HashMap<_, _> =
            live.r.world().overlay.counters().into_iter().collect();
        let drawn = live.r.terrain_system().live_stats().drawn;
        assert!(counters["overlay.map_passes"] > 0, "{counters:?}");
        assert_eq!(counters["overlay.cell_draws"], drawn as u64, "{counters:?}");
        // The grid went into the map of the overlay (the line over the ground is not drawn then).
        assert!(counters["overlay.grid_hexes"] > 0, "{counters:?}");
    }

    fn red() -> TextureBlob {
        let rgba = [255u8, 0, 0, 255];
        let mips = [4usize, 2, 1].iter().map(|&n| rgba.repeat(n * n)).collect();
        TextureBlob::new_2d(TextureFormat::Rgba8, false, 4, 4, mips)
    }

    #[test]
    fn a_decal_is_baked_into_the_pages_of_the_cells_it_covers_and_only_there() {
        let id = LogicalId::new("tx:test/crops.dds");
        let types = vec![DecalDef {
            kind: "Crops".into(),
            name: "Crops_01".into(),
            albedo: Some(id.clone()),
            heightmap: None,
            priority: None,
            extra: Extra::default(),
        }];
        let Some(mut live) = Live::new("terrain decals", types, vec![(id, red())]) else {
            return;
        };
        live.settle();
        let baked_before = live.r.terrain_system().live_stats().baked;
        let rebakes_before = live.rebakes();
        // A red patch over the middle hex, 60 units a side.
        let c = cell_center(3, 2);
        let corners = [(-30.0, -30.0), (30.0, -30.0), (30.0, 30.0), (-30.0, 30.0)];
        let verts: Vec<DecalVertex> = corners
            .iter()
            .zip([(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)])
            .map(|(&(dx, dz), (u, v))| DecalVertex {
                pos: c + Vec2::new(dx, dz),
                uv: Vec2::new(u, v),
            })
            .collect();
        let handle = live
            .r
            .terrain_decals_mut()
            .create_typed(
                DecalCategory::of_name("Crops").unwrap(),
                0,
                verts,
                vec![[0, 1, 2], [0, 2, 3]],
                PASS_LAYER,
            )
            .expect("a decal");
        live.work_off();
        let rebaked = live.rebakes() - rebakes_before;
        assert!(rebaked >= 1, "the pages under the decal were baked again");
        assert!(
            rebaked < baked_before,
            "not every page was baked again: {rebaked} of {baked_before}"
        );
        // Taking it away bakes the same pages again.
        let rebakes_mid = live.rebakes();
        assert!(live.r.terrain_decals_mut().destroy(handle));
        live.work_off();
        let again = live.rebakes() - rebakes_mid;
        assert_eq!(again, rebaked, "the pages under the decal were baked again");
        assert_eq!(live.r.terrain_system().decals().len(), 0);
    }

    /// AC-T1: the decals of a producer given again as they were bake nothing; a change bakes the pages
    /// under it again, and the cells stay in the draw list every frame while their jobs wait.
    #[test]
    fn the_same_decals_again_bake_nothing_and_a_rebake_leaves_no_hole() {
        use crate::world::terrain::decals::{DecalShape, DecalSource, ObjectDecal};
        let id = LogicalId::new("tx:test/crops.dds");
        let types = vec![DecalDef {
            kind: "Crops".into(),
            name: "Crops_01".into(),
            albedo: Some(id.clone()),
            heightmap: None,
            priority: None,
            extra: Extra::default(),
        }];
        let Some(mut live) = Live::new("terrain decals again", types, vec![(id, red())]) else {
            return;
        };
        live.settle();
        let c = cell_center(3, 2);
        let farm = |half: f32| ObjectDecal {
            category: DecalCategory::of_name("Crops").unwrap(),
            texture: None,
            shape: DecalShape::Quad([
                c + Vec2::new(-half, -half),
                c + Vec2::new(half, -half),
                c + Vec2::new(half, half),
                c + Vec2::new(-half, half),
            ]),
            cell: 17,
            casts_shadow: false,
        };
        let give = |live: &mut Live, half: f32| {
            live.r.terrain_decals_mut().set_object_decals(
                DecalSource::Farms,
                &[17],
                vec![farm(half)],
            );
        };
        give(&mut live, 30.0);
        live.work_off();
        let rebakes = |live: &Live| live.rebakes();
        let before = rebakes(&live);
        assert!(before > 0, "the first farm bakes the pages under it again");
        // The turn ends and the farm comes again, the same.
        give(&mut live, 30.0);
        live.work_off();
        assert_eq!(rebakes(&live), before, "nothing changed, nothing baked");
        // A bigger farm: the pages under it are baked again, and no cell drops out of the draw.
        let drawn = live.r.terrain_system().live_stats().drawn;
        give(&mut live, 40.0);
        for _ in 0..10 {
            live.frame();
            let s = live.r.terrain_system().live_stats();
            assert_eq!(s.drawn, drawn, "a cell left the draw while baked again");
            assert_eq!(s.unpaged, 0);
        }
        assert!(rebakes(&live) > before);
    }
}
