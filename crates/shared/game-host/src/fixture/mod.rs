//! The test world: a small deterministic world for the `Worker` (`FixtureCore`, below) and for the client
//! tests, so they run without the real gamecore. A development tool, not a game simplification.
//!
//! `FixtureCore` is the game: `map.rs` the map, `seats.rs` the seats and the catalog, `rules.rs` the orders,
//! `fog.rs` the sight. The opening data of the Duel map (`build_map`, `build_units`, `build_cities`,
//! `build_players`, `catalog`) is what the client tests start from.

use protocol::{
    ArtRefs, Blocker, BlockerTarget, Catalog, CatalogEntry, CityFlags, CityView, MapView,
    PlayerFlags, PlayerId, PlayerView, PlotFlags, PlotType, PlotView, TopBar, TurnStatus,
    UnitFlags, UnitView,
};

pub const WIDTH: u16 = 40;
pub const HEIGHT: u16 = 24;
pub const HUMAN: i8 = 0;
pub const AI: i8 = 1;

pub const TERRAINS: [&str; 7] = [
    "TERRAIN_GRASS",
    "TERRAIN_PLAINS",
    "TERRAIN_DESERT",
    "TERRAIN_TUNDRA",
    "TERRAIN_SNOW",
    "TERRAIN_COAST",
    "TERRAIN_OCEAN",
];
pub const FEATURES: [&str; 6] = [
    "FEATURE_ICE",
    "FEATURE_JUNGLE",
    "FEATURE_MARSH",
    "FEATURE_OASIS",
    "FEATURE_FLOOD_PLAINS",
    "FEATURE_FOREST",
];
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
/// Index in [`IMPROVEMENTS`] of the barbarian encampment and of the ruins.
pub const CAMP: i16 = 3;
pub const RUINS: i16 = 4;
/// Where they are: the first two in sight of the human, the encampment on the left in the fog.
const CAMP_AT: [(i16, i16); 2] = [(11, 14), (18, 12)];
const RUINS_AT: (i16, i16) = (5, 10);
pub const ROUTES: [&str; 1] = ["ROUTE_ROAD"];
pub const UNITS: [&str; 8] = [
    "UNIT_SETTLER",
    "UNIT_WORKER",
    "UNIT_WARRIOR",
    "UNIT_ARCHER",
    "UNIT_HORSEMAN",
    "UNIT_SCOUT",
    "UNIT_WORK_BOAT",
    "UNIT_TRIREME",
];
pub const BUILDINGS: [&str; 3] = ["BUILDING_MONUMENT", "BUILDING_GRANARY", "BUILDING_WALLS"];
pub const CIVS: [&str; 2] = ["CIVILIZATION_ROME", "CIVILIZATION_EGYPT"];

const CAPITAL: (i16, i16) = (6, 12);
const SECOND_CITY: (i16, i16) = (9, 17);
const AI_CITY: (i16, i16) = (30, 10);

/// X below this is visible, below `UNREVEALED_FROM` is revealed fog, the rest is unrevealed.
pub const VISIBLE_UNTIL: i16 = 14;
const UNREVEALED_FROM: i16 = 32;

fn entries(types: &[&str], names: &[&str]) -> Vec<CatalogEntry> {
    types
        .iter()
        .enumerate()
        .map(|(i, t)| CatalogEntry {
            id: i as i16,
            type_name: (*t).into(),
            name: names.get(i).map_or_else(|| (*t).into(), |n| (*n).into()),
            art: ArtRefs {
                art_define: format!("ART_DEF_{t}"),
                icon_index: i as i16,
                ..ArtRefs::default()
            },
        })
        .collect()
}

pub fn catalog() -> Catalog {
    let mut civs = entries(&CIVS, &["Rome", "Egypt"]);
    // Two city styles side by side (the art table's `ArtStylePrefix` keys), for AC-4.5.
    for (civ, style) in civs.iter_mut().zip(["MEDITERRANEAN", "AFRICAN"]) {
        civ.art.art_style = style.into();
    }
    Catalog {
        units: entries(
            &UNITS,
            &[
                "Settler",
                "Worker",
                "Warrior",
                "Archer",
                "Horseman",
                "Scout",
                "Work Boat",
                "Trireme",
            ],
        ),
        buildings: entries(&BUILDINGS, &["Monument", "Granary", "Walls"]),
        terrains: entries(
            &TERRAINS,
            &[
                "Grassland",
                "Plains",
                "Desert",
                "Tundra",
                "Snow",
                "Coast",
                "Ocean",
            ],
        ),
        features: entries(
            &FEATURES,
            &["Ice", "Jungle", "Marsh", "Oasis", "Flood Plains", "Forest"],
        ),
        resources: entries(&RESOURCES, &["Iron", "Horses", "Wheat", "Fish"]),
        improvements: entries(
            &IMPROVEMENTS,
            &["Farm", "Mine", "Pasture", "Barbarian Camp", "Ancient Ruins"],
        ),
        routes: entries(&ROUTES, &["Road"]),
        civs,
        eras: entries(
            &["ERA_ANCIENT", "ERA_RENAISSANCE"],
            &["Ancient", "Renaissance"],
        ),
        ..Catalog::default()
    }
}

fn hash(x: i16, y: i16) -> i16 {
    (x * 31 + y * 17) % 23
}

fn is_fixed(x: i16, y: i16) -> bool {
    let near = |p: (i16, i16)| (x - p.0).abs() <= 2 && (y - p.1).abs() <= 2;
    near(CAPITAL) || near(SECOND_CITY) || near(AI_CITY)
}

/// Distance to the nearest map edge; on a wrapping map the left and right edges do not count.
fn edge_distance(x: i16, y: i16, wrap: bool) -> i16 {
    let d = y.min(HEIGHT as i16 - 1 - y);
    if wrap {
        d
    } else {
        d.min(x).min(WIDTH as i16 - 1 - x)
    }
}

fn is_water(x: i16, y: i16, wrap: bool) -> bool {
    let d = edge_distance(x, y, wrap);
    d <= 1 || (y == 12 && (20..=21).contains(&x))
}

fn build_plot(x: i16, y: i16, wrap: bool) -> PlotView {
    // On a wrapping map the unrevealed strip is narrower, so land and borders reach the seam.
    let unrevealed_to = if wrap {
        UNREVEALED_FROM + 3
    } else {
        WIDTH as i16
    };
    if (UNREVEALED_FROM..unrevealed_to).contains(&x) {
        return PlotView::default();
    }
    let d = edge_distance(x, y, wrap);
    let h = hash(x, y);
    let mut p = PlotView {
        terrain: 0,
        feature: -1,
        resource: -1,
        improvement: -1,
        route: -1,
        owner: -1,
        continent_art: (x / 10 % 4) as u8,
        ..PlotView::default()
    };
    let mut flags = PlotFlags::REVEALED;
    if x < VISIBLE_UNTIL {
        flags |= PlotFlags::VISIBLE;
    }

    if is_water(x, y, wrap) {
        p.plot_type = PlotType::Water;
        p.terrain = if d == 0 { 6 } else { 5 };
        if d > 1 {
            flags |= PlotFlags::IS_LAKE;
        } else if d == 0 && y >= HEIGHT as i16 - 1 {
            p.feature = 0; // ice
        } else if x % 5 == 0 && y % 2 == 0 && d == 1 {
            p.resource = 3; // fish
        }
        p.yields = [1, 0, if d == 0 { 0 } else { 1 }, 0, 0, 0];
    } else {
        p.terrain = match y {
            19.. => 4 - (x % 2), // snow / tundra
            15..=18 => x % 2,    // grass / plains
            8..=14 => (x / 3 + y) % 2,
            3..=7 => 2, // desert
            _ => 1,
        };
        if is_fixed(x, y) {
            p.terrain = 1;
        } else {
            p.plot_type = match h {
                0 | 1 => PlotType::Mountain,
                2..=4 => PlotType::Hill,
                _ => PlotType::Land,
            };
        }
        let open = p.plot_type == PlotType::Land && !is_fixed(x, y);
        if open {
            match (h, p.terrain) {
                (5..=7, 0 | 1) => p.feature = 5,
                (8, 0) => p.feature = 1,
                (9, 0) => p.feature = 2,
                (10, 2) => p.feature = 3,
                (11, 2) => p.feature = 4,
                _ => {}
            }
            match h {
                13 => p.resource = 1,
                14 => p.resource = 2,
                _ => {}
            }
            if h == 15 {
                p.improvement = 0;
            }
        } else if p.plot_type == PlotType::Hill && !is_fixed(x, y) {
            if h == 2 {
                p.resource = 0;
                p.improvement = 1;
            }
            if h == 3 {
                p.improvement = 2;
            }
        }
        if y == 12 && (7..=14).contains(&x) {
            p.route = 0;
        }
        // An encampment and ruins on open land.
        if CAMP_AT.contains(&(x, y)) || (x, y) == RUINS_AT {
            p.plot_type = PlotType::Land;
            p.feature = -1;
            p.resource = -1;
            p.improvement = if (x, y) == RUINS_AT { RUINS } else { CAMP };
        }
        // A road branch that bends: (13, 13), (13, 14), (13, 15) are neighbours in turn.
        if x == 13 && (13..=15).contains(&y) && p.plot_type != PlotType::Mountain {
            p.route = 0;
        }
        // River edges (bit i is direction i); a few vertical runs and scattered single edges.
        if (x == 10 && (6..=16).contains(&y)) || (x == 22 && (4..=9).contains(&y)) {
            p.rivers = (1 << 2) | (1 << 3);
        } else if wrap && (6..=16).contains(&y) && x == WIDTH as i16 - 1 {
            p.rivers = 1 << 1; // East edge, over the seam
        } else if wrap && (6..=16).contains(&y) && x == 0 {
            p.rivers = 1 << 4; // West edge, the same river from the other side
        } else if (x + y) % 11 == 0 {
            p.rivers = 1 << ((x * 5 + y) % 6);
        }
        let food = [2, 1, 0, 0, 0][p.terrain as usize];
        let prod = match p.plot_type {
            PlotType::Hill => 2,
            PlotType::Mountain => 0,
            _ => i8::from(p.feature == 5),
        };
        p.yields = [food, prod, i8::from(p.route >= 0), 0, 0, 0];
    }
    // Borders of the two players.
    if p.plot_type != PlotType::Water {
        if x <= 12 {
            p.owner = HUMAN;
        } else if x >= 27 {
            p.owner = AI;
        }
    }
    // Coastal: land touching water.
    if p.plot_type != PlotType::Water {
        let near_water = [(-1, 0), (1, 0), (0, -1), (0, 1)]
            .iter()
            .any(|(dx, dy)| is_water(x + dx, y + dy, wrap));
        if near_water {
            flags |= PlotFlags::IS_COASTAL;
        }
    }
    if [CAPITAL, SECOND_CITY, AI_CITY].contains(&(x, y)) {
        flags |= PlotFlags::HAS_CITY;
    }
    p.flags = PlotFlags(flags);
    p
}

pub fn build_map() -> MapView {
    build_map_with(false)
}

/// The test map; with `wrap_x` the left and right edges join (no edge water, owners and fog
/// continue over the seam, a river runs along it).
pub fn build_map_with(wrap_x: bool) -> MapView {
    let mut plots = Vec::with_capacity(WIDTH as usize * HEIGHT as usize);
    for y in 0..HEIGHT as i16 {
        for x in 0..WIDTH as i16 {
            plots.push(build_plot(x, y, wrap_x));
        }
    }
    MapView {
        width: WIDTH,
        height: HEIGHT,
        wrap_x,
        wrap_y: false,
        plots,
        terrain_seed: 0,
    }
}

#[allow(clippy::too_many_arguments)]
fn unit(
    id: i32,
    owner: i8,
    unit_type: i16,
    (x, y): (i16, i16),
    hp: i16,
    max_moves: i16,
    strength: i16,
    flags: u16,
    name: &str,
) -> UnitView {
    UnitView {
        id,
        owner,
        unit_type,
        x,
        y,
        hp,
        max_hp: 10,
        moves_left: max_moves,
        max_moves,
        strength,
        domain: if unit_type >= 6 { 1 } else { 0 },
        flags: UnitFlags(
            flags
                | if max_moves > 0 {
                    UnitFlags::CAN_MOVE
                } else {
                    0
                },
        ),
        name: name.into(),
        ..UnitView::default()
    }
}

pub fn build_units() -> Vec<UnitView> {
    let civ = UnitFlags::CIVILIAN;
    let mut warrior = unit(
        3,
        HUMAN,
        2,
        CAPITAL,
        10,
        120,
        8,
        UnitFlags::GARRISONED | UnitFlags::FORTIFIED,
        "Warrior",
    );
    warrior.level = 2;
    warrior.experience = 30;
    let mut archer = unit(
        4,
        HUMAN,
        3,
        (8, 11),
        6,
        120,
        5,
        UnitFlags::NEEDS_ORDERS,
        "Archer",
    );
    archer.ranged = 7;
    archer.range = 2;
    let mut ai_warrior = unit(9, AI, 2, (13, 9), 10, 120, 8, 0, "Warrior");
    ai_warrior.experience = 5;
    vec![
        unit(
            1,
            HUMAN,
            0,
            (7, 12),
            10,
            120,
            0,
            civ | UnitFlags::NEEDS_ORDERS,
            "Settler",
        ),
        unit(
            2,
            HUMAN,
            1,
            (5, 13),
            10,
            120,
            0,
            civ | UnitFlags::AUTOMATED,
            "Worker",
        ),
        warrior,
        archer,
        unit(5, HUMAN, 4, (10, 10), 10, 240, 12, 0, "Horseman"),
        unit(
            6,
            HUMAN,
            5,
            (12, 8),
            10,
            240,
            0,
            UnitFlags::SLEEPING,
            "Scout",
        ),
        unit(7, HUMAN, 7, (1, 10), 3, 180, 4, 0, "Trireme"),
        unit(8, HUMAN, 1, (9, 16), 10, 120, 0, civ, "Worker"),
        ai_warrior,
        unit(
            10,
            HUMAN,
            6,
            (1, 14),
            10,
            240,
            0,
            civ | UnitFlags::EMBARKED,
            "Work Boat",
        ),
    ]
}

fn city(
    id: i32,
    owner: i8,
    (x, y): (i16, i16),
    name: &str,
    flags: u16,
    population: i16,
) -> CityView {
    CityView {
        id,
        owner,
        x,
        y,
        name: name.into(),
        population,
        flags: CityFlags(flags),
        hp: 20,
        max_hp: 20,
        defense: 800,
        production_order: 0,
        production_item: 2,
        production_turns: 4,
        production_stored: 12,
        production_needed: 40,
        growth_turns: 5,
        buildings: Vec::new(),
        founder: owner,
        yields: if owner == HUMAN {
            [4, 3, 2, 1, 1, 0]
        } else {
            [0; 6]
        },
    }
}

pub fn build_cities() -> Vec<CityView> {
    vec![
        city(1, HUMAN, CAPITAL, "Roma", CityFlags::CAPITAL, 3),
        city(2, HUMAN, SECOND_CITY, "Ostia", 0, 1),
        city(3, AI, AI_CITY, "Thebes", CityFlags::CAPITAL, 2),
    ]
}

pub fn build_players() -> Vec<PlayerView> {
    vec![
        PlayerView {
            id: HUMAN,
            team: 0,
            civ_type: 0,
            leader_type: 0,
            flags: PlayerFlags(PlayerFlags::ALIVE | PlayerFlags::HUMAN),
            color_primary: 0xB0_20_20_FF,
            color_secondary: 0xF0_D0_40_FF,
            name: "Player".into(),
            civ_name: "Rome".into(),
            era: 0,
            art_techs: 0,
        },
        PlayerView {
            id: AI,
            team: 1,
            civ_type: 1,
            leader_type: 1,
            flags: PlayerFlags(
                PlayerFlags::ALIVE | PlayerFlags::MET | PlayerFlags::AT_WAR_WITH_VIEWER,
            ),
            color_primary: 0x20_60_C0_FF,
            color_secondary: 0xFF_FF_FF_FF,
            name: "Ramesses".into(),
            civ_name: "Egypt".into(),
            // A later era than the human's, so two city looks per culture show (AC-4.5).
            era: 1,
            art_techs: 0,
        },
    ]
}

// ---------------------------------------------------------------- FixtureCore (game-host-mp 4.14)
//
// The test world as a `Core` for the `Worker` (GH-D1). The opening data of the Duel map (`build_map`,
// `build_units`, `build_cities`, `build_players`, `catalog`) stays for the client tests. Map: `map.rs`; seats, catalog, players: `seats.rs`; fog: `fog.rs`. A development tool, not a
// game simplification.

use std::collections::{BTreeMap, BTreeSet};

use protocol::{
    CityBuildView, Codec, CombatPreview, Command, FixtureTest, GameSettings, HumanWar,
    LobbyChoices, NotificationView, PathNode, PlotBasisView, Postcard, ReachNode, Shooter,
    SlotControl, SlotSetup, TeamId, UnitActionView, ViewHeader,
};

use crate::core::{
    Core, CoreError, CoreErrorKind, CoreEvent, CorePump, CoreStart, CoreTurn, MapInfo, Refusal,
};

use serde::{Deserialize, Serialize};

use self::fog::Fog;
use self::map::{BARBARIAN, Geo, MapData};
use self::seats::{Diplomacy, Major};

pub mod fog;
pub mod heavy;
pub mod map;
pub mod rules;
pub mod seats;

/// Where the game is: the humans move, or the AI phase of two steps (`rules.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum GamePhase {
    Humans,
    Ai { stage: u8 },
}

/// A game of the test world. It is also the state of a FIXT save: what is skipped is rebuilt empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Game {
    settings: GameSettings,
    turn: i32,
    map: MapData,
    fog: Fog,
    majors: Vec<Major>,
    dip: Diplomacy,
    /// Units of all players, barbarians included.
    units: Vec<UnitView>,
    cities: Vec<CityView>,
    phase: GamePhase,
    /// Humans that have ended their turn.
    finished: BTreeSet<PlayerId>,
    /// Last unit id given to each player, so that ids of dead units are not used again.
    unit_seq: BTreeMap<PlayerId, i32>,
    /// Plots whose record changed for some team since the last `dirty_plots`.
    #[serde(skip)]
    dirty: BTreeSet<u32>,
    #[serde(skip)]
    events: Vec<CoreEvent>,
    /// Wars of a human on a human that `Core::turn` has not reported yet.
    #[serde(skip)]
    pending_wars: Vec<HumanWar>,
    /// The AI turn queue of the phase, drained by `Core::turn_status`.
    turn_status: Vec<protocol::AiTurnEvent>,
    #[serde(skip)]
    hooks: FixtureTest,
    /// `fixture_test.ai_step_delay_ms`: when the current AI step began waiting.
    #[serde(skip)]
    ai_wait: Option<std::time::Instant>,
    /// A human slot was given to the AI since the last pump: like the real core (`gc-abi-v2` 4.9) the game is
    /// not at a safe point until the AI has played its turn, so every slot change and save must pump first.
    #[serde(skip)]
    kick_pending: bool,
}

impl Game {
    fn geo(&self) -> Geo {
        self.map.geo()
    }

    /// Recounts fog from the units and cities of the majors; the changed plots become dirty and the teams
    /// meet whoever they now see.
    fn refresh_fog(&mut self) {
        let geo = self.geo();
        let mut sources: Vec<(TeamId, (i16, i16))> = Vec::new();
        for u in self.units.iter().filter(|u| u.owner != BARBARIAN) {
            sources.push((u.owner, (u.x, u.y)));
        }
        for c in self.cities.iter().filter(|c| c.owner != BARBARIAN) {
            sources.push((c.owner, (c.x, c.y)));
        }
        let changed = self.fog.update(&sources);
        self.dirty.extend(changed);
        let teams: Vec<TeamId> = self.fog.teams().to_vec();
        for team in teams {
            let seen_units = self
                .units
                .iter()
                .filter(|u| self.fog.is_visible(team, geo.index((u.x, u.y))))
                .map(|u| u.owner);
            let seen_cities = self
                .cities
                .iter()
                .filter(|c| self.fog.is_visible(team, geo.index((c.x, c.y))))
                .map(|c| c.owner);
            let seen: BTreeSet<PlayerId> = seen_units.chain(seen_cities).collect();
            for owner in seen {
                self.dip.meet(team, owner);
            }
        }
    }

    fn major(&self, player: PlayerId) -> Option<&Major> {
        self.majors.iter().find(|m| m.player == player)
    }

    /// Indexes to answer for: all plots, or the sorted, unique `subset`.
    fn indexes(&self, subset: Option<&[u32]>) -> Result<Vec<u32>, CoreError> {
        let n = self.geo().plot_count() as u32;
        match subset {
            None => Ok((0..n).collect()),
            Some(list) => {
                let mut v = list.to_vec();
                v.sort_unstable();
                v.dedup();
                match v.last() {
                    Some(last) if *last >= n => Err(CoreError::failed(format!(
                        "fixture: plot {last} is outside the map ({n} plots)"
                    ))),
                    _ => Ok(v),
                }
            }
        }
    }
}

/// The test world as a [`Core`].
#[derive(Debug, Clone)]
pub struct FixtureCore {
    huge: bool,
    wrap_x: bool,
    hooks: FixtureTest,
    game: Option<Game>,
    /// The full set (card N-R2): one unit of every type and one landmark of every row of the asset
    /// set, on the Huge map, under no fog.
    gallery: Option<map::Gallery>,
    /// Every `set_control` call the core got, in order (tests: what the worker handed over).
    controls_set: Vec<(PlayerId, SlotControl)>,
    /// A loaded game starts at an unsafe point until the first pump (the real core: a human in auto moves).
    unsafe_after_load: bool,
}

impl FixtureCore {
    /// A test world of the Duel map (40x24) or, with `huge`, the synthetic Huge map (128x80); `wrap_x`
    /// joins the left and right edges. The game begins with [`Core::start`].
    pub fn new(huge: bool, wrap_x: bool) -> Self {
        FixtureCore {
            huge,
            wrap_x,
            hooks: FixtureTest::default(),
            game: None,
            gallery: None,
            controls_set: Vec::new(),
            unsafe_after_load: false,
        }
    }

    /// A loaded game is not at a safe point until it has been pumped once (a human of the save is in auto moves).
    pub fn unsafe_after_load(mut self) -> Self {
        self.unsafe_after_load = true;
        self
    }

    /// The `set_control` calls so far (a connection of a human after a load is one of them).
    pub fn controls_set(&self) -> &[(PlayerId, SlotControl)] {
        &self.controls_set
    }

    /// The full set mode: the game has every unit type and every landmark row of `gallery` standing on the
    /// map (always the Huge one), and nobody has fog. Needs two majors to show all rows: the first is of
    /// the Industrial era, the second has the technologies of the set. Set before the start.
    pub fn with_gallery(mut self, gallery: map::Gallery) -> Self {
        self.gallery = Some(gallery);
        self
    }

    fn gallery_names(&self) -> Option<map::Names> {
        self.gallery
            .as_ref()
            .map(|g| map::Names::new(g, &seats::UNITS))
    }

    /// The test hooks of `WorkerConfig.fixture_test`: a basis change at a turn, a fog leak in the units.
    /// May be set before or after the start.
    pub fn set_test_hooks(&mut self, hooks: FixtureTest) {
        self.hooks = hooks;
        if let Some(g) = self.game.as_mut() {
            g.hooks = hooks;
        }
    }

    fn game(&self) -> Result<&Game, CoreError> {
        self.game
            .as_ref()
            .ok_or_else(|| CoreError::failed("fixture: no game"))
    }

    fn game_mut(&mut self) -> Result<&mut Game, CoreError> {
        self.game
            .as_mut()
            .ok_or_else(|| CoreError::failed("fixture: no game"))
    }
}

impl Core for FixtureCore {
    fn start(&mut self, start: CoreStart<'_>) -> Result<(), CoreError> {
        if self.game.is_some() {
            return Err(CoreError::new(
                CoreErrorKind::BadStart,
                "fixture: the game has started already",
            ));
        }
        let (setup, seed) = match start {
            CoreStart::New { setup, seed } => (setup, seed),
            CoreStart::Load(bytes) => {
                let mut game = Game::decode(bytes)?;
                game.hooks = self.hooks;
                game.kick_pending = self.unsafe_after_load;
                let info = game.map.info();
                self.huge = (info.width, info.height) == map::HUGE;
                self.wrap_x = info.wrap_x;
                self.game = Some(game);
                return Ok(());
            }
        };
        let heavy = heavy::Heavy::from_env();
        let (w, h) = if self.huge || self.gallery.is_some() || heavy.is_some() {
            map::HUGE
        } else {
            map::DUEL
        };
        let geo = Geo::new(w, h, self.wrap_x);
        let mut majors = seats::majors_from_setup(setup, &geo)?;
        let sites: Vec<(i16, i16)> = majors.iter().map(|m| m.start).collect();
        let terrain_seed = (seed ^ (seed >> 32)) as u32;
        let mut gallery_units = Vec::new();
        let mut map = match (&self.gallery, self.gallery_names()) {
            (Some(gallery), Some(names)) => {
                // Whose era and technologies a landmark is drawn for: the barbarians are Ancient without
                // technologies, the first major Industrial, the second Ancient with the technologies.
                let first = majors[0].player;
                let owners = map::Owners {
                    ancient: BARBARIAN,
                    industrial: first,
                    tech: majors.get(1).map_or(first, |m| m.player),
                };
                majors[0].era = map::ERA_INDUSTRIAL;
                if let Some(m) = majors.get_mut(1) {
                    m.art_techs |= ((1u64 << names.techs.len().min(63)) - 1) & !1;
                }
                let set = map::GallerySet {
                    gallery,
                    names: &names,
                    owners,
                };
                let (map, layout) = MapData::generate_gallery(geo, terrain_seed, &sites, &set)
                    .map_err(|e| CoreError::new(CoreErrorKind::BadStart, e))?;
                gallery_units = seats::gallery_units(&names, &layout.unit_cells, first);
                eprintln!(
                    "fixture: full set: {} unit types, {} landmark rows ({} not showable)",
                    gallery_units.len(),
                    gallery.landmarks.len(),
                    layout.missed.len()
                );
                for row in &layout.missed {
                    eprintln!("fixture: full set: no way to show {row}");
                }
                map
            }
            _ => MapData::generate(geo, terrain_seed, &sites),
        };

        let mut units = Vec::new();
        let mut cities = Vec::new();
        for m in &majors {
            let (city, mut own) = seats::start_position(m, &geo);
            map.place_city(m.start);
            map.claim(m.start, 2, m.player);
            cities.push(city);
            units.append(&mut own);
        }
        units.extend(seats::barbarian_units(&geo));
        // Dev only: a late-game load for the render benchmarks (`CIV5_FIXTURE_HEAVY=1500[,80]`).
        if let Some(heavy) = heavy
            && let Some(first) = cities.first().cloned()
        {
            let (more_units, more_cities) = heavy::fill(heavy, &mut map, &majors, &geo, &first);
            eprintln!(
                "fixture: heavy world: {} units, {} cities",
                more_units.len(),
                more_cities.len()
            );
            units.extend(more_units);
            cities.extend(more_cities);
        }
        // The heavy world is seen whole, like the full set: nobody has fog, everybody has met everybody.
        let full_set = self.gallery.is_some() || heavy.is_some();
        units.extend(gallery_units);

        let teams: Vec<TeamId> = majors.iter().map(|m| m.player).collect();
        let mut dip = Diplomacy::default();
        if full_set {
            // Everybody has met everybody, or the eras of the owners of the landmarks would be unknown.
            for team in &teams {
                for owner in teams.iter().chain([&BARBARIAN]) {
                    dip.meet(*team, *owner);
                }
            }
        }
        let mut game = Game {
            settings: setup.settings.clone(),
            turn: 1,
            map,
            fog: if full_set {
                Fog::lit(geo, teams)
            } else {
                Fog::new(geo, teams)
            },
            majors,
            dip,
            units,
            cities,
            phase: GamePhase::Humans,
            finished: BTreeSet::new(),
            unit_seq: BTreeMap::new(),
            dirty: BTreeSet::new(),
            events: Vec::new(),
            pending_wars: Vec::new(),
            turn_status: Vec::new(),
            hooks: self.hooks,
            ai_wait: None,
            kick_pending: false,
        };
        game.refresh_fog();
        // The first views are whole; nothing is "changed" yet.
        game.dirty.clear();
        self.game = Some(game);
        Ok(())
    }

    fn catalog(&mut self) -> Result<Catalog, CoreError> {
        Ok(match self.gallery_names() {
            Some(names) => seats::catalog_with(&names),
            None => seats::catalog(),
        })
    }

    fn choices(&mut self) -> Result<LobbyChoices, CoreError> {
        Ok(seats::choices())
    }

    fn build_id(&self) -> String {
        "fixture".into()
    }

    fn map(&mut self) -> MapInfo {
        match &self.game {
            Some(g) => g.map.info(),
            None => {
                let (w, h) = if self.huge { map::HUGE } else { map::DUEL };
                MapInfo {
                    width: w,
                    height: h,
                    wrap_x: self.wrap_x,
                    wrap_y: false,
                    terrain_seed: 0,
                }
            }
        }
    }

    fn seats(&mut self) -> Result<Vec<SlotSetup>, CoreError> {
        Ok(self.game()?.majors.iter().map(Major::slot).collect())
    }

    fn pump(&mut self, _budget: u32) -> Result<CorePump, CoreError> {
        Ok(self.game_mut()?.pump())
    }

    fn turn_status(&mut self) -> Vec<protocol::AiTurnEvent> {
        self.game
            .as_mut()
            .map_or_else(Vec::new, |g| std::mem::take(&mut g.turn_status))
    }

    fn turn(&mut self) -> Result<CoreTurn, CoreError> {
        let g = self.game_mut()?;
        let (turn_len_s, timer_base_s) = seats::timer_of(&g.settings.timer);
        Ok(CoreTurn {
            mode: g.settings.turn_mode,
            seats: g.seat_infos(),
            turn_len_s,
            timer_base_s,
            human_wars: std::mem::take(&mut g.pending_wars),
        })
    }

    fn command(&mut self, player: PlayerId, cmd: &Command) -> Result<(), Refusal> {
        match self.game.as_mut() {
            Some(g) => g.apply_command(player, cmd),
            None => Err(Refusal::other("fixture: no game")),
        }
    }

    fn end_turn_timeout(&mut self, player: PlayerId) -> Result<(), Refusal> {
        match self.game.as_mut() {
            Some(g) => g.timeout(player),
            None => Err(Refusal::other("fixture: no game")),
        }
    }

    fn set_control(&mut self, player: PlayerId, control: &SlotControl) -> Result<(), CoreError> {
        self.controls_set.push((player, control.clone()));
        let g = self.game_mut()?;
        if g.kick_pending {
            return Err(CoreError::failed(format!(
                "slot {player}: command rejected: gc_slot_control: not at a safe point: the last gc_pump did not end in the human phase (the AI is computing); pump until the humans are up"
            )));
        }
        let m = g
            .majors
            .iter_mut()
            .find(|m| m.player == player)
            .ok_or_else(|| CoreError::failed(format!("fixture: no player {player}")))?;
        // The same owner again changes nothing.
        if m.control != *control {
            if matches!(control, SlotControl::Ai) {
                g.kick_pending = true;
            }
            m.control = control.clone();
            // A seat that changes hands has not ended its turn; the AI never holds it.
            g.finished.remove(&player);
        }
        Ok(())
    }

    fn safe_point(&mut self) -> Result<(), String> {
        let g = self.game().map_err(|e| e.message)?;
        match g.phase {
            GamePhase::Humans if g.kick_pending => {
                Err("fixture: the AI that took a slot has not played yet".into())
            }
            GamePhase::Humans => Ok(()),
            GamePhase::Ai { .. } => Err("fixture: the AI is moving".into()),
        }
    }

    fn save(&mut self) -> Result<Vec<u8>, CoreError> {
        self.game()?.encode()
    }

    fn dirty_plots(&mut self) -> Option<Vec<u32>> {
        let g = self.game.as_mut()?;
        Some(std::mem::take(&mut g.dirty).into_iter().collect())
    }

    fn plots(
        &mut self,
        team: TeamId,
        subset: Option<&[u32]>,
        out: &mut Vec<(u32, PlotView)>,
    ) -> Result<(), CoreError> {
        let g = self.game()?;
        for i in g.indexes(subset)? {
            let record = if g.fog.is_revealed(team, i) {
                g.map.team_record(i, g.fog.is_visible(team, i))
            } else {
                // The whole world goes to the client (NP-D19, 2026-10-22).
                g.map
                    .team_record(i, false)
                    .map(|p| PlotView::unrevealed_of(&p))
            };
            out.extend(record.map(|p| (i, p)));
        }
        Ok(())
    }

    fn basis(
        &mut self,
        subset: Option<&[u32]>,
        out: &mut Vec<(u32, PlotBasisView)>,
    ) -> Result<(), CoreError> {
        let g = self.game()?;
        for i in g.indexes(subset)? {
            out.extend(g.map.basis_of(i).map(|b| (i, b)));
        }
        Ok(())
    }

    fn units(&mut self, team: TeamId) -> Result<Vec<UnitView>, CoreError> {
        let g = self.game()?;
        let geo = g.geo();
        let mut out: Vec<UnitView> = g
            .units
            .iter()
            .filter(|u| u.owner == team || g.fog.is_visible(team, geo.index((u.x, u.y))))
            .cloned()
            .collect();
        if self.hooks.leak_fog {
            // The test hook: one unit of another player, in sight or not (the fog auditor must see it).
            let leaked = g
                .units
                .iter()
                .filter(|u| {
                    u.owner != team && !out.iter().any(|o| (o.owner, o.id) == (u.owner, u.id))
                })
                .min_by_key(|u| (u.owner, u.id));
            out.extend(leaked.cloned());
        }
        out.sort_by_key(|u| (u.owner, u.id));
        Ok(out)
    }

    fn cities(&mut self, team: TeamId) -> Result<Vec<CityView>, CoreError> {
        let g = self.game()?;
        let geo = g.geo();
        let mut out = Vec::new();
        for c in &g.cities {
            let i = geo.index((c.x, c.y));
            if c.owner == team {
                out.push(c.clone());
            } else if g.fog.is_visible(team, i) {
                // Seen now: the whole record, but the yields of a foreign city are not known.
                out.push(CityView {
                    yields: [0; 6],
                    ..c.clone()
                });
            } else if g.fog.is_revealed(team, i) {
                // Known, not in sight: the core does not keep the state of a city it does not see; name
                // and position only, the rest zero. The worker keeps the last seen values (4.3 item 6).
                out.push(CityView {
                    id: c.id,
                    owner: c.owner,
                    x: c.x,
                    y: c.y,
                    name: c.name.clone(),
                    ..CityView::default()
                });
            }
        }
        out.sort_by_key(|c| (c.owner, c.id));
        Ok(out)
    }

    fn players(&mut self, team: TeamId) -> Result<Vec<PlayerView>, CoreError> {
        let g = self.game()?;
        Ok(seats::player_views(&g.majors, &g.dip, team))
    }

    fn header(&mut self, player: PlayerId) -> Result<ViewHeader, CoreError> {
        let g = self.game()?;
        g.major(player)
            .ok_or_else(|| CoreError::failed(format!("fixture: no player {player}")))?;
        let needs = g
            .units
            .iter()
            .find(|u| u.owner == player && u.flags.contains(UnitFlags::NEEDS_ORDERS));
        Ok(ViewHeader {
            turn: g.turn,
            year: -4000 + (g.turn - 1) * 40,
            human: player,
            human_team: player,
            status: match g.phase {
                GamePhase::Humans => TurnStatus::HumanTurn,
                GamePhase::Ai { .. } => TurnStatus::AiTurn {
                    active: g.active_ai(),
                },
            },
            blocker: needs
                .filter(|_| !g.finished.contains(&player))
                .map(|u| Blocker {
                    kind: 1,
                    target: BlockerTarget::Unit(u.id),
                }),
        })
    }

    fn top_bar(&mut self, player: PlayerId) -> Result<TopBar, CoreError> {
        let g = self.game()?;
        let m = g
            .major(player)
            .ok_or_else(|| CoreError::failed(format!("fixture: no player {player}")))?;
        Ok(seats::top_bar(g.turn, m))
    }

    fn notifications(&mut self, player: PlayerId) -> Result<Vec<NotificationView>, CoreError> {
        let g = self.game()?;
        g.major(player)
            .ok_or_else(|| CoreError::failed(format!("fixture: no player {player}")))?;
        Ok(Vec::new())
    }

    fn unit_actions(
        &mut self,
        player: PlayerId,
        unit: i32,
    ) -> Result<Vec<UnitActionView>, CoreError> {
        self.game()?.query_unit_actions(player, unit)
    }

    fn reach(
        &mut self,
        player: PlayerId,
        unit: i32,
        horizon: u8,
    ) -> Result<Vec<ReachNode>, CoreError> {
        self.game()?.query_reach(player, unit, horizon)
    }

    fn targets(
        &mut self,
        player: PlayerId,
        shooter: Shooter,
    ) -> Result<Vec<(i16, i16, u16)>, CoreError> {
        self.game()?.query_targets(player, shooter)
    }

    fn preview(
        &mut self,
        player: PlayerId,
        attacker: Shooter,
        x: i16,
        y: i16,
    ) -> Result<Option<CombatPreview>, CoreError> {
        self.game()?.query_preview(player, attacker, (x, y))
    }

    fn city_builds(
        &mut self,
        player: PlayerId,
        city: i32,
    ) -> Result<Vec<CityBuildView>, CoreError> {
        self.game()?.query_city_builds(player, city)
    }

    fn path(
        &mut self,
        player: PlayerId,
        unit: i32,
        x: i16,
        y: i16,
    ) -> Result<Vec<PathNode>, CoreError> {
        self.game()?.query_path(player, unit, (x, y))
    }

    fn can_make_peace(&mut self, a: PlayerId, b: PlayerId) -> Result<bool, CoreError> {
        let g = self.game()?;
        Ok(a != BARBARIAN
            && b != BARBARIAN
            && g.major(a).is_some()
            && g.major(b).is_some()
            && g.dip.at_war(a, b))
    }

    fn events(&mut self) -> Vec<CoreEvent> {
        self.game
            .as_mut()
            .map(|g| std::mem::take(&mut g.events))
            .unwrap_or_default()
    }

    fn digest(&mut self, viewer_team: i32) -> Result<u64, CoreError> {
        let bytes = if viewer_team < 0 {
            let g = self.game()?;
            let plots: Vec<PlotView> = (0..g.geo().plot_count() as u32)
                .filter_map(|i| g.map.plot(i).cloned())
                .collect();
            Postcard.encode(&(g.turn, plots, &g.units, &g.cities))
        } else {
            let team = TeamId::try_from(viewer_team)
                .map_err(|_| CoreError::failed("fixture: team out of range"))?;
            let mut plots = Vec::new();
            self.plots(team, None, &mut plots)?;
            let (units, cities, players) =
                (self.units(team)?, self.cities(team)?, self.players(team)?);
            let turn = self.game()?.turn;
            Postcard.encode(&(turn, plots, units, cities, players))
        };
        let bytes = bytes.map_err(|e| CoreError::failed(format!("fixture: digest: {e}")))?;
        Ok(protocol::xxh64(0, &bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_has_what_the_renderer_needs() {
        let map = build_map();
        let ps = &map.plots;
        let any = |f: &dyn Fn(&PlotView) -> bool| ps.iter().any(f);
        for t in [
            PlotType::Land,
            PlotType::Hill,
            PlotType::Mountain,
            PlotType::Water,
        ] {
            assert!(
                any(&|p| p.flags.contains(PlotFlags::REVEALED) && p.plot_type == t),
                "{t:?}"
            );
        }
        for f in 0..6 {
            assert!(any(&|p| p.feature == f), "feature {f}");
        }
        for t in 0..7 {
            assert!(
                any(&|p| p.flags.contains(PlotFlags::REVEALED) && p.terrain == t),
                "terrain {t}"
            );
        }
        for r in 0..4 {
            assert!(any(&|p| p.resource == r), "resource {r}");
        }
        for i in 0..IMPROVEMENTS.len() as i16 {
            assert!(any(&|p| p.improvement == i), "improvement {i}");
        }
        assert!(any(&|p| p.route == 0));
        assert!(any(&|p| p.flags.contains(PlotFlags::IS_LAKE)));
        assert!(any(&|p| p.flags.contains(PlotFlags::IS_COASTAL)));
        for e in 0..6 {
            assert!(any(&|p| p.rivers & (1 << e) != 0), "river edge {e}");
        }
        assert_eq!(
            (0..4).filter(|a| any(&|p| p.continent_art == *a)).count(),
            4
        );
        assert!(any(&|p| p.owner == HUMAN) && any(&|p| p.owner == AI));
        // three fog kinds
        assert!(any(&|p| p.flags.contains(PlotFlags::VISIBLE)));
        assert!(any(
            &|p| p.flags.contains(PlotFlags::REVEALED) && !p.flags.contains(PlotFlags::VISIBLE)
        ));
        assert!(any(&|p| p.flags == PlotFlags::default()));
    }

    #[test]
    fn camp_and_ruins_stand_on_open_land() {
        let m = build_map();
        let at = |(x, y): (i16, i16)| &m.plots[y as usize * WIDTH as usize + x as usize];
        for c in CAMP_AT {
            assert_eq!(at(c).improvement, CAMP);
            assert_eq!(at(c).plot_type, PlotType::Land);
        }
        assert_eq!(at(RUINS_AT).improvement, RUINS);
        // One camp in sight, one in the fog; the road reaches the capital.
        assert!(at(CAMP_AT[0]).flags.contains(PlotFlags::VISIBLE));
        assert!(!at(CAMP_AT[1]).flags.contains(PlotFlags::VISIBLE));
        assert!(at(CAMP_AT[1]).flags.contains(PlotFlags::REVEALED));
        assert_eq!(at((7, 12)).route, 0);
    }

    #[test]
    fn map_is_deterministic() {
        assert_eq!(build_map(), build_map());
        assert_eq!(build_map_with(true), build_map_with(true));
    }

    #[test]
    fn wrapped_map_has_land_borders_and_a_river_over_the_seam() {
        let plain = build_map();
        assert!(!plain.wrap_x);
        let m = build_map_with(true);
        assert!(m.wrap_x);
        let at = |m: &MapView, x: usize, y: usize| m.plots[y * WIDTH as usize + x].clone();
        // No edge water at the seam, in the middle rows.
        for y in 6..=16 {
            assert_ne!(at(&m, 0, y).plot_type, PlotType::Water, "x 0, y {y}");
            assert_ne!(
                at(&m, WIDTH as usize - 1, y).plot_type,
                PlotType::Water,
                "x last, y {y}"
            );
            // The river is on both sides of the seam edge.
            assert_eq!(at(&m, WIDTH as usize - 1, y).rivers, 1 << 1);
            assert_eq!(at(&m, 0, y).rivers, 1 << 4);
        }
        // Two owners meet over the seam.
        assert_eq!(at(&m, 0, 12).owner, HUMAN);
        assert_eq!(at(&m, WIDTH as usize - 1, 12).owner, AI);
        assert_eq!(at(&plain, 0, 12).plot_type, PlotType::Water);
    }
}

#[cfg(test)]
mod core_tests {
    use protocol::{BasisState, MapBasis, basis_hash_of};

    use super::seats::testing::setup;
    use super::*;
    use crate::core::CorePhase;

    fn started(huge: bool, wrap: bool, humans: usize, ais: usize) -> FixtureCore {
        let mut core = FixtureCore::new(huge, wrap);
        core.start(CoreStart::New {
            setup: &setup(humans, ais),
            seed: 42,
        })
        .unwrap();
        core
    }

    fn team_plots(core: &mut FixtureCore, team: TeamId) -> Vec<(u32, PlotView)> {
        let mut out = Vec::new();
        core.plots(team, None, &mut out).unwrap();
        out
    }

    fn revealed(plots: &[(u32, PlotView)]) -> BTreeSet<u32> {
        plots
            .iter()
            .filter(|(_, p)| p.flags.contains(PlotFlags::REVEALED))
            .map(|(i, _)| *i)
            .collect()
    }

    #[test]
    fn starts_on_every_seat_count_and_map() {
        for (huge, wrap) in [(false, false), (false, true), (true, false), (true, true)] {
            for (h, a) in [(1, 1), (2, 1), (4, 2), (8, 2), (8, 0)] {
                let mut core = started(huge, wrap, h, a);
                let info = core.map();
                let (w, ht) = if huge { map::HUGE } else { map::DUEL };
                assert_eq!((info.width, info.height, info.wrap_x), (w, ht, wrap));
                assert_ne!(info.terrain_seed, 0);
                let seats = core.seats().unwrap();
                assert_eq!(seats.len(), h + a);
                let humans = seats
                    .iter()
                    .filter(|s| matches!(s.control, SlotControl::Human { .. }))
                    .count();
                assert_eq!(humans, h);
                assert_eq!(core.turn().unwrap().seats.len(), h);
                let pump = core.pump(32).unwrap();
                assert_eq!((pump.phase, pump.turn), (CorePhase::Humans, 1));
            }
        }
        let mut core = started(false, false, 2, 1);
        let again = core.start(CoreStart::New {
            setup: &setup(2, 1),
            seed: 1,
        });
        assert_eq!(again.unwrap_err().kind, CoreErrorKind::BadStart);
        let load = FixtureCore::new(false, false).start(CoreStart::Load(b"x"));
        assert_eq!(load.unwrap_err().kind, CoreErrorKind::BadSave);
    }

    #[test]
    fn fog_differs_between_seats() {
        for (huge, humans) in [(false, 4), (false, 8), (true, 8)] {
            let mut core = started(huge, false, humans, 2);
            let geo = core.game().unwrap().geo();
            let mut sets = Vec::new();
            for team in 0..humans as TeamId {
                let plots = team_plots(&mut core, team);
                let seen = revealed(&plots);
                // Radius 2 around the city and the units: more than the city's 19 plots, far from all.
                assert!(seen.len() >= 19, "team {team}: {} revealed", seen.len());
                assert!(seen.len() < geo.plot_count() / 4);
                // At the start what is revealed is in sight.
                for (i, p) in &plots {
                    assert_eq!(
                        p.flags.contains(PlotFlags::VISIBLE),
                        p.flags.contains(PlotFlags::REVEALED),
                        "plot {i}"
                    );
                }
                // The unrevealed plots carry the world (NP-D19, 2026-10-22), nothing of sight.
                let world = |i: u32| {
                    let g = core.game.as_ref().unwrap();
                    PlotView::unrevealed_of(&g.map.team_record(i, false).unwrap())
                };
                for (i, p) in plots.iter().filter(|(i, _)| !seen.contains(i)) {
                    assert!(!p.has_sight_data(), "plot {i}");
                    assert_eq!(*p, world(*i), "plot {i}");
                }
                sets.push(seen);
            }
            for a in 0..sets.len() {
                for b in a + 1..sets.len() {
                    assert!(
                        sets[a].is_disjoint(&sets[b]),
                        "teams {a} and {b} share plots"
                    );
                }
            }
        }
    }

    #[test]
    fn units_and_cities_follow_the_fog() {
        let mut core = started(false, false, 4, 2);
        for team in 0..4 {
            let units = core.units(team).unwrap();
            assert!(
                units.iter().all(|u| u.owner == team),
                "team {team} sees a foreign unit"
            );
            assert!((5..=6).contains(&units.len()));
            let cities = core.cities(team).unwrap();
            assert_eq!(cities.len(), 1);
            assert_eq!(cities[0].owner, team);
            let players = core.players(team).unwrap();
            assert_eq!(players.len(), 7); // 6 majors and the barbarians
            let met: Vec<_> = players
                .iter()
                .filter(|p| p.flags.contains(PlayerFlags::MET))
                .collect();
            assert!(met.is_empty());
            assert!(players.iter().filter(|p| p.id != team).all(|p| p.era == -1));
            // The sorting the worker relies on.
            assert!(
                units
                    .windows(2)
                    .all(|w| (w[0].owner, w[0].id) < (w[1].owner, w[1].id))
            );
        }
    }

    #[test]
    fn a_city_that_left_the_sight_is_name_and_position_only() {
        let mut core = started(false, false, 2, 0);
        let city0 = core.cities(0).unwrap()[0].clone();
        {
            // The scout of player 1 walks up to the capital of player 0.
            let g = core.game.as_mut().unwrap();
            let scout = g
                .units
                .iter_mut()
                .find(|u| u.owner == 1 && u.unit_type == seats::U_SCOUT)
                .unwrap();
            (scout.x, scout.y) = (city0.x + 1, city0.y);
            g.refresh_fog();
        }
        let seen = core.cities(1).unwrap();
        let foreign = seen.iter().find(|c| c.owner == 0).expect("city in sight");
        assert_eq!(foreign.population, city0.population);
        assert_eq!(
            foreign.yields, [0; 6],
            "yields of a foreign city are unknown"
        );
        assert!(
            core.players(1)
                .unwrap()
                .iter()
                .any(|p| p.id == 0 && p.flags.contains(PlayerFlags::MET))
        );
        assert!(
            core.players(0)
                .unwrap()
                .iter()
                .any(|p| p.id == 1 && p.flags.contains(PlayerFlags::MET))
        );

        {
            // The scout goes home: the city is revealed, not visible.
            let g = core.game.as_mut().unwrap();
            let home = g.major(1).unwrap().start;
            let scout = g
                .units
                .iter_mut()
                .find(|u| u.owner == 1 && u.unit_type == seats::U_SCOUT)
                .unwrap();
            (scout.x, scout.y) = home;
            g.refresh_fog();
        }
        let seen = core.cities(1).unwrap();
        let stub = seen
            .iter()
            .find(|c| c.owner == 0)
            .expect("city still known");
        assert_eq!(
            *stub,
            CityView {
                id: city0.id,
                owner: 0,
                x: city0.x,
                y: city0.y,
                name: city0.name.clone(),
                ..CityView::default()
            }
        );
        // And the units of player 0 are gone from the view of player 1.
        assert!(core.units(1).unwrap().iter().all(|u| u.owner == 1));
    }

    #[test]
    fn basis_covers_all_plots_and_hashes() {
        for (huge, wrap) in [(false, false), (false, true), (true, false)] {
            let mut core = started(huge, wrap, 3, 1);
            let info = core.map();
            let mut all = Vec::new();
            core.basis(None, &mut all).unwrap();
            assert_eq!(all.len(), info.plot_count());
            assert!(all.iter().enumerate().all(|(n, (i, _))| n as u32 == *i));
            let mut basis = MapBasis {
                game: 1,
                basis_hash: 0,
                width: info.width,
                height: info.height,
                wrap_x: info.wrap_x,
                wrap_y: info.wrap_y,
                terrain_seed: info.terrain_seed,
                plots: all.into_iter().map(|(_, b)| b).collect(),
            };
            basis.basis_hash = basis_hash_of(&basis);
            let state = BasisState::new(basis).unwrap();
            assert_eq!(state.hash(), state.map().basis_hash);

            // A subset gives the same records.
            let mut some = Vec::new();
            core.basis(Some(&[5, 3, 3, 100]), &mut some).unwrap();
            assert_eq!(
                some.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
                vec![3, 5, 100]
            );
            for (i, b) in &some {
                assert_eq!(*b, state.map().plots[*i as usize]);
            }
            let outside = info.plot_count() as u32;
            assert!(core.basis(Some(&[outside]), &mut Vec::new()).is_err());
        }
    }

    #[test]
    fn basis_has_the_relief_and_agrees_with_the_plots() {
        let mut core = started(false, true, 2, 0);
        let mut basis = Vec::new();
        core.basis(None, &mut basis).unwrap();
        let rivers = basis.iter().filter(|(_, b)| b.rivers != 0).count();
        let flows = basis
            .iter()
            .flat_map(|(_, b)| b.river_flow)
            .filter(|f| *f >= 0)
            .count();
        let feature = |f: i16| basis.iter().filter(|(_, b)| b.feature == f).count();
        assert!(rivers > 20 && flows > 5, "rivers {rivers} flows {flows}");
        assert!(feature(map::F_ICE) > 0);
        assert!(feature(map::F_MARSH) > 0);
        assert!(feature(map::F_FLOOD_PLAINS) > 0);
        // A team's plot agrees with the basis where it has seen it.
        let plots = team_plots(&mut core, 0);
        for (i, p) in plots
            .iter()
            .filter(|(_, p)| p.flags.contains(PlotFlags::REVEALED))
        {
            let b = &basis[*i as usize].1;
            assert_eq!(
                (p.plot_type, p.terrain, p.continent_art, p.rivers),
                (b.plot_type, b.terrain, b.continent_art, b.rivers)
            );
            assert_eq!(p.flags.0 & b.flags.0, b.flags.0);
            if b.feature >= 0 {
                assert_eq!(p.feature, b.feature);
            }
        }
    }

    #[test]
    fn natural_wonders_are_in_the_basis_under_fog() {
        let mut core = started(false, false, 4, 2);
        let geo = core.game().unwrap().geo();
        let wonders = map::specials(&geo).wonders;
        let mut basis = Vec::new();
        core.basis(None, &mut basis).unwrap();
        for (feature, at) in wonders {
            let i = geo.index(at);
            let b = &basis[i as usize].1;
            assert_eq!(b.feature, feature);
            assert!(b.flags.contains(PlotFlags::NATURAL_WONDER));
            for team in 0..4 {
                let mut one = Vec::new();
                core.plots(team, Some(&[i]), &mut one).unwrap();
                // Unrevealed, and the plot still carries its wonder (the whole world, NP-D19).
                let p = &one[0].1;
                assert!(
                    !p.flags.contains(PlotFlags::REVEALED),
                    "team {team} sees wonder {at:?}"
                );
                assert_eq!(p.feature, feature);
                assert!(p.flags.contains(PlotFlags::NATURAL_WONDER) && !p.has_sight_data());
            }
        }
        let count = basis
            .iter()
            .filter(|(_, b)| b.flags.contains(PlotFlags::NATURAL_WONDER))
            .count();
        assert_eq!(count, wonders.len());
    }

    #[test]
    fn dirty_plots_follow_the_fog() {
        let mut core = started(false, false, 2, 1);
        assert_eq!(
            core.dirty_plots(),
            Some(Vec::new()),
            "nothing changed at the start"
        );
        let before = (team_plots(&mut core, 0), team_plots(&mut core, 1));
        {
            let g = core.game.as_mut().unwrap();
            let scout = g
                .units
                .iter_mut()
                .find(|u| u.owner == 0 && u.unit_type == seats::U_SCOUT)
                .unwrap();
            scout.x += 2;
            g.refresh_fog();
        }
        let dirty = core.dirty_plots().expect("always known");
        assert!(!dirty.is_empty());
        assert!(dirty.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(core.dirty_plots(), Some(Vec::new()), "taken once");
        // Every plot that differs is in the dirty list, and only team 0 changed.
        let after = (team_plots(&mut core, 0), team_plots(&mut core, 1));
        assert_eq!(before.1, after.1);
        let differing: Vec<u32> = before
            .0
            .iter()
            .zip(&after.0)
            .filter(|(a, b)| a != b)
            .map(|(a, _)| a.0)
            .collect();
        assert!(!differing.is_empty());
        assert!(differing.iter().all(|i| dirty.contains(i)));
        // A plot that is asked by index equals the same one of the whole answer.
        let mut some = Vec::new();
        core.plots(0, Some(&dirty), &mut some).unwrap();
        for (i, p) in some {
            assert_eq!(after.0[i as usize].1, p);
        }
    }

    #[test]
    fn header_top_bar_and_timers() {
        let mut core = started(false, false, 2, 0);
        let h = core.header(0).unwrap();
        assert_eq!((h.turn, h.human, h.human_team), (1, 0, 0));
        assert!(h.blocker.is_some());
        assert!(core.header(5).is_err());
        assert!(core.top_bar(1).unwrap().gold > 0);
        assert!(core.notifications(0).unwrap().is_empty());
        let t = core.turn().unwrap();
        assert_eq!((t.turn_len_s, t.timer_base_s), (0, 0));

        let mut with_timer = setup(2, 0);
        with_timer.settings.timer = protocol::TimerMode::Original {
            row: "TURNTIMER_SLOW".into(),
        };
        with_timer.settings.turn_mode = protocol::TurnMode::Simultaneous;
        let mut core = FixtureCore::new(false, false);
        core.start(CoreStart::New {
            setup: &with_timer,
            seed: 1,
        })
        .unwrap();
        let t = core.turn().unwrap();
        assert_eq!((t.turn_len_s, t.timer_base_s), (120, 120));
        assert_eq!(t.mode, protocol::TurnMode::Simultaneous);
    }

    #[test]
    fn digests_differ_by_viewer_and_repeat() {
        let mut a = started(false, false, 3, 0);
        let mut b = started(false, false, 3, 0);
        assert_eq!(a.digest(-1).unwrap(), b.digest(-1).unwrap());
        assert_eq!(a.digest(0).unwrap(), b.digest(0).unwrap());
        assert_ne!(a.digest(0).unwrap(), a.digest(1).unwrap());
        assert_ne!(a.digest(0).unwrap(), a.digest(-1).unwrap());
    }

    #[test]
    fn slots_change_hands_and_peace_needs_a_war() {
        let mut core = started(false, false, 2, 1);
        core.set_control(1, &SlotControl::Ai).unwrap();
        assert!(matches!(core.seats().unwrap()[1].control, SlotControl::Ai));
        assert_eq!(core.turn().unwrap().seats.len(), 1);
        // Like the real core: after a kick no slot changes until the AI has played (a pump).
        assert!(core.safe_point().is_err());
        assert!(
            core.set_control(1, &SlotControl::Human { nick: "bob".into() })
                .is_err()
        );
        while !matches!(core.pump(1).unwrap().phase, CorePhase::Humans) {}
        core.safe_point().unwrap();
        core.set_control(1, &SlotControl::Human { nick: "bob".into() })
            .unwrap();
        assert!(core.set_control(30, &SlotControl::Ai).is_err());
        assert!(!core.can_make_peace(0, 1).unwrap());
        core.game.as_mut().unwrap().dip.declare_war(0, 1);
        assert!(core.can_make_peace(0, 1).unwrap());
        assert!(!core.can_make_peace(0, BARBARIAN).unwrap());
    }

    #[test]
    fn no_game_no_answers() {
        let mut core = FixtureCore::new(false, false);
        assert!(core.command(0, &Command::EndTurn { force: false }).is_err());
        assert!(core.end_turn_timeout(0).is_err());
        assert!(core.pump(1).is_err());
        assert!(core.save().is_err());
        assert!(core.events().is_empty());
        assert!(core.safe_point().is_err());
        assert!(core.dirty_plots().is_none());
        let mut core = started(false, false, 2, 0);
        assert!(core.safe_point().is_ok());
        assert!(core.events().is_empty());
    }
}

#[cfg(test)]
mod gallery_tests {
    use super::seats::testing::setup;
    use super::*;

    const UNITS_CSV: &str = "kind,id,x\nunit,UNIT_SETTLER,1\nunit,UNIT_AIRSHIP,1\nmember,UNIT_AIRSHIP,1\nunit,UNIT_AIRSHIP,1\n";
    const LANDMARKS_CSV: &str = "row,improvement,resource,era,state,tech\n\
        0,ART_DEF_IMPROVEMENT_MINE,ART_DEF_RESOURCE_IRON_HEAVY_MARSH,Any,Any,\n\
        1,ART_DEF_IMPROVEMENT_FARM,ART_DEF_RESOURCE_WHEAT,Any,UnderConstruction,\n\
        2,ART_DEF_IMPROVEMENT_NONE,ART_DEF_FEATURE_ATOLL,Any,Any,\n\
        3,ART_DEF_IMPROVEMENT_POLDER,ART_DEF_RESOURCE_ALL,Ancient,Pillaged,TECH_ECONOMICS\n\
        4,ART_DEF_FEATURE_HADRIANS_WALL_1,ART_DEF_RESOURCE_ALL,Any,Any,\n\
        5,ART_DEF_IMPROVEMENT_CAMP,ART_DEF_RESOURCE_FURS_HILL,Industrial,Constructed,\n\
        6,ART_DEF_IMPROVEMENT_POLDER,ART_DEF_RESOURCE_ALL,Ancient,UnderConstruction,TECH_ECONOMICS\n";

    fn full_set(humans: usize, ais: usize) -> (FixtureCore, map::Names) {
        let gallery = map::Gallery::parse(UNITS_CSV, LANDMARKS_CSV).unwrap();
        let names = map::Names::new(&gallery, &seats::UNITS);
        let mut core = FixtureCore::new(false, false).with_gallery(gallery);
        core.start(CoreStart::New {
            setup: &setup(humans, ais),
            seed: 42,
        })
        .unwrap();
        (core, names)
    }

    #[test]
    fn the_tables_are_read_by_column() {
        let g = map::Gallery::parse(UNITS_CSV, LANDMARKS_CSV).unwrap();
        assert_eq!(g.units, ["UNIT_SETTLER", "UNIT_AIRSHIP"]);
        assert_eq!(g.landmarks.len(), 7);
        assert_eq!(g.landmarks[3].tech, "TECH_ECONOMICS");
        assert!(map::Gallery::parse("kind,id\n", LANDMARKS_CSV).is_err());
        assert!(map::Gallery::parse("a,b\nx,y\n", LANDMARKS_CSV).is_err());
    }

    #[test]
    fn every_unit_type_and_landmark_row_has_a_place() {
        let (mut core, names) = full_set(1, 1);
        // The 8 plain types and the airship.
        assert_eq!(names.units.len(), 9);
        let catalog = core.catalog().unwrap();
        assert_eq!(catalog.units.len(), 9);
        assert_eq!(catalog.units[8].type_name, "UNIT_AIRSHIP");
        assert_eq!(
            catalog.eras[map::ERA_INDUSTRIAL as usize].type_name,
            "ERA_INDUSTRIAL"
        );
        // Huge map, no fog, a unit of every type on its own plot.
        let info = core.map();
        assert_eq!((info.width, info.height), map::HUGE);
        let units = core.units(0).unwrap();
        let gallery_units: Vec<_> = units.iter().filter(|u| u.id >= 1000).collect();
        assert_eq!(gallery_units.len(), 9);
        let types: BTreeSet<i16> = gallery_units.iter().map(|u| u.unit_type).collect();
        assert_eq!(types.len(), 9);
        let places: BTreeSet<(i16, i16)> = gallery_units.iter().map(|u| (u.x, u.y)).collect();
        assert_eq!(places.len(), 9);
        let mut plots = Vec::new();
        core.plots(0, None, &mut plots).unwrap();
        assert!(
            plots
                .iter()
                .all(|(_, p)| p.flags.contains(PlotFlags::VISIBLE))
        );
        // The landmark plots: the row says what lies on the plot.
        let by = |f: &dyn Fn(&PlotView) -> bool| plots.iter().filter(|(_, p)| f(p)).count();
        let iron = catalog
            .resources
            .iter()
            .find(|r| r.type_name == "RESOURCE_IRON")
            .unwrap()
            .id;
        let marsh_mine =
            by(&|p| p.resource == iron && p.feature == map::F_MARSH && p.improvement == 1);
        assert_eq!(marsh_mine, 1);
        let atoll = catalog
            .features
            .iter()
            .find(|r| r.type_name == "FEATURE_ATOLL")
            .unwrap()
            .id;
        assert_eq!(
            by(&|p| p.feature == atoll && p.plot_type == PlotType::Water),
            1
        );
        let pillaged = by(&|p| p.flags.contains(PlotFlags::PILLAGED));
        assert_eq!(pillaged, 1);
        // The farm in construction names a known build; the polder and the camp have none.
        assert_eq!(by(&|p| p.building == 2), 1);
        assert!(by(&|p| p.resource >= 0 && p.plot_type == PlotType::Hill) >= 1);
    }

    #[test]
    fn owners_stand_for_the_era_and_the_technology() {
        let (mut core, _) = full_set(1, 1);
        let mut plots = Vec::new();
        core.plots(0, None, &mut plots).unwrap();
        let polder = plots
            .iter()
            .find(|(_, p)| p.flags.contains(PlotFlags::PILLAGED))
            .unwrap()
            .1
            .owner;
        let players = core.players(0).unwrap();
        let tech_owner = players.iter().find(|p| p.id == polder).unwrap();
        assert_eq!(tech_owner.era, 0);
        assert_ne!(tech_owner.art_techs & 0b10, 0);
        let industrial = players.iter().find(|p| p.id == 0).unwrap();
        assert_eq!(industrial.era, map::ERA_INDUSTRIAL);
        let barbarians = players.iter().find(|p| p.id == map::BARBARIAN).unwrap();
        assert_eq!((barbarians.era, barbarians.art_techs), (0, 0));
    }

    #[test]
    fn rivers_stay_consistent_around_the_laid_plots() {
        let (core, _) = full_set(2, 2);
        let m = &core.game.as_ref().unwrap().map;
        let geo = m.geo();
        for y in 0..geo.h {
            for x in 0..geo.w {
                let p = m.plot(geo.index((x, y))).unwrap();
                for dir in 0..6u8 {
                    if p.rivers & (1 << dir) != 0 {
                        let nb = geo.neighbor((x, y), dir).unwrap();
                        let q = m.plot(geo.index(nb)).unwrap();
                        assert_ne!(q.rivers & (1 << ((dir + 3) % 6)), 0, "({x},{y}) dir {dir}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_saved_game_loads_again() {
        let (mut core, _) = full_set(1, 1);
        let bytes = core.save().unwrap();
        let mut again = FixtureCore::new(false, false);
        again.start(CoreStart::Load(&bytes)).unwrap();
        assert_eq!(again.map().width, map::HUGE.0);
        assert_eq!(again.units(0).unwrap().len(), core.units(0).unwrap().len());
    }

    /// The real asset reports, when `CIV5_FIXTURE_GALLERY` names their directory.
    #[test]
    fn the_real_set_fits_the_map() {
        let Some(dir) = std::env::var_os(map::GALLERY_ENV) else {
            return;
        };
        let gallery = map::Gallery::load(std::path::Path::new(&dir)).unwrap();
        let rows = gallery.landmarks.len();
        let types = gallery.units.len();
        let mut core = FixtureCore::new(false, false).with_gallery(gallery);
        core.start(CoreStart::New {
            setup: &setup(1, 1),
            seed: 7,
        })
        .unwrap();
        let units = core.units(0).unwrap();
        let shown = units.iter().filter(|u| u.id >= 1000).count();
        assert!(shown >= types, "{shown} of {types} unit types");
        eprintln!("full set: {types} unit types, {rows} landmark rows");
    }
}
