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
local function target(player, id)
    assert(type(id) == "number" and id == math.floor(id) and id >= 0 and id < GameDefines.MAX_MAJOR_CIVS,
        "a valid espionage target is required")
    assert(Players[id] and id ~= player and met(player, id), "espionage target is unavailable")
    return Players[id]
end
local function spy_city(p, target_id)
    local found
    for _, spy in ipairs(p:GetEspionageSpies()) do
        if spy.State == "TXT_KEY_SPY_STATE_GATHERING_INTEL" and not spy.IsDiplomat and spy.CityX >= 0 and spy.CityY >= 0 then
            local plot = Map.GetPlot(spy.CityX, spy.CityY)
            local city = plot and plot:IsRevealed(p:GetTeam(), false) and plot:GetPlotCity()
            if city and city:GetOwner() == target_id then
                -- SIMPLIFIED(SIMP-342): the steal request names the target player, so omit the city if multiple spies qualify.
                if found then return "" end
                found = city:GetName()
            end
        end
    end
    return found or ""
end
local function data(player, args)
    local p, team = Players[player], team_of(player)
    local tt, rate = team:GetTeamTechs(), p:GetScience()
    local free = p:GetNumFreeTechs()
    local mode = args.a == 3 and "steal" or ((args.a == 2 or args.a == 1) and free > 0 and "free" or "normal")
    local steal
    if mode == "steal" then
        local other = target(player, args.b)
        local civ = GameInfo.Civilizations[other:GetCivilizationType()]
        local primary = other:GetPlayerColors()
        local color = string.format("#%02x%02x%02x", math.floor(primary.x * 255 + .5),
            math.floor(primary.y * 255 + .5), math.floor(primary.z * 255 + .5))
        steal = {player = args.b, name = L(GameInfo.Leaders[other:GetLeaderType()].Description),
            civ = L(civ.Description), icon = icon(civ.IconAtlas, civ.PortraitIndex), color = color,
            spy_city = spy_city(p, args.b), can_steal = p:GetNumTechsToSteal(args.b)}
    end
    local rows, queue, eras, links, bykey, byera = array(), array(), array(), array(), {}, {}
    local researched, cols, gridrows, current = 0, 0, 0, nil
    local currentEra = p:GetCurrentEra()
    local unlocks = tech_unlocks_for(player)
    for tech in GameInfo.Technologies() do
        local id, pos = tech.ID, p:GetQueuePosition(tech.ID)
        local has, can = team:IsHasTech(id), p:CanResearch(id)
        local state = has and "researched" or (p:GetCurrentResearch() == id and "current" or (can and "available" or "unavailable"))
        local clickable = not has and p:CanEverResearch(id) and not science_off()
        if not has and not p:CanEverResearch(id) then state = "locked" end
        if mode == "free" and not has then
            clickable = clickable and p:CanResearchForFree(id)
            state = clickable and "free" or "locked"
        elseif mode == "steal" and not has then
            clickable = clickable and steal.can_steal > 0 and p:canStealTech(args.b, id)
            state = clickable and "free" or "locked"
        end
        local reason = not clickable and (has and "Already researched." or (science_off() and "Science is disabled." or "Technology requirements are not met for this choice.")) or nil
        local cost, progress = p:GetResearchCost(id), tt:GetResearchProgress(id)
        local era = GameInfo.Eras[tech.Era]
        local row = {id = id, key = tech.Type, name = L(tech.Description), icon = icon(tech.IconAtlas, tech.PortraitIndex),
            col = tech.GridX, column = tech.GridX, row = tech.GridY, era_key = tech.Era, era = era.ID, era_name = L(era.Description),
            state = state, status = has and "researched" or (p:GetCurrentResearch() == id and "current" or (pos > 0 and "queued" or (can and "available" or "locked"))),
            queue_position = pos, queue_pos = pos > 0 and pos or nil, turns = not has and turns(p, id, rate) or nil,
            cost = cost, progress = progress, fraction = fraction(progress, cost), forecast_fraction = fraction(progress + rate, cost),
            clickable = clickable, lock_reason = reason, help = tech.Help and L(tech.Help) or "",
            prerequisites = array(), prereqs = array(), or_prereqs = array(), leads_to = array(), unlocks = unlocks(tech, true)}
        if mode == "steal" and clickable then row.steal_science = p:ScienceToStealAmount(args.b, id) end
        rows[#rows + 1], bykey[tech.Type] = row, row
        cols, gridrows = math.max(cols, tech.GridX + 1), math.max(gridrows, tech.GridY + 1)
        if has then researched = researched + 1 end
        local er = byera[tech.Era]
        if not er then
            er = {key = tech.Era, name = L(era.Description), first_col = tech.GridX, cols = 1,
                state = era.ID < currentEra and "past" or (era.ID == currentEra and "current" or "future"), researched = 0, total = 0}
            byera[tech.Era], eras[#eras + 1] = er, er
        end
        local last = math.max(er.first_col + er.cols - 1, tech.GridX)
        er.first_col = math.min(er.first_col, tech.GridX); er.cols = last - er.first_col + 1
        er.total = er.total + 1; if has then er.researched = er.researched + 1 end
        if pos > 0 then queue[#queue + 1] = {key = tech.Type, pos = pos, name = row.name, turns = row.turns} end
        if p:GetCurrentResearch() == id then current = {key = tech.Type, name = row.name, icon = row.icon, turns = row.turns,
            progress = progress, cost = cost, fraction = row.fraction, forecast_fraction = row.forecast_fraction, turn_fraction = fraction(rate, cost)} end
    end
    local queued_or = {}
    for _, spec in ipairs({{"Technology_PrereqTechs", "and", "prereqs"}, {"Technology_ORPrereqTechs", "or", "or_prereqs"}}) do
        for link in GameInfo[spec[1]]() do
            local from, to = bykey[link.PrereqTech], bykey[link.TechType]
            if from and to then
                to[spec[3]][#to[spec[3]] + 1] = {key = from.key, name = from.name}
                if spec[2] == "and" then to.prerequisites[#to.prerequisites + 1] = {id = from.id} end
                local found = false
                for _, next in ipairs(from.leads_to) do if next.key == to.key then found = true end end
                if not found then from.leads_to[#from.leads_to + 1] = {key = to.key, name = to.name} end
                local edge = {key = from.key .. ":" .. to.key .. ":" .. spec[2], from_col = from.col, from_row = from.row,
                    to_col = to.col, to_row = to.row, kind = spec[2],
                    state = to.status == "researched" and "done" or (from.status == "researched" and "live" or "idle"),
                    queued = false}
                links[#links + 1] = edge
                if to.queue_pos then
                    local earlier = from.queue_pos and from.queue_pos < to.queue_pos
                    if spec[2] == "and" and earlier then
                        edge.queued = true
                    elseif spec[2] == "or" then
                        -- A researched alternative already meets the requirement. Otherwise the first queued
                        -- alternative to complete meets it, even when more than one is in the queue.
                        local pos = from.status == "researched" and 0 or (earlier and from.queue_pos or nil)
                        local best = queued_or[to.key]
                        if pos and (not best or pos < best.pos) then
                            queued_or[to.key] = {pos = pos, edge = edge}
                        end
                    end
                end
            end
        end
    end
    for _, alternative in pairs(queued_or) do
        if alternative.pos > 0 then alternative.edge.queued = true end
    end
    table.sort(rows, function(a,b) if a.col ~= b.col then return a.col < b.col end; if a.row ~= b.row then return a.row < b.row end; return a.id < b.id end)
    table.sort(eras, function(a,b) return a.first_col < b.first_col end)
    table.sort(queue, function(a,b) return a.pos < b.pos end)
    local era = GameInfo.Eras[currentEra]
    return {techs = rows, mode = mode, opened_by = (args.a or 0) > 0 and "request" or "menu", science_off = science_off(),
        science_rate = rate, researched_count = researched, total_count = #rows, era = {key = era.Type, name = L(era.Description)},
        free_count = free, free_reason = mode == "free" and "Choose a free technology." or nil, steal = steal,
        grid = {cols = cols, rows = gridrows}, current = current, queue = queue, eras = eras, links = links}
end
return {
    data = data,
    actions = {
        research = function(player, args, net)
            assert(type(args.a) == "number" and args.a == math.floor(args.a), "technology id is required")
            local p = Players[player]
            assert(GameInfo.Technologies[args.a] and not team_of(player):IsHasTech(args.a), "technology is unavailable")
            assert(not science_off() and p:CanEverResearch(args.a), "technology cannot be researched")
            assert(args.b == nil or args.b == 0 or args.b == 1, "queue flag must be 0 or 1")
            local free = p:GetNumFreeTechs()
            if free > 0 then assert(p:CanResearchForFree(args.a), "technology cannot be chosen for free") end
            net.send("sendResearch", args.a, free, -1, args.b or 0)
            if args.popup_id then net.popup_done(args.popup_id) end
        end,
        steal = function(player, args, net)
            target(player, args.b)
            assert(type(args.a) == "number" and GameInfo.Technologies[args.a], "technology id is required")
            local p = Players[player]
            assert(not science_off() and not team_of(player):IsHasTech(args.a) and p:GetNumTechsToSteal(args.b) > 0
                and p:canStealTech(args.b, args.a), "technology cannot be stolen")
            net.send("sendResearch", args.a, 0, args.b, 0)
            if args.popup_id then net.popup_done(args.popup_id) end
        end,
    },
}
