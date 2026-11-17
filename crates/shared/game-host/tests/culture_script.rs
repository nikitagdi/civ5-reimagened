//! Culture scripts also run against a small, explicit Lua game fixture on stub builds.
use lua_sys::Lua;

#[path = "fixtures/culture_model.rs"]
mod model;

#[test]
fn culture_validates_moves_offers_and_swaps_for_the_requesting_seat() {
    let lua = Lua::new().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    lua.exec(include_str!("fixtures/culture.lua"), "=culture-fixture")
        .unwrap();
    let source = format!(
        "culture = (function()\n{}\nend)()",
        include_str!("../screens/culture.lua")
    );
    lua.exec(&source, "=culture").unwrap();
    lua.exec(
        r#"
        local model = culture.data(0, {})
        assert(model.player.name == "Leader")
        assert(model.summary.works == 1 and model.summary.slots == 2)
        assert(#model.cities == 1 and #model.influence.sources == 1)
        assert(model.influence.sources[1].rows[1].known == false)
        assert(model.influence.sources[1].rows[1].name == nil)
        local first, second = model.cities[1].cells[1].slots[1].id, model.cities[1].cells[1].slots[2].id
        local again = culture.data(0, {})
        assert(again.cities[1].cells[1].slots[1].id == first)
        local sent = {}
        local net = {send = function(...) sent[#sent + 1] = {...} end}
        culture.actions.move(0, {a = first, b = second}, net)
        local message = sent[1]
        assert(message[1] == "sendMoveGreatWorks")
        assert(message[2] == 7 and message[3] == 0 and message[4] == 0)
        assert(message[5] == 7 and message[6] == 0 and message[7] == 1)
        culture.actions.offer(0, {a = 1, b = 10}, net)
        assert(sent[2][1] == "sendSetSwappableGreatWork" and sent[2][3] == 10)
        culture.actions.offer(0, {a = 1, b = -1}, net)
        local function refused(action, player, args)
            local before = #sent
            local ok, reason = pcall(culture.actions[action], player, args, net)
            assert(not ok and #tostring(reason) > 0 and #sent == before)
        end
        refused("move", 1, {a = first, b = second})
        refused("move", 0, {a = first, b = first})
        refused("move", 0, {a = second, b = first})
        refused("offer", 0, {a = 1, b = 20})
        refused("offer", 0, {a = 4, b = -1})
        refused("offer", 0, {a = 1.5, b = -1})
        refused("swap", 0, {a = 1, b = 20, c = 10})
        known = true
        culture.actions.swap(0, {a = 1, b = 20, c = 10}, net)
        assert(sent[4][1] == "sendSwapGreatWorks")
        assert(sent[4][2] == 10 and sent[4][3] == 1 and sent[4][4] == 20)
        refused("swap", 0, {a = 1, b = 21, c = 10})
        refused("swap", 0, {a = 1, b = 20, c = 11})
        war = true
        refused("swap", 0, {a = 1, b = 20, c = 10})
        building_present = false
        refused("move", 0, {a = first, b = second})
        "#,
        "=culture-assertions",
    ).unwrap();
    model::check(&lua, "culture", "culture");
}
