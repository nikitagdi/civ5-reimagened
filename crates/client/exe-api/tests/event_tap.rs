//! `ExeApi::set_event_tap`: the observer sees Lua-fired and Rust-fired events of its names, before the listeners.
mod fake;

use std::cell::RefCell;
use std::rc::Rc;

use exe_api::{Arg, ClientEvent};
use fake::rig;

type Seen = Rc<RefCell<Vec<(String, Vec<Arg>)>>>;

fn tap_on(r: &fake::Rig, names: &[&str]) -> Seen {
    let seen: Seen = Rc::default();
    let s = seen.clone();
    r.exe.set_event_tap(
        names,
        Rc::new(move |n, a| s.borrow_mut().push((n.to_owned(), a.to_vec()))),
    );
    seen
}

#[test]
fn a_local_machine_event_fired_from_lua_reaches_the_tap_first_with_converted_values() {
    let r = rig();
    let seen = tap_on(&r, &["SerialEventHexHighlight"]);
    r.exec(
        "log = {}
         Events.SerialEventHexHighlight.Add(function() table.insert(log, 'listener:' .. #_G.tapped) end)
         _G.tapped = {}
         Events.SerialEventHexHighlight({x=3,y=4}, true, {x=0,y=1,z=0,w=0.25}, 'MovementRangeBorder')
         table.insert(log, 'after')",
    );
    let v = seen.borrow();
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].0, "SerialEventHexHighlight");
    assert_eq!(
        v[0].1,
        vec![
            Arg::Vec(vec![3.0, 4.0]),
            Arg::Bool(true),
            Arg::Vec(vec![0.0, 1.0, 0.0, 0.25]),
            Arg::Str("MovementRangeBorder".into()),
        ]
    );
    // The listener ran too (at once).
    assert_eq!(r.log(), "listener:0,after");
}

#[test]
fn other_names_and_an_empty_name_list_are_not_observed() {
    let r = rig();
    let seen = tap_on(&r, &["ClearHexHighlights"]);
    r.exec("Events.SerialEventHexHighlight({x=1,y=1}, true, {x=0,y=0,z=0,w=1}, 's')");
    assert!(seen.borrow().is_empty());
    r.exec("Events.ClearHexHighlights()");
    assert_eq!(seen.borrow().len(), 1);
    let seen = tap_on(&r, &[]);
    r.exec("Events.ClearHexHighlights()");
    assert!(seen.borrow().is_empty());
}

#[test]
fn rust_made_events_and_tables_are_seen() {
    let mut r = rig();
    let seen = tap_on(&r, &["SerialEventMouseOverHex", "SerialEventCityInfoDirty"]);
    r.exe.post(ClientEvent::MouseOverHex { x: 5, y: 2 });
    r.exec("Events.SerialEventCityInfoDirty({a=1, b='s', c=true, d=function() end})");
    r.exe.dispatch(&r.rt);
    let v = seen.borrow();
    assert_eq!(v.len(), 2);
    assert_eq!(v[0].0, "SerialEventMouseOverHex");
    assert_eq!(v[0].1, vec![Arg::Num(5.0), Arg::Num(2.0)]);
    let Arg::Table(fields) = &v[1].1[0] else {
        panic!("a table")
    };
    assert_eq!(fields.len(), 3);
    assert!(fields.contains(&("a".to_owned(), Arg::Num(1.0))));
    assert!(fields.contains(&("c".to_owned(), Arg::Bool(true))));
}
