//! Engine callbacks to `Events.*`: dirty bits, the entity table, the reconciliation after a load (WO-4, fake game).
mod fake;

use exe_api::{UiEvent, UiEventKind as K};
use fake::{EvExt, Rig, city_block, ev, rig, unit_block, with_block};

fn unit(owner: i32, id: i32, x: i32, y: i32) -> UiEvent {
    // owner, id, x, y, type, culture, civ, colours, flag index, fog, selected, military, visible
    UiEvent::new(K::UnitCreated, unit_block(owner, id, x, y))
}

fn city(owner: i32, id: i32, x: i32, y: i32, pop: i32) -> UiEvent {
    // the city block and the era
    with_block(K::CityCreated, city_block(owner, id, x, y, pop), &[2])
}

fn fog(x: i32, y: i32, ty: i32) -> UiEvent {
    ev(K::FowChanged, &[x, y, ty, 0])
}

const LISTEN: &str = "log = {}
    local function hexs(h) return h.x .. ',' .. h.y end
    Events.SerialEventUnitCreated.Add(function(p, u, hex, ty) table.insert(log, 'UC' .. p .. ':' .. u .. '@' .. hexs(hex) .. 't' .. ty) end)
    Events.SerialEventUnitTeleportedToHex.Add(function(hx, hy, p, u) table.insert(log, 'UT' .. p .. ':' .. u .. '@' .. hx .. ',' .. hy) end)
    Events.SerialEventUnitDestroyed.Add(function(p, u) table.insert(log, 'UD' .. p .. ':' .. u) end)
    Events.SerialEventUnitSetDamage.Add(function(p, u, d, prev) table.insert(log, 'US' .. p .. ':' .. u .. '=' .. d .. '/' .. prev) end)
    Events.SerialEventCityCreated.Add(function(hex, p, c) table.insert(log, 'CC' .. p .. ':' .. c .. '@' .. hexs(hex)) end)
    Events.SerialEventCityDestroyed.Add(function(hex, p, c) table.insert(log, 'CD' .. p .. ':' .. c) end)
    Events.SerialEventCityPopulationChanged.Add(function(p, c, n) table.insert(log, 'CP' .. p .. ':' .. c .. '=' .. n) end)
    Events.HexFOWStateChanged.Add(function(hex, ty, all) table.insert(log, 'F' .. hexs(hex) .. '=' .. ty) end)
    Events.SerialEventGameDataDirty.Add(function() table.insert(log, 'GD') end)
    Events.SerialEventUnitInfoDirty.Add(function() table.insert(log, 'UI') end)
    Events.SerialEventCityInfoDirty.Add(function() table.insert(log, 'CI') end)
    Events.NewGameTurn.Add(function(t) table.insert(log, 'NT' .. t) end)
    Events.ActivePlayerTurnStart.Add(function() table.insert(log, 'TS') end)
    Events.SequenceGameInitComplete.Add(function() table.insert(log, 'INIT') end)";

/// What a rebuild of `world` tells the UI: every known unit and city is destroyed and created again (SIMP-exe-64:
/// the Lua objects of the old load are dead), units first, in the order of the core.
const REBUILD_OF_WORLD: &str = "UD0:10,UD1:11,CD0:5,UC0:10@3,2t7,UC1:11@5,3t7,CC0:5@2,3";

/// A game with a unit, a city and some fog, announced by `setup_graphical`.
fn world(r: &Rig) {
    let mut g = r.game.borrow_mut();
    g.graphical = vec![
        unit(0, 10, 4, 2),
        unit(1, 11, 6, 3),
        city(0, 5, 3, 3, 2),
        // FogOfWarModeTypes: 0 seen, 2 no visibility (Lua: 2 white, 1 grey).
        fog(4, 2, 0),
        fog(6, 3, 2),
    ];
    g.generation = 1;
}

#[test]
fn dirty_bits_coalesce_into_one_event_per_frame() {
    let mut r = rig();
    r.exec(LISTEN);
    // Bits: 11 GameData (twice), 4/6/19 all UnitInfo, 18 CityInfo, 3 has no event.
    for bit in [11, 4, 11, 6, 19, 3, 18] {
        r.game.borrow_mut().events.push(ev(K::SetDirty, &[bit, 1]));
    }
    // A bit set to false is not a request.
    r.game.borrow_mut().events.push(ev(K::SetDirty, &[12, 0]));
    r.exe.dispatch(&r.rt);
    // Once per event, in bit order of first appearance.
    assert_eq!(r.log(), "UI,GD,CI");
    // Nothing left for the next frame.
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "UI,GD,CI");
}

#[test]
fn a_dirty_bit_set_by_a_listener_is_raised_in_the_same_frame() {
    let mut r = rig();
    r.exec(LISTEN);
    r.exec("Events.SerialEventScoreDirty.Add(function() UI.SetDirty(11, true) end) Events.SerialEventScoreDirty()");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "GD");
}

/// Live report 2026-10-31 (no sparkle on a ruin found): the core's `GameplayDoFX` (a notification of the seat with a
/// plot, `NOTIFICATION_GOODY` among them) and a script's `Events.GameplayFX(hx, hy, -1)` reach the exe's particle
/// system, which plays the "look here" effect at the plot; the Lua listeners still hear the event. A notification
/// without a plot plays nothing.
#[test]
fn gameplay_fx_plays_the_look_here_effect_at_the_plot() {
    let mut r = rig();
    r.exec(
        "log = {}
        Events.GameplayFX.Add(function(x, y, t) table.insert(log, 'FX' .. x .. ',' .. y .. '/' .. t) end)",
    );
    {
        let mut g = r.game.borrow_mut();
        // A ruin found at grid (6, 3): hex (5, 3).
        g.events.push(ev(K::DoFx, &[6, 3, -1]));
        // `GameplayDoFX(NULL)`: the production notification.
        g.events.push(ev(K::DoFx, &[-1, -1, -1]));
    }
    r.exe.dispatch(&r.rt);
    assert_eq!(*r.host.fx.borrow(), vec![(6, 3)]);
    assert_eq!(r.log(), "FX5,3/-1,FX0,-1/-1");
    // A script (ActionInfoPanel, NotificationPanel, UnitPanel) fires the event with a hex.
    r.exec("Events.GameplayFX(2, 4, -1)");
    r.exe.dispatch(&r.rt);
    // Hex (2, 4) is grid (4, 4).
    assert_eq!(*r.host.fx.borrow(), vec![(6, 3), (4, 4)]);
    assert_eq!(r.errors(), 0);
}

#[test]
fn unit_and_city_callbacks_become_events_with_hex_vectors() {
    let mut r = rig();
    r.exec(LISTEN);
    {
        let mut g = r.game.borrow_mut();
        g.events.push(unit(0, 10, 4, 2));
        // The unit block of a move holds the plot it ends on, (6, 3): hex (5, 3).
        g.events
            .push(UiEvent::new(K::UnitMoved, unit_block(0, 10, 6, 3)));
        g.events.push(with_block(
            K::UnitSetDamage,
            unit_block(0, 10, 6, 3),
            &[30, 0],
        ));
        g.events.push(city(0, 5, 3, 3, 2));
        g.events.push(with_block(
            K::CityPopulationChanged,
            city_block(0, 5, 3, 3, 3),
            &[3],
        ));
        g.events
            .push(UiEvent::new(K::UnitDestroyed, unit_block(0, 10, 6, 3)));
        g.events.push(with_block(
            K::CityDestroyed,
            city_block(0, 5, 3, 3, 3),
            &[1],
        ));
    }
    r.exe.dispatch(&r.rt);
    // Hex of grid (4, 2) is (3, 2); of (3, 3) is (2, 3); of (6, 3) is (5, 3).
    assert_eq!(
        r.log(),
        "UC0:10@3,2t7,UT0:10@5,3,US0:10=30/0,CC0:5@2,3,CP0:5=3,UD0:10,CD0:5"
    );
    assert_eq!(r.exe.entity_counts(), (0, 0));
    assert_eq!(r.errors(), 0);
}

#[test]
fn the_first_announcement_creates_everything_and_fires_init_once() {
    let mut r = rig();
    r.exec(LISTEN);
    world(&r);
    r.exe.dispatch(&r.rt);
    let log = r.log();
    assert!(log.contains("UC0:10@3,2t7"), "{log}");
    assert!(log.contains("UC1:11@5,3t7"), "{log}");
    assert!(log.contains("CC0:5@2,3"), "{log}");
    assert!(log.contains("F3,2=2") && log.contains("F5,3=1"), "{log}");
    assert!(log.ends_with("GD,UI"), "{log}");
    assert_eq!(r.exe.entity_counts(), (2, 1));
    assert_eq!(r.game.borrow().graphical_calls, 2);
}

#[test]
fn a_rebuild_of_an_unchanged_game_recreates_the_same_entities_and_changes_no_fog() {
    let mut r = rig();
    world(&r);
    r.exe.dispatch(&r.rt);
    r.exec(LISTEN);
    // The core was replaced by the same game (resync, a new snapshot of the same state).
    r.game.borrow_mut().generation = 2;
    r.exe.dispatch(&r.rt);
    // Once for the first world, once for the replacement.
    assert_eq!(r.game.borrow().invalidated, 2);
    // Everything is destroyed and created again at the same plots (the old Lua objects are dead), nothing else:
    // no teleport, no population change, no fog change.
    assert_eq!(r.log(), format!("{REBUILD_OF_WORLD},GD,UI"));
    assert_eq!(r.exe.entity_counts(), (2, 1));
}

#[test]
fn a_rebuild_tells_what_moved_what_vanished_and_what_changed() {
    let mut r = rig();
    world(&r);
    r.exe.dispatch(&r.rt);
    r.exec(LISTEN);
    {
        let mut g = r.game.borrow_mut();
        g.graphical = vec![
            // Unit 10 moved to (7, 4); unit 11 is gone; unit 12 is new.
            unit(0, 10, 7, 4),
            unit(1, 12, 1, 1),
            // The city grew; a second city appeared.
            city(0, 5, 3, 3, 4),
            city(0, 6, 8, 8, 1),
            // One cell changed fog, one did not.
            fog(4, 2, 0),
            fog(6, 3, 0),
        ];
        g.generation = 2;
    }
    r.exe.dispatch(&r.rt);
    let log = r.log();
    // Hex of (7, 4) is (5, 4). Units are re-created, at their new plot (no teleport of a known unit).
    assert!(log.contains("UC0:10@5,4"), "{log}");
    assert!(
        !log.contains("UT0:10"),
        "a re-created unit does not teleport: {log}"
    );
    // Unit 11 is gone: destroyed, not re-created. Unit 12 is new.
    assert!(log.contains("UD1:11"), "{log}");
    assert!(!log.contains("UC1:11"), "{log}");
    assert!(log.contains("UC1:12@1,1"), "{log}");
    // Both cities are created (the grown one again, the new one), with the new state: no population event.
    assert!(log.contains("CC0:5@2,3"), "{log}");
    assert!(log.contains("CC0:6@4,8"), "{log}");
    assert!(!log.contains("CP0:5"), "{log}");
    // The diff events still follow: the changed fog cell is told, the unchanged one is not.
    assert!(log.contains("F5,3=2"), "{log}");
    assert!(
        !log.contains("F3,2"),
        "an unchanged cell is not told: {log}"
    );
    // Old objects die before the new ones appear.
    let died = log.find("UD0:10").expect("the old unit is destroyed");
    let born = log.find("UC0:10").expect("the unit is created again");
    assert!(died < born, "{log}");
    assert_eq!(r.exe.entity_counts(), (2, 2));
}

#[test]
fn a_vanished_city_is_destroyed_after_a_rebuild() {
    let mut r = rig();
    world(&r);
    r.exe.dispatch(&r.rt);
    r.exec(LISTEN);
    {
        let mut g = r.game.borrow_mut();
        g.graphical = vec![unit(0, 10, 4, 2), unit(1, 11, 6, 3)];
        g.generation = 2;
    }
    r.exe.dispatch(&r.rt);
    // Units are re-created; the vanished city is destroyed and not created again.
    assert_eq!(
        r.log(),
        "UD0:10,UD1:11,CD0:5,UC0:10@3,2t7,UC1:11@5,3t7,GD,UI"
    );
    assert_eq!(r.exe.entity_counts(), (2, 0));
}

#[test]
fn the_turn_start_after_a_rebuild_is_the_cores() {
    let mut r = rig();
    // Even a game whose turn moved: the exe API does not make a turn start of its own.
    r.ctx
        .lua()
        .exec("Game = { GetGameTurn = function() return 6 end }", "=game")
        .unwrap();
    world(&r);
    r.exe.dispatch(&r.rt);
    r.exec(LISTEN);
    // A rebuild alone: the entities are re-created and the data refreshed, but no turn event of the exe API's own.
    r.game.borrow_mut().generation = 2;
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), format!("{REBUILD_OF_WORLD},GD,UI"));
    r.exec("log = {}");
    // The new turn's snapshot: the core tells the turn (PUBLISH_NEW_GAME_TURN, ACTIVE_PLAYER_TURN_START) ahead of
    // the rebuild group.
    {
        let mut g = r.game.borrow_mut();
        g.events.push(ev(K::TurnChanged, &[6]));
        g.events.push(ev(K::PublishNewGameTurn, &[6]));
        g.events.push(ev(K::ActivePlayerTurnStart, &[]));
        g.generation = 3;
    }
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), format!("NT6,TS,{REBUILD_OF_WORLD},GD,UI"));
}

#[test]
fn init_complete_is_fired_once_after_the_first_announcement() {
    let mut r = rig();
    // `rig` already ran the first frame; a context loaded later has to ask for it.
    r.exec(LISTEN);
    r.exe.fire_init_complete();
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "INIT");
}

#[test]
fn notifications_are_added_removed_and_rebroadcast() {
    let mut r = rig();
    r.exec(
        "log = {}
         Events.NotificationAdded.Add(function(id, ty, tip, sum, gd, extra, player)
            table.insert(log, 'add' .. id .. ':' .. ty .. ':' .. tip .. ':' .. sum .. ':' .. gd .. ':' .. player) end)
         Events.NotificationRemoved.Add(function(id) table.insert(log, 'rm' .. id) end)",
    );
    {
        let mut g = r.game.borrow_mut();
        g.events.push(
            ev(K::NotificationAdded, &[1, 22, 3, 4, 7, 0, 0])
                .with_text("tip one")
                .with_text2("sum one"),
        );
        g.events.push(
            ev(K::NotificationAdded, &[2, 23, 0, 0, 8, 0, 0])
                .with_text("tip two")
                .with_text2("sum two"),
        );
        g.events.push(ev(K::NotificationRemoved, &[1, 0]));
    }
    r.exe.dispatch(&r.rt);
    assert_eq!(
        r.log(),
        "add1:22:tip one:sum one:7:0,add2:23:tip two:sum two:8:0,rm1"
    );
    // Only the live one is told again.
    r.exec("log = {} UI.RebroadcastNotifications()");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "add2:23:tip two:sum two:8:0");
}

#[test]
fn turn_and_player_callbacks() {
    let mut r = rig();
    r.exec(
        "log = {}
         Events.ActivePlayerTurnStart.Add(function() table.insert(log, 'start') end)
         Events.ActivePlayerTurnEnd.Add(function() table.insert(log, 'end') end)
         Events.GameplaySetActivePlayer.Add(function(a, b) table.insert(log, 'active' .. a .. b) end)
         Events.AIProcessingStartedForPlayer.Add(function(p, tag) table.insert(log, 'ai' .. p .. tag) end)
         Events.EndTurnBlockingChanged.Add(function(a, b) table.insert(log, 'block' .. a .. b) end)",
    );
    {
        let mut g = r.game.borrow_mut();
        g.events.push(ev(K::ActivePlayerTurnEnd, &[]));
        g.events
            .push(ev(K::PlayerTurnStatus, &[0, 3]).with_text("x"));
        g.events.push(ev(K::PlayerTurnStatus, &[2, 3]));
        g.events.push(ev(K::ActivePlayerTurnStart, &[]));
        g.events.push(ev(K::PublishActivePlayer, &[1, 0]));
        g.events.push(ev(K::EndTurnBlockingChanged, &[-1, 4]));
    }
    r.exe.dispatch(&r.rt);
    // `AIProcessingStartedForPlayer` is a local-machine event: it runs at once, ahead of the queued ones.
    assert_eq!(r.log(), "ai3x,end,start,active10,block-14");
}

#[test]
fn camera_callbacks_go_to_the_host_and_unmapped_kinds_are_counted() {
    let mut r = rig();
    r.game.borrow_mut().events.push(ev(K::LookAt, &[9, 8, 0]));
    r.game.borrow_mut().events.push(ev(K::UnitCombat, &[]));
    r.exe.dispatch(&r.rt);
    assert_eq!(*r.host.looked.borrow(), vec![(9, 8)]);
    assert_eq!(r.exe.stats().unmapped, 1);
    assert_eq!(
        r.exe.stats().simp_by_name.get("engine.UnitCombat"),
        Some(&1)
    );
}

#[test]
fn the_game_is_attached_once_and_invalidated_on_a_new_generation() {
    let mut r = rig();
    assert_eq!(r.game.borrow().attached, 1);
    assert_eq!(r.game.borrow().invalidated, 0);
    r.game.borrow_mut().generation = 1;
    r.exe.dispatch(&r.rt);
    assert_eq!(r.game.borrow().invalidated, 1);
    r.exe.dispatch(&r.rt);
    assert_eq!(r.game.borrow().invalidated, 1);
}
