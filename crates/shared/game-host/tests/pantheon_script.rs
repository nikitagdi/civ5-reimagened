use lua_sys::{Lua, Table};

#[test]
fn pantheon_and_reformation_recheck_the_live_belief_list() {
    let lua = Lua::new().unwrap();
    let screen: Table = lua
        .load(include_str!("../screens/pantheon.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua.exec(r#"
        local p = {}
        ready, taken = true, false
        function p:GetNumCities() return 1 end
        function p:CanCreatePantheon(faith) assert(faith); return ready end
        function p:CanChooseReformationBelief() return ready end
        Players = {[3] = p}
        GameOptionTypes = {GAMEOPTION_NO_RELIGION = 1}
        GameInfo = {Beliefs = {[1]={ID=1}, [2]={ID=2,Reformation=true}}}
        Game = {IsOption = function() return false end,
          GetAvailablePantheonBeliefs = function() return taken and {} or {1} end,
          GetAvailableReformationBeliefs = function() return taken and {} or {2} end}
        local sent = 0
        local net = {send = function(name,id) assert(name == 'SendFoundPantheon' and (id == 1 or id == 2)); sent = sent + 1 end}
        for id = 1,2 do
          ready, taken = true, false
          screen.actions.choose(3,{a=id},net)
          taken = true
          assert(not pcall(screen.actions.choose,3,{a=id},net))
          ready, taken = false, false
          assert(not pcall(screen.actions.choose,3,{a=id},net))
        end
        assert(sent == 2)
    "#, "=pantheon-actions").unwrap();
}
