//! Trade orders revalidate the current seat and current legal destination list.
use lua_sys::Lua;

#[test]
fn trade_missions_use_owned_units_and_native_candidates() {
    let lua = Lua::new().unwrap();
    lua.sandbox().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    lua.exec(r#"
        GameInfoTypes = { MISSION_ESTABLISH_TRADE_ROUTE=1, MISSION_CHANGE_TRADE_UNIT_HOME_CITY=2, MISSION_CHANGE_ADMIRAL_PORT=3 }
        local city = { GetID=function() return 8 end, GetName=function() return "Harbor" end, GetOwner=function() return 0 end }
        local plot = { GetPlotIndex=function() return 10 end, GetPlotCity=function() return city end, GetX=function() return 2 end, GetY=function() return 3 end }
        Map = { GetPlot=function() return plot end, GetPlotByIndex=function() return plot end }
        GameInfo = { Civilizations={[0]={Type="CIV_TEST",ShortDescription="Civilization",DefaultPlayerColor=0}},
            PlayerColors={[0]={PrimaryColor=0,SecondaryColor=0}},Colors={[0]={Red=0,Green=0,Blue=0}} }
        YieldTypes = { YIELD_FOOD=0,YIELD_PRODUCTION=1,YIELD_GOLD=2,YIELD_SCIENCE=3,YIELD_CULTURE=4,YIELD_FAITH=5 }
        Locale = { ConvertTextKey=function(s) return s end }
        local unit = { GetID=function() return 7 end, GetOwner=function() return 0 end, IsDelayedDeath=function() return false end,
            IsTrade=function() return true end, CanStartMission=function() return true end }
        local yields={};for i=1,6 do yields[i]={Mine=100,Theirs=200} end
        Players = {[0]={GetUnitByID=function(_,id) if id==7 then return unit end end,
            GetCivilizationType=function() return 0 end,
            GetPotentialInternationalTradeRouteDestinations=function() return {{X=2,Y=3,TradeConnectionType=1,Yields=yields,OldTradeRoute=false}} end,
            GetPotentialTradeUnitNewHomeCity=function() return {{X=2,Y=3}} end,
            GetPotentialAdmiralNewPort=function() return {{X=2,Y=3}} end},
            [1]={GetUnitByID=function() return nil end}}
    "#, "=fixture").unwrap();
    let screen: lua_sys::Table = lua
        .load(include_str!("../screens/trade_routes.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua.exec(r#"
        local sent={};local net={send=function(...) sent[#sent+1]={...} end}
        local transition=screen.actions.establish(0,{a=7,b=10,c=1},net)
        assert(transition.close and transition.open.screen=="trade_routes" and transition.open.a==0)
        assert(sent[1][1]=="sendPushMission" and sent[1][2]==7 and sent[1][3]==1 and sent[1][4]==10 and sent[1][5]==1)
        local home=screen.actions.rehome(0,{a=7,b=10},net)
        assert(home.close and home.open.screen=="trade_routes")
        assert(sent[2][3]==2 and sent[2][4]==2 and sent[2][5]==3)
        local port=screen.actions.admiral_port(0,{a=7,b=10},net)
        assert(port.close and port.open.screen=="trade_routes")
        assert(sent[3][3]==3)
        for _,args in ipairs({{a=7,b=11,c=1},{a=7,b=10,c=8},{a=99,b=10,c=1}}) do
            local ok,reason=pcall(screen.actions.establish,0,args,net)
            assert(not ok and #reason>0 and #sent==3)
        end
        local ok,reason=pcall(screen.actions.establish,1,{a=7,b=10,c=1},net)
        assert(not ok and #reason>0 and #sent==3)
    "#, "=assertions").unwrap();
}
