//! The bridge "map -> terrain": the port of the first pass of
//! `LoadGameState::StartInitTerrainFromGameplayMap`. It builds the grid of `HexPatternCell` the
//! matcher works on: the padded game grid of `WorldGlobals::SetupWorldGlobals`, its border coast,
//! and per hex the piece class bit (`m_uFlags`) and the continent style. The second pass of the
//! original (ice, flood plains, river sides) feeds `world-water` and is not here.

use super::blit::{Blit, PieceRef};
use super::matcher::{
    HexPatternMatcher, HexTransform, PatternCell, PatternGrid, PatternPiece, SquareGrid,
};
use super::{BuildError, BuildStage, FieldWindow, RasterCx, StageCx, StageId};
use crate::terrain::{Relief, TerrainKind};
use crate::world::terrain::basis::{BasisFeature, BasisFlags, NwPiece, PlotBasis, TerrainBasis};
use crate::world::terrain::field::CellRect;
use crate::world::terrain::library::{BaseKind, TerrainLibrary, style_of_continent};
use crate::world::terrain::sampler::TerrainFrame;

/// `HexPatternCell::m_uFlags`: the piece class of a hex; bit `i` is base type `i` of
/// `g_pBaseTypeNames`.
// re: LoadGameState::StartInitTerrainFromGameplayMap 0x0865cdd8 (LoadGameState.cpp:47-61)
pub mod cell {
    pub const GRASS: u32 = 0x1;
    pub const PLAINS: u32 = 0x2;
    pub const DESERT: u32 = 0x4;
    pub const TUNDRA: u32 = 0x8;
    pub const SNOW: u32 = 0x10;
    pub const GRASS_HILLS: u32 = 0x20;
    pub const PLAINS_HILLS: u32 = 0x40;
    pub const DESERT_HILLS: u32 = 0x80;
    pub const TUNDRA_HILLS: u32 = 0x100;
    pub const SNOW_HILLS: u32 = 0x200;
    pub const MOUNTAIN: u32 = 0x400;
    pub const KILIMANJARO: u32 = 0x800;
    pub const LAKE_VICTORIA: u32 = 0x1000;
    pub const COAST: u32 = 0x2000;
    pub const OCEAN: u32 = 0x4000;
    pub const GEYSER: u32 = 0x8000;
    pub const CRATER: u32 = 0x1_0000;
    pub const POTOSI: u32 = 0x2_0000;
    pub const FUJI: u32 = 0x4_0000;
    pub const MESA: u32 = 0x8_0000;
    pub const REEF: u32 = 0x10_0000;
    pub const VOLCANO: u32 = 0x20_0000;
    pub const ULURU: u32 = 0x40_0000;
    pub const MT_SINAI: u32 = 0x80_0000;
    pub const MT_KAILASH: u32 = 0x100_0000;
    pub const SRI_PADA: u32 = 0x200_0000;
    pub const GIBRALTAR: u32 = 0x400_0000;
}

/// The continent of the border cells.
// re: StartInitTerrainFromGameplayMap (LoadGameState.cpp:124-131)
pub const BORDER_CONTINENT: u8 = 0xff;

/// `g_eWrapMode`: `LoadGameState::Begin` passes 1 for a map that wraps in X, else 0.
// re: LoadGameState::Begin 0x0865bd2e; WorldGlobals::SetupWorldGlobals 0x086725f9
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WrapMode {
    None = 0,
    WrapX = 1,
}

impl WrapMode {
    pub fn of(wrap_x: bool) -> Self {
        if wrap_x { Self::WrapX } else { Self::None }
    }

    /// `WorldGlobals::GetPaddedMapSize`: X wrap pads 4 rows; no wrap pads 4 rows and 4 columns.
    // re: WorldGlobals::GetPaddedMapSize 0x086725b4
    pub fn padded_size(self, width: i32, height: i32) -> (i32, i32) {
        match self {
            Self::WrapX => (width, height + 4),
            Self::None => (width + 4, height + 4),
        }
    }

    /// The translation of the padded grid: the hex of the game grid (the identity) at the origin
    /// of the padded grid.
    // re: WorldGlobals::SetupWorldGlobals 0x086725f9 (mode 1: (1, -2, 1); mode 0: (-1, -2, 3))
    pub fn padded_translation(self) -> [i32; 3] {
        match self {
            Self::WrapX => [1, -2, 1],
            Self::None => [-1, -2, 3],
        }
    }
}

/// The padded game grid (`WorldGlobals::g_PaddedGameGrid`).
pub fn padded_grid(width: i32, height: i32, wrap: WrapMode) -> SquareGrid {
    let (w, h) = wrap.padded_size(width, height);
    SquareGrid::new(
        w,
        h,
        &HexTransform {
            translation: wrap.padded_translation(),
            ..HexTransform::IDENTITY
        },
    )
}

/// The world hex of the plot `(x, y)`: the game grid is the identity.
// re: StartInitTerrainFromGameplayMap, lambda PlotToHex (LoadGameState.cpp:69-74)
pub fn plot_hex(x: i32, y: i32) -> [i32; 3] {
    SquareGrid::hex_of(x, y)
}

/// The class of a natural wonder: its own bit; the ones the table does not list take Fuji's.
// re: StartInitTerrainFromGameplayMap, lambda NaturalWonderFlags (LoadGameState.cpp:77-97)
pub fn wonder_flags(piece: NwPiece) -> u32 {
    match piece {
        NwPiece::Geyser => cell::GEYSER,
        NwPiece::Crater => cell::CRATER,
        NwPiece::Gibraltar => cell::GIBRALTAR,
        NwPiece::Fuji => cell::FUJI,
        NwPiece::Mesa => cell::MESA,
        NwPiece::Reef => cell::REEF,
        NwPiece::Krakatoa => cell::VOLCANO,
        NwPiece::Potosi => cell::POTOSI,
        NwPiece::Uluru => cell::ULURU,
        NwPiece::MtSinai => cell::MT_SINAI,
        NwPiece::MtKailash => cell::MT_KAILASH,
        NwPiece::SriPada => cell::SRI_PADA,
        NwPiece::Kilimanjaro => cell::KILIMANJARO,
        NwPiece::LakeVictoria => cell::LAKE_VICTORIA,
    }
}

/// The class bit and the continent of one plot.
// re: StartInitTerrainFromGameplayMap (LoadGameState.cpp:148-203)
// SIMPLIFIED(SIMP-exe-1072): wonders without a piece by the type name of the catalog
pub fn plot_cell(plot: &PlotBasis) -> (u32, u8) {
    let (flags, continent) = match (plot.terrain, plot.relief) {
        (TerrainKind::Ocean, _) => (cell::OCEAN, 0),
        (TerrainKind::Coast, _) | (_, Relief::Water) => (cell::COAST, 0),
        (_, Relief::Mountain) => (cell::MOUNTAIN, plot.continent),
        (t, Relief::Hill) => (
            match t {
                TerrainKind::Grass => cell::GRASS_HILLS,
                TerrainKind::Desert => cell::DESERT_HILLS,
                TerrainKind::Snow => cell::SNOW_HILLS,
                TerrainKind::Tundra => cell::TUNDRA_HILLS,
                _ => cell::PLAINS_HILLS,
            },
            plot.continent,
        ),
        (t, Relief::Flat) => (
            match t {
                TerrainKind::Grass => cell::GRASS,
                TerrainKind::Desert => cell::DESERT,
                TerrainKind::Snow => cell::SNOW,
                TerrainKind::Tundra => cell::TUNDRA,
                _ => cell::PLAINS,
            },
            plot.continent,
        ),
    };
    match plot.feature {
        // A wonder keeps the continent of the plot, on water too.
        BasisFeature::NaturalWonder(nw) => (wonder_flags(nw), plot.continent),
        // El Dorado and the Fountain of Youth keep the class of the terrain under them (a mountain
        // stays a mountain), and the continent of the plot.
        BasisFeature::WonderKeepsTerrain => (flags, plot.continent),
        BasisFeature::WonderDesert => (cell::DESERT, plot.continent),
        // The other wonders the install names (Everest, Titicaca, ...) take Fuji's, as the original.
        _ if plot.flags.contains(BasisFlags::NATURAL_WONDER) => (cell::FUJI, plot.continent),
        _ => (flags, continent),
    }
}

/// The grid of the matcher for a basis: the padded grid, all coast of continent 0xff, then every
/// plot at its padded cell.
// re: StartInitTerrainFromGameplayMap (LoadGameState.cpp:101-211)
pub fn pattern_grid(basis: &TerrainBasis) -> PatternGrid {
    let (w, h) = (i32::from(basis.width), i32::from(basis.height));
    let policy = padded_grid(w, h, WrapMode::of(basis.wrap_x));
    let mut grid = PatternGrid::new(
        policy,
        PatternCell {
            flags: cell::COAST,
            continent: BORDER_CONTINENT,
            covered: false,
        },
    );
    for x in 0..w {
        for y in 0..h {
            let Some(plot) = basis.plot(x, y) else {
                continue;
            };
            let (px, py) = SquareGrid::cell_of(policy.forward.apply(plot_hex(x, y)));
            let (flags, continent) = plot_cell(plot);
            if let Some(c) = grid.cell_mut(px, py) {
                *c = PatternCell {
                    flags,
                    continent,
                    covered: false,
                };
            }
        }
    }
    grid
}

/// The plot of a padded cell, if the cell is on the map.
pub fn plot_of_cell(basis: &TerrainBasis, grid: &SquareGrid, x: i32, y: i32) -> Option<(i32, i32)> {
    let (px, py) = SquareGrid::cell_of(grid.inverse.apply(SquareGrid::hex_of(x, y)));
    basis.index(px, py).map(|_| (px, py))
}

/// The pieces of the library in the order the matcher takes them, without the spare
/// (`missing_hex`).
// TODO(assets-b2): the original's order is the one of `TerrainPiecesLibrary.fxsb`: base type
// (`g_pBaseTypeNames`) outer, continent inner, file name; 299 pieces, all with flag 4. The sets
// `pc:<style>` keep neither the base type nor that order, so the stream of the shuffles of the
// pieces differs from the original's until they do (D-3).
// SIMPLIFIED(SIMP-035): the piece variants come from our own stream of `rand`; D-3 has not shown it equals the original's choice.
pub fn library_pieces(library: &TerrainLibrary) -> Vec<PatternPiece> {
    library_pieces_with_refs(library).0
}

/// [`library_pieces`] and, for each, where its data is in the library (the blit reads it there).
pub fn library_pieces_with_refs(library: &TerrainLibrary) -> (Vec<PatternPiece>, Vec<PieceRef>) {
    let mut out = Vec::new();
    let mut refs = Vec::new();
    for continent in 0..5u8 {
        let Some(style) = style_of_continent(continent) else {
            continue;
        };
        let Some(set) = library.piece_set(style) else {
            continue;
        };
        for (index, piece) in set.pieces.iter().enumerate() {
            if BaseKind::of_piece_name(&piece.name) == BaseKind::Spare {
                continue;
            }
            if let Some(p) = PatternPiece::from_asset(piece, continent, out.len() as u32) {
                out.push(p);
                refs.push(PieceRef {
                    style,
                    index,
                    small: piece.side <= 128,
                });
            }
        }
    }
    (out, refs)
}

/// The stage `Pieces` with the matcher (WT-5): the global part runs the bridge and the matcher on
/// the stream of the build and sets up the jobs of the blit; the cells are written one by one,
/// lazily, when the cache asks for them.
#[derive(Default)]
pub struct MatchedPieces {
    matcher: HexPatternMatcher,
    grid: Option<SquareGrid>,
    blit: Option<Blit>,
    /// Frame and width of the basis the global part saw, to find the cells of a changed hex.
    seen: Option<(TerrainFrame, u16)>,
}

impl MatchedPieces {
    pub fn matcher(&self) -> &HexPatternMatcher {
        &self.matcher
    }

    /// The padded grid of the last basis.
    pub fn grid(&self) -> Option<&SquareGrid> {
        self.grid.as_ref()
    }

    pub fn blit(&self) -> Option<&Blit> {
        self.blit.as_ref()
    }
}

impl BuildStage for MatchedPieces {
    fn id(&self) -> StageId {
        StageId::Pieces
    }

    // re: cvTerrainBlend::BlendTerrainHeightmap 0x089184a4 (the matcher, before the blit jobs)
    fn global(&mut self, cx: &mut StageCx<'_>) -> Result<(), BuildError> {
        self.seen = Some((*cx.frame, cx.basis.width));
        self.matcher = HexPatternMatcher::new();
        let (pieces, refs) = library_pieces_with_refs(cx.library);
        for p in pieces {
            self.matcher.add_piece(p);
        }
        let mut grid = pattern_grid(cx.basis);
        self.matcher.compute_pattern_positions(&mut grid, cx.rand);
        // The blit needs the grid as the matcher left it only for its policy.
        self.blit = Some(Blit::new(
            cx.basis,
            &self.matcher,
            &pattern_grid(cx.basis),
            refs,
            cx.library.defs(),
            &cx.products.blend_splines,
        ));
        self.grid = Some(grid.policy);
        Ok(())
    }

    fn raster(&self, cell: CellRect, window: &mut FieldWindow<'_>, cx: &RasterCx<'_>) {
        let Some(blit) = &self.blit else {
            return;
        };
        let mut writes: Vec<(i32, i32, u16, u8)> = Vec::new();
        blit.raster(cell, cx.library, &mut |u, v, h, id| {
            writes.push((u, v, h, id))
        });
        for (u, v, h, id) in writes {
            window.set(u, v, h, id);
        }
    }

    fn invalidate(&mut self, changed: &[u32]) -> Vec<CellRect> {
        let Some((frame, width)) = self.seen else {
            return Vec::new();
        };
        let w = u32::from(width).max(1);
        changed
            .iter()
            .map(|&i| frame.hex_texel_rect((i % w) as i32, (i / w) as i32))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plot(relief: Relief, terrain: TerrainKind, continent: u8) -> PlotBasis {
        PlotBasis {
            relief,
            terrain,
            continent,
            river_flow: PlotBasis::NO_FLOW,
            ..PlotBasis::default()
        }
    }

    #[test]
    fn wonders_without_a_piece_take_the_class_the_original_gives() {
        let wonder = |feature: BasisFeature, relief: Relief| {
            let mut p = plot(relief, TerrainKind::Grass, 2);
            p.feature = feature;
            p.flags = BasisFlags(BasisFlags::NATURAL_WONDER);
            plot_cell(&p)
        };
        // El Dorado on a mountain: the mountain class, not Fuji's (the wonder floated over the relief).
        let eldorado = BasisFeature::from_type_name("FEATURE_EL_DORADO");
        assert_eq!(wonder(eldorado, Relief::Mountain), (cell::MOUNTAIN, 2));
        assert_eq!(wonder(eldorado, Relief::Hill), (cell::GRASS_HILLS, 2));
        let fountain = BasisFeature::from_type_name("FEATURE_FOUNTAIN_YOUTH");
        assert_eq!(wonder(fountain, Relief::Flat), (cell::GRASS, 2));
        // Solomon's Mines: the desert class.
        let mines = BasisFeature::from_type_name("FEATURE_SOLOMONS_MINES");
        assert_eq!(wonder(mines, Relief::Hill), (cell::DESERT, 2));
        // Any other wonder of the install without a piece: Fuji's.
        assert_eq!(wonder(BasisFeature::Other, Relief::Flat), (cell::FUJI, 2));
        // The ones with a piece keep theirs; Lake Victoria is its own tile set.
        let lake = BasisFeature::from_type_name("FEATURE_LAKE_VICTORIA");
        assert_eq!(wonder(lake, Relief::Flat), (cell::LAKE_VICTORIA, 2));
        // A plot with no wonder flag is the terrain.
        let mut p = plot(Relief::Mountain, TerrainKind::Grass, 2);
        p.feature = eldorado;
        assert_eq!(plot_cell(&p), (cell::MOUNTAIN, 2));
    }

    #[test]
    fn the_padded_grid_of_a_wrapped_map_has_two_rows_above_and_below() {
        let g = padded_grid(10, 6, WrapMode::WrapX);
        assert_eq!((g.width, g.height), (10, 10));
        for (x, y) in [(0, 0), (3, 1), (9, 5), (4, 4)] {
            let cell = SquareGrid::cell_of(g.forward.apply(plot_hex(x, y)));
            assert_eq!(cell, (x, y + 2), "plot ({x}, {y})");
            // And back.
            let back = SquareGrid::cell_of(g.inverse.apply(SquareGrid::hex_of(cell.0, cell.1)));
            assert_eq!(back, (x, y));
        }
    }

    #[test]
    fn the_padded_grid_of_a_flat_map_has_two_cells_on_every_side() {
        let g = padded_grid(10, 6, WrapMode::None);
        assert_eq!((g.width, g.height), (14, 10));
        for (x, y) in [(0, 0), (3, 1), (9, 5)] {
            assert_eq!(
                SquareGrid::cell_of(g.forward.apply(plot_hex(x, y))),
                (x + 2, y + 2)
            );
        }
    }

    #[test]
    fn plots_take_the_class_of_their_terrain_and_relief() {
        assert_eq!(
            plot_cell(&plot(Relief::Water, TerrainKind::Ocean, 3)),
            (cell::OCEAN, 0)
        );
        assert_eq!(
            plot_cell(&plot(Relief::Water, TerrainKind::Coast, 3)),
            (cell::COAST, 0)
        );
        assert_eq!(
            plot_cell(&plot(Relief::Mountain, TerrainKind::Snow, 2)),
            (cell::MOUNTAIN, 2)
        );
        assert_eq!(
            plot_cell(&plot(Relief::Hill, TerrainKind::Tundra, 1)),
            (cell::TUNDRA_HILLS, 1)
        );
        assert_eq!(
            plot_cell(&plot(Relief::Hill, TerrainKind::Plains, 1)),
            (cell::PLAINS_HILLS, 1)
        );
        assert_eq!(
            plot_cell(&plot(Relief::Flat, TerrainKind::Desert, 4)),
            (cell::DESERT, 4)
        );
        assert_eq!(
            plot_cell(&plot(Relief::Flat, TerrainKind::Unknown, 4)),
            (cell::PLAINS, 4)
        );
        let mut reef = plot(Relief::Water, TerrainKind::Coast, 3);
        reef.feature = BasisFeature::NaturalWonder(NwPiece::Reef);
        assert_eq!(plot_cell(&reef), (cell::REEF, 3));
    }

    /// With the pieces of the install (`CIV5_PIECES_FXSB` = the path of
    /// `TerrainPiecesLibrary.fxsb`; skipped without it): every hex of a map of every land class
    /// is covered but the ocean by the land, and the draws are the count of the shuffles.
    #[test]
    fn the_pieces_of_the_install_cover_a_map() {
        let Ok(path) = std::env::var("CIV5_PIECES_FXSB") else {
            return;
        };
        let bytes = std::fs::read(path).expect("fxsb");
        let pieces = super::super::matcher::pieces_from_fxsb(&bytes).expect("fxsb table");
        assert_eq!(pieces.len(), 299);
        let mut m = HexPatternMatcher::new();
        for p in pieces {
            m.add_piece(p);
        }
        assert_eq!(m.max_pass(), 4);
        let kinds = [
            (Relief::Flat, TerrainKind::Grass),
            (Relief::Flat, TerrainKind::Plains),
            (Relief::Flat, TerrainKind::Desert),
            (Relief::Flat, TerrainKind::Tundra),
            (Relief::Flat, TerrainKind::Snow),
            (Relief::Hill, TerrainKind::Grass),
            (Relief::Hill, TerrainKind::Desert),
            (Relief::Mountain, TerrainKind::Plains),
            (Relief::Water, TerrainKind::Coast),
            (Relief::Water, TerrainKind::Ocean),
        ];
        let mut b =
            TerrainBasis::uniform(20, 12, true, 1, plot(Relief::Flat, TerrainKind::Grass, 1));
        for (i, p) in b.plots.iter_mut().enumerate() {
            let (r, t) = kinds[(i / 3) % kinds.len()];
            *p = plot(r, t, 1 + (i % 4) as u8);
        }
        let mut grid = pattern_grid(&b);
        let mut rng = super::super::rand::MsvcRand::new(12345);
        m.compute_pattern_positions(&mut grid, &mut rng);
        assert_eq!(rng.calls(), m.draws(grid.policy.width, grid.policy.height));
        // The ocean piece wants water on all six sides: an ocean hex by the land stays open.
        let open: Vec<_> = grid
            .cells
            .iter()
            .filter(|c| !c.covered && c.flags != cell::OCEAN)
            .map(|c| (c.flags, c.continent))
            .collect();
        assert!(
            open.is_empty(),
            "{} positions; open {open:x?}",
            m.positions().len()
        );
    }

    #[test]
    fn the_grid_is_coast_around_the_map() {
        let mut b = TerrainBasis::uniform(4, 3, true, 1, plot(Relief::Flat, TerrainKind::Grass, 1));
        let i = b.index(2, 1).unwrap();
        b.plots[i] = plot(Relief::Mountain, TerrainKind::Grass, 1);
        let g = pattern_grid(&b);
        assert_eq!((g.policy.width, g.policy.height), (4, 7));
        assert_eq!(g.cell(0, 0).unwrap().flags, cell::COAST);
        assert_eq!(g.cell(0, 0).unwrap().continent, BORDER_CONTINENT);
        assert_eq!(g.cell(0, 6).unwrap().flags, cell::COAST);
        assert_eq!(g.cell(0, 2).unwrap().flags, cell::GRASS);
        assert_eq!(g.cell(2, 3).unwrap().flags, cell::MOUNTAIN);
        assert_eq!(plot_of_cell(&b, &g.policy, 2, 3), Some((2, 1)));
        assert_eq!(plot_of_cell(&b, &g.policy, 2, 0), None);
    }
}
