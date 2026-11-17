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
    let _ = city;
    for mode in 0..3 {
        let root = screen_helper::query(&mut h, "free_great_person", mode);
        assert_eq!(root["remaining"], 0);
        let candidate = root["people"]
            .as_array()
            .unwrap()
            .first()
            .expect("Great Person catalog")["unit"]
            .as_i64()
            .unwrap();
        let result = h.cmd(
            0,
            Command::ScreenAction {
                screen: "free_great_person".into(),
                action: "choose".into(),
                args: vec![("a".into(), candidate), ("b".into(), mode.into())],
                text: None,
            },
        );
        assert!(matches!(result,CommandOutcome::Rejected{reason,..} if !reason.is_empty()));
        assert_eq!(
            screen_helper::query(&mut h, "free_great_person", mode)["remaining"],
            0
        );
    }
}
