//! The field of the terrain: the height (u16) and the terrain id (u8)
//! of every texel of the map, 64 texels per hex, in blocks of 16 x 16 texels as the paged grids of
//! `cvTerrainSampler`. A block is allocated at the first write; a block that was never written reads
//! as 0. The map of the original has `(W << 6) x (H << 6)` texels.

/// Texels per hex side.
// re: TerrainSystem::Init (TerrainSystem.cpp:683-684, `kGrid.m_iWidth << 6`)
pub const TEXELS_PER_HEX: u32 = 64;

/// Texels per side of a block of the height and id grids.
// re: cvTerrainSampler.cpp (kGridBlockSize)
pub const BLOCK_SIZE: u32 = 16;

/// Texels per side of a cell of the cache and of a blit job: one cell per hex.
// re: cvTerrainBlend.cpp:127-240 (a blit job per cell of 64 x 64)
pub const CELL_SIZE: u32 = 64;

const BLOCK_TEXELS: usize = (BLOCK_SIZE * BLOCK_SIZE) as usize;

/// A rectangle of texels, `[u0, u1) x [v0, v1)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CellRect {
    pub u0: i32,
    pub v0: i32,
    pub u1: i32,
    pub v1: i32,
}

impl CellRect {
    pub const fn new(u0: i32, v0: i32, u1: i32, v1: i32) -> Self {
        Self { u0, v0, u1, v1 }
    }

    /// Cell `(cx, cy)` of the grid of cells (64 x 64 texels).
    pub const fn cell(cx: i32, cy: i32) -> Self {
        let s = CELL_SIZE as i32;
        Self::new(cx * s, cy * s, (cx + 1) * s, (cy + 1) * s)
    }

    /// The same rectangle `n` texels larger on every side.
    pub const fn expand(self, n: i32) -> Self {
        Self::new(self.u0 - n, self.v0 - n, self.u1 + n, self.v1 + n)
    }

    pub fn contains(&self, u: i32, v: i32) -> bool {
        (self.u0..self.u1).contains(&u) && (self.v0..self.v1).contains(&v)
    }

    pub fn width(&self) -> i32 {
        (self.u1 - self.u0).max(0)
    }

    pub fn height(&self) -> i32 {
        (self.v1 - self.v0).max(0)
    }

    pub fn is_empty(&self) -> bool {
        self.width() == 0 || self.height() == 0
    }

    pub fn intersects(&self, other: &CellRect) -> bool {
        self.u0 < other.u1 && other.u0 < self.u1 && self.v0 < other.v1 && other.v0 < self.v1
    }

    /// Smallest rectangle that holds both.
    pub fn union(self, other: CellRect) -> CellRect {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        Self::new(
            self.u0.min(other.u0),
            self.v0.min(other.v0),
            self.u1.max(other.u1),
            self.v1.max(other.v1),
        )
    }

    /// The cells `(cx, cy)` of the 64 x 64 grid that the rectangle touches, row by row, clipped to
    /// `0..cells_x` and `0..cells_y` (`wrap_x`: the columns past the edge come back into the grid).
    pub fn cells(&self, cells_x: i32, cells_y: i32, wrap_x: bool) -> Vec<(i32, i32)> {
        if self.is_empty() || cells_x <= 0 || cells_y <= 0 {
            return Vec::new();
        }
        let s = CELL_SIZE as i32;
        let (c0, c1) = (self.u0.div_euclid(s), (self.u1 - 1).div_euclid(s));
        let (r0, r1) = (self.v0.div_euclid(s), (self.v1 - 1).div_euclid(s));
        let mut out = Vec::new();
        for cy in r0.max(0)..=r1.min(cells_y - 1) {
            let mut row: Vec<i32> = Vec::new();
            for cx in c0..=c1 {
                let x = if wrap_x {
                    cx.rem_euclid(cells_x)
                } else if (0..cells_x).contains(&cx) {
                    cx
                } else {
                    continue;
                };
                if !row.contains(&x) {
                    row.push(x);
                }
            }
            out.extend(row.into_iter().map(|x| (x, cy)));
        }
        out
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Block {
    height: [u16; BLOCK_TEXELS],
    id: [u8; BLOCK_TEXELS],
}

impl Block {
    fn new() -> Box<Self> {
        Box::new(Self {
            height: [0; BLOCK_TEXELS],
            id: [0; BLOCK_TEXELS],
        })
    }
}

/// The height and id grids of the map.
#[derive(Clone)]
pub struct TerrainField {
    width: u32,
    height: u32,
    blocks_x: u32,
    blocks_y: u32,
    blocks: Vec<Option<Box<Block>>>,
    allocated: usize,
    revision: u64,
    /// The revision of the last change of a height in each block (what a cache of something made from
    /// the heights of a few blocks checks).
    block_rev: Vec<u64>,
}

impl TerrainField {
    /// The field of a map of `hexes_x` x `hexes_y` hexes. Nothing is allocated.
    pub fn new(hexes_x: u32, hexes_y: u32) -> Self {
        Self::with_texels(hexes_x * TEXELS_PER_HEX, hexes_y * TEXELS_PER_HEX)
    }

    /// A field of `width` x `height` texels.
    pub fn with_texels(width: u32, height: u32) -> Self {
        let blocks_x = width.div_ceil(BLOCK_SIZE);
        let blocks_y = height.div_ceil(BLOCK_SIZE);
        Self {
            width,
            height,
            blocks_x,
            blocks_y,
            blocks: vec![None; (blocks_x * blocks_y) as usize],
            allocated: 0,
            revision: 0,
            block_rev: vec![0; (blocks_x * blocks_y) as usize],
        }
    }

    /// Texels along X.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Texels along Y.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Blocks along X and Y.
    pub fn blocks_dim(&self) -> (u32, u32) {
        (self.blocks_x, self.blocks_y)
    }

    pub fn contains(&self, u: i32, v: i32) -> bool {
        u >= 0 && v >= 0 && (u as u32) < self.width && (v as u32) < self.height
    }

    fn slot(&self, u: u32, v: u32) -> (usize, usize) {
        let block = ((v / BLOCK_SIZE) * self.blocks_x + u / BLOCK_SIZE) as usize;
        let texel = ((v % BLOCK_SIZE) * BLOCK_SIZE + u % BLOCK_SIZE) as usize;
        (block, texel)
    }

    /// Height of the texel; 0 outside the map and in a block never written.
    pub fn get_height(&self, u: i32, v: i32) -> u16 {
        if !self.contains(u, v) {
            return 0;
        }
        let (b, t) = self.slot(u as u32, v as u32);
        self.blocks[b].as_ref().map_or(0, |blk| blk.height[t])
    }

    /// Terrain id of the texel; 0 outside the map and in a block never written.
    pub fn get_id(&self, u: i32, v: i32) -> u8 {
        if !self.contains(u, v) {
            return 0;
        }
        let (b, t) = self.slot(u as u32, v as u32);
        self.blocks[b].as_ref().map_or(0, |blk| blk.id[t])
    }

    fn block_mut(&mut self, b: usize) -> &mut Block {
        if self.blocks[b].is_none() {
            self.blocks[b] = Some(Block::new());
            self.allocated += 1;
        }
        self.blocks[b].as_mut().expect("just allocated")
    }

    /// Writes the height of a texel (allocating its block). Returns whether the value changed;
    /// a texel outside the map is not written. The revision grows when the value changes.
    pub fn set_height(&mut self, u: i32, v: i32, raw: u16) -> bool {
        if !self.contains(u, v) {
            return false;
        }
        let (b, t) = self.slot(u as u32, v as u32);
        let blk = self.block_mut(b);
        let changed = blk.height[t] != raw;
        blk.height[t] = raw;
        if changed {
            self.revision += 1;
            self.block_rev[b] = self.revision;
        }
        changed
    }

    /// Writes the id of a texel (allocating its block).
    pub fn set_id(&mut self, u: i32, v: i32, id: u8) -> bool {
        if !self.contains(u, v) {
            return false;
        }
        let (b, t) = self.slot(u as u32, v as u32);
        let blk = self.block_mut(b);
        let changed = blk.id[t] != id;
        blk.id[t] = id;
        changed
    }

    /// Writes both.
    pub fn set(&mut self, u: i32, v: i32, raw: u16, id: u8) -> bool {
        let a = self.set_height(u, v, raw);
        let b = self.set_id(u, v, id);
        a || b
    }

    /// Whether the block that holds the texel was written.
    pub fn is_allocated(&self, u: i32, v: i32) -> bool {
        if !self.contains(u, v) {
            return false;
        }
        let (b, _) = self.slot(u as u32, v as u32);
        self.blocks[b].is_some()
    }

    /// Blocks written so far.
    pub fn blocks_allocated(&self) -> usize {
        self.allocated
    }

    /// Bytes the blocks hold.
    pub fn memory_bytes(&self) -> usize {
        self.allocated * BLOCK_TEXELS * 3
    }

    /// Grows at every change of a height (a piece, a decal with height, a rebuild).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Marks a change that the writes do not show (a block replaced as a whole).
    pub fn touch(&mut self) {
        self.revision += 1;
        let r = self.revision;
        self.block_rev.iter_mut().for_each(|b| *b = r);
    }

    /// Forgets everything that was written.
    pub fn clear(&mut self) {
        self.blocks.iter_mut().for_each(|b| *b = None);
        self.allocated = 0;
        self.revision += 1;
        let r = self.revision;
        self.block_rev.iter_mut().for_each(|b| *b = r);
    }

    /// The newest change of a height in the blocks of the texels `[u0, u1) x [v0, v1)` (clipped to the
    /// field): equal for two reads, the heights there are the same.
    pub fn heights_revision(&self, u0: i32, v0: i32, u1: i32, v1: i32) -> u64 {
        let (w, h) = (self.width as i32, self.height as i32);
        if w == 0 || h == 0 {
            return 0;
        }
        let bs = BLOCK_SIZE as i32;
        let (bx0, bx1) = (u0.clamp(0, w - 1) / bs, (u1 - 1).clamp(0, w - 1) / bs);
        let (by0, by1) = (v0.clamp(0, h - 1) / bs, (v1 - 1).clamp(0, h - 1) / bs);
        let mut newest = 0;
        for by in by0..=by1 {
            for bx in bx0..=bx1 {
                newest =
                    newest.max(self.block_rev[(by as u32 * self.blocks_x + bx as u32) as usize]);
            }
        }
        newest
    }

    /// FNV-1a over the blocks that were written (their place and their bytes), to compare two
    /// runs of a build.
    pub fn digest(&self) -> u64 {
        const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;
        let mut h = OFFSET;
        let mut feed = |byte: u8| {
            h ^= u64::from(byte);
            h = h.wrapping_mul(PRIME);
        };
        for (i, b) in self.blocks.iter().enumerate() {
            let Some(b) = b else { continue };
            (i as u32).to_le_bytes().into_iter().for_each(&mut feed);
            for v in &b.height {
                v.to_le_bytes().into_iter().for_each(&mut feed);
            }
            b.id.iter().copied().for_each(&mut feed);
        }
        h
    }

    /// Same size, same blocks written, same bytes (the revision is not compared).
    pub fn same_content(&self, other: &TerrainField) -> bool {
        self.width == other.width && self.height == other.height && self.blocks == other.blocks
    }
}

/// The blocks of a rectangle of the field copied out, written away from the field (on a worker of the
/// pool, B3 render-mt) and put back by [`TerrainField::apply_patch`]: the field then holds the bytes,
/// the blocks and the revisions that the same writes made on it directly would have left.
#[derive(Clone)]
pub struct FieldPatch {
    width: u32,
    height: u32,
    /// The blocks of the patch: `bx0..bx1` x `by0..by1` of the field.
    bx0: u32,
    by0: u32,
    bx1: u32,
    by1: u32,
    blocks: Vec<Option<Box<Block>>>,
    /// Height changes so far (what the revision of the field would have grown by).
    changes: u64,
    /// The change (`1..=changes`) that last wrote a height of each block; 0: none.
    last_change: Vec<u64>,
}

impl FieldPatch {
    fn local(&self, u: i32, v: i32) -> Option<(usize, usize)> {
        if u < 0 || v < 0 || (u as u32) >= self.width || (v as u32) >= self.height {
            return None;
        }
        let (bu, bv) = (u as u32 / BLOCK_SIZE, v as u32 / BLOCK_SIZE);
        if bu < self.bx0 || bu >= self.bx1 || bv < self.by0 || bv >= self.by1 {
            return None;
        }
        let b = ((bv - self.by0) * (self.bx1 - self.bx0) + (bu - self.bx0)) as usize;
        let t = ((v as u32 % BLOCK_SIZE) * BLOCK_SIZE + u as u32 % BLOCK_SIZE) as usize;
        Some((b, t))
    }

    /// Whether the texel is one of the patch's blocks.
    pub fn holds(&self, u: i32, v: i32) -> bool {
        self.local(u, v).is_some()
    }

    /// Height of a texel of the patch (0 in a block never written, or outside the patch).
    pub fn get_height(&self, u: i32, v: i32) -> u16 {
        self.local(u, v)
            .and_then(|(b, t)| self.blocks[b].as_ref().map(|blk| blk.height[t]))
            .unwrap_or(0)
    }

    pub fn get_id(&self, u: i32, v: i32) -> u8 {
        self.local(u, v)
            .and_then(|(b, t)| self.blocks[b].as_ref().map(|blk| blk.id[t]))
            .unwrap_or(0)
    }

    fn block_mut(&mut self, b: usize) -> &mut Block {
        self.blocks[b].get_or_insert_with(Block::new)
    }

    /// [`TerrainField::set_height`] on the patch; a texel outside it is not written.
    pub fn set_height(&mut self, u: i32, v: i32, raw: u16) -> bool {
        let Some((b, t)) = self.local(u, v) else {
            return false;
        };
        let blk = self.block_mut(b);
        let changed = blk.height[t] != raw;
        blk.height[t] = raw;
        if changed {
            self.changes += 1;
            self.last_change[b] = self.changes;
        }
        changed
    }

    pub fn set_id(&mut self, u: i32, v: i32, id: u8) -> bool {
        let Some((b, t)) = self.local(u, v) else {
            return false;
        };
        let blk = self.block_mut(b);
        let changed = blk.id[t] != id;
        blk.id[t] = id;
        changed
    }

    /// Writes both.
    pub fn set(&mut self, u: i32, v: i32, raw: u16, id: u8) -> bool {
        let a = self.set_height(u, v, raw);
        let b = self.set_id(u, v, id);
        a || b
    }
}

impl TerrainField {
    /// The blocks that hold the texels of `rect` (clipped to the field), copied out.
    pub fn patch_of(&self, rect: CellRect) -> FieldPatch {
        let (w, h) = (self.width as i32, self.height as i32);
        let bs = BLOCK_SIZE as i32;
        let (u0, v0) = (rect.u0.clamp(0, w), rect.v0.clamp(0, h));
        let (u1, v1) = (rect.u1.clamp(0, w), rect.v1.clamp(0, h));
        let (bx0, by0) = ((u0 / bs) as u32, (v0 / bs) as u32);
        let (bx1, by1) = if u1 > u0 && v1 > v0 {
            (
                (u1 + bs - 1) as u32 / BLOCK_SIZE,
                (v1 + bs - 1) as u32 / BLOCK_SIZE,
            )
        } else {
            (bx0, by0)
        };
        let mut blocks = Vec::with_capacity(((bx1 - bx0) * (by1 - by0)) as usize);
        for by in by0..by1 {
            for bx in bx0..bx1 {
                blocks.push(self.blocks[(by * self.blocks_x + bx) as usize].clone());
            }
        }
        let n = blocks.len();
        FieldPatch {
            width: self.width,
            height: self.height,
            bx0,
            by0,
            bx1,
            by1,
            blocks,
            changes: 0,
            last_change: vec![0; n],
        }
    }

    /// Puts the blocks of a patch made by [`Self::patch_of`] of this field back, with the revisions
    /// its writes would have given here (nothing else may have written those blocks in between).
    pub fn apply_patch(&mut self, patch: FieldPatch) {
        debug_assert_eq!((patch.width, patch.height), (self.width, self.height));
        let base = self.revision;
        let pw = patch.bx1 - patch.bx0;
        for (i, blk) in patch.blocks.into_iter().enumerate() {
            let (bx, by) = (patch.bx0 + i as u32 % pw, patch.by0 + i as u32 / pw);
            let b = (by * self.blocks_x + bx) as usize;
            if blk.is_some() && self.blocks[b].is_none() {
                self.allocated += 1;
            }
            if blk.is_some() {
                self.blocks[b] = blk;
            }
            if patch.last_change[i] > 0 {
                self.block_rev[b] = base + patch.last_change[i];
            }
        }
        self.revision = base + patch.changes;
    }
}

impl std::fmt::Debug for TerrainField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerrainField")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("blocks_allocated", &self.allocated)
            .field("revision", &self.revision)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_field_has_the_size_of_the_map_and_no_block() {
        let f = TerrainField::new(40, 24);
        assert_eq!((f.width(), f.height()), (2560, 1536));
        assert_eq!(f.blocks_dim(), (160, 96));
        assert_eq!(f.blocks_allocated(), 0);
        assert_eq!(f.memory_bytes(), 0);
    }

    #[test]
    fn unbuilt_texels_read_as_zero() {
        let f = TerrainField::new(2, 2);
        assert_eq!((f.get_height(5, 5), f.get_id(5, 5)), (0, 0));
        assert_eq!(f.get_height(-1, 0), 0);
        assert_eq!(f.get_height(0, 128), 0);
        assert!(!f.is_allocated(5, 5));
    }

    #[test]
    fn a_block_is_allocated_by_the_first_write_only() {
        let mut f = TerrainField::new(2, 2);
        assert!(f.set_height(17, 33, 1234));
        assert_eq!(f.blocks_allocated(), 1);
        assert!(f.is_allocated(17, 33));
        // The neighbours in the block are in the same page, the next block is not.
        assert!(f.is_allocated(16, 32));
        assert!(f.is_allocated(31, 47));
        assert!(!f.is_allocated(32, 33));
        assert!(!f.is_allocated(17, 48));
        assert_eq!(f.get_height(17, 33), 1234);
        // Reading never allocates.
        let _ = f.get_height(90, 90);
        assert_eq!(f.blocks_allocated(), 1);
        // Another texel of the same block takes no new block.
        f.set_id(20, 40, 7);
        assert_eq!(f.blocks_allocated(), 1);
        assert_eq!(f.get_id(20, 40), 7);
        assert_eq!(f.memory_bytes(), 256 * 3);
    }

    #[test]
    fn writing_outside_the_map_does_nothing() {
        let mut f = TerrainField::new(1, 1);
        assert!(!f.set_height(64, 0, 5));
        assert!(!f.set_id(0, -1, 5));
        assert_eq!(f.blocks_allocated(), 0);
        assert_eq!(f.revision(), 0);
    }

    #[test]
    fn the_revision_grows_when_a_height_changes() {
        let mut f = TerrainField::new(1, 1);
        assert_eq!(f.revision(), 0);
        f.set_height(1, 1, 10);
        assert_eq!(f.revision(), 1);
        // The same value again is no change; an id is not a height.
        f.set_height(1, 1, 10);
        f.set_id(1, 1, 3);
        assert_eq!(f.revision(), 1);
        f.set_height(1, 1, 11);
        assert_eq!(f.revision(), 2);
        f.touch();
        assert_eq!(f.revision(), 3);
    }

    #[test]
    fn two_equal_runs_give_the_same_bytes() {
        let run = || {
            let mut f = TerrainField::new(2, 2);
            for v in 0..40 {
                for u in 0..70 {
                    f.set(u, v, (u * 7 + v * 13) as u16, (u % 5) as u8);
                }
            }
            f
        };
        let (a, b) = (run(), run());
        assert!(a.same_content(&b));
        assert_eq!(a.digest(), b.digest());
        let mut c = run();
        c.set_height(3, 3, 9999);
        assert!(!a.same_content(&c));
        assert_ne!(a.digest(), c.digest());
    }

    #[test]
    fn writes_through_a_patch_leave_what_direct_writes_leave() {
        let seed = |f: &mut TerrainField| {
            for v in 60..70 {
                for u in 0..200 {
                    f.set(u, v, (u * 3 + v) as u16, (v % 7) as u8);
                }
            }
        };
        let rect = CellRect::cell(1, 1);
        let writes = |put: &mut dyn FnMut(i32, i32, u16, u8)| {
            for v in (rect.v0 - 3)..(rect.v1 + 3) {
                for u in (rect.u0 - 3)..(rect.u1 + 3) {
                    if rect.contains(u, v) && (u + v) % 3 != 0 {
                        put(u, v, ((u * 11 + v * 5) % 97) as u16, (u % 4) as u8);
                    }
                }
            }
        };
        let mut direct = TerrainField::new(4, 3);
        seed(&mut direct);
        let mut via = direct.clone();
        writes(&mut |u, v, h, id| {
            direct.set(u, v, h, id);
        });
        let mut patch = via.patch_of(rect);
        writes(&mut |u, v, h, id| {
            assert_eq!(patch.get_height(u, v), via.get_height(u, v));
            patch.set(u, v, h, id);
        });
        assert!(!patch.holds(rect.u1, rect.v0));
        via.apply_patch(patch);
        assert!(direct.same_content(&via));
        assert_eq!(direct.revision(), via.revision());
        assert_eq!(direct.blocks_allocated(), via.blocks_allocated());
        for by in 0..12 {
            for bx in 0..16 {
                let (u, v) = (bx * 16, by * 16);
                assert_eq!(
                    direct.heights_revision(u, v, u + 1, v + 1),
                    via.heights_revision(u, v, u + 1, v + 1),
                    "block {bx},{by}"
                );
            }
        }
    }

    #[test]
    fn a_written_zero_block_differs_from_an_unbuilt_one() {
        let mut a = TerrainField::new(1, 1);
        let b = TerrainField::new(1, 1);
        a.set_height(0, 0, 0);
        assert_eq!(a.blocks_allocated(), 1);
        assert!(!a.same_content(&b));
    }

    #[test]
    fn rect_cells_cover_the_touched_cells_and_wrap() {
        let r = CellRect::new(50, 60, 130, 64);
        assert_eq!(r.cells(4, 4, false), vec![(0, 0), (1, 0), (2, 0)]);
        // Across the west edge with wrapping: the cell -1 is the last column.
        let r = CellRect::new(-10, 0, 10, 10);
        assert_eq!(r.cells(4, 4, true), vec![(3, 0), (0, 0)]);
        assert_eq!(r.cells(4, 4, false), vec![(0, 0)]);
        // Rows outside the map are dropped.
        let r = CellRect::new(0, -70, 10, 10);
        assert_eq!(r.cells(4, 4, false), vec![(0, 0)]);
        assert!(CellRect::new(5, 5, 5, 9).cells(4, 4, false).is_empty());
    }

    #[test]
    fn rect_helpers() {
        let c = CellRect::cell(2, 3);
        assert_eq!(c, CellRect::new(128, 192, 192, 256));
        assert!(c.contains(128, 192) && !c.contains(192, 192));
        assert_eq!(c.expand(2), CellRect::new(126, 190, 194, 258));
        assert!(c.intersects(&CellRect::cell(2, 4).expand(1)));
        assert!(!c.intersects(&CellRect::cell(3, 3)));
        assert_eq!(
            c.union(CellRect::cell(3, 3)),
            CellRect::new(128, 192, 256, 256)
        );
    }
}
