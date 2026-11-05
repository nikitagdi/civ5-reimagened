//! `net-bot play`: an autonomous bot on a mission. The mission is JSON:
//! where to connect, what to do in the lobby (host a room, join a room, take a seat of a running game), how
//! many turns to play, the pace, the persona, the seed and the knobs. The bot plays, then writes
//! `result.json`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use protocol::{
    AutoChoices, CivPick, GameSettings, LobbyChoice, LobbyChoices, LobbyCmd, Occupant,
    SlotOccupant, TimerMode, TurnMode,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::persona::Persona;
use super::policy::{Pace, Policy};
use super::{Bot, BotConfig, BotError};

fn default_turns() -> i32 {
    20
}
fn default_persona() -> String {
    "autoplay".into()
}
fn default_lobby_ms() -> u64 {
    60_000
}
fn default_turn_ms() -> u64 {
    120_000
}
fn one() -> u8 {
    1
}

/// What the host puts into the room; a field left out is the first entry of the server's list.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RoomSpec {
    pub name: Option<String>,
    pub world_size: Option<String>,
    pub map_script: Option<String>,
    pub speed: Option<String>,
    pub start_era: Option<String>,
    pub handicap: Option<String>,
    pub minor_civs: Option<u8>,
    /// `Hybrid` (the default) or `Simultaneous`.
    pub turn_mode: Option<String>,
    /// A fixed turn timer; none means `Off`.
    pub timer_seconds: Option<u16>,
    /// The Original timer row (a `TURNTIMER_*` type); wins over `timer_seconds`.
    pub timer_row: Option<String>,
    pub seed: Option<u32>,
    pub map_seed: Option<u32>,
    pub map_options: Vec<(String, i32)>,
    /// The civilization of the host's own slot (a `CIVILIZATION_*` type); none is random.
    pub civ: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum LobbyMission {
    /// Creates the room, puts AIs into the slots named, waits for `humans` humans (the host included) and
    /// starts.
    Host {
        #[serde(default)]
        settings: Box<RoomSpec>,
        #[serde(default)]
        ai_slots: Vec<u8>,
        #[serde(default = "one")]
        humans: u8,
        /// The slot the host sits in (default: the first). Slot 0 is then the AI's: the first human of a game
        /// is the core's active player, whose slot cannot be handed to the AI after a load (a long game for a
        /// save a person loads into an AI civilization).
        #[serde(default)]
        host_slot: Option<u8>,
    },
    /// Makes a room from a save (the id `save`, else the newest in the lobby's list), waits for `humans` humans
    /// (the host included; the others join with `Guest` and take their reserved slots) and starts.
    Load {
        #[serde(default)]
        save: Option<String>,
        #[serde(default = "one")]
        humans: u8,
    },
    /// Joins the room of the nick `host` and says it is ready. The host opens more slots than the size has by
    /// default after it made the room; with `slots` the guest waits until the room shows that many.
    Guest {
        host: String,
        #[serde(default)]
        slots: u8,
    },
    /// Takes the seat `player` of the running game `game`.
    Join { game: u32, player: i8 },
}

#[derive(Debug, Clone, Deserialize)]
pub struct Mission {
    pub url: String,
    pub nick: String,
    /// The name of the environment variable with the password.
    #[serde(default)]
    pub password_env: Option<String>,
    #[serde(default)]
    pub token_file: Option<PathBuf>,
    /// Without a lobby part the bot expects to be attached by its token (a game it already sits in).
    #[serde(default)]
    pub lobby: Option<LobbyMission>,
    #[serde(default = "default_turns")]
    pub turns: i32,
    #[serde(default)]
    pub pace: Pace,
    #[serde(default = "default_persona")]
    pub persona: String,
    #[serde(default)]
    pub persona_args: Value,
    #[serde(default)]
    pub seed: u64,
    #[serde(default)]
    pub knobs: Map<String, Value>,
    #[serde(default = "default_lobby_ms")]
    pub lobby_timeout_ms: u64,
    #[serde(default = "default_turn_ms")]
    pub turn_timeout_ms: u64,
}

fn first(list: &[LobbyChoice], what: &str) -> Result<String, BotError> {
    list.first()
        .map(|c| c.type_name.clone())
        .ok_or_else(|| BotError::Protocol(format!("the server's list of {what} is empty")))
}

/// The settings of a room from the spec and the lists of the server.
pub fn game_settings(
    spec: &RoomSpec,
    choices: &LobbyChoices,
    nick: &str,
    seed: u64,
    players: usize,
) -> Result<GameSettings, BotError> {
    let turn_mode = match spec.turn_mode.as_deref() {
        None | Some("Hybrid") => TurnMode::Hybrid,
        Some("Simultaneous") => TurnMode::Simultaneous,
        Some(other) => {
            return Err(BotError::Protocol(format!(
                "turn_mode {other}: Hybrid or Simultaneous"
            )));
        }
    };
    let pick = |own: &Option<String>, list: &[LobbyChoice], what: &str| match own {
        Some(v) => Ok(v.clone()),
        None => first(list, what),
    };
    Ok(GameSettings {
        name: spec
            .name
            .clone()
            .unwrap_or_else(|| format!("net-test {nick}")),
        // Not named: the smallest map that holds the room (the real core's Duel holds two players, the test world's
        // four); Duel when the list has none.
        world_size: spec.world_size.clone().unwrap_or_else(|| {
            choices
                .world_sizes
                .iter()
                .filter(|w| usize::from(w.max_players) >= players)
                .min_by_key(|w| w.max_players)
                .map_or_else(|| "WORLDSIZE_DUEL".into(), |w| w.id.clone())
        }),
        map_script: pick(&spec.map_script, &choices.map_scripts, "map scripts")?,
        map_options: spec.map_options.clone(),
        speed: pick(&spec.speed, &choices.speeds, "speeds")?,
        start_era: pick(&spec.start_era, &choices.eras, "eras")?,
        handicap: pick(&spec.handicap, &choices.handicaps, "difficulties")?,
        minor_civs: spec.minor_civs.unwrap_or(0),
        turn_mode,
        timer: match (&spec.timer_row, spec.timer_seconds) {
            (Some(row), _) => TimerMode::Original { row: row.clone() },
            (None, Some(seconds)) => TimerMode::Fixed { seconds },
            (None, None) => TimerMode::Off,
        },
        game_options: vec![],
        seed: Some(spec.seed.unwrap_or((seed & 0xFFFF_FFFF) as u32)),
        map_seed: spec.map_seed,
        auto_choices: AutoChoices::default(),
        diplo_auto_decline: true,
    })
}

fn fail(text: impl Into<String>) -> BotError {
    BotError::Protocol(text.into())
}

/// Waits for `pred`, and gives up with `what` after `timeout`; a login that was refused or a lobby error
/// ends the wait at once.
fn wait_for(
    bot: &mut Bot,
    timeout: Duration,
    what: &str,
    mut pred: impl FnMut(&Bot) -> bool,
) -> Result<(), BotError> {
    let errors = bot.s.stats.lobby_errors.len();
    let mut stop: Option<String> = None;
    let met = bot.wait_until(timeout, |b| {
        if let Some(r) = &b.s.rejected {
            stop = Some(format!("the server refused the login: {r}"));
            return true;
        }
        if b.s.stats.lobby_errors.len() > errors {
            stop = Some(format!(
                "lobby error: {}",
                b.s.stats.lobby_errors.last().map_or("", String::as_str)
            ));
            return true;
        }
        if !b.is_connected() && !b.is_reconnecting() {
            stop = Some("the connection is gone".into());
            return true;
        }
        pred(b)
    })?;
    match stop {
        Some(s) => Err(fail(s)),
        None if met => Ok(()),
        None => Err(BotError::Timeout(format!("waiting for {what}"))),
    }
}

/// The lobby part of a mission; on return the bot is attached to a seat and has its first view.
pub fn run_lobby(bot: &mut Bot, m: &Mission) -> Result<(), BotError> {
    let timeout = Duration::from_millis(m.lobby_timeout_ms);
    let nick = m.nick.clone();
    wait_for(bot, timeout, "the lobby", |b| {
        b.s.attach.is_some() || (b.s.choices.is_some() && b.s.lobby.is_some())
    })?;
    match &m.lobby {
        None => {}
        Some(LobbyMission::Host {
            settings,
            ai_slots,
            humans,
            host_slot,
        }) => {
            let choices = bot
                .s
                .choices
                .clone()
                .ok_or_else(|| fail("no pick lists from the server"))?;
            let settings_civ = settings.civ.clone();
            let settings = game_settings(
                settings,
                &choices,
                &nick,
                m.seed,
                usize::from(*humans) + ai_slots.len(),
            )?;
            bot.send_lobby(LobbyCmd::CreateRoom { settings })?;
            wait_for(bot, timeout, "my room", |b| {
                b.s.lobby.as_ref().is_some_and(|l| l.room.is_some())
            })?;
            if let Some(civ) = &settings_civ {
                bot.send_lobby(LobbyCmd::PickCiv {
                    civ: CivPick::Civ(civ.clone()),
                })?;
            }
            // The slots beyond the size's default are closed: open as many as the humans need; the open ones
            // beyond that are closed (an open slot at the start is a lobby error).
            let wanted = usize::from(*humans) + ai_slots.len();
            let slots = bot
                .s
                .lobby
                .as_ref()
                .and_then(|l| l.room.as_ref())
                .map(|r| r.slots.clone())
                .unwrap_or_default();
            let mut cmds = Vec::new();
            for s in slots {
                let inside = usize::from(s.index) < wanted;
                let occupant = match (&s.occupant, inside) {
                    (Occupant::Closed, true) if ai_slots.contains(&s.index) => SlotOccupant::Ai,
                    (Occupant::Closed, true) => SlotOccupant::Open,
                    (Occupant::Open, false) => SlotOccupant::Closed,
                    _ => continue,
                };
                cmds.push(LobbyCmd::Slot {
                    index: s.index,
                    occupant,
                    civ: CivPick::Random,
                });
            }
            for &index in ai_slots {
                cmds.push(LobbyCmd::Slot {
                    index,
                    occupant: SlotOccupant::Ai,
                    civ: CivPick::Random,
                });
            }
            // The gateway takes 10 lobby commands a second (a burst of 10). A room of a Huge map has a dozen slots to
            // set: the host then spaces them like a person clicking fast, and lets the room settle before it looks
            // at who is ready (every change of the room takes the guests' `Ready` off, and they say it again).
            // The host leaves slot 0 for `host_slot` before slot 0 is given to the AI.
            let (host_slot, mut late) = (host_slot.filter(|k| *k != 0), Vec::new());
            if host_slot.is_some() {
                let (zero, rest): (Vec<_>, Vec<_>) = cmds
                    .into_iter()
                    .partition(|c| matches!(c, LobbyCmd::Slot { index: 0, .. }));
                cmds = rest;
                late = zero;
            }
            let paced = cmds.len() > 8;
            for cmd in cmds {
                bot.send_lobby(cmd)?;
                if paced {
                    std::thread::sleep(Duration::from_millis(110));
                }
            }
            if paced {
                let _ = bot.wait_until(Duration::from_millis(600), |_| false);
            }
            if let Some(k) = host_slot {
                let me = nick.clone();
                wait_for(bot, timeout, "the slot of the host to be open", |b| {
                    b.s.lobby
                        .as_ref()
                        .and_then(|l| l.room.as_ref())
                        .and_then(|r| r.slots.get(usize::from(k)))
                        .is_some_and(|s| s.occupant == Occupant::Open)
                })?;
                bot.send_lobby(LobbyCmd::TakeSlot { index: k })?;
                wait_for(bot, timeout, "the host in its slot", |b| {
                    b.s.lobby
                        .as_ref()
                        .and_then(|l| l.room.as_ref())
                        .and_then(|r| r.slots.get(usize::from(k)))
                        .is_some_and(
                            |s| matches!(&s.occupant, Occupant::Human { nick, .. } if *nick == me),
                        )
                })?;
                for cmd in late {
                    bot.send_lobby(cmd)?;
                }
            }
            let (want, ai) = (usize::from(*humans), ai_slots.clone());
            let me = nick.clone();
            wait_for(bot, timeout, "the guests to be ready", |b| {
                let Some(room) = b.s.lobby.as_ref().and_then(|l| l.room.as_ref()) else {
                    return false;
                };
                let humans = room
                    .slots
                    .iter()
                    .filter(|s| matches!(s.occupant, Occupant::Human { .. }))
                    .count();
                let ready = room.slots.iter().all(|s| match &s.occupant {
                    Occupant::Human { nick, ready } => *nick == me || *ready,
                    _ => true,
                });
                let ais = ai.iter().all(|i| {
                    room.slots
                        .get(usize::from(*i))
                        .is_some_and(|s| s.occupant == Occupant::Ai)
                });
                humans >= want && ready && ais
            })?;
            bot.send_lobby(LobbyCmd::Start)?;
        }
        Some(LobbyMission::Load { save, humans }) => {
            let wanted = save.clone();
            let newest = move |b: &Bot| {
                let saves = &b.s.lobby.as_ref()?.saves;
                match &wanted {
                    Some(id) => saves.iter().find(|s| s.id == *id).map(|s| s.id.clone()),
                    None => saves
                        .iter()
                        .max_by_key(|s| s.saved_at_ms)
                        .map(|s| s.id.clone()),
                }
            };
            wait_for(bot, timeout, "a save in the lobby's list", |b| {
                newest(b).is_some()
            })?;
            let save = newest(bot).ok_or_else(|| fail("the save vanished from the list"))?;
            bot.send_lobby(LobbyCmd::LoadSave { save })?;
            wait_for(bot, timeout, "my room from the save", |b| {
                b.s.lobby
                    .as_ref()
                    .and_then(|l| l.room.as_ref())
                    .is_some_and(|r| r.from_save.is_some())
            })?;
            let (want, me) = (usize::from(*humans), nick.clone());
            wait_for(bot, timeout, "the humans of the save to be ready", |b| {
                let Some(room) = b.s.lobby.as_ref().and_then(|l| l.room.as_ref()) else {
                    return false;
                };
                let seated = room
                    .slots
                    .iter()
                    .filter(|s| matches!(s.occupant, Occupant::Human { .. }))
                    .count();
                let ready = room.slots.iter().all(|s| match &s.occupant {
                    Occupant::Human { nick, ready } => *nick == me || *ready,
                    _ => true,
                });
                seated >= want && ready
            })?;
            bot.send_lobby(LobbyCmd::Start)?;
        }
        Some(LobbyMission::Guest { host, slots }) => {
            let (host, slots) = (host.clone(), *slots);
            let room_of = move |b: &Bot| {
                b.s.lobby.as_ref().and_then(|l| {
                    l.rooms
                        .iter()
                        .find(|r| r.host == host && r.slots >= slots)
                        .map(|r| r.id)
                })
            };
            wait_for(bot, timeout, "the room of the host", |b| {
                room_of(b).is_some()
            })?;
            let room = room_of(bot).ok_or_else(|| fail("the room vanished"))?;
            bot.send_lobby(LobbyCmd::JoinRoom { room })?;
            let me = nick.clone();
            wait_for(bot, timeout, "a seat in the room", |b| {
                b.s.lobby
                    .as_ref()
                    .and_then(|l| l.room.as_ref())
                    .is_some_and(|r| {
                        r.slots.iter().any(
                            |s| matches!(&s.occupant, Occupant::Human { nick, .. } if *nick == me),
                        )
                    })
            })?;
            // `Ready` is taken off at any change of the room (also when another guest joins), so a guest
            // says it again every time the room shows it as not ready, until the game starts.
            let started =
                |b: &Bot| b.s.attach.is_some() && b.s.turn.is_some() && b.s.view.is_some();
            let my_ready = move |b: &Bot| {
                b.s.lobby
                    .as_ref()
                    .and_then(|l| l.room.as_ref())
                    .and_then(|r| {
                        r.slots.iter().find_map(|s| match &s.occupant {
                            Occupant::Human { nick, ready } if *nick == me => Some(*ready),
                            _ => None,
                        })
                    })
            };
            let deadline = Instant::now() + timeout;
            while !started(bot) && Instant::now() < deadline {
                if my_ready(bot) == Some(false) {
                    bot.send_lobby(LobbyCmd::Ready(true))?;
                }
                // Look again after a moment: a reset that came with the same broadcast as our Ready is only seen
                // as "not ready" (the broadcasts are coalesced), so waiting for a change would wait for ever.
                bot.wait_until(Duration::from_millis(300), |b| started(b))?;
            }
        }
        Some(LobbyMission::Join { game, player }) => {
            let basis = bot.s.basis_claim_for(*game);
            bot.send_lobby(LobbyCmd::JoinGame {
                game: *game,
                player: *player,
                basis,
            })?;
        }
    }
    wait_for(bot, timeout, "the game to start", |b| {
        b.s.attach.is_some() && b.s.turn.is_some() && b.s.view.is_some()
    })
}

/// Runs a mission to its end and returns `result.json`. A failure is an entry of `errors`, not a lost result.
pub fn play(m: &Mission, timeline: Option<PathBuf>) -> Value {
    let password = m
        .password_env
        .as_deref()
        .and_then(|v| std::env::var(v).ok())
        .unwrap_or_default();
    play_with(m, &password, timeline)
}

/// [`play`] with the password given (for tests that start their own server; a mission file never holds one).
pub fn play_with(m: &Mission, password: &str, timeline: Option<PathBuf>) -> Value {
    let mut cfg = BotConfig::new(&m.url, &m.nick, password);
    cfg.register = true;
    cfg.token_file = m.token_file.clone();
    cfg.timeline = timeline;
    let mut bot = Bot::new(cfg);
    let outcome = play_inner(&mut bot, m);
    let mut result = bot.result_json();
    let done = bot.play_done.as_ref().map(|d| d.text());
    if let Err(e) = outcome {
        bot.s.stats.error(format!("mission: {e}"));
        result = bot.result_json();
    }
    if let Some(o) = result.as_object_mut() {
        o.insert(
            "play".into(),
            json!({ "done": done, "game": bot.s.attach.as_ref().map(|a| a.game), "mission_turns": m.turns }),
        );
    }
    bot.close();
    result
}

/// The first login. The gateway closes the fresh connections of one address beyond `pending_per_ip` (four
/// unfinished at once, close 1013 "try again later"): a fleet that starts together meets it, and the bot tries
/// again after a moment, as a browser would.
fn first_login(bot: &mut Bot) -> Result<(), BotError> {
    const TRIES: u32 = 10;
    for attempt in 1..=TRIES {
        let last = attempt == TRIES;
        match bot.connect(false, None) {
            Ok(()) => {
                // A socket that breaks here is the refusal: it is tried again below.
                let waited = bot.wait_until(Duration::from_secs(5), |b| {
                    b.s.logged_in || b.s.rejected.is_some() || !b.is_connected()
                });
                if last {
                    return waited.map(|_| ());
                }
                if waited.is_ok() && bot.is_connected() {
                    return Ok(());
                }
            }
            Err(BotError::Ws(_)) if !last => {}
            Err(e) => return Err(e),
        }
        std::thread::sleep(Duration::from_millis(150 * u64::from(attempt)));
    }
    Ok(())
}

fn play_inner(bot: &mut Bot, m: &Mission) -> Result<(), BotError> {
    for (k, v) in &m.knobs {
        bot.s.knobs.set(k, v).map_err(fail)?;
    }
    let persona = Persona::parse(&m.persona, &m.persona_args).map_err(fail)?;
    first_login(bot)?;
    run_lobby(bot, m)?;
    bot.driver = Some(Box::new(Policy::new(
        persona,
        m.pace,
        m.turns,
        m.seed,
        Duration::from_millis(m.turn_timeout_ms),
    )));
    // The policy is driven by `pump`; it ends with `play_done`.
    while bot.play_done.is_none() {
        bot.pump()?;
        if let Some(r) = &bot.s.rejected {
            return Err(fail(format!("rejected: {r}")));
        }
        if !bot.is_connected() && !bot.is_reconnecting() {
            return Err(fail("the connection is gone"));
        }
        if let Some(reason) = &bot.s.detached
            && !bot.is_connected()
        {
            return Err(fail(format!("detached: {reason:?}")));
        }
    }
    Ok(())
}
