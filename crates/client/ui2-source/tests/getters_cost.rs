//! Cost of every `GameRead` method on the real core, natively: the time of one call, median and maximum, on the
//! late-huge game (a game of turn 150 with the biggest map). `cargo xtask web-selftest --scenario ui2-getters`
//! measures the same in wasm; the two tables calibrate the source's read budget (`DEFAULT_READ_BUDGET`).
//!
//! Ignored: it needs the real core (Linux/WSL, `GAMECORE_LIB_DIR_x86_64`) and the fixture of the replica test,
//! written by `REPLICA_FIXTURE_OUT=<dir> cargo test -p replica --test real_replica -- --ignored late_huge`.
//! `UI2_GETTERS_FIXTURE=<dir>/late-huge` names the scenario directory; `UI2_GETTERS_SEAT=<n>` joins as another seat than the script's (the player with the most cities is the worst case); `UI2_GETTERS_OUT=<file>` also writes the table
//! as JSON lines. Returns early on the stub.
//!
//! ```text
//! UI2_GETTERS_FIXTURE=<dir>/late-huge cargo test -p ui2-source --test getters_cost -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use protocol::xxh64;
use replica::{
    CatalogTable, GameDataHost, GameFiles, GameRead, GcBase, GcBlobRef, Replica, ReplicaMsg,
    ReplicaStep,
};
use serde_json::Value;

/// Calls timed per method (the id-taking methods cycle through the seat's units or cities).
const SAMPLES: usize = 200;

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("script: no {key}"))
}

fn num(v: &Value, key: &str) -> u64 {
    v.get(key)
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("script: no {key}"))
}

fn read(dir: &Path, name: &str) -> Vec<u8> {
    std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("fixture: {name}: {e}"))
}

/// The game data of the fixture, as the core reads it.
fn game_files(dir: &Path) -> GameFiles {
    let index = read(dir, "gamedata/vfs-index.json");
    let parsed: Value = serde_json::from_slice(&index).expect("vfs-index.json");
    let mut g = GameFiles::from_index(index).expect("vfs index");
    let mut put = |id: &str, path: &str| {
        let file = dir.join(format!("gamedata/{path}"));
        if let Ok(b) = std::fs::read(file) {
            g.put(id, b);
        }
    };
    put(text(&parsed["db"], "gameplay"), "db/gameplay.db");
    put(text(&parsed["db"], "localization"), "db/loc-en_us.db");
    for f in parsed["files"].as_array().expect("index: no files") {
        put(text(f, "id"), text(f, "path"));
    }
    g
}

/// A replica of the seat that joined the first snapshot of the script.
fn joined(dir: &Path) -> Replica {
    let script: Value = serde_json::from_slice(&read(dir, "script.json")).expect("script.json");
    let seat = std::env::var("UI2_GETTERS_SEAT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| script.get("seat").and_then(Value::as_u64).unwrap_or(0) as u8);
    let data = Arc::new(Mutex::new(game_files(dir)));
    let mut r = Replica::new(GameDataHost::new(data), seat).expect("the core boots");
    r.config_mut().pack_base = false;
    let base = script["steps"]
        .as_array()
        .expect("script: no steps")
        .iter()
        .find(|s| s["op"] == "base")
        .expect("script: no base step");
    let body = read(dir, text(base, "body"));
    let blob = GcBlobRef {
        hash: xxh64(0, &body),
        size: body.len() as u32,
    };
    r.on_msg(ReplicaMsg::Base(GcBase {
        epoch: num(base, "epoch") as u32,
        turn: num(base, "turn") as i32,
        jseq: num(base, "jseq"),
        raw_hash: u64::from_str_radix(text(base, "raw_hash"), 16).expect("raw_hash"),
        snapshot: blob,
    }));
    r.blob(blob.hash, body);
    assert_eq!(r.pump(0), ReplicaStep::Working, "the unpack stage");
    assert!(
        matches!(r.pump(0), ReplicaStep::Loaded { .. }),
        "the load stage"
    );
    r
}

/// One method's times, µs.
struct Row {
    name: String,
    first: f64,
    median: f64,
    max: f64,
    mean: f64,
    calls: usize,
    /// Rows the last answer had (a size of the answer).
    items: usize,
}

/// Times `call` `SAMPLES` times (`call(i)` is the i-th call; its result is the answer's item count).
fn measure(name: &str, calls: usize, mut call: impl FnMut(usize) -> usize) -> Row {
    let mut us = Vec::with_capacity(calls);
    let mut items = 0;
    for i in 0..calls {
        let t = Instant::now();
        items = call(i);
        us.push(t.elapsed().as_secs_f64() * 1e6);
    }
    let first = us.first().copied().unwrap_or(0.0);
    let mean = us.iter().sum::<f64>() / us.len().max(1) as f64;
    us.sort_by(f64::total_cmp);
    Row {
        name: name.to_owned(),
        first,
        median: us.get(us.len() / 2).copied().unwrap_or(0.0),
        max: us.last().copied().unwrap_or(0.0),
        mean,
        calls: us.len(),
        items,
    }
}

fn table(g: &mut dyn GameRead) -> Vec<Row> {
    let seat = g.seat() as i8;
    let mut rows = Vec::new();
    rows.push(measure("game_info", SAMPLES, |_| {
        g.game_info().map_or(0, |_| 1)
    }));
    rows.push(measure("top_bar", SAMPLES, |_| {
        g.top_bar().map_or(0, |_| 1)
    }));
    rows.push(measure("players", SAMPLES, |_| {
        g.players().map_or(0, |v| v.len())
    }));
    rows.push(measure("notifications", SAMPLES, |_| {
        g.notifications().map_or(0, |v| v.len())
    }));
    rows.push(measure("techs", SAMPLES, |_| {
        g.techs().map_or(0, |v| v.len())
    }));
    rows.push(measure("player_resources", SAMPLES, |_| {
        g.player_resources().map_or(0, |v| v.len())
    }));
    rows.push(measure("policy_branches", SAMPLES, |_| {
        g.policy_branches().map_or(0, |v| v.len())
    }));
    let units = g.units().unwrap_or_default();
    rows.push(measure("units", SAMPLES, |_| {
        g.units().map_or(0, |v| v.len())
    }));
    let cities = g.cities().unwrap_or_default();
    rows.push(measure("cities", SAMPLES, |_| {
        g.cities().map_or(0, |v| v.len())
    }));
    let own_units: Vec<i32> = units
        .iter()
        .filter(|u| u.owner == seat)
        .map(|u| u.id)
        .collect();
    let own_cities: Vec<i32> = cities
        .iter()
        .filter(|c| c.owner == seat)
        .map(|c| c.id)
        .collect();
    println!(
        "UI2GETTERS own units {}, own cities {}, all units {}, all cities {}",
        own_units.len(),
        own_cities.len(),
        units.len(),
        cities.len()
    );
    if !own_units.is_empty() {
        let n = SAMPLES.min(own_units.len().max(SAMPLES / 4));
        rows.push(measure("unit_actions", n, |i| {
            g.unit_actions(own_units[i % own_units.len()])
                .map_or(0, |v| v.len())
        }));
        rows.push(measure("unit_promotions", n, |i| {
            g.unit_promotions(own_units[i % own_units.len()])
                .map_or(0, |v| v.len())
        }));
    }
    if !own_cities.is_empty() {
        let n = SAMPLES.min(own_cities.len().max(SAMPLES / 4));
        rows.push(measure("city_builds", n, |i| {
            g.city_builds(own_cities[i % own_cities.len()])
                .map_or(0, |v| v.len())
        }));
        rows.push(measure("city_queue", n, |i| {
            g.city_queue(own_cities[i % own_cities.len()])
                .map_or(0, |v| v.len())
        }));
    }
    for t in CatalogTable::ALL.into_iter().chain([
        CatalogTable::ArtTechs,
        CatalogTable::GameSpeeds,
        CatalogTable::PolicyBranches,
    ]) {
        rows.push(measure(&format!("catalog({t:?})"), SAMPLES / 4, |_| {
            g.catalog(t).map_or(0, |v| v.len())
        }));
    }
    rows
}

fn fixture_dir() -> Option<PathBuf> {
    std::env::var_os("UI2_GETTERS_FIXTURE").map(PathBuf::from)
}

#[test]
#[ignore = "needs the real core and the late-huge fixture (UI2_GETTERS_FIXTURE)"]
fn getters_cost_native() {
    let Some(dir) = fixture_dir() else {
        eprintln!(
            "skipped: UI2_GETTERS_FIXTURE=<fixture dir>/late-huge is not set (and the real core is linked)"
        );
        return;
    };
    let mut r = joined(&dir);
    let g = r.read().expect("the replica is live after the join");
    // Warm-up: the first answers fill the core's own caches; `first` of each row still shows the cold call.
    let rows = table(g);
    println!(
        "UI2GETTERS method                         calls  first_us median_us   max_us  mean_us  items"
    );
    let mut json = String::new();
    for r in &rows {
        println!(
            "UI2GETTERS {:<32} {:>5} {:>9.1} {:>9.1} {:>8.1} {:>8.1} {:>6}",
            r.name, r.calls, r.first, r.median, r.max, r.mean, r.items
        );
        json.push_str(&format!(
            "{{\"method\":{:?},\"calls\":{},\"first_us\":{:.1},\"median_us\":{:.1},\"max_us\":{:.1},\"mean_us\":{:.1},\"items\":{}}}\n",
            r.name, r.calls, r.first, r.median, r.max, r.mean, r.items
        ));
    }
    if let Some(path) = std::env::var_os("UI2_GETTERS_OUT") {
        std::fs::write(path, json).expect("UI2_GETTERS_OUT");
    }
    assert!(rows.iter().any(|r| r.items > 0), "no read returned data");
}
