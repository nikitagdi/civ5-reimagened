-- Arguments are evaluated against the complete seat-local popup payload.
local function d1(p) return p.data1 end
local function d2(p) return p.data2 end
local function d3(p) return p.data3 end
local function id(p) return p.id end
local function zero() return 0 end
local function one() return 1 end
local function two() return 2 end
-- `lapsed(player, popup)` names why a queued decision can no longer be answered (nil: still pending). The worker asks
-- it of the seat's queue head and drops a lapsed decision, which only its own screen action could complete otherwise.
-- Without the core's Players table (the stub, fixtures) nothing lapses.
local function seat(player)
    local players = rawget(_G, "Players")
    return players and players[player]
end
local function city_gone(field)
    return function(player, p)
        local s = seat(player)
        if s and not s:GetCityByID(p[field]) then return "The city is no longer yours" end
    end
end
local function unit_gone(field)
    return function(player, p)
        local s = seat(player)
        local u = s and s:GetUnitByID(p[field])
        if s and (not u or u:IsDead() or u:IsDelayedDeath()) then return "The unit is gone" end
    end
end
local function production_chosen(player, p)
    local s = seat(player)
    if not s then return nil end
    local c = s:GetCityByID(p.data1)
    if not c then return "The city is no longer yours" end
    if c:GetOrderQueueLength() > 0 then return "The city already has production" end
end
local function research_chosen(player, p)
    local s = seat(player)
    if not s then return nil end
    if p.data2 > 0 then
        if s:GetNumFreeTechs() <= 0 then return "The free technology was taken" end
    elseif s:GetCurrentResearch() >= 0 then
        return "Research was chosen"
    end
end
local function ideology_chosen(player)
    local s = seat(player)
    if s and s:GetLateGamePolicyTree() >= 0 then return "An ideology was adopted" end
end
return {
    BUTTONPOPUP_TECH_AWARD = {screen = "tech_award", a = d3, decision = false},
    BUTTONPOPUP_CHOOSETECH = {screen = "techs", a = function(p) return p.data2 > 0 and 2 or 1 end, decision = true, lapsed = research_chosen},
    BUTTONPOPUP_TECH_TREE = {screen = "techs", decision = false},
    BUTTONPOPUP_CHOOSE_TECH_TO_STEAL = {screen = "techs", a = 3, b = d2, decision = true},
    BUTTONPOPUP_CITY_CAPTURED = {screen = "city_captured", a = d1, b = zero, decision = true, lapsed = city_gone("data1")},
    BUTTONPOPUP_ANNEX_CITY = {screen = "city_captured", a = d1, b = one, decision = false},
    BUTTONPOPUP_CHOOSE_GOODY_HUT_REWARD = {screen = "goody_choice", a = id, b = d2, decision = true},
    BUTTONPOPUP_FOUND_PANTHEON = {screen = "pantheon", a = function(p) return p.data2 == 1 and 0 or 1 end, decision = true},
    BUTTONPOPUP_FOUND_RELIGION = {screen = "religion", a = function(p)
        local plot = Map and Map.GetPlot(p.data1, p.data2)
        local city = plot and plot:GetPlotCity()
        return city and city:GetID() or nil
    end, b = function(p) return p.option1 and 0 or 1 end, decision = true},
    BUTTONPOPUP_CHOOSE_FREE_GREAT_PERSON = {screen = "free_great_person", a = zero, decision = true},
    BUTTONPOPUP_CHOOSE_FAITH_GREAT_PERSON = {screen = "free_great_person", a = one, decision = true},
    BUTTONPOPUP_CHOOSE_MAYA_BONUS = {screen = "free_great_person", a = two, decision = true},
    BUTTONPOPUP_CHOOSE_ARCHAEOLOGY = {screen = "archaeology", decision = true},
    BUTTONPOPUP_LEAGUE_OVERVIEW = {screen = "congress", a = zero, decision = true},
    BUTTONPOPUP_LEAGUE_SPLASH = {screen = "congress", a = one, b = d2, decision = false},
    BUTTONPOPUP_LEAGUE_PROJECT_COMPLETED = {screen = "congress", a = two, b = d2, decision = false},
    BUTTONPOPUP_DIPLO_VOTE = {screen = "mp_vote", a = two, decision = true},
    BUTTONPOPUP_VOTE_RESULTS = {screen = "mp_vote", a = zero, decision = false},
    BUTTONPOPUP_CHOOSEPRODUCTION = {screen = "city", a = d1, b = two, decision = true, lapsed = production_chosen},
    BUTTONPOPUP_CHOOSECITYPURCHASE = {screen = "city", a = d1, decision = false},
    BUTTONPOPUP_CITY_PLOT_MANAGEMENT = {screen = "city", a = d1, decision = false},
    BUTTONPOPUP_RENAME_CITY = {screen = "city_rename", a = d1, decision = true, lapsed = city_gone("data1")},
    BUTTONPOPUP_NOTIFICATION_LOG = {screen = "notification_log", decision = false},
    BUTTONPOPUP_ADVISOR_INFO = {screen = "advisor", a = d1, decision = false},
    BUTTONPOPUP_ADVISOR_COUNSEL = {screen = "advisor", a = d1, decision = false},
    BUTTONPOPUP_WHOS_WINNING = {screen = "demographics", a = one, decision = false},
    BUTTONPOPUP_MAIN_MENU = {screen = "game_menu", decision = false},
    BUTTONPOPUP_CONFIRM_MENU = {screen = "confirm", a = id, decision = true},
    BUTTONPOPUP_CONFIRM_CITY_TASK = {screen = "confirm", a = id, decision = true},
    BUTTONPOPUP_CONFIRM_POLICY_BRANCH_SWITCH = {screen = "policies", a = d1, decision = true},
    BUTTONPOPUP_DEAL_CANCELED = {screen = "message", a = id, decision = false},
    BUTTONPOPUP_EXTENDED_GAME = {screen = "end_game", decision = false},
    BUTTONPOPUP_MINOR_GOLD = {screen = "city_state", a = d1, b = zero, decision = false},
    BUTTONPOPUP_MINOR_CIV_QUEST = {screen = "city_state", a = d1, b = zero, decision = false},
    BUTTONPOPUP_MINOR_CIV_QUEST_COMPLETED = {screen = "city_state", a = d1, b = zero, decision = false},
    BUTTONPOPUP_MINOR_CIV_INTRUSION = {screen = "city_state", a = d1, b = zero, decision = false},
    BUTTONPOPUP_MINOR_CIV_GREETING_AND_WAR = {screen = "city_state", a = d1, b = zero, decision = false},
    BUTTONPOPUP_DIPLOMACY = {screen = "leader", a = d1, decision = true},
    BUTTONPOPUP_DECLAREWAR_PLUNDER_TRADE_ROUTE = {screen = "declare_war", a = d1, b = d2, decision = true},
    BUTTONPOPUP_CULTURE_OVERVIEW = {screen = "culture", a = d1, decision = false},
    BUTTONPOPUP_TRADE_ROUTE_OVERVIEW = {screen = "trade_routes", a = zero, decision = false},
    BUTTONPOPUP_CHOOSE_INTERNATIONAL_TRADE_ROUTE = {screen = "trade_routes", a = zero, b = d1, decision = false},
    BUTTONPOPUP_CHOOSE_TRADE_UNIT_NEW_HOME = {screen = "trade_routes", a = 3, b = d1, decision = false},
    BUTTONPOPUP_CHOOSE_ADMIRAL_PORT = {screen = "trade_routes", a = 4, b = d1, decision = false},
    BUTTONPOPUP_DIPLOVOTE = {screen = "mp_vote", a = two, decision = true},
    BUTTONPOPUP_CHOOSEELECTION = {screen = "mp_vote", a = two, decision = true},
    BUTTONPOPUP_MINOR_CIVS_LIST = {screen = "diplo", decision = false},
    BUTTONPOPUP_DISBANDCITY = {screen = "confirm", a = id, decision = true},
    BUTTONPOPUP_TEXT = {screen = "message", a = id, decision = false},
    BUTTONPOPUP_NEW_ERA = {screen = "message", a = id, decision = false},
    BUTTONPOPUP_NATURAL_WONDER_REWARD = {screen = "message", a = id, decision = false},
    BUTTONPOPUP_GOODY_HUT_REWARD = {screen = "message", a = id, decision = false},
    BUTTONPOPUP_BARBARIAN_CAMP_REWARD = {screen = "message", a = id, decision = false},
    BUTTONPOPUP_GOLDEN_AGE_REWARD = {screen = "message", a = id, decision = false},
    BUTTONPOPUP_GREAT_PERSON_REWARD = {screen = "message", a = id, decision = false},
    BUTTONPOPUP_WONDER_COMPLETED_ACTIVE_PLAYER = {screen = "message", a = id, decision = false},
    BUTTONPOPUP_BARBARIAN_RANSOM = {screen = "confirm", a = id, decision = true, lapsed = unit_gone("data1")},
    BUTTONPOPUP_CONFIRMCOMMAND = {screen = "confirm", a = id, decision = true},
    BUTTONPOPUP_CONFIRM_IMPROVEMENT_REBUILD = {screen = "confirm", a = id, decision = true},
    BUTTONPOPUP_RETURN_CIVILIAN = {screen = "confirm", a = id, decision = true, lapsed = unit_gone("data3")},
    BUTTONPOPUP_GIFT_CONFIRM = {screen = "confirm", a = id, decision = true},
    BUTTONPOPUP_ADVISOR_MODAL = {screen = "confirm", a = id, decision = true},
    BUTTONPOPUP_DECLAREWARMOVE = {screen = "declare_war", a = d1, b = d2, decision = true},
    BUTTONPOPUP_DECLAREWARRANGESTRIKE = {screen = "declare_war", a = d1, b = d2, decision = true},
    BUTTONPOPUP_CITY_STATE_GREETING = {screen = "city_state", a = d1, b = zero, decision = false},
    BUTTONPOPUP_CITY_STATE_MESSAGE = {screen = "city_state", a = d1, b = zero, decision = false},
    BUTTONPOPUP_CITY_STATE_DIPLO = {screen = "city_state", a = d1, b = zero, decision = false},
    BUTTONPOPUP_MINOR_CIV_ENTER_TERRITORY = {screen = "city_state", a = d1, b = one, decision = true},
    BUTTONPOPUP_LIBERATE_MINOR = {screen = "city_state", a = d1, b = two, decision = true},
    BUTTONPOPUP_CHOOSEPOLICY = {screen = "policies", decision = true},
    BUTTONPOPUP_CHOOSE_IDEOLOGY = {screen = "ideology", decision = true, lapsed = ideology_chosen},
    BUTTONPOPUP_GREAT_WORK_COMPLETED_ACTIVE_PLAYER = {screen = "great_work", a = d1, decision = false},
    BUTTONPOPUP_ESPIONAGE_OVERVIEW = {screen = "espionage", decision = false},
    BUTTONPOPUP_RELIGION_OVERVIEW = {screen = "religion_overview", decision = false},
    BUTTONPOPUP_ECONOMIC_OVERVIEW = {screen = "economy", decision = false},
    BUTTONPOPUP_MILITARY_OVERVIEW = {screen = "military", decision = false},
    BUTTONPOPUP_DIPLOMATIC_OVERVIEW = {screen = "diplo", decision = false},
    BUTTONPOPUP_DEMOGRAPHICS = {screen = "demographics", decision = false},
    BUTTONPOPUP_VICTORY_INFO = {screen = "victory", decision = false},
}
