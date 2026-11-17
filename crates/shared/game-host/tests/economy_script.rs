//! The read-only script totals its own seat's cities without active-player UI globals.
use lua_sys::Lua;

#[test]
fn economy_totals_empty_and_populated_seats_and_has_no_mutators() {
    let lua = Lua::new().unwrap();
    lua.sandbox().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    lua.exec(r#"
        Locale = { ConvertTextKey = function(s) return s end }
        GameOptionTypes = { GAMEOPTION_NO_SCIENCE = 1, GAMEOPTION_NO_RELIGION = 2, GAMEOPTION_NO_HAPPINESS = 3 }
        Game = { IsOption = function() return false end }
        GameDefines = { TRADE_ROUTE_BASE_GOLD = 100, TRADE_ROUTE_CITY_POP_GOLD_MULTIPLIER = 100 }
        YieldTypes = { YIELD_PRODUCTION = 1, YIELD_SCIENCE = 2, YIELD_GOLD = 3 }
        OrderTypes = { ORDER_TRAIN = 0, ORDER_CONSTRUCT = 1, ORDER_CREATE = 2, ORDER_MAINTAIN = 3 }
        GameInfo = {
            Civilizations = {[0] = { Type = "CIVILIZATION_TEST", ShortDescription = "Civilization", DefaultPlayerColor = "test" }},
            Leaders = {[0] = {Description = "Leader"}},
            PlayerColors = { test = {PrimaryColor = "a", SecondaryColor = "a"}},
            Colors = {a = {Red = 1, Green = 0, Blue = 0}},
            HandicapInfos = {[0] = { HappinessDefault = 9, UnitCostPercent = 100 }},
            Resources = function() return function() end end,
        }
        local function city(id, pop)
            local values = {GetID=id, GetName="City "..id, GetPopulation=pop, GetMaxHitPoints=200,
                GetFood=12, GrowthThreshold=20, FoodDifference=3, GetFoodTurnsLeft=3, GetYieldRate=4,
                GetJONSCulturePerTurnTimes100=125, GetUnhappinessFromCityForUI=250}
            return setmetatable({}, {__index = function(_, k)
                if k == "GetOrderFromQueue" then return function() return -1,-1 end end
                if k == "IsCapital" then return function() return id == 1 end end
                if k:sub(1,2) == "Is" then return function() return false end end
                return function() return values[k] or 0 end
            end})
        end
        local function seat(list)
            return setmetatable({}, {__index=function(_,k)
                if k == "Cities" then return function() local i=0; return function() i=i+1; return list[i] end end end
                if k == "IsCapitalConnectedToCity" then return function() return false end end
                return function() return 0 end
            end})
        end
        Players = {[0]=seat({city(1,2),city(2,3)}), [1]=seat({})}
    "#, "=fixture").unwrap();
    let screen: lua_sys::Table = lua
        .load(include_str!("../screens/economy.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua.exec(
        r#"
        local root = screen.data(0, {})
        assert(#root.cities == 2 and root.totals.population == 5)
        assert(root.totals.gold == 8 and root.totals.culture == 2.5)
        assert(root.finance.expense[1].tooltip.avg_cost == 0)
        assert(root.cities[1].golden_age == nil)
        assert(root.cities[1].city_key == "0:1")
        assert(next(screen.actions) == nil)
        local other = screen.data(1, {a=1})
        assert(#other.cities == 0 and other.totals.population == 0)
        assert(other.open_tab == "happiness")
    "#,
        "=assertions",
    )
    .unwrap();
}
