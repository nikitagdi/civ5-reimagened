use lua_sys::Lua;

#[path = "fixtures/culture_model.rs"]
mod model;

#[test]
fn great_work_preview_and_owned_instance_hide_foreign_locations() {
    let lua = Lua::new().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    lua.exec(include_str!("fixtures/culture.lua"), "=fixture")
        .unwrap();
    lua.exec(r#"
        GameInfo.Buildings.BUILDING_TEST.BuildingClass = "BUILDINGCLASS_TEST"
        GameInfo.Buildings = setmetatable(GameInfo.Buildings, {__call = function(self)
            local once = false
            return function() if not once then once = true; return self.BUILDING_TEST end end
        end})
        GameInfo.GreatWorks = setmetatable({[0] = {Type = "GREAT_WORK_TEST", Description = "Painting", GreatWorkClassType = "GREAT_WORK_ART", ArchaeologyOnly = 0}}, {__call = function(self)
            local once = false
            return function() if not once then once = true; return self[0] end end
        end})
        Game.GetGreatWorkType = function() return 0 end
    "#, "=viewer-fixture").unwrap();
    lua.exec(
        &format!(
            "viewer = (function()\n{}\nend)()",
            include_str!("../screens/great_work.lua")
        ),
        "=viewer",
    )
    .unwrap();
    lua.exec(
        r#"
        local preview = viewer.data(0, {})
        assert(preview.id == -1 and preview.artist == nil and preview.location == nil)
        assert(preview.image == "" and #preview.image_reason > 0)
        local own = viewer.data(0, {a = 10})
        assert(own.artist == "Artist" and own.location == "Gallery, City")
        assert(viewer.data(0, {a = -2}).id == 10)
        assert(not pcall(viewer.data, 1, {a = 10}))
        assert(not pcall(viewer.data, 0, {a = -1, b = 9999}))
        GameInfo.GreatWorks[0].ArchaeologyOnly = 1
        assert(viewer.data(0, {a = 10}).artist == nil)
        assert(viewer.data(0, {a = 10}).from_dig == true)
    "#,
        "=viewer-assertions",
    )
    .unwrap();
    model::check(&lua, "viewer", "great_work");
}
