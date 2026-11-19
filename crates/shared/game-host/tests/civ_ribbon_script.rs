use lua_sys::{Lua, Table};
#[test]
fn ribbon_keeps_self_first_opens_ai_leaders_and_refuses_unavailable_server_actions() {
    let lua = Lua::new().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    let screen: Table = lua
        .load(include_str!("../screens/civ_ribbon.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua.exec(r#"
      function L(s) return s end
      GameDefines={MAX_CIV_PLAYERS=3,MAX_MAJOR_CIVS=3}
      GameInfoTypes={}
      GameInfo={Civilizations={[0]={Type='CIVILIZATION_TEST',ShortDescription='Civilization'}},Leaders={[0]={Type='LEADER_TEST',Description='Leader'}}}
      Game={IsNetworkMultiPlayer=function() return true end,IsOption=function() return false end,GetActiveLeague=function() return nil end}
      Players={}
      Teams={}
      for id=0,2 do
        local pid=id
        Players[id]={GetID=function() return pid end,GetTeam=function() return pid end,IsAlive=function() return true end,IsHuman=function() return pid==0 end,IsMinorCiv=function() return false end,GetCivilizationType=function() return 0 end,GetLeaderType=function() return 0 end,IsTurnActive=function() return pid==0 end,GetScore=function() return 100+pid end,HasReceivedNetTurnComplete=function() return true end,IsDenouncedPlayer=function() return false end,GetApproachTowardsUsGuess=function() return 0 end,GetGold=function() return 10 end,IsDoF=function() return false end,GetEspionageSpies=function() return {} end,GetNickName=function() error('account names are private') end}
        Teams[id]={GetNumMembers=function() return 1 end,IsHasMet=function(_,other) return other~=2 end,IsAtWar=function() return false end,CanDeclareWar=function() return true end,IsHasResearchAgreement=function() return false end,IsDefensivePact=function() return false end,IsAllowsOpenBordersToTeam=function() return false end}
      end
      local root=screen.data(0,{})
      assert(#root.majors==2 and root.majors[1].is_me and root.majors[1].player_key=='0:0')
      assert(root.majors[2].turn_state=='done' and root.majors[2].can_declare_war)
      assert(#root.session_reason>0)
      local ok,why=pcall(screen.actions.propose_peace,0,{a=1},{send=function() error('must not emit') end})
      assert(not ok and #why>0)
      assert(not pcall(screen.actions.propose_peace,0,{a=2},{}))
      -- A met AI leader opens the leader screen with the seat's session; an unmet one is refused.
      Game.GetGameTurn=function() return 1 end
      local opened=screen.actions.begin_diplo(0,{a=1},{send=function() error('must not emit') end})
      assert(opened.open.screen=='leader' and opened.open.a==1 and leader_session(0,1))
      assert(root.majors[2].action_reason==nil)
      assert(not pcall(screen.actions.begin_diplo,0,{a=2},{}))
    "#,"=ribbon").unwrap();
}
