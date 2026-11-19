local function context(player, args)
    local popup = args.popup
    if not popup or popup.kind ~= "BUTTONPOPUP_CHOOSE_GOODY_HUT_REWARD" or popup.id ~= args.a then return nil end
    assert(popup.data1 == player, "This ruin choice belongs to another seat")
    return popup
end

local function explorer(player, popup)
    local unit = Players[player]:GetUnitByID(popup.data2)
    if not unit or unit:GetOwner() ~= player or unit:IsDead() or unit:IsDelayedDeath() then
        return nil, nil, "The ruin explorer is no longer available"
    end
    local plot = Map.GetPlot(unit:GetX(), unit:GetY())
    if not plot then return nil, nil, "The ruin plot is unavailable" end
    return unit, plot
end

-- The original puts its icons in the localized reward descriptions.
local function reward_parts(text)
    local parts, start = array(), 1
    while true do
        local first, last, glyph = text:find("%[(ICON_[%w_]+)%]", start)
        if not first then break end
        if first > start then parts[#parts + 1] = {id = #parts, text = text:sub(start, first - 1)} end
        parts[#parts + 1] = {id = #parts, icon = "font:" .. glyph}
        start = last + 1
    end
    if start <= #text then parts[#parts + 1] = {id = #parts, text = text:sub(start)} end
    return parts
end

return {
    data = function(player, args)
        local p = assert(Players[player], "Unknown seat")
        local popup = context(player, args)
        local r = {id = popup and popup.id or -1, unit = {id = -1, name = "", icon = ""},
            plot = {x = 0, y = 0}, rewards = array(), resolved = false}
        if not popup then
            r.reason = "No matching ancient ruin event is pending for this seat"
            return r
        end
        local unit, plot, reason = explorer(player, popup)
        if not unit then
            r.reason = reason
            return r
        end
        local info = GameInfo.Units[unit:GetUnitType()]
        r.unit = {id = popup.data2, name = unit:GetName(), icon = info and "unit:" .. info.Type or ""}
        r.plot = {x = plot:GetX(), y = plot:GetY()}
        for goody in GameInfo.GoodyHuts() do
            if p:CanGetGoody(plot, goody.ID, unit) then
                r.rewards[#r.rewards + 1] = {id = goody.ID, key = goody.Type,
                    parts = reward_parts(L(goody.ChooseDescription or goody.Description))}
            end
        end
        if #r.rewards == 0 then r.reason = "There are no eligible rewards for this explorer" end
        return r
    end,
    actions = {
        take = function(player, args, net)
            local popup = assert(context(player, args), "This ancient ruin choice is no longer pending")
            local unit, plot, reason = explorer(player, popup)
            assert(unit, reason)
            assert(args.b and GameInfo.GoodyHuts[args.b] and Players[player]:CanGetGoody(plot, args.b, unit),
                "This reward is not eligible")
            net.send("sendGoodyChoice", plot:GetX(), plot:GetY(), args.b, popup.data2)
            net.popup_done(args.popup_id)
            return {close = true}
        end,
        cancel = function(player, args, net)
            local popup = assert(context(player, args), "This ancient ruin choice is no longer pending")
            assert(not explorer(player, popup), "Choose a reward for the explorer")
            net.popup_done(args.popup_id)
            return {close = true}
        end,
    },
}
