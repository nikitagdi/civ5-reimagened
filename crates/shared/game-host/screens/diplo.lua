-- Seat-scoped diplomacy. Display names deliberately use leaders/civilizations, never account names.
local ai_reason = "AI conversations are unavailable: the server has no seat-scoped leader session."
local deals_reason = "Deal history is unavailable: the server has no read-only seat-scoped deal iterator."
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
                local minor = GameInfo.MinorCivilizations[q:GetMinorCivType()]
                local trait = GameInfo.MinorCivTraits[q:GetMinorCivTrait()]
                local personality = ({[MinorCivPersonalityTypes.MINOR_CIV_PERSONALITY_FRIENDLY]="Friendly",[MinorCivPersonalityTypes.MINOR_CIV_PERSONALITY_NEUTRAL]="Neutral",[MinorCivPersonalityTypes.MINOR_CIV_PERSONALITY_HOSTILE]="Hostile",[MinorCivPersonalityTypes.MINOR_CIV_PERSONALITY_IRRATIONAL]="Irrational"})[q:GetPersonality()]
                minors[#minors+1] = {id=id, name=L(q:GetCivilizationShortDescriptionKey()), icon=minor and "civ:" .. minor.Type or "", color="",
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
                row.action, row.action_enabled = q:IsHuman() and "trade" or "talk", q:IsHuman() and active
                row.action_reason = not active and "Only during your turn." or (not q:IsHuman() and ai_reason or nil)
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
    -- SIMPLIFIED(SIMP-450): client scratch-deal loaders are not used as server read APIs; history is explicitly unavailable.
    return {me=person(player), turn=Game.GetGameTurn(), can_open_diplo=active,
        open_block_reason=not active and "Only during your turn." or nil,
        league={active=league~=nil,in_session=league and league:IsInSession() or false,turns_to_session=league and league:GetTurnsUntilSession() or 0},
        counts=counts,majors=majors,city_states=minors,influence_scale={min=-60,max=180,
            friends=GameDefines.FRIENDSHIP_THRESHOLD_FRIENDS,allies=GameDefines.FRIENDSHIP_THRESHOLD_ALLIES},
        deals=array(),deals_available=false,deals_reason=deals_reason}
end
return {data=data,actions={
    begin_diplo=function(player,args)
        assert(type(args.a)=="number" and args.a==math.floor(args.a) and args.a>=0 and args.a<GameDefines.MAX_MAJOR_CIVS,"A major civilization is required.")
        local p=Players[args.a]
        assert(args.a~=player and p and p:IsAlive() and not p:IsHuman() and met(player,args.a),"This leader is unavailable.")
        assert(Players[player]:IsTurnActive(),"Only during your turn.")
        -- SIMPLIFIED(SIMP-451): beginning a conversation needs a seat-scoped server operation; direct AI mutation is forbidden.
        error(ai_reason)
    end,
}}
