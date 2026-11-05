//! Roads and railroads: lines of width 3 between the centres of neighbouring
//! cells that have a route, brown for a road.
//!
//! Each cell draws the half of every connection that lies inside itself, from its centre to the
//! middle of the shared edge, and the neighbour draws the other half. So a connection over the
//! seam of a wrapping map needs no unwrapped coordinates: each half sits in its own cell's frame,
//! and the renderer's map copies show it on both sides. A connection exists when both cells have
//! a route (a city counts as one; it takes the route of the neighbours it joins).
//!
//! SIMPLIFIED(SIMP-012): lines instead of the route models of the original.

use std::collections::{HashMap, HashSet};

use glam::Vec2;

use super::lines::{LineMesh, rgba};
use crate::hexgrid::{Dir, GridShape, HEX_WIDTH, cell_center};
use crate::terrain::{Fog, TerrainSnapshot};

/// Width of a road line.
// re: (RV: lines of width 3)
pub const ROAD_WIDTH: f32 = 3.0;
/// Railroads are drawn a little wider than roads.
pub const RAIL_WIDTH: f32 = 4.0;
/// Colour of a road (sRGB): brown.
pub const ROAD_COLOR: [u8; 3] = [139, 94, 52];
/// Colour of a railroad (sRGB): dark grey.
pub const RAIL_COLOR: [u8; 3] = [64, 64, 70];

/// Kind of route on a plot (`ROUTE_ROAD`, `ROUTE_RAILROAD`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RouteKind {
    Road,
    Railroad,
}

impl RouteKind {
    /// Kind of a catalog `ROUTE_*` type; any other name is a road.
    pub fn from_type_name(name: &str) -> Self {
        if name.eq_ignore_ascii_case("ROUTE_RAILROAD") {
            Self::Railroad
        } else {
            Self::Road
        }
    }

    fn width(self) -> f32 {
        match self {
            Self::Road => ROAD_WIDTH,
            Self::Railroad => RAIL_WIDTH,
        }
    }

    fn color(self) -> [f32; 4] {
        rgba(
            match self {
                Self::Road => ROAD_COLOR,
                Self::Railroad => RAIL_COLOR,
            },
            255,
        )
    }
}

/// Bit `d` is set when the neighbour of `(x, y)` in direction `d` (wrapped on a wrapping map) has
/// a route.
pub fn connection_mask(
    shape: &GridShape,
    x: i32,
    y: i32,
    has_route: &dyn Fn(i32, i32) -> bool,
) -> u8 {
    Dir::ALL
        .into_iter()
        .filter(|d| {
            shape
                .neighbor(x, y, *d)
                .is_some_and(|(nx, ny)| has_route(nx, ny))
        })
        .fold(0, |m, d| m | d.edge_bit())
}

/// World direction (X east, Z south) of the neighbour in `dir`, as a unit vector.
pub fn dir_vector(dir: Dir) -> Vec2 {
    let a = dir.normal_angle();
    Vec2::new(a.cos(), -a.sin())
}

/// Roads of the revealed cells. `routes` has the explicit routes by plot, `cities` the plots with
/// a city (they join roads but show none of their own).
pub fn build(
    terrain: &TerrainSnapshot,
    routes: &HashMap<(i32, i32), RouteKind>,
    cities: &HashSet<(i32, i32)>,
    out: &mut LineMesh,
) {
    let shape = terrain.shape();
    // A route on an unrevealed plot is not known: it neither shows nor connects.
    let known: HashMap<(i32, i32), RouteKind> = routes
        .iter()
        .filter(|((x, y), _)| {
            terrain
                .cell(*x, *y)
                .is_some_and(|c| c.fog != Fog::Unrevealed)
        })
        .map(|(p, k)| (*p, *k))
        .collect();
    let routes = &known;
    let connects = |x: i32, y: i32| routes.contains_key(&(x, y)) || cities.contains(&(x, y));
    // Row-major order: the mesh does not depend on the hash map's order.
    let mut plots: Vec<(&(i32, i32), &RouteKind)> = routes.iter().collect();
    plots.sort_by_key(|((x, y), _)| (*y, *x));
    for (&(x, y), &kind) in plots {
        let Some(index) = terrain.index(x, y) else {
            continue;
        };
        if terrain.cells()[index].fog == Fog::Unrevealed {
            continue;
        }
        let centre = cell_center(x, y);
        let mask = connection_mask(&shape, x, y, &connects);
        let color = kind.color();
        let width = kind.width();
        // The dot joins the spokes (and shows an isolated road).
        out.square(terrain, centre, width * 0.5, color, index as u32);
        for dir in Dir::ALL {
            if mask & dir.edge_bit() != 0 {
                let end = centre + dir_vector(dir) * (HEX_WIDTH * 0.5);
                out.strip(terrain, centre, end, width, color, index as u32);
            }
        }
    }
    // A city next to a route: its own spokes towards the routes, so the line reaches the city.
    let mut towns: Vec<&(i32, i32)> = cities.iter().collect();
    towns.sort_by_key(|(x, y)| (*y, *x));
    for &(x, y) in towns {
        let Some(index) = terrain.index(x, y) else {
            continue;
        };
        if terrain.cells()[index].fog == Fog::Unrevealed || routes.contains_key(&(x, y)) {
            continue;
        }
        let centre = cell_center(x, y);
        let on_route = |nx: i32, ny: i32| routes.contains_key(&(nx, ny));
        let mask = connection_mask(&shape, x, y, &on_route);
        for dir in Dir::ALL {
            if mask & dir.edge_bit() != 0 {
                let end = centre + dir_vector(dir) * (HEX_WIDTH * 0.5);
                out.strip(
                    terrain,
                    centre,
                    end,
                    ROAD_WIDTH,
                    RouteKind::Road.color(),
                    index as u32,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::test_util::{land, map};
    use crate::terrain::{TerrainCell, TerrainSnapshot};

    fn wrapped(w: u32, h: u32, f: impl Fn(i32, i32) -> TerrainCell) -> TerrainSnapshot {
        let cells = (0..h as i32)
            .flat_map(|y| (0..w as i32).map(move |x| (x, y)))
            .map(|(x, y)| f(x, y))
            .collect();
        TerrainSnapshot::new(w, h, true, cells).unwrap()
    }

    fn set(cells: &[(i32, i32)]) -> impl Fn(i32, i32) -> bool + '_ {
        move |x, y| cells.contains(&(x, y))
    }

    #[test]
    fn an_east_west_line_connects_east_and_west() {
        let shape = GridShape {
            width: 6,
            height: 4,
            wrap_x: false,
        };
        let route = [(1, 2), (2, 2), (3, 2)];
        let has = set(&route);
        assert_eq!(
            connection_mask(&shape, 2, 2, &has),
            Dir::East.edge_bit() | Dir::West.edge_bit()
        );
        assert_eq!(connection_mask(&shape, 1, 2, &has), Dir::East.edge_bit());
        assert_eq!(connection_mask(&shape, 3, 2, &has), Dir::West.edge_bit());
        // Not a neighbour, no bit.
        assert_eq!(connection_mask(&shape, 5, 2, &has), 0);
        assert_eq!(connection_mask(&shape, 2, 0, &has), 0);
    }

    #[test]
    fn diagonal_neighbours_depend_on_the_row() {
        let shape = GridShape {
            width: 8,
            height: 8,
            wrap_x: false,
        };
        // (2, 2) is an even row: its NE neighbour is (2, 3), SE is (2, 1).
        let has = set(&[(2, 3), (2, 1), (1, 3), (1, 1)]);
        let m = connection_mask(&shape, 2, 2, &has);
        assert_eq!(
            m,
            Dir::NorthEast.edge_bit()
                | Dir::SouthEast.edge_bit()
                | Dir::NorthWest.edge_bit()
                | Dir::SouthWest.edge_bit()
        );
        // (2, 3) is an odd row: its NE neighbour is (3, 4).
        let has = set(&[(3, 4)]);
        assert_eq!(
            connection_mask(&shape, 2, 3, &has),
            Dir::NorthEast.edge_bit()
        );
    }

    #[test]
    fn the_mask_wraps_over_the_seam_only_on_a_wrapping_map() {
        let route = [(0, 1), (5, 1), (5, 2), (0, 2)];
        let has = set(&route);
        let wrap = GridShape {
            width: 6,
            height: 4,
            wrap_x: true,
        };
        let flat = GridShape {
            wrap_x: false,
            ..wrap
        };
        // East of the last column is column 0; west of column 0 is the last.
        assert_ne!(connection_mask(&wrap, 5, 1, &has) & Dir::East.edge_bit(), 0);
        assert_ne!(connection_mask(&wrap, 0, 1, &has) & Dir::West.edge_bit(), 0);
        assert_eq!(connection_mask(&flat, 5, 1, &has) & Dir::East.edge_bit(), 0);
        assert_eq!(connection_mask(&flat, 0, 1, &has) & Dir::West.edge_bit(), 0);
        // Row 1 is odd: the NE neighbour of (5, 1) is (6, 2) = (0, 2) on a wrapping map.
        assert_ne!(
            connection_mask(&wrap, 5, 1, &has) & Dir::NorthEast.edge_bit(),
            0
        );
    }

    #[test]
    fn off_the_map_there_is_no_neighbour() {
        let shape = GridShape {
            width: 4,
            height: 4,
            wrap_x: true,
        };
        let has = |_: i32, _: i32| true;
        // Row 0: both southern neighbours are off the map.
        let m = connection_mask(&shape, 1, 0, &has);
        assert_eq!(
            m & (Dir::SouthEast.edge_bit() | Dir::SouthWest.edge_bit()),
            0
        );
        assert_eq!(m.count_ones(), 4);
    }

    fn spokes(m: &LineMesh) -> usize {
        // One dot (one quad) per route plot plus one piece per 16 units of each spoke (two).
        m.indices.len() / 6
    }

    #[test]
    fn a_line_of_three_draws_dots_and_half_spokes() {
        let t = map(6, 4, |_, _| land());
        let routes: HashMap<_, _> = [(1, 2), (2, 2), (3, 2)]
            .into_iter()
            .map(|p| (p, RouteKind::Road))
            .collect();
        let mut m = LineMesh::default();
        build(&t, &routes, &HashSet::new(), &mut m);
        // 3 dots; spokes: 1 + 2 + 1 = 4, each 32 long in two pieces.
        assert_eq!(spokes(&m), 3 + 4 * 2);
        assert!(m.indices_valid());
        // Every spoke ends on an edge midpoint, 32 from its centre.
        let far = m
            .vertices
            .iter()
            .filter(|v| v.cell == 2 * 6 + 2)
            .map(|v| {
                let p = Vec2::new(v.position[0], v.position[2]);
                (p - cell_center(2, 2)).length()
            })
            .fold(0.0f32, f32::max);
        assert!(far > 31.0 && far < 40.0, "{far}");
    }

    #[test]
    fn a_connection_over_the_seam_is_two_halves_in_their_own_cells() {
        let t = wrapped(6, 3, |_, _| land());
        let routes: HashMap<_, _> = [(5, 1), (0, 1)]
            .into_iter()
            .map(|p| (p, RouteKind::Road))
            .collect();
        let mut m = LineMesh::default();
        build(&t, &routes, &HashSet::new(), &mut m);
        // All geometry stays within a cell's reach of its own centre: no strip spans the map.
        let c5 = cell_center(5, 1);
        let c0 = cell_center(0, 1);
        for v in &m.vertices {
            let p = Vec2::new(v.position[0], v.position[2]);
            assert!(
                (p - c5).length() < 40.0 || (p - c0).length() < 40.0,
                "{p:?}"
            );
        }
        // The half of (5, 1) points east, out of its cell, on the right edge of the map.
        assert!(
            m.vertices
                .iter()
                .any(|v| v.position[0] > c5.x + 30.0 && (v.position[2] - c5.y).abs() < 2.0)
        );
        // And the half of (0, 1) points west of column 0.
        assert!(
            m.vertices
                .iter()
                .any(|v| v.position[0] < c0.x - 30.0 && (v.position[2] - c0.y).abs() < 2.0)
        );
    }

    #[test]
    fn unrevealed_plots_have_no_road_and_cities_join_roads() {
        let t = map(5, 3, |x, _| {
            if x == 4 {
                TerrainCell::default()
            } else {
                land()
            }
        });
        let routes: HashMap<_, _> = [(2, 1), (4, 1)]
            .into_iter()
            .map(|p| (p, RouteKind::Road))
            .collect();
        let mut m = LineMesh::default();
        build(&t, &routes, &HashSet::new(), &mut m);
        // One dot, no spoke: (4, 1) is not revealed and not a neighbour.
        assert_eq!(spokes(&m), 1);
        // A city next to the road: the road gets a spoke towards it, and the city one back.
        let cities: HashSet<_> = [(3, 1)].into_iter().collect();
        let mut with_city = LineMesh::default();
        build(&t, &routes, &cities, &mut with_city);
        // (2, 1): dot + one spoke (2 pieces); the city: one spoke (2 pieces).
        assert_eq!(spokes(&with_city), 1 + 2 + 2);
        // A city alone, or beside nothing, draws nothing.
        let mut lone = LineMesh::default();
        build(&t, &HashMap::new(), &cities, &mut lone);
        assert!(lone.is_empty());
    }

    #[test]
    fn railroads_are_wider_and_other_names_are_roads() {
        assert_eq!(
            RouteKind::from_type_name("ROUTE_RAILROAD"),
            RouteKind::Railroad
        );
        assert_eq!(RouteKind::from_type_name("ROUTE_ROAD"), RouteKind::Road);
        assert_eq!(RouteKind::from_type_name("ROUTE_X"), RouteKind::Road);
        assert!(RouteKind::Railroad.width() > RouteKind::Road.width());
        assert!(dir_vector(Dir::East).distance(Vec2::X) < 1e-5);
        // South is +Z, north is -Z.
        assert!(dir_vector(Dir::SouthEast).y > 0.0 && dir_vector(Dir::NorthEast).y < 0.0);
    }
}
