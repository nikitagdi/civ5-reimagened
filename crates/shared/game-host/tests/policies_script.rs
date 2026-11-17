use lua_sys::{Lua, Table};

#[test]
fn policies_recheck_seat_availability_before_emitting_orders() {
    let lua = Lua::new().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    let screen: Table = lua
        .load(include_str!("../screens/policies.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua.exec(r#"
        local seat = 3
        local p = {}
        cities, affordable, blocked, owned = 1, true, false, false
        function p:GetNumCities() return cities end
        function p:HasPolicy() return owned end
        function p:IsPolicyBlocked() return blocked end
        function p:CanAdoptPolicy() return affordable end
        function p:GetCurrentEra() return 0 end
        function p:IsPolicyBranchBlocked() return blocked end
        function p:IsPolicyBranchUnlocked() return false end
        function p:CanUnlockPolicyBranch() return affordable end
        function p:GetLateGamePolicyTree() return 2 end
        function p:GetPublicOpinionUnhappiness() return affordable and 1 or 0 end
        Players = {[seat] = p}
        Game = {IsOption = function() return false end}
        GameOptionTypes = {GAMEOPTION_NO_POLICIES = 1}
        GameInfo = {Policies = {[10] = {ID = 10, Level = 0}, [11] = {ID = 11, Level = 1}},
            PolicyBranchTypes = {[2] = {ID = 2}}, Eras = {}}
        local sent = 0
        local net = {send = function(name, a, b, c)
            sent = sent + 1
            assert(name == 'sendUpdatePolicies' or name == 'sendChangeIdeology')
            if name == 'sendUpdatePolicies' then
                assert(c == 1 and ((a == 0 and b == 2) or (a == 1 and (b == 10 or b == 11))))
            end
        end}
        for _, test in ipairs({{'adopt_policy',10}, {'adopt_tenet',11}, {'unlock_branch',2}, {'change_ideology',0}}) do
            local action, id = test[1], test[2]
            affordable = true
            screen.actions[action](seat, {a=id}, net)
            local before = sent
            affordable = false
            local ok, reason = pcall(screen.actions[action], seat, {a=id}, net)
            assert(not ok and #reason > 0 and sent == before)
            assert(not pcall(screen.actions[action], 4, {a=id}, net))
        end
        affordable, cities = true, 0
        assert(not pcall(screen.actions.unlock_branch, seat, {a=2}, net))
        assert(not pcall(screen.actions.adopt_policy, seat, {a=10}, net))
        cities, blocked = 1, true
        assert(not pcall(screen.actions.adopt_policy, seat, {a=10}, net))
        blocked, owned = false, true
        assert(not pcall(screen.actions.adopt_policy, seat, {a=10}, net))
        assert(not pcall(screen.actions.adopt_tenet, seat, {a=10}, net))
    "#, "=policy-actions").unwrap();
}
