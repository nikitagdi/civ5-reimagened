//! Focus and contexts (net-protocol 4.9, NP-D12): what the server precomputes for the units the player is
//! about to command. Field order is the wire format.

use serde::{Deserialize, Serialize};

use crate::world::{CityBuildView, ReachFlags, UnitActionView};

/// Client to server: the units (up to 4, the first is selected) and the city the player works with.
/// A new `Focus` replaces the old one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Focus {
    pub units: Vec<i32>,
    pub city: Option<i32>,
}

/// Move tree of a unit, actions and targets. Valid while the client's `view_seq` equals `view_seq`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitContext {
    pub unit: i32,
    /// Set by the gateway.
    pub view_seq: u64,
    /// As `Q_UNIT_ACTIONS`.
    pub actions: Vec<UnitActionView>,
    /// Node 0 is the unit's plot.
    pub reach: Vec<ReachNode>,
    /// Targets of ranged and melee attacks.
    pub targets: Vec<TargetView>,
}

/// A node of the move tree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReachNode {
    pub x: i16,
    pub y: i16,
    /// Index of the parent in the list; 0 for the root; a parent always comes before its children.
    pub parent: u16,
    /// 0: reached this turn.
    pub turn: u8,
    /// Moves left on arrival, 1/60 of a move.
    pub moves_left: i16,
    pub flags: ReachFlags,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetKind {
    Ranged,
    Melee,
}

/// A plot the unit or city can attack, with the forecast when it is known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetView {
    pub x: i16,
    pub y: i16,
    pub kind: TargetKind,
    /// `TargetView::CITY`.
    pub flags: u16,
    pub preview: Option<CombatPreview>,
}

impl TargetView {
    /// The target plot holds a city.
    pub const CITY: u16 = 1 << 0;
}

/// Forecast of one attack (`Q_COMBAT_PREVIEW` style numbers, as the core computes them).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CombatPreview {
    /// Expected HP loss of the attacker and of the defender.
    pub attacker_damage: i16,
    pub defender_damage: i16,
    /// Strength times 100, as in the core.
    pub attacker_strength: i32,
    pub defender_strength: i32,
    pub defender_hp: i16,
    pub defender_max_hp: i16,
    /// `CombatPreview::KILLS`, `CAPTURES`, `CITY`.
    pub flags: u16,
}

impl CombatPreview {
    pub const KILLS: u16 = 1 << 0;
    pub const CAPTURES: u16 = 1 << 1;
    pub const CITY: u16 = 1 << 2;
}

/// Build list and targets of the city in focus.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CityContext {
    pub city: i32,
    /// Set by the gateway.
    pub view_seq: u64,
    pub builds: Vec<CityBuildView>,
    pub targets: Vec<TargetView>,
}

/// `GameMsg::Context` payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextMsg {
    Unit(UnitContext),
    City(CityContext),
    /// The contexts of these units and this city did not change and are valid for this view too.
    Valid {
        units: Vec<i32>,
        city: Option<i32>,
        view_seq: u64,
    },
    /// The unit is gone or cannot act any more.
    Dropped {
        unit: i32,
    },
}
