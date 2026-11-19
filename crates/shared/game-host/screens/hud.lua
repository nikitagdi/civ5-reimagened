local function own_city(player, id)
    if id == nil then return nil end
    local city = Players[player]:GetCityByID(id)
    assert(city and city:GetOwner() == player, "city is unavailable")
    return city
end

local function own_unit(player, id)
    if id == nil then return nil end
    local unit = Players[player]:GetUnitByID(id)
    assert(unit and unit:GetOwner() == player, "unit is unavailable")
    return unit
end

local function turns(value)
    if value >= 0 and value < 32767 then return value end
end

-- ---------------------------------------------------------------- top strip help
-- The original top panel (Lekmod's TopPanel.lua, basic help on, its default) writes each tooltip as text lines built from
-- the game's own text keys. The same lines are built here and handed over as rows: the text without markup, the tone of
-- its colour (positive, negative or plain) and whether it is a bullet. Resource icons read as the resource's name.
-- SIMPLIFIED(SIMP-444): A row has one tone (a number coloured inside an otherwise plain line, such as an unconnected
-- resource count, reads plain), icons other than resources are dropped, the player's basic-help option is taken as on
-- (its default), and the science tooltip's progress ring and tech portrait are not drawn.
local TONES = {
    COLOR_POSITIVE_TEXT = "positive", COLOR_GREEN = "positive", ["COLOR:150:255:150:255"] = "positive",
    COLOR_WARNING_TEXT = "negative", COLOR_NEGATIVE_TEXT = "negative", COLOR_RED = "negative",
    ["COLOR:255:150:150:255"] = "negative", ["COLOR:255:60:60:255"] = "negative",
}
local function upper(s) return Locale.ToUpper and Locale.ToUpper(s) or s:upper() end
local resource_names
local function rows(lines)
    if not resource_names then
        resource_names = {}
        for r in GameInfo.Resources() do
            local tag = r.IconString and r.IconString:match("^%[(.-)%]$")
            if tag then resource_names[tag] = L(r.Description) end
        end
    end
    local out, tone = array(), "plain"
    local function row(line)
        local parts, first, bullet, pos = {}, nil, false, 1
        while pos <= #line do
            local s, e, tag = line:find("%[([^%]]-)%]", pos)
            local chunk = line:sub(pos, s and s - 1 or #line)
            if not first and chunk:find("%S") then first = tone end
            parts[#parts + 1] = chunk
            if not s then break end
            if tag == "ENDCOLOR" then tone = "plain"
            elseif tag:find("^COLOR") then tone = TONES[tag] or "plain"
            elseif tag == "ICON_BULLET" then bullet = true
            elseif resource_names[tag] then
                -- The icon reads as the resource's name, unless the name follows it already.
                local name, rest = resource_names[tag], line:sub(e + 1):gsub("^%s+", "")
                if rest:sub(1, #name) ~= name and rest:sub(1, #name) ~= upper(name) then
                    first = first or tone
                    parts[#parts + 1] = " " .. name .. " "
                end
            end
            pos = e + 1
        end
        local text = table.concat(parts):gsub("%s+", " "):gsub("^ ", ""):gsub(" $", "")
        out[#out + 1] = {key = tostring(#out + 1), text = text, tone = first or tone, bullet = bullet}
    end
    for _, line in ipairs(lines) do
        -- A line of the original is one or more lines on screen.
        local pos = 1
        while true do
            local s, e = line:find("[NEWLINE]", pos, true)
            row(line:sub(pos, s and s - 1 or #line))
            if not s then break end
            pos = e + 1
        end
    end
    return out
end

local function signed(x) return string.format("%+g", x) end
-- A player getter of the expansion rules, or 0 where these rules have none.
local function num(p, name, ...)
    local f = p[name]
    return f and f(p, ...) or 0
end
-- TopPanel's line list with the EUI table helpers it uses.
local function text_lines()
    local t = {}
    return t, {
        add = function(s) t[#t + 1] = s end,
        nz = function(key, v, ...) if v ~= 0 then t[#t + 1] = L(key, v, ...) end end,
        bullet_nz = function(key, v, ...) if v ~= 0 then t[#t + 1] = "[ICON_BULLET]" .. L(key, v, ...) end end,
        indent_nz = function(prefix, key, v, ...) if v ~= 0 then t[#t + 1] = prefix .. L(key, v, ...) end end,
    }
end
local function anarchy(p, b)
    if p:IsAnarchy() then b.add(L("TXT_KEY_TP_ANARCHY", p:GetAnarchyNumTurns())); b.add("") end
end
local function met_majors(player)
    local team, out = team_of(player), {}
    for id = 0, GameDefines.MAX_MAJOR_CIVS - 1 do
        local other = Players[id]
        if id ~= player and other and other:IsAlive() and team:IsHasMet(other:GetTeam()) then out[#out + 1] = other end
    end
    return out
end
-- The seat's current deals: (partner or giver, item, final turn) for every item (`GcUiGetCurrentDeals`, the deal list
-- `UI.LoadCurrentDeal` walks).
local function deal_items(player)
    local read, out = rawget(_G, "GcUiGetCurrentDeals"), {}
    if not read then return out end
    for _, deal in ipairs(read(player)) do
        local other = deal.from == player and deal.to or deal.from
        for _, item in ipairs(deal.items) do
            out[#out + 1] = {other = other, item = item, final = deal.start_turn + item.duration}
        end
    end
    return out
end
-- EUI's CityPlots: the plots the city bought or was given, ring by ring out to the acquire distance.
local function city_plots(city)
    local out, owner, id = {}, city:GetOwner(), city:GetID()
    local m, p = 0, GameDefines.MAXIMUM_ACQUIRE_PLOT_DISTANCE * 6
    local n = (GameDefines.MAXIMUM_ACQUIRE_PLOT_DISTANCE + 1) * GameDefines.MAXIMUM_ACQUIRE_PLOT_DISTANCE * 3
    repeat
        local found = false
        for i = m, n do
            local plot = city:GetCityIndexPlot(i)
            if plot and plot:GetOwner() == owner and plot:GetCityPurchaseID() == id then out[#out + 1] = plot; found = true end
        end
        if not found then break end
        m, p = n + 1, p + 6
        n = n + p
    until false
    return out
end
local function luxuries()
    local list = {}
    for r in GameInfo.Resources() do
        if Game.GetResourceUsageType(r.ID) == 2 then list[#list + 1] = r end
    end
    table.sort(list, function(a, b) return L(a.Description) < L(b.Description) end)
    return list
end
local function colorize_abs(x)
    if x > 0 then return "[COLOR_POSITIVE_TEXT]" .. x .. "[ENDCOLOR]" end
    if x < 0 then return "[COLOR_WARNING_TEXT]" .. -x .. "[ENDCOLOR]" end
    return "0"
end
local function colorize(x)
    if x > 0 then return "[COLOR_POSITIVE_TEXT]" .. x .. "[ENDCOLOR]" end
    if x < 0 then return "[COLOR_WARNING_TEXT]" .. x .. "[ENDCOLOR]" end
    return "0"
end
local function scratch_deal()
    local f = rawget(_G, "GcUiGetScratchDeal")
    return f and f()
end

local function science_text(player, p)
    local t, b = text_lines()
    if Game.IsOption(GameOptionTypes.GAMEOPTION_NO_SCIENCE) then
        b.add(L"TXT_KEY_TOP_PANEL_SCIENCE_OFF" .. ": " .. L"TXT_KEY_TOP_PANEL_SCIENCE_OFF_TOOLTIP")
        return t
    end
    local team = team_of(player)
    local rate = num(p, "GetScienceTimes100")
    local tech_id, recent = p:GetCurrentResearch(), team:GetTeamTechs():GetLastTechAcquired()
    anarchy(p, b)
    if tech_id ~= -1 then
        local left, tech = p:GetResearchTurnsLeft(tech_id, true), GameInfo.Technologies[tech_id]
        local cost, progress = p:GetResearchCost(tech_id), p:GetResearchProgress(tech_id)
        local line = progress .. "[ICON_RESEARCH]"
        if tech then
            line = L("TXT_KEY_PROGRESS_TOWARDS", "[COLOR:33:190:247:255]" .. upper(L(tech.Description)) .. "[ENDCOLOR]") .. " " .. line .. "/ " .. cost .. "[ICON_RESEARCH]"
        end
        b.add(line)
        if rate > 0 then
            local overflow = rate * left + (progress - cost) * 100
            line = "[COLOR:33:190:247:255]" .. upper(L("TXT_KEY_STR_TURNS", left)) .. "[ENDCOLOR] " .. signed(overflow / 100) .. "[ICON_RESEARCH]"
            if left > 1 then line = L("TXT_KEY_STR_TURNS", left - 1) .. " " .. signed((overflow - rate) / 100) .. "[ICON_RESEARCH]  " .. line end
            b.add(line)
        end
    elseif recent ~= -1 then
        local tech, line = GameInfo.Technologies[recent], L"TXT_KEY_NOTIFICATION_SUMMARY_NEW_RESEARCH"
        if tech then line = L"TXT_KEY_RESEARCH_FINISHED" .. " [COLOR:33:190:247:255]" .. upper(L(tech.Description)) .. "[ENDCOLOR], " .. line end
        b.add(line)
    end
    b.add("[COLOR:33:190:247:255]")
    b.add(signed(rate / 100) .. "[ENDCOLOR] " .. L"TXT_KEY_REPLAY_DATA_SCIENCEPERTURN")
    b.nz("TXT_KEY_TP_SCIENCE_FROM_BUDGET_DEFICIT", num(p, "GetScienceFromBudgetDeficitTimes100") / 100)
    local cities, all = num(p, "GetScienceFromCitiesTimes100", true), num(p, "GetScienceFromCitiesTimes100", false)
    b.nz("TXT_KEY_TP_SCIENCE_FROM_CITIES", cities / 100)
    b.nz("TXT_KEY_TP_SCIENCE_FROM_ITR", (all - cities) / 100)
    b.nz("TXT_KEY_TP_SCIENCE_FROM_MINORS", num(p, "GetScienceFromOtherPlayersTimes100") / 100)
    b.nz("TXT_KEY_TP_SCIENCE_FROM_HAPPINESS", num(p, "GetScienceFromHappinessTimes100") / 100)
    b.nz("TXT_KEY_TP_SCIENCE_FROM_RESEARCH_AGREEMENTS", num(p, "GetScienceFromResearchAgreementsTimes100") / 100)
    -- The research agreements: who has one with the seat (and its turns left), who has none.
    local counters, turn = {}, Game.GetGameTurn() - 1
    for _, d in ipairs(deal_items(player)) do
        if d.item.type == TradeableItems.TRADE_ITEM_RESEARCH_AGREEMENT and d.item.from ~= player then
            counters[d.item.from] = d.final - turn
        end
    end
    local before = #t
    for _, other in ipairs(met_majors(player)) do
        if team:IsHasResearchAgreement(other:GetTeam()) then
            local line = "[ICON_BULLET][COLOR_POSITIVE_TEXT]" .. other:GetName() .. "[ENDCOLOR]"
            if counters[other:GetID()] then line = line .. " [COLOR:33:190:247:255]" .. upper(L("TXT_KEY_STR_TURNS", counters[other:GetID()])) .. "[ENDCOLOR]" end
            b.add(line)
        else
            b.add("[ICON_BULLET][COLOR_WARNING_TEXT]" .. other:GetName() .. "[ENDCOLOR]")
        end
    end
    if #t > before then
        table.insert(t, before + 1, "")
        table.insert(t, before + 2, L"TXT_KEY_DO_RESEARCH_AGREEMENT")
    end
    b.add("")
    b.add(L("TXT_KEY_TP_TECH_CITY_COST", Game.GetNumCitiesTechCostMod()))
    return t
end

local function gold_text(player, p)
    local t, b = text_lines()
    local diplomacy = p:GetGoldPerTurnFromDiplomacy()
    local from_others, to_others = math.max(0, diplomacy) * 100, -math.min(0, diplomacy)
    local religion = num(p, "GetGoldPerTurnFromReligion") * 100
    local connections = p:GetCityConnectionGoldTimes100()
    local all_cities = p:GetGoldFromCitiesTimes100()
    local cities = num(p, "GetGoldFromCitiesMinusTradeRoutesTimes100")
    local trade = all_cities - cities
    local traits = num(p, "GetGoldPerTurnFromTraits") * 100
    local units, supply = p:CalculateUnitCost(), p:CalculateUnitSupply()
    local buildings, improvements = p:GetBuildingGoldMaintenance(), p:GetImprovementGoldMaintenance()
    local vassals = num(p, "GetVassalGoldMaintenance")
    anarchy(p, b)
    local income = cities + from_others + connections + religion + trade + traits
    b.add(L("TXT_KEY_TP_AVAILABLE_GOLD", p:GetGold()))
    local expenses = units + supply + buildings + improvements + to_others + vassals
    b.add("")
    b.add(string.format("[COLOR_YELLOW]%+g[ENDCOLOR] ", num(p, "CalculateGoldRateTimes100") / 100) .. L"TXT_KEY_REPLAY_DATA_GOLDPERTURN")
    b.nz("TXT_KEY_TP_SCIENCE_FROM_BUDGET_DEFICIT", num(p, "GetScienceFromBudgetDeficitTimes100") / 100)
    b.add("[COLOR_WHITE]")
    b.add(L("TXT_KEY_TP_TOTAL_INCOME", income / 100))
    b.bullet_nz("TXT_KEY_TP_CITY_OUTPUT", cities / 100)
    b.bullet_nz("TXT_KEY_TP_GOLD_FROM_CITY_CONNECTIONS", connections / 100)
    b.bullet_nz("TXT_KEY_TP_GOLD_FROM_ITR", trade / 100)
    b.bullet_nz("TXT_KEY_TP_GOLD_FROM_TRAITS", traits / 100)
    b.bullet_nz("TXT_KEY_TP_GOLD_FROM_OTHERS", from_others / 100)
    b.bullet_nz("TXT_KEY_TP_GOLD_FROM_RELIGION", religion / 100)
    b.add("[ENDCOLOR]")
    b.add("[COLOR:255:150:150:255]" .. L("TXT_KEY_TP_TOTAL_EXPENSES", expenses))
    b.bullet_nz("TXT_KEY_TP_UNIT_MAINT", units)
    b.bullet_nz("TXT_KEY_TP_GOLD_UNIT_SUPPLY", supply)
    b.bullet_nz("TXT_KEY_TP_GOLD_BUILDING_MAINT", buildings)
    b.bullet_nz("TXT_KEY_TP_GOLD_TILE_MAINT", improvements)
    b.bullet_nz("TXT_KEY_TP_GOLD_VASSAL_MAINT", vassals)
    b.bullet_nz("TXT_KEY_TP_GOLD_TO_OTHERS", to_others)
    b.add("[ENDCOLOR]")
    -- The gold the met civilizations could give in a deal, and their rate.
    local deal, before = scratch_deal(), #t
    for _, other in ipairs(met_majors(player)) do
        local available = deal and deal:GetGoldAvailable(other:GetID(), -1) or other:GetGold()
        b.add("[ICON_BULLET]" .. other:GetName() .. string.format("  %i%s(%+i)", math.floor(available), "[ICON_GOLD]", math.floor(other:CalculateGoldRate())))
    end
    if #t > before then
        table.insert(t, before + 1, "")
        table.insert(t, before + 2, L"TXT_KEY_EO_RESOURCES_AVAILBLE")
    end
    b.add("")
    b.add(L"TXT_KEY_TP_GOLD_EXPLANATION")
    return t
end

local function happiness_text(player, p)
    local t, b = text_lines()
    if Game.IsOption(GameOptionTypes.GAMEOPTION_NO_HAPPINESS) then
        b.add(L"TXT_KEY_TOP_PANEL_HAPPINESS_OFF_TOOLTIP")
        return t
    end
    local team_id = p:GetTeam()
    local excess = p:GetExcessHappiness()
    if not p:IsEmpireUnhappy() then b.add(L("TXT_KEY_TP_TOTAL_HAPPINESS", excess))
    elseif p:IsEmpireVeryUnhappy() then b.add(L("TXT_KEY_TP_TOTAL_UNHAPPINESS", "[ICON_HAPPINESS_4]", -excess))
    else b.add(L("TXT_KEY_TP_TOTAL_UNHAPPINESS", "[ICON_HAPPINESS_3]", -excess)) end
    local policies, resources = p:GetHappinessFromPolicies(), p:GetHappinessFromResources()
    local variety, per_luxury = num(p, "GetHappinessFromResourceVariety"), num(p, "GetExtraHappinessPerLuxury")
    local buildings, cities = p:GetHappinessFromBuildings(), num(p, "GetHappinessFromCities")
    local minors, religion = p:GetHappinessFromMinorCivs(), p:GetHappinessFromReligion()
    local routes, wonders = num(p, "GetHappinessFromTradeRoutes"), p:GetHappinessFromNaturalWonders()
    local per_city = num(p, "GetExtraHappinessPerCity") * p:GetNumCities()
    local leagues, total, vassals = p:GetHappinessFromLeagues(), p:GetHappiness(), num(p, "GetHappinessFromVassals")
    local handicap = total - policies - resources - cities - buildings - minors - routes - religion - wonders - per_city - leagues - vassals
    if p:IsEmpireVeryUnhappy() then
        b.add("[COLOR:255:60:60:255]" .. (p:IsEmpireSuperUnhappy() and L"TXT_KEY_TP_EMPIRE_SUPER_UNHAPPY" or L"TXT_KEY_TP_EMPIRE_VERY_UNHAPPY") .. "[ENDCOLOR]")
    elseif p:IsEmpireUnhappy() then
        b.add("[COLOR:255:60:60:255]" .. L"TXT_KEY_TP_EMPIRE_UNHAPPY" .. "[ENDCOLOR]")
    end
    b.add(L"TXT_KEY_TP_HAPPINESS_EXPLANATION")
    b.add("")
    local lux = luxuries()
    local base, count, available = 0, 0, ""
    for _, r in ipairs(lux) do
        local n = p:GetNumResourceAvailable(r.ID, true)
        if n > 0 then
            local h = num(p, "GetHappinessFromLuxury", r.ID)
            if h > 0 then
                available = available .. " [COLOR_POSITIVE_TEXT]" .. n .. "[ENDCOLOR]" .. r.IconString
                count, base = count + 1, base + h
            end
        end
    end
    local puppets, specialists = p:GetUnhappinessFromPuppetCityPopulation(), p:GetUnhappinessFromCitySpecialists()
    local population = p:GetUnhappinessFromCityPopulation() - specialists - puppets
    b.add("[COLOR:255:150:150:255]" .. L("TXT_KEY_TP_UNHAPPINESS_TOTAL", p:GetUnhappiness()))
    b.bullet_nz("TXT_KEY_TP_UNHAPPINESS_CITY_COUNT", p:GetUnhappinessFromCityCount() / 100)
    b.bullet_nz("TXT_KEY_TP_UNHAPPINESS_CAPTURED_CITY_COUNT", p:GetUnhappinessFromCapturedCityCount() / 100)
    b.bullet_nz("TXT_KEY_TP_UNHAPPINESS_POPULATION", population / 100)
    b.bullet_nz("TXT_KEY_TP_UNHAPPINESS_PUPPET_CITIES", puppets / 100)
    b.bullet_nz("TXT_KEY_TP_UNHAPPINESS_SPECIALISTS", specialists / 100)
    b.bullet_nz("TXT_KEY_TP_UNHAPPINESS_OCCUPIED_POPULATION", p:GetUnhappinessFromOccupiedCities() / 100)
    b.bullet_nz("TXT_KEY_TP_UNHAPPINESS_UNITS", p:GetUnhappinessFromUnits() / 100)
    b.bullet_nz("TXT_KEY_TP_HAPPINESS_POLICIES", math.min(policies, 0))
    b.bullet_nz("TXT_KEY_TP_UNHAPPINESS_PUBLIC_OPINION", num(p, "GetUnhappinessFromPublicOpinion"))
    b.add("[ENDCOLOR][COLOR:150:255:150:255]")
    b.add(L("TXT_KEY_TP_HAPPINESS_SOURCES", total))
    b.bullet_nz("TXT_KEY_TP_HAPPINESS_DIFFICULTY_LEVEL", handicap)
    b.bullet_nz("TXT_KEY_TP_HAPPINESS_POLICIES", math.max(policies, 0))
    b.bullet_nz("TXT_KEY_TP_HAPPINESS_BUILDINGS", buildings)
    b.bullet_nz("TXT_KEY_TP_HAPPINESS_CITIES", cities)
    b.bullet_nz("TXT_KEY_TP_HAPPINESS_CONNECTED_CITIES", routes)
    b.bullet_nz("TXT_KEY_TP_HAPPINESS_STATE_RELIGION", religion)
    b.bullet_nz("TXT_KEY_TP_HAPPINESS_NATURAL_WONDERS", wonders)
    b.bullet_nz("TXT_KEY_TP_HAPPINESS_CITY_COUNT", per_city)
    b.bullet_nz("TXT_KEY_TP_HAPPINESS_CITY_STATE_FRIENDSHIP", minors)
    b.bullet_nz("TXT_KEY_TP_HAPPINESS_LEAGUES", leagues)
    b.bullet_nz("TXT_KEY_TP_HAPPINESS_VASSALS", vassals)
    b.indent_nz("          ", "TXT_KEY_TP_HAPPINESS_RESOURCE_VARIETY", variety)
    b.indent_nz("          ", "TXT_KEY_TP_HAPPINESS_EXTRA_PER_RESOURCE", per_luxury, count)
    b.indent_nz("          ", "TXT_KEY_TP_HAPPINESS_OTHER_SOURCES", resources - base - variety - per_luxury * count)
    if #available > 0 then
        b.add("[ICON_BULLET]" .. L("TXT_KEY_TP_HAPPINESS_FROM_RESOURCES", resources))
        b.add("  " .. available)
    end
    b.add("[ENDCOLOR]")
    -- Local luxuries, then city by city (connected, and not yet connected in the warning colour).
    local line = ""
    for _, r in ipairs(lux) do
        local n = p:GetNumResourceTotal(r.ID, false) + p:GetResourceExport(r.ID)
        if n > 0 then line = line .. " " .. colorize_abs(n) .. r.IconString end
    end
    b.add(L"TXT_KEY_EO_LOCAL_RESOURCES" .. (#line > 0 and line or (" : " .. L"TXT_KEY_TP_NO_RESOURCES_DISCOVERED")))
    for city in p:Cities() do
        local connected, unconnected = {}, {}
        for _, plot in ipairs(city_plots(city)) do
            local r, n = plot:GetResourceType(team_id), plot:GetNumResource()
            if n > 0 and Game.GetResourceUsageType(r) == 2 then
                if plot:IsCity() or (not plot:IsImprovementPillaged() and plot:IsResourceConnectedByImprovement(plot:GetImprovementType())) then
                    connected[r] = (connected[r] or 0) + n
                else
                    unconnected[r] = (unconnected[r] or 0) + n
                end
            end
        end
        line = ""
        for _, r in ipairs(lux) do
            if (connected[r.ID] or 0) > 0 then line = line .. " " .. colorize_abs(connected[r.ID]) .. r.IconString end
            if (unconnected[r.ID] or 0) > 0 then line = line .. " " .. colorize_abs(-unconnected[r.ID]) .. r.IconString end
        end
        if #line > 0 then b.add("[ICON_BULLET]" .. city:GetName() .. line) end
    end
    -- Luxuries traded in deals, with the turns each trade has left.
    local turn, imports, exports = Game.GetGameTurn() - 1, {}, {}
    for _, d in ipairs(deal_items(player)) do
        local it = d.item
        if it.type == TradeableItems.TRADE_ITEM_RESOURCES and Game.GetResourceUsageType(it.data1) == 2 then
            local side = it.from == player and exports or imports
            local key = it.from == player and d.other or it.from
            side[key] = side[key] or {}
            side[key][#side[key] + 1] = {resource = it.data1, n = it.data2, turns = d.final - turn}
        end
    end
    local function traded(side)
        for id = 0, GameDefines.MAX_MAJOR_CIVS - 1 do
            local line = ""
            for _, row in ipairs(side[id] or {}) do
                if row.n > 0 then line = line .. " " .. row.n .. GameInfo.Resources[row.resource].IconString .. "(" .. row.turns .. ")" end
            end
            if #line > 0 then b.add("[ICON_BULLET]" .. Players[id]:GetCivilizationShortDescription() .. line) end
        end
    end
    line = ""
    for _, r in ipairs(lux) do
        local n = p:GetResourceImport(r.ID) + p:GetResourceFromMinors(r.ID)
        if n > 0 then line = line .. " " .. colorize_abs(n) .. r.IconString end
    end
    if #line > 0 then
        b.add("")
        b.add(L"TXT_KEY_RESOURCES_IMPORTED" .. line)
        traded(imports)
        for id = GameDefines.MAX_MAJOR_CIVS, GameDefines.MAX_CIV_PLAYERS - 1 do
            local minor = Players[id]
            if minor and minor:IsAlive() and minor:GetAlly() == player then
                local from = ""
                for _, r in ipairs(lux) do
                    local n = minor:GetResourceExport(r.ID)
                    if n > 0 then from = from .. " " .. n .. r.IconString end
                end
                if #from > 0 then b.add("[ICON_BULLET]" .. minor:GetCivilizationShortDescription() .. from) end
            end
        end
    end
    line = ""
    for _, r in ipairs(lux) do
        local n = p:GetResourceExport(r.ID)
        if n > 0 then line = line .. " " .. colorize_abs(n) .. r.IconString end
    end
    if #line > 0 then
        b.add("")
        b.add(L"TXT_KEY_RESOURCES_EXPORTED" .. line)
        traded(exports)
    end
    -- What the met civilizations could trade the seat.
    b.add("")
    b.add(L"TXT_KEY_EO_RESOURCES_AVAILBLE")
    local team, deal = team_of(player), scratch_deal()
    for _, r in ipairs(lux) do
        local offers = {}
        for id = 0, GameDefines.MAX_CIV_PLAYERS - 1 do
            local other = Players[id]
            if id ~= player and other and other:IsAlive() and team:IsHasMet(other:GetTeam()) and not (other:IsMinorCiv() and other:IsAllies(player)) then
                local n = (other:IsMinorCiv() and other:GetNumResourceTotal(r.ID, false) + other:GetResourceExport(r.ID))
                    or (deal and deal:IsPossibleToTradeItem(id, player, TradeableItems.TRADE_ITEM_RESOURCES, r.ID, 1) and other:GetNumResourceAvailable(r.ID, false))
                    or 0
                if n > 0 then offers[#offers + 1] = other:GetCivilizationShortDescription() .. " " .. n .. r.IconString end
            end
        end
        if #offers > 0 then b.add("[ICON_BULLET]" .. L(r.Description) .. ": " .. table.concat(offers, ", ")) end
    end
    return t
end

local function golden_age_text(player, p)
    local t, b = text_lines()
    if Game.IsOption(GameOptionTypes.GAMEOPTION_NO_HAPPINESS) then
        b.add(L"TXT_KEY_TOP_PANEL_HAPPINESS_OFF_TOOLTIP")
        return t
    end
    local turns_left, progress, needed = p:GetGoldenAgeTurns(), p:GetGoldenAgeProgressMeter(), p:GetGoldenAgeProgressThreshold()
    if turns_left > 0 then
        b.add(upper(L(p:GetGoldenAgeTourismModifier() > 0 and "TXT_KEY_UNIQUE_GOLDEN_AGE_ANNOUNCE" or "TXT_KEY_GOLDEN_AGE_ANNOUNCE")))
        b.add(L("TXT_KEY_TP_GOLDEN_AGE_NOW", turns_left))
    end
    local excess, cities, total = p:GetExcessHappiness(), num(p, "GetGoldenAgePointsFromCities"), num(p, "GetTotalGoldenAgePointsInEmpire")
    b.add(L("TXT_KEY_PROGRESS_TOWARDS", "[COLOR_YELLOW]" .. upper(L"TXT_KEY_SPECIALISTSANDGP_GOLDENAGE_HEADING4_TITLE") .. "[ENDCOLOR]") .. " " .. progress .. " / " .. needed)
    if total > 0 then
        b.add(L"TXT_KEY_MISSION_START_GOLDENAGE" .. ": [COLOR_YELLOW]" .. upper(L("TXT_KEY_STR_TURNS", math.ceil((needed - progress) / total))) .. "[ENDCOLOR]")
    end
    if excess >= 0 then b.add(L("TXT_KEY_TP_GOLDEN_AGE_ADDITION_HAPPINESS", excess))
    else b.add("[COLOR_WARNING_TEXT]" .. L("TXT_KEY_TP_GOLDEN_AGE_LOSS_UNHAPPINESS", -excess) .. "[ENDCOLOR]") end
    if cities >= 0 then b.add(L("TXT_KEY_TP_GOLDEN_AGE_ADDITION_CITIES", cities))
    else b.add("[COLOR_WARNING_TEXT]" .. L("TXT_KEY_TP_GOLDEN_AGE_LOSS_CITIES", -cities) .. "[ENDCOLOR]") end
    b.add("")
    b.add(L(p:IsGoldenAgeCultureBonusDisabled() and "TXT_KEY_TP_GOLDEN_AGE_EFFECT_NO_CULTURE" or "TXT_KEY_TP_GOLDEN_AGE_EFFECT"))
    if turns_left > 0 and p:GetGoldenAgeTourismModifier() > 0 then
        b.add("")
        b.add(L"TXT_KEY_TP_CARNIVAL_EFFECT")
    end
    return t
end

local function culture_text(player, p)
    local t, b = text_lines()
    if Game.IsOption(GameOptionTypes.GAMEOPTION_NO_POLICIES) then
        b.add(L"TXT_KEY_TOP_PANEL_POLICIES_OFF_TOOLTIP")
        return t
    end
    local progress, rate = p:GetJONSCulture(), p:GetTotalJONSCulturePerTurn()
    local free, cities = p:GetJONSCulturePerTurnForFree(), p:GetJONSCulturePerTurnFromCities()
    local happiness, traits = p:GetJONSCulturePerTurnFromExcessHappiness(), num(p, "GetJONSCulturePerTurnFromTraits")
    local threshold, left = p:GetNextPolicyCost(), 1
    if threshold > progress then left = rate > 0 and math.ceil((threshold - progress) / rate) or "?" end
    anarchy(p, b)
    b.add(L("TXT_KEY_PROGRESS_TOWARDS", "[COLOR_MAGENTA]" .. upper(L"TXT_KEY_ADVISOR_SCREEN_SOCIAL_POLICY_DISPLAY") .. "[ENDCOLOR]")
        .. " " .. progress .. "[ICON_CULTURE]/ " .. threshold .. "[ICON_CULTURE]")
    if rate > 0 then
        local overflow = rate * left + progress - threshold
        local line = "[COLOR_MAGENTA]" .. upper(L("TXT_KEY_STR_TURNS", left)) .. "[ENDCOLOR]" .. string.format(" %+g[ICON_CULTURE]", overflow)
        if left > 1 then line = L("TXT_KEY_STR_TURNS", left - 1) .. string.format(" %+g[ICON_CULTURE]  ", overflow - rate) .. line end
        b.add(line)
    end
    b.add("")
    b.add("[COLOR_MAGENTA]" .. signed(rate) .. "[ENDCOLOR] " .. L"TXT_KEY_REPLAY_DATA_CULTUREPERTURN")
    b.nz("TXT_KEY_TP_CULTURE_FOR_FREE", free)
    b.nz("TXT_KEY_TP_CULTURE_FROM_CITIES", cities)
    b.nz("TXT_KEY_TP_CULTURE_FROM_HAPPINESS", happiness)
    b.nz("TXT_KEY_TP_CULTURE_FROM_TRAITS", traits)
    local minors, religion = p:GetJONSCulturePerTurnFromMinorCivs(), p:GetCulturePerTurnFromReligion()
    b.nz("TXT_KEY_TP_CULTURE_FROM_MINORS", minors)
    b.nz("TXT_KEY_TP_CULTURE_FROM_RELIGION", religion)
    local bonus = num(p, "GetCulturePerTurnFromBonusTurns")
    b.nz("TXT_KEY_TP_CULTURE_FROM_BONUS_TURNS", bonus, num(p, "GetCultureBonusTurns"))
    local vassals = num(p, "GetJONSCulturePerTurnFromVassals")
    b.nz("TXT_KEY_TP_CULTURE_VASSALS", vassals)
    b.nz("TXT_KEY_TP_CULTURE_FROM_GOLDEN_AGE", rate - free - cities - happiness - minors - religion - traits - bonus - vassals)
    b.add("")
    b.add(L("TXT_KEY_TP_CULTURE_CITY_COST", Game.GetNumCitiesPolicyCostMod()))
    return t
end

local function faith_text(player, p)
    local t, b = text_lines()
    if Game.IsOption(GameOptionTypes.GAMEOPTION_NO_RELIGION) then
        b.add(L"TXT_KEY_TOP_PANEL_RELIGION_OFF_TOOLTIP")
        return t
    end
    anarchy(p, b)
    b.add(L("TXT_KEY_TP_FAITH_ACCUMULATED", p:GetFaith()))
    b.add("")
    b.add("[COLOR_WHITE]" .. signed(p:GetTotalFaithPerTurn()) .. "[ENDCOLOR] " .. L"TXT_KEY_YIELD_FAITH" .. "[ICON_PEACE] " .. L"TXT_KEY_GOLD_PERTURN_HEADING4_TITLE")
    b.nz("TXT_KEY_TP_FAITH_FROM_CITIES", p:GetFaithPerTurnFromCities())
    b.nz("TXT_KEY_TP_FAITH_FROM_MINORS", p:GetFaithPerTurnFromMinorCivs())
    b.nz("TXT_KEY_TP_FAITH_FROM_RELIGION", p:GetFaithPerTurnFromReligion())
    local industrial = p:GetCurrentEra() >= GameInfoTypes.ERA_INDUSTRIAL
    if p:HasCreatedPantheon() then
        if (Game.GetNumReligionsStillToFound() > 0 or p:HasCreatedReligion()) and not industrial then
            b.nz("TXT_KEY_TP_FAITH_NEXT_PROPHET", p:GetMinimumFaithNextGreatProphet())
        end
    elseif p:CanCreatePantheon(false) then
        b.nz("TXT_KEY_TP_FAITH_NEXT_PANTHEON", Game.GetMinimumFaithNextPantheon())
    else
        b.add(L"TXT_KEY_TP_FAITH_PANTHEONS_LOCKED")
    end
    b.add("")
    b.add(L("TXT_KEY_TP_FAITH_RELIGIONS_LEFT", math.max(Game.GetNumReligionsStillToFound(), 0)))
    if industrial then
        -- From the Industrial era faith buys great people: the ones this seat could buy now.
        b.add("")
        b.add(L("TXT_KEY_TP_FAITH_NEXT_GREAT_PERSON", p:GetMinimumFaithNextGreatProphet()))
        local before, capital = #t, p:GetCapitalCity()
        -- The religion helper behind this check only reads: is there a city of the seat's religion that may buy the unit.
        local passes = p["DoesUnitPassFaithPurchaseCheck"]
        if capital then
            for unit in GameInfo.Units{Special = "SPECIALUNIT_PEOPLE"} do
                if capital:GetUnitFaithPurchaseCost(unit.ID, true) > 0 and p:IsCanPurchaseAnyCity(false, true, unit.ID, -1, YieldTypes.YIELD_FAITH)
                    and passes(p, unit.ID) then
                    b.add("[ICON_BULLET]" .. L(unit.Description))
                end
            end
        end
        if #t == before then b.add("[ICON_BULLET]" .. L"TXT_KEY_RO_YR_NO_GREAT_PEOPLE") end
    end
    return t
end

local function tourism_text(player, p)
    local t, b = text_lines()
    local works, slots = p:GetNumGreatWorks(), p:GetNumGreatWorkSlots()
    b.add(L("TXT_KEY_TOP_PANEL_TOURISM_TOOLTIP_1", works))
    b.add(L("TXT_KEY_TOP_PANEL_TOURISM_TOOLTIP_2", slots - works))
    local victory, pre = GameInfo.Victories.VICTORY_CULTURAL, rawget(_G, "PreGame")
    local on = victory and (pre and pre.IsVictory and pre.IsVictory(victory.ID) or (not pre and Game.IsVictoryValid and Game.IsVictoryValid(victory.ID)))
    if on then
        b.add("")
        b.add(L("TXT_KEY_TOP_PANEL_TOURISM_TOOLTIP_3", L("TXT_KEY_CO_VICTORY_INFLUENTIAL_OF", p:GetNumCivsInfluentialOn(), p:GetNumCivsToBeInfluentialOn())))
    end
    return t
end

-- The strategic resource tooltip (TopPanel's ResourcesToolTip): local sources city by city and from buildings, trades
-- with their turns, what uses the resource (owned and queued units and buildings, and what could use it later), and
-- what met civilizations could trade.
local function resource_text(player, p, resource, demand)
    local t, b = text_lines()
    local team, team_id, id = team_of(player), p:GetTeam(), resource.ID
    local techs = team:GetTeamTechs()
    local function has(tech) return tech and GameInfoTypes[tech] and techs:HasTech(GameInfoTypes[tech]) end
    local civ = GameInfo.Civilizations[p:GetCivilizationType()]
    local used = p:GetNumResourceUsed(id)
    if used > 0 or (has(resource.TechReveal) and has(resource.TechCityTrade)) then
        local available, export = p:GetNumResourceAvailable(id, true), p:GetResourceExport(id)
        local import = p:GetResourceImport(id) + p:GetResourceFromMinors(id)
        local own = p:GetNumResourceTotal(id, false) + export
        b.add(colorize_abs(available) .. resource.IconString .. " " .. upper(L(resource.Description)))
        b.add("----------------")
        b.add("")
        b.add(colorize(own) .. " " .. L"TXT_KEY_EO_LOCAL_RESOURCES")
        for city in p:Cities() do
            local connected, unconnected = 0, 0
            for _, plot in ipairs(city_plots(city)) do
                local n = plot:GetNumResource()
                if n > 0 and plot:GetResourceType(team_id) == id then
                    if plot:IsCity() or (not plot:IsImprovementPillaged() and plot:IsResourceConnectedByImprovement(plot:GetImprovementType())) then
                        connected = connected + n
                    else
                        unconnected = unconnected + n
                    end
                end
            end
            local line = ""
            if connected > 0 then line = " " .. colorize_abs(connected) .. resource.IconString end
            if unconnected > 0 then line = line .. " " .. colorize_abs(-unconnected) .. resource.IconString end
            if #line > 0 then b.add("[ICON_BULLET]" .. city:GetName() .. line) end
        end
        local function can_build_building(building)
            local override = GameInfo.Civilization_BuildingClassOverrides{CivilizationType = civ.Type, BuildingClassType = building.BuildingClass}()
            local ok
            if override then ok = override.BuildingType == building.Type
            else ok = GameInfo.BuildingClasses[building.BuildingClass].DefaultBuilding == building.Type end
            return ok and not ((Game.IsOption(GameOptionTypes.GAMEOPTION_NO_ESPIONAGE) and building.IsEspionage) or (building.ObsoleteTech and has(building.ObsoleteTech)))
        end
        local function building_line(building, per, existing, builds, someday)
            local total, line = (existing + builds) * per, "[COLOR_YIELD_FOOD]" .. L(building.Description) .. "[ENDCOLOR]"
            if someday then
                local tech = building.PrereqTech and GameInfo.Technologies[building.PrereqTech]
                if tech and not techs:HasTech(tech.ID) then line = line .. " [COLOR_CYAN]" .. L(tech.Description) .. "[ENDCOLOR]" end
                local branch = building.PolicyBranchType and GameInfo.PolicyBranchTypes[building.PolicyBranchType]
                if branch and not p:GetPolicyBranchChosen(branch.ID) then line = line .. " [COLOR_MAGENTA]" .. L(branch.Description) .. "[ENDCOLOR]" end
            end
            return total, line
        end
        -- Buildings that make it.
        local index = #t
        for row in GameInfo.Building_ResourceQuantity{ResourceType = resource.Type} do
            local building, per = GameInfo.Buildings[row.BuildingType], row.Quantity
            if building and per and per > 0 then
                local existing, builds = p:CountNumBuildings(building.ID), 0
                for city in p:Cities() do if city:GetProductionBuilding() == building.ID then builds = builds + 1 end end
                local someday = can_build_building(building)
                if someday or existing > 0 or builds > 0 then
                    local total, line = building_line(building, per, existing, builds, someday)
                    if total > 0 then
                        index = index + 1
                        table.insert(t, index, "[ICON_BULLET]" .. total .. resource.IconString .. " = " .. existing .. " (+" .. builds .. ") " .. line)
                    else
                        b.add("[ICON_BULLET] (" .. per .. "/" .. line .. ")")
                    end
                end
            end
        end
        local gameturn, imports, exports = Game.GetGameTurn() - 1, {}, {}
        for _, d in ipairs(deal_items(player)) do
            local it = d.item
            if it.type == TradeableItems.TRADE_ITEM_RESOURCES and it.data1 == id and it.data2 then
                local side = it.from == player and exports or imports
                local key = it.from == player and d.other or it.from
                side[key] = side[key] or {}
                side[key][#side[key] + 1] = {n = it.data2, turns = d.final - gameturn}
            end
        end
        local function traded(side)
            for pid = 0, GameDefines.MAX_MAJOR_CIVS - 1 do
                local line = ""
                for _, row in ipairs(side[pid] or {}) do
                    if row.n > 0 then line = line .. " " .. row.n .. resource.IconString .. "(" .. row.turns .. ")" end
                end
                if #line > 0 then b.add("[ICON_BULLET]" .. Players[pid]:GetCivilizationShortDescription() .. line) end
            end
        end
        if import > 0 then
            b.add("")
            b.add(colorize(import) .. " " .. L"TXT_KEY_RESOURCES_IMPORTED")
            traded(imports)
            for mid = GameDefines.MAX_MAJOR_CIVS, GameDefines.MAX_CIV_PLAYERS - 1 do
                local minor = Players[mid]
                if minor and minor:IsAlive() and minor:GetAlly() == player then
                    local n = minor:GetResourceExport(id)
                    if n > 0 then b.add("[ICON_BULLET]" .. minor:GetCivilizationShortDescription() .. " " .. n .. resource.IconString) end
                end
            end
        end
        if export > 0 then
            b.add("")
            b.add(colorize(-export) .. " " .. L"TXT_KEY_RESOURCES_EXPORTED")
            traded(exports)
        end
        -- What uses it: units the seat has or queues, and the ones it could build some day.
        b.add("")
        b.add(colorize(-used) .. " " .. L"TXT_KEY_PEDIA_REQ_RESRC_LABEL")
        index = #t
        local religion_on = not Game.IsOption(GameOptionTypes.GAMEOPTION_NO_RELIGION)
        local leader = GameInfo.Leaders[p:GetLeaderType()]
        for _, need in ipairs(demand.units[id] or {}) do
            local unit, per = need.unit, need.n
            local existing, builds = demand.owned_units[unit.ID] or 0, demand.queued[0][unit.ID] or 0
            local someday = true
            for trait in GameInfo.Leader_Traits{LeaderType = leader.Type} do
                if GameInfo.Trait_NoTrain{UnitClassType = unit.Class, TraitType = trait.TraitType}() then someday = false; break end
            end
            if someday then
                local override = GameInfo.Civilization_UnitClassOverrides{CivilizationType = civ.Type, UnitClassType = unit.Class}()
                if override then someday = override.UnitType == unit.Type
                else someday = GameInfo.UnitClasses[unit.Class].DefaultUnit == unit.Type end
            end
            someday = someday and not ((Game.IsOption(GameOptionTypes.GAMEOPTION_ONE_CITY_CHALLENGE) and (unit.Found or unit.FoundAbroad))
                or (religion_on and (unit.FoundReligion or unit.SpreadReligion or unit.RemoveHeresy))
                or (unit.ObsoleteTech and has(unit.ObsoleteTech)))
            if someday or existing > 0 or builds > 0 then
                local total, line = (existing + builds) * per, "[COLOR_YELLOW]" .. L(unit.Description) .. "[ENDCOLOR]"
                if someday then
                    local tech = unit.PrereqTech and GameInfo.Technologies[unit.PrereqTech]
                    if tech and not techs:HasTech(tech.ID) then line = line .. " [COLOR_CYAN]" .. L(tech.Description) .. "[ENDCOLOR]" end
                    local policy = unit.PolicyType and GameInfo.Policies[unit.PolicyType]
                    if policy and not p:HasPolicy(policy.ID) then line = line .. " [COLOR_MAGENTA]" .. L(policy.Description) .. "[ENDCOLOR]" end
                end
                if total > 0 then
                    index = index + 1
                    table.insert(t, index, "[ICON_BULLET]" .. total .. resource.IconString .. " = " .. existing .. " (+" .. builds .. ") " .. line)
                else
                    b.add("[ICON_BULLET] (" .. per .. "/" .. line .. ")")
                end
            end
        end
        for _, need in ipairs(demand.buildings[id] or {}) do
            local building, per = need.building, need.n
            local existing, builds = p:CountNumBuildings(building.ID), demand.queued[1][building.ID] or 0
            local someday = can_build_building(building)
            if someday or existing > 0 or builds > 0 then
                local total, line = building_line(building, per, existing, builds, someday)
                if total > 0 then
                    index = index + 1
                    table.insert(t, index, "[ICON_BULLET]" .. total .. resource.IconString .. " = " .. existing .. " (+" .. builds .. ") " .. line)
                else
                    b.add("[ICON_BULLET] (" .. per .. "/" .. line .. ")")
                end
            end
        end
    end
    -- What met civilizations could trade the seat.
    local index, total, deal = #t, 0, scratch_deal()
    for pid = 0, GameDefines.MAX_CIV_PLAYERS - 1 do
        local other = Players[pid]
        if pid ~= player and other and other:IsAlive() and team:IsHasMet(other:GetTeam()) and not (other:IsMinorCiv() and other:IsAllies(player)) then
            local n = (other:IsMinorCiv() and other:GetNumResourceTotal(id, false) + other:GetResourceExport(id))
                or (deal and deal:IsPossibleToTradeItem(pid, player, TradeableItems.TRADE_ITEM_RESOURCES, id, 1) and other:GetNumResourceAvailable(id, false))
            if n and n > 0 then
                total = total + n
                b.add("[ICON_BULLET]" .. other:GetCivilizationShortDescription() .. " " .. n .. resource.IconString)
            end
        end
    end
    if total > 0 then
        table.insert(t, index + 1, "")
        table.insert(t, index + 2, "----------------")
        table.insert(t, index + 3, total .. " " .. L"TXT_KEY_EO_RESOURCES_AVAILBLE")
    end
    return t
end

-- Who needs which strategic resource, and what the seat owns and queues: shared by every resource tooltip.
local function resource_demand(p)
    local d = {units = {}, buildings = {}, owned_units = {}, queued = {[0] = {}, [1] = {}}}
    for unit in GameInfo.Units() do
        for row in GameInfo.Unit_ResourceQuantityRequirements{UnitType = unit.Type} do
            local r = GameInfo.Resources[row.ResourceType]
            local n = r and Game.GetNumResourceRequiredForUnit(unit.ID, r.ID) or 0
            if n > 0 then d.units[r.ID] = d.units[r.ID] or {}; table.insert(d.units[r.ID], {unit = unit, n = n}) end
        end
    end
    for building in GameInfo.Buildings() do
        for row in GameInfo.Building_ResourceQuantityRequirements{BuildingType = building.Type} do
            local r = GameInfo.Resources[row.ResourceType]
            local n = r and Game.GetNumResourceRequiredForBuilding(building.ID, r.ID) or 0
            if n > 0 then d.buildings[r.ID] = d.buildings[r.ID] or {}; table.insert(d.buildings[r.ID], {building = building, n = n}) end
        end
    end
    for u in p:Units() do d.owned_units[u:GetUnitType()] = (d.owned_units[u:GetUnitType()] or 0) + 1 end
    for city in p:Cities() do
        for i = 0, city:GetOrderQueueLength() - 1 do
            local order, item = city:GetOrderFromQueue(i)
            if order == 0 or order == 1 then d.queued[order][item] = (d.queued[order][item] or 0) + 1 end
        end
    end
    return d
end

local function top_tips(player, p)
    local tips = array()
    local function tip(kind, title, total, entries, text)
        local lines = array()
        for _, entry in ipairs(entries) do
            local value = p[entry[2]](p, entry[4]) / (entry[3] or 1)
            lines[#lines + 1] = {key = entry[2], label = entry[1], value = value}
        end
        tips[#tips + 1] = {kind = kind, title = title, total = total, lines = lines, text = rows(text(player, p))}
        return lines
    end
    local science = tip("science", "Science per turn", p:GetScience(), {
        {"Cities (including trade)", "GetScienceFromCitiesTimes100", 100, false},
        {"Other civilizations", "GetScienceFromOtherPlayersTimes100", 100},
        {"Happiness", "GetScienceFromHappinessTimes100", 100},
        {"Research agreements", "GetScienceFromResearchAgreementsTimes100", 100},
        {"Budget deficit", "GetScienceFromBudgetDeficitTimes100", 100},
    }, science_text)
    science[#science + 1] = {key = "overflow", label = "Research overflow", value = p:GetOverflowResearch()}
    tip("gold", "Gold per turn", p:CalculateGoldRate(), {
        {"Treasury", "GetGold"}, {"Gross income", "CalculateGrossGold"},
        {"Expenses", "CalculateInflatedCosts"}, {"Cities", "GetGoldFromCitiesTimes100", 100},
        {"City connections", "GetCityConnectionGoldTimes100", 100},
        {"Diplomacy", "GetGoldPerTurnFromDiplomacy"}, {"Religion", "GetGoldPerTurnFromReligion"},
        {"Unit maintenance", "CalculateUnitCost"}, {"Unit supply", "CalculateUnitSupply"},
        {"Buildings", "GetBuildingGoldMaintenance"}, {"Improvements", "GetImprovementGoldMaintenance"},
    }, gold_text)
    tip("happiness", "Empire happiness", p:GetExcessHappiness(), {
        {"Total happiness", "GetHappiness"}, {"Total unhappiness", "GetUnhappiness"},
        {"Policies", "GetHappinessFromPolicies"}, {"Resources", "GetHappinessFromResources"},
        {"Buildings", "GetHappinessFromBuildings"}, {"Religion", "GetHappinessFromReligion"},
        {"Natural wonders", "GetHappinessFromNaturalWonders"}, {"City-states", "GetHappinessFromMinorCivs"},
        {"World Congress", "GetHappinessFromLeagues"},
    }, happiness_text)
    tip("culture", "Culture per turn", p:GetTotalJONSCulturePerTurn(), {
        {"Stored culture", "GetJONSCulture"}, {"Next policy", "GetNextPolicyCost"},
        {"Cities", "GetJONSCulturePerTurnFromCities"}, {"Happiness", "GetJONSCulturePerTurnFromExcessHappiness"},
        {"Traits", "GetJONSCulturePerTurnFromTraits"}, {"Free culture", "GetJONSCulturePerTurnForFree"},
        {"City-states", "GetJONSCulturePerTurnFromMinorCivs"}, {"Religion", "GetCulturePerTurnFromReligion"},
    }, culture_text)
    tip("faith", "Faith per turn", p:GetTotalFaithPerTurn(), {
        {"Stored faith", "GetFaith"}, {"Cities", "GetFaithPerTurnFromCities"},
        {"City-states", "GetFaithPerTurnFromMinorCivs"}, {"Religion", "GetFaithPerTurnFromReligion"},
    }, faith_text)
    tip("golden_age", "Golden age", p:GetGoldenAgeTurns(), {
        {"Turns remaining", "GetGoldenAgeTurns"}, {"Progress", "GetGoldenAgeProgressMeter"},
        {"Threshold", "GetGoldenAgeProgressThreshold"}, {"Points from cities", "GetGoldenAgePointsFromCities"},
    }, golden_age_text)
    tip("tourism", "Tourism per turn", p:GetTourism(), {
        {"Great works", "GetNumGreatWorks"}, {"Great work slots", "GetNumGreatWorkSlots"},
    }, tourism_text)
    return tips
end

local function turn_data(player)
    local p, queue, waiting = Players[player], array(), array()
    local simultaneous = p:IsSimultaneousTurns()
    for id = 0, GameDefines.MAX_CIV_PLAYERS - 1 do
        local other = Players[id]
        if other and other:IsAlive() then
            local known = id == player or other:IsHuman() or met(player, id)
            local civ = known and GameInfo.Civilizations[other:GetCivilizationType()]
            queue[#queue + 1] = {key = "0:" .. id, team = known and other:GetTeam() or -1,
                name = civ and L(civ.ShortDescription) or "Unknown player", known = known,
                human = other:IsHuman(), active = other:IsTurnActive()}
        end
    end
    table.sort(queue, function(a, b)
        if simultaneous and a.human ~= b.human then return not a.human end
        if simultaneous and a.team ~= b.team then return a.team < b.team end
        return tonumber(a.key:match(":(%d+)$")) < tonumber(b.key:match(":(%d+)$"))
    end)
    for unit in p:Units() do
        if unit:IsReadyToMove() then waiting[#waiting + 1] = {key = key_unit(player, unit:GetID()), name = unit:GetName()} end
    end
    local notification = p:GetEndTurnBlockingNotificationIndex()
    return {blocker_kind = p:GetEndTurnBlockingType(),
        reason = notification >= 0 and p:GetNotificationStr(notification) or "",
        queue = queue, waiting_units = waiting}
end

-- The status column of the original unit list (UnitList.lua), in its order of precedence: an aircraft's base city,
-- paradrop readiness, embarked, garrisoned, automated (build or explore), heal, alert, fortified, sleep, the build
-- in progress with its turns left.
local function list_status(u, player)
    local plot, activity, build = u:GetPlot(), u:GetActivityType(), u:GetBuildType()
    local city = plot and plot:GetPlotCity()
    if u:GetDomainType() == 1 and city then return city:GetName() end
    if u:GetUnitCombatType() == 14 then return u:CanParadrop(plot) and L"TXT_KEY_CAN_PARADROP" or "" end
    if u:IsEmbarked() then return L"TXT_KEY_UNIT_STATUS_EMBARKED" end
    if u:IsGarrisoned() then return L"TXT_KEY_MISSION_GARRISON" end
    if u:IsAutomated() then return L(u:IsWork() and "TXT_KEY_ACTION_AUTOMATE_BUILD" or "TXT_KEY_ACTION_AUTOMATE_EXPLORE") end
    if activity == 3 then return L"TXT_KEY_MISSION_HEAL" end
    if activity == 4 then return L"TXT_KEY_MISSION_ALERT" end
    if u:GetFortifyTurns() > 0 then return L"TXT_KEY_UNIT_STATUS_FORTIFIED" end
    if activity == 2 then return L"TXT_KEY_MISSION_SLEEP" end
    if build ~= -1 then
        -- The seat's own build turns (the single-player original passes player 0, the active player there).
        local text, left = L(GameInfo.Builds[build].Description), plot:GetBuildTurnsLeft(build, player, 0, 0)
        if left < 4000 and left > 0 then text = text .. " (" .. left .. ")" end
        return text
    end
    return ""
end

local function list_data(player)
    local p, units, groups = Players[player], array(), array()
    local nearest
    local activities = {[0]="awake", "hold", "sleep", "heal", "sentry", "intercept", "mission"}
    for u in p:Units() do
        -- The original list's two stacks: combat units and aircraft, then the rest.
        units[#units + 1] = {key = key_unit(player, u:GetID()), id = u:GetID(), type_id = u:GetUnitType(),
            name = L(u:GetNameKey()), icon = "unit:" .. u:GetUnitType(), military = u:IsCombatUnit() or u:GetDomainType() == 1,
            status = activities[u:GetActivityType()] or "other", status_text = list_status(u, player), hp = GameDefines.MAX_HIT_POINTS - u:GetDamage(),
            moves = u:MovesLeft() / GameDefines.MOVE_DENOMINATOR, max_moves = u:MaxMoves() / GameDefines.MOVE_DENOMINATOR,
            can_promote = u:IsPromotionReady()}
    end
    for _, kind in ipairs({"ARTIST", "ENGINEER", "MERCHANT", "MUSICIAN", "SCIENTIST", "WRITER"}) do
        local specialist = GameInfo.Specialists["SPECIALIST_" .. kind]
        local cls = specialist and GameInfo.UnitClasses[specialist.GreatPeopleUnitClass]
        if cls then
            local rows = array()
            for c in p:Cities() do
                local base = specialist.GreatPeopleRateChange * c:GetSpecialistCount(specialist.ID)
                for building in GameInfo.Buildings{SpecialistType = specialist.Type} do
                    base = base + (building.GreatPeopleRateChange or 0) * c:GetNumBuilding(building.ID)
                end
                local title = kind:sub(1, 1) .. kind:sub(2):lower()
                local modifier = c:GetGreatPeopleRateModifier() + p:GetGreatPeopleRateModifier() + p["GetGreat" .. title .. "RateModifier"](p)
                if kind == "MUSICIAN" then
                    -- The policy getter includes production converted to musician rate; the ordinary player modifier already includes the flat policy rate.
                    local flat = 0
                    for policy in GameInfo.Policies() do
                        if p:HasPolicy(policy.ID) and not p:IsPolicyBlocked(policy.ID) then flat = flat + (policy.GreatMusicianRateModifier or 0) end
                    end
                    modifier = modifier + p:GetPolicyGreatMusicianRateModifier() - flat
                end
                if p:GetGoldenAgeTurns() > 0 and (kind == "ARTIST" or kind == "MUSICIAN" or kind == "WRITER") then
                    modifier = modifier + p["GetGoldenAgeGreat" .. title .. "RateModifier"](p)
                end
                local rate = math.floor(math.max(0, base * (100 + modifier))) / 100
                if rate > 0 then
                    local progress, threshold = c:GetSpecialistGreatPersonProgressTimes100(specialist.ID) / 100, c:GetSpecialistUpgradeThreshold(cls.ID)
                    rows[#rows + 1] = {city = key_city(player, c:GetID()), city_id = c:GetID(), name = c:GetName(),
                        progress = progress, threshold = threshold, per_turn = rate, turns = math.max(0, math.ceil((threshold - progress) / rate))}
                    local row = rows[#rows]
                    if not nearest or row.turns < nearest.turns then
                        local unit = GameInfo.Units[cls.DefaultUnit]
                        nearest = {city_id = c:GetID(), name = L(cls.Description), city_name = c:GetName(),
                            icon = unit and "unit:" .. unit.ID or "", turns = row.turns, progress = progress, threshold = threshold, per_turn = rate}
                    end
                end
            end
            if #rows > 0 then groups[#groups + 1] = {key = kind:lower(), name = L(cls.Description), rows = rows} end
        end
    end
    return {units = units, great_people = groups, nearest = nearest, general_xp = p:GetCombatExperience(), general_threshold = p:GreatGeneralThreshold(),
        admiral_xp = p:GetNavalCombatExperience(), admiral_threshold = p:GreatAdmiralThreshold()}
end

local function minimap_data(player)
    local p, beliefs = Players[player], array()
    local religion, pantheon
    local rid = p:GetReligionCreatedByPlayer()
    if rid > 0 then
        religion = L(Game.GetReligionName(rid))
        for _, id in ipairs(Game.GetBeliefsInReligion(rid)) do
            local belief = GameInfo.Beliefs[id]
            beliefs[#beliefs + 1] = {id = id, name = L(belief.ShortDescription)}
        end
    elseif p:HasCreatedPantheon() then
        local belief = GameInfo.Beliefs[p:GetBeliefInPantheon()]
        if belief then pantheon = L(belief.ShortDescription) end
    end
    return {observer = p:IsObserver(), religion = religion, pantheon = pantheon, beliefs = beliefs}
end

-- A localized core text without its markup (colours, line breaks, icons).
local function core_text(text)
    return (text:gsub("%[NEWLINE%]", " "):gsub("%[COLOR[^%]]*%]", ""):gsub("%[ENDCOLOR%]", ""):gsub("%[ICON_[^%]]*%]", ""):gsub("^%s+", ""):gsub("%s+$", ""))
end

local function city_data(city)
    if not city then return nil end
    local queue, reasons = array(), array()
    local getters = {
        [0] = {"GetUnitProductionTurnsLeft", "GetUnitProductionNeeded"},
        [1] = {"GetBuildingProductionTurnsLeft", "GetBuildingProductionNeeded"},
        [2] = {"GetProjectProductionTurnsLeft", "GetProjectProductionNeeded"},
        [3] = {"GetSpecialistProductionTurnsLeft"},
    }
    for slot = 0, city:GetOrderQueueLength() - 1 do
        local order, item = city:GetOrderFromQueue(slot)
        local g = getters[order]
        queue[#queue + 1] = {
            slot = slot, order = order, item = item,
            turns = g and turns(city[g[1]](city, item, slot)),
            needed = g and g[2] and city[g[2]](city, item) or 0,
        }
    end
    -- Units and buildings: the reason the core writes into the tooltip sink of canTrain / canConstruct.
    for _, kind in ipairs({
        {0, "Units", "CanTrain", "CanTrainTooltip"}, {1, "Buildings", "CanConstruct", "CanConstructTooltip"},
        {2, "Projects", "CanCreate"}, {4, "Processes", "CanMaintain"},
    }) do
        for item in GameInfo[kind[2]]() do
            if not city[kind[3]](city, item.ID, 0, 0) then
                local why = kind[4] and city[kind[4]] and city[kind[4]](city, item.ID)
                why = type(why) == "string" and core_text(why) or ""
                -- SIMPLIFIED(SIMP-324): projects and processes (and the few unit/building refusals the core does not explain) have no core reason text, so they use a general explanation.
                reasons[#reasons + 1] = {
                    id = kind[1] * 65536 + item.ID, order = kind[1], item = item.ID,
                    reason = why ~= "" and why or "Production requirements are not met in this city.",
                }
            end
        end
    end
    return {id = city:GetID(), queue = queue, overflow = city:GetOverflowProduction(), reasons = reasons}
end

local function unit_actions(unit)
    local rows = array()
    local busy = unit:IsBusy() or unit:IsDelayedDeath()
    local function add(id, applies, enabled, reason, cost)
        if applies then rows[#rows + 1] = {id = id, enabled = not busy and enabled, reason = (busy or not enabled) and reason or nil, cost = cost} end
    end
    local info = GameInfo.Units[unit:GetUnitType()]
    add(101, unit:IsCanDefend(), unit:CanStartMission(GameInfoTypes.MISSION_ALERT, -1, -1, nil, 0), "The unit cannot go on alert here.")
    add(102, info and (info.Pillage == true or info.Pillage == 1), unit:CanStartMission(GameInfoTypes.MISSION_PILLAGE, -1, -1, nil, 0), "The unit cannot pillage here.")
    local upgrade = unit:GetUpgradeUnitType()
    add(103, unit:CanUpgradeRightNow(1), unit:CanUpgradeRightNow(0), "Upgrade requirements are not met.", upgrade >= 0 and unit:UpgradePrice(upgrade) or nil)
    return rows
end

local AIR = DomainTypes and DomainTypes.DOMAIN_AIR or 1

local function unit_data(unit)
    if not unit then return nil end
    local promotions = array()
    for promotion in GameInfo.UnitPromotions() do
        if unit:IsHasPromotion(promotion.ID) and promotion.ShowInUnitPanel ~= false and promotion.ShowInUnitPanel ~= 0 then
            promotions[#promotions + 1] = {id = promotion.ID, name = L(promotion.Description),
                icon = "promotion:" .. promotion.ID, help = promotion.Help and L(promotion.Help) or ""}
        end
    end
    local can_gain_xp = unit:IsCombatUnit() or unit:GetDomainType() == AIR
    return {id = unit:GetID(), name = unit:GetName(), xp_next = can_gain_xp and unit:ExperienceNeeded() or 0,
        promotions = promotions, actions = unit_actions(unit)}
end

local function map_data(player)
    local p, cities, units = Players[player], array(), array()
    for id = 0, GameDefines.MAX_CIV_PLAYERS - 1 do
        local other = Players[id]
        if other and other:IsAlive() then
            local known = id == player or met(player, id)
            local civ = known and GameInfo.Civilizations[other:GetCivilizationType()]
            local owner = civ and L(civ.ShortDescription) or "Unknown civilization"
            for c in other:Cities() do
                if c:GetOwner() == id and (id == player or (c:IsRevealed(p:GetTeam(), false) and visible(c:Plot(), p:GetTeam()))) then
                    local own = id == player
                    local food, need, rate = 0, 0, 0
                    if own then food, need, rate = c:GetFood(), c:GrowthThreshold(), c:FoodDifference() end
                    cities[#cities + 1] = {key = key_city(id, c:GetID()), hover_key = key_city(id, c:GetID()), name = c:GetName(), owner = owner,
                        strength = math.floor(c:GetStrengthValue() / 100), hp = c:GetMaxHitPoints() - c:GetDamage(), hp_max = c:GetMaxHitPoints(),
                        -- Turns to grow only while the city grows: the core's count means nothing for a starving city.
                        growth_turns = own and rate > 0 and turns(c:GetFoodTurnsLeft()) or nil,
                        resistance_turns = own and c:IsResistance() and c:GetResistanceTurns() or nil,
                        razing_turns = own and c:IsRazing() and c:GetRazingTurns() or nil,
                        -- The banner's food bar and growth cell (the original CityBannerManager: growing, stagnant, starving).
                        food = food, food_needed = need, food_rate = rate,
                        starve_turns = rate < 0 and math.max(1, math.ceil(food / -rate)) or nil}
                end
            end
            for u in other:Units() do
                if u:GetOwner() == id and (id == player or (u:GetDomainType() ~= 1 and not u:IsCargo() and visible(u:GetPlot(), p:GetTeam()) and not u:IsInvisible(p:GetTeam(), false))) then
                    local info, promotions = GameInfo.Units[u:GetUnitType()], array()
                    for promotion in GameInfo.UnitPromotions() do
                        if u:IsHasPromotion(promotion.ID) and promotion.ShowInUnitPanel ~= false and promotion.ShowInUnitPanel ~= 0 then
                            promotions[#promotions + 1] = {id = promotion.ID, name = L(promotion.Description), icon = "promotion:" .. promotion.ID, help = promotion.Help and L(promotion.Help) or ""}
                        end
                    end
                    local shape = u:IsEmbarked() and "embarked" or (u:IsGarrisoned() and "garrison" or (u:GetFortifyTurns() > 0 and "fortified" or (u:IsTrade() and "trade" or ((info.Combat or 0) > 0 and "military" or "civilian"))))
                    units[#units + 1] = {key = key_unit(id, u:GetID()), hover_key = key_unit(id, u:GetID()), name = u:GetName(), owner = owner, shape = shape,
                        strength = u:GetBaseCombatStrength(), ranged_strength = u:GetBaseRangedCombatStrength(), range = u:Range(),
                        hp = u:GetCurrHitPoints(), hp_max = u:GetMaxHitPoints(), moves = (id == player and u:MovesLeft() or u:MaxMoves()) / GameDefines.MOVE_DENOMINATOR,
                        moves_is_max = id ~= player, cargo = u:IsCargo(), promotions = promotions}
                end
            end
        end
    end
    return {cities = cities, units = units}
end

local function action_hotkey(a)
    local n, code = a.HotKeyVal
    if n and n >= 13 and n <= 38 then code = "Key" .. string.char(65 + n - 13)
    elseif n and n >= 2 and n <= 11 then code = "Digit" .. (n - 2)
    elseif n and n >= 59 and n <= 70 then code = "F" .. (n - 58)
    elseif n and n >= 73 and n <= 82 then code = "Numpad" .. (n - 73)
    else code = ({[1]="Escape", [12]="Minus", [39]="Equal", [40]="Backspace", [41]="Tab", [42]="BracketLeft", [43]="BracketRight", [44]="Enter",
        [46]="Semicolon", [47]="Quote", [48]="Backquote", [50]="Backslash", [51]="Comma", [52]="Period", [53]="Slash", [55]="NumpadMultiply",
        [57]="Space", [58]="CapsLock", [71]="NumLock", [72]="ScrollLock", [83]="NumpadSubtract", [84]="NumpadAdd", [85]="NumpadDecimal",
        [95]="NumpadDivide", [98]="Pause", [99]="Home", [100]="ArrowUp", [101]="PageUp", [102]="ArrowLeft", [103]="ArrowRight", [104]="End",
        [105]="ArrowDown", [106]="PageDown", [107]="Insert", [108]="Delete"})[n] end
    if not code then return "" end
    return (a.CtrlDown and "Ctrl+" or "") .. (a.AltDown and "Alt+" or "") .. (a.ShiftDown and "Shift+" or "") .. code
end

-- The interface modes that ask for a plot (the `targets` screen scans them), with the core's CvUnit::CanDoInterfaceMode
-- for each.
local TARGET_MODES = {
    INTERFACEMODE_RANGE_ATTACK = function(u) return u:CanRangeStrike() and u:GetDomainType() ~= AIR end,
    INTERFACEMODE_AIRSTRIKE = function(u) return u:GetDomainType() == AIR and u:CanRangeStrike() end,
    -- canAirSweep: the sweep's check at the unit's own plot (in range, seen) is the unit's own test.
    INTERFACEMODE_AIR_SWEEP = function(u) return u:GetDomainType() == AIR and u:CanStartMission(GameInfoTypes.MISSION_AIR_SWEEP, u:GetX(), u:GetY(), u:GetPlot(), 0) end,
    INTERFACEMODE_REBASE = function(u) return u:GetDomainType() == AIR end,
    INTERFACEMODE_NUKE = function(u) return u:CanNuke() end,
    INTERFACEMODE_PARADROP = function(u, visible) return u:CanParadrop(u:GetPlot(), visible) end,
    INTERFACEMODE_AIRLIFT = function(u) return u:CanAirlift(u:GetPlot()) end,
    INTERFACEMODE_EMBARK = function(u) return u:CanEmbark(u:GetPlot()) end,
    INTERFACEMODE_DISEMBARK = function(u) return u:CanDisembark(u:GetPlot()) end,
    INTERFACEMODE_ROUTE_TO = function(u) return u:CanBuildRoute() end,
    INTERFACEMODE_ATTACK = function(u, visible)
        return u:IsCanAttackWithMove() and not u:IsOutOfAttacks() and (visible or u:IsEnemyInMovementRange(false, u:IsCityAttackOnly()))
    end,
}

local function action_available(unit, a, visible)
    if not a or not a.Visible then return false end
    if a.Type == "INTERFACEMODE_MOVE_TO" then return visible or unit:MovesLeft() > 0 end
    if TARGET_MODES[a.Type] then return TARGET_MODES[a.Type](unit, visible) end
    if a.Type == "INTERFACEMODE_ESTABLISH_TRADE_ROUTE" then return unit:CanMakeTradeRoute(unit:GetPlot()) end
    if a.MissionType and a.MissionType >= 0 then
        return unit:CanStartMission(a.MissionType, a.MissionData, -1, unit:GetPlot(), visible and 1 or 0)
    end
    if a.CommandType and a.CommandType >= 0 then
        return unit:CanDoCommand(a.CommandType, a.CommandData, -1, visible and 1 or 0, 1)
    end
    -- SIMPLIFIED(SIMP-440): the group moves (INTERFACEMODE_MOVE_TO_TYPE, _MOVE_TO_ALL) are not offered: the client
    -- selects one unit. The other modes that ask for a plot are above and in the `targets` screen.
    return false
end

-- The text markup of the native strings (colours, line breaks) has no place in the skin's text.
local function plain(text)
    return (text:gsub("%[NEWLINE%]", " "):gsub("%[COLOR[^%]]*%]", ""):gsub("%[ENDCOLOR%]", ""))
end

-- The order of the unit panel's buttons: a fixed weight per kind of order, so that a button keeps its place whatever
-- else the unit can do (move, attacks, fortify and sleep, builds, special orders, upgrade, automation, delete last).
local ACTION_WEIGHT = {
    INTERFACEMODE_MOVE_TO = 10, INTERFACEMODE_ROUTE_TO = 11, INTERFACEMODE_ATTACK = 20, INTERFACEMODE_RANGE_ATTACK = 21,
    INTERFACEMODE_AIRSTRIKE = 22, INTERFACEMODE_AIR_SWEEP = 23, INTERFACEMODE_NUKE = 24, INTERFACEMODE_PARADROP = 25,
    INTERFACEMODE_REBASE = 26,
    MISSION_FORTIFY = 30, MISSION_GARRISON = 30, MISSION_SLEEP = 31, MISSION_ALERT = 32, MISSION_HEAL = 33,
    MISSION_AIRPATROL = 34, MISSION_PILLAGE = 60, MISSION_EMBARK = 61, MISSION_DISEMBARK = 61, MISSION_FOUND = 62,
    COMMAND_UPGRADE = 70, COMMAND_WAKE = 85, COMMAND_CANCEL = 86, COMMAND_CANCEL_ALL = 86, MISSION_SKIP = 90,
    COMMAND_DELETE = 99,
}
local function action_weight(a)
    if a.CommandType == GameInfoTypes.COMMAND_PROMOTION then return 5 end
    if a.MissionType == GameInfoTypes.MISSION_BUILD then return 40 end
    local w = ACTION_WEIGHT[a.Type]
    if w then return w end
    if type(a.Type) == "string" and a.Type:find("^AUTOMATE_") then return 80 end
    return 63
end

-- The action's own picture: the icon of its row in the tables the original UnitPanel reads (missions, commands,
-- automations, interface modes, builds), or the promotion's portrait.
local ACTION_TABLES = {"Missions", "Commands", "Automates", "InterfaceModes", "Builds", "Controls"}
local function action_icon(a)
    if type(a.Type) ~= "string" then return "" end
    local ok, found = pcall(function()
        for _, name in ipairs(ACTION_TABLES) do
            local rows = GameInfo[name]
            local row = rows and rows[a.Type]
            if row and row.IconAtlas and row.IconIndex then return icon(row.IconAtlas, row.IconIndex) end
        end
        local p = GameInfo.UnitPromotions and GameInfo.UnitPromotions[a.Type]
        if p and p.IconAtlas and p.PortraitIndex then return icon(p.IconAtlas, p.PortraitIndex) end
        return ""
    end)
    return ok and found or ""
end

local function action_catalog(player, unit)
    local out, id = array(), 0
    while GameInfoActions[id] do
        local a = GameInfoActions[id]
        if action_available(unit, a, true) then
            local enabled = Players[player]:IsTurnActive() and not unit:IsBusy() and not unit:IsDelayedDeath() and action_available(unit, a, false)
            local reason
            if not enabled then
                reason = not Players[player]:IsTurnActive() and "Wait for your turn." or
                    ((unit:IsBusy() or unit:IsDelayedDeath()) and "The unit is busy." or
                    (a.DisabledHelp and a.DisabledHelp ~= "" and plain(L(a.DisabledHelp)) or "The unit cannot perform this action here."))
            end
            out[#out + 1] = {id = id, name = plain(L(a.TextKey)), help = a.Help and plain(L(a.Help)) or "", hotkey = action_hotkey(a), enabled = enabled,
                reason = reason, promotion = a.CommandType == GameInfoTypes.COMMAND_PROMOTION,
                build = a.MissionType == GameInfoTypes.MISSION_BUILD,
                client = TARGET_MODES[a.Type] and "target" or ({INTERFACEMODE_MOVE_TO="move", MISSION_ALERT="alert", MISSION_HEAL="heal", COMMAND_DELETE="disband", COMMAND_UPGRADE="upgrade", AUTOMATE_EXPLORE="explore", AUTOMATE_BUILD="automate_build", MISSION_PILLAGE="pillage", MISSION_FOUND="found_city", MISSION_FORTIFY="fortify", MISSION_SLEEP="sleep", MISSION_SKIP="skip",
                    INTERFACEMODE_ESTABLISH_TRADE_ROUTE="trade_route", MISSION_ESTABLISH_TRADE_ROUTE="trade_route",
                    MISSION_CHANGE_TRADE_UNIT_HOME_CITY="trade_home", MISSION_CHANGE_ADMIRAL_PORT="admiral_port"})[a.Type] or "",
                icon = action_icon(a), weight = action_weight(a)}
        end
        id = id + 1
    end
    table.sort(out, function(x, y)
        if x.weight ~= y.weight then return x.weight < y.weight end
        return x.id < y.id
    end)
    return out
end

local function order(player, args, net, row)
    local unit = assert(own_unit(player, args.a), "unit id is required")
    local enabled = false
    for _, action in ipairs(unit_actions(unit)) do
        if action.id == 100 + row then enabled = action.enabled end
    end
    assert(enabled, "unit action is unavailable")
    if row == 3 then
        net.send("sendDoCommand", args.a, GameInfoTypes.COMMAND_UPGRADE, 0, 0, 0)
    else
        local mission = row == 1 and GameInfoTypes.MISSION_ALERT or GameInfoTypes.MISSION_PILLAGE
        net.send("sendPushMission", args.a, mission, 0, 0, 0, 0)
    end
end

return {
    data = function(player, args)
        local p, team = Players[player], team_of(player)
        local tt = team:GetTeamTechs()
        local techs, resources, policies = array(), array(), array()
        for tech in GameInfo.Technologies() do
            local pos = p:GetQueuePosition(tech.ID)
            techs[#techs + 1] = {
                id = tech.ID, status = team:IsHasTech(tech.ID) and 0 or (p:CanResearch(tech.ID) and 1 or 2),
                queue = pos > 0 and pos - 1 or nil,
                turns = turns(p:GetResearchTurnsLeft(tech.ID, true)),
                progress = tt:GetResearchProgress(tech.ID), cost = p:GetResearchCost(tech.ID),
            }
        end
        local demand
        for resource in GameInfo.Resources() do
            local usage = Game.GetResourceUsageType(resource.ID)
            local tip
            if usage == 1 then
                demand = demand or resource_demand(p)
                tip = rows(resource_text(player, p, resource, demand))
            end
            if usage == 0 or usage == 1 or usage == 2 then
                resources[#resources + 1] = {
                    id = resource.ID, usage = usage, available = p:GetNumResourceAvailable(resource.ID, true),
                    used = p:GetNumResourceUsed(resource.ID), total = p:GetNumResourceTotal(resource.ID, true),
                    name = L(resource.Description), icon = "resource:" .. resource.ID,
                    glyph = type(resource.IconString) == "string" and resource.IconString:match("^%[(ICON_[%w_]+)%]$") and
                        "font:" .. resource.IconString:match("^%[(ICON_[%w_]+)%]$") or "",
                    revealed = not resource.TechReveal or team:IsHasTech(GameInfoTypes[resource.TechReveal]),
                    imported = p:GetResourceImport(resource.ID), exported = p:GetResourceExport(resource.ID),
                    tip = tip or array(),
                }
            end
        end
        for branch in GameInfo.PolicyBranchTypes() do
            local owned, total = 0, 0
            for policy in GameInfo.Policies{PolicyBranchType = branch.Type} do
                total = total + 1
                if p:HasPolicy(policy.ID) then owned = owned + 1 end
            end
            policies[#policies + 1] = {
                id = branch.ID, name = L(branch.Description), type_name = branch.Type,
                unlocked = p:IsPolicyBranchUnlocked(branch.ID), owned = owned, total = total,
            }
        end
        local unit = own_unit(player, args.unit)
        local focused_unit = unit_data(unit)
        if focused_unit then focused_unit.catalog = action_catalog(player, unit) end
        return {
            research = {techs = techs}, top = {resources = resources, policies = policies, tourism = p:GetTourism(),
                tips = top_tips(player, p), anarchy_turns = p:GetAnarchyNumTurns(),
                science_enabled = not Game.IsOption(GameOptionTypes.GAMEOPTION_NO_SCIENCE),
                culture_enabled = not Game.IsOption(GameOptionTypes.GAMEOPTION_NO_POLICIES),
                faith_enabled = not Game.IsOption(GameOptionTypes.GAMEOPTION_NO_RELIGION),
                trade_used = p:GetNumInternationalTradeRoutesUsed(), trade_available = p:GetNumInternationalTradeRoutesAvailable()},
            city = city_data(own_city(player, args.city)), unit = focused_unit,
            turn = turn_data(player),
            lists = list_data(player),
            minimap = minimap_data(player),
            map = map_data(player),
        }
    end,
    actions = {
        do_action = function(player, args, net)
            local unit = assert(own_unit(player, args.a), "unit id is required")
            assert(Players[player]:IsTurnActive(), "Wait for your turn.")
            assert(not unit:IsBusy() and not unit:IsDelayedDeath(), "The unit is busy.")
            local a = GameInfoActions[args.b]
            assert(action_available(unit, a, false), "The unit cannot perform this action here.")
            assert((a.MissionType and a.MissionType >= 0) or (a.CommandType and a.CommandType >= 0), "Choose a target on the map.")
            if a.MissionType and a.MissionType >= 0 then
                net.send("sendPushMission", args.a, a.MissionType, a.MissionData, -1, 0, args.d == 1 and 1 or 0)
            else
                net.send("sendDoCommand", args.a, a.CommandType, a.CommandData, -1, args.c == 1 and 1 or 0)
            end
        end,
        rename = function(player, args, net)
            local unit = assert(own_unit(player, args.a), "unit id is required")
            assert(Players[player]:IsTurnActive() and not unit:IsBusy() and not unit:IsDelayedDeath(), "The unit cannot be renamed now.")
            local name = assert(args.text, "A new name is required."):match("^%s*(.-)%s*$")
            local _, chars = name:gsub("[^\128-\191]", "")
            local compact = name:gsub("%s", "")
            local _, letters = compact:gsub("[^\128-\191]", "")
            assert(letters >= 3 and chars <= 15 and not name:find('[%%"<>|/\\*?%[%]%c]'), "Use 3 to 15 characters without reserved symbols.")
            net.send_text("SendRenameUnit", name, unit:GetID())
        end,
        alert = function(player, args, net) order(player, args, net, 1) end,
        pillage = function(player, args, net) order(player, args, net, 2) end,
        upgrade = function(player, args, net) order(player, args, net, 3) end,
    },
}
