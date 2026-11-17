local classes = {GREAT_WORK_LITERATURE = "writing", GREAT_WORK_ART = "art", GREAT_WORK_MUSIC = "music", GREAT_WORK_ARTIFACT = "artifact"}
local labels = {writing = "Great Work of Writing", art = "Great Work of Art", music = "Great Work of Music", artifact = "Artifact"}

local function location(player, id)
    for city in Players[player]:Cities() do
        for b in GameInfo.Buildings() do
            if b.GreatWorkCount > 0 and city:IsHasBuilding(b.ID) then
                local class = GameInfo.BuildingClasses[b.BuildingClass].ID
                for slot = 0, b.GreatWorkCount - 1 do
                    if city:GetBuildingGreatWork(class, slot) == id then
                        return L(b.Description) .. ", " .. city:GetName()
                    end
                end
            end
        end
    end
end

return {
    data = function(player, args)
        local id = args.a or -1
        -- SIMPLIFIED(SIMP-461): archaeology returns no instance id; its viewer resolves the newest work currently held by this seat.
        if id == -2 then
            local latest=-1
            for city in Players[player]:Cities() do
                for building in GameInfo.Buildings() do
                    if building.GreatWorkCount > 0 and city:IsHasBuilding(building.ID) then
                        local class=GameInfo.BuildingClasses[building.BuildingClass].ID
                        for slot=0,building.GreatWorkCount-1 do
                            latest=math.max(latest,city:GetBuildingGreatWork(class,slot))
                        end
                    end
                end
            end
            assert(latest>=0,"The resulting great work is unavailable.")
            id=latest
        end
        assert(id == math.floor(id) and id >= -1, "A great work id or preview type is required.")
        local place, info
        if id >= 0 then
            place = location(player, id)
            -- A viewer may only inspect an instance in one of the requesting seat's cities.
            assert(place, "This great work is unavailable to your civilization.")
            info = GameInfo.GreatWorks[Game.GetGreatWorkType(id)]
        else
            if args.b == nil then
                for candidate in GameInfo.GreatWorks() do
                    if classes[candidate.GreatWorkClassType] then info = candidate; break end
                end
            else
                assert(type(args.b) == "number" and args.b == math.floor(args.b), "A great work preview type is required.")
                info = GameInfo.GreatWorks[args.b]
            end
        end
        assert(info, "The great work type is unavailable.")
        local kind = assert(classes[info.GreatWorkClassType], "The great work class is unavailable.")
        local creator = id >= 0 and Players[Game.GetGreatWorkCreator(id)] or nil
        local civilization = creator and GameInfo.Civilizations[creator:GetCivilizationType()]
        local image = ""
        local from_dig = info.ArchaeologyOnly == true or info.ArchaeologyOnly == 1
        if info.IconAtlas and info.PortraitIndex then image = "great_work:" .. info.IconAtlas .. ":" .. info.PortraitIndex end
        -- SIMPLIFIED(SIMP-381): full GreatWorks.Image textures need a host resolver. Atlas-backed works render now and others expose the missing-image reason.
        return {id = id, type = info.Type, class = kind, class_name = labels[kind],
            artist = id >= 0 and not from_dig and L(Game.GetGreatWorkArtist(id)) or nil,
            from_dig = from_dig,
            name = id >= 0 and L(Game.GetGreatWorkName(id)) or L(info.Description),
            quote = info.Quote and info.Quote ~= "" and L(info.Quote) or nil,
            civ_name = civilization and L(civilization.ShortDescription) or "",
            era = id >= 0 and L(Game.GetGreatWorkEraShort(id)) or "", image = image,
            image_reason = image == "" and "This work's full-size image is unavailable in this skin." or nil,
            location = place}
    end,
    actions = {},
}
