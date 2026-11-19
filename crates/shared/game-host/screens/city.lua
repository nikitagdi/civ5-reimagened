-- City ids are seat-local. Every action carries the city in d, never a selection global.
local kinds = {
    [0] = {"Units", "unit", "CanTrain", "GetUnitProductionTurnsLeft", "GetUnitProductionNeeded", "GetUnitPurchaseCost", "GetUnitFaithPurchaseCost"},
    [1] = {"Buildings", "building", "CanConstruct", "GetBuildingProductionTurnsLeft", "GetBuildingProductionNeeded", "GetBuildingPurchaseCost", "GetBuildingFaithPurchaseCost"},
    [2] = {"Projects", "project", "CanCreate", "GetProjectProductionTurnsLeft", "GetProjectProductionNeeded", "GetProjectPurchaseCost"},
    [4] = {"Processes", "process", "CanMaintain"},
}
local focuses = {"default", "food", "production", "gold", "science", "culture", "great_people", "faith", "golden_age"}
local sales = {}
local focus_ids = {-1, 0, 1, 2, 4, 5, 3, 8, 9}
local yields = {"food", "production", "gold", "science", "culture", "faith"}
-- The game's font glyph of each yield and focus, as the original draws it in its texts ([ICON_FOOD]).
local glyphs = {food = "font:ICON_FOOD", production = "font:ICON_PRODUCTION", gold = "font:ICON_GOLD", science = "font:ICON_RESEARCH", culture = "font:ICON_CULTURE", faith = "font:ICON_PEACE", tourism = "font:ICON_TOURISM", golden_age = "font:ICON_GOLDEN_AGE", great_people = "font:ICON_GREAT_PEOPLE"}
local function whole(n) return math.floor(n / 100) end
local function color(name)
    local row = name and GameInfo.Colors[name]
    if not row then return 0 end
    return math.floor(row.Red * 255) * 16777216 + math.floor(row.Green * 255) * 65536 + math.floor(row.Blue * 255) * 256 + math.floor(row.Alpha * 255)
end
local function plain(key)
    return L(key):gsub("%[NEWLINE%]", "\n"):gsub("%[COLOR[^%]]*%]", ""):gsub("%[ENDCOLOR%]", ""):gsub("%[ICON_([^%]]+)%]", "%1 ")
end
-- Markup of an already localized core text.
local function markup(text)
    return (text:gsub("%[NEWLINE%]", "\n"):gsub("%[COLOR[^%]]*%]", ""):gsub("%[ENDCOLOR%]", ""):gsub("%[ICON_([^%]]+)%]", "%1 "):gsub("^%s+", ""):gsub("%s+$", ""))
end
-- The reasons the core itself writes for a locked unit or building (the tooltip sink of
-- CvCity::canTrain / canConstruct, plus the purchase checks), as the original city screens show them.
-- Per order: production, gold purchase, faith purchase. Read-only; the city belongs to the seat.
local lock_tips = {
    [0] = {"CanTrainTooltip", "GetPurchaseUnitTooltip", "GetFaithPurchaseUnitTooltip"},
    [1] = {"CanConstructTooltip", "GetPurchaseBuildingTooltip", "GetFaithPurchaseBuildingTooltip"},
}
local function core_reason(c, order, which, id)
    local getter = lock_tips[order] and lock_tips[order][which]
    if not getter or not c[getter] then return nil end
    local text = c[getter](c, id)
    if type(text) ~= "string" then return nil end
    text = markup(text)
    if text ~= "" then return text end
end
local function turns(n) if n and n >= 0 and n < 32767 then return n end end
local function own(player, id, edit)
    assert(type(id) == "number" and id == math.floor(id), "city id is required")
    local c = Players[player]:GetCityByID(id)
    assert(c and c:GetOwner() == player, "city is unavailable")
    if edit then
        assert(Players[player]:IsTurnActive(), "Wait for your turn.")
        assert(not c:IsPuppet(), "Annex the puppet to manage this city.")
    end
    return c
end
local function info(order, id)
    local k = assert(kinds[order], "unsupported production order")
    return assert(GameInfo[k[1]][id], "production item is unavailable"), k
end
local function decode(item)
    assert(type(item) == "number" and item >= 0 and item == math.floor(item), "item id is required")
    local order, id = math.floor(item / 65536), item % 65536
    local row, kind = info(order, id)
    return order, id, row, kind
end
local function item_kind(row, k)
    if k[2] == "building" then
        local cls = GameInfo.BuildingClasses[row.BuildingClass]
        if cls and (cls.MaxGlobalInstances > 0 or cls.MaxPlayerInstances > 0 or cls.MaxTeamInstances > 0) then return "wonder" end
    end
    return k[2]
end
local function purchase_args(order, id)
    return order == 0 and id or -1, order == 1 and id or -1, order == 2 and id or -1
end
local function purchasable(c, order, id, yield, cost)
    if order == 4 then return false end
    local u, b, p = purchase_args(order, id)
    return c:IsCanPurchase(cost, true, u, b, p, yield)
end
local function task(net, c, name, a, b, option)
    net.send("sendDoTask", c:GetID(), assert(TaskTypes[name], "task is unavailable"), a or -1, b or -1, option or 0, 0, 0, 0)
end
local function boolean(value) assert(value == 0 or value == 1, "flag must be 0 or 1"); return value end
local function slot(c, value)
    assert(type(value) == "number" and value == math.floor(value) and value >= 0 and value < c:GetOrderQueueLength(), "queue slot is unavailable")
    return value
end

return {
    data = function(player, args)
        local p = Players[player]
        local c
        if args.a ~= nil then c = own(player, args.a) else for candidate in p:Cities() do c = candidate; break end end
        if not c then return {city = -1, name = "No city selected", view_only = true, disabled_reason = "Found a city first.", status = array(), yields = array(), focus_options = array(), building_groups = array(), great_people = array(), tiles = array(), annex_preview = array(), production = {queue = array(), groups = array()}} end
        local view = c:IsPuppet() or args.b == 1
        local active = p:IsTurnActive()
        local reason = view and (c:IsPuppet() and "Annex the puppet to manage this city." or "This city is open for viewing only.") or (not active and "Wait for your turn." or nil)
        local editable = not view and active
        local queue, groups, buildings, tiles, statuses, income, gp, focus = array(), array(), array(), array(), array(), array(), array(), array()
        local rate = whole(c:GetCurrentProductionDifferenceTimes100(false, false))
        local after_repeat = false
        for i = 0, c:GetOrderQueueLength() - 1 do
            local order, id, _, repeating = c:GetOrderFromQueue(i)
            local row, k = info(order, id)
            queue[#queue + 1] = {slot = i, item = order * 65536 + id, name = L(row.Description), kind = item_kind(row, k), icon = icon(row.IconAtlas, row.PortraitIndex), turns = not after_repeat and k[4] and turns(c[k[4]](c, id, i)) or nil, number = not repeating and not after_repeat and i + 1 or nil, ["repeat"] = repeating, after_repeat = after_repeat}
            after_repeat = after_repeat or repeating or order == 4
        end
        local current
        if #queue > 0 then
            local q = queue[1]
            current = {item = q.item, name = q.name, kind = q.kind, icon = q.icon, stored = c:GetProduction(), needed = c:GetProductionNeeded(), turns = q.turns, next = rate}
        end
        local money = {gold = p:GetGold(), faith = p:GetFaith()}
        for _, order in ipairs({0, 1, 2, 4}) do
            local k = kinds[order]
            local options = array()
            for row in GameInfo[k[1]]() do
                local build = c[k[3]](c, row.ID, 0, 0)
                local gold = purchasable(c, order, row.ID, 2, false)
                local faith = purchasable(c, order, row.ID, 5, false)
                if build or gold or faith or c[k[3]](c, row.ID, 0, 1) then
                    local opt = {item = order * 65536 + row.ID, name = L(row.Description), kind = item_kind(row, k), icon = icon(row.IconAtlas, row.PortraitIndex), turns = k[4] and turns(c[k[4]](c, row.ID, 0)), cost = k[5] and c[k[5]](c, row.ID), can_build = editable and build, effect = plain(row.Help or row.Description), help = plain(row.Help or row.Description)}
                    -- SIMPLIFIED(SIMP-350): projects and processes have no core reason text (canCreate / canMaintain write none, the original screen shows none either), nor do the few unit/building refusals the core does not explain: those use a general line.
                    opt.reason = reason or (not build and (core_reason(c, order, 1, row.ID) or "Production requirements are not met in this city.") or nil)
                    for _, currency in ipairs({{"gold", gold, 2, k[6], 2}, {"faith", faith, 5, k[7], 3}}) do
                        local key, possible, y, getter = currency[1], currency[2], currency[3], currency[4]
                        if possible and getter then
                            local cost = c[getter](c, row.ID, true)
                            local purchase_control = active and args.b ~= 1 and (not c:IsPuppet() or p:MayNotAnnex())
                            local ok = purchase_control and purchasable(c, order, row.ID, y, true)
                            opt["buy_" .. key] = cost
                            opt[key .. "_state"] = ok and "ok" or (money[key] < cost and "short" or "off")
                            opt[key .. "_short"] = money[key] < cost and cost - money[key] or nil
                            if not ok and not opt.buy_reason then
                                opt.buy_reason = reason or core_reason(c, order, currency[5], row.ID) or "Insufficient funds or purchase requirements are not met."
                            end
                        end
                    end
                    options[#options + 1] = opt
                end
            end
            groups[#groups + 1] = {kind = order == 2 and "wonder_project" or k[2], count = #options, options = options}
        end
        local ordinary, wonders = groups[2], groups[3]
        local normal = array()
        for _, option in ipairs(ordinary.options) do
            if option.kind == "wonder" then wonders.options[#wonders.options + 1] = option else normal[#normal + 1] = option end
        end
        ordinary.options = normal; ordinary.count = #normal; wonders.count = #wonders.options
        for i, key in ipairs(focuses) do
            local disabled = reason or (key == "faith" and Game.IsOption(GameInfoTypes.GAMEOPTION_NO_RELIGION) and "Religion is disabled." or nil)
            focus[#focus + 1] = {key = key, icon = glyphs[key], id = focus_ids[i], enabled = not disabled, reason = disabled}
        end
        local focus_key = "default"
        for i, id in ipairs(focus_ids) do if c:GetFocusType() == id then focus_key = focuses[i] end end
        local function status(key, yes, text, n) if yes then statuses[#statuses + 1] = {key = key, text = text, turns = n} end end
        status("connected", p:IsCapitalConnectedToCity(c), "Connected to capital")
        status("blockaded", c:IsBlockaded(), "Blockaded")
        status("razing", c:IsRazing(), "Razing", c:GetRazingTurns())
        status("puppet", c:IsPuppet(), "Puppet")
        status("resistance", c:IsResistance(), "Resistance", c:GetResistanceTurns())
        status("occupied", c:IsOccupied() and not c:IsNoOccupiedUnhappiness(), "Occupied")
        for y = 0, 5 do
            local breakdown = array()
            for _, source in ipairs({{"Terrain", "Worked tiles"}, {"Buildings", "Buildings"}, {"Specialists", "Specialists"}, {"Misc", "Other sources"}, {"Religion", "Religion"}}) do
                breakdown[#breakdown + 1] = {label = source[2], value = c["GetBaseYieldRateFrom" .. source[1]](c, y)}
            end
            local value = y == 4 and c:GetJONSCulturePerTurn() or (y == 5 and c:GetFaithPerTurn() or c:GetYieldRate(y))
            income[#income + 1] = {key = yields[y + 1], icon = glyphs[yields[y + 1]], value = value, enabled = true, breakdown = breakdown}
        end
        income[#income + 1] = {key = "tourism", icon = glyphs.tourism, value = c:GetBaseTourism(), enabled = true, breakdown = array()}
        income[#income + 1] = {key = "golden_age", icon = glyphs.golden_age, value = c:GetYieldRate(6), enabled = true, breakdown = array()}
        local building_groups = {}
        for _, key in ipairs({"specialist", "great_work", "wonder", "other"}) do
            local g = {key = key, count = 0, maintenance = 0, buildings = array()}
            buildings[#buildings + 1] = g; building_groups[key] = g
        end
        local specialist_count, slots, gp_buildings = 0, 0, {}
        for row in GameInfo.Buildings() do
            if c:IsHasBuilding(row.ID) then
                local spec = row.SpecialistType and GameInfo.Specialists[row.SpecialistType]
                if spec and (row.GreatPeopleRateChange or 0) > 0 then gp_buildings[spec.ID] = true end
                local allowed = c:GetNumSpecialistsAllowedByBuilding(row.ID)
                local filled = c:GetNumSpecialistsInBuilding(row.ID)
                local key = allowed > 0 and "specialist" or ((row.GreatWorkCount or 0) > 0 and "great_work" or (item_kind(row, kinds[1]) == "wonder" and "wonder" or "other"))
                local g = building_groups[key]
                local sell = editable and c:IsBuildingSellable(row.ID)
                local b = {id = row.ID, name = L(row.Description), icon = icon(row.IconAtlas, row.PortraitIndex), yields = array(), defense = row.Defense and math.floor(row.Defense / 100), maintenance = row.GoldMaintenance or 0, free = c:GetNumFreeBuilding(row.ID) > 0, can_sell = sell, sell_refund = sell and c:GetSellBuildingRefund(row.ID) or nil, sell_reason = not sell and (reason or "This building cannot be sold this turn.") or nil, help = plain(row.Help or row.Description)}
                if spec and allowed > 0 then
                    b.specialist = {key = spec.Type, id = spec.ID, ["yield"] = "science", slots = allowed, filled = filled, can_add = editable and c:IsCanAddSpecialistToBuilding(row.ID), reason = reason or (not c:IsCanAddSpecialistToBuilding(row.ID) and "No citizen or specialist slot is available." or nil)}
                end
                for y = 0, 5 do
                    local v = c:GetBuildingYieldChange(GameInfo.BuildingClasses[row.BuildingClass].ID, y)
                    for change in GameInfo.Building_YieldChanges{BuildingType = row.Type} do
                        local yi = GameInfo.Yields[change.YieldType]
                        if yi and yi.ID == y then v = v + change.Yield end
                    end
                    if v ~= 0 then b.yields[#b.yields + 1] = {key = yields[y + 1], icon = glyphs[yields[y + 1]], value = v} end
                end
                if (row.GreatWorkCount or 0) > 0 then
                    local work_slots = array()
                    for index = 0, row.GreatWorkCount - 1 do
                        local work = c:GetBuildingGreatWork(GameInfo.BuildingClasses[row.BuildingClass].ID, index)
                        work_slots[#work_slots + 1] = {index = index, filled = work >= 0, name = work >= 0 and Game.GetGreatWorkName(work) or nil}
                    end
                    local work_kind = row.GreatWorkSlotType == "GREAT_WORK_SLOT_MUSIC" and "music" or (row.GreatWorkSlotType == "GREAT_WORK_SLOT_LITERATURE" and "writing" or "art")
                    b.great_works = {kind = work_kind, slots = work_slots}
                end
                g.buildings[#g.buildings + 1] = b; g.count = g.count + 1; g.maintenance = g.maintenance + b.maintenance
                specialist_count = specialist_count + filled; slots = slots + allowed
            end
        end
        -- CityView's great person meters: the rate from specialists and buildings with the player, policy, World
        -- Congress, city and golden age modifiers by great person type, the turns left, and the modifier lines.
        local league, golden
        local by_class = {
            UNITCLASS_WRITER = {"Writer", "GetArtsyGreatPersonRateModifier", true},
            UNITCLASS_ARTIST = {"Artist", "GetArtsyGreatPersonRateModifier", true},
            UNITCLASS_MUSICIAN = {"Musician", "GetArtsyGreatPersonRateModifier", true},
            UNITCLASS_SCIENTIST = {"Scientist", "GetScienceyGreatPersonRateModifier"},
            UNITCLASS_MERCHANT = {"Merchant", "GetScienceyGreatPersonRateModifier"},
            UNITCLASS_ENGINEER = {"Engineer", "GetScienceyGreatPersonRateModifier"},
        }
        for specialist in GameInfo.Specialists() do
            local cls = specialist.GreatPeopleUnitClass and GameInfo.UnitClasses[specialist.GreatPeopleUnitClass]
            if cls then
                if golden == nil then
                    league = Game.GetNumActiveLeagues() > 0 and Game.GetActiveLeague()
                    golden = p:GetGoldenAgeTurns() > 0
                end
                local threshold = c:GetSpecialistUpgradeThreshold(cls.ID)
                local progress = c:GetSpecialistGreatPersonProgressTimes100(specialist.ID) / 100
                local change = specialist.GreatPeopleRateChange * c:GetSpecialistCount(specialist.ID)
                for building in GameInfo.Buildings{SpecialistType = specialist.Type} do
                    if c:IsHasBuilding(building.ID) then change = change + building.GreatPeopleRateChange end
                end
                local player_mod, city_mod = p:GetGreatPeopleRateModifier(), c:GetGreatPeopleRateModifier()
                local policy_mod, congress_mod, golden_mod = p:GetPolicyGreatPeopleRateModifier(), 0, 0
                local kind = by_class[specialist.GreatPeopleUnitClass]
                if kind then
                    player_mod = player_mod + p["GetGreat" .. kind[1] .. "RateModifier"](p)
                    policy_mod = policy_mod + p["GetPolicyGreat" .. kind[1] .. "RateModifier"](p)
                    if league then congress_mod = congress_mod + league[kind[2]](league) end
                    if kind[3] and golden then
                        local mod = p["GetGoldenAgeGreat" .. kind[1] .. "RateModifier"](p)
                        if mod > 0 then golden_mod = golden_mod + mod end
                    end
                end
                -- The player modifier includes the policy and World Congress ones: shown apart.
                player_mod = player_mod - policy_mod - congress_mod
                change = ((player_mod + policy_mod + congress_mod + city_mod + golden_mod) / 100 + 1) * change
                if progress > 0 or change > 0 then
                    local unit = GameInfo.Units[cls.DefaultUnit]
                    local modifiers = array()
                    local function mod(key, value) if value ~= 0 then modifiers[#modifiers + 1] = {label = L(key, value), value = value} end end
                    mod("TXT_KEY_PLAYER_GP_MOD", player_mod)
                    mod("TXT_KEY_POLICY_GP_MOD", policy_mod)
                    mod("TXT_KEY_CITY_GP_MOD", city_mod)
                    mod("TXT_KEY_GOLDENAGE_GP_MOD", golden_mod)
                    mod(congress_mod < 0 and "TXT_KEY_WORLD_CONGRESS_NEGATIVE_GP_MOD" or "TXT_KEY_WORLD_CONGRESS_POSITIVE_GP_MOD", congress_mod)
                    gp[#gp + 1] = {key = specialist.Type, name = L(cls.Description), icon = unit and icon(unit.IconAtlas, unit.PortraitIndex) or "",
                        ["yield"] = specialist.Type == "SPECIALIST_ENGINEER" and "production" or (specialist.Type == "SPECIALIST_MERCHANT" and "gold" or "science"),
                        progress = math.floor(progress), threshold = threshold, rate = change > 0 and change or nil,
                        turns = change > 0 and math.ceil((threshold - progress) / change) or nil, modifiers = modifiers}
                end
            end
        end
        -- The tiles: the skin shows them on the map around the city (anchored on `cell`) and in the panel's list.
        local next_plot = c:GetNextBuyablePlot()
        for i = 0, c:GetNumCityPlots() - 1 do
            local plot = c:GetCityIndexPlot(i)
            if plot and visible(plot, p:GetTeam()) then
                local x, y = plot:GetX(), plot:GetY()
                local state = "unworked"
                if i == 0 then state = "center"
                elseif plot:GetOwner() == -1 then state = "unowned"
                elseif plot:GetOwner() ~= player then state = "enemy"
                elseif c:IsPlotBlockaded(plot) then state = "blocked"
                elseif c:IsForcedWorkingPlot(plot) then state = "locked"
                elseif c:IsWorkingPlot(plot) then state = "worked" end
                local t = {index = i, x = x, y = y, cell = x * 65536 + y, plot = plot:GetPlotIndex(), state = state, yields = array(), border_next = next_plot and next_plot:GetPlotIndex() == plot:GetPlotIndex() or false}
                if c:CanBuyPlotAt(x, y, true) then
                    t.buy_price = c:GetBuyPlotCost(x, y)
                    t.buy_state = editable and c:CanBuyPlotAt(x, y, false) and "ok" or (money.gold < t.buy_price and "short" or "off")
                    if t.buy_state ~= "ok" then t.buy_reason = reason or "Insufficient gold or this plot cannot be purchased." end
                end
                for yi = 0, 5 do local v = plot:CalculateYield(yi, true); if v ~= 0 then t.yields[#t.yields + 1] = {key = yields[yi + 1], icon = glyphs[yields[yi + 1]], value = v} end end
                tiles[#tiles + 1] = t
            end
        end
        local previous, following, next_choose, ids, at = nil, nil, nil, {}, 1
        for other in p:Cities() do
            ids[#ids + 1] = other:GetID()
            if other:GetID() == c:GetID() then at = #ids end
            if other:GetID() ~= c:GetID() and not other:IsPuppet() and other:GetOrderQueueLength() == 0 and not next_choose then next_choose = other:GetID() end
        end
        if #ids > 1 then previous = ids[(at - 2) % #ids + 1]; following = ids[at % #ids + 1] end
        -- SIMPLIFIED(SIMP-352): Completion history needs events, and the annex preview stays a fixed summary of the effects.
        local food = whole(c:FoodDifferenceTimes100())
        local growth = food > 0 and not c:IsFoodProduction() and "growing" or (food < 0 and "starving" or "stagnant")
        -- CityView's demand box: the We Love the King Day counter, or the demanded luxury with the turns until the city
        -- asks for another one (Lekmod's countdown).
        local demand
        local demanded = c:GetResourceDemanded(true)
        if demanded ~= -1 then
            local wltkd = c:GetWeLoveTheKingDayCounter()
            local r = GameInfo.Resources[c:GetResourceDemanded()]
            if wltkd > 0 then demand = {kind = "wltkd", text = L("TXT_KEY_CITYVIEW_WLTKD_COUNTER", wltkd), turns = wltkd}
            elseif r then
                local countdown = c:GetResourceDemandedCountdown()
                demand = {kind = "resource", text = L("TXT_KEY_CITYVIEW_RESOURCE_DEMANDED", L(r.Description) .. " (" .. countdown .. ")"), turns = countdown,
                    icon = "resource:" .. r.ID}
            end
        end
        local kind = c:IsRazing() and "unraze" or (c:IsPuppet() and "annex" or "raze")
        local task_ok = active and (kind == "unraze" or (kind == "annex" and not p:MayNotAnnex()) or (kind == "raze" and p:CanRaze(c))) and args.b ~= 1
        local civ = GameInfo.Civilizations[p:GetCivilizationType()]
        local palette = GameInfo.PlayerColors[p:GetPlayerColor()]
        local annex_preview = array()
        if c:IsPuppet() then
            annex_preview = array({{key = "control", label = "Production, citizens and buildings", value = "Under your control after annexation", tone = "neutral"}, {key = "penalty", label = "Puppet science and culture penalty", value = "Removed after annexation", tone = "neutral"}})
        end
        local sale = sales[player] and sales[player][c:GetID()]
        local sold_this_turn = sale and sale.turn == Game.GetGameTurn() and not c:IsHasBuilding(sale.building) or false
        return {city = c:GetID(), map_city = key_city(player, c:GetID()), name = c:GetName(), capital = c:IsCapital(), civ = icon(civ.IconAtlas, civ.PortraitIndex), civ_color = color(palette and palette.PrimaryColor), civ_color2 = color(palette and palette.SecondaryColor), owner = L(civ.ShortDescription), view_only = view, view_reason = view and (c:IsPuppet() and "puppet" or "popup_view") or "none", my_turn = active, disabled_reason = reason, can_rename = editable, can_cycle = not view and #ids > 1, prev_city = previous, next_city = following, city_count = #ids, choose_mode = args.b == 2, next_choose_city = next_choose, strength = whole(c:GetStrengthValue()), hp = c:GetMaxHitPoints() - c:GetDamage(), max_hp = c:GetMaxHitPoints(), status = statuses, demand = demand, population = c:GetPopulation(), population_label = L("TXT_KEY_CITYVIEW_CITIZENS_TEXT"), food = {stored = c:GetFood(), threshold = c:GrowthThreshold(), net = food, turns = growth ~= "stagnant" and turns(c:GetFoodTurnsLeft()) or nil, state = growth}, yields = income, border = {stored = c:GetJONSCultureStored(), threshold = c:GetJONSCultureThreshold()}, treasury = money, production = {rate = rate, overflow = c:GetOverflowProduction(), chop = c:GetFeatureProduction(), current = current, queue = queue, groups = groups}, great_people = gp, focus = focus_key, focus_options = focus, avoid_growth = c:IsForcedAvoidGrowth(), manual_specialists = c:IsNoAutoAssignSpecialists(), can_reset_tiles = editable and c:GetNumForcedWorkingPlots() > 0, unemployed = c:GetSpecialistCount(GameInfoTypes.DEFAULT_SPECIALIST or 0), specialists = specialist_count, specialist_slots = slots, sold_this_turn = sold_this_turn, building_groups = buildings, tiles = tiles, task = {kind = kind, mode = kind == "raze" and 1 or (kind == "unraze" and 2 or 3), enabled = task_ok, reason = not task_ok and "City task requirements are not met." or nil}, annex_preview = annex_preview}
    end,
    actions = {
        push_order = function(player, args, net)
            local c = own(player, args.d, true)
            local order, id, _, k = decode(args.a)
            assert(c[k[3]](c, id, 0, 0), "Production requirements are not met.")
            local mode = args.b or 0
            assert(mode == 0 or mode == 1 or mode == 2, "queue mode must be 0, 1 or 2")
            local repeating = boolean(args.c or 0)
            -- sendPushOrder(city, order, item, repeat, clear queue, append): 0 appends, 1 goes in front, 2 replaces the queue
            net.send("sendPushOrder", c:GetID(), order, id, repeating, mode == 2 and 1 or 0, mode == 0 and 1 or 0)
            if args.popup_id then net.popup_done(args.popup_id) end
            
        end,
        purchase = function(player, args, net)
            local c = own(player, args.d)
            assert(Players[player]:IsTurnActive() and (not c:IsPuppet() or Players[player]:MayNotAnnex()), "This city cannot purchase now.")
            local order, id = decode(args.a)
            local y = boolean(args.b) == 0 and 2 or 5
            assert(purchasable(c, order, id, y, true), "Purchase requirements are not met.")
            local u, b, p = purchase_args(order, id)
            net.send("sendPurchase_2", c:GetID(), u, b, p, y)
        end,
        pop_order = function(player, args, net) local c = own(player, args.d, true); net.send("sendPopOrder", c:GetID(), slot(c, args.a)) end,
        move_order = function(player, args, net)
            local c = own(player, args.d, true)
            local a, b = slot(c, args.a), slot(c, args.b)
            assert(a ~= b, "Choose a different queue position.")
            assert(math.abs(a - b) <= 256, "Move at most 256 queue positions at a time.")
            if a < b then for i = a, b - 1 do net.send("sendSwapOrder", c:GetID(), i) end
            else for i = a - 1, b, -1 do net.send("sendSwapOrder", c:GetID(), i) end end
        end,
        set_focus = function(player, args, net)
            local c = own(player, args.d, true)
            local valid = false; for _, id in ipairs(focus_ids) do if id == args.a then valid = true end end
            assert(valid, "focus is unavailable")
            assert(args.a ~= 8 or not Game.IsOption(GameInfoTypes.GAMEOPTION_NO_RELIGION), "Religion is disabled.")
            net.send("sendSetCityAIFocus", c:GetID(), args.a)
        end,
        avoid_growth = function(player, args, net) local c = own(player, args.d, true); net.send("sendSetCityAvoidGrowth", c:GetID(), boolean(args.a)) end,
        reset_tiles = function(player, args, net) local c = own(player, args.d, true); assert(c:GetNumForcedWorkingPlots() > 0, "No locked tiles to reset."); task(net, c, "TASK_CHANGE_WORKING_PLOT", 0) end,
        toggle_tile = function(player, args, net)
            local c = own(player, args.d, true)
            assert(args.a and args.a >= 0 and args.a < c:GetNumCityPlots() and args.a == math.floor(args.a), "plot index is unavailable")
            local plot = c:GetCityIndexPlot(args.a)
            assert(plot and plot:GetOwner() == player and not c:IsPlotBlockaded(plot) and not plot:IsVisibleEnemyUnit(player), "This plot cannot be worked.")
            task(net, c, "TASK_CHANGE_WORKING_PLOT", args.a)
        end,
        buy_plot = function(player, args, net) local c = own(player, args.d, true); assert(args.a and args.b and c:CanBuyPlotAt(args.a, args.b, false), "This plot cannot be purchased."); net.send("sendCityBuyPlot", c:GetID(), args.a, args.b) end,
        manual_specialists = function(player, args, net) local c = own(player, args.d, true); task(net, c, "TASK_NO_AUTO_ASSIGN_SPECIALISTS", -1, -1, boolean(args.a)) end,
        add_specialist = function(player, args, net)
            local c = own(player, args.d, true)
            local b = GameInfo.Buildings[args.b]
            assert(b and c:IsHasBuilding(args.b) and GameInfo.Specialists[b.SpecialistType] and GameInfo.Specialists[b.SpecialistType].ID == args.a and c:IsCanAddSpecialistToBuilding(args.b), "No specialist slot is available.")
            task(net, c, "TASK_NO_AUTO_ASSIGN_SPECIALISTS", -1, -1, 1); task(net, c, "TASK_ADD_SPECIALIST", args.a, args.b)
        end,
        remove_specialist = function(player, args, net)
            local c = own(player, args.d, true)
            local b = GameInfo.Buildings[args.b]
            assert(b and c:IsHasBuilding(args.b) and GameInfo.Specialists[b.SpecialistType] and GameInfo.Specialists[b.SpecialistType].ID == args.a and c:GetNumSpecialistsInBuilding(args.b) > 0, "No specialist is assigned to this building.")
            task(net, c, "TASK_NO_AUTO_ASSIGN_SPECIALISTS", -1, -1, 1); task(net, c, "TASK_REMOVE_SPECIALIST", args.a, args.b)
        end,
        unemployed_to_work = function(player, args, net)
            local c = own(player, args.d, true)
            local count = c:GetSpecialistCount(GameInfoTypes.DEFAULT_SPECIALIST or 0)
            assert(count > 0 and count <= 256, "No unemployed citizens are available.")
            if boolean(args.a) == 1 then count = 1 end
            for _ = 1, count do task(net, c, "TASK_REMOVE_SLACKER") end
        end,
        sell_building = function(player, args, net)
            local c = own(player, args.d, true)
            assert(GameInfo.Buildings[args.a] and c:IsHasBuilding(args.a) and c:IsBuildingSellable(args.a), "This building cannot be sold this turn.")
            net.send("SendSellBuilding", c:GetID(), args.a)
            sales[player] = sales[player] or {}
            sales[player][c:GetID()] = {turn = Game.GetGameTurn(), building = args.a}
        end,
    },
}
