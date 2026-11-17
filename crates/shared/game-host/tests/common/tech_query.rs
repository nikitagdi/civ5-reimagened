use protocol::{Query, QueryAnswer, ToWorker, WorkerMsg};
use ui2_model::{ModelStore, Sample, Schema};
pub fn query(
    h: &mut super::common::H,
    qid: u64,
    screen: &str,
    args: Vec<(String, i64)>,
) -> serde_json::Value {
    let before = h.frames.len();
    h.send(ToWorker::Query {
        player: 0,
        qid,
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
            WorkerMsg::QueryResult { qid: id, answer } if *id == qid => Some(answer),
            _ => None,
        })
        .expect("screen response");
    let QueryAnswer::Screen { json, .. } = answer else {
        panic!("screen query failed: {answer:?}")
    };
    let value: serde_json::Value = serde_json::from_str(json).unwrap();
    let mut model = serde_json::Map::new();
    model.insert(screen.into(), value.clone());
    Sample::parse(
        Schema::v1(),
        &serde_json::json!({"model":model}).to_string(),
    )
    .unwrap()
    .apply(&mut ModelStore::new(Schema::v1()))
    .unwrap();
    value
}
