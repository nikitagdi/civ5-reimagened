//! Events, FIFO, containers, listener errors (WO-4, fake game).
mod fake;

use exe_api::{ClientEvent, Family, UiEventKind as K};
use fake::{EvExt, ev, rig};

#[test]
fn listeners_run_in_registration_order_and_the_fifo_in_order_of_firing() {
    let mut r = rig();
    r.exec(
        "log = {}
         Events.SerialEventCityInfoDirty.Add(function() table.insert(log, 'a1') end)
         Events.SerialEventCityInfoDirty.Add(function() table.insert(log, 'a2') end)
         Events.SerialEventGameDataDirty.Add(function() table.insert(log, 'b') end)
         Events.SerialEventCityInfoDirty()
         Events.SerialEventGameDataDirty()
         table.insert(log, 'fired')",
    );
    // Serial events wait for the frame.
    assert_eq!(r.log(), "fired");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "fired,a1,a2,b");
    assert_eq!(r.errors(), 0);
}

#[test]
fn an_event_fired_by_a_listener_goes_to_the_end_of_the_same_pass() {
    let mut r = rig();
    r.exec(
        "log = {}
         Events.SerialEventCityInfoDirty.Add(function() table.insert(log, 'first'); Events.SerialEventScoreDirty() end)
         Events.SerialEventGameDataDirty.Add(function() table.insert(log, 'second') end)
         Events.SerialEventScoreDirty.Add(function() table.insert(log, 'nested') end)
         Events.SerialEventCityInfoDirty()
         Events.SerialEventGameDataDirty()",
    );
    r.exe.dispatch(&r.rt);
    // `nested` was fired while `first` ran: after `second`, which was already waiting, in the same pass.
    assert_eq!(r.log(), "first,second,nested");
    assert_eq!(r.exe.stats().queued, 0);
}

#[test]
fn local_machine_events_run_at_once() {
    let mut r = rig();
    r.exec(
        "log = {}
         Events.SerialEventMouseOverHex.Add(function(x, y) table.insert(log, 'over' .. x .. ':' .. y) end)
         Events.SerialEventMouseOverHex(3, 4)
         table.insert(log, 'after')",
    );
    assert_eq!(r.log(), "over3:4,after");
    // The client's own event is local too; its arguments are the grid position of the plot (scripts give them to Map.GetPlot).
    r.exe.post(ClientEvent::MouseOverHex { x: 5, y: 2 });
    assert_eq!(r.log(), "over3:4,after,over5:2");
}

#[test]
fn call_immediate_runs_a_serial_event_at_once() {
    let r = rig();
    r.exec(
        "log = {}
         Events.SerialEventScoreDirty.Add(function(a) table.insert(log, 'x' .. a) end)
         Events.SerialEventScoreDirty.CallImmediate(7)
         table.insert(log, 'after')",
    );
    assert_eq!(r.log(), "x7,after");
}

#[test]
fn a_failing_listener_does_not_stop_the_others() {
    let mut r = rig();
    r.exec(
        "log = {}
         Events.SerialEventScoreDirty.Add(function() error('boom') end)
         Events.SerialEventScoreDirty.Add(function() table.insert(log, 'still') end)
         Events.SerialEventScoreDirty()",
    );
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "still");
    assert_eq!(r.errors(), 1);
    let rec = r.rt.recent_errors();
    assert!(rec[0].error.message.contains("boom"), "{:?}", rec[0].error);
}

#[test]
fn events_are_callable_values_with_the_given_arguments() {
    let mut r = rig();
    r.exec(
        "log = {}
         Events.SerialEventUnitCreated.Add(function(p, u, hex, a, b, c, d, e, f, g, h, i, j)
            table.insert(log, p .. '/' .. u .. '/' .. hex.x .. ',' .. hex.y .. '/' .. tostring(j)) end)
         Events.SerialEventUnitCreated(1, 2, {x = 3, y = 4}, 0, 0, 0, 0, 0, 0, 0, false, true, true)
         local f = Events.SerialEventUnitCreated
         f(5, 6, {x = 7, y = 8})
         Events['SerialEventUnitCreated'](9, 10, {x = 1, y = 1})",
    );
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "1/2/3,4/true,5/6/7,8/nil,9/10/1,1/nil");
}

#[test]
fn remove_by_key_by_function_and_all() {
    let mut r = rig();
    r.exec(
        "log = {}
         local function f() table.insert(log, 'f') end
         Events.SerialEventScoreDirty.Add(f)
         Events.SerialEventScoreDirty.Add(function() table.insert(log, 'k') end, 'mykey')
         Events.SerialEventScoreDirty.Add(function() table.insert(log, 'g') end)
         count_before = Events.SerialEventScoreDirty.Count()
         Events.SerialEventScoreDirty.Remove('mykey')
         Events.SerialEventScoreDirty.Remove(f)
         count_after = Events.SerialEventScoreDirty.Count()
         Events.SerialEventScoreDirty()",
    );
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "g");
    assert_eq!(
        r.eval_str("return count_before .. '>' .. count_after"),
        "3>1"
    );
    r.exec("Events.SerialEventScoreDirty.RemoveAll()");
    assert_eq!(
        r.exe
            .listener_count(Family::Events, "SerialEventScoreDirty"),
        0
    );
    // A key replaces the listener under it.
    r.exec(
        "log = {}
         Events.SerialEventScoreDirty.Add(function() table.insert(log, 'one') end, 'k')
         Events.SerialEventScoreDirty.Add(function() table.insert(log, 'two') end, 'k')
         Events.SerialEventScoreDirty.CallImmediate()",
    );
    assert_eq!(r.log(), "two");
}

#[test]
fn a_listener_removed_while_the_event_runs_is_skipped() {
    let mut r = rig();
    r.exec(
        "log = {}
         local second
         Events.SerialEventScoreDirty.Add(function() table.insert(log, 'a'); Events.SerialEventScoreDirty.Remove(second) end)
         second = function() table.insert(log, 'b') end
         Events.SerialEventScoreDirty.Add(second)
         Events.SerialEventScoreDirty()",
    );
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "a");
}

#[test]
fn lua_events_are_made_by_name_and_synchronous() {
    let r = rig();
    r.exec(
        "log = {}
         LuaEvents.AnythingAtAll.Add(function(a, b) table.insert(log, a .. b) end)
         LuaEvents.AnythingAtAll('x', 'y')
         table.insert(log, 'after')",
    );
    assert_eq!(r.log(), "xy,after");
}

#[test]
fn game_events_subscribe_and_arrive_through_the_fifo() {
    let mut r = rig();
    r.exec(
        "log = {}
         GameEvents.PlayerAdoptPolicy.Add(function(p, pol) table.insert(log, 'p' .. p .. 'pol' .. pol) end)",
    );
    assert_eq!(
        r.game.borrow().subscribed,
        vec!["PlayerAdoptPolicy".to_owned()]
    );
    r.game.borrow_mut().events.push(
        ev(K::GameEvent, &[])
            .with_text("PlayerAdoptPolicy")
            .with_values(vec![exe_api::UiValue::Num(2.0), exe_api::UiValue::Num(9.0)]),
    );
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "p2pol9");
}

#[test]
fn lua_events_of_the_core_become_events() {
    let mut r = rig();
    r.exec(
        "log = {}
         Events.SerialEventHexHighlight.Add(function(v, on) table.insert(log, v.x .. ',' .. v.y .. ':' .. tostring(on)) end)
         Events.GameplayAlertMessage.Add(function(t) table.insert(log, t) end)",
    );
    r.game.borrow_mut().events.push(
        ev(K::LuaEvent, &[])
            .with_text("SerialEventHexHighlight")
            .with_values(vec![
                exe_api::UiValue::Vec(vec![4.0, 5.0]),
                exe_api::UiValue::Bool(true),
            ]),
    );
    r.game.borrow_mut().events.push(
        ev(K::LuaEvent, &[])
            .with_text("GameplayAlertMessage")
            .with_values(vec![exe_api::UiValue::Str("hello".into())]),
    );
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "4,5:true,hello");
}

#[test]
fn unknown_events_names_still_work_and_are_counted() {
    let mut r = rig();
    r.exec("log = {}\nEvents.NotAnExeEvent.Add(function() table.insert(log, 'hit') end)\nEvents.NotAnExeEvent()");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "hit");
    assert_eq!(
        r.exe.stats().simp_by_name.get("Events.NotAnExeEvent"),
        Some(&1)
    );
}

#[test]
fn the_event_table_has_the_exe_names_and_containers() {
    use exe_api::event_table::{Container, all_names, container_of};
    assert!(all_names().len() > 250);
    assert_eq!(
        container_of("SerialEventUnitCreated"),
        Some(Container::Serial)
    );
    assert_eq!(
        container_of("SerialEventMouseOverHex"),
        Some(Container::Immediate)
    );
    assert_eq!(
        container_of("SerialEventHexHighlight"),
        Some(Container::Immediate)
    );
    assert_eq!(container_of("NotAnExeEvent"), None);
}
