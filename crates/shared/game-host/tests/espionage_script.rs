//! Spy actions validate the seat, lifecycle, destination and diplomat role.
use lua_sys::Lua;

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
    "#, "=fixture").unwrap();
    let screen: lua_sys::Table = lua
        .load(include_str!("../screens/espionage.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua.exec(r#"
        local sent={};local net={send=function(...) sent[#sent+1]={...} end}
        screen.actions.move(0,{a=0,b=1,c=8,d=1},net)
        assert(sent[1][1]=="SendMoveSpy" and sent[1][2]==0 and sent[1][3]==0 and sent[1][4]==1 and sent[1][5]==8 and sent[1][6]==1)
        screen.actions.hideout(0,{a=1},net)
        assert(sent[2][4]==-1 and sent[2][5]==-1)
        screen.actions.coup(0,{a=1},net)
        assert(sent[3][1]=="SendStageCoup" and sent[3][2]==0 and sent[3][3]==1)
        for _,test in ipairs({{"move",0,{a=0,b=1,c=9,d=0}}, {"move",1,{a=0,b=1,c=8,d=0}},
            {"hideout",0,{a=0}}, {"coup",0,{a=0}}, {"coup",0,{a=2}}}) do
            local ok,reason=pcall(screen.actions[test[1]],test[2],test[3],net)
            assert(not ok and #reason>0 and #sent==3)
        end
    "#, "=assertions").unwrap();
}
