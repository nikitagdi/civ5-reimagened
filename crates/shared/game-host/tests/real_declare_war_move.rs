//! A seat's move into a rival's borders at peace raises the core's war question with the acting
//! unit; "yes" declares war and replays that unit's move, "no" leaves peace and the unit alone.
//! The question is asked only of seats whose client reads the HUD's popup head (the v2 client);
//! a classic seat keeps the old move behaviour and its own declare-then-move. An unanswered
//! question never blocks the end of turn, never piles up and lapses with the turn or the unit.
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
    /// A free land plot next to the unit outside the rival's land.
    free: (i32, i32),
    team: i32,
}

/// `v2`: the seat's client subscribes to the HUD (and so to popups) before it gives orders.
fn fixture(v2: bool) -> Fixture {
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
        local target, free
        for dx = -1, 1 do for dy = -1, 1 do
            local plot = Map.GetPlot(unit:GetX() + dx, unit:GetY() + dy)
            if plot and Map.PlotDistance(unit:GetX(), unit:GetY(), plot:GetX(), plot:GetY()) == 1
                and not plot:IsWater() and not plot:IsMountain() and not plot:IsImpassable()
                and plot:GetNumUnits() == 0 and not plot:IsCity() and plot:GetOwner() == -1 then
                if not target then target = plot elseif not free then free = plot end
            end
        end end
        assert(target and free, "two free land plots next to the unit")
        target:SetOwner(2, -1, true, true)
        assert(target:GetOwner() == 2, "AI 2 owns the plot")
        local team = Players[2]:GetTeam()
        Teams[p:GetTeam()]:Meet(team, true)
        assert(not Teams[p:GetTeam()]:IsAtWar(team) and Teams[p:GetTeam()]:CanDeclareWar(team), "met at peace")
        fixture_unit, fixture_x, fixture_y, fixture_team = unit:GetID(), target:GetX(), target:GetY(), team
        fixture_free_x, fixture_free_y = free:GetX(), free:GetY()
        function fixture_state()
            local u = Players[0]:GetUnitByID(fixture_unit)
            if not u then return -1, -1, Teams[Players[0]:GetTeam()]:IsAtWar(fixture_team) and 1 or 0 end
            return u:GetX(), u:GetY(), Teams[Players[0]:GetTeam()]:IsAtWar(fixture_team) and 1 or 0
        end
    "#,
        "=declare-war-move-fixture",
    )
    .unwrap();
    let get = |name: &str| -> i32 { lua.globals().get(name).unwrap() };
    let (unit, x, y, team) = (
        get("fixture_unit"),
        get("fixture_x"),
        get("fixture_y"),
        get("fixture_team"),
    );
    let free = (get("fixture_free_x"), get("fixture_free_y"));
    if v2 {
        let hud = screen_helper::query_args(&mut h, "hud", vec![]);
        assert!(hud["popup"].is_null(), "{hud}");
    }
    Fixture {
        h,
        lua,
        unit,
        x,
        y,
        free,
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

fn move_to(f: &mut Fixture, (x, y): (i32, i32)) -> CommandOutcome {
    f.h.cmd(
        0,
        Command::UnitMoveTo {
            unit: f.unit,
            x: x as i16,
            y: y as i16,
        },
    )
}

/// The move into the rival's land.
fn move_in(f: &mut Fixture) -> CommandOutcome {
    let target = (f.x, f.y);
    move_to(f, target)
}

/// The seat's popup head and queue length, as the HUD shows them.
fn popups(f: &mut Fixture) -> (serde_json::Value, i64) {
    let hud = screen_helper::query_args(&mut f.h, "hud", vec![]);
    (hud["popup"].clone(), hud["popup_count"].as_i64().unwrap())
}

/// Orders the move and answers the HUD's popup head and the war window for it.
fn ask(f: &mut Fixture) -> (i64, serde_json::Value) {
    let start = state(f);
    let moved = move_in(f);
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

fn real() -> bool {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return false;
    }
    true
}

#[test]
fn yes_declares_war_and_replays_the_acting_units_move() {
    if !real() {
        return;
    }
    let mut f = fixture(true);
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
    if !real() {
        return;
    }
    let mut f = fixture(true);
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

/// A seat that never read the HUD (the classic client) is not asked: the move goes to the core's
/// own move checks as before the question existed, and nothing waits in its popup queue.
#[test]
fn a_classic_seat_is_not_asked_and_keeps_the_old_move_checks() {
    if !real() {
        return;
    }
    let mut f = fixture(false);
    let start = state(&f);
    let outcome = move_in(&mut f);
    assert!(
        matches!(&outcome, CommandOutcome::Rejected { .. }),
        "the core's move check refuses a move into closed borders at peace: {outcome:?}"
    );
    assert_eq!(state(&f), start, "peace, the unit did not move");
    // Reading the HUD now (as a test) shows that no question was queued for the earlier order.
    let (head, count) = popups(&mut f);
    assert!(head.is_null(), "{head}");
    assert_eq!(count, 0);
}

/// The classic client's own way: it asks the player itself, then sends the declaration and the
/// move back to back. Both still work.
#[test]
fn a_classic_seat_declares_war_then_moves() {
    if !real() {
        return;
    }
    let mut f = fixture(false);
    let war = f.h.cmd(0, Command::DeclareWar { team: f.team as i8 });
    assert_eq!(war, CommandOutcome::Applied, "the declaration");
    let moved = move_in(&mut f);
    assert_eq!(moved, CommandOutcome::Applied, "the move after it");
    assert_eq!(
        state(&f),
        (f.x, f.y, true),
        "at war, the unit entered the plot"
    );
}

/// A declaration still waiting in the core's loopback (sent, not yet pumped) is the war this move
/// needs: the core does not ask again, the move follows the declaration.
#[test]
fn a_queued_declaration_is_not_asked_about_again() {
    if !real() {
        return;
    }
    let mut f = fixture(true);
    let g = f.h.w.core_mut().gamecore_mut();
    g.command(0, &gamecore::Command::DeclareWar { team: f.team })
        .expect("the declaration is accepted");
    // Sent before the declaration is delivered: the core's own move check then decides, as it did
    // before the question existed (refusal or order); either way no question is opened.
    let moved = g.command(
        0,
        &gamecore::Command::UnitMoveTo {
            unit: f.unit,
            x: f.x,
            y: f.y,
        },
    );
    g.pump(8).expect("the pump delivers the declaration");
    f.h.run();
    let (head, count) = popups(&mut f);
    assert!(
        head.is_null(),
        "no question for a declared war: {head} ({moved:?})"
    );
    assert_eq!(count, 0);
    assert!(state(&f).2, "at war");
    if moved.is_ok() {
        assert_eq!(
            (state(&f).0, state(&f).1),
            (f.x, f.y),
            "the unit entered the plot"
        );
    }
}

/// A question the seat never answers does not hold its end of turn and lapses with the turn.
#[test]
fn an_unanswered_question_lapses_at_the_end_of_turn() {
    if !real() {
        return;
    }
    let mut f = fixture(true);
    let start = state(&f);
    assert_eq!(move_in(&mut f), CommandOutcome::Applied);
    // The same order again replaces the question; it does not pile up.
    assert_eq!(move_in(&mut f), CommandOutcome::Applied);
    let (head, count) = popups(&mut f);
    assert_eq!(head["screen"], "declare_war", "{head}");
    assert_eq!(count, 1, "one question per unit");
    for seat in [0, 1] {
        f.h.rest_idle_units(seat);
    }
    f.h.next_turn(&[0, 1]);
    let (head, count) = popups(&mut f);
    assert!(head.is_null(), "the question lapsed with the turn: {head}");
    assert_eq!(count, 0);
    assert_eq!(state(&f).2, start.2, "still at peace");
}

/// A question lapses when its unit moves elsewhere.
#[test]
fn a_question_lapses_when_the_unit_moves() {
    if !real() {
        return;
    }
    let mut f = fixture(true);
    assert_eq!(move_in(&mut f), CommandOutcome::Applied);
    assert_eq!(popups(&mut f).1, 1);
    let free = f.free;
    assert_eq!(
        move_to(&mut f, free),
        CommandOutcome::Applied,
        "a move outside the rival's land"
    );
    assert_eq!((state(&f).0, state(&f).1), free, "the unit moved");
    let (head, count) = popups(&mut f);
    assert!(head.is_null(), "the question lapsed with the move: {head}");
    assert_eq!(count, 0);
}

/// A question lapses when its unit is gone.
#[test]
fn a_question_lapses_when_the_unit_is_gone() {
    if !real() {
        return;
    }
    let mut f = fixture(true);
    assert_eq!(move_in(&mut f), CommandOutcome::Applied);
    assert_eq!(popups(&mut f).1, 1);
    let unit = f.unit;
    assert_eq!(
        f.h.cmd(0, Command::UnitDisband { unit }),
        CommandOutcome::Applied
    );
    let (head, count) = popups(&mut f);
    assert!(head.is_null(), "the question lapsed with the unit: {head}");
    assert_eq!(count, 0);
}
