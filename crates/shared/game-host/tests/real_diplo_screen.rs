//! Live diplomacy roots and action refusals. Stub builds skip deliberately.
use game_host::Worker;
use protocol::{Command, CommandOutcome, Start};
#[path = "common/real.rs"]
mod common;
#[allow(dead_code)]
#[path = "common/city_screen.rs"]
mod screen_helper;

#[test]
fn diplomacy_root_is_well_formed_and_conversations_fail_with_a_reason() {
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
    let root = screen_helper::query_args(&mut h, "diplo", vec![]);
    assert_eq!(root["me"]["id"], 0);
    assert!(root["majors"].is_array() && root["city_states"].is_array());
    assert_eq!(root["deals_available"], false);
    assert!(!root["deals_reason"].as_str().unwrap().is_empty());
    // A new map may have no contacts. Prove the action always refuses with an explanation,
    // including every known AI when the seed starts within contact distance.
    let targets = std::iter::once(2).chain(
        root["majors"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["is_human"] == false)
            .map(|p| p["id"].as_i64().unwrap()),
    );
    for id in targets {
        match h.cmd(
            0,
            Command::ScreenAction {
                screen: "diplo".into(),
                action: "begin_diplo".into(),
                args: vec![("a".into(), id)],
                text: None,
            },
        ) {
            CommandOutcome::Rejected { reason, .. } => assert!(!reason.is_empty()),
            other => panic!("unsupported conversation must fail: {other:?}"),
        }
    }
}
