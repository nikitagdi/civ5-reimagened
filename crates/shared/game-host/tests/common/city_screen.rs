use super::common;
use protocol::{Query, QueryAnswer, ToWorker, WorkerMsg};
use serde_json::Value;
pub fn query(h: &mut common::H, screen: &str, city: i32) -> Value {
    query_args(h, screen, vec![("a".into(), city.into())])
}
pub fn query_args(h: &mut common::H, screen: &str, args: Vec<(String, i64)>) -> Value {
    let before = h.frames.len();
    h.send(ToWorker::Query {
        player: 0,
        qid: 800,
        q: Query::Screen {
            screen: screen.into(),
            args,
        },
    });
    h.run();
    let answer = h.frames[before..]
        .iter()
        .flat_map(|f| &f.msgs)
        .find_map(|m| match m {
            WorkerMsg::QueryResult { qid: 800, answer } => Some(answer),
            _ => None,
        })
        .expect("screen answer");
    let QueryAnswer::Screen { json, .. } = answer else {
        panic!("screen failed: {answer:?}")
    };
    let value: Value = serde_json::from_str(json).unwrap();
    ui2_model::Sample::parse(
        ui2_model::Schema::v1(),
        &serde_json::json!({"model":{screen:value}}).to_string(),
    )
    .expect("real root matches schema");
    value
}
