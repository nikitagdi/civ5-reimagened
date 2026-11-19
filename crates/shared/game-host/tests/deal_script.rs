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

#[test]
fn ai_table_is_checked_valued_and_sent_by_the_core_for_the_seat() {
    let lua = Lua::new().unwrap();
    lua.exec(r#"
      TradeableItems={TRADE_ITEM_GOLD=0,TRADE_ITEM_GOLD_PER_TURN=1,TRADE_ITEM_RESOURCES=3,TRADE_ITEM_OPEN_BORDERS=6,
        TRADE_ITEM_DEFENSIVE_PACT=7,TRADE_ITEM_RESEARCH_AGREEMENT=8,TRADE_ITEM_PEACE_TREATY=13,TRADE_ITEM_THIRD_PARTY_PEACE=14,
        TRADE_ITEM_THIRD_PARTY_WAR=15,TRADE_ITEM_ALLOW_EMBASSY=17}
    "#, "=enums").unwrap();
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
      local usage={[2]=2,[5]=1}
      Game={GetGameSpeedType=function() return 0 end,GetResourceUsageType=function(id) return usage[id] or 0 end,GetGameTurn=function() return 1 end}
      local resources={[2]={ID=2,Type='RESOURCE_SILK',Description='Silk'},[5]={ID=5,Type='RESOURCE_IRON',Description='Iron'}}
      setmetatable(resources,{__call=function() local list,i={resources[2],resources[5]},0;return function() i=i+1;return list[i] end end})
      GameInfo={Civilizations={[0]={Type='CIVILIZATION_TEST',ShortDescription='Civilization'}},Leaders={[0]={Type='LEADER_TEST',Description='Leader'}},Resources=resources,GameSpeeds={[0]={DealDuration=30}}}
      Players={};Teams={}
      for id=0,3 do
        local pid=id
        Players[id]={IsAlive=function() return true end,GetTeam=function() return pid end,IsHuman=function() return pid<2 end,IsTurnActive=function() return true end,GetGold=function() return 100 end,GetCivilizationType=function() return 0 end,GetLeaderType=function() return 0 end}
        Teams[id]={IsHasMet=function() return true end,IsAtWar=function() return false end,CanDeclareWar=function() return true end}
      end
      -- The core's answers for seat 0 and AI 2: the seat may give gold (max 100) and iron (3), the AI silk and open borders.
      local calls,verdict={},'accept'
      GcUiDealQuery=function(seat,ai,items,op,mode)
        calls[#calls+1]={seat=seat,ai=ai,op=op,n=#items,mode=mode}
        assert(seat==0 and ai==2)
        for _,it in ipairs(items) do assert(it.from==0 or it.from==2) end
        if op=='check' then local out={};for i=1,#items do out[i]=true end;return out end
        if op=='pockets' then return {{from=0,type=0,data1=0,data2=0,max=100},{from=0,type=3,data1=5,data2=0,max=3},
          {from=2,type=3,data1=2,data2=0,max=1},{from=2,type=6,data1=0,data2=0,max=1}} end
        if op=='evaluate' then return {response=verdict,value=40,mine=10,theirs=50,peace=false,cant_match=false} end
        if op=='equalize' then return {ok=true,good=false,cant_match=false,items={{type=3,from=2,data1=2,data2=1,data3=0},{type=0,from=0,data1=60,data2=0,data3=0},{type=4,from=0,data1=1,data2=1,data3=0}}} end
        if op=='blob' then return 'abcd' end
        error('unknown op ' .. op)
      end
      GcUiDiploText=function(seat,ai,name) return name end
      GcUiLeaderGreeting=function() return "Hello" end
      local sent={}
      local net={send=function(...) error('the AI table sends no plain message') end,send_deal=function(...) sent[#sent+1]={...} end}
      leader_begin(0,2,true)
      local root=screen.data(0,{a=2})
      assert(root.kind=='ai' and root.editable and root.question==nil and not root.right_button.enabled)
      local us,them={},{}
      for _,s in ipairs(root.pockets.us) do us[s.key]=s end
      for _,s in ipairs(root.pockets.them) do them[s.key]=s end
      assert(us.gold.amount==100 and us.strategic.items[1].amount==3 and us.luxury==nil)
      assert(them.luxury.items[1].ref.data1==2 and them.open_borders.amount==nil and them.gold==nil)
      screen.actions.add(0,{a=1,b=3,c=2,d=0},net)
      screen.actions.add_max(0,{a=0,b=3,c=5,d=0},net)
      assert(not pcall(screen.actions.add,0,{a=1,b=0,c=0},net), "the AI's gold is not in its pockets")
      root=screen.data(0,{a=2})
      assert(#root.table.them==1 and root.table.them[1].amount==1 and not root.table.them[1].editable)
      assert(#root.table.us==1 and root.table.us[1].amount==3 and root.table.us[1].editable and root.table.us[1].amount_max==3)
      assert(root.question.key=='equalize' and root.right_button.enabled)
      -- Refused: the leader's line, the table stays, nothing is sent.
      verdict='unacceptable'
      screen.actions.propose(0,{},net)
      root=screen.data(0,{a=2})
      assert(root.state=='ai_reply_no' and root.phrase=='TRADE_REJECT_UNACCEPTABLE' and #sent==0 and #root.table.us==1)
      screen.actions.propose(0,{},net)
      assert(screen.data(0,{a=2}).phrase=='REPEAT_TRADE')
      -- What would make it work: the AI's table replaces the draft (an item this table cannot show is left out).
      screen.actions.ask(0,{},net)
      root=screen.data(0,{a=2})
      assert(root.state=='ai_offer' and root.phrase=='TRADE_AI_MAKES_OFFER')
      assert(#root.table.us==1 and root.table.us[1].label=='Gold' and root.table.us[1].amount==60 and #root.table.them==1)
      -- Accepted: the core's blob goes out as the seat's sendNetDealAccepted, and the seat is back at the leader.
      verdict='accept'
      local r=screen.actions.propose(0,{},net)
      assert(#sent==1 and sent[1][1]=='sendNetDealAccepted' and sent[1][2]=='abcd' and sent[1][3]==2 and sent[1][4]==40 and sent[1][5]==10 and sent[1][6]==50)
      assert(r.close and r.open.screen=='leader' and r.open.a==2)
      assert(#screen.data(0,{a=2}).table.us==0, "the draft is gone")
      for _,c in ipairs(calls) do assert(c.seat==0) end
      assert(screen.actions.close(0,{},net).open.screen=='leader')
    "#,"=deal-ai").unwrap();
}
