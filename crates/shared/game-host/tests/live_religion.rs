//! Triage of a live save on the real core: `CIV5_LIVE_SAVE=<a .c5sv of the server>` is loaded as the worker loads it,
//! the seats it names are connected, the state of seat `CIV5_LIVE_PLAYER` (default 1) is printed (safe point, blocker,
//! religion, busy units), and the seat founds a religion as ChooseReligionPopup.lua does (`SendFoundReligion` with the
//! first religion and beliefs still available, the blocker's city). Run on Linux/WSL with `GAMECORE_LIB_DIR_x86_64` and
//! `CIV5_GAMEDATA`; skipped otherwise. Live game 931 (2026-10-30): END TURN blocked by "found a religion", the popup's
//! Found did nothing, a kick of the seat never came (no safe point).

use std::path::PathBuf;

use gamecore::replica::NetMessage;
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

    fn game_event(&self, ev: GameEvent) {
        eprintln!("EVENT {ev:?}");
    }
}

const PROBE: &str = r#"
function Report(p)
  local pl = Players[p]
  local out = {}
  local function add(s) out[#out + 1] = tostring(s) end
  add("turn " .. Game.GetGameTurn() .. " player " .. p .. " " .. tostring(pl:GetName()))
  add("pantheon " .. tostring(pl:HasCreatedPantheon()) .. " religion " .. tostring(pl:HasCreatedReligion())
      .. " still to found " .. tostring(Game.GetNumReligionsStillToFound()) .. " bonus trait " .. tostring(pl:IsTraitBonusReligiousBelief()))
  for i = 0, GameDefines.MAX_MAJOR_CIVS - 1 do
    local q = Players[i]
    if q and q:IsAlive() then
      local busy = {}
      for u in q:Units() do
        if u:IsBusy() then busy[#busy + 1] = u:GetID() .. ":" .. tostring(GameInfo.Units[u:GetUnitType()].Type) .. "@" .. u:GetX() .. "," .. u:GetY() .. (u:IsInCombat() and " combat" or "") end
      end
      if q:IsHuman() or #busy > 0 then
        add("player " .. i .. " human " .. tostring(q:IsHuman()) .. " busy units [" .. table.concat(busy, " ") .. "]")
      end
    end
  end
  for u in pl:Units() do
    local t = GameInfo.Units[u:GetUnitType()].Type
    if t == "UNIT_PROPHET" then add("prophet " .. u:GetID() .. " at " .. u:GetX() .. "," .. u:GetY()) end
  end
  for c in pl:Cities() do add("city " .. c:GetID() .. " " .. c:GetName() .. " at " .. c:GetX() .. "," .. c:GetY()) end
  local function list(name, t)
    local ids = {}
    for _, b in ipairs(t) do ids[#ids + 1] = tostring(b) end
    add(name .. " [" .. table.concat(ids, " ") .. "]")
  end
  list("pantheon beliefs", Game.GetAvailablePantheonBeliefs())
  list("founder beliefs", Game.GetAvailableFounderBeliefs())
  list("follower beliefs", Game.GetAvailableFollowerBeliefs())
  local free = {}
  for row in GameInfo.Religions() do
    if row.Type ~= "RELIGION_PANTHEON" and Game.GetFounder(row.ID, -1) == -1 then free[#free + 1] = row.ID end
  end
  add("religions not founded [" .. table.concat(free, " ") .. "]")
  return table.concat(out, "\n")
end

-- The arguments ChooseReligionPopup.lua sends (without the player): religion, b1..b4.
function Plan(p)
  local pl = Players[p]
  local religion = -1
  for row in GameInfo.Religions() do
    if religion < 0 and row.Type ~= "RELIGION_PANTHEON" and Game.GetFounder(row.ID, -1) == -1 then religion = row.ID end
  end
  local b = {}
  if not pl:HasCreatedPantheon() then b[#b + 1] = Game.GetAvailablePantheonBeliefs()[1] end
  b[#b + 1] = Game.GetAvailableFounderBeliefs()[1]
  b[#b + 1] = Game.GetAvailableFollowerBeliefs()[1]
  if pl:IsTraitBonusReligiousBelief() then b[#b + 1] = Game.GetAvailableFollowerBeliefs()[2] end
  return religion, b[1] or -1, b[2] or -1, b[3] or -1, b[4] or -1
end
"#;

fn report(lua: &lua_sys::Lua, player: u8) {
    match lua.call_global("Report", &[Value::from(i32::from(player))]) {
        Ok(v) => {
            if let Some(Value::String(s)) = v.first() {
                eprintln!("{s}");
            }
        }
        Err(e) => eprintln!("Report failed: {e}"),
    }
}

#[test]
fn live_save_founds_a_religion() {
    let Some(file) = std::env::var_os("CIV5_LIVE_SAVE") else {
        eprintln!("skipped: CIV5_LIVE_SAVE is not set");
        return;
    };
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let data = PathBuf::from(std::env::var_os("CIV5_GAMEDATA").expect("CIV5_GAMEDATA"));
    let player: u8 = std::env::var("CIV5_LIVE_PLAYER")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let bytes = std::fs::read(&file).expect("CIV5_LIVE_SAVE");
    let save = game_host::saves::decode(&bytes).expect("a C5SV save");
    let mut rep = ReplicaGame::new(DirHost(data), None).unwrap();
    let game = rep.game();
    game.load(&save.gcs1).unwrap();
    game.set_role(Role::Server).unwrap();
    let info = game_host::saves::read_header(&bytes).unwrap();
    eprintln!("seats {:?}", info.seats);
    for _ in 0..50 {
        let r = game.pump(64).unwrap();
        let gi = game.game_info().unwrap();
        eprintln!(
            "pump {:?} safe {} unsafe {:?}",
            r.status, gi.safe_point, gi.unsafe_reason
        );
        if gi.safe_point {
            break;
        }
    }
    for s in &info.seats {
        if let protocol::SlotControl::Human { nick } = &s.control {
            let r = game.slot_control(s.player as u8, SlotOp::Connect, Some(nick));
            eprintln!("connect {} {nick}: {r:?}", s.player);
        }
    }
    let lua = lua_sys::Lua::new().unwrap();
    rep.attach_lua(&lua).unwrap();
    lua.exec(PROBE, "=probe").unwrap();
    // `CIV5_LIVE_CLAIM=<player>:<nick>`: a human takes an AI seat after the load, as a JoinGame does.
    if let Ok(claim) = std::env::var("CIV5_LIVE_CLAIM") {
        let (p, nick) = claim.split_once(':').expect("player:nick");
        let p: u8 = p.parse().unwrap();
        let r = rep.game().slot_control(p, SlotOp::Connect, Some(nick));
        eprintln!("claim {p} {nick}: {r:?}");
        for i in 0..20 {
            let r = rep.game().pump(64).unwrap();
            let gi = rep.game().game_info().unwrap();
            eprintln!(
                "claim pump {i} {:?} slices {} safe {} unsafe {:?}",
                r.status, r.slices_used, gi.safe_point, gi.unsafe_reason
            );
        }
        report(&lua, p);
        let ts = rep.game().turn_state().unwrap();
        eprintln!("claimed: {:?}", ts.iter().find(|t| t.player == p));
    }
    let game = rep.game();
    let _ = game.pump(64).unwrap();
    let gi = game.game_info().unwrap();
    eprintln!(
        "after connect: safe {} unsafe {:?}",
        gi.safe_point, gi.unsafe_reason
    );
    let turn = game.turn_state().unwrap();
    for t in &turn {
        eprintln!("turn state {t:?}");
    }
    let me = turn
        .iter()
        .find(|t| t.player == player)
        .expect("the seat")
        .clone();
    for n in game.notifications(player).unwrap() {
        eprintln!("notification {n:?}");
    }
    report(&lua, player);

    let plan = lua
        .call_global("Plan", &[Value::from(i32::from(player))])
        .unwrap();
    let ids: Vec<i32> = plan.iter().filter_map(Value::as_i32).collect();
    let (x, y) = me.blocker_plot().expect("a blocker plot");
    let args = [
        ids[0],
        ids[1],
        ids[2],
        ids[3],
        ids[4],
        i32::from(x),
        i32::from(y),
    ];
    eprintln!("SendFoundReligion {args:?}");
    let m = NetMessage::new("SendFoundReligion", &args).unwrap();
    let game = rep.game();
    let sent = game.net_submit(player, &[m]);
    eprintln!("net_submit: {sent:?}");
    for _ in 0..4 {
        let r = game.pump(64).unwrap();
        eprintln!("pump {r:?}");
    }
    let turn = game.turn_state().unwrap();
    let me = turn.iter().find(|t| t.player == player).unwrap().clone();
    eprintln!("after: {me:?}");
    let gi = game.game_info().unwrap();
    eprintln!(
        "after: safe {} unsafe {:?}",
        gi.safe_point, gi.unsafe_reason
    );
    report(&lua, player);
    let kick = rep.game().slot_control(player, SlotOp::Kick, None);
    eprintln!("kick: {kick:?}");
}
