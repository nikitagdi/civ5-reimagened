use game_host::Worker;
use protocol::{Command, CommandOutcome, Start};
#[path = "common/real.rs"]
mod common;
#[allow(dead_code)]
#[path = "common/city_screen.rs"]
mod screen_helper;

#[test]
fn explicit_war_changes_relations_or_returns_a_reason() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let (worker, outs) = Worker::new(
        common::real_core(),
        common::init(Start::New(common::setup()), ""),
        0,
    );
    let mut h = common::H::with(worker, outs);
    h.found_city(0);
    let empty = screen_helper::query_args(&mut h, "declare_war", vec![]);
    assert_eq!(empty["enabled"], false);
    let lua = lua_sys::Lua::new().unwrap();
    h.w.core_mut().gamecore_mut().attach_lua(&lua).unwrap();
    lua.exec(
        "Teams[Players[0]:GetTeam()]:Meet(Players[1]:GetTeam(), false)",
        "=meet-rival",
    )
    .unwrap();
    lua.exec(
        "function rival_team() return Players[1]:GetTeam() end",
        "=rival-team",
    )
    .unwrap();
    let team = lua.call_global("rival_team", &[]).unwrap()[0]
        .as_i32()
        .unwrap();
    let root = screen_helper::query_args(&mut h, "declare_war", vec![("a".into(), team.into())]);
    assert_eq!(root["target"]["team"], team);
    match h.cmd(
        0,
        Command::ScreenAction {
            screen: "declare_war".into(),
            action: "confirm".into(),
            args: vec![("a".into(), team.into()), ("b".into(), -1)],
            text: None,
        },
    ) {
        CommandOutcome::Applied | CommandOutcome::ScreenApplied { .. } => {
            let root = screen_helper::query_args(&mut h, "diplo", vec![]);
            assert!(
                root["majors"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|r| r["id"] == 1 && r["status"] == "war")
            );
        }
        CommandOutcome::Rejected { reason, .. } => assert!(!reason.is_empty()),
        other => panic!("confirmation needs a final result: {other:?}"),
    }
    for action in ["plunder", "cancel"] {
        let outcome = h.cmd(
            0,
            Command::ScreenAction {
                screen: "declare_war".into(),
                action: action.into(),
                args: vec![],
                text: None,
            },
        );
        if action == "plunder" {
            assert!(matches!(outcome,CommandOutcome::Rejected {reason,..} if !reason.is_empty()));
        } else {
            assert!(matches!(
                outcome,
                CommandOutcome::ScreenApplied { close: true, .. }
            ));
        }
    }
}
