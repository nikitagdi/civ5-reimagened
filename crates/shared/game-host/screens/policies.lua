-- The game's help texts carry markup ([COLOR_*]...[ENDCOLOR], [NEWLINE], [ICON_*]); the screen shows plain text, and a
-- policy's help that starts with the policy's own name (the original's tooltip title line) drops that name.
local function plain(text)
    return (tostring(text or ""):gsub("%[NEWLINE%]", " "):gsub("%[[^%]]*%]", ""):gsub("%s+", " "):gsub("^ ", ""):gsub(" $", ""))
end
local function help(row)
    local text, name = plain(L(row.Help)), plain(L(row.Description))
    if name ~= "" and text:sub(1, #name) == name then text = (text:sub(#name + 1):gsub("^[%s:.,]+", "")) end
    return text
end
local function enabled()
    return not Game.IsOption(GameOptionTypes.GAMEOPTION_NO_POLICIES)
end
local function yes(value) return value == true or value == 1 end

local function effect(key)
    local policy = key and GameInfo.Policies[key]
    return policy and help(policy) or ""
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

local function names(rows)
    local out = {}
    for _, row in ipairs(rows) do out[#out + 1] = L(row.Description) end
    return table.concat(out, ", ")
end

-- The first check of CvPlayerPolicies::CanAdoptPolicy that fails, in the core's order, read through
-- the seat's own getters (the core writes no reason text for policies; the original screen also
-- derives its lock state from these getters).
local function adopt_refusal(p, policy)
    local branch = policy.PolicyBranchType and GameInfo.PolicyBranchTypes[policy.PolicyBranchType]
    local stored, cost = p:GetJONSCulture(), p:GetNextPolicyCost()
    local tenet = (policy.Level or 0) > 0
    if cost > 0 and stored < cost and p:GetNumFreePolicies() == 0 and (not tenet or p:GetNumFreeTenets() == 0) then
        return "Requires " .. (cost - stored) .. " more culture"
    end
    if not branch then return "Comes with its branch; it cannot be adopted on its own" end
    if not p:IsPolicyBranchUnlocked(branch.ID) then return "Open the " .. L(branch.Description) .. " branch first" end
    if yes(branch.PurchaseByLevel) and (policy.Level or 0) > 1 then
        local this, previous = 1, 0
        for other in GameInfo.Policies{PolicyBranchType = branch.Type} do
            if p:HasPolicy(other.ID) then
                if other.Level == policy.Level then this = this + 1 elseif other.Level == policy.Level - 1 then previous = previous + 1 end
            end
        end
        if this >= previous then return "Requires more tenets at the previous level" end
    end
    if GameInfo.Policy_PrereqORPolicies then
        local any, missing = false, {}
        for link in GameInfo.Policy_PrereqORPolicies{PolicyType = policy.Type} do
            local prev = GameInfo.Policies[link.PrereqPolicy]
            if prev then
                if p:HasPolicy(prev.ID) then any = true end
                missing[#missing + 1] = prev
            end
        end
        if #missing > 0 and not any then return "Requires one of: " .. names(missing) end
    end
    local missing = {}
    for link in GameInfo.Policy_PrereqPolicies{PolicyType = policy.Type} do
        local prev = GameInfo.Policies[link.PrereqPolicy]
        if prev and not p:HasPolicy(prev.ID) then missing[#missing + 1] = prev end
    end
    if #missing > 0 then return "Requires " .. names(missing) end
    for link in GameInfo.Policy_Disables{PolicyDisable = policy.Type} do
        local by = GameInfo.Policies[link.PolicyType]
        if by and p:HasPolicy(by.ID) then return "Disabled by " .. L(by.Description) end
    end
    local tech = policy.TechPrereq and GameInfo.Technologies[policy.TechPrereq]
    if tech and not team_of(p:GetID()):IsHasTech(tech.ID) then return "Requires " .. L(tech.Description) end
end

local function policy_reason(p, policy)
    if not enabled() then return "Social policies are disabled" end
    if p:GetNumCities() == 0 then return "Found a city first" end
    if p:HasPolicy(policy.ID) then return "Already adopted" end
    if p:IsPolicyBlocked(policy.ID) then return "Policy branch is blocked" end
    if not p:CanAdoptPolicy(policy.ID) then
        -- A refusal none of the core's own checks explains comes from a scripted rule (PlayerCanAdoptPolicy).
        return adopt_refusal(p, policy) or "A game rule forbids adopting this policy now"
    end
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
                    state = status, effect = help(policy), requires = requires, reason = why}
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
                unlock_cost = p:IsPolicyBranchUnlocked(b.ID) and 0 or cost, disables = disables, reason = reason, tooltip = reason or plain(L(b.Help)), policies = policies, links = links}
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
                    available[#available + 1] = {id = tenet, key = policy.Type, name = L(policy.Description), effect = help(policy)}
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
                    tenet_id = policy and policy.ID, name = policy and L(policy.Description), effect = policy and help(policy)}
            end
            counts[#counts + 1] = {level = level, count = #owned}
            levels[#levels + 1] = {level = level, adopted = #owned, slot_count = size, slots = slots, available = available}
            total = total + #owned
        end
        local unhappy = p:GetPublicOpinionUnhappiness()
        local preferred = GameInfo.PolicyBranchTypes[p:GetPublicOpinionPreferredIdeology()]
        local opinion = ({[0] = "content", "dissidents", "civil_resistance", "revolutionary_wave"})[p:GetPublicOpinionType()] or "content"
        -- As SocialPolicyPopup's ideology tab: the opinion's own text, the core's public opinion tooltip (the pressure each
        -- ideology puts on this civilization) and its unhappiness tooltip, the switch button's tooltips.
        local label = L("TXT_KEY_CO_PUBLIC_OPINION_" .. opinion:upper())
        local civ = GameInfo.Civilizations[p:GetCivilizationType()]
        ideology = {id = id, key = b.Type, name = L(b.Description), title = L("TXT_KEY_POLICYSCREEN_IDEOLOGY_TITLE", p:GetCivilizationAdjectiveKey(), b.Description),
            icon = icon(b.IconAtlas, b.PortraitIndex), emblem = icon(civ.IconAtlas, civ.PortraitIndex), adopted_date = "", summary = plain(L(b.Help)),
            adopted_by_level = counts, levels = levels,
            public_opinion = {state = opinion, label = label, unhappiness = unhappy, tooltip = plain(p:GetPublicOpinionTooltip()),
                unhappiness_tooltip = plain(p:GetPublicOpinionUnhappinessTooltip()), preferred = preferred and L(preferred.Description)},
            switch = {available = unhappy > 0, reason = unhappy <= 0 and L"TXT_KEY_POLICYSCREEN_CHANGE_IDEOLOGY_DISABLED_TT" or nil,
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
            if args.popup_id then net.popup_done(args.popup_id) end
            
end

return {data = data, actions = {
    unlock_branch = function(player, args, net)
        local branch = GameInfo.PolicyBranchTypes[args.a]
        assert(branch and not yes(branch.PurchaseByLevel), "Unknown social policy branch")
        local reason = branch_reason(Players[player], branch)
        assert(not reason, reason)
        net.send("sendUpdatePolicies", 0, args.a, 1)
            if args.popup_id then net.popup_done(args.popup_id) end
            
    end,
    adopt_policy = function(player, args, net) adopt(player, args, net, false) end,
    adopt_tenet = function(player, args, net) adopt(player, args, net, true) end,
    change_ideology = function(player, args, net)
        assert(Players[player]:GetLateGamePolicyTree() >= 0, "No ideology to change")
        assert(Players[player]:GetPublicOpinionUnhappiness() > 0, "Public opinion causes no unhappiness")
        net.send("sendChangeIdeology")
            if args.popup_id then net.popup_done(args.popup_id) end
            return {close = true, open = {screen = "policies", a = 1}}
    end,
}}
