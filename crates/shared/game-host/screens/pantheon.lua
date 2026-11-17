local function reason(p, reform)
    if Game.IsOption(GameOptionTypes.GAMEOPTION_NO_RELIGION) then return "Religion is disabled" end
    if p:GetNumCities() == 0 then return "Found a city first" end
    if reform then
        if not p:CanChooseReformationBelief() then return "No reformation belief choice is pending" end
    elseif not p:CanCreatePantheon(true) then return "A pantheon is already founded or more faith is required" end
end

return {
    data = function(player, args)
        local p = Players[player]
        local reform = args.a == 1
        local rows = array()
        local ids = reform and Game.GetAvailableReformationBeliefs() or Game.GetAvailablePantheonBeliefs()
        for _, id in ipairs(ids) do
            local b = GameInfo.Beliefs[id]
            if b then rows[#rows + 1] = {id = id, key = b.Type, name = L(b.ShortDescription), short_name = L(b.ShortDescription),
                effect = L(b.Description), icon = icon(b.IconAtlas, b.PortraitIndex)} end
        end
        table.sort(rows, function(a,b) return a.name < b.name end)
        local why = reason(p, reform)
        local civ = GameInfo.Civilizations[p:GetCivilizationType()]
        return {mode = reform and "reformation" or "pantheon", emblem = icon(civ.IconAtlas, civ.PortraitIndex),
            religion_name = reform and L(Game.GetReligionName(p:GetReligionCreatedByPlayer())) or nil, beliefs = rows, enabled = why == nil, reason = why,
            completed = reform and not p:CanChooseReformationBelief() or (not reform and p:HasCreatedPantheon())}
    end,
    actions = {choose = function(player, args, net)
        local b = GameInfo.Beliefs[args.a]
        assert(b, "Unknown belief")
        local reform = b.Reformation == true or b.Reformation == 1
        if args.popup then
            assert(args.popup.kind == "BUTTONPOPUP_FOUND_PANTHEON", "Wrong belief popup")
            assert(reform == (args.popup.data2 ~= 1), "Wrong pending belief mode")
        end
        local why = reason(Players[player], reform)
        assert(not why, why)
        local ids = reform and Game.GetAvailableReformationBeliefs() or Game.GetAvailablePantheonBeliefs()
        local found = false
        for _, id in ipairs(ids) do if id == args.a then found = true end end
        assert(found, "This belief is no longer available")
        net.send("SendFoundPantheon", args.a)
            if args.popup_id then net.popup_done(args.popup_id) end
            return {close = true}
    end},
}
