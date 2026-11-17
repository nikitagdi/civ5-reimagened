local kinds = {"battle_ranged", "battle_melee", "barbarian_camp", "ancient_ruins", "razed_city", "writing"}
local function color(v)
    return math.floor(v.x * 255 + 0.5) * 16777216 + math.floor(v.y * 255 + 0.5) * 65536 + math.floor(v.z * 255 + 0.5) * 256 + 255
end
local function civilization(player, other)
    if other < 0 or not Players[other] or (other ~= player and not met(player, other)) then
        return {id = other, name = L("TXT_KEY_UNMET_PLAYER"), civ_name = L("TXT_KEY_UNMET_PLAYER"), civ = "", leader = "", color_bg = 0x26221dff, color_fg = 0xebe2d2ff}
    end
    local p = Players[other]
    local info, leader = GameInfo.Civilizations[p:GetCivilizationType()], GameInfo.Leaders[p:GetLeaderType()]
    local bg, fg = p:GetPlayerColors()
    return {id = other, name = L(leader.Description), civ_name = L(info.ShortDescription),
        civ = "civ:" .. info.Type, leader = "leader:" .. leader.Type, color_bg = color(bg), color_fg = color(fg)}
end

local function pending(player, requested)
    local p = Players[player]
    local plot = p:GetNextDigCompletePlot()
    if not plot then return nil end
    local nearest, distance
    for unit in p:Units() do
        if unit:GetUnitAIType() == GameInfoTypes.UNITAI_ARCHAEOLOGIST then
            local d = Map.PlotDistance(unit:GetX(), unit:GetY(), plot:GetX(), plot:GetY())
            if not distance or d < distance then nearest, distance = unit, d end
            if d == 0 then break end
        end
    end
    assert(nearest and nearest:GetOwner() == player, "The completed dig's archaeologist is unavailable.")
    assert(requested == nil or requested == nearest:GetID(), "This excavation is no longer the next pending choice.")
    return plot, nearest
end

local function data(player, args)
    local p = Players[player]
    local root = {status = "idle", reason = "There is no completed excavation awaiting a decision.",
        unit_id = -1, archaeologist = "", near_city = "", player = civilization(player, player), choices = array()}
    local plot, unit = pending(player, args.a)
    if not plot then return root end
    root.status, root.reason, root.unit_id = "pending", nil, unit:GetID()
    root.archaeologist = "unit:" .. GameInfo.Units[unit:GetUnitType()].Type
    local x, y = plot:GetX(), plot:GetY()
    root.plot = {x = x, y = y}
    local closest_distance
    for city in p:Cities() do
        local d = Map.PlotDistance(x, y, city:GetX(), city:GetY())
        if not closest_distance or d < closest_distance then root.near_city, closest_distance = city:GetName(), d end
    end
    local kind = assert(kinds[plot:GetArchaeologyArtifactType()], "The excavation record is unavailable.")
    local era = GameInfo.Eras[plot:GetArchaeologyArtifactEra()]
    local first, second = plot:GetArchaeologyArtifactPlayer1(), plot:GetArchaeologyArtifactPlayer2()
    local participants = array()
    local roles = {battle_ranged = {"attacker", "defender"}, battle_melee = {"attacker", "defender"},
        barbarian_camp = {"plunderer"}, ancient_ruins = {"plunderer"}, razed_city = {"conqueror", "founder"}, writing = {"author"}}
    for index, role in ipairs(roles[kind]) do
        local ref = civilization(player, index == 1 and first or second)
        participants[#participants + 1] = {role = role, role_name = L("TXT_KEY_CHOOSE_ARCH_" .. string.upper(role)),
            civ_name = ref.civ_name, civ = ref.civ, color_bg = ref.color_bg, color_fg = ref.color_fg}
    end
    local found
    if kind == "writing" then
        local info = GameInfo.GreatWorks[plot:GetArchaeologyArtifactWork()]
        if info then found = {name = L(info.Description), type = info.Type, type_id = info.ID,
            creator = info.Artist and L(info.Artist) or "", civ_name = civilization(player, first).civ_name} end
    end
    root.find = {kind = kind, kind_name = L("TXT_KEY_CHOOSE_ARCH_ARTIFACT_TYPE" .. plot:GetArchaeologyArtifactType()),
        era = era and L(era.Description) or "", participants = participants, work = found}
    local slot_type = kind == "writing" and GameInfoTypes.GREAT_WORK_SLOT_LITERATURE or GameInfoTypes.GREAT_WORK_SLOT_ART_ARTIFACT
    local free = p:HasAvailableGreatWorkSlot(slot_type)
    root.no_slot_warning = not free and (kind == "writing" and "No free writing slot." or "No free art or artifact slot.") or nil
    local function add(id, choice_kind, title, description, enabled, reason, owner)
        local ref = owner and civilization(player, owner)
        root.choices[#root.choices + 1] = {id = id, kind = choice_kind, title = title, description = description,
            enabled = enabled, reason = reason, civ_name = ref and ref.civ_name, civ = ref and ref.civ,
            color_bg = ref and ref.color_bg, color_fg = ref and ref.color_fg}
        return root.choices[#root.choices]
    end
    if kind == "writing" then
        add(5, "writing", L("TXT_KEY_CHOOSE_ARCH_WRITING_HEADER", civilization(player, first).civ_name), L("TXT_KEY_CHOOSE_ARCH_WRITTEN_ARTIFACT_RESULT"), free, root.no_slot_warning)
        local amount = p:GetWrittenArtifactCulture()
        local choice = add(4, "culture", L("TXT_KEY_CHOOSE_ARCH_RENAISSANCE_HEADER"), tostring(amount) .. " " .. L("TXT_KEY_CULTURE"), true)
        choice.culture = amount
    else
        local housing = free and p:GetCityOfClosestGreatWorkSlot(x, y, slot_type) or nil
        local building = free and GameInfo.Buildings[p:GetBuildingOfClosestGreatWorkSlot(x, y, slot_type)] or nil
        local destination = housing and building and (L(building.Description) .. ", " .. housing:GetName()) or ""
        add(2, "artifact", L("TXT_KEY_CHOOSE_ARCH_ARTIFACT_HEADER", civilization(player, first).civ_name), destination, free, root.no_slot_warning, first)
        if second >= 0 and second ~= first then add(3, "artifact", L("TXT_KEY_CHOOSE_ARCH_ARTIFACT_HEADER", civilization(player, second).civ_name), destination, free, root.no_slot_warning, second) end
        local landmark = add(1, "landmark", L("TXT_KEY_CHOOSE_ARCH_LANDMARK_HEADER"), L("TXT_KEY_CHOOSE_ARCH_LANDMARK_RESULT"), true)
        local owner = plot:GetOwner()
        if owner >= 0 and owner ~= player and met(player, owner) and (Players[owner]:IsMinorCiv() or not Players[owner]:IsHuman()) then
            landmark.diplomacy = "A landmark here improves relations with " .. civilization(player, owner).civ_name .. "."
        end
    end
    return root
end

return {
    data = data,
    actions = {
        choose = function(player, args, net)
            assert(type(args.a) == "number" and args.a == math.floor(args.a), "An excavation choice is required.")
            assert(type(args.b) == "number" and args.b == math.floor(args.b), "An archaeologist id is required.")
            local model = data(player, {a = args.b})
            for _, choice in ipairs(model.choices) do
                if choice.id == args.a then
                    assert(choice.enabled, choice.reason or "This excavation choice is unavailable.")
                    net.send("ArchaeologyChoice", args.b, args.a)
                    return
                end
            end
            error(model.reason or "This excavation choice is unavailable.")
        end,
    },
}
