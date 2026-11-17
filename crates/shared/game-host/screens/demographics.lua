local popups = {}
local metric_units = { {"population", "people"}, {"food", "million_bushels"}, {"production", "million_tons"}, {"gold", "million_gold"}, {"land", "square_km"}, {"army", "soldiers"}, {"approval", "percent"}, {"literacy", "percent"} }
local presenters = {"herodotus", "pliny", "livy", "tacitus", "gibbon", "bede", "toynbee"}
local function color(key)
    local c = GameInfo.Colors[key]
    return math.floor(c.Red * 255) * 16777216 + math.floor(c.Green * 255) * 65536 + math.floor(c.Blue * 255) * 256 + 255
end
local function identity(player, other)
    local known = player == other or met(player, other)
    local row = { player = other, is_you = player == other, met = known, icon = "", color_bg = 0x26221dff, color_fg = 0xb2a794ff }
    if known then
        local p = Players[other]
        local civ = GameInfo.Civilizations[p:GetCivilizationType()]
        local colors = GameInfo.PlayerColors[civ.DefaultPlayerColor]
        row.icon, row.color_bg, row.color_fg = "civ:" .. civ.Type, color(colors.PrimaryColor), color(colors.SecondaryColor)
        row.name = L(civ.ShortDescription)
    end
    return row
end
local function values(player)
    local p, team = Players[player], team_of(player)
    local food, production, surplus, tourism, cities = 0, 0, 0, array(), 0
    for city in p:Cities() do
        food, production, surplus, cities = food + city:GetYieldRate(YieldTypes.YIELD_FOOD), production + city:GetYieldRate(YieldTypes.YIELD_PRODUCTION), surplus + city:FoodDifference(), cities + 1
        local value = city:GetBaseTourism()
        if value > 0 then tourism[#tourism + 1] = { city = city, value = value } end
    end
    local techs, total = 0, 0
    for tech in GameInfo.Technologies() do total = total + 1; if team:IsHasTech(tech.ID) then techs = techs + 1 end end
    local literacy = team:IsHasTech(GameInfoTypes.TECH_WRITING) and total > 0 and 100 * techs / total or 0
    local excess = p:GetExcessHappiness()
    return {
        population = p:GetRealPopulation(), food = food, production = production, gold = p:CalculateGrossGold(), land = p:GetNumPlots() * 10000,
        army = math.floor(math.sqrt(math.max(0, p:GetMilitaryMight())) * 2000 + 0.5), approval = math.max(0, math.min(100, 60 + 3 * excess)), literacy = math.floor(literacy + 0.5),
        average_food = cities > 0 and surplus / cities or 0, average_production = cities > 0 and production / cities or 0,
        stock_gold = p:GetGold(), science = techs, culture = p:GetNumPolicies(), happiness = excess, wonders = p:GetNumWorldWonders(), military_might = p:GetMilitaryMight(), tourism = tourism,
    }
end
local function winning(player, live, all, token)
    local industrial, enough_policies, wonders, influence, city_tourism = false, true, 0, false, false
    for _, id in ipairs(live) do
        local p, v = Players[id], all[id]
        industrial = industrial or p:GetCurrentEra() >= GameInfoTypes.ERA_INDUSTRIAL
        enough_policies = enough_policies and v.culture >= 2
        wonders = wonders + v.wonders
        city_tourism = city_tourism or #v.tourism > 0
        v.influence = 0
        for _, other in ipairs(live) do
            local generated = Players[other]:GetJONSCultureEverGenerated()
            if other ~= id and generated > 0 and p:GetInfluenceOn(other) >= generated then v.influence = v.influence + 1 end
        end
        influence = influence or v.influence > 0
    end
    local allowed = {"gold", "military_might"}
    if not industrial then
        allowed[#allowed + 1], allowed[#allowed + 2] = "food", "production"
        if enough_policies then allowed[#allowed + 1] = "culture" end
    end
    if not Game.IsOption(GameOptionTypes.GAMEOPTION_NO_HAPPINESS) then allowed[#allowed + 1] = "happiness" end
    if not Game.IsOption(GameOptionTypes.GAMEOPTION_NO_SCIENCE) then allowed[#allowed + 1] = "science" end
    if wonders >= 2 then allowed[#allowed + 1] = "wonders" end
    if influence then allowed[#allowed + 1] = "influence" end
    if city_tourism then allowed[#allowed + 1] = "city_tourism" end
    local state = popups[player]
    if not state or state.token ~= token then
        local choices = {}
        for _, key in ipairs(allowed) do if not state or key ~= state.metric then choices[#choices + 1] = key end end
        state = { token = token, metric = choices[math.random(#choices)], presenter = presenters[math.random(#presenters)] }
        popups[player] = state
    end
    local rows = array()
    local function add(id, value, city)
        local row = identity(player, id)
        row.row_key = id .. ":" .. (city and city:GetID() or -1)
        row.value = value
        row.name = nil
        if row.met then
            local p = Players[id]
            local leader = GameInfo.Leaders[p:GetLeaderType()]
            row.leader_name, row.civ_name, row.leader_icon = L(leader.Description), L(GameInfo.Civilizations[p:GetCivilizationType()].ShortDescription), "leader:" .. leader.Type
            if city then row.city = city:GetName() end
        end
        rows[#rows + 1] = row
    end
    for _, id in ipairs(live) do
        local v = all[id]
        if state.metric == "city_tourism" then for _, t in ipairs(v.tourism) do add(id, t.value, t.city) end
        else
            local key = state.metric == "food" and "average_food" or (state.metric == "production" and "average_production" or (state.metric == "gold" and "stock_gold" or state.metric))
            add(id, v[key])
        end
    end
    table.sort(rows, function(a,b) if a.value ~= b.value then return a.value > b.value end return a.row_key < b.row_key end)
    while #rows > 10 and state.metric == "city_tourism" do table.remove(rows) end
    for i, row in ipairs(rows) do row.rank = i end
    return { presenter_key = state.presenter, ranking_key = state.metric, rows = rows }
end
return {
    data = function(player, args)
        local option = GameInfoTypes.GAMEOPTION_AI_GIMP_NO_DEMOGRAPHICS
        local humans = Game.IsNetworkMultiPlayer() and option ~= nil and Game.IsOption(option)
        local pool, live, all = {}, {}, {}
        for id = 0, GameDefines.MAX_MAJOR_CIVS - 1 do
            local p = Players[id]
            if p and not p:IsMinorCiv() and p:IsEverAlive() then
                all[id] = values(id)
                if p:IsAlive() then live[#live + 1] = id end
                if (humans and p:IsHuman()) or (not humans and p:IsAlive()) then pool[#pool + 1] = id end
            end
        end
        if not all[player] then all[player] = values(player) end
        local rows = array()
        for _, spec in ipairs(metric_units) do
            local key, total, better, ties, best, worst = spec[1], 0, 0, 0, nil, nil
            local mine = all[player][key]
            for _, id in ipairs(pool) do
                local value = all[id][key]
                total = total + value
                if value > mine then better = better + 1 end
                if value == mine then ties = ties + 1 end
                if not best or value > all[best][key] then best = id end
                if not worst or value < all[worst][key] then worst = id end
            end
            local b, w = identity(player, best or player), identity(player, worst or player)
            b.value, w.value = all[best or player][key], all[worst or player][key]
            rows[#rows + 1] = { key = key, unit_key = spec[2], rank = better + 1, rank_shared = ties > 1, value = mine, average = #pool > 0 and math.floor(total / #pool + 0.5) or 0, best = b, worst = w }
        end
        local who = identity(player, player)
        who.civ_name, who.name = who.name, L(GameInfo.Leaders[Players[player]:GetLeaderType()].Description)
        who.is_you, who.met = nil, nil
        local root = { player = who, participants = #pool, humans_only = humans, rows = rows }
        -- SIMPLIFIED(SIMP-377): The popup is queryable with a=1; the periodic host opener is not available to screen scripts.
        if args.a == 1 then root.whos_winning = winning(player, live, all, args.b or Game.GetGameTurn()) end
        return root
    end,
    actions = {
        mark_open = function(player, args, net)
            assert(Players[player] and Players[player]:IsHuman(), "only a human seat can mark this window open")
            net.send("sendGiftUnit", -1, -8)
        end,
    },
}
