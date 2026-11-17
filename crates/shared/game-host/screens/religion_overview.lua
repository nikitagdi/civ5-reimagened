local function yes(value) return value == true or value == 1 end
local function belief_row(id, key)
    local b = GameInfo.Beliefs[id]
    local kind = yes(b.Pantheon) and "pantheon" or yes(b.Founder) and "founder" or yes(b.Enhancer) and "enhancer"
        or yes(b.Reformation) and "reformation" or "follower"
    return {key = key, type_order = ({pantheon=0,founder=1,follower=2,enhancer=3,reformation=4})[kind], type = kind, type_label = kind, name = L(b.ShortDescription), effect = L(b.Description)}
end
local function purchase_options(p)
    local rows, people = array({{key = "none", kind = "none", id = -1, label = "Do not spend (remind me later)"}}), array()
    local industrial = GameInfo.Eras.ERA_INDUSTRIAL
    if industrial and p:GetCurrentEra() < industrial.ID and (p:HasCreatedReligion() or Game.GetNumReligionsStillToFound() > 0) then
        rows[#rows + 1] = {key = "save_prophet", kind = "save_prophet", id = -1, label = "Save for a Great Prophet"}
    end
    local city = p:GetCapitalCity()
    if city then
        for u in GameInfo.Units() do
            if p["DoesUnitPassFaithPurchaseCheck"](p, u.ID) and city:IsCanPurchase(false, true, u.ID, -1, -1, YieldTypes.YIELD_FAITH) then
                rows[#rows + 1] = {key = "unit:" .. u.ID, kind = "unit", id = u.ID, label = "Buy " .. L(u.Description), cost = city:GetUnitFaithPurchaseCost(u.ID)}
                if u.Special == "SPECIALUNIT_PEOPLE" then people[#people + 1] = {key = u.Type, name = L(u.Description), icon = icon(u.IconAtlas, u.PortraitIndex)} end
            end
        end
        for b in GameInfo.Buildings() do
            if city:IsCanPurchase(false, true, -1, b.ID, -1, YieldTypes.YIELD_FAITH) then
                rows[#rows + 1] = {key = "building:" .. b.ID, kind = "building", id = b.ID, label = "Buy " .. L(b.Description), cost = city:GetBuildingFaithPurchaseCost(b.ID)}
            end
        end
    end
    table.sort(people, function(a,b) return a.name < b.name end)
    return rows, people
end
local net_kind = {[0] = 0, [1] = 1, [2] = 3, [3] = 2}
local names = {[0] = "none", [1] = "save_prophet", [2] = "unit", [3] = "building"}

return {
    data = function(player, args)
        local p = Players[player]
        local civ = GameInfo.Civilizations[p:GetCivilizationType()]
        local enabled = not Game.IsOption(GameOptionTypes.GAMEOPTION_NO_RELIGION)
        local world, all, mine, income = array(), array(), array(), array()
        local religion, pantheon
        local rid = p:GetReligionCreatedByPlayer()
        if p:HasCreatedPantheon() then
            local b = GameInfo.Beliefs[p:GetBeliefInPantheon()]
            if b then pantheon = {key = b.Type, name = L(b.ShortDescription)} end
        end
        for r in GameInfo.Religions() do
            if r.Type ~= "RELIGION_PANTHEON" then
                local founder = Game.GetFounder(r.ID, -1)
                if founder >= 0 then
                    local known = founder == player or met(player, founder)
                    local c = known and GameInfo.Civilizations[Players[founder]:GetCivilizationType()]
                    local holy = Game.GetHolyCityForReligion(r.ID, -1)
                    local holy_name
                    if holy and (holy:GetOwner() == player or visible(holy:Plot(), p:GetTeam())) then holy_name = holy:GetName() end
                    local name, image = L(Game.GetReligionName(r.ID)), icon(r.IconAtlas, r.PortraitIndex)
                    world[#world + 1] = {key = r.ID, name = name, icon = image, holy_city = holy_name, known = known, cities = Game.GetNumCitiesFollowing(r.ID),
                        is_mine = founder == player, founder = c and {name = L(c.ShortDescription), civ = icon(c.IconAtlas, c.PortraitIndex)} or nil}
                    if founder == player then religion = {id = rid, key = r.Type, name = name, icon = image} end
                    for _, id in ipairs(Game.GetBeliefsInReligion(r.ID)) do
                        local row = belief_row(id, r.ID .. ":" .. id)
                        if founder == player then local own = belief_row(id, tostring(id)); own.type_order = nil; mine[#mine + 1] = own end
                        row.religion_name, row.religion_icon, row.known = name, image, known
                        all[#all + 1] = row
                    end
                end
            end
        end
        for other = 0, GameDefines.MAX_MAJOR_CIVS - 1 do
            local q = Players[other]
            if q and q:IsAlive() and q:HasCreatedPantheon() and not q:HasCreatedReligion() then
                local known = other == player or met(player, other)
                local row
                if known then row = belief_row(q:GetBeliefInPantheon(), "pantheon:" .. other)
                else row = {key = "pantheon:" .. other, type = "pantheon", type_label = "pantheon", type_order = 0, name = "Unknown pantheon", effect = ""} end
                row.religion_name, row.known = known and "Pantheon" or "Unknown pantheon", known
                all[#all + 1] = row
                if other == player then local own = belief_row(q:GetBeliefInPantheon(), "pantheon"); own.type_order = nil; mine[#mine + 1] = own end
            end
        end
        for _, source in ipairs({{"cities", "Cities", "GetFaithPerTurnFromCities"}, {"city_states", "City-states", "GetFaithPerTurnFromMinorCivs"}, {"religion", "Religion", "GetFaithPerTurnFromReligion"}}) do
            local value = p[source[3]](p)
            if value ~= 0 then income[#income + 1] = {key = source[1], label = source[2], value = value} end
        end
        local opts, people = purchase_options(p)
        local native = p:GetFaithPurchaseType()
        local kind = ({[0] = "none", "save_prophet", "building", "unit"})[native] or "none"
        local current = kind
        if kind == "unit" or kind == "building" then current = kind .. ":" .. p:GetFaithPurchaseIndex() end
        local label = "Automatic purchase is currently unavailable"
        for _, opt in ipairs(opts) do if opt.key == current then label = opt.label end end
        local status, text = "not_enough_faith", "More faith is required to found a pantheon"
        if religion then status, text = "founded_religion", "You founded " .. religion.name
        elseif pantheon then status, text = "created_pantheon", "You founded a pantheon"
        elseif p:CanCreatePantheon(true) then status, text = "can_create_pantheon", "You can found a pantheon"
        elseif Game.GetNumReligionsStillToFound() <= 0 then status, text = "too_late", "No more religions can be founded" end
        local next_prophet = p:GetMinimumFaithNextGreatProphet()
        return {enabled = enabled, reason = not enabled and "Religion is disabled" or nil, emblem = icon(civ.IconAtlas, civ.PortraitIndex),
            status = {kind = status, text = text}, religion = religion, pantheon = pantheon, my_beliefs = mine,
            faith = {stored = p:GetFaith(), per_turn = p:GetTotalFaithPerTurn(), next_prophet = next_prophet,
                prophet_tooltip = "Next Great Prophet faith threshold: " .. next_prophet, income = income},
            great_people = people, auto_purchase = {current = current, label = label, options = opts}, world = world,
            religions_left = math.max(0, Game.GetNumReligionsStillToFound()), all_beliefs = all}
    end,
    actions = {auto_purchase = function(player, args, net)
        assert(not Game.IsOption(GameOptionTypes.GAMEOPTION_NO_RELIGION), "Religion is disabled")
        local kind = names[args.a]
        assert(kind and type(args.b) == "number", "Unknown faith purchase choice")
        local found = false
        for _, opt in ipairs(purchase_options(Players[player])) do
            if opt.kind == kind and opt.id == args.b then found = true end
        end
        assert(found, "This faith purchase is no longer available")
        net.send("SendFaithPurchase", net_kind[args.a], args.b)
    end},
}
