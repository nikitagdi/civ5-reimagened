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
local function data(player, args)
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
        -- SIMPLIFIED(SIMP-388): the host window state currently forwards a/b only. Territory and liberation questions require the popup queue to supply their additional unit, plot or city arguments.
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
    -- SIMPLIFIED(SIMP-382): unit and improvement map selection need client map-mode intents. Those entries expose explicit unavailable reasons.
    root.gifts = {gold = gold, gold_hint = tostring(status.friend_threshold) .. " / " .. tostring(status.ally_threshold),
        unit = {enabled = false, reason = "Choose-unit map mode is unavailable in this skin."},
        improvement = {enabled = false, cost = m:GetGiftTileImprovementCost(player), reason = "Choose-improvement map mode is unavailable in this skin."}}
    local bully_gold, bully_unit = not at_war and m:CanMajorBullyGold(player), not at_war and m:CanMajorBullyUnit(player)
    local loss = math.abs(GameDefines.MINOR_FRIENDSHIP_DROP_BULLY_GOLD_SUCCESS)
    root.can_bully, root.bully_text = bully_gold, m:GetMajorBullyGoldDetails(player)
    root.tribute = {tooltip = m:GetMajorBullyGoldDetails(player),
        gold = {enabled = bully_gold, amount = m:GetMinorCivBullyGoldAmount(player), influence_loss = loss, reason = bully_gold and "" or "Insufficient military pressure or tribute is blocked."},
        unit = {enabled = bully_unit, unit_name = L("TXT_KEY_UNIT_WORKER"), icon = "unit:UNIT_WORKER", reason = bully_unit and "" or "Insufficient military pressure or tribute is blocked."},
        confirm_influence_after = status.influence - loss, confirm_status_after = ""}
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
    for _, quest in pairs(MinorCivQuestTypes or {}) do
        if type(quest) == "number" and quest >= 0 and m:IsMinorCivActiveQuestForPlayer(player, quest) then
            local turns = m:GetQuestTurnsRemaining(player, quest, Game.GetGameTurn())
            root.quests[#root.quests + 1] = {key = "quest:" .. quest, type = tostring(quest), icon = "", title = "City-state quest",
                detail = tostring(m:GetQuestData1(player, quest)), tooltip = tostring(m:GetQuestData2(player, quest)), turns_left = turns >= 0 and turns or nil, can_locate = false}
        end
    end
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
    -- SIMPLIFIED(SIMP-383): numeric tribute pressure and detailed quest text await seat-safe server query helpers. Native action availability, amounts and visible capital-radius resources are queried directly.
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
    enter_land = function()
        -- SIMPLIFIED(SIMP-384): the whitelisted territory-entry message has no native dispatch handler, so movement fails closed.
        error("Continuing movement requires the territory-entry handler in the game host.")
    end,
    liberate = function(player, args, net)
        data(player, {a = args.a, b = 2, c = args.b})
        net.send("SendLiberateMinor", args.a, args.b)
    end,
    puppet = function(player, args, net)
        local city = Players[player]:GetCityByID(args.a)
        assert(city and city:GetOwner() == player and city:IsOccupied() and not city:IsPuppet(), "This captured city cannot become a puppet.")
        local original = Players[city:GetOriginalOwner()]
        assert(original and original:IsMinorCiv(), "This city was not founded by a city-state.")
        net.send("sendDoTask", args.a, TaskTypes.TASK_CREATE_PUPPET, -1, -1, 0, 0, 0, 0)
    end,
}}
