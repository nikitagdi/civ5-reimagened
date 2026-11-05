//! The entity table: what the UI has been told about units, cities and fog, and the reconciliation after a load
//! ("Согласование после смены хода/resync").
//!
//! The turn of the AI does not run on the replica, so there are no engine callbacks for it; after a snapshot load
//! `setup_graphical` announces everything alive (`rebuild` events). Against this table a "created" for a known
//! unit becomes the difference (moved, damaged, ...), and what the table has but the announcement lacks is
//! destroyed. An unchanged game gives no events at all.
//!
//! SIMPLIFIED(SIMP-exe-5): the turn of the AI is seen as a jump: units of the AI "teleport" to where they are, with
//! no animation (there is none in B2 either).

use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UnitRec {
    pub x: i32,
    pub y: i32,
    pub damage: i32,
    pub embarked: bool,
    pub garrisoned: bool,
    pub dim: bool,
    pub visible: bool,
    pub fog: i32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CityRec {
    pub x: i32,
    pub y: i32,
    pub population: i32,
    pub damage: i32,
}

#[derive(Debug, Default)]
pub struct EntityTable {
    pub units: BTreeMap<(i32, i32), UnitRec>,
    pub cities: BTreeMap<(i32, i32), CityRec>,
    pub fog: HashMap<(i32, i32), i32>,
    /// The game turn the UI last heard of (`TURN_CHANGED`, `PUBLISH_NEW_GAME_TURN`).
    pub turn: Option<i32>,
    /// The culture (art style) of each owner, from the unit blocks: the city block of the core has none, and
    /// `SerialEventCityCreated` passes it.
    pub culture: HashMap<i32, i32>,
    /// The head of the unit selection the UI was last told of (`UnitSelectionChanged`).
    pub selected: Option<(i32, i32)>,
}

/// What a reconciliation found, in the order the events have to be fired.
#[derive(Debug, Default, PartialEq)]
pub struct Seen {
    pub units: BTreeSet<(i32, i32)>,
    pub cities: BTreeSet<(i32, i32)>,
}

/// Units and cities the table has that an announcement lacks.
#[derive(Debug, Default)]
pub struct Missing {
    pub units: Vec<((i32, i32), UnitRec)>,
    pub cities: Vec<((i32, i32), CityRec)>,
}

impl EntityTable {
    pub fn clear(&mut self) {
        self.units.clear();
        self.cities.clear();
        self.fog.clear();
    }

    /// Units and cities the table has that `seen` lacks.
    pub fn missing(&self, seen: &Seen) -> Missing {
        let units = self
            .units
            .iter()
            .filter(|(k, _)| !seen.units.contains(k))
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        let cities = self
            .cities
            .iter()
            .filter(|(k, _)| !seen.cities.contains(k))
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        Missing { units, cities }
    }
}
