//! Server-role Lua attachment on a real running game.

use game_host::Worker;
use lua_sys::Value;
use protocol::Start;

#[path = "common/real.rs"]
mod common;

#[test]
fn server_lua_reads_turn_and_gold() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let (worker, outs) = Worker::new(
        common::real_core(),
        common::init(Start::New(common::setup()), ""),
        0,
    );
    let mut h = common::H::with(worker, outs);
    let lua = lua_sys::Lua::new().expect("Lua state");
    let game = h.w.core_mut().gamecore_mut();
    game.attach_lua(&lua).expect("attach in server role");
    lua.exec(
        "function probe() return Game.GetGameTurn(), Players[0]:GetGold() end",
        "=server-bridge",
    )
    .expect("probe script");
    let values = lua.call_global("probe", &[]).expect("read server game");
    let info = game.game_info().expect("game info");
    assert_eq!(values[0], Value::Number(f64::from(info.turn)));
    assert!(values[1].as_i32().is_some(), "gold is an integer");
}
