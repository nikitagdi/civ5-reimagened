//! The staging room options of the real core (login-lobby packet P2): `Describe` (`RealCore::choices`) lists the game
//! options, victories, city-states and the custom options of the map scripts out of the game data, and a game
//! starts with `victories` and `max_turns` in its setup. Returns early on the stub; run on Linux/WSL with
//! `GAMECORE_LIB_DIR_x86_64` set (`cargo xtask test -p game-host --gamecore-lib <dir> --test real_options`) and
//! the game data in `CIV5_GAMEDATA`.
//!
//! The real core is a process singleton: one test.

use game_host::lobby_options::BRAIN_OPTIONS;
use game_host::map_scripts::map_script_choices_in;
use game_host::{Core, CorePhase, CoreStart};
use protocol::OptionGroup;

#[path = "common/real.rs"]
mod common;
use common::{gamedata_dir, real_core, setup};

/// The lowercase index key of the script in `vfs-index.json`.
const PANGAEA: &str = "assets/maps/lekmap v5.2/lekmappangaeafractalv5.2.lua";

#[test]
fn real_core_lobby_options_and_victories() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let mut core = real_core().with_map_scripts(map_script_choices_in(&gamedata_dir()));
    let c = core.choices().expect("Describe");

    // Game options: the table of the game data without the brain's own, grouped as the Lekmod screen does.
    assert!(c.options.len() >= 40, "{} options", c.options.len());
    for brain in BRAIN_OPTIONS {
        assert!(c.options.iter().all(|o| o.key != brain), "{brain} offered");
    }
    let group = |key: &str| {
        c.options
            .iter()
            .find(|o| o.key == key)
            .unwrap_or_else(|| panic!("{key} not listed"))
            .group
    };
    // A parent switch and a general row are General; the rows under the two switches follow them; a row the
    // Lekmod screen does not show is Hidden.
    assert_eq!(group("GAMEOPTION_AI_OPTIONS"), OptionGroup::General);
    assert_eq!(group("GAMEOPTION_SPOILS_OF_WAR"), OptionGroup::General);
    assert_eq!(group("GAMEOPTION_AI_GIMP_NO_CULTURE"), OptionGroup::Ai);
    assert_eq!(group("GAMEOPTION_NO_BARBARIANS"), OptionGroup::Additional);
    assert_eq!(group("GAMEOPTION_NO_SCIENCE"), OptionGroup::Hidden);
    let ai = c
        .options
        .iter()
        .filter(|o| o.group == OptionGroup::Ai)
        .count();
    let extra = c
        .options
        .iter()
        .filter(|o| o.group == OptionGroup::Additional)
        .count();
    assert_eq!((ai, extra), (15, 12), "the children of the two switches");
    // Names are localized text, not keys; the AI white peace option is on by default.
    let nb = c
        .options
        .iter()
        .find(|o| o.key == "GAMEOPTION_NO_BARBARIANS")
        .unwrap();
    assert!(!nb.name.starts_with("TXT_KEY"), "{}", nb.name);
    assert!(!nb.help.starts_with("TXT_KEY"), "{}", nb.help);
    assert_eq!(nb.default, 0);
    assert!(nb.values.is_empty(), "a check box");
    let wp = c
        .options
        .iter()
        .find(|o| o.key == "GAMEOPTION_AI_GIMP_ALWAYS_WHITE_PEACE")
        .unwrap();
    assert_eq!(wp.default, 1);

    // The hover tip of the staging room (SIMP-exe-961): leader, traits, unique units and buildings of Rome, localized.
    let rome = c
        .civs
        .iter()
        .find(|c| c.id == "CIVILIZATION_ROME")
        .expect("Rome");
    assert!(rome.leader.contains("Augustus"), "{:?}", rome.leader);
    assert!(!rome.traits.is_empty(), "no traits");
    assert!(
        rome.traits.iter().all(|t| !t.contains("TXT_KEY")),
        "{:?}",
        rome.traits
    );
    assert!(
        // Rome has two unique units (Ballista, Legion) and no unique building in the game data.
        rome.unique.iter().any(|u| u == "Unit: Legion"),
        "{:?}",
        rome.unique
    );

    // Victories without the scrap one.
    let v: Vec<_> = c.victories.iter().map(|v| v.type_name.as_str()).collect();
    assert_eq!(
        v,
        [
            "VICTORY_TIME",
            "VICTORY_SPACE_RACE",
            "VICTORY_DOMINATION",
            "VICTORY_CULTURAL",
            "VICTORY_DIPLOMATIC"
        ]
    );
    assert!(c.victories.iter().all(|v| !v.name.starts_with("TXT_KEY")));

    // City-states: the whole list of the game data, the sizes take as many as the core does.
    assert!(c.max_minor_civs >= 41, "{} city-states", c.max_minor_civs);
    for w in &c.world_sizes {
        assert_eq!(w.max_minor, 41, "{}", w.id);
        assert!(w.default_minor <= w.max_minor);
    }

    // The options of LekmapPangaea: ids "1".., values by 1-based index, localized names, sorted by priority.
    let script = c
        .map_options
        .iter()
        .find(|m| m.script == PANGAEA)
        .expect("LekmapPangaea has map options");
    assert!(script.options.len() >= 13, "{}", script.options.len());
    let first = &script.options[0];
    assert_eq!(first.id, "1");
    assert_eq!(first.name, "World Age");
    assert_eq!(first.default, 2);
    assert_eq!(first.values.len(), 5);
    assert_eq!(first.values[0].0, 1);
    assert!(first.values.iter().all(|(_, n)| !n.starts_with("TXT_KEY")));
    for o in &script.options {
        assert!(
            o.values.iter().any(|(v, _)| *v == o.default),
            "{}: default {} not among the values",
            o.name,
            o.default
        );
    }
    assert!(c.map_options.len() > 10, "options for many scripts");

    // A game starts with a victory off and a turn limit: the setup is accepted and the core keeps it (the
    // setup text is a section of the save; the PreGame read back is the fork's `setup` selftest).
    let mut s = setup();
    s.settings.game_options = vec![
        ("VICTORY_TIME".into(), 0),
        ("MAX_TURNS".into(), 300),
        ("GAMEOPTION_NO_BARBARIANS".into(), 1),
    ];
    s.settings.map_options = vec![("1".into(), 3)];
    core.start(CoreStart::New {
        setup: &s,
        seed: 77,
    })
    .expect("a game with victories and max turns starts");
    for _ in 0..2000 {
        match core.pump(64).expect("pump").phase {
            CorePhase::Humans => break,
            CorePhase::Ai { .. } => {}
            other => panic!("unexpected phase {other:?}"),
        }
    }
    let save = core.save().expect("save");
    let text = String::from_utf8_lossy(&save);
    assert!(
        text.contains("\"victories\":{\"VICTORY_TIME\":0}") && text.contains("\"max_turns\":300"),
        "the save does not carry the setup keys"
    );
    assert!(text.contains("\"game_options\":{\"GAMEOPTION_NO_BARBARIANS\":1}"));
}
