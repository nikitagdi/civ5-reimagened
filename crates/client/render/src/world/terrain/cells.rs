//! The cells of the terrain: the grid of 64 x 64 texels that the
//! cache works on, the hexes each cell touches, the class of its style, its statistics, the
//! textures it blends and the weights of its vertices. All of it on the CPU, as in the original:
//! `Terrain::CalcHeightMap`, `BuildCellToHexTable`, `TerrainCell::CalculateStats`,
//! `CalculateTextureContributions`, `BuildTextureVertexBatch`, `CalcBlendVector`,
//! `IsVisible`, `CalcMaxWidth`, `CalcMIPLevelNeeded`.
//!
//! Open (RW-7): when `g_bUseGPUWeightBlending` is 1 on the user's machine the weights are the
//! business of `ResolveDominancePS`; [`build_cell_vertices`] has the switch.
//!
//! The vertices are values here, not bytes: the bake (R2.9b) packs them for the GPU.

use glam::{Mat4, Vec2, Vec3, Vec4};

use asset_format::{TerrainDefRow, f32_to_f16_bits};

use super::basis::TerrainBasis;
use super::field::TEXELS_PER_HEX;
use super::sampler::{HEIGHT_TO_UNIT, TerrainFrame, TerrainSampler};
use crate::hexgrid::{cell_center, corner_offset};
use crate::terrain::Fog;

/// Texels between the origins of two cells.
// re: Terrain::CalcHeightMap (Terrain.cpp:444-468: `uiCellX << 6`)
pub const CELL_STEP: i32 = 64;
/// Texels of a cell, with the row and column it shares with the next cell.
// re: Terrain::CalcHeightMap (Terrain.cpp:453, :464: `0x41`)
pub const CELL_SPAN: i32 = 0x41;
/// Textures a cell blends at most.
// re: TerrainCell::CalculateTextureContributions (TerrainCell.cpp:1201: kMaxTextures)
pub const MAX_TEXTURES: usize = 8;
/// Projections (axis entries of the textures) the table holds at most.
// re: Terrain::SetTextures (Terrain.cpp:1421: `== 64`)
pub const MAX_PROJECTIONS: usize = 64;
/// The normal is flat above this.
// re: TerrainCell.cpp:96 (kFlatNormalZ)
pub const FLAT_NORMAL_Z: f32 = 0.9;
/// A weight in `0..=1` is stored as `w * 255`.
// re: TerrainCell.cpp:98 (kWeightScale)
pub const WEIGHT_SCALE: f32 = 255.0;
/// Below this sum of weights the first texture takes all.
// re: TerrainCell.cpp:99 (kMinWeightSum)
pub const MIN_WEIGHT_SUM: f64 = 0.001;
/// The separable taps of the blend kernel (the product of two, normalized, is the 5 x 5 kernel).
// re: Terrain.cpp:39 (kBlendKernel1D)
pub const BLEND_KERNEL_1D: [f32; 5] = [0.125, 0.636_056, 1.0, 0.636_056, 0.125];
/// Largest height (in units of raw / 255) of a cell that has no ground, and of one that is below
/// the sea floor line.
// re: TerrainCell.cpp:1136-1137 (kStatsLowHeight, kStatsMidHeight)
pub const STATS_LOW_HEIGHT: f32 = 2.0;
pub const STATS_MID_HEIGHT: f32 = 30.0;
/// The scale of a texture along axis 0 and along the other axes snaps to whole world units of
/// `scale * factor * world range`.
// re: TerrainCell.cpp:826-845, Terrain.cpp:78-80 (kAxisZeroScale, kOtherAxisScale)
pub const SNAP_AXIS_ZERO: f32 = 0.33;
pub const SNAP_OTHER_AXES: f32 = 0.66;
/// Side of the grid of normals the contributions read.
// re: TerrainCell::CalculateTextureContributions (TerrainCell.cpp:1200: kNormalGrid)
const NORMAL_GRID: i32 = 69;

/// The block of normals (origin, size) [`texture_contributions`] reads for a cell.
pub fn contribution_normals(b: &CellBox) -> ((i32, i32), (i32, i32)) {
    ((b.x, b.y), (NORMAL_GRID, NORMAL_GRID))
}

/// The block of normals (origin, size) [`build_cell_vertices`] reads for a cell.
pub fn vertex_normals(b: &CellBox) -> ((i32, i32), (i32, i32)) {
    ((b.x - 1, b.y - 1), (b.w + 2, b.h + 2))
}

/// Class of the style from the mask of the continents of a cell: the lowest set bit of 1..=4, else 0.
// re: TerrainCell.cpp:41-50 (ClassFromMask)
pub fn class_from_mask(mask: u32) -> u8 {
    (1..5u8).find(|b| (mask >> b) & 1 == 1).unwrap_or(0)
}

/// The mask bit of a continent.
// re: Terrain::Init (Terrain.cpp:299: `1u << (ucContinent & 0x1f)`)
pub fn class_bit(continent: u8) -> u32 {
    1 << (continent & 0x1f)
}

/// The scale of a texture snapped as the bake does: `int(scale * k * range) / range / k`, with
/// `k` 0.33 for axis 0 and 0.66 for the others. `range` is the width of the world.
// re: TerrainCell::BuildTexture (TerrainCell.cpp:826-845); TerrainCell::CalcTexCoords (:1831, :1837)
pub fn snapped_scale(scale: f32, axis: u8, range: f32) -> f32 {
    let k = if axis == 0 {
        SNAP_AXIS_ZERO
    } else {
        SNAP_OTHER_AXES
    };
    (scale * k * range).trunc() / range / k
}

/// The projection mode of a normal: 0 flat, else 1 or 2 by the larger of `|ny|`, `|nx|`
/// (2 when `|ny| <= |nx|`).
// re: TerrainCell::BuildTextureVertexBatch (TerrainCell.cpp:229-232)
pub fn projection_mode_of_normal(n: Vec3) -> u8 {
    if n.z > FLAT_NORMAL_Z {
        0
    } else {
        u8::from(n.y.abs() <= n.x.abs()) + 1
    }
}

/// Where a cell is on the field: the origin and the size in texels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellBox {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// The grid of cells of a map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellGrid {
    pub cells_x: i32,
    pub cells_y: i32,
    pub tex_w: i32,
    pub tex_h: i32,
    pub wrap_x: bool,
}

impl CellGrid {
    /// The grid of a frame: `(size + 0x3e) >> 6` cells along each axis.
    // re: Terrain::CalcHeightMap (Terrain.cpp:449)
    pub fn new(frame: &TerrainFrame) -> Self {
        let (w, h) = frame.texel_size();
        let (w, h) = (w as i32, h as i32);
        Self {
            cells_x: (w + 0x3e) >> 6,
            cells_y: (h + 0x3e) >> 6,
            tex_w: w,
            tex_h: h,
            wrap_x: frame.wrap_x(),
        }
    }

    pub fn len(&self) -> usize {
        (self.cells_x * self.cells_y).max(0) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The index of a cell, row by row. (The original goes column by column.)
    pub fn index(&self, cx: i32, cy: i32) -> usize {
        (cy * self.cells_x + cx) as usize
    }

    pub fn cell_xy(&self, index: usize) -> (i32, i32) {
        (index as i32 % self.cells_x, index as i32 / self.cells_x)
    }

    /// The box of cell `(cx, cy)`: 64 texels apart, 65 wide, less at the end of the map. On a map that
    /// wraps along X the last column is one texel wider: its east edge is the texel `tex_w`, which is
    /// the texel 0 of the next copy, so it shares that edge with the first column as two cells inside
    /// the map share theirs (the heights, the normals and the page there are read wrapped).
    // re: Terrain::CalcHeightMap (Terrain.cpp:451-465) gives `min(remaining, 0x41)`; with it the lattice
    // of the last column ends one texel short of the seam and the void showed through (feedback
    // 2026-10-28). The original's height tiles wrap round the map (Terrain.cpp:354-364); the vertex
    // lattice of `CreateBaseTerrainCellVB` that closes its seam is not read (RW question in the report).
    pub fn cell_box(&self, cx: i32, cy: i32) -> CellBox {
        let (x, y) = (cx << 6, cy << 6);
        let (rx, ry) = (self.tex_w - x, self.tex_h - y);
        let close = i32::from(self.wrap_x && rx < 0x41);
        CellBox {
            x,
            y,
            w: if rx < 0x42 { rx + close } else { 0x41 },
            h: if ry < 0x42 { ry } else { 0x41 },
        }
    }

    /// The cell the texel `(u, v)` is in the grid of, clamped to the map.
    pub fn cell_of_texel(&self, u: f32, v: f32) -> (i32, i32) {
        (
            ((u / CELL_STEP as f32) as i32).clamp(0, self.cells_x - 1),
            ((v / CELL_STEP as f32) as i32).clamp(0, self.cells_y - 1),
        )
    }
}

/// The bounds of a cell in the original's world (the shaders' and the view's basis).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellBounds {
    pub min: Vec3,
    pub max: Vec3,
    pub center: Vec3,
    pub radius: f32,
}

impl CellBounds {
    /// The box of the cell: its texels times the scale less half the world, and the height of raw
    /// 255 on top.
    // re: TerrainCell::SetBuildParameters (TerrainCell.cpp:1387-1413)
    pub fn new(frame: &TerrainFrame, b: &CellBox) -> Self {
        let scale = frame.terrain_scale();
        let half = frame.half_world();
        let min = Vec3::new(
            b.x as f32 * scale.x - half.x,
            b.y as f32 * scale.y - half.y,
            0.0,
        );
        let max = Vec3::new(
            b.w as f32 * scale.x + min.x,
            b.h as f32 * scale.y + min.y,
            255.0 * scale.z,
        );
        let center = (min + max) * 0.5;
        let d = max - min;
        Self {
            min,
            max,
            center,
            radius: 0.5 * d.length(),
        }
    }

    /// Whether the cell is inside the six planes of a view (a point is inside when
    /// `dot(plane.xyz, p) + plane.w >= -radius` for every plane), and the six distances.
    // re: TerrainCell::IsVisible (TerrainCell.cpp:1107-1131)
    pub fn is_visible(&self, planes: &[Vec4; 6]) -> (bool, [f32; 6]) {
        let mut d = [0.0; 6];
        let mut visible = true;
        for (i, p) in planes.iter().enumerate() {
            d[i] = p.y * self.center.y + p.x * self.center.x + p.z * self.center.z + p.w;
            if d[i] < -self.radius {
                visible = false;
            }
        }
        (visible, d)
    }

    /// Width of the cell on the screen in pixels: the longest of the four diagonals of its box,
    /// half of it, times half the screen width.
    // re: TerrainCell::CalcMaxWidth (TerrainCell.cpp:1630-1657)
    pub fn max_width_px(&self, view_proj: &Mat4, screen_width: f32) -> f32 {
        let (lo, hi) = (self.min, self.max);
        let pairs = [
            (Vec3::new(hi.x, hi.y, hi.z), Vec3::new(lo.x, lo.y, lo.z)),
            (Vec3::new(hi.x, hi.y, lo.z), Vec3::new(lo.x, lo.y, hi.z)),
            (Vec3::new(hi.x, lo.y, lo.z), Vec3::new(lo.x, hi.y, hi.z)),
            (Vec3::new(hi.x, lo.y, hi.z), Vec3::new(lo.x, hi.y, lo.z)),
        ];
        let project = |p: Vec3| {
            let c = *view_proj * p.extend(1.0);
            Vec2::new(c.x / c.w, c.y / c.w)
        };
        let longest = pairs
            .iter()
            .map(|&(a, b)| (project(a) - project(b)).length() * 0.5)
            .fold(0.0f32, f32::max);
        longest * screen_width * 0.5
    }
}

/// The size of the page a cell of this width on the screen wants: wider than 256 pixels 512, than
/// 128 256, than 64 128, else 64; and not below the size of the cache pages.
// re: TerrainCell::CalcMIPLevelNeeded (TerrainCell.cpp:1660-1674)
pub fn page_size_for_width(width_px: f32, cache_size: u32) -> u32 {
    let size = if width_px > 256.0 {
        512
    } else if width_px > 128.0 {
        256
    } else if width_px > 64.0 {
        128
    } else {
        64
    };
    size.max(cache_size)
}

/// What the height of a cell says about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatsClass {
    /// 1: no ground (the field is empty there).
    Empty = 1,
    /// 2: the ground is below the line of the sea floor.
    Low = 2,
    /// 3: land or shallow water.
    Land = 3,
}

impl StatsClass {
    /// Cells of class 1 and 2 are not drawn, not built by the automaton and not hit by a ray.
    // re: Terrain::LaunchJobs (Terrain.cpp:976), Terrain::IntersectRay (:1261)
    pub fn is_drawn(self) -> bool {
        self == Self::Land
    }
}

/// `CalculateStats`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellStats {
    pub max_step: f32,
    pub max_height: f32,
    pub class: StatsClass,
}

impl Default for CellStats {
    /// A cell that has not been looked at is taken as land, so it is not left out.
    fn default() -> Self {
        Self {
            max_step: 0.0,
            max_height: 0.0,
            class: StatsClass::Land,
        }
    }
}

/// The statistics of the cell: the largest height (in raw / 255) over the texels from `+4` of the
/// origin, and the class from it (< 2 empty, < 30 low, else land).
// re: TerrainCell::CalculateStats (TerrainCell.cpp:1161-1196). The step the original keeps is the
// largest of the heights and the steps (it compares `fMaxStep` with `fHeightRight`); kept as it is.
pub fn calculate_stats(sampler: &TerrainSampler, b: &CellBox) -> CellStats {
    let field = sampler.field();
    let (mw, mh) = (field.width() as i32, field.height() as i32);
    let sample = |x: i32, y: i32| {
        let (x, y) = (x.clamp(0, mw - 1), y.clamp(0, mh - 1));
        f32::from(sampler.raw_height(x, y)) * HEIGHT_TO_UNIT
    };
    let (mut max_step, mut max_height) = (0.0f32, 0.0f32);
    for iy in (b.y + 4)..(b.y + b.h + 3) {
        for ix in (b.x + 4)..(b.x + b.w + 2) {
            let row = iy.rem_euclid(mh);
            let row_next = (iy + 1).rem_euclid(mh);
            let col = ix.rem_euclid(mw);
            let col_next = (ix + 1).rem_euclid(mw);
            let height = sample(col, row);
            let right = sample(col_next, row);
            let up = sample(col, row_next);
            let step = (height - up).abs();
            if max_step <= right {
                max_step = right;
            }
            if max_step <= step {
                max_step = step;
            }
            if max_height <= height {
                max_height = height;
            }
        }
    }
    let class = if STATS_LOW_HEIGHT > max_height {
        StatsClass::Empty
    } else if STATS_MID_HEIGHT > max_height {
        StatsClass::Low
    } else {
        StatsClass::Land
    };
    CellStats {
        max_step,
        max_height,
        class,
    }
}

/// The textures of `TerrainDef` as projections: a texture of the horizontal mode has one entry
/// (one axis), one of the vertical mode has three. The cell blends projections, so its textures
/// are the numbers of projections.
// re: Terrain::SetTextures (Terrain.cpp:1396-1444)
#[derive(Clone, Debug, Default)]
pub struct Projections {
    /// First projection of each texture.
    first: Vec<u32>,
    /// `Projection` column of each texture (0 horizontal, 1 vertical).
    mode: Vec<u8>,
    /// `TerrainDef` scale of each texture.
    scale: Vec<f32>,
    /// The texture has a highlight map.
    spec: Vec<bool>,
    /// The texture and the axis of each projection.
    texture: Vec<u32>,
    axis: Vec<u8>,
}

impl Projections {
    /// The projections of the rows of `TerrainDef`, in the order of the table.
    pub fn from_defs(rows: &[TerrainDefRow]) -> Self {
        let mut p = Self::default();
        for (i, row) in rows.iter().enumerate() {
            p.first.push(p.texture.len() as u32);
            p.mode.push(row.projection);
            p.scale.push(row.texture_scale);
            p.spec.push(row.specmap.is_some());
            let axes = if row.projection == 0 { 1 } else { 3 };
            let mut full = false;
            for axis in 0..axes {
                if p.texture.len() == MAX_PROJECTIONS {
                    full = true;
                    break;
                }
                p.texture.push(i as u32);
                p.axis.push(axis);
            }
            if full {
                // The original stops here; the textures after it have no entry.
                break;
            }
        }
        p
    }

    /// Entries of the table.
    pub fn count(&self) -> usize {
        self.texture.len()
    }

    /// Textures that have a first projection.
    pub fn textures(&self) -> usize {
        self.first.len()
    }

    /// First projection of the texture with this id (0 for an id the table does not have).
    pub fn first(&self, id: u8) -> u32 {
        self.first.get(usize::from(id)).copied().unwrap_or(0)
    }

    pub fn mode(&self, id: u8) -> u8 {
        self.mode.get(usize::from(id)).copied().unwrap_or(0)
    }

    pub fn has_spec_texture(&self, texture: usize) -> bool {
        self.spec.get(texture).copied().unwrap_or(false)
    }

    /// The texture of a projection.
    pub fn texture_of(&self, projection: u32) -> Option<u32> {
        self.texture.get(projection as usize).copied()
    }

    /// The axis of a projection: 0 from above, 1 and 2 on the sides.
    pub fn axis_of(&self, projection: u32) -> Option<u8> {
        self.axis.get(projection as usize).copied()
    }

    /// The `Texture Scale` of the texture of a projection.
    pub fn scale_of(&self, projection: u32) -> f32 {
        self.texture_of(projection)
            .and_then(|t| self.scale.get(t as usize).copied())
            .unwrap_or(0.0)
    }
}

/// The projections a cell blends, at most eight, and how many texels sample each.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextureSet {
    /// Projection numbers (the `aTextures` of a vertex).
    pub ids: Vec<u32>,
    /// Texels (with the weight of the vertical ones counted) that sample each.
    pub counts: Vec<u32>,
}

impl TextureSet {
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

/// `CalculateTextureContributions`: for every texel of the cell with a border of two on the west
/// and north and one on the east and south, the projection its terrain id and its normal pick,
/// counted. The projections that were sampled go in the order of the table; with more than eight
/// the eight most sampled stay.
// re: TerrainCell::CalculateTextureContributions (TerrainCell.cpp:1199-1308). The normal of a
// sample is read at the texel two further (`uCol = sampleX + 2 - x`), as the original does.
pub fn texture_contributions(
    sampler: &TerrainSampler,
    projections: &Projections,
    b: &CellBox,
) -> TextureSet {
    let (origin, size) = contribution_normals(b);
    let normals = sampler.normals_block(origin, size);
    let field = sampler.field();
    let (mw, mh) = (field.width() as i32, field.height() as i32);
    let mut counts = [0u32; 128];
    for sy in (b.y - 2)..=(b.y + b.h) {
        for sx in (b.x - 2)..=(b.x + b.w) {
            let (wx, wy) = ((sx + mw).rem_euclid(mw), (mh + sy).rem_euclid(mh));
            let id = field.get_id(wx, wy);
            let mut slot = projections.first(id) as usize;
            let (col, row) = (sx + 2 - b.x, sy + 2 - b.y);
            let n = if (0..NORMAL_GRID).contains(&col) && (0..NORMAL_GRID).contains(&row) {
                normals[(row * NORMAL_GRID + col) as usize]
            } else {
                Vec3::ZERO
            };
            if projections.mode(id) == 1 {
                if n.z <= FLAT_NORMAL_Z {
                    slot += if n.y.abs() > n.x.abs() { 1 } else { 2 };
                }
                counts[slot] += 1;
                counts[slot + 1] += 1;
                counts[slot + 2] += 1;
            }
            counts[slot] += 1;
        }
    }
    let mut ids = Vec::new();
    let mut weights = Vec::new();
    for (p, &n) in counts.iter().enumerate().take(projections.count()) {
        if n != 0 {
            ids.push(p as u32);
            weights.push(n);
        }
    }
    if ids.len() > MAX_TEXTURES {
        // The most sampled first (a pass of swaps of neighbours), then the first eight.
        loop {
            let mut swapped = false;
            for i in 0..ids.len() - 1 {
                if weights[i] < weights[i + 1] {
                    weights.swap(i, i + 1);
                    ids.swap(i, i + 1);
                    swapped = true;
                }
            }
            if !swapped {
                break;
            }
        }
        ids.truncate(MAX_TEXTURES);
        weights.truncate(MAX_TEXTURES);
    }
    TextureSet {
        ids,
        counts: weights,
    }
}

/// The 5 x 5 kernel of the weights: the product of [`BLEND_KERNEL_1D`] with itself, normalized to
/// a sum of 1, tap `y * 5 + x` at the offset `(x - 2, y - 2)`.
// re: Terrain::SetBlendWeights (Terrain.cpp:424-441)
#[derive(Clone, Debug, PartialEq)]
pub struct BlendKernel {
    pub offsets: [(i32, i32); 25],
    pub weights: [f32; 25],
}

impl BlendKernel {
    pub fn new() -> Self {
        let mut offsets = [(0, 0); 25];
        let mut weights = [0.0f32; 25];
        let mut total = 0.0f32;
        for (y, &ky) in BLEND_KERNEL_1D.iter().enumerate() {
            for (x, &kx) in BLEND_KERNEL_1D.iter().enumerate() {
                let i = y * 5 + x;
                let w = kx * ky;
                weights[i] = w;
                total += w;
                offsets[i] = (x as i32 - 2, y as i32 - 2);
            }
        }
        for w in &mut weights {
            *w /= total;
        }
        Self { offsets, weights }
    }
}

impl Default for BlendKernel {
    fn default() -> Self {
        Self::new()
    }
}

/// The share of each projection around a texel: the 25 taps, each adds its weight to the
/// projection of the terrain id at that tap (for a vertical texture shifted by the projection mode
/// of the normal), and the shares are scaled to a sum of 1.
// re: TerrainCell::CalcBlendVector (TerrainCell.cpp:383-417). The taps are wrapped with unsigned
// arithmetic, so a tap west of texel 0 comes from `(2^32 - n) mod width`, not from the east edge. On
// a map that wraps along X that put a strip of the textures of another column on the west edge of
// the map, against the east edge of the copy beside it (the seam of the feedback of 2026-10-28), so
// there a tap wraps by the period of the field (`rem_euclid`); a map that does not wrap keeps the
// unsigned form. (Whether the Windows build has the same unsigned taps: RW question in the report.)
pub fn calc_blend_vector(
    sampler: &TerrainSampler,
    projections: &Projections,
    kernel: &BlendKernel,
    mode: u8,
    texel: (i32, i32),
) -> [f32; MAX_PROJECTIONS] {
    let field = sampler.field();
    let wrap_x = sampler.frame().wrap_x();
    blend_of_ids(projections, kernel, mode, |ox, oy| {
        let (tx, ty) = blend_tap(
            field.width(),
            field.height(),
            wrap_x,
            texel.0 + ox,
            texel.1 + oy,
        );
        field.get_id(tx, ty)
    })
}

/// The texel a tap of [`calc_blend_vector`] reads for the texel `(x, y)` (the tap's offset added).
fn blend_tap(mw: u32, mh: u32, wrap_x: bool, x: i32, y: i32) -> (i32, i32) {
    let tx = if wrap_x {
        x.rem_euclid(mw as i32)
    } else {
        (x as u32 % mw) as i32
    };
    (tx, (y as u32 % mh) as i32)
}

/// The ids of the field around a block of texels, read once for the blend vectors of its vertices: the
/// taps of [`calc_blend_vector`] (two texels on each side), wrapped as it wraps them.
struct IdWindow {
    x0: i32,
    y0: i32,
    w: i32,
    ids: Vec<u8>,
}

impl IdWindow {
    fn new(sampler: &TerrainSampler, origin: (i32, i32), size: (i32, i32)) -> Self {
        let field = sampler.field();
        let wrap_x = sampler.frame().wrap_x();
        let (x0, y0) = (origin.0 - 2, origin.1 - 2);
        let (w, h) = (size.0 + 4, size.1 + 4);
        let mut ids = Vec::with_capacity((w * h).max(0) as usize);
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                let (tx, ty) = blend_tap(field.width(), field.height(), wrap_x, x, y);
                ids.push(field.get_id(tx, ty));
            }
        }
        Self { x0, y0, w, ids }
    }

    fn get(&self, x: i32, y: i32) -> u8 {
        self.ids[((y - self.y0) * self.w + (x - self.x0)) as usize]
    }
}

/// [`calc_blend_vector`] with the ids of its taps from `id_at(ox, oy)`.
fn blend_of_ids(
    projections: &Projections,
    kernel: &BlendKernel,
    mode: u8,
    id_at: impl Fn(i32, i32) -> u8,
) -> [f32; MAX_PROJECTIONS] {
    let mut blend = [0.0f32; MAX_PROJECTIONS];
    for (i, &(ox, oy)) in kernel.offsets.iter().enumerate() {
        let id = id_at(ox, oy);
        let mut p = projections.first(id) as usize;
        if mode != 0 && projections.mode(id) == 1 {
            p += match mode {
                2 => 2,
                1 => 1,
                _ => 0,
            };
        }
        if let Some(slot) = blend.get_mut(p) {
            *slot += kernel.weights[i];
        }
    }
    let n = projections.count().min(MAX_PROJECTIONS);
    if n != 0 {
        let sum: f32 = blend[..n].iter().sum();
        if sum > 0.0 {
            let inv = 1.0 / sum;
            for b in &mut blend[..n] {
                *b *= inv;
            }
        }
    }
    blend
}

/// A vertex of the composite buffer of a cell (`VStruct<21>`, 48 bytes in the original).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompositeVertex {
    /// Position in the clip space of the cell's page.
    pub clip: Vec3,
    /// The texel of the field.
    pub hex: Vec2,
    /// The world position: texel times the scale, and the height.
    pub world: Vec3,
    /// `w * 255` of each of the eight textures.
    pub weights: [u8; 8],
    /// The projections that are blended.
    pub textures: [u8; 8],
}

/// A vertex of the compressed buffer (`VStruct<16>`, 24 bytes): position, normal and texture
/// coordinate as the original packs them (two halves in a dword).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompressedVertex {
    pub position: Vec3,
    pub normal_xy: u32,
    pub normal_zw: u32,
    pub texcoord: u32,
}

/// Two floats as two halves in a dword, the low one first.
// re: TerrainCell.cpp:102-107 (PackHalves)
pub fn pack_halves(low: f32, high: f32) -> u32 {
    u32::from(f32_to_f16_bits(low)) | (u32::from(f32_to_f16_bits(high)) << 16)
}

/// The vertices of a cell: `(w + 2) x (h + 2)`, the column and row `-1` and `w`, `h` included.
#[derive(Clone, Debug, PartialEq)]
pub struct CellVertices {
    pub width: i32,
    pub height: i32,
    pub composite: Vec<CompositeVertex>,
    pub compressed: Vec<CompressedVertex>,
}

impl CellVertices {
    /// The index of the vertex at `(ix, iy)`, `-1 <= ix <= w`.
    // re: TerrainCell::BuildTextureVertexBatch (TerrainCell.cpp:241: `(iY + 1) * (iWidth + 2) + (iX + 1)`)
    pub fn index(&self, ix: i32, iy: i32) -> usize {
        ((iy + 1) * self.width + (ix + 1)) as usize
    }
}

/// What the vertices need besides the field.
pub struct VertexInputs<'a> {
    pub sampler: &'a TerrainSampler,
    pub projections: &'a Projections,
    pub kernel: &'a BlendKernel,
    /// The projections of the cell (`CalculateTextureContributions`).
    pub textures: &'a TextureSet,
    /// Size of the page the cell is baked into (the texel of the page is its inverse).
    pub page_size: u32,
    /// `g_bUseGPUWeightBlending`: the weights are not made here (RW-7).
    pub gpu_weights: bool,
}

/// The vertices of a cell: the clip position of the page, the texel, the world position, the
/// weights and the numbers of the textures (`BuildTextureVertexBatch`), and the packed normals.
// re: TerrainCell::BuildTextureVertexBatch (TerrainCell.cpp:177-298). The normals of the cell come
// from one block (`normals_block`), the original asks the sampler for those outside a block of 13.
pub fn build_cell_vertices(inp: &VertexInputs<'_>, b: &CellBox) -> CellVertices {
    let sampler = inp.sampler;
    let scale = sampler.frame().terrain_scale();
    let (width, height) = (b.w + 2, b.h + 2);
    let (origin, size) = vertex_normals(b);
    debug_assert_eq!(size, (width, height));
    let normals = sampler.normals_block(origin, size);
    let ids =
        (!inp.gpu_weights).then(|| IdWindow::new(sampler, (b.x - 1, b.y - 1), (width, height)));
    let (mw, mh) = (
        sampler.field().width() as i32,
        sampler.field().height() as i32,
    );
    let texel = 1.0 / inp.page_size.max(1) as f32;
    let (fw, fh) = ((b.w + 1) as f32, (b.h + 1) as f32);
    let mut composite = Vec::with_capacity((width * height) as usize);
    let mut compressed = Vec::with_capacity((width * height) as usize);
    for iy in -1..=b.h {
        for ix in -1..=b.w {
            let (hx, hy) = (b.x + ix, b.y + iy);
            let normal = normals[((iy + 1) * width + (ix + 1)) as usize];
            let mode = projection_mode_of_normal(normal);
            let (wx, wy) = ((mw + hx).rem_euclid(mw), (mh + hy).rem_euclid(mh));
            let world_z = f32::from(sampler.raw_height(wx, wy)) * HEIGHT_TO_UNIT * scale.z;
            let u = (ix + 1) as f32 / fw;
            let v = (iy + 1) as f32 / fh;
            let clip_x = u + u - 1.0;
            let clip_y = -2.0 * v + 1.0;
            compressed.push(CompressedVertex {
                position: Vec3::new(clip_x - texel, texel + clip_y, 0.5),
                normal_xy: pack_halves(normal.x, normal.y),
                normal_zw: pack_halves(normal.z, 1.0),
                texcoord: pack_halves(u, v),
            });
            let mut weights = [0u8; 8];
            let mut textures = [0u8; 8];
            if let Some(ids) = ids.as_ref() {
                let blend = blend_of_ids(inp.projections, inp.kernel, mode, |ox, oy| {
                    ids.get(hx + ox, hy + oy)
                });
                let mut w = [0.0f32; 8];
                if inp.textures.is_empty() {
                    w[0] = 1.0;
                } else {
                    let mut sum = 0.0f32;
                    for (i, &p) in inp.textures.ids.iter().take(8).enumerate() {
                        textures[i] = p as u8;
                        w[i] = blend.get(p as usize).copied().unwrap_or(0.0);
                        sum += w[i];
                    }
                    if f64::from(sum) < MIN_WEIGHT_SUM {
                        w[0] = 1.0;
                    }
                }
                for (o, f) in weights.iter_mut().zip(w) {
                    *o = (f * WEIGHT_SCALE) as u32 as u8;
                }
            } else {
                textures[0] = mode;
            }
            composite.push(CompositeVertex {
                clip: Vec3::new(clip_x, clip_y, 0.5),
                hex: Vec2::new(hx as f32, hy as f32),
                world: Vec3::new(scale.x * hx as f32, scale.y * hy as f32, world_z),
                weights,
                textures,
            });
        }
    }
    CellVertices {
        width,
        height,
        composite,
        compressed,
    }
}

/// Whether a segment touches a box: an end is inside, or the segment crosses an edge.
// re: TerrainCell::IntersectLine (TerrainCell.cpp:1847-1876)
pub fn segment_touches_box(p0: Vec2, p1: Vec2, min: Vec2, max: Vec2) -> bool {
    let inside = |p: Vec2| !(p.x < min.x || max.x < p.x || p.y < min.y || max.y < p.y);
    if inside(p0) || inside(p1) {
        return true;
    }
    let corners = [
        Vec2::new(min.x, min.y),
        Vec2::new(max.x, min.y),
        Vec2::new(max.x, max.y),
        Vec2::new(min.x, max.y),
    ];
    (0..4).any(|i| segments_cross(p0, p1, corners[i], corners[(i + 1) % 4]))
}

fn segments_cross(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> bool {
    let cross = |o: Vec2, p: Vec2, q: Vec2| (p.x - o.x) * (q.y - o.y) - (p.y - o.y) * (q.x - o.x);
    let (d1, d2) = (cross(c, d, a), cross(c, d, b));
    let (d3, d4) = (cross(a, b, c), cross(a, b, d));
    d1 * d2 <= 0.0 && d3 * d4 <= 0.0 && !(d1 == 0.0 && d2 == 0.0 && d3 == 0.0 && d4 == 0.0)
}

/// One cell of the terrain.
#[derive(Clone, Debug)]
pub struct TerrainCell {
    pub index: usize,
    pub rect: CellBox,
    pub bounds: CellBounds,
    /// The hexes whose outline touches the cell.
    pub hexes: Vec<(i32, i32)>,
    /// A bit per continent of the hexes (`1 << continent`).
    pub class_mask: u32,
    /// Bit 0: a hex that is seen now, bit 1: one revealed and not seen. Zero: nothing open.
    pub fow_flags: u8,
    pub stats: CellStats,
    pub textures: TextureSet,
    /// The region was invalidated (a decal with a highlight): the cell has the highlight page.
    pub force_spec: bool,
    /// The cell has the highlight page.
    pub has_spec: bool,
}

impl TerrainCell {
    /// The class of the style of the cell, 0..=4.
    pub fn class(&self) -> u8 {
        class_from_mask(self.class_mask)
    }

    /// Whether a hex of the cell is open.
    pub fn is_open(&self) -> bool {
        self.fow_flags != 0
    }
}

/// The value of a fog state for the flags: seen 1, revealed and not seen 2, unrevealed 0.
fn fow_bits(fog: Fog) -> u8 {
    match fog {
        Fog::Visible => 1,
        Fog::Hidden => 2,
        Fog::Unrevealed => 0,
    }
}

/// All the cells of a map.
#[derive(Clone, Debug)]
pub struct TerrainCells {
    frame: TerrainFrame,
    grid: CellGrid,
    cells: Vec<TerrainCell>,
    width: usize,
    fog: Vec<Fog>,
}

impl TerrainCells {
    /// The cells of a map, with the hexes each touches and the classes of its styles
    /// (`BuildCellToHexTable`, and the mask of `Terrain::Init`).
    // re: Terrain::BuildCellToHexTable (Terrain.cpp:525-599), Terrain::Init (:292-301)
    pub fn new(frame: &TerrainFrame, basis: &TerrainBasis) -> Self {
        let grid = CellGrid::new(frame);
        let mut cells: Vec<TerrainCell> = (0..grid.len())
            .map(|index| {
                let (cx, cy) = grid.cell_xy(index);
                let rect = grid.cell_box(cx, cy);
                TerrainCell {
                    index,
                    rect,
                    bounds: CellBounds::new(frame, &rect),
                    hexes: Vec::new(),
                    class_mask: 0,
                    fow_flags: 0,
                    stats: CellStats::default(),
                    textures: TextureSet::default(),
                    force_spec: false,
                    has_spec: false,
                }
            })
            .collect();
        let (bw, bh) = (i32::from(basis.width), i32::from(basis.height));
        let world_w = frame.world_width();
        for y in 0..bh {
            for x in 0..bw {
                let center = frame.hex_to_tex(x, y);
                let corners: Vec<Vec2> = (0..6)
                    .map(|i| frame.world_to_tex(cell_center(x, y) + corner_offset(i)))
                    .collect();
                let (cell_x, cell_y) = (
                    (center.x / CELL_STEP as f32) as i32,
                    (center.y / CELL_STEP as f32) as i32,
                );
                let mut listed: Vec<usize> = Vec::new();
                for gx in (cell_x - 1)..=(cell_x + 1) {
                    for gy in (cell_y - 1)..=(cell_y + 1) {
                        // The cell the original looks at: the indexes wrap on both axes.
                        let (wx, wy) = (
                            (grid.cells_x + gx).rem_euclid(grid.cells_x),
                            (grid.cells_y + gy).rem_euclid(grid.cells_y),
                        );
                        let index = grid.index(wx, wy);
                        if listed.contains(&index) {
                            continue;
                        }
                        let r = cells[index].rect;
                        let (min, max) = (
                            Vec2::new(r.x as f32, r.y as f32),
                            Vec2::new((r.x + r.w) as f32, (r.y + r.h) as f32),
                        );
                        // A wrapping map: the hex that leans over the seam also touches the cell
                        // on the other side; the outline is moved by a period for that cell.
                        let shift = if grid.wrap_x && wx != gx {
                            (gx - wx) as f32 * CELL_STEP as f32
                        } else {
                            0.0
                        };
                        let touched = (0..6).any(|i| {
                            let a = corners[i] - Vec2::new(shift, 0.0);
                            let b = corners[(i + 1) % 6] - Vec2::new(shift, 0.0);
                            segment_touches_box(a, b, min, max)
                        });
                        if touched {
                            listed.push(index);
                        }
                    }
                }
                let _ = world_w;
                let continent = basis.plot(x, y).map_or(0, |p| p.continent);
                for index in listed {
                    cells[index].hexes.push((x, y));
                    cells[index].class_mask |= class_bit(continent);
                }
            }
        }
        Self {
            frame: *frame,
            grid,
            cells,
            width: usize::from(basis.width),
            fog: Vec::new(),
        }
    }

    pub fn grid(&self) -> &CellGrid {
        &self.grid
    }

    pub fn frame(&self) -> &TerrainFrame {
        &self.frame
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn cell(&self, index: usize) -> &TerrainCell {
        &self.cells[index]
    }

    pub fn cell_mut(&mut self, index: usize) -> &mut TerrainCell {
        &mut self.cells[index]
    }

    pub fn cell_at(&self, cx: i32, cy: i32) -> Option<&TerrainCell> {
        ((0..self.grid.cells_x).contains(&cx) && (0..self.grid.cells_y).contains(&cy))
            .then(|| &self.cells[self.grid.index(cx, cy)])
    }

    pub fn iter(&self) -> impl Iterator<Item = &TerrainCell> {
        self.cells.iter()
    }

    /// The cells as a slice, in the order of the grid (the cache reads them so).
    pub fn as_slice(&self) -> &[TerrainCell] {
        &self.cells
    }

    fn flags_of(&self, cell: &TerrainCell) -> u8 {
        cell.hexes
            .iter()
            .map(|&(x, y)| {
                self.fog
                    .get(y as usize * self.width + x as usize)
                    .copied()
                    .map_or(0, fow_bits)
            })
            .fold(0, |a, b| a | b)
    }

    /// The fog of every hex (`OnFOWChanged` for the whole map and `SetGamePlayCell` for every
    /// cell). Returns the cells whose flags changed.
    // re: Terrain::OnFOWChanged (Terrain.cpp:471-514), Terrain::SetGamePlayCell (:1559-1597)
    pub fn set_fog(&mut self, fog: &[Fog]) -> Vec<usize> {
        if self.width == 0 || !fog.len().is_multiple_of(self.width) {
            return Vec::new();
        }
        self.fog = fog.to_vec();
        let mut changed = Vec::new();
        for i in 0..self.cells.len() {
            let flags = self.flags_of(&self.cells[i]);
            if self.cells[i].fow_flags != flags {
                self.cells[i].fow_flags = flags;
                changed.push(i);
            }
        }
        changed
    }

    /// The fog of one hex. Only the cells around the cell of the hex are looked at again: the
    /// columns -1..=+1 and the rows -2..=0.
    // re: Terrain::SetGamePlayCell (Terrain.cpp:1580-1596: "the window is asymmetric")
    pub fn set_fog_hex(&mut self, x: i32, y: i32, fog: Fog) -> Vec<usize> {
        let i = y as usize * self.width + x as usize;
        if self.fog.len() <= i {
            self.fog.resize(i + 1, Fog::Unrevealed);
        }
        self.fog[i] = fog;
        let c = self.frame.hex_to_tex(x, y);
        let (cell_x, cell_y) = (
            (c.x / CELL_STEP as f32) as i32,
            (c.y / CELL_STEP as f32) as i32,
        );
        let mut changed = Vec::new();
        for gx in (cell_x - 1)..=(cell_x + 1) {
            for gy in (cell_y - 2)..=cell_y {
                let (wx, wy) = (
                    (self.grid.cells_x + gx).rem_euclid(self.grid.cells_x),
                    (self.grid.cells_y + gy).rem_euclid(self.grid.cells_y),
                );
                let index = self.grid.index(wx, wy);
                let flags = self.flags_of(&self.cells[index]);
                if self.cells[index].fow_flags != flags {
                    self.cells[index].fow_flags = flags;
                    changed.push(index);
                }
            }
        }
        changed
    }

    /// Statistics and contributions of one cell (the stat job of the original).
    // re: TerrainCell::LaunchStatsBuild (TerrainCell.cpp:1311-1314)
    pub fn recompute(
        &mut self,
        index: usize,
        sampler: &TerrainSampler,
        projections: &Projections,
        spec_enabled: bool,
    ) {
        let rect = self.cells[index].rect;
        let stats = calculate_stats(sampler, &rect);
        let textures = texture_contributions(sampler, projections, &rect);
        let cell = &mut self.cells[index];
        cell.stats = stats;
        cell.has_spec = spec_enabled
            && (cell.force_spec
                || textures.ids.iter().any(|&p| {
                    projections
                        .texture_of(p)
                        .is_some_and(|t| projections.has_spec_texture(t as usize))
                }));
        cell.textures = textures;
    }

    /// As [`Self::recompute`] for the live cache: a cell that is not drawn (no ground, or below the
    /// sea floor) gets its statistics only. Its textures and its highlight are never read (it has no
    /// page), and the contributions read a 69 x 69 block of normals: with the whole map open most
    /// cells are sea, and that was most of the CPU of a frame while the field built.
    /// Returns whether the textures were made.
    pub fn recompute_live(
        &mut self,
        index: usize,
        sampler: &TerrainSampler,
        projections: &Projections,
        spec_enabled: bool,
    ) -> bool {
        let rect = self.cells[index].rect;
        let stats = calculate_stats(sampler, &rect);
        if stats.class.is_drawn() {
            self.recompute(index, sampler, projections, spec_enabled);
            return true;
        }
        let cell = &mut self.cells[index];
        cell.stats = stats;
        cell.has_spec = false;
        cell.textures = TextureSet::default();
        false
    }

    /// `PruneWork`: the statistics and the textures of every cell.
    // re: Terrain::PruneWork (Terrain.cpp:1388-1393)
    pub fn prune_work(
        &mut self,
        sampler: &TerrainSampler,
        projections: &Projections,
        spec_enabled: bool,
    ) {
        for i in 0..self.cells.len() {
            self.recompute(i, sampler, projections, spec_enabled);
        }
    }

    /// The cells a region of texels touches (clipped to the map, no wrap), as `InvalidateRegion`
    /// takes them, for a caller that makes them again later: they get the highlight page and keep
    /// their textures until [`Self::recompute`].
    pub fn touch_region(&mut self, min: Vec2, max: Vec2) -> Vec<usize> {
        let cells = self.invalidate_region_cells(min, max);
        for &i in &cells {
            // The textures stay until the cell is made again (`recompute` reads the force).
            self.cells[i].force_spec = true;
        }
        cells
    }

    /// The cells an area of the texture plane meets.
    fn invalidate_region_cells(&self, min: Vec2, max: Vec2) -> Vec<usize> {
        let g = self.grid;
        let cx0 = ((min.x / CELL_STEP as f32) as i32).max(0);
        let cy0 = ((min.y / CELL_STEP as f32) as i32).max(0);
        let cx1 = ((max.x / CELL_STEP as f32) as i32).min(g.cells_x - 1);
        let cy1 = ((max.y / CELL_STEP as f32) as i32).min(g.cells_y - 1);
        let mut out = Vec::new();
        for cx in cx0..=cx1 {
            for cy in cy0..=cy1 {
                out.push(g.index(cx, cy));
            }
        }
        out
    }

    /// [`Self::touch_region`], the cells forgetting their textures too.
    // re: Terrain::InvalidateRegion (Terrain.cpp:1352-1385)
    pub fn invalidate_region(&mut self, min: Vec2, max: Vec2) -> Vec<usize> {
        let out = self.touch_region(min, max);
        for &i in &out {
            self.cells[i].textures = TextureSet::default();
        }
        out
    }
}

/// How many texels a hex is wide: for the tests of the hex tables.
pub const HEX_TEXELS: i32 = TEXELS_PER_HEX as i32;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::{Relief, TerrainKind};
    use crate::world::terrain::Terrain;
    use crate::world::terrain::basis::PlotBasis;
    use crate::world::terrain::library::TerrainLibrary;
    use asset_format::LogicalId;
    use std::sync::Arc;

    fn plot(relief: Relief, terrain: TerrainKind, continent: u8) -> PlotBasis {
        PlotBasis {
            relief,
            terrain,
            continent,
            river_flow: PlotBasis::NO_FLOW,
            ..PlotBasis::default()
        }
    }

    fn row(projection: u8, spec: bool) -> TerrainDefRow {
        TerrainDefRow {
            continent: 0,
            diffuse: Some(LogicalId::new("tx:t.dds")),
            heightmap: None,
            specmap: spec.then(|| LogicalId::new("tx:s.dds")),
            base_dominance: 0,
            texture_scale: 1.0,
            projection,
            id: 0,
        }
    }

    /// The map used by most tests: 6 x 6 with every relief, built from the synthetic pieces.
    fn built_terrain() -> Terrain {
        let mut basis =
            TerrainBasis::uniform(6, 6, false, 3, plot(Relief::Flat, TerrainKind::Grass, 0));
        for (x, y, r, k) in [
            (1, 1, Relief::Hill, TerrainKind::Grass),
            (2, 2, Relief::Mountain, TerrainKind::Plains),
            (3, 3, Relief::Water, TerrainKind::Coast),
            (4, 1, Relief::Mountain, TerrainKind::Plains),
        ] {
            let i = basis.index(x, y).unwrap();
            basis.plots[i] = plot(r, k, 0);
        }
        let mut t = Terrain::default();
        t.set_library(Arc::new(TerrainLibrary::synthetic()));
        t.set_basis(basis);
        t.build_all();
        t
    }

    #[test]
    fn the_blend_kernel_is_five_by_five_and_sums_to_one() {
        let k = BlendKernel::new();
        let sum: f32 = k.weights.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6, "{sum}");
        assert_eq!(k.offsets[0], (-2, -2));
        assert_eq!(k.offsets[12], (0, 0));
        assert_eq!(k.offsets[24], (2, 2));
        // The middle tap is the largest, the corners the smallest, and it is symmetric.
        let max = k.weights.iter().cloned().fold(0.0f32, f32::max);
        assert_eq!(k.weights[12], max);
        let min = k.weights.iter().cloned().fold(1.0f32, f32::min);
        assert_eq!(k.weights[0], min);
        for y in 0..5 {
            for x in 0..5 {
                assert_eq!(k.weights[y * 5 + x], k.weights[x * 5 + y]);
                assert_eq!(k.weights[y * 5 + x], k.weights[(4 - y) * 5 + (4 - x)]);
            }
        }
        // The first and the middle tap by the formula: 0.125 * 0.125 / total.
        let total: f32 = BLEND_KERNEL_1D.iter().sum::<f32>().powi(2);
        assert!((k.weights[0] - 0.125 * 0.125 / total).abs() < 1e-7);
    }

    #[test]
    fn the_grid_has_cells_of_64_and_65_texels() {
        let frame = TerrainFrame::new(4, 3, false);
        let g = CellGrid::new(&frame);
        assert_eq!((g.cells_x, g.cells_y, g.len()), (4, 3, 12));
        assert_eq!(
            g.cell_box(0, 0),
            CellBox {
                x: 0,
                y: 0,
                w: 65,
                h: 65
            }
        );
        // The last cell has what is left: 256 - 192 = 64.
        assert_eq!(
            g.cell_box(3, 2),
            CellBox {
                x: 192,
                y: 128,
                w: 64,
                h: 64
            }
        );
        assert_eq!(g.cell_box(2, 1).w, 65);
        assert_eq!((g.index(2, 1), g.cell_xy(g.index(2, 1))), (6, (2, 1)));
        assert_eq!(g.cell_of_texel(130.0, 70.0), (2, 1));
        assert_eq!(g.cell_of_texel(-5.0, 9000.0), (0, 2));
    }

    #[test]
    fn the_class_is_the_lowest_bit_of_one_to_four() {
        assert_eq!(class_from_mask(0), 0);
        assert_eq!(class_from_mask(class_bit(0)), 0);
        assert_eq!(class_from_mask(class_bit(3)), 3);
        assert_eq!(class_from_mask(class_bit(3) | class_bit(4)), 3);
        assert_eq!(
            class_from_mask(class_bit(1) | class_bit(4) | class_bit(0)),
            1
        );
        // A bit above 4 is no class.
        assert_eq!(class_from_mask(1 << 9), 0);
    }

    #[test]
    fn the_projection_of_a_normal_follows_the_threshold_of_point_nine() {
        let n = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
        assert_eq!(projection_mode_of_normal(n(0.0, 0.0, 1.0)), 0);
        assert_eq!(projection_mode_of_normal(n(0.1, 0.1, 0.9001)), 0);
        // Exactly 0.9 is not flat.
        assert_ne!(projection_mode_of_normal(n(0.3, 0.1, 0.9)), 0);
        // The axis: 2 when |ny| <= |nx|, else 1.
        assert_eq!(projection_mode_of_normal(n(0.8, 0.2, 0.5)), 2);
        assert_eq!(projection_mode_of_normal(n(0.5, 0.5, 0.0)), 2);
        assert_eq!(projection_mode_of_normal(n(0.2, 0.8, 0.5)), 1);
    }

    #[test]
    fn the_scale_of_a_texture_snaps_to_whole_units_of_the_world() {
        // 2560 world units: 1.0 * 0.33 * 2560 = 844.8 -> 844 / 2560 / 0.33.
        let s = snapped_scale(1.0, 0, 2560.0);
        assert!((s - 844.0 / 2560.0 / 0.33).abs() < 1e-6);
        let s = snapped_scale(1.0, 1, 2560.0);
        assert!((s - 1689.0 / 2560.0 / 0.66).abs() < 1e-5, "{s}");
        // The snapped scale tiles the world: scale * range * k is a whole number.
        assert!(
            (snapped_scale(0.7, 2, 3000.0) * 0.66 * 3000.0)
                .fract()
                .abs()
                < 1e-3
        );
    }

    #[test]
    fn projections_take_one_entry_for_flat_and_three_for_vertical_textures() {
        let p = Projections::from_defs(&[row(0, false), row(1, true), row(0, false)]);
        assert_eq!((p.textures(), p.count()), (3, 5));
        assert_eq!((p.first(0), p.first(1), p.first(2)), (0, 1, 4));
        assert_eq!((p.mode(0), p.mode(1)), (0, 1));
        assert_eq!(
            (p.texture_of(1), p.texture_of(3), p.texture_of(4)),
            (Some(1), Some(1), Some(2))
        );
        assert_eq!(
            (p.axis_of(1), p.axis_of(2), p.axis_of(3)),
            (Some(0), Some(1), Some(2))
        );
        assert!(p.has_spec_texture(1) && !p.has_spec_texture(0));
        // An id the table does not have is texture 0.
        assert_eq!((p.first(200), p.mode(200)), (0, 0));
        // At most 64 entries: 30 vertical textures take 90 entries, 21 of them fit.
        let many: Vec<_> = (0..30).map(|_| row(1, false)).collect();
        let p = Projections::from_defs(&many);
        assert_eq!(p.count(), MAX_PROJECTIONS);
        assert_eq!(p.textures(), 22);
    }

    /// The blend vectors of the vertices come from one window of ids: the same numbers as the taps of
    /// `calc_blend_vector` one by one, over the edges and the seam, with vertical textures.
    #[test]
    fn the_blend_of_a_window_is_the_blend_of_the_taps() {
        let projections =
            Projections::from_defs(&[row(0, false), row(1, true), row(0, false), row(1, false)]);
        let kernel = BlendKernel::new();
        for wrap in [false, true] {
            let mut s = TerrainSampler::new(TerrainFrame::new(4, 4, wrap));
            let mut seed = 99u32;
            for v in 0..256 {
                for u in 0..256 {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    s.field_mut().set_id(u, v, ((seed >> 16) % 4) as u8);
                }
            }
            for (origin, size) in [
                ((-1, -1), (10, 9)),
                ((250, 120), (9, 7)),
                ((60, 250), (6, 8)),
            ] {
                let ids = IdWindow::new(&s, origin, size);
                for y in origin.1..origin.1 + size.1 {
                    for x in origin.0..origin.0 + size.0 {
                        for mode in 0..3 {
                            let want = calc_blend_vector(&s, &projections, &kernel, mode, (x, y));
                            let got = blend_of_ids(&projections, &kernel, mode, |ox, oy| {
                                ids.get(x + ox, y + oy)
                            });
                            assert_eq!(got, want, "{wrap} {x} {y} {mode}");
                        }
                    }
                }
            }
        }
    }

    /// A sampler with the ids of the bands of `widths` along u, then `rest`.
    fn banded_sampler(widths: &[i32], rest: u8) -> TerrainSampler {
        let mut s = TerrainSampler::new(TerrainFrame::new(4, 4, false));
        let mut edge = 0;
        let mut band_of = vec![rest; 256];
        for (id, w) in widths.iter().enumerate() {
            for u in edge..edge + w {
                band_of[u as usize] = id as u8;
            }
            edge += w;
        }
        for v in 0..256 {
            for u in 0..256 {
                s.field_mut().set(u, v, 15_423, band_of[u as usize]);
            }
        }
        s
    }

    #[test]
    fn a_cell_keeps_the_eight_most_sampled_of_nine_textures() {
        // Nine flat textures in bands: id 8 is one column, the rarest. The two columns west of
        // the map come from the east edge, which is id 0.
        let rows: Vec<_> = (0..9).map(|_| row(0, false)).collect();
        let projections = Projections::from_defs(&rows);
        let mut s = banded_sampler(&[18, 12, 10, 8, 6, 5, 3, 2, 1], 7);
        for u in 254..256 {
            for v in 0..256 {
                s.field_mut().set_id(u, v, 0);
            }
        }
        // The column 65 is id 7 again.
        for v in 0..256 {
            s.field_mut().set_id(65, v, 7);
        }
        let b = CellBox {
            x: 0,
            y: 0,
            w: 65,
            h: 65,
        };
        let set = texture_contributions(&s, &projections, &b);
        assert_eq!(set.len(), 8);
        assert!(!set.ids.contains(&8), "{:?}", set.ids);
        assert_eq!(set.ids, [0, 1, 2, 3, 4, 5, 6, 7]);
        // 68 rows each: id 0 has the 18 columns and the two of the east edge.
        assert_eq!(set.counts[0], 20 * 68);
        assert_eq!(set.counts[1], 12 * 68);
        // With fewer than nine the order is the order of the table, not of the counts.
        let few = Projections::from_defs(&rows[..3]);
        let s3 = banded_sampler(&[10, 30, 20], 2);
        let set = texture_contributions(&s3, &few, &b);
        assert_eq!(set.ids, [0, 1, 2]);
    }

    #[test]
    fn a_vertical_texture_counts_three_axes_and_the_flat_one_once() {
        let projections = Projections::from_defs(&[row(1, false)]);
        let mut s = TerrainSampler::new(TerrainFrame::new(2, 2, false));
        for v in 0..128 {
            for u in 0..128 {
                s.field_mut().set(u, v, 15_423, 0);
            }
        }
        let b = CellBox {
            x: 0,
            y: 0,
            w: 65,
            h: 65,
        };
        // Flat ground: only the first axis is the projection, but the three entries are counted.
        let set = texture_contributions(&s, &projections, &b);
        assert_eq!(set.ids, [0, 1, 2]);
        let n = 68 * 68;
        assert_eq!(set.counts, [2 * n, n, n]);
    }

    #[test]
    fn the_stats_class_follows_the_highest_texel() {
        let b = CellBox {
            x: 0,
            y: 0,
            w: 65,
            h: 65,
        };
        let mut s = TerrainSampler::new(TerrainFrame::new(2, 2, false));
        let class = |s: &TerrainSampler| calculate_stats(s, &b).class;
        // Nothing built: no ground.
        assert_eq!(class(&s), StatsClass::Empty);
        // 400 / 255 = 1.57 < 2.
        s.field_mut().set_height(10, 10, 400);
        assert_eq!(class(&s), StatsClass::Empty);
        s.field_mut().set_height(10, 10, 6000);
        assert_eq!(class(&s), StatsClass::Low);
        s.field_mut().set_height(11, 10, 8000);
        let stats = calculate_stats(&s, &b);
        assert_eq!(stats.class, StatsClass::Land);
        assert!((stats.max_height - 8000.0 / 255.0).abs() < 1e-3);
        assert!(StatsClass::Land.is_drawn() && !StatsClass::Low.is_drawn());
        // A texel before +4 of the origin is not looked at.
        let mut s = TerrainSampler::new(TerrainFrame::new(2, 2, false));
        s.field_mut().set_height(2, 2, 20_000);
        assert_eq!(class(&s), StatsClass::Empty);
    }

    #[test]
    fn the_hexes_of_the_cells_cover_the_map_and_each_hex_is_in_its_own_cell() {
        let t = built_terrain();
        let frame = *t.frame().unwrap();
        let cells = TerrainCells::new(&frame, t.basis().unwrap());
        assert_eq!(cells.len(), 36);
        for y in 0..6 {
            for x in 0..6 {
                let c = frame.hex_to_tex(x, y);
                let listed: Vec<&TerrainCell> =
                    cells.iter().filter(|c| c.hexes.contains(&(x, y))).collect();
                assert!(!listed.is_empty(), "hex ({x}, {y}) is in no cell");
                // The cell that holds the centre of the hex lists it.
                let (cx, cy) = cells.grid().cell_of_texel(c.x, c.y);
                assert!(
                    cells.cell_at(cx, cy).unwrap().hexes.contains(&(x, y)),
                    "({x}, {y})"
                );
                // And no cell far from it does.
                for cell in &listed {
                    let r = cell.rect;
                    assert!(
                        (r.x as f32 - 80.0..=(r.x + r.w) as f32 + 80.0).contains(&c.x)
                            && (r.y as f32 - 80.0..=(r.y + r.h) as f32 + 80.0).contains(&c.y),
                        "hex ({x}, {y}) in the cell at {r:?}"
                    );
                }
            }
        }
        // Every cell touches a hex.
        assert!(cells.iter().all(|c| !c.hexes.is_empty()));
    }

    /// The seam of a wrapping map: the last column of cells ends on the texel `tex_w`, the texel 0 of
    /// the next copy, and what the cells read there is what the first column reads at 0 (the heights
    /// of the lattice, the normals and the blend of the page).
    #[test]
    fn the_columns_on_both_sides_of_the_seam_share_their_edge() {
        let wrap = TerrainFrame::new(4, 3, true);
        let g = CellGrid::new(&wrap);
        let last = g.cell_box(3, 1);
        assert_eq!((last.x, last.w), (192, 65));
        assert_eq!(
            last.x + last.w - 1,
            g.tex_w,
            "the last vertex is the first of the copy"
        );
        // Inside the map, and on a map that does not wrap, the boxes are the original's.
        assert_eq!(g.cell_box(2, 1).w, 65);
        assert_eq!(
            CellGrid::new(&TerrainFrame::new(4, 3, false))
                .cell_box(3, 1)
                .w,
            64
        );

        // A field that is not flat and of two terrains near the seam.
        let mut s = TerrainSampler::new(wrap);
        for v in 0..192 {
            for u in 0..256 {
                let raw = 13_000 + ((u * 37 + v * 11) % 97) as u16 * 40;
                s.field_mut()
                    .set(u, v, raw, u8::from(!(3..=250).contains(&u)));
            }
        }
        let (tw, v) = (g.tex_w, 70);
        // The heights the lattice of the last column reads at its east edge.
        let surface =
            |b: &CellBox| crate::world::terrain::heights_gpu::HeightTiles::cell_surface(&s, b);
        let east = surface(&CellBox {
            x: tw,
            y: v,
            w: 1,
            h: 1,
        });
        let west = surface(&CellBox {
            x: 0,
            y: v,
            w: 1,
            h: 1,
        });
        assert_eq!(east, west);
        assert_eq!(s.calc_normal(tw, v), s.calc_normal(0, v));
        // The blend of a texel west of the seam is the one of the texel at the east edge.
        let defs = [row(0, false), row(0, false)];
        let projections = Projections::from_defs(&defs);
        let kernel = BlendKernel::new();
        for d in -2..=2 {
            assert_eq!(
                calc_blend_vector(&s, &projections, &kernel, 0, (d, v)),
                calc_blend_vector(&s, &projections, &kernel, 0, (tw + d, v)),
                "texel {d}"
            );
        }
    }

    #[test]
    fn a_hex_over_the_seam_is_in_the_cell_on_the_other_side() {
        let mut basis =
            TerrainBasis::uniform(4, 4, true, 1, plot(Relief::Flat, TerrainKind::Grass, 0));
        let i = basis.index(3, 1).unwrap();
        basis.plots[i].continent = 4;
        let frame = TerrainFrame::new(4, 4, true);
        let cells = TerrainCells::new(&frame, &basis);
        // Hex (3, 1) is an odd row: its middle is on the texel 4 * 64, past the last cell.
        let first_column = cells.cell_at(0, 1).unwrap();
        assert!(
            first_column.hexes.contains(&(3, 1)),
            "{:?}",
            first_column.hexes
        );
        assert_eq!(first_column.class(), 4);
        // Without the wrap the same hex is not in the first column.
        let frame = TerrainFrame::new(4, 4, false);
        let plain = TerrainCells::new(&frame, &basis);
        assert!(!plain.cell_at(0, 1).unwrap().hexes.contains(&(3, 1)));
    }

    #[test]
    fn the_class_of_a_cell_comes_from_the_continents_of_its_hexes() {
        let mut basis =
            TerrainBasis::uniform(6, 6, false, 1, plot(Relief::Flat, TerrainKind::Grass, 0));
        for x in 3..6 {
            for y in 0..6 {
                let i = basis.index(x, y).unwrap();
                basis.plots[i].continent = 3;
            }
        }
        let frame = TerrainFrame::new(6, 6, false);
        let cells = TerrainCells::new(&frame, &basis);
        assert_eq!(cells.cell_at(0, 0).unwrap().class(), 0);
        assert_eq!(cells.cell_at(5, 3).unwrap().class(), 3);
    }

    #[test]
    fn the_fog_gives_a_cell_its_flags_and_the_window_of_a_hex_is_small() {
        let t = built_terrain();
        let frame = *t.frame().unwrap();
        let mut cells = TerrainCells::new(&frame, t.basis().unwrap());
        assert!(cells.iter().all(|c| !c.is_open()));
        let mut fog = vec![Fog::Unrevealed; 36];
        fog[2 * 6 + 2] = Fog::Visible;
        fog[4 * 6 + 4] = Fog::Hidden;
        let changed = cells.set_fog(&fog);
        assert!(!changed.is_empty());
        let open: Vec<&TerrainCell> = cells.iter().filter(|c| c.is_open()).collect();
        assert!(!open.is_empty() && open.len() < 36);
        // The cell of the visible hex has the bit 0, that of the hidden one the bit 1.
        let (cx, cy) = cells
            .grid()
            .cell_of_texel(frame.hex_to_tex(2, 2).x, frame.hex_to_tex(2, 2).y);
        assert_eq!(cells.cell_at(cx, cy).unwrap().fow_flags & 1, 1);
        let (cx, cy) = cells
            .grid()
            .cell_of_texel(frame.hex_to_tex(4, 4).x, frame.hex_to_tex(4, 4).y);
        assert_eq!(cells.cell_at(cx, cy).unwrap().fow_flags & 2, 2);
        // The same fog again changes nothing.
        assert!(cells.set_fog(&fog).is_empty());
        // One hex goes from hidden to visible: only a few cells are looked at.
        let changed = cells.set_fog_hex(4, 4, Fog::Visible);
        assert!(
            !changed.is_empty() && changed.len() <= 9,
            "{}",
            changed.len()
        );
        assert_eq!(cells.cell_at(cx, cy).unwrap().fow_flags & 1, 1);
        // And the whole map hidden again.
        let changed = cells.set_fog(&[Fog::Unrevealed; 36]);
        assert!(!changed.is_empty());
        assert!(cells.iter().all(|c| !c.is_open()));
    }

    #[test]
    fn a_cell_is_visible_inside_the_planes_and_not_outside() {
        let frame = TerrainFrame::new(4, 4, false);
        let b = CellBox {
            x: 64,
            y: 64,
            w: 65,
            h: 65,
        };
        let bounds = CellBounds::new(&frame, &b);
        assert!(bounds.radius > 45.0 && bounds.radius < 200.0);
        let c = bounds.center;
        // Six planes of a big box around the cell: inside.
        let around = |d: f32| {
            [
                Vec4::new(1.0, 0.0, 0.0, d - c.x),
                Vec4::new(-1.0, 0.0, 0.0, d + c.x),
                Vec4::new(0.0, 1.0, 0.0, d - c.y),
                Vec4::new(0.0, -1.0, 0.0, d + c.y),
                Vec4::new(0.0, 0.0, 1.0, d - c.z),
                Vec4::new(0.0, 0.0, -1.0, d + c.z),
            ]
        };
        let (visible, d) = bounds.is_visible(&around(1000.0));
        assert!(visible && d.iter().all(|&x| (x - 1000.0).abs() < 1e-3));
        // A plane that has the whole cell behind it, further than the radius: not visible.
        let mut planes = around(1000.0);
        planes[0] = Vec4::new(1.0, 0.0, 0.0, -c.x - bounds.radius - 1.0);
        assert!(!bounds.is_visible(&planes).0);
        // Half a radius behind the plane: the sphere still touches it, visible.
        planes[0] = Vec4::new(1.0, 0.0, 0.0, -c.x - bounds.radius * 0.5);
        assert!(bounds.is_visible(&planes).0);
    }

    #[test]
    fn the_page_size_follows_the_width_on_the_screen() {
        for (w, size) in [
            (400.0, 512),
            (256.1, 512),
            (256.0, 256),
            (200.0, 256),
            (128.0, 128),
            (100.0, 128),
            (64.1, 128),
            (64.0, 64),
            (10.0, 64),
        ] {
            assert_eq!(page_size_for_width(w, 64), size, "{w}");
        }
        // Never below the size of the cache pages.
        assert_eq!(page_size_for_width(10.0, 128), 128);
        assert_eq!(page_size_for_width(10.0, 256), 256);
    }

    #[test]
    fn a_cell_looked_at_from_above_is_as_wide_as_the_screen_says() {
        let frame = TerrainFrame::new(4, 4, false);
        let b = CellBox {
            x: 64,
            y: 64,
            w: 65,
            h: 65,
        };
        let bounds = CellBounds::new(&frame, &b);
        // An orthographic view that shows 400 world units across 1000 pixels: the 65 texels of
        // the cell (about 65 x 56 units) are about 160 pixels long on the diagonal.
        let ortho = Mat4::from_scale(Vec3::new(1.0 / 200.0, 1.0 / 200.0, 0.001));
        let w = bounds.max_width_px(&ortho, 1000.0);
        let diagonal = (bounds.max - bounds.min).truncate().length();
        // The longest of the four diagonals of the box is the one with the height too, half of it
        // in the clip space is a quarter of the screen width per 400 units.
        assert!(
            w > diagonal * 1.25 * 0.9 && w < diagonal * 1.25 * 1.6,
            "{w} for {diagonal}"
        );
        // About 107 pixels: the page of 128.
        assert_eq!(page_size_for_width(w, 64), 128);
    }

    #[test]
    fn the_vertex_weights_of_a_cell_sum_to_255_within_the_truncation() {
        let t = built_terrain();
        let s = t.sampler().unwrap();
        let defs = t.library().defs();
        let projections = Projections::from_defs(defs.rows());
        let kernel = BlendKernel::new();
        // The cell of the hill and its neighbours: textures of grass and hill.
        let mut checked = 0;
        for (cx, cy) in [(1, 1), (2, 2), (3, 3), (4, 1), (0, 0)] {
            let b = CellGrid::new(s.frame()).cell_box(cx, cy);
            let textures = texture_contributions(s, &projections, &b);
            assert!(!textures.is_empty() && textures.len() <= MAX_TEXTURES);
            let v = build_cell_vertices(
                &VertexInputs {
                    sampler: s,
                    projections: &projections,
                    kernel: &kernel,
                    textures: &textures,
                    page_size: 64,
                    gpu_weights: false,
                },
                &b,
            );
            assert_eq!(v.composite.len(), ((b.w + 2) * (b.h + 2)) as usize);
            // The border of the contributions is two texels west and north and one east and south,
            // the kernel reaches two: the vertices of the row and column -1 and of the last ones may
            // blend a texture the cell does not list, as in the original. Inside, away from a
            // vertical projection, the weights sum to 255 within the truncation.
            // (The taps west of the map use the unsigned modulo of the original, so the first two
            // columns and rows of the first cells are left out as well.)
            for iy in 2..b.h - 1 {
                for ix in 2..b.w - 1 {
                    let vert = &v.composite[v.index(ix, iy)];
                    let sum: u32 = vert.weights.iter().map(|&w| u32::from(w)).sum();
                    assert!(sum <= 255, "cell ({cx}, {cy}): {sum} {vert:?}");
                    // A vertical projection (a mountain) takes three slots, of which the list
                    // counts the ones the normals pick: a vertex at its foot may blend a slot
                    // the list lacks, as the original does. Away from one the weights sum to 255.
                    let (hx, hy) = (vert.hex.x as i32, vert.hex.y as i32);
                    let near_vertical = kernel
                        .offsets
                        .iter()
                        .any(|&(ox, oy)| projections.mode(s.field().get_id(hx + ox, hy + oy)) == 1);
                    if !near_vertical {
                        assert!(
                            (255 - 8..=255).contains(&sum),
                            "cell ({cx}, {cy}): {sum} {vert:?}"
                        );
                    }
                    checked += 1;
                }
            }
        }
        assert!(checked > 5 * 60 * 60);
    }

    #[test]
    fn the_vertices_of_a_cell_have_the_clip_position_the_texel_and_the_height() {
        let t = built_terrain();
        let s = t.sampler().unwrap();
        let projections = Projections::from_defs(t.library().defs().rows());
        let kernel = BlendKernel::new();
        let b = CellGrid::new(s.frame()).cell_box(2, 2);
        let textures = texture_contributions(s, &projections, &b);
        let v = build_cell_vertices(
            &VertexInputs {
                sampler: s,
                projections: &projections,
                kernel: &kernel,
                textures: &textures,
                page_size: 128,
                gpu_weights: false,
            },
            &b,
        );
        assert_eq!((v.width, v.height), (67, 67));
        // The corner (-1, -1) is the upper left of the page, (w, h) the lower right.
        let first = v.composite[v.index(-1, -1)];
        assert_eq!((first.clip.x, first.clip.y, first.clip.z), (-1.0, 1.0, 0.5));
        let last = v.composite[v.index(b.w, b.h)];
        assert_eq!((last.clip.x, last.clip.y), (1.0, -1.0));
        // The texel and the world position of the vertex (3, 4).
        let at = v.composite[v.index(3, 4)];
        assert_eq!(at.hex, Vec2::new((b.x + 3) as f32, (b.y + 4) as f32));
        let scale = s.frame().terrain_scale();
        assert_eq!(at.world.x, scale.x * at.hex.x);
        assert_eq!(at.world.y, scale.y * at.hex.y);
        let want_z = f32::from(s.raw_height(b.x + 3, b.y + 4)) / 255.0 * scale.z;
        assert!((at.world.z - want_z).abs() < 1e-5);
        // The compressed vertex is moved by half a texel of the page (1 / 128) and carries the
        // normal as two halves; the texture coordinate is the place in the page.
        let c = v.compressed[v.index(3, 4)];
        assert!((c.position.x - (at.clip.x - 1.0 / 128.0)).abs() < 1e-6);
        assert!((c.position.y - (at.clip.y + 1.0 / 128.0)).abs() < 1e-6);
        let nz = asset_format::f16_bits_to_f32((c.normal_xy >> 16) as u16);
        let n = s.normals_block((b.x + 3, b.y + 4), (1, 1))[0];
        assert!((nz - n.y).abs() < 1e-3);
        assert_eq!(c.normal_zw >> 16, u32::from(f32_to_f16_bits(1.0)));
        // The GPU path puts only the projection mode in the first texture.
        let g = build_cell_vertices(
            &VertexInputs {
                sampler: s,
                projections: &projections,
                kernel: &kernel,
                textures: &textures,
                page_size: 128,
                gpu_weights: true,
            },
            &b,
        );
        let gv = g.composite[g.index(3, 4)];
        assert_eq!(gv.weights, [0; 8]);
        assert_eq!(gv.textures[0], projection_mode_of_normal(n));
    }

    #[test]
    fn a_cell_without_textures_gives_the_first_one_all_the_weight() {
        let t = built_terrain();
        let s = t.sampler().unwrap();
        let projections = Projections::from_defs(t.library().defs().rows());
        let kernel = BlendKernel::new();
        let b = CellGrid::new(s.frame()).cell_box(1, 1);
        let none = TextureSet::default();
        let v = build_cell_vertices(
            &VertexInputs {
                sampler: s,
                projections: &projections,
                kernel: &kernel,
                textures: &none,
                page_size: 64,
                gpu_weights: false,
            },
            &b,
        );
        assert!(
            v.composite
                .iter()
                .all(|c| c.weights == [255, 0, 0, 0, 0, 0, 0, 0])
        );
    }

    #[test]
    fn recompute_sets_the_stats_the_textures_and_the_highlight() {
        let t = built_terrain();
        let s = t.sampler().unwrap();
        let rows: Vec<TerrainDefRow> = t.library().defs().rows().to_vec();
        let projections = Projections::from_defs(&rows);
        let mut cells = TerrainCells::new(s.frame(), t.basis().unwrap());
        let i = cells.grid().index(1, 1);
        cells.recompute(i, s, &projections, true);
        let c = cells.cell(i);
        assert_eq!(c.stats.class, StatsClass::Land);
        assert!(!c.textures.is_empty());
        // The synthetic table has no highlight map: no highlight page, unless the region was
        // invalidated.
        assert!(!c.has_spec);
        let hit = cells.invalidate_region(Vec2::new(70.0, 70.0), Vec2::new(100.0, 100.0));
        assert_eq!(hit, [i]);
        assert!(cells.cell(i).textures.is_empty() && cells.cell(i).force_spec);
        cells.recompute(i, s, &projections, true);
        assert!(cells.cell(i).has_spec);
        // Highlights off in the settings: none.
        cells.recompute(i, s, &projections, false);
        assert!(!cells.cell(i).has_spec);
        // The whole map at once.
        cells.prune_work(s, &projections, true);
        assert!(cells.iter().filter(|c| c.stats.class.is_drawn()).count() > 20);
    }

    #[test]
    fn a_region_of_texels_touches_the_cells_under_it_and_stays_on_the_map() {
        let t = built_terrain();
        let mut cells = TerrainCells::new(t.frame().unwrap(), t.basis().unwrap());
        let g = *cells.grid();
        let mut hit = cells.invalidate_region(Vec2::new(60.0, 60.0), Vec2::new(130.0, 70.0));
        hit.sort_unstable();
        assert_eq!(
            hit,
            [
                g.index(0, 0),
                g.index(1, 0),
                g.index(2, 0),
                g.index(0, 1),
                g.index(1, 1),
                g.index(2, 1)
            ]
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
        );
        // Past the map: clipped, no wrap.
        let hit = cells.invalidate_region(Vec2::new(-50.0, 370.0), Vec2::new(5000.0, 9000.0));
        assert_eq!(hit.len(), 6);
        assert!(hit.iter().all(|&i| g.cell_xy(i).1 == 5));
    }

    #[test]
    fn a_segment_touches_a_box_by_an_end_or_by_crossing() {
        let (min, max) = (Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0));
        let v = Vec2::new;
        assert!(segment_touches_box(v(5.0, 5.0), v(50.0, 50.0), min, max));
        assert!(segment_touches_box(v(-5.0, 5.0), v(15.0, 5.0), min, max));
        assert!(segment_touches_box(v(-5.0, -5.0), v(15.0, 15.0), min, max));
        assert!(!segment_touches_box(v(-5.0, 11.0), v(15.0, 11.0), min, max));
        assert!(!segment_touches_box(v(11.0, -5.0), v(11.0, 15.0), min, max));
        // The anti-diagonal passes through the middle of the box.
        assert!(segment_touches_box(v(-5.0, 20.0), v(20.0, -5.0), min, max));
    }

    #[test]
    fn halves_pack_low_first() {
        let p = pack_halves(1.0, -2.0);
        assert_eq!(p & 0xffff, u32::from(f32_to_f16_bits(1.0)));
        assert_eq!(p >> 16, u32::from(f32_to_f16_bits(-2.0)));
        assert_eq!(asset_format::f16_bits_to_f32((p & 0xffff) as u16), 1.0);
    }
}
