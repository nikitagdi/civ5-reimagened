//! A free technology of a human on the real core (live report 2026-10-31: "the free tech of the Rationalism finisher
//! is picked for you"). `CIV5_LIVE_SAVE=<a .c5sv of the server>` is loaded as the worker loads it, the human seats are
//! connected, the seat `CIV5_LIVE_PLAYER` (default 0; its turn must not be ended in the save) gets Oxford University
//! (or the Great Library) in its capital (Lekmod's free-tech sources: its Rationalism finisher has NumFreeTechs 0,
//! HappinessToScience 10), and the test checks the DLL's human path
//! (CvPlayer::processBuilding -> chooseTech): the counter is 1, the FREE_TECH notification blocks the turn, END_TURN is
//! refused, the counter survives pumps, and the choice the seat sends (`sendResearch(tech, 1, -1, false)` as the
//! TechTree sends it) gives exactly that tech. Run on Linux/WSL with `GAMECORE_LIB_DIR_x86_64` and `CIV5_GAMEDATA`;
//! skipped otherwise.

use std::path::PathBuf;

use gamecore::replica::NetMessage;
use gamecore::{Command, GameEvent, HostServices, ReplicaGame, Role, SlotOp};
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
        if let GameEvent::AutoChoice { .. } = ev {
            eprintln!("EVENT {ev:?}");
        }
    }
}

const PROBE: &str = r#"
-- Oxford University, or the Great Library when the capital has Oxford already (both FreeTechs 1).
function Grant(p)
  local c = Players[p]:GetCapitalCity()
  local b = GameInfoTypes.BUILDING_OXFORD_UNIVERSITY
  if c:GetNumRealBuilding(b) > 0 then b = GameInfoTypes.BUILDING_GREAT_LIBRARY end
  c:SetNumRealBuilding(b, 1)
  return c:GetNumRealBuilding(b)
end
function State(p)
  local pl = Players[p]
  local n = 0
  for t in GameInfo.Technologies() do if Teams[pl:GetTeam()]:IsHasTech(t.ID) then n = n + 1 end end
  return pl:GetNumFreeTechs(), pl:GetEndTurnBlockingType(), n
end
-- A tech the seat may take for free (the TechTree's "FREE" buttons), the cheapest last one so it is not the AI's pick.
function Pick(p)
  local pl = Players[p]
  local best, cost = -1, -1
  for t in GameInfo.Technologies() do
    if pl:CanResearchForFree(t.ID) and pl:GetResearchCost(t.ID) >= cost then best, cost = t.ID, pl:GetResearchCost(t.ID) end
  end
  return best
end
function Has(p, tech) return Teams[Players[p]:GetTeam()]:IsHasTech(tech) end
"#;

fn ints(lua: &lua_sys::Lua, f: &str, args: &[i32]) -> Vec<i32> {
    let args: Vec<Value> = args.iter().map(|a| Value::from(*a)).collect();
    lua.call_global(f, &args)
        .unwrap_or_else(|e| panic!("{f}: {e}"))
        .iter()
        .map(|v| match v {
            Value::Boolean(b) => i32::from(*b),
            v => v.as_i32().unwrap_or(-99),
        })
        .collect()
}

#[test]
fn live_free_tech_is_the_humans_choice() {
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
        .unwrap_or(0);
    let bytes = std::fs::read(&file).expect("CIV5_LIVE_SAVE");
    let save = game_host::saves::decode(&bytes).expect("a C5SV save");
    let mut rep = ReplicaGame::new(DirHost(data), None).unwrap();
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
    lua.exec(PROBE, "=probe").unwrap();
    let p = i32::from(player);

    let before = ints(&lua, "State", &[p]);
    assert_eq!(ints(&lua, "Grant", &[p]), vec![1], "Oxford placed");
    for _ in 0..4 {
        let _ = rep.game().pump(64).unwrap();
    }
    let after = ints(&lua, "State", &[p]);
    eprintln!("before (free, blocking, techs) {before:?} after the grant {after:?}");
    assert_eq!(
        after[0],
        before[0] + 1,
        "the human's counter took the free tech"
    );
    assert_eq!(after[2], before[2], "no tech was picked for the human");
    // ENDTURN_BLOCKING_FREE_TECH = 6 (CvEnums.h EndTurnBlockingTypes)
    let free_tech_blocker = 6;
    let ended = rep.game().command(player, &Command::EndTurn);
    for _ in 0..4 {
        let _ = rep.game().pump(64).unwrap();
    }
    let blocked = ints(&lua, "State", &[p]);
    eprintln!("END_TURN with the free tech: {ended:?}, state {blocked:?}");
    assert_eq!(
        blocked[0], after[0],
        "the free tech is still the human's to choose"
    );
    assert_eq!(blocked[2], after[2], "still no tech picked");
    assert!(
        blocked[1] == free_tech_blocker || after[1] != free_tech_blocker,
        "blocker {blocked:?}"
    );

    let tech = ints(&lua, "Pick", &[p])[0];
    assert!(tech >= 0, "a tech to take for free");
    let m = NetMessage::new("sendResearch", &[tech, blocked[0], -1, 0]).unwrap();
    let sent = rep.game().net_submit(player, &[m]);
    eprintln!("sendResearch {tech}: {sent:?}");
    for _ in 0..4 {
        let _ = rep.game().pump(64).unwrap();
    }
    let chosen = ints(&lua, "State", &[p]);
    eprintln!("after the choice {chosen:?}");
    assert_eq!(ints(&lua, "Has", &[p, tech]), vec![1], "the chosen tech");
    assert_eq!(chosen[0], blocked[0] - 1);
    assert_eq!(chosen[2], blocked[2] + 1, "exactly one tech");
}
