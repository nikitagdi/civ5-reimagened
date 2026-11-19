local function color(key)
    local c = GameInfo.Colors[key]
    return math.floor(c.Red * 255) * 16777216 + math.floor(c.Green * 255) * 65536 + math.floor(c.Blue * 255) * 256 + 255
end
local function civ(player)
    local p = Players[player]
    local info = GameInfo.Civilizations[p:GetCivilizationType()]
    local colors = GameInfo.PlayerColors[info.DefaultPlayerColor]
    return { player = player, icon = "civ:" .. info.Type, color_bg = color(colors.PrimaryColor), color_fg = color(colors.SecondaryColor), owner_name = L(info.ShortDescription) }
end
local function endpoint(city)
    local row = civ(city:GetOwner())
    row.city_id, row.city = city:GetID(), city:GetName()
    return row
end
local function known(player, city)
    return city and (city:GetOwner() == player or (met(player, city:GetOwner()) and city:Plot():IsRevealed(Players[player]:GetTeam(), false)))
end
local function religion(id, pressure)
    if not id or id < 0 or not pressure or pressure <= 0 then return nil end
    local info = GameInfo.Religions[id]
    if not info or not pressure or pressure <= 0 then return nil end
    return { key = info.Type, name = L(info.Description), icon = icon(info.IconAtlas, info.PortraitIndex), pressure = pressure }
end
-- The original's leader label in a route tooltip: the nickname in a network game, else the leader name.
local function leader_name(id)
    local p = Players[id]
    local nick = p:GetNickName()
    if nick and nick ~= "" and Game.IsNetworkMultiPlayer() then return nick end
    return p:GetName()
end
local function line(lines, id, key, value, arg)
    lines[#lines + 1] = {id = id, key = key, value = value, arg = arg}
end
-- TradeRouteHelpers' BuildTradeRouteToolTipString, as structured blocks: the origin owner's gold components and total,
-- the destination owner's revenue, then each side's science from tech difference and cultural influence.
-- Revenue of the seat is "my_*", of the other side "their_*", as the original's "your"/"their" headers.
local function route_tooltip(player, from, to, domain)
    local origin, dest = from:GetOwner(), to:GetOwner()
    local p, q = Players[origin], Players[dest]
    if p:GetInternationalTradeRouteTotal(from, to, domain, true) <= 0 then return nil end
    local blocks = array()
    local mine = origin == player
    local lines = array()
    line(lines, "base", "base", p:GetInternationalTradeRouteBaseBonus(from, to, true) / 100)
    line(lines, "origin_city_gold", "origin_city_gold", p:GetInternationalTradeRouteGPTBonus(from, to, true) / 100, from:GetName())
    line(lines, "dest_city_gold", "dest_city_gold", p:GetInternationalTradeRouteGPTBonus(from, to, false) / 100, to:GetName())
    local n = p:GetInternationalTradeRoutePolicyBonus(from, to, domain)
    if n ~= 0 then line(lines, "policies", "policies", n / 100) end
    n = p:GetInternationalTradeRouteYourBuildingBonus(from, to, domain, true)
    if n ~= 0 then line(lines, "buildings:origin", "buildings", n / 100, from:GetName()) end
    n = p:GetInternationalTradeRouteTheirBuildingBonus(from, to, domain, true)
    if n ~= 0 then line(lines, "buildings:dest", "buildings", n / 100, to:GetName()) end
    local per_resource = 50 * (100 + p:GetInternationalTradeRouteResourceTraitModifier()) / 100 / 100
    for info in GameInfo.Resources() do
        local usage = Game.GetResourceUsageType(info.ID)
        if (usage == ResourceUsageTypes.RESOURCEUSAGE_LUXURY or usage == ResourceUsageTypes.RESOURCEUSAGE_STRATEGIC)
            and from:IsHasResourceLocal(info.ID) ~= to:IsHasResourceLocal(info.ID) then
            line(lines, "resource:" .. info.Type, "different_resources", per_resource, L(info.Description))
        end
    end
    n = p:GetInternationalTradeRouteExclusiveBonus(from, to)
    if n ~= 0 then line(lines, "exclusive_connection", "exclusive_connection", n / 100) end
    n = p:GetInternationalTradeRouteOtherTraitBonus(from, to, domain, true)
    if n ~= 0 then line(lines, "other_trait", "other_trait", n / 100, L(q:GetCivilizationAdjectiveKey())) end
    n = p:GetInternationalTradeRouteRiverModifier(from, to, domain, true)
    if n ~= 0 then line(lines, "river", "river", n) end
    local sea = domain == DomainTypes.DOMAIN_SEA and p:GetInternationalTradeRouteDomainModifier(domain) or 0
    if sea ~= 0 then line(lines, "sea_route_mult", "sea_route_mult", (sea + 100) / 100) end
    blocks[#blocks + 1] = {key = mine and "my_gold" or "their_gold", owner = leader_name(origin),
        total = p:GetInternationalTradeRouteTotal(from, to, domain, true) / 100, lines = lines}
    local tradee = q:GetInternationalTradeRouteTotal(from, to, domain, false)
    if tradee ~= 0 then
        lines = array()
        n = q:GetInternationalTradeRouteBaseBonus(from, to, false)
        if n ~= 0 then line(lines, "base", "base", n / 100) end
        n = p:GetInternationalTradeRouteRiverModifier(from, to, domain, false)
        if n ~= 0 then line(lines, "river", "river", n) end
        if sea ~= 0 then line(lines, "sea_route_mult", "sea_route_mult", (sea + 100) / 100) end
        n = q:GetInternationalTradeRouteTheirBuildingBonus(from, to, domain, false)
        if n ~= 0 then line(lines, "buildings:dest", "buildings", n / 100, to:GetName()) end
        blocks[#blocks + 1] = {key = mine and "their_gold" or "my_gold", owner = leader_name(dest), total = tradee / 100, lines = lines}
    end
    for _, side in ipairs({{origin, dest, true}, {dest, origin, false}}) do
        local who, other, is_origin = side[1], side[2], side[3]
        local science = Players[who]:GetInternationalTradeRouteScience(from, to, domain, is_origin) / 100
        if science > 0 then
            lines = array()
            line(lines, "tech_difference", "tech_difference", Players[who]:GetNumTechDifference(other), leader_name(other))
            line(lines, "cultural_influence", "cultural_influence", Players[who]:GetInfluenceTradeRouteScienceBonus(other))
            blocks[#blocks + 1] = {key = (is_origin == mine) and "my_science" or "their_science", owner = leader_name(who), total = science, lines = lines}
        end
    end
    return {blocks = blocks}
end
local function routes(player, list, reverse)
    local rows = array()
    for _, raw in ipairs(list) do
        if known(player, raw.FromCity) and known(player, raw.ToCity) then
            local row = {
                id = (raw.FromCity:Plot():GetPlotIndex() * Map.GetNumPlots() + raw.ToCity:Plot():GetPlotIndex()) * 6 + (raw.Domain == DomainTypes.DOMAIN_SEA and 3 or 0) + (raw.ToFood > 0 and 1 or (raw.ToProduction > 0 and 2 or 0)), domain = raw.Domain == DomainTypes.DOMAIN_SEA and "sea" or "land",
                from = endpoint(raw.FromCity), to = endpoint(raw.ToCity),
                my_gold = (reverse and raw.ToGPT or raw.FromGPT) / 100,
                their_gold = (reverse and raw.FromGPT or raw.ToGPT) / 100,
                my_science = (reverse and raw.ToScience or raw.FromScience) / 100,
                their_science = (reverse and raw.FromScience or raw.ToScience) / 100,
                food = raw.ToFood / 100, production = raw.ToProduction / 100,
                my_religion = religion(reverse and raw.FromReligion or raw.ToReligion, reverse and raw.FromPressure or raw.ToPressure),
                their_religion = religion(reverse and raw.ToReligion or raw.FromReligion, reverse and raw.ToPressure or raw.FromPressure),
            }
            if raw.TurnsLeft and raw.TurnsLeft >= 0 then row.turns_left = raw.TurnsLeft end
            row.tooltip = route_tooltip(player, raw.FromCity, raw.ToCity, raw.Domain)
            if row.tooltip then row.tooltip.route_id = row.id end
            rows[#rows + 1] = row
        end
    end
    return rows
end
local function own_unit(player, id)
    assert(type(id) == "number" and id == math.floor(id), "unit id is required")
    local u = assert(Players[player]:GetUnitByID(id), "unit is unavailable")
    assert(u:GetOwner() == player and not u:IsDelayedDeath(), "unit is not owned by this seat")
    return u
end
local function unit_data(player, unit, home)
    local city = unit:GetPlot():GetPlotCity()
    assert(city and city:GetOwner() == player, "unit must be in an owned city")
    local row = { id = unit:GetID(), name = unit:GetName(), icon = "unit:" .. GameInfo.Units[unit:GetUnitType()].Type,
        domain = unit:GetDomainType() == DomainTypes.DOMAIN_SEA and "sea" or "land" }
    if home then row.city_id, row.city = city:GetID(), city:GetName()
    else row.home_city_id, row.home_city, row.range = city:GetID(), city:GetName(), Players[player]:GetTradeRouteRange(unit:GetDomainType(), city) end
    return row
end
local function destinations(player, unit)
    local groups = array({{key = "yours", items = array()}, {key = "majors", items = array()}, {key = "city_states", items = array()}})
    for _, raw in ipairs(Players[player]:GetPotentialInternationalTradeRouteDestinations(unit)) do
        local plot = Map.GetPlot(raw.X, raw.Y)
        local city = plot and plot:GetPlotCity()
        if known(player, city) then
            local owner = city:GetOwner()
            local idx = owner == player and 1 or (Players[owner]:IsMinorCiv() and 3 or 2)
            local row = civ(owner)
            row.id = plot:GetPlotIndex() * 3 + raw.TradeConnectionType
            row.location = { x = raw.X, y = raw.Y }
            row.group, row.city_id, row.city, row.plot = groups[idx].key, city:GetID(), city:GetName(), plot:GetPlotIndex()
            row.connection_id = raw.TradeConnectionType
            row.connection = raw.TradeConnectionType == 1 and "food" or (raw.TradeConnectionType == 2 and "production" or "international")
            row.mine, row.theirs = {}, {}
            for _, name in ipairs({"food", "production", "gold", "science", "culture", "faith"}) do
                local yield = YieldTypes["YIELD_" .. string.upper(name)]
                local values = raw.Yields[yield + 1]
                row.mine[name], row.theirs[name] = values.Mine / 100, values.Theirs / 100
            end
            row.pressure_out = religion(raw.ToReligion, raw.ToPressureAmount)
            row.pressure_in = religion(raw.FromReligion, raw.FromPressureAmount)
            row.previous = raw.OldTradeRoute
            groups[idx].items[#groups[idx].items + 1] = row
        end
    end
    return groups
end
local function homes(player, unit, admiral)
    local p, rows = Players[player], array()
    local list = admiral and p:GetPotentialAdmiralNewPort(unit) or p:GetPotentialTradeUnitNewHomeCity(unit)
    for _, raw in ipairs(list) do
        local plot = Map.GetPlot(raw.X, raw.Y)
        local city = plot and plot:GetPlotCity()
        if city and city:GetOwner() == player then
            local row = civ(player)
            row.owner_name = nil
            row.id, row.name, row.plot = city:GetID(), city:GetName(), plot:GetPlotIndex()
            row.location = { x = raw.X, y = raw.Y }
            rows[#rows + 1] = row
        end
    end
    return rows
end
local function rehome(player, args, net, admiral)
    local unit = own_unit(player, args.a)
    assert(admiral or unit:IsTrade(), "a trade unit is required")
    local found
    for _, city in ipairs(homes(player, unit, admiral)) do if city.plot == args.b then found = true end end
    assert(found, "the destination is no longer a legal home city or port")
    local plot = assert(Map.GetPlotByIndex(args.b), "destination plot is unavailable")
    local mission = admiral and GameInfoTypes.MISSION_CHANGE_ADMIRAL_PORT or GameInfoTypes.MISSION_CHANGE_TRADE_UNIT_HOME_CITY
    assert(unit:CanStartMission(mission, plot:GetX(), plot:GetY(), nil, 0), "the unit cannot change city now")
    net.send("sendPushMission", args.a, mission, plot:GetX(), plot:GetY(), 0, 0)
    if args.popup_id then net.popup_done(args.popup_id) end
    return {close=true,open={screen="trade_routes",a=0}}
end
return {
    data = function(player, args)
        local p = Players[player]
        local yours = routes(player, p:GetTradeRoutes(), false)
        local available = routes(player, p:GetTradeRoutesAvailable(), false)
        local incoming = routes(player, p:GetTradeRoutesToYou(), true)
        local total = { used = p:GetNumInternationalTradeRoutesUsed(), capacity = p:GetNumInternationalTradeRoutesAvailable(), my_gold = 0, my_science = 0 }
        for _, row in ipairs(yours) do total.my_gold = total.my_gold + row.my_gold; total.my_science = total.my_science + row.my_science end
        local who = civ(player)
        who.name, who.civ_name = L(GameInfo.Leaders[p:GetLeaderType()].Description), who.owner_name
        who.owner_name = nil
        local root = { player = who, tab = args.a == 1 and "available" or (args.a == 2 and "to_you" or "yours"),
            counts = { yours = #yours, available = #available, to_you = #incoming }, totals = total, routes = { yours = yours, available = available, to_you = incoming } }
        if args.b and args.b >= 0 then
            local unit = own_unit(player, args.b)
            if args.a == 3 or args.a == 4 then
                root.new_home = { kind = args.a == 4 and "admiral_port" or "trade_home", unit = unit_data(player, unit, true), cities = homes(player, unit, args.a == 4) }
            else
                assert(unit:IsTrade(), "a trade unit is required")
                root.choose = { unit = unit_data(player, unit), sort = "city", groups = destinations(player, unit) }
            end
        end
        return root
    end,
    actions = {
        establish = function(player, args, net)
            local unit = own_unit(player, args.a)
            assert(unit:IsTrade(), "a trade unit is required")
            assert(args.c == 0 or args.c == 1 or args.c == 2, "connection type must be international, food or production")
            local found
            for _, group in ipairs(destinations(player, unit)) do
                for _, dest in ipairs(group.items) do if dest.plot == args.b and dest.connection_id == args.c then found = true end end
            end
            assert(found, "the route is no longer available")
            assert(unit:CanStartMission(GameInfoTypes.MISSION_ESTABLISH_TRADE_ROUTE, args.b, args.c, nil, 0), "the trade unit cannot start this route now")
            net.send("sendPushMission", args.a, GameInfoTypes.MISSION_ESTABLISH_TRADE_ROUTE, args.b, args.c, 0, 0)
            if args.popup_id then net.popup_done(args.popup_id) end
            return {close=true,open={screen="trade_routes",a=0}}
        end,
        rehome = function(player, args, net) return rehome(player, args, net, false) end,
        admiral_port = function(player, args, net) return rehome(player, args, net, true) end,
    },
}
