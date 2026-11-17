use game_host::Worker;
use protocol::{Command, CommandOutcome, Start};
#[path = "common/real.rs"]
mod common;
#[allow(dead_code)]
#[path = "common/city_screen.rs"]
mod screen_helper;
#[test]
fn ribbon_has_a_self_row_and_explained_action_refusals() {
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
    let root = screen_helper::query_args(&mut h, "civ_ribbon", vec![]);
    assert_eq!(root["majors"][0]["id"], 0);
    assert_eq!(root["majors"][0]["is_me"], true);
    for action in ["begin_diplo", "propose_peace"] {
        match h.cmd(
            0,
            Command::ScreenAction {
                screen: "civ_ribbon".into(),
                action: action.into(),
                args: vec![("a".into(), 1)],
                text: None,
            },
        ) {
            CommandOutcome::Rejected { reason, .. } => assert!(!reason.is_empty()),
            other => panic!("unsupported action must refuse: {other:?}"),
        }
    }
}
