-- All queries are scoped to the requesting seat; orders are validated again on each action.
local function science_off()
    return Game.IsOption(GameInfoTypes.GAMEOPTION_NO_SCIENCE)
end
local function fraction(n, cost) return cost > 0 and math.max(0, math.min(1, n / cost)) or 0 end
local function turns(p, id, rate)
    if rate <= 0 or science_off() then return nil end
    local n = p:GetResearchTurnsLeft(id, true)
    if n >= 0 and n < 2147483647 then return n end
end
-- Numeric hover kinds: 0 tech, 1 unit type, 2 building, 3 improvement, 4 policy,
-- 5 science progress, 6 city yields, 7 city religion, 8 player, 9 civilization, 10 unit action, 11 own unit,
-- 12 technology unlock (technology id and chip slot).
-- SIMPLIFIED(SIMP-345): EUI helpers are UI-context Lua, not server APIs; unsupported sections return a reason. The host must cache hover answers by seat, kind, id, city and world revision.
-- Unlock help uses the skin's text colors for original color tags and retains the original line breaks and font icons.
local function city_for(player, args)
    if args.city == nil then return nil end
    local city = Players[player]:GetCityByID(args.city)
    assert(city and city:GetOwner() == player, "city is unavailable")
    return city
end
local function tech_tip(player, id)
    -- SIMPLIFIED(SIMP-348): shared tooltip sections use a plain value ledger; conditional EUI details and progress rings await the help bridge.
    local tech = GameInfo.Technologies[id]
    if not tech then return nil end
    local p, team = Players[player], team_of(player)
    local unlocks = tech_unlocks_for(player)
    local has = team:IsHasTech(id)
    local era = GameInfo.Eras[tech.Era]
    local leads, seen = array(), {}
    for _, name in ipairs({"Technology_PrereqTechs", "Technology_ORPrereqTechs"}) do
        for link in GameInfo[name]{PrereqTech = tech.Type} do
            local next = GameInfo.Technologies[link.TechType]
            if next and not seen[next.Type] then
                seen[next.Type] = true; leads[#leads + 1] = {key = next.Type, name = L(next.Description)}
            end
        end
    end
    return {key = tech.Type, name = L(tech.Description), icon = icon(tech.IconAtlas, tech.PortraitIndex),
        portrait = icon(tech.IconAtlas, tech.PortraitIndex), era_name = L(era.Description),
        state = has and "researched" or (p:GetCurrentResearch() == id and "current" or (p:CanResearch(id) and "available" or "unavailable")),
        researched = has, progress = team:GetTeamTechs():GetResearchProgress(id), cost = p:GetResearchCost(id),
        turns = not has and turns(p, id, p:GetScience()) or nil, science_rate = p:GetScience(), leads_to = leads,
        unlocks = unlocks(tech), help = tech.Help and L(tech.Help) or ""}
end

-- Convert the original help's markup to inline text and font pictures (`markup_lines` of `_lib.lua`).
local unlock_lines = markup_lines

local function value(lines, tag, amount)
    if amount and amount ~= 0 then lines[#lines + 1] = L(tag, amount) end
end

local function unlock_unit(p, item, lines)
    if (item.Cost or 0) > 0 then value(lines, "TXT_KEY_PRODUCTION_COST", p:GetUnitProductionNeeded(item.ID)) end
    if item.Domain ~= "DOMAIN_AIR" then value(lines, "TXT_KEY_PRODUCTION_MOVEMENT", item.Moves) end
    value(lines, "TXT_KEY_PRODUCTION_RANGE", item.Range)
    value(lines, "TXT_KEY_PRODUCTION_RANGED_STRENGTH", item.RangedCombat)
    value(lines, "TXT_KEY_PRODUCTION_STRENGTH", item.Combat)
    for resource in GameInfo.Resources() do
        local count = Game.GetNumResourceRequiredForUnit(item.ID, resource.ID)
        if count > 0 then
            lines[#lines + 1] = L("TXT_KEY_PRODUCTION_RESOURCES_REQUIRED") .. " " .. count .. " "
                .. (resource.IconString or "") .. " " .. L(resource.Description)
        end
    end
    for row in GameInfo.Unit_FreePromotions{UnitType = item.Type} do
        local promotion = GameInfo.UnitPromotions[row.PromotionType]
        if promotion then
            local name, help = L(promotion.Description), L(promotion.Help)
            lines[#lines + 1] = help ~= "" and name .. ": " .. help or name
        end
    end
end

-- The no-city view in InfoTooltipInclude. Use the inspecting seat for costs and policy effects.
local building_yields = {
    {"CULTURE", "CULTURE"}, {"FAITH", "FAITH"}, {"FOOD", "FOOD"},
    {"GOLD", "GOLD_CHANGE", "GOLD"}, {"SCIENCE", "SCIENCE_CHANGE", "SCIENCE"},
    {"PRODUCTION", "PRODUCTION_CHANGE", "PRODUCTION"},
}
local building_tag = "TXT_KEY_PRODUCTION_BUILDING_"
local function unlock_building(player, item, lines)
    local p, id = Players[player], item.ID
    if (item.Cost or 0) > 0 then value(lines, "TXT_KEY_PRODUCTION_COST", p:GetBuildingProductionNeeded(id)) end
    if (item.UnlockedByLeague == true or item.UnlockedByLeague == 1)
        and not Game.IsOption(GameInfoTypes.GAMEOPTION_NO_LEAGUES) and Game.GetNumActiveLeagues() > 0 then
        local league = Game.GetActiveLeague()
        if league then
            lines[#lines + 1] = L("TXT_KEY_PEDIA_COST_LABEL") .. " "
                .. L("TXT_KEY_LEAGUE_PROJECT_COST_PER_PLAYER", league:GetProjectBuildingCostPerPlayer(id))
        end
    end
    value(lines, building_tag .. "MAINTENANCE", item.GoldMaintenance)
    value(lines, building_tag .. "HAPPINESS",
        (item.Happiness or 0) + (item.UnmoddedHappiness or 0) + p:GetExtraBuildingHappinessFromPolicies(id))
    value(lines, building_tag .. "DEFENSE", (item.Defense or 0) / 100)
    value(lines, building_tag .. "HITPOINTS", item.ExtraCityHitPoints)
    for _, spec in ipairs(building_yields) do
        local yield = GameInfo.Yields["YIELD_" .. spec[1]]
        if yield then
            local change = Game.GetBuildingYieldChange(id, yield.ID)
            if spec[1] == "SCIENCE" then
                change = change + p:GetPolicyBuildingClassYieldChange(GameInfo.BuildingClasses[item.BuildingClass].ID, yield.ID)
            end
            value(lines, building_tag .. spec[2], change)
            if spec[3] then value(lines, building_tag .. spec[3], Game.GetBuildingYieldModifier(id, yield.ID)) end
        end
    end
    local specialist = item.SpecialistType and GameInfo.Specialists[item.SpecialistType]
    if specialist then
        if (item.GreatPeopleRateChange or 0) ~= 0 then
            lines[#lines + 1] = "[ICON_GREAT_PEOPLE] " .. L(specialist.GreatPeopleTitle) .. " " .. item.GreatPeopleRateChange
        end
        if (item.SpecialistCount or 0) > 0 then
            lines[#lines + 1] = "[ICON_GREAT_PEOPLE] " .. L(specialist.Description .. "_SLOTS") .. " " .. item.SpecialistCount
        end
    end
    if (item.GreatWorkCount or 0) > 0 and item.GreatWorkSlotType then
        local slot = GameInfo.GreatWorkSlots[item.GreatWorkSlotType]
        if slot then lines[#lines + 1] = L(slot.SlotsToolTipText, item.GreatWorkCount) end
    end
    local technology = item.EnhancedYieldTech and GameInfoTypes[item.EnhancedYieldTech]
    if (item.TechEnhancedTourism or 0) > 0 and technology and team_of(player):IsHasTech(technology) then
        value(lines, building_tag .. "TOURISM", item.TechEnhancedTourism)
    end
end

local function unlock_tip(player, id, slot)
    if not slot or slot < 0 or slot ~= math.floor(slot) then return nil end
    local tech = GameInfo.Technologies[id]
    if not tech then return nil end
    local entry = tech_unlocks_for(player)(tech, true, true)[slot + 1]
    if not entry then return nil end
    local p, key, kind = Players[player], entry.key, entry.kind
    local lines, item, title = {}, nil, plain(entry.name)
    if kind == "unit" then
        item = GameInfo.Units[key]
        unlock_unit(p, item, lines)
    elseif kind == "building" or kind == "wonder" then
        item = GameInfo.Buildings[key]
        unlock_building(player, item, lines)
    elseif kind == "project" then
        item = GameInfo.Projects[key]
        value(lines, "TXT_KEY_PRODUCTION_COST", p:GetProjectProductionNeeded(item.ID))
    elseif kind == "improvement" then
        local build = GameInfo.Builds[key]
        item = build.ImprovementType and GameInfo.Improvements[build.ImprovementType] or build
        if item ~= build then
            value(lines, building_tag .. "MAINTENANCE", item.GoldMaintenance)
            for row in GameInfo.Improvement_Yields{ImprovementType = item.Type} do
                local yield = GameInfo.Yields[row.YieldType]
                if yield and row.Yield ~= 0 then
                    lines[#lines + 1] = string.format("%+d %s %s", row.Yield, yield.IconString or "", L(yield.Description))
                end
            end
            if item.Help ~= build.Help then lines[#lines + 1] = L(build.Help) end
        end
    elseif kind == "resource" then
        title = plain(L(GameInfo.Resources[key].Description))
        lines[#lines + 1] = entry.name
    else
        -- The original tech button captions already describe synthetic abilities and free promotions.
        item = GameInfo.Processes[key]
        title = item and plain(L(item.Description)) or ""
        lines[#lines + 1] = entry.name
    end
    if item then
        lines[#lines + 1] = L(item.Help)
        if kind == "unit" or kind == "project" then
            lines[#lines + 1] = L(item.Requirements)
        end
    end
    return {tech = id, slot = slot, title = title, icon = entry.icon, lines = unlock_lines(lines)}
end

return {
    data = function(player, args)
        local kind, id = args.kind or args.a or 0, args.id or args.b
        if id == nil then return {} end
        assert(id == math.floor(id) and id >= 0, "object id must be non-negative")
        if kind == 12 then return {unlock = unlock_tip(player, id, args.slot)} end
        local city = city_for(player, args)
        if kind == 0 then return {tech = tech_tip(player, id)} end
        if kind == 1 then
            local item = GameInfo.Units[id]
            if not item then return {} end
            return {unit = {key = item.Type, name = L(item.Description), icon = icon(item.IconAtlas, item.PortraitIndex),
                portrait = icon(item.IconAtlas, item.PortraitIndex), relation = "own", owner_line = "", owner_emblem = "",
                combat_class = item.CombatClass or "", strength = item.Combat, ranged_strength = item.RangedCombat,
                range = item.Range, moves_max = item.Moves, hp_max = GameDefines.MAX_HIT_POINTS,
                abilities = item.Help and L(item.Help) or "", promotions = array(), resources = array(), requires = array(), owner_colors = array(),
                cost = {city = city and city:GetName() or "", production = city and city:GetUnitProductionNeeded(id) or item.Cost,
                    gold = city and city:GetUnitPurchaseCost(id, true), faith = city and city:GetUnitFaithPurchaseCost(id, true)}}}
        end
        if kind == 2 then
            local item = GameInfo.Buildings[id]
            if not item then return {} end
            local lines = array()
            for row in GameInfo.Building_YieldChanges{BuildingType = item.Type} do
                local yield = GameInfo.Yields[row.YieldType]
                if yield and row.Yield ~= 0 then lines[#lines + 1] = {key = row.YieldType, label = L(yield.Description), value = tostring(row.Yield), yield = yield.Type} end
            end
            if (item.GoldMaintenance or 0) ~= 0 then lines[#lines + 1] = {key = "maintenance", label = "Maintenance", value = tostring(item.GoldMaintenance), yield = "gold"} end
            return {building = {key = item.Type, name = L(item.Description), icon = icon(item.IconAtlas, item.PortraitIndex),
                portrait = icon(item.IconAtlas, item.PortraitIndex), portrait_side = "left", city = city and city:GetName() or "",
                turns = city and city:GetBuildingProductionTurnsLeft(id, 0), lines = lines, future = array(), requires = array(),
                cost = {production = city and city:GetBuildingProductionNeeded(id) or item.Cost,
                    gold = city and city:GetBuildingPurchaseCost(id, true), faith = city and city:GetBuildingFaithPurchaseCost(id, true), free = false},
                help = item.Help and L(item.Help) or ""}, unavailable_reason = "Conditional building effects require the shared help catalog."}
        end
        if kind == 3 then
            local item = GameInfo.Improvements[id]
            if not item then return {} end
            local yields = array()
            for row in GameInfo.Improvement_Yields{ImprovementType = item.Type} do
                if row.Yield ~= 0 then yields[#yields + 1] = {yield = row.YieldType, value = row.Yield, fresh_water = 0} end
            end
            return {improvement = {key = item.Type, name = L(item.Description), icon = icon(item.IconAtlas, item.PortraitIndex),
                maintenance = item.GoldMaintenance or 0, yields = yields, bonuses = array(), terrain = "", resources = "", requires = ""},
                unavailable_reason = "Conditional improvement bonuses require the shared help catalog."}
        end
        if kind == 4 then
            local item = GameInfo.Policies[id]
            if not item then return {} end
            local p = Players[player]
            local branch = item.PolicyBranchType and GameInfo.PolicyBranchTypes[item.PolicyBranchType]
            local adopted, can = p:HasPolicy(id), p:CanAdoptPolicy(id)
            local requirements = array()
            for link in GameInfo.Policy_PrereqPolicies{PolicyType = item.Type} do
                local prev = GameInfo.Policies[link.PrereqPolicy]
                if prev then requirements[#requirements + 1] = {key = prev.Type, name = L(prev.Description), met = p:HasPolicy(prev.ID)} end
            end
            return {policy = {key = item.Type, name = L(item.Description), icon = icon(item.IconAtlas, item.PortraitIndex),
                branch = branch and L(branch.Description) or "", status = adopted and "adopted" or (can and "can_adopt" or "locked"),
                status_text = adopted and "Adopted" or (can and "Available" or "Policy requirements are not met."),
                cost = p:GetNextPolicyCost(), help = item.Help and L(item.Help) or "", requires = requirements}}
        end
        if kind == 5 then
            local p, lines = Players[player], array()
            local tech = GameInfo.Technologies[p:GetCurrentResearch()]
            local progress, cost = 0, 1
            if tech then progress = team_of(player):GetTeamTechs():GetResearchProgress(tech.ID); cost = p:GetResearchCost(tech.ID) end
            return {progress = {key = "science", title = "Science", subtitle = tech and L(tech.Description) or "Choose research",
                fraction = fraction(progress, cost), gain_fraction = fraction(math.max(0, p:GetScience()), cost), loss_fraction = 0,
                lines = lines, total = p:GetScience()}, unavailable_reason = "Science source breakdown is not exported by this screen."}
        end
        if kind == 6 and city then
            local out = array()
            for y in GameInfo.Yields() do
                local total = city:GetYieldRate(y.ID)
                if total ~= 0 then out[#out + 1] = {key = string.lower(y.Type:gsub("^YIELD_", "")), city = city:GetName(), title = L(y.Description),
                    subtitle = "", total = total, lines = array(), base = city:GetBaseYieldRate(y.ID), modifier_pct = 0, footer = array()} end
            end
            return {city_yields = out, unavailable_reason = "Conditional yield breakdown requires the shared help bridge."}
        end
        if kind == 7 and city then
            local religions = array()
            for rel in GameInfo.Religions() do
                local followers = city:GetNumFollowers(rel.ID)
                if followers > 0 then
                    local pressure, routes = city:GetPressurePerTurn(rel.ID)
                    religions[#religions + 1] = {key = rel.Type, name = L(rel.Description), icon = icon(rel.IconAtlas, rel.PortraitIndex),
                        followers = followers, pressure = pressure, routes = routes, majority = city:GetReligiousMajority() == rel.ID}
                end
            end
            return {city_religion = {city = city:GetName(), faith_total = city:GetFaithPerTurn(), religions = religions, beliefs = "", lines = array()},
                unavailable_reason = "Belief and source descriptions require the shared help bridge."}
        end
        if kind == 8 or kind == 9 then
            if id >= GameDefines.MAX_MAJOR_CIVS or not Players[id] or not met(player, id) then return {} end
            local other = Players[id]
            local leader, civ = GameInfo.Leaders[other:GetLeaderType()], GameInfo.Civilizations[other:GetCivilizationType()]
            if kind == 9 then return {civ_note = {leader = L(leader.Description), civ = L(civ.Description), portrait = "", emblem = "", text = ""}} end
            -- Only public identity is returned for another seat; hidden research, treasury and diplomacy need a knowledge-aware bridge.
            if id ~= player then return {unavailable_reason = "Detailed foreign-player help requires a knowledge-aware bridge."} end
            local era = GameInfo.Eras[other:GetCurrentEra()]
            local count = 0
            for tech in GameInfo.Technologies() do if team_of(player):IsHasTech(tech.ID) then count = count + 1 end end
            return {player = {player = id, met = true, human = other:IsHuman(), leader = L(leader.Description), civ = L(civ.Description),
                emblem = "", colors = array(), era = L(era.Description), techs = count, cities = other:GetNumCities(),
                population = other:GetTotalPopulation(), gold = other:GetGold(), gold_rate = other:CalculateGoldRate(),
                policies = "", wonders = array(), deals = array(), routes = array(), treaties = array(), opinion = array(),
                their_luxuries = "", their_strategic = "", you_offer = "", relations = ""},
                unavailable_reason = "Diplomatic agreements and opinion text require the shared help bridge."}
        end
        if kind == 11 then
            local unit = Players[player]:GetUnitByID(id)
            if not unit or unit:GetOwner() ~= player then return {} end
            local item = GameInfo.Units[unit:GetUnitType()]
            return {unit_short = array({{key = key_unit(player, id), name = L(item.Description), relation = "own", owner_line = "", emblem = "",
                strength = unit:GetBaseCombatStrength(), strength_base = unit:GetBaseCombatStrength(), strength_reduced = false,
                ranged_strength = item.RangedCombat, range = item.Range, air = item.Domain == "DOMAIN_AIR",
                moves = math.floor(unit:MovesLeft() / GameDefines.MOVE_DENOMINATOR), hp = GameDefines.MAX_HIT_POINTS - unit:GetDamage(),
                hp_max = GameDefines.MAX_HIT_POINTS, embarked = unit:IsEmbarked()}})}
        end
        return {unavailable_reason = "This tooltip kind requires a seat-scoped help bridge."}
    end,
    actions = {},
}
