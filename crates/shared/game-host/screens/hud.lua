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

local function top_tips(p)
    -- SIMPLIFIED(SIMP-444): Help uses exported yield totals; detailed deal/resource allocation and faith-purchase target help remain unimplemented.
    local tips = array()
    local function tip(kind, title, total, entries)
        local lines = array()
        for _, entry in ipairs(entries) do
            local value = p[entry[2]](p, entry[4]) / (entry[3] or 1)
            lines[#lines + 1] = {key = entry[2], label = entry[1], value = value}
        end
        tips[#tips + 1] = {kind = kind, title = title, total = total, lines = lines}
        return lines
    end
    local science = tip("science", "Science per turn", p:GetScience(), {
        {"Cities (including trade)", "GetScienceFromCitiesTimes100", 100, false},
        {"Other civilizations", "GetScienceFromOtherPlayersTimes100", 100},
        {"Happiness", "GetScienceFromHappinessTimes100", 100},
        {"Research agreements", "GetScienceFromResearchAgreementsTimes100", 100},
        {"Budget deficit", "GetScienceFromBudgetDeficitTimes100", 100},
    })
    science[#science + 1] = {key = "overflow", label = "Research overflow", value = p:GetOverflowResearch()}
    tip("gold", "Gold per turn", p:CalculateGoldRate(), {
        {"Treasury", "GetGold"}, {"Gross income", "CalculateGrossGold"},
        {"Expenses", "CalculateInflatedCosts"}, {"Cities", "GetGoldFromCitiesTimes100", 100},
        {"City connections", "GetCityConnectionGoldTimes100", 100},
        {"Diplomacy", "GetGoldPerTurnFromDiplomacy"}, {"Religion", "GetGoldPerTurnFromReligion"},
        {"Unit maintenance", "CalculateUnitCost"}, {"Unit supply", "CalculateUnitSupply"},
        {"Buildings", "GetBuildingGoldMaintenance"}, {"Improvements", "GetImprovementGoldMaintenance"},
    })
    tip("happiness", "Empire happiness", p:GetExcessHappiness(), {
        {"Total happiness", "GetHappiness"}, {"Total unhappiness", "GetUnhappiness"},
        {"Policies", "GetHappinessFromPolicies"}, {"Resources", "GetHappinessFromResources"},
        {"Buildings", "GetHappinessFromBuildings"}, {"Religion", "GetHappinessFromReligion"},
        {"Natural wonders", "GetHappinessFromNaturalWonders"}, {"City-states", "GetHappinessFromMinorCivs"},
        {"World Congress", "GetHappinessFromLeagues"},
    })
    tip("culture", "Culture per turn", p:GetTotalJONSCulturePerTurn(), {
        {"Stored culture", "GetJONSCulture"}, {"Next policy", "GetNextPolicyCost"},
        {"Cities", "GetJONSCulturePerTurnFromCities"}, {"Happiness", "GetJONSCulturePerTurnFromExcessHappiness"},
        {"Traits", "GetJONSCulturePerTurnFromTraits"}, {"Free culture", "GetJONSCulturePerTurnForFree"},
        {"City-states", "GetJONSCulturePerTurnFromMinorCivs"}, {"Religion", "GetCulturePerTurnFromReligion"},
    })
    tip("faith", "Faith per turn", p:GetTotalFaithPerTurn(), {
        {"Stored faith", "GetFaith"}, {"Cities", "GetFaithPerTurnFromCities"},
        {"City-states", "GetFaithPerTurnFromMinorCivs"}, {"Religion", "GetFaithPerTurnFromReligion"},
    })
    tip("golden_age", "Golden age", p:GetGoldenAgeTurns(), {
        {"Turns remaining", "GetGoldenAgeTurns"}, {"Progress", "GetGoldenAgeProgressMeter"},
        {"Threshold", "GetGoldenAgeProgressThreshold"}, {"Points from cities", "GetGoldenAgePointsFromCities"},
    })
    tip("tourism", "Tourism per turn", p:GetTourism(), {
        {"Great works", "GetNumGreatWorks"}, {"Great work slots", "GetNumGreatWorkSlots"},
    })
    return tips
end

local function turn_data(player)
    local p, queue, waiting = Players[player], array(), array()
    local simultaneous = p:IsSimultaneousTurns()
    for id = 0, GameDefines.MAX_CIV_PLAYERS - 1 do
        local other = Players[id]
        if other and other:IsAlive() then
            local known = id == player or met(player, id)
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
        if unit:ReadyToMove() then waiting[#waiting + 1] = {key = key_unit(player, unit:GetID()), name = unit:GetName()} end
    end
    local notification = p:GetEndTurnBlockingNotificationIndex()
    return {blocker_kind = p:GetEndTurnBlockingType(),
        reason = notification >= 0 and p:GetNotificationStr(notification) or "",
        queue = queue, waiting_units = waiting}
end

local function list_data(player)
    -- SIMPLIFIED(SIMP-445): List status uses native activity; special mission status, type filters and drag resizing remain unimplemented.
    local p, units, groups = Players[player], array(), array()
    local nearest
    local activities = {[0]="awake", "hold", "sleep", "heal", "sentry", "intercept", "mission"}
    for u in p:Units() do
        local info = GameInfo.Units[u:GetUnitType()]
        units[#units + 1] = {key = key_unit(player, u:GetID()), id = u:GetID(), type_id = u:GetUnitType(),
            name = u:GetName(), icon = "unit:" .. u:GetUnitType(), military = (info.Combat or 0) > 0 or (info.RangedCombat or 0) > 0,
            status = activities[u:GetActivityType()] or "other", hp = GameDefines.MAX_HIT_POINTS - u:GetDamage(),
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
    for _, kind in ipairs({
        {0, "Units", "CanTrain"}, {1, "Buildings", "CanConstruct"},
        {2, "Projects", "CanCreate"}, {4, "Processes", "CanMaintain"},
    }) do
        for item in GameInfo[kind[2]]() do
            if not city[kind[3]](city, item.ID, 0, 0) then
                -- SIMPLIFIED(SIMP-324): Lua availability getters omit native tooltip reasons, so locked options use a general explanation.
                reasons[#reasons + 1] = {
                    id = kind[1] * 65536 + item.ID, order = kind[1], item = item.ID,
                    reason = "Production requirements are not met in this city.",
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

local function unit_data(unit)
    if not unit then return nil end
    local promotions = array()
    for promotion in GameInfo.UnitPromotions() do
        if unit:IsHasPromotion(promotion.ID) and promotion.ShowInUnitPanel ~= false and promotion.ShowInUnitPanel ~= 0 then
            promotions[#promotions + 1] = {id = promotion.ID, name = L(promotion.Description),
                icon = "promotion:" .. promotion.ID, help = promotion.Help and L(promotion.Help) or ""}
        end
    end
    return {id = unit:GetID(), name = unit:GetName(), xp_next = unit:ExperienceNeeded(), promotions = promotions, actions = unit_actions(unit)}
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
                    cities[#cities + 1] = {key = key_city(id, c:GetID()), hover_key = key_city(id, c:GetID()), name = c:GetName(), owner = owner,
                        strength = math.floor(c:GetStrengthValue() / 100), hp = c:GetMaxHitPoints() - c:GetDamage(), hp_max = c:GetMaxHitPoints(),
                        growth_turns = id == player and turns(c:GetFoodTurnsLeft()) or nil,
                        resistance_turns = id == player and c:IsResistance() and c:GetResistanceTurns() or nil,
                        razing_turns = id == player and c:IsRazing() and c:GetRazingTurns() or nil}
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

local function action_available(unit, a, visible)
    if not a or not a.Visible then return false end
    if a.Type == "INTERFACEMODE_MOVE_TO" then return visible or unit:MovesLeft() > 0 end
    if a.Type == "INTERFACEMODE_RANGE_ATTACK" then return unit:CanRangeStrike() end
    if a.Type == "INTERFACEMODE_ESTABLISH_TRADE_ROUTE" then return unit:CanMakeTradeRoute(unit:GetPlot()) end
    if a.MissionType and a.MissionType >= 0 then
        return unit:CanStartMission(a.MissionType, a.MissionData, -1, unit:GetPlot(), visible and 1 or 0)
    end
    if a.CommandType and a.CommandType >= 0 then
        return unit:CanDoCommand(a.CommandType, a.CommandData, -1, visible and 1 or 0, 1)
    end
    -- SIMPLIFIED(SIMP-440): Other interface-mode actions need client targeting intents and a seat-scoped applicability query.
    return false
end

local function action_catalog(player, unit)
    -- SIMPLIFIED(SIMP-441): Combat prediction and plot help await hover-plot subscriptions; HUD focus currently supplies selection ids only.
    local out, id = array(), 0
    while GameInfoActions[id] do
        local a = GameInfoActions[id]
        if action_available(unit, a, true) then
            local enabled = Players[player]:IsTurnActive() and not unit:IsBusy() and not unit:IsDelayedDeath() and action_available(unit, a, false)
            local reason
            if not enabled then
                reason = not Players[player]:IsTurnActive() and "Wait for your turn." or
                    ((unit:IsBusy() or unit:IsDelayedDeath()) and "The unit is busy." or
                    (a.DisabledHelp and a.DisabledHelp ~= "" and L(a.DisabledHelp) or "The unit cannot perform this action here."))
            end
            out[#out + 1] = {id = id, name = L(a.TextKey), help = a.Help and L(a.Help) or "", hotkey = action_hotkey(a), enabled = enabled,
                reason = reason, promotion = a.CommandType == GameInfoTypes.COMMAND_PROMOTION,
                build = a.MissionType == GameInfoTypes.MISSION_BUILD,
                client = ({INTERFACEMODE_MOVE_TO="move", INTERFACEMODE_RANGE_ATTACK="range_attack", MISSION_FOUND="found_city", MISSION_FORTIFY="fortify", MISSION_SLEEP="sleep", MISSION_SKIP="skip",
                    INTERFACEMODE_ESTABLISH_TRADE_ROUTE="trade_route", MISSION_ESTABLISH_TRADE_ROUTE="trade_route",
                    MISSION_CHANGE_TRADE_UNIT_HOME_CITY="trade_home", MISSION_CHANGE_ADMIRAL_PORT="admiral_port"})[a.Type] or ""}
        end
        id = id + 1
    end
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
        for resource in GameInfo.Resources() do
            local usage = Game.GetResourceUsageType(resource.ID)
            if usage == 0 or usage == 1 or usage == 2 then
                resources[#resources + 1] = {
                    id = resource.ID, usage = usage, available = p:GetNumResourceAvailable(resource.ID, true),
                    used = p:GetNumResourceUsed(resource.ID), total = p:GetNumResourceTotal(resource.ID, true),
                    name = L(resource.Description), icon = "resource:" .. resource.ID,
                    revealed = not resource.TechReveal or team:IsHasTech(GameInfoTypes[resource.TechReveal]),
                    imported = p:GetResourceImport(resource.ID), exported = p:GetResourceExport(resource.ID),
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
                tips = top_tips(p), anarchy_turns = p:GetAnarchyNumTurns(),
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
