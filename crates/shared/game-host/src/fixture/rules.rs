//! Rules of the test world (game-host-mp 4.14): orders per player, the turn (humans, the AI phase of two
//! steps, the hybrid order), the move tree and its queries, production, the research of the one art tech,
//! the save FIXT v3. There is no combat readiness (user decision 2026-10-21). A development tool, not a
//! game simplification.

use std::collections::BTreeSet;

use protocol::{
    AiTurnEvent, CityBuildFlags, CityBuildView, Codec, CombatPreview, Command, HumanWar,
    JournalEntry, JournalKind, PathNode, PlayerId, PlotFlags, PlotType, Postcard, ReachFlags,
    ReachNode, SeatTurnInfo, Shooter, TurnMode, UnitActionView, UnitFlags, UnitView,
};

use super::map::{BARBARIAN, F_MARSH, IMPROVEMENTS};
use super::seats::{
    self, RESEARCH_COST, RESEARCH_PER_TURN, U_ARCHER, U_HORSEMAN, U_SCOUT, U_SETTLER, U_WARRIOR,
    U_WORKER,
};
use super::{Game, GamePhase};
use crate::core::RefusalCode;
use crate::core::{CoreError, CoreErrorKind, CoreEvent, CorePhase, CorePump, EventTarget, Refusal};

/// Magic and version of the save of the test world (4: the build queues and the research choice).
pub const SAVE_MAGIC: &[u8; 4] = b"FIXT";
pub const SAVE_VERSION: u16 = 4;
/// Entries a build queue may hold: every slot must be addressable by the protocol's `i8`.
pub const QUEUE_SLOTS: usize = 128;

/// Moves a step costs (1/60 of a move).
pub const MOVE_COST: i16 = 60;
/// Depth of the move tree in steps.
const TREE_DEPTH: u8 = 2;
/// HP a shot takes off its target.
pub const SHOT_DAMAGE: i16 = 3;
/// HP a melee fight takes off the defender and the attacker.
pub const MELEE_DAMAGE: (i16, i16) = (4, 2);
/// HP the units heal at the start of a turn.
const HEAL: i16 = 2;
/// `TargetView::CITY`.
const TARGET_CITY: u16 = 1;
/// Order types of a city (`production_order`): train a unit, construct a building.
pub const ORDER_UNIT: i8 = 0;
pub const ORDER_BUILDING: i8 = 1;
/// Units a city can train.
pub const BUILD_UNITS: [i16; 6] = [
    U_SETTLER, U_WORKER, U_WARRIOR, U_ARCHER, U_HORSEMAN, U_SCOUT,
];

fn unit_cost(unit_type: i16) -> i32 {
    match unit_type {
        U_SETTLER => 60,
        U_WARRIOR | U_SCOUT => 20,
        U_HORSEMAN => 40,
        _ => 30,
    }
}

fn building_cost(building: i16) -> i32 {
    [40, 60, 50].get(building as usize).copied().unwrap_or(50)
}

/// Gold price of a city order: the fixture sells its buildings (twice the production cost), no units.
fn gold_cost(order: i8, item: i16) -> Option<i32> {
    (order == ORDER_BUILDING && (0..seats::BUILDINGS.len() as i16).contains(&item))
        .then(|| building_cost(item) * 2)
}

/// Production cost of the city order.
pub fn production_cost(order: i8, item: i16) -> i32 {
    if order == ORDER_BUILDING {
        building_cost(item)
    } else {
        unit_cost(item)
    }
}

/// Recounts `production_needed` and `production_turns` of a city from its order and stored production.
pub fn sync_production(c: &mut protocol::CityView) {
    if c.production_order < 0 {
        // An empty queue builds nothing.
        c.production_needed = 0;
        c.production_turns = -1;
        return;
    }
    c.production_needed = production_cost(c.production_order, c.production_item);
    let prod = i32::from(c.yields[1]).max(1);
    let left = (c.production_needed - c.production_stored).max(0);
    c.production_turns = ((left + prod - 1) / prod) as i16;
}

/// `BUILDING_MONUMENT` as "Monument".
fn pretty(type_name: &str) -> String {
    let word = type_name
        .split_once('_')
        .map_or(type_name, |t| t.1)
        .to_lowercase();
    let mut chars = word.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn refuse<T>(text: impl AsRef<str>) -> Result<T, Refusal> {
    Err(Refusal::other(format!("fixture: {}", text.as_ref())))
}

/// The refusal of a v15 order whose check fails (`RejectCode::Invalid`, as the real host answers it).
fn invalid<T>(text: impl AsRef<str>) -> Result<T, Refusal> {
    Err(Refusal::invalid(format!("fixture: {}", text.as_ref())))
}

/// The index of a protocol slot in a queue of `len` entries.
fn slot_index(slot: i8, len: usize) -> Result<usize, Refusal> {
    match usize::try_from(slot) {
        Ok(i) if i < len => Ok(i),
        _ => invalid(format!("no slot {slot} in a queue of {len}")),
    }
}

fn failed<T>(text: impl AsRef<str>) -> Result<T, CoreError> {
    Err(CoreError::failed(format!("fixture: {}", text.as_ref())))
}

/// What a fight or a shot is aimed at: a unit or a city, by `(owner, id)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Unit(PlayerId, i32),
    City(PlayerId, i32),
}

fn set_moves(u: &mut UnitView, left: i16) {
    u.moves_left = left.max(0);
    if u.moves_left > 0 {
        u.flags = UnitFlags(u.flags.0 | UnitFlags::CAN_MOVE);
    } else {
        u.flags = UnitFlags(u.flags.0 & !(UnitFlags::CAN_MOVE | UnitFlags::NEEDS_ORDERS));
    }
}

fn is_unit_order(cmd: &Command) -> bool {
    matches!(
        cmd,
        Command::UnitMoveTo { .. }
            | Command::UnitRangeAttack { .. }
            | Command::CityRangeAttack { .. }
            | Command::UnitFortify { .. }
            | Command::UnitSleep { .. }
            | Command::UnitSkip { .. }
            | Command::UnitWake { .. }
            | Command::CitySetProduction { .. }
            | Command::CityPurchase { .. }
    )
}

// ---------------------------------------------------------------- seats and the turn

impl Game {
    fn is_human(&self, p: PlayerId) -> bool {
        self.major(p).is_some_and(seats::Major::is_human)
    }

    fn humans(&self) -> Vec<PlayerId> {
        self.majors
            .iter()
            .filter(|m| m.is_human())
            .map(|m| m.player)
            .collect()
    }

    fn has_human_war(&self, p: PlayerId) -> bool {
        self.majors
            .iter()
            .any(|m| m.is_human() && m.player != p && self.dip.at_war(p, m.player))
    }

    /// The seat moves with the others: always in simultaneous mode, in hybrid mode while it is at war with
    /// no human.
    fn is_simultaneous(&self, p: PlayerId) -> bool {
        self.settings.turn_mode == TurnMode::Simultaneous || !self.has_human_war(p)
    }

    /// Hybrid: of the humans at war with a human, the lowest player id that has not ended its turn.
    fn active_sequential(&self) -> Option<PlayerId> {
        self.humans()
            .into_iter()
            .filter(|p| !self.is_simultaneous(*p) && !self.finished.contains(p))
            .min()
    }

    fn may_act(&self, p: PlayerId) -> bool {
        self.is_simultaneous(p) || self.active_sequential() == Some(p)
    }

    /// One entry per human seat (`Core::turn`).
    pub(super) fn seat_infos(&self) -> Vec<SeatTurnInfo> {
        self.humans()
            .into_iter()
            .map(|p| SeatTurnInfo {
                player: p,
                finished: self.finished.contains(&p),
                may_act: self.may_act(p),
                simultaneous: self.is_simultaneous(p),
            })
            .collect()
    }

    /// Everybody who is human has ended the turn: the AI phase begins. With no human at all the game waits.
    pub(super) fn begin_ai_if_done(&mut self) {
        if self.phase != GamePhase::Humans {
            return;
        }
        let humans = self.humans();
        if !humans.is_empty() && humans.iter().all(|p| self.finished.contains(p)) {
            self.phase = GamePhase::Ai { stage: 0 };
            // The new turn's doTurn activates every AI at once (CvGame::doTurn, SIMP-exe-820).
            let ais = self.ai_players();
            self.turn_status
                .extend(ais.into_iter().map(|player| AiTurnEvent {
                    player,
                    ended: false,
                }));
        }
    }

    /// The non-human civilizations of the AI phase: the AI majors by id, then the barbarians.
    pub(super) fn ai_players(&self) -> Vec<PlayerId> {
        let mut v: Vec<PlayerId> = self
            .majors
            .iter()
            .filter(|m| !m.is_human())
            .map(|m| m.player)
            .collect();
        v.sort_unstable();
        v.push(BARBARIAN);
        v
    }

    fn end_ai_turns(&mut self, players: Vec<PlayerId>) {
        self.turn_status
            .extend(players.into_iter().map(|player| AiTurnEvent {
                player,
                ended: true,
            }));
    }

    /// The player who moves next in the AI phase: the first AI major, then the barbarians.
    pub(super) fn active_ai(&self) -> PlayerId {
        match self.phase {
            GamePhase::Ai { stage: 0 } => self
                .majors
                .iter()
                .find(|m| !m.is_human())
                .map_or(BARBARIAN, |m| m.player),
            _ => BARBARIAN,
        }
    }

    /// Runs the AI phase by one step: the AI majors move; then the barbarians move and the next turn
    /// begins.
    pub(super) fn pump(&mut self) -> CorePump {
        self.kick_pending = false;
        self.begin_ai_if_done();
        // `ai_step_delay_ms`: every AI step first waits, without blocking the worker for long, so the AI
        // phase stays open for orders and joins (the phase is reported unchanged until the wait is over).
        if matches!(self.phase, GamePhase::Ai { .. }) && self.hooks.ai_step_delay_ms > 0 {
            let began = *self.ai_wait.get_or_insert_with(std::time::Instant::now);
            let delay = std::time::Duration::from_millis(u64::from(self.hooks.ai_step_delay_ms));
            if began.elapsed() < delay {
                std::thread::sleep(std::time::Duration::from_millis(1));
                return CorePump {
                    phase: CorePhase::Ai {
                        active: self.active_ai(),
                    },
                    turn: self.turn,
                };
            }
            self.ai_wait = None;
        }
        match self.phase {
            GamePhase::Humans => {}
            GamePhase::Ai { stage: 0 } => {
                self.run_ai();
                let majors: Vec<PlayerId> = self
                    .ai_players()
                    .into_iter()
                    .filter(|p| *p != BARBARIAN)
                    .collect();
                self.end_ai_turns(majors);
                self.phase = GamePhase::Ai { stage: 1 };
            }
            GamePhase::Ai { .. } => {
                self.run_barbarians();
                self.end_ai_turns(vec![BARBARIAN]);
                self.start_turn();
                self.phase = GamePhase::Humans;
            }
        }
        CorePump {
            phase: match self.phase {
                GamePhase::Humans => CorePhase::Humans,
                GamePhase::Ai { .. } => CorePhase::Ai {
                    active: self.active_ai(),
                },
            },
            turn: self.turn,
        }
    }

    pub(super) fn note(
        &mut self,
        to: EventTarget,
        kind: JournalKind,
        text: String,
        at: Option<(i16, i16)>,
    ) {
        let entry = JournalEntry {
            turn: self.turn,
            kind,
            text,
            at,
            simplification: None,
        };
        self.events.push(CoreEvent { to, entry });
    }

    fn note_to(&mut self, p: PlayerId, kind: JournalKind, text: String, at: Option<(i16, i16)>) {
        // Only the majors have a journal.
        if self.major(p).is_some() {
            self.note(EventTarget::Player(p), kind, text, at);
        }
    }

    fn civ_of(&self, p: PlayerId) -> String {
        self.major(p)
            .map_or_else(|| "Barbarians".to_owned(), |m| m.civ_name().to_owned())
    }

    fn alloc_unit_id(&mut self, owner: PlayerId) -> i32 {
        let highest = self
            .units
            .iter()
            .filter(|u| u.owner == owner)
            .map(|u| u.id)
            .max()
            .unwrap_or(0);
        let next = self.unit_seq.entry(owner).or_insert(highest);
        *next = (*next).max(highest) + 1;
        *next
    }

    /// The start of a new turn: moves and HP back, orders wanted, production, growth, research, pillaged
    /// plots repaired, the test hook of the basis, fog.
    fn start_turn(&mut self) {
        self.turn += 1;
        self.finished.clear();
        let humans: BTreeSet<PlayerId> = self.humans().into_iter().collect();
        for u in &mut self.units {
            u.hp = (u.hp + HEAL).min(u.max_hp);
            let rest =
                u.flags.0 & (UnitFlags::FORTIFIED | UnitFlags::SLEEPING | UnitFlags::AUTOMATED);
            set_moves(u, u.max_moves);
            if u.max_moves > 0 && rest == 0 && humans.contains(&u.owner) {
                u.flags = UnitFlags(u.flags.0 | UnitFlags::NEEDS_ORDERS);
            }
        }
        self.grow_cities();
        let mut learned = Vec::new();
        for m in &mut self.majors {
            if !m.has_tech() {
                m.research = (m.research + RESEARCH_PER_TURN).min(RESEARCH_COST);
                if m.research >= RESEARCH_COST {
                    m.art_techs |= 1;
                    learned.push(m.player);
                }
            }
        }
        for p in learned {
            self.note_to(
                p,
                JournalKind::Notification,
                "Agriculture researched".into(),
                None,
            );
        }
        // A chosen research that is known now leaves the seat's research queue.
        for m in &self.majors {
            if let Some(q) = self.research.get_mut(&m.player) {
                q.retain(|&t| !seats::knows_tech(m, t));
            }
        }
        self.research.retain(|_, q| !q.is_empty());
        self.repair_plots();
        self.apply_basis_hook();
        self.apply_owner_hook();
        self.refresh_fog();
        let turn = self.turn;
        self.note(
            EventTarget::All,
            JournalKind::Notification,
            format!("Turn {turn} begins (fixture)"),
            None,
        );
    }

    /// Population, production and buildings of the cities.
    fn grow_cities(&mut self) {
        let mut built: Vec<(PlayerId, String, String, (i16, i16))> = Vec::new();
        let mut spawn: Vec<(PlayerId, i16, (i16, i16))> = Vec::new();
        for c in &mut self.cities {
            c.hp = c.max_hp;
            c.growth_turns -= 1;
            if c.growth_turns <= 0 {
                c.population += 1;
                c.growth_turns = 5 + c.population;
            }
            if c.production_order < 0 {
                // An empty queue: nothing is built and nothing is stored.
                sync_production(c);
                continue;
            }
            c.production_stored += i32::from(c.yields[1]);
            let needed = production_cost(c.production_order, c.production_item);
            if c.production_stored >= needed {
                c.production_stored -= needed;
                // The next entry of the queue, if any, follows what is done.
                let key = (c.owner, c.id);
                let next = self
                    .queues
                    .get_mut(&key)
                    .filter(|r| !r.is_empty())
                    .map(|r| r.remove(0));
                if self.queues.get(&key).is_some_and(Vec::is_empty) {
                    self.queues.remove(&key);
                }
                if c.production_order == ORDER_BUILDING {
                    if !c.buildings.contains(&c.production_item) {
                        c.buildings.push(c.production_item);
                    }
                    let name = seats::BUILDINGS
                        .get(c.production_item as usize)
                        .copied()
                        .unwrap_or("?");
                    built.push((c.owner, c.name.clone(), name.to_owned(), (c.x, c.y)));
                    c.production_order = ORDER_UNIT;
                    c.production_item = U_WARRIOR;
                } else {
                    let name = seats::UNITS
                        .get(c.production_item as usize)
                        .copied()
                        .unwrap_or("?");
                    built.push((c.owner, c.name.clone(), name.to_owned(), (c.x, c.y)));
                    spawn.push((c.owner, c.production_item, (c.x, c.y)));
                }
                // Without a queue a unit is trained again and a building gives way to a warrior (above).
                if let Some((order, item)) = next {
                    c.production_order = order;
                    c.production_item = item;
                }
            }
            sync_production(c);
        }
        for (owner, kind, at) in spawn {
            let id = self.alloc_unit_id(owner);
            self.units.push(seats::spawn_unit(id, owner, kind, at));
        }
        for (owner, city, what, at) in built {
            self.note_to(
                owner,
                JournalKind::Notification,
                format!("{city} completed {}", pretty(&what)),
                Some(at),
            );
        }
    }

    /// Plots whose improvement or road was pillaged are repaired once no enemy stands on them.
    fn repair_plots(&mut self) {
        let n = self.geo().plot_count() as u32;
        let geo = self.geo();
        for i in 0..n {
            let Some(p) = self.map.plot(i) else {
                continue;
            };
            let bits = PlotFlags::PILLAGED | PlotFlags::ROUTE_PILLAGED;
            if p.flags.0 & bits == 0 {
                continue;
            }
            let at = geo.xy(i);
            let owner = p.owner;
            let held = self
                .units
                .iter()
                .any(|u| (u.x, u.y) == at && u.owner != owner && self.dip.at_war(u.owner, owner));
            if held {
                continue;
            }
            if let Some(p) = self.map.plot_mut(i) {
                p.flags = PlotFlags(p.flags.0 & !bits);
            }
            self.dirty.insert(i);
        }
    }

    /// `fixture_test.flip_owner`: at the start of that turn the plot gets the owner (a border change); it is dirty.
    fn apply_owner_hook(&mut self) {
        let Some(f) = self.hooks.flip_owner.filter(|f| f.at_turn == self.turn) else {
            return;
        };
        if let Some(p) = self.map.plot_mut(f.plot) {
            p.owner = f.owner;
            self.dirty.insert(f.plot);
        }
    }

    /// `fixture_test.change_basis_at_turn`: at the start of that turn one open land plot becomes marsh, a
    /// change of the relief of the map basis (and of the plot record of every team that knows the plot).
    fn apply_basis_hook(&mut self) {
        if self.hooks.change_basis_at_turn != Some(self.turn) {
            return;
        }
        let n = self.geo().plot_count() as u32;
        for k in 0..n {
            let i = (n / 3 + k) % n;
            let Some(p) = self.map.plot(i) else {
                continue;
            };
            let free = p.plot_type == PlotType::Land
                && p.feature < 0
                && p.improvement < 0
                && p.flags.0
                    & (PlotFlags::HAS_CITY | PlotFlags::NATURAL_WONDER | PlotFlags::IS_LAKE)
                    == 0;
            if free && let Some(p) = self.map.plot_mut(i) {
                p.feature = F_MARSH;
                p.yields[0] = 1;
                self.dirty.insert(i);
                return;
            }
        }
    }

    // ------------------------------------------------------------ AI phase

    fn run_ai(&mut self) {
        let ai: Vec<PlayerId> = self
            .majors
            .iter()
            .filter(|m| !m.is_human())
            .map(|m| m.player)
            .collect();
        self.wander(&ai);
        self.refresh_fog();
    }

    fn run_barbarians(&mut self) {
        self.wander(&[BARBARIAN]);
        self.refresh_fog();
    }

    /// Each fighting unit of the owners that is not garrisoned takes one step, in a direction that follows
    /// the turn and the unit id.
    fn wander(&mut self, owners: &[PlayerId]) {
        let geo = self.geo();
        let mut movers: Vec<(PlayerId, i32)> = self
            .units
            .iter()
            .filter(|u| {
                owners.contains(&u.owner)
                    && u.max_moves > 0
                    && !u.flags.contains(UnitFlags::CIVILIAN)
                    && !u.flags.contains(UnitFlags::GARRISONED)
            })
            .map(|u| (u.owner, u.id))
            .collect();
        movers.sort_unstable();
        for (owner, id) in movers {
            let Some(i) = self.find_unit(owner, id) else {
                continue;
            };
            let from = (self.units[i].x, self.units[i].y);
            let first = (self.turn as usize + id as usize + owner as usize) % 6;
            for k in 0..6 {
                let Some(to) = geo.neighbor(from, ((first + k) % 6) as u8) else {
                    continue;
                };
                if !self.wander_ok(owner, to) {
                    continue;
                }
                let u = &mut self.units[i];
                (u.x, u.y) = to;
                u.flags = UnitFlags(u.flags.0 & !(UnitFlags::FORTIFIED | UnitFlags::SLEEPING));
                self.pillage(owner, to);
                break;
            }
        }
    }

    fn wander_ok(&self, owner: PlayerId, at: (i16, i16)) -> bool {
        let geo = self.geo();
        let Some(p) = self.map.plot(geo.index(at)) else {
            return false;
        };
        matches!(p.plot_type, PlotType::Land | PlotType::Hill)
            && !self.units.iter().any(|u| (u.x, u.y) == at)
            && !self
                .cities
                .iter()
                .any(|c| (c.x, c.y) == at && c.owner != owner)
    }

    /// A unit of `mover` stands on `at`: improvement and road of a plot of an enemy are pillaged.
    fn pillage(&mut self, mover: PlayerId, at: (i16, i16)) {
        let i = self.geo().index(at);
        let Some(p) = self.map.plot(i) else {
            return;
        };
        if p.owner < 0 || p.owner == mover || !self.dip.at_war(mover, p.owner) {
            return;
        }
        let (owner, improvement, route) = (p.owner, p.improvement, p.route);
        let mut flags = p.flags.0;
        let mut what = None;
        if (0..=2).contains(&improvement) && flags & PlotFlags::PILLAGED == 0 {
            flags |= PlotFlags::PILLAGED;
            what = IMPROVEMENTS.get(improvement as usize).copied();
        }
        if route >= 0 && flags & PlotFlags::ROUTE_PILLAGED == 0 {
            flags |= PlotFlags::ROUTE_PILLAGED;
            what = what.or(Some("ROUTE_ROAD"));
        }
        let Some(what) = what else {
            return;
        };
        if let Some(p) = self.map.plot_mut(i) {
            p.flags = PlotFlags(flags);
        }
        self.dirty.insert(i);
        self.note_to(
            owner,
            JournalKind::Notification,
            format!("Your {what} at ({}, {}) was pillaged", at.0, at.1),
            Some(at),
        );
    }

    // ------------------------------------------------------------ orders

    /// An order of a human (`Core::command`); `techs`: the technologies of the catalog the game serves.
    pub(super) fn apply_command(
        &mut self,
        player: PlayerId,
        cmd: &Command,
        techs: usize,
    ) -> Result<(), Refusal> {
        if !self.is_human(player) {
            return Err(Refusal::new(
                RefusalCode::NotHuman,
                format!("fixture: player {player} is not a human"),
            ));
        }
        if self.phase != GamePhase::Humans {
            let code = if matches!(cmd, Command::CancelEndTurn) {
                RefusalCode::TurnEnded
            } else {
                RefusalCode::AiRunning
            };
            return Err(Refusal::new(code, "fixture: the rivals are moving"));
        }
        match *cmd {
            Command::EndTurn { .. } => self.end_turn(player),
            Command::CancelEndTurn => self.cancel_end_turn(player),
            Command::ProposePeace { to } => self.propose_peace(player, to),
            Command::AnswerPeace { from, accept } => self.answer_peace(player, from, accept),
            Command::DeclareWar { team } => {
                self.seat_may_order(player, false)?;
                self.declare_war(player, team)
            }
            ref order if is_unit_order(order) => {
                self.seat_may_order(player, true)?;
                self.unit_order(player, order)
            }
            Command::CityQueuePush { .. }
            | Command::CityQueuePop { .. }
            | Command::CityQueueMove { .. }
            | Command::CityReplaceHead { .. }
            | Command::ResearchSet { .. } => {
                self.seat_may_order(player, true)?;
                self.v15_order(player, cmd, techs)
            }
            _ => refuse("command not supported"),
        }
    }

    /// The whole build queue of a city of `player`, head first; `None`: no such city.
    pub(super) fn queue_of(&self, player: PlayerId, city: i32) -> Option<Vec<(i8, i16)>> {
        let c = self
            .cities
            .iter()
            .find(|c| c.id == city && c.owner == player)?;
        let mut q = Vec::new();
        if c.production_order >= 0 {
            q.push((c.production_order, c.production_item));
        }
        q.extend(self.queues.get(&(player, city)).into_iter().flatten());
        Some(q)
    }

    /// Sets the whole build queue of a city: the head becomes its production (none for an empty queue),
    /// the rest is kept in `queues`.
    fn set_queue(&mut self, player: PlayerId, city: i32, mut q: Vec<(i8, i16)>) {
        let rest = if q.is_empty() {
            Vec::new()
        } else {
            q.split_off(1)
        };
        if let Some(c) = self
            .cities
            .iter_mut()
            .find(|c| c.id == city && c.owner == player)
        {
            (c.production_order, c.production_item) = q.first().copied().unwrap_or((-1, -1));
            sync_production(c);
        }
        if rest.is_empty() {
            self.queues.remove(&(player, city));
        } else {
            self.queues.insert((player, city), rest);
        }
    }

    /// `(order, item)` may join the queue `others` (the entries it will stand with) of the city: a unit the
    /// city trains, or a building it has neither built nor queued.
    fn check_queue_item(
        &self,
        player: PlayerId,
        city: i32,
        others: &[(i8, i16)],
        order: i8,
        item: i16,
    ) -> Result<(), Refusal> {
        let Some(c) = self
            .cities
            .iter()
            .find(|c| c.id == city && c.owner == player)
        else {
            return invalid(format!("player {player} has no city {city}"));
        };
        let ok = match order {
            ORDER_UNIT => BUILD_UNITS.contains(&item),
            ORDER_BUILDING => {
                (0..seats::BUILDINGS.len() as i16).contains(&item)
                    && !c.buildings.contains(&item)
                    && !others.contains(&(order, item))
            }
            _ => false,
        };
        if ok {
            Ok(())
        } else {
            invalid(format!(
                "city {city} cannot queue order {order} item {item}"
            ))
        }
    }

    /// The v15 queue and research orders, with the checks the real host makes (`RefusalCode::Invalid`).
    fn v15_order(&mut self, player: PlayerId, cmd: &Command, techs: usize) -> Result<(), Refusal> {
        if let Command::ResearchSet { tech, queue } = *cmd {
            if tech < 0 || usize::try_from(tech).is_ok_and(|t| t >= techs) {
                return invalid(format!("there is no technology {tech}"));
            }
            if self
                .major(player)
                .is_some_and(|m| seats::knows_tech(m, tech))
            {
                return invalid(format!("technology {tech} is known already"));
            }
            let q = self.research.entry(player).or_default();
            if !queue {
                q.clear();
            }
            if !q.contains(&tech) {
                q.push(tech);
            }
            return Ok(());
        }
        let city = match *cmd {
            Command::CityQueuePush { city, .. }
            | Command::CityQueuePop { city, .. }
            | Command::CityQueueMove { city, .. }
            | Command::CityReplaceHead { city, .. } => city,
            _ => return refuse("command not supported"),
        };
        let Some(mut q) = self.queue_of(player, city) else {
            return invalid(format!("player {player} has no city {city}"));
        };
        match *cmd {
            Command::CityQueuePush {
                order, item, front, ..
            } => {
                if q.len() >= QUEUE_SLOTS {
                    return invalid(format!("the queue of city {city} is full"));
                }
                self.check_queue_item(player, city, &q, order, item)?;
                if front {
                    q.insert(0, (order, item));
                } else {
                    q.push((order, item));
                }
            }
            Command::CityQueuePop { slot, .. } => {
                let i = slot_index(slot, q.len())?;
                q.remove(i);
            }
            Command::CityQueueMove { from, to, .. } => {
                let (i, j) = (slot_index(from, q.len())?, slot_index(to, q.len())?);
                if i == j {
                    return invalid(format!("slot {from} is moved onto itself"));
                }
                let e = q.remove(i);
                q.insert(j, e);
            }
            Command::CityReplaceHead { order, item, .. } => {
                if q.is_empty() {
                    return invalid(format!("the queue of city {city} is empty"));
                }
                self.check_queue_item(player, city, &q[1..], order, item)?;
                q[0] = (order, item);
            }
            _ => return refuse("command not supported"),
        }
        self.set_queue(player, city, q);
        Ok(())
    }

    /// The end of the seat's turn by the timer: no check of blockers or of whose turn it is.
    pub(super) fn timeout(&mut self, player: PlayerId) -> Result<(), Refusal> {
        if !self.is_human(player) {
            return Err(Refusal::new(
                RefusalCode::NotHuman,
                format!("fixture: player {player} is not a human"),
            ));
        }
        if self.phase != GamePhase::Humans {
            return Err(Refusal::new(
                RefusalCode::TurnEnded,
                "fixture: the turn is changing",
            ));
        }
        self.finished.insert(player);
        self.begin_ai_if_done();
        Ok(())
    }

    /// An order may be given: a seat that has ended its turn takes the end back (a simultaneous seat only;
    /// `cancels`), a hybrid seat that does not move now waits.
    fn seat_may_order(&mut self, p: PlayerId, cancels: bool) -> Result<(), Refusal> {
        if self.finished.contains(&p) {
            if !cancels || !self.is_simultaneous(p) {
                return Err(Refusal::new(
                    RefusalCode::TurnEnded,
                    "fixture: the turn of the seat is over",
                ));
            }
            self.finished.remove(&p);
        }
        if !self.may_act(p) {
            return Err(Refusal::new(
                RefusalCode::NotYourTurn,
                "fixture: another human moves now",
            ));
        }
        Ok(())
    }

    fn end_turn(&mut self, p: PlayerId) -> Result<(), Refusal> {
        if self.finished.contains(&p) {
            return Err(Refusal::new(
                RefusalCode::TurnEnded,
                "fixture: the turn of the seat is over",
            ));
        }
        if !self.may_act(p) {
            return Err(Refusal::new(
                RefusalCode::NotYourTurn,
                "fixture: another human moves now",
            ));
        }
        self.finished.insert(p);
        self.begin_ai_if_done();
        Ok(())
    }

    fn cancel_end_turn(&mut self, p: PlayerId) -> Result<(), Refusal> {
        if !self.finished.contains(&p) {
            return refuse("the turn is not ended");
        }
        if !self.is_simultaneous(p) {
            return Err(Refusal::new(
                RefusalCode::TurnEnded,
                "fixture: the next human is moving",
            ));
        }
        self.finished.remove(&p);
        Ok(())
    }

    fn declare_war(&mut self, player: PlayerId, target: PlayerId) -> Result<(), Refusal> {
        if target == player {
            return refuse("war on oneself");
        }
        if self.major(target).is_none() {
            return refuse("no such player");
        }
        if self.dip.at_war(player, target) {
            return refuse("already at war");
        }
        // Whether a side moves at once is judged before the war makes both of them take turns.
        let extends =
            self.is_human(target) && (self.is_simultaneous(player) || self.is_simultaneous(target));
        self.dip.declare_war(player, target);
        if extends {
            self.pending_wars.push(HumanWar {
                from: player,
                to: target,
            });
        }
        let text = format!(
            "{} declared war on {}",
            self.civ_of(player),
            self.civ_of(target)
        );
        self.note(EventTarget::All, JournalKind::War, text, None);
        Ok(())
    }

    fn propose_peace(&mut self, player: PlayerId, to: PlayerId) -> Result<(), Refusal> {
        if self.major(to).is_none() || to == player {
            return refuse("no such player");
        }
        if !self.dip.at_war(player, to) {
            return refuse("not at war");
        }
        Ok(())
    }

    fn answer_peace(
        &mut self,
        player: PlayerId,
        from: PlayerId,
        accept: bool,
    ) -> Result<(), Refusal> {
        if self.major(from).is_none() || from == player {
            return refuse("no such player");
        }
        if !self.dip.at_war(player, from) {
            return refuse("not at war");
        }
        if accept {
            self.dip.make_peace(player, from);
            let text = format!(
                "{} and {} made peace",
                self.civ_of(player),
                self.civ_of(from)
            );
            self.note(EventTarget::All, JournalKind::Diplomacy, text, None);
        }
        Ok(())
    }

    fn find_unit(&self, owner: PlayerId, id: i32) -> Option<usize> {
        self.units
            .iter()
            .position(|u| u.owner == owner && u.id == id)
    }

    fn own_unit(&self, player: PlayerId, id: i32) -> Result<usize, Refusal> {
        match self.find_unit(player, id) {
            Some(i) => Ok(i),
            None => refuse("no such unit"),
        }
    }

    fn unit_order(&mut self, player: PlayerId, cmd: &Command) -> Result<(), Refusal> {
        match *cmd {
            Command::UnitMoveTo { unit, x, y } => self.move_unit(player, unit, (x, y)),
            Command::UnitRangeAttack { unit, x, y } => {
                let i = self.own_unit(player, unit)?;
                let u = self.units[i].clone();
                if u.ranged <= 0 {
                    return refuse("no ranged attack");
                }
                if u.moves_left <= 0 {
                    return refuse("no moves left");
                }
                let t = self.shot_target(player, (u.x, u.y), i16::from(u.range), false, (x, y))?;
                self.strike(player, &u.name, (x, y), t, SHOT_DAMAGE, None);
                if let Some(i) = self.find_unit(player, unit) {
                    set_moves(&mut self.units[i], 0);
                }
                Ok(())
            }
            Command::CityRangeAttack { city, x, y } => {
                let Some(c) = self
                    .cities
                    .iter()
                    .find(|c| c.id == city && c.owner == player)
                    .cloned()
                else {
                    return refuse("no such city");
                };
                let t = self.shot_target(player, (c.x, c.y), 2, true, (x, y))?;
                self.strike(player, &c.name, (x, y), t, SHOT_DAMAGE, None);
                Ok(())
            }
            Command::UnitFortify { unit }
            | Command::UnitSleep { unit }
            | Command::UnitSkip { unit }
            | Command::UnitWake { unit } => {
                let i = self.own_unit(player, unit)?;
                let u = &mut self.units[i];
                let rest = UnitFlags::FORTIFIED | UnitFlags::SLEEPING;
                let f = u.flags.0 & !UnitFlags::NEEDS_ORDERS;
                u.flags = UnitFlags(match *cmd {
                    Command::UnitFortify { .. } => {
                        if u.flags.contains(UnitFlags::CIVILIAN) {
                            return refuse("civilians cannot fortify");
                        }
                        (f & !rest) | UnitFlags::FORTIFIED
                    }
                    Command::UnitSleep { .. } => (f & !rest) | UnitFlags::SLEEPING,
                    Command::UnitWake { .. } if u.max_moves > 0 => {
                        (f & !rest) | UnitFlags::NEEDS_ORDERS
                    }
                    Command::UnitWake { .. } => f & !rest,
                    _ => f,
                });
                Ok(())
            }
            Command::CitySetProduction { city, order, item } => {
                let Some(c) = self
                    .cities
                    .iter_mut()
                    .find(|c| c.id == city && c.owner == player)
                else {
                    return refuse("no such city");
                };
                let ok = match order {
                    ORDER_UNIT => BUILD_UNITS.contains(&item),
                    ORDER_BUILDING => {
                        (0..seats::BUILDINGS.len() as i16).contains(&item)
                            && !c.buildings.contains(&item)
                    }
                    _ => false,
                };
                if !ok {
                    return refuse("this cannot be built here");
                }
                c.production_order = order;
                c.production_item = item;
                sync_production(c);
                // As the core's CITY_SET_PRODUCTION (`pushOrder` with pop): the new item is the whole queue.
                self.queues.remove(&(player, city));
                Ok(())
            }
            Command::CityPurchase {
                city,
                order,
                item,
                yield_,
            } => {
                if i32::from(yield_) != gamecore_sys::ffi::GC_YIELD_GOLD {
                    return refuse("the fixture sells only for gold");
                }
                let Some(cost) = gold_cost(order, item) else {
                    return refuse("this cannot be bought");
                };
                let gold = self
                    .major(player)
                    .map_or(0, |m| seats::gold_at(self.turn, m));
                let Some(c) = self
                    .cities
                    .iter_mut()
                    .find(|c| c.id == city && c.owner == player)
                else {
                    return refuse("no such city");
                };
                if c.buildings.contains(&item) {
                    return refuse("this cannot be bought here");
                }
                if gold < cost {
                    return refuse("not enough gold");
                }
                c.buildings.push(item);
                // A bought building leaves the queue; what the city was building is done: the next entry of
                // the queue follows, else the default order.
                let rest = self.queues.entry((player, city)).or_default();
                rest.retain(|e| *e != (order, item));
                if c.production_order == order && c.production_item == item {
                    (c.production_order, c.production_item) = if rest.is_empty() {
                        (ORDER_UNIT, U_WARRIOR)
                    } else {
                        rest.remove(0)
                    };
                    sync_production(c);
                }
                if rest.is_empty() {
                    self.queues.remove(&(player, city));
                }
                if let Some(m) = self.majors.iter_mut().find(|m| m.player == player) {
                    m.gold_spent += cost;
                }
                Ok(())
            }
            _ => refuse("command not supported"),
        }
    }

    /// Moves along the move tree; onto a foe it is a melee fight.
    fn move_unit(&mut self, player: PlayerId, unit: i32, to: (i16, i16)) -> Result<(), Refusal> {
        let i = self.own_unit(player, unit)?;
        let u = self.units[i].clone();
        if u.moves_left <= 0 {
            return refuse("no moves left");
        }
        if !self.geo().contains(to) {
            return refuse("target outside the map");
        }
        let tree = self.move_tree(player, &u);
        let Some(node) = tree.iter().skip(1).find(|n| (n.x, n.y) == to).copied() else {
            return refuse("the plot is not in the move tree");
        };
        // What really stands there; the tree knows only what the player sees.
        if let Some(t) = self.target_at(player, to, false) {
            self.strike(
                player,
                &u.name,
                to,
                t,
                MELEE_DAMAGE.0,
                Some((unit, MELEE_DAMAGE.1)),
            );
            if let Some(i) = self.find_unit(player, unit) {
                set_moves(&mut self.units[i], 0);
            }
            return Ok(());
        }
        if node.flags.contains(ReachFlags::ATTACK) || self.foreign_at(player, to) {
            return refuse("the plot is occupied");
        }
        let in_city = self
            .cities
            .iter()
            .any(|c| c.owner == player && (c.x, c.y) == to);
        let u = &mut self.units[i];
        (u.x, u.y) = to;
        set_moves(u, node.moves_left);
        let mut flags =
            u.flags.0 & !(UnitFlags::FORTIFIED | UnitFlags::SLEEPING | UnitFlags::GARRISONED);
        if in_city && !u.flags.contains(UnitFlags::CIVILIAN) {
            flags |= UnitFlags::GARRISONED;
        }
        u.flags = UnitFlags(flags);
        self.pillage(player, to);
        self.refresh_fog();
        Ok(())
    }

    /// A foreign unit or city (at peace or at war) stands on the plot.
    fn foreign_at(&self, player: PlayerId, at: (i16, i16)) -> bool {
        self.units
            .iter()
            .any(|u| (u.x, u.y) == at && u.owner != player)
            || self
                .cities
                .iter()
                .any(|c| (c.x, c.y) == at && c.owner != player)
    }

    /// The enemy (at war) unit, else city, on the plot; with `visible` only on a plot the player sees.
    fn target_at(&self, player: PlayerId, at: (i16, i16), visible: bool) -> Option<Target> {
        let i = self.geo().index(at);
        if visible && !self.fog.is_visible(player, i) {
            return None;
        }
        if let Some(u) = self
            .units
            .iter()
            .find(|u| (u.x, u.y) == at && u.owner != player && self.dip.at_war(player, u.owner))
        {
            return Some(Target::Unit(u.owner, u.id));
        }
        self.cities
            .iter()
            .find(|c| (c.x, c.y) == at && c.owner != player && self.dip.at_war(player, c.owner))
            .map(|c| Target::City(c.owner, c.id))
    }

    /// The target of a shot at `at` from `from`: in range, in sight, an enemy (a city shooter takes units
    /// only).
    fn shot_target(
        &self,
        player: PlayerId,
        from: (i16, i16),
        range: i16,
        units_only: bool,
        at: (i16, i16),
    ) -> Result<Target, Refusal> {
        let listed = self
            .ranged_targets(player, from, range, units_only)
            .iter()
            .any(|t| (t.0, t.1) == at);
        match self.target_at(player, at, true) {
            Some(t) if listed => Ok(t),
            _ => refuse("nothing to shoot at"),
        }
    }

    /// `damage` off the target; `recoil` (attacker unit id, HP) off the attacker. Both HP are clamped to 1
    /// for cities and for the attacker. The journal lines go to both sides.
    fn strike(
        &mut self,
        by: PlayerId,
        by_name: &str,
        at: (i16, i16),
        target: Target,
        damage: i16,
        recoil: Option<(i32, i16)>,
    ) {
        match target {
            Target::Unit(owner, id) => {
                let Some(i) = self.find_unit(owner, id) else {
                    return;
                };
                self.units[i].hp -= damage;
                let (name, hp, max) = {
                    let u = &self.units[i];
                    (u.name.clone(), u.hp, u.max_hp)
                };
                let (by_civ, civ) = (self.civ_of(by), self.civ_of(owner));
                if hp <= 0 {
                    self.units.remove(i);
                    self.note_to(
                        by,
                        JournalKind::Combat,
                        format!("Your {by_name} destroyed {name} ({civ})"),
                        Some(at),
                    );
                    self.note_to(
                        owner,
                        JournalKind::Combat,
                        format!("Your {name} was destroyed by {by_name} ({by_civ})"),
                        Some(at),
                    );
                } else {
                    self.note_to(
                        by,
                        JournalKind::Combat,
                        format!("Your {by_name} hit {name} ({civ}): {hp}/{max} HP left"),
                        Some(at),
                    );
                    self.note_to(
                        owner,
                        JournalKind::Combat,
                        format!("Your {name} lost {damage} HP ({hp}/{max} left)"),
                        Some(at),
                    );
                }
            }
            Target::City(owner, id) => {
                let Some(c) = self
                    .cities
                    .iter_mut()
                    .find(|c| c.owner == owner && c.id == id)
                else {
                    return;
                };
                c.hp = (c.hp - damage).max(1);
                let (name, hp, max) = (c.name.clone(), c.hp, c.max_hp);
                let civ = self.civ_of(owner);
                self.note_to(
                    by,
                    JournalKind::Combat,
                    format!("Your {by_name} hit {name} ({civ}): {hp}/{max} HP left"),
                    Some(at),
                );
                self.note_to(
                    owner,
                    JournalKind::Combat,
                    format!("{name} lost {damage} HP ({hp}/{max} left)"),
                    Some(at),
                );
            }
        }
        if let Some((id, hurt)) = recoil
            && let Some(i) = self.find_unit(by, id)
        {
            let u = &mut self.units[i];
            u.hp = (u.hp - hurt).max(1);
            let line = format!(
                "Your {} lost {hurt} HP ({}/{} left)",
                u.name, u.hp, u.max_hp
            );
            let from = (u.x, u.y);
            self.note_to(by, JournalKind::Combat, line, Some(from));
        }
    }

    // ------------------------------------------------------------ contexts

    /// What the player may step onto from a neighbouring plot, as flags of the node: `None` blocked (not
    /// revealed, the wrong terrain, a foreign unit or city of someone at peace); `ATTACK` for a foe that is
    /// known (a unit in sight, a city on a revealed plot).
    fn step_flags(&self, player: PlayerId, domain: i8, at: (i16, i16)) -> Option<u16> {
        let i = self.geo().index(at);
        if !self.fog.is_revealed(player, i) {
            return None;
        }
        let p = self.map.plot(i)?;
        let ok = if domain == 1 {
            p.plot_type == PlotType::Water
        } else {
            matches!(p.plot_type, PlotType::Land | PlotType::Hill)
        };
        if !ok {
            return None;
        }
        let city = self.cities.iter().find(|c| (c.x, c.y) == at);
        let city_flag = if city.is_some() { ReachFlags::CITY } else { 0 };
        if self.fog.is_visible(player, i)
            && let Some(u) = self
                .units
                .iter()
                .find(|u| (u.x, u.y) == at && u.owner != player)
        {
            return self
                .dip
                .at_war(player, u.owner)
                .then_some(ReachFlags::ATTACK | city_flag);
        }
        match city {
            Some(c) if c.owner != player => self
                .dip
                .at_war(player, c.owner)
                .then_some(ReachFlags::ATTACK | ReachFlags::CITY),
            _ => Some(city_flag),
        }
    }

    /// Move tree of a unit: breadth first, two steps, node 0 its plot, `moves_left` less 60 per step.
    pub(super) fn move_tree(&self, player: PlayerId, u: &UnitView) -> Vec<ReachNode> {
        let geo = self.geo();
        let mut nodes = vec![ReachNode {
            x: u.x,
            y: u.y,
            parent: 0,
            turn: 0,
            moves_left: u.moves_left,
            flags: ReachFlags::default(),
        }];
        let mut depth = vec![0u8];
        let mut seen = BTreeSet::from([(u.x, u.y)]);
        let mut head = 0;
        while head < nodes.len() {
            let (n, d, parent) = (nodes[head], depth[head], head as u16);
            head += 1;
            if d >= TREE_DEPTH || n.moves_left <= 0 || n.flags.contains(ReachFlags::ATTACK) {
                continue;
            }
            for dir in 0..6 {
                let Some(at) = geo.neighbor((n.x, n.y), dir) else {
                    continue;
                };
                if !seen.insert(at) {
                    continue;
                }
                let Some(kind) = self.step_flags(player, u.domain, at) else {
                    continue;
                };
                let left = if kind & ReachFlags::ATTACK != 0 {
                    0
                } else {
                    (n.moves_left - MOVE_COST).max(0)
                };
                let stop = if left == 0 { ReachFlags::STOP } else { 0 };
                nodes.push(ReachNode {
                    x: at.0,
                    y: at.1,
                    parent,
                    turn: 0,
                    moves_left: left,
                    flags: ReachFlags(kind | stop),
                });
                depth.push(d + 1);
            }
        }
        nodes
    }

    fn query_unit(&self, player: PlayerId, id: i32) -> Result<&UnitView, CoreError> {
        match self.find_unit(player, id) {
            Some(i) => Ok(&self.units[i]),
            None => failed("no such unit"),
        }
    }

    pub(super) fn query_unit_actions(
        &self,
        player: PlayerId,
        unit: i32,
    ) -> Result<Vec<UnitActionView>, CoreError> {
        let u = self.query_unit(player, unit)?;
        let civilian = u.flags.contains(UnitFlags::CIVILIAN);
        let moves = u.moves_left > 0;
        let resting =
            u.flags.contains(UnitFlags::SLEEPING) || u.flags.contains(UnitFlags::FORTIFIED);
        let a = |action: i16, enabled: bool, reason: &str| UnitActionView {
            action,
            enabled,
            reason: if enabled {
                String::new()
            } else {
                reason.to_owned()
            },
        };
        let mut out = vec![a(1, moves, "fixture: no moves left")];
        if u.ranged > 0 {
            out.push(a(2, moves, "fixture: no moves left"));
        }
        if u.unit_type == U_SETTLER {
            out.push(a(3, false, "fixture: cities are not founded"));
        }
        out.push(a(
            4,
            !civilian && !u.flags.contains(UnitFlags::FORTIFIED),
            "fixture: cannot fortify",
        ));
        out.push(a(
            5,
            !u.flags.contains(UnitFlags::SLEEPING),
            "fixture: asleep already",
        ));
        out.push(a(6, true, ""));
        if resting {
            out.push(a(12, true, ""));
        }
        Ok(out)
    }

    pub(super) fn query_reach(
        &self,
        player: PlayerId,
        unit: i32,
        _horizon: u8,
    ) -> Result<Vec<ReachNode>, CoreError> {
        let u = self.query_unit(player, unit)?;
        Ok(self.move_tree(player, u))
    }

    /// `(x, y, flags)` of the plots a shooter can hit, ascending by plot index.
    fn ranged_targets(
        &self,
        player: PlayerId,
        from: (i16, i16),
        range: i16,
        units_only: bool,
    ) -> Vec<(i16, i16, u16)> {
        let geo = self.geo();
        let mut out: Vec<(u32, (i16, i16, u16))> = Vec::new();
        for at in geo.within(from, range.clamp(0, 255) as u8) {
            if at == from {
                continue;
            }
            match self.target_at(player, at, true) {
                Some(Target::Unit(..)) => out.push((geo.index(at), (at.0, at.1, 0))),
                Some(Target::City(..)) if !units_only => {
                    out.push((geo.index(at), (at.0, at.1, TARGET_CITY)));
                }
                _ => {}
            }
        }
        out.sort_unstable_by_key(|t| t.0);
        out.into_iter().map(|t| t.1).collect()
    }

    pub(super) fn query_targets(
        &self,
        player: PlayerId,
        shooter: Shooter,
    ) -> Result<Vec<(i16, i16, u16)>, CoreError> {
        match shooter {
            Shooter::Unit(id) => {
                let u = self.query_unit(player, id)?;
                if u.ranged <= 0 {
                    return Ok(Vec::new());
                }
                Ok(self.ranged_targets(player, (u.x, u.y), i16::from(u.range), false))
            }
            Shooter::City(id) => match self.cities.iter().find(|c| c.id == id && c.owner == player)
            {
                Some(c) => Ok(self.ranged_targets(player, (c.x, c.y), 2, true)),
                None => failed("no such city"),
            },
        }
    }

    pub(super) fn query_preview(
        &self,
        player: PlayerId,
        attacker: Shooter,
        at: (i16, i16),
    ) -> Result<Option<CombatPreview>, CoreError> {
        let geo = self.geo();
        if !geo.contains(at) {
            return failed("target outside the map");
        }
        // Strength x 100, the plot, ranged or melee, how far it reaches, whether a city shoots.
        let (strength, from, ranged, reach, city_shot) = match attacker {
            Shooter::Unit(id) => {
                let u = self.query_unit(player, id)?;
                if u.ranged > 0 {
                    (
                        i32::from(u.ranged) * 100,
                        (u.x, u.y),
                        true,
                        i16::from(u.range),
                        false,
                    )
                } else {
                    (i32::from(u.strength) * 100, (u.x, u.y), false, 1, false)
                }
            }
            Shooter::City(id) => match self.cities.iter().find(|c| c.id == id && c.owner == player)
            {
                Some(c) => (i32::from(c.defense), (c.x, c.y), true, 2, true),
                None => return failed("no such city"),
            },
        };
        let d = geo.distance(from, at);
        if d < 1 || d > reach {
            return Ok(None);
        }
        let Some(target) = self.target_at(player, at, true) else {
            return Ok(None);
        };
        let (defender_strength, hp, max_hp, is_city) = match target {
            Target::Unit(o, id) => {
                if city_shot && self.find_unit(o, id).is_none() {
                    return Ok(None);
                }
                let Some(u) = self.find_unit(o, id).map(|i| &self.units[i]) else {
                    return Ok(None);
                };
                let mut s = i32::from(u.strength) * 100;
                if u.flags.contains(UnitFlags::FORTIFIED) {
                    s = s * 125 / 100;
                }
                (s, u.hp, u.max_hp, false)
            }
            Target::City(o, id) => {
                if city_shot {
                    return Ok(None);
                }
                let Some(c) = self.cities.iter().find(|c| c.owner == o && c.id == id) else {
                    return Ok(None);
                };
                (i32::from(c.defense), c.hp, c.max_hp, true)
            }
        };
        let (dealt, taken) = if ranged {
            (SHOT_DAMAGE, 0)
        } else {
            MELEE_DAMAGE
        };
        let mut flags = 0;
        if !is_city && dealt >= hp {
            flags |= CombatPreview::KILLS;
        }
        if is_city {
            flags |= CombatPreview::CITY;
        }
        Ok(Some(CombatPreview {
            attacker_damage: taken,
            defender_damage: dealt,
            attacker_strength: strength,
            defender_strength,
            defender_hp: hp,
            defender_max_hp: max_hp,
            flags,
        }))
    }

    pub(super) fn query_city_builds(
        &self,
        player: PlayerId,
        city: i32,
    ) -> Result<Vec<CityBuildView>, CoreError> {
        let Some(c) = self
            .cities
            .iter()
            .find(|c| c.id == city && c.owner == player)
        else {
            return failed("no such city");
        };
        let prod = i32::from(c.yields[1]).max(1);
        let gold = self
            .major(player)
            .map_or(0, |m| seats::gold_at(self.turn, m));
        let entry = |order: i8, item: i16, enabled: bool| {
            let cost = production_cost(order, item);
            let current = c.production_order == order && c.production_item == item;
            let price = gold_cost(order, item);
            let mut flags = 0;
            if enabled {
                flags |= CityBuildFlags::ENABLED;
                if price.is_some_and(|p| p <= gold) {
                    flags |= CityBuildFlags::CAN_PURCHASE;
                }
            }
            if current {
                flags |= CityBuildFlags::CURRENT;
            }
            CityBuildView {
                order_type: order,
                item,
                turns: (((cost - c.production_stored).max(0) + prod - 1) / prod).max(1) as i16,
                production_cost: cost,
                gold_cost: price.unwrap_or(-1),
                flags: CityBuildFlags(flags),
                faith_cost: -1,
            }
        };
        let mut out: Vec<CityBuildView> = BUILD_UNITS
            .iter()
            .map(|t| entry(ORDER_UNIT, *t, true))
            .collect();
        // As the core asks it: a building behind the head of the queue cannot be started again.
        let queued = |b: i16| {
            self.queues
                .get(&(player, city))
                .is_some_and(|r| r.contains(&(ORDER_BUILDING, b)))
        };
        for b in 0..seats::BUILDINGS.len() as i16 {
            out.push(entry(
                ORDER_BUILDING,
                b,
                !c.buildings.contains(&b) && !queued(b),
            ));
        }
        Ok(out)
    }

    /// Path of a unit to a plot over revealed open land, as many turns as it takes. An unreachable target
    /// is one node with `UNREACHABLE`.
    pub(super) fn query_path(
        &self,
        player: PlayerId,
        unit: i32,
        target: (i16, i16),
    ) -> Result<Vec<PathNode>, CoreError> {
        let geo = self.geo();
        let u = self.query_unit(player, unit)?;
        if !geo.contains(target) {
            return failed("target outside the map");
        }
        let root = PathNode {
            x: u.x,
            y: u.y,
            turn: 0,
            flags: ReachFlags::default(),
        };
        if target == (u.x, u.y) {
            return Ok(vec![root]);
        }
        // (plot, parent index, flags)
        let mut nodes: Vec<((i16, i16), usize, u16)> = vec![((u.x, u.y), 0, 0)];
        let mut seen = BTreeSet::from([(u.x, u.y)]);
        let mut head = 0;
        let mut found = None;
        'search: while head < nodes.len() {
            let (at, _, kind) = nodes[head];
            head += 1;
            if kind & ReachFlags::ATTACK != 0 {
                continue;
            }
            for dir in 0..6 {
                let Some(nb) = geo.neighbor(at, dir) else {
                    continue;
                };
                if !seen.insert(nb) {
                    continue;
                }
                let Some(k) = self.step_flags(player, u.domain, nb) else {
                    continue;
                };
                if nb == target {
                    nodes.push((nb, head - 1, k));
                    found = Some(nodes.len() - 1);
                    break 'search;
                }
                if k & ReachFlags::ATTACK == 0 {
                    nodes.push((nb, head - 1, k));
                }
            }
        }
        let Some(mut at) = found else {
            return Ok(vec![PathNode {
                x: target.0,
                y: target.1,
                turn: 0,
                flags: ReachFlags(ReachFlags::UNREACHABLE),
            }]);
        };
        let mut chain = Vec::new();
        while at != 0 {
            chain.push(at);
            at = nodes[at].1;
        }
        chain.reverse();
        let mut path = vec![root];
        let (mut rem, mut turn) = (u.moves_left, 0i16);
        for i in chain {
            if rem <= 0 {
                turn += 1;
                rem = u.max_moves;
            }
            rem = (rem - MOVE_COST).max(0);
            let ((x, y), _, k) = nodes[i];
            path.push(PathNode {
                x,
                y,
                turn,
                flags: ReachFlags(k),
            });
        }
        Ok(path)
    }

    // ------------------------------------------------------------ save

    /// The whole state of a safe point as the bytes of the core part of a save: magic `FIXT`, version
    /// [`SAVE_VERSION`], postcard.
    pub(super) fn encode(&self) -> Result<Vec<u8>, CoreError> {
        if self.phase != GamePhase::Humans {
            return failed("the AI is moving: not a safe point");
        }
        let body = Postcard
            .encode(self)
            .map_err(|e| CoreError::failed(format!("fixture: save: {e}")))?;
        let mut out = Vec::with_capacity(body.len() + 6);
        out.extend_from_slice(SAVE_MAGIC);
        out.extend_from_slice(&SAVE_VERSION.to_le_bytes());
        out.extend_from_slice(&body);
        Ok(out)
    }

    pub(super) fn decode(bytes: &[u8]) -> Result<Game, CoreError> {
        let bad = |m: &str| CoreError::new(CoreErrorKind::BadSave, format!("fixture: {m}"));
        if bytes.len() < 6 || &bytes[..4] != SAVE_MAGIC {
            return Err(bad("not a FIXT save"));
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version != SAVE_VERSION {
            return Err(bad(&format!(
                "FIXT version {version}, {SAVE_VERSION} wanted"
            )));
        }
        let game: Game = Postcard
            .decode(&bytes[6..])
            .map_err(|e| bad(&format!("FIXT body: {e}")))?;
        if game.phase != GamePhase::Humans || game.majors.is_empty() {
            return Err(bad("FIXT state is inconsistent"));
        }
        Ok(game)
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use protocol::{
        Command, FixtureTest, JournalKind, PlayerFlags, PlotFlags, PlotView, ReachFlags, Shooter,
        TeamId, TurnMode, UnitFlags,
    };

    use super::super::seats::testing::setup;
    use super::super::{FixtureCore, map};
    use super::*;
    use crate::core::{Core, CoreStart};

    fn start(humans: usize, ais: usize, mode: TurnMode) -> FixtureCore {
        let mut s = setup(humans, ais);
        s.settings.turn_mode = mode;
        let mut core = FixtureCore::new(false, false);
        core.start(CoreStart::New {
            setup: &s,
            seed: 42,
        })
        .unwrap();
        core
    }

    fn g(core: &FixtureCore) -> &Game {
        core.game.as_ref().unwrap()
    }

    fn gm(core: &mut FixtureCore) -> &mut Game {
        core.game.as_mut().unwrap()
    }

    fn unit_of(core: &mut FixtureCore, p: PlayerId, ty: i16) -> UnitView {
        core.units(p)
            .unwrap()
            .into_iter()
            .find(|u| u.owner == p && u.unit_type == ty)
            .unwrap()
    }

    fn end(core: &mut FixtureCore, p: PlayerId) {
        core.command(p, &Command::EndTurn { force: true }).unwrap();
    }

    /// Everybody ends, the AI phase runs: the next turn begins.
    fn next_turn(core: &mut FixtureCore) {
        for p in 0..8 {
            if core.game().unwrap().is_human(p) {
                end(core, p);
            }
        }
        let a = core.pump(32).unwrap();
        assert!(matches!(a.phase, CorePhase::Ai { .. }));
        let b = core.pump(32).unwrap();
        assert_eq!(b.phase, CorePhase::Humans);
    }

    fn put(core: &mut FixtureCore, owner: PlayerId, id: i32, at: (i16, i16)) {
        let u = gm(core)
            .units
            .iter_mut()
            .find(|u| u.owner == owner && u.id == id)
            .unwrap();
        (u.x, u.y) = at;
        gm(core).refresh_fog();
    }

    /// A barbarian warrior (id 90) next to the unit.
    fn barbarian_next_to(core: &mut FixtureCore, u: &UnitView) -> (i16, i16) {
        let geo = g(core).geo();
        let at = (0..6)
            .filter_map(|d| geo.neighbor((u.x, u.y), d))
            .find(|a| {
                matches!(
                    g(core).map.plot(geo.index(*a)).unwrap().plot_type,
                    PlotType::Land | PlotType::Hill
                ) && !g(core).foreign_at(u.owner, *a)
            })
            .unwrap();
        let mut b = seats::spawn_unit(90, BARBARIAN, U_WARRIOR, at);
        b.name = "Barbarian Warrior".into();
        gm(core).units.push(b);
        gm(core).refresh_fog();
        at
    }

    #[test]
    fn move_follows_the_tree() {
        let mut core = start(2, 1, TurnMode::Simultaneous);
        let scout = unit_of(&mut core, 0, U_SCOUT);
        let tree = core.reach(0, scout.id, 1).unwrap();
        assert_eq!((tree[0].x, tree[0].y), (scout.x, scout.y));
        assert_eq!(tree[0].moves_left, scout.moves_left);
        // Parents come first, depth two, 60 less per step, all open land.
        for (i, n) in tree.iter().enumerate().skip(1) {
            assert!((n.parent as usize) < i);
            let p = &tree[n.parent as usize];
            assert_eq!(n.moves_left, (p.moves_left - MOVE_COST).max(0));
        }
        assert!(tree.len() > 6);
        let deep = tree.iter().find(|n| n.parent != 0).expect("a second step");
        let near = tree
            .iter()
            .find(|n| n.parent == 0 && n.flags.0 == 0)
            .unwrap();
        // A far plot is refused, a plot of the tree is taken.
        let far = (scout.x, (scout.y + 10).min(23));
        assert!(
            core.command(
                0,
                &Command::UnitMoveTo {
                    unit: scout.id,
                    x: far.0,
                    y: far.1
                }
            )
            .is_err()
        );
        core.command(
            0,
            &Command::UnitMoveTo {
                unit: scout.id,
                x: deep.x,
                y: deep.y,
            },
        )
        .unwrap();
        let after = unit_of(&mut core, 0, U_SCOUT);
        assert_eq!((after.x, after.y), (deep.x, deep.y));
        assert_eq!(after.moves_left, deep.moves_left);
        assert!(after.moves_left > 0 && after.flags.contains(UnitFlags::CAN_MOVE));
        // A plot that is not in the tree, a foreign unit's order, a missing unit.
        assert!(
            core.command(
                0,
                &Command::UnitMoveTo {
                    unit: scout.id,
                    x: near.x,
                    y: 99
                }
            )
            .is_err()
        );
        assert!(
            core.command(
                1,
                &Command::UnitMoveTo {
                    unit: 1,
                    x: 0,
                    y: 0
                }
            )
            .is_err()
        );
        let r = core
            .command(
                0,
                &Command::UnitMoveTo {
                    unit: 777,
                    x: 0,
                    y: 0,
                },
            )
            .unwrap_err();
        assert!(r.text.starts_with("fixture"));
    }

    #[test]
    fn fog_grows_when_a_unit_moves_and_dirty_plots_follow() {
        let mut core = start(2, 1, TurnMode::Simultaneous);
        assert_eq!(core.dirty_plots(), Some(vec![]));
        let scout = unit_of(&mut core, 0, U_SCOUT);
        let tree = core.reach(0, scout.id, 1).unwrap();
        let far = tree
            .iter()
            .max_by_key(|n| (n.parent != 0, n.x, n.y))
            .unwrap();
        core.command(
            0,
            &Command::UnitMoveTo {
                unit: scout.id,
                x: far.x,
                y: far.y,
            },
        )
        .unwrap();
        assert!(!core.dirty_plots().unwrap().is_empty());
    }

    #[test]
    fn fight_orders_and_previews() {
        let mut core = start(2, 1, TurnMode::Simultaneous);
        let archer = unit_of(&mut core, 0, U_ARCHER);
        let at = barbarian_next_to(&mut core, &archer);
        // The shot target list, the forecast, the fixed damage.
        let targets = core.targets(0, Shooter::Unit(archer.id)).unwrap();
        assert_eq!(targets, vec![(at.0, at.1, 0)]);
        let pre = core
            .preview(0, Shooter::Unit(archer.id), at.0, at.1)
            .unwrap()
            .unwrap();
        assert_eq!((pre.defender_damage, pre.attacker_damage), (SHOT_DAMAGE, 0));
        assert_eq!(pre.attacker_strength, 700);
        assert_eq!(pre.defender_hp, 10);
        assert!(
            core.preview(0, Shooter::Unit(archer.id), archer.x, archer.y)
                .unwrap()
                .is_none()
        );
        core.command(
            0,
            &Command::UnitRangeAttack {
                unit: archer.id,
                x: at.0,
                y: at.1,
            },
        )
        .unwrap();
        let hp = |core: &mut FixtureCore| {
            core.units(0)
                .unwrap()
                .iter()
                .find(|u| u.owner == BARBARIAN)
                .map(|u| u.hp)
        };
        assert_eq!(hp(&mut core), Some(10 - SHOT_DAMAGE));
        assert_eq!(unit_of(&mut core, 0, U_ARCHER).moves_left, 0);
        let events = core.events();
        assert!(events.iter().any(|e| e.entry.kind == JournalKind::Combat
            && e.to == EventTarget::Player(0)
            && e.entry.at == Some(at)));
        // No moves: no second shot.
        assert!(
            core.command(
                0,
                &Command::UnitRangeAttack {
                    unit: archer.id,
                    x: at.0,
                    y: at.1
                }
            )
            .is_err()
        );
        // The warrior of the city hits it in melee: both lose HP, the tree marks the plot.
        let warrior = unit_of(&mut core, 0, U_WARRIOR);
        let geo = g(&core).geo();
        let spot = (0..6)
            .filter_map(|d| geo.neighbor(at, d))
            .find(|a| {
                *a != (archer.x, archer.y)
                    && !g(&core).foreign_at(0, *a)
                    && matches!(
                        g(&core).map.plot(geo.index(*a)).unwrap().plot_type,
                        PlotType::Land | PlotType::Hill
                    )
            })
            .unwrap();
        put(&mut core, 0, warrior.id, spot);
        let warrior = unit_of(&mut core, 0, U_WARRIOR);
        let tree = core.reach(0, warrior.id, 1).unwrap();
        let node = tree.iter().find(|n| (n.x, n.y) == at).unwrap();
        assert!(node.flags.contains(ReachFlags::ATTACK) && node.moves_left == 0);
        core.command(
            0,
            &Command::UnitMoveTo {
                unit: warrior.id,
                x: at.0,
                y: at.1,
            },
        )
        .unwrap();
        assert_eq!(hp(&mut core), Some(10 - SHOT_DAMAGE - MELEE_DAMAGE.0));
        let w = unit_of(&mut core, 0, U_WARRIOR);
        assert_eq!(w.hp, 10 - MELEE_DAMAGE.1);
        assert!((w.x, w.y) != at);
        // A shot finishes the barbarian (3 + 4 + 3 = 10), it is gone.
        core.command(
            0,
            &Command::UnitRangeAttack {
                unit: archer.id,
                x: at.0,
                y: at.1,
            },
        )
        .unwrap_err();
        assert!(
            core.units(0)
                .unwrap()
                .iter()
                .all(|u| u.owner != BARBARIAN || u.hp > 0)
        );
    }

    #[test]
    fn city_shoots_units_in_range() {
        let mut core = start(2, 0, TurnMode::Simultaneous);
        let city = core.cities(0).unwrap()[0].clone();
        let geo = g(&core).geo();
        let two = geo
            .within((city.x, city.y), 2)
            .into_iter()
            .find(|a| geo.distance((city.x, city.y), *a) == 2)
            .unwrap();
        let mut b = seats::spawn_unit(91, BARBARIAN, U_WARRIOR, two);
        b.name = "Barbarian Warrior".into();
        gm(&mut core).units.push(b);
        gm(&mut core).refresh_fog();
        assert_eq!(
            core.targets(0, Shooter::City(city.id)).unwrap(),
            vec![(two.0, two.1, 0)]
        );
        core.command(
            0,
            &Command::CityRangeAttack {
                city: city.id,
                x: two.0,
                y: two.1,
            },
        )
        .unwrap();
        assert_eq!(
            g(&core).units.iter().find(|u| u.id == 91).unwrap().hp,
            10 - SHOT_DAMAGE
        );
        assert!(
            core.command(
                0,
                &Command::CityRangeAttack {
                    city: city.id,
                    x: city.x,
                    y: city.y
                }
            )
            .is_err()
        );
    }

    #[test]
    fn rest_orders_set_flags() {
        let mut core = start(2, 0, TurnMode::Simultaneous);
        let archer = unit_of(&mut core, 0, U_ARCHER);
        let settler = unit_of(&mut core, 0, U_SETTLER);
        let flags = |core: &mut FixtureCore, id: i32| {
            core.units(0)
                .unwrap()
                .into_iter()
                .find(|u| u.id == id && u.owner == 0)
                .unwrap()
                .flags
        };
        assert!(flags(&mut core, archer.id).contains(UnitFlags::NEEDS_ORDERS));
        core.command(0, &Command::UnitFortify { unit: archer.id })
            .unwrap();
        let f = flags(&mut core, archer.id);
        assert!(f.contains(UnitFlags::FORTIFIED) && !f.contains(UnitFlags::NEEDS_ORDERS));
        core.command(0, &Command::UnitSleep { unit: archer.id })
            .unwrap();
        let f = flags(&mut core, archer.id);
        assert!(f.contains(UnitFlags::SLEEPING) && !f.contains(UnitFlags::FORTIFIED));
        core.command(0, &Command::UnitWake { unit: archer.id })
            .unwrap();
        let f = flags(&mut core, archer.id);
        assert!(f.contains(UnitFlags::NEEDS_ORDERS) && !f.contains(UnitFlags::SLEEPING));
        core.command(0, &Command::UnitSkip { unit: archer.id })
            .unwrap();
        assert!(!flags(&mut core, archer.id).contains(UnitFlags::NEEDS_ORDERS));
        assert!(
            core.command(0, &Command::UnitFortify { unit: settler.id })
                .is_err()
        );
        let acts = core.unit_actions(0, archer.id).unwrap();
        assert!(acts.iter().any(|a| a.action == 2 && a.enabled));
        let acts = core.unit_actions(0, settler.id).unwrap();
        assert!(
            acts.iter()
                .any(|a| a.action == 3 && !a.enabled && !a.reason.is_empty())
        );
        assert!(core.unit_actions(0, 999).is_err());
        // The other commands are the fixture's refusal.
        let r = core
            .command(0, &Command::UnitDisband { unit: archer.id })
            .unwrap_err();
        assert_eq!(r.code, RefusalCode::Other);
        assert!(r.text.contains("fixture"));
    }

    #[test]
    fn production_builds_units_and_buildings() {
        let mut core = start(2, 0, TurnMode::Simultaneous);
        let city = core.cities(0).unwrap()[0].clone();
        assert_eq!(
            (
                city.production_item,
                city.production_needed,
                city.production_turns
            ),
            (U_WARRIOR, 20, 3)
        );
        let builds = core.city_builds(0, city.id).unwrap();
        assert!(
            builds
                .iter()
                .any(|b| b.order_type == 0 && b.flags.contains(CityBuildFlags::CURRENT))
        );
        assert!(builds.iter().filter(|b| b.order_type == 1).count() == 3);
        assert!(core.city_builds(0, 99).is_err());
        // A monument (40) at 3 per turn from 12 stored: 10 turns.
        core.command(
            0,
            &Command::CitySetProduction {
                city: city.id,
                order: ORDER_BUILDING,
                item: 0,
            },
        )
        .unwrap();
        let c = core.cities(0).unwrap()[0].clone();
        assert_eq!((c.production_needed, c.production_turns), (40, 10));
        assert!(
            core.command(
                0,
                &Command::CitySetProduction {
                    city: city.id,
                    order: 0,
                    item: 99
                }
            )
            .is_err()
        );
        assert!(
            core.command(
                0,
                &Command::CitySetProduction {
                    city: 99,
                    order: 0,
                    item: 2
                }
            )
            .is_err()
        );
        let units_before = core.units(0).unwrap().len();
        for _ in 0..10 {
            next_turn(&mut core);
        }
        let c = core.cities(0).unwrap()[0].clone();
        assert_eq!(c.buildings, vec![0]);
        assert_eq!(c.production_order, ORDER_UNIT);
        // The warriors came out of the loop after the monument.
        assert!(core.units(0).unwrap().len() >= units_before);
        // The finished monument is not on the list again.
        assert!(
            core.command(
                0,
                &Command::CitySetProduction {
                    city: c.id,
                    order: 1,
                    item: 0
                }
            )
            .is_err()
        );
        let builds = core.city_builds(0, c.id).unwrap();
        assert!(
            !builds
                .iter()
                .find(|b| b.order_type == 1 && b.item == 0)
                .unwrap()
                .flags
                .contains(CityBuildFlags::ENABLED)
        );
        // A unit comes out of the city and gets an id of its own.
        core.command(
            0,
            &Command::CitySetProduction {
                city: c.id,
                order: 0,
                item: U_SCOUT,
            },
        )
        .unwrap();
        for _ in 0..8 {
            next_turn(&mut core);
        }
        let ids: BTreeSet<i32> = g(&core)
            .units
            .iter()
            .filter(|u| u.owner == 0)
            .map(|u| u.id)
            .collect();
        assert_eq!(
            ids.len(),
            g(&core).units.iter().filter(|u| u.owner == 0).count()
        );
        assert!(ids.iter().any(|id| *id > 6));
    }

    #[test]
    fn a_building_is_bought_with_gold() {
        let mut core = start(2, 0, TurnMode::Simultaneous);
        let city = core.cities(0).unwrap()[0].clone();
        let gold = core.top_bar(0).unwrap().gold;
        let builds = core.city_builds(0, city.id).unwrap();
        let monument = builds
            .iter()
            .find(|b| b.order_type == ORDER_BUILDING && b.item == 0)
            .unwrap();
        assert_eq!(monument.gold_cost, 80);
        assert!(monument.flags.contains(CityBuildFlags::CAN_PURCHASE));
        assert!(
            builds
                .iter()
                .filter(|b| b.order_type == ORDER_UNIT)
                .all(|b| b.gold_cost == -1 && !b.flags.contains(CityBuildFlags::CAN_PURCHASE))
        );
        let buy = |item: i16, yield_: i8| Command::CityPurchase {
            city: city.id,
            order: ORDER_BUILDING,
            item,
            yield_,
        };
        // Not for faith, not a unit.
        assert!(core.command(0, &buy(0, 3)).is_err());
        assert!(
            core.command(
                0,
                &Command::CityPurchase {
                    city: city.id,
                    order: ORDER_UNIT,
                    item: U_WARRIOR,
                    yield_: 2,
                }
            )
            .is_err()
        );
        core.command(0, &buy(0, 2)).unwrap();
        assert_eq!(core.top_bar(0).unwrap().gold, gold - 80);
        assert_eq!(core.cities(0).unwrap()[0].buildings, vec![0]);
        // Bought: it cannot be bought or built again; the granary (120) is too dear now.
        assert!(core.command(0, &buy(0, 2)).is_err());
        assert!(core.command(0, &buy(1, 2)).is_err());
        let builds = core.city_builds(0, city.id).unwrap();
        let granary = builds
            .iter()
            .find(|b| b.order_type == ORDER_BUILDING && b.item == 1)
            .unwrap();
        assert!(!granary.flags.contains(CityBuildFlags::CAN_PURCHASE));
    }

    #[test]
    fn the_turn_passes_through_the_ai_phase_in_two_steps() {
        let mut core = start(2, 1, TurnMode::Simultaneous);
        assert_eq!(core.pump(32).unwrap().phase, CorePhase::Humans);
        end(&mut core, 0);
        assert_eq!(
            core.pump(32).unwrap().phase,
            CorePhase::Humans,
            "one human left"
        );
        assert!(core.safe_point().is_ok());
        let t = core.turn().unwrap();
        assert!(t.seats.iter().find(|s| s.player == 0).unwrap().finished);
        assert!(!t.seats.iter().find(|s| s.player == 1).unwrap().finished);
        // The first seat took its end back by an order.
        let u = unit_of(&mut core, 0, U_ARCHER);
        core.command(0, &Command::UnitSleep { unit: u.id }).unwrap();
        assert!(!core.turn().unwrap().seats[0].finished);
        end(&mut core, 0);
        assert!(core.command(0, &Command::EndTurn { force: false }).is_err());
        end(&mut core, 1);
        assert!(core.safe_point().is_err(), "the AI is moving");
        // Orders wait for the end of the AI phase.
        let r = core.command(1, &Command::UnitSkip { unit: 1 }).unwrap_err();
        assert_eq!(r.code, RefusalCode::AiRunning);
        let r = core.command(1, &Command::CancelEndTurn).unwrap_err();
        assert_eq!(r.code, RefusalCode::TurnEnded);
        let h = core.header(0).unwrap();
        assert!(matches!(h.status, protocol::TurnStatus::AiTurn { .. }));
        let a = core.pump(32).unwrap();
        assert!(matches!(a.phase, CorePhase::Ai { .. }));
        assert_eq!(a.turn, 1);
        let b = core.pump(32).unwrap();
        assert_eq!((b.phase, b.turn), (CorePhase::Humans, 2));
        assert_eq!(
            core.header(0).unwrap().status,
            protocol::TurnStatus::HumanTurn
        );
        assert!(core.turn().unwrap().seats.iter().all(|s| !s.finished));
        // The new turn: the log, orders wanted again, moves back.
        let ev = core.events();
        assert!(
            ev.iter()
                .any(|e| e.to == EventTarget::All && e.entry.text.contains("Turn 2"))
        );
        let scout = unit_of(&mut core, 0, U_SCOUT);
        assert!(scout.flags.contains(UnitFlags::NEEDS_ORDERS));
        assert_eq!(scout.moves_left, scout.max_moves);
        // The sleeper sleeps on.
        let archer = unit_of(&mut core, 0, U_ARCHER);
        assert!(
            archer.flags.contains(UnitFlags::SLEEPING)
                && !archer.flags.contains(UnitFlags::NEEDS_ORDERS)
        );
    }

    #[test]
    fn a_kicked_seat_does_not_hold_the_turn_and_a_claim_gives_it_back() {
        let mut core = start(2, 1, TurnMode::Simultaneous);
        end(&mut core, 0);
        core.set_control(1, &protocol::SlotControl::Ai).unwrap();
        let p = core.pump(32).unwrap();
        assert!(matches!(p.phase, CorePhase::Ai { .. }));
        core.pump(32).unwrap();
        assert_eq!(core.turn().unwrap().seats.len(), 1);
        core.set_control(
            1,
            &protocol::SlotControl::Human {
                nick: "back".into(),
            },
        )
        .unwrap();
        let t = core.turn().unwrap();
        assert_eq!(t.seats.len(), 2);
        assert!(t.seats.iter().all(|s| !s.finished));
        // A player that is not human any more cannot give orders.
        core.set_control(0, &protocol::SlotControl::Ai).unwrap();
        let r = core
            .command(0, &Command::EndTurn { force: true })
            .unwrap_err();
        assert_eq!(r.code, RefusalCode::NotHuman);
        let r = core
            .command(30, &Command::EndTurn { force: true })
            .unwrap_err();
        assert_eq!(r.code, RefusalCode::NotHuman);
    }

    #[test]
    fn timeout_ends_the_turn_without_checks() {
        let mut core = start(2, 0, TurnMode::Simultaneous);
        core.end_turn_timeout(0).unwrap();
        core.end_turn_timeout(1).unwrap();
        assert!(matches!(core.pump(32).unwrap().phase, CorePhase::Ai { .. }));
        assert_eq!(
            core.end_turn_timeout(0).unwrap_err().code,
            RefusalCode::TurnEnded
        );
        assert_eq!(
            core.end_turn_timeout(9).unwrap_err().code,
            RefusalCode::NotHuman
        );
    }

    #[test]
    fn hybrid_humans_at_war_take_turns() {
        let mut core = start(3, 0, TurnMode::Hybrid);
        assert!(
            core.turn()
                .unwrap()
                .seats
                .iter()
                .all(|s| s.simultaneous && s.may_act)
        );
        // 0 declares on 1: the war extends the timer (both moved at once before).
        core.command(0, &Command::DeclareWar { team: 1 }).unwrap();
        let t = core.turn().unwrap();
        assert_eq!(t.human_wars, vec![HumanWar { from: 0, to: 1 }]);
        assert!(core.turn().unwrap().human_wars.is_empty(), "drained");
        let seat = |t: &crate::core::CoreTurn, p: PlayerId| {
            *t.seats.iter().find(|s| s.player == p).unwrap()
        };
        let t = core.turn().unwrap();
        assert_eq!(
            (seat(&t, 0).simultaneous, seat(&t, 0).may_act),
            (false, true)
        );
        assert_eq!(
            (seat(&t, 1).simultaneous, seat(&t, 1).may_act),
            (false, false)
        );
        assert_eq!(
            (seat(&t, 2).simultaneous, seat(&t, 2).may_act),
            (true, true)
        );
        let r = core.command(1, &Command::UnitSkip { unit: 1 }).unwrap_err();
        assert_eq!(r.code, RefusalCode::NotYourTurn);
        let r = core
            .command(1, &Command::EndTurn { force: true })
            .unwrap_err();
        assert_eq!(r.code, RefusalCode::NotYourTurn);
        // 0 ends: 1 is next; 0 cannot take it back any more.
        end(&mut core, 0);
        let t = core.turn().unwrap();
        assert_eq!((seat(&t, 0).finished, seat(&t, 0).may_act), (true, false));
        assert!(seat(&t, 1).may_act);
        assert_eq!(
            core.command(0, &Command::CancelEndTurn).unwrap_err().code,
            RefusalCode::TurnEnded
        );
        let r = core.command(0, &Command::UnitSkip { unit: 1 }).unwrap_err();
        assert_eq!(r.code, RefusalCode::TurnEnded);
        end(&mut core, 1);
        end(&mut core, 2);
        assert!(matches!(core.pump(32).unwrap().phase, CorePhase::Ai { .. }));
        core.pump(32).unwrap();
        // Peace: both take part at once again.
        core.command(
            1,
            &Command::AnswerPeace {
                from: 0,
                accept: false,
            },
        )
        .unwrap();
        assert!(core.can_make_peace(0, 1).unwrap());
        core.command(
            1,
            &Command::AnswerPeace {
                from: 0,
                accept: true,
            },
        )
        .unwrap();
        assert!(!core.can_make_peace(0, 1).unwrap());
        assert!(core.turn().unwrap().seats.iter().all(|s| s.simultaneous));
        assert!(
            core.command(
                1,
                &Command::AnswerPeace {
                    from: 0,
                    accept: true
                }
            )
            .is_err()
        );
        // War again, then on the simultaneous mode a war on an AI is no human war.
        core.command(2, &Command::DeclareWar { team: 2 })
            .unwrap_err();
        core.command(2, &Command::DeclareWar { team: 0 }).unwrap();
        assert_eq!(core.turn().unwrap().human_wars.len(), 1);
        assert!(core.command(2, &Command::DeclareWar { team: 0 }).is_err());
        assert!(core.command(2, &Command::ProposePeace { to: 0 }).is_ok());
        assert!(core.command(2, &Command::ProposePeace { to: 1 }).is_err());
    }

    #[test]
    fn a_second_war_between_sequential_humans_does_not_extend() {
        let mut core = start(3, 0, TurnMode::Hybrid);
        core.command(0, &Command::DeclareWar { team: 1 }).unwrap();
        core.turn().unwrap();
        // 1 declares on 0 again? already at war; 0 and 1 are both sequential, a war on 2 starts with 2 at once.
        core.command(0, &Command::DeclareWar { team: 2 }).unwrap();
        assert_eq!(
            core.turn().unwrap().human_wars,
            vec![HumanWar { from: 0, to: 2 }]
        );
        let mut simultaneous = start(2, 0, TurnMode::Simultaneous);
        simultaneous
            .command(0, &Command::DeclareWar { team: 1 })
            .unwrap();
        assert_eq!(simultaneous.turn().unwrap().human_wars.len(), 1);
        let t = simultaneous.turn().unwrap();
        assert!(t.seats.iter().all(|s| s.simultaneous && s.may_act));
    }

    #[test]
    fn war_on_an_ai_is_not_a_human_war() {
        let mut core = start(2, 1, TurnMode::Hybrid);
        core.command(0, &Command::DeclareWar { team: 2 }).unwrap();
        assert!(core.turn().unwrap().human_wars.is_empty());
        let ps = core.players(0).unwrap();
        assert!(
            ps.iter()
                .find(|p| p.id == 2)
                .unwrap()
                .flags
                .contains(PlayerFlags::AT_WAR_WITH_VIEWER)
        );
        let ev = core.events();
        assert!(ev.iter().any(|e| e.entry.kind == JournalKind::War));
    }

    #[test]
    fn path_goes_over_open_land_and_counts_turns() {
        let mut core = start(2, 0, TurnMode::Simultaneous);
        let w = unit_of(&mut core, 0, U_WARRIOR);
        // Three steps away over known land: two on this turn, the third on the next.
        let tree = core.reach(0, w.id, 1).unwrap();
        let mid = tree.iter().find(|n| n.parent != 0).unwrap();
        let p = core.path(0, w.id, mid.x, mid.y).unwrap();
        assert_eq!(p.len(), 3);
        assert_eq!((p[0].x, p[0].y), (w.x, w.y));
        assert_eq!((p[2].x, p[2].y), (mid.x, mid.y));
        assert!(p.iter().all(|n| n.turn == 0));
        assert_eq!(core.path(0, w.id, w.x, w.y).unwrap().len(), 1);
        // A plot nobody has seen is unreachable; so is a plot outside.
        let far = core.path(0, w.id, 39, 23).unwrap();
        assert_eq!(far.len(), 1);
        assert!(far[0].flags.contains(ReachFlags::UNREACHABLE));
        assert!(core.path(0, w.id, 99, 99).is_err());
        assert!(core.path(0, 999, 1, 1).is_err());
    }

    #[test]
    fn barbarians_and_ai_move_and_pillage() {
        let mut core = start(2, 1, TurnMode::Simultaneous);
        let ai_before: Vec<(i16, i16)> = g(&core)
            .units
            .iter()
            .filter(|u| u.owner == 2)
            .map(|u| (u.x, u.y))
            .collect();
        let barb_before: Vec<(i16, i16)> = g(&core)
            .units
            .iter()
            .filter(|u| u.owner == BARBARIAN)
            .map(|u| (u.x, u.y))
            .collect();
        next_turn(&mut core);
        let ai_after: Vec<(i16, i16)> = g(&core)
            .units
            .iter()
            .filter(|u| u.owner == 2)
            .map(|u| (u.x, u.y))
            .collect();
        let barb_after: Vec<(i16, i16)> = g(&core)
            .units
            .iter()
            .filter(|u| u.owner == BARBARIAN)
            .map(|u| (u.x, u.y))
            .collect();
        assert_ne!(ai_before, ai_after);
        assert_ne!(barb_before, barb_after);
        // The garrison stays.
        assert!(
            g(&core)
                .units
                .iter()
                .any(|u| u.owner == 2 && u.flags.contains(UnitFlags::GARRISONED))
        );
        // A barbarian on a farm of a player pillages it; the farm is repaired when he is gone.
        let geo = g(&core).geo();
        let owned = (0..geo.plot_count() as u32)
            .find(|i| {
                let p = g(&core).map.plot(*i).unwrap();
                p.owner == 0
                    && p.plot_type == PlotType::Land
                    && p.improvement < 0
                    && p.route < 0
                    && !g(&core).foreign_at(0, geo.xy(*i))
            })
            .unwrap();
        gm(&mut core).map.plot_mut(owned).unwrap().improvement = 0;
        core.dirty_plots();
        let b = g(&core)
            .units
            .iter()
            .find(|u| u.owner == BARBARIAN)
            .unwrap()
            .id;
        put(&mut core, BARBARIAN, b, geo.xy(owned));
        gm(&mut core).pillage(BARBARIAN, geo.xy(owned));
        let dirty = core.dirty_plots().unwrap();
        assert!(dirty.contains(&owned));
        assert!(
            g(&core)
                .map
                .plot(owned)
                .unwrap()
                .flags
                .contains(PlotFlags::PILLAGED)
        );
        assert!(
            core.events()
                .iter()
                .any(|e| e.to == EventTarget::Player(0) && e.entry.text.contains("pillaged"))
        );
        put(&mut core, BARBARIAN, b, (1, 1));
        gm(&mut core).repair_plots();
        assert!(
            !g(&core)
                .map
                .plot(owned)
                .unwrap()
                .flags
                .contains(PlotFlags::PILLAGED)
        );
        assert!(core.dirty_plots().unwrap().contains(&owned));
    }

    #[test]
    fn the_basis_hook_changes_one_record_at_its_turn() {
        let mut core = FixtureCore::new(false, false);
        core.set_test_hooks(FixtureTest {
            change_basis_at_turn: Some(3),
            ..FixtureTest::default()
        });
        core.start(CoreStart::New {
            setup: &setup(2, 1),
            seed: 5,
        })
        .unwrap();
        let basis = |core: &mut FixtureCore| {
            let mut out = Vec::new();
            core.basis(None, &mut out).unwrap();
            out
        };
        let b1 = basis(&mut core);
        next_turn(&mut core);
        assert_eq!(b1, basis(&mut core), "not yet");
        core.dirty_plots();
        next_turn(&mut core);
        let b3 = basis(&mut core);
        let changed: Vec<u32> = b1
            .iter()
            .zip(&b3)
            .filter(|(a, b)| a != b)
            .map(|(a, _)| a.0)
            .collect();
        assert_eq!(changed.len(), 1);
        assert!(core.dirty_plots().unwrap().contains(&changed[0]));
        assert_eq!(b3[changed[0] as usize].1.feature, map::F_MARSH);
        next_turn(&mut core);
        assert_eq!(b3, basis(&mut core), "once");
        // Without the hook nothing changes.
        let mut plain = start(2, 1, TurnMode::Simultaneous);
        let p1 = basis(&mut plain);
        for _ in 0..4 {
            next_turn(&mut plain);
        }
        assert_eq!(p1, basis(&mut plain));
    }

    fn code_of(r: Result<(), Refusal>) -> Option<RefusalCode> {
        r.err().map(|e| e.code)
    }

    #[test]
    fn the_build_queue_takes_pushes_pops_moves_and_a_new_head() {
        let mut core = start(2, 0, TurnMode::Simultaneous);
        let id = core.cities(0).unwrap()[0].id;
        let q = |core: &FixtureCore| core.city_queue(0, id).unwrap();
        assert_eq!(q(&core), vec![(ORDER_UNIT, U_WARRIOR)]);
        let push = |order, item, front| Command::CityQueuePush {
            city: id,
            order,
            item,
            front,
        };
        core.command(0, &push(ORDER_BUILDING, 0, false)).unwrap();
        core.command(0, &push(ORDER_UNIT, U_SCOUT, true)).unwrap();
        assert_eq!(
            q(&core),
            vec![
                (ORDER_UNIT, U_SCOUT),
                (ORDER_UNIT, U_WARRIOR),
                (ORDER_BUILDING, 0)
            ]
        );
        // The head is the city's production.
        let c = core.cities(0).unwrap()[0].clone();
        assert_eq!(
            (c.production_order, c.production_item),
            (ORDER_UNIT, U_SCOUT)
        );
        // A queued building is no choice any more, and cannot be queued twice.
        let monument = core
            .city_builds(0, id)
            .unwrap()
            .into_iter()
            .find(|b| b.order_type == ORDER_BUILDING && b.item == 0)
            .unwrap();
        assert!(!monument.flags.contains(CityBuildFlags::ENABLED));
        assert_eq!(
            code_of(core.command(0, &push(ORDER_BUILDING, 0, false))),
            Some(RefusalCode::Invalid)
        );
        core.command(
            0,
            &Command::CityQueueMove {
                city: id,
                from: 2,
                to: 0,
            },
        )
        .unwrap();
        assert_eq!(
            q(&core),
            vec![
                (ORDER_BUILDING, 0),
                (ORDER_UNIT, U_SCOUT),
                (ORDER_UNIT, U_WARRIOR)
            ]
        );
        core.command(0, &Command::CityQueuePop { city: id, slot: 1 })
            .unwrap();
        core.command(
            0,
            &Command::CityReplaceHead {
                city: id,
                order: ORDER_UNIT,
                item: U_ARCHER,
            },
        )
        .unwrap();
        assert_eq!(
            q(&core),
            vec![(ORDER_UNIT, U_ARCHER), (ORDER_UNIT, U_WARRIOR)]
        );
        // Wrong slots, an unknown city, a move onto itself: `Invalid`, nothing changes.
        for bad in [
            Command::CityQueuePop { city: id, slot: 2 },
            Command::CityQueuePop { city: id, slot: -1 },
            Command::CityQueuePop { city: 99, slot: 0 },
            Command::CityQueueMove {
                city: id,
                from: 0,
                to: 5,
            },
            Command::CityQueueMove {
                city: id,
                from: 1,
                to: 1,
            },
            push(ORDER_UNIT, 99, false),
        ] {
            assert_eq!(
                code_of(core.command(0, &bad)),
                Some(RefusalCode::Invalid),
                "{bad:?}"
            );
        }
        assert_eq!(
            q(&core),
            vec![(ORDER_UNIT, U_ARCHER), (ORDER_UNIT, U_WARRIOR)]
        );
        // The archer (30) is done and the warrior follows; an empty queue builds nothing.
        for _ in 0..8 {
            next_turn(&mut core);
        }
        assert_eq!(q(&core).first(), Some(&(ORDER_UNIT, U_WARRIOR)));
        core.command(0, &Command::CityQueuePop { city: id, slot: 0 })
            .unwrap();
        assert!(q(&core).is_empty());
        let c = core.cities(0).unwrap()[0].clone();
        assert_eq!(c.production_order, -1);
        let units = core.units(0).unwrap().len();
        next_turn(&mut core);
        assert_eq!(core.units(0).unwrap().len(), units);
        assert_eq!(
            code_of(core.command(
                0,
                &Command::CityReplaceHead {
                    city: id,
                    order: ORDER_UNIT,
                    item: U_SCOUT
                }
            )),
            Some(RefusalCode::Invalid)
        );
        // Set production replaces the whole queue; the queue goes into the save.
        core.command(0, &push(ORDER_UNIT, U_SCOUT, false)).unwrap();
        core.command(0, &push(ORDER_UNIT, U_WORKER, false)).unwrap();
        let saved = core.save().unwrap();
        let mut again = FixtureCore::new(false, false);
        again.start(CoreStart::Load(&saved)).unwrap();
        assert_eq!(q(&again), q(&core));
        core.command(
            0,
            &Command::CitySetProduction {
                city: id,
                order: ORDER_UNIT,
                item: U_SETTLER,
            },
        )
        .unwrap();
        assert_eq!(q(&core), vec![(ORDER_UNIT, U_SETTLER)]);
    }

    #[test]
    fn a_research_choice_shows_in_the_top_bar_until_the_tech_is_known() {
        let mut core = start(2, 0, TurnMode::Simultaneous);
        let set = |tech, queue| Command::ResearchSet { tech, queue };
        for bad in [set(-1, false), set(1, false), set(64, true)] {
            assert_eq!(
                code_of(core.command(0, &bad)),
                Some(RefusalCode::Invalid),
                "{bad:?}"
            );
        }
        core.command(0, &set(0, false)).unwrap();
        core.command(0, &set(0, true)).unwrap();
        assert_eq!(g(&core).research.get(&0), Some(&vec![0]));
        assert_eq!(core.top_bar(0).unwrap().research_tech, 0);
        for _ in 0..8 {
            next_turn(&mut core);
        }
        assert_eq!(core.players(0).unwrap()[0].art_techs, 1);
        assert!(g(&core).research.is_empty());
        assert_eq!(core.top_bar(0).unwrap().research_tech, -1);
        assert_eq!(
            code_of(core.command(0, &set(0, false))),
            Some(RefusalCode::Invalid)
        );
    }

    #[test]
    fn research_changes_art_techs_and_the_top_bar() {
        let mut core = start(2, 0, TurnMode::Simultaneous);
        assert_eq!(core.players(0).unwrap()[0].art_techs, 0);
        assert_eq!(core.top_bar(0).unwrap().research_progress, 5);
        assert_eq!(core.top_bar(0).unwrap().research_turns, 8);
        for _ in 0..7 {
            next_turn(&mut core);
        }
        assert_eq!(core.players(0).unwrap()[0].art_techs, 0);
        next_turn(&mut core);
        assert_eq!(core.players(0).unwrap()[0].art_techs, 1);
        assert_eq!(core.top_bar(0).unwrap().research_tech, -1);
        // A player that has not been met shows no techs.
        assert_eq!(
            core.players(1)
                .unwrap()
                .iter()
                .find(|p| p.id == 0)
                .unwrap()
                .art_techs,
            0
        );
        gm(&mut core).dip.meet(1, 0);
        assert_eq!(
            core.players(1)
                .unwrap()
                .iter()
                .find(|p| p.id == 0)
                .unwrap()
                .art_techs,
            1
        );
        let learned = core
            .events()
            .into_iter()
            .filter(|e| e.entry.text.contains("researched"))
            .count();
        assert_eq!(learned, 2, "one line per player, once");
    }

    #[test]
    fn the_fog_leak_hook_shows_a_foreign_unit() {
        let mut core = FixtureCore::new(false, false);
        core.set_test_hooks(FixtureTest {
            leak_fog: true,
            ..FixtureTest::default()
        });
        core.start(CoreStart::New {
            setup: &setup(2, 0),
            seed: 5,
        })
        .unwrap();
        let units = core.units(0).unwrap();
        assert!(units.iter().any(|u| u.owner != 0));
        assert!(
            units
                .windows(2)
                .all(|w| (w[0].owner, w[0].id) < (w[1].owner, w[1].id))
        );
    }

    #[test]
    fn save_and_load_give_the_same_state() {
        for (huge, wrap) in [(false, false), (false, true), (true, false)] {
            let mut core = FixtureCore::new(huge, wrap);
            core.start(CoreStart::New {
                setup: &setup(3, 1),
                seed: 9,
            })
            .unwrap();
            // Some play: moves, a war, production, a finished turn and an ended turn.
            let scout = unit_of(&mut core, 0, U_SCOUT);
            let tree = core.reach(0, scout.id, 1).unwrap();
            let far = tree.last().unwrap();
            core.command(
                0,
                &Command::UnitMoveTo {
                    unit: scout.id,
                    x: far.x,
                    y: far.y,
                },
            )
            .unwrap();
            core.command(0, &Command::DeclareWar { team: 1 }).unwrap();
            let city = core.cities(2).unwrap()[0].id;
            core.command(
                2,
                &Command::CitySetProduction {
                    city,
                    order: 1,
                    item: 2,
                },
            )
            .unwrap();
            core.dirty_plots();
            next_turn(&mut core);
            next_turn(&mut core);
            end(&mut core, 2);
            let bytes = core.save().unwrap();
            assert!(bytes.starts_with(b"FIXT"));
            assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), SAVE_VERSION);

            let mut loaded = FixtureCore::new(!huge, !wrap);
            loaded.start(CoreStart::Load(&bytes)).unwrap();
            assert_eq!(loaded.map(), core.map());
            assert_eq!(loaded.save().unwrap(), bytes);
            assert_eq!(loaded.digest(-1).unwrap(), core.digest(-1).unwrap());
            for team in 0..4 {
                assert_eq!(loaded.digest(team).unwrap(), core.digest(team).unwrap());
            }
            assert_eq!(loaded.seats().unwrap(), core.seats().unwrap());
            let (a, b) = (loaded.turn().unwrap(), core.turn().unwrap());
            assert_eq!((a.seats, a.mode), (b.seats, b.mode));
            assert_eq!(loaded.dirty_plots(), Some(vec![]));
            // And it plays on the same way.
            for p in [&mut core, &mut loaded] {
                end(p, 0);
                end(p, 1);
                p.pump(32).unwrap();
                p.pump(32).unwrap();
            }
            assert_eq!(loaded.save().unwrap(), core.save().unwrap());
        }
    }

    #[test]
    fn bad_saves_are_refused() {
        let mut core = start(2, 0, TurnMode::Simultaneous);
        let bytes = core.save().unwrap();
        let load = |b: &[u8]| {
            FixtureCore::new(false, false)
                .start(CoreStart::Load(b))
                .unwrap_err()
        };
        assert_eq!(load(b"FIXT").kind, CoreErrorKind::BadSave);
        assert_eq!(load(b"nope-nope").kind, CoreErrorKind::BadSave);
        let mut v1 = bytes.clone();
        v1[4] = 1;
        assert!(load(&v1).message.contains("version"));
        assert_eq!(load(&bytes[..bytes.len() / 2]).kind, CoreErrorKind::BadSave);
        // The AI phase is not a safe point to save at.
        end(&mut core, 0);
        end(&mut core, 1);
        assert!(core.save().is_err());
    }

    #[test]
    fn many_random_orders_keep_the_world_consistent() {
        let mut core = start(4, 2, TurnMode::Hybrid);
        let mut x = 12345u32;
        let mut next = |m: u32| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (x >> 8) % m
        };
        for _ in 0..4000 {
            let p = next(4) as PlayerId;
            let units = core.units(p).unwrap();
            let own: Vec<&UnitView> = units.iter().filter(|u| u.owner == p).collect();
            let cmd = match next(9) {
                0..=2 => {
                    let u = own[next(own.len() as u32) as usize];
                    let tree = core.reach(p, u.id, 1).unwrap();
                    let n = tree[next(tree.len() as u32) as usize];
                    Command::UnitMoveTo {
                        unit: u.id,
                        x: n.x,
                        y: n.y,
                    }
                }
                3 => Command::UnitFortify {
                    unit: own[next(own.len() as u32) as usize].id,
                },
                4 => Command::UnitWake {
                    unit: own[next(own.len() as u32) as usize].id,
                },
                5 => Command::DeclareWar {
                    team: next(4) as i8,
                },
                6 => Command::EndTurn { force: true },
                7 => Command::CancelEndTurn,
                _ => Command::UnitRangeAttack {
                    unit: own[0].id,
                    x: 3,
                    y: 3,
                },
            };
            let _ = core.command(p, &cmd);
            if core.pump(32).unwrap().phase != CorePhase::Humans {
                core.pump(32).unwrap();
            }
            core.dirty_plots();
        }
        // Every unit sits on land or water of its domain; ids are unique per owner.
        let geo = g(&core).geo();
        let mut keys = BTreeSet::new();
        for u in &g(&core).units {
            assert!(keys.insert((u.owner, u.id)), "{u:?}");
            assert!(geo.contains((u.x, u.y)));
            assert!(u.hp > 0 && u.hp <= u.max_hp);
        }
        let t: TeamId = 0;
        let mut plots: Vec<(u32, PlotView)> = Vec::new();
        core.plots(t, None, &mut plots).unwrap();
        assert_eq!(plots.len(), geo.plot_count());
        // The state still saves and loads.
        while !matches!(core.safe_point(), Ok(())) {
            core.pump(32).unwrap();
        }
        let bytes = core.save().unwrap();
        let mut loaded = FixtureCore::new(false, false);
        loaded.start(CoreStart::Load(&bytes)).unwrap();
        assert_eq!(loaded.digest(-1).unwrap(), core.digest(-1).unwrap());
    }
}
