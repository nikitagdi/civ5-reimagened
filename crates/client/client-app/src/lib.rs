//! Client assembly: winit loop over client-core, ui, render, assets. No browser APIs (P2).

pub mod account_sync;
mod app;
mod asset_boot;
pub mod autostart;
pub mod bridge;
pub mod camera_control;
mod client_data;
mod cursor;
mod dom_ui;
mod entity_assets;
pub mod fp_debug;
mod fx_assets;
pub mod golden;
pub mod hud;
mod icon_assets;
mod input;
mod lobby_art;
pub mod m2;
pub mod timeline;

thread_local! {
    static TRACE: std::cell::Cell<(timeline::Level, f64)> =
        const { std::cell::Cell::new((timeline::Level::Coarse, 0.0)) };
}

/// The turn timeline's level (`?trace=off|coarse|fine`; unknown words mean the default, coarse) and the page clock's
/// offset to calendar time (unix ms minus clock ms), set before `run`.
pub fn set_trace(level: Option<&str>, wall_offset_ms: f64) {
    let level = level.and_then(timeline::Level::parse).unwrap_or_default();
    TRACE.with(|t| t.set((level, wall_offset_ms)));
}

thread_local! {
    static WALL_SOURCE: std::cell::Cell<Option<fn() -> f64>> = const { std::cell::Cell::new(None) };
}

/// The page's way to read the clock's offset to calendar time (UTC unix ms minus clock ms) afresh. The timeline asks
/// for it every frame, so a clock that stalled (a sleeping laptop, a tab suspended in the background) does not leave
/// the timestamps behind the real time. Without a source the offset given to [`set_trace`] stays.
pub fn set_wall_offset_source(f: fn() -> f64) {
    WALL_SOURCE.with(|w| w.set(Some(f)));
}

pub(crate) fn wall_offset_now() -> Option<f64> {
    WALL_SOURCE.with(std::cell::Cell::get).map(|f| f())
}

pub(crate) fn trace_settings() -> (timeline::Level, f64) {
    TRACE.with(std::cell::Cell::get)
}

thread_local! {
    static M2_DEBUG_LUA: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Dev: Lua code the M2 layer runs once after the root loaded (web `?luadbg=`); its `print` goes to the log.
pub fn set_m2_debug_lua(code: Option<String>) {
    M2_DEBUG_LUA.with(|c| *c.borrow_mut() = code);
}

pub(crate) fn m2_debug_lua() -> Option<String> {
    M2_DEBUG_LUA.with(|c| c.borrow().clone())
}

thread_local! {
    static M2_LUA_QUEUE: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Dev (the playtest, `window.__civ5test.lua(code)`): Lua the M2 layer runs on its next frame, in a context of its
/// own (`print` goes to the log).
pub fn push_m2_lua(code: String) {
    M2_LUA_QUEUE.with(|q| q.borrow_mut().push(code));
}

pub(crate) fn take_m2_lua() -> Vec<String> {
    M2_LUA_QUEUE.with(|q| std::mem::take(&mut *q.borrow_mut()))
}
pub mod gfx_prefs;
pub mod m2_highlights;
pub mod map_camera;
pub mod map_labels;
pub mod minimap_image;
pub mod mod_assets;
pub mod mod_hooks;
pub mod mod_host;
pub mod mod_net;
pub mod net;
pub mod net_ui;
pub mod original_ui;
pub mod perf_report;
pub mod picking;
pub mod play;
pub mod plot_tooltip;
pub mod pointer;
mod production;
pub mod settings_store;
mod showcase;
mod terrain_assets;
pub mod test_port;
#[cfg(test)]
mod test_world;
mod touch_hud;
mod touch_press;
mod ui_press;
pub mod ui_sound;
pub mod unit_panel;
pub mod unit_sound;
pub mod world_sound;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use platform::{HttpClient, TaskPool};

/// Re-exported for `app-*`, which pick the transport (the network one) and read the clock.
pub use platform::{Clock, Transport};
use render::BackendPref;
use winit::window::{Window, WindowAttributes};

pub use app::{run, run_with};
pub use render;

/// Re-exported for `app-*` window extensions.
pub use winit;

/// Non-`Send`: web is single-threaded; wgpu objects are not `Send`.
pub type LocalFuture = Pin<Box<dyn Future<Output = ()>>>;

/// `spawn_local` on web, blocking `block_on` natively.
pub type Spawner = Box<dyn Fn(LocalFuture)>;

/// Attempts after the first need a fresh target.
pub type SurfaceProvider =
    Box<dyn FnMut(&Arc<Window>, u32) -> Option<render::wgpu::SurfaceTarget<'static>>>;

pub type WindowAttributesHook = Box<dyn FnOnce(WindowAttributes) -> WindowAttributes>;

#[derive(Debug, Clone)]
pub struct AppConfig {
    /// e.g. `ws://127.0.0.1:8080/ws`.
    pub server_url: String,
    /// The nick came from the page address or the command line; otherwise the nick of the last login (the
    /// store) is preferred over `nick`, which is then the default.
    pub nick_given: bool,
    /// The address of the page (web): the server address is checked against it (NC-D16). `None` (desktop): the
    /// address is taken as the user gave it.
    pub page_url: Option<String>,
    pub nick: String,
    pub backend: BackendPref,
    /// For `Hello::build`.
    pub build: String,
    /// Hash (64 hex digits) of the manifest blob to load instead of the server's current one.
    pub manifest: Option<String>,
    /// Orbit camera and the Warrior showcase instead of the game camera (`?showcase=1`,
    /// `--showcase`).
    pub showcase: bool,
    /// Show the synthetic skinned model instead of loading the Warrior (`?demo=skinned`); implies
    /// `showcase`.
    pub demo_skinned: bool,
    /// Play at once, without the lobby (`&autostart=1`, `--autostart`): on a server session the quick start
    /// script (`RoomScript`, [`AppConfig::room_params`]) runs after the login; on a local game the default
    /// setup starts. Scripted runs and the time-to-map measurement.
    pub autostart: bool,
    /// What the script asks for: `size`, `ai`, `civ`, `seed`, `mode` (`&size=`, `&ai=`, ...).
    pub room_params: autostart::RoomParams,
    /// Log FPS, draw calls and triangles once per second (`?stats=1`, `--stats`).
    pub stats: bool,
    /// Show the performance panel from the start (`?hud=1`, `--hud`); F3 toggles it.
    pub hud: bool,
    /// Preset, key overrides and layer switches from the URL or the command line (`?preset=`,
    /// `?gfx=`, `?disable=`; `--preset`, `--gfx`, `--disable`).
    pub graphics: settings_store::GraphicsOptions,
    /// Draw one capture of a `golden` scene and stop (`?golden=<url>&capture=<name>`, web; native uses
    /// [`golden::run_native`]). The frame goes to the test port.
    pub golden: Option<GoldenConfig>,
    /// The original UI (EUI) is the game screen (default; `?ui=interim`, `--interim-ui` turn it off and bring back the
    /// interim egui game panels). The lobby and the room are egui either way. `AppConfig::default()` keeps it off.
    pub original_ui: bool,
    /// With `original_ui`: the M1 fixture (four EUI contexts on stubs) instead of M2 (`?ui=m1`).
    pub original_ui_m1: bool,
    /// Volume of the UI sounds, 0 to 1 (?volume= as a percent). SIMP-exe-190: not an OptionsManager option.
    pub ui_volume: f32,
    /// A phone or a tablet (`gfx_prefs::is_touch_platform`): the defaults of the render scale and the
    /// shadow quality are lower there.
    pub touch_device: bool,
}

/// Where the files of the scene are and which capture to draw (`golden::GoldenFetch`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoldenConfig {
    /// URL of the folder of the scene.
    pub base_url: String,
    pub capture: String,
}

/// Receives the fps and the stats of the last frame (draws, triangles and the time of each pass,
/// `FrameStats::passes`) once per second when `AppConfig::stats` is set.
pub type StatsSink = Box<dyn Fn(f32, &render::FrameStats)>;

/// Receives the whole performance snapshot (perf-hud) once per second while `AppConfig::stats` is set or the
/// panel is shown. Web publishes it to `window.__civ5stats`.
pub type PerfSink = Box<dyn Fn(&render::perf::PerfSnapshot)>;

/// Posts the performance report of the panel's "Send report" (the JSON of `perf_report`) with the recent log
/// lines, as the `perf` section of `POST /client-report/v2`; calls the closure with the id of the report or why
/// it failed. Web only (the page owns the login and the log ring); `None` on the desktop, which writes a file.
pub type ReportSink = Box<dyn Fn(String, Box<dyn FnOnce(Result<String, String>)>)>;

/// Receives a start-up milestone once: `(name, ms since the client clock's origin)`; on the web
/// that origin is the page start. Names: [`MILESTONE_TERRAIN`],
/// [`MILESTONE_MAP`].
pub type MilestoneSink = Box<dyn Fn(&'static str, f64)>;

/// The server attached this client to a game (`Attached`): the loading screen of the game begins (lobby-ui 4.6).
pub const MILESTONE_ATTACHED: &str = "attachedAtMs";
/// The first view of the game and its catalog are there (the phase is `Playing`).
pub const MILESTONE_KEYFRAME: &str = "keyframeAtMs";
/// The first frame with the world's terrain was drawn (the textures may still be loading).
pub const MILESTONE_TERRAIN: &str = "terrainAtMs";
/// The first frame with the world's terrain and its textures in place: the time to map (AC-1.3).
pub const MILESTONE_MAP: &str = "mapAtMs";

/// Platform services and hooks from `app-*`.
pub struct Services {
    pub transport: Box<dyn Transport>,
    pub http: Box<dyn HttpClient>,
    pub clock: Box<dyn Clock>,
    pub tasks: Box<dyn TaskPool>,
    pub spawner: Spawner,
    /// `None`: the winit window.
    pub surface_provider: Option<SurfaceProvider>,
    pub window_attributes: Option<WindowAttributesHook>,
    /// `None`: stats are only logged. Web publishes them to `window.__civ5stats`.
    pub stats_sink: Option<StatsSink>,
    /// `None`: the snapshot is only logged (`--stats`).
    pub perf_sink: Option<PerfSink>,
    /// `None`: the panel's "Send report" writes a file (`perf_report::write_file`). Web posts it.
    pub report_sink: Option<ReportSink>,
    /// `None`: milestones are only logged. Web publishes them to `window.__civ5stats.timing`.
    pub milestone_sink: Option<MilestoneSink>,
    /// Reloads the page (the error screen's Reload). `None`: the game state is reset in place.
    pub reload: Option<Box<dyn Fn()>>,
    /// The playtest's scripted player (web `?test=1`). `None`: no test hooks.
    pub test_port: Option<Box<dyn test_port::TestPort>>,
    /// Keeps `settings/graphics.ini` (see `settings_store`). `None`: the settings come from the
    /// URL or the command line each time and are not remembered.
    pub settings_blobs: Option<Box<dyn platform::BlobStore>>,
    /// The token of the session and the last nick (`localStorage` on the web). `None`: kept in memory only.
    pub kv_store: Option<Box<dyn platform::KeyValueStore>>,
    /// The choices of the device that stay on it and do not follow the account (`gfx_prefs`, `civ5.gfx.*`):
    /// a file on the desktop. `None`: the store of `kv_store` (`localStorage` on the web, which is the device's).
    pub device_store: Option<Box<dyn platform::KeyValueStore>>,
    /// Installs the drivers of the network that are not the frame (NC-D9, 4.8): the safety timer and the
    /// page events. Called once, when the session of the server is made.
    pub net_driver: Option<NetDriver>,
    /// Receives counter groups (ui, replica, exe, m2) about once a second in the M2 mode. Web publishes
    /// them to `window.__civ5stats.<group>`.
    pub group_stats_sink: Option<GroupStatsSink>,
    /// The sound output of the original UI (Web Audio in the browser). None: silent.
    pub audio: Option<Box<dyn platform::AudioOutput>>,
    /// The web page's own sign-in, lobby and room screens (DOM). None: egui draws them (native, ?egui=1).
    pub ui_bridge: Option<Box<dyn platform::UiBridge>>,
    /// What the page says about the pointer lock (web: `document.pointerLockElement`). `None` (desktop): the lock
    /// ends only when the app lets it go or the window loses the focus.
    pub pointer_lock_probe: Option<Box<dyn platform::PointerLockProbe>>,
}

/// `(group, [(name, value)])`.
pub type GroupStatsSink = Box<dyn Fn(&str, &[(String, f64)])>;

/// Gets a handle of the network and the clock of the app; what it returns is kept as long as the app runs
/// (the timer and the listeners stop when it is dropped).
pub type NetDriver =
    Box<dyn FnOnce(net::NetHandle, std::rc::Rc<dyn Clock>) -> Box<dyn std::any::Any>>;

pub fn default_surface_provider() -> SurfaceProvider {
    Box::new(|window, _attempt| Some(render::wgpu::SurfaceTarget::from(Arc::clone(window))))
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("event loop: {0}")]
    EventLoop(#[from] winit::error::EventLoopError),
}
