//! The coast of `world-water` (3.2 WW-D1 to WW-D3): the stages `CoastSplines`,
//! `CoastalRoads`, `CoastStencil`, `BlendSplines` and `CoastBlendMeshes` of the terrain build, in
//! the order of `TerrainSystem::Init`, with its one stream of `rand`.
//!
//! The coast of the original is relief (WW-D2): the lines come from the classes of the hexes, the
//! meshes of the five levels are written into the height and id field after the pieces, and the
//! splines of the blend steer the blit of the pieces. The stages share their intermediate results
//! through one [`CoastShared`] state, made by [`stages`] for the five of them.
//!
//! `rand` draws, in the order of the table: `CoastSplines` the noise of every line
//! ([`noise::draws`] per line, `AddNoiseToSingleCoastline`), `CoastBlendMeshes` two (the debug job
//! of `BlendAllCoastMeshes`); the other stages draw none.

pub mod blend;
pub mod blend_splines;
pub mod bspline;
pub mod grid;
pub mod levels;
pub mod lines;
pub mod mesh;
pub mod noise;
pub mod roads;
pub mod stencil;
pub mod styles;

#[cfg(test)]
mod testkit;

use std::sync::{Arc, Mutex, MutexGuard};

use asset_format::CoastSettings;
use glam::Vec2;

use self::blend::BlendWork;
use self::blend_splines::BlendSplineSet;
use self::grid::CoastGrid;
use self::levels::{FinalLevels, StyleSet, generate_levels};
use self::lines::CoastLines;
use self::roads::CoastalRoad;
use self::styles::CoastStyles;
use crate::world::terrain::build::{
    BuildError, BuildStage, FieldWindow, RasterCx, RasterScope, StageCx, StageId,
};
use crate::world::terrain::field::CellRect;

/// What `CoastSplines` made, for the stages after it.
pub struct CoastLinesDone {
    pub lines: CoastLines,
    pub levels: Vec<FinalLevels>,
    pub styles: StyleSet,
}

/// The results the stages hand to each other and to the rest of `world-water` (the waves and the
/// ice start from the lines).
#[derive(Default)]
pub struct CoastState {
    done: Option<Arc<CoastLinesDone>>,
    roads: Option<(Vec<Vec2>, Vec<CoastalRoad>)>,
    blend: Option<BlendSplineSet>,
}

/// The state shared by the five stages of one build, and the settings they work from.
#[derive(Clone)]
pub struct CoastShared {
    settings: Arc<CoastSettings>,
    state: Arc<Mutex<CoastState>>,
}

impl CoastShared {
    pub fn new(settings: Arc<CoastSettings>) -> Self {
        Self {
            settings,
            state: Arc::default(),
        }
    }

    fn lock(&self, stage: StageId) -> Result<MutexGuard<'_, CoastState>, BuildError> {
        self.state.lock().map_err(|_| BuildError::Stage {
            stage,
            message: "the state of the coast is poisoned".into(),
        })
    }

    fn lines_done(&self, stage: StageId) -> Result<Arc<CoastLinesDone>, BuildError> {
        self.lock(stage)?.done.clone().ok_or(BuildError::Stage {
            stage,
            message: "the stage CoastSplines has not run for this build".into(),
        })
    }

    /// The lines and the levels, once `CoastSplines` has run.
    pub fn lines(&self) -> Option<Arc<CoastLinesDone>> {
        self.state.lock().ok()?.done.clone()
    }

    /// The points of the coastal roads in the texel plane and the roads, once `CoastalRoads` has run.
    pub fn roads(&self) -> Option<(Vec<Vec2>, Vec<CoastalRoad>)> {
        self.state.lock().ok()?.roads.clone()
    }

    /// The splines of the blend with their marks, once `BlendSplines` has run.
    pub fn blend_splines(&self) -> Option<BlendSplineSet> {
        self.state.lock().ok()?.blend.clone()
    }

    pub fn coast_splines(&self) -> Box<dyn BuildStage> {
        Box::new(CoastSplinesStage(self.clone()))
    }

    pub fn coastal_roads(&self) -> Box<dyn BuildStage> {
        Box::new(CoastalRoadsStage(self.clone()))
    }

    pub fn coast_stencil(&self) -> Box<dyn BuildStage> {
        Box::new(CoastStencilStage(self.clone()))
    }

    pub fn blend_splines_stage(&self) -> Box<dyn BuildStage> {
        Box::new(BlendSplinesStage(self.clone()))
    }

    pub fn coast_blend_meshes(&self) -> Box<dyn BuildStage> {
        Box::new(CoastBlendMeshesStage {
            shared: self.clone(),
            work: None,
        })
    }
}

/// The five stages of the coast, sharing one state, in the order of the table.
pub fn stages(settings: Arc<CoastSettings>) -> Vec<Box<dyn BuildStage>> {
    let shared = CoastShared::new(settings);
    vec![
        shared.coast_splines(),
        shared.coastal_roads(),
        shared.coast_stencil(),
        shared.blend_splines_stage(),
        shared.coast_blend_meshes(),
    ]
}

/// `CoastSplines` with a state of its own; the stages that read its result need [`stages`] or a
/// [`CoastShared`].
pub fn coast_splines(settings: Arc<CoastSettings>) -> Box<dyn BuildStage> {
    CoastShared::new(settings).coast_splines()
}

/// `CoastalRoads` with a state of its own (it fails unless `CoastSplines` shares the state).
pub fn coastal_roads(settings: Arc<CoastSettings>) -> Box<dyn BuildStage> {
    CoastShared::new(settings).coastal_roads()
}

/// `CoastStencil` with a state of its own (it fails unless `CoastSplines` shares the state).
pub fn coast_stencil(settings: Arc<CoastSettings>) -> Box<dyn BuildStage> {
    CoastShared::new(settings).coast_stencil()
}

/// `BlendSplines` with a state of its own (it fails unless `CoastSplines` shares the state).
pub fn blend_splines(settings: Arc<CoastSettings>) -> Box<dyn BuildStage> {
    CoastShared::new(settings).blend_splines_stage()
}

/// `CoastBlendMeshes` with a state of its own (it fails unless `CoastSplines` shares the state).
pub fn coast_blend_meshes(settings: Arc<CoastSettings>) -> Box<dyn BuildStage> {
    CoastShared::new(settings).coast_blend_meshes()
}

/// Step 4: the coast grid, the lines with their noise, the five levels of every line.
// re: TerrainSystem::Init (TerrainSystem.cpp:710-725): CoastGrid::Init, LoadCoastDefinitions,
// cvCoastSplineSystem::Startup, GenerateAllLevelSplines, AddAllCoastlinesToDatabase 0x088ed400 (the
// segments of the shore lines go to the products, [`lines::shore_segments`]; the surf reads them from
// its `GameSplineDatabase`, re-b16 W3)
struct CoastSplinesStage(CoastShared);

impl BuildStage for CoastSplinesStage {
    fn id(&self) -> StageId {
        StageId::CoastSplines
    }

    fn global(&mut self, cx: &mut StageCx<'_>) -> Result<(), BuildError> {
        let styles = CoastStyles::from_settings(&self.0.settings);
        let mut grid = CoastGrid::new(cx.basis, cx.frame);
        let lines = CoastLines::extract(&mut grid, cx.rand);
        cx.products.coast_segments = lines::shore_segments(&lines, cx.frame);
        let set = StyleSet::new(&styles, cx.library.defs());
        let levels = lines
            .splines
            .iter()
            .map(|s| generate_levels(s, &set))
            .collect();
        let mut state = self.0.lock(StageId::CoastSplines)?;
        *state = CoastState {
            done: Some(Arc::new(CoastLinesDone {
                lines,
                levels,
                styles: set,
            })),
            roads: None,
            blend: None,
        };
        Ok(())
    }

    fn raster_scope(&self) -> RasterScope {
        RasterScope::Global
    }
}

/// Step 5: the polylines of the coastal roads.
// re: TerrainSystem::Init (TerrainSystem.cpp:728-744): GenerateAllCoastalRoads; the road layout
// (`AddLargeRoad`, `Fun_0868a43e`) is `world-objects`'
struct CoastalRoadsStage(CoastShared);

impl BuildStage for CoastalRoadsStage {
    fn id(&self) -> StageId {
        StageId::CoastalRoads
    }

    fn global(&mut self, cx: &mut StageCx<'_>) -> Result<(), BuildError> {
        let done = self.0.lines_done(StageId::CoastalRoads)?;
        let (points, roads) = roads::generate(&done.lines.splines);
        cx.products.coastal_roads = roads
            .iter()
            .map(|r| {
                points[r.first_point..r.first_point + r.num_points]
                    .iter()
                    .map(|&p| roads::to_layout(p, cx.frame))
                    .collect()
            })
            .collect();
        self.0.lock(StageId::CoastalRoads)?.roads = Some((points, roads));
        Ok(())
    }

    fn raster_scope(&self) -> RasterScope {
        RasterScope::Global
    }
}

/// Step 6: the stencil surfaces of the lines.
// re: TerrainSystem::Init (TerrainSystem.cpp:747-757): GenerateAllCoastStencil, AddStencil
struct CoastStencilStage(CoastShared);

impl BuildStage for CoastStencilStage {
    fn id(&self) -> StageId {
        StageId::CoastStencil
    }

    fn global(&mut self, cx: &mut StageCx<'_>) -> Result<(), BuildError> {
        let done = self.0.lines_done(StageId::CoastStencil)?;
        for surface in stencil::generate_all(&done.levels) {
            surface.add_to(cx.stencil);
        }
        Ok(())
    }

    fn raster_scope(&self) -> RasterScope {
        RasterScope::Global
    }
}

/// Step 7: the splines of the blend for the blit of the pieces.
// re: TerrainSystem::Init (TerrainSystem.cpp:758-921)
struct BlendSplinesStage(CoastShared);

impl BuildStage for BlendSplinesStage {
    fn id(&self) -> StageId {
        StageId::BlendSplines
    }

    fn global(&mut self, cx: &mut StageCx<'_>) -> Result<(), BuildError> {
        let done = self.0.lines_done(StageId::BlendSplines)?;
        let set = blend_splines::compute(&done.levels);
        cx.products.blend_splines = set.splines();
        self.0.lock(StageId::BlendSplines)?.blend = Some(set);
        Ok(())
    }

    fn raster_scope(&self) -> RasterScope {
        RasterScope::Global
    }
}

/// Step 10: the meshes of the coast, blended into the field after the pieces, cell by cell.
// re: TerrainSystem::Init (TerrainSystem.cpp:939-941): GenerateAllCoastBlendMeshes, BlendAllCoastMeshes
struct CoastBlendMeshesStage {
    shared: CoastShared,
    work: Option<Arc<BlendWork>>,
}

impl BuildStage for CoastBlendMeshesStage {
    fn id(&self) -> StageId {
        StageId::CoastBlendMeshes
    }

    fn global(&mut self, cx: &mut StageCx<'_>) -> Result<(), BuildError> {
        let done = self.shared.lines_done(StageId::CoastBlendMeshes)?;
        let meshes = done
            .levels
            .iter()
            .map(|l| mesh::generate_final_mesh(l, &done.styles))
            .collect();
        self.work = Some(Arc::new(BlendWork::new(
            meshes,
            cx.frame,
            cx.library.defs(),
            cx.rand,
        )));
        Ok(())
    }

    /// The field is written cell by cell, after the pieces of the cell: the texels are
    /// independent, so a cell gets what the whole map would.
    fn raster(&self, cell: CellRect, window: &mut FieldWindow<'_>, _cx: &RasterCx<'_>) {
        if let Some(work) = &self.work {
            work.apply_cell(cell, window);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::styles::synthetic_settings;
    use super::testkit::{basis_with, frame_of, island, island_at};
    use super::*;
    use crate::world::terrain::basis::TerrainBasis;
    use crate::world::terrain::build::TerrainBuild;
    use crate::world::terrain::field::TerrainField;
    use crate::world::terrain::library::TerrainLibrary;
    use crate::world::terrain::sampler::SEA_LEVEL_RAW;
    use crate::world::terrain::stencil::TerrainStencil;

    /// The settings of the tests: the waterline a half unit above 13000 so that the truncation of
    /// the store gives exactly 13000.
    fn settings() -> Arc<CoastSettings> {
        let mut s = synthetic_settings(false);
        for style in &mut s.styles {
            style.waterline.height = (f32::from(SEA_LEVEL_RAW) + 0.5) / 65535.0;
        }
        Arc::new(s)
    }

    struct Run {
        build: TerrainBuild,
        shared: CoastShared,
        field: TerrainField,
        stencil: TerrainStencil,
        frame: crate::world::terrain::sampler::TerrainFrame,
    }

    fn run(basis: &TerrainBasis) -> Run {
        let shared = CoastShared::new(settings());
        // The build of the terrain with the stage `Pieces` it has by itself: the coast joins it.
        let mut build = TerrainBuild::new(basis.seed);
        build.install(shared.coast_splines());
        build.install(shared.coastal_roads());
        build.install(shared.coast_stencil());
        build.install(shared.blend_splines_stage());
        build.install(shared.coast_blend_meshes());
        let frame = frame_of(basis);
        let (w, h) = frame.texel_size();
        let mut field = TerrainField::with_texels(w, h);
        let mut stencil = TerrainStencil::new(w, h);
        build
            .run_global(
                basis,
                &TerrainLibrary::synthetic(),
                &frame,
                &mut field,
                &mut stencil,
            )
            .expect("the build runs");
        Run {
            build,
            shared,
            field,
            stencil,
            frame,
        }
    }

    fn rasterize(r: &mut Run, basis: &TerrainBasis) {
        let lib = TerrainLibrary::synthetic();
        let products = r.build.products().clone();
        let cx = RasterCx {
            basis,
            library: &lib,
            frame: &r.frame,
            products: &products,
        };
        let (w, h) = r.frame.texel_size();
        for cy in 0..(h / 64) as i32 {
            for cx_ in 0..(w / 64) as i32 {
                r.build
                    .raster_cell(CellRect::cell(cx_, cy), &mut r.field, &mut r.stencil, &cx);
            }
        }
    }

    #[test]
    fn the_stages_are_the_five_rows_of_the_table_in_order() {
        let ids: Vec<StageId> = stages(settings()).iter().map(|s| s.id()).collect();
        assert_eq!(
            ids,
            [
                StageId::CoastSplines,
                StageId::CoastalRoads,
                StageId::CoastStencil,
                StageId::BlendSplines,
                StageId::CoastBlendMeshes
            ]
        );
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn a_stage_that_needs_the_lines_fails_without_them() {
        let b = island(12, 12, false, 1);
        let mut build = TerrainBuild::empty(1);
        build.install(coastal_roads(settings()));
        let frame = frame_of(&b);
        let (w, h) = frame.texel_size();
        let err = build
            .run_global(
                &b,
                &TerrainLibrary::synthetic(),
                &frame,
                &mut TerrainField::with_texels(w, h),
                &mut TerrainStencil::new(w, h),
            )
            .unwrap_err();
        assert!(
            err.to_string().contains("CoastSplines has not run"),
            "{err}"
        );
    }

    /// The cells built side by side from one field (B3 render-mt, `raster_cell_detached`) and put
    /// back in order leave the bytes, the stencil and the revisions of the cells built one by one:
    /// the per-cell parts of the pieces and the coast read and write only their own cell. Also for a
    /// second build over a built field (an invalidated cell reads what it wrote before).
    #[test]
    fn cells_built_apart_and_put_back_are_the_cells_built_in_place() {
        for wrap in [false, true] {
            let b = island(12, 10, wrap, 2);
            let mut seq = run(&b);
            let mut par = run(&b);
            let lib = TerrainLibrary::synthetic();
            let products = seq.build.products().clone();
            let cx = RasterCx {
                basis: &b,
                library: &lib,
                frame: &seq.frame,
                products: &products,
            };
            let (w, h) = seq.frame.texel_size();
            let cells: Vec<CellRect> = (0..(h / 64) as i32)
                .flat_map(|cy| (0..(w / 64) as i32).map(move |cx_| CellRect::cell(cx_, cy)))
                .collect();
            for pass in 0..2 {
                for &c in &cells {
                    seq.build
                        .raster_cell(c, &mut seq.field, &mut seq.stencil, &cx);
                }
                // All cells of the pass from the same field, then put back in order.
                let built: Vec<_> = cells
                    .iter()
                    .map(|&c| par.build.raster_cell_detached(c, &par.field, &cx))
                    .collect();
                for cell in built {
                    TerrainBuild::apply_cell(cell, &mut par.field, &mut par.stencil);
                }
                assert!(
                    seq.field.same_content(&par.field),
                    "wrap {wrap} pass {pass}"
                );
                assert_eq!(seq.field.digest(), par.field.digest());
                assert_eq!(seq.field.revision(), par.field.revision());
                assert_eq!(seq.field.blocks_allocated(), par.field.blocks_allocated());
                assert_eq!(seq.stencil, par.stencil, "wrap {wrap} pass {pass}");
            }
        }
    }

    /// The draws the stage `Pieces` of the build takes on `basis` (the matcher of the original).
    fn matcher_draws(basis: &TerrainBasis) -> u64 {
        use crate::world::terrain::build::{bridge, matcher::HexPatternMatcher};
        let mut m = HexPatternMatcher::new();
        for p in bridge::library_pieces(&TerrainLibrary::synthetic()) {
            m.add_piece(p);
        }
        let policy = bridge::pattern_grid(basis).policy;
        m.draws(policy.width, policy.height)
    }

    /// The stream of `rand` of the whole build, in the order of the table: the noise of every line
    /// (`CoastSplines`), then the draws of the matcher (`Pieces`), then the two draws of the debug
    /// job (`CoastBlendMeshes`). The coast joins the build `TerrainBuild::new` makes.
    #[test]
    fn the_stages_draw_exactly_the_calls_rw_10_states() {
        let b = island(20, 20, false, 2);
        let r = run(&b);
        assert_eq!(
            r.build.ran(),
            [
                StageId::CoastSplines,
                StageId::CoastalRoads,
                StageId::CoastStencil,
                StageId::BlendSplines,
                StageId::Pieces,
                StageId::CoastBlendMeshes
            ]
        );
        let done = r.shared.lines().unwrap();
        let mut sizes: Vec<usize> = done.lines.splines.iter().map(|s| s.points.len()).collect();
        sizes.sort_unstable();
        // The shore of land of radius 2 has 30 sides and the shelf 42: two points each plus the
        // two of the first side again; the shelf of the border of the 20 x 20 map (the padding of
        // the grid is coast) has 158 sides.
        assert_eq!(sizes, [62, 86, 318]);
        // By the formula (n_r - 2) * (2^(k+1) - 2) + 2k: 62 points -> 4 levels, 6 coarse points:
        // 4 * 30 + 8 = 128; 86 points -> 4 levels, 8 coarse points: 6 * 30 + 8 = 188.
        assert_eq!(noise::draws(62), 128);
        assert_eq!(noise::draws(86), 188);
        let coast: u64 = sizes.iter().map(|&n| noise::draws(n)).sum::<u64>() + 2;
        assert_eq!(coast, 128 + 188 + noise::draws(318) + 2);
        let pieces = matcher_draws(&b);
        assert!(pieces > 0, "the matcher draws on a map with pieces");
        assert_eq!(r.build.rand().calls(), coast + pieces);
        // The coast draws the same with or without the matcher: the stream of the coast alone.
        let mut alone = TerrainBuild::empty(b.seed);
        for stage in stages(settings()) {
            alone.install(stage);
        }
        let frame = frame_of(&b);
        let (w, h) = frame.texel_size();
        alone
            .run_global(
                &b,
                &TerrainLibrary::synthetic(),
                &frame,
                &mut TerrainField::with_texels(w, h),
                &mut TerrainStencil::new(w, h),
            )
            .expect("the build runs");
        assert_eq!(alone.rand().calls(), coast);
        // An ocean map has only the shelf of its border (the padding of the grid is coast): its
        // noise, the debug job and the draws of the matcher.
        let ocean = basis_with(8, 8, false, |_, _| 2);
        let ro = run(&ocean);
        let border: Vec<usize> = ro
            .shared
            .lines()
            .unwrap()
            .lines
            .splines
            .iter()
            .map(|s| s.points.len())
            .collect();
        assert_eq!(border.len(), 1);
        assert_eq!(
            ro.build.rand().calls(),
            noise::draws(border[0]) + 2 + matcher_draws(&ocean)
        );
    }

    #[test]
    fn the_island_gets_closed_lines_levels_that_do_not_cross_and_a_waterline_at_13000() {
        let b = island(20, 20, false, 2);
        let mut r = run(&b);
        let done = r.shared.lines().unwrap();
        // Closed lines: the shore, the shelf and the shelf along the border of the map.
        assert_eq!(done.lines.splines.len(), 3);
        assert!(done.lines.splines.iter().all(|s| s.closed));
        // Levels in the order of their offsets at every point.
        for l in &done.levels {
            let c = l.level(levels::COASTLINE_LEVEL);
            for i in 2..l.level_len() - 2 {
                let g = c[i + 1].pos - c[i - 1].pos;
                let normal = Vec2::new(g.y, -g.x).normalize();
                let d: Vec<f32> = (0..5)
                    .map(|k| (l.level(k)[i].pos - c[i].pos).dot(normal))
                    .collect();
                assert!(d.windows(2).all(|w| w[0] < w[1]), "point {i}: {d:?}");
            }
        }
        // The waterline of the mesh is at 13000, and the field carries it after the blend: the
        // texel at an integer position holds the height of the mesh there, so the bilinear
        // height on the line is 13000 up to the kink of the slope.
        rasterize(&mut r, &b);
        let shore = done
            .levels
            .iter()
            .zip(&done.lines.splines)
            .find(|(_, s)| s.points.len() < 70)
            .unwrap()
            .0;
        let sample = |p: Vec2| -> f32 {
            let (x0, y0) = (p.x.floor() as i32, p.y.floor() as i32);
            let (fx, fy) = (p.x - x0 as f32, p.y - y0 as f32);
            let h = |dx, dy| f32::from(r.field.get_height(x0 + dx, y0 + dy));
            (h(0, 0) * (1.0 - fx) + h(1, 0) * fx) * (1.0 - fy)
                + (h(0, 1) * (1.0 - fx) + h(1, 1) * fx) * fy
        };
        // Across the line, from the land to the sea, the field crosses 13000 on the line: the
        // slope changes there (the high beach is above the water, the low beach below), so the
        // crossing is the place to look at, to a quarter of a texel.
        let level2 = shore.level(levels::COASTLINE_LEVEL);
        let mut checked = 0;
        for i in (5..level2.len() - 5).step_by(13) {
            let g = level2[i + 1].pos - level2[i - 1].pos;
            let normal = Vec2::new(g.y, -g.x).normalize();
            let p = level2[i].pos;
            let crossing = (-8..=8)
                .map(|k| k as f32 * 0.25)
                .find(|&s| sample(p + normal * s) <= f32::from(SEA_LEVEL_RAW))
                .expect("the field falls below the water on the sea side");
            assert!(
                crossing.abs() <= 1.0,
                "crossing at {crossing} of the waterline, point {i}"
            );
            checked += 1;
        }
        assert!(checked > 5);
        // Products: roads, blend splines and the stencil are there.
        let products = r.build.products();
        assert!(!products.coastal_roads.is_empty());
        assert_eq!(products.blend_splines.len(), 3 + 6);
        assert!(r.stencil.pages_allocated() > 0);
    }

    /// re-b16 C1-1: on the padded grid (all coast round the map) the shore of land at the border of
    /// a map without wrap goes round through the padding: every line is closed, a beach at the border.
    #[test]
    fn a_map_without_wrap_closes_its_lines_through_the_padding_and_a_wrapping_map_across_the_seam()
    {
        // An island on the left edge of the map.
        let flat = island_at(12, 12, false, (0, 6), 2);
        let wrapped = island_at(12, 12, true, (0, 6), 2);
        let r = run(&flat);
        let lines = r.shared.lines().unwrap();
        assert!(!lines.lines.splines.is_empty());
        assert!(lines.lines.splines.iter().all(|s| s.closed));
        // The lines stay within the padding: two hexes beyond the border and the noise at most.
        let width = 12.0 * 64.0 + 200.0;
        for s in &lines.lines.splines {
            for p in &s.points {
                assert!(p.pos.x > -200.0 && p.pos.x < width, "{}", p.pos);
            }
        }
        assert!(!r.build.products().blend_splines.is_empty());
        // With wrap the island is whole across the seam: two closed lines, part of them on the
        // far side of the seam in unwrapped texels (and the shelves along the top and the bottom).
        let rw = run(&wrapped);
        let linesw = testkit::inner_lines(rw.shared.lines().unwrap().lines.clone(), &wrapped);
        let linesw = CoastLinesDone {
            lines: linesw,
            levels: Vec::new(),
            styles: rw.shared.lines().unwrap().styles.clone(),
        };
        assert_eq!(linesw.lines.splines.len(), 2);
        assert!(linesw.lines.splines.iter().all(|s| s.closed));
        assert!(
            linesw
                .lines
                .splines
                .iter()
                .any(|s| s.points.iter().any(|p| p.pos.x < 0.0))
        );
    }

    #[test]
    fn the_blend_splines_are_in_the_products_in_the_order_of_the_original() {
        let b = island(20, 20, false, 2);
        let r = run(&b);
        let set = r.shared.blend_splines().unwrap();
        let products = r.build.products();
        assert_eq!(products.blend_splines, set.splines());
        // Every coast first (range A), then the two shifts of every coast (range B).
        use crate::world::terrain::build::SplineRange;
        let kinds: Vec<SplineRange> = products.blend_splines.iter().map(|s| s.range).collect();
        assert_eq!(
            kinds,
            [
                SplineRange::A,
                SplineRange::A,
                SplineRange::A,
                SplineRange::B,
                SplineRange::B,
                SplineRange::B,
                SplineRange::B,
                SplineRange::B,
                SplineRange::B
            ]
        );
    }
}
