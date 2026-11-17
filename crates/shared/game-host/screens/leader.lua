local current={}
local reason="AI conversation is unavailable: leader messages are automatically answered in the core, with no seat-scoped conversation event delivered to this screen."
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
local function unavailable() error(reason) end
local function goodbye(_,args)
    assert(not args.popup,"The pending AI decision cannot be dismissed without its captured conversation state.")
    return {close=true}
end
return {data=function(player,args)
    local ok,p=pcall(target,player,args.a)
    if not ok then return {mode="main",mood={key="neutral",label="",reasons=array()},speaker_line="",phrase="",can_close=not args.popup,
        relation=array(),main_actions=array(),responses=array(),targets=array(),at_war=false,available=false,unavailable_reason="Choose a contacted AI leader first."} end
    local id=args.a
    current[player]=id
    local civ,leader=GameInfo.Civilizations[p:GetCivilizationType()],GameInfo.Leaders[p:GetLeaderType()]
    local team,qt=team_of(player),p:GetTeam()
    local war=team:IsAtWar(qt)
    local cap=p:GetCapitalCity()
    local ideology=GameInfo.PolicyBranchTypes[p:GetLateGamePolicyTree()]
    local reasons=array()
    -- Native opinion rows contain formatted text, not their internal numeric weights.
    for index,text in ipairs(p:GetOpinionTable(player)) do reasons[#reasons+1]={key=tostring(index),text=string.gsub(text,"%[[^%]]+%]","")} end
    local relation=array({{key="status",label="Status",value=war and "At war" or "At peace"},
        {key="borders",label="Open borders",value=Teams[qt]:IsAllowsOpenBordersToTeam(Players[player]:GetTeam()) and "Their borders are open" or "Closed"},
        {key="friendship",label="Friendship",value=Players[player]:IsDoF(id) and "Declaration of friendship" or "None"}})
    local can_war=not args.popup and Players[player]:IsTurnActive() and team:CanDeclareWar(qt)
    local actions=array()
    for _,spec in ipairs({{"discuss","Discuss"},{"trade","Trade"},{"demand","Demand"},{war and "make_peace" or "declare_war",war and "Make peace" or "Declare war"},{"goodbye","Goodbye"}}) do
        local enabled=(spec[1]=="declare_war" and can_war) or (spec[1]=="goodbye" and not args.popup)
        actions[#actions+1]={key=spec[1],label=spec[2],visible=true,enabled=enabled,reason=not enabled and (spec[1]=="declare_war" and "War cannot be declared now." or reason) or nil,tooltip=not enabled and reason or nil}
    end
    -- SIMPLIFIED(SIMP-457): this is a leader dossier with war/close actions; no AI dialogue or mandatory replies are fabricated.
    return {partner={id=id,name=L(leader.Description),leader_name=L(leader.Description),title="",civ=L(civ.ShortDescription),
        capital=cap and cap:Plot():IsRevealed(Players[player]:GetTeam(),false) and cap:GetName() or "",cities=p:GetNumCities(),score=p:GetScore(),
        ideology=ideology and L(ideology.Description) or nil,is_human=false,alive=true,team=qt,icon="leader:" .. leader.Type,
        emblem="civ:" .. civ.Type,color_bg="",color_fg=""},mood={key=mood(player,id),label=mood(player,id),reasons=reasons},
        mode="main",speaker_line="",phrase="",can_close=not args.popup,relation=relation,at_war=war,main_actions=actions,
        responses=array(),targets=array(),available=false,unavailable_reason=reason}
end,actions={
    discuss=unavailable,trade=unavailable,demand=unavailable,make_peace=unavailable,topic=unavailable,respond=unavailable,
    coop_war=unavailable,confirm=unavailable,back=unavailable,switch=unavailable,
    goodbye=goodbye,
    declare_war=function(player,args)
        assert(not args.popup,"Resolve the pending AI decision first.")
        local p=target(player,current[player])
        assert(Players[player]:IsTurnActive() and team_of(player):CanDeclareWar(p:GetTeam()),"War cannot be declared now.")
        return {open={screen="declare_war",a=p:GetTeam(),b=-1}}
    end,
}}
