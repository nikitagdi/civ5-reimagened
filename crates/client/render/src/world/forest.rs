//! `Forest` subsystem (world-objects WOB0): the cells that carry trees, by
//! cell index, and the tree instances of each.
//!
//! The trees of a cell come from the cell's kind and style, its relief and the lists of
//! `ForestTiles`, so they are placed once per cell and kept; `set_cells` places again only the
//! cells that changed (a fog change recolours cells, it does not move trees). B1's static
//! pipeline draws the batches (`B1Forest`); `world-objects` WOB3 replaces it with `ForestShader`.

use std::collections::BTreeMap;

use crate::frame::{
    DrawContext, InitContext, MainSlot, MaskContext, PassId, PassSlot, PrepareContext,
    RenderSystem, SystemId,
};
use crate::static_model::StaticInstance;
use crate::terrain::TerrainSnapshot;
pub mod batches;
pub mod blocks;
pub mod cull;
pub mod draw;
pub mod scatter;

use crate::world::objects::input::{Keyed, ObjectCounters};
use batches::{TreeBatch, TreeCell};
use scatter::{ForestTiles, TreeKind};

/// The shadow quads of the instances of a cell.
pub type ShadowFn<'a> = dyn Fn(&[StaticInstance]) -> Vec<[glam::Vec2; 4]> + 'a;

/// What the trees of a cell for the native draw are made from.
pub struct CellCx<'a> {
    pub to_orig: &'a glam::Mat4,
    pub gate: &'a dyn batches::ForestGate,
    pub ground: Option<&'a crate::world::terrain::sampler::TerrainSampler>,
    /// The variants of the model the art names for the cell's kind and style (0: no model, no trees).
    pub variants: &'a dyn Fn(&TreeCell) -> u32,
    /// The shadow quads of the instances of a cell; `None` leaves the shadows alone.
    pub shadow: Option<&'a ShadowFn<'a>>,
}

/// The trees of a cell as the native draw takes them: `{x, y, z, scale}` in the original's basis and
/// the variant of the model, for each tree the gate leaves, in the order of the cell.
pub fn cell_trees(
    instances: &[StaticInstance],
    variants: u32,
    to_orig: &glam::Mat4,
    gate: &dyn batches::ForestGate,
    ground: Option<&crate::world::terrain::sampler::TerrainSampler>,
) -> Vec<draw::TreeInstance> {
    let mut out = Vec::with_capacity(instances.len());
    for i in instances {
        let at = glam::Vec2::new(i.position[0], i.position[2]);
        let mut p = glam::Vec3::from(i.position);
        if let Some(s) = ground {
            // The capture S1a has trees at exactly the sea level (10.12), none below: the
            // height is the surface one, clamped to the sea.
            p.y = s.surface_height(at);
            // Nor on ground steeper than the limit (golden S2: the hill cells keep 245 to
            // 294 of their 351 trees).
            if s.world_normal_orig(at).z < batches::TREE_MIN_NORMAL_Z {
                continue;
            }
        }
        if !gate.allows(at, p.y) {
            continue;
        }
        let o = to_orig.transform_point3(p);
        out.push(draw::TreeInstance {
            location: [o.x, o.y, o.z, i.scale],
            variant: i.variant % variants,
        });
    }
    out
}

/// The cells with trees.
#[derive(Default)]
pub struct Forest {
    cells: Keyed<u32, TreeCell>,
    /// The instances of each cell with trees, placed at the last refresh.
    placed: BTreeMap<u32, Vec<StaticInstance>>,
    counters: ObjectCounters,
    /// The native draw through `ForestShader`; `None` until the renderer binds a device.
    gpu: Option<draw::ForestDraw>,
    /// The renderer has everything the native draw reads this frame (it takes B1's place).
    native: bool,
    /// The shadows of the trees of each cell as the decal layer last had them.
    shadows: BTreeMap<u32, Vec<[glam::Vec2; 4]>>,
    /// The native trees by block of the map.
    blocks: blocks::Blocks,
    /// The cells placed or lost since `take_touched`.
    touched: std::collections::BTreeSet<u32>,
    /// The cells to make again for the native draw.
    pending: std::collections::BTreeSet<u32>,
}

impl Forest {
    /// Sets the cells in `changed`: `Some` puts trees on the cell, `None` takes them away. True if
    /// anything changed.
    pub fn set_cells(&mut self, changed: &[(u32, Option<TreeCell>)]) -> bool {
        let mut any = false;
        for &(index, cell) in changed {
            any |= match cell {
                Some(c) => self.cells.upsert(index, c).1,
                None => self.cells.remove(&index).is_some(),
            };
            if cell.is_none() && self.placed.remove(&index).is_some() {
                self.touched.insert(index);
            }
        }
        any
    }

    /// Reads the cells off a terrain snapshot and sets what differs from the cells held; returns
    /// how many cells changed.
    pub fn sync_with(&mut self, snapshot: &TerrainSnapshot) -> usize {
        let width = snapshot.width() as usize;
        let mut changed: Vec<(u32, Option<TreeCell>)> = Vec::new();
        for (index, cell) in snapshot.cells().iter().enumerate() {
            let want = TreeCell::of(cell, index, width);
            if self.cells.get(&(index as u32)).copied() != want {
                changed.push((index as u32, want));
            }
        }
        // Cells the new map no longer has (a smaller map).
        let size = snapshot.cells().len() as u32;
        changed.extend(
            self.cells
                .keys()
                .filter(|&&i| i >= size)
                .map(|&i| (i, None)),
        );
        let n = changed.len();
        self.set_cells(&changed);
        n
    }

    /// Every cell is placed again at the next refresh (the lists changed).
    pub fn invalidate_all(&mut self) {
        self.cells.touch_all();
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Places the cells that changed; returns how many it placed.
    pub fn refresh(&mut self, tiles: &ForestTiles) -> usize {
        let dirty = self.cells.take_dirty();
        for index in &dirty {
            if let Some(cell) = self.cells.get(index) {
                self.placed.insert(*index, cell.instances(*index, tiles));
                self.touched.insert(*index);
            }
        }
        self.counters.placed_last = dirty.len() as u32;
        self.counters.placed_total += dirty.len() as u64;
        dirty.len()
    }

    /// The trees by kind and style, cells in index order: what `tree_batches` makes of a snapshot.
    pub fn batches(&mut self) -> Vec<TreeBatch> {
        let mut groups: BTreeMap<(TreeKind, usize), Vec<StaticInstance>> = BTreeMap::new();
        for (index, instances) in &self.placed {
            let Some(cell) = self.cells.get(index) else {
                continue;
            };
            groups
                .entry((cell.kind, cell.style))
                .or_default()
                .extend_from_slice(instances);
        }
        let batches: Vec<TreeBatch> = groups
            .into_iter()
            .map(|((kind, style), instances)| TreeBatch {
                kind,
                style,
                instances,
            })
            .collect();
        self.counters.instances = batches.iter().map(|b| b.instances.len() as u32).sum();
        self.counters.batches = batches.len() as u32;
        batches
    }

    /// The cell kinds and the instances of every placed cell, in index order, for the draw.
    pub fn placed(&self) -> impl Iterator<Item = (u32, &TreeCell, &[StaticInstance])> {
        self.placed
            .iter()
            .filter_map(|(i, v)| self.cells.get(i).map(|c| (*i, c, v.as_slice())))
    }

    /// The cells put in or taken out of `placed` since the last call.
    pub fn take_touched(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.touched).into_iter().collect()
    }

    /// Marks cells to be made again for the native draw (their ground, stencil or cuts changed).
    /// Only cells with trees count.
    pub fn mark_cells(&mut self, cells: impl IntoIterator<Item = u32>) {
        for c in cells {
            if self.placed.contains_key(&c) || self.blocks.has_cell(c) {
                self.pending.insert(c);
            }
        }
    }

    /// Every cell with trees is made again; the blocks follow only where their trees differ.
    pub fn mark_every_cell(&mut self) {
        let all: Vec<u32> = self
            .placed
            .keys()
            .copied()
            .chain(self.blocks.cell_indices())
            .collect();
        self.pending.extend(all);
    }

    /// Every cell is made again, and every block of the draw (the art or the basis changed).
    pub fn mark_all(&mut self) {
        let all: Vec<u32> = self
            .placed
            .keys()
            .copied()
            .chain(self.blocks.cell_indices())
            .collect();
        self.pending.extend(all);
        self.blocks.mark_all();
    }

    /// Cells waiting to be made again and blocks waiting to be uploaded.
    pub fn pending_cells(&self) -> usize {
        self.pending.len()
    }

    pub fn pending_iter(&self) -> impl Iterator<Item = u32> + '_ {
        self.pending.iter().copied()
    }

    pub fn dirty_ids(&self) -> impl Iterator<Item = &blocks::BlockId> {
        self.blocks.dirty()
    }

    pub fn dirty_blocks(&self) -> usize {
        self.blocks.dirty_count()
    }

    /// The cells to make again, the nearest first by `rank`.
    pub fn pending_by(&self, rank: &dyn Fn(u32) -> f32) -> Vec<u32> {
        let mut v: Vec<(f32, u32)> = self.pending.iter().map(|&c| (rank(c), c)).collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        v.into_iter().map(|(_, c)| c).collect()
    }

    /// The blocks to upload, the nearest first by `rank`.
    pub fn dirty_blocks_by(&self, rank: &dyn Fn(&blocks::BlockId) -> f32) -> Vec<blocks::BlockId> {
        let mut v: Vec<(f32, blocks::BlockId)> =
            self.blocks.dirty().map(|id| (rank(id), *id)).collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        v.into_iter().map(|(_, id)| id).collect()
    }

    /// The instances of a block of the native draw, for the upload.
    pub fn build_block(&self, id: &blocks::BlockId) -> Vec<draw::TreeInstance> {
        self.blocks.build(id)
    }

    /// The draw has the block.
    pub fn block_done(&mut self, id: &blocks::BlockId) {
        self.blocks.done(id);
    }

    /// Makes the trees of one cell again for the native draw: the cell's trees after the ground,
    /// stencil and cut tests, in the original's basis (the block structure takes them from here), and
    /// its shadows. Returns the shadow change of the cell, if it has one.
    ///
    /// With the sampler of the terrain a tree stands on the height of the field under it, and a tree
    /// whose ground is below the sea is left out (the ground test of `ForestVis::BuildBuffers`, per
    /// tree: the tile of a coast cell reaches over the water).
    pub fn update_cell(
        &mut self,
        index: u32,
        cx: &CellCx<'_>,
    ) -> Option<(u32, Vec<[glam::Vec2; 4]>)> {
        self.pending.remove(&index);
        let instances = self.placed.get(&index);
        let drawn = match (self.cells.get(&index), instances) {
            (Some(cell), Some(instances)) => {
                let variants = (cx.variants)(cell);
                (variants > 0).then(|| {
                    (
                        (cell.kind, cell.style),
                        cell_trees(instances, variants, cx.to_orig, cx.gate, cx.ground),
                    )
                })
            }
            _ => None,
        };
        let quads = match (cx.shadow, instances) {
            (Some(f), Some(instances)) => Some(f(instances)),
            (Some(_), None) => Some(Vec::new()),
            _ => None,
        };
        self.blocks.set_cell(index, drawn);
        let q = quads?;
        let old = self.shadows.get(&index);
        if q.is_empty() {
            old?;
            self.shadows.remove(&index);
            Some((index, q))
        } else if old != Some(&q) {
            self.shadows.insert(index, q.clone());
            Some((index, q))
        } else {
            None
        }
    }

    /// The shadows of the trees of the placed cells (`ForestVisSystem::CreateTreeShadows`), made by
    /// `quads` for each cell: the cells whose shadows changed since the last call, with their new
    /// quads (empty for a cell that lost its trees or its shadows). The caller gives them to the
    /// decal layer, which bakes again only the pages under them.
    pub fn shadow_changes(
        &mut self,
        quads: impl Fn(&[StaticInstance]) -> Vec<[glam::Vec2; 4]>,
    ) -> Vec<(u32, Vec<[glam::Vec2; 4]>)> {
        let mut now: BTreeMap<u32, Vec<[glam::Vec2; 4]>> = BTreeMap::new();
        for (index, _, instances) in self.placed() {
            let q = quads(instances);
            if !q.is_empty() {
                now.insert(index, q);
            }
        }
        let mut changed: Vec<(u32, Vec<[glam::Vec2; 4]>)> = self
            .shadows
            .keys()
            .filter(|i| !now.contains_key(i))
            .map(|&i| (i, Vec::new()))
            .collect();
        changed.extend(
            now.iter()
                .filter(|(i, q)| self.shadows.get(i) != Some(q))
                .map(|(i, q)| (*i, q.clone())),
        );
        changed.sort_by_key(|(i, _)| *i);
        self.shadows = now;
        changed
    }

    /// Makes the native draw (once, when the device exists).
    pub fn bind_device(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, samples: u32) {
        self.gpu = Some(draw::ForestDraw::new(device, queue, samples));
    }

    pub fn gpu(&self) -> Option<&draw::ForestDraw> {
        self.gpu.as_ref()
    }

    pub fn gpu_mut(&mut self) -> Option<&mut draw::ForestDraw> {
        self.gpu.as_mut()
    }

    /// The renderer says whether the native draw replaces B1's trees in this frame.
    pub fn set_native(&mut self, on: bool) {
        self.native = on;
    }

    pub fn is_native(&self) -> bool {
        self.native
    }

    pub fn counters(&self) -> ObjectCounters {
        ObjectCounters {
            records: self.cells.len() as u32,
            ..self.counters
        }
    }
}

impl RenderSystem for Forest {
    fn id(&self) -> SystemId {
        SystemId::Forest
    }

    fn passes(&self) -> &'static [PassSlot] {
        &[
            PassSlot::Pass(PassId::ShadowMap),
            PassSlot::Pass(PassId::Reflection),
            PassSlot::Main(MainSlot::Forest),
        ]
    }

    fn init(&mut self, _cx: &mut InitContext<'_>) {}

    fn prepare(&mut self, _cx: &mut PrepareContext<'_>) {}

    fn record_mask(&self, slot: PassSlot, cx: &mut MaskContext<'_, '_>) {
        if !self.native || slot != PassSlot::Main(MainSlot::Forest) {
            return;
        }
        if let Some(gpu) = &self.gpu {
            gpu.record_mask(cx.pass, cx.copy_shift);
        }
    }

    fn record(&self, slot: PassSlot, cx: &mut DrawContext<'_, '_>) {
        if !self.native {
            return;
        }
        // The reflection pass: the draw has the reflection camera's matrix in its constants.
        if slot == PassSlot::Pass(PassId::Reflection) {
            if let Some(gpu) = &self.gpu {
                gpu.record_reflection(cx.pass, cx.copy_shift, cx.stats);
            }
            return;
        }
        if slot != PassSlot::Main(MainSlot::Forest) {
            return;
        }
        if let Some(gpu) = &self.gpu {
            gpu.record(cx.pass, cx.copy_shift, cx.stats);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::batches::tree_batches;
    use super::*;
    use crate::terrain::test_util::{land, map};
    use crate::terrain::{FeatureKind, Fog, Relief, TerrainCell};
    use std::sync::OnceLock;

    fn tiles() -> &'static ForestTiles {
        static T: OnceLock<ForestTiles> = OnceLock::new();
        T.get_or_init(ForestTiles::default)
    }

    fn world(fog_of_forest: Fog) -> TerrainSnapshot {
        map(6, 5, |x, y| match (x, y) {
            (1, 1) | (2, 1) | (4, 3) => TerrainCell {
                feat: FeatureKind::Forest,
                fog: fog_of_forest,
                ..land()
            },
            (3, 3) => TerrainCell {
                feat: FeatureKind::Jungle,
                ..land()
            },
            (0, 4) => TerrainCell {
                feat: FeatureKind::Forest,
                relief: Relief::Mountain,
                ..land()
            },
            _ => land(),
        })
    }

    /// The shadows go to the decal layer for the cells that changed only, and a lost cell is cleared.
    #[test]
    fn the_shadows_change_with_the_cells_only() {
        let quad = |i: &[StaticInstance]| -> Vec<[glam::Vec2; 4]> {
            i.iter()
                .take(1)
                .map(|t| [glam::Vec2::new(t.position[0], t.position[2]); 4])
                .collect()
        };
        let mut forest = Forest::default();
        forest.sync_with(&world(Fog::Visible));
        forest.refresh(tiles());
        let first = forest.shadow_changes(quad);
        assert_eq!(first.len(), 4, "every cell with trees");
        assert!(forest.shadow_changes(quad).is_empty(), "nothing changed");
        forest.sync_with(&map(6, 5, |x, y| match (x, y) {
            (1, 1) | (2, 1) | (4, 3) => TerrainCell {
                feat: FeatureKind::Forest,
                ..land()
            },
            _ => land(),
        }));
        forest.refresh(tiles());
        let lost = forest.shadow_changes(quad);
        assert_eq!(lost.len(), 1, "the jungle is gone");
        assert!(lost[0].1.is_empty());
    }

    /// Closes the trees whose world x is in a band.
    struct BandGate(std::cell::Cell<(f32, f32)>);

    impl batches::ForestGate for BandGate {
        fn allows(&self, p: glam::Vec2, _height: f32) -> bool {
            let (a, b) = self.0.get();
            !(a..b).contains(&p.x)
        }
    }

    /// The draw of the whole map: every placed cell through `cell_trees`, the trees of each draw
    /// through `chunk_trees`.
    fn whole_map(
        forest: &Forest,
        gate: &dyn batches::ForestGate,
        to_orig: &glam::Mat4,
    ) -> BTreeMap<blocks::Group, Vec<draw::TreeInstance>> {
        let mut groups: BTreeMap<blocks::Group, Vec<draw::TreeInstance>> = BTreeMap::new();
        for (_, cell, instances) in forest.placed() {
            groups
                .entry((cell.kind, cell.style))
                .or_default()
                .extend(cell_trees(instances, 3, to_orig, gate, None));
        }
        groups
            .into_iter()
            .map(|(g, t)| (g, cull::chunk_trees(&t, (20.0, 60.0)).0))
            .filter(|(_, t)| !t.is_empty())
            .collect()
    }

    /// The draw as the blocks hold it, each draw's blocks in order.
    fn from_blocks(forest: &Forest) -> BTreeMap<blocks::Group, Vec<draw::TreeInstance>> {
        let mut out: BTreeMap<blocks::Group, Vec<draw::TreeInstance>> = BTreeMap::new();
        for (id, trees) in forest.blocks.all() {
            out.entry(id.0).or_default().extend(trees);
        }
        out
    }

    /// Making only the cells that changed gives the same trees, in the same order, as making the whole
    /// map again, through a sequence of changes of the ground, of the cells and of the basis.
    #[test]
    fn making_the_changed_cells_again_equals_making_the_whole_map() {
        let to_orig = crate::frame::basis_rotation();
        let gate = BandGate(std::cell::Cell::new((0.0, 0.0)));
        let variants = |_: &batches::TreeCell| 3;
        let drain = |forest: &mut Forest, to_orig: &glam::Mat4, gate: &BandGate| {
            let cx = CellCx {
                to_orig,
                gate,
                ground: None,
                variants: &variants,
                shadow: None,
            };
            for i in forest.pending_by(&|c| c as f32) {
                forest.update_cell(i, &cx);
            }
            // The uploads: every block that changed is built.
            for id in forest.dirty_blocks_by(&|_| 0.0) {
                let _ = forest.build_block(&id);
                forest.block_done(&id);
            }
        };
        let big = |cut: &[(i32, i32)]| {
            map(14, 10, |x, y| {
                if cut.contains(&(x, y)) {
                    return land();
                }
                match (x * 7 + y * 3) % 5 {
                    0 | 1 => TerrainCell {
                        feat: FeatureKind::Forest,
                        ..land()
                    },
                    2 => TerrainCell {
                        feat: FeatureKind::Jungle,
                        ..land()
                    },
                    _ => land(),
                }
            })
        };
        let mut forest = Forest::default();
        forest.sync_with(&big(&[]));
        forest.refresh(tiles());
        forest.mark_all();
        drain(&mut forest, &to_orig, &gate);
        assert_eq!(
            from_blocks(&forest),
            whole_map(&forest, &gate, &to_orig),
            "the first make"
        );
        assert!(!from_blocks(&forest).is_empty());

        // The ground changes under a band of the map: the cells near it are made again.
        let mut before = (0.0, 0.0);
        for (a, b) in [(100.0, 260.0), (300.0, 420.0), (0.0, 0.0)] {
            gate.0.set((a, b));
            // The cells near the band before and after.
            let near: Vec<u32> = (0..140u32)
                .filter(|&i| {
                    let x = crate::hexgrid::cell_center((i % 14) as i32, (i / 14) as i32).x;
                    let near = |(p, q): (f32, f32)| x + 100.0 > p && x - 100.0 < q;
                    near((a, b)) || near(before)
                })
                .collect();
            before = (a, b);
            forest.mark_cells(near);
            drain(&mut forest, &to_orig, &gate);
            assert_eq!(
                from_blocks(&forest),
                whole_map(&forest, &gate, &to_orig),
                "band {a}..{b}"
            );
        }

        // Cells lose and gain trees.
        gate.0.set((200.0, 330.0));
        forest.mark_all();
        drain(&mut forest, &to_orig, &gate);
        forest.sync_with(&big(&[(1, 1), (2, 1), (5, 5), (9, 8)]));
        forest.refresh(tiles());
        let touched = forest.take_touched();
        assert!(!touched.is_empty());
        forest.mark_cells(touched);
        drain(&mut forest, &to_orig, &gate);
        assert_eq!(
            from_blocks(&forest),
            whole_map(&forest, &gate, &to_orig),
            "cells cut"
        );
        forest.sync_with(&big(&[(2, 1)]));
        forest.refresh(tiles());
        let touched = forest.take_touched();
        forest.mark_cells(touched);
        drain(&mut forest, &to_orig, &gate);
        assert_eq!(
            from_blocks(&forest),
            whole_map(&forest, &gate, &to_orig),
            "cells grown back"
        );

        // Another basis: everything again.
        let moved = glam::Mat4::from_translation(glam::Vec3::new(900.0, -300.0, 0.0)) * to_orig;
        forest.mark_all();
        drain(&mut forest, &moved, &gate);
        assert_eq!(
            from_blocks(&forest),
            whole_map(&forest, &gate, &moved),
            "another basis"
        );
    }

    /// A change of one cell marks the blocks that cell is in and no others.
    #[test]
    fn one_changed_cell_dirties_only_its_blocks() {
        let to_orig = crate::frame::basis_rotation();
        let gate = BandGate(std::cell::Cell::new((0.0, 0.0)));
        let variants = |_: &batches::TreeCell| 3;
        let mut forest = Forest::default();
        forest.sync_with(&map(30, 20, |x, y| {
            if (x + y) % 3 == 0 {
                TerrainCell {
                    feat: FeatureKind::Forest,
                    ..land()
                }
            } else {
                land()
            }
        }));
        forest.refresh(tiles());
        forest.mark_all();
        let run = |forest: &mut Forest, gate: &BandGate| {
            let cx = CellCx {
                to_orig: &to_orig,
                gate,
                ground: None,
                variants: &variants,
                shadow: None,
            };
            for i in forest.pending_by(&|c| c as f32) {
                forest.update_cell(i, &cx);
            }
        };
        run(&mut forest, &gate);
        let total = forest.dirty_blocks();
        assert!(total > 4, "the map is in many blocks: {total}");
        for id in forest.dirty_blocks_by(&|_| 0.0) {
            forest.block_done(&id);
        }
        // The ground changes under one cell only (a band the width of a hex).
        let x = crate::hexgrid::cell_center(15, 9).x;
        gate.0.set((x - 20.0, x + 20.0));
        forest.mark_cells([9 * 30 + 15]);
        run(&mut forest, &gate);
        let dirty = forest.dirty_blocks();
        assert!((1..=4).contains(&dirty), "{dirty} of {total} blocks");
    }

    /// WOB0: the kept cells give the batches `tree_batches` makes of the snapshot.
    #[test]
    fn the_cells_make_the_batches_of_the_snapshot() {
        let snapshot = world(Fog::Visible);
        let mut forest = Forest::default();
        assert_eq!(forest.sync_with(&snapshot), 4, "three forests and a jungle");
        assert_eq!(forest.refresh(tiles()), 4);
        assert_eq!(forest.batches(), tree_batches(&snapshot, tiles()));
        assert_eq!(forest.counters().records, 4);
        assert_eq!(forest.counters().batches, 2);
    }

    /// A change of fog leaves the cells that stay as they are: nothing is placed again; a cell
    /// that is lost or gained is the only one placed.
    #[test]
    fn a_change_of_one_cell_places_that_cell_only() {
        let mut forest = Forest::default();
        forest.sync_with(&world(Fog::Visible));
        forest.refresh(tiles());
        let before = forest.batches();

        assert_eq!(forest.sync_with(&world(Fog::Visible)), 0);
        assert_eq!(forest.refresh(tiles()), 0);
        assert_eq!(forest.batches(), before);

        // The forests turn unrevealed: they keep their trees, under the clouds (NP-D19).
        let hidden = world(Fog::Unrevealed);
        assert_eq!(forest.sync_with(&hidden), 0);
        assert_eq!(forest.batches(), before);

        // Two forests are cut: they lose their trees, nothing is placed.
        let cut = map(6, 5, |x, y| match (x, y) {
            (4, 3) => TerrainCell {
                feat: FeatureKind::Forest,
                ..land()
            },
            (3, 3) => TerrainCell {
                feat: FeatureKind::Jungle,
                ..land()
            },
            (0, 4) => TerrainCell {
                feat: FeatureKind::Forest,
                relief: Relief::Mountain,
                ..land()
            },
            _ => land(),
        });
        assert_eq!(forest.sync_with(&cut), 2);
        assert_eq!(
            forest.refresh(tiles()),
            0,
            "nothing to place for a lost cell"
        );
        assert_eq!(forest.batches(), tree_batches(&cut, tiles()));

        // One forest grows back: that one cell is placed.
        let back = map(6, 5, |x, y| match (x, y) {
            (4, 3) | (1, 1) => TerrainCell {
                feat: FeatureKind::Forest,
                ..land()
            },
            (3, 3) => TerrainCell {
                feat: FeatureKind::Jungle,
                ..land()
            },
            _ => land(),
        });
        assert_eq!(forest.sync_with(&back), 1);
        assert_eq!(forest.refresh(tiles()), 1);
        assert_eq!(forest.batches(), tree_batches(&back, tiles()));
    }
}
