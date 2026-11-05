//! The stage `Marsh` of the terrain build (world-water spec 3.1 row `Marsh`, WW-D11, 4.6):
//! `cvMarshSimSystem::StartupGame` handles every marsh the game announced (`OnMarshAdded`) and
//! stamps the 128 x 128 mask `marsh_stamp.dds`, centred on the hex, into the terrain ids of the
//! field (`BlitMaskedIDs`, `Fun_087e735c`). The stamp writes ids only; it reads the heights as a
//! filter and never changes them.

use std::sync::Arc;

use asset_format::{TexDim, TextureBlob, TextureFormat};

use crate::world::terrain::basis::BasisFeature;
use crate::world::terrain::build::{
    BuildError, BuildStage, FieldWindow, RasterCx, RasterScope, StageCx, StageId,
};
use crate::world::terrain::field::CellRect;
use crate::world::terrain::library::TerrainDefs;
use crate::world::terrain::sampler::TerrainFrame;

/// Side of the stamp in texels and its half.
// re: cvMarshSimSystem.cpp `kStampSize`, `kStampHalfSize` (OnMarshAdded 0x087e6f7e: `(int)tex - 0x40`, 0x80)
pub const STAMP_SIZE: i32 = 128;
pub const STAMP_HALF: i32 = 64;

/// The stamp paints a texel only while its raw height is in `13001..=17999`.
// re: Fun_087e735c 0x087e735c (`(ushort)(h + 0xcd37) < 4999`): above the sea (13000) and below the hills
const HEIGHT_MIN: u16 = 13_001;
const HEIGHT_MAX: u16 = 17_999;

/// `g_aContinentPrefix`: the hex code of the marsh row of a continent is the prefix alone (the
/// row with `ID` 0 of the continent in `TerrainDef.csv`; the codes of the other rows are
/// `prefix | ID`, see `coast::blend`).
// re: g_aContinentPrefix 0x0a30fd0c = {0x100000, 0x110000, 0x120000, 0x130000, 0x140000}
const MARSH_ROW_ID: u32 = 0;
const CONTINENTS: u8 = 5;

/// The mask of the stamp as the original reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarshStamp {
    /// `surface[x * 128 + y]`: the order `Fun_087e735c` reads (`mask[x * height + y]`).
    surface: Vec<u8>,
}

impl MarshStamp {
    /// From the texels of the texture, row by row (`128 * 128` bytes of L8).
    ///
    /// The original copies the texture row by row to `record + 1 + row * 128 + col` and reads the
    /// mask from `record + x * 128 + y`, so the mask is the texture read transposed and moved by
    /// one byte: `surface[k] = texels[k - 1]`, `surface[0] = 0`.
    // re: cvMarshSimSystem::Startup 0x087e6af2 (`pDst[1 + col] = row`), Fun_087e735c 0x087e735c (`mask[x * h + y]`)
    // TODO(RW-12): the one byte shift and the transposition are the reading of the binary; check on a capture
    pub fn from_texels(texels: &[u8]) -> Option<Self> {
        let n = (STAMP_SIZE * STAMP_SIZE) as usize;
        if texels.len() < n {
            return None;
        }
        let mut surface = vec![0u8; n];
        surface[1..].copy_from_slice(&texels[..n - 1]);
        Some(Self { surface })
    }

    /// From the texture of the pack: 128 x 128, one byte a texel (`R8`, the pack's L8).
    // re: cvMarshSimSystem::Startup (format 0x32 = L8, "Incorrect marsh stamp texture format")
    pub fn from_blob(blob: &TextureBlob) -> Option<Self> {
        if blob.format != TextureFormat::R8
            || blob.dim != TexDim::D2
            || blob.width != STAMP_SIZE as u32
            || blob.height != STAMP_SIZE as u32
        {
            return None;
        }
        Self::from_texels(blob.mips.first()?)
    }

    /// A disc of full mask (radius 40 texels) for the tests and the runs without the pack.
    pub fn synthetic() -> Self {
        let mut texels = vec![0u8; (STAMP_SIZE * STAMP_SIZE) as usize];
        for row in 0..STAMP_SIZE {
            for col in 0..STAMP_SIZE {
                let (dx, dy) = (col - STAMP_HALF, row - STAMP_HALF);
                if dx * dx + dy * dy <= 40 * 40 {
                    texels[(row * STAMP_SIZE + col) as usize] = 255;
                }
            }
        }
        Self::from_texels(&texels).expect("a full square")
    }

    /// The mask at `(x, y)` of the surface, `0..128` on both.
    pub fn at(&self, x: i32, y: i32) -> u8 {
        if (0..STAMP_SIZE).contains(&x) && (0..STAMP_SIZE).contains(&y) {
            self.surface[(x * STAMP_SIZE + y) as usize]
        } else {
            0
        }
    }
}

/// A marsh hex ready to stamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MarshHex {
    /// The corner of the surface in the texel plane (`tex - 64`).
    u0: i32,
    v0: i32,
    /// The field id: the row of the marsh of the continent in `TerrainDef`; none if it has none.
    row: Option<u8>,
}

/// The index in the table (the id the field stores) of the marsh row of a continent.
// re: BlitMaskedIDs 0x087e70a6 (the first `m_aHexCode[i] == g_aContinentPrefix[c]`, 0xff if none)
pub fn marsh_row(defs: &TerrainDefs, continent: u8) -> Option<u8> {
    if continent >= CONTINENTS {
        return None;
    }
    defs.rows()
        .iter()
        .position(|r| r.continent == continent && r.id == MARSH_ROW_ID)
        .and_then(|i| u8::try_from(i).ok())
}

/// The stage `Marsh`. Join it with `TerrainBuild::install`.
pub struct MarshStage {
    stamp: Arc<MarshStamp>,
    hexes: Vec<MarshHex>,
    /// The frame and the width the global part saw, for `invalidate`.
    seen: Option<(TerrainFrame, u16)>,
}

/// The stage with the stamp of the pack.
pub fn stage(stamp: Arc<MarshStamp>) -> Box<dyn BuildStage> {
    Box::new(MarshStage {
        stamp,
        hexes: Vec::new(),
        seen: None,
    })
}

fn corner(frame: &TerrainFrame, x: i32, y: i32) -> (i32, i32) {
    let t = frame.hex_to_tex(x, y);
    // `(int)kTexel.x - 64` truncates toward zero.
    (t.x as i32 - STAMP_HALF, t.y as i32 - STAMP_HALF)
}

impl BuildStage for MarshStage {
    fn id(&self) -> StageId {
        StageId::Marsh
    }

    /// `cvMarshSimSystem::StartupGame`: the marshes in the order of the queue, which we take as
    /// the order of the map. No draw of `rand`.
    // re: cvMarshSimSystem::StartupGame 0x087e6f30, OnMarshAdded 0x087e6f7e
    // TODO(RW-12): the order the original queues the marshes in (row by row `y * width + x` assumed)
    fn global(&mut self, cx: &mut StageCx<'_>) -> Result<(), BuildError> {
        self.seen = Some((*cx.frame, cx.basis.width));
        self.hexes.clear();
        for (i, plot) in cx.basis.plots.iter().enumerate() {
            if plot.feature != BasisFeature::Marsh {
                continue;
            }
            let (x, y) = cx.basis.xy(i as u32);
            let (u0, v0) = corner(cx.frame, x, y);
            // TODO: a continent without the marsh row: the original paints 0xff; we paint nothing
            let row = marsh_row(cx.library.defs(), plot.continent);
            self.hexes.push(MarshHex { u0, v0, row });
        }
        Ok(())
    }

    fn raster_scope(&self) -> RasterScope {
        RasterScope::PerCell
    }

    /// The part of every stamp inside the cell. A texel is painted where the mask is set and the
    /// height is in `13001..=17999`; the heights are not changed. The stamps go in the order of
    /// the hexes, and a stamp that crosses the seam of a wrapped map comes in again on the other
    /// side (`GetWrappedPosition`).
    // re: Fun_087e735c 0x087e735c, cvTerrainSampler::GetWrappedPosition, Fun_087e75d2 (the id write)
    fn raster(&self, cell: CellRect, window: &mut FieldWindow<'_>, cx: &RasterCx<'_>) {
        let (width, _) = cx.frame.texel_size();
        let width = width as i32;
        let turns: &[i32] = if cx.frame.wrap_x() { &[-1, 0, 1] } else { &[0] };
        for hex in &self.hexes {
            let Some(row) = hex.row else { continue };
            for &k in turns {
                // The stamp seen from the cell: its corner moved by whole turns of the map.
                let u0 = hex.u0 - k * width;
                let (ua, ub) = (cell.u0.max(u0), cell.u1.min(u0 + STAMP_SIZE));
                let (va, vb) = (cell.v0.max(hex.v0), cell.v1.min(hex.v0 + STAMP_SIZE));
                for v in va..vb {
                    for u in ua..ub {
                        if self.stamp.at(u - u0, v - hex.v0) == 0 {
                            continue;
                        }
                        if (HEIGHT_MIN..=HEIGHT_MAX).contains(&window.height(u, v)) {
                            window.set_id(u, v, row);
                        }
                    }
                }
            }
        }
    }

    /// The stamp of a changed hex covers 128 x 128 texels around it.
    fn invalidate(&mut self, changed: &[u32]) -> Vec<CellRect> {
        let Some((frame, width)) = self.seen else {
            return Vec::new();
        };
        changed
            .iter()
            .map(|&i| {
                let (x, y) = ((i % u32::from(width)) as i32, (i / u32::from(width)) as i32);
                let (u0, v0) = corner(&frame, x, y);
                CellRect::new(u0, v0, u0 + STAMP_SIZE, v0 + STAMP_SIZE)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::{Relief, TerrainKind};
    use crate::world::terrain::basis::{PlotBasis, TerrainBasis};
    use crate::world::terrain::build::BuildProducts;
    use crate::world::terrain::build::rand::MsvcRand;
    use crate::world::terrain::field::TerrainField;
    use crate::world::terrain::library::{LAND_RAW, TerrainLibrary};
    use crate::world::terrain::stencil::TerrainStencil;

    const OLD_ID: u8 = 7;

    struct Fixture {
        basis: TerrainBasis,
        library: TerrainLibrary,
        frame: TerrainFrame,
        field: TerrainField,
        stencil: TerrainStencil,
    }

    fn fixture(marsh: &[(i32, i32)], continent: u8, wrap: bool) -> Fixture {
        let mut basis = TerrainBasis::uniform(
            6,
            6,
            wrap,
            42,
            PlotBasis {
                relief: Relief::Flat,
                terrain: TerrainKind::Grass,
                river_flow: PlotBasis::NO_FLOW,
                continent,
                ..PlotBasis::default()
            },
        );
        for &(x, y) in marsh {
            let i = basis.index(x, y).unwrap();
            basis.plots[i].feature = BasisFeature::Marsh;
        }
        let frame = TerrainFrame::new(6, 6, wrap);
        let (w, h) = frame.texel_size();
        let mut field = TerrainField::with_texels(w, h);
        for v in 0..h as i32 {
            for u in 0..w as i32 {
                field.set(u, v, LAND_RAW, OLD_ID);
            }
        }
        Fixture {
            basis,
            library: TerrainLibrary::synthetic(),
            frame,
            field,
            stencil: TerrainStencil::new(w, h),
        }
    }

    impl Fixture {
        fn global(&mut self, stage: &mut Box<dyn BuildStage>, rand: &mut MsvcRand) {
            let mut products = BuildProducts::default();
            let mut cx = StageCx {
                basis: &self.basis,
                library: &self.library,
                frame: &self.frame,
                rand,
                products: &mut products,
                field: &mut self.field,
                stencil: &mut self.stencil,
                prior: None,
            };
            stage.global(&mut cx).unwrap();
        }

        fn raster(&mut self, stage: &dyn BuildStage, cells: &[(i32, i32)]) {
            let products = BuildProducts::default();
            let cx = RasterCx {
                basis: &self.basis,
                library: &self.library,
                frame: &self.frame,
                products: &products,
            };
            for &(cx_, cy) in cells {
                let rect = CellRect::cell(cx_, cy);
                let mut window = FieldWindow::new(&mut self.field, &mut self.stencil, rect);
                stage.raster(rect, &mut window, &cx);
            }
        }

        fn all_cells(&self) -> Vec<(i32, i32)> {
            (0..6).flat_map(|y| (0..6).map(move |x| (x, y))).collect()
        }
    }

    fn run(f: &mut Fixture, cells: &[(i32, i32)]) -> Box<dyn BuildStage> {
        let mut st = stage(Arc::new(MarshStamp::synthetic()));
        let mut rand = MsvcRand::new(1);
        f.global(&mut st, &mut rand);
        f.raster(st.as_ref(), cells);
        st
    }

    fn marsh_id(f: &Fixture, continent: u8) -> u8 {
        marsh_row(f.library.defs(), continent).unwrap()
    }

    #[test]
    fn the_marsh_row_is_the_row_with_id_0_of_the_continent() {
        let lib = TerrainLibrary::synthetic();
        // The synthetic table has four rows (ids 0..3) for each of the 5 continents.
        for c in 0..5u8 {
            assert_eq!(marsh_row(lib.defs(), c), Some(c * 4));
        }
        assert_eq!(marsh_row(lib.defs(), 5), None);
    }

    #[test]
    fn the_stamp_reads_the_texture_moved_by_one_byte_and_transposed() {
        let mut texels = vec![0u8; 128 * 128];
        texels[3 * 128 + 5] = 9; // row 3, column 5
        let s = MarshStamp::from_texels(&texels).unwrap();
        // k = 3 * 128 + 5 + 1: x = 3, y = 6
        assert_eq!(s.at(3, 6), 9);
        assert_eq!(s.at(5, 3), 0);
        assert_eq!(s.at(0, 0), 0);
    }

    #[test]
    fn from_blob_wants_a_128_square_r8() {
        let good = TextureBlob {
            format: TextureFormat::R8,
            dim: TexDim::D2,
            srgb: false,
            width: 128,
            height: 128,
            depth: 1,
            mips: vec![vec![1; 128 * 128]],
        };
        assert!(MarshStamp::from_blob(&good).is_some());
        let mut bad = good.clone();
        bad.format = TextureFormat::Rgba8;
        assert!(MarshStamp::from_blob(&bad).is_none());
        let mut small = good;
        small.width = 64;
        assert!(MarshStamp::from_blob(&small).is_none());
    }

    #[test]
    fn one_marsh_paints_ids_under_the_mask_and_leaves_the_rest() {
        let mut f = fixture(&[(3, 3)], 2, false);
        let before = f.field.clone();
        let stamp = MarshStamp::synthetic();
        let all = f.all_cells();
        run(&mut f, &all);
        let (u0, v0) = corner(&f.frame, 3, 3);
        let id = marsh_id(&f, 2);
        assert_ne!(id, OLD_ID);
        let (w, h) = f.frame.texel_size();
        let mut painted = 0;
        for v in 0..h as i32 {
            for u in 0..w as i32 {
                let inside = (u0..u0 + 128).contains(&u) && (v0..v0 + 128).contains(&v);
                let want = if inside && stamp.at(u - u0, v - v0) != 0 {
                    painted += 1;
                    id
                } else {
                    OLD_ID
                };
                assert_eq!(f.field.get_id(u, v), want, "id at {u},{v}");
                assert_eq!(f.field.get_height(u, v), before.get_height(u, v));
            }
        }
        assert!(painted > 4000, "the disc is on the map: {painted}");
    }

    #[test]
    fn the_heights_of_the_filter_are_13001_to_17999() {
        let mut f = fixture(&[(3, 3)], 1, false);
        let (u0, v0) = corner(&f.frame, 3, 3);
        let at = |dx: i32, dy: i32| (u0 + dx, v0 + dy);
        // Texels of the mask next to the centre: x = 64, y = 60..64 (the shift moves the disc by one byte).
        for (i, h) in [13_000u16, 13_001, 17_999, 18_000].into_iter().enumerate() {
            let (u, v) = at(64, 60 + i as i32);
            f.field.set_height(u, v, h);
        }
        let all = f.all_cells();
        run(&mut f, &all);
        let id = marsh_id(&f, 1);
        let got: Vec<u8> = (0..4)
            .map(|i| {
                let (u, v) = at(64, 60 + i);
                f.field.get_id(u, v)
            })
            .collect();
        assert_eq!(got, [OLD_ID, id, id, OLD_ID]);
    }

    #[test]
    fn two_marshes_overlap_and_the_later_hex_wins() {
        let mut f = fixture(&[(3, 2), (4, 2)], 1, false);
        // The second hex has another continent: another row.
        let i = f.basis.index(4, 2).unwrap();
        f.basis.plots[i].continent = 3;
        let all = f.all_cells();
        run(&mut f, &all);
        let (a, b) = (marsh_id(&f, 1), marsh_id(&f, 3));
        let (ua, va) = corner(&f.frame, 3, 2);
        let (ub, _) = corner(&f.frame, 4, 2);
        assert_eq!(ub, ua + 64);
        // Only the first reaches here.
        assert_eq!(f.field.get_id(ua + 40, va + 64), a);
        // Inside both discs: the second (the later hex) paints over the first.
        assert_eq!(f.field.get_id(ua + 90, va + 64), b);
    }

    #[test]
    fn the_order_of_the_cells_does_not_change_the_field() {
        let mut a = fixture(&[(1, 1), (2, 1), (4, 4)], 4, false);
        let mut b = fixture(&[(1, 1), (2, 1), (4, 4)], 4, false);
        let cells = a.all_cells();
        let mut rev = cells.clone();
        rev.reverse();
        run(&mut a, &cells);
        run(&mut b, &rev);
        assert!(a.field.same_content(&b.field));
        assert_ne!(a.field.digest(), fixture(&[], 4, false).field.digest());
    }

    #[test]
    fn the_stage_draws_nothing_from_the_stream() {
        let mut f = fixture(&[(2, 2)], 1, false);
        let mut st = stage(Arc::new(MarshStamp::synthetic()));
        let mut rand = MsvcRand::new(99);
        let state = rand.state();
        f.global(&mut st, &mut rand);
        assert_eq!(rand.calls(), 0);
        assert_eq!(rand.state(), state);
    }

    #[test]
    fn a_stamp_over_the_seam_of_a_wrapped_map_comes_in_on_the_other_side() {
        let mut f = fixture(&[(0, 2)], 1, true);
        let all = f.all_cells();
        run(&mut f, &all);
        let (w, _) = f.frame.texel_size();
        let (_, v0) = corner(&f.frame, 0, 2);
        let id = marsh_id(&f, 1);
        // The disc of an even row is centred at u = 32: it reaches the west edge and the east edge.
        assert_eq!(f.field.get_id(0, v0 + 64), id);
        assert_eq!(f.field.get_id(w as i32 - 1, v0 + 64), id);
        // Without the wrap the east edge stays.
        let mut g = fixture(&[(0, 2)], 1, false);
        let all = g.all_cells();
        run(&mut g, &all);
        assert_eq!(g.field.get_id(w as i32 - 1, v0 + 64), OLD_ID);
    }

    #[test]
    fn a_continent_without_the_marsh_row_paints_nothing() {
        let mut f = fixture(&[(3, 3)], 9, false);
        let before = f.field.digest();
        let all = f.all_cells();
        run(&mut f, &all);
        assert_eq!(f.field.digest(), before);
    }

    #[test]
    fn invalidate_names_the_stamp_rect_of_a_changed_hex() {
        let mut f = fixture(&[(3, 3)], 1, false);
        let mut st = stage(Arc::new(MarshStamp::synthetic()));
        let mut rand = MsvcRand::new(1);
        f.global(&mut st, &mut rand);
        let i = f.basis.index(3, 3).unwrap() as u32;
        let rects = st.invalidate(&[i]);
        let (u0, v0) = corner(&f.frame, 3, 3);
        assert_eq!(rects, [CellRect::new(u0, v0, u0 + 128, v0 + 128)]);
    }
}
