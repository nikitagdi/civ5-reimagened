//! The staging room options of the real core (login-lobby packet P2): the pick lists of `LobbyChoices` that come from
//! the game data (game options, victories, map script options, city-state count), made from the core's
//! [`SetupDescription`], and the other way round for a game start (`VICTORY_*` and `MAX_TURNS` leave `game_options`).
//!
//! The grouping follows the Lekmod multiplayer screen (`MPGameOptions.lua` of LEKMOD_v34.15): the rows of its
//! `AI_OPTIONS` list are the AI group, the rows of its `ADDITIONAL` list the additional group, a row the screen shows
//! (`Visible = 1` and `SupportsMultiplayer = 1`, plus the two parent switches) the general group, every other row is
//! hidden (valid, but only under "More").

use gamecore::{GameOptionEntry, MapOptionEntry, SetupDescription, same_script};
use protocol::{
    LobbyChoice, MAX_TURNS_KEY, MapOptionChoice, MapScriptOptions, OptionChoice, OptionGroup,
    VICTORY_PREFIX,
};

/// Game options the brain sets itself (a network game, the server's clock, the quick animations): never offered.
pub const BRAIN_OPTIONS: [&str; 8] = [
    "GAMEOPTION_PITBOSS",
    "GAMEOPTION_END_TURN_TIMER_ENABLED",
    "GAMEOPTION_QUICK_COMBAT",
    "GAMEOPTION_QUICK_MOVEMENT",
    "GAMEOPTION_DYNAMIC_TURNS",
    "GAMEOPTION_SIMULTANEOUS_TURNS",
    "GAMEOPTION_FIRSTMOVE",
    "GAMEOPTION_FAST_HAND",
];

/// The parent switch of the AI list in `MPGameOptions.lua`.
pub const AI_SWITCH: &str = "GAMEOPTION_AI_OPTIONS";
/// The parent switch of the additional list.
pub const ADDITIONAL_SWITCH: &str = "GAMEOPTION_ADDITIONAL";

/// The children of `GAMEOPTION_AI_OPTIONS` (the `AI*Box` controls of `MPGameOptions.lua`).
const AI_CHILDREN: [&str; 15] = [
    "GAMEOPTION_AI_GIMP_ALWAYS_WHITE_PEACE",
    "GAMEOPTION_AI_GIMP_NO_CULTURE",
    "GAMEOPTION_AI_GIMP_NO_BUILDING_SETTLERS",
    "GAMEOPTION_AI_GIMP_NO_WORLD_WONDER",
    "GAMEOPTION_AI_GIMP_NO_COUP",
    "GAMEOPTION_AI_GIMP_NO_RELIGION_FOUNDING",
    "GAMEOPTION_AI_GIMP_NO_MINOR_RELIGION_SPREAD",
    "GAMEOPTION_AI_GIMP_NO_RELIGION_SPREAD",
    "GAMEOPTION_AI_NO_VOTE",
    "GAMEOPTION_AI_GIMP_NO_LIBERATION",
    "GAMEOPTION_AI_GIMP_NO_DEALS",
    "GAMEOPTION_AI_GIMP_NO_DEMOGRAPHICS",
    "GAMEOPTION_AI_HANDICAP_START",
    "GAMEOPTION_DISABLE_RECKLESS_EXPANDER",
    "GAMEOPTION_AI_XP_CAP",
];

/// The children of `GAMEOPTION_ADDITIONAL`.
const ADDITIONAL_CHILDREN: [&str; 12] = [
    "GAMEOPTION_POLICY_SAVING",
    "GAMEOPTION_PROMOTION_SAVING",
    "GAMEOPTION_TWEAKED_SCORING",
    "GAMEOPTION_COMPLETE_KILLS",
    "GAMEOPTION_DISABLE_START_BIAS",
    "GAMEOPTION_NO_GOODY_HUTS",
    "GAMEOPTION_NO_BARBARIANS",
    "GAMEOPTION_NO_CITY_RAZING",
    "GAMEOPTION_NO_ESPIONAGE",
    "GAMEOPTION_ONE_CITY_CHALLENGE",
    "GAMEOPTION_RAGING_BARBARIANS",
    "GAMEOPTION_RANDOM_PERSONALITIES",
];

/// The most city-states the core takes (`MAX_MINOR_CIVS`, `setup_json.minor_civs`).
// SIMPLIFIED(SIMP-exe-852): the slider ends here, not at the number of MinorCivilizations rows.
pub const CORE_MAX_MINOR: u8 = 41;

/// The `Civilizations` rows of the shipped game data whose `Playable` is 0: the city-state and the barbarian
/// pseudo-civilizations and Venice (the scenario civilization). The staging room's pull-down lists the rows with
/// `Playable = 1` only.
// SIMPLIFIED(SIMP-exe-1060): the core's civilization catalog carries no `Playable` column, so the rows of the Lekmod
// data are named here (the asset set `web-lobby` reads the column itself).
pub const NON_PLAYABLE_CIVS: [&str; 3] = [
    "CIVILIZATION_MINOR",
    "CIVILIZATION_BARBARIAN",
    "CIVILIZATION_VENICE",
];

/// Does the staging room list the civilization (`Playable = 1`)?
#[must_use]
pub fn playable_civ(type_name: &str) -> bool {
    !NON_PLAYABLE_CIVS.contains(&type_name)
}

/// The victory that is not a choice of the room (`MPGameOptions.lua` skips it).
const SCRAPPED_VICTORY: &str = "VICTORY_SCRAP";

/// The room group of a game option row.
// SIMPLIFIED(SIMP-exe-851): a row the Lekmod screen does not show is offered under "More" (Hidden), and the brain's own rows are never offered.
#[must_use]
pub fn group_of(e: &GameOptionEntry) -> OptionGroup {
    let t = e.type_name.as_str();
    if AI_CHILDREN.contains(&t) {
        OptionGroup::Ai
    } else if ADDITIONAL_CHILDREN.contains(&t) {
        OptionGroup::Additional
    } else if t == AI_SWITCH || t == ADDITIONAL_SWITCH || (e.visible && e.supports_multiplayer) {
        OptionGroup::General
    } else {
        OptionGroup::Hidden
    }
}

/// Is this `game_options` key a game option of the core (neither a victory nor the max turns)?
#[must_use]
pub fn is_core_option(key: &str) -> bool {
    key != MAX_TURNS_KEY && !key.starts_with(VICTORY_PREFIX)
}

/// The game options the room offers: every row but the brain's own, grouped, in the order of the table within a group
/// (general first).
#[must_use]
pub fn options_of(d: &SetupDescription) -> Vec<OptionChoice> {
    let mut out: Vec<OptionChoice> = d
        .game_options
        .iter()
        .filter(|e| !BRAIN_OPTIONS.contains(&e.type_name.as_str()))
        .map(|e| OptionChoice {
            key: e.type_name.clone(),
            name: e.name.clone(),
            default: i32::from(e.default),
            values: Vec::new(),
            help: e.help.clone(),
            group: group_of(e),
        })
        .collect();
    let rank = |g: OptionGroup| match g {
        OptionGroup::General => 0,
        OptionGroup::Ai => 1,
        OptionGroup::Additional => 2,
        OptionGroup::Hidden => 3,
    };
    out.sort_by_key(|o| rank(o.group));
    out
}

/// The victory conditions the room offers (`Victories` without `VICTORY_SCRAP`).
#[must_use]
pub fn victories_of(d: &SetupDescription) -> Vec<LobbyChoice> {
    d.victories
        .iter()
        .filter(|v| v.type_name != SCRAPPED_VICTORY)
        .map(|v| LobbyChoice {
            type_name: v.type_name.clone(),
            name: v.name.clone(),
        })
        .collect()
}

fn choice_of(d: &SetupDescription, o: &MapOptionEntry) -> MapOptionChoice {
    let mut values: Vec<(i32, i32, String)> = d
        .map_option_values
        .iter()
        .filter(|v| v.option_id == o.option_id && same_script(&v.script, &o.script))
        .map(|v| (v.sort_index, v.value, v.name.clone()))
        .collect();
    values.sort_by_key(|v| v.0);
    MapOptionChoice {
        id: o.option_id.clone(),
        name: o.name.clone(),
        help: o.help.clone(),
        default: o.default_value,
        values: values.into_iter().map(|(_, v, n)| (v, n)).collect(),
        read_only: o.read_only,
    }
}

/// The custom options of each offered map script, as the original screen sorts them (`SortPriority`, then name). A
/// hidden option is not offered (its default applies in the core). A script with no options is left out.
// SIMPLIFIED(SIMP-exe-850): the options come from the game data tables the exe filled, not from running GetMapScriptInfo().
#[must_use]
pub fn map_options_of(d: &SetupDescription, scripts: &[LobbyChoice]) -> Vec<MapScriptOptions> {
    scripts
        .iter()
        .filter_map(|s| {
            let mut own: Vec<&MapOptionEntry> = d
                .map_options
                .iter()
                .filter(|o| !o.hidden && same_script(&o.script, &s.type_name))
                .collect();
            if own.is_empty() {
                return None;
            }
            own.sort_by(|a, b| {
                a.sort_priority
                    .cmp(&b.sort_priority)
                    .then_with(|| a.name.cmp(&b.name))
            });
            Some(MapScriptOptions {
                script: s.type_name.clone(),
                options: own.into_iter().map(|o| choice_of(d, o)).collect(),
            })
        })
        .collect()
}

/// The city-state count of the game data, for `LobbyChoices.max_minor_civs`.
#[must_use]
pub fn minor_civ_count(d: &SetupDescription) -> u8 {
    u8::try_from(d.minor_civs.len()).unwrap_or(u8::MAX)
}

/// `WorldSizeChoice.max_minor`: all the game data's city-states, as far as the core takes them.
#[must_use]
pub fn max_minor_of(minor_civs: u8) -> u8 {
    minor_civs.min(CORE_MAX_MINOR)
}

/// `game_options` split for the core.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SplitOptions {
    /// The game options of the core (`setup_json.game_options`).
    pub core: Vec<(String, i32)>,
    /// `VICTORY_*` -> 0/1 (`setup_json.victories`).
    pub victories: Vec<(String, i32)>,
    /// `MAX_TURNS` (`setup_json.max_turns`), 0 = none.
    pub max_turns: u32,
}

/// Splits `game_options`: the `VICTORY_*` entries and `MAX_TURNS` go to their own setup fields.
#[must_use]
pub fn split_game_options(options: &[(String, i32)]) -> SplitOptions {
    let mut out = SplitOptions::default();
    for (k, v) in options {
        if k == MAX_TURNS_KEY {
            out.max_turns = u32::try_from(*v).unwrap_or(0);
        } else if k.starts_with(VICTORY_PREFIX) {
            out.victories.push((k.clone(), i32::from(*v != 0)));
        } else {
            out.core.push((k.clone(), *v));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use gamecore::{MapOptionValueEntry, NamedEntry};

    #[test]
    fn the_room_lists_playable_civs_only() {
        for t in [
            "CIVILIZATION_MINOR",
            "CIVILIZATION_BARBARIAN",
            "CIVILIZATION_VENICE",
        ] {
            assert!(!playable_civ(t), "{t}");
        }
        for t in [
            "CIVILIZATION_SPAIN",
            "CIVILIZATION_ROME",
            "CIVILIZATION_A",
            "",
        ] {
            assert!(playable_civ(t), "{t}");
        }
    }

    fn game_option(t: &str, default: bool, visible: bool, mp: bool) -> GameOptionEntry {
        GameOptionEntry {
            id: 0,
            type_name: t.into(),
            name: t.into(),
            help: String::new(),
            default,
            visible,
            supports_multiplayer: mp,
        }
    }

    fn map_option(script: &str, id: &str, name: &str, sort: i32, hidden: bool) -> MapOptionEntry {
        MapOptionEntry {
            script: script.into(),
            option_id: id.into(),
            name: name.into(),
            help: String::new(),
            sort_priority: sort,
            default_value: 2,
            read_only: false,
            hidden,
        }
    }

    fn value(script: &str, id: &str, name: &str, v: i32) -> MapOptionValueEntry {
        MapOptionValueEntry {
            script: script.into(),
            option_id: id.into(),
            name: name.into(),
            value: v,
            sort_index: v,
        }
    }

    #[test]
    fn groups_follow_the_lekmod_screen() {
        let d = SetupDescription {
            game_options: vec![
                game_option("GAMEOPTION_NO_BARBARIANS", false, true, false),
                game_option("GAMEOPTION_AI_GIMP_NO_CULTURE", false, true, false),
                game_option("GAMEOPTION_AI_OPTIONS", false, false, true),
                game_option("GAMEOPTION_ADDITIONAL", false, false, true),
                game_option("GAMEOPTION_SPOILS_OF_WAR", false, true, true),
                game_option("GAMEOPTION_NO_SCIENCE", false, false, true),
                game_option("GAMEOPTION_NEW_RANDOM_SEED", false, true, false),
                game_option("GAMEOPTION_PITBOSS", true, true, true),
                game_option("GAMEOPTION_QUICK_COMBAT", false, true, true),
                game_option("GAMEOPTION_FIRSTMOVE", false, true, true),
            ],
            ..SetupDescription::default()
        };
        let o = options_of(&d);
        let g = |k: &str| o.iter().find(|x| x.key == k).map(|x| x.group);
        assert_eq!(g("GAMEOPTION_NO_BARBARIANS"), Some(OptionGroup::Additional));
        assert_eq!(g("GAMEOPTION_AI_GIMP_NO_CULTURE"), Some(OptionGroup::Ai));
        assert_eq!(g("GAMEOPTION_AI_OPTIONS"), Some(OptionGroup::General));
        assert_eq!(g("GAMEOPTION_ADDITIONAL"), Some(OptionGroup::General));
        assert_eq!(g("GAMEOPTION_SPOILS_OF_WAR"), Some(OptionGroup::General));
        assert_eq!(g("GAMEOPTION_NO_SCIENCE"), Some(OptionGroup::Hidden));
        assert_eq!(g("GAMEOPTION_NEW_RANDOM_SEED"), Some(OptionGroup::Hidden));
        // The brain's own options are not offered.
        for k in BRAIN_OPTIONS {
            assert_eq!(g(k), None, "{k}");
        }
        // General first, hidden last.
        let groups: Vec<_> = o.iter().map(|x| x.group).collect();
        let mut sorted = groups.clone();
        sorted.sort_by_key(|g| match g {
            OptionGroup::General => 0,
            OptionGroup::Ai => 1,
            OptionGroup::Additional => 2,
            OptionGroup::Hidden => 3,
        });
        assert_eq!(groups, sorted);
    }

    #[test]
    fn defaults_and_checkboxes() {
        let d = SetupDescription {
            game_options: vec![game_option(
                "GAMEOPTION_ALLOW_EXTRA_RELIGIONS",
                true,
                true,
                true,
            )],
            ..SetupDescription::default()
        };
        let o = options_of(&d);
        assert_eq!(o[0].default, 1);
        assert!(o[0].values.is_empty());
    }

    #[test]
    fn victories_skip_scrap() {
        let n = |t: &str| NamedEntry {
            id: 0,
            type_name: t.into(),
            name: t.into(),
        };
        let d = SetupDescription {
            victories: vec![n("VICTORY_TIME"), n("VICTORY_SCRAP"), n("VICTORY_CULTURAL")],
            ..SetupDescription::default()
        };
        let v: Vec<_> = victories_of(&d).into_iter().map(|c| c.type_name).collect();
        assert_eq!(v, ["VICTORY_TIME", "VICTORY_CULTURAL"]);
    }

    #[test]
    fn map_options_per_script_sorted_and_hidden_left_out() {
        let a = "Assets\\Maps\\A.lua";
        let b = "Assets\\Maps\\B.lua";
        let d = SetupDescription {
            map_options: vec![
                map_option(a, "2", "Zeta", -90, false),
                map_option(a, "1", "Rainfall", -90, false),
                map_option(a, "3", "Age", -99, false),
                map_option(a, "4", "Secret", -100, true),
                map_option(b, "1", "Other", 0, false),
            ],
            map_option_values: vec![
                value(a, "1", "Wet", 2),
                value(a, "1", "Dry", 1),
                value(a, "2", "Z", 1),
                value(b, "1", "B-only", 1),
            ],
            ..SetupDescription::default()
        };
        let scripts = [
            LobbyChoice {
                type_name: "assets/maps/a.lua".into(),
                name: "A".into(),
            },
            LobbyChoice {
                type_name: "assets/maps/none.lua".into(),
                name: "None".into(),
            },
        ];
        let m = map_options_of(&d, &scripts);
        assert_eq!(m.len(), 1, "the script without options is left out");
        assert_eq!(m[0].script, "assets/maps/a.lua");
        let names: Vec<_> = m[0].options.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["Age", "Rainfall", "Zeta"], "priority, then name");
        let rain = &m[0].options[1];
        assert_eq!(rain.id, "1");
        assert_eq!(
            rain.values,
            vec![(1, "Dry".to_owned()), (2, "Wet".to_owned())],
            "values of this script only, in order"
        );
        assert!(
            m[0].options[0].values.is_empty(),
            "no value rows: a check box"
        );
    }

    #[test]
    fn minor_civ_limits() {
        assert_eq!(max_minor_of(58), CORE_MAX_MINOR);
        assert_eq!(max_minor_of(12), 12);
        let n = NamedEntry {
            id: 0,
            type_name: String::new(),
            name: String::new(),
        };
        let d = SetupDescription {
            minor_civs: vec![n; 58],
            ..SetupDescription::default()
        };
        assert_eq!(minor_civ_count(&d), 58);
    }

    #[test]
    fn victories_and_max_turns_leave_game_options() {
        let opts = vec![
            ("GAMEOPTION_NO_BARBARIANS".to_owned(), 1),
            ("VICTORY_TIME".to_owned(), 0),
            ("MAX_TURNS".to_owned(), 300),
            ("VICTORY_CULTURAL".to_owned(), 1),
        ];
        let split = split_game_options(&opts);
        assert_eq!(split.core, vec![("GAMEOPTION_NO_BARBARIANS".to_owned(), 1)]);
        assert_eq!(
            split.victories,
            vec![
                ("VICTORY_TIME".to_owned(), 0),
                ("VICTORY_CULTURAL".to_owned(), 1)
            ]
        );
        assert_eq!(split.max_turns, 300);
        assert!(is_core_option("GAMEOPTION_X") && !is_core_option("MAX_TURNS"));
        assert!(!is_core_option("VICTORY_TIME"));
        // A negative value never becomes a huge limit.
        assert_eq!(
            split_game_options(&[("MAX_TURNS".to_owned(), -4)]).max_turns,
            0
        );
    }
}
