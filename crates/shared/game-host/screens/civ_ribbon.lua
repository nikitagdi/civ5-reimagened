local function person(id)
    local p=Players[id]
    local c,l=GameInfo.Civilizations[p:GetCivilizationType()],GameInfo.Leaders[p:GetLeaderType()]
    return {id=id,name=L(l.Description),leader=L(l.Description),civ=L(c.ShortDescription),icon_leader="leader:" .. l.Type,
        icon_civ="civ:" .. c.Type,color_bg="",color_fg=""}
end
local function mood(player,other)
    if Players[other]:IsHuman() then return "human" end
    local guess=Players[player]:GetApproachTowardsUsGuess(other)
    for _,key in ipairs({"friendly","guarded","hostile","afraid"}) do
        if guess==GameInfoTypes["MAJOR_CIV_APPROACH_" .. string.upper(key)] then return key end
    end
    return "neutral"
end
local function other(player,id)
    assert(type(id)=="number" and id==math.floor(id) and id>=0 and id<GameDefines.MAX_MAJOR_CIVS and id~=player,"A rival major is required.")
    local p=Players[id]
    assert(p and p:IsAlive() and met(player,id),"This civilization is unavailable.")
    return p
end
return {data=function(player)
    local p,team=Players[player],team_of(player)
    local majors,minors=array(),array()
    for id=0,GameDefines.MAX_CIV_PLAYERS-1 do
        local q=Players[id]
        if q and q:IsAlive() and (id==player or met(player,id)) then
            local qt=q:GetTeam()
            local war=team:IsAtWar(qt)
            if q:IsMinorCiv() then
                local n,ally=q:GetMinorCivFriendshipWithMajor(player),q:GetAlly()
                local minor=GameInfo.MinorCivilizations[q:GetMinorCivType()]
                minors[#minors+1]={id=id,name=L(q:GetCivilizationShortDescriptionKey()),icon="civ:" .. minor.Type,color="",influence=n,
                    status=war and "war" or (q:IsAllies(player) and "allied" or (q:IsFriends(player) and "friends" or (n<0 and "angry" or "neutral"))),
                    ally_is_me=ally==player,ally_name=ally>=0 and (ally==player or met(player,ally)) and person(ally).name or nil,
                    has_my_spy=false,quests=q:GetMinorCivNumActiveQuestsForPlayer(player),pledged=p:IsProtectingMinor(id),at_war=war,turn_live=q:IsTurnActive()}
            else
                local row=person(id)
                row.player_key="0:" .. id
                row.is_me,row.is_human,row.met=id==player,q:IsHuman(),true
                row.team,row.team_number=qt,Teams[qt]:GetNumMembers()>1 and qt+1 or nil
                row.score,row.at_war,row.mood=q:GetScore(),war,id==player and "neutral" or mood(player,id)
                row.relation=id==player and "own" or (war and "war" or (q:IsDenouncedPlayer(player) and "denounced" or row.mood))
                row.turn_state=q:IsTurnActive() and "moving" or (q:HasReceivedNetTurnComplete() and "done" or "waiting")
                row.turn_live=q:IsTurnActive()
                row.connection,row.is_host,row.deal,row.can_open,row.can_kick="none",false,"none",false,false
                row.can_declare_war=id~=player and p:IsTurnActive() and team:CanDeclareWar(qt)
                row.action_reason=id==player and "This is your civilization." or (q:IsHuman() and "Trade proposals need a validated server deal serializer." or "AI conversations need a seat-scoped server session.")
                row.tip_portrait=row.action_reason
                if id~=player then
                    row.treaties={friendship=p:IsDoF(id),research=team:IsHasResearchAgreement(qt),defensive_pact=team:IsDefensivePact(qt),
                        borders_out=Teams[qt]:IsAllowsOpenBordersToTeam(p:GetTeam()),borders_in=team:IsAllowsOpenBordersToTeam(qt),closed=false}
                    if row.treaties.friendship then row.treaty_turns={friendship=math.max(0,(GameDefines.DOF_EXPIRATION_TIME or 50)-p:GetDoFCounter(id))} end
                    if war then row.can_make_peace=false;row.peace_block_reason="Peace negotiation needs a server trade session."
                    elseif not q:IsDenouncedPlayer(player) then row.gold=q:GetGold() end
                end
                majors[#majors+1]=row
            end
        end
    end
    table.sort(majors,function(a,b) if a.is_me~=b.is_me then return a.is_me end if a.score~=b.score then return a.score>b.score end return a.id<b.id end)
    table.sort(minors,function(a,b) if a.influence~=b.influence then return a.influence>b.influence end return a.id<b.id end)
    local spies=p:GetEspionageSpies()
    local unassigned=0
    for _,s in ipairs(spies) do if s.CityX<0 or s.CityY<0 then unassigned=unassigned+1 end end
    local league=Game.GetActiveLeague()
    -- SIMPLIFIED(SIMP-454): connection/host/ping/hot-join statistics are unavailable to scripts; the existing seat list supplies live connection state in the skin.
    return {visible=true,side="right",mp=Game.IsNetworkMultiPlayer(),i_am_host=false,hot_join=false,
        corner={league={active=league~=nil,in_session=league and league:IsInSession() or false,
            turns_to_session=league and league:GetTurnsUntilSession() or 0,victory_session=false},has_spies=#spies>0,spies_unassigned=unassigned,
            culture_overview=not (GameInfoTypes.GAMEOPTION_NO_CULTURE_OVERVIEW_UI and Game.IsOption(GameInfoTypes.GAMEOPTION_NO_CULTURE_OVERVIEW_UI)),extra=array()},majors=majors,city_states=minors,
        session_reason="Host, ping, hot-join and turn-duration statistics are unavailable to screen scripts."}
end,actions={
    begin_diplo=function(player,args) other(player,args.a);error("AI conversations need a seat-scoped server session.") end,
    propose_peace=function(player,args) other(player,args.a);error("Peace negotiation needs a server trade session.") end,
}}
