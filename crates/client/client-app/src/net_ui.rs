//! The screens of a server session (lobby-ui 4.1-4.6): which one is up for the `ClientPhase`, the view data
//! each one shows, and what the player's clicks do to the `ClientCore`.
//!
//! The screens themselves are in `interim-ui` and know no protocol types (LU-D1). Everything here that decides
//! something is a plain function over `protocol` types, so it is tested without a window or a socket.

use client_core::{
    ClientCore, ClientEvent, ClientPhase, ConnState, NoticeKind, OrderRefused, ReconnectStatus,
};
use interim_ui::screens::conn::{ConnView, Dot};
use interim_ui::screens::loading::LoadingView;
use interim_ui::screens::login::{ErrorButton, LoginView};
use interim_ui::screens::notice::{NoticeButton, NoticeView};
use interim_ui::screens::room::{
    AutoRow, Choice, CivTipRow, MapOptionRow, OptionGroup, OptionRow, RoomEdit, RoomView,
    SettingsView, SlotKind, SlotRow, SlotWho, TimerChoice, TurnModeChoice, VictoryRow,
};
use interim_ui::screens::rooms::{
    GameRow, LobbyListView, MySeat, OnlineRow, RoomRow, SaveRow, SeatRow,
};
use interim_ui::screens::{Screens, UiAction};
use protocol::{
    AutoChoices, CivChoice, CivPick, GameSettings, GameStatus, LobbyChoices, LobbyCmd, LobbyState,
    MAX_TURNS_KEY, MAX_TURNS_LIMIT, Occupant, PlayerPlace, RejectReason, RoomState, SeatControl,
    SlotOccupant, TimerMode, TurnMode, VICTORY_PREFIX,
};

use crate::autostart::{
    RoomParams, RoomScript, ScriptState, default_settings, settings_from_last_room,
};
use crate::hud::{NetScreen, map_name, net_screen, size_name, utc_text};

/// A refusal of the server stays under the lists this long.
pub const ERROR_SHOW_MS: u64 = 8_000;
/// A note over the room (the new host) stays this long.
pub const NOTE_SHOW_MS: u64 = 6_000;
/// A text typed over a settings change that has not been echoed yet is built on the sent values for this long.
pub const SETTINGS_BASE_MS: u64 = 2_000;
/// The stages of the loading screen (lobby-ui 4.6).
pub const LOADING_STEPS: u32 = 5;
/// "Starting the game on the server" that lasts this long is a game that will not come (the server answers a
/// failed start with a refusal, so this is the net under a lost message or a stuck worker): the player is told
/// and is back in the room or the lobby. The worker's own limit for the biggest map is 12 minutes of silence;
/// its heartbeats are not the client's to see, so this is a generous 3 minutes.
pub const START_TIMEOUT_MS: u64 = 180_000;

/// Options a player cannot set (gc-abi-v2 4.1; the server refuses them), so the room does not offer them.
const FORBIDDEN_OPTIONS: [&str; 6] = [
    "GAMEOPTION_PITBOSS",
    "END_TURN_TIMER_ENABLED",
    "QUICK_COMBAT",
    "QUICK_MOVEMENT",
    "FIRSTMOVE",
    "FAST_HAND",
];

// ---------------------------------------------------------------- editing the settings (LU-D3)

/// Builds a settings edit on the last *sent* values, so two quick edits do not undo each other (4.4).
#[derive(Debug, Default)]
pub struct SettingsEditor {
    sent: Option<(GameSettings, u64)>,
}

impl SettingsEditor {
    /// The settings an edit starts from: the ones last sent while the server has not echoed them (at most
    /// [`SETTINGS_BASE_MS`]), else the snapshot.
    pub fn base(&mut self, snapshot: &GameSettings, now_ms: u64) -> GameSettings {
        match &self.sent {
            Some((s, at)) if now_ms.saturating_sub(*at) < SETTINGS_BASE_MS && s != snapshot => {
                s.clone()
            }
            _ => {
                self.sent = None;
                snapshot.clone()
            }
        }
    }

    /// The settings with `edit` applied to the base; they are the new "sent" values.
    pub fn edit(
        &mut self,
        snapshot: &GameSettings,
        edit: &RoomEdit,
        choices: &LobbyChoices,
        now_ms: u64,
    ) -> GameSettings {
        let mut s = self.base(snapshot, now_ms);
        apply_edit(&mut s, edit, choices);
        self.sent = Some((s.clone(), now_ms));
        s
    }

    pub fn reset(&mut self) {
        self.sent = None;
    }
}

/// One changed field of the settings.
pub fn apply_edit(s: &mut GameSettings, edit: &RoomEdit, choices: &LobbyChoices) {
    match edit {
        RoomEdit::Name(v) => s.name.clone_from(v),
        RoomEdit::WorldSize(v) => {
            s.world_size.clone_from(v);
            // A smaller map takes fewer city-states: the server would refuse the old number.
            if let Some(size) = choices.world_sizes.iter().find(|w| &w.id == v) {
                s.minor_civs = s.minor_civs.min(size.max_minor);
            }
        }
        RoomEdit::MapScript(v) => {
            s.map_script.clone_from(v);
            // SIMPLIFIED(SIMP-uic-64): the options of the old script mean nothing to the new one: its own defaults.
            s.map_options = crate::autostart::script_default_options(choices, v);
        }
        RoomEdit::Speed(v) => s.speed.clone_from(v),
        RoomEdit::Era(v) => s.start_era.clone_from(v),
        RoomEdit::Handicap(v) => s.handicap.clone_from(v),
        RoomEdit::MinorCivs(n) => {
            // SIMPLIFIED(SIMP-uic-65): up to what the map size takes (the server refuses more).
            let max = choices
                .world_sizes
                .iter()
                .find(|w| w.id == s.world_size)
                .map_or(*n, |w| w.max_minor);
            s.minor_civs = (*n).min(max);
        }
        RoomEdit::TurnMode(m) => {
            s.turn_mode = match m {
                TurnModeChoice::Simultaneous => TurnMode::Simultaneous,
                TurnModeChoice::Hybrid => TurnMode::Hybrid,
            };
        }
        RoomEdit::Timer(t) => {
            s.timer = match t {
                TimerChoice::Off => TimerMode::Off,
                TimerChoice::Original(row) => TimerMode::Original { row: row.clone() },
                TimerChoice::Fixed(seconds) => TimerMode::Fixed { seconds: *seconds },
            };
        }
        RoomEdit::Option { key, value } => {
            s.game_options.retain(|(k, _)| k != key);
            let default = choices
                .options
                .iter()
                .find(|o| &o.key == key)
                .map(|o| o.default);
            // Only what differs from the default goes in the list.
            if default != Some(*value) {
                s.game_options.push((key.clone(), *value));
            }
        }
        RoomEdit::Seed(v) => s.seed = *v,
        RoomEdit::MapSeed(v) => s.map_seed = *v,
        // SIMPLIFIED(SIMP-uic-63): a victory that is not listed is on: only "off" goes into the list.
        RoomEdit::Victory { key, on } => {
            if key.starts_with(VICTORY_PREFIX) {
                s.game_options.retain(|(k, _)| k != key);
                if !*on {
                    s.game_options.push((key.clone(), 0));
                }
            }
        }
        // 0 is "off": no entry, the speed's own limit.
        RoomEdit::MaxTurns(n) => {
            s.game_options.retain(|(k, _)| k != MAX_TURNS_KEY);
            if *n > 0 {
                s.game_options
                    .push((MAX_TURNS_KEY.to_owned(), i32::from(*n).min(MAX_TURNS_LIMIT)));
            }
        }
        RoomEdit::MapOption { id, value } => {
            let known = choices
                .map_options
                .iter()
                .find(|m| m.script == s.map_script)
                .is_some_and(|m| m.options.iter().any(|o| &o.id == id && !o.read_only));
            if known {
                s.map_options.retain(|(k, _)| k != id);
                s.map_options.push((id.clone(), *value));
            }
        }
        RoomEdit::AutoChoice { key, on } => {
            if let Some(f) = auto_field(&mut s.auto_choices, key) {
                *f = *on;
            }
        }
    }
}

/// The decisions the AI can make for the human: the key of the edit, the label, the field.
const AUTO_ROWS: [(&str, &str); 11] = [
    ("research", "Research"),
    ("policy", "Social policies"),
    ("ideology", "Ideology"),
    ("religion", "Religion"),
    ("promotion", "Unit promotions"),
    ("great_person", "Great people"),
    ("captured_city", "Captured cities"),
    ("league", "World Congress"),
    ("archaeology", "Archaeology"),
    ("goody_hut", "Ancient ruins"),
    ("steal_tech", "Stealing technology"),
];

fn auto_field<'a>(a: &'a mut AutoChoices, key: &str) -> Option<&'a mut bool> {
    Some(match key {
        "research" => &mut a.research,
        "policy" => &mut a.policy,
        "ideology" => &mut a.ideology,
        "religion" => &mut a.religion,
        "promotion" => &mut a.promotion,
        "great_person" => &mut a.great_person,
        "captured_city" => &mut a.captured_city,
        "league" => &mut a.league,
        "archaeology" => &mut a.archaeology,
        "goody_hut" => &mut a.goody_hut,
        "steal_tech" => &mut a.steal_tech,
        _ => return None,
    })
}

fn auto_rows(a: &AutoChoices) -> Vec<AutoRow> {
    let mut a = *a;
    AUTO_ROWS
        .iter()
        .map(|(key, name)| AutoRow {
            key: (*key).to_owned(),
            name: (*name).to_owned(),
            on: auto_field(&mut a, key).is_some_and(|f| *f),
        })
        .collect()
}

fn group_of(g: protocol::OptionGroup) -> OptionGroup {
    match g {
        protocol::OptionGroup::General => OptionGroup::General,
        protocol::OptionGroup::Ai => OptionGroup::Ai,
        protocol::OptionGroup::Additional => OptionGroup::Additional,
        protocol::OptionGroup::Hidden => OptionGroup::Hidden,
    }
}

/// The custom options of the chosen map script with their current values (the default where the room has none).
fn map_option_rows(s: &GameSettings, choices: &LobbyChoices) -> Vec<MapOptionRow> {
    let Some(script) = choices
        .map_options
        .iter()
        .find(|m| m.script == s.map_script)
    else {
        return Vec::new();
    };
    script
        .options
        .iter()
        .map(|o| MapOptionRow {
            id: o.id.clone(),
            name: o.name.clone(),
            help: o.help.clone(),
            value: s
                .map_options
                .iter()
                .find(|(k, _)| k == &o.id)
                .map_or(o.default, |(_, v)| *v),
            values: o.values.clone(),
            read_only: o.read_only,
        })
        .collect()
}

/// "88 x 56, 8 players, 16 city-states".
fn size_info(choices: &LobbyChoices, id: &str) -> String {
    choices
        .world_sizes
        .iter()
        .find(|w| w.id == id)
        .map_or_else(String::new, |w| {
            format!(
                "{} x {}, {} players, {} city-states",
                w.width, w.height, w.default_players, w.default_minor
            )
        })
}

// ---------------------------------------------------------------- view data

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn choices_of(list: &[protocol::LobbyChoice]) -> Vec<Choice> {
    list.iter()
        .map(|c| Choice {
            key: c.type_name.clone(),
            name: c.name.clone(),
        })
        .collect()
}

/// The localized name of a map size, else a readable form of its id.
fn size_label(choices: Option<&LobbyChoices>, id: &str) -> String {
    choices
        .and_then(|c| c.world_sizes.iter().find(|w| w.id == id))
        .map_or_else(|| size_name(id), |w| w.name.clone())
}

fn script_label(choices: Option<&LobbyChoices>, id: &str) -> String {
    choices
        .and_then(|c| c.map_scripts.iter().find(|m| m.type_name == id))
        .map_or_else(|| map_name(id).to_owned(), |m| m.name.clone())
}

pub fn settings_view(s: &GameSettings, choices: &LobbyChoices) -> SettingsView {
    let max_minor = choices
        .world_sizes
        .iter()
        .find(|w| w.id == s.world_size)
        .map_or(0, |w| w.max_minor);
    SettingsView {
        name: s.name.clone(),
        world_size: s.world_size.clone(),
        map_script: s.map_script.clone(),
        speed: s.speed.clone(),
        era: s.start_era.clone(),
        handicap: s.handicap.clone(),
        minor_civs: s.minor_civs,
        max_minor,
        turn_mode: match s.turn_mode {
            TurnMode::Simultaneous => TurnModeChoice::Simultaneous,
            TurnMode::Hybrid => TurnModeChoice::Hybrid,
        },
        timer: match &s.timer {
            TimerMode::Off => TimerChoice::Off,
            TimerMode::Original { row } => TimerChoice::Original(row.clone()),
            TimerMode::Fixed { seconds } => TimerChoice::Fixed(*seconds),
        },
        size_info: size_info(choices, &s.world_size),
        options: choices
            .options
            .iter()
            .filter(|o| !FORBIDDEN_OPTIONS.contains(&o.key.as_str()))
            .map(|o| OptionRow {
                key: o.key.clone(),
                name: o.name.clone(),
                value: s
                    .game_options
                    .iter()
                    .find(|(k, _)| k == &o.key)
                    .map_or(o.default, |(_, v)| *v),
                values: o.values.clone(),
                help: o.help.clone(),
                group: group_of(o.group),
            })
            .collect(),
        // A victory not in the list is on.
        victories: choices
            .victories
            .iter()
            .map(|v| VictoryRow {
                key: v.type_name.clone(),
                name: v.name.clone(),
                on: s
                    .game_options
                    .iter()
                    .find(|(k, _)| k == &v.type_name)
                    .is_none_or(|(_, n)| *n != 0),
            })
            .collect(),
        max_turns: s
            .game_options
            .iter()
            .find(|(k, _)| k == MAX_TURNS_KEY)
            .map_or(0, |(_, n)| (*n).clamp(0, MAX_TURNS_LIMIT) as u16),
        map_options: map_option_rows(s, choices),
        auto_choices: auto_rows(&s.auto_choices),
        seed: s.seed,
        map_seed: s.map_seed,
        sizes: choices
            .world_sizes
            .iter()
            .map(|w| Choice {
                key: w.id.clone(),
                name: w.name.clone(),
            })
            .collect(),
        scripts: choices_of(&choices.map_scripts),
        speeds: choices_of(&choices.speeds),
        eras: choices_of(&choices.eras),
        // HANDICAP_AI is the AI players' own level, not one a human picks.
        handicaps: choices_of(&choices.handicaps)
            .into_iter()
            .filter(|c| c.key != "HANDICAP_AI")
            .collect(),
        timers: choices_of(&choices.timers),
    }
}

pub fn conn_view(core: &ClientCore) -> ConnView {
    let status = core.reconnect_status().map(|r| reconnect_text(&r));
    let (dot, text) = match core.conn() {
        ConnState::Online => (
            Dot::Green,
            core.rtt_ms()
                .map_or_else(|| "Online".to_owned(), |ms| format!("Online, {ms} ms")),
        ),
        ConnState::Poor => (Dot::Yellow, "Poor connection".to_owned()),
        ConnState::Lost | ConnState::Reconnecting | ConnState::Offline => {
            (Dot::Red, "No connection".to_owned())
        }
        ConnState::Replaced | ConnState::Closed => (Dot::Grey, "Closed".to_owned()),
    };
    ConnView {
        dot,
        text,
        orders_enabled: core.orders_enabled(),
        status: status.map(|s| {
            if core.conn() == ConnState::Offline {
                "No connection. Waiting for the network.".to_owned()
            } else {
                format!("No connection. {s}")
            }
        }),
    }
}

/// "Reconnecting... attempt 2, next in 3 s".
pub fn reconnect_text(r: &ReconnectStatus) -> String {
    let secs = r.next_in_ms.div_ceil(1000);
    if r.probing {
        format!("Reconnecting... attempt {}, trying now", r.attempt)
    } else {
        format!("Reconnecting... attempt {}, next in {secs} s", r.attempt)
    }
}

/// The text and the button of a refusal at sign-in (4.2). `None`: no refusal to show here.
pub fn reject_text(reason: &RejectReason) -> Option<(&'static str, Option<ErrorButton>)> {
    Some(match reason {
        // An unknown nick says the same (no account enumeration).
        RejectReason::BadPassword => ("Wrong nick or password.", None),
        RejectReason::BadNick => (
            "This nick is not allowed. Use 1-24 characters, no control characters.",
            None,
        ),
        RejectReason::VersionMismatch { .. } => (
            "The server was updated. Reload the page.",
            Some(ErrorButton::Reload),
        ),
        RejectReason::NickTaken => ("This nick is taken. Choose another one.", None),
        RejectReason::Full => (
            "The server is full. Try again later.",
            Some(ErrorButton::Retry),
        ),
        // The error screen shows it.
        // v13: the lines of the refusal are in the reason; the mod screen shows them.
        RejectReason::ModMismatch(_) => ("The game needs mods this client does not have.", None),
        RejectReason::Unsupported { .. } => return None,
    })
}

/// SIMPLIFIED(SIMP-uic-62): the sign-in is our own account page (the web page shows this view).
pub fn login_view(nick: &str, server: &str, rejected: Option<&RejectReason>) -> LoginView {
    let (error, button) = match rejected.and_then(reject_text) {
        Some((text, button)) => (Some(text.to_owned()), button),
        None => (None, None),
    };
    LoginView {
        nick: nick.to_owned(),
        server: server.to_owned(),
        error,
        button,
    }
}

fn game_status_text(s: GameStatus) -> &'static str {
    match s {
        GameStatus::Lobby => "gathering",
        GameStatus::Starting => "waking up...",
        GameStatus::Running => "running",
        GameStatus::Hibernated => "sleeping (joining wakes it)",
        GameStatus::Ended => "finished",
    }
}

fn seat_who(c: &SeatControl) -> String {
    match c {
        SeatControl::Human { nick } => nick.clone(),
        SeatControl::Ai { .. } => "AI".to_owned(),
        SeatControl::Closed => "closed".to_owned(),
    }
}

/// "Rome (alice), Egypt": the civilizations of a save and who played them.
fn save_civs(s: &protocol::SaveSummary) -> String {
    s.seats
        .iter()
        .map(|(_, civ, nick)| match nick {
            Some(n) => format!("{civ} ({n})"),
            None => civ.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn save_rows(state: &LobbyState) -> Vec<SaveRow> {
    state
        .saves
        .iter()
        .map(|s| SaveRow {
            id: s.id.clone(),
            name: s.name.clone(),
            turn: s.turn,
            saved: if s.saved_at_ms == 0 {
                "-".to_owned()
            } else {
                utc_text(s.saved_at_ms)
            },
            civs: save_civs(s),
            game: s.game,
            host: s.host.clone(),
        })
        .collect()
}

/// The lobby: rooms, games in progress (with the seats [Take] offers), saves.
pub fn lobby_list_view(
    state: &LobbyState,
    choices: Option<&LobbyChoices>,
    me: &str,
    conn: ConnView,
    error: Option<&str>,
) -> LobbyListView {
    // The seat the player has in a game: the game that lists the player as a human.
    let my_seat = state.games.iter().find_map(|g| {
        g.seats.iter().find_map(|s| match &s.control {
            SeatControl::Human { nick } if same(nick, me) => Some(MySeat {
                game: g.id,
                player: s.player,
                game_name: g.name.clone(),
                turn: g.turn,
                can_return: s.takeable,
            }),
            _ => None,
        })
    });
    LobbyListView {
        rooms: state
            .rooms
            .iter()
            .map(|r| RoomRow {
                id: r.id,
                name: r.name.clone(),
                host: r.host.clone(),
                players: format!("{}/{}", r.humans, r.slots),
                map: format!(
                    "{}, {}",
                    size_label(choices, &r.world_size),
                    script_label(choices, &r.map_script)
                ),
                starting: r.status == GameStatus::Starting,
            })
            .collect(),
        games: state
            .games
            .iter()
            .map(|g| GameRow {
                id: g.id,
                name: g.name.clone(),
                turn: g.turn,
                map: size_label(choices, &g.world_size),
                status: game_status_text(g.status).to_owned(),
                seats: g
                    .seats
                    .iter()
                    .map(|s| SeatRow {
                        player: s.player,
                        civ: s.civ_name.clone(),
                        who: seat_who(&s.control),
                        online: s.online,
                        takeable: s.takeable,
                        yours: match &s.control {
                            SeatControl::Human { nick } => same(nick, me),
                            SeatControl::Ai { kicked } => {
                                kicked.as_deref().is_some_and(|k| same(k, me))
                            }
                            SeatControl::Closed => false,
                        },
                    })
                    .collect(),
            })
            .collect(),
        saves: save_rows(state),
        error: error.map(str::to_owned),
        note: None,
        nick: me.to_owned(),
        conn,
        my_seat,
        confirm_give_up: false,
        online: online_rows(state, me),
    }
}

/// The online list for the page: the player first, then the server's order.
pub fn online_rows(state: &LobbyState, me: &str) -> Vec<OnlineRow> {
    let mut rows: Vec<OnlineRow> = state
        .online
        .iter()
        .map(|p| {
            let (place, status, ready, host) = match &p.place {
                PlayerPlace::Lobby => ("lobby", "In the lobby".to_owned(), None, false),
                PlayerPlace::Room {
                    name, host, ready, ..
                } => ("room", format!("In room {name}"), Some(*ready), *host),
                PlayerPlace::Game { name, turn, .. } => {
                    ("game", format!("Playing {name}, turn {turn}"), None, false)
                }
            };
            OnlineRow {
                nick: p.nick.clone(),
                me: same(&p.nick, me),
                place: place.to_owned(),
                status,
                ready,
                host,
            }
        })
        .collect();
    // Stable: the others keep the server's order.
    rows.sort_by_key(|r| !r.me);
    rows
}

/// "Waiting for: Anna, Bob" / "Need at least two civilizations" / ...: why [Start game] is off (LU-D4).
pub fn why_not_start(room: &RoomState) -> Option<String> {
    let waiting: Vec<&str> = room
        .slots
        .iter()
        .filter_map(|s| match &s.occupant {
            Occupant::Human { nick, ready: false } if !same(nick, &room.host) => {
                Some(nick.as_str())
            }
            _ => None,
        })
        .collect();
    if !waiting.is_empty() {
        return Some(format!("Waiting for: {}", waiting.join(", ")));
    }
    let taken = room
        .slots
        .iter()
        .filter(|s| matches!(s.occupant, Occupant::Human { .. } | Occupant::Ai))
        .count();
    if taken < 2 {
        return Some("Need at least two civilizations".to_owned());
    }
    // The server wants every slot settled; saying so spares the host a refusal.
    if room.slots.iter().any(|s| s.occupant == Occupant::Open) {
        return Some("Close or fill the open slots".to_owned());
    }
    None
}

/// The room window. `room_note` is the line over it (the new host), `error` the server's last refusal.
pub fn room_view(
    room: &RoomState,
    state: &LobbyState,
    choices: &LobbyChoices,
    me: &str,
    note: Option<&str>,
    error: Option<&str>,
) -> RoomView {
    let why_not = why_not_start(room);
    let from_save =
        room.from_save
            .as_ref()
            .map(|id| match state.saves.iter().find(|s| &s.id == id) {
                Some(s) => format!(
                    "Loaded from {}, turn {}. Players take their own civilizations",
                    s.name, s.turn
                ),
                None => format!("Loaded from {id}. Players take their own civilizations"),
            });
    RoomView {
        host: room.host.clone(),
        me_host: same(&room.host, me),
        starting: room.status == GameStatus::Starting,
        settings: settings_view(&room.settings, choices),
        slots: room
            .slots
            .iter()
            .map(|s| SlotRow {
                index: s.index,
                who: match &s.occupant {
                    Occupant::Open => SlotWho::Open,
                    Occupant::Human { nick, ready } => SlotWho::Human {
                        nick: nick.clone(),
                        ready: *ready,
                        me: same(nick, me),
                    },
                    Occupant::Ai => SlotWho::Ai,
                    Occupant::Closed => SlotWho::Closed,
                },
                civ: match &s.civ {
                    CivPick::Random => None,
                    CivPick::Civ(c) => Some(c.clone()),
                },
                was: s.reserved_for.clone(),
                yours: s.reserved_for.as_deref().is_some_and(|n| same(n, me))
                    && s.occupant == Occupant::Open,
            })
            .collect(),
        civs: choices
            .civs
            .iter()
            .map(|c| Choice {
                key: c.id.clone(),
                name: c.name.clone(),
            })
            .collect(),
        civ_tips: choices.civs.iter().filter_map(civ_tip_row).collect(),
        has_slot: room
            .slots
            .iter()
            .any(|s| matches!(&s.occupant, Occupant::Human { nick, .. } if same(nick, me))),
        can_start: same(&room.host, me) && why_not.is_none(),
        why_not,
        from_save,
        note: note.map(str::to_owned),
        error: error.map(str::to_owned),
        saves: save_rows(state),
    }
}

/// The structured tip of a civilization for the DOM room (P15: a styled tooltip with the game's pictures); None
/// when the core gave nothing for it.
pub fn civ_tip_row(c: &CivChoice) -> Option<CivTipRow> {
    (!c.leader.is_empty() || !c.traits.is_empty() || !c.unique.is_empty()).then(|| CivTipRow {
        key: c.id.clone(),
        leader: c.leader.clone(),
        traits: c.traits.clone(),
        unique: c.unique.clone(),
    })
}
/// The tip of a civilization in the staging room, as the original shows it on the pick list: the leader, the traits
/// of the leader and the unique units and buildings, one per line. SIMPLIFIED(SIMP-exe-961): the first leader of the
/// civilization. The DOM room shows the parts ([civ_tip_row]) as a card with pictures.
pub fn civ_tip(c: &CivChoice) -> String {
    let mut lines = Vec::new();
    if !c.leader.is_empty() {
        lines.push(c.leader.clone());
    }
    lines.extend(c.traits.iter().cloned());
    lines.extend(c.unique.iter().cloned());
    lines.join(
        "
",
    )
}

pub fn notice_view(kind: &NoticeKind) -> NoticeView {
    let (text, button, on_button) = match kind {
        NoticeKind::Kicked => (
            "You were removed from the game by the host. The AI plays your civilization. \
             You can take it back from the lobby."
                .to_owned(),
            "OK",
            NoticeButton::Dismiss,
        ),
        NoticeKind::Replaced => (
            "You signed in on another device or tab.".to_owned(),
            "Use this tab",
            NoticeButton::UseThisTab,
        ),
        NoticeKind::GameStopped { why } => (
            format!("The game was stopped: {why}."),
            "OK",
            NoticeButton::Dismiss,
        ),
        // The winner is in the world the client no longer shows; the line says what happened.
        NoticeKind::GameOver => (
            "Game over.".to_owned(),
            "Back to lobby",
            NoticeButton::Dismiss,
        ),
    };
    NoticeView {
        text,
        button: button.to_owned(),
        on_button,
    }
}

// ---------------------------------------------------------------- the screens and their actions

/// What the shell must do for an action: the core cannot (a store, the page).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetEffect {
    None,
    /// Sign in: the shell takes the token of that nick from its store (`NetClient::login`).
    Login {
        nick: String,
        password: String,
    },
    /// Forget the token of the nick and ask for the password again (`NetClient::sign_out`).
    SignOut,
    /// Reload the page.
    Reload,
}

#[derive(Debug, Default)]
pub struct NetUi {
    server: String,
    /// The server's last refusal and when it came.
    error: Option<(String, u64)>,
    /// The line over the room window and when it came.
    room_note: Option<(String, u64)>,
    editor: SettingsEditor,
    script: Option<RoomScript>,
    /// The seat being taken: the civilization name for "Taking the seat of Rome...".
    joining: Option<String>,
    /// The player typed a password on this page. Without it the first refusal (the automatic login without
    /// a token) is not a wrong password and the window does not say so.
    password_typed: bool,
    /// The loading stage shown and when it began (ms).
    stage: Option<(String, u64)>,
    /// Since when the phase is `Starting` (ms): `START_TIMEOUT_MS` ends the wait.
    starting_since: Option<u64>,
    /// [Give up seat] was pressed and the question is on the screen.
    confirm_give_up: bool,
    /// The rooms this lobby makes ([Create room], [Quick start]) leave the choices of the game to the human
    /// (`RoomParams::manual_choices`; the original UI, live report 2026-10-30: research and policies were
    /// chosen by the core and END TURN went on).
    manual_choices: bool,
}

impl NetUi {
    pub fn new(server: impl Into<String>) -> Self {
        Self {
            server: server.into(),
            ..Self::default()
        }
    }

    /// See [`NetUi::manual_choices`].
    pub fn set_manual_choices(&mut self, manual: bool) {
        self.manual_choices = manual;
    }

    /// The params of a room the lobby makes by itself.
    fn params(&self) -> RoomParams {
        RoomParams {
            manual_choices: self.manual_choices,
            ..RoomParams::default()
        }
    }

    /// The core's events: refusals, a new host, the script's cues.
    pub fn on_event(&mut self, ev: &ClientEvent, now_ms: u64) {
        match ev {
            ClientEvent::LobbyError(text) => {
                self.error = Some((text.clone(), now_ms));
                self.joining = None;
            }
            ClientEvent::HostChanged { to, me, .. } => {
                let text = if *me {
                    "You are the host now.".to_owned()
                } else {
                    format!("{to} is the host now.")
                };
                self.room_note = Some((text, now_ms));
            }
            ClientEvent::Detached(_) => self.joining = None,
            ClientEvent::LoggedIn => self.password_typed = false,
            _ => {}
        }
        if let Some(s) = &mut self.script {
            s.on_event(ev);
        }
    }

    /// Runs the quick start script one step (every frame).
    pub fn tick(&mut self, core: &mut ClientCore, now_ms: u64) {
        self.check_start_timeout(core, now_ms);
        let Some(script) = &mut self.script else {
            return;
        };
        match script.step(core, now_ms) {
            ScriptState::Running => {}
            ScriptState::Done => self.script = None,
            ScriptState::Failed(why) => {
                self.script = None;
                // The refusal was shown already when the server said it; a script's own reason is new.
                if self.error.as_ref().is_none_or(|(e, _)| e != &why) {
                    self.error = Some((why, now_ms));
                }
            }
        }
    }

    /// A start that has not attached for `START_TIMEOUT_MS` is given up with an error line.
    fn check_start_timeout(&mut self, core: &mut ClientCore, now_ms: u64) {
        if core.phase() != ClientPhase::Starting {
            self.starting_since = None;
            return;
        }
        let since = *self.starting_since.get_or_insert(now_ms);
        if now_ms.saturating_sub(since) < START_TIMEOUT_MS {
            return;
        }
        self.starting_since = None;
        core.abandon_start();
        self.joining = None;
        self.script = None;
        self.error = Some((
            "The game did not start: the server did not answer in time. Try again.".to_owned(),
            now_ms,
        ));
    }

    /// A game against the AI without the room window (`RoomScript`).
    pub fn quick_start(&mut self, params: RoomParams) {
        self.script = Some(RoomScript::new(params));
    }

    /// `true` while the quick start script runs (the lobby shows nothing in between).
    pub fn quick_starting(&self) -> bool {
        self.script.is_some()
    }

    fn live_error(&self, now_ms: u64) -> Option<&str> {
        self.error
            .as_ref()
            .filter(|(_, at)| now_ms.saturating_sub(*at) < ERROR_SHOW_MS)
            .map(|(t, _)| t.as_str())
    }

    fn live_note(&self, now_ms: u64) -> Option<&str> {
        self.room_note
            .as_ref()
            .filter(|(_, at)| now_ms.saturating_sub(*at) < NOTE_SHOW_MS)
            .map(|(t, _)| t.as_str())
    }

    /// The screens of this frame for `phase` (the shell's, which can be `Fatal` where the core's is not).
    pub fn screens(&mut self, core: &mut ClientCore, phase: &ClientPhase, now_ms: u64) -> Screens {
        let mut out = Screens::default();
        let rejected = core.rejected().cloned();
        let screen = net_screen(phase, rejected.as_ref());
        // The stage of "Drawing the map..." (`drawing_view`) belongs to the game screen: its clock runs on.
        if !matches!(screen, NetScreen::Loading | NetScreen::Play) {
            self.stage = None;
        }
        let conn = conn_view(core);
        match screen {
            NetScreen::Login => {
                let shown = match &rejected {
                    Some(RejectReason::BadPassword) if !self.password_typed => None,
                    other => other.as_ref(),
                };
                out.login = Some(login_view(core.nick(), &self.server, shown));
            }
            NetScreen::Loading => out.loading = Some(self.loading_view(core, phase, now_ms)),
            NetScreen::Lobby => {
                if let (Some(state), Some(choices)) = (core.lobby_state(), core.choices()) {
                    let mut view = lobby_list_view(
                        state,
                        Some(choices),
                        core.nick(),
                        conn.clone(),
                        self.live_error(now_ms),
                    );
                    // The question goes away with the seat.
                    self.confirm_give_up &= view.my_seat.is_some();
                    view.confirm_give_up = self.confirm_give_up;
                    out.lobby_list = Some(view);
                } else {
                    // The lists have not come yet: a plain "Loading".
                    out.loading = Some(self.loading_view(core, &ClientPhase::Connecting, now_ms));
                }
            }
            NetScreen::Room => {
                if let (Some(state), Some(choices)) = (core.lobby_state(), core.choices())
                    && let Some(room) = &state.room
                {
                    out.room = Some(room_view(
                        room,
                        state,
                        choices,
                        core.nick(),
                        self.live_note(now_ms),
                        self.live_error(now_ms),
                    ));
                }
            }
            // The game screens are the app's (`hud::net_play`); the status line and the player panel are LU4 (N4.11).
            NetScreen::Play => {}
            NetScreen::Notice(kind) => out.notice = Some(notice_view(&kind)),
            NetScreen::Fatal(text) => out.fatal = Some(text),
        }
        if !matches!(phase, ClientPhase::Starting) {
            self.joining = None;
        }
        out.conn = Some(conn);
        out
    }

    /// The last stage of the loading screen: the game is there (`Playing`), the map is not drawn yet (4.6).
    pub fn drawing_view(&mut self, core: &ClientCore, now_ms: u64) -> LoadingView {
        self.loading_view(core, &ClientPhase::Playing, now_ms)
    }

    fn loading_view(&mut self, core: &ClientCore, phase: &ClientPhase, now_ms: u64) -> LoadingView {
        let (stage, step) = match phase {
            ClientPhase::Connecting => (
                if core.token().is_some() {
                    "Signing in...".to_owned()
                } else {
                    "Connecting to the server...".to_owned()
                },
                1,
            ),
            ClientPhase::Starting => (
                match &self.joining {
                    Some(civ) => format!("Taking the seat of {civ}..."),
                    None => "Starting the game on the server...".to_owned(),
                },
                2,
            ),
            ClientPhase::Loading => ("Loading the world...".to_owned(), 3),
            _ => ("Drawing the map...".to_owned(), 4),
        };
        if self.stage.as_ref().is_none_or(|(s, _)| s != &stage) {
            self.stage = Some((stage.clone(), now_ms));
        }
        let since = self.stage.as_ref().map_or(now_ms, |(_, at)| *at);
        LoadingView {
            stage,
            detail: core.reconnect_status().map(|r| reconnect_text(&r)),
            step,
            steps: LOADING_STEPS,
            elapsed_s: now_ms.saturating_sub(since) as f32 / 1000.0,
        }
    }

    /// What the player's click does. The lobby commands go to the core; `NetEffect` is what only the shell
    /// can do.
    pub fn apply(&mut self, core: &mut ClientCore, action: UiAction, now_ms: u64) -> NetEffect {
        let send = |ui: &mut Self, core: &mut ClientCore, cmd: LobbyCmd| {
            if let Err(e) = core.lobby(cmd) {
                ui.error = Some((refusal_text(e).to_owned(), now_ms));
            }
        };
        match action {
            UiAction::Login { nick, password } => {
                self.password_typed = true;
                return NetEffect::Login { nick, password };
            }
            UiAction::SignOut => return NetEffect::SignOut,
            UiAction::Reload => return NetEffect::Reload,
            UiAction::Retry => core.retry(),
            UiAction::UseThisTab => core.use_this_tab(),
            UiAction::DismissNotice => core.dismiss_notice(),
            UiAction::CreateRoom => {
                let params = self.params();
                // v12: the settings of the last room this player started a game from are the defaults.
                let last = core.account().and_then(|a| a.last_room.clone());
                let made = core.choices().map(|c| {
                    default_settings(c, &params, core.nick()).map(|base| match &last {
                        Some(l) => settings_from_last_room(base, l, c),
                        None => base,
                    })
                });
                match made {
                    Some(Ok(settings)) => {
                        send(self, core, LobbyCmd::CreateRoom { settings });
                    }
                    Some(Err(why)) => self.error = Some((why, now_ms)),
                    None => self.error = Some(("The lists have not come yet".to_owned(), now_ms)),
                }
            }
            UiAction::QuickStart => self.quick_start(self.params()),
            // The B1 button of the playtest: the same as a quick start.
            UiAction::StartGame(choice) => self.quick_start(RoomParams {
                civ: choice.civ,
                ai: choice.ai_count,
                seed: choice.seed,
                ..self.params()
            }),
            UiAction::JoinRoom(room) => send(self, core, LobbyCmd::JoinRoom { room }),
            UiAction::LeaveRoom => {
                self.script = None;
                self.editor.reset();
                send(self, core, LobbyCmd::LeaveRoom);
            }
            UiAction::LoadSave(save) => send(self, core, LobbyCmd::LoadSave { save }),
            UiAction::GiveUpSeat => self.confirm_give_up = true,
            UiAction::ConfirmGiveUp(yes) => {
                self.confirm_give_up = false;
                if yes {
                    self.error = None;
                    send(self, core, LobbyCmd::LeaveSeat);
                }
            }
            UiAction::TakeSeat { game, player } => {
                self.joining = core
                    .lobby_state()
                    .and_then(|s| s.games.iter().find(|g| g.id == game))
                    .and_then(|g| g.seats.iter().find(|s| s.player == player))
                    .map(|s| s.civ_name.clone());
                send(
                    self,
                    core,
                    LobbyCmd::JoinGame {
                        game,
                        player,
                        basis: None,
                    },
                );
            }
            UiAction::RoomEdit(edit) => {
                let cmd = match (
                    core.lobby_state().and_then(|s| s.room.as_ref()),
                    core.choices(),
                ) {
                    (Some(room), Some(choices)) => Some(LobbyCmd::Settings(self.editor.edit(
                        &room.settings,
                        &edit,
                        choices,
                        now_ms,
                    ))),
                    _ => None,
                };
                if let Some(cmd) = cmd {
                    send(self, core, cmd);
                }
            }
            UiAction::SlotSet { index, kind, civ } => send(
                self,
                core,
                LobbyCmd::Slot {
                    index,
                    occupant: match kind {
                        SlotKind::Open => SlotOccupant::Open,
                        SlotKind::Ai => SlotOccupant::Ai,
                        SlotKind::Closed => SlotOccupant::Closed,
                    },
                    civ: civ.map_or(CivPick::Random, CivPick::Civ),
                },
            ),
            UiAction::PickCiv(civ) => send(
                self,
                core,
                LobbyCmd::PickCiv {
                    civ: civ.map_or(CivPick::Random, CivPick::Civ),
                },
            ),
            UiAction::TakeSlot(index) => send(self, core, LobbyCmd::TakeSlot { index }),
            UiAction::Ready(v) => send(self, core, LobbyCmd::Ready(v)),
            UiAction::Start => send(self, core, LobbyCmd::Start),
            // The game's own actions are the app's (`is_game_action`).
            _ => {}
        }
        NetEffect::None
    }
}

fn refusal_text(e: OrderRefused) -> &'static str {
    match e {
        OrderRefused::NoConnection => "No connection",
        OrderRefused::NoGame => "Not now",
        OrderRefused::TooMany => "Too many requests, wait a moment",
        OrderRefused::TurnChanged => "The turn has changed",
    }
}

/// For the log line of a phase change.
pub fn phase_name(p: &ClientPhase) -> &'static str {
    match p {
        ClientPhase::Connecting => "connecting",
        ClientPhase::NeedLogin => "login",
        ClientPhase::Lobby => "lobby",
        ClientPhase::Room => "room",
        ClientPhase::Starting => "starting",
        ClientPhase::Loading => "loading",
        ClientPhase::Playing => "play",
        ClientPhase::Notice(_) => "notice",
        ClientPhase::Fatal(_) => "fatal",
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_civilization_tip_row_keeps_the_parts_apart() {
        let c = CivChoice {
            id: "CIVILIZATION_ROME".into(),
            name: "Rome".into(),
            leader: "Augustus Caesar".into(),
            traits: vec!["Glory of Rome: [ICON_PRODUCTION] bonus".into()],
            unique: vec!["Unit: Legion".into()],
        };
        let row = civ_tip_row(&c).unwrap();
        assert_eq!(row.key, "CIVILIZATION_ROME");
        assert_eq!(row.leader, "Augustus Caesar");
        assert_eq!(row.traits, ["Glory of Rome: [ICON_PRODUCTION] bonus"]);
        assert_eq!(row.unique, ["Unit: Legion"]);
        let none = CivChoice {
            leader: String::new(),
            traits: vec![],
            unique: vec![],
            ..c
        };
        assert!(civ_tip_row(&none).is_none(), "no tip, no entry in civ_tips");
    }

    #[test]
    fn a_civilization_tip_lists_leader_traits_and_unique_items() {
        let c = CivChoice {
            id: "CIVILIZATION_ROME".into(),
            name: "Rome".into(),
            leader: "Augustus Caesar".into(),
            traits: vec!["Glory of Rome: bonus".into()],
            unique: vec!["Unit: Legion".into(), "Building: Baths".into()],
        };
        assert_eq!(
            civ_tip(&c),
            "Augustus Caesar
Glory of Rome: bonus
Unit: Legion
Building: Baths"
        );
        let none = CivChoice {
            leader: String::new(),
            traits: vec![],
            unique: vec![],
            ..c
        };
        assert_eq!(civ_tip(&none), "", "no tip, no entry in civ_tips");
    }
    use super::*;
    use client_core::{NetConfig, Sock, SocketCmd};
    use protocol::{
        CivChoice, ClientMsg, GameSummary, LobbyChoice, OptionChoice, RoomSlot, RoomSummary,
        SaveSummary, SeatSummary, ServerMsg, Session, Token, WorldSizeChoice,
    };
    use wire::{Encoder, Limits};

    fn lc(t: &str) -> LobbyChoice {
        LobbyChoice {
            type_name: t.into(),
            name: t.to_lowercase(),
        }
    }

    fn choices() -> LobbyChoices {
        let size = |id: &str, name: &str, def: u8, max: u8, max_minor: u8| WorldSizeChoice {
            id: id.into(),
            name: name.into(),
            width: 40,
            height: 24,
            default_players: def,
            max_players: max,
            default_minor: 1,
            max_minor,
        };
        LobbyChoices {
            hash: 1,
            civs: vec![CivChoice {
                id: "CIVILIZATION_ROME".into(),
                name: "Rome".into(),
                leader: "Caesar".into(),
                traits: vec![],
                unique: vec![],
            }],
            handicaps: vec![lc("HANDICAP_AI"), lc("HANDICAP_PRINCE")],
            world_sizes: vec![
                size("WORLDSIZE_DUEL", "Duel", 2, 4, 3),
                size("WORLDSIZE_HUGE", "Huge", 4, 8, 12),
            ],
            map_scripts: vec![lc("Pangaea")],
            speeds: vec![lc("GAMESPEED_STANDARD")],
            eras: vec![lc("ERA_ANCIENT")],
            timers: vec![lc("FAST")],
            options: vec![
                OptionChoice {
                    key: "GAMEOPTION_NO_BARBARIANS".into(),
                    name: "No barbarians".into(),
                    default: 0,
                    values: vec![],
                    help: String::new(),
                    group: protocol::OptionGroup::General,
                },
                OptionChoice {
                    key: "QUICK_COMBAT".into(),
                    name: "Quick combat".into(),
                    default: 0,
                    values: vec![],
                    help: String::new(),
                    group: protocol::OptionGroup::General,
                },
            ],
            ..LobbyChoices::default()
        }
    }

    fn settings() -> GameSettings {
        default_settings(&choices(), &RoomParams::default(), "alice").unwrap()
    }

    // ---- the settings editor

    #[test]
    fn two_quick_edits_do_not_undo_each_other() {
        let c = choices();
        let snapshot = settings();
        let mut ed = SettingsEditor::default();
        let first = ed.edit(&snapshot, &RoomEdit::Speed("X".into()), &c, 1_000);
        assert_eq!(first.speed, "X");
        // The snapshot has not changed yet (the echo is on its way): the second edit builds on the first.
        let second = ed.edit(&snapshot, &RoomEdit::Era("Y".into()), &c, 1_100);
        assert_eq!(
            (second.speed.as_str(), second.start_era.as_str()),
            ("X", "Y")
        );
        // The server's echo of the second one: the base is the snapshot again.
        assert_eq!(ed.base(&second, 1_200), second);
        let third = ed.edit(&second, &RoomEdit::Name("N".into()), &c, 1_300);
        assert_eq!(third.name, "N");
    }

    #[test]
    fn the_base_returns_to_the_snapshot_after_two_seconds() {
        let c = choices();
        let snapshot = settings();
        let mut ed = SettingsEditor::default();
        ed.edit(&snapshot, &RoomEdit::Speed("X".into()), &c, 0);
        assert_eq!(ed.base(&snapshot, SETTINGS_BASE_MS - 1).speed, "X");
        // The server never took it: the snapshot is the truth again.
        assert_eq!(ed.base(&snapshot, SETTINGS_BASE_MS), snapshot);
    }

    #[test]
    fn a_smaller_map_trims_the_city_states_and_defaults_leave_the_option_list() {
        let c = choices();
        let mut s = settings();
        s.world_size = "WORLDSIZE_HUGE".into();
        s.minor_civs = 10;
        apply_edit(&mut s, &RoomEdit::WorldSize("WORLDSIZE_DUEL".into()), &c);
        assert_eq!(
            s.minor_civs, 3,
            "the server would refuse more than the map takes"
        );
        apply_edit(
            &mut s,
            &RoomEdit::Option {
                key: "GAMEOPTION_NO_BARBARIANS".into(),
                value: 1,
            },
            &c,
        );
        assert_eq!(s.game_options, [("GAMEOPTION_NO_BARBARIANS".to_owned(), 1)]);
        apply_edit(
            &mut s,
            &RoomEdit::Option {
                key: "GAMEOPTION_NO_BARBARIANS".into(),
                value: 0,
            },
            &c,
        );
        assert!(s.game_options.is_empty(), "the default is not listed");
        apply_edit(&mut s, &RoomEdit::Timer(TimerChoice::Fixed(90)), &c);
        assert_eq!(s.timer, TimerMode::Fixed { seconds: 90 });
        apply_edit(
            &mut s,
            &RoomEdit::Timer(TimerChoice::Original("FAST".into())),
            &c,
        );
        assert_eq!(s.timer, TimerMode::Original { row: "FAST".into() });
        apply_edit(
            &mut s,
            &RoomEdit::TurnMode(TurnModeChoice::Simultaneous),
            &c,
        );
        assert_eq!(s.turn_mode, TurnMode::Simultaneous);
        apply_edit(&mut s, &RoomEdit::Seed(Some(9)), &c);
        assert_eq!(s.seed, Some(9));
    }

    /// `choices()` with the v8 lists: victories, two map scripts with options, grouped options, city-states.
    fn choices_v8() -> LobbyChoices {
        let opt =
            |id: &str, default: i32, values: &[&str], read_only: bool| protocol::MapOptionChoice {
                id: id.into(),
                name: format!("option {id}"),
                help: format!("help {id}"),
                default,
                values: values
                    .iter()
                    .enumerate()
                    .map(|(i, n)| (i as i32 + 1, (*n).to_owned()))
                    .collect(),
                read_only,
            };
        let mut c = choices();
        c.map_scripts = vec![lc("Pangaea"), lc("Continents")];
        c.victories = vec![lc("VICTORY_TIME"), lc("VICTORY_SPACE_RACE")];
        c.max_minor_civs = 24;
        c.map_options = vec![
            protocol::MapScriptOptions {
                script: "Pangaea".into(),
                options: vec![
                    opt("1", 2, &["Old", "Normal", "New"], false),
                    opt("2", 1, &[], true),
                ],
            },
            protocol::MapScriptOptions {
                script: "Continents".into(),
                options: vec![opt("1", 3, &["a", "b", "c", "d"], false)],
            },
        ];
        c.options.push(OptionChoice {
            key: "GAMEOPTION_AI_OPTIONS".into(),
            name: "AI options".into(),
            default: 0,
            values: vec![],
            help: "Opens the AI group".into(),
            group: protocol::OptionGroup::General,
        });
        c.options.push(OptionChoice {
            key: "GAMEOPTION_AI_X".into(),
            name: "AI x".into(),
            default: 0,
            values: vec![],
            help: String::new(),
            group: protocol::OptionGroup::Ai,
        });
        c
    }

    #[test]
    fn victories_max_turns_and_map_options_ride_in_the_settings() {
        let c = choices_v8();
        let mut s = default_settings(&c, &RoomParams::default(), "alice").unwrap();
        // The new room starts with the defaults of its script (the read only row is the script's own).
        assert_eq!(s.map_options, [("1".to_owned(), 2)]);
        let v = settings_view(&s, &c);
        assert!(
            v.victories.iter().all(|r| r.on),
            "an unlisted victory is on"
        );
        assert_eq!(v.max_turns, 0);
        assert_eq!(v.map_options.len(), 2);
        assert_eq!(v.map_options[0].value, 2);
        assert_eq!(v.map_options[0].help, "help 1");
        assert!(v.map_options[1].read_only);
        assert_eq!(v.size_info, "40 x 24, 2 players, 1 city-states");

        // A victory off is a (key, 0) entry; on again takes it out.
        apply_edit(
            &mut s,
            &RoomEdit::Victory {
                key: "VICTORY_TIME".into(),
                on: false,
            },
            &c,
        );
        assert_eq!(s.game_options, [("VICTORY_TIME".to_owned(), 0)]);
        assert!(!settings_view(&s, &c).victories[0].on);
        assert!(settings_view(&s, &c).victories[1].on);
        apply_edit(
            &mut s,
            &RoomEdit::Victory {
                key: "VICTORY_TIME".into(),
                on: true,
            },
            &c,
        );
        assert!(s.game_options.is_empty());
        // Not a victory key: ignored.
        apply_edit(
            &mut s,
            &RoomEdit::Victory {
                key: "GAMEOPTION_X".into(),
                on: false,
            },
            &c,
        );
        assert!(s.game_options.is_empty());

        // Max turns: 0 takes the entry out, a number is capped at the limit.
        apply_edit(&mut s, &RoomEdit::MaxTurns(300), &c);
        assert_eq!(s.game_options, [("MAX_TURNS".to_owned(), 300)]);
        assert_eq!(settings_view(&s, &c).max_turns, 300);
        apply_edit(&mut s, &RoomEdit::MaxTurns(u16::MAX), &c);
        assert_eq!(s.game_options, [("MAX_TURNS".to_owned(), 2000)]);
        apply_edit(&mut s, &RoomEdit::MaxTurns(0), &c);
        assert!(s.game_options.is_empty());

        // A map option of the script; an id the script lacks and a read only one are refused.
        apply_edit(
            &mut s,
            &RoomEdit::MapOption {
                id: "1".into(),
                value: 3,
            },
            &c,
        );
        assert_eq!(s.map_options, [("1".to_owned(), 3)]);
        apply_edit(
            &mut s,
            &RoomEdit::MapOption {
                id: "9".into(),
                value: 1,
            },
            &c,
        );
        apply_edit(
            &mut s,
            &RoomEdit::MapOption {
                id: "2".into(),
                value: 0,
            },
            &c,
        );
        assert_eq!(s.map_options, [("1".to_owned(), 3)]);
        // Another script: its own defaults, not the old values.
        apply_edit(&mut s, &RoomEdit::MapScript("Continents".into()), &c);
        assert_eq!(s.map_options, [("1".to_owned(), 3)]);
        apply_edit(
            &mut s,
            &RoomEdit::MapOption {
                id: "1".into(),
                value: 1,
            },
            &c,
        );
        apply_edit(&mut s, &RoomEdit::MapScript("Pangaea".into()), &c);
        assert_eq!(s.map_options, [("1".to_owned(), 2)]);

        // City-states: up to what the size takes.
        apply_edit(&mut s, &RoomEdit::MinorCivs(20), &c);
        assert_eq!(s.minor_civs, 3, "Duel takes 3");
    }

    #[test]
    fn the_options_carry_their_help_and_group_and_the_ai_choices_are_listed() {
        let c = choices_v8();
        let mut s = default_settings(&c, &RoomParams::default(), "alice").unwrap();
        let v = settings_view(&s, &c);
        let ai = v
            .options
            .iter()
            .find(|o| o.key == "GAMEOPTION_AI_X")
            .unwrap();
        assert_eq!(ai.group, OptionGroup::Ai);
        assert!(!v.option_shown(ai), "the AI group waits for its switch");
        let switch = v
            .options
            .iter()
            .find(|o| o.key == "GAMEOPTION_AI_OPTIONS")
            .unwrap();
        assert_eq!(switch.help, "Opens the AI group");
        apply_edit(
            &mut s,
            &RoomEdit::Option {
                key: "GAMEOPTION_AI_OPTIONS".into(),
                value: 1,
            },
            &c,
        );
        let v = settings_view(&s, &c);
        let ai = v
            .options
            .iter()
            .find(|o| o.key == "GAMEOPTION_AI_X")
            .unwrap();
        assert!(v.option_shown(ai));

        assert_eq!(v.auto_choices.len(), 11);
        assert!(v.auto_choices.iter().all(|a| a.on), "the quick defaults");
        apply_edit(
            &mut s,
            &RoomEdit::AutoChoice {
                key: "religion".into(),
                on: false,
            },
            &c,
        );
        assert!(!s.auto_choices.religion && s.auto_choices.research);
        let v = settings_view(&s, &c);
        assert!(
            !v.auto_choices
                .iter()
                .find(|a| a.key == "religion")
                .unwrap()
                .on
        );
    }

    #[test]
    fn the_views_of_the_page_are_json() {
        let c = choices_v8();
        let s = default_settings(&c, &RoomParams::default(), "alice").unwrap();
        let text = serde_json::to_string(&settings_view(&s, &c)).unwrap();
        assert!(text.contains("\"max_turns\":0"));
        assert!(text.contains("\"victories\":[{\"key\":\"VICTORY_TIME\""));
        assert!(text.contains("\"group\":\"Ai\""));
        assert!(text.contains("\"timer\":\"Off\""));
    }

    // ---- view data

    fn room_state(slots: Vec<Occupant>) -> RoomState {
        RoomState {
            id: 5,
            name: "Friday".into(),
            host: "alice".into(),
            status: GameStatus::Lobby,
            settings: settings(),
            slots: slots
                .into_iter()
                .enumerate()
                .map(|(i, occupant)| RoomSlot {
                    index: i as u8,
                    occupant,
                    civ: CivPick::Random,
                    reserved_for: None,
                })
                .collect(),
            from_save: None,
            mod_plan: protocol::ModPlan::default(),
        }
    }

    fn human(nick: &str, ready: bool) -> Occupant {
        Occupant::Human {
            nick: nick.into(),
            ready,
        }
    }

    #[test]
    fn the_start_rule_says_what_is_missing() {
        // Anna and Bob are not ready.
        let r = room_state(vec![
            human("alice", false),
            human("Anna", false),
            human("Bob", true),
            human("Cy", false),
        ]);
        assert_eq!(why_not_start(&r).as_deref(), Some("Waiting for: Anna, Cy"));
        // The host needs no "Ready" of its own.
        let r = room_state(vec![human("alice", false), human("Anna", true)]);
        assert_eq!(why_not_start(&r), None);
        let r = room_state(vec![human("alice", false), Occupant::Closed]);
        assert_eq!(
            why_not_start(&r).as_deref(),
            Some("Need at least two civilizations")
        );
        let r = room_state(vec![human("alice", false), Occupant::Ai, Occupant::Open]);
        assert_eq!(
            why_not_start(&r).as_deref(),
            Some("Close or fill the open slots")
        );
        let r = room_state(vec![human("alice", false), Occupant::Ai]);
        assert_eq!(why_not_start(&r), None);
    }

    #[test]
    fn the_room_view_knows_the_host_slots_and_the_save() {
        let mut r = room_state(vec![
            human("alice", false),
            Occupant::Open,
            Occupant::Closed,
        ]);
        r.slots[1].reserved_for = Some("bob".into());
        r.slots[1].civ = CivPick::Civ("CIVILIZATION_ROME".into());
        r.from_save = Some("s1".into());
        let state = LobbyState {
            room: Some(r.clone()),
            saves: vec![SaveSummary {
                id: "s1".into(),
                name: "Autosave".into(),
                turn: 12,
                saved_at_ms: 1_760_000_000_000,
                seats: vec![
                    (0, "Egypt".into(), Some("alice".into())),
                    (1, "Rome".into(), Some("bob".into())),
                ],
                game: 5,
                host: "alice".into(),
            }],
            ..LobbyState::default()
        };
        let v = room_view(&r, &state, &choices(), "bob", None, None);
        assert!(!v.me_host && !v.has_slot, "bob has no slot yet");
        assert_eq!(
            v.from_save.as_deref(),
            Some("Loaded from Autosave, turn 12. Players take their own civilizations")
        );
        assert_eq!(v.slots[1].was.as_deref(), Some("bob"));
        assert!(v.slots[1].yours && !v.slots[0].yours);
        assert_eq!(v.slots[1].civ.as_deref(), Some("CIVILIZATION_ROME"));
        assert_eq!(v.saves[0].civs, "Egypt (alice), Rome (bob)");
        assert_eq!(v.saves[0].saved, "2025-10-09 08:53 UTC");
        assert_eq!((v.saves[0].game, v.saves[0].host.as_str()), (5, "alice"));
        // The difficulty list leaves out the AI's own level; options the server refuses are not offered.
        let keys: Vec<_> = v
            .settings
            .handicaps
            .iter()
            .map(|c| c.key.as_str())
            .collect();
        assert_eq!(keys, ["HANDICAP_PRINCE"]);
        assert_eq!(v.settings.options.len(), 1);
        assert_eq!(v.settings.max_minor, 3);
        let host = room_view(&r, &state, &choices(), "alice", Some("note"), Some("err"));
        assert!(host.me_host && host.has_slot);
        assert_eq!(
            (host.note.as_deref(), host.error.as_deref()),
            (Some("note"), Some("err"))
        );
    }

    #[test]
    fn the_online_list_puts_me_first_and_words_the_places() {
        let p = |nick: &str, place| protocol::OnlinePlayer {
            nick: nick.into(),
            place,
        };
        let state = LobbyState {
            online: vec![
                p("alice", PlayerPlace::Lobby),
                p(
                    "bob",
                    PlayerPlace::Room {
                        room: 3,
                        name: "Friday".into(),
                        host: true,
                        ready: false,
                    },
                ),
                p(
                    "Carl",
                    PlayerPlace::Game {
                        game: 9,
                        name: "Old".into(),
                        turn: 41,
                    },
                ),
            ],
            ..LobbyState::default()
        };
        let rows = online_rows(&state, "carl");
        let got: Vec<_> = rows
            .iter()
            .map(|r| (r.nick.as_str(), r.me, r.place.as_str(), r.status.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("Carl", true, "game", "Playing Old, turn 41"),
                ("alice", false, "lobby", "In the lobby"),
                ("bob", false, "room", "In room Friday"),
            ]
        );
        assert_eq!((rows[2].ready, rows[2].host), (Some(false), true));
        assert_eq!((rows[0].ready, rows[0].host), (None, false));
        let v = lobby_list_view(&state, None, "alice", ConnView::default(), None);
        assert_eq!(v.online[0].nick, "alice");
        assert_eq!(v.online.len(), 3);
    }

    #[test]
    fn the_lobby_list_shows_rooms_games_and_the_seats_to_take() {
        let state = LobbyState {
            rooms: vec![
                RoomSummary {
                    id: 3,
                    name: "Friday".into(),
                    host: "alice".into(),
                    humans: 1,
                    slots: 4,
                    world_size: "WORLDSIZE_DUEL".into(),
                    map_script: "Pangaea".into(),
                    status: GameStatus::Lobby,
                },
                RoomSummary {
                    id: 4,
                    name: "Busy".into(),
                    host: "bob".into(),
                    humans: 2,
                    slots: 2,
                    world_size: "WORLDSIZE_UNKNOWN".into(),
                    map_script: "Assets/Maps/X/Foo.lua".into(),
                    status: GameStatus::Starting,
                },
            ],
            games: vec![GameSummary {
                id: 9,
                name: "Old".into(),
                turn: 41,
                world_size: "WORLDSIZE_HUGE".into(),
                host: "alice".into(),
                status: GameStatus::Hibernated,
                seats: vec![
                    SeatSummary {
                        player: 0,
                        civ_name: "Egypt".into(),
                        control: SeatControl::Human {
                            nick: "carl".into(),
                        },
                        online: true,
                        takeable: false,
                    },
                    SeatSummary {
                        player: 1,
                        civ_name: "Rome".into(),
                        control: SeatControl::Ai {
                            kicked: Some("Bob".into()),
                        },
                        online: false,
                        takeable: true,
                    },
                ],
            }],
            ..LobbyState::default()
        };
        let v = lobby_list_view(
            &state,
            Some(&choices()),
            "bob",
            ConnView::default(),
            Some("no"),
        );
        assert_eq!(v.rooms[0].players, "1/4");
        assert_eq!(v.rooms[0].map, "Duel, pangaea");
        assert!(!v.rooms[0].starting && v.rooms[1].starting);
        assert_eq!(
            v.rooms[1].map, "Unknown, Foo",
            "an unknown id falls back to a readable form"
        );
        assert_eq!(v.games[0].status, "sleeping (joining wakes it)");
        assert_eq!(v.games[0].map, "Huge");
        let seats = &v.games[0].seats;
        assert!(!seats[0].yours && !seats[0].takeable && seats[0].who == "carl");
        assert!(
            seats[1].yours && seats[1].takeable && seats[1].who == "AI",
            "the seat the kick took from bob"
        );
        assert_eq!(v.error.as_deref(), Some("no"));
        assert_eq!(v.nick, "bob");
    }

    #[test]
    fn refusals_have_their_text_and_button() {
        let t = |r: RejectReason| reject_text(&r);
        assert_eq!(
            t(RejectReason::BadPassword),
            Some(("Wrong nick or password.", None))
        );
        assert_eq!(
            t(RejectReason::NickTaken),
            Some(("This nick is taken. Choose another one.", None))
        );
        assert_eq!(
            t(RejectReason::VersionMismatch { server: 9 }),
            Some((
                "The server was updated. Reload the page.",
                Some(ErrorButton::Reload)
            ))
        );
        assert_eq!(
            t(RejectReason::Full),
            Some((
                "The server is full. Try again later.",
                Some(ErrorButton::Retry)
            ))
        );
        assert!(
            t(RejectReason::BadNick)
                .unwrap()
                .0
                .starts_with("This nick is not allowed.")
        );
        assert_eq!(t(RejectReason::Unsupported { kind: "x".into() }), None);
        let v = login_view("alice", "ws://h/ws", Some(&RejectReason::BadPassword));
        assert_eq!(v.error.as_deref(), Some("Wrong nick or password."));
        assert_eq!(login_view("a", "s", None).error, None);
    }

    #[test]
    fn each_notice_has_its_text_and_button() {
        let n = notice_view(&NoticeKind::Kicked);
        assert!(
            n.text
                .starts_with("You were removed from the game by the host.")
        );
        assert_eq!(
            (n.button.as_str(), n.on_button),
            ("OK", NoticeButton::Dismiss)
        );
        let n = notice_view(&NoticeKind::Replaced);
        assert_eq!(n.text, "You signed in on another device or tab.");
        assert_eq!(
            (n.button.as_str(), n.on_button),
            ("Use this tab", NoticeButton::UseThisTab)
        );
        let n = notice_view(&NoticeKind::GameStopped {
            why: "the server restarts".into(),
        });
        assert_eq!(n.text, "The game was stopped: the server restarts.");
        assert_eq!(notice_view(&NoticeKind::GameOver).button, "Back to lobby");
    }

    #[test]
    fn the_reconnect_line_counts_down() {
        let r = ReconnectStatus {
            attempt: 2,
            next_in_ms: 2_100,
            probing: false,
        };
        assert_eq!(reconnect_text(&r), "Reconnecting... attempt 2, next in 3 s");
        let p = ReconnectStatus { probing: true, ..r };
        assert_eq!(reconnect_text(&p), "Reconnecting... attempt 2, trying now");
    }

    // ---- a core on a scripted line

    const SOCK: Sock = Sock(1);

    /// A `ClientCore` logged in on one socket, and a double of the lobby of the server (rooms only).
    struct Rig {
        core: ClientCore,
        ui: NetUi,
        now: u64,
        room: Option<RoomState>,
        /// What the client sent, decoded.
        sent: Vec<ClientMsg>,
    }

    impl Rig {
        fn new() -> Self {
            let mut core = ClientCore::new(NetConfig::new("test", "alice"));
            core.connect(0);
            let Some(SocketCmd::Open { sock, .. }) = core.poll_socket_cmd() else {
                panic!("the core opens a socket");
            };
            assert_eq!(sock, SOCK);
            core.on_connected(0, sock);
            let mut rig = Rig {
                core,
                ui: NetUi::new("ws://127.0.0.1:8080/ws"),
                now: 0,
                room: None,
                sent: Vec::new(),
            };
            rig.push(ServerMsg::Welcome {
                session: 1,
                build: "srv".into(),
            });
            rig.push(ServerMsg::LoggedIn(Session {
                token: Some(Token([1; 16])),
                nick: "alice".into(),
            }));
            rig.push(ServerMsg::Choices(choices()));
            rig.push_lobby();
            rig.settle();
            rig
        }

        fn push(&mut self, msg: ServerMsg) {
            let bytes = Encoder::default().frame_msg(&msg).expect("encode");
            self.core.on_bytes(self.now, SOCK, &bytes);
        }

        fn push_lobby(&mut self) {
            let state = LobbyState {
                room: self.room.clone(),
                ..LobbyState::default()
            };
            self.push(ServerMsg::LobbyState(state));
        }

        /// The double answers what the client sent, until nothing is left.
        fn settle(&mut self) {
            for _ in 0..20 {
                let mut any = false;
                while let Some((_, bytes)) = self.core.poll_outgoing() {
                    any = true;
                    let msg: ClientMsg =
                        wire::decode(&bytes, &Limits::GATEWAY).expect("a client frame");
                    if let ClientMsg::Lobby(cmd) = &msg {
                        self.serve(cmd.clone());
                    }
                    self.sent.push(msg);
                }
                while let Some(ev) = self.core.poll_event() {
                    self.ui.on_event(&ev, self.now);
                }
                if !any {
                    return;
                }
            }
        }

        /// The room rules of the server, as far as the tests need them.
        fn serve(&mut self, cmd: LobbyCmd) {
            match cmd {
                LobbyCmd::CreateRoom { settings } => {
                    let size = choices()
                        .world_sizes
                        .into_iter()
                        .find(|w| w.id == settings.world_size)
                        .expect("a known size");
                    let slots = (0..size.max_players)
                        .map(|i| RoomSlot {
                            index: i,
                            occupant: match i {
                                0 => human("alice", false),
                                i if i < size.default_players => Occupant::Open,
                                _ => Occupant::Closed,
                            },
                            civ: CivPick::Random,
                            reserved_for: None,
                        })
                        .collect();
                    self.room = Some(RoomState {
                        id: 1,
                        name: settings.name.clone(),
                        host: "alice".into(),
                        status: GameStatus::Lobby,
                        settings,
                        slots,
                        from_save: None,
                        mod_plan: protocol::ModPlan::default(),
                    });
                }
                LobbyCmd::Settings(s) => self.room.as_mut().unwrap().settings = s,
                LobbyCmd::Slot {
                    index,
                    occupant,
                    civ,
                } => {
                    let r = self.room.as_mut().unwrap();
                    r.slots[usize::from(index)].occupant = match occupant {
                        SlotOccupant::Open => Occupant::Open,
                        SlotOccupant::Ai => Occupant::Ai,
                        SlotOccupant::Closed => Occupant::Closed,
                    };
                    r.slots[usize::from(index)].civ = civ;
                    unready(r);
                }
                LobbyCmd::PickCiv { civ } => {
                    let r = self.room.as_mut().unwrap();
                    r.slots[0].civ = civ;
                    unready(r);
                }
                LobbyCmd::Ready(v) => {
                    let r = self.room.as_mut().unwrap();
                    r.slots[0].occupant = human("alice", v);
                }
                // Start and the rest change nothing the double shows.
                _ => return,
            }
            self.push_lobby();
        }

        fn frame(&mut self) {
            self.now += 50;
            self.ui.tick(&mut self.core, self.now);
            self.settle();
        }

        fn lobby_cmds(&self) -> Vec<&LobbyCmd> {
            self.sent
                .iter()
                .filter_map(|m| match m {
                    ClientMsg::Lobby(c) => Some(c),
                    _ => None,
                })
                .collect()
        }
    }

    fn unready(r: &mut RoomState) {
        for s in &mut r.slots {
            if let Occupant::Human { ready, .. } = &mut s.occupant {
                *ready = false;
            }
        }
    }

    #[test]
    fn a_core_in_the_lobby_shows_the_lobby_screen() {
        let mut rig = Rig::new();
        assert_eq!(rig.core.phase(), ClientPhase::Lobby);
        let phase = rig.core.phase();
        let s = rig.ui.screens(&mut rig.core, &phase, 100);
        assert!(s.lobby_list.is_some() && s.room.is_none() && s.login.is_none());
        assert_eq!(s.lobby_list.unwrap().nick, "alice");
        assert!(
            s.conn.is_some_and(|c| c.status.is_none()),
            "no strip while the line is up"
        );
    }

    #[test]
    fn create_room_sends_default_settings_and_the_room_screen_follows() {
        let mut rig = Rig::new();
        let now = rig.now;
        assert_eq!(
            rig.ui.apply(&mut rig.core, UiAction::CreateRoom, now),
            NetEffect::None
        );
        rig.settle();
        let Some(LobbyCmd::CreateRoom { settings }) =
            rig.lobby_cmds().first().map(|c| (*c).clone())
        else {
            panic!("CreateRoom was sent");
        };
        assert_eq!(settings.world_size, "WORLDSIZE_DUEL");
        assert_eq!(rig.core.phase(), ClientPhase::Room);
        let phase = rig.core.phase();
        let s = rig.ui.screens(&mut rig.core, &phase, 200);
        let room = s.room.expect("the room screen");
        assert!(room.me_host && room.has_slot);
        assert!(!room.can_start, "one civilization only");
        assert_eq!(
            room.why_not.as_deref(),
            Some("Need at least two civilizations")
        );
        assert!(s.lobby_list.is_none());
        // Without `set_manual_choices` (the interim UI) the core makes the choices of the human.
        assert_eq!(settings.auto_choices, protocol::AutoChoices::default());
    }

    #[test]
    fn a_new_room_starts_from_the_last_room_of_the_account() {
        let mut rig = Rig::new();
        let mut last = settings();
        last.world_size = "WORLDSIZE_HUGE".into();
        last.minor_civs = 9;
        last.turn_mode = TurnMode::Simultaneous;
        last.name = "yesterday".into();
        rig.push(ServerMsg::Account(protocol::AccountData {
            version: protocol::ACCOUNT_VERSION,
            prefs: vec![],
            last_room: Some(last),
        }));
        let now = rig.now;
        rig.ui.apply(&mut rig.core, UiAction::CreateRoom, now);
        rig.settle();
        let Some(LobbyCmd::CreateRoom { settings }) =
            rig.lobby_cmds().first().map(|c| (*c).clone())
        else {
            panic!("CreateRoom was sent");
        };
        assert_eq!(settings.world_size, "WORLDSIZE_HUGE");
        assert_eq!(settings.minor_civs, 9);
        assert_eq!(settings.turn_mode, TurnMode::Simultaneous);
        assert_ne!(settings.name, "yesterday", "the name is the default's");
    }

    #[test]
    fn a_room_of_the_original_ui_leaves_the_choices_to_the_human() {
        // Live report 2026-10-30: research and policies were chosen by the core and END TURN went on, the
        // Great Prophet founded a religion with no popup: the lobby's rooms had `auto_choices` all on.
        let mut rig = Rig::new();
        rig.ui.set_manual_choices(true);
        let now = rig.now;
        rig.ui.apply(&mut rig.core, UiAction::CreateRoom, now);
        rig.settle();
        let Some(LobbyCmd::CreateRoom { settings }) =
            rig.lobby_cmds().first().map(|c| (*c).clone())
        else {
            panic!("CreateRoom was sent");
        };
        let a = settings.auto_choices;
        assert!(
            ![
                a.research,
                a.policy,
                a.ideology,
                a.religion,
                a.promotion,
                a.great_person,
                a.captured_city,
                a.league,
                a.archaeology,
                a.goody_hut,
                a.steal_tech
            ]
            .contains(&true),
            "{a:?}"
        );
        // The quick start of that lobby too.
        assert!(rig.ui.params().manual_choices);
    }

    #[test]
    fn edits_go_out_on_the_last_sent_settings_and_slot_commands_carry_the_civ() {
        let mut rig = Rig::new();
        let now = rig.now;
        rig.ui.apply(&mut rig.core, UiAction::CreateRoom, now);
        rig.settle();
        // Two edits in one go, before the server echoes the first.
        rig.ui.apply(
            &mut rig.core,
            UiAction::RoomEdit(RoomEdit::MinorCivs(2)),
            500,
        );
        rig.ui.apply(
            &mut rig.core,
            UiAction::RoomEdit(RoomEdit::Name("Two".into())),
            520,
        );
        // The double has not answered yet: drain the client's frames by hand.
        let mut settings = Vec::new();
        while let Some((_, bytes)) = rig.core.poll_outgoing() {
            if let ClientMsg::Lobby(LobbyCmd::Settings(s)) =
                wire::decode::<ClientMsg>(&bytes, &Limits::GATEWAY).unwrap()
            {
                settings.push(s);
            }
        }
        assert_eq!(settings.len(), 2);
        assert_eq!(settings[0].minor_civs, 2);
        assert_eq!(
            (settings[1].minor_civs, settings[1].name.as_str()),
            (2, "Two"),
            "the second edit kept the first"
        );
        // A slot command and a civ pick.
        rig.ui.apply(
            &mut rig.core,
            UiAction::SlotSet {
                index: 1,
                kind: SlotKind::Ai,
                civ: Some("CIVILIZATION_ROME".into()),
            },
            600,
        );
        rig.ui.apply(&mut rig.core, UiAction::PickCiv(None), 600);
        rig.ui.apply(&mut rig.core, UiAction::Ready(true), 600);
        let mut cmds = Vec::new();
        while let Some((_, bytes)) = rig.core.poll_outgoing() {
            if let ClientMsg::Lobby(c) =
                wire::decode::<ClientMsg>(&bytes, &Limits::GATEWAY).unwrap()
            {
                cmds.push(c);
            }
        }
        assert_eq!(
            cmds,
            [
                LobbyCmd::Slot {
                    index: 1,
                    occupant: SlotOccupant::Ai,
                    civ: CivPick::Civ("CIVILIZATION_ROME".into())
                },
                LobbyCmd::PickCiv {
                    civ: CivPick::Random
                },
                LobbyCmd::Ready(true),
            ]
        );
    }

    #[test]
    fn quick_start_walks_the_lobby_to_the_start() {
        let mut rig = Rig::new();
        rig.ui.quick_start(RoomParams::default());
        assert!(rig.ui.quick_starting());
        for _ in 0..40 {
            rig.frame();
            if rig.core.phase() == ClientPhase::Starting {
                break;
            }
        }
        assert_eq!(
            rig.core.phase(),
            ClientPhase::Starting,
            "the Start went out"
        );
        let kinds: Vec<&str> = rig
            .lobby_cmds()
            .iter()
            .map(|c| match c {
                LobbyCmd::CreateRoom { .. } => "create",
                LobbyCmd::Slot {
                    occupant: SlotOccupant::Ai,
                    ..
                } => "ai",
                LobbyCmd::Ready(true) => "ready",
                LobbyCmd::Start => "start",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["create", "ai", "ready", "start"]);
        rig.frame();
        assert!(!rig.ui.quick_starting(), "the script is done");
        // The screen says so.
        let phase = rig.core.phase();
        let s = rig.ui.screens(&mut rig.core, &phase, rig.now);
        let l = s.loading.expect("the loading screen");
        assert_eq!(
            (l.stage.as_str(), l.step, l.steps),
            ("Starting the game on the server...", 2, 5)
        );
    }

    #[test]
    fn a_start_that_never_attaches_ends_in_an_error_and_the_lobby() {
        let mut rig = Rig::new();
        rig.ui.quick_start(RoomParams::default());
        for _ in 0..40 {
            if rig.core.phase() == ClientPhase::Starting {
                break;
            }
            rig.frame();
        }
        assert_eq!(
            rig.core.phase(),
            ClientPhase::Starting,
            "the Start went out"
        );
        rig.frame();
        // Nothing answers: right before the limit it is still waiting ...
        rig.now += START_TIMEOUT_MS - 1_000;
        rig.frame();
        assert_eq!(rig.core.phase(), ClientPhase::Starting);
        // ... and past it the player is told and sees the room again.
        rig.now += 2_000;
        rig.frame();
        let phase = rig.core.phase();
        assert_eq!(phase, ClientPhase::Room);
        let s = rig.ui.screens(&mut rig.core, &phase, rig.now + 10);
        let shown = s.room.and_then(|r| r.error);
        assert!(
            shown.is_some_and(|e| e.contains("did not start")),
            "the error is on the room window"
        );
    }

    #[test]
    fn loading_a_save_shows_the_room_not_the_starting_screen() {
        let mut rig = Rig::new();
        rig.frame();
        assert_eq!(
            rig.core.lobby(LobbyCmd::LoadSave { save: "s1".into() }),
            Ok(())
        );
        assert_ne!(rig.core.phase(), ClientPhase::Starting);
    }

    #[test]
    fn a_refusal_stops_quick_start_and_shows_under_the_lobby() {
        let mut rig = Rig::new();
        rig.ui.quick_start(RoomParams::default());
        rig.frame();
        rig.push(ServerMsg::LobbyError {
            text: "too many rooms".into(),
        });
        rig.settle();
        rig.frame();
        assert!(!rig.ui.quick_starting());
        let phase = ClientPhase::Lobby;
        let s = rig.ui.screens(&mut rig.core, &phase, rig.now + 10);
        // The room of the double may exist already; the refusal is on whichever screen is up.
        let shown = s
            .lobby_list
            .and_then(|l| l.error)
            .or_else(|| s.room.and_then(|r| r.error));
        assert_eq!(shown.as_deref(), Some("too many rooms"));
        // ... for 8 seconds.
        let s = rig
            .ui
            .screens(&mut rig.core, &phase, rig.now + ERROR_SHOW_MS + 100);
        assert!(s.lobby_list.is_none_or(|l| l.error.is_none()) || s.room.is_some());
    }

    #[test]
    fn taking_a_seat_says_whose_seat_and_sends_join_game() {
        let mut rig = Rig::new();
        rig.push(ServerMsg::LobbyState(LobbyState {
            games: vec![GameSummary {
                id: 9,
                name: "Old".into(),
                turn: 41,
                world_size: "WORLDSIZE_DUEL".into(),
                host: "bob".into(),
                status: GameStatus::Running,
                seats: vec![SeatSummary {
                    player: 1,
                    civ_name: "Rome".into(),
                    control: SeatControl::Ai { kicked: None },
                    online: false,
                    takeable: true,
                }],
            }],
            ..LobbyState::default()
        }));
        let now = rig.now;
        rig.ui.apply(
            &mut rig.core,
            UiAction::TakeSeat { game: 9, player: 1 },
            now,
        );
        rig.settle();
        assert!(matches!(
            rig.lobby_cmds().last(),
            Some(LobbyCmd::JoinGame {
                game: 9,
                player: 1,
                ..
            })
        ));
        assert_eq!(rig.core.phase(), ClientPhase::Starting);
        let phase = rig.core.phase();
        let s = rig.ui.screens(&mut rig.core, &phase, 10);
        assert_eq!(s.loading.unwrap().stage, "Taking the seat of Rome...");
        // The server refuses: the line goes back to the lobby and the stage forgets the seat.
        rig.push(ServerMsg::LobbyError {
            text: "the seat is taken".into(),
        });
        rig.settle();
        assert_eq!(rig.core.phase(), ClientPhase::Lobby);
    }

    #[test]
    fn a_seat_in_a_game_is_given_up_after_a_question() {
        let mut rig = Rig::new();
        rig.push(ServerMsg::LobbyState(LobbyState {
            games: vec![GameSummary {
                id: 9,
                name: "Old".into(),
                turn: 41,
                world_size: "WORLDSIZE_DUEL".into(),
                host: "alice".into(),
                status: GameStatus::Running,
                seats: vec![SeatSummary {
                    player: 0,
                    civ_name: "Egypt".into(),
                    control: SeatControl::Human {
                        nick: "alice".into(),
                    },
                    online: false,
                    takeable: true,
                }],
            }],
            ..LobbyState::default()
        }));
        rig.settle();
        let phase = ClientPhase::Lobby;
        let seat = |ui: &mut NetUi, core: &mut ClientCore| {
            ui.screens(core, &phase, 10).lobby_list.unwrap()
        };
        let v = seat(&mut rig.ui, &mut rig.core);
        let my = v.my_seat.expect("the seat is on the lobby window");
        assert_eq!(
            (my.game, my.player, my.turn, my.can_return),
            (9, 0, 41, true)
        );
        assert!(!v.confirm_give_up);
        // The button only asks; nothing is sent.
        let before = rig.lobby_cmds().len();
        rig.ui.apply(&mut rig.core, UiAction::GiveUpSeat, rig.now);
        assert!(seat(&mut rig.ui, &mut rig.core).confirm_give_up);
        rig.settle();
        assert_eq!(rig.lobby_cmds().len(), before);
        // "Keep the seat" closes the question, "Yes" sends LeaveSeat.
        rig.ui
            .apply(&mut rig.core, UiAction::ConfirmGiveUp(false), rig.now);
        assert!(!seat(&mut rig.ui, &mut rig.core).confirm_give_up);
        rig.settle();
        assert_eq!(rig.lobby_cmds().len(), before);
        rig.ui.apply(&mut rig.core, UiAction::GiveUpSeat, rig.now);
        rig.ui
            .apply(&mut rig.core, UiAction::ConfirmGiveUp(true), rig.now);
        rig.settle();
        assert!(matches!(rig.lobby_cmds().last(), Some(LobbyCmd::LeaveSeat)));
        assert!(!seat(&mut rig.ui, &mut rig.core).confirm_give_up);
        // The seat is gone from the next state: no line.
        rig.push(ServerMsg::LobbyState(LobbyState::default()));
        rig.settle();
        assert!(seat(&mut rig.ui, &mut rig.core).my_seat.is_none());
    }

    #[test]
    fn loading_stages_follow_the_phase() {
        let mut rig = Rig::new();
        let l = |ui: &mut NetUi, core: &mut ClientCore, p: ClientPhase, now: u64| {
            ui.screens(core, &p, now).loading.unwrap()
        };
        let a = l(&mut rig.ui, &mut rig.core, ClientPhase::Loading, 0);
        assert_eq!((a.stage.as_str(), a.step), ("Loading the world...", 3));
        let b = l(&mut rig.ui, &mut rig.core, ClientPhase::Loading, 2_000);
        assert_eq!(
            b.elapsed_s, 2.0,
            "the seconds count from the first frame of the stage"
        );
        let c = l(&mut rig.ui, &mut rig.core, ClientPhase::Connecting, 3_000);
        assert_eq!(
            (c.stage.as_str(), c.step, c.elapsed_s),
            ("Signing in...", 1, 0.0)
        );
    }

    #[test]
    fn the_first_refusal_is_not_called_a_wrong_password() {
        // A page without a token logs in with no password at all; the server says no. The window opens
        // without a red line: the player has typed nothing yet.
        let mut core = ClientCore::new(NetConfig::new("test", "alice"));
        core.connect(0);
        let Some(SocketCmd::Open { sock, .. }) = core.poll_socket_cmd() else {
            panic!("a socket");
        };
        core.on_connected(0, sock);
        for msg in [
            ServerMsg::Welcome {
                session: 1,
                build: "srv".into(),
            },
            ServerMsg::Reject(RejectReason::BadPassword),
        ] {
            let bytes = Encoder::default().frame_msg(&msg).expect("encode");
            core.on_bytes(0, sock, &bytes);
        }
        assert_eq!(core.phase(), ClientPhase::NeedLogin);
        let mut ui = NetUi::new("ws://h/ws");
        let s = ui.screens(&mut core, &ClientPhase::NeedLogin, 0);
        assert_eq!(s.login.as_ref().map(|l| l.error.clone()), Some(None));
        // The player sends a password and is refused again: now it is a wrong password.
        assert!(matches!(
            ui.apply(
                &mut core,
                UiAction::Login {
                    nick: "alice".into(),
                    password: "x".into()
                },
                1
            ),
            NetEffect::Login { .. }
        ));
        let s = ui.screens(&mut core, &ClientPhase::NeedLogin, 1);
        assert_eq!(
            s.login.and_then(|l| l.error).as_deref(),
            Some("Wrong nick or password.")
        );
        // A signed-in player forgets it.
        ui.on_event(&ClientEvent::LoggedIn, 2);
        let s = ui.screens(&mut core, &ClientPhase::NeedLogin, 2);
        assert_eq!(s.login.map(|l| l.error), Some(None));
    }

    #[test]
    fn shell_actions_are_effects() {
        let mut rig = Rig::new();
        let mut apply = |a| rig.ui.apply(&mut rig.core, a, 0);
        assert_eq!(
            apply(UiAction::Login {
                nick: "bob".into(),
                password: "pw".into()
            }),
            NetEffect::Login {
                nick: "bob".into(),
                password: "pw".into()
            }
        );
        assert_eq!(apply(UiAction::SignOut), NetEffect::SignOut);
        assert_eq!(apply(UiAction::Reload), NetEffect::Reload);
        assert_eq!(apply(UiAction::LeaveRoom), NetEffect::None);
    }
}
