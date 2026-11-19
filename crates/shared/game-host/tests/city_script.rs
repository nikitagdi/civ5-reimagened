//! City actions are seat-scoped, validated and translated into the DLL message order.
use lua_sys::{Function, Lua, Table};
#[path = "common/city_json.rs"]
mod city_json;

fn lua() -> Lua {
    let lua = Lua::new().unwrap();
    lua.sandbox().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    lua.exec(include_str!("fixtures/city_api.lua"), "=city-api")
        .unwrap();
    let screen: Table = lua
        .load(include_str!("../screens/city.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua
}

#[test]
fn city_actions_validate_owner_turn_and_native_availability() {
    let lua = lua();
    lua.exec(r#"
        local function act(name, a, b, c, d)
            sent = {}
            screen.actions[name](3, {a=a,b=b,c=c,d=d or 7}, {send=function(...) sent[#sent+1]={...} end})
            return sent
        end
        local m = act('push_order', 2, 1, 1)[1]
        assert(m[1]=='sendPushOrder' and m[2]==7 and m[3]==0 and m[4]==2 and m[5]==1 and m[6]==0 and m[7]==0)
        local append = act('push_order', 2, 0, 0)[1]
        assert(append[5]==0 and append[6]==0 and append[7]==1)
        local replace = act('push_order', 2, 2, 0)[1]
        assert(replace[5]==0 and replace[6]==1 and replace[7]==0)
        assert(act('purchase', 2, 1)[1][6]==5)
        local swapped=act('move_order', 2, 0)
        assert(#swapped==2 and swapped[1][3]==1 and swapped[2][3]==0)
        assert(act('pop_order', 1)[1][3]==1)
        assert(act('set_focus', 4)[1][3]==4)
        assert(act('avoid_growth', 1)[1][3]==1)
        assert(act('manual_specialists', 1)[1][6]==1)
        assert(#act('add_specialist', 1, 4)==2)
        assert(#act('remove_specialist', 1, 4)==2)
        assert(#act('unemployed_to_work', 0)==2)
        assert(act('sell_building', 4)[1][1]=='SendSellBuilding')
        assert(act('reset_tiles')[1][4]==0)
        assert(act('toggle_tile', 1)[1][4]==1)
        assert(act('buy_plot', 1, 2)[1][1]=='sendCityBuyPlot')
        for _, bad in ipairs({
            function() act('push_order', 9) end,
            function() act('move_order', 2, 9) end,
            function() act('set_focus', 99) end,
            function() act('avoid_growth', 2) end,
            function() act('sell_building', 9) end,
            function() act('add_specialist', 99, 4) end,
            function() act('buy_plot', 9, 9) end,
            function() act('push_order', 2, 0, 0, 999) end,
        }) do assert(not pcall(bad)) end
        city_turn=false; assert(not pcall(act,'push_order',2)); city_turn=true
        city_puppet=true; assert(not pcall(act,'push_order',2)); city_puppet=false
        assert(not pcall(screen.actions.pop_order, 1, {a=0,d=7}, {send=function() error('foreign emission') end}))
    "#, "=city-actions").unwrap();
}

#[test]
fn city_data_matches_schema_and_keeps_duplicate_orders_separate() {
    let lua = lua();
    let screen: Table = lua.globals().get("screen").unwrap();
    let data: Function = screen.get("data").unwrap();
    let args = lua.create_table();
    args.raw_set("a", 7).unwrap();
    let root: Table = data.call((3, args)).unwrap();
    let queue: Table = root
        .get::<_, Table>("production")
        .unwrap()
        .get("queue")
        .unwrap();
    assert_eq!(queue.raw_len(), 3);
    assert_eq!(
        queue
            .raw_get::<_, Table>(1)
            .unwrap()
            .get::<_, i32>("slot")
            .unwrap(),
        0
    );
    assert_eq!(
        queue
            .raw_get::<_, Table>(2)
            .unwrap()
            .get::<_, i32>("slot")
            .unwrap(),
        1
    );
    assert_eq!(root.get::<_, String>("owner").unwrap(), "Civilization");
    assert_eq!(root.get::<_, Table>("focus_options").unwrap().raw_len(), 9);
    let value = city_json::json(lua_sys::LuaValue::Table(root));
    ui2_model::Sample::parse(
        ui2_model::Schema::v1(),
        &serde_json::json!({"model":{"city":value}}).to_string(),
    )
    .expect("city root matches schema");
}

#[test]
fn city_food_uses_net_growth_and_keeps_gross_food_and_consumption_in_the_breakdown() {
    lua()
        .exec(
            r#"
            city_test_city.FoodDifferenceTimes100 = function() return -875 end
            local root = screen.data(3, {a=7})
            local food = root.yields[1]
            assert(food.key == "food" and food.value == -8.75)
            assert(root.food.net == food.value and root.food.state == "starving")
            assert(food.breakdown[6].label == "Gross food" and food.breakdown[6].value == 9.25)
            assert(food.breakdown[7].label == "Food consumption" and food.breakdown[7].value == -18)
            assert(root.yields[2].key == "production" and root.yields[2].value == 1)
            city_test_city.FoodDifferenceTimes100 = function() return 25 end
            root = screen.data(3, {a=7})
            assert(root.food.state == "growing" and root.yields[1].value == 0.25)
            "#,
            "=net-food",
        )
        .unwrap();
}

/// The great person meter as the original city view computes it (specialists and buildings, modifiers by type), and the
/// demand box with Lekmod's countdown.
#[test]
fn city_great_person_meter_and_demand_follow_the_original_view() {
    let lua = lua();
    lua.exec(
        r#"
        local c, p = city_test_city, city_test_player
        GameInfo.Specialists = (function()
            local spec = {ID=1, Type="SPEC", GreatPeopleUnitClass="UNITCLASS_SCIENTIST", GreatPeopleRateChange=3}
            return setmetatable({[1]=spec, SPEC=spec}, {__call=function() local done=false; return function() if not done then done=true; return spec end end end})
        end)()
        GameInfo.UnitClasses = {UNITCLASS_SCIENTIST = {ID=9, Description="Great Scientist", DefaultUnit="UNIT_TEST"}}
        GameInfo.Buildings[4].GreatPeopleRateChange = 2
        c.GetSpecialistUpgradeThreshold = function() return 100 end
        c.GetSpecialistGreatPersonProgressTimes100 = function() return 4050 end
        c.GetGreatPeopleRateModifier = function() return 25 end
        p.GetGreatPeopleRateModifier = function() return 10 end
        p.GetPolicyGreatPeopleRateModifier = function() return 0 end
        p.GetGreatScientistRateModifier = function() return 15 end
        p.GetPolicyGreatScientistRateModifier = function() return 5 end
        p.GetGoldenAgeTurns = function() return 0 end
        Game.GetNumActiveLeagues = function() return 0 end
        -- Demanded luxury with its countdown.
        GameInfo.Resources = {[6] = {ID=6, Description="Silk"}}
        c.GetResourceDemanded = function() return 6 end
        c.GetResourceDemandedCountdown = function() return 11 end
        c.GetWeLoveTheKingDayCounter = function() return 0 end
        local root = screen.data(3, {a=7})
        local gp = root.great_people[1]
        -- 2 specialists x 3 + a building's 2 = 8, x (1 + (10 + 15 - 5 player, 5 policy, 25 city) / 100) = 8 x 1.5.
        assert(gp.rate == 12 and gp.progress == 40 and gp.turns == 5, tostring(gp.rate) .. " " .. tostring(gp.turns))
        local labels = {}
        for _, m in ipairs(gp.modifiers) do labels[m.label] = m.value end
        assert(labels.TXT_KEY_PLAYER_GP_MOD == 20 and labels.TXT_KEY_POLICY_GP_MOD == 5 and labels.TXT_KEY_CITY_GP_MOD == 25)
        assert(root.demand.kind == 'resource' and root.demand.turns == 11 and root.demand.text == 'TXT_KEY_CITYVIEW_RESOURCE_DEMANDED')
        c.GetWeLoveTheKingDayCounter = function() return 4 end
        root = screen.data(3, {a=7})
        assert(root.demand.kind == 'wltkd' and root.demand.turns == 4 and root.demand.text == 'TXT_KEY_CITYVIEW_WLTKD_COUNTER')
        c.GetResourceDemanded = function() return -1 end
        assert(screen.data(3, {a=7}).demand == nil, 'no demand box without a demand')
    "#,
        "=city-gp",
    )
    .unwrap();
}

#[test]
fn city_list_only_uses_the_requesting_seat() {
    let lua = lua();
    let screen: Table = lua
        .load(include_str!("../screens/city_list.lua"))
        .eval()
        .unwrap();
    let data: Function = screen.get("data").unwrap();
    let root: Table = data.call((3, lua.create_table())).unwrap();
    assert_eq!(root.get::<_, i32>("count").unwrap(), 1);
    let cities: Table = root.get("cities").unwrap();
    assert_eq!(
        cities
            .raw_get::<_, Table>(1)
            .unwrap()
            .get::<_, String>("map_city")
            .unwrap(),
        "3:7"
    );
}
