-- The trade screen of a seat (the original's DiploTrade / TradeLogic). With a human partner the seat edits a draft and
-- answers proposals; with an AI partner the table is checked, valued and answered by the core for this seat
-- (GcUiDealQuery: the AI's legality, its "what would make this work" and its verdict), and an accepted offer is the
-- seat's ordered sendNetDealAccepted carrying the core's own deal blob.
local drafts,current={},{}
local submit_reason="Proposal submission is unavailable: the net emitter cannot carry a serialized deal."
local peace_reason="At war: talk peace from the leader screen."
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
-- A vote commitment names its proposal and whether it is a repeal: the item's ref folds both into data1.
local function vote_ref(e) return e.data1*2+(e.flag1 and 1 or 0) end
local function item_key(e)
    if e.type==T.TRADE_ITEM_RESOURCES or e.type==T.TRADE_ITEM_THIRD_PARTY_PEACE or e.type==T.TRADE_ITEM_THIRD_PARTY_WAR then
        return e.type .. ":" .. e.data1
    end
    if e.type==T.TRADE_ITEM_CITIES then return e.type .. ":" .. e.data1 .. ":" .. (e.data2 or 0) end
    if e.type==T.TRADE_ITEM_VOTE_COMMITMENT then return e.type .. ":" .. vote_ref(e) .. ":" .. (e.data2 or 0) end
    return e.type .. ":0"
end
-- The item a pocket or table click names (a: side, b: type, c: ref.data1, d: ref.data2).
local function entry_of(args)
    local c,d=integer(args.c) and args.c or 0,integer(args.d) and args.d or 0
    local e={type=args.b,data1=0,data2=0}
    if args.b==T.TRADE_ITEM_RESOURCES or args.b==T.TRADE_ITEM_THIRD_PARTY_PEACE or args.b==T.TRADE_ITEM_THIRD_PARTY_WAR then e.data1=c
    elseif args.b==T.TRADE_ITEM_CITIES then e.data1,e.data2=c,d
    elseif args.b==T.TRADE_ITEM_VOTE_COMMITMENT then e.data1,e.flag1,e.data2=math.floor(c/2),c%2==1,d end
    return e
end
-- TradeLogic puts these on both sides at once and takes them off both sides at once.
local function mutual(kind)
    return kind~=nil and (kind==T.TRADE_ITEM_DEFENSIVE_PACT or kind==T.TRADE_ITEM_RESEARCH_AGREEMENT or kind==T.TRADE_ITEM_TRADE_AGREEMENT
        or kind==T.TRADE_ITEM_PEACE_TREATY)
end
local function ref_of(e)
    if e.type==T.TRADE_ITEM_VOTE_COMMITMENT then return {type=e.type,data1=vote_ref(e),data2=e.data2 or 0} end
    return {type=e.type,data1=e.data1,data2=e.data2 or 0}
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
                out[#out+1]={type=e.type,from=side==0 and player or id,data1=e.data1,data2=e.data2 or 0,data3=e.data3 or 0,flag1=e.flag1 or false}
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
local function team_name(team)
    for id=0,GameDefines.MAX_MAJOR_CIVS-1 do
        local p=Players[id]
        if p and p:GetTeam()==team then return L(GameInfo.Civilizations[p:GetCivilizationType()].ShortDescription) end
    end
    local t=Teams[team]
    return t and t.GetName and t:GetName() or "Another civilization"
end
local function city_name(x,y)
    local plot=Map and Map.GetPlot(x,y)
    local city=plot and plot:GetPlotCity()
    return city and city:GetName() or "City"
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
    if e.type==T.TRADE_ITEM_TRADE_AGREEMENT then return "Trade agreement","" end
    if e.type==T.TRADE_ITEM_PEACE_TREATY then return "Peace treaty","" end
    if e.type==T.TRADE_ITEM_CITIES then return city_name(e.data1,e.data2),"" end
    if e.type==T.TRADE_ITEM_THIRD_PARTY_PEACE then return "Peace with " .. team_name(e.data1),"" end
    if e.type==T.TRADE_ITEM_THIRD_PARTY_WAR then return "War on " .. team_name(e.data1),"" end
    if e.type==T.TRADE_ITEM_VOTE_COMMITMENT then return e.label or "Vote commitment","" end
    return "Item","" end
-- The leader session decides what kind of table this is: a trade, the seat's demand, or peace talks (leader.lua).
local function table_mode(player,id)
    local s=leader_session(player,id)
    return s and s.deal_mode or "trade"
end
local function ai_context(player)
    local id=current[player]
    local p=partner(player,id)
    assert(not p:IsHuman(),"This partner is not an AI.")
    assert(Players[player]:IsTurnActive(),"Only during your turn.")
    local d=draft(player,id)
    assert(d.peace or not team_of(player):IsAtWar(p:GetTeam()),peace_reason)
    return id,p,d
end
local function ai_change(player,args,mode)
    local id,_,d=ai_context(player)
    assert(args.a==0 or args.a==1,"The trade side is invalid.")
    assert(integer(args.b),"An item type is required.")
    assert(not (d.demand and args.a==0),"A demand asks for their items only.")
    local e=entry_of(mode=="set" and {b=args.b,c=args.c,d=0} or args)
    local key=item_key(e)
    assert(not (d.fixed and d.fixed[args.a .. "/" .. key]),"The peace treaty stays on the table.")
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
    if e.type==T.TRADE_ITEM_VOTE_COMMITMENT then e.data3,e.label=row.data3,row.label end
    side[key]=e
    if mutual(e.type) then
        local other=args.a==0 and d.them or d.us
        other[key]={type=e.type,data1=e.data1,data2=e.data2}
    end
    d.state="own"
end
-- Peace talks open the table with the core's peace treaties and the allies' peace (DoTradeScreenOpened); those stay put.
local function peace_draft(player,id,items)
    local d={us={},them={},state="own",peace=true,fixed={}}
    for _,it in ipairs(items or {}) do
        local e={type=it.type,data1=it.data1,data2=it.data2,data3=it.data3,flag1=it.flag1}
        local side=it.from==player and 0 or 1
        local key=item_key(e)
        ;(side==0 and d.us or d.them)[key]=e
        d.fixed[side .. "/" .. key]=true
    end
    drafts[player]=drafts[player] or {}
    drafts[player][id]=d
    return d
end
-- What the AI is asked by the "?" button (TradeLogic): what it wants for its items, what it gives for ours, or both.
local function question_of(d)
    local us,them=next(d.us)~=nil,next(d.them)~=nil
    if d.peace then return {key="end_war",label="What will end this war?",hint="The AI says what it wants for peace."},0 end
    if them and not us then return {key="what_want",label="What do you want for this?",hint="The AI asks for what it values these items at."},1 end
    if us and not them then return {key="what_give",label="What will you give me for this?",hint="The AI offers what it values your items at."},2 end
    return {key="equalize",label="What would make this deal work?",hint="The AI balances the table."},0
end
local function list_item(r,label,image)
    return {key=item_key(r),label=label,icon=image or "",amount=r.type==T.TRADE_ITEM_VOTE_COMMITMENT and r.max or nil,enabled=true,ref=ref_of(r)}
end
local function ai_data(player,id,p,me,them)
    local s=leader_session(player,id)
    local mode=table_mode(player,id)
    local d=draft(player,id)
    -- A table left over from another kind of talk starts again.
    if (d.demand and mode~="demand") or (d.peace and not them.at_war) then drafts[player][id]=nil;d=draft(player,id) end
    if mode=="peace" and them.at_war and not d.peace then d=peace_draft(player,id,s and s.peace_items) end
    if mode=="demand" and not d.demand then d.demand=true;d.us={} end
    local turn=Players[player]:IsTurnActive()
    local editable=turn and (not them.at_war or d.peace==true)
    local reason=them.at_war and not d.peace and peace_reason or (not turn and "Only during your turn." or nil)
    local duration=GameInfo.GameSpeeds[Game.GetGameSpeedType()].DealDuration
    local pockets,table_rows={them=array(),us=array()},{them=array(),us=array()}
    local items=core_items(player,id,d)
    local taken=GcUiDealQuery and ai_query(player,id,items,"check") or {}
    local legal_by={}
    for i,it in ipairs(items) do legal_by[(it.from==player and 0 or 1) .. "/" .. item_key(it)]=taken[i] end
    local rows=editable and GcUiDealQuery and ai_pockets(player,id,d) or {[0]={},[1]={}}
    local sections={{T.TRADE_ITEM_GOLD,"gold","Gold"},{T.TRADE_ITEM_GOLD_PER_TURN,"gold_per_turn","Gold per turn"},
        {T.TRADE_ITEM_ALLOW_EMBASSY,"embassy","Embassy"},{T.TRADE_ITEM_OPEN_BORDERS,"open_borders","Open borders"},
        {T.TRADE_ITEM_DEFENSIVE_PACT,"pact","Defensive pact"},{T.TRADE_ITEM_RESEARCH_AGREEMENT,"research_agreement","Research agreement"},
        {T.TRADE_ITEM_TRADE_AGREEMENT,"trade_agreement","Trade agreement"}}
    for side=0,1 do
        local key,list=side==0 and "us" or "them",side==0 and d.us or d.them
        if editable and not (d.demand and side==0) then
            for _,spec in ipairs(sections) do
                local k=spec[1]~=nil and spec[1] .. ":0" or ""
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
            -- Cities, a third team's peace and war, and votes: the core's rows in its order.
            local more={{T.TRADE_ITEM_CITIES,"cities","Cities"},{T.TRADE_ITEM_THIRD_PARTY_PEACE,"third_peace","Make peace with"},
                {T.TRADE_ITEM_THIRD_PARTY_WAR,"third_war","Declare war on"},{T.TRADE_ITEM_VOTE_COMMITMENT,"votes","World Congress votes"}}
            for _,spec in ipairs(more) do
                local found=array()
                if spec[1]~=nil then
                    local ordered={}
                    for k,r in pairs(rows[side]) do if r.type==spec[1] and not list[k] then ordered[#ordered+1]=k end end
                    table.sort(ordered)
                    for _,k in ipairs(ordered) do
                        local r=rows[side][k]
                        local e={type=r.type,data1=r.data1,data2=r.data2,flag1=r.flag1,label=r.label}
                        found[#found+1]=list_item(r,(label_of(player,e)))
                    end
                end
                if #found>0 then pockets[key][#pockets[key]+1]={key=spec[2],label=spec[3],kind="list",enabled=true,items=found} end
            end
        end
        for k,e in pairs(list) do
            local label,image=label_of(player,e)
            local legal=legal_by[side .. "/" .. k]
            local amount=amount_of(e)
            local fixed=d.fixed and d.fixed[side .. "/" .. k] or false
            -- Gold, income and strategic resources take an amount; a luxury is one copy.
            local adjustable=amount~=nil and (e.type~=T.TRADE_ITEM_RESOURCES or Game.GetResourceUsageType(e.data1)==1)
            local row=adjustable and editable and GcUiDealQuery and ai_pockets(player,id,d,{side=side,key=k})[side][k] or nil
            local once=e.type==T.TRADE_ITEM_GOLD or e.type==T.TRADE_ITEM_ALLOW_EMBASSY or e.type==T.TRADE_ITEM_CITIES
                or e.type==T.TRADE_ITEM_THIRD_PARTY_WAR or e.type==T.TRADE_ITEM_VOTE_COMMITMENT
            local peace=e.type==T.TRADE_ITEM_PEACE_TREATY or e.type==T.TRADE_ITEM_THIRD_PARTY_PEACE
            local turns=not once and (peace and GameDefines.PEACE_TREATY_LENGTH or duration) or nil
            table_rows[key][#table_rows[key]+1]={key=k,label=label,icon=image,sub=legal and "" or "The core refuses this item now.",
                amount=amount or (e.type==T.TRADE_ITEM_VOTE_COMMITMENT and e.data3 or nil),amount_max=row and row.max or nil,editable=row and true or false,
                turns=turns,duration_text=once and "Once" or turns .. " turns",removable=editable and not fixed,fixed=fixed,destroyed=not legal,
                ref=ref_of(e)}
        end
        table.sort(table_rows[key],function(a,b) return a.key<b.key end)
    end
    local any=#items>0
    local question=editable and any and not d.demand and question_of(d) or nil
    local state=d.state or "own"
    if state=="own" and d.peace then state="peace" elseif state=="own" and d.demand then state="demand" end
    -- TradeLogic's pockets with an AI: every item the core allows (a declaration of friendship is traded between humans only).
    local right=d.demand and button("demand","Demand",editable and any,not editable and reason or (not any and "Put something of theirs on the table." or nil))
        or button("propose","Propose",editable and any,not editable and reason or (not any and "Put something on the table." or nil))
    return {kind="ai",state=state,partner=them,me=me,deal_turns=duration,peace_turns=GameDefines.PEACE_TREATY_LENGTH,
        phrase=d.phrase or (s and s.trade_phrase) or nil,editable=editable,pockets_visible={them=editable,us=editable and not d.demand},question=question,
        left_button=button("clear","Clear the table",editable and any,not editable and reason or nil),
        right_button=right,esc_allowed=true,side_note=reason or "",pockets=pockets,["table"]=table_rows}
end
-- Leaving the AI's table goes back to its leader (the original's trade screen sits in the leader screen); the draft is gone.
local function ai_close(player)
    local id=current[player]
    if drafts[player] then drafts[player][id]=nil end
    local s=leader_session(player,id)
    if s then s.deal_mode,s.peace_items=nil,nil end
    return {close=true,open=s and {screen="leader",a=id} or nil}
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
        local key=ai and item_key(entry_of(args)) or args.b .. ":" .. args.c
        assert(not (d.fixed and d.fixed[args.a .. "/" .. key]),"The peace treaty stays on the table.")
        d[args.a==0 and "us" or "them"][key]=nil
        if ai and mutual(args.b) then d[args.a==0 and "them" or "us"][key]=nil end
        d.state="own"
    end,
    clear=function(player)
        local _,_,d=(is_ai(player) and ai_context or context)(player)
        -- Peace talks keep their treaties on the table.
        for _,side in ipairs({"us","them"}) do
            local kept={}
            for k,e in pairs(d[side]) do if d.fixed and d.fixed[(side=="us" and 0 or 1) .. "/" .. k] then kept[k]=e end end
            d[side]=kept
        end
        d.state="own"
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
            if s then s.mode,s.phrase,s.offers,s.deal_mode,s.peace_items="main","",0,nil,nil end
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
        assert(not d.demand,"A demand is not a question.")
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
                local side,key=it.from==player and "us" or "them",item_key(e)
                local old=d[side][key]
                if old and old.label then e.label=old.label end
                if label_of(player,e)~="Item" then (side=="us" and us or them)[key]=e end
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
    -- The original's UI.DoDemand: the AI's answer for this seat (the decision half of CvDealAI's DoHumanDemand); the answer goes out as the seat's
    -- FROM_UI_DIPLO_EVENT_HUMAN_DEMAND (it starts the AI's demand timer everywhere), and a demand the AI gives in to as the
    -- seat's sendNetDemandAccepted with the core's deal blob. The leader says its answer at its root.
    demand=function(player,_,net)
        local id=current[player]
        if not is_ai(player) then error("Only an AI answers a demand.") end
        local _,_,d=ai_context(player)
        assert(d.demand,"Open a demand from the leader first.")
        local items=core_items(player,id,d)
        assert(#items>0,"Put something of theirs on the table.")
        for _,it in ipairs(items) do assert(it.from==id,"A demand asks for their items only.") end
        for _,ok in ipairs(ai_query(player,id,items,"check")) do assert(ok,"The core refuses an item on the table.") end
        local r=ai_query(player,id,items,"demand")
        local line={accept="HUMAN_DEMAND_YES",refuse_weak="HUMAN_DEMAND_REFUSE_WEAK",refuse_hostile="HUMAN_DEMAND_REFUSE_HOSTILE",
            refuse_too_much="HUMAN_DEMAND_REFUSE_TOO_MUCH",refuse_too_soon="HUMAN_DEMAND_REFUSE_TOO_SOON"}
        assert(line[r.response],"The AI cannot answer this demand.")
        local event=FromUIDiploEventTypes and FromUIDiploEventTypes.FROM_UI_DIPLO_EVENT_HUMAN_DEMAND
        assert(event,"This diplomacy event is unavailable.")
        net.send("sendFromUIDiploEvent",id,event,r.code,-1)
        if r.response=="accept" then net.send_deal("sendNetDemandAccepted",ai_query(player,id,items,"blob"),id) end
        drafts[player][id]=nil
        local s=leader_session(player,id)
        if s then s.mode,s.deal_mode,s.offers="main",nil,0;s.phrase=plain(GcUiDiploText(player,id,line[r.response])) end
        return {close=true,open=s and {screen="leader",a=id} or nil}
    end,
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
