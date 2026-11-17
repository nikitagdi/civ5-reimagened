use game_host::Worker;
use protocol::{Command, CommandOutcome, Start};
#[path = "common/real.rs"]
mod common;
#[allow(dead_code)]
#[path = "common/city_screen.rs"]
mod screen_helper;
#[test]
fn leader_dossier_is_real_and_dialogue_actions_are_explained_refusals() {
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
        screen_helper::query_args(&mut h, "leader", vec![])["available"],
        false
    );
    let lua = lua_sys::Lua::new().unwrap();
    h.w.core_mut().gamecore_mut().attach_lua(&lua).unwrap();
    lua.exec(
        "Teams[Players[0]:GetTeam()]:Meet(Players[2]:GetTeam(), false)",
        "=meet-ai",
    )
    .unwrap();
    let root = screen_helper::query_args(&mut h, "leader", vec![("a".into(), 2)]);
    assert_eq!(root["partner"]["id"], 2);
    assert!(root["phrase"].as_str().unwrap().is_empty());
    assert!(!root["unavailable_reason"].as_str().unwrap().is_empty());
    for action in [
        "discuss",
        "trade",
        "demand",
        "make_peace",
        "topic",
        "respond",
        "coop_war",
        "confirm",
        "back",
        "switch",
    ] {
        match h.cmd(
            0,
            Command::ScreenAction {
                screen: "leader".into(),
                action: action.into(),
                args: vec![("a".into(), 0)],
                text: None,
            },
        ) {
            CommandOutcome::Rejected { reason, .. } => assert!(!reason.is_empty()),
            other => panic!("dialogue without a session must refuse: {other:?}"),
        }
    }
    let war = h.cmd(
        0,
        Command::ScreenAction {
            screen: "leader".into(),
            action: "declare_war".into(),
            args: vec![],
            text: None,
        },
    );
    assert!(
        matches!(war,CommandOutcome::ScreenApplied {open:Some(ref o),..} if o.screen=="declare_war")
            || matches!(war,CommandOutcome::Rejected {ref reason,..} if !reason.is_empty())
    );
    assert!(matches!(
        h.cmd(
            0,
            Command::ScreenAction {
                screen: "leader".into(),
                action: "goodbye".into(),
                args: vec![],
                text: None
            }
        ),
        CommandOutcome::ScreenApplied { close: true, .. }
    ));
}
