//! From the cells of a player to the control points of its borders (`TraverseBorder`,
//! `AddControlPointsFromBorderList`, `GenerateControlPoints`, `GenerateHexSegment`; RW-19).
//!
//! An edge of a cell of the player that faces a cell of another owner (or of none) is a border edge;
//! an edge on the edge of the map is not. The edges are directed round their cell (the corners as
//! `edge_corners` gives them), so a region has its cells on one side of its outline, and are chained
//! end to start into lines: closed for a region, open where the map ends. Where two regions of the
//! player touch in a corner and the chain could go either way, it keeps to the cell it came along
//! (`CivIDShouldBranch`: the sharpest turn towards the inside).
//!
//! [`control_lines`] (the spline borders of the highlights) gives every edge two control points on
//! the hex shrunk by `corner_bias`, a fifth from each end; the culture borders take theirs from
//! [`super::culture`]. Coordinates are our ground plane, `(X, Z)`; a line that crosses the seam of a
//! wrapping map goes on past it (the renderer draws the map's copies).

use std::collections::HashMap;

use glam::Vec2;

use super::super::OwnersInput;
use crate::hexgrid::{CORNER_RADIUS, Dir, GridShape, cell_center, corner_offset, edge_corners};

/// Where along an inset edge the two control points lie (from its start).
pub const EDGE_POINTS: [f32; 2] = [0.2, 0.8];

/// One border edge: the cell, the side, and the corners in the frame of the chain (continuous
/// across the seam of a wrapping map).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BorderEdge {
    pub cell: u32,
    pub dir: Dir,
    pub a: Vec2,
    pub b: Vec2,
    /// The centre of the cell, in the same frame.
    pub center: Vec2,
}

/// A control point of a border line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlPoint {
    pub position: Vec2,
    /// The cell whose edge the point lies on.
    pub cell: u32,
    /// Stable per point (cell, side, slot): the seed of the jitter, whatever else changes.
    pub key: u32,
}

/// A chain of border edges as control points.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlLine {
    pub points: Vec<ControlPoint>,
    pub closed: bool,
    /// The inside of the player's region lies to the left of the line (in the plane `(X, Z)` with
    /// the angle counted from X towards Z).
    pub inward_left: bool,
}

type Key = (i64, i64);

/// The corner at `p` as a key: equal for the two cells that share it, also over the seam.
fn corner_key(p: Vec2, shape: &GridShape) -> Key {
    let x = if shape.wrap_x {
        p.x.rem_euclid(shape.world_width())
    } else {
        p.x
    };
    let q = |v: f32| (v * 16.0).round() as i64;
    // Both ends of the period are the same corner.
    let kx = if shape.wrap_x && q(x) >= q(shape.world_width()) {
        q(x) - q(shape.world_width())
    } else {
        q(x)
    };
    (kx, q(p.y))
}

/// The border edges of `owner`, in the frame of their own cell.
fn border_edges(input: &OwnersInput, shape: &GridShape, owner: i8) -> Vec<BorderEdge> {
    let w = input.width as i32;
    let mut out = Vec::new();
    for (i, &o) in input.owners.iter().enumerate() {
        if o != owner {
            continue;
        }
        let (x, y) = (i as i32 % w, i as i32 / w);
        let center = cell_center(x, y);
        for dir in Dir::ALL {
            // The edge of the map is no border.
            let Some((nx, ny)) = shape.neighbor(x, y, dir) else {
                continue;
            };
            if input.owners[(ny * w + nx) as usize] == owner {
                continue;
            }
            let (a, b) = edge_corners(dir);
            out.push(BorderEdge {
                cell: i as u32,
                dir,
                a: center + corner_offset(a),
                b: center + corner_offset(b),
                center,
            });
        }
    }
    out
}

/// Signed angle from `from` to `to`, left turns positive in the plane's own orientation.
fn turn(from: Vec2, to: Vec2) -> f32 {
    from.perp_dot(to).atan2(from.dot(to))
}

/// The edges of `owner` as chains of edges, each moved into one frame.
pub fn chains(input: &OwnersInput, shape: &GridShape, owner: i8) -> Vec<(Vec<BorderEdge>, bool)> {
    let edges = border_edges(input, shape, owner);
    let starts: HashMap<Key, Vec<usize>> = {
        let mut m: HashMap<Key, Vec<usize>> = HashMap::new();
        for (i, e) in edges.iter().enumerate() {
            m.entry(corner_key(e.a, shape)).or_default().push(i);
        }
        m
    };
    let ends: std::collections::HashSet<Key> =
        edges.iter().map(|e| corner_key(e.b, shape)).collect();
    // First the chains that begin where nothing arrives (open ones), then what is left, which are
    // closed lines.
    let mut order: Vec<usize> = (0..edges.len())
        .filter(|&i| !ends.contains(&corner_key(edges[i].a, shape)))
        .collect();
    order.extend(0..edges.len());
    let mut used = vec![false; edges.len()];
    let mut out = Vec::new();
    for first in order {
        if used[first] {
            continue;
        }
        let mut chain: Vec<BorderEdge> = Vec::new();
        let mut shift = Vec2::ZERO;
        let mut cur = first;
        let first_key = corner_key(edges[first].a, shape);
        let mut closed = false;
        loop {
            used[cur] = true;
            let e = edges[cur];
            let placed = BorderEdge {
                a: e.a + shift,
                b: e.b + shift,
                center: e.center + shift,
                ..e
            };
            chain.push(placed);
            let end_key = corner_key(e.b, shape);
            if end_key == first_key && chain.len() > 1 {
                closed = true;
                break;
            }
            let Some(cands) = starts.get(&end_key) else {
                break;
            };
            let heading = placed.b - placed.a;
            let next = cands
                .iter()
                .copied()
                .filter(|&c| !used[c])
                .max_by(|&p, &q| {
                    let tp = turn(heading, edges[p].b - edges[p].a);
                    let tq = turn(heading, edges[q].b - edges[q].a);
                    tp.total_cmp(&tq)
                });
            let Some(next) = next else {
                break;
            };
            // The next edge's own frame may be a map width away: move it so that it starts where
            // this one ends.
            let want = placed.b;
            let have = edges[next].a;
            let step = if shape.wrap_x {
                let w = shape.world_width();
                ((want.x - have.x) / w).round() * w
            } else {
                0.0
            };
            shift = Vec2::new(step, 0.0);
            cur = next;
        }
        out.push((chain, closed));
    }
    out
}

/// The control lines of `owner`.
pub fn control_lines(
    input: &OwnersInput,
    shape: &GridShape,
    owner: i8,
    corner_bias: f32,
) -> Vec<ControlLine> {
    chains(input, shape, owner)
        .into_iter()
        .map(|(chain, closed)| {
            // The side of the inside from the first edge: the cell's centre against the edge.
            let first = chain[0];
            let t = first.b - first.a;
            let to_centre = first.center - (first.a + first.b) * 0.5;
            let inward_left = t.perp_dot(to_centre) > 0.0;
            let mut points = Vec::with_capacity(chain.len() * EDGE_POINTS.len());
            for e in &chain {
                // The hex shrunk about its centre: the edge moves inwards by `corner_bias` of the
                // distance from the centre to the edge.
                let keep = 1.0 - corner_bias;
                let a = e.center + (e.a - e.center) * keep;
                let b = e.center + (e.b - e.center) * keep;
                for (slot, &t) in EDGE_POINTS.iter().enumerate() {
                    points.push(ControlPoint {
                        position: a.lerp(b, t),
                        cell: e.cell,
                        key: e
                            .cell
                            .wrapping_mul(24)
                            .wrapping_add(e.dir.index() as u32 * 4 + slot as u32),
                    });
                }
            }
            ControlLine {
                points,
                closed,
                inward_left,
            }
        })
        .collect()
}

/// The inset of a control point from the hex edge, in world units: `corner_bias` of the apothem.
pub fn inset(corner_bias: f32) -> f32 {
    corner_bias * CORNER_RADIUS * 0.866_025_4
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(w: u32, h: u32, f: impl Fn(i32, i32) -> i8) -> OwnersInput {
        let owners = (0..h as i32)
            .flat_map(|y| (0..w as i32).map(move |x| (x, y)))
            .map(|(x, y)| f(x, y))
            .collect();
        OwnersInput::new(w, h, owners, vec![[200, 0, 0], [0, 0, 200]]).unwrap()
    }

    fn shape(w: u32, h: u32, wrap_x: bool) -> GridShape {
        GridShape {
            width: w as i32,
            height: h as i32,
            wrap_x,
        }
    }

    #[test]
    fn a_blob_is_one_closed_line_of_all_its_outer_edges() {
        // One cell in the middle of a map: six edges, one closed chain.
        let inp = input(6, 6, |x, y| if (x, y) == (3, 3) { 0 } else { -1 });
        let c = chains(&inp, &shape(6, 6, false), 0);
        assert_eq!(c.len(), 1);
        assert!(c[0].1, "closed");
        assert_eq!(c[0].0.len(), 6);
        // Each edge starts where the last ended.
        for w in c[0].0.windows(2) {
            assert!((w[0].b - w[1].a).length() < 1e-3);
        }
        let last = c[0].0.last().unwrap();
        assert!((last.b - c[0].0[0].a).length() < 1e-3);
    }

    #[test]
    fn the_edge_of_the_map_is_no_border_and_leaves_an_open_line() {
        // A cell in the corner of a flat map.
        let inp = input(4, 4, |x, y| if (x, y) == (0, 0) { 0 } else { -1 });
        let c = chains(&inp, &shape(4, 4, false), 0);
        assert_eq!(c.len(), 1);
        assert!(!c[0].1, "open at the map's edge");
        assert!(c[0].0.len() < 6);
    }

    #[test]
    fn the_cells_of_the_player_have_no_border_between_them() {
        let inp = input(8, 6, |x, y| {
            if (2..5).contains(&x) && (2..4).contains(&y) {
                0
            } else {
                1
            }
        });
        let edges = border_edges(&inp, &shape(8, 6, false), 0);
        // Perimeter of a 3 x 2 blob in a hex grid: 6 * 6 edges less two for each of the 9 pairs
        // of neighbours.
        assert_eq!(edges.len(), 18);
        let lines = chains(&inp, &shape(8, 6, false), 0);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].1);
    }

    #[test]
    fn a_region_over_the_seam_is_one_continuous_line() {
        // Columns 0 and 7 of a wrapping map of 8: one region over the seam.
        let inp = input(
            8,
            4,
            |x, y| if (x == 0 || x == 7) && y == 1 { 0 } else { -1 },
        );
        let c = chains(&inp, &shape(8, 4, true), 0);
        assert_eq!(c.len(), 1);
        assert!(c[0].1);
        for w in c[0].0.windows(2) {
            assert!(
                (w[0].b - w[1].a).length() < 1e-3,
                "continuous past the seam: {:?} {:?}",
                w[0].b,
                w[1].a
            );
        }
    }

    #[test]
    fn the_control_points_lie_inside_the_owner_near_the_edge() {
        let inp = input(6, 6, |x, y| if (x, y) == (3, 3) { 0 } else { -1 });
        let lines = control_lines(&inp, &shape(6, 6, false), 0, 0.105);
        assert_eq!(lines.len(), 1);
        let c = cell_center(3, 3);
        let l = &lines[0];
        assert_eq!(l.points.len(), 12);
        for p in &l.points {
            let d = (p.position - c).length();
            // Between the shrunk hex's inner circle and its corners.
            assert!(d > 28.0 && d < CORNER_RADIUS, "{d}");
            assert_eq!(p.cell, 3 * 6 + 3);
        }
        // Keys differ point to point.
        let mut keys: Vec<u32> = l.points.iter().map(|p| p.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), 12);
    }

    #[test]
    fn the_inside_is_on_the_same_side_all_round() {
        let inp = input(6, 6, |x, y| if (x, y) == (3, 3) { 0 } else { -1 });
        let lines = control_lines(&inp, &shape(6, 6, false), 0, 0.105);
        let l = &lines[0];
        let c = cell_center(3, 3);
        for w in l.points.windows(2) {
            let t = w[1].position - w[0].position;
            let to_centre = c - w[0].position;
            assert_eq!(t.perp_dot(to_centre) > 0.0, l.inward_left);
        }
    }

    #[test]
    fn the_inset_is_corner_bias_of_the_apothem() {
        assert!((inset(0.105) - 0.105 * 32.0).abs() < 1e-3);
    }
}
