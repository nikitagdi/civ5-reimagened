//! Typed game setup (gc-abi-v2 4.1) and its serialization to the `setup_json` of
//! `gc_new_game`.
//!
//! Defaults are the B1 game (`game_type` single); the v2 core would default to a network game, so
//! `game_type` is always written. The rest of the keys B1 had are always written, so the JSON does not
//! depend on the core's defaults; the v2-only keys are written when in use.

use std::fmt::Write as _;

use crate::types::TurnMode;

/// Which minor civilizations (city-states) the game gets.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MinorCivs {
    /// The default count of the world size.
    #[default]
    Default,
    /// This many, chosen from the seed.
    Count(u32),
    /// Exactly these `MINOR_CIV_*` types.
    List(Vec<String>),
}

/// Which decisions the core makes itself (1) or leaves to the UI (0). 1 for all is the B1 mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoChoices {
    pub research: i32,
    pub policy: i32,
    pub ideology: i32,
    pub religion: i32,
    pub promotion: i32,
    pub great_person: i32,
    pub captured_city: i32,
    pub league: i32,
    pub archaeology: i32,
    pub goody_hut: i32,
    pub steal_tech: i32,
}

impl Default for AutoChoices {
    fn default() -> Self {
        AutoChoices {
            research: 1,
            policy: 1,
            ideology: 1,
            religion: 1,
            promotion: 1,
            great_person: 1,
            captured_city: 1,
            league: 1,
            archaeology: 1,
            goody_hut: 1,
            steal_tech: 1,
        }
    }
}

/// Which kind of game the core starts (`setup_json.game_type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GameType {
    /// The B1 game: one human in slot 0 (or an observer), no server seat. The default of `Setup`,
    /// so B1 callers and the stub keep working.
    #[default]
    Single,
    /// The multiplayer game of B2: 1 to 8 humans, Pitboss, a server seat.
    Network,
}

impl GameType {
    /// The `setup_json` value.
    pub fn name(self) -> &'static str {
        match self {
            GameType::Single => "single",
            GameType::Network => "network",
        }
    }
}

/// Who plays a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotKind {
    Human,
    Ai,
}

/// One slot of `Setup::players`; the slot number is its index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotSetup {
    pub kind: SlotKind,
    /// `CIVILIZATION_*` type, or `"random"`.
    pub civ: String,
    /// The human's nickname, UTF-8, up to 64 bytes; humans only (the core refuses it on an AI).
    pub nick: Option<String>,
}

impl SlotSetup {
    pub fn human(civ: impl Into<String>, nick: impl Into<String>) -> Self {
        SlotSetup {
            kind: SlotKind::Human,
            civ: civ.into(),
            nick: Some(nick.into()),
        }
    }

    pub fn ai(civ: impl Into<String>) -> Self {
        SlotSetup {
            kind: SlotKind::Ai,
            civ: civ.into(),
            nick: None,
        }
    }
}

/// Game setup. Two ways to name the slots: the B1 form (`human_civ` and `ai_count`: one human in
/// slot 0, then the AIs) when `players` is empty, or the `players` list. The core refuses both at
/// once, so `to_json` writes only the one in use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setup {
    pub seed: u32,
    /// `None` = the core uses `seed`.
    pub map_seed: Option<u32>,
    /// `Single` (the default here) is the B1 game; `Network` is the multiplayer game.
    pub game_type: GameType,
    /// Slots in order (slot number = index). Empty = the B1 form below.
    pub players: Vec<SlotSetup>,
    /// How the humans take turns; written for a network game and when it is not the default.
    pub turn_mode: TurnMode,
    /// `TurnTimers` row type for `Q_GAME.turn_len_s`; `None` = `"none"`. Written for a network
    /// game and when set.
    pub turn_timer: Option<String>,
    /// Slots played by the test bot `rt_autoplay` (tests only). Empty = none.
    pub autoplay_humans: Vec<u8>,
    /// B1 form: `CIVILIZATION_*` type of the human, or `"random"`. Ignored when `players` is set.
    pub human_civ: String,
    /// B1 form: AI major civilizations (the human is not counted), 0..=62 in the core's limits.
    /// Ignored when `players` is set.
    pub ai_count: u32,
    /// Not written when `world_builder_map` is set: the map decides the size.
    pub world_size: String,
    /// Civ5 VFS path with `/` separators.
    pub map_script: String,
    /// Map script option id -> value.
    pub map_options: Vec<(String, i32)>,
    pub handicap: String,
    pub game_speed: String,
    pub start_era: String,
    pub minor_civs: MinorCivs,
    /// Game option name -> value.
    pub game_options: Vec<(String, i32)>,
    /// Victory conditions: `VICTORY_*` type -> 1 (on) or 0 (off); a victory not listed stays on. Written only when
    /// not empty (`setup_json.victories`).
    pub victories: Vec<(String, i32)>,
    /// `PreGame.SetMaxTurns`; 0 = the game speed's default. Written only when not 0 (`setup_json.max_turns`).
    pub max_turns: u32,
    pub auto_choices: AutoChoices,
    pub diplo_auto_decline: i32,
    /// 1 = the AI plays the human slot too.
    pub autoplay_human: i32,
    /// 1 = an observer slot is the active player.
    pub observer: i32,
    /// A `.Civ5Map` file of the game's VFS instead of a map script.
    pub world_builder_map: Option<String>,
    /// With `world_builder_map`: load the scenario data of the map too (cities, units ...).
    pub world_builder_scenario: bool,
}

impl Default for Setup {
    fn default() -> Self {
        Setup {
            seed: 12345,
            map_seed: None,
            game_type: GameType::Single,
            players: Vec::new(),
            turn_mode: TurnMode::default(),
            turn_timer: None,
            autoplay_humans: Vec::new(),
            human_civ: "CIVILIZATION_ROME".into(),
            ai_count: 1,
            world_size: "WORLDSIZE_DUEL".into(),
            map_script: "Assets/Maps/Lekmap v5.2/LekmapPangaeaFractalv5.2.lua".into(),
            map_options: Vec::new(),
            handicap: "HANDICAP_PRINCE".into(),
            game_speed: "GAMESPEED_STANDARD".into(),
            start_era: "ERA_ANCIENT".into(),
            minor_civs: MinorCivs::Default,
            game_options: Vec::new(),
            victories: Vec::new(),
            max_turns: 0,
            auto_choices: AutoChoices::default(),
            diplo_auto_decline: 1,
            autoplay_human: 0,
            observer: 0,
            world_builder_map: None,
            world_builder_scenario: false,
        }
    }
}

impl Setup {
    /// Default setup with a seed (the map seed follows it).
    pub fn with_seed(seed: u32) -> Self {
        Setup {
            seed,
            ..Setup::default()
        }
    }

    /// A network game with these slots, hybrid turns and no timer; the rest
    /// is the default.
    pub fn network(seed: u32, players: Vec<SlotSetup>) -> Self {
        Setup {
            seed,
            game_type: GameType::Network,
            players,
            ..Setup::default()
        }
    }

    /// The `setup_json` text. Keys of v2 that the B1 game does not use are written only when they
    /// are in use, so a B1 setup is the same text as before and the B1-only stub accepts it.
    pub fn to_json(&self) -> String {
        let mut o = String::with_capacity(512);
        o.push('{');
        // The v2 core defaults to a network game (gc-abi-v2 4.1); always say which one this is.
        let _ = write!(o, "\"game_type\":\"{}\",", self.game_type.name());
        let _ = write!(o, "\"seed\":{}", self.seed);
        if let Some(m) = self.map_seed {
            let _ = write!(o, ",\"map_seed\":{m}");
        }
        if self.players.is_empty() {
            o.push_str(",\"human\":{\"civ\":");
            json_str(&mut o, &self.human_civ);
            let _ = write!(o, "}},\"ai_count\":{}", self.ai_count);
        } else {
            o.push_str(",\"players\":[");
            for (i, p) in self.players.iter().enumerate() {
                if i > 0 {
                    o.push(',');
                }
                o.push_str(match p.kind {
                    SlotKind::Human => "{\"kind\":\"human\",\"civ\":",
                    SlotKind::Ai => "{\"kind\":\"ai\",\"civ\":",
                });
                json_str(&mut o, &p.civ);
                if let Some(nick) = &p.nick {
                    o.push_str(",\"nick\":");
                    json_str(&mut o, nick);
                }
                o.push('}');
            }
            o.push(']');
        }
        let network = self.game_type == GameType::Network;
        if network || self.turn_mode != TurnMode::default() {
            let _ = write!(o, ",\"turn_mode\":\"{}\"", self.turn_mode.name());
        }
        if network || self.turn_timer.is_some() {
            o.push_str(",\"turn_timer\":");
            json_str(&mut o, self.turn_timer.as_deref().unwrap_or("none"));
        }
        // A world builder map decides the world size; the core refuses both.
        if self.world_builder_map.is_none() {
            o.push_str(",\"world_size\":");
            json_str(&mut o, &self.world_size);
        }
        o.push_str(",\"map_script\":");
        json_str(&mut o, &self.map_script);
        o.push_str(",\"map_options\":");
        json_options(&mut o, &self.map_options);
        o.push_str(",\"handicap\":");
        json_str(&mut o, &self.handicap);
        o.push_str(",\"game_speed\":");
        json_str(&mut o, &self.game_speed);
        o.push_str(",\"start_era\":");
        json_str(&mut o, &self.start_era);
        o.push_str(",\"minor_civs\":");
        match &self.minor_civs {
            MinorCivs::Default => o.push_str("\"default\""),
            MinorCivs::Count(n) => {
                let _ = write!(o, "{n}");
            }
            MinorCivs::List(list) => {
                o.push('[');
                for (i, t) in list.iter().enumerate() {
                    if i > 0 {
                        o.push(',');
                    }
                    json_str(&mut o, t);
                }
                o.push(']');
            }
        }
        o.push_str(",\"game_options\":");
        json_options(&mut o, &self.game_options);
        let a = &self.auto_choices;
        let _ = write!(
            o,
            ",\"auto_choices\":{{\"research\":{},\"policy\":{},\"ideology\":{},\"religion\":{},\
             \"promotion\":{},\"great_person\":{},\"captured_city\":{},\"league\":{},\
             \"archaeology\":{},\"goody_hut\":{},\"steal_tech\":{}}}",
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
        );
        let _ = write!(
            o,
            ",\"diplo_auto_decline\":{},\"autoplay_human\":{},\"observer\":{}}}",
            self.diplo_auto_decline, self.autoplay_human, self.observer
        );
        if !self.autoplay_humans.is_empty() {
            o.pop();
            o.push_str(",\"autoplay_humans\":[");
            for (i, s) in self.autoplay_humans.iter().enumerate() {
                if i > 0 {
                    o.push(',');
                }
                let _ = write!(o, "{s}");
            }
            o.push_str("]}");
        }
        if !self.victories.is_empty() || self.max_turns != 0 {
            o.pop();
            if !self.victories.is_empty() {
                o.push_str(",\"victories\":");
                json_options(&mut o, &self.victories);
            }
            if self.max_turns != 0 {
                let _ = write!(o, ",\"max_turns\":{}", self.max_turns);
            }
            o.push('}');
        }
        if let Some(map) = &self.world_builder_map {
            o.pop();
            o.push_str(",\"world_builder_map\":");
            json_str(&mut o, map);
            let _ = write!(
                o,
                ",\"world_builder_scenario\":{}}}",
                i32::from(self.world_builder_scenario)
            );
        }
        o
    }
}

fn json_options(o: &mut String, list: &[(String, i32)]) {
    o.push('{');
    for (i, (k, v)) in list.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        json_str(o, k);
        let _ = write!(o, ":{v}");
    }
    o.push('}');
}

/// JSON string literal with the escapes JSON requires.
fn json_str(o: &mut String, s: &str) {
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_json_is_the_spec_example_shape() {
        let j = Setup::default().to_json();
        assert_eq!(
            j,
            "{\"game_type\":\"single\",\"seed\":12345,\"human\":{\"civ\":\"CIVILIZATION_ROME\"},\"ai_count\":1,\
             \"world_size\":\"WORLDSIZE_DUEL\",\
             \"map_script\":\"Assets/Maps/Lekmap v5.2/LekmapPangaeaFractalv5.2.lua\",\
             \"map_options\":{},\"handicap\":\"HANDICAP_PRINCE\",\
             \"game_speed\":\"GAMESPEED_STANDARD\",\"start_era\":\"ERA_ANCIENT\",\
             \"minor_civs\":\"default\",\"game_options\":{},\
             \"auto_choices\":{\"research\":1,\"policy\":1,\"ideology\":1,\"religion\":1,\
             \"promotion\":1,\"great_person\":1,\"captured_city\":1,\"league\":1,\
             \"archaeology\":1,\"goody_hut\":1,\"steal_tech\":1},\
             \"diplo_auto_decline\":1,\"autoplay_human\":0,\"observer\":0}"
        );
    }

    #[test]
    fn options_lists_and_escapes() {
        let s = Setup {
            map_seed: Some(7),
            minor_civs: MinorCivs::List(vec!["MINOR_CIV_A".into(), "MINOR_CIV_B".into()]),
            map_options: vec![("Water \"level\"".into(), 2)],
            game_options: vec![("GAMEOPTION_X".into(), 1)],
            human_civ: "random".into(),
            ..Setup::default()
        };
        let j = s.to_json();
        assert!(j.contains("\"map_seed\":7"));
        assert!(j.contains("\"minor_civs\":[\"MINOR_CIV_A\",\"MINOR_CIV_B\"]"));
        assert!(j.contains("\"map_options\":{\"Water \\\"level\\\"\":2}"));
        assert!(j.contains("\"game_options\":{\"GAMEOPTION_X\":1}"));
        let n = Setup {
            minor_civs: MinorCivs::Count(3),
            ..Setup::default()
        };
        assert!(n.to_json().contains("\"minor_civs\":3,"));
    }

    #[test]
    fn network_setup_names_every_slot_and_the_turn_keys() {
        let s = Setup {
            map_seed: Some(5),
            turn_mode: TurnMode::Simultaneous,
            turn_timer: Some("TURNTIMER_BLITZ".into()),
            autoplay_humans: vec![1, 2],
            ..Setup::network(
                7,
                vec![
                    SlotSetup::human("CIVILIZATION_ROME", "alice"),
                    SlotSetup::human("random", "fr\"iend"),
                    SlotSetup::ai("random"),
                ],
            )
        };
        assert_eq!(
            s.to_json(),
            "{\"game_type\":\"network\",\"seed\":7,\"map_seed\":5,\
             \"players\":[{\"kind\":\"human\",\"civ\":\"CIVILIZATION_ROME\",\"nick\":\"alice\"},\
             {\"kind\":\"human\",\"civ\":\"random\",\"nick\":\"fr\\\"iend\"},\
             {\"kind\":\"ai\",\"civ\":\"random\"}],\
             \"turn_mode\":\"simultaneous\",\"turn_timer\":\"TURNTIMER_BLITZ\",\
             \"world_size\":\"WORLDSIZE_DUEL\",\
             \"map_script\":\"Assets/Maps/Lekmap v5.2/LekmapPangaeaFractalv5.2.lua\",\
             \"map_options\":{},\"handicap\":\"HANDICAP_PRINCE\",\
             \"game_speed\":\"GAMESPEED_STANDARD\",\"start_era\":\"ERA_ANCIENT\",\
             \"minor_civs\":\"default\",\"game_options\":{},\
             \"auto_choices\":{\"research\":1,\"policy\":1,\"ideology\":1,\"religion\":1,\
             \"promotion\":1,\"great_person\":1,\"captured_city\":1,\"league\":1,\
             \"archaeology\":1,\"goody_hut\":1,\"steal_tech\":1},\
             \"diplo_auto_decline\":1,\"autoplay_human\":0,\"observer\":0,\
             \"autoplay_humans\":[1,2]}"
        );
        // The B1 keys are gone: the core refuses both forms together.
        let j = s.to_json();
        assert!(!j.contains("\"human\":{") && !j.contains("ai_count"));
    }

    #[test]
    fn network_defaults_are_written_and_single_defaults_are_not() {
        let n = Setup::network(1, vec![SlotSetup::human("random", "a")]).to_json();
        assert!(
            n.contains("\"turn_mode\":\"hybrid\",\"turn_timer\":\"none\","),
            "{n}"
        );
        assert!(!n.contains("autoplay_humans") && !n.contains("world_builder"));
        // A B1 game stays free of the v2 keys the stub refuses.
        let b1 = Setup::default().to_json();
        for key in [
            "players",
            "turn_mode",
            "turn_timer",
            "autoplay_humans",
            "world_builder",
        ] {
            assert!(!b1.contains(key), "{key}");
        }
        // But a non-default value is written even in a single game.
        let s = Setup {
            turn_mode: TurnMode::Simultaneous,
            ..Setup::default()
        }
        .to_json();
        assert!(s.contains("\"turn_mode\":\"simultaneous\""));
    }

    #[test]
    fn world_builder_map_replaces_the_world_size() {
        let s = Setup {
            world_builder_map: Some("Tutorials/Maps/LateGameTest.Civ5Map".into()),
            world_builder_scenario: true,
            ..Setup::default()
        }
        .to_json();
        assert!(!s.contains("world_size"), "{s}");
        assert!(s.ends_with(
            ",\"world_builder_map\":\"Tutorials/Maps/LateGameTest.Civ5Map\",\"world_builder_scenario\":1}"
        ));
        let both = Setup {
            world_builder_map: Some("a.Civ5Map".into()),
            autoplay_humans: vec![0],
            ..Setup::default()
        }
        .to_json();
        assert!(both.ends_with(
            "\"autoplay_humans\":[0],\"world_builder_map\":\"a.Civ5Map\",\"world_builder_scenario\":0}"
        ));
    }

    #[test]
    fn lobby_options_are_written_only_when_set() {
        // The default text is the same as before the keys existed.
        let j = Setup::default().to_json();
        assert!(!j.contains("victories") && !j.contains("max_turns"), "{j}");
        let s = Setup {
            victories: vec![("VICTORY_TIME".into(), 0), ("VICTORY_CULTURAL".into(), 1)],
            max_turns: 300,
            ..Setup::default()
        }
        .to_json();
        assert!(
            s.ends_with(
                ",\"victories\":{\"VICTORY_TIME\":0,\"VICTORY_CULTURAL\":1},\"max_turns\":300}"
            ),
            "{s}"
        );
        let only_turns = Setup {
            max_turns: 50,
            ..Setup::default()
        }
        .to_json();
        assert!(
            only_turns.ends_with("\"observer\":0,\"max_turns\":50}"),
            "{only_turns}"
        );
        // After autoplay_humans and before a world builder map, each closing the object itself.
        let all = Setup {
            victories: vec![("VICTORY_TIME".into(), 0)],
            autoplay_humans: vec![0],
            world_builder_map: Some("a.Civ5Map".into()),
            ..Setup::default()
        }
        .to_json();
        assert!(
            all.ends_with(
                "\"autoplay_humans\":[0],\"victories\":{\"VICTORY_TIME\":0},\"world_builder_map\":\"a.Civ5Map\",\"world_builder_scenario\":0}"
            ),
            "{all}"
        );
    }

    #[test]
    fn control_characters_are_escaped() {
        let mut o = String::new();
        json_str(&mut o, "a\u{1}b\n");
        assert_eq!(o, "\"a\\u0001b\\n\"");
    }
}
