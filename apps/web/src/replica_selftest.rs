//! `replica_load` of `cargo xtask web-selftest`: the scripted replica run of `replica::selftest` in the browser.
//! The page (`replica-selftest.html`) fetches the fixture files and hands them over; the report comes back as JSON.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use js_sys::{Array, Object, Uint8Array};
use platform_web::WebClock;
use protocol::xxh64;
use replica::{
    CatalogTable, GameDataHost, GameFiles, GameRead, GcBase, GcBlobRef, Replica, ReplicaMsg,
    ReplicaStep,
};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

/// The core's clocks must be set before it boots (the C runtime reads 1970 otherwise).
fn set_core_clock() {
    let perf_ns = web_sys::window()
        .and_then(|w| w.performance())
        .map_or(0.0, |p| p.now() * 1e6);
    let wall_ns = js_sys::Date::now() * 1e6;
    replica::selftest::set_clock(wall_ns as u64, perf_ns as u64);
}

/// Runs the script in `files` (an object: fixture file name to `Uint8Array`) and returns the report as JSON
/// (`{"ok":true,"scenario":..,<metrics>}` or `{"ok":false,"error":..}`).
#[wasm_bindgen]
pub fn replica_selftest(scenario: &str, files: &Object) -> String {
    set_core_clock();
    let mut map: HashMap<String, Vec<u8>> = HashMap::new();
    for entry in Object::entries(files).iter() {
        let pair: Array = entry.unchecked_into();
        let (Some(name), bytes) = (pair.get(0).as_string(), pair.get(1)) else {
            continue;
        };
        map.insert(name, Uint8Array::new(&bytes).to_vec());
    }
    let memory = || {
        // Linear memory of the module in MiB: what the replica costs the tab.
        (core::arch::wasm32::memory_size(0) * 64 / 1024) as f64
    };
    match replica::selftest::run(&map, Box::new(WebClock::new()), Some(&memory)) {
        Ok(report) => {
            let json = report.to_json(scenario);
            let missed = report.missed.len();
            if missed > 0 {
                return format!(
                    "{{\"ok\":false,\"error\":\"the core asked for files the data lacks: {:?}\"}}",
                    report.missed
                );
            }
            format!("{{\"ok\":true,{}", &json[1..])
        }
        Err(e) => format!("{{\"ok\":false,\"error\":{e:?}}}"),
    }
}

// ---------------------------------------------------------------- the cost of the reads (`ui2-getters`)

/// Calls timed per method (the id-taking methods cycle through the seat's units or cities).
const SAMPLES: usize = 200;

fn str_of<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("script: no {key}"))
}

fn u64_of(v: &Value, key: &str) -> Result<u64, String> {
    v.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("script: no {key}"))
}

fn file_of<'a>(files: &'a HashMap<String, Vec<u8>>, name: &str) -> Result<&'a Vec<u8>, String> {
    files
        .get(name)
        .ok_or_else(|| format!("fixture: no file {name}"))
}

/// A replica of the seat that joined the first snapshot of the script (the same steps as
/// `replica::selftest::run`, without the rest of the script).
fn joined(files: &HashMap<String, Vec<u8>>, seat: Option<u8>) -> Result<Replica, String> {
    let script: Value = serde_json::from_slice(file_of(files, "script.json")?)
        .map_err(|e| format!("script.json: {e}"))?;
    let index = file_of(files, "gamedata/vfs-index.json")?.clone();
    let parsed: Value = serde_json::from_slice(&index).map_err(|e| e.to_string())?;
    let mut g = GameFiles::from_index(index).map_err(|e| e.to_string())?;
    let mut put = |id: &str, path: &str| {
        if let Some(b) = files.get(&format!("gamedata/{path}")) {
            g.put(id, b.clone());
        }
    };
    put(str_of(&parsed["db"], "gameplay")?, "db/gameplay.db");
    put(str_of(&parsed["db"], "localization")?, "db/loc-en_us.db");
    for f in parsed["files"].as_array().ok_or("index: no files")? {
        put(str_of(f, "id")?, str_of(f, "path")?);
    }
    let seat =
        seat.unwrap_or_else(|| script.get("seat").and_then(Value::as_u64).unwrap_or(0) as u8);
    let mut r = Replica::new(GameDataHost::new(Arc::new(Mutex::new(g))), seat)
        .map_err(|e| e.to_string())?;
    r.config_mut().pack_base = false;
    let base = script["steps"]
        .as_array()
        .ok_or("script: no steps")?
        .iter()
        .find(|s| s["op"] == "base")
        .ok_or("script: no base step")?;
    let body = file_of(files, str_of(base, "body")?)?.clone();
    let blob = GcBlobRef {
        hash: xxh64(0, &body),
        size: body.len() as u32,
    };
    r.on_msg(ReplicaMsg::Base(GcBase {
        epoch: u64_of(base, "epoch")? as u32,
        turn: u64_of(base, "turn")? as i32,
        jseq: u64_of(base, "jseq")?,
        raw_hash: u64::from_str_radix(str_of(base, "raw_hash")?, 16).map_err(|e| e.to_string())?,
        snapshot: blob,
    }));
    r.blob(blob.hash, body);
    if r.pump(0) != ReplicaStep::Working {
        return Err("the unpack stage did not run".into());
    }
    if !matches!(r.pump(0), ReplicaStep::Loaded { .. }) {
        return Err("the load stage did not run".into());
    }
    Ok(r)
}

/// Times `calls` calls of `call(i)` (its result is the item count of the answer); the row as JSON, times in µs.
fn measure(
    clock: &WebClock,
    name: &str,
    calls: usize,
    mut call: impl FnMut(usize) -> usize,
) -> Value {
    let mut us = Vec::with_capacity(calls);
    let mut items = 0;
    for i in 0..calls {
        let t = clock.now_ms_f64();
        items = call(i);
        us.push((clock.now_ms_f64() - t) * 1000.0);
    }
    let first = us.first().copied().unwrap_or(0.0);
    let mean = us.iter().sum::<f64>() / us.len().max(1) as f64;
    us.sort_by(f64::total_cmp);
    json!({
        "method": name,
        "calls": us.len(),
        "first_us": first,
        "median_us": us.get(us.len() / 2).copied().unwrap_or(0.0),
        "max_us": us.last().copied().unwrap_or(0.0),
        "mean_us": mean,
        "items": items,
    })
}

fn getters_table(clock: &WebClock, g: &mut dyn GameRead) -> (Vec<Value>, Value) {
    let seat = g.seat() as i8;
    let mut rows = Vec::new();
    rows.push(measure(clock, "game_info", SAMPLES, |_| {
        g.game_info().map_or(0, |_| 1)
    }));
    rows.push(measure(clock, "top_bar", SAMPLES, |_| {
        g.top_bar().map_or(0, |_| 1)
    }));
    rows.push(measure(clock, "players", SAMPLES, |_| {
        g.players().map_or(0, |v| v.len())
    }));
    rows.push(measure(clock, "notifications", SAMPLES, |_| {
        g.notifications().map_or(0, |v| v.len())
    }));
    rows.push(measure(clock, "techs", SAMPLES, |_| {
        g.techs().map_or(0, |v| v.len())
    }));
    rows.push(measure(clock, "player_resources", SAMPLES, |_| {
        g.player_resources().map_or(0, |v| v.len())
    }));
    rows.push(measure(clock, "policy_branches", SAMPLES, |_| {
        g.policy_branches().map_or(0, |v| v.len())
    }));
    let units = g.units().unwrap_or_default();
    rows.push(measure(clock, "units", SAMPLES, |_| {
        g.units().map_or(0, |v| v.len())
    }));
    let cities = g.cities().unwrap_or_default();
    rows.push(measure(clock, "cities", SAMPLES, |_| {
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
    if !own_units.is_empty() {
        let n = SAMPLES.min(own_units.len().max(SAMPLES / 4));
        rows.push(measure(clock, "unit_actions", n, |i| {
            g.unit_actions(own_units[i % own_units.len()])
                .map_or(0, |v| v.len())
        }));
        rows.push(measure(clock, "unit_promotions", n, |i| {
            g.unit_promotions(own_units[i % own_units.len()])
                .map_or(0, |v| v.len())
        }));
    }
    if !own_cities.is_empty() {
        let n = SAMPLES.min(own_cities.len().max(SAMPLES / 4));
        rows.push(measure(clock, "city_builds", n, |i| {
            g.city_builds(own_cities[i % own_cities.len()])
                .map_or(0, |v| v.len())
        }));
        rows.push(measure(clock, "city_queue", n, |i| {
            g.city_queue(own_cities[i % own_cities.len()])
                .map_or(0, |v| v.len())
        }));
    }
    for t in CatalogTable::ALL.into_iter().chain([
        CatalogTable::ArtTechs,
        CatalogTable::GameSpeeds,
        CatalogTable::PolicyBranches,
    ]) {
        rows.push(measure(
            clock,
            &format!("catalog({t:?})"),
            SAMPLES / 4,
            |_| g.catalog(t).map_or(0, |v| v.len()),
        ));
    }
    let counts = json!({
        "own_units": own_units.len(), "own_cities": own_cities.len(),
        "units": units.len(), "cities": cities.len(),
    });
    (rows, counts)
}

/// The cost of every `GameRead` method on the game of the fixture (`scenario`, `files` as for
/// [`replica_selftest`]; `seat` < 0: the script's seat): joins the first snapshot, times each read, returns
/// `{"ok":true,"scenario":..,"counts":{..},"rows":[{"method","calls","first_us","median_us","max_us","mean_us","items"}]}`.
#[wasm_bindgen]
pub fn replica_getters(scenario: &str, files: &Object, seat: i32) -> String {
    set_core_clock();
    let mut map: HashMap<String, Vec<u8>> = HashMap::new();
    for entry in Object::entries(files).iter() {
        let pair: Array = entry.unchecked_into();
        if let (Some(name), bytes) = (pair.get(0).as_string(), pair.get(1)) {
            map.insert(name, Uint8Array::new(&bytes).to_vec());
        }
    }
    let result = joined(&map, u8::try_from(seat).ok()).and_then(|mut r| {
        let clock = WebClock::new();
        let g = r.read().ok_or("the replica is not live after the join")?;
        Ok(getters_table(&clock, g))
    });
    match result {
        Ok((rows, counts)) => {
            json!({"ok": true, "scenario": scenario, "counts": counts, "rows": rows}).to_string()
        }
        Err(e) => json!({"ok": false, "error": e}).to_string(),
    }
}

// ---------------------------------------------------------------- the replica stays untouched (`ui2-r17`)

/// The steps of the native test `ui2-source/tests/r17_replica_untouched.rs`, the same file: the wasm core is a
/// build of its own, so the rule is checked on it too.
#[path = "../../../crates/client/ui2-source/tests/r17/scenario.rs"]
mod r17;

/// The replica-untouched test on the fixture (`scenario`, `files` as for [`replica_selftest`]; `seat` < 0: the
/// script's seat): the replica's digest, batch count and last batch stay the same over full rebuilds of the UI v2
/// model and every intent of the catalogue, and a control batch is seen. Returns
/// `{"ok":true,"scenario":..,<the report>}` or `{"ok":false,"error":..}`.
#[wasm_bindgen]
pub fn replica_r17(scenario: &str, files: &Object, seat: i32) -> String {
    set_core_clock();
    let mut map: HashMap<String, Vec<u8>> = HashMap::new();
    for entry in Object::entries(files).iter() {
        let pair: Array = entry.unchecked_into();
        if let (Some(name), bytes) = (pair.get(0).as_string(), pair.get(1)) {
            map.insert(name, Uint8Array::new(&bytes).to_vec());
        }
    }
    match r17::run(&map, u8::try_from(seat).ok()) {
        Ok(report) => {
            let mut body = report.to_json();
            body["ok"] = json!(true);
            body["scenario"] = json!(scenario);
            body.to_string()
        }
        Err(e) => json!({"ok": false, "scenario": scenario, "error": e}).to_string(),
    }
}
