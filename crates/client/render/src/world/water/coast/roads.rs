//! The coastal roads: `GenerateAllCoastalRoads`. A road follows every
//! coastline that has the land on its left (type 0): the noisy line is cut twice into `4n - 6`
//! points, sampled evenly along the curve, and each sample is moved five units to the land side.
//! The roads are polylines for the road layout of `world-objects`; this file does not call it
//! (`AddLargeRoad` and `Fun_0868a43e` of `TerrainSystem::Init`).

use glam::Vec2;

use super::bspline;
use super::lines::CoastlineSpline;
use crate::world::terrain::sampler::TerrainFrame;

/// Distance of the road from the coastline, toward the land.
// re: GenerateAllCoastalRoads 0x088e7306 (DAT_0912ca10 = 5.0)
pub const ROAD_OFFSET: f32 = 5.0;

/// The ratio of the rows of hexes to their width the road layout uses.
// re: TerrainSystem.cpp:53 (kHexHeightRatio)
pub const HEX_HEIGHT_RATIO: f32 = 0.866;

/// A road: a run of points of the list.
// re: class CoastalRoad (cvCoastMeshEngine.h:18)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoastalRoad {
    pub first_point: usize,
    pub num_points: usize,
    /// True for a road around an island or a lake (`field_8`, the flag `m_bClosed` of the line).
    pub closed: bool,
}

/// `GenerateAllCoastalRoads`: the points of all roads and the roads, in the texel plane.
// re: cvCoastMeshEngine::GenerateAllCoastalRoads 0x088e7306
pub fn generate(splines: &[CoastlineSpline]) -> (Vec<Vec2>, Vec<CoastalRoad>) {
    let mut points = Vec::new();
    let mut roads = Vec::new();
    for spline in splines.iter().filter(|s| s.line_type == 0) {
        let base: Vec<Vec2> = spline.points.iter().map(|p| p.pos).collect();
        let n = base.len();
        let m = n * 4 - 6;
        // Two cuts: n -> 2n-2 -> 4n-6.
        let fine = bspline::reconstruct(&bspline::reconstruct(&base));
        debug_assert_eq!(fine.len(), m);
        roads.push(CoastalRoad {
            first_point: points.len(),
            num_points: m,
            closed: spline.closed,
        });
        // The samples are evenly spaced in the parameter, from 0 to m - 2 in m steps.
        let last = (m - 2) as f32;
        let step = last / (m - 1) as f32;
        let mut t = 0.0f32;
        for _ in 0..m {
            let g = bspline::gradient(&fine, t);
            let v = bspline::value(&fine, t);
            let len = (g.y * g.y + g.x * g.x).sqrt();
            points.push(Vec2::new(
                v.x - (g.y / len) * ROAD_OFFSET,
                (g.x / len) * ROAD_OFFSET + v.y,
            ));
            t = (t + step).min(last).max(0.0);
        }
    }
    (points, roads)
}

/// The point of the texel plane in the plane of the road layout of the original:
/// `x - offset_x`, `y * 0.866 - offset_y`, with the offsets of the sampler (half the world).
// re: TerrainSystem::Init (TerrainSystem.cpp:734-738)
pub fn to_layout(p: Vec2, frame: &TerrainFrame) -> Vec2 {
    let half = frame.half_world();
    Vec2::new(p.x - half.x, p.y * HEX_HEIGHT_RATIO - half.y)
}

#[cfg(test)]
mod tests {
    use super::super::grid::CoastGrid;
    use super::super::lines::{CoastLines, CoastlineSpline};
    use super::super::testkit::{frame_of, island};
    use super::*;

    fn lines() -> Vec<CoastlineSpline> {
        let b = island(20, 20, false, 2);
        let mut g = CoastGrid::new(&b, &frame_of(&b));
        CoastLines::walk(&mut g).splines
    }

    #[test]
    fn a_road_follows_every_line_with_the_land_on_its_left_and_has_4n_minus_6_points() {
        let splines = lines();
        let (points, roads) = generate(&splines);
        let land: Vec<&CoastlineSpline> = splines.iter().filter(|s| s.line_type == 0).collect();
        assert_eq!(roads.len(), land.len());
        assert!(!roads.is_empty());
        let mut first = 0;
        for (r, s) in roads.iter().zip(&land) {
            assert_eq!(r.num_points, 4 * s.points.len() - 6);
            assert_eq!(r.first_point, first);
            assert_eq!(r.closed, s.closed);
            first += r.num_points;
        }
        assert_eq!(points.len(), first);
    }

    #[test]
    fn the_road_lies_five_units_inland_of_the_line() {
        let splines = lines();
        let (points, roads) = generate(&splines);
        let shore = splines.iter().find(|s| s.line_type == 0).unwrap();
        let road = &points[roads[0].first_point..roads[0].first_point + roads[0].num_points];
        // Every road point is about 5 units from the line (the polyline of its control points
        // differs from the curve by a few units), and not on it.
        let line: Vec<Vec2> = shore.points.iter().map(|p| p.pos).collect();
        let distance = |p: Vec2| {
            line.windows(2)
                .map(|w| {
                    let ab = w[1] - w[0];
                    let t = ((p - w[0]).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                    (w[0] + ab * t - p).length()
                })
                .fold(f32::MAX, f32::min)
        };
        for p in road.iter().step_by(7) {
            let d = distance(*p);
            assert!(d > 1.0 && d < 9.0, "{d}");
        }
        // The island is inland: the road is nearer to the middle of the island than the line is.
        let centre = frame_of(&island(20, 20, false, 2)).hex_to_tex(10, 10);
        let mean =
            |v: &[Vec2]| v.iter().map(|p| (*p - centre).length()).sum::<f32>() / v.len() as f32;
        assert!(mean(road) < mean(&line), "{} {}", mean(road), mean(&line));
    }

    #[test]
    fn the_layout_plane_is_the_texel_plane_minus_half_the_world_with_rows_scaled() {
        let b = island(8, 8, false, 1);
        let frame = frame_of(&b);
        let half = frame.half_world();
        let p = to_layout(Vec2::new(100.0, 200.0), &frame);
        assert_eq!(p, Vec2::new(100.0 - half.x, 200.0 * 0.866 - half.y));
    }
}
