use game_host::Worker;
use protocol::{Command, CommandOutcome, Query, QueryAnswer, Start, ToWorker, WorkerMsg};
use ui2_model::{Sample, Schema};

#[path = "common/real.rs"]
mod common;

#[test]
fn city_state_new_game_root_and_refused_orders() {
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
    h.send(ToWorker::Query {
        player: 0,
        qid: 1,
        q: Query::Screen {
            screen: "city_state".into(),
            args: vec![],
        },
    });
    h.run();
    let answer = h
        .frames
        .iter()
        .flat_map(|f| &f.msgs)
        .find_map(|msg| match msg {
            WorkerMsg::QueryResult { qid: 1, answer } => Some(answer),
            _ => None,
        })
        .expect("city-state answer");
    let QueryAnswer::Screen { json, .. } = answer else {
        panic!("city-state query failed")
    };
    let root: serde_json::Value = serde_json::from_str(json).unwrap();
    Sample::parse(
        Schema::v1(),
        &serde_json::json!({"model":{"city_state":root}}).to_string(),
    )
    .expect("city-state schema");
    assert_eq!(root["player_id"], 0);
    for action in [
        "peace",
        "gift_gold",
        "bully_gold",
        "bully_unit",
        "pledge",
        "unit_spawning",
        "buyout",
        "enter_land",
        "liberate",
        "puppet",
    ] {
        let result = h.cmd(
            0,
            Command::ScreenAction {
                screen: "city_state".into(),
                action: action.into(),
                args: vec![("a".into(), -1), ("b".into(), 0)],
                text: None,
            },
        );
        let CommandOutcome::Rejected { reason, .. } = result else {
            panic!("invalid city-state action accepted")
        };
        assert!(!reason.is_empty());
    }
}
