//! The round wall of a city (world-objects 4.4, WOB5): the convex hull of the inhabited land
//! samples of the density field, pushed out along its normals, cut into spline chunks with towers
//! every 12 units.
//!
//! re: `GenerateWallPolygonFromCity` (0x087c464a), `GenerateWallDataFromPolygon` (0x087c442e),
//! `GenerateWallBetweenPoints` (0x087c3dc8), `UpdateCityWall` (0x087b6f9e); the uniform quadratic
//! B-spline of `B_SPLINE::ReconstructScalingFunc` (0x087bb5c1, Chaikin 0.75/0.25). The bodies are
//! those of civ5-re `cvCityBuildingSim.cpp` (RW8); the generator sends no `AddWallGate`.
//!
//! Positions are in the original's world plane (x east, y up the rows).
//!
//! SIMPLIFIED(SIMP-021): the 8x8 jitter table of the sample lattice (filled by 0x087b997a) is not
//! read (no jitter), nor `POLY_ComputeBoundingHull` (ours: the monotone chain, turned clockwise so
//! that the offset of the original pushes out) and `CalculateApproxSplineLength` (ours: 16 chords a span).

use glam::Vec2;

use super::fields::{CLASS_NONE, DensityField, ROW_SCALE, tex_to_field};

/// Samples farther than this from the middle (tex units) are out.
pub const SAMPLE_RADIUS: f32 = 40.0;
/// A hull smaller than this gets the corners of the hex.
pub const MIN_HULL_AREA: f32 = 200.0;
/// The push of each vertex along its normal.
pub const NORMAL_OFFSET: f32 = 2.0;
/// One tower every this much of the spline.
pub const TOWER_SPACING: f32 = 12.0;
/// The control points of one `AddWallSegment`.
pub const SEGMENT_POINTS: usize = 6;
/// `SetWallScale`.
pub const WALL_SCALE: f32 = 0.05;
/// The hex corners added to a small hull are this far out (0x0914e098).
const CORNER_SHARE: f32 = 0.6;

/// What the wall needs of the ground around a city.
pub struct Ground<'a> {
    /// Dry land in the height band of the houses (and not water on the stencil).
    pub land: &'a dyn Fn(Vec2) -> bool,
    /// The up component of the terrain normal.
    pub normal_z: &'a dyn Fn(Vec2) -> f32,
}

/// The class test of a sample by population (`pop < 4`: any house; `4..5`: not the lowest class;
/// `6..`: neither of the two lowest).
pub fn sample_is_city(class: u8, population: i32) -> bool {
    if population <= 3 {
        class != CLASS_NONE
    } else if population <= 5 {
        class | 4 != 4
    } else {
        !(class < 5 && (0x13u32 >> class) & 1 != 0)
    }
}

/// The inhabited land samples of the 64x64 lattice (step 2 tex units) around the city.
pub fn samples(
    centre: Vec2,
    field: &DensityField,
    reflection: usize,
    population: i32,
    ground: &Ground<'_>,
) -> Vec<Vec2> {
    let mut out = Vec::new();
    for r in 0..64 {
        for c in 0..64 {
            let rel = Vec2::new(2.0 * c as f32 - 64.0, 2.0 * r as f32 - 64.0);
            let Some(class) = field.class_at(tex_to_field(rel, reflection)) else {
                continue;
            };
            if !sample_is_city(class, population) {
                continue;
            }
            let p = centre + Vec2::new(rel.x, rel.y * ROW_SCALE);
            if !(ground.land)(p) {
                continue;
            }
            if (rel.x * rel.x + rel.y * rel.y).sqrt() > SAMPLE_RADIUS {
                continue;
            }
            if (ground.normal_z)(p) < 0.85 {
                continue;
            }
            out.push(p);
        }
    }
    out
}

/// The convex hull, counter-clockwise, from the lowest-leftmost point.
pub fn hull(points: &[Vec2]) -> Vec<Vec2> {
    let mut p: Vec<Vec2> = points.to_vec();
    p.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    p.dedup();
    if p.len() < 3 {
        return p;
    }
    let cross = |o: Vec2, a: Vec2, b: Vec2| (a - o).perp_dot(b - o);
    let mut lower: Vec<Vec2> = Vec::new();
    for &q in &p {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], q) <= 0.0 {
            lower.pop();
        }
        lower.push(q);
    }
    let mut upper: Vec<Vec2> = Vec::new();
    for &q in p.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], q) <= 0.0 {
            upper.pop();
        }
        upper.push(q);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// `POLY_SignedArea`.
pub fn signed_area(poly: &[Vec2]) -> f32 {
    let n = poly.len();
    (0..n)
        .map(|i| poly[i].perp_dot(poly[(i + 1) % n]))
        .sum::<f32>()
        * 0.5
}

/// The six corners of the city hex at 0.6 of the way out, in the world plane around `centre`.
pub fn hex_corners(centre: Vec2) -> [Vec2; 6] {
    const CUBE: [[f32; 2]; 6] = [
        [2.0 / 3.0, -1.0 / 3.0],
        [1.0 / 3.0, 1.0 / 3.0],
        [-1.0 / 3.0, 2.0 / 3.0],
        [-2.0 / 3.0, 1.0 / 3.0],
        [-1.0 / 3.0, -1.0 / 3.0],
        [1.0 / 3.0, -2.0 / 3.0],
    ];
    CUBE.map(|[hx, hy]| {
        let (hx, hy) = (hx * CORNER_SHARE, hy * CORNER_SHARE);
        centre + Vec2::new(64.0 * hx + 32.0 * hy, 64.0 * hy * ROW_SCALE)
    })
}

/// The point of the uniform quadratic B-spline of `p` at `t` (0..len-2).
pub fn eval(p: &[Vec2], t: f32) -> Vec2 {
    let (j, u) = span(p.len(), t);
    0.5 * (1.0 - u) * (1.0 - u) * p[j] + (0.5 + u - u * u) * p[j + 1] + 0.5 * u * u * p[j + 2]
}

/// The derivative of [`eval`].
pub fn gradient(p: &[Vec2], t: f32) -> Vec2 {
    let (j, u) = span(p.len(), t);
    (u - 1.0) * p[j] + (1.0 - 2.0 * u) * p[j + 1] + u * p[j + 2]
}

fn span(n: usize, t: f32) -> (usize, f32) {
    let last = n.saturating_sub(3);
    let j = (t.max(0.0).floor() as usize).min(last);
    (j, t - j as f32)
}

/// The length of the spline of `p`.
pub fn spline_length(p: &[Vec2]) -> f32 {
    if p.len() < 3 {
        return 0.0;
    }
    let steps = (p.len() - 2) * 16;
    let end = (p.len() - 2) as f32;
    (1..=steps)
        .map(|k| {
            let a = eval(p, end * (k - 1) as f32 / steps as f32);
            let b = eval(p, end * k as f32 / steps as f32);
            a.distance(b)
        })
        .sum()
}

/// The polygon of the wall of a city, or `None` (fewer than 3 vertices: no wall).
pub fn polygon(mut samples: Vec<Vec2>, centre: Vec2) -> Option<Vec<Vec2>> {
    let mut poly = hull(&samples);
    if signed_area(&poly).abs() < MIN_HULL_AREA {
        samples.extend(hex_corners(centre));
        poly = hull(&samples);
    }
    let n = poly.len();
    if n < 3 {
        return None;
    }
    // Clockwise, so that the left normal of the original's offset points out of the city.
    poly.reverse();
    // The gradient at the middle of the span centred on each vertex of the closed ring.
    let mut ext = Vec::with_capacity(n + 2);
    ext.push(poly[n - 1]);
    ext.extend_from_slice(&poly);
    ext.push(poly[0]);
    let offsets: Vec<Vec2> = (0..n)
        .map(|i| {
            let g = gradient(&ext, 0.5 + i as f32);
            let g = g / g.length();
            Vec2::new(g.y * -NORMAL_OFFSET, g.x + g.x)
        })
        .collect();
    Some(poly.iter().zip(offsets).map(|(v, o)| *v + o).collect())
}

/// The wall line the houses are kept off: the closed polygon refined three times.
pub fn mask_line(poly: &[Vec2]) -> Vec<Vec2> {
    let mut pts: Vec<Vec2> = poly.to_vec();
    if pts.len() >= 2 {
        pts.push(poly[0]);
        pts.push(poly[1]);
    }
    for _ in 0..3 {
        pts = pts
            .windows(2)
            .flat_map(|w| [0.75 * w[0] + 0.25 * w[1], 0.25 * w[0] + 0.75 * w[1]])
            .collect();
    }
    pts
}

/// A tower: the place and the heading (radians, the angle of the tangent in the world plane).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tower {
    pub at: Vec2,
    pub heading: f32,
}

/// The wall of one polygon: the control points of each segment and the towers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WallPlan {
    pub polygon: Vec<Vec2>,
    pub segments: Vec<Vec<Vec2>>,
    pub towers: Vec<Tower>,
}

/// `GenerateWallBetweenPoints(start, end, total)`.
fn between(pts: &[Vec2], start: usize, end: usize, plan: &mut WallPlan) {
    let total = pts.len();
    let n = end + if end < start { total } else { 0 } - start;
    if n < 4 {
        return;
    }
    let curve: Vec<Vec2> = (0..n).map(|k| pts[(start + k) % total]).collect();
    // The chunks: at most six points, the next repeats the last two, the last keeps four.
    let mut seg: Vec<Vec2> = Vec::new();
    let mut rem = n;
    for &p in &curve {
        let k = seg.len();
        if k == SEGMENT_POINTS || (k >= 4 && (3..=6).contains(&rem)) {
            plan.segments.push(seg.clone());
            seg = vec![seg[k - 2], seg[k - 1]];
        }
        seg.push(p);
        rem -= 1;
    }
    if seg.len() >= 4 {
        plan.segments.push(seg);
    }
    // The towers.
    let length = spline_length(&curve);
    let mut arc = vec![0.0f32; n - 1];
    for i in 1..n - 2 {
        let a = 0.5 * (curve[i] + curve[i + 1]);
        let b = 0.5 * (curve[i - 1] + curve[i]);
        arc[i] = arc[i - 1] + a.distance(b);
    }
    arc[n - 2] = length;
    let count = (length / TOWER_SPACING).floor() as usize;
    if count == 0 {
        return;
    }
    let spacing = length / count as f32;
    let mut s = 0.0;
    for k in 0..count {
        let t = match (0..n - 1).find(|&j| arc[j] > s) {
            Some(j) if j > 0 => {
                let w = (1.0 - (arc[j] - s) / (arc[j] - arc[j - 1])).clamp(0.0, 1.0);
                (j - 1) as f32 + w
            }
            Some(_) => 0.0,
            None => (n - 2) as f32,
        }
        .clamp(0.0, (n - 2) as f32);
        if k > 0 {
            let g = gradient(&curve, t);
            plan.towers.push(Tower {
                at: eval(&curve, t),
                heading: g.y.atan2(g.x),
            });
        }
        s += spacing;
    }
}

/// `GenerateWallDataFromPolygon`: the ring, then the seam.
pub fn plan(polygon: Vec<Vec2>) -> WallPlan {
    let n = polygon.len();
    let mut plan = WallPlan::default();
    if n < 3 {
        return plan;
    }
    between(&polygon, 0, n, &mut plan);
    between(&polygon, n - 2, 2, &mut plan);
    plan.polygon = polygon;
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hull_is_counter_clockwise_and_convex() {
        let pts = [
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 10.0),
            Vec2::new(0.0, 10.0),
            Vec2::new(5.0, 5.0),
        ];
        let h = hull(&pts);
        assert_eq!(h.len(), 4);
        assert!(signed_area(&h) > 0.0);
        assert_eq!(signed_area(&h), 100.0);
    }

    #[test]
    fn the_polygon_is_pushed_out_by_two() {
        let ring: Vec<Vec2> = (0..12)
            .map(|k| {
                let a = k as f32 / 12.0 * std::f32::consts::TAU;
                Vec2::new(a.cos(), a.sin()) * 30.0
            })
            .collect();
        let p = polygon(ring, Vec2::ZERO).unwrap();
        for v in &p {
            assert!((v.length() - 32.0).abs() < 0.05, "{}", v.length());
        }
    }

    #[test]
    fn a_tiny_hull_takes_the_hex_corners() {
        let p = polygon(vec![Vec2::new(1.0, 1.0)], Vec2::ZERO).unwrap();
        assert_eq!(p.len(), 6);
    }

    #[test]
    fn the_ring_has_chunks_of_six_and_a_tower_every_twelve_units() {
        let ring: Vec<Vec2> = (0..20)
            .map(|k| {
                let a = k as f32 / 20.0 * std::f32::consts::TAU;
                Vec2::new(a.cos(), a.sin()) * 30.0
            })
            .collect();
        let w = plan(ring);
        assert!(w.segments.iter().all(|s| (4..=6).contains(&s.len())));
        // The seam is one segment of four.
        assert_eq!(w.segments.last().unwrap().len(), 4);
        // About 2*pi*30 / 12 towers, two starts without one.
        let n = w.towers.len();
        assert!((12..=16).contains(&n), "{n}");
        for t in &w.towers {
            assert!((t.at.length() - 30.0).abs() < 1.5);
            // The heading runs along the ring.
            let along = Vec2::new(t.heading.cos(), t.heading.sin());
            assert!(along.dot(t.at.normalize()).abs() < 0.2);
        }
    }

    #[test]
    fn the_class_test_tightens_with_population() {
        assert!(sample_is_city(0, 3));
        assert!(!sample_is_city(0, 4));
        assert!(sample_is_city(1, 5));
        assert!(!sample_is_city(1, 6));
        assert!(sample_is_city(2, 9));
        assert!(!sample_is_city(4, 1));
    }
}
