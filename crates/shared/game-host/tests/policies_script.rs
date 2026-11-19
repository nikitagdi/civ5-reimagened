use lua_sys::{Lua, Table};

#[test]
fn policy_branch_art_follows_the_branch_type() {
    let lua = Lua::new().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    let screen: Table = lua
        .load(include_str!("../screens/policies.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua.exec(r#"
        local function rows(list)
            return function()
                local i = 0
                return function() i = i + 1; return list[i] end
            end
        end
        local function zero() return 0 end
        local function no() return false end
        local function none() return -1 end
        Locale = {ConvertTextKey=function(key) return key end}
        Game = {IsOption=no}
        GameOptionTypes = {GAMEOPTION_NO_POLICIES=1}
        GameInfo = {
            PolicyBranchTypes=setmetatable({}, {__call=rows({
                {ID=26, Type="POLICY_BRANCH_EXPLORATION", Description="Exploration"},
                {ID=2, Type="POLICY_BRANCH_TRADITION", Description="Tradition"},
                {ID=0, Type="POLICY_BRANCH_CUSTOM", Description="Custom"},
            })}),
            Policies=rows({}), PolicyBranch_Disables=rows({}), Civilizations={},
        }
        Players = {[3]={
            GetJONSCulture=zero, GetTotalJONSCulturePerTurn=zero, GetNextPolicyCost=zero,
            GetNumCities=zero, GetNumFreePolicies=zero, GetNumFreeTenets=zero, GetCivilizationType=zero,
            IsPolicyBranchUnlocked=no, IsPolicyBranchFinished=no, IsPolicyBranchBlocked=no,
            GetLateGamePolicyTree=none, GetDominantPolicyBranchForTitle=none,
        }}
        local branches = screen.data(3, {}).branches
        assert(branches[1].background == "ui:tx:PolicyBranch_Exploration.dds")
        assert(branches[2].background == "ui:tx:SocialPoliciesTradition.dds")
        assert(branches[3].background == nil)
    "#, "=branch-art").unwrap();
}

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

#[test]
fn a_locked_policy_names_the_first_failing_core_check() {
    let lua = Lua::new().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    let screen: Table = lua
        .load(include_str!("../screens/policies.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua.exec(r#"
        function L(s) return s end
        local function rows(list, by)
            local t = {}
            for _, r in ipairs(list) do if r.ID then t[r.ID] = r end; if r.Type then t[r.Type] = r end end
            return setmetatable(t, {__call = function(_, filter)
                local out = {}
                for _, r in ipairs(list) do
                    local ok = true
                    for k, v in pairs(filter or {}) do ok = ok and r[k] == v end
                    if ok then out[#out + 1] = r end
                end
                local i = 0
                return function() i = i + 1; return out[i] end
            end})
        end
        local seat = 0
        local owned, unlocked, culture, has_tech = {}, false, 0, false
        local p = {}
        function p:GetID() return seat end
        function p:GetTeam() return 0 end
        function p:GetNumCities() return 1 end
        function p:HasPolicy(id) return owned[id] == true end
        function p:IsPolicyBlocked() return false end
        function p:CanAdoptPolicy() return false end
        function p:GetJONSCulture() return culture end
        function p:GetNextPolicyCost() return 25 end
        function p:GetNumFreePolicies() return 0 end
        function p:GetNumFreeTenets() return 0 end
        function p:IsPolicyBranchUnlocked() return unlocked end
        Players = {[seat] = p}
        Teams = {[0] = {IsHasMet = function() return true end, IsHasTech = function() return has_tech end}}
        Game = {IsOption = function() return false end}
        GameOptionTypes = {GAMEOPTION_NO_POLICIES = 1}
        GameInfo = {
            PolicyBranchTypes = rows({{ID = 1, Type = 'BRANCH_A', Description = 'Branch A'}}),
            Policies = rows({{ID = 10, Type = 'P_BASE', Description = 'Base', PolicyBranchType = 'BRANCH_A', Level = 0},
                {ID = 11, Type = 'P_OR', Description = 'Or', PolicyBranchType = 'BRANCH_A', Level = 0},
                {ID = 12, Type = 'P_NEXT', Description = 'Next', PolicyBranchType = 'BRANCH_A', Level = 0, TechPrereq = 'TECH_X'},
                {ID = 13, Type = 'P_RIVAL', Description = 'Rival', PolicyBranchType = 'BRANCH_A', Level = 0}}),
            Policy_PrereqORPolicies = rows({{PolicyType = 'P_NEXT', PrereqPolicy = 'P_OR'}}),
            Policy_PrereqPolicies = rows({{PolicyType = 'P_NEXT', PrereqPolicy = 'P_BASE'}}),
            Policy_Disables = rows({{PolicyType = 'P_RIVAL', PolicyDisable = 'P_NEXT'}}),
            Technologies = rows({{ID = 5, Type = 'TECH_X', Description = 'Tech X'}}),
        }
        -- The reason the adopt action refuses with (the same text the root shows).
        local net = {send = function() error('must not send') end}
        local function root_reason()
            local ok, reason = pcall(screen.actions.adopt_policy, seat, {a = 12}, net)
            assert(not ok)
            return (reason:gsub('^.-:%d+: ', ''))
        end
        assert(root_reason() == 'Requires 25 more culture', root_reason())
        culture = 30
        assert(root_reason() == 'Open the Branch A branch first', root_reason())
        unlocked = true
        assert(root_reason() == 'Requires one of: Or', root_reason())
        owned[11] = true
        assert(root_reason() == 'Requires Base', root_reason())
        owned[10] = true; owned[13] = true
        assert(root_reason() == 'Disabled by Rival', root_reason())
        owned[13] = nil
        assert(root_reason() == 'Requires Tech X', root_reason())
        has_tech = true
        -- Every core check passes and the core still refuses: a scripted game rule.
        assert(root_reason() == 'A game rule forbids adopting this policy now', root_reason())
    "#, "=policy-reasons").unwrap();
}
