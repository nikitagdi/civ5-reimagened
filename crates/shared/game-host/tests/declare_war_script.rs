use lua_sys::{Lua, Table};

#[test]
fn war_confirmation_rechecks_team_unit_plot_and_stages_cancel() {
    let lua = Lua::new().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    let screen: Table = lua
        .load(include_str!("../screens/declare_war.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua.exec(r#"
      GameDefines={MAX_CIV_TEAMS=8,MAX_CIV_PLAYERS=8}
      Locale={ConvertTextKey=function(key) return key end}
      local can,moves=true,true
      local unit={GetOwner=function() return 0 end,CanMove=function() return moves end}
      local function civ(team)
          return {GetTeam=function() return team end,IsAlive=function() return true end,IsMinorCiv=function() return false end,IsHuman=function() return true end,
              GetCivilizationType=function() return team end,GetLeaderType=function() return team end,IsDoF=function() return false end,
              IsDenouncedPlayer=function() return false end,GetTradeRoutes=function() return {} end,GetTradeRoutesToYou=function() return {} end}
      end
      Players={[0]=civ(0),[1]=civ(1)}
      Players[0].IsTurnActive=function() return true end
      Players[0].GetUnitByID=function(_,id) if id==9 then return unit end end
      GameInfo={Civilizations={[0]={Type='CIVILIZATION_A',ShortDescription='A'},[1]={Type='CIVILIZATION_B',ShortDescription='B'}},
          Leaders={[0]={Type='LEADER_A',Description='Leader A'},[1]={Type='LEADER_B',Description='Leader B'}}}
      Teams={[0]={IsHasMet=function(_,t) return t==1 end,CanDeclareWar=function() return can end},[1]={GetLeaderID=function() return 1 end}}
      MissionTypes={MISSION_MOVE_TO=0,MISSION_RANGE_ATTACK=1}
      Map={GetPlot=function(x,y) return {IsVisible=function() return true end,GetX=function() return x end,GetY=function() return y end} end}
      local sent,done={},nil
      local net={send=function(...) sent[#sent+1]={...} end,popup_done=function(id) done=id end}
      for _,rival in ipairs({-1,0,2,99}) do
          local root=screen.data(0,{a=rival})
          assert(not root.enabled and type(root.disabled_reason)=='string' and #root.disabled_reason>0)
      end
      assert(screen.actions.confirm(0,{a=1,b=-1},net).close)
      assert(#sent==1 and sent[1][1]=='sendChangeWar' and sent[1][2]==1 and sent[1][3]==1)
      -- The core's move question names the acting unit (data4) and its mission flags (data5).
      local args={a=1,popup_id=7,popup={kind='BUTTONPOPUP_DECLAREWARMOVE',data1=1,data2=4,data3=5,data4=9,data5=0}}
      local root=screen.data(0,args)
      assert(root.enabled and root.reason=='move' and root.plot.x==4 and root.plot.y==5, tostring(root.disabled_reason))
      assert(screen.actions.confirm(0,args,net).close)
      assert(#sent==3 and sent[2][1]=='sendChangeWar' and sent[3][1]=='sendPushMission' and sent[3][2]==9 and sent[3][3]==0
          and sent[3][4]==4 and sent[3][5]==5 and sent[3][6]==0 and sent[3][7]==0 and done==7)
      -- A window that echoes another unit or plot is refused.
      args.b=10
      assert(not pcall(screen.actions.confirm,0,args,net) and #sent==3)
      args.b=9;args.c=6
      assert(not pcall(screen.actions.confirm,0,args,net) and #sent==3)
      args.c=nil;args.a=2
      assert(not pcall(screen.actions.confirm,0,args,net) and #sent==3)
      args.a=1
      -- No named unit, a unit without moves, or a question raised by the client: shown, but "yes" is unavailable.
      for _,change in ipairs({function() args.popup.data4=nil end,function() args.popup.data4=-1 end,function() args.popup.data4=9;moves=false end,
          function() moves=true;args.popup.kind='BUTTONPOPUP_DECLAREWARRANGESTRIKE' end}) do
          change()
          root=screen.data(0,args)
          assert(not root.enabled and #root.disabled_reason>0)
          assert(not pcall(screen.actions.confirm,0,args,net) and #sent==3)
      end
      can=false
      assert(not pcall(screen.actions.confirm,0,{a=1},net) and #sent==3)
      assert(not pcall(screen.actions.plunder,0,{},net) and #sent==3)
      assert(screen.actions.cancel(0,{popup_id=8},net).close and done==8 and #sent==3)
    "#,"=war-guards").unwrap();
}

#[test]
fn unit_war_popups_route_to_the_confirmation_with_the_native_team_and_plot() {
    let lua = Lua::new().unwrap();
    let mappings: Table = lua
        .load(include_str!("../screens/_popups.lua"))
        .eval()
        .unwrap();
    for kind in [
        "BUTTONPOPUP_DECLAREWARMOVE",
        "BUTTONPOPUP_DECLAREWARRANGESTRIKE",
        "BUTTONPOPUP_DECLAREWAR_PLUNDER_TRADE_ROUTE",
    ] {
        let route: Table = mappings.get(kind).unwrap();
        assert_eq!(route.get::<_, String>("screen").unwrap(), "declare_war");
        assert!(route.get::<_, bool>("decision").unwrap());
        let payload = lua.create_table();
        payload.set("data1", 2).unwrap();
        payload.set("data2", 4).unwrap();
        let a: lua_sys::Function = route.get("a").unwrap();
        assert_eq!(a.call::<_, i32>(payload).unwrap(), 2);
    }
}
