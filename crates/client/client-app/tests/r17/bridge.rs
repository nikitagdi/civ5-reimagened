//! Test bridge for auditing a classic replica while the HUD consumes independent records.
use ui2_source::read as hud;

trait Convert {
    type Output;
    fn convert(self) -> Self::Output;
}
impl<T: Convert> Convert for Option<T> {
    type Output = Option<T::Output>;
    fn convert(self) -> Self::Output {
        self.map(Convert::convert)
    }
}
impl<T: Convert> Convert for Vec<T> {
    type Output = Vec<T::Output>;
    fn convert(self) -> Self::Output {
        self.into_iter().map(Convert::convert).collect()
    }
}
impl Convert for replica::OrderType {
    type Output = hud::OrderType;
    fn convert(self) -> Self::Output {
        match self {
            replica::OrderType::Train => hud::OrderType::Train,
            replica::OrderType::Construct => hud::OrderType::Construct,
            replica::OrderType::Create => hud::OrderType::Create,
            replica::OrderType::Maintain => hud::OrderType::Maintain,
            replica::OrderType::Other(v) => hud::OrderType::Other(v),
        }
    }
}
impl Convert for replica::Activity {
    type Output = hud::Activity;
    fn convert(self) -> Self::Output {
        match self {
            replica::Activity::Awake => hud::Activity::Awake,
            replica::Activity::Hold => hud::Activity::Hold,
            replica::Activity::Sleep => hud::Activity::Sleep,
            replica::Activity::Heal => hud::Activity::Heal,
            replica::Activity::Sentry => hud::Activity::Sentry,
            replica::Activity::Intercept => hud::Activity::Intercept,
            replica::Activity::Mission => hud::Activity::Mission,
            replica::Activity::Other(v) => hud::Activity::Other(v),
        }
    }
}
impl Convert for replica::UnitRow {
    type Output = hud::UnitRow;
    fn convert(self) -> Self::Output {
        match self {
            replica::UnitRow::Alert => hud::UnitRow::Alert,
            replica::UnitRow::Pillage => hud::UnitRow::Pillage,
            replica::UnitRow::Upgrade => hud::UnitRow::Upgrade,
        }
    }
}
impl Convert for replica::TechStatus {
    type Output = hud::TechStatus;
    fn convert(self) -> Self::Output {
        match self {
            replica::TechStatus::Have => hud::TechStatus::Have,
            replica::TechStatus::CanResearch => hud::TechStatus::CanResearch,
            replica::TechStatus::Locked => hud::TechStatus::Locked,
            replica::TechStatus::Other(v) => hud::TechStatus::Other(v),
        }
    }
}
impl Convert for replica::ResourceUsage {
    type Output = hud::ResourceUsage;
    fn convert(self) -> Self::Output {
        match self {
            replica::ResourceUsage::Bonus => hud::ResourceUsage::Bonus,
            replica::ResourceUsage::Strategic => hud::ResourceUsage::Strategic,
            replica::ResourceUsage::Luxury => hud::ResourceUsage::Luxury,
            replica::ResourceUsage::Other(v) => hud::ResourceUsage::Other(v),
        }
    }
}
impl Convert for replica::TurnMode {
    type Output = hud::TurnMode;
    fn convert(self) -> Self::Output {
        match self {
            replica::TurnMode::Simultaneous => hud::TurnMode::Simultaneous,
            replica::TurnMode::Hybrid => hud::TurnMode::Hybrid,
        }
    }
}
impl Convert for replica::GameStatus {
    type Output = hud::GameStatus;
    fn convert(self) -> Self::Output {
        match self {
            replica::GameStatus::WaitingHuman => hud::GameStatus::WaitingHuman,
            replica::GameStatus::AiRunning => hud::GameStatus::AiRunning,
            replica::GameStatus::GameOver => hud::GameStatus::GameOver,
            replica::GameStatus::Stuck => hud::GameStatus::Stuck,
            replica::GameStatus::Other(v) => hud::GameStatus::Other(v),
        }
    }
}
impl Convert for replica::Phase {
    type Output = hud::Phase;
    fn convert(self) -> Self::Output {
        match self {
            replica::Phase::Humans => hud::Phase::Humans,
            replica::Phase::Ai => hud::Phase::Ai,
        }
    }
}
impl Convert for replica::UnsafeReason {
    type Output = hud::UnsafeReason;
    fn convert(self) -> Self::Output {
        match self {
            replica::UnsafeReason::AiRunning => hud::UnsafeReason::AiRunning,
            replica::UnsafeReason::CommandGiven => hud::UnsafeReason::CommandGiven,
            replica::UnsafeReason::AiRequestOpen => hud::UnsafeReason::AiRequestOpen,
            replica::UnsafeReason::Automoves => hud::UnsafeReason::Automoves,
            replica::UnsafeReason::Other(v) => hud::UnsafeReason::Other(v),
        }
    }
}
impl Convert for replica::CatalogTable {
    type Output = hud::CatalogTable;
    fn convert(self) -> Self::Output {
        match self {
            replica::CatalogTable::Units => hud::CatalogTable::Units,
            replica::CatalogTable::Buildings => hud::CatalogTable::Buildings,
            replica::CatalogTable::Projects => hud::CatalogTable::Projects,
            replica::CatalogTable::Technologies => hud::CatalogTable::Technologies,
            replica::CatalogTable::Policies => hud::CatalogTable::Policies,
            replica::CatalogTable::Terrains => hud::CatalogTable::Terrains,
            replica::CatalogTable::Features => hud::CatalogTable::Features,
            replica::CatalogTable::Resources => hud::CatalogTable::Resources,
            replica::CatalogTable::Improvements => hud::CatalogTable::Improvements,
            replica::CatalogTable::Routes => hud::CatalogTable::Routes,
            replica::CatalogTable::Civilizations => hud::CatalogTable::Civilizations,
            replica::CatalogTable::Leaders => hud::CatalogTable::Leaders,
            replica::CatalogTable::Eras => hud::CatalogTable::Eras,
            replica::CatalogTable::Handicaps => hud::CatalogTable::Handicaps,
            replica::CatalogTable::Promotions => hud::CatalogTable::Promotions,
            replica::CatalogTable::ArtTechs => hud::CatalogTable::ArtTechs,
            replica::CatalogTable::GameSpeeds => hud::CatalogTable::GameSpeeds,
            replica::CatalogTable::PolicyBranches => hud::CatalogTable::PolicyBranches,
        }
    }
}
impl Convert for replica::GameType {
    type Output = hud::GameType;
    fn convert(self) -> Self::Output {
        match self {
            replica::GameType::Single => hud::GameType::Single,
            replica::GameType::Network => hud::GameType::Network,
        }
    }
}
impl Convert for replica::UnitFlags {
    type Output = hud::UnitFlags;
    fn convert(self) -> Self::Output {
        hud::UnitFlags(self.0)
    }
}
impl Convert for replica::Unit {
    type Output = hud::Unit;
    fn convert(self) -> Self::Output {
        hud::Unit {
            id: self.id,
            owner: self.owner,
            unit_type: self.unit_type,
            x: self.x,
            y: self.y,
            hp: self.hp,
            max_hp: self.max_hp,
            moves_left: self.moves_left,
            max_moves: self.max_moves,
            strength: self.strength,
            ranged: self.ranged,
            range: self.range,
            domain: self.domain,
            activity: self.activity.convert(),
            level: self.level,
            experience: self.experience,
            flags: self.flags.convert(),
            transport_id: self.transport_id,
            build_type: self.build_type,
            facing: self
                .facing
                .and_then(|d| hud::Direction::ALL.get(d as usize).copied()),
            name: self.name,
        }
    }
}
impl Convert for replica::UnitAction {
    type Output = hud::UnitAction;
    fn convert(self) -> Self::Output {
        hud::UnitAction {
            action: self.action,
            enabled: self.enabled,
            reason: self.reason,
            cost: self.cost,
        }
    }
}
impl Convert for replica::Notification {
    type Output = hud::Notification;
    fn convert(self) -> Self::Output {
        hud::Notification {
            id: self.id,
            kind: self.kind,
            x: self.x,
            y: self.y,
            data1: self.data1,
            data2: self.data2,
            turn: self.turn,
            dismissed: self.dismissed,
            message: self.message,
            summary: self.summary,
        }
    }
}
impl Convert for replica::CityFlags {
    type Output = hud::CityFlags;
    fn convert(self) -> Self::Output {
        hud::CityFlags(self.0)
    }
}
impl Convert for replica::City {
    type Output = hud::City;
    fn convert(self) -> Self::Output {
        hud::City {
            id: self.id,
            owner: self.owner,
            production_order: self.production_order.convert(),
            x: self.x,
            y: self.y,
            population: self.population,
            flags: self.flags.convert(),
            hp: self.hp,
            max_hp: self.max_hp,
            defense: self.defense,
            production_item: self.production_item,
            production_turns: self.production_turns,
            growth_turns: self.growth_turns,
            production_stored: self.production_stored,
            production_needed: self.production_needed,
            yields: self.yields,
            founder: self.founder,
            overflow: self.overflow,
            name: self.name,
        }
    }
}
impl Convert for replica::TopBarFlags {
    type Output = hud::TopBarFlags;
    fn convert(self) -> Self::Output {
        hud::TopBarFlags(self.0)
    }
}
impl Convert for replica::TopBar {
    type Output = hud::TopBar;
    fn convert(self) -> Self::Output {
        hud::TopBar {
            gold: self.gold,
            gold_per_turn: self.gold_per_turn,
            science_per_turn: self.science_per_turn,
            research_tech: self.research_tech,
            research_turns: self.research_turns,
            research_progress: self.research_progress,
            research_cost: self.research_cost,
            culture: self.culture,
            culture_per_turn: self.culture_per_turn,
            next_policy_cost: self.next_policy_cost,
            happiness: self.happiness,
            golden_age_turns: self.golden_age_turns,
            faith: self.faith,
            faith_per_turn: self.faith_per_turn,
            era: self.era,
            flags: self.flags.convert(),
            golden_age_progress: self.golden_age_progress,
            golden_age_threshold: self.golden_age_threshold,
            tourism: self.tourism,
        }
    }
}
impl Convert for replica::BuildFlags {
    type Output = hud::BuildFlags;
    fn convert(self) -> Self::Output {
        hud::BuildFlags(self.0)
    }
}
impl Convert for replica::CityBuild {
    type Output = hud::CityBuild;
    fn convert(self) -> Self::Output {
        hud::CityBuild {
            order: self.order.convert(),
            item: self.item,
            turns: self.turns,
            flags: self.flags.convert(),
            production_cost: self.production_cost,
            gold_cost: self.gold_cost,
            faith_cost: self.faith_cost,
            reason: self.reason,
        }
    }
}
impl Convert for replica::CityOrder {
    type Output = hud::CityOrder;
    fn convert(self) -> Self::Output {
        hud::CityOrder {
            slot: self.slot,
            order: self.order.convert(),
            item: self.item,
            turns: self.turns,
            needed: self.needed,
        }
    }
}
impl Convert for replica::TechState {
    type Output = hud::TechState;
    fn convert(self) -> Self::Output {
        hud::TechState {
            tech: self.tech,
            status: self.status.convert(),
            queue: self.queue,
            turns: self.turns,
            progress: self.progress,
            cost: self.cost,
        }
    }
}
impl Convert for replica::PlayerResource {
    type Output = hud::PlayerResource;
    fn convert(self) -> Self::Output {
        hud::PlayerResource {
            resource: self.resource,
            usage: self.usage.convert(),
            available: self.available,
            used: self.used,
            total: self.total,
        }
    }
}
impl Convert for replica::PolicyBranchState {
    type Output = hud::PolicyBranchState;
    fn convert(self) -> Self::Output {
        hud::PolicyBranchState {
            branch: self.branch,
            unlocked: self.unlocked,
            owned: self.owned,
            total: self.total,
        }
    }
}
impl Convert for replica::GameInfo {
    type Output = hud::GameInfo;
    fn convert(self) -> Self::Output {
        hud::GameInfo {
            turn: self.turn,
            year: self.year,
            blocker_unit: self.blocker_unit,
            blocker_city: self.blocker_city,
            blocker_x: self.blocker_x,
            blocker_y: self.blocker_y,
            map_width: self.map_width,
            map_height: self.map_height,
            human_player: self.human_player,
            human_team: self.human_team,
            active_player: self.active_player,
            status: self.status.convert(),
            blocker: self.blocker,
            wrap_x: self.wrap_x,
            wrap_y: self.wrap_y,
            difficulty: self.difficulty,
            winner: self.winner,
            victory_type: self.victory_type,
            game_type: self.game_type.convert(),
            turn_mode: self.turn_mode.convert(),
            phase: self.phase.convert(),
            humans: self.humans,
            humans_left: self.humans_left,
            safe_point: self.safe_point,
            unsafe_reason: self.unsafe_reason.convert(),
            turn_len_s: self.turn_len_s,
            turn_base_s: self.turn_base_s,
            map_seed: self.map_seed,
        }
    }
}
impl Convert for replica::PlayerFlags {
    type Output = hud::PlayerFlags;
    fn convert(self) -> Self::Output {
        hud::PlayerFlags(self.0)
    }
}
impl Convert for replica::Player {
    type Output = hud::Player;
    fn convert(self) -> Self::Output {
        hud::Player {
            id: self.id,
            team: self.team,
            civ_type: self.civ_type,
            leader_type: self.leader_type,
            flags: self.flags.convert(),
            color_primary: self.color_primary,
            color_secondary: self.color_secondary,
            name: self.name,
            civ_name: self.civ_name,
            era: self.era,
            nick: self.nick,
            slot: self.slot,
            art_techs: self.art_techs,
        }
    }
}
impl Convert for replica::CatalogEntry {
    type Output = hud::CatalogEntry;
    fn convert(self) -> Self::Output {
        hud::CatalogEntry {
            id: self.id,
            icon_index: self.icon_index,
            flag_index: self.flag_index,
            type_name: self.type_name,
            name: self.name,
            art_define: self.art_define,
            icon_atlas: self.icon_atlas,
            flag_atlas: self.flag_atlas,
            art_style: self.art_style,
            strategic_view: self.strategic_view,
        }
    }
}

pub struct Bridge<'a>(pub &'a mut dyn replica::GameRead);
fn error(e: replica::ReadError) -> hud::ReadError {
    match e {
        replica::ReadError::Unsupported => hud::ReadError::Unsupported,
        replica::ReadError::Core(e) => hud::ReadError::Core(e),
    }
}
impl hud::GameRead for Bridge<'_> {
    fn seat(&self) -> u8 {
        self.0.seat()
    }
    fn team(&self) -> i32 {
        self.0.team()
    }
    fn game_info(&mut self) -> Result<hud::GameInfo, hud::ReadError> {
        self.0.game_info().map(Convert::convert).map_err(error)
    }
    fn players(&mut self) -> Result<Vec<hud::Player>, hud::ReadError> {
        self.0.players().map(Convert::convert).map_err(error)
    }
    fn catalog(
        &mut self,
        table: hud::CatalogTable,
    ) -> Result<Vec<hud::CatalogEntry>, hud::ReadError> {
        self.0
            .catalog(match table {
                hud::CatalogTable::Units => replica::CatalogTable::Units,
                hud::CatalogTable::Buildings => replica::CatalogTable::Buildings,
                hud::CatalogTable::Projects => replica::CatalogTable::Projects,
                hud::CatalogTable::Technologies => replica::CatalogTable::Technologies,
                hud::CatalogTable::Policies => replica::CatalogTable::Policies,
                hud::CatalogTable::Terrains => replica::CatalogTable::Terrains,
                hud::CatalogTable::Features => replica::CatalogTable::Features,
                hud::CatalogTable::Resources => replica::CatalogTable::Resources,
                hud::CatalogTable::Improvements => replica::CatalogTable::Improvements,
                hud::CatalogTable::Routes => replica::CatalogTable::Routes,
                hud::CatalogTable::Civilizations => replica::CatalogTable::Civilizations,
                hud::CatalogTable::Leaders => replica::CatalogTable::Leaders,
                hud::CatalogTable::Eras => replica::CatalogTable::Eras,
                hud::CatalogTable::Handicaps => replica::CatalogTable::Handicaps,
                hud::CatalogTable::Promotions => replica::CatalogTable::Promotions,
                hud::CatalogTable::ArtTechs => replica::CatalogTable::ArtTechs,
                hud::CatalogTable::GameSpeeds => replica::CatalogTable::GameSpeeds,
                hud::CatalogTable::PolicyBranches => replica::CatalogTable::PolicyBranches,
            })
            .map(Convert::convert)
            .map_err(error)
    }
    fn units(&mut self) -> Result<Vec<hud::Unit>, hud::ReadError> {
        self.0.units().map(Convert::convert).map_err(error)
    }
    fn unit_actions(&mut self, id: i32) -> Result<Vec<hud::UnitAction>, hud::ReadError> {
        self.0.unit_actions(id).map(Convert::convert).map_err(error)
    }
    fn cities(&mut self) -> Result<Vec<hud::City>, hud::ReadError> {
        self.0.cities().map(Convert::convert).map_err(error)
    }
    fn city_builds(&mut self, id: i32) -> Result<Vec<hud::CityBuild>, hud::ReadError> {
        self.0.city_builds(id).map(Convert::convert).map_err(error)
    }
    fn top_bar(&mut self) -> Result<hud::TopBar, hud::ReadError> {
        self.0.top_bar().map(Convert::convert).map_err(error)
    }
    fn notifications(&mut self) -> Result<Vec<hud::Notification>, hud::ReadError> {
        self.0.notifications().map(Convert::convert).map_err(error)
    }
    fn city_queue(&mut self, id: i32) -> Result<Vec<hud::CityOrder>, hud::ReadError> {
        self.0.city_queue(id).map(Convert::convert).map_err(error)
    }
    fn techs(&mut self) -> Result<Vec<hud::TechState>, hud::ReadError> {
        self.0.techs().map(Convert::convert).map_err(error)
    }
    fn player_resources(&mut self) -> Result<Vec<hud::PlayerResource>, hud::ReadError> {
        self.0
            .player_resources()
            .map(Convert::convert)
            .map_err(error)
    }
    fn policy_branches(&mut self) -> Result<Vec<hud::PolicyBranchState>, hud::ReadError> {
        self.0
            .policy_branches()
            .map(Convert::convert)
            .map_err(error)
    }
    fn unit_promotions(&mut self, id: i32) -> Result<Vec<i16>, hud::ReadError> {
        self.0.unit_promotions(id).map_err(error)
    }
}
