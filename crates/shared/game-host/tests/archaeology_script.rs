use lua_sys::Lua;

#[path = "fixtures/culture_model.rs"]
mod model;

#[test]
fn excavation_choices_revalidate_pending_unit_and_free_slots() {
    let lua = Lua::new().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    lua.exec(include_str!("fixtures/culture.lua"), "=fixture")
        .unwrap();
    lua.exec(r#"
        local p = Players[0]
        local unit = {GetID = function() return 77 end, GetOwner = function() return 0 end,
            GetUnitType = function() return 5 end, GetUnitAIType = function() return 4 end,
            GetX = function() return 2 end, GetY = function() return 3 end}
        GameInfoTypes.UNITAI_ARCHAEOLOGIST = 4
        GameInfoTypes.GREAT_WORK_SLOT_LITERATURE = 2
        GameInfoTypes.GREAT_WORK_SLOT_ART_ARTIFACT = 1
        GameInfo.Units = {[5] = {Type = "UNIT_ARCHAEOLOGIST"}}
        GameInfo.Eras = {[0] = {Description = "Ancient"}}
        local plot = {GetX = function() return 2 end, GetY = function() return 3 end,
            GetArchaeologyArtifactType = function() return written and 6 or 1 end,
            GetArchaeologyArtifactEra = function() return 0 end,
            GetArchaeologyArtifactPlayer1 = function() return 0 end,
            GetArchaeologyArtifactPlayer2 = function() return 1 end,
            GetArchaeologyArtifactWork = function() return 0 end, GetOwner = function() return -1 end}
        p.GetNextDigCompletePlot = function() if dig then return plot end end
        p.Units = function() local once = false; return function() if not once then once = true; return unit end end end
        p.HasAvailableGreatWorkSlot = function() return free end
        p.GetCityOfClosestGreatWorkSlot = function() return p:GetCityByID(7) end
        p.GetBuildingOfClosestGreatWorkSlot = function() return "BUILDING_TEST" end
        p.GetWrittenArtifactCulture = function() return 125 end
        local city = p:GetCityByID(7)
        city.GetX, city.GetY = function() return 0 end, function() return 0 end
        Map = {PlotDistance = function() return 0 end}
        GameInfo.GreatWorks = {[0] = {ID = 0, Type = "GREAT_WORK_TEST", Description = "Writing"}}
        dig, free, written = false, true, false
    "#, "=dig-fixture").unwrap();
    lua.exec(
        &format!(
            "dig_screen = (function()\n{}\nend)()",
            include_str!("../screens/archaeology.lua")
        ),
        "=archaeology",
    )
    .unwrap();
    lua.exec(
        r#"
        local sent = {}
        local net = {send = function(...) sent[#sent + 1] = {...} end}
        assert(dig_screen.data(0, {}).status == "idle")
        dig = true
        local model = dig_screen.data(0, {a = 77})
        assert(model.status == "pending" and #model.choices == 3)
        assert(model.find.participants[2].civ == "")
        assert(not pcall(dig_screen.actions.choose, 0, {a = 2, b = 78}, net))
        free = false
        local locked = dig_screen.data(0, {})
        assert(not locked.choices[1].enabled and #locked.choices[1].reason > 0)
        assert(not pcall(dig_screen.actions.choose, 0, {a = 2, b = 77}, net))
        assert(#sent == 0)
        dig_screen.actions.choose(0, {a = 1, b = 77}, net)
        assert(sent[1][1] == "ArchaeologyChoice" and sent[1][2] == 77 and sent[1][3] == 1)
        written = true
        local writing = dig_screen.data(0, {})
        assert(writing.choices[1].id == 5 and not writing.choices[1].enabled)
        assert(writing.choices[2].id == 4 and writing.choices[2].culture == 125)
        dig_screen.actions.choose(0, {a = 4, b = 77}, net)
        assert(sent[2][3] == 4)
        free = true
        dig_screen.actions.choose(0, {a = 5, b = 77}, net)
        assert(sent[3][3] == 5)
        written = false
        dig_screen.actions.choose(0, {a = 2, b = 77}, net)
        dig_screen.actions.choose(0, {a = 3, b = 77}, net)
        assert(sent[4][3] == 2 and sent[5][3] == 3)
        dig = false
        assert(not pcall(dig_screen.actions.choose, 0, {a = 1, b = 77}, net))
    "#,
        "=dig-assertions",
    )
    .unwrap();
    model::check(&lua, "dig_screen", "archaeology");
}
