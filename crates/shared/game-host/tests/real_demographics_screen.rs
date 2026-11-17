//! Seat-scoped demographics data through the real worker and public schema.
use game_host::Worker;
use protocol::{Query, QueryAnswer, Start, ToWorker, WorkerMsg};
use ui2_model::{Sample, Schema};

#[path = "common/real.rs"]
mod common;

fn query(h: &mut common::H, qid: u64, player: i8) -> serde_json::Value {
    let before = h.frames.len();
    h.send(ToWorker::Query {
        player,
        qid,
        q: Query::Screen {
            screen: "demographics".into(),
            args: vec![],
        },
    });
    h.run();
    let answer = h.frames[before..]
        .iter()
        .flat_map(|f| &f.msgs)
        .find_map(|m| match m {
            WorkerMsg::QueryResult { qid: id, answer } if *id == qid => Some(answer),
            _ => None,
        })
        .expect("screen answer");
    let QueryAnswer::Screen { screen, json } = answer else {
        panic!("demographics query failed: {answer:?}")
    };
    assert_eq!(screen, "demographics");
    let root: serde_json::Value = serde_json::from_str(json).unwrap();
    Sample::parse(
        Schema::v1(),
        &serde_json::json!({"model":{"demographics":root}}).to_string(),
    )
    .expect("schema");
    root
}

#[test]
fn demographics_data_and_actions_validate_through_the_real_worker() {
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
    let root = query(&mut h, 1, 0);
    assert_eq!(root["rows"].as_array().unwrap().len(), 8);
    assert!(root["participants"].as_i64().unwrap() >= 2);
    h.found_city(0);
    let after = query(&mut h, 2, 0);
    assert!(
        after["rows"][0]["value"].as_i64().unwrap() > root["rows"][0]["value"].as_i64().unwrap()
    );
    let outcome = h.cmd(
        0,
        protocol::Command::ScreenAction {
            screen: "demographics".into(),
            action: "mark_open".into(),
            args: vec![],
            text: None,
        },
    );
    match outcome {
        protocol::CommandOutcome::Applied => {}
        protocol::CommandOutcome::Rejected { reason, .. } => assert!(!reason.trim().is_empty()),
        other => panic!("unexpected mark outcome: {other:?}"),
    }
}
