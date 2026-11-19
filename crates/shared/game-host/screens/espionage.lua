local function color(key)
    local c = GameInfo.Colors[key]
    return math.floor(c.Red * 255) * 16777216 + math.floor(c.Green * 255) * 65536 + math.floor(c.Blue * 255) * 256 + 255
end
local function civ(player)
    local p = Players[player]
    local info = GameInfo.Civilizations[p:GetCivilizationType()]
    local colors = GameInfo.PlayerColors[info.DefaultPlayerColor]
    return { player = player, icon = "civ:" .. info.Type, color_bg = color(colors.PrimaryColor), color_fg = color(colors.SecondaryColor) }, L(info.ShortDescription)
end
local function city_key(player, id) return player * 65536 + id end
local function spy(player, id)
    assert(type(id) == "number" and id == math.floor(id), "agent id is required")
    for _, s in ipairs(Players[player]:GetEspionageSpies()) do
        if s.AgentID == id then assert(s.State ~= "TXT_KEY_SPY_STATE_DEAD", "agent is dead"); return s end
    end
    error("agent is not available to this seat")
end
local function target(player, agent, owner, city)
    for _, c in ipairs(Players[player]:GetAvailableSpyRelocationCities(agent)) do
        if c.PlayerID == owner and c.CityID == city then
            local p = Players[owner]
            local destination = p and p:GetCityByID(city)
            if not destination then return false end
            return owner == player or (p:GetTeam() ~= Players[player]:GetTeam() and met(player, owner) and destination:Plot():IsRevealed(Players[player]:GetTeam(), false))
        end
    end
    return false
end
-- A coup staged from this window, by seat: the city-state city's name key and the turn the order went. The core
-- settles a coup when it applies the order and tells the seat by a notification; the window shows that notification
-- once (the original pops it up while the overview is open), until the seat closes it.
local coups = {}
local function coup_result(player)
    local c = coups[player]
    if not c then return nil end
    if c.result then return c.result end
    local p = Players[player]
    local success = L("TXT_KEY_NOTIFICATION_SPY_YOU_STAGE_COUP_SUCCESS_S", c.city)
    local failure = L("TXT_KEY_NOTIFICATION_SPY_YOU_STAGE_COUP_FAILURE_S", c.city)
    -- The newest first: the list may have lost old entries since the order went.
    for i = p:GetNumNotifications() - 1, 0, -1 do
        local summary = p:GetNotificationSummaryStr(i)
        if p:GetNotificationTurn(i) < c.turn then break end
        if summary == success or summary == failure then
            c.result = { success = summary == success, city = summary, text = p:GetNotificationStr(i) }
            return c.result
        end
    end
    return nil
end
local activities = {
    TXT_KEY_SPY_STATE_UNASSIGNED = "unassigned", TXT_KEY_SPY_STATE_TRAVELLING = "travelling",
    TXT_KEY_SPY_STATE_SURVEILLANCE = "surveillance", TXT_KEY_SPY_STATE_GATHERING_INTEL = "stealing",
    TXT_KEY_SPY_STATE_RIGGING_ELECTION = "rigging", TXT_KEY_SPY_STATE_COUNTER_INTEL = "counterintel",
    TXT_KEY_SPY_STATE_DEAD = "dead", TXT_KEY_SPY_STATE_MAKING_INTRODUCTIONS = "travelling", TXT_KEY_SPY_STATE_SCHMOOZING = "schmoozing",
}
return {
    data = function(player, args)
        local p, agents, own, other, intrigue = Players[player], array(), array(), array(), array()
        local enabled = not Game.IsOption(GameOptionTypes.GAMEOPTION_NO_ESPIONAGE)
        local counts, by_city = { alive = 0, killed = 0 }, {}
        for _, raw in ipairs(p:GetEspionageSpies()) do
            local dead = raw.State == "TXT_KEY_SPY_STATE_DEAD"
            local row = {
                id = raw.AgentID, name = L(raw.Name), rank = raw.Rank == "TXT_KEY_SPY_RANK_2" and "special_agent" or (raw.Rank == "TXT_KEY_SPY_RANK_1" and "agent" or "recruit"),
                rank_text = L(raw.Rank), role = raw.IsDiplomat and "diplomat" or "spy", activity = activities[raw.State] or "unassigned", activity_text = L(raw.State),
                surveillance = raw.EstablishedSurveillance, dead = dead, can_view = false, is_cs = false, can_coup = false,
                view_reason = "The city window shows only your own cities.", coup_reason = "A living spy established in an eligible city-state is required.",
            }
            -- SIMPLIFIED(SIMP-374): the original views the foreign city a spy watches in a read-only city screen; the city window here shows the seat's own cities only, so the view stays off (coup results are shown, see `coup_result`).
            if dead then counts.killed = counts.killed + 1; row.death_text = L(raw.State)
            else counts.alive = counts.alive + 1 end
            if not dead and raw.CityX >= 0 and raw.CityY >= 0 then
                local plot = Map.GetPlot(raw.CityX, raw.CityY)
                local city = plot and plot:GetPlotCity()
                if city then
                    local owner = city:GetOwner()
                    local place = civ(owner)
                    place.city_id, place.city = city_key(owner, city:GetID()), city:GetName()
                    row.place, row.is_cs = place, Players[owner]:IsMinorCiv()
                    by_city[place.city_id] = row
                    if row.is_cs then
                        row.can_coup = p:CanSpyStageCoup(raw.AgentID)
                        if row.can_coup then row.coup_reason = nil end
                        local ally = Players[owner]:GetAlly()
                        local ally_name = "No ally"
                        if ally >= 0 then ally_name = met(player, ally) and select(2, civ(ally)) or "Unknown civilization" end
                        row.coup = { chance_pct = p:GetCoupChanceOfSuccess(city), ally = ally_name, ally_player = ally >= 0 and met(player, ally) and ally or -1 }
                    end
                end
            end
            if not dead and raw.PercentComplete >= 0 and raw.PercentComplete <= 100 then
                row.progress_pct = raw.PercentComplete
                if raw.TurnsLeft > 0 and raw.TurnsLeft < 4000 then
                    row.turns_left = raw.TurnsLeft
                end
            end
            agents[#agents + 1] = row
        end
        local moving, legal = nil, {}
        if args.a and args.a >= 0 then
            spy(player, args.a)
            moving = { agent_id = args.a, targets = array(), ask_role = array() }
            for _, c in ipairs(p:GetAvailableSpyRelocationCities(args.a)) do legal[city_key(c.PlayerID, c.CityID)] = true end
        end
        local max_own = 0
        local city_rows = p:GetEspionageCityStatus()
        for _, raw in ipairs(city_rows) do if raw.PlayerID == player then max_own = math.max(max_own, raw.Potential) end end
        for _, raw in ipairs(city_rows) do
            local owner = raw.PlayerID
            local city = Players[owner]:GetCityByID(raw.CityID)
            if city and (owner == player or (Players[owner]:GetTeam() ~= p:GetTeam() and met(player, owner) and city:Plot():IsRevealed(p:GetTeam(), false))) then
                local row, name = civ(owner)
                row.id, row.raw_city_id, row.name, row.population = city_key(owner, raw.CityID), raw.CityID, L(raw.Name), raw.Population
                local assigned = by_city[row.id]
                if assigned then row.agent_id, row.agent_role = assigned.id, assigned.role end
                local war = team_of(player):IsAtWar(Players[owner]:GetTeam())
                row.can_move = legal[row.id] == true
                row.ask_role = row.can_move and owner ~= player and not Players[owner]:IsMinorCiv() and city:IsCapital() and not war
                if not row.can_move then row.move_reason = "Another agent is present or this city is out of reach." end
                if moving and row.can_move then
                    moving.targets[#moving.targets + 1] = { id = row.id }
                    if row.ask_role then moving.ask_role[#moving.ask_role + 1] = { id = row.id } end
                end
                if owner == player then
                    row.potential = raw.Potential
                    row.potential_rel = max_own > 0 and raw.Potential / max_own or 0
                    own[#own + 1] = row
                else
                    row.civ_name, row.is_city_state, row.capital, row.at_war, row.can_view = name, Players[owner]:IsMinorCiv(), city:IsCapital(), war, false
                    row.can_rig = row.is_city_state and not war
                    row.potential_state = row.is_city_state and "city_state" or (raw.Potential >= 0 and "known" or (raw.BasePotential >= 0 and "ghost" or "unknown"))
                    if raw.Potential >= 0 then row.potential = raw.Potential
                    elseif raw.BasePotential >= 0 then row.potential = raw.BasePotential end
                    if row.potential then row.potential_rel = raw.LargestBasePotential > 0 and row.potential / raw.LargestBasePotential or 0 end
                    other[#other + 1] = row
                end
            end
        end
        local messages = p:GetIntrigueMessages()
        for i, raw in ipairs(messages) do
            local source = { kind = "own_spy", name = L(raw.SpyName) }
            if raw.DiscoveringPlayer ~= player then
                if raw.DiscoveringPlayer >= 0 and met(player, raw.DiscoveringPlayer) then
                    source = civ(raw.DiscoveringPlayer)
                    source.kind, source.name = "civ", select(2, civ(raw.DiscoveringPlayer))
                else source = { kind = "civ", name = "Unknown civilization" } end
            end
            intrigue[#intrigue + 1] = { id = #messages - i, turn = raw.Turn, source = source, text = raw.String }
        end
        local who, name = civ(player)
        who.name, who.civ_name = L(GameInfo.Leaders[p:GetLeaderType()].Description), name
        return { player = who, enabled = enabled, started = #agents > 0, late_era = p:GetCurrentEra() > GameInfoTypes.ERA_RENAISSANCE,
            counts = counts, agents = agents, own_cities = own, other_cities = other, intrigue = intrigue, move = moving,
            coup_result = coup_result(player) }
    end,
    actions = {
        move = function(player, args, net)
            spy(player, args.a)
            assert(args.d == 0 or args.d == 1, "role must be spy or diplomat")
            assert(target(player, args.a, args.b, args.c), "city is not a legal relocation target")
            if args.d == 1 then
                local p = Players[args.b]
                local city = p and p:GetCityByID(args.c)
                assert(city and args.b ~= player and not p:IsMinorCiv() and city:IsCapital() and not team_of(player):IsAtWar(p:GetTeam()), "diplomats require a foreign capital at peace")
            end
            -- The core message carries no sender: the worker submits it as this seat.
            net.send("SendMoveSpy", args.a, args.b, args.c, args.d)
        end,
        hideout = function(player, args, net)
            local s = spy(player, args.a)
            assert(s.CityX >= 0 and s.CityY >= 0, "agent is already in the hideout")
            net.send("SendMoveSpy", args.a, -1, -1, 0)
        end,
        coup = function(player, args, net)
            local s = spy(player, args.a)
            assert(Players[player]:CanSpyStageCoup(args.a), "this agent cannot stage a coup now")
            local plot = Map.GetPlot(s.CityX, s.CityY)
            local city = plot and plot:GetPlotCity()
            assert(city, "the agent is in no city")
            coups[player] = { city = city:GetNameKey(), turn = Game.GetGameTurn() }
            net.send("SendStageCoup", args.a)
        end,
        coup_seen = function(player)
            coups[player] = nil
        end,
    },
}
