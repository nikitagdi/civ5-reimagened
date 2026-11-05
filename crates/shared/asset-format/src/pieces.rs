//! Terrain pieces (`C5TP`, manifest id `pc:<style>`): the prefabricated height, id and blend maps
//! the original stamps into the world, one blob per continent style.
//!
//! A piece is four DDS files of the original (`_H` height, `_T1` terrain id, `_H_BLEND` and
//! `_T_BLEND` weights) and one CSV row set (the hex code grid). It is stored losslessly, level 0
//! only; the client builds mips if a reader needs them. Every large plane is a [`Section`] that is
//! unpacked on its own: [`TerrainPieceSet::piece`] unpacks one piece (about 3.3 MB at 512 x 512) and
//! the rest stay packed.
//!
//! Layout of a piece (all planes are `side` x `side`, row by row as in the DDS):
//! - `height_g`, `height_a`: the G and A channels of the A16B16G16R16 height map as u16 LE. The
//!   channels R and B equal G except for a few texels, which are listed in `height_diff`.
//! - `id_palette` and `id_index`: the A8R8G8B8 terrain id map as an index into the distinct values
//!   (at most 7 per file in the install); the code is the file's bytes (B, G, R, A) as a u32 LE.
//! - `h_blend`, `t_blend`: the A8R8G8B8 blend maps as four planes of u8, in the order R, G, B, A.
//!
//! The meaning of the channels and codes is open (RW-6); this format keeps all of them.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{FormatError, LogicalId, Section, blob};

pub const PIECES_MAGIC: [u8; 4] = *b"C5TP";

/// Most pieces a set may hold (the install has about 80 per style).
const MAX_PIECES: usize = 4096;
/// Largest side of a piece; the install has 128 and 512.
const MAX_SIDE: u16 = 2048;
/// Most extra numbers in a CSV row.
const MAX_EXTRA: usize = 64;

/// The continent styles of the pieces, in the order of the files of the install.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PieceStyle {
    Generic,
    Africa,
    America,
    Asia,
    Europe,
}

impl PieceStyle {
    pub const ALL: [PieceStyle; 5] = [
        Self::Generic,
        Self::Africa,
        Self::America,
        Self::Asia,
        Self::Europe,
    ];

    /// Lower-case name, the part of the id after `pc:`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::Africa => "africa",
            Self::America => "america",
            Self::Asia => "asia",
            Self::Europe => "europe",
        }
    }

    /// Case-insensitive.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|s| s.name().eq_ignore_ascii_case(name))
    }

    /// `pc:europe`.
    pub fn id(self) -> LogicalId {
        LogicalId::new(format!("pc:{}", self.name()))
    }
}

impl fmt::Display for PieceStyle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A texel where the height map's R or B channel differs from G: the differences as i16.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeightDiff {
    /// Texel index in the plane (`y * side + x`).
    pub index: u32,
    /// `R - G` and `B - G`.
    pub dr: i16,
    pub db: i16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerrainPiece {
    /// Lower-case name of the piece: the CSV file name without `.csv`. `missing_hex` is the spare.
    pub name: String,
    /// 128 or 512.
    pub side: u16,
    /// First line of the CSV (3 or 4).
    pub csv_version: u8,
    /// First number of the last CSV row (`-1` of `missing_hex` is `0xffff_ffff`).
    pub flags: u32,
    /// The other numbers of that row, as they are.
    pub csv_extra: Vec<u32>,
    /// Columns and rows of the hex code grid (4 x 4 or 10 x 10).
    pub grid: (u8, u8),
    /// `grid.0 * grid.1` codes, row by row as in the CSV (`0x800fffff`).
    pub hex_codes: Vec<u32>,
    /// u16 LE, `side * side`.
    pub height_g: Section,
    pub height_a: Section,
    /// Sorted by index.
    pub height_diff: Vec<HeightDiff>,
    /// Distinct terrain id codes, in order of first use, at most 256.
    pub id_palette: Vec<u32>,
    /// u8 per texel, an index into `id_palette`.
    pub id_index: Section,
    /// 4 planes of u8 (R, G, B, A), `4 * side * side`.
    pub h_blend: Section,
    pub t_blend: Section,
}

/// One piece unpacked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PieceData {
    pub side: u16,
    pub height_g: Vec<u16>,
    pub height_a: Vec<u16>,
    pub height_diff: Vec<HeightDiff>,
    pub id_palette: Vec<u32>,
    pub id_index: Vec<u8>,
    /// R, G, B, A planes.
    pub h_blend: [Vec<u8>; 4],
    pub t_blend: [Vec<u8>; 4],
}

impl PieceData {
    /// The height map as the original file has it: `[R, G, B, A]` per texel.
    pub fn height_rgba(&self) -> Vec<[u16; 4]> {
        let mut v: Vec<[u16; 4]> = self
            .height_g
            .iter()
            .zip(&self.height_a)
            .map(|(&g, &a)| [g, g, g, a])
            .collect();
        for d in &self.height_diff {
            let t = &mut v[d.index as usize];
            t[0] = t[1].wrapping_add_signed(d.dr);
            t[2] = t[1].wrapping_add_signed(d.db);
        }
        v
    }

    /// The terrain id code of a texel.
    pub fn id_code(&self, texel: usize) -> u32 {
        self.id_palette[usize::from(self.id_index[texel])]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerrainPieceSet {
    pub style: PieceStyle,
    /// Sorted by name.
    pub pieces: Vec<TerrainPiece>,
}

fn texels(side: u16) -> usize {
    usize::from(side) * usize::from(side)
}

fn split_planes(data: &[u8], texels: usize) -> Result<[Vec<u8>; 4], FormatError> {
    if data.len() != texels * 4 {
        return Err(FormatError::invalid("blend section has the wrong length"));
    }
    Ok([
        data[..texels].to_vec(),
        data[texels..texels * 2].to_vec(),
        data[texels * 2..texels * 3].to_vec(),
        data[texels * 3..].to_vec(),
    ])
}

fn u16s(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|p| u16::from_le_bytes([p[0], p[1]]))
        .collect()
}

impl TerrainPiece {
    /// Structure of the piece without unpacking anything: the lengths of the sections agree with
    /// the side, the grid with the hex codes, the palette and the differences are in range.
    pub fn validate(&self) -> Result<(), FormatError> {
        let bad = |what: &str| FormatError::invalid(format!("piece {:?}: {what}", self.name));
        if self.side == 0 || self.side > MAX_SIDE {
            return Err(bad("side is out of range"));
        }
        let n = texels(self.side);
        for (what, s, want) in [
            ("height_g", &self.height_g, n * 2),
            ("height_a", &self.height_a, n * 2),
            ("id_index", &self.id_index, n),
            ("h_blend", &self.h_blend, n * 4),
            ("t_blend", &self.t_blend, n * 4),
        ] {
            s.validate()?;
            if s.raw_len as usize != want {
                return Err(bad(&format!(
                    "{what} is {} bytes, {want} expected",
                    s.raw_len
                )));
            }
        }
        if self.grid.0 == 0
            || self.grid.1 == 0
            || self.hex_codes.len() != usize::from(self.grid.0) * usize::from(self.grid.1)
        {
            return Err(bad("the hex code grid does not match its size"));
        }
        if self.id_palette.is_empty() || self.id_palette.len() > 256 {
            return Err(bad("the id palette has no entry or more than 256"));
        }
        if self.csv_extra.len() > MAX_EXTRA {
            return Err(bad("too many extra CSV numbers"));
        }
        let mut last = None;
        for d in &self.height_diff {
            if d.index as usize >= n || last.is_some_and(|l| d.index <= l) {
                return Err(bad("height differences are out of range or not sorted"));
            }
            last = Some(d.index);
        }
        Ok(())
    }

    /// Unpacks the piece. The sections are decoded and checked against the structure; an id index
    /// past the palette is an error.
    pub fn data(&self) -> Result<PieceData, FormatError> {
        self.validate()?;
        let n = texels(self.side);
        let id_index = self.id_index.decode()?;
        if id_index
            .iter()
            .any(|&i| usize::from(i) >= self.id_palette.len())
        {
            return Err(FormatError::invalid(format!(
                "piece {:?}: an id index is past the palette",
                self.name
            )));
        }
        Ok(PieceData {
            side: self.side,
            height_g: u16s(&self.height_g.decode()?),
            height_a: u16s(&self.height_a.decode()?),
            height_diff: self.height_diff.clone(),
            id_palette: self.id_palette.clone(),
            id_index,
            h_blend: split_planes(&self.h_blend.decode()?, n)?,
            t_blend: split_planes(&self.t_blend.decode()?, n)?,
        })
    }
}

impl TerrainPieceSet {
    pub const VERSION: u16 = 1;
    pub const MIN_VERSION: u16 = 1;

    pub fn encode(&self) -> Vec<u8> {
        blob::encode_v(PIECES_MAGIC, Self::VERSION, self)
    }

    /// Reads the blob and checks the structure of every piece; nothing is unpacked.
    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (_, set): (u16, Self) =
            blob::decode_range(PIECES_MAGIC, Self::MIN_VERSION, Self::VERSION, bytes)?;
        set.validate()?;
        Ok(set)
    }

    fn validate(&self) -> Result<(), FormatError> {
        if self.pieces.len() > MAX_PIECES {
            return Err(FormatError::invalid("too many pieces"));
        }
        for w in self.pieces.windows(2) {
            if w[0].name >= w[1].name {
                return Err(FormatError::invalid(format!(
                    "pieces are not sorted by name or repeat: {:?}, {:?}",
                    w[0].name, w[1].name
                )));
            }
        }
        self.pieces.iter().try_for_each(TerrainPiece::validate)
    }

    /// Unpacks piece `i` only; the others stay packed.
    pub fn piece(&self, i: usize) -> Result<PieceData, FormatError> {
        self.pieces
            .get(i)
            .ok_or_else(|| {
                FormatError::invalid(format!("no piece {i} in a set of {}", self.pieces.len()))
            })?
            .data()
    }

    /// Index of the piece with this name (lower case).
    pub fn find(&self, name: &str) -> Option<usize> {
        self.pieces
            .binary_search_by(|p| p.name.as_str().cmp(name))
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use ruzstd::encoding::{CompressionLevel, compress_to_vec};

    use super::*;
    use crate::Codec;

    fn zstd(raw: &[u8], shuffle: bool) -> Section {
        let src = if shuffle {
            crate::shuffle_u16(raw)
        } else {
            raw.to_vec()
        };
        Section {
            codec: Codec::Zstd,
            shuffle,
            raw_len: raw.len() as u32,
            data: compress_to_vec(&src[..], CompressionLevel::Fastest),
        }
    }

    fn piece(name: &str, side: u16, seed: u16) -> TerrainPiece {
        let n = texels(side);
        let g: Vec<u16> = (0..n)
            .map(|i| seed.wrapping_add((i as u16).wrapping_mul(7)))
            .collect();
        let bytes: Vec<u8> = g.iter().flat_map(|v| v.to_le_bytes()).collect();
        TerrainPiece {
            name: name.into(),
            side,
            csv_version: 3,
            flags: u32::MAX,
            csv_extra: vec![1],
            grid: (2, 2),
            hex_codes: vec![0x800f_ffff, 1, 2, 0x20],
            height_g: zstd(&bytes, true),
            height_a: Section::constant(&[0x34, 0x12], (n * 2) as u32),
            height_diff: vec![
                HeightDiff {
                    index: 1,
                    dr: -2,
                    db: 3,
                },
                HeightDiff {
                    index: 5,
                    dr: 0,
                    db: -7,
                },
            ],
            id_palette: vec![0x0010_0005, 0xff00_0000],
            id_index: zstd(&(0..n).map(|i| (i % 2) as u8).collect::<Vec<_>>(), false),
            h_blend: Section::raw((0..n * 4).map(|i| i as u8).collect()).unwrap(),
            t_blend: Section::constant(&[9], (n * 4) as u32),
        }
    }

    fn set() -> TerrainPieceSet {
        TerrainPieceSet {
            style: PieceStyle::Europe,
            pieces: vec![piece("euro_a", 8, 100), piece("euro_b", 16, 5)],
        }
    }

    #[test]
    fn round_trips_and_one_piece_unpacks_alone() {
        let s = set();
        let bytes = s.encode();
        assert_eq!(&bytes[..4], b"C5TP");
        let back = TerrainPieceSet::decode(&bytes).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.find("euro_b"), Some(1));
        assert_eq!(back.find("nope"), None);
        let d = back.piece(0).unwrap();
        assert_eq!(d.side, 8);
        assert_eq!(d.height_g[3], 100u16.wrapping_add(21));
        assert!(d.height_a.iter().all(|&a| a == 0x1234));
        assert_eq!(d.id_code(0), 0x0010_0005);
        assert_eq!(d.id_code(1), 0xff00_0000);
        assert_eq!(d.h_blend[1][0], 64);
        assert!(d.t_blend[3].iter().all(|&v| v == 9));
        // R and B come back from the differences.
        let rgba = d.height_rgba();
        assert_eq!(
            rgba[1],
            [d.height_g[1] - 2, d.height_g[1], d.height_g[1] + 3, 0x1234]
        );
        assert_eq!(
            rgba[2],
            [d.height_g[2], d.height_g[2], d.height_g[2], 0x1234]
        );
        assert!(back.piece(2).is_err());
    }

    #[test]
    fn style_names() {
        assert_eq!(PieceStyle::from_name("Europe"), Some(PieceStyle::Europe));
        assert_eq!(PieceStyle::Asia.id().as_str(), "pc:asia");
        assert_eq!(PieceStyle::from_name("mars"), None);
    }

    #[test]
    fn broken_structure_is_an_error_not_a_panic() {
        let mut s = set();
        s.pieces[0].side = 16;
        assert!(
            TerrainPieceSet::decode(&s.encode()).is_err(),
            "sections do not fit the side"
        );

        let mut s = set();
        s.pieces[0].hex_codes.pop();
        assert!(TerrainPieceSet::decode(&s.encode()).is_err(), "grid");

        let mut s = set();
        s.pieces[0].id_palette.clear();
        assert!(TerrainPieceSet::decode(&s.encode()).is_err(), "palette");

        let mut s = set();
        s.pieces[0].height_diff[1].index = 1;
        assert!(
            TerrainPieceSet::decode(&s.encode()).is_err(),
            "unsorted differences"
        );
        let mut s = set();
        s.pieces[0].height_diff[0].index = 64;
        assert!(
            TerrainPieceSet::decode(&s.encode()).is_err(),
            "difference out of range"
        );

        let mut s = set();
        s.pieces.swap(0, 1);
        assert!(
            TerrainPieceSet::decode(&s.encode()).is_err(),
            "unsorted pieces"
        );

        // An index past the palette is found when the piece is unpacked.
        let mut s = set();
        s.pieces[0].id_index = Section::constant(&[7], 64);
        let back = TerrainPieceSet::decode(&s.encode()).unwrap();
        assert!(back.piece(0).is_err());
        assert!(back.piece(1).is_ok());
    }

    #[test]
    fn wrong_magic_version_and_tail_are_refused() {
        let good = set().encode();
        let mut bad = good.clone();
        bad[0] = b'X';
        assert!(matches!(
            TerrainPieceSet::decode(&bad),
            Err(FormatError::BadMagic { .. })
        ));
        let mut v2 = good.clone();
        v2[4] = 2;
        assert!(matches!(
            TerrainPieceSet::decode(&v2),
            Err(FormatError::UnsupportedVersion { .. })
        ));
        let mut tail = good.clone();
        tail.push(0);
        assert!(matches!(
            TerrainPieceSet::decode(&tail),
            Err(FormatError::TrailingBytes(1))
        ));
        assert!(TerrainPieceSet::decode(&good[..good.len() / 2]).is_err());
    }

    #[test]
    fn the_blob_is_deterministic_and_small_for_flat_data() {
        let mut p = piece("flat", 128, 0);
        let n = texels(128);
        p.height_g = Section::constant(&[0, 0], (n * 2) as u32);
        p.id_index = Section::constant(&[0], n as u32);
        p.h_blend = Section::constant(&[0], (n * 4) as u32);
        let s = TerrainPieceSet {
            style: PieceStyle::Generic,
            pieces: vec![p],
        };
        assert_eq!(s.encode(), s.encode());
        assert!(s.encode().len() < 300, "{} B", s.encode().len());
    }
}
