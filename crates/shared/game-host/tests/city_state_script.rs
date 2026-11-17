use lua_sys::Lua;

#[path = "fixtures/culture_model.rs"]
mod model;

#[test]
fn city_state_gifts_and_orders_are_scoped_and_revalidated() {
    let lua = Lua::new().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    lua.exec(include_str!("fixtures/culture.lua"), "=fixture")
        .unwrap();
    lua.exec(r#"
        GameDefines.MAX_CIV_PLAYERS = 2
        GameDefines.FRIENDSHIP_THRESHOLD_ALLIES = 60
        GameDefines.FRIENDSHIP_THRESHOLD_FRIENDS = 30
        GameDefines.MINOR_FRIENDSHIP_AT_WAR = -60
        GameDefines.MINOR_GOLD_GIFT_SMALL = 250
        GameDefines.MINOR_GOLD_GIFT_MEDIUM = 500
        GameDefines.MINOR_GOLD_GIFT_LARGE = 1000
        GameDefines.MINOR_FRIENDSHIP_DROP_BULLY_GOLD_SUCCESS = -30
        GameInfo.MinorCivilizations = {[0] = {Description = "City-state", Type = "MINOR_CIV_TEST"}}
        GameInfoTypes.UNIT_WORKER = 2
        MinorCivQuestTypes = {}
        Players[0].GetGold = function() return gold end
        Players[0].IsMinorCiv = function() return false end
        local m = Players[1]
        m.IsMinorCiv = function() return true end
        for _, name in ipairs({"IsAllies", "IsFriends", "IsMinorPermanentWar", "IsPeaceBlocked", "IsProtectedByMajor", "IsMinorCivUnitSpawningDisabled", "CanMajorBuyout"}) do m[name] = function() return false end end
        for _, name in ipairs({"CanMajorStartProtection", "CanMajorWithdrawProtection", "CanMajorBullyGold", "CanMajorBullyUnit"}) do m[name] = function() return true end end
        for _, name in ipairs({"GetMinorCivType", "GetMinorCivTrait", "GetPersonality", "GetFriendshipChangePerTurnTimes100", "GetMinorCivFriendshipAnchorWithMajor", "GetBuyoutCost", "GetGiftTileImprovementCost", "GetMinorCivCurrentCultureBonus", "GetMinorCivCurrentFaithBonus", "GetMinorCivCurrentHappinessBonus", "GetCurrentCapitalFoodBonus", "GetCurrentOtherCityFoodBonus"}) do m[name] = function() return 0 end end
        m.GetMinorCivFriendshipWithMajor = function() return 15 end
        m.GetCapitalCity = function() return nil end
        m.GetAlly = function() return -1 end
        m.GetFriendshipFromGoldGift = function(self, player, amount) return amount / 10 end
        m.GetMinorCivBullyGoldAmount = function() return 100 end
        m.GetMajorBullyGoldDetails = function() return "Pressure" end
        Teams[0].CanChangeWarPeace = function() return true end
        Teams[0].CanDeclareWar = function() return true end
        Teams[0].GetNumTurnsLockedIntoWar = function() return 0 end
        Teams[0].GetNumTurnsAtWar = function() return 3 end
        Teams[1] = {IsAtWar = function() return false end}
        gold, known = 300, false
    "#, "=minor-fixture").unwrap();
    lua.exec(
        &format!(
            "cs = (function()\n{}\nend)()",
            include_str!("../screens/city_state.lua")
        ),
        "=city-state",
    )
    .unwrap();
    lua.exec(
        r#"
        assert(cs.data(0, {}).minor == nil)
        local sent = {}
        local net = {send = function(...) sent[#sent + 1] = {...} end}
        assert(not pcall(cs.actions.gift_gold, 0, {a = 1, b = 0}, net))
        known = true
        local model = cs.data(0, {a = 1})
        assert(model.gifts.gold[1].enabled and not model.gifts.gold[2].enabled)
        cs.actions.gift_gold(0, {a = 1, b = 0}, net)
        assert(sent[1][1] == "sendMinorGiftGold" and sent[1][2] == 1 and sent[1][3] == 250)
        assert(not pcall(cs.actions.gift_gold, 0, {a = 1, b = 1}, net))
        cs.actions.pledge(0, {a = 1, b = 1}, net)
        assert(sent[2][1] == "sendMinorPledgeProtection" and sent[2][2] == 1)
        cs.actions.bully_gold(0, {a = 1}, net)
        assert(sent[3][1] == "sendMinorBullyGold" and sent[3][3] == 100)
        cs.actions.bully_unit(0, {a = 1}, net)
        assert(sent[4][1] == "sendMinorBullyUnit" and sent[4][3] == 2)
        war = true
        assert(not pcall(cs.actions.gift_gold, 0, {a = 1, b = 0}, net))
        cs.actions.peace(0, {a = 1}, net)
        assert(sent[5][1] == "sendChangeWar" and sent[5][2] == 1 and sent[5][3] == 0)
        assert(not pcall(cs.actions.enter_land, 0, {a = 1}, net))
        assert(not pcall(cs.actions.buyout, 0, {a = 1}, net))
        assert(#sent == 5)
        local done
        net.popup_done = function(id) done=id end
        local captured = {GetID=function() return 8 end,GetName=function() return "Captured" end,
            GetOwner=function() return 0 end,GetOriginalOwner=function() return 1 end,
            IsOccupied=function() return true end,IsPuppet=function() return false end}
        Players[0].GetCityByID=function(self,id) if id==8 then return captured end end
        TaskTypes={TASK_CREATE_PUPPET=7}
        assert(not pcall(cs.actions.liberate,0,{a=1,b=8},net))
        assert(not pcall(cs.actions.puppet,0,{a=8},net))
        local popup={id=42,kind="BUTTONPOPUP_LIBERATE_MINOR",data1=1,data2=8,data3=0}
        local question=cs.data(0,{a=999,b=2,popup=popup})
        assert(question.liberate.city_id==8 and question.minor.id==1)
        assert(not pcall(cs.actions.liberate,0,{a=1,b=9,popup=popup,popup_id=42},net))
        local result=cs.actions.liberate(0,{a=1,b=8,popup=popup,popup_id=42},net)
        assert(result.close and done==42 and sent[6][1]=="SendLiberateMinor" and sent[6][3]==8)
        done=nil
        result=cs.actions.puppet(0,{a=8,popup=popup,popup_id=42},net)
        assert(result.close and done==42 and sent[7][1]=="sendDoTask" and sent[7][2]==8)
        done=nil
        result=cs.actions.cancel(0,{popup={kind="BUTTONPOPUP_MINOR_CIV_ENTER_TERRITORY"},popup_id=43},net)
        assert(result.close and done==43 and #sent==7)
    "#,
        "=minor-assertions",
    )
    .unwrap();
    model::check(&lua, "cs", "city_state");
}
