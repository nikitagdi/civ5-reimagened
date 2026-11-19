//! An international route lists the original tooltip's components, and each block's total is the
//! yield the route row shows.
use game_host::Worker;
use protocol::Start;
#[path = "common/real.rs"]
mod common;
#[allow(dead_code)]
#[path = "common/city_screen.rs"]
mod screen_helper;

#[test]
fn international_route_tooltip_matches_the_route_yields() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let mut setup = common::setup();
    setup.settings.start_era = "ERA_RENAISSANCE".into();
    let (worker, outs) = Worker::new(common::real_core(), common::init(Start::New(setup), ""), 0);
    let mut h = common::H::with(worker, outs);
    h.found_city(0);
    // Test-only bridge: seat 1 gets a city near seat 0's capital, the two meet, and seat 0 gets a caravan.
    let lua = lua_sys::Lua::new().unwrap();
    h.w.core_mut().gamecore_mut().attach_lua(&lua).unwrap();
    lua.exec(
        r#"
        local p, q = Players[0], Players[1]
        local origin = p:GetCapitalCity()
        local target
        for i = 0, Map.GetNumPlots() - 1 do
            local plot = Map.GetPlotByIndex(i)
            local d = Map.PlotDistance(origin:GetX(), origin:GetY(), plot:GetX(), plot:GetY())
            if not target and d >= 4 and d < 8 and q:CanFound(plot:GetX(), plot:GetY()) and plot:GetArea() == origin:Plot():GetArea() then
                target = q:InitCity(plot:GetX(), plot:GetY())
            end
        end
        assert(target and target:GetOwner() == 1, "a seat 1 city near seat 0")
        Teams[p:GetTeam()]:Meet(q:GetTeam(), true)
        -- Trade paths cross revealed plots only.
        for i = 0, Map.GetNumPlots() - 1 do
            local plot = Map.GetPlotByIndex(i)
            if Map.PlotDistance(origin:GetX(), origin:GetY(), plot:GetX(), plot:GetY()) <= 10 then plot:SetRevealed(p:GetTeam(), true) end
        end
        fixture_unit = p:InitUnit(GameInfoTypes.UNIT_CARAVAN, origin:GetX(), origin:GetY()):GetID()
        fixture_city = target:GetID()
    "#,
        "=route-tooltip-fixture",
    )
    .unwrap();
    let unit: i64 = lua.globals().get::<_, i32>("fixture_unit").unwrap().into();
    let city: i64 = lua.globals().get::<_, i32>("fixture_city").unwrap().into();
    drop(lua);
    let choice = screen_helper::query_args(&mut h, "trade_routes", vec![("b".into(), unit)]);
    let dest = choice["choose"]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["items"].as_array().unwrap())
        .find(|d| d["city_id"] == city && d["connection"] == "international")
        .unwrap_or_else(|| panic!("an international destination: {choice}"))
        .clone();
    let outcome = h.cmd(
        0,
        protocol::Command::ScreenAction {
            screen: "trade_routes".into(),
            action: "establish".into(),
            args: vec![
                ("a".into(), unit),
                ("b".into(), dest["plot"].as_i64().unwrap()),
                ("c".into(), dest["connection_id"].as_i64().unwrap()),
            ],
            text: None,
        },
    );
    assert!(
        matches!(
            outcome,
            protocol::CommandOutcome::ScreenApplied { .. } | protocol::CommandOutcome::Applied
        ),
        "{outcome:?}"
    );
    let root = screen_helper::query_args(&mut h, "trade_routes", vec![]);
    let route = root["routes"]["yours"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["to"]["city_id"] == city)
        .unwrap_or_else(|| panic!("the established route: {root}"));
    let tip = &route["tooltip"];
    assert_eq!(tip["route_id"], route["id"]);
    let blocks = tip["blocks"].as_array().unwrap();
    let near = |a: &serde_json::Value, b: &serde_json::Value| {
        (a.as_f64().unwrap() - b.as_f64().unwrap()).abs() < 0.011
    };
    let mine = blocks
        .iter()
        .find(|b| b["key"] == "my_gold")
        .expect("your revenue");
    assert!(near(&mine["total"], &route["my_gold"]), "{mine} vs {route}");
    let terms: Vec<&str> = mine["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["key"].as_str().unwrap())
        .collect();
    assert_eq!(
        &terms[..3],
        ["base", "origin_city_gold", "dest_city_gold"],
        "{mine}"
    );
    assert!(!mine["owner"].as_str().unwrap().is_empty());
    for (key, column) in [
        ("their_gold", "their_gold"),
        ("my_science", "my_science"),
        ("their_science", "their_science"),
    ] {
        match blocks.iter().find(|b| b["key"] == key) {
            Some(b) => assert!(near(&b["total"], &route[column]), "{key}: {b} vs {route}"),
            None => assert_eq!(
                route[column].as_f64().unwrap(),
                0.0,
                "{key} has no block: {route}"
            ),
        }
    }
}
