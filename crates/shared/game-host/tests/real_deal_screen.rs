use game_host::Worker;
use protocol::{Command, CommandOutcome, Start};
#[path = "common/real.rs"]
mod common;
#[allow(dead_code)]
#[path = "common/city_screen.rs"]
mod screen_helper;
#[test]
fn human_draft_edits_or_refuses_with_a_reason_without_spending_gold() {
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
    assert_eq!(
        screen_helper::query_args(&mut h, "deal", vec![])["editable"],
        false
    );
    let lua = lua_sys::Lua::new().unwrap();
    h.w.core_mut().gamecore_mut().attach_lua(&lua).unwrap();
    lua.exec(
        "Teams[Players[0]:GetTeam()]:Meet(Players[1]:GetTeam(), false)",
        "=meet-trade-partner",
    )
    .unwrap();
    let root = screen_helper::query_args(&mut h, "deal", vec![("a".into(), 1)]);
    assert_eq!(root["partner"]["id"], 1);
    let gold = root["me"]["gold"].clone();
    for action in [
        "add",
        "add_max",
        "set_amount",
        "remove",
        "clear",
        "propose",
        "accept",
        "refuse",
        "withdraw",
        "modify",
        "ask",
        "demand",
        "switch",
        "cancel",
        "close",
    ] {
        let result = h.cmd(
            0,
            Command::ScreenAction {
                screen: "deal".into(),
                action: action.into(),
                args: vec![
                    ("a".into(), 0),
                    ("b".into(), 1),
                    ("c".into(), 0),
                    ("d".into(), 1),
                ],
                text: None,
            },
        );
        match result {
            CommandOutcome::Applied | CommandOutcome::ScreenApplied { .. } => {
                let root = screen_helper::query_args(&mut h, "deal", vec![("a".into(), 1)]);
                assert_eq!(root["me"]["gold"], gold, "drafts must not spend gold");
                if action == "add" || action == "add_max" || action == "set_amount" {
                    assert!(!root["table"]["us"].as_array().unwrap().is_empty());
                }
                if action == "clear" {
                    assert!(root["table"]["us"].as_array().unwrap().is_empty());
                }
            }
            CommandOutcome::Rejected { reason, .. } => assert!(!reason.is_empty()),
            other => panic!("expected final result: {other:?}"),
        }
    }
}
