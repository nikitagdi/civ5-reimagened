//! Seat-scoped economy data through the real worker and public schema.
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
            screen: "economy".into(),
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
        panic!("economy query failed: {answer:?}")
    };
    assert_eq!(screen, "economy");
    let root: serde_json::Value = serde_json::from_str(json).unwrap();
    Sample::parse(
        Schema::v1(),
        &serde_json::json!({"model":{"economy":root}}).to_string(),
    )
    .expect("schema");
    root
}

#[test]
fn economy_data_and_actions_validate_through_the_real_worker() {
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
    let empty = query(&mut h, 1, 0);
    assert!(empty["cities"].as_array().unwrap().is_empty());
    h.found_city(0);
    let root = query(&mut h, 2, 0);
    let cities = root["cities"].as_array().unwrap();
    assert_eq!(cities.len(), 1);
    assert!(cities[0]["city_key"].as_str().unwrap().starts_with("0:"));
    assert_eq!(root["totals"]["population"], cities[0]["population"]);
    assert!(root["finance"]["treasury"].is_number());
    assert!(root["happiness"]["items"].as_array().unwrap().len() >= 10);
    assert!(query(&mut h, 3, 1)["cities"].as_array().unwrap().is_empty());
    // No server actions: the production cell uses the existing client window intent.
}
