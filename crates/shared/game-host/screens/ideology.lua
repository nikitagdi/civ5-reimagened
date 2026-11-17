local function yes(value) return value == true or value == 1 end
local function reason(p)
    if Game.IsOption(GameOptionTypes.GAMEOPTION_NO_POLICIES) then return "Social policies are disabled" end
    if p:GetNumCities() == 0 then return "Found a city first" end
    if p:GetLateGamePolicyTree() >= 0 then return "An ideology is already adopted" end
    local industrial = GameInfo.Eras.ERA_INDUSTRIAL
    if industrial and p:GetCurrentEra() > industrial.ID then return nil end
    for b in GameInfo.Buildings() do
        local trigger = b.XBuiltTriggersIdeologyChoice or 0
        if trigger > 0 then
            local class = GameInfo.BuildingClasses[b.BuildingClass]
            local civ = GameInfo.Civilizations[p:GetCivilizationType()]
            local selected = class and class.DefaultBuilding
            if class then
                for override in GameInfo.Civilization_BuildingClassOverrides{CivilizationType = civ.Type, BuildingClassType = class.Type} do
                    selected = override.BuildingType
                end
            end
            if class and selected == b.Type and p:GetBuildingClassCount(class.ID) >= math.min(trigger, p:GetNumCities()) then return nil end
        end
    end
    return "Requires the modern era or the required ideology buildings"
end

local victories = {
    POLICY_BRANCH_AUTOCRACY = {"culture", "diplomacy", "domination"},
    POLICY_BRANCH_FREEDOM = {"culture", "diplomacy", "science"},
    POLICY_BRANCH_ORDER = {"culture", "domination", "science"},
}

return {
    data = function(player, args)
        local p = Players[player]
        local civ = GameInfo.Civilizations[p:GetCivilizationType()]
        local options = array()
        for b in GameInfo.PolicyBranchTypes() do
            if yes(b.PurchaseByLevel) then
                local adopters, levels, wins = array(), array(), array()
                for other = 0, GameDefines.MAX_MAJOR_CIVS - 1 do
                    local q = Players[other]
                    if q and q:IsAlive() and q:GetLateGamePolicyTree() == b.ID then
                        local known = other == player or met(player, other)
                        local c = known and GameInfo.Civilizations[q:GetCivilizationType()]
                        adopters[#adopters + 1] = {key = other, known = known, name = c and L(c.ShortDescription) or "Unmet player",
                            civ = c and icon(c.IconAtlas, c.PortraitIndex) or nil}
                    end
                end
                for level = 1, 3 do
                    local tenets = array()
                    for policy in GameInfo.Policies{PolicyBranchType = b.Type} do
                        if policy.Level == level then
                            tenets[#tenets + 1] = {id = policy.ID, key = policy.Type, name = L(policy.Description), effect = L(policy.Help)}
                        end
                    end
                    levels[#levels + 1] = {level = level, tenets = tenets}
                end
                for _, kind in ipairs(victories[b.Type] or {}) do wins[#wins + 1] = {kind = kind} end
                options[#options + 1] = {id = b.ID, key = b.Type, name = L(b.Description), icon = icon(b.IconAtlas, b.PortraitIndex),
                    free_tenets = Game.GetNumFreePolicies(b.ID), victories = wins, adopted_by = adopters, levels = levels}
            end
        end
        local why = reason(p)
        local current = p:GetLateGamePolicyTree()
        return {emblem = icon(civ.IconAtlas, civ.PortraitIndex), civ_adjective = L(civ.Adjective), civ_name = L(civ.ShortDescription),
            current = current >= 0 and current or nil, enabled = why == nil, reason = why, options = options}
    end,
    actions = {choose = function(player, args, net)
        local b = GameInfo.PolicyBranchTypes[args.a]
        assert(b and yes(b.PurchaseByLevel), "Unknown ideology")
        local why = reason(Players[player])
        assert(not why, why)
        net.send("IdeologyChoice", b.ID)
            if args.popup_id then net.popup_done(args.popup_id) end
            return {close = true, open = {screen = "policies", a = 1}}
    end},
}
