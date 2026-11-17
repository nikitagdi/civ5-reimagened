//! Decision screens never authorize orders from client selection or event arguments alone.
use lua_sys::{Lua, LuaValue, Table};
fn runtime() -> Lua {
    let lua = Lua::new().unwrap();
    lua.sandbox().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    lua.exec(include_str!("fixtures/city_api.lua"), "=city-api")
        .unwrap();
    lua.exec(
        include_str!("fixtures/city_decisions_api.lua"),
        "=decisions-api",
    )
    .unwrap();
    for name in ["city_rename", "city_captured", "free_great_person"] {
        let source = game_host::screens::REGISTRY
            .iter()
            .find(|(n, _)| *n == name)
            .unwrap()
            .1;
        let screen: Table = lua.load(source).eval().unwrap();
        lua.globals().raw_set(name, screen).unwrap();
    }
    lua
}
#[path = "common/city_json.rs"]
mod city_json;
use city_json::json;

#[test]
fn city_decision_roots_match_schema_in_every_mode() {
    let lua = runtime();
    for (name, modes) in [
        ("city_rename", 4),
        ("city_captured", 2),
        ("free_great_person", 3),
    ] {
        for mode in 0..modes {
            let args = lua.create_table();
            args.raw_set("a", if name == "free_great_person" { mode } else { 7 })
                .unwrap();
            args.raw_set("b", mode).unwrap();
            let screen: Table = lua.globals().get(name).unwrap();
            let root: LuaValue = screen
                .get::<_, lua_sys::Function>("data")
                .unwrap()
                .call((3, args))
                .unwrap();
            ui2_model::Sample::parse(
                ui2_model::Schema::v1(),
                &serde_json::json!({"model":{name:json(root)}}).to_string(),
            )
            .expect("decision root matches schema");
        }
    }
}
#[test]
fn rename_and_capture_refuse_unavailable_operations_with_reasons() {
    runtime()
        .exec(
            r#"
        local net={send=function(...) last_message={...} end}
        for _,name in ipairs({'ab','bad/name','Valid name'}) do
            local ok,why=pcall(city_rename.actions.rename,3,{a=7,text=name},net)
            assert(not ok and #why>0 and not last_message)
        end
        assert(not pcall(city_rename.actions.do_task,3,{a=7,b=10},net))
        for _,name in ipairs({'puppet','liberate','raze','annex'}) do
            local ok,why=pcall(city_captured.actions[name],3,{a=7},net)
            assert(not ok and #why>0)
        end
        city_puppet=true
        city_rename.actions.do_task(3,{a=7,b=12},net)
        assert(last_message[1]=='sendDoTask' and last_message[2]==7 and last_message[3]==12)
        city_captured.actions.annex(3,{a=7},net)
        assert(last_message[3]==12)
        assert(not pcall(city_rename.actions.do_task,1,{a=7,b=12},net))
    "#,
            "=decision-actions",
        )
        .unwrap();
}
#[test]
fn great_person_rechecks_mode_quota_civilization_and_policy_before_emitting() {
    runtime().exec(r#"
        local net={send=function(...) last_message={...} end}
        assert(not pcall(free_great_person.actions.choose,3,{a=2},net))
        local free=free_great_person.data(3,{a=0})
        assert(#free.people==1 and free.people[1].available)
        free_great_person.actions.choose(3,{a=2,b=0},net)
        assert(last_message[1]=='sendGreatPersonChoice' and last_message[2]==2 and last_message[3]==nil)
        gp_remaining.free=0
        assert(not pcall(free_great_person.actions.choose,3,{a=2,b=0},net))
        assert(not pcall(free_great_person.actions.choose,3,{a=99},net))
        local faith=free_great_person.data(3,{a=1})
        assert(not faith.people[1].available and faith.people[1].reason)
        assert(not pcall(free_great_person.actions.choose,3,{a=2,b=1},net))
        gp_policy=true; free_great_person.actions.choose(3,{a=2,b=1},net)
        assert(last_message[1]=='sendFaithGreatPersonChoice')
        gp_baktun=13; local maya=free_great_person.data(3,{a=2})
        assert(not maya.people[1].available and maya.can_close)
        assert(not pcall(free_great_person.actions.choose,3,{a=2,b=2},net))
        gp_baktun=0; free_great_person.actions.choose(3,{a=2,b=2},net)
        assert(last_message[1]=='sendMayaBonusChoice')
        assert(not pcall(free_great_person.actions.choose,1,{a=2,b=2},net))
    "#, "=gp-actions").unwrap();
}
