//! The control points of a civilization's border as the original makes them (`GenerateHexSegment`,
//! `GameSplineDatabase::GenerateHexSide`, `SmoothControlPoints`, `ExpandControlPoints`,
//! `RandomizeControlPoints`; RE-B18 item 3 BRD-1) and the B-spline through them.
//!
//! An edge against unowned land or sea gives two points on the edge, a quarter from each end, of
//! weight 0; an edge between two owners gives three, `0.75 + corner_bias` of the way from each end
//! and the middle, of weight 1. On each line, in this order: a 5-tap smoothing of the points and the
//! weights (a point keeps `w` of itself), a push of `expansion` along the normal into the territory
//! and a shake of up to `randomness` along it, both by `1 - w`; so the edges against nobody are
//! smooth, a little inside and shaken, and the edges between two owners stay where they are.
// re: GenerateHexSegment 0x08a4a8ca, GenerateHexSide 0x087d4f28, SmoothControlPoints 0x08a4945e,
// ExpandControlPoints 0x08a49846, RandomizeControlPoints 0x08a49b92, FRandom 0x087fbf02
// (CultureBorders ctor 0x08a47ffc: modulus 0xffffcfef, scale 0x3df0000301109066)

use glam::Vec2;

use super::points::{ControlLine, ControlPoint, chains};
use super::smooth::Samples;
use crate::hexgrid::GridShape;
use crate::world::overlay::OwnersInput;

/// The 5-tap kernel of `SmoothControlPoints`.
pub const SMOOTH_KERNEL: [f32; 5] = [0.15, 0.2, 0.3, 0.2, 0.15];

/// The steps of the curve parameter per span of the B-spline (0.25: four samples).
// re: the sampling of the border spline at 0.25 of the parameter (8 samples per 2-point edge)
pub const SAMPLES_PER_SPAN: usize = 4;

/// A border line: its control points and their weights (0 against nobody, 1 between owners).
#[derive(Debug, Clone, PartialEq)]
pub struct CultureLine {
    pub line: ControlLine,
    pub weights: Vec<f32>,
}

/// The control lines of `owner` with their weights.
pub fn culture_lines(
    input: &OwnersInput,
    shape: &GridShape,
    owner: i8,
    corner_bias: f32,
) -> Vec<CultureLine> {
    let w = input.width as i32;
    chains(input, shape, owner)
        .into_iter()
        .map(|(chain, closed)| {
            let first = chain[0];
            let t = first.b - first.a;
            let to_centre = first.center - (first.a + first.b) * 0.5;
            let inward_left = t.perp_dot(to_centre) > 0.0;
            let mut points = Vec::new();
            let mut weights = Vec::new();
            for e in &chain {
                let (x, y) = (e.cell as i32 % w, e.cell as i32 / w);
                let other = shape
                    .neighbor(x, y, e.dir)
                    .map_or(-1, |(nx, ny)| input.owners[(ny * w + nx) as usize]);
                // From the start of the edge, and the weight.
                let (along, weight): (&[f32], f32) = if other < 0 {
                    (&[0.25, 0.75], 0.0)
                } else {
                    let near = 0.25 - corner_bias;
                    (&[near, 0.5, 1.0 - near], 1.0)
                };
                for (slot, &t) in along.iter().enumerate() {
                    points.push(ControlPoint {
                        position: e.a.lerp(e.b, t),
                        cell: e.cell,
                        key: e
                            .cell
                            .wrapping_mul(24)
                            .wrapping_add(e.dir.index() as u32 * 4 + slot as u32),
                    });
                    weights.push(weight);
                }
            }
            CultureLine {
                line: ControlLine {
                    points,
                    closed,
                    inward_left,
                },
                weights,
            }
        })
        .collect()
}

/// Index `i` of a line of `n`: round a closed one, held at the ends of an open one.
fn at(i: isize, n: usize, closed: bool) -> usize {
    if closed {
        i.rem_euclid(n as isize) as usize
    } else {
        i.clamp(0, n as isize - 1) as usize
    }
}

/// The unit normal into the territory at point `i` (from its neighbours).
fn inward(points: &[Vec2], i: usize, closed: bool, inward_left: bool) -> Vec2 {
    let n = points.len();
    let t = (points[at(i as isize + 1, n, closed)] - points[at(i as isize - 1, n, closed)])
        .normalize_or_zero();
    if inward_left {
        Vec2::new(-t.y, t.x)
    } else {
        Vec2::new(t.y, -t.x)
    }
}

/// The first draw of an `FRandom` seeded with `seed`, in [0, 1).
pub fn frandom_first(seed: u32) -> f32 {
    let scale = f64::from_bits(0x3df0_0003_0110_9066);
    let s = seed.wrapping_mul(0x343fd).wrapping_add(0x0026_9ec3) % 0xffff_cfef;
    (f64::from(s) * scale) as f32
}

/// The seed of a point: `(uint)(y y + x x + x + y)`.
fn seed_of(p: Vec2) -> u32 {
    (p.y * p.y + p.x * p.x + p.x + p.y) as i64 as u32
}

/// The control points smoothed, pushed by `expansion` and shaken by up to `randomness`, each by one
/// minus its (smoothed) weight.
pub fn shape_points(line: &CultureLine, expansion: f32, randomness: f32) -> Vec<ControlPoint> {
    let pts = &line.line.points;
    let n = pts.len();
    let closed = line.line.closed;
    if n < 2 {
        return pts.clone();
    }
    let pos: Vec<Vec2> = pts.iter().map(|p| p.position).collect();
    // Smooth.
    let mut ws = vec![0.0f32; n];
    let mut smoothed = vec![Vec2::ZERO; n];
    for i in 0..n {
        let mut s = Vec2::ZERO;
        let mut w = 0.0;
        for (k, &c) in SMOOTH_KERNEL.iter().enumerate() {
            let j = at(i as isize + k as isize - 2, n, closed);
            s += pos[j] * c;
            w += line.weights[j] * c;
        }
        ws[i] = w;
        smoothed[i] = pos[i] * w + s * (1.0 - w);
    }
    // Expand.
    let expanded: Vec<Vec2> = (0..n)
        .map(|i| {
            let nrm = inward(&smoothed, i, closed, line.line.inward_left);
            smoothed[i] + nrm * expansion * (1.0 - ws[i])
        })
        .collect();
    // Randomize: the offset is a function of the point's place.
    (0..n)
        .map(|i| {
            let nrm = inward(&expanded, i, closed, line.line.inward_left);
            let off = 2.0 * randomness * frandom_first(seed_of(expanded[i])) - randomness;
            ControlPoint {
                position: expanded[i] + nrm * off * (1.0 - ws[i]),
                ..pts[i]
            }
        })
        .collect()
}

/// The uniform cubic B-spline of the points (it rounds the corners and does not pass through the
/// points), [`SAMPLES_PER_SPAN`] samples per span; an open line is held at its ends.
pub fn bspline(points: &[ControlPoint], closed: bool) -> Samples {
    let n = points.len();
    let mut out = Samples {
        closed,
        ..Samples::default()
    };
    if n == 0 {
        return out;
    }
    let p = |i: isize| points[at(i, n, closed)].position;
    let spans = if closed { n } else { n.saturating_sub(1) };
    for i in 0..spans as isize {
        let (p0, p1, p2, p3) = (p(i - 1), p(i), p(i + 1), p(i + 2));
        for s in 0..SAMPLES_PER_SPAN {
            let t = s as f32 / SAMPLES_PER_SPAN as f32;
            let (t2, t3) = (t * t, t * t * t);
            let b0 = (1.0 - t) * (1.0 - t) * (1.0 - t) / 6.0;
            let b1 = (3.0 * t3 - 6.0 * t2 + 4.0) / 6.0;
            let b2 = (-3.0 * t3 + 3.0 * t2 + 3.0 * t + 1.0) / 6.0;
            let b3 = t3 / 6.0;
            out.points.push(p0 * b0 + p1 * b1 + p2 * b2 + p3 * b3);
            out.cells.push(points[i as usize].cell);
        }
    }
    if !closed {
        let last = p(n as isize - 1);
        out.points
            .push((p(n as isize - 2) + 4.0 * last + last) / 6.0);
        out.cells.push(points[n - 1].cell);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hexgrid::{CORNER_RADIUS, cell_center};

    fn input(w: u32, h: u32, f: impl Fn(i32, i32) -> i8) -> OwnersInput {
        let owners = (0..h as i32)
            .flat_map(|y| (0..w as i32).map(move |x| (x, y)))
            .map(|(x, y)| f(x, y))
            .collect();
        OwnersInput::new(w, h, owners, vec![[200, 0, 0], [0, 0, 200]]).unwrap()
    }

    fn shape(w: u32, h: u32) -> GridShape {
        GridShape {
            width: w as i32,
            height: h as i32,
            wrap_x: false,
        }
    }

    #[test]
    fn an_edge_against_nobody_has_two_points_on_it_and_one_between_owners_three() {
        // Cell (3, 3) of player 0, its east neighbour of player 1, the rest nobody's.
        let inp = input(7, 7, |x, y| match (x, y) {
            (3, 3) => 0,
            (4, 3) => 1,
            _ => -1,
        });
        let lines = culture_lines(&inp, &shape(7, 7), 0, 0.105);
        assert_eq!(lines.len(), 1);
        let l = &lines[0];
        // Five edges of two points, one of three.
        assert_eq!(l.line.points.len(), 5 * 2 + 3);
        assert_eq!(l.weights.iter().filter(|&&w| w == 1.0).count(), 3);
        let c = cell_center(3, 3);
        for p in &l.line.points {
            // On the hex: between the apothem and the corner radius from the centre.
            let d = (p.position - c).length();
            assert!(d > 31.9 && d <= CORNER_RADIUS, "{d}");
        }
    }

    #[test]
    fn the_points_against_nobody_go_in_by_the_expansion_and_shake_and_the_others_stay() {
        let inp = input(7, 7, |x, y| match (x, y) {
            (3, 3) => 0,
            (4, 3) => 1,
            _ => -1,
        });
        let l = &culture_lines(&inp, &shape(7, 7), 0, 0.105)[0];
        let c = cell_center(3, 3);
        let still = shape_points(l, 2.0, 0.0);
        let shaken = shape_points(l, 2.0, 6.0);
        assert_eq!(shape_points(l, 2.0, 6.0), shaken, "a function of the place");
        let mut moved = 0;
        for (i, w) in l.weights.iter().enumerate() {
            let before = (l.line.points[i].position - c).length();
            let after = (still[i].position - c).length();
            let d = still[i].position.distance(shaken[i].position);
            assert!(d <= 6.0 + 1e-3);
            if *w == 0.0 {
                // Smoothed and pushed in: nearer the centre.
                assert!(after < before, "{after} vs {before}");
                if d > 0.5 {
                    moved += 1;
                }
            }
        }
        assert!(moved > 2, "the shake moves the points against nobody");
    }

    #[test]
    fn the_first_draw_of_frandom_is_the_lcg_over_the_modulus() {
        let s = 12345u32.wrapping_mul(0x343fd).wrapping_add(0x269ec3) % 0xffff_cfef;
        assert!((frandom_first(12345) - s as f32 / 4_294_954_991.0).abs() < 1e-6);
        assert!((0.0..1.0).contains(&frandom_first(0)));
    }

    #[test]
    fn the_bspline_rounds_the_corners_in_four_samples_a_span() {
        let pts: Vec<ControlPoint> = [(0.0, 0.0), (30.0, 0.0), (30.0, 30.0), (0.0, 30.0)]
            .iter()
            .enumerate()
            .map(|(i, &(x, y))| ControlPoint {
                position: Vec2::new(x, y),
                cell: i as u32,
                key: i as u32,
            })
            .collect();
        let s = bspline(&pts, true);
        assert_eq!(s.points.len(), 16);
        // It does not pass through a corner: the first sample is (p3 + 4 p0 + p1) / 6.
        assert!((s.points[0] - Vec2::new(5.0, 5.0)).length() < 1e-4);
        for p in &pts {
            assert!(s.points.iter().all(|q| q.distance(p.position) > 1.0));
        }
    }
}
