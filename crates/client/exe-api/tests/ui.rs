//! `UI.*`, `Network.*` and `InterfaceBuddy` (WO-4, fake game).
mod fake;

use exe_api::{ClientEvent, ExeConfig, NetMessage, UiEventKind as K, UiOp};
use fake::{EvExt, FakeGame, Rig, ev, rig, rig_with, rig_with_game, unit_block, with_block};

/// The first `n` arguments of a message; the rest are 0.
fn args(m: &NetMessage, n: usize) -> Vec<i32> {
    assert!(m.args[n..].iter().all(|a| *a == 0), "{m:?}");
    m.args[..n].to_vec()
}

/// The part of the DLL's API the UI glue touches: `Players[i]:GetUnitByID(id)` and friends.
fn dll_stand_in(r: &Rig) {
    r.ctx
        .lua()
        .exec(
            "local function plot(x, y) return { GetX = function() return x end, GetY = function() return y end } end
             local function unit(o, id) return { GetOwner = function() return o end, GetID = function() return id end,
                 GetPlot = function() return plot(3, 4) end } end
             local function city(o, id) return { GetOwner = function() return o end, GetID = function() return id end,
                 Plot = function() return plot(5, 6) end } end
             Players = setmetatable({}, { __index = function(t, o)
                 return { GetUnitByID = function(self, id) return unit(o, id) end, GetCityByID = function(self, id) return city(o, id) end }
             end })
             MyPlot = plot(8, 9)
             MyUnit = unit(0, 42)
             MyCity = city(1, 7)
             Map = { GetPlot = function(x, y) return plot(x, y) end }
             Game = { GetActivePlayer = function() return 2 end }
             GameInfo = { Units = { [3] = { PortraitIndex = 11, IconAtlas = 'UNIT_ATLAS', UnitFlagIconOffset = 5, UnitFlagAtlas = 'FLAG_ATLAS' } } }",
            "=dll",
        )
        .unwrap();
}

#[test]
fn selection_goes_to_the_replica_and_comes_back_as_events() {
    let mut r = rig();
    dll_stand_in(&r);
    r.exec(
        "log = {}
         Events.UnitSelectionChanged.Add(function(p, u, i, j, k, sel) table.insert(log, 'sel' .. p .. ':' .. u .. ':' .. tostring(sel)) end)
         Events.UnitSelectionCleared.Add(function() table.insert(log, 'cleared') end)
         UI.SelectUnit(MyUnit)
         local u = UI.GetHeadSelectedUnit()
         table.insert(log, 'head' .. u:GetOwner() .. ':' .. u:GetID())
         table.insert(log, 'id' .. UI.GetSelectedUnitID())
         UI.ClearSelectionList()
         table.insert(log, 'after' .. tostring(UI.GetHeadSelectedUnit()))",
    );
    r.exe.dispatch(&r.rt);
    // The core tells the selection as (count, head); the exe's events are made from it.
    assert_eq!(
        r.log(),
        "head0:42,id42,afternil,sel0:42:true,sel0:42:false,cleared"
    );
    assert_eq!(r.game.borrow().ops[0], (UiOp::SelectUnit, vec![0, 42]));
    assert_eq!(r.errors(), 0);
}

#[test]
fn city_selection_and_interface_mode() {
    let mut r = rig();
    dll_stand_in(&r);
    r.exec(
        "log = {}
         Events.InterfaceModeChanged.Add(function(old, new) table.insert(log, 'mode' .. old .. '>' .. new) end)
         UI.SelectCity(MyCity)
         local c = UI.GetHeadSelectedCity()
         table.insert(log, 'city' .. c:GetOwner() .. ':' .. c:GetID())
         UI.ClearSelectedCities()
         table.insert(log, tostring(UI.GetHeadSelectedCity()))
         UI.SetInterfaceMode(5, 2)
         table.insert(log, 'now' .. UI.GetInterfaceMode() .. ':' .. UI.GetInterfaceModeValue() .. ':' .. UI.GetInterfaceModeDebugItemID2())
         table.insert(log, tostring(UI.CanDoInterfaceMode(5)))
         UI.DoSelectCityAtPlot(MyPlot)",
    );
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "city1:7,nil,now5:2:0,true,mode0>5");
    assert!(
        r.game
            .borrow()
            .ops
            .iter()
            .any(|(op, a)| *op == UiOp::DoSelectCityAtPlot && a == &vec![8, 9])
    );
}

#[test]
fn selecting_a_city_state_city_raises_its_diplomacy_popup() {
    // Live report 2026-10-30: a click on a city-state's banner (CityBannerManager.OnBannerClick ->
    // UI.DoSelectCityAtPlot) opened nothing; the replica selects own cities only.
    let mut r = rig();
    r.ctx
        .lua()
        .exec(
            "ButtonPopupTypes = { BUTTONPOPUP_CITY_STATE_DIPLO = 77 }
             Game = { GetActivePlayer = function() return 0 end }
             function CityPlot(owner)
                 return { GetX = function() return 1 end, GetY = function() return 2 end,
                          GetPlotCity = function() return { GetOwner = function() return owner end } end }
             end
             Players = { [0] = { IsMinorCiv = function() return false end }, [3] = { IsMinorCiv = function() return true end },
                         [4] = { IsMinorCiv = function() return false end } }",
            "=dll",
        )
        .unwrap();
    r.exec(
        "log = {}
         Events.SerialEventGameMessagePopup.Add(function(info) table.insert(log, 'popup' .. info.Type .. ':' .. info.Data1) end)
         local plot = CityPlot
         UI.DoSelectCityAtPlot(plot(3))
         UI.DoSelectCityAtPlot(plot(4))
         UI.DoSelectCityAtPlot(plot(0))",
    );
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "popup77:3");
    assert_eq!(r.errors(), 0);
}

#[test]
fn state_questions_of_the_core() {
    let mut r = rig();
    r.game.borrow_mut().can_end_turn = true;
    r.exec("log = {}
        Events.SerialEventEnterCityScreen.Add(function() table.insert(log, 'enter') end)
        Events.SerialEventExitCityScreen.Add(function() table.insert(log, 'exit') end)
        table.insert(log, tostring(UI.CanEndTurn())) UI.SetCityScreenUp(true) table.insert(log, tostring(UI.IsCityScreenUp()))");
    assert_eq!(r.log(), "true,true");
    assert!(r.game.borrow().city_screen);
    // CITY_SCREEN_CHANGED of the core is the exe's enter / exit.
    r.exe.dispatch(&r.rt);
    r.exec("UI.SetCityScreenUp(false)");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "true,true,enter,exit");
}

#[test]
fn entering_the_city_screen_makes_it_dirty() {
    // Live report 2026-10-30: CityView.lua (EUI) refreshes on SerialEventCityScreenDirty only; a screen opened again
    // after RETURN kept the viewing mode of its exit and took no production click.
    let mut r = rig();
    r.exec(
        "log = {}
        Events.SerialEventEnterCityScreen.Add(function() table.insert(log, 'enter') end)
        Events.SerialEventCityScreenDirty.Add(function() table.insert(log, 'dirty') end)
        UI.SetCityScreenUp(true)",
    );
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "enter,dirty");
    // Leaving does not; entering again does again.
    r.exec("UI.SetCityScreenUp(false)");
    r.exe.dispatch(&r.rt);
    r.exec("UI.SetCityScreenUp(true)");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "enter,dirty,enter,dirty");
    assert_eq!(r.errors(), 0);
}

#[test]
fn look_at_a_dead_plot_is_no_error() {
    // InGame.lua keeps `lastCityEnteredPlot` across a turn's load and looks at it when the city screen closes.
    let r = rig();
    r.exec(
        "log = {}
        local dead = setmetatable({}, { __index = function() return function() error('Not a valid instance') end end })
        UI.LookAt(dead) table.insert(log, 'ok')",
    );
    assert_eq!(r.log(), "ok");
    assert_eq!(r.errors(), 0);
}

#[test]
fn look_at_selection_plot_follows_a_selected_city() {
    // Live report 2026-10-31: EUI's CityView.lua cycles to the next city that needs production with
    // UI.SelectCity + UI.LookAtSelectionPlot; the camera must go to that city.
    let r = rig();
    dll_stand_in(&r);
    r.exec("UI.SelectCity(MyCity) UI.LookAtSelectionPlot()");
    assert_eq!(r.host.looked.borrow().last(), Some(&(5, 6)));
    assert_eq!(r.errors(), 0);
}

/// A seat 2 with its city 7 at (5, 6) and another player's city 9 at (20, 21).
fn camera_stand_in(r: &Rig) {
    dll_stand_in(r);
    r.ctx
        .lua()
        .exec(
            "local function plot(x, y) return { GetX = function() return x end, GetY = function() return y end } end
             local function city(o, id, x, y) return { GetOwner = function() return o end, GetID = function() return id end,
                 Plot = function() return plot(x, y) end } end
             Players = setmetatable({}, { __index = function(t, o)
                 return { GetUnitByID = function(self, id) return MyUnit end, GetCityByID = function(self, id) if id == 7 then return city(o, id, 5, 6) end return city(o, id, 20, 21) end }
             end })
             Mine = city(2, 7, 5, 6)
             Theirs = city(3, 9, 20, 21)",
            "=cam",
        )
        .unwrap();
}

#[test]
fn the_cores_look_at_selection_plot_moves_the_camera_to_the_selected_city() {
    // Live report 2026-10-31: the city screen's previous / next arrows (CityView.lua: Game.DoControl(CONTROL_NEXTCITY))
    // select the next city and the DLL asks lookAtSelectionPlot; the exe API dropped that event, the camera stayed.
    let mut r = rig();
    camera_stand_in(&r);
    r.exec("UI.SelectCity(Mine)");
    r.game
        .borrow_mut()
        .events
        .push(ev(K::LookAtSelectionPlot, &[0]));
    r.exe.dispatch(&r.rt);
    assert_eq!(r.host.looked.borrow().last(), Some(&(5, 6)));
    // The release form moves nothing.
    r.host.looked.borrow_mut().clear();
    r.game
        .borrow_mut()
        .events
        .push(ev(K::LookAtSelectionPlot, &[1]));
    r.exe.dispatch(&r.rt);
    assert!(r.host.looked.borrow().is_empty());
    assert_eq!(r.errors(), 0);
}

#[test]
fn another_players_city_never_moves_the_camera() {
    // Commit 54e0b6f: no camera jump to others' cities. A selection that is not the seat's is not looked at, whichever way
    // the event or the city screen came.
    let mut r = rig();
    camera_stand_in(&r);
    r.exec("UI.SelectCity(Theirs)");
    r.game
        .borrow_mut()
        .events
        .push(ev(K::LookAtSelectionPlot, &[0]));
    r.game
        .borrow_mut()
        .events
        .push(ev(K::CityScreenChanged, &[1]));
    r.exe.dispatch(&r.rt);
    assert!(r.host.looked.borrow().is_empty(), "{:?}", r.host.looked);
    assert_eq!(r.errors(), 0);
}

#[test]
fn opening_the_city_screen_centres_on_its_city_however_the_core_opened_it() {
    let mut r = rig();
    camera_stand_in(&r);
    r.exec("UI.SelectCity(Mine)");
    // The core raises the screen itself (no UI.SetCityScreenUp from a script).
    r.game
        .borrow_mut()
        .events
        .push(ev(K::CityScreenChanged, &[1]));
    r.exe.dispatch(&r.rt);
    assert_eq!(*r.host.looked.borrow(), vec![(5, 6)]);
    // Leaving does not look anywhere.
    r.game
        .borrow_mut()
        .events
        .push(ev(K::CityScreenChanged, &[0]));
    r.exe.dispatch(&r.rt);
    assert_eq!(r.host.looked.borrow().len(), 1);
}

#[test]
fn look_at_selection_plot_prefers_the_city_while_the_city_screen_is_up() {
    let r = rig();
    camera_stand_in(&r);
    r.exec("UI.SelectUnit(MyUnit) UI.SelectCity(Mine) UI.LookAtSelectionPlot()");
    assert_eq!(
        r.host.looked.borrow().last(),
        Some(&(3, 4)),
        "the unit first"
    );
    r.exec("UI.SetCityScreenUp(true)");
    r.host.looked.borrow_mut().clear();
    r.exec("UI.LookAtSelectionPlot() UI.LookAtSelectionPlot(1)");
    assert_eq!(
        *r.host.looked.borrow(),
        vec![(5, 6)],
        "the city, once: the release moves nothing"
    );
}

#[test]
fn camera_mouse_and_keys_are_the_clients() {
    let mut r = rig();
    dll_stand_in(&r);
    r.exec("UI.LookAt(MyPlot, 0)");
    assert_eq!(*r.host.looked.borrow(), vec![(8, 9)]);
    r.exec("UI.LookAtSelectionPlot()");
    assert!(r.host.looked.borrow().len() == 1, "nothing is selected");
    r.exec("UI.SelectUnit(MyUnit) UI.LookAtSelectionPlot()");
    assert_eq!(r.host.looked.borrow().last(), Some(&(3, 4)));

    r.exe.post(ClientEvent::MouseOverHex { x: 6, y: 7 });
    assert_eq!(
        r.eval_str("local x, y = UI.GetMouseOverHex() return x .. ',' .. y"),
        "6,7"
    );
    assert!(!r.eval_bool("return UI.CtrlKeyDown()"));
    r.exe.post(ClientEvent::Modifiers {
        ctrl: true,
        shift: false,
        alt: true,
    });
    assert!(r.eval_bool("return UI.CtrlKeyDown() and not UI.ShiftKeyDown() and UI.AltKeyDown()"));
    assert!(!r.eval_bool("return UI.IsTouchScreenEnabled() or UI.IsCameraMoving()"));
}

#[test]
fn portrait_and_flag_icons_come_from_the_database() {
    let r = rig();
    dll_stand_in(&r);
    assert_eq!(
        r.eval_str("local i, a = UI.GetUnitPortraitIcon(3, 0) return i .. a"),
        "11UNIT_ATLAS"
    );
    assert_eq!(
        r.eval_str("local i, a = UI.GetUnitFlagIcon(3, 0) return i .. a"),
        "5FLAG_ATLAS"
    );
}

#[test]
fn activate_and_remove_notification_go_to_the_replica_by_lookup_id() {
    let mut r = rig();
    dll_stand_in(&r);
    r.exec(
        "log = {} Events.NotificationRemoved.Add(function(id) table.insert(log, 'rm' .. id) end)
         UI.ActivateNotification(12) UI.RemoveNotification(13)",
    );
    let ops = r.game.borrow().ops.clone();
    assert_eq!(ops[0], (UiOp::ActivateNotification, vec![12]));
    assert_eq!(ops[1], (UiOp::RemoveNotification, vec![13]));
    // The core answers a removal with NOTIFICATION_REMOVED.
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "rm13");
}

#[test]
fn stubs_answer_and_are_counted() {
    let r = rig();
    // (ProposedDealExists is real since K12: without a replica it answers false and counts nothing.)
    assert!(!r.eval_bool("return UI.ProposedDealExists(0, 1)"));
    assert_eq!(r.eval_str("return tostring(UI.HasMadeProposal(0))"), "-1");
    assert_eq!(r.eval_str("return tostring(UI.GetNumCurrentDeals(0))"), "0");
    r.exec("local d = UI.GetScratchDeal() d.AddGoldTrade(d, 0, 1)");
    let s = r.exe.stats().simp_by_name;
    assert_eq!(s.get("UI.HasMadeProposal"), Some(&1));
    assert_eq!(s.get("UI.ScratchDeal.AddGoldTrade"), Some(&1));
    assert_eq!(r.errors(), 0);
}

#[test]
fn a_deal_answer_given_before_the_replica_attached_goes_out_after_it() {
    // The Lua libraries of the game attach late after a reload; an answer in that window used to vanish in the counted stub.
    let r = rig();
    r.exec("UI.DoFinalizePlayerDeal(0, 1, true)");
    // The DLL's side: base globals, set when the game's Lua libraries attach.
    r.ctx
        .lua()
        .exec(
            "sent = {} GcUiFinalizePlayerDeal = function(a, b, c) table.insert(sent, a .. ':' .. b .. ':' .. tostring(c)) end GcUiHasMadeProposal = function() return -1 end",
            "=dll",
        )
        .unwrap();
    assert_eq!(r.eval_str("return tostring(#sent)"), "0");
    r.exec("UI.HasMadeProposal(0)");
    assert_eq!(r.eval_str("return table.concat(sent, ',')"), "0:1:true");
    // Once only, and a live answer goes out at once, in order.
    r.exec("UI.DoFinalizePlayerDeal(1, 0, false)");
    assert_eq!(
        r.eval_str("return table.concat(sent, ',')"),
        "0:1:true,1:0:false"
    );
    assert_eq!(r.errors(), 0);
}

#[test]
fn the_equalize_buttons_answer_with_the_leaders_line_on_the_trade_state() {
    // The core changes the scratch deal and says nothing; the trade screen redraws on an AILeaderMessage of the trade
    // state, so the exe API fires one with the leader's Diplomacy_Responses line.
    let mut r = rig();
    // The DLL's side (the base globals the exe API's Lua reads), then the screen's calls.
    r.ctx
        .lua()
        .exec(
            "DiploUIStateTypes = { DIPLO_UI_STATE_TRADE = 7 }
         Locale = { ConvertTextKey = function(k) return 'text:' .. k end }
         Players = { [3] = { GetLeaderType = function() return 0 end } }
         GameInfo = { Leaders = { [0] = { Type = 'LEADER_X' } },
           Diplomacy_Responses = function()
             local rows = {
               { LeaderType = 'LEADER_X', ResponseType = 'RESPONSE_TRADE_AI_MAKES_OFFER', Response = 'TXT_OFFER', Bias = 1 },
               { LeaderType = 'LEADER_X', ResponseType = 'RESPONSE_TRADE_NO_DEAL_POSSIBLE', Response = 'TXT_NO', Bias = 1 },
               { LeaderType = 'LEADER_X', ResponseType = 'RESPONSE_TRADE_DEAL_UNCHANGED', Response = 'TXT_SAME', Bias = 1 },
               { LeaderType = 'LEADER_Y', ResponseType = 'RESPONSE_TRADE_AI_MAKES_OFFER', Response = 'TXT_OTHER', Bias = 1 } }
             local i = 0
             return function() i = i + 1 return rows[i] end
           end }
         items = 1
         GcUiGetScratchDeal = function()
           return { GetToPlayer = function() return 3 end, GetNumItems = function() return items end,
                    ResetIterator = function() end, GetNextItem = function() end }
         end
         LOG = {}
         Ctl = { verdict = true, grow = true }
         GcUiEqualize = function(mode)
           table.insert(LOG, 'mode' .. mode)
           if Ctl.grow then items = items + 1 end
           return Ctl.verdict
         end",
            "=dll",
        )
        .unwrap();
    r.exec(
        "log = LOG
         Events.AILeaderMessage.Add(function(p, s, t, a, d) table.insert(log, p .. ':' .. s .. ':' .. t) end)
         UI.DoWhatDoesAIWant()
         Ctl.grow = false
         UI.DoWhatWillAIGive()
         Ctl.verdict = false
         UI.DoEqualizeDealWithHuman()",
    );
    r.exe.dispatch(&r.rt);
    assert_eq!(
        r.log(),
        "mode1,mode2,mode0,3:7:text:TXT_OFFER,3:7:text:TXT_SAME,3:7:text:TXT_NO"
    );
    assert_eq!(r.errors(), 0);
}

#[test]
fn an_operation_the_core_refuses_is_nil_not_a_crash() {
    let r = rig();
    // No game state yet: nothing selected.
    assert_eq!(
        r.eval_str("return tostring(UI.GetHeadSelectedUnit())"),
        "nil"
    );
    assert_eq!(r.eval_str("return tostring(UI.GetSelectedUnitID())"), "-1");
}

// ---- Network ----

#[test]
fn network_sends_are_posted_in_order_with_ints_and_text() {
    let mut r = rig();
    r.exec(
        "Network.SendResearch(12, 0, -1, false)
         Network.SendFoundReligion(0, 3, 'My Faith', 1, 2, 3, 4, 5, 6)
         Network.SendRenameCity(9, 'Roma')
         Network.SendSetCityAvoidGrowth(4, true)",
    );
    r.exe.dispatch(&r.rt);
    let posted = r.game.borrow().posted.clone();
    let names: Vec<_> = posted.iter().map(NetMessage::name).collect();
    assert_eq!(
        names,
        [
            "sendResearch",
            "SendFoundReligion",
            "SendRenameCity",
            "sendSetCityAvoidGrowth"
        ]
    );
    assert_eq!(args(&posted[0], 4), [12, 0, -1, 0]);
    // The sending player (first argument) is not carried (SIMP-exe-331).
    assert_eq!(args(&posted[1], 8), [3, 1, 2, 3, 4, 5, 6, 0]);
    assert_eq!(posted[1].text, "My Faith");
    assert_eq!(args(&posted[2], 1), [9]);
    assert_eq!(posted[2].text, "Roma");
    assert_eq!(args(&posted[3], 2), [4, 1]);
    assert!(posted[0].text.is_empty());
}

struct StepClock(std::cell::Cell<f64>);
impl platform::Clock for StepClock {
    fn now_ms(&self) -> u64 {
        self.0.get() as u64
    }
    fn now_ms_f64(&self) -> f64 {
        // Every reading is 2 ms later: each piece of work has a length.
        self.0.set(self.0.get() + 2.0);
        self.0.get()
    }
}

#[test]
fn the_click_a_load_and_the_turn_events_are_timed_for_the_timeline() {
    let mut r = rig();
    r.exe
        .set_clock(std::rc::Rc::new(StepClock(std::cell::Cell::new(100.0))));
    r.exe.dispatch(&r.rt);
    let _ = r.exe.take_spans();
    r.exec("Network.SendTurnComplete()");
    let spans = r.exe.take_spans();
    assert_eq!(spans.len(), 1, "{spans:?}");
    assert_eq!(spans[0].name, "c.click");
    assert!(spans[0].dur_ms < 0.0, "a mark");
    // A load (a new generation): the Lua reset, then the turn's events.
    r.game.borrow_mut().generation += 1;
    r.exe.dispatch(&r.rt);
    let spans = r.exe.take_spans();
    let names: Vec<_> = spans.iter().map(|s| s.name).collect();
    assert_eq!(names, ["c.lua_reset", "c.turn_events"], "{spans:?}");
    assert!(spans[0].dur_ms > 0.0 && spans[1].dur_ms > 0.0);
    assert!(
        spans[1].t0_ms >= spans[0].t0_ms + spans[0].dur_ms,
        "the events follow the reset: {spans:?}"
    );
    // A frame with no load times nothing.
    r.exe.dispatch(&r.rt);
    assert!(r.exe.take_spans().is_empty());
}

#[test]
fn network_turn_handshake_and_getters() {
    let r = rig();
    assert!(!r.eval_bool("return Network.HasSentNetTurnComplete()"));
    r.exec("Network.SendTurnComplete()");
    assert!(r.eval_bool("return Network.HasSentNetTurnComplete()"));
    r.exec("Network.SendTurnUnready()");
    assert!(!r.eval_bool("return Network.HasSentNetTurnComplete()"));
    let names: Vec<_> = r
        .game
        .borrow()
        .posted
        .iter()
        .map(NetMessage::name)
        .collect();
    assert_eq!(names, ["sendTurnComplete", "sendTurnUnready"]);
    *r.host.seats.borrow_mut() = exe_api::SeatInfo {
        local_id: 0,
        pings_ms: vec![(2, 77)],
        ..Default::default()
    };
    assert!(
        r.eval_bool("return Network.IsPlayerConnected(2) and not Network.IsPlayerConnected(1)")
    );
    assert_eq!(r.eval_str("return tostring(Network.GetPingTime(2))"), "77");
    assert!(!r.eval_bool("return Network.IsPlayerHotJoining(0)"));
}

/// Every seat shows its own emblem: `PreGame.GetCivilization(player)` is the live player's civilization (SIMP-exe-650), not the
/// one shared value the replica's pregame table gave (live report 2026-10-31).
#[test]
fn civ_icons_use_the_civilization_of_each_player() {
    let game = FakeGame {
        attach_script: "Game = { GetActivePlayer = function() return 0 end } \
            Players = {} \
            for i = 0, 3 do Players[i] = { GetCivilizationType = function() return 10 + i end } end"
            .into(),
        ..Default::default()
    };
    let r = rig_with_game(ExeConfig::default(), game);
    for i in 0..4 {
        assert_eq!(
            r.eval_str(&format!("return tostring(PreGame.GetCivilization({i}))")),
            (10 + i).to_string()
        );
    }
}

/// The turn handshake of a network game as the UI sees it: a human's turn complete coming or taken back raises
/// `RemotePlayerTurnEnd` (the players panel and the turn queue redraw) and `SerialEventEndTurnDirty`; the router's take-back ahead of an
/// order makes `Network.HasSentNetTurnComplete` false again; a load takes the seat's end of turn from the game.
#[test]
fn turn_complete_of_the_humans_reaches_the_ui() {
    let game = FakeGame {
        attach_script: "done = {} \
            Game = { IsNetworkMultiPlayer = function() return true end, GetActivePlayer = function() return 0 end, \
                     GetGameTurn = function() return 5 end } \
            GameDefines = { MAX_MAJOR_CIVS = 4 } \
            Players = {} \
            for i = 0, 3 do Players[i] = { IsHuman = function() return i < 2 end, \
                HasReceivedNetTurnComplete = function() return done[i] == true end } end"
            .into(),
        ..Default::default()
    };
    let mut r = rig_with_game(ExeConfig::default(), game);
    r.exec(
        "log = {} \
         Events.SerialEventEndTurnDirty.Add(function() log[#log + 1] = 'end' end) \
         Events.RemotePlayerTurnEnd.Add(function() log[#log + 1] = 'remote' end)",
    );
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "");
    // The other human ended the turn (the replica replayed the server's END_TURN).
    r.exec("done[1] = true");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "remote,end");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "remote,end");
    // An AI's flag is not the UI's business.
    r.exec("done[3] = true");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "remote,end");

    // The seat ended the turn, then the router took it back ahead of an order.
    r.exec("Network.SendTurnComplete()");
    assert!(r.eval_bool("return Network.HasSentNetTurnComplete()"));
    r.exe.dispatch(&r.rt);
    // The button is told at once ("Waiting for players"), as the exe's SetSentTurnComplete does.
    assert_eq!(r.log(), "remote,end,end");
    r.game.borrow_mut().reopened = true;
    r.exe.dispatch(&r.rt);
    assert!(!r.eval_bool("return Network.HasSentNetTurnComplete()"));
    assert_eq!(r.log(), "remote,end,end,end");

    // A load inside the turn where the seat has ended it: the UI and the router say so.
    r.exec("done[0] = true");
    r.game.borrow_mut().generation += 1;
    r.exe.dispatch(&r.rt);
    assert!(r.eval_bool("return Network.HasSentNetTurnComplete()"));
    assert_eq!(r.game.borrow().turn_sent, Some(true));
    assert_eq!(r.errors(), 0);
}

/// A kick (or a dropped connection) reaches the players panel: the seat that leaves the lobby's list of seats on line, or
/// stops being a human of the game, raises `MultiplayerGamePlayerDisconnected` and `MultiplayerGamePlayerUpdated` (MPList,
/// MPTurnPanel and EUI's civ list rebuild on them; the kicked seat stayed in the list until a reload, live report 2026-10-30).
#[test]
fn a_kicked_seat_leaves_the_players_panel() {
    let game = FakeGame {
        attach_script: "human = { [0] = true, [1] = true, [2] = true } \
            Game = { IsNetworkMultiPlayer = function() return true end, GetActivePlayer = function() return 0 end, \
                     GetGameTurn = function() return 5 end } \
            GameDefines = { MAX_MAJOR_CIVS = 4 } \
            Players = {} \
            for i = 0, 3 do Players[i] = { IsHuman = function() return human[i] == true end, \
                HasReceivedNetTurnComplete = function() return false end } end"
            .into(),
        ..Default::default()
    };
    let mut r = rig_with_game(ExeConfig::default(), game);
    *r.host.seats.borrow_mut() = exe_api::SeatInfo {
        local_id: 0,
        pings_ms: vec![(1, 30), (2, 30)],
        ..Default::default()
    };
    r.exec(
        "log = {} \
         Events.MultiplayerGamePlayerDisconnected.Add(function(p) log[#log + 1] = 'gone' .. p end) \
         Events.MultiplayerGamePlayerUpdated.Add(function(p) log[#log + 1] = 'upd' .. tostring(p) end)",
    );
    // The seats come on line (the rig's first frame saw none).
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "updnil");
    r.exec("log = {}");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "");
    // The host kicked player 2: the lobby drops the seat, then the replica gives the slot to the AI.
    r.host.seats.borrow_mut().pings_ms = vec![(1, 30)];
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "gone2,updnil");
    r.exec("human[2] = false");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "gone2,updnil,gone2,updnil");
    // Player 1 comes back after a drop: an update only.
    r.host.seats.borrow_mut().pings_ms = vec![];
    r.exe.dispatch(&r.rt);
    r.host.seats.borrow_mut().pings_ms = vec![(1, 40)];
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "gone2,updnil,gone2,updnil,gone1,updnil,updnil");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "gone2,updnil,gone2,updnil,gone1,updnil,updnil");
    assert_eq!(r.errors(), 0);
}

/// The kick reaches the other clients as the original's events and notification (items 17, 18): a seat that stops being a
/// human raises NOTIFICATION_PLAYER_KICKED (the replica's core adds it to the server seat only), `Network.IsPlayerKicked`,
/// and when no other player is left on line `MultiplayerGameLastPlayer`; a seat that only drops off line is a disconnect;
/// a host change raises `MultiplayerGameHostMigration`, a ping change `MultiplayerPingTimesChanged`; the kicked seat itself
/// gets `MultiplayerGameAbandoned(2)`.
#[test]
fn a_kick_and_a_drop_raise_the_notifications_and_events_of_the_original() {
    let game = FakeGame {
        attach_script: "human = { [0] = true, [1] = true, [2] = true }             Game = { IsNetworkMultiPlayer = function() return true end, GetActivePlayer = function() return 0 end,                      GetGameTurn = function() return 5 end }             GameDefines = { MAX_MAJOR_CIVS = 4 }             NotificationTypes = { NOTIFICATION_PLAYER_KICKED = 61, NOTIFICATION_PLAYER_DISCONNECTED = 62 }             Players = {}             for i = 0, 3 do Players[i] = { IsHuman = function() return human[i] == true end, GetNameKey = function() return 'P' .. i end,                 HasReceivedNetTurnComplete = function() return false end } end"
            .into(),
        ..Default::default()
    };
    let mut r = rig_with_game(ExeConfig::default(), game);
    *r.host.seats.borrow_mut() = exe_api::SeatInfo {
        local_id: 0,
        host_id: 0,
        pings_ms: vec![(1, 30), (2, 30)],
        ..Default::default()
    };
    r.exec(
        "log = {}          Events.NotificationAdded.Add(function(id, ty, tip, sum, gd) log[#log + 1] = 'note' .. ty .. ':' .. gd end)          Events.MultiplayerGameLastPlayer.Add(function() log[#log + 1] = 'last' end)          Events.MultiplayerGameAbandoned.Add(function(r) log[#log + 1] = 'abandoned' .. r end)          Events.MultiplayerGameHostMigration.Add(function(p) log[#log + 1] = 'host' .. p end)          Events.MultiplayerPingTimesChanged.Add(function() log[#log + 1] = 'ping' end)",
    );
    r.exe.dispatch(&r.rt);
    r.exec("log = {}");
    // Player 1 drops off line (still a human): a disconnect, not a kick.
    r.host.seats.borrow_mut().pings_ms = vec![(2, 30)];
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "note62:1");
    assert!(!r.eval_bool("return Network.IsPlayerKicked(1)"));
    // The host kicked player 2: the seat goes to the AI. Nobody else is on line: the last player.
    r.exec("log = {}");
    r.host.seats.borrow_mut().pings_ms = vec![];
    r.exec("human[2] = false");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "last,note61:2");
    assert!(r.eval_bool("return Network.IsPlayerKicked(2)"));
    // The host seat moves.
    r.exec("log = {}");
    r.host.seats.borrow_mut().host_id = 1;
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "host1");
    // The pings change (not more often than every two seconds of frames).
    r.exec("log = {}");
    r.host.seats.borrow_mut().pings_ms = vec![(1, 50)];
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "");
    for _ in 0..130 {
        r.exe.dispatch(&r.rt);
    }
    assert_eq!(r.log(), "ping");
    // The local seat is kicked: the game is abandoned with reason 2.
    r.exec("log = {} human[0] = false");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "abandoned2");
    assert_eq!(r.errors(), 0);
}

/// A human who takes a seat of a game in progress raises `MultiplayerHotJoinStarted` and, a look later,
/// `MultiplayerHotJoinCompleted`, both without arguments, on the other machines; `Network.IsPlayerHotJoining` is true in
/// between (RB10 item 4b). The UnitMember* events exist and nothing raises them (item 4a).
#[test]
fn a_hot_join_raises_started_and_completed() {
    let game = FakeGame {
        attach_script: "human = { [0] = true, [1] = true, [2] = false }             Game = { IsNetworkMultiPlayer = function() return true end, GetActivePlayer = function() return 0 end,                      GetGameTurn = function() return 5 end }             GameDefines = { MAX_MAJOR_CIVS = 3 }             Players = {}             for i = 0, 2 do Players[i] = { IsHuman = function() return human[i] == true end, GetNameKey = function() return 'P' .. i end,                 HasReceivedNetTurnComplete = function() return false end } end"
            .into(),
        ..Default::default()
    };
    let mut r = rig_with_game(ExeConfig::default(), game);
    *r.host.seats.borrow_mut() = exe_api::SeatInfo {
        local_id: 0,
        host_id: 0,
        pings_ms: vec![(1, 30)],
        ..Default::default()
    };
    r.exec(
        "log = {}          Events.MultiplayerHotJoinStarted.Add(function(...) log[#log + 1] = 'hs' .. select('#', ...) end)          Events.MultiplayerHotJoinCompleted.Add(function(...) log[#log + 1] = 'hc' .. select('#', ...) end)          Events.UnitMemberOverlayAdd.Add(function() log[#log + 1] = 'unitmember' end)",
    );
    // The rig has looked at the (empty) seat list already: seat 1 coming on line now is a join too, and is done a look later.
    r.exe.dispatch(&r.rt);
    r.exe.dispatch(&r.rt);
    r.exec("log = {}");
    r.host.seats.borrow_mut().pings_ms = vec![(1, 30), (2, 30)];
    r.exec("human[2] = true");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "hs0");
    assert!(
        r.eval_bool("return Network.IsPlayerHotJoining(2) and not Network.IsPlayerHotJoining(1)")
    );
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "hs0,hc0");
    assert!(!r.eval_bool("return Network.IsPlayerHotJoining(2)"));
    r.exe.dispatch(&r.rt);
    assert_eq!(
        r.log(),
        "hs0,hc0",
        "once per join; no UnitMember event is raised"
    );
    assert_eq!(r.errors(), 0);
}

/// Human seats get `AIProcessingStartedForPlayer` / `AIProcessingEndedForPlayer` as the original's `setTurnActive` does
/// (item 20), once each, from the replica's flags.
#[test]
fn humans_get_the_ai_processing_events() {
    let game = FakeGame {
        attach_script: "active = { [0] = false, [1] = false }             Game = { IsNetworkMultiPlayer = function() return true end, GetActivePlayer = function() return 0 end,                      GetGameTurn = function() return 5 end }             GameDefines = { MAX_MAJOR_CIVS = 2 }             Players = {}             for i = 0, 1 do Players[i] = { IsHuman = function() return true end, IsTurnActive = function() return active[i] end,                 HasReceivedNetTurnComplete = function() return false end } end"
            .into(),
        ..Default::default()
    };
    let mut r = rig_with_game(ExeConfig::default(), game);
    r.exec(
        "log = {}          Events.AIProcessingStartedForPlayer.Add(function(p) log[#log + 1] = 's' .. p end)          Events.AIProcessingEndedForPlayer.Add(function(p) log[#log + 1] = 'e' .. p end)",
    );
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "");
    r.exec("active[0] = true active[1] = true");
    r.exe.dispatch(&r.rt);
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "s0,s1");
    r.exec("active[1] = false");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "s0,s1,e1");
    assert_eq!(r.errors(), 0);
}

/// "Multiplayer Auto End Turn" (the options menu): with the option on and nothing holding the seat's turn, the turn ends
/// by itself after a while; a blocker or a ready selected unit holds it. The option is the client's store
/// (`civ5.options.MultiplayerAutoEndTurn`).
#[test]
fn multiplayer_auto_end_turn_ends_a_free_turn() {
    use platform::KeyValueStore;
    let game = FakeGame {
        attach_script: "st = { turn = 5, block = -1, can = true, ready = false } \
            Game = { IsNetworkMultiPlayer = function() return true end, GetActivePlayer = function() return 0 end, \
                     GetGameTurn = function() return st.turn end } \
            GameDefines = { MAX_MAJOR_CIVS = 2 } \
            EndTurnBlockingTypes = { NO_ENDTURN_BLOCKING_TYPE = -1 } \
            Players = {} \
            for i = 0, 1 do Players[i] = { IsHuman = function() return true end, IsAlive = function() return true end, \
                IsTurnActive = function() return true end, GetEndTurnBlockingType = function() return st.block end, \
                HasReceivedNetTurnComplete = function() return false end } end"
            .into(),
        ..Default::default()
    };
    let mut r = rig_with_game(ExeConfig::default(), game);
    r.exec(
        "UI.CanEndTurn = function() return st.can end \
         local unit = { GetOwner = function() return 0 end, IsReadyToMove = function() return st.ready end, \
                        IsAutomated = function() return false end } \
         UI.GetHeadSelectedUnit = function() if st.ready then return unit end end",
    );
    let sent = |r: &Rig| {
        r.game
            .borrow()
            .posted
            .iter()
            .filter(|m| m.name() == "sendTurnComplete")
            .count()
    };
    let frames = |r: &mut Rig, n: u32| {
        for _ in 0..n {
            r.exe.dispatch(&r.rt);
        }
    };
    // Off (the default): nothing.
    frames(&mut r, 100);
    assert_eq!(sent(&r), 0);
    // The options menu turned it on (the committed value in the store).
    r.exec("OptionsManager.SetMultiplayerAutoEndTurnEnabled_Cached(true) OptionsManager.CommitGameOptions()");
    assert_eq!(
        r.host
            .kv
            .get("civ5.options.MultiplayerAutoEndTurn")
            .as_deref(),
        Some("1")
    );
    // A blocker holds the turn, so does a selected unit that is ready for orders.
    r.exec("st.block = 3");
    frames(&mut r, 100);
    r.exec("st.block = -1 st.ready = true");
    frames(&mut r, 100);
    assert_eq!(sent(&r), 0);
    // A city that can shoot holds the turn while the option is on (CvNotifications.cpp:678, city range attack).
    r.exec(
        "st.shoot = true st.ready = false \
         Players[0].Cities = function() local n = 0 return function() n = n + 1 \
             if n == 1 then return { CanRangeStrikeNow = function() return st.shoot end } end end end",
    );
    frames(&mut r, 100);
    assert_eq!(sent(&r), 0);
    r.exec("st.shoot = false");
    // Part A: a unit moved a moment ago, so the turn waits 10 slices of 0.1 s after it (RB10 item 1: sixty frames of a
    // sixtieth of a second are ten slices, one slice at most per frame); then it ends, once.
    r.exe.note_unit_moved();
    frames(&mut r, 54);
    assert_eq!(sent(&r), 0);
    frames(&mut r, 20);
    assert_eq!(sent(&r), 1);
    assert!(r.eval_bool("return Network.HasSentNetTurnComplete()"));
    // The player takes it back and selects a unit to move: held, and the take-back latches the auto end for the rest
    // of the turn (SIMP-exe-700; the DLL keeps no latch): the turn does not end by itself again, whatever happens.
    r.exec("Network.SendTurnUnready() st.ready = true");
    frames(&mut r, 200);
    assert_eq!(sent(&r), 1);
    r.exec("st.ready = false");
    frames(&mut r, 200);
    assert_eq!(sent(&r), 1);
    // A new turn clears the latch: the free turn ends by itself again.
    r.exec("st.turn = 6");
    frames(&mut r, 200);
    assert_eq!(sent(&r), 2);
    // Part B: the seat gave an order, a unit stays selected (Part A holds), nothing blocks: three frames, then the end.
    r.exec("Network.SendTurnUnready() st.ready = true");
    frames(&mut r, 2);
    r.exec("st.turn = 7");
    frames(&mut r, 5);
    r.exe.note_unit_moved();
    r.exec("Network.SendPushMission(0, 1, 2, 3, 0, false, false)");
    frames(&mut r, 2);
    assert_eq!(sent(&r), 2);
    frames(&mut r, 6);
    assert_eq!(sent(&r), 3);
    // Turned off in the menu (not yet accepted: the exe reads the menu's cache): nothing more.
    r.exec(
        "Network.SendTurnUnready() OptionsManager.SetMultiplayerAutoEndTurnEnabled_Cached(false)",
    );
    frames(&mut r, 200);
    assert_eq!(sent(&r), 3);
    assert_eq!(r.errors(), 0);
}

#[test]
fn a_message_before_the_game_is_bound_waits_for_it() {
    use exe_api::{ExeApi, GameSide};
    use std::cell::RefCell;
    use std::rc::Rc;
    let host = Rc::new(fake::TestHost::default());
    let mut exe = ExeApi::new(ExeConfig::default(), host);
    let rt = lua_sys::Runtime::new(
        lua_sys::RuntimeConfig::default(),
        Rc::new(lua_sys::MemorySource::new()),
        exe.base_setup(),
    )
    .unwrap();
    exe.attach_runtime(&rt);
    let ctx = rt.create_context("T", |_, _| Ok(())).unwrap();
    ctx.exec("Network.SendResearch(1, 0, -1, false)", "=t")
        .unwrap();
    let game = Rc::new(RefCell::new(fake::FakeGame::default()));
    let dynamic: Rc<RefCell<dyn GameSide>> = game.clone();
    exe.bind_game(dynamic, ctx.lua());
    assert_eq!(game.borrow().posted.len(), 1);
}

#[test]
fn every_dll_net_message_has_a_lua_send_function() {
    use exe_api::network::{NET_MESSAGE_NAMES, lua_name};
    let r = rig();
    let mut n = 0;
    for dll in NET_MESSAGE_NAMES {
        if let Some(lua) = lua_name(dll) {
            assert!(
                r.eval_bool(&format!("return type(Network.{lua}) == 'function'")),
                "Network.{lua} for {dll}"
            );
            n += 1;
        }
    }
    assert!(n >= 58, "{n}");
    // The names the EUI files call (api.md B3) are all there.
    for name in [
        "SendDoTask",
        "SendChangeWar",
        "SendMoveSpy",
        "SendUpdatePolicies",
        "SendResearch",
        "SendIgnoreWarning",
        "SendLiberateMinor",
        "SendBarbarianRansom",
        "SendReturnCivilian",
        "SendUpdateCityCitizens",
        "SendCityBuyPlot",
        "SendSetCityAIFocus",
        "SendSetCityAvoidGrowth",
        "SendSellBuilding",
        "SendArchaeologyChoice",
        "SendFaithGreatPersonChoice",
        "SendGreatPersonChoice",
        "SendGoodyChoice",
        "SendIdeologyChoice",
        "SendMayaBonusChoice",
        "SendFoundPantheon",
        "SendFoundReligion",
        "SendEnhanceReligion",
        "SendMinorNoUnitSpawning",
        "SendMoveGreatWorks",
        "SendSwapGreatWorks",
        "SendSetSwappableGreatWork",
        "SendDiploVote",
        "SendStageCoup",
        "SendLeagueEditName",
        "SendPledgeMinorProtection",
    ] {
        assert!(
            r.eval_bool(&format!("return type(Network.{name}) == 'function'")),
            "{name}"
        );
    }
}

// ---- InterfaceBuddy ----

const POPUP_LISTEN: &str = "log = {}
    Events.SerialEventGameMessagePopup.Add(function(info)
        table.insert(log, 'popup' .. info.Type .. ':' .. tostring(info.Data1) .. ':' .. tostring(info.Option1) .. ':' .. tostring(info.Text)) end)";

fn add_popup(r: &Rig, ty: i32, data1: i32) {
    // POPUP: type, data1, data2, data3, option1, option2, player; text
    r.game
        .borrow_mut()
        .events
        .push(ev(K::Popup, &[ty, data1, 0, 0, 1, 0, 0]).with_text("txt"));
}

#[test]
fn one_popup_per_type_the_next_waits_for_processed() {
    let mut r = rig();
    r.exec(POPUP_LISTEN);
    add_popup(&r, 5, 100);
    add_popup(&r, 5, 101);
    add_popup(&r, 6, 200);
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "popup5:100:true:txt,popup6:200:true:txt");
    assert!(r.eval_bool("return UI.IsPopupUp() and UI.IsPopupTypeOpen(5) and UI.IsPopupTypeOpen(6) and not UI.IsPopupTypeOpen(7)"));
    // The first popup of type 5 is processed: the waiting one is shown.
    r.exec("Events.SerialEventGameMessagePopupProcessed.CallImmediate(5, 0)");
    r.exe.dispatch(&r.rt);
    assert_eq!(
        r.log(),
        "popup5:100:true:txt,popup6:200:true:txt,popup5:101:true:txt"
    );
    // Both are processed: the type closes.
    r.exec("Events.SerialEventGameMessagePopupProcessed.CallImmediate(5, 0)");
    r.exec("Events.SerialEventGameMessagePopupProcessed.CallImmediate(6, 0)");
    assert!(!r.eval_bool("return UI.IsPopupUp()"));
    // A new popup of type 5 is shown at once.
    add_popup(&r, 5, 102);
    r.exe.dispatch(&r.rt);
    assert!(r.log().ends_with("popup5:102:true:txt"));
}

#[test]
fn the_city_screen_exit_closes_the_choose_production_popup() {
    // CityView.lua: Processed, then Shown (the type is open), and no Processed when the screen goes down.
    let mut r = rig();
    r.exec(POPUP_LISTEN);
    r.exec(
        "Events.SerialEventGameMessagePopup.Add(function(i) if i.Type == 16 then
            Events.SerialEventGameMessagePopupProcessed.CallImmediate(16, 0) Events.SerialEventGameMessagePopupShown(i) end end)",
    );
    add_popup(&r, 16, 1);
    r.exe.dispatch(&r.rt);
    assert!(r.eval_bool("return UI.IsPopupTypeOpen(16)"));
    // The next one waits behind the open type.
    add_popup(&r, 16, 2);
    r.exe.dispatch(&r.rt);
    assert!(!r.log().contains("popup16:2"));
    // The screen goes down: the popup is processed and the waiting one is shown.
    r.exec("Events.SerialEventExitCityScreen()");
    r.exe.dispatch(&r.rt);
    assert!(r.log().contains("popup16:2"));
}

#[test]
fn stack_types_open_several_times() {
    let mut r = rig_with(ExeConfig {
        stack_popup_types: vec![9],
        ..ExeConfig::default()
    });
    r.exec(POPUP_LISTEN);
    add_popup(&r, 9, 1);
    add_popup(&r, 9, 2);
    r.exe.register_popup_type(8, true);
    add_popup(&r, 8, 3);
    add_popup(&r, 8, 4);
    r.exe.dispatch(&r.rt);
    assert_eq!(
        r.log(),
        "popup9:1:true:txt,popup9:2:true:txt,popup8:3:true:txt,popup8:4:true:txt"
    );
}

#[test]
fn a_popup_made_by_a_lua_context_counts_as_open_once_it_says_shown() {
    let mut r = rig();
    r.exec(POPUP_LISTEN);
    // A context that fires the popup event itself, then tells it is on the screen.
    r.exec("local info = { Type = 4, Data1 = 1 } Events.SerialEventGameMessagePopupShown(info)");
    r.exe.dispatch(&r.rt);
    assert!(r.eval_bool("return UI.IsPopupTypeOpen(4)"));
    add_popup(&r, 4, 7);
    r.exe.dispatch(&r.rt);
    // Waiting: not shown yet.
    assert_eq!(r.log(), "");
    r.exec("Events.SerialEventGameMessagePopupProcessed.CallImmediate(4, 0)");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "popup4:7:true:txt");
}

#[test]
fn dont_show_mode_delays_and_shows_in_order_of_arrival() {
    let mut r = rig_with(ExeConfig {
        dont_show_popups: true,
        stack_popup_types: vec![3],
        ..ExeConfig::default()
    });
    r.exec(POPUP_LISTEN);
    add_popup(&r, 1, 10);
    add_popup(&r, 2, 20);
    add_popup(&r, 3, 30);
    add_popup(&r, 3, 31);
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "");
    r.exe.set_dont_show_popups(false);
    r.exe.dispatch(&r.rt);
    // The first (nothing is open), then only stack types; type 2 stays delayed.
    assert_eq!(
        r.log(),
        "popup1:10:true:txt,popup3:30:true:txt,popup3:31:true:txt"
    );
}

#[test]
fn ui_add_popup_from_lua_goes_through_the_queue() {
    let mut r = rig();
    r.exec(POPUP_LISTEN);
    r.exec(
        "UI.AddPopup{ Type = ButtonPopupTypes_X or 12, Data1 = 5, Option1 = true, Text = 'hi' }",
    );
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "popup12:5:true:hi");
}

#[test]
fn a_popup_array_carries_its_ints_and_bools() {
    let mut r = rig();
    r.exec(POPUP_LISTEN);
    // POPUP_ARRAY: type, n ints, the ints, n bools, the bools
    r.game
        .borrow_mut()
        .events
        .push(ev(K::PopupArray, &[7, 2, 70, 71, 1, 0]));
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "popup7:70:false:nil");
}

#[test]
fn a_new_head_deselects_the_old_one_at_its_plot() {
    let mut r = rig();
    dll_stand_in(&r);
    r.exec(
        "log = {}
         Events.UnitSelectionChanged.Add(function(p, u, i, j, k, sel) table.insert(log, p .. ':' .. u .. '@' .. i .. ',' .. j .. ',' .. k .. ':' .. tostring(sel)) end)
         Events.UnitSelectionCleared.Add(function() table.insert(log, 'cleared') end)",
    );
    // Unit 0:42 stands on grid (4, 2), hex (3, 2).
    r.game.borrow_mut().events.push(ev(
        K::UnitCreated,
        &[0, 42, 4, 2, 7, 1, 2, 3, 4, 5, 2, 0, 1, 1],
    ));
    r.exe.dispatch(&r.rt);
    r.exec("UI.SelectUnit(MyUnit) UI.SelectUnit(Players[0]:GetUnitByID(43))");
    r.exe.dispatch(&r.rt);
    // 0:43 is not known to the UI: its hex is 0, 0.
    assert_eq!(
        r.log(),
        "0:42@3,2,-5:true,0:42@3,2,-5:false,0:43@0,0,0:true"
    );
}

#[test]
fn the_ui_cannot_change_the_saved_game_without_a_message() {
    // Live 2026-10-30 (m5-map-sync): WorldView.lua's right-click move calls Game.SetEverRightClickMoved, a saved field of
    // CvGame the exe set on its own machine only; in the replica it made the game differ from the server's, the next
    // checked digest forced a resync. Such calls change nothing now (SIMP-exe-66); the rest of the DLL's API is kept.
    let r = rig_with_game(
        ExeConfig::default(),
        FakeGame {
            attach_script: "applied = {}
                local function rec(n) return function() table.insert(applied, n) end end
                Game = { SetEverRightClickMoved = rec('rclick'), SetAdvisorMessageHasBeenSeen = rec('advisor'),
                         SetStaticTutorialActive = rec('tutorial'), SetPausePlayer = rec('pause'),
                         SelectionListMove = rec('move'), CityPushOrder = rec('order') }
                Player = { AddNotification = rec('notification'), GetID = function() return 0 end }
                Players = { [0] = setmetatable({}, { __index = Player }) }"
                .to_owned(),
            ..Default::default()
        },
    );
    r.exec(
        "Game.SetEverRightClickMoved(true)
         Game.SetAdvisorMessageHasBeenSeen('TXT_KEY_X', true)
         Game.SetStaticTutorialActive(true)
         Game.SetPausePlayer(0)
         Players[0]:AddNotification(1, 'a', 'b', 0, 0, -1, -1)
         Game.SelectionListMove(nil, false, false, false)
         Game.CityPushOrder(nil, 0, 1, false, false, true)",
    );
    assert_eq!(
        r.eval_str("return table.concat(applied, ',')"),
        "move,order"
    );
    assert_eq!(r.eval_str("return tostring(Players[0]:GetID())"), "0");
    assert_eq!(r.errors(), 0);
}

/// GameInfo and the unit API for the sound lookups: unit 0:42 is a UNIT_ARCHER, unit 0:43 a UNIT_NOSOUND, and the
/// seat is player 0 (`dll_stand_in`'s `Game` answers 2, so it is replaced).
fn sound_stand_in(r: &Rig) {
    r.ctx
        .lua()
        .exec(
            "local types = { [42] = 7, [43] = 8, [44] = 7 }
             local function unit(o, id) return { GetUnitType = function() return types[id] end } end
             Players = setmetatable({}, { __index = function(t, o)
                 return { GetUnitByID = function(self, id) return unit(o, id) end,
                          GetCityByID = function(self, id) return { GetPopulation = function() return 15 end } end,
                          GetCurrentEra = function() return 3 end }
             end })
             Game = { GetActivePlayer = function() return 0 end }
             local rows = { UNIT_ARCHER = { SelectionSound = 'AS2D_SELECT_ARCHER', FirstSelectionSound = 'AS2D_BIRTH_ARCHER' } }
             GameInfo = { Units = { [7] = { Type = 'UNIT_ARCHER' }, [8] = { Type = 'UNIT_NOSOUND' } },
                          Eras = { [3] = { Type = 'ERA_RENAISSANCE' } },
                          UnitGameplay2DScripts = setmetatable({}, { __call = function(t, filter)
                              local row = rows[filter.UnitType]
                              local done = false
                              return function() if not done then done = true return row end end
                          end }) }",
            "=sounds",
        )
        .unwrap();
}

#[test]
fn selecting_an_own_unit_plays_its_selection_sound() {
    let mut r = rig();
    sound_stand_in(&r);
    let sel = |r: &mut Rig, id: i32| {
        r.game
            .borrow_mut()
            .events
            .push(ev(K::UnitSelectionChanged, &[1, 0, id]));
        r.exe.dispatch(&r.rt);
    };
    // The first selection of a unit plays its first-selection (birth) script, the later ones the selection script.
    sel(&mut r, 42);
    sel(&mut r, 44);
    sel(&mut r, 42);
    // A unit whose type has no row makes the exe's default sound (SIMP-exe-970); another player's unit is silent.
    sel(&mut r, 43);
    r.game
        .borrow_mut()
        .events
        .push(ev(K::UnitSelectionChanged, &[1, 1, 42]));
    r.exe.dispatch(&r.rt);
    assert_eq!(
        *r.host.unit_sounds.borrow(),
        [
            "AS2D_BIRTH_ARCHER",
            "AS2D_BIRTH_ARCHER",
            "AS2D_SELECT_ARCHER",
            "AS2D_BUILD_UNIT"
        ]
    );
}

/// The DLL's ever-selected counter (`CvUnit:IsFirstTimeSelected`, exposed by the core of this build) decides which of the
/// two scripts plays, not the exe-api's own count: a unit selected before the page was reloaded is not new.
#[test]
fn the_dll_counter_picks_the_first_selection_sound() {
    let mut r = rig();
    sound_stand_in(&r);
    r.ctx
        .lua()
        .exec(
            "FIRST = { [42] = false, [44] = true }
             local old = Players
             Players = setmetatable({}, { __index = function(t, o)
                 local P = old[o]
                 return { GetUnitByID = function(self, id)
                     local u = P:GetUnitByID(id)
                     u.IsFirstTimeSelected = function() return FIRST[id] end
                     return u
                 end, GetCityByID = P.GetCityByID, GetCurrentEra = P.GetCurrentEra }
             end })",
            "=first",
        )
        .unwrap();
    for id in [42, 44] {
        r.game
            .borrow_mut()
            .events
            .push(ev(K::UnitSelectionChanged, &[1, 0, id]));
        r.exe.dispatch(&r.rt);
    }
    // Unit 42 was selected before (counter 2): the plain script; unit 44 is on its first selection.
    assert_eq!(
        *r.host.unit_sounds.borrow(),
        ["AS2D_SELECT_ARCHER", "AS2D_BIRTH_ARCHER"]
    );
}

/// The files of a unit type's selection sounds are asked for when the seat has such a unit on the map, once per type,
/// so the first click on the unit is heard (live report 2026-11-01: selection sounds missing).
#[test]
fn the_selection_sounds_of_the_seats_unit_types_are_fetched_ahead() {
    let mut r = rig();
    sound_stand_in(&r);
    let created =
        |owner: i32, id: i32| with_block(K::UnitCreated, unit_block(owner, id, 5, 5), &[]);
    {
        let mut g = r.game.borrow_mut();
        // Another player's unit of the type asks for nothing (and does not use the type up).
        g.events.push(created(3, 43));
        g.events.push(created(0, 42));
        g.events.push(created(0, 44));
    }
    r.exe.dispatch(&r.rt);
    assert_eq!(
        *r.host.sound_preloads.borrow(),
        ["AS2D_SELECT_ARCHER", "AS2D_BIRTH_ARCHER"]
    );
}

#[test]
fn the_city_screen_plays_the_ambience_of_the_era_and_size() {
    let mut r = rig();
    sound_stand_in(&r);
    r.game.borrow_mut().head_city = Some((0, 7));
    r.game
        .borrow_mut()
        .events
        .push(ev(K::CityScreenChanged, &[1]));
    r.exe.dispatch(&r.rt);
    r.game
        .borrow_mut()
        .events
        .push(ev(K::CityScreenChanged, &[0]));
    r.exe.dispatch(&r.rt);
    assert_eq!(
        *r.host.ambience.borrow(),
        [
            Some("AS2D_AMBIENCE_CITY_ERA_RENAISSANCE_LARGE".to_owned()),
            None
        ]
    );
}

#[test]
fn the_ambience_ends_when_a_script_exits_the_city_screen() {
    let mut r = rig();
    sound_stand_in(&r);
    r.game.borrow_mut().head_city = Some((0, 7));
    r.game
        .borrow_mut()
        .events
        .push(ev(K::CityScreenChanged, &[1]));
    r.exe.dispatch(&r.rt);
    // RETURN / Esc of the city screen: CityView.lua fires the exit itself.
    r.exec("Events.SerialEventExitCityScreen()");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.host.ambience.borrow().last(), Some(&None));
}

#[test]
fn ambience_names() {
    use exe_api::city_ambience_name as n;
    let s = |x: Option<String>| x.unwrap_or_default();
    assert_eq!(
        s(n("ERA_RENAISSANCE", 7)),
        "AS2D_AMBIENCE_CITY_ERA_RENAISSANCE_SMALL"
    );
    assert_eq!(
        s(n("ERA_ANCIENT", 0)),
        "AS2D_AMBIENCE_CITY_ERA_ANCIENT_SMALL"
    );
    assert_eq!(
        s(n("ERA_ANCIENT", 13)),
        "AS2D_AMBIENCE_CITY_ERA_ANCIENT_SMALL"
    );
    assert_eq!(
        s(n("ERA_MODERN", 14)),
        "AS2D_AMBIENCE_CITY_ERA_MODERN_LARGE"
    );
    assert_eq!(s(n("ERA_FUTURE", 3)), "AS2D_AMBIENCE_CITY_ERA_FUTURE_SMALL");
    // The game has no script for the post-modern era: the modern one plays (SIMP-exe-1000).
    assert_eq!(
        s(n("ERA_POSTMODERN", 20)),
        "AS2D_AMBIENCE_CITY_ERA_MODERN_LARGE"
    );
    assert_eq!(n("ERA_NOPE", 20), None);
}

/// The seat's units for the auto-cycle probe: `ready[id]` says whether unit `id` can take orders (a unit exists while it has an
/// entry there), `gone[id]` that it is dead; the head selection is `head` (a unit id or nil); `cycled` counts what the cycle
/// did (`UI.SelectUnit` of the next unit or `UI.ClearSelectionList`), never `Game.CycleUnits` (SIMP-exe-634).
fn cycle_game(option: bool) -> FakeGame {
    FakeGame {
        attach_script: format!(
            "S = {{ ready = {{}}, gone = {{}}, cycled = 0, picked = {{}} }}
             local function unit(id) return {{ GetOwner = function() return 0 end, GetID = function() return id end,
                 GetX = function() return 0 end, GetY = function() return 0 end,
                 IsReadyToMove = function() return S.ready[id] end, IsAutomated = function() return false end }} end
             local me = {{ IsHuman = function() return true end, IsAlive = function() return true end,
                 IsTurnActive = function() return true end,
                 GetUnitByID = function(self, id) if S.gone[id] then return nil end return unit(id) end,
                 Units = function(self)
                     local ids = {{}}
                     for id in pairs(S.ready) do if not S.gone[id] then ids[#ids + 1] = id end end
                     table.sort(ids)
                     local i = 0
                     return function() i = i + 1 return ids[i] and unit(ids[i]) end
                 end }}
             Players = {{ [0] = me }}
             Map = {{ PlotDistance = function() return 0 end }}
             Game = {{ GetActivePlayer = function() return 0 end,
                 CycleUnits = function() error('Game.CycleUnits changes the game (SIMP-exe-634)') end }}
             OptionsManager = {{ GetAutoUnitCycle = function() return {option} end }}
             Network = {{ HasSentNetTurnComplete = function() return false end }}
             UI = {{ GetHeadSelectedUnit = function() return S.head and unit(S.head) end,
                 SelectUnit = function(u) S.cycled = S.cycled + 1 S.head = u:GetID() S.picked[#S.picked + 1] = u:GetID() end,
                 ClearSelectionList = function() S.cycled = S.cycled + 1 S.head = nil end }}"
        ),
        ..Default::default()
    }
}
#[test]
fn the_exe_selects_the_next_unit_when_the_head_unit_has_no_orders_left() {
    let mut r = rig_with_game(ExeConfig::default(), cycle_game(true));
    let cycled = |r: &Rig| r.eval_str("return tostring(S.cycled)");
    let frame = |r: &mut Rig| r.exe.dispatch(&r.rt);
    // A unit with moves is selected: nothing happens.
    r.exec("S.ready[1] = true S.head = 1");
    frame(&mut r);
    assert_eq!(cycled(&r), "0");
    // Its moves are used up: the next unit is selected, once.
    r.exec("S.ready[1] = false");
    frame(&mut r);
    frame(&mut r);
    assert_eq!(cycled(&r), "1");
    // A unit the player selected by hand without orders to give (fortified) stays selected.
    r.exec("S.head = 2 S.ready[2] = false");
    frame(&mut r);
    assert_eq!(cycled(&r), "1");
    // A unit with moves that the player then clears is not replaced...
    r.exec("S.ready[3] = true S.head = 3");
    frame(&mut r);
    r.exec("S.head = nil");
    frame(&mut r);
    assert_eq!(cycled(&r), "1");
    // ... but one that died is.
    r.exec("S.ready[4] = true S.head = 4");
    frame(&mut r);
    r.exec("S.gone[4] = true S.head = nil");
    frame(&mut r);
    assert_eq!(cycled(&r), "2");
    assert_eq!(r.errors(), 0);
}

#[test]
fn the_unit_cycle_follows_the_option() {
    let mut r = rig_with_game(ExeConfig::default(), cycle_game(false));
    r.exec("S.ready[1] = true S.head = 1");
    r.exe.dispatch(&r.rt);
    r.exec("S.ready[1] = false");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.eval_str("return tostring(S.cycled)"), "0");
    assert_eq!(r.errors(), 0);
}

/// The start of a turn (a load of the turn's snapshot) selects the next unit on the first free frame when nothing ready
/// is selected (the exe arms its cycle counter at setTurnActive; live report 2026-10-31: the first unit came late).
#[test]
fn the_turn_start_selects_a_unit_at_once() {
    let mut r = rig_with_game(ExeConfig::default(), cycle_game(true));
    let cycled = |r: &Rig| r.eval_str("return tostring(S.cycled)");
    let frame = |r: &mut Rig| r.exe.dispatch(&r.rt);
    r.exec("S.ready[5] = true");
    frame(&mut r);
    // The rig's own first load had nothing selected: the first free frame selected the ready unit.
    assert_eq!(r.eval_str("return tostring(S.head)"), "5");
    assert_eq!(cycled(&r), "1");
    // A new turn with a unit selected that is ready: it stays.
    r.exec("S.ready[1] = true S.head = 1");
    r.game.borrow_mut().generation += 1;
    frame(&mut r);
    frame(&mut r);
    assert_eq!(cycled(&r), "1");
    // A new turn with a selected unit that has nothing to do (or none): the next unit at once.
    r.exec("S.ready[1] = false");
    r.game.borrow_mut().generation += 1;
    frame(&mut r);
    frame(&mut r);
    assert_eq!(cycled(&r), "2");
    // A turn that starts while the city screen is up waits for the screen to close (unit 1 is left, none is ready).
    r.exec("S.ready[5] = false S.head = 1");
    r.game.borrow_mut().generation += 1;
    r.game.borrow_mut().city_screen = true;
    frame(&mut r);
    frame(&mut r);
    frame(&mut r);
    assert_eq!(cycled(&r), "2");
    r.game.borrow_mut().city_screen = false;
    frame(&mut r);
    assert_eq!(cycled(&r), "3");
    assert_eq!(r.errors(), 0);
}

/// The seat's own orders on their way hold the cycle: the replica shows a unit just ordered as ready until the journal
/// brings the order back, so the cycle must not pick it (live report 2026-10-31: the unit that had moved was selected).
#[test]
fn the_unit_cycle_waits_for_the_seats_own_orders() {
    let mut r = rig_with_game(ExeConfig::default(), cycle_game(true));
    let cycled = |r: &Rig| r.eval_str("return tostring(S.cycled)");
    let frame = |r: &mut Rig| r.exe.dispatch(&r.rt);
    r.exec("S.ready[1] = true S.head = 1");
    frame(&mut r);
    frame(&mut r);
    let base = cycled(&r);
    // Unit 1 was ordered: while the order is out nothing cycles, even when the unit already looks done.
    r.game.borrow_mut().own_pending = true;
    r.exec("S.ready[1] = false");
    frame(&mut r);
    frame(&mut r);
    assert_eq!(cycled(&r), base);
    // The order is back: the unit is done, the next one is selected.
    r.game.borrow_mut().own_pending = false;
    frame(&mut r);
    assert_ne!(cycled(&r), base);
    let base = cycled(&r);
    // The player moved unit 2 and picked unit 3 by hand while the order was out: unit 3 stays.
    r.exec("S.ready[2] = true S.head = 2");
    frame(&mut r);
    r.game.borrow_mut().own_pending = true;
    r.exec("S.ready[3] = true S.head = 3");
    frame(&mut r);
    r.exec("S.ready[2] = false");
    r.game.borrow_mut().own_pending = false;
    frame(&mut r);
    frame(&mut r);
    assert_eq!(cycled(&r), base);
    assert_eq!(r.errors(), 0);
}

/// A load (the turn's snapshot) raises SerialEventUnitInfoDirty: EUI's unit panel showed the last turn for a unit kept
/// selected across the end of turn (live report 2026-10-31).
#[test]
fn a_load_refreshes_the_unit_panel() {
    let mut r = rig();
    r.exec(
        "log = {} Events.SerialEventUnitInfoDirty.Add(function() table.insert(log, 'unit') end)",
    );
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "");
    r.game.borrow_mut().generation += 1;
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "unit");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "unit");
    assert_eq!(r.errors(), 0);
}

/// "Waiting for players": the seat ended the turn, another human has not. `Network.HasSentNetTurnAllComplete` stays false
/// (ActionInfoPanel keeps the button enabled) and `Network.SendTurnUnready` takes the end of turn back and answers true;
/// once every human has ended it, the end of turn can no longer be taken back (live report 2026-10-31).
#[test]
fn waiting_for_players_can_be_taken_back_until_everybody_is_done() {
    let game = FakeGame {
        attach_script: "done = {} \
            Game = { IsNetworkMultiPlayer = function() return true end, GetActivePlayer = function() return 0 end, \
                     GetGameTurn = function() return 5 end } \
            GameDefines = { MAX_MAJOR_CIVS = 4 } \
            Players = {} \
            for i = 0, 3 do Players[i] = { IsHuman = function() return i < 2 end, \
                HasReceivedNetTurnComplete = function() return done[i] == true end } end"
            .into(),
        ..Default::default()
    };
    let mut r = rig_with_game(ExeConfig::default(), game);
    r.exe.dispatch(&r.rt);
    r.exec("Network.SendTurnComplete() done[0] = true");
    r.exe.dispatch(&r.rt);
    assert!(r.eval_bool("return Network.HasSentNetTurnComplete()"));
    assert!(!r.eval_bool("return Network.HasSentNetTurnAllComplete()"));
    assert!(r.eval_bool("return Network.SendTurnUnready() == true"));
    assert!(!r.eval_bool("return Network.HasSentNetTurnComplete()"));
    let names: Vec<_> = r
        .game
        .borrow()
        .posted
        .iter()
        .map(|m| m.name().to_owned())
        .collect();
    assert_eq!(names, ["sendTurnComplete", "sendTurnUnready"]);
    // Ended again, and now the other human too: too late to take it back.
    r.exec("Network.SendTurnComplete() done[1] = true");
    r.exe.dispatch(&r.rt);
    assert!(r.eval_bool("return Network.HasSentNetTurnAllComplete()"));
    assert!(r.eval_bool("return Network.SendTurnUnready() == false"));
    assert!(r.eval_bool("return Network.HasSentNetTurnComplete()"));
    assert_eq!(r.game.borrow().posted.len(), 3);
    assert_eq!(r.errors(), 0);
}

/// The server's AI turn log with `started` civilizations (ids 2..) started and the first `ended` of them ended.
fn ai_log(turn: i32, started: i32, ended: i32) -> exe_api::AiTurnLog {
    let mut events: Vec<(i32, bool)> = (2..2 + started).map(|p| (p, false)).collect();
    events.extend((2..2 + ended).map(|p| (p, true)));
    exe_api::AiTurnLog { turn, events }
}

/// While the server plays the AI phase the end-turn button says how far it is (SIMP-exe-790): the label `EndTurnText` of the
/// original ActionInfoPanel gets "Processing AI turns: N of M" (N civilizations ended, M started) once every human has ended
/// the turn and the server said how far it is; when the progress ends the button draws its own text again
/// (`SerialEventEndTurnDirty`).
#[test]
fn the_end_turn_button_says_how_far_the_ai_is() {
    let game = FakeGame {
        attach_script: "done = {}             Game = { IsNetworkMultiPlayer = function() return true end, GetActivePlayer = function() return 0 end,                      GetGameTurn = function() return 5 end }             GameDefines = { MAX_MAJOR_CIVS = 2 }             Players = {}             for i = 0, 1 do Players[i] = { IsHuman = function() return true end,                 HasReceivedNetTurnComplete = function() return done[i] == true end } end             label = { text = 'Please Wait', GetText = function(self) return self.text end,                       SetText = function(self, t) self.text = t end }             __ui_find_label = function(suffix) if suffix == '/EndTurnText' then return label end end             dirty = 0             Events.SerialEventEndTurnDirty.Add(function() dirty = dirty + 1 end)"
            .into(),
        ..Default::default()
    };
    let mut r = rig_with_game(ExeConfig::default(), game);
    r.exe.dispatch(&r.rt);
    // Nobody has ended the turn: the server's number would be stale, nothing is shown.
    *r.host.ai_turn.borrow_mut() = Some(ai_log(5, 9, 2));
    r.exe.dispatch(&r.rt);
    assert!(r.eval_bool("return label.text == 'Please Wait'"));
    // Both ended the turn: the processing state; the progress is the button's text.
    r.exec("Network.SendTurnComplete() done[0] = true done[1] = true");
    r.exe.dispatch(&r.rt);
    assert!(r.eval_bool("return label.text == 'Processing AI turns: 2 of 9'"));
    *r.host.ai_turn.borrow_mut() = Some(ai_log(5, 9, 5));
    r.exe.dispatch(&r.rt);
    assert!(r.eval_bool("return label.text == 'Processing AI turns: 5 of 9'"));

    // The original's own redraw puts its text back: it is replaced again within 15 frames.
    r.exec("label.text = 'Please Wait'");
    run_frames(&mut r, 16);
    assert!(r.eval_bool("return label.text == 'Processing AI turns: 5 of 9'"));
    // The progress ends: the button is told to draw its own text.
    let before = r.eval_str("return tostring(dirty)");
    *r.host.ai_turn.borrow_mut() = None;
    r.exe.dispatch(&r.rt);
    r.exe.dispatch(&r.rt);
    assert_ne!(
        r.eval_str("return tostring(dirty)"),
        before,
        "SerialEventEndTurnDirty"
    );
    assert_eq!(r.errors(), 0);
}

/// A log of the turn before is not the progress of this turn (the old count must not show while the next turn's first
/// civilization has not been started yet).
#[test]
fn the_ai_progress_of_another_turn_is_not_shown() {
    let game = FakeGame {
        attach_script: "done = {}             Game = { IsNetworkMultiPlayer = function() return true end, GetActivePlayer = function() return 0 end,                      GetGameTurn = function() return 6 end }             GameDefines = { MAX_MAJOR_CIVS = 2 }             Players = {}             for i = 0, 1 do Players[i] = { IsHuman = function() return true end,                 HasReceivedNetTurnComplete = function() return done[i] == true end } end             label = { text = 'Please Wait', GetText = function(self) return self.text end,                       SetText = function(self, t) self.text = t end }             __ui_find_label = function(suffix) if suffix == '/EndTurnText' then return label end end"
            .into(),
        ..Default::default()
    };
    let mut r = rig_with_game(ExeConfig::default(), game);
    r.exe.dispatch(&r.rt);
    r.exec("Network.SendTurnComplete() done[0] = true done[1] = true");
    *r.host.ai_turn.borrow_mut() = Some(ai_log(5, 9, 9));
    run_frames(&mut r, 3);
    assert!(r.eval_bool("return label.text == 'Please Wait'"));
    *r.host.ai_turn.borrow_mut() = Some(ai_log(6, 4, 1));
    run_frames(&mut r, 3);
    assert!(r.eval_bool("return label.text == 'Processing AI turns: 1 of 4'"));
    assert_eq!(r.errors(), 0);
}

/// The AI turn queue (SIMP-exe-820): the server's started / ended civilizations become `Events.AIProcessingStartedForPlayer` /
/// `AIProcessingEndedForPlayer(id, "")` in order, and `Player:IsTurnActive` of the replica reads true for a started AI and
/// false for an ended one when the handler runs (the overlay flips first); a human is untouched. The next load takes the
/// overlay away. Events already told are not told again.
#[test]
fn the_ai_turn_queue_raises_the_events_in_order_and_the_overlay_flips_is_turn_active_first() {
    let game = FakeGame {
        attach_script: "trace = {}             Game = { IsNetworkMultiPlayer = function() return true end, GetActivePlayer = function() return 0 end,                      GetGameTurn = function() return 5 end }             GameDefines = { MAX_MAJOR_CIVS = 4 }             Player = { GetID = function(p) return p.id end, IsHuman = function(p) return p.id == 0 end,                        IsTurnActive = function(p) return p.id == 0 end, HasReceivedNetTurnComplete = function() return false end }             Players = {}             for i = 0, 3 do Players[i] = setmetatable({ id = i }, { __index = Player }) end             Events.AIProcessingStartedForPlayer.Add(function(id, tag) table.insert(trace, 'S' .. id .. (Players[id]:IsTurnActive() and 'T' or 'F') .. '[' .. tostring(tag) .. ']') end)             Events.AIProcessingEndedForPlayer.Add(function(id, tag) table.insert(trace, 'E' .. id .. (Players[id]:IsTurnActive() and 'T' or 'F')) end)"
            .into(),
        ..Default::default()
    };
    let mut r = rig_with_game(ExeConfig::default(), game);
    r.exe.dispatch(&r.rt);
    let say = |r: &Rig| r.eval_str("return table.concat(trace, ' ')");
    let log = |events: &[(i32, bool)]| exe_api::AiTurnLog {
        turn: 5,
        events: events.to_vec(),
    };
    // Dark before anything: the replica's own flags (the AIs are done while the humans play).
    assert!(r.eval_bool("return not Players[1]:IsTurnActive() and not Players[3]:IsTurnActive()"));
    // The burst: every AI is lit, one event each, in the server's order.
    *r.host.ai_turn.borrow_mut() = Some(log(&[(1, false), (2, false), (3, false)]));
    r.exe.dispatch(&r.rt);
    assert_eq!(say(&r), "S1T[] S2T[] S3T[]");
    assert!(r.eval_bool("return Players[2]:IsTurnActive() and Players[0]:IsTurnActive()"));
    // Told once: the same log again raises nothing.
    r.exe.dispatch(&r.rt);
    assert_eq!(say(&r), "S1T[] S2T[] S3T[]");
    // They go dark one by one.
    *r.host.ai_turn.borrow_mut() = Some(log(&[(1, false), (2, false), (3, false), (1, true)]));
    r.exe.dispatch(&r.rt);
    assert_eq!(say(&r), "S1T[] S2T[] S3T[] E1F");
    assert!(r.eval_bool("return not Players[1]:IsTurnActive() and Players[2]:IsTurnActive()"));
    // The next turn's state is loaded: the overlay is gone, the loaded flags speak (all AIs done).
    r.game.borrow_mut().generation += 1;
    r.exe.dispatch(&r.rt);
    assert!(r.eval_bool("return not Players[2]:IsTurnActive() and not Players[3]:IsTurnActive()"));
    // A human is not touched by it.
    assert!(r.eval_bool("return Players[0]:IsTurnActive()"));
    assert_eq!(r.errors(), 0);
}

fn auto_end_rig() -> Rig {
    let game = FakeGame {
        attach_script: "st = { turn = 5 }             Game = { IsNetworkMultiPlayer = function() return true end, GetActivePlayer = function() return 0 end,                      GetGameTurn = function() return st.turn end }             GameDefines = { MAX_MAJOR_CIVS = 2 }             EndTurnBlockingTypes = { NO_ENDTURN_BLOCKING_TYPE = -1 }             Players = {}             for i = 0, 1 do Players[i] = { IsHuman = function() return true end, IsAlive = function() return true end,                 IsTurnActive = function() return true end, GetEndTurnBlockingType = function() return -1 end,                 HasReceivedNetTurnComplete = function() return false end } end"
            .into(),
        can_end_turn: true,
        ..Default::default()
    };
    let r = rig_with_game(ExeConfig::default(), game);
    r.exec("UI.GetHeadSelectedUnit = function() return nil end OptionsManager.SetMultiplayerAutoEndTurnEnabled_Cached(true) OptionsManager.CommitGameOptions()");
    r
}

fn sent_ends(r: &Rig) -> usize {
    r.game
        .borrow()
        .posted
        .iter()
        .filter(|m| m.name() == "sendTurnComplete")
        .count()
}

fn run_frames(r: &mut Rig, n: u32) {
    for _ in 0..n {
        r.exe.dispatch(&r.rt);
    }
}

/// A take-back after a manual end of turn latches the auto end too (SIMP-exe-700), and so does an order that reopens the
/// turn (the router's take-back); a server's refusal does not.
#[test]
fn auto_end_turn_stays_off_after_any_take_back_of_the_turn() {
    // By hand: end, take back, nothing more.
    let mut r = auto_end_rig();
    r.exec("Network.SendTurnComplete()");
    run_frames(&mut r, 10);
    r.exec("Network.SendTurnUnready()");
    run_frames(&mut r, 200);
    assert_eq!(sent_ends(&r), 1);
    // An order that reopens the turn.
    let mut r = auto_end_rig();
    run_frames(&mut r, 100);
    assert_eq!(sent_ends(&r), 1);
    r.game.borrow_mut().reopened = true;
    run_frames(&mut r, 200);
    assert_eq!(sent_ends(&r), 1);
    // The server refused the end of turn: not the player's doing, the auto end tries again.
    let mut r = auto_end_rig();
    run_frames(&mut r, 100);
    assert_eq!(sent_ends(&r), 1);
    r.exe.turn_complete_refused();
    run_frames(&mut r, 100);
    assert_eq!(sent_ends(&r), 2);
}

/// The auto end waits while a window is open: the city screen, a popup (diplomacy, tech tree, the menus are popups).
#[test]
fn auto_end_turn_waits_for_windows() {
    let mut r = auto_end_rig();
    r.game.borrow_mut().city_screen = true;
    run_frames(&mut r, 200);
    assert_eq!(sent_ends(&r), 0);
    r.game.borrow_mut().city_screen = false;
    add_popup(&r, 5, 100);
    run_frames(&mut r, 200);
    assert_eq!(sent_ends(&r), 0, "a popup is up");
    r.exec("Events.SerialEventGameMessagePopupProcessed(5)");
    run_frames(&mut r, 200);
    assert_eq!(sent_ends(&r), 1);
}

/// "The turn is being processed" (live report 2026-10-31: the END TURN button flipped several times while the server ran
/// the AI of a late game). The last human's own click puts the seat in it, before the server's echo (SIMP-exe-740); the
/// router is told (no CancelEndTurn ahead of a city or unit order), `SendTurnUnready` answers false, and once the replica
/// has every human's end of turn a refused duplicate END_TURN does not reopen the turn; a refusal before the echo does
/// (the server did not take it). The next load (the new turn) ends it.
#[test]
fn the_turn_being_processed_holds_until_the_new_turn() {
    let game = FakeGame {
        attach_script: "done = {} \
            Game = { IsNetworkMultiPlayer = function() return true end, GetActivePlayer = function() return 0 end, \
                     GetGameTurn = function() return 5 end } \
            GameDefines = { MAX_MAJOR_CIVS = 4 } \
            Players = {} \
            for i = 0, 3 do Players[i] = { IsHuman = function() return i < 2 end, \
                HasReceivedNetTurnComplete = function() return done[i] == true end } end"
            .into(),
        ..Default::default()
    };
    let mut r = rig_with_game(ExeConfig::default(), game);
    r.exec("done[1] = true");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.game.borrow().turn_locked, Some(false));
    // The last human ends the turn: processed at once, before the server's echo.
    r.exec("Network.SendTurnComplete()");
    assert!(r.eval_bool("return Network.HasSentNetTurnAllComplete()"));
    r.exe.dispatch(&r.rt);
    assert_eq!(r.game.borrow().turn_locked, Some(true));
    assert!(r.eval_bool("return Network.SendTurnUnready() == false"));
    // The server refused that end of turn (a blocker the replica did not have): the turn goes on.
    assert!(r.exe.turn_complete_refused());
    assert!(!r.eval_bool("return Network.HasSentNetTurnAllComplete()"));
    r.exe.dispatch(&r.rt);
    assert_eq!(r.game.borrow().turn_locked, Some(false));
    // Ended again and echoed: a later refusal (a duplicate stamped with the next turn) keeps it.
    r.exec("Network.SendTurnComplete() done[0] = true");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.game.borrow().turn_locked, Some(true));
    assert!(!r.exe.turn_complete_refused());
    assert!(r.eval_bool("return Network.HasSentNetTurnComplete()"));
    r.exe.dispatch(&r.rt);
    assert_eq!(r.game.borrow().turn_locked, Some(true));
    // The new turn's snapshot: nobody has ended it.
    r.exec("done[0] = nil done[1] = nil");
    r.game.borrow_mut().generation += 1;
    r.exe.dispatch(&r.rt);
    assert!(!r.eval_bool("return Network.HasSentNetTurnComplete()"));
    assert_eq!(r.game.borrow().turn_locked, Some(false));
    assert_eq!(r.errors(), 0);
}

#[test]
fn the_cores_temp_string_is_what_the_popup_reads() {
    let mut r = rig();
    // Nothing was said yet.
    assert_eq!(r.eval_str("return UI.GetTempString()"), "");
    // CvNotifications::Activate: SetTempString(message), then the city-state message popup.
    r.game
        .borrow_mut()
        .events
        .push(ev(K::SetTempString, &[]).with_text("Kiev has a message for you."));
    r.exe.dispatch(&r.rt);
    assert_eq!(
        r.eval_str("return UI.GetTempString()"),
        "Kiev has a message for you."
    );
    r.game
        .borrow_mut()
        .events
        .push(ev(K::SetTempString, &[]).with_text("Next."));
    r.exe.dispatch(&r.rt);
    assert_eq!(r.eval_str("return UI.GetTempString()"), "Next.");
    assert_eq!(r.errors(), 0);
}

#[test]
fn without_a_replica_the_deal_lists_are_empty() {
    let r = rig();
    assert_eq!(
        r.eval_str(
            "return tostring(UI.GetNumCurrentDeals(0)) .. tostring(UI.GetNumHistoricDeals(0))"
        ),
        "00"
    );
    r.exec("UI.LoadCurrentDeal(0, 0) UI.LoadHistoricDeal(0, 0)");
    assert_eq!(r.errors(), 0);
}

#[test]
fn the_replicas_deal_lists_are_the_ui_functions() {
    let r = rig();
    // The GcUi* functions are globals of the Lua state (the replica sets them), not of a context.
    r.ctx
        .lua()
        .load(
            "calls = {}
             GcUiGetNumCurrentDeals = function(p) table.insert(calls, 'n' .. p) return 2 end
             GcUiLoadCurrentDeal = function(p, i) table.insert(calls, 'c' .. p .. ':' .. i) end
             GcUiGetNumHistoricDeals = function(p) table.insert(calls, 'h' .. p) return 1 end
             GcUiLoadHistoricDeal = function(p, i) table.insert(calls, 'l' .. p .. ':' .. i) end",
        )
        .exec()
        .unwrap();
    r.exec(
        "log = {}
         table.insert(log, UI.GetNumCurrentDeals(3)) UI.LoadCurrentDeal(3, 1)
         table.insert(log, UI.GetNumHistoricDeals(3)) UI.LoadHistoricDeal(3, 0)
         table.insert(log, table.concat(calls, '/'))",
    );
    assert_eq!(r.log(), "2,1,n3/c3:1/h3/l3:0");
}
#[test]
fn the_multiplayer_proposal_chart_is_a_loaded_context() {
    // Lekmod's vote chart (ActionInfoPanel raises BUTTONPOPUP_MODDER_0 for it): its Lua context is in NotificationPanel.xml and
    // stayed an empty hidden context while it was not in the list.
    assert!(
        exe_api::m2_contexts()
            .iter()
            .any(|c| c == "ProposalChartPopup")
    );
}

#[test]
fn the_multiplayer_cc_vote_popup_is_a_loaded_context() {
    // Lekmod's "propose a vote for another player" popup: VictoryProgress.xml nests it as a LuaContext and its
    // LuaEvents.MPProposeCCButtonPress listener lives in the context, so the button did nothing while it was not listed.
    assert!(exe_api::m2_contexts().iter().any(|c| c == "CCVotePopup"));
}

#[test]
fn the_exe_never_sees_the_ai_asking_for_concessions_without_the_dll() {
    // InterfaceBuddy::IsAIRequestingConcessions is a byte only the DLL sets; its source only ever clears it.
    let r = rig();
    r.exec("log = {} table.insert(log, tostring(UI.IsAIRequestingConcessions()))");
    assert_eq!(r.log(), "false");
}

#[test]
fn selecting_a_unit_fires_unit_hex_highlight_on_its_plot() {
    // audit 35: InGame.lua's OnUnitHexHighlight (settler yields) hangs on this event; the exe raises it on select, on
    // every move of the selected unit and on deselect.
    let mut r = rig();
    dll_stand_in(&r);
    r.exec(
        "log = {}
         Events.UnitHexHighlight.Add(function(i, j, k, on, id) table.insert(log, i .. ',' .. j .. ',' .. k .. ':' .. tostring(on) .. ':' .. id) end)",
    );
    r.game.borrow_mut().events.push(ev(
        K::UnitCreated,
        &[0, 42, 4, 2, 7, 1, 2, 3, 4, 5, 2, 0, 1, 1],
    ));
    r.exe.dispatch(&r.rt);
    r.exec("UI.SelectUnit(MyUnit)");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "3,2,-5:true:42");
    r.exec("log = {} UI.SelectUnit(Players[0]:GetUnitByID(43))");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "3,2,-5:false:42,0,0,0:true:43");
}

#[test]
fn the_camera_is_moving_when_the_client_says_so() {
    // audit 34: WorldView.lua ignores a right click move while UI.IsCameraMoving().
    let r = rig();
    assert!(!r.eval_bool("return UI.IsCameraMoving()"));
    r.exe.set_camera_moving(true);
    assert!(r.eval_bool("return UI.IsCameraMoving()"));
    r.exe.set_camera_moving(false);
    assert!(!r.eval_bool("return UI.IsCameraMoving()"));
}

#[test]
fn the_city_view_height_follows_the_city_distance_high_water_mark() {
    // audit 38 / night answers 1b P4: CITY_ZOOM_IN is 500 / 700 / 800 for the mark below 2 / 2 / 3 and more; the mark
    // never falls.
    let r = rig_with_game(
        ExeConfig::default(),
        FakeGame {
            attach_script: "radius = 1
                function SetRadius(v) radius = v end
                Game = { GetActivePlayer = function() return 0 end }
                local city = { GetX = function() return 10 end, GetY = function() return 10 end }
                Players = { [0] = { Cities = function() local done = false
                    return function() if not done then done = true return city end end end } }
                Map = {
                  GetGridSize = function() return 40, 40 end,
                  GetPlot = function(x, y)
                    local own = math.max(math.abs(x - 10), math.abs(y - 10)) <= radius
                    return { GetOwner = function() return own and 0 or -1 end, GetX = function() return x end,
                             GetY = function() return y end, GetPlotIndex = function() return x * 100 + y end }
                  end,
                  PlotDistance = function(ax, ay, bx, by) return math.max(math.abs(ax - bx), math.abs(ay - by)) end,
                }"
                .to_owned(),
            ..Default::default()
        },
    );
    assert_eq!(r.exe.city_zoom_height(), 500.0);
    r.exec("SetRadius(2)");
    assert_eq!(r.exe.city_zoom_height(), 700.0);
    r.exec("SetRadius(4)");
    assert_eq!(r.exe.city_zoom_height(), 800.0);
    r.exec("SetRadius(1)");
    assert_eq!(r.exe.city_zoom_height(), 800.0);
}

/// SIMP-exe-950: Goodbye of the trade screen (TradeLogic OnBack) asks to leave the leader view; every leader screen closes.
#[test]
fn request_leave_leader_raises_leaving_leader_view_mode_and_clears_the_flag() {
    let r = rig();
    r.exec(
        "log = {} Events.LeavingLeaderViewMode.Add(function() table.insert(log, 'left') end)
         UI.SetLeaderHeadRootUp(true)",
    );
    assert!(r.eval_bool("return UI.GetLeaderHeadRootUp()"));
    r.exec("UI.RequestLeaveLeader()");
    assert_eq!(r.log(), "left");
    assert!(!r.eval_bool("return UI.GetLeaderHeadRootUp()"));
    assert_eq!(r.errors(), 0);
}

/// SIMP-exe-952: a moved border redraws the city screen's highlights.
#[test]
fn a_moved_border_raises_the_city_hex_highlight_event() {
    let mut r = rig();
    r.exec("log = {} Events.SerialEventCityHexHighlightDirty.Add(function() table.insert(log, 'hex') end)");
    r.game
        .borrow_mut()
        .events
        .push(ev(K::UpdateCountryBorder, &[3, 4]));
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "hex");
    assert_eq!(r.errors(), 0);
}

/// SIMP-exe-953: a renamed unit's flag is told.
#[test]
fn a_renamed_unit_raises_unit_name_changed() {
    let mut r = rig();
    dll_stand_in(&r);
    r.exec("log = {} Events.UnitNameChanged.Add(function(p, u) table.insert(log, 'n' .. p .. ':' .. u) end)");
    r.exec("Network.SendRenameUnit(42, 'Bob')");
    r.exe.dispatch(&r.rt);
    assert!(r.log().starts_with("n2:42"), "{}", r.log());
    assert_eq!(r.errors(), 0);
}
