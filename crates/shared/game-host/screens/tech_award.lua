-- All queries are scoped to the requesting seat; orders are validated again on each action.
local function science_off()
    return Game.IsOption(GameInfoTypes.GAMEOPTION_NO_SCIENCE)
end
local function fraction(n, cost) return cost > 0 and math.max(0, math.min(1, n / cost)) or 0 end
local function turns(p, id, rate)
    if rate <= 0 or science_off() then return nil end
    local n = p:GetResearchTurnsLeft(id, true)
    if n >= 0 and n < 2147483647 then return n end
end
local function unlocks(player, tech)
    local out, seen = array(), {}
    local civ = GameInfo.Civilizations[Players[player]:GetCivilizationType()]
    for _, spec in ipairs({{"Units", "unit", "UnitClass", "Civilization_UnitClassOverrides", "UnitType"},
        {"Buildings", "building", "BuildingClass", "Civilization_BuildingClassOverrides", "BuildingType"},
        {"Projects", "project"}, {"Builds", "improvement"}}) do
        for item in GameInfo[spec[1]]{PrereqTech = tech.Type} do
            local chosen = item
            if spec[4] then
                for override in GameInfo[spec[4]]{CivilizationType = civ.Type} do
                    if override[spec[3] .. "Type"] == item[spec[3]] then
                        chosen = override[spec[5]] and GameInfo[spec[1]][override[spec[5]]] or nil
                    end
                end
            end
            if chosen and chosen.PrereqTech == tech.Type and not seen[chosen.Type] then
                seen[chosen.Type] = true
                local kind = spec[2]
                if kind == "building" then
                    local class = GameInfo.BuildingClasses[chosen.BuildingClass]
                    if class and (class.MaxGlobalInstances or -1) > 0 then kind = "wonder" end
                end
                if #out < 5 then out[#out + 1] = {idx = #out, key = chosen.Type, kind = kind,
                    name = L(chosen.Description), icon = icon(chosen.IconAtlas, chosen.PortraitIndex)} end
            end
        end
    end
    -- SIMPLIFIED(SIMP-341): unlock chips cover civilization-specific units, buildings, projects and build actions; passive abilities/resources need a shared catalog.
    return out
end
return {
    data = function(player, args)
        local tech = args.a and GameInfo.Technologies[args.a]
        if not tech then return {} end
        local era, leads, seen = GameInfo.Eras[tech.Era], array(), {}
        for _, name in ipairs({"Technology_PrereqTechs", "Technology_ORPrereqTechs"}) do
            for link in GameInfo[name]{PrereqTech = tech.Type} do
                local next = GameInfo.Technologies[link.TechType]
                if next and not seen[next.Type] then
                    seen[next.Type] = true; leads[#leads + 1] = {key = next.Type, name = L(next.Description)}
                end
            end
        end
        local quote, author = tech.Quote and L(tech.Quote), nil
        if quote then
            local body, by = quote:gsub(string.char(226, 128, 148), "--"):match("^(.-)%[NEWLINE%]%s*%-+%s*(.+)$")
            if body then quote, author = body, by end
        end
        local items = unlocks(player, tech)
        for _, item in ipairs(items) do
            item.idx = nil
            item.kind_name = ({unit = "Unit", building = "Building", wonder = "World wonder", project = "Project", improvement = "Improvement"})[item.kind] or "Ability"
        end
        return {tech = {key = tech.Type, name = L(tech.Description), icon = icon(tech.IconAtlas, tech.PortraitIndex),
            portrait = icon(tech.IconAtlas, tech.PortraitIndex)}, era = L(era.Description), turn = Game.GetGameTurn(),
            quote = quote, quote_by = author, help = tech.Help and L(tech.Help) or "", leads_to = leads, unlocks = items}
    end,
    actions = {},
}
