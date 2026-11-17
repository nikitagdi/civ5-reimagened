local modes = {[0] = "free", [1] = "faith", [2] = "maya"}
local pending = {}
local branches = {
    UNITCLASS_WRITER = "POLICY_BRANCH_AESTHETICS", UNITCLASS_ARTIST = "POLICY_BRANCH_AESTHETICS", UNITCLASS_MUSICIAN = "POLICY_BRANCH_AESTHETICS",
    UNITCLASS_SCIENTIST = "POLICY_BRANCH_RATIONALISM", UNITCLASS_MERCHANT = "POLICY_BRANCH_COMMERCE", UNITCLASS_ENGINEER = "POLICY_BRANCH_TRADITION",
    UNITCLASS_GREAT_GENERAL = "POLICY_BRANCH_HONOR", UNITCLASS_GREAT_ADMIRAL = "POLICY_BRANCH_EXPLORATION",
}
local function remaining(p, mode)
    if mode == "faith" then return p:GetNumFaithGreatPeople() end
    if mode == "maya" then return p:GetNumMayaBoosts() end
    return p:GetNumFreeGreatPeople()
end
local function choices(player, mode)
    local p, people = Players[player], array()
    local civ = GameInfo.Civilizations[p:GetCivilizationType()]
    local overrides = {}
    for row in GameInfo.Civilization_UnitClassOverrides{CivilizationType = civ.Type} do overrides[row.UnitClassType] = row.UnitType or false end
    for row in GameInfo.Units{Special = "SPECIALUNIT_PEOPLE"} do
        local cls = GameInfo.UnitClasses[row.Class]
        local own_type = overrides[row.Class]
        if own_type == nil then own_type = cls and cls.DefaultUnit end
        if own_type == row.Type and (row.Class ~= "UNITCLASS_PROPHET" or p:HasCreatedPantheon()) then
            local why
            if remaining(p, mode) <= 0 then why = "No Great Person choices remain."
            elseif not p:GetCapitalCity() then why = "A city is required before choosing a Great Person."
            elseif mode == "maya" and p:GetUnitBaktun(row.ID) > 0 and not p:IsFreeMayaGreatPersonChoice() then why = "Already chosen in an earlier baktun."
            elseif mode == "faith" then
                local branch = branches[row.Class] and GameInfo.PolicyBranchTypes[branches[row.Class]]
                if branch and not p:IsPolicyBranchFinished(branch.ID) then why = "Needs the " .. L(branch.Description) .. " branch completed."
                elseif row.Class == "UNITCLASS_PROPHET" and p:GetReligionCreatedByPlayer() < 0 and Game.GetNumReligionsStillToFound() <= 0 then why = "No religion can still be founded." end
            end
            -- SIMPLIFIED(SIMP-357): Abilities use localized native help; per-game numeric ability projections and exact spawn-city selection need additional queries.
            people[#people + 1] = {unit = row.ID, key = row.Type, name = L(row.Description), icon = icon(row.IconAtlas, row.PortraitIndex), strategy = L(row.Strategy or row.Help or row.Description), abilities = array({{index = 0, label = "Abilities", text = L(row.Help or row.Description)}}), available = not why, reason = why}
        end
    end
    return people
end
return {
    data = function(player, args)
        local id = args.a or 0
        local mode = assert(modes[id], "unknown Great Person choice mode")
        pending[player] = id
        local p = Players[player]
        local people = choices(player, mode)
        local any = false; for _, row in ipairs(people) do any = any or row.available end
        local capital = p:GetCapitalCity()
        return {mode = mode, mode_id = id, title = mode == "maya" and "Long Count" or (mode == "faith" and "Great Person for faith" or "Choose a free Great Person"), instructions = "Choose a Great Person, review the abilities, then confirm.", remaining = remaining(p, mode), can_close = mode == "free" or (mode == "maya" and not any), people = people, appears_in = capital and capital:GetName() or nil}
    end,
    actions = {
        choose = function(player, args, net)
            local id = pending[player]
            assert(id ~= nil and (args.b == nil or args.b == id), "Open the Great Person choice first.")
            local mode = modes[id]
            assert(remaining(Players[player], mode) > 0, "No Great Person choices remain.")
            local chosen
            for _, row in ipairs(choices(player, mode)) do if row.unit == args.a then chosen = row end end
            assert(chosen, "This Great Person is unavailable to your civilization.")
            assert(chosen.available, chosen.reason)
            local message = mode == "maya" and "sendMayaBonusChoice" or (mode == "faith" and "sendFaithGreatPersonChoice" or "sendGreatPersonChoice")
            net.send(message, args.a)
        end,
    },
}
