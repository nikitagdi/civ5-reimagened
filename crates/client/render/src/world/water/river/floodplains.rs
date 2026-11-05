//! `cvFloodplainsSystem` (world-water WW-4, RW-12): the decals of the flood plains and of the banks
//! of the rivers. A bank flag per hex (`m_auBankFlags`) is set for the hexes with a flood plain
//! (`OnFloodplainAdded`) and, for the second pass, for both hexes of every side of a river
//! (`AddAllRiverBanks`). Each pass walks the rivers from their source, cuts them into runs of sides
//! that have a flagged hex on one bank (`BuildTileLists`, `AddBankAlongRiver`, `CheckForBank`) and
//! makes one strip of quads along every run (`BuildBankVis`, [`super::banks`]), in up to three
//! decals: the start, the middle and the end of the strip.
//!
//! The original creates the decals in the layer while it builds them. Here the build only makes
//! them ([`BankDecal`]); the terrain creates them when the basis is set. A decal of the original
//! draws `rand()` once to choose its type, when its category has two types or more
//! (`TerrainDecalLayer::CreateDecal` 0x0867b85e); the draw is made here, in the order of the
//! original, and kept in [`BankDecal::type_roll`].
//!
//! Differences from the original, all about the grid of flags: it is 200 x 200 and does not wrap; ours is
//! the map (cells put inside the map, so the hexes at the seam of a wrapping map count), and the
//! visited sides are kept under their wrapped name (`RiverGrid::both_names`).

// The constants and the comparisons are those of the original (NaN included), kept as they are.
#![allow(clippy::too_many_arguments, clippy::type_complexity)]
// re: cvFloodplainsSystem.cpp (BuildFloodplains 0x08661cea, BuildTileLists 0x08661e2c, BuildBankVis
// 0x0866217c, AddAllRiverBanks 0x0866343c, OnFloodplainAdded 0x08663558, AddRiverBank 0x0866367a,
// AddBankAlongRiver 0x08663704, CheckForBank 0x08663b40)

use std::collections::{BTreeSet, HashSet};

use glam::Vec2;

use super::RiverSim;
use super::banks::{
    BankSide, BankSim, add_endcaps, add_endcaps_scalar, build_strip, extrude_control_points,
};
use super::hex::{HexEdge, RiverGrid};
use crate::hexgrid::GridShape;
use crate::world::decal::{CATEGORY_NAMES, DecalCategory};
use crate::world::terrain::basis::{BasisFeature, TerrainBasis};
use crate::world::terrain::build::rand::MsvcRand;
use crate::world::terrain::decals::DecalVertex;
use crate::world::terrain::sampler::TerrainFrame;

/// The bits of `m_auBankFlags`, and the kind of bank a pass builds.
// re: cvFloodplainsSystem.h `RIVERBANK_FLAG`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BankKind {
    River,
    Floodplain,
}

/// Decal categories of the start, middle and end of a strip, as names in `decal_filenames.xml`
/// (the original indexes 5, 4, 6 and 8, 7, 9 into its list of categories).
// re: BuildBankVis 0x0866217c (`auiFloodplainCategories`, `auiRiverBankCategories`);
// TerrainDecalLayer.cpp:32-43 (`kCategoryNames`)
const FLOODPLAIN_PARTS: [&str; 3] = ["Floodplains_EndCap1", "Floodplains", "Floodplains_EndCap2"];
const RIVERBANK_PARTS: [&str; 3] = ["Riverbank_EndCap1", "Riverbank", "Riverbank_EndCap2"];

/// A decal the banks make: the mesh in our world and the draw of `rand()` that picks its type.
#[derive(Clone, Debug, PartialEq)]
pub struct BankDecal {
    pub category: DecalCategory,
    pub verts: Vec<DecalVertex>,
    pub tris: Vec<[u16; 3]>,
    /// The raw `rand()` the original draws for this decal (0 if the category had fewer than two
    /// types, where it draws nothing); the type is `roll % number of types`.
    pub type_roll: u32,
}

/// Number of types per category that makes the draw happen.
const MIN_TYPES_FOR_DRAW: usize = 2;

struct Flags {
    shape: GridShape,
    floodplain: BTreeSet<(i32, i32)>,
    river: BTreeSet<(i32, i32)>,
}

impl Flags {
    fn new(shape: GridShape) -> Self {
        Self {
            shape,
            floodplain: BTreeSet::new(),
            river: BTreeSet::new(),
        }
    }

    /// The cell `(col, row)` of a cube hex, put inside the map.
    fn cell(&self, x: i32, y: i32) -> Option<(i32, i32)> {
        self.shape.normalize(x + (y >> 1), y)
    }

    fn set_of(&self, kind: BankKind) -> &BTreeSet<(i32, i32)> {
        match kind {
            BankKind::River => &self.river,
            BankKind::Floodplain => &self.floodplain,
        }
    }

    /// Sets the flag of the hex.
    // re: OnFloodplainAdded 0x08663558, AddRiverBank 0x0866367a
    fn set(&mut self, kind: BankKind, x: i32, y: i32) {
        if let Some(c) = self.cell(x, y) {
            match kind {
                BankKind::River => self.river.insert(c),
                BankKind::Floodplain => self.floodplain.insert(c),
            };
        }
    }

    // re: TestBankFlag 0x08663c4e
    fn test(&self, kind: BankKind, x: i32, y: i32) -> bool {
        self.cell(x, y)
            .is_some_and(|c| self.set_of(kind).contains(&c))
    }
}

/// The side is in the set under either of its two names.
fn is_visited(grid: &RiverGrid, visited: &HashSet<HexEdge>, e: HexEdge) -> bool {
    grid.both_names(e)
        .into_iter()
        .flatten()
        .any(|k| visited.contains(&k))
}

/// Walks up the river from `start` to its source, stopping below a stretch that was visited.
// re: cvRiverSimSystem::TraverseToRiverSource 0x087f5194 (the same walk as `sim.rs`, whose copy is private)
fn traverse_to_river_source(
    grid: &RiverGrid,
    start: HexEdge,
    visited: &HashSet<HexEdge>,
) -> Option<HexEdge> {
    if !grid.is_river(start) || is_visited(grid, visited, start) {
        return None;
    }
    // Not in the original: a ring of river edges would walk for ever.
    let mut walked: HashSet<Option<HexEdge>> = HashSet::new();
    walked.insert(grid.key(start));
    let mut edge = start;
    loop {
        let Some(prev) = grid.prev_edge(edge) else {
            return Some(edge);
        };
        if is_visited(grid, visited, prev) || !walked.insert(grid.key(prev)) {
            return Some(edge);
        }
        edge = prev;
    }
}

/// The hex of the side on `side` of the river has the flag.
// re: cvFloodplainsSystem::CheckForBank 0x08663b40
fn check_for_bank(flags: &Flags, e: HexEdge, side: BankSide, kind: BankKind) -> bool {
    let across = match side {
        BankSide::Right => e.code % 12 >= 6,
        BankSide::Left => e.code % 12 <= 5,
    };
    let h = if across { e.twin() } else { e };
    flags.test(kind, h.x, h.y)
}

/// Walks the river downstream from `start`; consecutive sides with a flagged hex on one bank form
/// one sim per bank.
// re: cvFloodplainsSystem::AddBankAlongRiver 0x08663704
fn add_bank_along_river(
    grid: &RiverGrid,
    flags: &Flags,
    start: HexEdge,
    visited: &mut HashSet<HexEdge>,
    sims: &mut Vec<BankSim>,
    kind: BankKind,
) {
    let mut left = BankSim::new(BankSide::Left);
    let mut right = BankSim::new(BankSide::Right);
    let mut e = start;
    loop {
        if e.code % 6 >= 3 {
            e = e.twin();
        }
        if is_visited(grid, visited, e) {
            break;
        }
        let has_left = check_for_bank(flags, e, BankSide::Left, kind);
        let has_right = check_for_bank(flags, e, BankSide::Right, kind);
        for (has, sim) in [(has_left, &mut left), (has_right, &mut right)] {
            if has {
                sim.edges.push(e);
            } else if !sim.edges.is_empty() {
                sims.push(BankSim {
                    side: sim.side,
                    edges: std::mem::take(&mut sim.edges),
                });
            }
        }
        if let Some(k) = grid.wrapped(e) {
            visited.insert(k);
        }
        match grid.next_edge(e) {
            Some(n) => e = n,
            None => break,
        }
    }
    for sim in [left, right] {
        if !sim.edges.is_empty() {
            sims.push(sim);
        }
    }
}

/// The sims of a pass: for every flagged hex (column by column, as the original), every river side
/// it names (sides 0..2 in both directions; the others belong to the neighbours) starts the walk of
/// a river not visited yet.
// re: cvFloodplainsSystem::BuildTileLists 0x08661e2c
fn build_tile_lists(grid: &RiverGrid, flags: &Flags, kind: BankKind) -> Vec<BankSim> {
    let mut sims = Vec::new();
    let mut visited: HashSet<HexEdge> = HashSet::new();
    for &(col, row) in flags.set_of(kind) {
        for code in 0..12u8 {
            if code % 6 > 2 {
                continue;
            }
            let e = HexEdge::of_cell(col, row, code);
            if !grid.is_river(e) || is_visited(grid, &visited, e) {
                continue;
            }
            if let Some(source) = traverse_to_river_source(grid, e, &visited) {
                add_bank_along_river(grid, flags, source, &mut visited, &mut sims, kind);
            }
        }
    }
    sims
}

/// Flags both hexes of every side of every river.
// re: cvFloodplainsSystem::AddAllRiverBanks 0x0866343c
fn add_all_river_banks(flags: &mut Flags, rivers: &[Vec<HexEdge>]) {
    for river in rivers {
        for e in river {
            flags.set(BankKind::River, e.x, e.y);
            let a = e.twin();
            flags.set(BankKind::River, a.x, a.y);
        }
    }
}

/// The strips of a pass as decals, in the order of the original, drawing the rolls.
// re: cvFloodplainsSystem::BuildBankVis 0x0866217c
fn build_bank_vis(
    grid: &RiverGrid,
    sim: &RiverSim,
    sims: &[BankSim],
    kind: BankKind,
    frame: &TerrainFrame,
    type_counts: &[usize; 10],
    rand: &mut MsvcRand,
    out: &mut Vec<BankDecal>,
) {
    let names = match kind {
        BankKind::Floodplain => FLOODPLAIN_PARTS,
        BankKind::River => RIVERBANK_PARTS,
    };
    let cats: Vec<DecalCategory> = names
        .iter()
        .map(|n| DecalCategory::of_name(n).expect("a category of the table"))
        .collect();
    debug_assert_eq!(CATEGORY_NAMES.len(), 10);
    for s in sims {
        let mut river: Vec<Vec2> = Vec::new();
        for e in &s.edges {
            river.extend(sim.db.query(*e));
        }
        if river.len() < 2 {
            continue;
        }
        let outline = extrude_control_points(grid, s, frame);
        let mut bank = outline.points;
        let mut widths = outline.widths;
        add_endcaps(&mut bank);
        add_endcaps(&mut river);
        add_endcaps_scalar(&mut widths);
        if bank.len() != river.len() || widths.len() != river.len() {
            // TODO(RW-12): the original reads past the end when the segments of the database do not
            // have two points per side; ours are always two.
            continue;
        }
        let strip = build_strip(&river, &bank, &widths);
        let verts: Vec<DecalVertex> = strip
            .verts
            .iter()
            .map(|v| DecalVertex {
                pos: frame.tex_to_world(v.pos),
                uv: v.uv,
            })
            .collect();
        for (part, (a, b)) in strip.parts().into_iter().enumerate() {
            if b == a {
                continue;
            }
            let category = cats[part];
            let type_roll = if type_counts[usize::from(category.0)] >= MIN_TYPES_FOR_DRAW {
                rand.next()
            } else {
                0
            };
            out.push(BankDecal {
                category,
                verts: verts.clone(),
                tris: strip.indices[a..b]
                    .chunks_exact(3)
                    .map(|t| [t[0], t[1], t[2]])
                    .collect(),
                type_roll,
            });
        }
    }
}

/// The decals of the flood plains and of the banks of the rivers, with a draw of `rand()` for each
/// as if every category had at least two types (see [`build_with_counts`]).
pub fn build(
    grid: &RiverGrid,
    sim: &RiverSim,
    basis: &TerrainBasis,
    frame: &TerrainFrame,
    rand: &mut MsvcRand,
) -> Vec<BankDecal> {
    build_with_counts(grid, sim, basis, frame, rand, &[MIN_TYPES_FOR_DRAW; 10])
}

/// [`build`] with the number of decal types of each category (indexed as `CATEGORY_NAMES`): a category
/// with fewer than two types draws nothing, as `TerrainDecalLayer::CreateDecal` does.
// re: cvFloodplainsSystem::BuildFloodplains 0x08661cea
pub fn build_with_counts(
    grid: &RiverGrid,
    sim: &RiverSim,
    basis: &TerrainBasis,
    frame: &TerrainFrame,
    rand: &mut MsvcRand,
    type_counts: &[usize; 10],
) -> Vec<BankDecal> {
    let mut out = Vec::new();
    if grid.is_empty() {
        return out;
    }
    let mut flags = Flags::new(grid.shape());
    for y in 0..i32::from(basis.height) {
        for x in 0..i32::from(basis.width) {
            if basis
                .plot(x, y)
                .is_some_and(|p| p.feature == BasisFeature::FloodPlains)
            {
                // The cube hex of the cell.
                flags.set(BankKind::Floodplain, x - (y >> 1), y);
            }
        }
    }
    let sims = build_tile_lists(grid, &flags, BankKind::Floodplain);
    build_bank_vis(
        grid,
        sim,
        &sims,
        BankKind::Floodplain,
        frame,
        type_counts,
        rand,
        &mut out,
    );
    add_all_river_banks(&mut flags, &sim.edges);
    let sims = build_tile_lists(grid, &flags, BankKind::River);
    build_bank_vis(
        grid,
        sim,
        &sims,
        BankKind::River,
        frame,
        type_counts,
        rand,
        &mut out,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::{Relief, TerrainKind};
    use crate::world::terrain::basis::PlotBasis;
    use crate::world::water::river::sim;

    const MAP: u32 = 24;

    fn frame() -> TerrainFrame {
        TerrainFrame::new(MAP, MAP, false)
    }

    fn plain() -> PlotBasis {
        PlotBasis {
            relief: Relief::Flat,
            terrain: TerrainKind::Desert,
            river_flow: PlotBasis::NO_FLOW,
            ..PlotBasis::default()
        }
    }

    fn basis(floodplains: &[(i32, i32)]) -> TerrainBasis {
        let mut b = TerrainBasis::uniform(MAP as u16, MAP as u16, false, 1, plain());
        for &(x, y) in floodplains {
            b.plots[(y * MAP as i32 + x) as usize].feature = BasisFeature::FloodPlains;
        }
        b
    }

    fn noisy(u: i32, v: i32) -> u16 {
        (20_000 + ((u * 37 + v * 91) % 1000)) as u16
    }

    /// `steps` edges from `start`, alternating the left and right continuation.
    fn chain(start: HexEdge, steps: usize) -> Vec<HexEdge> {
        let mut v = vec![start];
        let mut e = start;
        for k in 1..steps {
            e = if k % 2 == 0 {
                e.next_left()
            } else {
                e.next_right()
            };
            v.push(e);
        }
        v
    }

    fn river() -> (RiverGrid, RiverSim) {
        let mut g = RiverGrid::new(frame().grid_shape());
        for e in chain(HexEdge::of_cell(12, 12, 0), 6) {
            g.add(e);
        }
        let mut rand = MsvcRand::new(3);
        let s = sim::build(&g, &frame(), &mut rand, &noisy);
        (g, s)
    }

    /// The cell `(col, row)` of the hex of the cube `(x, y)`.
    fn cell_of(x: i32, y: i32) -> (i32, i32) {
        (x + (y >> 1), y)
    }

    /// The cells on the left and on the right of the river's sides.
    fn banks_of(s: &RiverSim) -> (Vec<(i32, i32)>, Vec<(i32, i32)>) {
        let (mut l, mut r) = (Vec::new(), Vec::new());
        for e in &s.edges[0] {
            let (left, right) = if e.code >= 6 {
                (*e, e.twin())
            } else {
                (e.twin(), *e)
            };
            l.push(cell_of(left.x, left.y));
            r.push(cell_of(right.x, right.y));
        }
        (l, r)
    }

    fn run(
        g: &RiverGrid,
        s: &RiverSim,
        fp: &[(i32, i32)],
        counts: &[usize; 10],
    ) -> (Vec<BankDecal>, u64) {
        let mut rand = MsvcRand::new(11);
        let d = build_with_counts(g, s, &basis(fp), &frame(), &mut rand, counts);
        (d, rand.calls())
    }

    fn is_floodplain(c: DecalCategory) -> bool {
        (4..=6).contains(&c.0)
    }

    /// Which bank the strip lies on: 1 left, -1 right, of the flow (north up), by the middle sample.
    fn side_of(d: &BankDecal) -> i32 {
        let q: Vec<&[DecalVertex]> = d.verts.chunks(4).collect();
        let k = q.len() / 2;
        let flow = q[k + 1][0].pos - q[k][0].pos;
        let to_bank = q[k][2].pos - q[k][0].pos;
        // Z points south: flip it for the cross product of the map seen from above.
        let cross = flow.x * (-to_bank.y) - (-flow.y) * to_bank.x;
        if cross > 0.0 { 1 } else { -1 }
    }

    #[test]
    fn without_a_river_or_without_floodplains_there_are_no_floodplain_decals() {
        let g = RiverGrid::new(frame().grid_shape());
        let s = sim::build(&g, &frame(), &mut MsvcRand::new(1), &noisy);
        let (d, calls) = run(&g, &s, &[(3, 3)], &[2; 10]);
        assert!(d.is_empty() && calls == 0);

        let (g, s) = river();
        let (d, _) = run(&g, &s, &[], &[2; 10]);
        assert!(!d.iter().any(|b| is_floodplain(b.category)));
        // The banks of the river are there all the same.
        assert!(d.iter().any(|b| (7..=9).contains(&b.category.0)));
        // A floodplain far from the river makes nothing.
        let (d, _) = run(&g, &s, &[(2, 2)], &[2; 10]);
        assert!(!d.iter().any(|b| is_floodplain(b.category)));
    }

    #[test]
    fn floodplains_on_one_side_give_a_strip_on_that_side() {
        let (g, s) = river();
        let (left, right) = banks_of(&s);
        for (cells, want) in [(&left, 1), (&right, -1)] {
            let (d, _) = run(&g, &s, cells, &[2; 10]);
            let fp: Vec<&BankDecal> = d.iter().filter(|b| is_floodplain(b.category)).collect();
            assert!(!fp.is_empty(), "side {want}");
            for b in &fp {
                assert_eq!(side_of(b), want, "{:?}", b.category);
            }
            // The first part of a strip is the start cap, and the plain middle follows.
            assert_eq!(fp[0].category.name(), "Floodplains_EndCap1");
            assert!(fp.iter().any(|b| b.category.name() == "Floodplains"));
            // The mesh is quads of four vertices with indices inside it.
            for b in &fp {
                assert_eq!(b.verts.len() % 4, 0);
                assert!(!b.tris.is_empty());
                assert!(
                    b.tris
                        .iter()
                        .flatten()
                        .all(|&i| usize::from(i) < b.verts.len())
                );
            }
        }
    }

    #[test]
    fn the_banks_of_the_river_are_on_both_sides() {
        let (g, s) = river();
        let (d, _) = run(&g, &s, &[], &[2; 10]);
        let rb: Vec<&BankDecal> = d
            .iter()
            .filter(|b| (7..=9).contains(&b.category.0))
            .collect();
        assert!(rb.iter().any(|b| side_of(b) == 1));
        assert!(rb.iter().any(|b| side_of(b) == -1));
        assert_eq!(rb[0].category.name(), "Riverbank_EndCap1");
    }

    #[test]
    fn floodplains_come_before_the_banks_and_each_decal_draws_one_roll() {
        let (g, s) = river();
        let (left, _) = banks_of(&s);
        let (d, calls) = run(&g, &s, &left, &[2; 10]);
        let first_bank = d
            .iter()
            .position(|b| (7..=9).contains(&b.category.0))
            .unwrap();
        assert!(d[..first_bank].iter().all(|b| is_floodplain(b.category)));
        assert!(d[first_bank..].iter().all(|b| !is_floodplain(b.category)));
        assert_eq!(calls, d.len() as u64);
        // The rolls are the draws of the stream, in order.
        let mut r = MsvcRand::new(11);
        for b in &d {
            assert_eq!(b.type_roll, r.next());
        }
        // A category with a single type (or none) draws nothing.
        let (d1, calls1) = run(&g, &s, &left, &[1; 10]);
        assert_eq!(calls1, 0);
        assert_eq!(d1.len(), d.len());
        assert!(d1.iter().all(|b| b.type_roll == 0));
        // Only the categories with two types or more draw.
        let mut counts = [1usize; 10];
        counts[4] = 3;
        let (d2, calls2) = run(&g, &s, &left, &counts);
        assert_eq!(
            calls2,
            d2.iter().filter(|b| b.category.0 == 4).count() as u64
        );
    }

    #[test]
    fn build_is_deterministic_and_draws_as_many_rolls_as_decals() {
        let (g, s) = river();
        let (left, right) = banks_of(&s);
        let fp: Vec<(i32, i32)> = left.into_iter().chain(right).collect();
        let (a, ca) = run(&g, &s, &fp, &[2; 10]);
        let (b, cb) = run(&g, &s, &fp, &[2; 10]);
        assert_eq!(a, b);
        assert_eq!(ca, cb);
        assert_eq!(ca, a.len() as u64);
        let mut rand = MsvcRand::new(11);
        let c = build(&g, &s, &basis(&fp), &frame(), &mut rand);
        assert_eq!(a, c);
    }

    #[test]
    fn a_run_of_sides_round_a_hex_is_counted() {
        use super::super::banks::count_internal_edges;
        let (g, s) = river();
        let edges = s.edges[0].clone();
        let sim_l = BankSim {
            side: BankSide::Left,
            edges: edges.clone(),
        };
        let sim_r = BankSim {
            side: BankSide::Right,
            edges,
        };
        // Every count is at least one and inside the list; the last side is a run of one.
        for sm in [&sim_l, &sim_r] {
            let mut i = 0;
            while i < sm.edges.len() {
                let n = count_internal_edges(&g, sm, i);
                assert!(n >= 1 && i + n <= sm.edges.len());
                i += n;
            }
            assert_eq!(count_internal_edges(&g, sm, sm.edges.len() - 1), 1);
        }
        // The chain alternates left and right, so on each bank some runs are longer than one.
        let long = |sm: &BankSim| (0..sm.edges.len()).any(|i| count_internal_edges(&g, sm, i) > 1);
        assert!(long(&sim_l) || long(&sim_r));
    }
}
