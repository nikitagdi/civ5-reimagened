//! Checks of the room settings against the pick lists of the `Describe` run (GS-D17).
//! A mistake is a `LobbyError` text, never a worker failure.

use protocol::{
    CivPick, GameSettings, LobbyChoice, LobbyChoices, MAX_TURNS_KEY, MAX_TURNS_LIMIT,
    MapScriptOptions, SlotOccupant, TimerMode, VICTORY_PREFIX, WorldSizeChoice,
};

/// Longest room name.
pub const MAX_NAME_CHARS: usize = 40;

/// Options a player cannot set (gc-abi-v2 4.1), without the `GAMEOPTION_` prefix (a key is checked with and without it):
/// the brain sets them itself or the room has its own control.
// SIMPLIFIED(SIMP-055): FIRSTMOVE and FAST_HAND are refused; the original Lekmod offers them (they need the brain's clock and the exe UI).
pub const FORBIDDEN_OPTIONS: [&str; 8] = [
    "PITBOSS",
    "END_TURN_TIMER_ENABLED",
    "QUICK_COMBAT",
    "QUICK_MOVEMENT",
    "DYNAMIC_TURNS",
    "SIMULTANEOUS_TURNS",
    "FIRSTMOVE",
    "FAST_HAND",
];

fn listed(list: &[LobbyChoice], what: &str, value: &str) -> Result<(), String> {
    if list.iter().any(|c| c.type_name == value) {
        Ok(())
    } else {
        Err(format!("unknown {what} `{value}`"))
    }
}

/// A picked civilization must be one the room lists (the playable ones of the game data); "random" always is. A pick
/// list without civilizations (a core that did not describe them) accepts any name.
pub fn civ(choices: &LobbyChoices, pick: &CivPick) -> Result<(), String> {
    match pick {
        CivPick::Civ(id)
            if !choices.civs.is_empty() && !choices.civs.iter().any(|c| &c.id == id) =>
        {
            Err(format!("unknown civilization `{id}`"))
        }
        _ => Ok(()),
    }
}

/// The size entry of `id`, if the lists know it.
pub fn world_size<'a>(choices: &'a LobbyChoices, id: &str) -> Option<&'a WorldSizeChoice> {
    choices.world_sizes.iter().find(|w| w.id == id)
}

/// Checks name, size, map script, speed, era, difficulty, city-states, options and the timer.
pub fn settings(s: &GameSettings, choices: &LobbyChoices) -> Result<(), String> {
    let name = s.name.trim();
    if name.is_empty()
        || name.chars().count() > MAX_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        return Err(format!(
            "the room name must be 1 to {MAX_NAME_CHARS} characters"
        ));
    }
    let size = world_size(choices, &s.world_size)
        .ok_or_else(|| format!("unknown world size `{}`", s.world_size))?;
    listed(&choices.map_scripts, "map script", &s.map_script)?;
    listed(&choices.speeds, "game speed", &s.speed)?;
    listed(&choices.eras, "start era", &s.start_era)?;
    listed(&choices.handicaps, "difficulty", &s.handicap)?;
    if s.minor_civs > size.max_minor {
        return Err(format!(
            "at most {} city-states on this map size",
            size.max_minor
        ));
    }
    game_options(s, choices)?;
    map_options(s, choices)?;
    match &s.timer {
        TimerMode::Off => {}
        TimerMode::Original { row } => listed(&choices.timers, "turn timer", row)?,
        TimerMode::Fixed { seconds } => {
            if !(10..=3600).contains(seconds) {
                return Err("a fixed timer is 10 to 3600 seconds".to_owned());
            }
        }
    }
    Ok(())
}

/// `game_options` carries three kinds of keys (protocol v8): a listed game option (a check box 0/1, or one of its
/// values), a listed victory (`VICTORY_*`, 0 or 1) and the turn limit (`MAX_TURNS`, 0 to `MAX_TURNS_LIMIT`).
fn game_options(s: &GameSettings, choices: &LobbyChoices) -> Result<(), String> {
    for (i, (key, value)) in s.game_options.iter().enumerate() {
        if s.game_options[..i].iter().any(|(k, _)| k == key) {
            return Err(format!("option `{key}` is given twice"));
        }
        let short = key.strip_prefix("GAMEOPTION_").unwrap_or(key);
        if FORBIDDEN_OPTIONS.contains(&short) {
            return Err(format!("option `{key}` is not allowed"));
        }
        if key == MAX_TURNS_KEY {
            // SIMPLIFIED(SIMP-exe-853): a whole number 0 to MAX_TURNS_LIMIT; the original edit box takes any positive number.
            if !(0..=MAX_TURNS_LIMIT).contains(value) {
                return Err(format!("max turns is 0 to {MAX_TURNS_LIMIT}"));
            }
            continue;
        }
        if key.starts_with(VICTORY_PREFIX) {
            listed(&choices.victories, "victory", key)?;
            if !matches!(value, 0 | 1) {
                return Err(format!("bad value {value} for victory `{key}`"));
            }
            continue;
        }
        let Some(opt) = choices.options.iter().find(|o| &o.key == key) else {
            return Err(format!("unknown option `{key}`"));
        };
        let ok = if opt.values.is_empty() {
            matches!(value, 0 | 1)
        } else {
            opt.values.iter().any(|(v, _)| v == value)
        };
        if !ok {
            return Err(format!("bad value {value} for option `{key}`"));
        }
    }
    Ok(())
}

/// The custom options of a map script, if the lists know the script.
fn script_options<'a>(choices: &'a LobbyChoices, script: &str) -> Option<&'a MapScriptOptions> {
    choices.map_options.iter().find(|m| m.script == script)
}

/// Every `map_options` id belongs to the chosen map script and carries one of its values (0 or 1 for a check box);
/// a read-only option keeps its default.
fn map_options(s: &GameSettings, choices: &LobbyChoices) -> Result<(), String> {
    let own = script_options(choices, &s.map_script);
    for (i, (id, value)) in s.map_options.iter().enumerate() {
        if s.map_options[..i].iter().any(|(k, _)| k == id) {
            return Err(format!("map option `{id}` is given twice"));
        }
        let Some(opt) = own.and_then(|m| m.options.iter().find(|o| &o.id == id)) else {
            return Err(format!("unknown map option `{id}` for this map type"));
        };
        let ok = if opt.values.is_empty() {
            matches!(value, 0 | 1)
        } else {
            opt.values.iter().any(|(v, _)| v == value)
        };
        if !ok {
            return Err(format!("bad value {value} for map option `{}`", opt.name));
        }
        if opt.read_only && *value != opt.default {
            return Err(format!("map option `{}` cannot be changed", opt.name));
        }
    }
    Ok(())
}

/// The defaults of the custom options of `script`, as `GameSettings.map_options` entries.
pub fn map_defaults(choices: &LobbyChoices, script: &str) -> Vec<(String, i32)> {
    script_options(choices, script)
        .map(|m| {
            m.options
                .iter()
                .map(|o| (o.id.clone(), o.default))
                .collect()
        })
        .unwrap_or_default()
}

/// Puts the map script options in order before the settings are checked. A different map script than `previous`
/// starts from the defaults of the new script (the options of the old one mean nothing there); a new room
/// (`previous` is `None`) or the same script keeps what the host chose and gets the defaults of the options it
/// lacks. The server does this so that every client sees the same values.
pub fn fill_map_options(s: &mut GameSettings, previous: Option<&str>, choices: &LobbyChoices) {
    let defaults = map_defaults(choices, &s.map_script);
    if previous.is_some_and(|p| p != s.map_script) {
        s.map_options = defaults;
        return;
    }
    for (id, v) in defaults {
        if !s.map_options.iter().any(|(k, _)| *k == id) {
            s.map_options.push((id, v));
        }
    }
}

/// What the host may put into a slot of a room made from a save: the players of the save
/// are `Open` (a place for a human) or `Ai`, never `Closed` (they exist in the game); the slots the
/// save does not have stay closed.
pub fn save_slot(occupant: SlotOccupant, in_save: bool) -> Result<(), String> {
    match (in_save, occupant) {
        (true, SlotOccupant::Closed) => {
            Err("a player of the save cannot be closed; give the slot to the AI".to_owned())
        }
        (false, SlotOccupant::Open | SlotOccupant::Ai) => {
            Err("the save has no player for this slot".to_owned())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{AutoChoices, MapOptionChoice, OptionChoice, TurnMode};

    fn choices() -> LobbyChoices {
        let c = |t: &str| LobbyChoice {
            type_name: t.into(),
            name: t.into(),
        };
        LobbyChoices {
            world_sizes: vec![WorldSizeChoice {
                id: "DUEL".into(),
                name: "Duel".into(),
                width: 40,
                height: 24,
                default_players: 2,
                max_players: 4,
                default_minor: 0,
                max_minor: 2,
            }],
            map_scripts: vec![c("map"), c("other")],
            speeds: vec![c("speed")],
            eras: vec![c("era")],
            handicaps: vec![c("hc")],
            timers: vec![c("FAST")],
            options: vec![OptionChoice {
                key: "OPT".into(),
                name: "opt".into(),
                default: 0,
                values: vec![],
                help: String::new(),
                group: protocol::OptionGroup::General,
            }],
            victories: vec![c("VICTORY_TIME"), c("VICTORY_CULTURAL")],
            map_options: vec![
                MapScriptOptions {
                    script: "map".into(),
                    options: vec![
                        MapOptionChoice {
                            id: "1".into(),
                            name: "Age".into(),
                            help: String::new(),
                            default: 2,
                            values: vec![(1, "Young".into()), (2, "Mid".into()), (3, "Old".into())],
                            read_only: false,
                        },
                        MapOptionChoice {
                            id: "2".into(),
                            name: "Rivers".into(),
                            help: String::new(),
                            default: 1,
                            values: vec![],
                            read_only: false,
                        },
                        MapOptionChoice {
                            id: "3".into(),
                            name: "Fixed".into(),
                            help: String::new(),
                            default: 2,
                            values: vec![(1, "A".into()), (2, "B".into())],
                            read_only: true,
                        },
                    ],
                },
                MapScriptOptions {
                    script: "other".into(),
                    options: vec![MapOptionChoice {
                        id: "9".into(),
                        name: "Nine".into(),
                        help: String::new(),
                        default: 1,
                        values: vec![(1, "x".into()), (2, "y".into())],
                        read_only: false,
                    }],
                },
            ],
            ..LobbyChoices::default()
        }
    }

    fn good() -> GameSettings {
        GameSettings {
            name: "room".into(),
            world_size: "DUEL".into(),
            map_script: "map".into(),
            map_options: vec![],
            speed: "speed".into(),
            start_era: "era".into(),
            handicap: "hc".into(),
            minor_civs: 0,
            turn_mode: TurnMode::Hybrid,
            timer: TimerMode::Off,
            game_options: vec![],
            seed: None,
            map_seed: None,
            auto_choices: AutoChoices::default(),
            diplo_auto_decline: true,
            mods: Vec::new(),
        }
    }

    #[test]
    fn a_civilization_must_be_listed() {
        let mut c = choices();
        c.civs = vec![protocol::CivChoice {
            id: "CIVILIZATION_ROME".into(),
            name: "Rome".into(),
            leader: String::new(),
            traits: vec![],
            unique: vec![],
        }];
        assert!(civ(&c, &CivPick::Random).is_ok());
        assert!(civ(&c, &CivPick::Civ("CIVILIZATION_ROME".into())).is_ok());
        for bad in ["CIVILIZATION_MINOR", "CIVILIZATION_BARBARIAN", "rome"] {
            let e = civ(&c, &CivPick::Civ(bad.into())).unwrap_err();
            assert!(e.contains("unknown civilization"), "{e}");
        }
        // No list (a core that did not describe the civilizations): any name goes.
        c.civs.clear();
        assert!(civ(&c, &CivPick::Civ("CIVILIZATION_MINOR".into())).is_ok());
    }

    #[test]
    fn good_settings_pass_and_each_list_is_checked() {
        let c = choices();
        assert!(settings(&good(), &c).is_ok());
        let mut s = good();
        s.world_size = "HUGE".into();
        assert!(settings(&s, &c).is_err());
        let mut s = good();
        s.speed = "x".into();
        assert!(settings(&s, &c).is_err());
        let mut s = good();
        s.minor_civs = 3;
        assert!(settings(&s, &c).is_err());
        let mut s = good();
        s.name = "n".repeat(41);
        assert!(settings(&s, &c).is_err());
    }

    #[test]
    fn slots_of_a_save_follow_the_save() {
        assert!(save_slot(SlotOccupant::Ai, true).is_ok());
        assert!(save_slot(SlotOccupant::Open, true).is_ok());
        assert!(save_slot(SlotOccupant::Closed, true).is_err());
        assert!(save_slot(SlotOccupant::Closed, false).is_ok());
        assert!(save_slot(SlotOccupant::Open, false).is_err());
        assert!(save_slot(SlotOccupant::Ai, false).is_err());
    }

    #[test]
    fn the_brains_own_options_are_forbidden_with_or_without_the_prefix() {
        let c = choices();
        for key in [
            "GAMEOPTION_PITBOSS",
            "GAMEOPTION_END_TURN_TIMER_ENABLED",
            "GAMEOPTION_QUICK_COMBAT",
            "GAMEOPTION_QUICK_MOVEMENT",
            "GAMEOPTION_DYNAMIC_TURNS",
            "GAMEOPTION_SIMULTANEOUS_TURNS",
            "GAMEOPTION_FIRSTMOVE",
            "GAMEOPTION_FAST_HAND",
            "FAST_HAND",
        ] {
            let mut s = good();
            s.game_options = vec![(key.into(), 1)];
            assert!(
                settings(&s, &c).unwrap_err().contains("not allowed"),
                "{key}"
            );
        }
    }

    #[test]
    fn victories_are_listed_and_zero_or_one() {
        let c = choices();
        let mut s = good();
        s.game_options = vec![("VICTORY_TIME".into(), 0), ("VICTORY_CULTURAL".into(), 1)];
        assert!(settings(&s, &c).is_ok());
        s.game_options = vec![("VICTORY_TIME".into(), 2)];
        assert!(settings(&s, &c).unwrap_err().contains("bad value"));
        s.game_options = vec![("VICTORY_SCRAP".into(), 1)];
        assert!(settings(&s, &c).unwrap_err().contains("unknown victory"));
        s.game_options = vec![("VICTORY_TIME".into(), 0), ("VICTORY_TIME".into(), 1)];
        assert!(settings(&s, &c).unwrap_err().contains("twice"));
    }

    #[test]
    fn max_turns_is_zero_to_the_limit() {
        let c = choices();
        let mut s = good();
        for ok in [0, 1, 300, MAX_TURNS_LIMIT] {
            s.game_options = vec![(MAX_TURNS_KEY.into(), ok)];
            assert!(settings(&s, &c).is_ok(), "{ok}");
        }
        for bad in [-1, MAX_TURNS_LIMIT + 1, i32::MAX] {
            s.game_options = vec![(MAX_TURNS_KEY.into(), bad)];
            assert!(settings(&s, &c).unwrap_err().contains("max turns"), "{bad}");
        }
    }

    #[test]
    fn map_options_belong_to_the_script_and_carry_a_listed_value() {
        let c = choices();
        let mut s = good();
        s.map_options = vec![("1".into(), 3), ("2".into(), 0)];
        assert!(settings(&s, &c).is_ok());
        // An id of another script, an unknown id.
        s.map_options = vec![("9".into(), 1)];
        assert!(settings(&s, &c).unwrap_err().contains("unknown map option"));
        s.map_options = vec![("7".into(), 1)];
        assert!(settings(&s, &c).is_err());
        // A value outside the list, 0 and 4 for a list from 1; 2 for a check box.
        for (id, v) in [("1", 0), ("1", 4), ("2", 2), ("2", -1)] {
            s.map_options = vec![(id.into(), v)];
            assert!(
                settings(&s, &c).unwrap_err().contains("bad value"),
                "{id}={v}"
            );
        }
        // A read-only option keeps its default.
        s.map_options = vec![("3".into(), 2)];
        assert!(settings(&s, &c).is_ok());
        s.map_options = vec![("3".into(), 1)];
        assert!(settings(&s, &c).unwrap_err().contains("cannot be changed"));
        s.map_options = vec![("1".into(), 1), ("1".into(), 2)];
        assert!(settings(&s, &c).unwrap_err().contains("twice"));
        // The same id is fine on the other script, which has it.
        let mut o = good();
        o.map_script = "map".into();
        o.map_options = vec![("9".into(), 1)];
        assert!(settings(&o, &c).is_err());
    }

    #[test]
    fn city_states_stay_within_the_size() {
        let c = choices();
        let mut s = good();
        s.minor_civs = 2;
        assert!(settings(&s, &c).is_ok());
        s.minor_civs = 3;
        assert!(settings(&s, &c).unwrap_err().contains("city-states"));
    }

    #[test]
    fn a_new_room_starts_with_the_defaults_of_its_script() {
        let c = choices();
        let mut s = good();
        fill_map_options(&mut s, None, &c);
        assert_eq!(
            s.map_options,
            vec![("1".into(), 2), ("2".into(), 1), ("3".into(), 2)]
        );
        assert!(settings(&s, &c).is_ok());
        // What the creator chose stays, the rest is filled in.
        let mut s = good();
        s.map_options = vec![("1".into(), 3)];
        fill_map_options(&mut s, None, &c);
        assert_eq!(
            s.map_options,
            vec![("1".into(), 3), ("2".into(), 1), ("3".into(), 2)]
        );
        // A script the lists have no options for gets none.
        let mut s = good();
        s.map_script = "map-without-options".into();
        fill_map_options(&mut s, None, &c);
        assert!(s.map_options.is_empty());
    }

    #[test]
    fn changing_the_map_script_resets_the_options_to_the_new_defaults() {
        let c = choices();
        let mut s = good();
        s.map_options = vec![("1".into(), 3), ("2".into(), 0), ("3".into(), 2)];
        // Same script: kept.
        fill_map_options(&mut s, Some("map"), &c);
        assert_eq!(
            s.map_options,
            vec![("1".into(), 3), ("2".into(), 0), ("3".into(), 2)]
        );
        // The old options are sent along with the new script: dropped for the new defaults.
        s.map_script = "other".into();
        fill_map_options(&mut s, Some("map"), &c);
        assert_eq!(s.map_options, vec![("9".into(), 1)]);
        assert!(settings(&s, &c).is_ok());
        // Back to a script without options: empty.
        s.map_script = "map-without-options".into();
        fill_map_options(&mut s, Some("other"), &c);
        assert!(s.map_options.is_empty());
    }

    #[test]
    fn options_and_timers() {
        let c = choices();
        let mut s = good();
        s.game_options = vec![("QUICK_COMBAT".into(), 1)];
        assert!(settings(&s, &c).unwrap_err().contains("not allowed"));
        s.game_options = vec![("OPT".into(), 1)];
        assert!(settings(&s, &c).is_ok());
        s.game_options = vec![("OPT".into(), 5)];
        assert!(settings(&s, &c).is_err());
        s.game_options = vec![("NOPE".into(), 0)];
        assert!(settings(&s, &c).is_err());
        let mut s = good();
        s.timer = TimerMode::Fixed { seconds: 9 };
        assert!(settings(&s, &c).is_err());
        s.timer = TimerMode::Fixed { seconds: 3600 };
        assert!(settings(&s, &c).is_ok());
        s.timer = TimerMode::Original { row: "FAST".into() };
        assert!(settings(&s, &c).is_ok());
        s.timer = TimerMode::Original { row: "NOPE".into() };
        assert!(settings(&s, &c).is_err());
    }
}
