//! Maps for the tests of the coast: an island of a given radius, a map built from a function.

use crate::terrain::{Relief, TerrainKind};
use crate::world::terrain::basis::{PlotBasis, TerrainBasis};
use crate::world::terrain::sampler::TerrainFrame;

use super::grid::hex_of_cell;

fn plot(relief: Relief, terrain: TerrainKind) -> PlotBasis {
    PlotBasis {
        relief,
        terrain,
        river_flow: PlotBasis::NO_FLOW,
        ..PlotBasis::default()
    }
}

/// A map from a function of the cell.
pub fn basis_from(w: u16, h: u16, wrap: bool, f: impl Fn(i32, i32) -> PlotBasis) -> TerrainBasis {
    let mut plots = Vec::new();
    for y in 0..i32::from(h) {
        for x in 0..i32::from(w) {
            plots.push(f(x, y));
        }
    }
    TerrainBasis::new(w, h, wrap, 7, plots).expect("size matches")
}

/// A map where the function gives 0 for land, 1 for coast, anything else for ocean.
pub fn basis_with(w: u16, h: u16, wrap: bool, f: impl Fn(i32, i32) -> u8) -> TerrainBasis {
    basis_from(w, h, wrap, |x, y| match f(x, y) {
        0 => plot(Relief::Flat, TerrainKind::Grass),
        1 => plot(Relief::Water, TerrainKind::Coast),
        _ => plot(Relief::Water, TerrainKind::Ocean),
    })
}

/// The distance in hexes of the cell `(x, y)` from the hex in the middle of the map.
pub fn distance_from_center(w: u16, h: u16, x: i32, y: i32) -> i32 {
    let c = hex_of_cell(i32::from(w) / 2, i32::from(h) / 2);
    let p = hex_of_cell(x, y);
    ((c[0] - p[0]).abs() + (c[1] - p[1]).abs() + (c[2] - p[2]).abs()) / 2
}

/// An island of grass of the radius `radius` (in hexes) in the middle, a ring of coast, ocean
/// beyond.
pub fn island(w: u16, h: u16, wrap: bool, radius: i32) -> TerrainBasis {
    island_at(w, h, wrap, (i32::from(w) / 2, i32::from(h) / 2), radius)
}

/// The same island around the cell `center`; on a map that wraps the distance goes the short
/// way round.
pub fn island_at(w: u16, h: u16, wrap: bool, center: (i32, i32), radius: i32) -> TerrainBasis {
    let c = hex_of_cell(center.0, center.1);
    basis_with(w, h, wrap, |x, y| {
        let shifts: &[i32] = if wrap { &[-1, 0, 1] } else { &[0] };
        let d = shifts
            .iter()
            .map(|k| {
                let p = hex_of_cell(x + k * i32::from(w), y);
                ((c[0] - p[0]).abs() + (c[1] - p[1]).abs() + (c[2] - p[2]).abs()) / 2
            })
            .min()
            .unwrap_or(i32::MAX);
        if d <= radius {
            0
        } else if d == radius + 1 {
            1
        } else {
            2
        }
    })
}

/// The frame of a basis.
pub fn frame_of(b: &TerrainBasis) -> TerrainFrame {
    TerrainFrame::new(u32::from(b.width), u32::from(b.height), b.wrap_x)
}

/// Whether a line runs along the border of the map: the shelf where the deep sea of the map meets
/// the padding of the grid, which is all coast (re-b16 C1-1). Along y always; along x on a map that
/// does not wrap.
pub fn is_border_line(s: &super::lines::CoastlineSpline, b: &TerrainBasis) -> bool {
    let (w, h) = (f32::from(b.width) * 64.0, f32::from(b.height) * 64.0);
    s.points.iter().any(|p| {
        p.pos.y < 40.0 || p.pos.y > h - 8.0 || (!b.wrap_x && (p.pos.x < 40.0 || p.pos.x > w + 24.0))
    })
}

/// The lines of the map without the ones along its border.
pub fn inner_lines(
    mut lines: super::lines::CoastLines,
    b: &TerrainBasis,
) -> super::lines::CoastLines {
    lines.splines.retain(|s| !is_border_line(s, b));
    lines
}
