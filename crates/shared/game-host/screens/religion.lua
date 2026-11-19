local function yes(v) return v == true or v == 1 end
-- ChooseReligionPopup's label: "Founded in" and the current game year.
local function founded_in()
    local year = Game.GetGameTurnYear()
    return L("TXT_KEY_CHOOSE_RELIGION_FOUNDED_IN", L(year < 0 and "TXT_KEY_TIME_BC" or "TXT_KEY_TIME_AD", math.abs(year)))
end
local function belief(id)
    local b = GameInfo.Beliefs[id]
    if b then return {id = id, key = b.Type, name = L(b.ShortDescription), short_name = L(b.ShortDescription),
        effect = plain(L(b.Description)), icon = icon(b.IconAtlas, b.PortraitIndex)} end
end
local function options(kind)
    local rows = array()
    local names = {pantheon = "GetAvailablePantheonBeliefs", founder = "GetAvailableFounderBeliefs", follower = "GetAvailableFollowerBeliefs",
        enhancer = "GetAvailableEnhancerBeliefs", bonus = "GetAvailableBonusBeliefs"}
    for _, id in ipairs(Game[names[kind]]()) do
        local row = belief(id)
        if row then rows[#rows + 1] = row end
    end
    table.sort(rows, function(a,b) return a.name < b.name end)
    return rows
end
local function own_city(player, id)
    if id == nil or id < 0 then return nil end
    local c = Players[player]:GetCityByID(id)
    if c and c:GetOwner() == player then return c end
end
local function pending(p, city, enhance)
    if Game.IsOption(GameOptionTypes.GAMEOPTION_NO_RELIGION) then return "Religion is disabled" end
    if not city then return "Select an owned city containing a Great Prophet" end
    if enhance and not p:HasCreatedReligion() then return "No religion to enhance" end
    if not enhance and p:HasCreatedReligion() then return "A religion is already founded" end
    if not enhance and Game.GetNumReligionsStillToFound() <= 0 then return "No religions remain to be founded" end
    local mission = enhance and GameInfoTypes.MISSION_ENHANCE_RELIGION or GameInfoTypes.MISSION_FOUND_RELIGION
    for unit in p:Units() do
        if unit:GetX() == city:GetX() and unit:GetY() == city:GetY() and unit:CanStartMission(mission, -1, -1, nil, 0) then return nil end
    end
    return "An eligible Great Prophet is required in this city"
end
local function validate_belief(id, kind, used)
    assert(type(id) == "number" and id >= 0 and id < 65535 and id == math.floor(id), "A belief choice is required")
    assert(not used[id], "Belief choices must be distinct")
    local found = false
    for _, row in ipairs(options(kind)) do if row.id == id then found = true end end
    assert(found, "This belief is no longer available for this slot")
    used[id] = true
    return id
end
local function unpack_pair(value)
    assert(type(value) == "number" and value == math.floor(value), "Packed belief choices are required")
    assert(value >= -2147483648 and value <= 4294967295, "Packed belief choices exceed 32 bits")
    value = value % 4294967296
    return value % 65536, math.floor(value / 65536)
end

return {
    data = function(player, args)
        local p = Players[player]
        local enhance = args.b == 1 and p:HasCreatedReligion()
        local city = own_city(player, args.a)
        local symbols, slots, fixed = array(), array(), {}
        local rid = p:GetReligionCreatedByPlayer()
        local existing
        for r in GameInfo.Religions() do
            if r.Type ~= "RELIGION_PANTHEON" then
                local founder = Game.GetFounder(r.ID, -1)
                local taken = founder ~= -1
                local known
                if taken then known = founder == player or met(player, founder) end
                local civ = known and GameInfo.Civilizations[Players[founder]:GetCivilizationType()]
                symbols[#symbols + 1] = {id = r.ID, key = r.Type, name = L(r.Description), icon = icon(r.IconAtlas, r.PortraitIndex),
                    taken = taken, founder = taken and (civ and L(civ.ShortDescription) or "Unmet player") or nil,
                    founder_known = known, tooltip = taken and "This symbol is already taken" or L(r.Description)}
            end
        end
        if p:HasCreatedReligion() then
            local r = GameInfo.Religions[rid]
            local holy = Game.GetHolyCityForReligion(rid, player)
            -- The popup's "founded in" label reads the current game year in the original too (ChooseReligionPopup).
            existing = {id = rid, key = r.Type, name = L(Game.GetReligionName(rid)), icon = icon(r.IconAtlas, r.PortraitIndex),
                default_name = L(r.Description), holy_city = holy and (holy:GetOwner() == player or visible(holy:Plot(), p:GetTeam())) and holy:GetName() or "", founded = founded_in()}
            for index, id in ipairs(Game.GetBeliefsInReligion(rid)) do
                local b = GameInfo.Beliefs[id]
                if not yes(b.Reformation) then
                    local kind = yes(b.Pantheon) and "pantheon" or yes(b.Founder) and "founder" or yes(b.Enhancer) and "enhancer" or "follower"
                    if index == 4 and p:IsTraitBonusReligiousBelief() then kind = "bonus"
                    elseif fixed[kind] then kind = kind == "follower" and "follower2" or "bonus" end
                    fixed[kind] = belief(id)
                end
            end
        elseif p:HasCreatedPantheon() then fixed.pantheon = belief(p:GetBeliefInPantheon()) end
        for _, spec in ipairs({{"pantheon", "pantheon"}, {"founder", "founder"}, {"follower", "follower"},
            {"follower2", "follower"}, {"enhancer", "enhancer"}, {"bonus", "bonus"}}) do
            local key, kind = spec[1], spec[2]
            local state, why = "choose", nil
            if not enhance and (key == "follower2" or key == "enhancer") then state, why = "locked", "Added when you enhance" end
            if key == "bonus" and not p:IsTraitBonusReligiousBelief() then state, why = "locked", "Requires the bonus belief trait" end
            if enhance and key == "bonus" and not fixed[key] then state, why = "locked", "Bonus beliefs are chosen when founding" end
            if fixed[key] then state, why = "fixed", nil end
            slots[#slots + 1] = {key = key, type = kind, label = key, state = state, required = state == "choose", fixed = fixed[key],
                reason = why, options = state == "choose" and options(kind) or array()}
        end
        local civ = GameInfo.Civilizations[p:GetCivilizationType()]
        local why = pending(p, city, enhance)
        return {mode = enhance and "enhance" or "found", city = city and {id = city:GetID(), key = key_city(player, city:GetID()), name = city:GetName()},
            date = founded_in(), emblem = icon(civ.IconAtlas, civ.PortraitIndex),
            symbols = symbols, religion = existing, slots = slots, enabled = why == nil, reason = why,
            custom_name_available = true}
    end,
    actions = {
        found = function(player, args, net)
            local p = Players[player]
            local city = own_city(player, args.b)
            local why = pending(p, city, false)
            assert(not why, why)
            local symbol = GameInfo.Religions[args.a]
            assert(symbol and symbol.Type ~= "RELIGION_PANTHEON" and Game.GetFounder(args.a, -1) == -1, "This religion symbol is unavailable")
            local name = (args.text or ""):match("^%s*(.-)%s*$")
            local _, length = name:gsub("[^\128-\193]", "")
            assert(length <= 16, "Religion names must have at most 16 characters")
            assert(not name:find("[%c<>|]"), "Religion names contain reserved symbols")
            local founder, follower = unpack_pair(args.c)
            local pantheon, bonus = unpack_pair(args.d)
            local used = {}
            if p:HasCreatedPantheon() then used[p:GetBeliefInPantheon()] = true end
            validate_belief(founder, "founder", used)
            validate_belief(follower, "follower", used)
            if not p:HasCreatedPantheon() then validate_belief(pantheon, "pantheon", used)
            else assert(pantheon == 65535, "An existing pantheon must not be replaced") end
            if p:IsTraitBonusReligiousBelief() then validate_belief(bonus, "bonus", used)
            else assert(bonus == 65535, "This civilization has no bonus belief") end
            local ids = {founder, follower}
            if pantheon ~= 65535 then ids[#ids + 1] = pantheon end
            if bonus ~= 65535 then ids[#ids + 1] = bonus end
            net.send_text("SendFoundReligion", name, args.a, ids[1], ids[2], ids[3] or -1, ids[4] or -1, city:GetX(), city:GetY())
            if args.popup_id then net.popup_done(args.popup_id) end
            return {close = true}
        end,
        enhance = function(player, args, net)
            local p = Players[player]
            local city = own_city(player, args.b)
            local why = pending(p, city, true)
            assert(not why, why)
            assert(args.a == p:GetReligionCreatedByPlayer(), "This religion does not belong to the acting player")
            local used = {}
            validate_belief(args.c, "follower", used)
            validate_belief(args.d, "enhancer", used)
            net.send_text("SendEnhanceReligion", "", args.a, args.c, args.d, city:GetX(), city:GetY())
            if args.popup_id then net.popup_done(args.popup_id) end
            return {close = true}
        end,
    },
}
