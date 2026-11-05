//! Metric tests without game data.

use golden::metrics::de2000::ciede2000;
use golden::metrics::flip::{DEFAULT_PPD, flip_ldr, flip_mean};
use golden::metrics::ssim::{ssim_map, ssim_mean};
use golden::metrics::{Image, Mask, PairMaps};

/// The 34 test pairs of G. Sharma, W. Wu, E. N. Dalal, Color Research & Application 30(1), 2005:
/// L1 a1 b1 L2 a2 b2 and the expected CIEDE2000 difference.
#[rustfmt::skip]
const SHARMA: [([f64; 3], [f64; 3], f64); 34] = [
    ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
    ([50.0, 3.1571, -77.2803], [50.0, 0.0, -82.7485], 2.8615),
    ([50.0, 2.8361, -74.0200], [50.0, 0.0, -82.7485], 3.4412),
    ([50.0, -1.3802, -84.2814], [50.0, 0.0, -82.7485], 1.0000),
    ([50.0, -1.1848, -84.8006], [50.0, 0.0, -82.7485], 1.0000),
    ([50.0, -0.9009, -85.5211], [50.0, 0.0, -82.7485], 1.0000),
    ([50.0, 0.0, 0.0], [50.0, -1.0, 2.0], 2.3669),
    ([50.0, -1.0, 2.0], [50.0, 0.0, 0.0], 2.3669),
    ([50.0, 2.49, -0.001], [50.0, -2.49, 0.0009], 7.1792),
    ([50.0, 2.49, -0.001], [50.0, -2.49, 0.0010], 7.1792),
    ([50.0, 2.49, -0.001], [50.0, -2.49, 0.0011], 7.2195),
    ([50.0, 2.49, -0.001], [50.0, -2.49, 0.0012], 7.2195),
    ([50.0, -0.001, 2.49], [50.0, 0.0009, -2.49], 4.8045),
    ([50.0, -0.001, 2.49], [50.0, 0.0010, -2.49], 4.8045),
    ([50.0, -0.001, 2.49], [50.0, 0.0011, -2.49], 4.7461),
    ([50.0, 2.5, 0.0], [50.0, 0.0, -2.5], 4.3065),
    ([50.0, 2.5, 0.0], [73.0, 25.0, -18.0], 27.1492),
    ([50.0, 2.5, 0.0], [61.0, -5.0, 29.0], 22.8977),
    ([50.0, 2.5, 0.0], [56.0, -27.0, -3.0], 31.9030),
    ([50.0, 2.5, 0.0], [58.0, 24.0, 15.0], 19.4535),
    ([50.0, 2.5, 0.0], [50.0, 3.1736, 0.5854], 1.0000),
    ([50.0, 2.5, 0.0], [50.0, 3.2972, 0.0], 1.0000),
    ([50.0, 2.5, 0.0], [50.0, 1.8634, 0.5757], 1.0000),
    ([50.0, 2.5, 0.0], [50.0, 3.2592, 0.3350], 1.0000),
    ([60.2574, -34.0099, 36.2677], [60.4626, -34.1751, 39.4387], 1.2644),
    ([63.0109, -31.0961, -5.8663], [62.8187, -29.7946, -4.0864], 1.2630),
    ([61.2901, 3.7196, -5.3901], [61.4292, 2.2480, -4.9620], 1.8731),
    ([35.0831, -44.1164, 3.7933], [35.0232, -40.0716, 1.5901], 1.8645),
    ([22.7233, 20.0904, -46.6940], [23.0331, 14.9730, -42.5619], 2.0373),
    ([36.4612, 47.8580, 18.3852], [36.2715, 50.5065, 21.2231], 1.4146),
    ([90.8027, -2.0831, 1.4410], [91.1528, -1.6435, 0.0447], 1.4441),
    ([90.9257, -0.5406, -0.9208], [88.6381, -0.8985, -0.7239], 1.5381),
    ([6.7747, -0.2908, -2.4247], [5.8714, -0.0985, -2.2286], 0.6377),
    ([2.0776, 0.0795, -1.1350], [0.9033, -0.0636, -0.5514], 0.9082),
];

#[test]
fn de2000_sharma_34_pairs() {
    for (i, (a, b, want)) in SHARMA.iter().enumerate() {
        let got = ciede2000(*a, *b);
        assert!((got - want).abs() < 1e-4, "pair {}: {got} vs {want}", i + 1);
    }
}

/// Deterministic picture: gradients, a hard edge, a fine pattern.
fn picture(w: usize, h: usize, variant: u32) -> Image {
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

/// Pseudo-random grey-ish noise picture (xorshift), amplitude `amp` around 128.
fn noise_picture(w: usize, h: usize, seed: u32, amp: i32) -> Image {
    let mut s = seed;
    let mut rgb = Vec::with_capacity(w * h * 3);
    for _ in 0..w * h {
        for _ in 0..3 {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            rgb.push((128 + (s % (2 * amp as u32 + 1)) as i32 - amp) as u8);
        }
    }
    Image::new(w, h, rgb)
}

#[test]
fn ssim_identical_is_one() {
    let a = picture(64, 48, 0);
    assert!(ssim_map(&a, &a).iter().all(|&v| v == 1.0));
    let n = noise_picture(40, 40, 7, 60);
    assert_eq!(ssim_mean(&n, &n), 1.0);
}

#[test]
fn ssim_orders_distortions() {
    let a = picture(96, 64, 0);
    // Brightness shift: structure is kept, SSIM stays high.
    let shifted = Image::new(
        a.width,
        a.height,
        a.rgb.iter().map(|&v| v.saturating_add(10)).collect(),
    );
    // Noise: structure is damaged.
    let noisy = Image::new(
        a.width,
        a.height,
        a.rgb
            .iter()
            .zip(noise_picture(96, 64, 3, 40).rgb)
            .map(|(&v, n)| (i32::from(v) + i32::from(n) - 128).clamp(0, 255) as u8)
            .collect(),
    );
    // Unrelated picture.
    let other = noise_picture(96, 64, 11, 100);
    let (s_shift, s_noise, s_other) = (
        ssim_mean(&a, &shifted),
        ssim_mean(&a, &noisy),
        ssim_mean(&a, &other),
    );
    assert!(s_shift > 0.95, "{s_shift}");
    assert!(
        s_noise < s_shift && s_noise > s_other,
        "{s_shift} {s_noise} {s_other}"
    );
    assert!(s_other < 0.3, "{s_other}");
    // Symmetry.
    assert!((ssim_mean(&shifted, &a) - s_shift).abs() < 1e-12);
}

#[test]
fn flip_identical_is_zero_and_different_is_not() {
    let a = picture(64, 48, 0);
    assert!(flip_mean(&a, &a) < 1e-4);
    let b = picture(64, 48, 1);
    assert!(flip_mean(&a, &b) > 0.05);
}

/// FLIP value of a fixed pair from an independent NumPy implementation of the published reference
/// code (full 2D kernels, no separable passes).
/// Regenerate with `python tests/fixtures/flip_ref.py` (needs NumPy).
const FLIP_NUMPY_MEAN_A_B: f64 = 0.408775;

#[test]
fn flip_matches_numpy_port() {
    let a = picture(64, 48, 0);
    let b = picture(64, 48, 1);
    let got = flip_mean(&a, &b);
    assert!(
        (got - FLIP_NUMPY_MEAN_A_B).abs() < 1e-4,
        "{got} vs {FLIP_NUMPY_MEAN_A_B}"
    );
}

#[test]
fn flip_error_map_is_bounded() {
    let a = picture(64, 48, 0);
    let b = picture(64, 48, 1);
    let m = flip_ldr(&a, &b, DEFAULT_PPD);
    assert_eq!(m.len(), 64 * 48);
    assert!(m.iter().all(|&v| (0.0..=1.0).contains(&v)));
}

#[test]
fn metrics_over_a_masked_region() {
    let a = picture(64, 48, 0);
    let b = picture(64, 48, 1);
    let maps = PairMaps::new(&a, &b);
    let full = maps.over(&Mask::full(64, 48));
    // Interior of the full mask: 60 x 44 pixels after the 2-pixel erosion.
    assert_eq!(full.flip.count, 60 * 44);
    let same = PairMaps::new(&a, &a).over(&Mask::full(64, 48));
    assert!(same.flip.mean < 1e-4);
    assert_eq!(same.one_minus_ssim, 0.0);
    assert_eq!(same.de2000.mean, 0.0);
    assert!(full.de2000.mean > 1.0);
}

/// Full-frame run (1920x1080). Slow in debug builds: `cargo test -p golden --release -- --ignored`.
#[test]
#[ignore]
fn full_frame_pair() {
    let a = picture(1920, 1080, 0);
    let b = picture(1920, 1080, 1);
    let t = std::time::Instant::now();
    let maps = PairMaps::new(&a, &b);
    let m = maps.over(&Mask::full(1920, 1080));
    eprintln!("full frame metrics: {:?} {m:?}", t.elapsed());
    assert!(m.flip.mean > 0.0);
}
