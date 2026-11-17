//! Tooltip requests expose no mutations and reject foreign city focus.
use lua_sys::Lua;
#[test]
fn tooltip_technology_is_seat_scoped_and_invalid_objects_are_empty() {
    let lua = Lua::new().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    lua.exec(include_str!("fixtures/techs_api.lua"), "=fixture")
        .unwrap();
    lua.exec(
        &format!(
            "screen = (function() {} end)()",
            include_str!("../screens/rich_tooltips.lua")
        ),
        "=tooltip",
    )
    .unwrap();
    lua.exec(
        r#"
        local root = screen.data(3, {kind=0,id=1})
        assert(root.tech.name == "Pottery" and root.tech.cost == 50)
        assert(root.tech.leads_to[1].name == "Writing")
        assert(next(screen.data(3, {kind=0,id=999})) == nil)
        assert(next(screen.actions) == nil)
        assert(not pcall(screen.data, 3, {kind=0,id=1,city=999}))
        assert(screen.data(3, {kind=10,id=1}).unavailable_reason)
        assert(not pcall(screen.data, 3, {kind=0,id=-1}))
    "#,
        "=assertions",
    )
    .unwrap();
}
