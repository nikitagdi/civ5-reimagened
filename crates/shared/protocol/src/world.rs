//! WorldView protocol (SIMP-004): everything the human sees, plus commands and queries.
//!
//! The brain (`gamecore`) filters fog; the screen only draws what it gets. Field order is the wire
//! format: add fields and variants at the end only. Protocol v3 adds the fields marked "v3" (net-protocol
//! 4.5, 4.15) and the map basis (`MapBasis`) that travels apart from the view.
//!
//! Coordinates are the core's: plot (x, y), y grows north, odd rows are shifted half a hex east,
//! `wrap_x` comes from the map. `MapView::plots` is row-major starting at y = 0.
//! Records mirror the `gc-abi-v1` queries (`Q_PLOTS`, `Q_UNITS`, `Q_CITIES`, `Q_PLAYERS`,
//! `Q_TOP_BAR`, `Q_CATALOG`); all texts are already localized (en_US).

use serde::{Deserialize, Serialize};

use crate::context::{CombatPreview, ReachNode};

/// Player slot index in the core.
pub type PlayerId = i8;
/// Team index in the core.
pub type TeamId = i8;
/// Game (room that started) id on the server (net-protocol 4.11).
pub type GameId = u32;

/// Declares a `u16` bit-set newtype with named bits.
macro_rules! flags {
    ($(#[$meta:meta])* $name:ident { $($(#[$cmeta:meta])* $bit:ident = $val:expr),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(pub u16);

        impl $name {
            $($(#[$cmeta])* pub const $bit: u16 = $val;)+

            pub const fn contains(self, bit: u16) -> bool {
                self.0 & bit == bit
            }
        }
    };
}

flags! {
    /// `Q_PLOTS` flags without the river edges (those are `PlotView::rivers`).
    PlotFlags {
        REVEALED = 1 << 0,
        VISIBLE = 1 << 1,
        HAS_CITY = 1 << 2,
        IS_LAKE = 1 << 3,
        IS_COASTAL = 1 << 4,
        PILLAGED = 1 << 5,
        NATURAL_WONDER = 1 << 6,
        /// The route (road, railroad) is pillaged. Visible plots only (v3, net-protocol 4.15).
        ROUTE_PILLAGED = 1 << 7,
        /// A city of the viewer works the plot. Visible plots of the viewer's cities only (v3).
        WORKED = 1 << 8,
    }
}

flags! {
    /// `Q_UNITS` flags.
    UnitFlags {
        CAN_MOVE = 1 << 0,
        NEEDS_ORDERS = 1 << 1,
        FORTIFIED = 1 << 2,
        SLEEPING = 1 << 3,
        AUTOMATED = 1 << 4,
        EMBARKED = 1 << 5,
        CIVILIAN = 1 << 6,
        CAN_PROMOTE = 1 << 7,
        GARRISONED = 1 << 8,
    }
}

flags! {
    /// `Q_CITIES` flags.
    CityFlags {
        CAPITAL = 1 << 0,
        PUPPET = 1 << 1,
        RAZING = 1 << 2,
        OCCUPIED = 1 << 3,
        CAN_RANGE_ATTACK = 1 << 4,
    }
}

flags! {
    /// `Q_PLAYERS` flags.
    PlayerFlags {
        ALIVE = 1 << 0,
        HUMAN = 1 << 1,
        MINOR = 1 << 2,
        BARBARIAN = 1 << 3,
        MET = 1 << 4,
        AT_WAR_WITH_VIEWER = 1 << 5,
    }
}

flags! {
    /// `Q_CITY_BUILDS` flags.
    CityBuildFlags {
        WONDER = 1 << 0,
        CURRENT = 1 << 1,
        CAN_PURCHASE = 1 << 2,
        /// Can be started now (`CitySetProduction` would be accepted).
        ENABLED = 1 << 3,
    }
}

flags! {
    /// Flags of a path-search node (`PathNode::flags`, `ReachNode::flags`; net-protocol 4.2, 4.9).
    ReachFlags {
        /// The move onto this node is an attack.
        ATTACK = 1 << 0,
        EMBARK = 1 << 1,
        DISEMBARK = 1 << 2,
        CITY = 1 << 3,
        /// The unit stops here (zone of control, end of its moves).
        STOP = 1 << 4,
        /// The plot is not revealed to the viewer.
        UNREVEALED = 1 << 5,
        CAPTURE = 1 << 6,
        /// Only in the answer of `Query::Path`: the target cannot be reached. Never inside a reach tree.
        UNREACHABLE = 1 << 7,
    }
}

// ---------------------------------------------------------------- catalog

/// Asset references of a catalog entry; empty strings mean "none".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtRefs {
    pub art_define: String,
    pub icon_atlas: String,
    pub icon_index: i16,
    pub flag_atlas: String,
    pub flag_index: i16,
    pub art_style: String,
    pub strategic_view: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub id: i16,
    /// Database type, e.g. `UNIT_WARRIOR`.
    pub type_name: String,
    /// Localized name.
    pub name: String,
    pub art: ArtRefs,
}

/// Static reference data, sent once per game (`ServerMsg::GameStarted`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    pub units: Vec<CatalogEntry>,
    pub buildings: Vec<CatalogEntry>,
    pub projects: Vec<CatalogEntry>,
    pub techs: Vec<CatalogEntry>,
    pub policies: Vec<CatalogEntry>,
    pub terrains: Vec<CatalogEntry>,
    pub features: Vec<CatalogEntry>,
    pub resources: Vec<CatalogEntry>,
    pub improvements: Vec<CatalogEntry>,
    pub routes: Vec<CatalogEntry>,
    pub civs: Vec<CatalogEntry>,
    pub leaders: Vec<CatalogEntry>,
    pub eras: Vec<CatalogEntry>,
    pub handicaps: Vec<CatalogEntry>,
    pub promotions: Vec<CatalogEntry>,
    /// v3: tech ids the art depends on; bit i of `PlayerView::art_techs` is "has tech `art_techs[i]`".
    /// At most 64. Appended last: the codec is positional.
    pub art_techs: Vec<i16>,
}

// ---------------------------------------------------------------- world

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnStatus {
    HumanTurn,
    AiTurn { active: PlayerId },
    GameOver { winner: Option<TeamId> },
}

/// What blocks the end of the human's turn: the raw `EndTurnBlockingTypes` value and its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blocker {
    pub kind: i32,
    pub target: BlockerTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockerTarget {
    None,
    Unit(i32),
    City(i32),
    Plot { x: i16, y: i16 },
}

/// Land / hill / mountain / water.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlotType {
    #[default]
    Land,
    Hill,
    Mountain,
    Water,
}

/// One plot. Ids are catalog ids; `-1` means none.
///
/// The server gives the client the whole world (NP-D19, user decision 2026-10-22): a plot the seat has not
/// revealed carries its world fields too (relief, feature, resource, improvement, route, rivers), without
/// `REVEALED` and without what only sight gives (`PlotView::unrevealed_of`). Fog of war is a matter of the
/// render.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlotView {
    pub flags: PlotFlags,
    pub plot_type: PlotType,
    pub terrain: i16,
    pub feature: i16,
    pub resource: i16,
    pub improvement: i16,
    pub route: i16,
    pub owner: i8,
    pub continent_art: u8,
    /// Six edge bits; bit i is gamecore `DirectionType` i (0 NE, 1 E, 2 SE, 3 SW, 4 W, 5 NW).
    /// A set bit means a river on the edge towards that neighbour.
    pub rivers: u8,
    /// Food, production, gold, science, culture, faith.
    pub yields: [i8; 6],
    /// v3: the job under way on the plot, a `BuildTypes` id (row of the `Builds` table, not a catalog
    /// id; the client maps it to the improvement by name), `-1` none. Appended last (positional codec).
    pub building: i16,
    /// v3: resource quantity (`gcq_plot`).
    pub resource_qty: i8,
}

impl PlotView {
    /// The flags an unrevealed plot keeps: those of the world, not of sight.
    pub const WORLD_FLAGS: u16 = PlotFlags::IS_LAKE
        | PlotFlags::IS_COASTAL
        | PlotFlags::NATURAL_WONDER
        | PlotFlags::PILLAGED
        | PlotFlags::ROUTE_PILLAGED;

    /// The record of a plot the seat has not revealed, from the record of the whole truth (viewer `-1`):
    /// the world fields as they are, no `REVEALED`/`VISIBLE`/`HAS_CITY`/`WORKED`, no owner, no yields, no
    /// job under way (NP-D19, 2026-10-22).
    pub fn unrevealed_of(truth: &PlotView) -> PlotView {
        PlotView {
            flags: PlotFlags(truth.flags.0 & Self::WORLD_FLAGS),
            owner: -1,
            yields: [0; 6],
            building: -1,
            ..truth.clone()
        }
    }

    /// An unrevealed plot that carries something only sight gives: a leak (the fog audits, NP 4.5).
    pub fn has_sight_data(&self) -> bool {
        self.flags.0 & !Self::WORLD_FLAGS != 0
            || self.owner >= 0
            || self.yields != [0; 6]
            || self.building >= 0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapView {
    pub width: u16,
    pub height: u16,
    pub wrap_x: bool,
    pub wrap_y: bool,
    /// Row-major, y from 0 (south), `width * height` entries.
    pub plots: Vec<PlotView>,
    /// v3: relief seed of the game (once per game, like `wrap_x`). Appended last (positional codec).
    pub terrain_seed: u32,
}

/// Static relief of one plot, revealed or not (v3, net-protocol 4.5, NP-D19). Lives in `MapBasis`, not in
/// `WorldView`. `feature` holds only relief features (natural wonder, marsh, flood plains, ice), else `-1`;
/// `flags` only `IS_LAKE`, `IS_COASTAL`, `NATURAL_WONDER`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlotBasisView {
    pub plot_type: PlotType,
    /// Catalog id.
    pub terrain: i16,
    pub continent_art: u8,
    pub feature: i16,
    pub flags: PlotFlags,
    /// Six edge bits, as `PlotView::rivers`.
    pub rivers: u8,
    /// River flow (`FlowDirectionTypes`) on the edges E, SE, SW; `-1` none.
    pub river_flow: [i8; 3],
}

/// Map basis of one game for one seat: the static relief of ALL plots, fog does not cut it (NP-D19).
/// `GameMsg::MapBasis` (stream tag 8); never part of `WorldView` or of the keyframe (NP-D22).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapBasis {
    pub game: GameId,
    /// Hash of the header below and the plot records (`BasisState`, net-protocol 4.6); `game` is not in it.
    pub basis_hash: u64,
    pub width: u16,
    pub height: u16,
    pub wrap_x: bool,
    pub wrap_y: bool,
    pub terrain_seed: u32,
    /// All plots, dense, index `y * width + x`.
    pub plots: Vec<PlotBasisView>,
}

/// Moves are in 1/60 of a move, as in the core.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitView {
    pub id: i32,
    pub owner: PlayerId,
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
    pub domain: i8,
    pub activity: i8,
    pub level: i8,
    pub experience: i16,
    pub flags: UnitFlags,
    pub name: String,
    /// v3: `DirectionTypes` the unit faces. Appended last (positional codec).
    pub facing: i8,
    /// v3: id of the transport carrying this unit, `-1` none.
    pub cargo_of: i32,
    /// v3: what a worker is building now (`BuildTypes`), `-1` none; visible units only.
    pub building: i16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CityView {
    pub id: i32,
    pub owner: PlayerId,
    pub x: i16,
    pub y: i16,
    pub name: String,
    pub population: i16,
    pub flags: CityFlags,
    pub hp: i16,
    pub max_hp: i16,
    pub defense: i16,
    pub production_order: i8,
    pub production_item: i16,
    pub production_turns: i16,
    pub production_stored: i32,
    pub production_needed: i32,
    pub growth_turns: i16,
    /// Per turn; zero for foreign cities.
    pub yields: [i16; 6],
    /// v3: buildings with a model in the world (wonders, walls, ...); of a foreign city only the ones
    /// visible from outside. Appended last (positional codec).
    pub buildings: Vec<i16>,
    /// v3: the player who founded the city (`CvCity::getOriginalOwner`).
    pub founder: PlayerId,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerView {
    pub id: PlayerId,
    pub team: TeamId,
    pub civ_type: i16,
    pub leader_type: i16,
    pub flags: PlayerFlags,
    /// RGBA.
    pub color_primary: u32,
    /// RGBA.
    pub color_secondary: u32,
    pub name: String,
    pub civ_name: String,
    /// Current era (`Eras` id, as `TopBar::era`); `-1` when the host does not know it for this
    /// player. Appended last: the codec is positional.
    pub era: i16,
    /// v3: bit i is set when the player has tech `Catalog::art_techs[i]`; for foreign players only
    /// when `MET`. Appended last (positional codec).
    pub art_techs: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopBar {
    pub gold: i32,
    pub gold_per_turn: i32,
    pub science_per_turn: i32,
    pub research_tech: i16,
    pub research_turns: i16,
    pub research_progress: i32,
    pub research_cost: i32,
    pub culture: i32,
    pub culture_per_turn: i32,
    pub next_policy_cost: i32,
    pub happiness: i16,
    pub golden_age_turns: i16,
    pub faith: i32,
    pub faith_per_turn: i32,
    pub era: i16,
    /// Golden age points gathered and needed for the next one (`Q_TOP_BAR`); appended last.
    pub golden_age_progress: i32,
    pub golden_age_threshold: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldView {
    pub turn: i32,
    pub year: i32,
    pub human: PlayerId,
    pub human_team: TeamId,
    pub status: TurnStatus,
    pub blocker: Option<Blocker>,
    pub map: MapView,
    /// Visible to the human team.
    pub units: Vec<UnitView>,
    /// Known to the human team.
    pub cities: Vec<CityView>,
    pub players: Vec<PlayerView>,
    pub top_bar: TopBar,
    /// v3: the seat's notifications, by id.
    pub notifications: Vec<NotificationView>,
    /// v3: peace offers the seat takes part in.
    pub peace_offers: Vec<PeaceOffer>,
    /// v3: trade routes visible to the seat (Should). A new section: it takes hash section number 9
    /// (net-protocol 4.6, 4.15).
    pub trade_routes: Vec<TradeRouteView>,
}

/// A white-peace offer between two humans (NP-D17).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeaceOffer {
    pub from: PlayerId,
    pub to: PlayerId,
    pub turn: i32,
}

/// A trade route line (v3, net-protocol 4.15).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradeRouteView {
    pub owner: PlayerId,
    pub from: (i16, i16),
    pub to: (i16, i16),
    pub domain: i8,
}

/// Key of a unit in a view: `(owner, id)`.
pub type UnitKey = (PlayerId, i32);
/// Key of a city in a view: `(owner, id)`.
pub type CityKey = (PlayerId, i32);

// ---------------------------------------------------------------- commands and queries

/// Human orders. Ids are the player's own unit / city ids; `order` is the city order type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    UnitMoveTo {
        unit: i32,
        x: i16,
        y: i16,
    },
    UnitRangeAttack {
        unit: i32,
        x: i16,
        y: i16,
    },
    UnitFoundCity {
        unit: i32,
    },
    UnitFortify {
        unit: i32,
    },
    UnitSleep {
        unit: i32,
    },
    UnitSkip {
        unit: i32,
    },
    UnitHeal {
        unit: i32,
    },
    UnitAutomateBuild {
        unit: i32,
    },
    UnitAutomateExplore {
        unit: i32,
    },
    UnitCancel {
        unit: i32,
    },
    UnitDisband {
        unit: i32,
    },
    UnitWake {
        unit: i32,
    },
    CitySetProduction {
        city: i32,
        order: i8,
        item: i16,
    },
    CityPurchase {
        city: i32,
        order: i8,
        item: i16,
        yield_: i8,
    },
    CityRangeAttack {
        city: i32,
        x: i16,
        y: i16,
    },
    DeclareWar {
        team: TeamId,
    },
    EndTurn {
        force: bool,
    },
    /// v3 (tag 17): take back the seat's end of turn (AC-N4.9).
    CancelEndTurn,
    /// v3 (tag 18): offer white peace to a human at war with the seat (NP-D17, SIMP-057).
    ProposePeace {
        to: PlayerId,
    },
    /// v3 (tag 19): answer a peace offer.
    AnswerPeace {
        from: PlayerId,
        accept: bool,
    },
    /// v15 (tag 20): add `item` of order type `order` to the city's build queue, at the front
    /// (`front = true`) or at the back.
    CityQueuePush {
        city: i32,
        order: i8,
        item: i16,
        front: bool,
    },
    /// v15 (tag 21): remove the queue entry at `slot` (0 is the item being built).
    CityQueuePop {
        city: i32,
        slot: i8,
    },
    /// v15 (tag 22): move the queue entry at `from` to `to`, shifting the entries between them.
    CityQueueMove {
        city: i32,
        from: i8,
        to: i8,
    },
    /// v15 (tag 23): replace only the item being built; the rest of the queue stays.
    CityReplaceHead {
        city: i32,
        order: i8,
        item: i16,
    },
    /// v15 (tag 24): research `tech` now (`queue = false`) or append it to the research queue.
    ResearchSet {
        tech: i16,
        queue: bool,
    },
}

/// Who shoots in `Query::RangeTargets`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Shooter {
    Unit(i32),
    City(i32),
}

/// Questions answered by the brain (rules are never computed on the host side).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Query {
    UnitActions {
        unit: i32,
    },
    CityBuilds {
        city: i32,
    },
    Path {
        unit: i32,
        x: i16,
        y: i16,
    },
    Reachable {
        unit: i32,
    },
    RangeTargets {
        from: Shooter,
    },
    /// The units the human's team sees (the same list as `WorldView::units`, fresh).
    Units,
    /// The cities the human's team knows (the same list as `WorldView::cities`, fresh).
    Cities,
    /// The human's top bar (the same record as `WorldView::top_bar`, fresh).
    TopBar,
    /// The human's live notifications (`Q_NOTIFICATIONS`).
    Notifications,
    /// v3 (tag 9): move tree of a unit, `horizon` turns (only 1 is used in B2); answer `QueryAnswer::Reach`.
    Reach {
        unit: i32,
        horizon: u8,
    },
    /// v3 (tag 10): combat forecast of `attacker` against plot (x, y); answer `QueryAnswer::CombatPreview`.
    CombatPreview {
        attacker: Shooter,
        x: i16,
        y: i16,
    },
}

/// `Q_NOTIFICATIONS` record. `at` is `None` when the notification has no plot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationView {
    pub id: i32,
    /// `NotificationTypes` hash.
    pub kind: u32,
    pub at: Option<(i16, i16)>,
    pub data1: i32,
    pub data2: i32,
    /// Game turn it was added on.
    pub turn: i16,
    pub dismissed: bool,
    pub message: String,
    pub summary: String,
}

/// `Q_UNIT_ACTIONS` record. `action` is the command number of gc-abi-v1 4.4.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitActionView {
    pub action: i16,
    pub enabled: bool,
    /// Why it is disabled, if known.
    pub reason: String,
}

/// `Q_CITY_BUILDS` record. `gold_cost` / `faith_cost` are `-1` when it cannot be bought with that.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CityBuildView {
    pub order_type: i8,
    pub item: i16,
    pub turns: i16,
    pub production_cost: i32,
    pub gold_cost: i32,
    pub flags: CityBuildFlags,
    /// Cost with faith, `-1` when it cannot be bought with faith. Appended last (positional codec).
    pub faith_cost: i32,
}

/// `Q_PATH` record: a path node and the turn it is reached on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathNode {
    pub x: i16,
    pub y: i16,
    pub turn: i16,
    /// v3: flags of the search node; `UNREACHABLE` marks the only node of an unreachable target.
    /// Appended last (positional codec).
    pub flags: ReachFlags,
}

/// Payload of `ServerMsg::QueryResult`; the variant matches the asked `Query`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueryAnswer {
    UnitActions(Vec<UnitActionView>),
    CityBuilds(Vec<CityBuildView>),
    Path(Vec<PathNode>),
    Reachable(Vec<(i16, i16)>),
    RangeTargets(Vec<(i16, i16)>),
    /// The query failed; the reply to every failed `Query`.
    Error {
        reason: String,
    },
    Units(Vec<UnitView>),
    Cities(Vec<CityView>),
    TopBar(TopBar),
    Notifications(Vec<NotificationView>),
    /// v3 (tag 10): the move tree, node 0 is the unit's plot.
    Reach(Vec<ReachNode>),
    /// v3 (tag 11): the forecast, `None` when there is nothing to attack on the plot.
    CombatPreview(Option<CombatPreview>),
}

// ---------------------------------------------------------------- journal

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JournalKind {
    Notification,
    /// An auto-choice made for the human (SIMP-002).
    AutoChoice,
    /// A diplomacy offer declined for the human (SIMP-003).
    DiploAutoDeclined,
    /// Informational popup text (ruins, natural wonder, golden age, ...).
    Info,
    War,
    LuaError,
    /// A combat line of the core's own message log (`GC_EVENT_MESSAGE`, type COMBAT_MESSAGE).
    /// Appended last: the codec is positional.
    Combat,
    /// v3 (tag 7): restart, pause, save.
    System,
    /// v3 (tag 8): seat events (taken over, kick, return, host change).
    Session,
    /// v3 (tag 9): peace offers and answers.
    Diplomacy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub turn: i32,
    pub kind: JournalKind,
    pub text: String,
    pub at: Option<(i16, i16)>,
    /// SIMP number for auto-choices and auto-declines.
    pub simplification: Option<u16>,
}

// ---------------------------------------------------------------- lobby, setup, saves

/// Game setup; mirrors the `setup_json` of gc-abi-v1 4.2. `seed` / `map_seed` `None` means
/// random (the host picks). Option maps are `(key, value)` lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Setup {
    pub seed: Option<u32>,
    pub map_seed: Option<u32>,
    /// `CIVILIZATION_*` type or `"random"`.
    pub human_civ: String,
    pub ai_count: u8,
    pub world_size: String,
    pub map_script: String,
    pub map_options: Vec<(String, i32)>,
    pub handicap: String,
    pub game_speed: String,
    pub start_era: String,
    pub minor_civs: String,
    pub game_options: Vec<(String, i32)>,
    pub auto_choices: AutoChoices,
    pub diplo_auto_decline: bool,
    pub autoplay_human: bool,
    pub observer: bool,
}

/// Which decisions the brain makes for the human (SIMP-002).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoChoices {
    pub research: bool,
    pub policy: bool,
    pub ideology: bool,
    pub religion: bool,
    pub promotion: bool,
    pub great_person: bool,
    pub captured_city: bool,
    pub league: bool,
    pub archaeology: bool,
    pub goody_hut: bool,
    pub steal_tech: bool,
}

impl Default for AutoChoices {
    fn default() -> Self {
        Self {
            research: true,
            policy: true,
            ideology: true,
            religion: true,
            promotion: true,
            great_person: true,
            captured_city: true,
            league: true,
            archaeology: true,
            goody_hut: true,
            steal_tech: true,
        }
    }
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            seed: None,
            map_seed: None,
            human_civ: "random".into(),
            ai_count: 1,
            world_size: "WORLDSIZE_DUEL".into(),
            map_script: "Assets/Maps/Lekmap v5.2/LekmapPangaeaFractalv5.2.lua".into(),
            map_options: Vec::new(),
            handicap: "HANDICAP_PRINCE".into(),
            game_speed: "GAMESPEED_STANDARD".into(),
            start_era: "ERA_ANCIENT".into(),
            minor_civs: "default".into(),
            game_options: Vec::new(),
            auto_choices: AutoChoices::default(),
            diplo_auto_decline: true,
            autoplay_human: false,
            observer: false,
        }
    }
}

/// A pick-list entry of the lobby.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyChoice {
    pub type_name: String,
    pub name: String,
}

/// Header of a save slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveMeta {
    pub seq: u64,
    pub build_id: String,
    pub turn: i32,
    pub civ_name: String,
    /// Unix time, milliseconds.
    pub saved_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyInfo {
    pub civs: Vec<LobbyChoice>,
    pub handicaps: Vec<LobbyChoice>,
    /// The newest valid save, if any.
    pub save: Option<SaveMeta>,
}
