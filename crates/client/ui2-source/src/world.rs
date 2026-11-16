//! HUD reads of the server-synced world, including the client's optimistic overlay.

use std::collections::BTreeMap;

use client_core::ClientCore;
use protocol::{BlockerTarget, Query, QueryAnswer, TurnStatus};
use ui2_model::Key;
use ui2_model::generated::{Ui, WinCity};

use crate::read::*;

/// Additional HUD facts supplied by a server screen. Unsupported facts retain model defaults.
/// SIMPLIFIED(SIMP-310): the HUD screen is supplied separately, so its fields remain unknown here.
pub trait HudExtras {
    /// Changes whenever asynchronous extra facts change. Unsupported is permanent for one provider.
    fn revision(&self) -> u64 {
        0
    }
    fn policy_branch_catalog(&self) -> Vec<CatalogEntry> {
        Vec::new()
    }
    fn city_queue(&self, _city: i32) -> Result<Vec<CityOrder>, ReadError> {
        Err(ReadError::Unsupported)
    }
    fn city_build_reason(&self, _city: i32, _order: OrderType, _item: i16) -> Option<String> {
        None
    }
    fn city_overflow(&self, _city: i32) -> Option<i32> {
        None
    }
    fn techs(&self) -> Result<Vec<TechState>, ReadError> {
        Err(ReadError::Unsupported)
    }
    fn player_resources(&self) -> Result<Vec<PlayerResource>, ReadError> {
        Err(ReadError::Unsupported)
    }
    fn policy_branches(&self) -> Result<Vec<PolicyBranchState>, ReadError> {
        Err(ReadError::Unsupported)
    }
    fn unit_promotions(&self, _unit: i32) -> Result<Vec<i16>, ReadError> {
        Err(ReadError::Unsupported)
    }
    fn unit_actions(&self, _unit: i32) -> Vec<UnitAction> {
        Vec::new()
    }
    fn tourism(&self) -> Option<i32> {
        None
    }
}

#[derive(Default)]
pub struct NoHudExtras;
impl HudExtras for NoHudExtras {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Request {
    Unit(i32),
    City(i32),
}

struct Answer {
    revision: u64,
    result: QueryAnswer,
}

/// Query answers belong to a shown-world revision. Responses from older revisions are discarded.
#[derive(Default)]
pub struct WorldSource {
    revision: Option<u64>,
    serial: u64,
    selection: Option<Key>,
    production: WinCity,
    connected: bool,
    prepared: bool,
    pending: BTreeMap<u32, (Request, u64)>,
    answers: BTreeMap<Request, Answer>,
}

impl WorldSource {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn answer(&mut self, id: u32, result: &QueryAnswer) {
        if let Some((request, revision)) = self.pending.remove(&id)
            && self.revision == Some(revision)
        {
            self.answers.insert(
                request,
                Answer {
                    revision,
                    result: result.clone(),
                },
            );
            self.serial = self.serial.wrapping_add(1);
        }
    }

    /// Called after intents, so reads and query invalidation see orders applied in this frame.
    pub fn prepare(&mut self, core: &mut ClientCore, ui: &Ui) -> bool {
        let revision = core.revision();
        let connected = core.orders_enabled();
        if self.prepared
            && self.revision == Some(revision)
            && self.selection == ui.selection.unit
            && self.production == ui.windows.production
            && self.connected == connected
        {
            return false;
        }
        let connection_changed = self.connected != connected;
        self.prepared = true;
        self.selection = ui.selection.unit;
        self.production.clone_from(&ui.windows.production);
        self.connected = connected;
        if self.revision != Some(revision) || connection_changed {
            self.revision = Some(revision);
            self.answers.clear();
            self.pending.clear();
            self.serial = self.serial.wrapping_add(1);
        }
        let Some(world) = core.shown() else {
            return true;
        };
        let mut requests = Vec::new();
        if let Some(key) = ui.selection.unit
            && let Some(unit) = world
                .units
                .iter()
                .find(|u| crate::build::entity_key(u.owner, u.id) == key && u.owner == world.human)
        {
            requests.push(Request::Unit(unit.id));
        }
        if ui.windows.production.open {
            requests.extend(
                world
                    .cities
                    .iter()
                    .filter(|c| c.owner == world.human)
                    .map(|c| Request::City(c.id)),
            );
        }
        for request in requests {
            if self.answers.contains_key(&request)
                || self.pending.values().any(|(r, _)| *r == request)
            {
                continue;
            }
            let query = match request {
                Request::Unit(unit) => Query::UnitActions { unit },
                Request::City(city) => Query::CityBuilds { city },
            };
            let id = core.query(query);
            if id != 0 {
                self.pending.insert(id, (request, revision));
            }
        }
        true
    }

    #[must_use]
    pub fn revision(&self, core: &ClientCore) -> u64 {
        core.revision().wrapping_add(self.serial)
    }

    pub fn reader<'a>(
        &'a self,
        core: &'a ClientCore,
        extras: &'a dyn HudExtras,
    ) -> Option<WorldRead<'a>> {
        Some(WorldRead {
            world: core.shown()?,
            catalog: core.catalog(),
            seats: core.seats(),
            queries: self,
            extras,
        })
    }
}

/// Builder input borrowed from the shown view and revision-scoped query results.
pub struct WorldRead<'a> {
    pub world: &'a protocol::WorldView,
    pub catalog: Option<&'a protocol::Catalog>,
    pub seats: &'a [protocol::SeatView],
    pub queries: &'a WorldSource,
    pub extras: &'a dyn HudExtras,
}

fn optional<T: PartialOrd + Default + Copy>(value: T) -> Option<T> {
    (value >= T::default()).then_some(value)
}

impl GameRead for WorldRead<'_> {
    fn seat(&self) -> u8 {
        self.world.human.cast_unsigned()
    }
    fn team(&self) -> i32 {
        i32::from(self.world.human_team)
    }
    fn game_info(&mut self) -> Result<GameInfo, ReadError> {
        let w = self.world;
        let mut info = GameInfo {
            turn: w.turn,
            year: w.year,
            human_player: w.human,
            human_team: w.human_team,
            active_player: w.human,
            humans: u8::try_from(
                w.players
                    .iter()
                    .filter(|p| {
                        p.flags.contains(protocol::PlayerFlags::HUMAN)
                            && p.flags.contains(protocol::PlayerFlags::ALIVE)
                    })
                    .count(),
            )
            .unwrap_or(u8::MAX),
            map_width: i16::try_from(w.map.width).unwrap_or(i16::MAX),
            map_height: i16::try_from(w.map.height).unwrap_or(i16::MAX),
            wrap_x: w.map.wrap_x,
            wrap_y: w.map.wrap_y,
            blocker: w.blocker.map_or(-1, |b| i8::try_from(b.kind).unwrap_or(-1)),
            ..Default::default()
        };
        info.humans_left = info.humans;
        if let Some(blocker) = w.blocker {
            match blocker.target {
                BlockerTarget::Unit(id) => info.blocker_unit = id,
                BlockerTarget::City(id) => info.blocker_city = id,
                BlockerTarget::Plot { x, y } => {
                    info.blocker_x = x;
                    info.blocker_y = y;
                }
                BlockerTarget::None => {}
            }
        }
        info.status = match w.status {
            TurnStatus::HumanTurn => GameStatus::WaitingHuman,
            TurnStatus::AiTurn { active } => {
                info.active_player = active;
                info.phase = Phase::Ai;
                GameStatus::AiRunning
            }
            TurnStatus::GameOver { winner } => {
                info.winner = winner.unwrap_or(-1);
                GameStatus::GameOver
            }
        };
        Ok(info)
    }
    fn players(&mut self) -> Result<Vec<Player>, ReadError> {
        Ok(self
            .world
            .players
            .iter()
            .map(|p| Player {
                id: p.id,
                team: p.team,
                civ_type: optional(p.civ_type),
                leader_type: optional(p.leader_type),
                flags: PlayerFlags(p.flags.0),
                color_primary: p.color_primary,
                color_secondary: p.color_secondary,
                name: p.name.clone(),
                civ_name: p.civ_name.clone(),
                era: optional(p.era),
                nick: self
                    .seats
                    .iter()
                    .find(|s| s.player == p.id)
                    .and_then(|s| match &s.control {
                        protocol::SeatControl::Human { nick } => Some(nick.clone()),
                        _ => None,
                    }),
                slot: 1,
                art_techs: p.art_techs,
            })
            .collect())
    }
    fn catalog(&mut self, table: CatalogTable) -> Result<Vec<CatalogEntry>, ReadError> {
        let Some(c) = self.catalog else {
            return Err(ReadError::Core("catalog pending".into()));
        };
        let entries = match table {
            CatalogTable::Units => &c.units,
            CatalogTable::Buildings => &c.buildings,
            CatalogTable::Projects => &c.projects,
            CatalogTable::Technologies => &c.techs,
            CatalogTable::Resources => &c.resources,
            CatalogTable::Eras => &c.eras,
            CatalogTable::Promotions => &c.promotions,
            CatalogTable::PolicyBranches => return Ok(self.extras.policy_branch_catalog()),
            _ => return Ok(Vec::new()),
        };
        Ok(entries
            .iter()
            .map(|e| CatalogEntry {
                id: e.id,
                type_name: e.type_name.clone(),
                name: e.name.clone(),
                art_define: None,
                icon_atlas: (!e.art.icon_atlas.is_empty()).then(|| e.art.icon_atlas.clone()),
                icon_index: optional(e.art.icon_index),
                flag_atlas: None,
                flag_index: None,
                art_style: None,
                strategic_view: None,
            })
            .collect())
    }
    fn units(&mut self) -> Result<Vec<Unit>, ReadError> {
        Ok(self
            .world
            .units
            .iter()
            .map(|u| Unit {
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
                activity: Activity::from_abi(u.activity),
                level: u.level,
                experience: u.experience,
                flags: UnitFlags(u.flags.0 | if u.cargo_of >= 0 { 0x200 } else { 0 }),
                transport_id: optional(u.cargo_of),
                build_type: optional(u.building),
                facing: usize::try_from(u.facing)
                    .ok()
                    .and_then(|i| Direction::ALL.get(i).copied()),
                name: u.name.clone(),
            })
            .collect())
    }
    fn unit_actions(&mut self, unit: i32) -> Result<Vec<UnitAction>, ReadError> {
        let mut rows = Vec::new();
        if let Some(a) = self.queries.answers.get(&Request::Unit(unit))
            && Some(a.revision) == self.queries.revision
        {
            match &a.result {
                QueryAnswer::UnitActions(actions) => {
                    rows.extend(actions.iter().map(|a| UnitAction {
                        action: a.action,
                        enabled: a.enabled,
                        reason: (!a.reason.is_empty()).then(|| a.reason.clone()),
                        cost: None,
                    }))
                }
                QueryAnswer::Error { reason } => return Err(ReadError::Core(reason.clone())),
                _ => return Err(ReadError::Core("unexpected unit actions answer".into())),
            }
        }
        rows.extend(self.extras.unit_actions(unit));
        Ok(rows)
    }
    fn cities(&mut self) -> Result<Vec<City>, ReadError> {
        Ok(self
            .world
            .cities
            .iter()
            .map(|c| City {
                id: c.id,
                owner: c.owner,
                production_order: optional(c.production_order).map(OrderType::from_abi),
                x: c.x,
                y: c.y,
                population: c.population,
                flags: CityFlags(c.flags.0),
                hp: c.hp,
                max_hp: c.max_hp,
                defense: c.defense,
                production_item: optional(c.production_item),
                production_turns: optional(c.production_turns),
                growth_turns: optional(c.growth_turns),
                production_stored: c.production_stored,
                production_needed: c.production_needed,
                yields: c.yields,
                founder: u8::try_from(c.founder).ok(),
                overflow: self.extras.city_overflow(c.id),
                name: c.name.clone(),
            })
            .collect())
    }
    fn city_builds(&mut self, city: i32) -> Result<Vec<CityBuild>, ReadError> {
        let a = self
            .queries
            .answers
            .get(&Request::City(city))
            .ok_or_else(|| ReadError::Core("city builds pending".into()))?;
        match &a.result {
            QueryAnswer::CityBuilds(rows) => Ok(rows
                .iter()
                .map(|r| CityBuild {
                    order: OrderType::from_abi(r.order_type),
                    item: r.item,
                    turns: optional(r.turns),
                    flags: BuildFlags(r.flags.0),
                    production_cost: r.production_cost,
                    gold_cost: optional(r.gold_cost),
                    faith_cost: optional(r.faith_cost),
                    reason: self.extras.city_build_reason(
                        city,
                        OrderType::from_abi(r.order_type),
                        r.item,
                    ),
                })
                .collect()),
            QueryAnswer::Error { reason } => Err(ReadError::Core(reason.clone())),
            _ => Err(ReadError::Core("unexpected city builds answer".into())),
        }
    }
    fn top_bar(&mut self) -> Result<TopBar, ReadError> {
        let t = &self.world.top_bar;
        Ok(TopBar {
            gold: t.gold,
            gold_per_turn: t.gold_per_turn,
            science_per_turn: t.science_per_turn,
            research_tech: optional(t.research_tech),
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
            // SIMPLIFIED(SIMP-311): WorldView omits mood flags, so the standard -10 happiness threshold is used.
            flags: TopBarFlags(
                if t.happiness <= -10 {
                    3
                } else {
                    u16::from(t.happiness < 0)
                } | if t.golden_age_turns > 0 { 4 } else { 0 },
            ),
            golden_age_progress: t.golden_age_progress,
            golden_age_threshold: t.golden_age_threshold,
            tourism: self.extras.tourism(),
        })
    }
    fn notifications(&mut self) -> Result<Vec<Notification>, ReadError> {
        Ok(self
            .world
            .notifications
            .iter()
            .map(|n| Notification {
                id: n.id,
                kind: n.kind,
                x: n.at.map_or(-1, |p| p.0),
                y: n.at.map_or(-1, |p| p.1),
                data1: n.data1,
                data2: n.data2,
                turn: n.turn,
                dismissed: n.dismissed,
                message: n.message.clone(),
                summary: n.summary.clone(),
            })
            .collect())
    }
    fn city_queue(&mut self, city: i32) -> Result<Vec<CityOrder>, ReadError> {
        self.extras.city_queue(city)
    }
    fn techs(&mut self) -> Result<Vec<TechState>, ReadError> {
        self.extras.techs()
    }
    fn player_resources(&mut self) -> Result<Vec<PlayerResource>, ReadError> {
        self.extras.player_resources()
    }
    fn policy_branches(&mut self) -> Result<Vec<PolicyBranchState>, ReadError> {
        self.extras.policy_branches()
    }
    fn unit_promotions(&mut self, unit: i32) -> Result<Vec<i16>, ReadError> {
        self.extras.unit_promotions(unit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_old_query_answer_cannot_refresh_a_new_world() {
        let mut source = WorldSource {
            revision: Some(2),
            ..Default::default()
        };
        source.pending.insert(7, (Request::Unit(4), 1));
        source.answer(7, &QueryAnswer::UnitActions(Vec::new()));
        assert!(source.answers.is_empty());
        assert_eq!(source.serial, 0);
        source.pending.insert(8, (Request::Unit(4), 2));
        source.answer(8, &QueryAnswer::UnitActions(Vec::new()));
        assert!(source.answers.contains_key(&Request::Unit(4)));
        assert_eq!(source.serial, 1);
        source.answer(8, &QueryAnswer::UnitActions(Vec::new()));
        assert_eq!(source.serial, 1, "a duplicate answer has no effect");
    }

    #[test]
    fn build_query_prices_and_unknown_queue_are_projected_without_an_engine() {
        let fixture = crate::testing::FakeRead::new(0, 0);
        let world = fixture.shown();
        let mut source = WorldSource {
            revision: Some(1),
            ..Default::default()
        };
        source.pending.insert(1, (Request::City(3), 1));
        source.answer(
            1,
            &QueryAnswer::CityBuilds(vec![protocol::CityBuildView {
                order_type: 0,
                item: 2,
                turns: 4,
                production_cost: 60,
                gold_cost: 200,
                faith_cost: -1,
                flags: protocol::CityBuildFlags(protocol::CityBuildFlags::ENABLED),
            }]),
        );
        let mut read = WorldRead {
            world: &world,
            catalog: None,
            seats: &[],
            queries: &source,
            extras: &NoHudExtras,
        };
        let builds = read.city_builds(3).expect("builds");
        assert_eq!(builds[0].gold_cost, Some(200));
        assert_eq!(builds[0].faith_cost, None);
        assert_eq!(builds[0].turns, Some(4));
        assert_eq!(read.city_queue(3), Err(ReadError::Unsupported));
        assert_eq!(read.unit_promotions(4), Err(ReadError::Unsupported));
    }
}
