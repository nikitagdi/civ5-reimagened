use game_host::Worker;
use protocol::{Command, CommandOutcome, Query, QueryAnswer, Start, ToWorker, WorkerMsg};
use ui2_model::{Sample, Schema};

#[path = "common/real.rs"]
mod common;

fn query(h: &mut common::H, id: u64) -> serde_json::Value {
    query_named(h, id, "policies")
}

fn query_named(h: &mut common::H, id: u64, name: &str) -> serde_json::Value {
    let before = h.frames.len();
    h.send(ToWorker::Query {
        player: 0,
        qid: id,
        q: Query::Screen {
            screen: name.into(),
            args: vec![],
        },
    });
    h.run();
    let answer = h.frames[before..]
        .iter()
        .flat_map(|f| &f.msgs)
        .find_map(|m| match m {
            WorkerMsg::QueryResult { qid, answer } if *qid == id => Some(answer),
            _ => None,
        })
        .expect("screen answer");
    let QueryAnswer::Screen { json, .. } = answer else {
        panic!("policy data failed: {answer:?}")
    };
    let root: serde_json::Value = serde_json::from_str(json).unwrap();
    Sample::parse(
        Schema::v1(),
        &serde_json::json!({"model":{name:root}}).to_string(),
    )
    .expect("valid root");
    root
}

#[test]
fn new_game_policy_root_and_actions() {
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
    let root = query(&mut h, 1);
    assert!(!root["branches"].as_array().unwrap().is_empty());
    assert!(
        root["branches"]
            .as_array()
            .unwrap()
            .iter()
            .all(|b| b["reason"].as_str().is_some_and(|r| !r.is_empty()))
    );
    h.found_city(0);
    let mut before = query(&mut h, 2);
    let branch = before["branches"][0]["id"].as_i64().unwrap();
    let policy = before["branches"][0]["policies"][0]["id"].as_i64().unwrap();
    let tenets = query_named(&mut h, 20, "ideology");
    let tenet = tenets["options"][0]["levels"][0]["tenets"][0]["id"]
        .as_i64()
        .unwrap();
    for (i, (action, id)) in [
        ("unlock_branch", branch),
        ("adopt_policy", policy),
        ("adopt_tenet", tenet),
        ("change_ideology", 0),
    ]
    .into_iter()
    .enumerate()
    {
        let outcome = h.cmd(
            0,
            Command::ScreenAction {
                screen: "policies".into(),
                action: action.into(),
                args: vec![("a".into(), id)],
                text: None,
            },
        );
        let after = query(&mut h, 3 + i as u64);
        match outcome {
            CommandOutcome::Applied => {
                assert_ne!(before, after, "applied action changes policy data")
            }
            CommandOutcome::Rejected { reason, .. } => assert!(!reason.is_empty()),
            other => panic!("unexpected outcome {other:?}"),
        }
        before = after;
    }
}
