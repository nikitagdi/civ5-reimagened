use lua_sys::{Lua, Table};

#[test]
fn overview_filters_contacts_uses_public_mood_and_refuses_unsupported_conversations() {
    let lua = Lua::new().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    let screen: Table = lua
        .load(include_str!("../screens/diplo.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua.exec(r#"
      function L(s) return s end
      local function catalog(rows)
        return setmetatable(rows, {__call=function(t) local i=0; return function() i=i+1; return t[i] end end})
      end
      GameDefines={MAX_CIV_PLAYERS=5,MAX_MAJOR_CIVS=4,DOF_EXPIRATION_TIME=50,FRIENDSHIP_THRESHOLD_FRIENDS=30,FRIENDSHIP_THRESHOLD_ALLIES=60}
      GameInfoTypes={MAJOR_CIV_APPROACH_FRIENDLY=5}
      GameInfo={Civilizations={[0]={Type='CIVILIZATION_TEST',ShortDescription='Civilization'}},Leaders={[0]={Type='LEADER_TEST',Description='Leader'}},Eras={[0]={Description='Ancient'}},Resources=catalog({}),PolicyBranchTypes=catalog({}),Buildings=catalog({})}
      Game={GetGameTurn=function() return 12 end,GetActiveLeague=function() return nil end}
      local contacts=true
      Teams={}
      for id=0,4 do
        Teams[id]={IsHasMet=function(_,other) return contacts and other~=3 end,IsAtWar=function(_,other) return other==2 end,GetNumMembers=function() return 1 end,GetNumTurnsAtWar=function() return 7 end,HasEmbassyAtTeam=function() return false end,IsAllowsOpenBordersToTeam=function() return false end,IsDefensivePact=function() return false end,CanDeclareWar=function(_,other) return other==1 end,IsHasResearchAgreement=function() return false end}
      end
      Players={}
      for id=0,4 do
        local pid=id
        Players[id]={GetID=function() return pid end,GetTeam=function() return pid end,GetCivilizationShortDescriptionKey=function() return "Civilization" end,GetCivilizationType=function() return 0 end,GetLeaderType=function() return 0 end,GetScore=function() return 100-pid end,IsTurnActive=function() return true end,IsAlive=function() return pid~=4 end,IsMinorCiv=function() return false end,IsHuman=function() return pid==1 end,IsDenouncedPlayer=function() return false end,IsDoF=function() return false end,GetCurrentEra=function() return 0 end,GetGold=function() return 40 end,CalculateGoldRate=function() return -2 end,Cities=function() return function() end end,GetApproachTowardsUsGuess=function(_,other) assert(pid==0 and other==2); return 5 end,GetNickName=function() error('account name must never be read') end,GetMajorCivApproach=function() error('hidden approach must never be read') end}
      end
      local root=screen.data(0,{})
      assert(root.me.id==0 and #root.majors==2 and #root.city_states==0)
      assert(root.majors[1].id==1 and root.majors[1].mood=='human')
      assert(root.majors[2].id==2 and root.majors[2].mood=='friendly')
      assert(root.majors[2].war_turns==7 and root.majors[2].gold==nil)
      assert(root.counts.at_war==1 and root.counts.majors_met==2)
      assert(not root.deals_available and #root.deals_reason>0 and #root.deals==0)
      assert(root.majors[1].action_enabled and root.majors[1].can_declare_war)
      assert(not root.majors[2].can_declare_war)
      assert(not root.majors[2].action_enabled and #root.majors[2].action_reason>0)
      local ok,reason=pcall(screen.actions.begin_diplo,0,{a=2},{send=function() error('must not send') end})
      assert(not ok and string.find(reason,'seat%-scoped'))
      assert(not pcall(screen.actions.begin_diplo,0,{a=1},{}))
      assert(not pcall(screen.actions.begin_diplo,0,{a=3},{}))
      contacts=false
      assert(#screen.data(0,{}).majors==0)
      contacts=true
      MinorCivPersonalityTypes={MINOR_CIV_PERSONALITY_FRIENDLY=0,MINOR_CIV_PERSONALITY_NEUTRAL=1,MINOR_CIV_PERSONALITY_HOSTILE=2,MINOR_CIV_PERSONALITY_IRRATIONAL=3}
      GameInfo.MinorCivilizations={[0]={Type='MINOR_CIV_TEST'}}
      GameInfo.MinorCivTraits={[0]={Description='Maritime'}}
      local cs=Players[4]
      cs.IsAlive=function() return true end
      cs.IsMinorCiv=function() return true end
      cs.GetMinorCivFriendshipWithMajor=function(_,seat) assert(seat==0);return -10 end
      cs.GetAlly=function() return 3 end
      cs.IsAllies=function() return false end
      cs.IsFriends=function() return false end
      cs.GetMinorCivType=function() return 0 end
      cs.GetMinorCivTrait=function() return 0 end
      cs.GetPersonality=function() return 0 end
      cs.GetCapitalCity=function() return nil end
      local minors=screen.data(0,{}).city_states
      assert(#minors==1 and minors[1].status=='angry' and minors[1].personality=='Friendly')
      assert(minors[1].ally_name==nil and minors[1].ally_id==nil)

    "#, "=overview").unwrap();
}
