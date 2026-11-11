//! The transform and clip tables: one `Rgba32Float` texture of 1024 x 64 texels (1 MiB) that the vertex shader
//! reads with `textureLoad`.
//!
//! - Rows 0-15 hold transform records, two texels each: `(a, b, c, d)` and `(tx, ty, visible, 0)`, so that
//!   `x' = a x + c y + tx` and `y' = b x + d y + ty` (glam's column order: `x_axis = (a, b)`, `y_axis = (c, d)`).
//!   Record `r` sits in row `r / 512` at texel `2 * (r % 512)`. Record 0 is the identity. Records 1-2047 (rows 0-3)
//!   are anchors ([`AnchorSlot`]); records 2048-8191 are element transforms, shared by equal matrices.
//!   `visible` is 1 for everything but a hidden anchor, which the shader collapses to zero area.
//! - Rows 16-63 hold clips, eight texels each: the rectangle `(x, y, w, h)`; `(radius, n_faces, 0, 0)`; then six
//!   texels with the face coefficients `a, b, c` of up to 8 polygon faces, face `i` at floats `3i..3i+3` of those
//!   24. A face is `a x + b y + c >= 0` inside, with `(a, b)` a unit inward normal. Clip `k` (1 to 6143; 0 means
//!   none) sits in row `16 + k / 128` at texel `8 * (k % 128)`.
//!
//! The scene writes the texels on the CPU and records which texel range of each row changed; the pass uploads
//! exactly those segments ([`Tables::take_dirty`]).

use std::collections::HashMap;

use glam::{Affine2, Vec2};

use crate::{AnchorSlot, AnchorXform, DrawError, Rect};

/// Texels per row.
pub const WIDTH: u32 = 1024;
/// Rows of the texture.
pub const HEIGHT: u32 = 64;
/// First row of the clips.
pub const CLIP_FIRST_ROW: u32 = 16;
/// Transform records per row.
pub const RECORDS_PER_ROW: u32 = WIDTH / 2;
/// Records reserved for the identity (0) and the anchors (1..).
pub const ANCHOR_RECORDS: u16 = 2048;
/// Total transform records.
pub const RECORD_COUNT: u32 = 16 * RECORDS_PER_ROW;
/// Clips per row.
pub const CLIPS_PER_ROW: u32 = WIDTH / 8;
/// Total clip slots (slot 0 is "no clip").
pub const CLIP_COUNT: u32 = 48 * CLIPS_PER_ROW;
/// Most faces of a clip polygon.
pub const MAX_FACES: usize = 8;
/// Bytes per texel.
pub const TEXEL_BYTES: u32 = 16;

const IDENTITY_TEXELS: [[f32; 4]; 2] = [[1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 0.0]];
/// Large enough to never clip, small enough to keep f32 distances exact to a pixel.
const UNBOUNDED: f32 = 1.0e7;

/// A convex polygon of 3 to 8 vertices in chunk space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Polygon {
    n: u8,
    pts: [Vec2; MAX_FACES],
}

impl Polygon {
    /// Validates `points` (any winding) as a convex polygon with 3 to 8 vertices and non-zero area.
    pub fn new(points: &[Vec2]) -> Result<Self, DrawError> {
        let n = points.len();
        if !(3..=MAX_FACES).contains(&n) {
            return Err(DrawError::InvalidPolygon("needs 3 to 8 vertices"));
        }
        if points.iter().any(|p| !p.is_finite()) {
            return Err(DrawError::InvalidPolygon("non-finite vertex"));
        }
        let mut pts = [Vec2::ZERO; MAX_FACES];
        pts[..n].copy_from_slice(points);
        let area2 = signed_area2(points);
        if area2 == 0.0 {
            return Err(DrawError::InvalidPolygon("zero area"));
        }
        let sign = area2.signum();
        for i in 0..n {
            let (a, b, c) = (points[i], points[(i + 1) % n], points[(i + 2) % n]);
            let cross = (b - a).perp_dot(c - b);
            if cross * sign < 0.0 {
                return Err(DrawError::InvalidPolygon("not convex"));
            }
        }
        Ok(Self { n: n as u8, pts })
    }

    /// The vertices.
    pub fn points(&self) -> &[Vec2] {
        &self.pts[..usize::from(self.n)]
    }

    /// The faces `(a, b, c)`: `a x + b y + c >= 0` inside, `(a, b)` a unit inward normal.
    fn faces(&self) -> Vec<[f32; 3]> {
        let pts = self.points();
        let n = pts.len();
        let flip = signed_area2(pts) < 0.0;
        (0..n)
            .map(|i| {
                let (mut p, mut q) = (pts[i], pts[(i + 1) % n]);
                if flip {
                    std::mem::swap(&mut p, &mut q);
                }
                let e = q - p;
                let len = e.length();
                // Left normal of the edge in a positive-area polygon points inside.
                let nrm = Vec2::new(-e.y, e.x) / len;
                [nrm.x, nrm.y, -nrm.dot(p)]
            })
            .collect()
    }
}

/// Twice the signed area (positive when the vertices run counter-clockwise in a y-up system).
fn signed_area2(p: &[Vec2]) -> f32 {
    let n = p.len();
    (0..n).map(|i| p[i].perp_dot(p[(i + 1) % n])).sum()
}

/// A clip: the intersection of an optional rectangle (with a corner radius) and an optional convex polygon, in
/// chunk space. A polygon is one per item (nested rectangles are intersected by the runtime).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Clip {
    /// The clip rectangle (`None`: unbounded).
    pub rect: Option<Rect>,
    /// Corner radius of the rectangle, device px.
    pub radius: f32,
    /// The polygon.
    pub polygon: Option<Polygon>,
}

impl Clip {
    /// A rectangle clip.
    pub fn rect(rect: Rect) -> Self {
        Self {
            rect: Some(rect),
            ..Self::default()
        }
    }

    /// A rounded rectangle clip.
    pub fn rounded(rect: Rect, radius: f32) -> Self {
        Self {
            rect: Some(rect),
            radius,
            polygon: None,
        }
    }

    /// A polygon clip.
    pub fn polygon(points: &[Vec2]) -> Result<Self, DrawError> {
        Ok(Self {
            polygon: Some(Polygon::new(points)?),
            ..Self::default()
        })
    }

    /// True if the clip restricts nothing.
    pub fn is_empty(&self) -> bool {
        self.rect.is_none() && self.polygon.is_none()
    }

    /// The 8 texels of the clip record.
    pub fn texels(&self) -> [[f32; 4]; 8] {
        let mut t = [[0.0f32; 4]; 8];
        let r = self.rect.unwrap_or(Rect::new(
            -UNBOUNDED,
            -UNBOUNDED,
            2.0 * UNBOUNDED,
            2.0 * UNBOUNDED,
        ));
        t[0] = [r.x, r.y, r.w, r.h];
        let faces = self.polygon.map(|p| p.faces()).unwrap_or_default();
        t[1] = [self.radius.max(0.0), faces.len() as f32, 0.0, 0.0];
        for (i, f) in faces.iter().enumerate() {
            for (j, v) in f.iter().enumerate() {
                let flat = 3 * i + j;
                t[2 + flat / 4][flat % 4] = *v;
            }
        }
        t
    }
}

/// Reference-counted slots addressed by index, shared by equal keys.
struct Interner {
    first: u32,
    slots: Vec<Option<(Box<[u32]>, u32)>>,
    free: Vec<u32>,
    map: HashMap<Box<[u32]>, u32>,
    limit: u32,
}

impl Interner {
    fn new(first: u32, limit: u32) -> Self {
        Self {
            first,
            slots: Vec::new(),
            free: Vec::new(),
            map: HashMap::new(),
            limit,
        }
    }

    /// Returns `(index, is_new)`, or `None` if the table is full.
    fn acquire(&mut self, key: &[u32]) -> Option<(u32, bool)> {
        if let Some(&idx) = self.map.get(key) {
            if let Some(Some((_, refs))) = self.slots.get_mut((idx - self.first) as usize) {
                *refs += 1;
            }
            return Some((idx, false));
        }
        let idx = match self.free.pop() {
            Some(i) => i,
            None => {
                let i = self.first + self.slots.len() as u32;
                if i >= self.limit {
                    return None;
                }
                self.slots.push(None);
                i
            }
        };
        let key: Box<[u32]> = key.into();
        self.slots[(idx - self.first) as usize] = Some((key.clone(), 1));
        self.map.insert(key, idx);
        Some((idx, true))
    }

    fn release(&mut self, idx: u32) {
        let Some(slot) = idx
            .checked_sub(self.first)
            .and_then(|i| self.slots.get_mut(i as usize))
        else {
            return;
        };
        let Some((key, refs)) = slot else { return };
        *refs -= 1;
        if *refs == 0 {
            self.map.remove(&**key);
            *slot = None;
            self.free.push(idx);
        }
    }

    fn live(&self) -> usize {
        self.map.len()
    }
}

/// A run of texels of one row that changed since the last [`Tables::take_dirty`].
#[derive(Debug, Clone, PartialEq)]
pub struct TableSegment {
    /// Row of the texture.
    pub row: u32,
    /// First texel of the segment in the row.
    pub x: u32,
    /// The texels (`Rgba32Float`).
    pub texels: Vec<[f32; 4]>,
}

impl TableSegment {
    /// The segment as bytes for a texture upload.
    pub fn bytes(&self) -> &[u8] {
        bytemuck::cast_slice(&self.texels)
    }
}

/// The CPU copy of the table texture, the slot allocators and the dirty ranges.
pub struct Tables {
    texels: Vec<[f32; 4]>,
    xforms: Interner,
    clips: Interner,
    anchors_next: u16,
    anchors_free: Vec<u16>,
    dirty: [Option<(u32, u32)>; HEIGHT as usize],
}

impl Default for Tables {
    fn default() -> Self {
        Self::new()
    }
}

impl Tables {
    /// Tables with the identity in record 0 (reported dirty, so the first upload carries it).
    pub fn new() -> Self {
        let mut t = Self {
            texels: vec![[0.0; 4]; (WIDTH * HEIGHT) as usize],
            xforms: Interner::new(u32::from(ANCHOR_RECORDS), RECORD_COUNT),
            clips: Interner::new(1, CLIP_COUNT),
            anchors_next: 1,
            anchors_free: Vec::new(),
            dirty: [None; HEIGHT as usize],
        };
        t.write(0, 0, &IDENTITY_TEXELS);
        t
    }

    fn write(&mut self, row: u32, x: u32, texels: &[[f32; 4]]) {
        let start = (row * WIDTH + x) as usize;
        self.texels[start..start + texels.len()].copy_from_slice(texels);
        let end = x + texels.len() as u32;
        let d = &mut self.dirty[row as usize];
        *d = Some(match *d {
            Some((lo, hi)) => (lo.min(x), hi.max(end)),
            None => (x, end),
        });
    }

    fn write_record(&mut self, record: u32, texels: [[f32; 4]; 2]) {
        self.write(
            record / RECORDS_PER_ROW,
            2 * (record % RECORDS_PER_ROW),
            &texels,
        );
    }

    /// The texels of a transform record.
    pub fn record(&self, record: u32) -> [[f32; 4]; 2] {
        let start = ((record / RECORDS_PER_ROW) * WIDTH + 2 * (record % RECORDS_PER_ROW)) as usize;
        [self.texels[start], self.texels[start + 1]]
    }

    /// The texels of a clip record.
    pub fn clip_record(&self, clip: u16) -> [[f32; 4]; 8] {
        let k = u32::from(clip);
        let start =
            ((CLIP_FIRST_ROW + k / CLIPS_PER_ROW) * WIDTH + 8 * (k % CLIPS_PER_ROW)) as usize;
        let mut out = [[0.0; 4]; 8];
        out.copy_from_slice(&self.texels[start..start + 8]);
        out
    }

    /// The record of an element transform (equal matrices share one record, with a reference count). `None`
    /// when the table is full. The identity needs no record: use index 0.
    pub fn intern_xform(&mut self, m: &Affine2) -> Option<u16> {
        let c = m.to_cols_array();
        let texels = [[c[0], c[1], c[2], c[3]], [c[4], c[5], 1.0, 0.0]];
        let key: Vec<u32> = texels.iter().flatten().map(|v| v.to_bits()).collect();
        let (idx, new) = self.xforms.acquire(&key)?;
        if new {
            self.write_record(idx, texels);
        }
        Some(idx as u16)
    }

    /// Drops one reference of an element transform record.
    pub fn release_xform(&mut self, record: u16) {
        self.xforms.release(u32::from(record));
    }

    /// The slot of a clip (shared by equal clips, reference counted). `None` when the table is full.
    pub fn intern_clip(&mut self, clip: &Clip) -> Option<u16> {
        let texels = clip.texels();
        let key: Vec<u32> = texels.iter().flatten().map(|v| v.to_bits()).collect();
        let (idx, new) = self.clips.acquire(&key)?;
        if new {
            let k = idx;
            self.write(
                CLIP_FIRST_ROW + k / CLIPS_PER_ROW,
                8 * (k % CLIPS_PER_ROW),
                &texels,
            );
        }
        Some(idx as u16)
    }

    /// Drops one reference of a clip.
    pub fn release_clip(&mut self, clip: u16) {
        self.clips.release(u32::from(clip));
    }

    /// Element transform records and clips in use.
    pub fn live(&self) -> (usize, usize) {
        (self.xforms.live(), self.clips.live())
    }

    /// Takes a free anchor slot (hidden until [`Tables::set_anchor`]).
    pub fn alloc_anchor(&mut self) -> Result<AnchorSlot, DrawError> {
        let slot = match self.anchors_free.pop() {
            Some(s) => s,
            None if self.anchors_next < ANCHOR_RECORDS => {
                let s = self.anchors_next;
                self.anchors_next += 1;
                s
            }
            None => return Err(DrawError::AnchorsExhausted),
        };
        Ok(AnchorSlot(slot))
    }

    /// Returns a slot; it is hidden at once.
    pub fn free_anchor(&mut self, slot: AnchorSlot) {
        if slot.0 == 0 || slot.0 >= ANCHOR_RECORDS || self.anchors_free.contains(&slot.0) {
            return;
        }
        self.write_record(u32::from(slot.0), [[0.0; 4]; 2]);
        self.anchors_free.push(slot.0);
    }

    /// Writes the anchor record: 2 texels (32 bytes).
    pub fn set_anchor(&mut self, slot: AnchorSlot, a: AnchorXform) {
        if slot.0 == 0 || slot.0 >= ANCHOR_RECORDS {
            return;
        }
        let vis = if a.visible { 1.0 } else { 0.0 };
        self.write_record(
            u32::from(slot.0),
            [[a.scale, 0.0, 0.0, a.scale], [a.pos.x, a.pos.y, vis, 0.0]],
        );
    }

    /// True if [`Tables::take_dirty`] would return something.
    pub fn has_dirty(&self) -> bool {
        self.dirty.iter().any(Option::is_some)
    }

    /// The changed texel ranges, one segment per changed row, and forgets them.
    pub fn take_dirty(&mut self) -> Vec<TableSegment> {
        let mut out = Vec::new();
        for (row, d) in self.dirty.iter_mut().enumerate() {
            if let Some((lo, hi)) = d.take() {
                let base = row * WIDTH as usize;
                out.push(TableSegment {
                    row: row as u32,
                    x: lo,
                    texels: self.texels[base + lo as usize..base + hi as usize].to_vec(),
                });
            }
        }
        out
    }

    /// All texels, row-major (`WIDTH * HEIGHT`), for a full upload.
    pub fn all_texels(&self) -> &[[f32; 4]] {
        &self.texels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg_bytes(t: &mut Tables) -> usize {
        t.take_dirty().iter().map(|s| s.bytes().len()).sum()
    }

    #[test]
    fn new_tables_report_only_the_identity_dirty() {
        let mut t = Tables::new();
        let segs = t.take_dirty();
        assert_eq!(segs.len(), 1);
        assert_eq!((segs[0].row, segs[0].x), (0, 0));
        assert_eq!(segs[0].texels, IDENTITY_TEXELS.to_vec());
        assert!(!t.has_dirty());
    }

    #[test]
    fn anchors_write_only_their_own_texels() {
        let mut t = Tables::new();
        t.take_dirty();
        let slots: Vec<_> = (0..200).map(|_| t.alloc_anchor().unwrap()).collect();
        assert_eq!(slots[0], AnchorSlot(1));
        assert_eq!(slots[199], AnchorSlot(200));
        assert!(!t.has_dirty());
        for (i, s) in slots.iter().enumerate() {
            t.set_anchor(
                *s,
                AnchorXform {
                    pos: Vec2::new(i as f32, 2.0 * i as f32),
                    scale: 0.5,
                    visible: i % 2 == 0,
                },
            );
        }
        let segs = t.take_dirty();
        let bytes: usize = segs.iter().map(|s| s.bytes().len()).sum();
        assert_eq!(bytes, 200 * 32);
        assert!(segs.iter().all(|s| s.row < 4));
        // Record 201 (slot 200 sits at row 0, texel 400) carries scale, position, visibility.
        assert_eq!(
            t.record(200),
            [[0.5, 0.0, 0.0, 0.5], [199.0, 398.0, 0.0, 0.0]]
        );
        assert_eq!(t.record(1)[1], [0.0, 0.0, 1.0, 0.0]);
        // Moving one anchor uploads exactly 32 bytes.
        t.set_anchor(
            slots[7],
            AnchorXform {
                pos: Vec2::new(5.0, 6.0),
                scale: 1.0,
                visible: true,
            },
        );
        assert_eq!(seg_bytes(&mut t), 32);
        assert_eq!(seg_bytes(&mut t), 0);
    }

    #[test]
    fn anchor_slots_are_recycled_and_exhausted() {
        let mut t = Tables::new();
        let all: Vec<_> = (1..ANCHOR_RECORDS)
            .map(|_| t.alloc_anchor().unwrap())
            .collect();
        assert_eq!(t.alloc_anchor(), Err(DrawError::AnchorsExhausted));
        t.free_anchor(all[10]);
        t.free_anchor(all[10]);
        assert_eq!(t.alloc_anchor(), Ok(all[10]));
        assert_eq!(t.alloc_anchor(), Err(DrawError::AnchorsExhausted));
        // A freed anchor is hidden.
        t.free_anchor(all[3]);
        assert_eq!(t.record(u32::from(all[3].0)), [[0.0; 4]; 2]);
    }

    #[test]
    fn equal_transforms_share_a_record() {
        let mut t = Tables::new();
        t.take_dirty();
        let m = Affine2::from_angle_translation(0.5, Vec2::new(3.0, 4.0));
        let a = t.intern_xform(&m).unwrap();
        let b = t.intern_xform(&m).unwrap();
        assert_eq!(a, b);
        assert_eq!(a, ANCHOR_RECORDS);
        let c = t
            .intern_xform(&Affine2::from_translation(Vec2::new(1.0, 0.0)))
            .unwrap();
        assert_eq!(c, ANCHOR_RECORDS + 1);
        // 2048 / 512 = row 4.
        let segs = t.take_dirty();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].row, 4);
        assert_eq!(segs[0].x, 0);
        assert_eq!(segs[0].texels.len(), 4);
        let cols = m.to_cols_array();
        assert_eq!(
            t.record(u32::from(a)),
            [
                [cols[0], cols[1], cols[2], cols[3]],
                [cols[4], cols[5], 1.0, 0.0]
            ]
        );
        t.release_xform(a);
        assert_eq!(t.live().0, 2);
        t.release_xform(b);
        assert_eq!(t.live().0, 1);
        // The freed record is reused.
        let d = t
            .intern_xform(&Affine2::from_translation(Vec2::new(9.0, 9.0)))
            .unwrap();
        assert_eq!(d, a);
    }

    #[test]
    fn clip_rect_record_layout() {
        let mut t = Tables::new();
        t.take_dirty();
        let c = Clip::rounded(Rect::new(1.0, 2.0, 30.0, 40.0), 6.0);
        let k = t.intern_clip(&c).unwrap();
        assert_eq!(k, 1);
        let r = t.clip_record(k);
        assert_eq!(r[0], [1.0, 2.0, 30.0, 40.0]);
        assert_eq!(r[1], [6.0, 0.0, 0.0, 0.0]);
        assert!(r[2..].iter().all(|x| *x == [0.0; 4]));
        let segs = t.take_dirty();
        assert_eq!((segs[0].row, segs[0].x, segs[0].texels.len()), (16, 8, 8));
        assert_eq!(t.intern_clip(&c), Some(1));
    }

    #[test]
    fn clip_polygon_faces_point_inward_for_both_windings() {
        let square = [
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 10.0),
            Vec2::new(0.0, 10.0),
        ];
        let mut rev = square;
        rev.reverse();
        let inside = Vec2::new(4.0, 6.0);
        let outside = Vec2::new(12.0, 5.0);
        for pts in [square, rev] {
            let t = Clip::polygon(&pts).unwrap().texels();
            assert_eq!(t[1][1], 4.0);
            let faces: Vec<[f32; 3]> = (0..4)
                .map(|i| {
                    let f = 3 * i;
                    [
                        t[2 + f / 4][f % 4],
                        t[2 + (f + 1) / 4][(f + 1) % 4],
                        t[2 + (f + 2) / 4][(f + 2) % 4],
                    ]
                })
                .collect();
            let d = |p: Vec2| {
                faces
                    .iter()
                    .map(|f| f[0] * p.x + f[1] * p.y + f[2])
                    .fold(f32::INFINITY, f32::min)
            };
            assert!((d(inside) - 4.0).abs() < 1e-5, "{pts:?}");
            assert!(d(outside) < 0.0);
        }
    }

    #[test]
    fn clip_polygon_validation() {
        let v = |x, y| Vec2::new(x, y);
        assert!(Clip::polygon(&[v(0., 0.), v(1., 0.)]).is_err());
        assert!(Clip::polygon(&[v(0., 0.), v(1., 1.), v(2., 2.)]).is_err());
        // An arrow head (concave).
        assert!(Clip::polygon(&[v(0., 0.), v(4., 2.), v(0., 4.), v(1., 2.)]).is_err());
        let nine: Vec<Vec2> = (0..9)
            .map(|i| Vec2::from_angle(i as f32 * std::f32::consts::TAU / 9.0) * 10.0)
            .collect();
        assert!(Clip::polygon(&nine).is_err());
        assert!(Clip::polygon(&nine[..8]).is_ok());
        assert!(Clip::polygon(&[v(f32::NAN, 0.), v(1., 0.), v(0., 1.)]).is_err());
    }

    #[test]
    fn clip_table_reports_exhaustion() {
        let mut t = Tables::new();
        for i in 1..CLIP_COUNT {
            let c = Clip::rect(Rect::new(i as f32, 0.0, 1.0, 1.0));
            assert_eq!(t.intern_clip(&c), Some(i as u16));
        }
        assert_eq!(
            t.intern_clip(&Clip::rect(Rect::new(-1.0, 0.0, 1.0, 1.0))),
            None
        );
    }
}
