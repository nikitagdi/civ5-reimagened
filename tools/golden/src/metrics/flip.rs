//! ꟻLIP, LDR version: a Rust port of NVIDIA's LDR-FLIP (NVlabs/flip, BSD-3-Clause,
//! Copyright (c) 2020-2024 NVIDIA CORPORATION & AFFILIATES; see NOTICE).
//!
//! Paper: P. Andersson, J. Nilsson, T. Akenine-Moller, M. Oskarsson, K. Astrom, M. D. Fairchild,
//! "FLIP: A Difference Evaluator for Alternating Images", Proc. ACM Comput. Graph. Interact.
//! Tech. 3(2), 2020. Every constant below is the value of the reference implementation.
//!
//! Differences from the reference code, none of which changes the maths: the CSF filters and
//! the feature detectors are applied as sums of separable passes (the 2D kernels there are
//! sums of products of 1D Gaussians), with replicated borders as in the reference.

use std::f64::consts::PI;

/// Pixels per degree of the reference implementation: a 0.7 m wide 4K monitor seen from 0.7 m.
pub const DEFAULT_PPD: f64 = (0.7 * 3840.0 / 0.7) * PI / 180.0;

/// Exponent of the colour error (`qc`).
const QC: f64 = 0.7;
/// Exponent of the feature error (`qf`).
const QF: f64 = 0.5;
/// Multiplier of the maximum colour error (`pc`) and the target value of `pc * cmax` (`pt`).
const PC: f64 = 0.4;
const PT: f64 = 0.95;
/// Stability term of the reference.
const EPS: f32 = 1e-15;
/// Feature detector width in degrees of visual angle (`w`).
const FEATURE_W: f64 = 0.082;

/// Contrast sensitivity parameters `(a1, b1, a2, b2)` of the achromatic, red-green and
/// blue-yellow channels.
const CSF_A: [f64; 4] = [1.0, 0.0047, 0.0, 1e-5];
const CSF_RG: [f64; 4] = [1.0, 0.0053, 0.0, 1e-5];
const CSF_BY: [f64; 4] = [34.1, 0.04, 13.5, 0.025];

/// D65 white of the reference and its reciprocal.
const WHITE: [f64; 3] = [0.950_428_545, 1.0, 1.088_900_371];
/// Linear sRGB to XYZ of the reference.
const RGB_TO_XYZ: [[f64; 3]; 3] = [
    [
        10135552.0 / 24577794.0,
        8788810.0 / 24577794.0,
        4435075.0 / 24577794.0,
    ],
    [
        2613072.0 / 12288897.0,
        8788810.0 / 12288897.0,
        887015.0 / 12288897.0,
    ],
    [
        1425312.0 / 73733382.0,
        8788810.0 / 73733382.0,
        70074185.0 / 73733382.0,
    ],
];
/// XYZ to linear sRGB of the reference.
const XYZ_TO_RGB: [[f64; 3]; 3] = [
    [3.241003275, -1.537398934, -0.498615861],
    [-0.969224334, 1.875930071, 0.041554224],
    [0.055639423, -0.204011202, 1.057148933],
];

type Plane = Vec<f32>;

fn mul3(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    super::color::mul3(m, v)
}

fn srgb_decode(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn lab_f(t: f32) -> f32 {
    const DELTA: f32 = 6.0 / 29.0;
    if t > DELTA * DELTA * DELTA {
        t.cbrt()
    } else {
        t / (3.0 * DELTA * DELTA) + 4.0 / 29.0
    }
}

/// Linear RGB to L*a*b* (white of the reference), then the Hunt adjustment.
fn linrgb_to_hunt_lab(rgb: [f64; 3]) -> [f32; 3] {
    let xyz = mul3(&RGB_TO_XYZ, rgb);
    let fx = lab_f((xyz[0] / WHITE[0]) as f32);
    let fy = lab_f((xyz[1] / WHITE[1]) as f32);
    let fz = lab_f((xyz[2] / WHITE[2]) as f32);
    let l = 116.0 * fy - 16.0;
    let a = 500.0 * (fx - fy);
    let b = 200.0 * (fy - fz);
    [l, 0.01 * l * a, 0.01 * l * b]
}

fn hyab(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dl = a[0] - b[0];
    let root = (dl * dl).max(EPS).sqrt();
    let da = a[1] - b[1];
    let db = a[2] - b[2];
    root + (da * da + db * db).sqrt()
}

/// Separable clamp-to-edge convolution of `src` (w x h) with `kx` along rows, then `ky` along columns.
/// Kernels are odd length and symmetric in position (centre at the middle element).
fn convolve_sep(src: &[f32], w: usize, h: usize, kx: &[f32], ky: &[f32]) -> Plane {
    let rx = (kx.len() / 2) as isize;
    let ry = (ky.len() / 2) as isize;
    let mut tmp = vec![0.0f32; w * h];
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        for x in 0..w {
            let mut s = 0.0f32;
            for (i, k) in kx.iter().enumerate() {
                let xx = (x as isize + i as isize - rx).clamp(0, w as isize - 1) as usize;
                s += k * row[xx];
            }
            tmp[y * w + x] = s;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for (i, k) in ky.iter().enumerate() {
            let yy = (y as isize + i as isize - ry).clamp(0, h as isize - 1) as usize;
            let trow = &tmp[yy * w..(yy + 1) * w];
            let orow = &mut out[y * w..(y + 1) * w];
            for x in 0..w {
                orow[x] += k * trow[x];
            }
        }
    }
    out
}

fn to_f32(v: &[f64]) -> Vec<f32> {
    v.iter().map(|&x| x as f32).collect()
}

/// Radius of the CSF kernels, shared by the three channels (largest `b` of all of them).
fn csf_radius(ppd: f64) -> usize {
    let max_b = [
        CSF_A[1], CSF_A[3], CSF_RG[1], CSF_RG[3], CSF_BY[1], CSF_BY[3],
    ]
    .into_iter()
    .fold(0.0f64, f64::max);
    (3.0 * (max_b / (2.0 * PI * PI)).sqrt() * ppd).ceil() as usize
}

/// Filters one opponent channel with its CSF: a sum of up to two Gaussians, normalised so the
/// whole 2D kernel sums to 1.
fn csf_filter(src: &[f32], w: usize, h: usize, p: &[f64; 4], ppd: f64, r: usize) -> Plane {
    let dx = 1.0 / ppd;
    let mut total = 0.0f64;
    let mut parts: Vec<(f64, Vec<f64>)> = Vec::new();
    for (a, b) in [(p[0], p[1]), (p[2], p[3])] {
        if a == 0.0 {
            continue;
        }
        let scale = a * (PI / b).sqrt();
        let k: Vec<f64> = (-(r as isize)..=r as isize)
            .map(|x| (-PI * PI * ((x as f64) * dx).powi(2) / b).exp())
            .collect();
        let s: f64 = k.iter().sum();
        total += scale * s * s;
        parts.push((scale, k));
    }
    let mut out = vec![0.0f32; w * h];
    for (scale, k) in parts {
        let k32 = to_f32(&k);
        let f = convolve_sep(src, w, h, &k32, &k32);
        let wgt = (scale / total) as f32;
        for (o, v) in out.iter_mut().zip(f) {
            *o += wgt * v;
        }
    }
    out
}

/// sRGB (8 bit) to YCxCz planes.
fn to_ycxcz(rgb: &[u8]) -> [Plane; 3] {
    let n = rgb.len() / 3;
    let (mut y, mut cx, mut cz) = (vec![0.0f32; n], vec![0.0f32; n], vec![0.0f32; n]);
    for i in 0..n {
        let lin = [
            f64::from(srgb_decode(f32::from(rgb[i * 3]) / 255.0)),
            f64::from(srgb_decode(f32::from(rgb[i * 3 + 1]) / 255.0)),
            f64::from(srgb_decode(f32::from(rgb[i * 3 + 2]) / 255.0)),
        ];
        let xyz = mul3(&RGB_TO_XYZ, lin);
        let (xn, yn, zn) = (xyz[0] / WHITE[0], xyz[1] / WHITE[1], xyz[2] / WHITE[2]);
        y[i] = (116.0 * yn - 16.0) as f32;
        cx[i] = (500.0 * (xn - yn)) as f32;
        cz[i] = (200.0 * (yn - zn)) as f32;
    }
    [y, cx, cz]
}

/// YCxCz to linear RGB clamped to 0..1.
fn ycxcz_to_linrgb(y: f32, cx: f32, cz: f32) -> [f32; 3] {
    let yn = (f64::from(y) + 16.0) / 116.0;
    let xn = yn + f64::from(cx) / 500.0;
    let zn = yn - f64::from(cz) / 200.0;
    let rgb = mul3(&XYZ_TO_RGB, [xn * WHITE[0], yn * WHITE[1], zn * WHITE[2]]);
    [
        rgb[0].clamp(0.0, 1.0) as f32,
        rgb[1].clamp(0.0, 1.0) as f32,
        rgb[2].clamp(0.0, 1.0) as f32,
    ]
}

fn linrgb_f32_to_hunt_lab(rgb: [f32; 3]) -> [f32; 3] {
    linrgb_to_hunt_lab([f64::from(rgb[0]), f64::from(rgb[1]), f64::from(rgb[2])])
}

/// Edge and point detector responses (norm over the x and y directions) of a luminance plane.
fn feature_norms(yn: &[f32], w: usize, h: usize, ppd: f64) -> (Plane, Plane) {
    let sd = 0.5 * FEATURE_W * ppd;
    let r = (3.0 * sd).ceil() as isize;
    let xs: Vec<f64> = (-r..=r).map(|x| x as f64).collect();
    let g: Vec<f64> = xs
        .iter()
        .map(|x| (-(x * x) / (2.0 * sd * sd)).exp())
        .collect();
    let gsum: f64 = g.iter().sum();
    let normalised = |raw: Vec<f64>| -> Vec<f32> {
        // Positive weights of the 2D kernel sum to +1, negative ones to -1.
        let pos: f64 = raw.iter().filter(|&&v| v > 0.0).sum::<f64>() * gsum;
        let neg: f64 = -raw.iter().filter(|&&v| v < 0.0).sum::<f64>() * gsum;
        raw.iter()
            .map(|&v| if v < 0.0 { v / neg } else { v / pos } as f32)
            .collect()
    };
    let edge = normalised(xs.iter().zip(&g).map(|(x, g)| -x * g).collect());
    let point = normalised(
        xs.iter()
            .zip(&g)
            .map(|(x, g)| (x * x / (sd * sd) - 1.0) * g)
            .collect(),
    );
    let g32 = to_f32(&g);
    let norm = |k: &[f32]| -> Plane {
        let fx = convolve_sep(yn, w, h, k, &g32);
        let fy = convolve_sep(yn, w, h, &g32, k);
        fx.iter()
            .zip(&fy)
            .map(|(a, b)| (a * a + b * b).sqrt())
            .collect()
    };
    (norm(&edge), norm(&point))
}

/// LDR-FLIP error map (0 = no difference, 1 = largest), one value per pixel.
/// `reference` and `test` are 8-bit sRGB with the same size.
pub fn flip_ldr(reference: &super::Image, test: &super::Image, ppd: f64) -> Vec<f32> {
    assert_eq!(
        (reference.width, reference.height),
        (test.width, test.height)
    );
    let (w, h) = (reference.width, reference.height);
    let n = w * h;
    let r = csf_radius(ppd);

    let yr = to_ycxcz(&reference.rgb);
    let yt = to_ycxcz(&test.rgb);

    // Colour pipeline.
    let filt = |p: &[Plane; 3]| -> [Plane; 3] {
        [
            csf_filter(&p[0], w, h, &CSF_A, ppd, r),
            csf_filter(&p[1], w, h, &CSF_RG, ppd, r),
            csf_filter(&p[2], w, h, &CSF_BY, ppd, r),
        ]
    };
    let fr = filt(&yr);
    let ft = filt(&yt);

    let green = linrgb_to_hunt_lab([0.0, 1.0, 0.0]);
    let blue = linrgb_to_hunt_lab([0.0, 0.0, 1.0]);
    let cmax = f64::from(hyab(green, blue)).powf(QC);
    let pccmax = PC * cmax;

    // Feature pipeline on the unfiltered luminance.
    let norm_y = |p: &[Plane; 3]| -> Plane { p[0].iter().map(|&y| (y + 16.0) / 116.0).collect() };
    let (er, pr) = feature_norms(&norm_y(&yr), w, h, ppd);
    let (et, pt) = feature_norms(&norm_y(&yt), w, h, ppd);

    let inv_sqrt2 = (1.0 / 2.0f64.sqrt()) as f32;
    (0..n)
        .map(|i| {
            let lr = linrgb_f32_to_hunt_lab(ycxcz_to_linrgb(fr[0][i], fr[1][i], fr[2][i]));
            let lt = linrgb_f32_to_hunt_lab(ycxcz_to_linrgb(ft[0][i], ft[1][i], ft[2][i]));
            let pow_de = f64::from(hyab(lr, lt)).powf(QC);
            let de_c = if pow_de < pccmax {
                (PT / pccmax) * pow_de
            } else {
                PT + ((pow_de - pccmax) / (cmax - pccmax)) * (1.0 - PT)
            };
            let de_f = (er[i] - et[i]).abs().max((pt[i] - pr[i]).abs());
            let de_f = (inv_sqrt2 * de_f.max(EPS)).powf(QF as f32);
            de_c.powf(f64::from(1.0 - de_f)) as f32
        })
        .collect()
}

/// Mean of the error map.
pub fn flip_mean(reference: &super::Image, test: &super::Image) -> f64 {
    let m = flip_ldr(reference, test, DEFAULT_PPD);
    m.iter().map(|&v| f64::from(v)).sum::<f64>() / m.len() as f64
}

#[cfg(test)]
mod tests {
    use super::super::Image;
    use super::*;

    /// Deterministic test picture: smooth gradients, a hard edge and a fine pattern.
    pub(crate) fn picture(w: usize, h: usize, variant: u32) -> Image {
        let mut rgb = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                let (xf, yf) = (x as u32, y as u32);
                let mut c = [
                    (xf * 255 / (w as u32 - 1)) as i32,
                    (yf * 255 / (h as u32 - 1)) as i32,
                    ((xf * 7 + yf * 13) % 256) as i32,
                ];
                if x > w / 2 && y > h / 3 {
                    c = [200, 90, 30];
                }
                if (x / 2 + y / 2) % 2 == 0 && x < w / 3 {
                    c = [c[0] / 2 + 30, c[1] / 2, c[2]];
                }
                if variant == 1 {
                    c = [c[0] + 12, c[1] - 9, c[2] + 20];
                    if y > h / 2 && x < w / 2 {
                        c = [c[2], c[0], c[1]];
                    }
                }
                rgb.extend(c.map(|v| v.clamp(0, 255) as u8));
            }
        }
        Image::new(w, h, rgb)
    }

    #[test]
    fn ppd_of_the_reference() {
        assert!((DEFAULT_PPD - 67.020643).abs() < 1e-5);
        assert_eq!(csf_radius(DEFAULT_PPD), 10);
    }

    #[test]
    fn identical_images_have_no_error() {
        let a = picture(64, 48, 0);
        let m = flip_ldr(&a, &a, DEFAULT_PPD);
        let max = m.iter().cloned().fold(0.0f32, f32::max);
        assert!(max < 1e-4, "max {max}");
    }

    #[test]
    fn error_grows_with_the_difference() {
        let a = picture(64, 48, 0);
        let b = picture(64, 48, 1);
        let mean = flip_mean(&a, &b);
        assert!(mean > 0.05 && mean < 1.0, "mean {mean}");
        // Black against white is the largest difference the metric reports.
        let black = Image::new(32, 32, vec![0; 32 * 32 * 3]);
        let white = Image::new(32, 32, vec![255; 32 * 32 * 3]);
        let m = flip_mean(&black, &white);
        assert!(m > 0.9, "black/white {m}");
        // Symmetric colour error: swapping the images keeps the colour part, so means are close.
        let back = flip_mean(&b, &a);
        assert!((mean - back).abs() < 0.05, "{mean} vs {back}");
    }
}
