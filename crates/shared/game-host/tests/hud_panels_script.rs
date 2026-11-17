//! Seat-scoped HUD facts, fractional yields and action validation on getter fixtures.
use lua_sys::{Lua, Table};

fn script() -> Lua {
    let lua = Lua::new().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    lua.exec(include_str!("fixtures/hud_api.lua"), "=hud-api")
        .unwrap();
    let screen: Table = lua.load(include_str!("../screens/hud.lua")).eval().unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua
}

#[test]
fn top_panel_has_fractional_yield_sources_and_resource_accounting() {
    script()
        .exec(
            r#"
        local top = screen.data(3, {}).top
        assert(#top.tips == 7 and top.tips[1].kind == 'science')
        assert(top.tips[1].lines[1].value == 1.25)
        assert(top.tips[2].lines[1].key == 'GetGold')
        assert(top.resources[1].name == 'Iron' and top.resources[1].revealed)
        assert(top.resources[1].imported == 125 and top.resources[1].exported == 125)
        assert(top.trade_used == 125 and top.trade_available == 125)
    "#,
            "=top-panel",
        )
        .unwrap();
}

#[test]
fn end_turn_keeps_native_reason_and_anonymizes_unmet_queue_entries() {
    script()
        .exec(
            r#"
        local turn = screen.data(3, {}).turn
        assert(turn.blocker_kind == 8 and turn.reason == 'A unit needs orders.')
        assert(#turn.waiting_units == 1 and turn.waiting_units[1].key == '3:4')
        assert(#turn.queue == 2)
        local hidden = turn.queue[1]
        assert(not hidden.known and hidden.name == 'Unknown player' and hidden.team == -1)
        hud_busy = true
        assert(#screen.data(3, {}).turn.waiting_units == 0)
    "#,
            "=end-turn",
        )
        .unwrap();
}

#[test]
fn unit_panel_catalog_uses_native_hotkeys_and_revalidates_orders_and_names() {
    script().exec(r#"
        local unit = screen.data(3, {unit=4}).unit
        assert(#unit.catalog == 2 and unit.catalog[1].hotkey == 'KeyA')
          GameInfoActions[0].Type = 'MISSION_FORTIFY'
          assert(screen.data(3, {unit=4}).unit.catalog[1].client == 'fortify')
        local messages = {}
        local net = {send=function(...) messages[#messages+1]={...} end,
            send_text=function(...) messages[#messages+1]={...} end}
        screen.actions.do_action(3, {a=4,b=0,d=1}, net)
        assert(messages[1][1]=='sendPushMission' and messages[1][2]==4 and messages[1][3]==10 and messages[1][7]==1)
        screen.actions.do_action(3, {a=4,b=1,c=1}, net)
        assert(messages[2][1]=='sendDoCommand' and messages[2][6]==1)
        screen.actions.rename(3, {a=4,text='  New Guard  '}, net)
        assert(messages[3][1]=='SendRenameUnit' and messages[3][2]=='New Guard' and messages[3][3]==4)
        for _, text in ipairs({'ab','invalid/name','a very long unit name'}) do
            assert(not pcall(screen.actions.rename,3,{a=4,text=text},net))
        end
        assert(not pcall(screen.actions.rename,1,{a=4,text='New Guard'},net))
        hud_busy = true
        assert(not pcall(screen.actions.do_action,3,{a=4,b=0},net))
        assert(not pcall(screen.actions.rename,3,{a=4,text='New Guard'},net))
        assert(not screen.data(3,{unit=4}).unit.catalog[1].enabled)
        assert(#messages==3)
    "#, "=unit-panel").unwrap();
}

#[test]
fn unit_lists_include_only_owned_units_and_count_all_great_person_modifiers() {
    script()
        .exec(
            r#"
        local lists = screen.data(3, {}).lists
        assert(#lists.units == 1 and lists.units[1].key == '3:4')
        assert(lists.units[1].hp == 90 and lists.units[1].moves == 2)
        assert(#lists.great_people == 1 and lists.great_people[1].key == 'engineer')
        local gp = lists.great_people[1].rows[1]
        assert(gp.city == '3:3' and gp.progress == 40 and gp.threshold == 100)
        assert(gp.per_turn == 12 and gp.turns == 5)
        assert(lists.general_xp == 15 and lists.admiral_xp == 10)
    "#,
            "=unit-lists",
        )
        .unwrap();
}

#[test]
fn minimap_summary_uses_seat_religion_and_omits_client_map_options() {
    script()
        .exec(
            r#"
        local root = screen.data(3, {}).minimap
        assert(not root.observer and root.religion == nil and #root.beliefs == 0)
        assert(root.view == nil and root.options == nil)
        Players[3].GetReligionCreatedByPlayer = function() return 2 end
        Game.GetReligionName = function(id) assert(id==2); return 'Fixture religion' end
        Game.GetBeliefsInReligion = function(id) assert(id==2); return {7} end
        GameInfo.Beliefs = {[7]={ShortDescription='Fixture belief'}}
        root = screen.data(3, {}).minimap
        assert(root.religion=='Fixture religion' and root.beliefs[1].id==7)
    "#,
            "=minimap",
        )
        .unwrap();
}

#[test]
fn on_map_filters_invisible_units_foreign_aircraft_and_cargo() {
    script().exec(r#"
        local own = Players[3]:GetUnitByID(4)
        local hidden, air, cargo = false, false, false
        local foreign = setmetatable({GetOwner=function() return 1 end, GetID=function() return 6 end,
            GetDomainType=function() return air and 1 or 2 end, IsCargo=function() return cargo end,
            IsInvisible=function(_, team) assert(team==3); return hidden end,
            GetPlot=function() return {IsVisible=function(_,team) assert(team==3); return true end} end}, {__index=own})
        Players[1] = setmetatable({GetTeam=function() return 4 end,
            Units=function() local done=false; return function() if not done then done=true; return foreign end end end,
            Cities=function() return function() end end}, {__index=Players[3]})
        local map = screen.data(3, {}).map
        assert(#map.cities==1 and map.cities[1].key=='3:3')
        assert(#map.units==2 and map.units[1].key=='1:6' and map.units[1].moves_is_max)
        assert(map.units[1].owner=='Unknown civilization')
        hidden=true; assert(#screen.data(3, {}).map.units==1)
        hidden=false; air=true; assert(#screen.data(3, {}).map.units==1)
        air=false; cargo=true; assert(#screen.data(3, {}).map.units==1)
    "#, "=on-map").unwrap();
}

#[test]
fn rename_modal_validates_the_target_and_closes_only_after_emitting_the_text_action() {
    let lua = script();
    let modal: Table = lua
        .load(include_str!("../screens/unit_rename.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("modal", modal).unwrap();
    lua.exec(
        r#"
        __screens = {hud=screen}
        assert(not modal.data(3,{}).enabled)
        assert(modal.data(3,{a=4}).enabled)
        assert(not pcall(modal.data,1,{a=4}))
        local sent=0
        local net={send_text=function(name,text,id)
            assert(name=='SendRenameUnit' and text=='New Guard' and id==4); sent=sent+1
        end}
        assert(modal.actions.rename(3,{a=4,text='New Guard'},net).close)
        assert(not pcall(modal.actions.rename,3,{a=4,text='ab'},net))
        assert(sent==1)
    "#,
        "=unit-rename",
    )
    .unwrap();
}

#[test]
fn unit_catalog_exposes_window_actions_only_when_native_applicability_allows() {
    script().exec(r#"
        local unit = Players[3]:GetUnitByID(4)
        unit.CanMakeTradeRoute = function() return true end
        GameInfoActions[2] = {Visible=true, TextKey="Trade", Type="INTERFACEMODE_ESTABLISH_TRADE_ROUTE"}
        GameInfoActions[3] = {Visible=true, TextKey="Home", Type="MISSION_CHANGE_TRADE_UNIT_HOME_CITY", MissionType=10}
        GameInfoActions[4] = {Visible=true, TextKey="Port", Type="MISSION_CHANGE_ADMIRAL_PORT", MissionType=10}
        local rows = screen.data(3, {unit=4}).unit.catalog
        assert(#rows==5 and rows[3].client=='trade_route' and rows[3].enabled)
        assert(rows[4].client=='trade_home' and rows[5].client=='admiral_port')
        unit.CanMakeTradeRoute = function() return false end
        assert(#screen.data(3, {unit=4}).unit.catalog==4)
        hud_busy = true
        for _, row in ipairs(screen.data(3, {unit=4}).unit.catalog) do assert(not row.enabled) end
    "#, "=chooser-actions").unwrap();
}
