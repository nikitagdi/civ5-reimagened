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
        local unlocks = by_unlock_key(root.tech.unlocks)
        assert(#root.tech.unlocks == 16 and unlocks.RESOURCE_IRON.name == "Reveals Iron")
        assert(unlocks["ability:BridgeBuilding"].kind == "ability")
        assert(unlocks["ability:AllowsEmbarking"].icon == "icon:GENERIC_FUNC_ATLAS:0")
        assert(unlocks["ability:InternationalTradeRoutesChange"].idx == nil)
        assert(next(screen.data(3, {kind=0,id=999})) == nil)
        assert(next(screen.actions) == nil)
        assert(not pcall(screen.data, 3, {kind=0,id=1,city=999}))
        assert(screen.data(3, {kind=10,id=1}).unavailable_reason)
        assert(not pcall(screen.data, 3, {kind=0,id=-1}))
        local function help(slot)
            local u = assert(screen.data(3, {kind=12,id=1,slot=slot}).unlock)
            assert(u.tech == 1 and u.slot == slot)
            local lines, icons = {}, {}
            for _, line in ipairs(u.lines) do
                local text = ""
                for _, part in ipairs(line.parts) do
                    text = text .. part.text
                    if part.icon then icons[part.icon] = true end
                end
                lines[#lines + 1] = text
            end
            return u.title, table.concat(lines, "|"), icons
        end
        local name, text, icons = help(0)
        assert(name == "Unique unit" and icons["font:ICON_PRODUCTION"] and icons["font:ICON_FOOD"])
        assert(text:find("Cost: 25") and text:find("Moves: 2") and text:find("Strength: 7"))
        assert(text:find("Promotion help") and text:find("A guard.| Rations", 1, true))
        name, text, icons = help(1)
        assert(name == "Wonder" and icons["font:ICON_GOLD"] and icons["font:ICON_FOOD"])
        assert(text:find("Cost: 60") and text:find("Maintenance: 2") and text:find("Food: +3", 1, true))
        assert(text:find("Wonder effect.", 1, true))
        name, text = help(2)
        assert(name == "Iron" and text == "Reveals Iron")
        name, text = help(3)
        assert(name == "Project" and text:find("Cost: 75") and text:find("Project effect.", 1, true))
        assert(text:find("Requires a factory.", 1, true))
        name, text, icons = help(4)
        assert(name == "Build farm" and text:find("Improvement effect.", 1, true) and icons["font:ICON_FOOD"])
        name, text = help(5)
        assert(name == "Wealth" and text:find("Converts production to gold.", 1, true))
        name, text = help(6)
        assert(name == "" and text == "Faster movement on Road")
        assert(next(screen.data(3, {kind=12,id=1,slot=999})) == nil)
        assert(next(screen.data(3, {kind=12,id=999,slot=0})) == nil)
    "#,
        "=assertions",
    )
    .unwrap();
}
