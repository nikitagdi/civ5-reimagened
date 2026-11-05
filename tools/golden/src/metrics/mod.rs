//! Image metrics: ꟻLIP LDR, SSIM, ΔE2000, plus images and masks.
//! Images are 8-bit sRGB, as on screen.

pub mod color;
pub mod de2000;
pub mod flip;
pub mod ssim;

/// 8-bit sRGB image, 3 bytes per pixel, row major.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
}

impl Image {
    pub fn new(width: usize, height: usize, rgb: Vec<u8>) -> Self {
        assert_eq!(rgb.len(), width * height * 3);
        Self { width, height, rgb }
    }

    /// From `RGBA8` (alpha dropped).
    pub fn from_rgba(width: usize, height: usize, rgba: &[u8]) -> Self {
        assert_eq!(rgba.len(), width * height * 4);
        let rgb = rgba
            .chunks_exact(4)
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        Self { width, height, rgb }
    }

    pub fn pixel(&self, i: usize) -> [u8; 3] {
        [self.rgb[i * 3], self.rgb[i * 3 + 1], self.rgb[i * 3 + 2]]
    }

    /// Luminance 0..255 (ITU-R BT.601: 0.299 R + 0.587 G + 0.114 B).
    pub fn luma601(&self) -> Vec<f64> {
        (0..self.width * self.height)
            .map(|i| {
                let [r, g, b] = self.pixel(i);
                0.299 * f64::from(r) + 0.587 * f64::from(g) + 0.114 * f64::from(b)
            })
            .collect()
    }
}

/// Mask of the pixels a metric looks at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mask {
    pub width: usize,
    pub height: usize,
    pub bits: Vec<bool>,
}

/// Mask erosion by two pixels before metrics (MSAA edges, sub-pixel shifts).
pub const MASK_ERODE_PX: usize = 2;

/// Bit 0x40 of a class-mask pixel: fog clouds. Bit 0x80: overlay.
pub const CLASS_BIT_FOG: u8 = 0x40;
pub const CLASS_BIT_OVERLAY: u8 = 0x80;
const CLASS_NUMBER_BITS: u8 = 0x3F;

impl Mask {
    /// All pixels.
    pub fn full(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            bits: vec![true; width * height],
        }
    }

    /// Pixels whose class number (low 6 bits of the class-mask byte) is `class`.
    pub fn from_class(width: usize, height: usize, classes: &[u8], class: u8) -> Self {
        assert_eq!(classes.len(), width * height);
        let bits = classes
            .iter()
            .map(|&c| c & CLASS_NUMBER_BITS == class)
            .collect();
        Self {
            width,
            height,
            bits,
        }
    }

    /// Pixels that have all the bits of `flag` (fog `0x40`, overlay `0x80`).
    pub fn from_flag(width: usize, height: usize, classes: &[u8], flag: u8) -> Self {
        assert_eq!(classes.len(), width * height);
        let bits = classes.iter().map(|&c| c & flag == flag).collect();
        Self {
            width,
            height,
            bits,
        }
    }

    /// The mask `all`: every pixel that is not empty (class 0, no flags).
    pub fn all_non_empty(width: usize, height: usize, classes: &[u8]) -> Self {
        assert_eq!(classes.len(), width * height);
        Self {
            width,
            height,
            bits: classes.iter().map(|&c| c != 0).collect(),
        }
    }

    pub fn count(&self) -> usize {
        self.bits.iter().filter(|&&b| b).count()
    }

    /// Erosion with a square window of radius `px`: a pixel stays when every pixel
    /// within `px` (Chebyshev) is in the mask. Outside the image counts as not in the mask.
    pub fn eroded(&self, px: usize) -> Self {
        let (w, h) = (self.width, self.height);
        let mut tmp = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                tmp[y * w + x] =
                    x >= px && x + px < w && (x - px..=x + px).all(|xx| self.bits[y * w + xx]);
            }
        }
        let mut out = vec![false; w * h];
        for y in 0..h {
            if y < px || y + px >= h {
                continue;
            }
            for x in 0..w {
                out[y * w + x] = (y - px..=y + px).all(|yy| tmp[yy * w + x]);
            }
        }
        Self {
            width: w,
            height: h,
            bits: out,
        }
    }

    /// The mask used for metrics: eroded by `MASK_ERODE_PX`.
    pub fn for_metrics(&self) -> Self {
        self.eroded(MASK_ERODE_PX)
    }
}

/// Mean and 95th percentile (nearest rank) of a per-pixel map over a mask.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaskStats {
    pub mean: f64,
    pub p95: f64,
    pub count: usize,
}

impl MaskStats {
    pub fn of(map: &[f32], mask: &Mask) -> Self {
        assert_eq!(map.len(), mask.bits.len());
        let mut v: Vec<f32> = map
            .iter()
            .zip(&mask.bits)
            .filter_map(|(&m, &b)| b.then_some(m))
            .collect();
        if v.is_empty() {
            return Self {
                mean: f64::NAN,
                p95: f64::NAN,
                count: 0,
            };
        }
        let mean = v.iter().map(|&x| f64::from(x)).sum::<f64>() / v.len() as f64;
        v.sort_by(|a, b| a.total_cmp(b));
        let rank = (0.95 * v.len() as f64).ceil() as usize;
        Self {
            mean,
            p95: f64::from(v[rank.max(1) - 1]),
            count: v.len(),
        }
    }
}

/// All metrics of one mask (what `golden noise` and the report use).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaskMetrics {
    pub flip: MaskStats,
    /// Mean of `1 - SSIM`.
    pub one_minus_ssim: f64,
    pub de2000: MaskStats,
}

/// Per-pixel maps of the three metrics; compute once per frame pair, then take any mask.
pub struct PairMaps {
    pub flip: Vec<f32>,
    pub ssim: Vec<f32>,
    pub de2000: Vec<f32>,
}

impl PairMaps {
    pub fn new(reference: &Image, test: &Image) -> Self {
        Self {
            flip: flip::flip_ldr(reference, test, flip::DEFAULT_PPD),
            ssim: ssim::ssim_map(reference, test),
            de2000: de2000::de2000_map(reference, test),
        }
    }

    /// Metrics over a mask (the mask is eroded here).
    pub fn over(&self, mask: &Mask) -> MaskMetrics {
        let m = mask.for_metrics();
        MaskMetrics {
            flip: MaskStats::of(&self.flip, &m),
            one_minus_ssim: 1.0 - MaskStats::of(&self.ssim, &m).mean,
            de2000: MaskStats::of(&self.de2000, &m),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erosion_by_two_pixels() {
        let (w, h) = (12, 10);
        let mut bits = vec![false; w * h];
        for y in 2..9 {
            for x in 3..10 {
                bits[y * w + x] = true;
            }
        }
        let m = Mask {
            width: w,
            height: h,
            bits,
        };
        let e = m.eroded(2);
        // A 7x7 block shrinks to 3x3, centred.
        assert_eq!(e.count(), 9);
        assert!(e.bits[5 * w + 6]);
        assert!(!e.bits[3 * w + 6]);
        assert_eq!(e.eroded(2).count(), 0);
    }

    #[test]
    fn erosion_treats_outside_as_empty() {
        let m = Mask::full(8, 8);
        let e = m.eroded(2);
        assert_eq!(e.count(), 16);
        assert!(!e.bits[0] && e.bits[2 * 8 + 2]);
    }

    #[test]
    fn class_bits() {
        let classes = [
            0u8,
            1,
            4,
            4 | CLASS_BIT_FOG,
            5 | CLASS_BIT_OVERLAY,
            CLASS_BIT_FOG | CLASS_BIT_OVERLAY,
        ];
        let land = Mask::from_class(6, 1, &classes, 1);
        assert_eq!(land.bits, [false, true, false, false, false, false]);
        let forest = Mask::from_class(6, 1, &classes, 4);
        assert_eq!(forest.bits, [false, false, true, true, false, false]);
        let fog = Mask::from_flag(6, 1, &classes, CLASS_BIT_FOG);
        assert_eq!(fog.bits, [false, false, false, true, false, true]);
        let ov = Mask::from_flag(6, 1, &classes, CLASS_BIT_OVERLAY);
        assert_eq!(ov.bits, [false, false, false, false, true, true]);
        let all = Mask::all_non_empty(6, 1, &classes);
        assert_eq!(all.count(), 5);
    }

    #[test]
    fn stats_mean_and_p95() {
        let map: Vec<f32> = (1..=20).map(|v| v as f32).collect();
        let s = MaskStats::of(&map, &Mask::full(20, 1));
        assert_eq!(s.count, 20);
        assert!((s.mean - 10.5).abs() < 1e-12);
        assert_eq!(s.p95, 19.0);
    }
}
