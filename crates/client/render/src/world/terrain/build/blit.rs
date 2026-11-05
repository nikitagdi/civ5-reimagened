//! The blit of the pieces: the port of `CellBlitJob` and of
//! the set-up of its jobs in `cvTerrainBlend::LaunchBlendTerrainJobs`.
//!
//! The original works on the padded field (two hex rows of padding above and below, two columns
//! each side when the map does not wrap) in jobs of 64 x 64 texels. Our field is the map without
//! the padding, so a texel of ours is the texel of the original shifted by [`Blit::shift`]; a cell
//! of ours is made of up to four cells of the original, each run as the original runs it (the
//! classes of a cell depend on where its borders are).
//!
//! A job: every texel starts as "no class" (4); the lines of the coast splines (range A) mark
//! their texels, the lines of range B seed a flood fill that stops at marked texels, the hexes
//! seed the rest with their class; "no class" left becomes 2. Then every piece whose pattern type
//! is the class of a texel adds its heights, weighted, and the piece with the highest priority
//! gives the terrain id.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use asset_format::{PieceData, PieceStyle, TerrainPiece};

use super::matcher::{HexMat, HexPatternMatcher, PatternPiece, SquareGrid};
use super::tables::{WorldMatrices, transform4};
use super::{BlendSpline, SplineRange};
use crate::world::terrain::basis::TerrainBasis;
use crate::world::terrain::field::CellRect;
use crate::world::terrain::library::{TerrainDefs, TerrainLibrary};

/// Texels of a job along one side.
// re: cvTerrainBlend.cpp:29 (kCellSize)
pub const JOB: i32 = 64;
/// Texel with no class yet.
const NO_CLASS: u8 = 4;
/// The terrain id of a texel no piece reaches (`Fun_0891bc58`).
// re: CellBlitJob::Fun_0891bc58 0x0891bc58
const DEFAULT_ID: u8 = 3;
/// Starts the weight sum so a texel no piece covers keeps a finite height.
const WEIGHT_EPSILON: f32 = 1.0e-6;
/// `cvTerrainBlend::field_1dc`: the offset of the stored heights; raw 0x3c3f is the plain land.
// re: cvTerrainBlend::cvTerrainBlend 0x089180ec
pub const HEIGHT_OFFSET: f32 = 0.235_355_15;
/// Texels the original's field is wider/higher than ours on each side before the first hex.
const PAD_V: i32 = 96;
const PAD_U_NO_WRAP: i32 = 96;
/// The corners of a hex the classes seed: `kCornerOffsets`.
const CORNER_OFFSETS: [(i32, i32); 4] = [(1, 1), (-1, 1), (1, -1), (-1, -1)];

/// `ComputePatternType`: 0 for land, 1 for coast (and reef), 2 for ocean, 4 for a mix.
// re: cvBaseTerrainPiecesLibrary::ComputePatternType 0x0891e38c
pub fn pattern_type(piece: &PatternPiece) -> u8 {
    let (mut other, mut mask_a, mut bit14) = (0, 0, 0);
    for &(row, column) in &piece.used {
        let c = &piece.cells[usize::from(column) * 10 + usize::from(row)];
        if !c.valid {
            continue;
        }
        let code = c.hex_code;
        if code & 0xf3ef_9fff != 0 {
            other += 1;
        } else if code & 0x0c10_2000 != 0 {
            mask_a += 1;
        } else {
            bit14 += (code >> 14) & 1;
        }
    }
    if mask_a != 0 && other == 0 && bit14 == 0 {
        1
    } else if bit14 != 0 && other == 0 && mask_a == 0 {
        2
    } else if other == 0 || bit14 != 0 || mask_a != 0 {
        4
    } else {
        0
    }
}

/// The class of a hex of the grid: land 0, coast 1, ocean 2.
// re: TerrainClassGrid::Init 0x088e3dd4
pub fn hex_class(flags: u32) -> u8 {
    if flags & 0xf3ef_9fff == 0 {
        u8::from(flags & 0x0c10_2000 == 0) + 1
    } else {
        0
    }
}

/// A piece ready to blit: planes in the orientation of the original's library.
pub struct BlitPiece {
    side: usize,
    heights: Vec<u16>,
    ids: Vec<u8>,
    weights: Vec<u8>,
    priorities: Vec<u8>,
}

impl BlitPiece {
    /// Loads a piece as `LoadTerrainPiece` does: the heights and the blend maps are stored
    /// bottom-up (the row `y` is the file row `side - 1 - y`), the id map as it is; the id map
    /// and the blend maps are cut to the discs around the hexes of the piece.
    // re: cvCompressedTerrainPiecesLibrary::LoadTerrainPiece 0x0891ee4a, LoadHeightmapFromFile
    // 0x0891ff36 (third channel), LoadIDMapFromFile 0x0892026c, LoadBlendmapFromFile 0x089205a2
    pub fn load(
        piece: &TerrainPiece,
        data: &PieceData,
        hexes: &[[f32; 3]],
        mats: &WorldMatrices,
        id_row: &dyn Fn(u32) -> u8,
    ) -> Self {
        let side = usize::from(piece.side);
        let flip = |y: usize| side - 1 - y;
        let mut heights = vec![0u16; side * side];
        for y in 0..side {
            heights[y * side..(y + 1) * side]
                .copy_from_slice(&data.height_g[flip(y) * side..(flip(y) + 1) * side]);
        }
        for d in &data.height_diff {
            let i = d.index as usize;
            let (fy, x) = (i / side, i % side);
            heights[flip(fy) * side + x] = data.height_g[i].wrapping_add_signed(d.db);
        }
        let palette: Vec<u8> = data.id_palette.iter().map(|&c| id_row(c)).collect();
        let mut ids: Vec<u8> = data
            .id_index
            .iter()
            .map(|&i| palette[usize::from(i)])
            .collect();
        let plane = |p: &[u8]| {
            let mut o = vec![0u8; side * side];
            for y in 0..side {
                o[y * side..(y + 1) * side]
                    .copy_from_slice(&p[flip(y) * side..(flip(y) + 1) * side]);
            }
            o
        };
        let mut weights = plane(&data.h_blend[0]);
        let mut priorities = plane(&data.t_blend[0]);
        // The distance in hexes to the nearest hex of the piece.
        // re: Fun_08924890 0x08924890, Fun_08924b3e 0x08924b3e
        for y in 0..side {
            for x in 0..side {
                let h = mats.piece_texel_to_hex(x as f32, y as f32);
                let mut d = f32::MAX;
                for c in hexes {
                    let (dx, dy) = (h[0] - c[0], h[1] - c[1]);
                    let dz = -dx - dy;
                    let v =
                        ((dz * dz + dy * dy + dx * dx) as f64).sqrt() / std::f64::consts::SQRT_2;
                    d = d.min(v as f32);
                }
                let i = y * side + x;
                if d >= 1.0 {
                    ids[i] = 0;
                }
                weights[i] = if d >= 2.0 { 0 } else { weights[i].max(1) };
                priorities[i] = if d >= 1.0 { 0 } else { priorities[i].max(1) };
            }
        }
        Self {
            side,
            heights,
            ids,
            weights,
            priorities,
        }
    }
}

/// `BlendCell`: a placed piece in texels of the original.
#[derive(Clone, Copy, Debug)]
struct BlendCell {
    /// Index into the pieces of the matcher.
    piece: u32,
    pattern: u8,
    c0: (i32, i32),
    c1: (i32, i32),
}

/// `m_aCornerPositions`: a seed of a class.
#[derive(Clone, Copy, Debug)]
struct Seed {
    pos: (i32, i32),
    class: u8,
}

/// A line of the blend in texels of the original, with the class it marks.
#[derive(Clone, Debug)]
struct Line {
    points: Vec<(i32, i32)>,
    class: u8,
}

/// The heights and ids of one job.
pub struct JobOut {
    heights: Vec<u16>,
    ids: Vec<u8>,
}

/// Where the matcher's pieces are in the library of assets.
#[derive(Clone, Debug)]
pub struct PieceRef {
    pub style: PieceStyle,
    pub index: usize,
    /// The piece is the 128 one (flag 1).
    pub small: bool,
}

/// Everything the jobs of one basis share.
pub struct Blit {
    map: (i32, i32),
    cells: (i32, i32),
    shift: (i32, i32),
    blends: Vec<BlendCell>,
    blend_lists: HashMap<(i32, i32), Vec<u32>>,
    seeds: HashMap<(i32, i32), Vec<Seed>>,
    lines_a: Vec<Line>,
    lines_b: Vec<Line>,
    lines_a_of: HashMap<(i32, i32), Vec<u32>>,
    lines_b_of: HashMap<(i32, i32), Vec<u32>>,
    id_rows: HashMap<u32, u8>,
    refs: Vec<PieceRef>,
    hexes: Vec<Vec<[f32; 3]>>,
    mats: WorldMatrices,
    pieces: Mutex<Lru<u32, BlitPiece>>,
    jobs: Mutex<Lru<(i32, i32), JobOut>>,
}

/// Bytes the loaded pieces may hold ([`Blit::piece`]): the pieces a huge map uses, at about a third of a
/// megabyte each for the big ones. Emptied whole at 96 pieces, the cache decoded the same big pieces
/// again and again as the field built across the map (cells of 20 to 40 ms).
const PIECE_CACHE_BYTES: usize = 96 << 20;
/// Jobs of the original kept ([`Blit::job`]), 12 KB each: the four jobs a cell of ours reads are the
/// ones its neighbours read too.
const JOB_CACHE_BYTES: usize = 16 << 20;

/// A cache that gives up the entry used longest ago when its bytes are past the limit.
struct Lru<K, V> {
    map: HashMap<K, (Arc<V>, u64, usize)>,
    clock: u64,
    bytes: usize,
    limit: usize,
}

impl<K: std::hash::Hash + Eq + Copy, V> Lru<K, V> {
    fn new(limit: usize) -> Self {
        Self {
            map: HashMap::new(),
            clock: 0,
            bytes: 0,
            limit,
        }
    }

    fn get(&mut self, key: &K) -> Option<Arc<V>> {
        self.clock += 1;
        let clock = self.clock;
        self.map.get_mut(key).map(|e| {
            e.1 = clock;
            Arc::clone(&e.0)
        })
    }

    fn insert(&mut self, key: K, value: Arc<V>, bytes: usize) {
        self.clock += 1;
        if let Some(old) = self.map.insert(key, (value, self.clock, bytes)) {
            self.bytes -= old.2;
        }
        self.bytes += bytes;
        while self.bytes > self.limit && self.map.len() > 1 {
            let Some(oldest) = self
                .map
                .iter()
                .filter(|(k, _)| **k != key)
                .min_by_key(|(_, e)| e.1)
                .map(|(k, _)| *k)
            else {
                break;
            };
            if let Some(e) = self.map.remove(&oldest) {
                self.bytes -= e.2;
            }
        }
    }
}

/// `kClassGridSize`-bounded loops of the original use the plain `/` of C: truncation to zero.
fn tdiv(a: i32, b: i32) -> i32 {
    a / b
}

/// The code a row of `TerrainDef` has in the id maps: the hex digits of the decimal `ID` column
/// with the prefix of the continent.
// re: TerrainData::LoadTerrainDefinition 0x0891d24c (`%x` of the last column, `| g_aContinentPrefix`)
pub fn row_code(id: u32, continent: u8) -> u32 {
    let hex = u32::from_str_radix(&id.to_string(), 16).unwrap_or(0);
    hex | super::tables::CONTINENT_PREFIX[usize::from(continent.min(4))]
}

impl Blit {
    /// The set-up of the jobs for a basis whose pieces were placed by `matcher`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        basis: &TerrainBasis,
        matcher: &HexPatternMatcher,
        grid: &super::matcher::PatternGrid,
        refs: Vec<PieceRef>,
        defs: &TerrainDefs,
        splines: &[BlendSpline],
    ) -> Self {
        let wrap = basis.wrap_x;
        let (w, h) = (i32::from(basis.width), i32::from(basis.height));
        let map = if wrap {
            (w * 64, (h + 4) * 64)
        } else {
            ((w + 4) * 64, (h + 4) * 64)
        };
        let cells = (map.0 / JOB, map.1 / JOB);
        let shift = (if wrap { 0 } else { PAD_U_NO_WRAP }, PAD_V);
        let mats = WorldMatrices::new(wrap);

        let mut id_rows = HashMap::new();
        for (i, r) in defs.rows().iter().enumerate() {
            id_rows
                .entry(row_code(r.id, r.continent))
                .or_insert(i.min(255) as u8);
        }

        // Each hex of the class grid seeds the cells around its centre with its class.
        let mut seeds: HashMap<(i32, i32), Vec<Seed>> = HashMap::new();
        let policy = grid.policy;
        for row in 0..policy.height {
            for col in 0..policy.width {
                let class = grid.cell(col, row).map_or(0, |c| hex_class(c.flags));
                let hex = policy.inverse.apply(SquareGrid::hex_of(col, row));
                let t = mats.hex_to_texel([hex[0] as f32, hex[1] as f32, hex[2] as f32]);
                let (tx, ty) = (t[0] as i32, t[1] as i32);
                for (ox, oy) in CORNER_OFFSETS {
                    let (x, y) = (ox + tx, oy + ty);
                    let (cx, cy) = (tdiv(x, JOB), tdiv(y, JOB));
                    if cx >= cells.0 || cy >= cells.1 {
                        continue;
                    }
                    seeds
                        .entry((cx, cy))
                        .or_default()
                        .push(Seed { pos: (x, y), class });
                }
            }
        }

        // The corners of the small and the large piece, one set per transform.
        let corners: Vec<[(i32, i32); 4]> = (0..4)
            .map(|t| corner_offsets(&mats, matcher.transform_matrix(t)))
            .collect();

        let mut blends = Vec::new();
        let mut blend_lists: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for pos in matcher.positions() {
            let piece = &matcher.pieces()[pos.piece as usize];
            let small = refs
                .get(piece.library_index as usize)
                .is_some_and(|r| r.small);
            let anchor =
                mats.hex_to_texel([pos.hex[0] as f32, pos.hex[1] as f32, pos.hex[2] as f32]);
            let (tx, ty) = (anchor[0].round() as i32, anchor[1].round() as i32);
            let ext = &corners[pos.transform as usize][if small { 0 } else { 2 }..];
            let c0 = (ext[0].0 + tx, ext[0].1 + ty);
            let c1 = (tx + ext[1].0, ty + ext[1].1);
            let index = blends.len() as u32;
            blends.push(BlendCell {
                piece: pos.piece,
                pattern: pattern_type(piece),
                c0,
                c1,
            });
            let (min_x, max_x) = (c0.0.min(c1.0), c0.0.max(c1.0));
            let (min_y, max_y) = (c0.1.min(c1.1), c0.1.max(c1.1));
            for cx in tdiv(min_x, JOB)..tdiv(max_x, JOB) + 1 {
                for cy in tdiv(min_y, JOB)..tdiv(max_y, JOB) + 1 {
                    if cy < 0 || cy >= cells.1 {
                        continue;
                    }
                    let x = (cx + cells.0 * 100).rem_euclid(cells.0);
                    blend_lists.entry((x, cy)).or_default().push(index);
                }
            }
        }

        // Each spline goes to every job its points pass through.
        let mut lines_a = Vec::new();
        let mut lines_b = Vec::new();
        let mut lines_a_of: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        let mut lines_b_of: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for s in splines {
            let line = Line {
                points: s
                    .points
                    .iter()
                    .map(|p| (p.x as i32 + shift.0, p.y as i32 + shift.1))
                    .collect(),
                class: s.class,
            };
            let (list, of) = match s.range {
                SplineRange::A => (&mut lines_a, &mut lines_a_of),
                SplineRange::B => (&mut lines_b, &mut lines_b_of),
            };
            let index = list.len() as u32;
            let mut last = (-1, 0);
            for &(mut x, mut y) in &line.points {
                while x < 0 {
                    x += map.0;
                }
                while y < 0 {
                    y += map.1;
                }
                let c = (x % map.0 / JOB, y % map.1 / JOB);
                if c == last {
                    continue;
                }
                last = c;
                let v = of.entry(c).or_default();
                if !v.contains(&index) {
                    v.push(index);
                }
            }
            list.push(line);
        }

        let hexes = matcher
            .pieces()
            .iter()
            .map(|p| {
                p.used
                    .iter()
                    .filter(|&&(row, column)| {
                        p.cells[usize::from(column) * 10 + usize::from(row)].valid
                    })
                    .map(|&(row, column)| {
                        let h = SquareGrid::hex_of(i32::from(row), i32::from(column));
                        [h[0] as f32, h[1] as f32, h[2] as f32]
                    })
                    .collect()
            })
            .collect();

        Self {
            map,
            cells,
            shift,
            blends,
            blend_lists,
            seeds,
            lines_a,
            lines_b,
            lines_a_of,
            lines_b_of,
            id_rows,
            refs,
            hexes,
            mats,
            pieces: Mutex::new(Lru::new(PIECE_CACHE_BYTES)),
            jobs: Mutex::new(Lru::new(JOB_CACHE_BYTES)),
        }
    }

    /// Texels of the original's padded field, and the shift from ours to it.
    pub fn map_size(&self) -> (i32, i32) {
        self.map
    }

    pub fn shift(&self) -> (i32, i32) {
        self.shift
    }

    /// The blend cells (placed pieces) and the seeds the job `(cx, cy)` of the original has.
    pub fn job_counts(&self, cx: i32, cy: i32) -> (usize, usize) {
        (
            self.blend_lists.get(&(cx, cy)).map_or(0, Vec::len),
            self.seeds.get(&(cx, cy)).map_or(0, Vec::len),
        )
    }

    /// Writes the texels of `rect` (ours) into `put(u, v, height, id)`.
    pub fn raster(
        &self,
        rect: CellRect,
        lib: &TerrainLibrary,
        put: &mut dyn FnMut(i32, i32, u16, u8),
    ) {
        let (su, sv) = self.shift;
        let (oy0, oy1) = (
            (rect.v0 + sv).div_euclid(JOB),
            (rect.v1 - 1 + sv).div_euclid(JOB),
        );
        let (ox0, ox1) = (
            (rect.u0 + su).div_euclid(JOB),
            (rect.u1 - 1 + su).div_euclid(JOB),
        );
        for oy in oy0..=oy1 {
            for ox in ox0..=ox1 {
                if oy < 0 || oy >= self.cells.1 {
                    continue;
                }
                let ox_wrapped = ox.rem_euclid(self.cells.0);
                let job = self.job(ox_wrapped, oy, lib);
                for v in (oy * JOB - sv).max(rect.v0)..((oy + 1) * JOB - sv).min(rect.v1) {
                    for u in (ox * JOB - su).max(rect.u0)..((ox + 1) * JOB - su).min(rect.u1) {
                        let (lx, ly) = ((u + su - ox * JOB) as usize, (v + sv - oy * JOB) as usize);
                        put(u, v, job.heights[ly * 64 + lx], job.ids[ly * 64 + lx]);
                    }
                }
            }
        }
    }

    fn job(&self, cx: i32, cy: i32, lib: &TerrainLibrary) -> Arc<JobOut> {
        if let Some(j) = jobs::lock_without_wait(&self.jobs).get(&(cx, cy)) {
            return j;
        }
        let out = Arc::new(self.run_job(cx, cy, lib));
        let bytes = out.heights.len() * 2 + out.ids.len();
        jobs::lock_without_wait(&self.jobs).insert((cx, cy), Arc::clone(&out), bytes);
        out
    }

    fn piece(&self, index: u32, lib: &TerrainLibrary) -> Option<Arc<BlitPiece>> {
        if let Some(p) = jobs::lock_without_wait(&self.pieces).get(&index) {
            return Some(p);
        }
        let r = self.refs.get(index as usize)?;
        let piece = lib.piece_set(r.style)?.pieces.get(r.index)?;
        let data = piece.data().ok()?;
        let rows = &self.id_rows;
        let built = Arc::new(BlitPiece::load(
            piece,
            &data,
            &self.hexes[index as usize],
            &self.mats,
            &|code| rows.get(&code).copied().unwrap_or(0),
        ));
        let bytes = built.heights.len() * 2
            + built.ids.len()
            + built.weights.len()
            + built.priorities.len();
        jobs::lock_without_wait(&self.pieces).insert(index, Arc::clone(&built), bytes);
        Some(built)
    }

    /// `CellBlitJob::Execute` on the job `(cx, cy)` of the original.
    // re: CellBlitJob::Execute 0x0891bc02
    fn run_job(&self, cx: i32, cy: i32, lib: &TerrainLibrary) -> JobOut {
        let mut t = Texels::new((cx * JOB, cy * JOB), self.map);
        self.classify(&mut t, cx, cy);
        self.blend(&mut t, cx, cy, lib);
        t.finish()
    }

    // re: CellBlitJob::Fun_0891bcd2 0x0891bcd2
    fn classify(&self, t: &mut Texels, cx: i32, cy: i32) {
        let key = (cx, cy);
        let mut stack: Vec<(i32, i32)> = Vec::new();
        for &i in self.lines_a_of.get(&key).map_or(&[][..], Vec::as_slice) {
            let line = &self.lines_a[i as usize];
            if line.points.len() < 2 {
                continue;
            }
            t.class = line.class;
            let mut p0 = line.points[0];
            for &p1 in &line.points[1..] {
                rasterize_line(p0, p1, &mut |pos| t.mark(pos));
                p0 = p1;
            }
        }
        for &i in self.lines_b_of.get(&key).map_or(&[][..], Vec::as_slice) {
            let line = &self.lines_b[i as usize];
            t.class = line.class;
            if line.points.len() >= 2 {
                let mut p0 = line.points[0];
                for &p1 in &line.points[1..] {
                    rasterize_line(p0, p1, &mut |pos| {
                        let w = t.wrap(pos);
                        if t.inside(w) && t.class_at(w) == NO_CLASS {
                            t.seed(w, &mut stack);
                        }
                    });
                    p0 = p1;
                }
            }
            t.flood(&mut stack);
        }
        for s in self.seeds.get(&key).map_or(&[][..], Vec::as_slice) {
            t.class = s.class;
            if !t.classified(s.pos) {
                t.seed(s.pos, &mut stack);
                t.flood(&mut stack);
            }
        }
        for tex in &mut t.texels {
            if tex.class == NO_CLASS {
                tex.class = 2;
            }
        }
    }

    // re: CellBlitJob::Fun_0891bfd0 0x0891bfd0
    fn blend(&self, t: &mut Texels, cx: i32, cy: i32, lib: &TerrainLibrary) {
        let (kmin, kmax) = (t.min, (t.min.0 + JOB, t.min.1 + JOB));
        let map = self.map;
        for &bi in self
            .blend_lists
            .get(&(cx, cy))
            .map_or(&[][..], Vec::as_slice)
        {
            let b = self.blends[bi as usize];
            let Some(piece) = self.piece(b.piece, lib) else {
                continue;
            };
            let step_y = if b.c0.1 < b.c1.1 { 1 } else { -1 };
            let step_x = if b.c0.0 < b.c1.0 { 1 } else { -1 };
            let (mut x0, mut x1) = (b.c0.0, b.c1.0);
            if x0.min(x1) > kmax.0 {
                x0 -= map.0;
                x1 -= map.0;
            }
            if x0.max(x1) < kmin.0 {
                x0 += map.0;
                x1 += map.0;
            }
            let (first_x, end_x, src_x0);
            if step_x > 0 {
                first_x = kmin.0.max(x0);
                end_x = kmax.0.min(x1);
                src_x0 = first_x - x0;
            } else {
                let low = kmin.0.max(x1);
                let high = kmax.0.min(x0);
                src_x0 = x0 - high;
                first_x = high - 1;
                end_x = low - 1;
            }
            let (y0, y1) = (b.c0.1, b.c1.1);
            let (first_y, end_y, src_y0);
            if step_y > 0 {
                first_y = kmin.1.max(y0);
                end_y = kmax.1.min(y1);
                src_y0 = first_y - y0;
            } else {
                let low = kmin.1.max(y1);
                let high = kmax.1.min(y0);
                src_y0 = y0 - high;
                first_y = high - 1;
                end_y = low - 1;
            }
            // A range that runs against its step has nothing to blit.
            if (step_x > 0 && first_x >= end_x)
                || (step_x < 0 && first_x <= end_x)
                || (step_y > 0 && first_y >= end_y)
                || (step_y < 0 && first_y <= end_y)
            {
                continue;
            }
            let side = piece.side as i32;
            let mut ry = 0;
            let mut y = first_y;
            while y != end_y {
                let mut rx = 0;
                let mut x = first_x;
                while x != end_x {
                    let map_x = (self.map.0 * 10 + x).rem_euclid(self.map.0);
                    let (sx, sy) = (src_x0 + rx, src_y0 + ry);
                    rx += 1;
                    if map_x >= kmin.0
                        && map_x < kmax.0
                        && y >= kmin.1
                        && y < kmax.1
                        && sx >= 0
                        && sx < side
                        && sy >= 0
                        && sy < side
                    {
                        let si = (sy * side + sx) as usize;
                        let tex = &mut t.texels[((y % 64) * 64 + map_x % 64) as usize];
                        if tex.class == b.pattern {
                            if piece.priorities[si] > tex.piece_weight {
                                tex.piece_weight = piece.priorities[si];
                                tex.piece_id = piece.ids[si];
                            }
                            let w = f32::from(piece.weights[si]) / 255.0;
                            tex.height_sum +=
                                (f32::from(piece.heights[si]) / 65535.0 - HEIGHT_OFFSET) * w;
                            tex.weight_sum += w;
                        }
                    }
                    x += step_x;
                }
                ry += 1;
                y += step_y;
            }
        }
    }
}

/// The corners of the small (0, 1) and the large (2, 3) piece in texels from the anchor, for the
/// transform `m`.
// re: cvTerrainBlend::LaunchBlendTerrainJobs 0x08918592 (`aCorners`)
pub fn corner_offsets(mats: &WorldMatrices, m: &HexMat) -> [(i32, i32); 4] {
    let origin = transform4([0.0, 0.0, -0.0, 1.0], &mats.hex_to_tex);
    let mut out = [(0, 0); 4];
    for (c, o) in out.iter_mut().enumerate() {
        let src = if c < 2 {
            mats.small_corners[c & 1]
        } else {
            mats.large_corners[c & 1]
        };
        let local = mat_transform(src, m);
        let tex = transform4(local, &mats.hex_to_tex);
        *o = (
            (tex[0] - origin[0]).round() as i32,
            (tex[1] - origin[1]).round() as i32,
        );
    }
    out
}

/// `v * M` with the integer matrix of a transform.
fn mat_transform(v: [f32; 4], m: &HexMat) -> [f32; 4] {
    let mut o = [0.0f32; 4];
    for (j, oj) in o.iter_mut().enumerate() {
        *oj = (0..4).map(|i| v[i] * m.0[i * 4 + j] as f32).sum();
    }
    o
}

/// One texel of the job (`BlendTexel`).
#[derive(Clone, Copy)]
struct Texel {
    height_sum: f32,
    weight_sum: f32,
    class: u8,
    piece_id: u8,
    piece_weight: u8,
}

/// The texels of a job and its classifier.
struct Texels {
    texels: Vec<Texel>,
    min: (i32, i32),
    map: (i32, i32),
    class: u8,
}

impl Texels {
    // re: CellBlitJob::Fun_0891bc58 0x0891bc58
    fn new(min: (i32, i32), map: (i32, i32)) -> Self {
        Self {
            texels: vec![
                Texel {
                    height_sum: 0.0,
                    weight_sum: WEIGHT_EPSILON,
                    class: NO_CLASS,
                    piece_id: DEFAULT_ID,
                    piece_weight: 0,
                };
                64 * 64
            ],
            min,
            map,
            class: 2,
        }
    }

    fn wrap(&self, p: (i32, i32)) -> (i32, i32) {
        (
            (self.map.0 * 100 + p.0).rem_euclid(self.map.0),
            (self.map.1 * 100 + p.1).rem_euclid(self.map.1),
        )
    }

    fn inside(&self, w: (i32, i32)) -> bool {
        w.0 >= self.min.0 && w.0 < self.min.0 + JOB && w.1 >= self.min.1 && w.1 < self.min.1 + JOB
    }

    fn class_at(&self, w: (i32, i32)) -> u8 {
        self.texels[((w.1 % 64) * 64 + w.0 % 64) as usize].class
    }

    /// `TerrainGenCallback::Fun_0891ca60`: classified, or outside the job.
    fn classified(&self, p: (i32, i32)) -> bool {
        let w = self.wrap(p);
        !self.inside(w) || self.class_at(w) != NO_CLASS
    }

    /// `TerrainGenCallback::Fun_0891cb4c`.
    fn mark(&mut self, p: (i32, i32)) {
        let w = self.wrap(p);
        if self.inside(w) {
            let c = self.class;
            self.texels[((w.1 % 64) * 64 + w.0 % 64) as usize].class = c;
        }
    }

    /// `TerrainFloodFill::Fun_0891cad8`.
    fn seed(&mut self, p: (i32, i32), stack: &mut Vec<(i32, i32)>) {
        if self.classified(p) {
            return;
        }
        self.mark(p);
        stack.push(p);
    }

    /// `TerrainFloodFill::Fun_0891c9a6`.
    fn flood(&mut self, stack: &mut Vec<(i32, i32)>) {
        while let Some((x, y)) = stack.pop() {
            for n in [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)] {
                self.seed(n, stack);
            }
        }
    }

    // re: CellBlitJob::Fun_0891c5e8 0x0891c5e8
    fn finish(self) -> JobOut {
        let mut heights = Vec::with_capacity(64 * 64);
        let mut ids = Vec::with_capacity(64 * 64);
        for t in &self.texels {
            let h = f64::from(t.height_sum / t.weight_sum + HEIGHT_OFFSET) * 65535.0;
            heights.push(h as i32 as u16);
            ids.push(t.piece_id);
        }
        JobOut { heights, ids }
    }
}

/// `RasterizeLine<..>`: a Bresenham line along its major axis, from the smaller coordinate up.
// re: RasterizeLine<TerrainLineSeedCallback> 0x0891c889
pub fn rasterize_line(a: (i32, i32), b: (i32, i32), f: &mut dyn FnMut((i32, i32))) {
    let (x0, y0, x1, y1) = (a.0, a.1, b.0, b.1);
    let dy = (y1 - y0).abs();
    let dx = (x1 - x0).abs();
    let steep = dx < dy;
    // Major axis (p0 -> p1) and minor axis (m0 -> m1).
    let (mut p0, p1, mut m0, mut m1) = if steep {
        (y0, y1, x0, x1)
    } else {
        (x0, x1, y0, y1)
    };
    if p1 < p0 {
        std::mem::swap(&mut m0, &mut m1);
    }
    p0 = p0.min(p1);
    let p_max = if steep { y0.max(y1) } else { x0.max(x1) };
    let step = if m1 - m0 != 0 && m0 <= m1 { 1 } else { -1 };
    let dm = (m1 - m0).abs();
    let span = p_max - p0;
    let mut err = span / 2;
    let mut minor = m0;
    for major in p0..=p_max {
        if steep {
            f((minor, major));
        } else {
            f((major, minor));
        }
        err -= dm;
        if err < 0 {
            minor += step;
            err += span;
        }
    }
}

#[cfg(test)]
mod tests;
