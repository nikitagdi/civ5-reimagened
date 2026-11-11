//! A read-only view of the replica's game for the UI v2 source.
//!
//! [`GameRead`] is the only way the new UI reaches the core: every method is one typed query of the core
//! (`gc_query`), none of them changes the game. The viewer (seat and team) is fixed by the facade, so a caller
//! cannot look through another player's eyes. Only [`Replica::read`](crate::Replica::read) hands one out, and only
//! while the replica follows the journal.
//!
//! The record types are `gamecore`'s; they are re-exported here because a client crate other than `replica` may
//! not depend on `gamecore` (rule R10).

pub use gamecore::{
    Activity, BuildFlags, CatalogEntry, CatalogTable, City, CityBuild, CityFlags, GameInfo,
    GameStatus, GameType, Notification, OrderType, Phase, Player, PlayerFlags, TopBar, TopBarFlags,
    TurnMode, Unit, UnitAction, UnitFlags, UnsafeReason,
};

use gamecore::{GameError, GcError};

/// Why a read failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    /// The linked core does not serve this query (the stub, or a core older than the query).
    #[error("the core does not serve this query")]
    Unsupported,
    /// The core failed or its reply was malformed.
    #[error("core read failed: {0}")]
    Core(String),
}

impl From<GameError> for ReadError {
    fn from(e: GameError) -> Self {
        match e {
            GameError::Core(GcError::NotImplemented) => ReadError::Unsupported,
            other => ReadError::Core(other.to_string()),
        }
    }
}

/// The game as the replica's seat sees it, read only. Each method is one query of the core.
pub trait GameRead {
    /// The seat (player id) of the replica.
    fn seat(&self) -> u8;
    /// The seat's team as of the last [`game_info`](Self::game_info), -1 before the first one.
    fn team(&self) -> i32;
    /// `Q_GAME` for the seat (turn, year, end-turn blocker, phase, status).
    fn game_info(&mut self) -> Result<GameInfo, ReadError>;
    /// `Q_PLAYERS` as the seat's team sees them.
    fn players(&mut self) -> Result<Vec<Player>, ReadError>;
    /// `Q_CATALOG`: static reference data of one table.
    fn catalog(&mut self, table: CatalogTable) -> Result<Vec<CatalogEntry>, ReadError>;
    /// `Q_UNITS` the seat's team sees.
    fn units(&mut self) -> Result<Vec<Unit>, ReadError>;
    /// `Q_UNIT_ACTIONS` of one of the seat's units.
    fn unit_actions(&mut self, unit: i32) -> Result<Vec<UnitAction>, ReadError>;
    /// `Q_CITIES` the seat's team sees.
    fn cities(&mut self) -> Result<Vec<City>, ReadError>;
    /// `Q_CITY_BUILDS` of one of the seat's cities.
    fn city_builds(&mut self, city: i32) -> Result<Vec<CityBuild>, ReadError>;
    /// `Q_TOP_BAR` of the seat.
    fn top_bar(&mut self) -> Result<TopBar, ReadError>;
    /// `Q_NOTIFICATIONS` of the seat.
    fn notifications(&mut self) -> Result<Vec<Notification>, ReadError>;
}

/// Test doubles of [`GameRead`].
pub mod testing {
    use std::collections::BTreeMap;

    use super::{
        CatalogEntry, CatalogTable, City, CityBuild, GameInfo, GameRead, GameStatus, GameType,
        Notification, Phase, Player, ReadError, TopBar, TopBarFlags, Unit, UnitAction,
    };

    /// A [`GameRead`] over plain data that records every call. `error`, when set, is the answer of every read
    /// (the call is still recorded).
    #[derive(Debug, Clone)]
    pub struct FakeRead {
        pub seat: u8,
        pub team: i32,
        pub info: GameInfo,
        pub players: Vec<Player>,
        pub catalogs: Vec<(CatalogTable, Vec<CatalogEntry>)>,
        pub units: Vec<Unit>,
        pub actions: BTreeMap<i32, Vec<UnitAction>>,
        pub cities: Vec<City>,
        pub builds: BTreeMap<i32, Vec<CityBuild>>,
        pub top: TopBar,
        pub notes: Vec<Notification>,
        pub error: Option<ReadError>,
        /// The name of every method called, in order (`seat` and `team` are not calls of the core).
        pub calls: Vec<&'static str>,
    }

    impl FakeRead {
        /// An empty game of turn 0 seen by `seat` of `team`.
        #[must_use]
        pub fn new(seat: u8, team: i32) -> FakeRead {
            FakeRead {
                seat,
                team,
                info: game_info(),
                players: Vec::new(),
                catalogs: Vec::new(),
                units: Vec::new(),
                actions: BTreeMap::new(),
                cities: Vec::new(),
                builds: BTreeMap::new(),
                top: top_bar(),
                notes: Vec::new(),
                error: None,
                calls: Vec::new(),
            }
        }

        /// Reads made so far.
        #[must_use]
        pub fn call_count(&self) -> usize {
            self.calls.len()
        }

        fn call(&mut self, name: &'static str) -> Result<(), ReadError> {
            self.calls.push(name);
            match &self.error {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            }
        }
    }

    impl GameRead for FakeRead {
        fn seat(&self) -> u8 {
            self.seat
        }

        fn team(&self) -> i32 {
            self.team
        }

        fn game_info(&mut self) -> Result<GameInfo, ReadError> {
            self.call("game_info")?;
            Ok(self.info.clone())
        }

        fn players(&mut self) -> Result<Vec<Player>, ReadError> {
            self.call("players")?;
            Ok(self.players.clone())
        }

        fn catalog(&mut self, table: CatalogTable) -> Result<Vec<CatalogEntry>, ReadError> {
            self.call("catalog")?;
            Ok(self
                .catalogs
                .iter()
                .find(|(t, _)| *t == table)
                .map(|(_, rows)| rows.clone())
                .unwrap_or_default())
        }

        fn units(&mut self) -> Result<Vec<Unit>, ReadError> {
            self.call("units")?;
            Ok(self.units.clone())
        }

        fn unit_actions(&mut self, unit: i32) -> Result<Vec<UnitAction>, ReadError> {
            self.call("unit_actions")?;
            Ok(self.actions.get(&unit).cloned().unwrap_or_default())
        }

        fn cities(&mut self) -> Result<Vec<City>, ReadError> {
            self.call("cities")?;
            Ok(self.cities.clone())
        }

        fn city_builds(&mut self, city: i32) -> Result<Vec<CityBuild>, ReadError> {
            self.call("city_builds")?;
            Ok(self.builds.get(&city).cloned().unwrap_or_default())
        }

        fn top_bar(&mut self) -> Result<TopBar, ReadError> {
            self.call("top_bar")?;
            Ok(self.top.clone())
        }

        fn notifications(&mut self) -> Result<Vec<Notification>, ReadError> {
            self.call("notifications")?;
            Ok(self.notes.clone())
        }
    }

    /// A `Q_GAME` record of turn 0 with no blocker, the humans moving, nothing won.
    #[must_use]
    pub fn game_info() -> GameInfo {
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

    /// A `Q_TOP_BAR` record of all zeros with no research chosen.
    #[must_use]
    pub fn top_bar() -> TopBar {
        TopBar {
            gold: 0,
            gold_per_turn: 0,
            science_per_turn: 0,
            research_tech: None,
            research_turns: -1,
            research_progress: 0,
            research_cost: 0,
            culture: 0,
            culture_per_turn: 0,
            next_policy_cost: 0,
            happiness: 0,
            golden_age_turns: 0,
            faith: 0,
            faith_per_turn: 0,
            era: 0,
            flags: TopBarFlags::default(),
            golden_age_progress: 0,
            golden_age_threshold: 0,
        }
    }

    /// A catalog row with a name and an icon.
    #[must_use]
    pub fn catalog_entry(
        id: i16,
        type_name: &str,
        name: &str,
        atlas: &str,
        icon: i16,
    ) -> CatalogEntry {
        CatalogEntry {
            id,
            icon_index: Some(icon),
            flag_index: None,
            type_name: type_name.into(),
            name: name.into(),
            art_define: None,
            icon_atlas: Some(atlas.into()),
            flag_atlas: None,
            art_style: None,
            strategic_view: None,
        }
    }
}
