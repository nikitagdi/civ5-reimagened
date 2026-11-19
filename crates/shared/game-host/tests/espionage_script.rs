//! Spy actions validate the seat, lifecycle, destination and diplomat role; a coup's result is shown once.
use lua_sys::Lua;

#[test]
fn a_coup_staged_from_the_window_shows_its_result_until_closed() {
    let lua = Lua::new().unwrap();
    lua.sandbox().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    lua.exec(r#"
        spies={{AgentID=1,State="TXT_KEY_SPY_STATE_RIGGING_ELECTION",CityX=2,CityY=3}}
        notes={}
        Locale={ConvertTextKey=function(key,...) return table.concat({key,...},"|") end}
        local cs={GetNameKey=function() return "TXT_KEY_CITY_VILNIUS" end}
        Map={GetPlot=function(x,y) if x==2 and y==3 then return {GetPlotCity=function() return cs end} end end}
        Game={GetGameTurn=function() return 40 end,IsOption=function() return false end}
        GameOptionTypes={GAMEOPTION_NO_ESPIONAGE=1}
        GameInfoTypes={ERA_RENAISSANCE=3}
        GameInfo={Civilizations={[0]={Type="CIVILIZATION_X",ShortDescription="X",DefaultPlayerColor="PC"}},
            PlayerColors={PC={PrimaryColor="A",SecondaryColor="B"}},Colors={A={Red=1,Green=0,Blue=0},B={Red=0,Green=0,Blue=1}},
            Leaders={[0]={Description="Leader"}}}
        Players={[0]={GetEspionageSpies=function() return spies end,GetTeam=function() return 0 end,
            CanSpyStageCoup=function() return true end,GetEspionageCityStatus=function() return {} end,
            GetIntrigueMessages=function() return {} end,GetCivilizationType=function() return 0 end,
            GetLeaderType=function() return 0 end,GetCurrentEra=function() return 0 end,
            GetNumNotifications=function() return #notes end,
            GetNotificationSummaryStr=function(_,i) return notes[i+1][1] end,
            GetNotificationStr=function(_,i) return notes[i+1][2] end,
            GetNotificationTurn=function(_,i) return notes[i+1][3] end}}
    "#, "=fixture").unwrap();
    let screen: lua_sys::Table = lua
        .load(include_str!("../screens/espionage.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua.exec(r#"
        local net={send=function() end}
        screen.actions.coup(0,{a=1},net)
        -- The spy is gone from the list once the coup is settled (here: before the window reads again).
        spies={}
        assert(screen.data(0,{}).coup_result==nil, "no result before the core's notification")
        notes={{"Old news","",39},{L("TXT_KEY_NOTIFICATION_SPY_YOU_STAGE_COUP_FAILURE_S","TXT_KEY_CITY_VILNIUS"),"The coup failed.",40}}
        local r=screen.data(0,{}).coup_result
        assert(r and r.success==false and r.text=="The coup failed.", "the failure is shown")
        notes={}
        assert(screen.data(0,{}).coup_result.text=="The coup failed.", "shown until closed")
        screen.actions.coup_seen(0,{},net)
        assert(screen.data(0,{}).coup_result==nil, "closed: gone")
    "#, "=assertions").unwrap();
}

#[test]
fn espionage_orders_validate_before_emitting_messages() {
    let lua = Lua::new().unwrap();
    lua.sandbox().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    lua.exec(r#"
        local spies={{AgentID=0,State="TXT_KEY_SPY_STATE_UNASSIGNED",CityX=-1,CityY=-1},
            {AgentID=1,State="TXT_KEY_SPY_STATE_RIGGING_ELECTION",CityX=2,CityY=3},
            {AgentID=2,State="TXT_KEY_SPY_STATE_DEAD",CityX=2,CityY=3}}
        local city={IsCapital=function() return true end,Plot=function() return {IsRevealed=function() return true end} end}
        Players={[0]={GetEspionageSpies=function() return spies end,GetTeam=function() return 0 end,
            GetAvailableSpyRelocationCities=function() return {{PlayerID=1,CityID=8}} end,
            CanSpyStageCoup=function(_,id) return id==1 end},
            [1]={GetEspionageSpies=function() return {} end,GetTeam=function() return 1 end,
            GetCityByID=function(_,id) if id==8 then return city end end,IsMinorCiv=function() return false end}}
        Teams={[0]={IsAtWar=function() return false end,IsHasMet=function() return true end}}
        local cs={GetNameKey=function() return "TXT_KEY_CITY_VILNIUS" end}
        Map={GetPlot=function(x,y) if x==2 and y==3 then return {GetPlotCity=function() return cs end} end end}
        Game={GetGameTurn=function() return 40 end}
    "#, "=fixture").unwrap();
    let screen: lua_sys::Table = lua
        .load(include_str!("../screens/espionage.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua.exec(r#"
        local sent={};local net={send=function(...) sent[#sent+1]={...} end}
        screen.actions.move(0,{a=0,b=1,c=8,d=1},net)
        -- The core's message arguments omit the sender: the worker submits them as the seat
        -- (SendMoveSpy: spy, target player, target city, as diplomat; SendStageCoup: spy).
        assert(#sent[1]==5 and sent[1][1]=="SendMoveSpy" and sent[1][2]==0 and sent[1][3]==1 and sent[1][4]==8 and sent[1][5]==1)
        screen.actions.hideout(0,{a=1},net)
        assert(#sent[2]==5 and sent[2][2]==1 and sent[2][3]==-1 and sent[2][4]==-1 and sent[2][5]==0)
        screen.actions.coup(0,{a=1},net)
        assert(#sent[3]==2 and sent[3][1]=="SendStageCoup" and sent[3][2]==1)
        for _,test in ipairs({{"move",0,{a=0,b=1,c=9,d=0}}, {"move",1,{a=0,b=1,c=8,d=0}},
            {"hideout",0,{a=0}}, {"coup",0,{a=0}}, {"coup",0,{a=2}}}) do
            local ok,reason=pcall(screen.actions[test[1]],test[2],test[3],net)
            assert(not ok and #reason>0 and #sent==3)
        end
    "#, "=assertions").unwrap();
}
