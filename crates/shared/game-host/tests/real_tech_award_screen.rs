//! Award data is schema-checked on the real core; it has no game actions.
use game_host::Worker;
use protocol::Start;
#[path = "common/real.rs"]
mod common;
#[path = "common/tech_query.rs"]
mod tech_query;
#[test]
fn real_award_reads_technology_by_argument() {
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
    let root = tech_query::query(&mut h, 1, "tech_award", vec![("a".into(), 0)]);
    assert!(root["tech"]["name"].is_string());
    assert!(root["unlocks"].as_array().unwrap().len() <= 5);
    let missing = tech_query::query(
        &mut h,
        2,
        "tech_award",
        vec![("a".into(), i64::from(i32::MAX))],
    );
    assert!(missing.get("tech").is_none());
}
