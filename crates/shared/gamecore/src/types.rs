//! Typed query results: `GameInfo`, `Plot`, `Player`, `CatalogEntry`.
//!
//! "No value" is `None` where the ABI uses -1 for an id; fields the ABI documents as plain
//! numbers (yields, blocker plot, `human_player`) keep the ABI value.

use gamecore_sys::ffi;

use crate::setup::GameType;

pub(crate) fn opt_i16(v: i16) -> Option<i16> {
    (v >= 0).then_some(v)
}

pub(crate) fn opt_u8(v: i8) -> Option<u8> {
    u8::try_from(v).ok()
}

/// Why the core refused a command or a slot operation (`gc_error.reason`, `GC_REASON_*`).
/// The gateway maps them to the protocol's reject codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// 1: the player is not a human of this game.
    NotHuman,
    /// 2: the player's turn is not active now (hybrid, someone else's turn).
    NotYourTurn,
    /// 3: the player already ended the turn.
    TurnEnded,
    /// 4: the AI is moving; the command may pass after it.
    AiRunning,
    /// 5: the unit or city is busy or dying.
    UnitBusy,
    /// 6: the target is off the map, unreachable or not an enemy.
    BadTarget,
    /// 7: the rules do not allow it.
    Rule,
    /// 8: a blocker holds the end of the turn.
    Blocked,
    /// 9: an undelivered command or a busy unit: `pump` first.
    PumpFirst,
    /// 10: allowed only at a safe point.
    NotSafePoint,
    /// 11: the slot operation does not fit the slot's state.
    SlotState,
    /// A code this reader does not know. 0 (`GC_REASON_NONE`, a refusal with text only) is
    /// `Unknown(0)` too.
    Unknown(i32),
}

impl RejectReason {
    /// Types a `gc_error.reason`; never fails.
    pub fn from_abi(v: i32) -> Self {
        match v {
            ffi::GC_REASON_NOT_HUMAN => RejectReason::NotHuman,
            ffi::GC_REASON_NOT_YOUR_TURN => RejectReason::NotYourTurn,
            ffi::GC_REASON_TURN_ENDED => RejectReason::TurnEnded,
            ffi::GC_REASON_AI_RUNNING => RejectReason::AiRunning,
            ffi::GC_REASON_UNIT_BUSY => RejectReason::UnitBusy,
            ffi::GC_REASON_BAD_TARGET => RejectReason::BadTarget,
            ffi::GC_REASON_RULE => RejectReason::Rule,
            ffi::GC_REASON_BLOCKED => RejectReason::Blocked,
            ffi::GC_REASON_PUMP_FIRST => RejectReason::PumpFirst,
            ffi::GC_REASON_NOT_SAFE_POINT => RejectReason::NotSafePoint,
            ffi::GC_REASON_SLOT_STATE => RejectReason::SlotState,
            other => RejectReason::Unknown(other),
        }
    }

    /// The `GC_REASON_*` number.
    pub fn abi(self) -> i32 {
        match self {
            RejectReason::NotHuman => ffi::GC_REASON_NOT_HUMAN,
            RejectReason::NotYourTurn => ffi::GC_REASON_NOT_YOUR_TURN,
            RejectReason::TurnEnded => ffi::GC_REASON_TURN_ENDED,
            RejectReason::AiRunning => ffi::GC_REASON_AI_RUNNING,
            RejectReason::UnitBusy => ffi::GC_REASON_UNIT_BUSY,
            RejectReason::BadTarget => ffi::GC_REASON_BAD_TARGET,
            RejectReason::Rule => ffi::GC_REASON_RULE,
            RejectReason::Blocked => ffi::GC_REASON_BLOCKED,
            RejectReason::PumpFirst => ffi::GC_REASON_PUMP_FIRST,
            RejectReason::NotSafePoint => ffi::GC_REASON_NOT_SAFE_POINT,
            RejectReason::SlotState => ffi::GC_REASON_SLOT_STATE,
            RejectReason::Unknown(v) => v,
        }
    }
}

/// How the humans of a game take their turns (`setup_json.turn_mode`; `Q_GAME.turn_mode` is 1 / 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TurnMode {
    /// All humans move at once after the AI.
    Simultaneous,
    /// The core decides each turn who moves at once; humans at war with each other wait their
    /// turn (N-D5). The default.
    #[default]
    Hybrid,
}

impl TurnMode {
    /// The `setup_json` value.
    pub fn name(self) -> &'static str {
        match self {
            TurnMode::Simultaneous => "simultaneous",
            TurnMode::Hybrid => "hybrid",
        }
    }

    /// The `Q_GAME.turn_mode` number (`GC_TURN_MODE_*`).
    pub fn abi(self) -> i32 {
        match self {
            TurnMode::Simultaneous => ffi::GC_TURN_MODE_SIMULTANEOUS,
            TurnMode::Hybrid => ffi::GC_TURN_MODE_HYBRID,
        }
    }
}

/// `status` of `Q_GAME` (`GC_GAME_STATUS_*`, equal to `GC_PUMP_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameStatus {
    WaitingHuman,
    AiRunning,
    GameOver,
    Stuck,
    /// A value this reader does not know.
    Other(i8),
}

impl GameStatus {
    /// The status of a `pump` result (`GC_PUMP_*` equal `GC_GAME_STATUS_*`).
    pub fn from_pump(r: &gamecore_sys::PumpResult) -> Self {
        Self::from_abi(i8::try_from(r.status).unwrap_or(i8::MAX))
    }

    pub(crate) fn from_abi(v: i8) -> Self {
        match i32::from(v) {
            ffi::GC_GAME_STATUS_WAITING_HUMAN => GameStatus::WaitingHuman,
            ffi::GC_GAME_STATUS_AI_RUNNING => GameStatus::AiRunning,
            ffi::GC_GAME_STATUS_GAME_OVER => GameStatus::GameOver,
            ffi::GC_GAME_STATUS_STUCK => GameStatus::Stuck,
            _ => GameStatus::Other(v),
        }
    }
}

/// `Q_GAME`: the game as a whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameInfo {
    pub turn: i32,
    /// Calendar year, negative = BC.
    pub year: i32,
    /// Unit id of the end-turn blocker's target, -1 = none.
    pub blocker_unit: i32,
    /// City id of the blocker's target, -1 = none.
    pub blocker_city: i32,
    /// Plot of the blocker's target, -1 = none.
    pub blocker_x: i16,
    pub blocker_y: i16,
    pub map_width: i16,
    pub map_height: i16,
    /// The active (human or observer) slot, -1 = none.
    pub human_player: i8,
    pub human_team: i8,
    /// Who is moving now, -1 = none.
    pub active_player: i8,
    pub status: GameStatus,
    /// `EndTurnBlockingTypes` of the human, -1 = none.
    pub blocker: i8,
    pub wrap_x: bool,
    pub wrap_y: bool,
    /// `HandicapTypes`.
    pub difficulty: i8,
    /// Winning team, -1 = none.
    pub winner: i8,
    /// `VictoryTypes`, -1 = none.
    pub victory_type: i8,
    /// The v2 fields below are zero / `Single` / `None` for a core without them (a `Q_GAME`
    /// record of 36 bytes).
    pub game_type: GameType,
    /// `None` for a single-player game.
    pub turn_mode: Option<TurnMode>,
    pub phase: Phase,
    /// Seated humans.
    pub humans: u8,
    /// Humans the turn waits for.
    pub humans_left: u8,
    /// A safe point: `save` and slot changes are allowed.
    pub safe_point: bool,
    /// Why the point is not safe; `None` at a safe point.
    pub unsafe_reason: Option<UnsafeReason>,
    /// The length of this turn's timer in seconds as the exe enforces it; 0 = no timer.
    pub turn_len_s: i32,
    /// The base time of the chosen turn-timer row; 0 = no row.
    pub turn_base_s: i32,
    pub map_seed: u32,
}

/// `phase` of `Q_GAME` (`GC_PHASE_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// The AI civs are done, the humans act.
    #[default]
    Humans,
    /// The AI civs are moving.
    Ai,
}

/// `unsafe_reason` of `Q_GAME` (`GC_UNSAFE_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsafeReason {
    /// 1: the AI is computing.
    AiRunning,
    /// 2: a command was given after `WAITING_HUMAN`.
    CommandGiven,
    /// 3: an AI request to a human is open.
    AiRequestOpen,
    /// 4: a human is in auto moves.
    Automoves,
    /// A code this reader does not know.
    Other(i8),
}

impl UnsafeReason {
    /// Types an `unsafe_reason`; `None` for 0 (`GC_UNSAFE_NONE`).
    pub fn from_abi(v: i8) -> Option<Self> {
        match i32::from(v) {
            ffi::GC_UNSAFE_NONE => None,
            ffi::GC_UNSAFE_AI_RUNNING => Some(UnsafeReason::AiRunning),
            ffi::GC_UNSAFE_COMMAND_GIVEN => Some(UnsafeReason::CommandGiven),
            ffi::GC_UNSAFE_AI_REQUEST_OPEN => Some(UnsafeReason::AiRequestOpen),
            ffi::GC_UNSAFE_AUTOMOVES => Some(UnsafeReason::Automoves),
            _ => Some(UnsafeReason::Other(v)),
        }
    }
}

impl GameInfo {
    /// Plot of the blocker's target.
    pub fn blocker_plot(&self) -> Option<(i16, i16)> {
        (self.blocker_x >= 0 && self.blocker_y >= 0).then_some((self.blocker_x, self.blocker_y))
    }

    /// `map_width * map_height`, the plot count of `Q_PLOTS`.
    pub fn plot_count(&self) -> usize {
        usize::try_from(self.map_width).unwrap_or(0) * usize::try_from(self.map_height).unwrap_or(0)
    }
}

/// `DirectionType` of an edge of a hex plot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    NorthEast = 0,
    East = 1,
    SouthEast = 2,
    SouthWest = 3,
    West = 4,
    NorthWest = 5,
}

impl Direction {
    pub const ALL: [Direction; 6] = [
        Direction::NorthEast,
        Direction::East,
        Direction::SouthEast,
        Direction::SouthWest,
        Direction::West,
        Direction::NorthWest,
    ];

    /// The `DirectionType` number.
    pub fn index(self) -> usize {
        self as usize
    }

    /// The opposite direction.
    pub fn opposite(self) -> Direction {
        Direction::ALL[(self.index() + 3) % 6]
    }
}

/// `PlotTypes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlotType {
    Mountain,
    Hills,
    Land,
    Ocean,
    Other(i8),
}

impl PlotType {
    pub(crate) fn from_abi(v: i8) -> Self {
        match i32::from(v) {
            ffi::GC_PLOT_TYPE_MOUNTAIN => PlotType::Mountain,
            ffi::GC_PLOT_TYPE_HILLS => PlotType::Hills,
            ffi::GC_PLOT_TYPE_LAND => PlotType::Land,
            ffi::GC_PLOT_TYPE_OCEAN => PlotType::Ocean,
            _ => PlotType::Other(v),
        }
    }
}

/// `flags` of a plot record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlotFlags(pub u16);

impl PlotFlags {
    fn has(self, bit: u16) -> bool {
        self.0 & bit != 0
    }

    /// A city works the plot (v2; set only on a plot the viewer sees now, of a team the viewer is on).
    pub fn worked(self) -> bool {
        self.has(ffi::GC_PLOT_F_WORKED)
    }

    /// The route is pillaged (v2; visible plots only).
    pub fn route_pillaged(self) -> bool {
        self.has(ffi::GC_PLOT_F_ROUTE_PILLAGED)
    }
}

/// `Q_PLOTS` record. An unrevealed plot has only `x`, `y` and zeroes: test `revealed()` before reading
/// `terrain` or `owner`. A revealed plot that is not visible has no live data (city, pillaged, yields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plot {
    pub x: i16,
    pub y: i16,
    pub flags: PlotFlags,
    pub plot_type: PlotType,
    /// `TerrainTypes`.
    pub terrain: i16,
    pub feature: Option<i16>,
    pub resource: Option<i16>,
    pub resource_qty: i8,
    /// Last seen.
    pub improvement: Option<i16>,
    /// Last seen.
    pub route: Option<i16>,
    /// Last seen player.
    pub owner: Option<u8>,
    /// Art style of the continent.
    pub continent_art: i8,
    /// Food, production, gold, science, culture, faith; visible plots only.
    pub yields: [i8; 6],
    /// `BuildTypes` of the builder working the plot (v2), `None` = none or a core without the field.
    pub build_type: Option<i16>,
    /// Progress of `build_type` in percent, 0..100 (v2); 0 when none.
    pub build_pct: u8,
}

impl Plot {
    pub fn revealed(&self) -> bool {
        self.flags.has(ffi::GC_PLOT_F_REVEALED)
    }

    pub fn visible(&self) -> bool {
        self.flags.has(ffi::GC_PLOT_F_VISIBLE)
    }

    pub fn has_city(&self) -> bool {
        self.flags.has(ffi::GC_PLOT_F_CITY)
    }

    pub fn lake(&self) -> bool {
        self.flags.has(ffi::GC_PLOT_F_LAKE)
    }

    pub fn coastal(&self) -> bool {
        self.flags.has(ffi::GC_PLOT_F_COASTAL)
    }

    pub fn pillaged(&self) -> bool {
        self.flags.has(ffi::GC_PLOT_F_PILLAGED)
    }

    pub fn natural_wonder(&self) -> bool {
        self.flags.has(ffi::GC_PLOT_F_NATURAL_WONDER)
    }

    /// River on the edge towards `dir`.
    pub fn river(&self, dir: Direction) -> bool {
        self.flags
            .has(1 << (ffi::GC_PLOT_F_RIVER_SHIFT + dir as u32))
    }

    /// Rivers by `DirectionType`: NE, E, SE, SW, W, NW.
    pub fn rivers(&self) -> [bool; 6] {
        Direction::ALL.map(|d| self.river(d))
    }
}

/// `flags` of a player record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlayerFlags(pub u16);

impl PlayerFlags {
    fn has(self, bit: u16) -> bool {
        self.0 & bit != 0
    }

    pub fn alive(self) -> bool {
        self.has(ffi::GC_PLAYER_F_ALIVE)
    }

    pub fn human(self) -> bool {
        self.has(ffi::GC_PLAYER_F_HUMAN)
    }

    pub fn minor(self) -> bool {
        self.has(ffi::GC_PLAYER_F_MINOR)
    }

    pub fn barbarian(self) -> bool {
        self.has(ffi::GC_PLAYER_F_BARBARIAN)
    }

    /// The viewer has met this player.
    pub fn met(self) -> bool {
        self.has(ffi::GC_PLAYER_F_MET)
    }

    pub fn at_war_with_viewer(self) -> bool {
        self.has(ffi::GC_PLAYER_F_AT_WAR_WITH_VIEWER)
    }

    /// The server's seat. The core does not list it among the players of a network game; a
    /// single-player game with an observer does.
    pub fn observer(self) -> bool {
        self.has(ffi::GC_PLAYER_F_OBSERVER)
    }
}

/// `Q_PLAYERS` record. For a player the viewer has not met `civ_type` and `leader_type` are `None`
/// and the names are "Unknown".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Player {
    pub id: i8,
    pub team: i8,
    pub civ_type: Option<i16>,
    pub leader_type: Option<i16>,
    pub flags: PlayerFlags,
    /// `0xRRGGBBAA`.
    pub color_primary: u32,
    pub color_secondary: u32,
    /// Leader or city-state name.
    pub name: String,
    /// Civilization short name.
    pub civ_name: String,
    /// `CvPlayer::GetCurrentEra`; `None` for a player the viewer has not met.
    pub era: Option<i16>,
    /// The human's nickname (v2); `None` for an AI or a core without the field.
    pub nick: Option<String>,
    /// `SlotStatus` of the core (`SS_*`, v2); -1 for a core without the field.
    pub slot: i8,
    /// Bit `i` = the player has the technology of row `i` of the landmark art list
    /// (`Q_CATALOG` art techs; v2). 0 for a core without the field.
    pub art_techs: u64,
}

impl Player {
    /// Has the technology of row `i` of the landmark art list (`art_techs`); `i` < 64.
    pub fn has_art_tech(&self, i: u32) -> bool {
        i < 64 && self.art_techs & (1 << i) != 0
    }
}

/// `flags` of a `Q_PLOT_BASIS` record (`GC_BASIS_F_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BasisFlags(pub u16);

impl BasisFlags {
    fn has(self, bit: u16) -> bool {
        self.0 & bit != 0
    }

    pub fn lake(self) -> bool {
        self.has(ffi::GC_BASIS_F_LAKE)
    }

    pub fn coastal(self) -> bool {
        self.has(ffi::GC_BASIS_F_COASTAL)
    }

    pub fn natural_wonder(self) -> bool {
        self.has(ffi::GC_BASIS_F_NATURAL_WONDER)
    }
}

/// `Q_PLOT_BASIS` record: the static terrain of one plot, revealed or not, natural wonders
/// included, no fog (NP-D19). No resources, owners, improvements, units or cities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlotBasis {
    /// `y * map_width + x`.
    pub index: u32,
    pub plot_type: PlotType,
    /// Art style of the continent.
    pub continent_art: u8,
    /// `TerrainTypes`.
    pub terrain: i16,
    /// Terrain feature that is part of the relief only (natural wonder, marsh, flood plains, ice).
    pub feature: Option<i16>,
    pub flags: BasisFlags,
    /// Bit `d` = a river on the edge towards `DirectionType` `d`.
    pub rivers: u8,
    /// `CvPlot::getRiver*FlowDirection` of the own E, SE, SW edges; -1 = none.
    pub river_flow: [i8; 3],
}

impl PlotBasis {
    /// River on the edge towards `dir`.
    pub fn river(&self, dir: Direction) -> bool {
        self.rivers & (1 << dir.index()) != 0
    }

    /// Rivers by `DirectionType`: NE, E, SE, SW, W, NW.
    pub fn river_edges(&self) -> [bool; 6] {
        Direction::ALL.map(|d| self.river(d))
    }
}

/// `flags` of a `Q_DIPLO` record (`GC_DIPLO_F_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiploFlags(pub u16);

impl DiploFlags {
    fn has(self, bit: u16) -> bool {
        self.0 & bit != 0
    }

    pub fn met(self) -> bool {
        self.has(ffi::GC_DIPLO_F_MET)
    }

    pub fn at_war(self) -> bool {
        self.has(ffi::GC_DIPLO_F_AT_WAR)
    }

    pub fn human(self) -> bool {
        self.has(ffi::GC_DIPLO_F_HUMAN)
    }

    pub fn can_declare_war(self) -> bool {
        self.has(ffi::GC_DIPLO_F_CAN_DECLARE_WAR)
    }

    pub fn can_make_peace(self) -> bool {
        self.has(ffi::GC_DIPLO_F_CAN_MAKE_PEACE)
    }
}

/// `Q_DIPLO` record: the asking player's relation to one other living major civ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiploRelation {
    pub other: i8,
    pub team: i8,
    pub flags: DiploFlags,
    /// Turns before peace is possible again; 0 = free.
    pub war_locked_turns: i16,
}

/// `flags` of a `Q_COMBAT_PREVIEW` record (`GC_COMBAT_F_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CombatFlags(pub u16);

impl CombatFlags {
    fn has(self, bit: u16) -> bool {
        self.0 & bit != 0
    }

    /// The attack kills the target.
    pub fn kills(self) -> bool {
        self.has(ffi::GC_COMBAT_F_KILLS)
    }

    /// The attack captures the target city.
    pub fn captures(self) -> bool {
        self.has(ffi::GC_COMBAT_F_CAPTURES)
    }

    /// The target is a city.
    pub fn city(self) -> bool {
        self.has(ffi::GC_COMBAT_F_CITY)
    }
}

/// `Q_COMBAT_PREVIEW` record: what an attack would do, without doing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CombatForecast {
    pub damage_to_target: i16,
    pub damage_to_attacker: i16,
    /// Combat strength x100.
    pub attacker_strength: i32,
    /// Combat strength x100.
    pub defender_strength: i32,
    pub target_hp: i16,
    pub target_max_hp: i16,
    pub flags: CombatFlags,
}

/// A table of `Q_CATALOG` (`GC_CATALOG_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum CatalogTable {
    Units = ffi::GC_CATALOG_UNITS,
    Buildings = ffi::GC_CATALOG_BUILDINGS,
    Projects = ffi::GC_CATALOG_PROJECTS,
    Technologies = ffi::GC_CATALOG_TECHNOLOGIES,
    Policies = ffi::GC_CATALOG_POLICIES,
    Terrains = ffi::GC_CATALOG_TERRAINS,
    Features = ffi::GC_CATALOG_FEATURES,
    Resources = ffi::GC_CATALOG_RESOURCES,
    Improvements = ffi::GC_CATALOG_IMPROVEMENTS,
    Routes = ffi::GC_CATALOG_ROUTES,
    Civilizations = ffi::GC_CATALOG_CIVILIZATIONS,
    Leaders = ffi::GC_CATALOG_LEADERS,
    Eras = ffi::GC_CATALOG_ERAS,
    Handicaps = ffi::GC_CATALOG_HANDICAPS,
    Promotions = ffi::GC_CATALOG_PROMOTIONS,
    /// v2: the landmark art technologies, in the bit order of `Player::art_techs`.
    ArtTechs = ffi::GC_CATALOG_ART_TECHS,
    /// v2: the `GameSpeeds` rows (only id, type and name are set).
    GameSpeeds = ffi::GC_CATALOG_GAME_SPEEDS,
    /// v5 (UI v2): the policy branches, ids as in `Q_POLICY_BRANCHES` (only id, type and name are set).
    PolicyBranches = ffi::GC_CATALOG_POLICY_BRANCHES,
}

impl CatalogTable {
    /// The tables of v1, in `GC_CATALOG_*` order (the art techs, a v2 table, are not here: a core
    /// of the first version does not serve them).
    pub const ALL: [CatalogTable; 15] = [
        CatalogTable::Units,
        CatalogTable::Buildings,
        CatalogTable::Projects,
        CatalogTable::Technologies,
        CatalogTable::Policies,
        CatalogTable::Terrains,
        CatalogTable::Features,
        CatalogTable::Resources,
        CatalogTable::Improvements,
        CatalogTable::Routes,
        CatalogTable::Civilizations,
        CatalogTable::Leaders,
        CatalogTable::Eras,
        CatalogTable::Handicaps,
        CatalogTable::Promotions,
    ];

    /// The `GC_CATALOG_*` number.
    pub fn abi(self) -> i32 {
        self as i32
    }
}

/// `Q_CATALOG` record: static reference data. `id` is the core's type index, the number every other
/// query uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub id: i16,
    pub icon_index: Option<i16>,
    pub flag_index: Option<i16>,
    /// `"UNIT_WARRIOR"`.
    pub type_name: String,
    /// Localized (en_US).
    pub name: String,
    pub art_define: Option<String>,
    pub icon_atlas: Option<String>,
    pub flag_atlas: Option<String>,
    pub art_style: Option<String>,
    /// Always `None` for now (no such column in the game data).
    pub strategic_view: Option<String>,
}

/// `Q_CATALOG` record of the Worlds table (v2, `GC_CATALOG_WORLD_SIZES`): a map size of the game data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldSizeEntry {
    pub id: i16,
    /// `"WORLDSIZE_DUEL"`.
    pub type_name: String,
    /// Localized (en_US).
    pub name: String,
    pub width: i16,
    pub height: i16,
    pub default_players: i16,
    pub default_minor: i16,
}

/// `Q_CATALOG` record of the TurnTimers table (v2, `GC_CATALOG_TURN_TIMERS`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimerEntry {
    pub id: i16,
    /// `"TURNTIMER_DYNAMIC"`.
    pub type_name: String,
    /// Localized (en_US).
    pub name: String,
    /// `CvTurnTimerInfo::getBaseTime()`.
    pub base_time: i32,
    pub city_resource: i32,
    pub unit_resource: i32,
}

/// `Q_CATALOG` record of the GameOptions table (v3, `GC_CATALOG_GAME_OPTIONS`): a row of the game options of the
/// staging room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameOptionEntry {
    pub id: i32,
    /// `"GAMEOPTION_NO_BARBARIANS"`.
    pub type_name: String,
    /// Localized (en_US) `Description`.
    pub name: String,
    /// Localized (en_US) `Help`; may be empty.
    pub help: String,
    /// `Default`.
    pub default: bool,
    /// `Visible`.
    pub visible: bool,
    /// `SupportsMultiplayer`.
    pub supports_multiplayer: bool,
}

/// A `Q_CATALOG` row that has an id, a type and a name only (v3: the Victories and MinorCivilizations tables).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedEntry {
    pub id: i32,
    /// `"VICTORY_TIME"`, `"MINOR_CIV_ALMATY"`.
    pub type_name: String,
    /// Localized (en_US).
    pub name: String,
}

/// `Q_CATALOG` record of the MapScriptOptions table (v3, `GC_CATALOG_MAP_OPTIONS`): one custom option of a map
/// script, as the game stored it from `GetMapScriptInfo().CustomOptions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapOptionEntry {
    /// `MapScriptOptions.FileName`, as the table spells it (backslashes): compare with [`same_script`].
    pub script: String,
    /// The option id (`"1"`, `"2"` ...): the key of `Setup::map_options` and of `Map.GetCustomOption`.
    pub option_id: String,
    /// Localized (en_US).
    pub name: String,
    /// Localized (en_US) `Description`; may be empty.
    pub help: String,
    pub sort_priority: i32,
    pub default_value: i32,
    pub read_only: bool,
    pub hidden: bool,
}

/// `Q_CATALOG` record of the MapScriptOptionPossibleValues table (v3, `GC_CATALOG_MAP_OPTION_VALUES`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapOptionValueEntry {
    pub script: String,
    pub option_id: String,
    /// Localized (en_US).
    pub name: String,
    /// The value `Setup::map_options` carries.
    pub value: i32,
    pub sort_index: i32,
}

/// What the staging room shows when the mouse rests on a civilization (v4, `GC_CATALOG_CIV_INFO`): one row of the game data.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CivInfo {
    pub type_name: String,
    pub leader: String,
    /// One `"Name: description"` per trait of the leader.
    pub traits: Vec<String>,
    /// One `"Unit: name"` or `"Building: name"` per unique unit or building.
    pub unique: Vec<String>,
}

/// The staging room options of the game data (v3): what `Game::describe_setup` returns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SetupDescription {
    pub game_options: Vec<GameOptionEntry>,
    pub victories: Vec<NamedEntry>,
    pub minor_civs: Vec<NamedEntry>,
    pub map_options: Vec<MapOptionEntry>,
    pub map_option_values: Vec<MapOptionValueEntry>,
}

/// Do two map script names (a `MapScriptOptions.FileName` with backslashes, a VFS path with slashes, any case) name
/// the same file?
pub fn same_script(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.replace('\\', "/").to_lowercase();
    norm(a) == norm(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_names_compare_by_path_not_by_spelling() {
        assert!(same_script(
            "Assets\\Maps\\Lekmap v5.2\\LekmapPangaeaFractalv5.2.lua",
            "assets/maps/lekmap v5.2/lekmappangaeafractalv5.2.lua"
        ));
        assert!(!same_script("Assets\\Maps\\A.lua", "Assets\\Maps\\B.lua"));
    }

    #[test]
    fn reject_reasons_round_trip_the_header_codes() {
        let table = [
            (1, RejectReason::NotHuman),
            (2, RejectReason::NotYourTurn),
            (3, RejectReason::TurnEnded),
            (4, RejectReason::AiRunning),
            (5, RejectReason::UnitBusy),
            (6, RejectReason::BadTarget),
            (7, RejectReason::Rule),
            (8, RejectReason::Blocked),
            (9, RejectReason::PumpFirst),
            (10, RejectReason::NotSafePoint),
            (11, RejectReason::SlotState),
        ];
        for (code, want) in table {
            assert_eq!(RejectReason::from_abi(code), want, "{code}");
            assert_eq!(want.abi(), code);
        }
        // No reason given, and codes from a newer core.
        assert_eq!(RejectReason::from_abi(0), RejectReason::Unknown(0));
        assert_eq!(RejectReason::from_abi(12), RejectReason::Unknown(12));
        assert_eq!(RejectReason::Unknown(-4).abi(), -4);
    }

    #[test]
    fn turn_mode_names_and_numbers() {
        assert_eq!(TurnMode::default(), TurnMode::Hybrid);
        assert_eq!(
            (TurnMode::Simultaneous.name(), TurnMode::Simultaneous.abi()),
            ("simultaneous", 1)
        );
        assert_eq!(
            (TurnMode::Hybrid.name(), TurnMode::Hybrid.abi()),
            ("hybrid", 2)
        );
    }
}
