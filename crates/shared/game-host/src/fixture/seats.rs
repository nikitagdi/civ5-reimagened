//! Seats of the test world (game-host-mp 4.14): 1-8 humans, 1-2 AI and the barbarians, each major with a
//! start site, a capital and 5-6 units; the fixed catalog and pick lists; player records as a team sees them.

use std::collections::BTreeSet;

use protocol::{
    ArtRefs, Catalog, CatalogEntry, CityFlags, CityView, CivChoice, CivPick, Codec, GameSetup,
    LobbyChoice, LobbyChoices, OptionChoice, PlayerFlags, PlayerId, PlayerView, Postcard,
    SlotControl, SlotSetup, TeamId, TimerMode, TopBar, UnitFlags, UnitView, WorldSizeChoice,
};
use serde::{Deserialize, Serialize};

use super::map::{
    self, BARBARIAN, DUEL, FEATURES, Geo, HUGE, IMPROVEMENTS, RESOURCES, ROUTES, TERRAINS,
};
use crate::core::{CoreError, CoreErrorKind};

/// Most major players (8 humans and 2 AI).
pub const MAX_MAJORS: usize = 10;
/// Most humans.
pub const MAX_HUMANS: usize = 8;

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
pub const U_SETTLER: i16 = 0;
pub const U_WORKER: i16 = 1;
pub const U_WARRIOR: i16 = 2;
pub const U_ARCHER: i16 = 3;
pub const U_HORSEMAN: i16 = 4;
pub const U_SCOUT: i16 = 5;

pub const BUILDINGS: [&str; 3] = ["BUILDING_MONUMENT", "BUILDING_GRANARY", "BUILDING_WALLS"];

/// Civilizations: type, name, leader, capital name, colours (primary, secondary), art style.
const CIVS: [(&str, &str, &str, &str, u32, u32, &str); 10] = [
    (
        "CIVILIZATION_ROME",
        "Rome",
        "Augustus",
        "Roma",
        0xB0_20_20_FF,
        0xF0_D0_40_FF,
        "MEDITERRANEAN",
    ),
    (
        "CIVILIZATION_EGYPT",
        "Egypt",
        "Ramesses",
        "Thebes",
        0x20_60_C0_FF,
        0xFF_FF_FF_FF,
        "AFRICAN",
    ),
    (
        "CIVILIZATION_GREECE",
        "Greece",
        "Alexander",
        "Athens",
        0x40_90_E0_FF,
        0xFF_FF_FF_FF,
        "MEDITERRANEAN",
    ),
    (
        "CIVILIZATION_PERSIA",
        "Persia",
        "Darius",
        "Persepolis",
        0x60_30_A0_FF,
        0xE0_C0_40_FF,
        "ASIAN",
    ),
    (
        "CIVILIZATION_BABYLON",
        "Babylon",
        "Nebuchadnezzar",
        "Babylon",
        0x30_80_50_FF,
        0xF0_F0_C0_FF,
        "ASIAN",
    ),
    (
        "CIVILIZATION_CHINA",
        "China",
        "Wu Zetian",
        "Beijing",
        0x20_A0_60_FF,
        0xFF_FF_FF_FF,
        "ASIAN",
    ),
    (
        "CIVILIZATION_INDIA",
        "India",
        "Gandhi",
        "Delhi",
        0xE0_80_20_FF,
        0x20_60_20_FF,
        "ASIAN",
    ),
    (
        "CIVILIZATION_AZTEC",
        "Aztecs",
        "Montezuma",
        "Tenochtitlan",
        0x20_90_90_FF,
        0xF0_E0_A0_FF,
        "AMERICAN",
    ),
    (
        "CIVILIZATION_SONGHAI",
        "Songhai",
        "Askia",
        "Gao",
        0x90_50_20_FF,
        0xF0_F0_F0_FF,
        "AFRICAN",
    ),
    (
        "CIVILIZATION_ARABIA",
        "Arabia",
        "Harun al-Rashid",
        "Mecca",
        0x30_30_30_FF,
        0x40_D0_80_FF,
        "AFRICAN",
    ),
];

/// Rows of `TurnTimers`: type, name, base time in seconds (no additions).
pub const TIMERS: [(&str, &str, u32); 2] = [
    ("TURNTIMER_FAST", "Fast", 30),
    ("TURNTIMER_SLOW", "Slow", 120),
];

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

/// The catalog of the test world: every id the map and the units use. `art_techs` has one entry (the
/// tech `TECH_AGRICULTURE`).
pub fn catalog() -> Catalog {
    let civ_types: Vec<&str> = CIVS.iter().map(|c| c.0).collect();
    let civ_names: Vec<&str> = CIVS.iter().map(|c| c.1).collect();
    let mut civs = entries(&civ_types, &civ_names);
    for (entry, civ) in civs.iter_mut().zip(CIVS) {
        entry.art.art_style = civ.6.into();
    }
    let leader_types: Vec<String> = CIVS
        .iter()
        .map(|c| format!("LEADER_{}", c.1.to_uppercase()))
        .collect();
    let leader_types: Vec<&str> = leader_types.iter().map(String::as_str).collect();
    let leader_names: Vec<&str> = CIVS.iter().map(|c| c.2).collect();
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
        techs: entries(&["TECH_AGRICULTURE"], &["Agriculture"]),
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
            &[
                "Ice",
                "Jungle",
                "Marsh",
                "Oasis",
                "Flood Plains",
                "Forest",
                "Mt. Fuji",
                "Crater",
                "Mesa",
            ],
        ),
        resources: entries(&RESOURCES, &["Iron", "Horses", "Wheat", "Fish"]),
        improvements: entries(
            &IMPROVEMENTS,
            &["Farm", "Mine", "Pasture", "Barbarian Camp", "Ancient Ruins"],
        ),
        routes: entries(&ROUTES, &["Road"]),
        civs,
        leaders: entries(&leader_types, &leader_names),
        eras: entries(
            &["ERA_ANCIENT", "ERA_RENAISSANCE"],
            &["Ancient", "Renaissance"],
        ),
        handicaps: entries(&["HANDICAP_PRINCE"], &["Prince"]),
        art_techs: vec![0],
        ..Catalog::default()
    }
}

/// Catalog entries from art defines: the type name is the art define without `ART_DEF_`.
fn art_entries(arts: &[String]) -> Vec<CatalogEntry> {
    arts.iter()
        .enumerate()
        .map(|(i, a)| CatalogEntry {
            id: i as i16,
            type_name: map::type_of(a),
            name: map::type_of(a),
            art: ArtRefs {
                art_define: a.clone(),
                icon_index: i as i16,
                ..ArtRefs::default()
            },
        })
        .collect()
}

/// The catalog of the full set (card N-R2): the plain catalog with every unit type, improvement,
/// resource and feature the set names, the technologies of its rows and the Industrial era.
pub fn catalog_with(names: &map::Names) -> Catalog {
    let mut c = catalog();
    let units: Vec<&str> = names.units.iter().map(String::as_str).collect();
    let techs: Vec<&str> = names.techs.iter().map(String::as_str).collect();
    c.units = entries(&units, &[]);
    c.improvements = art_entries(&names.improvements);
    c.resources = art_entries(&names.resources);
    c.features = art_entries(&names.features);
    c.techs = entries(&techs, &["Agriculture"]);
    c.art_techs = (0..techs.len()).map(|i| i as _).collect();
    c.eras = entries(
        &["ERA_ANCIENT", "ERA_RENAISSANCE", "ERA_INDUSTRIAL"],
        &["Ancient", "Renaissance", "Industrial"],
    );
    c
}

/// A unit of every type of the full set, standing still on `cells` (one cell per type, in the order of
/// `Names::units`). Ids start at 1000 so that they do not meet those of a start position.
pub fn gallery_units(names: &map::Names, cells: &[(i16, i16)], owner: PlayerId) -> Vec<UnitView> {
    names
        .units
        .iter()
        .zip(cells)
        .enumerate()
        .map(|(i, (t, at))| unit(1000 + i as i32, owner, i as i16, *at, 0, 0, 0, t))
        .collect()
}

/// Fixed pick lists of the lobby.
pub fn choices() -> LobbyChoices {
    let choice = |t: &str, n: &str| LobbyChoice {
        type_name: t.into(),
        name: n.into(),
    };
    let mut c = LobbyChoices {
        hash: 0,
        civs: CIVS
            .iter()
            .map(|c| CivChoice {
                id: c.0.into(),
                name: c.1.into(),
                leader: c.2.into(),
                traits: vec![],
                unique: vec![],
            })
            .collect(),
        handicaps: vec![choice("HANDICAP_PRINCE", "Prince")],
        world_sizes: vec![
            WorldSizeChoice {
                id: "WORLDSIZE_DUEL".into(),
                name: "Duel".into(),
                width: DUEL.0,
                height: DUEL.1,
                default_players: 2,
                max_players: 4,
                default_minor: 0,
                max_minor: 0,
            },
            WorldSizeChoice {
                id: "WORLDSIZE_HUGE".into(),
                name: "Huge".into(),
                width: HUGE.0,
                height: HUGE.1,
                default_players: 8,
                max_players: MAX_MAJORS as u8,
                default_minor: 0,
                max_minor: 0,
            },
        ],
        map_scripts: vec![choice("fixture.lua", "Test world")],
        speeds: vec![choice("GAMESPEED_STANDARD", "Standard")],
        eras: vec![
            choice("ERA_ANCIENT", "Ancient"),
            choice("ERA_RENAISSANCE", "Renaissance"),
        ],
        timers: TIMERS.iter().map(|t| choice(t.0, t.1)).collect(),
        options: vec![OptionChoice {
            key: "GAMEOPTION_FIXTURE".into(),
            name: "Fixture".into(),
            default: 0,
            values: Vec::new(),
            help: String::new(),
            group: protocol::OptionGroup::General,
        }],
        victories: vec![
            choice("VICTORY_TIME", "Time"),
            choice("VICTORY_DOMINATION", "Domination"),
        ],
        map_options: vec![protocol::MapScriptOptions {
            script: "fixture.lua".into(),
            options: vec![protocol::MapOptionChoice {
                id: "1".into(),
                name: "Sea level".into(),
                help: String::new(),
                default: 2,
                values: vec![(1, "Low".into()), (2, "Medium".into()), (3, "High".into())],
                read_only: false,
            }],
        }],
        max_minor_civs: 0,
        mods: Vec::new(),
    };
    // The hash of the content with `hash` zero.
    if let Ok(bytes) = Postcard.encode(&c) {
        c.hash = protocol::xxh64(0, &bytes);
    }
    c
}

/// Base time and length of the turn for the timer of the settings: `Original { row }` takes the row
/// (`turn_len_s` = base, no additions), `Off` and `Fixed` have no row (game-host-mp 4.7).
pub fn timer_of(timer: &TimerMode) -> (u32, u32) {
    match timer {
        TimerMode::Original { row } => TIMERS
            .iter()
            .find(|t| t.0 == row)
            .map_or((0, 0), |t| (t.2, t.2)),
        TimerMode::Off | TimerMode::Fixed { .. } => (0, 0),
    }
}

// ---------------------------------------------------------------- majors

/// A major player of the test world.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Major {
    pub player: PlayerId,
    /// Index in the civilization table (the catalog id of the civ and of the leader).
    pub civ: usize,
    pub control: SlotControl,
    pub start: (i16, i16),
    /// Science stored towards the one art tech (`RESEARCH_COST`).
    pub research: i32,
    /// Bit i: has tech `Catalog::art_techs[i]`.
    pub art_techs: u64,
    /// Catalog id of the era of the player: 0 Ancient, except in the full set (`map::ERA_INDUSTRIAL`).
    pub era: i16,
    /// Gold paid for purchases in cities (the fixture's income is a function of the turn, this is taken off it).
    pub gold_spent: i32,
}

/// Science stored at the start, per turn, and the cost of the one tech of the test world.
pub const RESEARCH_START: i32 = 5;
pub const RESEARCH_PER_TURN: i32 = 2;
pub const RESEARCH_COST: i32 = 20;

fn bad_start(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorKind::BadStart, message)
}

/// The majors of a setup, ordered by player id, with their start sites.
///
/// # Errors
/// No slots, more than ten, more than eight humans, a repeated or unusable player id, an unknown civ.
pub fn majors_from_setup(setup: &GameSetup, geo: &Geo) -> Result<Vec<Major>, CoreError> {
    let mut slots: Vec<&SlotSetup> = setup.slots.iter().collect();
    slots.sort_by_key(|s| s.player);
    if slots.is_empty() || slots.len() > MAX_MAJORS {
        return Err(bad_start(format!(
            "fixture: {} slots, 1..={MAX_MAJORS} wanted",
            slots.len()
        )));
    }
    if slots.windows(2).any(|w| w[0].player == w[1].player) {
        return Err(bad_start("fixture: a player twice in the slots"));
    }
    if slots.iter().any(|s| !(0..BARBARIAN).contains(&s.player)) {
        return Err(bad_start("fixture: player id out of range"));
    }
    let humans = slots
        .iter()
        .filter(|s| matches!(s.control, SlotControl::Human { .. }))
        .count();
    if humans > MAX_HUMANS {
        return Err(bad_start(format!(
            "fixture: {humans} humans, at most {MAX_HUMANS}"
        )));
    }
    // Named civs first, then the random ones take the free civs in table order.
    let mut taken = BTreeSet::new();
    let mut civ_of: Vec<Option<usize>> = Vec::with_capacity(slots.len());
    for s in &slots {
        civ_of.push(match &s.civ {
            CivPick::Random => None,
            CivPick::Civ(name) => {
                let i = CIVS
                    .iter()
                    .position(|c| c.0 == name)
                    .ok_or_else(|| bad_start(format!("fixture: unknown civ {name}")))?;
                if !taken.insert(i) {
                    return Err(bad_start(format!("fixture: civ {name} twice")));
                }
                Some(i)
            }
        });
    }
    let mut free = (0..CIVS.len()).filter(|i| !taken.contains(i));
    let sites = map::start_sites(geo, slots.len());
    let mut out = Vec::with_capacity(slots.len());
    for ((s, civ), start) in slots.iter().zip(civ_of).zip(sites) {
        let civ = match civ {
            Some(c) => c,
            None => free
                .next()
                .ok_or_else(|| bad_start("fixture: no free civ"))?,
        };
        out.push(Major {
            player: s.player,
            civ,
            control: s.control.clone(),
            start,
            research: RESEARCH_START,
            art_techs: 0,
            era: 0,
            gold_spent: 0,
        });
    }
    Ok(out)
}

impl Major {
    pub fn is_human(&self) -> bool {
        matches!(self.control, SlotControl::Human { .. })
    }

    /// The slot as the worker reports it.
    pub fn slot(&self) -> SlotSetup {
        SlotSetup {
            player: self.player,
            civ: CivPick::Civ(CIVS[self.civ].0.into()),
            control: self.control.clone(),
        }
    }

    pub fn civ_type(&self) -> &'static str {
        CIVS[self.civ].0
    }

    pub fn civ_name(&self) -> &'static str {
        CIVS[self.civ].1
    }

    /// The tech of the test world is known.
    pub fn has_tech(&self) -> bool {
        self.art_techs & 1 != 0
    }
}

#[allow(clippy::too_many_arguments)]
fn unit(
    id: i32,
    owner: PlayerId,
    unit_type: i16,
    (x, y): (i16, i16),
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
        hp: 10,
        max_hp: 10,
        moves_left: max_moves,
        max_moves,
        strength,
        flags: UnitFlags(
            flags
                | if max_moves > 0 {
                    UnitFlags::CAN_MOVE
                } else {
                    0
                },
        ),
        name: name.into(),
        facing: 0,
        cargo_of: -1,
        building: -1,
        ..UnitView::default()
    }
}

/// A unit built in a city: stats by kind. Navy is not built.
pub fn spawn_unit(id: i32, owner: PlayerId, unit_type: i16, at: (i16, i16)) -> UnitView {
    let civ = UnitFlags::CIVILIAN;
    let (moves, strength, flags, name) = match unit_type {
        U_SETTLER => (120, 0, civ, "Settler"),
        U_WORKER => (120, 0, civ, "Worker"),
        U_ARCHER => (120, 5, 0, "Archer"),
        U_HORSEMAN => (240, 12, 0, "Horseman"),
        U_SCOUT => (240, 0, 0, "Scout"),
        _ => (120, 8, 0, "Warrior"),
    };
    let mut u = unit(id, owner, unit_type, at, moves, strength, flags, name);
    if unit_type == U_ARCHER {
        u.ranged = 7;
        u.range = 2;
    }
    u
}

/// The capital and the first units of a major: a garrisoned warrior in the city, a settler, a worker, an
/// archer and a scout around it, and a horseman for even player ids. Ids start at 1 for each player.
pub fn start_position(major: &Major, geo: &Geo) -> (CityView, Vec<UnitView>) {
    let p = major.player;
    let city = CityView {
        id: 1,
        owner: p,
        x: major.start.0,
        y: major.start.1,
        name: CIVS[major.civ].3.into(),
        population: 3,
        flags: CityFlags(CityFlags::CAPITAL),
        hp: 20,
        max_hp: 20,
        defense: 800,
        production_order: 0,
        production_item: U_WARRIOR,
        production_turns: 3,
        production_stored: 12,
        production_needed: 20,
        growth_turns: 5,
        yields: [4, 3, 2, 1, 1, 0],
        buildings: Vec::new(),
        founder: p,
    };
    let ring = |dir: u8| geo.neighbor(major.start, dir).unwrap_or(major.start);
    let civ = UnitFlags::CIVILIAN | UnitFlags::NEEDS_ORDERS;
    let mut units = vec![
        unit(
            1,
            p,
            U_WARRIOR,
            major.start,
            120,
            8,
            UnitFlags::GARRISONED | UnitFlags::FORTIFIED,
            "Warrior",
        ),
        unit(2, p, U_SETTLER, ring(0), 120, 0, civ, "Settler"),
        unit(3, p, U_WORKER, ring(1), 120, 0, civ, "Worker"),
        unit(
            4,
            p,
            U_ARCHER,
            ring(2),
            120,
            5,
            UnitFlags::NEEDS_ORDERS,
            "Archer",
        ),
        unit(
            5,
            p,
            U_SCOUT,
            ring(3),
            240,
            0,
            UnitFlags::NEEDS_ORDERS,
            "Scout",
        ),
    ];
    units[3].ranged = 7;
    units[3].range = 2;
    if p % 2 == 0 {
        units.push(unit(
            6,
            p,
            U_HORSEMAN,
            ring(4),
            240,
            12,
            UnitFlags::NEEDS_ORDERS,
            "Horseman",
        ));
    }
    (city, units)
}

/// The barbarians: a warrior in each camp. Ids 1.. of the player `BARBARIAN`.
pub fn barbarian_units(geo: &Geo) -> Vec<UnitView> {
    let sp = map::specials(geo);
    sp.camps
        .iter()
        .enumerate()
        .map(|(i, at)| {
            unit(
                i as i32 + 1,
                BARBARIAN,
                U_WARRIOR,
                *at,
                120,
                6,
                0,
                "Barbarian Warrior",
            )
        })
        .collect()
}

// ---------------------------------------------------------------- diplomacy and player records

/// Wars and meetings: who is at war with whom, and which teams have met which players.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diplomacy {
    wars: BTreeSet<(PlayerId, PlayerId)>,
    met: BTreeSet<(TeamId, PlayerId)>,
}

impl Diplomacy {
    pub fn at_war(&self, a: PlayerId, b: PlayerId) -> bool {
        if a == b {
            return false;
        }
        a == BARBARIAN || b == BARBARIAN || self.wars.contains(&(a.min(b), a.max(b)))
    }

    pub fn declare_war(&mut self, a: PlayerId, b: PlayerId) {
        if a != b {
            self.wars.insert((a.min(b), a.max(b)));
        }
    }

    pub fn make_peace(&mut self, a: PlayerId, b: PlayerId) {
        self.wars.remove(&(a.min(b), a.max(b)));
    }

    pub fn has_met(&self, team: TeamId, player: PlayerId) -> bool {
        self.met.contains(&(team, player))
    }

    /// The team saw a unit or a city of the player. Returns true when this is news.
    pub fn meet(&mut self, team: TeamId, player: PlayerId) -> bool {
        team != player && self.met.insert((team, player))
    }
}

/// The player records a team sees (`Core::players`): every major and the barbarians; a player the team
/// has not met has no era and no art techs.
pub fn player_views(majors: &[Major], dip: &Diplomacy, viewer: TeamId) -> Vec<PlayerView> {
    let mut out: Vec<PlayerView> = majors
        .iter()
        .map(|m| {
            let me = m.player == viewer;
            let met = me || dip.has_met(viewer, m.player);
            let mut flags = PlayerFlags::ALIVE;
            if m.is_human() {
                flags |= PlayerFlags::HUMAN;
            }
            if met && !me {
                flags |= PlayerFlags::MET;
            }
            if dip.at_war(viewer, m.player) {
                flags |= PlayerFlags::AT_WAR_WITH_VIEWER;
            }
            let civ = &CIVS[m.civ];
            PlayerView {
                id: m.player,
                team: m.player,
                civ_type: m.civ as i16,
                leader_type: m.civ as i16,
                flags: PlayerFlags(flags),
                color_primary: civ.4,
                color_secondary: civ.5,
                name: match &m.control {
                    SlotControl::Human { nick } => nick.clone(),
                    SlotControl::Ai => civ.2.into(),
                },
                civ_name: civ.1.into(),
                era: if met { m.era } else { -1 },
                art_techs: if met { m.art_techs } else { 0 },
            }
        })
        .collect();
    let mut flags = PlayerFlags::ALIVE | PlayerFlags::BARBARIAN;
    if dip.has_met(viewer, BARBARIAN) {
        flags |= PlayerFlags::MET;
    }
    if viewer != BARBARIAN {
        flags |= PlayerFlags::AT_WAR_WITH_VIEWER;
    }
    out.push(PlayerView {
        id: BARBARIAN,
        team: BARBARIAN,
        civ_type: -1,
        leader_type: -1,
        flags: PlayerFlags(flags),
        color_primary: 0x40_40_40_FF,
        color_secondary: 0xB0_B0_B0_FF,
        name: "Barbarians".into(),
        civ_name: "Barbarians".into(),
        era: if dip.has_met(viewer, BARBARIAN) {
            0
        } else {
            -1
        },
        art_techs: 0,
    });
    out.sort_by_key(|p| p.id);
    out
}

/// Gold of a player at `turn`: 100 and 3 a turn, less what the city purchases took.
pub fn gold_at(turn: i32, major: &Major) -> i32 {
    100 + turn * 3 - major.gold_spent
}

/// The top bar of a player at `turn`: the one tech of the test world is researched in a few turns.
pub fn top_bar(turn: i32, major: &Major) -> TopBar {
    let left = (RESEARCH_COST - major.research).max(0);
    TopBar {
        gold: gold_at(turn, major),
        gold_per_turn: 3,
        science_per_turn: RESEARCH_PER_TURN,
        research_tech: if major.has_tech() { -1 } else { 0 },
        research_turns: if major.has_tech() {
            0
        } else {
            ((left + RESEARCH_PER_TURN - 1) / RESEARCH_PER_TURN) as i16
        },
        research_progress: major.research,
        research_cost: RESEARCH_COST,
        culture: turn,
        culture_per_turn: 1,
        next_policy_cost: 25,
        happiness: 5,
        ..TopBar::default()
    }
}

/// Setups for the tests of this module and of `fixture`.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use protocol::{GameSettings, TurnMode};

    pub(crate) fn settings() -> GameSettings {
        GameSettings {
            name: "fixture".into(),
            world_size: "WORLDSIZE_DUEL".into(),
            map_script: "fixture.lua".into(),
            map_options: Vec::new(),
            speed: "GAMESPEED_STANDARD".into(),
            start_era: "ERA_ANCIENT".into(),
            handicap: "HANDICAP_PRINCE".into(),
            minor_civs: 0,
            turn_mode: TurnMode::Hybrid,
            timer: TimerMode::Off,
            game_options: Vec::new(),
            seed: Some(1),
            map_seed: None,
            auto_choices: protocol::AutoChoices::default(),
            diplo_auto_decline: true,
            mods: Vec::new(),
        }
    }

    pub(crate) fn setup(humans: usize, ais: usize) -> GameSetup {
        let mut slots = Vec::new();
        for i in 0..humans {
            slots.push(SlotSetup {
                player: i as i8,
                civ: CivPick::Random,
                control: SlotControl::Human {
                    nick: format!("p{i}"),
                },
            });
        }
        for i in 0..ais {
            slots.push(SlotSetup {
                player: (humans + i) as i8,
                civ: CivPick::Random,
                control: SlotControl::Ai,
            });
        }
        GameSetup {
            settings: settings(),
            slots,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::setup;
    use super::*;

    #[test]
    fn majors_get_sites_and_distinct_civs() {
        let geo = Geo::new(DUEL.0, DUEL.1, false);
        for (h, a) in [(1, 1), (2, 0), (4, 2), (8, 2)] {
            let majors = majors_from_setup(&setup(h, a), &geo).unwrap();
            assert_eq!(majors.len(), h + a);
            let civs: BTreeSet<_> = majors.iter().map(|m| m.civ).collect();
            assert_eq!(civs.len(), majors.len());
            let starts: BTreeSet<_> = majors.iter().map(|m| m.start).collect();
            assert_eq!(starts.len(), majors.len());
        }
    }

    #[test]
    fn bad_setups_are_refused() {
        let geo = Geo::new(DUEL.0, DUEL.1, false);
        assert!(majors_from_setup(&setup(0, 0), &geo).is_err());
        assert!(majors_from_setup(&setup(9, 0), &geo).is_err());
        assert!(majors_from_setup(&setup(8, 3), &geo).is_err());
        let mut twice = setup(2, 0);
        twice.slots[1].player = 0;
        assert!(majors_from_setup(&twice, &geo).is_err());
        let mut unknown = setup(1, 0);
        unknown.slots[0].civ = CivPick::Civ("CIVILIZATION_ATLANTIS".into());
        let err = majors_from_setup(&unknown, &geo).unwrap_err();
        assert_eq!(err.kind, CoreErrorKind::BadStart);
    }

    #[test]
    fn start_positions_have_five_or_six_units() {
        let geo = Geo::new(DUEL.0, DUEL.1, false);
        let majors = majors_from_setup(&setup(4, 2), &geo).unwrap();
        for m in &majors {
            let (city, units) = start_position(m, &geo);
            assert!((5..=6).contains(&units.len()));
            assert_eq!((city.x, city.y), m.start);
            assert!(units.iter().all(|u| u.owner == m.player));
            assert!(units.iter().all(|u| geo.distance((u.x, u.y), m.start) <= 1));
            let ids: BTreeSet<_> = units.iter().map(|u| u.id).collect();
            assert_eq!(ids.len(), units.len());
        }
    }

    #[test]
    fn catalog_covers_the_tables() {
        let c = catalog();
        assert_eq!(c.terrains.len(), TERRAINS.len());
        assert_eq!(c.features.len(), FEATURES.len());
        assert_eq!(c.civs.len(), CIVS.len());
        assert_eq!(c.leaders.len(), CIVS.len());
        assert_eq!(c.art_techs, vec![0]);
        assert!(c.techs.len() > usize::from(c.art_techs[0] as u16));
    }

    #[test]
    fn choices_have_two_timer_rows_and_a_stable_hash() {
        let c = choices();
        assert_eq!(c.timers.len(), 2);
        assert_eq!(c.world_sizes.len(), 2);
        assert_ne!(c.hash, 0);
        assert_eq!(c, choices());
        assert_eq!(
            timer_of(&TimerMode::Original {
                row: "TURNTIMER_SLOW".into()
            }),
            (120, 120)
        );
        assert_eq!(
            timer_of(&TimerMode::Original {
                row: "TURNTIMER_FAST".into()
            }),
            (30, 30)
        );
        assert_eq!(timer_of(&TimerMode::Off), (0, 0));
    }

    #[test]
    fn unmet_players_are_hidden() {
        let geo = Geo::new(DUEL.0, DUEL.1, false);
        let majors = majors_from_setup(&setup(2, 1), &geo).unwrap();
        let mut dip = Diplomacy::default();
        let v = player_views(&majors, &dip, 0);
        let other = v.iter().find(|p| p.id == 1).unwrap();
        assert_eq!(other.era, -1);
        assert!(!other.flags.contains(PlayerFlags::MET));
        assert!(!other.flags.contains(PlayerFlags::AT_WAR_WITH_VIEWER));
        assert!(dip.meet(0, 1));
        assert!(!dip.meet(0, 1));
        let v = player_views(&majors, &dip, 0);
        assert!(
            v.iter()
                .find(|p| p.id == 1)
                .unwrap()
                .flags
                .contains(PlayerFlags::MET)
        );
        // The barbarians are always at war.
        assert!(
            v.iter()
                .find(|p| p.id == BARBARIAN)
                .unwrap()
                .flags
                .contains(PlayerFlags::AT_WAR_WITH_VIEWER)
        );
    }
}
