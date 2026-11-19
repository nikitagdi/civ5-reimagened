//! Live diplomacy roots, Talk and its refusals. Stub builds skip deliberately.
use game_host::Worker;
use protocol::{Command, CommandOutcome, Start};
#[path = "common/real.rs"]
mod common;
#[allow(dead_code)]
#[path = "common/city_screen.rs"]
mod screen_helper;

#[test]
fn diplomacy_root_is_well_formed_and_talk_opens_only_met_ai_leaders() {
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
    // The core's seat-scoped deal snapshots: history is available (empty in a new game).
    assert_eq!(root["deals_available"], true);
    assert!(root["deals_reason"].is_null());
    assert!(root["deals"].as_array().unwrap().is_empty());
    // Talk to a leader the seat has not met is refused with an explanation; a met AI opens its leader screen.
    let known: Vec<i64> = root["majors"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["is_human"] == false)
        .map(|p| p["id"].as_i64().unwrap())
        .collect();
    let talk = |h: &mut common::H, id: i64| {
        h.cmd(
            0,
            Command::ScreenAction {
                screen: "diplo".into(),
                action: "begin_diplo".into(),
                args: vec![("a".into(), id)],
                text: None,
            },
        )
    };
    if !known.contains(&2) {
        match talk(&mut h, 2) {
            CommandOutcome::Rejected { reason, .. } => assert!(!reason.is_empty()),
            other => panic!("an unmet leader must be refused: {other:?}"),
        }
    }
    for id in known {
        match talk(&mut h, id) {
            CommandOutcome::ScreenApplied { open: Some(o), .. } => assert_eq!(o.screen, "leader"),
            other => panic!("a met AI opens its leader: {other:?}"),
        }
    }
}
