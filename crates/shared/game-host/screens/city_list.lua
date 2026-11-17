local function turns(n) if n >= 0 and n < 32767 then return n end end
return {
    data = function(player, args)
        local cities = array()
        for c in Players[player]:Cities() do
            local prod
            local name = L(c:GetProductionNameKey())
            if c:GetOrderQueueLength() > 0 then
                local order, id = c:GetOrderFromQueue(0)
                local kind = ({[0] = "unit", [1] = "building", [2] = "project", [4] = "process"})[order]
                local row = GameInfo[({[0] = "Units", [1] = "Buildings", [2] = "Projects", [4] = "Processes"})[order]][id]
                if order == 1 then
                    local cls = GameInfo.BuildingClasses[row.BuildingClass]
                    if cls.MaxGlobalInstances > 0 or cls.MaxTeamInstances > 0 or cls.MaxPlayerInstances > 0 then kind = "wonder" end
                end
                prod = {name = name, kind = kind, icon = icon(row.IconAtlas, row.PortraitIndex), stored = c:GetProduction(), needed = c:GetProductionNeeded(), rate = math.floor(c:GetCurrentProductionDifferenceTimes100(false, false) / 100), turns = order ~= 4 and turns(c:GetProductionTurnsLeft()) or nil}
            end
            local food = math.floor(c:FoodDifferenceTimes100() / 100)
            local state = food < 0 and "starving" or (food == 0 or c:IsFoodProduction()) and "stagnant" or "growing"
            cities[#cities + 1] = {city = c:GetID(), map_city = key_city(player, c:GetID()), name = c:GetName(), status = c:IsCapital() and "capital" or (c:IsPuppet() and "puppet" or (c:IsOccupied() and "occupied" or "none")), population = c:GetPopulation(), food_stored = c:GetFood(), food_threshold = c:GrowthThreshold(), food_net = food, growth_turns = state == "growing" and turns(c:GetFoodTurnsLeft()) or nil, growth_state = state, strength = math.floor(c:GetStrengthValue() / 100), hp_fraction = (c:GetMaxHitPoints() - c:GetDamage()) / c:GetMaxHitPoints(), can_range_strike = c:CanRangeStrikeNow(), production = prod, production_sort = name, can_choose = not c:IsPuppet(), x = c:GetX(), y = c:GetY()}
        end
        table.sort(cities, function(a, b) return a.name < b.name end)
        return {count = #cities, cities = cities}
    end,
    actions = {},
}
