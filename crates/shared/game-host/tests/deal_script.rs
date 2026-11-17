use lua_sys::{Lua, Table};
#[test]
fn drafts_are_seat_scoped_clamped_and_never_mutate_the_core() {
    let lua = Lua::new().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    let screen: Table = lua
        .load(include_str!("../screens/deal.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua.exec(r#"
      function L(s) return s end
      GameDefines={MAX_MAJOR_CIVS=4,PEACE_TREATY_LENGTH=10}
      Game={GetGameSpeedType=function() return 0 end,GetResourceUsageType=function() return 1 end}
      local resources={[2]={ID=2,Type='RESOURCE_TEST',Description='Test resource'}}
      setmetatable(resources,{__call=function() local once=false;return function() if not once then once=true;return resources[2] end end end})
      GameInfo={Civilizations={[0]={Type='CIVILIZATION_TEST',ShortDescription='Civilization'}},Leaders={[0]={Type='LEADER_TEST',Description='Leader'}},Resources=resources,GameSpeeds={[0]={DealDuration=30}}}
      Players={};Teams={}
      for id=0,3 do
        local pid=id
        Players[id]={IsAlive=function() return true end,GetTeam=function() return pid end,IsHuman=function() return pid~=3 end,IsTurnActive=function() return true end,IsDoF=function() return true end,GetGold=function() return 100 end,CalculateGoldRate=function() return 10 end,GetNumResourceAvailable=function() return 3 end,GetCivilizationType=function() return 0 end,GetLeaderType=function() return 0 end}
        Teams[id]={IsHasMet=function() return true end,IsAtWar=function() return false end,CanDeclareWar=function() return true end}
      end
      local proposals={}
      GcUiProposedDealExists=function(a,b) return proposals[a..':'..b] or false end
      local emitted={}
      local net={send=function(...) emitted[#emitted+1]={...} end}
      screen.data(0,{a=1})
      screen.actions.add_max(0,{a=0,b=0,c=0},net)
      assert(screen.data(0,{a=1}).table.us[1].amount==100)
      assert(#screen.data(2,{a=1}).table.us==0)
      screen.actions.set_amount(0,{a=0,b=0,c=0,d=300},net)
      assert(screen.data(0,{a=1}).table.us[1].amount==100)
      screen.actions.add(0,{a=1,b=3,c=2,d=0},net)
      assert(screen.data(0,{a=1}).table.them[1].amount==1)
      assert(not pcall(screen.actions.add,0,{a=4,b=0,c=0},net))
      for _,action in ipairs({'propose','accept','ask','demand','modify','switch'}) do
        local ok,why=pcall(screen.actions[action],0,{},net);assert(not ok and #why>0)
      end
      assert(#emitted==0)
      proposals['1:0']=true
      assert(screen.data(0,{a=1}).state=='player_incoming')
      assert(#screen.data(0,{a=1}).table.us==0)
      assert(not pcall(screen.actions.clear,0,{},net))
      screen.actions.refuse(0,{},net)
      assert(emitted[1][1]=='sendNetPlayerDealFinalized' and emitted[1][2]==1 and emitted[1][3]==0 and emitted[1][4]==0)
      proposals['1:0']=nil;proposals['0:1']=true
      screen.actions.withdraw(0,{},net)
      assert(emitted[2][2]==0 and emitted[2][3]==1 and emitted[2][4]==0)
      proposals['0:1']=nil
      screen.actions.remove(0,{a=0,b=0,c=0},net)
      screen.actions.clear(0,{},net)
      assert(#screen.data(0,{a=1}).table.them==0 and #emitted==2)
      assert(screen.actions.cancel(0,{},net).close)
      assert(screen.actions.declare_war(0,{},net).open.screen=='declare_war')
      screen.data(0,{a=3})
      assert(not pcall(screen.actions.add,0,{a=0,b=0,c=0},net))
    "#,"=deal-drafts").unwrap();
}
