//! The screens of a local game (play-ui §3.3): which one is up and the view data it shows, built
//! from the session; the screens themselves are in `interim-ui`. Lobby -> loading -> play (top
//! bar, end-turn button, journal, city panel, unit panel, flags and banners) -> error.
//!
//! The server session has its own router: [`net_screen`] (lobby-ui 4.1, one row per `ClientPhase`); the view
//! data of its screens is in `net_ui`.

use client_core::{ChatEntry, ClientEvent, ClientPhase, NoticeKind};
use interim_ui::labels::MapLabelsView;
use interim_ui::screens::chat::{ChatRow, ChatTarget, ChatView};
use interim_ui::screens::confirm::ConfirmView;
use interim_ui::screens::defeated::DefeatedView;
use interim_ui::screens::end_turn::{EndTurnState, EndTurnView};
use interim_ui::screens::journal::{JournalLine, JournalView, LineKind, LogTab};
use interim_ui::screens::lobby::{Choice, LobbyView};
use interim_ui::screens::players::{Diplomacy, PlayerLine, PlayerRow, PlayersView, TurnMark};
use interim_ui::screens::tooltip::TooltipView;
use interim_ui::screens::top_bar::{TimerView, TopBarView, timer_text};
use interim_ui::screens::unit_panel::UnitPanelView;
use interim_ui::screens::{Forms, Screens, UiAction};
use protocol::{
    Catalog, ChatTo, Conn, GameCtlReason, JournalKind, LobbyInfo, PlayerFlags, PlayerId,
    RejectReason, SeatControl, SeatView, Setup, TurnMode, TurnPhase, TurnState, TurnStatus,
    WorldView,
};

use crate::production::ProductionChooser;

/// The journal shows at most this many of the newest entries.
const JOURNAL_LINES: usize = 300;
/// How long the note over the players panel stays (a refused host command, a new host; LU-D18, LU-D21).
const PLAYERS_NOTE_MS: u64 = 6_000;
/// The refusal of an order, over the end-turn button and in the city window (4.7).
const REFUSAL_NOTE_MS: u64 = 6_000;
/// "End turn cancelled by your order..." (LU-D20).
const CANCEL_NOTE_MS: u64 = 3_000;
/// The host that is away loses the role after this many seconds (LU-D8, `host_lost_s` of the server).
const HOST_LOST_S: u64 = 60;
const CANCEL_TEXT: &str = "End turn cancelled by your order. Press Next turn when you are done.";

/// Keyboard help (the top bar's "?").
pub const KEY_HELP: &str = "Left click: select a unit or open a city\n\
Right click: move / attack (a civilization at peace asks first)\n\
Enter: next turn, or go to what blocks it\n\
Shift+Enter (Shift+click the button): end the turn anyway\n\
W: next unit   C: centre   G: grid\n\
F: fortify   S: sleep   Space: skip   H: heal   A: automate   E: explore\n\
B: found city, or ranged attack (then click a red plot)   Backspace: cancel orders\n\
T: chat\n\
Esc: cancel / close";

/// Which screen a server session shows (lobby-ui 4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetScreen {
    /// "Sign in": nick and password, or the text of a refusal.
    Login,
    /// "Connecting to the server...", "Starting the game...", "Loading the world...": `LoadingView`.
    Loading,
    Lobby,
    Room,
    /// The game panels (N4.11); they stay while the line is down (LU-D19).
    Play,
    /// A window over the lobby or the map: kicked, replaced, stopped, over.
    Notice(NoticeKind),
    Fatal(String),
}

/// The screen for the phase (lobby-ui 4.1). `rejected` is the core's refusal: `VersionMismatch` and `Full` are
/// `Fatal` phases in the core but are shown in the sign-in window with their button (4.2).
pub fn net_screen(phase: &ClientPhase, rejected: Option<&RejectReason>) -> NetScreen {
    match phase {
        ClientPhase::Connecting | ClientPhase::Starting | ClientPhase::Loading => {
            NetScreen::Loading
        }
        ClientPhase::NeedLogin => NetScreen::Login,
        ClientPhase::Lobby => NetScreen::Lobby,
        ClientPhase::Room => NetScreen::Room,
        ClientPhase::Playing => NetScreen::Play,
        ClientPhase::Notice(kind) => NetScreen::Notice(kind.clone()),
        ClientPhase::Fatal(text) => match rejected {
            Some(RejectReason::VersionMismatch { .. } | RejectReason::Full) => NetScreen::Login,
            _ => NetScreen::Fatal(text.clone()),
        },
    }
}

/// Screen state kept between frames.
#[derive(Debug)]
pub struct Hud {
    forms: Forms,
    journal_open: bool,
    /// The tab of the journal window.
    tab: LogTab,
    players_open: bool,
    /// The settings window is open (the top bar's [Settings]).
    settings_open: bool,
    /// Lines of the chat the player has seen (the tab was open); the rest are "new".
    chat_seen: usize,
    /// The note over the players panel and until when it shows (ms).
    players_note: Option<(String, u64)>,
    /// The core's refusal of the last order and until when it shows: over the button and in the city window.
    refusal: Option<(String, u64)>,
    /// The note under the button for an order that took the end of turn back.
    cancel_note: Option<(String, u64)>,
    /// `Watch` was chosen in the defeat window: it stays shut while the defeat lasts.
    defeat_watch: bool,
}

impl Default for Hud {
    fn default() -> Self {
        Self::new()
    }
}

/// What a frame of the game screens of a server session needs.
pub struct NetPlay<'a> {
    /// The view the controls take: `base` plus the layer.
    pub world: &'a WorldView,
    pub catalog: Option<&'a Catalog>,
    pub journal: &'a [protocol::JournalEntry],
    /// Our own end of turn is set, as shown (the layer included).
    pub finished: bool,
    /// The line is up (NC-D5): without it the order screens are grey (LU-D19).
    pub orders_enabled: bool,
    /// The turn state as shown (our own `finished` has the layer on it), the seats and the chat.
    pub turn: Option<&'a TurnState>,
    pub seats: &'a [SeatView],
    pub chat: &'a [ChatEntry],
    /// Our seat and our nick.
    pub me: Option<PlayerId>,
    pub nick: &'a str,
    /// The client's clock and the gateway's (ms), and the time left on the timer of our group.
    pub now_ms: u64,
    pub server_ms: u64,
    pub timer_left_ms: Option<u64>,
    pub production: &'a ProductionChooser,
    pub fps: f32,
    pub unit: Option<UnitPanelView>,
    pub labels: Option<MapLabelsView>,
    pub confirm: Option<ConfirmView>,
    pub tooltip: Option<TooltipView>,
}

impl Hud {
    pub fn new() -> Self {
        Self {
            forms: Forms::default(),
            journal_open: true,
            tab: LogTab::Journal,
            players_open: false,
            settings_open: false,
            chat_seen: 0,
            players_note: None,
            refusal: None,
            cancel_note: None,
            defeat_watch: false,
        }
    }

    /// What the player typed and picked on the screens, for `interim_ui::screens::show`.
    pub fn forms(&mut self) -> &mut Forms {
        &mut self.forms
    }

    /// The game screens of a server session: the same panels as the local game, read off the view the
    /// controls take (`base` plus the layer) and the core's journal (net-client 4.6), and the panels of
    /// lobby-ui 4.7: the end-turn states, the timer, the line, the players and the chat.
    pub fn net_play(&mut self, f: &NetPlay<'_>, out: &mut Screens) {
        let world = f.world;
        let now = f.now_ms;
        let mut bar = top_bar_view(world, f.catalog, f.fps);
        bar.timer = timer_view(f.turn, f.seats, f.me, f.timer_left_ms, f.orders_enabled);
        bar.line = out.conn.as_ref().map(|c| (c.dot, c.text.clone()));
        out.top_bar = Some(bar);
        let defeated = is_defeated(world, f.turn);
        if !defeated {
            self.defeat_watch = false;
        } else if !self.defeat_watch {
            out.defeated = Some(DefeatedView {
                title: "Defeated".to_owned(),
                text: DEFEATED_TEXT.to_owned(),
            });
        }
        let refusal = live(&self.refusal, now);
        out.city = f.production.view(world, f.catalog).map(|mut c| {
            c.no_connection = !f.orders_enabled || defeated;
            c.refused = refusal.clone();
            c
        });
        // The chat tab: what is new counts while another tab (or a folded window) is shown.
        let on_chat = self.journal_open && self.tab == LogTab::Chat;
        if on_chat || self.chat_seen > f.chat.len() {
            self.chat_seen = f.chat.len();
        }
        let unread = f.chat[self.chat_seen..]
            .iter()
            .filter(|e| !is_mine(e, f.me, f.nick))
            .count();
        out.journal = Some(JournalView {
            lines: journal_lines(f.journal),
            open: self.journal_open,
            tab: self.tab,
            chat: Some(chat_view(
                f.chat,
                f.seats,
                world,
                f.me,
                now,
                unread,
                f.orders_enabled,
            )),
            above_minimap: false,
        });
        out.players = Some(PlayersView {
            rows: player_rows(world, f.turn, f.seats, f.me, f.server_ms),
            open: self.players_open,
            note: live(&self.players_note, now),
            orders_enabled: f.orders_enabled,
        });
        let note = live(&self.cancel_note, now).or(refusal);
        out.end_turn = Some(net_end_turn_view(
            world,
            f.turn,
            f.seats,
            f.me,
            note,
            f.orders_enabled,
        ));
        out.unit.clone_from(&f.unit);
        out.labels.clone_from(&f.labels);
        out.confirm.clone_from(&f.confirm);
        out.tooltip.clone_from(&f.tooltip);
    }

    /// What the core tells the game screens: notes over the end-turn button and the players panel.
    pub fn on_game_event(&mut self, ev: &ClientEvent, now_ms: u64) {
        match ev {
            ClientEvent::GameCtlRejected { reason, .. } => {
                self.players_note = Some((
                    ctl_refusal_text(*reason).to_owned(),
                    now_ms + PLAYERS_NOTE_MS,
                ));
            }
            ClientEvent::HostChanged { to, me, .. } => {
                let text = if *me {
                    "You are the host now: the previous host was away for 60 s.".to_owned()
                } else {
                    format!("{to} is the host now.")
                };
                self.players_note = Some((text, now_ms + PLAYERS_NOTE_MS));
            }
            ClientEvent::EndTurnCancelled { by_order: true } => {
                self.cancel_note = Some((CANCEL_TEXT.to_owned(), now_ms + CANCEL_NOTE_MS));
            }
            ClientEvent::OrderRolledBack { text, .. } => {
                self.refusal = Some((text.clone(), now_ms + REFUSAL_NOTE_MS));
            }
            // Another game: the old notes and the chat count are not its own.
            ClientEvent::Attached { .. } => {
                self.chat_seen = 0;
                self.players_note = None;
                self.refusal = None;
                self.cancel_note = None;
            }
            _ => {}
        }
    }

    /// The settings window is open: the shell adds its view to the frame's screens.
    pub fn settings_open(&self) -> bool {
        self.settings_open
    }

    /// `T`: the chat tab opens and its input takes the focus (LU-D9).
    pub fn focus_chat(&mut self) {
        self.tab = LogTab::Chat;
        self.journal_open = true;
        self.forms.chat.focus = true;
    }

    /// Handles what only the screens care about (the folds and the tab) and returns the rest.
    pub fn take_actions(&mut self, actions: Vec<UiAction>) -> Vec<UiAction> {
        actions
            .into_iter()
            .filter(|a| match a {
                UiAction::ToggleJournal => {
                    self.journal_open = !self.journal_open;
                    false
                }
                UiAction::LogTab(tab) => {
                    self.tab = *tab;
                    self.journal_open = true;
                    false
                }
                UiAction::TogglePlayers => {
                    self.players_open = !self.players_open;
                    false
                }
                UiAction::ToggleSettings => {
                    self.settings_open = !self.settings_open;
                    false
                }
                // `Watch`: the window stays shut, the map stays, the orders are off (`order_block`).
                UiAction::DefeatWatch => {
                    self.defeat_watch = true;
                    false
                }
                // The question "Leave game?" comes up over the map, not over the window.
                UiAction::LeaveGame => {
                    self.settings_open = false;
                    true
                }
                _ => true,
            })
            .collect()
    }

    /// The game was left: what belongs to it goes (the defeat watch, the notes, the chat count, the open windows); the
    /// forms the player typed in stay.
    pub fn game_left(&mut self) {
        self.journal_open = true;
        self.tab = LogTab::Journal;
        self.players_open = false;
        self.settings_open = false;
        self.chat_seen = 0;
        self.players_note = None;
        self.refusal = None;
        self.cancel_note = None;
        self.defeat_watch = false;
    }

    /// A new game from scratch (Reload without a page reload).
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}
/// The lobby of the host's `Lobby`.
pub fn lobby_view(lobby: Option<&LobbyInfo>, error: Option<&str>) -> LobbyView {
    let choices = |l: &[protocol::LobbyChoice]| {
        l.iter()
            .map(|c| Choice {
                key: c.type_name.clone(),
                name: c.name.clone(),
            })
            .collect()
    };
    let setup = Setup::default();
    LobbyView {
        civs: lobby.map(|l| choices(&l.civs)).unwrap_or_default(),
        // HANDICAP_AI is the AI players' own level, not one a human picks.
        handicaps: lobby
            .map(|l| {
                let mut h: Vec<Choice> = choices(&l.handicaps);
                h.retain(|c| c.key != "HANDICAP_AI");
                h
            })
            .unwrap_or_default(),
        map: format!(
            "{} ({})",
            map_name(&setup.map_script),
            size_name(&setup.world_size)
        ),
        save: lobby.and_then(|l| l.save.as_ref()).map(save_label),
        error: error.map(str::to_owned),
    }
}

/// "Rome, turn 12, 2026-10-21 14:03 UTC": what the lobby's Continue button names.
pub fn save_label(s: &protocol::SaveMeta) -> String {
    let civ = if s.civ_name.is_empty() {
        "Autosave"
    } else {
        &s.civ_name
    };
    if s.saved_at_ms == 0 {
        return format!("{civ}, turn {}", s.turn);
    }
    format!("{civ}, turn {}, {}", s.turn, utc_text(s.saved_at_ms))
}

/// Unix ms as "YYYY-MM-DD hh:mm UTC".
// SIMPLIFIED(SIMP-028): the save time in UTC; original: the local date and time of the computer.
pub(crate) fn utc_text(ms: u64) -> String {
    let secs = ms / 1000;
    let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
    // Civil date from days since 1970-01-01 (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rest / 3600,
        rest % 3600 / 60
    )
}

/// "Assets/Maps/Lekmap v5.2/LekmapPangaeaFractalv5.2.lua" -> "LekmapPangaeaFractalv5.2".
pub(crate) fn map_name(script: &str) -> &str {
    let file = script.rsplit(['/', '\\']).next().unwrap_or(script);
    file.strip_suffix(".lua").unwrap_or(file)
}

/// "WORLDSIZE_DUEL" -> "Duel".
pub(crate) fn size_name(size: &str) -> String {
    let s = size
        .strip_prefix("WORLDSIZE_")
        .unwrap_or(size)
        .to_lowercase();
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

fn catalog_name(table: Option<&Vec<protocol::CatalogEntry>>, id: i16) -> Option<String> {
    table?.iter().find(|e| e.id == id).map(|e| e.name.clone())
}

pub fn top_bar_view(world: &WorldView, catalog: Option<&Catalog>, fps: f32) -> TopBarView {
    let t = &world.top_bar;
    let research = (t.research_tech >= 0).then(|| {
        let name = catalog_name(catalog.map(|c| &c.techs), t.research_tech)
            .unwrap_or_else(|| format!("tech {}", t.research_tech));
        (name, t.research_turns)
    });
    TopBarView {
        turn: world.turn,
        year: world.year,
        civ: player_civ(world, world.human).unwrap_or_default(),
        gold: t.gold,
        gold_per_turn: t.gold_per_turn,
        science_per_turn: t.science_per_turn,
        research,
        research_progress: t.research_progress,
        research_cost: t.research_cost,
        culture: t.culture,
        culture_per_turn: t.culture_per_turn,
        next_policy_cost: t.next_policy_cost,
        happiness: t.happiness,
        golden_age_turns: t.golden_age_turns,
        golden_age_progress: t.golden_age_progress,
        golden_age_threshold: t.golden_age_threshold,
        faith: t.faith,
        faith_per_turn: t.faith_per_turn,
        era: (t.era >= 0)
            .then(|| catalog_name(catalog.map(|c| &c.eras), t.era))
            .flatten(),
        fps,
        help: KEY_HELP.to_owned(),
        timer: None,
        line: None,
    }
}

fn player_civ(world: &WorldView, id: PlayerId) -> Option<String> {
    world
        .players
        .iter()
        .find(|p| p.id == id)
        .map(|p| p.civ_name.clone())
        .filter(|n| !n.is_empty())
}

/// The end-turn button for the world: the blocker in the human's turn, the moving rival during
/// the AI turn (from `TurnProgress`, else the status).
pub fn end_turn_view(
    world: &WorldView,
    ai_active: Option<PlayerId>,
    note: Option<&str>,
) -> EndTurnView {
    end_turn_view_on(world, ai_active, note, false)
}

/// What the end-turn button and the grey order buttons say once we are out of the game, and the defeat window.
const DEFEATED_LINE: &str = "You were defeated";
const DEFEATED_TEXT: &str = "You were defeated. The game goes on without you: you can watch the map (no orders) or leave to the lobby.";

/// We are eliminated while the game goes on: the world we are shown is over, the turn state is not (the server
/// gives `GameOver` to the view of an eliminated human only; when the game is over for everybody the turn phase
/// says so too, and the `GameOver` notice follows).
pub fn is_defeated(world: &WorldView, turn: Option<&TurnState>) -> bool {
    matches!(world.status, TurnStatus::GameOver { .. })
        && turn.is_some_and(|t| !matches!(t.phase, TurnPhase::GameOver { .. }))
}

/// The end-turn button of a server session (lobby-ui 4.7): the blocker the layer has not settled, and the
/// states the turn state adds. Our own end of turn set (`finished`, the layer included) waits for the humans
/// that are not done; the hybrid turn of another player; the start of the turn.
pub fn net_end_turn_view(
    world: &WorldView,
    turn: Option<&TurnState>,
    seats: &[SeatView],
    me: Option<PlayerId>,
    note: Option<String>,
    orders_enabled: bool,
) -> EndTurnView {
    let mut v = end_turn_view_on(world, None, note.as_deref(), true);
    v.no_connection = !orders_enabled;
    let Some(ts) = turn else {
        return v;
    };
    if is_defeated(world, turn) {
        v.state = EndTurnState::GameOver(DEFEATED_LINE.to_owned());
        return v;
    }
    if matches!(v.state, EndTurnState::GameOver(_)) {
        return v;
    }
    match ts.phase {
        TurnPhase::Starting => v.state = EndTurnState::Starting,
        TurnPhase::Ai { active } => v.state = EndTurnState::Waiting(player_civ(world, active)),
        TurnPhase::GameOver { .. } => {}
        TurnPhase::Humans => {
            let mine = me.and_then(|m| ts.seats.iter().find(|s| s.player == m));
            match mine {
                Some(seat) if seat.finished => {
                    v.state = EndTurnState::WaitingHumans {
                        names: ts
                            .seats
                            .iter()
                            .filter(|s| !s.finished)
                            .map(|s| seat_name(world, seats, s.player))
                            .collect(),
                    };
                }
                Some(seat) if ts.mode == TurnMode::Hybrid && !seat.may_act => {
                    let who = ts
                        .seats
                        .iter()
                        .find(|s| s.may_act && !s.finished)
                        .map_or_else(
                            || "another player".to_owned(),
                            |s| seat_name(world, seats, s.player),
                        );
                    v.state = EndTurnState::WaitingTurn { who };
                }
                _ => {}
            }
        }
    }
    v
}

/// Why the turn takes no orders from us now (the unit panel's grey buttons, 4.10): the AI or the start of the
/// turn ("Rivals are moving"), or another player's turn in hybrid mode ("Waiting for Bob"). `None`: we may act.
/// Our own end of turn does not block (LU-D20).
pub fn order_block(
    world: &WorldView,
    turn: Option<&TurnState>,
    seats: &[SeatView],
    me: Option<PlayerId>,
) -> Option<String> {
    let ts = turn?;
    if is_defeated(world, turn) {
        return Some(DEFEATED_LINE.to_owned());
    }
    match ts.phase {
        TurnPhase::Starting | TurnPhase::Ai { .. } => {
            Some(crate::unit_panel::RIVALS_MOVING.to_owned())
        }
        TurnPhase::GameOver { .. } => None,
        TurnPhase::Humans => {
            let mine = me.and_then(|m| ts.seats.iter().find(|s| s.player == m))?;
            if ts.mode == TurnMode::Hybrid && !mine.may_act {
                let who = ts
                    .seats
                    .iter()
                    .find(|s| s.may_act && !s.finished)
                    .map_or_else(
                        || "another player".to_owned(),
                        |s| seat_name(world, seats, s.player),
                    );
                Some(format!("Waiting for {who}"))
            } else {
                None
            }
        }
    }
}

/// The note of `slot` while it lives (the number is the time it ends).
fn live(slot: &Option<(String, u64)>, now_ms: u64) -> Option<String> {
    slot.as_ref()
        .filter(|(_, until)| now_ms < *until)
        .map(|(t, _)| t.clone())
}

/// The nick of the human at `player`, else the civilization's name.
fn seat_name(world: &WorldView, seats: &[SeatView], player: PlayerId) -> String {
    let nick = seats
        .iter()
        .find(|s| s.player == player)
        .and_then(|s| match &s.control {
            SeatControl::Human { nick } => Some(nick.clone()),
            _ => None,
        });
    nick.or_else(|| player_civ(world, player))
        .unwrap_or_else(|| format!("player {player}"))
}

fn seat_nick(seats: &[SeatView], player: PlayerId) -> Option<&str> {
    seats
        .iter()
        .find(|s| s.player == player)
        .and_then(|s| match &s.control {
            SeatControl::Human { nick } => Some(nick.as_str()),
            _ => None,
        })
}

/// The words of a refused `GameCtl` (LU-D18).
pub fn ctl_refusal_text(reason: GameCtlReason) -> &'static str {
    match reason {
        GameCtlReason::NotHost => "Only the host can do this",
        GameCtlReason::NoSuchPlayer => "No such player",
        GameCtlReason::NotHuman => "This civilization is not played by a human",
        GameCtlReason::SelfKick => "You cannot remove yourself",
        GameCtlReason::LastHuman => "This is the last human player",
        GameCtlReason::NoGame => "The game is not running",
    }
}

/// The timer of our seat's group (4.7): the time, red under 10 s, stopped by the host's pause or by a
/// human that dropped out, and the host's button.
pub fn timer_view(
    turn: Option<&TurnState>,
    seats: &[SeatView],
    me: Option<PlayerId>,
    left_ms: Option<u64>,
    orders_enabled: bool,
) -> Option<TimerView> {
    let (ts, me, left) = (turn?, me?, left_ms?);
    if ts.phase != TurnPhase::Humans {
        return None;
    }
    let group = ts.seats.iter().find(|s| s.player == me)?.group;
    let timer = ts.timers.iter().find(|t| t.group == group)?;
    let stopped = if ts.paused {
        Some("stopped (paused by host)".to_owned())
    } else if timer.left_ms_when_stopped.is_some() {
        let who = seats
            .iter()
            .filter(|s| s.player != me && !matches!(s.conn, Conn::Online))
            .find_map(|s| match &s.control {
                SeatControl::Human { nick } => Some(nick.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "a player".to_owned());
        Some(format!("stopped (waiting for {who})"))
    } else {
        None
    };
    Some(TimerView {
        text: timer_text(left),
        red: left < 10_000,
        stopped,
        can_pause: seats.iter().any(|s| s.player == me && s.host),
        paused: ts.paused,
        orders_enabled,
    })
}

/// "1:05" of the time a seat has been lost.
fn lost_text(server_ms: u64, since_ms: u64) -> String {
    let s = server_ms.saturating_sub(since_ms) / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The rows of the players panel: a major civilization each (4.7 "Панель игроков").
pub fn player_rows(
    world: &WorldView,
    turn: Option<&TurnState>,
    seats: &[SeatView],
    me: Option<PlayerId>,
    server_ms: u64,
) -> Vec<PlayerRow> {
    let i_am_host = me.is_some_and(|m| seats.iter().any(|s| s.player == m && s.host));
    world
        .players
        .iter()
        .filter(|p| {
            p.flags.contains(PlayerFlags::ALIVE)
                && !p.flags.contains(PlayerFlags::MINOR)
                && !p.flags.contains(PlayerFlags::BARBARIAN)
        })
        .map(|p| {
            let seat = seats.iter().find(|s| s.player == p.id);
            let nick = seat.and_then(|s| match &s.control {
                SeatControl::Human { nick } => Some(nick.clone()),
                _ => None,
            });
            let human = nick.is_some();
            let is_me = Some(p.id) == me;
            let host = seat.is_some_and(|s| s.host);
            let line = match seat {
                Some(s) if human => match s.conn {
                    Conn::Online => PlayerLine::Online,
                    Conn::Lost { since_ms } => PlayerLine::Lost(lost_text(server_ms, since_ms)),
                    Conn::None => PlayerLine::Lost(lost_text(server_ms, server_ms)),
                },
                _ => PlayerLine::Ai,
            };
            let host_passes_in_s = match (seat, host) {
                (Some(s), true) => match s.conn {
                    Conn::Lost { since_ms } => {
                        let away = server_ms.saturating_sub(since_ms) / 1000;
                        Some(HOST_LOST_S.saturating_sub(away) as u32).filter(|s| *s > 0)
                    }
                    _ => None,
                },
                _ => None,
            };
            let mark = turn
                .filter(|t| t.phase == TurnPhase::Humans)
                .and_then(|t| t.seats.iter().find(|s| s.player == p.id).map(|s| (t, s)))
                .map_or(TurnMark::None, |(t, s)| {
                    if s.finished {
                        TurnMark::Done
                    } else if t.mode == TurnMode::Hybrid && !s.may_act {
                        TurnMark::Waiting
                    } else {
                        TurnMark::Thinking
                    }
                });
            let diplomacy = (!is_me && p.flags.contains(PlayerFlags::MET)).then(|| {
                if !p.flags.contains(PlayerFlags::AT_WAR_WITH_VIEWER) {
                    Diplomacy::Peace
                } else if !human {
                    Diplomacy::WarAi
                } else if world
                    .peace_offers
                    .iter()
                    .any(|o| Some(o.from) == me && o.to == p.id)
                {
                    Diplomacy::WarOffered
                } else if world
                    .peace_offers
                    .iter()
                    .any(|o| o.from == p.id && Some(o.to) == me)
                {
                    Diplomacy::WarIncoming
                } else {
                    Diplomacy::WarHuman
                }
            });
            let c = p.color_primary;
            PlayerRow {
                player: p.id,
                team: p.team,
                color: [(c >> 24) as u8, (c >> 16) as u8, (c >> 8) as u8],
                civ: p.civ_name.clone(),
                nick,
                me: is_me,
                host,
                host_passes_in_s,
                turn: mark,
                line,
                diplomacy,
                can_kick: i_am_host && human && !is_me,
            }
        })
        .collect()
}

/// Whether the line was written by us.
fn is_mine(e: &ChatEntry, me: Option<PlayerId>, nick: &str) -> bool {
    (me.is_some() && e.line.from_player == me) || e.line.from == nick
}

/// The chat tab: the lines with their age by our clock (LU-D17) and the humans we can write to.
#[allow(clippy::too_many_arguments)]
pub fn chat_view(
    lines: &[ChatEntry],
    seats: &[SeatView],
    world: &WorldView,
    me: Option<PlayerId>,
    now_ms: u64,
    unread: usize,
    orders_enabled: bool,
) -> ChatView {
    let rows = lines
        .iter()
        .map(|e| ChatRow {
            age_ms: now_ms.saturating_sub(e.received_ms),
            from: e.line.from.clone(),
            private_to: match e.line.to {
                ChatTo::All => None,
                ChatTo::Player(p) if Some(p) == me => Some("you".to_owned()),
                ChatTo::Player(p) => Some(
                    seat_nick(seats, p)
                        .map(str::to_owned)
                        .unwrap_or_else(|| seat_name(world, seats, p)),
                ),
            },
            text: e.line.text.clone(),
        })
        .collect();
    let targets = seats
        .iter()
        .filter(|s| Some(s.player) != me)
        .filter_map(|s| match &s.control {
            SeatControl::Human { nick } if !nick.is_empty() => Some(ChatTarget {
                player: s.player,
                nick: nick.clone(),
            }),
            _ => None,
        })
        .collect();
    ChatView {
        lines: rows,
        targets,
        unread,
        orders_enabled,
    }
}

fn end_turn_view_on(
    world: &WorldView,
    ai_active: Option<PlayerId>,
    note: Option<&str>,
    layered: bool,
) -> EndTurnView {
    let state = match world.status {
        TurnStatus::HumanTurn => match crate::play::blocker_of(world, layered) {
            Some(b) => EndTurnState::Blocked(b.kind),
            None => EndTurnState::Ready,
        },
        TurnStatus::AiTurn { active } => {
            EndTurnState::Waiting(player_civ(world, ai_active.unwrap_or(active)))
        }
        TurnStatus::GameOver { winner } => {
            let who = winner.and_then(|team| {
                world
                    .players
                    .iter()
                    .find(|p| p.team == team)
                    .map(|p| p.civ_name.clone())
            });
            EndTurnState::GameOver(match who {
                Some(civ) => format!("Game over: {civ} wins"),
                None => "Game over".to_owned(),
            })
        }
    };
    EndTurnView {
        state,
        note: note.map(str::to_owned),
        no_connection: false,
    }
}

pub fn journal_lines(entries: &[protocol::JournalEntry]) -> Vec<JournalLine> {
    let skip = entries.len().saturating_sub(JOURNAL_LINES);
    entries[skip..]
        .iter()
        .map(|e| JournalLine {
            turn: e.turn,
            text: plain_text(&e.text),
            simplification: e.simplification,
            at: e.at,
            kind: match e.kind {
                JournalKind::System => LineKind::System,
                JournalKind::Session => LineKind::Session,
                JournalKind::Diplomacy => LineKind::Diplomacy,
                _ => LineKind::Normal,
            },
        })
        .collect()
}

/// The plot the camera moves to for a UI action (a click on a journal line with a place).
pub fn camera_target(action: &UiAction) -> Option<(i16, i16)> {
    match action {
        UiAction::JournalGoto { x, y } => Some((*x, *y)),
        _ => None,
    }
}

/// The core's text without the game's markup tags (`[ICON_FOOD]`, `[COLOR_POSITIVE_TEXT]`,
/// `[ENDCOLOR]`, `[NEWLINE]`).
///
/// SIMPLIFIED(SIMP-001): icon and colour tags are dropped (a newline becomes a space); original:
/// the Lua UI draws them as font icons and colours.
pub fn plain_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let Some(len) = rest[open..].find(']') else {
            break;
        };
        let tag = &rest[open + 1..open + len];
        let markup = tag.starts_with("ICON_")
            || tag.starts_with("COLOR_")
            || tag == "ENDCOLOR"
            || tag == "NEWLINE"
            || tag == "TAB";
        out.push_str(&rest[..open]);
        if !markup {
            out.push_str(&rest[open..=open + len]);
        } else if tag == "NEWLINE" || tag == "TAB" {
            out.push(' ');
        }
        rest = &rest[open + len + 1..];
    }
    out.push_str(rest);
    // Dropped icons leave double spaces ("2 [ICON_CITIZEN] Citizens").
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{Blocker, BlockerTarget, JournalEntry, JournalKind};

    /// The test world of the fixture host (a dev-dependency: the client itself has no host).
    fn fixture_world() -> WorldView {
        crate::test_world::world()
    }

    #[test]
    fn leaving_a_game_clears_its_notes_and_windows() {
        let mut hud = Hud::new();
        hud.defeat_watch = true;
        hud.chat_seen = 12;
        hud.players_open = true;
        hud.settings_open = true;
        hud.players_note = Some(("x".into(), 9));
        hud.refusal = Some(("y".into(), 9));
        hud.cancel_note = Some(("z".into(), 9));
        hud.game_left();
        assert!(!hud.defeat_watch && !hud.players_open && !hud.settings_open);
        assert_eq!(hud.chat_seen, 0);
        assert!(hud.players_note.is_none() && hud.refusal.is_none() && hud.cancel_note.is_none());
    }

    #[test]
    fn every_phase_of_the_server_session_has_its_screen() {
        use NetScreen as S;
        let screen = |p: ClientPhase| net_screen(&p, None);
        assert_eq!(screen(ClientPhase::Connecting), S::Loading);
        assert_eq!(screen(ClientPhase::NeedLogin), S::Login);
        assert_eq!(screen(ClientPhase::Lobby), S::Lobby);
        assert_eq!(screen(ClientPhase::Room), S::Room);
        assert_eq!(screen(ClientPhase::Starting), S::Loading);
        assert_eq!(screen(ClientPhase::Loading), S::Loading);
        assert_eq!(screen(ClientPhase::Playing), S::Play);
        for kind in [
            NoticeKind::Kicked,
            NoticeKind::Replaced,
            NoticeKind::GameStopped { why: "x".into() },
            NoticeKind::GameOver,
        ] {
            assert_eq!(screen(ClientPhase::Notice(kind.clone())), S::Notice(kind));
        }
        assert_eq!(
            screen(ClientPhase::Fatal("boom".into())),
            S::Fatal("boom".into())
        );
        // The core makes these two fatal; the sign-in window shows them with their button.
        for r in [
            RejectReason::VersionMismatch { server: 4 },
            RejectReason::Full,
        ] {
            assert_eq!(
                net_screen(&ClientPhase::Fatal("x".into()), Some(&r)),
                S::Login
            );
        }
        assert_eq!(
            net_screen(
                &ClientPhase::Fatal("x".into()),
                Some(&RejectReason::Unsupported { kind: "k".into() })
            ),
            S::Fatal("x".into())
        );
    }

    #[test]
    fn the_save_is_named_with_its_date() {
        let mut s = protocol::SaveMeta {
            seq: 4,
            build_id: "b".into(),
            turn: 12,
            civ_name: "Rome".into(),
            saved_at_ms: 1_760_000_000_000,
        };
        assert_eq!(save_label(&s), "Rome, turn 12, 2025-10-09 08:53 UTC");
        s.saved_at_ms = 951_782_400_000; // 2000-02-29 00:00
        assert_eq!(save_label(&s), "Rome, turn 12, 2000-02-29 00:00 UTC");
        s.saved_at_ms = 0;
        s.civ_name.clear();
        assert_eq!(save_label(&s), "Autosave, turn 12");
    }

    #[test]
    fn top_bar_and_end_turn_from_the_world() {
        let mut w = fixture_world();
        let bar = top_bar_view(&w, Some(&crate::test_world::catalog()), 60.0);
        assert_eq!((bar.turn, bar.gold), (w.turn, w.top_bar.gold));
        assert!(!bar.civ.is_empty());

        w.blocker = Some(Blocker {
            kind: 8,
            target: BlockerTarget::Unit(4),
        });
        assert_eq!(end_turn_view(&w, None, None).label(), "Unit needs orders");
        w.blocker = None;
        assert_eq!(
            end_turn_view(&w, None, Some("Refused: x")).label(),
            "Next turn"
        );
        w.status = TurnStatus::AiTurn { active: -1 };
        let rival = w.players.iter().find(|p| p.id != w.human).unwrap();
        assert_eq!(
            end_turn_view(&w, Some(rival.id), None).state,
            EndTurnState::Waiting(Some(rival.civ_name.clone()))
        );
        assert_eq!(
            end_turn_view(&w, None, None).state,
            EndTurnState::Waiting(None)
        );
    }

    #[test]
    fn the_end_turn_button_of_a_session_follows_the_layer() {
        let mut w = fixture_world();
        w.blocker = Some(Blocker {
            kind: 3,
            target: BlockerTarget::Unit(4),
        });
        let label = |w: &WorldView| net_end_turn_view(w, None, &[], None, None, true).label();
        assert_eq!(label(&w), "Unit needs orders");
        // The layer took the unit's orders: the button says "Next turn" before the server has said so.
        let u = w.units.iter_mut().find(|u| u.id == 4).unwrap();
        u.flags = protocol::UnitFlags(u.flags.0 & !protocol::UnitFlags::NEEDS_ORDERS);
        assert_eq!(label(&w), "Next turn");
        assert_eq!(
            end_turn_view(&w, None, None).label(),
            "Unit needs orders",
            "the local game has no layer"
        );
        // The line is down: the same button, grey; the note is kept.
        let v = net_end_turn_view(&w, None, &[], None, Some("x".into()), false);
        assert!(v.no_connection);
        assert_eq!(v.note.as_deref(), Some("x"));
    }

    // ---- the game of a server session (lobby-ui 4.7)

    fn pview(id: i8, flags: u16, civ: &str) -> protocol::PlayerView {
        protocol::PlayerView {
            id,
            team: id,
            flags: PlayerFlags(flags),
            civ_name: civ.into(),
            color_primary: 0x10_20_30_FF,
            ..protocol::PlayerView::default()
        }
    }

    fn seat(player: i8, nick: Option<&str>, conn: Conn, host: bool) -> SeatView {
        SeatView {
            player,
            control: match nick {
                Some(n) => SeatControl::Human { nick: n.into() },
                None => SeatControl::Ai { kicked: None },
            },
            conn,
            host,
        }
    }

    fn sturn(player: i8, finished: bool, may_act: bool, group: u8) -> protocol::SeatTurn {
        protocol::SeatTurn {
            player,
            finished,
            may_act,
            group,
        }
    }

    fn turn_state(phase: TurnPhase, seats: Vec<protocol::SeatTurn>) -> TurnState {
        TurnState {
            turn: 5,
            phase,
            mode: TurnMode::Simultaneous,
            seats,
            timers: Vec::new(),
            paused: false,
        }
    }

    const ME: i8 = 0;
    const ALIVE: u16 = PlayerFlags::ALIVE;

    /// Rome (us, human), Egypt (human Bob), Greece (AI), a city-state and the barbarians.
    fn game_world() -> WorldView {
        let mut w = fixture_world();
        w.human = ME;
        w.players = vec![
            pview(ME, ALIVE | PlayerFlags::HUMAN, "Rome"),
            pview(1, ALIVE | PlayerFlags::HUMAN | PlayerFlags::MET, "Egypt"),
            pview(2, ALIVE | PlayerFlags::MET, "Greece"),
            pview(3, ALIVE | PlayerFlags::MINOR | PlayerFlags::MET, "Geneva"),
            pview(4, ALIVE | PlayerFlags::BARBARIAN, "Barbarians"),
        ];
        w.peace_offers.clear();
        w
    }

    fn game_seats() -> Vec<SeatView> {
        vec![
            seat(ME, Some("Ann"), Conn::Online, true),
            seat(1, Some("Bob"), Conn::Online, false),
            seat(2, None, Conn::None, false),
        ]
    }

    #[test]
    fn end_turn_states_of_a_server_game() {
        let mut w = game_world();
        w.status = TurnStatus::HumanTurn;
        w.blocker = None;
        let seats = game_seats();
        let view = |ts: &TurnState, orders| {
            net_end_turn_view(&w, Some(ts), &seats, Some(ME), None, orders)
        };
        let humans = |me_done, bob_done| {
            turn_state(
                TurnPhase::Humans,
                vec![sturn(ME, me_done, true, 0), sturn(1, bob_done, true, 0)],
            )
        };
        assert_eq!(view(&humans(false, false), true).state, EndTurnState::Ready);
        // We are done: the button waits for Bob, by his nick.
        let v = view(&humans(true, false), true);
        assert_eq!(
            v.state,
            EndTurnState::WaitingHumans {
                names: vec!["Bob".into()]
            }
        );
        assert_eq!(v.label(), "Waiting for 1 player");
        // An order took the end of turn back: the layer shows `finished = false` and the button is back.
        assert_eq!(view(&humans(false, false), true).state, EndTurnState::Ready);
        // Hybrid: another player has the turn.
        let mut hybrid = turn_state(
            TurnPhase::Humans,
            vec![sturn(ME, false, false, 1), sturn(1, false, true, 2)],
        );
        hybrid.mode = TurnMode::Hybrid;
        assert_eq!(
            view(&hybrid, true).state,
            EndTurnState::WaitingTurn { who: "Bob".into() }
        );
        // The server prepares the turn; the AI moves.
        let starting = turn_state(TurnPhase::Starting, vec![]);
        assert_eq!(view(&starting, true).state, EndTurnState::Starting);
        let ai = turn_state(TurnPhase::Ai { active: 2 }, vec![]);
        assert_eq!(
            view(&ai, true).state,
            EndTurnState::Waiting(Some("Greece".into()))
        );
        // Game over keeps its text.
        let mut ended = w.clone();
        ended.status = TurnStatus::GameOver { winner: Some(0) };
        let over = turn_state(TurnPhase::GameOver { winner: Some(0) }, vec![]);
        assert!(matches!(
            net_end_turn_view(&ended, Some(&over), &seats, Some(ME), None, true).state,
            EndTurnState::GameOver(_)
        ));
        // Without a line: the same state, grey.
        assert!(view(&humans(true, false), false).no_connection);
    }

    #[test]
    fn players_rows_and_the_diplomacy_matrix() {
        let mut w = game_world();
        let seats = game_seats();
        // Peace with a human, war with the AI.
        w.players[2].flags = PlayerFlags(w.players[2].flags.0 | PlayerFlags::AT_WAR_WITH_VIEWER);
        let rows = player_rows(&w, None, &seats, Some(ME), 0);
        assert_eq!(
            rows.iter().map(|r| r.civ.as_str()).collect::<Vec<_>>(),
            ["Rome", "Egypt", "Greece"],
            "no city-states, no barbarians"
        );
        assert_eq!(rows[0].diplomacy, None, "not with ourselves");
        assert_eq!(rows[1].diplomacy, Some(Diplomacy::Peace));
        assert_eq!(rows[2].diplomacy, Some(Diplomacy::WarAi));
        assert_eq!(rows[1].nick.as_deref(), Some("Bob"));
        assert_eq!(rows[2].nick, None);
        assert_eq!(rows[2].line, PlayerLine::Ai);
        assert_eq!(rows[1].color, [0x10, 0x20, 0x30]);
        assert!(rows[0].me && rows[0].host && !rows[1].host);
        // We are the host: Kick for the other human, not for the AI nor ourselves.
        assert_eq!(
            rows.iter().map(|r| r.can_kick).collect::<Vec<_>>(),
            [false, true, false]
        );
        // War with the human Bob: nothing, our offer, his offer.
        w.players[1].flags = PlayerFlags(w.players[1].flags.0 | PlayerFlags::AT_WAR_WITH_VIEWER);
        let dip = |w: &WorldView| player_rows(w, None, &seats, Some(ME), 0)[1].diplomacy;
        assert_eq!(dip(&w), Some(Diplomacy::WarHuman));
        w.peace_offers = vec![protocol::PeaceOffer {
            from: ME,
            to: 1,
            turn: 5,
        }];
        assert_eq!(dip(&w), Some(Diplomacy::WarOffered));
        w.peace_offers = vec![protocol::PeaceOffer {
            from: 1,
            to: ME,
            turn: 5,
        }];
        assert_eq!(dip(&w), Some(Diplomacy::WarIncoming));
        // Not met: no buttons.
        w.players[1].flags = PlayerFlags(ALIVE | PlayerFlags::HUMAN);
        assert_eq!(dip(&w), None);
        // A guest has no Kick.
        let seats = vec![
            seat(ME, Some("Ann"), Conn::Online, false),
            seat(1, Some("Bob"), Conn::Online, true),
        ];
        assert!(
            player_rows(&w, None, &seats, Some(ME), 0)
                .iter()
                .all(|r| !r.can_kick)
        );
    }

    #[test]
    fn players_turn_marks_and_the_line() {
        let w = game_world();
        let mut seats = game_seats();
        seats[1].conn = Conn::Lost { since_ms: 10_000 };
        let ts = turn_state(
            TurnPhase::Humans,
            vec![sturn(ME, true, true, 0), sturn(1, false, true, 0)],
        );
        let rows = player_rows(&w, Some(&ts), &seats, Some(ME), 75_000);
        assert_eq!(rows[0].turn, TurnMark::Done);
        assert_eq!(rows[1].turn, TurnMark::Thinking);
        assert_eq!(rows[2].turn, TurnMark::None, "the AI has none");
        assert_eq!(rows[1].line, PlayerLine::Lost("1:05".into()));
        // The order took our end of turn back: "Done" becomes "Thinking" (LU-D20).
        let ts = turn_state(
            TurnPhase::Humans,
            vec![sturn(ME, false, true, 0), sturn(1, false, true, 0)],
        );
        assert_eq!(
            player_rows(&w, Some(&ts), &seats, Some(ME), 0)[0].turn,
            TurnMark::Thinking
        );
        // Hybrid: a waiting one.
        let mut ts = turn_state(
            TurnPhase::Humans,
            vec![sturn(ME, false, true, 0), sturn(1, false, false, 1)],
        );
        ts.mode = TurnMode::Hybrid;
        assert_eq!(
            player_rows(&w, Some(&ts), &seats, Some(ME), 0)[1].turn,
            TurnMark::Waiting
        );
    }

    #[test]
    fn a_host_that_is_away_has_the_countdown() {
        let w = game_world();
        let mut seats = game_seats();
        // We are a guest; the host Bob is away for 18 s.
        seats[0].host = false;
        seats[1].host = true;
        seats[1].conn = Conn::Lost { since_ms: 100_000 };
        let rows = player_rows(&w, None, &seats, Some(ME), 118_000);
        assert_eq!(rows[1].host_passes_in_s, Some(42));
        assert!(rows[1].host && !rows[1].can_kick, "a guest cannot kick");
        // After the 60 s the label goes; the new host comes with the next `Seats`.
        let rows = player_rows(&w, None, &seats, Some(ME), 170_000);
        assert_eq!(rows[1].host_passes_in_s, None);
        // The returned former host is an ordinary player: no label, no buttons, no countdown.
        seats[1].host = false;
        seats[1].conn = Conn::Online;
        seats[0].host = true;
        let rows = player_rows(&w, None, &seats, Some(ME), 200_000);
        assert!(!rows[1].host && rows[1].host_passes_in_s.is_none());
        assert!(
            rows[0].host && rows[1].can_kick,
            "the new host may remove him"
        );
    }

    fn timers(group: u8, stopped: Option<u32>) -> Vec<protocol::TimerState> {
        vec![protocol::TimerState {
            group,
            deadline_ms: 100_000,
            len_ms: 60_000,
            left_ms_when_stopped: stopped,
        }]
    }

    #[test]
    fn the_timer_states() {
        let seats = game_seats();
        let mut ts = turn_state(TurnPhase::Humans, vec![sturn(ME, false, true, 0)]);
        ts.timers = timers(0, None);
        let t = timer_view(Some(&ts), &seats, Some(ME), Some(83_000), true).unwrap();
        assert_eq!(
            (t.text.as_str(), t.red, t.stopped.is_some()),
            ("1:23", false, false)
        );
        assert!(t.can_pause, "we are the host");
        // Under 10 s: red.
        assert!(
            timer_view(Some(&ts), &seats, Some(ME), Some(9_000), true)
                .unwrap()
                .red
        );
        // No timer in the state: no field.
        ts.timers.clear();
        assert!(timer_view(Some(&ts), &seats, Some(ME), None, true).is_none());
        // The host's pause.
        ts.timers = timers(0, Some(30_000));
        ts.paused = true;
        let t = timer_view(Some(&ts), &seats, Some(ME), Some(30_000), true).unwrap();
        assert_eq!(t.stopped.as_deref(), Some("stopped (paused by host)"));
        assert!(t.paused);
        // Stopped for a human that dropped out.
        ts.paused = false;
        let mut lost = game_seats();
        lost[1].conn = Conn::Lost { since_ms: 1 };
        let t = timer_view(Some(&ts), &lost, Some(ME), Some(30_000), true).unwrap();
        assert_eq!(t.stopped.as_deref(), Some("stopped (waiting for Bob)"));
        // A guest has no button; the timer is that of our own group.
        let mut guest = game_seats();
        guest[0].host = false;
        let mut ts2 = turn_state(
            TurnPhase::Humans,
            vec![sturn(ME, false, true, 2), sturn(1, false, true, 0)],
        );
        ts2.timers = timers(2, None);
        let t = timer_view(Some(&ts2), &guest, Some(ME), Some(5_000), true).unwrap();
        assert!(!t.can_pause);
        assert_eq!(t.text, "0:05");
    }

    #[test]
    fn chat_lines_age_by_our_clock_and_privates_name_both() {
        use protocol::ChatLine;
        let w = game_world();
        let seats = game_seats();
        let entry = |from: &str, from_player, to, text: &str, received_ms| ChatEntry {
            line: ChatLine {
                from: from.into(),
                from_player,
                to,
                text: text.into(),
                server_ms: 7,
            },
            received_ms,
        };
        let lines = vec![
            entry("Bob", Some(1), ChatTo::All, "hello", 1_000),
            entry("Bob", Some(1), ChatTo::Player(ME), "psst", 61_000),
            entry("Ann", Some(ME), ChatTo::Player(1), "ok", 90_000),
        ];
        let v = chat_view(&lines, &seats, &w, Some(ME), 181_000, 2, true);
        assert_eq!(v.lines[0].text(), "3 min ago Bob: hello");
        assert_eq!(v.lines[1].text(), "2 min ago (private) Bob -> you: psst");
        assert_eq!(v.lines[2].text(), "1 min ago (private) Ann -> Bob: ok");
        assert_eq!(v.unread, 2);
        assert_eq!(
            v.targets,
            vec![ChatTarget {
                player: 1,
                nick: "Bob".into()
            }],
            "the other humans only"
        );
        assert!(is_mine(&lines[2], Some(ME), "Ann"));
        assert!(!is_mine(&lines[0], Some(ME), "Ann"));
    }

    #[test]
    fn notes_live_their_time_and_the_events_make_them() {
        use protocol::GameCtl;
        let mut hud = Hud::new();
        hud.on_game_event(
            &ClientEvent::GameCtlRejected {
                ctl: GameCtl::Pause(true),
                reason: GameCtlReason::NotHost,
            },
            1_000,
        );
        assert_eq!(
            live(&hud.players_note, 1_000).as_deref(),
            Some("Only the host can do this")
        );
        assert!(live(&hud.players_note, 6_999).is_some());
        assert!(live(&hud.players_note, 7_000).is_none(), "6 s");
        // A new host: the text depends on who.
        hud.on_game_event(
            &ClientEvent::HostChanged {
                from: Some("Ann".into()),
                to: "Bob".into(),
                me: false,
            },
            2_000,
        );
        assert_eq!(
            live(&hud.players_note, 2_000).as_deref(),
            Some("Bob is the host now.")
        );
        hud.on_game_event(
            &ClientEvent::HostChanged {
                from: None,
                to: "Ann".into(),
                me: true,
            },
            2_000,
        );
        assert_eq!(
            live(&hud.players_note, 2_000).as_deref(),
            Some("You are the host now: the previous host was away for 60 s.")
        );
        // An order that cancelled the end of turn: 3 s; an explicit cancel gives no note.
        hud.on_game_event(&ClientEvent::EndTurnCancelled { by_order: false }, 0);
        assert!(live(&hud.cancel_note, 0).is_none());
        hud.on_game_event(&ClientEvent::EndTurnCancelled { by_order: true }, 10_000);
        assert_eq!(
            live(&hud.cancel_note, 12_999).as_deref(),
            Some("End turn cancelled by your order. Press Next turn when you are done.")
        );
        assert!(live(&hud.cancel_note, 13_000).is_none());
        // A refused order: 6 s over the button and in the city window.
        hud.on_game_event(
            &ClientEvent::OrderRolledBack {
                id: 3,
                text: "The turn is already ending".into(),
            },
            20_000,
        );
        assert!(live(&hud.refusal, 25_999).is_some());
        assert!(live(&hud.refusal, 26_000).is_none());
    }

    #[test]
    fn the_unit_panel_says_why_the_turn_takes_no_orders() {
        let w = game_world();
        let seats = game_seats();
        let block = |ts: &TurnState| order_block(&w, Some(ts), &seats, Some(ME));
        let ai = turn_state(TurnPhase::Ai { active: 2 }, vec![]);
        assert_eq!(block(&ai).as_deref(), Some("Rivals are moving"));
        let starting = turn_state(TurnPhase::Starting, vec![]);
        assert_eq!(block(&starting).as_deref(), Some("Rivals are moving"));
        // Our own end of turn does not block the panel (LU-D20).
        let done = turn_state(TurnPhase::Humans, vec![sturn(ME, true, true, 0)]);
        assert_eq!(block(&done), None);
        let mut hybrid = turn_state(
            TurnPhase::Humans,
            vec![sturn(ME, false, false, 1), sturn(1, false, true, 2)],
        );
        hybrid.mode = TurnMode::Hybrid;
        assert_eq!(block(&hybrid).as_deref(), Some("Waiting for Bob"));
        assert_eq!(order_block(&w, None, &seats, Some(ME)), None);
    }

    #[test]
    fn every_refusal_has_its_words() {
        for (r, text) in [
            (GameCtlReason::NotHost, "Only the host can do this"),
            (GameCtlReason::NoSuchPlayer, "No such player"),
            (
                GameCtlReason::NotHuman,
                "This civilization is not played by a human",
            ),
            (GameCtlReason::SelfKick, "You cannot remove yourself"),
            (GameCtlReason::LastHuman, "This is the last human player"),
            (GameCtlReason::NoGame, "The game is not running"),
        ] {
            assert_eq!(ctl_refusal_text(r), text);
        }
    }

    #[test]
    fn the_settings_window_opens_and_closes_before_leaving() {
        let mut hud = Hud::new();
        assert!(!hud.settings_open());
        assert!(hud.take_actions(vec![UiAction::ToggleSettings]).is_empty());
        assert!(hud.settings_open());
        // The question "Leave game?" comes up over the map: the window closes, the action goes on.
        assert_eq!(
            hud.take_actions(vec![UiAction::LeaveGame]),
            [UiAction::LeaveGame]
        );
        assert!(!hud.settings_open());
        let kept = hud.take_actions(vec![UiAction::ToggleStats(true)]);
        assert_eq!(kept, [UiAction::ToggleStats(true)]);
    }

    #[test]
    fn the_tabs_and_folds_are_the_huds() {
        let mut hud = Hud::new();
        assert_eq!(
            hud.take_actions(vec![
                UiAction::LogTab(LogTab::Chat),
                UiAction::TogglePlayers,
                UiAction::CancelEndTurn,
            ]),
            [UiAction::CancelEndTurn]
        );
        assert_eq!(hud.tab, LogTab::Chat);
        assert!(hud.players_open && hud.journal_open);
        hud.take_actions(vec![UiAction::ToggleJournal]);
        assert!(!hud.journal_open);
        // T: the chat opens with its input focused.
        hud.focus_chat();
        assert!(hud.journal_open && hud.forms().chat.focus);
    }

    #[test]
    fn journal_kinds_colour_the_lines() {
        let e = |kind| JournalEntry {
            turn: 1,
            kind,
            text: "x".into(),
            at: None,
            simplification: None,
        };
        let lines = journal_lines(&[
            e(JournalKind::System),
            e(JournalKind::Session),
            e(JournalKind::Diplomacy),
            e(JournalKind::Notification),
        ]);
        assert_eq!(
            lines.iter().map(|l| l.kind).collect::<Vec<_>>(),
            [
                LineKind::System,
                LineKind::Session,
                LineKind::Diplomacy,
                LineKind::Normal
            ]
        );
    }

    #[test]
    fn the_game_screens_of_a_session_hold_the_new_panels_and_grey_without_a_line() {
        let w = game_world();
        let seats = game_seats();
        let ts = turn_state(
            TurnPhase::Humans,
            vec![sturn(ME, true, true, 0), sturn(1, false, true, 0)],
        );
        let production = ProductionChooser::default();
        let mut hud = Hud::new();
        let chat = [ChatEntry {
            line: protocol::ChatLine {
                from: "Bob".into(),
                from_player: Some(1),
                to: ChatTo::All,
                text: "hi".into(),
                server_ms: 1,
            },
            received_ms: 0,
        }];
        let play = |orders_enabled| NetPlay {
            world: &w,
            catalog: None,
            journal: &[],
            finished: true,
            orders_enabled,
            turn: Some(&ts),
            seats: &seats,
            chat: &chat,
            me: Some(ME),
            nick: "Ann",
            now_ms: 100_000,
            server_ms: 100_000,
            timer_left_ms: None,
            production: &production,
            fps: 60.0,
            unit: None,
            labels: None,
            confirm: None,
            tooltip: None,
        };
        let mut out = Screens {
            conn: Some(interim_ui::screens::conn::ConnView {
                dot: interim_ui::screens::conn::Dot::Green,
                text: "Online, 35 ms".into(),
                orders_enabled: true,
                status: None,
            }),
            ..Screens::default()
        };
        hud.net_play(&play(true), &mut out);
        let players = out.players.as_ref().expect("players");
        assert_eq!(players.rows.len(), 3);
        assert!(players.orders_enabled && !players.open);
        let journal = out.journal.as_ref().expect("journal");
        assert_eq!(journal.chat.as_ref().map(|c| c.unread), Some(1));
        assert_eq!(
            out.end_turn.as_ref().map(|e| e.label()).as_deref(),
            Some("Waiting for 1 player")
        );
        assert_eq!(
            out.top_bar.as_ref().and_then(|b| b.line.clone()),
            Some((
                interim_ui::screens::conn::Dot::Green,
                "Online, 35 ms".into()
            ))
        );
        // Opening the chat tab reads the lines.
        hud.take_actions(vec![UiAction::LogTab(LogTab::Chat)]);
        let mut out2 = Screens::default();
        hud.net_play(&play(false), &mut out2);
        assert_eq!(
            out2.journal
                .as_ref()
                .and_then(|j| j.chat.as_ref())
                .map(|c| (c.unread, c.orders_enabled)),
            Some((0, false))
        );
        assert!(out2.end_turn.as_ref().unwrap().no_connection);
        assert!(!out2.players.as_ref().unwrap().orders_enabled);
    }

    /// An eliminated human sees `GameOver` in his own view while the turn state goes on (the others play).
    #[test]
    fn a_defeated_human_gets_the_window_then_watches_without_orders() {
        let mut w = game_world();
        w.status = TurnStatus::GameOver { winner: None };
        let seats = game_seats();
        let ts = turn_state(TurnPhase::Humans, vec![sturn(1, false, true, 0)]);
        let production = ProductionChooser::default();
        let mut hud = Hud::new();
        let frame = |hud: &mut Hud, w: &WorldView, ts: &TurnState| {
            let mut out = Screens::default();
            hud.net_play(
                &NetPlay {
                    world: w,
                    catalog: None,
                    journal: &[],
                    finished: false,
                    orders_enabled: true,
                    turn: Some(ts),
                    seats: &seats,
                    chat: &[],
                    me: Some(ME),
                    nick: "Ann",
                    now_ms: 1,
                    server_ms: 1,
                    timer_left_ms: None,
                    production: &production,
                    fps: 60.0,
                    unit: None,
                    labels: None,
                    confirm: None,
                    tooltip: None,
                },
                &mut out,
            );
            out
        };
        assert!(is_defeated(&w, Some(&ts)));
        let out = frame(&mut hud, &w, &ts);
        let win = out.defeated.as_ref().expect("the defeat window");
        assert!(win.text.contains("defeated"));
        assert_eq!(
            out.end_turn.as_ref().map(|e| e.label()).as_deref(),
            Some("You were defeated")
        );
        assert_eq!(
            order_block(&w, Some(&ts), &seats, Some(ME)).as_deref(),
            Some("You were defeated")
        );
        // Leave goes on to the shell; Watch is the hud's: the window shuts, the orders stay off.
        let kept = hud.take_actions(vec![UiAction::DefeatLeave, UiAction::DefeatWatch]);
        assert_eq!(kept, [UiAction::DefeatLeave]);
        let out = frame(&mut hud, &w, &ts);
        assert!(out.defeated.is_none());
        assert!(order_block(&w, Some(&ts), &seats, Some(ME)).is_some());
        // The game is over for everybody (turn phase too), or we are alive: no window, and the next defeat asks again.
        let over = turn_state(TurnPhase::GameOver { winner: Some(1) }, vec![]);
        assert!(!is_defeated(&w, Some(&over)));
        assert!(!is_defeated(&w, None));
        let alive = game_world();
        assert!(!is_defeated(&alive, Some(&ts)));
        assert!(frame(&mut hud, &alive, &ts).defeated.is_none());
        assert!(frame(&mut hud, &w, &ts).defeated.is_some());
    }

    #[test]
    fn markup_tags_are_dropped() {
        assert_eq!(
            plain_text(
                "Paris now has 2 [ICON_CITIZEN] Citizens![NEWLINE]Gain [COLOR_POSITIVE_TEXT]+1[ENDCOLOR] [ICON_FOOD] Food."
            ),
            "Paris now has 2 Citizens! Gain +1 Food."
        );
        assert_eq!(
            plain_text("A [note] stays [unclosed"),
            "A [note] stays [unclosed"
        );
    }

    #[test]
    fn journal_keeps_the_newest_lines() {
        let e = |turn| JournalEntry {
            turn,
            kind: JournalKind::AutoChoice,
            text: "Research".into(),
            at: None,
            simplification: Some(2),
        };
        let entries: Vec<_> = (0..JOURNAL_LINES as i32 + 5).map(e).collect();
        let lines = journal_lines(&entries);
        assert_eq!(lines.len(), JOURNAL_LINES);
        assert_eq!(lines[0].turn, 5);
        assert_eq!(lines[0].simplification, Some(2));
    }

    #[test]
    fn journal_lines_keep_the_place_and_a_click_names_the_camera_target() {
        let e = |at| JournalEntry {
            turn: 3,
            kind: JournalKind::Combat,
            text: "Your Archer lost 2 HP (8/10 left)".into(),
            at,
            simplification: None,
        };
        let lines = journal_lines(&[e(Some((9, 4))), e(None)]);
        assert_eq!(lines[0].at, Some((9, 4)));
        assert_eq!(lines[1].at, None);
        let (x, y) = lines[0].at.unwrap();
        assert_eq!(camera_target(&UiAction::JournalGoto { x, y }), Some((9, 4)));
        assert_eq!(camera_target(&UiAction::ToggleJournal), None);
        // The hud lets the click through to the app.
        let mut hud = Hud::new();
        assert_eq!(
            hud.take_actions(vec![UiAction::JournalGoto { x: 9, y: 4 }]),
            [UiAction::JournalGoto { x: 9, y: 4 }]
        );
    }
}
