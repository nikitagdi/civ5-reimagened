use game_host::Worker;
use protocol::{Command, CommandOutcome, Query, QueryAnswer, Start, ToWorker, WorkerMsg};
use ui2_model::{Sample, Schema};

#[path = "common/real.rs"]
mod common;

#[test]
fn archaeology_new_game_is_idle_and_refuses_unavailable_choices() {
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
            screen: "archaeology".into(),
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
        .expect("archaeology answer");
    let QueryAnswer::Screen { json, .. } = answer else {
        panic!("excavation query failed")
    };
    let root: serde_json::Value = serde_json::from_str(json).unwrap();
    Sample::parse(
        Schema::v1(),
        &serde_json::json!({"model":{"archaeology":root}}).to_string(),
    )
    .expect("excavation schema");
    assert_eq!(root["status"], "idle");
    assert!(root["choices"].as_array().unwrap().is_empty());
    for choice in 1..=5 {
        let result = h.cmd(
            0,
            Command::ScreenAction {
                screen: "archaeology".into(),
                action: "choose".into(),
                args: vec![("a".into(), choice), ("b".into(), -1)],
                text: None,
            },
        );
        let CommandOutcome::Rejected { reason, .. } = result else {
            panic!("unavailable excavation accepted")
        };
        assert!(!reason.is_empty());
    }
}
