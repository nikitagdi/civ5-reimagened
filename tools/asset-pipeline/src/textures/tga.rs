//! TGA reader for the game's `Assets/colorkeys/*.tga` lookup tables.
//!
//! Handles what those files and ordinary exports use: true-color images, 24 or 32 bit,
//! uncompressed (type 2) or run-length encoded (type 10), either row order. No new dependency.

use super::{MAX_SIDE, TextureError};

const HEADER_LEN: usize = 18;
const TYPE_TRUECOLOR: u8 = 2;
const TYPE_TRUECOLOR_RLE: u8 = 10;
/// Image descriptor bit 5: first row in the file is the top row.
const DESC_TOP_FIRST: u8 = 0x20;
/// Image descriptor bit 4: first column in the file is the right column.
const DESC_RIGHT_FIRST: u8 = 0x10;

/// A decoded TGA: RGBA8, rows tightly packed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TgaImage {
    pub width: u32,
    pub height: u32,
    /// Rows from the top of the image down, left to right, whatever the file's row order was.
    pub pixels: Vec<u8>,
    /// The file stored the bottom row first (the usual TGA layout; the colorkey files do).
    /// [`Self::pixels_in_file_order`] gives the rows the way they were stored.
    pub stored_bottom_up: bool,
}

impl TgaImage {
    /// The pixels with rows in the order of the file: bottom row first when `stored_bottom_up`.
    pub fn pixels_in_file_order(&self) -> Vec<u8> {
        if !self.stored_bottom_up {
            return self.pixels.clone();
        }
        let row = self.width as usize * 4;
        self.pixels
            .chunks_exact(row)
            .rev()
            .flatten()
            .copied()
            .collect()
    }
}

fn bad(msg: &str) -> TextureError {
    TextureError::Tga(msg.into())
}

/// Reads a true-color TGA. Palettes, gray-scale and right-to-left images are `Unsupported`;
/// short data is `Truncated`; sizes are checked before anything is allocated.
pub fn read_tga(bytes: &[u8]) -> Result<TgaImage, TextureError> {
    if bytes.len() < HEADER_LEN {
        return Err(bad("shorter than the 18-byte header"));
    }
    let id_len = usize::from(bytes[0]);
    let color_map_type = bytes[1];
    let image_type = bytes[2];
    let map_len = usize::from(u16::from_le_bytes([bytes[5], bytes[6]]));
    let map_entry_bits = usize::from(bytes[7]);
    let width = u32::from(u16::from_le_bytes([bytes[12], bytes[13]]));
    let height = u32::from(u16::from_le_bytes([bytes[14], bytes[15]]));
    let bits = bytes[16];
    let descriptor = bytes[17];

    if !matches!(image_type, TYPE_TRUECOLOR | TYPE_TRUECOLOR_RLE) {
        return Err(TextureError::Unsupported(format!(
            "TGA image type {image_type}"
        )));
    }
    if !matches!(bits, 24 | 32) {
        return Err(TextureError::Unsupported(format!("TGA {bits} bits")));
    }
    if descriptor & DESC_RIGHT_FIRST != 0 {
        return Err(TextureError::Unsupported("TGA right-to-left rows".into()));
    }
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(TextureError::BadSize { width, height });
    }

    // Skip the id field and a color map the image does not use.
    let map_bytes = if color_map_type == 1 {
        map_len * map_entry_bits.div_ceil(8)
    } else {
        0
    };
    let data_start = HEADER_LEN + id_len + map_bytes;
    let data = bytes.get(data_start..).unwrap_or(&[]);

    let bpp = usize::from(bits / 8);
    let count = width as usize * height as usize;
    // Never reserve more than the data present can fill (a run packet makes at most 128
    // pixels from `1 + bpp` bytes).
    let most = if image_type == TYPE_TRUECOLOR {
        data.len() / bpp
    } else {
        data.len() / (1 + bpp) * 128 + 128
    };
    if count > most {
        return Err(TextureError::Truncated {
            need: (count * bpp) as u64,
            have: data.len() as u64,
        });
    }
    let mut pixels = Vec::with_capacity(count * 4);

    let mut push = |px: &[u8]| {
        // File order is B, G, R[, A].
        let a = if bpp == 4 { px[3] } else { 255 };
        pixels.extend_from_slice(&[px[2], px[1], px[0], a]);
    };
    if image_type == TYPE_TRUECOLOR {
        let need = (count * bpp) as u64;
        if (data.len() as u64) < need {
            return Err(TextureError::Truncated {
                need,
                have: data.len() as u64,
            });
        }
        data.chunks_exact(bpp).take(count).for_each(&mut push);
    } else {
        let mut at = 0;
        let mut done = 0;
        while done < count {
            let head = *data.get(at).ok_or(TextureError::Truncated {
                need: (at + 1) as u64,
                have: data.len() as u64,
            })?;
            at += 1;
            let run = usize::from(head & 0x7f) + 1;
            if done + run > count {
                return Err(bad("run goes past the end of the image"));
            }
            let repeated = head & 0x80 != 0;
            let take = if repeated { bpp } else { run * bpp };
            let chunk = data.get(at..at + take).ok_or(TextureError::Truncated {
                need: (at + take) as u64,
                have: data.len() as u64,
            })?;
            at += take;
            if repeated {
                (0..run).for_each(|_| push(chunk));
            } else {
                chunk.chunks_exact(bpp).for_each(&mut push);
            }
            done += run;
        }
    }

    let stored_bottom_up = descriptor & DESC_TOP_FIRST == 0;
    if stored_bottom_up {
        let row = width as usize * 4;
        pixels = pixels.chunks_exact(row).rev().flatten().copied().collect();
    }
    Ok(TgaImage {
        width,
        height,
        pixels,
        stored_bottom_up,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a TGA file; `rows` are in file order, pixels as (b, g, r, a).
    fn tga(image_type: u8, bits: u8, descriptor: u8, w: u16, h: u16, body: &[u8]) -> Vec<u8> {
        let mut v = vec![0u8; HEADER_LEN];
        v[2] = image_type;
        v[12..14].copy_from_slice(&w.to_le_bytes());
        v[14..16].copy_from_slice(&h.to_le_bytes());
        v[16] = bits;
        v[17] = descriptor;
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn bottom_up_32_bit() {
        // 2x2, descriptor 8 (8 alpha bits, origin bottom-left): the first row in the file is
        // the bottom row of the image.
        let body = [
            1, 2, 3, 4, 5, 6, 7, 8, // bottom row: (r,g,b,a) = (3,2,1,4) (7,6,5,8)
            11, 12, 13, 14, 15, 16, 17, 18, // top row
        ];
        let img = read_tga(&tga(2, 32, 8, 2, 2, &body)).unwrap();
        assert!(img.stored_bottom_up);
        assert_eq!(
            img.pixels,
            [13, 12, 11, 14, 17, 16, 15, 18, 3, 2, 1, 4, 7, 6, 5, 8]
        );
        assert_eq!(
            img.pixels_in_file_order(),
            [3, 2, 1, 4, 7, 6, 5, 8, 13, 12, 11, 14, 17, 16, 15, 18]
        );
    }

    #[test]
    fn top_down_24_bit() {
        let body = [1, 2, 3, 4, 5, 6];
        let img = read_tga(&tga(2, 24, 0x20, 2, 1, &body)).unwrap();
        assert!(!img.stored_bottom_up);
        assert_eq!(img.pixels, [3, 2, 1, 255, 6, 5, 4, 255]);
        assert_eq!(img.pixels_in_file_order(), img.pixels);
    }

    #[test]
    fn rle_runs() {
        // 4x1: a repeat packet of 3 then a raw packet of 1.
        let body = [0x82, 1, 2, 3, 4, 0x00, 9, 8, 7, 6];
        let img = read_tga(&tga(10, 32, 0x28, 4, 1, &body)).unwrap();
        assert_eq!(img.pixels, [3, 2, 1, 4, 3, 2, 1, 4, 3, 2, 1, 4, 7, 8, 9, 6]);
        // A run past the end of the image is an error.
        let body = [0x83, 1, 2, 3, 4];
        assert!(read_tga(&tga(10, 32, 0x28, 3, 1, &body)).is_err());
    }

    #[test]
    fn id_field_is_skipped() {
        let mut f = tga(2, 24, 0x20, 1, 1, &[0, 0, 0, 0, 1, 2, 3]);
        f[0] = 4;
        let img = read_tga(&f).unwrap();
        assert_eq!(img.pixels, [3, 2, 1, 255]);
    }

    #[test]
    fn bad_files_are_errors() {
        assert!(read_tga(&[]).is_err());
        assert!(read_tga(&[0; 17]).is_err());
        // Truncated pixel data.
        assert!(matches!(
            read_tga(&tga(2, 32, 8, 2, 2, &[0; 15])),
            Err(TextureError::Truncated { need: 16, have: 15 })
        ));
        // Palette / gray-scale / 16 bit / right-to-left.
        assert!(matches!(
            read_tga(&tga(1, 8, 0, 1, 1, &[0])),
            Err(TextureError::Unsupported(_))
        ));
        assert!(matches!(
            read_tga(&tga(2, 16, 0, 1, 1, &[0, 0])),
            Err(TextureError::Unsupported(_))
        ));
        assert!(matches!(
            read_tga(&tga(2, 24, 0x30, 1, 1, &[0; 3])),
            Err(TextureError::Unsupported(_))
        ));
        // Zero size, and sizes over the cap fail before allocating.
        assert!(matches!(
            read_tga(&tga(2, 32, 8, 0, 4, &[])),
            Err(TextureError::BadSize { .. })
        ));
        assert!(matches!(
            read_tga(&tga(2, 32, 8, u16::MAX, u16::MAX, &[])),
            Err(TextureError::BadSize { .. })
        ));
        // Large but legal size with no data: Truncated, not an allocation of the full image.
        assert!(matches!(
            read_tga(&tga(2, 32, 8, 16384, 16384, &[])),
            Err(TextureError::Truncated { .. })
        ));
    }
}
