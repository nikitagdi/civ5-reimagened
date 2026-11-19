use lua_sys::{Lua, Table};
#[test]
fn leader_session_greets_discusses_and_sends_only_the_seats_own_events() {
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
      GameDefines={MAX_MAJOR_CIVS=4};GameInfoTypes={}
      GameInfo={Civilizations={[0]={Type='CIVILIZATION_TEST',ShortDescription='Civilization'}},Leaders={[0]={Type='LEADER_TEST',Description='Leader'}},PolicyBranchTypes={}}
      Game={GetGameTurn=function() return 5 end}
      FromUIDiploEventTypes={FROM_UI_DIPLO_EVENT_HUMAN_DISCUSSION_WORK_WITH_US=10,FROM_UI_DIPLO_EVENT_HUMAN_DISCUSSION_DONT_SETTLE=11,
        FROM_UI_DIPLO_EVENT_HUMAN_DISCUSSION_STOP_SPYING=12,FROM_UI_DIPLO_EVENT_HUMAN_DISCUSSION_STOP_SPREADING_RELIGION=13,
        FROM_UI_DIPLO_EVENT_HUMAN_DISCUSSION_STOP_DIGGING=14,FROM_UI_DIPLO_EVENT_COOP_WAR_OFFER=15,FROM_UI_DIPLO_EVENT_DENOUNCE=16}
      DiploUIStateTypes={DIPLO_UI_STATE_BLANK_DISCUSSION=12,DIPLO_UI_STATE_DISCUSS_HUMAN_INVOKED=16}
      local greetings={}
      GcUiLeaderGreeting=function(seat,ai,n) greetings[#greetings+1]={seat,ai,n}; return "Greetings",1 end
      GcUiDiploText=function(seat,ai,name) return name .. " for " .. seat end
      Players={};Teams={}
      for id=0,3 do local pid=id
        Players[id]={IsAlive=function() return true end,IsHuman=function() return pid==0 or pid==3 end,GetTeam=function() return pid end,IsTurnActive=function() return true end,GetCivilizationType=function() return 0 end,GetLeaderType=function() return 0 end,IsDenouncedPlayer=function() return false end,GetApproachTowardsUsGuess=function() return 0 end,GetCapitalCity=function() return nil end,GetLateGamePolicyTree=function() return -1 end,GetOpinionTable=function() return {'[COLOR_RED]Concern[ENDCOLOR]'} end,IsDoF=function() return false end,GetNumCities=function() return 0 end,GetScore=function() return 100 end,IsAskedToStopConverting=function() return false end,IsAskedToStopDigging=function() return true end}
        Teams[id]={IsAtWar=function() return false end,IsHasMet=function() return true end,CanDeclareWar=function() return true end,IsAllowsOpenBordersToTeam=function() return false end}
      end
      local sent={}
      local net={send=function(...) sent[#sent+1]={...} end}
      local function act(name,args) return screen.actions[name](0,args or {},net) end

      -- No contacted AI: nothing to talk to.
      assert(not screen.data(0,{a=0}).available)
      -- A window opened before Talk starts the session with the greeting, without counting a repeat.
      local root=screen.data(0,{a=1})
      assert(root.available and root.mode=='main' and root.phrase=='Greetings' and greetings[1][3]==0)
      assert(root.mood.reasons[1].text=='Concern' and root.mood.reasons[1].value==nil)
      local keys={}
      for _,a in ipairs(root.main_actions) do keys[a.key]=a end
      assert(keys.discuss.enabled and keys.trade.enabled and keys.goodbye.enabled and keys.declare_war.enabled)
      assert(not keys.demand.enabled and #keys.demand.reason>0)
      assert(screen.data(0,{a=1}).phrase=='Greetings' and #greetings==1, "a refresh keeps the session")
      assert(leader_session(3)==nil, "another seat has no session")

      act('discuss')
      root=screen.data(0,{a=1})
      assert(root.mode=='discussion' and root.phrase=='LETS_HEAR_IT for 0' and #sent==0)
      local topic={}
      for _,r in ipairs(root.responses) do topic[r.key]=r end
      assert(topic.friendship.index==1 and topic.dont_settle.index==3 and topic.never_mind.index==8)
      assert(topic.coop_war and topic.stop_converting and not topic.stop_digging)
      act('topic',{a=3})
      assert(sent[1][1]=='sendFromUIDiploEvent' and sent[1][2]==1 and sent[1][3]==11 and sent[1][4]==0)
      assert(screen.data(0,{a=1}).phrase=='')
      -- The AI's reply arrives as the core's leader message for this seat and this leader only.
      __leader_message(3,1,16,'Not you',false,4)
      __leader_message(0,2,16,'Not this leader',false,4)
      __leader_message(0,1,16,'[COLOR_NEGATIVE_TEXT]Very well.[ENDCOLOR]',false,4)
      root=screen.data(0,{a=1})
      assert(root.mode=='discussion' and root.phrase=='Very well.')
      assert(not pcall(act,'topic',{a=6}), "a hidden topic is refused")

      act('topic',{a=7})
      root=screen.data(0,{a=1})
      assert(root.mode=='coop_target' and #root.targets==2 and root.targets[1].player==2 and root.targets[2].player==3)
      assert(not pcall(act,'coop_war',{a=1}) and not pcall(act,'coop_war',{a=0}))
      act('coop_war',{a=2})
      assert(sent[2][3]==15 and sent[2][4]==2 and screen.data(0,{a=1}).mode=='discussion')

      act('topic',{a=2})
      root=screen.data(0,{a=1})
      assert(root.mode=='confirm_denounce' and root.confirm.kind=='denounce')
      act('confirm',{a=0})
      assert(screen.data(0,{a=1}).mode=='discussion' and #sent==2)
      act('topic',{a=2});act('confirm',{a=1})
      assert(sent[3][3]==16 and screen.data(0,{a=1}).mode=='main')
      __leader_message(0,1,12,'So be it.',false,4)
      assert(screen.data(0,{a=1}).phrase=='So be it.')

      act('discuss');act('topic',{a=8})
      assert(screen.data(0,{a=1}).mode=='main' and #sent==3)
      local trade=act('trade')
      assert(trade.close and trade.open.screen=='deal' and trade.open.a==1)
      assert(leader_session(0,1).trade_phrase=='LETS_HEAR_IT for 0')
      for _,action in ipairs({'demand','make_peace','respond','switch'}) do
        local ok,why=pcall(act,action,{a=1});assert(not ok and #why>0)
      end
      assert(act('declare_war').open.a==1)
      assert(not pcall(screen.actions.goodbye,0,{popup={}},net))
      assert(not pcall(screen.actions.declare_war,0,{popup={}},net))
      assert(not screen.data(0,{a=1,popup={}}).can_close)
      assert(act('goodbye').close and leader_session(0)==nil)
      for _,action in ipairs({'discuss','trade','topic','coop_war','confirm','back','declare_war'}) do
        assert(not pcall(act,action,{a=1}), action .. " needs a session")
      end
      assert(#sent==3)
    "#,"=leader-session").unwrap();
}
