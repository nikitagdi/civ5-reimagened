//! Typed results of the in-game queries: units, unit actions, paths, range targets, notifications,
//! cities, city builds, top bar.
//!
//! "No value" is `None` where the ABI uses -1 for an id, an order or a turn count.

use gamecore_sys::ffi;

use crate::types::Direction;

/// Denominator of the move fields (`MOVE_DENOMINATOR` of the core): 60 = one whole move.
pub const MOVE_DENOMINATOR: i16 = 60;

/// `OrderTypes` of the core (`GC_ORDER_*`): what a city builds or buys.
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
    pub(crate) fn from_abi(v: i8) -> Self {
        match i32::from(v) {
            ffi::GC_ORDER_TRAIN => OrderType::Train,
            ffi::GC_ORDER_CONSTRUCT => OrderType::Construct,
            ffi::GC_ORDER_CREATE => OrderType::Create,
            ffi::GC_ORDER_MAINTAIN => OrderType::Maintain,
            _ => OrderType::Other(v),
        }
    }

    /// The `GC_ORDER_*` number.
    pub fn abi(self) -> i32 {
        match self {
            OrderType::Train => ffi::GC_ORDER_TRAIN,
            OrderType::Construct => ffi::GC_ORDER_CONSTRUCT,
            OrderType::Create => ffi::GC_ORDER_CREATE,
            OrderType::Maintain => ffi::GC_ORDER_MAINTAIN,
            OrderType::Other(v) => i32::from(v),
        }
    }
}

/// `ActivityTypes` of a unit.
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
    pub(crate) fn from_abi(v: i8) -> Self {
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

/// `flags` of a unit record (`GC_UNIT_F_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UnitFlags(pub u16);

impl UnitFlags {
    fn has(self, bit: u16) -> bool {
        self.0 & bit != 0
    }

    /// Moves left and not immobile.
    pub fn can_move(self) -> bool {
        self.has(ffi::GC_UNIT_F_CAN_MOVE)
    }

    /// Awake, no orders queued, not automated: what the UNITS end-turn blocker waits for.
    pub fn needs_orders(self) -> bool {
        self.has(ffi::GC_UNIT_F_NEEDS_ORDERS)
    }

    pub fn fortified(self) -> bool {
        self.has(ffi::GC_UNIT_F_FORTIFIED)
    }

    /// Sleeping and not fortified.
    pub fn sleeping(self) -> bool {
        self.has(ffi::GC_UNIT_F_SLEEPING)
    }

    pub fn automated(self) -> bool {
        self.has(ffi::GC_UNIT_F_AUTOMATED)
    }

    pub fn embarked(self) -> bool {
        self.has(ffi::GC_UNIT_F_EMBARKED)
    }

    pub fn civilian(self) -> bool {
        self.has(ffi::GC_UNIT_F_CIVILIAN)
    }

    pub fn can_promote(self) -> bool {
        self.has(ffi::GC_UNIT_F_CAN_PROMOTE)
    }

    pub fn garrisoned(self) -> bool {
        self.has(ffi::GC_UNIT_F_GARRISONED)
    }

    /// Carried by a transport (v2; `transport_id` says which).
    pub fn cargo(self) -> bool {
        self.has(ffi::GC_UNIT_F_CARGO)
    }
}

/// `Q_UNITS` record. Moves are in 1/60 of a move ([`MOVE_DENOMINATOR`]); `strength` and `ranged` are
/// the base values (0 = none).
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

/// `Q_UNIT_ACTIONS` record: one unit command that applies to the unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitAction {
    /// `GC_CMD_UNIT_*`.
    pub action: i16,
    /// Can be given now (the target is not checked).
    pub enabled: bool,
    /// Why not, when `enabled` is false and the core says.
    pub reason: Option<String>,
}

/// Flags of a path-search node (`GC_REACH_F_*`): `Q_PATH`, `Q_REACHABLE`, `Q_REACHABLE_TREE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReachFlags(pub u16);

impl ReachFlags {
    fn has(self, bit: u16) -> bool {
        self.0 & bit != 0
    }

    /// Moving here attacks.
    pub fn attack(self) -> bool {
        self.has(ffi::GC_REACH_F_ATTACK)
    }

    /// The unit embarks on this step.
    pub fn embark(self) -> bool {
        self.has(ffi::GC_REACH_F_EMBARK)
    }

    /// The unit disembarks on this step.
    pub fn disembark(self) -> bool {
        self.has(ffi::GC_REACH_F_DISEMBARK)
    }

    /// A city stands here.
    pub fn city(self) -> bool {
        self.has(ffi::GC_REACH_F_CITY)
    }

    /// The unit has to stop here (zone of control or the end of its moves).
    pub fn stop(self) -> bool {
        self.has(ffi::GC_REACH_F_STOP)
    }

    /// The plot is not revealed to the unit's team.
    pub fn unrevealed(self) -> bool {
        self.has(ffi::GC_REACH_F_UNREVEALED)
    }

    /// The step captures what stands on the plot.
    pub fn capture(self) -> bool {
        self.has(ffi::GC_REACH_F_CAPTURE)
    }

    /// `Q_PATH` only: no path to the target; the one node is the target. (`Q_REACHABLE_TREE` never
    /// sets it.)
    pub fn unreachable(self) -> bool {
        self.has(ffi::GC_REACH_F_UNREACHABLE)
    }
}

/// `Q_PATH` / `Q_REACHABLE` record. First node of a path = the unit's plot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathNode {
    pub x: i16,
    pub y: i16,
    /// Turns from now at which the unit stands here, 0 = this turn.
    pub turn: i16,
    /// Moves left on arrival, 1/60 of a move.
    pub moves_left: i16,
    /// What the step does (v2 records; empty for a reply of a core without the field). A target
    /// that cannot be reached comes as the only node, with `unreachable()` set, instead of an empty
    /// path.
    pub flags: ReachFlags,
}

/// A `Q_REACHABLE` record is a [`PathNode`] (`turn` 0).
pub type Reachable = PathNode;

/// `Q_REACHABLE_TREE` record: a plot the unit reaches within the horizon, and the plot it comes from.
/// The root (the unit's plot) is not listed; its children have it as the parent. A path to any node
/// is the chain of parents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeNode {
    pub x: i16,
    pub y: i16,
    pub parent_x: i16,
    pub parent_y: i16,
    /// Moves left on arrival, 1/60 of a move.
    pub moves_left: i16,
    /// 0 = reached this turn.
    pub turn: u8,
    pub flags: ReachFlags,
}

/// Who attacks in `Q_COMBAT_PREVIEW`: a unit or a city of the asking player, by id (the same
/// encoding as the shooter of `Q_RANGE_TARGETS`).
pub type Attacker = RangeShooter;

/// Who shoots in `Q_RANGE_TARGETS`: a unit or a city of the asking player, by id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeShooter {
    Unit(i32),
    City(i32),
}

impl RangeShooter {
    /// The `shooter` argument of the query: a unit id as is, a city as `-id`. `None` for an id the
    /// encoding cannot carry (a negative unit id, a city id that is not positive); the core never
    /// hands out such ids.
    pub fn abi(self) -> Option<i32> {
        match self {
            RangeShooter::Unit(id) => (id >= 0).then_some(id),
            RangeShooter::City(id) => (id > 0).then(|| -id),
        }
    }
}

/// `flags` of a `Q_RANGE_TARGETS` record (`GC_RANGE_TARGET_F_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RangeTargetFlags(pub u16);

impl RangeTargetFlags {
    /// The target plot holds a city (unit shooters only).
    pub fn city(self) -> bool {
        self.0 & ffi::GC_RANGE_TARGET_F_CITY != 0
    }
}

/// `Q_RANGE_TARGETS` record: a plot the shooter can hit right now (the range attack command
/// accepts it), by y then x.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeTarget {
    pub x: i16,
    pub y: i16,
    pub flags: RangeTargetFlags,
}

/// `Q_NOTIFICATIONS` record.
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

/// `flags` of a city record (`GC_CITY_F_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CityFlags(pub u16);

impl CityFlags {
    fn has(self, bit: u16) -> bool {
        self.0 & bit != 0
    }

    pub fn capital(self) -> bool {
        self.has(ffi::GC_CITY_F_CAPITAL)
    }

    /// Own cities only.
    pub fn puppet(self) -> bool {
        self.has(ffi::GC_CITY_F_PUPPET)
    }

    pub fn razing(self) -> bool {
        self.has(ffi::GC_CITY_F_RAZING)
    }

    /// Own cities only: captured, not yet pacified by a courthouse.
    pub fn occupied(self) -> bool {
        self.has(ffi::GC_CITY_F_OCCUPIED)
    }

    /// Own cities only: can shoot at a target in range right now.
    pub fn can_range_attack(self) -> bool {
        self.has(ffi::GC_CITY_F_CAN_RANGE_ATTACK)
    }

    /// Live data; clear = last seen (fogged).
    pub fn visible(self) -> bool {
        self.has(ffi::GC_CITY_F_VISIBLE)
    }

    /// Own cities only.
    pub fn resistance(self) -> bool {
        self.has(ffi::GC_CITY_F_RESISTANCE)
    }
}

/// `Q_CITIES` record. A fogged foreign city (`!flags.visible()`) has only `id`, `owner`, `x`, `y` and
/// `name`. Production and `yields` are set for own cities only.
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
    pub name: String,
}

/// `Q_CITY_ART` record: a city to draw in the art of the viewer's view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CityArt {
    /// Unique inside the owner.
    pub city_id: i32,
    pub owner: u8,
    /// `BuildingTypes` shown on the city.
    pub building: i16,
    pub x: i16,
    pub y: i16,
}

/// `Q_TRADE_ROUTES` record: one trade route of a player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TradeRoute {
    pub id: i32,
    pub owner: u8,
    pub dest_owner: u8,
    /// `DomainTypes`.
    pub domain: i8,
    /// Plot of the origin city.
    pub from: (i16, i16),
    /// Plot of the destination city.
    pub to: (i16, i16),
}

/// `Q_TRADE_PATHS` (B4 fix-path): the path of one trade route, as the DLL's trade route path finder laid it
/// (`TradeConnection::m_aPlotList`, what the caravan walks and the exe's `TradeVisSystem` draws).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TradePath {
    /// As [`TradeRoute::id`].
    pub id: i32,
    /// `(x, y)` from the origin city to the destination city.
    pub plots: Vec<(i16, i16)>,
}

/// `flags` of a `Q_TOP_BAR` record (`GC_TOPBAR_F_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TopBarFlags(pub u16);

impl TopBarFlags {
    fn has(self, bit: u16) -> bool {
        self.0 & bit != 0
    }

    pub fn unhappy(self) -> bool {
        self.has(ffi::GC_TOPBAR_F_UNHAPPY)
    }

    pub fn very_unhappy(self) -> bool {
        self.has(ffi::GC_TOPBAR_F_VERY_UNHAPPY)
    }

    pub fn golden_age(self) -> bool {
        self.has(ffi::GC_TOPBAR_F_GOLDEN_AGE)
    }
}

/// `Q_TOP_BAR`: the values of the top panel.
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
}

/// `flags` of a `Q_CITY_BUILDS` record (`GC_BUILD_F_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BuildFlags(pub u16);

impl BuildFlags {
    fn has(self, bit: u16) -> bool {
        self.0 & bit != 0
    }

    /// Wonder, national wonder or other limited building.
    pub fn wonder(self) -> bool {
        self.has(ffi::GC_BUILD_F_WONDER)
    }

    /// The head of the order queue.
    pub fn current(self) -> bool {
        self.has(ffi::GC_BUILD_F_CURRENT)
    }

    /// A `CityPurchase` with gold would be accepted now.
    pub fn can_purchase(self) -> bool {
        self.has(ffi::GC_BUILD_F_CAN_PURCHASE)
    }

    /// Can be started now.
    pub fn enabled(self) -> bool {
        self.has(ffi::GC_BUILD_F_ENABLED)
    }
}

/// `Q_CITY_BUILDS` record: one thing the city could build (units, buildings and wonders, projects,
/// processes, in that order, by item id).
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
