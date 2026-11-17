use game_host::Worker;
use protocol::{Query, QueryAnswer, Start, ToWorker, WorkerMsg};
use ui2_model::{Sample, Schema};

#[path = "common/real.rs"]
mod common;

#[test]
fn great_work_new_game_preview_is_schema_valid() {
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
            screen: "great_work".into(),
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
        .expect("viewer answer");
    let QueryAnswer::Screen { json, .. } = answer else {
        panic!("preview failed")
    };
    let root: serde_json::Value = serde_json::from_str(json).unwrap();
    Sample::parse(
        Schema::v1(),
        &serde_json::json!({"model":{"great_work":root}}).to_string(),
    )
    .expect("viewer schema");
    assert_eq!(root["id"], -1);
    assert!(root["artist"].is_null() && root["location"].is_null());
    assert!(!root["name"].as_str().unwrap().is_empty());
}
