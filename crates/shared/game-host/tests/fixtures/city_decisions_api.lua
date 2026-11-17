city_test_city.GetPreviousOwner=function() return -1 end
city_test_city.GetOriginalOwner=function() return 3 end
TaskTypes.TASK_RAZE=10; TaskTypes.TASK_UNRAZE=11; TaskTypes.TASK_ANNEX_PUPPET=12; TaskTypes.TASK_CREATE_PUPPET=13
gp_remaining = {free=1,faith=1,maya=1}; gp_policy=false; gp_baktun=0
local function rows(values)
    local out={}; for _,v in ipairs(values) do out[v.ID]=v; out[v.Type]=v end
    return setmetatable(out,{__call=function(_,filter)
        local selected={}
        for _,v in ipairs(values) do local ok=true; for k,w in pairs(filter or {}) do ok=ok and v[k]==w end; if ok then selected[#selected+1]=v end end
        local i=0; return function() i=i+1; return selected[i] end
    end})
end
GameInfo.Units=rows({{ID=2,Type="UNIT_SCIENTIST",Class="UNITCLASS_SCIENTIST",Special="SPECIALUNIT_PEOPLE",Description="Scientist",Strategy="Science",Help="Discover science"},{ID=3,Type="UNIT_PROPHET",Class="UNITCLASS_PROPHET",Special="SPECIALUNIT_PEOPLE",Description="Prophet",Help="Religion"}})
GameInfo.UnitClasses=rows({{ID=0,Type="UNITCLASS_SCIENTIST",DefaultUnit="UNIT_SCIENTIST"},{ID=1,Type="UNITCLASS_PROPHET",DefaultUnit="UNIT_PROPHET"}})
GameInfo.Civilization_UnitClassOverrides=rows({})
GameInfo.PolicyBranchTypes=rows({{ID=0,Type="POLICY_BRANCH_RATIONALISM",Description="Rationalism"}})
city_test_player.GetNumFreeGreatPeople=function() return gp_remaining.free end
city_test_player.GetNumFaithGreatPeople=function() return gp_remaining.faith end
city_test_player.GetNumMayaBoosts=function() return gp_remaining.maya end
city_test_player.HasCreatedPantheon=function() return false end
city_test_player.GetCapitalCity=function() return city_test_city end
city_test_player.GetUnitBaktun=function() return gp_baktun end
city_test_player.IsFreeMayaGreatPersonChoice=function() return false end
city_test_player.IsPolicyBranchFinished=function() return gp_policy end
