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
        assert(#d.techs[1].unlocks == 0)
        local unlocks = by_unlock_key(d.techs[2].unlocks)
        assert(#d.techs[2].unlocks == 16)
        for i, item in ipairs(d.techs[2].unlocks) do assert(item.idx == i - 1) end
        assert(unlocks.UNIT_UNIQUE.icon == "icon:UNIT_ATLAS:0")
        assert(not unlocks.UNIT_DEFAULT and not unlocks.UNIT_FOREIGN and not unlocks.UNIT_DISABLED)
        assert(unlocks.BUILDING_WONDER.kind == "wonder" and unlocks.PROJECT_TEST.kind == "project")
        assert(unlocks.BUILD_FARM.icon == "icon:BUILD_ATLAS:4")
        assert(not unlocks.BUILD_FOREIGN and not unlocks.BUILD_HIDDEN)
        assert(unlocks.RESOURCE_IRON.name == "Reveals Iron")
        assert(unlocks.RESOURCE_IRON.icon == "icon:RESOURCE_ATLAS:7" and unlocks.RESOURCE_IRON.kind == "resource")
        assert(not unlocks.RESOURCE_COAL)
        assert(unlocks.PROCESS_TEST.kind == "ability")
        assert(unlocks["ability:route:ROUTE_ROAD"].name == "Faster movement on Road")
        assert(unlocks["ability:Improvement_TechYieldChanges:IMPROVEMENT_FARM"].name == "Farm Food +1. Farm Gold +2.")
        assert(unlocks["ability:Improvement_TechFreshWaterYieldChanges:IMPROVEMENT_FARM"].name == "Farm with fresh water: Food +1")
        assert(unlocks["ability:Improvement_TechNoFreshWaterYieldChanges:IMPROVEMENT_FARM"].name == "Farm without fresh water: Food +1")
        assert(unlocks["ability:AllowsEmbarking"].name == "Allows embarkation")
        assert(unlocks["ability:AllowsEmbarking"].icon == "icon:GENERIC_FUNC_ATLAS:0")
        assert(unlocks["ability:EmbarkedAllWaterPassage"] and unlocks["ability:BridgeBuilding"])
        assert(not unlocks["ability:AllowsDefensiveEmbarking"] and not unlocks["ability:MapVisible"])
        assert(unlocks["ability:InternationalTradeRoutesChange"].name == "+2 international trade routes")
        assert(unlocks["ability:trade_range:DOMAIN_LAND"])
        assert(unlocks.PROMOTION_TEST.icon == "icon:PROMOTION_ATLAS:3")
        local writing = by_unlock_key(d.techs[3].unlocks)
        assert(writing["ability:OpenBordersTradingAllowed"] and writing["ability:DefensivePactTradingAllowed"])
        assert(writing["ability:ResearchAgreementTradingAllowed"] and writing["ability:MapTrading"])
        assert(writing["ability:EmbarkedMoveChange"] and writing.RESOURCE_COAL.icon == "")
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
        local steal = screen.data(3, {a=3,b=1})
        assert(steal.techs[2].steal_science == 30 and steal.steal.can_steal == 1)
        assert(steal.steal.spy_city == "Spy City" and steal.steal.civ == "Rival")
        assert(steal.steal.icon == "icon:CIV_ATLAS:2" and steal.steal.color == '#336699')
        spies[#spies + 1] = {State="TXT_KEY_SPY_STATE_GATHERING_INTEL", CityX=5, CityY=1}
        assert(screen.data(3, {a=3,b=1}).steal.spy_city == "")
        spies = {}
        assert(screen.data(3, {a=3,b=1}).steal.spy_city == "")
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
