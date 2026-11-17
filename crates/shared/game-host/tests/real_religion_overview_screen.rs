use game_host::Worker;
use protocol::{Command, CommandOutcome, Query, QueryAnswer, Start, ToWorker, WorkerMsg};
use ui2_model::{Sample, Schema};

#[path = "common/real.rs"]
mod common;

fn query(h: &mut common::H, id: u64) -> serde_json::Value {
    let before = h.frames.len();
    h.send(ToWorker::Query {
        player: 0,
        qid: id,
        q: Query::Screen {
            screen: "religion_overview".into(),
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
        panic!("religion_overview data failed: {answer:?}")
    };
    let root: serde_json::Value = serde_json::from_str(json).unwrap();
    Sample::parse(
        Schema::v1(),
        &serde_json::json!({"model":{"religion_overview":root}}).to_string(),
    )
    .expect("valid root");
    root
}

#[test]
fn new_game_faith_purchase_changes_the_server_label() {
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
    assert!(root["auto_purchase"]["options"].as_array().unwrap().len() >= 2);
    assert_eq!(
        h.cmd(
            0,
            Command::ScreenAction {
                screen: "religion_overview".into(),
                action: "auto_purchase".into(),
                args: vec![("a".into(), 1), ("b".into(), -1)],
                text: None
            }
        ),
        CommandOutcome::Applied
    );
    let after = query(&mut h, 2);
    assert_eq!(after["auto_purchase"]["current"], "save_prophet");
    assert_ne!(root["auto_purchase"], after["auto_purchase"]);
    assert!(matches!(
        h.cmd(
            0,
            Command::ScreenAction {
                screen: "religion_overview".into(),
                action: "auto_purchase".into(),
                args: vec![("a".into(), 2), ("b".into(), -1)],
                text: None
            }
        ),
        CommandOutcome::Rejected { ref reason, .. } if !reason.is_empty()
    ));
}
