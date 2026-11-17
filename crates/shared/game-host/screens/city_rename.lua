local modes = {[0] = "rename", [1] = "raze", [2] = "unraze", [3] = "annex"}
local function city(player, id)
    assert(type(id) == "number" and id == math.floor(id), "city id is required")
    local c = Players[player]:GetCityByID(id)
    assert(c and c:GetOwner() == player, "city is unavailable")
    return c
end
local function allowed(player, c, mode)
    local p = Players[player]
    if not p:IsTurnActive() then return false, "Wait for your turn." end
    if mode == "raze" then return p:CanRaze(c) and not c:IsRazing(), "This city cannot be razed." end
    if mode == "unraze" then return c:IsRazing(), "This city is not being razed." end
    if mode == "annex" then return c:IsPuppet() and not p:MayNotAnnex(), "This city cannot be annexed." end
    -- SIMPLIFIED(SIMP-354): The Lua net emitter cannot attach text to SendRenameCity yet.
    return false, "Renaming is unavailable until the server net emitter supports text."
end
local function task_id(mode)
    return ({raze = TaskTypes.TASK_RAZE, unraze = TaskTypes.TASK_UNRAZE, annex = TaskTypes.TASK_ANNEX_PUPPET})[mode]
end
return {
    data = function(player, args)
        if args.a == nil then return {mode = "rename", city = -1, name = "No city selected", max_length = 15, enabled = false, reason = "Choose an owned city first."} end
        local c = city(player, args.a)
        local mode = assert(modes[args.b or 0], "unknown city question")
        local ok, why = allowed(player, c, mode)
        local task
        if mode ~= "rename" then
            task = {kind = mode, id = assert(task_id(mode), "city task is unavailable"), explain = mode == "annex" and "Annexing gives you control of production, citizens and buildings. Occupation remains until a Courthouse is built." or nil}
        end
        local done = (mode == "raze" and c:IsRazing()) or (mode == "unraze" and not c:IsRazing()) or (mode == "annex" and not c:IsPuppet())
        return {mode = mode, city = c:GetID(), name = c:GetName(), max_length = 15, task = task, enabled = ok, done = done, reason = not ok and why or nil}
    end,
    actions = {
        rename = function(player, args, net)
            local c = city(player, args.a)
            assert(Players[player]:IsTurnActive() and not c:IsPuppet(), "This city cannot be renamed now.")
            local name = assert(args.text, "A new name is required.")
            name = name:match("^%s*(.-)%s*$")
            local _, chars = name:gsub("[^\128-\191]", "")
            local compact = name:gsub("%s", "")
            local _, visible_chars = compact:gsub("[^\128-\191]", "")
            assert(visible_chars >= 3 and chars <= 15 and not name:find('[%%"<>|/\\*?%[%]%c]'), "Use 3 to 15 characters without reserved symbols.")
            error("Renaming is unavailable until the server net emitter supports text.")
        end,
        do_task = function(player, args, net)
            local c = city(player, args.a)
            local mode
            for _, name in ipairs({"raze", "unraze", "annex"}) do if task_id(name) == args.b then mode = name end end
            assert(mode, "Unknown city task.")
            local ok, why = allowed(player, c, mode)
            assert(ok, why)
            net.send("sendDoTask", c:GetID(), args.b, -1, -1, 0, 0, 0, 0)
        end,
    },
}
