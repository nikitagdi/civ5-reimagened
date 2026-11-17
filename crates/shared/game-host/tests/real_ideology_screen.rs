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
            screen: "ideology".into(),
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
        panic!("ideology data failed: {answer:?}")
    };
    let root: serde_json::Value = serde_json::from_str(json).unwrap();
    Sample::parse(
        Schema::v1(),
        &serde_json::json!({"model":{"ideology":root}}).to_string(),
    )
    .expect("valid root");
    root
}

#[test]
fn new_game_ideology_root_and_choice() {
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
    assert_eq!(root["options"].as_array().unwrap().len(), 3);
    assert_eq!(root["enabled"], false);
    let id = root["options"][0]["id"].as_i64().unwrap();
    let outcome = h.cmd(
        0,
        Command::ScreenAction {
            screen: "ideology".into(),
            action: "choose".into(),
            args: vec![("a".into(), id)],
            text: None,
        },
    );
    assert!(matches!(outcome, CommandOutcome::Rejected { ref reason, .. } if !reason.is_empty()));
    assert_eq!(root, query(&mut h, 2));
}
