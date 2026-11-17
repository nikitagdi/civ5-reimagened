use game_host::Worker;
use protocol::{Command, CommandOutcome, Query, QueryAnswer, Start, ToWorker, WorkerMsg};
use ui2_model::{Sample, Schema};

#[path = "common/real.rs"]
mod common;

fn query(h: &mut common::H, id: u64) -> serde_json::Value {
    query_at(h, id, vec![])
}

fn query_at(h: &mut common::H, id: u64, args: Vec<(String, i64)>) -> serde_json::Value {
    let before = h.frames.len();
    h.send(ToWorker::Query {
        player: 0,
        qid: id,
        q: Query::Screen {
            screen: "religion".into(),
            args,
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
        panic!("religion data failed: {answer:?}")
    };
    let root: serde_json::Value = serde_json::from_str(json).unwrap();
    Sample::parse(
        Schema::v1(),
        &serde_json::json!({"model":{"religion":root}}).to_string(),
    )
    .expect("valid root");
    root
}

#[test]
fn new_game_religion_root_refuses_choices_without_a_prophet() {
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
    assert!(!root["symbols"].as_array().unwrap().is_empty());
    assert_eq!(root["slots"].as_array().unwrap().len(), 6);
    assert_eq!(root["enabled"], false);
    h.found_city(0);
    let city =
        h.w.fresh_view(0)
            .unwrap()
            .cities
            .iter()
            .find(|c| c.owner == 0)
            .unwrap()
            .id;
    let args = vec![("a".into(), i64::from(city))];
    let before = query_at(&mut h, 2, args.clone());
    let symbol = before["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["taken"] == false)
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    let pick = |key: &str| {
        let slot = before["slots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["key"] == key)
            .unwrap();
        if slot["state"] == "choose" {
            slot["options"][0]["id"].as_i64().unwrap()
        } else {
            65535
        }
    };
    let pack = |low: i64, high: i64| i64::from((low as u32 | ((high as u32) << 16)) as i32);
    for action in ["found", "enhance"] {
        let outcome = h.cmd(
            0,
            Command::ScreenAction {
                screen: "religion".into(),
                action: action.into(),
                args: vec![
                    ("a".into(), symbol),
                    ("b".into(), i64::from(city)),
                    ("c".into(), pack(pick("founder"), pick("follower"))),
                    ("d".into(), pack(pick("pantheon"), pick("bonus"))),
                ],
                text: Some(String::new()),
            },
        );
        assert!(
            matches!(outcome, CommandOutcome::Rejected { ref reason, .. } if !reason.is_empty())
        );
    }
    assert_eq!(before, query_at(&mut h, 3, args));
}
