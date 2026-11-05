//! `RoomScript` (net-client 4.11 "Autostart"): after `LoggedIn` without a seat, the client walks the lobby by
//! itself: `CreateRoom` with the settings, the open slots to AI, "Ready", `Start`. The "Quick start" button of
//! the lobby runs it (lobby-ui 4.3); `?autostart=1` is the same script (the app starts it after the login).
//!
//! The script leans on `LobbyState`, not on timers: after a command it waits for the next `LobbyChanged`
//! (or a refusal, or [`STEP_TIMEOUT_MS`]) before it looks again.

use client_core::{ClientCore, ClientEvent, ClientPhase};
use protocol::{
    AutoChoices, CivPick, GameSettings, LobbyChoices, LobbyCmd, Occupant, RoomState, SlotOccupant,
    TimerMode, TurnMode, WorldSizeChoice,
};

/// How long a command may take to show in `LobbyState` before the script looks again.
pub const STEP_TIMEOUT_MS: u64 = 3_000;
/// The room does not appear this long after `CreateRoom`: the script gives up.
const ROOM_TIMEOUT_MS: u64 = 10_000;

/// What the script is asked for (`size`, `ai`, `civ`, `seed`, `mode`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomParams {
    /// A world size by id (`WORLDSIZE_DUEL`) or name (`Duel`), case does not matter. Default Duel.
    pub size: Option<String>,
    /// AI players, at least 1.
    pub ai: u8,
    /// The host's civilization (`CIVILIZATION_ROME`); random when `None`.
    pub civ: Option<String>,
    pub seed: Option<u32>,
    pub mode: TurnMode,
    /// The human makes the choices of the game (research, policies, religion, ...) himself through the UI: no
    /// `auto_choices`, the choices block END TURN as in the original. The original UI has the popups and sets it
    /// (`&manual=0` takes it back); the interim UI has no screens for them (SIMP-002), there the core decides.
    pub manual_choices: bool,
}

impl Default for RoomParams {
    fn default() -> Self {
        Self {
            size: None,
            ai: 1,
            civ: None,
            seed: None,
            mode: TurnMode::Hybrid,
            manual_choices: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptState {
    Running,
    /// The game is starting (or already attached).
    Done,
    Failed(String),
}

#[derive(Debug)]
pub struct RoomScript {
    params: RoomParams,
    created: bool,
    /// `LobbyState` changed since the last command (the first one counts).
    fresh: bool,
    sent_at: Option<u64>,
    started: bool,
    failed: Option<String>,
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// The size of the params, else Duel, else the first one.
fn pick_size<'a>(choices: &'a LobbyChoices, want: Option<&str>) -> Option<&'a WorldSizeChoice> {
    let by = |w: &str| {
        choices
            .world_sizes
            .iter()
            .find(|s| same(&s.id, w) || same(&s.name, w) || same(&s.id, &format!("WORLDSIZE_{w}")))
    };
    want.and_then(by)
        .or_else(|| by("DUEL"))
        .or_else(|| choices.world_sizes.first())
}

/// An entry by preferred type name, else the first (one that is not `skip`).
fn prefer<'a>(list: &'a [protocol::LobbyChoice], want: &[&str], skip: &str) -> Option<&'a str> {
    want.iter()
        .find_map(|w| list.iter().find(|c| same(&c.type_name, w)))
        .or_else(|| list.iter().find(|c| c.type_name != skip))
        .map(|c| c.type_name.as_str())
}

/// The default value of each custom option of a map script (`LobbyChoices.map_options`), as the room keeps
/// them in `GameSettings::map_options`. Read only options and scripts the server does not list give nothing.
pub fn script_default_options(choices: &LobbyChoices, script: &str) -> Vec<(String, i32)> {
    choices
        .map_options
        .iter()
        .find(|m| m.script == script)
        .map(|m| {
            m.options
                .iter()
                .filter(|o| !o.read_only)
                .map(|o| (o.id.clone(), o.default))
                .collect()
        })
        .unwrap_or_default()
}

/// The settings of a quick game, or why there are none (the lists are empty).
pub fn default_settings(
    choices: &LobbyChoices,
    params: &RoomParams,
    nick: &str,
) -> Result<GameSettings, String> {
    let size =
        pick_size(choices, params.size.as_deref()).ok_or("the server has no map sizes yet")?;
    let script = prefer(
        &choices.map_scripts,
        &["LekmapPangaeaFractalv5.2", "Pangaea"],
        "",
    )
    .ok_or("the server has no map scripts")?;
    let speed = prefer(&choices.speeds, &["GAMESPEED_STANDARD"], "")
        .ok_or("the server has no game speeds")?;
    let era = prefer(&choices.eras, &["ERA_ANCIENT"], "").ok_or("the server has no eras")?;
    let handicap = prefer(&choices.handicaps, &["HANDICAP_PRINCE"], "HANDICAP_AI")
        .ok_or("the server has no difficulties")?;
    Ok(GameSettings {
        name: format!("{nick}'s game").chars().take(40).collect(),
        world_size: size.id.clone(),
        map_script: script.to_owned(),
        map_options: script_default_options(choices, script),
        speed: speed.to_owned(),
        start_era: era.to_owned(),
        handicap: handicap.to_owned(),
        minor_civs: size.default_minor.min(size.max_minor),
        turn_mode: params.mode,
        timer: TimerMode::Off,
        game_options: vec![],
        seed: params.seed,
        map_seed: None,
        auto_choices: if params.manual_choices {
            AutoChoices {
                research: false,
                policy: false,
                ideology: false,
                religion: false,
                promotion: false,
                great_person: false,
                captured_city: false,
                league: false,
                archaeology: false,
                goody_hut: false,
                steal_tech: false,
            }
        } else {
            AutoChoices::default()
        },
        // SIMP-003: offers of the AI are declined.
        diplo_auto_decline: true,
    })
}

// SIMPLIFIED(SIMP-exe-1087): the last room's seeds and name are not kept; stale picks fall back to the defaults.
/// v12: the settings of the room the player started a game from last (`AccountData.last_room`) laid over the
/// quick defaults `base`. Every pick is checked against the server's lists, and what the server would not take (a
/// size, map script, speed, era, difficulty or timer row that is gone, options of another map script) falls back
/// to `base`: a stale store must never make a room that cannot be created. The name stays `base`'s.
pub fn settings_from_last_room(
    base: GameSettings,
    last: &GameSettings,
    choices: &LobbyChoices,
) -> GameSettings {
    let has = |list: &[protocol::LobbyChoice], t: &str| list.iter().any(|c| c.type_name == t);
    let mut s = base;
    if let Some(size) = choices.world_sizes.iter().find(|z| z.id == last.world_size) {
        s.world_size = size.id.clone();
        s.minor_civs = last.minor_civs.min(size.max_minor);
    }
    if has(&choices.map_scripts, &last.map_script) {
        s.map_script = last.map_script.clone();
        let known = choices
            .map_options
            .iter()
            .find(|m| m.script == s.map_script);
        s.map_options = last
            .map_options
            .iter()
            .filter(|(k, _)| known.is_some_and(|m| m.options.iter().any(|o| &o.id == k)))
            .cloned()
            .collect();
        if s.map_options.is_empty() {
            s.map_options = script_default_options(choices, &s.map_script);
        }
    }
    if has(&choices.speeds, &last.speed) {
        s.speed = last.speed.clone();
    }
    if has(&choices.eras, &last.start_era) {
        s.start_era = last.start_era.clone();
    }
    if has(&choices.handicaps, &last.handicap) {
        s.handicap = last.handicap.clone();
    }
    s.turn_mode = last.turn_mode;
    s.timer = match &last.timer {
        TimerMode::Original { row } if !has(&choices.timers, row) => TimerMode::Off,
        t => t.clone(),
    };
    s.game_options = last
        .game_options
        .iter()
        .filter(|(k, _)| {
            k == protocol::MAX_TURNS_KEY
                || (k.starts_with(protocol::VICTORY_PREFIX)
                    && choices.victories.iter().any(|v| &v.type_name == k))
                || choices.options.iter().any(|o| &o.key == k)
        })
        .cloned()
        .collect();
    s.auto_choices = last.auto_choices;
    s.diplo_auto_decline = last.diplo_auto_decline;
    s
}

impl RoomScript {
    pub fn new(params: RoomParams) -> Self {
        Self {
            params,
            created: false,
            fresh: true,
            sent_at: None,
            started: false,
            failed: None,
        }
    }

    /// The core's events: a new `LobbyState` lets the next step go; a refusal stops the script.
    pub fn on_event(&mut self, ev: &ClientEvent) {
        match ev {
            ClientEvent::LobbyChanged => self.fresh = true,
            ClientEvent::LobbyError(text) => self.failed = Some(text.clone()),
            _ => {}
        }
    }

    /// One look at the lobby; at most one command goes out.
    pub fn step(&mut self, core: &mut ClientCore, now: u64) -> ScriptState {
        if let Some(why) = &self.failed {
            return ScriptState::Failed(why.clone());
        }
        match core.phase() {
            ClientPhase::Lobby | ClientPhase::Room => {}
            ClientPhase::Starting | ClientPhase::Loading | ClientPhase::Playing => {
                return ScriptState::Done;
            }
            // The login or the line is not ready yet: wait.
            _ => return ScriptState::Running,
        }
        let (Some(choices), Some(state)) = (core.choices().cloned(), core.lobby_state().cloned())
        else {
            return ScriptState::Running;
        };
        if self.started {
            return ScriptState::Done;
        }
        if !self.fresh
            && self
                .sent_at
                .is_none_or(|t| now.saturating_sub(t) < STEP_TIMEOUT_MS)
        {
            return ScriptState::Running;
        }
        let me = core.nick().to_owned();
        let cmd = match &state.room {
            None => {
                if self.created {
                    if self
                        .sent_at
                        .is_some_and(|t| now.saturating_sub(t) > ROOM_TIMEOUT_MS)
                    {
                        return ScriptState::Failed("the room did not appear".into());
                    }
                    return ScriptState::Running;
                }
                match default_settings(&choices, &self.params, &me) {
                    Ok(settings) => {
                        self.created = true;
                        Some(LobbyCmd::CreateRoom { settings })
                    }
                    Err(why) => return ScriptState::Failed(why),
                }
            }
            Some(room) => {
                if !same(&room.host, &me) {
                    return ScriptState::Failed("the script needs a room of its own".into());
                }
                self.next_in_room(room, &me)
            }
        };
        let Some(cmd) = cmd else {
            return ScriptState::Running;
        };
        let start = cmd == LobbyCmd::Start;
        match core.lobby(cmd) {
            Ok(()) => {
                self.fresh = false;
                self.sent_at = Some(now);
                self.started = start;
                ScriptState::Running
            }
            Err(e) => ScriptState::Failed(format!("{e:?}")),
        }
    }

    /// The next command for the room as it is, `None` when it waits for something.
    fn next_in_room(&self, room: &RoomState, me: &str) -> Option<LobbyCmd> {
        let mine = room
            .slots
            .iter()
            .find(|s| matches!(&s.occupant, Occupant::Human { nick, .. } if same(nick, me)))?;
        // 1. our civilization
        let want = self
            .params
            .civ
            .as_ref()
            .map_or(CivPick::Random, |c| CivPick::Civ(c.clone()));
        if mine.civ != want {
            return Some(LobbyCmd::PickCiv { civ: want });
        }
        // 2. the AI slots: as many as asked, the others closed
        let others = room.slots.iter().filter(|s| s.index != mine.index);
        let ai = others
            .clone()
            .filter(|s| s.occupant == Occupant::Ai)
            .count();
        if ai < usize::from(self.params.ai.max(1)) {
            let slot = others
                .clone()
                .find(|s| s.occupant == Occupant::Open)
                .or_else(|| others.clone().find(|s| s.occupant == Occupant::Closed));
            // No slot left: go on with what there is.
            if let Some(s) = slot {
                return Some(LobbyCmd::Slot {
                    index: s.index,
                    occupant: SlotOccupant::Ai,
                    civ: CivPick::Random,
                });
            }
        }
        if let Some(s) = others.clone().find(|s| s.occupant == Occupant::Open) {
            return Some(LobbyCmd::Slot {
                index: s.index,
                occupant: SlotOccupant::Closed,
                civ: CivPick::Random,
            });
        }
        // 3. ready, 4. start
        if !matches!(&mine.occupant, Occupant::Human { ready: true, .. }) {
            return Some(LobbyCmd::Ready(true));
        }
        Some(LobbyCmd::Start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{LobbyChoice, RoomSlot};

    fn lc(t: &str) -> LobbyChoice {
        LobbyChoice {
            type_name: t.into(),
            name: t.into(),
        }
    }

    fn choices() -> LobbyChoices {
        let size = |id: &str, name: &str, def: u8, max: u8| WorldSizeChoice {
            id: id.into(),
            name: name.into(),
            width: 40,
            height: 24,
            default_players: def,
            max_players: max,
            default_minor: 2,
            max_minor: 4,
        };
        LobbyChoices {
            world_sizes: vec![
                size("WORLDSIZE_TINY", "Tiny", 3, 6),
                size("WORLDSIZE_DUEL", "Duel", 2, 4),
            ],
            map_scripts: vec![lc("continents"), lc("Pangaea")],
            speeds: vec![lc("GAMESPEED_QUICK"), lc("GAMESPEED_STANDARD")],
            eras: vec![lc("ERA_ANCIENT"), lc("ERA_MEDIEVAL")],
            handicaps: vec![
                lc("HANDICAP_AI"),
                lc("HANDICAP_KING"),
                lc("HANDICAP_PRINCE"),
            ],
            ..LobbyChoices::default()
        }
    }

    fn room(slots: Vec<Occupant>, civ0: CivPick) -> RoomState {
        let settings = default_settings(&choices(), &RoomParams::default(), "alice").unwrap();
        RoomState {
            id: 1,
            name: "r".into(),
            host: "alice".into(),
            status: protocol::GameStatus::Lobby,
            settings,
            slots: slots
                .into_iter()
                .enumerate()
                .map(|(i, occupant)| RoomSlot {
                    index: i as u8,
                    occupant,
                    civ: if i == 0 {
                        civ0.clone()
                    } else {
                        CivPick::Random
                    },
                    reserved_for: None,
                })
                .collect(),
            from_save: None,
        }
    }

    fn human(ready: bool) -> Occupant {
        Occupant::Human {
            nick: "alice".into(),
            ready,
        }
    }

    #[test]
    fn the_last_room_is_laid_over_the_defaults_and_stale_picks_fall_back() {
        let c = choices();
        let base = default_settings(&c, &RoomParams::default(), "alice").unwrap();
        let mut last = base.clone();
        last.name = "old name".into();
        last.world_size = c.world_sizes.last().unwrap().id.clone();
        last.turn_mode = TurnMode::Simultaneous;
        last.timer = TimerMode::Fixed { seconds: 45 };
        last.diplo_auto_decline = false;
        let s = settings_from_last_room(base.clone(), &last, &c);
        assert_eq!(s.name, base.name, "the name is the default's");
        assert_eq!(s.world_size, last.world_size);
        assert_eq!(s.turn_mode, TurnMode::Simultaneous);
        assert_eq!(s.timer, TimerMode::Fixed { seconds: 45 });
        assert!(!s.diplo_auto_decline);
        // A size, script, speed, era, difficulty and timer row that are gone fall back.
        let mut stale = base.clone();
        stale.world_size = "WORLDSIZE_GONE".into();
        stale.map_script = "gone.lua".into();
        stale.speed = "GAMESPEED_GONE".into();
        stale.start_era = "ERA_GONE".into();
        stale.handicap = "HANDICAP_GONE".into();
        stale.timer = TimerMode::Original {
            row: "TURNTIMER_GONE".into(),
        };
        stale.game_options = vec![("GAMEOPTION_GONE".into(), 1), ("MAX_TURNS".into(), 200)];
        let s = settings_from_last_room(base.clone(), &stale, &c);
        assert_eq!(
            (
                s.world_size.as_str(),
                s.map_script.as_str(),
                s.speed.as_str()
            ),
            (
                base.world_size.as_str(),
                base.map_script.as_str(),
                base.speed.as_str()
            )
        );
        assert_eq!(
            (s.start_era.as_str(), s.handicap.as_str()),
            (base.start_era.as_str(), base.handicap.as_str())
        );
        assert_eq!(s.timer, TimerMode::Off);
        assert_eq!(
            s.game_options,
            [("MAX_TURNS".to_string(), 200)],
            "an unknown option is dropped, the max turns stay"
        );
    }

    #[test]
    fn the_default_settings_pick_duel_prince_and_hybrid() {
        let s = default_settings(&choices(), &RoomParams::default(), "alice").unwrap();
        assert_eq!(s.world_size, "WORLDSIZE_DUEL");
        assert_eq!(s.handicap, "HANDICAP_PRINCE");
        assert_eq!(s.speed, "GAMESPEED_STANDARD");
        assert_eq!(s.start_era, "ERA_ANCIENT");
        assert_eq!(s.map_script, "Pangaea");
        assert_eq!(s.turn_mode, TurnMode::Hybrid);
        assert_eq!(s.minor_civs, 2);
        assert!(s.diplo_auto_decline);
        assert_eq!(s.name, "alice's game");
        // Size by name or id, case does not matter; a stranger falls back to Duel.
        for want in ["tiny", "WORLDSIZE_TINY", "Tiny"] {
            let p = RoomParams {
                size: Some(want.into()),
                seed: Some(5),
                mode: TurnMode::Simultaneous,
                ..RoomParams::default()
            };
            let s = default_settings(&choices(), &p, "a").unwrap();
            assert_eq!(s.world_size, "WORLDSIZE_TINY");
            assert_eq!((s.seed, s.turn_mode), (Some(5), TurnMode::Simultaneous));
        }
        let p = RoomParams {
            size: Some("nonsense".into()),
            ..RoomParams::default()
        };
        assert_eq!(
            default_settings(&choices(), &p, "a").unwrap().world_size,
            "WORLDSIZE_DUEL"
        );
        assert!(default_settings(&LobbyChoices::default(), &RoomParams::default(), "a").is_err());
    }

    #[test]
    fn the_steps_of_a_duel_room() {
        let script = RoomScript::new(RoomParams::default());
        // Our slot, one open: the open slot becomes AI.
        let r = room(vec![human(false), Occupant::Open], CivPick::Random);
        assert_eq!(
            script.next_in_room(&r, "alice"),
            Some(LobbyCmd::Slot {
                index: 1,
                occupant: SlotOccupant::Ai,
                civ: CivPick::Random
            })
        );
        // Then ready, then start.
        let r = room(vec![human(false), Occupant::Ai], CivPick::Random);
        assert_eq!(
            script.next_in_room(&r, "alice"),
            Some(LobbyCmd::Ready(true))
        );
        let r = room(vec![human(true), Occupant::Ai], CivPick::Random);
        assert_eq!(script.next_in_room(&r, "alice"), Some(LobbyCmd::Start));
    }

    #[test]
    fn extra_open_slots_are_closed_and_a_civilization_comes_first() {
        let script = RoomScript::new(RoomParams {
            civ: Some("CIVILIZATION_ROME".into()),
            ..RoomParams::default()
        });
        let r = room(
            vec![human(false), Occupant::Ai, Occupant::Open],
            CivPick::Random,
        );
        assert_eq!(
            script.next_in_room(&r, "alice"),
            Some(LobbyCmd::PickCiv {
                civ: CivPick::Civ("CIVILIZATION_ROME".into())
            })
        );
        let r = room(
            vec![human(false), Occupant::Ai, Occupant::Open],
            CivPick::Civ("CIVILIZATION_ROME".into()),
        );
        assert_eq!(
            script.next_in_room(&r, "alice"),
            Some(LobbyCmd::Slot {
                index: 2,
                occupant: SlotOccupant::Closed,
                civ: CivPick::Random
            })
        );
    }

    #[test]
    fn more_ai_than_default_opens_closed_slots() {
        let script = RoomScript::new(RoomParams {
            ai: 2,
            ..RoomParams::default()
        });
        let r = room(
            vec![human(false), Occupant::Ai, Occupant::Closed],
            CivPick::Random,
        );
        assert_eq!(
            script.next_in_room(&r, "alice"),
            Some(LobbyCmd::Slot {
                index: 2,
                occupant: SlotOccupant::Ai,
                civ: CivPick::Random
            })
        );
    }

    #[test]
    fn a_refusal_ends_the_script() {
        let mut s = RoomScript::new(RoomParams::default());
        s.on_event(&ClientEvent::LobbyError("too many rooms".into()));
        let mut core = ClientCore::new(client_core::NetConfig::new("t", "alice"));
        assert_eq!(
            s.step(&mut core, 0),
            ScriptState::Failed("too many rooms".into())
        );
    }
}
