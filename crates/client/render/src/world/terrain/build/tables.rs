//! The constant tables of the blit and of the pieces: `CellBlitJob.h:97-114`, `TerrainData.h:15-20`.

// re: g_aTTPostfix 0x0a30fcd8
pub const TT_POSTFIX: [u32; 13] = [2, 39, 38, 40, 48, 2, 39, 38, 40, 48, 16, 0, 0];

// re: g_aContinentPrefix 0x0a30fd0c
pub const CONTINENT_PREFIX: [u32; 5] = [
    0x0010_0000,
    0x0011_0000,
    0x0012_0000,
    0x0013_0000,
    0x0014_0000,
];

// re: g_mTextureMask 0x0a30fcd4
pub const TEXTURE_MASK: u32 = 0x00ff_0000;

// re: g_pHexColorCodes 0x0a30fd20
pub const HEX_COLOR_CODES: [u32; 31] = [
    0x0060_f030,
    0x00e0_f030,
    0x00d0_7040,
    0x00b0_a000,
    0x00ff_ffff,
    0x0040_c020,
    0x00b0_c020,
    0x00b0_5030,
    0x0080_7000,
    0x00a0_a0a0,
    0x0090_a0b0,
    0x00ff_ffff,
    0x00ff_ffff,
    0x00f0_c000,
    0x0020_2080,
    0x00ff_ffff,
    0x00ff_ffff,
    0x00ff_ffff,
    0x00ff_ffff,
    0x00ff_ffff,
    0x00ff_ffff,
    0x00ff_ffff,
    0x00ff_ffff,
    0x00ff_ffff,
    0x00ff_ffff,
    0x00ff_ffff,
    0x00ff_ffff,
    0x00ff_ffff,
    0x00ff_ffff,
    0x00ff_ffff,
    0x00ff_ffff,
];

// re: g_pTerrainPatternClassNames 0x0a30fe2c
pub const TERRAIN_PATTERN_CLASS_NAMES: [&str; 5] = ["Sys_Val", "Low", "Medium", "High", "Wonder"];

// re: g_pBaseTypeNames 0x0a30fd9c
pub const BASE_TYPE_NAMES: [&str; 31] = [
    "Grass",
    "Plains",
    "Desert",
    "Tundra",
    "Snow",
    "Grass Hill",
    "Plains Hill",
    "Desert Hill",
    "Tundra Hill",
    "Snow Hill",
    "Mountain",
    "Wonder_Kilimanjaro",
    "Wonder_Lake_Victoria",
    "Coast",
    "Ocean",
    "Wonder_Geyser",
    "Wonder_Crater",
    "Wonder_Potosi",
    "Wonder_Fuji",
    "Wonder_Mesa",
    "Wonder_Reef",
    "Wonder_Krakatoa",
    "Wonder_Uluru",
    "Wonder_Mt_Sinai",
    "Wonder_Mt_Kailash",
    "Wonder_Sri_Pada",
    "Wonder_Gibraltar",
    "Empty Water Wonder 1",
    "Generic",
    "Generic Land",
    "Generic Water",
];

// re: g_pContinentNames 0x0a30fe18
pub const CONTINENT_NAMES: [&str; 5] = ["Generic", "America", "Asia", "Africa", "Europe"];

// re: TEXTURE_PATHS 0x0a30ff78
pub const TEXTURE_PATHS: [&str; 6] = [
    "assets/Overlays/arrow.dds",
    "assets/Overlays/arrow_food.dds",
    "assets/Overlays/arrow_prod.dds",
    "assets/Overlays/arrow_popup.dds",
    "assets/Overlays/arrow_popup_food.dds",
    "assets/Overlays/arrow_popup_prod.dds",
];

// re: g_aLegacyHexCodeValues 0x09215020
pub const LEGACY_HEX_CODE_VALUES: [u32; 31] = [
    1,
    2,
    4,
    8,
    16,
    32,
    64,
    128,
    0x0000_0100,
    0x0000_0200,
    0x0000_0400,
    0x0000_0800,
    0x0000_1000,
    0x0000_2000,
    0x0000_4000,
    0x0000_8000,
    0x0001_0000,
    0x0002_0000,
    0x0004_0000,
    0x0008_0000,
    0x0010_0000,
    0x0020_0000,
    0x0040_0000,
    0x0080_0000,
    0x0100_0000,
    0x0200_0000,
    0x0400_0000,
    0x0800_0000,
    0xffff_ffff,
    0xf3ef_9fff,
    0x0010_6000,
];

// re: g_aLegacyHexCodeKeys 0x0921509c
pub const LEGACY_HEX_CODE_KEYS: [u32; 23] = [
    1,
    2,
    4,
    8,
    16,
    32,
    64,
    128,
    0x0000_0100,
    0x0000_0200,
    0x0000_0400,
    0x0000_0800,
    0x0000_1000,
    0x0000_2000,
    0x0000_4000,
    0x0000_8000,
    0x0001_0000,
    0x0002_0000,
    0x0004_0000,
    0x0008_0000,
    0x000f_ffff,
    0x000f_e7ff,
    0x000c_1800,
];

// re: g_aLegacyHexCodeIndex 0x092150f8
pub const LEGACY_HEX_CODE_INDEX: [usize; 23] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 13, 14, 15, 16, 17, 18, 19, 20, 21, 28, 29, 30,
];

/// `TerrainData::GetTerrainHexCodeFromType`.
// re: TerrainData::GetTerrainHexCodeFromType 0x0891dfa2
pub fn terrain_hex_code_from_type(terrain_type: usize, continent: usize) -> u32 {
    (TT_POSTFIX[terrain_type] & !TEXTURE_MASK) | (CONTINENT_PREFIX[continent] & TEXTURE_MASK)
}

/// `g_TerrainScale` (`m_vTerrainScale`): the world units of a texel along x and y, and the height
/// of raw 255 is `z * 255` (golden facts S1a ev 819, S2 ev 793).
pub const TERRAIN_SCALE: [f32; 3] = [1.0, 0.866, 0.2];

/// A 4 x 4 matrix of D3DX, row-major, applied to a row vector.
type Mat = [[f64; 4]; 4];

fn identity() -> Mat {
    let mut m = [[0.0; 4]; 4];
    for (i, r) in m.iter_mut().enumerate() {
        r[i] = 1.0;
    }
    m
}

fn mul(a: &Mat, b: &Mat) -> Mat {
    let mut o = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            o[i][j] = (0..4).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    o
}

fn scaling(x: f32, y: f32) -> Mat {
    let mut m = identity();
    m[0][0] = f64::from(x);
    m[1][1] = f64::from(y);
    m
}

fn translation(x: f64, y: f64, z: f64) -> Mat {
    let mut m = identity();
    m[3] = [x, y, z, 1.0];
    m
}

/// Gauss-Jordan inverse of a 4 x 4 matrix.
fn inverse(m: &Mat) -> Mat {
    let mut a = *m;
    let mut inv = identity();
    for c in 0..4 {
        let p = (c..4)
            .max_by(|&i, &j| a[i][c].abs().total_cmp(&a[j][c].abs()))
            .unwrap_or(c);
        a.swap(c, p);
        inv.swap(c, p);
        let d = a[c][c];
        for j in 0..4 {
            a[c][j] /= d;
            inv[c][j] /= d;
        }
        for i in 0..4 {
            if i != c {
                let f = a[i][c];
                for j in 0..4 {
                    a[i][j] -= f * a[c][j];
                    inv[i][j] -= f * inv[c][j];
                }
            }
        }
    }
    inv
}

/// `v * m` for a row vector of four.
fn transform(v: [f32; 4], m: &[[f32; 4]; 4]) -> [f32; 4] {
    let mut o = [0.0f32; 4];
    for (j, oj) in o.iter_mut().enumerate() {
        *oj = (0..4).map(|i| v[i] * m[i][j]).sum();
    }
    o
}

fn to_f32(m: &Mat) -> [[f32; 4]; 4] {
    let mut o = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            o[i][j] = m[i][j] as f32;
        }
    }
    o
}

/// The matrix `HEXMatNormalize` returns: `z = -x - y`.
// re: HEXMatNormalize 0x0868de49 (read from the disassembly)
fn normalize() -> Mat {
    let mut m = identity();
    m[0][2] = -1.0;
    m[1][2] = -1.0;
    m[2][2] = 0.0;
    m
}

/// Scale `(1, 0.866)`, turn of 0.5235988, scale `(0.0180427, 1/64)`, shear `m01 = -0.5`.
// re: WorldGlobals::SetupWorldGlobals 0x086725f9 (constants 0x3f5db22d, 0x3c93ce56, 0x3c800000)
fn base() -> Mat {
    let mut a = scaling(1.0, f32::from_bits(0x3f5d_b22d));
    let angle = f64::from(std::f32::consts::FRAC_PI_6);
    let (s, c) = (angle.sin(), angle.cos());
    let mut r = identity();
    r[0][0] = c;
    r[0][1] = s;
    r[1][0] = -s;
    r[1][1] = c;
    a = mul(&a, &r);
    a = mul(
        &a,
        &scaling(f32::from_bits(0x3c93_ce56), f32::from_bits(0x3c80_0000)),
    );
    let mut shear = identity();
    shear[0][1] = -0.5;
    mul(&a, &shear)
}

/// The world matrices `WorldGlobals::SetupWorldGlobals` and `SetupTerrainPieceGlobals` build:
/// hex to texel, texel to hex, piece texel to hex and the corners of the pieces in hex space.
/// Hex `(x, y)` is the texel `(63.9986 x + 32 y + tu, 0.0009 x + 64.0019 y + tv)`; the field is the
/// padded map, so the first hex row of the map is at `v = 128`.
// re: WorldGlobals::SetupWorldGlobals 0x086725f9; WorldGlobals::SetupTerrainPieceGlobals 0x08672d82
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldMatrices {
    pub hex_to_tex: [[f32; 4]; 4],
    pub tex_to_hex: [[f32; 4]; 4],
    pub piece_to_hex: [[f32; 4]; 4],
    /// `g_ahSmallCorners`: the hex of piece texel (0, 0) and (128, 128).
    pub small_corners: [[f32; 4]; 2],
    /// `g_ahLargeCorners`: the hex of piece texel (0, 0) and (512, 512).
    pub large_corners: [[f32; 4]; 2],
}

impl WorldMatrices {
    /// `wrap_x` is `g_eWrapMode` 1 (translation `(0.5, -2, 1.5)`), else mode 0 (`(-1, -2, 3)`).
    pub fn new(wrap_x: bool) -> Self {
        let t = if wrap_x {
            translation(0.5, -2.0, 1.5)
        } else {
            translation(-1.0, -2.0, 3.0)
        };
        let tex_to_hex_pre = mul(&base(), &t);
        let mut hex_to_tex = inverse(&tex_to_hex_pre);
        for r in &mut hex_to_tex {
            r[2] = 0.0;
        }
        let tex_to_hex = mul(&tex_to_hex_pre, &normalize());
        let piece_pre = mul(&base(), &translation(0.5, 0.0, -0.5));
        let piece_to_hex = to_f32(&mul(&piece_pre, &normalize()));
        let corner = |x: f32, y: f32| transform([x, y, 0.0, 1.0], &piece_to_hex);
        Self {
            hex_to_tex: to_f32(&hex_to_tex),
            tex_to_hex: to_f32(&tex_to_hex),
            piece_to_hex,
            small_corners: [corner(0.0, 0.0), corner(128.0, 128.0)],
            large_corners: [corner(0.0, 0.0), corner(512.0, 512.0)],
        }
    }

    /// `g_HexToTex` applied to a cube hex (the point `(x, y, z, 1)`).
    pub fn hex_to_texel(&self, hex: [f32; 3]) -> [f32; 2] {
        let v = transform([hex[0], hex[1], hex[2], 1.0], &self.hex_to_tex);
        [v[0], v[1]]
    }

    /// The hex (float cube coordinates) under the texel `(u, v)` of a piece.
    pub fn piece_texel_to_hex(&self, u: f32, v: f32) -> [f32; 3] {
        let h = transform([u, v, 0.0, 1.0], &self.piece_to_hex);
        [h[0], h[1], h[2]]
    }
}

/// `v * m` for a row vector of four (public for the blit).
pub fn transform4(v: [f32; 4], m: &[[f32; 4]; 4]) -> [f32; 4] {
    transform(v, m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hex_to_texel_matrix_is_64_texels_per_hex() {
        for wrap in [true, false] {
            let m = WorldMatrices::new(wrap);
            let o = m.hex_to_texel([0.0, 0.0, 0.0]);
            let px = m.hex_to_texel([1.0, 0.0, -1.0]);
            let py = m.hex_to_texel([0.0, 1.0, -1.0]);
            assert!((px[0] - o[0] - 64.0).abs() < 0.01 && (px[1] - o[1]).abs() < 0.01);
            assert!((py[0] - o[0] - 32.0).abs() < 0.01 && (py[1] - o[1] - 64.0).abs() < 0.01);
            // Two rows of padding above: the first hex row of the map is at 128.
            assert!((o[1] - 128.0).abs() < 0.01, "{o:?}");
            let want_u = if wrap { 32.0 } else { 128.0 };
            assert!((o[0] - want_u).abs() < 0.01, "{o:?}");
        }
    }

    #[test]
    fn the_piece_matrix_puts_the_cell_one_one_in_the_middle_of_a_small_piece() {
        let m = WorldMatrices::new(true);
        // hex_of(row 1, column 1) = (1, 1, -2) is at texel (64, 64) of a 128 piece.
        let h = m.piece_texel_to_hex(64.0, 64.0);
        assert!(
            (h[0] - 1.0).abs() < 0.01 && (h[1] - 1.0).abs() < 0.01,
            "{h:?}"
        );
        assert!((h[2] + 2.0).abs() < 0.01, "{h:?}");
        assert!((m.small_corners[0][0] - 0.5).abs() < 1e-3);
        assert!((m.small_corners[0][2] + 0.5).abs() < 1e-3);
    }

    #[test]
    fn array_lengths_match_declared_sizes() {
        let _: [u32; 13] = TT_POSTFIX;
        let _: [u32; 5] = CONTINENT_PREFIX;
        let _: [u32; 31] = HEX_COLOR_CODES;
        let _: [&str; 5] = TERRAIN_PATTERN_CLASS_NAMES;
        let _: [&str; 31] = BASE_TYPE_NAMES;
        let _: [&str; 5] = CONTINENT_NAMES;
        let _: [&str; 6] = TEXTURE_PATHS;
        let _: [u32; 31] = LEGACY_HEX_CODE_VALUES;
        let _: [u32; 23] = LEGACY_HEX_CODE_KEYS;
        let _: [usize; 23] = LEGACY_HEX_CODE_INDEX;
    }

    #[test]
    fn spot_values() {
        assert_eq!(LEGACY_HEX_CODE_VALUES[29], 0xf3ef_9fff);
        assert_eq!(CONTINENT_PREFIX[3], 0x0013_0000);
        assert_eq!(terrain_hex_code_from_type(0, 0), 0x0010_0002);
        assert_eq!(BASE_TYPE_NAMES[10], "Mountain");
    }
}
