//! Test doubles of [`GameRead`].

use std::collections::BTreeMap;

use crate::read::{
    CatalogEntry, CatalogTable, City, CityBuild, CityOrder, GameInfo, GameRead, GameStatus,
    Notification, Player, PlayerResource, PolicyBranchState, ReadError, TechState, TopBar,
    TopBarFlags, Unit, UnitAction,
};

/// A [`GameRead`] over plain data that records every call. `error`, when set, is the answer of every read
/// (the call is still recorded); `ui2_error`, when set, is the answer of the five reads of the UI v2 queries
/// (`city_queue`, `techs`, `player_resources`, `policy_branches`, `unit_promotions`), e.g.
/// `ReadError::Unsupported` for a core without them.
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
    /// `city_queue` by city id.
    pub queues: BTreeMap<i32, Vec<CityOrder>>,
    pub techs: Vec<TechState>,
    pub resources: Vec<PlayerResource>,
    pub branches: Vec<PolicyBranchState>,
    /// `unit_promotions` by unit id.
    pub promotions: BTreeMap<i32, Vec<i16>>,
    pub error: Option<ReadError>,
    pub ui2_error: Option<ReadError>,
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
            queues: BTreeMap::new(),
            techs: Vec::new(),
            resources: Vec::new(),
            branches: Vec::new(),
            promotions: BTreeMap::new(),
            error: None,
            ui2_error: None,
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

    /// A call of one of the UI v2 queries: `error` first, then `ui2_error`.
    fn call_ui2(&mut self, name: &'static str) -> Result<(), ReadError> {
        self.call(name)?;
        match &self.ui2_error {
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
        self.with_world(|read| read.units())
    }

    fn unit_actions(&mut self, unit: i32) -> Result<Vec<UnitAction>, ReadError> {
        self.call("unit_actions")?;
        Ok(self.actions.get(&unit).cloned().unwrap_or_default())
    }

    fn cities(&mut self) -> Result<Vec<City>, ReadError> {
        self.call("cities")?;
        self.with_world(|read| read.cities())
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
        self.with_world(|read| read.notifications())
    }

    fn city_queue(&mut self, city: i32) -> Result<Vec<CityOrder>, ReadError> {
        self.call_ui2("city_queue")?;
        Ok(self.queues.get(&city).cloned().unwrap_or_default())
    }

    fn techs(&mut self) -> Result<Vec<TechState>, ReadError> {
        self.call_ui2("techs")?;
        Ok(self.techs.clone())
    }

    fn player_resources(&mut self) -> Result<Vec<PlayerResource>, ReadError> {
        self.call_ui2("player_resources")?;
        Ok(self.resources.clone())
    }

    fn policy_branches(&mut self) -> Result<Vec<PolicyBranchState>, ReadError> {
        self.call_ui2("policy_branches")?;
        Ok(self.branches.clone())
    }

    fn unit_promotions(&mut self, unit: i32) -> Result<Vec<i16>, ReadError> {
        self.call_ui2("unit_promotions")?;
        Ok(self.promotions.get(&unit).cloned().unwrap_or_default())
    }
}

/// A `Q_GAME` record of turn 0 with no blocker, the humans moving, nothing won.
#[must_use]
pub fn game_info() -> GameInfo {
    GameInfo::default()
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
        tourism: Some(0),
    }
}

/// A catalog row with a name and an icon.
#[must_use]
pub fn catalog_entry(id: i16, type_name: &str, name: &str, atlas: &str, icon: i16) -> CatalogEntry {
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

/// Fixture availability, independent of a local engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Live,
    Resyncing,
}

/// Events used to script historical fixture sequences.
#[derive(Debug, Clone, Copy)]
pub enum Change {
    Loaded { turn: i32 },
    TurnSwitched { turn: i32 },
    Applied { jseq: u64 },
    Idle,
    Working,
    Resyncing,
}

impl FakeRead {
    /// A protocol world fixture. The projection tests exercise the production world reader.
    #[must_use]
    pub fn shown(&self) -> protocol::WorldView {
        let info = &self.info;
        let t = &self.top;
        protocol::WorldView {
            turn: info.turn,
            year: info.year,
            human: self.seat.cast_signed(),
            human_team: self.team as i8,
            status: match info.status {
                GameStatus::GameOver => protocol::TurnStatus::GameOver {
                    winner: (info.winner >= 0).then_some(info.winner),
                },
                GameStatus::AiRunning => protocol::TurnStatus::AiTurn {
                    active: info.active_player,
                },
                _ => protocol::TurnStatus::HumanTurn,
            },
            blocker: (info.blocker >= 0).then_some(protocol::Blocker {
                kind: i32::from(info.blocker),
                target: if info.blocker_unit >= 0 {
                    protocol::BlockerTarget::Unit(info.blocker_unit)
                } else if info.blocker_city >= 0 {
                    protocol::BlockerTarget::City(info.blocker_city)
                } else if let Some((x, y)) = info.blocker_plot() {
                    protocol::BlockerTarget::Plot { x, y }
                } else {
                    protocol::BlockerTarget::None
                },
            }),
            map: protocol::MapView {
                width: info.map_width as u16,
                height: info.map_height as u16,
                wrap_x: info.wrap_x,
                wrap_y: info.wrap_y,
                ..Default::default()
            },
            units: self
                .units
                .iter()
                .map(|u| protocol::UnitView {
                    id: u.id,
                    owner: u.owner,
                    unit_type: u.unit_type,
                    x: u.x,
                    y: u.y,
                    hp: u.hp,
                    max_hp: u.max_hp,
                    moves_left: u.moves_left,
                    max_moves: u.max_moves,
                    strength: u.strength,
                    ranged: u.ranged,
                    range: u.range,
                    domain: u.domain,
                    activity: u.activity.abi(),
                    level: u.level,
                    experience: u.experience,
                    flags: protocol::UnitFlags(u.flags.0),
                    name: u.name.clone(),
                    facing: u.facing.map_or(-1, |d| d as i8),
                    cargo_of: u.transport_id.unwrap_or(-1),
                    building: u.build_type.unwrap_or(-1),
                })
                .collect(),
            cities: self
                .cities
                .iter()
                .map(|c| protocol::CityView {
                    id: c.id,
                    owner: c.owner,
                    x: c.x,
                    y: c.y,
                    name: c.name.clone(),
                    population: c.population,
                    flags: protocol::CityFlags(c.flags.0),
                    hp: c.hp,
                    max_hp: c.max_hp,
                    defense: c.defense,
                    production_order: c.production_order.map_or(-1, |o| o.abi() as i8),
                    production_item: c.production_item.unwrap_or(-1),
                    production_turns: c.production_turns.unwrap_or(-1),
                    production_stored: c.production_stored,
                    production_needed: c.production_needed,
                    growth_turns: c.growth_turns.unwrap_or(-1),
                    yields: c.yields,
                    buildings: Vec::new(),
                    founder: c.founder.map_or(-1, |p| p as i8),
                })
                .collect(),
            players: self
                .players
                .iter()
                .map(|p| protocol::PlayerView {
                    id: p.id,
                    team: p.team,
                    civ_type: p.civ_type.unwrap_or(-1),
                    leader_type: p.leader_type.unwrap_or(-1),
                    flags: protocol::PlayerFlags(p.flags.0),
                    color_primary: p.color_primary,
                    color_secondary: p.color_secondary,
                    name: p.name.clone(),
                    civ_name: p.civ_name.clone(),
                    era: p.era.unwrap_or(-1),
                    art_techs: p.art_techs,
                })
                .collect(),
            top_bar: protocol::TopBar {
                gold: t.gold,
                gold_per_turn: t.gold_per_turn,
                science_per_turn: t.science_per_turn,
                research_tech: t.research_tech.unwrap_or(-1),
                research_turns: t.research_turns,
                research_progress: t.research_progress,
                research_cost: t.research_cost,
                culture: t.culture,
                culture_per_turn: t.culture_per_turn,
                next_policy_cost: t.next_policy_cost,
                happiness: t.happiness,
                golden_age_turns: t.golden_age_turns,
                faith: t.faith,
                faith_per_turn: t.faith_per_turn,
                era: t.era,
                golden_age_progress: t.golden_age_progress,
                golden_age_threshold: t.golden_age_threshold,
            },
            notifications: self
                .notes
                .iter()
                .map(|n| protocol::NotificationView {
                    id: n.id,
                    kind: n.kind,
                    at: n.plot(),
                    data1: n.data1,
                    data2: n.data2,
                    turn: n.turn,
                    dismissed: n.dismissed,
                    message: n.message.clone(),
                    summary: n.summary.clone(),
                })
                .collect(),
            peace_offers: Vec::new(),
            trade_routes: Vec::new(),
        }
    }

    fn with_world<T>(&self, f: impl FnOnce(&mut crate::world::WorldRead<'_>) -> T) -> T {
        let world = self.shown();
        let queries = crate::world::WorldSource::default();
        let mut read = crate::world::WorldRead {
            world: &world,
            catalog: None,
            seats: &[],
            queries: &queries,
            extras: self,
        };
        f(&mut read)
    }
}

impl crate::world::HudExtras for FakeRead {
    fn city_overflow(&self, city: i32) -> Option<i32> {
        self.cities
            .iter()
            .find(|c| c.id == city)
            .and_then(|c| c.overflow)
    }
    fn tourism(&self) -> Option<i32> {
        self.top.tourism
    }
}
