//! The stage `Rivers` of the terrain build: the global part of
//! `cvRiverSimSystem::StartupGame` after the pieces and the meshes of the coast, with the stream of
//! `rand` of `TerrainSystem::Init`. It puts the stencil of the rivers (bits 0x09) into the stencil of
//! the terrain, gives the edges to `products().river_edges` and keeps the simulation for the draw.
//!
//! The cut of the bed into the field (`BlendIntoTerrain`) and the decals of the banks
//! (`BuildFloodplains`) are the next packet of WW-4: this stage writes nothing into the field.

use std::sync::{Arc, Mutex};

use glam::Vec2;

use super::hex::{RiverGrid, SIDE_DIR};
use super::{RiverSim, blend, floodplains, sim, stencil};
use crate::hexgrid::{cell_center, nearest_cell};
use crate::terrain::mesh::LAND_HEIGHT;
use crate::terrain::relief_height;
use crate::world::terrain::basis::TerrainBasis;
use crate::world::terrain::build::rand::MsvcRand;
use crate::world::terrain::build::{
    BuildError, BuildStage, FieldWindow, RasterCx, RasterScope, RiverEdge, StageCx, StageId,
};
use crate::world::terrain::field::CellRect;
use crate::world::terrain::sampler::{TerrainFrame, raw_from_height};

/// What the stage made, for the draw (shared with the subsystem `River`).
#[derive(Clone, Default)]
pub struct RiverShared(
    Arc<Mutex<Option<Arc<RiverSim>>>>,
    Arc<Mutex<Option<[usize; 10]>>>,
);

impl RiverShared {
    /// The simulation of the last build, `None` before the stage ran.
    pub fn sim(&self) -> Option<Arc<RiverSim>> {
        self.0.lock().ok()?.clone()
    }

    /// The number of types of every category of decal (`DecalTypes`), which decides whether
    /// `CreateDecal` draws from `rand`. Set it before the basis.
    pub fn set_decal_type_counts(&self, counts: [usize; 10]) {
        if let Ok(mut c) = self.1.lock() {
            *c = Some(counts);
        }
    }

    /// The stage; it writes its result here.
    pub fn stage(&self) -> Box<dyn BuildStage> {
        Box::new(RiversStage(self.clone(), None))
    }
}

struct RiversStage(RiverShared, Option<blend::RiverBlend>);

impl BuildStage for RiversStage {
    fn id(&self) -> StageId {
        StageId::Rivers
    }

    // re: TerrainSystem::Init (TerrainSystem.cpp:943-952): `StartupGame`, then `BuildFloodplains`
    fn global(&mut self, cx: &mut StageCx<'_>) -> Result<(), BuildError> {
        let grid = RiverGrid::from_basis(cx.basis);
        let frame = *cx.frame;
        let basis = cx.basis;
        let shape = frame.grid_shape();
        let scale_z = frame.terrain_scale().z;
        let (tw, th) = frame.texel_size();
        // The height of the field before the rivers (`StartupGame` runs after the pieces and the coast
        // meshes): the field the stages before this one make, cell by cell as it is asked. The sea floor
        // at a mouth is the one of the coast, not the level of the water.
        let prior = cx.prior;
        let height_at = |u: i32, v: i32| -> u16 {
            if let Some(prior) = prior {
                return prior.height(u, v);
            }
            // Without the stages before (a build with no pieces): the relief of B1, flat water.
            let u = u.rem_euclid(tw as i32);
            let v = v.clamp(0, th as i32 - 1);
            let world = frame.tex_to_world(Vec2::new(u as f32 + 0.5, v as f32 + 0.5));
            let hex = nearest_cell(world);
            let h = match shape
                .normalize(hex.0, hex.1)
                .and_then(|(x, y)| basis.plot(x, y))
            {
                Some(p) => relief_height(p.relief, world - cell_center(hex.0, hex.1)),
                None => LAND_HEIGHT,
            };
            raw_from_height(h, scale_z)
        };
        let sim = sim::build(&grid, &frame, cx.rand, &height_at);
        stencil::add_to(&sim, cx.stencil);
        cx.products.river_edges = sim
            .edges
            .iter()
            .map(|river| {
                river
                    .iter()
                    .map(|e| {
                        let e = e.owned();
                        RiverEdge {
                            hex: e.cell(),
                            dir: SIDE_DIR[e.side()] as u8,
                        }
                    })
                    .collect()
            })
            .collect();
        // re: cvRiverSimSystem::BlendIntoTerrain (cuts the bed cell by cell in `raster`)
        let ids = blend::RiverIds::from_defs(cx.library.defs());
        self.1 = Some(blend::RiverBlend::new(&sim, &ids).with_map_size(tw, th));
        // re: cvFloodplainsSystem::BuildFloodplains, after StartupGame, with the same stream
        let counts = self.0.1.lock().ok().and_then(|c| *c).unwrap_or([2; 10]);
        cx.products.bank_decals =
            floodplains::build_with_counts(&grid, &sim, cx.basis, cx.frame, cx.rand, &counts);
        if let Ok(mut slot) = self.0.0.lock() {
            *slot = Some(Arc::new(sim));
        }
        Ok(())
    }

    fn raster_scope(&self) -> RasterScope {
        RasterScope::PerCell
    }

    fn wants_prior_field(&self) -> bool {
        true
    }

    /// The heights `StartupGame` reads are the ones under the control points of the rivers, which do
    /// not depend on the heights: a run with the stream as it will be and a height of 0 lists the
    /// texels in the order they are asked for.
    fn prior_texels(
        &self,
        basis: &TerrainBasis,
        frame: &TerrainFrame,
        rand: &MsvcRand,
    ) -> Vec<(i32, i32)> {
        let grid = RiverGrid::from_basis(basis);
        let mut rand = rand.clone();
        let asked = std::cell::RefCell::new(Vec::new());
        let height_at = |u: i32, v: i32| -> u16 {
            asked.borrow_mut().push((u, v));
            0
        };
        let _ = sim::build(&grid, frame, &mut rand, &height_at);
        asked.into_inner()
    }

    fn raster(&self, cell: CellRect, window: &mut FieldWindow<'_>, _cx: &RasterCx<'_>) {
        if let Some(blend) = &self.1 {
            blend.raster(cell, window);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hexgrid::Dir;
    use crate::terrain::{Relief, TerrainKind};
    use crate::world::terrain::basis::{PlotBasis, TerrainBasis};
    use crate::world::terrain::build::TerrainBuild;
    use crate::world::terrain::field::TerrainField;
    use crate::world::terrain::library::TerrainLibrary;
    use crate::world::terrain::sampler::TerrainFrame;
    use crate::world::terrain::stencil::TerrainStencil;

    fn basis() -> TerrainBasis {
        let mut b = TerrainBasis::uniform(
            10,
            10,
            false,
            3,
            PlotBasis {
                relief: Relief::Flat,
                terrain: TerrainKind::Grass,
                river_flow: PlotBasis::NO_FLOW,
                ..PlotBasis::default()
            },
        );
        // A river down the east sides of a column, into the sea at the south.
        for y in 2..7 {
            let p = &mut b.plots[y * 10 + 4];
            p.rivers |= Dir::East.edge_bit();
            p.river_flow[0] = 3;
        }
        for x in 0..10 {
            b.plots[x].relief = Relief::Water;
        }
        b
    }

    #[test]
    fn the_stage_gives_edges_and_a_stencil() {
        let b = basis();
        let shared = RiverShared::default();
        let mut build = TerrainBuild::empty(b.seed);
        build.install(shared.stage());
        let frame = TerrainFrame::new(10, 10, false);
        let (w, h) = frame.texel_size();
        let mut field = TerrainField::with_texels(w, h);
        let mut stencil = TerrainStencil::new(w, h);
        let lib = TerrainLibrary::default();
        build
            .run_global(&b, &lib, &frame, &mut field, &mut stencil)
            .unwrap();
        // Five edges of the grid, in as many pieces as the rivers join up.
        let edges = &build.products().river_edges;
        assert!(!edges.is_empty());
        assert_eq!(edges.iter().map(Vec::len).sum::<usize>(), 5);
        assert!(shared.sim().is_some());
        assert!(stencil.pages_allocated() > 0);
        let again = {
            let mut s2 = TerrainStencil::new(w, h);
            let mut f2 = TerrainField::with_texels(w, h);
            let mut b2 = TerrainBuild::empty(b.seed);
            b2.install(RiverShared::default().stage());
            b2.run_global(&b, &lib, &frame, &mut f2, &mut s2).unwrap();
            b2.rand().state()
        };
        assert_eq!(again, build.rand().state());
    }

    /// The build in pieces of no time (a cell of the prior field, a stage, a step) gives what the
    /// build at once gives: the stream, the edges, the heights of the rivers.
    #[test]
    fn the_build_in_pieces_gives_the_build_at_once() {
        let b = basis();
        let frame = TerrainFrame::new(10, 10, false);
        let (w, h) = frame.texel_size();
        let lib = TerrainLibrary::default();
        let once = {
            let shared = RiverShared::default();
            let mut build = TerrainBuild::new(b.seed);
            build.install(shared.stage());
            let mut field = TerrainField::with_texels(w, h);
            let mut stencil = TerrainStencil::new(w, h);
            build
                .run_global(&b, &lib, &frame, &mut field, &mut stencil)
                .unwrap();
            let heights = shared.sim().unwrap().heights.clone();
            (
                build.rand().state(),
                build.products().river_edges.clone(),
                heights,
            )
        };
        let shared = RiverShared::default();
        let mut build = TerrainBuild::new(b.seed);
        build.install(shared.stage());
        let mut field = TerrainField::with_texels(w, h);
        let mut stencil = TerrainStencil::new(w, h);
        let mut run = build.begin_global(&b);
        let mut steps = 0;
        while !build
            .step_global(&mut run, &b, &lib, &frame, &mut field, &mut stencil, 0.0)
            .unwrap()
        {
            steps += 1;
            assert!(steps < 10_000, "the build does not end");
        }
        assert!(steps > 1, "a budget of nothing is more than one step");
        let heights = shared.sim().unwrap().heights.clone();
        assert_eq!(build.rand().state(), once.0);
        assert_eq!(build.products().river_edges, once.1);
        assert_eq!(heights, once.2);
    }
}
