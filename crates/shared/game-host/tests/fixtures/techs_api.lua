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
    {ID=1, Type="TECH_B", Description="Pottery", Era="ERA_A", GridX=1, GridY=0, Help="Granary"},
    {ID=2, Type="TECH_C", Description="Writing", Era="ERA_A", GridX=2, GridY=0, Help="Library"}}),
    Eras=rows({{ID=0, Type="ERA_A", Description="Ancient"}}),
    Civilizations=rows({{ID=0, Type="CIV_TEST", Description="Civilization"}}),
    Leaders=rows({{ID=0, Type="LEADER_TEST", Description="Leader"}}),
    Technology_PrereqTechs=rows({{ID=0, Type="LINK", TechType="TECH_C", PrereqTech="TECH_B"}}),
    Technology_ORPrereqTechs=rows({}), Units=rows({}), Buildings=rows({}), Projects=rows({}), Builds=rows({}),
    Civilization_UnitClassOverrides=rows({}), Civilization_BuildingClassOverrides=rows({})}
Locale={ConvertTextKey=function(key) return key end}
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
    ScienceToStealAmount=function() return 30 end}
Players={[3]=p,[1]=p}
Teams={[3]={IsHasTech=function(_,id) return id==0 end, IsHasMet=function() return true end,
    GetTeamTechs=function() return {GetResearchProgress=function() return 0 end} end}}
