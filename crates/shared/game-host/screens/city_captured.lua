local function event_for(args, c)
    local p = args.popup
    if args.popup_id and p and p.kind == "BUTTONPOPUP_CITY_CAPTURED" and p.data1 == c:GetID() then return p end
end
local function choices_for(player, c, event)
    local p = Players[player]
    return {annex = event ~= nil and not p:MayNotAnnex(), puppet = event ~= nil and not p:MayNotAnnex(),
        raze = event ~= nil and p:CanRaze(c), liberate = false}
end
-- SIMPLIFIED(SIMP-420): the popup transport omits capture integers four/five, so captured Great Works and liberation authorization are unavailable.
local unavailable = "Capture decisions require an authenticated pending game event."
local function city(player, id)
    assert(type(id) == "number" and id == math.floor(id), "city id is required")
    local c = Players[player]:GetCityByID(id)
    assert(c and c:GetOwner() == player, "city is unavailable")
    return c
end
local function civ(player, other)
    if not Players[other] or (other ~= player and not met(player, other)) then return "Unknown civilization" end
    return L(GameInfo.Civilizations[Players[other]:GetCivilizationType()].ShortDescription)
end
local function capture_action(player, args, net, choice)
    local c = city(player, args.a)
    assert(Players[player]:IsTurnActive(), "Wait for your turn.")
    local event = event_for(args, c)
    assert(event, unavailable)
    assert(choices_for(player, c, event)[choice], "This capture choice is unavailable.")
    if choice == "liberate" then net.send("SendLiberateMinor", event.original_owner, c:GetID())
    else
        local id = choice == "puppet" and TaskTypes.TASK_CREATE_PUPPET or (choice == "raze" and TaskTypes.TASK_RAZE or TaskTypes.TASK_ANNEX_PUPPET)
        assert(choice ~= "raze" or Players[player]:CanRaze(c), "This city cannot be razed.")
        assert(choice ~= "annex" or not Players[player]:MayNotAnnex(), "Annexation is unavailable.")
        net.send("sendDoTask", c:GetID(), id, -1, -1, 0, 0, 0, 0)
    end
    net.popup_done(args.popup_id)
    return {close = true, open = choice == "annex" and {screen = "city", a = c:GetID()} or nil}
end
return {
    data = function(player, args)
        if args.a == nil then return {mode = "captured", city = -1, name = "No city selected", options = array(), can_view = false, reason = "Choose an owned city first."} end
        local c = city(player, args.a)
        local mode = args.b == 1 and "annex" or "captured"
        local options = array()
        local event = event_for(args, c)
        local allowed = choices_for(player, c, event)
        if mode == "captured" then
            for _, row in ipairs({{"liberate", "Liberate", "Return the city to its original owner."}, {"annex", "Annex", "Control production, citizens and buildings."}, {"puppet", "Make a puppet", "The city chooses its own production."}, {"raze", "Raze", "Burn the city down."}}) do
                options[#options + 1] = {key = row[1], title = row[2], text = row[3], enabled = Players[player]:IsTurnActive() and allowed[row[1]], reason = not allowed[row[1]] and (row[1] == "liberate" and "Liberation details are unavailable." or unavailable) or nil}
            end
        end
        local annex = mode == "annex" and c:IsPuppet() and not Players[player]:MayNotAnnex() and Players[player]:IsTurnActive()
        return {mode = mode, city = c:GetID(), name = c:GetName(), population = c:GetPopulation(), taken_from = civ(player, c:GetPreviousOwner()), founded_by = civ(player, c:GetOriginalOwner()), minor_buyout = event and event.option1 or false, loot = event and {gold = event.data2, culture = event.data3, great_works = array()} or nil, options = options, can_view = true, may_annex = mode == "annex" and annex or nil, reason = not event and mode == "captured" and unavailable or (mode == "annex" and not annex and "This city cannot be annexed now." or nil)}
    end,
    actions = {
        annex = function(player, args, net)
            local c = city(player, args.a)
            if args.popup and args.popup.kind == "BUTTONPOPUP_CITY_CAPTURED" then
                return capture_action(player, args, net, "annex")
            end
            assert(not args.popup or args.popup.data1 == c:GetID(), "Wrong pending annex city")
            if c:IsPuppet() then
                assert(Players[player]:IsTurnActive() and not Players[player]:MayNotAnnex(), "This city cannot be annexed now.")
                net.send("sendDoTask", c:GetID(), TaskTypes.TASK_ANNEX_PUPPET, -1, -1, 0, 0, 0, 0)
                if args.popup_id then net.popup_done(args.popup_id) end
                return {close = true, open = {screen = "city", a = c:GetID()}}
            else return capture_action(player, args, net, "annex") end
        end,
        liberate = function(player, args, net) return capture_action(player, args, net, "liberate") end,
        puppet = function(player, args, net) return capture_action(player, args, net, "puppet") end,
        raze = function(player, args, net) return capture_action(player, args, net, "raze") end,
    },
}
