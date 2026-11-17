//! Proposal checks and per-seat throttling require no game core.
use lua_sys::Lua;

#[test]
fn victory_proposals_validate_type_native_locks_and_seat_cooldowns() {
    let lua = Lua::new().unwrap();
    lua.sandbox().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    lua.exec(r#"
        turn=3;blocked=false
        Game={GetGameTurn=function() return turn end,IsNetworkMultiPlayer=function() return true end,
            IsProposalTypeAvailable=function() return true end,IsPlayerHasActiveProposal=function() return blocked end,
            IsAnyActiveProposalType=function() return false end,IsProposalTypeOnCooldown=function() return false end}
    "#, "=fixture").unwrap();
    let screen: lua_sys::Table = lua
        .load(include_str!("../screens/victory.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua.exec(
        r#"
        local sent={};local net={send=function(...) sent[#sent+1]={...} end}
        screen.actions.propose(0,{a=0},net)
        assert(sent[1][1]=="sendGiftUnit" and sent[1][2]==-1 and sent[1][3]==-2)
        local ok,reason=pcall(screen.actions.propose,0,{a=2},net)
        assert(not ok and #reason>0 and #sent==1)
        screen.actions.propose(1,{a=2},net)
        assert(sent[2][3]==-4)
        turn=4;screen.actions.propose(0,{a=2},net)
        assert(#sent==3)
        turn=5;local next=screen.actions.propose(0,{a=1},net)
        assert(next.open.screen=="mp_vote" and next.open.a==1 and #sent==3)
        blocked=true
        ok,reason=pcall(screen.actions.propose,0,{a=0},net)
        assert(not ok and #reason>0 and #sent==3)
        ok,reason=pcall(screen.actions.propose,0,{a=1},net)
        assert(not ok and #reason>0 and #sent==3)
    "#,
        "=assertions",
    )
    .unwrap();
}
