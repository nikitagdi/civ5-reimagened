//! The uniform quadratic B-spline of the coast code (`B_SPLINE::*` templates of the original),
//! on points of two coordinates and on control points with styles. The gradient and the value of
//! a segment, and the corner cutting (reconstruct) and its left inverse (decompose) that the
//! noise and the levels use.
//!
//! Open question for the whole file: the weights of the gradient and the value are `.bss` data of
//! the original, written at start-up and zero in the dump (`DAT_0c0582bc..` and
//! `DAT_0c058260..`), so they are the textbook weights of the uniform quadratic B-spline, which
//! the rest of the code agrees with (the corner cutting 0.75 / 0.25 is read from the code).
// TODO(RW-10): read the .bss constants of `B_SPLINE::EvalGradient` / `EvalScalingFunc` from a run
// or from the static initializer; the textbook values are used until then.

use glam::Vec2;

use super::lines::CoastCP;

/// The segment a parameter falls in and the fraction inside it: three control points from the
/// returned index. The last segment is evaluated at its end, so a parameter at or past `n - 2`
/// gives the points `n - 3 ..= n - 1` and the fraction 1.
// re: B_SPLINE::EvalGradient<CoastCP,...,0,0> 0x088ebf0a (`iVar12 < a1 + -2 ? (a2, t - floor(t)) : (a2 - 1, 1.0)`)
pub fn locate(n: usize, t: f32) -> (usize, f32) {
    let i0 = t as i32;
    if i0 < n as i32 - 2 {
        (i0.max(0) as usize, t - t.floor())
    } else {
        ((i0 - 1).max(0) as usize, 1.0)
    }
}

/// Weights of the gradient at fraction `f`: the derivative of the three quadratic pieces.
// re: B_SPLINE::EvalGradient 0x088ebf0a (`c1 * f + c0` per point; the constants are the derivative of the uniform B-spline)
pub(crate) fn gradient_weights(f: f32) -> [f32; 3] {
    [f - 1.0, 1.0 - 2.0 * f, f]
}

/// Weights of the value at fraction `f`.
// re: B_SPLINE::EvalScalingFunc 0x088ebd91 (`c2 * f^2 + c1 * f + c0` per point)
pub(crate) fn value_weights(f: f32) -> [f32; 3] {
    let f2 = f * f;
    [0.5 * f2 - f + 0.5, -f2 + f + 0.5, 0.5 * f2]
}

fn at<T: Copy>(pts: &[T], i: usize) -> T {
    pts[i.min(pts.len() - 1)]
}

/// Gradient of the spline through `pts` at the parameter `t` (the first `n` points count).
pub fn gradient(pts: &[Vec2], t: f32) -> Vec2 {
    let (i, f) = locate(pts.len(), t);
    let w = gradient_weights(f);
    let (p0, p1, p2) = (at(pts, i), at(pts, i + 1), at(pts, i + 2));
    Vec2::new(
        p1.x * w[1] + p0.x * w[0] + p2.x * w[2],
        p1.y * w[1] + p0.y * w[0] + p2.y * w[2],
    )
}

/// Value of the spline through `pts` at the parameter `t`.
pub fn value(pts: &[Vec2], t: f32) -> Vec2 {
    let (i, f) = locate(pts.len(), t);
    let w = value_weights(f);
    let (p0, p1, p2) = (at(pts, i), at(pts, i + 1), at(pts, i + 2));
    Vec2::new(
        p1.x * w[1] + p0.x * w[0] + p2.x * w[2],
        p1.y * w[1] + p0.y * w[0] + p2.y * w[2],
    )
}

/// Gradient of the position of control points; the style bytes and the weight do not take part.
pub fn gradient_cp(pts: &[CoastCP], t: f32) -> Vec2 {
    let (i, f) = locate(pts.len(), t);
    let w = gradient_weights(f);
    let (p0, p1, p2) = (at(pts, i), at(pts, i + 1), at(pts, i + 2));
    Vec2::new(
        p1.pos.x * w[1] + p0.pos.x * w[0] + p2.pos.x * w[2],
        p1.pos.y * w[1] + p0.pos.y * w[0] + p2.pos.y * w[2],
    )
}

/// Value of a control point spline: position and blend weight are blended, the styles are the
/// ones of the first of the three points.
// re: B_SPLINE::EvalScalingFunc<CoastCP,...> 0x088ebd91 (`unk_c` is copied from `pCVar10[0]`)
pub fn value_cp(pts: &[CoastCP], t: f32) -> CoastCP {
    let (i, f) = locate(pts.len(), t);
    let w = value_weights(f);
    let (p0, p1, p2) = (at(pts, i), at(pts, i + 1), at(pts, i + 2));
    CoastCP {
        pos: Vec2::new(
            p1.pos.x * w[1] + p0.pos.x * w[0] + p2.pos.x * w[2],
            p1.pos.y * w[1] + p0.pos.y * w[0] + p2.pos.y * w[2],
        ),
        blend: w[1] * p1.blend + w[0] * p0.blend + w[2] * p2.blend,
        style_a: p0.style_a,
        style_b: p0.style_b,
    }
}

/// One step of the corner cutting: `m` points become `2m - 2`; the point `2j` is
/// `0.75 c[j] + 0.25 c[j+1]` and the point `2j+1` is `0.25 c[j] + 0.75 c[j+1]`.
// re: B_SPLINE::ReconstructScalingFunc<FGXVector2,...,0,0> 0x087bb5c1 (0.75 / 0.25 of DAT_09149c84, DAT_09149c6c)
pub fn reconstruct(c: &[Vec2]) -> Vec<Vec2> {
    let mut out = Vec::with_capacity(c.len().saturating_sub(1) * 2);
    for w in c.windows(2) {
        out.push(w[0] * 0.75 + w[1] * 0.25);
        out.push(w[0] * 0.25 + w[1] * 0.75);
    }
    out
}

/// The same corner cutting on control points. The style bytes of both new points are the ones of
/// `c[j+1]`; the blend weight is blended like the position.
// re: B_SPLINE::ReconstructScalingFunc<CoastCP,...,0,0> 0x088ebab2 (`a3[-1].unk_c = a2[-1].unk_c`)
pub fn reconstruct_cp(c: &[CoastCP]) -> Vec<CoastCP> {
    let mut out = Vec::with_capacity(c.len().saturating_sub(1) * 2);
    for w in c.windows(2) {
        let (a, b) = (w[0], w[1]);
        out.push(CoastCP {
            pos: a.pos * 0.75 + b.pos * 0.25,
            blend: a.blend * 0.75 + b.blend * 0.25,
            style_a: b.style_a,
            style_b: b.style_b,
        });
        out.push(CoastCP {
            pos: a.pos * 0.25 + b.pos * 0.75,
            blend: a.blend * 0.25 + b.blend * 0.75,
            style_a: b.style_a,
            style_b: b.style_b,
        });
    }
    out
}

/// The inverse of [`reconstruct`] on the scaling part: `n` (even) points become `n / 2 + 1`. The
/// ends are `1.5 f[0] - 0.5 f[1]`, the middle is the filter `-0.25 0.75 0.75 -0.25`.
// re: B_SPLINE::DecomposeScalingFunc<FGXVector2*,...,0,0> 0x087f9c97 (DAT_09146b70 = 1.5 / -0.5, DAT_0920c6f8 = -0.25, DAT_09149c84 = 0.75)
pub fn decompose(f: &[Vec2]) -> Vec<Vec2> {
    let n = f.len();
    debug_assert!(
        n >= 4 && n.is_multiple_of(2),
        "decompose needs an even count of 4 or more"
    );
    let mut out = Vec::with_capacity(n / 2 + 1);
    out.push(f[0] * 1.5 + f[1] * -0.5);
    for i in 1..n / 2 {
        let (a, b, c, d) = (f[2 * i - 2], f[2 * i - 1], f[2 * i], f[2 * i + 1]);
        out.push(d * -0.25 + c * 0.75 + b * 0.75 + a * -0.25);
    }
    out.push(f[n - 1] * 1.5 + f[n - 2] * -0.5);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(n: usize) -> Vec<Vec2> {
        (0..n)
            .map(|i| Vec2::new(i as f32 * 2.0, i as f32))
            .collect()
    }

    #[test]
    fn the_gradient_of_a_straight_line_is_its_direction_everywhere() {
        let pts = line(8);
        for t in [0.0, 0.5, 1.0, 2.5, 5.0, 6.0, 5.99] {
            let g = gradient(&pts, t);
            assert!((g - Vec2::new(2.0, 1.0)).length() < 1e-5, "t = {t}: {g}");
        }
    }

    #[test]
    fn the_gradient_at_a_half_is_the_central_difference() {
        let pts = [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 3.0),
            Vec2::new(5.0, 4.0),
            Vec2::new(6.0, 9.0),
        ];
        // t = 0.5: points 0..=2, weights (-0.5, 0, 0.5).
        assert_eq!(gradient(&pts, 0.5), (pts[2] - pts[0]) * 0.5);
        // t = 0: the first difference; at the last segment the last difference.
        assert_eq!(gradient(&pts, 0.0), pts[1] - pts[0]);
        assert_eq!(gradient(&pts, 2.0), pts[3] - pts[2]);
    }

    #[test]
    fn the_value_weights_sum_to_one_and_the_value_is_between_the_neighbours() {
        for f in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
            let w = value_weights(f);
            assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        }
        let pts = line(5);
        // A knot is the middle of two points.
        assert!((value(&pts, 1.0) - (pts[1] + pts[2]) * 0.5).length() < 1e-6);
    }

    #[test]
    fn corner_cutting_then_decomposing_gives_the_points_back() {
        let c: Vec<Vec2> = [(0.0, 0.0), (3.0, 1.0), (4.0, 5.0), (9.0, 2.0), (1.0, 7.0)]
            .iter()
            .map(|&(x, y)| Vec2::new(x, y))
            .collect();
        let fine = reconstruct(&c);
        assert_eq!(fine.len(), 2 * c.len() - 2);
        let back = decompose(&fine);
        assert_eq!(back.len(), c.len());
        for (a, b) in c.iter().zip(&back) {
            assert!((*a - *b).length() < 1e-5, "{a} {b}");
        }
    }

    #[test]
    fn the_control_point_version_cuts_the_same_and_takes_the_next_styles() {
        let mk = |x: f32, a: u8, b: u8, w: f32| CoastCP {
            pos: Vec2::new(x, 0.0),
            blend: w,
            style_a: a,
            style_b: b,
        };
        let c = [mk(0.0, 1, 2, 0.0), mk(4.0, 3, 4, 1.0), mk(8.0, 5, 6, 1.0)];
        let f = reconstruct_cp(&c);
        assert_eq!(f.len(), 4);
        assert_eq!(f[0].pos.x, 1.0);
        assert_eq!(f[1].pos.x, 3.0);
        assert_eq!((f[0].style_a, f[1].style_b), (3, 4));
        assert_eq!((f[2].style_a, f[3].style_b), (5, 6));
        assert_eq!(f[0].blend, 0.25);
    }
}
