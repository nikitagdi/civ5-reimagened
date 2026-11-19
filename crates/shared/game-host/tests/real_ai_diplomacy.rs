//! Diplomacy of a human seat with an AI on the real core: Talk opens the leader with its real greeting, the discussion
//! sends the seat's own event and the AI's reply reaches that seat's leader session only, and a trade is checked,
//! valued and enacted by the core in order. Reads and refused or local actions leave the simulation digest as it was.
use game_host::Worker;
use protocol::{Command, CommandOutcome, PlayerId, Query, QueryAnswer, Start, ToWorker, WorkerMsg};
use serde_json::Value;
#[path = "common/real.rs"]
mod common;

const AI: i64 = 2;

fn digest(h: &mut common::H) -> u64 {
    h.w.core_mut().gamecore_mut().state_digest(-2).unwrap()
}

fn query(h: &mut common::H, player: PlayerId, screen: &str, args: Vec<(String, i64)>) -> Value {
    let before = h.frames.len();
    h.send(ToWorker::Query {
        player,
        qid: 900,
        q: Query::Screen {
            screen: screen.into(),
            args,
        },
    });
    h.run();
    let answer = h.frames[before..]
        .iter()
        .flat_map(|f| &f.msgs)
        .find_map(|m| match m {
            WorkerMsg::QueryResult { qid: 900, answer } => Some(answer),
            _ => None,
        })
        .expect("screen answer");
    let QueryAnswer::Screen { json, .. } = answer else {
        panic!("screen failed: {answer:?}")
    };
    let value: Value = serde_json::from_str(json).unwrap();
    ui2_model::Sample::parse(
        ui2_model::Schema::v1(),
        &serde_json::json!({"model":{screen:value}}).to_string(),
    )
    .expect("real root matches schema");
    value
}

fn act(
    h: &mut common::H,
    player: PlayerId,
    screen: &str,
    action: &str,
    args: &[(&str, i64)],
) -> CommandOutcome {
    h.cmd(
        player,
        Command::ScreenAction {
            screen: screen.into(),
            action: action.into(),
            args: args.iter().map(|(k, v)| ((*k).into(), *v)).collect(),
            text: None,
        },
    )
}

fn leader(h: &mut common::H, player: PlayerId) -> Value {
    query(h, player, "leader", vec![("a".into(), AI)])
}

fn deal(h: &mut common::H) -> Value {
    query(h, 0, "deal", vec![("a".into(), AI)])
}

#[test]
fn talk_discuss_and_trade_with_an_ai_are_seat_scoped_and_ordered() {
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
    let lua = lua_sys::Lua::new().unwrap();
    game_host::Core::attach_screen_lua(h.w.core_mut(), &lua).unwrap();
    // Both humans know the AI; seat 0 and the AI can trade embassies.
    lua.exec(
        r#"
        local ai = Players[2]:GetTeam()
        for seat = 0, 1 do Teams[Players[seat]:GetTeam()]:Meet(ai, false) end
        for _, owner in ipairs({0, 2}) do
            Teams[Players[owner]:GetTeam()]:SetHasTech(GameInfoTypes.TECH_WRITING, true, owner, false, false)
        end
        "#,
        "=meet-ai",
    )
    .unwrap();

    // The diplomacy overview offers Talk.
    let overview = query(&mut h, 0, "diplo", vec![]);
    let row = overview["majors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == AI)
        .expect("the AI is listed")
        .clone();
    assert_eq!(row["action"], "talk");
    assert_eq!(row["action_enabled"], true, "{row}");

    // The seat-scoped diplomacy queries of the core leave the simulation as it was (the worker runs them in the
    // read scope, as here). An applied order pumps the core, which moves the digest on by itself, so the actions'
    // own effects are checked by what they change, not by the digest.
    let before = digest(&mut h);
    lua.exec(
        r#"
        GcUiReadScope(function()
            local embassy = {{type = TradeableItems.TRADE_ITEM_ALLOW_EMBASSY, from = 0}}
            local theirs = {{type = TradeableItems.TRADE_ITEM_ALLOW_EMBASSY, from = 2}}
            for n = 0, 6 do assert(GcUiLeaderGreeting(0, 2, n) ~= "") end
            assert(GcUiDiploText(0, 2, "LETS_HEAR_IT") ~= "")
            assert(#GcUiDealQuery(0, 2, {}, "pockets") > 0)
            assert(GcUiDealQuery(0, 2, embassy, "check")[1])
            assert(GcUiDealQuery(0, 2, embassy, "evaluate").response == "accept")
            for mode = 0, 2 do
                GcUiDealQuery(0, 2, embassy, "equalize", mode)
                GcUiDealQuery(0, 2, theirs, "equalize", mode)
            end
            assert(#GcUiDealQuery(0, 2, embassy, "blob") > 0)
        end)
        "#,
        "=diplomacy-queries",
    )
    .unwrap();
    assert_eq!(digest(&mut h), before, "the diplomacy queries only read");

    // Talk: the leader window with the AI's own greeting.
    match act(&mut h, 0, "diplo", "begin_diplo", &[("a", AI)]) {
        CommandOutcome::ScreenApplied { open: Some(o), .. } => {
            assert_eq!((o.screen.as_str(), o.a), ("leader", Some(AI as i32)))
        }
        other => panic!("talk: {other:?}"),
    }
    let root = leader(&mut h, 0);
    assert_eq!(root["available"], true, "{root}");
    assert_eq!(root["mode"], "main");
    let greeting = root["phrase"].as_str().unwrap().to_owned();
    assert!(!greeting.is_empty(), "the AI greets the seat: {root}");
    // The other human talks to the same AI in the same turn: a session of its own.
    assert!(matches!(
        act(&mut h, 1, "diplo", "begin_diplo", &[("a", AI)]),
        CommandOutcome::ScreenApplied { .. }
    ));
    let other = leader(&mut h, 1);
    assert!(!other["phrase"].as_str().unwrap().is_empty());

    // Discuss: the AI listens (a leader's line, nothing sent).
    assert_eq!(
        act(&mut h, 0, "leader", "discuss", &[]),
        CommandOutcome::Applied
    );
    let root = leader(&mut h, 0);
    assert_eq!(root["mode"], "discussion");
    assert!(!root["phrase"].as_str().unwrap().is_empty());
    let topics: Vec<String> = root["responses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["key"].as_str().unwrap().to_owned())
        .collect();
    assert!(topics.contains(&"dont_settle".to_owned()), "{topics:?}");
    // Refreshing the leader windows of both seats reads only.
    let before = digest(&mut h);
    for seat in [0, 1] {
        leader(&mut h, seat);
    }
    assert_eq!(digest(&mut h), before, "leader queries only read");

    // A topic is the seat's ordered event; the AI's answer comes back to this seat's session.
    assert_eq!(
        act(&mut h, 0, "leader", "topic", &[("a", 3)]),
        CommandOutcome::Applied
    );
    let root = leader(&mut h, 0);
    assert!(
        !root["phrase"].as_str().unwrap().is_empty(),
        "the AI's reply reached the leader session: {root}"
    );
    let asked: bool = lua
        .load("return Players[2]:IsPlayerNoSettleRequestEverAsked(0)")
        .eval()
        .unwrap();
    assert!(asked, "the request reached the AI");
    let other_now = leader(&mut h, 1);
    assert_eq!(other_now["mode"], "main", "seat 1's session is untouched");
    assert_eq!(other_now["phrase"], other["phrase"]);

    // Trade: the AI's table for this seat.
    match act(&mut h, 0, "leader", "trade", &[]) {
        CommandOutcome::ScreenApplied { open: Some(o), .. } => assert_eq!(o.screen, "deal"),
        other => panic!("trade: {other:?}"),
    }
    let table = deal(&mut h);
    assert_eq!(table["kind"], "ai");
    assert_eq!(table["editable"], true, "{table}");
    let pockets = |t: &Value, side: &str| -> Vec<String> {
        t["pockets"][side]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["key"].as_str().unwrap().to_owned())
            .collect()
    };
    assert!(
        pockets(&table, "us").contains(&"embassy".to_owned()),
        "{table}"
    );
    let embassy = 17;
    assert_eq!(
        act(
            &mut h,
            0,
            "deal",
            "add",
            &[("a", 0), ("b", embassy), ("c", 0), ("d", 0)]
        ),
        CommandOutcome::Applied
    );
    let before = digest(&mut h);
    let table = deal(&mut h);
    assert_eq!(digest(&mut h), before, "the AI table query only reads");
    assert_eq!(table["table"]["us"].as_array().unwrap().len(), 1);
    assert!(table["question"].is_object(), "{table}");
    // What would make this work: valued on a copy; the leader answers.
    assert_eq!(act(&mut h, 0, "deal", "ask", &[]), CommandOutcome::Applied);
    assert!(!deal(&mut h)["phrase"].as_str().unwrap_or("").is_empty());
    let has: bool = lua
        .load("return Teams[Players[2]:GetTeam()]:HasEmbassyAtTeam(Players[0]:GetTeam())")
        .eval()
        .unwrap();
    assert!(!has, "drafting and asking enact nothing");

    // The gift of an embassy: the AI accepts, the deal goes out as the seat's order and the core enacts it.
    assert_eq!(
        act(&mut h, 0, "deal", "clear", &[]),
        CommandOutcome::Applied
    );
    assert_eq!(
        act(
            &mut h,
            0,
            "deal",
            "add",
            &[("a", 0), ("b", embassy), ("c", 0), ("d", 0)]
        ),
        CommandOutcome::Applied
    );
    match act(&mut h, 0, "deal", "propose", &[]) {
        CommandOutcome::ScreenApplied {
            open: Some(o),
            close: true,
            ..
        } => {
            assert_eq!(o.screen, "leader")
        }
        other => panic!("the AI accepts a gift: {other:?}"),
    }
    let has: bool = lua
        .load("return Teams[Players[2]:GetTeam()]:HasEmbassyAtTeam(Players[0]:GetTeam())")
        .eval()
        .unwrap();
    assert!(has, "the accepted deal was enacted");
    let root = leader(&mut h, 0);
    assert_eq!(root["mode"], "main");
    assert!(
        !root["phrase"].as_str().unwrap().is_empty(),
        "the AI's thanks reached the session: {root}"
    );
    assert_eq!(deal(&mut h)["table"]["us"].as_array().unwrap().len(), 0);
    assert!(matches!(
        act(&mut h, 0, "leader", "goodbye", &[]),
        CommandOutcome::ScreenApplied { close: true, .. }
    ));
}
