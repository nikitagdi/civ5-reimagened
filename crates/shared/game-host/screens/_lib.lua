-- Helpers shared by built-in data scripts. Scripts always receive the seat explicitly.
function array(values) return setmetatable(values or {}, { __screen_array = true }) end
function L(key, ...)
    -- Rows without a text column (no help, no title) read as empty text.
    if key == nil then return "" end
    if Locale and Locale.ConvertTextKey then return Locale.ConvertTextKey(key, ...) end
    error("server localization is unavailable")
end
-- A game text without its markup ([NEWLINE], [COLOR_*]/[ENDCOLOR], [ICON_*]): the skins show plain text.
function plain(text)
    return (tostring(text or ""):gsub("%[NEWLINE%]", " "):gsub("%[[^%]]*%]", ""):gsub("%s+", " "):gsub("^ ", ""):gsub(" $", ""))
end
function icon(atlas, index)
    if not atlas or not index or index < 0 then return "" end
    return "icon:" .. atlas .. ":" .. index
end

local tech_function_icon = icon("GENERIC_FUNC_ATLAS", 0)
local tech_flags = {
    {"AllowsEmbarking", "TXT_KEY_ALLOWS_EMBARKING"},
    {"AllowsDefensiveEmbarking", "TXT_KEY_ABLTY_DEFENSIVE_EMBARK_STRING"},
    {"EmbarkedAllWaterPassage", "TXT_KEY_ALLOWS_CROSSING_OCEANS"},
    {"AllowEmbassyTradingAllowed", "TXT_KEY_ALLOWS_EMBASSY"},
    {"OpenBordersTradingAllowed", "TXT_KEY_ALLOWS_OPEN_BORDERS"},
    {"DefensivePactTradingAllowed", "TXT_KEY_ALLOWS_DEFENSIVE_PACTS"},
    {"ResearchAgreementTradingAllowed", "TXT_KEY_ALLOWS_RESEARCH_AGREEMENTS"},
    {"TradeAgreementTradingAllowed", "TXT_KEY_ALLOWS_TRADE_AGREEMENTS"},
    {"MapTrading", "Allows map trading"},
    {"BridgeBuilding", "TXT_KEY_ALLOWS_BRIDGES"},
    {"MapVisible", "TXT_KEY_REVEALS_ENTIRE_MAP"},
    {"AllowsWorldCongress", "TXT_KEY_ALLOWS_WORLD_CONGRESS"},
}
local tech_modifiers = {
    {"EmbarkedMoveChange", "TXT_KEY_FASTER_EMBARKED_MOVEMENT"},
    {"UnitFortificationModifier", "TXT_KEY_UNIT_FORTIFICATION_MOD"},
    {"UnitBaseHealModifier", "TXT_KEY_UNIT_BASE_HEAL_MOD"},
    {"InfluenceSpreadModifier", "TXT_KEY_DOUBLE_TOURISM"},
    {"ExtraVotesPerDiplomat", "TXT_KEY_EXTRA_VOTES_FROM_DIPLOMATS"},
}
local tech_yield_tables = {
    {"Improvement_TechYieldChanges", "TXT_KEY_YIELD_IMPROVED"},
    {"Improvement_TechNoFreshWaterYieldChanges", "TXT_KEY_NO_FRESH_WATER"},
    {"Improvement_TechFreshWaterYieldChanges", "TXT_KEY_FRESH_WATER"},
}
local function enabled(value) return value == true or value == 1 end

local function tech_choices(classes, default, overrides, class_field, item_field, civ)
    local choices = {}
    for class in classes() do choices[class.Type] = class[default] end
    for override in overrides{CivilizationType = civ} do choices[override[class_field]] = override[item_field] end
    return choices
end

-- The original TechButtonInclude.lua reads these flags and GameInfo relations.
-- Resolve class defaults and overrides once per request, including for the full tree.
function tech_unlocks_for(player)
    local civ = GameInfo.Civilizations[Players[player]:GetCivilizationType()].Type
    local units = tech_choices(GameInfo.UnitClasses, "DefaultUnit", GameInfo.Civilization_UnitClassOverrides,
        "UnitClassType", "UnitType", civ)
    local buildings = tech_choices(GameInfo.BuildingClasses, "DefaultBuilding", GameInfo.Civilization_BuildingClassOverrides,
        "BuildingClassType", "BuildingType", civ)
    return function(tech, indexed, markup)
        local out, seen = array(), {}
        local function caption(text) return markup and text or plain(text) end
        local function add(key, kind, name, asset)
            if seen[key] then return end
            seen[key] = true
            out[#out + 1] = {idx = indexed and #out or nil, key = key, kind = kind, name = name, icon = asset}
        end
        local function item(row, kind, name)
            add(row.Type, kind, name and caption(name) or L(row.Description), icon(row.IconAtlas, row.PortraitIndex or row.IconIndex))
        end
        local function ability(key, name)
            add("ability:" .. key, "ability", caption(name), tech_function_icon)
        end
        for unit in GameInfo.Units{PrereqTech = tech.Type} do
            if units[unit.Class] == unit.Type then item(unit, "unit") end
        end
        for building in GameInfo.Buildings{PrereqTech = tech.Type} do
            if buildings[building.BuildingClass] == building.Type then
                local class = GameInfo.BuildingClasses[building.BuildingClass]
                item(building, (class.MaxGlobalInstances or -1) > 0 and "wonder" or "building")
            end
        end
        for resource in GameInfo.Resources{TechReveal = tech.Type} do
            item(resource, "resource", L("TXT_KEY_REVEALS_RESOURCE_ON_MAP", resource.Description))
        end
        for project in GameInfo.Projects{TechPrereq = tech.Type} do item(project, "project") end
        for build in GameInfo.Builds{PrereqTech = tech.Type, ShowInTechTree = 1} do
            local improvement = build.ImprovementType and GameInfo.Improvements[build.ImprovementType]
            if not build.ImprovementType or (improvement and
                (not improvement.CivilizationType or improvement.CivilizationType == civ)) then
                item(build, "improvement")
            end
        end
        for process in GameInfo.Processes{TechPrereq = tech.Type} do
            item(process, "ability", L("TXT_KEY_ENABLE_PRODUCITON_CONVERSION", process.Description))
        end
        local filter = {TechType = tech.Type}
        for row in GameInfo.Route_TechMovementChanges(filter) do
            local route = GameInfo.Routes[row.RouteType]
            if route and row.MovementChange < 0 then
                ability("route:" .. route.Type, L("TXT_KEY_FASTER_MOVEMENT", route.Description))
            elseif route and row.MovementChange > 0 then
                ability("route:" .. route.Type, "Slower movement on " .. L(route.Description))
            end
        end
        for _, spec in ipairs(tech_yield_tables) do
            local changes, order = {}, {}
            for row in GameInfo[spec[1]](filter) do
                local improvement, yield = GameInfo.Improvements[row.ImprovementType], GameInfo.Yields[row.YieldType]
                if improvement and yield and row.Yield ~= 0 then
                    local key = improvement.Type
                    if not changes[key] then
                        changes[key] = {}
                        order[#order + 1] = key
                    end
                    local entries = changes[key]
                    entries[#entries + 1] = L(spec[2], improvement.Description, yield.Description, row.Yield)
                end
            end
            table.sort(order)
            for _, key in ipairs(order) do
                table.sort(changes[key])
                ability(spec[1] .. ":" .. key, table.concat(changes[key], " "))
            end
        end
        for _, spec in ipairs(tech_flags) do
            if enabled(tech[spec[1]]) then ability(spec[1], L(spec[2])) end
        end
        for _, spec in ipairs(tech_modifiers) do
            local value = tech[spec[1]] or 0
            if value > 0 then ability(spec[1], L(spec[2], value)) end
        end
        local routes = tech.InternationalTradeRoutesChange or 0
        if routes == 1 then
            ability("InternationalTradeRoutesChange", L("TXT_KEY_ADDITIONAL_INTERNATIONAL_TRADE_ROUTE"))
        elseif routes ~= 0 then
            ability("InternationalTradeRoutesChange", string.format("%+d international trade routes", routes))
        end
        local range_labels = {DOMAIN_LAND = "TXT_KEY_EXTENDS_LAND_TRADE_ROUTE_RANGE", DOMAIN_SEA = "TXT_KEY_EXTENDS_SEA_TRADE_ROUTE_RANGE"}
        for row in GameInfo.Technology_TradeRouteDomainExtraRange(filter) do
            local label = range_labels[row.DomainType]
            if label and row.Range > 0 then ability("trade_range:" .. row.DomainType, L(label)) end
        end
        for row in GameInfo.Technology_FreePromotions(filter) do
            local promotion = GameInfo.UnitPromotions[row.PromotionType]
            if promotion then
                item(promotion, "ability", L("TXT_KEY_FREE_PROMOTION_FROM_TECH", promotion.Description, promotion.Help))
            end
        end
        -- SIMPLIFIED(SIMP-341): scenario-specific buttons from ScenarioTechButton are omitted.
        return out
    end
end

function key_city(owner, id) return owner .. ":" .. id end
function key_unit(owner, id) return owner .. ":" .. id end
function key_item(order, item) return order .. ":" .. item end
function team_of(player) return Teams[Players[player]:GetTeam()] end
function met(player, other) return team_of(player):IsHasMet(Players[other]:GetTeam()) end
function visible(plot, team) return plot:IsVisible(team, false) end
-- The worker runs every screen query and action through this: with the real core, GcUiReadScope puts back the synced
-- state that Lua getters change on the asking machine only (the game RNG, the might caches), so a screen cannot change
-- the simulation. Without it (the stub, tests) the function is called directly.
-- Where a seat's unit stands, for the worker's stale move questions: true, x, y; true when the unit is gone; false
-- without the core's API (the stub).
function __unit_plot(player, id)
    local players = rawget(_G, "Players")
    if not players then return false end
    local p = players[player]
    local u = p and p:GetUnitByID(id)
    if not u or u:IsDead() or u:IsDelayedDeath() then return true end
    return true, u:GetX(), u:GetY()
end
function __screen_call(f, ...)
    local scope = rawget(_G, "GcUiReadScope")
    if scope then return scope(f, ...) end
    return f(...)
end
