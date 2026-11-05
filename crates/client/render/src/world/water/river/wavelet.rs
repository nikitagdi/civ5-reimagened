//! The one-level B-spline wavelet of the river control points (`RiverWaveletBuffer` with its
//! `Fun_087f8f6c` / `Fun_087f901e`). `SmoothLowFrequency` and `RandomizeLowFrequency` take a river
//! of `n` points (even) apart into a coarse level of `n / 2 + 1` points and a detail of `n / 2 - 1`
//! points, change the coarse level and put the river together again.
//!
//! The scaling part (`DecomposeScalingFunc` / `ReconstructScalingFunc`, the corner cutting) is the
//! one of the coast code; the wavelet part (`DecomposeWavelet` / `ReconstructWavelet`) is here.
//! The original keeps the levels in one buffer (`m_pPoints`: the fine level at 0, the coarse level
//! at `n`, the detail behind it); here they are two vectors.

use glam::Vec2;

use crate::world::water::coast::bspline;

/// The river after one decomposition step.
// re: RiverWaveletBuffer (cvRiverSimSystem.h) after Fun_087f8f6c
#[derive(Clone, Debug, PartialEq)]
pub struct RiverWavelet {
    /// The coarse level (`m_uiCount = n / 2 + 1` points).
    pub coarse: Vec<Vec2>,
    /// The detail (`n / 2 - 1` points), kept as it is while the coarse level changes.
    pub detail: Vec<Vec2>,
}

/// Weights of `DecomposeWavelet` over four neighbours `x[2k] .. x[2k + 3]`.
// re: B_SPLINE::DecomposeWavelet<FGXVector2*,FGXVector2*,0,0> 0x087f9be4 (DAT_09149c6c = 0.25,
// DAT_09149c50 = -0.75, DAT_09149c84 = 0.75, DAT_0920c6f8 = -0.25)
const DECOMPOSE_DETAIL: [f32; 4] = [0.25, -0.75, 0.75, -0.25];

/// `DecomposeWavelet`: `n` (even) points give `n / 2 - 1` detail points.
// re: B_SPLINE::DecomposeWavelet<FGXVector2*,FGXVector2*,0,0> 0x087f9be4
fn decompose_wavelet(f: &[Vec2]) -> Vec<Vec2> {
    let n = f.len();
    (0..n / 2 - 1)
        .map(|k| {
            let i = 2 * k;
            // The original sums in the order of the four products, left to right.
            f[i] * DECOMPOSE_DETAIL[0]
                + f[i + 1] * DECOMPOSE_DETAIL[1]
                + f[i + 2] * DECOMPOSE_DETAIL[2]
                + f[i + 3] * DECOMPOSE_DETAIL[3]
        })
        .collect()
}

/// `ReconstructWavelet`: `m` detail points give `2 m + 2` points. The point `2j` is
/// `0.75 d[j-1] - 0.25 d[j]` and `2j + 1` is `0.25 d[j-1] - 0.75 d[j]` (with `d[-1] = d[m] = 0`):
/// the transpose of [`decompose_wavelet`].
// re: B_SPLINE::ReconstructWavelet<FGXVector2*,FGXVector2*,0,0> 0x087f979c (DAT_0920c6f8 = -0.25,
// DAT_09149c50 = -0.75, DAT_09149c84 = 0.75, DAT_09149c6c = 0.25)
fn reconstruct_wavelet(d: &[Vec2]) -> Vec<Vec2> {
    let m = d.len();
    let mut out = Vec::with_capacity(2 * m + 2);
    for j in 0..=m {
        let prev = if j == 0 { Vec2::ZERO } else { d[j - 1] };
        let next = if j == m { Vec2::ZERO } else { d[j] };
        out.push(prev * 0.75 + next * -0.25);
        out.push(prev * 0.25 + next * -0.75);
    }
    out
}

impl RiverWavelet {
    /// One decomposition step of a river of an even number of points, at least 4.
    // re: Fun_087f8f6c 0x087f8f6c (`DecomposeWavelet` then `DecomposeScalingFunc`)
    pub fn decompose(points: &[Vec2]) -> Self {
        Self {
            detail: decompose_wavelet(points),
            coarse: bspline::decompose(points),
        }
    }

    /// One reconstruction step back to the fine level.
    // re: Fun_087f901e 0x087f901e (`ReconstructWavelet` writes, `ReconstructScalingFunc_ADD` adds)
    pub fn reconstruct(&self) -> Vec<Vec2> {
        let mut out = reconstruct_wavelet(&self.detail);
        for (o, s) in out.iter_mut().zip(bspline::reconstruct(&self.coarse)) {
            *o += s;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wiggle(n: usize) -> Vec<Vec2> {
        (0..n)
            .map(|i| {
                let t = i as f32;
                Vec2::new(
                    t * 5.0 + (t * 1.7).sin() * 3.0,
                    40.0 + (t * 0.9).cos() * 6.0,
                )
            })
            .collect()
    }

    #[test]
    fn the_levels_have_the_sizes_of_the_original() {
        for n in [4usize, 6, 8, 12, 30] {
            let w = RiverWavelet::decompose(&wiggle(n));
            assert_eq!(w.coarse.len(), n / 2 + 1, "{n}");
            assert_eq!(w.detail.len(), n / 2 - 1, "{n}");
            assert_eq!(w.reconstruct().len(), n, "{n}");
        }
    }

    #[test]
    fn decompose_then_reconstruct_gives_the_points_back() {
        for n in [4usize, 6, 8, 12, 30] {
            let f = wiggle(n);
            let back = RiverWavelet::decompose(&f).reconstruct();
            for (i, (a, b)) in f.iter().zip(&back).enumerate() {
                assert!(a.distance(*b) < 1e-3, "n = {n}, point {i}: {a} {b}");
            }
        }
    }

    #[test]
    fn the_detail_of_a_straight_river_is_zero() {
        let f: Vec<Vec2> = (0..10).map(|i| Vec2::new(i as f32 * 3.0, 7.0)).collect();
        let w = RiverWavelet::decompose(&f);
        assert!(w.detail.iter().all(|d| d.length() < 1e-4), "{:?}", w.detail);
    }

    #[test]
    fn moving_a_coarse_point_moves_the_river_near_it_only() {
        let f = wiggle(16);
        let mut w = RiverWavelet::decompose(&f);
        w.coarse[4] += Vec2::new(0.0, 8.0);
        let moved = w.reconstruct();
        // The corner cutting of knot 4 reaches the fine points 6 ..= 9.
        for (i, (a, b)) in f.iter().zip(&moved).enumerate() {
            let d = a.distance(*b);
            if (6..=9).contains(&i) {
                assert!(d > 0.1, "{i}");
            } else {
                assert!(d < 1e-3, "{i}: {d}");
            }
        }
    }
}
