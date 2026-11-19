//! Seat-scoped military data through the real worker and public schema.
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
            screen: "military".into(),
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
        panic!("military query failed: {answer:?}")
    };
    assert_eq!(screen, "military");
    let root: serde_json::Value = serde_json::from_str(json).unwrap();
    Sample::parse(
        Schema::v1(),
        &serde_json::json!({"model":{"military":root}}).to_string(),
    )
    .expect("schema");
    root
}

#[test]
fn military_data_and_actions_validate_through_the_real_worker() {
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
    let root = query(&mut h, 1, 0);
    let rows = root["units"].as_array().unwrap();
    assert!(!rows.is_empty());
    assert!(
        rows.iter()
            .all(|u| u["unit_key"].as_str().unwrap().starts_with("0:"))
    );
    assert_eq!(
        root["counts"]["military"].as_u64().unwrap() + root["counts"]["civilian"].as_u64().unwrap(),
        rows.len() as u64
    );
    h.found_city(0);
    let after = query(&mut h, 2, 0);
    assert!(after["units"].as_array().unwrap().len() < rows.len());
    assert!(
        query(&mut h, 3, 1)["units"]
            .as_array()
            .unwrap()
            .iter()
            .all(|u| u["unit_key"].as_str().unwrap().starts_with("1:"))
    );
    // An exploring unit reads as automated exploring (the original's text), not as automated building.
    let lua = lua_sys::Lua::new().unwrap();
    game_host::Core::attach_screen_lua(h.w.core_mut(), &lua).unwrap();
    lua.exec(
        r#"explore = Locale.ConvertTextKey("TXT_KEY_ACTION_AUTOMATE_EXPLORE")
        for u in Players[0]:Units() do if u:IsCombatUnit() then scout = u:GetID() end end"#,
        "=explore",
    )
    .unwrap();
    let explore: String = lua.globals().get("explore").unwrap();
    let scout: i32 = lua.globals().get("scout").unwrap();
    assert_eq!(
        h.cmd(0, protocol::Command::UnitAutomateExplore { unit: scout }),
        protocol::CommandOutcome::Applied
    );
    let root = query(&mut h, 4, 0);
    let row = root["units"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"] == scout)
        .expect("the exploring unit");
    assert_eq!(row["status"], "automated", "{row}");
    assert_eq!(row["status_text"], explore.as_str());
}
