use lua_sys::{Lua, Table};

#[test]
fn faith_purchase_maps_unit_and_building_kinds_and_rechecks_capital() {
    let lua = Lua::new().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    let screen: Table = lua
        .load(include_str!("../screens/religion_overview.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua.exec(r#"
        local p, c = {}, {}
        allowed = true
        function L(key) return key or '' end
        function p:GetCurrentEra() return 0 end
        function p:HasCreatedReligion() return false end
        function p:GetCapitalCity() return c end
        function p:DoesUnitPassFaithPurchaseCheck() return allowed end
        function c:IsCanPurchase() return allowed end
        function c:GetUnitFaithPurchaseCost() return 200 end
        function c:GetBuildingFaithPurchaseCost() return 100 end
        Players = {[3]=p}
        GameOptionTypes = {GAMEOPTION_NO_RELIGION=1}
        YieldTypes = {YIELD_FAITH=5}
        Game = {IsOption=function() return false end, GetNumReligionsStillToFound=function() return 1 end}
        local function catalog(row)
          return function() local once=false; return function() if not once then once=true; return row end end end
        end
        GameInfo = {Eras={ERA_INDUSTRIAL={ID=4}},
          Units=catalog({ID=10,Description='Unit'}), Buildings=catalog({ID=20,Description='Building'})}
        local sent=0
        local net={send=function(name,kind,id)
          assert(name=='SendFaithPurchase')
          if id==10 then assert(kind==3) elseif id==20 then assert(kind==2) else assert(id==-1 and (kind==0 or kind==1)) end
          sent=sent+1
        end}
        for _, args in ipairs({{a=0,b=-1},{a=1,b=-1},{a=2,b=10},{a=3,b=20}}) do screen.actions.auto_purchase(3,args,net) end
        assert(sent==4)
        allowed=false
        local ok,why=pcall(screen.actions.auto_purchase,3,{a=2,b=10},net)
        assert(not ok and #why>0)
        assert(not pcall(screen.actions.auto_purchase,3,{a=3,b=20},net))
        assert(not pcall(screen.actions.auto_purchase,4,{a=0,b=-1},net))
        assert(sent==4)
    "#, "=faith-purchase").unwrap();
}
