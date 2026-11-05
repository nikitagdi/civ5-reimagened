//! The surf of the coast (world-water 4.7, WW-7, card R5.8): `cvWaveSystem` of the original
//! (RW-22 (a)) without the GPU. `world/waves.rs` draws what this makes.
//!
//! - **Edges** ([`coast_edges`]): every side of a land hex whose neighbour is water (terrain
//!   coast or ocean, the wrap along X followed); `SpawnWaves` 0x0893c67c.
//! - **The spline of an edge** ([`edge_spline`]): the control points of the edge before it, the
//!   edge and the edge after it along the coast (`GetSplineForEdge`), each two points a side from
//!   `GameSplineDatabase::QueryHexSegment`: the noised shore the coast saved there
//!   (`AddAllCoastlinesToDatabase`), else the straight side `GenerateHexSide` makes, in the
//!   original's world.
//! - **Waves** ([`spawn_waves`]): `floor(WavesPerEdge)` passes over all the edges and random edges
//!   for the fraction, each wave with a size, a phase and a page of the crest atlas drawn from the
//!   engine's `FRandom` ([`FRandom`]), sorted along a Morton curve; 248 a batch.
//! - **Vertices** ([`WaveVertex`], 12 bytes): 9 samples of the spline, an outer and an inner vertex
//!   each, `CreateWave` 0x0893dc6a.
//! - **Update** ([`WaveField::update`]): the time of each wave runs on; past `CycleTime` it starts
//!   again after a random delay with a new page (`Update` 0x0893d21a).
//!
//! SIMPLIFIED(SIMP-042): the moment each wave appears and its page are the engine's draws of
//! `FRandom` from `RandomSeed`, but the edges the fraction falls on, the order of the edges and the
//! sort of the waves (the original sorts "probably by Morton code") are ours; which neighbouring
//! edges `GetSplineForEdge` chains comes from its byte tables 0x092159c0 / 0x09214548, which are not
//! read: ours follows the coast (the next edge round the land hex if it faces water, else the one
//! round the water hex, which is the original's choice by re-b16 W3); Gibraltar is not left out.

use glam::{Vec2, Vec3};

use asset_format::WaveSettings;

use crate::terrain::TerrainKind;
use crate::world::terrain::basis::TerrainBasis;
use crate::world::terrain::sampler::TerrainFrame;
use crate::world::water::river::hex::HexEdge;
use crate::world::water::river::spline_db::SplineDb;

/// Waves of one batch (`SetupWaveBatch`): the size of the array of constants of `VS_Wave`.
pub const BATCH: usize = 248;
/// Spline samples of a wave (t = 0, 0.125, ..., 1), two vertices each.
pub const SAMPLES: usize = 9;
pub const VERTICES_PER_WAVE: usize = 2 * SAMPLES;
pub const INDICES_PER_WAVE: usize = 6 * (SAMPLES - 1);

// re: cvWaveSystem::CreateWave 0x0893dc6a (rw-a6-answers RW-22 (a) 3)
const OUTER: f32 = 1.2;
const INNER: f32 = 0.1;
const HALF_EXTENT: f32 = 0.4;

/// The random numbers of the engine (`FRandom` 0x087fbf02): an LCG modulo `0xffffcfef`, the result
/// in `[0, 1)` is the state over `2^32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FRandom {
    state: u32,
}

impl FRandom {
    /// Seeded with `RandomSeed` (a float of the settings, taken as an integer).
    pub fn new(seed: f32) -> Self {
        Self {
            state: seed.max(0.0) as u32,
        }
    }

    /// The next raw state.
    // re: FRandom 0x087fbf02, Update 0x0893d21a: `x * 0x343fd + 0x269ec3 mod 0xffffcfef`
    pub fn next_raw(&mut self) -> u32 {
        self.state = ((u64::from(self.state) * 0x343fd + 0x269ec3) % 0xffff_cfef) as u32;
        self.state
    }

    /// The next number in `[0, 1)`.
    pub fn next_unit(&mut self) -> f32 {
        (f64::from(self.next_raw()) / 4_294_967_296.0) as f32
    }
}

fn is_water(kind: TerrainKind) -> bool {
    matches!(kind, TerrainKind::Coast | TerrainKind::Ocean)
}

/// Water at the cell `(col, row)` (unwrapped): off the map is neither land nor water for the
/// waves, `None`.
fn water_at(basis: &TerrainBasis, frame: &TerrainFrame, col: i32, row: i32) -> Option<bool> {
    let (x, y) = frame.grid_shape().normalize(col, row)?;
    basis.plot(x, y).map(|p| is_water(p.terrain))
}

/// The edge separates a land hex and a water hex.
fn is_coast(basis: &TerrainBasis, frame: &TerrainFrame, e: &HexEdge) -> bool {
    let (c, r) = e.cell();
    let t = e.twin();
    let (tc, tr) = t.cell();
    matches!(
        (water_at(basis, frame, c, r), water_at(basis, frame, tc, tr)),
        (Some(a), Some(b)) if a != b
    )
}

/// The edges of the coast: for every land hex, its sides that face water, in the form with the
/// land hex on the right (codes 0..5), in the order of the plots.
pub fn coast_edges(basis: &TerrainBasis, frame: &TerrainFrame) -> Vec<HexEdge> {
    let mut out = Vec::new();
    for row in 0..i32::from(basis.height) {
        for col in 0..i32::from(basis.width) {
            if water_at(basis, frame, col, row) != Some(false) {
                continue;
            }
            for side in 0..6u8 {
                let e = HexEdge::of_cell(col, row, side);
                if is_coast(basis, frame, &e) {
                    out.push(e);
                }
            }
        }
    }
    out
}

/// The edge after `e` along the coast: round the land hex if that side faces water, else round the
/// water hex (the side of the neighbouring land hex that faces the same water). The original always
/// takes one of the two, even at the border of the map (`GetSplineForEdge` tests only the water mask
/// of the land hex).
// re: cvWaveSystem::GetSplineForEdge 0x0893da3a (T1 = DAT_092159c0, T2 = DAT_09214548; re-b16 W3)
fn next_coast(basis: &TerrainBasis, frame: &TerrainFrame, e: &HexEdge) -> HexEdge {
    let round_land = e.next_right();
    if is_coast(basis, frame, &round_land) {
        round_land
    } else {
        e.next_left()
    }
}

/// The edge before `e` along the coast.
fn prev_coast(basis: &TerrainBasis, frame: &TerrainFrame, e: &HexEdge) -> HexEdge {
    let r = e.reversed();
    let round_land = r.next_left();
    if is_coast(basis, frame, &round_land) {
        round_land.reversed()
    } else {
        r.next_right().reversed()
    }
}

/// The spline database of the surf: the shore segments the coast saved
/// ([`crate::world::water::coast::lines::shore_segments`]), over the generated sides.
pub fn shore_db(frame: &TerrainFrame, shore: &[(HexEdge, [Vec2; 2])]) -> SplineDb {
    let mut db = SplineDb::new(*frame);
    for (edge, pts) in shore {
        db.submit(*edge, pts);
    }
    db
}

/// The control points of the spline of a coast edge in the original's world (x, y): the points of
/// the edge before, of the edge and of the edge after, as the database gives them (the noised shore
/// where the coast saved it, else the straight side).
// re: cvWaveSystem::GetSplineForEdge 0x0893da3a (QueryHexSegment x 3, TexToWorld; re-b16 W3)
pub fn edge_spline(
    basis: &TerrainBasis,
    frame: &TerrainFrame,
    db: &SplineDb,
    e: &HexEdge,
) -> Vec<Vec2> {
    let mut tex = Vec::with_capacity(6);
    tex.extend(db.query(prev_coast(basis, frame, e)));
    tex.extend(db.query(*e));
    tex.extend(db.query(next_coast(basis, frame, e)));
    tex.into_iter()
        .map(|t| frame.orig_point(t, 0.0).truncate())
        .collect()
}

/// The B-spline of the engine over `pts` at `(n - 2) * t`: the value and the normal `(gy, -gx)`.
// re: cvWaveSystem::EvaluateSpline 0x0893e3e2
pub fn eval_spline(pts: &[Vec2], t: f32) -> (Vec2, Vec2) {
    let s = (pts.len().saturating_sub(2)) as f32 * t;
    let p = crate::world::water::coast::bspline::value(pts, s);
    let g = crate::world::water::coast::bspline::gradient(pts, s);
    (p, Vec2::new(g.y, -g.x).normalize_or_zero())
}

/// The length of a spline, by its control polygon of 9 samples.
fn spline_length(pts: &[Vec2]) -> f32 {
    (0..SAMPLES - 1)
        .map(|i| {
            let a = eval_spline(pts, i as f32 / (SAMPLES - 1) as f32).0;
            let b = eval_spline(pts, (i + 1) as f32 / (SAMPLES - 1) as f32).0;
            (b - a).length()
        })
        .sum()
}

/// The vertex of the original (`VStruct<25>`, 12 bytes): x, y as halves; the normal as bytes
/// `(n + 1) / 2 * 255`; the side (0 outer, 255 inner); `t * 255`; the slot of the wave in its batch.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WaveVertex {
    pub pos: [u16; 2],
    /// `nx, ny, side, t`.
    pub tex: [u8; 4],
    /// The slot, then padding.
    pub slot: [u8; 4],
}

/// One wave: where it lies, its size, its time in the cycle and its page of the atlas.
#[derive(Debug, Clone, PartialEq)]
pub struct Wave {
    /// The control points of its spline.
    pub spline: Vec<Vec2>,
    pub size: f32,
    pub time: f32,
    pub page: u32,
}

/// The waves of a map and their vertices.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WaveField {
    pub waves: Vec<Wave>,
    pub vertices: Vec<WaveVertex>,
    /// The box of each batch in the original's world: min x, min y, max x, max y.
    pub batch_bounds: Vec<[f32; 4]>,
}

fn quantize_unit(v: f32) -> u8 {
    ((v + 1.0) * 0.5 * 255.0).round().clamp(0.0, 255.0) as u8
}

/// The `u, v` of the page `page` in the atlas.
pub fn page_uv(cfg: &WaveSettings, page: u32) -> [f32; 2] {
    let (pu, pv) = (cfg.crest_pages_u.max(1), cfg.crest_pages_v.max(1));
    [
        (page % pu) as f32 / pu as f32,
        (page / pu) as f32 / pv as f32,
    ]
}

/// The draws of `CreateWave` for one wave.
// re: cvWaveSystem::CreateWave 0x0893dc6a: (1) size, (2) phase, (3) page = raw % CrestPagesInUse
fn create_wave(cfg: &WaveSettings, rng: &mut FRandom, spline: Vec<Vec2>) -> Wave {
    let size = cfg.min_size + (cfg.max_size - cfg.min_size) * rng.next_unit();
    let time = cfg.cycle_time * rng.next_unit();
    let page = rng.next_raw() % cfg.crest_pages_in_use.max(1);
    Wave {
        spline,
        size,
        time,
        page,
    }
}

/// Interleaves the bits of two 16-bit numbers.
fn morton(x: u32, y: u32) -> u64 {
    let spread = |v: u32| {
        let mut v = u64::from(v & 0xffff);
        v = (v | (v << 8)) & 0x00ff_00ff;
        v = (v | (v << 4)) & 0x0f0f_0f0f;
        v = (v | (v << 2)) & 0x3333_3333;
        (v | (v << 1)) & 0x5555_5555
    };
    spread(x) | (spread(y) << 1)
}

/// The waves of the coast of a map (`SpawnWaves`, once a game) and their vertices. `shore` are the
/// segments the coast saved (`BuildProducts::coast_segments`; empty: the straight sides).
pub fn spawn_waves(
    basis: &TerrainBasis,
    frame: &TerrainFrame,
    cfg: &WaveSettings,
    shore: &[(HexEdge, [Vec2; 2])],
) -> WaveField {
    let edges = coast_edges(basis, frame);
    if edges.is_empty() || cfg.waves_per_edge <= 0.0 {
        return WaveField::default();
    }
    let db = shore_db(frame, shore);
    let splines: Vec<Vec<Vec2>> = edges
        .iter()
        .map(|e| edge_spline(basis, frame, &db, e))
        .collect();
    let mut rng = FRandom::new(cfg.random_seed);
    let total = (edges.len() as f32 * cfg.waves_per_edge).floor() as usize;
    let passes = cfg.waves_per_edge.floor() as usize;
    let mut waves = Vec::with_capacity(total);
    for _ in 0..passes {
        for s in &splines {
            waves.push(create_wave(cfg, &mut rng, s.clone()));
        }
    }
    while waves.len() < total {
        let i = ((rng.next_unit() * splines.len() as f32) as usize).min(splines.len() - 1);
        waves.push(create_wave(cfg, &mut rng, splines[i].clone()));
    }
    // Along a Morton curve of the middle of the spline, so a batch is a patch of the coast.
    let half = frame.half_world();
    waves.sort_by_key(|w| {
        let (m, _) = eval_spline(&w.spline, 0.5);
        let q = |v: f32, h: f32| ((v + h).max(0.0) / 16.0) as u32;
        morton(q(m.x, half.x), q(m.y, half.y))
    });
    let mut field = WaveField {
        waves,
        ..WaveField::default()
    };
    field.build_vertices(cfg);
    field
}

/// `worldScale = 2 * max(world width / grid width, world height / grid height)` with the whole world
/// (`vWorldSize`, not its half): about two hexes across. `StartDistance` and `EndDistance` are in its
/// units. With the half world the crests started at half their distance and drifted half as far.
// re: cvWaveSystem::SpawnWaves 0x0893c67c (rw-a6-answers RW-22 (a) 6); cvTerrainSampler.cpp:69-70
// (`field_c0/c4 = vWorldSize`, TerrainSystem.cpp:692-699; re-b16 W2)
pub fn world_scale(frame: &TerrainFrame) -> f32 {
    let world = frame.half_world() * 2.0;
    2.0 * (world.x / frame.width_hexes().max(1) as f32)
        .max(world.y / frame.height_hexes().max(1) as f32)
}

impl WaveField {
    /// The 18 vertices of every wave and the boxes of the batches.
    fn build_vertices(&mut self, cfg: &WaveSettings) {
        let aspect = cfg.crest_pages_v.max(1) as f32 / cfg.crest_pages_u.max(1) as f32;
        self.vertices = Vec::with_capacity(self.waves.len() * VERTICES_PER_WAVE);
        self.batch_bounds.clear();
        for (index, w) in self.waves.iter().enumerate() {
            if index % BATCH == 0 {
                self.batch_bounds
                    .push([f32::MAX, f32::MAX, f32::MIN, f32::MIN]);
            }
            let half = w.size * spline_length(&w.spline) * aspect * HALF_EXTENT;
            let slot = (index % BATCH) as u8;
            let b = self.batch_bounds.last_mut().expect("pushed above");
            for k in 0..SAMPLES {
                let t = k as f32 / (SAMPLES - 1) as f32;
                let (p, n) = eval_spline(&w.spline, t);
                let tb = (t * 255.0).round() as u8;
                for (q, side) in [(p + n * half * OUTER, 0u8), (p - n * half * INNER, 255)] {
                    b[0] = b[0].min(q.x);
                    b[1] = b[1].min(q.y);
                    b[2] = b[2].max(q.x);
                    b[3] = b[3].max(q.y);
                    self.vertices.push(WaveVertex {
                        pos: [
                            asset_format::f32_to_f16_bits(q.x),
                            asset_format::f32_to_f16_bits(q.y),
                        ],
                        tex: [quantize_unit(n.x), quantize_unit(n.y), side, tb],
                        slot: [slot, 0, 0, 0],
                    });
                }
            }
        }
    }

    /// The indices of one batch: 8 quads a wave, `(v, v+2, v+1, v+2, v+3, v+1)`.
    pub fn batch_indices() -> Vec<u16> {
        let mut out = Vec::with_capacity(BATCH * INDICES_PER_WAVE);
        for w in 0..BATCH {
            let base = (w * VERTICES_PER_WAVE) as u16;
            for q in 0..(SAMPLES - 1) as u16 {
                let v = base + 2 * q;
                out.extend([v, v + 2, v + 1, v + 2, v + 3, v + 1]);
            }
        }
        out
    }

    /// The time of the waves runs `dt` on. A wave past `CycleTime` waits a random delay (its time
    /// goes below zero) and takes a new page. The random numbers start again from `RandomSeed` at
    /// every call, as the original's.
    // re: cvWaveSystem::Update 0x0893d21a (rw-a6b-d15 1)
    pub fn update(&mut self, cfg: &WaveSettings, dt: f32) {
        let mut rng = FRandom::new(cfg.random_seed);
        for w in &mut self.waves {
            w.time += dt;
            if w.time > cfg.cycle_time {
                w.time = -(cfg.restart_delay_min
                    + rng.next_unit() * (cfg.restart_delay_max - cfg.restart_delay_min));
                w.page = rng.next_raw() % cfg.crest_pages_in_use.max(1);
            }
        }
    }

    /// `{u, v, time, 0}` of each wave of the batch `b` (`WaveData` of `VS_Wave`).
    pub fn batch_data(&self, cfg: &WaveSettings, b: usize) -> Vec<[f32; 4]> {
        let start = b * BATCH;
        let end = (start + BATCH).min(self.waves.len());
        let mut out: Vec<[f32; 4]> = self.waves[start.min(end)..end]
            .iter()
            .map(|w| {
                let [u, v] = page_uv(cfg, w.page);
                [u, v, w.time, 0.0]
            })
            .collect();
        out.resize(BATCH, [0.0; 4]);
        out
    }

    pub fn batches(&self) -> usize {
        self.waves.len().div_ceil(BATCH)
    }

    /// The waves of the batch `b`.
    pub fn batch_len(&self, b: usize) -> usize {
        (self.waves.len() - (b * BATCH).min(self.waves.len())).min(BATCH)
    }
}

/// `pow(v / 255, 2.2)`: the colours of the settings as the engine stores them.
// re: cvWaveSystem::ParseSettingsFromXML 0x0893bd36 (Color_R/G/B)
pub fn linear(rgb: [u8; 3]) -> Vec3 {
    let c = |v: u8| (f32::from(v) / 255.0).powf(2.2);
    Vec3::new(c(rgb[0]), c(rgb[1]), c(rgb[2]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::terrain::basis::PlotBasis;

    /// The game's `wavesettings.xml` (in the FPK).
    pub(crate) fn settings() -> WaveSettings {
        WaveSettings {
            waves_per_edge: 1.13,
            random_seed: 0.0,
            crest_pages_u: 8,
            crest_pages_v: 2,
            crest_pages_in_use: 16,
            min_size: 0.6,
            max_size: 1.4,
            start_distance: 0.08,
            end_distance: 0.02,
            cycle_time: 9.0,
            fade_in_start: 0.05,
            fade_in_end: 4.0,
            fade_out_start: 6.5,
            fade_out_end: 8.7,
            restart_delay_min: 2.1,
            restart_delay_max: 5.0,
            color: [64, 64, 64],
            ..WaveSettings::default()
        }
    }

    /// An island of one land hex in the middle of a 5 x 5 ocean.
    pub(crate) fn island() -> (TerrainBasis, TerrainFrame) {
        let water = PlotBasis {
            terrain: TerrainKind::Ocean,
            ..PlotBasis::default()
        };
        let mut b = TerrainBasis::uniform(5, 5, false, 1, water);
        let i = b.index(2, 2).unwrap();
        b.plots[i].terrain = TerrainKind::Grass;
        (b, TerrainFrame::new(5, 5, false))
    }

    #[test]
    fn the_random_numbers_are_the_lcg_of_the_engine() {
        let mut r = FRandom::new(0.0);
        assert_eq!(r.next_raw(), 0x269ec3);
        let x = 0x269ec3u64;
        assert_eq!(
            u64::from(r.next_raw()),
            (x * 0x343fd + 0x269ec3) % 0xffff_cfef
        );
        let mut r = FRandom::new(0.0);
        for _ in 0..1000 {
            let v = r.next_unit();
            assert!((0.0..1.0).contains(&v));
        }
    }

    #[test]
    fn an_island_has_six_coast_edges_round_it() {
        let (b, f) = island();
        let e = coast_edges(&b, &f);
        assert_eq!(e.len(), 6);
        let db = SplineDb::new(f);
        for edge in &e {
            // The edges before and after go round the island: six points a spline.
            assert_eq!(edge_spline(&b, &f, &db, edge).len(), 6);
        }
    }

    /// re-b16 W3: where the coast saved the noised shore under a side of the land hex, the surf's
    /// spline takes those points instead of the straight side; the edges before and after are asked
    /// for the same way.
    #[test]
    fn the_surf_follows_the_shore_the_coast_saved() {
        let (b, f) = island();
        let e = coast_edges(&b, &f)[0];
        let straight = edge_spline(&b, &f, &SplineDb::new(f), &e);
        let [p, q] = e.side_points(&f);
        let bent = [p + Vec2::new(3.0, -2.0), q + Vec2::new(-1.0, 4.0)];
        let db = shore_db(&f, &[(e, bent)]);
        let saved = edge_spline(&b, &f, &db, &e);
        assert_eq!(saved.len(), 6);
        // The own segment is points 2 and 3: the saved ones, in the original's world.
        for (k, t) in bent.iter().enumerate() {
            let want = f.orig_point(*t, 0.0).truncate();
            assert!((saved[2 + k] - want).length() < 1e-3, "{k}");
            assert!((straight[2 + k] - want).length() > 1.0, "{k}");
        }
        // The neighbours are the straight sides still.
        assert!((saved[0] - straight[0]).length() < 1e-3);
        assert!((saved[5] - straight[5]).length() < 1e-3);
    }

    /// WW-7 (`waves`): the count is `floor(edges * WavesPerEdge)`, about 1.13 a coast edge,
    /// and every wave has 18 vertices of 12 bytes.
    #[test]
    fn the_waves_are_wavesperedge_a_coast_edge() {
        let (b, f) = island();
        let field = spawn_waves(&b, &f, &settings(), &[]);
        assert_eq!(field.waves.len(), (6.0f32 * 1.13).floor() as usize);
        assert_eq!(std::mem::size_of::<WaveVertex>(), 12);
        assert_eq!(field.vertices.len(), field.waves.len() * VERTICES_PER_WAVE);
        assert_eq!(field.batches(), 1);
        for w in &field.waves {
            assert!((0.6..=1.4).contains(&w.size));
            assert!((0.0..9.0).contains(&w.time));
            assert!(w.page < 16);
        }
        // The outer vertex of a pair lies farther out than the inner one: the side bytes alternate.
        assert!(
            field
                .vertices
                .chunks(2)
                .all(|p| p[0].tex[2] == 0 && p[1].tex[2] == 255)
        );
        let idx = WaveField::batch_indices();
        assert_eq!(idx.len(), BATCH * 48);
        assert_eq!(&idx[..6], &[0, 2, 1, 2, 3, 1]);
    }

    #[test]
    fn a_wave_past_its_cycle_waits_and_takes_a_new_page() {
        let (b, f) = island();
        let cfg = settings();
        let mut field = spawn_waves(&b, &f, &cfg, &[]);
        field.waves[0].time = 8.95;
        field.update(&cfg, 0.1);
        let t = field.waves[0].time;
        assert!((-5.0..=-2.1).contains(&t), "{t}");
        let data = field.batch_data(&cfg, 0);
        assert_eq!(data.len(), BATCH);
        assert_eq!(data[0][2], t);
        assert_eq!(page_uv(&cfg, 9), [1.0 / 8.0, 0.5]);
    }

    /// re-b16 W2: the scale is made of the whole world, two hexes across; the crest starts
    /// `StartDistance` (0.08) of it off the shore and drifts to `EndDistance` (0.02): about 7.7 units
    /// over the cycle, not the 3.8 of the half world.
    #[test]
    fn the_world_scale_is_about_two_hexes() {
        let f = TerrainFrame::new(40, 24, true);
        let s = world_scale(&f);
        assert!((s - 2.0 * crate::hexgrid::HEX_WIDTH).abs() < 2.0, "{s}");
        let cfg = settings();
        let drift = (cfg.start_distance - cfg.end_distance) * s;
        assert!((7.0..8.5).contains(&drift), "{drift}");
    }
}
