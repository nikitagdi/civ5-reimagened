//! Demands of an AI and peace talks with it on the real core: the AI answers the seat's demand for that seat and the
//! answer is the seat's ordered event; at war the AI talks peace only when it wants it, opens the table with the core's
//! treaties at the seat's own price, and a peace it accepts is enacted in order. The queries leave the digest as it was.
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

/// The leader's root says what it says now.
fn phrase(h: &mut common::H) -> String {
    leader(h, 0)["phrase"].as_str().unwrap().to_owned()
}

#[test]
fn demands_and_peace_talks_with_an_ai_are_answered_for_the_seat() {
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
    lua.exec(
        r#"
        local ai = Players[2]:GetTeam()
        for seat = 0, 1 do Teams[Players[seat]:GetTeam()]:Meet(ai, false) end
        "#,
        "=meet-ai",
    )
    .unwrap();
    // A turn passes: the AI has its capital and an income to demand.
    h.next_turn(&[0, 1]);
    let gpt: i64 = lua
        .load("return TradeableItems.TRADE_ITEM_GOLD_PER_TURN")
        .eval()
        .unwrap();

    // The demand and peace queries read only, the async draw of the demand included.
    let before = digest(&mut h);
    lua.exec(
        r#"
        GcUiReadScope(function()
            local r = GcUiDealQuery(0, 2, {{type = TradeableItems.TRADE_ITEM_GOLD_PER_TURN, from = 2, data1 = 1}}, "demand")
            assert(r.response ~= "none" and r.code >= 0, r.response .. " (the AI makes " .. Players[2]:CalculateGoldRate() .. " gold a turn)")
            local q = GcUiDealQuery(0, 2, {}, "peace")
            assert(not q.at_war and not q.willing and #q.items == 0)
        end)
        "#,
        "=demand-queries",
    )
    .unwrap();
    assert_eq!(
        digest(&mut h),
        before,
        "the demand and peace queries only read"
    );

    // A demand: the AI's answer is the seat's ordered event (which starts the AI's demand timer everywhere); the income
    // moves only when it gives in.
    assert!(matches!(
        act(&mut h, 0, "diplo", "begin_diplo", &[("a", AI)]),
        CommandOutcome::ScreenApplied { .. }
    ));
    match act(&mut h, 0, "leader", "demand", &[]) {
        CommandOutcome::ScreenApplied { open: Some(o), .. } => assert_eq!(o.screen, "deal"),
        other => panic!("demand: {other:?}"),
    }
    let table = deal(&mut h);
    assert_eq!(table["state"], "demand", "{table}");
    assert_eq!(table["pockets_visible"]["us"], false);
    assert_eq!(
        act(
            &mut h,
            0,
            "deal",
            "set_amount",
            &[("a", 1), ("b", gpt), ("c", 0), ("d", 1)]
        ),
        CommandOutcome::Applied
    );
    assert_eq!(deal(&mut h)["right_button"]["key"], "demand");
    match act(&mut h, 0, "deal", "demand", &[]) {
        CommandOutcome::ScreenApplied {
            open: Some(o),
            close: true,
            ..
        } => assert_eq!(o.screen, "leader"),
        other => panic!("the AI answers the demand: {other:?}"),
    }
    assert!(!phrase(&mut h).is_empty(), "the leader answers the demand");
    let again: String = lua
        .load(
            r#"return GcUiReadScope(function()
                return GcUiDealQuery(0, 2, {{type = TradeableItems.TRADE_ITEM_GOLD_PER_TURN, from = 2, data1 = 1}}, "demand").response
            end)"#,
        )
        .eval()
        .unwrap();
    assert_eq!(again, "refuse_too_soon", "the demand reached the AI");
    // The other seat's demand timer is its own.
    let other: String = lua
        .load(
            r#"return GcUiReadScope(function()
                return GcUiDealQuery(1, 2, {{type = TradeableItems.TRADE_ITEM_GOLD_PER_TURN, from = 2, data1 = 1}}, "demand").response
            end)"#,
        )
        .eval()
        .unwrap();
    assert_ne!(other, "refuse_too_soon");

    // War: the AI will not talk peace the turn it began.
    lua.exec(
        "Teams[Players[0]:GetTeam()]:DeclareWar(Players[2]:GetTeam())",
        "=war",
    )
    .unwrap();
    let root = leader(&mut h, 0);
    assert_eq!(root["at_war"], true);
    assert_eq!(
        act(&mut h, 0, "leader", "make_peace", &[]),
        CommandOutcome::Applied
    );
    let refused = phrase(&mut h);
    assert!(!refused.is_empty());
    assert!(matches!(
        act(&mut h, 0, "leader", "trade", &[]),
        CommandOutcome::Rejected { .. }
    ));
    // Turns pass until the AI wants peace with the seat; its price is the seat's own (the core's peace query).
    let mut willing = false;
    for _ in 0..12 {
        h.next_turn(&[0, 1]);
        willing = lua
            .load("return GcUiReadScope(function() return GcUiDealQuery(0, 2, {}, 'peace').willing end)")
            .eval()
            .unwrap();
        if willing {
            break;
        }
    }
    assert!(
        willing,
        "the AI wants peace after some turns of a war without battles"
    );
    assert!(matches!(
        act(&mut h, 0, "diplo", "begin_diplo", &[("a", AI)]),
        CommandOutcome::ScreenApplied { .. }
    ));
    match act(&mut h, 0, "leader", "make_peace", &[]) {
        CommandOutcome::ScreenApplied { open: Some(o), .. } => assert_eq!(o.screen, "deal"),
        other => panic!("peace talks: {other:?}"),
    }
    let table = deal(&mut h);
    assert_eq!(table["state"], "peace", "{table}");
    let treaty = table["table"]["us"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["fixed"] == true)
        .expect("the seat's treaty is on the table")
        .clone();
    assert_eq!(treaty["removable"], false);
    let before = digest(&mut h);
    deal(&mut h);
    assert_eq!(digest(&mut h), before, "the peace table query only reads");
    // Ask what ends the war, then offer what the AI asks; a peace it accepts ends the war in order.
    assert_eq!(act(&mut h, 0, "deal", "ask", &[]), CommandOutcome::Applied);
    match act(&mut h, 0, "deal", "propose", &[]) {
        CommandOutcome::ScreenApplied {
            open: Some(o),
            close: true,
            ..
        } => assert_eq!(o.screen, "leader"),
        CommandOutcome::Applied => {
            let t = deal(&mut h);
            panic!("the AI refused its own terms: {t}");
        }
        other => panic!("peace: {other:?}"),
    }
    let war: bool = lua
        .load("return Teams[Players[0]:GetTeam()]:IsAtWar(Players[2]:GetTeam())")
        .eval()
        .unwrap();
    assert!(!war, "the peace was enacted");
}
