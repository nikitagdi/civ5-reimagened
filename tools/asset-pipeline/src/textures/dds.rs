//! DDS container to RGBA8 mip chain.

use std::io::{Cursor, Read};

use asset_format::{TexDim, TextureBlob, TextureFormat};
use ddsfile::{Caps2, D3DFormat, Dds, DxgiFormat};

use super::mips::full_chain_len;
use super::{MAX_SIDE, Rgba8Image, TextureError};

/// Source pixel layouts the decoder understands. Names follow D3D (high bits first),
/// so `A8R8G8B8` is stored B, G, R, A in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceFormat {
    Dxt1,
    Dxt3,
    Dxt5,
    A8R8G8B8,
    X8R8G8B8,
    A8B8G8R8,
    X8B8G8R8,
    R8G8B8,
    L8,
    A8L8,
}

impl SourceFormat {
    pub fn name(self) -> &'static str {
        match self {
            Self::Dxt1 => "DXT1",
            Self::Dxt3 => "DXT3",
            Self::Dxt5 => "DXT5",
            Self::A8R8G8B8 => "A8R8G8B8",
            Self::X8R8G8B8 => "X8R8G8B8",
            Self::A8B8G8R8 => "A8B8G8R8",
            Self::X8B8G8R8 => "X8B8G8R8",
            Self::R8G8B8 => "R8G8B8",
            Self::L8 => "L8",
            Self::A8L8 => "A8L8",
        }
    }

    /// `Some(bytes per 4x4 block)` for block-compressed formats.
    fn block_bytes(self) -> Option<usize> {
        match self {
            Self::Dxt1 => Some(8),
            Self::Dxt3 | Self::Dxt5 => Some(16),
            _ => None,
        }
    }

    fn pixel_bytes(self) -> usize {
        match self {
            Self::A8R8G8B8 | Self::X8R8G8B8 | Self::A8B8G8R8 | Self::X8B8G8R8 => 4,
            Self::R8G8B8 => 3,
            Self::A8L8 => 2,
            Self::L8 => 1,
            Self::Dxt1 | Self::Dxt3 | Self::Dxt5 => 0,
        }
    }

    /// Byte size of one mip level as stored in the file.
    fn mip_bytes(self, width: u32, height: u32) -> u64 {
        let (w, h) = (u64::from(width), u64::from(height));
        match self.block_bytes() {
            Some(b) => w.div_ceil(4) * h.div_ceil(4) * b as u64,
            None => w * h * self.pixel_bytes() as u64,
        }
    }
}

/// Header facts, available even when the pixel data cannot be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdsInfo {
    pub width: u32,
    pub height: u32,
    /// Levels declared by the header (1 when the field is absent).
    pub mip_count: u32,
    /// Display name of the pixel format, e.g. `DXT5` or `R32F`.
    pub format_name: String,
    /// `Ok` when the pipeline can decode it, else the reason.
    pub format: Result<SourceFormat, String>,
}

/// Decoded source texture: the file's own mip levels, level 0 first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedDds {
    pub format: SourceFormat,
    pub mips: Vec<Rgba8Image>,
}

/// Pixel-format fields straight from the file: ddsfile drops flags it does not know
/// (palette-indexed) and the bit count of non-RGB formats (luminance).
struct RawPixelFormat {
    flags: u32,
    bits: u32,
}

/// Magic plus the 124-byte header.
const HEADER_END: usize = 128;
const PF_LUMINANCE: u32 = 0x2_0000;
const PF_INDEXED8: u32 = 0x20;
const PF_ALPHA_PIXELS: u32 = 0x1;

fn parse(bytes: &[u8]) -> Result<(Dds, RawPixelFormat), TextureError> {
    let bad = |e: ddsfile::Error| TextureError::Dds(e.to_string());
    let field = |at: usize| {
        bytes
            .get(at..at + 4)
            .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    // Some files in the install (the *_hb_* and tower textures) write dwSize = 24 but have a
    // normal 124-byte header; the game accepts them, so patch the field rather than reject.
    let dds = if bytes.len() >= HEADER_END && field(4) != 124 {
        let mut head = [0u8; HEADER_END];
        head.copy_from_slice(&bytes[..HEADER_END]);
        head[4..8].copy_from_slice(&124u32.to_le_bytes());
        Dds::read(Cursor::new(head).chain(&bytes[HEADER_END..])).map_err(bad)?
    } else {
        Dds::read(Cursor::new(bytes)).map_err(bad)?
    };
    let raw = RawPixelFormat {
        flags: field(80),
        bits: field(88),
    };
    Ok((dds, raw))
}

fn classify(dds: &Dds, raw: &RawPixelFormat) -> (String, Result<SourceFormat, String>) {
    if let Some(h10) = &dds.header10 {
        let name = format!("{:?}", h10.dxgi_format);
        let fmt = match h10.dxgi_format {
            DxgiFormat::BC1_UNorm | DxgiFormat::BC1_UNorm_sRGB => Some(SourceFormat::Dxt1),
            DxgiFormat::BC2_UNorm | DxgiFormat::BC2_UNorm_sRGB => Some(SourceFormat::Dxt3),
            DxgiFormat::BC3_UNorm | DxgiFormat::BC3_UNorm_sRGB => Some(SourceFormat::Dxt5),
            DxgiFormat::R8G8B8A8_UNorm | DxgiFormat::R8G8B8A8_UNorm_sRGB => {
                Some(SourceFormat::A8B8G8R8)
            }
            DxgiFormat::B8G8R8A8_UNorm | DxgiFormat::B8G8R8A8_UNorm_sRGB => {
                Some(SourceFormat::A8R8G8B8)
            }
            DxgiFormat::B8G8R8X8_UNorm | DxgiFormat::B8G8R8X8_UNorm_sRGB => {
                Some(SourceFormat::X8R8G8B8)
            }
            _ => None,
        };
        return (name.clone(), fmt.ok_or(name));
    }
    match dds.get_d3d_format() {
        Some(d3d) => {
            let name = format!("{d3d:?}");
            let fmt = match d3d {
                D3DFormat::DXT1 => Some(SourceFormat::Dxt1),
                D3DFormat::DXT3 => Some(SourceFormat::Dxt3),
                D3DFormat::DXT5 => Some(SourceFormat::Dxt5),
                D3DFormat::A8R8G8B8 => Some(SourceFormat::A8R8G8B8),
                D3DFormat::X8R8G8B8 => Some(SourceFormat::X8R8G8B8),
                D3DFormat::A8B8G8R8 => Some(SourceFormat::A8B8G8R8),
                D3DFormat::X8B8G8R8 => Some(SourceFormat::X8B8G8R8),
                D3DFormat::R8G8B8 => Some(SourceFormat::R8G8B8),
                D3DFormat::L8 => Some(SourceFormat::L8),
                D3DFormat::A8L8 => Some(SourceFormat::A8L8),
                _ => None,
            };
            (name.clone(), fmt.ok_or(name))
        }
        None if raw.flags & PF_LUMINANCE != 0 && dds.header.spf.fourcc.is_none() => {
            let alpha = raw.flags & PF_ALPHA_PIXELS != 0;
            match (raw.bits, alpha) {
                (8, false) => ("L8".into(), Ok(SourceFormat::L8)),
                (16, true) => ("A8L8".into(), Ok(SourceFormat::A8L8)),
                // The game's "-index" UI textures: 16-bit palette indices, not colors.
                (16, false) => ("L16 (UI index)".into(), Err("L16 (UI index)".into())),
                (bits, _) => {
                    let name = format!("luminance {bits} bpp");
                    (name.clone(), Err(name))
                }
            }
        }
        None => {
            let name = match &dds.header.spf.fourcc {
                Some(cc) => format!("fourcc {:#x}", cc.0),
                None if raw.flags & PF_INDEXED8 != 0 => "P8 palette-indexed".into(),
                None => format!("pixel format flags {:#x}, {} bpp", raw.flags, raw.bits),
            };
            (name.clone(), Err(name))
        }
    }
}

fn check_size(width: u32, height: u32) -> Result<(), TextureError> {
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(TextureError::BadSize { width, height });
    }
    Ok(())
}

/// Reads only the header.
pub fn inspect_dds(bytes: &[u8]) -> Result<DdsInfo, TextureError> {
    let (dds, raw) = parse(bytes)?;
    let (format_name, format) = classify(&dds, &raw);
    Ok(DdsInfo {
        width: dds.header.width,
        height: dds.header.height,
        mip_count: dds.get_num_mipmap_levels(),
        format_name,
        format,
    })
}

/// Decodes every stored mip level of a 2D texture to RGBA8.
///
/// Cubemaps, volumes and formats outside [`SourceFormat`] give `Unsupported`; missing pixel
/// data gives `Truncated`. A header that declares more levels than a full chain is clamped.
pub fn decode_dds(bytes: &[u8]) -> Result<DecodedDds, TextureError> {
    let (dds, raw) = parse(bytes)?;
    let (name, format) = classify(&dds, &raw);
    let format = format.map_err(TextureError::Unsupported)?;
    if dds.header.caps2.contains(Caps2::CUBEMAP) || dds.get_num_array_layers() != 1 {
        return Err(TextureError::Unsupported(format!(
            "cubemap or array ({name})"
        )));
    }
    if dds.header.caps2.contains(Caps2::VOLUME) || dds.header.depth.is_some_and(|d| d > 1) {
        return Err(TextureError::Unsupported(format!("volume ({name})")));
    }
    let (width, height) = (dds.header.width, dds.header.height);
    check_size(width, height)?;

    let levels = (dds.get_num_mipmap_levels().max(1) as usize).min(full_chain_len(width, height));
    let dims = |level: usize| ((width >> level).max(1), (height >> level).max(1));
    let need: u64 = (0..levels)
        .map(|l| {
            let (w, h) = dims(l);
            format.mip_bytes(w, h)
        })
        .sum();
    let have = dds.data.len() as u64;
    if have < need {
        return Err(TextureError::Truncated { need, have });
    }

    let mut offset = 0usize;
    let mut mips = Vec::with_capacity(levels);
    for level in 0..levels {
        let (w, h) = dims(level);
        let size = format.mip_bytes(w, h) as usize;
        let src = &dds.data[offset..offset + size];
        offset += size;
        mips.push(decode_level(format, w, h, src));
    }
    Ok(DecodedDds { format, mips })
}

/// `src` is exactly one level; the caller has checked its length.
fn decode_level(format: SourceFormat, width: u32, height: u32, src: &[u8]) -> Rgba8Image {
    let (w, h) = (width as usize, height as usize);
    let mut out = vec![0u8; w * h * 4];
    if let Some(block_bytes) = format.block_bytes() {
        let decode: fn(&[u8], &mut [u8], usize) = match format {
            SourceFormat::Dxt1 => bcdec_rs::bc1,
            SourceFormat::Dxt3 => bcdec_rs::bc2,
            _ => bcdec_rs::bc3,
        };
        decode_blocks(src, block_bytes, w, h, &mut out, decode);
    } else {
        let bpp = format.pixel_bytes();
        for (px, dst) in src.chunks_exact(bpp).zip(out.chunks_exact_mut(4)) {
            dst.copy_from_slice(&match format {
                SourceFormat::A8R8G8B8 => [px[2], px[1], px[0], px[3]],
                SourceFormat::X8R8G8B8 | SourceFormat::R8G8B8 => [px[2], px[1], px[0], 255],
                SourceFormat::A8B8G8R8 => [px[0], px[1], px[2], px[3]],
                SourceFormat::X8B8G8R8 => [px[0], px[1], px[2], 255],
                SourceFormat::L8 => [px[0], px[0], px[0], 255],
                SourceFormat::A8L8 => [px[0], px[0], px[0], px[1]],
                _ => unreachable!("block formats handled above"),
            });
        }
    }
    Rgba8Image::new(width, height, out)
}

/// Decodes 4x4 blocks and crops them at the right and bottom edges.
pub(super) fn decode_blocks(
    src: &[u8],
    block_bytes: usize,
    w: usize,
    h: usize,
    out: &mut [u8],
    decode: fn(&[u8], &mut [u8], usize),
) {
    let blocks_x = w.div_ceil(4);
    for (i, block) in src.chunks_exact(block_bytes).enumerate() {
        let (bx, by) = (i % blocks_x * 4, i / blocks_x * 4);
        let mut tmp = [0u8; 64];
        decode(block, &mut tmp, 16);
        for row in 0..4.min(h - by) {
            let n = 4.min(w - bx);
            let d = ((by + row) * w + bx) * 4;
            out[d..d + n * 4].copy_from_slice(&tmp[row * 16..row * 16 + n * 4]);
        }
    }
}

// ---- Native-layout reader -----------------------------------
//
// `decode_dds` above turns everything into RGBA8 for the B1 encoders. The world textures keep
// the bytes of the original instead: DXT blocks as they are, L8/A8L8 as `R8`/`Rg8`, 8-bit RGB
// as `Rgba8`, 16-bit data as raw u16. Cubes and volumes are read by `caps2`.

/// Largest side of a volume; the same cap as `TextureBlob` validation.
const MAX_SIDE_3D: u32 = 256;
const CUBE_FACES: usize = 6;

/// Pixel layout of a [`RawDds`] after the swizzle to memory order R, G, B, A.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawFormat {
    /// DXT1, DXT3, DXT5 blocks as stored.
    Bc1,
    Bc2,
    Bc3,
    /// 8-bit R, G, B, A (from A8R8G8B8 "RGB32A", X8R8G8B8 with A = 255, and the 8-bit variants).
    Rgba8,
    /// L8.
    R8,
    /// A8L8: luminance first, alpha second, as stored.
    Rg8,
    /// A16B16G16R16: R, G, B, A as u16 little endian (terrain pieces, `.lean`).
    Rgba16,
    /// L16: u16 little endian.
    R16,
}

impl RawFormat {
    /// The blob format that holds this layout, if there is one (the 16-bit unorm layouts go to
    /// their own containers).
    pub fn texture_format(self) -> Option<TextureFormat> {
        match self {
            Self::Bc1 => Some(TextureFormat::Bc1),
            Self::Bc2 => Some(TextureFormat::Bc2),
            Self::Bc3 => Some(TextureFormat::Bc3),
            Self::Rgba8 => Some(TextureFormat::Rgba8),
            Self::R8 => Some(TextureFormat::R8),
            Self::Rg8 => Some(TextureFormat::Rg8),
            Self::Rgba16 | Self::R16 => None,
        }
    }
}

/// A DDS read without changing the pixel data. Level 0 first; the layout of `mips[i]` is the one
/// of `TextureBlob::mips` (cube: six faces back to back, volume: all slices back to back).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawDds {
    pub format: RawFormat,
    pub dim: TexDim,
    pub width: u32,
    pub height: u32,
    /// 1 unless `dim` is `D3`.
    pub depth: u32,
    pub mips: Vec<Vec<u8>>,
}

impl RawDds {
    /// The blob for this texture; `srgb` is always false for world textures (AB-D2).
    pub fn into_blob(self) -> Result<TextureBlob, TextureError> {
        let format = self.format.texture_format().ok_or_else(|| {
            TextureError::Unsupported(format!("{:?} has no blob format", self.format))
        })?;
        Ok(TextureBlob {
            format,
            dim: self.dim,
            srgb: false,
            width: self.width,
            height: self.height,
            depth: self.depth,
            mips: self.mips,
        })
    }
}

/// File layouts [`read_dds`] knows, with their memory order named as in D3D (high bits first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Src {
    Dxt1,
    Dxt3,
    Dxt5,
    A8R8G8B8,
    X8R8G8B8,
    A8B8G8R8,
    X8B8G8R8,
    R8G8B8,
    L8,
    A8L8,
    A16B16G16R16,
    L16,
}

impl Src {
    fn raw_format(self) -> RawFormat {
        match self {
            Self::Dxt1 => RawFormat::Bc1,
            Self::Dxt3 => RawFormat::Bc2,
            Self::Dxt5 => RawFormat::Bc3,
            Self::A8R8G8B8 | Self::X8R8G8B8 | Self::A8B8G8R8 | Self::X8B8G8R8 | Self::R8G8B8 => {
                RawFormat::Rgba8
            }
            Self::L8 => RawFormat::R8,
            Self::A8L8 => RawFormat::Rg8,
            Self::A16B16G16R16 => RawFormat::Rgba16,
            Self::L16 => RawFormat::R16,
        }
    }

    /// `(texels per block side, bytes per block or texel)` as stored in the file.
    fn file_cell(self) -> (u32, u32) {
        match self {
            Self::Dxt1 => (4, 8),
            Self::Dxt3 | Self::Dxt5 => (4, 16),
            Self::A8R8G8B8 | Self::X8R8G8B8 | Self::A8B8G8R8 | Self::X8B8G8R8 => (1, 4),
            Self::R8G8B8 => (1, 3),
            Self::L8 => (1, 1),
            Self::A8L8 | Self::L16 => (1, 2),
            Self::A16B16G16R16 => (1, 8),
        }
    }

    /// Bytes of one 2D slice of a level as stored in the file.
    fn slice_bytes(self, w: u32, h: u32) -> u64 {
        let (dim, bytes) = self.file_cell();
        u64::from(w.div_ceil(dim)) * u64::from(h.div_ceil(dim)) * u64::from(bytes)
    }

    /// One level or slice in file layout to the output layout.
    fn convert(self, src: &[u8]) -> Vec<u8> {
        match self {
            Self::A8R8G8B8 => src
                .chunks_exact(4)
                .flat_map(|p| [p[2], p[1], p[0], p[3]])
                .collect(),
            Self::X8R8G8B8 => src
                .chunks_exact(4)
                .flat_map(|p| [p[2], p[1], p[0], 255])
                .collect(),
            Self::X8B8G8R8 => src
                .chunks_exact(4)
                .flat_map(|p| [p[0], p[1], p[2], 255])
                .collect(),
            Self::R8G8B8 => src
                .chunks_exact(3)
                .flat_map(|p| [p[2], p[1], p[0], 255])
                .collect(),
            Self::Dxt1
            | Self::Dxt3
            | Self::Dxt5
            | Self::A8B8G8R8
            | Self::L8
            | Self::A8L8
            | Self::A16B16G16R16
            | Self::L16 => src.to_vec(),
        }
    }
}

fn classify_src(dds: &Dds, raw: &RawPixelFormat) -> Result<Src, String> {
    if let Some(h10) = &dds.header10 {
        return match h10.dxgi_format {
            DxgiFormat::BC1_UNorm | DxgiFormat::BC1_UNorm_sRGB => Ok(Src::Dxt1),
            DxgiFormat::BC2_UNorm | DxgiFormat::BC2_UNorm_sRGB => Ok(Src::Dxt3),
            DxgiFormat::BC3_UNorm | DxgiFormat::BC3_UNorm_sRGB => Ok(Src::Dxt5),
            DxgiFormat::R8G8B8A8_UNorm | DxgiFormat::R8G8B8A8_UNorm_sRGB => Ok(Src::A8B8G8R8),
            DxgiFormat::B8G8R8A8_UNorm | DxgiFormat::B8G8R8A8_UNorm_sRGB => Ok(Src::A8R8G8B8),
            DxgiFormat::B8G8R8X8_UNorm | DxgiFormat::B8G8R8X8_UNorm_sRGB => Ok(Src::X8R8G8B8),
            DxgiFormat::R8_UNorm => Ok(Src::L8),
            DxgiFormat::R8G8_UNorm => Ok(Src::A8L8),
            DxgiFormat::R16G16B16A16_UNorm => Ok(Src::A16B16G16R16),
            DxgiFormat::R16_UNorm => Ok(Src::L16),
            other => Err(format!("{other:?}")),
        };
    }
    match dds.get_d3d_format() {
        Some(D3DFormat::DXT1) => Ok(Src::Dxt1),
        Some(D3DFormat::DXT3) => Ok(Src::Dxt3),
        Some(D3DFormat::DXT5) => Ok(Src::Dxt5),
        Some(D3DFormat::A8R8G8B8) => Ok(Src::A8R8G8B8),
        Some(D3DFormat::X8R8G8B8) => Ok(Src::X8R8G8B8),
        Some(D3DFormat::A8B8G8R8) => Ok(Src::A8B8G8R8),
        Some(D3DFormat::X8B8G8R8) => Ok(Src::X8B8G8R8),
        Some(D3DFormat::R8G8B8) => Ok(Src::R8G8B8),
        // A8 (alpha only, UI font glyphs) has the byte layout of L8; the value is alpha.
        Some(D3DFormat::L8 | D3DFormat::A8) => Ok(Src::L8),
        Some(D3DFormat::A8L8) => Ok(Src::A8L8),
        Some(D3DFormat::A16B16G16R16) => Ok(Src::A16B16G16R16),
        Some(D3DFormat::L16) => Ok(Src::L16),
        Some(other) => Err(format!("{other:?}")),
        None if raw.flags & PF_LUMINANCE != 0 && dds.header.spf.fourcc.is_none() => {
            match (raw.bits, raw.flags & PF_ALPHA_PIXELS != 0) {
                (8, false) => Ok(Src::L8),
                (16, true) => Ok(Src::A8L8),
                (16, false) => Ok(Src::L16),
                (bits, _) => Err(format!("luminance {bits} bpp")),
            }
        }
        None => Err(classify(dds, raw).0),
    }
}

/// Reads a DDS keeping the pixel data of the file (see the section comment above).
///
/// Cubes and volumes are recognised by `caps2` (or the DX10 header). The file stores a cube
/// face by face, each face with its whole mip chain; the result groups it by level instead,
/// faces in file order (+X, -X, +Y, -Y, +Z, -Z). A header that declares more levels than a full
/// chain is clamped; sizes are checked in `u64` against the data actually present before
/// anything is allocated.
pub fn read_dds(bytes: &[u8]) -> Result<RawDds, TextureError> {
    let (dds, raw) = parse(bytes)?;
    let src = classify_src(&dds, &raw).map_err(TextureError::Unsupported)?;

    let h10 = dds.header10.as_ref();
    let is_cube = dds.header.caps2.contains(Caps2::CUBEMAP)
        || h10.is_some_and(|h| h.misc_flag.contains(ddsfile::MiscFlag::TEXTURECUBE));
    let is_volume = !is_cube
        && (dds.header.caps2.contains(Caps2::VOLUME)
            || h10.is_some_and(|h| {
                h.resource_dimension == ddsfile::D3D10ResourceDimension::Texture3D
            }));
    if h10.is_some_and(|h| h.array_size > 1) {
        return Err(TextureError::Unsupported("texture array".into()));
    }
    if is_cube && h10.is_none() && !dds.header.caps2.contains(Caps2::CUBEMAP_ALLFACES) {
        return Err(TextureError::Unsupported(
            "cubemap without all six faces".into(),
        ));
    }

    let (width, height) = (dds.header.width, dds.header.height);
    check_size(width, height)?;
    let depth = if is_volume {
        dds.header.depth.unwrap_or(1).max(1)
    } else {
        1
    };
    if is_cube && width != height {
        return Err(TextureError::Unsupported(format!(
            "non-square cubemap {width}x{height}"
        )));
    }
    if is_volume && (width > MAX_SIDE_3D || height > MAX_SIDE_3D || depth > MAX_SIDE_3D) {
        return Err(TextureError::Unsupported(format!(
            "volume {width}x{height}x{depth} is larger than {MAX_SIDE_3D}"
        )));
    }

    let longest = if is_volume {
        width.max(height).max(depth)
    } else {
        width.max(height)
    };
    let levels = (dds.get_num_mipmap_levels().max(1) as usize).min(longest.ilog2() as usize + 1);
    let side = |v: u32, l: usize| (v >> l).max(1);
    // Bytes of one level in file layout for one face (cube) or all slices (volume).
    let level_bytes = |l: usize| -> u64 {
        src.slice_bytes(side(width, l), side(height, l)) * u64::from(side(depth, l))
    };
    let per_face: u64 = (0..levels).map(level_bytes).sum();
    let faces = if is_cube { CUBE_FACES } else { 1 };
    let need = per_face * faces as u64;
    let have = dds.data.len() as u64;
    if have < need {
        return Err(TextureError::Truncated { need, have });
    }

    let mut mips = Vec::with_capacity(levels);
    let mut before = 0u64;
    for l in 0..levels {
        let size = level_bytes(l) as usize;
        let mut level = Vec::new();
        for face in 0..faces {
            // Face-major in the file: face `f` starts after `f` whole chains.
            let at = face * per_face as usize + before as usize;
            level.extend(src.convert(&dds.data[at..at + size]));
        }
        before += size as u64;
        mips.push(level);
    }
    Ok(RawDds {
        format: src.raw_format(),
        dim: match (is_cube, is_volume) {
            (true, _) => TexDim::Cube,
            (_, true) => TexDim::D3,
            _ => TexDim::D2,
        },
        width,
        height,
        depth,
        mips,
    })
}

#[cfg(test)]
mod tests {
    use ddsfile::{NewD3dParams, NewDxgiParams};

    use super::*;

    fn d3d(width: u32, height: u32, format: D3DFormat, mips: u32) -> Dds {
        Dds::new_d3d(NewD3dParams {
            height,
            width,
            depth: None,
            format,
            mipmap_levels: Some(mips),
            caps2: None,
        })
        .unwrap()
    }

    fn to_bytes(dds: &Dds) -> Vec<u8> {
        let mut v = Vec::new();
        dds.write(&mut v).unwrap();
        v
    }

    #[test]
    fn dxt1_known_block() {
        // color0 = pure red (0xF800), color1 = black, all indices 0 -> opaque red.
        let mut dds = d3d(4, 4, D3DFormat::DXT1, 1);
        dds.data.copy_from_slice(&[0x00, 0xF8, 0, 0, 0, 0, 0, 0]);
        let d = decode_dds(&to_bytes(&dds)).unwrap();
        assert_eq!(d.format, SourceFormat::Dxt1);
        assert_eq!(d.mips.len(), 1);
        assert!(
            d.mips[0]
                .pixels
                .chunks_exact(4)
                .all(|p| p == [255, 0, 0, 255])
        );
    }

    #[test]
    fn dxt5_known_block_with_alpha() {
        // alpha0 = 255, alpha1 = 0, all alpha indices 0 -> 255; colors as above but green.
        let mut dds = d3d(4, 4, D3DFormat::DXT5, 1);
        let mut block = [0u8; 16];
        block[0] = 128; // alpha0
        block[1] = 0; // alpha1, indices 0 -> alpha0
        block[8..10].copy_from_slice(&0x07E0u16.to_le_bytes()); // green
        dds.data.copy_from_slice(&block);
        let d = decode_dds(&to_bytes(&dds)).unwrap();
        assert!(
            d.mips[0]
                .pixels
                .chunks_exact(4)
                .all(|p| p == [0, 255, 0, 128])
        );
    }

    #[test]
    fn dxt1_mip_chain_and_odd_size_crop() {
        // 6x5 needs 2x2 blocks; mips 3x2 (1x1 blocks... 3x2 -> 1 block), 1x1.
        let mut dds = d3d(6, 5, D3DFormat::DXT1, 3);
        let n = dds.data.len();
        assert_eq!(n, 4 * 8 + 8 + 8);
        for b in dds.data.chunks_exact_mut(8) {
            b[..2].copy_from_slice(&0x001Fu16.to_le_bytes()); // blue
        }
        let d = decode_dds(&to_bytes(&dds)).unwrap();
        let dims: Vec<_> = d.mips.iter().map(|m| (m.width, m.height)).collect();
        assert_eq!(dims, [(6, 5), (3, 2), (1, 1)]);
        assert!(
            d.mips
                .iter()
                .all(|m| m.pixels.chunks_exact(4).all(|p| p == [0, 0, 255, 255]))
        );
    }

    #[test]
    fn uncompressed_channel_order() {
        let mut dds = d3d(2, 1, D3DFormat::A8R8G8B8, 1);
        // B, G, R, A in memory.
        dds.data.copy_from_slice(&[3, 2, 1, 4, 30, 20, 10, 40]);
        let d = decode_dds(&to_bytes(&dds)).unwrap();
        assert_eq!(d.mips[0].pixels, [1, 2, 3, 4, 10, 20, 30, 40]);

        let mut dds = d3d(1, 1, D3DFormat::X8R8G8B8, 1);
        dds.data.copy_from_slice(&[3, 2, 1, 0]);
        let d = decode_dds(&to_bytes(&dds)).unwrap();
        assert_eq!(d.mips[0].pixels, [1, 2, 3, 255]);

        // ddsfile does not write L8 correctly, so patch the pixel format by hand.
        let mut dds = d3d(1, 1, D3DFormat::A8R8G8B8, 1);
        dds.data.truncate(1);
        dds.data[0] = 9;
        let bytes = with_pixel_format(to_bytes(&dds), PF_LUMINANCE, 0, 8, [0xff, 0, 0, 0]);
        let d = decode_dds(&bytes).unwrap();
        assert_eq!(d.format, SourceFormat::L8);
        assert_eq!(d.mips[0].pixels, [9, 9, 9, 255]);
    }

    const PF_FOURCC: u32 = 0x4;

    /// Overwrites the 32-byte pixel format struct (file offset 76).
    fn with_pixel_format(
        mut bytes: Vec<u8>,
        flags: u32,
        fourcc: u32,
        bits: u32,
        masks: [u32; 4],
    ) -> Vec<u8> {
        let fields = [flags, fourcc, bits, masks[0], masks[1], masks[2], masks[3]];
        for (i, v) in fields.iter().enumerate() {
            bytes[80 + i * 4..84 + i * 4].copy_from_slice(&v.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn header_size_field_quirk_is_tolerated() {
        let mut dds = d3d(4, 4, D3DFormat::DXT1, 1);
        dds.data.copy_from_slice(&[0x00, 0xF8, 0, 0, 0, 0, 0, 0]);
        let mut bytes = to_bytes(&dds);
        bytes[4..8].copy_from_slice(&24u32.to_le_bytes());
        let d = decode_dds(&bytes).unwrap();
        assert_eq!(d.mips[0].pixels[..4], [255, 0, 0, 255]);
    }

    #[test]
    fn dx10_bc3_is_supported() {
        let dds = Dds::new_dxgi(NewDxgiParams {
            height: 4,
            width: 4,
            depth: None,
            format: DxgiFormat::BC3_UNorm,
            mipmap_levels: Some(1),
            array_layers: None,
            caps2: None,
            is_cubemap: false,
            resource_dimension: ddsfile::D3D10ResourceDimension::Texture2D,
            alpha_mode: ddsfile::AlphaMode::Unknown,
        })
        .unwrap();
        assert_eq!(
            decode_dds(&to_bytes(&dds)).unwrap().format,
            SourceFormat::Dxt5
        );
    }

    #[test]
    fn unsupported_formats_are_typed_errors() {
        // D3DFMT numbers used as fourcc by the game: 114 = R32F, 36 = A16B16G16R16.
        for (fourcc, name) in [(114, "R32F"), (36, "A16B16G16R16")] {
            let bytes = to_bytes(&d3d(4, 4, D3DFormat::DXT1, 1));
            let bytes = with_pixel_format(bytes, PF_FOURCC, fourcc, 0, [0; 4]);
            match decode_dds(&bytes) {
                Err(TextureError::Unsupported(what)) => assert_eq!(what, name),
                other => panic!("{other:?}"),
            }
            let info = inspect_dds(&bytes).unwrap();
            assert_eq!(info.format_name, name);
            assert!(info.format.is_err());
        }
        // 8-bit palette-indexed (UI "-index" textures) is named, not decoded.
        let bytes = to_bytes(&d3d(4, 4, D3DFormat::DXT1, 1));
        let bytes = with_pixel_format(bytes, 0x20, 0, 8, [0; 4]);
        assert!(matches!(
            decode_dds(&bytes),
            Err(TextureError::Unsupported(_))
        ));
    }

    #[test]
    fn cubemap_is_unsupported() {
        let dds = Dds::new_d3d(NewD3dParams {
            height: 4,
            width: 4,
            depth: None,
            format: D3DFormat::DXT1,
            mipmap_levels: Some(1),
            caps2: Some(Caps2::CUBEMAP | Caps2::CUBEMAP_ALLFACES),
        })
        .unwrap();
        assert!(matches!(
            decode_dds(&to_bytes(&dds)),
            Err(TextureError::Unsupported(_))
        ));
    }

    #[test]
    fn truncated_and_garbage_are_errors() {
        let dds = d3d(8, 8, D3DFormat::DXT5, 4);
        let full = to_bytes(&dds);
        for cut in [1, 16, full.len() - 128] {
            let r = decode_dds(&full[..full.len() - cut]);
            assert!(matches!(r, Err(TextureError::Truncated { .. })), "{r:?}");
        }
        assert!(matches!(decode_dds(b"nope"), Err(TextureError::Dds(_))));
        assert!(matches!(decode_dds(&[]), Err(TextureError::Dds(_))));
        let mut bad = full.clone();
        bad[12..16].copy_from_slice(&0u32.to_le_bytes()); // width = 0
        assert!(matches!(
            decode_dds(&bad),
            Err(TextureError::BadSize { .. } | TextureError::Dds(_))
        ));
    }

    #[test]
    fn oversized_header_is_rejected_before_allocating() {
        let mut bytes = to_bytes(&d3d(4, 4, D3DFormat::DXT1, 1));
        bytes[12..16].copy_from_slice(&0x7FFF_FFFFu32.to_le_bytes()); // width
        let r = decode_dds(&bytes);
        assert!(matches!(r, Err(TextureError::BadSize { .. })), "{r:?}");
    }

    // ---- read_dds ------------------------------------------------------------------------

    /// ddsfile allocates one face for a new cubemap; the file has six.
    fn cube_dds(side: u32, format: D3DFormat, mips: u32) -> Dds {
        let mut dds = Dds::new_d3d(NewD3dParams {
            height: side,
            width: side,
            depth: None,
            format,
            mipmap_levels: Some(mips),
            caps2: Some(Caps2::CUBEMAP | Caps2::CUBEMAP_ALLFACES),
        })
        .unwrap();
        dds.data.resize(dds.data.len() * 6, 0);
        dds
    }

    #[test]
    fn cube_128_with_8_mips_is_regrouped_by_level_in_face_order() {
        let mut dds = cube_dds(128, D3DFormat::DXT1, 8);
        // DXT1 level sizes for 128..1: 32x32 blocks ... 1 block, 8 bytes each.
        let level_bytes: Vec<usize> = (0..8)
            .map(|l| ((128usize >> l).div_ceil(4)).max(1).pow(2) * 8)
            .collect();
        let per_face: usize = level_bytes.iter().sum();
        assert_eq!(dds.data.len(), 6 * per_face, "ddsfile cube data size");
        // Byte = face * 16 + level, so every (face, level) chunk is recognisable.
        let mut at = 0;
        for face in 0..6 {
            for (level, &n) in level_bytes.iter().enumerate() {
                dds.data[at..at + n].fill((face * 16 + level) as u8);
                at += n;
            }
        }
        let r = read_dds(&to_bytes(&dds)).unwrap();
        assert_eq!((r.dim, r.format), (TexDim::Cube, RawFormat::Bc1));
        assert_eq!((r.width, r.height, r.depth), (128, 128, 1));
        assert_eq!(r.mips.len(), 8);
        for (level, mip) in r.mips.iter().enumerate() {
            let n = level_bytes[level];
            assert_eq!(mip.len(), 6 * n, "level {level}");
            for face in 0..6 {
                assert!(
                    mip[face * n..(face + 1) * n]
                        .iter()
                        .all(|&b| b == (face * 16 + level) as u8),
                    "level {level} face {face}"
                );
            }
        }
        // And it is a valid blob: six faces per level, Bc1.
        let blob = r.into_blob().unwrap();
        assert_eq!(TextureBlob::decode(&blob.encode()).unwrap(), blob);
    }

    #[test]
    fn cube_with_wrong_header_size_field_is_read() {
        let mut bytes = to_bytes(&cube_dds(4, D3DFormat::DXT1, 1));
        bytes[4..8].copy_from_slice(&24u32.to_le_bytes());
        let r = read_dds(&bytes).unwrap();
        assert_eq!(
            (r.dim, r.mips.len(), r.mips[0].len()),
            (TexDim::Cube, 1, 48)
        );
    }

    #[test]
    fn cube_errors() {
        // Missing faces in the data.
        let full = to_bytes(&cube_dds(8, D3DFormat::DXT1, 2));
        assert!(matches!(
            read_dds(&full[..full.len() - 8]),
            Err(TextureError::Truncated { .. })
        ));
        // Some faces missing in caps2.
        let mut dds = cube_dds(4, D3DFormat::DXT1, 1);
        dds.header.caps2 = Caps2::CUBEMAP | Caps2::CUBEMAP_POSITIVEX;
        assert!(matches!(
            read_dds(&to_bytes(&dds)),
            Err(TextureError::Unsupported(_))
        ));
        // Not square.
        let mut bytes = to_bytes(&cube_dds(8, D3DFormat::DXT1, 1));
        bytes[12..16].copy_from_slice(&4u32.to_le_bytes());
        assert!(matches!(
            read_dds(&bytes),
            Err(TextureError::Unsupported(_))
        ));
    }

    #[test]
    fn volume_slices_stay_together_per_level() {
        let dds = Dds::new_d3d(NewD3dParams {
            height: 4,
            width: 4,
            depth: Some(4),
            format: D3DFormat::A8R8G8B8,
            mipmap_levels: Some(3),
            caps2: Some(Caps2::VOLUME),
        })
        .unwrap();
        // Level sizes: 4x4x4, 2x2x2, 1x1x1 texels of 4 bytes.
        assert_eq!(dds.data.len(), (64 + 8 + 1) * 4, "ddsfile volume data size");
        let mut bytes = to_bytes(&dds);
        let data_at = HEADER_END;
        for (i, b) in bytes[data_at..].iter_mut().enumerate() {
            *b = i as u8;
        }
        let r = read_dds(&bytes).unwrap();
        assert_eq!((r.dim, r.format), (TexDim::D3, RawFormat::Rgba8));
        assert_eq!((r.width, r.height, r.depth), (4, 4, 4));
        let lens: Vec<_> = r.mips.iter().map(Vec::len).collect();
        assert_eq!(lens, [256, 32, 4]);
        // A8R8G8B8 is swizzled to R, G, B, A: first texel bytes were 0, 1, 2, 3 (B, G, R, A).
        assert_eq!(r.mips[0][..4], [2, 1, 0, 3]);
        let blob = r.into_blob().unwrap();
        assert_eq!(TextureBlob::decode(&blob.encode()).unwrap(), blob);
    }

    #[test]
    fn oversized_volume_and_headers_are_errors() {
        let mut dds = Dds::new_d3d(NewD3dParams {
            height: 4,
            width: 4,
            depth: Some(2),
            format: D3DFormat::A8R8G8B8,
            mipmap_levels: Some(1),
            caps2: Some(Caps2::VOLUME),
        })
        .unwrap();
        dds.header.depth = Some(0x4000_0000);
        assert!(matches!(
            read_dds(&to_bytes(&dds)),
            Err(TextureError::Unsupported(_))
        ));
        // 16384x16384 with no data: Truncated (u64 sizes), no allocation, no panic.
        let mut bytes = to_bytes(&d3d(4, 4, D3DFormat::A8R8G8B8, 1));
        bytes[12..16].copy_from_slice(&16384u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&16384u32.to_le_bytes());
        let r = read_dds(&bytes);
        assert!(matches!(r, Err(TextureError::Truncated { .. })), "{r:?}");
        // Width beyond the cap.
        bytes[12..16].copy_from_slice(&0x7FFF_FFFFu32.to_le_bytes());
        assert!(matches!(
            read_dds(&bytes),
            Err(TextureError::BadSize { .. })
        ));
    }

    #[test]
    fn raw_formats_keep_the_bytes_of_the_file() {
        // DXT1/3/5 blocks are copied as they are, mips included.
        for (format, block) in [
            (D3DFormat::DXT1, 8usize),
            (D3DFormat::DXT3, 16),
            (D3DFormat::DXT5, 16),
        ] {
            let mut dds = d3d(8, 4, format, 3);
            for (i, b) in dds.data.iter_mut().enumerate() {
                *b = (i * 7) as u8;
            }
            let r = read_dds(&to_bytes(&dds)).unwrap();
            assert_eq!(r.mips.concat(), dds.data);
            assert_eq!(r.mips[0].len(), 2 * block);
        }

        // RGB32A: B, G, R, A in the file, R, G, B, A in the output.
        let mut dds = d3d(2, 1, D3DFormat::A8R8G8B8, 1);
        dds.data.copy_from_slice(&[3, 2, 1, 4, 30, 20, 10, 40]);
        let r = read_dds(&to_bytes(&dds)).unwrap();
        assert_eq!(r.format, RawFormat::Rgba8);
        assert_eq!(r.mips[0], [1, 2, 3, 4, 10, 20, 30, 40]);

        // X8R8G8B8: alpha forced to 255.
        let mut dds = d3d(1, 1, D3DFormat::X8R8G8B8, 1);
        dds.data.copy_from_slice(&[3, 2, 1, 0]);
        assert_eq!(read_dds(&to_bytes(&dds)).unwrap().mips[0], [1, 2, 3, 255]);

        // A8L8 -> Rg8 as stored (luminance, alpha).
        let mut dds = d3d(2, 1, D3DFormat::A8R8G8B8, 1);
        dds.data.copy_from_slice(&[9, 200, 8, 100, 0, 0, 0, 0]);
        let mut bytes = to_bytes(&dds);
        bytes.truncate(HEADER_END + 4);
        let bytes = with_pixel_format(
            bytes,
            PF_LUMINANCE | PF_ALPHA_PIXELS,
            0,
            16,
            [0xff, 0, 0, 0xff00],
        );
        let r = read_dds(&bytes).unwrap();
        assert_eq!(r.format, RawFormat::Rg8);
        assert_eq!(r.mips[0], [9, 200, 8, 100]);
        assert_eq!(r.into_blob().unwrap().format, TextureFormat::Rg8);

        // L8 -> R8.
        let mut bytes = to_bytes(&d3d(2, 1, D3DFormat::A8R8G8B8, 1));
        bytes.truncate(HEADER_END + 2);
        bytes[HEADER_END..].copy_from_slice(&[5, 6]);
        let bytes = with_pixel_format(bytes, PF_LUMINANCE, 0, 8, [0xff, 0, 0, 0]);
        let r = read_dds(&bytes).unwrap();
        assert_eq!(r.format, RawFormat::R8);
        assert_eq!(r.mips[0], [5, 6]);
        assert_eq!(r.into_blob().unwrap().format, TextureFormat::R8);
    }

    #[test]
    fn sixteen_bit_formats_are_read_as_u16() {
        // A16B16G16R16 is fourcc 36 in this game's files; memory order is R, G, B, A.
        let mut bytes = to_bytes(&d3d(2, 1, D3DFormat::DXT1, 1));
        bytes.truncate(HEADER_END);
        bytes.extend((0..16u8).map(|i| i + 1));
        let bytes = with_pixel_format(bytes, PF_FOURCC, 36, 0, [0; 4]);
        let r = read_dds(&bytes).unwrap();
        assert_eq!(r.format, RawFormat::Rgba16);
        assert_eq!(r.mips[0], (1..=16).collect::<Vec<u8>>());
        // No blob format for it: pieces and `.lean` have their own containers.
        assert!(matches!(r.into_blob(), Err(TextureError::Unsupported(_))));

        // L16: luminance flag, 16 bits, no alpha.
        let mut bytes = to_bytes(&d3d(2, 1, D3DFormat::DXT1, 1));
        bytes.truncate(HEADER_END);
        bytes.extend([1, 2, 3, 4]);
        let bytes = with_pixel_format(bytes, PF_LUMINANCE, 0, 16, [0xffff, 0, 0, 0]);
        let r = read_dds(&bytes).unwrap();
        assert_eq!(r.format, RawFormat::R16);
        assert_eq!(r.mips[0], [1, 2, 3, 4]);
    }

    #[test]
    fn unsupported_raw_formats_are_typed_errors() {
        // R32F (fourcc 114) has no raw layout.
        let bytes = with_pixel_format(
            to_bytes(&d3d(4, 4, D3DFormat::DXT1, 1)),
            PF_FOURCC,
            114,
            0,
            [0; 4],
        );
        assert!(matches!(
            read_dds(&bytes),
            Err(TextureError::Unsupported(_))
        ));
        assert!(matches!(read_dds(b"nope"), Err(TextureError::Dds(_))));
    }
}
