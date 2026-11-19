use game_host::Worker;
use protocol::{Command, CommandOutcome, Start};
#[path = "common/real.rs"]
mod common;
#[allow(dead_code)]
#[path = "common/city_screen.rs"]
mod screen_helper;
#[test]
fn leader_greets_with_the_core_and_unoffered_dialogue_is_an_explained_refusal() {
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
    assert_eq!(root["available"], true);
    assert!(
        !root["phrase"].as_str().unwrap().is_empty(),
        "the AI's own greeting: {root}"
    );
    // Demands, peace talks and answers to AI requests are not offered yet (SIMP-457, SIMP-003); a topic, a war target
    // or a confirmation need the discussion first.
    for action in [
        "demand",
        "make_peace",
        "topic",
        "respond",
        "coop_war",
        "confirm",
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
            other => panic!("{action} must refuse here: {other:?}"),
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
