//! wasm entry (`app-web`): reads `?nick=`, `&gfx=` (`webgl2` and/or `Key:value` graphics keys), `&preset=low|medium|high`, `&disable=FOW,FluidFOW`, `&debug=shadow`, `&manifest=<hash>`, `&demo=skinned`, `&autostart=1` (skip the lobby: after the login the quick start script runs; `&size=`, `&ai=`, `&civ=`, `&seed=`, `&mode=hybrid|simultaneous` say what game), `&ui=interim` (the interim egui game panels instead of the original UI, which is the default), `&fpcam=1` (the debug first-person camera on F4), `&test=1` (test hooks, `testhooks.rs`), `&golden=<url of a scene folder>&capture=<name>` (one frame of a `golden` scene, with `&test=1`), dev-only `&server=`.
//! Start-up timings (and the time to map, AC-1.3) go to `window.__civ5stats.timing` / `.timeToMapMs`.

#![cfg(target_arch = "wasm32")]
// The test hooks' JSON (`testhooks.rs`) is a deep `json!`.
#![recursion_limit = "256"]

mod testhooks;

#[cfg(feature = "replica-selftest")]
mod replica_selftest;

use client_app::render::{BackendPref, wgpu};
use client_app::winit::platform::web::WindowAttributesExtWebSys;
use client_app::{AppConfig, Services};
use platform_web::page;
use wasm_bindgen::prelude::*;

/// Counts the heap by owner for the performance panel (ram.*).
#[global_allocator]
static ALLOC: client_app::render::perf::CountingAlloc = client_app::render::perf::CountingAlloc;

const CANVAS_ID: &str = "civ5-canvas";
const DEFAULT_NICK: &str = "guest";

/// The threaded build (B3 render-mt): `bootstrap.js` starts web workers on the module and its shared
/// memory ([`jobs_module`], [`jobs_memory`]); each instantiates them and enters [`jobs_worker_main`].
#[cfg(target_feature = "atomics")]
mod threads {
    use wasm_bindgen::prelude::*;

    /// The compiled module, for the workers.
    #[wasm_bindgen]
    pub fn jobs_module() -> JsValue {
        wasm_bindgen::module()
    }

    /// The shared memory, for the workers.
    #[wasm_bindgen]
    pub fn jobs_memory() -> JsValue {
        wasm_bindgen::memory()
    }

    /// The loop of a worker of the pool: never returns (the worker's event loop stays blocked).
    #[wasm_bindgen]
    pub fn jobs_worker_main() {
        jobs::worker_main()
    }

    /// Workers that joined the pool so far.
    #[wasm_bindgen]
    pub fn jobs_workers() -> usize {
        jobs::workers()
    }
}

/// Whether this module was built with threads (atomics and a shared memory).
#[wasm_bindgen]
pub fn threaded() -> bool {
    cfg!(target_feature = "atomics")
}

/// The module start. A generated dist package (`cargo xtask mods build --target client`, feature `external-start`)
/// has its own start function that calls [`start_with`] with the hooks of its mods.
#[cfg_attr(not(feature = "external-start"), wasm_bindgen(start))]
pub fn start() {
    start_with(client_app::mod_hooks::ModHooks::new());
}

/// [`start`] with the hooks of the client halves of mods (an empty set is the public client).
pub fn start_with(mut hooks: client_app::mod_hooks::ModHooks) {
    // In the threaded build a web worker instantiates the module too: the app starts on the page only.
    #[cfg(target_feature = "atomics")]
    if web_sys::window().is_none() {
        return;
    }
    platform_web::init_logging(tracing::Level::INFO);
    note_worker_panics();
    // The core's clocks must be set before it boots (the C runtime reads 1970 otherwise).
    {
        let perf_ns = web_sys::window()
            .and_then(|w| w.performance())
            .map_or(0.0, |p| p.now() * 1e6);
        replica::selftest::set_clock((js_sys::Date::now() * 1e6) as u64, perf_ns as u64);
    }
    // A lost GPU device would leave a black canvas: show what happened over it.
    client_app::render::set_device_lost_hook(|message| {
        platform_web::show_crash("The graphics device was lost.", message);
    });
    let nick = page::query_param("nick")
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| DEFAULT_NICK.to_owned());
    // `?gfx=webgl2` picks the backend; `?gfx=ShadowLevel:0,FOWLevel:1` sets graphics keys (both can
    // be in one list); `?preset=low|medium|high`; `?disable=FOW,FluidFOW` switches layers off.
    let gfx = page::query_param("gfx")
        .map(|v| client_app::settings_store::split_gfx(&v))
        .unwrap_or(client_app::settings_store::GfxParam {
            backend: BackendPref::Auto,
            overrides: None,
            unknown: Vec::new(),
            backend_given: false,
        });
    for word in &gfx.unknown {
        tracing::warn!("?gfx={word}: neither a backend (webgl2) nor Key:value, ignored");
    }
    // WebGL2 on Firefox unless the URL names a backend (`?gfx=webgpu`): its WebGPU runs the
    // browser's GPU process out of memory and the whole browser freezes (`backend_for`).
    let backend = client_app::settings_store::backend_for(&gfx, &page::user_agent());
    if backend != gfx.backend {
        tracing::info!(
            "Firefox: WebGL2 (its WebGPU runs out of GPU memory; ?gfx=webgpu to try it)"
        );
    }
    let graphics = client_app::settings_store::GraphicsOptions {
        preset: page::query_param("preset").filter(|p| !p.trim().is_empty()),
        overrides: gfx.overrides,
        disable: page::query_param("disable").filter(|d| !d.trim().is_empty()),
        // `?debug=shadow`: the shadow map in place of the picture.
        debug: page::query_param("debug").filter(|d| !d.trim().is_empty()),
    };
    let server_url = page::query_param("server")
        .or_else(|| page::ws_url_from_location("/ws"))
        .unwrap_or_else(|| "ws://127.0.0.1:8080/ws".to_owned());
    tracing::info!(%nick, ?backend, %server_url, "app-web {}", env!("CARGO_PKG_VERSION"));

    let Some(canvas) = page::canvas_by_id(CANVAS_ID) else {
        tracing::error!("page has no <canvas id=\"{CANVAS_ID}\">");
        return;
    };

    let input_canvas = canvas.clone();
    let surface_provider: client_app::SurfaceProvider = Box::new(move |_window, attempt| {
        if attempt == 0 {
            Some(wgpu::SurfaceTarget::Canvas(input_canvas.clone()))
        } else {
            // WebGPU took the canvas: fall back to WebGL2 on a fresh one.
            page::create_canvas_behind(&input_canvas).map(wgpu::SurfaceTarget::Canvas)
        }
    });
    let window_canvas = canvas;
    let stats = page::query_param("stats").as_deref() == Some("1");
    install_page_probes();
    // The original UI (EUI) is the game screen (B4 AC-S4, WO-K); `&ui=interim` brings back the interim egui game
    // panels (B1/B2), `&ui=m1` is the M1 fixture. The lobby and the room are egui either way.
    let original_ui = page::query_param("ui").as_deref() != Some("interim");
    // A hidden tab reads its socket by a slowed timer: past `hidden_backlog_bytes` the socket closes
    // itself and the client returns with `resume` (4.8).
    let transport: Box<dyn client_app::Transport> =
        Box::new(platform_web::WebSocketTransport::with_backlog(8 << 20));
    let services = Services {
        transport,
        http: Box::new(platform_web::FetchHttp),
        clock: Box::new(platform_web::WebClock::new()),
        tasks: Box::new(platform_web::WebTaskPool::new()),
        spawner: Box::new(wasm_bindgen_futures::spawn_local),
        surface_provider: Some(surface_provider),
        window_attributes: Some(Box::new(move |attrs| {
            attrs
                .with_canvas(Some(window_canvas))
                .with_prevent_default(true)
                .with_focusable(true)
        })),
        stats_sink: stats.then(|| Box::new(publish_stats) as client_app::StatsSink),
        perf_sink: Some(Box::new(publish_perf)),
        report_sink: Some(Box::new(send_perf_report)),
        milestone_sink: Some(Box::new(publish_milestone)),
        reload: Some(Box::new(page::reload)),
        // `&test=1`: `window.__civ5test` for the playtest.
        test_port: testhooks::install(),
        // No `BlobStore` on the web yet (Cache API): settings come from the URL each time.
        settings_blobs: None,
        // The token of the session and the last nick (`localStorage`).
        kv_store: Some(Box::new(platform_web::LocalStorage::new())),
        // The graphics choices live in the same `localStorage`, under keys the account does not follow.
        device_store: None,
        // The network turns without frames too: the safety timer and the page events (NC-D9).
        net_driver: Some(Box::new(install_net_driver)),
        audio: web_audio(original_ui),
        // Sign-in, lobby and room are DOM (`ui.js`); `?egui=1` brings back the egui screens.
        ui_bridge: if page::query_param("egui").as_deref() == Some("1") {
            None
        } else {
            platform_web::WebUiBridge::detect().map(|b| Box::new(b) as Box<dyn platform::UiBridge>)
        },
        pointer_lock_probe: platform_web::WebPointerLockProbe::new()
            .map(|p| Box::new(p) as Box<dyn platform::PointerLockProbe>),
        group_stats_sink: Some(Box::new(|group, values| {
            let v: Vec<(&str, f64)> = values.iter().map(|(k, n)| (k.as_str(), *n)).collect();
            // `turn` nests its rows by turn (`__civ5stats.turn.turns[<turn>]`).
            if group == "turn" {
                platform_web::stats::set_turn_group(&v);
            } else {
                platform_web::stats::set_group(group, &v);
            }
        })),
    };
    let config = AppConfig {
        server_url,
        nick_given: page::query_param("nick").is_some_and(|n| !n.trim().is_empty()),
        page_url: page::href(),
        nick,
        backend,
        build: env!("CARGO_PKG_VERSION").to_owned(),
        manifest: page::query_param("manifest").filter(|m| !m.trim().is_empty()),
        showcase: page::query_param("showcase").as_deref() == Some("1"),
        demo_skinned: page::query_param("demo").as_deref() == Some("skinned"),
        // `&autostart=1`: no lobby, a game at once (time-to-map runs); `&size=`, `&ai=`, `&civ=`, `&seed=`,
        // `&mode=hybrid|simultaneous` say what game.
        autostart: page::query_param("autostart").as_deref() == Some("1"),
        room_params: room_params(original_ui),
        stats,
        // `&hud=1`: the performance panel from the start (F3 toggles it).
        hud: page::query_param("hud").as_deref() == Some("1"),
        graphics,
        // `&golden=<url of the scene folder>&capture=<name>` (with `&test=1`): one frame of a `golden` scene
        // instead of the game, delivered to `window.__civ5test.golden`.
        // The original UI (EUI) on the game screen, unless `&ui=interim` (the interim egui panels).
        original_ui,
        // &volume=0..100: the volume of the UI sounds (default 50, as the game's own default).
        ui_volume: page::query_param("volume")
            .and_then(|v| v.trim().parse::<f32>().ok())
            .map_or(0.5, |v| (v / 100.0).clamp(0.0, 1.0)),
        original_ui_m1: page::query_param("ui").as_deref() == Some("m1"),
        touch_device: client_app::gfx_prefs::is_touch_platform(
            &page::user_agent(),
            page::max_touch_points(),
        ),
        golden: page::query_param("golden")
            .filter(|u| !u.trim().is_empty())
            .zip(page::query_param("capture").filter(|c| !c.trim().is_empty()))
            .map(|(base_url, capture)| client_app::GoldenConfig { base_url, capture }),
    };
    client_app::set_m2_debug_lua(page::query_param("luadbg").filter(|c| !c.trim().is_empty()));
    // `&ui=v2` or `&ui=v2:<skin>`: a UI v2 skin (`ui2/skins.json` and its package, fetched relative to the page) over
    // the original UI, which keeps running but neither draws nor gets input. A skin that cannot load leaves the
    // original UI as it is. Any other `&ui=` fetches nothing.
    client_app::set_ui2_param(page::query_param("ui").as_deref());
    // `?trace=off|coarse|fine`: the turn timeline (remembered for the next visits).
    client_app::set_trace(
        page::remembered_param("trace", "civ5.trace").as_deref(),
        page::wall_offset_ms(),
    );
    client_app::set_wall_offset_source(page::wall_offset_ms);
    // `&fpcam=1` (or `&fpcam=x,z,yaw,pitch`, angles in degrees, to start in the view): the debug first-person camera
    // on F4, the web twin of the native `CIV5_FP_CAM` (see `client_app::fp_debug`).
    if let Some(v) = page::query_param("fpcam") {
        hooks.register(Box::new(
            client_app::fp_debug::FirstPersonDebug::from_value(&v),
        ));
    }
    if let Err(e) = client_app::run_with(config, services, hooks) {
        tracing::error!("client failed to start: {e}");
    }
}

/// The performance panel's "Send report": the page's report code (`clientlog.js`, `sendPerfReport`) adds what only the
/// page knows (user agent, pixel ratio, canvas), the recent log lines and the login, posts it to
/// `POST /client-report/v2` and answers `{ok, id, error}`.
fn send_perf_report(json: String, done: Box<dyn FnOnce(Result<String, String>)>) {
    wasm_bindgen_futures::spawn_local(async move {
        done(post_perf_report(&json).await);
    });
}

async fn post_perf_report(json: &str) -> Result<String, String> {
    use wasm_bindgen::JsCast;
    let no_code = || "the page has no report code".to_owned();
    let log =
        js_sys::Reflect::get(&js_sys::global(), &"__civ5log".into()).map_err(|_| no_code())?;
    let send = js_sys::Reflect::get(&log, &"sendPerfReport".into())
        .ok()
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
        .ok_or_else(no_code)?;
    let promise = send
        .call1(&log, &JsValue::from_str(json))
        .map_err(|e| format!("the report code failed: {e:?}"))?;
    let answer = wasm_bindgen_futures::JsFuture::from(js_sys::Promise::from(promise))
        .await
        .map_err(|e| format!("the report code failed: {e:?}"))?;
    let field = |name: &str| js_sys::Reflect::get(&answer, &name.into()).ok();
    if field("ok").and_then(|v| v.as_bool()) == Some(true) {
        field("id")
            .and_then(|v| v.as_string())
            .ok_or_else(|| "the server gave no report id".to_owned())
    } else {
        Err(field("error")
            .and_then(|v| v.as_string())
            .unwrap_or_else(|| "the report was not sent".to_owned()))
    }
}

/// The panic hook is one for the whole module, so the workers of the jobs pool (threaded build) run the
/// page's hook too; a panic there (no window: no overlay) is also kept for the page
/// (`jobs::note_worker_failure`), which names it when it finds the module terminated instead of spinning on
/// the dead worker (live report 2026-10-31).
fn note_worker_panics() {
    let page_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if web_sys::window().is_none() {
            let text = format!("panic in a jobs worker: {info}");
            // The worker's script puts it in the message of its trap (`jobs-worker.js`).
            if let Ok(f) = js_sys::Reflect::get(&js_sys::global(), &"__civ5OnPanic".into())
                && let Some(f) = f.dyn_ref::<js_sys::Function>()
            {
                let _ = f.call1(&JsValue::NULL, &text.as_str().into());
            }
            jobs::note_worker_failure(text);
        }
        page_hook(info);
    }));
}

/// The size of the wasm memory is logged as it passes these (it never shrinks): a tab that runs out of memory
/// traps without a panic, so the console says how big it was.
const MEMORY_MARKS_MB: [u64; 5] = [512, 1024, 2048, 3072, 3584];

/// Logs the wasm memory: at info when it passes a mark of [`MEMORY_MARKS_MB`], at debug once a minute; and
/// keeps it, the turn and the game in `window.__civ5stats.ctx*` for the page's crash report (`bootstrap.js`).
fn watch_memory(seconds: u64) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LOGGED_MB: AtomicU64 = AtomicU64::new(0);
    let mb = client_app::render::perf::ram::wasm_memory_bytes() >> 20;
    platform_web::stats::set("ctxWasmMB", mb as f64);
    let last = LOGGED_MB.load(Ordering::Relaxed);
    if let Some(mark) = MEMORY_MARKS_MB
        .iter()
        .rev()
        .find(|m| mb >= **m && last < **m)
    {
        LOGGED_MB.store(mb, Ordering::Relaxed);
        tracing::info!("wasm memory {mb} MB (past {mark} MB; it never shrinks)");
    } else if seconds.is_multiple_of(60) {
        tracing::debug!("wasm memory {mb} MB");
    }
}

/// What the quick start script is asked for by the page address (net-client 4.11).
fn room_params(original_ui: bool) -> client_app::autostart::RoomParams {
    let text = |name: &str| page::query_param(name).filter(|v| !v.trim().is_empty());
    client_app::autostart::RoomParams {
        size: text("size"),
        ai: text("ai")
            .and_then(|v| v.trim().parse::<u8>().ok())
            .map_or(1, |n| n.clamp(1, 20)),
        civ: text("civ"),
        seed: text("seed").and_then(|v| v.trim().parse().ok()),
        mode: match text("mode").as_deref() {
            Some("simultaneous") => client_app::test_port::TurnMode::Simultaneous,
            _ => client_app::test_port::TurnMode::Hybrid,
        },
        // The human makes the choices of the game (research, policy, religion ...) through the UI (the popups of
        // the original UI, which blocks END TURN until they are made); no `auto_choices`. The default of the
        // original UI; the interim UI has no screens for them. `&manual=1` / `&manual=0` force it.
        manual_choices: manual_choices(original_ui),
    }
}

/// `RoomParams::manual_choices` of the page: `&manual=1` / `&manual=0`, else on with the original UI.
pub(crate) fn manual_choices(original_ui: bool) -> bool {
    match page::query_param("manual").as_deref() {
        Some("1") => true,
        Some("0") => false,
        _ => original_ui,
    }
}

/// The drivers of the network that are not the frame (4.8): the safety timer, which turns the network when
/// no frame comes (a hidden tab), and the page events (`visibilitychange`, `online`, `offline`, `pagehide`).
/// What it returns keeps them alive.
fn install_net_driver(
    handle: client_app::net::NetHandle,
    clock: std::rc::Rc<dyn client_app::Clock>,
) -> Box<dyn std::any::Any> {
    // What is already true when the page starts (a tab opened in the background, no network).
    for ev in platform_web::host::initial_events() {
        handle.host_event(ev, clock.now_ms());
    }
    let events = {
        let (handle, clock) = (handle.clone(), std::rc::Rc::clone(&clock));
        platform_web::host::PageEvents::install(move |ev| handle.host_event(ev, clock.now_ms()))
    };
    if let Err(e) = &events {
        tracing::warn!("page events are not watched: {e}");
    }
    // `?test=1`: what the network knows is published to `window.__civ5stats` by the timer, since a hidden tab
    // draws no frames and the test hooks' snapshot is the frame's (the background tab spike, NC12).
    let publish = page::query_param("test").as_deref() == Some("1");
    let timer = {
        let (handle, clock) = (handle, clock);
        platform_web::host::IntervalTimer::start(
            platform_web::host::IntervalTimer::SAFETY_MS,
            move || {
                let _t = platform_web::stats::between("js.net_timer");
                let now = clock.now_ms();
                handle.pump_if_idle(now, 200);
                // The turn and the game for the page's crash report (`bootstrap.js`); -1 outside a game.
                if let Some((turn, game)) =
                    handle.peek(|c| (c.base().map(|v| v.turn), c.attached_game()))
                {
                    platform_web::stats::set("ctxTurn", turn.map_or(-1.0, f64::from));
                    platform_web::stats::set("ctxGame", game.map_or(-1.0, f64::from));
                }
                // More of the place of a report (`clientlog.js`): the seat, the stream position, the next order
                // number and the server's clock as the client estimates it.
                if let Some((player, seq, next_cmd, view_seq, server_ms)) = handle.peek(|c| {
                    (
                        c.attach_info().map(|a| a.player),
                        c.attach_info().map(|a| a.last_seq),
                        c.next_cmd_id(),
                        c.view_seq(),
                        c.server_ms(now),
                    )
                }) {
                    platform_web::stats::set("ctxPlayer", player.map_or(-1.0, f64::from));
                    platform_web::stats::set("ctxBatchSeq", seq.map_or(-1.0, |s| s as f64));
                    platform_web::stats::set("ctxNextCmd", f64::from(next_cmd));
                    platform_web::stats::set("ctxViewSeq", view_seq as f64);
                    platform_web::stats::set("ctxServerMs", server_ms as f64);
                }
                if publish {
                    publish_net(&handle, now);
                }
            },
        )
    };
    if let Err(e) = &timer {
        tracing::warn!("the safety timer of the network did not start: {e}");
    }
    Box::new((events.ok(), timer.ok()))
}

/// `window.__civ5stats.net*`: the time of this reading (the page clock, ms), the revision, `seq` and turn of
/// the view, the line (`0` Online, `1` Poor, `2` Lost, `3` Reconnecting, `4` Offline, `5` Replaced, `6` Closed),
/// the bytes taken so far and how many attaches the client has made.
fn publish_net(handle: &client_app::net::NetHandle, now: u64) {
    let read = handle.peek(|c| {
        (
            c.revision(),
            c.view_seq(),
            c.base().map(|v| v.turn),
            format!("{:?}", c.conn()),
            c.stats().bytes_in,
            c.stats().attached_fresh + c.stats().attached_replay,
        )
    });
    let Some((revision, view_seq, turn, conn, bytes_in, attaches)) = read else {
        return;
    };
    let line = [
        "Online",
        "Poor",
        "Lost",
        "Reconnecting",
        "Offline",
        "Replaced",
        "Closed",
    ]
    .iter()
    .position(|n| *n == conn)
    .map_or(-1.0, |i| i as f64);
    platform_web::stats::set("netAtMs", now as f64);
    platform_web::stats::set("netRevision", revision as f64);
    platform_web::stats::set("netViewSeq", view_seq as f64);
    platform_web::stats::set("netTurn", turn.map_or(-1.0, f64::from));
    platform_web::stats::set("netLine", line);
    platform_web::stats::set("netBytesIn", bytes_in as f64);
    platform_web::stats::set("netAttaches", attaches as f64);
}

/// `?stats=1`: sets `fps`, `draws`, `triangles`, `passes` (time of each pass in ms, AC-R8.6) and
/// `counters` (with the `ui.*` ones) of `window.__civ5stats` once per second.
fn publish_stats(fps: f32, frame: &client_app::render::FrameStats) {
    platform_web::stats::set("fps", f64::from(fps));
    platform_web::stats::set("draws", f64::from(frame.draws));
    platform_web::stats::set("triangles", f64::from(frame.triangles));
    let passes: Vec<(&str, f64)> = frame
        .passes
        .iter()
        .map(|p| {
            (
                client_app::render::pass_name(p.pass),
                f64::from(p.shown_ms()),
            )
        })
        .collect();
    platform_web::stats::set_passes(&passes);
    // The UI's counters and the times of its passes go in with the page's own (`ui.gpu_ms` is NaN, shown as
    // `null`, when the device gives no timestamps).
    for (name, n) in &frame.counters {
        if name.starts_with("ui.") {
            client_app::render::perf::set_counter(name, *n as f64);
        }
    }
    let t = &frame.ui_times;
    client_app::render::perf::set_counter("ui.cpu_ms", f64::from(t.cpu_ms));
    client_app::render::perf::set_counter("ui.gpu_ms", t.gpu_ms.map_or(f64::NAN, f64::from));
    client_app::render::perf::set_counter("ui.overlay_ms", f64::from(t.overlay_ms));
}

/// Samples what only the page knows (the JS heap, `performance.memory`, and the asset delivery's
/// `net.*`/`assets.*`/`storage.*`) into the counters once per second, so that the panel and
/// `window.__civ5stats.counters` show it next to the rest.
fn install_page_probes() {
    // What the page runs between frames goes to the gap rows of the slow-frame record.
    client_app::render::perf::slow::set_between_source(Box::new(platform_web::stats::take_between));
    fn tick() {
        let _t = platform_web::stats::between("js.page_probes");
        thread_local! {
            static SECONDS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
        }
        let seconds = SECONDS.with(|s| {
            s.set(s.get() + 1);
            s.get()
        });
        watch_memory(seconds);
        use client_app::render::perf::set_counter;
        if let Some((used, total, limit)) = platform_web::stats::js_heap() {
            set_counter("ram.js_heap_used", used);
            set_counter("ram.js_heap_total", total);
            set_counter("ram.js_heap_limit", limit);
        }
        let (verified, failed) = assets::verify_stats();
        set_counter("assets.verify_bytes", verified as f64);
        set_counter("assets.hash_fail", failed as f64);
        for (name, value) in platform_web::delivery::counters() {
            set_counter(name, value);
        }
    }
    tick();
    if let Ok(timer) = platform_web::host::IntervalTimer::start(1000, tick) {
        // Lives as long as the page.
        std::mem::forget(timer);
    }
}

/// `window.__civ5stats` from the performance snapshot (`perf-hud`, AC-H7), once per second while
/// `?stats=1` or the panel is on: `frame`, `passes_full`, `groups` (counters by prefix), `gpu_mem`,
/// `ram` and the flat `counters`. The panel and the desktop log line read the same snapshot.
fn publish_perf(s: &client_app::render::perf::PerfSnapshot) {
    use platform_web::stats;
    let f = &s.frame;
    stats::set_object(
        "frame",
        &[
            ("fps", f64::from(f.fps)),
            ("p50_ms", f64::from(f.dt_p50_ms)),
            ("p99_ms", f64::from(f.dt_p99_ms)),
            ("low1_fps", f64::from(f.low1_fps)),
            ("cpu_ms", f64::from(f.cpu_ms)),
            ("gpu_ms", f.gpu_ms.map_or(-1.0, f64::from)),
            ("frames", f64::from(f.frames)),
        ],
    );
    stats::set_str("verdict", f.verdict.map_or("unknown", |v| v.name()));
    let passes: Vec<(&str, Vec<(&str, f64)>)> = s
        .passes
        .iter()
        .map(|p| {
            (
                p.name,
                vec![
                    ("gpu_ms", p.gpu_ms.map_or(-1.0, f64::from)),
                    ("cpu_ms", f64::from(p.cpu_ms)),
                    ("draws", f64::from(p.draw_calls)),
                    ("triangles", p.triangles as f64),
                    ("instances", p.instances as f64),
                    ("pipelines", f64::from(p.pipeline_sets)),
                    ("bind_groups", f64::from(p.bind_group_sets)),
                ],
            )
        })
        .collect();
    stats::set_nested("passes_full", &passes);
    let flat: Vec<(&str, f64)> = s.counters.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    stats::set_counters(&flat);
    let groups = s.groups();
    let nested: Vec<(&str, Vec<(&str, f64)>)> = groups
        .iter()
        .map(|(g, rows)| (g.as_str(), rows.clone()))
        .collect();
    stats::set_nested("groups", &nested);
    let slow: Vec<stats::SlowRow<'_>> = s
        .slow_frames
        .iter()
        .map(|f| (f.seq as f64, f.ms, f.turn, f.scopes.clone()))
        .collect();
    stats::set_slow_frames(&slow);
    for (g, rows) in &nested {
        if *g == "gpu_mem" || *g == "ram" {
            stats::set_object(g, rows);
        }
    }
}
/// Start-up milestones (ms since the page started): `window.__civ5stats.timing[name]`; the map on
/// screen also sets `window.__civ5stats.timeToMapMs` (AC-1.3).
fn publish_milestone(name: &'static str, ms: f64) {
    platform_web::stats::set_timing(name, ms);
    if name == client_app::MILESTONE_MAP {
        platform_web::stats::set("timeToMapMs", ms);
    }
}

/// Called by `bootstrap.js` with `.wasm` size, instantiation and download time (ms).
#[wasm_bindgen]
pub fn on_loaded(wasm_bytes: f64, instantiate_ms: f64, download_ms: Option<f64>) {
    let kib = wasm_bytes / 1024.0;
    tracing::info!("wasm: {kib:.1} KiB, instantiation {instantiate_ms:.1} ms");
    platform_web::stats::set_timing("wasmInstantiateMs", instantiate_ms);
    if let Some(ms) = download_ms {
        tracing::info!("wasm download: {ms:.0} ms");
        platform_web::stats::set_timing("wasmDownloadMs", ms);
    }
}

/// The Web Audio output of the original UI (U-D7); the context starts suspended until the first press.
fn web_audio(on: bool) -> Option<Box<dyn platform::AudioOutput>> {
    if !on {
        return None;
    }
    match platform_web::WebAudio::new() {
        Ok(a) => Some(Box::new(a)),
        Err(e) => {
            tracing::warn!("no sound output: {e}");
            None
        }
    }
}
