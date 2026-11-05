//! The matcher of the pieces: the port of `HexPatternMatcher`.
//! `ComputePatternPositions` runs `FindLinearPacking` once per packing pass, from the highest pass
//! of the pieces down to 0. A pass shuffles the anchor hexes of the grid (the grid and a border of
//! two), and at every anchor shuffles the order of the pieces and of the four transforms, then
//! places every piece of the pass, in every transform, that fits the hexes and covers no covered
//! hex. All the shuffles are `vc9_random_shuffle` on the one stream of `rand` of
//! `TerrainSystem::Init` (D-3).
//!
//! The hex matrices are the integer 4 x 4 matrices of the original (`HEXMatrix_i`, row vectors,
//! `v * M`), so a placement is the same product of the same numbers.

use super::rand::{MsvcRand, vc9_random_shuffle};

/// A cube hex `(x, y, z)`, `x + y + z = 0` (`HEXVec_i`).
pub type HexVec = [i32; 3];

/// `HEXMatrix_i`: 16 integers, row-major, applied to a row vector: `out[i] = m[i] x + m[4 + i] y
/// + m[8 + i] z + m[12 + i]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HexMat(pub [i32; 16]);

/// The table of `HEXMat_iRotate`: `rotation(r)` reads it at `(r + 8) % 6` and `(r + 10) % 6`.
// re: HEXMat_iRotate 0x0868e2af, table at 0x0914ba08 (read from the binary)
const ROTATION_TABLE: [i32; 6] = [0, 1, 1, 0, -1, -1];

/// `HEXMatrix_i::REFLECTIONS`: the 12 matrices `HEXTransform_i` field `+0x10` indexes.
// re: HEXMatrix_i::REFLECTIONS 0x0a3c924c, filled by _INIT_148 0x0848dbad (read from the binary)
pub const REFLECTIONS: [HexMat; 12] = [
    HexMat([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]),
    HexMat([-1, 0, 0, 0, 0, 0, -1, 0, 0, -1, 0, 0, 0, 0, 0, 1]),
    HexMat([0, 0, -1, 0, 0, -1, 0, 0, -1, 0, 0, 0, 0, 0, 0, 1]),
    HexMat([0, -1, 0, 0, -1, 0, 0, 0, 0, 0, -1, 0, 0, 0, 0, 1]),
    HexMat([0, 1, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]),
    HexMat([1, 0, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0, 0, 0, 1]),
    HexMat([0, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 1]),
    HexMat([0, -1, 0, 0, 0, 0, -1, 0, -1, 0, 0, 0, 0, 0, 0, 1]),
    HexMat([0, 0, 1, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1]),
    HexMat([-1, 0, 0, 0, 0, -1, 0, 0, 0, 0, -1, 0, 0, 0, 0, 1]),
    HexMat([0, 1, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0, 1]),
    HexMat([0, 0, -1, 0, -1, 0, 0, 0, 0, -1, 0, 0, 0, 0, 0, 1]),
];

impl HexMat {
    pub const IDENTITY: Self = Self([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]);

    /// `v * M` without the fourth column.
    // re: Fun_086616f0 0x086616f0
    pub fn apply(&self, v: HexVec) -> HexVec {
        let m = &self.0;
        let mut out = [0; 3];
        for (i, o) in out.iter_mut().enumerate() {
            *o = m[i]
                .wrapping_mul(v[0])
                .wrapping_add(m[4 + i].wrapping_mul(v[1]))
                .wrapping_add(m[8 + i].wrapping_mul(v[2]))
                .wrapping_add(m[12 + i]);
        }
        out
    }

    /// `self * b`, row-major: `self` applies first.
    // re: Fun_0868e43a 0x0868e43a
    pub fn mul(&self, b: &HexMat) -> HexMat {
        let mut out = [0i32; 16];
        for r in 0..4 {
            for c in 0..4 {
                let mut s = 0i32;
                for k in 0..4 {
                    s = s.wrapping_add(self.0[r * 4 + k].wrapping_mul(b.0[k * 4 + c]));
                }
                out[r * 4 + c] = s;
            }
        }
        HexMat(out)
    }

    /// `HEXMat_iTranslation`.
    // re: HEXMat_iTranslation 0x0868e39c
    pub fn translation(t: HexVec) -> HexMat {
        let mut m = Self::IDENTITY.0;
        m[12] = t[0];
        m[13] = t[1];
        m[14] = t[2];
        HexMat(m)
    }

    /// `HEXMat_iRotate(r)`: on the plane `x + y + z = 0` a turn by `r` sixths (0 is the identity
    /// there; off the plane it is not, as in the original).
    // re: HEXMat_iRotate 0x0868e2af
    pub fn rotation(r: i32) -> HexMat {
        // C remainders: `r % 6` keeps the sign of `r`; `r % 6 + 8` and `+ 10` are positive.
        let r = r % 6;
        let a = -ROTATION_TABLE[((r + 8) % 6) as usize];
        let b = ROTATION_TABLE[((r + 10) % 6) as usize];
        let mut m = [0i32; 16];
        m[1] = a;
        m[2] = b;
        m[4] = b;
        m[6] = a;
        m[8] = a;
        m[9] = b;
        m[15] = 1;
        HexMat(m)
    }
}

/// `HEXTransform_i`: a translation, a turn and a reflection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HexTransform {
    pub translation: HexVec,
    pub rotation: i32,
    /// Index into [`REFLECTIONS`].
    pub reflection: usize,
}

impl HexTransform {
    pub const IDENTITY: Self = Self {
        translation: [0, 0, 0],
        rotation: 0,
        reflection: 0,
    };

    /// `HEXTransform_i::matrix` (`HEXMat_iForwardTransform`): reflection, turn back, then the
    /// translation back.
    // re: HEXMat_iForwardTransform 0x0868d6e8
    pub fn matrix(&self) -> HexMat {
        let t = self.translation;
        HexMat::IDENTITY
            .mul(&REFLECTIONS[self.reflection])
            .mul(&HexMat::rotation(-self.rotation))
            .mul(&HexMat::translation([-t[0], -t[1], -t[2]]))
    }

    /// `HEXTransform_i::inv_matrix` (`HEXMat_iInverseTransform`): translation, turn, reflection.
    // re: HEXMat_iInverseTransform 0x0868da00
    pub fn inv_matrix(&self) -> HexMat {
        HexMat::IDENTITY
            .mul(&HexMat::translation(self.translation))
            .mul(&HexMat::rotation(self.rotation))
            .mul(&REFLECTIONS[self.reflection])
    }
}

/// The transforms the matcher tries, in the order its constructor pushes them: the inverse
/// matrices of these four `HEXTransform_i`.
// re: Fun_0891b3fa 0x0891b3fa (HexPatternMatcher constructor)
pub const MATCHER_TRANSFORMS: [HexTransform; 4] = [
    HexTransform::IDENTITY,
    HexTransform {
        translation: [1, -3, 2],
        rotation: 0,
        reflection: 2,
    },
    HexTransform {
        translation: [-4, 0, 4],
        rotation: 0,
        reflection: 6,
    },
    HexTransform {
        translation: [-2, -3, 5],
        rotation: 0,
        reflection: 9,
    },
];

/// `SquareGridPolicy`: world hex to grid cube coordinates (`forward`) and back, and the size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SquareGrid {
    pub forward: HexMat,
    pub inverse: HexMat,
    pub width: i32,
    pub height: i32,
}

impl SquareGrid {
    /// `Fun_08661752`: a grid of a size and a transform.
    // re: Fun_08661752 0x08661752
    pub fn new(width: i32, height: i32, transform: &HexTransform) -> Self {
        Self {
            forward: transform.matrix(),
            inverse: transform.inv_matrix(),
            width,
            height,
        }
    }

    /// Offset cell `(x, y)` (a column `x`, a row `y`) of a cube hex of the grid.
    pub fn cell_of(hex: HexVec) -> (i32, i32) {
        (hex[0] + (hex[1] >> 1), hex[1])
    }

    /// The cube hex of the offset cell `(x, y)`.
    pub fn hex_of(x: i32, y: i32) -> HexVec {
        let hx = x - (y >> 1);
        [hx, y, -hx - y]
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= 0 && x < self.width && y >= 0 && y < self.height
    }
}

/// `HexPatternCell`: the piece class bits of a hex (the bridge, `bridge.rs`), its continent
/// style, and whether a placed piece took it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PatternCell {
    pub flags: u32,
    pub continent: u8,
    pub covered: bool,
}

/// `DynamicHexGrid<HexPatternCell, SquareGridPolicy>`: cells column-major, `x * height + y`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatternGrid {
    pub policy: SquareGrid,
    pub cells: Vec<PatternCell>,
}

impl PatternGrid {
    /// A grid of `fill`.
    // re: Fun_0865f122 0x0865f122
    pub fn new(policy: SquareGrid, fill: PatternCell) -> Self {
        let n = (policy.width.max(0) * policy.height.max(0)) as usize;
        Self {
            policy,
            cells: vec![fill; n],
        }
    }

    pub fn index(&self, x: i32, y: i32) -> Option<usize> {
        self.policy
            .contains(x, y)
            .then(|| (x * self.policy.height + y) as usize)
    }

    pub fn cell(&self, x: i32, y: i32) -> Option<&PatternCell> {
        self.index(x, y).map(|i| &self.cells[i])
    }

    pub fn cell_mut(&mut self, x: i32, y: i32) -> Option<&mut PatternCell> {
        self.index(x, y).map(|i| &mut self.cells[i])
    }
}

/// `TerrainPieceCell`: a hex of the 10 x 10 grid of a piece.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PieceCell {
    pub hex_code: u32,
    pub continent: u8,
    pub valid: bool,
}

impl PieceCell {
    /// A cell the CSV leaves out.
    // re: cvCompressedTerrainPiecesLibrary::LoadTerrainPiece 0x0891ee4a (TerrainData.cpp:407-414)
    pub const EMPTY: Self = Self {
        hex_code: 0xffff_ffff,
        continent: 0,
        valid: false,
    };

    /// A cell from the number of the CSV of a piece of the style `continent`. A negative number
    /// is a cell the piece must fit but does not take (`valid` false, continent 0); the code is
    /// the low 31 bits, 0 is "none" (`0xffffffff`). Version 3 codes go through the legacy table.
    // re: LoadTerrainPiece 0x0891ee4a (TerrainData.cpp:435-461)
    pub fn from_csv(value: i32, continent: u8, csv_version: u8) -> Self {
        let mut code = (value as u32) & 0x7fff_ffff;
        if code == 0 {
            code = 0xffff_ffff;
        }
        if csv_version == 3 {
            code = legacy_hex_code(code);
        }
        Self {
            hex_code: code,
            continent: if value < 0 { 0 } else { continent },
            valid: value >= 0,
        }
    }

    /// `Fun_08a478b6`: the piece cell may lie on the grid cell. A zero on either side matches;
    /// otherwise the masks share a bit and the continent of the piece cell is 0, the one of the
    /// grid cell, or the grid cell has 0.
    // re: Fun_08a478b6 0x08a478b6
    pub fn fits(&self, grid: &PatternCell) -> bool {
        let continent_ok =
            self.continent == 0 || self.continent == grid.continent || grid.continent == 0;
        (grid.flags & self.hex_code != 0 && continent_ok) || grid.flags == 0 || self.hex_code == 0
    }
}

/// `g_aLegacyHexCodeValues`.
// re: g_aLegacyHexCodeValues 0x09215020 (read from the binary)
const LEGACY_VALUES: [u32; 31] = [
    0x1,
    0x2,
    0x4,
    0x8,
    0x10,
    0x20,
    0x40,
    0x80,
    0x100,
    0x200,
    0x400,
    0x800,
    0x1000,
    0x2000,
    0x4000,
    0x8000,
    0x1_0000,
    0x2_0000,
    0x4_0000,
    0x8_0000,
    0x10_0000,
    0x20_0000,
    0x40_0000,
    0x80_0000,
    0x100_0000,
    0x200_0000,
    0x400_0000,
    0x800_0000,
    0xffff_ffff,
    0xf3ef_9fff,
    0x10_6000,
];
/// `g_aLegacyHexCodeKeys`: the codes of the version 3 CSV the table recodes.
// re: g_aLegacyHexCodeKeys 0x0921509c (read from the binary)
const LEGACY_KEYS: [u32; 23] = [
    0x1, 0x2, 0x4, 0x8, 0x10, 0x20, 0x40, 0x80, 0x100, 0x200, 0x400, 0x800, 0x1000, 0x2000, 0x4000,
    0x8000, 0x1_0000, 0x2_0000, 0x4_0000, 0x8_0000, 0xf_ffff, 0xf_e7ff, 0xc_1800,
];
/// `g_aLegacyHexCodeIndex`: the value of each key.
// re: g_aLegacyHexCodeIndex 0x092150f8 (read from the binary)
const LEGACY_INDEX: [usize; 23] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 13, 14, 15, 16, 17, 18, 19, 20, 21, 28, 29, 30,
];

/// The code of a version 3 CSV cell: `0xffffff` is none, `0x1800` is coast and the reef, a key of
/// the legacy table is recoded, the rest stays.
// re: LoadTerrainPiece 0x0891ee4a (TerrainData.cpp:442-457)
fn legacy_hex_code(code: u32) -> u32 {
    if code == 0x00ff_ffff {
        return 0xffff_ffff;
    }
    if code == 0x1800 {
        return 0x0010_6000;
    }
    LEGACY_KEYS
        .iter()
        .position(|&k| k == code)
        .map_or(code, |i| LEGACY_VALUES[LEGACY_INDEX[i]])
}

/// Cells of a piece grid: `[column * 10 + row]`.
pub const PIECE_GRID: usize = 10;

/// `PatternPiece`: a piece reduced to its hexes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatternPiece {
    pub rows: i32,
    pub columns: i32,
    /// `[column * 10 + row]`.
    pub cells: [PieceCell; PIECE_GRID * PIECE_GRID],
    /// `m_uiPackingPass` (`field_3c8`): the piece is placed in this pass only.
    pub pass: u32,
    /// What the blit looks the piece up by (`m_iLibraryIndex`): the index in the caller's list.
    pub library_index: u32,
    /// `(row, column)` of every cell with a code or valid, row-major over the rows.
    pub used: Vec<(u8, u8)>,
}

impl PatternPiece {
    /// `Fun_0891b67a`: the used cells of a piece, rows outer, columns inner; the size is cut to
    /// 10 x 10.
    // re: Fun_0891b67a 0x0891b67a (cvTerrainBlend.cpp:591-626)
    pub fn new(
        rows: i32,
        columns: i32,
        cells: [PieceCell; PIECE_GRID * PIECE_GRID],
        pass: u32,
        library_index: u32,
    ) -> Self {
        let rows = rows.clamp(0, PIECE_GRID as i32);
        let columns = columns.clamp(0, PIECE_GRID as i32);
        let mut used = Vec::new();
        for row in 0..rows {
            for column in 0..columns {
                let c = &cells[(column * 10 + row) as usize];
                if c.hex_code != 0xffff_ffff || c.valid {
                    used.push((row as u8, column as u8));
                }
            }
        }
        Self {
            rows,
            columns,
            cells,
            pass,
            library_index,
            used,
        }
    }

    /// A piece of the asset library (`assets-b2`): the CSV grid of `grid.0` columns and `grid.1`
    /// rows in a 10 x 10 grid, the pass from the last CSV row (`flags + 1`, 2 if the row has no
    /// number). `None` for a piece the original keeps out of the matcher: the second number of
    /// that row even (the blend flag 4 cleared).
    // re: LoadTerrainPiece 0x0891ee4a (TerrainData.cpp:513-520); cvTerrainBlend::LoadTerrainPieces
    // 0x08918194 (only flag 4)
    pub fn from_asset(
        piece: &asset_format::TerrainPiece,
        continent: u8,
        library_index: u32,
    ) -> Option<Self> {
        if piece.csv_extra.first().is_some_and(|e| e & 1 == 0) {
            return None;
        }
        let mut cells = [PieceCell::EMPTY; PIECE_GRID * PIECE_GRID];
        let (cols, rows) = (usize::from(piece.grid.0), usize::from(piece.grid.1));
        for row in 0..rows.min(PIECE_GRID) {
            for col in 0..cols.min(PIECE_GRID) {
                if let Some(&v) = piece.hex_codes.get(row * cols + col) {
                    cells[col * 10 + row] =
                        PieceCell::from_csv(v as i32, continent, piece.csv_version);
                }
            }
        }
        let pass = piece.flags.wrapping_add(1);
        Some(Self::new(10, 10, cells, pass, library_index))
    }
}

/// Version of `TerrainPiecesLibrary.fxsb`.
// re: kPieceBinaryVersion (TerrainData.cpp:32)
pub const FXSB_VERSION: u32 = 0x14;
/// Size of a `TerrainPiece` in the file.
// re: kPieceStride (TerrainData.cpp:33)
pub const FXSB_PIECE_STRIDE: usize = 0x56c;

/// The pieces of `Resource/Common/TerrainPiecesLibrary.fxsb`, in its order, as the game loads
/// them (`LoadFromBinary`; flag 4 only): the reference library of the matcher for the checks of
/// D-3. The file holds 299 pieces of passes 1..=4, all with flag 4.
// re: cvCompressedTerrainPiecesLibrary::LoadFromBinary 0x0891eb7a, SaveToBinary 0x0891ed30;
// TerrainPiece layout (TerrainData.h:133-160)
pub fn pieces_from_fxsb(bytes: &[u8]) -> Result<Vec<PatternPiece>, String> {
    let u32_at = |o: usize| -> Result<u32, String> {
        bytes
            .get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| format!("fxsb ends at {o}"))
    };
    if u32_at(0)? != FXSB_VERSION {
        return Err("not a version 0x14 fxsb".into());
    }
    let mut o = 4usize;
    // Row offsets (u32), byte runs (u8), height runs (u16), then the pieces.
    for elem in [4usize, 1, 2] {
        let n = u32_at(o)? as usize;
        o += 4 + n * elem;
    }
    let count = u32_at(o)? as usize;
    o += 4;
    if bytes.len() < o + count * FXSB_PIECE_STRIDE {
        return Err("fxsb piece table cut short".into());
    }
    let mut out = Vec::new();
    for i in 0..count {
        let b = o + i * FXSB_PIECE_STRIDE;
        if bytes[b + 0x3d0] & 4 == 0 {
            continue;
        }
        let rows = u32_at(b + 0x94)? as i32;
        let columns = u32_at(b + 0x98)? as i32;
        let mut cells = [PieceCell::EMPTY; PIECE_GRID * PIECE_GRID];
        for (k, c) in cells.iter_mut().enumerate() {
            let at = b + 0x9c + 8 * k;
            *c = PieceCell {
                hex_code: u32_at(at)?,
                continent: bytes[at + 4],
                valid: bytes[at + 5] != 0,
            };
        }
        let (cut_rows, cut_columns) = (u32_at(b + 0x3c0)? as i32, u32_at(b + 0x3c4)? as i32);
        let pass = u32_at(b + 0x3c8)?;
        let mut p = PatternPiece::new(rows, columns, cells, pass, i as u32);
        p.rows = p.rows.min(cut_rows);
        p.columns = p.columns.min(cut_columns);
        out.push(p);
    }
    Ok(out)
}

/// `HexPatternMatcher::PatternPosition`: a placed piece.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PatternPosition {
    /// Index into the pieces of the matcher.
    pub piece: u32,
    /// Index into [`MATCHER_TRANSFORMS`].
    pub transform: u32,
    /// The anchor hex (world hexes).
    pub hex: HexVec,
}

/// `HexPatternMatcher`.
#[derive(Clone, Debug)]
pub struct HexPatternMatcher {
    pieces: Vec<PatternPiece>,
    transforms: Vec<HexMat>,
    max_pass: u32,
    positions: Vec<PatternPosition>,
}

impl Default for HexPatternMatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl HexPatternMatcher {
    /// The matcher of `cvTerrainBlend`: no piece, the four transforms.
    // re: Fun_0891b3fa 0x0891b3fa
    pub fn new() -> Self {
        Self {
            pieces: Vec::new(),
            transforms: MATCHER_TRANSFORMS.iter().map(|t| t.inv_matrix()).collect(),
            max_pass: 0,
            positions: Vec::new(),
        }
    }

    /// Adds a piece in the order of the library (the order is part of the stream).
    pub fn add_piece(&mut self, piece: PatternPiece) {
        self.max_pass = self.max_pass.max(piece.pass);
        self.pieces.push(piece);
    }

    pub fn pieces(&self) -> &[PatternPiece] {
        &self.pieces
    }

    /// The matrix of transform `i` (`m_pTransforms[i]`), which the blit turns the corners by.
    pub fn transform_matrix(&self, i: usize) -> &HexMat {
        &self.transforms[i]
    }

    pub fn max_pass(&self) -> u32 {
        self.max_pass
    }

    /// What the last [`Self::compute_pattern_positions`] placed, in order.
    pub fn positions(&self) -> &[PatternPosition] {
        &self.positions
    }

    /// The transform of a position: piece hex `(row, column)` to grid hex is
    /// `hex_of(row, column) * transform * translation(anchor) * grid.forward`.
    pub fn placement(&self, position: &PatternPosition, grid: &SquareGrid) -> HexMat {
        self.transforms[position.transform as usize]
            .mul(&HexMat::translation(position.hex))
            .mul(&grid.forward)
    }

    /// The grid cells a position takes (its valid cells inside the grid).
    pub fn covered_cells(&self, position: &PatternPosition, grid: &SquareGrid) -> Vec<(i32, i32)> {
        let piece = &self.pieces[position.piece as usize];
        let placement = self.placement(position, grid);
        piece
            .used
            .iter()
            .filter(|&&(row, column)| {
                piece.cells[usize::from(column) * 10 + usize::from(row)].valid
            })
            .map(|&(row, column)| {
                SquareGrid::cell_of(
                    placement.apply(SquareGrid::hex_of(i32::from(row), i32::from(column))),
                )
            })
            .filter(|&(x, y)| grid.contains(x, y))
            .collect()
    }

    /// `ComputePatternPositions`: the passes from the highest down to 0.
    // re: HexPatternMatcher::ComputePatternPositions 0x08a46f58
    pub fn compute_pattern_positions(&mut self, grid: &mut PatternGrid, rng: &mut MsvcRand) {
        self.positions.clear();
        for pass in (0..=self.max_pass).rev() {
            self.find_linear_packing(grid, pass, rng);
        }
    }

    /// `FindLinearPacking`: one pass.
    // re: HexPatternMatcher::FindLinearPacking 0x08a46fd6
    pub fn find_linear_packing(&mut self, grid: &mut PatternGrid, pass: u32, rng: &mut MsvcRand) {
        let policy = grid.policy;
        let mut piece_order: Vec<u32> = (0..self.pieces.len() as u32).collect();
        let mut transform_order: Vec<u32> = (0..self.transforms.len() as u32).collect();

        // The anchors: every offset cell and a border of two, x outer, in world hexes.
        let mut candidates: Vec<HexVec> =
            Vec::with_capacity(((policy.width + 4) * (policy.height + 4)).max(0) as usize);
        for x in -2..policy.width + 2 {
            for y in -2..policy.height + 2 {
                let w = policy.inverse.apply(SquareGrid::hex_of(x, y));
                candidates.push([w[0], w[1], -(w[0] + w[1])]);
            }
        }
        vc9_random_shuffle(&mut candidates, rng);

        let mut covered: Vec<(i32, i32)> = Vec::new();
        for &candidate in &candidates {
            let translation = HexMat::translation(candidate);
            vc9_random_shuffle(&mut piece_order, rng);
            vc9_random_shuffle(&mut transform_order, rng);
            for &pi in &piece_order {
                for &ti in &transform_order {
                    let piece = &self.pieces[pi as usize];
                    if piece.pass != pass {
                        continue;
                    }
                    covered.clear();
                    let placement = self.transforms[ti as usize]
                        .mul(&translation)
                        .mul(&policy.forward);
                    if piece.used.is_empty() {
                        continue;
                    }
                    let mut fits = true;
                    for &(row, column) in &piece.used {
                        let (x, y) = SquareGrid::cell_of(
                            placement.apply(SquareGrid::hex_of(i32::from(row), i32::from(column))),
                        );
                        let Some(gi) = grid.index(x, y) else {
                            // Off the grid: not checked, not taken.
                            continue;
                        };
                        let pc = &piece.cells[usize::from(column) * 10 + usize::from(row)];
                        let gc = &grid.cells[gi];
                        if !pc.fits(gc) {
                            fits = false;
                            break;
                        }
                        if pc.valid {
                            covered.push((x, y));
                            if gc.covered {
                                fits = false;
                                break;
                            }
                        }
                    }
                    if !fits || covered.is_empty() {
                        continue;
                    }
                    self.positions.push(PatternPosition {
                        piece: pi,
                        transform: ti,
                        hex: candidate,
                    });
                    for &(x, y) in &covered {
                        if let Some(c) = grid.cell_mut(x, y) {
                            c.covered = true;
                        }
                    }
                }
            }
        }
    }

    /// The draws of `rand` [`Self::compute_pattern_positions`] takes on a grid of this size: per
    /// pass one shuffle of the anchors and per anchor one of the pieces and one of the transforms.
    pub fn draws(&self, width: i32, height: i32) -> u64 {
        let anchors = ((width + 4) * (height + 4)).max(0) as u64;
        let per_pass = shuffle_draws(anchors)
            + anchors
                * (shuffle_draws(self.pieces.len() as u64)
                    + shuffle_draws(self.transforms.len() as u64));
        per_pass * (u64::from(self.max_pass) + 1)
    }
}

/// Draws `vc9_random_shuffle` takes on `n` items: one per index from 2 to `n`, two from 32768.
pub fn shuffle_draws(n: u64) -> u64 {
    let one = n.saturating_sub(1).min(32_766);
    let two = n.saturating_sub(32_767);
    one + 2 * two
}

#[cfg(test)]
mod tests;
