//! CIEDE2000 colour difference, kL = kC = kH = 1.
//! Formulas: G. Sharma, W. Wu, E. N. Dalal, "The CIEDE2000 Color-Difference Formula:
//! Implementation Notes, Supplementary Test Data, and Mathematical Observations",
//! Color Research & Application 30(1), 2005, section 3.

use super::color::srgb8_to_lab;
use super::{Image, Mask, MaskStats};

/// 25^7 in the chroma-dependent terms (Sharma 2005, eqs. 2 and 17).
const POW25_7: f64 = 6_103_515_625.0;

/// CIEDE2000 between two CIELAB colours.
pub fn ciede2000(lab1: [f64; 3], lab2: [f64; 3]) -> f64 {
    let [l1, a1, b1] = lab1;
    let [l2, a2, b2] = lab2;
    let c1 = a1.hypot(b1);
    let c2 = a2.hypot(b2);
    let cbar = (c1 + c2) / 2.0;
    let cbar7 = cbar.powi(7);
    let g = 0.5 * (1.0 - (cbar7 / (cbar7 + POW25_7)).sqrt());
    let a1p = (1.0 + g) * a1;
    let a2p = (1.0 + g) * a2;
    let c1p = a1p.hypot(b1);
    let c2p = a2p.hypot(b2);
    let hue = |b: f64, ap: f64| {
        if b == 0.0 && ap == 0.0 {
            0.0
        } else {
            let h = b.atan2(ap).to_degrees();
            if h < 0.0 { h + 360.0 } else { h }
        }
    };
    let h1p = hue(b1, a1p);
    let h2p = hue(b2, a2p);

    let dlp = l2 - l1;
    let dcp = c2p - c1p;
    let dhp_deg = if c1p * c2p == 0.0 {
        0.0
    } else {
        let d = h2p - h1p;
        if d > 180.0 {
            d - 360.0
        } else if d < -180.0 {
            d + 360.0
        } else {
            d
        }
    };
    let dhp = 2.0 * (c1p * c2p).sqrt() * (dhp_deg.to_radians() / 2.0).sin();

    let lbarp = (l1 + l2) / 2.0;
    let cbarp = (c1p + c2p) / 2.0;
    let hbarp = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let cosd = |deg: f64| deg.to_radians().cos();
    let t =
        1.0 - 0.17 * cosd(hbarp - 30.0) + 0.24 * cosd(2.0 * hbarp) + 0.32 * cosd(3.0 * hbarp + 6.0)
            - 0.20 * cosd(4.0 * hbarp - 63.0);
    let dtheta = 30.0 * (-((hbarp - 275.0) / 25.0).powi(2)).exp();
    let cbarp7 = cbarp.powi(7);
    let rc = 2.0 * (cbarp7 / (cbarp7 + POW25_7)).sqrt();
    let l50 = (lbarp - 50.0).powi(2);
    let sl = 1.0 + 0.015 * l50 / (20.0 + l50).sqrt();
    let sc = 1.0 + 0.045 * cbarp;
    let sh = 1.0 + 0.015 * cbarp * t;
    let rt = -(2.0 * dtheta).to_radians().sin() * rc;

    let x = dlp / sl;
    let y = dcp / sc;
    let z = dhp / sh;
    (x * x + y * y + z * z + rt * y * z).sqrt()
}

/// ΔE2000 per pixel between two 8-bit sRGB images (D65).
pub fn de2000_map(a: &Image, b: &Image) -> Vec<f32> {
    assert_eq!((a.width, a.height), (b.width, b.height));
    // Lab of each distinct 8-bit colour is computed once; frames repeat colours a lot.
    let mut cache: std::collections::HashMap<[u8; 3], [f64; 3]> = std::collections::HashMap::new();
    let mut lab = |p: [u8; 3]| *cache.entry(p).or_insert_with(|| srgb8_to_lab(p));
    (0..a.width * a.height)
        .map(|i| {
            let (pa, pb) = (a.pixel(i), b.pixel(i));
            if pa == pb {
                0.0
            } else {
                ciede2000(lab(pa), lab(pb)) as f32
            }
        })
        .collect()
}

/// Mean and p95 of ΔE2000 over a mask.
pub fn de2000_stats(a: &Image, b: &Image, mask: &Mask) -> MaskStats {
    MaskStats::of(&de2000_map(a, b), mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_is_zero() {
        assert_eq!(ciede2000([50.0, 10.0, -20.0], [50.0, 10.0, -20.0]), 0.0);
    }
}
