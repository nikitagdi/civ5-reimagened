//! `GameSplineDatabase` (world-water WW-4, RW-12): the control points of the sides of the hexes.
//! The rivers submit the segments they run along (`AddSegmentsToGameSplineDatabase`); the
//! floodplains, the waves and the culture borders ask for the segment of a side and get the saved
//! one, or the one `GenerateHexSide` makes from the corners of the hex.
//!
//! The original keeps one entry per (hex, edge code) in a grid of 200 x 200 hexes; the points are
//! kept relative to the centre of the hex in the texel plane (so a segment of a hex that was
//! asked for unwrapped comes out unwrapped). A point is a position and a float (`field_8`): 1 for a
//! saved point, 0 for a generated one.

use std::collections::HashMap;

use glam::Vec2;

use super::hex::HexEdge;
use crate::hexgrid::{GridShape, cell_center};
use crate::world::terrain::sampler::TerrainFrame;

/// The float a saved point carries.
// re: GameSplineDatabase::InsertEntry 0x087d49b8 (`= 0x3f800000`)
pub const SAVED_EXTRA: f32 = 1.0;
/// The float a generated point carries.
// re: GameSplineDatabase::GenerateHexSide 0x087d4f28 (`local_3c = 0`)
pub const GENERATED_EXTRA: f32 = 0.0;

/// A point of a segment.
// re: GameSpline::ControlPoint (`m_vPos`, `field_8`)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlPoint {
    pub pos: Vec2,
    pub extra: f32,
}

/// The saved segments, by (wrapped hex, edge code).
// re: GameSplineDatabase (g_GameSplineDatabase)
#[derive(Clone, Debug, Default)]
pub struct SplineDb {
    ctx: Option<(TerrainFrame, GridShape)>,
    /// Points relative to the centre of the owned hex (texels).
    saved: HashMap<HexEdge, Vec<Vec2>>,
}

impl SplineDb {
    pub fn new(frame: TerrainFrame) -> Self {
        Self {
            ctx: Some((frame, frame.grid_shape())),
            saved: HashMap::new(),
        }
    }

    fn frame(&self) -> &TerrainFrame {
        &self.ctx.as_ref().expect("a spline database has a frame").0
    }

    fn shape(&self) -> GridShape {
        self.ctx.as_ref().expect("a spline database has a frame").1
    }

    /// The number of saved entries (both directions count).
    pub fn len(&self) -> usize {
        self.saved.len()
    }

    pub fn is_empty(&self) -> bool {
        self.saved.is_empty()
    }

    /// The centre of the (unwrapped) hex of the edge in the texel plane.
    fn hex_tex(&self, e: HexEdge) -> Vec2 {
        let (col, row) = e.cell();
        self.frame().world_to_tex(cell_center(col, row))
    }

    /// The key of an entry: the hex put inside the map, the code kept.
    // re: GameSplineDatabase::QuerySavedHexSegment 0x087d4cd2 (Fun_0865f23e on the owned form)
    fn key(&self, e: HexEdge) -> Option<HexEdge> {
        let (col, row) = e.cell();
        let (c, r) = self.shape().normalize(col, row)?;
        Some(HexEdge::of_cell(c, r, e.code))
    }

    /// The hex across the side, with the hex of the edge wrapped in x first.
    fn across_cell(&self, e: HexEdge) -> (i32, i32) {
        let shape = self.shape();
        let (col, row) = e.cell();
        let col = if shape.wrap_x {
            col.rem_euclid(shape.width)
        } else {
            col
        };
        HexEdge::of_cell(col, row, e.code).twin().cell()
    }

    /// The hex across the side lies above or below the map.
    // re: GameSplineDatabase::IsAtTopOrBottomBoundary 0x087d4858
    fn is_at_top_or_bottom(&self, e: HexEdge) -> bool {
        let (_, row) = self.across_cell(e);
        row < 0 || row >= self.shape().height
    }

    /// The hex across the side lies left or right of the map.
    // re: GameSplineDatabase::IsAtLeftOrRightBoundary 0x087d4904
    fn is_at_left_or_right(&self, e: HexEdge) -> bool {
        let (col, _) = self.across_cell(e);
        col < 0 || col >= self.shape().width
    }

    /// Saves the points of a segment under the edge, and the reversed points under the edge the
    /// other way round. A side at the border of the map is not saved.
    // re: GameSplineDatabase::SubmitSegmentToDatabase 0x087d4762 and InsertEntry 0x087d49b8
    // (`WorldGlobals::g_eWrapMode == 0` is a map that does not wrap: `wrap_x` false)
    pub fn submit(&mut self, edge: HexEdge, pts: &[Vec2]) {
        if self.is_at_top_or_bottom(edge) {
            return;
        }
        if !self.shape().wrap_x && self.is_at_left_or_right(edge) {
            return;
        }
        let owned = edge.owned();
        let base = self.hex_tex(owned);
        let rel: Vec<Vec2> = pts.iter().map(|p| *p - base).collect();
        let reversed: Vec<Vec2> = rel.iter().rev().copied().collect();
        if let Some(k) = self.key(owned) {
            self.saved.insert(k, rel);
        }
        if let Some(k) = self.key(owned.reversed()) {
            self.saved.insert(k, reversed);
        }
    }

    /// The saved segment of the edge, `None` if nothing was saved for it.
    // re: GameSplineDatabase::QuerySavedHexSegment 0x087d4cd2
    pub fn query_saved(&self, edge: HexEdge) -> Option<Vec<ControlPoint>> {
        let owned = edge.owned();
        let rel = self.saved.get(&self.key(owned)?)?;
        let base = self.hex_tex(owned);
        Some(
            rel.iter()
                .map(|p| ControlPoint {
                    pos: base + *p,
                    extra: SAVED_EXTRA,
                })
                .collect(),
        )
    }

    /// The two points `GenerateHexSide` makes for the side of the edge, in the direction of the
    /// edge.
    // re: GameSplineDatabase::GenerateHexSide 0x087d4f28 (via QueryGeneratedHexSegment 0x087d4ef2)
    pub fn query_generated_segment(&self, edge: HexEdge) -> Vec<ControlPoint> {
        edge.side_points(self.frame())
            .into_iter()
            .map(|pos| ControlPoint {
                pos,
                extra: GENERATED_EXTRA,
            })
            .collect()
    }

    /// The saved segment if there is one, else the generated one.
    // re: GameSplineDatabase::QueryHexSegment 0x087d4ea0
    pub fn query_segment(&self, edge: HexEdge) -> Vec<ControlPoint> {
        self.query_saved(edge)
            .unwrap_or_else(|| self.query_generated_segment(edge))
    }

    /// [`Self::query_segment`] without the floats.
    pub fn query(&self, edge: HexEdge) -> Vec<Vec2> {
        self.query_segment(edge)
            .into_iter()
            .map(|c| c.pos)
            .collect()
    }

    /// [`Self::query_generated_segment`] without the floats.
    pub fn query_generated(&self, edge: HexEdge) -> Vec<Vec2> {
        self.query_generated_segment(edge)
            .into_iter()
            .map(|c| c.pos)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(wrap: bool) -> SplineDb {
        SplineDb::new(TerrainFrame::new(16, 16, wrap))
    }

    fn pts(e: HexEdge, db: &SplineDb) -> [Vec2; 2] {
        let [a, b] = [db.query_generated(e)[0], db.query_generated(e)[1]];
        // A bend away from the straight side.
        [a + Vec2::new(3.0, -2.0), b + Vec2::new(-1.0, 4.0)]
    }

    #[test]
    fn an_unsaved_edge_gives_the_generated_side() {
        let d = db(false);
        let e = HexEdge::of_cell(5, 5, 1);
        assert!(d.query_saved(e).is_none());
        let seg = d.query_segment(e);
        assert_eq!(seg.len(), 2);
        assert!(seg.iter().all(|c| c.extra == GENERATED_EXTRA));
        assert_eq!(d.query(e), d.query_generated(e));
        assert_eq!(d.query(e), e.side_points(d.frame()).to_vec());
    }

    #[test]
    fn a_saved_segment_comes_back_with_the_float_one() {
        let mut d = db(false);
        let e = HexEdge::of_cell(5, 5, 1);
        let p = pts(e, &d);
        d.submit(e, &p);
        let seg = d.query_saved(e).expect("saved");
        assert_eq!(seg.len(), 2);
        for (c, q) in seg.iter().zip(p) {
            assert!(c.pos.distance(q) < 1e-3);
            assert_eq!(c.extra, SAVED_EXTRA);
        }
        assert_eq!(d.query(e).len(), 2);
        assert!(d.query(e)[0].distance(p[0]) < 1e-3);
    }

    #[test]
    fn the_same_edge_from_the_hex_across_gives_the_same_points() {
        let mut d = db(false);
        let e = HexEdge::of_cell(5, 5, 4);
        let p = pts(e, &d);
        d.submit(e, &p);
        let from_across = d.query(e.twin());
        assert_eq!(from_across.len(), 2);
        assert!(from_across[0].distance(p[0]) < 1e-3);
        assert!(from_across[1].distance(p[1]) < 1e-3);
    }

    #[test]
    fn the_other_direction_gets_the_points_reversed() {
        let mut d = db(false);
        let e = HexEdge::of_cell(5, 5, 0);
        let p = pts(e, &d);
        d.submit(e, &p);
        assert_eq!(d.len(), 2);
        let back = d.query(e.reversed());
        assert_eq!(back.len(), 2);
        assert!(back[0].distance(p[1]) < 1e-3);
        assert!(back[1].distance(p[0]) < 1e-3);
    }

    #[test]
    fn a_later_submit_replaces_the_entry() {
        let mut d = db(false);
        let e = HexEdge::of_cell(5, 5, 2);
        d.submit(e, &[Vec2::new(1.0, 1.0), Vec2::new(2.0, 2.0)]);
        d.submit(e, &[Vec2::new(5.0, 5.0), Vec2::new(6.0, 6.0)]);
        assert_eq!(d.len(), 2);
        assert!(d.query(e)[0].distance(Vec2::new(5.0, 5.0)) < 1e-3);
    }

    #[test]
    fn sides_at_the_top_and_bottom_of_the_map_are_not_saved() {
        let d = db(true);
        for code in 0..12u8 {
            assert!(
                !d.is_at_top_or_bottom(HexEdge::of_cell(5, 8, code)),
                "{code}"
            );
        }
        for row in [0, 15] {
            let rejected = (0..12u8)
                .filter(|c| d.is_at_top_or_bottom(HexEdge::of_cell(5, row, *c)))
                .count();
            assert!(rejected > 0 && rejected < 12, "row {row}: {rejected}");
        }
        let mut d = db(true);
        let e = (0..12u8)
            .map(|c| HexEdge::of_cell(5, 0, c))
            .find(|e| d.is_at_top_or_bottom(*e))
            .unwrap();
        d.submit(e, &[Vec2::ZERO, Vec2::ONE]);
        assert!(d.is_empty());
    }

    #[test]
    fn a_map_that_does_not_wrap_drops_the_sides_at_its_left_and_right() {
        let mut flat = db(false);
        let e = (0..12u8)
            .map(|c| HexEdge::of_cell(0, 8, c))
            .find(|e| flat.is_at_left_or_right(*e))
            .expect("a side leads off the map");
        flat.submit(e, &[Vec2::ZERO, Vec2::ONE]);
        assert!(flat.is_empty());
        // The same on a wrapping map is saved.
        let mut round = db(true);
        round.submit(e, &[Vec2::ZERO, Vec2::ONE]);
        assert_eq!(round.len(), 2);
    }

    #[test]
    fn a_segment_asked_for_unwrapped_comes_out_unwrapped() {
        let mut d = db(true);
        let e = HexEdge::of_cell(15, 5, 1);
        let p = pts(e, &d);
        d.submit(e, &p);
        // The hex one map to the left: the same entry, a map width (16 hexes of 64 texels) to
        // the left.
        let left = HexEdge::of_cell(-1, 5, 1);
        let q = d.query(left);
        assert_eq!(q.len(), 2);
        for (a, b) in q.iter().zip(p) {
            assert!(
                (*a - (b - Vec2::new(16.0 * 64.0, 0.0))).length() < 1e-2,
                "{a} {b}"
            );
        }
    }
}
