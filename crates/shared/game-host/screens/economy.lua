-- All queries are scoped to the requesting seat; no player display names are exported.
local function color(key)
    local c = GameInfo.Colors[key]
    return math.floor(c.Red * 255) * 16777216 + math.floor(c.Green * 255) * 65536 + math.floor(c.Blue * 255) * 256 + 255
end

local function order(city)
    local kind, id = city:GetOrderFromQueue(0)
    local info, name
    if kind == OrderTypes.ORDER_TRAIN then info = GameInfo.Units[id]; name = "unit"
    elseif kind == OrderTypes.ORDER_CONSTRUCT then
        info = GameInfo.Buildings[id]; name = "building"
        local class = info and GameInfo.BuildingClasses[info.BuildingClass]
        if class and class.MaxGlobalInstances > 0 then name = "wonder" end
    elseif kind == OrderTypes.ORDER_CREATE then info = GameInfo.Projects[id]; name = "project"
    elseif kind == OrderTypes.ORDER_MAINTAIN then info = GameInfo.Processes[id]; name = "process" end
    if not info then return nil end
    local row = { kind = name, key = info.Type, name = L(info.Description), icon = icon(info.IconAtlas, info.PortraitIndex) }
    if name ~= "process" then
        row.stored = city:GetProduction()
        row.cost = city:GetProductionNeeded()
        row.per_turn = city:GetCurrentProductionDifference(false, false)
        row.turns = city:GetProductionTurnsLeft()
    end
    return row
end

return {
    data = function(player, args)
        local p = assert(Players[player], "player is unavailable")
        local civ = GameInfo.Civilizations[p:GetCivilizationType()]
        local leader = GameInfo.Leaders[p:GetLeaderType()]
        local colors = GameInfo.PlayerColors[civ.DefaultPlayerColor]
        local handicap = GameInfo.HandicapInfos[p:GetHandicapType()]
        local options = {
            science = not Game.IsOption(GameOptionTypes.GAMEOPTION_NO_SCIENCE),
            religion = not Game.IsOption(GameOptionTypes.GAMEOPTION_NO_RELIGION),
            happiness = not Game.IsOption(GameOptionTypes.GAMEOPTION_NO_HAPPINESS),
        }
        local cities, gold, maintenance, connections = array(), array(), array(), array()
        local local_happy, building_happy, connected_happy, unhappy = array(), array(), array(), array()
        local totals = { population = 0, food = 0, production = 0, science = 0, gold = 0, culture = 0, faith = 0 }
        local occupied_count, occupied_pop = 0, 0
        for city in p:Cities() do
            local food = city:FoodDifference()
            local damage, hp = city:GetDamage(), city:GetMaxHitPoints()
            local row = {
                id = city:GetID(), city_key = key_city(player, city:GetID()), name = city:GetName(), population = city:GetPopulation(),
                food_stored = city:GetFood(), food_threshold = city:GrowthThreshold(), food_per_turn = food,
                starving = food < 0, capital = city:IsCapital(), puppet = city:IsPuppet(),
                occupied = city:IsOccupied() and not city:IsNoOccupiedUnhappiness(),
                defense = math.floor(city:GetStrengthValue() / 100),
                production = city:GetYieldRate(YieldTypes.YIELD_PRODUCTION), production_mod_pct = city:GetProductionModifier(),
                science = city:GetYieldRate(YieldTypes.YIELD_SCIENCE), gold = city:GetYieldRate(YieldTypes.YIELD_GOLD),
                culture = city:GetJONSCulturePerTurnTimes100() / 100, faith = city:GetFaithPerTurn(), order = order(city),
            }
            -- SIMPLIFIED(SIMP-370): The bridge has no city golden-age yield getter; nullable values stay unknown.
            if food > 0 then row.growth_turns = city:GetFoodTurnsLeft() end
            if damage > 0 then row.health_pct = math.floor(100 * (hp - damage) / hp) end
            cities[#cities + 1] = row
            for _, key in ipairs({"population", "production", "science", "gold", "culture", "faith"}) do totals[key] = totals[key] + row[key] end
            totals.food = totals.food + food
            gold[#gold + 1] = { id = row.id, name = row.name, value = row.gold }
            maintenance[#maintenance + 1] = { id = row.id, name = row.name, value = city:GetTotalBaseBuildingMaintenance() }
            if p:IsCapitalConnectedToCity(city) and not row.capital then
                connections[#connections + 1] = { id = row.id, name = row.name, value = p:GetCityConnectionRouteGoldTimes100(city) / 100 }
                connected_happy[#connected_happy + 1] = { id = row.id, name = row.name, value = p:GetHappinessPerTradeRoute() }
            end
            local_happy[#local_happy + 1] = { id = row.id, name = row.name, value = city:GetLocalHappiness() }
            building_happy[#building_happy + 1] = { id = row.id, name = row.name, value = city:GetHappinessFromBuildings() }
            unhappy[#unhappy + 1] = { id = row.id, name = row.name, value = p:GetUnhappinessFromCityForUI(city) / 100, occupied = row.occupied }
            if row.occupied then occupied_count = occupied_count + 1; occupied_pop = occupied_pop + row.population end
        end
        table.sort(cities, function(a, b) return a.name < b.name end)
        local resources = { available = array(), imported = array(), exported = array(), ["local"] = array() }
        local luxuries, luxury_base, luxury_count = array(), 0, 0
        for r in GameInfo.Resources() do
            if r.ResourceUsage ~= 0 then
                local counts = { available = p:GetNumResourceAvailable(r.ID, true), imported = p:GetResourceImport(r.ID), exported = p:GetResourceExport(r.ID), ["local"] = p:GetNumResourceTotal(r.ID, false) }
                for group, count in pairs(counts) do
                    if count > 0 then resources[group][#resources[group] + 1] = { key = r.Type, name = L(r.Description), icon = "resource:" .. r.Type, count = count } end
                end
                if r.ResourceUsage == 2 and p:GetNumResourceAvailable(r.ID, true) > 0 then
                    local value = p:GetHappinessFromLuxury(r.ID)
                    if value > 0 then
                        value = value - p:GetExtraHappinessPerLuxury()
                        luxury_base, luxury_count = luxury_base + value, luxury_count + 1
                    end
                    luxuries[#luxuries + 1] = { key = r.Type, name = L(r.Description), icon = "resource:" .. r.Type, value = value }
                end
            end
        end
        local happy = array()
        local function h(key, value, items)
            for _, item in ipairs(items or {}) do item.entry_key = item.key or tostring(item.id) end
            happy[#happy + 1] = { key = key, value = value, items = items or array() }
        end
        h("luxuries", luxury_base, luxuries)
        h("variety", p:GetHappinessFromResourceVariety())
        local extra = p:GetExtraHappinessPerLuxury() * luxury_count
        h("extra_per_luxury", extra)
        h("other_luxury", p:GetHappinessFromResources() - luxury_base - extra - p:GetHappinessFromResourceVariety())
        h("local", p:GetHappinessFromCities(), local_happy)
        h("buildings", p:GetHappinessFromBuildings(), building_happy)
        h("connections", p:GetHappinessFromTradeRoutes(), connected_happy)
        h("city_states", p:GetHappinessFromMinorCivs())
        h("policies", p:GetHappinessFromPolicies())
        h("religion", p:GetHappinessFromReligion())
        h("natural_wonders", p:GetHappinessFromNaturalWonders())
        h("free_per_city", p:GetExtraHappinessPerCity() * #cities)
        h("world_congress", p:GetHappinessFromLeagues())
        h("difficulty", handicap.HappinessDefault)
        local diplo, cost = p:GetGoldPerTurnFromDiplomacy(), p:CalculateUnitCost()
        local free = p:GetNumMaintenanceFreeUnits()
        local paid = math.max(0, p:GetNumUnits() - free)
        local finance = {
            treasury = p:GetGold(), net = p:CalculateGoldRate(), gross = p:CalculateGrossGold(), expenses = p:CalculateInflatedCosts(),
            income = array({
                { key = "cities", value = p:GetGoldFromCitiesTimes100() / 100, items = gold },
                { key = "diplomacy", value = math.max(0, diplo), items = array() },
                { key = "religion", value = p:GetGoldPerTurnFromReligion(), items = array() },
                { key = "connections", value = p:GetCityConnectionGoldTimes100() / 100, items = connections,
                  tooltip = { base = GameDefines.TRADE_ROUTE_BASE_GOLD / 100, per_citizen = GameDefines.TRADE_ROUTE_CITY_POP_GOLD_MULTIPLIER / 100, player_mod_pct = p:GetCityConnectionTradeRouteGoldModifier() } },
            }),
            expense = array({
                { key = "units", value = cost, items = array(), tooltip = { paid = paid, free = free, avg_cost = paid > 0 and cost / paid or 0, difficulty_mod_pct = handicap.UnitCostPercent - 100 } },
                { key = "buildings", value = p:GetBuildingGoldMaintenance(), items = maintenance },
                { key = "improvements", value = p:GetImprovementGoldMaintenance(), items = array() },
                { key = "diplomacy", value = math.max(0, -diplo), items = array() },
            }),
        }
        local loss = p:GetScienceFromBudgetDeficitTimes100() / 100
        if loss < 0 then finance.science_loss = math.floor(loss) end
        return {
            player = { name = L(leader.Description), civ_name = L(civ.ShortDescription), player = player, icon = "civ:" .. civ.Type, color_bg = color(colors.PrimaryColor), color_fg = color(colors.SecondaryColor) },
            options = options, open_tab = args.a == 1 and "happiness" or "general", finance = finance, cities = cities, totals = totals,
            happiness = { enabled = options.happiness, total = p:GetHappiness(), items = happy },
            unhappiness = {
                total = p:GetUnhappiness(), cities = { count = #cities - occupied_count, value = p:GetUnhappinessFromCityCount() / 100 },
                occupied_cities = { count = occupied_count, value = p:GetUnhappinessFromCapturedCityCount() / 100 },
                population = { count = totals.population - occupied_pop, value = p:GetUnhappinessFromCityPopulation() / 100 },
                occupied_population = { count = occupied_pop, value = p:GetUnhappinessFromOccupiedCities() / 100 },
                public_opinion = p:GetUnhappinessFromPublicOpinion(), by_city = unhappy,
            }, resources = resources,
        }
    end,
    actions = {},
}
