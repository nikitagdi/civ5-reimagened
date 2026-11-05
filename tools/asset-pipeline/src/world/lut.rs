//! Colour-key lookup tables: `Assets/colorkeys/*.tga` to `D3` 32x32x32 `Rgba8` volumes
//!.
//!
//! The files are unrolled volumes: a TGA of 1600x21, 1280x26, 1920x18 or 3840x9 pixels whose first
//! 32768 pixels in top-down raster order are the volume with raster index `r + 32*g + 1024*b`;
//! the pixels after that are padding of the last row. The TGA stores its rows bottom-up, so the
//! raster starts at the LAST stored row. // measured: 2026-10-21, the eleven `Terrain_*`,
//! `Buildings_*` and `TerrainColorKey` tables are identity grids (`8*i + 3` per channel) in this order and in no other;
//! in the graded tables the neighbour difference along all three axes is equal only in this order.

use asset_format::{TexDim, TextureBlob, TextureFormat};

use crate::textures::{TextureError, read_tga};

/// Side of the volume. // measured: 2026-10-21, `R` steps by 8 from 3 along the raster.
pub const LUT_SIDE: u32 = 32;
/// Pixels of the volume.
pub const LUT_TEXELS: usize = (LUT_SIDE * LUT_SIDE * LUT_SIDE) as usize;

/// The colour-key files of the install, by file name (lower case). Systems `Terrain`, `Buildings`,
/// `TerrainShadow` and `TerrainFOW` come in five styles; the other two are single files.
pub const LUT_STYLES: [&str; 5] = ["africa", "america", "asia", "europe", "generic"];

/// Every colour-key file the b2-light set converts, in a fixed order.
pub fn lut_files() -> Vec<String> {
    let mut v = Vec::new();
    for system in ["terrain", "buildings", "terrainshadow", "terrainfow"] {
        for style in LUT_STYLES {
            v.push(format!("{system}_{style}_colorkey.tga"));
        }
    }
    v.push("rivers_generic_colorkey.tga".to_owned());
    v.push("terraincolorkey.tga".to_owned());
    v
}

/// Reads a colour-key TGA into a `D3` volume with one level. The first [`LUT_TEXELS`] pixels of
/// the top-down raster are kept; a file with fewer is an error.
pub fn lut_from_tga(bytes: &[u8]) -> Result<TextureBlob, TextureError> {
    let tga = read_tga(bytes)?;
    let pixels = &tga.pixels;
    let need = LUT_TEXELS * 4;
    if pixels.len() < need {
        return Err(TextureError::Unsupported(format!(
            "colour key {}x{} has {} pixels, a 32^3 volume needs {LUT_TEXELS}",
            tga.width,
            tga.height,
            pixels.len() / 4
        )));
    }
    Ok(TextureBlob {
        format: TextureFormat::Rgba8,
        dim: TexDim::D3,
        srgb: false,
        width: LUT_SIDE,
        height: LUT_SIDE,
        depth: LUT_SIDE,
        mips: vec![pixels[..need].to_vec()],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 32 bit TGA stored bottom-up (descriptor bit 5 clear) whose pixel `i` in top-down raster
    /// order is `f(i)`.
    fn tga(width: u16, height: u16, f: impl Fn(usize) -> [u8; 4]) -> Vec<u8> {
        let mut v = vec![0u8; 18];
        v[2] = 2;
        v[12..14].copy_from_slice(&width.to_le_bytes());
        v[14..16].copy_from_slice(&height.to_le_bytes());
        v[16] = 32;
        let (w, h) = (usize::from(width), usize::from(height));
        for stored_row in 0..h {
            let top_down_row = h - 1 - stored_row;
            for x in 0..w {
                let [r, g, b, a] = f(top_down_row * w + x);
                v.extend_from_slice(&[b, g, r, a]);
            }
        }
        v
    }

    #[test]
    fn volume_follows_the_raster_index() {
        // Pixel (r, g, b) of the volume is raster index r + 32 g + 1024 b.
        let f = |i: usize| {
            let (r, g, b) = (i % 32, i / 32 % 32, i / 1024);
            [r as u8 * 8 + 3, g as u8 * 8 + 1, b as u8 * 8 + 2, 255]
        };
        // 1600 x 21 = 33600 pixels: 832 of padding after the volume.
        let bytes = tga(
            1600,
            21,
            |i| if i < LUT_TEXELS { f(i) } else { [9, 9, 9, 9] },
        );
        let t = lut_from_tga(&bytes).unwrap();
        assert_eq!(
            (t.dim, t.width, t.height, t.depth),
            (TexDim::D3, 32, 32, 32)
        );
        assert_eq!(t.mips.len(), 1);
        assert_eq!(t.mips[0].len(), LUT_TEXELS * 4);
        let texel = |r: usize, g: usize, b: usize| {
            let at = (r + 32 * g + 1024 * b) * 4;
            &t.mips[0][at..at + 4]
        };
        assert_eq!(texel(0, 0, 0), [3, 1, 2, 255]);
        assert_eq!(texel(31, 0, 0), [31 * 8 + 3, 1, 2, 255]);
        assert_eq!(texel(5, 7, 31), [5 * 8 + 3, 7 * 8 + 1, 31 * 8 + 2, 255]);
        // The padding is gone; the blob survives its own validation.
        assert_eq!(TextureBlob::decode(&t.encode()).unwrap(), t);
    }

    #[test]
    fn the_raster_starts_at_the_last_stored_row() {
        // The file stores the bottom row first, the volume starts at the top row of the image:
        // the first texel is the first pixel of the last stored row.
        let bytes = tga(1600, 21, |i| {
            [(i % 251) as u8, (i / 251 % 251) as u8, 0, 255]
        });
        let t = lut_from_tga(&bytes).unwrap();
        assert_eq!(&t.mips[0][..4], [0, 0, 0, 255]);
        assert_eq!(&t.mips[0][4..8], [1, 0, 0, 255]);
        // Texel 1600 is the first pixel of the second image row.
        assert_eq!(
            &t.mips[0][1600 * 4..1600 * 4 + 2],
            [(1600 % 251) as u8, (1600 / 251) as u8]
        );
    }

    #[test]
    fn too_small_is_an_error() {
        let bytes = tga(100, 100, |_| [0, 0, 0, 0]);
        assert!(matches!(
            lut_from_tga(&bytes),
            Err(TextureError::Unsupported(_))
        ));
    }

    #[test]
    fn file_list_has_twenty_two_distinct_names() {
        let v = lut_files();
        assert_eq!(v.len(), 22);
        let set: std::collections::BTreeSet<_> = v.iter().collect();
        assert_eq!(set.len(), 22);
    }
}
