use lua_sys::Lua;

#[path = "fixtures/culture_model.rs"]
mod model;

#[test]
fn network_votes_hide_private_answers_and_validate_seat_eligibility() {
    let lua = Lua::new().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    lua.exec(include_str!("fixtures/culture.lua"), "=fixture")
        .unwrap();
    lua.exec(r#"
        GameDefines.MAX_CIV_TEAMS = 2
        EndTurnBlockingTypes = {ENDTURN_BLOCKING_DIPLO_VOTE = 9}
        Teams[0].GetLeaderID = function() return 0 end
        Teams[1] = {GetLeaderID = function() return 1 end, IsAtWar = function() return false end}
        for id = 0, 1 do
            Players[id].IsMinorCiv = function() return false end
            Players[id].IsObserver = function() return false end
            Players[id].IsHuman = function() return true end
            Players[id].GetScore = function() return 100 end
            Players[id].GetEndTurnBlockingType = function() return blocking and 9 or -1 end
        end
        Game.GetLastProposalID = function() return 1 end
        Game.GetProposalStatus = function() return status end
        Game.GetProposalType = function() return proposal_type end
        Game.GetProposalSubject = function() return 1 end
        Game.GetProposalOwner = function() return 0 end
        Game.GetProposalCompletion = function() return status ~= 0 end
        Game.GetProposalVoterHasVoted = function(id, player) return player == 1 or voted end
        Game.GetProposalVoterEligibility = function() return eligible end
        Game.GetProposalVoterVote = function() return true end
        Game.GetMaxVotes = function() return 2 end
        Game.GetYesVotes = function() return 1 end
        Game.GetElapsedGameTurns = function() return 20 end
        Game.GetProposalExpirationCounter = function() return 1 end
        Game.IsProposalTypeAvailable = function() return true end
        Game.IsPlayerHasActiveProposal = function() return false end
        Game.IsProposalTypeOnCooldown = function() return cooldown end
        status, proposal_type, voted, eligible, known, cooldown, blocking = 0, 0, false, true, false, false, false
    "#, "=vote-fixture").unwrap();
    lua.exec(
        &format!(
            "votes = (function()\n{}\nend)()",
            include_str!("../screens/mp_vote.lua")
        ),
        "=mp-vote",
    )
    .unwrap();
    lua.exec(
        r#"
        local model = votes.data(0, {})
        assert(model.proposal.can_vote and model.proposal.secret)
        assert(model.proposal.subject.leader == "TXT_KEY_UNMET_PLAYER")
        assert(model.teams[2].players[1].vote == "hidden")
        local sent = {}
        local net = {send = function(...) sent[#sent + 1] = {...} end}
        votes.actions.vote_yes(0, {a = 1}, net)
        votes.actions.vote_no(0, {a = 1}, net)
        assert(sent[1][1] == "sendGiftUnit" and sent[1][2] == 1 and sent[1][3] == -5)
        assert(sent[2][3] == -6)
        voted = true
        assert(not pcall(votes.actions.vote_yes, 0, {a = 1}, net))
        voted, eligible = false, false
        assert(not pcall(votes.actions.vote_no, 0, {a = 1}, net))
        eligible = true
        assert(not pcall(votes.actions.vote_yes, 0, {a = 2}, net))
        status = 1
        assert(not pcall(votes.actions.vote_yes, 0, {a = 1}, net))
        proposal_type = 3
        local result = votes.data(0, {})
        assert(not result.proposal.show_counts and result.proposal.needed == nil)
        assert(result.teams[2].players[1].vote == "yes")
        votes.actions.concede(0, {a = 1}, net)
        assert(sent[3][3] == -3)
        cooldown = true
        assert(not pcall(votes.actions.concede, 0, {a = 1}, net))
        cooldown = false
        assert(not pcall(votes.actions.concede, 0, {a = 0}, net))
        assert(not pcall(votes.actions.diplo_vote, 0, {a = 1}, net))
        known, blocking = true, true
        votes.actions.diplo_vote(0, {a = 1}, net)
        assert(sent[4][1] == "SendDiploVote" and sent[4][2] == 1 and #sent == 4)
    "#,
        "=vote-assertions",
    )
    .unwrap();
    model::check(&lua, "votes", "mp_vote");
}
