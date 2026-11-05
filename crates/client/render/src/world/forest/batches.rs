//! The trees of the cells as the draws take them (world-objects spec 4.6, WOB3; RW-16
//! `ForestVis::BuildBuffers`): the instances of the `static` fallback by kind and style, and the
//! `ForestShader` draws of 90 trees, one `treeLocation` register `{x, y, z, scale}` each.
//!
//! A tree is put out of the draw (its scale would be 0 in the original) when the stencil of the
//! terrain has the forest bits set at its place ([`ForestGate`]; the bits of the layout are written
//! by `world-water`, stage `CoastStencil`). A type 3 or 4 tree (the forest and the jungle on a hill)
//! is drawn at 0.75. Water and mountain cells carry no trees at all; on the others the ground test of the
//! original (below the water level or steeper than 0.9) is made per tree by `Forest::draw_specs`.

use std::collections::BTreeMap;

use glam::Vec2;

use super::scatter::{ForestTiles, TreeKind};
use crate::hexgrid::cell_center;
use crate::static_model::StaticInstance;
use crate::terrain::height::ground_height;
use crate::terrain::texture_array::art_style;
use crate::terrain::{Relief, TerrainSnapshot};

/// Trees in one draw of `ForestShader` (`treeLocation[90]`).
pub const TREES_PER_DRAW: usize = 90;
/// Scale of the trees on a hill (types 3 and 4: the forest and the jungle on a hill) against types
/// 1 and 2. Golden S2: the trees of the six hill cells have scales 0.375 to 0.75, the flat ones
/// 0.5 to 1.0.
// re: ForestVis::BuildBuffers 0x0866ff5e (RW-16: x1.0 for types 1 and 2, x0.75 for types 3 and 4)
pub const HILL_SCALE: f32 = 0.75;
/// A tree stands on ground no steeper than this (the z of the normal, z up), else its scale is 0.
// re: ForestVis::BuildBuffers 0x0866ff5e (the ground test: below the water level or steeper than 0.9)
pub const TREE_MIN_NORMAL_Z: f32 = 0.9;
/// Size of a model unit in world units: the models are 6 to 9 units tall, a hex is 64 wide.
// SIMPLIFIED(SIMP-009): the scale constant of `ForestVS` comes from the capture S2 (RW-16 leaves
// it open); one model unit is one world unit until then.
pub const TREE_SCALE: f32 = 1.0;
/// Lift above the ground so the trunk base does not z-fight with the terrain.
const GROUND_LIFT: f32 = -0.3;

/// What the terrain says about a place for a tree.
pub trait ForestGate {
    /// False where the original would force the scale of a tree standing at `p` (world `(X, Z)`),
    /// at ground height `height`, to 0.
    fn allows(&self, p: Vec2, height: f32) -> bool;
}

/// Everything is open (no stencil yet).
pub struct OpenGate;

impl ForestGate for OpenGate {
    fn allows(&self, _p: Vec2, _height: f32) -> bool {
        true
    }
}

/// The stencil of the terrain read at the tree: closed where any bit of `mask` is set.
pub struct StencilGate<'a> {
    pub stencil: &'a crate::world::terrain::stencil::TerrainStencil,
    pub frame: &'a crate::world::terrain::sampler::TerrainFrame,
    pub mask: u8,
}

impl ForestGate for StencilGate<'_> {
    fn allows(&self, p: Vec2, _height: f32) -> bool {
        self.stencil.sample_world(self.frame, p, self.mask) == 0
    }
}

/// The ground the `Tree Cut` meshes of the landmarks cover (the `LandmarkShader_Stencil` material:
/// the line around the fruit trees of a citrus, the deer of a camp, the stone of a quarry), as
/// triangles in world `(X, Z)`. The original writes them into the stencil the trees read, so a tree
/// standing on them has its scale forced to 0 (golden S1a: the citrus plot is a clearing in the
/// jungle, its fruit trees in the open).
// SIMPLIFIED(SIMP-065): the cuts are triangles tested on the CPU, not a stencil the trees read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TreeCuts {
    /// Each triangle with its bounding box (min, max); a slot is empty after its triangle was taken
    /// out (see [`TreeCuts::remove`]).
    tris: Vec<Option<([Vec2; 3], Vec2, Vec2)>>,
    /// The empty slots of `tris`.
    free: Vec<u32>,
    /// The triangles by square of [`CUT_CELL`] units that their box meets: a tree is tested against the
    /// ones of its square (the full map has thousands of cuts and a quarter of a million trees, and
    /// the scan of every triangle for every tree took seconds).
    grid: std::collections::HashMap<(i32, i32), Vec<u32>>,
    /// The triangles that span too many squares for the index.
    wide: Vec<u32>,
}

/// Side of a square of the index of the cuts, in world units (a hex is 64 wide).
const CUT_CELL: f32 = 64.0;

fn cut_square(p: Vec2) -> (i32, i32) {
    (
        (p.x / CUT_CELL).floor() as i32,
        (p.y / CUT_CELL).floor() as i32,
    )
}

impl TreeCuts {
    pub fn new(tris: impl IntoIterator<Item = [Vec2; 3]>) -> Self {
        let mut cuts = Self::default();
        for t in tris {
            cuts.add(t);
        }
        cuts
    }

    /// The squares a triangle with this box is listed in; `None` for a wide one.
    fn squares(lo: Vec2, hi: Vec2) -> Option<impl Iterator<Item = (i32, i32)>> {
        let (x0, y0) = cut_square(lo);
        let (x1, y1) = cut_square(hi);
        // A triangle that spans many squares (a broken mesh) is tested for every tree.
        if i64::from(x1 - x0 + 1) * i64::from(y1 - y0 + 1) > 64 {
            return None;
        }
        Some((y0..=y1).flat_map(move |y| (x0..=x1).map(move |x| (x, y))))
    }

    /// Puts a triangle in the index.
    pub fn add(&mut self, t: [Vec2; 3]) {
        let (lo, hi) = (t[0].min(t[1]).min(t[2]), t[0].max(t[1]).max(t[2]));
        let entry = Some((t, lo, hi));
        let i = match self.free.pop() {
            Some(i) => {
                self.tris[i as usize] = entry;
                i
            }
            None => {
                self.tris.push(entry);
                (self.tris.len() - 1) as u32
            }
        };
        match Self::squares(lo, hi) {
            None => self.wide.push(i),
            Some(squares) => {
                for s in squares {
                    self.grid.entry(s).or_default().push(i);
                }
            }
        }
    }

    /// Takes one triangle equal to `t` out of the index; returns whether there was one.
    pub fn remove(&mut self, t: &[Vec2; 3]) -> bool {
        let (lo, hi) = (t[0].min(t[1]).min(t[2]), t[0].max(t[1]).max(t[2]));
        let squares = Self::squares(lo, hi);
        let found = {
            let list: &[u32] = match &squares {
                None => &self.wide,
                Some(_) => self
                    .grid
                    .get(&cut_square(lo))
                    .map_or(&[][..], Vec::as_slice),
            };
            list.iter().copied().find(|&i| {
                self.tris[i as usize]
                    .as_ref()
                    .is_some_and(|(u, _, _)| u == t)
            })
        };
        let Some(i) = found else {
            return false;
        };
        match squares {
            None => self.wide.retain(|&j| j != i),
            Some(squares) => {
                for s in squares {
                    if let Some(l) = self.grid.get_mut(&s) {
                        l.retain(|&j| j != i);
                        if l.is_empty() {
                            self.grid.remove(&s);
                        }
                    }
                }
            }
        }
        self.tris[i as usize] = None;
        self.free.push(i);
        true
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The boxes (world `(X, Z)`) of the triangles that one of the two has and the other has not: the
    /// ground whose trees may differ between the two.
    pub fn changed_boxes(&self, other: &TreeCuts) -> Vec<(Vec2, Vec2)> {
        let key = |t: &[Vec2; 3]| t.map(|p| [p.x.to_bits(), p.y.to_bits()]);
        let live = |c: &TreeCuts| c.tris.iter().flatten().cloned().collect::<Vec<_>>();
        let (a, b) = (live(self), live(other));
        let mine: std::collections::HashSet<_> = a.iter().map(|(t, _, _)| key(t)).collect();
        let theirs: std::collections::HashSet<_> = b.iter().map(|(t, _, _)| key(t)).collect();
        a.iter()
            .filter(|(t, _, _)| !theirs.contains(&key(t)))
            .chain(b.iter().filter(|(t, _, _)| !mine.contains(&key(t))))
            .map(|&(_, lo, hi)| (lo, hi))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.tris.len() - self.free.len()
    }

    /// True if `p` lies on a cut (edges included).
    pub fn covers(&self, p: Vec2) -> bool {
        let near = self.grid.get(&cut_square(p)).map_or(&[][..], Vec::as_slice);
        near.iter().chain(&self.wide).any(|&i| {
            let Some((t, lo, hi)) = &self.tris[i as usize] else {
                return false;
            };
            p.x >= lo.x && p.y >= lo.y && p.x <= hi.x && p.y <= hi.y && in_triangle(p, t)
        })
    }
}

fn in_triangle(p: Vec2, t: &[Vec2; 3]) -> bool {
    let side = |a: Vec2, b: Vec2| (b - a).perp_dot(p - a);
    let (d0, d1, d2) = (side(t[0], t[1]), side(t[1], t[2]), side(t[2], t[0]));
    let neg = d0 < 0.0 || d1 < 0.0 || d2 < 0.0;
    let pos = d0 > 0.0 || d1 > 0.0 || d2 > 0.0;
    !(neg && pos)
}

/// A gate with the cuts of the landmarks over it: closed on a cut, else as `inner`.
pub struct CutGate<'a> {
    pub inner: &'a dyn ForestGate,
    pub cuts: &'a TreeCuts,
}

impl ForestGate for CutGate<'_> {
    fn allows(&self, p: Vec2, height: f32) -> bool {
        !self.cuts.covers(p) && self.inner.allows(p, height)
    }
}

/// Instances of one tree model: all cells of one kind and continent style.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeBatch {
    pub kind: TreeKind,
    /// Continent style 0..5 (`art_style`).
    pub style: usize,
    /// `variant` is the raw number, see [`super::scatter::Tree::variant`].
    pub instances: Vec<StaticInstance>,
}

/// Tree instances for every forest or jungle cell on land, grouped by kind and style (the art table
/// has one tree model per kind and style). Unrevealed cells have their trees too, under the clouds, as
/// in the original (S1a draws 14040 trees over the whole view; the server gives the whole world,
/// NP-D19, 2026-10-22); the shader darkens the hidden ones with the fog texture.
pub fn tree_batches(snapshot: &TerrainSnapshot, tiles: &ForestTiles) -> Vec<TreeBatch> {
    let width = snapshot.width() as usize;
    let mut groups: BTreeMap<(TreeKind, usize), Vec<StaticInstance>> = BTreeMap::new();
    for (index, cell) in snapshot.cells().iter().enumerate() {
        let Some(tree_cell) = TreeCell::of(cell, index, width) else {
            continue;
        };
        groups
            .entry((tree_cell.kind, tree_cell.style))
            .or_default()
            .extend(tree_cell.instances(index as u32, tiles));
    }
    groups
        .into_iter()
        .map(|((kind, style), instances)| TreeBatch {
            kind,
            style,
            instances,
        })
        .collect()
}

/// A cell that carries trees: what `Forest` keeps per cell (WOB0, `set_cells`). The trees of a cell
/// are a function of this and of the lists (`ForestTiles`) only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeCell {
    pub kind: TreeKind,
    /// Continent style 0..5 (`art_style`).
    pub style: usize,
    pub relief: Relief,
    pub x: i32,
    pub y: i32,
}

impl TreeCell {
    /// The tree cell of `cell` at `index` of a map `width` wide: a forest or jungle cell on land,
    /// revealed or not (the fog is the render's, NP-D19); `None` for any other.
    pub fn of(cell: &crate::terrain::TerrainCell, index: usize, width: usize) -> Option<Self> {
        let kind = TreeKind::from_feature(cell.feat)?;
        if matches!(cell.relief, Relief::Water | Relief::Mountain) {
            return None;
        }
        Some(Self {
            kind,
            style: art_style(cell.art),
            relief: cell.relief,
            x: (index % width) as i32,
            y: (index / width) as i32,
        })
    }

    /// The tree instances of the cell; `index` is the cell's index in the map (the fog texel).
    pub fn instances(&self, index: u32, tiles: &ForestTiles) -> Vec<StaticInstance> {
        self.instances_gated(index, tiles, &OpenGate)
    }

    /// As [`Self::instances`], without the trees the gate closes.
    pub fn instances_gated(
        &self,
        index: u32,
        tiles: &ForestTiles,
        gate: &dyn ForestGate,
    ) -> Vec<StaticInstance> {
        let centre = cell_center(self.x, self.y);
        tiles
            .cell_trees(self.x, self.y)
            .iter()
            .filter_map(|tree| {
                let h = ground_height(self.relief, tree.offset) + GROUND_LIFT;
                let at = centre + tree.offset;
                if !gate.allows(at, h) {
                    return None;
                }
                let mut i = StaticInstance::new(
                    [at.x, h, at.y],
                    tree.rotation_y,
                    tree.scale * TREE_SCALE * self.scale_factor(),
                );
                i.tint = [tree.tint; 3];
                i.cell = index;
                i.variant = tree.variant;
                Some(i)
            })
            .collect()
    }

    /// The `treeLocation` registers of the cell: `{x, y, z, scale}` of each tree the gate leaves.
    pub fn locations(&self, tiles: &ForestTiles, gate: &dyn ForestGate) -> Vec<[f32; 4]> {
        self.instances_gated(0, tiles, gate)
            .iter()
            .map(|i| [i.position[0], i.position[1], i.position[2], i.scale])
            .collect()
    }

    /// The factor of the scale of the trees: [`HILL_SCALE`] on a hill (types 3 and 4), else 1.
    pub fn scale_factor(&self) -> f32 {
        if self.relief == Relief::Hill {
            HILL_SCALE
        } else {
            1.0
        }
    }
}

/// Half the side of the shadow of a tree, in texels of the field: 6 for the forest and the jungle
/// (types 1 and 2); the upgraded ones (types 3 and 4) have 3.6, not drawn yet (see `tree_draws`).
// re: ForestVis::BuildShadows 0x086703c6 (DAT_0914a148: 6.0, 3.6 for `m_eType - 3 < 2`)
pub const SHADOW_HALF_SIDE: f32 = 6.0;
/// A tree casts no shadow on ground steeper than this (the z of the normal, z up).
// re: ForestVis::BuildShadows 0x086703c6 (DAT_0914a154 = 0.9 against `GetWorldNormal`)
pub const SHADOW_MIN_NORMAL_Z: f32 = 0.9;
/// Nor below this height, in units of `g_TerrainScale.z` (raw 12899 of 65535: just under the sea).
// re: ForestVis::BuildShadows 0x086703c6 (DAT_0914a150 = 50.583656 times `g_TerrainScale.z`)
pub const SHADOW_MIN_HEIGHT: f32 = 50.583656;
/// Nor where the stencil of the terrain has any of these bits (`SampleWorldPosition(pos, 7)`).
// re: ForestVis::BuildShadows 0x086703c6 (`TerrainStencil::SampleWorldPosition(&g_TerrainStencil, pos, 7)`)
pub const SHADOW_STENCIL_MASK: u8 = 0x07;

/// The shadows of the trees of a hex (`ForestVis::BuildShadows`): a square of
/// [`SHADOW_HALF_SIDE`] texels round each tree, in world `(X, Z)`, for the trees on flat, dry ground
/// outside the stencil. The original makes them one decal of the `Tree_Shadow` category, drawn into
/// the macro shadow of the pages (its pass flag is set).
pub fn tree_shadow_quads(
    instances: &[StaticInstance],
    sampler: &crate::world::terrain::sampler::TerrainSampler,
    stencil: Option<&crate::world::terrain::stencil::TerrainStencil>,
    cuts: &TreeCuts,
) -> Vec<[Vec2; 4]> {
    let frame = sampler.frame();
    let min_height = frame.terrain_scale().z * SHADOW_MIN_HEIGHT;
    let s = SHADOW_HALF_SIDE;
    instances
        .iter()
        .filter(|i| i.scale != 0.0)
        .filter_map(|i| {
            let at = Vec2::new(i.position[0], i.position[2]);
            if stencil.is_some_and(|st| st.sample_world(frame, at, SHADOW_STENCIL_MASK) != 0)
                || sampler.world_normal_orig(at).z < SHADOW_MIN_NORMAL_Z
                || sampler.world_height(at) < min_height
                // SIMPLIFIED(SIMP-065): the cuts of the landmarks are in the stencil of the original.
                || cuts.covers(at)
            {
                return None;
            }
            let t = frame.world_to_tex(at);
            Some(
                [(-s, -s), (s, -s), (s, s), (-s, s)]
                    .map(|(du, dv)| frame.tex_to_world(t + Vec2::new(du, dv))),
            )
        })
        .collect()
}

/// One draw of `ForestShader`: up to 90 trees of one kind and style.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeDraw {
    pub kind: TreeKind,
    pub style: usize,
    /// `treeLocation`: at most [`TREES_PER_DRAW`] registers.
    pub locations: Vec<[f32; 4]>,
}

/// The draws of the cells in the order given: the trees of each kind and style, 90 to a draw, the
/// last draw of a group the remainder.
pub fn tree_draws<'a>(
    cells: impl Iterator<Item = &'a TreeCell>,
    tiles: &ForestTiles,
    gate: &dyn ForestGate,
) -> Vec<TreeDraw> {
    let mut groups: BTreeMap<(TreeKind, usize), Vec<[f32; 4]>> = BTreeMap::new();
    for cell in cells {
        groups
            .entry((cell.kind, cell.style))
            .or_default()
            .extend(cell.locations(tiles, gate));
    }
    let mut out = Vec::new();
    for ((kind, style), all) in groups {
        for chunk in all.chunks(TREES_PER_DRAW) {
            out.push(TreeDraw {
                kind,
                style,
                locations: chunk.to_vec(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::scatter::{LAST_VISIBLE_TREE, TREE_MARGIN};
    use super::*;
    use crate::terrain::height::hex_norm;
    use crate::terrain::mesh::LAND_HEIGHT;
    use crate::terrain::test_util::{land, map};
    use crate::terrain::{FeatureKind, Fog, TerrainCell};
    use std::sync::OnceLock;

    fn tiles() -> &'static ForestTiles {
        static T: OnceLock<ForestTiles> = OnceLock::new();
        T.get_or_init(ForestTiles::default)
    }

    const VISIBLE: usize = LAST_VISIBLE_TREE + 1;

    fn forest() -> TerrainCell {
        TerrainCell {
            feat: FeatureKind::Forest,
            ..land()
        }
    }

    fn total(batches: &[TreeBatch]) -> usize {
        batches.iter().map(|b| b.instances.len()).sum()
    }

    #[test]
    fn every_forest_and_jungle_cell_gets_the_visible_trees() {
        let snap = map(6, 4, |x, y| match (x + y) % 4 {
            0 => forest(),
            1 => TerrainCell {
                feat: FeatureKind::Jungle,
                ..land()
            },
            _ => land(),
        });
        let batches = tree_batches(&snap, tiles());
        let cells = snap
            .cells()
            .iter()
            .filter(|c| c.feat != FeatureKind::None)
            .count();
        assert_eq!(total(&batches), cells * VISIBLE, "351 trees a hex");
        assert_eq!(batches.len(), 2, "forest and jungle, one style");
        assert_eq!(batches[0].kind, TreeKind::Forest);
        assert_eq!(batches[1].kind, TreeKind::Jungle);
    }

    #[test]
    fn land_cells_have_trees_revealed_or_not() {
        let snap = map(5, 1, |x, _| match x {
            0 => TerrainCell {
                fog: Fog::Unrevealed,
                ..forest()
            },
            1 => TerrainCell {
                relief: Relief::Water,
                ..forest()
            },
            2 => TerrainCell {
                relief: Relief::Mountain,
                ..forest()
            },
            3 => TerrainCell {
                fog: Fog::Hidden,
                relief: Relief::Hill,
                ..forest()
            },
            _ => TerrainCell {
                feat: FeatureKind::Marsh,
                ..land()
            },
        });
        let batches = tree_batches(&snap, tiles());
        assert_eq!(
            total(&batches),
            2 * VISIBLE,
            "the unrevealed forest and the hidden hill forest"
        );
        assert!(
            batches[0]
                .instances
                .iter()
                .all(|i| i.cell == 0 || i.cell == 3)
        );
    }

    #[test]
    fn instances_stand_in_their_cell_on_the_ground() {
        let snap = map(4, 3, |x, y| {
            if (x, y) == (2, 1) {
                TerrainCell {
                    relief: Relief::Hill,
                    ..forest()
                }
            } else {
                land()
            }
        });
        let batches = tree_batches(&snap, tiles());
        assert_eq!(batches.len(), 1);
        let centre = cell_center(2, 1);
        for i in &batches[0].instances {
            assert_eq!(i.cell, 4 + 2);
            let off = Vec2::new(i.position[0] - centre.x, i.position[2] - centre.y);
            assert!(hex_norm(off) <= TREE_MARGIN + 1e-3);
            assert!((i.position[1] - GROUND_LIFT - ground_height(Relief::Hill, off)).abs() < 1e-3);
            assert!(i.position[1] > LAND_HEIGHT + GROUND_LIFT - 1e-3);
            // A hill: three quarters of the flat scales.
            let k = TREE_SCALE * HILL_SCALE;
            assert!((0.5 * k..=k).contains(&i.scale));
        }
    }

    #[test]
    fn batches_are_split_by_continent_style() {
        let snap = map(4, 1, |x, _| TerrainCell {
            art: [0, 2, 2, 9][x as usize],
            ..forest()
        });
        let batches = tree_batches(&snap, tiles());
        let styles: Vec<(usize, usize)> = batches
            .iter()
            .map(|b| (b.style, b.instances.len() / VISIBLE))
            .collect();
        // Style 9 does not exist and takes Generic, like the terrain.
        assert_eq!(styles, vec![(0, 2), (2, 2)]);
    }

    #[test]
    fn trees_do_not_depend_on_the_rest_of_the_map() {
        let small = map(4, 4, |_, _| forest());
        let big = map(9, 7, |_, _| forest());
        let pick = |s: &TerrainSnapshot| {
            let b = tree_batches(s, tiles());
            let w = s.width();
            let want = 2 * w + 3;
            b[0].instances
                .iter()
                .filter(|i| i.cell == want)
                .map(|i| (i.position, i.variant))
                .collect::<Vec<_>>()
        };
        assert_eq!(pick(&small).len(), VISIBLE);
        assert_eq!(pick(&small), pick(&big));
    }

    /// A gate that closes the world west of `x`.
    struct West(f32);
    impl ForestGate for West {
        fn allows(&self, p: Vec2, _h: f32) -> bool {
            p.x >= self.0
        }
    }

    #[test]
    fn a_closed_place_loses_its_trees() {
        let cell = TreeCell::of(&forest(), 0, 1).unwrap();
        let all = cell.locations(tiles(), &OpenGate);
        assert_eq!(all.len(), VISIBLE);
        let centre = cell_center(0, 0);
        let part = cell.locations(tiles(), &West(centre.x));
        assert!(!part.is_empty() && part.len() < all.len(), "{}", part.len());
        assert!(part.iter().all(|l| l[0] >= centre.x));
        assert!(cell.locations(tiles(), &West(1e9)).is_empty());
    }

    /// The index of the cuts gives what the scan of every triangle gave, also for triangles on the
    /// edge of a square and for one that spans too many squares for the index.
    #[test]
    fn the_cut_index_agrees_with_the_scan_of_every_triangle() {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut rnd = move |range: f32| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 40) as f32 / (1u64 << 24) as f32 * range
        };
        let mut tris: Vec<[Vec2; 3]> = (0..300)
            .map(|_| {
                let o = Vec2::new(rnd(2000.0) - 500.0, rnd(2000.0) - 500.0);
                [
                    o,
                    o + Vec2::new(rnd(90.0), rnd(40.0)),
                    o + Vec2::new(rnd(40.0), rnd(90.0)),
                ]
            })
            .collect();
        // A triangle on the line between two squares, and a huge one.
        tris.push([
            Vec2::new(64.0, 64.0),
            Vec2::new(128.0, 64.0),
            Vec2::new(64.0, 128.0),
        ]);
        tris.push([
            Vec2::new(-9000.0, 3000.0),
            Vec2::new(-8000.0, 3000.0),
            Vec2::new(-8500.0, 3600.0),
        ]);
        let cuts = TreeCuts::new(tris.clone());
        let scan = |p: Vec2| {
            tris.iter().any(|t| {
                let lo = t[0].min(t[1]).min(t[2]);
                let hi = t[0].max(t[1]).max(t[2]);
                p.x >= lo.x && p.y >= lo.y && p.x <= hi.x && p.y <= hi.y && in_triangle(p, t)
            })
        };
        let mut hits = 0;
        for _ in 0..4000 {
            let p = Vec2::new(rnd(2600.0) - 600.0, rnd(2600.0) - 600.0);
            assert_eq!(cuts.covers(p), scan(p), "{p}");
            hits += usize::from(scan(p));
        }
        assert!(hits > 100, "the points reach the cuts: {hits}");
        for p in [
            Vec2::new(64.0, 64.0),
            Vec2::new(128.0, 64.0),
            Vec2::new(-8500.0, 3100.0),
        ] {
            assert!(cuts.covers(p), "{p}");
        }
        assert!(!cuts.covers(Vec2::new(-8500.0, 100.0)));
    }

    /// Triangles taken out and put in again leave an index that covers what a fresh one covers (a
    /// triangle twice in it needs two takings out).
    #[test]
    fn cuts_taken_out_and_put_in_agree_with_a_fresh_index() {
        let tri = |x: f32, y: f32| {
            [
                Vec2::new(x, y),
                Vec2::new(x + 70.0, y + 10.0),
                Vec2::new(x + 10.0, y + 80.0),
            ]
        };
        let wide = [
            Vec2::new(-9000.0, 3000.0),
            Vec2::new(-8000.0, 3000.0),
            Vec2::new(-8500.0, 3600.0),
        ];
        let mut tris: Vec<[Vec2; 3]> = (0..60)
            .map(|i| tri((i % 10) as f32 * 90.0, (i / 10) as f32 * 90.0))
            .collect();
        tris.push(tris[3]);
        tris.push(wide);
        let mut cuts = TreeCuts::new(tris.clone());
        assert_eq!(cuts.len(), 62);
        assert!(!cuts.remove(&tri(5.0, 5.0)));
        for t in [tris[3], tris[7], tris[20], wide] {
            assert!(cuts.remove(&t));
        }
        // The twin of tris[3] still stands; the rest of what was taken out is gone.
        let mut kept: Vec<[Vec2; 3]> = tris.clone();
        kept.retain(|t| *t != tris[7] && *t != tris[20] && *t != wide);
        kept.remove(3);
        for t in [tri(7.0 * 90.0 % 900.0, 0.0), tri(5.0 * 90.0, 90.0)] {
            cuts.add(t);
            kept.push(t);
        }
        let fresh = TreeCuts::new(kept.clone());
        assert_eq!(cuts.len(), fresh.len());
        let mut p = Vec2::new(-20.0, -20.0);
        while p.y < 700.0 {
            p.x = -20.0;
            while p.x < 950.0 {
                assert_eq!(cuts.covers(p), fresh.covers(p), "{p}");
                p.x += 7.0;
            }
            p.y += 7.0;
        }
        assert!(!cuts.covers(Vec2::new(-8500.0, 3100.0)));
        assert!(cuts.remove(&tris[3]) && !cuts.remove(&tris[3]));
    }

    #[test]
    fn a_tree_cut_clears_the_ground_it_covers() {
        let cell = TreeCell::of(&forest(), 0, 1).unwrap();
        let all = cell.locations(tiles(), &OpenGate);
        let c = cell_center(0, 0);
        // The east half of the cell, as two triangles.
        let (a, b, d, e) = (
            Vec2::new(c.x, c.y - 100.0),
            Vec2::new(c.x + 100.0, c.y - 100.0),
            Vec2::new(c.x + 100.0, c.y + 100.0),
            Vec2::new(c.x, c.y + 100.0),
        );
        let cuts = TreeCuts::new([[a, b, d], [a, d, e]]);
        assert!(cuts.covers(c + Vec2::new(1.0, 1.0)) && !cuts.covers(c - Vec2::new(1.0, 1.0)));
        let gate = CutGate {
            inner: &OpenGate,
            cuts: &cuts,
        };
        let left = cell.locations(tiles(), &gate);
        assert!(!left.is_empty() && left.len() < all.len(), "{}", left.len());
        assert!(left.iter().all(|l| l[0] <= c.x));
        // No cuts: the inner gate alone.
        let none = TreeCuts::default();
        let open = CutGate {
            inner: &West(1e9),
            cuts: &none,
        };
        assert!(cell.locations(tiles(), &open).is_empty());
    }

    /// Golden S2: the trees of a hill cell are three quarters of those of a flat one.
    #[test]
    fn hill_trees_are_three_quarters() {
        let cell = TreeCell::of(&forest(), 0, 1).unwrap();
        let hill = TreeCell {
            relief: Relief::Hill,
            ..cell
        };
        let a = cell.locations(tiles(), &OpenGate);
        let b = hill.locations(tiles(), &OpenGate);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert!((y[3] - x[3] * HILL_SCALE).abs() < 1e-6);
            assert_eq!([x[0], x[2]], [y[0], y[2]]);
        }
    }

    #[test]
    fn draws_hold_ninety_trees_and_the_last_the_rest() {
        let snap = map(3, 1, |_, _| forest());
        let width = snap.width() as usize;
        let cells: Vec<TreeCell> = snap
            .cells()
            .iter()
            .enumerate()
            .filter_map(|(i, c)| TreeCell::of(c, i, width))
            .collect();
        let draws = tree_draws(cells.iter(), tiles(), &OpenGate);
        let n = 3 * VISIBLE;
        assert_eq!(draws.iter().map(|d| d.locations.len()).sum::<usize>(), n);
        assert!(draws.iter().all(|d| d.locations.len() <= TREES_PER_DRAW));
        assert_eq!(draws.len(), n.div_ceil(TREES_PER_DRAW));
        assert!(
            draws[..draws.len() - 1]
                .iter()
                .all(|d| d.locations.len() == TREES_PER_DRAW)
        );
    }

    /// `ForestVis::BuildShadows`: a square of 12 texels round each tree on dry flat ground; none under
    /// the sea, on the stencil or on a cut.
    #[test]
    fn a_tree_casts_a_square_shadow_on_dry_flat_ground_only() {
        use crate::world::terrain::sampler::{TerrainFrame, TerrainSampler};
        use crate::world::terrain::stencil::TerrainStencil;
        let frame = TerrainFrame::new(4, 4, false);
        let mut sampler = TerrainSampler::new(frame);
        let (w, h) = frame.texel_size();
        for v in 0..h as i32 {
            for u in 0..w as i32 {
                // Dry land on the west half, the bottom of the sea on the east one.
                let raw = if u < w as i32 / 2 { 15_000 } else { 5_000 };
                sampler.field_mut().set_height(u, v, raw);
            }
        }
        let dry = frame.tex_to_world(Vec2::new(40.0, 100.0));
        let wet = frame.tex_to_world(Vec2::new(200.0, 100.0));
        let tree = |p: Vec2| StaticInstance::new([p.x, 0.0, p.y], 0.0, 1.0);
        let trees = [tree(dry), tree(wet)];
        let quads = tree_shadow_quads(&trees, &sampler, None, &TreeCuts::default());
        assert_eq!(quads.len(), 1, "the tree under the sea casts none");
        let t: Vec<Vec2> = quads[0].iter().map(|&p| frame.world_to_tex(p)).collect();
        for (c, (du, dv)) in t
            .iter()
            .zip([(-6.0, -6.0), (6.0, -6.0), (6.0, 6.0), (-6.0, 6.0)])
        {
            assert!(
                (*c - Vec2::new(40.0 + du, 100.0 + dv)).length() < 1e-3,
                "{c}"
            );
        }
        // The stencil (bits 0..2) and a cut close the ground.
        let mut stencil = TerrainStencil::new(w, h);
        stencil.add_triangle(
            [
                Vec2::new(0.0, 0.0),
                Vec2::new(100.0, 0.0),
                Vec2::new(0.0, 100.0),
            ],
            0x04,
        );
        assert!(
            tree_shadow_quads(&trees, &sampler, Some(&stencil), &TreeCuts::default()).is_empty()
        );
        let cut = TreeCuts::new([[
            dry + Vec2::splat(-5.0),
            dry + Vec2::new(5.0, -5.0),
            dry + Vec2::new(0.0, 5.0),
        ]]);
        assert!(tree_shadow_quads(&trees, &sampler, None, &cut).is_empty());
    }
}
