local kinds = {[0] = "kick", "concede", "scrap", "remap"}
local titles = {[0] = "TXT_KEY_MP_PROPOSAL_SCREEN_SUMMARY_IRR", "TXT_KEY_MP_PROPOSAL_SCREEN_SUMMARY_CC", "TXT_KEY_MP_PROPOSAL_SCREEN_SUMMARY_SCRAP", "TXT_KEY_MP_PROPOSAL_SCREEN_SUMMARY_REMAP"}
local function color(v)
    return math.floor(v.x * 255 + 0.5) * 16777216 + math.floor(v.y * 255 + 0.5) * 65536 + math.floor(v.z * 255 + 0.5) * 256 + 255
end
local function reference(player, id)
    local row = {player_id = id, nick = "", leader = L("TXT_KEY_UNMET_PLAYER"), civ_name = L("TXT_KEY_UNMET_PLAYER"), portrait = "", emblem = ""}
    if Players[id] and (id == player or met(player, id)) then
        local p = Players[id]
        local c, leader = GameInfo.Civilizations[p:GetCivilizationType()], GameInfo.Leaders[p:GetLeaderType()]
        local bg, fg = p:GetPlayerColors()
        row.leader, row.civ_name, row.portrait, row.emblem = L(leader.Description), L(c.ShortDescription), "leader:" .. leader.Type, "civ:" .. c.Type
        row.color_bg, row.color_fg = color(bg), color(fg)
    end
    return row
end
local function valid_proposal(player, id)
    assert(type(id) == "number" and id == math.floor(id) and id > 0 and id <= Game.GetLastProposalID(), "The proposal is unavailable.")
    assert(Game.GetProposalStatus(id) == 0 and not Game.GetProposalCompletion(id), "This proposal is already closed.")
    assert(Game.GetProposalVoterEligibility(id, player), "You are not eligible to vote on this proposal.")
    assert(not Game.GetProposalVoterHasVoted(id, player), "You have already voted on this proposal.")
end
local function candidate_rows(player, diplo)
    local rows = array()
    for team = 0, GameDefines.MAX_CIV_TEAMS - 1 do
        local t = Teams[team]
        local id = t:GetLeaderID()
        local p = id >= 0 and Players[id]
        if p and p:IsAlive() and not p:IsMinorCiv() and not p:IsObserver() and team ~= Players[player]:GetTeam()
            and (diplo and met(player, id) or not diplo and p:IsHuman()) then
            local row = reference(player, id)
            row.key, row.team_label, row.is_human = "player:" .. id, "Team " .. (team + 1), p:IsHuman()
            row.sub, row.score_text = row.civ_name, tostring(p:GetScore())
            rows[#rows + 1] = row
        end
    end
    return rows
end
local function diplo_blocked(player)
    return Players[player]:GetEndTurnBlockingType() == EndTurnBlockingTypes.ENDTURN_BLOCKING_DIPLO_VOTE
end
local function concede_available(player)
    return Game.IsProposalTypeAvailable(1) and not Game.IsPlayerHasActiveProposal(player) and not Game.IsProposalTypeOnCooldown(1, player)
end
local function data(player, args)
    local root = {view = args.a == 1 and "concede" or args.a == 2 and "diplo_vote" or "proposal",
        player_id = player, open_count = 0, human_count = 0, teams = array()}
    local last = Game.GetLastProposalID()
    for id = 1, last do if Game.GetProposalStatus(id) == 0 then root.open_count = root.open_count + 1 end end
    for id = 0, GameDefines.MAX_MAJOR_CIVS - 1 do
        if Players[id]:IsAlive() and Players[id]:IsHuman() and not Players[id]:IsObserver() then root.human_count = root.human_count + 1 end
    end
    if root.view == "concede" then
        local available = concede_available(player)
        root.concede = {candidates = available and candidate_rows(player, false) or array(),
            empty_text = available and "There are no eligible human teams." or "A concession proposal is unavailable or on cooldown."}
        return root
    elseif root.view == "diplo_vote" then
        root.diplo_vote = {candidates = candidate_rows(player, true), blocking = diplo_blocked(player)}
        if not root.diplo_vote.blocking then root.diplo_vote.candidates = array(); root.reason = "No diplomatic vote is awaiting your decision." end
        return root
    end
    local proposal
    if last > 0 and kinds[Game.GetProposalType(last)] and Game.GetProposalStatus(last) >= 0 then
        local kind, status = Game.GetProposalType(last), Game.GetProposalStatus(last)
        local subject, author = Game.GetProposalSubject(last), Game.GetProposalOwner(last)
        local voted, eligible = Game.GetProposalVoterHasVoted(last, player), Game.GetProposalVoterEligibility(last, player)
        local counts = kind ~= 3
        local voters, yes = Game.GetMaxVotes(last), Game.GetYesVotes(last)
        local waiting = array()
        for id = 0, GameDefines.MAX_MAJOR_CIVS - 1 do
            if Players[id]:IsAlive() and Game.GetProposalVoterEligibility(last, id) and not Game.GetProposalVoterHasVoted(last, id) then
                waiting[#waiting + 1] = {key = "player:" .. id, nick = reference(player, id).leader}
            end
        end
        local subject_ref = subject >= 0 and reference(player, subject) or nil
        proposal = {id = last, key = "proposal:" .. last, kind = kinds[kind], kind_label = kinds[kind],
            title = L(titles[kind], subject_ref and subject_ref.leader or ""), subject = subject_ref,
            author = author >= 0 and reference(player, author) or nil, author_known = author >= 0 and (author == player or met(player, author)),
            ends_turn = Game.GetElapsedGameTurns() + Game.GetProposalExpirationCounter(last) + 1,
            turns_left = Game.GetProposalExpirationCounter(last) + 1, voters = voters,
            yes = counts and yes or 0, needed = counts and voters or nil, still_needed = counts and math.max(0, voters - yes) or nil,
            show_counts = counts, yes_pct = counts and voters > 0 and yes / voters * 100 or nil, needed_pct = counts and 100 or nil,
            waiting_count = #waiting, waiting = waiting, waiting_text = #waiting == 0 and "No voters are pending." or tostring(#waiting) .. " voters are pending.",
            status = status == 0 and "open" or status == 1 and "passed" or "failed", status_text = status == 0 and "Open" or status == 1 and "Passed" or "Failed",
            can_vote = status == 0 and eligible and not voted and not Game.GetProposalCompletion(last),
            my_vote = voted and (Game.GetProposalVoterVote(last, player) and "yes" or "no") or "none",
            vote_reason = status ~= 0 and "Voting has closed." or not eligible and "You are not eligible to vote." or voted and "Your vote has been submitted." or "", secret = kind ~= 3}
    else root.reason = "No network proposal is available." end
    root.proposal = proposal
    local groups = {}
    for id = 0, GameDefines.MAX_MAJOR_CIVS - 1 do
        local p = Players[id]
        if p:IsAlive() or p:IsObserver() then
            local observer = p:IsObserver()
            local key = observer and "observers" or "team:" .. p:GetTeam()
            if not groups[key] then
                groups[key] = {key = key, label = observer and "Observers" or "Team " .. (p:GetTeam() + 1), players = array(), multi = false}
                root.teams[#root.teams + 1] = groups[key]
            end
            local row = reference(player, id)
            row.key, row.score, row.is_you, row.is_human, row.is_observer = "player:" .. id, not observer and p:GetScore() or nil, id == player, p:IsHuman(), observer
            row.player_key = "0:" .. id
            row.known, row.at_war = id == player or met(player, id), team_of(player):IsAtWar(p:GetTeam())
            row.eligible = proposal and Game.GetProposalVoterEligibility(proposal.id, id) or false
            local voted = proposal and Game.GetProposalVoterHasVoted(proposal.id, id)
            row.vote = "none"
            if voted then
                if id == player or not proposal.secret and proposal.status ~= "open" then row.vote = Game.GetProposalVoterVote(proposal.id, id) and "yes" or "no"
                else row.vote = "hidden" end
            end
            row.vote_label, row.note = row.vote == "hidden" and "—" or row.vote, observer and "Observer" or ""
            groups[key].players[#groups[key].players + 1] = row
        end
    end
    for _, group in ipairs(root.teams) do
        table.sort(group.players, function(a, b) if a.score ~= b.score then return (a.score or -1) > (b.score or -1) end return a.player_id < b.player_id end)
        group.multi = #group.players > 1 or group.key == "observers"
    end
    table.sort(root.teams, function(a, b) return (a.players[1].score or -1) > (b.players[1].score or -1) end)
    -- Nicknames and connections are the session's: the skin joins the client's `session` root by `player_key`, so the
    -- nick and connected fields here stay empty.
    return root
end

local function candidate(player, id, diplo)
    assert(type(id) == "number" and id == math.floor(id), "A candidate id is required.")
    for _, row in ipairs(candidate_rows(player, diplo)) do if row.player_id == id then return end end
    error("This candidate is unavailable.")
end
return {data = data, actions = {
    vote_yes = function(player, args, net)
        valid_proposal(player, args.a)
        net.send("sendGiftUnit", args.a, -5)
        if args.popup_id then net.popup_done(args.popup_id) end
        return {close=true}
    end,
    vote_no = function(player, args, net)
        valid_proposal(player, args.a)
        net.send("sendGiftUnit", args.a, -6)
        if args.popup_id then net.popup_done(args.popup_id) end
        return {close=true}
    end,
    concede = function(player, args, net)
        assert(concede_available(player), "A concession proposal is unavailable or on cooldown.")
        candidate(player, args.a, false)
        net.send("sendGiftUnit", args.a, -3)
        if args.popup_id then net.popup_done(args.popup_id) end
        return {close=true}
    end,
    diplo_vote = function(player, args, net)
        assert(diplo_blocked(player), "No diplomatic vote is awaiting your decision.")
        candidate(player, args.a, true)
        net.send("SendDiploVote", args.a)
        if args.popup_id then net.popup_done(args.popup_id) end
        return {close=true}
    end,
}}
