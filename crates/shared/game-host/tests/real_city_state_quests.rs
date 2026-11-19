//! The city-state window words its quests as the original (formal texts in display order, turns
//! left, contest standings) and shows the core's tribute pressure breakdowns and scores.
use game_host::Worker;
use protocol::Start;
#[path = "common/real.rs"]
mod common;
#[allow(dead_code)]
#[path = "common/city_screen.rs"]
mod screen_helper;

#[test]
fn quests_and_tribute_pressure_come_from_the_core() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let mut setup = common::setup();
    setup.settings.minor_civs = 2;
    let (worker, outs) = Worker::new(common::real_core(), common::init(Start::New(setup), ""), 0);
    let mut h = common::H::with(worker, outs);
    // Quests need both capitals: the city-states found theirs in the first AI turn.
    for seat in [0, 1] {
        h.found_city(seat);
        h.rest_idle_units(seat);
    }
    h.next_turn(&[0, 1]);
    // Test-only bridge: seat 0 meets a city-state, which then gives it a quest it can give.
    let lua = lua_sys::Lua::new().unwrap();
    h.w.core_mut().gamecore_mut().attach_lua(&lua).unwrap();
    lua.exec(
        r#"
        local p = Players[0]
        for id = GameDefines.MAX_MAJOR_CIVS, GameDefines.MAX_CIV_PLAYERS - 1 do
            local m = Players[id]
            if not fixture_minor and m and m:IsMinorCiv() and m:IsAlive() then
                Teams[p:GetTeam()]:Meet(m:GetTeam(), true)
                for name, quest in pairs(MinorCivQuestTypes) do
                    if type(quest) == "number" and quest >= 0 and not fixture_minor and m:AddMinorCivQuestIfAble(0, quest)
                        and m:IsMinorCivDisplayedQuestForPlayer(0, quest) then fixture_minor = id end
                end
            end
        end
        assert(fixture_minor, "a city-state with a quest for seat 0")
        local m = Players[fixture_minor]
        fixture_can_gold = m:CanMajorBullyGold(0) and 1 or 0
        fixture_can_unit = m:CanMajorBullyUnit(0) and 1 or 0
        fixture_quests = 0
        for _, quest in pairs(MinorCivQuestTypes) do
            if type(quest) == "number" and m:IsMinorCivDisplayedQuestForPlayer(0, quest) then fixture_quests = fixture_quests + 1 end
        end
    "#,
        "=city-state-quest-fixture",
    )
    .unwrap();
    let minor: i64 = lua.globals().get::<_, i32>("fixture_minor").unwrap().into();
    let can_gold: i32 = lua.globals().get("fixture_can_gold").unwrap();
    let can_unit: i32 = lua.globals().get("fixture_can_unit").unwrap();
    let quests: usize = lua.globals().get::<_, i32>("fixture_quests").unwrap() as usize;
    drop(lua);
    let root = screen_helper::query_args(&mut h, "city_state", vec![("a".into(), minor)]);
    assert!(root["reason"].is_null(), "{root}");
    let rows = root["quests"].as_array().unwrap();
    assert!(!rows.is_empty() && rows.len() <= quests, "{root}");
    for row in rows {
        let title = row["title"].as_str().unwrap();
        assert!(
            !title.is_empty() && !title.contains("TXT_KEY_"),
            "a localized formal text: {row}"
        );
        assert!(row["tooltip"].as_str().unwrap().starts_with(title), "{row}");
        if row["type"].as_str().unwrap().starts_with("contest_") {
            assert!(
                row["contest_you"].is_i64() && row["contest_leader"].is_i64(),
                "{row}"
            );
        }
    }
    assert_eq!(root["no_quests_text"], "");
    assert!(root["threat_barbarians"].is_boolean() && root["threat_proxy_war"].is_boolean());
    let tribute = &root["tribute"];
    let pressure = tribute["pressure"].as_i64().expect("numeric pressure");
    assert_eq!(pressure >= 0, can_gold == 1, "{tribute}");
    assert_eq!(tribute["pressure_gold_needed"], (-pressure).max(0));
    assert_eq!(
        tribute["pressure_unit_needed"].as_i64().unwrap() == 0,
        can_unit == 1,
        "{tribute}"
    );
    for kind in ["gold", "unit"] {
        let tip = tribute[kind]["tooltip"].as_str().unwrap();
        assert!(
            !tip.is_empty() && !tip.contains("TXT_KEY_"),
            "{kind}: {tip}"
        );
    }
    // The original's influence costs: the defines are influence times 100.
    assert_eq!(tribute["gold"]["influence_loss"], 15);
    assert_eq!(tribute["unit"]["influence_loss"], 50);
}
