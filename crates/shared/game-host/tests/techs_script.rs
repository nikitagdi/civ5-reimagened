//! Screen-local Lua contract checks with seat-explicit API fixtures.
use lua_sys::Lua;

#[test]
fn techs_modes_recheck_eligibility_and_emit_only_research_messages() {
    let lua = Lua::new().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    lua.exec(include_str!("fixtures/techs_api.lua"), "=fixture")
        .unwrap();
    lua.exec(
        &format!(
            "screen = (function() {} end)()",
            include_str!("../screens/techs.lua")
        ),
        "=techs",
    )
    .unwrap();
    lua.exec(
        r#"
        local d = screen.data(3, {})
        assert(#d.techs == 3 and #d.links == 1 and d.researched_count == 1)
        assert(d.techs[1].lock_reason and not d.techs[1].clickable)
        assert(d.techs[2].clickable and d.techs[2].status == "available")
        local sent = 0
        local net = {send=function(name, id, free, target, append)
            assert(name == "sendResearch" and id == 1)
            assert(free == free_count)
            sent = sent + 1
        end}
        screen.actions.research(3, {a=1,b=0}, net)
        screen.actions.research(3, {a=1,b=1}, net)
        assert(not pcall(screen.actions.research, 3, {a=0}, net))
        assert(not pcall(screen.actions.research, 3, {a=1,b=9}, net))
        free_count = 1
        assert(screen.data(3, {a=2}).mode == "free")
        screen.actions.research(3, {a=1}, net)
        assert(not pcall(screen.actions.research, 3, {a=2}, net))
        free_count = 0
        assert(screen.data(3, {a=3,b=1}).techs[2].steal_science == 30)
        screen.actions.steal(3, {a=1,b=1}, net)
        assert(not pcall(screen.actions.steal, 3, {a=1,b=3}, net))
        assert(not pcall(screen.actions.steal, 3, {a=0,b=1}, net))
        off = true
        assert(not screen.data(3, {}).techs[2].clickable)
        assert(not pcall(screen.actions.research, 3, {a=1}, net))
        assert(sent == 4)
    "#,
        "=assertions",
    )
    .unwrap();
}
