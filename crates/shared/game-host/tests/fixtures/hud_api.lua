-- Seat-explicit getter fixtures for the built-in HUD script.
local function rows(values)
    local out = {}
    for _, value in ipairs(values) do out[value.ID] = value end
    return setmetatable(out, {__call = function()
        local index = 0
        return function() index = index + 1; return values[index] end
    end})
end
GameInfo = {
    Technologies = rows({{ID = 0}, {ID = 1}}),
    Resources = rows({{ID = 0}, {ID = 1}}),
    PolicyBranchTypes = rows({{ID = 0, Type = "TRADITION", Description = "Tradition"}}),
    Policies = rows({{ID = 0}, {ID = 1}}),
    UnitPromotions = rows({{ID = 2}}),
    Units = rows({{ID = 2, Pillage = true}}),
    Buildings = rows({{ID = 4}}), Projects = rows({}), Processes = rows({}),
}
GameInfoTypes = {MISSION_ALERT = 10, MISSION_PILLAGE = 11, COMMAND_UPGRADE = 12}
Game = {GetResourceUsageType = function(id) return id + 1 end}
Locale = {ConvertTextKey = function(key) return key end}
hud_busy = false
local unit = {
    GetID = function() return 4 end, GetOwner = function() return 3 end,
    IsBusy = function() return hud_busy end, IsDelayedDeath = function() return false end,
    GetUnitType = function() return 2 end, IsCanDefend = function() return true end,
    CanStartMission = function(_, mission) return mission == 10 end,
    CanUpgradeRightNow = function() return true end,
    GetUpgradeUnitType = function() return 5 end, UpgradePrice = function() return 210 end,
    IsHasPromotion = function(_, id) return id == 2 end,
}
local city = {
    GetID = function() return 3 end, GetOwner = function() return 3 end,
    GetOrderQueueLength = function() return 2 end,
    GetOrderFromQueue = function(_, slot) return slot, slot == 0 and 2 or 4 end,
    GetUnitProductionTurnsLeft = function(_, item, slot) assert(item == 2 and slot == 0); return 3 end,
    GetBuildingProductionTurnsLeft = function(_, item, slot) assert(item == 4 and slot == 1); return 7 end,
    GetUnitProductionNeeded = function() return 60 end,
    GetBuildingProductionNeeded = function() return 100 end,
    GetOverflowProduction = function() return 7 end,
    CanTrain = function() return true end, CanConstruct = function() return false end,
}
local player = {
    GetTeam = function() return 3 end,
    GetCityByID = function(_, id) if id == 3 then return city end end,
    GetUnitByID = function(_, id) if id == 4 then return unit end end,
    GetQueuePosition = function(_, id) return id + 1 end,
    CanResearch = function(_, id) return id == 1 end,
    GetResearchTurnsLeft = function() return 3 end,
    GetResearchCost = function() return 100 end,
    GetNumResourceAvailable = function() return 2 end,
    GetNumResourceUsed = function() return 1 end,
    GetNumResourceTotal = function() return 3 end,
    HasPolicy = function(_, id) return id == 0 end,
    IsPolicyBranchUnlocked = function() return true end,
    GetTourism = function() return 4 end,
}
Players = {[3] = player, [1] = player}
Teams = {[3] = {
    IsHasTech = function(_, id) return id == 0 end,
    GetTeamTechs = function() return {GetResearchProgress = function() return 20 end} end,
}}
