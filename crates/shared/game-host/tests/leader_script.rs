use lua_sys::{Lua, Table};
#[test]
fn dossier_never_fabricates_dialogue_or_answers_a_pending_decision() {
    let lua = Lua::new().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    let screen: Table = lua
        .load(include_str!("../screens/leader.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua.exec(r#"
      function L(s) return s end
      GameDefines={MAX_MAJOR_CIVS=3};GameInfoTypes={}
      GameInfo={Civilizations={[0]={Type='CIVILIZATION_TEST',ShortDescription='Civilization'}},Leaders={[0]={Type='LEADER_TEST',Description='Leader'}},PolicyBranchTypes={}}
      Players={};Teams={}
      for id=0,2 do local pid=id
        Players[id]={IsAlive=function() return true end,IsHuman=function() return pid==0 end,GetTeam=function() return pid end,IsTurnActive=function() return true end,GetCivilizationType=function() return 0 end,GetLeaderType=function() return 0 end,IsDenouncedPlayer=function() return false end,GetApproachTowardsUsGuess=function() return 0 end,GetCapitalCity=function() return nil end,GetLateGamePolicyTree=function() return -1 end,GetOpinionTable=function() return {'[COLOR_RED]Concern[ENDCOLOR]'} end,IsDoF=function() return false end,GetNumCities=function() return 0 end,GetScore=function() return 100 end}
        Teams[id]={IsAtWar=function() return false end,IsHasMet=function() return true end,CanDeclareWar=function() return true end,IsAllowsOpenBordersToTeam=function() return false end}
      end
      local root=screen.data(0,{a=1})
      assert(not root.available and #root.unavailable_reason>0 and root.phrase=='' and #root.responses==0)
      assert(root.mood.reasons[1].text=='Concern' and root.mood.reasons[1].value==nil)
      assert(screen.actions.declare_war(0,{}).open.a==1)
      assert(screen.actions.goodbye(0,{}).close)
      local net={send=function() error('must not emit') end}
      for _,action in ipairs({'discuss','trade','demand','make_peace','topic','respond','coop_war','confirm','back','switch'}) do
        local ok,why=pcall(screen.actions[action],0,{a=1},net)
        assert(not ok and #why>0)
      end
      assert(not pcall(screen.actions.goodbye,0,{popup={}},net))
      assert(not pcall(screen.actions.declare_war,0,{popup={}},net))
      assert(not screen.data(0,{a=1,popup={}}).can_close)
    "#,"=leader-dossier").unwrap();
}
