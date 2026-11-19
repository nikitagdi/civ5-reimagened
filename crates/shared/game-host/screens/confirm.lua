-- Confirmation context is reconstructed from the authenticated per-seat queue head.
local kinds = {
    BUTTONPOPUP_BARBARIAN_RANSOM="barbarian_ransom", BUTTONPOPUP_RETURN_CIVILIAN="return_civilian",
    BUTTONPOPUP_GIFT_CONFIRM="gift_unit", BUTTONPOPUP_CONFIRMCOMMAND="confirm_command",
    BUTTONPOPUP_CONFIRM_IMPROVEMENT_REBUILD="improvement_rebuild", BUTTONPOPUP_ADVISOR_MODAL="combat_warning",
    BUTTONPOPUP_CONFIRM_MENU="exit_game", BUTTONPOPUP_CONFIRM_CITY_TASK="confirm_command",
    BUTTONPOPUP_DISBANDCITY="confirm_command",
}
local function choice(id, key, label, primary, reason)
    return {id=id, key=key, label=label, primary=primary, enabled=reason == nil, reason=reason}
end
local function context(player, args)
    local p = args.popup
    if not p or p.id ~= args.a or not kinds[p.kind] then return nil end
    local e = {id=p.id, kind=kinds[p.kind], text=p.text, flags=p.flag, alt=p.option1}
    if e.kind == "return_civilian" then
        assert(p.data1 == player, "This civilian question belongs to another seat")
        e.recipient, e.unit = p.data2, p.data3
    elseif e.kind == "barbarian_ransom" then e.unit = p.data1 end
    -- SIMPLIFIED(SIMP-460): command and rebuild prompts come only from the core's selection-based action handler, which a server never runs, and gift prompts from the original client, so no payload names their unit; affirmative replay stays unavailable.
    return e
end
local function owned(player, e)
    local u = Players[player]:GetUnitByID(e.unit)
    assert(u and u:GetOwner() == player, "The unit is no longer yours")
    return u
end
local function order_reason(player,e)
    if not e.unit then return "The original unit and order context is unavailable" end
    local u=Players[player]:GetUnitByID(e.unit)
    if not u or u:GetOwner()~=player then return "The unit is no longer yours" end
    if e.kind=="improvement_rebuild" then
        if not e.mission or not GameInfo.Missions[e.mission] then return "The original build mission is unavailable" end
        if not u:CanStartMission(e.mission,e.data1 or 0,e.data2 or 0,nil,0) then return "The unit cannot perform this build mission here" end
    elseif e.kind=="confirm_command" then
        if not e.command or not GameInfo.Commands[e.command] then return "The original unit command is unavailable" end
        if not u:CanDoCommand(e.command,e.data1 or 0,e.data2 or 0,0,1) then return "The unit cannot perform this command now" end
    end
end
local function model(player, e)
    local p = Players[player]
    local r = {id=e.id, kind=e.kind, title=e.title or L("TXT_KEY_POPUP_ARE_YOU_SURE"),
        text=e.text or "", kicker=e.kicker, icon=e.icon, civ=e.civ, advisor=e.advisor,
        facts=array(), consequence=e.consequence, dont_warn=e.dont_warn, closable=false,
        esc_choice=e.esc_choice, enter_choice=e.enter_choice, choices=array(), resolved=e.resolved or false}
    if e.kind=="combat_warning" or e.kind=="confirm_command" then r.esc_choice=1;r.enter_choice=1 end
    if e.kind=="exit_game" then r.esc_choice=1;r.enter_choice=nil end
    if e.kind=="combat_warning" then r.dont_warn={label="Do not warn about this attack again",checked=false} end
    if e.resolved then return r end
    if e.kind == "barbarian_ransom" then
        local gold = p:GetGold()
        local ransom = math.min(GameDefines.BARBARIAN_UNIT_GOLD_RANSOM, gold)
        r.facts = array({{key="ransom", label=L("TXT_KEY_POPUP_BARBARIAN_RANSOM_TITLE"), value=ransom, yield="gold"},
            {key="treasury", label=L("TXT_KEY_TOP_PANEL_GOLD"), value=gold, yield="gold"}})
        r.choices = array({choice(1,"abandon",L("TXT_KEY_POPUP_BARBARIAN_RANSOM_ABANDON"),false),
            choice(0,"pay",L("TXT_KEY_POPUP_BARBARIAN_RANSOM_PAY",ransom),true)})
    elseif e.kind == "return_civilian" then
        r.choices = array({choice(1,"keep",L("TXT_KEY_POPUP_RETURN_CIVILIAN_KEEP"),false),
            choice(0,"return",L("TXT_KEY_POPUP_RETURN_CIVILIAN_RETURN"),true)})
    else
        local reason
        if e.kind=="improvement_rebuild" or e.kind=="confirm_command" then
            reason=order_reason(player,e)
        elseif e.kind == "gift_unit" then
            reason = "The original gift recipient and unit context is unavailable"
        elseif e.kind == "exit_game" then
            -- Yes completes the question here; the skin sends it with `then: 'leave'`, so the client leaves for the
            -- lobby once the server applied the answer.
            reason = nil
        else
            -- SIMPLIFIED(SIMP-390): the combat warning carries no attacker, target or order in its payload, so the warned attack cannot be replayed; the player attacks again from the map.
            reason = "The warned order is not in the question; order it again from the map"
        end
        r.choices = array({choice(1,"no",L("TXT_KEY_NO_BUTTON"),true), choice(0,"yes",L("TXT_KEY_YES_BUTTON"),false,reason)})
    end
    return r
end
return {
    data = function(player, args)
        assert(Players[player], "Unknown seat")
        local e = context(player,args)
        if not e then
            return {id=args.a or -1, kind="confirm_command", title="Confirmation unavailable",
                text="No matching confirmation event is pending for this seat", facts=array(),
                choices=array(), closable=false, resolved=false}
        end
        return model(player,e)
    end,
    actions = {
        choose = function(player,args,net)
            local e = assert(context(player,args), "This confirmation is no longer pending")
            assert(not e.resolved, "This confirmation was already answered")
            assert(not e.submitted, "A confirmation answer is already awaiting the game response")
            if args.b == -1 and e.kind~="combat_warning" and e.kind~="confirm_command" and e.kind~="exit_game" then return end
            assert(args.b == 0 or args.b == 1, "Unknown confirmation choice")
            assert(args.c == nil or args.c == 0 or args.c == 1, "Invalid warning preference")
            local emitted=false
            if e.kind == "barbarian_ransom" then
                owned(player,e)
                net.send("sendBarbarianRansom",args.b,e.unit)
                emitted=true
            elseif e.kind == "return_civilian" then
                owned(player,e)
                assert(Players[e.recipient] and e.recipient ~= player, "Invalid civilian recipient")
                net.send("sendReturnCivilian",args.b == 0 and 1 or 0,e.recipient,e.unit)
                emitted=true
            elseif e.kind == "gift_unit" and args.b == 0 then
                owned(player,e)
                assert(Players[e.recipient] and Players[e.recipient]:IsMinorCiv(), "Invalid gift recipient")
                net.send("sendGiftUnit",e.recipient,e.unit)
                emitted=true
            elseif (e.kind=="improvement_rebuild" or e.kind=="confirm_command") and args.b==0 then
                local reason=order_reason(player,e); assert(not reason,reason)
                if e.kind=="improvement_rebuild" then
                    net.send("sendPushMission",e.unit,e.mission,e.data1 or 0,e.data2 or 0,e.flags or 0,e.shift and 1 or 0)
                else
                    net.send("sendDoCommand",e.unit,e.command,e.data1 or 0,e.data2 or 0,e.alt and 1 or 0)
                end
                emitted=true
            elseif e.kind == "exit_game" and args.b == 0 then
                -- Nothing for the game: leaving is the session's (the chained `then: 'leave'` of the skin).
                emitted=false
            elseif args.b == 0 then
                error("The warned order is not in the question; order it again from the map")
            end
            net.popup_done(args.popup_id)
            return {close=true}
        end,
    },
}
