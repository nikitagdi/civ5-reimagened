//! Ranking ties, unknown identities, popup refresh stability and the replay mark.
use lua_sys::Lua;

#[test]
fn demographics_ranks_ties_and_filters_humans_without_exposing_unmet_civs() {
    let lua = Lua::new().unwrap();
    lua.sandbox().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    lua.exec(r#"
        humans=false
        GameInfoTypes={TECH_WRITING=1,ERA_INDUSTRIAL=4,GAMEOPTION_AI_GIMP_NO_DEMOGRAPHICS=9}
        GameOptionTypes={GAMEOPTION_NO_HAPPINESS=2,GAMEOPTION_NO_SCIENCE=3}
        GameDefines={MAX_MAJOR_CIVS=3}
        Game={IsNetworkMultiPlayer=function() return true end,IsOption=function(id) return id==9 and humans end,GetGameTurn=function() return 5 end}
        YieldTypes={YIELD_FOOD=0,YIELD_PRODUCTION=1}
        Locale={ConvertTextKey=function(s) return s end}
        GameInfo={Civilizations={[0]={Type="CIV_TEST",ShortDescription="Civilization",DefaultPlayerColor=0}},
            PlayerColors={[0]={PrimaryColor=0,SecondaryColor=0}},Colors={[0]={Red=0,Green=0,Blue=0}},
            Leaders={[0]={Description="Leader",Type="LEADER_TEST"}},
            Technologies=function() local done=false;return function() if not done then done=true;return {ID=1} end end end}
        Teams={};Players={}
        for id=0,2 do
            Teams[id]={IsHasTech=function() return true end,IsHasMet=function(_,other) return other==id end}
            local values={GetTeam=id,GetCivilizationType=0,GetLeaderType=0,GetRealPopulation=id==2 and 50 or 100,
                GetMilitaryMight=4,GetExcessHappiness=0,GetNumPlots=1,CalculateGrossGold=4,GetGold=id+1,GetNumPolicies=2,GetNumWorldWonders=0,GetCurrentEra=0,GetJONSCultureEverGenerated=10,GetInfluenceOn=0}
            Players[id]=setmetatable({Cities=function() return function() end end,IsAlive=function() return id~=2 end,
                IsHuman=function() return id~=1 end,IsMinorCiv=function() return false end,IsEverAlive=function() return true end},
                {__index=function(_,k) return function() assert(values[k]~=nil,k);return values[k] end end})
        end
    "#, "=fixture").unwrap();
    let screen: lua_sys::Table = lua
        .load(include_str!("../screens/demographics.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua.exec(r#"
        local root=screen.data(0,{})
        assert(root.participants==2 and #root.rows==8)
        local r=root.rows[1]
        assert(r.rank==1 and r.rank_shared and r.average==100)
        assert(r.worst.player==0)
        assert(root.rows[6].value==4000 and root.rows[7].value==60 and root.rows[8].value==100)
        humans=true;root=screen.data(0,{})
        assert(root.participants==2 and root.humans_only)
        r=root.rows[1];assert(r.average==75 and not r.rank_shared)
        assert(r.worst.player==2 and not r.worst.met and r.worst.name==nil and r.worst.icon=="")
        local a=screen.data(0,{a=1,b=1}).whos_winning
        local b=screen.data(0,{a=1,b=1}).whos_winning
        local c=screen.data(0,{a=1,b=2}).whos_winning
        assert(a.ranking_key==b.ranking_key and a.presenter_key==b.presenter_key)
        assert(a.ranking_key~=c.ranking_key and #a.rows==2)
        local sent={};screen.actions.mark_open(0,{}, {send=function(...) sent={...} end})
        assert(sent[1]=="sendGiftUnit" and sent[2]==-1 and sent[3]==-8)
        local ok,reason=pcall(screen.actions.mark_open,1,{}, {send=function() error("must not send") end})
        assert(not ok and #reason>0)
    "#, "=assertions").unwrap();
}
