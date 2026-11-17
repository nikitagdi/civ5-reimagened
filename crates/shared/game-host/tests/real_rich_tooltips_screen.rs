//! Real tooltip data is validated against the screen schema.
use game_host::Worker;
use protocol::Start;
#[path = "common/real.rs"]
mod common;
#[path = "common/tech_query.rs"]
mod tech_query;
#[test]
fn real_tooltip_query_by_kind_and_id() {
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
    for kind in 0..=5 {
        let root = tech_query::query(
            &mut h,
            kind as u64 + 1,
            "rich_tooltips",
            vec![("kind".into(), kind), ("id".into(), 0)],
        );
        assert!(root.is_object());
        if kind == 0 {
            assert!(root["tech"]["cost"].as_i64().unwrap() > 0);
        }
    }
    let missing = tech_query::query(
        &mut h,
        10,
        "rich_tooltips",
        vec![("kind".into(), 0), ("id".into(), i64::from(i32::MAX))],
    );
    assert!(missing.get("tech").is_none());
    // This service has no game actions.
}
