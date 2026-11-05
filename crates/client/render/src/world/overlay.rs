//! `Overlay` subsystem (world-overlays 4.3).
//!
//! WOV0: the code of B1 for what lies over the ground lives here without a change of the picture:
//! rivers, borders and the hex grid (one blend mesh, built from the ground and the owners). The
//! inputs are `set_ground` (the relief the lines
//! lie on), `set_owners` (who owns which cell), `set_highlights` (the selected plot, for the ring
//! of the original) and `set_grid_visible`; the fog
//! state the blend shader reads comes from `FluidFow` (`set_fog_state`).
//!
//! WOV2 adds the pass `ScreenOverlay` of the original: the grid (`HexOverlay`, `grid.rs`) goes into the
//! map of the overlay (`map.rs`, `OverlayMap`), and the cells of the ground (`cells.rs`) lay that map
//! over the relief into `ScreenOverlay`, which the final shader of the ground reads. The pass runs
//! when the ground gives its cells (`world-terrain`'s `cell_geometry()` through `WorldSystems`, or
//! the triangles of [`Overlay::set_cell_geometry`]), at `OverlayLevel` 2 with `Use Screen Space Overlay` 1; the grid line of B1 is then
//! not drawn. Without them the rivers, borders and the grid still go over the ground like in B1 (in
//! `Main.Terrain`). The contours and the ring have no line of B1 any more (SIMP-024): they are drawn
//! only through the styles of `Highlights.xml` and the ring textures.
//!
//! WOV4 makes the borders the original's: a strip along a spline round the cells of every player
//! (`borders/`), drawn into the map of the overlay after the grid; the meshes are kept per player and
//! only the players whose cells or colours changed are built again. Where the pass does not run the
//! same mesh, lifted onto the relief, is the line over the ground.

pub mod borders;
pub mod cells;
pub mod grid;
pub mod highlights;
pub mod map;
pub mod ring;

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use std::collections::BTreeSet;
use std::rc::Rc;

use glam::Vec2;

use self::borders::gpu::BordersGpu;
use self::borders::{BorderStyle, Borders};
use self::cells::{CellGeometry, CellsGpu, OVERLAY_DEPTH};
use self::grid::{GridGpu, HexGridStyle, grid_mesh};
use self::highlights::gpu::HighlightGpu;
use self::highlights::library::HighlightLibrary;
use self::highlights::model::HexModels;
use self::highlights::{HighlightMesh, HighlightSet};
use self::map::{MapView, screen_overlay_used};
use self::ring::{RingGpu, RingStyle};
use super::b1_env::{B1Env, copy_group, map_shifts};
use super::fow::FogGpu;
use super::fow::mask::{FowView, cells_in_rect};
use super::terrain::CellGeometry as TerrainCells;
use super::trade::Trade;
use crate::blend::{BlendMesh, BlendVertex};
use crate::entities::Highlights;
use crate::frame::{
    DEPTH_FORMAT, DrawContext, EncoderContext, InitContext, MainSlot, MaskContext, OVERLAY_MAP,
    PassId, PassSlot, PrepareContext, RenderSystem, ResourceDesc, SCREEN_OVERLAY, SystemId,
    TargetSize,
};
use crate::hexgrid::cell_center;
use crate::settings::{KeySet, RenderSettings};
use crate::shadow::visible_rect;
use crate::terrain::{TerrainSnapshot, overlay_mesh};

const OVERLAY_LEVEL: &str = "OverlayLevel";
const USE_SCREEN_SPACE_OVERLAY: &str = "Use Screen Space Overlay";
/// Half-size of the square seen around the eye when no edge of the frustum reaches the ground.
const FALLBACK_HALF: f32 = 1024.0;

/// The owner of every cell and the colour of every owner (`set_owners`): the input of the borders.
/// Row-major from y = 0 like the protocol; `-1` is no owner; `colors` is indexed by player id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnersInput {
    pub width: u32,
    pub height: u32,
    pub owners: Vec<i8>,
    pub colors: Vec<[u8; 3]>,
    /// The second colour of every player (the fill of the border), indexed like `colors`; empty
    /// where the caller does not know it (a lighter shade of the first is used).
    pub secondary: Vec<[u8; 3]>,
    /// Which players are city states (their border is dashed), indexed like `colors`.
    pub city_states: Vec<bool>,
}

impl OwnersInput {
    /// The second colours (see [`Self::secondary`]).
    /// Cut like the colours, so that the same borders are the same input.
    pub fn with_secondary_colors(mut self, mut secondary: Vec<[u8; 3]>) -> Self {
        secondary.truncate(self.colors.len());
        self.secondary = secondary;
        self
    }

    /// The city states (see [`Self::city_states`]); cut like the colours.
    pub fn with_city_states(mut self, mut city_states: Vec<bool>) -> Self {
        city_states.truncate(self.colors.len());
        self.city_states = city_states;
        self
    }

    /// The colour of the band of `owner`'s border.
    pub fn primary_color(&self, owner: i8) -> [u8; 3] {
        usize::try_from(owner)
            .ok()
            .and_then(|i| self.colors.get(i))
            .copied()
            .unwrap_or([128; 3])
    }

    /// The colour of the fill of `owner`'s border: the second colour, or the first made lighter.
    pub fn secondary_color(&self, owner: i8) -> [u8; 3] {
        usize::try_from(owner)
            .ok()
            .and_then(|i| self.secondary.get(i))
            .copied()
            .unwrap_or_else(|| self.primary_color(owner).map(|c| c / 2 + 127))
    }

    pub fn is_city_state(&self, owner: i8) -> bool {
        usize::try_from(owner)
            .ok()
            .and_then(|i| self.city_states.get(i))
            .copied()
            .unwrap_or(false)
    }

    /// `None` if `owners.len() != width * height`. The colours are cut after the last owner that
    /// has a cell, so that the same borders are the same input however long the list was.
    pub fn new(width: u32, height: u32, owners: Vec<i8>, mut colors: Vec<[u8; 3]>) -> Option<Self> {
        if owners.len() != width as usize * height as usize {
            return None;
        }
        let used = owners
            .iter()
            .copied()
            .max()
            .map_or(0, |m| usize::try_from(m).map_or(0, |m| m + 1));
        colors.truncate(used);
        Some(Self {
            width,
            height,
            owners,
            colors,
            secondary: Vec::new(),
            city_states: Vec::new(),
        })
    }

    /// The owners of a terrain snapshot: the B1 adapter of `Renderer::set_terrain`, until every
    /// caller feeds `set_owners` itself.
    pub fn from_snapshot(snapshot: &TerrainSnapshot) -> Self {
        let colors = (0..)
            .map_while(|id| {
                snapshot
                    .cells()
                    .iter()
                    .any(|c| c.owner >= id)
                    .then(|| snapshot.owner_color(id))
            })
            .collect();
        Self::new(
            snapshot.width(),
            snapshot.height(),
            snapshot.cells().iter().map(|c| c.owner).collect(),
            colors,
        )
        .expect("a snapshot has a cell per plot")
    }

    /// Cells whose owner differs from `other`'s, as indices; `None` when the size differs.
    pub fn changed_cells(&self, other: &Self) -> Option<Vec<u32>> {
        ((self.width, self.height) == (other.width, other.height)).then(|| {
            self.owners
                .iter()
                .zip(&other.owners)
                .enumerate()
                .filter(|(_, (a, b))| a != b)
                .map(|(i, _)| i as u32)
                .collect()
        })
    }
}

/// Counters of `?stats=1`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OverlayCounters {
    /// Meshes of the ground lines built (a new map, new owners).
    pub ground_builds: u64,
    /// Indices of the ground lines: rivers and borders, and the grid after them.
    pub ground_indices: u32,
    pub grid_indices: u32,
    /// Meshes of the highlights built.
    pub highlight_builds: u64,
    pub highlight_indices: u32,
    /// `set_owners` calls that changed something.
    pub owner_updates: u64,
    /// Frames the pass `ScreenOverlay` drew (the map of the overlay and the cells).
    pub map_passes: u64,
    /// Hexes of the grid in the map of the last such frame.
    pub grid_hexes: u32,
    /// Draws of the cells in the last such frame.
    pub cell_draws: u32,
    /// Meshes of a player's borders built (a player whose cells or colours changed counts one).
    pub border_builds: u64,
    /// Indices of all the borders' meshes.
    pub border_indices: u32,
    /// Meshes of the styled highlights built, and their indices.
    pub style_builds: u64,
    pub style_indices: u32,
}

/// Rivers, borders and grid over the map (blend pipeline), drawn once per terrain copy.
struct GpuGround {
    vertices: Buf,
    indices: Buf,
    /// Rivers and borders `0..overlay_count`; the grid follows, `overlay_count..total`.
    overlay_count: u32,
    /// The strips of the rivers are `0..river_count`, the borders follow.
    river_count: u32,
    total: u32,
    copies: Vec<(f32, wgpu::BindGroup)>,
}

/// The indices of the lines over the ground to draw, `(start, end)`, from `[river_count,
/// overlay_count, total]`: the strips of the rivers, then the borders, then the grid.
///
/// While the pass `ScreenOverlay` runs the borders and the grid go into the map of the overlay only
/// (the original has no other border line): drawing B1's lifted copy of the borders as well put a
/// second line next to the first, shifted by the relief, the doubled borders of the report. The
/// native rivers (`world-water` WW-4) replace B1's strips.
fn ground_draw_range(
    [river_count, overlay_count, total]: [u32; 3],
    grid_visible: bool,
    screen_overlay: bool,
    rivers_native: bool,
) -> (u32, u32) {
    let end = if screen_overlay {
        river_count
    } else if grid_visible {
        total
    } else {
        overlay_count
    };
    let start = if rivers_native { river_count } else { 0 };
    (start, end)
}

/// Draws the lines over the ground and the highlights.
pub struct Overlay {
    env: Option<B1Env>,
    ground: Option<Rc<TerrainSnapshot>>,
    owners: Option<OwnersInput>,
    /// Bumped whenever the owners change.
    owners_rev: u64,
    fog: Option<Rc<FogGpu>>,
    /// The selected plot (the ring of the original takes it); the path is `MovementPath`'s and the
    /// cells go through `hl_set`.
    highlights: Highlights,
    /// Hex grid on/off (G key); off like in the original until switched on.
    grid_visible: bool,
    /// Seconds, for the turn of the selection ring.
    time: f32,
    ground_dirty: bool,
    gpu: Option<GpuGround>,
    /// The lines into the class mask pass, writing `OVERLAY_BIT`.
    mask_pipeline: Option<wgpu::RenderPipeline>,
    counters: OverlayCounters,
    /// The camera of the frame about to be drawn.
    view: FowView,
    /// `OverlayLevel` and `Use Screen Space Overlay`.
    overlay_level: i64,
    use_screen_space: i64,
    style: HexGridStyle,
    /// The rivers of `world-water` are drawn: B1's strips are not.
    rivers_native: bool,
    /// The map of the overlay and the cell pass (WOV2): drawn when the ground gives its cells.
    passes: Option<Passes>,
    /// The ground draws its cells through `world-terrain` (`set_terrain_cells`): the pass runs on
    /// them and not on the triangles of `set_cell_geometry`.
    terrain_cells: bool,
    /// The rectangle of the map of this frame.
    map_view: Option<MapView>,
    /// The borders of the players (WOV4): the meshes, and which of them are out of date.
    borders: Borders,
    border_stale: Stale,
    /// The meshes changed: the buffers of the pass are made again.
    borders_gpu_dirty: bool,
    /// The highlights of the styles (WOV5): the library, the set asked for and its mesh.
    hl_library: HighlightLibrary,
    hl_set: HighlightSet,
    hl_mesh: HighlightMesh,
    hl_dirty: bool,
    hl_gpu_dirty: bool,
    hex_models: HexModels,
    /// The selection ring of the map of the overlay (WOV5): its look.
    ring_style: RingStyle,
    /// The turn and `fAlpha` of the ring when pinned (golden).
    ring_pin: Option<(f32, f32)>,
}

/// Which players' border meshes must be built again.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Stale {
    All,
    Some(BTreeSet<i8>),
}

impl Stale {
    fn add(&mut self, owner: i8) {
        if let Self::Some(set) = self {
            set.insert(owner);
        }
    }
}

/// What the pass `ScreenOverlay` draws with.
struct Passes {
    grid: GridGpu,
    borders: BordersGpu,
    highlights: HighlightGpu,
    ring: RingGpu,
    cells: CellsGpu,
}

impl Default for Overlay {
    fn default() -> Self {
        Self {
            owners_rev: 0,
            env: None,
            ground: None,
            owners: None,
            fog: None,
            highlights: Highlights::default(),
            grid_visible: false,
            time: 0.0,
            ground_dirty: false,
            gpu: None,
            mask_pipeline: None,
            counters: OverlayCounters::default(),
            view: FowView::default(),
            // The defaults of the user's file: both on.
            overlay_level: 2,
            use_screen_space: 1,
            style: HexGridStyle::default(),
            rivers_native: false,
            passes: None,
            terrain_cells: false,
            map_view: None,
            borders: Borders::default(),
            border_stale: Stale::All,
            borders_gpu_dirty: false,
            hl_library: HighlightLibrary::default(),
            hl_set: HighlightSet::default(),
            hl_mesh: HighlightMesh::default(),
            hl_dirty: false,
            hl_gpu_dirty: false,
            hex_models: HexModels::default(),
            ring_style: RingStyle::default(),
            ring_pin: None,
        }
    }
}

impl Overlay {
    /// Hands over B1's pipelines (`Renderer` does it once).
    pub(crate) fn bind(&mut self, env: &B1Env) {
        self.mask_pipeline = Some(mask_pipeline(env));
        let grid = GridGpu::new(&env.device, &env.queue);
        grid.set_style(&env.queue, &self.style);
        self.passes = Some(Passes {
            grid,
            borders: BordersGpu::new(&env.device, &env.queue),
            highlights: HighlightGpu::new(&env.device, &env.queue),
            ring: RingGpu::new(&env.device, &env.queue),
            cells: CellsGpu::new(&env.device),
        });
        self.borders_gpu_dirty = true;
        self.env = Some(env.clone());
    }

    /// `OverlayLevel` and `Use Screen Space Overlay` (the renderer gives them at the start and when
    /// they change).
    pub fn set_overlay_settings(&mut self, overlay_level: i64, use_screen_space: i64) {
        self.overlay_level = overlay_level;
        self.use_screen_space = use_screen_space;
    }

    /// The camera of the frame about to be drawn.
    pub fn set_frame(&mut self, view: FowView) {
        self.view = view;
    }

    /// The rivers are drawn natively: B1's strips are left out.
    pub fn set_rivers_native(&mut self, native: bool) {
        self.rivers_native = native;
    }

    /// The look of the grid (`HexGrid.xml`).
    pub fn set_hex_grid_style(&mut self, style: HexGridStyle) {
        self.style = style;
        if let (Some(env), Some(p)) = (&self.env, &self.passes) {
            p.grid.set_style(&env.queue, &style);
        }
    }

    /// `hexgrid.dds`. Without it the ring of every hex is the line. Returns whether it was taken.
    pub fn set_hex_grid_texture(&mut self, blob: &asset_format::TextureBlob) -> bool {
        match (&self.env, &mut self.passes) {
            (Some(env), Some(p)) => p.grid.set_texture(&env.device, &env.queue, blob),
            _ => false,
        }
    }

    /// The cells of the ground with their relief, for the cell pass (`world-terrain`'s
    /// `cell_geometry()`). An empty geometry stops the pass and gives the grid back to B1's line.
    pub fn set_cell_geometry(&mut self, geometry: &CellGeometry) {
        if let (Some(env), Some(p)) = (&self.env, &mut self.passes) {
            p.cells.set_geometry(&env.device, geometry);
        }
    }

    /// Whether `world-terrain` has cells on the screen to draw the pass through (its `cell_geometry()`;
    /// `WorldSystems` says it before every `prepare`). Without them and without triangles from
    /// `set_cell_geometry` the pass does not run and B1's lines stay.
    pub fn set_terrain_cells(&mut self, drawn: bool) {
        self.terrain_cells = drawn;
    }

    /// The pass `ScreenOverlay` draws this frame: the ground has given its cells and the settings ask
    /// for it. The grid then goes into the map of the overlay and not over the ground.
    pub fn screen_overlay_active(&self) -> bool {
        screen_overlay_used(self.overlay_level, self.use_screen_space)
            && (self.terrain_cells || self.passes.as_ref().is_some_and(|p| p.cells.has_geometry()))
    }

    /// The relief the lines lie on: a new map, everything is built again.
    pub fn set_ground(&mut self, ground: Rc<TerrainSnapshot>) {
        self.ground = Some(ground);
        self.ground_dirty = true;
        self.hl_dirty = true;
        self.border_stale = Stale::All;
    }

    /// The styles of `Highlights.xml` (`WorldSettings::highlights`); the mesh is made again.
    pub fn set_highlight_styles(&mut self, styles: &[asset_format::HighlightStyle]) {
        self.hl_library = HighlightLibrary::from_styles(styles);
        for w in self.hl_library.warnings() {
            tracing::warn!("{w}");
        }
        self.hl_dirty = true;
    }

    pub fn highlight_library(&self) -> &HighlightLibrary {
        &self.hl_library
    }

    /// A texture the styles name, by the id they have (`HighlightLibrary::texture_ids`). Returns
    /// whether it was taken.
    pub fn set_highlight_texture(&mut self, id: &str, blob: &asset_format::TextureBlob) -> bool {
        match (&self.env, &mut self.passes) {
            (Some(env), Some(p)) => p.highlights.set_texture(&env.device, &env.queue, id, blob),
            _ => false,
        }
    }

    /// `UnitSelectionHighlight` of `Civ5ArtDefines_Overlay.xml`: the colour, pulse and turn of the ring.
    pub fn set_ring_style(&mut self, defs: &asset_format::OverlayDefs) {
        self.ring_style = RingStyle::from_defs(defs);
    }

    pub fn ring_style(&self) -> &RingStyle {
        &self.ring_style
    }

    /// Pins the turn (radians) and `fAlpha` of the ring instead of the clock's (golden: the
    /// values of a capture's `PSValidMoves` draw); `None` follows the clock again.
    pub fn set_ring_pin(&mut self, pin: Option<(f32, f32)>) {
        self.ring_pin = pin;
    }

    /// The two textures of the ring (`ring::TEXTURES`). Returns how many were taken; without the
    /// ring and its glow no ring is drawn.
    pub fn set_ring_textures(
        &mut self,
        textures: &std::collections::HashMap<asset_format::LogicalId, asset_format::TextureBlob>,
    ) -> usize {
        match (&self.env, &mut self.passes) {
            (Some(env), Some(p)) => p.ring.set_textures(&env.device, &env.queue, textures),
            _ => 0,
        }
    }

    /// What to highlight through the styles: `(style, cell, colour)`. The same set again builds
    /// nothing; returns true if it was a change.
    pub fn set_highlight_set(&mut self, set: HighlightSet) -> bool {
        if self.hl_set == set {
            return false;
        }
        self.hl_set = set;
        self.hl_dirty = true;
        true
    }

    /// The mesh of the set, as of the last `prepare`.
    pub fn highlight_mesh(&self) -> &HighlightMesh {
        &self.hl_mesh
    }

    /// Attaches the animations of the `HexModel` placements to `anim` (call it after the set and the
    /// models change, with the service that the world keeps). Returns how many are attached.
    pub fn sync_hex_models(&mut self, anim: &mut crate::anim::AnimService) -> usize {
        self.hex_models.sync(anim, &self.hl_mesh.models)
    }

    /// The `HexModel` placements with their poses, for the drawer of models.
    pub fn hex_model_instances(
        &self,
        anim: &crate::anim::AnimService,
    ) -> Vec<highlights::model::ModelInstance> {
        self.hex_models.instances(anim)
    }

    /// Makes the mesh of the set if it is out of date.
    fn update_highlights(&mut self) {
        if !self.hl_dirty {
            return;
        }
        let Some(ground) = &self.ground else {
            return;
        };
        self.hl_dirty = false;
        self.hl_mesh = highlights::build(&self.hl_set, &self.hl_library, &ground.shape());
        self.hl_gpu_dirty = true;
        self.counters.style_builds += 1;
        self.counters.style_indices = self.hl_mesh.indices.len() as u32;
    }

    /// `CultureBorders.xml`; a different look builds every mesh again.
    pub fn set_border_style(&mut self, style: BorderStyle) {
        if self.borders.set_style(style) {
            self.border_stale = Stale::All;
            self.ground_dirty = true;
        }
    }

    /// Who owns which cell. The same owners again do nothing; returns true if something changed.
    /// Only the players whose cells or colours differ get a new border mesh.
    pub fn set_owners(&mut self, owners: OwnersInput) -> bool {
        if self.owners.as_ref() == Some(&owners) {
            return false;
        }
        let diff = self.owners.as_ref().and_then(|old| {
            let cells = owners.changed_cells(old)?;
            let mut players = BTreeSet::new();
            for i in cells {
                players.insert(old.owners[i as usize]);
                players.insert(owners.owners[i as usize]);
            }
            // The colours and the kind of a player change its mesh without a cell moving.
            let ids = old.colors.len().max(owners.colors.len()) as i8;
            for id in 0..ids {
                if old.primary_color(id) != owners.primary_color(id)
                    || old.secondary_color(id) != owners.secondary_color(id)
                    || old.is_city_state(id) != owners.is_city_state(id)
                {
                    players.insert(id);
                }
            }
            Some(players)
        });
        match (diff, &mut self.border_stale) {
            (Some(players), stale @ Stale::Some(_)) => {
                players.into_iter().for_each(|p| stale.add(p))
            }
            (Some(players), Stale::All) => {
                let _ = players;
            }
            (None, stale) => *stale = Stale::All,
        }
        self.owners = Some(owners);
        self.owners_rev += 1;
        self.ground_dirty = true;
        self.counters.owner_updates += 1;
        true
    }

    /// Changes whenever [`Self::owners`] does.
    pub fn owners_revision(&self) -> u64 {
        self.owners_rev
    }

    pub fn owners(&self) -> Option<&OwnersInput> {
        self.owners.as_ref()
    }

    /// The fog-state texture the blend shader reads (`FluidFow::fog_gpu`).
    pub fn set_fog_state(&mut self, fog: Option<Rc<FogGpu>>) {
        self.fog = fog;
    }

    /// The selected plot, which the ring of the original takes (the cells go through
    /// [`Self::set_highlight_set`] and the path is left to `MovementPath`). Replaces the previous
    /// one; an empty set clears the ring.
    pub fn set_highlights(&mut self, highlights: &Highlights) {
        self.highlights = Highlights {
            selected: highlights.selected,
            ..Highlights::default()
        };
    }

    pub fn highlights(&self) -> &Highlights {
        &self.highlights
    }

    /// The cell the selection ring was drawn on in the last frame: it has a cell and its textures.
    pub fn ring_drawn(&self) -> Option<(i32, i32)> {
        self.passes
            .as_ref()
            .filter(|p| p.ring.shown())
            .and(self.highlights.selected)
    }

    /// Shows or hides the hex grid (G key).
    pub fn set_grid_visible(&mut self, visible: bool) {
        self.grid_visible = visible;
    }

    pub fn grid_visible(&self) -> bool {
        self.grid_visible
    }

    /// Time in seconds; the selection ring pulses with it. Call once per frame.
    pub fn set_time(&mut self, seconds: f32) {
        self.time = seconds;
    }

    /// Back to no map.
    pub fn clear(&mut self) {
        self.ground = None;
        self.gpu = None;
        self.ground_dirty = false;
    }

    pub fn stats(&self) -> OverlayCounters {
        self.counters
    }

    /// Builds the border meshes of the players that are out of date (`set_owners` says which).
    fn update_borders(&mut self) {
        let Some(ground) = self.ground.clone() else {
            return;
        };
        let stale = std::mem::replace(&mut self.border_stale, Stale::Some(BTreeSet::new()));
        if matches!(&stale, Stale::Some(set) if set.is_empty()) {
            return;
        }
        let from_ground;
        let input = match &self.owners {
            Some(o) if (o.width, o.height) == (ground.width(), ground.height()) => o,
            _ => {
                from_ground = OwnersInput::from_snapshot(&ground);
                &from_ground
            }
        };
        let which = match &stale {
            Stale::All => None,
            Stale::Some(set) => Some(set),
        };
        self.borders.rebuild(input, &ground.shape(), which);
        self.counters.border_builds = self.borders.builds();
        self.counters.border_indices = self.borders.index_count();
        self.borders_gpu_dirty = true;
        // The line over the ground is made of the same meshes.
        self.ground_dirty = true;
    }

    fn rebuild_ground(&mut self, env: &B1Env) {
        self.gpu = None;
        let Some(ground) = &self.ground else { return };
        let snapshot: &TerrainSnapshot = ground;
        let mut mesh = overlay_mesh(snapshot);
        // The borders of the players, lifted onto the relief: the line over the ground.
        borders::blend_fallback(&self.borders.merged(), snapshot, &mut mesh);
        let BlendMesh {
            vertices,
            rivers,
            borders,
            grid,
        } = mesh;
        let river_count = rivers.len() as u32;
        let overlay_count = (rivers.len() + borders.len()) as u32;
        let indices: Vec<u32> = rivers.into_iter().chain(borders).chain(grid).collect();
        self.counters.ground_builds += 1;
        self.counters.ground_indices = overlay_count;
        self.counters.grid_indices = indices.len() as u32 - overlay_count;
        self.gpu = Some(GpuGround {
            vertices: env.device.buf_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("overlay vertices"),
                    contents: bytemuck::cast_slice(&vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                },
                Cluster::Other,
            ),
            indices: env.device.buf_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("overlay indices"),
                    contents: bytemuck::cast_slice(&indices),
                    usage: wgpu::BufferUsages::INDEX,
                },
                Cluster::Other,
            ),
            overlay_count,
            river_count,
            total: indices.len() as u32,
            copies: env.copy_groups(&map_shifts(snapshot)),
        });
    }

    fn draw_ground(&self, cx: &mut DrawContext<'_, '_>) {
        let (Some(g), Some(fog)) = (&self.gpu, &self.fog) else {
            return;
        };
        let (Some(env), Some(copy)) = (&self.env, copy_group(&g.copies, cx.copy_shift)) else {
            return;
        };
        let (start, end) = ground_draw_range(
            [g.river_count, g.overlay_count, g.total],
            self.grid_visible,
            self.screen_overlay_active(),
            self.rivers_native,
        );
        if end <= start {
            return;
        }
        let pass = &mut *cx.pass;
        pass.set_pipeline_c(&env.blend.pipeline);
        pass.set_bind_group_c(0, cx.view_group, &[]);
        pass.set_bind_group_c(2, &fog.bind_group, &[]);
        pass.set_vertex_buffer(0, g.vertices.slice(..));
        pass.set_index_buffer(g.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.set_bind_group_c(1, copy, &[]);
        cx.stats.record_draw(end - start);
        pass.draw_indexed_c(start..end, 0, 0..1);
    }
}

/// The pipeline that draws the lines into the class mask: the vertex stage of the blend pipeline,
/// groups 0 (the view) and 1 (the copy), and the fragment of `common/mask.wgsl` with the class bit
/// `OVERLAY_BIT`. Every pixel of a line gets the bit, the feathered edges and the lines of cells
/// that are hidden from the player too: the class mask is a coarse tool of `golden`.
fn mask_pipeline(env: &B1Env) -> wgpu::RenderPipeline {
    let vertex = env
        .device
        .create_shader_module(wgpu::include_wgsl!("../blend.wgsl"));
    let fragment = crate::shaders::create_module(
        &env.device,
        "overlay mask",
        crate::wgsl_module!("common/mask.wgsl"),
    );
    let view_layout = env.blend.pipeline.get_bind_group_layout(0);
    let layout = env
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay mask"),
            bind_group_layouts: &[Some(&view_layout), Some(&env.object_layout)],
            immediate_size: 0,
        });
    let constants = [(
        "MASK_CLASS",
        f64::from(crate::frame::mask::class::OVERLAY_BIT),
    )];
    env.device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay mask"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &vertex,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<BlendVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3, 1 => Float32, 2 => Float32x4, 3 => Uint32
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            // As in the main pass: tested against the terrain, never written, a bias towards the
            // camera for the strips that lie on a slope.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::frame::DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState {
                    constant: -2,
                    slope_scale: -1.0,
                    clamp: 0.0,
                },
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &fragment,
                entry_point: Some("fs_mask"),
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

impl Overlay {
    /// The ground lines into the class mask pass.
    fn mask_lines(&self, slot: PassSlot, cx: &mut MaskContext<'_, '_>) {
        let (Some(pipeline), Some(g)) = (&self.mask_pipeline, &self.gpu) else {
            return;
        };
        let Some(copy) = copy_group(&g.copies, cx.copy_shift) else {
            return;
        };
        let (vertices, indices, count) = match slot {
            PassSlot::Main(MainSlot::Terrain) => {
                let end = if self.grid_visible && !self.screen_overlay_active() {
                    g.total
                } else {
                    g.overlay_count
                };
                (&g.vertices, &g.indices, end)
            }
            _ => return,
        };
        if count == 0 {
            return;
        }
        let pass = &mut *cx.pass;
        pass.set_pipeline_c(pipeline);
        pass.set_bind_group_c(0, cx.view_group, &[]);
        pass.set_bind_group_c(1, copy, &[]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed_c(0..count, 0, 0..1);
    }
}

impl Overlay {
    /// The hexes of the grid in the rectangle of the map: the cells of the map in it, centres in the
    /// original's basis. Empty without a map.
    fn grid_centres(&self, view: &MapView) -> Vec<Vec2> {
        let Some(ground) = &self.ground else {
            return Vec::new();
        };
        let shape = ground.shape();
        let ours_to_orig = self.view.ours_to_orig;
        let (lo, hi) = view.ours_rect(&ours_to_orig);
        cells_in_rect(lo, hi, crate::hexgrid::CORNER_RADIUS)
            .into_iter()
            .filter(|&(x, y)| shape.normalize(x, y).is_some())
            .map(|(x, y)| {
                let c = cell_center(x, y);
                ours_to_orig
                    .transform_point3(glam::Vec3::new(c.x, 0.0, c.y))
                    .truncate()
            })
            .collect()
    }

    /// The CPU half of the pass: the rectangle of the map, the grid's mesh and the constants.
    fn prepare_screen_overlay(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.map_view = None;
        self.counters.grid_hexes = 0;
        if !self.screen_overlay_active() {
            return;
        }
        let shifts = self.ground.as_deref().map_or_else(|| vec![0.0], map_shifts);
        let (lo, hi) = visible_rect(
            &self.view.view_proj,
            self.view.eye,
            &self.view.ours_to_orig,
            FALLBACK_HALF,
        );
        let view = MapView::of(lo.truncate(), hi.truncate());
        let centres = if self.grid_visible {
            self.grid_centres(&view)
        } else {
            Vec::new()
        };
        let (vertices, indices) = grid_mesh(&centres, self.style.width);
        self.counters.grid_hexes = (vertices.len() / grid::VERTICES_PER_HEX) as u32;
        if let Some(p) = &mut self.passes {
            p.grid
                .set_frame(queue, &view.projection(), &vertices, &indices);
            p.cells.set_frame(
                queue,
                &self.view.view_proj,
                &view.world_to_map(&self.view.ours_to_orig),
            );
            p.cells.set_map_projection(queue, &view.projection());
            if std::mem::take(&mut self.borders_gpu_dirty) {
                p.borders.set_mesh(device, &self.borders.merged());
            }
            p.borders
                .set_frame(queue, &view.projection(), &self.view.ours_to_orig, &shifts);
            if std::mem::take(&mut self.hl_gpu_dirty) {
                p.highlights.set_mesh(device, queue, &self.hl_mesh);
            }
            p.highlights
                .set_frame(queue, &view.projection(), &self.view.ours_to_orig, &shifts);
            let cell = self.highlights.selected;
            p.ring.set_frame(
                queue,
                &ring::RingFrame {
                    projection: &view.projection(),
                    ours_to_orig: &self.view.ours_to_orig,
                    shifts: &shifts,
                    cell,
                    style: &self.ring_style,
                    time: self.time,
                    pin: self.ring_pin,
                },
            );
        }
        self.map_view = Some(view);
    }

    /// `OverlaySystem::Render` and `TerrainSystem::RenderOverlay`: the map of the overlay, then the
    /// cells into `ScreenOverlay`.
    pub(crate) fn encode_screen_overlay(
        &mut self,
        cx: &mut EncoderContext<'_>,
        terrain: Option<&TerrainCells<'_>>,
        trade: Option<&Trade>,
    ) {
        if self.map_view.is_none() {
            return;
        }
        let Some(passes) = &mut self.passes else {
            return;
        };
        let (Some(map), Some(screen), Some(depth)) = (
            cx.targets.get(OVERLAY_MAP),
            cx.targets.get(SCREEN_OVERLAY),
            cx.targets.get(OVERLAY_DEPTH),
        ) else {
            return;
        };
        passes.cells.bind_map(cx.device, map);
        let clear = wgpu::Operations {
            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            store: wgpu::StoreOp::Store,
        };
        // 1. The map: cleared ("Primer Pass Clear"), then the grid.
        let timestamps = cx.timestamps();
        let mut sizes: Vec<u32> = Vec::new();
        {
            let mut rp = cx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Terrain Overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &map.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: clear,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: timestamps,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if self.grid_visible {
                passes.grid.draw(&mut rp);
                if passes.grid.index_count() > 0 {
                    sizes.push(passes.grid.index_count());
                }
            }
        }
        // 1b. The borders, over the grid, with a depth that lets the first triangle at a pixel win.
        passes.borders.ensure_depth(cx.device, map.size);
        if passes.borders.index_count() > 0
            && let Some(depth) = passes.borders.depth()
        {
            let mut rp = cx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Terrain Overlay Borders"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &map.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            sizes.extend(passes.borders.draw(&mut rp));
        }
        // 1c. The highlights of the styles, over the borders, and the selection ring.
        if passes.highlights.index_count() > 0 || passes.ring.shown() {
            let mut rp = cx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Terrain Overlay Highlights"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &map.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            sizes.extend(passes.highlights.draw(&mut rp));
            if passes.ring.shown() {
                passes.ring.draw(&mut rp);
                sizes.push(6);
            }
        }
        // 1d. The trade routes, over the rest of the overlay (`OverlaySystem::Render`).
        if let (Some(trade), Some(view)) = (trade, &self.map_view)
            && trade.drawn()
        {
            let shifts = self.ground.as_deref().map_or_else(|| vec![0.0], map_shifts);
            let mut rp = cx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Terrain Overlay Trade"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &map.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            sizes.extend(trade.draw(
                &mut rp,
                &view.projection(),
                &self.view.ours_to_orig,
                &shifts,
            ));
        }
        // 2. The cells, with the relief, into the screen's target.
        {
            let mut rp = cx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Screen Overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &screen.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: clear,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth.view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let cell_sizes = passes.cells.draw(&mut rp);
            let mut draws = cell_sizes.len() as u32;
            sizes.extend(cell_sizes);
            if self.terrain_cells
                && let Some(cells) = terrain
            {
                // One draw per cell of the ground, as the ground draws them itself.
                let drawn = passes.cells.draw_terrain(cx.device, &mut rp, cells);
                draws += drawn.iter().sum::<u32>();
            }
            self.counters.cell_draws = draws;
        }
        self.counters.map_passes += 1;
        for n in sizes {
            cx.stats.record_draw(n);
        }
    }
}

impl RenderSystem for Overlay {
    fn id(&self) -> SystemId {
        SystemId::Overlay
    }

    fn passes(&self) -> &'static [PassSlot] {
        &[
            PassSlot::Pass(PassId::ScreenOverlay),
            PassSlot::Main(MainSlot::Terrain),
        ]
    }

    fn init(&mut self, cx: &mut InitContext<'_>) {
        // The depth of the cell pass: the nearest cell wins where the relief overlaps on the screen.
        let _ = cx.targets.declare(
            OVERLAY_DEPTH,
            ResourceDesc::color(DEPTH_FORMAT, TargetSize::Screen { num: 1, den: 1 }),
        );
    }

    fn on_settings(
        &mut self,
        settings: &RenderSettings,
        changed: &KeySet,
        _cx: &mut InitContext<'_>,
    ) {
        if changed.contains(OVERLAY_LEVEL) || changed.contains(USE_SCREEN_SPACE_OVERLAY) {
            self.set_overlay_settings(
                settings.get(OVERLAY_LEVEL).unwrap_or(2),
                settings.get(USE_SCREEN_SPACE_OVERLAY).unwrap_or(1),
            );
        }
    }

    fn prepare(&mut self, cx: &mut PrepareContext<'_>) {
        self.update_borders();
        self.update_highlights();
        self.prepare_screen_overlay(cx.device, cx.queue);
        let Some(env) = self.env.clone() else { return };
        if std::mem::take(&mut self.ground_dirty) {
            self.rebuild_ground(&env);
        }
    }

    fn record(&self, slot: PassSlot, cx: &mut DrawContext<'_, '_>) {
        if let PassSlot::Main(MainSlot::Terrain) = slot {
            self.draw_ground(cx);
        }
    }

    fn record_offscreen(&mut self, pass: PassId, cx: &mut EncoderContext<'_>) {
        if pass == PassId::ScreenOverlay {
            self.encode_screen_overlay(cx, None, None);
        }
    }

    fn record_mask(&self, slot: PassSlot, cx: &mut MaskContext<'_, '_>) {
        self.mask_lines(slot, cx);
    }

    fn counters(&self) -> Vec<(&'static str, u64)> {
        let c = self.counters;
        vec![
            ("overlay.ground_builds", c.ground_builds),
            ("overlay.ground_indices", u64::from(c.ground_indices)),
            ("overlay.grid_indices", u64::from(c.grid_indices)),
            ("overlay.highlight_builds", c.highlight_builds),
            ("overlay.highlight_indices", u64::from(c.highlight_indices)),
            ("overlay.owner_updates", c.owner_updates),
            ("overlay.map_passes", c.map_passes),
            ("overlay.grid_hexes", u64::from(c.grid_hexes)),
            ("overlay.cell_draws", u64::from(c.cell_draws)),
            ("overlay.border_builds", c.border_builds),
            ("overlay.border_indices", u64::from(c.border_indices)),
            ("overlay.style_builds", c.style_builds),
            ("overlay.style_indices", u64::from(c.style_indices)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::TerrainCell;
    use crate::terrain::test_util::{land, map};

    /// Rivers 0..10, borders 10..30, grid 30..50.
    const COUNTS: [u32; 3] = [10, 30, 50];

    #[test]
    fn the_borders_are_drawn_once_either_into_the_map_of_the_overlay_or_over_the_ground() {
        // The pass ScreenOverlay runs: the borders and the grid are in the map of the overlay, the
        // ground line has none of them (the doubled border of the report was both at once).
        assert_eq!(ground_draw_range(COUNTS, true, true, false), (0, 10));
        assert_eq!(ground_draw_range(COUNTS, false, true, false), (0, 10));
        // Native rivers: nothing is left to draw.
        let (start, end) = ground_draw_range(COUNTS, true, true, true);
        assert!(end <= start);
        // B1's path: rivers and borders, and the grid when it is on.
        assert_eq!(ground_draw_range(COUNTS, false, false, false), (0, 30));
        assert_eq!(ground_draw_range(COUNTS, true, false, false), (0, 50));
        assert_eq!(ground_draw_range(COUNTS, true, false, true), (10, 50));
    }

    fn owned(owner_of: impl Fn(i32) -> i8) -> TerrainSnapshot {
        map(4, 2, |x, _| TerrainCell {
            owner: owner_of(x),
            ..land()
        })
        .with_owner_colors(vec![[200, 60, 60], [60, 110, 200]])
    }

    #[test]
    fn owners_come_from_the_snapshot_with_their_colours() {
        let s = owned(|x| if x < 2 { 0 } else { 1 });
        let o = OwnersInput::from_snapshot(&s);
        assert_eq!(o.owners, [0, 0, 1, 1, 0, 0, 1, 1]);
        assert_eq!(o.colors, [[200, 60, 60], [60, 110, 200]]);
        assert_eq!((o.width, o.height), (4, 2));
    }

    #[test]
    fn a_wrong_sized_owner_list_is_refused() {
        assert!(OwnersInput::new(2, 2, vec![0; 3], vec![]).is_none());
        assert!(OwnersInput::new(2, 2, vec![0; 4], vec![]).is_some());
    }

    #[test]
    fn one_changed_owner_is_one_changed_cell() {
        let a = OwnersInput::from_snapshot(&owned(|_| 0));
        let mut b = a.clone();
        b.owners[5] = 1;
        assert_eq!(b.changed_cells(&a), Some(vec![5]));
        let c = OwnersInput::new(2, 2, vec![0; 4], vec![]).unwrap();
        assert_eq!(c.changed_cells(&a), None);
    }

    /// A 12 x 6 map in three blocks of four columns: players 0, 1 and 2, or nobody on the middle one
    /// with `gap`.
    fn blocks(gap: bool) -> (Rc<TerrainSnapshot>, OwnersInput) {
        let ground = Rc::new(map(12, 6, |_, _| land()));
        let owners = (0..72)
            .map(|i| match (i % 12) / 4 {
                1 if gap => -1,
                b => b as i8,
            })
            .collect();
        let input =
            OwnersInput::new(12, 6, owners, vec![[200, 0, 0], [0, 200, 0], [0, 0, 200]]).unwrap();
        (ground, input)
    }

    #[test]
    fn the_borders_follow_the_owners() {
        let (ground, input) = blocks(false);
        let mut o = Overlay::default();
        o.set_ground(ground);
        o.update_borders();
        assert_eq!(
            o.borders.index_count(),
            0,
            "a ground without owners has no border"
        );
        o.set_owners(input);
        o.update_borders();
        assert!(o.borders.index_count() > 0);
        for owner in 0..3 {
            assert!(
                o.borders.mesh(owner).is_some_and(|m| !m.is_empty()),
                "{owner}"
            );
        }
        assert_eq!(o.stats().border_builds, 3);
    }

    #[test]
    fn a_changed_cell_builds_only_the_two_owners_it_moved_between() {
        let (ground, input) = blocks(false);
        let mut o = Overlay::default();
        o.set_ground(ground);
        o.set_owners(input.clone());
        o.update_borders();
        assert_eq!(o.stats().border_builds, 3);
        // Nothing changed: nothing is built.
        o.update_borders();
        assert_eq!(o.stats().border_builds, 3);
        // A cell of player 0 goes to player 1 (the blocks touch at column 3 and 4).
        let mut moved = input.clone();
        moved.owners[3] = 1;
        let before_2 = o.borders.mesh(2).cloned();
        assert!(o.set_owners(moved));
        o.update_borders();
        assert_eq!(o.stats().border_builds, 5, "players 0 and 1");
        assert_eq!(
            o.borders.mesh(2).cloned(),
            before_2,
            "player 2 kept its mesh"
        );
        // A new colour for player 2 builds player 2 alone.
        let mut recoloured = input.clone();
        recoloured.owners[3] = 1;
        recoloured.colors[2] = [10, 10, 10];
        o.set_owners(recoloured);
        o.update_borders();
        assert_eq!(o.stats().border_builds, 6);
        assert_ne!(o.borders.mesh(2).cloned(), before_2);
    }

    #[test]
    fn a_player_without_cells_loses_its_mesh() {
        let (ground, input) = blocks(false);
        let mut o = Overlay::default();
        o.set_ground(ground);
        o.set_owners(input.clone());
        o.update_borders();
        assert!(o.borders.mesh(1).is_some());
        let (_, gap) = blocks(true);
        o.set_owners(gap);
        o.update_borders();
        assert!(o.borders.mesh(1).is_none());
        assert!(o.borders.mesh(0).is_some());
    }

    #[test]
    fn the_same_cells_with_the_same_look_build_nothing_and_a_new_look_builds_all() {
        let (ground, input) = blocks(false);
        let mut o = Overlay::default();
        o.set_ground(ground);
        o.set_owners(input);
        o.update_borders();
        o.set_border_style(BorderStyle::default());
        o.update_borders();
        assert_eq!(o.stats().border_builds, 3, "the same look");
        o.set_border_style(BorderStyle {
            primary_width: 3.0,
            ..BorderStyle::default()
        });
        o.update_borders();
        assert_eq!(o.stats().border_builds, 6);
    }

    #[test]
    fn the_same_highlights_again_build_nothing() {
        let mut o = Overlay::default();
        let h = Highlights {
            selected: Some((1, 1)),
            reachable: vec![(1, 0)],
            ..Highlights::default()
        };
        o.set_highlights(&h);
        // Only the selected plot is the overlay's: the cells go through the styles, the path is
        // `MovementPath`'s.
        assert_eq!(o.highlights().selected, Some((1, 1)));
        assert!(o.highlights().reachable.is_empty() && o.highlights().path.is_empty());
        o.set_highlights(&Highlights::default());
        assert!(o.highlights().is_empty());
    }

    #[test]
    fn the_grid_is_off_until_switched_on() {
        let mut o = Overlay::default();
        assert!(!o.grid_visible());
        o.set_grid_visible(true);
        assert!(o.grid_visible());
    }

    #[test]
    fn same_owners_are_not_an_update() {
        let mut o = Overlay::default();
        let a = OwnersInput::from_snapshot(&owned(|_| 0));
        assert!(o.set_owners(a.clone()));
        assert!(!o.set_owners(a));
        assert_eq!(o.stats().owner_updates, 1);
    }
}
