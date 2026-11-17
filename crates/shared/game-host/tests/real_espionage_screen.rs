//! A new Renaissance game reaches the agent lifecycle before testing relocation.
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
            screen: "espionage".into(),
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
        panic!("espionage query failed: {answer:?}")
    };
    assert_eq!(screen, "espionage");
    let root: serde_json::Value = serde_json::from_str(json).unwrap();
    Sample::parse(
        Schema::v1(),
        &serde_json::json!({"model":{"espionage":root}}).to_string(),
    )
    .expect("schema");
    root
}

#[test]
fn espionage_data_and_actions_validate_through_the_real_worker() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let mut setup = common::setup();
    setup.settings.start_era = "ERA_RENAISSANCE".into();
    let (worker, outs) = Worker::new(common::real_core(), common::init(Start::New(setup), ""), 0);
    let mut h = common::H::with(worker, outs);
    let root = query(&mut h, 1, 0);
    assert!(root["agents"].is_array());
    assert!(root["intrigue"].is_array());
    h.found_city(0);
    let root = query(&mut h, 2, 0);
    assert!(root["enabled"].is_boolean());
    assert!(
        root["own_cities"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["player"] == 0)
    );
    let agent = root["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["dead"] == false)
        .expect("the Renaissance game provides a living agent")["id"]
        .as_i64()
        .unwrap();
    let city = root["own_cities"]
        .as_array()
        .unwrap()
        .first()
        .expect("founding provides an espionage city")["raw_city_id"]
        .as_i64()
        .unwrap();
    for (index, action) in ["move", "hideout", "coup"].into_iter().enumerate() {
        let outcome = h.cmd(
            0,
            protocol::Command::ScreenAction {
                screen: "espionage".into(),
                action: action.into(),
                args: vec![
                    ("a".into(), agent),
                    ("b".into(), 0),
                    ("c".into(), city),
                    ("d".into(), 0),
                ],
                text: None,
            },
        );
        match outcome {
            protocol::CommandOutcome::Applied => {
                let fresh = query(&mut h, 10 + index as u64, 0);
                let row = fresh["agents"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|a| a["id"] == agent)
                    .unwrap();
                match action {
                    "move" => assert!(row["place"].is_object()),
                    "hideout" => assert!(row["place"].is_null()),
                    "coup" => panic!("an agent in an owned city cannot stage a coup"),
                    _ => unreachable!(),
                }
            }
            protocol::CommandOutcome::Rejected { reason, .. } => assert!(!reason.trim().is_empty()),
            other => panic!("unexpected spy outcome: {other:?}"),
        }
    }
    for action in ["move", "hideout", "coup"] {
        let outcome = h.cmd(
            0,
            protocol::Command::ScreenAction {
                screen: "espionage".into(),
                action: action.into(),
                args: vec![
                    ("a".into(), i64::from(i32::MAX)),
                    ("b".into(), 0),
                    ("c".into(), 0),
                    ("d".into(), 0),
                ],
                text: None,
            },
        );
        match outcome {
            protocol::CommandOutcome::Rejected { reason, .. } => assert!(!reason.trim().is_empty()),
            other => panic!("missing agent must be refused: {other:?}"),
        }
    }
}
