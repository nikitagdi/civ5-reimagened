-- The leader screen of a seat talking to an AI: the original's LeaderHeadRoot (greeting, discuss, trade, war) and the
-- human-invoked DiscussionDialog, on the seat's leader session (_lib.lua). Every change of the game is the seat's
-- ordered net message (sendFromUIDiploEvent); the AI's reply reaches the session as the core's leader message.
local not_turn = "Only during your turn."
local function target(player,id)
    assert(type(id)=="number" and id==math.floor(id) and id>=0 and id<GameDefines.MAX_MAJOR_CIVS and id~=player,"Choose another major leader.")
    local p=Players[id]
    assert(p and p:IsAlive() and not p:IsHuman() and met(player,id),"This AI leader is unavailable.")
    return p
end
local function mood(player,id)
    local p=Players[id]
    if team_of(player):IsAtWar(p:GetTeam()) then return "war" end
    if p:IsDenouncedPlayer(player) then return "denounced" end
    local approach=Players[player]:GetApproachTowardsUsGuess(id)
    for _,key in ipairs({"friendly","guarded","hostile","afraid"}) do
        if approach==GameInfoTypes["MAJOR_CIV_APPROACH_" .. string.upper(key)] then return key end
    end
    return "neutral"
end
local function event(name)
    local e=FromUIDiploEventTypes and FromUIDiploEventTypes["FROM_UI_DIPLO_EVENT_" .. name]
    assert(e,"This diplomacy event is unavailable.")
    return e
end
-- The seat's session with this AI; the window may be opened before Talk (a notification, a reconnect): it starts one.
local function session(player,id)
    return leader_session(player,id) or leader_begin(player,id,false)
end
local function current(player)
    local s=leader_session(player)
    assert(s,"Talk to a leader first.")
    return s,target(player,s.ai)
end
-- Leaders both sides know and the AI is at peace with: the targets of a war together (DiscussionDialog's list).
local function coop_targets(player,ai)
    local out=array()
    local aiteam=Teams[Players[ai]:GetTeam()]
    for id=0,GameDefines.MAX_MAJOR_CIVS-1 do
        local q=Players[id]
        if id~=player and id~=ai and q and q:IsAlive() and met(player,id) and aiteam:IsHasMet(q:GetTeam())
            and q:GetTeam()~=Players[player]:GetTeam() and q:GetTeam()~=Players[ai]:GetTeam() and not aiteam:IsAtWar(q:GetTeam()) then
            local civ,leader=GameInfo.Civilizations[q:GetCivilizationType()],GameInfo.Leaders[q:GetLeaderType()]
            out[#out+1]={player=id,name=L(leader.Description),civ=L(civ.ShortDescription),emblem="civ:" .. civ.Type,color_bg="",color_fg="",
                meta=team_of(player):IsAtWar(q:GetTeam()) and "At war with you" or "",score=q:GetScore()}
        end
    end
    return out
end
-- The topics of the human-invoked discussion, in DiscussionDialog's order; `index` is the position here.
local function topics(player,ai)
    local me,p=Players[player],Players[ai]
    local friend,denounced=me:IsDoF(ai),me:IsDenouncedPlayer(ai)
    local list={
        {key="friendship",label="Declare friendship",hint="Ask to work together openly.",visible=not friend and not denounced,event="HUMAN_DISCUSSION_WORK_WITH_US"},
        {key="denounce",label="Denounce",hint="Condemn this leader before the world.",tone="warn",visible=not denounced,confirm=true},
        {key="dont_settle",label="Do not settle near me",visible=true,event="HUMAN_DISCUSSION_DONT_SETTLE"},
        {key="stop_spying",label="Stop spying on me",visible=true,event="HUMAN_DISCUSSION_STOP_SPYING"},
        {key="stop_converting",label="Stop spreading your religion",visible=not p:IsAskedToStopConverting(player),event="HUMAN_DISCUSSION_STOP_SPREADING_RELIGION"},
        {key="stop_digging",label="Stop digging up my artifacts",visible=not p:IsAskedToStopDigging(player),event="HUMAN_DISCUSSION_STOP_DIGGING"},
        {key="coop_war",label="Declare war together",hint="Choose a common enemy.",tone="war",visible=true,coop=true},
        {key="never_mind",label="Never mind",visible=true,back=true},
    }
    local out={}
    for index,t in ipairs(list) do
        t.index=index
        if t.coop and #coop_targets(player,ai)==0 then t.visible=false end
        if t.visible then out[#out+1]=t end
    end
    return out,list
end
return {data=function(player,args)
    local ok,p=pcall(target,player,args.a)
    if not ok then return {mode="main",mood={key="neutral",label="",reasons=array()},speaker_line="",phrase="",can_close=not args.popup,
        relation=array(),main_actions=array(),responses=array(),targets=array(),at_war=false,available=false,unavailable_reason="Choose a contacted AI leader first."} end
    local id=args.a
    local s=session(player,id)
    local civ,leader=GameInfo.Civilizations[p:GetCivilizationType()],GameInfo.Leaders[p:GetLeaderType()]
    local team,qt=team_of(player),p:GetTeam()
    local war=team:IsAtWar(qt)
    local turn=Players[player]:IsTurnActive()
    local cap=p:GetCapitalCity()
    local ideology=GameInfo.PolicyBranchTypes[p:GetLateGamePolicyTree()]
    local reasons=array()
    -- Native opinion rows contain formatted text, not their internal numeric weights.
    for index,text in ipairs(p:GetOpinionTable(player)) do reasons[#reasons+1]={key=tostring(index),text=string.gsub(text,"%[[^%]]+%]","")} end
    local relation=array({{key="status",label="Status",value=war and "At war" or "At peace"},
        {key="borders",label="Open borders",value=Teams[qt]:IsAllowsOpenBordersToTeam(Players[player]:GetTeam()) and "Their borders are open" or "Closed"},
        {key="friendship",label="Friendship",value=Players[player]:IsDoF(id) and "Declaration of friendship" or "None"}})
    local can_war=not args.popup and turn and team:CanDeclareWar(qt)
    local actions=array()
    local rows={
        {"discuss","Discuss",turn and not war,not turn and not_turn or (war and "Not while you are at war." or nil)},
        {"trade","Trade",turn and not war,not turn and not_turn or (war and "Not while you are at war." or nil)},
        {"demand","Demand",turn and not war,not turn and not_turn or (war and "Not while you are at war." or nil)},
        {war and "make_peace" or "declare_war",war and "Make peace" or "Declare war",war and turn and not args.popup or (not war and can_war),
            war and not_turn or (not can_war and "War cannot be declared now." or nil)},
        {"goodbye","Goodbye",not args.popup,nil},
    }
    for _,r in ipairs(rows) do
        actions[#actions+1]={key=r[1],label=r[2],visible=true,enabled=r[3] and true or false,reason=not r[3] and r[4] or nil,tooltip=not r[3] and r[4] or nil}
    end
    local responses,targets,confirm=array(),array(),nil
    if s.mode=="discussion" then
        for _,t in ipairs((topics(player,id))) do
            responses[#responses+1]={key=t.key,index=t.index,label=t.label,hint=t.hint,tone=t.tone or "normal",
                enabled=turn or t.back==true,reason=not turn and not t.back and not_turn or nil}
        end
    elseif s.mode=="coop_target" then
        targets=coop_targets(player,id)
    elseif s.mode=="confirm_denounce" then
        confirm={kind="denounce",text="Denounce " .. L(leader.Description) .. "?",
            consequences=array({{key="opinion",text="Every leader who knows you both will hear of it."}})}
    end
    return {partner={id=id,name=L(leader.Description),leader_name=L(leader.Description),title="",civ=L(civ.ShortDescription),
        capital=cap and cap:Plot():IsRevealed(Players[player]:GetTeam(),false) and cap:GetName() or nil,cities=p:GetNumCities(),score=p:GetScore(),
        ideology=ideology and L(ideology.Description) or nil,is_human=false,alive=true,team=qt,icon="leader:" .. leader.Type,
        emblem="civ:" .. civ.Type,color_bg="",color_fg=""},mood={key=mood(player,id),label=mood(player,id),reasons=reasons},
        mode=s.mode,speaker_line=L(leader.Description),phrase=s.phrase or "",can_close=not args.popup,relation=relation,at_war=war,main_actions=actions,
        responses=responses,targets=targets,confirm=confirm,available=true}
end,actions={
    discuss=function(player)
        local s,p=current(player)
        assert(Players[player]:IsTurnActive(),not_turn)
        assert(not team_of(player):IsAtWar(p:GetTeam()),"Not while you are at war.")
        -- FROM_UI_DIPLO_EVENT_HUMAN_WANTS_DISCUSSION changes nothing but the leader's line: it is read here, not sent.
        s.mode="discussion"
        s.phrase=plain(GcUiDiploText(player,s.ai,"LETS_HEAR_IT"))
    end,
    trade=function(player)
        local s,p=current(player)
        assert(Players[player]:IsTurnActive(),"Only during your turn.")
        assert(not team_of(player):IsAtWar(p:GetTeam()),"Not while you are at war.")
        s.trade_phrase=plain(GcUiDiploText(player,s.ai,"LETS_HEAR_IT"))
        s.deal_mode=nil
        return {close=true,open={screen="deal",a=s.ai}}
    end,
    -- UI.OnHumanDemand: the trade screen in demand mode, their side only; the deal screen asks the AI and sends its answer.
    demand=function(player)
        local s,p=current(player)
        assert(Players[player]:IsTurnActive(),not_turn)
        assert(not team_of(player):IsAtWar(p:GetTeam()),"Not while you are at war.")
        s.trade_phrase=plain(GcUiDiploText(player,s.ai,"LETS_HEAR_IT"))
        s.deal_mode="demand"
        return {close=true,open={screen="deal",a=s.ai}}
    end,
    -- FROM_UI_DIPLO_EVENT_HUMAN_NEGOTIATE_PEACE: it changes nothing but the asking machine's screen, so it is read here for
    -- the seat, not sent. An AI that wants peace opens the trade screen with the peace treaties on it (DoTradeScreenOpened,
    -- its price for this seat kept by the core's deal queries); one that does not says why.
    make_peace=function(player)
        local s,p=current(player)
        assert(Players[player]:IsTurnActive(),not_turn)
        assert(team_of(player):IsAtWar(p:GetTeam()),"You are not at war.")
        assert(GcUiDealQuery,"Peace talks need the game core.")
        local q=GcUiDealQuery(player,s.ai,{},"peace")
        if not (q.willing and #q.items>0) then
            s.mode,s.phrase="main",plain(GcUiDiploText(player,s.ai,q.too_soon and "TOO_SOON_NO_PEACE" or "NO_PEACE"))
            return
        end
        s.trade_phrase=plain(GcUiDiploText(player,s.ai,"PEACE_WHAT_WILL_HUMAN_OFFER"))
        s.deal_mode,s.peace_items="peace",q.items
        return {close=true,open={screen="deal",a=s.ai}}
    end,
    topic=function(player,args,net)
        local s,p=current(player)
        assert(s.mode=="discussion","Open the discussion first.")
        local _,list=topics(player,s.ai)
        local t=list[args.a]
        assert(t and t.visible,"This topic is unavailable.")
        if t.back then s.mode="main" return end
        assert(Players[player]:IsTurnActive(),"Only during your turn.")
        if t.coop then s.mode="coop_target" return end
        if t.confirm then s.mode="confirm_denounce" return end
        net.send("sendFromUIDiploEvent",s.ai,event(t.event),0,0)
        s.phrase=""
    end,
    coop_war=function(player,args,net)
        local s=current(player)
        assert(s.mode=="coop_target","Choose the topic first.")
        assert(Players[player]:IsTurnActive(),"Only during your turn.")
        local ok=false
        for _,t in ipairs(coop_targets(player,s.ai)) do if t.player==args.a then ok=true end end
        assert(ok,"Choose a leader you both know.")
        net.send("sendFromUIDiploEvent",s.ai,event("COOP_WAR_OFFER"),args.a,0)
        s.mode,s.phrase="discussion",""
    end,
    confirm=function(player,args,net)
        local s=current(player)
        assert(s.mode=="confirm_denounce","Nothing to confirm.")
        if args.a~=1 then s.mode="discussion" return end
        assert(Players[player]:IsTurnActive(),"Only during your turn.")
        assert(not Players[player]:IsDenouncedPlayer(s.ai),"You have denounced this leader already.")
        net.send("sendFromUIDiploEvent",s.ai,event("DENOUNCE"),0,0)
        s.mode,s.phrase="main",""
    end,
    back=function(player)
        local s=current(player)
        s.mode=(s.mode=="coop_target" or s.mode=="confirm_denounce") and "discussion" or "main"
    end,

    -- SIMPLIFIED(SIMP-003): the AI's own requests are still declined by the core (rt_diplo); there is nothing for the seat to respond to.
    respond=function() error("There is no request to answer.") end,
    switch=function() error("Say goodbye before talking to another leader.") end,
    goodbye=function(player,args)
        assert(not args.popup,"The pending AI decision cannot be dismissed without its captured conversation state.")
        leader_end(player)
        return {close=true}
    end,
    declare_war=function(player,args)
        assert(not args.popup,"Resolve the pending AI decision first.")
        local _,p=current(player)
        assert(Players[player]:IsTurnActive() and team_of(player):CanDeclareWar(p:GetTeam()),"War cannot be declared now.")
        return {open={screen="declare_war",a=p:GetTeam(),b=-1}}
    end,
}}
