//! Texture blob (`C5TX`): mip chain ready for `write_texture`.
//!
//! v1 was 2D only (`Rgba8`/`Bc7`/`Astc4x4`). v2 adds cubes, volumes and the BC1-BC5, R8, Rg8,
//! R16F, Rgba16F formats. v1 blobs are read and converted in memory.

use serde::{Deserialize, Serialize};

use crate::{FormatError, blob};

pub const TEXTURE_MAGIC: [u8; 4] = *b"C5TX";

/// Largest side of a 2D texture or cube face; also keeps size arithmetic far from overflow.
const MAX_SIDE: u32 = 16384;
/// Largest side of a volume (WebGL2 `MAX_3D_TEXTURE_SIZE` floor).
const MAX_SIDE_3D: u32 = 256;
const CUBE_FACES: u64 = 6;

/// Postcard writes the variant index: the v1 formats keep indices 0-2, new ones go at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextureFormat {
    Rgba8,
    Bc7,
    Astc4x4,
    Bc1,
    Bc2,
    Bc3,
    Bc4,
    Bc5,
    R8,
    Rg8,
    R16F,
    Rgba16F,
}

impl TextureFormat {
    /// `(texels per block side, bytes per block)`; uncompressed formats are 1x1 blocks.
    fn cell(self) -> (u32, u32) {
        match self {
            Self::Rgba8 => (1, 4),
            Self::R8 => (1, 1),
            Self::Rg8 | Self::R16F => (1, 2),
            Self::Rgba16F => (1, 8),
            Self::Bc1 | Self::Bc4 => (4, 8),
            Self::Bc2 | Self::Bc3 | Self::Bc5 | Self::Bc7 | Self::Astc4x4 => (4, 16),
        }
    }

    /// Texels per block side: 4 for the block-compressed formats, 1 otherwise.
    pub fn block_dim(self) -> u32 {
        self.cell().0
    }

    /// Bytes of one block (or of one texel when uncompressed).
    pub fn block_bytes(self) -> u32 {
        self.cell().1
    }

    pub fn is_compressed(self) -> bool {
        self.block_dim() > 1
    }

    /// Byte size of one 2D slice of a mip level, with tightly packed rows or blocks. Saturates
    /// instead of overflowing.
    pub fn mip_size(self, width: u32, height: u32) -> usize {
        usize::try_from(self.slice_size(width, height)).unwrap_or(usize::MAX)
    }

    fn slice_size(self, width: u32, height: u32) -> u64 {
        let (dim, bytes) = self.cell();
        let blocks = |side: u32| u64::from(side.div_ceil(dim));
        blocks(width)
            .saturating_mul(blocks(height))
            .saturating_mul(u64::from(bytes))
    }
}

/// `side` rounded up to whole 4x4 blocks: WebGPU creates a block-compressed texture only when level
/// 0 is a multiple of the block ("Hard constraint").
pub fn pad_to_block(side: u32) -> u32 {
    side.div_ceil(4) * 4
}

/// The part of a stored UI texture that holds the image, for a texture whose logical size (the one
/// the UI lays out with, `UiIndex::tex_dims`) is `logical` and whose blob is `stored`.
///
/// A compressed variant of an image that is not a multiple of 4 is padded to whole blocks
/// ([`pad_to_block`]), and the `Astc` variant of a large image may be stored at half size or less
/// (option A). The image then fills `ceil(logical / 2^k)` of the stored
/// size; this finds `k`. A blob that matches no such shape is taken as the image itself.
pub fn stored_content(logical: (u32, u32), stored: (u32, u32)) -> (u32, u32) {
    match stored_shrink(logical, stored) {
        Some(k) => shrunk(logical, k),
        None => stored,
    }
}

/// How many times an image of `logical` size was halved to be stored as `stored` (each side
/// rounded up, then maybe padded to whole blocks); `None` when `stored` is no such shape.
pub fn stored_shrink(logical: (u32, u32), stored: (u32, u32)) -> Option<u32> {
    (0..8u32).find(|&k| {
        let c = shrunk(logical, k);
        c == stored || (pad_to_block(c.0), pad_to_block(c.1)) == stored
    })
}

fn shrunk(logical: (u32, u32), k: u32) -> (u32, u32) {
    let s = |v: u32| v.div_ceil(1 << k).max(1);
    (s(logical.0), s(logical.1))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TexDim {
    D2,
    /// Six square faces.
    Cube,
    /// Volume; `depth` slices at level 0.
    D3,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextureBlob {
    pub format: TextureFormat,
    pub dim: TexDim,
    pub srgb: bool,
    pub width: u32,
    pub height: u32,
    /// 1 except for `D3`.
    pub depth: u32,
    /// Level 0 first; each level is `max(1, dim >> level)` wide, high and (`D3`) deep.
    /// `D2`: one slice. `Cube`: the six faces of the level back to back in DDS order
    /// (+X, -X, +Y, -Y, +Z, -Z). `D3`: all depth slices of the level back to back.
    pub mips: Vec<Vec<u8>>,
}

/// The v1 payload: 2D only.
#[derive(Deserialize)]
struct TextureBlobV1 {
    format: TextureFormat,
    srgb: bool,
    width: u32,
    height: u32,
    mips: Vec<Vec<u8>>,
}

/// Borrowed v1 payload for writing.
#[derive(Serialize)]
struct TextureBlobV1Ref<'a> {
    format: TextureFormat,
    srgb: bool,
    width: u32,
    height: u32,
    mips: &'a [Vec<u8>],
}

impl From<TextureBlobV1> for TextureBlob {
    fn from(v: TextureBlobV1) -> Self {
        Self {
            format: v.format,
            dim: TexDim::D2,
            srgb: v.srgb,
            width: v.width,
            height: v.height,
            depth: 1,
            mips: v.mips,
        }
    }
}

impl TextureBlob {
    /// Version written by [`Self::encode`].
    pub const VERSION: u16 = 2;
    /// Oldest version [`Self::decode`] reads.
    pub const MIN_VERSION: u16 = 1;

    /// A 2D texture.
    pub fn new_2d(
        format: TextureFormat,
        srgb: bool,
        width: u32,
        height: u32,
        mips: Vec<Vec<u8>>,
    ) -> Self {
        Self {
            format,
            dim: TexDim::D2,
            srgb,
            width,
            height,
            depth: 1,
            mips,
        }
    }

    /// Writes the oldest version that can hold the texture: a 2D texture in a B1 format is a v1
    /// blob, byte for byte what B1 wrote (so B1 builds keep their hashes and old readers keep
    /// working); anything else is v2.
    pub fn encode(&self) -> Vec<u8> {
        if self.fits_v1() {
            let v1 = TextureBlobV1Ref {
                format: self.format,
                srgb: self.srgb,
                width: self.width,
                height: self.height,
                mips: &self.mips,
            };
            blob::encode_v(TEXTURE_MAGIC, 1, &v1)
        } else {
            self.encode_v2()
        }
    }

    /// Always writes version 2.
    pub fn encode_v2(&self) -> Vec<u8> {
        blob::encode_v(TEXTURE_MAGIC, Self::VERSION, self)
    }

    fn fits_v1(&self) -> bool {
        self.dim == TexDim::D2
            && self.depth == 1
            && matches!(
                self.format,
                TextureFormat::Rgba8 | TextureFormat::Bc7 | TextureFormat::Astc4x4
            )
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (version, body) =
            blob::split_range(TEXTURE_MAGIC, Self::MIN_VERSION, Self::VERSION, bytes)?;
        let t: Self = if version == 1 {
            blob::payload::<TextureBlobV1>(body)?.into()
        } else {
            blob::payload(body)?
        };
        t.validate()?;
        Ok(t)
    }

    /// Byte size of mip `level` as stored in `mips`; `u64` so no dimension can overflow.
    fn level_size(&self, level: usize) -> u64 {
        let shift = u32::try_from(level).unwrap_or(u32::MAX);
        let side = |v: u32| v.checked_shr(shift).unwrap_or(0).max(1);
        let slice = self.format.slice_size(side(self.width), side(self.height));
        match self.dim {
            TexDim::D2 => slice,
            TexDim::Cube => slice.saturating_mul(CUBE_FACES),
            TexDim::D3 => slice.saturating_mul(u64::from(side(self.depth))),
        }
    }

    fn validate(&self) -> Result<(), FormatError> {
        if self.width == 0 || self.height == 0 || self.depth == 0 {
            return Err(FormatError::invalid("texture has zero size"));
        }
        // Cap keeps size arithmetic far from overflow, even on wasm32.
        if self.width > MAX_SIDE || self.height > MAX_SIDE {
            return Err(FormatError::invalid("texture larger than 16384"));
        }
        match self.dim {
            TexDim::D2 | TexDim::Cube if self.depth != 1 => {
                return Err(FormatError::invalid(
                    "depth must be 1 unless the texture is 3D",
                ));
            }
            TexDim::Cube if self.width != self.height => {
                return Err(FormatError::invalid("cube faces must be square"));
            }
            TexDim::D3
                if self.width > MAX_SIDE_3D
                    || self.height > MAX_SIDE_3D
                    || self.depth > MAX_SIDE_3D =>
            {
                return Err(FormatError::invalid("3D texture larger than 256"));
            }
            _ => {}
        }
        if self.mips.is_empty() {
            return Err(FormatError::invalid("texture has no mips"));
        }
        let longest = match self.dim {
            TexDim::D3 => self.width.max(self.height).max(self.depth),
            _ => self.width.max(self.height),
        };
        let full_chain = longest.ilog2() as usize + 1;
        if self.mips.len() > full_chain {
            return Err(FormatError::invalid("more mips than a full chain"));
        }
        for (level, data) in self.mips.iter().enumerate() {
            let expected = self.level_size(level);
            if data.len() as u64 != expected {
                return Err(FormatError::BadMipSize {
                    level,
                    expected: usize::try_from(expected).unwrap_or(usize::MAX),
                    found: data.len(),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(w: u32, h: u32, levels: usize) -> TextureBlob {
        let mips = (0..levels)
            .map(|l| vec![7u8; TextureFormat::Rgba8.mip_size((w >> l).max(1), (h >> l).max(1))])
            .collect();
        TextureBlob::new_2d(TextureFormat::Rgba8, true, w, h, mips)
    }

    const ALL_FORMATS: [TextureFormat; 12] = [
        TextureFormat::Rgba8,
        TextureFormat::Bc7,
        TextureFormat::Astc4x4,
        TextureFormat::Bc1,
        TextureFormat::Bc2,
        TextureFormat::Bc3,
        TextureFormat::Bc4,
        TextureFormat::Bc5,
        TextureFormat::R8,
        TextureFormat::Rg8,
        TextureFormat::R16F,
        TextureFormat::Rgba16F,
    ];

    /// A blob of `dim` with every level filled by a level-dependent byte.
    fn full(
        format: TextureFormat,
        dim: TexDim,
        side: u32,
        depth: u32,
        levels: usize,
    ) -> TextureBlob {
        let mut t = TextureBlob {
            format,
            dim,
            srgb: false,
            width: side,
            height: side,
            depth,
            mips: Vec::new(),
        };
        t.mips = (0..levels)
            .map(|l| vec![l as u8 + 1; t.level_size(l) as usize])
            .collect();
        t
    }

    #[test]
    fn roundtrip_all_formats() {
        let t = rgba(8, 4, 4);
        assert_eq!(TextureBlob::decode(&t.encode()).unwrap(), t);

        for format in [TextureFormat::Bc7, TextureFormat::Astc4x4] {
            let t = TextureBlob::new_2d(
                format,
                false,
                9,
                5,
                vec![
                    vec![1; format.mip_size(9, 5)],
                    vec![2; format.mip_size(4, 2)],
                    vec![3; format.mip_size(2, 1)],
                ],
            );
            assert_eq!(format.mip_size(9, 5), 3 * 2 * 16);
            assert_eq!(TextureBlob::decode(&t.encode()).unwrap(), t);
        }
    }

    #[test]
    fn roundtrip_every_dim_and_format() {
        for format in ALL_FORMATS {
            for (dim, depth) in [(TexDim::D2, 1), (TexDim::Cube, 1), (TexDim::D3, 8)] {
                let t = full(format, dim, 8, depth, 4);
                assert_eq!(
                    TextureBlob::decode(&t.encode()).unwrap(),
                    t,
                    "{format:?} {dim:?}"
                );
            }
        }
    }

    #[test]
    fn block_sizes() {
        let sizes = [
            (TextureFormat::Bc1, 8),
            (TextureFormat::Bc4, 8),
            (TextureFormat::Bc2, 16),
            (TextureFormat::Bc3, 16),
            (TextureFormat::Bc5, 16),
            (TextureFormat::Bc7, 16),
        ];
        for (f, bytes) in sizes {
            assert_eq!(f.mip_size(8, 4), 2 * bytes, "{f:?}");
            assert_eq!(f.mip_size(1, 1), bytes, "{f:?}");
        }
        assert_eq!(TextureFormat::R8.mip_size(3, 5), 15);
        assert_eq!(TextureFormat::Rg8.mip_size(3, 5), 30);
        assert_eq!(TextureFormat::R16F.mip_size(3, 5), 30);
        assert_eq!(TextureFormat::Rgba16F.mip_size(3, 5), 120);
    }

    #[test]
    fn cube_and_volume_level_sizes() {
        // 128 cube, 8 mips: six faces per level.
        let cube = full(TextureFormat::Bc1, TexDim::Cube, 128, 1, 8);
        assert_eq!(cube.mips[0].len(), 6 * 32 * 32 * 8);
        assert_eq!(cube.mips[7].len(), 6 * 8);
        assert!(TextureBlob::decode(&cube.encode()).is_ok());
        // Volume depth halves with the level: 32^3 -> 16^3 -> ...
        let vol = full(TextureFormat::Rgba8, TexDim::D3, 32, 32, 6);
        assert_eq!(vol.mips[0].len(), 32 * 32 * 32 * 4);
        assert_eq!(vol.mips[1].len(), 16 * 16 * 16 * 4);
        assert_eq!(vol.mips[5].len(), 4);
        assert!(TextureBlob::decode(&vol.encode()).is_ok());
    }

    #[test]
    fn v1_blob_is_read_and_converted() {
        // Exactly what B1 wrote: version 1, no dim or depth in the payload.
        #[derive(Serialize)]
        struct V1 {
            format: TextureFormat,
            srgb: bool,
            width: u32,
            height: u32,
            mips: Vec<Vec<u8>>,
        }
        let v1 = V1 {
            format: TextureFormat::Bc7,
            srgb: true,
            width: 8,
            height: 4,
            mips: vec![vec![5; 2 * 16], vec![6; 16], vec![7; 16]],
        };
        let bytes = blob::encode_v(TEXTURE_MAGIC, 1, &v1);
        let t = TextureBlob::decode(&bytes).unwrap();
        assert_eq!(t.dim, TexDim::D2);
        assert_eq!(t.depth, 1);
        assert_eq!(
            (t.format, t.srgb, t.width, t.height),
            (TextureFormat::Bc7, true, 8, 4)
        );
        assert_eq!(t.mips.len(), 3);
        // A 2D B1-format texture is written back as the same v1 bytes; v2 on request.
        assert_eq!(t.encode(), bytes);
        assert_eq!(u16::from_le_bytes([t.encode_v2()[4], t.encode_v2()[5]]), 2);
        assert_eq!(TextureBlob::decode(&t.encode_v2()).unwrap(), t);
        // Anything v1 cannot hold is written as v2.
        let bc1 = full(TextureFormat::Bc1, TexDim::D2, 4, 1, 1);
        let cube = full(TextureFormat::Rgba8, TexDim::Cube, 4, 1, 1);
        for t in [bc1, cube] {
            assert_eq!(u16::from_le_bytes([t.encode()[4], t.encode()[5]]), 2);
        }
        // The v1 format indices are unchanged.
        assert_eq!(postcard::to_allocvec(&TextureFormat::Astc4x4).unwrap(), [2]);
        assert_eq!(postcard::to_allocvec(&TextureFormat::Bc1).unwrap(), [3]);
    }

    #[test]
    fn rejects_wrong_mip_size() {
        let mut t = rgba(4, 4, 3);
        t.mips[1].pop();
        assert!(matches!(
            TextureBlob::decode(&t.encode()),
            Err(FormatError::BadMipSize {
                level: 1,
                expected: 16,
                found: 15
            })
        ));
        // A cube level with five faces instead of six.
        let mut cube = full(TextureFormat::Rgba8, TexDim::Cube, 4, 1, 2);
        cube.mips[0].truncate(5 * 4 * 4 * 4);
        assert!(matches!(
            TextureBlob::decode(&cube.encode()),
            Err(FormatError::BadMipSize { level: 0, .. })
        ));
    }

    #[test]
    fn rejects_bad_shape() {
        assert!(TextureBlob::decode(&rgba(4, 4, 4).encode()).is_err());
        let mut t = rgba(4, 4, 1);
        t.mips.clear();
        assert!(TextureBlob::decode(&t.encode()).is_err());
        let mut t = rgba(4, 4, 1);
        t.width = 0;
        assert!(TextureBlob::decode(&t.encode()).is_err());
        let mut t = rgba(4, 4, 1);
        t.depth = 0;
        assert!(TextureBlob::decode(&t.encode()).is_err());

        // Cube must be square; 2D and cube have depth 1.
        let mut cube = full(TextureFormat::Rgba8, TexDim::Cube, 4, 1, 1);
        cube.height = 2;
        assert!(TextureBlob::decode(&cube.encode()).is_err());
        let mut t = full(TextureFormat::Rgba8, TexDim::D2, 4, 1, 1);
        t.depth = 2;
        assert!(TextureBlob::decode(&t.encode()).is_err());

        // A 3D side above 256.
        let big = TextureBlob {
            format: TextureFormat::R8,
            dim: TexDim::D3,
            srgb: false,
            width: 257,
            height: 1,
            depth: 1,
            mips: vec![vec![0; 257]],
        };
        assert!(TextureBlob::decode(&big.encode()).is_err());
        // More mips than the longest side of a volume allows.
        let vol = full(TextureFormat::R8, TexDim::D3, 4, 2, 4);
        assert!(TextureBlob::decode(&vol.encode()).is_err());
    }

    #[test]
    fn huge_sizes_are_errors_not_overflow() {
        // 2D: the side cap rejects before any size arithmetic.
        let t = TextureBlob::new_2d(
            TextureFormat::Rgba16F,
            false,
            u32::MAX,
            u32::MAX,
            vec![vec![]],
        );
        assert!(TextureBlob::decode(&t.encode()).is_err());
        // Largest legal cube level is sized in u64; the (short) data is a size error.
        let t = TextureBlob {
            format: TextureFormat::Rgba16F,
            dim: TexDim::Cube,
            srgb: false,
            width: 16384,
            height: 16384,
            depth: 1,
            mips: vec![vec![0; 16]],
        };
        assert!(matches!(
            TextureBlob::decode(&t.encode()),
            Err(FormatError::BadMipSize { level: 0, .. })
        ));
        assert!(TextureFormat::Rgba16F.mip_size(u32::MAX, u32::MAX) > 0);
    }

    #[test]
    fn padded_and_halved_ui_textures_give_back_the_image_part() {
        assert_eq!(pad_to_block(30), 32);
        assert_eq!(pad_to_block(32), 32);
        assert_eq!(pad_to_block(1), 4);
        // Stored as it is.
        assert_eq!(stored_content((64, 32), (64, 32)), (64, 32));
        // Padded to whole blocks.
        assert_eq!(stored_content((30, 17), (32, 20)), (30, 17));
        // Halved (the Astc variant of a large image), then padded.
        assert_eq!(stored_content((2048, 1024), (1024, 512)), (1024, 512));
        assert_eq!(stored_content((2050, 1030), (1028, 516)), (1025, 515));
        // Anything else is the image itself.
        assert_eq!(stored_content((64, 64), (40, 40)), (40, 40));
        assert_eq!(stored_shrink((64, 64), (40, 40)), None);
        assert_eq!(stored_shrink((30, 17), (32, 20)), Some(0));
        assert_eq!(stored_shrink((2048, 1024), (1024, 512)), Some(1));
    }

    #[test]
    fn rejects_bad_magic_and_version() {
        let bytes = rgba(2, 2, 1).encode();
        let mut bad = bytes.clone();
        bad[3] = b'?';
        assert!(matches!(
            TextureBlob::decode(&bad),
            Err(FormatError::BadMagic { .. })
        ));
        for v in [0u8, 3] {
            let mut bad = bytes.clone();
            bad[4] = v;
            assert!(matches!(
                TextureBlob::decode(&bad),
                Err(FormatError::UnsupportedVersion { .. })
            ));
        }
    }
}
