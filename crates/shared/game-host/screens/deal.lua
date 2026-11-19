-- The trade screen of a seat (the original's DiploTrade / TradeLogic). With a human partner the seat edits a draft and
-- answers proposals; with an AI partner the table is checked, valued and answered by the core for this seat
-- (GcUiDealQuery: the AI's legality, its "what would make this work" and its verdict), and an accepted offer is the
-- seat's ordered sendNetDealAccepted carrying the core's own deal blob.
local drafts,current={},{}
local submit_reason="Proposal submission is unavailable: the net emitter cannot carry a serialized deal."
local peace_reason="Peace talks with an AI are not available yet."
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
    if not d then d={us={},them={},state="own"};drafts[player][id]=d end
    return d
end
local function button(key,label,enabled,reason) return {key=key,label=label,enabled=enabled,reason=reason} end
local function person(id)
    local p=Players[id]
    local c,l=GameInfo.Civilizations[p:GetCivilizationType()],GameInfo.Leaders[p:GetLeaderType()]
    return {id=id,name=L(l.Description),civ=L(c.ShortDescription),emblem="civ:" .. c.Type,gold=p:GetGold()}
end

-- ---- a human partner: a draft of gold, income and resources ----
local function context(player)
    local id=current[player]
    local p=partner(player,id)
    assert(p:IsHuman(),"Talk to this leader first.")
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
local function unavailable(reason)
    return {kind="player",state="view",editable=false,pockets_visible={them=false,us=false},
        left_button=button("cancel","Close",true),right_button=button("propose","Propose",false,reason),esc_allowed=true,
        side_note=reason,pockets={them=array(),us=array()},["table"]={them=array(),us=array()}}
end
local function human_data(player,id,p,me,them)
    local incoming,outgoing=exists(id,player),exists(player,id)
    local editable=not incoming and not outgoing and not them.at_war and Players[player]:IsTurnActive()
    local reason=them.at_war and "Peace drafts require a native deal legality builder." or submit_reason
    local state=incoming and "player_incoming" or (outgoing and "player_outgoing" or "player_new")
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
    -- SIMPLIFIED(SIMP-456): drafts with a human support gold, income and resources; proposing to a human needs a deal-text emitter.
    return {kind="player",state=state,partner=them,me=me,deal_turns=duration,peace_turns=GameDefines.PEACE_TREATY_LENGTH,
        editable=editable,pockets_visible={them=editable,us=editable},
        left_button=incoming and button("refuse","Refuse",true) or (outgoing and button("withdraw","Withdraw",true) or button("clear","Clear the table",editable,not editable and reason or nil)),
        right_button=button(incoming and "accept" or "propose",incoming and "Accept" or "Propose",false,incoming and "Native proposal contents cannot be displayed and revalidated." or reason),esc_allowed=true,side_note=(incoming or outgoing) and "Proposal contents are unavailable. You may refuse or withdraw it, but cannot accept an unseen offer." or reason,
        pockets=pockets,["table"]=table_rows}
end

-- ---- an AI partner: the core's table ----
local T=TradeableItems or {}
local function is_ai(player)
    local id=current[player]
    return id~=nil and Players[id]~=nil and not Players[id]:IsHuman()
end
local function item_key(e)
    if e.type==T.TRADE_ITEM_RESOURCES or e.type==T.TRADE_ITEM_THIRD_PARTY_PEACE or e.type==T.TRADE_ITEM_THIRD_PARTY_WAR then
        return e.type .. ":" .. e.data1
    end
    return e.type .. ":0"
end
local function amount_of(e)
    if e.type==T.TRADE_ITEM_GOLD or e.type==T.TRADE_ITEM_GOLD_PER_TURN then return e.data1 end
    if e.type==T.TRADE_ITEM_RESOURCES then return e.data2 end
    return nil
end
-- The items of the draft for the core, the seat's side first, each side in key order (TradeLogic adds them in turn).
local function core_items(player,id,d,skip)
    local out={}
    for side=0,1 do
        local list,keys=side==0 and d.us or d.them,{}
        for k in pairs(list) do keys[#keys+1]=k end
        table.sort(keys)
        for _,k in ipairs(keys) do
            local e=list[k]
            if not (skip and skip.side==side and skip.key==k) then
                out[#out+1]={type=e.type,from=side==0 and player or id,data1=e.data1,data2=e.data2 or 0,data3=e.data3 or 0,flag1=e.flag1}
            end
        end
    end
    return out
end
local function ai_query(player,id,items,op,mode)
    assert(GcUiDealQuery,"Trade with an AI needs the game core.")
    return GcUiDealQuery(player,id,items,op,mode)
end
-- What a side may still put on the table: the core's pocket rows of that side, by item key.
local function ai_pockets(player,id,d,skip)
    local by={[0]={},[1]={}}
    for _,r in ipairs(ai_query(player,id,core_items(player,id,d,skip),"pockets")) do
        local side=r.from==player and 0 or 1
        by[side][item_key(r)]=r
    end
    return by
end
local function label_of(player,e)
    if e.type==T.TRADE_ITEM_GOLD then return "Gold","" end
    if e.type==T.TRADE_ITEM_GOLD_PER_TURN then return "Gold per turn","" end
    if e.type==T.TRADE_ITEM_RESOURCES then
        local r=GameInfo.Resources[e.data1]
        return r and L(r.Description) or "Resource",r and "resource:" .. r.Type or ""
    end
    if e.type==T.TRADE_ITEM_ALLOW_EMBASSY then return "Embassy","" end
    if e.type==T.TRADE_ITEM_OPEN_BORDERS then return "Open borders","" end
    if e.type==T.TRADE_ITEM_DEFENSIVE_PACT then return "Defensive pact","" end
    if e.type==T.TRADE_ITEM_RESEARCH_AGREEMENT then return "Research agreement","" end
    if e.type==T.TRADE_ITEM_PEACE_TREATY then return "Peace treaty","" end
    return "Item","" end
local function ai_context(player)
    local id=current[player]
    local p=partner(player,id)
    assert(not p:IsHuman(),"This partner is not an AI.")
    assert(Players[player]:IsTurnActive(),"Only during your turn.")
    assert(not team_of(player):IsAtWar(p:GetTeam()),peace_reason)
    return id,p,draft(player,id)
end
local function ai_change(player,args,mode)
    local id,_,d=ai_context(player)
    assert(args.a==0 or args.a==1,"The trade side is invalid.")
    assert(integer(args.b),"An item type is required.")
    local e={type=args.b,data1=integer(args.c) and args.c or 0,data2=0}
    local key=item_key(e)
    local side=args.a==0 and d.us or d.them
    local row=ai_pockets(player,id,d,{side=args.a,key=key})[args.a][key]
    assert(row,"The core does not allow this item now.")
    local old=side[key] and amount_of(side[key]) or 0
    local n
    if amount_of({type=e.type,data1=0,data2=0})==nil then
        n=1
    else
        n=mode=="max" and row.max or (mode=="set" and args.d or old+(e.type==T.TRADE_ITEM_GOLD and 30 or 1))
        assert(integer(n),"The amount must be a whole number.")
        n=math.max(0,math.min(row.max,n))
    end
    if n==0 then side[key]=nil return end
    if e.type==T.TRADE_ITEM_GOLD or e.type==T.TRADE_ITEM_GOLD_PER_TURN then e.data1=n elseif e.type==T.TRADE_ITEM_RESOURCES then e.data2=n end
    side[key]=e
    d.state="own"
end
-- What the AI is asked by the "?" button (TradeLogic): what it wants for its items, what it gives for ours, or both.
local function question_of(d)
    local us,them=next(d.us)~=nil,next(d.them)~=nil
    if them and not us then return {key="what_want",label="What do you want for this?",hint="The AI asks for what it values these items at."},1 end
    if us and not them then return {key="what_give",label="What will you give me for this?",hint="The AI offers what it values your items at."},2 end
    return {key="equalize",label="What would make this deal work?",hint="The AI balances the table."},0
end
local function ai_data(player,id,p,me,them)
    local d=draft(player,id)
    local s=leader_session(player,id)
    local turn=Players[player]:IsTurnActive()
    local editable=turn and not them.at_war
    local reason=them.at_war and peace_reason or (not turn and "Only during your turn." or nil)
    local duration=GameInfo.GameSpeeds[Game.GetGameSpeedType()].DealDuration
    local pockets,table_rows={them=array(),us=array()},{them=array(),us=array()}
    local items=core_items(player,id,d)
    local taken=GcUiDealQuery and ai_query(player,id,items,"check") or {}
    local legal_by={}
    for i,it in ipairs(items) do legal_by[(it.from==player and 0 or 1) .. "/" .. item_key(it)]=taken[i] end
    local rows=editable and GcUiDealQuery and ai_pockets(player,id,d) or {[0]={},[1]={}}
    local sections={{T.TRADE_ITEM_GOLD,"gold","Gold"},{T.TRADE_ITEM_GOLD_PER_TURN,"gold_per_turn","Gold per turn"},
        {T.TRADE_ITEM_ALLOW_EMBASSY,"embassy","Embassy"},{T.TRADE_ITEM_OPEN_BORDERS,"open_borders","Open borders"},
        {T.TRADE_ITEM_DEFENSIVE_PACT,"pact","Defensive pact"},{T.TRADE_ITEM_RESEARCH_AGREEMENT,"research_agreement","Research agreement"}}
    for side=0,1 do
        local key,list=side==0 and "us" or "them",side==0 and d.us or d.them
        if editable then
            for _,spec in ipairs(sections) do
                local k=spec[1] .. ":0"
                local r=rows[side][k]
                if r and not list[k] then
                    local counted=spec[1]==T.TRADE_ITEM_GOLD or spec[1]==T.TRADE_ITEM_GOLD_PER_TURN
                    pockets[key][#pockets[key]+1]={key=spec[2],label=spec[3],kind="single",amount=counted and r.max or nil,
                        enabled=true,ref={type=spec[1],data1=0,data2=0},items=array()}
                end
            end
            local lux,strat=array(),array()
            for r in GameInfo.Resources() do
                local k=T.TRADE_ITEM_RESOURCES .. ":" .. r.ID
                local row=rows[side][k]
                if row and not list[k] then
                    local target=Game.GetResourceUsageType(r.ID)==2 and lux or strat
                    target[#target+1]={key=r.Type,label=L(r.Description),icon="resource:" .. r.Type,amount=row.max,enabled=true,ref={type=T.TRADE_ITEM_RESOURCES,data1=r.ID,data2=0}}
                end
            end
            if #lux>0 then pockets[key][#pockets[key]+1]={key="luxury",label="Luxury resources",kind="list",enabled=true,items=lux} end
            if #strat>0 then pockets[key][#pockets[key]+1]={key="strategic",label="Strategic resources",kind="list",enabled=true,items=strat} end
        end
        for k,e in pairs(list) do
            local label,image=label_of(player,e)
            local legal=legal_by[side .. "/" .. k]
            local amount=amount_of(e)
            -- Gold, income and strategic resources take an amount; a luxury is one copy.
            local adjustable=amount~=nil and (e.type~=T.TRADE_ITEM_RESOURCES or Game.GetResourceUsageType(e.data1)==1)
            local row=adjustable and editable and GcUiDealQuery and ai_pockets(player,id,d,{side=side,key=k})[side][k] or nil
            local once=e.type==T.TRADE_ITEM_GOLD or e.type==T.TRADE_ITEM_ALLOW_EMBASSY
            table_rows[key][#table_rows[key]+1]={key=k,label=label,icon=image,sub=legal and "" or "The core refuses this item now.",
                amount=amount,amount_max=row and row.max or nil,editable=row and true or false,turns=not once and duration or nil,
                duration_text=once and "Once" or duration .. " turns",removable=editable,fixed=false,destroyed=not legal,
                ref={type=e.type,data1=e.data1,data2=e.data2 or 0}}
        end
        table.sort(table_rows[key],function(a,b) return a.key<b.key end)
    end
    local any=#items>0
    local question=editable and any and question_of(d) or nil
    local state=d.state or "own"
    -- SIMPLIFIED(SIMP-456): the AI table holds gold, income, resources, embassies, open borders, defensive pacts and research
    -- agreements; cities, other civilizations' wars and peace, and votes of the original trade screen are not offered.
    return {kind="ai",state=state,partner=them,me=me,deal_turns=duration,peace_turns=GameDefines.PEACE_TREATY_LENGTH,
        phrase=d.phrase or (s and s.trade_phrase) or nil,editable=editable,pockets_visible={them=editable,us=editable},question=question,
        left_button=button("clear","Clear the table",editable and any,not editable and reason or nil),
        right_button=button("propose","Propose",editable and any,not editable and reason or (not any and "Put something on the table." or nil)),
        esc_allowed=true,side_note=reason or "",pockets=pockets,["table"]=table_rows}
end
-- Leaving the AI's table goes back to its leader (the original's trade screen sits in the leader screen); the draft is gone.
local function ai_close(player)
    local id=current[player]
    if drafts[player] then drafts[player][id]=nil end
    return {close=true,open=leader_session(player,id) and {screen="leader",a=id} or nil}
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
    if p:IsHuman() then return human_data(player,id,p,me,them) end
    if not (GcUiDealQuery and T.TRADE_ITEM_GOLD) then
        local u=unavailable("Trade with an AI needs the game core.")
        u.kind,u.partner,u.me="ai",them,me
        return u
    end
    return ai_data(player,id,p,me,them)
end,actions={
    add=function(player,args)
        if is_ai(player) then return ai_change(player,args,"add") end
        change(player,args,"add")
    end,
    add_max=function(player,args)
        if is_ai(player) then return ai_change(player,args,"max") end
        change(player,args,"max")
    end,
    set_amount=function(player,args)
        if is_ai(player) then return ai_change(player,args,"set") end
        change(player,args,"set")
    end,
    remove=function(player,args)
        local ai=is_ai(player)
        local _,_,d=(ai and ai_context or context)(player)
        assert(args.a==0 or args.a==1,"The trade side is invalid.")
        assert(integer(args.b) and integer(args.c),"The item is invalid.")
        local key=ai and item_key({type=args.b,data1=args.c}) or args.b .. ":" .. args.c
        d[args.a==0 and "us" or "them"][key]=nil
        d.state="own"
    end,
    clear=function(player)
        local _,_,d=(is_ai(player) and ai_context or context)(player)
        d.us={};d.them={};d.state="own"
    end,
    -- UI.DoProposeDeal to an AI: DoHumanOfferDealToThisAI's verdict for this seat; accepted, the deal goes out as the
    -- seat's sendNetDealAccepted (the core finalizes it in order, where every item is checked again), refused, the
    -- leader says so and the table stays.
    propose=function(player,_,net)
        local id=current[player]
        if not is_ai(player) then error(submit_reason) end
        local _,_,d=ai_context(player)
        local items=core_items(player,id,d)
        assert(#items>0,"Put something on the table.")
        for _,ok in ipairs(ai_query(player,id,items,"check")) do assert(ok,"The core refuses an item on the table.") end
        local verdict=ai_query(player,id,items,"evaluate")
        local s=leader_session(player,id)
        if verdict.response=="accept" then
            net.send_deal("sendNetDealAccepted",ai_query(player,id,items,"blob"),id,verdict.value,verdict.mine,verdict.theirs)
            drafts[player][id]=nil
            if s then s.mode,s.phrase,s.offers="main","",0 end
            return {close=true,open=s and {screen="leader",a=id} or nil}
        end
        -- The original counts the refused offers of this screen (UI.GetOfferTradeRepeatCount) for the repeat lines.
        local n=(s and s.offers or 0)+1
        if s then s.offers=n end
        local line=n>4 and "REPEAT_TRADE_TOO_MUCH" or (n>1 and "REPEAT_TRADE" or (verdict.response=="insulting" and "TRADE_REJECT_INSULTING" or "TRADE_REJECT_UNACCEPTABLE"))
        d.state,d.phrase="ai_reply_no",plain(GcUiDiploText(player,id,line))
    end,
    -- UI.DoEqualizeDealWithHuman / DoWhatDoesAIWant / DoWhatWillAIGive: the AI rebuilds the table on a copy; what it adds
    -- or removes becomes the seat's draft.
    ask=function(player)
        local id=current[player]
        if not (id and Players[id] and not Players[id]:IsHuman()) then error("Only an AI answers this question.") end
        local _,_,d=ai_context(player)
        local items=core_items(player,id,d)
        assert(#items>0,"Put something on the table.")
        local _,mode=question_of(d)
        local r=ai_query(player,id,items,"equalize",mode)
        local line
        if r.ok and r.good then line="TRADE_DEAL_UNCHANGED"
        elseif r.ok then
            local us,them={},{}
            for _,it in ipairs(r.items) do
                local e={type=it.type,data1=it.data1,data2=it.data2,data3=it.data3,flag1=it.flag1}
                if label_of(player,e)~="Item" then (it.from==player and us or them)[item_key(e)]=e end
            end
            d.us,d.them=us,them
            line="TRADE_AI_MAKES_OFFER"
        else line=r.cant_match and "TRADE_CANT_MATCH_OFFER" or "TRADE_NO_DEAL_POSSIBLE" end
        d.state=r.ok and not r.good and "ai_offer" or "own"
        d.phrase=plain(GcUiDiploText(player,id,line))
    end,
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
    demand=function() error("Demands of an AI are not available yet.") end,
    modify=function() error("Proposal contents cannot be copied without a read-only native deal iterator.") end,
    cancel=function(player)
        local id=current[player]
        if id and Players[id] and not Players[id]:IsHuman() then return ai_close(player) end
        return {close=true}
    end,
    close=function(player,_,net)
        local id=current[player]
        if id and Players[id] and not Players[id]:IsHuman() then return ai_close(player) end
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
