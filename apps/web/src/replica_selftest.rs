//! `replica_load` of `cargo xtask web-selftest`: the scripted replica run of `replica::selftest` in the browser.
//! The page (`replica-selftest.html`) fetches the fixture files and hands them over; the report comes back as JSON.

use std::collections::HashMap;

use js_sys::{Array, Object, Uint8Array};
use platform_web::WebClock;
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
