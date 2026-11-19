known, war, building_present = false, false, true
Locale = {ConvertTextKey = function(key) return key end}
GameDefines = {MAX_MAJOR_CIVS = 2, CULTURE_LEVEL_EXOTIC = 10, CULTURE_LEVEL_FAMILIAR = 30, CULTURE_LEVEL_POPULAR = 60, CULTURE_LEVEL_INFLUENTIAL = 100, CULTURE_LEVEL_DOMINANT = 200}
GameInfoTypes = {GAMEOPTION_NO_CULTURE_OVERVIEW = 1}
local function records(rows)
    return setmetatable(rows, {__call = function(self)
        local index = 0
        return function() local value = self[index]; index = index + 1; return value end
    end})
end
GameInfo = {
    Civilizations = {[0] = {Type = "CIVILIZATION_TEST", ShortDescription = "Civilization"}},
    Leaders = {[0] = {Type = "LEADER_TEST", Description = "Leader"}},
    Civilization_BuildingClassOverrides = records({}),
    Buildings = {BUILDING_TEST = {ID = 0, Type = "BUILDING_TEST", Description = "Gallery", GreatWorkCount = 2, GreatWorkSlotType = "GREAT_WORK_SLOT_ART_ARTIFACT"}},
    BuildingClasses = records({[0] = {ID = 0, Type = "BUILDINGCLASS_TEST", Description = "Gallery", DefaultBuilding = "BUILDING_TEST", MaxGlobalInstances = -1, MaxPlayerInstances = -1}}),
    PolicyBranchTypes = {},
}
GameInfo.BuildingClasses.BUILDINGCLASS_TEST = GameInfo.BuildingClasses[0]
local city = {
    GetID = function() return 7 end, GetOwner = function() return 0 end,
    GetName = function() return "City" end, IsCapital = function() return true end,
    IsPuppet = function() return false end, IsOccupied = function() return false end,
    GetDamage = function() return 0 end, GetMaxHitPoints = function() return 200 end,
    GetJONSCulturePerTurn = function() return 3 end, GetBaseTourism = function() return 2 end,
    GetTourismTooltip = function() return "Tourism" end, GetNumGreatWorks = function() return 1 end,
    GetTotalSlotsTooltip = function() return "One free slot" end,
    IsHasBuilding = function() return building_present end,
    GetThemingBonus = function() return 0 end, GetThemingTooltip = function() return "Theme" end,
    GetBuildingGreatWork = function(self, class, slot) return slot == 0 and 10 or -1 end,
}
local function player(id)
    return {
        GetCivilizationType = function() return 0 end, GetLeaderType = function() return 0 end,
        GetTeam = function() return id end, IsAlive = function() return true end, IsHuman = function() return false end,
        GetPlayerColors = function() return {x = 1, y = 0, z = 0}, {x = 1, y = 1, z = 1} end,
        Cities = function() local once = false; return function() if not once and id == 0 then once = true; return city end end end,
        GetCityByID = function(self, value) if value == 7 and id == 0 then return city end end,
        GetGreatWorks = function(self, class) return class == 1 and {{Index = id == 0 and 10 or 20}} or {} end,
        GetSwappableGreatWriting = function() return -1 end,
        GetSwappableGreatArt = function() return id == 0 and 10 or 20 end,
        GetSwappableGreatArtifact = function() return -1 end,
        GetTourism = function() return 2 end, GetNumCivsInfluentialOn = function() return 0 end,
        GetNumCivsToBeInfluentialOn = function() return 1 end, GetLateGamePolicyTree = function() return -1 end,
        GetExcessHappiness = function() return 9 end, GetInfluenceOn = function() return 0 end,
        GetJONSCultureEverGenerated = function() return 1 end, GetInfluenceLevel = function() return 0 end,
        GetTurnsToInfluential = function() return 999 end, GetInfluenceTrend = function() return 0 end,
        GetTourismModifierWith = function() return 0 end, GetTourismModifierWithTooltip = function() return "No modifiers" end,
        GetInfluencePerTurn = function() return 0 end,
        GetCivilizationShortDescriptionKey = function() return "Civilization " .. id end,
    }
end
Players = {[0] = player(0), [1] = player(1)}
Teams = {[0] = {IsHasMet = function() return known end, IsAtWar = function() return war end}}
Game = {
    IsOption = function() return false end,
    GetGreatWorkCreator = function() return 0 end, GetGreatWorkClass = function() return 1 end,
    GetGreatWorkName = function() return "Painting" end, GetGreatWorkArtist = function() return "Artist" end,
    GetGreatWorkEraShort = function() return "Era" end, GetGreatWorkTooltip = function() return "Painting tooltip" end,
    GetGreatWorkCurrentThemingBonus = function() return 0 end,
    GetGreatWorkController = function(id) return id == 10 and 0 or 1 end,
}
