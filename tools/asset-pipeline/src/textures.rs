//! textures: DDS to RGBA8 / BC7 / ASTC 4x4 texture blobs.
//!
//! Pipeline: [`decode_dds`] (source mips to RGBA8), [`extend_mips`] (complete the chain
//! if the file ships a partial one), [`encode_chain`] (per target format).
//! [`convert_dds`] runs all three. [`variants`] adds the BC7 and ASTC variants of a finished pack.
//!
//! Encoder presets (offline batch, so quality over speed):
//! - BC7: ctt `Quality::Slow` (bc7enc-rdo, full mode search with pbit search), perceptual
//!   weights off so the error is plain RGB and PSNR is meaningful for data textures too.
//! - ASTC 4x4: ctt `Quality::Basic` (astcenc "medium", 60). At 4x4 (8 bpp) the search
//!   gains little beyond it, while "thorough" costs several times more.

mod dds;
mod encode;
mod lean;
mod mips;
mod passthrough;
mod probe;
mod quality;
pub mod repack;
mod tga;
pub mod variants;

use asset_format::{TextureBlob, TextureFormat};

pub use dds::{DdsInfo, RawDds, RawFormat, SourceFormat, decode_dds, inspect_dds, read_dds};
pub use encode::encode_chain;
pub use lean::{
    HALF_ERROR, LEAN_HEADER_BYTES, Lean, LeanError, LeanHeader, LeanTextures, lean_textures,
    parse_lean,
};
pub use mips::{extend_mips, full_chain_len};
pub use passthrough::{dds_passthrough, level0_only, variant_format};
pub use probe::{DdsProbe, FormatStat, probe_dds};
pub use quality::{decode_mip, decode_slice, psnr};
pub use tga::{TgaImage, read_tga};

/// Largest side accepted; matches the `TextureBlob` validation cap.
pub const MAX_SIDE: u32 = 16384;

#[derive(Debug, thiserror::Error)]
pub enum TextureError {
    #[error("invalid DDS: {0}")]
    Dds(String),
    /// Valid file, but a format or layout the pipeline does not convert.
    #[error("unsupported DDS: {0}")]
    Unsupported(String),
    #[error("truncated DDS: need {need} bytes of pixel data, have {have}")]
    Truncated { need: u64, have: u64 },
    #[error("bad texture size {width}x{height}")]
    BadSize { width: u32, height: u32 },
    #[error("invalid TGA: {0}")]
    Tga(String),
    #[error("texture encoder: {0}")]
    Encode(String),
}

impl TextureError {
    /// Stable label for grouping failures in reports.
    pub fn reason(&self) -> String {
        match self {
            Self::Dds(_) => "invalid header".into(),
            Self::Unsupported(what) => format!("unsupported: {what}"),
            Self::Truncated { .. } => "truncated data".into(),
            Self::BadSize { .. } => "bad size".into(),
            Self::Tga(_) => "invalid TGA".into(),
            Self::Encode(_) => "encoder failure".into(),
        }
    }
}

/// How the pixel values must be interpreted; picks gamma-correct mip filtering and the
/// sRGB variant of the output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColorSpace {
    Srgb,
    Linear,
}

/// Plain 8-bit RGBA, rows tightly packed, straight (non-premultiplied) alpha.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba8Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Rgba8Image {
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        debug_assert_eq!(pixels.len(), width as usize * height as usize * 4);
        Self {
            width,
            height,
            pixels,
        }
    }
}

/// Guesses the color space from a Civ5 texture file name.
///
/// Data textures are linear: `_sref` (specular/reflection, 852 files), `_blend`, `_h`
/// (heightmaps), `_norm`, `_tang`, `_opac`, `_mask` and similar. Everything else (`_diff`,
/// `_d`, `_t1`, decals, UI, unknown) is color and therefore sRGB. The suffix is the part after
/// the last underscore of the stem, so `warrior_diff.dds` is sRGB and `warrior_sref.dds` is
/// linear. The list was built from the suffix histogram of the install (`_diff` 1115,
/// `_sref` 852, `_blend` 608, `_h` 492, `_t1` 304, `_norm` 257, `_d` 255, `_opac` 152).
pub fn color_space_for(name: &str) -> ColorSpace {
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".dds").unwrap_or(&lower);
    let suffix = stem.rsplit_once('_').map_or(stem, |(_, s)| s);
    const LINEAR: &[&str] = &[
        "sref",
        "spec",
        "norm",
        "nrm",
        "normal",
        "n",
        "h",
        "height",
        "blend",
        "mask",
        "alpha",
        "gloss",
        "rough",
        "ao",
        "irradiance",
        "tang",
        "opac",
        "opacity",
    ];
    // `forest_tile<i>.dds`: L8 tree placement masks, not colors.
    // `waterbumps.dds`: the bump (normal) map of the water surface.
    if LINEAR.contains(&suffix) || stem.starts_with("forest_tile") || stem == "waterbumps" {
        ColorSpace::Linear
    } else {
        ColorSpace::Srgb
    }
}

/// What [`convert_dds_with`] does besides decoding.
#[derive(Debug, Clone, Copy)]
pub struct ConvertOptions {
    pub color_space: ColorSpace,
    /// Complete a partial mip chain (e.g. UI textures with one level) down to 1x1.
    pub generate_mips: bool,
    /// Drop the top levels until the largest side is at most this (a chain without lower
    /// levels is box-filtered down). `None` keeps the source size.
    pub max_side: Option<u32>,
}

/// Removes top levels of `mips` until `max(width, height) <= max_side` (at least 1).
/// Authored lower levels are reused; a one-level chain is downsampled.
pub fn cap_mips(mips: &mut Vec<Rgba8Image>, color_space: ColorSpace, max_side: u32) {
    let max_side = max_side.max(1);
    while mips
        .first()
        .is_some_and(|m| m.width.max(m.height) > max_side)
    {
        if mips.len() > 1 {
            mips.remove(0);
        } else {
            let small = mips::downsample(&mips[0], color_space);
            mips[0] = small;
        }
    }
}

/// Converts a DDS file to one blob per requested format.
pub fn convert_dds(
    bytes: &[u8],
    color_space: ColorSpace,
    formats: &[TextureFormat],
) -> Result<Vec<TextureBlob>, TextureError> {
    convert_dds_with(
        bytes,
        ConvertOptions {
            color_space,
            generate_mips: true,
            max_side: None,
        },
        formats,
    )
}

pub fn convert_dds_with(
    bytes: &[u8],
    options: ConvertOptions,
    formats: &[TextureFormat],
) -> Result<Vec<TextureBlob>, TextureError> {
    let mut mips = decode_dds(bytes)?.mips;
    if let Some(max_side) = options.max_side {
        cap_mips(&mut mips, options.color_space, max_side);
    }
    if options.generate_mips {
        extend_mips(&mut mips, options.color_space);
    }
    formats
        .iter()
        .map(|&format| encode_chain(&mips, options.color_space, format))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_space_rule() {
        for (name, want) in [
            ("warrior_diff.dds", ColorSpace::Srgb),
            ("Warrior_DIFF.dds", ColorSpace::Srgb),
            ("euro_grassland_d.dds", ColorSpace::Srgb),
            ("warrior_sref.dds", ColorSpace::Linear),
            ("pacal_clothing_norm.dds", ColorSpace::Linear),
            ("generic_tundra_h.dds", ColorSpace::Linear),
            ("plains_hill_2_1_h_blend.dds", ColorSpace::Linear),
            ("fonticons.dds", ColorSpace::Srgb),
            ("white.dds", ColorSpace::Srgb),
        ] {
            assert_eq!(color_space_for(name), want, "{name}");
        }
    }
}
