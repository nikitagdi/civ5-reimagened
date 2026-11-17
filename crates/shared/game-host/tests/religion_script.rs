use lua_sys::{Lua, Table};

#[test]
fn religion_validates_packed_choices_city_prophet_and_text() {
    let lua = Lua::new().unwrap();
    let screen: Table = lua
        .load(include_str!("../screens/religion.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua.exec(r#"
        local p, c, u = {}, {}, {}
        mine, eligible, founded = true, true, false
        function p:GetCityByID(id) if id == 4 then return c end end
        function c:GetOwner() return mine and 3 or 4 end
        function c:GetX() return 10 end
        function c:GetY() return 20 end
        function u:GetX() return 10 end
        function u:GetY() return 20 end
        function u:CanStartMission() return eligible end
        function p:Units() local once = false; return function() if not once then once=true; return u end end end
        function p:HasCreatedReligion() return founded end
        function p:HasCreatedPantheon() return true end
        function p:GetBeliefInPantheon() return 0 end
        function p:IsTraitBonusReligiousBelief() return false end
        function p:GetReligionCreatedByPlayer() return 5 end
        Players = {[3]=p}
        GameInfoTypes = {MISSION_FOUND_RELIGION=1, MISSION_ENHANCE_RELIGION=2}
        GameOptionTypes = {GAMEOPTION_NO_RELIGION=1}
        GameInfo = {Religions = {[5]={Type='RELIGION_TEST'}}, Beliefs = {}}
        for i=1,4 do GameInfo.Beliefs[i]={ID=i,Type='belief'..i} end
        function L() return '' end
        function icon() return '' end
        function array() return {} end
        Game = {IsOption=function() return false end, GetFounder=function() return -1 end,
          GetNumReligionsStillToFound=function() return 1 end,
          GetAvailableFounderBeliefs=function() return {1} end, GetAvailableFollowerBeliefs=function() return {2,3} end,
          GetAvailableEnhancerBeliefs=function() return {4} end}
        local sent=0
        local last_text, done
        local net={send_text=function(name,text,a,b,c,d,e,f,g)
          assert(text == "" or text == "Custom name")
          last_text=text
          sent=sent+1
          if name=='SendFoundReligion' then assert(a==5 and b==1 and c==2 and d==-1 and e==-1 and f==10 and g==20)
          else assert(name=='SendEnhanceReligion' and a==5 and b==3 and c==4 and d==10 and e==20) end
        end}
        net.popup_done=function(id) done=id end
        local args={a=5,b=4,c=1+2*65536,d=-1,text='',popup_id=11}
        local result=screen.actions.found(3,args,net)
        assert(done==11 and result.close)
        args.text='Custom name'; screen.actions.found(3,args,net)
        assert(last_text=='Custom name')
        for _, text in ipairs({'bad<name','12345678901234567'}) do
          args.text=text
          local ok, why=pcall(screen.actions.found,3,args,net)
          assert(not ok and #why>0)
        end
        args.text=''
        mine=false; assert(not pcall(screen.actions.found,3,args,net)); mine=true
        eligible=false; assert(not pcall(screen.actions.found,3,args,net)); eligible=true
        args.c=1+1*65536; assert(not pcall(screen.actions.found,3,args,net))
        founded=true
        screen.actions.enhance(3,{a=5,b=4,c=3,d=4},net)
        assert(not pcall(screen.actions.enhance,3,{a=6,b=4,c=3,d=4},net))
        assert(sent==3)
    "#, "=religion-actions").unwrap();
}
