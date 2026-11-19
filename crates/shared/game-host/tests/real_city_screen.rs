//! Real city screen queries and actions. Stub builds exercise compilation only.
use game_host::Worker;
use protocol::{Command, CommandOutcome, Start};

#[path = "common/real.rs"]
mod common;

#[path = "common/city_screen.rs"]
mod screen_helper;
use screen_helper::query;

fn act(h: &mut common::H, city: i32, action: &str, a: i64, b: i64) -> CommandOutcome {
    h.cmd(
        0,
        Command::ScreenAction {
            screen: "city".into(),
            action: action.into(),
            args: vec![("a".into(), a), ("b".into(), b), ("d".into(), city.into())],
            text: None,
        },
    )
}

#[test]
fn city_root_and_orders_change_the_real_game() {
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
    let city =
        h.w.fresh_view(0)
            .unwrap()
            .cities
            .iter()
            .find(|c| c.owner == 0)
            .unwrap()
            .id;
    let root = query(&mut h, "city", city);
    assert_eq!(root["city"], city);
    assert!(!root["name"].as_str().unwrap().is_empty());
    assert!(!root["tiles"].as_array().unwrap().is_empty());
    assert_eq!(
        act(&mut h, city, "set_focus", 4, 0),
        CommandOutcome::Applied
    );
    assert_eq!(query(&mut h, "city", city)["focus"], "science");
    assert_eq!(
        act(&mut h, city, "avoid_growth", 1, 0),
        CommandOutcome::Applied
    );
    assert_eq!(query(&mut h, "city", city)["avoid_growth"], true);
    let build = root["production"]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["options"].as_array().unwrap())
        .find(|o| o["can_build"] == true)
        .unwrap()["item"]
        .as_i64()
        .unwrap();
    let count = root["production"]["queue"].as_array().unwrap().len();
    assert_eq!(
        act(&mut h, city, "push_order", build, 0),
        CommandOutcome::Applied
    );
    assert_eq!(
        query(&mut h, "city", city)["production"]["queue"]
            .as_array()
            .unwrap()
            .len(),
        count + 1
    );
    assert_eq!(
        act(&mut h, city, "move_order", count as i64, 0),
        CommandOutcome::Applied
    );
    assert_eq!(
        query(&mut h, "city", city)["production"]["queue"][0]["item"],
        build
    );
    assert_eq!(
        act(&mut h, city, "pop_order", 0, 0),
        CommandOutcome::Applied
    );
    assert_eq!(
        query(&mut h, "city", city)["production"]["queue"]
            .as_array()
            .unwrap()
            .len(),
        count
    );
    // A fresh city has no sale or specialist slot. Invalid targets must explain refusal.
    for action in [
        "purchase",
        "reset_tiles",
        "toggle_tile",
        "buy_plot",
        "add_specialist",
        "remove_specialist",
        "sell_building",
        "unemployed_to_work",
    ] {
        match act(
            &mut h,
            city,
            action,
            i64::from(i32::MAX),
            i64::from(i32::MAX),
        ) {
            CommandOutcome::Rejected { reason, .. } => assert!(!reason.is_empty(), "{action}"),
            other => panic!("{action}: invalid target accepted: {other:?}"),
        }
    }
    assert_eq!(
        act(&mut h, city, "manual_specialists", 1, 0),
        CommandOutcome::Applied
    );
    assert_eq!(query(&mut h, "city", city)["manual_specialists"], true);
}

/// The demand box and the great person meters follow the original city view: Lekmod's demand countdown from the
/// core, and meters whose turns agree with their rate.
#[test]
fn city_demand_countdown_and_great_person_meters_come_from_the_core() {
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
    for seat in [0, 1] {
        h.rest_idle_units(seat);
    }
    h.next_turn(&[0, 1]);
    let city =
        h.w.fresh_view(0)
            .unwrap()
            .cities
            .iter()
            .find(|c| c.owner == 0)
            .unwrap()
            .id;
    let lua = lua_sys::Lua::new().unwrap();
    h.w.core_mut().gamecore_mut().attach_lua(&lua).unwrap();
    lua.exec(
        &format!(
            "local c = Players[0]:GetCityByID({city}) demanded = c:GetResourceDemanded(true) countdown = c:GetResourceDemandedCountdown() wltkd = c:GetWeLoveTheKingDayCounter()"
        ),
        "=demand",
    )
    .unwrap();
    let demanded: i64 = lua.globals().get("demanded").unwrap();
    let countdown: i64 = lua.globals().get("countdown").unwrap();
    let wltkd: i64 = lua.globals().get("wltkd").unwrap();
    let root = query(&mut h, "city", city);
    let demand = &root["demand"];
    if demanded == -1 {
        assert!(demand.is_null(), "{demand}");
    } else {
        let text = demand["text"].as_str().unwrap();
        assert!(!text.contains("TXT_KEY"), "{text}");
        if wltkd > 0 {
            assert_eq!(demand["turns"].as_i64(), Some(wltkd));
        } else {
            assert_eq!(demand["kind"], "resource");
            assert_eq!(demand["turns"].as_i64(), Some(countdown));
            assert!(text.contains(&countdown.to_string()), "{text}");
        }
    }
    for gp in root["great_people"].as_array().unwrap() {
        if let Some(rate) = gp["rate"].as_f64() {
            let left = gp["threshold"].as_f64().unwrap() - gp["progress"].as_f64().unwrap();
            assert!(
                gp["turns"].as_i64().unwrap() >= (left / rate).floor() as i64,
                "{gp}"
            );
        }
        for m in gp["modifiers"].as_array().unwrap() {
            assert!(!m["label"].as_str().unwrap().contains("TXT_KEY"), "{m}");
        }
    }
}
