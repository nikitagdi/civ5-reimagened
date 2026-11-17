//! HUD input records, independent of a local game engine.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderType {
    /// Item = `UnitTypes`.
    Train,
    /// Item = `BuildingTypes` (wonders too).
    Construct,
    /// Item = `ProjectTypes`.
    Create,
    /// Item = `ProcessTypes` (set production only).
    Maintain,
    /// A value this reader does not know.
    Other(i8),
}

impl OrderType {
    pub fn from_abi(v: i8) -> Self {
        match i32::from(v) {
            0 => OrderType::Train,
            1 => OrderType::Construct,
            2 => OrderType::Create,
            4 => OrderType::Maintain,
            _ => OrderType::Other(v),
        }
    }

    /// The `GC_ORDER_*` number.
    pub fn abi(self) -> i32 {
        match self {
            OrderType::Train => 0,
            OrderType::Construct => 1,
            OrderType::Create => 2,
            OrderType::Maintain => 4,
            OrderType::Other(v) => i32::from(v),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    Awake,
    Hold,
    Sleep,
    Heal,
    Sentry,
    Intercept,
    Mission,
    /// A value this reader does not know.
    Other(i8),
}

impl Activity {
    pub fn abi(self) -> i8 {
        match self {
            Self::Awake => 0,
            Self::Hold => 1,
            Self::Sleep => 2,
            Self::Heal => 3,
            Self::Sentry => 4,
            Self::Intercept => 5,
            Self::Mission => 6,
            Self::Other(v) => v,
        }
    }

    pub fn from_abi(v: i8) -> Self {
        match v {
            0 => Activity::Awake,
            1 => Activity::Hold,
            2 => Activity::Sleep,
            3 => Activity::Heal,
            4 => Activity::Sentry,
            5 => Activity::Intercept,
            6 => Activity::Mission,
            _ => Activity::Other(v),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UnitFlags(pub u16);

impl UnitFlags {
    fn has(self, bit: u16) -> bool {
        self.0 & bit != 0
    }

    /// Moves left and not immobile.
    pub fn can_move(self) -> bool {
        self.has(0x0001)
    }

    /// Awake, no orders queued, not automated: what the UNITS end-turn blocker waits for.
    pub fn needs_orders(self) -> bool {
        self.has(0x0002)
    }

    pub fn fortified(self) -> bool {
        self.has(0x0004)
    }

    /// Sleeping and not fortified.
    pub fn sleeping(self) -> bool {
        self.has(0x0008)
    }

    pub fn automated(self) -> bool {
        self.has(0x0010)
    }

    pub fn embarked(self) -> bool {
        self.has(0x0020)
    }

    pub fn civilian(self) -> bool {
        self.has(0x0040)
    }

    pub fn can_promote(self) -> bool {
        self.has(0x0080)
    }

    pub fn garrisoned(self) -> bool {
        self.has(0x0100)
    }

    /// Carried by a transport (v2; `transport_id` says which).
    pub fn cargo(self) -> bool {
        self.has(0x0200)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    /// Unique inside the owner.
    pub id: i32,
    pub owner: i8,
    /// `UnitTypes`.
    pub unit_type: i16,
    pub x: i16,
    pub y: i16,
    pub hp: i16,
    pub max_hp: i16,
    pub moves_left: i16,
    pub max_moves: i16,
    pub strength: i16,
    pub ranged: i16,
    pub range: i8,
    /// `DomainTypes`.
    pub domain: i8,
    pub activity: Activity,
    pub level: i8,
    pub experience: i16,
    pub flags: UnitFlags,
    /// Id (inside the owner) of the transport that carries the unit (v2); `None` = not carried or a
    /// core without the field.
    pub transport_id: Option<i32>,
    /// `BuildTypes` of the builder (v2), `None` = none.
    pub build_type: Option<i16>,
    /// Facing (v2), `None` = none or a value outside `DirectionTypes`.
    pub facing: Option<Direction>,
    /// Localized name (a custom name when the unit has one).
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitAction {
    /// `GC_CMD_UNIT_*`, or `GC_UNIT_ROW_*` for an extra row.
    pub action: i16,
    /// Can be given now (the target is not checked).
    pub enabled: bool,
    /// Why not, when `enabled` is false and the core says.
    pub reason: Option<String>,
    /// Gold the row costs now (the upgrade price); `None` = no cost, or a core without the field.
    pub cost: Option<i32>,
}

impl UnitAction {
    /// The extra row this record is, `None` for a unit command.
    pub fn row(&self) -> Option<UnitRow> {
        UnitRow::from_abi(self.action)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitRow {
    /// Put the unit on alert (`MISSION_ALERT`).
    Alert,
    /// Pillage the plot (`MISSION_PILLAGE`).
    Pillage,
    /// Upgrade the unit; the record's `cost` is the price.
    Upgrade,
}

impl UnitRow {
    pub fn from_abi(v: i16) -> Option<Self> {
        match v {
            101 => Some(UnitRow::Alert),
            102 => Some(UnitRow::Pillage),
            103 => Some(UnitRow::Upgrade),
            _ => None,
        }
    }

    /// The `GC_UNIT_ROW_*` number.
    pub fn abi(self) -> i16 {
        match self {
            UnitRow::Alert => 101,
            UnitRow::Pillage => 102,
            UnitRow::Upgrade => 103,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub id: i32,
    /// `NotificationTypes` hash.
    pub kind: u32,
    /// -1 when the notification has no plot.
    pub x: i16,
    pub y: i16,
    pub data1: i32,
    pub data2: i32,
    /// Game turn it was added.
    pub turn: i16,
    pub dismissed: bool,
    pub message: String,
    pub summary: String,
}

impl Notification {
    /// The plot it points to.
    pub fn plot(&self) -> Option<(i16, i16)> {
        (self.x >= 0 && self.y >= 0).then_some((self.x, self.y))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CityFlags(pub u16);

impl CityFlags {
    fn has(self, bit: u16) -> bool {
        self.0 & bit != 0
    }

    pub fn capital(self) -> bool {
        self.has(0x0001)
    }

    /// Own cities only.
    pub fn puppet(self) -> bool {
        self.has(0x0002)
    }

    pub fn razing(self) -> bool {
        self.has(0x0004)
    }

    /// Own cities only: captured, not yet pacified by a courthouse.
    pub fn occupied(self) -> bool {
        self.has(0x0008)
    }

    /// Own cities only: can shoot at a target in range right now.
    pub fn can_range_attack(self) -> bool {
        self.has(0x0010)
    }

    /// Live data; clear = last seen (fogged).
    pub fn visible(self) -> bool {
        self.has(0x0020)
    }

    /// Own cities only.
    pub fn resistance(self) -> bool {
        self.has(0x0040)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct City {
    /// Unique inside the owner.
    pub id: i32,
    pub owner: i8,
    /// Head of the order queue, `None` = nothing.
    pub production_order: Option<OrderType>,
    pub x: i16,
    pub y: i16,
    pub population: i16,
    pub flags: CityFlags,
    pub hp: i16,
    pub max_hp: i16,
    /// Combat strength (strength value / 100).
    pub defense: i16,
    /// Item of the head order, `None` = nothing.
    pub production_item: Option<i16>,
    /// `None` = never (no surplus).
    pub production_turns: Option<i16>,
    /// `None` = never (no surplus).
    pub growth_turns: Option<i16>,
    pub production_stored: i32,
    pub production_needed: i32,
    /// Per turn: food, production, gold, science, culture, faith.
    pub yields: [i16; 6],
    /// Founder (v2): the player the city was founded by (`CvCity::getOriginalOwner`); `None` =
    /// unknown or a core without the field.
    pub founder: Option<u8>,
    /// Production carried over into the next item (`CvCity::getOverflowProduction`); own cities only,
    /// `None` for the others and for a core without the field.
    pub overflow: Option<i32>,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TopBarFlags(pub u16);

impl TopBarFlags {
    fn has(self, bit: u16) -> bool {
        self.0 & bit != 0
    }

    pub fn unhappy(self) -> bool {
        self.has(0x0001)
    }

    pub fn very_unhappy(self) -> bool {
        self.has(0x0002)
    }

    pub fn golden_age(self) -> bool {
        self.has(0x0004)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopBar {
    pub gold: i32,
    /// Net rate.
    pub gold_per_turn: i32,
    /// Net rate.
    pub science_per_turn: i32,
    /// `TechTypes`, `None` = none chosen (turns, progress and cost are then -1 / 0 / 0).
    pub research_tech: Option<i16>,
    pub research_turns: i16,
    pub research_progress: i32,
    pub research_cost: i32,
    pub culture: i32,
    pub culture_per_turn: i32,
    pub next_policy_cost: i32,
    /// Excess happiness, negative = unhappy.
    pub happiness: i16,
    /// Turns left of a running golden age, else 0.
    pub golden_age_turns: i16,
    pub faith: i32,
    pub faith_per_turn: i32,
    /// `EraTypes`.
    pub era: i16,
    pub flags: TopBarFlags,
    pub golden_age_progress: i32,
    pub golden_age_threshold: i32,
    /// The empire's tourism per turn; `None` for a core without the field.
    pub tourism: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BuildFlags(pub u16);

impl BuildFlags {
    fn has(self, bit: u16) -> bool {
        self.0 & bit != 0
    }

    /// Wonder, national wonder or other limited building.
    pub fn wonder(self) -> bool {
        self.has(0x0001)
    }

    /// The head of the order queue.
    pub fn current(self) -> bool {
        self.has(0x0002)
    }

    /// A `CityPurchase` with gold would be accepted now.
    pub fn can_purchase(self) -> bool {
        self.has(0x0004)
    }

    /// Can be started now.
    pub fn enabled(self) -> bool {
        self.has(0x0008)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CityBuild {
    pub order: OrderType,
    pub item: i16,
    /// Turns to finish at the current production; `None` = never (and for processes).
    pub turns: Option<i16>,
    pub flags: BuildFlags,
    /// 0 for processes.
    pub production_cost: i32,
    /// Cost to buy now with gold; `None` = cannot be bought here.
    pub gold_cost: Option<i32>,
    /// Cost to buy now with faith; `None` = cannot be bought here.
    pub faith_cost: Option<i32>,
    /// Why it is disabled (the core's own text); `None` when enabled.
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CityOrder {
    /// Position in the queue, 0 = what the city builds now.
    pub slot: i32,
    /// `OrderType::Other(3)` is a specialist order of the core (never given through the ABI).
    pub order: OrderType,
    pub item: i16,
    /// Turns to finish at the current production, as the city screen shows them for this slot;
    /// `None` = never (no production) and for processes.
    pub turns: Option<i16>,
    /// Production the item costs, 0 for processes.
    pub needed: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TechStatus {
    /// The team has it.
    Have,
    /// Can be researched now.
    CanResearch,
    /// Prerequisites missing, or disabled.
    Locked,
    /// A value this reader does not know.
    Other(u8),
}

impl TechStatus {
    pub fn from_abi(v: u8) -> Self {
        match v {
            0 => TechStatus::Have,
            1 => TechStatus::CanResearch,
            2 => TechStatus::Locked,
            _ => TechStatus::Other(v),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TechState {
    /// `TechTypes`.
    pub tech: i16,
    pub status: TechStatus,
    /// Position in the research queue: `Some(0)` = the current research, `None` = not queued.
    pub queue: Option<u8>,
    /// Turns to research at the current science; `None` = never (no science) or a technology the team has.
    pub turns: Option<i16>,
    pub progress: i32,
    pub cost: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceUsage {
    Bonus,
    Strategic,
    Luxury,
    /// A value this reader does not know.
    Other(u8),
}

impl ResourceUsage {
    pub fn from_abi(v: u8) -> Self {
        match v {
            0 => ResourceUsage::Bonus,
            1 => ResourceUsage::Strategic,
            2 => ResourceUsage::Luxury,
            _ => ResourceUsage::Other(v),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerResource {
    /// `ResourceTypes`.
    pub resource: i16,
    pub usage: ResourceUsage,
    /// `total - used`.
    pub available: i32,
    pub used: i32,
    /// Imports, city-states and siphons included, exports taken off.
    pub total: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyBranchState {
    /// `PolicyBranchTypes`.
    pub branch: i16,
    pub unlocked: bool,
    /// Policies of the branch the player has.
    pub owned: u8,
    /// Policies of the branch in the game data.
    pub total: u8,
}

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
            TurnMode::Simultaneous => 1,
            TurnMode::Hybrid => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameStatus {
    WaitingHuman,
    AiRunning,
    GameOver,
    Stuck,
    /// A value this reader does not know.
    Other(i8),
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// The AI civs are done, the humans act.
    #[default]
    Humans,
    /// The AI civs are moving.
    Ai,
}

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
            0 => None,
            1 => Some(UnsafeReason::AiRunning),
            2 => Some(UnsafeReason::CommandGiven),
            3 => Some(UnsafeReason::AiRequestOpen),
            4 => Some(UnsafeReason::Automoves),
            _ => Some(UnsafeReason::Other(v)),
        }
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlayerFlags(pub u16);

impl PlayerFlags {
    fn has(self, bit: u16) -> bool {
        self.0 & bit != 0
    }

    pub fn alive(self) -> bool {
        self.has(0x0001)
    }

    pub fn human(self) -> bool {
        self.has(0x0002)
    }

    pub fn minor(self) -> bool {
        self.has(0x0004)
    }

    pub fn barbarian(self) -> bool {
        self.has(0x0008)
    }

    /// The viewer has met this player.
    pub fn met(self) -> bool {
        self.has(0x0010)
    }

    pub fn at_war_with_viewer(self) -> bool {
        self.has(0x0020)
    }

    /// The server's seat. The core does not list it among the players of a network game; a
    /// single-player game with an observer does.
    pub fn observer(self) -> bool {
        self.has(0x0040)
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum CatalogTable {
    Units = 0,
    Buildings = 1,
    Projects = 2,
    Technologies = 3,
    Policies = 4,
    Terrains = 5,
    Features = 6,
    Resources = 7,
    Improvements = 8,
    Routes = 9,
    Civilizations = 10,
    Leaders = 11,
    Eras = 12,
    Handicaps = 13,
    Promotions = 14,
    /// v2: the landmark art technologies, in the bit order of `Player::art_techs`.
    ArtTechs = 16,
    /// v2: the `GameSpeeds` rows (only id, type and name are set).
    GameSpeeds = 18,
    /// v5 (UI v2): the policy branches, ids as in `Q_POLICY_BRANCHES` (only id, type and name are set).
    PolicyBranches = 25,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GameType {
    /// The B1 game: one human in slot 0 (or an observer), no server seat. The default of `Setup`,
    /// so B1 callers and the stub keep working.
    #[default]
    Single,
    /// The multiplayer game of B2: 1 to 8 humans, Pitboss, a server seat.
    Network,
}

impl GameType {
    /// The `setup_json` value.
    pub fn name(self) -> &'static str {
        match self {
            GameType::Single => "single",
            GameType::Network => "network",
        }
    }
}

impl Default for GameInfo {
    fn default() -> Self {
        GameInfo {
            turn: 0,
            year: -4000,
            blocker_unit: -1,
            blocker_city: -1,
            blocker_x: -1,
            blocker_y: -1,
            map_width: 0,
            map_height: 0,
            human_player: 0,
            human_team: 0,
            active_player: 0,
            status: GameStatus::WaitingHuman,
            blocker: -1,
            wrap_x: false,
            wrap_y: false,
            difficulty: 0,
            winner: -1,
            victory_type: -1,
            game_type: GameType::Single,
            turn_mode: None,
            phase: Phase::Humans,
            humans: 1,
            humans_left: 1,
            safe_point: true,
            unsafe_reason: None,
            turn_len_s: 0,
            turn_base_s: 0,
            map_seed: 0,
        }
    }
}
