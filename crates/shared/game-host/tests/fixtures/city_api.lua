local function rows(values)
    local out = {}; for _, v in ipairs(values) do out[v.ID]=v; out[v.Type]=v end
    return setmetatable(out,{__call=function() local i=0; return function() i=i+1; return values[i] end end})
end
GameInfo = {
    Units=rows({{ID=2,Type="UNIT_TEST",Description="Unit",Help="Help"}}),
    Buildings=rows({{ID=4,Type="BUILDING_TEST",Description="Building",SpecialistType="SPEC",BuildingClass="CLASS",GoldMaintenance=1}}),
    BuildingClasses=rows({{ID=4,Type="CLASS",MaxGlobalInstances=0,MaxPlayerInstances=0,MaxTeamInstances=0}}),
    Specialists=rows({{ID=1,Type="SPEC"}}), Projects=rows({}), Processes=rows({}),
    Civilizations=rows({{ID=0,Type="CIV",ShortDescription="Civilization"}}), Resources=rows({}),
}
GameInfoTypes={DEFAULT_SPECIALIST=0,GAMEOPTION_NO_RELIGION=1,TASK_CHANGE_WORKING_PLOT=2,TASK_NO_AUTO_ASSIGN_SPECIALISTS=3,TASK_ADD_SPECIALIST=4,TASK_REMOVE_SPECIALIST=5,TASK_REMOVE_SLACKER=6}
Locale={ConvertTextKey=function(key) return key end}
Game={IsOption=function() return false end}
city_turn=true; city_puppet=false
local plot={GetOwner=function() return 3 end,IsVisibleEnemyUnit=function() return false end,IsVisible=function() return true end,GetX=function() return 1 end,GetY=function() return 2 end,GetPlotIndex=function() return 12 end,CalculateYield=function() return 1 end}
local city={
    GetProductionNameKey=function() return "Unit" end,GetProductionTurnsLeft=function() return 3 end,CanRangeStrikeNow=function() return false end,GetX=function() return 1 end,GetY=function() return 2 end,GetID=function() return 7 end,GetOwner=function() return 3 end,GetName=function() return "City" end,
    IsPuppet=function() return city_puppet end,GetOrderQueueLength=function() return 3 end,
    GetOrderFromQueue=function() return 0,2,0,false end,
    CanTrain=function(_,id) return id==2 end,CanConstruct=function() return false end,
    IsCanPurchase=function(_,cost,train,u,b,p,y) assert(type(cost)=='boolean' and train and (y==2 or y==5)); return u==2 and b==-1 and p==-1 end,
    IsBuildingSellable=function(_,id) return id==4 end,
    IsCanAddSpecialistToBuilding=function(_,id) return id==4 end,
    GetNumSpecialistsInBuilding=function() return 1 end,GetSpecialistCount=function() return 2 end,
    GetNumForcedWorkingPlots=function() return 1 end,GetNumCityPlots=function() return 2 end,
    GetCityIndexPlot=function() return plot end,IsPlotBlockaded=function() return false end,
    CanBuyPlotAt=function(_,x,y) return x==1 and y==2 end,
}
for _,name in ipairs({'GetUnitProductionTurnsLeft','GetUnitProductionNeeded','GetUnitPurchaseCost','GetUnitFaithPurchaseCost','GetCurrentProductionDifferenceTimes100','GetProduction','GetProductionNeeded','GetRazingTurns','GetResistanceTurns','GetYieldRate','GetBaseYieldRateFromTerrain','GetBaseYieldRateFromBuildings','GetBaseYieldRateFromSpecialists','GetBaseYieldRateFromMisc','GetBaseYieldRateFromReligion','GetBaseTourism','GetNumFreeBuilding','GetNumSpecialistsAllowedByBuilding','GetBuildingYieldChange','GetSellBuildingRefund','GetBuyPlotCost','GetFocusType','FoodDifferenceTimes100','GetWeLoveTheKingDayCounter','GetStrengthValue','GetMaxHitPoints','GetDamage','GetPopulation','GetFood','GrowthThreshold','GetFoodTurnsLeft','GetJONSCultureStored','GetJONSCultureThreshold','GetOverflowProduction','GetFeatureProduction'}) do city[name]=city[name] or function() return 1 end end
for _,name in ipairs({'IsCapital','IsBlockaded','IsRazing','IsResistance','IsOccupied','IsNoOccupiedUnhappiness','IsHasBuilding','IsForcedWorkingPlot','IsWorkingPlot','IsFoodProduction','IsForcedAvoidGrowth','IsNoAutoAssignSpecialists'}) do city[name]=function() return false end end
city.GetResourceDemanded=function() return -1 end; city.GetNextBuyablePlot=function() return nil end
local player={GetCityByID=function(_,id) if id==7 then return city end end,IsTurnActive=function() return city_turn end,GetGold=function() return 100 end,GetFaith=function() return 100 end,GetTeam=function() return 3 end,IsCapitalConnectedToCity=function() return true end,MayNotAnnex=function() return false end,CanRaze=function() return false end,GetCivilizationType=function() return 0 end,Cities=function() local done=false; return function() if not done then done=true; return city end end end}
Players={[3]=player,[1]={GetCityByID=function() return nil end}}

TaskTypes=GameInfoTypes

city_test_city=city; city_test_player=player

city.IsHasBuilding=function(_,id) return id==4 end
city.GetJONSCulturePerTurn=function() return 1 end
city.GetFaithPerTurn=function() return 1 end

GameInfo.PlayerColors=rows({{ID=0,Type="PALETTE"}})
GameInfo.Colors=rows({}); GameInfo.Yields=rows({}); GameInfo.Building_YieldChanges=rows({})
player.GetPlayerColor=function() return 0 end

Game.GetGameTurn=function() return 1 end
