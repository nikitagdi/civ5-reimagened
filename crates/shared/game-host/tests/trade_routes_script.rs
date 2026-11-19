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

#[test]
fn route_tooltip_follows_the_original_components() {
    let lua = Lua::new().unwrap();
    lua.sandbox().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    lua.exec(r#"
        Locale = { ConvertTextKey=function(s) return s end }
        DomainTypes = { DOMAIN_SEA=0, DOMAIN_LAND=2 }
        ResourceUsageTypes = { RESOURCEUSAGE_BONUS=0, RESOURCEUSAGE_STRATEGIC=1, RESOURCEUSAGE_LUXURY=2 }
        local resources = {{ID=0,Type="RESOURCE_IRON",Description="Iron"},{ID=1,Type="RESOURCE_WHEAT",Description="Wheat"}}
        GameInfo = { Civilizations={[0]={Type="CIV_A",ShortDescription="A",DefaultPlayerColor=0},[1]={Type="CIV_B",ShortDescription="B",DefaultPlayerColor=0}},
            PlayerColors={[0]={PrimaryColor=0,SecondaryColor=0}},Colors={[0]={Red=0,Green=0,Blue=0}},
            Leaders={[0]={Description="Leader"}},
            Resources=function() local i=0 return function() i=i+1 return resources[i] end end }
        Game = { GetResourceUsageType=function(id) return id==0 and 1 or 0 end, IsNetworkMultiPlayer=function() return true end }
        Map = { GetNumPlots=function() return 100 end }
        local function city(id, owner, name, plot, iron)
            return { GetID=function() return id end, GetOwner=function() return owner end, GetName=function() return name end,
                IsHasResourceLocal=function(_, r) return r==0 and iron end,
                Plot=function() return {GetPlotIndex=function() return plot end, IsRevealed=function() return true end} end }
        end
        local rome, tyre = city(1, 0, "Rome", 10, true), city(2, 1, "Tyre", 20, false)
        Teams = {[0]={IsHasMet=function() return true end}}
        local function player(id, nick)
            return { GetTeam=function() return 0 end, GetCivilizationType=function() return id end, GetLeaderType=function() return 0 end,
                GetNickName=function() return nick end, GetName=function() return "Name" .. id end,
                GetCivilizationAdjectiveKey=function() return "ADJ" .. id end,
                GetInternationalTradeRouteBaseBonus=function(_, _, _, origin) return origin and 300 or 100 end,
                GetInternationalTradeRouteGPTBonus=function(_, _, _, origin) return origin and 50 or 70 end,
                GetInternationalTradeRoutePolicyBonus=function() return 0 end,
                GetInternationalTradeRouteYourBuildingBonus=function() return 200 end,
                GetInternationalTradeRouteTheirBuildingBonus=function() return 0 end,
                GetInternationalTradeRouteResourceTraitModifier=function() return 0 end,
                GetInternationalTradeRouteExclusiveBonus=function() return 0 end,
                GetInternationalTradeRouteOtherTraitBonus=function() return 0 end,
                GetInternationalTradeRouteRiverModifier=function(_, _, _, _, origin) return origin and 25 or 0 end,
                GetInternationalTradeRouteDomainModifier=function() return 0 end,
                GetInternationalTradeRouteTotal=function(_, _, _, _, origin) return origin and 870 or 210 end,
                GetInternationalTradeRouteScience=function(_, _, _, _, origin) return origin and 0 or 300 end,
                GetNumTechDifference=function() return 3 end, GetInfluenceTradeRouteScienceBonus=function() return 1 end,
                GetTradeRoutesAvailable=function() return {} end, GetTradeRoutesToYou=function() return {} end,
                GetNumInternationalTradeRoutesUsed=function() return 1 end, GetNumInternationalTradeRoutesAvailable=function() return 2 end }
        end
        Players = {[0]=player(0, "seat"), [1]=player(1, "")}
        Players[0].GetTradeRoutes = function() return {{FromCity=rome, ToCity=tyre, Domain=2, FromGPT=870, ToGPT=210, FromScience=0, ToScience=300,
            ToFood=0, ToProduction=0, TurnsLeft=12}} end
    "#, "=fixture").unwrap();
    let screen: lua_sys::Table = lua
        .load(include_str!("../screens/trade_routes.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua.exec(r#"
        local root = screen.data(0, {})
        local tip = root.routes.yours[1].tooltip
        assert(tip and tip.route_id == root.routes.yours[1].id)
        local mine, theirs, science = tip.blocks[1], tip.blocks[2], tip.blocks[3]
        assert(#tip.blocks == 3 and mine.key == "my_gold" and mine.owner == "seat" and mine.total == 8.7)
        local ids = {}
        for i, l in ipairs(mine.lines) do ids[i] = l.id end
        -- Base, both cities' income, the origin building, the differing strategic resource, the river: the original's order.
        assert(table.concat(ids, ",") == "base,origin_city_gold,dest_city_gold,buildings:origin,resource:RESOURCE_IRON,river", table.concat(ids, ","))
        assert(mine.lines[1].value == 3 and mine.lines[2].arg == "Rome" and mine.lines[3].arg == "Tyre" and mine.lines[5].value == 0.5 and mine.lines[6].value == 25)
        -- The partner's revenue, under its leader name (no nickname), then its science from the techs it lacks.
        assert(theirs.key == "their_gold" and theirs.owner == "Name1" and theirs.total == 2.1 and theirs.lines[1].id == "base")
        assert(science.key == "their_science" and science.total == 3 and science.lines[1].value == 3 and science.lines[1].arg == "seat")
    "#, "=tooltip").unwrap();
}
