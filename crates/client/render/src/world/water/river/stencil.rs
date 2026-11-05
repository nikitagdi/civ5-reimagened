//! The stencil of the rivers: `cvRiverSimSystem::CreateStencilMeshes`. The
//! centre line of every river is walked in steps of 0.1 of the spline parameter and every step
//! makes a quad of the half width 5 texels (two triangles), grown by a quarter on the sides that
//! face south. `StartupGame` puts the triangles into the stencil of the terrain with the bits
//! `'\t'`, the ones the layout of the forests reads.

use glam::{Vec2, Vec3};

use super::RiverSim;
use crate::world::terrain::stencil::TerrainStencil;

/// The stencil bits of the quads: bit 0 and bit 3.
// re: cvRiverSimSystem::StartupGame (`AddStencil(vertices, '\t', true)`)
pub const RIVER_STENCIL_BITS: u8 = 0x09;

/// A river with fewer control points makes no stencil.
// re: cvRiverSimSystem::CreateStencilMeshes 0x087f43da (`kMinBankPoints`)
const MIN_POINTS: usize = 4;
/// The step of the spline parameter.
// re: cvRiverSimSystem::CreateStencilMeshes 0x087f43da (`kStencilStep`)
const STEP: f32 = 0.1;
/// The loop runs while the parameter is at most the last segment plus this.
// re: cvRiverSimSystem::CreateStencilMeshes 0x087f43da (`kStencilEndEpsilon`)
const END_EPSILON: f32 = 1.0e-4;
/// Half the width of the quad, in texels.
// re: cvRiverSimSystem::CreateStencilMeshes 0x087f43da (`kStencilHalfWidth`)
const HALF_WIDTH: f32 = 5.0;
/// How far the side that faces south moves out, in half widths.
// re: cvRiverSimSystem::CreateStencilMeshes 0x087f43da (`kStencilSouthGrow`)
const SOUTH_GROW: f32 = 1.25;
/// The direction the growth looks to (-y).
// re: cvRiverSimSystem::CreateStencilMeshes 0x087f43da (`kStencilSouth`)
const SOUTH: Vec2 = Vec2::new(0.0, -1.0);

/// The segment a parameter falls in and the fraction inside it, as the evaluators of
/// `B_SPLINE` (begin type 1, end type 0) do: the raw index of the parameter, the index of the
/// first of the three points, and the fraction. The last segment is evaluated at its end.
// re: B_SPLINE::EvalScalingFunc<FGXVector3,FGXVector3*,1,0> 0x087fb45f, EvalGradient<..,1,0> 0x087fb2c5
// (`i < n - 2 ? (i, t - floorf(t)) : (i - 1, 1.0)`, the weights depend on the raw `i == 0`)
fn locate(n: usize, t: f32) -> (i32, usize, f32) {
    let i0 = t as i32;
    if i0 < n as i32 - 2 {
        (i0, i0.max(0) as usize, t - t.floor())
    } else {
        (i0, (i0 - 1).max(0) as usize, 1.0)
    }
}

/// The three points of the segment; an index past the end repeats the last point.
fn three(pts: &[Vec3], first: usize) -> [Vec3; 3] {
    let at = |i: usize| pts[i.min(pts.len() - 1)];
    [at(first), at(first + 1), at(first + 2)]
}

/// Weights of the value at fraction `f`. Begin type 1 is the clamped start: the first segment
/// (raw index 0) is the one of the spline whose first knot is repeated, so the curve starts at
/// the first control point.
// re: B_SPLINE::EvalScalingFunc<..,1,0> 0x087fb45f (`.bss` constants written at start-up by
// the initializer at 0x084b444a: first segment (1 - 2f + f^2, 2f - 1.5f^2, 0.5f^2), the other
// segments (0.5 - f + 0.5f^2, 0.5 + f - f^2, 0.5f^2))
fn value_weights(first_segment: bool, f: f32) -> [f32; 3] {
    let f2 = f * f;
    if first_segment {
        [1.0 - 2.0 * f + f2, 2.0 * f - 1.5 * f2, 0.5 * f2]
    } else {
        [0.5 - f + 0.5 * f2, 0.5 + f - f2, 0.5 * f2]
    }
}

/// Weights of the gradient at fraction `f` (the derivatives of [`value_weights`]).
// re: B_SPLINE::EvalGradient<..,1,0> 0x087fb2c5 (constants of the same initializer: first
// segment (2f - 2, 2 - 3f, f), the others (f - 1, 1 - 2f, f))
fn gradient_weights(first_segment: bool, f: f32) -> [f32; 3] {
    if first_segment {
        [2.0 * f - 2.0, 2.0 - 3.0 * f, f]
    } else {
        [f - 1.0, 1.0 - 2.0 * f, f]
    }
}

/// Value of the spline through `pts` at the parameter `t`.
// re: B_SPLINE::EvalScalingFunc<FGXVector3,FGXVector3*,1,0> 0x087fb45f
pub fn value(pts: &[Vec3], t: f32) -> Vec3 {
    let (raw, first, f) = locate(pts.len(), t);
    let w = value_weights(raw == 0, f);
    let p = three(pts, first);
    p[1] * w[1] + p[0] * w[0] + p[2] * w[2]
}

/// Gradient of the spline through `pts` at the parameter `t`.
// re: B_SPLINE::EvalGradient<FGXVector3,FGXVector3*,1,0> 0x087fb2c5
pub fn gradient(pts: &[Vec3], t: f32) -> Vec3 {
    let (raw, first, f) = locate(pts.len(), t);
    let w = gradient_weights(raw == 0, f);
    let p = three(pts, first);
    p[1] * w[1] + p[0] * w[0] + p[2] * w[2]
}

/// D3DXVec2Normalize: the zero vector stays zero.
fn normalize2(v: Vec2) -> Vec2 {
    v.normalize_or_zero()
}

/// How much the corner moves out: the facing of its side against the south, clamped to [0, 1].
fn grow(facing: Vec2) -> f32 {
    (facing.y * SOUTH.y + facing.x * SOUTH.x).clamp(0.0, 1.0)
}

/// The side vector (up x gradient, normalized, times the half width) at a parameter.
fn side_at(center: &[Vec3], t: f32) -> Vec3 {
    let up = Vec3::Z;
    let g = gradient(center, t).normalize_or_zero();
    let side = Vec3::new(
        up.y * g.z - up.z * g.y,
        up.z * g.x - up.x * g.z,
        up.x * g.y - up.y * g.x,
    )
    .normalize_or_zero();
    side * HALF_WIDTH
}

/// The two edge points of the quad at a parameter: the left one (A) moves out by its growth, the
/// right one (B) too.
fn edge_points(center: Vec3, side: Vec3) -> (Vec2, Vec2) {
    let s = Vec2::new(side.x, side.y);
    let c = Vec2::new(center.x, center.y);
    let a = c - s - grow(normalize2(-s)) * s * SOUTH_GROW;
    let b = c + s + grow(normalize2(s)) * s * SOUTH_GROW;
    (a, b)
}

/// `CreateStencilMeshes`: the triangles of every river, in groups of three, in texels of the field.
// re: cvRiverSimSystem::CreateStencilMeshes 0x087f43da
// (the bank splines are evaluated by the original too, but only the centre line shapes the stencil)
pub fn stencil_triangles(sim: &RiverSim) -> Vec<[Vec2; 3]> {
    let mut out = Vec::new();
    for (river, spline) in sim.rivers.iter().enumerate() {
        let n = spline.points.len();
        if n < MIN_POINTS {
            continue;
        }
        let heights = sim.heights.get(river);
        let center: Vec<Vec3> = spline
            .points
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let z = heights.and_then(|h| h.get(i)).copied().unwrap_or(0.0);
                Vec3::new(p.x, p.y, z)
            })
            .collect();

        let last_t = (n - 2) as f32;
        let end_t = last_t + END_EPSILON;
        let mut t = STEP;
        while t <= end_t {
            let t1 = if last_t < t { last_t } else { t };
            let t0 = -STEP + t1;
            let (c1, c0) = (value(&center, t1), value(&center, t0));
            let (a1, b1) = edge_points(c1, side_at(&center, t1));
            let (a0, b0) = edge_points(c0, side_at(&center, t0));
            out.extend_from_slice(&[[a0, a1, b1], [a0, b1, b0]]);
            t = t1 + STEP;
        }
    }
    out
}

/// Puts the triangles into the stencil of the terrain. The stencil has half the texels of the
/// field, so the corners go to half their value.
// re: cvRiverSimSystem::StartupGame (`TerrainStencil::AddStencil(vertices, '\t', true)`)
// TODO(RW-10): `TerrainStencil::AddStencil` (0x087fd588) converts the vertices with
// `Fun_087fdaa4`, not in the dump; the field texel / 2 is the scale of `SampleWorldPosition`
// (the same shortcut as the coast stencil).
pub fn add_to(sim: &RiverSim, stencil: &mut TerrainStencil) {
    for t in stencil_triangles(sim) {
        stencil.add_triangle(t.map(|p| p * 0.5), RIVER_STENCIL_BITS);
    }
}

#[cfg(test)]
mod tests {
    use super::super::RiverSpline;
    use super::*;

    const Y: f32 = 40.0;

    /// A straight river along +x with `n` control points, 4 texels apart, from x = 20.
    fn straight(n: usize) -> RiverSim {
        let points: Vec<Vec3> = (0..n)
            .map(|i| Vec3::new(20.0 + 4.0 * i as f32, Y, 0.0))
            .collect();
        let spline = RiverSpline {
            points,
            ends_at_mouth: false,
        };
        RiverSim {
            rivers: vec![spline.clone()],
            left_banks: vec![spline.clone()],
            right_banks: vec![spline],
            heights: vec![(0..n).map(|i| 1000.0 - 10.0 * i as f32).collect()],
            ..RiverSim::default()
        }
    }

    fn center_of(sim: &RiverSim) -> Vec<Vec3> {
        sim.rivers[0]
            .points
            .iter()
            .zip(&sim.heights[0])
            .map(|(p, &h)| Vec3::new(p.x, p.y, h))
            .collect()
    }

    #[test]
    fn begin_type_one_starts_at_the_first_point_and_ends_in_the_middle_of_the_last_two() {
        let c = center_of(&straight(8));
        assert!((value(&c, 0.0) - c[0]).length() < 1e-4);
        // The last segment is evaluated at its end: (0, 0.5, 0.5) of the last three points.
        let last = value(&c, 6.0);
        assert!((last - (c[6] + c[7]) * 0.5).length() < 1e-4, "{last}");
        // The plain spline would start in the middle of the first two points.
        assert!((value(&c, 0.0) - (c[0] + c[1]) * 0.5).length() > 1.0);
    }

    #[test]
    fn the_weights_sum_to_one_and_the_gradients_to_zero() {
        for first in [true, false] {
            for f in [0.0f32, 0.3, 0.5, 1.0] {
                assert!((value_weights(first, f).iter().sum::<f32>() - 1.0).abs() < 1e-6);
                assert!(gradient_weights(first, f).iter().sum::<f32>().abs() < 1e-6);
            }
        }
    }

    #[test]
    fn the_value_is_continuous_over_the_knots_and_the_gradient_matches_the_difference() {
        // A bent river, so that the weights matter.
        let c: Vec<Vec3> = [
            (0.0, 0.0, 0.0),
            (3.0, 1.0, 5.0),
            (4.0, 5.0, 2.0),
            (9.0, 2.0, 7.0),
            (1.0, 7.0, 1.0),
            (6.0, 9.0, 3.0),
        ]
        .iter()
        .map(|&(x, y, z)| Vec3::new(x, y, z))
        .collect();
        let eps = 1e-3f32;
        for knot in 1..4 {
            let k = knot as f32;
            let (below, above) = (value(&c, k - eps), value(&c, k + eps));
            assert!(
                (below - above).length() < 0.05,
                "knot {knot}: {below} {above}"
            );
            // The gradient is continuous too (quadratic B-spline: C1).
            let (gb, ga) = (gradient(&c, k - eps), gradient(&c, k + eps));
            assert!((gb - ga).length() < 0.05, "knot {knot}: {gb} {ga}");
        }
        for t in [0.2f32, 0.7, 1.4, 2.5, 3.9] {
            let h = 1e-2;
            let numeric = (value(&c, t + h) - value(&c, t - h)) / (2.0 * h);
            assert!((gradient(&c, t) - numeric).length() < 0.05, "t = {t}");
        }
    }

    #[test]
    fn the_stencil_has_two_triangles_per_step() {
        let sim = straight(8);
        // The loop of the original, in single precision: t = 0.1, 0.2, ... up to 6 + 1e-4.
        let mut steps = 0;
        let mut t = 0.1f32;
        while t <= 6.0 + 1.0e-4 {
            steps += 1;
            t = if 6.0 < t { 6.0 } else { t } + 0.1;
        }
        assert!((60..=61).contains(&steps), "{steps}");
        assert_eq!(stencil_triangles(&sim).len(), 2 * steps);
    }

    #[test]
    fn the_triangles_lie_in_the_band_around_the_centre_line() {
        let tris = stencil_triangles(&straight(8));
        // x: from the first point to the middle of the last two; y: half width to the north
        // (+y), half width plus the south growth to the south.
        let (x0, x1) = (20.0 - 1e-3, 20.0 + 4.0 * 6.5 + 1e-3);
        let (y0, y1) = (
            Y - HALF_WIDTH * (1.0 + SOUTH_GROW) - 1e-3,
            Y + HALF_WIDTH + 1e-3,
        );
        let mut south = f32::MAX;
        for p in tris.iter().flatten() {
            assert!(p.x >= x0 && p.x <= x1, "{p}");
            assert!(p.y >= y0 && p.y <= y1, "{p}");
            south = south.min(p.y);
        }
        // The south side is really grown, the north side is not.
        assert!((south - (Y - 11.25)).abs() < 1e-3, "{south}");
        let north = tris.iter().flatten().map(|p| p.y).fold(f32::MIN, f32::max);
        assert!((north - (Y + 5.0)).abs() < 1e-3, "{north}");
    }

    #[test]
    fn a_river_of_three_points_emits_nothing() {
        assert!(stencil_triangles(&straight(3)).is_empty());
        assert!(!stencil_triangles(&straight(4)).is_empty());
    }

    #[test]
    fn the_stencil_gets_bits_near_the_river_and_none_far_away() {
        let sim = straight(8);
        let mut stencil = TerrainStencil::new(128, 128);
        add_to(&sim, &mut stencil);
        // On the centre line, in the middle of the river.
        assert_eq!(stencil.sample_texel(40, Y as i32, 0xff), 0x09);
        // To the south the band is wider than to the north.
        assert_eq!(stencil.sample_texel(40, Y as i32 - 9, 0xff), 0x09);
        assert_eq!(stencil.sample_texel(40, Y as i32 + 9, 0xff), 0);
        // Far away.
        assert_eq!(stencil.sample_texel(40, 100, 0xff), 0);
        assert_eq!(stencil.sample_texel(40, 5, 0xff), 0);
        assert_eq!(stencil.sample_texel(110, Y as i32, 0xff), 0);
        assert_eq!(stencil.sample_texel(10, Y as i32, 0xff), 0);
        assert!(stencil.pages_allocated() > 0);
    }
}
