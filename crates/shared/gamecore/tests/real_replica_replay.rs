//! The end-game ReplayViewer reads the whole game on show (`GenerateReplayInfoFromCurrentGame`). On the replica this trapped
//! the wasm (live crash, game 943, turn 143). Returns early on the stub; run on Linux/WSL with `GAMECORE_LIB_DIR_x86_64` and
//! `CIV5_GAMEDATA` set.

use std::path::PathBuf;

use gamecore::replica::REPLICA_DIGEST;
use gamecore::{Game, GameEvent, HostServices, Phase, ReplicaGame, Role, Setup, SlotSetup};
use gamecore_sys::ffi;
use lua_sys::Value;

const PROBE: &str = include_str!("fixtures/replay_viewer.lua");

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

fn pump_to_save_point(game: &mut Game, after: i32) -> (i32, Vec<u8>) {
    for _ in 0..20_000 {
        let r = game.pump(64).unwrap();
        assert!(
            r.status == ffi::GC_PUMP_AI_RUNNING || r.status == ffi::GC_PUMP_WAITING_HUMAN,
            "{r:?}"
        );
        let humans = game.game_info().unwrap().phase == Phase::Humans;
        let _ = game.journal_take(humans).unwrap();
        let _ = game.state_digest(REPLICA_DIGEST).unwrap();
        if r.turn > after
            && let Ok(snap) = game.save()
        {
            return (r.turn, snap);
        }
    }
    panic!("no save point after turn {after}");
}

#[test]
fn replay_viewer_reads_the_replica() {
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
    setup.world_size = std::env::var("REPLAY_WORLD").unwrap_or("WORLDSIZE_TINY".into());
    setup.handicap = "HANDICAP_CHIEFTAIN".into();
    setup.autoplay_humans = vec![0, 1];
    rep.game().new_game(&setup).unwrap();
    rep.game().set_role(Role::Server).unwrap();
    let (start, first) = pump_to_save_point(rep.game(), -1);
    let mut turn = start;
    let mut last = first.clone();
    let turns: i32 = std::env::var("REPLAY_TURNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    while turn < start + turns {
        let (t, snap) = pump_to_save_point(rep.game(), turn);
        turn = t;
        last = snap;
    }

    // As the server (the live game of a host) and as a replica resynced from a save.
    for replica in [false, true] {
        if replica {
            rep.game()
                .set_role(Role::Replica { seat: Some(0) })
                .unwrap();
            rep.game().resync(&last).unwrap();
        }
        let lua = lua_sys::Lua::new().unwrap();
        rep.attach_lua(&lua).unwrap();
        lua.exec(PROBE, "=replay_viewer").unwrap();
        if replica {
            rep.setup_graphical().unwrap();
        }
        lua.register("TraceStep", |c| {
            if let Value::String(s) = c.arg(1) {
                eprintln!("step {s}");
            }
            Ok(0)
        })
        .unwrap();
        let r = lua.call_global("ReplayProbe", &[]).expect("ReplayProbe");
        let Some(Value::String(log)) = r.get(2) else {
            panic!("no log: {r:?}")
        };
        eprintln!(
            "replica={replica} sets={:?} empty={:?}\n{log}",
            r.first(),
            r.get(1)
        );
        assert!(
            log.lines().all(|l| l.ends_with("=ok")
                || l.starts_with("messages=")
                || l.starts_with("deals=")),
            "{log}"
        );
        // Players nothing was recorded for have empty data sets (the history getters create them): reading them aborted the process.
        assert!(
            r.get(1).and_then(Value::as_i32).unwrap_or(0) > 0,
            "no empty data set: the case is not tested"
        );
        let t0 = std::time::Instant::now();
        let r = lua.call_global("ReplayFull", &[]);
        eprintln!(
            "the full replay read (replica={replica}): {:?} in {:?}",
            r.as_ref().map(|v| v.len()),
            t0.elapsed()
        );
        r.expect("ReplayFull");

        rep.invalidate_lua(&lua);
    }
}
