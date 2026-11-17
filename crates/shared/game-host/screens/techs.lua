return {
    data = function(player, args)
        local p = Players[player]
        local team = team_of(player)
        local tt = team:GetTeamTechs()
        local rows = array()
        for tech in GameInfo.Technologies() do
            local id = tech.ID
            local pos = p:GetQueuePosition(id)
            local status = "locked"
            if team:IsHasTech(id) then status = "researched"
            elseif p:GetCurrentResearch() == id then status = "current"
            elseif pos > 0 then status = "queued"
            elseif p:CanResearch(id) then status = "available" end
            local prereqs = array()
            for link in GameInfo.Technology_PrereqTechs{TechType = tech.Type} do
                local prev = GameInfo.Technologies[link.PrereqTech]
                if prev then prereqs[#prereqs + 1] = { id = prev.ID } end
            end
            local unlocks = array()
            for _, tableName in ipairs({"Units", "Buildings", "Builds"}) do
                for item in GameInfo[tableName]{PrereqTech = tech.Type} do
                    if #unlocks < 6 then unlocks[#unlocks + 1] = { idx = #unlocks, name = L(item.Description) } end
                end
            end
            local era = GameInfo.Eras[tech.Era]
            rows[#rows + 1] = {
                id = id, name = L(tech.Description), column = tech.GridX, row = tech.GridY,
                status = status, cost = p:GetResearchCost(id), progress = tt:GetResearchProgress(id),
                turns = p:GetResearchTurnsLeft(id, true), prerequisites = prereqs,
                queue_position = pos, era = era.ID, era_name = L(era.Description),
                icon = icon(tech.IconAtlas, tech.PortraitIndex), unlocks = unlocks,
            }
        end
        table.sort(rows, function(a,b)
            if a.era ~= b.era then return a.era < b.era end
            if a.column ~= b.column then return a.column < b.column end
            if a.row ~= b.row then return a.row < b.row end
            return a.id < b.id
        end)
        return { techs = rows }
    end,
    actions = {
        research = function(player, args, net)
            assert(type(args.a) == "number" and args.a == math.floor(args.a), "technology id is required")
            local tech = GameInfo.Technologies[args.a]
            assert(tech and not team_of(player):IsHasTech(args.a), "technology is unavailable")
            assert(args.b == nil or args.b == 0 or args.b == 1, "queue flag must be 0 or 1")
            net.send("sendResearch", args.a, 0, -1, args.b or 0)
        end,
    },
}
