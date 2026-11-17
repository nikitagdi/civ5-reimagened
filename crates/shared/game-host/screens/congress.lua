local seats = {}
local function integer(n, label)
    assert(type(n) == "number" and n == math.floor(n), label .. " is required.")
    return n
end
local function color(v)
    return math.floor(v.x * 255 + 0.5) * 16777216 + math.floor(v.y * 255 + 0.5) * 65536 + math.floor(v.z * 255 + 0.5) * 256 + 255
end
local function member(player, id)
    local row = {key = "player:" .. id, player_id = id, is_you = id == player}
    if not Players[id] or (id ~= player and not met(player, id)) then
        row.name, row.civ_name, row.portrait, row.emblem = L("TXT_KEY_UNMET_PLAYER"), L("TXT_KEY_UNMET_PLAYER"), "", ""
        row.color_bg, row.color_fg = 0x26221dff, 0xebe2d2ff
        return row
    end
    local p = Players[id]
    local c, leader = GameInfo.Civilizations[p:GetCivilizationType()], GameInfo.Leaders[p:GetLeaderType()]
    local bg, fg = p:GetPlayerColors()
    row.name, row.civ_name, row.portrait, row.emblem = L(leader.Description), L(c.ShortDescription), "leader:" .. leader.Type, "civ:" .. c.Type
    row.color_bg, row.color_fg = color(bg), color(fg)
    return row
end
local function active()
    local league = Game.GetActiveLeague()
    assert(league and not league:IsNone(), "The World Congress has not been founded.")
    return league
end
local function proposals(league, player)
    local rows = array()
    for _, group in ipairs({{"enact", "GetEnactProposals", false}, {"repeal", "GetRepealProposals", false}, {"enact", "GetEnactProposalsOnHold", true}, {"repeal", "GetRepealProposalsOnHold", true}}) do
        for _, source in ipairs(league[group[2]](league)) do
            local info = GameInfo.Resolutions[source.Type]
            local decision = group[1] == "repeal" and GameInfo.ResolutionDecisions.RESOLUTION_DECISION_YES_OR_NO or GameInfo.ResolutionDecisions[info.VoterDecision]
            local elect = decision.Type ~= "RESOLUTION_DECISION_YES_OR_NO"
            local candidates = array()
            if elect then
                for _, id in ipairs(league:GetChoicesForDecision(decision.ID, player)) do
                    local candidate = member(player, id)
                    candidate.enabled, candidate.reason = true, ""
                    candidates[#candidates + 1] = candidate
                end
            end
            rows[#rows + 1] = {key = group[1] .. ":" .. source.ID, kind = group[1], resolution_id = source.ID,
                resolution_type = info.Type, name = league:GetResolutionName(source.Type, source.ID, source.ProposerDecision, true),
                by = member(player, source.ProposalPlayer), description = league:GetResolutionDetails(source.Type, player, source.ID, source.ProposerDecision),
                effect = league:GetResolutionDetails(source.Type, player, source.ID, source.ProposerDecision),
                postponed = group[3], vote_kind = elect and "elect" or "yes_no", candidates = candidates,
                my_votes = 0, state = group[3] and "postponed" or "abstain", state_label = group[3] and "Postponed" or "Abstain"}
        end
    end
    return rows
end
local function draft(player, league, rows)
    local tokens = {league:GetID(), league:IsInSession() and 1 or 0, league:GetRemainingVotesForMember(player), league:GetRemainingProposalsForMember(player)}
    for _, row in ipairs(rows) do tokens[#tokens + 1] = row.key .. (row.postponed and ":hold" or "") end
    local token = table.concat(tokens, "/")
    local state = seats[player]
    if not state or state.token ~= token then state = {token = token, votes = {}, slots = {}}; seats[player] = state end
    return state
end
local function choices(league, player, state)
    local result = {enact = array(), repeal = array(), unavailable = array(), unavailable_count = 0}
    local function add(kind, id, info, source)
        local enabled = kind == "repeal" and league:CanProposeRepeal(id, player) or kind == "enact" and league:CanProposeEnactAnyChoice(id, player)
        local used = false
        for _, slot in pairs(state.slots) do if slot.kind == kind and slot.id == id then used = true end end
        local params = array()
        local decision = GameInfo.ResolutionDecisions[info.ProposerDecision]
        if kind == "enact" then
            for _, value in ipairs(league:GetChoicesForDecision(decision.ID, player)) do
                local valid = league:CanProposeEnact(id, player, value)
                params[#params + 1] = {key = tostring(value), value = value, name = league:GetTextForChoice(decision.ID, value),
                    enabled = valid and not used, reason = used and "Already selected in another slot." or not valid and "This parameter is unavailable." or "", icon = "", emblem = ""}
            end
        end
        local details = league:GetResolutionDetails(info.ID, player, kind == "repeal" and id or -1, source and source.ProposerDecision or -1)
        local row = {key = kind .. ":" .. id, kind = kind, id = id, name = L(info.Description), effect = details, description = details,
            needs = #params > 1 and (string.find(decision.Type, "LUXURY") and "luxury" or string.find(decision.Type, "RELIGION") and "religion" or "civ") or "none", params = params, enabled = enabled and not used,
            reason = used and "Already selected in another slot." or not enabled and "Resolution requirements are not met." or ""}
        local group = enabled and result[kind] or result.unavailable
        group[#group + 1] = row
    end
    for info in GameInfo.Resolutions() do add("enact", info.ID, info) end
    for _, source in ipairs(league:GetActiveResolutions()) do add("repeal", source.ID, GameInfo.Resolutions[source.Type], source) end
    result.unavailable_count = #result.unavailable
    return result
end
local function data(player, args)
    local league = Game.GetActiveLeague()
    local root = {view = "league", player_id = player}
    if not league or league:IsNone() then
        local disabled = Game.IsOption(GameInfoTypes.GAMEOPTION_NO_LEAGUES)
        root.league = {exists = false, absent_reason = disabled and "disabled" or "not_founded", absent_text = disabled and "The World Congress is disabled." or "The World Congress has not been founded.", members = array(), proposals = array(), effects = array(), slots = array(), mode = "view", phase = "between"}
        return root
    end
    if args.a == 1 then
        local session = args.b and GameInfo.LeagueSpecialSessions[args.b]
        if not session then for item in GameInfo.LeagueSpecialSessions() do session = item; break end end
        assert(session, "Congress splash details are unavailable.")
        local first = args.c == 1
        root.view = "splash"
        root.splash = {kind = first and "founded" or "new_era", first_session = first, art = "", caption = league:GetName(),
            title = league:GetLeagueSplashTitle(session.ID, first), description = league:GetLeagueSplashDescription(session.ID, first),
            this_era_title = L(session.Description), this_era = array({{key = "rules", icon = "", text = league:GetLeagueSplashThisEraDetails(session.ID, first)}}),
            next_era_title = "", next_era = array({{key = "rules", icon = "", text = league:GetLeagueSplashNextEraDetails(session.ID, first)}})}
        return root
    elseif args.a == 2 then
        local info = args.b and GameInfo.LeagueProjects[args.b]
        assert(info and (league:IsProjectActive(info.ID) or league:IsProjectComplete(info.ID)), "Project results are unavailable.")
        root.view = "project"
        local project = {id = info.ID, key = info.Type, name = L(info.Description), art = "", completed = league:IsProjectComplete(info.ID),
            description = league:GetProjectDetails(info.ID, player), tiers = array(), ranking = array()}
        for tier = 1, 3 do project.tiers[#project.tiers + 1] = {key = tostring(tier), tier = tier, title = tostring(tier),
            threshold = league:GetContributionTierThreshold(tier, info.ID), reward = league:GetProjectRewardTierDetails(tier, info.ID, player)} end
        local total = 0
        for id = 0, GameDefines.MAX_MAJOR_CIVS - 1 do
            if Players[id]:IsAlive() and league:IsMember(id) then
                local row = member(player, id)
                row.known, row.sub = id == player or met(player, id), row.civ_name
                row.contribution, row.tier = league:GetMemberContribution(id, info.ID), league:GetMemberContributionTier(id, info.ID)
                row.awards, row.awards_tooltip = array(), row.tier > 0 and league:GetProjectRewardTierDetails(row.tier, info.ID, player) or ""
                local awards = {"bronze", "silver", "gold"}
                for tier = 1, math.min(3, row.tier) do row.awards[#row.awards + 1] = {key = tostring(tier), tier = awards[tier]} end
                total = total + row.contribution
                project.ranking[#project.ranking + 1] = row
            end
        end
        table.sort(project.ranking, function(a, b) if a.tier ~= b.tier then return a.tier > b.tier end return a.contribution > b.contribution end)
        for rank, row in ipairs(project.ranking) do row.rank = rank; row.contribution_pct = total > 0 and row.contribution / total * 100 or 0 end
        root.project = project
        return root
    end
    local rows = proposals(league, player)
    local state = draft(player, league, rows)
    local mode = league:CanVote(player) and "vote" or league:CanPropose(player) and "propose" or "view"
    local votes, spent = league:GetRemainingVotesForMember(player), 0
    for _, row in ipairs(rows) do
        local vote = state.votes[row.key]
        if vote and not row.postponed then
            row.my_votes, row.candidate = vote.count, vote.candidate and member(player, vote.candidate) or nil
            row.state = vote.count > 0 and "yea" or vote.count < 0 and "nay" or "abstain"
            row.state_label = row.state
            spent = spent + math.abs(vote.count)
        end
    end
    local free = math.max(0, votes - spent)
    for _, row in ipairs(rows) do
        row.can_up = mode == "vote" and not row.postponed and (free > 0 or row.my_votes < 0)
        row.can_down = mode == "vote" and not row.postponed and (free > 0 or row.my_votes > 0)
    end
    local model = {exists = true, id = league:GetID(), name = league:GetName(), is_un = league:IsUnitedNations(), art = "",
        can_rename = false, rename_reason = "Congress renaming requires a text net-message emitter.",
        diplo_victory_enabled = Game.IsVictoryValid and GameInfoTypes.VICTORY_DIPLOMATIC and Game.IsVictoryValid(GameInfoTypes.VICTORY_DIPLOMATIC) or false,
        delegates_to_win = Game.GetVotesNeededForDiploVictory and Game.GetVotesNeededForDiploVictory() or nil,
        phase = league:IsInSession() and "session" or "between", session_label = league:IsInSession() and "In session" or "Between sessions",
        special_session = league:IsInSpecialSession(), turns_to_session = league:GetTurnsUntilSession(),
        turns_to_world_leader_vote = league:GetTurnsUntilVictorySession(), mode = mode, members = array(), proposals = rows,
        delegates_total = league:GetCoreVotesForMember(player), delegates_free = mode == "vote" and free or nil,
        can_reset_votes = mode == "vote" and spent > 0, effects = array(), slots = array()}
    for id = 0, GameDefines.MAX_MAJOR_CIVS - 1 do
        if Players[id]:IsAlive() and league:IsMember(id) then
            local row = member(player, id)
            row.is_host, row.is_human = league:IsHostMember(id), Players[id]:IsHuman()
            row.delegates, row.delegates_left, row.delegates_spent = league:GetCoreVotesForMember(id), league:GetRemainingVotesForMember(id), league:GetSpentVotesForMember(id)
            row.can_propose, row.details = league:CanPropose(id), league:GetMemberDetails(id, player)
            model.members[#model.members + 1] = row
        end
    end
    table.sort(model.members, function(a, b) if a.is_host ~= b.is_host then return a.is_host end return a.player_id < b.player_id end)
    for _, source in ipairs(league:GetActiveResolutions()) do
        local info = GameInfo.Resolutions[source.Type]
        model.effects[#model.effects + 1] = {key = tostring(source.ID), icon = "", text = league:GetResolutionDetails(info.ID, player, source.ID, source.ProposerDecision)}
    end
    if mode == "propose" then
        local remaining = league:GetRemainingProposalsForMember(player)
        local filled = 0
        for index = 0, remaining - 1 do
            local slot = state.slots[index]
            local row = {key = tostring(index), index = index, filled = slot ~= nil, param = -1, name = "Choose a resolution", effect = ""}
            if slot then
                local info = GameInfo.Resolutions[slot.type]
                row.kind, row.resolution_id, row.resolution_type, row.param, row.name = slot.kind, slot.id, info.Type, slot.param, L(info.Description)
                row.effect = league:GetResolutionDetails(info.ID, player, slot.kind == "repeal" and slot.id or -1, slot.param)
                filled = filled + 1
            end
            model.slots[#model.slots + 1] = row
        end
        model.choices = choices(league, player, state)
        model.can_submit_proposals, model.submit_reason = filled == remaining and remaining > 0, filled == remaining and "" or "Fill every proposal slot."
    end
    if not model.choices then model.choices = choices(league, player, state) end
    -- SIMPLIFIED(SIMP-385): Congress art and parsed diplomat stances await host assets and structured queries. Native seat-scoped member and resolution detail strings remain available.
    root.league = model
    return root
end

local function context(player)
    local league = active()
    local model = data(player, {}).league
    return league, model, seats[player]
end
return {data = data, actions = {
    open = function(player)
        seats[player] = nil
        data(player, {})
    end,
    vote = function(player, args)
        local league, model, state = context(player)
        assert(model.mode == "vote", "Voting is unavailable.")
        assert(args.b == 0 or args.b == 1, "Choose enactment or repeal.")
        assert(args.c == 1 or args.c == -1 or args.c == 99 or args.c == -99, "Invalid vote adjustment.")
        local key = (args.b == 1 and "repeal:" or "enact:") .. integer(args.a, "Resolution id")
        local row
        for _, item in ipairs(model.proposals) do if item.key == key then row = item end end
        assert(row and not row.postponed, "The proposal is unavailable or postponed.")
        local count = args.c == -99 and 0 or args.c == 99 and (model.delegates_free + math.abs(row.my_votes)) or row.my_votes + args.c
        assert(math.abs(count) - math.abs(row.my_votes) <= model.delegates_free, "No delegates remain.")
        local candidate
        if row.vote_kind == "elect" and count ~= 0 then
            assert(count >= 0, "Election votes must be assigned to a candidate.")
            for _, item in ipairs(row.candidates) do if item.player_id == args.d and item.enabled then candidate = args.d end end
            assert(candidate, "Choose an eligible candidate.")
        end
        state.votes[key] = {count = count, candidate = candidate}
    end,
    vote_reset = function(player)
        local _, model, state = context(player)
        assert(model.mode == "vote", "Voting is unavailable.")
        state.votes = {}
    end,
    vote_submit = function(player, args, net)
        local league, model = context(player)
        assert(model.mode == "vote", "Voting is unavailable.")
        local assigned = 0
        for _, row in ipairs(model.proposals) do
            assigned = assigned + math.abs(row.my_votes)
            if row.my_votes ~= 0 then
                assert(not row.postponed, "A selected proposal is postponed.")
                if row.vote_kind == "elect" then
                    local valid = false
                    for _, candidate in ipairs(row.candidates) do
                        if row.candidate and candidate.player_id == row.candidate.player_id and candidate.enabled then valid = true end
                    end
                    assert(valid, "The selected candidate is unavailable.")
                end
            end
        end
        assert(assigned <= league:GetRemainingVotesForMember(player), "The available delegate count has changed.")
        for _, row in ipairs(model.proposals) do
            if row.my_votes ~= 0 then
                local choice = row.vote_kind == "elect" and row.candidate.player_id or row.my_votes > 0 and 1 or 0
                net.send(row.kind == "repeal" and "SendLeagueVoteRepeal" or "SendLeagueVoteEnact", league:GetID(), row.resolution_id, math.abs(row.my_votes), choice)
            end
        end
        if model.delegates_free > 0 then net.send("SendLeagueVoteAbstain", league:GetID(), model.delegates_free) end
    end,
    propose_set = function(player, args)
        local league, model, state = context(player)
        assert(model.mode == "propose", "Proposals are unavailable.")
        integer(args.a, "Proposal slot")
        assert(args.a >= 0 and args.a < #model.slots, "The proposal slot is unavailable.")
        assert(args.b == 0 or args.b == 1, "Choose enactment or repeal.")
        integer(args.c, "Resolution id"); integer(args.d, "Resolution parameter")
        local kind = args.b == 1 and "repeal" or "enact"
        for index, slot in pairs(state.slots) do assert(index == args.a or slot.kind ~= kind or slot.id ~= args.c, "This resolution is already in another slot.") end
        local info
        if kind == "repeal" then
            assert(league:CanProposeRepeal(args.c, player), "This resolution cannot be repealed.")
            for _, source in ipairs(league:GetActiveResolutions()) do if source.ID == args.c then info = GameInfo.Resolutions[source.Type] end end
        else
            info = GameInfo.Resolutions[args.c]
            assert(info and league:CanProposeEnact(args.c, player, args.d), "This resolution or parameter cannot be proposed.")
        end
        assert(info, "This resolution is unavailable.")
        state.slots[args.a] = {kind = kind, id = args.c, type = info.ID, param = args.d}
    end,
    propose_reset = function(player)
        local _, model, state = context(player)
        assert(model.mode == "propose", "Proposals are unavailable.")
        state.slots = {}
    end,
    propose_submit = function(player, args, net)
        local league, model, state = context(player)
        assert(model.mode == "propose" and model.can_submit_proposals, model.submit_reason or "Proposals are unavailable.")
        for index = 0, #model.slots - 1 do
            local slot = state.slots[index]
            assert(slot.kind == "repeal" and league:CanProposeRepeal(slot.id, player) or slot.kind == "enact" and league:CanProposeEnact(slot.id, player, slot.param), "The selected proposal is no longer available.")
        end
        for index = 0, #model.slots - 1 do
            local slot = state.slots[index]
            if slot.kind == "repeal" then net.send("SendLeagueProposeRepeal", league:GetID(), slot.id)
            else net.send("SendLeagueProposeEnact", league:GetID(), slot.type, slot.param) end
        end
    end,
    rename = function(player, args, net)
        local league = active()
        assert(league:CanChangeCustomName(player), "You cannot rename this Congress.")
        local name = type(args.text) == "string" and string.match(args.text, "^%s*(.-)%s*$") or ""
        assert(#name > 0 and #(string.gsub(name, "[\128-\191]", "")) <= 48, "The Congress name must contain 1 to 48 characters.")
        -- SIMPLIFIED(SIMP-386): the current screen emitter cannot attach text to net messages. Rename fails closed until net.send_text is provided.
        assert(net.send_text, "Congress renaming requires a text net-message emitter.")
        net.send_text("SendLeagueEditName", name, league:GetID())
    end,
}}
