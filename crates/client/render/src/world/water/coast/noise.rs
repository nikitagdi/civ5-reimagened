//! The noise of the coastline: `AddNoiseToSingleCoastline`.
//! The points of a coastline go down a pyramid of the quadratic B-spline (up to four levels,
//! the points padded with the last one so that every level halves cleanly) and come up again;
//! on the way up every point of a level is moved along the normal of the curve by a random
//! distance, and the next level is built from the moved points. The random numbers are the
//! `rand()` of the C runtime of the build thread ([`MsvcRand`]), so the noise is the original's.
//!
//! The wavelet details of the original (`DecomposeWavelet`, `ReconstructWavelet`,
//! `ReconstructScalingFunc_ADD`) are not needed as such: the level `j + 1` is the corner cutting of
//! the moved level `j` plus the detail of the unmoved pyramid, and the detail is the difference
//! between the unmoved level `j + 1` and the corner cutting of the unmoved level `j` (the
//! reconstruction of the pyramid is perfect, so the two are the same sum).

use glam::Vec2;

use super::bspline;
use crate::world::terrain::build::rand::MsvcRand;

/// A coastline with fewer points than this gets no noise.
// re: cvCoastSplineSystem.cpp:32 (kMinNoisePoints), AddNoiseToSingleCoastline 0x088ece2e (`5 < uVar13`)
pub const MIN_POINTS: usize = 6;
/// The deepest pyramid.
// re: AddNoiseToSingleCoastline 0x088ece2e (`3 < local_90` breaks after the fourth level)
pub const MAX_LEVELS: u32 = 4;
/// Base of the fall-off of the amplitude per level.
// re: AddNoiseToSingleCoastline 0x088ece2e (`powf(0.78, 5 - levels)`)
const FALLOFF: f32 = 0.78;
/// Extent of the noise in texels at an amplitude of 1.
// re: AddNoiseToSingleCoastline 0x088ece2e (DAT_0920cb24 = 35.0)
const SCALE: f32 = 35.0;
/// Resolution of the draw.
// re: AddNoiseToSingleCoastline 0x088ece2e (`% 1000`, DAT_0912f5fc = 1000.0)
const RESOLUTION: i32 = 1000;

/// The levels and the number of points at the coarsest of them for a coastline of `n` points:
/// the number of points is halved (rounded up) and one added per level, up to four levels and
/// while it is above 5. `None` if the coastline gets no noise.
// re: AddNoiseToSingleCoastline 0x088ece2e
pub fn plan(n: usize) -> Option<(u32, usize)> {
    if n < MIN_POINTS {
        return None;
    }
    let mut coarse = n;
    let mut levels = 0;
    loop {
        coarse = ((coarse & 1) + coarse) / 2 + 1;
        levels += 1;
        if levels > MAX_LEVELS - 1 || coarse <= 5 {
            break;
        }
    }
    Some((levels, coarse))
}

/// The number of points the pyramid has at its finest level (the coastline padded).
pub fn padded_len(levels: u32, coarse: usize) -> usize {
    (coarse - 2) * (1 << levels) + 2
}

/// How many times the noise of a coastline of `n` points calls `rand()`: every level draws one
/// number per point of the level (the last one is thrown away).
// re: RW-10 D-4: total = (n_r - 2) * (2^(k+1) - 2) + 2k
pub fn draws(n: usize) -> u64 {
    plan(n).map_or(0, |(k, nr)| {
        ((nr - 2) * ((1usize << (k + 1)) - 2) + 2 * k as usize) as u64
    })
}

/// The pyramid of `points` without a change: the points go down `levels` levels and up again;
/// `on_level` may change the points of every level on the way up (`j` counts from 0).
fn refine(
    points: &[Vec2],
    levels: u32,
    coarse: usize,
    mut on_level: impl FnMut(&mut [Vec2], u32),
) -> Vec<Vec2> {
    let n = points.len();
    let mut finest = points.to_vec();
    finest.resize(padded_len(levels, coarse), points[n - 1]);
    // xs[j] is the unmoved level j, xs[0] the coarsest.
    let mut xs = vec![finest];
    for _ in 0..levels {
        let down = bspline::decompose(&xs[xs.len() - 1]);
        xs.push(down);
    }
    xs.reverse();
    let mut p = xs[0].clone();
    for j in 0..levels as usize {
        let rx = bspline::reconstruct(&xs[j]);
        let mut next = bspline::reconstruct(&p);
        for ((q, x), r) in next.iter_mut().zip(&xs[j + 1]).zip(&rx) {
            *q += *x - *r;
        }
        on_level(&mut next, j as u32);
        p = next;
    }
    p.truncate(n);
    p
}

/// Moves a point along the left normal of the gradient by `distance`.
// re: AddNoiseToSingleCoastline 0x088ece2e (`OffsetAlongNormal`)
fn offset_along_normal(point: &mut Vec2, gradient: Vec2, distance: f32) {
    let len = (gradient.x * gradient.x + gradient.y * gradient.y).sqrt();
    // A zero length (two equal points of the padding) leaves the point where it is; the original
    // divides by zero there.
    if len > 0.0 {
        point.x += (gradient.y / len) * distance;
        point.y -= (gradient.x / len) * distance;
    }
}

/// The noise of one level: every point moves by a draw; the first and the last ones as the
/// original does. `amplitude` is the amplitude of the level.
// re: AddNoiseToSingleCoastline 0x088ece2e (the loop over the levels)
fn displace_level(p: &mut [Vec2], amplitude: f32, rng: &mut MsvcRand) {
    let draw = |rng: &mut MsvcRand| -> f32 {
        let r = rng.next() as i32;
        ((r % RESOLUTION) as f32 / RESOLUTION as f32 + -0.5) * amplitude * SCALE
    };
    let count = p.len();
    let last = count - 1;
    let d = draw(rng);
    let g = bspline::gradient(p, 0.0);
    offset_along_normal(&mut p[0], g, d);
    let mut t = 0.5f32;
    for i in 1..last {
        let d = draw(rng);
        let g = bspline::gradient(p, t);
        offset_along_normal(&mut p[i], g, d);
        t += 1.0;
    }
    // The last point draws a number it does not use, and moves by the plain amplitude.
    rng.next();
    let g = bspline::gradient(p, (count - 2) as f32);
    offset_along_normal(&mut p[last], g, amplitude);
}

/// The coastline `points` with the noise of the original; the stream `rng` is advanced by
/// [`draws`]`(points.len())` draws. A short coastline comes back as it is.
// re: cvCoastSplineSystem::AddNoiseToSingleCoastline 0x088ece2e (cvCoastSplineSystem.cpp:317-398)
// SIMPLIFIED(SIMP-039): coast noise form is ours on the MSVC rand stream; not confirmed equal to the original's noise (D-3).
pub fn add_noise(points: &[Vec2], rng: &mut MsvcRand) -> Vec<Vec2> {
    let Some((levels, coarse)) = plan(points.len()) else {
        return points.to_vec();
    };
    let mut amplitude = FALLOFF.powf((5 - levels as i32) as f32);
    refine(points, levels, coarse, |level, _| {
        displace_level(level, amplitude, rng);
        amplitude *= amplitude;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wave(n: usize) -> Vec<Vec2> {
        (0..n)
            .map(|i| {
                let t = i as f32;
                Vec2::new(t * 10.0, 30.0 * (t * 0.3).sin() + t)
            })
            .collect()
    }

    #[test]
    fn the_plan_halves_up_to_four_times_and_stops_at_five_points() {
        assert_eq!(plan(5), None);
        // 6 -> 4: one level; 12 -> 7 -> 5: two; 100 -> 51 -> 27 -> 15 -> 9: four (the cap).
        assert_eq!(plan(6), Some((1, 4)));
        assert_eq!(plan(12), Some((2, 5)));
        assert_eq!(plan(100), Some((4, 9)));
        assert_eq!(plan(3000), Some((4, 190)));
        // The padded line is long enough for the points.
        for n in 6..500 {
            let (k, nr) = plan(n).unwrap();
            assert!(padded_len(k, nr) >= n, "{n}");
            assert_eq!(padded_len(k, nr) % 2, 0);
        }
    }

    #[test]
    fn the_stream_takes_the_number_of_draws_rw_10_gives() {
        // total = (n_r - 2) * (2^(k+1) - 2) + 2k; by hand for n = 12: k = 2, n_r = 5:
        // 3 * 6 + 4 = 22 = (3 * 4 + 2) + ... level 0 has 3 * 2 + 2 = 8 points, level 1 has 14.
        assert_eq!(draws(12), 22);
        assert_eq!(draws(5), 0);
        for n in 0..400 {
            let mut rng = MsvcRand::new(11);
            let out = add_noise(&wave(n.max(1))[..n], &mut rng);
            assert_eq!(rng.calls(), draws(n), "n = {n}");
            assert_eq!(out.len(), n);
        }
    }

    #[test]
    fn without_noise_the_pyramid_gives_the_points_back() {
        for n in [6, 7, 12, 31, 100, 257] {
            let pts = wave(n);
            let (k, nr) = plan(n).unwrap();
            let back = refine(&pts, k, nr, |_, _| {});
            assert_eq!(back.len(), n);
            for (a, b) in pts.iter().zip(&back) {
                assert!((*a - *b).length() < 1e-2, "n = {n}: {a} vs {b}");
            }
        }
    }

    #[test]
    fn the_noise_moves_points_by_at_most_the_extent_and_the_same_seed_gives_the_same_line() {
        let pts = wave(60);
        let mut a = MsvcRand::new(5);
        let mut b = MsvcRand::new(5);
        let na = add_noise(&pts, &mut a);
        let nb = add_noise(&pts, &mut b);
        assert_eq!(na, nb);
        let mut c = MsvcRand::new(6);
        assert_ne!(na, add_noise(&pts, &mut c));
        // The moves are small against the size of the line and not zero.
        let moved: Vec<f32> = pts
            .iter()
            .zip(&na)
            .map(|(p, q)| (*p - *q).length())
            .collect();
        assert!(moved.iter().any(|&m| m > 0.1));
        assert!(
            moved.iter().all(|&m| m.is_finite() && m < 60.0),
            "{moved:?}"
        );
    }

    #[test]
    fn the_first_draw_moves_the_first_point_along_its_normal_by_hand() {
        // 6 points on a straight line along x: n_r = 4, one level (k = 1), amplitude 0.78^4.
        let pts: Vec<Vec2> = (0..6).map(|i| Vec2::new(i as f32 * 8.0, 0.0)).collect();
        let mut rng = MsvcRand::new(3);
        let first = {
            let mut r = MsvcRand::new(3);
            r.next()
        };
        let out = add_noise(&pts, &mut rng);
        let amp = FALLOFF.powf(4.0);
        let d = (((first as i32 % 1000) as f32 / 1000.0) - 0.5) * amp * SCALE;
        // The gradient is (+8, 0): the normal (gy, -gx) / len is (0, -1), so y moves by -d.
        assert!((out[0].y - (0.0 - d)).abs() < 1e-3, "{} {}", out[0].y, -d);
        assert!((out[0].x - 0.0).abs() < 1e-3);
    }
}
