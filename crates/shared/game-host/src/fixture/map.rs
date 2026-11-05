//! Map of the test world for `FixtureCore` (game-host-mp 4.14): Duel size 40x24 (with and without
//! `wrap_x`) and the synthetic Huge 128x80 (`WORLDSIZE_HUGE`, 10 240 plots).
//!
//! The map has every plot kind the renderer and the basis need: rivers with `river_flow`, ice at the poles,
//! marsh and flood plains, natural wonders (they sit in the map basis of the whole map, fog or not, NP-D19),
//! a few resources and improvements, barbarian camps and ruins. It is deterministic: the same size, wrap,
//! `terrain_seed` and start sites give the same map.
//!
//! This is the *truth* of the map. What a team sees of it (fog) is `fog.rs`; the plot record of a team is
//! [`MapData::team_record`].

use protocol::{PlayerId, PlotBasisView, PlotFlags, PlotType, PlotView};
use serde::{Deserialize, Serialize};

use crate::core::MapInfo;

/// The barbarian player.
pub const BARBARIAN: PlayerId = 63;

/// Duel map size (as the B1 fixture).
pub const DUEL: (u16, u16) = (40, 24);
/// Huge map size (`WORLDSIZE_HUGE` of Lekmod): 128 x 80 = 10 240 plots.
pub const HUGE: (u16, u16) = (128, 80);

pub const TERRAINS: [&str; 7] = [
    "TERRAIN_GRASS",
    "TERRAIN_PLAINS",
    "TERRAIN_DESERT",
    "TERRAIN_TUNDRA",
    "TERRAIN_SNOW",
    "TERRAIN_COAST",
    "TERRAIN_OCEAN",
];
pub const T_GRASS: i16 = 0;
pub const T_PLAINS: i16 = 1;
pub const T_DESERT: i16 = 2;
pub const T_TUNDRA: i16 = 3;
pub const T_SNOW: i16 = 4;
pub const T_COAST: i16 = 5;
pub const T_OCEAN: i16 = 6;

pub const FEATURES: [&str; 9] = [
    "FEATURE_ICE",
    "FEATURE_JUNGLE",
    "FEATURE_MARSH",
    "FEATURE_OASIS",
    "FEATURE_FLOOD_PLAINS",
    "FEATURE_FOREST",
    "FEATURE_FUJI",
    "FEATURE_CRATER",
    "FEATURE_MESA",
];
pub const F_ICE: i16 = 0;
pub const F_JUNGLE: i16 = 1;
pub const F_MARSH: i16 = 2;
pub const F_OASIS: i16 = 3;
pub const F_FLOOD_PLAINS: i16 = 4;
pub const F_FOREST: i16 = 5;
/// The natural wonders are `F_FUJI..=F_MESA`.
pub const F_FUJI: i16 = 6;
pub const F_MESA: i16 = 8;

pub const RESOURCES: [&str; 4] = [
    "RESOURCE_IRON",
    "RESOURCE_HORSE",
    "RESOURCE_WHEAT",
    "RESOURCE_FISH",
];
pub const IMPROVEMENTS: [&str; 5] = [
    "IMPROVEMENT_FARM",
    "IMPROVEMENT_MINE",
    "IMPROVEMENT_PASTURE",
    "IMPROVEMENT_BARBARIAN_CAMP",
    "IMPROVEMENT_GOODY_HUT",
];
pub const I_CAMP: i16 = 3;
pub const I_RUINS: i16 = 4;
pub const ROUTES: [&str; 1] = ["ROUTE_ROAD"];

/// True for a relief feature: the ones the map basis carries (ice, marsh, flood plains, natural wonders).
pub fn is_relief_feature(feature: i16) -> bool {
    matches!(feature, F_ICE | F_MARSH | F_FLOOD_PLAINS) || (F_FUJI..=F_MESA).contains(&feature)
}

// ---------------------------------------------------------------- geometry

/// Hex directions, `DirectionType` of the core: 0 NE, 1 E, 2 SE, 3 SW, 4 W, 5 NW.
pub const DIR_E: u8 = 1;
pub const DIR_SE: u8 = 2;
pub const DIR_SW: u8 = 3;

/// Size and wrapping of the map; plot arithmetic (odd rows are shifted half a hex east, y grows north).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Geo {
    pub w: i16,
    pub h: i16,
    pub wrap_x: bool,
}

impl Geo {
    pub fn new(w: u16, h: u16, wrap_x: bool) -> Self {
        Geo {
            w: w as i16,
            h: h as i16,
            wrap_x,
        }
    }

    pub fn plot_count(&self) -> usize {
        self.w as usize * self.h as usize
    }

    pub fn contains(&self, (x, y): (i16, i16)) -> bool {
        (0..self.w).contains(&x) && (0..self.h).contains(&y)
    }

    /// Plot index `y * width + x`.
    pub fn index(&self, (x, y): (i16, i16)) -> u32 {
        y as u32 * self.w as u32 + x as u32
    }

    pub fn xy(&self, index: u32) -> (i16, i16) {
        let w = self.w as u32;
        ((index % w) as i16, (index / w) as i16)
    }

    /// The neighbour of a plot in direction `dir` (0..6), `None` past the map edge.
    pub fn neighbor(&self, (x, y): (i16, i16), dir: u8) -> Option<(i16, i16)> {
        let odd = y & 1 == 1;
        let (dx, dy): (i16, i16) = match (dir, odd) {
            (0, false) => (0, 1),
            (0, true) => (1, 1),
            (1, _) => (1, 0),
            (2, false) => (0, -1),
            (2, true) => (1, -1),
            (3, false) => (-1, -1),
            (3, true) => (0, -1),
            (4, _) => (-1, 0),
            (5, false) => (-1, 1),
            (5, true) => (0, 1),
            _ => return None,
        };
        let (mut nx, ny) = (x + dx, y + dy);
        if ny < 0 || ny >= self.h {
            return None;
        }
        if self.wrap_x {
            nx = nx.rem_euclid(self.w);
        } else if nx < 0 || nx >= self.w {
            return None;
        }
        Some((nx, ny))
    }

    /// Distance in plots; over the seam on a wrapping map.
    pub fn distance(&self, a: (i16, i16), b: (i16, i16)) -> i16 {
        let axial = |(x, y): (i16, i16)| (x - (y - (y & 1)) / 2, y);
        let dist = |a: (i16, i16), b: (i16, i16)| {
            let ((aq, ar), (bq, br)) = (axial(a), axial(b));
            let (dq, dr) = (aq - bq, ar - br);
            (dq.abs() + dr.abs() + (dq + dr).abs()) / 2
        };
        if !self.wrap_x {
            return dist(a, b);
        }
        // Width is even, so a shift by the width keeps the row parity.
        [-self.w, 0, self.w]
            .into_iter()
            .map(|s| dist(a, (b.0 + s, b.1)))
            .min()
            .unwrap_or(0)
    }

    /// The plot and every plot within `radius` of it (breadth first, so seams and edges are right).
    pub fn within(&self, centre: (i16, i16), radius: u8) -> Vec<(i16, i16)> {
        let mut seen = vec![centre];
        let mut frontier = vec![centre];
        for _ in 0..radius {
            let mut next = Vec::new();
            for p in &frontier {
                for dir in 0..6 {
                    if let Some(n) = self.neighbor(*p, dir)
                        && !seen.contains(&n)
                    {
                        seen.push(n);
                        next.push(n);
                    }
                }
            }
            frontier = next;
        }
        seen
    }
}

/// Deterministic hash of a plot and a salt.
pub fn mix(x: i32, y: i32, salt: u32) -> u32 {
    let mut v = (x as u32).wrapping_mul(0x9E37_79B1)
        ^ (y as u32).wrapping_mul(0x85EB_CA6B)
        ^ salt.wrapping_mul(0xC2B2_AE35);
    v ^= v >> 15;
    v = v.wrapping_mul(0x2C1B_3C6D);
    v ^= v >> 12;
    v = v.wrapping_mul(0x297A_2D39);
    v ^ (v >> 15)
}

/// Start sites of `n` major players: two rows (south 25 %, north 75 % of the height) with the sites spread
/// along X; site `i` of the list alternates south and north so that the first two players are far apart.
/// Every site is at least 5 plots from every other, so no one sees a neighbour at the start (n <= 10).
pub fn start_sites(geo: &Geo, n: usize) -> Vec<(i16, i16)> {
    let (w, h) = (i32::from(geo.w), i32::from(geo.h));
    let south = n.div_ceil(2);
    let north = n / 2;
    let row = |count: usize, y: i32, shift: i32| -> Vec<(i16, i16)> {
        (0..count)
            .map(|i| {
                let x = ((2 * i as i32 + 1) * w / (2 * count as i32) + shift).rem_euclid(w);
                (x as i16, y as i16)
            })
            .collect()
    };
    let s = row(south, h / 4, 0);
    let nrow = row(
        north,
        h * 3 / 4,
        if north > 0 { w / (4 * north as i32) } else { 0 },
    );
    let mut out = Vec::with_capacity(n);
    for (i, site) in s.iter().enumerate() {
        out.push(*site);
        if let Some(site) = nrow.get(i) {
            out.push(*site);
        }
    }
    out
}

/// Positions of the three natural wonders, the two barbarian camps and the ruins: all in the middle band
/// of the map, at least 4 plots from every start site row, so they are under fog at the start.
pub fn specials(geo: &Geo) -> Specials {
    let (w, h) = (geo.w, geo.h);
    Specials {
        wonders: [
            (F_FUJI, (w / 8, h / 2)),
            (F_FUJI + 1, (w * 3 / 8 + 1, h / 2 + 1)),
            (F_MESA, (w * 5 / 8 + 1, h / 2 - 1)),
        ],
        camps: [(w / 4, h / 2), (w * 3 / 4, h / 2)],
        ruins: (w / 2, h / 2),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Specials {
    pub wonders: [(i16, (i16, i16)); 3],
    pub camps: [(i16, i16); 2],
    pub ruins: (i16, i16),
}

// ---------------------------------------------------------------- map data

/// Truth of the map: a record per plot as a team that has the plot in sight sees it (without the
/// `REVEALED`/`VISIBLE` flags, which belong to the fog), and the river flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapData {
    geo: Geo,
    terrain_seed: u32,
    plots: Vec<PlotView>,
    /// `FlowDirectionTypes` on the edges E, SE, SW of every plot, `-1` none.
    flow: Vec<[i8; 3]>,
}

impl MapData {
    /// Builds the map. The plots within 3 of a start site are open, dry land.
    pub fn generate(geo: Geo, terrain_seed: u32, starts: &[(i16, i16)]) -> Self {
        Self::generate_with(geo, terrain_seed, starts, None)
            .expect("a map without a gallery has no failure")
    }

    /// [`MapData::generate`] and, with a `gallery`, the plots of the full set laid over it (see
    /// [`MapData::lay_gallery`]). Returns where the units of the set stand with the map.
    ///
    /// # Errors
    /// The map has no room for the set.
    pub fn generate_gallery(
        geo: Geo,
        terrain_seed: u32,
        starts: &[(i16, i16)],
        set: &GallerySet<'_>,
    ) -> Result<(Self, GalleryLayout), String> {
        let mut layout = None;
        let map = Self::generate_with(geo, terrain_seed, starts, Some((set, &mut layout)))?;
        Ok((map, layout.expect("the gallery was laid")))
    }

    fn generate_with(
        geo: Geo,
        terrain_seed: u32,
        starts: &[(i16, i16)],
        gallery: Option<(&GallerySet<'_>, &mut Option<GalleryLayout>)>,
    ) -> Result<Self, String> {
        let n = geo.plot_count();
        let mut map = MapData {
            geo,
            terrain_seed,
            plots: vec![PlotView::default(); n],
            flow: vec![[-1; 3]; n],
        };
        let sp = specials(&geo);
        for y in 0..geo.h {
            for x in 0..geo.w {
                let p = map.terrain_plot((x, y), starts);
                map.plots[geo.index((x, y)) as usize] = p;
            }
        }
        map.place_specials(&sp);
        map.rivers();
        map.relief_from_rivers();
        if let Some((set, out)) = gallery {
            *out = Some(map.lay_gallery(set, starts)?);
        }
        map.coast_and_yields();
        Ok(map)
    }

    pub fn geo(&self) -> Geo {
        self.geo
    }

    pub fn info(&self) -> MapInfo {
        MapInfo {
            width: self.geo.w as u16,
            height: self.geo.h as u16,
            wrap_x: self.geo.wrap_x,
            wrap_y: false,
            terrain_seed: self.terrain_seed,
        }
    }

    pub fn plot(&self, index: u32) -> Option<&PlotView> {
        self.plots.get(index as usize)
    }

    pub fn plot_mut(&mut self, index: u32) -> Option<&mut PlotView> {
        self.plots.get_mut(index as usize)
    }

    /// The map basis record of a plot (derived from the truth, so a change of the relief changes both).
    pub fn basis_of(&self, index: u32) -> Option<PlotBasisView> {
        let p = self.plots.get(index as usize)?;
        let flow = self.flow.get(index as usize).copied().unwrap_or([-1; 3]);
        let keep = PlotFlags::IS_LAKE | PlotFlags::IS_COASTAL | PlotFlags::NATURAL_WONDER;
        Some(PlotBasisView {
            plot_type: p.plot_type,
            terrain: p.terrain,
            continent_art: p.continent_art,
            feature: if is_relief_feature(p.feature) {
                p.feature
            } else {
                -1
            },
            flags: PlotFlags(p.flags.0 & keep),
            rivers: p.rivers,
            river_flow: flow,
        })
    }

    /// The plot as a team sees it: `visible` in sight now, else only revealed (what is on the plot now is
    /// not known: no `WORKED`, no pillaged route). An unrevealed plot is `PlotView::default()` and is not
    /// built by this function.
    pub fn team_record(&self, index: u32, visible: bool) -> Option<PlotView> {
        let mut p = self.plots.get(index as usize)?.clone();
        p.flags = PlotFlags(p.flags.0 | PlotFlags::REVEALED);
        if visible {
            p.flags = PlotFlags(p.flags.0 | PlotFlags::VISIBLE);
        } else {
            p.flags = PlotFlags(p.flags.0 & !(PlotFlags::WORKED | PlotFlags::ROUTE_PILLAGED));
        }
        Some(p)
    }

    /// A city stands on the plot: a road under it.
    pub fn place_city(&mut self, at: (i16, i16)) {
        if let Some(p) = self.plots.get_mut(self.geo.index(at) as usize) {
            p.flags = PlotFlags(p.flags.0 | PlotFlags::HAS_CITY);
            p.route = 0;
            p.improvement = -1;
            p.feature = -1;
            p.resource = -1;
        }
    }

    /// Borders: the land plots within `radius` of `centre` that have no owner get `owner`. Returns the
    /// indexes that changed.
    pub fn claim(&mut self, centre: (i16, i16), radius: u8, owner: PlayerId) -> Vec<u32> {
        let mut changed = Vec::new();
        for at in self.geo.within(centre, radius) {
            let i = self.geo.index(at);
            if let Some(p) = self.plots.get_mut(i as usize)
                && p.plot_type != PlotType::Water
                && p.owner < 0
            {
                p.owner = owner;
                changed.push(i);
            }
        }
        changed
    }

    // ---- generation

    fn edge_distance(&self, (x, y): (i16, i16)) -> i16 {
        let d = y.min(self.geo.h - 1 - y);
        if self.geo.wrap_x {
            d
        } else {
            d.min(x).min(self.geo.w - 1 - x)
        }
    }

    fn terrain_plot(&self, at: (i16, i16), starts: &[(i16, i16)]) -> PlotView {
        let geo = self.geo;
        let (x, y) = at;
        let d = self.edge_distance(at);
        let near = |r: i16| starts.iter().any(|s| geo.distance(*s, at) <= r);
        let salt = self.terrain_seed;
        let h = (mix(i32::from(x), i32::from(y), salt) % 23) as i16;
        let lake =
            d > 2 && !near(5) && mix(i32::from(x), i32::from(y), salt ^ 0x51).is_multiple_of(41);
        let water = !near(3) && (d <= 1 || lake);

        let mut p = PlotView {
            terrain: T_GRASS,
            feature: -1,
            resource: -1,
            improvement: -1,
            route: -1,
            owner: -1,
            continent_art: (i32::from(x) * 4 / i32::from(geo.w)) as u8,
            building: -1,
            ..PlotView::default()
        };
        let mut flags = 0;
        if water {
            p.plot_type = PlotType::Water;
            p.terrain = if d == 0 { T_OCEAN } else { T_COAST };
            if d > 1 {
                flags |= PlotFlags::IS_LAKE;
            } else if d == 0 && (y == 0 || y == geo.h - 1) {
                p.feature = F_ICE;
            } else if x % 5 == 0 && y % 2 == 0 && d == 1 {
                p.resource = 3; // fish
                p.resource_qty = 2;
            }
            p.flags = PlotFlags(flags);
            return p;
        }

        let lat = i32::from(y) * 100 / i32::from(geo.h);
        p.terrain = if near(3) {
            (x + y) % 2
        } else if !(14..86).contains(&lat) {
            T_SNOW - (x % 2)
        } else if lat < 32 {
            if (x / 4 + y) % 3 == 0 {
                T_PLAINS
            } else {
                T_DESERT
            }
        } else if lat <= 70 {
            (x / 3 + y) % 2
        } else {
            x % 2
        };
        if !near(2) {
            p.plot_type = match h {
                0 | 1 => PlotType::Mountain,
                2..=4 => PlotType::Hill,
                _ => PlotType::Land,
            };
        }
        let open = p.plot_type == PlotType::Land && !near(2);
        if open {
            match (h, p.terrain) {
                (5..=7, T_GRASS | T_PLAINS) => p.feature = F_FOREST,
                (8, T_GRASS) => p.feature = F_JUNGLE,
                (9, T_GRASS) => p.feature = F_MARSH,
                (10, T_DESERT) => p.feature = F_OASIS,
                _ => {}
            }
            match h {
                13 => p.resource = 1,
                14 => p.resource = 2,
                _ => {}
            }
            if p.resource >= 0 {
                p.resource_qty = 2;
            }
            if h == 15 && p.feature < 0 {
                p.improvement = 0;
            }
        } else if p.plot_type == PlotType::Hill && !near(2) {
            if h == 2 {
                p.resource = 0;
                p.resource_qty = 3;
                p.improvement = 1;
            }
            if h == 3 {
                p.improvement = 2;
            }
        }
        p.flags = PlotFlags(flags);
        p
    }

    fn place_specials(&mut self, sp: &Specials) {
        for (feature, at) in sp.wonders {
            if let Some(p) = self.plots.get_mut(self.geo.index(at) as usize) {
                *p = PlotView {
                    plot_type: PlotType::Land,
                    terrain: T_PLAINS,
                    feature,
                    resource: -1,
                    improvement: -1,
                    route: -1,
                    owner: -1,
                    continent_art: p.continent_art,
                    building: -1,
                    flags: PlotFlags(PlotFlags::NATURAL_WONDER),
                    ..PlotView::default()
                };
            }
        }
        for at in sp.camps.into_iter().chain([sp.ruins]) {
            if let Some(p) = self.plots.get_mut(self.geo.index(at) as usize) {
                p.plot_type = PlotType::Land;
                p.feature = -1;
                p.resource = -1;
                p.resource_qty = 0;
                p.flags = PlotFlags(p.flags.0 & !PlotFlags::IS_LAKE);
                p.improvement = if at == sp.ruins { I_RUINS } else { I_CAMP };
            }
        }
    }

    fn is_dry(&self, at: (i16, i16)) -> bool {
        self.plots
            .get(self.geo.index(at) as usize)
            .is_some_and(|p| {
                p.plot_type != PlotType::Water && p.flags.0 & PlotFlags::NATURAL_WONDER == 0
            })
    }

    /// Sets the river on the edge `dir` (E, SE or SW) of `at` and on the matching edge of the neighbour.
    fn river_edge(&mut self, at: (i16, i16), dir: u8, flow: i8) {
        let Some(nb) = self.geo.neighbor(at, dir) else {
            return;
        };
        if !self.is_dry(at) || !self.is_dry(nb) {
            return;
        }
        // The edge as the neighbour sees it: the opposite direction.
        let opposite = (dir + 3) % 6;
        let (i, j) = (self.geo.index(at) as usize, self.geo.index(nb) as usize);
        self.plots[i].rivers |= 1 << dir;
        self.plots[j].rivers |= 1 << opposite;
        self.flow[i][usize::from(dir - DIR_E)] = flow;
    }

    fn rivers(&mut self) {
        let geo = self.geo;
        let salt = self.terrain_seed;
        for y in 0..geo.h {
            for x in 0..geo.w {
                let at = (x, y);
                let lat = i32::from(y) * 100 / i32::from(geo.h);
                // Runs: a zig-zag of SE + SW edges flowing south, along a column (and along the seam).
                let run = (20..=80).contains(&lat)
                    && (x == geo.w / 4 || x == geo.w * 3 / 4 || (geo.wrap_x && x == geo.w - 1));
                if run {
                    self.river_edge(at, DIR_SE, 4);
                    self.river_edge(at, DIR_SW, 2);
                    continue;
                }
                // Single edges, scattered.
                let m = mix(i32::from(x), i32::from(y), salt ^ 0x77);
                if m.is_multiple_of(13) {
                    let dir = DIR_E + (m / 13 % 3) as u8;
                    let flow = match dir {
                        DIR_E => [0, 3][(m / 39 % 2) as usize],
                        DIR_SE => [1, 4][(m / 39 % 2) as usize],
                        _ => [2, 5][(m / 39 % 2) as usize],
                    };
                    self.river_edge(at, dir, flow);
                }
            }
        }
    }

    /// Flood plains on desert next to a river.
    fn relief_from_rivers(&mut self) {
        for p in &mut self.plots {
            if p.rivers != 0
                && p.terrain == T_DESERT
                && p.plot_type == PlotType::Land
                && p.feature < 0
                && p.improvement != I_CAMP
                && p.improvement != I_RUINS
            {
                p.feature = F_FLOOD_PLAINS;
            }
        }
    }

    fn coast_and_yields(&mut self) {
        let geo = self.geo;
        let water: Vec<bool> = self
            .plots
            .iter()
            .map(|p| p.plot_type == PlotType::Water)
            .collect();
        for y in 0..geo.h {
            for x in 0..geo.w {
                let i = geo.index((x, y)) as usize;
                let p = &mut self.plots[i];
                if p.plot_type == PlotType::Water {
                    p.yields = [1, 0, i8::from(p.terrain == T_COAST), 0, 0, 0];
                    continue;
                }
                let coastal = (0..6).any(|dir| {
                    geo.neighbor((x, y), dir)
                        .is_some_and(|n| water[geo.index(n) as usize])
                });
                if coastal {
                    p.flags = PlotFlags(p.flags.0 | PlotFlags::IS_COASTAL);
                }
                let food = [2, 1, 0, 0, 0][p.terrain.clamp(0, 4) as usize];
                let prod = match p.plot_type {
                    PlotType::Hill => 2,
                    PlotType::Mountain => 0,
                    _ => i8::from(p.feature == F_FOREST),
                };
                p.yields = [food, prod, i8::from(p.route >= 0), 0, 0, 0];
            }
        }
    }
}

// ---------------------------------------------------------------- the full set (card N-R2)
//
// A test world with one unit of every type of `units.csv` and one landmark for every row of
// `landmarks.csv` (world-objects 9, assets-b2 4.15 item 9), so that the client shows every model of the
// asset set. The rows are laid on the Huge map, one per plot, two plots apart. A development tool.

/// The environment variable that turns the full set on: a directory with `units.csv` and `landmarks.csv`.
pub const GALLERY_ENV: &str = "CIV5_FIXTURE_GALLERY";

/// Catalog id of the Industrial era in the full set (`ERA_ANCIENT` 0, `ERA_RENAISSANCE` 1).
pub const ERA_INDUSTRIAL: i16 = 2;

const IMPROVEMENT_NONE: &str = "ART_DEF_IMPROVEMENT_NONE";
const RESOURCE_ALL: &str = "ART_DEF_RESOURCE_ALL";
const RESOURCE_NONE: &str = "ART_DEF_RESOURCE_NONE";
const FEATURE_PREFIX: &str = "ART_DEF_FEATURE_";
/// A large deposit shows from this quantity (the client's `HEAVY_RESOURCE_QTY`).
const HEAVY_QTY: i8 = 6;

/// `BuildTypes` in the order the client knows them (`render::anim::BUILD_NAMES`): `PlotView.building` is an
/// index of this list, and the client finds the improvement of a build as `IMPROVEMENT_<name>`.
const BUILDS: [&str; 25] = [
    "BUILD_ROAD",
    "BUILD_RAILROAD",
    "BUILD_FARM",
    "BUILD_MINE",
    "BUILD_TRADING_POST",
    "BUILD_LUMBERMILL",
    "BUILD_PASTURE",
    "BUILD_CAMP",
    "BUILD_PLANTATION",
    "BUILD_QUARRY",
    "BUILD_WELL",
    "BUILD_OFFSHORE_PLATFORM",
    "BUILD_FISHING_BOATS",
    "BUILD_FORT",
    "BUILD_REMOVE_JUNGLE",
    "BUILD_REMOVE_FOREST",
    "BUILD_REMOVE_MARSH",
    "BUILD_SCRUB_FALLOUT",
    "BUILD_REPAIR",
    "BUILD_REMOVE_ROUTE",
    "BUILD_LANDMARK",
    "BUILD_ACADEMY",
    "BUILD_CUSTOMS_HOUSE",
    "BUILD_MANUFACTORY",
    "BUILD_CITADEL",
];

/// A row of `landmarks.csv`, the columns the choice of a landmark reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandmarkRow {
    pub improvement: String,
    pub resource: String,
    pub era: String,
    pub state: String,
    /// Empty: no technology.
    pub tech: String,
}

/// What the asset set has: the unit types of `units.csv` and the rows of `landmarks.csv`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Gallery {
    pub units: Vec<String>,
    pub landmarks: Vec<LandmarkRow>,
}

/// The columns `cols` of the rows of a CSV table (no quoting in the asset reports), by header name.
fn csv_rows<'a>(text: &'a str, what: &str, cols: &[&str]) -> Result<Vec<Vec<&'a str>>, String> {
    let mut lines = text.lines();
    let header: Vec<&str> = lines.next().unwrap_or("").split(',').collect();
    let at: Vec<usize> = cols
        .iter()
        .map(|c| {
            header
                .iter()
                .position(|h| h == c)
                .ok_or_else(|| format!("{what}: no column {c}"))
        })
        .collect::<Result<_, _>>()?;
    let mut rows = Vec::new();
    for (n, line) in lines.enumerate().filter(|(_, l)| !l.is_empty()) {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() != header.len() {
            return Err(format!("{what}: line {} has {} fields", n + 2, f.len()));
        }
        rows.push(at.iter().map(|i| f[*i]).collect());
    }
    Ok(rows)
}

impl Gallery {
    /// Reads `units.csv` and `landmarks.csv` of an asset report directory.
    ///
    /// # Errors
    /// A file is missing or is not the table it should be.
    pub fn load(dir: &std::path::Path) -> Result<Self, String> {
        let read = |name: &str| {
            std::fs::read_to_string(dir.join(name))
                .map_err(|e| format!("{}: {e}", dir.join(name).display()))
        };
        Self::parse(&read("units.csv")?, &read("landmarks.csv")?)
    }

    /// The tables as text. A unit type is one row of kind `unit` (its `member` rows are skins of it).
    ///
    /// # Errors
    /// A column is missing, a line is short, or a table has no rows.
    pub fn parse(units: &str, landmarks: &str) -> Result<Self, String> {
        let mut types: Vec<String> = Vec::new();
        for r in csv_rows(units, "units.csv", &["kind", "id"])? {
            if r[0] == "unit" && !types.iter().any(|t| t == r[1]) {
                types.push(r[1].to_owned());
            }
        }
        let landmarks: Vec<LandmarkRow> = csv_rows(
            landmarks,
            "landmarks.csv",
            &["improvement", "resource", "era", "state", "tech"],
        )?
        .into_iter()
        .map(|r| LandmarkRow {
            improvement: r[0].to_owned(),
            resource: r[1].to_owned(),
            era: r[2].to_owned(),
            state: r[3].to_owned(),
            tech: r[4].to_owned(),
        })
        .collect();
        if types.is_empty() || landmarks.is_empty() {
            return Err("units.csv or landmarks.csv has no rows".into());
        }
        Ok(Gallery {
            units: types,
            landmarks,
        })
    }
}

/// The type name of an art define (`ART_DEF_RESOURCE_IRON` is `RESOURCE_IRON`).
pub fn type_of(art_define: &str) -> String {
    art_define
        .strip_prefix("ART_DEF_")
        .unwrap_or(art_define)
        .to_owned()
}

/// A resource art name cut into the resource and the variant the plot must show: `IRON_HEAVY_MARSH` is
/// the heavy iron on a marsh (the client's `resource_names`).
fn split_resource(art: &str) -> (&str, bool, Option<&'static str>) {
    let mut name = art;
    let mut spot = None;
    for suffix in ["_MARSH", "_HILL", "_WATER"] {
        if let Some(rest) = name.strip_suffix(suffix) {
            name = rest;
            spot = Some(suffix);
            break;
        }
    }
    let heavy = match name.strip_suffix("_HEAVY") {
        Some(rest) => {
            name = rest;
            true
        }
        None => false,
    };
    (name, heavy, spot)
}

/// The catalog lists of the full set. An id is the position in the list; the first entries are those of
/// the plain test world, so its rules keep their ids. Improvements, resources and features hold art
/// defines (their type name is [`type_of`] of it).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Names {
    pub units: Vec<String>,
    pub improvements: Vec<String>,
    pub resources: Vec<String>,
    pub features: Vec<String>,
    /// Type names; entry 0 is the plain world's `TECH_AGRICULTURE`.
    pub techs: Vec<String>,
}

fn push_new(list: &mut Vec<String>, name: &str) {
    if !list.iter().any(|n| n == name) {
        list.push(name.to_owned());
    }
}

impl Names {
    /// The plain world's lists (`base_units` are its unit types) with what the set adds.
    pub fn new(set: &Gallery, base_units: &[&str]) -> Self {
        let art = |t: &&str| format!("ART_DEF_{t}");
        let mut n = Names {
            units: base_units.iter().map(|t| (*t).to_owned()).collect(),
            improvements: IMPROVEMENTS.iter().map(art).collect(),
            resources: RESOURCES.iter().map(art).collect(),
            features: FEATURES.iter().map(art).collect(),
            techs: vec!["TECH_AGRICULTURE".into()],
        };
        for u in &set.units {
            push_new(&mut n.units, u);
        }
        for r in &set.landmarks {
            if r.improvement != IMPROVEMENT_NONE {
                push_new(&mut n.improvements, &r.improvement);
            }
            if r.resource.starts_with(FEATURE_PREFIX) {
                push_new(&mut n.features, &r.resource);
            } else if r.resource != RESOURCE_ALL && r.resource != RESOURCE_NONE {
                push_new(&mut n.resources, split_resource(&r.resource).0);
            }
            if !r.tech.is_empty() {
                push_new(&mut n.techs, &r.tech);
            }
        }
        n
    }
}

fn id_in(list: &[String], key: &str) -> Option<i16> {
    list.iter().position(|n| n == key).map(|i| i as i16)
}

/// Who owns the plots of a landmark: the owner decides the era and the technologies it is drawn for.
#[derive(Debug, Clone, Copy)]
pub struct Owners {
    /// Ancient era, no technology.
    pub ancient: PlayerId,
    /// Industrial era, no technology.
    pub industrial: PlayerId,
    /// Ancient era with the technologies of the set.
    pub tech: PlayerId,
}

/// Everything `lay_gallery` needs.
pub struct GallerySet<'a> {
    pub gallery: &'a Gallery,
    pub names: &'a Names,
    pub owners: Owners,
}

/// What laying the set left to the caller.
#[derive(Debug, Clone, Default)]
pub struct GalleryLayout {
    /// Where the unit of each entry of `Names::units` stands.
    pub unit_cells: Vec<(i16, i16)>,
    /// Rows the client cannot be shown (a build under construction it does not know): their text.
    pub missed: Vec<String>,
}

impl MapData {
    /// Lays one plot for every landmark row and one for every unit type over the generated map: the
    /// plots two apart, in the middle band, away from the start sites and the special plots. Plots of a
    /// landmark are flat grass (a water, hill or marsh variant of a resource excepted) owned by the
    /// player whose era and technologies the row is drawn for.
    fn lay_gallery(
        &mut self,
        set: &GallerySet<'_>,
        starts: &[(i16, i16)],
    ) -> Result<GalleryLayout, String> {
        let geo = self.geo;
        let sp = specials(&geo);
        let keep_out: Vec<((i16, i16), i16)> = starts
            .iter()
            .map(|s| (*s, 4))
            .chain(sp.wonders.iter().map(|w| (w.1, 3)))
            .chain(sp.camps.iter().chain([&sp.ruins]).map(|c| (*c, 3)))
            .collect();
        let mut cells = Vec::new();
        for y in (6..geo.h - 6).step_by(2) {
            for x in (4..geo.w - 4).step_by(2) {
                if keep_out.iter().all(|(c, r)| geo.distance(*c, (x, y)) > *r) {
                    cells.push((x, y));
                }
            }
        }
        let rows = &set.gallery.landmarks;
        let need = rows.len() + set.names.units.len();
        if cells.len() < need {
            return Err(format!(
                "fixture: the full set needs {need} plots, the map has {}",
                cells.len()
            ));
        }
        let mut layout = GalleryLayout::default();
        for (row, at) in rows.iter().zip(&cells) {
            self.clear_rivers(*at);
            let i = geo.index(*at) as usize;
            match self.landmark_plot(set, row, self.plots[i].continent_art) {
                Some(p) => self.plots[i] = p,
                None => {
                    self.plots[i] = Self::open_plot(self.plots[i].continent_art);
                    layout.missed.push(format!(
                        "{} {} {} {}",
                        row.improvement, row.resource, row.era, row.state
                    ));
                }
            }
        }
        for at in &cells[rows.len()..need] {
            self.clear_rivers(*at);
            let i = geo.index(*at) as usize;
            self.plots[i] = Self::open_plot(self.plots[i].continent_art);
            layout.unit_cells.push(*at);
        }
        Ok(layout)
    }

    /// Flat dry grassland with nothing on it.
    fn open_plot(continent_art: u8) -> PlotView {
        PlotView {
            terrain: T_GRASS,
            feature: -1,
            resource: -1,
            improvement: -1,
            route: -1,
            owner: -1,
            continent_art,
            building: -1,
            ..PlotView::default()
        }
    }

    /// The plot on which the client chooses `row`, the way the client's own completeness test builds it:
    /// the improvement (finished, pillaged or in construction), the resource with its variant, the
    /// feature. `None` when the row names something the lists lack or a build the client does not know.
    fn landmark_plot(
        &self,
        set: &GallerySet<'_>,
        row: &LandmarkRow,
        continent_art: u8,
    ) -> Option<PlotView> {
        let names = set.names;
        let mut p = Self::open_plot(continent_art);
        let mut water = matches!(
            row.improvement.as_str(),
            "ART_DEF_IMPROVEMENT_FISHING_BOATS" | "ART_DEF_IMPROVEMENT_OFFSHORE_PLATFORM"
        );
        let mut flags = 0;
        if row.resource.starts_with(FEATURE_PREFIX) {
            p.feature = id_in(&names.features, &row.resource)?;
            water |= row.resource.ends_with("_ATOLL") || row.resource.ends_with("_REEF");
        } else if row.resource != RESOURCE_ALL && row.resource != RESOURCE_NONE {
            let (base, heavy, spot) = split_resource(&row.resource);
            p.resource = id_in(&names.resources, base)?;
            p.resource_qty = if heavy { HEAVY_QTY } else { 2 };
            match spot {
                Some("_MARSH") => p.feature = F_MARSH,
                Some("_HILL") => p.plot_type = PlotType::Hill,
                Some(_) => water = true,
                None => {}
            }
            water |= ["FISH", "WHALE", "CRAB", "CORAL", "PEARLS"]
                .iter()
                .any(|r| base.ends_with(&format!("RESOURCE_{r}")));
        }
        if row.improvement != IMPROVEMENT_NONE {
            let imp = id_in(&names.improvements, &row.improvement)?;
            match row.state.as_str() {
                "UnderConstruction" => {
                    let suffix = type_of(&row.improvement)
                        .strip_prefix("IMPROVEMENT_")?
                        .to_owned();
                    let build = BUILDS
                        .iter()
                        .position(|b| b.strip_prefix("BUILD_") == Some(suffix.as_str()))?;
                    p.building = build as i16;
                }
                "Pillaged" => {
                    p.improvement = imp;
                    flags |= PlotFlags::PILLAGED;
                }
                _ => p.improvement = imp,
            }
        }
        if water {
            p.plot_type = PlotType::Water;
            p.terrain = T_COAST;
            flags |= PlotFlags::IS_LAKE;
        }
        p.flags = PlotFlags(flags);
        let o = set.owners;
        p.owner = if !row.tech.is_empty() {
            o.tech
        } else if row.era == "Industrial" {
            o.industrial
        } else {
            o.ancient
        };
        Some(p)
    }

    /// Takes the rivers off a plot and off the edges its neighbours share with it.
    fn clear_rivers(&mut self, at: (i16, i16)) {
        let geo = self.geo;
        let i = geo.index(at) as usize;
        for dir in 0..6u8 {
            if self.plots[i].rivers & (1 << dir) == 0 {
                continue;
            }
            self.plots[i].rivers &= !(1 << dir);
            if (DIR_E..=DIR_SW).contains(&dir) {
                self.flow[i][usize::from(dir - DIR_E)] = -1;
            }
            if let Some(nb) = geo.neighbor(at, dir) {
                let j = geo.index(nb) as usize;
                let opposite = (dir + 3) % 6;
                self.plots[j].rivers &= !(1 << opposite);
                if (DIR_E..=DIR_SW).contains(&opposite) {
                    self.flow[j][usize::from(opposite - DIR_E)] = -1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(w: u16, h: u16, wrap: bool, n: usize) -> MapData {
        let geo = Geo::new(w, h, wrap);
        let starts = start_sites(&geo, n);
        MapData::generate(geo, 7, &starts)
    }

    #[test]
    fn sizes_and_wrap() {
        for (size, wrap) in [(DUEL, false), (DUEL, true), (HUGE, false), (HUGE, true)] {
            let m = make(size.0, size.1, wrap, 4);
            let info = m.info();
            assert_eq!(
                (info.width, info.height, info.wrap_x),
                (size.0, size.1, wrap)
            );
            assert_eq!(m.plots.len(), usize::from(size.0) * usize::from(size.1));
        }
        assert_eq!(usize::from(HUGE.0) * usize::from(HUGE.1), 10_240);
    }

    #[test]
    fn neighbours_are_symmetric() {
        for wrap in [false, true] {
            let geo = Geo::new(DUEL.0, DUEL.1, wrap);
            for y in 0..geo.h {
                for x in 0..geo.w {
                    for dir in 0..6u8 {
                        if let Some(n) = geo.neighbor((x, y), dir) {
                            assert_eq!(geo.neighbor(n, (dir + 3) % 6), Some((x, y)));
                            assert_eq!(geo.distance((x, y), n), 1);
                        }
                    }
                }
            }
        }
        // Over the seam.
        let geo = Geo::new(40, 24, true);
        assert_eq!(geo.distance((0, 10), (39, 10)), 1);
        assert_eq!(geo.within((0, 10), 2).len(), 19);
    }

    #[test]
    fn rivers_are_consistent_on_both_sides() {
        for wrap in [false, true] {
            let m = make(DUEL.0, DUEL.1, wrap, 4);
            let geo = m.geo();
            let mut edges = 0;
            for y in 0..geo.h {
                for x in 0..geo.w {
                    let p = m.plot(geo.index((x, y))).unwrap();
                    for dir in 0..6u8 {
                        if p.rivers & (1 << dir) == 0 {
                            continue;
                        }
                        edges += 1;
                        let n = geo
                            .neighbor((x, y), dir)
                            .expect("river edge inside the map");
                        let q = m.plot(geo.index(n)).unwrap();
                        assert_ne!(
                            q.rivers & (1 << ((dir + 3) % 6)),
                            0,
                            "edge {x},{y} dir {dir}"
                        );
                    }
                }
            }
            assert!(edges > 20, "wrap={wrap}: {edges} river edges");
        }
    }

    #[test]
    fn basis_has_relief_everywhere() {
        let m = make(DUEL.0, DUEL.1, false, 4);
        let n = m.geo().plot_count() as u32;
        let mut ice = 0;
        let mut wonders = 0;
        let mut flows = 0;
        let mut marsh = 0;
        let mut flood = 0;
        for i in 0..n {
            let b = m.basis_of(i).unwrap();
            let p = m.plot(i).unwrap();
            assert_eq!(b.rivers, p.rivers);
            assert_eq!(b.terrain, p.terrain);
            ice += i32::from(b.feature == F_ICE);
            marsh += i32::from(b.feature == F_MARSH);
            flood += i32::from(b.feature == F_FLOOD_PLAINS);
            wonders += i32::from(b.flags.contains(PlotFlags::NATURAL_WONDER));
            flows += b.river_flow.iter().filter(|f| **f >= 0).count();
            // Forest, jungle and oasis are not relief.
            assert!(b.feature < 0 || is_relief_feature(b.feature));
        }
        assert!(
            ice > 0 && marsh > 0 && flood > 0,
            "ice {ice} marsh {marsh} flood {flood}"
        );
        assert_eq!(wonders, 3);
        assert!(flows > 0);
    }

    #[test]
    fn start_regions_are_open_land() {
        for n in 1..=10 {
            for size in [DUEL, HUGE] {
                let geo = Geo::new(size.0, size.1, false);
                let sites = start_sites(&geo, n);
                assert_eq!(sites.len(), n);
                let m = MapData::generate(geo, 1, &sites);
                for (i, s) in sites.iter().enumerate() {
                    for (j, t) in sites.iter().enumerate() {
                        if i != j {
                            assert!(geo.distance(*s, *t) >= 5, "{n} sites {s:?} {t:?}");
                        }
                    }
                    for at in geo.within(*s, 3) {
                        let p = m.plot(geo.index(at)).unwrap();
                        assert_ne!(p.plot_type, PlotType::Water, "site {s:?} at {at:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn specials_are_far_from_the_starts() {
        for size in [DUEL, HUGE] {
            let geo = Geo::new(size.0, size.1, false);
            let sp = specials(&geo);
            for n in 1..=10 {
                for s in start_sites(&geo, n) {
                    for (_, w) in sp.wonders {
                        assert!(geo.distance(s, w) >= 4, "wonder {w:?} start {s:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn generation_is_deterministic() {
        let a = make(DUEL.0, DUEL.1, true, 3);
        let b = make(DUEL.0, DUEL.1, true, 3);
        assert_eq!(a.plots, b.plots);
        assert_eq!(a.flow, b.flow);
    }
}
