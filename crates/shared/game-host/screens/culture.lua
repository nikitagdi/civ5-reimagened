local seats = {}
local classes = { [1] = "art", [2] = "artifact", [3] = "writing", [4] = "music" }
local offers = {{3, "writing", "GetSwappableGreatWriting"}, {1, "art", "GetSwappableGreatArt"}, {2, "artifact", "GetSwappableGreatArtifact"}}
local slot_types = {GREAT_WORK_SLOT_ART_ARTIFACT = "art_artifact", GREAT_WORK_SLOT_LITERATURE = "literature", GREAT_WORK_SLOT_MUSIC = "music"}

local function color(value)
    return math.floor(value.x * 255 + 0.5) * 16777216 + math.floor(value.y * 255 + 0.5) * 65536 + math.floor(value.z * 255 + 0.5) * 256 + 255
end

local function civ(id)
    local p = Players[id]
    if not p then
        return {id = id, name = L("TXT_KEY_UNMET_PLAYER"), civ_name = L("TXT_KEY_UNMET_PLAYER"),
            civ = "", leader = "", color_bg = 0x26221dff, color_fg = 0xebe2d2ff}
    end
    local info = GameInfo.Civilizations[p:GetCivilizationType()]
    local leader = GameInfo.Leaders[p:GetLeaderType()]
    local primary, secondary = p:GetPlayerColors()
    return {id = id, name = L(leader.Description), civ_name = L(info.ShortDescription),
        civ = "civ:" .. info.Type, leader = "leader:" .. leader.Type,
        color_bg = color(primary), color_fg = color(secondary)}
end

local function work(id, player)
    if id == nil or id < 0 then return nil end
    local origin = civ(Game.GetGreatWorkCreator(id))
    local kind = assert(classes[Game.GetGreatWorkClass(id)], "unknown great work class")
    return {id = id, name = L(Game.GetGreatWorkName(id)), class = kind,
        creator = kind ~= "artifact" and L(Game.GetGreatWorkArtist(id)) or nil,
        civ_name = origin.civ_name, civ = origin.civ, color_bg = origin.color_bg,
        color_fg = origin.color_fg, era = L(Game.GetGreatWorkEraShort(id)),
        tooltip = Game.GetGreatWorkTooltip(id, player), viewable = kind ~= "artifact"}
end

local function buildings(player)
    local p = Players[player]
    local civ_type = GameInfo.Civilizations[p:GetCivilizationType()].Type
    local replacements = {}
    for row in GameInfo.Civilization_BuildingClassOverrides{CivilizationType = civ_type} do
        replacements[row.BuildingClassType] = row.BuildingType or false
    end
    local result = array()
    for class in GameInfo.BuildingClasses() do
        local kind = replacements[class.Type]
        if kind == nil then kind = class.DefaultBuilding end
        local b = kind and GameInfo.Buildings[kind]
        if b and b.GreatWorkCount > 0 and slot_types[b.GreatWorkSlotType] then
            result[#result + 1] = {info = b, class = class.ID, key = class.Type,
                extra = class.MaxGlobalInstances > 0, slot_type = slot_types[b.GreatWorkSlotType]}
        end
    end
    return result
end

local function slots(player, city, building, state)
    local out = array()
    for index = 0, building.info.GreatWorkCount - 1 do
        local key = city:GetID() .. ":" .. building.class .. ":" .. index
        local address = state.keys[key]
        if not address then
            state.next = state.next + 1
            assert(state.next < 2147483647, "great work address limit reached")
            address = state.next
            state.keys[key] = address
        end
        state.slots[address] = {city = city:GetID(), building = building.info.ID,
            class = building.class, index = index, slot_type = building.slot_type}
        out[#out + 1] = {id = address, slot_type = building.slot_type,
            work = work(city:GetBuildingGreatWork(building.class, index), player)}
    end
    return out
end

local function city_rows(player, catalog)
    local state = seats[player] or {keys = {}, next = 0}
    seats[player] = state
    state.slots = {}
    local rows, total_slots, total_works = array(), 0, 0
    for city in Players[player]:Cities() do
        local cells, extra, count = array(), array(), 0
        for _, b in ipairs(catalog) do
            local built = city:IsHasBuilding(b.info.ID)
            local entries = built and slots(player, city, b, state) or array()
            if built then count = count + #entries end
            local bonus = built and city:GetThemingBonus(b.class) or nil
            local tooltip = built and city:GetThemingTooltip(b.class) or nil
            if b.extra then
                if built then extra[#extra + 1] = {key = b.key, building = "building:" .. b.info.Type,
                    name = L(b.info.Description), theme_bonus = bonus, theme_tooltip = tooltip, slots = entries} end
            else
                local status = built and "built" or "not_built"
                if not built and GameInfo.BuildingClasses[b.key].MaxPlayerInstances == 1 then
                    for other in Players[player]:Cities() do
                        if other:IsHasBuilding(b.info.ID) then status = "none" end
                    end
                end
                cells[#cells + 1] = {column = b.key, state = status, theme_bonus = bonus,
                    theme_tooltip = tooltip, slots = entries}
            end
        end
        local works = city:GetNumGreatWorks()
        total_slots, total_works = total_slots + count, total_works + works
        rows[#rows + 1] = {id = city:GetID(), name = city:GetName(), capital = city:IsCapital(),
            puppet = city:IsPuppet(), occupied = city:IsOccupied(),
            health = city:GetDamage() > 0 and (1 - city:GetDamage() / city:GetMaxHitPoints()) or nil,
            culture = city:GetJONSCulturePerTurn(), tourism = city:GetBaseTourism(),
            tourism_tooltip = city:GetTourismTooltip(), works = works, slots_free = count - works,
            slots_tooltip = city:GetTotalSlotsTooltip(), cells = cells, extra = extra}
    end
    return rows, total_slots, total_works
end

-- A core tooltip as one line of text: markup (colours, icons) dropped, line breaks as spaces.
local function plain(text)
    return (tostring(text or ""):gsub("%[NEWLINE%]", " "):gsub("%[[^%]]*%]", ""):gsub("%s+", " "):gsub("^ ", ""):gsub(" $", ""))
end
local levels = {"unknown", "exotic", "familiar", "popular", "influential", "dominant"}
local level_keys = {"TXT_KEY_CO_UNKNOWN", "TXT_KEY_CO_EXOTIC", "TXT_KEY_CO_FAMILIAR", "TXT_KEY_CO_POPULAR", "TXT_KEY_CO_INFLUENTIAL", "TXT_KEY_CO_DOMINANT"}
local function influence(player, source)
    local p, rows = Players[source], array()
    for id = 0, GameDefines.MAX_MAJOR_CIVS - 1 do
        local target = Players[id]
        if id ~= source and target:IsAlive() then
            local known = id == player or met(player, id)
            local ref = known and civ(id) or {}
            local accumulated = p:GetInfluenceOn(id)
            local culture = target:GetJONSCultureEverGenerated()
            local level = math.max(0, math.min(5, p:GetInfluenceLevel(id)))
            local turns = p:GetTurnsToInfluential(id)
            local trend, trend_name
            if level < 5 then
                local value = p:GetInfluenceTrend(id)
                trend = value < 0 and "falling" or value == 0 and "static" or turns == 999 and "rising_slowly" or "rising"
                trend_name = L("TXT_KEY_CO_" .. string.upper(trend))
            end
            local level_name = L(level_keys[level + 1])
            -- CultureOverview's tooltips: the share of the target's lifetime culture, what the next level needs and
            -- the bonuses the current one gives; the turns to influential while influence rises.
            local share = culture > 0 and accumulated / culture or 0
            local share_tip = known
                and L("TXT_KEY_CO_THIRD_PARTY_CULTURE_INFLUENCE", p:GetCivilizationShortDescriptionKey(), share * 100, target:GetCivilizationShortDescriptionKey())
                or L("TXT_KEY_CO_THIRD_PARTY_CULTURE_INFLUENCE_UNMET", p:GetCivilizationShortDescriptionKey(), share * 100)
            local next_level = ({[0] = {"TXT_KEY_CO_EXOTIC", "CULTURE_LEVEL_EXOTIC"}, {"TXT_KEY_CO_FAMILIAR", "CULTURE_LEVEL_FAMILIAR"},
                {"TXT_KEY_CO_POPULAR", "CULTURE_LEVEL_POPULAR", "TXT_KEY_CO_INFLUENCE_BONUSES_FAMILIAR"},
                {"TXT_KEY_CO_INFLUENTIAL", "CULTURE_LEVEL_INFLUENTIAL", "TXT_KEY_CO_INFLUENCE_BONUSES_POPULAR"},
                {"TXT_KEY_CO_DOMINANT", "CULTURE_LEVEL_DOMINANT", "TXT_KEY_CO_INFLUENCE_BONUSES_INFLUENTIAL"}})[level]
            local level_tip
            if next_level then
                level_tip = L("TXT_KEY_CO_TOURISM_PERCENT_TT", share_tip, L(next_level[1]), GameDefines[next_level[2]])
                if next_level[3] then level_tip = level_tip .. L(next_level[3]) end
            else
                level_tip = L"TXT_KEY_CO_INFLUENCE_BONUSES_DOMINANT"
            end
            local trend_tip = ""
            if trend == "rising" and level < 4 then trend_tip = L("TXT_KEY_CO_INFLUENTIAL_TURNS_TT", turns) end
            rows[#rows + 1] = {id = id, known = known, name = ref.name, civ_name = ref.civ_name,
                civ = ref.civ, color_bg = ref.color_bg, color_fg = ref.color_fg,
                level = levels[level + 1], level_name = level_name, level_tooltip = plain(level_tip),
                share = share, tourism_accum = accumulated,
                culture_accum = known and culture or nil, bars_tooltip = plain(share_tip),
                modifier_pct = p:GetTourismModifierWith(id), modifier_tooltip = p:GetTourismModifierWithTooltip(id),
                per_turn = math.floor(p:GetInfluencePerTurn(id)), trend = trend, trend_name = trend_name,
                trend_tooltip = plain(trend_tip), turns_to_influential = level < 4 and turns >= 0 and turns < 999 and turns or nil}
        end
    end
    return rows
end

local function data(player)
    local option = GameInfoTypes.GAMEOPTION_NO_CULTURE_OVERVIEW
    assert(not option or not Game.IsOption(option), "Culture overview is disabled for this game.")
    local p = Players[player]
    local catalog, columns = buildings(player), array()
    for _, b in ipairs(catalog) do
        if not b.extra then columns[#columns + 1] = {key = b.key, building = "building:" .. b.info.Type,
            name = L(b.info.Description), slot_type = b.slot_type, slots = b.info.GreatWorkCount,
            tooltip = L(b.info.Description)} end
    end
    local cities, total_slots, total_works = city_rows(player, catalog)
    local own_offers, partners, victory, sources = array(), array(), array(), array()
    for _, entry in ipairs(offers) do
        local options = array()
        for _, row in ipairs(p:GetGreatWorks(entry[1])) do
            local item = work(row.Index, player)
            item.theme_bonus = Game.GetGreatWorkCurrentThemingBonus(row.Index)
            for _, city in ipairs(cities) do
                for _, cell in ipairs(city.cells) do
                    for _, slot in ipairs(cell.slots) do
                        if slot.work and slot.work.id == row.Index then item.location = city.name .. ", " .. L(GameInfo.BuildingClasses[cell.column].Description) end
                    end
                end
                for _, extra in ipairs(city.extra) do
                    for _, slot in ipairs(extra.slots) do
                        if slot.work and slot.work.id == row.Index then item.location = city.name .. ", " .. extra.name end
                    end
                end
            end
            options[#options + 1] = item
        end
        own_offers[#own_offers + 1] = {class = entry[2], class_id = entry[1], work = work(p[entry[3]](p), player), options = options}
    end
    for id = 0, GameDefines.MAX_MAJOR_CIVS - 1 do
        local other = Players[id]
        if other:IsAlive() and (id == player or met(player, id)) then
            if id ~= player and not team_of(player):IsAtWar(other:GetTeam()) then
                local partner = civ(id)
                partner.human = other:IsHuman()
                for _, entry in ipairs(offers) do partner[entry[2]] = work(other[entry[3]](other), player) end
                partners[#partners + 1] = partner
            end
            local row = civ(id)
            row.you, row.tourism = id == player, other:GetTourism()
            row.influential_on, row.influential_of = other:GetNumCivsInfluentialOn(), other:GetNumCivsToBeInfluentialOn()
            local ideology = GameInfo.PolicyBranchTypes[other:GetLateGamePolicyTree()]
            row.ideology = ideology and string.lower(string.gsub(ideology.Type, "POLICY_BRANCH_", "")) or "none"
            row.ideology_name = ideology and L(ideology.Description) or nil
            if ideology then
                local opinions = {"TXT_KEY_CO_PUBLIC_OPINION_CONTENT", "TXT_KEY_CO_PUBLIC_OPINION_DISSIDENTS", "TXT_KEY_CO_PUBLIC_OPINION_CIVIL_RESISTANCE", "TXT_KEY_CO_PUBLIC_OPINION_REVOLUTIONARY_WAVE"}
                row.opinion = L(opinions[other:GetPublicOpinionType() + 1] or opinions[1])
                row.opinion_tooltip = other:GetPublicOpinionTooltip()
                row.opinion_unhappiness = other:GetPublicOpinionUnhappiness()
                row.opinion_unhappiness_tooltip = other:GetPublicOpinionUnhappinessTooltip()
            end
            row.excess_happiness = other:GetExcessHappiness()
            victory[#victory + 1] = row
            local source = civ(id)
            source.tourism, source.rows = other:GetTourism(), influence(player, id)
            sources[#sources + 1] = source
        end
    end
    local antiquity
    if Map and Map.GetNumPlots and GameInfoTypes.RESOURCE_ARTIFACTS then
        antiquity = {sites = 0, hidden = 0}
        for index = 0, Map.GetNumPlots() - 1 do
            local plot = Map.GetPlotByIndex(index)
            if visible(plot, p:GetTeam()) then
                local resource = plot:GetResourceType(p:GetTeam())
                if resource == GameInfoTypes.RESOURCE_ARTIFACTS then antiquity.sites = antiquity.sites + 1
                elseif GameInfoTypes.RESOURCE_HIDDEN_ARTIFACTS and resource == GameInfoTypes.RESOURCE_HIDDEN_ARTIFACTS then antiquity.hidden = antiquity.hidden + 1 end
            end
        end
    end
    return {player = civ(player), antiquity = antiquity, summary = {works = total_works, slots = total_slots,
        tourism = p:GetTourism(), influential_on = p:GetNumCivsInfluentialOn(), influential_of = p:GetNumCivsToBeInfluentialOn()},
        columns = columns, cities = cities, swap = {offers = own_offers, partners = partners},
        victory = {rows = victory}, influence = {sources = sources}}
end

local function integer(value, label)
    assert(type(value) == "number" and value == math.floor(value), label .. " is required")
    return value
end

local function address(player, id)
    integer(id, "slot id")
    local state = seats[player]
    local slot = state and state.slots and state.slots[id]
    assert(slot, "The great work slot is no longer available. Reopen culture overview.")
    local city = Players[player]:GetCityByID(slot.city)
    assert(city and city:GetOwner() == player and city:IsHasBuilding(slot.building), "The slot's building is unavailable.")
    return slot, city
end

local function offered(p, class)
    for _, entry in ipairs(offers) do if entry[1] == class then return p[entry[3]](p) end end
    error("This great work class cannot be traded.")
end

return {
    data = data,
    actions = {
        move = function(player, args, net)
            local from, city = address(player, args.a)
            local to = address(player, args.b)
            assert(args.a ~= args.b, "Choose a different destination slot.")
            assert(from.slot_type == to.slot_type, "The destination accepts a different kind of great work.")
            assert(city:GetBuildingGreatWork(from.class, from.index) >= 0, "The source slot is empty.")
            net.send("sendMoveGreatWorks", from.city, from.class, from.index, to.city, to.class, to.index)
        end,
        offer = function(player, args, net)
            local class, id = integer(args.a, "great work class"), integer(args.b, "great work id")
            offered(Players[player], class)
            local owned = id == -1
            for _, row in ipairs(Players[player]:GetGreatWorks(class)) do if row.Index == id then owned = true end end
            assert(owned, "Only your own great works can be offered.")
            net.send("sendSetSwappableGreatWork", class, id)
        end,
        swap = function(player, args, net)
            local partner = integer(args.a, "partner id")
            assert(partner >= 0 and partner < GameDefines.MAX_MAJOR_CIVS and partner ~= player, "Choose another civilization.")
            assert(Players[partner]:IsAlive() and met(player, partner), "The trade partner is unavailable.")
            assert(not team_of(player):IsAtWar(Players[partner]:GetTeam()), "Great works cannot be traded during war.")
            integer(args.b, "partner great work id")
            integer(args.c, "own great work id")
            local class
            for _, entry in ipairs(offers) do
                if Players[partner][entry[3]](Players[partner]) == args.b and args.b >= 0 then class = entry[1] end
            end
            assert(class, "The partner is no longer offering this great work.")
            assert(args.c >= 0 and offered(Players[player], class) == args.c, "Offer one of your works of the same class first.")
            assert(Game.GetGreatWorkController(args.c) == player and Game.GetGreatWorkController(args.b) == partner, "Great work ownership has changed.")
            net.send("sendSwapGreatWorks", args.c, partner, args.b)
        end,
    },
}
