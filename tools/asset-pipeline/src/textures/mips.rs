//! Mip chain generation: 2x2 box filter, in linear light for sRGB color.

use std::sync::OnceLock;

use super::{ColorSpace, Rgba8Image};

/// Levels in a full chain down to 1x1.
pub fn full_chain_len(width: u32, height: u32) -> usize {
    width.max(height).max(1).ilog2() as usize + 1
}

/// Appends levels after the last existing one until the chain reaches 1x1.
///
/// Existing levels are kept as authored. Odd sizes round down like D3D/GL; the last
/// row or column of an odd level is dropped from the average. Alpha is averaged linearly
/// and not premultiplied, so colors under transparent pixels can bleed slightly.
pub fn extend_mips(mips: &mut Vec<Rgba8Image>, color_space: ColorSpace) {
    let Some(base) = mips.first() else { return };
    let total = full_chain_len(base.width, base.height);
    while mips.len() < total {
        let next = downsample(&mips[mips.len() - 1], color_space);
        mips.push(next);
    }
}

fn srgb_to_linear_lut() -> &'static [f32; 256] {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| {
        std::array::from_fn(|i| {
            let c = i as f32 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        })
    })
}

fn linear_to_srgb8(l: f32) -> u8 {
    let l = l.clamp(0.0, 1.0);
    let c = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (c * 255.0 + 0.5) as u8
}

pub(super) fn downsample(src: &Rgba8Image, color_space: ColorSpace) -> Rgba8Image {
    let (sw, sh) = (src.width as usize, src.height as usize);
    let (dw, dh) = ((sw / 2).max(1), (sh / 2).max(1));
    let lut = srgb_to_linear_lut();
    let mut out = vec![0u8; dw * dh * 4];
    for y in 0..dh {
        let (y0, y1) = ((2 * y).min(sh - 1), (2 * y + 1).min(sh - 1));
        for x in 0..dw {
            let (x0, x1) = ((2 * x).min(sw - 1), (2 * x + 1).min(sw - 1));
            let taps = [(x0, y0), (x1, y0), (x0, y1), (x1, y1)];
            let dst = &mut out[(y * dw + x) * 4..][..4];
            for (c, out_c) in dst.iter_mut().enumerate() {
                let sample = |&(sx, sy): &(usize, usize)| src.pixels[(sy * sw + sx) * 4 + c];
                *out_c = if c == 3 || color_space == ColorSpace::Linear {
                    let sum: u32 = taps.iter().map(|t| u32::from(sample(t))).sum();
                    ((sum + 2) / 4) as u8
                } else {
                    let sum: f32 = taps.iter().map(|t| lut[sample(t) as usize]).sum();
                    linear_to_srgb8(sum / 4.0)
                };
            }
        }
    }
    Rgba8Image::new(dw as u32, dh as u32, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, px: [u8; 4]) -> Rgba8Image {
        Rgba8Image::new(w, h, px.repeat((w * h) as usize))
    }

    #[test]
    fn chain_lengths() {
        assert_eq!(full_chain_len(1, 1), 1);
        assert_eq!(full_chain_len(128, 128), 8);
        assert_eq!(full_chain_len(512, 256), 10);
        assert_eq!(full_chain_len(142, 77), 8);
        assert_eq!(full_chain_len(5, 1), 3);
    }

    #[test]
    fn generated_sizes_follow_floor_halving() {
        let mut mips = vec![solid(13, 6, [10, 20, 30, 255])];
        extend_mips(&mut mips, ColorSpace::Srgb);
        let dims: Vec<_> = mips.iter().map(|m| (m.width, m.height)).collect();
        assert_eq!(dims, [(13, 6), (6, 3), (3, 1), (1, 1)]);
    }

    #[test]
    fn existing_levels_are_kept_and_tail_is_generated() {
        let mut mips = vec![solid(4, 4, [200; 4]), solid(2, 2, [7, 7, 7, 7])];
        extend_mips(&mut mips, ColorSpace::Linear);
        assert_eq!(mips.len(), 3);
        assert_eq!(mips[1].pixels[0], 7);
        assert_eq!(mips[2].pixels, [7, 7, 7, 7]);
    }

    #[test]
    fn complete_chain_is_untouched() {
        let mut mips = vec![solid(2, 2, [1; 4]), solid(1, 1, [9; 4])];
        extend_mips(&mut mips, ColorSpace::Srgb);
        assert_eq!(mips.len(), 2);
        assert_eq!(mips[1].pixels, [9; 4]);
    }

    #[test]
    fn linear_average_in_linear_space() {
        // Black and white pixels: raw average is 128 (linear data) but 188 in sRGB
        // (50% linear light).
        let mut px = Vec::new();
        for v in [0u8, 255, 255, 0] {
            px.extend_from_slice(&[v, v, v, 255]);
        }
        let img = Rgba8Image::new(2, 2, px);
        assert_eq!(
            downsample(&img, ColorSpace::Linear).pixels,
            [128, 128, 128, 255]
        );
        let s = downsample(&img, ColorSpace::Srgb).pixels;
        assert!((187..=189).contains(&s[0]), "{}", s[0]);
        assert_eq!(s[3], 255, "alpha is always linear");
    }

    #[test]
    fn solid_color_is_stable_through_srgb_chain() {
        let mut mips = vec![solid(16, 16, [37, 120, 201, 255])];
        extend_mips(&mut mips, ColorSpace::Srgb);
        for m in &mips {
            assert_eq!(m.pixels[..4], [37, 120, 201, 255]);
        }
    }
}
