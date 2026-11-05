//! The build of the terrain: the stages of
//! `TerrainSystem::Init` in the order of the original with one stream of `rand` from `srand(seed)`.
//! The stage of the coast, the rivers, the marsh and the ice belong to `world-water` and join
//! through [`BuildStage`]; a stage that is not there is empty. The global part of every stage runs
//! once for a basis; the rasterization into the field goes cell by cell (64 x 64 texels), lazily,
//! unless the stage says [`RasterScope::Global`] and writes the whole field in its global part.

pub mod blit;
pub mod bridge;
pub mod matcher;
pub mod rand;
pub mod tables;

use std::cell::RefCell;
use std::collections::HashSet;
use std::fmt;

use glam::Vec2;

use self::rand::MsvcRand;
use super::basis::TerrainBasis;
use super::field::{CELL_SIZE, CellRect, FieldPatch, TerrainField};
use super::library::TerrainLibrary;
use super::sampler::TerrainFrame;
use super::stencil::TerrainStencil;
use crate::world::water::river::floodplains::BankDecal;

/// The stages of `TerrainSystem::Init` that the build owns or lends to `world-water`, in the
/// order of the table 3.1 (steps 4 to 13). The step 0 (`srand`) is the creation of the stream, the
/// steps 1 to 3 are the library, the field and the decal layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StageId {
    CoastSplines,
    CoastalRoads,
    CoastStencil,
    BlendSplines,
    Pieces,
    Marsh,
    CoastBlendMeshes,
    Rivers,
    Ice,
    Cells,
}

/// The name of a stage's raster in the slow frames (`perf::slow`).
fn stage_scope(id: StageId) -> &'static str {
    match id {
        StageId::CoastSplines => "raster.coast_splines",
        StageId::CoastalRoads => "raster.coastal_roads",
        StageId::CoastStencil => "raster.coast_stencil",
        StageId::BlendSplines => "raster.blend_splines",
        StageId::Pieces => "raster.pieces",
        StageId::Marsh => "raster.marsh",
        StageId::CoastBlendMeshes => "raster.coast_blend_meshes",
        StageId::Rivers => "raster.rivers",
        StageId::Ice => "raster.ice",
        StageId::Cells => "raster.cells",
    }
}

/// Who writes a stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageOwner {
    WorldTerrain,
    WorldWater,
}

impl StageId {
    /// All stages in the order of the table.
    // re: TerrainSystem::Init (TerrainSystem.cpp:661-963)
    pub const ORDER: [StageId; 10] = [
        Self::CoastSplines,
        Self::CoastalRoads,
        Self::CoastStencil,
        Self::BlendSplines,
        Self::Pieces,
        Self::Marsh,
        Self::CoastBlendMeshes,
        Self::Rivers,
        Self::Ice,
        Self::Cells,
    ];

    /// The row of the table 3.1.
    pub fn step(self) -> u8 {
        match self {
            Self::CoastSplines => 4,
            Self::CoastalRoads => 5,
            Self::CoastStencil => 6,
            Self::BlendSplines => 7,
            Self::Pieces => 8,
            Self::Marsh => 9,
            Self::CoastBlendMeshes => 10,
            Self::Rivers => 11,
            Self::Ice => 12,
            Self::Cells => 13,
        }
    }

    pub fn owner(self) -> StageOwner {
        match self {
            Self::Pieces | Self::Cells => StageOwner::WorldTerrain,
            _ => StageOwner::WorldWater,
        }
    }
}

/// How a stage writes the field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RasterScope {
    /// Cell by cell, lazily, by [`BuildStage::raster`].
    PerCell,
    /// The whole field in [`BuildStage::global`]; the stage has no per-cell part.
    Global,
}

/// Why a stage failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildError {
    Stage { stage: StageId, message: String },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stage { stage, message } => write!(f, "stage {stage:?}: {message}"),
        }
    }
}

impl std::error::Error for BuildError {}

/// A line of the blend: the spline of the coast of level 2 (range A) or its shift by one (range B).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplineRange {
    A,
    B,
}

/// A spline that the blit hands to the cells it passes through.
// re: cvTerrainBlend.cpp:127-240 (splines A and B)
#[derive(Clone, Debug, PartialEq)]
pub struct BlendSpline {
    pub range: SplineRange,
    /// `SplineRange::field_8`: the class the line marks in a cell of the blit.
    pub class: u8,
    pub points: Vec<Vec2>,
}

/// An edge of a river: the hex and the direction (`DirectionTypes`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RiverEdge {
    pub hex: (i32, i32),
    pub dir: u8,
}

/// A decal a stage asks for: the category of `decal.rs`, the place in
/// our world and the turn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecalRequest {
    pub category: u8,
    pub at: Vec2,
    pub rotation: f32,
}

/// What the stages hand to each other and to the other modules.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuildProducts {
    /// The splines of the blend, for the blit of the pieces.
    pub blend_splines: Vec<BlendSpline>,
    /// The polylines of the coastal roads, for `world-objects`.
    pub coastal_roads: Vec<Vec<Vec2>>,
    /// The noised segments of the shore in the texel plane, by the side of their land hex
    /// (`AddAllCoastlinesToDatabase`), in the order they are submitted: the surf follows them.
    pub coast_segments: Vec<(crate::world::water::river::hex::HexEdge, [Vec2; 2])>,
    /// The edges of the rivers, for `world-water` and `world-objects`.
    pub river_edges: Vec<Vec<RiverEdge>>,
    /// The decals the stages ask for.
    pub decal_requests: Vec<DecalRequest>,
    /// The decals of the flood plains and the banks of the rivers (`cvFloodplainsSystem`), made by
    /// the stage `Rivers`; the terrain creates them in the decal layer when the basis is set.
    pub bank_decals: Vec<BankDecal>,
}

/// What a stage sees and writes in its global part.
pub struct StageCx<'a> {
    pub basis: &'a TerrainBasis,
    pub library: &'a TerrainLibrary,
    pub frame: &'a TerrainFrame,
    /// The stream of `rand` of the thread of `TerrainSystem::Init`.
    pub rand: &'a mut MsvcRand,
    pub products: &'a mut BuildProducts,
    /// The whole field, for the stage of [`RasterScope::Global`].
    pub field: &'a mut TerrainField,
    pub stencil: &'a mut TerrainStencil,
    /// The field as the stages before this one leave it, for a stage that says
    /// [`BuildStage::wants_prior_field`] (`StartupGame` reads the heights the pieces and the coast made).
    pub prior: Option<&'a PriorField<'a>>,
}

/// The field of the stages that run before a stage, asked texel by texel. In the original the field is
/// whole when `cvRiverSimSystem::StartupGame` reads it; here the cells are built lazily, so the cell of a
/// texel is built into a copy of the field the first time it is asked for, by the per-cell parts of the
/// stages before.
pub struct PriorField<'a> {
    stages: &'a [Box<dyn BuildStage>],
    basis: &'a TerrainBasis,
    library: &'a TerrainLibrary,
    frame: &'a TerrainFrame,
    /// The products of the stages before, as they were when the stage started.
    products: BuildProducts,
    state: RefCell<PriorState>,
}

struct PriorState {
    field: TerrainField,
    stencil: TerrainStencil,
    built: HashSet<(i32, i32)>,
}

/// Where [`TerrainBuild::step_global`] stopped: the next stage, and the cells of its prior field made
/// so far.
pub struct GlobalRun {
    next: usize,
    warm: Option<Warm>,
}

struct Warm {
    /// The products as they were when the stage started, and the copy of the field; out while a step
    /// works on them.
    parts: Option<(BuildProducts, PriorState)>,
    texels: Vec<(i32, i32)>,
    at: usize,
}

impl<'a> PriorField<'a> {
    fn new(
        stages: &'a [Box<dyn BuildStage>],
        basis: &'a TerrainBasis,
        library: &'a TerrainLibrary,
        frame: &'a TerrainFrame,
        products: BuildProducts,
        field: &TerrainField,
        stencil: &TerrainStencil,
    ) -> Self {
        Self {
            stages,
            basis,
            library,
            frame,
            products,
            state: RefCell::new(PriorState {
                field: field.clone(),
                stencil: stencil.clone(),
                built: HashSet::new(),
            }),
        }
    }

    /// The state of a warm-up that stopped (see [`TerrainBuild::step_global`]) in place of a new copy
    /// of the field.
    fn from_parts(
        stages: &'a [Box<dyn BuildStage>],
        basis: &'a TerrainBasis,
        library: &'a TerrainLibrary,
        frame: &'a TerrainFrame,
        products: BuildProducts,
        state: PriorState,
    ) -> Self {
        Self {
            stages,
            basis,
            library,
            frame,
            products,
            state: RefCell::new(state),
        }
    }

    fn into_parts(self) -> (BuildProducts, PriorState) {
        (self.products, self.state.into_inner())
    }

    /// The texel `(u, v)` as [`Self::height`] reads it: `u` wraps (the map of the original wraps
    /// along X when the basis says so, else the texel is clamped), `v` is clamped to the rows.
    fn wrapped(&self, state: &PriorState, u: i32, v: i32) -> (i32, i32) {
        let (w, h) = (state.field.width() as i32, state.field.height() as i32);
        let u = if self.basis.wrap_x {
            u.rem_euclid(w)
        } else {
            u.clamp(0, w - 1)
        };
        (u, v.clamp(0, h - 1))
    }

    /// Builds the cell of the texel into the copy of the field if it is not built.
    fn ensure_cell(&self, state: &mut PriorState, cell: (i32, i32)) {
        if state.built.insert(cell) {
            let rect = CellRect::cell(cell.0, cell.1);
            let cx = RasterCx {
                basis: self.basis,
                library: self.library,
                frame: self.frame,
                products: &self.products,
            };
            let PriorState { field, stencil, .. } = state;
            for stage in self.stages {
                if stage.raster_scope() == RasterScope::PerCell {
                    let mut window = FieldWindow::new(field, stencil, rect);
                    stage.raster(rect, &mut window, &cx);
                }
            }
        }
    }

    /// The cell a texel is in, as [`Self::height`] builds it.
    fn cell_of(&self, u: i32, v: i32) -> (i32, i32) {
        let state = self.state.borrow();
        let (u, v) = self.wrapped(&state, u, v);
        let size = CELL_SIZE as i32;
        (u / size, v / size)
    }

    /// Builds one cell ahead of the asks for its heights.
    fn warm_cell(&self, cell: (i32, i32)) {
        let mut state = self.state.borrow_mut();
        self.ensure_cell(&mut state, cell);
    }

    /// The raw height of the texel `(u, v)`; `u` wraps (the map of the original wraps along X when the
    /// basis says so, else the texel is clamped), `v` is clamped to the rows of the field.
    pub fn height(&self, u: i32, v: i32) -> u16 {
        let mut state = self.state.borrow_mut();
        let (u, v) = self.wrapped(&state, u, v);
        let size = CELL_SIZE as i32;
        self.ensure_cell(&mut state, (u / size, v / size));
        state.field.get_height(u, v)
    }
}

/// What a stage sees in its per-cell part: read only.
pub struct RasterCx<'a> {
    pub basis: &'a TerrainBasis,
    pub library: &'a TerrainLibrary,
    pub frame: &'a TerrainFrame,
    pub products: &'a BuildProducts,
}

/// A window on the field and the stencil: writes inside the cell and its border; a write outside
/// is dropped (it belongs to another cell).
pub struct FieldWindow<'a> {
    target: WindowTarget<'a>,
    rect: CellRect,
}

/// Where the writes of a window go.
enum WindowTarget<'a> {
    /// Into the field and the stencil.
    Field {
        field: &'a mut TerrainField,
        stencil: &'a mut TerrainStencil,
    },
    /// Into a copy of the cell's blocks (a cell built on a worker of the pool, [`CellBuilt`]); the
    /// texels outside it are read from the field as it is, which nobody writes meanwhile.
    Patch {
        base: &'a TerrainField,
        patch: &'a mut FieldPatch,
        stencil: &'a mut Vec<(i32, i32, u8)>,
    },
}

impl<'a> FieldWindow<'a> {
    pub fn new(
        field: &'a mut TerrainField,
        stencil: &'a mut TerrainStencil,
        rect: CellRect,
    ) -> Self {
        Self {
            target: WindowTarget::Field { field, stencil },
            rect,
        }
    }

    /// What the window lets through.
    pub fn rect(&self) -> CellRect {
        self.rect
    }

    pub fn height(&self, u: i32, v: i32) -> u16 {
        match &self.target {
            WindowTarget::Field { field, .. } => field.get_height(u, v),
            WindowTarget::Patch { base, patch, .. } => {
                if patch.holds(u, v) {
                    patch.get_height(u, v)
                } else {
                    base.get_height(u, v)
                }
            }
        }
    }

    pub fn id(&self, u: i32, v: i32) -> u8 {
        match &self.target {
            WindowTarget::Field { field, .. } => field.get_id(u, v),
            WindowTarget::Patch { base, patch, .. } => {
                if patch.holds(u, v) {
                    patch.get_id(u, v)
                } else {
                    base.get_id(u, v)
                }
            }
        }
    }

    /// Writes the height; `false` if the texel is outside the window or the map.
    pub fn set_height(&mut self, u: i32, v: i32, raw: u16) -> bool {
        self.rect.contains(u, v)
            && match &mut self.target {
                WindowTarget::Field { field, .. } => {
                    field.set_height(u, v, raw);
                    field.contains(u, v)
                }
                WindowTarget::Patch { base, patch, .. } => {
                    patch.set_height(u, v, raw);
                    base.contains(u, v)
                }
            }
    }

    pub fn set_id(&mut self, u: i32, v: i32, id: u8) -> bool {
        self.rect.contains(u, v)
            && match &mut self.target {
                WindowTarget::Field { field, .. } => {
                    field.set_id(u, v, id);
                    field.contains(u, v)
                }
                WindowTarget::Patch { base, patch, .. } => {
                    patch.set_id(u, v, id);
                    base.contains(u, v)
                }
            }
    }

    /// Writes both.
    pub fn set(&mut self, u: i32, v: i32, raw: u16, id: u8) -> bool {
        self.set_height(u, v, raw) & self.set_id(u, v, id)
    }

    /// Adds stencil bits to the stencil texel of the field texel `(u, v)`.
    pub fn or_stencil(&mut self, u: i32, v: i32, bits: u8) {
        if self.rect.contains(u, v) {
            match &mut self.target {
                WindowTarget::Field { stencil, .. } => stencil.or_bits(u >> 1, v >> 1, bits),
                WindowTarget::Patch { stencil, .. } => stencil.push((u >> 1, v >> 1, bits)),
            }
        }
    }
}

/// A cell rasterized away from the field ([`TerrainBuild::raster_cell_detached`]), to be put into it
/// by [`TerrainBuild::apply_cell`].
pub struct CellBuilt {
    pub cell: CellRect,
    field: FieldPatch,
    /// The stencil bits the stages added, in order.
    stencil: Vec<(i32, i32, u8)>,
}

/// A stage of the build. `world-water` implements the stages of the coast, the rivers, the marsh
/// and the ice; `world-objects` may add one if its road layout draws `rand` in this thread.
///
/// `Sync`: the per-cell part of several cells runs side by side on the pool of `jobs`
/// ([`TerrainBuild::raster_cell_detached`]). A stage's `raster` writes only the texels of its cell and
/// reads only those (what the window lets it write), and takes no lock the main thread could wait on.
pub trait BuildStage: Send + Sync {
    fn id(&self) -> StageId;

    /// Once for a basis: decisions, the stream of `rand`, lines, products.
    fn global(&mut self, cx: &mut StageCx<'_>) -> Result<(), BuildError>;

    fn raster_scope(&self) -> RasterScope {
        RasterScope::PerCell
    }

    /// The global part reads the heights of the field the stages before it make
    /// ([`StageCx::prior`]).
    fn wants_prior_field(&self) -> bool {
        false
    }

    /// The texels of the prior field the global part is going to ask for, in the order it asks, when
    /// the stage can know them before it runs (`rand` is the stream as the stage will find it). The
    /// build then makes their cells a few at a time over several frames
    /// ([`TerrainBuild::step_global`]) and the global part finds them ready. Empty: the cells are
    /// built when asked for.
    fn prior_texels(
        &self,
        _basis: &TerrainBasis,
        _frame: &TerrainFrame,
        _rand: &MsvcRand,
    ) -> Vec<(i32, i32)> {
        Vec::new()
    }

    /// The cell `cell` (64 x 64 texels, the window allows the border).
    fn raster(&self, _cell: CellRect, _window: &mut FieldWindow<'_>, _cx: &RasterCx<'_>) {}

    /// The hexes `changed` (indexes of the basis) have another record: the texel rectangles whose
    /// field the stage has to write again.
    fn invalidate(&mut self, _changed: &[u32]) -> Vec<CellRect> {
        Vec::new()
    }
}

/// The build of one map.
pub struct TerrainBuild {
    seed: u32,
    rand: MsvcRand,
    stages: Vec<Box<dyn BuildStage>>,
    products: BuildProducts,
    ran: Vec<StageId>,
}

impl TerrainBuild {
    /// A build with the stages `world-terrain` owns today: `Pieces`, the matcher and the blit of
    /// the original on the stream.
    pub fn new(seed: u32) -> Self {
        let mut b = Self::empty(seed);
        b.install(Box::new(bridge::MatchedPieces::default()));
        b
    }

    /// A build with no stage.
    pub fn empty(seed: u32) -> Self {
        Self {
            seed,
            rand: MsvcRand::new(seed),
            stages: Vec::new(),
            products: BuildProducts::default(),
            ran: Vec::new(),
        }
    }

    /// Puts a stage in its place of the table; a stage with the same id is replaced and returned.
    pub fn install(&mut self, stage: Box<dyn BuildStage>) -> Option<Box<dyn BuildStage>> {
        let id = stage.id();
        let old = self
            .stages
            .iter()
            .position(|s| s.id() == id)
            .map(|i| self.stages.remove(i));
        let at = self.stages.partition_point(|s| s.id() < id);
        self.stages.insert(at, stage);
        old
    }

    /// The ids of the stages that are installed, in the order they run.
    pub fn stage_ids(&self) -> Vec<StageId> {
        self.stages.iter().map(|s| s.id()).collect()
    }

    /// The ids in the order their global parts ran in the last [`Self::run_global`].
    pub fn ran(&self) -> &[StageId] {
        &self.ran
    }

    pub fn seed(&self) -> u32 {
        self.seed
    }

    /// The stream, to see how many draws the stages took.
    pub fn rand(&self) -> &MsvcRand {
        &self.rand
    }

    pub fn products(&self) -> &BuildProducts {
        &self.products
    }

    /// The global part of every stage, in the order of the table, with the stream restarted at
    /// `srand(seed)` and the products emptied. The seed is the one of the basis.
    pub fn run_global(
        &mut self,
        basis: &TerrainBasis,
        library: &TerrainLibrary,
        frame: &TerrainFrame,
        field: &mut TerrainField,
        stencil: &mut TerrainStencil,
    ) -> Result<(), BuildError> {
        let mut run = self.begin_global(basis);
        while !self.step_global(
            &mut run,
            basis,
            library,
            frame,
            field,
            stencil,
            f64::INFINITY,
        )? {}
        Ok(())
    }

    /// Starts the global parts again for `basis`: the stream restarted, the products emptied. The
    /// stages then run through [`Self::step_global`], in pieces of time.
    pub fn begin_global(&mut self, basis: &TerrainBasis) -> GlobalRun {
        self.seed = basis.seed;
        self.rand = MsvcRand::new(basis.seed);
        self.products = BuildProducts::default();
        self.ran.clear();
        GlobalRun {
            next: 0,
            warm: None,
        }
    }

    /// Runs the global parts from where `run` stopped, for about `budget_ms` (a stage, or one cell of
    /// a stage's prior field, is never cut). `true` when all the stages ran. The result is the one
    /// of [`Self::run_global`], which is this with no end of the budget.
    ///
    /// The stages, the basis, the library and the frame must be the ones of [`Self::begin_global`]
    /// until the run is done.
    #[allow(clippy::too_many_arguments)]
    pub fn step_global(
        &mut self,
        run: &mut GlobalRun,
        basis: &TerrainBasis,
        library: &TerrainLibrary,
        frame: &TerrainFrame,
        field: &mut TerrainField,
        stencil: &mut TerrainStencil,
        budget_ms: f64,
    ) -> Result<bool, BuildError> {
        let t0 = crate::perf::slow::now_ms();
        let over = || crate::perf::slow::now_ms() - t0 >= budget_ms;
        // Each call does something, however small the budget: a cell, or a stage.
        let mut did_work = false;
        loop {
            let i = run.next;
            if i >= self.stages.len() {
                return Ok(true);
            }
            let (before, rest) = self.stages.split_at_mut(i);
            let stage = &mut rest[0];
            let mut prior = None;
            if stage.wants_prior_field() {
                if run.warm.is_none() {
                    let texels = stage.prior_texels(basis, frame, &self.rand);
                    if !texels.is_empty() {
                        run.warm = Some(Warm {
                            parts: Some((
                                self.products.clone(),
                                PriorState {
                                    field: field.clone(),
                                    stencil: stencil.clone(),
                                    built: HashSet::new(),
                                },
                            )),
                            texels,
                            at: 0,
                        });
                    }
                }
                if let Some(warm) = run.warm.as_mut() {
                    let (products, state) = warm.parts.take().expect("the parts are put back");
                    let p = PriorField::from_parts(before, basis, library, frame, products, state);
                    while warm.at < warm.texels.len() {
                        let (u, v) = warm.texels[warm.at];
                        warm.at += 1;
                        let cell = p.cell_of(u, v);
                        let fresh = !p.state.borrow().built.contains(&cell);
                        p.warm_cell(cell);
                        did_work |= fresh;
                        if fresh && over() {
                            break;
                        }
                    }
                    if warm.at < warm.texels.len() {
                        warm.parts = Some(p.into_parts());
                        return Ok(false);
                    }
                    prior = Some(p);
                } else {
                    prior = Some(PriorField::new(
                        before,
                        basis,
                        library,
                        frame,
                        self.products.clone(),
                        field,
                        stencil,
                    ));
                }
                // The cells are in: the global part itself is the next piece of time.
                if did_work
                    && over()
                    && let Some(warm) = run.warm.as_mut()
                {
                    warm.parts = prior.take().map(PriorField::into_parts);
                    return Ok(false);
                }
            }
            let mut cx = StageCx {
                basis,
                library,
                frame,
                rand: &mut self.rand,
                products: &mut self.products,
                field,
                stencil,
                prior: prior.as_ref(),
            };
            stage.global(&mut cx)?;
            self.ran.push(stage.id());
            run.next += 1;
            run.warm = None;
            if over() {
                return Ok(run.next >= self.stages.len());
            }
        }
    }

    /// The per-cell part of the stages, in the order of the table. A stage of
    /// [`RasterScope::Global`] has written its field already and is not called.
    pub fn raster_cell(
        &self,
        cell: CellRect,
        field: &mut TerrainField,
        stencil: &mut TerrainStencil,
        cx: &RasterCx<'_>,
    ) {
        for stage in &self.stages {
            if stage.raster_scope() == RasterScope::PerCell {
                // The border is a kernel of the normal wide: the stage may write the cell only,
                // the window lets the border be read.
                let mut window = FieldWindow::new(field, stencil, cell);
                let _s = crate::perf::slow::scope(stage_scope(stage.id()));
                stage.raster(cell, &mut window, cx);
            }
        }
    }

    /// [`Self::raster_cell`] without writing the field: the stages write a copy of the cell's blocks
    /// (they write only inside the cell and read only the texels they write, see the stages), so
    /// cells can be built side by side on the pool from the same `&field`. The field must not change
    /// until the result is put back with [`Self::apply_cell`], in the order the cells would have been
    /// built in.
    pub fn raster_cell_detached(
        &self,
        cell: CellRect,
        field: &TerrainField,
        cx: &RasterCx<'_>,
    ) -> CellBuilt {
        let mut patch = field.patch_of(cell);
        let mut stencil = Vec::new();
        for stage in &self.stages {
            if stage.raster_scope() == RasterScope::PerCell {
                let mut window = FieldWindow {
                    target: WindowTarget::Patch {
                        base: field,
                        patch: &mut patch,
                        stencil: &mut stencil,
                    },
                    rect: cell,
                };
                let _s = crate::perf::slow::scope(stage_scope(stage.id()));
                stage.raster(cell, &mut window, cx);
            }
        }
        CellBuilt {
            cell,
            field: patch,
            stencil,
        }
    }

    /// Puts a cell of [`Self::raster_cell_detached`] into the field and the stencil: the same bytes
    /// and revisions [`Self::raster_cell`] leaves.
    pub fn apply_cell(built: CellBuilt, field: &mut TerrainField, stencil: &mut TerrainStencil) {
        field.apply_patch(built.field);
        for (u, v, bits) in built.stencil {
            stencil.or_bits(u, v, bits);
        }
    }

    /// The texel rectangles the stages have to write again after the hexes `changed` took other
    /// records.
    pub fn invalidate(&mut self, changed: &[u32]) -> Vec<CellRect> {
        self.stages
            .iter_mut()
            .flat_map(|s| s.invalidate(changed))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::super::basis::PlotBasis;
    use super::*;
    use crate::terrain::{Relief, TerrainKind};

    fn basis(w: u16, h: u16) -> TerrainBasis {
        TerrainBasis::uniform(
            w,
            h,
            false,
            42,
            PlotBasis {
                relief: Relief::Flat,
                terrain: TerrainKind::Grass,
                river_flow: PlotBasis::NO_FLOW,
                ..PlotBasis::default()
            },
        )
    }

    struct Recorder {
        id: StageId,
        scope: RasterScope,
        draws: u64,
        log: Arc<Mutex<Vec<String>>>,
    }

    impl Recorder {
        fn boxed(id: StageId, log: &Arc<Mutex<Vec<String>>>) -> Box<dyn BuildStage> {
            Box::new(Self {
                id,
                scope: RasterScope::PerCell,
                draws: 0,
                log: Arc::clone(log),
            })
        }
    }

    impl BuildStage for Recorder {
        fn id(&self) -> StageId {
            self.id
        }

        fn global(&mut self, cx: &mut StageCx<'_>) -> Result<(), BuildError> {
            self.log
                .lock()
                .unwrap()
                .push(format!("global {:?}", self.id));
            cx.rand.skip(self.draws);
            if self.scope == RasterScope::Global {
                for v in 0..64 {
                    for u in 0..64 {
                        cx.field.set(u, v, 777, 3);
                    }
                }
            }
            Ok(())
        }

        fn raster_scope(&self) -> RasterScope {
            self.scope
        }

        fn raster(&self, cell: CellRect, window: &mut FieldWindow<'_>, _cx: &RasterCx<'_>) {
            self.log.lock().unwrap().push(format!(
                "raster {:?} sees {}",
                self.id,
                window.height(cell.u0, cell.v0)
            ));
        }
    }

    fn run(build: &mut TerrainBuild, b: &TerrainBasis) -> (TerrainField, TerrainStencil) {
        let frame = TerrainFrame::new(u32::from(b.width), u32::from(b.height), b.wrap_x);
        let (w, h) = frame.texel_size();
        let mut field = TerrainField::with_texels(w, h);
        let mut stencil = TerrainStencil::new(w, h);
        build
            .run_global(
                b,
                &TerrainLibrary::synthetic(),
                &frame,
                &mut field,
                &mut stencil,
            )
            .unwrap();
        (field, stencil)
    }

    #[test]
    fn the_stages_are_the_rows_of_the_table_in_order() {
        let steps: Vec<u8> = StageId::ORDER.iter().map(|s| s.step()).collect();
        assert_eq!(steps, [4, 5, 6, 7, 8, 9, 10, 11, 12, 13]);
        assert!(StageId::ORDER.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(StageId::Pieces.owner(), StageOwner::WorldTerrain);
        assert_eq!(StageId::Cells.owner(), StageOwner::WorldTerrain);
        for s in [
            StageId::CoastSplines,
            StageId::CoastalRoads,
            StageId::CoastStencil,
            StageId::BlendSplines,
            StageId::Marsh,
            StageId::CoastBlendMeshes,
            StageId::Rivers,
            StageId::Ice,
        ] {
            assert_eq!(s.owner(), StageOwner::WorldWater, "{s:?}");
        }
    }

    #[test]
    fn a_build_runs_without_the_stages_of_world_water() {
        let mut build = TerrainBuild::new(7);
        assert_eq!(build.stage_ids(), [StageId::Pieces]);
        let b = basis(2, 2);
        run(&mut build, &b);
        assert_eq!(build.ran(), [StageId::Pieces]);
        // The matcher takes the draws of its shuffles: the synthetic library has pieces of passes
        // 0..=2 (the padded grid of 2 x 2 is 6 x 6).
        let mut m = matcher::HexPatternMatcher::new();
        for p in bridge::library_pieces(&TerrainLibrary::synthetic()) {
            m.add_piece(p);
        }
        assert_eq!(build.rand().calls(), m.draws(6, 6));
        assert_eq!(build.products(), &BuildProducts::default());
    }

    #[test]
    fn stages_run_in_the_order_of_the_table_whatever_the_order_they_joined_in() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut build = TerrainBuild::empty(1);
        for id in [
            StageId::Ice,
            StageId::CoastSplines,
            StageId::Pieces,
            StageId::Rivers,
        ] {
            build.install(Recorder::boxed(id, &log));
        }
        assert_eq!(
            build.stage_ids(),
            [
                StageId::CoastSplines,
                StageId::Pieces,
                StageId::Rivers,
                StageId::Ice
            ]
        );
        run(&mut build, &basis(2, 2));
        assert_eq!(build.ran(), build.stage_ids().as_slice());
        let lines = log.lock().unwrap().clone();
        assert_eq!(
            lines,
            [
                "global CoastSplines",
                "global Pieces",
                "global Rivers",
                "global Ice"
            ]
        );
    }

    /// Writes `height` over the cell it is called for; `reader` reads the field before itself.
    struct Floor {
        id: StageId,
        height: u16,
        reads: Vec<(i32, i32)>,
        seen: Arc<Mutex<Vec<u16>>>,
    }

    impl BuildStage for Floor {
        fn id(&self) -> StageId {
            self.id
        }

        fn wants_prior_field(&self) -> bool {
            !self.reads.is_empty()
        }

        fn global(&mut self, cx: &mut StageCx<'_>) -> Result<(), BuildError> {
            if let Some(prior) = cx.prior {
                let mut seen = self.seen.lock().unwrap();
                for &(u, v) in &self.reads {
                    seen.push(prior.height(u, v));
                }
            }
            Ok(())
        }

        fn raster(&self, cell: CellRect, window: &mut FieldWindow<'_>, _cx: &RasterCx<'_>) {
            for v in cell.v0..cell.v1 {
                for u in cell.u0..cell.u1 {
                    window.set_height(u, v, self.height);
                }
            }
        }
    }

    #[test]
    fn a_stage_that_asks_for_the_prior_field_sees_what_the_stages_before_it_wrote() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let floor = |id, height, reads: Vec<(i32, i32)>| {
            Box::new(Floor {
                id,
                height,
                reads,
                seen: Arc::clone(&seen),
            }) as Box<dyn BuildStage>
        };
        let mut build = TerrainBuild::empty(1);
        build.install(floor(StageId::Pieces, 9_000, vec![]));
        // Reads two texels of two cells, one of them past the east edge of a map that wraps, and
        // one row below the map.
        build.install(floor(StageId::Rivers, 1, vec![(70, 5), (-1, 5), (5, -3)]));
        // Never asked for by the stages after it.
        build.install(floor(StageId::Ice, 2, vec![]));
        let mut b = basis(2, 2);
        b.wrap_x = true;
        let (field, _) = run(&mut build, &b);
        assert_eq!(*seen.lock().unwrap(), [9_000, 9_000, 9_000]);
        // Only the real field is untouched: the cells are built lazily, later.
        assert_eq!(field.get_height(70, 5), 0);
    }

    #[test]
    fn the_prior_field_is_not_given_to_a_stage_that_does_not_ask() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        struct Blind(Arc<Mutex<Vec<u16>>>);
        impl BuildStage for Blind {
            fn id(&self) -> StageId {
                StageId::Rivers
            }
            fn global(&mut self, cx: &mut StageCx<'_>) -> Result<(), BuildError> {
                self.0.lock().unwrap().push(u16::from(cx.prior.is_some()));
                Ok(())
            }
        }
        let mut build = TerrainBuild::empty(1);
        build.install(Box::new(Blind(Arc::clone(&seen))));
        run(&mut build, &basis(2, 2));
        assert_eq!(*seen.lock().unwrap(), [0]);
    }

    #[test]
    fn a_stage_that_joins_again_replaces_the_one_before() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut build = TerrainBuild::new(1);
        let old = build.install(Recorder::boxed(StageId::Pieces, &log));
        assert!(old.is_some());
        assert_eq!(build.stage_ids(), [StageId::Pieces]);
        assert!(
            build
                .install(Recorder::boxed(StageId::Marsh, &log))
                .is_none()
        );
        assert_eq!(build.stage_ids(), [StageId::Pieces, StageId::Marsh]);
    }

    #[test]
    fn the_stream_starts_at_the_seed_of_the_basis_and_the_stages_share_it() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut build = TerrainBuild::empty(0);
        for (id, draws) in [(StageId::CoastSplines, 5), (StageId::Pieces, 3)] {
            build.install(Box::new(Recorder {
                id,
                scope: RasterScope::PerCell,
                draws,
                log: Arc::clone(&log),
            }));
        }
        let b = basis(2, 2);
        run(&mut build, &b);
        assert_eq!(build.seed(), 42);
        assert_eq!(build.rand().calls(), 8);
        // The state after 8 draws from srand(42) is the state of a stream that took 8.
        let mut want = MsvcRand::new(42);
        want.skip(8);
        assert_eq!(build.rand().state(), want.state());
        // A second run starts again at the seed.
        run(&mut build, &b);
        assert_eq!(build.rand().calls(), 8);
        assert_eq!(build.rand().state(), want.state());
    }

    #[test]
    fn a_global_stage_writes_the_field_before_the_cells() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut build = TerrainBuild::empty(1);
        build.install(Box::new(Recorder {
            id: StageId::CoastBlendMeshes,
            scope: RasterScope::Global,
            draws: 0,
            log: Arc::clone(&log),
        }));
        build.install(Recorder::boxed(StageId::Rivers, &log));
        let b = basis(2, 2);
        let (mut field, mut stencil) = run(&mut build, &b);
        // The field is written before any cell is called.
        assert_eq!(field.get_height(10, 10), 777);
        assert_eq!(field.get_id(10, 10), 3);
        let frame = TerrainFrame::new(2, 2, false);
        let lib = TerrainLibrary::synthetic();
        let products = build.products().clone();
        let cx = RasterCx {
            basis: &b,
            library: &lib,
            frame: &frame,
            products: &products,
        };
        log.lock().unwrap().clear();
        build.raster_cell(CellRect::cell(0, 0), &mut field, &mut stencil, &cx);
        // The global stage has no per-cell part; the other one sees what the global one wrote.
        assert_eq!(*log.lock().unwrap(), ["raster Rivers sees 777"]);
    }

    #[test]
    fn a_window_drops_the_writes_outside_its_rect() {
        let mut field = TerrainField::new(2, 2);
        let mut stencil = TerrainStencil::new(128, 128);
        let mut w = FieldWindow::new(&mut field, &mut stencil, CellRect::cell(0, 0));
        assert!(w.set(5, 5, 100, 2));
        assert!(!w.set_height(64, 5, 100));
        assert!(!w.set_id(5, 64, 1));
        // Outside the map is dropped too.
        let mut w2 = FieldWindow::new(&mut field, &mut stencil, CellRect::new(-5, -5, 5, 5));
        assert!(!w2.set_height(-1, 0, 1));
        w2.or_stencil(2, 2, 0b1);
        w2.or_stencil(60, 60, 0b1);
        assert_eq!(field.get_height(5, 5), 100);
        assert_eq!(field.get_height(64, 5), 0);
        assert_eq!(field.get_id(5, 5), 2);
        assert_eq!(stencil.get(1, 1), 0b1);
        assert_eq!(stencil.get(30, 30), 0);
    }

    #[test]
    fn a_failing_stage_stops_the_build() {
        struct Fails;
        impl BuildStage for Fails {
            fn id(&self) -> StageId {
                StageId::Rivers
            }
            fn global(&mut self, _cx: &mut StageCx<'_>) -> Result<(), BuildError> {
                Err(BuildError::Stage {
                    stage: StageId::Rivers,
                    message: "no river".into(),
                })
            }
        }
        let mut build = TerrainBuild::new(1);
        build.install(Box::new(Fails));
        let b = basis(1, 1);
        let frame = TerrainFrame::new(1, 1, false);
        let mut field = TerrainField::new(1, 1);
        let mut stencil = TerrainStencil::new(64, 64);
        let err = build
            .run_global(
                &b,
                &TerrainLibrary::synthetic(),
                &frame,
                &mut field,
                &mut stencil,
            )
            .unwrap_err();
        assert_eq!(err.to_string(), "stage Rivers: no river");
        // The stage before it ran.
        assert_eq!(build.ran(), [StageId::Pieces]);
    }
}
