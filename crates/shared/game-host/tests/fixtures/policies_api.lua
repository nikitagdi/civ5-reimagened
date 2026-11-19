-- Deterministic seat-scoped queries; deliberately no active-player API.
local function catalog(rows)
    local out = {}
    for _, row in ipairs(rows) do out[row.ID] = row; if row.Type then out[row.Type] = row end end
    return setmetatable(out, {__call = function(_, filter)
        local i = 0
        return function()
            while i < #rows do
                i = i + 1
                local row, match = rows[i], true
                for k,v in pairs(filter or {}) do if row[k] ~= v then match = false end end
                if match then return row end
            end
        end
    end})
end
GameInfo = {
    Civilizations = catalog({{ID=0,Type="CIV_OWN",ShortDescription="Own civilization",Adjective="Own",IconAtlas="CIV",PortraitIndex=0},
        {ID=1,Type="CIV_OTHER",ShortDescription="Foreign civilization",Adjective="Foreign",IconAtlas="CIV",PortraitIndex=1}}),
    Eras = catalog({{ID=0,Type="ERA_ANCIENT",Description="Ancient"},{ID=1,Type="ERA_CLASSICAL",Description="Classical"},
        {ID=4,Type="ERA_INDUSTRIAL",Description="Industrial"}}),
    PolicyBranchTypes = catalog({{ID=0,Type="BRANCH",Description="Branch",Help="Branch effect",EraPrereq="ERA_CLASSICAL",PurchaseByLevel=false},
        {ID=9,Type="POLICY_BRANCH_ORDER",Description="Order",Help="Ideology effect",PurchaseByLevel=true}}),
    Policies = catalog({{ID=1,Type="POLICY_TEST",Description="Policy",Help="Policy effect",PolicyBranchType="BRANCH",GridX=1,GridY=1,Level=0},
        {ID=2,Type="TENET",Description="Tenet",Help="Tenet effect",PolicyBranchType="POLICY_BRANCH_ORDER",GridX=1,GridY=1,Level=1}}),
    Policy_PrereqPolicies = catalog({}), PolicyBranch_Disables = catalog({}),
    Beliefs = catalog({{ID=1,Type="BELIEF_TEST",ShortDescription="Belief",Description="Belief effect",Pantheon=true},
        {ID=2,Type="BELIEF_FOUNDER",ShortDescription="Founder",Description="Founder effect",Founder=true},
        {ID=3,Type="BELIEF_FOLLOWER",ShortDescription="Follower",Description="Follower effect",Follower=true},
        {ID=4,Type="BELIEF_ENHANCER",ShortDescription="Enhancer",Description="Enhancer effect",Enhancer=true}}),
    Religions = catalog({{ID=0,Type="RELIGION_PANTHEON",Description="Pantheon"},
        {ID=1,Type="RELIGION_OTHER",Description="Religion"},{ID=2,Type="RELIGION_OWN",Description="Own religion"}}),
    Units = catalog({}), Buildings = catalog({}), BuildingClasses = catalog({}),
}
GameOptionTypes = {GAMEOPTION_NO_POLICIES=1,GAMEOPTION_NO_RELIGION=2}
GameInfoTypes = {MISSION_FOUND_RELIGION=1,MISSION_ENHANCE_RELIGION=2}
GameDefines = {MAX_MAJOR_CIVS=6}
YieldTypes = {YIELD_FAITH=5}
Locale = {ConvertTextKey=function(key) return key or "" end}
cities, era, blocked, unlocked, ownreligion, ideology, faith = 1, 0, false, false, false, -1, 50
local city, unit, p, other, pantheon = {}, {}, {}, {}, {}
function city:GetID() return 4 end
function city:GetOwner() return 3 end
function city:GetName() return "Own city" end
function city:GetX() return 10 end
function city:GetY() return 20 end
function city:Plot() return {IsVisible=function() return false end} end
function city:IsCanPurchase() return false end
function unit:GetX() return 10 end
function unit:GetY() return 20 end
function unit:CanStartMission() return true end
function p:GetNumCities() return cities end
function p:IsAlive() return true end
function p:GetTeam() return 0 end
function p:GetCivilizationType() return 0 end
function p:GetCurrentEra() return era end
function p:GetJONSCulture() return 200 end
function p:GetTotalJONSCulturePerTurn() return 10 end
function p:GetNextPolicyCost() return 100 end
function p:GetNumFreePolicies() return 0 end
function p:GetNumFreeTenets() return 0 end
function p:IsPolicyBranchUnlocked() return unlocked end
function p:IsPolicyBranchFinished() return false end
function p:IsPolicyBranchBlocked() return blocked end
function p:IsPolicyBlocked() return blocked end
function p:HasPolicy() return false end
function p:CanAdoptPolicy() return cities>0 and unlocked and not blocked end
function p:CanUnlockPolicyBranch() return cities>0 and era>=1 end
function p:GetLateGamePolicyTree() return ideology end
function p:GetDominantPolicyBranchForTitle() return -1 end
function p:GetAvailableTenets(level) return level==1 and {2} or {} end
function p:GetPublicOpinionUnhappiness() return 0 end
function p:GetPublicOpinionPreferredIdeology() return -1 end
function p:GetPublicOpinionType() return 0 end
function p:GetPublicOpinionTooltip() return "Content" end
function p:GetPublicOpinionUnhappinessTooltip() return "No unhappiness" end
function p:GetCivilizationAdjectiveKey() return "Own" end
function p:GetBuildingClassCount() return 0 end
function p:GetFaith() return faith end
function p:GetTotalFaithPerTurn() return 5 end
function p:GetFaithPerTurnFromCities() return 5 end
function p:GetFaithPerTurnFromMinorCivs() return 0 end
function p:GetFaithPerTurnFromReligion() return 0 end
function p:GetMinimumFaithNextGreatProphet() return 200 end
function p:HasCreatedPantheon() return false end
function p:HasCreatedReligion() return ownreligion end
function p:CanCreatePantheon() return cities>0 and faith>=25 end
function p:CanChooseReformationBelief() return false end
function p:GetReligionCreatedByPlayer() return ownreligion and 2 or -1 end
function p:IsTraitBonusReligiousBelief() return false end
function p:GetCityByID(id) if cities>0 and id==4 then return city end end
function p:GetCapitalCity() if cities>0 then return city end end
function p:GetFaithPurchaseType() return 0 end
function p:GetFaithPurchaseIndex() return -1 end
function p:Units() local sent=false; return function() if cities>0 and not sent then sent=true; return unit end end end
function other:IsAlive() return true end
function other:GetTeam() return 1 end
function other:GetCivilizationType() return 1 end
function other:GetLateGamePolicyTree() return 9 end
function other:HasCreatedPantheon() return true end
function other:HasCreatedReligion() return true end
function pantheon:IsAlive() return true end
function pantheon:GetTeam() return 1 end
function pantheon:HasCreatedPantheon() return true end
function pantheon:HasCreatedReligion() return false end
function pantheon:GetLateGamePolicyTree() return -1 end
function pantheon:GetBeliefInPantheon() return 1 end
Players = {[3]=p,[4]=other,[5]=pantheon}
Teams = {[0]={IsHasMet=function() return false end}}
Game = {
    IsOption=function() return false end, GetNumFreePolicies=function() return 2 end,
    GetAvailablePantheonBeliefs=function() return {1} end, GetAvailableReformationBeliefs=function() return {} end,
    GetAvailableFounderBeliefs=function() return {2} end, GetAvailableFollowerBeliefs=function() return {3} end,
    GetAvailableEnhancerBeliefs=function() return {4} end, GetAvailableBonusBeliefs=function() return {1,2,3,4} end,
    GetNumReligionsStillToFound=function() return 1 end,
    GetReligionName=function(id) return id==2 and "Own religion" or "Religion" end,
    GetGameTurnYear=function() return -4000 end,
    GetFounder=function(id) return id==1 and 4 or ownreligion and 3 or -1 end,
    GetHolyCityForReligion=function(id)
        if id==2 then return city end
        return {GetOwner=function() return 4 end,GetName=function() return "Hidden city" end,Plot=city.Plot}
    end,
    GetNumCitiesFollowing=function() return 1 end,
    GetBeliefsInReligion=function(id) return id==2 and {1,2,3} or {1} end,
}
