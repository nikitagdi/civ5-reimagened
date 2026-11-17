//! Award content is read-only and argument-scoped.
use lua_sys::Lua;
#[test]
fn award_uses_requested_technology_and_omits_invalid_content() {
    let lua = Lua::new().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    lua.exec(include_str!("fixtures/techs_api.lua"), "=fixture")
        .unwrap();
    lua.exec(
        &format!(
            "screen = (function() {} end)()",
            include_str!("../screens/tech_award.lua")
        ),
        "=award",
    )
    .unwrap();
    lua.exec(
        r#"
        GameInfo.Technologies[1].Quote = "Knowledge.[NEWLINE]-- A scholar"
        local d = screen.data(3, {a=1})
        assert(d.tech.name == "Pottery" and d.turn == 1 and d.help == "Granary")
        assert(d.quote == "Knowledge." and d.quote_by == "A scholar")
        assert(d.leads_to[1].name == "Writing")
        assert(next(screen.data(3, {a=999})) == nil)
        assert(next(screen.data(3, {})) == nil and next(screen.actions) == nil)
    "#,
        "=assertions",
    )
    .unwrap();
}
