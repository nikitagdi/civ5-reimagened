//! A real conquest raises the core's ten-value capture event; the seat's popup queue and the
//! `city_captured` screen carry its Great Works count and the liberation player.
use game_host::Worker;
use protocol::{Command, CommandOutcome, Query, QueryAnswer, Start, ToWorker, WorkerMsg};
#[path = "common/real.rs"]
mod common;

fn query(h: &mut common::H, qid: u64, name: &str, args: Vec<(String, i64)>) -> serde_json::Value {
    h.send(ToWorker::Query {
        player: 0,
        qid,
        q: Query::Screen {
            screen: name.into(),
            args,
        },
    });
    h.run();
    let json = h
        .msgs()
        .find_map(|m| match m {
            WorkerMsg::QueryResult {
                qid: id,
                answer: QueryAnswer::Screen { json, .. },
            } if *id == qid => Some(json.clone()),
            WorkerMsg::QueryResult {
                qid: id,
                answer: QueryAnswer::Error { reason },
            } if *id == qid => panic!("{name}: {reason}"),
            _ => None,
        })
        .expect("screen answer");
    let root: serde_json::Value = serde_json::from_str(&json).unwrap();
    ui2_model::Sample::parse(
        ui2_model::Schema::v1(),
        &serde_json::json!({"model":{name:root.clone()}}).to_string(),
    )
    .unwrap_or_else(|e| panic!("{name} root matches the schema: {e:?}"));
    root
}

#[test]
fn conquest_event_carries_great_works_and_liberation() {
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
    // Test-only bridge: a city founded by seat 1 is handed to AI player 2 next to seat 0's
    // melee unit, left undefended with one hit point, and the two teams meet. Seat 0 then
    // declares war and attacks it with its own orders, so the core conquers it for seat 0 and
    // offers to return it to seat 1 (its living original owner).
    let lua = lua_sys::Lua::new().unwrap();
    h.w.core_mut().gamecore_mut().attach_lua(&lua).unwrap();
    lua.exec(
        r#"
        local p = Players[0]
        local unit
        -- A melee unit that may take cities (a scout may not).
        for u in p:Units() do
            if u:IsCombatUnit() and u:IsCanAttack() and not u:IsNoCapture() and not u:IsRanged() then unit = u; break end
        end
        assert(unit, "melee unit that captures")
        fixture_ux, fixture_uy = unit:GetX(), unit:GetY()
        local target
        for dx = -1, 1 do for dy = -1, 1 do
            local plot = Map.GetPlot(unit:GetX() + dx, unit:GetY() + dy)
            if not target and plot and Map.PlotDistance(unit:GetX(), unit:GetY(), plot:GetX(), plot:GetY()) == 1
                and not plot:IsWater() and not plot:IsMountain() and not plot:IsImpassable()
                and plot:GetNumUnits() == 0 and not plot:IsCity() and plot:GetOwner() == -1 then target = plot end
        end end
        assert(target, "free land next to the unit")
        local city = Players[1]:InitCity(target:GetX(), target:GetY())
        assert(city and city:GetOwner() == 1, "seat 1 city")
        Players[2]:AcquireCity(city, false, true)
        city = target:GetPlotCity()
        assert(city and city:GetOwner() == 2 and city:GetOriginalOwner() == 1, "AI 2 holds seat 1's city")
        city:SetDamage(city:GetMaxHitPoints() - 1)
        Teams[p:GetTeam()]:Meet(Players[2]:GetTeam(), true)
        -- The new borders push the unit out (no open borders); remember where it stood.
        fixture_unit, fixture_x, fixture_y, fixture_team = unit:GetID(), target:GetX(), target:GetY(), Players[2]:GetTeam()
    "#,
        "=capture-fixture",
    )
    .unwrap();
    let unit: i32 = lua.globals().get("fixture_unit").unwrap();
    let x: i32 = lua.globals().get("fixture_x").unwrap();
    let y: i32 = lua.globals().get("fixture_y").unwrap();
    let team: i32 = lua.globals().get("fixture_team").unwrap();
    let war = h.cmd(
        0,
        Command::DeclareWar {
            team: team.try_into().unwrap(),
        },
    );
    assert_eq!(war, CommandOutcome::Applied, "war on the city's owner");
    // At war the unit may stand in the enemy borders again: back next to the city, full moves.
    lua.exec(
        r#"
        local unit = Players[0]:GetUnitByID(fixture_unit)
        unit:SetXY(fixture_ux, fixture_uy)
        unit:SetMoves(unit:MaxMoves())
        assert(Map.PlotDistance(unit:GetX(), unit:GetY(), fixture_x, fixture_y) == 1, "next to the city")
        "#,
        "=capture-fixture-2",
    )
    .unwrap();
    drop(lua);
    let attack = h.cmd(
        0,
        Command::UnitMoveTo {
            unit,
            x: x as i16,
            y: y as i16,
        },
    );
    assert_eq!(attack, CommandOutcome::Applied, "the attack on the city");
    let city =
        h.w.fresh_view(0)
            .unwrap()
            .cities
            .iter()
            .find(|c| c.owner == 0 && (c.x, c.y) == (x as i16, y as i16))
            .map(|c| c.id)
            .expect("seat 0 conquered the city");

    let hud = query(&mut h, 901, "hud", vec![]);
    assert_eq!(hud["popup"]["screen"], "city_captured", "{hud}");
    assert_eq!(hud["popup"]["decision"], true);
    assert_eq!(hud["popup"]["a"], city);
    let popup = hud["popup"]["id"].as_i64().unwrap();

    let root = query(
        &mut h,
        902,
        "city_captured",
        vec![("a".into(), city.into()), ("popup_id".into(), popup)],
    );
    assert!(root["reason"].is_null(), "{root}");
    // The city held no Great Works: the core's count is zero, not a missing value.
    assert_eq!(root["loot"]["great_works_count"], 0);
    let options = root["options"].as_array().unwrap();
    let liberate = options.iter().find(|o| o["key"] == "liberate").unwrap();
    assert_eq!(liberate["enabled"], true, "{liberate}");
    assert!(liberate["reason"].is_null());
    assert!(!liberate["liberate_to"].as_str().unwrap().is_empty());
}
