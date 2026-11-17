use game_host::Worker;
use protocol::{Command, CommandOutcome, Query, QueryAnswer, Start, ToWorker, WorkerMsg};
use ui2_model::{Sample, Schema};

#[path = "common/real.rs"]
mod common;

#[test]
fn congress_new_game_absence_and_action_reasons() {
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
        qid: 1,
        q: Query::Screen {
            screen: "congress".into(),
            args: vec![],
        },
    });
    h.run();
    let answer = h
        .frames
        .iter()
        .flat_map(|f| &f.msgs)
        .find_map(|msg| match msg {
            WorkerMsg::QueryResult { qid: 1, answer } => Some(answer),
            _ => None,
        })
        .expect("Congress answer");
    let QueryAnswer::Screen { json, .. } = answer else {
        panic!("Congress query failed")
    };
    let root: serde_json::Value = serde_json::from_str(json).unwrap();
    Sample::parse(
        Schema::v1(),
        &serde_json::json!({"model":{"congress":root}}).to_string(),
    )
    .expect("Congress schema");
    assert_eq!(root["league"]["exists"], false);
    assert_eq!(
        h.cmd(
            0,
            Command::ScreenAction {
                screen: "congress".into(),
                action: "open".into(),
                args: vec![],
                text: None
            }
        ),
        CommandOutcome::Applied
    );
    for action in [
        "vote",
        "vote_reset",
        "vote_submit",
        "propose_set",
        "propose_reset",
        "propose_submit",
        "rename",
    ] {
        let result = h.cmd(
            0,
            Command::ScreenAction {
                screen: "congress".into(),
                action: action.into(),
                args: vec![],
                text: Some("Congress".into()),
            },
        );
        let CommandOutcome::Rejected { reason, .. } = result else {
            panic!("Congress action accepted before founding")
        };
        assert!(!reason.is_empty());
    }
}
