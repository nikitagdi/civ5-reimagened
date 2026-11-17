//! Seat-scoped victory data through the real worker and public schema.
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
            screen: "victory".into(),
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
        panic!("victory query failed: {answer:?}")
    };
    assert_eq!(screen, "victory");
    let root: serde_json::Value = serde_json::from_str(json).unwrap();
    Sample::parse(
        Schema::v1(),
        &serde_json::json!({"model":{"victory":root}}).to_string(),
    )
    .expect("schema");
    root
}

#[test]
fn victory_data_and_actions_validate_through_the_real_worker() {
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
    assert!(root["ranking"].as_array().unwrap().len() >= 2);
    assert_eq!(root["science"]["parts"].as_array().unwrap().len(), 6);
    h.found_city(0);
    let after = query(&mut h, 2, 0);
    assert!(after["score"]["total"].as_i64().unwrap() >= root["score"]["total"].as_i64().unwrap());
    for kind in [0, 2] {
        let outcome = h.cmd(
            0,
            protocol::Command::ScreenAction {
                screen: "victory".into(),
                action: "propose".into(),
                args: vec![("a".into(), kind)],
                text: None,
            },
        );
        match outcome {
            protocol::CommandOutcome::Rejected { reason, .. } => assert!(!reason.trim().is_empty()),
            protocol::CommandOutcome::Applied => {
                let fresh = query(&mut h, 10 + kind as u64, 0);
                assert!(
                    fresh["proposals"]["items"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|r| r["enabled"] == false)
                );
            }
            other => panic!("unexpected proposal outcome: {other:?}"),
        }
    }
}
