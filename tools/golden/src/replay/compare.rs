//! Comparison of a replayed target with the captured `after`.
//!
//! Both are sample 0 of a multisampled target (the replay draws into 4 samples too), compared on
//! the pixels whose sample 0 our draw covered. UNORM targets: `|d| <= 2/255`; float targets:
//! `|d| <= 2/255 * max(1, |reference|)`.

use serde::{Deserialize, Serialize};

use crate::image::Rgba8;

/// 2/255.
pub const TOLERANCE: f32 = 2.0 / 255.0;

/// A pixel with its values and the channel of the largest error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Worst {
    pub x: u32,
    pub y: u32,
    pub ours: [f32; 4],
    pub reference: [f32; 4],
    pub channel: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stats {
    /// Pixels compared: those whose sample 0 the draw covered.
    pub compared: usize,
    /// Pixels over the tolerance.
    pub failing: usize,
    /// Largest `|ours - reference|` of any channel.
    pub max_abs: f32,
    /// 99.9 % of the pixels have at most this error (largest channel of the pixel).
    pub p99_9: f32,
    /// Largest `|d| / tolerance`; the comparison passes when it is at most 1.
    pub max_ratio: f32,
    pub passed: bool,
    pub worst: Option<Worst>,
    /// The failing pixels with the largest error (up to 6), for the report.
    #[serde(default)]
    pub examples: Vec<Worst>,
}

fn tolerance(reference: f32, unorm: bool) -> f32 {
    if unorm {
        TOLERANCE
    } else {
        TOLERANCE * reference.abs().max(1.0)
    }
}

/// Compares `channels` channels of the pixels in `cover`. The second result is the error of each
/// pixel in units of its tolerance (0 where nothing was compared), for the error image.
pub fn compare(
    ours: &[[f32; 4]],
    reference: &[[f32; 4]],
    cover: &[bool],
    w: usize,
    channels: usize,
    unorm: bool,
) -> (Stats, Vec<f32>) {
    let mut errs = Vec::new();
    let mut ratio_map = vec![0.0f32; cover.len()];
    let mut stats = Stats {
        compared: 0,
        failing: 0,
        max_abs: 0.0,
        p99_9: 0.0,
        max_ratio: 0.0,
        passed: true,
        worst: None,
        examples: Vec::new(),
    };
    for i in 0..cover.len() {
        if !cover[i] {
            continue;
        }
        stats.compared += 1;
        let mut px_err = 0.0f32;
        let mut px_ratio = 0.0f32;
        for c in 0..channels.min(4) {
            let (a, b) = (ours[i][c], reference[i][c]);
            // Equal values (also two infinities) and two NaNs agree; a lone NaN or infinity
            // differs from everything.
            let d = if a == b || (a.is_nan() && b.is_nan()) {
                0.0
            } else {
                let d = (a - b).abs();
                if d.is_nan() { f32::INFINITY } else { d }
            };
            let ratio = d / tolerance(b, unorm);
            px_err = px_err.max(d);
            px_ratio = px_ratio.max(ratio);
            if ratio > stats.max_ratio {
                stats.max_ratio = ratio;
                stats.worst = Some(Worst {
                    x: (i % w) as u32,
                    y: (i / w) as u32,
                    ours: ours[i],
                    reference: reference[i],
                    channel: c,
                });
            }
        }
        stats.max_abs = stats.max_abs.max(px_err);
        if px_ratio > 1.0 {
            stats.failing += 1;
        }
        ratio_map[i] = px_ratio;
        errs.push(px_err);
    }
    errs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if !errs.is_empty() {
        let k = ((errs.len() as f64 * 0.999).ceil() as usize).clamp(1, errs.len()) - 1;
        stats.p99_9 = errs[k];
    }
    stats.passed = stats.max_ratio <= 1.0;
    let mut bad: Vec<usize> = (0..cover.len()).filter(|&i| ratio_map[i] > 1.0).collect();
    bad.sort_by(|&a, &b| {
        ratio_map[b]
            .partial_cmp(&ratio_map[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for &i in bad.iter().take(6) {
        let c = (0..channels.min(4))
            .max_by(|&p, &q| {
                let dp = (ours[i][p] - reference[i][p]).abs();
                let dq = (ours[i][q] - reference[i][q]).abs();
                dp.partial_cmp(&dq).unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(0);
        stats.examples.push(Worst {
            x: (i % w) as u32,
            y: (i / w) as u32,
            ours: ours[i],
            reference: reference[i],
            channel: c,
        });
    }
    (stats, ratio_map)
}

/// The error map as an image: black where nothing was compared, green (brighter with the error)
/// within the tolerance, red beyond it.
pub fn error_image(ratio: &[f32], cover: &[bool], w: usize, h: usize) -> Rgba8 {
    let mut rgba = vec![0u8; w * h * 4];
    for i in 0..w * h {
        let p = &mut rgba[i * 4..i * 4 + 4];
        p[3] = 255;
        if !cover[i] {
            continue;
        }
        let r = ratio[i];
        if r <= 1.0 {
            let t = (r * 200.0) as u8;
            p.copy_from_slice(&[40 + t / 2, 80 + t / 2, 40, 255]);
        } else {
            p.copy_from_slice(&[255, 0, 0, 255]);
        }
    }
    Rgba8 {
        width: w,
        height: h,
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tolerance_is_absolute_for_unorm_and_scaled_for_floats() {
        let ours = vec![[0.5, 0.0, 0.0, 1.0]; 9];
        let mut reference = ours.clone();
        reference[4][0] = 0.5 + 1.9 / 255.0;
        let cover = vec![true; 9];
        let (s, _) = compare(&ours, &reference, &cover, 3, 4, true);
        assert!(s.passed, "{s:?}");
        reference[4][0] = 0.5 + 2.1 / 255.0;
        let (s, _) = compare(&ours, &reference, &cover, 3, 4, true);
        assert!(!s.passed && s.failing == 1);
        // A float value of 10: the tolerance is 20/255.
        let mut a = vec![[10.0, 0.0, 0.0, 1.0]; 9];
        let r = a.clone();
        a[4][0] = 10.0 + 19.0 / 255.0;
        let (s, _) = compare(&a, &r, &cover, 3, 4, false);
        assert!(s.passed, "{s:?}");
        a[4][0] = 10.0 + 21.0 / 255.0;
        let (s, _) = compare(&a, &r, &cover, 3, 4, false);
        assert!(!s.passed);
    }

    #[test]
    fn only_covered_pixels_count() {
        let ours = vec![[0.0; 4]; 4];
        let mut reference = ours.clone();
        reference[3][1] = 1.0;
        let (s, _) = compare(&ours, &reference, &[true, true, true, false], 2, 4, true);
        assert!(s.passed && s.compared == 3);
        let (s, _) = compare(&ours, &reference, &[true, true, true, true], 2, 4, true);
        assert!(!s.passed && s.failing == 1 && s.examples.len() == 1);
    }
}
