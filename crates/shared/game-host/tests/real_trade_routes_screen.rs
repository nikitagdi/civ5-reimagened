//! Real route destinations and missions after a native city/unit test fixture is prepared.
use game_host::Worker;
use protocol::{Query, QueryAnswer, Start, ToWorker, WorkerMsg};
use ui2_model::{Sample, Schema};

#[path = "common/real.rs"]
mod common;

fn query(h: &mut common::H, qid: u64, player: i8) -> serde_json::Value {
    query_args(h, qid, player, vec![])
}

fn query_args(
    h: &mut common::H,
    qid: u64,
    player: i8,
    args: Vec<(String, i64)>,
) -> serde_json::Value {
    let before = h.frames.len();
    h.send(ToWorker::Query {
        player,
        qid,
        q: Query::Screen {
            screen: "trade_routes".into(),
            args,
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
        panic!("trade_routes query failed: {answer:?}")
    };
    assert_eq!(screen, "trade_routes");
    let root: serde_json::Value = serde_json::from_str(json).unwrap();
    Sample::parse(
        Schema::v1(),
        &serde_json::json!({"model":{"trade_routes":root}}).to_string(),
    )
    .expect("schema");
    root
}

#[test]
fn trade_routes_data_and_actions_validate_through_the_real_worker() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let mut setup = common::setup();
    setup.settings.start_era = "ERA_RENAISSANCE".into();
    let (worker, outs) = Worker::new(common::real_core(), common::init(Start::New(setup), ""), 0);
    let mut h = common::H::with(worker, outs);
    let root = query(&mut h, 1, 0);
    assert!(root["routes"]["yours"].as_array().unwrap().is_empty());
    h.found_city(0);
    let root = query(&mut h, 2, 0);
    assert!(root["totals"]["capacity"].is_number());
    assert!(root["routes"]["available"].is_array());
    let world = h.w.fresh_view(0).unwrap();
    let own = world
        .units
        .iter()
        .find(|u| u.owner == 0)
        .expect("own unit")
        .id;
    for action in ["establish", "rehome", "admiral_port"] {
        let outcome = h.cmd(
            0,
            protocol::Command::ScreenAction {
                screen: "trade_routes".into(),
                action: action.into(),
                args: vec![
                    ("a".into(), i64::from(own)),
                    ("b".into(), 0),
                    ("c".into(), 0),
                ],
                text: None,
            },
        );
        match outcome {
            protocol::CommandOutcome::Rejected { reason, .. } => assert!(!reason.trim().is_empty()),
            other => panic!("new-game non-trade unit must be refused: {other:?}"),
        }
    }
    // Fixture-only mutations reach meaningful trade actions without hundreds of autoplay turns.
    // These setters are confined to test setup; screen actions use net messages.
    let lua = lua_sys::Lua::new().unwrap();
    h.w.core_mut().gamecore_mut().attach_lua(&lua).unwrap();
    lua.exec(r#"
        function prepare_trade_fixture()
            local p = Players[0]
            local origin = p:GetCapitalCity()
            local target
            for i = 0, Map.GetNumPlots() - 1 do
                local plot = Map.GetPlotByIndex(i)
                if Map.PlotDistance(origin:GetX(), origin:GetY(), plot:GetX(), plot:GetY()) < 8 and p:CanFound(plot:GetX(), plot:GetY()) then
                    target = p:InitCity(plot:GetX(), plot:GetY())
                    break
                end
            end
            assert(target, "a second legal city is required")
            origin:SetNumRealBuilding(GameInfoTypes.BUILDING_GRANARY, 1)
            local route = p:InitUnit(GameInfoTypes.UNIT_CARAVAN, origin:GetX(), origin:GetY())
            local home = p:InitUnit(GameInfoTypes.UNIT_CARAVAN, origin:GetX(), origin:GetY())
            local admiral = p:InitUnit(GameInfoTypes.UNIT_GREAT_ADMIRAL, origin:GetX(), origin:GetY())
            return route:GetID(), home:GetID(), admiral:GetID(), target:Plot():GetPlotIndex()
        end
    "#, "=trade-fixture").unwrap();
    let ids = lua.call_global("prepare_trade_fixture", &[]).unwrap();
    let route = ids[0].as_i32().unwrap();
    let home = ids[1].as_i32().unwrap();
    let admiral = ids[2].as_i32().unwrap();
    let target = ids[3].as_i32().unwrap();
    drop(lua);
    let choice = query_args(&mut h, 20, 0, vec![("b".into(), i64::from(route))]);
    let destination = choice["choose"]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["items"].as_array().unwrap())
        .next()
        .expect("a granary and a second city provide a food destination");
    let plot = destination["plot"].as_i64().unwrap();
    let connection = destination["connection_id"].as_i64().unwrap();
    let used = choice["totals"]["used"].as_i64().unwrap();
    for (qid, action, unit, plot, connection) in [
        (21, "establish", route, plot, connection),
        (22, "rehome", home, i64::from(target), 0),
        (23, "admiral_port", admiral, i64::from(target), 0),
    ] {
        let outcome = h.cmd(
            0,
            protocol::Command::ScreenAction {
                screen: "trade_routes".into(),
                action: action.into(),
                args: vec![
                    ("a".into(), i64::from(unit)),
                    ("b".into(), plot),
                    ("c".into(), connection),
                ],
                text: None,
            },
        );
        match outcome {
            protocol::CommandOutcome::Applied | protocol::CommandOutcome::ScreenApplied { .. } => {
                if action == "establish" {
                    assert!(query(&mut h, qid, 0)["totals"]["used"].as_i64().unwrap() > used);
                } else {
                    let args = vec![
                        ("a".into(), if action == "rehome" { 3 } else { 4 }),
                        ("b".into(), i64::from(unit)),
                    ];
                    let fresh = query_args(&mut h, qid, 0, args);
                    assert_ne!(
                        fresh["new_home"]["unit"]["city_id"],
                        choice["choose"]["unit"]["home_city_id"]
                    );
                }
            }
            protocol::CommandOutcome::Rejected { reason, .. } => assert!(!reason.trim().is_empty()),
            other => panic!("unexpected trade outcome: {other:?}"),
        }
    }
}
