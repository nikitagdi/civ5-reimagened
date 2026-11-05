//! The splines of the blend: the body of `TerrainSystem::Init` between
//! the coast stencil and the blit of the pieces (`TerrainSystem.cpp:756-921`). The waterline (level
//! 2) of every coast is kept as it is, and from it two lines are made, one unit to each side: the
//! midpoint of every segment moved along its normal; the loops that the shift makes on a sharp
//! turn are cut. The pieces blit along these lines.

use glam::Vec2;

use super::bspline;
use super::levels::{COASTLINE_LEVEL, FinalLevels};
use crate::world::terrain::build::{BlendSpline, SplineRange};

/// Distance of the points of the shifted lines from the coast.
// re: TerrainSystem.cpp:903, :917 (`PushOffsetPoints(..., -1.0f)`, `(..., 1.0f)`)
pub const OFFSET: f32 = 1.0;

/// A corner whose points are closer to the other segment than this is dropped.
// re: TerrainSystem.cpp:56 (kMinCornerDistance)
const MIN_CORNER_DISTANCE: f64 = 0.01;
/// Segments after a segment that are searched for a crossing with it.
// re: TerrainSystem.cpp:57 (kLoopSearchWindow)
const LOOP_SEARCH_WINDOW: usize = 50;
/// Slack of the crossing test, and the smallest determinant of two lines that cross.
// re: TerrainSystem.cpp:58 (kIntersectEpsilon)
const INTERSECT_EPSILON: f32 = 1.0e-4;

/// A run of the points of [`BlendSplineSet`] and the kind of line it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub begin: usize,
    pub end: usize,
    /// The type of the coastline (0 or 1) for the coast itself and the first shift, 1 or 2 for
    /// the second shift: the mark the blit puts on the ranges.
    // re: TerrainSystem.cpp:875-914 (`kRange.field_8`)
    pub kind: u8,
}

impl Range {
    pub fn len(&self) -> usize {
        self.end - self.begin
    }

    pub fn is_empty(&self) -> bool {
        self.begin == self.end
    }
}

/// The points and ranges the blit gets: every coast, then the two shifted lines of every coast.
// re: cvTerrainBlend::BlendTerrainHeightmap(kGrid, size, aPoints, aCoastRanges, aOffsetRanges, ...)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlendSplineSet {
    pub points: Vec<Vec2>,
    pub coast_ranges: Vec<Range>,
    pub offset_ranges: Vec<Range>,
}

impl BlendSplineSet {
    /// The points of a range.
    pub fn slice(&self, r: &Range) -> &[Vec2] {
        &self.points[r.begin..r.end]
    }

    /// The lines as the products of the build hand them out: the coasts (range A), then the shifted
    /// lines (range B), in the order of the ranges.
    pub fn splines(&self) -> Vec<BlendSpline> {
        let line = |range, r: &Range| BlendSpline {
            range,
            class: r.kind,
            points: self.slice(r).to_vec(),
        };
        self.coast_ranges
            .iter()
            .map(|r| line(SplineRange::A, r))
            .chain(self.offset_ranges.iter().map(|r| line(SplineRange::B, r)))
            .collect()
    }
}

/// Distance from `p` to the segment `[a, b]`. The distance to the line is truncated to a whole
/// number, as the original does with an integer `abs`.
// re: TerrainSystem.cpp:762-778 (`SegmentDistance`)
fn segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let bp = p - b;
    if (a.x == b.x && a.y == b.y) || ab.y * bp.y + ab.x * bp.x >= 0.0 {
        return (bp.y * bp.y + bp.x * bp.x).sqrt();
    }
    let ap = p - a;
    if ab.y * ap.y + ab.x * ap.x <= 0.0 {
        return (ap.y * ap.y + ap.x * ap.x).sqrt();
    }
    let len = (ab.y * ab.y + ab.x * ab.x).sqrt();
    ((ap.y * ab.x - ap.x * ab.y) / len).trunc().abs()
}

/// Appends one point per segment of `spline`: the midpoint moved one unit along `side` times the
/// normal of the gradient.
// re: TerrainSystem.cpp:781-794 (`PushOffsetPoints`)
fn push_offset_points(out: &mut Vec<Vec2>, spline: &[Vec2], side: f32) {
    for i in 0..spline.len() - 1 {
        let g = bspline::gradient(spline, i as f32);
        let len = (g.y * g.y + g.x * g.x).sqrt();
        // Two equal points have no normal; the midpoint stays where it is.
        let (nx, ny) = if len > 0.0 {
            (g.y / len, -(g.x / len))
        } else {
            (0.0, 0.0)
        };
        out.push(Vec2::new(
            (spline[i].x + spline[i + 1].x) * 0.5 + side * nx,
            (spline[i].y + spline[i + 1].y) * 0.5 + side * ny,
        ));
    }
}

/// A segment of `[begin, end)` that crosses one of the next [`LOOP_SEARCH_WINDOW`] segments pulls
/// every point in between onto the crossing; a near-degenerate corner drops its middle point.
// re: TerrainSystem.cpp:798-856 (`RemoveSplineLoops`)
pub fn remove_loops(p: &mut [Vec2], begin: usize, end: usize) {
    let mut i = begin;
    while i + 2 < end {
        let (a, b, c) = (p[i], p[i + 1], p[i + 2]);
        let dist_c = segment_distance(c, a, b);
        let dist_a = segment_distance(a, b, c);
        let min_dist = if dist_a <= dist_c { dist_a } else { dist_c };
        if f64::from(min_dist) < MIN_CORNER_DISTANCE {
            p[i + 1] = p[i + 2];
            i += 1;
            continue;
        }

        // Line AB as a1 * x + b1 * y = c1.
        let a1 = b.y - a.y;
        let b1 = a.x - b.x;
        let c1 = a.y * b1 + a.x * a1;
        let max_abx = if a.x <= b.x { b.x } else { a.x };
        let min_abx = if b.x <= a.x { b.x } else { a.x };
        let max_aby = if a.y <= b.y { b.y } else { a.y };
        let min_aby = if b.y <= a.y { b.y } else { a.y };
        let last = (end - 1).min(i + LOOP_SEARCH_WINDOW);
        for j in i + 2..last {
            let (q0, q1) = (p[j], p[j + 1]);
            let a2 = q1.y - q0.y;
            let b2 = q0.x - q1.x;
            let det = a1 * b2 - b1 * a2;
            if ((det as i32).wrapping_abs() as f32) < INTERSECT_EPSILON {
                continue;
            }
            let c2 = q0.y * b2 + q0.x * a2;
            let inv_det = 1.0 / det;
            let max_pqx = if q0.x <= q1.x { q1.x } else { q0.x };
            let min_pqx = if q1.x <= q0.x { q1.x } else { q0.x };
            let max_pqy = if q0.y <= q1.y { q1.y } else { q0.y };
            let min_pqy = if q1.y <= q0.y { q1.y } else { q0.y };
            let cross_x = (b2 * c1 - b1 * c2) * inv_det;
            if cross_x > (if max_pqx <= max_abx { max_pqx } else { max_abx }) + INTERSECT_EPSILON {
                continue;
            }
            if (if min_abx <= min_pqx { min_pqx } else { min_abx }) - INTERSECT_EPSILON > cross_x {
                continue;
            }
            let cross_y = (c2 * a1 - a2 * c1) * inv_det;
            if cross_y > (if max_pqy <= max_aby { max_pqy } else { max_aby }) + INTERSECT_EPSILON {
                continue;
            }
            if (if min_aby <= min_pqy { min_pqy } else { min_aby }) - INTERSECT_EPSILON > cross_y {
                continue;
            }
            let cross = Vec2::new(cross_x, cross_y);
            for k in (i + 1..=j).rev() {
                p[k] = cross;
            }
            break;
        }
        i += 1;
    }
}

/// The blend splines of the levels of all coasts: the body of `TerrainSystem::Init`.
// re: TerrainSystem::Init 0x0868046a (TerrainSystem.cpp:858-921)
pub fn compute(levels: &[FinalLevels]) -> BlendSplineSet {
    let mut points: Vec<Vec2> = Vec::new();
    let mut coast_ranges = Vec::with_capacity(levels.len());
    let mut offset_ranges = Vec::with_capacity(levels.len() * 2);
    let mut cursor = 0;
    for coast in levels {
        let line = coast.level(COASTLINE_LEVEL);
        let kind = match coast.line_type {
            1 => 1,
            _ => 0,
        };
        let range = Range {
            begin: cursor,
            end: cursor + line.len(),
            kind,
        };
        points.extend(line.iter().map(|c| c.pos));
        coast_ranges.push(range);
        cursor = range.end;
    }

    for coast in &coast_ranges {
        let spline: Vec<Vec2> = points[coast.begin..coast.end].to_vec();
        let segments = spline.len() - 1;
        let mut range = Range {
            begin: cursor,
            end: cursor + segments,
            kind: coast.kind,
        };
        push_offset_points(&mut points, &spline, -OFFSET);
        remove_loops(&mut points, range.begin, range.end);
        offset_ranges.push(range);

        range = Range {
            begin: range.end,
            end: range.end + segments,
            kind: if coast.kind == 1 { 2 } else { 1 },
        };
        push_offset_points(&mut points, &spline, OFFSET);
        remove_loops(&mut points, range.begin, range.end);
        offset_ranges.push(range);
        cursor = range.end;
    }
    BlendSplineSet {
        points,
        coast_ranges,
        offset_ranges,
    }
}

#[cfg(test)]
mod tests {
    use super::super::levels::tests::island_levels;
    use super::super::lines::CoastCP;
    use super::*;

    fn v(x: f32, y: f32) -> Vec2 {
        Vec2::new(x, y)
    }

    fn straight_coast(kind: i32, n: usize) -> FinalLevels {
        // Five identical levels of a straight line along x with 10 units between the points.
        let pts: Vec<CoastCP> = (0..n)
            .map(|i| CoastCP {
                pos: v(10.0 * i as f32, 0.0),
                ..CoastCP::default()
            })
            .collect();
        let mut points = Vec::new();
        let mut ranges = [[0; 2]; 5];
        for r in &mut ranges {
            *r = [points.len(), points.len() + n];
            points.extend_from_slice(&pts);
        }
        FinalLevels {
            ranges,
            points,
            line_type: kind,
        }
    }

    #[test]
    fn the_shifts_of_a_straight_line_lie_one_unit_to_each_side() {
        let set = compute(&[straight_coast(0, 5)]);
        assert_eq!(set.coast_ranges.len(), 1);
        assert_eq!(set.offset_ranges.len(), 2);
        let (minus, plus) = (&set.offset_ranges[0], &set.offset_ranges[1]);
        // One point per segment, at the midpoint of the segment, moved along (gy, -gx) / len =
        // (0, -1) times the side: the shift of -1 is above the line, the shift of +1 below it.
        let want = |y: f32| -> Vec<Vec2> { (0..4).map(|i| v(5.0 + 10.0 * i as f32, y)).collect() };
        assert_eq!(set.slice(minus), want(1.0).as_slice());
        assert_eq!(set.slice(plus), want(-1.0).as_slice());
        // The marks: type 0 gives 0 and 1, type 1 gives 1 and 2.
        assert_eq!((minus.kind, plus.kind), (0, 1));
        let set1 = compute(&[straight_coast(1, 5)]);
        assert_eq!(
            (
                set1.coast_ranges[0].kind,
                set1.offset_ranges[0].kind,
                set1.offset_ranges[1].kind
            ),
            (1, 1, 2)
        );
    }

    #[test]
    fn the_ranges_follow_each_other_in_one_list_of_points() {
        let set = compute(&[straight_coast(0, 5), straight_coast(1, 7)]);
        assert_eq!(
            set.coast_ranges
                .iter()
                .map(|r| (r.begin, r.end))
                .collect::<Vec<_>>(),
            [(0, 5), (5, 12)]
        );
        // The shifts start where the coasts end: 4 and 4 for the first, 6 and 6 for the second.
        assert_eq!(
            set.offset_ranges
                .iter()
                .map(|r| (r.begin, r.end))
                .collect::<Vec<_>>(),
            [(12, 16), (16, 20), (20, 26), (26, 32)]
        );
        assert_eq!(set.points.len(), 32);
        let splines = set.splines();
        assert_eq!(splines.len(), 2 + 4);
        assert!(splines[..2].iter().all(|s| s.range == SplineRange::A));
        assert!(splines[2..].iter().all(|s| s.range == SplineRange::B));
        assert_eq!(splines[0].points.len(), 5);
    }

    #[test]
    fn a_loop_is_pulled_onto_its_crossing_by_the_hand_calculation_of_the_original() {
        // A = (0,0), B = (10,0), C = (10,-3), D = (6,-5), E = (6,5). The segment DE crosses AB
        // at (6, 0): i = 0 finds it at j = 3 and sets the points 3, 2, 1 to the crossing. i = 1
        // sees three equal points and drops the middle one (point 2 becomes point 3); i = 2 sees
        // (6,0), (6,0), (6,5): the distance of the first to the segment of the other two is 0, so
        // point 3 becomes point 4.
        let mut p = vec![
            v(0.0, 0.0),
            v(10.0, 0.0),
            v(10.0, -3.0),
            v(6.0, -5.0),
            v(6.0, 5.0),
        ];
        remove_loops(&mut p, 0, 5);
        assert_eq!(
            p,
            [
                v(0.0, 0.0),
                v(6.0, 0.0),
                v(6.0, 0.0),
                v(6.0, 5.0),
                v(6.0, 5.0)
            ]
        );
    }

    #[test]
    fn the_segment_distance_takes_the_ends_and_truncates_the_perpendicular() {
        let (a, b) = (v(0.0, 0.0), v(10.0, 0.0));
        // Beyond B: the distance to B. Beyond A: to A.
        assert_eq!(segment_distance(v(13.0, 4.0), a, b), 5.0);
        assert_eq!(segment_distance(v(-3.0, 4.0), a, b), 5.0);
        // Over the segment: the perpendicular, cut to a whole number (3.7 -> 3).
        assert_eq!(segment_distance(v(5.0, 3.7), a, b), 3.0);
        // A point segment is a point.
        assert_eq!(segment_distance(v(3.0, 4.0), a, a), 5.0);
    }

    #[test]
    fn a_line_with_no_crossing_is_left_alone() {
        let mut p: Vec<Vec2> = (0..20).map(|i| v(i as f32 * 4.0, (i % 2) as f32)).collect();
        let before = p.clone();
        remove_loops(&mut p, 0, 20);
        assert_eq!(p, before);
    }

    #[test]
    fn the_shifts_of_a_closed_island_line_have_no_loops_and_stay_close_to_the_coast() {
        let (_, levels, _) = island_levels(false);
        let set = compute(&levels);
        assert_eq!(set.coast_ranges.len(), 2);
        assert_eq!(set.offset_ranges.len(), 4);
        for (c, pair) in set.coast_ranges.iter().zip(set.offset_ranges.chunks(2)) {
            let coast = set.slice(c);
            for r in pair {
                let shifted = set.slice(r);
                assert_eq!(shifted.len(), c.len() - 1);
                // Every shifted point lies within a unit and a half of the segment it came from.
                for (i, q) in shifted.iter().enumerate() {
                    let mid = (coast[i] + coast[i + 1]) * 0.5;
                    assert!((*q - mid).length() < 3.0, "point {i}: {q} vs {mid}");
                }
            }
            // The two shifts are on opposite sides of the coast.
            let side = |r: &Range| {
                let shifted = set.slice(r);
                let i = shifted.len() / 2;
                let g = coast[i + 1] - coast[i];
                let mid = (coast[i] + coast[i + 1]) * 0.5;
                g.perp_dot(shifted[i] - mid).signum()
            };
            assert_ne!(side(&pair[0]), side(&pair[1]));
        }
    }
}
