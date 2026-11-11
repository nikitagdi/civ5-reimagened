//! Test port of the game (playtest §3.1, net-client 4.12): what a scripted player sees and does, on the local
//! game and on a server session (`ClientCore`). The shell (`app-web` `testhooks.rs`, `?test=1`) implements
//! [`TestPort`]; without one nothing here runs.
//!
//! The port gets a [`TestSnapshot`] of every frame's screens (the same view data the screens draw)
//! and hands back [`TestAction`]s, which become the [`UiAction`]s a click on those screens makes, or
//! a click on a map plot as the picker delivers it. Nothing goes to the host around the client,
//! except a [`Fault`] (the playtest's error screen check), which breaks the link on purpose.

use client_core::{ClientCore, ClientPhase};
use interim_ui::labels::LabelTarget;
use interim_ui::screens::Screens;
use interim_ui::screens::end_turn::EndTurnState;
use interim_ui::screens::lobby::NewGameChoice;
use interim_ui::screens::room::SlotWho;
use protocol::{BlockerTarget, TurnStatus, WorldView};

pub use crate::autostart::RoomParams;
/// The counters of the client, for `metrics()`.
pub use client_core::ClientStats;
/// The lobby actions of the playtest (lobby-ui 4.11) are the `UiAction`s a click makes; the shell builds them from
/// these types.
pub use interim_ui::screens::UiAction;
pub use interim_ui::screens::room::{RoomEdit, SlotKind, TimerChoice, TurnModeChoice};
pub use interim_ui::screens::settings::QualityPreset;
pub use protocol::TurnMode;

/// The journal lines a snapshot keeps (the newest).
const JOURNAL_TAIL: usize = 20;

/// The shell's side of the test port.
pub trait TestPort {
    /// This frame's screens (only while a local game runs and the screens are drawn).
    fn publish(&mut self, snapshot: TestSnapshot);
    /// The actions asked since the last frame, oldest first.
    fn take_actions(&mut self) -> Vec<TestAction>;
    /// A `golden` frame (`AppConfig::golden`) is final, or could not be made (the reason). Once.
    fn golden(&mut self, _result: Result<&crate::golden::GoldenOutput, &str>) {}
}

/// What a scripted player does; each one is what a player can do on the screens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestAction {
    /// The lobby's Start with these choices (`None`: the host's default).
    StartGame {
        civ: Option<String>,
        handicap: Option<String>,
        ai_count: u8,
        seed: Option<u32>,
    },
    ContinueGame,
    /// The end-turn button (Shift+click when `force`).
    EndTurn {
        force: bool,
    },
    /// A left click on an own unit's flag.
    SelectUnit(i32),
    /// A left click on an own city's banner.
    OpenCity(i32),
    /// A click on a map plot (right click when `secondary`), as the picker hands it over.
    MapClick {
        x: i32,
        y: i32,
        secondary: bool,
    },
    /// A unit panel button (gc-abi-v1 unit command number).
    UnitAction(i16),
    /// A build of the open city panel, by index.
    ChooseBuild(usize),
    /// A purchase button of the open city panel (gold, or faith when `faith`).
    BuyBuild {
        index: usize,
        faith: bool,
    },
    CloseCity,
    /// The answer to the open question.
    Confirm(bool),
    /// The error screen's Reload.
    Reload,
    /// Breaks the game on purpose, for the error screen check (playtest `fatal`).
    Fault(Fault),
    /// A click on a screen of the server session: login, lobby, room (lobby-ui 4.11). The same `UiAction` the
    /// screen makes.
    Ui(UiAction),
    /// The quick start with these parameters (`RoomScript`), as the lobby's [Quick start] button runs it.
    QuickStart(RoomParams),
    /// The game UI to switch to: `classic`, `v2` or `v2:<skin>` (`UiChoice::parse`). `save`: kept as the player's
    /// choice, as the settings window does; `cold`: how the classic UI wakes up from now on (`true`: its layer is
    /// made again; `None`: as it is set).
    SetUi {
        value: String,
        save: bool,
        cold: Option<bool>,
    },
    /// Computes the replica's digest (`Replica::digest`) in the next frame; `state().ui.replicaDigest` has it.
    ReplicaDigest,
}

/// A failure the test port causes (only with a test port, `?test=1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// Bytes that are no `ClientMsg`: the host answers `Fatal`; the server drops the connection.
    BadMessage,
    /// The link to the host ends (the browser worker is terminated). B1 local game only.
    KillHost,
    /// The socket is closed from our side: the client comes back by itself (net-client 4.12).
    DropSocket,
    /// A value of `base` is spoiled without its hash changing: the next diff makes the client ask for a
    /// keyframe.
    CorruptView,
    /// The socket is not read for `ms`: a probe of the liveness checks.
    Silence { ms: u32 },
}

/// The bytes of [`Fault::BadMessage`]: an over-long varint where the `ClientMsg` tag goes.
pub const BAD_MESSAGE: [u8; 12] = [0xff; 12];

/// How the app carries out a [`TestAction`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dispatch {
    Ui(UiAction),
    MapClick {
        cell: (i32, i32),
        secondary: bool,
    },
    /// The link breaks on purpose.
    Fault(Fault),
    /// A quick start of the server session.
    QuickStart(RoomParams),
    /// A switch of the game UI (see [`TestAction::SetUi`]).
    SetUi {
        value: String,
        save: bool,
        cold: Option<bool>,
    },
    /// The replica's digest for the test port.
    ReplicaDigest,
    /// Not possible now (a unit or city that is not on the map).
    Skip,
}

impl TestAction {
    /// The screen action for `self` on `world` (the flags and banners stand on their plots).
    pub fn dispatch(self, world: Option<&WorldView>) -> Dispatch {
        let cell = |x: i16, y: i16| (i32::from(x), i32::from(y));
        match self {
            TestAction::StartGame {
                civ,
                handicap,
                ai_count,
                seed,
            } => Dispatch::Ui(UiAction::StartGame(NewGameChoice {
                civ,
                handicap,
                ai_count,
                seed,
            })),
            TestAction::ContinueGame => Dispatch::Ui(UiAction::ContinueGame),
            TestAction::EndTurn { force } => Dispatch::Ui(UiAction::EndTurn { force }),
            TestAction::SelectUnit(id) => world
                .and_then(|w| w.units.iter().find(|u| u.id == id && u.owner == w.human))
                .map_or(Dispatch::Skip, |u| {
                    Dispatch::Ui(UiAction::LabelClicked {
                        target: LabelTarget::Unit(id),
                        cell: cell(u.x, u.y),
                        secondary: false,
                    })
                }),
            TestAction::OpenCity(id) => world
                .and_then(|w| w.cities.iter().find(|c| c.id == id && c.owner == w.human))
                .map_or(Dispatch::Skip, |c| {
                    Dispatch::Ui(UiAction::LabelClicked {
                        target: LabelTarget::City(id),
                        cell: cell(c.x, c.y),
                        secondary: false,
                    })
                }),
            TestAction::MapClick { x, y, secondary } => Dispatch::MapClick {
                cell: (x, y),
                secondary,
            },
            TestAction::UnitAction(a) => Dispatch::Ui(UiAction::UnitAction(a)),
            TestAction::ChooseBuild(i) => Dispatch::Ui(UiAction::ChooseBuild(i)),
            TestAction::BuyBuild { index, faith } => {
                Dispatch::Ui(UiAction::BuyBuild { index, faith })
            }
            TestAction::CloseCity => Dispatch::Ui(UiAction::CloseCity),
            TestAction::Confirm(yes) => Dispatch::Ui(UiAction::Confirm(yes)),
            TestAction::Reload => Dispatch::Ui(UiAction::Reload),
            TestAction::Fault(f) => Dispatch::Fault(f),
            TestAction::Ui(a) => Dispatch::Ui(a),
            TestAction::QuickStart(p) => Dispatch::QuickStart(p),
            TestAction::SetUi { value, save, cold } => Dispatch::SetUi { value, save, cold },
            TestAction::ReplicaDigest => Dispatch::ReplicaDigest,
        }
    }
}

/// The game UIs as the playtest sees them (`state().ui`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TestUi {
    /// The game UI on the screen: "classic", "v2"; `None`: none (interim).
    pub active: Option<&'static str>,
    /// The choice in force: `classic`, `v2` or `v2:<skin>`.
    pub choice: String,
    /// The player's saved choice (`civ5.options.ui`), when there is one.
    pub saved: Option<String>,
    /// The running UI v2 skin.
    pub skin: Option<String>,
    /// A switch is under way.
    pub switching: bool,
    /// Switches done, and the last one's time from the request to the target's first frame, ms.
    pub switches: u64,
    pub last_switch_ms: Option<u64>,
    /// Game UIs that failed ("classic", "v2") and why, oldest first.
    pub failed: Vec<(&'static str, String)>,
    /// The notice of the last failure while it shows.
    pub notice: Option<String>,
    /// Frames and input events each UI got: `[classic, v2]`.
    pub frames: [u64; 2],
    pub inputs: [u64; 2],
    /// The replica digests computed for the port so far, and the last one (hex; `None`: no replica loaded).
    pub replica_digest: Option<(u64, Option<String>)>,
}

/// One action button of the unit panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestUnitAction {
    pub action: i16,
    pub label: String,
    pub enabled: bool,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestUnitPanel {
    pub name: String,
    pub actions: Vec<TestUnitAction>,
    pub loading: bool,
    pub aiming: bool,
}

/// A purchase button: its price and whether it is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TestOffer {
    pub cost: i32,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestBuild {
    pub name: String,
    /// "Units", "Buildings", "Wonders", "Other".
    pub section: &'static str,
    pub turns: i16,
    pub current: bool,
    pub gold: Option<TestOffer>,
    pub faith: Option<TestOffer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestCityPanel {
    pub name: String,
    pub producing: Option<String>,
    pub builds: Vec<TestBuild>,
    pub loading: bool,
    pub refused: Option<String>,
    /// The empire's gold and faith the panel shows.
    pub gold: i32,
    pub faith: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestUnit {
    pub id: i32,
    pub name: String,
    pub x: i16,
    pub y: i16,
    pub hp: i16,
    pub moves_left: i16,
    pub activity: i8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestCity {
    pub id: i32,
    pub name: String,
    pub x: i16,
    pub y: i16,
    pub population: i16,
    pub production_item: i16,
    pub production_turns: i16,
}

/// What the screens show this frame, plus the human's units and cities of the world behind them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TestSnapshot {
    /// "loading", "lobby", "play", "fatal".
    pub screen: &'static str,
    /// The loading screen's stage.
    pub loading: Option<String>,
    pub fatal: Option<String>,
    /// Lobby: the civilization and difficulty keys, the save line.
    pub lobby_civs: Vec<String>,
    pub lobby_handicaps: Vec<String>,
    pub lobby_save: Option<String>,
    pub lobby_error: Option<String>,
    pub turn: Option<i32>,
    pub year: Option<i32>,
    /// The human's treasury and rates of the world's top bar: gold, gold per turn, science per turn.
    pub top: Option<(i32, i32, i32)>,
    /// "human", "ai", "over".
    pub status: Option<&'static str>,
    /// The blocker's `EndTurnBlockingTypes` value and its target ("unit 3", "city 1", "plot 4,5").
    pub blocker: Option<(i32, String)>,
    /// The end-turn button's text and whether it is a blocker.
    pub end_turn: Option<String>,
    pub end_turn_note: Option<String>,
    pub selected_unit: Option<i32>,
    pub unit_panel: Option<TestUnitPanel>,
    pub city_panel: Option<TestCityPanel>,
    /// The title of the open question.
    pub question: Option<String>,
    /// The lines of the hovered plot's tooltip (empty when none shows).
    pub tooltip: Vec<String>,
    pub units: Vec<TestUnit>,
    pub cities: Vec<TestCity>,
    pub rival_units: usize,
    pub rival_cities: usize,
    pub players: usize,
    /// "T3: Research: Pottery (auto, SIMP-002)", newest last.
    pub journal: Vec<String>,
    /// `World`s received so far.
    pub worlds: u64,
    /// The screens of a server session: sign-in, lobby, room, notice (lobby-ui 4.11).
    pub net: Option<TestNet>,
    /// The turn timer of the top bar: its text, and why it stopped.
    pub timer: Option<(String, Option<String>)>,
    /// The panel of the players of a server game.
    pub players_panel: Option<TestPlayers>,
    /// The chat lines of the journal window, newest last.
    pub chat: Vec<String>,
    /// The minimap panel's source: "image" (the stand-in, SIMP-060) or "texture".
    pub minimap: Option<&'static str>,
    /// The settings window is open.
    pub settings_open: bool,
    /// The renderer's quality: "low", "medium", "high", or "custom" (set by the shell).
    pub quality: Option<&'static str>,
    /// Terrain cells in view and the ones with no page yet (terrain-scroll, AC-R1.5).
    pub terrain_cells: Option<(u32, u32)>,
    /// The unit and city models the render bridge gave the renderer, `[owner, id, x, y]` (set by the shell).
    pub map_units: Vec<[i32; 4]>,
    pub map_cities: Vec<[i32; 4]>,
    /// The game UIs (set by the app when a game UI is registered).
    pub ui: Option<TestUi>,
}

/// A row of the players panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestPlayerRow {
    pub player: i8,
    /// "Rome (Bob)", "Rome (AI)".
    pub name: String,
    pub me: bool,
    pub host: bool,
    pub host_passes_in_s: Option<u32>,
    /// "none", "done", "thinking", "waiting".
    pub turn: &'static str,
    /// "online", "lost:<time>", "ai".
    pub line: String,
    /// "peace", "war-human", "war-offered", "war-incoming", "war-ai"; `None`: no buttons.
    pub diplomacy: Option<&'static str>,
    pub can_kick: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestPlayers {
    pub open: bool,
    pub note: Option<String>,
    pub orders_enabled: bool,
    pub rows: Vec<TestPlayerRow>,
}

/// A room of the lobby list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestRoomRow {
    pub id: u32,
    pub name: String,
    pub host: String,
    pub players: String,
    pub map: String,
    pub starting: bool,
}

/// A seat of a game in progress: `(player, civilization, who, takeable, yours)`.
pub type TestSeat = (i8, String, String, bool, bool);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestGameRow {
    pub id: u32,
    pub name: String,
    pub turn: i32,
    pub status: String,
    pub seats: Vec<TestSeat>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestSaveRow {
    pub id: String,
    pub name: String,
    pub turn: i32,
    pub civs: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestSlot {
    pub index: u8,
    /// "open", "human", "ai", "closed".
    pub who: &'static str,
    pub nick: Option<String>,
    pub ready: bool,
    pub me: bool,
    /// The civilization key; `None` is "Random".
    pub civ: Option<String>,
    pub was: Option<String>,
    pub yours: bool,
}

/// The room window: the settings as the server has them, the slots, the Start rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestRoom {
    pub host: String,
    pub me_host: bool,
    pub starting: bool,
    pub has_slot: bool,
    pub name: String,
    pub world_size: String,
    pub map_script: String,
    pub speed: String,
    pub era: String,
    pub handicap: String,
    pub minor_civs: u8,
    /// "simultaneous" or "hybrid".
    pub turn_mode: &'static str,
    /// "off", "original:<row>", "fixed:<seconds>".
    pub timer: String,
    pub seed: Option<u32>,
    pub map_seed: Option<u32>,
    pub options: Vec<(String, i32)>,
    pub slots: Vec<TestSlot>,
    pub can_start: bool,
    pub why_not: Option<String>,
    pub from_save: Option<String>,
    pub note: Option<String>,
    pub error: Option<String>,
}

/// The server session's screens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TestNet {
    /// The `ClientPhase`: "connecting", "login", "lobby", "room", "starting", "loading", "play", "notice",
    /// "fatal".
    pub phase: &'static str,
    pub nick: String,
    /// The line: the dot's word and whether orders are taken (LU-D19).
    pub conn: String,
    pub conn_status: Option<String>,
    pub orders_enabled: bool,
    pub login_error: Option<String>,
    pub rooms: Vec<TestRoomRow>,
    pub games: Vec<TestGameRow>,
    pub saves: Vec<TestSaveRow>,
    pub lobby_error: Option<String>,
    pub room: Option<TestRoom>,
    /// The text of the notice window.
    pub notice: Option<String>,
    /// The nick of the host of the game by the last seat list (empty outside a game).
    pub host: Option<String>,
    // ---- the game stream (net-client 4.12)
    pub rtt_ms: Option<u64>,
    /// `seq` of the batch with the last view message, the hash of `base` and of the map basis (hex; empty
    /// when there is none) and the revision the frame reads.
    pub view_seq: u64,
    pub view_hash: String,
    pub basis_hash: String,
    pub revision: u64,
    /// Orders sent and not answered finally; records of the optimistic layer.
    pub inflight: usize,
    pub overlay: usize,
    /// The context of the selected unit: "fresh", "stale" or "missing".
    pub ctx: &'static str,
    /// The turn state as shown: the turn, the phase and `(player, finished, may_act)` of each human seat.
    pub turn_state: Option<TestTurnState>,
    /// The seats of the game: `(player, who)`, `who` the nick, "ai" or "closed".
    pub seats: Vec<(i8, String)>,
    /// `(attempt, next_in_ms, probing)` while reconnecting.
    pub reconnect: Option<(u32, u64, bool)>,
    pub stats: ClientStats,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestTurnState {
    pub turn: i32,
    /// "starting", "ai", "humans", "over".
    pub phase: &'static str,
    /// "simultaneous" or "hybrid".
    pub mode: &'static str,
    pub paused: bool,
    pub seats: Vec<(i8, bool, bool)>,
}

/// The snapshot of a server session: the screen, its data, and what the core knows of the game. `selected` is
/// the unit the controls have selected.
pub fn net_snapshot(
    screens: &Screens,
    phase: &ClientPhase,
    core: &ClientCore,
    selected: Option<i32>,
) -> TestSnapshot {
    let screen = if screens.fatal.is_some() {
        "fatal"
    } else if screens.notice.is_some() {
        "notice"
    } else if screens.login.is_some() {
        "login"
    } else if screens.room.is_some() {
        "room"
    } else if screens.lobby_list.is_some() {
        "lobby"
    } else if matches!(phase, ClientPhase::Starting) {
        "starting"
    } else if screens.loading.is_some() {
        "loading"
    } else {
        "play"
    };
    let mut s = TestSnapshot {
        screen,
        selected_unit: selected,
        worlds: core.revision(),
        ..TestSnapshot::default()
    };
    screens_part(&mut s, screens);
    if matches!(phase, ClientPhase::Playing)
        && let Some(w) = core.shown()
    {
        world_part(&mut s, w);
    }
    let host = core
        .seats()
        .iter()
        .find(|x| x.host)
        .and_then(|x| match &x.control {
            protocol::SeatControl::Human { nick } => Some(nick.clone()),
            _ => None,
        });
    let conn = screens.conn.clone().unwrap_or_default();
    let mut net = TestNet {
        phase: crate::net_ui::phase_name(phase),
        nick: core.nick().to_owned(),
        conn: conn.text,
        conn_status: conn.status,
        orders_enabled: conn.orders_enabled,
        login_error: screens.login.as_ref().and_then(|l| l.error.clone()),
        notice: screens.notice.as_ref().map(|n| n.text.clone()),
        host,
        rtt_ms: core.rtt_ms(),
        view_seq: core.view_seq(),
        view_hash: core
            .base_hash()
            .map(|h| format!("{h:016x}"))
            .unwrap_or_default(),
        basis_hash: core
            .basis_hash()
            .map(|h| format!("{h:016x}"))
            .unwrap_or_default(),
        revision: core.revision(),
        inflight: core.orders_in_flight(),
        overlay: core.overlay_len(),
        ctx: selected.map_or("missing", |u| core.contexts().unit(u).name()),
        turn_state: core.turn_state().map(|t| TestTurnState {
            turn: t.turn,
            phase: match t.phase {
                protocol::TurnPhase::Starting => "starting",
                protocol::TurnPhase::Ai { .. } => "ai",
                protocol::TurnPhase::Humans => "humans",
                protocol::TurnPhase::GameOver { .. } => "over",
            },
            mode: match t.mode {
                protocol::TurnMode::Simultaneous => "simultaneous",
                protocol::TurnMode::Hybrid => "hybrid",
            },
            paused: t.paused,
            seats: t
                .seats
                .iter()
                .map(|x| (x.player, x.finished, x.may_act))
                .collect(),
        }),
        seats: core
            .seats()
            .iter()
            .map(|x| {
                let who = match &x.control {
                    protocol::SeatControl::Human { nick } => nick.clone(),
                    protocol::SeatControl::Ai { .. } => "ai".to_owned(),
                    protocol::SeatControl::Closed => "closed".to_owned(),
                };
                (x.player, who)
            })
            .collect(),
        reconnect: core
            .reconnect_status()
            .map(|r| (r.attempt, r.next_in_ms, r.probing)),
        stats: core.stats().clone(),
        ..TestNet::default()
    };
    if let Some(l) = &screens.lobby_list {
        net.rooms = l
            .rooms
            .iter()
            .map(|r| TestRoomRow {
                id: r.id,
                name: r.name.clone(),
                host: r.host.clone(),
                players: r.players.clone(),
                map: r.map.clone(),
                starting: r.starting,
            })
            .collect();
        net.games = l
            .games
            .iter()
            .map(|g| TestGameRow {
                id: g.id,
                name: g.name.clone(),
                turn: g.turn,
                status: g.status.clone(),
                seats: g
                    .seats
                    .iter()
                    .map(|x| (x.player, x.civ.clone(), x.who.clone(), x.takeable, x.yours))
                    .collect(),
            })
            .collect();
        net.saves = l
            .saves
            .iter()
            .map(|x| TestSaveRow {
                id: x.id.clone(),
                name: x.name.clone(),
                turn: x.turn,
                civs: x.civs.clone(),
            })
            .collect();
        net.lobby_error.clone_from(&l.error);
    }
    if let Some(r) = &screens.room {
        let st = &r.settings;
        net.room = Some(TestRoom {
            host: r.host.clone(),
            me_host: r.me_host,
            starting: r.starting,
            has_slot: r.has_slot,
            name: st.name.clone(),
            world_size: st.world_size.clone(),
            map_script: st.map_script.clone(),
            speed: st.speed.clone(),
            era: st.era.clone(),
            handicap: st.handicap.clone(),
            minor_civs: st.minor_civs,
            turn_mode: match st.turn_mode {
                TurnModeChoice::Simultaneous => "simultaneous",
                TurnModeChoice::Hybrid => "hybrid",
            },
            timer: match &st.timer {
                TimerChoice::Off => "off".to_owned(),
                TimerChoice::Original(row) => format!("original:{row}"),
                TimerChoice::Fixed(secs) => format!("fixed:{secs}"),
            },
            seed: st.seed,
            map_seed: st.map_seed,
            options: st
                .options
                .iter()
                .map(|o| (o.key.clone(), o.value))
                .collect(),
            slots: r
                .slots
                .iter()
                .map(|x| {
                    let (who, nick, ready, me) = match &x.who {
                        SlotWho::Open => ("open", None, false, false),
                        SlotWho::Human { nick, ready, me } => {
                            ("human", Some(nick.clone()), *ready, *me)
                        }
                        SlotWho::Ai => ("ai", None, false, false),
                        SlotWho::Closed => ("closed", None, false, false),
                    };
                    TestSlot {
                        index: x.index,
                        who,
                        nick,
                        ready,
                        me,
                        civ: x.civ.clone(),
                        was: x.was.clone(),
                        yours: x.yours,
                    }
                })
                .collect(),
            can_start: r.can_start,
            why_not: r.why_not.clone(),
            from_save: r.from_save.clone(),
            note: r.note.clone(),
            error: r.error.clone(),
        });
        net.lobby_error.clone_from(&r.error);
    }
    s.net = Some(net);
    s
}

/// The `kind` of a slot as the playtest writes it.
pub fn parse_slot_kind(s: &str) -> Option<SlotKind> {
    match s {
        "open" => Some(SlotKind::Open),
        "ai" => Some(SlotKind::Ai),
        "closed" => Some(SlotKind::Closed),
        _ => None,
    }
}

/// What the screens of the frame show (the same for the local game and a server session).
fn screens_part(s: &mut TestSnapshot, screens: &Screens) {
    s.loading = screens.loading.as_ref().map(|l| l.stage.clone());
    s.fatal.clone_from(&screens.fatal);
    s.question = screens.confirm.as_ref().map(|c| c.title.clone());
    s.tooltip = screens
        .tooltip
        .as_ref()
        .map(|t| t.lines.clone())
        .unwrap_or_default();
    if let Some(l) = &screens.lobby {
        s.lobby_civs = l.civs.iter().map(|c| c.key.clone()).collect();
        s.lobby_handicaps = l.handicaps.iter().map(|c| c.key.clone()).collect();
        s.lobby_save.clone_from(&l.save);
        s.lobby_error.clone_from(&l.error);
    }
    if let Some(e) = &screens.end_turn {
        s.end_turn = Some(e.label());
        s.end_turn_note.clone_from(&e.note);
        if let EndTurnState::GameOver(_) = e.state {
            s.status = Some("over");
        }
    }
    if let Some(u) = &screens.unit {
        s.unit_panel = Some(TestUnitPanel {
            name: u.name.clone(),
            actions: u
                .actions
                .iter()
                .map(|a| TestUnitAction {
                    action: a.action,
                    label: a.label.clone(),
                    enabled: a.enabled,
                    reason: a.reason.clone(),
                })
                .collect(),
            loading: u.loading,
            aiming: u.aiming.is_some(),
        });
    }
    if let Some(c) = &screens.city {
        s.city_panel = Some(TestCityPanel {
            name: c.name.clone(),
            producing: c.producing.as_ref().map(|p| p.name.clone()),
            builds: c
                .builds
                .iter()
                .map(|b| {
                    let offer = |o: &interim_ui::screens::city_panel::Offer| TestOffer {
                        cost: o.cost,
                        enabled: o.enabled,
                    };
                    TestBuild {
                        name: b.name.clone(),
                        section: section_name(b.section),
                        turns: b.turns,
                        current: b.current,
                        gold: b.gold.as_ref().map(offer),
                        faith: b.faith.as_ref().map(offer),
                    }
                })
                .collect(),
            loading: c.loading,
            refused: c.refused.clone(),
            gold: c.gold,
            faith: c.faith,
        });
    }
    if let Some(j) = &screens.journal {
        let skip = j.lines.len().saturating_sub(JOURNAL_TAIL);
        s.journal = j.lines[skip..].iter().map(|l| l.text()).collect();
        if let Some(c) = &j.chat {
            let skip = c.lines.len().saturating_sub(JOURNAL_TAIL);
            s.chat = c.lines[skip..].iter().map(|l| l.text()).collect();
        }
    }
    s.timer = screens
        .top_bar
        .as_ref()
        .and_then(|b| b.timer.as_ref())
        .map(|t| (t.text.clone(), t.stopped.clone()));
    s.players_panel = screens.players.as_ref().map(players_part);
    s.minimap = screens.minimap.as_ref().and_then(|m| match m.source {
        interim_ui::screens::minimap::MinimapSource::None => None,
        interim_ui::screens::minimap::MinimapSource::Image(_) => Some("image"),
        interim_ui::screens::minimap::MinimapSource::Texture { .. } => Some("texture"),
    });
    s.settings_open = screens.settings.is_some();
}

fn players_part(p: &interim_ui::screens::players::PlayersView) -> TestPlayers {
    use interim_ui::screens::players::{Diplomacy, PlayerLine, TurnMark};
    TestPlayers {
        open: p.open,
        note: p.note.clone(),
        orders_enabled: p.orders_enabled,
        rows: p
            .rows
            .iter()
            .map(|r| TestPlayerRow {
                player: r.player,
                name: r.name(),
                me: r.me,
                host: r.host,
                host_passes_in_s: r.host_passes_in_s,
                turn: match r.turn {
                    TurnMark::None => "none",
                    TurnMark::Done => "done",
                    TurnMark::Thinking => "thinking",
                    TurnMark::Waiting => "waiting",
                },
                line: match &r.line {
                    PlayerLine::Online => "online".to_owned(),
                    PlayerLine::Lost(t) => format!("lost:{t}"),
                    PlayerLine::Ai => "ai".to_owned(),
                },
                diplomacy: r.diplomacy.map(|d| match d {
                    Diplomacy::Peace => "peace",
                    Diplomacy::WarHuman => "war-human",
                    Diplomacy::WarOffered => "war-offered",
                    Diplomacy::WarIncoming => "war-incoming",
                    Diplomacy::WarAi => "war-ai",
                }),
                can_kick: r.can_kick,
            })
            .collect(),
    }
}

/// The quality name for `TestSnapshot::quality`: the preset whose keys the renderer has, or "custom".
pub fn quality_name(settings: &render::RenderSettings) -> &'static str {
    render::Preset::ALL
        .into_iter()
        .find(|p| render::RenderSettings::preset(*p) == *settings)
        .map_or("custom", render::Preset::name)
}

fn section_name(s: interim_ui::screens::city_panel::BuildSection) -> &'static str {
    use interim_ui::screens::city_panel::BuildSection;
    match s {
        BuildSection::Units => "Units",
        BuildSection::Buildings => "Buildings",
        BuildSection::Wonders => "Wonders",
        BuildSection::Other => "Other",
    }
}

fn world_part(s: &mut TestSnapshot, w: &WorldView) {
    s.turn = Some(w.turn);
    s.year = Some(w.year);
    s.top = Some((
        w.top_bar.gold,
        w.top_bar.gold_per_turn,
        w.top_bar.science_per_turn,
    ));
    s.status = Some(match w.status {
        TurnStatus::HumanTurn => "human",
        TurnStatus::AiTurn { .. } => "ai",
        TurnStatus::GameOver { .. } => "over",
    });
    s.blocker = w.blocker.map(|b| {
        let target = match b.target {
            BlockerTarget::None => String::new(),
            BlockerTarget::Unit(id) => format!("unit {id}"),
            BlockerTarget::City(id) => format!("city {id}"),
            BlockerTarget::Plot { x, y } => format!("plot {x},{y}"),
        };
        (b.kind, target)
    });
    s.players = w.players.len();
    for u in &w.units {
        if u.owner == w.human {
            s.units.push(TestUnit {
                id: u.id,
                name: u.name.clone(),
                x: u.x,
                y: u.y,
                hp: u.hp,
                moves_left: u.moves_left,
                activity: u.activity,
            });
        } else {
            s.rival_units += 1;
        }
    }
    for c in &w.cities {
        if c.owner == w.human {
            s.cities.push(TestCity {
                id: c.id,
                name: c.name.clone(),
                x: c.x,
                y: c.y,
                population: c.population,
                production_item: c.production_item,
                production_turns: c.production_turns,
            });
        } else {
            s.rival_cities += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test world of the fixture host (a dev-dependency: the client itself has no host).
    fn world() -> WorldView {
        crate::test_world::world()
    }

    /// The snapshot of `screens` over `world`, as the server session builds it.
    fn snapshot(screens: &Screens, world: &WorldView) -> TestSnapshot {
        let mut s = TestSnapshot {
            screen: if screens.fatal.is_some() {
                "fatal"
            } else {
                "play"
            },
            fatal: screens.fatal.clone(),
            ..TestSnapshot::default()
        };
        screens_part(&mut s, screens);
        world_part(&mut s, world);
        s
    }

    #[test]
    fn select_and_open_click_the_flag_and_banner() {
        let w = &world();
        let u = w.units.iter().find(|u| u.owner == w.human).unwrap();
        assert_eq!(
            TestAction::SelectUnit(u.id).dispatch(Some(w)),
            Dispatch::Ui(UiAction::LabelClicked {
                target: LabelTarget::Unit(u.id),
                cell: (i32::from(u.x), i32::from(u.y)),
                secondary: false,
            })
        );
        let rival = w.units.iter().find(|u| u.owner != w.human).map(|u| u.id);
        if let Some(id) = rival {
            assert_eq!(TestAction::SelectUnit(id).dispatch(Some(w)), Dispatch::Skip);
        }
        assert_eq!(TestAction::OpenCity(-7).dispatch(Some(w)), Dispatch::Skip);
        assert_eq!(
            TestAction::EndTurn { force: true }.dispatch(None),
            Dispatch::Ui(UiAction::EndTurn { force: true })
        );
        assert_eq!(
            TestAction::BuyBuild {
                index: 2,
                faith: false
            }
            .dispatch(None),
            Dispatch::Ui(UiAction::BuyBuild {
                index: 2,
                faith: false
            })
        );
        assert_eq!(
            TestAction::Reload.dispatch(None),
            Dispatch::Ui(UiAction::Reload)
        );
        assert_eq!(
            TestAction::Fault(Fault::KillHost).dispatch(Some(w)),
            Dispatch::Fault(Fault::KillHost)
        );
    }

    #[test]
    fn the_snapshot_names_the_minimap_the_settings_and_the_players() {
        use interim_ui::screens::minimap::{MinimapSource, MinimapView};
        use interim_ui::screens::players::{PlayerRow, PlayersView};
        use interim_ui::screens::settings::SettingsView;
        let screens = Screens {
            minimap: Some(MinimapView {
                source: MinimapSource::Texture {
                    size: (320, 190),
                    generation: 1,
                },
                camera_frame: None,
            }),
            settings: Some(SettingsView::default()),
            players: Some(PlayersView {
                rows: vec![PlayerRow {
                    civ: "Rome".into(),
                    nick: Some("Bob".into()),
                    host: true,
                    ..PlayerRow::default()
                }],
                open: true,
                ..PlayersView::default()
            }),
            ..Screens::default()
        };
        let s = snapshot(&screens, &world());
        assert_eq!(s.minimap, Some("texture"));
        assert!(s.settings_open);
        let rows = s.players_panel.expect("players").rows;
        assert_eq!(rows[0].name, "Rome (Bob)");
        assert!(rows[0].host);
        assert_eq!(
            quality_name(&render::RenderSettings::preset(render::Preset::Low)),
            "low"
        );
        let mut custom = render::RenderSettings::preset(render::Preset::High);
        custom.set("ShadowLevel", 0).ok();
        assert_eq!(quality_name(&custom), "custom");
    }

    #[test]
    fn snapshot_has_the_world_and_the_screens() {
        let w = world();
        let screens = Screens {
            end_turn: Some(interim_ui::screens::end_turn::EndTurnView {
                state: EndTurnState::Ready,
                note: None,
                no_connection: false,
            }),
            ..Screens::default()
        };
        let s = snapshot(&screens, &w);
        assert_eq!(s.screen, "play");
        assert_eq!(s.turn, Some(w.turn));
        assert_eq!(s.status, Some("human"));
        assert_eq!(s.end_turn.as_deref(), Some("Next turn"));
        assert_eq!(
            s.units.len() + s.rival_units,
            w.units.len(),
            "every unit counted once"
        );
        let fatal = Screens {
            fatal: Some("boom".into()),
            ..Screens::default()
        };
        assert_eq!(snapshot(&fatal, &w).screen, "fatal");
    }
}
