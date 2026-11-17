//! Culture overview through the real worker channel. Stub builds compile and skip.
use game_host::Worker;
use protocol::{Command, CommandOutcome, Query, QueryAnswer, Start, ToWorker, WorkerMsg};
use ui2_model::{Sample, Schema};

#[path = "common/real.rs"]
mod common;

fn query(h: &mut common::H, qid: u64) -> serde_json::Value {
    let before = h.frames.len();
    h.send(ToWorker::Query {
        player: 0,
        qid,
        q: Query::Screen {
            screen: "culture".into(),
            args: vec![],
        },
    });
    h.run();
    let answer = h.frames[before..]
        .iter()
        .flat_map(|frame| &frame.msgs)
        .find_map(|msg| match msg {
            WorkerMsg::QueryResult { qid: id, answer } if *id == qid => Some(answer),
            _ => None,
        })
        .expect("culture answer");
    let QueryAnswer::Screen { screen, json } = answer else {
        panic!("culture query failed")
    };
    assert_eq!(screen, "culture");
    let root: serde_json::Value = serde_json::from_str(json).unwrap();
    Sample::parse(
        Schema::v1(),
        &serde_json::json!({"model":{"culture":root}}).to_string(),
    )
    .expect("schema-valid culture");
    root
}

#[test]
fn culture_new_game_city_and_action_results() {
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
    assert!(query(&mut h, 1)["cities"].as_array().unwrap().is_empty());
    h.found_city(0);
    let root = query(&mut h, 2);
    assert_eq!(root["cities"].as_array().unwrap().len(), 1);
    assert!(root["summary"]["slots"].as_i64().unwrap() > 0);
    assert!(!root["columns"].as_array().unwrap().is_empty());
    let action = |name: &str, args| Command::ScreenAction {
        screen: "culture".into(),
        action: name.into(),
        args,
        text: None,
    };
    assert_eq!(
        h.cmd(0, action("offer", vec![("a".into(), 1), ("b".into(), -1)])),
        CommandOutcome::Applied
    );
    let refreshed = query(&mut h, 3);
    assert!(
        refreshed["swap"]["offers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["class_id"] == 1)
            .unwrap()["work"]
            .is_null()
    );
    for (name, args) in [
        ("move", vec![("a".into(), -1), ("b".into(), -1)]),
        (
            "offer",
            vec![("a".into(), 1), ("b".into(), i64::from(i32::MAX))],
        ),
        (
            "swap",
            vec![("a".into(), 1), ("b".into(), -1), ("c".into(), -1)],
        ),
    ] {
        let CommandOutcome::Rejected { reason, .. } = h.cmd(0, action(name, args)) else {
            panic!("invalid culture action accepted")
        };
        assert!(!reason.is_empty());
    }
}
