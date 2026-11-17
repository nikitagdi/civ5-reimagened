local function own_city(player, id)
    if id == nil then return nil end
    local city = Players[player]:GetCityByID(id)
    assert(city and city:GetOwner() == player, "city is unavailable")
    return city
end

local function own_unit(player, id)
    if id == nil then return nil end
    local unit = Players[player]:GetUnitByID(id)
    assert(unit and unit:GetOwner() == player, "unit is unavailable")
    return unit
end

local function turns(value)
    if value >= 0 and value < 32767 then return value end
end

local function city_data(city)
    if not city then return nil end
    local queue, reasons = array(), array()
    local getters = {
        [0] = {"GetUnitProductionTurnsLeft", "GetUnitProductionNeeded"},
        [1] = {"GetBuildingProductionTurnsLeft", "GetBuildingProductionNeeded"},
        [2] = {"GetProjectProductionTurnsLeft", "GetProjectProductionNeeded"},
        [3] = {"GetSpecialistProductionTurnsLeft"},
    }
    for slot = 0, city:GetOrderQueueLength() - 1 do
        local order, item = city:GetOrderFromQueue(slot)
        local g = getters[order]
        queue[#queue + 1] = {
            slot = slot, order = order, item = item,
            turns = g and turns(city[g[1]](city, item, slot)),
            needed = g and g[2] and city[g[2]](city, item) or 0,
        }
    end
    for _, kind in ipairs({
        {0, "Units", "CanTrain"}, {1, "Buildings", "CanConstruct"},
        {2, "Projects", "CanCreate"}, {4, "Processes", "CanMaintain"},
    }) do
        for item in GameInfo[kind[2]]() do
            if not city[kind[3]](city, item.ID, 0, 0) then
                -- SIMPLIFIED(SIMP-324): Lua availability getters omit native tooltip reasons, so locked options use a general explanation.
                reasons[#reasons + 1] = {
                    id = kind[1] * 65536 + item.ID, order = kind[1], item = item.ID,
                    reason = "Production requirements are not met in this city.",
                }
            end
        end
    end
    return {id = city:GetID(), queue = queue, overflow = city:GetOverflowProduction(), reasons = reasons}
end

local function unit_actions(unit)
    local rows = array()
    local busy = unit:IsBusy() or unit:IsDelayedDeath()
    local function add(id, applies, enabled, reason, cost)
        if applies then rows[#rows + 1] = {id = id, enabled = not busy and enabled, reason = (busy or not enabled) and reason or nil, cost = cost} end
    end
    local info = GameInfo.Units[unit:GetUnitType()]
    add(101, unit:IsCanDefend(), unit:CanStartMission(GameInfoTypes.MISSION_ALERT, -1, -1, nil, 0), "The unit cannot go on alert here.")
    add(102, info and (info.Pillage == true or info.Pillage == 1), unit:CanStartMission(GameInfoTypes.MISSION_PILLAGE, -1, -1, nil, 0), "The unit cannot pillage here.")
    local upgrade = unit:GetUpgradeUnitType()
    add(103, unit:CanUpgradeRightNow(1), unit:CanUpgradeRightNow(0), "Upgrade requirements are not met.", upgrade >= 0 and unit:UpgradePrice(upgrade) or nil)
    return rows
end

local function unit_data(unit)
    if not unit then return nil end
    local promotions = array()
    for promotion in GameInfo.UnitPromotions() do
        if unit:IsHasPromotion(promotion.ID) then promotions[#promotions + 1] = {id = promotion.ID} end
    end
    return {id = unit:GetID(), promotions = promotions, actions = unit_actions(unit)}
end

local function order(player, args, net, row)
    local unit = assert(own_unit(player, args.a), "unit id is required")
    local enabled = false
    for _, action in ipairs(unit_actions(unit)) do
        if action.id == 100 + row then enabled = action.enabled end
    end
    assert(enabled, "unit action is unavailable")
    if row == 3 then
        net.send("sendDoCommand", args.a, GameInfoTypes.COMMAND_UPGRADE, 0, 0, 0)
    else
        local mission = row == 1 and GameInfoTypes.MISSION_ALERT or GameInfoTypes.MISSION_PILLAGE
        net.send("sendPushMission", args.a, mission, 0, 0, 0, 0)
    end
end

return {
    data = function(player, args)
        local p, team = Players[player], team_of(player)
        local tt = team:GetTeamTechs()
        local techs, resources, policies = array(), array(), array()
        for tech in GameInfo.Technologies() do
            local pos = p:GetQueuePosition(tech.ID)
            techs[#techs + 1] = {
                id = tech.ID, status = team:IsHasTech(tech.ID) and 0 or (p:CanResearch(tech.ID) and 1 or 2),
                queue = pos > 0 and pos - 1 or nil,
                turns = turns(p:GetResearchTurnsLeft(tech.ID, true)),
                progress = tt:GetResearchProgress(tech.ID), cost = p:GetResearchCost(tech.ID),
            }
        end
        for resource in GameInfo.Resources() do
            local usage = Game.GetResourceUsageType(resource.ID)
            if usage == 1 or usage == 2 then
                resources[#resources + 1] = {
                    id = resource.ID, usage = usage, available = p:GetNumResourceAvailable(resource.ID, true),
                    used = p:GetNumResourceUsed(resource.ID), total = p:GetNumResourceTotal(resource.ID, true),
                }
            end
        end
        for branch in GameInfo.PolicyBranchTypes() do
            local owned, total = 0, 0
            for policy in GameInfo.Policies{PolicyBranchType = branch.Type} do
                total = total + 1
                if p:HasPolicy(policy.ID) then owned = owned + 1 end
            end
            policies[#policies + 1] = {
                id = branch.ID, name = L(branch.Description), type_name = branch.Type,
                unlocked = p:IsPolicyBranchUnlocked(branch.ID), owned = owned, total = total,
            }
        end
        return {
            research = {techs = techs}, top = {resources = resources, policies = policies, tourism = p:GetTourism()},
            city = city_data(own_city(player, args.city)), unit = unit_data(own_unit(player, args.unit)),
        }
    end,
    actions = {
        alert = function(player, args, net) order(player, args, net, 1) end,
        pillage = function(player, args, net) order(player, args, net, 2) end,
        upgrade = function(player, args, net) order(player, args, net, 3) end,
    },
}
