//! The replica's UI bridge on the real Lekmod gamecore. Returns early on the stub; run on
//! Linux/WSL with `GAMECORE_LIB_DIR_x86_64` and `CIV5_GAMEDATA` set.
//!
//! The core is a process singleton, so one instance plays both parts, as in `real_replica.rs`: as the server it plays turns
//! of two test-bot humans and two AI and keeps the journal; then the instance becomes a replica of seat 0 with a Lua state
//! attached (`attach_lua`), the UI events subscribed, `setup_graphical` run, and the batches applied one by one. Between the
//! batches a Lua script calls ~780 getters of the original UI (`fixtures/eui_k1_k3_getters.lua`) and `ui_op` moves the
//! selection. The replica digest must equal the server's after every batch; after a resync the old game's objects must raise a
//! Lua error while `Players` stays valid.

use std::path::PathBuf;

use gamecore::replica::REPLICA_DIGEST;
use gamecore::{
    Game, GameEvent, HostServices, Phase, ReplicaGame, Role, Setup, SlotSetup, UiEvent,
    UiEventKind, UiOp,
};
use gamecore_sys::ffi;
use lua_sys::Value;

const GETTERS: &str = include_str!("fixtures/eui_k1_k3_getters.lua");
const TURNS: i32 = 30;

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

/// One pass of the getters: (called, raised, missing in this DLL).
fn getters(lua: &lua_sys::Lua, show: bool) -> (i32, i32, i32) {
    let r = lua.call_global("GettersPass", &[]).expect("GettersPass");
    let n = nums(&r);
    if show
        && let Some(Value::String(s)) = r.get(3)
        && !s.is_empty()
    {
        eprintln!("getter errors (samples): {s}");
    }
    (n[0], n[1], n[2])
}

#[test]
fn real_replica_serves_the_ui() {
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
    let mut last = Vec::new();
    while turn < start + TURNS {
        let (t, snap) = pump_to_save_point(game, turn, &mut batches);
        turn = t;
        last = snap;
    }
    assert!(batches.len() > 20, "{} batches", batches.len());

    // The same instance as a replica of seat 0, with the UI's Lua state.
    game.set_role(Role::Replica { seat: Some(0) }).unwrap();
    game.resync(&first).unwrap();

    let lua = lua_sys::Lua::new().unwrap();
    rep.attach_lua(&lua).unwrap();
    lua.exec(GETTERS, "=eui_k1_k3_getters").unwrap();
    if let Some(skip) = std::env::var_os("GETTER_SKIP") {
        let list: Vec<String> = skip
            .to_string_lossy()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| format!("[\"{s}\"]=true"))
            .collect();
        lua.exec(&format!("SKIP_EXTRA = {{{}}}", list.join(",")), "=skip")
            .unwrap();
    }
    if std::env::var_os("GETTER_TRACE").is_some() {
        lua.register("GetterTrace", |c| {
            if let Value::String(s) = c.arg(1) {
                eprintln!("trace {s}");
            }
            Ok(0)
        })
        .unwrap();
    }
    for name in ["PlayerAdoptPolicy", "PlayerDoTurn"] {
        rep.subscribe_game_event(name);
    }
    rep.setup_graphical().unwrap();
    let mut events: Vec<UiEvent> = Vec::new();
    rep.take_ui_events(&mut events);

    // setup_graphical: exactly one created per living unit and city, all flagged as a rebuild.
    let alive = nums(&lua.call_global("CountAlive", &[]).unwrap());
    let (units, cities) = (alive[0] as usize, alive[1] as usize);
    assert!(units > 0 && cities > 0, "{units} units, {cities} cities");
    let count = |k: &[UiEventKind]| {
        events
            .iter()
            .filter(|e| e.rebuild && k.contains(&e.kind))
            .count()
    };
    assert_eq!(
        count(&[UiEventKind::UnitCreated, UiEventKind::UnitEmbark]),
        units
    );
    assert_eq!(count(&[UiEventKind::CityCreated]), cities);
    eprintln!(
        "setup: {units} units, {cities} cities, {} events",
        events.len()
    );

    if std::env::var_os("GETTER_BISECT").is_some() {
        let all = lua.call_global("AllNames", &[]).unwrap();
        let Some(Value::String(all)) = all.first() else {
            panic!("AllNames")
        };
        let mut base = rep.game().state_digest(REPLICA_DIGEST).unwrap();
        for name in all.lines() {
            eprintln!("bisect {name}");
            let _ = lua.call_global("GettersPass", &[Value::from(name)]);
            let now = rep.game().state_digest(REPLICA_DIGEST).unwrap();
            if now != base {
                eprintln!("MOVES THE DIGEST: {name}");
                base = now;
            }
        }
    }
    let (calls, errs, missing) = getters(&lua, true);
    eprintln!(
        "getters on the loaded game: {calls} called, {errs} raised, {missing} not in this DLL"
    );
    assert!(calls > 500, "{calls} getters ran");
    assert!(missing < calls / 4, "{missing} getters missing");

    let mut out = Vec::new();
    let mut adopted = 0usize;
    let mut turns_done = 0usize;
    let mut picked = 0usize;
    let total = batches.len();
    for (i, (batch, digest)) in batches.iter().enumerate() {
        rep.game()
            .journal_apply(batch)
            .unwrap_or_else(|e| panic!("batch {i} of {total}: {e}"));
        assert_eq!(
            rep.game().state_digest(REPLICA_DIGEST).unwrap(),
            *digest,
            "digest after batch {i}"
        );
        events.clear();
        rep.take_ui_events(&mut events);
        for e in &events {
            if e.kind == UiEventKind::GameEvent {
                match e.text.as_deref() {
                    Some("PlayerAdoptPolicy") => adopted += 1,
                    Some("PlayerDoTurn") => turns_done += 1,
                    _ => {}
                }
            }
        }
        if i % 8 == 0 {
            let (calls, _errs, _missing) = getters(&lua, false);
            assert!(calls > 500);
            // The selection of seat 0 moves; the game does not.
            let unit = nums(&lua.call_global("FirstUnit", &[Value::from(0)]).unwrap());
            if let [owner, id] = unit[..] {
                rep.ui_op(UiOp::SelectUnit, &[owner, id], &mut out).unwrap();
                out.clear();
                rep.ui_op(UiOp::HeadSelectedUnit, &[], &mut out).unwrap();
                assert_eq!(out, [owner, id]);
                out.clear();
                rep.ui_op(UiOp::GetInterfaceMode, &[], &mut out).unwrap();
                let mode = out[0];
                out.clear();
                rep.ui_op(UiOp::SetInterfaceMode, &[mode], &mut out)
                    .unwrap();
                rep.ui_op(UiOp::ClearSelection, &[], &mut out).unwrap();
                out.clear();
                picked += 1;
            }
            assert_eq!(
                rep.game().state_digest(REPLICA_DIGEST).unwrap(),
                *digest,
                "the digest moved by the UI after batch {i}"
            );
        }
    }
    eprintln!(
        "{total} batches: {turns_done} PlayerDoTurn, {adopted} PlayerAdoptPolicy, {picked} selections"
    );
    assert!(turns_done > 0, "PlayerDoTurn never reached the UI");
    assert!(
        adopted > 0,
        "PlayerAdoptPolicy never reached the UI in {TURNS} turns"
    );
    assert!(picked > 0);

    // After a resync the old game's objects raise a Lua error; Players stay valid.
    lua.call_global("KeepOldUnit", &[Value::from(0)]).unwrap();
    rep.game().resync(&last).unwrap();
    rep.invalidate_lua(&lua);
    assert!(
        lua.call_global("OldUnitId", &[]).is_err(),
        "an object of the old game is still usable"
    );
    let name = lua.call_global("MyName", &[]).unwrap();
    assert!(
        matches!(name.first(), Some(Value::String(s)) if !s.is_empty()),
        "{name:?}"
    );
    let (calls, _errs, _missing) = getters(&lua, false);
    assert!(calls > 500, "the getters work on the new game");
}
