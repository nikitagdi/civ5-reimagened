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
        local quote, author = tech.Quote and (L(tech.Quote):gsub("%[TAB%]", "")), nil
        if quote then
            local body, by = quote:gsub(string.char(226, 128, 148), "--"):match("^(.-)%[NEWLINE%]%s*%-+%s*(.+)$")
            if body then quote, author = body, by end
        end
        local unlocks = tech_unlocks_for(player)
        local items = unlocks(tech)
        for _, item in ipairs(items) do
            item.kind_name = ({unit = "Unit", building = "Building", wonder = "World wonder", project = "Project", resource = "Resource", improvement = "Improvement"})[item.kind] or "Ability"
        end
        return {tech = {key = tech.Type, name = L(tech.Description), icon = icon(tech.IconAtlas, tech.PortraitIndex),
            portrait = icon(tech.IconAtlas, tech.PortraitIndex)}, era = L(era.Description), turn = Game.GetGameTurn(),
            quote = quote, quote_by = author, help = tech.Help and L(tech.Help) or "", leads_to = leads, unlocks = items}
    end,
    actions = {},
}
