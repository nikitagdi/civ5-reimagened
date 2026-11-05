//! Terrain pieces (`C5TP`): the CSV and the four DDS files of each
//! piece to a [`TerrainPiece`], packed losslessly with `zstd` level 19.
//!
//! A piece CSV (`Afri_Grass_1_1.csv`) is:
//! ```text
//! 3                              version (3 or 4)
//! Afri_Grass_1_1_H.dds           height, A16B16G16R16
//! Afri_Grass_1_1_T1.dds          terrain id, A8R8G8B8
//! Afri_Grass_1_1_H_BLEND.dds     height blend, A8R8G8B8
//! Afri_Grass_1_1_T_BLEND.dds     terrain blend, A8R8G8B8
//! 4,4                            columns, rows of the hex code grid
//! 0x800fffff,0x800fffff,...      one row per line
//! 0,1                            flags and the other numbers
//! ```
//! Cells may be separated by `, `, rows may end in commas (measured: 2026-10-21, all 304 files).
//! Nothing is interpreted: the meaning of the codes and flags is RW-6.

use asset_format::{Codec, HeightDiff, PieceStyle, Section, TerrainPiece};

use crate::textures::{RawDds, RawFormat, TextureError, read_dds};

/// Level used for packing. // measured: 2026-10-21, see the size notes of `b2_terrain`.
const ZSTD_LEVEL: i32 = 19;

#[derive(Debug, thiserror::Error)]
pub enum PieceError {
    #[error("{name}: {msg}")]
    Csv { name: String, msg: String },
    #[error("{name}: {file}: {source}")]
    Texture {
        name: String,
        file: String,
        #[source]
        source: TextureError,
    },
    #[error("{name}: {msg}")]
    Data { name: String, msg: String },
}

fn csv_err(name: &str, msg: impl Into<String>) -> PieceError {
    PieceError::Csv {
        name: name.to_owned(),
        msg: msg.into(),
    }
}

fn data_err(name: &str, msg: impl Into<String>) -> PieceError {
    PieceError::Data {
        name: name.to_owned(),
        msg: msg.into(),
    }
}

/// What the CSV of a piece says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PieceCsv {
    /// File name without `.csv`, lower case.
    pub name: String,
    pub version: u8,
    /// `_H`, `_T1`, `_H_BLEND`, `_T_BLEND`, in the order of the file.
    pub files: [String; 4],
    pub grid: (u8, u8),
    pub hex_codes: Vec<u32>,
    pub flags: u32,
    pub extra: Vec<u32>,
}

fn is_number_cell(s: &str) -> bool {
    !s.is_empty()
}

/// One cell of the CSV: decimal (maybe negative) or `0x` hex; empty cells are the caller's to skip.
fn number(name: &str, what: &str, cell: &str) -> Result<i64, PieceError> {
    let c = cell.trim();
    let parsed = match c.strip_prefix("0x").or_else(|| c.strip_prefix("0X")) {
        Some(h) => i64::from_str_radix(h, 16),
        None => c.parse::<i64>(),
    };
    parsed.map_err(|_| csv_err(name, format!("{what}: {cell:?} is not a number")))
}

fn cells(line: &str) -> Vec<&str> {
    line.split(',')
        .map(str::trim)
        .filter(|c| is_number_cell(c))
        .collect()
}

/// Parses a piece CSV. `name` is the file name without the extension. A file that is not a piece
/// (its first line is not a version number) is an error; the caller decides what that means.
pub fn parse_piece_csv(name: &str, text: &str) -> Result<PieceCsv, PieceError> {
    let name = name.to_ascii_lowercase();
    let lines: Vec<&str> = text
        .trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let first = |i: usize, what: &str| -> Result<Vec<&str>, PieceError> {
        lines
            .get(i)
            .map(|l| cells(l))
            .filter(|c| !c.is_empty())
            .ok_or_else(|| csv_err(&name, format!("{what} is missing")))
    };
    let version = number(&name, "version", first(0, "the version line")?[0])?;
    if !matches!(version, 3 | 4) {
        return Err(csv_err(
            &name,
            format!("version {version} (3 or 4 expected)"),
        ));
    }
    let mut files: Vec<String> = Vec::new();
    for (i, what) in ["height", "id", "height blend", "terrain blend"]
        .iter()
        .enumerate()
    {
        let c = first(1 + i, &format!("the {what} file name"))?;
        if c.len() != 1 {
            return Err(csv_err(
                &name,
                format!("the {what} line has {} cells", c.len()),
            ));
        }
        files.push(c[0].to_owned());
    }
    let grid_line = first(5, "the grid size")?;
    if grid_line.len() != 2 {
        return Err(csv_err(&name, "the grid size line needs two numbers"));
    }
    let (cols, rows) = (
        number(&name, "grid columns", grid_line[0])?,
        number(&name, "grid rows", grid_line[1])?,
    );
    if !(1..=64).contains(&cols) || !(1..=64).contains(&rows) {
        return Err(csv_err(
            &name,
            format!("grid {cols} x {rows} is out of range"),
        ));
    }
    let mut hex_codes = Vec::new();
    for r in 0..rows as usize {
        let row = first(6 + r, &format!("grid row {r}"))?;
        if row.len() != cols as usize {
            return Err(csv_err(
                &name,
                format!("grid row {r} has {} cells, {cols} expected", row.len()),
            ));
        }
        for c in row {
            let v = number(&name, "hex code", c)?;
            let v = u32::try_from(v)
                .map_err(|_| csv_err(&name, format!("hex code {c:?} does not fit 32 bits")))?;
            hex_codes.push(v);
        }
    }
    let tail = first(6 + rows as usize, "the flags line")?;
    // The numbers as they are; a negative one keeps its 32-bit pattern (`-1` is 0xffffffff).
    let mut nums = Vec::new();
    for c in tail {
        let v = number(&name, "flags", c)?;
        nums.push(v as i32 as u32);
    }
    if lines.len() > 7 + rows as usize {
        return Err(csv_err(&name, "lines after the flags line"));
    }
    let flags = nums[0];
    let extra = nums[1..].to_vec();
    let files: [String; 4] = files
        .try_into()
        .map_err(|_| csv_err(&name, "four file names expected"))?;
    Ok(PieceCsv {
        name,
        version: version as u8,
        files,
        grid: (cols as u8, rows as u8),
        hex_codes,
        flags,
        extra,
    })
}

/// The style of a piece from the folder of its CSV: `.../Grass/Africa/x.csv` is Africa. The spare
/// piece `missing_hex.csv` lies in the folder of all pieces and is Generic.
pub fn style_of_folder(folder: &str, name: &str) -> Option<PieceStyle> {
    PieceStyle::from_name(folder).or_else(|| (name == "missing_hex").then_some(PieceStyle::Generic))
}

// ---- packing -------------------------------------------------------------------------------

fn zstd_pack(data: &[u8]) -> Vec<u8> {
    // The level is a constant and the input is in memory: the only failure is out of memory.
    zstd::bulk::compress(data, ZSTD_LEVEL).expect("zstd cannot fail on in-memory data")
}

/// A section of `data`: `Const` when every element is the same, else the smallest of zstd with
/// and without the byte shuffle, else raw. `wide` marks u16 elements (shuffle applies, constants
/// are two bytes).
fn pack(data: &[u8], wide: bool) -> Section {
    let raw_len = data.len() as u32;
    let width = if wide { 2 } else { 1 };
    if data.len() >= width && data.chunks_exact(width).all(|c| c == &data[..width]) {
        return Section::constant(&data[..width], raw_len);
    }
    let mut best = Section {
        codec: Codec::Raw,
        shuffle: false,
        raw_len,
        data: data.to_vec(),
    };
    let mut consider = |shuffle: bool, input: &[u8]| {
        let z = zstd_pack(input);
        if z.len() < best.data.len() {
            best = Section {
                codec: Codec::Zstd,
                shuffle,
                raw_len,
                data: z,
            };
        }
    };
    consider(false, data);
    if wide && data.len().is_multiple_of(2) {
        consider(true, &asset_format::shuffle_u16(data));
    }
    best
}

fn rgba16(d: &RawDds) -> Vec<[u16; 4]> {
    d.mips[0]
        .chunks_exact(8)
        .map(|p| {
            [
                u16::from_le_bytes([p[0], p[1]]),
                u16::from_le_bytes([p[2], p[3]]),
                u16::from_le_bytes([p[4], p[5]]),
                u16::from_le_bytes([p[6], p[7]]),
            ]
        })
        .collect()
}

fn check_dds(
    name: &str,
    file: &str,
    d: &RawDds,
    want: RawFormat,
    side: Option<u16>,
) -> Result<u16, PieceError> {
    let bad = |msg: String| PieceError::Texture {
        name: name.to_owned(),
        file: file.to_owned(),
        source: TextureError::Unsupported(msg),
    };
    if d.format != want || d.dim != asset_format::TexDim::D2 {
        return Err(bad(format!(
            "expected {want:?} 2D, the file is {:?} {:?}",
            d.format, d.dim
        )));
    }
    if d.width != d.height || d.width == 0 || d.width > u32::from(u16::MAX) {
        return Err(bad(format!(
            "{}x{} is not a square piece",
            d.width, d.height
        )));
    }
    let s = d.width as u16;
    if side.is_some_and(|w| w != s) {
        return Err(bad(format!(
            "side {s} differs from the height map's {}",
            side.unwrap_or(0)
        )));
    }
    Ok(s)
}

/// Builds the piece from its CSV and the four decoded files (in the order of `csv.files`).
pub fn build_piece(csv: &PieceCsv, files: [&[u8]; 4]) -> Result<TerrainPiece, PieceError> {
    let name = csv.name.as_str();
    let read = |i: usize| -> Result<RawDds, PieceError> {
        read_dds(files[i]).map_err(|source| PieceError::Texture {
            name: name.to_owned(),
            file: csv.files[i].clone(),
            source,
        })
    };
    let (h, t1, hb, tb) = (read(0)?, read(1)?, read(2)?, read(3)?);
    let side = check_dds(name, &csv.files[0], &h, RawFormat::Rgba16, None)?;
    check_dds(name, &csv.files[1], &t1, RawFormat::Rgba8, Some(side))?;
    check_dds(name, &csv.files[2], &hb, RawFormat::Rgba8, Some(side))?;
    check_dds(name, &csv.files[3], &tb, RawFormat::Rgba8, Some(side))?;

    // Height: G and A planes, the other two channels as differences from G.
    let texels = rgba16(&h);
    let g: Vec<u8> = texels.iter().flat_map(|t| t[1].to_le_bytes()).collect();
    let a: Vec<u8> = texels.iter().flat_map(|t| t[3].to_le_bytes()).collect();
    let mut height_diff = Vec::new();
    for (i, t) in texels.iter().enumerate() {
        if t[0] != t[1] || t[2] != t[1] {
            let diff = |c: u16| {
                i16::try_from(i32::from(c) - i32::from(t[1])).map_err(|_| {
                    data_err(
                        name,
                        format!("texel {i}: a height channel differs from G by more than 32767"),
                    )
                })
            };
            height_diff.push(HeightDiff {
                index: i as u32,
                dr: diff(t[0])?,
                db: diff(t[2])?,
            });
        }
    }

    // Ids: the code is the file's bytes B, G, R, A as a u32 LE; the DDS reader gave R, G, B, A.
    let mut palette: Vec<u32> = Vec::new();
    let mut index = Vec::with_capacity(t1.mips[0].len() / 4);
    for p in t1.mips[0].chunks_exact(4) {
        let code = u32::from_le_bytes([p[2], p[1], p[0], p[3]]);
        let at = match palette.iter().position(|&c| c == code) {
            Some(at) => at,
            None => {
                if palette.len() == 256 {
                    return Err(data_err(name, "more than 256 distinct terrain id codes"));
                }
                palette.push(code);
                palette.len() - 1
            }
        };
        index.push(at as u8);
    }

    let planes = |d: &RawDds| -> Vec<u8> {
        let n = d.mips[0].len() / 4;
        let mut v = vec![0u8; n * 4];
        for (i, p) in d.mips[0].chunks_exact(4).enumerate() {
            for (c, &b) in p.iter().enumerate() {
                v[c * n + i] = b;
            }
        }
        v
    };

    Ok(TerrainPiece {
        name: csv.name.clone(),
        side,
        csv_version: csv.version,
        flags: csv.flags,
        csv_extra: csv.extra.clone(),
        grid: csv.grid,
        hex_codes: csv.hex_codes.clone(),
        height_g: pack(&g, true),
        height_a: pack(&a, true),
        height_diff,
        id_palette: palette,
        id_index: pack(&index, false),
        h_blend: pack(&planes(&hb), false),
        t_blend: pack(&planes(&tb), false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRASS: &str = "3,,,\r\nAfri_Grass_1_1_H.dds,,,\r\nAfri_Grass_1_1_T1.dds,,,\r\nAfri_Grass_1_1_H_BLEND.dds,,,\r\nAfri_Grass_1_1_T_BLEND.dds,,,\r\n4,4,,\r\n0x800fffff,0x800fffff,0x800fffff,0x800fffff\r\n0x800fffff,0x1,0x800fffff,0x800fffff\r\n0x800fffff,0x800fffff,0x800fffff,0x800fffff\r\n0x800fffff,0x800fffff,0x800fffff,0x800fffff\r\n0,1,,\r\n";

    #[test]
    fn a_piece_csv_is_read_with_its_trailing_commas_and_spaces() {
        let c = parse_piece_csv("Afri_Grass_1_1", GRASS).unwrap();
        assert_eq!(c.name, "afri_grass_1_1");
        assert_eq!((c.version, c.grid), (3, (4, 4)));
        assert_eq!(c.files[2], "Afri_Grass_1_1_H_BLEND.dds");
        assert_eq!(c.hex_codes.len(), 16);
        assert_eq!((c.hex_codes[0], c.hex_codes[5]), (0x800f_ffff, 1));
        assert_eq!((c.flags, c.extra.as_slice()), (0, &[1][..]));

        let spaced = "4\nA_H.dds\nA_T1.dds\nA_HB.dds\nA_TB.dds\n4, 4\n1, 2, 3, 4\n1,2,3,4\n1,2,3,4\n1,2,3,4\n-1, 1\n";
        let c = parse_piece_csv("a", spaced).unwrap();
        assert_eq!(c.version, 4);
        assert_eq!(c.flags, u32::MAX, "-1 keeps its bit pattern");
        assert_eq!(c.extra, [1]);
    }

    #[test]
    fn what_is_not_a_piece_csv_is_an_error() {
        assert!(parse_piece_csv("fogcolor", "146,182,243\r\n").is_err());
        assert!(parse_piece_csv("terraindef", "Continent ID,Diffuse\n0,a.dds").is_err());
        assert!(parse_piece_csv("x", "").is_err());
        // Version 5, a short grid row, a bad number, a missing flags line.
        assert!(parse_piece_csv("x", &GRASS.replacen('3', "5", 1)).is_err());
        let short = GRASS.replacen("0x800fffff,0x800fffff,0x800fffff,0x800fffff", "0x1,0x2", 1);
        assert!(parse_piece_csv("x", &short).is_err());
        assert!(parse_piece_csv("x", &GRASS.replace("0x1,", "0xZZ,")).is_err());
        assert!(parse_piece_csv("x", GRASS.trim_end().rsplit_once('\n').unwrap().0).is_err());
        // A line after the flags is not ours to ignore.
        assert!(parse_piece_csv("x", &format!("{GRASS}9,9\n")).is_err());
    }

    #[test]
    fn style_comes_from_the_folder() {
        assert_eq!(style_of_folder("Africa", "x"), Some(PieceStyle::Africa));
        assert_eq!(style_of_folder("generic", "x"), Some(PieceStyle::Generic));
        assert_eq!(
            style_of_folder("pieces", "missing_hex"),
            Some(PieceStyle::Generic)
        );
        assert_eq!(style_of_folder("pieces", "x"), None);
    }

    #[test]
    fn sections_pick_the_smallest_form() {
        // Flat u16 data is a constant of two bytes.
        let flat: Vec<u8> = std::iter::repeat_n([0x34u8, 0x12], 1000)
            .flatten()
            .collect();
        let s = pack(&flat, true);
        assert_eq!((s.codec, s.data.len()), (Codec::Const, 2));
        assert_eq!(s.decode().unwrap(), flat);
        // A smooth ramp compresses, and the shuffle helps: low bytes cycle, high bytes are flat.
        let ramp: Vec<u8> = (0..4096u32)
            .flat_map(|i| (i as u16).to_le_bytes())
            .collect();
        let s = pack(&ramp, true);
        assert_eq!(s.codec, Codec::Zstd);
        assert!(s.data.len() < ramp.len() / 4);
        assert_eq!(s.decode().unwrap(), ramp);
        // Noise stays raw.
        let mut x = 12345u32;
        let noise: Vec<u8> = (0..300)
            .map(|_| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (x >> 24) as u8
            })
            .collect();
        let s = pack(&noise, false);
        assert_eq!(s.codec, Codec::Raw);
        assert_eq!(s.decode().unwrap(), noise);
    }
}
