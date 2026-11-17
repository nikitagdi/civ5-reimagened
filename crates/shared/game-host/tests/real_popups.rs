//! Native popup capture and mapping against the real core's exported enums.
use game_host::Worker;
use protocol::{Query, QueryAnswer, Start, ToWorker, WorkerMsg};

#[path = "common/real.rs"]
mod common;

#[test]
fn native_ruin_choice_reaches_the_seat_hud() {
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
    let lua = lua_sys::Lua::new().unwrap();
    h.w.core_mut().gamecore_mut().attach_lua(&lua).unwrap();
    let mappings: lua_sys::Table = lua
        .load(include_str!("../screens/_popups.lua"))
        .eval()
        .unwrap();
    let enums: lua_sys::Table = lua.globals().get("ButtonPopupTypes").unwrap();
    for pair in mappings.pairs::<String, lua_sys::Table>() {
        let (kind, _) = pair.unwrap();
        assert!(
            matches!(
                enums.get::<_, lua_sys::LuaValue>(kind.as_str()).unwrap(),
                lua_sys::LuaValue::Number(_)
            ),
            "missing core enum {kind}"
        );
    }
    // The test-only bridge mutates the running game to produce a genuine AddPopup.
    // Screen scripts themselves continue to submit every mutation through net.send.
    lua.exec(
        r#"
        local p = Players[0]
        local unit
        for u in p:Units() do if u:IsCombatUnit() then unit = u; break end end
        assert(unit, "combat unit")
        local promotion = GameInfo.UnitPromotions.PROMOTION_GOODY_HUT_PICKER
        assert(promotion, "ruin picker promotion")
        unit:SetHasPromotion(promotion.ID, true)
        local plot = unit:GetPlot()
        plot:SetImprovementType(GameInfo.Improvements.IMPROVEMENT_GOODY_HUT.ID)
        p:DoGoody(plot, unit)
    "#,
        "=native-popup-fixture",
    )
    .unwrap();
    drop(mappings);
    drop(enums);
    drop(lua);
    h.send(ToWorker::Query {
        player: 0,
        qid: 701,
        q: Query::Screen {
            screen: "hud".into(),
            args: vec![],
        },
    });
    h.run();
    let json = h
        .msgs()
        .find_map(|m| match m {
            WorkerMsg::QueryResult {
                qid: 701,
                answer: QueryAnswer::Screen { json, .. },
            } => Some(json),
            _ => None,
        })
        .expect("HUD response");
    let root: serde_json::Value = serde_json::from_str(json).unwrap();
    assert_eq!(root["popup"]["screen"], "goody_choice");
    assert_eq!(root["popup"]["decision"], true);
    assert!(root["popup_count"].as_u64().unwrap() >= 1);
    let sample = ui2_model::Sample::parse(
        ui2_model::Schema::v1(),
        &serde_json::json!({"model":{"hud":root}}).to_string(),
    );
    assert!(sample.is_ok(), "popup root matches the public schema");
}
