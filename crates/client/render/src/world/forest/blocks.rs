//! The native trees by block of the map, kept per cell so that a change rebuilds only what it
//! touches.
//!
//! The draw holds the trees of one model and style in blocks of [`super::cull::CHUNK`] units, each
//! with its own instance buffer. Here every cell with trees keeps the trees it stands for (after the
//! ground, stencil and cut tests); a block lists the cells that put trees in it. When a cell is made
//! again only the blocks whose trees differ are marked, and [`Blocks::build`] makes the instances of
//! one block: the trees of its cells in the order of the cells, back to front (a stable sort), which
//! is what `chunk_trees` makes of the whole map.

use std::collections::{BTreeMap, BTreeSet};

use super::cull::{BlockKey, block_key};
use super::draw::{TreeInstance, sort_back_to_front};
use super::scatter::TreeKind;

/// One draw: the trees of a kind and a style.
pub type Group = (TreeKind, usize);
/// A block of a draw.
pub type BlockId = (Group, BlockKey);

/// The trees of a cell split by block, in the order the cell has them.
fn split(trees: &[TreeInstance]) -> BTreeMap<BlockKey, Vec<TreeInstance>> {
    let mut out: BTreeMap<BlockKey, Vec<TreeInstance>> = BTreeMap::new();
    for t in trees {
        out.entry(block_key(t)).or_default().push(*t);
    }
    out
}

#[derive(Default)]
pub struct Blocks {
    /// The trees each cell puts in the draw.
    cells: BTreeMap<u32, (Group, Vec<TreeInstance>)>,
    /// The cells with trees in each block.
    members: BTreeMap<BlockId, BTreeSet<u32>>,
    /// The blocks whose instances are not the ones the draw has.
    dirty: BTreeSet<BlockId>,
}

impl Blocks {
    /// Sets the trees of a cell (`None`: none). Marks the blocks whose trees differ from what the
    /// cell had; returns whether any does.
    pub fn set_cell(&mut self, index: u32, new: Option<(Group, Vec<TreeInstance>)>) -> bool {
        let new = new.filter(|(_, t)| !t.is_empty());
        let old = self.cells.get(&index);
        if old == new.as_ref() {
            return false;
        }
        let old_split: BTreeMap<BlockId, Vec<TreeInstance>> = old
            .map(|(g, t)| split(t).into_iter().map(|(k, v)| ((*g, k), v)).collect())
            .unwrap_or_default();
        let new_split: BTreeMap<BlockId, Vec<TreeInstance>> = new
            .as_ref()
            .map(|(g, t)| split(t).into_iter().map(|(k, v)| ((*g, k), v)).collect())
            .unwrap_or_default();
        for (id, trees) in &old_split {
            if new_split.get(id) == Some(trees) {
                continue;
            }
            self.dirty.insert(*id);
            if !new_split.contains_key(id)
                && let Some(m) = self.members.get_mut(id)
            {
                m.remove(&index);
            }
        }
        for (id, trees) in &new_split {
            if old_split.get(id) != Some(trees) {
                self.dirty.insert(*id);
            }
            self.members.entry(*id).or_default().insert(index);
        }
        match new {
            Some(n) => {
                self.cells.insert(index, n);
            }
            None => {
                self.cells.remove(&index);
            }
        }
        true
    }

    /// Every block the draw may hold is made again (the art or the basis changed).
    pub fn mark_all(&mut self) {
        self.dirty.extend(self.members.keys().copied());
    }

    pub fn has_cell(&self, index: u32) -> bool {
        self.cells.contains_key(&index)
    }

    /// The cells that have trees.
    pub fn cell_indices(&self) -> impl Iterator<Item = u32> + '_ {
        self.cells.keys().copied()
    }

    /// The blocks to make again.
    pub fn dirty(&self) -> impl Iterator<Item = &BlockId> {
        self.dirty.iter()
    }

    pub fn dirty_count(&self) -> usize {
        self.dirty.len()
    }

    /// The instances of a block: its cells in order, back to front. Empty if no cell has trees in it.
    pub fn build(&self, id: &BlockId) -> Vec<TreeInstance> {
        let mut out = Vec::new();
        if let Some(cells) = self.members.get(id) {
            for c in cells {
                if let Some((_, trees)) = self.cells.get(c) {
                    out.extend(trees.iter().filter(|t| block_key(t) == id.1).copied());
                }
            }
        }
        sort_back_to_front(&mut out);
        out
    }

    /// The draw has the block as [`Self::build`] made it.
    pub fn done(&mut self, id: &BlockId) {
        self.dirty.remove(id);
        if self.members.get(id).is_some_and(BTreeSet::is_empty) {
            self.members.remove(id);
        }
    }

    /// Every block with trees, in the order they are drawn within a group, for a check against the
    /// whole-map order.
    pub fn all(&self) -> Vec<(BlockId, Vec<TreeInstance>)> {
        self.members
            .keys()
            .filter_map(|id| {
                let b = self.build(id);
                (!b.is_empty()).then_some((*id, b))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::forest::cull::{CHUNK, chunk_trees};

    fn tree(x: f32, y: f32, v: u32) -> TreeInstance {
        TreeInstance {
            location: [x, y, 0.0, 1.0],
            variant: v,
        }
    }

    const G: Group = (TreeKind::Forest, 0);

    #[test]
    fn a_cell_that_changes_marks_only_the_blocks_it_changes() {
        let mut b = Blocks::default();
        let a = tree(10.0, 10.0, 0);
        let far = tree(CHUNK * 3.0 + 5.0, 10.0, 1);
        assert!(b.set_cell(1, Some((G, vec![a, far]))));
        assert_eq!(b.dirty_count(), 2);
        let ids: Vec<BlockId> = b.dirty().copied().collect();
        for id in &ids {
            b.done(id);
        }
        // The same trees: nothing is marked.
        assert!(!b.set_cell(1, Some((G, vec![a, far]))));
        assert_eq!(b.dirty_count(), 0);
        // One tree moves inside its block: that block only.
        assert!(b.set_cell(1, Some((G, vec![tree(11.0, 10.0, 0), far]))));
        assert_eq!(b.dirty_count(), 1);
    }

    /// Whole-map order against the blocks after a sequence of cell changes.
    #[test]
    fn blocks_after_changes_equal_the_whole_map_order() {
        let mut state: u32 = 12345;
        let mut rnd = move || {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            (state >> 8) as f32 / (1u32 << 24) as f32
        };
        let cell_trees = |rnd: &mut dyn FnMut() -> f32, n: usize, cell: u32| -> Vec<TreeInstance> {
            (0..n)
                .map(|i| TreeInstance {
                    location: [
                        (cell % 12) as f32 * 160.0 + rnd() * 160.0,
                        (cell / 12) as f32 * 140.0 + rnd() * 140.0,
                        1.0,
                        0.5 + rnd(),
                    ],
                    variant: i as u32,
                })
                .collect()
        };
        let mut b = Blocks::default();
        let mut truth: BTreeMap<u32, (Group, Vec<TreeInstance>)> = BTreeMap::new();
        for step in 0..400 {
            let cell = (rnd() * 96.0) as u32;
            let group: Group = if rnd() < 0.5 {
                (TreeKind::Forest, 0)
            } else {
                (TreeKind::Jungle, 1)
            };
            let new = if rnd() < 0.2 {
                None
            } else {
                let n = (rnd() * 12.0) as usize;
                Some((group, cell_trees(&mut rnd, n, cell)))
            };
            b.set_cell(cell, new.clone());
            match new {
                Some(n) if !n.1.is_empty() => {
                    truth.insert(cell, n);
                }
                _ => {
                    truth.remove(&cell);
                }
            }
            // The draw takes blocks in a few steps, not at every change.
            if step % 7 == 0 {
                let ids: Vec<BlockId> = b.dirty().copied().collect();
                for id in ids {
                    b.done(&id);
                }
            }
        }
        // The whole map: the trees of each group, cells in order, through `chunk_trees`.
        let mut want: BTreeMap<Group, Vec<TreeInstance>> = BTreeMap::new();
        for (g, t) in truth.values() {
            want.entry(*g).or_default().extend_from_slice(t);
        }
        let got = b.all();
        for (g, trees) in &want {
            let (sorted, chunks) = chunk_trees(trees, (20.0, 60.0));
            let mine: Vec<&(BlockId, Vec<TreeInstance>)> =
                got.iter().filter(|(id, _)| id.0 == *g).collect();
            assert_eq!(mine.len(), chunks.len(), "the same blocks");
            let flat: Vec<TreeInstance> =
                mine.iter().flat_map(|(_, t)| t.iter().copied()).collect();
            assert_eq!(flat, sorted, "the same trees in the same order");
            for ((_, t), c) in mine.iter().zip(&chunks) {
                assert_eq!(t.len() as u32, c.count);
            }
        }
        assert_eq!(
            got.iter().map(|(_, t)| t.len()).sum::<usize>(),
            want.values().map(Vec::len).sum::<usize>()
        );
    }
}
