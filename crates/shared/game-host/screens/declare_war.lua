local function integer(n) return type(n)=="number" and n==math.floor(n) end
local function person(id)
    local p=Players[id]
    local civ=GameInfo.Civilizations[p:GetCivilizationType()]
    local leader=GameInfo.Leaders[p:GetLeaderType()]
    return {id=id,name=p:IsMinorCiv() and L(p:GetCivilizationShortDescriptionKey()) or L(leader.Description),
        leader=L(leader.Description),civ=L(civ.ShortDescription),icon_leader="leader:" .. leader.Type,
        icon_civ="civ:" .. civ.Type,color_bg="",color_fg=""}
end
local function target(player, team)
    assert(integer(team) and team>=0 and team<(GameDefines.MAX_CIV_TEAMS or GameDefines.MAX_CIV_PLAYERS),"A valid rival team is required.")
    assert(team~=Players[player]:GetTeam() and team_of(player):IsHasMet(team),"This team is unavailable.")
    local t=Teams[team]
    local id=t:GetLeaderID()
    assert(id>=0 and Players[id] and Players[id]:IsAlive(),"The rival team is no longer alive.")
    return id,t
end
local function context(player,args)
    local popup=args.popup
    local team=popup and popup.data1 or args.a
    local reason="explicit"
    if popup then
        if popup.kind=="BUTTONPOPUP_DECLAREWARMOVE" then reason="move"
        elseif popup.kind=="BUTTONPOPUP_DECLAREWARRANGESTRIKE" then reason="range_attack"
        elseif popup.kind=="BUTTONPOPUP_DECLAREWAR_PLUNDER_TRADE_ROUTE" then reason="plunder"
        else error("This confirmation payload is unsupported.") end
    end
    local id,t=target(player,team)
    return team,id,t,reason,popup
end
local function yields(route,from)
    local out=array()
    for _,spec in ipairs({{"gold",from and "FromGPT" or "ToGPT"},{"sci",from and "FromScience" or "ToScience"},{"food",from and "FromFood" or "ToFood"},{"prod",from and "FromProduction" or "ToProduction"}}) do
        local n=route[spec[2]] or 0
        if n>0 then out[#out+1]={yield=spec[1],value=n/100} end
    end
    return out
end
local function data(player,args)
    -- Registry probes may query this window before a rival has been selected.
    if not args.popup and (args.a==nil or args.a<0 or args.a==Players[player]:GetTeam()) then
        return {kind="declare_war",reason="explicit",enabled=false,disabled_reason="Choose a contacted rival team first.",
            diplomacy=array(),protectors=array(),allied_city_states=array(),deals_from_them=array(),deals_from_you=array(),vote_promises=array(),routes=array()}
    end
    local team,id,t,reason,popup=context(player,args)
    local p,q=Players[player],Players[id]
    local row=person(id)
    row.team,row.is_city_state,row.is_human=team,q:IsMinorCiv(),q:IsHuman()
    local diplomacy,protectors,allies,routes=array(),array(),array(),array()
    if not q:IsMinorCiv() then
        if p:IsDoF(id) then diplomacy[#diplomacy+1]={key="friend",text="Declaration of friendship",tone="good",turns=math.max(0,(GameDefines.DOF_EXPIRATION_TIME or 50)-p:GetDoFCounter(id))} end
        if p:IsDenouncedPlayer(id) then diplomacy[#diplomacy+1]={key="we_denounced",text="You denounced them",tone="bad"} end
        if q:IsDenouncedPlayer(player) then diplomacy[#diplomacy+1]={key="they_denounced",text="They denounced you",tone="bad"} end
    end
    local affected={[id]=true}
    for other=0,GameDefines.MAX_CIV_PLAYERS-1 do
        local r=Players[other]
        if r and r:IsAlive() and other~=player and met(player,other) then
            if q:IsMinorCiv() and not r:IsMinorCiv() and r:IsProtectingMinor(id) then
                local v=person(other);v.leader,v.civ=nil,nil;protectors[#protectors+1]=v
            elseif r:IsMinorCiv() and r:GetAlly()==id then
                local minor=GameInfo.MinorCivilizations[r:GetMinorCivType()]
                allies[#allies+1]={id=other,name=L(r:GetCivilizationShortDescriptionKey()),icon="civ:" .. minor.Type,color=""}
                affected[other]=true
            end
        end
    end
    for _,spec in ipairs({{p:GetTradeRoutes(),true},{p:GetTradeRoutesToYou(),false}}) do
        for index,r in ipairs(spec[1]) do
            local other=spec[2] and r.ToID or r.FromID
            if affected[other] then
                routes[#routes+1]={key=(spec[2] and "out:" or "in:") .. index,dir=spec[2] and "out" or "in",
                    from_city=r.FromCityName,to_city=r.ToCityName,other_icon=person(other).icon_civ,
                    ours=yields(r,spec[2]),theirs=yields(r,not spec[2])}
            end
        end
    end
    local enabled=p:IsTurnActive() and team_of(player):CanDeclareWar(team)
    local why=not enabled and "War cannot be declared on this team now." or nil
    -- Native events contain team/x/y, not the selected unit id. Never reinterpret x as a unit.
    if popup then enabled=false;why="The pending unit order has no captured unit id. Cancel and issue an explicit declaration first." end
    -- SIMPLIFIED(SIMP-452): native movement/attack/plunder questions fail closed until their payload captures the acting unit; explicit war works.
    return {kind="declare_war",reason=reason,me=person(player),target=row,question="Declare war on " .. row.civ .. "?",
        plot=popup and {x=popup.data2,y=popup.data3} or nil,diplomacy=diplomacy,protectors=protectors,allied_city_states=allies,
        deals_from_them=array(),deals_from_you=array(),vote_promises=array(),routes=routes,enabled=enabled,disabled_reason=why,
        deals_reason="Active deals and vote commitments are unavailable through a read-only server iterator."}
end
local function cancel(_,args,net)
    if args.popup_id then net.popup_done(args.popup_id) end
    return {close=true}
end
return {data=data,actions={
    cancel=cancel,
    confirm=function(player,args,net)
        local team,_,_,reason,popup=context(player,args)
        assert(Players[player]:IsTurnActive() and team_of(player):CanDeclareWar(team),"War cannot be declared on this team now.")
        assert(integer(args.a) and args.a==team,"The rival team changed.")
        local unit,plot,mission
        if popup then
            assert(integer(args.b) and args.b>=0,"An owned unit is required to resume this order.")
            unit=Players[player]:GetUnitByID(args.b)
            assert(unit and unit:GetOwner()==player and unit:CanMove(),"The acting unit is unavailable.")
            assert(integer(args.c) and integer(args.d) and args.c==popup.data2 and args.d==popup.data3,"The target plot changed.")
            plot=Map.GetPlot(args.c,args.d)
            assert(plot and visible(plot,Players[player]:GetTeam()),"The target plot is not visible.")
            if reason=="move" then mission=MissionTypes.MISSION_MOVE_TO
            elseif reason=="range_attack" then
                assert(unit:CanRangeStrikeAt(args.c,args.d,false,true),"This unit cannot attack that plot.")
                mission=MissionTypes.MISSION_RANGE_ATTACK
            else error("Plunder requires a captured route and warning team; this payload is incomplete.") end
        else assert(args.b==nil or args.b==-1,"An explicit declaration cannot resume a unit order.") end
        assert(mission~=nil or not popup,"The follow-up mission is unavailable.")
        net.send("sendChangeWar",team,1)
        -- SIMPLIFIED(SIMP-453): a resumed order addresses one validated unit, rather than the client's selected stack.
        if unit then net.send("sendPushMission",args.b,mission,plot:GetX(),plot:GetY(),0,0) end
        if args.popup_id then net.popup_done(args.popup_id) end
        return {close=true}
    end,
    plunder=function() error("Plunder is unavailable: the popup does not capture the route, acting unit and affected warning team.") end,
}}
