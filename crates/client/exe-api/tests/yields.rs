//! The yield and resource icons of the map (`yields.rs`): the minimap panel's boxes reach the Lua managers as
//! `RequestYieldDisplay` / `ShowHexYield`, and the DLL's plot state as the resource icon events.

mod fake;

use exe_api::UiEventKind;
use fake::{FakeGame, ev, rig_with_game};
use platform::KeyValueStore;

/// A 3 x 2 map; player 0 owns the plots of row 0.
fn game() -> FakeGame {
    FakeGame {
        attach_script: "Map = {
              GetGridSize = function() return 3, 2 end,
              GetPlot = function(x, y)
                return { GetOwner = function() return y == 0 and 0 or -1 end }
              end,
              PlotDistance = function(ax, ay, bx, by) return math.max(math.abs(ax - bx), math.abs(ay - by)) end,
            }
            Game = { GetActivePlayer = function() return 0 end }"
            .into(),
        ..FakeGame::default()
    }
}

fn shown(r: &fake::Rig) -> String {
    r.log()
}

#[test]
fn the_yield_box_shows_and_hides_every_plot() {
    let mut r = rig_with_game(Default::default(), game());
    r.exec(
        "log = {} req = {}
         Events.ShowHexYield.Add(function(x, y, on) table.insert(log, x .. ':' .. y .. ':' .. tostring(on)) end)
         Events.RequestYieldDisplay.Add(function(t) table.insert(req, t) end)
         UI.SetYieldVisibleMode(true)",
    );
    r.exe.dispatch(&r.rt);
    assert_eq!(
        shown(&r),
        "0:0:true,0:1:true,1:0:true,1:1:true,2:0:true,2:1:true"
    );
    assert_eq!(r.eval_str("return table.concat(req, ',')"), "0");
    // A refresh shows the same set again; the box off hides it.
    r.exec("log = {} UI.RefreshYieldVisibleMode()");
    r.exe.dispatch(&r.rt);
    assert_eq!(shown(&r).split(',').count(), 6);
    r.exec("log = {} UI.SetYieldVisibleMode(false)");
    r.exe.dispatch(&r.rt);
    assert!(
        shown(&r).split(',').all(|s| s.ends_with("false")),
        "{}",
        shown(&r)
    );
    assert_eq!(r.errors(), 0);
}

/// A second game in the same page (b10): the exe of the first game goes with its Lua state, the second one starts from
/// nothing, so every plot gets its yield icon once, not once per game played.
#[test]
fn a_second_game_in_the_page_shows_each_yield_icon_once() {
    for game_number in 0..3 {
        let mut r = rig_with_game(Default::default(), game());
        r.exec(
            "log = {}
             Events.ShowHexYield.Add(function(x, y, on) table.insert(log, x .. ':' .. y .. ':' .. tostring(on)) end)
             UI.SetYieldVisibleMode(true)",
        );
        r.exe.dispatch(&r.rt);
        r.exe.dispatch(&r.rt);
        let mut plots: Vec<_> = shown(&r).split(',').map(str::to_owned).collect();
        assert_eq!(plots.len(), 6, "game {game_number}: {plots:?}");
        plots.sort();
        plots.dedup();
        assert_eq!(plots.len(), 6, "game {game_number}: a plot shown twice");
        assert_eq!(r.errors(), 0);
    }
}

#[test]
fn an_area_and_the_empire_replace_each_other() {
    let mut r = rig_with_game(Default::default(), game());
    r.exec(
        "log = {}
         Events.ShowHexYield.Add(function(x, y, on) table.insert(log, x .. ':' .. y .. ':' .. tostring(on)) end)
         Events.RequestYieldDisplay(YieldDisplayTypes.AREA, 1, 0, 1)",
    );
    r.exe.dispatch(&r.rt);
    assert_eq!(shown(&r), "0:0:true,0:1:true,1:0:true,1:1:true");
    r.exec("log = {} Events.RequestYieldDisplay(YieldDisplayTypes.EMPIRE)");
    r.exe.dispatch(&r.rt);
    assert_eq!(shown(&r), "0:1:false,1:1:false,2:0:true");
    r.exec("log = {} Events.RequestYieldDisplay(YieldDisplayTypes.AREA, 0)");
    r.exe.dispatch(&r.rt);
    assert_eq!(shown(&r), "0:0:false,1:0:false,2:0:false");
    assert_eq!(r.errors(), 0);
}

/// The replica sends no plot state: the exe reads the resources the active team sees after a load and after a change
/// of the fog (an unrevealed plot and a city plot have no icon).
#[test]
fn the_seen_resources_are_announced_after_the_load_and_follow_the_fog() {
    let g = FakeGame {
        attach_script: "res = { ['1:0'] = 5, ['2:1'] = 7, ['0:1'] = 3 }
            Map = {
              GetGridSize = function() return 3, 2 end,
              GetPlot = function(x, y)
                return {
                  IsRevealed = function() return not (x == 0 and y == 1) end,
                  IsCity = function() return x == 2 and y == 1 end,
                  GetResourceType = function() return res[x .. ':' .. y] or -1 end,
                  GetImprovementType = function() return -1 end,
                }
              end,
            }
            Game = { GetActivePlayer = function() return 0 end, GetActiveTeam = function() return 0 end }"
            .into(),
        ..FakeGame::default()
    };
    let mut r = rig_with_game(Default::default(), g);
    r.exec(
        "log = {}
         Events.SerialEventRawResourceIconCreated.Add(function(i, j, imp, res) table.insert(log, 'c' .. i .. ':' .. j .. ':' .. res) end)
         Events.SerialEventRawResourceIconDestroyed.Add(function(i, j) table.insert(log, 'd' .. i .. ':' .. j) end)",
    );
    // A load of the game (a new generation of the replica).
    r.game.borrow_mut().generation += 1;
    r.exe.dispatch(&r.rt);
    let (i, j) = exe_api::hex::grid_to_hex(1, 0);
    assert_eq!(r.log(), format!("c{i}:{j}:5"));
    r.exec("log = {} res['1:0'] = nil");
    r.game
        .borrow_mut()
        .events
        .push(ev(UiEventKind::FowChanged, &[1, 0, 0, 0]));
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), format!("d{i}:{j}"));
    assert_eq!(r.errors(), 0);
}

#[test]
fn the_resource_box_reaches_the_resource_manager_and_the_plot_state_its_icons() {
    let mut r = rig_with_game(Default::default(), game());
    r.exec(
        "log = {} req = {}
         Events.RequestYieldDisplay.Add(function(t) table.insert(req, t) end)
         Events.SerialEventRawResourceIconCreated.Add(function(i, j, imp, res) table.insert(log, 'c' .. i .. ':' .. j .. ':' .. res) end)
         Events.SerialEventRawResourceIconDestroyed.Add(function(i, j) table.insert(log, 'd' .. i .. ':' .. j) end)
         UI.SetResourceVisibleMode(true) UI.SetResourceVisibleMode(false)",
    );
    r.exe.dispatch(&r.rt);
    assert_eq!(r.eval_str("return table.concat(req, ',')"), "2,3");
    let state = |x, y, res| ev(UiEventKind::PlotStateChange, &[x, y, res, -1, 0, -1, 0]);
    {
        let mut g = r.game.borrow_mut();
        g.events.push(state(2, 1, 5));
        g.events.push(state(2, 1, 5));
        g.events.push(state(1, 0, -1));
        g.events.push(state(2, 1, -1));
    }
    r.exe.dispatch(&r.rt);
    let (i, j) = exe_api::hex::grid_to_hex(2, 1);
    assert_eq!(r.log(), format!("c{i}:{j}:5,d{i}:{j}"));
    assert_eq!(r.errors(), 0);
}

/// The exe keeps to the plots of the camera's rectangle: a move of the camera shows and hides the strips.
#[test]
fn the_yield_icons_keep_to_the_camera_rectangle() {
    let mut r = rig_with_game(Default::default(), game());
    r.game.borrow_mut().generation += 1;
    r.exe.dispatch(&r.rt);
    r.host.visible.set(Some([0, 0, 2, 2]));
    r.exec(
        "log = {}
         Events.ShowHexYield.Add(function(x, y, on) table.insert(log, x .. ':' .. y .. ':' .. tostring(on)) end)
         UI.SetYieldVisibleMode(true)",
    );
    r.exe.dispatch(&r.rt);
    r.exe.dispatch(&r.rt);
    let all = shown(&r);
    assert!(
        all.contains("0:0:true") && all.contains("1:1:true"),
        "{all}"
    );
    assert!(!all.contains("2:0"), "{all}");
    r.exec("log = {}");
    r.host.visible.set(Some([1, 0, 3, 2]));
    r.exe.dispatch(&r.rt);
    r.exe.dispatch(&r.rt);
    assert_eq!(shown(&r), "0:0:false,0:1:false,2:0:true,2:1:true");
    assert_eq!(r.errors(), 0);
}

/// A plot whose yields changed (a belief, a tech) is told again: `HexYieldMightHaveChanged` with the grid position.
#[test]
fn a_plot_whose_yield_changed_is_told_again() {
    let g = FakeGame {
        attach_script: "S = { food = 3 }
            Map = {
              GetGridSize = function() return 2, 1 end,
              GetPlot = function(x, y)
                return {
                  IsRevealed = function() return true end,
                  CalculateYield = function(_, k, d) if x == 1 and k == 0 then return S.food end return 1 end,
                }
              end,
            }
            Game = { GetActivePlayer = function() return 0 end, GetActiveTeam = function() return 0 end }"
            .into(),
        ..FakeGame::default()
    };
    let mut r = rig_with_game(Default::default(), g);
    r.game.borrow_mut().generation += 1;
    r.exe.dispatch(&r.rt);
    r.exec(
        "log = {}
         Events.HexYieldMightHaveChanged.Add(function(x, y) table.insert(log, tostring(x) .. ':' .. tostring(y)) end)
         UI.SetYieldVisibleMode(true)",
    );
    // The first scan (after the setup) takes the yields as they are.
    for _ in 0..30 {
        r.exe.dispatch(&r.rt);
    }
    assert_eq!(r.log(), "");
    r.exec("S.food = 4");
    // The scan is idle-paced: enough frames pass.
    for _ in 0..100 {
        r.exe.dispatch(&r.rt);
    }
    assert_eq!(r.log(), "1:0");
    assert_eq!(r.errors(), 0);
}

/// The stored options are in force from the first frame of a game.
#[test]
fn the_stored_map_options_are_applied_at_the_start() {
    let host = fake::TestHost::default();
    host.kv.set("civ5.options.YieldOn", "1");
    host.kv.set("civ5.options.ResourceOn", "1");
    host.kv.set("civ5.options.GridOn", "0");
    let mut r = fake::rig_with_host(Default::default(), game(), host);
    r.exec(
        "log = {} Events.ShowHexYield.Add(function(x, y, on) table.insert(log, x .. ':' .. y) end)",
    );
    r.exe.dispatch(&r.rt);
    r.exec("UI.RefreshYieldVisibleMode()");
    r.exe.dispatch(&r.rt);
    assert_eq!(shown(&r).split(',').count(), 6, "{}", shown(&r));
    assert_eq!(r.host.grid.borrow().as_slice(), &[false]);
    assert_eq!(r.errors(), 0);
}

#[test]
fn a_plot_back_in_view_has_its_icon_numbers_rebuilt() {
    // audit 40: the Lua caches the numbers of an icon; a plot out of the shown set is not polled, so on the way back it
    // is told HexYieldMightHaveChanged before ShowHexYield. The first showing needs nothing.
    let mut r = rig_with_game(Default::default(), game());
    r.exec(
        "log = {}
         Events.ShowHexYield.Add(function(x, y, on) table.insert(log, 'show' .. x .. ':' .. y .. ':' .. tostring(on)) end)
         Events.HexYieldMightHaveChanged.Add(function(x, y) table.insert(log, 'chg' .. x .. ':' .. y) end)
         UI.SetYieldVisibleMode(true)",
    );
    r.exe.dispatch(&r.rt);
    assert!(!shown(&r).contains("chg"), "{}", shown(&r));
    r.exec("log = {} UI.SetYieldVisibleMode(false)");
    r.exe.dispatch(&r.rt);
    r.exec("log = {} UI.SetYieldVisibleMode(true)");
    r.exe.dispatch(&r.rt);
    assert!(
        shown(&r).starts_with("chg0:0,show0:0:true"),
        "{}",
        shown(&r)
    );
}

/// SIMP-exe-951: an improvement or a road that appeared on a revealed plot is told (the first read only records).
#[test]
fn a_new_improvement_and_road_are_told_after_the_first_read() {
    let g = FakeGame {
        attach_script: "imp = { ['0:0'] = 4 } road = {}
            Map = {
              GetGridSize = function() return 3, 2 end,
              GetPlot = function(x, y)
                local k = x .. ':' .. y
                return {
                  IsRevealed = function() return true end,
                  IsCity = function() return false end,
                  GetResourceType = function() return -1 end,
                  GetImprovementType = function() return imp[k] or -1 end,
                  GetRouteType = function() return road[k] or -1 end,
                  GetOwner = function() return 0 end,
                  GetContinentArtType = function() return 0 end,
                }
              end,
            }
            Game = { GetActivePlayer = function() return 0 end, GetActiveTeam = function() return 0 end }"
            .into(),
        ..FakeGame::default()
    };
    let mut r = rig_with_game(Default::default(), g);
    r.exec(
        "log = {}
         Events.SerialEventImprovementCreated.Add(function(i, j, cul, cont, pl, what) table.insert(log, 'i' .. pl .. ':' .. what) end)
         Events.SerialEventRoadCreated.Add(function(i, j, pl, what) table.insert(log, 'r' .. pl .. ':' .. what) end)",
    );
    // The load only records what stands there.
    r.game.borrow_mut().generation += 1;
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "");
    r.exec("imp['1:1'] = 7 road['2:0'] = 1");
    r.game
        .borrow_mut()
        .events
        .push(ev(UiEventKind::TechAcquired, &[0, 1]));
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "i0:7,r0:1");
    // Nothing changed: nothing is told again.
    r.exec("log = {}");
    r.game
        .borrow_mut()
        .events
        .push(ev(UiEventKind::TechAcquired, &[0, 2]));
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "");
    assert_eq!(r.errors(), 0);
}
