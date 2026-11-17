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
      local can=true
      local unit={GetOwner=function() return 0 end,CanMove=function() return true end,CanRangeStrikeAt=function(_,x,y) return x==4 and y==5 end}
      Players={[0]={GetTeam=function() return 0 end,IsTurnActive=function() return true end,GetUnitByID=function(_,id) if id==9 then return unit end end},[1]={IsAlive=function() return true end}}
      Teams={[0]={IsHasMet=function(_,t) return t==1 end,CanDeclareWar=function() return can end},[1]={GetLeaderID=function() return 1 end}}
      MissionTypes={MISSION_MOVE_TO=0,MISSION_RANGE_ATTACK=1}
      Map={GetPlot=function(x,y) return {IsVisible=function() return true end,GetX=function() return x end,GetY=function() return y end} end}
      local sent,done={},nil
      local net={send=function(...) sent[#sent+1]={...} end,popup_done=function(id) done=id end}
      assert(screen.actions.confirm(0,{a=1,b=-1},net).close)
      assert(#sent==1 and sent[1][1]=='sendChangeWar' and sent[1][2]==1 and sent[1][3]==1)
      local args={a=1,b=9,c=4,d=5,popup_id=7,popup={kind='BUTTONPOPUP_DECLAREWARRANGESTRIKE',data1=1,data2=4,data3=5}}
      screen.actions.confirm(0,args,net)
      assert(#sent==3 and sent[3][1]=='sendPushMission' and sent[3][2]==9 and sent[3][3]==1 and sent[3][4]==4 and sent[3][5]==5 and done==7)
      args.c=6
      assert(not pcall(screen.actions.confirm,0,args,net) and #sent==3)
      args.c=4;args.b=10
      assert(not pcall(screen.actions.confirm,0,args,net) and #sent==3)
      args.b=9;args.a=2
      assert(not pcall(screen.actions.confirm,0,args,net) and #sent==3)
      can=false
      assert(not pcall(screen.actions.confirm,0,{a=1},net) and #sent==3)
      assert(not pcall(screen.actions.plunder,0,{},net) and #sent==3)
      assert(screen.actions.cancel(0,{popup_id=8},net).close and done==8 and #sent==3)
    "#,"=war-guards").unwrap();
}
