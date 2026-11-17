//! Popup roots on a real game; event answer paths are covered by the Lua event fixtures.
//! One test: the real core allows a single game per process, so every screen is probed on the same worker.
use game_host::Worker;
use protocol::{Command, CommandOutcome, Query, QueryAnswer, Start, ToWorker, WorkerMsg};
#[path = "common/real.rs"]
mod common;

fn probe(h: &mut common::H, qid: u64, screen: &str, action: Option<&str>) {
    h.send(ToWorker::Query {
        player: 0,
        qid,
        q: Query::Screen {
            screen: screen.into(),
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
            } if *id == qid => Some(json),
            WorkerMsg::QueryResult {
                qid: id,
                answer: QueryAnswer::Error { reason },
            } if *id == qid => panic!("{screen}: {reason}"),
            _ => None,
        })
        .expect("screen answer");
    let root: serde_json::Value = serde_json::from_str(json).unwrap();
    ui2_model::Sample::parse(
        ui2_model::Schema::v1(),
        &serde_json::json!({"model":{screen:root}}).to_string(),
    )
    .unwrap();
    if let Some(action) = action {
        match h.cmd(
            0,
            Command::ScreenAction {
                screen: screen.into(),
                action: action.into(),
                args: vec![("a".into(), -1), ("b".into(), -1)],
                text: None,
            },
        ) {
            CommandOutcome::Rejected { reason, .. } => assert!(!reason.trim().is_empty()),
            other => panic!("An unavailable action must have a reason: {other:?}"),
        }
    }
}

#[test]
fn popup_screens_answer_and_refuse_without_events() {
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
    h.found_city(0);
    let mut qid = 100;
    let mut next = || {
        qid += 1;
        qid
    };
    probe(&mut h, next(), "confirm", Some("choose"));
    probe(&mut h, next(), "message", Some("ok"));
    probe(&mut h, next(), "goody_choice", Some("take"));
    probe(&mut h, next(), "notification_log", None);
    probe(&mut h, next(), "game_menu", Some("save"));
    for action in ["exit", "set_quick", "set_email", "set_autosave"] {
        probe(&mut h, next(), "game_menu", Some(action));
    }
    probe(&mut h, next(), "end_game", Some("continue"));
    probe(&mut h, next(), "end_game", Some("exit"));
    probe(&mut h, next(), "advisor", None);
}
