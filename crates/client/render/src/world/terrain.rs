//! `Terrain` subsystem: the ground from pieces,
//! the cache of cells and their light, shadows. The module is built in packages; this file holds
//! what each of them has put in so far.
//!
//! - WT-0 (R1.17, the milestone M1): the quality keys ([`settings_map`]), and the light of the
//!   original on the geometry of B1 ([`m1`]).
//! - WT-1 (R2.8): the library of pieces, `TerrainDef` and textures ([`library`]).
//! - WT-2 (R2.8): the field, the sampler and the frame ([`field`], [`sampler`], [`stencil`]), the
//!   basis of the map ([`basis`]) and the build with its stages ([`build`]). The subsystem takes
//!   a basis, builds the field of the cells the fog opens, a few cells per call of `prepare`, and
//!   answers `sampler()`, `frame()`, `height_revision()`, `load_progress()`.
//!
//! - WT-4 (R2.13): the cells, the cache and the bake driven live, the draw of the cells from their
//!   pages ([`live`], [`draw`], [`heights_gpu`]), the pick on the field ([`pick`]).
//!
//! The ground of B1 is drawn by the adapter `frame::b1_systems::B1Terrain`, which stands in for this
//! subsystem in the frame plan, until the terrain through the cache can be drawn: the renderer calls
//! [`Terrain::live_frame`] every frame it waits for the boot group, and when it answers `true` (the
//! boot group is in, the map is known, the textures of its styles are in the library) the adapter
//! leaves the plan and this subsystem bakes ("TerrainBake") and draws ("Main.Terrain"). Without the
//! boot group the adapter draws as before, through [`m1::TerrainM1`] or B1's own pipeline. The field is
//! built by [`Terrain::pump`]; the world frame reaches the frame constants when a map is set. The
//! subsystem reads the keys: `on_settings` makes [`settings_map::TerrainSettings`] again when one of
//! them changes (AC-R8.1).

pub mod bake;
pub mod basis;
pub mod build;
pub mod cache;
pub mod cells;
pub mod decal_pass;
pub mod decals;
pub mod draw;
pub mod field;
pub mod heights_gpu;
pub mod library;
pub mod live;
pub mod m1;
pub mod pages;
pub mod pick;
pub mod sampler;
pub mod settings_map;
pub mod shadow_pass;
pub mod stencil;

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

/// CPU time of the raster of the field cells ([`Terrain::pump`], microseconds) and the cells timed
/// (`terrain.raster_us_total`, `terrain.raster_cells_total`). Their ratio is the bench's slow-state
/// probe (`scene.slowTier`: about twice as slow on an efficiency core).
static RASTER_US: AtomicU64 = AtomicU64::new(0);
static RASTER_CELLS: AtomicU64 = AtomicU64::new(0);

use glam::{Mat4, Vec3};

pub use basis::{BasisFeature, BasisFlags, NwPiece, PlotBasis, TerrainBasis};
pub use build::{
    BuildError, BuildProducts, BuildStage, FieldWindow, RasterCx, RasterScope, StageCx, StageId,
    TerrainBuild,
};
pub use draw::{LiveConfig, LiveLights};
pub use field::{CELL_SIZE, CellRect, TerrainField};
pub use library::TerrainLibrary;
pub use live::{LiveInputs, LiveStats, LiveView};
pub use m1::{ClassRanges, TerrainM1, env_class, partition_by_class};
pub use sampler::{TerrainFrame, TerrainSampler};
pub use settings_map::TerrainSettings;
pub use stencil::TerrainStencil;

use crate::frame::{
    DrawContext, EncoderContext, InitContext, MainSlot, MaskContext, PassId, PassSlot,
    PrepareContext, RenderSystem, SystemId,
};
use crate::settings::{KeySet, RenderSettings};
use crate::terrain::Fog;
use crate::terrain_detail::TerrainDetail;

/// Cells a call of `prepare` builds: the original builds four per call while it loads.
// re: Terrain.cpp:1275-1349
pub const CELLS_PER_PREPARE: usize = 4;

/// The share of a frame the field builds in ([`Terrain::pump_for_frame`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FieldShare {
    /// Open cells in view wait for the field: these milliseconds ([`live::FIELD_FRAME_MS`] while the
    /// map loads, [`live::FIELD_VIEW_MS`] once a view was complete).
    View(f64),
    /// Only cells of the ring around the view wait for it: a few milliseconds.
    Ring,
    /// The view and its ring are built: the rest of the map in the background, in these milliseconds.
    Background(f64),
}

/// The map the subsystem works on.
struct MapState {
    basis: TerrainBasis,
    sampler: TerrainSampler,
    stencil: TerrainStencil,
    /// Cells along X and Y: one per hex.
    cells_x: i32,
    cells_y: i32,
    /// Cells the fog opens (an open hex and the ring of one cell), and the ones built.
    wanted: Vec<bool>,
    done: Vec<bool>,
    /// Counts the times a cell was built: the cache sees that the field of a cell changed.
    cell_rev: Vec<u32>,
    /// The revision of the heights of the field (`TerrainSampler::height_revision`) after the last
    /// build of each cell that changed a height in it; 0 for a cell whose heights never changed.
    cell_height_rev: Vec<u64>,
    /// Cells built since the basis was set (`terrain.field.built_total`).
    built_total: u64,
    pending: VecDeque<(i32, i32)>,
    /// Cells put at the front of `pending` because they are in view ([`MapState::hurry`]), once each.
    hurried: Vec<bool>,
    /// The cell the queue was last put in order from (nearest first).
    sorted_focus: Option<(i32, i32)>,
    wanted_count: usize,
    done_count: usize,
    error: Option<BuildError>,
    /// The continents (`continent_art`) the hexes have, and the generic one.
    continents: Vec<u8>,
}

impl MapState {
    fn index(&self, cx: i32, cy: i32) -> usize {
        (cy * self.cells_x + cx) as usize
    }

    /// Wants a cell: builds it if it is not built.
    fn want(&mut self, cx: i32, cy: i32) {
        let i = self.index(cx, cy);
        if !self.wanted[i] {
            self.wanted[i] = true;
            self.wanted_count += 1;
            if !self.done[i] {
                self.pending.push_back((cx, cy));
            }
        }
    }

    /// A cell in view that is not built goes to the front of the queue, with the cells around it (a cell
    /// is made ready when its neighbours are built too). The queue keeps its old place too; the build
    /// skips a cell that is done. Returns whether the cell waits for the field: it or a cell around it
    /// is wanted and not built (a cell that is built and waits only to be made ready does not).
    fn hurry(&mut self, index: usize) -> bool {
        let (cx, cy) = ((index as i32) % self.cells_x, (index as i32) / self.cells_x);
        let mut waits = false;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (x, y) = (cx + dx, cy + dy);
                if !(0..self.cells_x).contains(&x) || !(0..self.cells_y).contains(&y) {
                    continue;
                }
                let i = self.index(x, y);
                if self.wanted[i] && !self.done[i] {
                    waits = true;
                    if !self.hurried[i] {
                        self.hurried[i] = true;
                        self.pending.push_front((x, y));
                    }
                }
            }
        }
        waits
    }

    /// The queue nearest first from the cell `focus` (across the seam of a map that wraps), each cell
    /// once, when the focus has moved [`live::BACKGROUND_REFOCUS`] cells or more since the last time:
    /// the field out of view builds outwards from the camera.
    fn sort_pending(&mut self, focus: (i32, i32), wrap_x: bool) {
        let r = live::BACKGROUND_REFOCUS * 2;
        if self.pending.is_empty()
            || self
                .sorted_focus
                .is_some_and(|(x, y)| (x - focus.0).abs() < r && (y - focus.1).abs() < r)
        {
            return;
        }
        self.sorted_focus = Some(focus);
        let cells_x = self.cells_x;
        let dist = |(x, y): (i32, i32)| {
            let mut dx = (x - focus.0).abs();
            if wrap_x {
                dx = dx.min(cells_x - dx);
            }
            let dy = y - focus.1;
            i64::from(dx) * i64::from(dx) + i64::from(dy) * i64::from(dy)
        };
        let mut seen = vec![false; self.done.len()];
        let mut order: Vec<(i32, i32)> = Vec::with_capacity(self.pending.len());
        for (x, y) in self.pending.drain(..) {
            let i = (y * cells_x + x) as usize;
            if !self.done[i] && !seen[i] {
                seen[i] = true;
                order.push((x, y));
            }
        }
        order.sort_unstable_by_key(|&c| (dist(c), c.1, c.0));
        self.pending = order.into();
        self.hurried.iter_mut().for_each(|h| *h = false);
    }

    /// The cells a rectangle of texels touches.
    fn cells_of(&self, rect: &CellRect) -> Vec<(i32, i32)> {
        rect.cells(self.cells_x, self.cells_y, self.sampler.frame().wrap_x())
    }
}

/// The cells the last frame drew, for a pass of another module that draws the ground again with its
/// own pixel stage (the overlays, the fog of war). The vertex stage is `terrain_shader_vs`: its
/// group 2 is [`CellGeometry::vs_layout`], its buffers are [`CellDrawer::vertex_layouts`].
pub struct CellGeometry<'a> {
    drawer: &'a draw::CellDrawer,
    heights: &'a heights_gpu::HeightTiles,
}

impl CellGeometry<'_> {
    /// The shifts of the copies of the map in X that have cells.
    pub fn copies(&self) -> &[f32] {
        self.drawer.copy_shifts()
    }

    /// Visible cells in a copy.
    pub fn cells(&self, copy_shift: f32) -> usize {
        self.drawer.cells_in_copy(copy_shift)
    }

    /// The layout of group 2 of the vertex stage.
    pub fn vs_layout(&self) -> &wgpu::BindGroupLayout {
        self.drawer.vs_layout()
    }

    /// The tiles of heights on the GPU (the same the cells read).
    pub fn height_tiles(&self) -> &heights_gpu::HeightTiles {
        self.heights
    }

    /// Attributes of the two vertex buffers `draw` binds (the lattice, then the origin of the cell
    /// per instance), and the stride of the lattice's.
    pub fn vertex_attributes() -> ([wgpu::VertexAttribute; 3], [wgpu::VertexAttribute; 1]) {
        draw::CellDrawer::vertex_layouts()
    }

    /// Bytes of a vertex of the lattice.
    pub const VERTEX_STRIDE: u64 = draw::CellDrawer::VERTEX_STRIDE;

    /// Draws the cells of a copy; see [`draw::CellDrawer::draw_geometry`].
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, copy_shift: f32, vs_group: u32) -> usize {
        self.drawer.draw_geometry(pass, copy_shift, vs_group)
    }
}

/// The terrain: the keys it reads, the basis, the field and the build of its cells. It draws
/// nothing itself yet (B1's adapter does).
pub struct Terrain {
    settings: TerrainSettings,
    /// How many times the keys were read again (tests, `?stats=1`).
    settings_reads: u32,
    library: Arc<TerrainLibrary>,
    build: TerrainBuild,
    map: Option<MapState>,
    fog: Vec<Fog>,
    fog_known: bool,
    /// Maps set so far.
    generation: u32,
    /// Added to the revision of the field so it grows across maps too.
    revision_base: u64,
    /// The world frame has to go to the frame constants.
    frame_dirty: bool,
    /// Grows when the table or a texture of the library changes, and when the fog does.
    library_epoch: u64,
    /// Grows when a texture that is no row of `TerrainDef` arrives (the albedo of a decal): the decal
    /// textures are picked up and the pages of the decals that use it made again, the map is kept.
    texture_epoch: u64,
    fog_epoch: u64,
    /// The pipelines and the textures of `TerrainDef`, and the part that belongs to the map.
    live_gpu: Option<live::LiveGpu>,
    live: Option<live::LiveMap>,
    /// The decals baked into the pages of the cells (WT-6).
    decals: decals::DecalLayer,
    /// The decals of the banks of the rivers (`BuildProducts::bank_decals`) of the basis in force.
    build_decals: Vec<decals::DecalHandle>,
    /// How many decal types the layer had when `build_decals` were made (`None`: not made yet, the
    /// types may still be on their way).
    build_decals_types: Option<usize>,
    /// Areas of our world whose pages are made again at the next frame.
    regions: Vec<(glam::Vec2, glam::Vec2)>,
    /// The pages baked again at the start of the turn (`terrain.rebakes_turn`).
    turn_rebaked: u64,
    turn_seen: Option<i32>,
    /// The last answer of [`Terrain::wanted`] and what it was made from: the client asks every frame,
    /// and the answer walks every decal of the map (tens of thousands) through the library.
    wanted_cache: std::sync::Mutex<Option<WantedCache>>,
    /// The CPU the field and the cells it built took in the last frame (the rest of the frame is the
    /// last frame's time without it).
    field_spent_ms: f64,
    /// Build the maps in pieces of a frame ([`Terrain::set_incremental`]).
    incremental: bool,
    /// The basis being built, in pieces.
    job: Option<BasisJob>,
    /// Builds started so far (the answer of `wanted` depends on the basis being built).
    job_seq: u32,
}

/// The build of a basis that goes on over several frames.
struct BasisJob {
    basis: TerrainBasis,
    continents: Vec<u8>,
    frame: TerrainFrame,
    sampler: TerrainSampler,
    stencil: TerrainStencil,
    run: build::GlobalRun,
}

/// The answer of [`Terrain::wanted`], the key it was made for, the revision of the decals it saw
/// and the calls since (the decals are walked again at most every [`WANTED_DECAL_CALLS`] calls).
struct WantedCache {
    key: WantedKey,
    decals_seen: u64,
    calls: u32,
    ids: Vec<asset_format::LogicalId>,
}

/// Calls of [`Terrain::wanted`] between two walks of the decals of the layer when only they changed
/// (a turn makes thousands of them over a few frames; their own textures can wait).
const WANTED_DECAL_CALLS: u32 = 30;

/// What [`Terrain::wanted`] depends on: the map, the library, the decals and their types, the key of
/// the highlight.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct WantedKey {
    generation: u32,
    library_epoch: u64,
    texture_epoch: u64,
    decal_types: u64,
    spec: bool,
    job: u32,
}

impl Default for Terrain {
    fn default() -> Self {
        Self {
            settings: TerrainSettings::default(),
            settings_reads: 0,
            library: Arc::new(TerrainLibrary::new()),
            build: TerrainBuild::new(0),
            map: None,
            fog: Vec::new(),
            fog_known: false,
            generation: 0,
            revision_base: 0,
            frame_dirty: false,
            library_epoch: 0,
            texture_epoch: 0,
            fog_epoch: 0,
            live_gpu: None,
            live: None,
            decals: decals::DecalLayer::new(),
            build_decals: Vec::new(),
            build_decals_types: None,
            regions: Vec::new(),
            turn_rebaked: 0,
            turn_seen: None,
            wanted_cache: std::sync::Mutex::new(None),
            field_spent_ms: 0.0,
            incremental: false,
            job: None,
            job_seq: 0,
        }
    }
}

impl Terrain {
    /// The keys in force.
    pub fn settings(&self) -> &TerrainSettings {
        &self.settings
    }

    /// Takes the keys of the renderer's settings.
    pub fn set_settings(&mut self, settings: &RenderSettings) {
        self.settings = TerrainSettings::from_settings(settings).with_detail(self.settings.detail);
        self.settings_reads += 1;
    }

    /// The terrain detail of the player (a level of its own, not a key of the ini). A new level makes
    /// the lattices and the pages again, like a changed key; the field and the build stay.
    pub fn set_detail(&mut self, detail: TerrainDetail) {
        if detail == self.settings.detail {
            return;
        }
        self.settings.detail = detail;
        self.live = None;
        if let Some(g) = self.live_gpu.as_mut() {
            g.drawer.reset();
        }
    }

    /// The terrain detail in force.
    pub fn detail(&self) -> TerrainDetail {
        self.settings.detail
    }

    /// How many times the keys were read.
    pub fn settings_reads(&self) -> u32 {
        self.settings_reads
    }

    /// Takes the library (pieces, `TerrainDef`, textures). A map that is set is built again: the
    /// ids of the field come from the table.
    pub fn set_library(&mut self, library: Arc<TerrainLibrary>) {
        self.library = library;
        self.library_epoch += 1;
        if let Some(basis) = self.current_basis() {
            self.set_basis(basis);
        }
    }

    /// Takes the table `TerrainDef` (`WorldSettings::terrain_defs`). A map that is set is built again.
    pub fn set_defs(&mut self, rows: Vec<asset_format::TerrainDefRow>) {
        Arc::make_mut(&mut self.library).set_defs(rows);
        self.library_epoch += 1;
        if let Some(basis) = self.current_basis() {
            self.set_basis(basis);
        }
    }

    /// Gives the library the pieces of a style. A map that is set is built again.
    pub fn add_piece_set(&mut self, set: asset_format::TerrainPieceSet) {
        Arc::make_mut(&mut self.library).add_piece_set(set);
        self.library_epoch += 1;
        if let Some(basis) = self.current_basis() {
            self.set_basis(basis);
        }
    }

    /// Gives the library the pieces of several styles; a map that is set is built again once.
    pub fn add_piece_sets(&mut self, sets: Vec<asset_format::TerrainPieceSet>) {
        if sets.is_empty() {
            return;
        }
        let lib = Arc::make_mut(&mut self.library);
        for set in sets {
            lib.add_piece_set(set);
        }
        self.library_epoch += 1;
        if let Some(basis) = self.current_basis() {
            self.set_basis(basis);
        }
    }

    /// Gives the library a texture of `TerrainDef` (the diffuse, height or specular of a row). The
    /// field does not change with a texture; the pages are made again from it.
    ///
    /// A texture that no row names is one of a decal (the types of the layer, the fields of a farm, the
    /// ground of a landmark): it comes while the map is on the screen, and the map is not built again
    /// for it (that took every page down and the cells through their statistics once more, for each
    /// texture that came); only the pages under the decals that name it are made again.
    pub fn add_texture(&mut self, id: asset_format::LogicalId, blob: asset_format::TextureBlob) {
        let in_row = self.library.defs().rows().iter().any(|r| {
            [&r.diffuse, &r.heightmap, &r.specmap]
                .into_iter()
                .flatten()
                .any(|t| *t == id)
        });
        if in_row {
            Arc::make_mut(&mut self.library).add_texture(id, blob);
            self.library_epoch += 1;
            return;
        }
        for d in self.decals.iter() {
            if d.albedo.as_ref() == Some(&id) || d.height.as_ref() == Some(&id) {
                self.regions.push((d.min, d.max));
            }
        }
        Arc::make_mut(&mut self.library).add_texture(id, blob);
        self.texture_epoch += 1;
    }

    /// The textures the map needs are in the library: every row of the continents of its hexes has its
    /// diffuse and height texture. The pages of the cache cannot be baked before.
    pub fn textures_ready(&self) -> bool {
        self.map.as_ref().is_some_and(|m| {
            !self.library.defs().is_empty()
                && self.library.textures_ready(m.continents.iter().copied())
        })
    }

    /// What the library still lacks for the map: the pieces of the styles of its continents
    /// (`pc:<style>`) and the textures its `TerrainDef` rows name (`tx:...`). The client asks the asset
    /// loader for these and hands them over through `add_piece_set` and `add_texture`.
    ///
    /// The answer is kept until one of the things it is made from changes ([`WantedKey`]).
    pub fn wanted(&self) -> Vec<asset_format::LogicalId> {
        // The basis being built asks for its pieces and textures at once.
        let continents = match (self.job.as_ref(), self.map.as_ref()) {
            (Some(j), _) => &j.continents,
            (None, Some(m)) => &m.continents,
            (None, None) => return Vec::new(),
        };
        let key = WantedKey {
            generation: self.generation,
            library_epoch: self.library_epoch,
            texture_epoch: self.texture_epoch,
            decal_types: self.decals.types_epoch(),
            spec: self.settings().has_spec(),
            job: if self.job.is_some() { self.job_seq } else { 0 },
        };
        let mut cache = self.wanted_cache.lock().unwrap_or_else(|e| e.into_inner());
        let decals = self.decals.revision();
        if let Some(c) = cache.as_mut()
            && c.key == key
        {
            c.calls += 1;
            if c.decals_seen == decals || c.calls < WANTED_DECAL_CALLS {
                return c.ids.clone();
            }
        }
        let ids = self.wanted_of(continents);
        *cache = Some(WantedCache {
            key,
            decals_seen: decals,
            calls: 0,
            ids: ids.clone(),
        });
        ids
    }

    fn wanted_of(&self, continents: &[u8]) -> Vec<asset_format::LogicalId> {
        use asset_format::{LogicalId, PieceStyle};
        let mut seen: std::collections::HashSet<LogicalId, crate::anim::fx::FxBuild> =
            Default::default();
        let mut out: Vec<LogicalId> = Vec::new();
        for style in PieceStyle::ALL {
            let c = library::continent_of_style(style);
            if (c == 0 || continents.contains(&c)) && self.library.piece_set(style).is_none() {
                out.push(LogicalId::new(format!("pc:{}", style.name())));
            }
        }
        // The specular maps only when the key has the highlight (`TerrainDetailLevel` 3).
        let spec = self.settings().has_spec();
        for &c in continents {
            for row in self.library.defs().of_continent(c) {
                let specmap = if spec { &row.specmap } else { &None };
                for id in [&row.diffuse, &row.heightmap, specmap]
                    .into_iter()
                    .flatten()
                {
                    // Each id once: thousands of decals share a few textures.
                    if !seen.contains(id)
                        && seen.insert(id.clone())
                        && self.library.texture(id).is_none()
                    {
                        out.push(id.clone());
                    }
                }
            }
        }
        // The textures of every decal type: the original loads them all with the types
        // (`TerrainDecalLayer::LoadTerrainDecals`). Without them the bake draws no decal (the banks
        // of the rivers, the fields, the shadows of the trees).
        let types = self.decals.types();
        for c in 0..crate::world::decal::CATEGORY_NAMES.len() {
            for ty in types.of(crate::world::decal::DecalCategory(c as u8)) {
                for id in [&ty.albedo, &ty.heightmap].into_iter().flatten() {
                    // Each id once: thousands of decals share a few textures.
                    if !seen.contains(id)
                        && seen.insert(id.clone())
                        && self.library.texture(id).is_none()
                    {
                        out.push(id.clone());
                    }
                }
            }
        }
        // The textures the decals of the layer bring of their own (the ground meshes of the
        // landmarks name the ones of their material).
        for d in self.decals.iter() {
            for id in [&d.albedo, &d.height].into_iter().flatten() {
                // Each id once: thousands of decals share a few textures.
                if !seen.contains(id)
                    && seen.insert(id.clone())
                    && self.library.texture(id).is_none()
                {
                    out.push(id.clone());
                }
            }
        }
        out
    }

    pub fn library(&self) -> &TerrainLibrary {
        &self.library
    }

    /// Puts a stage of `world-water` (or another module) in the build. A stage that joins after the
    /// basis is set (its asset came later, as the marsh stamp does) builds the map again with it.
    pub fn install_stage(&mut self, stage: Box<dyn BuildStage>) {
        self.build.install(stage);
        if let Some(basis) = self.current_basis() {
            self.set_basis(basis);
        }
    }

    /// The map goes (the game was left): nothing is built or drawn until the next basis, which then
    /// starts like the first of a load (built at once, not in pieces beside a shown map).
    pub fn unload(&mut self) {
        if let Some(old) = self.map.take() {
            self.revision_base += old.sampler.height_revision() + 1;
        }
        self.job = None;
        self.live = None;
        self.fog.clear();
        self.fog_known = false;
        self.frame_dirty = true;
        self.regions.clear();
        self.turn_seen = None;
        for h in self.build_decals.drain(..) {
            self.decals.destroy(h);
        }
        self.build_decals_types = None;
        *self
            .wanted_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// A new basis (`MapBasis`: the first one, or another `basis_hash`): the build starts again.
    /// The field is empty until the cells the fog opens are built ([`Self::pump`]).
    pub fn set_basis(&mut self, basis: TerrainBasis) {
        // Only a map that is shown waits for the new one in pieces (a freeze would be seen); the first
        // map of a load has nothing on the screen to stutter and is built at once, which is faster.
        if self.incremental && self.map.is_some() {
            self.start_job(basis);
        } else {
            self.job = None;
            self.set_basis_now(basis);
        }
    }

    /// Builds a new basis in pieces of a frame ([`Self::step_job`]) instead of at once while a map is
    /// shown: the build (the stages, the heights the rivers read) is long. The map of the new basis is
    /// there when the build is done; until then the one before stays. The first map is built at once.
    pub fn set_incremental(&mut self, on: bool) {
        self.incremental = on;
    }

    /// The basis in force: the one being built, else the one of the map.
    fn current_basis(&self) -> Option<TerrainBasis> {
        self.job
            .as_ref()
            .map(|j| j.basis.clone())
            .or_else(|| self.map.as_ref().map(|m| m.basis.clone()))
    }

    /// A basis is being built (see [`Self::set_incremental`]).
    pub fn building(&self) -> bool {
        self.job.is_some()
    }

    fn continents_of(basis: &TerrainBasis) -> Vec<u8> {
        let mut continents: Vec<u8> = basis.plots.iter().map(|p| p.continent).collect();
        continents.push(0);
        continents.sort_unstable();
        continents.dedup();
        continents
    }

    fn start_job(&mut self, basis: TerrainBasis) {
        let frame = TerrainFrame::new(
            u32::from(basis.width),
            u32::from(basis.height),
            basis.wrap_x,
        );
        let sampler = TerrainSampler::new(frame);
        let (w, h) = frame.texel_size();
        let run = self.build.begin_global(&basis);
        self.job_seq += 1;
        self.job = Some(BasisJob {
            continents: Self::continents_of(&basis),
            basis,
            frame,
            sampler,
            stencil: TerrainStencil::new(w, h),
            run,
        });
    }

    /// Works on the build of the basis for about `budget_ms`; the map is set when it is done.
    pub fn step_job(&mut self, budget_ms: f64) {
        let Some(job) = self.job.as_mut() else {
            return;
        };
        let r = self.build.step_global(
            &mut job.run,
            &job.basis,
            &self.library,
            &job.frame,
            job.sampler.field_mut(),
            &mut job.stencil,
            budget_ms,
        );
        let error = match r {
            Ok(false) => return,
            Ok(true) => None,
            Err(e) => Some(e),
        };
        if let Some(job) = self.job.take() {
            self.finish_basis(job.basis, job.sampler, job.stencil, error);
        }
    }

    fn set_basis_now(&mut self, basis: TerrainBasis) {
        let frame = TerrainFrame::new(
            u32::from(basis.width),
            u32::from(basis.height),
            basis.wrap_x,
        );
        let mut sampler = TerrainSampler::new(frame);
        let (w, h) = frame.texel_size();
        let mut stencil = TerrainStencil::new(w, h);
        let error = self
            .build
            .run_global(
                &basis,
                &self.library,
                &frame,
                sampler.field_mut(),
                &mut stencil,
            )
            .err();
        self.finish_basis(basis, sampler, stencil, error);
    }

    fn finish_basis(
        &mut self,
        basis: TerrainBasis,
        sampler: TerrainSampler,
        stencil: TerrainStencil,
        error: Option<BuildError>,
    ) {
        if let Some(old) = &self.map {
            self.revision_base += old.sampler.height_revision() + 1;
        }
        let (cells_x, cells_y) = (i32::from(basis.width), i32::from(basis.height));
        let n = (cells_x * cells_y) as usize;
        let continents = Self::continents_of(&basis);
        self.map = Some(MapState {
            basis,
            sampler,
            stencil,
            cells_x,
            cells_y,
            wanted: vec![false; n],
            done: vec![false; n],
            cell_rev: vec![0; n],
            cell_height_rev: vec![0; n],
            built_total: 0,
            pending: VecDeque::new(),
            hurried: vec![false; n],
            sorted_focus: None,
            wanted_count: 0,
            done_count: 0,
            error,
            continents,
        });
        self.live = None;
        self.generation += 1;
        self.frame_dirty = true;
        self.sync_build_decals();
        self.queue_open();
    }

    /// The decals of the banks of the rivers of the build are in the layer, made with the types the
    /// layer has now: the ones of the previous basis are taken out and the new ones created, with the
    /// draws the build made in the order of the original (`DecalLayer::create_rolled`, in the pass of
    /// the layer). Without decal types (they come with the world settings, after the basis) nothing
    /// is created yet; [`Self::refresh_build_decals`] does it when they arrive.
    fn sync_build_decals(&mut self) {
        for h in self.build_decals.drain(..) {
            self.decals.destroy(h);
        }
        self.build_decals_types = None;
        let n = self.decals.types().len();
        let products = self.build.products();
        if products.bank_decals.is_empty() {
            self.build_decals_types = Some(n);
            return;
        }
        if n == 0 {
            return;
        }
        for d in &products.bank_decals {
            if let Some(h) = self.decals.create_rolled(
                d.category,
                d.type_roll,
                d.verts.clone(),
                d.tris.clone(),
                decals::PASS_LAYER,
            ) {
                self.build_decals.push(h);
            }
        }
        self.build_decals_types = Some(n);
    }

    /// Makes the decals of the banks again when the types of the layer are not the ones they were
    /// made with (the types arrived after the basis, or changed).
    fn refresh_build_decals(&mut self) {
        if self.job.is_none()
            && self.map.is_some()
            && self.build_decals_types != Some(self.decals.types().len())
        {
            self.sync_build_decals();
        }
    }

    /// The records of a `basis` diff (`(index, record)`): the stages that care build their cells
    /// again, and only the cells the changed hexes cover. Ignored while there is no basis.
    pub fn patch_basis(&mut self, changes: &[(u32, PlotBasis)]) {
        // The basis being built takes the diff; the build starts again with it.
        if let Some(job) = self.job.as_mut() {
            if !job.basis.patch(changes).is_empty() {
                let basis = job.basis.clone();
                self.start_job(basis);
            }
            return;
        }
        let Some(map) = self.map.as_mut() else {
            return;
        };
        // The pieces are matched on the classes of the hexes (relief, terrain, wonder, continent):
        // when one changes the original has no cheaper way than to match again.
        let class_before: Vec<_> = changes
            .iter()
            .filter_map(|(i, _)| {
                map.basis
                    .plots
                    .get(*i as usize)
                    .map(build::bridge::plot_cell)
            })
            .collect();
        let changed = map.basis.patch(changes);
        if changed.is_empty() {
            return;
        }
        let class_after: Vec<_> = changes
            .iter()
            .filter_map(|(i, _)| {
                map.basis
                    .plots
                    .get(*i as usize)
                    .map(build::bridge::plot_cell)
            })
            .collect();
        if class_before != class_after {
            let basis = map.basis.clone();
            self.set_basis(basis);
            return;
        }
        for rect in self.build.invalidate(&changed) {
            for (cx, cy) in map.cells_of(&rect) {
                let i = map.index(cx, cy);
                // A cell nobody wants is built from the new basis when the fog opens it.
                if map.wanted[i] && map.done[i] {
                    map.done[i] = false;
                    map.done_count -= 1;
                    map.pending.push_back((cx, cy));
                }
            }
        }
    }

    /// The fog of the hexes, `y * width + x`. The cells of an open hex and the ring of one cell
    /// around it are the ones to build ("cells without an open hex are not drawn").
    pub fn set_fog(&mut self, fog: &[Fog]) {
        if self.fog_known && self.fog == fog {
            return;
        }
        self.fog.clear();
        self.fog.extend_from_slice(fog);
        self.fog_known = true;
        self.fog_epoch += 1;
        self.queue_open();
    }

    fn queue_open(&mut self) {
        let Some(map) = self.map.as_mut() else {
            return;
        };
        if self.fog.len() != map.basis.plots.len() {
            return;
        }
        let frame = *map.sampler.frame();
        let width = usize::from(map.basis.width).max(1);
        for (i, fog) in self.fog.iter().enumerate() {
            if *fog == Fog::Unrevealed {
                continue;
            }
            let rect = frame
                .hex_texel_rect((i % width) as i32, (i / width) as i32)
                .expand(CELL_SIZE as i32);
            for (cx, cy) in map.cells_of(&rect) {
                map.want(cx, cy);
            }
        }
    }

    /// Builds up to `max_cells` of the cells that wait, in the order they were asked for.
    /// Returns how many it built.
    pub fn pump(&mut self, max_cells: usize) -> usize {
        // The stages are being built again for another map: the cells of this one wait.
        if self.job.is_some() {
            return 0;
        }
        self.refresh_build_decals();
        if jobs::workers() > 0 && max_cells > 1 {
            return self.pump_pooled(max_cells);
        }
        let Some(map) = self.map.as_mut() else {
            return 0;
        };
        let frame = *map.sampler.frame();
        let mut built = 0;
        while built < max_cells {
            let Some((cx, cy)) = map.pending.pop_front() else {
                break;
            };
            let i = map.index(cx, cy);
            if map.done[i] {
                continue;
            }
            let _s = crate::perf::slow::scope("terrain.raster_cell");
            let t0 = crate::perf::slow::now_ms();
            let rev_before = map.sampler.height_revision();
            let cx_raster = RasterCx {
                basis: &map.basis,
                library: &self.library,
                frame: &frame,
                products: self.build.products(),
            };
            self.build.raster_cell(
                CellRect::cell(cx, cy),
                map.sampler.field_mut(),
                &mut map.stencil,
                &cx_raster,
            );
            let rev_after = map.sampler.height_revision();
            if rev_after != rev_before {
                map.cell_height_rev[i] = rev_after;
            }
            map.done[i] = true;
            map.hurried[i] = false;
            map.cell_rev[i] = map.cell_rev[i].wrapping_add(1);
            map.done_count += 1;
            map.built_total += 1;
            built += 1;
            // The cells over 4 ms apart from the sum of them all (`terrain.raster_cell`).
            let ms = crate::perf::slow::now_ms() - t0;
            if ms > 0.0 {
                use std::sync::atomic::Ordering::Relaxed;
                RASTER_US.fetch_add((ms * 1000.0) as u64, Relaxed);
                RASTER_CELLS.fetch_add(1, Relaxed);
            }
            if ms > 4.0 {
                crate::perf::slow::record("terrain.raster_cell.over_4ms", ms);
            }
            if ms > 20.0 {
                tracing::warn!(cx, cy, ms, "a cell of the field took long to build");
            }
        }
        built
    }

    /// [`Self::pump`] on the pool of `jobs` (B3 render-mt): the cells that wait are taken in batches of
    /// one per thread, rasterized side by side into copies of their blocks
    /// ([`TerrainBuild::raster_cell_detached`]) and put into the field here, in the order of the queue:
    /// the field, the stencil and the revisions end as the loop of [`Self::pump`] leaves them.
    fn pump_pooled(&mut self, max_cells: usize) -> usize {
        let Some(map) = self.map.as_mut() else {
            return 0;
        };
        let frame = *map.sampler.frame();
        let batch_max = jobs::workers() + 1;
        let mut built = 0;
        let mut batch: Vec<(i32, i32)> = Vec::with_capacity(batch_max);
        while built < max_cells {
            batch.clear();
            let want = (max_cells - built).min(batch_max);
            while batch.len() < want {
                let Some((cx, cy)) = map.pending.pop_front() else {
                    break;
                };
                // A cell queued twice is built once, as the loop does (it is done when it comes again).
                if map.done[map.index(cx, cy)] || batch.contains(&(cx, cy)) {
                    continue;
                }
                batch.push((cx, cy));
            }
            if batch.is_empty() {
                break;
            }
            let _s = crate::perf::slow::scope("terrain.raster_cell");
            let t0 = crate::perf::slow::now_ms();
            let cells = {
                let cx_raster = RasterCx {
                    basis: &map.basis,
                    library: &self.library,
                    frame: &frame,
                    products: self.build.products(),
                };
                let field = map.sampler.field();
                let build = &self.build;
                let batch = &batch;
                jobs::map(batch.len(), |k| {
                    let (cx, cy) = batch[k];
                    build.raster_cell_detached(CellRect::cell(cx, cy), field, &cx_raster)
                })
            };
            for (&(cx, cy), cell) in batch.iter().zip(cells) {
                let i = map.index(cx, cy);
                let rev_before = map.sampler.height_revision();
                TerrainBuild::apply_cell(cell, map.sampler.field_mut(), &mut map.stencil);
                let rev_after = map.sampler.height_revision();
                if rev_after != rev_before {
                    map.cell_height_rev[i] = rev_after;
                }
                map.done[i] = true;
                map.hurried[i] = false;
                map.cell_rev[i] = map.cell_rev[i].wrapping_add(1);
                map.done_count += 1;
                map.built_total += 1;
                built += 1;
            }
            let ms = crate::perf::slow::now_ms() - t0;
            if ms > 4.0 {
                crate::perf::slow::record("terrain.raster_cell.over_4ms", ms);
            }
        }
        built
    }

    /// Cells [`Self::pump_for_frame`] builds at once where the loop builds one: one per thread of the
    /// pool (1 without workers).
    fn pump_step() -> usize {
        jobs::workers() + 1
    }

    /// The field's share of a frame. While cells in view wait for it ([`FieldShare::View`]): one cell,
    /// then more while the milliseconds of the share last (when the clock moves), up to
    /// [`live::FIELD_CELLS_MAX_PER_FRAME`]; without a clock, [`live::FIELD_CELLS_PER_FRAME`]. While
    /// only cells of the ring wait ([`FieldShare::Ring`]): one cell, then more while
    /// [`live::FIELD_RING_MS`] last. Returns the cells built.
    ///
    /// Otherwise (the view and its ring are built) the rest of the map is built in the background: one
    /// cell, then more while the milliseconds of [`FieldShare::Background`] last
    /// ([`live::FIELD_BACKGROUND_CELLS`] without a clock).
    pub fn pump_for_frame(&mut self, clock: Option<&dyn Fn() -> f64>, share: FieldShare) -> usize {
        let start = clock.map(|c| c());
        // Sixteen cells before the clock was read again were 30 ms and more in one frame when a unit
        // opened hexes in view after the load: the view's share starts at one cell too.
        let (first, share, no_clock) = match (share, start) {
            (FieldShare::View(ms), _) => (1, ms, live::FIELD_CELLS_PER_FRAME),
            (FieldShare::Ring, _) => (1, live::FIELD_RING_MS, 1),
            (FieldShare::Background(ms), Some(_)) => (1, ms, 1),
            (FieldShare::Background(_), None) => (
                live::FIELD_BACKGROUND_CELLS,
                0.0,
                live::FIELD_BACKGROUND_CELLS,
            ),
        };
        // With the pool a step is one cell per thread, built side by side (as long as one cell).
        let step = Self::pump_step();
        let mut built = self.pump(first * step);
        let (Some(clock), Some(start)) = (clock, start) else {
            return built + self.pump(no_clock.saturating_sub(built));
        };
        let mut now = clock();
        // A clock that does not move (the browser's before the app gives one) is no clock.
        if now <= start {
            return built + self.pump(no_clock.saturating_sub(built));
        }
        let max = live::FIELD_CELLS_MAX_PER_FRAME * step;
        while built < max && now - start < share && self.pending_work() > 0 {
            built += self.pump(step.min(max - built));
            now = clock();
        }
        built
    }

    /// Wants every cell of the map and builds them all: for the tests and for `golden`, which
    /// opens the whole map.
    pub fn build_all(&mut self) {
        let Some(map) = self.map.as_mut() else {
            return;
        };
        for cy in 0..map.cells_y {
            for cx in 0..map.cells_x {
                map.want(cx, cy);
            }
        }
        self.pump(usize::MAX);
    }

    /// Cells that wait to be built (`pending_work` for `golden`).
    pub fn pending_work(&self) -> usize {
        self.map.as_ref().map_or(0, |m| m.pending.len())
    }

    /// Cells wanted so far, and cells built.
    pub fn cells_wanted(&self) -> usize {
        self.map.as_ref().map_or(0, |m| m.wanted_count)
    }

    pub fn cells_done(&self) -> usize {
        self.map.as_ref().map_or(0, |m| m.done_count)
    }

    /// The cells (`(cx, cy)`) not built yet: their field is still empty.
    pub fn unbuilt_cells(&self) -> Vec<(i32, i32)> {
        self.map.as_ref().map_or_else(Vec::new, |m| {
            m.done
                .iter()
                .enumerate()
                .filter(|&(_, &done)| !done)
                .map(|(i, _)| ((i as i32) % m.cells_x, (i as i32) / m.cells_x))
                .collect()
        })
    }

    /// Whether a cell was built.
    pub fn cell_done(&self, cx: i32, cy: i32) -> bool {
        self.map.as_ref().is_some_and(|m| {
            (0..m.cells_x).contains(&cx) && (0..m.cells_y).contains(&cy) && m.done[m.index(cx, cy)]
        })
    }

    /// How much of what the fog opens is built, 0 to 1. Nothing is known before the fog comes.
    pub fn load_progress(&self) -> f32 {
        match &self.map {
            Some(m) if self.fog_known => {
                if m.wanted_count == 0 {
                    1.0
                } else {
                    m.done_count as f32 / m.wanted_count as f32
                }
            }
            _ => 0.0,
        }
    }

    pub fn basis(&self) -> Option<&TerrainBasis> {
        self.map.as_ref().map(|m| &m.basis)
    }

    /// The sampler of the field: heights, normals, texel and hex.
    pub fn sampler(&self) -> Option<&TerrainSampler> {
        self.map.as_ref().map(|m| &m.sampler)
    }

    /// The frame of the map: texel and our world, `ours_to_orig()`.
    pub fn frame(&self) -> Option<&TerrainFrame> {
        self.map.as_ref().map(|m| m.sampler.frame())
    }

    pub fn stencil(&self) -> Option<&TerrainStencil> {
        self.map.as_ref().map(|m| &m.stencil)
    }

    /// Grows at every change of a height, across maps too. It changes only when a height of the field
    /// really changes: while the field builds (a cell for the first time, or again after a basis
    /// diff) and for a new basis; a map that is built and stays has a revision that stays.
    pub fn height_revision(&self) -> u64 {
        self.revision_base + self.map.as_ref().map_or(0, |m| m.sampler.height_revision())
    }

    /// The cells (`(cx, cy)`, one per hex) whose heights changed after the revision `since` (a value
    /// of [`Self::height_revision`] the caller kept), for a caller that places things on the ground
    /// and wants to place again only what stands on cells that changed. A cell is the texels of its
    /// hex's cell (`CellRect::cell`); what reads the heights a little around a point should take the
    /// cells around too. `None`: the map is another one since then (a new basis), everything changed.
    pub fn height_dirty_cells(&self, since: u64) -> Option<Vec<(i32, i32)>> {
        let map = self.map.as_ref()?;
        if since < self.revision_base {
            return None;
        }
        let local = since - self.revision_base;
        Some(
            map.cell_height_rev
                .iter()
                .enumerate()
                .filter(|&(_, &r)| r > local)
                .map(|(i, _)| ((i as i32) % map.cells_x, (i as i32) / map.cells_x))
                .collect(),
        )
    }

    /// Splits the cells [`Self::height_dirty_cells`] named into the ones whose heights are final (the
    /// cell and the cells around it that are wanted are built: no build to come changes them) and the
    /// ones that wait, which go into `waiting` with the ones that waited before. A caller that places
    /// things on the ground looks at a cell once its heights are final instead of at every cell around
    /// it that builds (each a revision of the field). The cells that waited come back when they are
    /// final; a basis diff that builds a cell again names it again.
    pub fn final_height_cells(
        &self,
        dirty: Vec<(i32, i32)>,
        waiting: &mut Vec<(i32, i32)>,
    ) -> Vec<(i32, i32)> {
        let Some(map) = self.map.as_ref() else {
            return dirty;
        };
        let built = |x: i32, y: i32| {
            !(0..map.cells_x).contains(&x) || !(0..map.cells_y).contains(&y) || {
                let i = map.index(x, y);
                map.done[i] || !map.wanted[i]
            }
        };
        let mut all = std::mem::take(waiting);
        all.extend(dirty);
        all.sort_unstable();
        all.dedup();
        let (done, wait): (Vec<_>, Vec<_>) = all
            .into_iter()
            .partition(|&(x, y)| (-1..=1).all(|dy| (-1..=1).all(|dx| built(x + dx, y + dy))));
        *waiting = wait;
        done
    }

    /// `m_vTerrainScale`: the texel along X and Y, the height of raw 255.
    pub fn terrain_scale(&self) -> Vec3 {
        self.frame()
            .copied()
            .unwrap_or_else(|| TerrainFrame::new(0, 0, false))
            .terrain_scale()
    }

    /// What the stages hand over: splines, coastal roads, river edges, decal requests.
    pub fn products(&self) -> &BuildProducts {
        self.build.products()
    }

    /// The stream and the stages of the last build.
    pub fn build(&self) -> &TerrainBuild {
        &self.build
    }

    /// Why the last build failed, if it did.
    pub fn build_error(&self) -> Option<&BuildError> {
        self.map.as_ref().and_then(|m| m.error.as_ref())
    }

    /// Bases set so far.
    pub fn generation(&self) -> u32 {
        self.generation
    }

    /// `ours_to_orig` of the map, once after a new map: the frame constants take it.
    pub fn take_frame_update(&mut self) -> Option<Mat4> {
        if !std::mem::take(&mut self.frame_dirty) {
            return None;
        }
        self.frame().map(|f| f.ours_to_orig())
    }
}

impl Terrain {
    /// The CPU work of a frame of the terrain through the cache: the cells the field finishes, the
    /// visible cells, the pages they ask for, the tiles of heights and the draw list. Returns `true`
    /// when the terrain can be drawn this way (a map, the textures of its styles, the light): then
    /// the subsystem bakes and draws, and the adapter of B1 stays out of the plan. The pass of the
    /// bake and the draw record what this made.
    pub fn live_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cfg: &LiveConfig,
        inputs: &LiveInputs<'_>,
    ) -> bool {
        if !self.textures_ready() || self.map.as_ref().is_some_and(|m| m.error.is_some()) {
            self.live = None;
            return false;
        }
        {
            let _s = crate::perf::slow::scope("terrain.refresh_decals");
            self.refresh_build_decals();
        }
        // The background work of the field and of the cells it builds (built, made ready) fills the
        // frame up to [`live::FRAME_FILL_MS`]: what the last frame cost without it is the rest of the
        // frame. Without the time of the frame, [`live::FIELD_BACKGROUND_MS`] each.
        let slack = inputs
            .frame_cpu_ms
            .map_or(2.0 * live::FIELD_BACKGROUND_MS, |ms| {
                (live::FRAME_FILL_MS - (f64::from(ms) - self.field_spent_ms))
                    .clamp(live::FIELD_SLACK_MIN_MS, live::FIELD_SLACK_MAX_MS)
            });
        let started = inputs.clock.map(|c| c());
        let mut pump_ms = 0.0;
        // The cells the fog opens are built in the field a few at a time.
        if self.job.is_none() && self.pending_work() > 0 {
            // The cells the last frame saw without ground are built first; the rest outwards from the
            // camera. Only a cell that waits for the field itself counts: one that is built and waits
            // to be made ready is the share of the live map's (`LiveMap::sync`), and the field building
            // at the share of the load for it (the ring of a far view, a cell or two of it) kept the
            // frame at 80 ms for as long as the map took to build.
            // After the first complete view a hole in view (hexes a unit opened) is not the load.
            let view_ms = if self.live.as_ref().is_some_and(|l| l.was_ready) {
                live::FIELD_VIEW_MS
            } else {
                live::FIELD_FRAME_MS
            };
            let mut share = if self.live.is_none() {
                FieldShare::View(view_ms)
            } else {
                FieldShare::Background(0.5 * slack)
            };
            if let (Some(map), Some(live)) = (self.map.as_mut(), self.live.as_ref()) {
                if let Some(f) = live.focus {
                    let wrap = map.sampler.frame().wrap_x();
                    map.sort_pending(((f as i32) % map.cells_x, (f as i32) / map.cells_x), wrap);
                }
                for (k, &i) in live.unbuilt_cells.iter().enumerate() {
                    if map.hurry(i) {
                        // The cells in view come first in the list, the ring's after.
                        if k < live.stats.unbuilt {
                            share = FieldShare::View(view_ms);
                        } else if !matches!(share, FieldShare::View(_)) {
                            share = FieldShare::Ring;
                        }
                    }
                }
            }
            let _s = crate::perf::slow::scope("terrain.pump");
            let _share = crate::perf::slow::scope(match share {
                FieldShare::View(_) => "terrain.pump.view",
                FieldShare::Ring => "terrain.pump.ring",
                FieldShare::Background(_) => "terrain.pump.background",
            });
            self.pump_for_frame(inputs.clock, share);
            if let (Some(c), Some(t0)) = (inputs.clock, started) {
                pump_ms = c() - t0;
            }
        }
        let epoch = self.library_epoch;
        match &mut self.live_gpu {
            Some(g) if g.cfg == *cfg && g.defs_epoch == epoch => {}
            Some(g) if g.cfg == *cfg => {
                g.defs = live::make_defs(device, queue, &self.library);
                g.defs_epoch = epoch;
                self.live = None;
            }
            slot => {
                *slot = Some(live::LiveGpu::new(device, queue, cfg, &self.library, epoch));
                self.live = None;
            }
        }
        let (Some(map), Some(gpu)) = (self.map.as_ref(), self.live_gpu.as_mut()) else {
            return false;
        };
        if self
            .live
            .as_ref()
            .is_none_or(|l| l.generation != self.generation || l.library_epoch != epoch)
        {
            self.live = Some(live::LiveMap::new(
                device,
                queue,
                map,
                &self.library,
                &self.settings,
                self.generation,
                epoch,
            ));
        }
        let Some(live) = self.live.as_mut() else {
            return false;
        };
        // The areas the decals changed and the ones asked for: their pages are made again.
        let mut regions = std::mem::take(&mut self.regions);
        regions.extend(self.decals.take_dirty());
        for (min, max) in regions {
            live.invalidate_world_region(map, min, max);
        }
        let _s = crate::perf::slow::scope("terrain.live_map_frame");
        // What the field left of the background's time is for the cells it built.
        live.ready_share_ms = (slack - pump_ms).max(0.0);
        live.frame(device, queue, gpu, map, &self.fog, self.fog_epoch, inputs);
        self.field_spent_ms = pump_ms + live.ready_spent_ms;
        true
    }

    /// The visible cells of the last frame, by copy of the map, with their lattices and tiles of heights,
    /// for the pass of the cells of `world-overlays` into `ScreenOverlay`. `None` until the
    /// terrain draws through the cache.
    pub fn cell_geometry(&self) -> Option<CellGeometry<'_>> {
        let gpu = self.live_gpu.as_ref()?;
        let live = self.live.as_ref()?;
        Some(CellGeometry {
            drawer: &gpu.drawer,
            heights: live.heights(),
        })
    }

    /// The decals: `create`, `destroy` and `set_object_decals` for the producers.
    pub fn decals_mut(&mut self) -> &mut decals::DecalLayer {
        &mut self.decals
    }

    pub fn decals(&self) -> &decals::DecalLayer {
        &self.decals
    }

    /// The pages of the cells an area of our world touches are baked again at the next frame
    /// (`Terrain::InvalidateRegion`).
    pub fn invalidate_region(&mut self, min: glam::Vec2, max: glam::Vec2) {
        self.regions.push((min, max));
    }

    /// The map's part of the cache, for the tests and `?stats=1`.
    pub fn live(&self) -> Option<&live::LiveMap> {
        self.live.as_ref()
    }

    /// What the pages and the draw did.
    /// The `terrain.*` counters (tests).
    #[cfg(test)]
    pub(crate) fn terrain_counters_for_test(&self) -> Vec<(&'static str, u64)> {
        self.terrain_counters()
    }

    /// The drawer of the cells (tests: the batched and the per-cell draw side by side).
    #[cfg(test)]
    pub(crate) fn cell_drawer_mut(&mut self) -> Option<&mut draw::CellDrawer> {
        self.live_gpu.as_mut().map(|g| &mut g.drawer)
    }

    pub fn live_stats(&self) -> LiveStats {
        self.live.as_ref().map_or(LiveStats::default(), |l| l.stats)
    }

    /// The turn of the view shown; a new one is [`Self::mark_turn`].
    pub fn note_turn(&mut self, turn: i32) {
        if self.turn_seen.replace(turn) != Some(turn) {
            self.mark_turn();
        }
    }

    /// A new turn began: `terrain.rebakes_turn` counts from here.
    pub fn mark_turn(&mut self) {
        self.turn_rebaked = self.live.as_ref().map_or(0, |l| l.cache.counters().rebaked);
    }

    /// The counters of the cache of pages (`terrain.*`, AC-H8): the instant ones of the last frame and
    /// the ones that grow from the start of the map (`*_total`; a reader takes the difference).
    fn terrain_counters(&self) -> Vec<(&'static str, u64)> {
        use settings_map::PAGE_SIZES;
        let mut out = Vec::new();
        if let Some(live) = self.live.as_ref() {
            let s = live.stats;
            let c = live.cache.counters();
            let pools = live.cache.pools();
            let in_use = |i: usize| {
                pools
                    .pool(PAGE_SIZES[i])
                    .map_or(0, |p| u64::from(p.in_use()))
            };
            out.extend([
                ("terrain.visible", s.visible as u64),
                ("terrain.holes", s.unpaged as u64),
                ("terrain.unbuilt", s.unbuilt as u64),
                ("terrain.holes_total", s.holes_total),
                ("terrain.hole_frames_total", s.hole_frames),
                ("terrain.jobs", live.cache.jobs().count() as u64),
                ("terrain.jobs_visible", c.jobs_visible as u64),
                ("terrain.budget", s.budget as u64),
                // Where the vertices of the pages are made: 1 the GPU (compute), 0 the CPU (WebGL2, or
                // the compute pipeline failed: the console has the reason).
                (
                    "terrain.vertex_path",
                    u64::from(
                        self.live_gpu
                            .as_ref()
                            .is_some_and(|g| g.baker.gpu_vertices()),
                    ),
                ),
                // The bakes of the view (in view and the ring); the base pages of the rest of the map are
                // `terrain.background.paged_total` (not `*bake*`: the bench's quiet test waits for
                // those to stop, and the background goes on for minutes on a huge map).
                (
                    "terrain.baked_total",
                    s.baked.saturating_sub(c.background_baked),
                ),
                ("terrain.background.paged_total", c.background_baked),
                (
                    "terrain.background.ms_total",
                    live.background_spent().0 as u64,
                ),
                (
                    "terrain.background.page_us",
                    (live.background_spent().1 * 1000.0) as u64,
                ),
                // The CPU raster of the field cells: its time per cell is the slow-state probe of
                // the bench (`scene.slowTier`; pure wasm, no GPU in it).
                (
                    "terrain.raster_us_total",
                    RASTER_US.load(std::sync::atomic::Ordering::Relaxed),
                ),
                (
                    "terrain.raster_cells_total",
                    RASTER_CELLS.load(std::sync::atomic::Ordering::Relaxed),
                ),
                ("terrain.rebakes_total", c.rebaked),
                (
                    "terrain.rebakes_turn",
                    c.rebaked.saturating_sub(self.turn_rebaked),
                ),
                ("terrain.rebake.field_total", c.rebake_cause[0]),
                ("terrain.rebake.decal_total", c.rebake_cause[1]),
                ("terrain.rebake.settings_total", c.rebake_cause[2]),
                ("terrain.evict.64_total", c.evicted[0]),
                ("terrain.evict.128_total", c.evicted[1]),
                ("terrain.evict.256_total", c.evicted[2]),
                ("terrain.evict.512_total", c.evicted[3]),
                ("terrain.demote_total", c.demoted),
                ("terrain.pages.64", in_use(0)),
                ("terrain.pages.128", in_use(1)),
                ("terrain.pages.256", in_use(2)),
                ("terrain.pages.512", in_use(3)),
                ("terrain.page_bytes", live.arrays().memory_bytes()),
                ("terrain.cells.recomputed_total", s.recomputed),
                ("terrain.cells.textured_total", s.textured),
                ("terrain.cells.waiting", s.waiting as u64),
                ("terrain.ring_unbuilt", s.ring_unbuilt as u64),
                // 1 when every open cell in view and in its ring is built and has a page.
                ("terrain.view_ready", u64::from(live.view_ready())),
            ]);
            let limits = live.cache.limits();
            out.extend([
                ("terrain.pool_limit.64", u64::from(limits[0])),
                ("terrain.pool_limit.128", u64::from(limits[1])),
                ("terrain.pool_limit.256", u64::from(limits[2])),
                ("terrain.pool_limit.512", u64::from(limits[3])),
                // The terrain detail and what it caps: the biggest page and the memory of the pages.
                ("terrain.detail", u64::from(self.settings.detail.index())),
                (
                    "terrain.lattice_step",
                    u64::from(self.settings.lattice_step()),
                ),
                ("terrain.max_page", u64::from(live.cache.max_page())),
                ("terrain.page_budget_bytes", live.cache.budget_bytes()),
                ("terrain.spec.pages", u64::from(live.cache.cache_specs())),
                (
                    "terrain.spec.budget",
                    u64::from(live.cache.spec_budget().unwrap_or(0)),
                ),
                ("terrain.spec.dropped_total", c.spec_dropped),
                ("terrain.spec.restored_total", c.spec_restored),
            ]);
            if let Some(g) = self.live_gpu.as_ref() {
                // The cells the main pass draws over all the copies, and in how many calls (one a
                // cell on GL; a run of cells of one array, tile and lattice elsewhere).
                out.extend([
                    ("terrain.cells.drawn", g.drawer.last_draws() as u64),
                    ("terrain.cells.draw_calls", g.drawer.last_batches() as u64),
                ]);
            }
        }
        if let Some(map) = self.map.as_ref() {
            // The field: whether it is still building, and the revision of its heights (it stays
            // once the map is built; the cities and the landmarks are placed again when it moves).
            out.extend([
                ("terrain.field.wanted", map.wanted_count as u64),
                ("terrain.field.done", map.done_count as u64),
                // Not `*pending*`: the bench's quiet test waits for those, and the field out of view
                // builds in the background after the view is complete (its cost is in the frames).
                ("terrain.field.background", map.pending.len() as u64),
                ("terrain.field.built_total", map.built_total),
                ("terrain.height_revision", self.height_revision()),
                (
                    "terrain.normals.bytes",
                    map.sampler.normal_cache_stats().0 as u64,
                ),
                (
                    "terrain.normals.hits_total",
                    map.sampler.normal_cache_stats().1,
                ),
                (
                    "terrain.normals.made_total",
                    map.sampler.normal_cache_stats().2,
                ),
            ]);
        }
        out.extend(self.decals.dirty_counters());
        out
    }

    /// The hex and the point under a ray (our basis), by the field. Cells that are not built, and
    /// the ones with no ground, are not hit.
    pub fn pick(&self, ray: &crate::game_camera::Ray) -> Option<crate::picking::Pick> {
        let map = self.map.as_ref()?;
        let grid = cells::CellGrid::new(map.sampler.frame());
        let classes = std::cell::RefCell::new(std::collections::HashMap::new());
        let pickable = |cx: i32, cy: i32| -> bool {
            if !(0..map.cells_x).contains(&cx) || !(0..map.cells_y).contains(&cy) {
                return false;
            }
            let i = map.index(cx, cy);
            if !map.done[i] {
                return false;
            }
            *classes.borrow_mut().entry(i).or_insert_with(|| {
                cells::calculate_stats(&map.sampler, &grid.cell_box(cx, cy))
                    .class
                    .is_drawn()
            })
        };
        pick::pick_ray(&map.sampler, ray, &pickable)
    }
}

impl RenderSystem for Terrain {
    fn id(&self) -> SystemId {
        SystemId::Terrain
    }

    fn passes(&self) -> &'static [PassSlot] {
        &[
            PassSlot::Pass(PassId::TerrainBake),
            PassSlot::Pass(PassId::ShadowMap),
            PassSlot::Main(MainSlot::Terrain),
        ]
    }

    fn init(&mut self, _cx: &mut InitContext<'_>) {}

    fn on_settings(
        &mut self,
        settings: &RenderSettings,
        _changed: &KeySet,
        _cx: &mut InitContext<'_>,
    ) {
        self.set_settings(settings);
        // The pages and the lattices are made again; the field and the build stay (AC-R8.1).
        self.live = None;
        if let Some(g) = self.live_gpu.as_mut() {
            g.drawer.reset();
        }
    }

    fn prepare(&mut self, cx: &mut PrepareContext<'_>) {
        if let Some(m) = self.take_frame_update() {
            cx.set_world_frame(m);
        }
        // Through the cache the field has its share of the frame in `live_frame`.
        if self.live.is_none() {
            self.pump(CELLS_PER_PREPARE);
        }
    }

    fn record(&self, _slot: PassSlot, cx: &mut DrawContext<'_, '_>) {
        if let (Some(_), Some(gpu)) = (self.live.as_ref(), self.live_gpu.as_ref()) {
            gpu.drawer.draw(cx);
        }
    }

    fn record_offscreen(&mut self, pass: PassId, cx: &mut EncoderContext<'_>) {
        if pass != PassId::TerrainBake {
            return;
        }
        if let (Some(map), Some(live), Some(gpu)) = (
            self.map.as_ref(),
            self.live.as_mut(),
            self.live_gpu.as_mut(),
        ) {
            live.bake(
                cx.device,
                cx.queue,
                cx.encoder,
                gpu,
                map,
                &self.library,
                self.library_epoch + self.texture_epoch,
                &self.decals,
                None,
            );
        }
    }

    fn record_mask(&self, _slot: PassSlot, cx: &mut MaskContext<'_, '_>) {
        if let (Some(_), Some(gpu)) = (self.live.as_ref(), self.live_gpu.as_ref()) {
            gpu.drawer.draw_mask(cx);
        }
    }

    fn counters(&self) -> Vec<(&'static str, u64)> {
        self.terrain_counters()
    }

    fn pending_work(&self) -> u32 {
        self.live
            .as_ref()
            .map_or(0, |l| l.pending_work() as u32)
            .saturating_add(Terrain::pending_work(self) as u32)
            .saturating_add(u32::from(self.job.is_some()))
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec2;

    use super::*;
    use crate::hexgrid::cell_center;
    use crate::settings::{Preset, readers_of};
    use crate::terrain::{Relief, TerrainKind};

    #[test]
    fn the_terrain_reads_the_keys_of_table_4_12() {
        let keys = readers_of(SystemId::Terrain);
        for k in [
            "TerrainDetailLevel",
            "TerrainTessLevel",
            "TerrainShadowQuality",
            "TerrainPageinSpeedStill",
            "TerrainPageinSpeedMoving",
            "Terrain64Chunks",
            "Terrain512Chunks",
            "BicubicTerrainTessellation",
            "MinimizeGrayTiles",
            "OverlayLevel",
            "Use Screen Space Overlay",
        ] {
            assert!(keys.contains(k), "{k}");
        }
        // Water keys are not the terrain's.
        assert!(!keys.contains("TerrainWaterQuality"));
    }

    #[test]
    fn a_change_of_a_key_reaches_the_subsystem_and_changes_what_it_says() {
        let mut t = Terrain::default();
        assert!(t.settings().has_spec());
        let mut low = RenderSettings::preset(Preset::Low);
        t.set_settings(&low);
        assert!(!t.settings().has_spec());
        low.set("TerrainDetailLevel", 3).unwrap();
        t.set_settings(&low);
        assert!(t.settings().has_spec());
        assert_eq!(t.settings_reads(), 2);
    }

    fn plot(relief: Relief) -> PlotBasis {
        PlotBasis {
            relief,
            terrain: TerrainKind::Grass,
            river_flow: PlotBasis::NO_FLOW,
            ..PlotBasis::default()
        }
    }

    fn basis6(seed: u32) -> TerrainBasis {
        TerrainBasis::uniform(6, 6, false, seed, plot(Relief::Flat))
    }

    fn terrain_with(basis: TerrainBasis) -> Terrain {
        let mut t = Terrain::default();
        t.set_library(Arc::new(TerrainLibrary::synthetic()));
        t.set_basis(basis);
        t
    }

    fn fog_only(open: &[(usize, usize)]) -> Vec<Fog> {
        let mut f = vec![Fog::Unrevealed; 36];
        for &(x, y) in open {
            f[y * 6 + x] = Fog::Visible;
        }
        f
    }

    #[test]
    fn without_a_map_there_is_nothing_to_ask() {
        let mut t = Terrain::default();
        assert!(t.sampler().is_none() && t.frame().is_none() && t.stencil().is_none());
        assert_eq!(
            (t.pump(4), t.pending_work(), t.load_progress()),
            (0, 0, 0.0)
        );
        assert!((t.terrain_scale().z - sampler::TERRAIN_SCALE_Z).abs() < 1e-6);
        assert_eq!(t.height_revision(), 0);
        // A diff before the basis changes nothing.
        t.patch_basis(&[(1, plot(Relief::Hill))]);
        assert!(t.basis().is_none());
        assert!(t.take_frame_update().is_none());
    }

    #[test]
    fn a_basis_makes_the_frame_and_an_empty_field() {
        let mut t = terrain_with(basis6(7));
        let f = *t.frame().unwrap();
        assert_eq!(f.texel_size(), (384, 384));
        assert_eq!(t.sampler().unwrap().field().blocks_allocated(), 0);
        assert_eq!(t.generation(), 1);
        assert_eq!(t.build().seed(), 7);
        assert!(t.build_error().is_none());
        // The frame goes to the constants once.
        assert_eq!(t.take_frame_update(), Some(f.ours_to_orig()));
        assert_eq!(t.take_frame_update(), None);
        // The fog has not come: nothing is wanted and the load is not known to be done.
        assert_eq!((t.pending_work(), t.load_progress()), (0, 0.0));
    }

    #[test]
    fn the_fog_opens_the_cells_of_the_open_hexes_and_a_ring() {
        let mut t = terrain_with(basis6(7));
        t.set_fog(&fog_only(&[(3, 3)]));
        let frame = *t.frame().unwrap();
        let want = frame.hex_texel_rect(3, 3).expand(64).cells(6, 6, false);
        assert!(want.len() > 1 && want.len() < 36, "{}", want.len());
        assert_eq!(t.pending_work(), want.len());
        assert_eq!(t.load_progress(), 0.0);
        // Four cells per call of the original's loader.
        assert_eq!(t.pump(CELLS_PER_PREPARE), 4);
        assert_eq!(t.cells_done(), 4);
        assert!((t.load_progress() - 4.0 / want.len() as f32).abs() < 1e-6);
        t.pump(usize::MAX);
        assert_eq!((t.pending_work(), t.load_progress()), (0, 1.0));
        assert_eq!(t.cells_wanted(), want.len());
        // The hex that is open is on the ground; a hex far from the fog is not built.
        let s = t.sampler().unwrap();
        assert!(
            (s.world_height(cell_center(3, 3)) - 15423.0 / 255.0 * sampler::TERRAIN_SCALE_Z).abs()
                < 0.05
        );
        assert_eq!(s.world_height(cell_center(0, 0)), 0.0);
        assert!(!s.field().is_allocated(10, 10));
        // The same fog again changes nothing.
        t.set_fog(&fog_only(&[(3, 3)]));
        assert_eq!(t.pending_work(), 0);
    }

    #[test]
    fn the_fog_may_come_before_the_basis() {
        let mut a = Terrain::default();
        a.set_library(Arc::new(TerrainLibrary::synthetic()));
        a.set_fog(&fog_only(&[(2, 2)]));
        assert_eq!(a.pending_work(), 0);
        a.set_basis(basis6(1));
        let mut b = terrain_with(basis6(1));
        b.set_fog(&fog_only(&[(2, 2)]));
        assert_eq!(a.pending_work(), b.pending_work());
        assert!(a.pending_work() > 0);
        // A fog of another size is not for this map.
        a.set_fog(&[Fog::Visible; 4]);
        assert_eq!(a.pending_work(), b.pending_work());
    }

    #[test]
    fn a_hex_that_is_all_unrevealed_wants_nothing_but_the_map_is_loaded() {
        let mut t = terrain_with(basis6(1));
        t.set_fog(&fog_only(&[]));
        assert_eq!(
            (t.pending_work(), t.cells_wanted(), t.load_progress()),
            (0, 0, 1.0)
        );
    }

    #[test]
    fn a_wonder_under_the_fog_builds_the_same_field_as_an_open_one() {
        let wonder = PlotBasis {
            feature: BasisFeature::NaturalWonder(NwPiece::Fuji),
            flags: BasisFlags(BasisFlags::NATURAL_WONDER),
            ..plot(Relief::Mountain)
        };
        let mut basis = basis6(99);
        let i = basis.index(2, 2).unwrap();
        basis.plots[i] = wonder;

        let mut hidden = terrain_with(basis.clone());
        hidden.set_fog(&fog_only(&[(0, 0), (5, 5), (4, 0), (0, 4)]));
        hidden.pump(usize::MAX);
        hidden.build_all();
        let mut open = terrain_with(basis);
        open.set_fog(&fog_only(&[(2, 2)]));
        open.pump(usize::MAX);
        open.build_all();

        let (a, b) = (hidden.sampler().unwrap(), open.sampler().unwrap());
        assert!(a.field().same_content(b.field()));
        assert_eq!(a.field().digest(), b.field().digest());
        assert_eq!(hidden.build().rand().state(), open.build().rand().state());
        // The synthetic library has no piece for a wonder: the hex stays plain land.
        assert!((a.world_height(cell_center(2, 2)) - 12.0).abs() < 0.2);
    }

    #[test]
    fn another_basis_builds_again_and_the_revision_keeps_growing() {
        let mut t = terrain_with(basis6(1));
        t.set_fog(&fog_only(&[(1, 1)]));
        t.pump(usize::MAX);
        let before = t.height_revision();
        assert!(before > 0);
        t.set_basis(basis6(2));
        assert_eq!(t.generation(), 2);
        assert_eq!(t.build().seed(), 2);
        // The new field is empty; the stored fog opens its cells again.
        assert_eq!(t.sampler().unwrap().field().blocks_allocated(), 0);
        assert!(t.pending_work() > 0 && t.cells_done() == 0);
        assert!(t.height_revision() > before);
        assert!(t.take_frame_update().is_some());
        t.pump(usize::MAX);
        assert!(t.height_revision() > before + 1);
    }

    #[test]
    fn a_diff_builds_again_only_the_cells_the_changed_hex_covers() {
        let mut t = terrain_with(basis6(3));
        t.set_fog(&fog_only(&[(1, 1), (2, 1)]));
        t.pump(usize::MAX);
        let done = t.cells_done();
        let s = t.sampler().unwrap();
        assert!(
            (s.world_height(cell_center(1, 1)) - 15423.0 / 255.0 * sampler::TERRAIN_SCALE_Z).abs()
                < 0.05
        );
        let rev = t.height_revision();

        // A record that is the same changes nothing.
        t.patch_basis(&[(6 + 1, plot(Relief::Flat))]);
        assert_eq!(t.pending_work(), 0);

        // A hill appears at (1, 1).
        t.patch_basis(&[(6 + 1, plot(Relief::Hill))]);
        let frame = *t.frame().unwrap();
        // The class of a hex changed: the pieces are matched again, every open cell is built again.
        let _ = frame;
        assert_eq!(t.pending_work(), done);
        t.pump(usize::MAX);
        let s = t.sampler().unwrap();
        assert!((s.world_height(cell_center(1, 1)) - 25.0).abs() < 0.6);
        // A hex that nobody changed has the height it had.
        assert!(
            (s.world_height(cell_center(2, 1)) - 15423.0 / 255.0 * sampler::TERRAIN_SCALE_Z).abs()
                < 0.05
        );
        assert!(t.height_revision() > rev);

        // A hex in a cell nobody wants: the diff waits for the fog.
        t.patch_basis(&[(4 * 6 + 5, plot(Relief::Hill))]);
        t.pump(usize::MAX);
        assert_eq!(t.pending_work(), 0);
        assert_eq!(t.sampler().unwrap().world_height(cell_center(5, 4)), 0.0);
        t.set_fog(&fog_only(&[(1, 1), (2, 1), (5, 4)]));
        t.pump(usize::MAX);
        assert!((t.sampler().unwrap().world_height(cell_center(5, 4)) - 25.0).abs() < 0.6);
    }

    /// The revision of the heights stays once the field is built (B3: the cities were laid out again
    /// whenever it moved), and the cells whose heights changed since a revision are known.
    #[test]
    fn the_height_revision_stays_once_built_and_names_the_cells_that_changed() {
        let mut t = terrain_with(basis6(3));
        assert_eq!(t.height_dirty_cells(0), Some(Vec::new()));
        t.set_fog(&fog_only(&[(1, 1), (2, 1)]));
        t.pump(usize::MAX);
        let built = t.height_revision();
        let all = t.height_dirty_cells(0).unwrap();
        assert!(!all.is_empty() && all.len() <= t.cells_done(), "{all:?}");
        // Nothing builds: the revision stays and no cell changed since.
        for _ in 0..3 {
            t.pump(16);
        }
        assert_eq!(t.height_revision(), built);
        assert_eq!(t.height_dirty_cells(built), Some(Vec::new()));
        // A record that is the same changes nothing either.
        t.patch_basis(&[(6 + 1, plot(Relief::Flat))]);
        t.pump(usize::MAX);
        assert_eq!(t.height_revision(), built);
        // A new basis: a caller with a revision of the old map hears that everything changed.
        t.set_basis(basis6(4));
        assert_eq!(t.height_dirty_cells(built), None);
        let fresh = t.height_revision();
        t.pump(usize::MAX);
        let changed = t.height_dirty_cells(fresh).unwrap();
        assert!(!changed.is_empty());
        assert!(changed.iter().all(|&(x, y)| t.cell_done(x, y)));
    }

    /// The field's share of a frame: the count, then more while the clock leaves time; a clock that
    /// does not move gives the count only.
    #[test]
    fn the_field_builds_in_the_time_of_its_share() {
        let basis = TerrainBasis::uniform(12, 12, false, 1, plot(Relief::Flat));
        let mut t = terrain_with(basis);
        t.set_fog(&[Fog::Visible; 144]);
        let wanted = t.pending_work();
        assert!(wanted > 60, "{wanted}");
        // Without a clock, or with one that stands: sixteen.
        let view = FieldShare::View(live::FIELD_FRAME_MS);
        assert_eq!(t.pump_for_frame(None, view), live::FIELD_CELLS_PER_FRAME);
        let still = || 5.0;
        assert_eq!(
            t.pump_for_frame(Some(&still), view),
            live::FIELD_CELLS_PER_FRAME
        );
        // A clock a millisecond further at each reading: one cell, the next reading at 1 ms, then one
        // cell a millisecond until 8 ms.
        let ticks = std::cell::Cell::new(0.0f64);
        let clock = || {
            let now = ticks.get();
            ticks.set(now + 1.0);
            now
        };
        assert_eq!(t.pump_for_frame(Some(&clock), view), 8);
        // Only the ring waits: one cell, then 2 ms (readings at 1 ms and 2 ms: one more).
        ticks.set(0.0);
        assert_eq!(t.pump_for_frame(Some(&clock), FieldShare::Ring), 2);
        // The view is built: the background share, one cell and then 1.5 ms (one more reading at
        // 1 ms); without a clock, two cells.
        ticks.set(0.0);
        assert_eq!(
            t.pump_for_frame(Some(&clock), FieldShare::Background(1.5)),
            2
        );
        assert_eq!(
            t.pump_for_frame(None, FieldShare::Background(1.5)),
            live::FIELD_BACKGROUND_CELLS
        );
    }

    /// What the terrain still wants is kept between the frames and made again when what it comes from
    /// changes: a texture that arrives, decals with a texture of their own.
    #[test]
    fn what_the_terrain_wants_follows_its_inputs() {
        use asset_format::{LogicalId, TextureBlob, TextureFormat};
        let mut t = Terrain::default();
        t.set_library(Arc::new(TerrainLibrary::synthetic()));
        t.install_stage(Box::new(BankStage(vec![bank_decal(3)])));
        t.set_basis(basis6(1));
        let before = t.wanted();
        assert_eq!(t.wanted(), before);
        // The decal types come: their textures are wanted.
        t.decals_mut().set_types(&floodplain_types());
        t.pump(0);
        let with_types = t.wanted();
        let flood = LogicalId::new("tx:Floodplains_01.dds");
        assert!(with_types.contains(&flood), "{with_types:?}");
        // The texture comes: it is not wanted any more.
        let mips = [4usize, 2, 1]
            .iter()
            .map(|&n| [9u8, 9, 9, 255].repeat(n * n))
            .collect();
        t.add_texture(
            flood.clone(),
            TextureBlob::new_2d(TextureFormat::Rgba8, false, 4, 4, mips),
        );
        let after = t.wanted();
        assert!(!after.contains(&flood));
        // Each id once.
        let mut sorted = after.clone();
        sorted.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        sorted.dedup();
        assert_eq!(sorted.len(), after.len());
    }

    /// A cell in view that has no ground goes to the front of the build, with the cells around it,
    /// once; the field it builds is the same.
    #[test]
    fn the_cells_in_view_are_built_first() {
        let basis = TerrainBasis::uniform(12, 12, false, 1, plot(Relief::Flat));
        let mut t = terrain_with(basis.clone());
        t.set_fog(&[Fog::Visible; 144]);
        let map = t.map.as_mut().unwrap();
        let last = map.index(10, 10);
        map.hurry(last);
        map.hurry(last);
        assert_eq!(map.pending.len(), 144 + 9);
        t.pump(9);
        for y in 9..=11 {
            for x in 9..=11 {
                assert!(t.cell_done(x, y), "{x} {y}");
            }
        }
        assert!(!t.cell_done(0, 0));
        t.pump(usize::MAX);
        assert_eq!(t.pending_work(), 0);
        let mut plain = terrain_with(basis);
        plain.set_fog(&[Fog::Visible; 144]);
        plain.pump(usize::MAX);
        assert!(
            t.sampler()
                .unwrap()
                .field()
                .same_content(plain.sampler().unwrap().field())
        );
    }

    #[test]
    fn two_runs_with_one_basis_and_seed_give_the_same_field() {
        let run = || {
            let mut basis = basis6(5);
            basis.plots[8] = plot(Relief::Hill);
            basis.plots[20] = plot(Relief::Water);
            let mut t = terrain_with(basis);
            t.build_all();
            t
        };
        let (a, b) = (run(), run());
        assert!(
            a.sampler()
                .unwrap()
                .field()
                .same_content(b.sampler().unwrap().field())
        );
        assert_eq!(a.cells_done(), 36);
        assert_eq!(
            a.sampler().unwrap().field().digest(),
            b.sampler().unwrap().field().digest()
        );
    }

    #[test]
    fn the_frame_of_a_wrapping_map_continues_over_the_seam() {
        let mut basis = TerrainBasis::uniform(6, 6, true, 1, plot(Relief::Flat));
        basis.plots[5] = plot(Relief::Hill);
        let mut t = terrain_with(basis);
        t.set_fog(&[Fog::Visible; 36]);
        t.pump(usize::MAX);
        let s = t.sampler().unwrap();
        let one_period = Vec2::new(s.frame().world_width(), 0.0);
        // Hex (5, 0) one period to the west of itself is the same ground.
        let p = cell_center(5, 0);
        assert!((s.world_height(p) - s.world_height(p - one_period)).abs() < 1e-4);
        assert!((s.world_height(p) - 25.0).abs() < 0.6);
        // Wrapping, the ring of the open hexes crosses the seam: all 36 cells are open.
        assert_eq!(t.cells_done(), 36);
    }

    #[test]
    fn the_library_arriving_late_builds_the_map_again() {
        let mut t = Terrain::default();
        let mut basis = basis6(1);
        basis.plots[7] = plot(Relief::Mountain);
        t.set_basis(basis);
        t.set_fog(&[Fog::Visible; 36]);
        t.pump(usize::MAX);
        // No pieces: no texel is reached, every id is the default of the blit (3).
        let hex = cell_center(1, 1);
        let (u, v) = t.sampler().unwrap().texel_of(hex);
        assert_eq!(t.sampler().unwrap().field().get_id(u, v), 3);
        t.set_library(Arc::new(TerrainLibrary::synthetic()));
        assert_eq!(t.generation(), 2);
        t.pump(usize::MAX);
        // The mountain row of whichever style the matcher drew (4 rows per style).
        assert_eq!(t.sampler().unwrap().field().get_id(u, v) % 4, 2);
    }

    struct BankStage(Vec<crate::world::water::river::floodplains::BankDecal>);

    impl BuildStage for BankStage {
        fn id(&self) -> StageId {
            StageId::Rivers
        }

        fn global(&mut self, cx: &mut StageCx<'_>) -> Result<(), BuildError> {
            cx.products.bank_decals = self.0.clone();
            Ok(())
        }
    }

    fn bank_decal(roll: u32) -> crate::world::water::river::floodplains::BankDecal {
        use crate::world::decal::DecalCategory;
        let v = |x: f32, y: f32| decals::DecalVertex {
            pos: Vec2::new(x, y),
            uv: Vec2::new(x, y),
        };
        crate::world::water::river::floodplains::BankDecal {
            category: DecalCategory::of_name("Floodplains").unwrap(),
            verts: vec![v(0.0, 0.0), v(8.0, 0.0), v(8.0, 8.0), v(0.0, 8.0)],
            tris: vec![[0, 1, 2], [0, 2, 3]],
            type_roll: roll,
        }
    }

    fn floodplain_types() -> Vec<asset_format::DecalDef> {
        (1..=2)
            .map(|i| asset_format::DecalDef {
                kind: "Floodplains".into(),
                name: format!("Floodplains_0{i}"),
                albedo: Some(asset_format::LogicalId::new(format!(
                    "tx:Floodplains_0{i}.dds"
                ))),
                heightmap: None,
                priority: None,
                extra: asset_format::Extra::default(),
            })
            .collect()
    }

    #[test]
    fn the_decals_of_the_banks_wait_for_the_types_and_follow_the_basis() {
        let mut t = Terrain::default();
        t.set_library(Arc::new(TerrainLibrary::synthetic()));
        t.install_stage(Box::new(BankStage(vec![bank_decal(3), bank_decal(4)])));
        t.set_basis(basis6(1));
        assert!(t.build_error().is_none());
        assert_eq!(t.products().bank_decals.len(), 2);
        // The types come after the basis: nothing yet, then the next pump makes the decals.
        assert!(t.decals().is_empty());
        t.decals_mut().set_types(&floodplain_types());
        t.pump(0);
        assert_eq!(t.decals().len(), 2);
        // The type is `roll % number of types`.
        let names: Vec<String> = t
            .decals()
            .iter()
            .map(|d| d.albedo.as_ref().unwrap().as_str().to_string())
            .collect();
        assert_eq!(names, ["tx:Floodplains_02.dds", "tx:Floodplains_01.dds"]);
        let rev = t.decals().revision();
        // Pumping again changes nothing.
        t.pump(0);
        assert_eq!(t.decals().revision(), rev);
        // A new basis takes the old decals out and makes the new ones.
        let old: Vec<_> = t.decals().iter().map(|d| d.handle).collect();
        t.set_basis(basis6(2));
        assert_eq!(t.decals().len(), 2);
        assert!(t.decals().iter().all(|d| !old.contains(&d.handle)));
        // The area is marked for the bake.
        assert!(!t.decals_mut().take_dirty().is_empty());
    }

    /// The albedo of a decal that comes while the map is on the screen is no reason to build the
    /// map again (the epoch of the library is what recreates the live map and drops every page): only
    /// the pages under the decals that name it are made again. A texture of a row of `TerrainDef`
    /// is the library changing, as before.
    #[test]
    fn a_decal_texture_that_comes_late_marks_its_decals_and_keeps_the_map() {
        use asset_format::{LogicalId, TextureBlob, TextureFormat};
        let solid = || {
            let mips = [4usize, 2, 1]
                .iter()
                .map(|&n| [9u8, 9, 9, 255].repeat(n * n))
                .collect();
            TextureBlob::new_2d(TextureFormat::Rgba8, false, 4, 4, mips)
        };
        let mut t = Terrain::default();
        t.set_library(Arc::new(TerrainLibrary::synthetic()));
        t.install_stage(Box::new(BankStage(vec![bank_decal(3), bank_decal(4)])));
        t.set_basis(basis6(1));
        t.decals_mut().set_types(&floodplain_types());
        t.pump(0);
        assert_eq!(t.decals().len(), 2);
        let epoch = t.library_epoch;
        let area: Vec<_> = t
            .decals()
            .iter()
            .filter(|d| d.albedo.as_ref().unwrap().as_str() == "tx:Floodplains_01.dds")
            .map(|d| (d.min, d.max))
            .collect();
        assert_eq!(area.len(), 1);
        t.add_texture(LogicalId::new("tx:Floodplains_01.dds"), solid());
        assert_eq!(t.library_epoch, epoch, "the map is kept");
        assert_eq!(t.regions, area, "the pages under the decal are made again");
        assert!(
            t.library()
                .texture(&LogicalId::new("tx:Floodplains_01.dds"))
                .is_some()
        );
        // A row of the table is the library changing.
        t.add_texture(LogicalId::new("tx:synthetic/grass.dds"), solid());
        assert_eq!(t.library_epoch, epoch + 1);
    }
}
