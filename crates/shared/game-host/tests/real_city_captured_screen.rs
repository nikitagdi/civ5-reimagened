//! Real-core decision query and action validation.
use game_host::Worker;
use protocol::{Command, CommandOutcome, Start};
#[path = "common/real.rs"]
mod common;
#[path = "common/city_screen.rs"]
mod screen_helper;
#[test]
fn real_decision_root_and_action_refusals() {
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
    let city =
        h.w.fresh_view(0)
            .unwrap()
            .cities
            .iter()
            .find(|c| c.owner == 0)
            .unwrap()
            .id;
    let root = screen_helper::query(&mut h, "city_captured", city);
    assert_eq!(root["options"].as_array().unwrap().len(), 4);
    for action in ["liberate", "annex", "puppet", "raze"] {
        let result = h.cmd(
            0,
            Command::ScreenAction {
                screen: "city_captured".into(),
                action: action.into(),
                args: vec![("a".into(), city.into())],
                text: None,
            },
        );
        assert!(
            matches!(result,CommandOutcome::Rejected{reason,..} if !reason.is_empty()),
            "{action}"
        );
    }
    let annex = screen_helper::query_args(
        &mut h,
        "city_captured",
        vec![("a".into(), city.into()), ("b".into(), 1)],
    );
    assert_eq!(annex["mode"], "annex");
}
