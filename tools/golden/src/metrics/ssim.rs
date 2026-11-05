//! SSIM on BT.601 luma. Z. Wang, A. C. Bovik, H. R. Sheikh, E. P. Simoncelli,
//! "Image quality assessment: from error visibility to structural similarity",
//! IEEE TIP 13(4), 2004: Gaussian window 11x11, sigma 1.5, K1 = 0.01, K2 = 0.03, L = 255.
//! Borders: the window is clamped to the edge pixels, so the map has the image size.

use super::Image;

pub const WINDOW: usize = 11;
pub const SIGMA: f64 = 1.5;
pub const K1: f64 = 0.01;
pub const K2: f64 = 0.03;
pub const DYNAMIC_RANGE: f64 = 255.0;

fn kernel() -> [f64; WINDOW] {
    let r = (WINDOW / 2) as f64;
    let mut k = [0.0; WINDOW];
    for (i, v) in k.iter_mut().enumerate() {
        let d = i as f64 - r;
        *v = (-d * d / (2.0 * SIGMA * SIGMA)).exp();
    }
    let s: f64 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= s);
    k
}

/// Separable Gaussian blur with clamped borders.
fn blur(src: &[f64], w: usize, h: usize, k: &[f64; WINDOW]) -> Vec<f64> {
    let r = (WINDOW / 2) as isize;
    let mut tmp = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (i, kv) in k.iter().enumerate() {
                let xx = (x as isize + i as isize - r).clamp(0, w as isize - 1) as usize;
                s += kv * src[y * w + xx];
            }
            tmp[y * w + x] = s;
        }
    }
    let mut out = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (i, kv) in k.iter().enumerate() {
                let yy = (y as isize + i as isize - r).clamp(0, h as isize - 1) as usize;
                s += kv * tmp[yy * w + x];
            }
            out[y * w + x] = s;
        }
    }
    out
}

/// Per-pixel SSIM, 1 = same.
pub fn ssim_map(a: &Image, b: &Image) -> Vec<f32> {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let (w, h) = (a.width, a.height);
    let k = kernel();
    let x = a.luma601();
    let y = b.luma601();
    let xx: Vec<f64> = x.iter().map(|v| v * v).collect();
    let yy: Vec<f64> = y.iter().map(|v| v * v).collect();
    let xy: Vec<f64> = x.iter().zip(&y).map(|(p, q)| p * q).collect();
    let (mx, my) = (blur(&x, w, h, &k), blur(&y, w, h, &k));
    let (exx, eyy, exy) = (
        blur(&xx, w, h, &k),
        blur(&yy, w, h, &k),
        blur(&xy, w, h, &k),
    );
    let c1 = (K1 * DYNAMIC_RANGE).powi(2);
    let c2 = (K2 * DYNAMIC_RANGE).powi(2);
    (0..w * h)
        .map(|i| {
            let (mx, my) = (mx[i], my[i]);
            let vx = exx[i] - mx * mx;
            let vy = eyy[i] - my * my;
            let cov = exy[i] - mx * my;
            (((2.0 * mx * my + c1) * (2.0 * cov + c2))
                / ((mx * mx + my * my + c1) * (vx + vy + c2))) as f32
        })
        .collect()
}

/// Mean SSIM over the whole image.
pub fn ssim_mean(a: &Image, b: &Image) -> f64 {
    let m = ssim_map(a, b);
    m.iter().map(|&v| f64::from(v)).sum::<f64>() / m.len() as f64
}
