local gifts = {"MINOR_GOLD_GIFT_SMALL", "MINOR_GOLD_GIFT_MEDIUM", "MINOR_GOLD_GIFT_LARGE"}
local function minor(player, id, dead)
    assert(type(id) == "number" and id == math.floor(id), "A city-state id is required.")
    local m = Players[id]
    assert(m and m:IsMinorCiv() and (dead or m:IsAlive()) and met(player, id), "This city-state is unavailable to your civilization.")
    return m
end
local function color(v)
    return math.floor(v.x * 255 + 0.5) * 16777216 + math.floor(v.y * 255 + 0.5) * 65536 + math.floor(v.z * 255 + 0.5) * 256 + 255
end
local function reference(player, id)
    local p = Players[id]
    local c = GameInfo.Civilizations[p:GetCivilizationType()]
    local leader = GameInfo.Leaders[p:GetLeaderType()]
    local bg, fg = p:GetPlayerColors()
    return {key = "player:" .. id, player_id = id, name = L(leader.Description), is_you = id == player,
        emblem = "civ:" .. c.Type, color_bg = color(bg), color_fg = color(fg)}
end
local function state(player, m)
    local at_war = team_of(player):IsAtWar(m:GetTeam())
    local influence = m:GetMinorCivFriendshipWithMajor(player)
    local kind = "neutral"
    if m:IsAllies(player) then kind = "ally"
    elseif m:IsFriends(player) then kind = "friend"
    elseif m:IsMinorPermanentWar(Players[player]:GetTeam()) then kind = "permanent_war"
    elseif m:IsPeaceBlocked(Players[player]:GetTeam()) then kind = "no_peace"
    elseif at_war then kind = "war"
    elseif m:CanMajorBullyGold(player) then kind = "afraid"
    elseif influence < 0 then kind = "angry" end
    local style = at_war and "war" or kind == "ally" and "ally" or kind == "friend" and "friend" or kind == "afraid" and "bully" or influence < 0 and "negative" or "neutral"
    local ally = GameDefines.FRIENDSHIP_THRESHOLD_ALLIES
    local friend = GameDefines.FRIENDSHIP_THRESHOLD_FRIENDS
    local anchor = m:GetMinorCivFriendshipAnchorWithMajor(player)
    local limit = influence < 0 and math.abs(GameDefines.MINOR_FRIENDSHIP_AT_WAR) or ally
    local fill = math.min(100, math.abs(influence) / math.max(1, limit) * 100)
    local labels = {ally = "Allies", friend = "Friends", permanent_war = "Permanent war", no_peace = "Peace blocked", war = "War", afraid = "Afraid", angry = "Angry", neutral = "Neutral"}
    return {kind = kind, style = style, label = labels[kind],
        influence = influence, per_turn = m:GetFriendshipChangePerTurnTimes100(player) / 100,
        resting_point = anchor, at_rest = influence == anchor, at_war = at_war,
        war_turns = at_war and team_of(player):GetNumTurnsAtWar(m:GetTeam()) or 0,
        war_threshold = GameDefines.MINOR_FRIENDSHIP_AT_WAR, friend_threshold = friend, ally_threshold = ally,
        scale_side = influence < 0 and "negative" or influence > 0 and "positive" or "zero",
        scale_fill_pct = fill, scale_marker_pct = fill, friend_tick_pct = friend / math.max(1, ally) * 100}
end
-- CityStateStatusHelper's ktQuestsDisplayOrder: global quests, then personal support, then other personal quests.
local quest_order = {"CONTEST_CULTURE", "CONTEST_FAITH", "CONTEST_TECHS", "INVEST", "KILL_CAMP", "GIVE_GOLD", "PLEDGE_TO_PROTECT",
    "DENOUNCE_MAJOR", "TRADE_ROUTE", "SPREAD_RELIGION", "BULLY_CITY_STATE", "FIND_NATURAL_WONDER", "FIND_PLAYER", "KILL_CITY_STATE",
    "GREAT_PERSON", "CONSTRUCT_WONDER", "CONNECT_RESOURCE", "ROUTE"}
local function civ_short(id) return L(Players[id]:GetCivilizationShortDescriptionKey()) end
-- One quest as GetActiveQuestToolTip words it: the formal text, contest standings and the turns left.
local function quest_row(player, m, name, quest)
    local d1, d2 = m:GetQuestData1(player, quest), m:GetQuestData2(player, quest)
    local turns = m:GetQuestTurnsRemaining(player, quest, Game.GetGameTurn())
    local row = {icon = "", detail = "", can_locate = false, turns_left = turns >= 0 and turns or nil}
    local key = "TXT_KEY_CITY_STATE_QUEST_" .. name .. "_FORMAL"
    if name == "CONNECT_RESOURCE" then
        local info = GameInfo.Resources[d1]
        row.title, row.icon = L(key, L(info.Description)), "resource:" .. info.Type
    elseif name == "CONSTRUCT_WONDER" then row.title = L(key, L(GameInfo.Buildings[d1].Description))
    elseif name == "GREAT_PERSON" then
        local info = GameInfo.Units[d1]
        row.title, row.icon = L(key, L(info.Description)), "unit:" .. info.Type
    elseif name == "KILL_CITY_STATE" then row.title = L(key, L(Players[d1]:GetNameKey()))
    elseif name == "FIND_PLAYER" or name == "GIVE_GOLD" or name == "PLEDGE_TO_PROTECT" or name == "BULLY_CITY_STATE" or name == "DENOUNCE_MAJOR" then
        row.title = L(key, civ_short(d1))
    elseif name == "SPREAD_RELIGION" then row.title = L(key, Game.GetReligionName(d1))
    elseif name == "CONTEST_CULTURE" or name == "CONTEST_FAITH" or name == "CONTEST_TECHS" then
        local leader, you = m:GetMinorCivContestValueForLeader(quest), m:GetMinorCivContestValueForPlayer(player, quest)
        local lead = m:IsMinorCivContestLeader(player, quest)
        row.contest_you, row.contest_leader, row.contest_you_lead = you, leader, lead
        local base = "TXT_KEY_CITY_STATE_QUEST_" .. name
        row.title = lead and L(base .. "_WINNING_FORMAL", you) or L(base .. "_LOSING_FORMAL", leader, you)
    else
        row.title = L(key)
        if name == "KILL_CAMP" then
            -- The original's quest button centres the map on the camp.
            local plot = Map.GetPlot(d1, d2)
            if plot and plot:IsRevealed(Players[player]:GetTeam(), false) then row.plot, row.can_locate = plot:GetPlotIndex(), true end
        end
    end
    row.tooltip = turns >= 0 and row.title .. " " .. L("TXT_KEY_CITY_STATE_QUEST_TURNS_REMAINING_FORMAL", turns) or row.title
    return row
end
local function data(player, args)
    if args.popup then
        local p=args.popup
        args={a=p.data1,b=args.b,c=p.data2,d=p.data3,popup=p,popup_id=args.popup_id}
    end
    local p = Players[player]
    local id = args.a
    local root = {view = "diplo", player_id = player, player_gold = p:GetGold(), protectors = array(), enemies = array(),
        bonuses = array(), resources = array(), quests = array(), no_quests_text = L("TXT_KEY_CITY_STATE_QUEST_NONE")}
    if id == nil then
        for other = 0, GameDefines.MAX_CIV_PLAYERS - 1 do
            if Players[other] and Players[other]:IsMinorCiv() and Players[other]:IsAlive() and met(player, other) then id = other; break end
        end
        if id == nil then root.reason = "No known city-state is available."; return root end
    end
    local m = minor(player, id, args.b == 2)
    local info = GameInfo.MinorCivilizations[m:GetMinorCivType()]
    local primary, secondary = m:GetPlayerColors()
    local capital = m:GetCapitalCity()
    local traits = {[0] = "cultural", "militaristic", "maritime", "mercantile", "religious"}
    local personalities = {[0] = "friendly", "neutral", "hostile", "irrational"}
    local trait, personality = traits[m:GetMinorCivTrait()], personalities[m:GetPersonality()]
    assert(trait and personality, "The city-state's traits are unavailable.")
    root.minor = {id = id, key = "player:" .. id, team_id = m:GetTeam(), name = L(info.Description), emblem = "civ:" .. info.Type,
        color_bg = color(secondary), color_fg = color(primary), name_color = color(primary), trait = trait,
        trait_name = L("TXT_KEY_CITY_STATE_" .. (trait == "cultural" and "CULTURED" or string.upper(trait)) .. "_ADJECTIVE"),
        trait_tooltip = L("TXT_KEY_CITY_STATE_" .. (trait == "militaristic" and "MILITARISTIC_NO_UU" or trait == "cultural" and "CULTURED" or string.upper(trait)) .. "_TT"),
        personality = personality, personality_name = L("TXT_KEY_CITY_STATE_PERSONALITY_" .. string.upper(personality)),
        personality_tooltip = L("TXT_KEY_CITY_STATE_PERSONALITY_" .. string.upper(personality) .. "_TT"),
        has_capital = capital ~= nil, capital_location = capital and {x = capital:GetX(), y = capital:GetY()} or nil, capital_plot = capital and capital:Plot():GetPlotIndex() or nil}
    if args.b == 1 then
        assert(type(args.c) == "number" and type(args.d) == "number", "The territory popup requires unit and plot arguments from the host queue.")
        local unit = p:GetUnitByID(args.c)
        local plot = args.d and Map.GetPlotByIndex(args.d)
        assert(unit and unit:GetOwner() == player and plot and plot:GetOwner() == id and visible(plot, p:GetTeam()), "The territory-entry question is stale.")
        root.view = "enter_land"
        root.enter_land = {owner_name = root.minor.name, owner_kind = "city_state", owner_team_id = m:GetTeam(),
            subtitle = root.minor.name, unit_id = unit:GetID(), unit_name = L(GameInfo.Units[unit:GetUnitType()].Description),
            plot = args.d, queue = false, text = "Enter this city-state's territory?"}
        root.reason = "Continuing movement requires the territory-entry handler in the game host."
        return root
    elseif args.b == 2 then
        assert(type(args.c) == "number", "The liberation popup requires a city argument from the host queue.")
        local city = p:GetCityByID(args.c)
        assert(city and city:GetOwner() == player and city:GetOriginalOwner() == id and city:IsOccupied(), "This captured city is unavailable for liberation.")
        root.view = "liberate"
        root.liberate = {city_id = city:GetID(), city_key = key_city(player, city:GetID()), city_name = city:GetName(),
            captured_from = "", subtitle = root.minor.name, text = "Choose the future of the captured city.", preview = array(),
            can_puppet = not city:IsPuppet(), annex_text = "Annex city"}
        return root
    end
    local status = state(player, m)
    root.status = status
    local at_war = status.at_war
    local pledge = m:IsProtectedByMajor(player)
    local pledge_enabled = not at_war and (pledge and m:CanMajorWithdrawProtection(player) or not pledge and m:CanMajorStartProtection(player))
    local peace = at_war and not m:IsPeaceBlocked(Players[player]:GetTeam()) and not m:IsMinorPermanentWar(Players[player]:GetTeam()) and team_of(player):CanChangeWarPeace(m:GetTeam())
    local can_war = not at_war and team_of(player):CanDeclareWar(m:GetTeam())
    local buyout = not at_war and m:CanMajorBuyout(player)
    root.actions = {
        peace = {visible = at_war, enabled = peace, turns = team_of(player):GetNumTurnsLockedIntoWar(m:GetTeam()), reason = peace and "" or "Peace is currently blocked by the game rules."},
        war = {visible = not at_war, enabled = can_war, reason = can_war and "" or "War is currently blocked by the game rules."},
        gifts = {visible = not at_war, enabled = not at_war}, tribute = {visible = not at_war, enabled = not at_war},
        pledge = {visible = not at_war, state = pledge and "pledged" or "can_pledge", enabled = pledge_enabled, turns = 0, reason = pledge_enabled and "" or "Protection requirements are not met."},
        unit_spawning = {visible = not at_war and trait == "militaristic" and (m:IsFriends(player) or m:IsAllies(player)), on = not m:IsMinorCivUnitSpawningDisabled(player), subtitle = "Military unit gifts"},
        buyout = {visible = buyout, enabled = buyout, cost = m:GetBuyoutCost(player), reason = buyout and "" or "Buyout requirements are not met."}}
    local gold = array()
    local ally_id = m:GetAlly()
    local refuses_bribes = false
    if ally_id >= 0 and ally_id ~= player and Players[ally_id].GetNumTurnsBeforeMinorAlliesRefuseBribes then
        local duration = Players[ally_id]:GetNumTurnsBeforeMinorAlliesRefuseBribes()
        refuses_bribes = duration > 0 and m:GetAlliedTurns() >= duration
    end
    for index, key in ipairs(gifts) do
        local amount = assert(GameDefines[key], "Gold gift amount is unavailable.")
        local enabled = not at_war and not refuses_bribes and p:GetGold() >= amount
        gold[#gold + 1] = {key = key, index = index - 1, amount = amount, influence = m:GetFriendshipFromGoldGift(player, amount),
            enabled = enabled, reason = enabled and "" or at_war and "Gifts are unavailable during war." or refuses_bribes and "This city-state refuses gold gifts while allied to its current ally." or "Insufficient gold."}
    end
    -- The unit and the improvement are chosen on the map (the client's targeting mode, the `targets` screen), as the
    -- original's INTERFACEMODE_GIFT_UNIT and _GIFT_TILE_IMPROVEMENT; CityStateDiploPopup's tests enable them.
    -- SIMPLIFIED(SIMP-382): a unit gift is sent at the click on its plot, without the original's confirmation popup
    -- (BUTTONPOPUP_GIFT_CONFIRM).
    local unit_gift = not at_war and m:GetIncomingUnitCountdown(player) < 0
    local improvement_gift = not at_war and m:CanMajorGiftTileImprovement(player)
    root.gifts = {gold = gold, gold_hint = tostring(status.friend_threshold) .. " / " .. tostring(status.ally_threshold),
        unit = {enabled = unit_gift, reason = unit_gift and "" or at_war and "Gifts are unavailable during war." or "A gifted unit is still on its way."},
        improvement = {enabled = improvement_gift, cost = m:GetGiftTileImprovementCost(player),
            reason = improvement_gift and "" or "Only an ally with the gold can improve a tile of this city-state."}}
    local bully_gold, bully_unit = not at_war and m:CanMajorBullyGold(player), not at_war and m:CanMajorBullyUnit(player)
    -- The defines are influence times 100 (CityStateDiploPopup divides them the same way).
    local loss = math.floor(math.abs(GameDefines.MINOR_FRIENDSHIP_DROP_BULLY_GOLD_SUCCESS) / 100)
    -- The original's tribute tooltips are the core's pressure breakdowns (each factor with its score, then the total).
    local gold_details, unit_details = m:GetMajorBullyGoldDetails(player), m:GetMajorBullyUnitDetails(player)
    local unit_loss = math.floor(math.abs(GameDefines.MINOR_FRIENDSHIP_DROP_BULLY_WORKER_SUCCESS or 0) / 100)
    root.can_bully, root.bully_text = bully_gold, gold_details
    root.tribute = {tooltip = gold_details,
        gold = {enabled = bully_gold, amount = m:GetMinorCivBullyGoldAmount(player), influence_loss = loss, tooltip = gold_details, reason = bully_gold and "" or "Insufficient military pressure or tribute is blocked."},
        unit = {enabled = bully_unit, unit_name = L("TXT_KEY_UNIT_WORKER"), icon = "unit:UNIT_WORKER", influence_loss = unit_loss, tooltip = unit_details, reason = bully_unit and "" or "Insufficient military pressure or tribute is blocked."},
        confirm_influence_after = status.influence - loss, confirm_status_after = ""}
    if GcUiGetBullyScores and m:IsAlive() then
        -- Tribute is possible at a score of zero and above.
        local gold_score, unit_score = GcUiGetBullyScores(player, id)
        root.tribute.pressure = gold_score
        root.tribute.pressure_gold_needed = math.max(0, -gold_score)
        root.tribute.pressure_unit_needed = math.max(0, -unit_score)
    end
    local ally = m:GetAlly()
    root.ally = {kind = "none", name = L("TXT_KEY_CITY_STATE_NOBODY"), tooltip = "", refuses_bribes = refuses_bribes}
    if ally >= 0 then
        root.ally.kind = ally == player and "you" or met(player, ally) and "civ" or "unknown"
        if ally == player or met(player, ally) then
            local ref = reference(player, ally)
            root.ally.name, root.ally.player_id, root.ally.emblem = ref.name, ally, ref.emblem
            root.ally.color_bg, root.ally.color_fg = ref.color_bg, ref.color_fg
            root.ally.influence = m:GetMinorCivFriendshipWithMajor(ally)
            root.ally.influence_to_overtake = ally ~= player and math.max(0, root.ally.influence - status.influence + 1) or 0
            root.ally.ally_turns = m:GetAlliedTurns()
            root.ally.tooltip = ref.name .. " (" .. root.ally.influence .. " influence)"
        else root.ally.name = L("TXT_KEY_UNMET_PLAYER") end
    end
    for other = 0, GameDefines.MAX_MAJOR_CIVS - 1 do
        if Players[other]:IsAlive() and (other == player or met(player, other)) then
            if m:IsProtectedByMajor(other) then root.protectors[#root.protectors + 1] = reference(player, other) end
            if Teams[m:GetTeam()]:IsAtWar(Players[other]:GetTeam()) then root.enemies[#root.enemies + 1] = reference(player, other) end
        end
    end
    for _, query in ipairs({{"culture", "GetMinorCivCurrentCultureBonus"}, {"faith", "GetMinorCivCurrentFaithBonus"}, {"happiness", "GetMinorCivCurrentHappinessBonus"}, {"capital_food", "GetCurrentCapitalFoodBonus"}, {"city_food", "GetCurrentOtherCityFoodBonus"}}) do
        local amount = m[query[2]](m, player)
        if amount ~= 0 then root.bonuses[#root.bonuses + 1] = {key = query[1], kind = query[1], text = tostring(amount), short = tostring(amount), icon = ""} end
    end
    -- The original's quest list: CityStateStatusHelper's display order and formal texts.
    local types = MinorCivQuestTypes or {}
    for _, name in ipairs(quest_order) do
        local quest = types["MINOR_CIV_QUEST_" .. name]
        if type(quest) == "number" and m:IsMinorCivDisplayedQuestForPlayer(player, quest) then
            local row = quest_row(player, m, name, quest)
            row.key, row.type = "quest:" .. quest, string.lower(name)
            root.quests[#root.quests + 1] = row
        end
    end
    root.threat_barbarians = m:IsThreateningBarbariansEventActiveForPlayer(player)
    root.threat_barbarians_text = L("TXT_KEY_CITY_STATE_QUEST_INVADING_BARBS_FORMAL")
    root.threat_proxy_war = m:IsProxyWarActiveForMajor(player)
    root.threat_proxy_war_text = L("TXT_KEY_CITY_STATE_QUEST_GIFT_UNIT_FORMAL")
    if #root.quests > 0 or root.threat_barbarians or root.threat_proxy_war then root.no_quests_text = "" end
    if capital and Map and Map.GetNumPlots then
        local counts = {}
        for index = 0, Map.GetNumPlots() - 1 do
            local plot = Map.GetPlotByIndex(index)
            local distance = Map.PlotDistance(capital:GetX(), capital:GetY(), plot:GetX(), plot:GetY())
            if visible(plot, p:GetTeam()) and ((plot:GetOwner() == id and distance <= 5) or (plot:GetOwner() == -1 and distance <= 2)) then
                local resource = plot:GetResourceType(p:GetTeam())
                if resource >= 0 then counts[resource] = (counts[resource] or 0) + plot:GetNumResource() end
            end
        end
        for resource, amount in pairs(counts) do
            local info = GameInfo.Resources[resource]
            if info.ResourceClassType == "RESOURCECLASS_LUXURY" or info.ResourceClassType == "RESOURCECLASS_RUSH" or info.ResourceClassType == "RESOURCECLASS_MODERN" then
                root.resources[#root.resources + 1] = {key = info.Type, icon = "resource:" .. info.Type, name = L(info.Description), count = amount,
                    kind = info.ResourceClassType == "RESOURCECLASS_LUXURY" and "luxury" or "strategic", exported = m:GetResourceExport(resource) > 0}
            end
        end
        table.sort(root.resources, function(a, b) return a.key < b.key end)
    end
    return root
end

local function action_model(player, args)
    minor(player, args.a)
    return data(player, {a = args.a})
end
return {data = data, actions = {
    gift_gold = function(player, args, net)
        assert(args.b == 0 or args.b == 1 or args.b == 2, "Choose a gold gift size.")
        local gift = action_model(player, args).gifts.gold[args.b + 1]
        assert(gift.enabled, gift.reason)
        net.send("sendMinorGiftGold", args.a, gift.amount)
    end,
    declare_war = function(player, args, net)
        local model = action_model(player, args)
        assert(model.actions.war.enabled, model.actions.war.reason)
        net.send("sendChangeWar", model.minor.team_id, 1)
    end,
    peace = function(player, args, net)
        local model = action_model(player, args)
        assert(model.actions.peace.enabled, model.actions.peace.reason)
        net.send("sendChangeWar", model.minor.team_id, 0)
    end,
    pledge = function(player, args, net)
        assert(args.b == 0 or args.b == 1, "Protection flag must be zero or one.")
        local model = action_model(player, args)
        assert(model.actions.pledge.enabled, model.actions.pledge.reason)
        assert((args.b == 1) == (model.actions.pledge.state == "can_pledge"), "Protection state has changed.")
        net.send("sendMinorPledgeProtection", args.a, args.b, 0)
    end,
    unit_spawning = function(player, args, net)
        assert(args.b == 0 or args.b == 1, "Unit gift flag must be zero or one.")
        assert(action_model(player, args).actions.unit_spawning.visible, "Military unit gifts are unavailable.")
        net.send("SendMinorNoUnitSpawning", args.a, args.b)
    end,
    bully_gold = function(player, args, net)
        local choice = action_model(player, args).tribute.gold
        assert(choice.enabled, choice.reason)
        net.send("sendMinorBullyGold", args.a, choice.amount)
    end,
    bully_unit = function(player, args, net)
        local choice = action_model(player, args).tribute.unit
        assert(choice.enabled, choice.reason)
        net.send("sendMinorBullyUnit", args.a, GameInfoTypes.UNIT_WORKER)
    end,
    buyout = function(player, args, net)
        local choice = action_model(player, args).actions.buyout
        assert(choice.enabled, choice.reason)
        net.send("sendMinorBuyout", args.a)
    end,
    cancel = function(player,args,net)
        assert(args.popup and args.popup.kind == "BUTTONPOPUP_MINOR_CIV_ENTER_TERRITORY", "A territory question is required.")
        net.popup_done(args.popup_id)
        return {close=true}
    end,
    enter_land = function()
        -- SIMPLIFIED(SIMP-384): neither the core nor the original interface raises this question, and its game message has no handler in the core, so "enter" fails closed.
        error("Continuing movement requires the territory-entry handler in the game host.")
    end,
    liberate = function(player, args, net)
        assert(args.popup and args.popup.kind == "BUTTONPOPUP_LIBERATE_MINOR", "A pending liberation question is required.")
        assert(args.a == args.popup.data1 and args.b == args.popup.data2, "This liberation question is stale.")
        data(player, {a = args.a, b = 2, popup = args.popup})
        net.send("SendLiberateMinor", args.a, args.b)
        net.popup_done(args.popup_id)
        return {close=true}
    end,
    puppet = function(player, args, net)
        assert(args.popup and args.popup.kind == "BUTTONPOPUP_LIBERATE_MINOR" and args.popup.data2 == args.a, "A matching liberation question is required.")
        local city = Players[player]:GetCityByID(args.a)
        assert(city and city:GetOwner() == player and city:IsOccupied() and not city:IsPuppet(), "This captured city cannot become a puppet.")
        local original = Players[city:GetOriginalOwner()]
        assert(original and original:IsMinorCiv(), "This city was not founded by a city-state.")
        net.send("sendDoTask", args.a, TaskTypes.TASK_CREATE_PUPPET, -1, -1, 0, 0, 0, 0)
        net.popup_done(args.popup_id)
        return {close=true}
    end,
}}
