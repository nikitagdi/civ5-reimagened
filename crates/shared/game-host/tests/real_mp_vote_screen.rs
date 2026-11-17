use game_host::Worker;
use protocol::{Command, CommandOutcome, Query, QueryAnswer, Start, ToWorker, WorkerMsg};
use ui2_model::{Sample, Schema};

#[path = "common/real.rs"]
mod common;

#[test]
fn network_vote_new_game_root_and_unavailable_action_reasons() {
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
    for (qid, view) in [(1, 0), (2, 1), (3, 2)] {
        h.send(ToWorker::Query {
            player: 0,
            qid,
            q: Query::Screen {
                screen: "mp_vote".into(),
                args: vec![("a".into(), view)],
            },
        });
        h.run();
        let answer = h
            .frames
            .iter()
            .flat_map(|f| &f.msgs)
            .find_map(|msg| match msg {
                WorkerMsg::QueryResult { qid: id, answer } if *id == qid => Some(answer),
                _ => None,
            })
            .expect("network vote answer");
        let QueryAnswer::Screen { json, .. } = answer else {
            panic!("network vote query failed")
        };
        let root: serde_json::Value = serde_json::from_str(json).unwrap();
        Sample::parse(
            Schema::v1(),
            &serde_json::json!({"model":{"mp_vote":root}}).to_string(),
        )
        .expect("network vote schema");
        assert_eq!(root["player_id"], 0);
    }
    for action in ["vote_yes", "vote_no", "concede", "diplo_vote"] {
        let result = h.cmd(
            0,
            Command::ScreenAction {
                screen: "mp_vote".into(),
                action: action.into(),
                args: vec![("a".into(), -1)],
                text: None,
            },
        );
        let CommandOutcome::Rejected { reason, .. } = result else {
            panic!("invalid network vote accepted")
        };
        assert!(!reason.is_empty());
    }
}
