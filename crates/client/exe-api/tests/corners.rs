//! WO-C (M3): the minimap handle, the chat, the kick and the map options of the corners.
mod fake;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use exe_api::{ExeApi, ExeConfig, ExeHost, GameSide, SeatInfo};
use lua_sys::{Context, MemorySource, Runtime, RuntimeConfig};
use platform::{KeyValueStore, MemoryStore};

#[derive(Default)]
struct Host {
    kv: MemoryStore,
    chats: RefCell<Vec<(Option<i32>, String)>>,
    kicks: RefCell<Vec<i32>>,
    grid: Cell<Option<bool>>,
}

impl ExeHost for Host {
    fn kv(&self) -> &dyn KeyValueStore {
        &self.kv
    }
    fn look_at(&self, _x: i32, _y: i32) {}
    fn seat_info(&self) -> SeatInfo {
        SeatInfo::default()
    }
    fn minimap_size(&self) -> (u32, u32) {
        (300, 180)
    }
    fn send_chat(&self, to: Option<i32>, text: &str) {
        self.chats.borrow_mut().push((to, text.to_owned()));
    }
    fn kick_player(&self, player: i32) {
        self.kicks.borrow_mut().push(player);
    }
    fn set_grid_visible(&self, on: bool) {
        self.grid.set(Some(on));
    }
}

struct Rig {
    rt: Runtime,
    exe: ExeApi,
    ctx: Context,
    host: Rc<Host>,
    game: Rc<RefCell<fake::FakeGame>>,
}

fn rig() -> Rig {
    rig_with_game(fake::FakeGame::default())
}

fn rig_with_game(fake_game: fake::FakeGame) -> Rig {
    let host = Rc::new(Host::default());
    let mut exe = ExeApi::new(ExeConfig::default(), host.clone());
    let rt = Runtime::new(
        RuntimeConfig::default(),
        Rc::new(MemorySource::new()),
        exe.base_setup(),
    )
    .expect("runtime");
    let ctx = rt.create_context("Test", |_, _| Ok(())).expect("context");
    let game = Rc::new(RefCell::new(fake_game));
    let dynamic: Rc<RefCell<dyn GameSide>> = game.clone();
    exe.attach_runtime(&rt);
    exe.bind_game(dynamic, ctx.lua());
    exe.dispatch(&rt);
    Rig {
        rt,
        exe,
        ctx,
        host,
        game,
    }
}

impl Rig {
    fn exec(&self, code: &str) {
        self.ctx
            .exec(code, "=test")
            .unwrap_or_else(|e| panic!("{e}"));
    }

    fn log(&self) -> String {
        self.ctx
            .lua()
            .load("return table.concat(log, ',')")
            .set_name("=eval")
            .set_environment(self.ctx.env())
            .eval::<String>()
            .unwrap_or_else(|e| panic!("{e}"))
    }
}

#[test]
fn the_minimap_panel_gets_the_handle_and_the_size_of_the_host() {
    let mut r = rig();
    r.exec(
        "log = {}
         Events.MinimapTextureBroadcastEvent.Add(function(h, w, hh) table.insert(log, h .. ':' .. w .. 'x' .. hh) end)
         UI:RequestMinimapBroadcast()",
    );
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "1:300x180");
    // The texture changed: the panel is told again.
    r.exe.broadcast_minimap();
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "1:300x180,1:300x180");
    assert_eq!(r.rt.error_count(), 0);
}

#[test]
fn a_click_on_the_minimap_is_an_event_the_app_can_tap() {
    let r = rig();
    r.exec(
        "log = {}
         Events.MinimapClickedEvent.Add(function(x, y) table.insert(log, x .. ',' .. y) end)
         Events.MinimapClickedEvent(12, 34)",
    );
    assert_eq!(r.log(), "12,34");
}

#[test]
fn chat_goes_out_through_the_host_and_comes_in_as_game_message_chat() {
    let r = rig();
    r.exec(
        "log = {}
         Events.GameMessageChat.Add(function(from, to, text, target, skip, turn, stamp)
           table.insert(log, from .. '>' .. to .. ':' .. text .. ':' .. target .. ':' .. tostring(skip) .. ':' .. turn .. ':' .. stamp)
         end)
         Network.SendChat('hello', -1, -1)
         Network.SendChat('psst', -1, 2)",
    );
    assert_eq!(
        *r.host.chats.borrow(),
        vec![(None, "hello".to_owned()), (Some(2), "psst".to_owned())]
    );
    r.exe.chat_received(1, None, "yo", 3, 9);
    r.exe.chat_received(0, Some(1), "you", 4, 10);
    assert_eq!(r.log(), "1>-1:yo:0:false:3:9,0>1:you:2:false:4:10");
    assert_eq!(r.rt.error_count(), 0);
}

/// A team line (`Network.SendChat(text, team, -1)`) goes to the humans of that team only, one direct line each (SIMP-exe-912),
/// and to the sender alone when nobody else is on the team: never to everybody (audit A7).
#[test]
fn a_team_chat_line_goes_to_the_team_only() {
    let r = rig_with_game(fake::FakeGame {
        attach_script: "Game = { IsNetworkMultiPlayer = function() return true end, GetActivePlayer = function() return 0 end }             GameDefines = { MAX_MAJOR_CIVS = 4 }             local team = { [0] = 0, [1] = 0, [2] = 1, [3] = 0 } local human = { [0] = true, [1] = true, [2] = true, [3] = false }             Players = {}             for i = 0, 3 do Players[i] = { IsHuman = function() return human[i] end, GetTeam = function() return team[i] end } end"
            .into(),
        ..Default::default()
    });
    r.exec("Network.SendChat('us', 0, -1) Network.SendChat('alone', 1, -1)");
    assert_eq!(
        *r.host.chats.borrow(),
        vec![
            (Some(0), "us".to_owned()),
            (Some(1), "us".to_owned()),
            (Some(2), "alone".to_owned())
        ]
    );
    assert_eq!(r.rt.error_count(), 0);
}

/// Russian text goes through the Lua border both ways as the same UTF-8 (SIMP-exe-716: the glyphs are the font's
/// business, the bytes must not change here).
#[test]
fn cyrillic_chat_survives_both_hops_of_the_lua_border() {
    let r = rig();
    r.exec(
        "log = {}
         Events.GameMessageChat.Add(function(from, to, text) table.insert(log, #text .. ':' .. text) end)
         Network.SendChat('привет, мир № 1', -1, -1)",
    );
    assert_eq!(
        *r.host.chats.borrow(),
        vec![(None, "привет, мир № 1".to_owned())]
    );
    r.exe.chat_received(1, None, "Ёлка и щи", 3, 9);
    // Lua counts bytes: 7 Cyrillic letters of two bytes, 2 spaces
    assert_eq!(r.log(), "16:Ёлка и щи");
    assert_eq!(r.rt.error_count(), 0);
}

#[test]
fn the_chat_copy_of_the_lekmod_corner_is_dropped_but_other_enhance_religion_goes_on() {
    let mut r = rig();
    r.exec("Network.SendEnhanceReligion(0, -1, 'chat text', 0, -1, -1, -1)");
    assert!(r.game.borrow().posted.is_empty());
    r.exec("Network.SendEnhanceReligion(0, 2, 1, 2, 3, 4, 5)");
    r.exe.dispatch(&r.rt);
    assert_eq!(r.game.borrow().posted.len(), 1);
}

#[test]
fn kick_and_grid_reach_the_host_and_the_strategic_view_lists_exist() {
    let mut r = rig();
    r.exec(
        "log = {}
         Matchmaking.KickPlayer(3)
         UI.SetGridVisibleMode(true)
         Events.SerialEventHexGridOn.Add(function() table.insert(log, 'on') end)
         UI.SetGridVisibleMode(true)
         table.insert(log, #GetStrategicViewOverlays() .. '/' .. #GetStrategicViewIconSettings())
         table.insert(log, tostring(GetOverlayLegend()))
         table.insert(log, SlotClaim.SLOTCLAIM_ASSIGNED .. '/' .. SlotStatus.SS_OBSERVER)",
    );
    r.exe.dispatch(&r.rt);
    assert_eq!(*r.host.kicks.borrow(), vec![3]);
    assert_eq!(r.host.grid.get(), Some(true));
    assert_eq!(r.log(), "on,10/5,nil,2/3");
    assert_eq!(r.rt.error_count(), 0);
}

#[test]
fn a_loaded_later_turn_raises_the_turn_events_once() {
    let mut r = rig_with_game(fake::FakeGame {
        attach_script: "TURN = 0; Game = { GetGameTurn = function() return TURN end }".to_owned(),
        ..Default::default()
    });
    r.exec(
        "log = {}
         Events.NewGameTurn.Add(function(t) table.insert(log, 'new' .. t) end)
         Events.ActivePlayerTurnStart.Add(function() table.insert(log, 'start') end)",
    );
    // The same turn loaded again (a resync): nothing.
    r.game.borrow_mut().generation += 1;
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "");
    // The next turn's snapshot: the turn starts for the player.
    r.ctx.lua().globals().set("TURN", 1).expect("set");
    r.game.borrow_mut().generation += 1;
    r.exe.dispatch(&r.rt);
    assert_eq!(r.log(), "new1,start");
    assert_eq!(r.rt.error_count(), 0);
}
