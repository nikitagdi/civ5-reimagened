//! Research-panel inputs are supplied by the live HUD and world snapshot.
use game_host::Worker;
use protocol::{Command, CommandOutcome, Start};
#[path = "common/real.rs"]
mod common;
#[path = "common/tech_query.rs"]
mod tech_query;
#[test]
fn real_panel_inputs_track_research_selection() {
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
    h.found_city(0); // research is closed until the first city exists
    let hud = tech_query::query(&mut h, 1, "hud", vec![]);
    let id = hud["research"]["techs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["status"] == 1)
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    assert_eq!(
        h.cmd(
            0,
            Command::ScreenAction {
                screen: "techs".into(),
                action: "research".into(),
                args: vec![("a".into(), id)],
                text: None
            }
        ),
        CommandOutcome::Applied
    );
    let hud = tech_query::query(&mut h, 2, "hud", vec![]);
    assert!(
        hud["research"]["techs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == id && t["queue"] == 0)
    );
}
