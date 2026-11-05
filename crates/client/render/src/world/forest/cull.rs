//! View culling of the native trees. A draw holds every tree of one model and style on the whole
//! map (the original draws the trees of the cells in view: `ForestVisSystem` keeps a vis list); with
//! the map revealed that is 280 000 trees a frame, 7 million triangles, whatever the camera looks at.
//!
//! The trees of a draw are put in blocks of [`CHUNK`] units, ordered as the original draws them: the
//! block rows from the top of the screen down, the blocks of a row from west to east, the trees of a
//! block by falling `y` ([`sort_back_to_front`]). Each block is a run of the instance buffer with its
//! bounds; a frame draws the runs whose bounds meet the view, neighbouring blocks in one call.

use glam::{Vec3, Vec4};

use super::draw::{TreeInstance, sort_back_to_front};

/// Side of a block, in the units of the original's basis (8 hexes).
pub const CHUNK: f32 = 8.0 * 64.0;
/// Room around the bounds of a block for what the vertex stage moves (the sway of the leaves).
const MARGIN: f32 = 16.0;

/// A run of the instance buffer and the box that holds its trees (the original's basis, z up).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Chunk {
    pub first: u32,
    pub count: u32,
    pub min: Vec3,
    pub max: Vec3,
}

/// The key of a block, in the order the blocks are drawn: the rows from the top (largest `y`) down,
/// then west to east.
pub type BlockKey = (std::cmp::Reverse<i32>, i32);

/// The block a tree stands in.
pub fn block_key(t: &TreeInstance) -> BlockKey {
    (
        std::cmp::Reverse((t.location[1] / CHUNK).floor() as i32),
        (t.location[0] / CHUNK).floor() as i32,
    )
}

/// The box that holds one tree with its reach and the margin of the sway.
fn tree_box(t: &TreeInstance, reach: (f32, f32)) -> (Vec3, Vec3) {
    let r = reach.0 * t.location[3].abs() + MARGIN;
    let h = reach.1 * t.location[3].abs() + MARGIN;
    (
        Vec3::new(t.location[0] - r, t.location[1] - r, t.location[2] - h),
        Vec3::new(t.location[0] + r, t.location[1] + r, t.location[2] + h),
    )
}

/// The box that holds all of `trees` (empty trees: an inverted box).
pub fn bounds_of(trees: &[TreeInstance], reach: (f32, f32)) -> (Vec3, Vec3) {
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for t in trees {
        let (a, b) = tree_box(t, reach);
        lo = lo.min(a);
        hi = hi.max(b);
    }
    (lo, hi)
}

/// `trees` in the order of the blocks and the blocks with their bounds. `reach` is how far a tree of
/// the model goes from its place at scale 1: across (x, y) and up (z).
pub fn chunk_trees(trees: &[TreeInstance], reach: (f32, f32)) -> (Vec<TreeInstance>, Vec<Chunk>) {
    let mut sorted = trees.to_vec();
    sort_back_to_front(&mut sorted);
    // Stable: the trees of a block stay by falling y. Rows from the top (largest y) down, then west
    // to east.
    sorted.sort_by_key(block_key);
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut last = None;
    for (i, t) in sorted.iter().enumerate() {
        let key = block_key(t);
        let (lo, hi) = tree_box(t, reach);
        if last == Some(key) {
            let c = chunks.last_mut().expect("a chunk is open");
            c.count += 1;
            c.min = c.min.min(lo);
            c.max = c.max.max(hi);
        } else {
            last = Some(key);
            chunks.push(Chunk {
                first: i as u32,
                count: 1,
                min: lo,
                max: hi,
            });
        }
    }
    (sorted, chunks)
}

/// True if the box is wholly outside one of the planes (normalised, pointing in).
pub fn outside(planes: &[Vec4; 6], min: Vec3, max: Vec3) -> bool {
    planes.iter().any(|p| {
        // The corner of the box that is furthest along the normal.
        let c = Vec3::new(
            if p.x >= 0.0 { max.x } else { min.x },
            if p.y >= 0.0 { max.y } else { min.y },
            if p.z >= 0.0 { max.z } else { min.z },
        );
        p.truncate().dot(c) + p.w < 0.0
    })
}

/// The runs `(first, count)` of the instance buffer that meet the view, in draw order; chunks that
/// follow each other are one run.
pub fn visible_runs(chunks: &[Chunk], planes: &[Vec4; 6]) -> Vec<(u32, u32)> {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for c in chunks {
        if outside(planes, c.min, c.max) {
            continue;
        }
        match runs.last_mut() {
            Some((first, count)) if *first + *count == c.first => *count += c.count,
            _ => runs.push((c.first, c.count)),
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use glam::Mat4;

    use super::*;
    use crate::world::terrain::draw::frustum_planes;

    /// A clip matrix with depth 0 to 1 over a box of the original's basis (z from -1000 to 1000).
    fn ortho(l: f32, r: f32, b: f32, t: f32, _near: f32, _far: f32) -> Mat4 {
        Mat4::from_cols(
            Vec4::new(2.0 / (r - l), 0.0, 0.0, 0.0),
            Vec4::new(0.0, 2.0 / (t - b), 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0 / 2000.0, 0.0),
            Vec4::new(-(r + l) / (r - l), -(t + b) / (t - b), 0.5, 1.0),
        )
    }

    fn tree(x: f32, y: f32) -> TreeInstance {
        TreeInstance {
            location: [x, y, 0.0, 1.0],
            variant: 0,
        }
    }

    #[test]
    fn blocks_go_from_the_top_row_down_and_hold_all_trees_back_to_front() {
        let trees = vec![
            tree(10.0, 10.0),
            tree(10.0, 5.0),
            tree(CHUNK + 10.0, 700.0),
            tree(20.0, 700.0),
            tree(30.0, 600.0),
            tree(-10.0, 20.0),
        ];
        let (sorted, chunks) = chunk_trees(&trees, (20.0, 60.0));
        assert_eq!(sorted.len(), trees.len());
        assert_eq!(chunks.iter().map(|c| c.count).sum::<u32>(), 6);
        // Row 1 (y 512..1024): the block of x 0..512 (y 700 then 600), then x 512..1024; row 0: the
        // block west of 0 (x -10), then the block of x 0..512 (y 10, 5).
        let at = |i: usize| (sorted[i].location[0], sorted[i].location[1]);
        assert_eq!(at(0), (20.0, 700.0));
        assert_eq!(at(1), (30.0, 600.0));
        assert_eq!(at(2), (CHUNK + 10.0, 700.0));
        assert_eq!(at(3), (-10.0, 20.0));
        assert_eq!(at(4), (10.0, 10.0));
        assert_eq!(at(5), (10.0, 5.0));
        assert_eq!(chunks.len(), 4);
        assert!(
            chunks
                .windows(2)
                .all(|w| w[0].first + w[0].count == w[1].first)
        );
        // The box holds the trees with their reach.
        let c = chunks[0];
        assert!(c.min.x <= 20.0 - 20.0 && c.max.x >= 30.0 + 20.0);
        assert!(c.max.z >= 60.0);
    }

    #[test]
    fn only_the_blocks_in_view_are_drawn_and_neighbours_make_one_run() {
        // 6 blocks in a row along x.
        let trees: Vec<TreeInstance> = (0..6)
            .map(|i| tree(i as f32 * CHUNK + 10.0, 10.0))
            .collect();
        let (_, chunks) = chunk_trees(&trees, (20.0, 60.0));
        assert_eq!(chunks.len(), 6);
        // A view over x 500..2100: blocks 1, 2, 3, 4 (x 512..2560) meet it; 0 (x up to 30+16+20) does
        // not, 5 (from x 2570) does not.
        let proj = ortho(500.0, 2100.0, -100.0, 100.0, -1000.0, 1000.0);
        let planes = frustum_planes(&proj);
        let runs = visible_runs(&chunks, &planes);
        assert_eq!(runs, vec![(1, 4)]);
        // All in view: one run of everything. None: nothing.
        let all = frustum_planes(&ortho(-100.0, 9000.0, -100.0, 100.0, -1000.0, 1000.0));
        assert_eq!(visible_runs(&chunks, &all), vec![(0, 6)]);
        let none = frustum_planes(&ortho(9000.0, 9100.0, -100.0, 100.0, -1000.0, 1000.0));
        assert!(visible_runs(&chunks, &none).is_empty());
        // Blocks apart in the buffer are two runs.
        let wide = frustum_planes(&ortho(-100.0, 1100.0, -100.0, 100.0, -1000.0, 1000.0));
        let mut c = chunks.clone();
        c.remove(1);
        assert_eq!(visible_runs(&c, &wide), vec![(0, 1), (2, 1)]);
    }
}
