//! Popup roots on a real game; event answer paths are covered by the Lua event fixtures.
use game_host::Worker;
use protocol::{Command, CommandOutcome, Query, QueryAnswer, Start, ToWorker, WorkerMsg};
#[path = "common/real.rs"]
mod common;

fn probe(screen: &str, action: Option<&str>) {
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
    h.send(ToWorker::Query {
        player: 0,
        qid: 99,
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
                qid: 99,
                answer: QueryAnswer::Screen { json, .. },
            } => Some(json),
            WorkerMsg::QueryResult {
                qid: 99,
                answer: QueryAnswer::Error { reason },
            } => panic!("{screen}: {reason}"),
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
fn confirm() {
    probe("confirm", Some("choose"));
}

#[test]
fn message() {
    probe("message", Some("ok"));
}

#[test]
fn goody_choice() {
    probe("goody_choice", Some("take"));
}

#[test]
fn notification_log() {
    probe("notification_log", None);
}

#[test]
fn game_menu() {
    probe("game_menu", Some("save"));
    for action in ["exit", "set_quick", "set_email", "set_autosave"] {
        probe("game_menu", Some(action));
    }
}

#[test]
fn end_game() {
    probe("end_game", Some("continue"));
    probe("end_game", Some("exit"));
}
#[test]
fn advisor() {
    probe("advisor", None);
}
