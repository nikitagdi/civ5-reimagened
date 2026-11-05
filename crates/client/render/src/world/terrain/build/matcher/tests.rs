use super::*;

use std::collections::HashSet;

use super::super::bridge::{cell, pattern_grid};
use super::super::rand::MsvcRand;
use crate::terrain::{Relief, TerrainKind};
use crate::world::terrain::basis::{PlotBasis, TerrainBasis};

/// The hexes of a window of the plane `x + y + z = 0`.
fn plane_hexes() -> Vec<HexVec> {
    let mut out = Vec::new();
    for x in -3..=12 {
        for y in -3..=12 {
            out.push(SquareGrid::hex_of(x, y));
        }
    }
    out
}

#[test]
fn the_transforms_of_the_matcher_are_inverse_of_their_forward_matrix() {
    for t in MATCHER_TRANSFORMS.iter() {
        for h in plane_hexes() {
            assert_eq!(
                t.matrix().apply(t.inv_matrix().apply(h)),
                h,
                "{t:?} at {h:?}"
            );
        }
    }
    for h in plane_hexes() {
        assert_eq!(HexTransform::IDENTITY.inv_matrix().apply(h), h);
    }
}

#[test]
fn rotation_zero_is_the_identity_on_the_hex_plane() {
    for h in plane_hexes() {
        assert_eq!(HexMat::rotation(0).apply(h), h);
    }
    // Six sixths is a full turn.
    assert_eq!(HexMat::rotation(6), HexMat::rotation(0));
}

#[test]
fn every_reflection_keeps_hexes_on_the_plane() {
    for m in REFLECTIONS.iter() {
        for h in plane_hexes() {
            let r = m.apply(h);
            assert_eq!(r[0] + r[1] + r[2], 0, "{m:?} at {h:?}");
        }
        // A unit hex stays a unit hex.
        let r = m.apply([1, -1, 0]);
        assert_eq!(r.iter().map(|v| v.abs()).max(), Some(1), "{m:?}");
    }
}

#[test]
fn a_csv_cell_decodes_like_the_loader() {
    assert_eq!(
        PieceCell::from_csv(1, 3, 4),
        PieceCell {
            hex_code: 1,
            continent: 3,
            valid: true
        }
    );
    // Zero is "none": the code becomes 0xffffffff, the cell is still valid.
    let none = PieceCell::from_csv(0, 3, 4);
    assert_eq!(
        (none.hex_code, none.continent, none.valid),
        (0xffff_ffff, 3, true)
    );
    // A negative number is a cell the piece must fit but does not take.
    let must_fit = PieceCell::from_csv(-1, 3, 4);
    assert_eq!(
        (must_fit.hex_code, must_fit.continent, must_fit.valid),
        (0x7fff_ffff, 0, false)
    );
    // Version 3 codes go through the legacy table.
    assert_eq!(PieceCell::from_csv(0x0f_ffff, 2, 3).hex_code, 0xffff_ffff);
    assert_eq!(PieceCell::from_csv(0x1800, 2, 3).hex_code, 0x10_6000);
    // Key index 11 is value index 13.
    assert_eq!(PieceCell::from_csv(0x800, 2, 3).hex_code, 0x2000);
    assert_eq!(PieceCell::from_csv(0x00ff_ffff, 2, 3).hex_code, 0xffff_ffff);
}

#[test]
fn fits_matches_zero_masks_and_shared_bits() {
    let piece = PieceCell {
        hex_code: 0x1,
        continent: 1,
        valid: true,
    };
    let grid = |flags, continent| PatternCell {
        flags,
        continent,
        covered: false,
    };
    // A shared bit and the same continent.
    assert!(piece.fits(&grid(0x1, 1)));
    // A shared bit but another continent on both sides.
    assert!(!piece.fits(&grid(0x1, 2)));
    // A grid continent of 0 matches any.
    assert!(piece.fits(&grid(0x1, 0)));
    // A piece continent of 0 matches any grid continent.
    let any_continent = PieceCell {
        continent: 0,
        ..piece
    };
    assert!(any_continent.fits(&grid(0x1, 2)));
    // No shared bit.
    assert!(!piece.fits(&grid(0x2, 1)));
    // A grid hex of no class takes any piece.
    assert!(piece.fits(&grid(0, 1)));
    // A piece cell of no code takes any grid hex.
    let no_code = PieceCell {
        hex_code: 0,
        ..piece
    };
    assert!(no_code.fits(&grid(0x2, 1)));
}

/// A piece with one hex, at row 1 and column 1.
fn one_hex_piece(code: u32, continent: u8, pass: u32, index: u32) -> PatternPiece {
    let mut cells = [PieceCell::EMPTY; PIECE_GRID * PIECE_GRID];
    // Column 1, row 1.
    cells[10 + 1] = PieceCell {
        hex_code: code,
        continent,
        valid: true,
    };
    PatternPiece::new(10, 10, cells, pass, index)
}

/// A piece with three hexes: (row, column) (3, 3), (4, 4) and (5, 4).
fn three_hex_piece(code: u32, continent: u8, pass: u32, index: u32) -> PatternPiece {
    let mut cells = [PieceCell::EMPTY; PIECE_GRID * PIECE_GRID];
    for (row, col) in [(3, 3), (4, 4), (5, 4)] {
        cells[col * 10 + row] = PieceCell {
            hex_code: code,
            continent,
            valid: true,
        };
    }
    PatternPiece::new(10, 10, cells, pass, index)
}

#[test]
fn a_piece_of_one_hex_uses_one_cell() {
    let p = one_hex_piece(cell::GRASS, 1, 1, 0);
    assert_eq!(p.used, vec![(1, 1)]);
    assert_eq!(
        three_hex_piece(cell::GRASS, 1, 2, 0).used,
        vec![(3, 3), (4, 4), (5, 4)]
    );
}

/// A map of `w` x `h` plots, the classes in turn: grass, plains, hill, mountain, ocean.
fn basis(w: u16, h: u16, wrap: bool) -> TerrainBasis {
    let land = PlotBasis {
        relief: Relief::Flat,
        terrain: TerrainKind::Grass,
        continent: 1,
        river_flow: PlotBasis::NO_FLOW,
        ..PlotBasis::default()
    };
    let mut b = TerrainBasis::uniform(w, h, wrap, 7, land);
    for (i, p) in b.plots.iter_mut().enumerate() {
        *p = match i % 5 {
            0 => land,
            1 => PlotBasis {
                terrain: TerrainKind::Plains,
                ..land
            },
            2 => PlotBasis {
                relief: Relief::Hill,
                ..land
            },
            3 => PlotBasis {
                relief: Relief::Mountain,
                ..land
            },
            _ => PlotBasis {
                relief: Relief::Water,
                terrain: TerrainKind::Ocean,
                ..land
            },
        };
    }
    b
}

/// One piece per class of the map, and two three-hex pieces of pass 2 (the big pieces). A hex of
/// the `basis` never touches a hex of its own class, so the big pieces take a mask of several
/// classes: land (grass, plains, hills) and water (ocean, coast).
fn library() -> Vec<PatternPiece> {
    let mut lib = Vec::new();
    let one_hex = [
        (cell::GRASS, 1),
        (cell::PLAINS, 1),
        (cell::GRASS_HILLS, 1),
        (cell::MOUNTAIN, 1),
        (cell::OCEAN, 0),
        (cell::COAST, 0),
    ];
    for (code, continent) in one_hex {
        let index = lib.len() as u32;
        lib.push(one_hex_piece(code, continent, 1, index));
    }
    let index = lib.len() as u32;
    lib.push(three_hex_piece(
        cell::GRASS | cell::PLAINS | cell::GRASS_HILLS,
        1,
        2,
        index,
    ));
    let index = lib.len() as u32;
    lib.push(three_hex_piece(cell::OCEAN | cell::COAST, 0, 2, index));
    lib
}

/// Runs the matcher on a 12 x 8 map with the library and the seed.
fn run(wrap: bool, seed: u32) -> (HexPatternMatcher, PatternGrid, MsvcRand) {
    let b = basis(12, 8, wrap);
    let mut grid = pattern_grid(&b);
    let mut m = HexPatternMatcher::new();
    for p in library() {
        m.add_piece(p);
    }
    let mut rng = MsvcRand::new(seed);
    m.compute_pattern_positions(&mut grid, &mut rng);
    (m, grid, rng)
}

#[test]
fn every_hex_of_the_map_is_covered() {
    for wrap in [false, true] {
        let (m, grid, rng) = run(wrap, 12345);
        assert!(grid.cells.iter().all(|c| c.covered), "wrap {wrap}");
        // The draws of the stream are the ones the count predicts.
        assert_eq!(rng.calls(), m.draws(grid.policy.width, grid.policy.height));
    }
}

#[test]
fn no_hex_is_covered_twice() {
    for wrap in [false, true] {
        let (m, grid, _) = run(wrap, 12345);
        let mut seen = HashSet::new();
        for pos in m.positions() {
            for xy in m.covered_cells(pos, &grid.policy) {
                assert!(seen.insert(xy), "{xy:?} covered twice (wrap {wrap})");
            }
        }
        assert_eq!(seen.len(), grid.cells.len());
    }
}

#[test]
fn the_big_pieces_go_first() {
    for wrap in [false, true] {
        let (m, _, _) = run(wrap, 12345);
        let passes: Vec<u32> = m
            .positions()
            .iter()
            .map(|p| m.pieces()[p.piece as usize].pass)
            .collect();
        let last_big = passes
            .iter()
            .rposition(|&p| p == 2)
            .expect("a pass-2 piece is placed");
        // No position of a smaller pass comes before the last big one.
        assert!(passes[..=last_big].iter().all(|&p| p == 2), "wrap {wrap}");
    }
}

#[test]
fn the_same_seed_gives_the_same_positions_and_another_seed_others() {
    let (a, _, _) = run(false, 12345);
    let (b, _, _) = run(false, 12345);
    assert_eq!(a.positions(), b.positions());
    let (c, _, _) = run(false, 1);
    assert_ne!(a.positions(), c.positions());
}

#[test]
fn a_piece_fits_only_its_class() {
    for wrap in [false, true] {
        let (m, _, _) = run(wrap, 12345);
        // A fresh grid, for the class bits of the map.
        let fresh = pattern_grid(&basis(12, 8, wrap));
        for pos in m.positions() {
            let piece = &m.pieces()[pos.piece as usize];
            let placement = m.placement(pos, &fresh.policy);
            for &(row, column) in &piece.used {
                let pc = piece.cells[usize::from(column) * 10 + usize::from(row)];
                if !pc.valid {
                    continue;
                }
                let hex = SquareGrid::hex_of(i32::from(row), i32::from(column));
                let (x, y) = SquareGrid::cell_of(placement.apply(hex));
                if let Some(c) = fresh.cell(x, y) {
                    assert!(
                        c.flags & pc.hex_code != 0,
                        "piece {} at ({x}, {y})",
                        pos.piece
                    );
                }
            }
        }
    }
}

#[test]
fn shuffle_draws_counts_like_the_shuffle() {
    for n in [0usize, 1, 2, 5, 300, 40_000] {
        let mut v: Vec<u32> = (0..n as u32).collect();
        let mut r = MsvcRand::new(3);
        vc9_random_shuffle(&mut v, &mut r);
        assert_eq!(r.calls(), shuffle_draws(n as u64), "n = {n}");
    }
}
