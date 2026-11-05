//! Where the buildings of a city stand (world-objects 4.4, WOB5): the order of
//! `cvCityBuildingSim::Update` (0x087b6964) for one city, ported.
//!
//! 1. The wall (`UpdateCityWall`, [`super::wall_gen`]) from the density field of the city.
//! 2. The house lots (`GeneratePopBuildingPlacements` 0x087b73a4, [`super::layout_gen`]): the rect
//!    of 128 tex units round the city, the wall line and the wonder boxes in the blocked mask, the
//!    relaxation, then `StorePopBuildingPlacementsInCity` (0x087b923c: a lot with a corner on water
//!    is dropped).
//! 3. `ResetPopBuildingsFromPlacementsInCity` (0x087b7de4): the class of each lot from the density
//!    field of the hex-edge pattern and the population, the land and slope tests, then the burn
//!    order (`std::random_shuffle`, the ranks 3, 4, 6, 8).
//!
//! The whole layout is made again from scratch when the population or the era changes; only the
//! model picks and the burn order draw random numbers.
//!
//! The original works in its world plane: x east, y up the rows (ours `(X, -Z)`). The numbers of
//! the catalogs ([`CITY_SCALE`]) are those of the B2 city models.

use glam::Vec2;

use super::fields::{
    CLASS_NONE, DensityField, FIELD_SIDE, HEX_ROW_STRETCH, LayoutFields, Pattern, ROW_SCALE,
    TRANSLATIONS, TensorField, tex_to_field,
};
use super::layout_gen::{LayoutGen, MASK_WALL, MASK_WONDER, Rect, SOLVE_TICKS};
use super::msvc::MsvcRand;
use super::wall_gen::{self, WallPlan};
use crate::entities::CityKey;
use crate::world::landmark::select::mix;

/// Catalog units to world units: the 400-unit ground quad of a catalog is as wide as a hex (64).
pub const CITY_SCALE: f32 = 64.0 / 400.0;

/// The scale of a house of the catalog in the world: `m_uBuildingSize * 0.038` of the matrix of a city
/// building (`FUN_088587b8`: `D3DXMatrixScaling` by the building size times `DAT_092105c0`); the size is
/// 1.0 (the label `1.00` and `g_nBuildingSize = 100` of `DebugMode.lua`, the only sender of
/// `SerialEventBuildingSizeChanged`).
pub const BUILDING_SCALE: f32 = 0.038;

/// `CvGame::ms_aiSizes`: the populations at which the size of a city goes up.
const CITY_SIZES: [u32; 9] = [2, 3, 5, 7, 9, 11, 13, 15, 17];

/// The size of a city as the layout knows it: `CvGame::GetVariableCitySizeFromPopulation` (0..9),
/// which `CvDLLUtility::GameplayCityPopulationChanged` / `GameplayCityCreated` send in the place of
/// the population (`m_nPopulation` of the city data is this size). It picks the density field
/// (`_A1` .. `_A10`), the tensor field (above 6: the large city) and the class test of the wall.
pub fn city_size(population: i32) -> i32 {
    let p = population.max(0) as u32;
    CITY_SIZES
        .iter()
        .position(|&s| p < s)
        .unwrap_or(CITY_SIZES.len()) as i32
}

/// The ground quad of the city is this share of the hex wide.
pub const GROUND: f32 = 0.8;

/// `DAT_0920bf60`: the layout rect round the city, tex units.
pub const RECT_HALF: f32 = 64.0;

/// `kDamageRankThresholds`: the rank of a shuffled position is the number of these it reaches.
pub const DAMAGE_RANKS: [usize; 4] = [3, 4, 6, 8];

/// The rank of a building that never burns.
pub const RANK_NONE: u8 = 4;

/// The burnable buildings of a city (`kBurningBuildingCount`).
const BURNING: usize = 8;

/// `kLandHeightBias` and `kLandHeightRange` of the land band ([`Site::in_land_band`]).
const LAND_HEIGHT_BIAS: u16 = 0xcd37;
const LAND_HEIGHT_RANGE: u16 = 22_000;

/// `kMinNormalZ`.
pub const MIN_NORMAL_Z: f32 = 0.85;

/// The points of the footprint of a house that must be dry: its corners and the middles of its
/// sides, half a footprint of 5 units each way.
pub const FOOTPRINT_PROBES: [(f32, f32); 8] = [
    (5.0, -5.0),
    (5.0, 5.0),
    (-5.0, 5.0),
    (-5.0, -5.0),
    (5.0, 0.0),
    (0.0, 5.0),
    (-5.0, 0.0),
    (0.0, -5.0),
];

/// Our world point (X, Z) in the original's plane.
pub fn to_orig(p: Vec2) -> Vec2 {
    Vec2::new(p.x, -p.y)
}

/// A point of the original's plane in ours (X, Z).
pub fn from_orig(p: Vec2) -> Vec2 {
    Vec2::new(p.x, -p.y)
}

/// The seed of a city: a number from its key.
pub fn seed_of(key: CityKey) -> u32 {
    mix(u32::from(key.0 as u8) << 24 ^ key.1 as u32, 0xc17e)
}

/// The cube coordinates `(x, y)` of a plot (`ToHexFromGrid`: `x = col - (row >> 1)`).
pub fn hex_of(col: i32, row: i32) -> (i32, i32) {
    (col - (row >> 1), row)
}

/// The plot of cube coordinates.
pub fn plot_of(hx: i32, hy: i32) -> (i32, i32) {
    (hx + (hy >> 1), hy)
}

/// What the edge flags of the original's hex grid need to know of a plot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlotEdges {
    pub water: bool,
    pub mountain: bool,
    /// The river bits of the plot (bit i: `Dir` i).
    pub rivers: u8,
}

/// The `Dir` index of each of `TRANSLATIONS`.
const DIR_OF_TRANSLATION: [usize; 6] = [0, 4, 2, 3, 1, 5];

/// The edge byte of the hex `(hx, hy)` towards `TRANSLATIONS[k]` (k 0..2: the three edges a hex
/// holds): 1, `| 2` water on either side, `| 8` a mountain on either side (`OnSpecialTileAdded`,
/// types 0 and 2), `| 0x10` a river on the edge (`OnRiverTileAdded`).
// SIMPLIFIED(SIMP-021): the value of an edge without anything (1) is inferred from the test of
// `CalculateHexEdgeCoverage`; the grid's initialisation is not read.
fn edge_byte(hex: (i32, i32), k: usize, plot: &dyn Fn(i32, i32) -> Option<PlotEdges>) -> u8 {
    let t = TRANSLATIONS[k];
    let at = |h: (i32, i32)| {
        let (c, r) = plot_of(h.0, h.1);
        plot(c, r)
    };
    let a = at(hex);
    let b = at((hex.0 + t[0], hex.1 + t[1]));
    let mut v = 1u8;
    let side =
        |f: &dyn Fn(&PlotEdges) -> bool| a.as_ref().is_some_and(f) || b.as_ref().is_some_and(f);
    if side(&|p| p.water) {
        v |= 2;
    }
    if side(&|p| p.mountain) {
        v |= 8;
    }
    if a.is_some_and(|p| p.rivers >> DIR_OF_TRANSLATION[k] & 1 != 0) {
        v |= 0x10;
    }
    v
}

/// `CalculateHexEdgeCoverage`: bits 0..2 the edges the city hex holds whose byte has `0xA`, bits
/// 3..5 the edges its neighbours hold that are not plain.
pub fn edge_coverage(col: i32, row: i32, plot: &dyn Fn(i32, i32) -> Option<PlotEdges>) -> u8 {
    let hex = hex_of(col, row);
    let mut mask = 0u8;
    for k in 0..3 {
        if edge_byte(hex, k, plot) & 0xa != 0 {
            mask |= 1 << k;
        }
    }
    for (k, t) in TRANSLATIONS.iter().enumerate().skip(3) {
        let n = (hex.0 + t[0], hex.1 + t[1]);
        if edge_byte(n, k % 3, plot) != 1 {
            mask |= 1 << k;
        }
    }
    mask
}

/// The city and its ground, for the layout.
pub struct Site<'a> {
    /// The middle of the city hex in the original's plane.
    pub centre: Vec2,
    /// The size of the city ([`city_size`]), not its population.
    pub population: i32,
    /// The hex-edge mask ([`edge_coverage`]).
    pub edges: u8,
    pub has_wall: bool,
    /// The height of the ground at a point of the original's plane.
    pub ground: &'a dyn Fn(Vec2) -> f32,
    /// The level of the sea, `None` before the water is there.
    pub water_level: Option<f32>,
}

impl Site<'_> {
    /// The stencil bit 8 (water) at a point.
    pub fn is_water(&self, p: Vec2) -> bool {
        self.water_level.is_some_and(|w| (self.ground)(p) < w)
    }

    /// The land band of the houses: the raw height `h` of the terrain with `u16(h + 0xcd37) < 22000`,
    /// i.e. `13001..=34999` (above the sea at 13000, below the high ground).
    pub fn in_land_band(&self, p: Vec2) -> bool {
        let raw = crate::world::terrain::sampler::raw_from_height(
            (self.ground)(p),
            crate::world::terrain::sampler::TERRAIN_SCALE_Z,
        );
        raw.wrapping_add(LAND_HEIGHT_BIAS) < LAND_HEIGHT_RANGE
    }

    /// The up component of the terrain normal at a point.
    pub fn normal_z(&self, p: Vec2) -> f32 {
        let h = |d: Vec2| (self.ground)(p + d);
        let gx = (h(Vec2::X) - h(-Vec2::X)) * 0.5;
        let gy = (h(Vec2::Y) - h(-Vec2::Y)) * 0.5;
        1.0 / (1.0 + gx * gx + gy * gy).sqrt()
    }

    /// The layout rect: 64 tex units each way, the rows at 0.866.
    pub fn rect(&self) -> Rect {
        let half = Vec2::new(RECT_HALF, RECT_HALF * ROW_SCALE);
        Rect {
            min: self.centre - half,
            max: self.centre + half,
        }
    }
}

/// A wonder or a building with a model of its own: its box on the ground (four corners in the
/// original's plane), for the blocked mask.
#[derive(Debug, Clone, PartialEq)]
pub struct Footprint {
    pub corners: [Vec2; 4],
}

/// One house of the layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct House {
    /// The place in the original's plane.
    pub pos: Vec2,
    /// The heading vector (the tensor at the place).
    pub dir: Vec2,
    /// The height class 0..3.
    pub class: u8,
    /// The burn rank 0..3, [`RANK_NONE`] for a building that never burns.
    pub rank: u8,
}

/// The layout of a city.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CityLayout {
    pub wall: Option<WallPlan>,
    pub houses: Vec<House>,
    pub pattern: Option<Pattern>,
}

/// A point of the original's plane in the field space of a pattern.
fn field_point(site: &Site<'_>, p: Vec2, pattern: Pattern) -> Vec2 {
    let rel = p - site.centre;
    tex_to_field(
        Vec2::new(rel.x, rel.y * HEX_ROW_STRETCH),
        pattern.reflection,
    )
}

/// The layout of one city. `rand` is the C library state of the city (the model picks come after,
/// from the same state). The footprints are the wonders already placed.
pub fn generate(
    fields: &LayoutFields,
    site: &Site<'_>,
    footprints: &[Footprint],
    rand: &mut MsvcRand,
) -> CityLayout {
    let mut job = LayoutJob::begin(fields, site, footprints);
    job.advance(SOLVE_TICKS);
    job.finish(fields, site, rand)
}

/// A layout being solved a few ticks at a time (the relaxation of the lots is 40 ticks, the dear
/// part): [`generate`] split in three, `begin`, `advance` and `finish`, with the arithmetic of the
/// whole.
pub struct LayoutJob {
    pattern: Pattern,
    wall: Option<WallPlan>,
    solver: LayoutGen,
    tick: u32,
}

impl LayoutJob {
    /// The wall, and the solver with the wall and the wonders in its mask.
    pub fn begin(fields: &LayoutFields, site: &Site<'_>, footprints: &[Footprint]) -> Self {
        let pattern = fields.pattern_of(site.edges);
        let field = fields.density(pattern, site.population);
        let land = |p: Vec2| site.in_land_band(p) && !site.is_water(p);
        let normal = |p: Vec2| site.normal_z(p);
        // 1. The wall.
        let wall = site
            .has_wall
            .then(|| {
                let ground = wall_gen::Ground {
                    land: &land,
                    normal_z: &normal,
                };
                let samples = wall_gen::samples(
                    site.centre,
                    field,
                    pattern.reflection,
                    site.population,
                    &ground,
                );
                wall_gen::polygon(samples, site.centre)
            })
            .flatten()
            .map(wall_gen::plan);

        // 2. The lots.
        let mut solver = LayoutGen::new(site.rect(), fields.tensor(site.population));
        if let Some(w) = &wall {
            solver.rasterize_polyline(&wall_gen::mask_line(&w.polygon), MASK_WALL);
        }
        for f in footprints.iter().take(0x20) {
            solver.fill_polygon(&f.corners, MASK_WONDER);
        }
        Self {
            pattern,
            wall,
            solver,
            tick: 0,
        }
    }

    /// Does up to `ticks` ticks of the relaxation; returns how many it did.
    pub fn advance(&mut self, ticks: u32) -> u32 {
        let before = self.tick;
        self.tick = self.solver.solve_ticks(before, ticks);
        self.tick - before
    }

    /// True once the relaxation is done.
    pub fn is_done(&self) -> bool {
        self.tick >= SOLVE_TICKS
    }

    /// The layout, once the relaxation is done: the lots, their classes and the burn order.
    pub fn finish(
        mut self,
        fields: &LayoutFields,
        site: &Site<'_>,
        rand: &mut MsvcRand,
    ) -> CityLayout {
        assert!(self.tick >= SOLVE_TICKS, "the layout is not solved");
        let (pattern, wall) = (self.pattern, self.wall);
        let field = fields.density(pattern, site.population);
        let lots = self.solver.finalize();
        // Store: a lot with a corner on water is dropped.
        // SIMPLIFIED(SIMP-exe-1173): the original tests the four corners at 2 units (`0x0912c9e4`),
        // yet stamps a footprint of 5 (`UpdateBuildingBase`) and its houses are wider than the
        // test, so a house at the shore stood partly in the water here; the corners are taken at
        // the footprint of 5 and the middles of its sides as well.
        let lots = lots.into_iter().filter(|l| {
            FOOTPRINT_PROBES
                .iter()
                .all(|(dx, dy)| !site.is_water(l.pos + Vec2::new(*dx, *dy)))
        });
        // 3. Reset: the classes, the land and slope tests.
        let mut houses: Vec<House> = Vec::new();
        for l in lots {
            let Some(class) = field.class_at(field_point(site, l.pos, pattern)) else {
                continue;
            };
            let class = class & 7;
            if class == CLASS_NONE {
                continue;
            }
            if !site.in_land_band(l.pos) || site.normal_z(l.pos) < MIN_NORMAL_Z {
                continue;
            }
            houses.push(House {
                pos: l.pos,
                dir: l.dir,
                class: class.min(3),
                rank: RANK_NONE,
            });
        }
        // The burn order: the first eight of the shuffled list take the ranks.
        let mut order: Vec<usize> = (0..houses.len()).collect();
        rand.shuffle(&mut order);
        for (position, &i) in order.iter().take(BURNING).enumerate() {
            houses[i].rank = DAMAGE_RANKS.iter().filter(|&&t| position >= t).count() as u8;
        }
        CityLayout {
            wall,
            houses,
            pattern: Some(pattern),
        }
    }
}

/// Stand-in fields for a pack without the files of the layout system: rings of the four classes
/// that grow with the population, the houses along x.
// SIMPLIFIED(SIMP-021): only until the pack has the authored fields (`b2-city`, `tx:df_*`).
pub fn stand_in_fields() -> LayoutFields {
    let set: Vec<DensityField> = (0..10)
        .map(|p| {
            let reach = 14.0 + 3.0 * p as f32;
            let mut cells = Box::new([CLASS_NONE; FIELD_SIDE * FIELD_SIDE]);
            for y in 0..FIELD_SIDE {
                for x in 0..FIELD_SIDE {
                    let r = Vec2::new(x as f32 - 64.0, y as f32 - 64.0).length() / reach;
                    cells[y * FIELD_SIDE + x] = match r {
                        r if r < 0.4 => 3,
                        r if r < 0.6 => 2,
                        r if r < 0.8 => 1,
                        r if r < 1.0 => 0,
                        _ => CLASS_NONE,
                    };
                }
            }
            DensityField { cells }
        })
        .collect();
    LayoutFields {
        patterns: vec![Pattern {
            field_set: 0,
            edge_mask: 0,
            reflection: 0,
        }],
        sets: vec![set],
        tensors: [TensorField::uniform(100, 0), TensorField::uniform(100, 0)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site<'a>(ground: &'a dyn Fn(Vec2) -> f32, pop: i32, wall: bool) -> Site<'a> {
        Site {
            centre: Vec2::new(640.0, 300.0),
            population: pop,
            edges: 0,
            has_wall: wall,
            ground,
            water_level: Some(10.2),
        }
    }

    #[test]
    fn the_size_of_a_city_follows_the_steps_of_the_game() {
        let sizes: Vec<i32> = [0, 1, 2, 4, 5, 8, 12, 16, 17, 40].map(city_size).into();
        assert_eq!(sizes, [0, 0, 1, 2, 3, 4, 6, 8, 9, 9]);
    }

    #[test]
    fn a_city_grows_more_and_taller_houses() {
        let fields = stand_in_fields();
        let flat = |_: Vec2| 12.0;
        let count = |pop| {
            generate(
                &fields,
                &site(&flat, pop, false),
                &[],
                &mut MsvcRand::new(1),
            )
            .houses
            .len()
        };
        let (small, big) = (count(1), count(9));
        assert!(small > 5 && big > small * 2, "{small} {big}");
    }

    #[test]
    fn the_layout_is_the_same_again_and_only_the_burn_order_draws() {
        let fields = stand_in_fields();
        let flat = |_: Vec2| 12.0;
        let a = generate(&fields, &site(&flat, 5, true), &[], &mut MsvcRand::new(3));
        let b = generate(&fields, &site(&flat, 5, true), &[], &mut MsvcRand::new(99));
        let pos = |l: &CityLayout| l.houses.iter().map(|h| h.pos).collect::<Vec<_>>();
        assert_eq!(pos(&a), pos(&b));
        let burning = a.houses.iter().filter(|h| h.rank < RANK_NONE).count();
        assert_eq!(burning, 8.min(a.houses.len()));
        let ranks: Vec<usize> = (0..4)
            .map(|r| a.houses.iter().filter(|h| h.rank == r).count())
            .collect();
        assert_eq!(ranks, [3, 1, 2, 2]);
        // A wall round the houses.
        let w = a.wall.as_ref().unwrap();
        assert!(w.towers.len() >= 6);
    }

    #[test]
    fn a_layout_solved_a_few_ticks_at_a_time_is_the_whole_layout() {
        let fields = stand_in_fields();
        let flat = |_: Vec2| 12.0;
        let site = site(&flat, 6, true);
        let whole = generate(&fields, &site, &[], &mut MsvcRand::new(5));
        let mut job = LayoutJob::begin(&fields, &site, &[]);
        let mut frames = 0;
        while !job.is_done() {
            assert!(job.advance(3) <= 3);
            frames += 1;
        }
        assert_eq!(frames, 14);
        let parts = job.finish(&fields, &site, &mut MsvcRand::new(5));
        assert_eq!(parts, whole);
    }

    #[test]
    fn no_house_on_the_water() {
        let fields = stand_in_fields();
        // The sea east of the middle.
        let coast = |p: Vec2| if p.x > 650.0 { 5.0 } else { 12.0 };
        let l = generate(&fields, &site(&coast, 8, false), &[], &mut MsvcRand::new(1));
        assert!(!l.houses.is_empty());
        assert!(l.houses.iter().all(|h| h.pos.x < 648.0));
    }

    #[test]
    fn no_house_stands_partly_in_the_water_and_an_inland_city_keeps_its_houses() {
        let fields = stand_in_fields();
        let coast = |p: Vec2| if p.x > 650.0 { 5.0 } else { 12.0 };
        let l = generate(&fields, &site(&coast, 8, false), &[], &mut MsvcRand::new(1));
        let s = site(&coast, 8, false);
        assert!(!l.houses.is_empty());
        for h in &l.houses {
            for (dx, dy) in FOOTPRINT_PROBES {
                assert!(!s.is_water(h.pos + Vec2::new(dx, dy)), "{:?}", h.pos);
            }
        }
        // No sea in reach: the houses are those of the old rule (the corners at 2 units).
        let flat = |_: Vec2| 12.0;
        let inland = generate(&fields, &site(&flat, 8, false), &[], &mut MsvcRand::new(1));
        assert!(inland.houses.len() > 20);
    }

    #[test]
    fn the_edge_mask_sees_the_sea_and_the_rivers() {
        let land = |_: i32, _: i32| Some(PlotEdges::default());
        assert_eq!(edge_coverage(5, 5, &land), 0);
        // The plot east of (5, 5) is the sea: the edge towards it is held by the city (T4 = E is
        // a neighbour-held edge, bit 4).
        let sea_east = |c: i32, r: i32| {
            Some(PlotEdges {
                water: (c, r) == (6, 5),
                ..PlotEdges::default()
            })
        };
        assert_eq!(edge_coverage(5, 5, &sea_east), 1 << 4);
        // The sea north-east (T0, held by the city).
        let (c, r) = plot_of(hex_of(5, 5).0, hex_of(5, 5).1 + 1);
        let sea_ne = move |x: i32, y: i32| {
            Some(PlotEdges {
                water: (x, y) == (c, r),
                ..PlotEdges::default()
            })
        };
        assert_eq!(edge_coverage(5, 5, &sea_ne), 1);
    }
}
