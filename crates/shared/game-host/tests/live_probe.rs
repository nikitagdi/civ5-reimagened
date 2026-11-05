//! Generic triage of a live save on the real core: `CIV5_LIVE_SAVE=<a .c5sv of the server>` is loaded as the worker
//! loads it, the human seats it names are connected, and the Lua file `CIV5_PROBE_LUA` runs against the game; its
//! global `Probe()` returns a string that is printed. Run on Linux/WSL with `GAMECORE_LIB_DIR_x86_64` and
//! `CIV5_GAMEDATA`; skipped otherwise. Live game 936 (2026-10-31): the top panel's golden age points read -69152.

use std::path::PathBuf;

use gamecore::{GameEvent, HostServices, ReplicaGame, Role, SlotOp};
use lua_sys::Value;

struct DirHost(PathBuf);

impl HostServices for DirHost {
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        if path.split(['/', '\\']).any(|p| p == "..") {
            return None;
        }
        std::fs::read(self.0.join(path)).ok()
    }

    fn game_event(&self, _ev: GameEvent) {}
}

#[test]
fn live_save_probe() {
    let Some(file) = std::env::var_os("CIV5_LIVE_SAVE") else {
        eprintln!("skipped: CIV5_LIVE_SAVE is not set");
        return;
    };
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let data = PathBuf::from(std::env::var_os("CIV5_GAMEDATA").expect("CIV5_GAMEDATA"));
    let probe =
        std::fs::read_to_string(std::env::var_os("CIV5_PROBE_LUA").expect("CIV5_PROBE_LUA"))
            .expect("the probe file");
    let bytes = std::fs::read(&file).expect("CIV5_LIVE_SAVE");
    let save = game_host::saves::decode(&bytes).expect("a C5SV save");
    // `CIV5_PROBE_SEAT=<player>`: the game is a client's replica of that seat, loaded as a client loads a
    // snapshot (no pump, no slot change).
    let seat: Option<u8> = std::env::var("CIV5_PROBE_SEAT")
        .ok()
        .and_then(|v| v.parse().ok());
    let mut rep = ReplicaGame::new(DirHost(data), seat).unwrap();
    if seat.is_some() {
        rep.resync(&save.gcs1).unwrap();
        let lua = lua_sys::Lua::new().unwrap();
        rep.attach_lua(&lua).unwrap();
        lua.exec(&probe, "=probe").unwrap();
        match lua.call_global("Probe", &[]) {
            Ok(v) => {
                if let Some(Value::String(s)) = v.first() {
                    println!("{s}");
                }
            }
            Err(e) => panic!("Probe failed: {e}"),
        }
        return;
    }
    let game = rep.game();
    game.load(&save.gcs1).unwrap();
    game.set_role(Role::Server).unwrap();
    let info = game_host::saves::read_header(&bytes).unwrap();
    for _ in 0..50 {
        let _ = game.pump(64).unwrap();
        if game.game_info().unwrap().safe_point {
            break;
        }
    }
    for s in &info.seats {
        if let protocol::SlotControl::Human { nick } = &s.control {
            let r = game.slot_control(s.player as u8, SlotOp::Connect, Some(nick));
            eprintln!("connect {} {nick}: {r:?}", s.player);
        }
    }
    let _ = game.pump(64).unwrap();
    let lua = lua_sys::Lua::new().unwrap();
    rep.attach_lua(&lua).unwrap();
    lua.exec(&probe, "=probe").unwrap();
    match lua.call_global("Probe", &[]) {
        Ok(v) => {
            if let Some(Value::String(s)) = v.first() {
                println!("{s}");
            }
        }
        Err(e) => panic!("Probe failed: {e}"),
    }
}
