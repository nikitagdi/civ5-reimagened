use super::super::matcher::{MATCHER_TRANSFORMS, PIECE_GRID, PieceCell};
use super::*;

#[test]
fn a_line_has_one_texel_per_step_of_its_major_axis() {
    let mut v = Vec::new();
    rasterize_line((0, 0), (5, 0), &mut |p| v.push(p));
    assert_eq!(v, [(0, 0), (1, 0), (2, 0), (3, 0), (4, 0), (5, 0)]);
    v.clear();
    rasterize_line((0, 0), (2, 6), &mut |p| v.push(p));
    assert_eq!(v.len(), 7);
    assert_eq!(v[0], (0, 0));
    assert_eq!(*v.last().unwrap(), (2, 6));
    // Backwards, the same texels (from the other end).
    let mut w = Vec::new();
    rasterize_line((2, 6), (0, 0), &mut |p| w.push(p));
    assert_eq!(w.len(), 7);
    assert!(w.contains(&(0, 0)) && w.contains(&(2, 6)));
}

#[test]
fn the_pattern_type_and_the_class_of_a_hex_follow_the_masks() {
    let piece = |code: u32| {
        let mut cells = [PieceCell::EMPTY; PIECE_GRID * PIECE_GRID];
        cells[11] = PieceCell {
            hex_code: code,
            continent: 0,
            valid: true,
        };
        PatternPiece::new(10, 10, cells, 2, 0)
    };
    assert_eq!(pattern_type(&piece(0x400)), 0, "mountain");
    assert_eq!(pattern_type(&piece(0x2000)), 1, "coast");
    assert_eq!(pattern_type(&piece(0x4000)), 2, "ocean");
    assert_eq!(hex_class(0x1), 0);
    assert_eq!(hex_class(0x2000), 1);
    assert_eq!(hex_class(0x4000), 2);
}

#[test]
fn the_rows_of_terrain_def_have_the_codes_of_the_id_maps() {
    // ID 43 of Africa is the digits 4 and 3 in hex, with the prefix of the continent.
    assert_eq!(row_code(43, 3), 0x0013_0043);
    assert_eq!(row_code(0, 0), 0x0010_0000);
    assert_eq!(row_code(10, 4), 0x0014_0010);
}

/// The hex of the piece the corners of every transform put in the middle of its rectangle.
#[test]
fn the_corners_of_a_small_piece_centre_on_its_hex_for_every_transform() {
    for wrap in [true, false] {
        let mats = WorldMatrices::new(wrap);
        for (i, t) in MATCHER_TRANSFORMS.iter().enumerate() {
            let m = t.inv_matrix();
            let c = corner_offsets(&mats, &m);
            // Piece hex (row 1, column 1) = cube (1, 1, -2), through the transform.
            let h = m.apply([1, 1, -2]);
            let at = mats.hex_to_texel([0.0, 0.0, 0.0]);
            let hx = mats.hex_to_texel([h[0] as f32, h[1] as f32, h[2] as f32]);
            let (cu, cv) = (hx[0] - at[0], hx[1] - at[1]);
            let (mu, mv) = (
                (c[0].0 + c[1].0) as f32 / 2.0,
                (c[0].1 + c[1].1) as f32 / 2.0,
            );
            assert!(
                (cu - mu).abs() <= 2.0 && (cv - mv).abs() <= 2.0,
                "t{i}: {cu},{cv} vs {mu},{mv}"
            );
            // The rectangle is 128 texels on each side.
            assert!(((c[1].0 - c[0].0).abs() - 128).abs() <= 2, "t{i} {c:?}");
            assert!(((c[1].1 - c[0].1).abs() - 128).abs() <= 2, "t{i} {c:?}");
            assert!(((c[3].0 - c[2].0).abs() - 512).abs() <= 2, "t{i} {c:?}");
        }
    }
}

mod map {
    use super::super::super::{RasterCx, TerrainBuild};
    use crate::hexgrid::cell_center;
    use crate::terrain::{Relief, TerrainKind};
    use crate::world::terrain::basis::{PlotBasis, TerrainBasis};
    use crate::world::terrain::field::CellRect;
    use crate::world::terrain::library::TerrainLibrary;
    use crate::world::terrain::sampler::{TerrainFrame, TerrainSampler};
    use crate::world::terrain::stencil::TerrainStencil;

    fn flat() -> PlotBasis {
        PlotBasis {
            terrain: TerrainKind::Grass,
            river_flow: PlotBasis::NO_FLOW,
            ..PlotBasis::default()
        }
    }

    pub fn built(w: u16, h: u16, wrap: bool) -> (TerrainSampler, TerrainBasis) {
        let mut b = TerrainBasis::uniform(w, h, wrap, 5, flat());
        let set = |b: &mut TerrainBasis, x: i32, y: i32, relief, terrain| {
            let i = b.index(x, y).unwrap();
            b.plots[i] = PlotBasis {
                relief,
                terrain,
                ..flat()
            };
        };
        set(&mut b, 1, 1, Relief::Hill, TerrainKind::Grass);
        set(&mut b, 2, 2, Relief::Mountain, TerrainKind::Plains);
        set(&mut b, 0, 0, Relief::Water, TerrainKind::Coast);
        if w > 4 {
            set(&mut b, 1, 4, Relief::Water, TerrainKind::Coast);
        }
        let lib = TerrainLibrary::synthetic();
        let frame = TerrainFrame::new(u32::from(w), u32::from(h), wrap);
        let mut s = TerrainSampler::new(frame);
        let (tw, th) = frame.texel_size();
        let mut stencil = TerrainStencil::new(tw, th);
        let mut build = TerrainBuild::new(b.seed);
        build
            .run_global(&b, &lib, &frame, s.field_mut(), &mut stencil)
            .unwrap();
        let products = build.products().clone();
        let cx = RasterCx {
            basis: &b,
            library: &lib,
            frame: &frame,
            products: &products,
        };
        for y in 0..i32::from(h) {
            for x in 0..i32::from(w) {
                build.raster_cell(CellRect::cell(x, y), s.field_mut(), &mut stencil, &cx);
            }
        }
        (s, b)
    }

    #[test]
    fn the_pieces_give_each_hex_the_height_and_the_id_of_its_relief() {
        for wrap in [false, true] {
            let (s, _) = built(8, 6, wrap);
            let at = |x, y| s.world_height(cell_center(x, y));
            let id = |x, y| {
                let c = s.frame().hex_to_tex(x, y);
                s.field().get_id(c.x as i32, c.y as i32)
            };
            assert!((at(3, 2) - 12.097).abs() < 0.05, "flat {}", at(3, 2));
            assert!((at(1, 1) - 25.0).abs() < 0.8, "hill {}", at(1, 1));
            assert!((at(2, 2) - 49.0).abs() < 1.5, "mountain {}", at(2, 2));
            assert!((at(0, 0) - 10.196).abs() < 0.05, "water {}", at(0, 0));
            assert!((at(1, 4) - 10.196).abs() < 0.05, "water {}", at(1, 4));
            // 4 rows per style in the synthetic table: grass, hill, mountain, coast.
            assert_eq!(id(1, 1) % 4, 1);
            assert_eq!(id(2, 2) % 4, 2);
            assert_eq!(id(0, 0) % 4, 3);
        }
    }

    #[test]
    fn a_second_run_gives_the_same_field_byte_for_byte() {
        let (a, _) = built(8, 6, true);
        let (b, _) = built(8, 6, true);
        assert!(a.field().same_content(b.field()));
        assert_eq!(a.field().digest(), b.field().digest());
    }
}
