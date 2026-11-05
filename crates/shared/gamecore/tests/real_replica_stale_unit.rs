//! Regression test for the live crash of build 5bea73c, turn 106 ("memory access out of bounds" in `CvUnit::IsGarrisoned`
//! from `CvLuaUnit::lIsGarrisoned`; a Great Musician's "Create Great Work" froze a game): a Lua instance of a unit or city
//! (the DLL's `CvLuaScopedInstance`: a table with the raw pointer in `__instance`, cached by pointer in `<Type>.__instances`)
//! outlives the object, because the replica frees it while it replays a journal batch. Returns early on the stub; run on
//! Linux/WSL with `GAMECORE_LIB_DIR_x86_64` and `CIV5_GAMEDATA` set.
//!
//! Set-up as in `real_replica_ui.rs`: one core instance plays the server (journal), then a replica of seat 0 with a Lua state.
//! Before each batch the Lua objects of every unit and city are kept (`Snap`); after it those no player lists any more are
//! the stale ones (`Gone`). Checked:
//!  a. without the sweep, `__instance` of such an object is still a non-nil lightuserdata (a method call would be a
//!     use-after-free: never made on it here);
//!  b. after the sweep (`SWEEP_DEAD_LUA` of `exe-api`, copied; gamecore cannot depend on exe-api) `__instance` is nil, a method
//!     call raises "Not a valid instance", and a live unit and city still work.

use std::path::PathBuf;

use gamecore::replica::REPLICA_DIGEST;
use gamecore::{
    Game, GameEvent, HostServices, Phase, ReplicaGame, Role, Setup, SlotSetup, UiEventKind,
};
use gamecore_sys::ffi;
use lua_sys::Value;

const TURNS: i32 = 40;

/// Copy of `SWEEP_DEAD_LUA` in crates/client/exe-api/src/lib.rs (keep in step).
const SWEEP_DEAD_LUA: &str = r#"
local live = {}
local n = (GameDefines and GameDefines.MAX_PLAYERS) or 64
for i = 0, n - 1 do
  local p = Players[i]
  if p ~= nil then
    for u in p:Units() do live[u] = true end
    for c in p:Cities() do live[c] = true end
  end
end
local dropped = 0
for _, T in ipairs({ Unit, City }) do
  local cache = type(T) == "table" and rawget(T, "__instances")
  if type(cache) == "table" then
    for k, t in pairs(cache) do
      if not live[t] then
        if type(t) == "table" then rawset(t, "__instance", nil) end
        cache[k] = nil
        dropped = dropped + 1
      end
    end
  end
end
return dropped
"#;

const HELPERS: &str = r##"
SNAP = {}
KEPT = {}
local function LiveSet()
  local live = {}
  for i = 0, 63 do
    local p = Players[i]
    if p ~= nil then
      for u in p:Units() do live[u] = true end
      for c in p:Cities() do live[c] = true end
    end
  end
  return live
end

-- Keeps the Lua object of every unit and city; returns how many, and how many GetUnitByID answers differ from Units().
function Snap()
  SNAP = {}
  local differ = 0
  for i = 0, 63 do
    local p = Players[i]
    if p ~= nil then
      for u in p:Units() do
        SNAP[#SNAP + 1] = { u, "unit", i, u:GetID() }
        if p:GetUnitByID(u:GetID()) ~= u then differ = differ + 1 end
      end
      for c in p:Cities() do SNAP[#SNAP + 1] = { c, "city", i, c:GetID() } end
    end
  end
  return #SNAP, differ
end

-- After a batch: moves the kept objects that no player lists any more to KEPT; returns gone units, gone cities.
-- Touches no object but the live ones.
function Gone(batch)
  local live = LiveSet()
  local nu, nc = 0, 0
  for _, e in ipairs(SNAP) do
    if not live[e[1]] then
      KEPT[#KEPT + 1] = { obj = e[1], kind = e[2], pid = e[3], id = e[4], batch = batch }
      if e[2] == "unit" then nu = nu + 1 else nc = nc + 1 end
    end
  end
  return nu, nc
end

-- The kept objects that are still gone (a freed address can be handed to a new object of the cache): count, how many still
-- hold a non-nil lightuserdata in __instance, how many units, how many cities. Reads the field only.
function StaleStats()
  local live = LiveSet()
  local stale, withptr, units, cities = 0, 0, 0, 0
  for _, k in ipairs(KEPT) do
    if not live[k.obj] then
      stale = stale + 1
      if k.kind == "unit" then units = units + 1 else cities = cities + 1 end
      if type(rawget(k.obj, "__instance")) == "userdata" then withptr = withptr + 1 end
    end
  end
  return stale, withptr, units, cities
end

-- After the sweep: stale, with nil __instance, whose method raised "Not a valid instance", still in the caches.
function AfterSweep()
  local live = LiveSet()
  local stale, nilptr, raised, incache = 0, 0, 0, 0
  local bad = ""
  for _, k in ipairs(KEPT) do
    if not live[k.obj] then
      stale = stale + 1
      if rawget(k.obj, "__instance") == nil then nilptr = nilptr + 1 end
      local ok, err
      if k.kind == "unit" then
        ok, err = pcall(function() return k.obj:IsGarrisoned() end)
      else
        ok, err = pcall(function() return k.obj:IsCapital() end)
      end
      if ok == false and string.find(tostring(err), "Not a valid instance", 1, true) then
        raised = raised + 1
      else
        bad = bad .. k.kind .. "#" .. k.id .. ":" .. tostring(ok) .. ":" .. tostring(err) .. " "
      end
    end
  end
  for _, T in ipairs({ Unit, City }) do
    for _, t in pairs(rawget(T, "__instances")) do
      for _, k in ipairs(KEPT) do
        if k.obj == t and not live[t] then incache = incache + 1 end
      end
    end
  end
  return stale, nilptr, raised, incache, bad
end

-- A live unit and a live city still answer; returns units tried, cities tried, errors.
function LiveStillWork()
  local nu, nc, errs = 0, 0, ""
  for i = 0, 63 do
    local p = Players[i]
    if p ~= nil then
      for u in p:Units() do
        local ok, err = pcall(function() return u:IsGarrisoned(), u:GetID(), u:GetX() end)
        if ok then nu = nu + 1 else errs = errs .. tostring(err) .. " " end
      end
      for c in p:Cities() do
        local ok, err = pcall(function() return c:IsCapital(), c:GetID(), c:GetX() end)
        if ok then nc = nc + 1 else errs = errs .. tostring(err) .. " " end
      end
    end
  end
  return nu, nc, errs
end

-- Diagnostic: stale kept objects that no __instances cache holds (the sweep cannot reach them).
function NotCached()
  local live = LiveSet()
  local out = {}
  for _, k in ipairs(KEPT) do
    if not live[k.obj] then
      local found = false
      for _, T in ipairs({ Unit, City }) do
        for _, t in pairs(rawget(T, "__instances")) do if t == k.obj then found = true end end
      end
      if not found then out[#out + 1] = k.kind .. " p" .. k.pid .. " id" .. k.id .. " batch" .. k.batch end
    end
  end
  return table.concat(out, "; ")
end

function Describe()
  local t = {}
  for _, k in ipairs(KEPT) do t[#t + 1] = k.kind .. " p" .. k.pid .. " #" .. k.id .. " (batch " .. k.batch .. ")" end
  return table.concat(t, "; ")
end
"##;

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

fn gamedata_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CIV5_GAMEDATA") {
        return PathBuf::from(d);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .flat_map(|a| {
            ["gamedata-b2", "gamedata-b1"]
                .into_iter()
                .map(move |n| a.join("assets-out").join(n))
        })
        .find(|d| d.join("vfs-index.json").is_file())
        .expect("game data not found: set CIV5_GAMEDATA=<export-gamedata dir>")
}

/// Pumps until the game can be saved at a turn after `after`; every batch and the server digest after it are kept.
fn pump_to_save_point(
    game: &mut Game,
    after: i32,
    batches: &mut Vec<(Vec<u8>, u64)>,
) -> (i32, Vec<u8>) {
    for _ in 0..20_000 {
        let r = game.pump(64).unwrap();
        assert!(
            r.status == ffi::GC_PUMP_AI_RUNNING || r.status == ffi::GC_PUMP_WAITING_HUMAN,
            "{r:?}"
        );
        let humans = game.game_info().unwrap().phase == Phase::Humans;
        let batch = game.journal_take(humans).unwrap();
        let digest = game.state_digest(REPLICA_DIGEST).unwrap();
        batches.push((batch, digest));
        if r.turn > after
            && let Ok(snap) = game.save()
        {
            return (r.turn, snap);
        }
    }
    panic!("no save point after turn {after}");
}

fn nums(v: &[Value]) -> Vec<i32> {
    v.iter().filter_map(Value::as_i32).collect()
}

#[test]
fn replica_unit_instances_outlive_their_unit_until_swept() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let mut rep = ReplicaGame::new(DirHost(gamedata_dir()), Some(0)).unwrap();
    let mut setup = Setup::network(
        1,
        vec![
            SlotSetup::human("random", "a"),
            SlotSetup::human("random", "b"),
            SlotSetup::ai("random"),
            SlotSetup::ai("random"),
        ],
    );
    setup.world_size = "WORLDSIZE_TINY".into();
    setup.handicap = "HANDICAP_CHIEFTAIN".into();
    setup.autoplay_humans = vec![0, 1];
    let game = rep.game();
    game.new_game(&setup).unwrap();
    game.set_role(Role::Server).unwrap();

    let mut ignored = Vec::new();
    let (start, first) = pump_to_save_point(game, -1, &mut ignored);
    let mut batches = Vec::new();
    let mut turn = start;
    while turn < start + TURNS {
        turn = pump_to_save_point(game, turn, &mut batches).0;
    }
    assert!(batches.len() > 20, "{} batches", batches.len());

    game.set_role(Role::Replica { seat: Some(0) }).unwrap();
    game.resync(&first).unwrap();
    let lua = lua_sys::Lua::new().unwrap();
    rep.attach_lua(&lua).unwrap();
    lua.exec(HELPERS, "=stale_helpers").unwrap();
    rep.setup_graphical().unwrap();
    let mut events = Vec::new();
    rep.take_ui_events(&mut events);

    let (mut gone_units, mut gone_cities, mut gone_batches) = (0, 0, 0);
    let mut differ = 0;
    let mut events_out = Vec::new();
    for (i, (batch, digest)) in batches.iter().enumerate() {
        let snap = nums(&lua.call_global("Snap", &[]).unwrap());
        differ += snap[1];
        rep.game()
            .journal_apply(batch)
            .unwrap_or_else(|e| panic!("batch {i}: {e}"));
        assert_eq!(
            rep.game().state_digest(REPLICA_DIGEST).unwrap(),
            *digest,
            "digest after batch {i}"
        );
        events_out.clear();
        rep.take_ui_events(&mut events_out);
        let died = events_out
            .iter()
            .filter(|e| {
                matches!(
                    e.kind,
                    UiEventKind::UnitDestroyed | UiEventKind::UnitDestroyedInCombat
                )
            })
            .count();
        let g = nums(&lua.call_global("Gone", &[Value::from(i as i32)]).unwrap());
        if g[0] + g[1] > 0 {
            gone_batches += 1;
            eprintln!(
                "batch {i}: {} units, {} cities gone ({died} destroy events)",
                g[0], g[1]
            );
        }
        gone_units += g[0];
        gone_cities += g[1];
    }
    eprintln!(
        "{} batches: {gone_units} unit and {gone_cities} city objects gone in {gone_batches} batches; GetUnitByID differs from Units() {differ} times",
        batches.len()
    );
    assert_eq!(
        differ, 0,
        "GetUnitByID returned another object than Units()"
    );
    assert!(
        gone_units > 0,
        "no unit died or was consumed in {TURNS} turns: raise TURNS"
    );

    // a. Without the sweep the dead units' tables still hold their raw pointer (a method call would be a use-after-free).
    let st = nums(&lua.call_global("StaleStats", &[]).unwrap());
    let (stale, withptr, su, sc) = (st[0], st[1], st[2], st[3]);
    eprintln!(
        "stale objects: {stale} ({su} units, {sc} cities), {withptr} with a non-nil __instance"
    );
    if let Some(Value::String(d)) = lua.call_global("Describe", &[]).unwrap().first() {
        eprintln!("kept: {d}");
    }
    assert!(stale > 0 && su > 0);
    assert_eq!(
        withptr, stale,
        "every stale object still holds its raw pointer (this is the crash: the DLL derefs it)"
    );

    // b. The sweep clears them; a call is a Lua error; live objects still work.
    if let Some(Value::String(n)) = lua.call_global("NotCached", &[]).unwrap().first() {
        eprintln!("stale and not in any cache: {n}");
    }
    let dropped = lua
        .load(SWEEP_DEAD_LUA)
        .set_name("=sweep")
        .eval::<i32>()
        .unwrap();
    eprintln!("sweep dropped {dropped}");
    eprintln!("dropped {dropped} for {stale} stale");
    let r = lua.call_global("AfterSweep", &[]).unwrap();
    let after = nums(&r);
    eprintln!(
        "after the sweep: {} stale, {} nil __instance, {} raised 'Not a valid instance', {} still cached",
        after[0], after[1], after[2], after[3]
    );
    if let Some(Value::String(bad)) = r.get(4)
        && !bad.is_empty()
    {
        panic!("a stale object did not raise 'Not a valid instance': {bad}");
    }
    assert_eq!(after[0], stale);
    assert_eq!(after[1], stale, "__instance is nil after the sweep");
    assert_eq!(after[2], stale, "a method raises 'Not a valid instance'");
    assert_eq!(after[3], 0, "no stale object is left in the caches");

    let live = lua.call_global("LiveStillWork", &[]).unwrap();
    let l = nums(&live);
    if let Some(Value::String(e)) = live.get(2)
        && !e.is_empty()
    {
        panic!("a live object broke after the sweep: {e}");
    }
    eprintln!(
        "live after the sweep: {} units, {} cities answer",
        l[0], l[1]
    );
    assert!(l[0] > 0 && l[1] > 0);
}
