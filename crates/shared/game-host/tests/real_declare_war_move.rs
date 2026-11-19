//! A seat's move into a rival's borders at peace raises the core's war question with the acting
//! unit; "yes" declares war and replays that unit's move, "no" leaves peace and the unit alone.
use game_host::Worker;
use protocol::{Command, CommandOutcome, Start};
#[path = "common/real.rs"]
mod common;
#[allow(dead_code)]
#[path = "common/city_screen.rs"]
mod screen_helper;

struct Fixture {
    h: common::H,
    lua: lua_sys::Lua,
    unit: i32,
    x: i32,
    y: i32,
    team: i32,
}

fn fixture() -> Fixture {
    let (worker, outs) = Worker::new(
        common::real_core(),
        common::init(Start::New(common::setup()), ""),
        0,
    );
    let mut h = common::H::with(worker, outs);
    // Test-only bridge: a land plot next to seat 0's combat unit becomes AI player 2's territory and
    // the two teams meet at peace without open borders. Seat 0 then orders the move itself.
    let lua = lua_sys::Lua::new().unwrap();
    h.w.core_mut().gamecore_mut().attach_lua(&lua).unwrap();
    lua.exec(
        r#"
        local p = Players[0]
        local unit
        for u in p:Units() do if u:IsCombatUnit() and u:CanMove() then unit = u; break end end
        assert(unit, "combat unit with moves")
        local target
        for dx = -1, 1 do for dy = -1, 1 do
            local plot = Map.GetPlot(unit:GetX() + dx, unit:GetY() + dy)
            if not target and plot and Map.PlotDistance(unit:GetX(), unit:GetY(), plot:GetX(), plot:GetY()) == 1
                and not plot:IsWater() and not plot:IsMountain() and not plot:IsImpassable()
                and plot:GetNumUnits() == 0 and not plot:IsCity() and plot:GetOwner() == -1 then target = plot end
        end end
        assert(target, "free land next to the unit")
        target:SetOwner(2, -1, true, true)
        assert(target:GetOwner() == 2, "AI 2 owns the plot")
        local team = Players[2]:GetTeam()
        Teams[p:GetTeam()]:Meet(team, true)
        assert(not Teams[p:GetTeam()]:IsAtWar(team) and Teams[p:GetTeam()]:CanDeclareWar(team), "met at peace")
        fixture_unit, fixture_x, fixture_y, fixture_team = unit:GetID(), target:GetX(), target:GetY(), team
        function fixture_state()
            local u = Players[0]:GetUnitByID(fixture_unit)
            return u:GetX(), u:GetY(), Teams[Players[0]:GetTeam()]:IsAtWar(fixture_team) and 1 or 0
        end
    "#,
        "=declare-war-move-fixture",
    )
    .unwrap();
    let unit: i32 = lua.globals().get("fixture_unit").unwrap();
    let x: i32 = lua.globals().get("fixture_x").unwrap();
    let y: i32 = lua.globals().get("fixture_y").unwrap();
    let team: i32 = lua.globals().get("fixture_team").unwrap();
    Fixture {
        h,
        lua,
        unit,
        x,
        y,
        team,
    }
}

fn state(f: &Fixture) -> (i32, i32, bool) {
    let v = f.lua.call_global("fixture_state", &[]).unwrap();
    (
        v[0].as_i32().unwrap(),
        v[1].as_i32().unwrap(),
        v[2].as_i32().unwrap() == 1,
    )
}

/// Orders the move and answers the HUD's popup head and the war window for it.
fn ask(f: &mut Fixture) -> (i64, serde_json::Value) {
    let start = state(f);
    let moved = f.h.cmd(
        0,
        Command::UnitMoveTo {
            unit: f.unit,
            x: f.x as i16,
            y: f.y as i16,
        },
    );
    assert_eq!(moved, CommandOutcome::Applied, "the move order is accepted");
    assert_eq!(state(f), start, "the question stops the move at peace");
    let hud = screen_helper::query_args(&mut f.h, "hud", vec![]);
    assert_eq!(hud["popup"]["screen"], "declare_war", "{hud}");
    assert_eq!(hud["popup"]["decision"], true);
    assert_eq!(hud["popup"]["a"], f.team);
    let popup = hud["popup"]["id"].as_i64().unwrap();
    let root = screen_helper::query_args(
        &mut f.h,
        "declare_war",
        vec![("a".into(), f.team.into()), ("popup_id".into(), popup)],
    );
    (popup, root)
}

#[test]
fn yes_declares_war_and_replays_the_acting_units_move() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let mut f = fixture();
    let (popup, root) = ask(&mut f);
    assert_eq!(root["reason"], "move", "{root}");
    assert_eq!(root["plot"]["x"], f.x);
    assert_eq!(root["plot"]["y"], f.y);
    assert_eq!(root["enabled"], true, "{root}");
    assert!(root["disabled_reason"].is_null(), "{root}");
    let answer = f.h.cmd(
        0,
        Command::ScreenAction {
            screen: "declare_war".into(),
            action: "confirm".into(),
            args: vec![("a".into(), f.team.into()), ("popup_id".into(), popup)],
            text: None,
        },
    );
    assert!(
        matches!(answer, CommandOutcome::ScreenApplied { close: true, .. }),
        "{answer:?}"
    );
    assert_eq!(
        state(&f),
        (f.x, f.y, true),
        "at war, the unit entered the plot"
    );
    let hud = screen_helper::query_args(&mut f.h, "hud", vec![]);
    assert_ne!(
        hud["popup"]["id"].as_i64(),
        Some(popup),
        "the question is answered"
    );
}

#[test]
fn no_keeps_peace_and_the_unit_in_place() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let mut f = fixture();
    let start = state(&f);
    let (popup, root) = ask(&mut f);
    assert_eq!(root["enabled"], true, "{root}");
    let answer = f.h.cmd(
        0,
        Command::ScreenAction {
            screen: "declare_war".into(),
            action: "cancel".into(),
            args: vec![("a".into(), f.team.into()), ("popup_id".into(), popup)],
            text: None,
        },
    );
    assert!(
        matches!(answer, CommandOutcome::ScreenApplied { close: true, .. }),
        "{answer:?}"
    );
    assert_eq!(state(&f), start, "peace, the unit did not move");
    let hud = screen_helper::query_args(&mut f.h, "hud", vec![]);
    assert_ne!(
        hud["popup"]["id"].as_i64(),
        Some(popup),
        "the question is answered"
    );
}
