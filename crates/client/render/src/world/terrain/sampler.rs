//! The sampler of the terrain: the port of `cvTerrainSampler`. Texel,
//! world and hex; the height of the nearest texel; the normal from the 5 x 5 kernels of `CalcNormal`;
//! the wrap along X. [`TerrainFrame`] holds the translation between the texels of the original and
//! our world, and `ours_to_orig()` for the shaders.
//!
//! Positions are `(X, Z)` of our basis (X east, Y up, Z south, the centre of hex (0, 0) at the
//! origin, as `hexgrid::cell_center`); heights are our `Y`. The original's world is the texel
//! times `g_TerrainScale` minus half the world, with y north and z up; `ours_to_orig()` moves one
//! to the other.
//!
//! Open (RW-5, RW-6): `g_HexToTex` and `g_TexToHex`, `g_TerrainScale`. Until they are answered the
//! frame takes: 64 texels per hex, a hex centre in the middle of its 64 x 64 texels, a scale of
//! `(1, ROW_STEP / 64, 0.2)` (the capture; B1 had 0.1985 with the heights 13000 -> 10.12, 15423 -> 12.0 of
//! spec 3.1). The values are in one place ([`TerrainFrame::new`]) and the tests check the
//! relations, not the numbers of a capture.

use glam::{Mat4, Vec2, Vec3, Vec4};

use super::field::{CellRect, TEXELS_PER_HEX, TerrainField};
use crate::hexgrid::{GridShape, HEX_WIDTH, ROW_STEP, nearest_cell};

/// Height of the texel of the original: `raw / 255 * g_TerrainScale.z`.
// re: cvTerrainSampler::GetWorldHeight (cvTerrainSampler.cpp:224-235)
pub const HEIGHT_TO_UNIT: f32 = 1.0 / 255.0;

/// `g_TerrainScale.z` of the capture (`build::tables::TERRAIN_SCALE`, S1a ev 819): 13000 / 255 * z = 10.196
/// (the water), 15423 / 255 * z = 12.097 (the plain).
pub const TERRAIN_SCALE_Z: f32 = 0.2;

/// The texel of a hex centre inside its 64 x 64 texels, on both axes.
// TODO(RW-6): the middle of the hex block, to be replaced by `g_HexToTex[12], [13]`
pub const HEX_CENTER_TEXEL: f32 = 32.0;

/// Texel rows the original's world is longer than our field along y: its centre is half a row of
/// hexes (32 rows of texels) north of the middle of our field. Measured on the capture S1a with the
/// camera of the capture: with it the hex grid of the original, its selection ring and its river lie
/// on our hexes and river edges; without it everything of ours is half a row north. The field itself
/// is not moved (G-T1 registers it with the capture's height texels at offset 0).
// TODO(RW-6): the `size` of `TerrainSystem::Init` read from the code, and whether a map that does not
// wrap has the same along x
pub const ORIG_EXTRA_ROWS: u32 = 64;

/// Raw height of the sea: the level the water of the original starts at (`WaterSystem::Startup(13000)`).
// re: TerrainSystem::Init (TerrainSystem.cpp:218, :699)
pub const SEA_LEVEL_RAW: u16 = 13_000;

/// The raw height (u16) of a height in our `Y`: the inverse of `raw / 255 * scale_z`, rounded.
pub fn raw_from_height(height: f32, scale_z: f32) -> u16 {
    (height / (scale_z * HEIGHT_TO_UNIT))
        .round()
        .clamp(0.0, f32::from(u16::MAX)) as u16
}

/// The taps of the 5 x 5 filter: smoothing across, derivative along.
// re: cvTerrainSampler.cpp:20-21 (kSmoothKernel, kDerivativeKernel)
pub(crate) const SMOOTH_KERNEL: [f32; 5] = [0.0228, 0.2293, 0.4948, 0.2293, 0.0228];
pub(crate) const DERIVATIVE_KERNEL: [f32; 5] = [0.0702, 0.3526, 0.0, -0.3526, -0.0702];
/// Scale of the distance of a tap from the plane in the range weight.
// re: cvTerrainSampler.cpp:18 (kRangeScale)
pub(crate) const RANGE_SCALE: f32 = 0.007_812_5;
// re: cvTerrainSampler.cpp:15 (kExpScale)
pub(crate) const EXP_SCALE: f32 = -3.333_333_3;
// re: cvTerrainSampler.cpp:19 (kWeightNorm)
pub(crate) const WEIGHT_NORM: f32 = 1.030_064_5;

/// Texel and world of one map: `X = u * sx + ax`, `Z = -(v * sy + ay)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainFrame {
    width_hexes: u32,
    height_hexes: u32,
    wrap_x: bool,
    /// `g_TerrainScale`.
    scale: Vec3,
}

impl TerrainFrame {
    /// The frame of a map of `width` x `height` hexes.
    pub fn new(width: u32, height: u32, wrap_x: bool) -> Self {
        Self {
            width_hexes: width,
            height_hexes: height,
            wrap_x,
            // A texel is one world unit along X and a row of hexes is 64 texels.
            scale: Vec3::new(
                HEX_WIDTH / TEXELS_PER_HEX as f32,
                ROW_STEP / TEXELS_PER_HEX as f32,
                TERRAIN_SCALE_Z,
            ),
        }
    }

    /// The same frame with the height scale of a capture (`m_vTerrainScale.z`).
    pub fn with_height_scale(mut self, z: f32) -> Self {
        self.scale.z = z;
        self
    }

    pub fn width_hexes(&self) -> u32 {
        self.width_hexes
    }

    pub fn height_hexes(&self) -> u32 {
        self.height_hexes
    }

    pub fn wrap_x(&self) -> bool {
        self.wrap_x
    }

    /// Texels along X and Y: `(W << 6, H << 6)`.
    // re: TerrainSystem::Init (TerrainSystem.cpp:687-689)
    pub fn texel_size(&self) -> (u32, u32) {
        (
            self.width_hexes * TEXELS_PER_HEX,
            self.height_hexes * TEXELS_PER_HEX,
        )
    }

    /// `g_TerrainScale` (`m_vTerrainScale`): the world units of a texel along X and Y and the
    /// height of raw 255.
    pub fn terrain_scale(&self) -> Vec3 {
        self.scale
    }

    /// Period of the map along X in our world: `W * 64`.
    pub fn world_width(&self) -> f32 {
        self.width_hexes as f32 * HEX_WIDTH
    }

    /// The shape of the grid of hexes.
    pub fn grid_shape(&self) -> GridShape {
        GridShape {
            width: self.width_hexes as i32,
            height: self.height_hexes as i32,
            wrap_x: self.wrap_x,
        }
    }

    fn ax(&self) -> f32 {
        -HEX_CENTER_TEXEL * self.scale.x
    }

    fn ay(&self) -> f32 {
        -HEX_CENTER_TEXEL * self.scale.y
    }

    /// Our `(X, Z)` of the point `(u, v)` of the texel plane.
    pub fn tex_to_world(&self, tex: Vec2) -> Vec2 {
        Vec2::new(
            tex.x * self.scale.x + self.ax(),
            -(tex.y * self.scale.y + self.ay()),
        )
    }

    /// The point `(u, v)` of the texel plane under our `(X, Z)`.
    pub fn world_to_tex(&self, p: Vec2) -> Vec2 {
        Vec2::new(
            (p.x - self.ax()) / self.scale.x,
            (-p.y - self.ay()) / self.scale.y,
        )
    }

    /// The point `(u, v)` of the centre of hex `(x, y)`: the offset rows are half a hex east.
    // TODO(RW-6): the layout of B1's grid; `g_HexToTex` of the original is a matrix
    pub fn hex_to_tex(&self, x: i32, y: i32) -> Vec2 {
        Vec2::new(
            TEXELS_PER_HEX as f32 * (x as f32 + 0.5 * (y & 1) as f32) + HEX_CENTER_TEXEL,
            TEXELS_PER_HEX as f32 * y as f32 + HEX_CENTER_TEXEL,
        )
    }

    /// The hex under the point `(u, v)` (`g_TexToHex`), unwrapped and without bounds.
    pub fn tex_to_hex(&self, tex: Vec2) -> (i32, i32) {
        nearest_cell(self.tex_to_world(tex))
    }

    /// The texels a hex covers: its width, and its height from corner to corner.
    pub fn hex_texel_rect(&self, x: i32, y: i32) -> CellRect {
        let c = self.hex_to_tex(x, y);
        let half_w = HEX_WIDTH * 0.5 / self.scale.x;
        let half_h = crate::hexgrid::CORNER_RADIUS / self.scale.y;
        CellRect::new(
            (c.x - half_w).floor() as i32,
            (c.y - half_h).floor() as i32,
            (c.x + half_w).ceil() as i32,
            (c.y + half_h).ceil() as i32,
        )
    }

    /// Where `Tx` and `Ty` put the world: half of it (`m_fHalfWorldWidth`, `m_fHalfWorldHeight`).
    /// Along y the `size` of the original is [`ORIG_EXTRA_ROWS`] texels more than our field.
    // re: TerrainSystem::Init (TerrainSystem.cpp:694-696, `vWorldCenter = (size >> 1) * scale`)
    pub fn half_world(&self) -> Vec2 {
        let (w, h) = self.texel_size();
        Vec2::new(
            (w >> 1) as f32 * self.scale.x,
            ((h + ORIG_EXTRA_ROWS) >> 1) as f32 * self.scale.y,
        )
    }

    /// The point of the original's world under the texel `(u, v)` at height `h`:
    /// `(u * scale.x - half.x, v * scale.y - half.y, h)`.
    // re: cvTerrainSampler::TexToWorld (cvTerrainSampler.cpp:203-208)
    pub fn orig_point(&self, tex: Vec2, h: f32) -> Vec3 {
        let half = self.half_world();
        Vec3::new(
            tex.x * self.scale.x - half.x,
            tex.y * self.scale.y - half.y,
            h,
        )
    }

    /// `ours_to_orig`: the rotation of render-core 4.6 (our `(x, y, z)` becomes `(x, -z, y)`) and
    /// the translation `T` that puts the texel `(u, v)` at the original's world point.
    pub fn ours_to_orig(&self) -> Mat4 {
        let half = self.half_world();
        // orig.x = X - ax - half.x; orig.y = -Z - ay - half.y; orig.z = Y.
        let t = Vec3::new(-self.ax() - half.x, -self.ay() - half.y, 0.0);
        Mat4::from_cols(
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(0.0, -1.0, 0.0, 0.0),
            t.extend(1.0),
        )
    }
}

/// The height field with its frame, read as `cvTerrainSampler` does.
#[derive(Clone, Debug)]
pub struct TerrainSampler {
    frame: TerrainFrame,
    field: TerrainField,
    sea_level_raw: u16,
    normals: NormalCache,
}

/// The most bytes of normals [`TerrainSampler::normals_block`] keeps (`terrain.normals.bytes`).
pub const NORMAL_CACHE_BYTES: usize = 64 << 20;

/// The blocks of normals made lately, by their origin and size, with the revision of the heights they
/// were made from ([`TerrainField::heights_revision`]): a page of a cell is baked again and again (its
/// near page as it comes and goes, its highlight given back, a decal), and each bake asked for the same
/// 67 x 67 normals, 25 taps each (B3: a third of the main thread in a far pan). The oldest go first
/// past [`NORMAL_CACHE_BYTES`].
#[derive(Default)]
struct NormalCache(std::sync::Mutex<NormalBlocks>);

/// The revision of the heights it was made from, the last use, the normals.
type NormalEntry = (u64, u64, std::sync::Arc<Vec<Vec3>>);

#[derive(Default)]
struct NormalBlocks {
    blocks: crate::anim::fx::FxHashMap<(i32, i32, i32, i32), NormalEntry>,
    bytes: usize,
    clock: u64,
    hits: u64,
    misses: u64,
}

/// A block of normals made a few rows at a time ([`TerrainSampler::begin_normals`]).
pub struct NormalsInProgress {
    origin: (i32, i32),
    size: (i32, i32),
    cols: Vec<i32>,
    stamp: u64,
    rows: Vec<Vec3>,
    next: i32,
}

impl NormalsInProgress {
    /// The origin and size of the block.
    pub fn block(&self) -> ((i32, i32), (i32, i32)) {
        (self.origin, self.size)
    }
}

impl Clone for NormalCache {
    /// A copy of the sampler starts with no normals kept.
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl std::fmt::Debug for NormalCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NormalCache")
    }
}

impl TerrainSampler {
    /// A sampler over an empty field of the frame's size.
    pub fn new(frame: TerrainFrame) -> Self {
        let (w, h) = frame.texel_size();
        Self {
            frame,
            field: TerrainField::with_texels(w, h),
            sea_level_raw: SEA_LEVEL_RAW,
            normals: NormalCache::default(),
        }
    }

    pub fn frame(&self) -> &TerrainFrame {
        &self.frame
    }

    pub fn field(&self) -> &TerrainField {
        &self.field
    }

    pub fn field_mut(&mut self) -> &mut TerrainField {
        &mut self.field
    }

    /// Grows at every change of a height.
    pub fn height_revision(&self) -> u64 {
        self.field.revision()
    }

    /// The column of a texel after the wrap along X: `(size * 100 + x) % size`.
    // re: cvTerrainSampler::GetWrappedPosition (cvTerrainSampler.cpp:150-155)
    fn wrap(&self, u: i32) -> i32 {
        if self.frame.wrap_x() {
            u.rem_euclid(self.field.width() as i32)
        } else {
            u
        }
    }

    /// Raw height of the texel `(u, v)` after the wrap; 0 outside the map.
    pub fn raw_height(&self, u: i32, v: i32) -> u16 {
        self.field.get_height(self.wrap(u), v)
    }

    /// The texel under our `(X, Z)`: the integer part of the texel plane's point, as the cast
    /// `(int)` of the original.
    // re: cvTerrainSampler::GetWorldHeight (cvTerrainSampler.cpp:173-178, `(int)((pos + offset) / scale)`)
    pub fn texel_of(&self, p: Vec2) -> (i32, i32) {
        let t = self.frame.world_to_tex(p);
        (t.x as i32, t.y as i32)
    }

    /// Height of the texel `(u, v)` in our `Y`: `raw / 255 * scale.z`; 0 outside the map.
    // re: cvTerrainSampler::GetWorldHeight (cvTerrainSampler.cpp:224-235)
    pub fn texel_height(&self, u: i32, v: i32) -> f32 {
        let wu = self.wrap(u);
        if wu < 0 || v < 0 || wu >= self.field.width() as i32 || v >= self.field.height() as i32 {
            return 0.0;
        }
        f32::from(self.field.get_height(wu, v)) * HEIGHT_TO_UNIT * self.frame.terrain_scale().z
    }

    /// Height under `(X, Z)`: the nearest texel, no filtering.
    // re: cvTerrainSampler::GetWorldHeight (cvTerrainSampler.cpp:173-178)
    pub fn world_height(&self, p: Vec2) -> f32 {
        let (u, v) = self.texel_of(p);
        self.texel_height(u, v)
    }

    /// The sea level in our `Y`.
    pub fn sea_level(&self) -> f32 {
        f32::from(self.sea_level_raw) * HEIGHT_TO_UNIT * self.frame.terrain_scale().z
    }

    /// Height of the surface under `(X, Z)`: the ground, but not below the sea.
    // re: TerrainSystem::Init (TerrainSystem.cpp:699, `field_4 = seaLevel * 255 * scale.z`), meaning from RW-14 (i)
    pub fn surface_height(&self, p: Vec2) -> f32 {
        self.world_height(p).max(self.sea_level())
    }

    /// Whether `(X, Z)` is on the field.
    // re: cvTerrainSampler::IsValidWorldLocation (cvTerrainSampler.cpp:211-221)
    pub fn is_valid_world_location(&self, p: Vec2) -> bool {
        // The test is on the cast value: the point -0.5 is texel 0, as in the original.
        let (u, v) = self.texel_of(p);
        (0..self.field.width() as i32).contains(&u) && (0..self.field.height() as i32).contains(&v)
    }

    /// The hex under `(X, Z)`, wrapped along X; `None` off the map.
    pub fn hex_at(&self, p: Vec2) -> Option<(i32, i32)> {
        let (x, y) = nearest_cell(p);
        self.frame.grid_shape().normalize(x, y)
    }

    /// The centre of hex `(x, y)` on the ground: our `(X, height, Z)`.
    // re: cvTerrainSampler::GetWorldLocation (cvTerrainSampler.cpp:158-170)
    pub fn hex_location(&self, x: i32, y: i32) -> Vec3 {
        let tex = self.frame.hex_to_tex(x, y);
        let p = self.frame.tex_to_world(tex);
        let (u, v) = (tex.x as i32, tex.y as i32);
        Vec3::new(p.x, self.texel_height(u, v), p.y)
    }

    /// The normal at the texel `(u, v)`, in the original's basis (x east, y north, z up).
    /// The taps outside the map take the height of the centre (the original leaves them unset).
    // re: cvTerrainSampler::CalcNormal (cvTerrainSampler.cpp:259-321)
    pub fn calc_normal(&self, u: i32, v: i32) -> Vec3 {
        self.normal_with_taps(u, v, false)
    }

    fn tap_raw(&self, u: i32, v: i32, clamp: bool) -> Option<u16> {
        let (w, h) = (self.field.width() as i32, self.field.height() as i32);
        let u = self.wrap(u);
        if clamp {
            Some(self.field.get_height(u.clamp(0, w - 1), v.clamp(0, h - 1)))
        } else if (u | v) >= 0 && u < w && v < h {
            Some(self.field.get_height(u, v))
        } else {
            None
        }
    }

    fn normal_with_taps(&self, u: i32, v: i32, clamp: bool) -> Vec3 {
        let center_raw = self
            .tap_raw(u, v, true)
            .expect("a clamped tap is always there");
        let mut raw = [[center_raw; 5]; 5];
        for (i, raw_i) in raw.iter_mut().enumerate() {
            for (j, r) in raw_i.iter_mut().enumerate() {
                if let Some(t) = self.tap_raw(u - 2 + i as i32, v - 2 + j as i32, clamp) {
                    *r = t;
                }
            }
        }
        self.normal_of_taps(&raw)
    }

    /// The 5 x 5 filter of `CalcNormal` on its taps, `raw[i][j]` at the offset `(i - 2, j - 2)`; a
    /// missing tap holds the centre's raw height.
    fn normal_of_taps(&self, raw: &[[u16; 5]; 5]) -> Vec3 {
        let center = f32::from(raw[2][2]) * HEIGHT_TO_UNIT;
        let mut height = [[0.0f32; 5]; 5];
        for (raw_i, height_i) in raw.iter().zip(height.iter_mut()) {
            for (r, h) in raw_i.iter().zip(height_i.iter_mut()) {
                *h = f32::from(*r) * HEIGHT_TO_UNIT;
            }
        }
        let plane_x =
            ((i32::from(raw[3][2]) - i32::from(raw[1][2])) >> 1) as f32 * HEIGHT_TO_UNIT + center;
        let plane_y =
            ((i32::from(raw[2][3]) - i32::from(raw[2][1])) >> 1) as f32 * HEIGHT_TO_UNIT + center;

        let (mut sum_x, mut sum_y, mut weight_x, mut weight_y) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for i in 0..5 {
            for j in 0..5 {
                let h = height[i][j];
                let ux = (plane_x - h) * RANGE_SCALE;
                let uy = (plane_y - h) * RANGE_SCALE;
                let tx = ux * EXP_SCALE * ux;
                let ty = uy * EXP_SCALE * uy;
                let (txx, tyy) = (tx * tx, ty * ty);
                let wx = (tx * txx / 6.0 + (txx * 0.5 + (tx + 1.0)))
                    * WEIGHT_NORM
                    * DERIVATIVE_KERNEL[i]
                    * SMOOTH_KERNEL[j];
                let wy = (ty * tyy / 6.0 + (tyy * 0.5 + (ty + 1.0)))
                    * WEIGHT_NORM
                    * SMOOTH_KERNEL[i]
                    * DERIVATIVE_KERNEL[j];
                sum_x += h * wx;
                sum_y += h * wy;
                weight_x += wx.abs();
                weight_y += wy.abs();
            }
        }
        let scale = self.frame.terrain_scale();
        let slope_x = (sum_x / weight_x) * scale.z;
        let slope_y = (sum_y / weight_y) * scale.z;
        // The original adds a product with -0.0 to each term; it changes nothing.
        Vec3::new(slope_x * scale.y, scale.x * slope_y, scale.x * scale.y).normalize()
    }

    /// The normals of a block of texels, for the normal map: the taps past the edge of the map
    /// are the texels of the edge (`CalcNormalBlockMax12x12` and `69x69`), row by row.
    // The block filters of the original weigh the taps with `m_afNormalWeightX/Y`, whose values
    // the source does not show; the block here is the 5 x 5 filter of `CalcNormal` per texel.
    // TODO(RW-6): compare with the normal map of a capture (G-T2) and take the tables if they differ.
    // The taps of the whole block are read from the field once (a window two texels wider on each
    // side, wrapped and clamped as `tap_raw` does) instead of 25 lookups per texel: the same numbers,
    // the filter is the one of `normal_with_taps`.
    //
    // A block asked for again with the heights under it unchanged comes from [`NormalCache`].
    pub fn normals_block(&self, origin: (i32, i32), size: (i32, i32)) -> Vec<Vec3> {
        let (sw, sh) = (size.0.max(0), size.1.max(0));
        if sw == 0 || sh == 0 {
            return Vec::new();
        }
        let (cols, stamp) = self.normal_window(origin, (sw, sh));
        if let Some(v) = self.cached_normals(origin, (sw, sh), stamp) {
            return v;
        }
        let out = self.normal_rows_par(origin, (sw, sh), &cols, 0..sh);
        self.keep_normals(origin, (sw, sh), stamp, out.clone());
        out
    }

    /// [`Self::normal_rows_of`] in bands of rows over the pool of `jobs` (B3 render-mt): each band
    /// reads its own window of taps, so the normals are the same numbers in the same order. Without
    /// workers, or for a short block, it is the call on this thread.
    fn normal_rows_par(
        &self,
        origin: (i32, i32),
        size: (i32, i32),
        cols: &[i32],
        rows: std::ops::Range<i32>,
    ) -> Vec<Vec3> {
        const BAND: usize = 8;
        let n = (rows.end - rows.start).max(0) as usize;
        if jobs::workers() == 0 || n < 2 * BAND {
            return self.normal_rows_of(origin, size, cols, rows);
        }
        let bands: Vec<_> = jobs::chunks(n, BAND)
            .map(|r| rows.start + r.start as i32..rows.start + r.end as i32)
            .collect();
        jobs::map(bands.len(), |b| {
            self.normal_rows_of(origin, size, cols, bands[b].clone())
        })
        .concat()
    }

    /// The columns of the taps of a block (wrapped and clamped) and the revision of the heights under
    /// them.
    fn normal_window(&self, origin: (i32, i32), (sw, sh): (i32, i32)) -> (Vec<i32>, u64) {
        let w = self.field.width() as i32;
        let cols: Vec<i32> = (0..sw + 4)
            .map(|x| self.wrap(origin.0 - 2 + x).clamp(0, w - 1))
            .collect();
        let (c0, c1) = cols
            .iter()
            .fold((i32::MAX, i32::MIN), |(a, b), &c| (a.min(c), b.max(c)));
        let stamp = self
            .field
            .heights_revision(c0, origin.1 - 2, c1 + 1, origin.1 + 2 + sh);
        (cols, stamp)
    }

    fn cached_normals(
        &self,
        origin: (i32, i32),
        (sw, sh): (i32, i32),
        stamp: u64,
    ) -> Option<Vec<Vec3>> {
        let mut c = jobs::lock_without_wait(&self.normals.0);
        c.clock += 1;
        let now = c.clock;
        let key = (origin.0, origin.1, sw, sh);
        let hit = c.blocks.get_mut(&key).and_then(|e| {
            (e.0 == stamp).then(|| {
                e.1 = now;
                e.2.clone()
            })
        });
        if hit.is_some() {
            c.hits += 1;
        } else {
            c.misses += 1;
        }
        hit.map(|v| (*v).clone())
    }

    fn keep_normals(&self, origin: (i32, i32), (sw, sh): (i32, i32), stamp: u64, out: Vec<Vec3>) {
        let mut c = jobs::lock_without_wait(&self.normals.0);
        let bytes = out.len() * std::mem::size_of::<Vec3>();
        let now = c.clock;
        let key = (origin.0, origin.1, sw, sh);
        if let Some(old) = c.blocks.insert(key, (stamp, now, std::sync::Arc::new(out))) {
            c.bytes -= old.2.len() * std::mem::size_of::<Vec3>();
        }
        c.bytes += bytes;
        while c.bytes > NORMAL_CACHE_BYTES {
            let Some(oldest) = c.blocks.iter().min_by_key(|(_, e)| e.1).map(|(k, _)| *k) else {
                break;
            };
            if let Some(e) = c.blocks.remove(&oldest) {
                c.bytes -= e.2.len() * std::mem::size_of::<Vec3>();
            }
        }
    }

    /// Starts making a block of normals a few rows at a time ([`NormalsInProgress`]): the background
    /// spreads the normals of a page over frames. `None` when the block is kept already.
    pub fn begin_normals(&self, origin: (i32, i32), size: (i32, i32)) -> Option<NormalsInProgress> {
        let (sw, sh) = (size.0.max(1), size.1.max(1));
        let (cols, stamp) = self.normal_window(origin, (sw, sh));
        let key = (origin.0, origin.1, sw, sh);
        let kept = jobs::lock_without_wait(&self.normals.0)
            .blocks
            .get(&key)
            .is_some_and(|e| e.0 == stamp);
        (!kept).then(|| NormalsInProgress {
            origin,
            size: (sw, sh),
            cols,
            stamp,
            rows: Vec::with_capacity((sw * sh) as usize),
            next: 0,
        })
    }

    /// Makes `rows` more rows of a block begun with [`Self::begin_normals`]; when the last is made the
    /// block is kept (with the revision of the heights it began on: a change since makes it stale).
    /// Returns whether the block is done.
    pub fn continue_normals(&self, p: &mut NormalsInProgress, rows: i32) -> bool {
        let to = (p.next + rows.max(1)).min(p.size.1);
        let more = self.normal_rows_of(p.origin, p.size, &p.cols, p.next..to);
        p.rows.extend(more);
        p.next = to;
        if p.next >= p.size.1 {
            self.keep_normals(p.origin, p.size, p.stamp, std::mem::take(&mut p.rows));
            return true;
        }
        false
    }

    /// The bytes of normals kept, and the blocks served from them and made since the start
    /// (`terrain.normals.*`).
    pub fn normal_cache_stats(&self) -> (usize, u64, u64) {
        let c = jobs::lock_without_wait(&self.normals.0);
        (c.bytes, c.hits, c.misses)
    }

    /// The normals of the rows `rows` of a block (whose tap columns are `cols`).
    fn normal_rows_of(
        &self,
        origin: (i32, i32),
        (sw, _sh): (i32, i32),
        cols: &[i32],
        rows: std::ops::Range<i32>,
    ) -> Vec<Vec3> {
        let h = self.field.height() as i32;
        let n = (rows.end - rows.start).max(0);
        let mut out = Vec::with_capacity((sw * n) as usize);
        let (ww, wh) = (sw + 4, n + 4);
        let mut window = vec![0u16; (ww * wh) as usize];
        for y in 0..wh {
            let v = (origin.1 + rows.start - 2 + y).clamp(0, h - 1);
            let row = &mut window[(y * ww) as usize..((y + 1) * ww) as usize];
            for (r, &u) in row.iter_mut().zip(cols) {
                *r = self.field.get_height(u, v);
            }
        }
        let mut raw = [[0u16; 5]; 5];
        for y in 0..n {
            for x in 0..sw {
                for (i, raw_i) in raw.iter_mut().enumerate() {
                    for (j, r) in raw_i.iter_mut().enumerate() {
                        *r = window[((y + j as i32) * ww + x + i as i32) as usize];
                    }
                }
                out.push(self.normal_of_taps(&raw));
            }
        }
        out
    }
    /// The normal under `(X, Z)` in the original's basis (z up): `(0, 0, 1)` on a flat field.
    // re: cvTerrainSampler::GetWorldNormal (cvTerrainSampler.cpp:238-256, the path without a normal field)
    pub fn world_normal_orig(&self, p: Vec2) -> Vec3 {
        let (u, v) = self.texel_of(p);
        self.calc_normal(u, v)
    }

    /// The normal under `(X, Z)` in our basis (Y up): `(0, 1, 0)` on a flat field.
    pub fn world_normal(&self, p: Vec2) -> Vec3 {
        let n = self.world_normal_orig(p);
        Vec3::new(n.x, n.z, -n.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hexgrid::cell_center;

    fn frame() -> TerrainFrame {
        TerrainFrame::new(40, 24, false)
    }

    #[test]
    fn the_centre_of_every_hex_is_the_centre_of_b1() {
        let f = frame();
        for y in 0..24 {
            for x in 0..40 {
                let p = f.tex_to_world(f.hex_to_tex(x, y));
                let want = cell_center(x, y);
                assert!(p.distance(want) < 1e-3, "({x}, {y}): {p} against {want}");
                assert_eq!(f.tex_to_hex(f.hex_to_tex(x, y)), (x, y));
            }
        }
    }

    #[test]
    fn the_seam_is_one_period_of_the_texels() {
        let f = TerrainFrame::new(40, 24, true);
        for y in 0..24 {
            let a = f.hex_to_tex(0, y);
            let b = f.hex_to_tex(40, y);
            assert_eq!(b.x - a.x, 40.0 * 64.0);
            assert_eq!(b.y, a.y);
            // The world period is the width of the map in hexes.
            let wa = f.tex_to_world(a);
            let wb = f.tex_to_world(b);
            assert!((wb.x - wa.x - f.world_width()).abs() < 1e-2);
        }
        // The hex past the last column is column 0 of the same row.
        let s = TerrainSampler::new(f);
        assert_eq!(s.hex_at(cell_center(40, 5)), Some((0, 5)));
        assert_eq!(s.hex_at(cell_center(-1, 5)), Some((39, 5)));
        assert_eq!(s.hex_at(cell_center(3, 24)), None);
    }

    #[test]
    fn the_texel_plane_and_the_world_are_inverse() {
        let f = frame();
        for t in [
            Vec2::new(0.0, 0.0),
            Vec2::new(123.5, 900.25),
            Vec2::new(2559.0, 1535.0),
        ] {
            assert!(f.world_to_tex(f.tex_to_world(t)).distance(t) < 1e-3);
        }
    }

    #[test]
    fn ours_to_orig_puts_a_texel_where_the_original_formulas_do() {
        let f = frame();
        let m = f.ours_to_orig();
        for (tex, h) in [
            (Vec2::new(0.0, 0.0), 0.0),
            (Vec2::new(100.5, 200.25), 12.0),
            (Vec2::new(2560.0, 1536.0), 49.0),
        ] {
            let w = f.tex_to_world(tex);
            let ours = Vec3::new(w.x, h, w.y);
            let got = m.transform_point3(ours);
            let want = f.orig_point(tex, h);
            assert!(got.distance(want) < 1e-2, "{tex}: {got} against {want}");
        }
        // The middle of the original's texel plane (32 rows north of ours) is the origin of its world.
        let mid = Vec2::new(1280.0, 768.0 + ORIG_EXTRA_ROWS as f32 / 2.0);
        let w = f.tex_to_world(mid);
        let o = m.transform_point3(Vec3::new(w.x, 0.0, w.y));
        assert!(o.truncate().length() < 1e-2, "{o}");
        // The rotation part is the one of render-core 4.6.
        let rot = crate::frame::basis_rotation();
        assert_eq!(m.x_axis, rot.x_axis);
        assert_eq!(m.y_axis, rot.y_axis);
        assert_eq!(m.z_axis, rot.z_axis);
    }

    #[test]
    fn the_original_scale_gives_the_heights_of_b1() {
        let f = frame();
        let z = f.terrain_scale().z;
        assert_eq!(z, TERRAIN_SCALE_Z);
        assert!((13000.0 / 255.0 * z - 10.196).abs() < 0.02);
        assert!((15423.0 / 255.0 * z - 12.097).abs() < 0.02);
        // A texel is one world unit along X; the rows of hexes are 64 texels.
        assert_eq!(f.terrain_scale().x * 64.0, HEX_WIDTH);
        assert!((f.terrain_scale().y * 64.0 - ROW_STEP).abs() < 1e-3);
        assert_eq!(f.texel_size(), (2560, 1536));
        assert_eq!(
            f.half_world(),
            Vec2::new(1280.0, (768.0 + 32.0) * f.terrain_scale().y)
        );
    }

    fn sampler_with(f: impl Fn(i32, i32) -> u16) -> TerrainSampler {
        let mut s = TerrainSampler::new(TerrainFrame::new(2, 2, false));
        for v in 0..128 {
            for u in 0..128 {
                s.field_mut().set_height(u, v, f(u, v));
            }
        }
        s
    }

    #[test]
    fn the_height_is_the_nearest_texel_without_filtering() {
        let mut s = TerrainSampler::new(TerrainFrame::new(2, 2, false));
        s.field_mut().set_height(100, 60, 5100);
        s.field_mut().set_height(101, 60, 255);
        let f = *s.frame();
        let z = f.terrain_scale().z;
        for du in [0.0, 0.3, 0.99] {
            let p = f.tex_to_world(Vec2::new(100.0 + du, 60.5));
            assert!(
                (s.world_height(p) - 5100.0 / 255.0 * z).abs() < 1e-4,
                "{du}"
            );
        }
        let p = f.tex_to_world(Vec2::new(101.1, 60.5));
        assert!((s.world_height(p) - z).abs() < 1e-4);
        // A texel of an unbuilt block and one outside the map are 0.
        assert_eq!(s.world_height(f.tex_to_world(Vec2::new(10.5, 10.5))), 0.0);
        assert_eq!(s.world_height(f.tex_to_world(Vec2::new(-50.0, 10.5))), 0.0);
        assert!(!s.is_valid_world_location(f.tex_to_world(Vec2::new(-50.0, 10.5))));
        assert!(s.is_valid_world_location(f.tex_to_world(Vec2::new(50.0, 10.5))));
    }

    #[test]
    fn the_surface_is_never_below_the_sea() {
        let mut s = TerrainSampler::new(TerrainFrame::new(2, 2, false));
        s.field_mut().set_height(10, 10, 6000);
        s.field_mut().set_height(11, 10, 20000);
        let f = *s.frame();
        let low = f.tex_to_world(Vec2::new(10.5, 10.5));
        let high = f.tex_to_world(Vec2::new(11.5, 10.5));
        assert!(s.world_height(low) < s.sea_level());
        assert_eq!(s.surface_height(low), s.sea_level());
        assert_eq!(s.surface_height(high), s.world_height(high));
        assert!((s.sea_level() - 13000.0 / 255.0 * TERRAIN_SCALE_Z).abs() < 0.02);
    }

    #[test]
    fn a_flat_field_has_the_normal_straight_up() {
        let s = sampler_with(|_, _| 15423);
        let f = *s.frame();
        let p = f.tex_to_world(Vec2::new(60.5, 60.5));
        let n = s.world_normal_orig(p);
        assert!((n - Vec3::Z).length() < 1e-6, "{n}");
        let n = s.world_normal(p);
        assert!((n - Vec3::Y).length() < 1e-6, "{n}");
    }

    #[test]
    fn a_ramp_leans_the_normal_against_the_slope() {
        // 2 raw per texel along u: the derivative taps give -1.16609 per texel of the slope.
        let s = sampler_with(|u, _| 10_000 + 2 * u as u16);
        let f = *s.frame();
        let n = s.calc_normal(60, 60);
        let ratio =
            (-0.0702f32 * 2.0 - 0.3526 - 0.3526 - 0.0702 * 2.0) / (0.0702 * 2.0 + 0.3526 * 2.0);
        let slope_x = ratio * (2.0 / 255.0) * f.terrain_scale().z;
        let want = Vec3::new(
            slope_x * f.terrain_scale().y,
            0.0,
            f.terrain_scale().x * f.terrain_scale().y,
        )
        .normalize();
        assert!(n.x < 0.0);
        assert!((n - want).length() < 2e-4, "{n} against {want}");
        // A ramp along v leans y, not x.
        let s = sampler_with(|_, v| 10_000 + 2 * v as u16);
        let n = s.calc_normal(60, 60);
        assert!(n.y < 0.0 && n.x.abs() < 1e-6, "{n}");
    }

    #[test]
    fn a_cliff_gives_a_unit_normal_that_leans_and_flat_ground_far_from_it_does_not() {
        // The range weight cuts the taps across a cliff down, so the sign of the lean is not that
        // of a linear filter; what holds is the length of the normal and the flat ground beyond
        // the reach of the kernel.
        let s = sampler_with(|u, _| if u < 64 { 10_000 } else { 30_000 });
        let near = s.calc_normal(62, 60);
        let far = s.calc_normal(40, 60);
        assert!((near.length() - 1.0).abs() < 1e-5);
        assert!(near.x.abs() > 1e-3, "{near}");
        assert!(far.z > 0.9999 && far.x.abs() < 1e-6);
        // The taps of texel 61 end at 63: they do not reach the cliff.
        assert!(s.calc_normal(61, 60).x.abs() < 1e-6);
    }

    #[test]
    fn the_wrap_along_x_reads_the_other_edge() {
        let mut open = TerrainSampler::new(TerrainFrame::new(2, 2, false));
        let mut wrapped = TerrainSampler::new(TerrainFrame::new(2, 2, true));
        for s in [&mut open, &mut wrapped] {
            s.field_mut().set_height(3, 50, 7000);
            s.field_mut().set_height(127, 50, 9000);
        }
        let f = *wrapped.frame();
        // One period east of texel 3.
        let p = f.tex_to_world(Vec2::new(3.5, 50.5)) + Vec2::new(f.world_width(), 0.0);
        let want = 7000.0 / 255.0 * f.terrain_scale().z;
        assert!((wrapped.world_height(p) - want).abs() < 1e-3);
        assert_eq!(open.world_height(p), 0.0);
        // The taps of the normal cross the seam too: the texel -1 is the last one.
        assert_eq!(wrapped.raw_height(-1, 50), 9000);
        assert_eq!(open.raw_height(-1, 50), 0);
        assert_eq!(wrapped.raw_height(128 + 3, 50), 7000);
    }

    #[test]
    fn the_hex_location_stands_on_the_ground_of_its_centre() {
        let mut s = TerrainSampler::new(TerrainFrame::new(4, 4, false));
        let f = *s.frame();
        let c = f.hex_to_tex(2, 1);
        s.field_mut().set_height(c.x as i32, c.y as i32, 15423);
        let loc = s.hex_location(2, 1);
        let want = cell_center(2, 1);
        assert!(Vec2::new(loc.x, loc.z).distance(want) < 1e-3);
        assert!((loc.y - 15423.0 / 255.0 * TERRAIN_SCALE_Z).abs() < 0.02);
    }

    #[test]
    fn the_revision_grows_with_the_heights() {
        let mut s = TerrainSampler::new(TerrainFrame::new(1, 1, false));
        let r = s.height_revision();
        s.field_mut().set_height(3, 3, 100);
        assert!(s.height_revision() > r);
        let r = s.height_revision();
        s.field_mut().set_height(3, 3, 100);
        assert_eq!(s.height_revision(), r);
    }

    /// The block reads its taps from one window: every normal is the one of the per-texel filter, bit
    /// for bit, at the edges of the map and over the seam of a wrapping map too.
    #[test]
    fn the_block_normals_are_the_per_texel_normals() {
        for wrap in [false, true] {
            let mut s = TerrainSampler::new(TerrainFrame::new(2, 2, wrap));
            let mut seed = 12345u32;
            for v in 0..128 {
                for u in 0..128 {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    s.field_mut()
                        .set_height(u, v, 10_000 + ((seed >> 16) % 9000) as u16);
                }
            }
            for (origin, size) in [
                ((-3, -2), (12, 9)),
                ((120, 60), (13, 7)),
                ((30, 118), (5, 14)),
            ] {
                let block = s.normals_block(origin, size);
                for y in 0..size.1 {
                    for x in 0..size.0 {
                        let want = s.normal_with_taps(origin.0 + x, origin.1 + y, true);
                        let got = block[(y * size.0 + x) as usize];
                        assert_eq!(got.to_array(), want.to_array(), "{wrap} {origin:?} {x} {y}");
                    }
                }
            }
        }
    }

    /// A block asked for again comes from the cache while the heights under it stay, and is made again
    /// when a height under it (or in the border of its taps) changes; a change elsewhere keeps it.
    #[test]
    fn the_block_normals_are_kept_until_the_heights_under_them_change() {
        let mut s = sampler_with(|u, v| (10_000 + u * 37 + v * 11) as u16);
        let a = s.normals_block((20, 20), (12, 12));
        assert_eq!(s.normals_block((20, 20), (12, 12)), a);
        assert_eq!(s.normal_cache_stats().1, 1);
        // Far from the block (another block of the field): kept.
        s.field_mut().set_height(100, 100, 40_000);
        assert_eq!(s.normals_block((20, 20), (12, 12)), a);
        assert_eq!(s.normal_cache_stats().1, 2);
        // In the border of the taps: made again, as the per-texel filter gives it.
        s.field_mut().set_height(18, 25, 40_000);
        let b = s.normals_block((20, 20), (12, 12));
        assert_ne!(b, a);
        for y in 0..12 {
            for x in 0..12 {
                assert_eq!(
                    b[(y * 12 + x) as usize].to_array(),
                    s.normal_with_taps(20 + x, 20 + y, true).to_array()
                );
            }
        }
        assert_eq!(s.normal_cache_stats().1, 2);
    }

    /// A block made a few rows at a time is the block made at once, and is kept for the bake.
    #[test]
    fn a_block_of_normals_made_over_frames_is_the_same_block() {
        let s = sampler_with(|u, v| (10_000 + (u * u) % 977 + v * 13) as u16);
        let fresh = sampler_with(|u, v| (10_000 + (u * u) % 977 + v * 13) as u16);
        let mut p = s.begin_normals((-1, 30), (67, 23)).expect("not kept yet");
        let mut steps = 0;
        while !s.continue_normals(&mut p, 5) {
            steps += 1;
        }
        assert_eq!(steps, 4);
        assert!(s.begin_normals((-1, 30), (67, 23)).is_none(), "kept");
        let (_, hits, _) = s.normal_cache_stats();
        let got = s.normals_block((-1, 30), (67, 23));
        assert_eq!(s.normal_cache_stats().1, hits + 1);
        let want = fresh.normals_block((-1, 30), (67, 23));
        assert!(
            got.iter()
                .zip(&want)
                .all(|(a, b)| a.to_array() == b.to_array())
        );
        assert_eq!(got.len(), want.len());
    }

    #[test]
    fn the_block_normals_use_the_edge_texels_past_the_map() {
        let s = sampler_with(|_, _| 15423);
        let block = s.normals_block((0, 0), (12, 12));
        assert_eq!(block.len(), 144);
        assert!(block.iter().all(|n| (*n - Vec3::Z).length() < 1e-6));
        // At the corner the unclamped taps would be missing; the block does not lean.
        let corner = s.normals_block((0, 0), (1, 1))[0];
        assert!((corner - Vec3::Z).length() < 1e-6);
    }
}
