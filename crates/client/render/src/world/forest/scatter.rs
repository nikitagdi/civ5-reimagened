//! Where the trees of forests and jungles stand (world-objects spec 4.6, WOB3; RW-16
//! `ForestVisSystem::BuildTreeLists`). CPU only, so placement is tested without a device.
//!
//! The original builds 20 tree lists once; list `i` takes the placement mask `forest_tile<i % N>`
//! (`R8`, 128x128, `num_tiles` = N). Each list scatters `random_trees_per_tile` (rounded to a
//! multiple of 90) trees at random in a 96x96 tile. A tree of scale `0.5 + 0.5 r` takes a position
//! `(x, y)` in the tile and is accepted when the mask texel `((x / 96) * 127, (1 - y / 96) * 127)`
//! is not zero (otherwise the same index tries again), and when no placed tree is nearer than
//! `(scale_a + scale_b) * space_between` (20 tries, then the spot is taken anyway). Trees with an
//! index above 350 have scale 0 and are not made. A forest cell takes one of the lists at random,
//! centred on the hex. The same is done here, from fixed seeds; the list of a cell is picked from
//! its coordinates, so a tree never moves when the rest of the map changes.
//!
//! SIMPLIFIED(SIMP-009): the positions are ours (the original draws from the C runtime `rand`; its
//! counts, scales, clearance and masks are kept). Why they stay ours (b2/r-forest, 2026-10-28): the
//! lists come from the shared CRT stream (`rand_wincompatible`, the LCG `214013 x + 2531011`; the
//! `w` of the `treeLocation` registers of S1a give the 15-bit draws exactly, and from one tree's
//! draw the following trees of its cell are found in the stream at 125 to 197 hits in 1500 draws
//! against 16 by chance). That stream is seeded by `TerrainSystem::Init` (`srand(uiRandomSeed)`)
//! and drawn by the whole terrain build before `ForestVisSystem::StartupGame` builds the lists, so
//! they differ from game to game (S3 shares none of its lists with S1, S2 and S4, which share all
//! of theirs), and a cell takes its list by `rand() % 20` when the sim creates its forest
//! (`ForestVisSystem::UpdateFromHandle`). Their places need a `rand`-exact port of the terrain
//! build, which is not done (SIMP-035). Without masks (an art set that has none) the hex
//! outline ([`TREE_MARGIN`]) stands in for the tile mask. The rotation and the tint are used by the
//! `static` fallback only: `ForestVS` has no rotation (a tree is a position and a scale).

use std::collections::BTreeMap;

use asset_format::{TextureBlob, TextureFormat};
use glam::Vec2;

use crate::terrain::FeatureKind;
use crate::terrain::height::hex_norm;

/// Edge of the square tile the trees are scattered in (`kTileExtent`), hex-space units.
pub const TILE_EXTENT: f32 = 96.0;
/// The north-south extent of the tile in the world against the east-west one: the tile is laid in
/// the texels of the height map, a texel 1 unit across and `g_TerrainScale.y` = 0.866 along (golden
/// S2 had measured 0.9).
// re: ForestVisSystem::AddRandomLocationsInTile 0x08668cda (tex = HexToTex(0) + (x - 48, y - 48)),
// g_TerrainScale 0x0a3414b4 (1, 0.866, 0.2) (RE-B18 item 3b F3)
pub const TILE_Y_SCALE: f32 = 0.866;
/// Number of tree lists (`kNumTreeLists`).
pub const TILE_LISTS: usize = 20;
/// Placement tries per tree before the spacing is given up (`kMaxPlacementTries`).
pub const MAX_PLACEMENT_TRIES: u32 = 20;
/// Trees with a larger index have scale 0 (`kLastVisibleTree`).
pub const LAST_VISIBLE_TREE: usize = 350;
/// The XML tree counts are rounded to a multiple of this (`kTreesPerShader`).
pub const TREES_PER_SHADER: f32 = 90.0;
/// Names of the placement parameters of `forests.xml` in the art table.
pub const RANDOM_TREES_KEY: &str = "random_trees_per_tile";
pub const SPACE_BETWEEN_KEY: &str = "space_between";
/// Largest distance of a tree from the cell centre, as a fraction of the hex (1.0 is the edge).
// SIMPLIFIED(SIMP-009): the hex outline instead of the tile mask
pub const TREE_MARGIN: f32 = 0.95;

/// Side of a placement mask (`forest_tile<i>.dds`, L8).
pub const MASK_SIZE: usize = 128;

/// One placement mask: where in the tile a tree may stand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForestMask {
    texels: Vec<u8>,
}

impl ForestMask {
    /// The texels of an `R8` texture of 128x128; `None` for any other shape.
    pub fn from_blob(blob: &TextureBlob) -> Option<Self> {
        if blob.format != TextureFormat::R8
            || blob.width as usize != MASK_SIZE
            || blob.height as usize != MASK_SIZE
        {
            return None;
        }
        let level0 = blob.mips.first()?;
        (level0.len() == MASK_SIZE * MASK_SIZE).then(|| Self {
            texels: level0.clone(),
        })
    }

    /// From the 128 x 128 texels row by row, for the tests.
    pub fn from_texels(texels: Vec<u8>) -> Option<Self> {
        (texels.len() == MASK_SIZE * MASK_SIZE).then_some(Self { texels })
    }

    /// True if the tile position `p` (0..96 on both axes) is open.
    // re: ForestVisSystem::BuildTreeLists (RW-16): the texel at ((x / 96) * 127, (1 - y / 96) * 127)
    pub fn open(&self, p: Vec2) -> bool {
        let last = (MASK_SIZE - 1) as f32;
        let tx = ((p.x / TILE_EXTENT) * last).clamp(0.0, last) as usize;
        let ty = ((1.0 - p.y / TILE_EXTENT) * last).clamp(0.0, last) as usize;
        self.texels[ty * MASK_SIZE + tx] != 0
    }
}

/// Placement parameters of `forests.xml`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForestParams {
    pub random_trees_per_tile: u32,
    pub space_between: f32,
}

impl Default for ForestParams {
    /// The values of the shipped `forests.xml`.
    fn default() -> Self {
        Self {
            random_trees_per_tile: 360,
            space_between: 1.33,
        }
    }
}

impl ForestParams {
    /// Reads the `placement` of a feature record; a missing key keeps the shipped value.
    pub fn from_placement(placement: &BTreeMap<String, f32>) -> Self {
        let d = Self::default();
        Self {
            random_trees_per_tile: placement
                .get(RANDOM_TREES_KEY)
                .filter(|v| v.is_finite() && **v >= 0.0)
                .map_or(d.random_trees_per_tile, |v| *v as u32),
            space_between: placement
                .get(SPACE_BETWEEN_KEY)
                .filter(|v| v.is_finite() && **v >= 0.0)
                .copied()
                .unwrap_or(d.space_between),
        }
    }

    /// Trees per tile after the rounding to a multiple of 90.
    pub fn rounded_trees(&self) -> usize {
        ((self.random_trees_per_tile as f32 / TREES_PER_SHADER).round() * TREES_PER_SHADER) as usize
    }

    /// Trees of a tile that are drawn: the ones up to index 350.
    pub fn visible_trees(&self) -> usize {
        self.rounded_trees().min(LAST_VISIBLE_TREE + 1)
    }
}

/// What kind of trees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TreeKind {
    Forest,
    Jungle,
}

impl TreeKind {
    pub const ALL: [TreeKind; 2] = [Self::Forest, Self::Jungle];

    pub fn from_feature(f: FeatureKind) -> Option<Self> {
        match f {
            FeatureKind::Forest => Some(Self::Forest),
            FeatureKind::Jungle => Some(Self::Jungle),
            _ => None,
        }
    }

    pub fn feature(self) -> FeatureKind {
        match self {
            Self::Forest => FeatureKind::Forest,
            Self::Jungle => FeatureKind::Jungle,
        }
    }
}

/// SplitMix64: small, fast, and the same everywhere.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// In `[0, 1)`.
    fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// One tree of a list, relative to the cell centre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tree {
    /// World `(X, Z)` offset from the cell centre.
    pub offset: Vec2,
    pub rotation_y: f32,
    /// `0.5 + 0.5 r` of the original; the model is scaled by this times `TREE_SCALE` (batches.rs).
    pub scale: f32,
    /// Brightness multiplier.
    pub tint: f32,
    /// Raw variant number; the model takes it modulo its variant count.
    pub variant: u32,
}

/// Which of the lists a cell takes: from its coordinates only.
pub fn list_index(x: i32, y: i32) -> usize {
    let mut rng = Rng((u64::from(x as u32) << 32 | u64::from(y as u32)) ^ 0x7EE5_F0E5_7C9D_3A11);
    (rng.next_u64() % TILE_LISTS as u64) as usize
}

/// Scatters one list: the same for the same `index` and `params`.
fn scatter_list(index: usize, params: &ForestParams, mask: Option<&ForestMask>) -> Vec<Tree> {
    let want = params.visible_trees();
    let mut rng = Rng(0xF04E_57C0_FFEE_0001 ^ (index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    // Bucket grid so that the spacing test looks at the neighbours only: the largest spacing is
    // 2 * space_between (both scales are at most 1).
    let bucket = (2.0 * params.space_between).max(1.0);
    let n = (TILE_EXTENT / bucket).ceil() as usize + 1;
    let mut grid: Vec<Vec<u32>> = vec![Vec::new(); n * n];
    let cell_of = |p: Vec2| {
        (
            ((p.x / bucket) as usize).min(n - 1),
            ((p.y / bucket) as usize).min(n - 1),
        )
    };
    let mut placed: Vec<(Vec2, f32)> = Vec::with_capacity(want);
    let mut trees = Vec::with_capacity(want);
    let mut attempts = 0usize;
    while trees.len() < want && attempts < want * 1000 {
        attempts += 1;
        let scale = 0.5 + 0.5 * rng.unit();
        let mut p = Vec2::ZERO;
        let mut tries = 0;
        let mut redraws = 0;
        while tries < MAX_PLACEMENT_TRIES {
            p = Vec2::new(rng.unit(), rng.unit()) * TILE_EXTENT;
            // A closed texel of the mask: the same index draws again (not a try of the clearance).
            // The bound keeps a nearly closed mask from looping for ever.
            if mask.is_some_and(|m| !m.open(p)) && redraws < 10_000 {
                redraws += 1;
                continue;
            }
            tries += 1;
            let (cx, cy) = cell_of(p);
            let crowded = (cx.saturating_sub(1)..=(cx + 1).min(n - 1)).any(|gx| {
                (cy.saturating_sub(1)..=(cy + 1).min(n - 1)).any(|gy| {
                    grid[gy * n + gx].iter().any(|&o| {
                        let (op, os) = placed[o as usize];
                        op.distance(p) < (os + scale) * params.space_between
                    })
                })
            });
            if !crowded || tries == MAX_PLACEMENT_TRIES {
                break;
            }
        }
        // The tile is centred on the hex. With masks the mask is the shape of the forest, a little
        // flatter north to south than the tile.
        let mut offset = p - Vec2::splat(TILE_EXTENT / 2.0);
        if mask.is_none() && hex_norm(offset) > TREE_MARGIN {
            continue;
        }
        if mask.is_some() {
            offset.y *= TILE_Y_SCALE;
        }
        let (cx, cy) = cell_of(p);
        grid[cy * n + cx].push(placed.len() as u32);
        placed.push((p, scale));
        trees.push(Tree {
            offset,
            rotation_y: rng.unit() * std::f32::consts::TAU,
            scale,
            tint: 0.85 + 0.15 * rng.unit(),
            variant: rng.next_u64() as u32,
        });
    }
    trees
}

/// The tree lists of a game: built once, then a forest cell takes one by [`list_index`].
#[derive(Debug, Clone, PartialEq)]
pub struct ForestTiles {
    lists: Vec<Vec<Tree>>,
    /// Masks the lists were scattered with (`num_tiles`); empty for the hex outline.
    masks: usize,
}

impl ForestTiles {
    /// The lists with the hex outline in place of the masks.
    pub fn new(params: &ForestParams) -> Self {
        Self::with_masks(params, &[])
    }

    /// The lists with the masks of the art: list `i` takes mask `i % masks.len()`.
    pub fn with_masks(params: &ForestParams, masks: &[ForestMask]) -> Self {
        Self {
            lists: (0..TILE_LISTS)
                .map(|i| {
                    let mask = (!masks.is_empty()).then(|| &masks[i % masks.len()]);
                    scatter_list(i, params, mask)
                })
                .collect(),
            masks: masks.len(),
        }
    }

    /// How many masks the lists were made with.
    pub fn mask_count(&self) -> usize {
        self.masks
    }

    /// The trees of cell `(x, y)`: the same for the same coordinates, whatever the map around it.
    pub fn cell_trees(&self, x: i32, y: i32) -> &[Tree] {
        &self.lists[list_index(x, y)]
    }

    pub fn list(&self, index: usize) -> &[Tree] {
        &self.lists[index % TILE_LISTS]
    }
}

impl Default for ForestTiles {
    fn default() -> Self {
        Self::new(&ForestParams::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    fn tiles() -> &'static ForestTiles {
        static T: OnceLock<ForestTiles> = OnceLock::new();
        T.get_or_init(ForestTiles::default)
    }

    const VISIBLE: usize = LAST_VISIBLE_TREE + 1;

    fn params(random: f32, space: f32) -> BTreeMap<String, f32> {
        BTreeMap::from([
            (RANDOM_TREES_KEY.to_owned(), random),
            (SPACE_BETWEEN_KEY.to_owned(), space),
        ])
    }

    #[test]
    fn counts_follow_forests_xml() {
        let p = ForestParams::default();
        assert_eq!(p.rounded_trees(), 360);
        assert_eq!(p.visible_trees(), 351, "trees above index 350 have scale 0");
        for (n, rounded) in [(0, 0), (44, 0), (45, 90), (100, 90), (140, 180), (360, 360)] {
            let p = ForestParams {
                random_trees_per_tile: n,
                ..p
            };
            assert_eq!(p.rounded_trees(), rounded, "{n}");
        }
        let few = ForestParams {
            random_trees_per_tile: 90,
            ..p
        };
        assert_eq!(few.visible_trees(), 90);
        let from = ForestParams::from_placement(&params(180.0, 2.0));
        assert_eq!((from.random_trees_per_tile, from.space_between), (180, 2.0));
        assert_eq!(
            ForestParams::from_placement(&BTreeMap::new()),
            ForestParams::default()
        );
        assert_eq!(
            ForestParams::from_placement(&params(f32::NAN, -1.0)),
            ForestParams::default()
        );
    }

    #[test]
    fn every_list_has_the_visible_trees_and_is_deterministic() {
        let t = tiles();
        for i in 0..TILE_LISTS {
            assert_eq!(t.list(i).len(), VISIBLE, "list {i}");
        }
        assert_eq!(&ForestTiles::new(&ForestParams::default()), t);
        assert_ne!(t.list(0), t.list(1));
        let small = ForestTiles::new(&ForestParams {
            random_trees_per_tile: 90,
            ..ForestParams::default()
        });
        // A shorter list is a prefix of the longer one: the same random stream.
        assert_eq!(small.list(3), &t.list(3)[..90]);
    }

    #[test]
    fn a_cell_gets_the_same_list_every_time() {
        let t = tiles();
        assert_eq!(t.cell_trees(7, 11), t.cell_trees(7, 11));
        assert_eq!(list_index(7, 11), list_index(7, 11));
        // The cells of a map spread over the lists.
        let used: std::collections::BTreeSet<usize> = (0..40)
            .flat_map(|x| (0..24).map(move |y| list_index(x, y)))
            .collect();
        assert_eq!(used.len(), TILE_LISTS);
    }

    #[test]
    fn trees_stay_inside_the_hex_and_scales_follow_the_original() {
        let t = tiles();
        for i in 0..TILE_LISTS {
            for tree in t.list(i) {
                assert!(
                    hex_norm(tree.offset) <= TREE_MARGIN + 1e-4,
                    "{:?}",
                    tree.offset
                );
                assert!((0.0..std::f32::consts::TAU).contains(&tree.rotation_y));
                assert!((0.5..=1.0).contains(&tree.scale), "{}", tree.scale);
                assert!(tree.tint > 0.0 && tree.tint <= 1.0);
            }
        }
    }

    #[test]
    fn spacing_holds_except_where_the_tries_ran_out() {
        let t = tiles();
        let space = ForestParams::default().space_between;
        let list = t.list(0);
        let mut close = 0;
        for (i, a) in list.iter().enumerate() {
            for b in &list[..i] {
                if a.offset.distance(b.offset) < (a.scale + b.scale) * space {
                    close += 1;
                    break;
                }
            }
        }
        // The first trees are placed clear of the others; only the crowded late ones give up.
        assert!(close < list.len() / 4, "{close} of {}", list.len());
        assert!(
            list[..100].iter().enumerate().all(|(i, a)| list[..i]
                .iter()
                .all(|b| a.offset.distance(b.offset) >= (a.scale + b.scale) * space)),
            "the first hundred trees keep the spacing"
        );
    }

    #[test]
    fn a_wider_spacing_spreads_the_trees() {
        let tight = ForestTiles::new(&ForestParams {
            random_trees_per_tile: 90,
            space_between: 0.1,
        });
        let wide = ForestTiles::new(&ForestParams {
            random_trees_per_tile: 90,
            space_between: 6.0,
        });
        let nearest = |l: &[Tree]| {
            l.iter()
                .enumerate()
                .flat_map(|(i, a)| l[..i].iter().map(move |b| a.offset.distance(b.offset)))
                .fold(f32::MAX, f32::min)
        };
        assert!(nearest(wide.list(0)) > nearest(tight.list(0)));
    }

    /// A mask that is open in the left half of the tile only.
    fn left_half() -> ForestMask {
        let mut t = vec![0u8; MASK_SIZE * MASK_SIZE];
        for y in 0..MASK_SIZE {
            for x in 0..MASK_SIZE / 2 {
                t[y * MASK_SIZE + x] = 255;
            }
        }
        ForestMask::from_texels(t).unwrap()
    }

    #[test]
    fn a_mask_texel_is_read_flipped_in_y() {
        // Only the top row of the texture is open: y = 0 of the file is y = 96 of the tile.
        let mut t = vec![0u8; MASK_SIZE * MASK_SIZE];
        t[..MASK_SIZE].fill(1);
        let m = ForestMask::from_texels(t).unwrap();
        assert!(m.open(Vec2::new(10.0, TILE_EXTENT)));
        assert!(!m.open(Vec2::new(10.0, 0.0)));
        assert!(ForestMask::from_texels(vec![0; 5]).is_none());
    }

    #[test]
    fn a_mask_comes_from_an_r8_texture_of_128() {
        let blob = |format, w, h, len| TextureBlob {
            format,
            dim: asset_format::TexDim::D2,
            srgb: false,
            width: w,
            height: h,
            depth: 1,
            mips: vec![vec![255; len]],
        };
        assert!(ForestMask::from_blob(&blob(TextureFormat::R8, 128, 128, 128 * 128)).is_some());
        assert!(ForestMask::from_blob(&blob(TextureFormat::R8, 64, 64, 64 * 64)).is_none());
        assert!(ForestMask::from_blob(&blob(TextureFormat::Rgba8, 128, 128, 128 * 128)).is_none());
    }

    /// RW-16: the trees stand where the mask is open, 351 of them still, and a list takes the mask
    /// `i % N`.
    #[test]
    fn the_trees_keep_to_the_mask_and_the_count() {
        let masks = [left_half()];
        let t = ForestTiles::with_masks(&ForestParams::default(), &masks);
        assert_eq!(t.mask_count(), 1);
        for i in 0..TILE_LISTS {
            let list = t.list(i);
            assert_eq!(list.len(), LAST_VISIBLE_TREE + 1, "list {i}");
            for tree in list {
                // The tile is centred on the hex: tile x = offset.x + 48; the open half is x < 48.
                assert!(tree.offset.x <= 0.8, "{:?}", tree.offset);
            }
        }
        // Scales stay in the original's range, whatever the mask.
        assert!(t.list(0).iter().all(|tr| (0.5..=1.0).contains(&tr.scale)));
        // The same mask and params give the same lists.
        assert_eq!(t, ForestTiles::with_masks(&ForestParams::default(), &masks));
        assert_ne!(&t, tiles());
    }

    #[test]
    fn lists_alternate_the_masks() {
        let open_all = ForestMask::from_texels(vec![255; MASK_SIZE * MASK_SIZE]).unwrap();
        let masks = [left_half(), open_all];
        let t = ForestTiles::with_masks(&ForestParams::default(), &masks);
        let wide = |i: usize| t.list(i).iter().any(|tr| tr.offset.x > 5.0);
        assert!(!wide(0) && wide(1) && !wide(2) && wide(3));
    }
}
