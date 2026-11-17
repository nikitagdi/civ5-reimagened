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
    let root = screen_helper::query(&mut h, "city_rename", city);
    assert_eq!(root["mode"], "rename");
    let result = h.cmd(
        0,
        Command::ScreenAction {
            screen: "city_rename".into(),
            action: "rename".into(),
            args: vec![("a".into(), city.into())],
            text: Some("New city name".into()),
        },
    );
    assert!(matches!(result,CommandOutcome::Rejected{reason,..} if reason.contains("text")));
    let question = screen_helper::query_args(
        &mut h,
        "city_rename",
        vec![("a".into(), city.into()), ("b".into(), 1)],
    );
    let result = h.cmd(
        0,
        Command::ScreenAction {
            screen: "city_rename".into(),
            action: "do_task".into(),
            args: vec![
                ("a".into(), city.into()),
                ("b".into(), question["task"]["id"].as_i64().unwrap()),
            ],
            text: None,
        },
    );
    assert!(matches!(result,CommandOutcome::Rejected{reason,..} if !reason.is_empty()));
    assert_eq!(
        screen_helper::query(&mut h, "city_rename", city)["name"],
        root["name"]
    );
}
