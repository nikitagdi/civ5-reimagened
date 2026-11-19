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
    Resources = rows({{ID = 0, Description = "Iron"}, {ID = 1, Description = "Silk"}}),
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

-- Nonzero source values exercise hundredths conversion and preserve real getter names.
local yield_getters = {
    "GetScience", "GetScienceFromCitiesTimes100", "GetScienceFromOtherPlayersTimes100",
    "GetScienceFromHappinessTimes100", "GetScienceFromResearchAgreementsTimes100", "GetScienceFromBudgetDeficitTimes100",
    "GetOverflowResearch", "CalculateGoldRate", "GetGold", "CalculateGrossGold", "CalculateInflatedCosts",
    "GetGoldFromCitiesTimes100", "GetCityConnectionGoldTimes100", "GetGoldPerTurnFromDiplomacy", "GetGoldPerTurnFromReligion",
    "CalculateUnitCost", "CalculateUnitSupply", "GetBuildingGoldMaintenance", "GetImprovementGoldMaintenance",
    "GetExcessHappiness", "GetHappiness", "GetUnhappiness", "GetHappinessFromPolicies", "GetHappinessFromResources",
    "GetHappinessFromBuildings", "GetHappinessFromReligion", "GetHappinessFromNaturalWonders", "GetHappinessFromMinorCivs",
    "GetHappinessFromLeagues", "GetTotalJONSCulturePerTurn", "GetJONSCulture", "GetNextPolicyCost",
    "GetJONSCulturePerTurnFromCities", "GetJONSCulturePerTurnFromExcessHappiness", "GetJONSCulturePerTurnFromTraits",
    "GetJONSCulturePerTurnForFree", "GetJONSCulturePerTurnFromMinorCivs", "GetCulturePerTurnFromReligion",
    "GetTotalFaithPerTurn", "GetFaith", "GetFaithPerTurnFromCities", "GetFaithPerTurnFromMinorCivs", "GetFaithPerTurnFromReligion",
    "GetGoldenAgeTurns", "GetGoldenAgeProgressMeter", "GetGoldenAgeProgressThreshold", "GetGoldenAgePointsFromCities",
    "GetNumGreatWorks", "GetNumGreatWorkSlots", "GetAnarchyNumTurns", "GetNumInternationalTradeRoutesUsed",
    "GetNumInternationalTradeRoutesAvailable", "GetResourceImport", "GetResourceExport",
}
for _, name in ipairs(yield_getters) do player[name] = function() return 125 end end

GameDefines = {MAX_CIV_PLAYERS = 4}
GameInfo.Civilizations = rows({{ID=0, ShortDescription="Fixture civilization"}})
function player:IsAlive() return true end
function player:IsHuman() return true end
function player:IsTurnActive() return true end
function player:IsSimultaneousTurns() return true end
function player:GetCivilizationType() return 0 end
function player:GetEndTurnBlockingType() return 8 end
function player:GetEndTurnBlockingNotificationIndex() return 7 end
function player:GetNotificationStr(id) assert(id == 7); return "A unit needs orders." end
function player:Units() local done=false; return function() if not done then done=true; return unit end end end
function unit:IsReadyToMove() return not hud_busy end
function unit:GetName() return "Fixture unit" end
Teams[3].IsHasMet = function() return false end

function unit:ExperienceNeeded() return 60 end
function unit:GetPlot() return {} end
function unit:CanDoCommand(_, _, _, visible) return visible == 1 or not hud_busy end
GameInfoActions = {
    [0] = {Visible=true, TextKey="Alert", MissionType=10, MissionData=0, CommandType=-1, HotKeyVal=13, Help="Watch for enemies."},
    [1] = {Visible=true, TextKey="Upgrade", MissionType=-1, CommandType=12, CommandData=0, HotKeyVal=33, DisabledHelp="Not enough gold."},
}

GameDefines.MAX_HIT_POINTS = 100
GameDefines.MOVE_DENOMINATOR = 60
GameInfo.Specialists = rows({})
GameInfo.UnitClasses = rows({})
function unit:GetActivityType() return 0 end
function unit:GetDamage() return 10 end
function unit:MovesLeft() return 120 end
function unit:MaxMoves() return 120 end
function unit:IsPromotionReady() return false end
function player:GetCombatExperience() return 15 end
function player:GreatGeneralThreshold() return 100 end
function player:GetNavalCombatExperience() return 10 end
function player:GreatAdmiralThreshold() return 100 end

GameInfo.Specialists.SPECIALIST_ENGINEER = {ID=1, Type="SPECIALIST_ENGINEER", GreatPeopleUnitClass="UNITCLASS_ENGINEER", GreatPeopleRateChange=3}
GameInfo.UnitClasses.UNITCLASS_ENGINEER = {ID=1, Description="Great Engineer"}
GameInfo.Buildings[4].GreatPeopleRateChange = 2
function player:Cities() local done=false; return function() if not done then done=true; return city end end end
function player:GetGreatPeopleRateModifier() return 25 end
function player:GetGreatEngineerRateModifier() return 0 end
function city:GetName() return "Fixture capital" end
function city:GetSpecialistCount() return 2 end
function city:GetNumBuilding() return 1 end
function city:GetGreatPeopleRateModifier() return 25 end
function city:GetSpecialistGreatPersonProgressTimes100() return 4000 end
function city:GetSpecialistUpgradeThreshold() return 100 end

function player:IsObserver() return false end
function player:GetReligionCreatedByPlayer() return -1 end
function player:HasCreatedPantheon() return false end

function city:GetStrengthValue() return 1800 end
function city:GetMaxHitPoints() return 200 end
function city:GetDamage() return 10 end
function city:GetFoodTurnsLeft() return 8 end
function city:GetFood() return 12 end
function city:GrowthThreshold() return 30 end
function city:FoodDifference() return -5 end
function city:IsResistance() return false end
function city:IsRazing() return false end
function unit:IsEmbarked() return false end
function unit:IsGarrisoned() return false end
function unit:GetFortifyTurns() return 0 end
function unit:IsTrade() return false end
function unit:GetBaseCombatStrength() return 8 end
function unit:GetBaseRangedCombatStrength() return 0 end
function unit:Range() return 0 end
function unit:GetCurrHitPoints() return 90 end
function unit:GetMaxHitPoints() return 100 end

function unit:IsCargo() return false end

GameOptionTypes = {GAMEOPTION_NO_SCIENCE=0, GAMEOPTION_NO_POLICIES=1, GAMEOPTION_NO_RELIGION=2,
    GAMEOPTION_NO_HAPPINESS=3, GAMEOPTION_NO_ESPIONAGE=4, GAMEOPTION_ONE_CITY_CHALLENGE=5}
Game.IsOption = function() return false end

-- The original top panel tooltips (TopPanel.lua): the getters they read, one value each.
GameDefines.MAX_MAJOR_CIVS = 4
GameDefines.MAXIMUM_ACQUIRE_PLOT_DISTANCE = 5
GameInfoTypes.ERA_INDUSTRIAL = 4
TradeableItems = {TRADE_ITEM_RESOURCES = 2, TRADE_ITEM_RESEARCH_AGREEMENT = 11}
YieldTypes = {YIELD_FAITH = 6}
GameInfo.Resources[0].Type, GameInfo.Resources[0].IconString = "RESOURCE_IRON", "[ICON_RES_IRON]"
GameInfo.Resources[1].Type, GameInfo.Resources[1].IconString = "RESOURCE_SILK", "[ICON_RES_SILK]"
GameInfo.Civilizations[0].Type = "CIVILIZATION_FIXTURE"
GameInfo.Leaders = rows({{ID = 0, Type = "LEADER_FIXTURE"}})
GameInfo.Victories = {}
for _, name in ipairs({"Leader_Traits", "Trait_NoTrain", "Civilization_UnitClassOverrides", "Civilization_BuildingClassOverrides",
    "Building_ResourceQuantity", "Building_ResourceQuantityRequirements"}) do GameInfo[name] = rows({}) end
GameInfo.Units[2].Type, GameInfo.Units[2].Class, GameInfo.Units[2].Description = "UNIT_FIXTURE", "UNITCLASS_FIXTURE", "Fixture swordsman"
GameInfo.UnitClasses.UNITCLASS_FIXTURE = {ID = 2, DefaultUnit = "UNIT_FIXTURE"}
GameInfo.Unit_ResourceQuantityRequirements = rows({{ID = 0, UnitType = "UNIT_FIXTURE", ResourceType = "RESOURCE_IRON"}})
GameInfo.Resources.RESOURCE_IRON = GameInfo.Resources[0]
Game.GetGameTurn = function() return 10 end
Game.GetNumCitiesTechCostMod = function() return 5 end
Game.GetNumCitiesPolicyCostMod = function() return 10 end
Game.GetNumReligionsStillToFound = function() return 2 end
Game.GetMinimumFaithNextPantheon = function() return 25 end
Game.GetNumResourceRequiredForUnit = function(unit, resource) return (unit == 2 and resource == 0) and 1 or 0 end
Game.GetNumResourceRequiredForBuilding = function() return 0 end
for _, name in ipairs({"GetUnhappinessFromPuppetCityPopulation", "GetUnhappinessFromCitySpecialists", "GetUnhappinessFromCityPopulation",
    "GetUnhappinessFromCityCount", "GetUnhappinessFromCapturedCityCount", "GetUnhappinessFromOccupiedCities", "GetUnhappinessFromUnits",
    "GetResourceFromMinors", "GetGoldenAgeTourismModifier", "GetMinimumFaithNextGreatProphet", "CountNumBuildings"}) do
    player[name] = function() return 0 end
end
function player:GetNumCities() return 1 end
function player:GetCurrentResearch() return 1 end
function player:GetResearchProgress() return 20 end
function player:GetCurrentEra() return 0 end
function player:GetLeaderType() return 0 end
function player:IsAnarchy() return false end
function player:IsEmpireUnhappy() return false end
function player:IsEmpireVeryUnhappy() return false end
function player:IsEmpireSuperUnhappy() return false end
function player:IsGoldenAgeCultureBonusDisabled() return false end
function player:CanCreatePantheon() return true end
function player:HasCreatedReligion() return false end
function city:GetCityIndexPlot() return nil end
function city:GetProductionBuilding() return -1 end
Teams[3].GetTeamTechs = function() return {GetResearchProgress = function() return 20 end,
    GetLastTechAcquired = function() return -1 end, HasTech = function(_, id) return id == 0 end} end
Teams[3].IsHasResearchAgreement = function() return false end

-- The original unit list's status (UnitList.lua).
function unit:GetNameKey() return "Fixture unit" end
function unit:IsCombatUnit() return true end
function unit:GetDomainType() return 2 end
function unit:GetBuildType() return -1 end
function unit:GetUnitCombatType() return 1 end
function unit:IsAutomated() return false end
function unit:GetPlot() return {GetPlotCity = function() return nil end, IsVisible = function() return true end} end
