use lua_sys::{Lua, Table};

#[test]
fn ideology_choice_checks_era_current_branch_and_seat() {
    let lua = Lua::new().unwrap();
    let screen: Table = lua
        .load(include_str!("../screens/ideology.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua.exec(r#"
        local p = {}
        current, era, cities = -1, 0, 1
        function p:GetNumCities() return cities end
        function p:GetCurrentEra() return era end
        function p:GetLateGamePolicyTree() return current end
        Players = {[3] = p}
        Game = {IsOption = function() return false end}
        GameOptionTypes = {GAMEOPTION_NO_POLICIES = 1}
        GameInfo = {PolicyBranchTypes = {[9] = {ID = 9, PurchaseByLevel = true}}, Eras = {ERA_INDUSTRIAL = {ID=4}},
          Buildings = function() return function() end end}
        local sent = 0
        local net = {send = function(name, id) assert(name == 'IdeologyChoice' and id == 9); sent = sent + 1 end}
        assert(not pcall(screen.actions.choose, 3, {a=9}, net))
        era = 5
        screen.actions.choose(3, {a=9}, net)
        assert(sent == 1)
        current = 9
        assert(not pcall(screen.actions.choose, 3, {a=9}, net))
        assert(not pcall(screen.actions.choose, 4, {a=9}, net))
        assert(not pcall(screen.actions.choose, 3, {a=-1}, net))
        assert(sent == 1)
    "#, "=ideology-actions").unwrap();
}
