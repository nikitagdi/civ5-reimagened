//! Real game proof; stub builds deliberately skip.
use game_host::Worker;
use protocol::{Command, CommandOutcome, Query, QueryAnswer, Start, ToWorker, WorkerMsg};
use ui2_model::{ModelStore, Sample, Schema};
#[path = "common/real.rs"]
mod common;

fn query(h: &mut common::H, qid: u64) -> serde_json::Value {
    let before = h.frames.len();
    h.send(ToWorker::Query {
        player: 0,
        qid,
        q: Query::Screen {
            screen: "techs".into(),
            args: vec![],
        },
    });
    h.run();
    let json = h.frames[before..]
        .iter()
        .flat_map(|f| &f.msgs)
        .find_map(|m| match m {
            WorkerMsg::QueryResult {
                qid: id,
                answer: QueryAnswer::Screen { json, .. },
            } if *id == qid => Some(json),
            _ => None,
        })
        .expect("technology response");
    let value: serde_json::Value = serde_json::from_str(json).unwrap();
    Sample::parse(
        Schema::v1(),
        &serde_json::json!({"model":{"techs":value}}).to_string(),
    )
    .unwrap()
    .apply(&mut ModelStore::new(Schema::v1()))
    .unwrap();
    value
}
#[test]
fn real_techs_catalog_and_research_actions() {
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
    h.found_city(0); // research is closed until the first city exists
    let root = query(&mut h, 1);
    assert!(root["techs"].as_array().unwrap().len() > 50);
    let id = root["techs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["status"] == "available")
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    let next_id = root["techs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["status"] == "available" && t["id"] != id)
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    let action = |name: &str, args| Command::ScreenAction {
        screen: "techs".into(),
        action: name.into(),
        args,
        text: None,
    };
    assert_eq!(
        h.cmd(
            0,
            action("research", vec![("a".into(), id), ("b".into(), 0)])
        ),
        CommandOutcome::Applied
    );
    let changed = query(&mut h, 2);
    assert!(
        changed["techs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == id && t["status"] == "current")
    );
    assert_eq!(
        h.cmd(
            0,
            action("research", vec![("a".into(), next_id), ("b".into(), 1)])
        ),
        CommandOutcome::Applied
    );
    let queued = query(&mut h, 3);
    assert!(
        queued["techs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == next_id && t["queue_position"].as_i64().unwrap() > 1)
    );
    match h.cmd(0, action("steal", vec![("a".into(), id), ("b".into(), 0)])) {
        CommandOutcome::Rejected { reason, .. } => assert!(!reason.is_empty()),
        other => panic!("stealing from self must fail: {other:?}"),
    }
}
