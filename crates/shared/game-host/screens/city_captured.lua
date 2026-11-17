local pending = {}
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
    -- SIMPLIFIED(SIMP-355): Query arguments cannot authorize a capture choice; wait for the lead's authenticated event queue.
    local event = pending[player]
    assert(event and event.city == c:GetID(), unavailable)
    assert(event.choices[choice], "This capture choice is unavailable.")
    if choice == "liberate" then net.send("SendLiberateMinor", event.original_owner, c:GetID())
    else
        local id = choice == "puppet" and TaskTypes.TASK_CREATE_PUPPET or (choice == "raze" and TaskTypes.TASK_RAZE or TaskTypes.TASK_ANNEX_PUPPET)
        assert(choice ~= "raze" or Players[player]:CanRaze(c), "This city cannot be razed.")
        assert(choice ~= "annex" or not Players[player]:MayNotAnnex(), "Annexation is unavailable.")
        net.send("sendDoTask", c:GetID(), id, -1, -1, 0, 0, 0, 0)
    end
end
return {
    data = function(player, args)
        if args.a == nil then return {mode = "captured", city = -1, name = "No city selected", options = array(), can_view = false, reason = "Choose an owned city first."} end
        local c = city(player, args.a)
        local mode = args.b == 1 and "annex" or "captured"
        local options = array()
        if mode == "captured" then
            for _, row in ipairs({{"liberate", "Liberate", "Return the city to its original owner."}, {"annex", "Annex", "Control production, citizens and buildings."}, {"puppet", "Make a puppet", "The city chooses its own production."}, {"raze", "Raze", "Burn the city down."}}) do
                options[#options + 1] = {key = row[1], title = row[2], text = row[3], enabled = false, reason = unavailable}
            end
        end
        local annex = mode == "annex" and c:IsPuppet() and not Players[player]:MayNotAnnex() and Players[player]:IsTurnActive()
        return {mode = mode, city = c:GetID(), name = c:GetName(), population = c:GetPopulation(), taken_from = civ(player, c:GetPreviousOwner()), founded_by = civ(player, c:GetOriginalOwner()), minor_buyout = false, options = options, can_view = true, may_annex = mode == "annex" and annex or nil, reason = mode == "captured" and unavailable or (not annex and "This city cannot be annexed now." or nil)}
    end,
    actions = {
        annex = function(player, args, net)
            local c = city(player, args.a)
            if c:IsPuppet() then
                assert(Players[player]:IsTurnActive() and not Players[player]:MayNotAnnex(), "This city cannot be annexed now.")
                net.send("sendDoTask", c:GetID(), TaskTypes.TASK_ANNEX_PUPPET, -1, -1, 0, 0, 0, 0)
            else capture_action(player, args, net, "annex") end
        end,
        liberate = function(player, args, net) capture_action(player, args, net, "liberate") end,
        puppet = function(player, args, net) capture_action(player, args, net, "puppet") end,
        raze = function(player, args, net) capture_action(player, args, net, "raze") end,
    },
}
