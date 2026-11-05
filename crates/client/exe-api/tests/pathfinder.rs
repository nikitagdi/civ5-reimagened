//! The interface path finder of the exe: `UI.SendPathfinderUpdate` -> `Events.UIPathFinderUpdate`, the movement range of
//! `Events.ShowMovementRange`, the drawn path (`Events.DisplayMovementIndicator`), and `SerialEventMouseOverHex` once per plot.

mod fake;

use exe_api::ClientEvent;
use fake::{FakeGame, node, rig_with_game};

/// Player 0's unit 3 stands on (5, 5) and is selected.
fn game() -> FakeGame {
    FakeGame {
        attach_script: "Players = { [0] = { GetUnitByID = function(self, id)
             return { GetX = function() return 5 end, GetY = function() return 5 end,
                      GetOwner = function() return 0 end, GetID = function() return id end }
           end } }"
            .into(),
        head_unit: Some((0, 3)),
        ..FakeGame::default()
    }
}

#[test]
fn the_path_to_the_mouse_goes_to_the_lua_and_to_the_drawing() {
    let mut r = rig_with_game(Default::default(), game());
    r.game.borrow_mut().path = vec![node(5, 5, 0, 0), node(6, 5, 0, 0), node(7, 5, 1, 0)];
    r.exec(
        "log = {}
         Events.UIPathFinderUpdate.Add(function(d)
           for _, n in ipairs(d) do table.insert(log, n.x .. ':' .. n.y .. ':' .. n.turn) end
         end)",
    );
    r.exe.post(ClientEvent::MouseOverHex { x: 7, y: 5 });
    r.exe.dispatch(&r.rt);
    r.exec("UI.SendPathfinderUpdate() Events.DisplayMovementIndicator(true)");
    r.exe.dispatch(&r.rt);
    // The turn of the Lua counts from 1 (PathHelpManager.lua starts at 1).
    assert_eq!(r.log(), "5:5:1,6:5:1,7:5:2");
    assert_eq!(r.game.borrow().path_asked, [(0, 3, 7, 5)]);
    assert_eq!(r.exe.movement_path().len(), 3);
    r.exec("Events.DisplayMovementIndicator(false)");
    r.exe.dispatch(&r.rt);
    assert!(r.exe.movement_path().is_empty(), "the indicator is off");
    // A target the unit cannot reach: no path for the Lua, the inaccessible marker for the drawing.
    r.game.borrow_mut().path = vec![node(9, 9, 0, 0x80)];
    r.exec("log = {} UI.SendPathfinderUpdate() Events.DisplayMovementIndicator(true)");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "");
    assert_eq!(r.exe.movement_path().len(), 1);
    assert_eq!(r.errors(), 0);
}

/// Live report 2026-10-31 ("the path goes over a mountain"): the client lays no path of its own. The drawn path and the
/// Lua's nodes are the core's interface path finder's answer node by node (here around a mountain at (6, 5)), asked for
/// the head selected unit and the plot under the mouse, as the exe's TEMPCalculatePathFinderUpdates.
#[test]
fn the_drawn_path_is_the_cores_answer_node_by_node() {
    let mut r = rig_with_game(Default::default(), game());
    let around = vec![
        node(5, 5, 0, 0),
        node(5, 6, 0, 0),
        node(6, 6, 0, 0),
        node(7, 5, 1, 0),
    ];
    r.game.borrow_mut().path = around.clone();
    r.exec(
        "log = {}
         Events.UIPathFinderUpdate.Add(function(d)
           for _, n in ipairs(d) do table.insert(log, n.x .. ':' .. n.y) end
         end)",
    );
    r.exe.post(ClientEvent::MouseOverHex { x: 7, y: 5 });
    r.exe.dispatch(&r.rt);
    r.exec("UI.SendPathfinderUpdate() Events.DisplayMovementIndicator(true)");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "5:5,5:6,6:6,7:5");
    assert_eq!(r.exe.movement_path(), around);
    assert!(!r.exe.movement_path().iter().any(|n| (n.x, n.y) == (6, 5)));
    assert_eq!(r.game.borrow().path_asked, [(0, 3, 7, 5)]);
}

/// The trade route paths come from the core (`Q_TRADE_PATHS`) as it answers them.
#[test]
fn trade_paths_are_the_cores() {
    let r = rig_with_game(Default::default(), game());
    let path = exe_api::TradePath {
        id: 9,
        plots: vec![(3, 3), (4, 4), (5, 4), (6, 3)],
    };
    r.game.borrow_mut().trade = vec![path.clone()];
    assert_eq!(r.exe.trade_paths(-1), vec![path]);
    assert_eq!(r.game.borrow().trade_asked, [-1]);
}

#[test]
fn show_movement_range_lists_the_unit_plot_and_the_reach() {
    let mut r = rig_with_game(Default::default(), game());
    r.game.borrow_mut().reach = vec![node(6, 5, 0, 0), node(7, 5, 0, 1)];
    r.exec(
        "log = {}
         Events.ClearUnitMoveHexRange.Add(function() table.insert(log, 'clear') end)
         Events.StartUnitMoveHexRange.Add(function() table.insert(log, 'start') end)
         Events.AddUnitMoveHexRangeHex.Add(function(i, j, k, a) table.insert(log, i .. ':' .. j .. ':' .. tostring(a)) end)
         Events.EndUnitMoveHexRange.Add(function() table.insert(log, 'end') end)
         Events.ShowMovementRange(0, 3)",
    );
    r.exe.dispatch(&r.rt);
    let hex = |x, y| {
        let (i, j) = exe_api::hex::grid_to_hex(x, y);
        format!("{i}:{j}")
    };
    assert_eq!(
        r.log(),
        format!(
            "clear,start,{}:false,{}:false,{}:true,end",
            hex(5, 5),
            hex(6, 5),
            hex(7, 5)
        )
    );
    assert_eq!(r.errors(), 0);
}

/// Ranged attack mode: only the plots the unit can shoot at, each an attack plot (the unit's own plot is not in it).
#[test]
fn show_attack_targets_lists_the_shot_plots_as_attack_plots() {
    let mut r = rig_with_game(Default::default(), game());
    r.game.borrow_mut().targets = vec![(7, 5), (8, 6)];
    r.exec(
        "log = {}
         Events.ClearUnitMoveHexRange.Add(function() table.insert(log, 'clear') end)
         Events.AddUnitMoveHexRangeHex.Add(function(i, j, k, a, u) table.insert(log, i .. ':' .. j .. ':' .. tostring(a) .. ':' .. u) end)
         Events.EndUnitMoveHexRange.Add(function() table.insert(log, 'end') end)
         Events.ShowAttackTargets(0, 3)",
    );
    r.exe.dispatch(&r.rt);
    let hex = |x, y| {
        let (i, j) = exe_api::hex::grid_to_hex(x, y);
        format!("{i}:{j}")
    };
    assert_eq!(
        r.log(),
        format!("clear,{}:true:3,{}:true:3,end", hex(7, 5), hex(8, 6))
    );
    assert_eq!(r.errors(), 0);
}

/// The exe cuts a path at 64 nodes and gives an air unit none.
#[test]
fn a_path_is_cut_at_64_nodes_and_an_air_unit_has_none() {
    assert_eq!(exe_api::ui_api::PATH_NODES_MAX, 64);
    let long: Vec<_> = (0..90).map(|i| node(i, 5, i / 10, 0)).collect();
    let cut = exe_api::ui_api::cap_path(long.clone());
    assert_eq!(cut, long[..64]);

    let mut r = rig_with_game(Default::default(), game());
    r.game.borrow_mut().path = long;
    r.exec(
        "log = {}
         Events.UIPathFinderUpdate.Add(function(d) table.insert(log, #d) end)",
    );
    r.exe.post(ClientEvent::MouseOverHex { x: 7, y: 5 });
    r.exe.dispatch(&r.rt);
    r.exec("UI.SendPathfinderUpdate() Events.DisplayMovementIndicator(true)");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "64");
    assert_eq!(r.exe.movement_path().len(), 64);

    // An air unit: no path.
    let mut g = game();
    g.attach_script = "DomainTypes = { DOMAIN_AIR = 2 }
         Players = { [0] = { GetUnitByID = function(self, id)
           return { GetDomainType = function() return 2 end, GetX = function() return 5 end,
                    GetY = function() return 5 end, GetOwner = function() return 0 end,
                    GetID = function() return id end }
         end } }"
        .into();
    let mut r = rig_with_game(Default::default(), g);
    r.game.borrow_mut().path = vec![node(5, 5, 0, 0), node(6, 5, 0, 0)];
    r.exec("log = {} Events.UIPathFinderUpdate.Add(function(d) table.insert(log, #d) end)");
    r.exe.post(ClientEvent::MouseOverHex { x: 7, y: 5 });
    r.exe.dispatch(&r.rt);
    r.exec("UI.SendPathfinderUpdate() Events.DisplayMovementIndicator(true)");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "0");
    assert!(r.exe.movement_path().is_empty());
    assert_eq!(r.errors(), 0);
}

/// A new notification plays its sound once (by its type name); a known id or a re-announced one does not.
#[test]
fn a_new_notification_asks_for_its_sound() {
    use exe_api::UiEventKind;
    use fake::{EvExt, ev};
    let mut g = game();
    g.attach_script
        .push_str(" NotificationTypes = { NOTIFICATION_GOODY = 7 }");
    let mut r = rig_with_game(Default::default(), g);
    let note = |id| {
        ev(UiEventKind::NotificationAdded, &[id, 7, 1, 1, 0, 0, 0])
            .with_text("Ruins")
            .with_text2("Ruins")
    };
    r.game.borrow_mut().events.push(note(1));
    r.exe.dispatch(&r.rt);
    r.game.borrow_mut().events.push(note(1));
    r.game.borrow_mut().events.push(note(2).rebuilt());
    r.exe.dispatch(&r.rt);
    assert_eq!(*r.host.notification_sounds.borrow(), ["NOTIFICATION_GOODY"]);
}

#[test]
fn mouse_over_hex_fires_once_per_plot() {
    let mut r = rig_with_game(Default::default(), game());
    r.exec("log = {} Events.SerialEventMouseOverHex.Add(function(x, y) table.insert(log, x .. ',' .. y) end)");
    for (x, y) in [(1, 1), (1, 1), (2, 1), (2, 1), (1, 1)] {
        r.exe.post(ClientEvent::MouseOverHex { x, y });
    }
    r.exe.dispatch(&r.rt);
    assert_eq!(
        r.log().split(',').count(),
        6,
        "three plot changes: {}",
        r.log()
    );
    assert_eq!(
        r.eval_str("local x, y = UI.GetMouseOverHex() return x .. ',' .. y"),
        "1,1"
    );
}

/// The popup route of the trade visuals: the core's temporary route (9 plots of the path at most), the plot the UI
/// asked it for (remembered from `Game.SelectedUnit_SpeculativePopupTradeRoute_Display`), and the routes by index.
#[test]
fn the_popup_route_follows_the_cores_trade_visuals_events() {
    use exe_api::{PopupRoute, UiEventKind};
    use fake::ev;
    let mut g = game();
    g.attach_script.push_str(
        " Game = Game or {}
          Game.SelectedUnit_SpeculativePopupTradeRoute_Display = function(x, y, t, d) end",
    );
    let mut r = rig_with_game(Default::default(), g);
    assert_eq!(r.exe.popup_route(), None);
    r.exec("Game.SelectedUnit_SpeculativePopupTradeRoute_Display(12, 7, 0, 0)");
    r.game.borrow_mut().events.extend([
        ev(UiEventKind::TradeDestroyRoute, &[-1, 0]),
        ev(
            UiEventKind::TradeNewRoute,
            &[
                -1, 0, 1, 11, 1, 1, 2, 1, 3, 1, 4, 1, 5, 1, 6, 1, 7, 1, 8, 1, 9, 1,
            ],
        ),
        ev(UiEventKind::TradeActivatePopupRoute, &[-1]),
    ]);
    r.exe.dispatch(&r.rt);
    let Some(PopupRoute::Temp { route, dest }) = r.exe.popup_route() else {
        panic!("no temp popup route");
    };
    assert_eq!((route.player, route.kind, route.count), (0, 1, 11));
    assert_eq!(route.plots.len(), 9);
    assert_eq!(route.plots[8], (9, 1));
    assert_eq!(dest, Some((12, 7)));
    r.game.borrow_mut().events.extend([
        ev(UiEventKind::TradeDeactivatePopupRoute, &[]),
        ev(UiEventKind::TradeActivatePopupRoute, &[4]),
    ]);
    r.exe.dispatch(&r.rt);
    assert_eq!(r.exe.popup_route(), Some(PopupRoute::Existing(4)));
    // b10: a route of more than 9 plots comes whole in the event's data (x, y pairs).
    let mut long = ev(UiEventKind::TradeNewRoute, &[-1, 0, 0, 20]);
    long.path = (0..20u32).flat_map(|i| [i + 2, 5]).collect();
    r.game.borrow_mut().events.extend([
        ev(UiEventKind::TradeDeactivatePopupRoute, &[]),
        long,
        ev(UiEventKind::TradeActivatePopupRoute, &[-1]),
    ]);
    r.exe.dispatch(&r.rt);
    let Some(PopupRoute::Temp { route, .. }) = r.exe.popup_route() else {
        panic!("no temp popup route");
    };
    assert_eq!((route.count, route.plots.len()), (20, 20));
    assert_eq!(route.plots[19], (21, 5));
    r.game.borrow_mut().events.extend([
        ev(UiEventKind::TradeDeactivatePopupRoute, &[]),
        ev(UiEventKind::TradeActivatePopupRoute, &[4]),
    ]);
    r.exe.dispatch(&r.rt);
    r.game
        .borrow_mut()
        .events
        .push(ev(UiEventKind::TradeDestroyRoute, &[4, 0]));
    r.exe.dispatch(&r.rt);
    assert_eq!(r.exe.popup_route(), None);
    assert_eq!(r.errors(), 0);
}
