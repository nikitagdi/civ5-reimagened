local function rows(values)
    local out = {}
    for _, v in ipairs(values) do out[v.ID], out[v.Type] = v, v end
    return setmetatable(out, {__call = function(_, filter)
        local i = 0
        return function()
            while i < #values do
                i = i + 1
                local v, match = values[i], true
                for k, value in pairs(filter or {}) do if v[k] ~= value then match = false end end
                if match then return v end
            end
        end
    end})
end
GameInfo = { Technologies = rows({
    {ID=0, Type="TECH_A", Description="Agriculture", Era="ERA_A", GridX=0, GridY=1, Help="Food"},
    {ID=1, Type="TECH_B", Description="Pottery", Era="ERA_A", GridX=1, GridY=0, Help="Granary",
        AllowsEmbarking=true, EmbarkedAllWaterPassage=1, AllowsDefensiveEmbarking=false, MapVisible=0,
        BridgeBuilding=true, InternationalTradeRoutesChange=2},
    {ID=2, Type="TECH_C", Description="Writing", Era="ERA_A", GridX=2, GridY=0, Help="Library",
        OpenBordersTradingAllowed=true, DefensivePactTradingAllowed=true, ResearchAgreementTradingAllowed=true,
        MapTrading=true, EmbarkedMoveChange=1}}),
    Eras=rows({{ID=0, Type="ERA_A", Description="Ancient"}}),
    Civilizations=rows({{ID=0, Type="CIV_TEST", Description="Civilization"},
        {ID=1, Type="CIV_TARGET", Description="Rival", IconAtlas="CIV_ATLAS", PortraitIndex=2}}),
    Leaders=rows({{ID=0, Type="LEADER_TEST", Description="Leader"}}),
    Technology_PrereqTechs=rows({{ID=0, Type="LINK", TechType="TECH_C", PrereqTech="TECH_B"}}),
    Technology_ORPrereqTechs=rows({}),
    UnitClasses=rows({
        {ID=0, Type="UNITCLASS_TEST", DefaultUnit="UNIT_DEFAULT"},
        {ID=1, Type="UNITCLASS_DISABLED", DefaultUnit="UNIT_DISABLED"}}),
    Units=rows({
        {ID=0, Type="UNIT_DEFAULT", Class="UNITCLASS_TEST", PrereqTech="TECH_B", Description="Default unit"},
        {ID=1, Type="UNIT_UNIQUE", Class="UNITCLASS_TEST", PrereqTech="TECH_B", Description="Unique unit", IconAtlas="UNIT_ATLAS", PortraitIndex=0},
        {ID=2, Type="UNIT_FOREIGN", Class="UNITCLASS_TEST", PrereqTech="TECH_B", Description="Foreign unit"},
        {ID=3, Type="UNIT_DISABLED", Class="UNITCLASS_DISABLED", PrereqTech="TECH_B", Description="Disabled unit"}}),
    Civilization_UnitClassOverrides=rows({
        {ID=0, Type="UNIQUE", CivilizationType="CIV_TEST", UnitClassType="UNITCLASS_TEST", UnitType="UNIT_UNIQUE"},
        {ID=1, Type="DISABLED", CivilizationType="CIV_TEST", UnitClassType="UNITCLASS_DISABLED"}}),
    BuildingClasses=rows({{ID=0, Type="BUILDINGCLASS_WONDER", DefaultBuilding="BUILDING_WONDER", MaxGlobalInstances=1}}),
    Buildings=rows({{ID=0, Type="BUILDING_WONDER", BuildingClass="BUILDINGCLASS_WONDER", PrereqTech="TECH_B", Description="Wonder"}}),
    Civilization_BuildingClassOverrides=rows({}),
    Resources=rows({
        {ID=0, Type="RESOURCE_IRON", TechReveal="TECH_B", Description="Iron", IconAtlas="RESOURCE_ATLAS", PortraitIndex=7},
        {ID=1, Type="RESOURCE_COAL", TechReveal="TECH_C", Description="Coal"}}),
    Projects=rows({{ID=0, Type="PROJECT_TEST", TechPrereq="TECH_B", Description="Project"}}),
    Improvements=rows({
        {ID=0, Type="IMPROVEMENT_FARM", Description="Farm"},
        {ID=1, Type="IMPROVEMENT_FOREIGN", CivilizationType="CIV_TARGET", Description="Foreign improvement"}}),
    Builds=rows({
        {ID=0, Type="BUILD_FARM", PrereqTech="TECH_B", ImprovementType="IMPROVEMENT_FARM", ShowInTechTree=1,
            Description="Build farm", IconAtlas="BUILD_ATLAS", IconIndex=4},
        {ID=1, Type="BUILD_FOREIGN", PrereqTech="TECH_B", ImprovementType="IMPROVEMENT_FOREIGN", ShowInTechTree=1,
            Description="Build foreign improvement"},
        {ID=2, Type="BUILD_HIDDEN", PrereqTech="TECH_B", ShowInTechTree=0, Description="Hidden action"}}),
    Processes=rows({{ID=0, Type="PROCESS_TEST", TechPrereq="TECH_B", Description="Wealth"}}),
    Routes=rows({{ID=0, Type="ROUTE_ROAD", Description="Road"}}),
    Route_TechMovementChanges=rows({{ID=0, Type="ROUTE_BOOST", TechType="TECH_B", RouteType="ROUTE_ROAD", MovementChange=-20}}),
    Yields=rows({{ID=0, Type="YIELD_FOOD", Description="Food"}, {ID=1, Type="YIELD_GOLD", Description="Gold"}}),
    Improvement_TechYieldChanges=rows({
        {ID=0, Type="FOOD", TechType="TECH_B", ImprovementType="IMPROVEMENT_FARM", YieldType="YIELD_FOOD", Yield=1},
        {ID=1, Type="GOLD", TechType="TECH_B", ImprovementType="IMPROVEMENT_FARM", YieldType="YIELD_GOLD", Yield=2}}),
    Improvement_TechFreshWaterYieldChanges=rows({
        {ID=0, Type="FRESH", TechType="TECH_B", ImprovementType="IMPROVEMENT_FARM", YieldType="YIELD_FOOD", Yield=1}}),
    Improvement_TechNoFreshWaterYieldChanges=rows({
        {ID=0, Type="DRY", TechType="TECH_B", ImprovementType="IMPROVEMENT_FARM", YieldType="YIELD_FOOD", Yield=1}}),
    Technology_TradeRouteDomainExtraRange=rows({{ID=0, Type="LAND", TechType="TECH_B", DomainType="DOMAIN_LAND", Range=5}}),
    UnitPromotions=rows({{ID=0, Type="PROMOTION_TEST", Description="Promotion", Help="Promotion help", IconAtlas="PROMOTION_ATLAS", PortraitIndex=3}}),
    Technology_FreePromotions=rows({{ID=0, Type="FREE_PROMOTION", TechType="TECH_B", PromotionType="PROMOTION_TEST"}})}
local translations = {
    TXT_KEY_REVEALS_RESOURCE_ON_MAP="Reveals %s",
    TXT_KEY_ALLOWS_EMBARKING="Allows embarkation",
    TXT_KEY_YIELD_IMPROVED="%s [ICON_FOOD]%s +%d.",
    TXT_KEY_FRESH_WATER="%s with fresh water: %s +%d",
    TXT_KEY_NO_FRESH_WATER="%s without fresh water: %s +%d",
    TXT_KEY_FASTER_MOVEMENT="Faster movement on %s",
}
Locale={ConvertTextKey=function(key, ...)
    return translations[key] and string.format(translations[key], ...) or key
end}
function by_unlock_key(unlocks)
    local by_key = {}
    for _, unlock in ipairs(unlocks) do
        assert(not by_key[unlock.key])
        by_key[unlock.key] = unlock
    end
    return by_key
end
free_count, current, off = 0, -1, false
Game={IsOption=function() return off end, GetGameTurn=function() return 1 end}
GameInfoTypes={GAMEOPTION_NO_SCIENCE=0}
GameDefines={MAX_MAJOR_CIVS=22}
local p={GetTeam=function() return 3 end, GetCivilizationType=function() return 0 end,
    GetLeaderType=function() return 0 end, GetScience=function() return 10 end,
    GetNumFreeTechs=function() return free_count end, GetQueuePosition=function(_, id) return id==current and 1 or -1 end,
    GetCurrentResearch=function() return current end, CanResearch=function(_, id) return id==1 end,
    CanEverResearch=function(_, id) return id~=0 end, CanResearchForFree=function(_, id) return id==1 end,
    GetResearchCost=function() return 50 end, GetResearchTurnsLeft=function() return 5 end,
    GetCurrentEra=function() return 0 end, GetNumTechsToSteal=function() return 1 end,
    canStealTech=function(_, target, id) return target==1 and id==1 end,
    ScienceToStealAmount=function() return 30 end,
    GetEspionageSpies=function() return spies end}
Players={[3]=p,[1]={
    GetTeam=function() return 1 end, GetCivilizationType=function() return 1 end,
    GetLeaderType=function() return 0 end, GetPlayerColors=function() return {x=.2,y=.4,z=.6,w=1} end}}
Teams={[3]={IsHasTech=function(_,id) return id==0 end, IsHasMet=function() return true end,
    GetTeamTechs=function() return {GetResearchProgress=function() return 0 end} end}}
local plots = {}
for x = 1, 5 do
    plots[x] = {
        IsRevealed=function(_, team, debug) assert(team == 3 and not debug); return x ~= 4 end,
        GetPlotCity=function() return {
            GetOwner=function() return x == 3 and 2 or 1 end,
            GetName=function() return x == 1 and "Spy City" or "Other City" end,
        } end,
    }
end
Map={GetPlot=function(x, y) assert(y == 1); return plots[x] end}
local gathering = "TXT_KEY_SPY_STATE_GATHERING_INTEL"
spies={
    {State=gathering, CityX=-1, CityY=-1},
    {State="TXT_KEY_SPY_STATE_SURVEILLANCE", CityX=2, CityY=1},
    {State=gathering, CityX=2, CityY=1, IsDiplomat=true},
    {State=gathering, CityX=3, CityY=1},
    {State=gathering, CityX=4, CityY=1},
    {State=gathering, CityX=1, CityY=1},
}
