//! `clip-path: polygon(...)`: the points of a record resolved against a node's border box and mapped into the
//! space of the caller (chunk space for painting, page space for hit-testing).
//!
//! The polygon cuts the node itself and its whole subtree. An empty list is no clip. A polygon that encloses
//! no area (all points on a line, or fewer than three distinct points) hides the node and its subtree, as in
//! CSS. A polygon that is not convex at the node's size cannot be drawn (the draw layer clips with convex
//! polygons only): it is reported and the node is drawn unclipped.

use glam::Vec2;
use ui2_draw::Polygon;
use ui2_package::style::Point;

/// What the clip-path of a node does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ClipPath {
    /// No `clip-path`.
    None,
    /// The polygon encloses nothing: the node and its subtree are hidden.
    Empty,
    /// The node and its subtree are cut to the polygon.
    Poly(Polygon),
}

/// Twice the signed area of a polygon.
fn area2(p: &[Vec2]) -> f32 {
    let n = p.len();
    (0..n).map(|i| p[i].perp_dot(p[(i + 1) % n])).sum()
}

/// Resolves a record's polygon in a border box of `size` CSS px; `map` takes a point of the border box (CSS
/// px from its top left corner) to the target space. `Err` when the polygon is not convex there.
pub(crate) fn resolve(
    points: &[Point],
    size: [f32; 2],
    map: impl Fn([f32; 2]) -> Vec2,
) -> Result<ClipPath, &'static str> {
    if points.is_empty() {
        return Ok(ClipPath::None);
    }
    let mut pts: Vec<Vec2> = Vec::with_capacity(points.len());
    for p in points {
        let q = map(p.resolve(size));
        if !q.is_finite() {
            return Ok(ClipPath::Empty);
        }
        // Repeated points add no edge (an edge of length zero has no direction).
        if pts.last().is_none_or(|l| (*l - q).length_squared() > 1e-8) {
            pts.push(q);
        }
    }
    while pts.len() > 1
        && pts
            .first()
            .zip(pts.last())
            .is_some_and(|(a, b)| (*a - *b).length_squared() <= 1e-8)
    {
        pts.pop();
    }
    if pts.len() < 3 || area2(&pts).abs() < 1e-4 {
        return Ok(ClipPath::Empty);
    }
    Polygon::new(&pts)
        .map(ClipPath::Poly)
        .map_err(|_| "a clip-path polygon that is not convex at the node's size")
}

/// Whether a point is inside a convex polygon (on an edge counts as inside).
pub(crate) fn contains(poly: &Polygon, p: Vec2) -> bool {
    let pts = poly.points();
    let n = pts.len();
    let mut sign = 0.0f32;
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let c = (b - a).perp_dot(p - a);
        if c == 0.0 {
            continue;
        }
        if sign == 0.0 {
            sign = c.signum();
        } else if c.signum() != sign {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use ui2_package::style::Len;

    fn pt(x: Len, y: Len) -> Point {
        Point { x, y }
    }

    #[test]
    fn a_polygon_resolves_against_the_box_and_contains_its_inside() {
        // A diamond in a 100 x 50 box, mapped 10 px to the right.
        let pts = [
            pt(Len::pct(50.0), Len::ZERO),
            pt(Len::pct(100.0), Len::pct(50.0)),
            pt(Len::pct(50.0), Len::pct(100.0)),
            pt(Len::ZERO, Len::pct(50.0)),
        ];
        let c = resolve(&pts, [100.0, 50.0], |p| Vec2::new(p[0] + 10.0, p[1]));
        let Ok(ClipPath::Poly(poly)) = c else {
            panic!("{c:?}");
        };
        assert_eq!(poly.points()[1], Vec2::new(110.0, 25.0));
        assert!(contains(&poly, Vec2::new(60.0, 25.0)));
        assert!(contains(&poly, Vec2::new(60.0, 0.0)), "a vertex is inside");
        assert!(
            !contains(&poly, Vec2::new(12.0, 2.0)),
            "a corner of the box is cut"
        );
    }

    #[test]
    fn empty_flat_and_concave_polygons() {
        assert_eq!(resolve(&[], [10.0, 10.0], Vec2::from), Ok(ClipPath::None));
        let line = [
            pt(Len::ZERO, Len::ZERO),
            pt(Len::pct(50.0), Len::pct(50.0)),
            pt(Len::pct(100.0), Len::pct(100.0)),
        ];
        assert_eq!(
            resolve(&line, [10.0, 10.0], Vec2::from),
            Ok(ClipPath::Empty)
        );
        // A zero-size box makes every polygon flat.
        let tri = [
            pt(Len::ZERO, Len::ZERO),
            pt(Len::pct(100.0), Len::ZERO),
            pt(Len::ZERO, Len::pct(100.0)),
            pt(Len::ZERO, Len::ZERO),
        ];
        assert_eq!(resolve(&tri, [0.0, 0.0], Vec2::from), Ok(ClipPath::Empty));
        assert!(matches!(
            resolve(&tri, [10.0, 10.0], Vec2::from),
            Ok(ClipPath::Poly(p)) if p.points().len() == 3
        ));
        let dart = [
            pt(Len::ZERO, Len::ZERO),
            pt(Len::pct(100.0), Len::pct(50.0)),
            pt(Len::ZERO, Len::pct(100.0)),
            pt(Len::pct(30.0), Len::pct(50.0)),
        ];
        assert!(resolve(&dart, [10.0, 10.0], Vec2::from).is_err());
    }
}
