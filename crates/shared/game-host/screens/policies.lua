local function enabled()
    return not Game.IsOption(GameOptionTypes.GAMEOPTION_NO_POLICIES)
end
local function yes(value) return value == true or value == 1 end

local function effect(key)
    local policy = key and GameInfo.Policies[key]
    return policy and L(policy.Help) or ""
end

local function branch_reason(p, branch)
    if not enabled() then return "Social policies are disabled" end
    if p:GetNumCities() == 0 then return "Found a city first" end
    local era = branch.EraPrereq and GameInfo.Eras[branch.EraPrereq]
    if era and p:GetCurrentEra() < era.ID then return "Requires " .. L(era.Description) end
    if yes(branch.LockedWithoutReligion) and Game.IsOption(GameOptionTypes.GAMEOPTION_NO_RELIGION) then return "Religion is disabled" end
    if p:IsPolicyBranchUnlocked(branch.ID) then
        if p:IsPolicyBranchBlocked(branch.ID) then return nil end
        return "Already opened"
    end
    if not p:CanUnlockPolicyBranch(branch.ID) then return "Branch already opened or insufficient culture" end
end

local function policy_reason(p, policy)
    if not enabled() then return "Social policies are disabled" end
    if p:GetNumCities() == 0 then return "Found a city first" end
    if p:HasPolicy(policy.ID) then return "Already adopted" end
    if p:IsPolicyBlocked(policy.ID) then return "Policy branch is blocked" end
    -- SIMPLIFIED(SIMP-363): native boolean availability has no structured lock explanation; use a general prerequisite reason.
    if not p:CanAdoptPolicy(policy.ID) then return "Requires culture, an open branch and prerequisite policies" end
end

local function data(player, args)
    local p = Players[player]
    local stored, rate, cost = p:GetJONSCulture(), p:GetTotalJONSCulturePerTurn(), p:GetNextPolicyCost()
    local turns
    if stored >= cost then turns = 0 elseif rate > 0 then turns = math.ceil((cost - stored) / rate) end
    local branches, must = array(), false
    for b in GameInfo.PolicyBranchTypes() do
        if not yes(b.PurchaseByLevel) then
            local reason = branch_reason(p, b)
            local state = "closed"
            if p:IsPolicyBranchUnlocked(b.ID) then state = "opened" end
            if p:IsPolicyBranchFinished(b.ID) then state = "complete" end
            local era = b.EraPrereq and GameInfo.Eras[b.EraPrereq]
            if era and p:GetCurrentEra() < era.ID then state = "era_locked" end
            if p:IsPolicyBranchBlocked(b.ID) then state = "blocked" end
            local policies, links, disables, adopted = array(), array(), array(), 0
            for policy in GameInfo.Policies{PolicyBranchType = b.Type} do
                local why = policy_reason(p, policy)
                local status = "locked"
                if not why then status = "available"; must = true end
                if p:IsPolicyBlocked(policy.ID) then status = "blocked" end
                if p:HasPolicy(policy.ID) then status = "adopted"; adopted = adopted + 1 end
                local requires = array()
                for link in GameInfo.Policy_PrereqPolicies{PolicyType = policy.Type} do
                    local prev = GameInfo.Policies[link.PrereqPolicy]
                    if prev then
                        requires[#requires + 1] = {id = prev.ID, name = L(prev.Description)}
                        links[#links + 1] = {key = prev.ID .. "-" .. policy.ID, from_x = prev.GridX, from_y = prev.GridY,
                            to_x = policy.GridX, to_y = policy.GridY, lit = p:HasPolicy(prev.ID) and p:HasPolicy(policy.ID)}
                    end
                end
                policies[#policies + 1] = {id = policy.ID, key = policy.Type, name = L(policy.Description),
                    icon = icon(policy.IconAtlas, policy.PortraitIndex), grid_x = policy.GridX, grid_y = policy.GridY,
                    state = status, effect = L(policy.Help), requires = requires, reason = why}
            end
            for link in GameInfo.PolicyBranch_Disables{PolicyBranchType = b.Type} do
                local other = GameInfo.PolicyBranchTypes[link.PolicyBranchDisable]
                if other and p:IsPolicyBranchUnlocked(other.ID) then
                    disables[#disables + 1] = {id = other.ID, name = L(other.Description)}
                end
            end
            if not reason and not p:IsPolicyBranchUnlocked(b.ID) then must = true end
            branches[#branches + 1] = {id = b.ID, key = b.Type, name = L(b.Description), icon = icon(b.IconAtlas, b.PortraitIndex),
                state = state, adopted = adopted, total = #policies, opener = effect(b.FreePolicy), finisher = effect(b.FreeFinishingPolicy),
                finisher_done = p:IsPolicyBranchFinished(b.ID), era = era and L(era.Description) or "", can_unlock = reason == nil,
                unlock_cost = p:IsPolicyBranchUnlocked(b.ID) and 0 or cost, disables = disables, reason = reason, tooltip = reason or L(b.Help), policies = policies, links = links}
        end
    end
    local ideology
    local id = p:GetLateGamePolicyTree()
    local b = GameInfo.PolicyBranchTypes[id]
    if b then
        local levels, counts, total = array(), array(), 0
        for level = 1, 3 do
            local owned, available = {}, array()
            for policy in GameInfo.Policies{PolicyBranchType = b.Type} do
                if policy.Level == level then
                    if p:HasPolicy(policy.ID) then owned[#owned + 1] = policy
                    end
                end
            end
            local can_adopt = false
            for _, tenet in ipairs(p:GetAvailableTenets(level)) do
                local policy = GameInfo.Policies[tenet]
                if policy then
                    available[#available + 1] = {id = tenet, key = policy.Type, name = L(policy.Description), effect = L(policy.Help)}
                    if not policy_reason(p, policy) then can_adopt = true end
                end
            end
            local slots, size = array(), ({7, 4, 3})[level]
            for slot = 1, size do
                local policy = owned[slot]
                local state, reason = "empty", "Earlier slots must be filled first"
                if slot == #owned + 1 then
                    state, reason = "locked", "Requires more tenets at the previous level"
                    if level == 1 or counts[level - 1].count > #owned + 1 then
                        state, reason = "unaffordable", "Insufficient culture or no eligible tenets"
                        if can_adopt then state, reason = "open", nil end
                    end
                end
                if policy then state, reason = "adopted", "Already adopted" end
                slots[#slots + 1] = {key = level .. "-" .. slot, state = state, reason = reason,
                    tenet_id = policy and policy.ID, name = policy and L(policy.Description), effect = policy and L(policy.Help)}
            end
            counts[#counts + 1] = {level = level, count = #owned}
            levels[#levels + 1] = {level = level, adopted = #owned, slot_count = size, slots = slots, available = available}
            total = total + #owned
        end
        local unhappy = p:GetPublicOpinionUnhappiness()
        local preferred = GameInfo.PolicyBranchTypes[p:GetPublicOpinionPreferredIdeology()]
        local opinion = ({[0] = "content", "dissidents", "civil_resistance", "revolutionary_wave"})[p:GetPublicOpinionType()] or "content"
        -- SIMPLIFIED(SIMP-360): omit unverified pressure totals/date and replace cached foreign-civ tooltip details with a seat-safe summary.
        local label = ({content = "Content", dissidents = "Dissidents", civil_resistance = "Civil resistance", revolutionary_wave = "Revolutionary wave"})[opinion]
        local civ = GameInfo.Civilizations[p:GetCivilizationType()]
        ideology = {id = id, key = b.Type, name = L(b.Description), title = L(civ.Adjective) .. " " .. L(b.Description),
            icon = icon(b.IconAtlas, b.PortraitIndex), emblem = icon(civ.IconAtlas, civ.PortraitIndex), adopted_date = "", summary = L(b.Help),
            adopted_by_level = counts, levels = levels,
            public_opinion = {state = opinion, label = label, unhappiness = unhappy, tooltip = label .. "; unhappiness: " .. unhappy,
                unhappiness_tooltip = p:GetPublicOpinionUnhappinessTooltip(), preferred = preferred and L(preferred.Description)},
            switch = {available = unhappy > 0, reason = unhappy <= 0 and "Public opinion causes no unhappiness" or nil,
                target = preferred and L(preferred.Description), target_id = preferred and preferred.ID,
                anarchy_turns = GameDefines.SWITCH_POLICY_BRANCHES_ANARCHY_TURNS or 2, tenets_now = total,
                tenets_kept = math.max(0, total - (GameDefines.SWITCH_POLICY_BRANCHES_TENETS_LOST or 2)), unhappiness_removed = unhappy}}
    end
    local dominant = GameInfo.PolicyBranchTypes[p:GetDominantPolicyBranchForTitle()]
    local civ = GameInfo.Civilizations[p:GetCivilizationType()]
    local title = dominant and dominant.Title and (L(dominant.Title) .. " of " .. L(civ.ShortDescription)) or nil
    return {open_tab = (args.a == 1 or p:GetNumFreeTenets() > 0) and "ideology" or "policies", policies_enabled = enabled(), player_title = title,
        ideologies_enabled = enabled(), free_policies = p:GetNumFreePolicies(), free_tenets = p:GetNumFreeTenets(), must_choose = must or p:GetNumFreeTenets() > 0,
        culture = {stored = stored, per_turn = rate, next_cost = cost, turns = turns, can_afford = stored >= cost}, branches = branches, ideology = ideology}
end

local function adopt(player, args, net, tenet)
    local policy = GameInfo.Policies[args.a]
    assert(policy, "Unknown policy")
    assert((policy.Level or 0) > 0 == tenet, "Wrong policy category")
    local reason = policy_reason(Players[player], policy)
    assert(not reason, reason)
    net.send("sendUpdatePolicies", 1, args.a, 1)
end

return {data = data, actions = {
    unlock_branch = function(player, args, net)
        local branch = GameInfo.PolicyBranchTypes[args.a]
        assert(branch and not yes(branch.PurchaseByLevel), "Unknown social policy branch")
        local reason = branch_reason(Players[player], branch)
        assert(not reason, reason)
        net.send("sendUpdatePolicies", 0, args.a, 1)
    end,
    adopt_policy = function(player, args, net) adopt(player, args, net, false) end,
    adopt_tenet = function(player, args, net) adopt(player, args, net, true) end,
    change_ideology = function(player, args, net)
        assert(Players[player]:GetLateGamePolicyTree() >= 0, "No ideology to change")
        assert(Players[player]:GetPublicOpinionUnhappiness() > 0, "Public opinion causes no unhappiness")
        net.send("sendChangeIdeology")
    end,
}}
