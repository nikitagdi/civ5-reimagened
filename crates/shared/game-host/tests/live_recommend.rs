//! The tile recommendations of the original UI (InGame.lua OnUpdateSelection: `Player:GetRecommendedWorkerPlots`,
//! `GetRecommendedFoundCityPlots`) on a client's replica of a live save (live report 2026-10-31: "the hints of where to
//! found a city and which tile to improve don't work"). Both DLL getters walk the engine UI's `GetSelectedUnits`, which
//! the portable replica answered with nothing, so they returned empty tables. `CIV5_LIVE_SAVE=<a .c5sv>` is loaded as
//! the replica of seat `CIV5_LIVE_PLAYER` (default 0); each worker and settler of the seat is selected in turn
//! (`UiOp::SelectUnit`, as the client's selection does) and the getters must answer plots, without touching the
//! serialized state (the replica's digest, `state_digest(-2)`, is unchanged). Run on Linux/WSL with
//! `GAMECORE_LIB_DIR_x86_64` and `CIV5_GAMEDATA`; skipped otherwise.

use std::path::PathBuf;

use gamecore::{HostServices, ReplicaGame, UiOp};
use lua_sys::Value;

struct DirHost(PathBuf);

impl HostServices for DirHost {
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        if path.split(['/', '\\']).any(|p| p == "..") {
            return None;
        }
        std::fs::read(self.0.join(path)).ok()
    }

    fn game_event(&self, _ev: gamecore::GameEvent) {}
}

const PROBE: &str = r#"
-- The seat's workers and settlers: "w:<id>" / "s:<id>".
function Civilians(p)
  local out = {}
  for u in Players[p]:Units() do
    local info = GameInfo.Units[u:GetUnitType()]
    if info.WorkRate > 0 then out[#out + 1] = "w:" .. u:GetID() end
    if info.Found then out[#out + 1] = "s:" .. u:GetID() end
  end
  return table.concat(out, " ")
end
function Worker(p)
  local t = Players[p]:GetRecommendedWorkerPlots()
  local s = {}
  for _, v in ipairs(t) do s[#s + 1] = v.plot:GetX() .. "," .. v.plot:GetY() .. ":" .. GameInfo.Builds[v.buildType].Type end
  return #t, table.concat(s, " ")
end
function Settler(p)
  local t = Players[p]:GetRecommendedFoundCityPlots()
  local s = {}
  for _, v in ipairs(t) do s[#s + 1] = v:GetX() .. "," .. v:GetY() end
  return #t, table.concat(s, " ")
end
"#;

#[test]
fn live_recommendations_follow_the_selection() {
    let Some(file) = std::env::var_os("CIV5_LIVE_SAVE") else {
        eprintln!("skipped: CIV5_LIVE_SAVE is not set");
        return;
    };
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let data = PathBuf::from(std::env::var_os("CIV5_GAMEDATA").expect("CIV5_GAMEDATA"));
    let seat: u8 = std::env::var("CIV5_LIVE_PLAYER")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let bytes = std::fs::read(&file).expect("CIV5_LIVE_SAVE");
    let save = game_host::saves::decode(&bytes).expect("a C5SV save");
    let mut rep = ReplicaGame::new(DirHost(data), Some(seat)).unwrap();
    rep.resync(&save.gcs1).unwrap();
    let lua = lua_sys::Lua::new().unwrap();
    rep.attach_lua(&lua).unwrap();
    lua.exec(PROBE, "=probe").unwrap();
    let p = Value::from(i32::from(seat));
    let civilians = match lua
        .call_global("Civilians", std::slice::from_ref(&p))
        .unwrap()
        .first()
    {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    eprintln!("seat {seat} civilians: {civilians}");
    let before = rep.game().state_digest(-2).unwrap();
    let (mut workers, mut settlers) = (0, 0);
    for c in civilians.split_whitespace() {
        let (kind, id) = c.split_once(':').unwrap();
        let id: i32 = id.parse().unwrap();
        let mut out = Vec::new();
        rep.ui_op(UiOp::SelectUnit, &[i32::from(seat), id], &mut out)
            .unwrap();
        let f = if kind == "w" { "Worker" } else { "Settler" };
        let r = lua.call_global(f, std::slice::from_ref(&p)).unwrap();
        let n = r.first().and_then(Value::as_i32).unwrap_or(-1);
        eprintln!("{f} {id}: {n} {:?}", r.get(1));
        assert!((0..=3).contains(&n), "{f}: at most 3 plots");
        if kind == "w" {
            workers += usize::from(n > 0);
        } else {
            settlers += usize::from(n > 0);
        }
    }
    let mut out = Vec::new();
    rep.ui_op(UiOp::ClearSelection, &[], &mut out).unwrap();
    let after = rep.game().state_digest(-2).unwrap();
    assert_eq!(
        before, after,
        "the recommendations left the replica's state as it was"
    );
    // A seat whose workers have nothing worth doing gets none (CIV5_LIVE_ALLOW_EMPTY=1); game 936 seat 0 at turn 30
    // and 120 and seat 2 at turn 30 get them for every worker.
    if !civilians.is_empty() && std::env::var_os("CIV5_LIVE_ALLOW_EMPTY").is_none() {
        assert!(
            workers + settlers > 0,
            "a worker or settler of the seat got recommendations"
        );
    }
    eprintln!("workers with plots {workers}, settlers with plots {settlers}");
}
