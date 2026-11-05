//! The coastlines: `cvCoastSplineSystem::Startup` and
//! `ModifyAndExtractCoastlines`. The links of the grid are followed into lines of control points,
//! open ones first, then the closed loops; lines of fewer than four points are dropped; the noise
//! of [`super::noise`] is added line by line in the order they were found, so the stream of
//! `rand` is spent in that order.

use glam::Vec2;

use super::grid::{
    CLASS_DEEP, CLASS_LAND, CLASS_SHALLOW, CoastGrid, NO_PEER, TRANSLATIONS, cell_of_hex,
    hex_of_cell,
};
use super::noise;
use crate::world::terrain::build::rand::MsvcRand;
use crate::world::terrain::sampler::TerrainFrame;
use crate::world::water::river::hex::HexEdge;

/// A line needs at least this many control points to be kept.
// re: cvCoastSplineSystem.cpp:23 (kMinSplinePoints), SplineAdder::Add 0x088ec1b8
pub const MIN_SPLINE_POINTS: usize = 4;

/// A control point of a coastline: the position, the blend of the two styles of the coast cells
/// on both sides of the point, and the indices of the styles.
// re: class CoastCP (cvCoastSplineSystem.h:26)
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CoastCP {
    pub pos: Vec2,
    pub blend: f32,
    pub style_a: u8,
    pub style_b: u8,
}

/// The grid edge of one pair of control points of a line: the cell as the walk names it (its column
/// can lie outside the map when the map wraps), the edge id of the cell (`m_aucEdges`), the ends of the
/// segment before the noise (in the texels of the cell put inside the map) and the shift of the wrap
/// the walk added to the pair.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SegmentCell {
    pub col: i32,
    pub row: i32,
    pub edge_id: u8,
    pub near: Vec2,
    pub far: Vec2,
    pub shift: f32,
}

/// One coastline: a run of control points found by walking the links of the grid.
// re: class CoastlineSpline (cvCoastSplineSystem.h:37)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CoastlineSpline {
    pub points: Vec<CoastCP>,
    pub start_col: i32,
    pub start_row: i32,
    pub start_edge: u8,
    /// True when the walk came back to its first segment; `wrap_offset` is then the last point
    /// minus the second one.
    pub closed: bool,
    pub wrap_offset: Vec2,
    /// 0 when the first edge is on the land side of its line (a shore seen from land, a shelf
    /// edge seen from shallow water), 1 on the sea side.
    pub line_type: i32,
    /// The art style of the continent whose styles the line takes (always 1, America).
    pub continent: u8,
    /// The grid edge of every pair of points (`points[2k]`, `points[2k + 1]`), for
    /// `AddSingleCoastlineToDatabase`.
    pub cells: Vec<SegmentCell>,
}

/// The lines of a map.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CoastLines {
    pub splines: Vec<CoastlineSpline>,
}

/// How many whole widths of the grid a column lies outside it (negative: to the left).
// re: ExtractSingleCoastlineFromGrid 0x088ec7a2 (`GridWrapCount`)
fn grid_wrap_count(column: i32, width: i32) -> i32 {
    let mut wraps = 0;
    let mut shifted = column;
    while shifted < 0 {
        wraps -= 1;
        shifted += width;
    }
    if width <= shifted {
        wraps = shifted / width;
    }
    wraps
}

impl CoastLines {
    /// `Startup` / `ModifyAndExtractCoastlines`: the lines of the grid, with the noise. The
    /// stream `rng` is the one of the build; the noise takes [`noise::draws`] draws per line.
    // re: cvCoastSplineSystem::ModifyAndExtractCoastlines 0x088ec1b8 (cvCoastSplineSystem.cpp:111-192)
    pub fn extract(grid: &mut CoastGrid, rng: &mut MsvcRand) -> Self {
        let mut lines = Self::walk(grid);
        for spline in &mut lines.splines {
            spline.add_noise(rng);
        }
        lines
    }

    /// The lines without the noise; the marks of the grid are cleared at the end.
    pub fn walk(grid: &mut CoastGrid) -> Self {
        let mut splines = Vec::new();
        // Open coastlines first: a start link that nothing leads into.
        Self::scan(grid, &mut splines, true);
        // What is left are closed loops.
        Self::scan(grid, &mut splines, false);
        grid.clear_marks();
        Self { splines }
    }

    fn scan(grid: &mut CoastGrid, splines: &mut Vec<CoastlineSpline>, open_only: bool) {
        for row in grid.rows() {
            for col in grid.cols() {
                for slot in 0..3usize {
                    let cell = grid.cell(col, row);
                    let next = cell.next[slot];
                    let prev = cell.prev[slot];
                    let start = next.peer_edge != NO_PEER
                        && !next.marked
                        && (!open_only || prev.peer_edge == NO_PEER);
                    if start {
                        let spline = CoastlineSpline::extract(grid, col, row, slot as u8);
                        if spline.points.len() >= MIN_SPLINE_POINTS {
                            splines.push(spline);
                        }
                    }
                }
            }
        }
    }
}

/// `AddAllCoastlinesToDatabase`: the noised segments of the shore lines (type 0: land against
/// shallow water), each under the side of its land hex that faces the water, in the direction of
/// that side (`HexEdge` code 0..5: the land on the right of the flow), in the order the original
/// submits them; a later segment of the same side replaces an earlier one. The points are in the
/// texel plane of the cell put inside the map (the shift of the wrap taken off), as
/// `GameSplineDatabase::SubmitSegmentToDatabase` takes them.
// re: cvCoastSplineSystem::AddSingleCoastlineToDatabase 0x088ed458 (cvCoastSplineSystem.cpp:400-471):
// the edge id of the cell is its code there; ours names the side by its land hex and the hex across.
pub fn shore_segments(lines: &CoastLines, frame: &TerrainFrame) -> Vec<(HexEdge, [Vec2; 2])> {
    let mut out = Vec::new();
    for spline in lines.splines.iter().filter(|s| s.line_type == 0) {
        for (k, cell) in spline.cells.iter().enumerate() {
            let (Some(a), Some(b)) = (spline.points.get(2 * k), spline.points.get(2 * k + 1))
            else {
                break;
            };
            let id = cell.edge_id;
            if id >= 12 {
                continue;
            }
            // The side toward the direction `id % 6` of the cell; ids 0..5 are seen from the land.
            let owner = hex_of_cell(cell.col, cell.row);
            let t = TRANSLATIONS[usize::from(id % 6)];
            let across = cell_of_hex([owner[0] + t[0], owner[1] + t[1], owner[2] + t[2]]);
            let ((land_c, land_r), water) = if id < 6 {
                ((cell.col, cell.row), across)
            } else {
                (across, (cell.col, cell.row))
            };
            let Some(edge) = (0..6u8)
                .map(|s| HexEdge::of_cell(land_c, land_r, s))
                .find(|e| e.twin().cell() == water)
            else {
                continue;
            };
            let shift = Vec2::new(cell.shift, 0.0);
            let (near, far) = (a.pos - shift, b.pos - shift);
            // The side's own direction, from its generated points.
            let [p, q] = edge.side_points(frame);
            let pts = if (q - p).dot(cell.far - cell.near) > 0.0 {
                [near, far]
            } else {
                [far, near]
            };
            out.push((edge, pts));
        }
    }
    out
}

impl CoastlineSpline {
    fn push(&mut self, pos: Vec2, blend: f32, style_a: u8, style_b: u8) {
        self.points.push(CoastCP {
            pos,
            blend,
            style_a,
            style_b,
        });
    }

    /// Walks the links from the edge `start_edge` of the cell `(col, row)` and marks them.
    // re: cvCoastSplineSystem::ExtractSingleCoastlineFromGrid 0x088ec7a2 (cvCoastSplineSystem.cpp:195-301)
    fn extract(grid: &mut CoastGrid, col: i32, row: i32, start_edge: u8) -> Self {
        let mut s = Self {
            start_col: col,
            start_row: row,
            start_edge,
            continent: 1,
            ..Self::default()
        };
        let width = grid.width();
        let wrap_shift = grid.wrap_shift();
        let cell = *grid.cell(col, row);
        let edge = usize::from(start_edge);
        let seg = cell.segments[edge];
        let shore = cell.edges[edge] % 12 < 6;
        // The line runs from the first to the second end for a side seen from the land side, the
        // other way for the other one.
        let ends = |seg: &super::grid::Segment, shore: bool| {
            if shore {
                (seg.first, seg.second)
            } else {
                (seg.second, seg.first)
            }
        };
        let (near, far) = ends(&seg, shore);
        let wraps = grid.wraps();
        let in_map = move |c: i32| if wraps { c.rem_euclid(width) } else { c };
        s.cells.push(SegmentCell {
            col: in_map(col),
            row,
            edge_id: cell.edges[edge],
            near,
            far,
            shift: 0.0,
        });
        let first_kind = cell.edge_kind[edge];
        s.push(near, 1.0, first_kind, 0);
        let second_kind = cell.edge_kind[edge];
        let mut weight = if first_kind == second_kind {
            1.0f32
        } else {
            0.0
        };
        s.push(far, weight, second_kind, first_kind);
        let mut previous_kind = second_kind;
        let mut current_kind = first_kind;
        // The terrain class of the first cell decides which side of the line is land.
        s.line_type = match (shore, cell.terrain_class) {
            (true, CLASS_SHALLOW) | (false, CLASS_DEEP) => 1,
            (true, CLASS_LAND) | (false, CLASS_SHALLOW) => 0,
            _ => 0,
        };

        // The link in hand: the cell it sits in (as the walk names it) and the slot.
        let (mut link_col, mut link_row, mut link_slot) = (col, row, edge);
        let (mut last_col, mut last_row, mut last_edge) = (col, row, start_edge);
        if !grid.cell(link_col, link_row).next[link_slot].marked {
            let mut shift = 0.0f32;
            loop {
                let link = grid.cell(link_col, link_row).next[link_slot];
                let edge_next = link.peer_edge;
                let (next_col, next_row) = (link.col, link.row);
                last_col = next_col;
                last_row = next_row;
                last_edge = edge_next;
                let next = *grid.cell(next_col, next_row);
                let e = usize::from(edge_next);
                // The padding of a map that does not wrap has columns below 0: no wrap there.
                if wraps {
                    shift += grid_wrap_count(next_col, width) as f32 * wrap_shift.x;
                }
                let nseg = next.segments[e];
                let next_shore = next.edges[e] % 12 < 6;
                let (near, far) = ends(&nseg, next_shore);
                let kind = next.edge_kind[e];
                // The blend weight flips between 0 and 1 whenever the style changes along the line.
                if current_kind != kind {
                    weight = if weight == 1.0 { 0.0 } else { 1.0 };
                    current_kind = kind;
                }
                s.cells.push(SegmentCell {
                    col: in_map(next_col),
                    row: next_row,
                    edge_id: next.edges[e],
                    near,
                    far,
                    shift,
                });
                s.push(near + Vec2::new(shift, 0.0), weight, kind, previous_kind);
                s.push(far + Vec2::new(shift, 0.0), weight, kind, kind);
                previous_kind = kind;
                grid.set_marked(link_col, link_row, link_slot, true);
                (link_col, link_row, link_slot) = (next_col, next_row, e);
                // The walk stops at a link that was walked, and at the end of an open line. The
                // original goes on at the end of an open line with the edge 0xff and reads past
                // the cell.
                let onward = grid.cell(link_col, link_row).next[link_slot];
                if onward.marked || onward.peer_edge == NO_PEER {
                    break;
                }
            }
        }

        let (mut start_col, mut end_col) = (s.start_col, last_col);
        if grid.wraps() {
            start_col = start_col.rem_euclid(width);
            end_col = end_col.rem_euclid(width);
        }
        if end_col == start_col && last_row == s.start_row && s.start_edge == last_edge {
            s.closed = true;
            let n = s.points.len();
            s.wrap_offset = s.points[n - 1].pos - s.points[1].pos;
        }
        s
    }

    /// `AddNoiseToSingleCoastline`: the noise of [`noise::add_noise`] on the positions; a closed
    /// line repeats its last two points in front of the first two.
    // re: cvCoastSplineSystem::AddNoiseToSingleCoastline 0x088ece2e (cvCoastSplineSystem.cpp:317-398)
    pub fn add_noise(&mut self, rng: &mut MsvcRand) {
        let positions: Vec<Vec2> = self.points.iter().map(|p| p.pos).collect();
        let moved = noise::add_noise(&positions, rng);
        for (p, m) in self.points.iter_mut().zip(moved) {
            p.pos = m;
        }
        if self.closed {
            let n = self.points.len();
            self.points[0].pos = self.points[n - 2].pos - self.wrap_offset;
            self.points[1].pos = self.points[n - 1].pos - self.wrap_offset;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::{frame_of, island};
    use super::*;

    fn lines_of(radius: i32) -> (CoastGrid, CoastLines) {
        let b = island(20, 20, false, radius);
        let mut g = CoastGrid::new(&b, &frame_of(&b));
        let lines = super::super::testkit::inner_lines(CoastLines::walk(&mut g), &b);
        (g, lines)
    }

    #[test]
    fn an_island_gives_a_closed_shore_and_a_closed_shelf_line() {
        let (_, lines) = lines_of(2);
        assert_eq!(lines.splines.len(), 2);
        assert!(lines.splines.iter().all(|s| s.closed));
        // The shore of land of radius 2 has 30 sides, two points each; the shelf of the coast
        // of radius 3 has 42.
        let mut sizes: Vec<usize> = lines.splines.iter().map(|s| s.points.len()).collect();
        sizes.sort_unstable();
        assert_eq!(sizes, [60 + 2, 84 + 2]);
    }

    /// re-b16 W3: every side of the shore of the island goes to the database once, under its land
    /// hex (the hex across is water), in the direction of the side, near the straight side.
    #[test]
    fn the_shore_goes_to_the_database_under_the_sides_of_its_land() {
        let b = island(20, 20, false, 2);
        let frame = frame_of(&b);
        let mut g = CoastGrid::new(&b, &frame);
        let lines = CoastLines::walk(&mut g);
        let segs = shore_segments(&lines, &frame);
        let sides: std::collections::BTreeSet<(i32, i32, u8)> =
            segs.iter().map(|(e, _)| (e.x, e.y, e.code)).collect();
        // 30 sides of land against water; the first of the closed line is submitted twice.
        assert_eq!(sides.len(), 30);
        for (e, pts) in &segs {
            assert!(e.code < 6);
            let (c, r) = e.cell();
            let land = b.plot(c, r).unwrap();
            assert_ne!(land.relief, crate::terrain::Relief::Water);
            let (tc, tr) = e.twin().cell();
            assert_eq!(
                b.plot(tc, tr).unwrap().relief,
                crate::terrain::Relief::Water
            );
            let [p, q] = e.side_points(&frame);
            assert!((q - p).dot(pts[1] - pts[0]) > 0.0);
            assert!(
                p.distance(pts[0]) < 8.0 && q.distance(pts[1]) < 8.0,
                "{e:?}"
            );
        }
    }

    #[test]
    fn a_closed_line_comes_back_to_where_it_started() {
        let (_, lines) = lines_of(2);
        for s in &lines.splines {
            let n = s.points.len();
            // The walk ends on the first segment again: its last two points are the first two.
            assert!((s.points[n - 2].pos - s.points[0].pos).length() < 1e-3);
            assert!((s.points[n - 1].pos - s.points[1].pos).length() < 1e-3);
            assert!(s.wrap_offset.length() < 1e-3, "{}", s.wrap_offset);
        }
    }

    #[test]
    fn the_line_keeps_the_land_on_its_left() {
        // The shore line of the island goes counter-clockwise around the centre: the cross
        // product of the radius and the step is positive.
        let (_, lines) = lines_of(2);
        let centre = frame_of(&island(20, 20, false, 2)).hex_to_tex(10, 10);
        for s in &lines.splines {
            let mut turn = 0.0f32;
            for w in s.points.windows(2) {
                let r = w[0].pos - centre;
                let d = w[1].pos - w[0].pos;
                turn += r.x * d.y - r.y * d.x;
            }
            assert!(turn > 0.0, "type {} turn {turn}", s.line_type);
        }
    }

    #[test]
    fn the_kind_of_the_first_edge_decides_the_type_and_the_styles_follow_the_hexes() {
        let (_, lines) = lines_of(2);
        for s in &lines.splines {
            assert!(s.line_type == 0 || s.line_type == 1);
            assert_eq!(s.continent, 1);
        }
        // The shore of a grass island has the style of grass on the land side and the shallows
        // style on the shelf.
        let kinds: std::collections::BTreeSet<u8> = lines
            .splines
            .iter()
            .flat_map(|s| s.points.iter().map(|p| p.style_a))
            .collect();
        assert!(kinds.contains(&1), "{kinds:?}");
        assert!(kinds.contains(&12), "{kinds:?}");
    }

    /// re-b16 C1-1: the grid is the padded game grid, all coast round the map, so a coast that runs
    /// into the border of a map without wrap goes on through the padding and closes: the land at the
    /// border gets its beach, the deep sea at the border its shelf, and no line is left open.
    #[test]
    fn a_coast_that_runs_into_the_border_of_a_map_without_wrap_goes_round_through_the_padding() {
        // An island cut by the left border of the map.
        let b = super::super::testkit::island_at(12, 12, false, (0, 6), 2);
        let mut g = CoastGrid::new(&b, &frame_of(&b));
        let lines = CoastLines::walk(&mut g);
        assert!(!lines.splines.is_empty());
        assert!(lines.splines.iter().all(|s| s.closed));
        // The shore of the island runs along the left border of the map (the sides of column 0
        // toward the padding lie at x = 0).
        assert!(
            lines
                .splines
                .iter()
                .any(|s| s.points.iter().any(|p| p.pos.x < 8.0))
        );
        // The deep sea at the border of the map has a shelf line all round it.
        assert!(
            lines
                .splines
                .iter()
                .any(|s| super::super::testkit::is_border_line(s, &b))
        );
    }
    #[test]
    fn the_same_island_on_a_map_that_wraps_is_whole_across_the_seam() {
        let b = super::super::testkit::island_at(12, 12, true, (0, 6), 2);
        let mut g = CoastGrid::new(&b, &frame_of(&b));
        let lines = super::super::testkit::inner_lines(CoastLines::walk(&mut g), &b);
        assert_eq!(lines.splines.len(), 2);
        assert!(lines.splines.iter().all(|s| s.closed));
        // Part of the line is on the other side of the seam, in unwrapped texels.
        let width = 12.0 * 64.0;
        assert!(
            lines
                .splines
                .iter()
                .any(|s| s.points.iter().any(|p| p.pos.x < 0.0 || p.pos.x > width))
        );
        // The line goes across the seam and back: it closes on itself, with no offset.
        assert!(lines.splines.iter().all(|s| s.wrap_offset.length() < 1e-2));
    }

    #[test]
    fn land_next_to_deep_water_with_no_shallows_has_no_coast() {
        let b =
            super::super::testkit::basis_with(
                10,
                10,
                false,
                |x, y| {
                    if (x, y) == (5, 5) { 0 } else { 2 }
                },
            );
        let mut g = CoastGrid::new(&b, &frame_of(&b));
        // Land and deep water share no edge class: no edge at all (but the shelf of the border).
        let lines = super::super::testkit::inner_lines(CoastLines::walk(&mut g), &b);
        assert!(lines.splines.is_empty());
    }
}
