-- The plots a map targeting mode accepts for the seat that asks: the client's `ui.target`, the original's interface
-- modes that ask for a plot. args.kind: 0 a unit's order (args.id the unit, args.action its row of GameInfoActions,
-- -1 its ranged attack), 1 a city's ranged strike (args.id the city), 2 a unit gift to a city-state and 3 an
-- improvement gift (args.id the city-state). Each mode scans the plots the original's InGame.lua highlights for it
-- (Bombardment.lua for the attacks) with the core's own check, the one its click in WorldView.lua / InGame.lua tests
-- before it sends the order. Fog: only plots the seat's team has revealed; the attacks' checks need the plot seen.
-- The action `go` gives the order on one plot after the same check (a = kind, b = id, c = action, d = x * 65536 + y).
local UNIT, CITY, GIFT_UNIT, GIFT_IMPROVEMENT = 0, 1, 2, 3

local function cell(plot) return plot:GetX() * 65536 + plot:GetY() end

-- Every plot within `range` of (x, y), as the original's highlight loops walk them.
local function around(x, y, range, f)
    for dx = -range, range do
        for dy = -range, range do
            local plot = Map.PlotXYWithRangeCheck(x, y, dx, dy, range)
            if plot then f(plot) end
        end
    end
end

local function every_plot(f)
    for i = 0, Map.GetNumPlots() - 1 do f(Map.GetPlotByIndex(i)) end
end

-- Bombardment.lua's radius of a ranged attack: the plots in range the team sees, in line of sight unless the shooter
-- fires indirectly, in its own domain when it may only shoot there.
local function fire_area(from, range, team, indirect, domain)
    return function(plot)
        if not plot:IsVisible(team, false) then return false end
        if not indirect and not from:CanSeePlot(plot, team, range - 1, -1) then return false end
        if domain == DomainTypes.DOMAIN_LAND and plot:IsWater() then return false end
        if domain == DomainTypes.DOMAIN_SEA and not plot:IsWater() then return false end
        return true
    end
end

-- The unit modes: the highlight look, the radius, the check of a plot and the radius drawn around the targets.
local function rebase_range(u) return math.floor(u:Range() * GameDefines.AIR_UNIT_REBASE_RANGE_MULTIPLIER / 100) end
local MODES = {
    INTERFACEMODE_RANGE_ATTACK = {look = "fire", range = function(u) return u:Range() end,
        ok = function(u, plot) return u:CanRangeStrikeAt(plot:GetX(), plot:GetY(), true, true) end,
        area = function(u, team)
            return fire_area(u:GetPlot(), u:Range(), team, u:IsRangeAttackIgnoreLOS(),
                u:IsRangeAttackOnlyInDomain() and u:GetDomainType() or -1)
        end},
    INTERFACEMODE_AIRSTRIKE = {look = "air", range = function(u) return u:Range() end,
        ok = function(u, plot) return u:CanRangeStrikeAt(plot:GetX(), plot:GetY(), true, true) end,
        area = function() return function() return true end end},
    INTERFACEMODE_NUKE = {look = "air", range = function(u) return u:Range() end,
        ok = function(u, plot) return u:CanNukeAt(plot:GetX(), plot:GetY()) end,
        area = function() return function() return true end end},
    INTERFACEMODE_AIR_SWEEP = {look = "sweep", range = function(u) return u:Range() end,
        ok = function(u, plot) return u:CanStartMission(GameInfoTypes.MISSION_AIR_SWEEP, plot:GetX(), plot:GetY(), u:GetPlot(), 0) end,
        area = function() return function() return true end end},
    INTERFACEMODE_REBASE = {look = "move", range = rebase_range,
        ok = function(u, plot) return u:CanRebaseAt(u:GetPlot(), plot:GetX(), plot:GetY()) end},
    INTERFACEMODE_PARADROP = {look = "move", range = function(u) return u:GetDropRange() end,
        ok = function(u, plot) return u:CanParadropAt(u:GetPlot(), plot:GetX(), plot:GetY()) end},
    INTERFACEMODE_AIRLIFT = {look = "move",
        ok = function(u, plot) return u:CanAirliftAt(u:GetPlot(), plot:GetX(), plot:GetY()) end},
    INTERFACEMODE_EMBARK = {look = "mark", range = function() return 1 end,
        ok = function(u, plot) return not u:AtPlot(plot) and u:CanEmbarkOnto(u:GetPlot(), plot) end},
    INTERFACEMODE_DISEMBARK = {look = "mark", range = function() return 1 end,
        ok = function(u, plot) return not u:AtPlot(plot) and u:CanDisembarkOnto(plot) end},
    -- InGame.lua's AttackIntoTile: a seen plot with a visible enemy unit or an enemy city, reached by a move.
    INTERFACEMODE_ATTACK = {look = "attack",
        range = function(u) return math.max(1, math.ceil(u:MovesLeft() / GameDefines.MOVE_DENOMINATOR)) end,
        ok = function(u, plot)
            return not u:AtPlot(plot) and plot:IsVisible(u:GetTeam(), false)
                and (plot:IsVisibleEnemyUnit(u:GetOwner()) or plot:IsEnemyCity(u))
        end},
    -- No highlight in the original: any plot, the core finds the route when the order comes.
    INTERFACEMODE_ROUTE_TO = {look = "move", any = true,
        ok = function(u, plot) return u:CanStartMission(GameInfoTypes.MISSION_ROUTE_TO, plot:GetX(), plot:GetY(), u:GetPlot(), 0) end},
}

local function mode_name(type_name)
    local row = GameInfo.InterfaceModes[type_name]
    return row and plain(L(row.Description)) or ""
end

-- The unit's row of GameInfoActions for args.action: -1 is its ranged attack (an air unit's is the air strike).
local function unit_action(u, action)
    if action == -1 then
        local want = u:GetDomainType() == DomainTypes.DOMAIN_AIR and "INTERFACEMODE_AIRSTRIKE" or "INTERFACEMODE_RANGE_ATTACK"
        local id = 0
        while GameInfoActions[id] do
            if GameInfoActions[id].Type == want then return id, GameInfoActions[id] end
            id = id + 1
        end
        return nil
    end
    local a = GameInfoActions[action]
    if a and type(a.Type) == "string" and MODES[a.Type] then return action, a end
    return nil
end

-- What acts and how it judges a plot: {name, look, range (nil: the whole map), x, y, ok(plot), area(plot), any,
-- send(net, plot)}, or nil and why.
local function mode(player, kind, id, action)
    local p = Players[player]
    if kind == UNIT then
        local u = id and p:GetUnitByID(id)
        if not u or u:GetOwner() ~= player or u:IsDelayedDeath() then return nil, "The unit is unavailable." end
        local row, a = unit_action(u, action)
        if not row then return nil, "The unit has no such order." end
        local m = MODES[a.Type]
        local mission = GameInfoTypes[(GameInfo.InterfaceModes[a.Type] or {}).Mission or ""]
        return {name = mode_name(a.Type), look = m.look, range = m.range and m.range(u), x = u:GetX(), y = u:GetY(),
            any = m.any, ok = function(plot) return m.ok(u, plot) end, area = m.area and m.area(u, p:GetTeam()),
            send = function(net, plot) net.send("sendPushMission", u:GetID(), mission, plot:GetX(), plot:GetY(), 0, 0) end}
    elseif kind == CITY then
        local c = id and p:GetCityByID(id)
        if not c or c:GetOwner() ~= player then return nil, "The city is unavailable." end
        local range = GameDefines.CITY_ATTACK_RANGE
        return {name = mode_name("INTERFACEMODE_CITY_RANGE_ATTACK"), look = "fire", range = range, x = c:GetX(), y = c:GetY(),
            -- Two arguments: a third one would ask only whether the strike is ever possible.
            ok = function(plot) return c:CanRangeStrikeAt(plot:GetX(), plot:GetY()) end,
            area = fire_area(c:Plot(), range, p:GetTeam(), GameDefines.CAN_CITY_USE_INDIRECT_FIRE == 1, -1),
            send = function(net, plot) net.send("sendDoTask", c:GetID(), TaskTypes.TASK_RANGED_ATTACK, plot:GetX(), plot:GetY(), 0, 0, 0, 0) end}
    elseif kind == GIFT_UNIT or kind == GIFT_IMPROVEMENT then
        local m = id and Players[id]
        if not m or not m:IsMinorCiv() or not m:IsAlive() or not met(player, id) then return nil, "This city-state is unavailable." end
        if kind == GIFT_UNIT then
            -- InGame.lua's HighlightGiftUnits / GiftUnit: the seat's units that may go to the city-state.
            local function giftable(plot)
                local found
                for i = 0, plot:GetNumUnits() - 1 do
                    local u = plot:GetUnit(i)
                    if u and u:GetOwner() == player and u:CanDistanceGift(id) then found = u end
                end
                return found
            end
            return {name = mode_name("INTERFACEMODE_GIFT_UNIT"), look = "mark", units = true,
                ok = function(plot) return giftable(plot) ~= nil end,
                send = function(net, plot) net.send("sendGiftUnit", id, giftable(plot):GetID()) end}
        end
        local capital = m:GetCapitalCity()
        if not capital then return nil, "This city-state has no capital." end
        return {name = mode_name("INTERFACEMODE_GIFT_TILE_IMPROVEMENT"), look = "mark",
            range = GameDefines.MINOR_CIV_RESOURCE_SEARCH_RADIUS, x = capital:GetX(), y = capital:GetY(),
            ok = function(plot) return m:CanMajorGiftTileImprovementAtPlot(player, plot:GetX(), plot:GetY()) end,
            send = function(net, plot) net.send("sendMinorGiftTileImprovement", player, id, plot:GetX(), plot:GetY()) end}
    end
    return nil, "Unknown targeting mode."
end

local function int(v) return type(v) == "number" and v == math.floor(v) and v or nil end

return {
    data = function(player, args)
        local kind, id, action = int(args.kind), int(args.id), int(args.action) or 0
        local out = {id = id or -1, action = action, name = "", look = "fire", plots = array(), area = array(), any = false}
        local m, why = mode(player, kind, id, action)
        if not m then out.reason = why return out end
        out.name, out.look, out.any = m.name, m.look, m.any == true
        if m.any then return out end
        local team = Players[player]:GetTeam()
        local function visit(plot)
            if not plot:IsRevealed(team, false) then return end
            if m.ok(plot) then out.plots[#out.plots + 1] = {cell = cell(plot), x = plot:GetX(), y = plot:GetY()} end
            if m.area and m.area(plot) then out.area[#out.area + 1] = {cell = cell(plot), x = plot:GetX(), y = plot:GetY()} end
        end
        if m.units then
            local seen = {}
            for u in Players[player]:Units() do
                local plot = u:GetPlot()
                if plot and not seen[cell(plot)] then seen[cell(plot)] = true visit(plot) end
            end
        elseif m.range then
            around(m.x, m.y, m.range, visit)
        else
            every_plot(visit)
        end
        if #out.plots == 0 then out.reason = "No plot can be chosen now." end
        return out
    end,
    actions = {
        go = function(player, args, net)
            assert(Players[player]:IsTurnActive(), "Wait for your turn.")
            local m, why = mode(player, int(args.a), int(args.b), int(args.c) or 0)
            assert(m, why)
            local d = assert(int(args.d), "a plot is required")
            local plot = Map.GetPlot(math.floor(d / 65536), d % 65536)
            assert(plot and plot:IsRevealed(Players[player]:GetTeam(), false), "The plot is unknown.")
            assert(m.ok(plot), "The order cannot target this plot.")
            m.send(net, plot)
        end,
    },
}
