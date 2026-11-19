-- The plot under the mouse of the seat that asks (args.x, args.y; args.unit: its selected unit), as the original's
-- PlotMouseoverInclude.lua (nature, resource, improvement, route, owner, yields, units) and EnemyUnitPanel.lua (the
-- combat preview) show it. Fog rules of the seat's team: an unrevealed plot says only that; a revealed one shows its
-- revealed improvement, route and owner; units, a city's state, yields and pillage marks only while the team sees it.
local YIELD_KEYS = {[0] = "food", "production", "gold", "science", "culture", "faith"}
local OUTCOMES = {[0] = "ranged", "stalemate", "total_defeat", "total_victory", "major_victory", "small_victory", "major_defeat", "small_defeat"}

local function try(f, fallback)
    local ok, v = pcall(f)
    if ok and v ~= nil then return v end
    return fallback
end

local function relation(team, other)
    if other == team then return "own" end
    if Teams[team]:IsAtWar(other) then return "war" end
    return "friend"
end

local function civ_name(owner)
    local p = Players[owner]
    if not p then return "" end
    local nick = try(function() return p:GetNickName() end, "")
    if nick ~= "" and p:IsHuman() then return nick end
    return try(function() return p:GetCivilizationShortDescription() end, "")
end

local SPECIAL = {FEATURE_JUNGLE = true, FEATURE_MARSH = true, FEATURE_OASIS = true, FEATURE_ICE = true}

-- A localized text without the markup the skin's text does not draw (icons become their names).
local function plain(text)
    return (text:gsub("%[NEWLINE%]", " "):gsub("%[COLOR[^%]]*%]", ""):gsub("%[ENDCOLOR%]", "")
        :gsub("%[ICON_([^%]]+)%]", ""):gsub("%s+", " "):gsub("^%s+", ""):gsub("%s+$", ""))
end

-- The original's GenerateResourceToolTip (ResourceTooltipGenerator.lua, basic help on): the quantity and the name in
-- capitals, the resource's help, the happiness it gives improved, the yields it adds improved and worked. Each line is
-- the original's markup as text parts and font pictures (`markup_lines`); `text` is the same line without markup.
local function resource_tip(out, info, count)
    local name = L(info.Description)
    name = (Locale and Locale.ToUpper) and Locale.ToUpper(name) or name:upper()
    out.resource_title = (count > 1 and (count .. " ") or "") .. name
    local lines = out.resource_lines
    local function add(text)
        for _, line in ipairs(markup_lines({text})) do
            local words = {}
            for _, p in ipairs(line.parts) do words[#words + 1] = p.text end
            lines[#lines + 1] = {idx = #lines, text = plain(table.concat(words)), parts = line.parts}
        end
    end
    if info.Help then
        add("[ICON_BULLET]" .. L(info.Help))
        add("---------------------")
    end
    if info.Happiness and info.Happiness ~= 0 then
        add(L("TXT_KEY_RESOURCE_TOOLTIP_IMPROVED"))
        add("[ICON_BULLET]+" .. info.Happiness .. " [ICON_HAPPINESS_1] " .. L("TXT_KEY_GAME_CONCEPT_SECTION_10"))
    end
    local yields = ""
    for row in GameInfo.Resource_YieldChanges("ResourceType = '" .. info.Type .. "'") do
        local y = GameInfo.Yields[row.YieldType]
        if y then
            yields = yields .. (yields == "" and "[ICON_BULLET]" or " ") .. (row.Yield > 0 and "+" or "") .. row.Yield
                .. (y.IconString or "") .. " " .. L(y.Description)
        end
    end
    if yields ~= "" then
        add(L("TXT_KEY_RESOURCE_TOOLTIP_IMPROVED_WORKED"))
        add(yields)
    end
end

-- `GetNatureString`: the feature (alone when it always has one terrain), else the mountain or the terrain (a lake),
-- then hills and river.
local function nature(plot)
    local parts = {}
    local feature = plot:GetFeatureType()
    local info = feature >= 0 and GameInfo.Features[feature] or nil
    if info and SPECIAL[info.Type] then
        parts[#parts + 1] = L(info.Description)
    else
        local mountain = false
        if info then
            parts[#parts + 1] = L(info.Description)
            mountain = info.NaturalWonder == true or info.NaturalWonder == 1
        elseif plot:IsMountain() then
            parts[#parts + 1] = L("TXT_KEY_PLOTROLL_MOUNTAIN")
            mountain = true
        end
        if not mountain then
            if plot:IsLake() then
                parts[#parts + 1] = L("TXT_KEY_PLOTROLL_LAKE")
            else
                local terrain = GameInfo.Terrains[plot:GetTerrainType()]
                if terrain then parts[#parts + 1] = L(terrain.Description) end
            end
        end
    end
    if plot:IsHills() then parts[#parts + 1] = L("TXT_KEY_PLOTROLL_HILL") end
    if plot:IsRiver() then parts[#parts + 1] = L("TXT_KEY_PLOTROLL_RIVER") end
    return table.concat(parts, ", ")
end

-- EnemyUnitPanel's estimate for the seat's unit against a visible enemy unit on the plot.
local function combat(mine, theirs, plot)
    local max_hp = GameDefines.MAX_HIT_POINTS
    local from = mine:GetPlot()
    local ranged = mine:GetBaseRangedCombatStrength() > 0
    local my_strength, their_strength, my_damage, their_damage = 0, 0, 0, 0
    if ranged then
        my_strength = mine:GetMaxRangedCombatStrength(theirs, nil, true, true)
        their_damage = mine:GetRangeCombatDamage(theirs, nil, false)
        if theirs:IsEmbarked() then
            their_strength = try(function() return theirs:GetEmbarkedUnitDefense() end, 0)
        else
            their_strength = theirs:GetMaxRangedCombatStrength(mine, nil, false, false)
        end
        if their_strength == 0 or theirs:GetDomainType() == DomainTypes.DOMAIN_SEA or theirs:IsRangedSupportFire() then
            their_strength = theirs:GetMaxDefenseStrength(plot, mine, true)
        end
    else
        my_strength = mine:GetMaxAttackStrength(from, plot, theirs)
        their_strength = theirs:GetMaxDefenseStrength(plot, mine)
        my_damage = mine:GetCombatDamage(my_strength, their_strength, mine:GetDamage(), false, false, false)
        their_damage = theirs:GetCombatDamage(their_strength, my_strength, theirs:GetDamage(), false, false, false)
        -- The panel names the damage each side takes; GetCombatDamage gives the damage dealt to the other side.
        my_damage, their_damage = their_damage, my_damage
    end
    if my_strength <= 0 then return nil end
    local prediction = try(function() return Game.GetCombatPrediction(mine, theirs) end, -1)
    return {ranged = ranged, outcome = OUTCOMES[prediction] or "none",
        my_name = mine:GetName(), their_name = theirs:GetName(),
        my_strength = math.floor(my_strength / 100), their_strength = math.floor(their_strength / 100),
        my_damage = math.min(my_damage, max_hp), their_damage = math.min(their_damage, max_hp),
        my_hp = max_hp - mine:GetDamage(), their_hp = max_hp - theirs:GetDamage(), hp_max = max_hp}
end

return {
    data = function(player, args)
        local x, y = args.x, args.y
        assert(x and y and x == math.floor(x) and y == math.floor(y), "a plot is required")
        local out = {x = x, y = y, cell = x * 65536 + y, revealed = false, visible = false, nature = "",
            yields = array(), units = array(), resource_lines = array()}
        local plot = Map.GetPlot(x, y)
        if not plot then return out end
        local p = Players[player]
        local team = p:GetTeam()
        if not plot:IsRevealed(team, false) then return out end
        local seen = visible(plot, team)
        out.revealed, out.visible = true, seen
        out.nature = nature(plot)
        local resource = plot:GetResourceType(team)
        if resource >= 0 then
            local info = GameInfo.Resources[resource]
            out.resource = L(info.Description)
            out.resource_count = plot:GetNumResource()
            resource_tip(out, info, out.resource_count)
            local tech = info.TechCityTrade and GameInfoTypes[info.TechCityTrade]
            if tech and tech >= 0 and not team_of(player):GetTeamTechs():HasTech(tech) then
                out.resource_tech = L(GameInfo.Technologies[tech].Description)
            end
        end
        local improvement = plot:GetRevealedImprovementType(team, false)
        if improvement >= 0 then
            out.improvement = L(GameInfo.Improvements[improvement].Description)
            out.improvement_pillaged = seen and plot:IsImprovementPillaged()
        end
        local route = plot:GetRevealedRouteType(team, false)
        if route >= 0 then
            out.route = L(GameInfo.Routes[route].Description)
            out.route_pillaged = seen and plot:IsRoutePillaged()
        end
        local owner = plot:GetRevealedOwner(team, false)
        if owner >= 0 and Players[owner] then
            out.owner = civ_name(owner)
            out.owner_relation = relation(team, Players[owner]:GetTeam())
        end
        out.defense = try(function() return plot:DefenseModifier(team, false, false) end, 0)
        if plot:IsCity() then
            local city = plot:GetPlotCity()
            if city then
                local o = city:GetOwner()
                out.city = {name = city:GetName(), owner = civ_name(o), relation = relation(team, Players[o]:GetTeam())}
                if seen or o == player then
                    out.city.population = city:GetPopulation()
                    out.city.strength = math.floor(city:GetStrengthValue() / 100)
                    out.city.hp_max = city:GetMaxHitPoints()
                    out.city.hp = city:GetMaxHitPoints() - city:GetDamage()
                end
            end
        end
        -- SIMPLIFIED(SIMP-446): the yields of a plot the team does not see now are left out (the core's display
        -- yields read the active team's knowledge, which a seat-scoped query cannot use).
        if seen then
            for i = 0, 5 do
                local yield = GameInfo.Yields[i]
                local v = plot:CalculateYield(i, true)
                if yield and v > 0 then
                    out.yields[#out.yields + 1] = {key = YIELD_KEYS[i], name = L(yield.Description), value = v}
                end
            end
            local mine = args.unit and p:GetUnitByID(args.unit) or nil
            if mine and mine:GetOwner() ~= player then mine = nil end
            for i = 0, plot:GetNumUnits() - 1 do
                local u = plot:GetUnit(i)
                if u and not u:IsInvisible(team, false) then
                    local rel = relation(team, u:GetTeam())
                    out.units[#out.units + 1] = {idx = #out.units, name = u:GetName(), owner = civ_name(u:GetOwner()),
                        relation = rel, strength = u:GetBaseCombatStrength(),
                        hp = GameDefines.MAX_HIT_POINTS - u:GetDamage(), hp_max = GameDefines.MAX_HIT_POINTS,
                        embarked = u:IsEmbarked()}
                    if rel == "war" and out.combat == nil and mine and mine:GetBaseCombatStrength() > 0 then
                        out.combat = combat(mine, u, plot)
                    end
                end
            end
            -- SIMPLIFIED(SIMP-447): the preview of an attack on a city (EnemyUnitPanel's city half) is not built.
        end
        return out
    end,
    actions = {},
}
