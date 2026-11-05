//! The performance report: the document that the panel's "Send report" posts (the `perf` section of
//! `POST /client-report/v2`, next to the recent log lines the page adds) and that the desktop client
//! writes to a file. One snapshot of what the panel shows, plus what a person looking at a phone's
//! numbers needs to know about the phone.
//!
//! Shape (`v` 1; every number is plain JSON, a missing GPU time is `null`):
//!
//! | key | what |
//! |---|---|
//! | `v`, `at_ms`, `build`, `touch` | the shape, the app clock, the client build, a phone or tablet |
//! | `frame` | `fps`, `avg_ms`, `p50_ms`, `p99_ms`, `low1_fps`, `cpu_ms`, `gpu_ms`, `verdict` (`cpu`/`gpu`/`vsync`/null), `frames` |
//! | `graph_ms` | the frame periods of the last ~10 s, oldest first (at most 300, the worst of each bucket) |
//! | `passes` | one object per pass: `name`, `gpu_ms`, `cpu_ms`, `draws`, `tris`, `inst`, `pipe`, `bind` |
//! | `main_groups` | `Main` by draw group, the largest in triangles first: `name`, `draws`, `tris`, `inst`, `drawn` |
//! | `groups_off` | the mask of the groups the operator switched off (0: all drawn) and `groups_off_names` |
//! | `counters` | every counter the panel shows, nested by group: `counters.gpu_mem.total`, `counters.ram.heap_live`, `counters.terrain.*`, ... |
//! | `slow_frames` | the recent frames over 33 ms and where their time went |
//! | `frame_trace` | the last 48 frames, slow or not, oldest first: `seq`, `period_ms`, `work_ms`, `acquire_ms` (waiting for the swapchain), `submit_ms`, `present_ms`, `terrain_ms`; with `counters.frame.stall_every` / `stall_ms` it shows a long frame that comes back at the same distance and what that frame waited on |
//! | `graphics` | `render_scale_pct`, `shadow_quality` (index and `shadow_quality_name`), `shadow_map_side`, `shadow_raw`, `scene`, `screen`, `terrain_detail` (index and `terrain_detail_name`), `terrain_max_page`, `terrain_page_budget_bytes`, `terrain_lattice_step`, `preset`, `settings` (every key of the ini) |
//! | `device` | the adapter (`adapter`, `vendor`, `device_id`, `device_type`, `backend`, `driver`), `features`, `features_all`, `limits`, `msaa_samples` |
//! | `assets` | `texture_variants`: the variant profile in use, most preferred first |
//! | `game` | `game`, `player`, `turn`, `map` (`[w, h]`), all null outside a game |
//!
//! The page adds `page` (user agent, `devicePixelRatio`, screen and canvas sizes, cores, memory) to the
//! object before it posts it (`clientlog.js`, `sendPerfReport`): the wasm side cannot see most of it.

use render::perf::PerfSnapshot;
use serde_json::{Map, Value, json};

/// The version of the shape above.
pub const VERSION: u32 = 1;

/// Where in a game the report was made.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameInfo {
    pub game: Option<u32>,
    pub player: Option<i64>,
    pub turn: Option<i64>,
    pub map: Option<(u32, u32)>,
}

/// What the report needs besides the snapshot.
#[derive(Debug, Clone, Default)]
pub struct ReportContext {
    pub at_ms: u64,
    pub build: String,
    pub touch: bool,
    pub device: render::DeviceReport,
    /// The name of the preset the keys equal, or `custom`.
    pub preset: String,
    /// `(key, value)` of every key of the graphics ini.
    pub settings: Vec<(&'static str, i64)>,
    pub game: GameInfo,
}

fn num(v: f64) -> Value {
    // JSON has no NaN or infinity: they become `null`.
    json!(v)
}

fn opt(v: Option<f32>) -> Value {
    v.map_or(Value::Null, |v| json!(v))
}

/// The nested form of the counters: `counters.<group>.<name>`; names without a dot go to `""`.
fn counters(snap: &PerfSnapshot) -> Value {
    let mut out = Map::new();
    for (group, rows) in snap.groups() {
        let mut m = Map::new();
        for (name, v) in rows {
            m.insert(name.to_owned(), num(v));
        }
        out.insert(group, Value::Object(m));
    }
    Value::Object(out)
}

/// The report of one frame's snapshot.
pub fn report_json(snap: &PerfSnapshot, cx: &ReportContext) -> Value {
    let f = &snap.frame;
    let c = |name: &str| snap.counter(name);
    let pair = |a: &str, b: &str| match (c(a), c(b)) {
        (Some(w), Some(h)) => json!([w, h]),
        _ => Value::Null,
    };
    let off_names: Vec<&str> = snap
        .main_groups
        .iter()
        .filter(|r| snap.groups_off & r.bit != 0)
        .map(|r| r.name)
        .collect();
    let msaa_name = match c("render.msaa") {
        Some(n) if n > 1.0 => format!("{n:.0}x"),
        Some(_) => "Off".to_owned(),
        None => "?".to_owned(),
    };
    let gfx_preset = c("render.gfx_preset").map_or("?", |i| {
        render::gfx_choice::GfxPreset::name_of_index(i as u32)
    });
    let quality = c("render.shadow_quality").unwrap_or(-1.0);
    let quality_name = u32::try_from(quality as i64)
        .ok()
        .and_then(render::shadow::ShadowQuality::from_index)
        .map_or("?", render::shadow::ShadowQuality::name);
    let terrain_detail_name = c("render.terrain_detail")
        .and_then(|d| u32::try_from(d as i64).ok())
        .and_then(render::terrain_detail::TerrainDetail::from_index)
        .map_or("?", render::terrain_detail::TerrainDetail::name);
    let settings: Map<String, Value> = cx
        .settings
        .iter()
        .map(|(k, v)| ((*k).to_owned(), json!(v)))
        .collect();
    let d = &cx.device;
    json!({
        "v": VERSION,
        "at_ms": cx.at_ms,
        "build": cx.build,
        "touch": cx.touch,
        "frame": {
            "frames": f.frames,
            "fps": f.fps,
            "avg_ms": if f.fps > 0.0 { json!(1000.0 / f.fps) } else { Value::Null },
            "p50_ms": f.dt_p50_ms,
            "p99_ms": f.dt_p99_ms,
            "low1_fps": f.low1_fps,
            "cpu_ms": f.cpu_ms,
            "gpu_ms": opt(f.gpu_ms),
            "verdict": f.verdict.map(|v| v.name()),
        },
        "graph_ms": snap.graph,
        "passes": snap.passes.iter().map(|p| json!({
            "name": p.name,
            "gpu_ms": opt(p.gpu_ms),
            "cpu_ms": p.cpu_ms,
            "draws": p.draw_calls,
            "tris": p.triangles,
            "inst": p.instances,
            "pipe": p.pipeline_sets,
            "bind": p.bind_group_sets,
        })).collect::<Vec<_>>(),
        "main_groups": snap.main_groups.iter().map(|r| json!({
            "name": r.name,
            "draws": r.draw_calls,
            "tris": r.triangles,
            "inst": r.instances,
            "drawn": snap.groups_off & r.bit == 0,
        })).collect::<Vec<_>>(),
        "groups_off": snap.groups_off,
        "groups_off_names": off_names,
        "counters": counters(snap),
        "slow_frames": snap.slow_frames.iter().map(|s| json!({
            "seq": s.seq,
            "ms": s.ms,
            "turn": s.turn,
            "scopes": s.scopes.iter().map(|(n, ms)| json!([n, ms])).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "frame_trace": snap.frame_trace.iter().map(|r| json!({
            "seq": r.seq,
            "period_ms": r.period_ms,
            "work_ms": r.work_ms,
            "acquire_ms": r.acquire_ms,
            "submit_ms": r.submit_ms,
            "present_ms": r.present_ms,
            "terrain_ms": r.terrain_ms,
        })).collect::<Vec<_>>(),
        "graphics": {
            "render_scale_pct": c("render.scale_pct"),
            "shadow_quality": quality as i64,
            "shadow_quality_name": quality_name,
            "shadow_map_side": c("render.shadow_map_side"),
            "shadow_raw": pair("render.shadow_raw_w", "render.shadow_raw_h"),
            "scene": pair("render.scene_w", "render.scene_h"),
            "screen": pair("render.screen_w", "render.screen_h"),
            "msaa": c("render.msaa"),
            "msaa_name": msaa_name,
            "gfx_preset": gfx_preset,
            "terrain_detail": c("render.terrain_detail"),
            "terrain_detail_name": terrain_detail_name,
            "terrain_max_page": c("terrain.max_page"),
            "terrain_page_budget_bytes": c("terrain.page_budget_bytes"),
            "terrain_lattice_step": c("terrain.lattice_step"),
            "preset": cx.preset,
            "settings": settings,
        },
        "device": {
            "adapter": d.adapter,
            "vendor": d.vendor,
            "device_id": d.device_id,
            "device_type": d.device_type,
            "backend": d.backend,
            "driver": d.driver,
            "driver_info": d.driver_info,
            "features": d.features,
            "features_all": d.features_all,
            "limits": d.limits.iter().map(|(k, v)| ((*k).to_owned(), json!(v))).collect::<Map<_, _>>(),
            "msaa_samples": d.msaa_samples,
        },
        "assets": {"texture_variants": d.texture_variants},
        "game": {
            "game": cx.game.game,
            "player": cx.game.player,
            "turn": cx.game.turn,
            "map": cx.game.map.map(|(w, h)| json!([w, h])),
        },
    })
}

/// The file name of a report written on the desktop: `perf-report-<ms>.json`.
pub fn file_name(at_ms: u64) -> String {
    format!("perf-report-{at_ms}.json")
}

/// The folder the desktop client writes its reports to: `$CIV5_LOG_DIR`, else `logs` in the working
/// directory (the desktop's log goes to the console; this is where its files go).
#[cfg(not(target_arch = "wasm32"))]
pub fn log_dir() -> std::path::PathBuf {
    std::env::var_os("CIV5_LOG_DIR")
        .filter(|d| !d.is_empty())
        .map_or_else(
            || std::path::PathBuf::from("logs"),
            std::path::PathBuf::from,
        )
}

/// Writes `json` to `dir/perf-report-<at_ms>.json` and returns the path.
#[cfg(not(target_arch = "wasm32"))]
pub fn write_file(
    dir: &std::path::Path,
    at_ms: u64,
    json: &str,
) -> std::io::Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(file_name(at_ms));
    std::fs::write(&path, json)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use render::perf::{FrameSummary, GroupRow, PassRow, Verdict};

    fn snapshot() -> PerfSnapshot {
        PerfSnapshot {
            frame: FrameSummary {
                frames: 600,
                fps: 40.0,
                dt_p50_ms: 25.0,
                dt_p99_ms: 41.0,
                low1_fps: 24.4,
                cpu_ms: 6.0,
                gpu_ms: Some(31.0),
                verdict: Some(Verdict::Gpu),
            },
            passes: vec![
                PassRow {
                    name: "Main",
                    gpu_ms: Some(19.3),
                    cpu_ms: 3.1,
                    draw_calls: 430,
                    triangles: 2_960_819,
                    instances: 23_251,
                    pipeline_sets: 90,
                    bind_group_sets: 700,
                },
                PassRow {
                    name: "ShadowMap",
                    gpu_ms: Some(9.37),
                    ..Default::default()
                },
            ],
            main_groups: vec![
                GroupRow {
                    name: "terrain",
                    bit: 1,
                    draw_calls: 100,
                    triangles: 2_000_000,
                    instances: 0,
                },
                GroupRow {
                    name: "units",
                    bit: 8,
                    draw_calls: 30,
                    triangles: 900_000,
                    instances: 23_000,
                },
            ],
            groups_off: 8,
            graph: vec![16.0, 17.0, 40.0],
            frame_trace: [(20.0, 2.0), (20.0, 2.0), (50.0, 40.0)]
                .iter()
                .enumerate()
                .map(|(i, &(work, acquire))| render::perf::slow::TraceRow {
                    seq: i as u64 + 1,
                    period_ms: 20.0,
                    work_ms: work,
                    acquire_ms: acquire,
                    ..Default::default()
                })
                .collect(),
            counters: vec![
                ("gpu_mem.total".into(), 1.5e9),
                ("gpu_mem.textures".into(), 1.0e9),
                ("ram.heap_live".into(), 2.0e8),
                ("terrain.holes".into(), 3.0),
                ("render.scale_pct".into(), 75.0),
                ("render.scene_w".into(), 1800.0),
                ("render.scene_h".into(), 810.0),
                ("render.screen_w".into(), 2400.0),
                ("render.screen_h".into(), 1080.0),
                ("render.shadow_quality".into(), 2.0),
                ("render.terrain_detail".into(), 1.0),
                ("terrain.max_page".into(), 256.0),
                ("terrain.page_budget_bytes".into(), 402_653_184.0),
                ("terrain.lattice_step".into(), 2.0),
                ("render.shadow_map_side".into(), 1024.0),
                ("render.shadow_raw_w".into(), 900.0),
                ("render.shadow_raw_h".into(), 405.0),
                ("render.msaa".into(), 1.0),
                ("render.gfx_preset".into(), 1.0),
                ("weird.nan".into(), f64::NAN),
            ],
            ..Default::default()
        }
    }

    fn context() -> ReportContext {
        ReportContext {
            at_ms: 123_456,
            build: "0.1.0".into(),
            touch: true,
            device: render::DeviceReport {
                adapter: "Mali-G78".into(),
                backend: "BrowserWebGpu".into(),
                features: vec!["TEXTURE_COMPRESSION_ASTC", "TIMESTAMP_QUERY"],
                limits: vec![("max_texture_dimension_2d", 8192)],
                texture_variants: vec!["Astc".into(), "Uncompressed".into()],
                msaa_samples: 4,
                ..Default::default()
            },
            preset: "medium".into(),
            settings: vec![("ShadowLevel", 2), ("MSAASamples", 4)],
            game: GameInfo {
                game: Some(936),
                player: Some(1),
                turn: Some(126),
                map: Some((128, 80)),
            },
        }
    }

    #[test]
    fn the_report_has_every_section_of_the_documented_shape() {
        let j = report_json(&snapshot(), &context());
        assert_eq!(j["v"], VERSION);
        assert_eq!(j["touch"], true);
        // The frame summary: the average, the 1% low, the verdict.
        assert_eq!(j["frame"]["avg_ms"], 25.0);
        assert_eq!(j["frame"]["low1_fps"].as_f64().unwrap().round(), 24.0);
        assert_eq!(j["frame"]["verdict"], "gpu");
        assert_eq!(j["frame"]["gpu_ms"], 31.0);
        assert_eq!(j["graph_ms"].as_array().unwrap().len(), 3);
        // The pass table, all columns; a pass the device did not time has a null GPU time.
        let main = &j["passes"][0];
        assert_eq!(main["name"], "Main");
        assert_eq!(
            (
                main["draws"].as_u64(),
                main["tris"].as_u64(),
                main["inst"].as_u64()
            ),
            (Some(430), Some(2_960_819), Some(23_251))
        );
        assert_eq!(
            (main["pipe"].as_u64(), main["bind"].as_u64()),
            (Some(90), Some(700))
        );
        assert_eq!(main["gpu_ms"].as_f64().unwrap() as f32, 19.3);
        assert_eq!(j["passes"][1]["cpu_ms"], 0.0);
        // Main by group, in the order given (largest first), with what is switched off.
        assert_eq!(j["main_groups"][0]["name"], "terrain");
        assert_eq!(j["main_groups"][0]["drawn"], true);
        assert_eq!(j["main_groups"][1]["drawn"], false);
        assert_eq!(j["groups_off"], 8);
        assert_eq!(j["groups_off_names"], json!(["units"]));
        // Every counter of the panel, by group; memory is in `gpu_mem` and `ram`.
        assert_eq!(j["counters"]["gpu_mem"]["total"], 1.5e9);
        assert_eq!(j["counters"]["gpu_mem"]["textures"], 1.0e9);
        assert_eq!(j["counters"]["ram"]["heap_live"], 2.0e8);
        assert_eq!(j["counters"]["terrain"]["holes"], 3.0);
        assert!(j["counters"]["weird"]["nan"].is_null(), "JSON has no NaN");
        // Graphics settings.
        let g = &j["graphics"];
        assert_eq!(g["render_scale_pct"], 75.0);
        assert_eq!(g["shadow_quality"], 2);
        assert_eq!(g["shadow_quality_name"], "Medium");
        // The trace of the last frames: one row each, with the waits of the render loop.
        assert_eq!(j["frame_trace"].as_array().unwrap().len(), 3);
        assert_eq!(j["frame_trace"][2]["acquire_ms"], 40.0);
        assert_eq!(j["frame_trace"][2]["period_ms"], 20.0);
        assert_eq!(g["terrain_detail"], 1.0);
        assert_eq!(g["terrain_detail_name"], "Medium");
        assert_eq!(g["terrain_max_page"], 256.0);
        assert_eq!(g["terrain_page_budget_bytes"], 402_653_184.0);
        assert_eq!(g["terrain_lattice_step"], 2.0);
        assert_eq!(g["shadow_map_side"], 1024.0);
        assert_eq!(g["shadow_raw"], json!([900.0, 405.0]));
        assert_eq!(g["scene"], json!([1800.0, 810.0]));
        assert_eq!(g["screen"], json!([2400.0, 1080.0]));
        assert_eq!(g["preset"], "medium");
        assert_eq!(g["msaa"], 1.0);
        assert_eq!(g["msaa_name"], "Off");
        assert_eq!(g["gfx_preset"], "Low");
        assert_eq!(g["settings"]["ShadowLevel"], 2);
        // The device, the variant profile, the game.
        let d = &j["device"];
        assert_eq!(d["adapter"], "Mali-G78");
        assert_eq!(
            d["features"],
            json!(["TEXTURE_COMPRESSION_ASTC", "TIMESTAMP_QUERY"])
        );
        assert_eq!(d["limits"]["max_texture_dimension_2d"], 8192);
        assert_eq!(
            j["assets"]["texture_variants"],
            json!(["Astc", "Uncompressed"])
        );
        assert_eq!(j["game"]["turn"], 126);
        assert_eq!(j["game"]["map"], json!([128, 80]));
    }

    #[test]
    fn an_empty_snapshot_and_a_session_outside_a_game_still_make_a_report() {
        let j = report_json(&PerfSnapshot::default(), &ReportContext::default());
        assert_eq!(j["v"], VERSION);
        assert!(j["frame"]["avg_ms"].is_null(), "no frames: no average");
        assert!(j["frame"]["verdict"].is_null() && j["frame"]["gpu_ms"].is_null());
        assert_eq!(j["passes"], json!([]));
        assert_eq!(j["main_groups"], json!([]));
        assert!(j["game"]["game"].is_null() && j["game"]["map"].is_null());
        assert_eq!(j["graphics"]["shadow_quality_name"], "?");
        assert_eq!(j["graphics"]["msaa_name"], "?");
        assert_eq!(j["graphics"]["gfx_preset"], "?");
        assert!(j["graphics"]["scene"].is_null());
    }

    #[test]
    fn a_report_is_small_enough_for_the_server() {
        // 150 counters of 12 groups, 14 passes, a full graph: a real snapshot is of this size.
        let mut s = snapshot();
        s.counters.extend((0..150).map(|i| {
            (
                format!("g{}.counter_with_a_long_name_{i}", i % 12),
                f64::from(i) * 1.5,
            )
        }));
        s.graph = vec![16.6; 300];
        s.passes = (0..14).map(|_| s.passes[0].clone()).collect();
        let text = report_json(&s, &context()).to_string();
        assert!(text.len() < 64 * 1024, "{} bytes", text.len());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_desktop_writes_the_same_json_to_a_file() {
        let dir = std::env::temp_dir().join(format!("civ5-perf-report-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let text = report_json(&snapshot(), &context()).to_string();
        let path = write_file(&dir.join("logs"), 42, &text).unwrap();
        assert_eq!(path.file_name().unwrap(), "perf-report-42.json");
        let back: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back["passes"][0]["name"], "Main");
        assert_eq!(back, report_json(&snapshot(), &context()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
