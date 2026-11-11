//! `RealCore`: the multiplayer [`Core`] on gamecore v2 (game-host-mp 4.15, GH-D1). The game database and
//! Lua come from the game files through a [`FileReader`]; the `Worker` asks the core for views and gives
//! it the orders. The core is a process singleton: one game per process.

use std::sync::{Arc, Mutex};

use gamecore::{GameError, GameEvent, HostServices};
use gamecore_sys::GcError;

use crate::files::GameFiles;
use crate::journal;
use crate::lobby_options;
use crate::replica::ReplicaOps;
use crate::world_build as wb;

/// The file under the key `path`: exact, then with `/` separators and lower case (the keys of the
/// game data index are lower case, the core asks for the paths as Civ5 spells them).
fn find_file<'a>(files: &'a GameFiles, path: &str) -> Option<&'a Arc<[u8]>> {
    if let Some(f) = files.get(path) {
        return Some(f);
    }
    let norm = path.replace('\\', "/").to_lowercase();
    files.get(norm.trim_start_matches("./"))
}
use std::collections::{BTreeMap, HashMap};

use gamecore as gc;
use protocol::{
    AiTurnEvent, CivChoice, CivPick, Codec, CombatPreview, GameSetup, HumanWar, LobbyChoices,
    NotificationView, PathNode, PlayerId, PlayerView, Postcard, ReachNode, SeatTurnInfo, Shooter,
    SlotControl, SlotSetup, TeamId, TimerMode, TopBar, TurnMode, TurnStatus, UnitActionView,
    ViewHeader,
};

use crate::core::{
    Core, CoreError, CoreErrorKind, CoreEvent, CorePhase, CorePump, CoreStart, CoreTurn,
    EventTarget, MapInfo, Refusal, RefusalCode,
};

/// Reads a file of the game data by its Civ5 VFS path (`db/gameplay.db`, `Assets/...`); `None`: no such
/// file. The shell supplies it (disk), the host itself does no I/O (R4).
pub type FileReader = Box<dyn Fn(&str) -> Option<Vec<u8>> + Send>;

/// `read_file` of the core through a [`FileReader`]; collects the core's events.
struct ReaderHost {
    read: FileReader,
    events: Arc<Mutex<Vec<GameEvent>>>,
}

impl HostServices for ReaderHost {
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        (self.read)(path)
    }

    fn game_event(&self, ev: GameEvent) {
        if let Ok(mut q) = self.events.lock() {
            q.push(ev);
        }
    }
}

/// The error of a core call as the worker sees it: `Stuck` for a core that cannot go on, else `Failed`.
fn core_err(what: &str, e: GameError) -> CoreError {
    let kind = match e {
        GameError::Core(GcError::Stuck(_)) => CoreErrorKind::Stuck,
        _ => CoreErrorKind::Failed,
    };
    CoreError::new(kind, format!("{what}: {e}"))
}

fn failed(what: &str) -> impl Fn(GameError) -> CoreError + '_ {
    move |e| core_err(what, e)
}

fn player_u8(p: PlayerId) -> Result<u8, CoreError> {
    u8::try_from(p).map_err(|_| CoreError::failed(format!("no player {p}")))
}

fn turn_mode_of(m: gc::TurnMode) -> TurnMode {
    match m {
        gc::TurnMode::Simultaneous => TurnMode::Simultaneous,
        gc::TurnMode::Hybrid => TurnMode::Hybrid,
    }
}

fn core_turn_mode(m: TurnMode) -> gc::TurnMode {
    match m {
        TurnMode::Simultaneous => gc::TurnMode::Simultaneous,
        TurnMode::Hybrid => gc::TurnMode::Hybrid,
    }
}

fn refusal_of(reason: gc::RejectReason, text: String) -> Refusal {
    let code = match reason {
        gc::RejectReason::NotHuman => RefusalCode::NotHuman,
        gc::RejectReason::NotYourTurn => RefusalCode::NotYourTurn,
        gc::RejectReason::TurnEnded => RefusalCode::TurnEnded,
        gc::RejectReason::AiRunning => RefusalCode::AiRunning,
        gc::RejectReason::PumpFirst => RefusalCode::PumpFirst,
        _ => RefusalCode::Other,
    };
    Refusal::new(code, text)
}

// ---------------------------------------------------------------- v15 orders as net messages
//
// The build queue and research orders of protocol v15 (tags 20-24) are not core commands: the host checks
// them against the core's own answers and gives them to the core as the net messages the original UI sends
// for the same click, through `gc_net_submit` (which checks the seat again and stamps the sender). The
// messages are delivered by the next pump, which the worker runs after every order it answers `Applied`.
//
// What the core does with them (Lekmod sources):
// * `sendPushOrder(city, order, item, alt, shift, ctrl)` -> `CvDllNetMessageHandler::ResponseCityPushOrder`
//   -> `CvCity::pushOrder(order, item, -1, bSave = alt, bPop = shift, bAppend = ctrl)`. `bPop` clears the
//   whole queue first; `bAppend` adds at the end, otherwise the item goes in front of the head
//   (`stopHeadOrder`, insert at the beginning, `startHeadOrder`). An item `canTrain` / `canConstruct` /
//   `canCreate` / `canMaintain` refuses is dropped without a word.
// * `sendPopOrder(city, n)` -> `ResponseCityPopOrder` -> `CvCity::popOrder(n)`: removes entry `n` (0 = the
//   head); an index past the end is ignored.
// * `sendSwapOrder(city, n)` -> `ResponseCitySwapOrder` -> `CvCity::swapOrder(n)`: exchanges entries `n` and
//   `n + 1` (the queue's `swapUp`); an index without a successor is ignored.
// * `sendResearch(tech, discover, steal_from, shift)` -> `ResponseResearch`: with `discover = 0` and no
//   victim it is the normal choice, `CvPlayer::pushResearch(tech, bClear = !shift)` (prerequisites are
//   queued too); without `shift` a tech already known or being researched clears the research queue.
// * The core has no limit of the queue length (the order list grows past its inline storage).

/// The seat's city as the checks of a v15 queue order see it: the core's own answers (`Q_CITIES`,
/// `Q_CITY_BUILDS`, `Q_CITY_QUEUE`).
#[derive(Debug, Clone)]
pub(crate) struct QueueCity {
    pub id: i32,
    pub puppet: bool,
    /// The queue has a head (the production order of `Q_CITIES`).
    pub has_head: bool,
    /// The length of the queue (`Q_CITY_QUEUE`); `None` for a core that does not answer that query, whose
    /// slots past the end are then left to the core (it ignores them).
    pub queue_len: Option<usize>,
    pub builds: Vec<gc::CityBuild>,
}

/// What the check of a `ResearchSet` knows: the rows of the technology table and the technologies the
/// seat's team has (`Q_TECHS`; empty for a core that does not answer it).
#[derive(Debug, Clone, Default)]
pub(crate) struct ResearchCheck {
    pub techs: usize,
    pub known: Vec<i16>,
}

/// A v15 order of the build queue or the research (protocol tags 20-24).
pub(crate) fn is_v15_order(cmd: &protocol::Command) -> bool {
    use protocol::Command as P;
    matches!(
        cmd,
        P::CityQueuePush { .. }
            | P::CityQueuePop { .. }
            | P::CityQueueMove { .. }
            | P::CityReplaceHead { .. }
            | P::ResearchSet { .. }
    )
}

/// One net message by its `gDLL->send*` name.
fn net(name: &str, args: &[i32]) -> Result<gc::replica::NetMessage, String> {
    gc::replica::NetMessage::new(name, args)
        .ok_or_else(|| format!("the core has no net message {name}"))
}

/// `sendPushOrder` of `item`: appended (`ctrl`) or in front of the head; never `shift` (that would clear the
/// queue) nor `alt` (the repeat flag of the original UI).
fn push_order(
    city: i32,
    order: i8,
    item: i16,
    append: bool,
) -> Result<gc::replica::NetMessage, String> {
    net(
        "sendPushOrder",
        &[
            city,
            i32::from(order),
            i32::from(item),
            0,
            0,
            i32::from(append),
        ],
    )
}

/// The build `order` / `item` of the city may go into its queue now. `CvCity::pushOrder` drops what the
/// city cannot start without a word, so the host refuses it first.
fn check_build(c: &QueueCity, order: i8, item: i16, adding: bool) -> Result<(), String> {
    let o = wb::order_of(order)?;
    let b = c
        .builds
        .iter()
        .find(|b| b.order == o && b.item == item)
        .ok_or_else(|| format!("city {} cannot build order {order} item {item}", c.id))?;
    if !b.flags.enabled() {
        return Err(format!(
            "city {}: {}",
            c.id,
            b.reason.as_deref().unwrap_or("cannot be started now")
        ));
    }
    // `Q_CITY_BUILDS` asks about the head item as one that goes on (`bContinue`), `pushOrder` asks without it: a
    // building or a project that is being built cannot go into the queue a second time.
    if adding && b.flags.current() && matches!(o, gc::OrderType::Construct | gc::OrderType::Create)
    {
        return Err(format!(
            "city {}: item {item} is in the queue already",
            c.id
        ));
    }
    Ok(())
}

/// The net messages of a v15 order, or why the host refuses it (answered `Invalid`). `city`: the seat's
/// city the order names (`None`: the seat has no such city); `research`: the technology table and what the
/// team has.
///
/// A slot past the end of the queue is refused when the core told the queue's length (`Q_CITY_QUEUE`). A
/// technology the team has is refused; one whose prerequisites are missing is not: `pushResearch` queues
/// the prerequisites first, as the original tech tree does. A technology the civilization can never
/// research is dropped by the core without a word (`CanEverResearch`), so such an order changes nothing.
pub(crate) fn v15_net(
    cmd: &protocol::Command,
    city: Option<&QueueCity>,
    research: &ResearchCheck,
) -> Result<Vec<gc::replica::NetMessage>, String> {
    use protocol::Command as P;
    if let P::ResearchSet { tech, queue } = *cmd {
        if tech < 0 || usize::try_from(tech).is_ok_and(|t| t >= research.techs) {
            return Err(format!("there is no technology {tech}"));
        }
        if research.known.contains(&tech) {
            return Err(format!("technology {tech} is known already"));
        }
        // discover 0, no victim: the normal research choice; shift = add to the research queue.
        return Ok(vec![net(
            "sendResearch",
            &[i32::from(tech), 0, -1, i32::from(queue)],
        )?]);
    }
    let id = match *cmd {
        P::CityQueuePush { city, .. }
        | P::CityQueuePop { city, .. }
        | P::CityQueueMove { city, .. }
        | P::CityReplaceHead { city, .. } => city,
        _ => return Err("not a v15 queue or research order".into()),
    };
    let c = city
        .filter(|c| c.id == id)
        .ok_or_else(|| format!("the seat has no city {id}"))?;
    if c.puppet {
        return Err(format!(
            "city {id}: a puppet city chooses its own production"
        ));
    }
    let empty = || format!("city {id}: the build queue is empty");
    // A slot the queue does not have (when the core told its length).
    let past_end = |slot: i8| {
        c.queue_len
            .filter(|&n| usize::try_from(slot).is_ok_and(|s| s >= n))
            .map(|n| format!("city {id}: no slot {slot} in a queue of {n}"))
    };
    match *cmd {
        P::CityQueuePush {
            order, item, front, ..
        } => {
            check_build(c, order, item, true)?;
            Ok(vec![push_order(id, order, item, !front)?])
        }
        P::CityQueuePop { slot, .. } => {
            if slot < 0 {
                return Err(format!("city {id}: no slot {slot}"));
            }
            if !c.has_head {
                return Err(empty());
            }
            if let Some(e) = past_end(slot) {
                return Err(e);
            }
            Ok(vec![net("sendPopOrder", &[id, i32::from(slot)])?])
        }
        P::CityQueueMove { from, to, .. } => {
            if from < 0 || to < 0 {
                return Err(format!("city {id}: no slot {}", from.min(to)));
            }
            if from == to {
                return Err(format!("city {id}: slot {from} is moved onto itself"));
            }
            if !c.has_head {
                return Err(empty());
            }
            if let Some(e) = past_end(from).or_else(|| past_end(to)) {
                return Err(e);
            }
            // `swapOrder(n)` exchanges n and n + 1: walk the entry step by step, in one submit.
            let (from, to) = (i32::from(from), i32::from(to));
            let steps: Vec<i32> = if from < to {
                (from..to).collect()
            } else {
                (to..from).rev().collect()
            };
            steps
                .into_iter()
                .map(|n| net("sendSwapOrder", &[id, n]))
                .collect()
        }
        P::CityReplaceHead { order, item, .. } => {
            if !c.has_head {
                return Err(empty());
            }
            check_build(c, order, item, false)?;
            // The head goes, the new item goes in front of what is left: the rest of the queue stays.
            Ok(vec![
                net("sendPopOrder", &[id, 0])?,
                push_order(id, order, item, false)?,
            ])
        }
        _ => Err("not a v15 queue or research order".into()),
    }
}

/// A `map_script` with this prefix names a `.Civ5Map` of the game's VFS to build the world from (and its
/// scenario data), not a script. Only the golden tool sends it; the lobby validates map scripts against its list.
pub const WORLD_BUILDER_MAP_PREFIX: &str = "world_builder_map:";

/// The game of `GameSetup` as the core's network setup. `fallback_seed` is the game seed of `WorkerInit`
/// (the settings' own seed wins).
///
/// # Errors
/// The slots are not numbered 0.. without gaps.
pub fn network_setup(setup: &GameSetup, fallback_seed: u64) -> Result<gc::Setup, String> {
    let s = &setup.settings;
    let mut slots: Vec<&SlotSetup> = setup.slots.iter().collect();
    slots.sort_by_key(|x| x.player);
    let mut players = Vec::with_capacity(slots.len());
    for (i, slot) in slots.iter().enumerate() {
        if usize::try_from(slot.player).ok() != Some(i) {
            return Err(format!(
                "the slots are not numbered 0.. without gaps (slot {i} is player {})",
                slot.player
            ));
        }
        let civ = match &slot.civ {
            CivPick::Random => "random".to_owned(),
            CivPick::Civ(t) => t.clone(),
        };
        players.push(match &slot.control {
            SlotControl::Human { nick } => gc::SlotSetup::human(civ, nick.clone()),
            SlotControl::Ai => gc::SlotSetup::ai(civ),
        });
    }
    let seed = s
        .seed
        .unwrap_or((fallback_seed ^ (fallback_seed >> 32)) as u32);
    let a = &s.auto_choices;
    let flag = |b: bool| i32::from(b);
    let mut core = gc::Setup::network(seed, players);
    core.map_seed = s.map_seed;
    core.turn_mode = core_turn_mode(s.turn_mode);
    core.turn_timer = match &s.timer {
        TimerMode::Original { row } => Some(row.clone()),
        // A fixed timer is the gateway's; the core has no row for it.
        TimerMode::Off | TimerMode::Fixed { .. } => None,
    };
    core.world_size = s.world_size.clone();
    core.map_script = s.map_script.clone();
    // The settings have no field for a world builder map (a wire change): the golden tool sends it as a
    // map script `world_builder_map:<VFS path>` and the map decides the size (gc-abi-v2 4.15).
    if let Some(map) = s.map_script.strip_prefix(WORLD_BUILDER_MAP_PREFIX) {
        core.world_builder_map = Some(map.to_owned());
        core.world_builder_scenario = true;
    }
    core.map_options = s.map_options.clone();
    core.handicap = s.handicap.clone();
    core.game_speed = s.speed.clone();
    core.start_era = s.start_era.clone();
    core.minor_civs = gc::MinorCivs::Count(u32::from(s.minor_civs));
    // The lobby carries the victories and the turn limit in `game_options`; the core has setup keys for them.
    let split = crate::lobby_options::split_game_options(&s.game_options);
    core.game_options = split.core;
    core.victories = split.victories;
    core.max_turns = split.max_turns;
    core.auto_choices = gc::AutoChoices {
        research: flag(a.research),
        policy: flag(a.policy),
        ideology: flag(a.ideology),
        religion: flag(a.religion),
        promotion: flag(a.promotion),
        great_person: flag(a.great_person),
        captured_city: flag(a.captured_city),
        league: flag(a.league),
        archaeology: flag(a.archaeology),
        goody_hut: flag(a.goody_hut),
        steal_tech: flag(a.steal_tech),
    };
    core.diplo_auto_decline = flag(s.diplo_auto_decline);
    Ok(core)
}

/// The multiplayer core on gamecore (one per process). Fog is applied by the core: every call that takes a
/// team or a player gets the viewer's data (game-host-mp 4.15).
pub struct RealCore {
    game: gc::Game,
    events: Arc<Mutex<Vec<GameEvent>>>,
    /// Events taken from the core's queue and not yet turned into journal entries.
    backlog: Vec<GameEvent>,
    /// Wars between humans that extend the timer, not yet reported by `turn`.
    pending_wars: Vec<HumanWar>,
    map: MapInfo,
    /// The game turn of the last pump, dates the journal lines.
    turn: i32,
    /// `Civilizations` catalog: id -> `CIVILIZATION_*` type.
    civs: HashMap<i16, String>,
    /// The build id of the core (`gc_build_id`), read once at boot.
    build_id: String,
    /// Map scripts of the lobby (the core has no list of them): set by [`RealCore::with_map_scripts`].
    map_scripts: Vec<protocol::LobbyChoice>,
    /// The core is the server of a replica (`Role::Server`): its inputs go to the journal.
    replica_on: bool,
    /// Slices the last pump ran (`gc_pump_result.slices_used`).
    last_slices: u32,
    /// The `setTurnActive` of the non-human civilizations since the last `turn_status` (SIMP-exe-820).
    turn_status: Vec<AiTurnEvent>,
}

impl RealCore {
    /// Creates the core; its game database and Lua come from `read`.
    ///
    /// # Errors
    /// `BadStart`: the core did not boot (missing or broken game data, a second instance in the process).
    pub fn boot(read: FileReader) -> Result<Self, CoreError> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut game = gc::Game::new(ReaderHost {
            read,
            events: Arc::clone(&events),
        })
        .map_err(|e| {
            CoreError::new(
                CoreErrorKind::BadStart,
                format!("the game core did not start: {e}"),
            )
        })?;
        let build_id = game.build_id().map_err(|e| {
            CoreError::new(
                CoreErrorKind::BadStart,
                format!("the game core has no build id: {e}"),
            )
        })?;
        Ok(RealCore {
            game,
            build_id,
            events,
            backlog: Vec::new(),
            pending_wars: Vec::new(),
            map: MapInfo::default(),
            turn: 0,
            civs: HashMap::new(),
            map_scripts: Vec::new(),
            replica_on: false,
            last_slices: 0,
            turn_status: Vec::new(),
        })
    }

    /// The map scripts `Describe` offers (see [`crate::map_scripts`]).
    #[must_use]
    pub fn with_map_scripts(mut self, scripts: Vec<protocol::LobbyChoice>) -> Self {
        self.map_scripts = scripts;
        self
    }

    /// [`RealCore::boot`] on game files held in memory.
    ///
    /// # Errors
    /// As [`RealCore::boot`].
    pub fn boot_files(files: GameFiles) -> Result<Self, CoreError> {
        Self::boot(Box::new(move |p| find_file(&files, p).map(|b| b.to_vec())))
    }

    /// Moves the core's queued events into the backlog and notes the wars that extend the timer.
    fn pull(&mut self) {
        let fresh = match self.events.lock() {
            Ok(mut q) => std::mem::take(&mut *q),
            Err(_) => return,
        };
        if fresh.is_empty() {
            return;
        }
        // The turn status of the AI civilizations goes to the AI turn queue, not into the journal; the humans' own
        // activation reaches the clients through the replica.
        let (status, fresh): (Vec<GameEvent>, Vec<GameEvent>) = fresh
            .into_iter()
            .partition(|e| matches!(e, GameEvent::PlayerTurnStatus { .. }));
        for e in status {
            if let GameEvent::PlayerTurnStatus {
                started,
                player,
                human: false,
            } = e
                && let Ok(player) = PlayerId::try_from(player)
            {
                self.turn_status.push(AiTurnEvent {
                    player,
                    ended: !started,
                });
            }
        }
        if fresh.is_empty() {
            return;
        }
        let wars: Vec<(i32, i32)> = fresh
            .iter()
            .filter_map(|e| match e {
                GameEvent::HumanWarDeclared {
                    from_team,
                    to_team,
                    extends_timer: true,
                } => Some((*from_team, *to_team)),
                _ => None,
            })
            .collect();
        self.backlog.extend(fresh);
        if wars.is_empty() {
            return;
        }
        let Ok(slots) = self.game.turn_state() else {
            return;
        };
        let human_of = |team: i32| {
            slots
                .iter()
                .find(|s| s.flags.human() && i32::from(s.team) == team)
                .and_then(|s| PlayerId::try_from(s.player).ok())
        };
        for (from, to) in wars {
            if let (Some(from), Some(to)) = (human_of(from), human_of(to)) {
                self.pending_wars.push(HumanWar { from, to });
            }
        }
    }

    fn slots(&mut self) -> Result<Vec<gc::HumanSlot>, CoreError> {
        self.game.turn_state().map_err(failed("Q_TURN"))
    }

    /// A human who lost everything: the core answers no per-player query for him (Q_GAME, Q_TOP_BAR,
    /// Q_NOTIFICATIONS name a living major civilization).
    fn is_dead(&mut self, player: u8) -> Result<bool, CoreError> {
        Ok(self
            .slots()?
            .iter()
            .any(|s| s.player == player && s.flags.human() && !s.flags.alive()))
    }

    /// `(player, alive, connected)` of every human slot, as `Q_TURN` has it (tests: a load leaves the humans
    /// off line until their login, and a human who lost everything is dead).
    pub fn human_states(&mut self) -> Result<Vec<(PlayerId, bool, bool)>, CoreError> {
        Ok(self
            .slots()?
            .iter()
            .filter(|s| s.flags.human())
            .filter_map(|s| {
                Some((
                    PlayerId::try_from(s.player).ok()?,
                    s.flags.alive(),
                    s.flags.connected(),
                ))
            })
            .collect())
    }

    /// The team of a human player.
    fn team_of(&mut self, player: PlayerId) -> Result<TeamId, CoreError> {
        let p = player_u8(player)?;
        self.slots()?
            .into_iter()
            .find(|s| s.player == p)
            .map(|s| s.team)
            .ok_or_else(|| CoreError::failed(format!("player {player} is not a human of the game")))
    }

    fn refresh_map(&mut self) -> Result<(), CoreError> {
        let info = self.game.game_info().map_err(failed("Q_GAME"))?;
        self.map = MapInfo {
            width: u16::try_from(info.map_width).unwrap_or(0),
            height: u16::try_from(info.map_height).unwrap_or(0),
            wrap_x: info.wrap_x,
            wrap_y: info.wrap_y,
            terrain_seed: info.map_seed,
        };
        self.turn = info.turn;
        Ok(())
    }

    fn civ_types(&mut self) -> Result<&HashMap<i16, String>, CoreError> {
        if self.civs.is_empty() {
            let list = self
                .game
                .catalog(gc::CatalogTable::Civilizations)
                .map_err(failed("Q_CATALOG civilizations"))?;
            self.civs = list.into_iter().map(|e| (e.id, e.type_name)).collect();
        }
        Ok(&self.civs)
    }

    fn catalog_table(
        &mut self,
        t: gc::CatalogTable,
    ) -> Result<Vec<protocol::CatalogEntry>, CoreError> {
        let list = self
            .game
            .catalog(t)
            .map_err(failed(&format!("Q_CATALOG {t:?}")))?;
        Ok(list.iter().map(wb::catalog_entry).collect())
    }

    /// The command of the core for a worker order.
    fn core_command(&mut self, cmd: &protocol::Command) -> Result<gc::Command, Refusal> {
        use protocol::Command as P;
        match *cmd {
            P::CancelEndTurn => Ok(gc::Command::CancelEndTurn),
            P::AnswerPeace { from, accept: true } => {
                let team = self.team_of(from).map_err(|e| Refusal::other(e.message))?;
                Ok(gc::Command::MakePeace {
                    team: i32::from(team),
                })
            }
            P::AnswerPeace { .. } | P::ProposePeace { .. } => Err(Refusal::other(
                "an offer of peace between humans is kept by the worker, not by the core",
            )),
            ref other => wb::core_command(other).map_err(Refusal::other),
        }
    }

    fn give(&mut self, player: PlayerId, cmd: &gc::Command) -> Result<(), Refusal> {
        let p = u8::try_from(player).map_err(|_| Refusal::other(format!("no player {player}")))?;
        match self.game.command(p, cmd) {
            Ok(()) => Ok(()),
            Err(gc::CommandError::Rejected { reason, message }) => Err(refusal_of(reason, message)),
            Err(gc::CommandError::Other(e)) => Err(Refusal::other(e.to_string())),
        }
    }

    /// A v15 order (build queue, research): checked against the core's answers and given to the core as the
    /// net messages of the original UI (see [`v15_net`]). A failed check is `Invalid`; a refusal of
    /// `gc_net_submit` keeps the core's reason.
    pub fn typed_net(&mut self, player: PlayerId, cmd: &protocol::Command) -> Result<(), Refusal> {
        use protocol::Command as P;
        let p =
            u8::try_from(player).map_err(|_| Refusal::invalid(format!("no player {player}")))?;
        let msgs = match *cmd {
            P::ResearchSet { .. } => {
                let techs = self
                    .game
                    .catalog(gc::CatalogTable::Technologies)
                    .map_err(|e| Refusal::other(format!("Q_CATALOG technologies: {e}")))?
                    .len();
                // A core without `Q_TECHS` cannot say what the team has: the core's own rule then applies (a
                // known technology changes nothing).
                let known = self
                    .game
                    .techs(p)
                    .map(|rows| {
                        rows.iter()
                            .filter(|t| t.status == gc::TechStatus::Have)
                            .map(|t| t.tech)
                            .collect()
                    })
                    .unwrap_or_default();
                v15_net(cmd, None, &ResearchCheck { techs, known })
            }
            P::CityQueuePush { city, .. }
            | P::CityQueuePop { city, .. }
            | P::CityQueueMove { city, .. }
            | P::CityReplaceHead { city, .. } => {
                let c = self.queue_city(player, city)?;
                v15_net(cmd, c.as_ref(), &ResearchCheck::default())
            }
            _ => return Err(Refusal::other("not a v15 queue or research order")),
        }
        .map_err(Refusal::invalid)?;
        self.game
            .net_submit(p, &msgs)
            .map_err(|e| match gc::CommandError::from(e) {
                gc::CommandError::Rejected { reason, message } => refusal_of(reason, message),
                gc::CommandError::Other(e) => Refusal::other(e.to_string()),
            })
    }

    /// The seat's city `city` for the checks of a queue order; `None`: the seat has no such city.
    fn queue_city(&mut self, player: PlayerId, city: i32) -> Result<Option<QueueCity>, Refusal> {
        let p =
            u8::try_from(player).map_err(|_| Refusal::invalid(format!("no player {player}")))?;
        let team = self
            .team_of(player)
            .map_err(|e| Refusal::other(e.message))?;
        let cities = self
            .game
            .cities(i32::from(team))
            .map_err(|e| Refusal::other(format!("Q_CITIES: {e}")))?;
        let Some(c) = cities
            .into_iter()
            .find(|c| c.owner == player && c.id == city)
        else {
            return Ok(None);
        };
        let builds = self
            .game
            .city_builds(p, city)
            .map_err(|e| Refusal::other(format!("Q_CITY_BUILDS: {e}")))?;
        // A core without `Q_CITY_QUEUE` (or one that fails on it) leaves the slots past the end to the core.
        let queue_len = self.game.city_queue(p, city).ok().map(|q| q.len());
        Ok(Some(QueueCity {
            id: city,
            puppet: c.flags.puppet(),
            has_head: c.production_order.is_some(),
            queue_len,
            builds,
        }))
    }

    /// The core itself, for read queries the host has no method for (tests and diagnostics: the UI v2 queries
    /// `Q_CITY_QUEUE`, `Q_TECHS`, ...). Orders go through [`Core`], never through this.
    pub fn gamecore_mut(&mut self) -> &mut gc::Game {
        &mut self.game
    }

    /// Journal entries of a batch of core events, addressed (GH-R6).
    fn journal_of(&mut self, events: Vec<GameEvent>) -> Vec<CoreEvent> {
        let mut turn = self.turn;
        let mut out = Vec::new();
        let mut war_ctx: Option<(Vec<gc::HumanSlot>, Vec<PlayerView>)> = None;
        for ev in &events {
            match ev {
                GameEvent::TurnStarted { turn: t } => turn = *t,
                GameEvent::WarState { team_a, team_b, .. } => {
                    if war_ctx.is_none() {
                        let slots = self.game.turn_state().unwrap_or_default();
                        let players = self
                            .game
                            .players(-1)
                            .map(|l| l.iter().map(wb::player_view).collect())
                            .unwrap_or_default();
                        war_ctx = Some((slots, players));
                    }
                    let Some((slots, players)) = &war_ctx else {
                        continue;
                    };
                    for s in slots.iter().filter(|s| s.flags.human()) {
                        let team = i32::from(s.team);
                        if team != *team_a && team != *team_b {
                            continue;
                        }
                        let Ok(to) = PlayerId::try_from(s.player) else {
                            continue;
                        };
                        for entry in journal::entries(
                            std::slice::from_ref(ev),
                            turn,
                            Some(team),
                            players,
                            None,
                        ) {
                            out.push(CoreEvent {
                                to: EventTarget::Player(to),
                                entry,
                            });
                        }
                    }
                }
                _ => {
                    if let Some(entry) = journal::entry(ev, turn) {
                        out.push(CoreEvent {
                            to: addressee(ev),
                            entry,
                        });
                    }
                }
            }
        }
        self.turn = turn;
        out
    }
}

/// Whom a core event is for: the human it names, else everybody.
fn addressee(ev: &GameEvent) -> EventTarget {
    let player = match ev {
        GameEvent::NotificationAdded { player, .. }
        | GameEvent::AutoChoice { player, .. }
        | GameEvent::PopupInfo { player, .. }
        | GameEvent::Message { player, .. } => *player,
        GameEvent::DiploAutoDeclined { human, .. } => *human,
        _ => -1,
    };
    match PlayerId::try_from(player) {
        Ok(p) if p >= 0 => EventTarget::Player(p),
        _ => EventTarget::All,
    }
}

impl Core for RealCore {
    fn start(&mut self, start: CoreStart<'_>) -> Result<(), CoreError> {
        match start {
            CoreStart::New { setup, seed } => {
                let core = network_setup(setup, seed)
                    .map_err(|m| CoreError::new(CoreErrorKind::BadStart, m))?;
                self.game.new_game(&core).map_err(|e| {
                    CoreError::new(
                        CoreErrorKind::BadStart,
                        format!("the game did not start: {e}"),
                    )
                })?;
            }
            CoreStart::Load(bytes) => {
                self.game.load(bytes).map_err(|e| {
                    CoreError::new(
                        CoreErrorKind::BadSave,
                        format!("the save did not load: {e}"),
                    )
                })?;
            }
        }
        // The server of the replica: every input from here on is journaled. A core without the replica ABI
        // (the stub) refuses, and the worker then sends no replica stream.
        self.replica_on = match self.game.set_role(gc::Role::Server) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("the core is not a replica server: {e}");
                false
            }
        };
        self.refresh_map().map_err(|mut e| {
            e.kind = CoreErrorKind::BadStart;
            e
        })
    }

    fn catalog(&mut self) -> Result<protocol::Catalog, CoreError> {
        use gc::CatalogTable as T;
        Ok(protocol::Catalog {
            units: self.catalog_table(T::Units)?,
            buildings: self.catalog_table(T::Buildings)?,
            projects: self.catalog_table(T::Projects)?,
            techs: self.catalog_table(T::Technologies)?,
            policies: self.catalog_table(T::Policies)?,
            terrains: self.catalog_table(T::Terrains)?,
            features: self.catalog_table(T::Features)?,
            resources: self.catalog_table(T::Resources)?,
            improvements: self.catalog_table(T::Improvements)?,
            routes: self.catalog_table(T::Routes)?,
            civs: self.catalog_table(T::Civilizations)?,
            leaders: self.catalog_table(T::Leaders)?,
            eras: self.catalog_table(T::Eras)?,
            handicaps: self.catalog_table(T::Handicaps)?,
            promotions: self.catalog_table(T::Promotions)?,
            art_techs: self
                .catalog_table(T::ArtTechs)?
                .into_iter()
                .map(|e| e.id)
                .collect(),
        })
    }

    fn choices(&mut self) -> Result<LobbyChoices, CoreError> {
        let pick = |list: Vec<protocol::CatalogEntry>| -> Vec<protocol::LobbyChoice> {
            list.into_iter()
                .map(|e| protocol::LobbyChoice {
                    type_name: e.type_name,
                    name: e.name,
                })
                .collect()
        };
        // The hover tips (leader, traits, unique units and buildings) are a separate catalog table (v4); a core
        // without it (a stub) shows no tip.
        let infos = self.game.civ_infos().unwrap_or_default();
        let civs = self
            .catalog_table(gc::CatalogTable::Civilizations)?
            .into_iter()
            // The pull-down of the original lists `Playable = 1` civilizations only, not the city-states or the barbarians.
            .filter(|e| lobby_options::playable_civ(&e.type_name))
            .map(|e| {
                let info = infos.iter().find(|i| i.type_name == e.type_name);
                CivChoice {
                    leader: info.map(|i| i.leader.clone()).unwrap_or_default(),
                    traits: info.map(|i| i.traits.clone()).unwrap_or_default(),
                    unique: info.map(|i| i.unique.clone()).unwrap_or_default(),
                    id: e.type_name,
                    name: e.name,
                }
            })
            .collect();
        let handicaps = pick(self.catalog_table(gc::CatalogTable::Handicaps)?);
        let eras = pick(self.catalog_table(gc::CatalogTable::Eras)?);
        let timers = self
            .game
            .catalog_timers()
            .map_err(failed("Q_CATALOG turn timers"))?
            .into_iter()
            .map(|t| protocol::LobbyChoice {
                type_name: t.type_name,
                name: t.name,
            })
            .collect();
        // The options of the staging room: game options, victories, map script options, city-states (Q_CATALOG
        // tables 19 to 23).
        let desc = self
            .game
            .describe_setup()
            .map_err(failed("Q_CATALOG setup options"))?;
        let max_minor_civs = lobby_options::minor_civ_count(&desc);
        // World sizes and speeds are rows of the game data (Q_CATALOG tables 17 and 18). A map size lets as
        // many players as the game data's default for it (the original rule) and as many city-states as the
        // game data has, as far as the core takes them (the original slider).
        let world_sizes = self
            .game
            .catalog_world_sizes()
            .map_err(failed("Q_CATALOG world sizes"))?
            .into_iter()
            .map(|w| {
                let players = u8::try_from(w.default_players).unwrap_or(0);
                let minor = u8::try_from(w.default_minor).unwrap_or(0);
                protocol::WorldSizeChoice {
                    id: w.type_name,
                    name: w.name,
                    width: u16::try_from(w.width).unwrap_or(0),
                    height: u16::try_from(w.height).unwrap_or(0),
                    default_players: players,
                    max_players: players,
                    default_minor: minor,
                    max_minor: lobby_options::max_minor_of(max_minor_civs).max(minor),
                }
            })
            .collect();
        let speeds = pick(self.catalog_table(gc::CatalogTable::GameSpeeds)?);
        let mut choices = LobbyChoices {
            hash: 0,
            civs,
            handicaps,
            world_sizes,
            map_scripts: self.map_scripts.clone(),
            speeds,
            eras,
            timers,
            options: lobby_options::options_of(&desc),
            victories: lobby_options::victories_of(&desc),
            map_options: lobby_options::map_options_of(&desc, &self.map_scripts),
            max_minor_civs,
            mods: Vec::new(),
        };
        let bytes = Postcard
            .encode(&choices)
            .map_err(|e| CoreError::failed(format!("choices: {e}")))?;
        choices.hash = protocol::xxh64(0, &bytes);
        Ok(choices)
    }

    fn build_id(&self) -> String {
        self.build_id.clone()
    }

    fn map(&mut self) -> MapInfo {
        self.map
    }

    fn seats(&mut self) -> Result<Vec<SlotSetup>, CoreError> {
        let slots = self.slots()?;
        let players = self.game.players(-1).map_err(failed("Q_PLAYERS"))?;
        let civs = self.civ_types()?.clone();
        let mut out = Vec::new();
        for p in players
            .iter()
            .filter(|p| !p.flags.minor() && !p.flags.barbarian() && !p.flags.observer())
        {
            let Ok(id) = u8::try_from(p.id) else {
                continue;
            };
            let civ = p
                .civ_type
                .and_then(|c| civs.get(&c))
                .map_or(CivPick::Random, |t| CivPick::Civ(t.clone()));
            let control = match slots.iter().find(|s| s.player == id) {
                Some(s) if s.flags.human() => SlotControl::Human {
                    nick: s
                        .nick
                        .clone()
                        .or_else(|| p.nick.clone())
                        .unwrap_or_default(),
                },
                _ => SlotControl::Ai,
            };
            out.push(SlotSetup {
                player: p.id,
                civ,
                control,
            });
        }
        out.sort_by_key(|s| s.player);
        Ok(out)
    }

    fn last_pump_slices(&self) -> u32 {
        self.last_slices
    }

    fn turn_status(&mut self) -> Vec<AiTurnEvent> {
        self.pull();
        std::mem::take(&mut self.turn_status)
    }

    fn pump(&mut self, budget: u32) -> Result<CorePump, CoreError> {
        let r = self.game.pump(budget).map_err(failed("pump"))?;
        self.last_slices = u32::try_from(r.slices_used).unwrap_or(0);
        self.turn = r.turn;
        self.pull();
        let phase = match gc::GameStatus::from_pump(&r) {
            gc::GameStatus::WaitingHuman => CorePhase::Humans,
            gc::GameStatus::AiRunning => CorePhase::Ai {
                active: PlayerId::try_from(r.active_player).unwrap_or(0),
            },
            gc::GameStatus::GameOver => {
                let info = self.game.game_info().map_err(failed("Q_GAME"))?;
                CorePhase::GameOver {
                    winner: (info.winner >= 0).then_some(info.winner),
                }
            }
            gc::GameStatus::Stuck | gc::GameStatus::Other(_) => {
                return Err(CoreError::new(
                    CoreErrorKind::Stuck,
                    format!("the core is stuck (pump status {})", r.status),
                ));
            }
        };
        Ok(CorePump {
            phase,
            turn: r.turn,
        })
    }

    fn turn(&mut self) -> Result<CoreTurn, CoreError> {
        self.pull();
        let info = self.game.game_info().map_err(failed("Q_GAME"))?;
        let slots = self.slots()?;
        let seats = slots
            .iter()
            .filter(|s| s.flags.human())
            .filter_map(|s| {
                Some(SeatTurnInfo {
                    player: PlayerId::try_from(s.player).ok()?,
                    // A dead human has nothing left to end: nobody waits for him (the timer, the "waiting for" line).
                    finished: s.flags.turn_ended() || !s.flags.alive(),
                    may_act: s.flags.turn_active(),
                    simultaneous: s.flags.simultaneous(),
                })
            })
            .collect();
        Ok(CoreTurn {
            mode: info.turn_mode.map_or(TurnMode::Hybrid, turn_mode_of),
            seats,
            turn_len_s: u32::try_from(info.turn_len_s).unwrap_or(0),
            timer_base_s: u32::try_from(info.turn_base_s).unwrap_or(0),
            human_wars: std::mem::take(&mut self.pending_wars),
        })
    }

    fn command(&mut self, player: PlayerId, cmd: &protocol::Command) -> Result<(), Refusal> {
        if is_v15_order(cmd) {
            return self.typed_net(player, cmd);
        }
        let core = self.core_command(cmd)?;
        self.give(player, &core)
    }

    fn end_turn_timeout(&mut self, player: PlayerId) -> Result<(), Refusal> {
        self.give(player, &gc::Command::EndTurnTimeout)
    }

    fn set_control(&mut self, player: PlayerId, control: &SlotControl) -> Result<(), CoreError> {
        let p = player_u8(player)?;
        let slot = self.slots()?.into_iter().find(|s| s.player == p);
        let r = match control {
            SlotControl::Human { nick } => {
                // The same owner again changes nothing.
                if slot.as_ref().is_some_and(|s| {
                    s.flags.human()
                        && s.flags.connected()
                        && !s.flags.kicked()
                        && s.nick.as_deref() == Some(nick.as_str())
                }) {
                    return Ok(());
                }
                self.game.slot_control(p, gc::SlotOp::Connect, Some(nick))
            }
            SlotControl::Ai => {
                if slot.as_ref().is_none_or(|s| !s.flags.human()) {
                    return Ok(());
                }
                self.game.slot_control(p, gc::SlotOp::Kick, None)
            }
        };
        r.map_err(|e| CoreError::failed(format!("slot {player}: {e}")))
    }

    fn safe_point(&mut self) -> Result<(), String> {
        let info = self.game.game_info().map_err(|e| e.to_string())?;
        if info.safe_point {
            Ok(())
        } else {
            Err(match info.unsafe_reason {
                Some(why) => format!("not a safe point: {why:?}"),
                None => "not a safe point".into(),
            })
        }
    }

    fn save(&mut self) -> Result<Vec<u8>, CoreError> {
        // `gc_save` refuses away from a save point (a human's unit is still busy with a move after the pump
        // stopped for the other humans): that is `NotSafe`, which the worker retries, never a fatal error.
        self.game.save().map_err(|e| match e {
            GameError::Rejected { .. } => {
                CoreError::new(CoreErrorKind::NotSafe, format!("save: {e}"))
            }
            e => core_err("save", e),
        })
    }

    fn dirty_plots(&mut self) -> Option<Vec<u32>> {
        match self.game.plots_dirty() {
            Ok(list) => list,
            Err(e) => {
                tracing::warn!("Q_PLOTS_DIRTY failed, all plots are taken: {e}");
                None
            }
        }
    }

    fn plots(
        &mut self,
        team: TeamId,
        subset: Option<&[u32]>,
        out: &mut Vec<(u32, protocol::PlotView)>,
    ) -> Result<(), CoreError> {
        let viewer = i32::from(team);
        let list = match subset {
            None => self.game.plots(viewer),
            Some(s) => self.game.plots_at(viewer, s),
        }
        .map_err(failed("Q_PLOTS"))?;
        let w = u32::from(self.map.width);
        let start = out.len();
        let mut dark = Vec::new();
        for p in &list {
            let index = u32::try_from(p.y).unwrap_or(0) * w + u32::try_from(p.x).unwrap_or(0);
            if !p.revealed() {
                dark.push((out.len(), index));
            }
            out.push((index, wb::plot_view(p).map_err(CoreError::failed)?));
        }
        // The whole world goes to the client (NP-D19, 2026-10-22): an unrevealed plot gets its world fields
        // from the truth, without what only sight gives.
        if !dark.is_empty() {
            let indexes: Vec<u32> = dark.iter().map(|&(_, i)| i).collect();
            let truth = self
                .game
                .plots_at(-1, &indexes)
                .map_err(failed("Q_PLOTS"))?;
            for (&(at, _), t) in dark.iter().zip(&truth) {
                let world = wb::plot_view(t).map_err(CoreError::failed)?;
                out[at].1 = protocol::PlotView::unrevealed_of(&world);
            }
        }
        debug_assert!(out[start..].len() == list.len());
        Ok(())
    }

    fn owners(
        &mut self,
        subset: Option<&[u32]>,
        out: &mut Vec<(u32, i8)>,
    ) -> Result<(), CoreError> {
        // Viewer -1 is the whole truth: the owner as the core holds it now, revealed or not.
        let list = match subset {
            None => self.game.plots(-1),
            Some(s) => self.game.plots_at(-1, s),
        }
        .map_err(failed("Q_PLOTS"))?;
        let w = u32::from(self.map.width);
        for p in &list {
            let index = u32::try_from(p.y).unwrap_or(0) * w + u32::try_from(p.x).unwrap_or(0);
            let owner = p.owner.and_then(|o| i8::try_from(o).ok()).unwrap_or(-1);
            out.push((index, owner));
        }
        Ok(())
    }

    fn basis(
        &mut self,
        subset: Option<&[u32]>,
        out: &mut Vec<(u32, protocol::PlotBasisView)>,
    ) -> Result<(), CoreError> {
        let list = self
            .game
            .plot_basis(subset)
            .map_err(failed("Q_PLOT_BASIS"))?;
        for b in &list {
            out.push(wb::basis_view(b).map_err(CoreError::failed)?);
        }
        Ok(())
    }

    fn units(&mut self, team: TeamId) -> Result<Vec<protocol::UnitView>, CoreError> {
        let list = self
            .game
            .units(i32::from(team))
            .map_err(failed("Q_UNITS"))?;
        Ok(list.iter().map(wb::unit_view).collect())
    }

    fn cities(&mut self, team: TeamId) -> Result<Vec<protocol::CityView>, CoreError> {
        let viewer = i32::from(team);
        let list = self.game.cities(viewer).map_err(failed("Q_CITIES"))?;
        let art = match self.game.city_art(viewer) {
            Ok(a) => a,
            Err(GameError::Core(GcError::NotImplemented)) => Vec::new(),
            Err(e) => return Err(core_err("Q_CITY_ART", e)),
        };
        let mut buildings: BTreeMap<(i8, i32), Vec<i16>> = BTreeMap::new();
        for a in &art {
            if let Ok(owner) = i8::try_from(a.owner) {
                buildings
                    .entry((owner, a.city_id))
                    .or_default()
                    .push(a.building);
            }
        }
        Ok(list
            .iter()
            .map(|c| {
                if !c.flags.visible() {
                    // Not in sight: the core has no live values, only the place and the name; the worker
                    // keeps the last seen ones (game-host-mp 4.3 item 6).
                    return protocol::CityView {
                        id: c.id,
                        owner: c.owner,
                        x: c.x,
                        y: c.y,
                        name: c.name.clone(),
                        ..protocol::CityView::default()
                    };
                }
                let mut v = wb::city_view(c);
                if let Some(mut b) = buildings.remove(&(c.owner, c.id)) {
                    b.sort_unstable();
                    b.dedup();
                    v.buildings = b;
                }
                v
            })
            .collect())
    }

    fn trade_routes(&mut self, team: TeamId) -> Result<Vec<protocol::TradeRouteView>, CoreError> {
        let list = match self.game.trade_routes(i32::from(team)) {
            Ok(l) => l,
            Err(GameError::Core(GcError::NotImplemented)) => Vec::new(),
            Err(e) => return Err(core_err("Q_TRADE_ROUTES", e)),
        };
        Ok(list.iter().map(wb::trade_route_view).collect())
    }

    fn players(&mut self, team: TeamId) -> Result<Vec<PlayerView>, CoreError> {
        let list = self
            .game
            .players(i32::from(team))
            .map_err(failed("Q_PLAYERS"))?;
        Ok(list.iter().map(wb::player_view).collect())
    }

    fn header(&mut self, player: PlayerId) -> Result<ViewHeader, CoreError> {
        let p = player_u8(player)?;
        let team = self.team_of(player)?;
        // A human who lost everything is dead: the core answers no per-player Q_GAME for him. He keeps his
        // seat (he may watch, or leave) and sees the end for himself, while the game goes on for the others
        // (the original: the dead local player gets the game over of his own machine; the victory ends the
        // game for everybody, and then the global status says so for every seat).
        let dead = self.is_dead(p)?;
        if dead {
            let info = self.game.game_info().map_err(failed("Q_GAME"))?;
            return Ok(ViewHeader {
                turn: info.turn,
                year: info.year,
                human: player,
                human_team: team,
                status: TurnStatus::GameOver {
                    winner: (info.winner >= 0).then_some(info.winner),
                },
                blocker: None,
            });
        }
        let info = self.game.game_info_for(p).map_err(failed("Q_GAME"))?;
        Ok(ViewHeader {
            turn: info.turn,
            year: info.year,
            human: player,
            human_team: team,
            status: wb::turn_status(&info).map_err(|m| CoreError::new(CoreErrorKind::Stuck, m))?,
            blocker: wb::blocker(&info),
        })
    }

    fn top_bar(&mut self, player: PlayerId) -> Result<TopBar, CoreError> {
        if self.is_dead(player_u8(player)?)? {
            return Ok(TopBar::default());
        }
        let t = self
            .game
            .top_bar(player_u8(player)?)
            .map_err(failed("Q_TOP_BAR"))?;
        Ok(wb::top_bar(&t))
    }

    fn notifications(&mut self, player: PlayerId) -> Result<Vec<NotificationView>, CoreError> {
        if self.is_dead(player_u8(player)?)? {
            return Ok(Vec::new());
        }
        let list = self
            .game
            .notifications(player_u8(player)?)
            .map_err(failed("Q_NOTIFICATIONS"))?;
        Ok(list.iter().map(wb::notification_view).collect())
    }

    fn unit_actions(
        &mut self,
        player: PlayerId,
        unit: i32,
    ) -> Result<Vec<UnitActionView>, CoreError> {
        let list = self
            .game
            .unit_actions(player_u8(player)?, unit)
            .map_err(failed("Q_UNIT_ACTIONS"))?;
        Ok(list.iter().map(wb::action_view).collect())
    }

    fn reach(
        &mut self,
        player: PlayerId,
        unit: i32,
        horizon: u8,
    ) -> Result<Vec<ReachNode>, CoreError> {
        let p = player_u8(player)?;
        let team = self.team_of(player)?;
        let root = self
            .game
            .units(i32::from(team))
            .map_err(failed("Q_UNITS"))?
            .into_iter()
            .find(|u| u.owner == player && u.id == unit)
            .ok_or_else(|| CoreError::failed(format!("player {player} has no unit {unit}")))?;
        let tree = self
            .game
            .reachable_tree(p, unit, horizon)
            .map_err(failed("Q_REACHABLE_TREE"))?;
        // The core lists the nodes with the plot they come from; the protocol wants parents before
        // children and the index of the parent.
        let mut children: HashMap<(i16, i16), Vec<usize>> = HashMap::new();
        for (i, n) in tree.iter().enumerate() {
            children
                .entry((n.parent_x, n.parent_y))
                .or_default()
                .push(i);
        }
        let mut out = vec![ReachNode {
            x: root.x,
            y: root.y,
            parent: 0,
            turn: 0,
            moves_left: root.moves_left,
            flags: protocol::ReachFlags::default(),
        }];
        let mut at = 0;
        while at < out.len() {
            let from = (out[at].x, out[at].y);
            if let Some(kids) = children.remove(&from) {
                for k in kids {
                    let n = &tree[k];
                    out.push(ReachNode {
                        x: n.x,
                        y: n.y,
                        parent: u16::try_from(at).unwrap_or(0),
                        turn: n.turn,
                        moves_left: n.moves_left,
                        flags: wb::reach_flags(n.flags),
                    });
                }
            }
            at += 1;
        }
        if out.len() != tree.len() + 1 {
            tracing::warn!(
                "move tree of unit {unit}: {} of {} nodes hang off no parent",
                tree.len() + 1 - out.len(),
                tree.len()
            );
        }
        Ok(out)
    }

    fn targets(
        &mut self,
        player: PlayerId,
        shooter: Shooter,
    ) -> Result<Vec<(i16, i16, u16)>, CoreError> {
        let list = self
            .game
            .range_targets(player_u8(player)?, wb::range_shooter(shooter))
            .map_err(failed("Q_RANGE_TARGETS"))?;
        Ok(list
            .iter()
            .map(|t| (t.x, t.y, u16::from(t.flags.city())))
            .collect())
    }

    fn preview(
        &mut self,
        player: PlayerId,
        attacker: Shooter,
        x: i16,
        y: i16,
    ) -> Result<Option<CombatPreview>, CoreError> {
        let r = self
            .game
            .combat_preview(player_u8(player)?, wb::range_shooter(attacker), x, y)
            .map_err(failed("Q_COMBAT_PREVIEW"))?;
        Ok(r.as_ref().map(wb::combat_preview))
    }

    fn city_builds(
        &mut self,
        player: PlayerId,
        city: i32,
    ) -> Result<Vec<protocol::CityBuildView>, CoreError> {
        let list = self
            .game
            .city_builds(player_u8(player)?, city)
            .map_err(failed("Q_CITY_BUILDS"))?;
        Ok(list.iter().map(wb::build_view).collect())
    }

    fn path(
        &mut self,
        player: PlayerId,
        unit: i32,
        x: i16,
        y: i16,
    ) -> Result<Vec<PathNode>, CoreError> {
        let list = self
            .game
            .path(player_u8(player)?, unit, i32::from(x), i32::from(y))
            .map_err(failed("Q_PATH"))?;
        Ok(list.iter().map(wb::path_node).collect())
    }

    fn can_make_peace(&mut self, a: PlayerId, b: PlayerId) -> Result<bool, CoreError> {
        let rel = self.game.diplo(player_u8(a)?).map_err(failed("Q_DIPLO"))?;
        Ok(rel
            .iter()
            .find(|r| r.other == b)
            .is_some_and(|r| r.flags.can_make_peace()))
    }

    fn events(&mut self) -> Vec<CoreEvent> {
        self.pull();
        let events = std::mem::take(&mut self.backlog);
        self.journal_of(events)
    }

    fn digest(&mut self, viewer_team: i32) -> Result<u64, CoreError> {
        self.game
            .state_digest(viewer_team)
            .map_err(|e| CoreError::failed(format!("digest: {e}")))
    }

    fn replica(&mut self) -> Option<&mut dyn ReplicaOps> {
        if self.replica_on { Some(self) } else { None }
    }
}

impl ReplicaOps for RealCore {
    fn journal_take(&mut self, check: bool) -> Result<Vec<u8>, CoreError> {
        self.game.journal_take(check).map_err(failed("journal"))
    }

    fn net_submit(&mut self, player: PlayerId, msgs: &[u8]) -> Result<(), String> {
        let list = gc::replica::decode_net(msgs).map_err(|e| e.to_string())?;
        let p = u8::try_from(player).map_err(|_| format!("no player {player}"))?;
        self.game.net_submit(p, &list).map_err(|e| match e {
            GameError::Rejected { message, .. } => message,
            other => other.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{AutoChoices, GameSettings};

    fn setup_with(game_options: Vec<(String, i32)>) -> GameSetup {
        GameSetup {
            settings: GameSettings {
                name: "t".into(),
                world_size: "WORLDSIZE_DUEL".into(),
                map_script: "assets/maps/continents.lua".into(),
                map_options: vec![("1".into(), 3)],
                speed: "GAMESPEED_STANDARD".into(),
                start_era: "ERA_ANCIENT".into(),
                handicap: "HANDICAP_PRINCE".into(),
                minor_civs: 5,
                turn_mode: TurnMode::Hybrid,
                timer: TimerMode::Off,
                game_options,
                seed: Some(7),
                map_seed: None,
                auto_choices: AutoChoices::default(),
                diplo_auto_decline: true,
                mods: Vec::new(),
            },
            slots: vec![
                SlotSetup {
                    player: 0,
                    civ: CivPick::Random,
                    control: SlotControl::Human { nick: "a".into() },
                },
                SlotSetup {
                    player: 1,
                    civ: CivPick::Random,
                    control: SlotControl::Ai,
                },
            ],
        }
    }

    #[test]
    fn victories_and_max_turns_become_setup_fields() {
        let s = setup_with(vec![
            ("GAMEOPTION_NO_BARBARIANS".into(), 1),
            ("VICTORY_TIME".into(), 0),
            ("MAX_TURNS".into(), 300),
        ]);
        let core = network_setup(&s, 1).unwrap();
        assert_eq!(
            core.game_options,
            vec![("GAMEOPTION_NO_BARBARIANS".to_owned(), 1)]
        );
        assert_eq!(core.victories, vec![("VICTORY_TIME".to_owned(), 0)]);
        assert_eq!(core.max_turns, 300);
        assert_eq!(core.map_options, vec![("1".to_owned(), 3)]);
        let json = core.to_json();
        assert!(
            json.contains("\"game_options\":{\"GAMEOPTION_NO_BARBARIANS\":1}"),
            "{json}"
        );
        assert!(
            json.contains("\"victories\":{\"VICTORY_TIME\":0},\"max_turns\":300}"),
            "{json}"
        );
    }

    #[test]
    fn a_plain_room_writes_neither_key() {
        let core = network_setup(&setup_with(Vec::new()), 1).unwrap();
        assert!(core.victories.is_empty() && core.max_turns == 0);
        let json = core.to_json();
        assert!(
            !json.contains("victories") && !json.contains("max_turns"),
            "{json}"
        );
    }

    // ------------------------------------------------------------ v15 orders as net messages

    use gamecore_sys::ffi;
    use protocol::Command as P;

    fn build(order: gc::OrderType, item: i16, flags: u16) -> gc::CityBuild {
        gc::CityBuild {
            order,
            item,
            turns: Some(3),
            flags: gc::BuildFlags(flags),
            production_cost: 40,
            gold_cost: None,
            faith_cost: None,
            reason: (flags & ffi::GC_BUILD_F_ENABLED == 0).then(|| "needs Pottery".to_owned()),
        }
    }

    /// City 7 builds warrior 2 (the head of a queue of four); it could train unit 3, build building 5, not
    /// building 6.
    fn city7() -> QueueCity {
        let on = ffi::GC_BUILD_F_ENABLED;
        QueueCity {
            id: 7,
            puppet: false,
            has_head: true,
            queue_len: Some(4),
            builds: vec![
                build(gc::OrderType::Train, 2, on | ffi::GC_BUILD_F_CURRENT),
                build(gc::OrderType::Train, 3, on),
                build(gc::OrderType::Construct, 5, on),
                build(gc::OrderType::Construct, 6, 0),
            ],
        }
    }

    /// `(name, leading args)` of the messages.
    fn plan(cmd: P, city: Option<&QueueCity>) -> Result<Vec<(&'static str, Vec<i32>)>, String> {
        plan_with(cmd, city, &[])
    }

    /// As [`plan`], with the technologies the team has.
    fn plan_with(
        cmd: P,
        city: Option<&QueueCity>,
        known: &[i16],
    ) -> Result<Vec<(&'static str, Vec<i32>)>, String> {
        let research = ResearchCheck {
            techs: 80,
            known: known.to_vec(),
        };
        Ok(v15_net(&cmd, city, &research)?
            .iter()
            .map(|m| (m.name(), m.args[..6].to_vec()))
            .collect())
    }

    #[test]
    fn a_push_is_send_push_order_with_ctrl_for_the_back_and_without_for_the_front() {
        let c = city7();
        let push = |front| P::CityQueuePush {
            city: 7,
            order: 1,
            item: 5,
            front,
        };
        assert_eq!(
            plan(push(false), Some(&c)),
            Ok(vec![("sendPushOrder", vec![7, 1, 5, 0, 0, 1])])
        );
        assert_eq!(
            plan(push(true), Some(&c)),
            Ok(vec![("sendPushOrder", vec![7, 1, 5, 0, 0, 0])])
        );
    }

    #[test]
    fn pop_move_and_replace_head_use_the_messages_of_the_city_screen() {
        let c = city7();
        assert_eq!(
            plan(P::CityQueuePop { city: 7, slot: 2 }, Some(&c)),
            Ok(vec![("sendPopOrder", vec![7, 2, 0, 0, 0, 0])])
        );
        let swaps = |from, to| {
            plan(P::CityQueueMove { city: 7, from, to }, Some(&c))
                .unwrap()
                .into_iter()
                .map(|(n, a)| {
                    assert_eq!((n, a[0]), ("sendSwapOrder", 7));
                    a[1]
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(swaps(0, 3), vec![0, 1, 2]);
        assert_eq!(swaps(3, 1), vec![2, 1]);
        assert_eq!(swaps(1, 0), vec![0]);
        assert_eq!(
            plan(
                P::CityReplaceHead {
                    city: 7,
                    order: 0,
                    item: 3
                },
                Some(&c)
            ),
            Ok(vec![
                ("sendPopOrder", vec![7, 0, 0, 0, 0, 0]),
                ("sendPushOrder", vec![7, 0, 3, 0, 0, 0]),
            ])
        );
    }

    #[test]
    fn research_is_send_research_with_shift_for_the_queue() {
        assert_eq!(
            plan(
                P::ResearchSet {
                    tech: 12,
                    queue: false
                },
                None
            ),
            Ok(vec![("sendResearch", vec![12, 0, -1, 0, 0, 0])])
        );
        assert_eq!(
            plan(
                P::ResearchSet {
                    tech: 12,
                    queue: true
                },
                None
            ),
            Ok(vec![("sendResearch", vec![12, 0, -1, 1, 0, 0])])
        );
        for tech in [-1, 80, 300] {
            assert!(plan(P::ResearchSet { tech, queue: false }, None).is_err());
        }
    }

    #[test]
    fn a_technology_the_team_has_is_refused_and_a_locked_one_is_not() {
        let known = [0i16, 1];
        for queue in [false, true] {
            assert!(
                plan_with(P::ResearchSet { tech: 1, queue }, None, &known)
                    .unwrap_err()
                    .contains("known already")
            );
        }
        // Tech 40 has prerequisites the team lacks: the core queues them first, the host lets it through.
        assert_eq!(
            plan_with(
                P::ResearchSet {
                    tech: 40,
                    queue: false
                },
                None,
                &known
            ),
            Ok(vec![("sendResearch", vec![40, 0, -1, 0, 0, 0])])
        );
    }

    #[test]
    fn a_slot_past_the_end_of_the_queue_is_refused_when_the_core_tells_its_length() {
        let c = city7();
        let pop = |slot| P::CityQueuePop { city: 7, slot };
        let mv = |from, to| P::CityQueueMove { city: 7, from, to };
        assert!(plan(pop(3), Some(&c)).is_ok());
        assert!(plan(mv(3, 0), Some(&c)).is_ok());
        for bad in [pop(4), pop(i8::MAX), mv(0, 4), mv(4, 0), mv(5, 9)] {
            assert!(
                plan(bad, Some(&c)).unwrap_err().contains("queue of 4"),
                "{bad:?}"
            );
        }
        // A core without `Q_CITY_QUEUE`: the core is left to ignore the slot.
        let unknown = QueueCity {
            queue_len: None,
            ..city7()
        };
        assert!(plan(pop(9), Some(&unknown)).is_ok());
        assert!(plan(mv(0, 9), Some(&unknown)).is_ok());
    }

    #[test]
    fn what_the_city_cannot_take_is_refused_before_the_core() {
        let c = city7();
        let push = |order, item| P::CityQueuePush {
            city: 7,
            order,
            item,
            front: false,
        };
        // Not the seat's city, a puppet, an unknown order type, an item it cannot build or start.
        assert!(plan(push(0, 3), None).unwrap_err().contains("no city 7"));
        let puppet = QueueCity {
            puppet: true,
            ..city7()
        };
        assert!(
            plan(push(0, 3), Some(&puppet))
                .unwrap_err()
                .contains("puppet")
        );
        assert!(plan(push(9, 3), Some(&c)).is_err());
        assert!(plan(push(0, 99), Some(&c)).is_err());
        assert!(
            plan(push(1, 6), Some(&c))
                .unwrap_err()
                .contains("needs Pottery")
        );
        // A building that is being built cannot be queued again; a unit can.
        let head_building = QueueCity {
            builds: vec![build(
                gc::OrderType::Construct,
                5,
                ffi::GC_BUILD_F_ENABLED | ffi::GC_BUILD_F_CURRENT,
            )],
            ..city7()
        };
        assert!(plan(push(1, 5), Some(&head_building)).is_err());
        assert!(plan(push(0, 2), Some(&c)).is_ok());
        // Slots: negative, a move onto itself, an empty queue.
        assert!(plan(P::CityQueuePop { city: 7, slot: -1 }, Some(&c)).is_err());
        assert!(
            plan(
                P::CityQueueMove {
                    city: 7,
                    from: 1,
                    to: 1
                },
                Some(&c)
            )
            .is_err()
        );
        let empty = QueueCity {
            has_head: false,
            ..city7()
        };
        assert!(plan(P::CityQueuePop { city: 7, slot: 0 }, Some(&empty)).is_err());
        assert!(
            plan(
                P::CityReplaceHead {
                    city: 7,
                    order: 0,
                    item: 3
                },
                Some(&empty)
            )
            .is_err()
        );
        // Not a v15 order at all.
        assert!(plan(P::EndTurn { force: false }, Some(&c)).is_err());
    }
}
