//! CPU decoder for BC1-BC5 (DXT1/3/5, RGTC1/2): the fallback for devices without
//! `TEXTURE_COMPRESSION_BC`. No dependencies.
//!
//! Output matches what a GPU returns for the `*Unorm` formats, within 1/255 (hardware rounds the
//! colour interpolation its own way): BC1/BC2/BC3 -> RGBA8, BC4 -> R8, BC5 -> RG8. Levels whose
//! sides are not multiples of 4 are decoded from whole blocks and cropped to `width` x `height`.

use asset_format::TextureFormat;

/// Format the decoded texels are stored in, `None` for formats this module does not decode.
pub fn decoded_format(format: TextureFormat) -> Option<TextureFormat> {
    match format {
        TextureFormat::Bc1 | TextureFormat::Bc2 | TextureFormat::Bc3 => Some(TextureFormat::Rgba8),
        TextureFormat::Bc4 => Some(TextureFormat::R8),
        TextureFormat::Bc5 => Some(TextureFormat::Rg8),
        _ => None,
    }
}

/// Decodes one 2D slice (`width` x `height` texels, tightly packed blocks) of `format`.
/// `None` when the format is not BC1-BC5 or `blocks` is shorter than the slice needs.
pub fn decode_slice(
    format: TextureFormat,
    blocks: &[u8],
    width: u32,
    height: u32,
) -> Option<Vec<u8>> {
    let out_format = decoded_format(format)?;
    if blocks.len() < format.mip_size(width, height) {
        return None;
    }
    let (w, h) = (width as usize, height as usize);
    let bpp = out_format.block_bytes() as usize;
    let bx = w.div_ceil(4);
    let by = h.div_ceil(4);
    let block_bytes = format.block_bytes() as usize;
    let mut out = vec![0u8; w * h * bpp];
    for y in 0..by {
        for x in 0..bx {
            let at = (y * bx + x) * block_bytes;
            let block = &blocks[at..at + block_bytes];
            // A block decodes to 16 texels of `bpp` bytes.
            let mut texels = [0u8; 16 * 4];
            match format {
                TextureFormat::Bc1 => bc1_block(block, &mut texels, true),
                TextureFormat::Bc2 => {
                    bc1_block(&block[8..], &mut texels, false);
                    for i in 0..16 {
                        let nibble = (block[i / 2] >> (4 * (i % 2))) & 0xf;
                        texels[i * 4 + 3] = nibble * 17;
                    }
                }
                TextureFormat::Bc3 => {
                    bc1_block(&block[8..], &mut texels, false);
                    let alpha = bc4_block(&block[..8]);
                    for i in 0..16 {
                        texels[i * 4 + 3] = alpha[i];
                    }
                }
                TextureFormat::Bc4 => {
                    let r = bc4_block(block);
                    texels[..16].copy_from_slice(&r);
                }
                TextureFormat::Bc5 => {
                    let r = bc4_block(&block[..8]);
                    let g = bc4_block(&block[8..]);
                    for i in 0..16 {
                        texels[i * 2] = r[i];
                        texels[i * 2 + 1] = g[i];
                    }
                }
                _ => unreachable!("checked by decoded_format"),
            }
            for ty in 0..4 {
                let py = y * 4 + ty;
                if py >= h {
                    break;
                }
                for tx in 0..4 {
                    let px = x * 4 + tx;
                    if px >= w {
                        break;
                    }
                    let src = (ty * 4 + tx) * bpp;
                    let dst = (py * w + px) * bpp;
                    out[dst..dst + bpp].copy_from_slice(&texels[src..src + bpp]);
                }
            }
        }
    }
    Some(out)
}

/// 5:6:5 to 8 bits per channel, replicating the high bits into the low ones.
fn expand565(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 31) as u8;
    let g = ((c >> 5) & 63) as u8;
    let b = (c & 31) as u8;
    [
        (r << 3) | (r >> 2),
        (g << 2) | (g >> 4),
        (b << 3) | (b >> 2),
    ]
}

/// The colour half of BC1/BC2/BC3 into RGBA texels (`out` is 16 x 4 bytes, alpha written as 255 or,
/// for BC1 punch-through, 0). `allow_punch` is BC1 only: BC2/BC3 colour blocks are always 4-colour.
fn bc1_block(block: &[u8], out: &mut [u8; 64], allow_punch: bool) {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let (e0, e1) = (expand565(c0), expand565(c1));
    let mut palette = [[0u8; 4]; 4];
    palette[0] = [e0[0], e0[1], e0[2], 255];
    palette[1] = [e1[0], e1[1], e1[2], 255];
    if c0 > c1 || !allow_punch {
        for k in 0..3 {
            palette[2][k] = ((2 * u16::from(e0[k]) + u16::from(e1[k])) / 3) as u8;
            palette[3][k] = ((u16::from(e0[k]) + 2 * u16::from(e1[k])) / 3) as u8;
        }
        palette[2][3] = 255;
        palette[3][3] = 255;
    } else {
        for k in 0..3 {
            palette[2][k] = ((u16::from(e0[k]) + u16::from(e1[k])) / 2) as u8;
        }
        palette[2][3] = 255;
        // Index 3 is transparent black.
        palette[3] = [0, 0, 0, 0];
    }
    let bits = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    for i in 0..16 {
        let idx = ((bits >> (2 * i)) & 3) as usize;
        out[i * 4..i * 4 + 4].copy_from_slice(&palette[idx]);
    }
}

/// One BC4 block (also the BC3 alpha block and each half of BC5) as 16 unorm bytes.
fn bc4_block(block: &[u8]) -> [u8; 16] {
    let a0 = u32::from(block[0]);
    let a1 = u32::from(block[1]);
    let mut palette = [0u32; 8];
    palette[0] = a0;
    palette[1] = a1;
    if a0 > a1 {
        for i in 1..7u32 {
            palette[i as usize + 1] = ((7 - i) * a0 + i * a1 + 3) / 7;
        }
    } else {
        for i in 1..5u32 {
            palette[i as usize + 1] = ((5 - i) * a0 + i * a1 + 2) / 5;
        }
        palette[6] = 0;
        palette[7] = 255;
    }
    // 16 three-bit indices in the 48 bits after the endpoints.
    let mut bits = 0u64;
    for (i, b) in block[2..8].iter().enumerate() {
        bits |= u64::from(*b) << (8 * i);
    }
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = palette[((bits >> (3 * i)) & 7) as usize] as u8;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb565(r: u8, g: u8, b: u8) -> u16 {
        (u16::from(r) << 11) | (u16::from(g) << 5) | u16::from(b)
    }

    fn texel(out: &[u8], w: usize, bpp: usize, x: usize, y: usize) -> &[u8] {
        &out[(y * w + x) * bpp..][..bpp]
    }

    #[test]
    fn bc1_four_colour_formulas() {
        // c0 = pure red 5:6:5 (31,0,0), c1 = pure blue (0,0,31); c0 > c1 -> 4-colour mode.
        let c0 = rgb565(31, 0, 0);
        let c1 = rgb565(0, 0, 31);
        assert!(c0 > c1);
        let mut block = Vec::new();
        block.extend(c0.to_le_bytes());
        block.extend(c1.to_le_bytes());
        // Row 0: indices 0,1,2,3; other rows all 0.
        block.extend([0b11_10_01_00, 0, 0, 0]);
        let out = decode_slice(TextureFormat::Bc1, &block, 4, 4).unwrap();
        assert_eq!(texel(&out, 4, 4, 0, 0), [255, 0, 0, 255]);
        assert_eq!(texel(&out, 4, 4, 1, 0), [0, 0, 255, 255]);
        // (2*255 + 0) / 3 = 170 and (255 + 0) / 3 = 85.
        assert_eq!(texel(&out, 4, 4, 2, 0), [170, 0, 85, 255]);
        assert_eq!(texel(&out, 4, 4, 3, 0), [85, 0, 170, 255]);
        assert_eq!(texel(&out, 4, 4, 0, 3), [255, 0, 0, 255]);
    }

    #[test]
    fn bc1_three_colour_mode_has_transparent_black() {
        let c0 = rgb565(0, 0, 31);
        let c1 = rgb565(31, 0, 0);
        assert!(c0 < c1);
        let mut block = Vec::new();
        block.extend(c0.to_le_bytes());
        block.extend(c1.to_le_bytes());
        block.extend([0b11_10_01_00, 0, 0, 0]);
        let out = decode_slice(TextureFormat::Bc1, &block, 4, 4).unwrap();
        // Index 2 is the midpoint (255 + 0) / 2 = 127, index 3 is transparent black.
        assert_eq!(texel(&out, 4, 4, 2, 0), [127, 0, 127, 255]);
        assert_eq!(texel(&out, 4, 4, 3, 0), [0, 0, 0, 0]);
    }

    #[test]
    fn bc2_explicit_alpha_and_always_four_colours() {
        let mut block = Vec::new();
        // Alpha nibbles for texels 0..16: 0,1,2,...,15 -> 0,17,...,255.
        for i in 0..8u8 {
            block.push((2 * i) | ((2 * i + 1) << 4));
        }
        // Colour block with c0 < c1 would be 3-colour in BC1; BC2 still uses 4 colours.
        block.extend(rgb565(0, 0, 31).to_le_bytes());
        block.extend(rgb565(31, 0, 0).to_le_bytes());
        block.extend([0b11_10_01_00, 0, 0, 0]);
        let out = decode_slice(TextureFormat::Bc2, &block, 4, 4).unwrap();
        for i in 0..4usize {
            assert_eq!(texel(&out, 4, 4, i, 0)[3], (i * 17) as u8);
        }
        assert_eq!(texel(&out, 4, 4, 3, 3)[3], 255);
        // Index 2 = (2*blue + red)/3.
        assert_eq!(texel(&out, 4, 4, 2, 0)[..3], [85, 0, 170]);
        assert_eq!(texel(&out, 4, 4, 3, 0)[..3], [170, 0, 85]);
    }

    /// `n` three-bit indices packed from bit 0.
    fn pack3(indices: &[u8; 16]) -> [u8; 6] {
        let mut bits = 0u64;
        for (i, v) in indices.iter().enumerate() {
            bits |= u64::from(*v) << (3 * i);
        }
        let b = bits.to_le_bytes();
        [b[0], b[1], b[2], b[3], b[4], b[5]]
    }

    #[test]
    fn bc4_eight_and_six_value_modes() {
        let idx: [u8; 16] = std::array::from_fn(|i| (i % 8) as u8);
        // a0 > a1: 8 values, interpolated in sevenths.
        let mut block = vec![255, 0];
        block.extend(pack3(&idx));
        let out = decode_slice(TextureFormat::Bc4, &block, 4, 4).unwrap();
        let expect = [255u8, 0, 219, 182, 146, 109, 73, 36];
        for (i, e) in expect.iter().enumerate() {
            assert_eq!(out[i], *e, "index {i}");
        }
        // a0 <= a1: 6 values in fifths, then 0 and 255.
        let mut block = vec![0, 255];
        block.extend(pack3(&idx));
        let out = decode_slice(TextureFormat::Bc4, &block, 4, 4).unwrap();
        assert_eq!(out[..8], [0, 255, 51, 102, 153, 204, 0, 255]);
    }

    #[test]
    fn bc3_alpha_and_bc5_two_channels() {
        let idx: [u8; 16] = std::array::from_fn(|i| (i % 8) as u8);
        let mut alpha = vec![255, 0];
        alpha.extend(pack3(&idx));
        // BC3 = BC4 alpha block + BC1-style colour block (solid white here).
        let mut bc3 = alpha.clone();
        bc3.extend(rgb565(31, 63, 31).to_le_bytes());
        bc3.extend(rgb565(0, 0, 0).to_le_bytes());
        bc3.extend([0, 0, 0, 0]);
        let out = decode_slice(TextureFormat::Bc3, &bc3, 4, 4).unwrap();
        assert_eq!(texel(&out, 4, 4, 0, 0), [255, 255, 255, 255]);
        assert_eq!(texel(&out, 4, 4, 2, 0)[3], 219);
        // BC5: R from the first block, G from the second (indices reversed).
        let rev: [u8; 16] = std::array::from_fn(|i| 7 - (i % 8) as u8);
        let mut green = vec![255, 0];
        green.extend(pack3(&rev));
        let mut bc5 = alpha;
        bc5.extend(green);
        let out = decode_slice(TextureFormat::Bc5, &bc5, 4, 4).unwrap();
        assert_eq!(texel(&out, 4, 2, 0, 0), [255, 36]);
        assert_eq!(texel(&out, 4, 2, 1, 0), [0, 73]);
    }

    #[test]
    fn partial_blocks_are_cropped() {
        // 2x1 level of a BC1 texture: one block, only the first two texels are kept.
        let mut block = Vec::new();
        block.extend(rgb565(31, 0, 0).to_le_bytes());
        block.extend(rgb565(0, 0, 31).to_le_bytes());
        block.extend([0b0100, 0, 0, 0]);
        let out = decode_slice(TextureFormat::Bc1, &block, 2, 1).unwrap();
        assert_eq!(out.len(), 2 * 4);
        assert_eq!(out[..4], [255, 0, 0, 255]);
        assert_eq!(out[4..], [0, 0, 255, 255]);
    }

    #[test]
    fn rejects_other_formats_and_short_input() {
        assert!(decode_slice(TextureFormat::Bc7, &[0; 16], 4, 4).is_none());
        assert!(decode_slice(TextureFormat::Rgba8, &[0; 64], 4, 4).is_none());
        assert!(decode_slice(TextureFormat::Bc1, &[0; 7], 4, 4).is_none());
        assert_eq!(decoded_format(TextureFormat::Bc4), Some(TextureFormat::R8));
        assert_eq!(decoded_format(TextureFormat::Bc5), Some(TextureFormat::Rg8));
    }
}
