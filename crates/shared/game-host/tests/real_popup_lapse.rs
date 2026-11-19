//! A queued decision that can no longer be answered leaves the seat's queue on the real core: its `lapsed` test (in
//! `_popups.lua`) uses the core's own getters. One test: the real core allows a single game per process.
use game_host::Worker;
use game_host::popups::Popup;
use protocol::{Query, QueryAnswer, Start, ToWorker, WorkerMsg};
#[path = "common/real.rs"]
mod common;

fn hud(h: &mut common::H, qid: u64) -> serde_json::Value {
    h.send(ToWorker::Query {
        player: 0,
        qid,
        q: Query::Screen {
            screen: "hud".into(),
            args: vec![],
        },
    });
    h.run();
    let json = h
        .frames
        .iter()
        .flat_map(|f| &f.msgs)
        .find_map(|m| match m {
            WorkerMsg::QueryResult {
                qid: id,
                answer: QueryAnswer::Screen { json, .. },
            } if *id == qid => Some(json.clone()),
            WorkerMsg::QueryResult {
                qid: id,
                answer: QueryAnswer::Error { reason },
            } if *id == qid => panic!("hud: {reason}"),
            _ => None,
        })
        .expect("hud answer");
    serde_json::from_str(&json).unwrap()
}

fn popup(kind: &str, data1: i32) -> Popup {
    Popup {
        kind: kind.into(),
        data1,
        data2: -1,
        data3: -1,
        ..Popup::default()
    }
}

#[test]
fn lapsed_decisions_leave_the_queue_and_live_ones_stay() {
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
    // The city is founded with its production set (`found_city`).
    h.found_city(0);
    let city =
        h.w.fresh_view(0)
            .unwrap()
            .cities
            .iter()
            .find(|c| c.owner == 0)
            .expect("the city")
            .id;
    // A captured city that is not ours, a production question for a city that already builds: both lapsed. A capture
    // question for our own city is still open.
    let gone =
        h.w.inject_popup(0, popup("BUTTONPOPUP_CITY_CAPTURED", city + 1000))
            .unwrap()
            .unwrap();
    let chosen =
        h.w.inject_popup(0, popup("BUTTONPOPUP_CHOOSEPRODUCTION", city))
            .unwrap()
            .unwrap();
    let live =
        h.w.inject_popup(0, popup("BUTTONPOPUP_CITY_CAPTURED", city))
            .unwrap()
            .unwrap();
    let root = hud(&mut h, 900);
    assert_eq!(
        root["popup"]["id"], live,
        "{gone} and {chosen} lapsed: {root}"
    );
    assert_eq!(root["popup_count"], 1);
    assert_eq!(root["popup"]["screen"], "city_captured");
    assert_eq!(
        hud(&mut h, 901)["popup"]["id"],
        live,
        "a live decision stays"
    );
}
