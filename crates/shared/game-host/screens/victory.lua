local sent_turn = {}
local function color(key)
    local c = GameInfo.Colors[key]
    return math.floor(c.Red * 255) * 16777216 + math.floor(c.Green * 255) * 65536 + math.floor(c.Blue * 255) * 256 + 255
end
local function civ(player, known)
    if not known then return { player = player, icon = "", color_bg = 0x26221dff, color_fg = 0xb2a794ff }, "Unknown civilization" end
    local p = Players[player]
    local info = GameInfo.Civilizations[p:GetCivilizationType()]
    local colors = GameInfo.PlayerColors[info.DefaultPlayerColor]
    return { player = player, icon = "civ:" .. info.Type, color_bg = color(colors.PrimaryColor), color_fg = color(colors.SecondaryColor) }, L(info.ShortDescription)
end
local function proposals(player)
    local rows = array()
    for i, key in ipairs({"irr", "cc", "scrap"}) do
        local id, reasons = i - 1, array()
        local function reason(key, turn)
            for _, r in ipairs(reasons) do if r.key == key then return end end
            reasons[#reasons + 1] = { key = key, turn = turn }
        end
        if not Game.IsNetworkMultiPlayer() or not Game.IsProposalTypeAvailable(id) then reason("type_unavailable") end
        if sent_turn[player] == Game.GetGameTurn() then reason("wait_next_turn") end
        if Game.IsPlayerHasActiveProposal(player) then reason("own_active") end
        if Game.IsAnyActiveProposalType(id) then reason("type_active") end
        if Game.IsProposalTypeOnCooldown(id, player) then reason("cooldown", Game.GetProposalTypeCooldownResetTurn(id, player)) end
        -- SIMPLIFIED(SIMP-375): Concede voting needs its dedicated client window; this screen cannot open an unregistered window.
        if id == 1 then reason("type_unavailable") end
        rows[#rows + 1] = { key = key, enabled = #reasons == 0, reasons = reasons }
    end
    return { visible = Game.IsNetworkMultiPlayer(), items = rows }
end
local part_specs = {
    {"booster_1", "PROJECT_SS_BOOSTER", 1}, {"booster_2", "PROJECT_SS_BOOSTER", 2}, {"booster_3", "PROJECT_SS_BOOSTER", 3},
    {"cockpit", "PROJECT_SS_COCKPIT", 1}, {"stasis_chamber", "PROJECT_SS_STASIS_CHAMBER", 1}, {"engine", "PROJECT_SS_ENGINE", 1},
}
local function project_count(team, name)
    local info = GameInfo.Projects[name]
    return info and team:GetProjectCount(info.ID) or 0
end
local function parts(team)
    local rows, count = array(), 0
    for _, spec in ipairs(part_specs) do
        local info = GameInfo.Projects[spec[2]]
        local built = project_count(team, spec[2]) >= spec[3]
        if built then count = count + 1 end
        rows[#rows + 1] = { key = spec[1], icon = info and icon(info.IconAtlas, info.PortraitIndex) or "", built = built }
    end
    return rows, count
end
return {
    data = function(player)
        local p, team, turn = Players[player], team_of(player), Game.GetGameTurn()
        local ranking, space, capitals, cultural = array(), array(), array(), array()
        local cap_by_founder, cap_counts, team_seen = {}, {}, {}
        local apollo_teams, influence_count, rivals = 0, 0, 0
        -- SIMPLIFIED(SIMP-376): Without original-capital coordinates, concealed capitals and the global domination leader remain unknown.
        for id = 0, GameDefines.MAX_MAJOR_CIVS - 1 do
            local other = Players[id]
            if other and other:IsEverAlive() then
                for city in other:Cities() do
                    if city:Plot():IsRevealed(p:GetTeam(), false) and city:IsOriginalCapital() then
                        cap_by_founder[city:GetOriginalOwner()] = city
                    end
                end
            end
        end
        for id = 0, GameDefines.MAX_MAJOR_CIVS - 1 do
            local other = Players[id]
            if other and other:IsEverAlive() and not other:IsMinorCiv() then
                local known = id == player or met(player, id)
                local row, civ_name = civ(id, known)
                row.name = known and id ~= player and L(GameInfo.Leaders[other:GetLeaderType()].Description) or nil
                row.civ_name, row.leader_icon, row.score, row.alive, row.is_you = civ_name, known and "leader:" .. GameInfo.Leaders[other:GetLeaderType()].Type or "", other:GetScore(), other:IsAlive(), id == player
                ranking[#ranking + 1] = row
                local cap, capital = civ(id, known), cap_by_founder[id]
                cap.id, cap.alive, cap.capital_name = id, other:IsAlive(), "Unknown capital"
                if capital then
                    cap.capital_name = known and capital:GetName() or "Unknown capital"
                    local owner = capital:GetOwner()
                    if owner == player or met(player, owner) then
                        cap.owner, cap.owner_met, cap.owner_icon = owner, true, select(1, civ(owner, true)).icon
                        local t = Players[owner]:GetTeam()
                        cap_counts[t] = (cap_counts[t] or 0) + 1
                        if Teams[t]:GetNumMembers() > 1 then cap.team = t end
                    end
                end
                capitals[#capitals + 1] = cap
                do
                    local t, tid = team_of(id), other:GetTeam()
                    local completed = project_count(t, "PROJECT_APOLLO_PROGRAM") > 0
                    local _, built = parts(t)
                    local race = civ(id, known)
                    race.name, race.is_you, race.apollo, race.parts_built = row.name, id == player, completed, built
                    space[#space + 1] = race
                    if completed and not team_seen[tid] then apollo_teams = apollo_teams + 1; team_seen[tid] = true end
                end
                if id ~= player then
                    if other:IsAlive() then rivals = rivals + 1 end
                    local generated = other:GetJONSCultureEverGenerated()
                    local pct = generated > 0 and math.floor(100 * p:GetInfluenceOn(id) / generated) or 0
                    if pct >= 100 and other:IsAlive() then influence_count = influence_count + 1 end
                    if known then
                        local influence = civ(id, true)
                        influence.influence_pct, influence.met = pct, true
                        cultural[#cultural + 1] = influence
                    end
                end
            end
        end
        table.sort(ranking, function(a,b) if a.score ~= b.score then return a.score > b.score end return a.player < b.player end)
        local rank = 1
        for i, row in ipairs(ranking) do row.rank = i; if row.player == player then rank = i end end
        -- The headless bridge may omit the scenario loader; ordinary generated games have no scenario score.
        local scenario = PreGame.GetLoadWBScenario and PreGame.GetLoadWBScenario() or false
        local score_items = array()
        for _, spec in ipairs({{"cities", "GetScoreFromCities"}, {"population", "GetScoreFromPopulation"}, {"land", "GetScoreFromLand"}, {"wonders", "GetScoreFromWonders"},
            {"technologies", "GetScoreFromTechs"}, {"future_techs", "GetScoreFromFutureTech"}, {"policies", "GetScoreFromPolicies"}, {"great_works", "GetScoreFromGreatWorks"}, {"religion", "GetScoreFromReligion"}}) do
            local show = not ((spec[1] == "technologies" or spec[1] == "future_techs") and Game.IsOption(GameOptionTypes.GAMEOPTION_NO_SCIENCE))
                and not (spec[1] == "policies" and Game.IsOption(GameOptionTypes.GAMEOPTION_NO_POLICIES))
                and not (spec[1] == "religion" and Game.IsOption(GameOptionTypes.GAMEOPTION_NO_RELIGION))
            if show then score_items[#score_items + 1] = { key = spec[1], value = p[spec[2]](p) } end
        end
        if scenario then for i = 1, 4 do score_items[#score_items + 1] = { key = "scenario_" .. i, value = p["GetScoreFromScenario" .. i](p) } end end
        local prereqs = {}
        local function visit(key)
            if not key or prereqs[key] then return end
            prereqs[key] = true
            for link in GameInfo.Technology_PrereqTechs{TechType = key} do visit(link.PrereqTech) end
        end
        local apollo = GameInfo.Projects.PROJECT_APOLLO_PROGRAM
        if apollo then visit(apollo.TechPrereq) end
        local done, total = 0, 0
        for key in pairs(prereqs) do total = total + 1; if team:IsHasTech(GameInfo.Technologies[key].ID) then done = done + 1 end end
        local diplo = { enabled = Game.IsVictoryValid(GameInfoTypes.VICTORY_DIPLOMATIC), league_active = Game.GetNumActiveLeagues() > 0, un_active = false }
        if diplo.league_active then
            local league = Game.GetActiveLeague()
            diplo.un_active, diplo.turns_to_session = league:IsUnitedNations(), league:GetTurnsUntilVictorySession()
            diplo.delegates_needed, diplo.delegates_controlled = Game.GetVotesNeededForDiploVictory(), league:CalculateStartingVotesForMember(player)
        end
        local who, name = civ(player, true)
        who.name, who.civ_name = L(GameInfo.Leaders[p:GetLeaderType()].Description), name
        return {
            player = who, turn = turn, scenario = scenario, time = { enabled = Game.IsVictoryValid(GameInfoTypes.VICTORY_TIME), max_turn = Game.GetMaxTurns(), turns_left = math.max(0, Game.GetMaxTurns() - turn) },
            score = { total = p:GetScore(), rank = rank, items = score_items },
            domination = { enabled = Game.IsVictoryValid(GameInfoTypes.VICTORY_DOMINATION), own_count = cap_counts[p:GetTeam()] or 0, capitals = capitals },
            science = { enabled = Game.IsVictoryValid(GameInfoTypes.VICTORY_SPACE_RACE), apollo_built = project_count(team, "PROJECT_APOLLO_PROGRAM") > 0, apollo_teams = apollo_teams, prereq_done = done, prereq_total = total, parts = parts(team) },
            diplomatic = diplo, culture = { enabled = Game.IsVictoryValid(GameInfoTypes.VICTORY_CULTURAL), influential_count = influence_count, rivals_count = rivals, civs = cultural },
            proposals = proposals(player), ranking = ranking, space_race = space,
        }
    end,
    actions = {
        propose = function(player, args, net)
            assert(args.a == 0 or args.a == 2, "proposal type must be irrelevant player or scrap game")
            local row = proposals(player).items[args.a + 1]
            assert(row.enabled, "proposal unavailable: active vote, cooldown, or wait until the next turn")
            net.send("sendGiftUnit", -1, args.a == 0 and -2 or -4)
            sent_turn[player] = Game.GetGameTurn()
        end,
    },
}
