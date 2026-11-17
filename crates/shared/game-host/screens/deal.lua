local drafts,current={},{}
local submit_reason="Proposal submission is unavailable: the net emitter cannot carry a serialized deal."
local ai_reason="AI trade needs a seat-scoped deal evaluator and conversation session."
local function integer(n) return type(n)=="number" and n==math.floor(n) end
local function partner(player,id)
    assert(integer(id) and id>=0 and id<GameDefines.MAX_MAJOR_CIVS and id~=player,"Choose another major civilization.")
    local p=Players[id]
    assert(p and p:IsAlive() and met(player,id),"The trade partner is unavailable.")
    return p
end
local function exists(from,to)
    -- Explicit player arguments; unlike scratch-deal loaders this read does not use Active().
    return GcUiProposedDealExists and GcUiProposedDealExists(from,to) or false
end
local function draft(player,id)
    drafts[player]=drafts[player] or {}
    local d=drafts[player][id]
    if not d then d={us={},them={}};drafts[player][id]=d end
    return d
end
local function context(player)
    local id=current[player]
    local p=partner(player,id)
    assert(p:IsHuman(),ai_reason)
    assert(Players[player]:IsTurnActive(),"Only during your turn.")
    assert(not exists(id,player) and not exists(player,id),"Resolve the existing proposal first.")
    assert(not team_of(player):IsAtWar(p:GetTeam()),"Peace drafts require a native deal legality builder.")
    return id,p,draft(player,id)
end
local function capacity(player,id,side,kind,resource)
    assert(side==0 or side==1,"The trade side is invalid.")
    local owner=side==0 and player or id
    local p=Players[owner]
    if kind==0 then
        assert(Players[player]:IsDoF(id),"Lump gold requires a Declaration of Friendship.")
        return math.max(0,p:GetGold()),"Gold",""
    elseif kind==1 then return math.max(0,p:CalculateGoldRate()),"Gold per turn",""
    elseif kind==3 then
        assert(integer(resource) and GameInfo.Resources[resource],"Choose a valid resource.")
        local r=GameInfo.Resources[resource]
        assert(Game.GetResourceUsageType(resource)>0,"Bonus resources cannot be traded.")
        return math.max(0,p:GetNumResourceAvailable(resource,false)),L(r.Description),"resource:" .. r.Type
    end
    error("This trade item needs the native seat-scoped legality builder.")
end
local function change(player,args,mode)
    local id,_,d=context(player)
    assert(integer(args.b),"An item type is required.")
    local max,label,image=capacity(player,id,args.a,args.b,args.c)
    local side=args.a==0 and d.us or d.them
    local key=args.b .. ":" .. (args.c or 0)
    local old=side[key] and side[key].amount or 0
    local n=mode=="max" and max or (mode=="set" and args.d or old+(args.b==0 and 30 or 1))
    assert(integer(n),"The amount must be a whole number.")
    n=math.max(0,math.min(max,n))
    if n==0 then side[key]=nil else side[key]={key=key,label=label,icon=image,amount=n,type=args.b,data1=args.c or 0,data2=0} end
end
local function person(id)
    local p=Players[id]
    local c,l=GameInfo.Civilizations[p:GetCivilizationType()],GameInfo.Leaders[p:GetLeaderType()]
    return {id=id,name=L(l.Description),civ=L(c.ShortDescription),emblem="civ:" .. c.Type,gold=p:GetGold()}
end
local function button(key,label,enabled,reason) return {key=key,label=label,enabled=enabled,reason=reason} end
local function unavailable(reason)
    return {kind="player",state="view",editable=false,pockets_visible={them=false,us=false},
        left_button=button("cancel","Close",true),right_button=button("propose","Propose",false,reason),esc_allowed=true,
        side_note=reason,pockets={them=array(),us=array()},["table"]={them=array(),us=array()}}
end
return {data=function(player,args)
    local ok,p=pcall(partner,player,args.a)
    if not ok then return unavailable("Choose a contacted trade partner first.") end
    local id=args.a
    current[player]=id
    local me,them=person(player),person(id)
    them.is_human,them.icon,them.color_bg,them.color_fg=p:IsHuman(),"leader:" .. GameInfo.Leaders[p:GetLeaderType()].Type,"",""
    them.at_war=team_of(player):IsAtWar(p:GetTeam())
    them.mood=them.at_war and "war" or "at_peace";them.mood_label=them.at_war and "At war" or "At peace";them.war_note=""
    local incoming,outgoing=exists(id,player),exists(player,id)
    local editable=p:IsHuman() and not incoming and not outgoing and not them.at_war and Players[player]:IsTurnActive()
    local reason=not p:IsHuman() and ai_reason or (them.at_war and "Peace drafts require a native deal legality builder." or submit_reason)
    local state=incoming and "player_incoming" or (outgoing and "player_outgoing" or (p:IsHuman() and "player_new" or "own"))
    local pockets,table_rows={them=array(),us=array()},{them=array(),us=array()}
    local d=(incoming or outgoing) and {us={},them={}} or draft(player,id)
    local duration=GameInfo.GameSpeeds[Game.GetGameSpeedType()].DealDuration
    for side=0,1 do
        local key=side==0 and "us" or "them"
        if editable then
            for _,spec in ipairs({{0,"gold","Gold"},{1,"gold_per_turn","Gold per turn"}}) do
                local yes,max=pcall(capacity,player,id,side,spec[1],0)
                pockets[key][#pockets[key]+1]={key=spec[2],label=spec[3],kind="single",amount=yes and max or nil,
                    enabled=yes and max>0,reason=not yes and tostring(max) or (max<=0 and "None available." or nil),
                    ref={type=spec[1],data1=0,data2=0},items=array()}
            end
            local resources=array()
            for r in GameInfo.Resources() do
                if Game.GetResourceUsageType(r.ID)>0 then
                    local max,label,image=capacity(player,id,side,3,r.ID)
                    if max>0 then resources[#resources+1]={key=r.Type,label=label,icon=image,amount=max,enabled=true,ref={type=3,data1=r.ID,data2=0}} end
                end
            end
            pockets[key][#pockets[key]+1]={key="strategic",label="Resources",kind="list",enabled=true,items=resources}
        end
        for _,item in pairs(d[key]) do
            local legal,max=pcall(capacity,player,id,side,item.type,item.data1)
            local amount=legal and math.min(item.amount,max) or item.amount
            table_rows[key][#table_rows[key]+1]={key=item.key,label=item.label,icon=item.icon,sub=legal and "Draft only" or tostring(max),
                amount=amount,amount_max=legal and max or 0,editable=editable and legal,turns=item.type~=0 and duration or nil,
                duration_text=item.type==0 and "Once" or duration .. " turns",removable=editable,fixed=false,destroyed=not legal,
                ref={type=item.type,data1=item.data1,data2=0}}
        end
        table.sort(table_rows[key],function(a,b) return a.key<b.key end)
    end
    -- SIMPLIFIED(SIMP-456): drafts support gold, income and resources; proposing/accepting needs native legality, serialization and a deal-text emitter.
    return {kind=p:IsHuman() and "player" or "ai",state=state,partner=them,me=me,deal_turns=duration,peace_turns=GameDefines.PEACE_TREATY_LENGTH,
        editable=editable,pockets_visible={them=editable,us=editable},
        left_button=incoming and button("refuse","Refuse",true) or (outgoing and button("withdraw","Withdraw",true) or button("clear","Clear the table",editable,not editable and reason or nil)),
        right_button=button(incoming and "accept" or "propose",incoming and "Accept" or "Propose",false,incoming and "Native proposal contents cannot be displayed and revalidated." or reason),esc_allowed=true,side_note=(incoming or outgoing) and "Proposal contents are unavailable. You may refuse or withdraw it, but cannot accept an unseen offer." or reason,
        pockets=pockets,["table"]=table_rows}
end,actions={
    add=function(player,args) change(player,args,"add") end,
    add_max=function(player,args) change(player,args,"max") end,
    set_amount=function(player,args) change(player,args,"set") end,
    remove=function(player,args)
        local _,_,d=context(player)
        assert(args.a==0 or args.a==1,"The trade side is invalid.")
        assert(integer(args.b) and integer(args.c),"The item is invalid.")
        d[args.a==0 and "us" or "them"][args.b .. ":" .. args.c]=nil
    end,
    clear=function(player) local _,_,d=context(player);d.us={};d.them={} end,
    propose=function() error(submit_reason) end,
    accept=function() error("Cannot accept a proposal whose native contents cannot be displayed and revalidated.") end,
    refuse=function(player,_,net)
        local id=current[player];partner(player,id)
        assert(exists(id,player),"No incoming proposal remains.")
        net.send("sendNetPlayerDealFinalized",id,player,0);return {close=true}
    end,
    withdraw=function(player,_,net)
        local id=current[player];partner(player,id)
        assert(exists(player,id),"No outgoing proposal remains.")
        net.send("sendNetPlayerDealFinalized",player,id,0);return {close=true}
    end,
    ask=function() error(ai_reason) end,demand=function() error(ai_reason) end,
    modify=function() error("Proposal contents cannot be copied without a read-only native deal iterator.") end,
    cancel=function() return {close=true} end,
    close=function(player,_,net)
        local id=current[player]
        if id and exists(id,player) then net.send("sendNetPlayerDealFinalized",id,player,0)
        elseif id and exists(player,id) then net.send("sendNetPlayerDealFinalized",player,id,0)
        elseif id then drafts[player][id]=nil end
        return {close=true}
    end,
    declare_war=function(player)
        local id=current[player];local p=partner(player,id)
        assert(team_of(player):CanDeclareWar(p:GetTeam()),"War cannot be declared now.")
        return {open={screen="declare_war",a=p:GetTeam(),b=-1}}
    end,
    switch=function() error("Close this draft before changing partners.") end,
}}
