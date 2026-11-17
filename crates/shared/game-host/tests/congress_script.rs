use lua_sys::Lua;

#[path = "fixtures/culture_model.rs"]
mod model;

#[test]
fn congress_drafts_are_per_seat_budgeted_and_only_submit_emits_messages() {
    let lua = Lua::new().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    lua.exec(include_str!("fixtures/culture.lua"), "=fixture")
        .unwrap();
    lua.exec(r#"
        Teams[1] = Teams[0]
        local resolution = {ID = 0, Type = "RESOLUTION_TEST", Description = "Resolution", VoterDecision = "RESOLUTION_DECISION_YES_OR_NO", ProposerDecision = "RESOLUTION_DECISION_NONE"}
        local other = {ID = 1, Type = "RESOLUTION_OTHER", Description = "Other", VoterDecision = "RESOLUTION_DECISION_YES_OR_NO", ProposerDecision = "RESOLUTION_DECISION_NONE"}
        GameInfo.Resolutions = setmetatable({[0] = resolution, [1] = other}, {__call = function(self)
            local index = -1
            return function() index = index + 1; return self[index] end
        end})
        GameInfo.ResolutionDecisions = {RESOLUTION_DECISION_YES_OR_NO = {ID = 1, Type = "RESOLUTION_DECISION_YES_OR_NO"}, RESOLUTION_DECISION_NONE = {ID = 0, Type = "RESOLUTION_DECISION_NONE"}}
        league = {
            IsNone = function() return false end, GetID = function() return 5 end,
            GetName = function() return "Congress" end, IsUnitedNations = function() return false end,
            CanChangeCustomName = function() return true end, IsInSession = function() return mode == "vote" end,
            IsInSpecialSession = function() return false end, GetTurnsUntilSession = function() return 5 end,
            GetTurnsUntilVictorySession = function() return 20 end, GetCoreVotesForMember = function() return 3 end,
            GetRemainingVotesForMember = function() return mode == "vote" and 3 or 0 end,
            GetRemainingProposalsForMember = function() return mode == "propose" and 2 or 0 end,
            CanVote = function() return mode == "vote" end, CanPropose = function() return mode == "propose" end,
            IsMember = function() return true end, IsHostMember = function(self, id) return id == 0 end,
            GetSpentVotesForMember = function() return 0 end, GetMemberDetails = function() return "Member details" end,
            GetEnactProposals = function() return {{ID = 10, Type = 0, ProposerDecision = -1, ProposalPlayer = 0}} end,
            GetEnactProposalsOnHold = function() return {{ID = 11, Type = 0, ProposerDecision = -1, ProposalPlayer = 0}} end,
            GetRepealProposals = function() return {} end, GetRepealProposalsOnHold = function() return {} end,
            GetResolutionName = function() return "Resolution" end, GetResolutionDetails = function() return "Details" end,
            GetChoicesForDecision = function() return {-1} end, GetTextForChoice = function() return "None" end,
            GetActiveResolutions = function() return {} end,
            CanProposeEnactAnyChoice = function() return true end,
            CanProposeEnact = function(self, id, player, value) return (id == 0 or id == 1) and value == -1 end,
        }
        Game.GetActiveLeague = function() if exists then return league end end
        exists, mode, known = false, "vote", true
    "#, "=league-fixture").unwrap();
    lua.exec(
        &format!(
            "congress = (function()\n{}\nend)()",
            include_str!("../screens/congress.lua")
        ),
        "=congress",
    )
    .unwrap();
    lua.exec(
        r#"
        assert(not congress.data(0, {}).league.exists)
        exists = true
        local sent = {}
        local net = {send = function(...) sent[#sent + 1] = {...} end,
            send_text = function(...) sent[#sent + 1] = {...} end}
        congress.actions.open(0)
        congress.actions.vote(0, {a = 10, b = 0, c = 1, d = -1})
        assert(congress.data(0, {}).league.delegates_free == 2)
        assert(congress.data(0, {}).league.proposals[1].my_votes == 1)
        assert(congress.data(1, {}).league.delegates_free == 3)
        assert(not pcall(congress.actions.vote, 0, {a = 11, b = 0, c = 1, d = -1}))
        congress.actions.vote(0, {a = 10, b = 0, c = 99, d = -1})
        assert(congress.data(0, {}).league.delegates_free == 0)
        assert(not pcall(congress.actions.vote, 0, {a = 10, b = 0, c = 1, d = -1}))
        assert(#sent == 0)
        congress.actions.vote_submit(0, {}, net)
        assert(#sent == 1 and sent[1][1] == "SendLeagueVoteEnact")
        assert(sent[1][2] == 5 and sent[1][3] == 10 and sent[1][4] == 3 and sent[1][5] == 1)
        congress.actions.vote_reset(0)
        congress.actions.vote(0, {a = 10, b = 0, c = -1, d = -1})
        congress.actions.vote_submit(0, {}, net)
        assert(sent[2][5] == 0 and sent[3][1] == "SendLeagueVoteAbstain" and sent[3][3] == 2)
        congress.actions.open(0)
        assert(congress.data(0, {}).league.delegates_free == 3)
        mode = "propose"
        congress.actions.propose_set(0, {a = 0, b = 0, c = 0, d = -1})
        assert(congress.data(0, {}).league.slots[1].filled)
        assert(not pcall(congress.actions.propose_set, 0, {a = 1, b = 0, c = 0, d = -1}))
        assert(not pcall(congress.actions.propose_submit, 0, {}, net))
        congress.actions.propose_set(0, {a = 1, b = 0, c = 1, d = -1})
        congress.actions.propose_submit(0, {}, net)
        assert(sent[4][1] == "SendLeagueProposeEnact" and sent[4][3] == 0)
        assert(sent[5][1] == "SendLeagueProposeEnact" and sent[5][3] == 1)
        congress.actions.propose_reset(0)
        assert(not congress.data(0, {}).league.slots[1].filled)
        assert(congress.data(0, {}).league.can_rename)
        congress.actions.rename(0, {text = "  New Congress  "}, net)
        assert(#sent == 6 and sent[6][1] == "SendLeagueEditName" and sent[6][2] == "New Congress" and sent[6][3] == 5)
        assert(not pcall(congress.actions.rename, 0, {text = "   "}, net))
        assert(not pcall(congress.actions.rename, 0, {text = string.rep("a",49)}, net))
        assert(#sent == 6)
    "#,
        "=league-assertions",
    )
    .unwrap();
    model::check(&lua, "congress", "congress");
}
