-- Seat-scoped diplomacy. Display names deliberately use leaders/civilizations, never account names.
local deals_reason = "Deal history is unavailable: this game core has no read-only seat-scoped deal query."
local function person(id)
    local p = Players[id]
    local civ, leader = GameInfo.Civilizations[p:GetCivilizationType()], GameInfo.Leaders[p:GetLeaderType()]
    local name = L(leader.Description)
    return {id=id, name=name, leader=name, civ=L(civ.ShortDescription), icon_leader="leader:" .. leader.Type,
        icon_civ="civ:" .. civ.Type, color_bg="", color_fg="", score=p:GetScore()}
end
local function direction(ours, theirs)
    return ours and (theirs and "both" or "ours") or (theirs and "theirs" or "none")
end
local function remaining(counter)
    return math.max(0, (GameDefines.DOF_EXPIRATION_TIME or 50) - counter)
end
local function resources(p)
    local out = array()
    for r in GameInfo.Resources() do
        local usage = Game.GetResourceUsageType(r.ID)
        if usage == 1 or usage == 2 then
            local n = p:GetNumResourceAvailable(r.ID, false)
            if n > 0 then out[#out+1] = {key=r.Type, name=L(r.Description), count=n,
                kind=usage == 1 and "strategic" or "luxury", icon="resource:" .. r.Type} end
        end
    end
    return out
end
local function cs_resources(player, p)
    local out, counts, seen = array(), {}, {}
    local cap = p:GetCapitalCity()
    if not cap then return out end
    local team = Players[player]:GetTeam()
    for dx=-5,5 do for dy=-5,5 do
        local plot = Map.PlotXYWithRangeCheck(cap:GetX(), cap:GetY(), dx, dy, 5)
        if plot and not seen[plot:GetPlotIndex()] then
            seen[plot:GetPlotIndex()] = true
            local distance = Map.PlotDistance(cap:GetX(), cap:GetY(), plot:GetX(), plot:GetY())
            if plot:IsRevealed(team, false) and (plot:GetOwner() == p:GetID() or (plot:GetOwner() == -1 and distance <= 2)) then
                local id = plot:GetResourceType(team)
                if id >= 0 and Game.GetResourceUsageType(id) > 0 then counts[id] = (counts[id] or 0) + plot:GetNumResource() end
            end
        end
    end end
    for r in GameInfo.Resources() do
        if counts[r.ID] then out[#out+1] = {key=r.Type, name=L(r.Description), count=counts[r.ID],
            kind=Game.GetResourceUsageType(r.ID) == 1 and "strategic" or "luxury", icon="resource:" .. r.Type} end
    end
    return out
end
local function mood(player, p)
    if p:IsHuman() then return "human" end
    -- This getter accepts the other player explicitly; the true hidden approach is never queried.
    local guess = Players[player]:GetApproachTowardsUsGuess(p:GetID())
    for _, name in ipairs({"friendly", "guarded", "hostile", "afraid"}) do
        if guess == GameInfoTypes["MAJOR_CIV_APPROACH_" .. string.upper(name)] then return name end
    end
    return "neutral"
end
-- Deal snapshots of the seat (read-only core queries; no scratch deal, no active player).
local function deal_item(player, deal_key, n, it)
    local T, team = TradeableItems, Players[player]:GetTeam()
    local item = {key=deal_key .. "_" .. n, side=it.from == player and "me" or "them", group="other",
        duration=it.duration > 0 and it.duration or nil}
    local function civ_of_team(t)
        local leader = Teams[t] and Teams[t]:GetLeaderID()
        if leader == nil or leader < 0 or not Players[leader] then return "Unknown civilization" end
        if leader ~= player and not met(player, leader) then return "Unknown civilization" end
        return L(Players[leader]:GetCivilizationShortDescriptionKey())
    end
    if it.type == T.TRADE_ITEM_GOLD then
        item.group, item.kind, item.name, item.amount, item.duration = "gold", "gold", "Gold", it.data1, nil
    elseif it.type == T.TRADE_ITEM_GOLD_PER_TURN then
        item.group, item.kind, item.name, item.amount = "gold", "gold_per_turn", "Gold per turn", it.data1
    elseif it.type == T.TRADE_ITEM_RESOURCES then
        local r = GameInfo.Resources[it.data1]
        local usage = Game.GetResourceUsageType(it.data1)
        item.group = usage == 1 and "strategic" or (usage == 2 and "luxury" or "other")
        item.kind, item.name, item.amount = "resource", r and L(r.Description) or "Resource", it.data2
    elseif it.type == T.TRADE_ITEM_CITIES then
        local plot = Map.GetPlot(it.data1, it.data2)
        local city = plot and plot:IsRevealed(team, false) and plot:GetPlotCity()
        item.kind, item.name = "city", city and city:GetName() or "City"
    elseif it.type == T.TRADE_ITEM_OPEN_BORDERS then item.kind, item.name = "open_borders", "Open borders"
    elseif it.type == T.TRADE_ITEM_DEFENSIVE_PACT then item.kind, item.name = "defensive_pact", "Defensive pact"
    elseif it.type == T.TRADE_ITEM_RESEARCH_AGREEMENT then item.kind, item.name = "research_agreement", "Research agreement"
    elseif it.type == T.TRADE_ITEM_TRADE_AGREEMENT then item.kind, item.name = "trade_agreement", "Trade agreement"
    elseif it.type == T.TRADE_ITEM_PEACE_TREATY then item.kind, item.name = "peace", "Peace treaty"
    elseif it.type == T.TRADE_ITEM_TRUCE then item.kind, item.name = "peace", "Truce"
    elseif it.type == T.TRADE_ITEM_ALLOW_EMBASSY then item.kind, item.name = "embassy", "Embassy"
    elseif it.type == T.TRADE_ITEM_DECLARATION_OF_FRIENDSHIP then item.kind, item.name = "friendship", "Declaration of friendship"
    elseif it.type == T.TRADE_ITEM_THIRD_PARTY_PEACE then
        item.kind, item.name = "third_party_peace", "Make peace with " .. civ_of_team(it.data1)
    elseif it.type == T.TRADE_ITEM_THIRD_PARTY_WAR then
        item.kind, item.name = "third_party_war", "Declare war on " .. civ_of_team(it.data1)
    elseif it.type == T.TRADE_ITEM_VOTE_COMMITMENT then
        item.kind, item.name, item.amount = "vote", it.flag1 and "Vote commitment (repeal)" or "Vote commitment", it.data3
    else
        -- SIMPLIFIED(SIMP-450): maps, units, permanent alliance, surrender and embargo items (absent from the trade screen) have no model kind and are not listed.
        return nil
    end
    return item
end
local function deals_of(player)
    if not (GcUiGetCurrentDeals and GcUiGetHistoricDeals) then return nil end
    local out = array()
    for _, part in ipairs({{"current", GcUiGetCurrentDeals(player)}, {"completed", GcUiGetHistoricDeals(player)}}) do
        for _, d in ipairs(part[2]) do
            local id = #out + 1
            local partner_id = d.from == player and d.to or d.from
            local partner = person(partner_id)
            local items = array()
            for n, it in ipairs(d.items) do
                local item = deal_item(player, id, n - 1, it)
                if item then items[#items+1] = item end
            end
            out[id] = {id=id, state=part[1], partner_id=partner_id, partner_name=partner.name, partner_civ=partner.civ,
                partner_icon=partner.icon_civ, partner_color_bg="", partner_color_fg="", start_turn=d.start_turn,
                duration=d.duration, end_turn=d.final_turn, items=items}
        end
    end
    return out
end
local function data(player)
    local p, team, tid = Players[player], team_of(player), Players[player]:GetTeam()
    local majors, minors = array(), array()
    local counts = {majors_met=0, city_states_met=0, at_war=0, friends=0, denounced_us=0, allies_cs=0, cs_at_war=0}
    local active = p:IsTurnActive()
    for id=0,GameDefines.MAX_CIV_PLAYERS-1 do
        local q = Players[id]
        if id ~= player and q and q:IsAlive() and met(player,id) then
            local qt, war = q:GetTeam(), team:IsAtWar(q:GetTeam())
            if q:IsMinorCiv() then
                local influence, ally = q:GetMinorCivFriendshipWithMajor(player), q:GetAlly()
                local status = war and "war" or (q:IsAllies(player) and "allied" or (q:IsFriends(player) and "friends" or (influence < 0 and "angry" or "neutral")))
                local trait = GameInfo.MinorCivTraits[q:GetMinorCivTrait()]
                local texture = trait and trait.TraitIcon
                local personality = ({[MinorCivPersonalityTypes.MINOR_CIV_PERSONALITY_FRIENDLY]="Friendly",[MinorCivPersonalityTypes.MINOR_CIV_PERSONALITY_NEUTRAL]="Neutral",[MinorCivPersonalityTypes.MINOR_CIV_PERSONALITY_HOSTILE]="Hostile",[MinorCivPersonalityTypes.MINOR_CIV_PERSONALITY_IRRATIONAL]="Irrational"})[q:GetPersonality()]
                minors[#minors+1] = {id=id, name=L(q:GetCivilizationShortDescriptionKey()), icon=texture and texture ~= "" and "ui:tx:" .. texture or "", color="",
                    at_war=war, influence=influence, status=status, trait=trait and L(trait.Description) or "",
                    personality=personality or "", ally_id=ally >= 0 and (ally == player or met(player,ally)) and ally or nil,
                    ally_name=ally >= 0 and (ally == player or met(player,ally)) and person(ally).name or nil,
                    ally_is_me=ally == player, resources=cs_resources(player,q), tip_status=status .. ": " .. influence}
                counts.city_states_met = counts.city_states_met + 1
                if ally == player then counts.allies_cs = counts.allies_cs + 1 end
                if war then counts.cs_at_war = counts.cs_at_war + 1 end
            else
                local row = person(id)
                local denounced, friend = q:IsDenouncedPlayer(player), p:IsDoF(id)
                row.team, row.team_size, row.is_human = qt, Teams[qt]:GetNumMembers(), q:IsHuman()
                row.can_declare_war = active and not war and team:CanDeclareWar(qt)
                row.status = war and "war" or (denounced and "denounced_us" or "none")
                row.mood, row.war_turns = mood(player,q), war and math.max(0,team:GetNumTurnsAtWar(qt)) or nil
                row.denounce_turns = denounced and remaining(q:GetDenouncedPlayerCounter(player)) or nil
                row.dof_turns = friend and remaining(p:GetDoFCounter(id)) or nil
                row.summary = war and "At war: trade information is hidden." or (denounced and "They denounced you: trade information is hidden." or "")
                row.trade_visible = not war and not denounced
                if row.trade_visible then
                    row.gold, row.gold_per_turn = q:GetGold(), q:CalculateGoldRate()
                    row.embassy = direction(team:HasEmbassyAtTeam(qt), Teams[qt]:HasEmbassyAtTeam(tid))
                    row.borders = direction(Teams[qt]:IsAllowsOpenBordersToTeam(tid), team:IsAllowsOpenBordersToTeam(qt))
                    row.defensive_pact, row.research_agreement = team:IsDefensivePact(qt), team:IsHasResearchAgreement(qt)
                    row.resources = resources(q)
                else row.resources = array() end
                row.action, row.action_enabled = q:IsHuman() and "trade" or "talk", active
                row.action_reason = not active and "Only during your turn." or nil
                row.era = L(GameInfo.Eras[q:GetCurrentEra()].Description)
                row.policies, row.wonders, row.relations = array(), array(), array()
                for b in GameInfo.PolicyBranchTypes() do
                    local n=q:GetNumPoliciesInBranch(b.ID)
                    if n>0 then row.policies[#row.policies+1] = {key=b.Type, name=L(b.Description), count=n, is_ideology=b.PurchaseByLevel == true or b.PurchaseByLevel == 1} end
                end
                -- Only revealed cities contribute wonders; never inspect hidden foreign cities.
                for city in q:Cities() do
                    if city:Plot():IsRevealed(tid,false) then
                        for b in GameInfo.Buildings() do
                            local class = GameInfo.BuildingClasses[b.BuildingClass]
                            if class.MaxGlobalInstances == 1 and city:IsHasBuilding(b.ID) then
                                row.wonders[#row.wonders+1] = {key=b.Type, name=L(b.Description), icon="building:" .. b.Type}
                            end
                        end
                    end
                end
                for other=0,GameDefines.MAX_CIV_PLAYERS-1 do
                    local r=Players[other]
                    if other~=id and r and r:IsAlive() and (other==player or met(player,other)) and Teams[qt]:IsHasMet(r:GetTeam()) then
                        local kind, tone
                        if r:IsMinorCiv() then if r:GetAlly()==id then kind,tone="ally_cs","good" end
                        elseif Teams[qt]:IsAtWar(r:GetTeam()) then kind,tone=other==player and "war_us" or "war","bad"
                        elseif q:IsDenouncedPlayer(other) then kind,tone="denounced","bad"
                        elseif q:IsDoF(other) then kind,tone="friend","good"
                        elseif Teams[qt]:IsDefensivePact(r:GetTeam()) then kind,tone="dp","good" end
                        if kind then row.relations[#row.relations+1]={key=kind .. ":" .. other,kind=kind,tone=tone,
                            text=kind .. ": " .. L(r:GetCivilizationShortDescriptionKey())} end
                    end
                end
                majors[#majors+1] = row
                counts.majors_met = counts.majors_met + 1
                if war then counts.at_war=counts.at_war+1 end
                if friend then counts.friends=counts.friends+1 end
                if denounced then counts.denounced_us=counts.denounced_us+1 end
            end
        end
    end
    table.sort(majors,function(a,b) if a.score ~= b.score then return a.score>b.score end return a.id<b.id end)
    table.sort(minors,function(a,b) if a.influence ~= b.influence then return a.influence>b.influence end return a.id<b.id end)
    local league = Game.GetActiveLeague()
    local deals = deals_of(player)
    return {me=person(player), turn=Game.GetGameTurn(), can_open_diplo=active,
        open_block_reason=not active and "Only during your turn." or nil,
        league={active=league~=nil,in_session=league and league:IsInSession() or false,turns_to_session=league and league:GetTurnsUntilSession() or 0},
        counts=counts,majors=majors,city_states=minors,influence_scale={min=-60,max=180,
            friends=GameDefines.FRIENDSHIP_THRESHOLD_FRIENDS,allies=GameDefines.FRIENDSHIP_THRESHOLD_ALLIES},
        deals=deals or array(),deals_available=deals~=nil,deals_reason=not deals and deals_reason or nil}
end
return {data=data,actions={
    begin_diplo=function(player,args)
        assert(type(args.a)=="number" and args.a==math.floor(args.a) and args.a>=0 and args.a<GameDefines.MAX_MAJOR_CIVS,"A major civilization is required.")
        local p=Players[args.a]
        assert(args.a~=player and p and p:IsAlive() and not p:IsHuman() and met(player,args.a),"This leader is unavailable.")
        assert(Players[player]:IsTurnActive(),"Only during your turn.")
        -- The original's leader list: UI.ChangeStartDiploRepeatCount, then DoBeginDiploWithHuman, whose greeting is the
        -- only thing it changes (the leader screen of this machine). The seat's leader session holds both.
        leader_begin(player,args.a,true)
        return {open={screen="leader",a=args.a}}
    end,
}}
