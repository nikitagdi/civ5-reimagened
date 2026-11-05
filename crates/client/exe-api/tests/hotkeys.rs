//! The exe's hot keys (`ExeApi::hotkey`) over a stand-in action table, and the plot of the selected unit.
mod fake;

use exe_api::{ExeConfig, UiEventKind as K};
use fake::{FakeGame, rig_with_game, unit_block, with_block};

/// `GameInfoActions` and the part of `Game` the hot keys use: F is FORTIFY (priority 1) and a lower SLEEP (0) the
/// game does not allow, Shift+F an other action, Enter the END TURN control, Space SKIP through the alternate key.
const ACTIONS: &str = "
    local function act(t) for k, v in pairs({ HotKeyVal = -1, HotKeyPriority = 0, HotKeyValAlt = -1,
        HotKeyPriorityAlt = 0, AltDown = false, ShiftDown = false, CtrlDown = false, AltDownAlt = false,
        ShiftDownAlt = false, CtrlDownAlt = false, ControlType = -1 }) do if t[k] == nil then t[k] = v end end return t end
    GameInfoActions = {
        [0] = act { Type = 'MISSION_SLEEP', HotKeyVal = 18, HotKeyPriority = 0 },
        [1] = act { Type = 'MISSION_FORTIFY', HotKeyVal = 18, HotKeyPriority = 1 },
        [2] = act { Type = 'MISSION_ALERT', HotKeyVal = 18, HotKeyPriority = 5 },
        [3] = act { Type = 'COMMAND_SHIFT_F', HotKeyVal = 18, ShiftDown = true },
        [4] = act { Type = 'CONTROL_ENDTURN', HotKeyVal = 44, ControlType = 7 },
        [5] = act { Type = 'MISSION_SKIP', HotKeyValAlt = 57, HotKeyPriorityAlt = 1 },
    }
    GameInfoTypes = { CONTROL_ENDTURN = 7 }
    handled = {}
    Game = {
        CanHandleAction = function(i) return i ~= 2 end,
        HandleAction = function(i) table.insert(handled, 'action' .. i) end,
        DoControl = function(c) table.insert(handled, 'control' .. c) end,
    }";

fn game() -> FakeGame {
    FakeGame {
        attach_script: ACTIONS.into(),
        ..FakeGame::default()
    }
}

#[test]
fn a_key_runs_the_action_of_the_highest_priority_the_game_allows() {
    let r = rig_with_game(ExeConfig::default(), game());
    // F: ALERT has the highest priority but the game does not allow it; FORTIFY beats SLEEP.
    assert_eq!(
        r.exe
            .hotkey(u32::from(b'F'), false, false, false)
            .as_deref(),
        Some("MISSION_FORTIFY")
    );
    // Shift+F is an other action.
    assert_eq!(
        r.exe.hotkey(u32::from(b'F'), false, false, true).as_deref(),
        Some("COMMAND_SHIFT_F")
    );
    // Space through the alternate key.
    assert_eq!(
        r.exe.hotkey(0x20, false, false, false).as_deref(),
        Some("MISSION_SKIP")
    );
    // A key no action has does nothing.
    assert_eq!(r.exe.hotkey(u32::from(b'Q'), false, false, false), None);
    assert_eq!(r.exe.hotkey(0xff, false, false, false), None);
    assert_eq!(
        r.eval_str("return table.concat(handled, ',')"),
        "action1,action3,action5"
    );
    assert_eq!(r.errors(), 0);
}

#[test]
fn a_control_key_goes_through_do_control() {
    let r = rig_with_game(ExeConfig::default(), game());
    // Enter is END TURN: `Game.DoControl`, which the exe API wraps into the turn handshake (SIMP-exe-65).
    assert_eq!(
        r.exe.hotkey(0x0d, false, false, false).as_deref(),
        Some("CONTROL_ENDTURN")
    );
    assert!(
        !r.eval_str("return table.concat(handled, ',')")
            .contains("control"),
        "the wrapped DoControl sends the handshake instead"
    );
    assert!(r.exe.turn_complete_sent());
}

#[test]
fn the_selected_unit_has_a_plot() {
    let mut r = rig_with_game(ExeConfig::default(), game());
    assert_eq!(r.exe.selected_unit_plot(), None);
    r.game
        .borrow_mut()
        .events
        .push(with_block(K::UnitCreated, unit_block(0, 42, 5, 6), &[]));
    r.game
        .borrow_mut()
        .events
        .push(fake::ev(K::UnitSelectionChanged, &[1, 0, 42]));
    r.exe.dispatch(&r.rt);
    assert_eq!(r.exe.selected_unit_plot(), Some((5, 6)));
    r.game
        .borrow_mut()
        .events
        .push(fake::ev(K::UnitSelectionChanged, &[0, -1, -1]));
    r.exe.dispatch(&r.rt);
    assert_eq!(r.exe.selected_unit_plot(), None);
}

/// Plots with units for `UI.LocationSelect`: (3, 4) holds units 10 and 11 of player 0 and 50 of player 1; (5, 5) a city
/// of player 0 and no unit; the active player is 0.
const MAP: &str = "
    local function unit(o, id, x, y) return { GetOwner = function() return o end, GetID = function() return id end,
        GetPlot = function() return Map.GetPlot(x, y) end } end
    local units = { ['3,4'] = { unit(1, 50, 3, 4), unit(0, 10, 3, 4), unit(0, 11, 3, 4) } }
    local cities = { ['5,5'] = { GetOwner = function() return 0 end, GetID = function() return 7 end } }
    Map = { GetPlot = function(x, y)
        local k = x .. ',' .. y
        local list = units[k] or {}
        return { GetX = function() return x end, GetY = function() return y end,
            GetNumUnits = function() return #list end, GetUnit = function(self, i) return list[i + 1] end,
            GetPlotCity = function() return cities[k] end }
    end }
    Players = setmetatable({}, { __index = function(t, o) return { GetUnitByID = function(self, id) return unit(o, id, 3, 4) end } end })
    Game.GetActivePlayer = function() return 0 end";

#[test]
fn a_click_on_a_plot_selects_an_own_unit_there_and_the_next_on_the_next_click() {
    let r = rig_with_game(ExeConfig::default(), game());
    r.exec(MAP);
    let head = |r: &fake::Rig| r.game.borrow().head_unit;
    r.exec("UI.LocationSelect(Map.GetPlot(3, 4), false, false, false)");
    assert_eq!(
        head(&r),
        Some((0, 10)),
        "the first own unit, not the other player's"
    );
    r.exec("UI.LocationSelect(Map.GetPlot(3, 4), false, false, false)");
    assert_eq!(head(&r), Some((0, 11)));
    r.exec("UI.LocationSelect(Map.GetPlot(3, 4), false, false, false)");
    assert_eq!(head(&r), Some((0, 10)), "round again");
    // A plot with an own city and no unit opens the city.
    r.exec("UI.LocationSelect(Map.GetPlot(5, 5), false, false, false)");
    assert!(
        r.game
            .borrow()
            .ops
            .iter()
            .any(|(op, a)| *op == exe_api::UiOp::DoSelectCityAtPlot && a == &vec![5, 5])
    );
    // An empty plot changes nothing.
    r.exec("UI.LocationSelect(Map.GetPlot(9, 9), false, false, false)");
    assert_eq!(head(&r), Some((0, 10)));
    assert_eq!(r.errors(), 0);
}

#[test]
fn a_click_on_an_own_unit_flag_selects_the_unit() {
    let mut r = rig_with_game(ExeConfig::default(), game());
    r.exec(MAP);
    // UnitFlagManager.lua's flag button fires the event; the exe selects.
    r.exec("Events.SerialEventUnitFlagSelected(1, 50)");
    r.exe.dispatch(&r.rt);
    assert_eq!(
        r.game.borrow().head_unit,
        None,
        "an other player's flag selects nothing"
    );
    r.exec("Events.SerialEventUnitFlagSelected(0, 11)");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.game.borrow().head_unit, Some((0, 11)));
    assert_eq!(r.errors(), 0);
}
/// The active player and its unit 5 at (2, 3), for the turn gate and the numpad moves.
const PLAYER: &str = "
    turn_active = true
    Players = { [0] = { IsHuman = function() return true end, IsAlive = function() return true end,
        IsTurnActive = function() return turn_active end,
        GetUnitByID = function(self, id) return { GetX = function() return 2 end, GetY = function() return 3 end,
            GetOwner = function() return 0 end, GetID = function() return id end } end } }
    Game.GetActivePlayer = function() return 0 end
    Game.SelectionListMove = function(p, a, s, c) table.insert(handled, 'move' .. p.x .. ',' .. p.y .. ':' .. p.dir) end
    Map = { PlotDirection = function(x, y, d) return { x = x, y = y, dir = d } end }";

#[test]
fn numpad_keys_move_the_selected_unit_and_nothing_runs_off_turn() {
    let r = rig_with_game(
        ExeConfig::default(),
        FakeGame {
            attach_script: format!("{ACTIONS}\n{PLAYER}"),
            head_unit: Some((0, 5)),
            ..FakeGame::default()
        },
    );
    // Numpad 9: north-east (0); numpad 4: west (4).
    assert_eq!(
        r.exe.hotkey(0x69, false, false, false).as_deref(),
        Some("DIRECTION_MOVE")
    );
    assert_eq!(
        r.exe.hotkey(0x64, false, false, false).as_deref(),
        Some("DIRECTION_MOVE")
    );
    assert_eq!(
        r.exe
            .hotkey(u32::from(b'F'), false, false, false)
            .as_deref(),
        Some("MISSION_FORTIFY")
    );
    r.exec("Players[0].IsTurnActive = function() return false end");
    assert_eq!(
        r.exe.hotkey(u32::from(b'F'), false, false, false),
        None,
        "not our turn"
    );
    assert_eq!(
        r.eval_str("return table.concat(handled, ',')"),
        "move2,3:0,move2,3:4,action1"
    );
    assert_eq!(r.errors(), 0);
}

/// SIMP-exe-960: no hot key while the server's core moves the other civilizations (every human has ended the turn, the
/// exe's `IsGameCoreExecuting`); an observer is let through the turn gate (`SS_OBSERVER`).
#[test]
fn no_hot_key_while_the_core_executes_and_an_observer_may() {
    let game = FakeGame {
        attach_script: format!(
            "{ACTIONS}
            done = {{}}
            Game.IsNetworkMultiPlayer = function() return true end
            Game.GetActivePlayer = function() return 0 end
            Game.GetGameTurn = function() return 5 end
            GameDefines = {{ MAX_MAJOR_CIVS = 4 }}
            Players = {{}}
            for i = 0, 3 do Players[i] = {{ IsHuman = function() return i < 2 end, IsAlive = function() return true end,
                IsTurnActive = function() return true end, IsObserver = function() return false end,
                HasReceivedNetTurnComplete = function() return done[i] == true end }} end"
        ),
        ..FakeGame::default()
    };
    let mut r = rig_with_game(ExeConfig::default(), game);
    r.exec("done[1] = true");
    r.exe.dispatch(&r.rt);
    let f = |r: &fake::Rig| r.exe.hotkey(u32::from(b'F'), false, false, false);
    assert_eq!(f(&r).as_deref(), Some("MISSION_FORTIFY"));
    // The last human ends the turn: the AI phase.
    r.exec("Network.SendTurnComplete()");
    r.exe.dispatch(&r.rt);
    assert_eq!(f(&r), None, "the core executes");
    assert!(r.exe.turn_complete_refused());
    r.exe.dispatch(&r.rt);
    assert_eq!(f(&r).as_deref(), Some("MISSION_FORTIFY"));
    // An observer: not a human and no active turn, still handled.
    r.exec(
        "Players[0].IsHuman = function() return false end Players[0].IsTurnActive = function() return false end \
         Players[0].IsObserver = function() return true end",
    );
    assert_eq!(f(&r).as_deref(), Some("MISSION_FORTIFY"));
    r.exec("Players[0].IsObserver = function() return false end");
    assert_eq!(f(&r), None);
    assert_eq!(r.errors(), 0);
}
