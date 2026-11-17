//! Real city list is scoped to the querying seat.
use game_host::Worker;
use protocol::Start;
#[path = "common/real.rs"]
mod common;
#[path = "common/city_screen.rs"]
mod screen_helper;
#[test]
fn city_list_tracks_founded_cities() {
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
    assert_eq!(screen_helper::query(&mut h, "city_list", 0)["count"], 0);
    h.found_city(0);
    let root = screen_helper::query(&mut h, "city_list", 0);
    assert_eq!(root["count"], 1);
    assert_eq!(root["cities"].as_array().unwrap().len(), 1);
    assert!(!root["cities"][0]["name"].as_str().unwrap().is_empty());
}
