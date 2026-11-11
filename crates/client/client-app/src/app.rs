//! Frame loop (winit). The renderer is built async (winit can't block on web).

use std::cell::RefCell;
use std::rc::Rc;

use render::perf::{RamTag, ram_scope};
use std::sync::Arc;

use client_core::{AddrContext, ClientEvent, ClientPhase, Ctx, NetConfig};
use draw_list::UiDrawList;
use glam::{Mat4, Vec3};
use interim_ui::InterimUi;
use interim_ui::screens::minimap::{MinimapSource, MinimapView};
use interim_ui::screens::settings::{QualityPreset, SettingsView};
use interim_ui::screens::{self, Screens, UiAction};
use protocol::{GameId, WorldView};
use render::{Camera, CameraView, GameCamera, RenderError, Renderer, Scene};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use crate::asset_boot::{AssetsBoot, LightBoot, TerrainBoot, WaterBoot};
use crate::bridge::WorldBridge;
use crate::camera_control::CameraController;
use crate::entity_assets::EntityAssets;
use crate::hud::{self, Hud};
use crate::icon_assets::IconAssets;
use crate::input::InputTranslator;
use crate::map_camera::MapCameraController;
use crate::map_labels::LabelCamera;
use crate::minimap_image::{self, MinimapBuilder};
use crate::mod_hooks::ModHooks;
use crate::net::{NetClient, NetHandle, stored_nick};
use crate::net_ui::{NetEffect, NetUi, phase_name};
use crate::picking::Picker;
use crate::play::hotkeys::HotKey;
use crate::play::{Outputs, PlayController, UiEffect};
use crate::production::ProductionChooser;
use crate::settings_store::{self, SettingsStore};
use crate::showcase::{Showcase, Source};
use crate::terrain_assets::TerrainAssets;
use crate::unit_panel::{self, UnitPanel};
use crate::{AppConfig, AppError, Services, SurfaceProvider, default_surface_provider};

/// Background task budget per frame (web).
const TASK_BUDGET_MS: u32 = 4;
/// The least time between two chat lines (lobby-ui 4.7).
const CHAT_GAP_MS: u64 = 500;
/// Milliseconds between two looks at the store for a new render scale or shadow quality.
const GFX_POLL_MS: u64 = 250;

/// The server session, or the refusal of the server address.
enum Link {
    /// The server session on `ClientCore`: sign-in, lobby, room, loading (lobby-ui), and the game itself.
    Net(Box<NetLink>),
    /// The server address was refused (NC-D16): the error screen says why.
    NetFailed(String),
}

impl Link {
    fn update(&mut self, now_ms: u64) {
        match self {
            Link::Net(n) => n.update(now_ms),
            Link::NetFailed(_) => {}
        }
    }
}

/// `Rc<dyn HttpClient>` as an `HttpClient`: the asset loader and the catalog fetch share one client.
struct SharedHttp(Rc<dyn platform::HttpClient>);

impl platform::HttpClient for SharedHttp {
    fn get(&self, url: &str) -> Box<dyn platform::HttpOp> {
        self.0.get(url)
    }
}

/// A store two owners share (`Rc`): the net client's `Box<dyn KeyValueStore>` over the page's store.
struct SharedKv(Rc<dyn platform::KeyValueStore>);

impl platform::KeyValueStore for SharedKv {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key)
    }

    fn set(&self, key: &str, value: &str) {
        self.0.set(key, value);
    }

    fn remove(&self, key: &str) {
        self.0.remove(key);
    }

    fn keys_with_prefix(&self, prefix: &str) -> Vec<String> {
        self.0.keys_with_prefix(prefix)
    }
}

/// The server session: the network shell, the screens' state, the phase seen last and what the app has taken
/// from the view so far.
struct NetLink {
    handle: NetHandle,
    ui: NetUi,
    started: bool,
    /// The phase of the last frame, for the milestones.
    phase_seen: Option<&'static str>,
    /// The game is attached and its first view and catalog are in (`ClientPhase::Playing`).
    playing: bool,
    /// Events of the core the game screens act on, in the order they came (taken by `App::pump_net_events`).
    game_events: Vec<ClientEvent>,
    /// The game the controls were made for; another one starts them afresh.
    game: Option<GameId>,
    /// The hash of the catalog the bridge has.
    bridge_catalog: Option<u64>,
    /// The game and the `revision` the bridge has synced; the controller was told of.
    bridge_rev: Option<(GameId, u64)>,
    /// The catalog and the revision of the mods' terrain share (mod API 1.2) the renderer was given.
    share_rev: Option<(u64, u64)>,
    world_rev: Option<(GameId, u64)>,
    /// The camera went to the first unit of this game.
    centered: Option<GameId>,
    /// The bridge has handed a world to the renderer, which the next game must not inherit.
    world_up: bool,
    /// Another game was attached while the world of the last one was still up (no lobby frame in between): the
    /// world is cleared before the new game's first view reaches the renderer.
    clear_world: bool,
    /// v12: the account's settings follow the player to the server (`account_sync`), over the same store the
    /// original UI writes its options to.
    account: Rc<crate::account_sync::AccountSync>,
    account_kv: Rc<dyn platform::KeyValueStore>,
    /// What the page keeps running besides the frame (timer, listeners); dropped with the app.
    _driver: Option<Box<dyn std::any::Any>>,
}

impl NetLink {
    /// One frame of the network, the events, the quick start script.
    fn update(&mut self, now_ms: u64) {
        use render::perf::slow::scope;
        let s = scope("net.pump");
        if self.started {
            self.handle.pump(now_ms);
        } else {
            self.started = true;
            self.handle.connect(now_ms);
        }
        drop(s);
        let s = scope("net.events");
        while let Some(ev) = self.handle.poll_event() {
            self.ui.on_event(&ev, now_ms);
            if matches!(ev, ClientEvent::AccountLoaded)
                && let Some(data) = self.handle.with(|c| c.account().cloned())
            {
                self.account.on_loaded(&data, &*self.account_kv);
            }
            if matches!(
                ev,
                ClientEvent::Attached { .. }
                    | ClientEvent::OrderAnswered { .. }
                    | ClientEvent::OrderRolledBack { .. }
                    | ClientEvent::QueryResult { .. }
                    | ClientEvent::Detached(_)
                    | ClientEvent::GameCtlRejected { .. }
                    | ClientEvent::HostChanged { .. }
                    | ClientEvent::EndTurnCancelled { .. }
                    | ClientEvent::ModSet(_)
                    | ClientEvent::ModMsg(_)
            ) {
                self.game_events.push(ev);
            }
        }
        drop(s);
        let _s = scope("net.ui_tick");
        let ui = &mut self.ui;
        let account = &self.account;
        self.handle.with(|core| {
            ui.tick(core, now_ms);
            account.flush(core, now_ms);
        });
        self.playing = matches!(self.phase(), ClientPhase::Playing);
    }

    fn phase(&self) -> ClientPhase {
        self.handle.with_client(|c| c.phase())
    }

    /// The milestone this phase change reaches, once: the time of `Attached` and of the first view.
    fn milestone(&mut self) -> Option<&'static str> {
        let now = phase_name(&self.phase());
        if self.phase_seen == Some(now) {
            return None;
        }
        self.phase_seen = Some(now);
        match now {
            "loading" => Some(crate::MILESTONE_ATTACHED),
            "play" => Some(crate::MILESTONE_KEYFRAME),
            _ => None,
        }
    }

    fn apply(&mut self, action: UiAction, now_ms: u64) -> NetEffect {
        let ui = &mut self.ui;
        self.handle.with(|core| ui.apply(core, action, now_ms))
    }
}

type RendererSlot = Rc<RefCell<Option<Result<Renderer, RenderError>>>>;

/// The yes / no question on the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Question {
    /// The selected unit's move (or ranged attack, `ranged`) attacks this civilization at peace
    /// (`PlayController::answer_war`).
    War { civ: String, ranged: bool },
    /// Disband this unit?
    Disband { unit: i32, name: String },
    /// Declare war on this civilization (the players panel, LU-D7).
    DeclareWar { team: i8, civ: String },
    /// Remove this human; the AI plays the civilization (LU-D7, LU-D8).
    Kick {
        player: i8,
        nick: String,
        civ: String,
    },
    /// Leave to the lobby (LU-D14); `host`: the role passes on after 60 s.
    LeaveGame { host: bool },
}

impl Question {
    fn view(&self) -> screens::confirm::ConfirmView {
        let (title, text, yes, no, danger) = match self {
            Question::War { civ, ranged } => (
                "Declare war?",
                format!(
                    "You are at peace with {civ}. {} there attacks them: declare war on {civ}?",
                    if *ranged { "Shooting" } else { "Moving" }
                ),
                "Declare war",
                "Cancel",
                true,
            ),
            Question::Disband { name, .. } => (
                "Disband unit?",
                format!("Disband the {name}? It is gone for good."),
                "Disband",
                "Keep",
                true,
            ),
            Question::DeclareWar { civ, .. } => (
                "Declare war?",
                format!("Declare war on {civ}?"),
                "Declare war",
                "Cancel",
                true,
            ),
            Question::Kick { nick, civ, .. } => (
                "Remove player?",
                format!("Remove {nick}? The AI will play {civ}."),
                "Remove",
                "Cancel",
                true,
            ),
            Question::LeaveGame { host } => {
                let mut text = "Leave to the lobby? Your civilization stays yours: the game waits for you until you come back or the host hands it to the AI.".to_owned();
                if *host {
                    text.push_str("\nYou are the host. After 60 s away the host role passes to the next player.");
                }
                ("Leave game?", text, "Leave game", "Stay", false)
            }
        };
        screens::confirm::ConfirmView {
            title: title.to_owned(),
            text,
            yes: yes.to_owned(),
            no: no.to_owned(),
            danger,
        }
    }
}

/// Native blocks until the window closes; web returns at once.
pub fn run(config: AppConfig, services: Services) -> Result<(), AppError> {
    // Debug: `CIV5_FP_CAM` brings the first-person camera on F4 (client-hooks C2). Not set: no hook.
    #[allow(unused_mut)]
    let mut hooks = ModHooks::new();
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(hook) = crate::fp_debug::FirstPersonDebug::from_env() {
        hooks.register(Box::new(hook));
    }
    run_with(config, services, hooks)
}

/// [`run`] with the hooks of the client's mods (client-hooks). Empty hooks: the app of `run`.
pub fn run_with(config: AppConfig, services: Services, hooks: ModHooks) -> Result<(), AppError> {
    let event_loop = EventLoop::new()?;
    let app = App::new(config, services, hooks);
    tracing::info!("client started");
    #[cfg(target_arch = "wasm32")]
    {
        use winit::platform::web::EventLoopExtWebSys;
        event_loop.spawn_app(app);
        Ok(())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut app = app;
        event_loop.run_app(&mut app)?;
        Ok(())
    }
}

struct App {
    config: AppConfig,
    clock: Rc<dyn platform::Clock>,
    tasks: Box<dyn platform::TaskPool>,
    spawner: crate::Spawner,
    surface_provider: Option<SurfaceProvider>,
    window_attributes: Option<crate::WindowAttributesHook>,

    window: Option<Arc<Window>>,
    /// The window waiting for its renderer: the stored settings must arrive first, since the
    /// samples of the main pass are fixed when the renderer is made.
    renderer_window: Option<Arc<Window>>,
    /// The game's mouse cursors (from the asset set's `data:cursors`) and the one shown now.
    cursors: Option<crate::cursor::Cursors>,
    /// The cursor set that arrived and waits for the event loop to make the window cursors.
    pending_cursors: Option<render::asset_format::CursorSet>,
    /// The data blobs read once (`data:cursors`).
    client_data: crate::client_data::ClientData,
    cursor_shown: Option<crate::cursor::Pick>,
    /// The game is loading (the globe cursor), set by the frame's net branch.
    loading_cursor: bool,
    settings_store: SettingsStore,
    /// Switches resolved at start, given to the renderer once it exists.
    start_layers: Option<render::LayerSwitches>,
    renderer_slot: RendererSlot,
    renderer: Option<Renderer>,
    render_failed: bool,
    /// The render scale, the shadow quality and the multisampling (`gfx_prefs`): the store they live in
    /// (the device's own, the account does not follow it), and what the renderer was last told.
    gfx_kv: Rc<dyn platform::KeyValueStore>,
    gfx_applied: Option<crate::gfx_prefs::GfxPrefs>,
    gfx_polled_ms: u64,

    net: Link,
    assets: Option<AssetsBoot>,
    showcase: Showcase,
    terrain_assets: TerrainAssets,
    /// The light data of the `boot` group; the map is not drawn before it.
    light_boot: LightBoot,
    terrain_boot: TerrainBoot,
    water_boot: WaterBoot,
    entity_assets: EntityAssets,
    fx_assets: crate::fx_assets::FxAssets,
    icon_assets: IconAssets,
    input: InputTranslator,
    /// The touch long press that is a right click.
    long_press: crate::touch_press::LongPress,
    /// Three fingers held: toggles the performance panel.
    hud_hold: crate::touch_hud::HudHold,
    /// The pointer lock of the window (client-hooks); idle until something asks for it.
    pointer: crate::pointer::PointerService,
    /// The hooks of the client's mods: they see the input first (empty: nothing changes).
    hooks: ModHooks,
    /// The HUD textures of mods that are on the GPU (kept, to put them back when the cache evicts one).
    mod_textures: Vec<(draw_list::TextureId, render::UiTextureData)>,
    /// The pose of the strategic camera for the mods: kept as it was while a mod holds the camera.
    strategic_pose: Option<mod_client::StrategicCamera>,
    /// The plot under the cursor in the strategic view, for the mods.
    mod_hover: Option<(i32, i32)>,
    /// `?showcase=1` only: orbit camera and the Warrior.
    showcase_mode: bool,
    camera: Camera,
    controller: CameraController,
    game_camera: GameCamera,
    map_controller: MapCameraController,
    picker: Picker,
    /// Selection, orders and hotkeys of the game.
    play: PlayController,
    /// Interim production panel of the opened city (SIMP-001).
    production: ProductionChooser,
    /// The brain's actions of the selected unit, for the unit panel.
    unit_panel: UnitPanel,
    question: Option<Question>,
    /// When the last chat line went out (ms): at most one in 500 ms (lobby-ui 4.7).
    last_chat_ms: Option<u64>,
    /// How long the pointer rested on the same plot, and the plot whose tooltip shows.
    dwell: crate::plot_tooltip::Dwell,
    tooltip_cell: Option<(i32, i32)>,
    /// Lobby, loading, top bar, end-turn button, journal, error screen of the local game.
    hud: Hud,
    reload: Option<Box<dyn Fn()>>,
    bridge: WorldBridge,
    /// When the trade route paths were last asked of the replica core (seconds of `t`).
    trade_paths_at: f32,
    ui: UiDrawList,
    /// The original UI (EUI), when `AppConfig::original_ui` is set; it fills `ui` each frame.
    original_ui: Option<crate::original_ui::OriginalUi>,
    /// The game's interfaces (the classic original UI over the replica of a real game, when
    /// `AppConfig::original_ui` is set): the one on the screen gets the frames and the input.
    ui_host: crate::ui_host::UiHost,
    /// The game session (the replica, its data and network, the game's localizer): there when a game UI reads the
    /// replica (not with `?ui=interim`).
    session: Option<crate::session::GameSession>,
    /// TEMP(T53): `?ui=v2[:skin]`, a UI v2 skin over the classic UI (which then neither draws nor gets input).
    v2: Option<crate::ui_host::v2::V2Host>,
    group_stats_sink: Option<crate::GroupStatsSink>,
    /// The page's own sign-in, lobby and room screens, when the platform has them (web).
    dom_ui: Option<crate::dom_ui::DomUi>,
    /// The game art of the page's lobby and room (web:lobby), handed to the page once.
    lobby_art: crate::lobby_art::LobbyArt,
    /// The sounds of the original UI (button clicks, Lua requests), when there is an output.
    ui_sound: Option<crate::ui_sound::UiSound>,
    ui_sound_stats_ms: u64,
    /// SIMPLIFIED(SIMP-001): the interim egui UI, drawn as the renderer's overlay.
    interim: InterimUi,
    /// Smoothed frames per second, for the placeholder panel.
    fps: f32,
    /// The stand-in picture of the minimap (SIMP-060), and whether the panel showed the renderer's
    /// texture this frame (then a click goes through `Renderer::minimap_to_world`).
    minimap: MinimapBuilder,
    minimap_texture: bool,

    start_ms: u64,
    last_frame_ms: u64,
    frames: u64,
    fps_window_start_ms: u64,
    render_errors: u64,
    /// `?stats=1`: frames and window start for the once-per-second line.
    stats_frames: u64,
    stats_window_start_ms: u64,
    stats_sink: Option<crate::StatsSink>,
    /// The performance panel is shown (F3, `?hud=1`), its last snapshot, and when it was taken.
    hud_on: bool,
    perf_view: Option<interim_ui::screens::perf_panel::PerfView>,
    perf_view_at_ms: u64,
    /// Where the last "Send report" of the panel is, and the answer of the page when it comes.
    perf_report: interim_ui::screens::perf_panel::ReportState,
    perf_report_answer: Rc<std::cell::RefCell<Option<Result<String, String>>>>,
    /// Posts the report (web); None: a file.
    report_sink: Option<crate::ReportSink>,
    /// Receives the snapshot once per second while `?stats=1` or the panel is on.
    perf_sink: Option<crate::PerfSink>,
    milestone_sink: Option<crate::MilestoneSink>,
    /// Start-up milestones already reported (each once).
    milestones: Vec<&'static str>,
    /// The playtest's scripted player (`?test=1`).
    test_port: Option<Box<dyn crate::test_port::TestPort>>,
    /// `?golden=<url>`: one capture of a `golden` scene instead of the game.
    golden: GoldenState,
}

/// The `golden` frame of this page (`AppConfig::golden`).
enum GoldenState {
    Off,
    Fetching(crate::golden::GoldenFetch),
    Running(Box<crate::golden::GoldenDriver>),
    /// The test port has the frame, or the reason there is none.
    Ended,
}

impl App {
    fn new(config: AppConfig, services: Services, mut hooks: ModHooks) -> Self {
        let Services {
            transport,
            http,
            clock,
            tasks,
            spawner,
            surface_provider,
            window_attributes,
            stats_sink,
            perf_sink,
            report_sink,
            milestone_sink,
            reload,
            test_port,
            settings_blobs,
            kv_store,
            device_store,
            net_driver,
            group_stats_sink,
            audio,
            ui_bridge,
            pointer_lock_probe,
        } = services;
        let hud_on = config.hud;
        let (config_original_ui, config_ui_volume) = (config.original_ui, config.ui_volume);
        let http: Rc<dyn platform::HttpClient> = Rc::from(http);
        let clock: Rc<dyn platform::Clock> = Rc::from(clock);
        // One store for the net client (token, nick) and the original UI (options, user data): its keys differ.
        let shared_kv: Rc<dyn platform::KeyValueStore> =
            Rc::from(kv_store.unwrap_or_else(|| Box::new(platform::MemoryStore::new())));
        // The mods' hooks learn what only the app has: the HTTP client and address (their asset packs are fetched
        // from the server) and the player's settings store (their keys live in it, one namespace per mod).
        hooks.bind(&crate::mod_hooks::HookEnv {
            http: Rc::clone(&http),
            server_url: config.server_url.clone(),
            kv: Rc::clone(&shared_kv),
        });
        // The graphics choices stay on the device: a store of their own, else the page's (not through the account).
        let device_kv: Rc<dyn platform::KeyValueStore> = match device_store {
            Some(store) => Rc::from(store),
            None => Rc::clone(&shared_kv),
        };
        // v12: the options the original UI writes follow the account; the local store stays the fallback.
        let account = crate::account_sync::AccountSync::new();
        let ui_kv: Rc<dyn platform::KeyValueStore> = Rc::new(crate::account_sync::AccountKv::new(
            Rc::clone(&shared_kv),
            Rc::clone(&account),
        ));
        // The volume knobs the player saved: the main menu's music has its slider before a game runs the options menu.
        let saved_knobs = exe_api::m4_wog::stored_volume_knobs(&*ui_kv, config_ui_volume);
        let net = {
            let store: Box<dyn platform::KeyValueStore> = Box::new(SharedKv(shared_kv));
            // The nick of the page, else the one of the last login, else the default.
            let nick = if config.nick_given {
                config.nick.clone()
            } else {
                stored_nick(&*store).unwrap_or_else(|| config.nick.clone())
            };
            let ctx = match &config.page_url {
                Some(url) => AddrContext::Page { url },
                None => AddrContext::Desktop,
            };
            // The mods built in tell the server in the handshake (`ModHello`); no mods, no change.
            let mut net_config = NetConfig::new(config.build.clone(), nick);
            hooks.configure_net(&mut net_config);
            match NetClient::new(
                net_config,
                ctx,
                Some(&config.server_url),
                transport,
                Box::new(SharedHttp(Rc::clone(&http))),
                store,
            ) {
                Ok(client) => {
                    let mut ui = NetUi::new(client.url());
                    ui.set_manual_choices(config.room_params.manual_choices);
                    if config.autostart {
                        ui.quick_start(config.room_params.clone());
                    }
                    let handle = NetHandle::new(client);
                    let driver =
                        net_driver.map(|install| install(handle.clone(), Rc::clone(&clock)));
                    Link::Net(Box::new(NetLink {
                        handle,
                        ui,
                        started: false,
                        phase_seen: None,
                        playing: false,
                        game_events: Vec::new(),
                        game: None,
                        bridge_catalog: None,
                        bridge_rev: None,
                        share_rev: None,
                        world_up: false,
                        clear_world: false,
                        world_rev: None,
                        centered: None,
                        account: Rc::clone(&account),
                        account_kv: Rc::clone(&ui_kv),
                        _driver: driver,
                    }))
                }
                Err(e) => {
                    tracing::error!("the server address is refused: {e}");
                    Link::NetFailed(format!("Cannot connect: {e}"))
                }
            }
        };
        let golden = match &config.golden {
            Some(g) => GoldenState::Fetching(crate::golden::GoldenFetch::start(
                &*http,
                &g.base_url,
                &g.capture,
            )),
            None => GoldenState::Off,
        };
        let lobby_art = crate::lobby_art::LobbyArt::new(
            crate::asset_boot::assets_base_from_ws_url(&config.server_url),
        );
        let v2_http = Rc::clone(&http);
        let assets = AssetsBoot::start(http, &config.server_url, config.manifest.as_deref());
        let showcase = Showcase::new(if config.demo_skinned {
            Source::SyntheticDemo
        } else {
            Source::Warrior
        });
        let original_ui =
            (config.original_ui && config.original_ui_m1).then(crate::original_ui::OriginalUi::m1);
        let mut ui_host = crate::ui_host::UiHost::new();
        if config.original_ui && !config.original_ui_m1 && matches!(net, Link::Net(_)) {
            // The classic UI: removing it is removing this registration and its modules.
            ui_host.register(Box::new(crate::ui_host::classic::ClassicFrontend::new(
                Rc::clone(&ui_kv),
                config_ui_volume,
            )));
        }
        let session = ui_host
            .needs_replica()
            .then(crate::session::GameSession::new);
        let v2 = crate::ui_host::v2::take_choice()
            .filter(|_| ui_host.has_game_ui())
            .map(|choice| {
                let c = Rc::clone(&clock);
                crate::ui_host::v2::V2Host::new(choice, v2_http, Rc::new(move || c.now_ms_f64()))
            });
        let start_ms = clock.now_ms();
        let showcase_mode = config.showcase || config.demo_skinned;
        // The controls of a server session read `base` plus the layer (NC-D13).
        let mut play = PlayController::new();
        play.set_layered(matches!(net, Link::Net(_)));
        Self {
            showcase_mode,
            config,
            clock,
            tasks,
            spawner,
            surface_provider: Some(surface_provider.unwrap_or_else(default_surface_provider)),
            window_attributes,
            window: None,
            renderer_window: None,
            cursors: None,
            cursor_shown: None,
            loading_cursor: false,
            settings_store: SettingsStore::new(settings_blobs),
            start_layers: None,
            renderer_slot: Rc::new(RefCell::new(None)),
            renderer: None,
            render_failed: false,
            gfx_kv: device_kv,
            gfx_applied: None,
            gfx_polled_ms: 0,
            net,
            assets,
            showcase,
            terrain_assets: TerrainAssets::new(),
            light_boot: LightBoot::new(),
            terrain_boot: TerrainBoot::new(),
            water_boot: WaterBoot::new(),
            entity_assets: EntityAssets::new(),
            fx_assets: crate::fx_assets::FxAssets::new(),
            icon_assets: IconAssets::new(),
            input: InputTranslator::default(),
            long_press: crate::touch_press::LongPress::new(),
            hud_hold: crate::touch_hud::HudHold::new(),
            pointer: crate::pointer::PointerService::new(pointer_lock_probe),
            hooks,
            mod_textures: Vec::new(),
            strategic_pose: None,
            mod_hover: None,
            camera: Camera::default(),
            controller: CameraController::new(1, 1),
            game_camera: GameCamera::default(),
            map_controller: MapCameraController::new(1, 1),
            picker: Picker::default(),
            play,
            production: ProductionChooser::default(),
            unit_panel: UnitPanel::default(),
            question: None,
            last_chat_ms: None,
            dwell: crate::plot_tooltip::Dwell::default(),
            tooltip_cell: None,
            hud: Hud::new(),
            reload,
            bridge: WorldBridge::new(),
            trade_paths_at: f32::NEG_INFINITY,
            ui: UiDrawList::new(),
            original_ui,
            ui_host,
            session,
            v2,
            group_stats_sink,
            dom_ui: ui_bridge.map(crate::dom_ui::DomUi::new),
            lobby_art,
            pending_cursors: None,
            client_data: crate::client_data::ClientData::new(),
            ui_sound: audio
                .filter(|_| config_original_ui)
                .map(|out| crate::ui_sound::UiSound::new(out, saved_knobs)),
            ui_sound_stats_ms: 0,
            interim: InterimUi::new(),
            fps: 0.0,
            minimap: MinimapBuilder::default(),
            minimap_texture: false,
            start_ms,
            last_frame_ms: start_ms,
            frames: 0,
            fps_window_start_ms: start_ms,
            render_errors: 0,
            stats_frames: 0,
            stats_window_start_ms: start_ms,
            stats_sink,
            hud_on,
            perf_view: None,
            perf_view_at_ms: 0,
            perf_report: Default::default(),
            perf_report_answer: Rc::default(),
            report_sink,
            perf_sink,
            milestone_sink,
            milestones: Vec::new(),
            test_port,
            golden,
        }
    }

    /// Logs and reports a start-up milestone the first time it happens.
    fn milestone(&mut self, name: &'static str, what: &str) {
        report_milestone(
            &mut self.milestones,
            self.milestone_sink.as_ref(),
            &*self.clock,
            name,
            what,
        );
    }

    fn create_window(&mut self, event_loop: &ActiveEventLoop) {
        let mut attrs = Window::default_attributes().with_title("civ5-reimagened");
        if let Some(hook) = self.window_attributes.take() {
            attrs = hook(attrs);
        }
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                tracing::error!("failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };
        let size = window.inner_size();
        self.controller = CameraController::new(size.width, size.height);
        self.map_controller = MapCameraController::new(size.width, size.height);
        self.window = Some(Arc::clone(&window));
        self.renderer_window = Some(window);
        self.try_start_renderer();
    }

    /// Starts the renderer once the stored settings are read (at once without a store), with the
    /// settings and switches the sources give (`settings_store::resolve`).
    fn try_start_renderer(&mut self) {
        if self.renderer_window.is_none() {
            return;
        }
        let Some(stored) = self.settings_store.poll_load() else {
            return;
        };
        let resolved = settings_store::resolve(stored.as_deref(), &self.config.graphics);
        for warning in &resolved.warnings {
            tracing::warn!("graphics settings: {warning}");
        }
        tracing::info!(
            stored = stored.is_some(),
            msaa = resolved.settings.msaa_samples,
            shadows = resolved.settings.shadow_level,
            fow = resolved.settings.fow_level,
            off = ?resolved.layers.names().collect::<Vec<_>>(),
            "graphics settings"
        );
        self.start_layers = Some(resolved.layers);
        let mut settings = resolved.settings;
        // The multisampling the player picked, unless the address or the command line asked for a preset
        // or a key (the samples are fixed when the renderer is made).
        let asked =
            self.config.graphics.preset.is_some() || self.config.graphics.overrides.is_some();
        if matches!(self.golden, GoldenState::Off) && !asked {
            let prefs = crate::gfx_prefs::GfxPrefs::read(&*self.gfx_kv, self.config.touch_device);
            settings.msaa_samples = prefs.msaa.samples();
        }
        if let Some(window) = self.renderer_window.take() {
            self.start_renderer(window, settings);
        }
    }

    fn start_renderer(&mut self, window: Arc<Window>, settings: render::RenderSettings) {
        let Some(mut provider) = self.surface_provider.take() else {
            return;
        };
        let slot = Rc::clone(&self.renderer_slot);
        let pref = self.config.backend;
        let size = window.inner_size();
        let size = (size.width.max(1), size.height.max(1));
        tracing::info!(?pref, "initializing renderer, {}×{}", size.0, size.1);
        (self.spawner)(Box::pin(async move {
            let mut attempt = 0;
            let mut targets = || {
                let t = provider(&window, attempt);
                attempt += 1;
                t
            };
            let result = Renderer::with_settings(&mut targets, size, pref, settings).await;
            *slot.borrow_mut() = Some(result);
            // Wake the loop (web frames need request_redraw).
            window.request_redraw();
        }));
    }

    /// Takes the renderer once async init finishes.
    fn poll_renderer(&mut self) {
        if self.renderer.is_some() || self.render_failed {
            return;
        }
        let Some(result) = self.renderer_slot.borrow_mut().take() else {
            return;
        };
        match result {
            Ok(mut r) => {
                if let Some(layers) = self.start_layers.take() {
                    r.set_layers(layers);
                }
                // The build of a map's terrain goes on a few milliseconds a frame (not for the goldens,
                // which compare a whole map).
                r.set_terrain_incremental(matches!(self.golden, GoldenState::Off));
                // `?debug=shadow`.
                r.set_debug_shadow(self.config.graphics.debug_has("shadow"));
                // The pass times of `?stats=1` need a clock with fractions of a millisecond; the
                // browser has no `Instant`.
                let clock = Rc::clone(&self.clock);
                r.set_cpu_clock(Box::new({
                    let clock = Rc::clone(&clock);
                    move || clock.now_ms_f64()
                }));
                // The slow-frame record reads the same clock.
                render::perf::slow::set_clock(Rc::new(move || clock.now_ms_f64()));
                // The render scale and the shadow quality before the first targets are made.
                if matches!(self.golden, GoldenState::Off) {
                    let prefs =
                        crate::gfx_prefs::GfxPrefs::read(&*self.gfx_kv, self.config.touch_device);
                    r.set_render_scale(prefs.render_scale);
                    r.set_shadow_quality(prefs.shadow_quality);
                    r.set_terrain_detail(prefs.terrain_detail);
                    r.set_gfx_preset(render::gfx_choice::GfxPreset::index_of(prefs.preset()));
                    self.gfx_applied = Some(prefs);
                    self.gfx_polled_ms = self.clock.now_ms();
                }
                if let Some(w) = &self.window {
                    let s = w.inner_size();
                    r.resize((s.width, s.height), w.scale_factor() as f32);
                }
                let info = r.backend();
                tracing::info!(
                    "renderer ready: {:?} ({}), texture compression {:?}",
                    info.kind,
                    info.adapter,
                    info.compression
                );
                self.renderer = Some(r);
            }
            Err(e) => {
                // Without a renderer, networking and logs keep running.
                tracing::error!("renderer creation failed: {e}");
                self.render_failed = true;
            }
        }
    }

    /// What the player did on the screens of a server session: the lobby commands go to the core; what only
    /// the shell can do (sign in, sign out, reload) is done here.
    fn apply_net_ui(&mut self, actions: Vec<UiAction>) {
        let now = self.clock.now_ms();
        for action in actions {
            let Link::Net(n) = &mut self.net else {
                return;
            };
            match n.apply(action, now) {
                NetEffect::None => {}
                NetEffect::Login { nick, password } => {
                    n.handle.with_client(|c| c.login(&nick, &password, now));
                }
                NetEffect::SignOut => n.handle.with_client(|c| c.sign_out(now)),
                NetEffect::Reload => self.reload_game(),
            }
        }
    }

    /// A host or seat control of the game (kick, pause, leave); refused without a line (NC-D5).
    fn send_game_ctl(&mut self, ctl: protocol::GameCtl) {
        tracing::info!("game control sent: {ctl:?}");
        if let Link::Net(n) = &mut self.net
            && let Err(why) = n.handle.with(|core| core.game_ctl(ctl))
        {
            tracing::info!("game control refused: {why:?}");
        }
    }

    /// A chat line: at most one in 500 ms, an empty one is not sent.
    fn send_chat(&mut self, to: Option<i8>, text: &str) {
        let now = self.clock.now_ms();
        if self
            .last_chat_ms
            .is_some_and(|at| now.saturating_sub(at) < CHAT_GAP_MS)
        {
            return;
        }
        let to = to.map_or(protocol::ChatTo::All, protocol::ChatTo::Player);
        if let Link::Net(n) = &mut self.net {
            match n.handle.with(|core| core.chat(to, text)) {
                Ok(()) => self.last_chat_ms = Some(now),
                Err(why) => tracing::info!("chat refused: {why:?}"),
            }
        }
    }

    /// A field of the player `pick` finds in the shown world (a question's names).
    fn player_text(
        &self,
        pick: impl Fn(&protocol::PlayerView) -> bool,
        field: impl Fn(&protocol::PlayerView) -> String,
    ) -> Option<String> {
        let find = |w: &WorldView| w.players.iter().find(|p| pick(p)).map(&field);
        match &self.net {
            Link::Net(n) => n.handle.with(|core| core.shown().and_then(find)),
            Link::NetFailed(_) => None,
        }
    }

    /// The nick of the human at `player`.
    fn seat_nick(&self, player: i8) -> Option<String> {
        let Link::Net(n) = &self.net else {
            return None;
        };
        n.handle.with(|core| {
            core.seats()
                .iter()
                .find(|s| s.player == player)
                .and_then(|s| match &s.control {
                    protocol::SeatControl::Human { nick } => Some(nick.clone()),
                    _ => None,
                })
        })
    }

    fn i_am_host(&self) -> bool {
        let Link::Net(n) = &self.net else {
            return false;
        };
        n.handle.with(|core| {
            let me = core.attach_info().map(|a| a.player);
            core.seats().iter().any(|s| Some(s.player) == me && s.host)
        })
    }

    /// An order that is not the controller's (the city panel's): into the session or the core, as the link has
    /// it. A refusal (no connection, too many in flight) leaves nothing behind (NC-D5).
    fn send_command(&mut self, cmd: protocol::Command) {
        tracing::info!("command sent: {cmd:?}");
        match &mut self.net {
            Link::Net(n) => {
                if let Err(why) = n.handle.with(|core| core.order(cmd)) {
                    tracing::info!("order refused: {why:?}");
                }
            }
            Link::NetFailed(_) => {}
        }
    }

    /// What the player did on the screens.
    fn apply_ui(&mut self, actions: Vec<UiAction>) {
        for action in actions {
            // The screens of a server session (sign-in, lobby, room, notices) are the `NetUi`'s; the game's own
            // actions are the same on both links.
            if matches!(self.net, Link::Net(_)) && !is_game_action(&action) {
                self.apply_net_ui(vec![action]);
                continue;
            }
            match action {
                // The button is Enter; Shift+click is Shift+Enter.
                UiAction::EndTurn { force } => {
                    self.play_apply(|play, world| play.hotkey(world, HotKey::EndTurn { force }));
                }
                UiAction::ChooseBuild(i) => {
                    if let Some(cmd) = self.production.choose(i) {
                        self.send_command(cmd);
                        self.play.close_city_window();
                    }
                }
                UiAction::BuyBuild { index, faith } => {
                    if let Some(cmd) = self.production.buy(index, faith) {
                        self.send_command(cmd);
                    }
                }
                UiAction::CloseCity => {
                    self.production.close();
                    self.play.close_city_window();
                }
                UiAction::ToggleJournal => {}
                UiAction::JournalGoto { x, y } => {
                    self.apply_effect(UiEffect::CenterOn { x, y });
                }
                UiAction::Reload => self.reload_game(),
                UiAction::UnitAction(action) => self.unit_action(action),
                UiAction::LabelClicked {
                    target,
                    cell,
                    secondary,
                } => self.label_clicked(target, cell, secondary),
                UiAction::CityRangeAttack(city) => {
                    self.play_apply(|play, world| {
                        if world.status == protocol::TurnStatus::HumanTurn {
                            play.begin_city_range_attack(city)
                        } else {
                            Default::default()
                        }
                    });
                }
                UiAction::Confirm(yes) => match self.question.take() {
                    Some(Question::War { .. }) => {
                        self.play_apply(|play, _| play.answer_war(yes));
                    }
                    Some(Question::Disband { unit, .. }) if yes => {
                        self.play_apply(|play, world| {
                            play.unit_command(world, protocol::Command::UnitDisband { unit })
                        });
                    }
                    Some(Question::DeclareWar { team, .. }) if yes => {
                        self.send_command(protocol::Command::DeclareWar { team });
                    }
                    Some(Question::Kick { player, .. }) if yes => {
                        self.send_game_ctl(protocol::GameCtl::Kick { player });
                    }
                    Some(Question::LeaveGame { .. }) if yes => {
                        self.send_game_ctl(protocol::GameCtl::Leave);
                    }
                    _ => {}
                },
                // The orders of the players panel and the end-turn button (lobby-ui 4.7).
                UiAction::CancelEndTurn => self.send_command(protocol::Command::CancelEndTurn),
                UiAction::ProposePeace { to } => {
                    self.send_command(protocol::Command::ProposePeace { to });
                }
                UiAction::AnswerPeace { from, accept } => {
                    self.send_command(protocol::Command::AnswerPeace { from, accept });
                }
                UiAction::Pause(on) => self.send_game_ctl(protocol::GameCtl::Pause(on)),
                UiAction::DeclareWar { team } => {
                    let civ = self.player_text(|p| p.team == team, |p| p.civ_name.clone());
                    self.question = Some(Question::DeclareWar {
                        team,
                        civ: civ.unwrap_or_else(|| "this civilization".to_owned()),
                    });
                }
                UiAction::Kick { player } => {
                    let civ = self.player_text(|p| p.id == player, |p| p.civ_name.clone());
                    let nick = self.seat_nick(player);
                    self.question = Some(Question::Kick {
                        player,
                        nick: nick.unwrap_or_else(|| "this player".to_owned()),
                        civ: civ.unwrap_or_else(|| "their civilization".to_owned()),
                    });
                }
                // Defeated: no question, nothing is left to lose. (`Watch` is the `Hud`'s.)
                UiAction::DefeatLeave => self.send_game_ctl(protocol::GameCtl::Leave),
                UiAction::LeaveGame => {
                    let host = self.i_am_host();
                    self.question = Some(Question::LeaveGame { host });
                }
                UiAction::Chat { to, text } => self.send_chat(to, &text),
                UiAction::MinimapPointer { u, v, .. } => self.minimap_pointer(u, v),
                UiAction::SetQuality(p) => self.set_quality(p),
                UiAction::SetRenderScale(pct) => {
                    crate::gfx_prefs::store_render_scale(
                        &*self.gfx_kv,
                        render::render_scale::percent_to_scale(pct),
                    );
                    self.gfx_polled_ms = 0;
                }
                UiAction::SetShadowQuality(q) => {
                    crate::gfx_prefs::store_shadow_quality(&*self.gfx_kv, q);
                    self.gfx_polled_ms = 0;
                }
                // The multisampling is fixed when the renderer is made: kept now, in force at the next start.
                UiAction::SetMsaa(m) => crate::gfx_prefs::store_msaa(&*self.gfx_kv, m),
                UiAction::SetGfxPreset(p) => {
                    crate::gfx_prefs::store_preset(&*self.gfx_kv, p);
                    self.gfx_polled_ms = 0;
                }
                UiAction::SetTerrainDetail(d) => {
                    crate::gfx_prefs::store_terrain_detail(&*self.gfx_kv, d);
                    self.gfx_polled_ms = 0;
                }
                UiAction::ToggleStats(on) => self.config.stats = on,
                UiAction::SetPerfGroups(mask) => {
                    if let Some(r) = self.renderer.as_mut() {
                        r.set_disabled_groups(mask);
                    }
                }
                UiAction::SendPerfReport => self.send_perf_report(),
                // The screens of a server session (sign-in, lobby, room): `apply_net_ui`.
                _ => {}
            }
        }
    }

    /// The panel's "Send report": one snapshot with the device, the graphics settings and the place in the
    /// game. The web page posts it with the recent log lines (`report_sink`) and the answer comes back in
    /// `perf_report_answer`; the desktop writes it to a file.
    // SIMPLIFIED(SIMP-exe-1120): the desktop writes a file (no log lines) instead of posting to the server.
    fn send_perf_report(&mut self) {
        use crate::perf_report::{GameInfo, ReportContext, report_json};
        use interim_ui::screens::perf_panel::ReportState;
        let Some(renderer) = self.renderer.as_ref() else {
            return;
        };
        let at_ms = self.clock.now_ms();
        let settings = renderer.settings();
        let game = match &self.net {
            Link::Net(n) => n.handle.with(|core| GameInfo {
                game: core.attach_info().map(|a| a.game),
                player: core.attach_info().map(|a| i64::from(a.player)),
                turn: core.base().map(|v| i64::from(v.turn)),
                map: core
                    .shown()
                    .map(|w| (u32::from(w.map.width), u32::from(w.map.height))),
            }),
            Link::NetFailed(_) => GameInfo::default(),
        };
        let cx = ReportContext {
            at_ms,
            build: self.config.build.clone(),
            touch: self.config.touch_device,
            device: renderer.device_report(),
            preset: crate::test_port::quality_name(settings).to_owned(),
            settings: render::settings::KEYS
                .iter()
                .filter_map(|k| settings.get(k.name).map(|v| (k.name, v)))
                .collect(),
            game,
        };
        let json = report_json(&renderer.perf_snapshot(true), &cx).to_string();
        if let Some(sink) = &self.report_sink {
            self.perf_report = ReportState::Sending;
            let answer = Rc::clone(&self.perf_report_answer);
            sink(json, Box::new(move |r| *answer.borrow_mut() = Some(r)));
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let dir = crate::perf_report::log_dir();
            self.perf_report = match crate::perf_report::write_file(&dir, at_ms, &json) {
                Ok(path) => {
                    tracing::info!("performance report written to {}", path.display());
                    ReportState::Saved(path.display().to_string())
                }
                Err(e) => ReportState::Failed(format!("cannot write the report: {e}")),
            };
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.perf_report = ReportState::Failed("this page has no way to send reports".into());
        }
    }

    /// A press or drag on the minimap: the camera goes to that point (lobby-ui 4.8).
    fn minimap_pointer(&mut self, u: u16, v: u16) {
        if self.minimap_texture {
            if let Some(at) = self
                .renderer
                .as_ref()
                .and_then(|r| r.minimap_to_world(u, v))
            {
                self.game_camera.center_on(at);
            }
            return;
        }
        let size = match &self.net {
            Link::Net(n) => n
                .handle
                .with(|core| core.shown().map(|w| (w.map.width, w.map.height))),
            Link::NetFailed(_) => None,
        };
        if let Some((w, h)) = size {
            let (x, y) = minimap_image::cell_at(u, v, w, h);
            // The minimap moves the camera without the "look here" beam of `CenterOn`.
            self.game_camera
                .center_on(render::hexgrid::cell_center(x, y));
        }
    }

    /// Tells the renderer the render scale and the shadow quality the store holds when they differ from
    /// what it has (a few times a second). In force at once, no reload; the multisampling waits for the
    /// next start.
    fn poll_gfx_prefs(&mut self, now: u64) {
        if self.renderer.is_none()
            || !matches!(self.golden, GoldenState::Off)
            || now.saturating_sub(self.gfx_polled_ms) < GFX_POLL_MS
        {
            return;
        }
        self.gfx_polled_ms = now;
        let prefs = crate::gfx_prefs::GfxPrefs::read(&*self.gfx_kv, self.config.touch_device);
        if self.gfx_applied == Some(prefs) {
            return;
        }
        if let Some(r) = self.renderer.as_mut() {
            r.set_render_scale(prefs.render_scale);
            r.set_shadow_quality(prefs.shadow_quality);
            r.set_terrain_detail(prefs.terrain_detail);
            r.set_gfx_preset(render::gfx_choice::GfxPreset::index_of(prefs.preset()));
            self.gfx_applied = Some(prefs);
        }
    }

    /// A quality preset of the settings window: in force at once, kept for the next start (LU-D11).
    fn set_quality(&mut self, p: QualityPreset) {
        let settings = render::RenderSettings::preset(preset_of(p));
        if let Some(r) = self.renderer.as_mut() {
            r.set_settings(settings.clone());
        }
        self.settings_store.save(&settings);
    }

    /// A button of the unit panel: the ranged mode, a question before a disband, else the order.
    fn unit_action(&mut self, action: i16) {
        let Some(unit) = self.play.selected_unit() else {
            return;
        };
        match action {
            unit_panel::ACTION_RANGE_ATTACK => {
                self.play_apply(|play, world| play.toggle_range_attack(world));
            }
            unit_panel::ACTION_DISBAND => {
                let own_name = |w: &WorldView| {
                    w.units
                        .iter()
                        .find(|u| u.id == unit && u.owner == w.human)
                        .map(|u| u.name.clone())
                };
                let name = match &self.net {
                    Link::Net(n) => n.handle.with(|core| core.shown().and_then(own_name)),
                    Link::NetFailed(_) => None,
                };
                self.question = Some(Question::Disband {
                    unit,
                    name: name.unwrap_or_else(|| "unit".to_owned()),
                });
            }
            a => {
                if let Some(cmd) = unit_panel::command_for(a, unit) {
                    self.play_apply(|play, world| play.unit_command(world, cmd));
                }
            }
        }
    }

    /// A click on a flag or a banner: what a click on its plot does, except that a left click on an
    /// own unit's flag selects that unit and one on an own city's banner opens the city (outside
    /// the ranged mode, where every left click aims).
    fn label_clicked(
        &mut self,
        target: interim_ui::labels::LabelTarget,
        cell: (i32, i32),
        secondary: bool,
    ) {
        use interim_ui::labels::LabelTarget;
        self.play_apply(|play, world| {
            if secondary {
                return play.right_click(world, cell);
            }
            if play.mode() != crate::play::Mode::Normal {
                return play.left_click(world, cell);
            }
            let at = |x: i16, y: i16| (i32::from(x), i32::from(y)) == cell;
            match target {
                LabelTarget::Unit(id)
                    if world
                        .units
                        .iter()
                        .any(|u| u.id == id && u.owner == world.human && at(u.x, u.y)) =>
                {
                    play.select_unit(world, id)
                }
                LabelTarget::City(id)
                    if world
                        .cities
                        .iter()
                        .any(|c| c.id == id && c.owner == world.human && at(c.x, c.y)) =>
                {
                    play.open_city_window(world, id)
                }
                _ => play.left_click(world, cell),
            }
        });
    }

    /// Keeps the unit panel's actions and the cities' strike buttons in step with the selection and
    /// the newest `World`.
    fn sync_panels(&mut self) {
        match &mut self.net {
            Link::Net(n) if n.playing => {
                let (play, unit_panel, production) =
                    (&mut self.play, &mut self.unit_panel, &mut self.production);
                n.handle.with(|core| {
                    // The focus goes to the server when it changed (NC-D7); the contexts come back with
                    // the view.
                    if let Some(world) = core.shown() {
                        let plan = play.focus_plan(world);
                        core.set_focus(&plan);
                    }
                    let cx = core.contexts();
                    play.sync_contexts(&cx);
                    let selected = play.selected_unit();
                    unit_panel.sync(
                        selected,
                        selected.map_or(Ctx::Missing, |u| cx.unit(u)),
                        core.orders_enabled(),
                    );
                    let me = core.attach_info().map(|a| a.player);
                    unit_panel
                        .set_blocked(core.shown().and_then(|w| {
                            hud::order_block(w, core.turn_state(), core.seats(), me)
                        }));
                    if let Some(city) = production.city() {
                        production.sync(cx.city(city));
                    }
                });
            }
            Link::Net(_) | Link::NetFailed(_) => return,
        }
        // Esc (or the rivals' turn) dropped the war question in the controller.
        if matches!(self.question, Some(Question::War { .. })) && !self.play.war_question() {
            self.question = None;
        }
    }

    /// The error screen's Reload: the page reloads (web), or a fresh local host starts.
    fn reload_game(&mut self) {
        if let Some(reload) = &self.reload {
            reload();
            return;
        }
        self.play = PlayController::new();
        self.play.set_layered(matches!(self.net, Link::Net(_)));
        self.production.close();
        self.hud.reset();
    }

    /// Runs `f` on the controller with the newest local world, sends what it asks for and applies
    /// its UI effects. Does nothing without a local game.
    fn play_apply(&mut self, f: impl FnOnce(&mut PlayController, &WorldView) -> Outputs) {
        let effects = match &mut self.net {
            // The view the controls take: `base` plus the layer. Cloned, since the orders go into the core
            // while the controller reads it; this runs on input, not every frame.
            Link::Net(n) => {
                let play = &mut self.play;
                n.handle
                    .with(|core| {
                        let world = core.shown().cloned()?;
                        let out = f(play, &world);
                        Some(out.send(play, core))
                    })
                    .unwrap_or_default()
            }
            Link::NetFailed(_) => return,
        };
        for effect in effects {
            self.apply_effect(effect);
        }
    }

    /// Gives the controller what the server session reported in this frame: the orders' fates (a picture
    /// that came off brings the selection back, 4.4), the answers to its queries, a new game.
    fn pump_net_events(&mut self) {
        let events = match &mut self.net {
            Link::Net(n) => std::mem::take(&mut n.game_events),
            Link::NetFailed(_) => return,
        };
        let now = self.clock.now_ms();
        for ev in events {
            // The client halves of mods see the set, their messages and the game's attach and detach.
            if !self.hooks.is_empty() {
                let lock = self.hooks.net_event(&ev);
                self.apply_pointer_request(lock);
            }
            self.hud.on_game_event(&ev, now);
            // TEMP(T53): the answers to UI v2's orders and the game's attach and detach, for its next frame.
            if let Some(v2) = self.v2.as_mut() {
                v2.net_event(&ev);
            }
            // The game UIs follow their orders' fates (the classic UI's end of turn, live game 931).
            self.ui_host.net_event(&ev);
            match ev {
                ClientEvent::Attached { game, .. } => {
                    // A reconnect keeps the selection; another game starts afresh.
                    if let Link::Net(n) = &mut self.net
                        && n.game != Some(game)
                    {
                        n.clear_world |= n.world_up;
                        n.game = Some(game);
                        self.play = PlayController::new();
                        self.play.set_layered(true);
                        self.production.close();
                        self.question = None;
                    }
                }
                ClientEvent::Detached(_) => {
                    self.production.close();
                    self.play.close_city_window();
                    self.question = None;
                }
                ClientEvent::QueryResult { id, answer } => self.play.on_query_result(id, &answer),
                ClientEvent::OrderAnswered {
                    id,
                    outcome: protocol::CommandOutcome::Applied,
                } => self.play.on_order_applied(id),
                ClientEvent::OrderRolledBack { id, .. } => {
                    self.play_apply(|play, world| play.on_order_rolled_back(world, id));
                }
                _ => {}
            }
        }
        // What the mods sent since the last frame goes out as `ClientMsg::Mod`.
        if !self.hooks.is_empty()
            && let Link::Net(n) = &self.net
        {
            n.handle.with(|core| self.hooks.flush_net(core));
        }
    }

    /// The controller reads each new revision of the view the controls take: the selection goes on after an
    /// order without waiting for the answer, the city panel opens again when production still blocks the
    /// turn, the camera finds the first unit of a game (4.6).
    fn net_world_step(&mut self) {
        let Link::Net(n) = &mut self.net else {
            return;
        };
        if !n.playing {
            return;
        }
        let handle = n.handle.clone();
        let (play, production) = (&mut self.play, &mut self.production);
        let (world_rev, centered) = (&mut n.world_rev, &mut n.centered);
        let effects = handle.with(|core| {
            let game = core.attach_info().map(|a| a.game)?;
            let at = Some((game, core.revision()));
            if *world_rev == at {
                return None;
            }
            *world_rev = at;
            let world = core.shown()?;
            let mut outs = vec![play.on_world(world)];
            if *centered != Some(game) {
                *centered = Some(game);
                outs.push(play.hotkey(world, HotKey::Center));
            }
            if let Some(city) = production.reopen_for(world, true) {
                outs.push(play.open_city_window(world, city));
            }
            let mut effects = Vec::new();
            for out in outs {
                effects.extend(out.send(play, core));
            }
            Some(effects)
        });
        for effect in effects.into_iter().flatten() {
            self.apply_effect(effect);
        }
    }

    /// A game runs on this link: a server session with its first view in.
    fn in_game(&self) -> bool {
        match &self.net {
            Link::Net(n) => n.playing,
            Link::NetFailed(_) => false,
        }
    }

    /// The mouse is over a plot: the controller asks for the path of the selected unit there
    /// Not while the map is being dragged.
    fn hover_path(&mut self, now_ms: u64) {
        if self.showcase_mode || !self.in_game() || self.ui_host.has_game_ui() {
            return;
        }
        // Not through a panel or a window: only the map (and its flags) shows a path.
        let over_ui = self.interim.pointer_over_window();
        let cell = match self.map_controller.cursor() {
            Some(at) if !self.map_controller.is_dragging() && !over_ui => {
                let ray = self.map_controller.ray_at(at, &self.game_camera);
                self.picker.pick(&self.bridge, &ray).map(|p| p.cell)
            }
            _ => None,
        };
        self.tooltip_cell = self.dwell.update(cell, now_ms);
        self.play_apply(|play, world| play.hover(world, cell, now_ms));
    }

    fn apply_effect(&mut self, effect: UiEffect) {
        match effect {
            UiEffect::CenterOn { x, y } => {
                // The camera eases to the plot (a notification, the journal, the city list).
                self.game_camera
                    .look_at(render::hexgrid::cell_center(i32::from(x), i32::from(y)));
                // Where the original's Lua moves the view to a unit, a city or a notification's
                // plot it calls `Events.GameplayFX(x, y, -1)`: the "look here" beam (R5.12).
                if let Some(r) = self.renderer.as_mut() {
                    r.particles_mut().look_at(i32::from(x), i32::from(y));
                }
            }
            UiEffect::ToggleGrid => {
                if let Some(r) = self.renderer.as_mut() {
                    r.set_grid_visible(!r.grid_visible());
                }
            }
            // The interim city panel (the full city window is G2).
            UiEffect::OpenCity { city } => {
                tracing::info!("open city window {city}");
                self.production.open(city);
            }
            UiEffect::CloseCityWindow => {
                tracing::info!("close city window");
                self.production.close();
            }
            UiEffect::ConfirmWar { team, civ, ranged } => {
                tracing::info!("declare war on {civ} (team {team}, ranged {ranged})?");
                self.question = Some(Question::War { civ, ranged });
            }
        }
    }

    fn frame(&mut self) {
        render::perf::slow::begin_frame();
        self.frame_body();
        if let Some(slow) = render::perf::slow::end_frame() {
            tracing::warn!("{}", slow.line());
        }
    }

    fn frame_body(&mut self) {
        use render::perf::slow::scope;
        let now = self.clock.now_ms();
        self.ui_host.frame_started(self.clock.now_ms_f64());
        if let Some(r) = self.renderer.as_mut() {
            r.mark_frame_start();
        }
        if let Some(a) = self.assets.as_mut()
            && let Some(set) = self.client_data.update(a.loader_mut())
        {
            self.pending_cursors = Some(set);
        }
        let _slow1 = scope("app.tasks");
        self.tasks.run_pending(TASK_BUDGET_MS);
        drop(_slow1);
        {
            let _ram = ram_scope(RamTag::Net);
            let _slow2 = scope("app.net_update");
            self.net.update(now);
            drop(_slow2);
        }
        if let Link::Net(n) = &mut self.net
            && let Some(name) = n.milestone()
        {
            self.milestone(name, "server session phase reached");
        }
        {
            let _ram = ram_scope(RamTag::Sim);
            let _slow3 = scope("app.net_events");
            self.pump_net_events();
            drop(_slow3);
            let _slow4 = scope("app.net_world_step");
            self.net_world_step();
            drop(_slow4);
        }
        if let Some(assets) = self.assets.as_mut() {
            let _ram = ram_scope(RamTag::Assets);
            let _slow5 = scope("app.assets_update");
            assets.update();
            drop(_slow5);
        }
        self.settings_store.poll_saves();
        let _slow6 = scope("app.try_start_renderer");
        self.try_start_renderer();
        drop(_slow6);
        let _slow7 = scope("app.poll_renderer");
        self.poll_renderer();
        drop(_slow7);
        self.poll_gfx_prefs(now);
        if !matches!(self.golden, GoldenState::Off) {
            self.golden_frame(now);
            return;
        }
        let _slow8 = scope("app.hover_path");
        self.hover_path(now);
        drop(_slow8);
        let _slow9 = scope("app.sync_panels");
        self.sync_panels();
        drop(_slow9);

        // What the player did on the screens this frame; applied after the frame is drawn.
        let mut ui_actions = Vec::new();
        if let Some(renderer) = self.renderer.as_mut() {
            // The panel's numbers are taken 10 times a second, not every frame.
            // The switches of the panel are debug aids: closing it draws everything again.
            if !self.hud_on {
                self.perf_view = None;
                self.perf_report = Default::default();
                if renderer.disabled_groups() != 0 {
                    renderer.set_disabled_groups(0);
                }
            } else if self.perf_view.is_none() || now.saturating_sub(self.perf_view_at_ms) >= 100 {
                if let Some(answer) = self.perf_report_answer.borrow_mut().take() {
                    self.perf_report = match answer {
                        Ok(id) => interim_ui::screens::perf_panel::ReportState::Sent(id),
                        Err(why) => interim_ui::screens::perf_panel::ReportState::Failed(why),
                    };
                }
                self.perf_view = Some(interim_ui::screens::perf_panel::PerfView {
                    snapshot: Rc::new(renderer.perf_snapshot(true)),
                    report: self.perf_report.clone(),
                });
                self.perf_view_at_ms = now;
            }
            let _slow101 = scope("app.light_boot");
            self.light_boot
                .update(self.assets.as_mut().map(AssetsBoot::loader_mut), renderer);
            drop(_slow101);
            {
                let _ram = ram_scope(RamTag::Terrain);
                let _slow102 = scope("app.terrain_boot");
                self.terrain_boot.update(
                    &self.light_boot,
                    self.assets.as_mut().map(AssetsBoot::loader_mut),
                    renderer,
                );
                drop(_slow102);
            }
            let _slow103 = scope("app.water_boot");
            self.water_boot.update(
                &self.light_boot,
                self.assets.as_mut().map(AssetsBoot::loader_mut),
                renderer,
            );
            drop(_slow103);
            let t = (now - self.start_ms) as f32 / 1000.0;
            let mut scene = Scene::test(t);
            let dt = (now.saturating_sub(self.last_frame_ms)) as f32 / 1000.0;
            self.last_frame_ms = now;
            if self.showcase_mode {
                self.showcase.update(
                    now,
                    renderer,
                    self.assets.as_mut().map(AssetsBoot::loader_mut),
                    &mut self.camera,
                );
                if self.showcase.is_showing() {
                    // Hide the test sphere; it would hover over the unit.
                    scene.model_transform = Mat4::from_scale(Vec3::ZERO);
                }
            } else {
                let _slow104 = scope("app.terrain_assets");
                self.terrain_assets.update(
                    self.assets.as_mut().map(AssetsBoot::loader_mut),
                    renderer,
                    self.light_boot.map_may_draw(),
                );
                drop(_slow104);
                let _slow105 = scope("app.entity_assets");
                self.entity_assets
                    .update(self.assets.as_mut().map(AssetsBoot::loader_mut), renderer);
                drop(_slow105);
                let _slow106 = scope("app.fx_assets");
                self.fx_assets
                    .update(self.assets.as_mut().map(AssetsBoot::loader_mut), renderer);
                drop(_slow106);
                let _slow107 = scope("app.icon_assets");
                self.icon_assets
                    .update(self.assets.as_mut().map(AssetsBoot::loader_mut), renderer);
                drop(_slow107);
                // The game was left (lobby, room, starting another, a notice): the world of that game goes from
                // the renderer, the bridge and the controls; the next game sets them up from nothing.
                if let Link::Net(n) = &mut self.net
                    && n.world_up
                    && (n.clear_world
                        || matches!(
                            n.phase(),
                            ClientPhase::Lobby
                                | ClientPhase::Room
                                | ClientPhase::Starting
                                | ClientPhase::Notice(_)
                        ))
                {
                    tracing::info!("the game was left: the world is cleared");
                    renderer.clear_world();
                    self.bridge = crate::bridge::WorldBridge::new();
                    n.bridge_catalog = None;
                    n.bridge_rev = None;
                    n.share_rev = None;
                    n.world_rev = None;
                    n.centered = None;
                    // Another game attached over this one: it keeps its id; leaving for the lobby forgets it.
                    n.game = n.game.filter(|_| n.clear_world);
                    n.world_up = false;
                    n.clear_world = false;
                    self.play = PlayController::new();
                    self.play.set_layered(true);
                    self.production.close();
                    self.question = None;
                    // The screens and notes of that game, the tooltip, the trade route paths asked of it.
                    self.hud.game_left();
                    self.unit_panel = UnitPanel::default();
                    self.dwell = Default::default();
                    self.tooltip_cell = None;
                    self.trade_paths_at = f32::NEG_INFINITY;
                    // The original UI of that game goes too (kicked, defeated, "Leave game" of the interim screens).
                    self.ui_host.new_game();
                    if let Some(session) = self.session.as_mut() {
                        session.new_game();
                    }
                    if let Some(v2) = self.v2.as_mut() {
                        v2.new_game();
                    }
                }
                // The view of a server session reaches the bridge by its `ViewChange` (4.10): only what
                // changed since the last frame is converted.
                if let Link::Net(n) = &mut self.net
                    && n.playing
                {
                    let handle = n.handle.clone();
                    let (bridge, camera) = (&mut self.bridge, &mut self.game_camera);
                    let (bridge_catalog, bridge_rev) = (&mut n.bridge_catalog, &mut n.bridge_rev);
                    let share_rev = &mut n.share_rev;
                    let world_up = &mut n.world_up;
                    let _slow108 = scope("app.bridge_sync_flush");
                    handle.with(|core| {
                        let Some((game, hash)) =
                            core.attach_info().map(|a| (a.game, a.catalog.hash))
                        else {
                            return;
                        };
                        if *bridge_catalog != Some(hash)
                            && let Some(catalog) = core.catalog()
                        {
                            bridge.set_catalog(catalog);
                            *bridge_catalog = Some(hash);
                        }
                        let at = Some((game, core.revision()));
                        if *bridge_rev != at {
                            *bridge_rev = at;
                            let change = core.take_change();
                            if let Some(view) = core.shown() {
                                bridge.sync(view, core.basis(), &change, camera);
                                bridge.flush(renderer);
                                *world_up = true;
                                renderer.note_turn(view.turn);
                            }
                        }
                        // The hexes the mods let this seat draw without revealing them (mod API 1.2): the ground of
                        // the renderer only, never the view (the minimap, the tooltips and the UI see the view).
                        let share = core.terrain_share();
                        let key = Some((hash, share.rev()));
                        if *share_rev != key && core.shown().is_some() {
                            *share_rev = key;
                            renderer.set_ground_share(bridge.ground_share(share));
                        }
                    });
                    drop(_slow108);
                    // SIMPLIFIED(SIMP-exe-660): the trade route lines lie on the paths of the replica core
                    // (`Q_TRADE_PATHS`), the plots the caravans walk, read here instead of the exe's
                    // `TradeVisuals_NewRoute` pushes: asked often while a route of the view has none yet (the replica
                    // replays the route's creation after the view shows it), then every two seconds.
                    let every = if self.bridge.trade_routes_without_path() > 0 {
                        0.25
                    } else {
                        2.0
                    };
                    if t - self.trade_paths_at >= every
                        && let Some((paths, ids)) = self.ui_host.trade_paths()
                    {
                        self.trade_paths_at = t;
                        self.bridge.set_trade_paths(paths, ids);
                        self.bridge.flush(renderer);
                    }
                    // SIMPLIFIED(SIMP-exe-921): the popup route of the trade visuals, from the exe API's events.
                    let popup = self.ui_host.popup_route();
                    self.bridge.set_trade_popup(popup);
                    self.bridge.flush(renderer);
                }
                // Selection ring, reachable cells, shot targets and path of the game.
                renderer.set_time(t);
                let (mut shown, arrow) = match &self.net {
                    Link::Net(n) if n.playing => n.handle.with(|core| {
                        core.shown()
                            .map(|w| (self.play.highlights(w), self.play.attack_arrow(w)))
                            .unwrap_or_default()
                    }),
                    Link::Net(_) | Link::NetFailed(_) => Default::default(),
                };
                // The arrow of the ranged attack, to the shot target under the mouse.
                renderer.set_arrows(
                    arrow
                        .map(|(from, to)| render::world::arrows::ArrowSpec { from, to })
                        .into_iter()
                        .collect(),
                );
                // The cells go through the styles of `Highlights.xml` (the table of calls) once the
                // styles are in; before that B1's outlines stay.
                let styles = renderer.has_highlight_styles();
                if self.ui_host.has_game_ui() {
                    // The game UI owns the highlights, the path and the selection ring.
                    if let Some((path, selected)) =
                        self.ui_host.selection(renderer.selection_ring())
                    {
                        shown.path = path;
                        shown.selected = selected;
                    }
                } else if styles {
                    let ranged = matches!(self.play.mode(), crate::play::Mode::RangeAttack(_));
                    renderer.set_highlight_set(crate::play::highlight_calls::from_highlights(
                        &shown, ranged,
                    ));
                    shown.reachable.clear();
                    shown.targets.clear();
                } else {
                    renderer.set_highlight_set(Default::default());
                }
                renderer.set_highlights(shown);
                let fullscreen = self
                    .window
                    .as_ref()
                    .is_some_and(|w| w.fullscreen().is_some());
                self.map_controller.set_edge_scroll(fullscreen);
                self.map_controller.update(dt, &mut self.game_camera);
                // The hooks of mods move their camera (client-hooks C2). No hooks: nothing happens.
                if !self.hooks.is_empty() {
                    let screen = self.window.as_ref().map_or((0, 0, 1.0), |w| {
                        let s = w.inner_size();
                        (s.width, s.height, w.scale_factor() as f32)
                    });
                    let focus = self.game_camera.focus();
                    if self.hooks.camera().is_none() {
                        let cam = &self.game_camera;
                        let aspect = screen.0 as f32 / screen.1.max(1) as f32;
                        let f = cam.focus();
                        self.strategic_pose = Some(mod_client::StrategicCamera {
                            eye: cam.eye().to_array(),
                            look_at: [f.x, render::camera_params::FOCUS_HEIGHT, f.y],
                            yaw: 0.0,
                            pitch: cam.pitch(),
                            fov_y: cam.fov_y(aspect),
                        });
                        self.mod_hover = match self.map_controller.cursor() {
                            Some(at) if !self.map_controller.is_dragging() => {
                                let ray = self.map_controller.ray_at(at, &self.game_camera);
                                self.picker.pick(&self.bridge, &ray).map(|p| p.cell)
                            }
                            _ => None,
                        };
                    } else {
                        self.mod_hover = None;
                    }
                    let hooks = &mut self.hooks;
                    let view = (self.mod_hover, self.strategic_pose);
                    let lock = with_mod_info(
                        &self.net,
                        renderer,
                        (dt, now, focus),
                        screen,
                        view,
                        |info| hooks.frame(info),
                    );
                    if let (Some(want), Some(w)) = (lock, self.window.as_ref()) {
                        if want {
                            self.pointer.request(w);
                        } else {
                            self.pointer.release(w);
                        }
                    }
                    let req = self.hooks.take_requests();
                    apply_mod_requests(
                        renderer,
                        self.ui_sound.as_mut(),
                        req,
                        &mut self.mod_textures,
                    );
                    // A required mod that cannot run here (no pack): the game ends with the reason on the screen.
                    if let Some(text) = self.hooks.take_problem() {
                        if let Some(w) = self.window.as_ref() {
                            self.pointer.release(w);
                        }
                        self.net = Link::NetFailed(format!("Cannot play this game: {text}"));
                    }
                }
                // The virtual focus: while a mod holds the camera the strategic one stays where the mod is, so the
                // labels, the minimap, the picking, the sound of the map and the original UI look at its place.
                if let Some(cam) = self.hooks.camera() {
                    self.game_camera.center_on(cam.terrain_focus());
                }
            }
            self.ui.clear();
            let _slow_ui = scope("app.ui");
            if dt > 0.0 {
                let instant = 1.0 / dt;
                self.fps = if self.fps == 0.0 {
                    instant
                } else {
                    self.fps + (instant - self.fps) * 0.1
                };
            }
            if let Some(w) = &self.window {
                let s = w.inner_size();
                let size = (s.width, s.height);
                let scale = w.scale_factor() as f32;
                let time = (now - self.start_ms) as f64 / 1000.0;
                match &mut self.net {
                    // The server session: sign-in, lobby, room, loading (lobby-ui), and the game (4.6): the
                    // screens read the view the controls take (`base` plus the layer) and the contexts.
                    Link::Net(n) => {
                        let phase = n.phase();
                        let handle = n.handle.clone();
                        let ready =
                            self.bridge.terrain().is_some() && self.light_boot.map_may_draw();
                        self.loading_cursor = matches!(
                            phase,
                            ClientPhase::Connecting | ClientPhase::Starting | ClientPhase::Loading
                        ) || (matches!(phase, ClientPhase::Playing)
                            && !ready);
                        let ui = &mut n.ui;
                        let terrain_ready =
                            self.bridge.terrain().is_some() && self.light_boot.map_may_draw();
                        let (play, production, unit_panel, hud) = (
                            &self.play,
                            &self.production,
                            &self.unit_panel,
                            &mut self.hud,
                        );
                        let m2_on = self.ui_host.has_game_ui();
                        let (question, tooltip_cell, fps) =
                            (&self.question, self.tooltip_cell, self.fps);
                        let camera = &self.game_camera;
                        let (minimap, interim) = (&mut self.minimap, &self.interim);
                        let mut texture = false;
                        let mut views = handle.with(|core| {
                            let mut views = ui.screens(core, &phase, now);
                            if !matches!(phase, ClientPhase::Playing) {
                                return views;
                            }
                            if !terrain_ready {
                                // The map is not drawn: the last stage of the loading screen (4.6).
                                views.loading = Some(ui.drawing_view(core, now));
                                return views;
                            }
                            if m2_on {
                                // The original UI draws the game's panels, flags and banners.
                                return views;
                            }
                            let Some(world) = core.shown() else {
                                return views;
                            };
                            let catalog = core.catalog();
                            let cam =
                                label_camera(camera, size, scale, renderer.terrain_snapshot());
                            let labels = crate::map_labels::labels_view(
                                renderer.entity_labels(),
                                renderer.terrain_snapshot(),
                                &cam,
                                world,
                                catalog,
                                play.selected_unit(),
                            );
                            let finished = core.my_finished() == Some(true);
                            hud.net_play(
                                &hud::NetPlay {
                                    world,
                                    catalog,
                                    journal: core.journal(),
                                    finished,
                                    orders_enabled: core.orders_enabled(),
                                    turn: core.turn_state(),
                                    seats: core.seats(),
                                    chat: core.chat_lines(),
                                    me: core.attach_info().map(|a| a.player),
                                    nick: core.nick(),
                                    now_ms: now,
                                    server_ms: core.server_ms(now),
                                    timer_left_ms: core.timer_left_ms(now),
                                    production,
                                    fps,
                                    unit: unit_panel.view(world, catalog, play.mode()),
                                    labels: Some(labels),
                                    confirm: question.as_ref().map(Question::view),
                                    tooltip: tooltip_cell.filter(|_| question.is_none()).and_then(
                                        |cell| {
                                            crate::plot_tooltip::plot_tooltip(world, catalog, cell)
                                        },
                                    ),
                                },
                                &mut views,
                            );
                            let shown = minimap_view(
                                minimap,
                                interim,
                                renderer,
                                camera,
                                size,
                                Some((world, core.revision())),
                            );
                            texture = shown.as_ref().is_some_and(|s| s.1);
                            attach_minimap(&mut views, shown.map(|s| s.0));
                            views
                        });
                        self.minimap_texture = texture;
                        let session = (
                            self.config.server_url.as_str(),
                            handle.with(|core| core.rtt_ms()),
                            matches!(phase, ClientPhase::Playing),
                        );
                        views.perf = self.perf_view.clone();
                        views.settings = settings_view(
                            hud.settings_open(),
                            renderer,
                            crate::gfx_prefs::GfxPrefs::read(
                                &*self.gfx_kv,
                                self.config.touch_device,
                            ),
                            self.config.stats,
                            Some(session),
                        );
                        if m2_on && matches!(phase, ClientPhase::Playing) && terrain_ready {
                            // AC-S4: the game screen of the original UI has no egui panel of the game.
                            original_game_screen(&mut views);
                            if self.hooks.ui_suppressed() {
                                suppress_game_screens(&mut views);
                            }
                        }
                        // The page draws sign-in, lobby and room (web): egui skips them, the page's clicks
                        // join the actions of this frame.
                        if let (Some(d), Some(a)) = (self.dom_ui.as_mut(), self.assets.as_mut())
                            && let Some(art) = self.lobby_art.update(a.loader_mut())
                        {
                            d.send_art(&art);
                        }
                        let dom_hidden = self.dom_ui.as_mut().map(|d| d.present(&mut views));
                        let dom_actions = self.dom_ui.as_ref().map(|d| d.actions());
                        let forms = hud.forms();
                        let mut actions = Vec::new();
                        self.interim.run(size, scale, time, |ctx| {
                            actions = screens::show(ctx, &views, forms);
                        });
                        if let Some(hidden) = dom_hidden {
                            hidden.restore(&mut views);
                        }
                        actions.extend(dom_actions.into_iter().flatten());
                        ui_actions = hud.take_actions(actions);
                        if let Some(port) = self.test_port.as_mut() {
                            let mut snap = handle.with(|core| {
                                crate::test_port::net_snapshot(
                                    &views,
                                    &phase,
                                    core,
                                    self.play.selected_unit(),
                                )
                            });
                            snap.quality =
                                Some(crate::test_port::quality_name(renderer.settings()));
                            let live = renderer.terrain_live_stats();
                            snap.terrain_cells = Some((live.visible as u32, live.unpaged as u32));
                            (snap.map_units, snap.map_cities) = self.bridge.drawn();
                            port.publish(snap);
                        }
                    }
                    Link::NetFailed(text) => {
                        let views = screens::Screens {
                            fatal: Some(text.clone()),
                            ..screens::Screens::default()
                        };
                        let forms = self.hud.forms();
                        let mut actions = Vec::new();
                        self.interim.run(size, scale, time, |ctx| {
                            actions = screens::show(ctx, &views, forms);
                        });
                        ui_actions = actions;
                    }
                }
            }
            // A locked pointer has no cursor to show: the animated game cursor would bring it back.
            if let (Some(c), Some(w), false) = (&self.cursors, &self.window, self.pointer.engaged())
            {
                // The globe while the game loads and while the server runs the turn, else the arrow.
                let busy = self.loading_cursor || self.ui_host.turn_processing();
                // TEMP(T53): a running UI v2 skin's cursor over its panels; `auto` keeps the game UI's row.
                let row = self
                    .v2
                    .as_ref()
                    .and_then(|v| v.cursor_row())
                    .or_else(|| self.ui_host.cursor_row())
                    .unwrap_or(0);
                let pick = crate::cursor::pick(c.timing(), row, busy, now);
                if self.cursor_shown != Some(pick) {
                    w.set_cursor(c.get(pick).clone());
                    self.cursor_shown = Some(pick);
                }
            }
            // The game UI is not run while a mod owns the screen (client-hooks C3).
            if let (true, Some(w), false) = (
                self.ui_host.has_game_ui(),
                self.window.as_ref(),
                self.hooks.ui_suppressed(),
            ) {
                let handle = match &self.net {
                    Link::Net(n) => Some(n.handle.clone()),
                    Link::NetFailed(_) => None,
                };
                let s = w.inner_size();
                let seat = seat_info(handle.as_ref());
                // The game session first: the replica's network and pump, the game's texts.
                let step = match self.session.as_mut() {
                    Some(session) if s.width > 0 && s.height > 0 => {
                        session.frame(crate::session::SessionEnv {
                            now_ms: now,
                            loader: self.assets.as_mut().map(AssetsBoot::loader_mut),
                            net: handle.as_ref(),
                            clock: &self.clock,
                            seat: seat.as_ref().map_or(0, |s| s.local_id),
                            seat_known: seat.is_some(),
                            drain_events: !self.ui_host.dispatches_core_events(),
                        })
                    }
                    _ => crate::session::SessionStep::default(),
                };
                if let Some(session) = self.session.as_ref() {
                    // TEMP(T53): while a UI v2 skin runs, the classic layer draws into a list nobody submits.
                    let ui = match self.v2.as_mut().and_then(|v| v.classic_sink()) {
                        Some(scratch) => scratch,
                        None => &mut self.ui,
                    };
                    let mut cx = crate::ui_host::FrameCx {
                        now,
                        now_ms: self.clock.now_ms_f64(),
                        size: (s.width, s.height),
                        dpr: w.scale_factor() as f32,
                        net: handle.as_ref(),
                        seat,
                        session,
                        step: &step,
                        camera: &mut self.game_camera,
                        map_controller: &mut self.map_controller,
                        loader: self.assets.as_mut().map(AssetsBoot::loader_mut),
                        clock: &self.clock,
                        renderer,
                        stats: self.group_stats_sink.as_ref(),
                    };
                    self.ui_host.frame(&mut cx, ui);
                    // TEMP(T53): UI v2 reads the session's replica once the game is up; its scene is drawn this
                    // frame.
                    if let Some(v2) = self.v2.as_mut() {
                        let env = ui2_host::Env {
                            viewport_px: (s.width, s.height),
                            dpr: w.scale_factor() as f32,
                            ..ui2_host::Env::default()
                        };
                        let mut look = None;
                        if let Some(ui) = v2.frame(
                            session,
                            &step.steps,
                            handle.as_ref(),
                            env,
                            self.clock.now_ms_f64(),
                            &mut look,
                        ) {
                            renderer.prepare_ui2(ui.scene_mut());
                        }
                        if let Some((x, y)) = look {
                            self.game_camera.look_at(render::hexgrid::cell_center(x, y));
                        }
                    }
                }
            }
            // The HUD of the mods, after the game's own UI of the frame.
            if !self.hooks.is_empty() {
                let screen = self.window.as_ref().map_or((0, 0, 1.0), |w| {
                    let s = w.inner_size();
                    (s.width, s.height, w.scale_factor() as f32)
                });
                let focus = self.game_camera.focus();
                let (hooks, ui) = (&mut self.hooks, &mut self.ui);
                let view = (self.mod_hover, self.strategic_pose);
                with_mod_info(
                    &self.net,
                    renderer,
                    (dt, now, focus),
                    screen,
                    view,
                    |info| {
                        hooks.hud(info, ui);
                    },
                );
                let req = self.hooks.take_requests();
                apply_mod_requests(
                    renderer,
                    self.ui_sound.as_mut(),
                    req,
                    &mut self.mod_textures,
                );
            }
            drop(_slow_ui);
            let mod_camera = self
                .hooks
                .camera()
                .filter(|_| !self.showcase_mode)
                .map(render::ModCameraView);
            let camera: &dyn CameraView = if self.showcase_mode {
                &self.camera
            } else if let Some(view) = &mod_camera {
                view
            } else {
                &self.game_camera
            };
            // Where the world sound listens: the strategic camera's focus, or the mod camera's listener.
            let listener = self.hooks.camera().map_or(
                (self.game_camera.focus(), self.game_camera.height()),
                |c| {
                    let at = c.listener_pos();
                    (glam::Vec2::new(at.x, at.z), at.y)
                },
            );
            renderer.set_animation_time((now - self.start_ms) as f32 / 1000.0);
            let _slow109 = scope("app.render");
            if let Some(original) = self.original_ui.as_mut() {
                let size = self
                    .window
                    .as_ref()
                    .map_or((0, 0), |w| (w.inner_size().width, w.inner_size().height));
                let loader = self.assets.as_mut().map(AssetsBoot::loader_mut);
                original.set_texture_profile(&renderer.platform_profile());
                original.begin_frame(now, loader, size, &mut self.ui);
            }
            // The world soundscapes listen to the map under the camera while a game is on the screen (not on the loading
            // screen, whose cursor `loading_cursor` is).
            let playing = matches!(&self.net, Link::Net(n) if n.playing);
            let world_on = playing && !self.loading_cursor && !self.showcase_mode;
            // The units near the camera and their clips, for their sounds (re-b15 ITEM 4).
            let figures = if world_on {
                renderer.sounding_figures(listener.0, crate::ui_sound::UNIT_HEAR_RADIUS)
            } else {
                Vec::new()
            };
            let world = world_on
                .then(|| self.bridge.terrain())
                .flatten()
                .map(|(snapshot, _)| crate::world_sound::WorldInput {
                    snapshot,
                    focus: listener.0,
                    height: listener.1,
                    figures: &figures,
                });
            // The music: the main menu loop in the lobby, the game's music once its loading screen is gone.
            let music_scene = if !playing {
                audio::MusicScene::Menu
            } else if self.loading_cursor {
                audio::MusicScene::Loading
            } else {
                audio::MusicScene::Game
            };
            ui_sound_frame(
                self.ui_sound.as_mut(),
                (&self.ui_host, self.original_ui.as_ref()),
                world,
                music_scene,
                self.assets.as_mut(),
                self.group_stats_sink.as_ref(),
                &mut self.ui_sound_stats_ms,
                now,
            );
            let drawn = match renderer.render(&scene, camera, &self.ui, Some(&mut self.interim)) {
                Ok(()) => true,
                Err(e) => {
                    self.render_errors += 1;
                    // Log at powers of two only, to avoid flooding.
                    if self.render_errors.is_power_of_two() {
                        tracing::error!("frame error (total {}): {e}", self.render_errors);
                    }
                    false
                }
            };
            if let Some(original) = self.original_ui.as_mut() {
                original.end_frame(renderer);
            }
            // The game UI's textures the frame asked for, and its frame timing.
            self.ui_host.after_render(renderer, drawn);
            drop(_slow109);
            if drawn && !self.showcase_mode && self.bridge.terrain().is_some() {
                let mut report = |name, what: &str| {
                    report_milestone(
                        &mut self.milestones,
                        self.milestone_sink.as_ref(),
                        &*self.clock,
                        name,
                        what,
                    );
                };
                report(crate::MILESTONE_TERRAIN, "first frame with the terrain");
                if self.terrain_assets.textures_settled() {
                    report(crate::MILESTONE_MAP, "time to map (terrain with textures)");
                }
            }
            self.frames += 1;
            if now - self.fps_window_start_ms >= 10_000 {
                let secs = (now - self.fps_window_start_ms) as f64 / 1000.0;
                tracing::debug!("fps {:.1}", self.frames as f64 / secs);
                self.frames = 0;
                self.fps_window_start_ms = now;
            }
            self.stats_frames += 1;
            if (self.config.stats || self.hud_on) && now - self.stats_window_start_ms >= 1_000 {
                let secs = (now - self.stats_window_start_ms) as f64 / 1000.0;
                let fps = (self.stats_frames as f64 / secs) as f32;
                let frame = renderer.last_frame_stats();
                let perf = renderer.perf_snapshot(false);
                if self.config.stats {
                    tracing::info!("{}", perf.log_line());
                }
                if let Some(sink) = &self.perf_sink {
                    sink(&perf);
                }
                if self.config.stats {
                    tracing::info!(
                        "stats fps={:.1} draws={} triangles={}",
                        fps,
                        frame.draws,
                        frame.triangles
                    );
                    tracing::info!("{}", frame.pass_table());
                    tracing::info!("{}", frame.object_table());
                    if let Some(sink) = &self.stats_sink {
                        sink(fps, frame);
                    }
                }
                self.stats_frames = 0;
                self.stats_window_start_ms = now;
            }
        }
        self.apply_ui(ui_actions);
        self.apply_test_actions();
    }

    /// One frame of the `golden` page: fetches the files, then lets the driver load and draw until the
    /// frame is final, and hands the frame to the test port.
    fn golden_frame(&mut self, now: u64) {
        if let GoldenState::Fetching(fetch) = &mut self.golden {
            match fetch.poll() {
                None => return,
                Some(Err(e)) => return self.golden_end(Err(e)),
                Some(Ok(files)) => {
                    let made = crate::golden::GoldenSpec::parse(&files.spec).and_then(|spec| {
                        crate::golden::GoldenScene::load(&files)
                            .map(|scene| crate::golden::GoldenDriver::new(spec, scene))
                    });
                    match made {
                        Ok(driver) => self.golden = GoldenState::Running(Box::new(driver)),
                        Err(e) => return self.golden_end(Err(e)),
                    }
                }
            }
        }
        if self.render_failed {
            return self.golden_end(Err("the renderer could not be made".into()));
        }
        let (Some(renderer), GoldenState::Running(driver)) =
            (self.renderer.as_mut(), &mut self.golden)
        else {
            return;
        };
        let loader = self.assets.as_mut().map(AssetsBoot::loader_mut);
        match driver.frame(renderer, loader, now) {
            crate::golden::Progress::Running => {}
            crate::golden::Progress::Done(out) => self.golden_end(Ok(out)),
            crate::golden::Progress::Failed(e) => self.golden_end(Err(e)),
        }
    }

    fn golden_end(&mut self, result: Result<crate::golden::GoldenOutput, String>) {
        self.golden = GoldenState::Ended;
        match &result {
            Ok(out) => tracing::info!(capture = %out.capture, "golden frame done"),
            Err(e) => tracing::error!("golden frame failed: {e}"),
        }
        if let Some(port) = self.test_port.as_mut() {
            port.golden(result.as_ref().map_err(String::as_str));
        }
    }

    /// The scripted player's actions (`?test=1`): the same screen actions and map clicks a player
    /// makes.
    fn apply_test_actions(&mut self) {
        use crate::test_port::Dispatch;
        let Some(actions) = self.test_port.as_mut().map(|p| p.take_actions()) else {
            return;
        };
        for action in actions {
            let world = match &self.net {
                Link::Net(n) => n.handle.with(|core| core.shown().cloned()),
                Link::NetFailed(_) => None,
            };
            match action.dispatch(world.as_ref()) {
                Dispatch::Ui(a) => self.apply_ui(vec![a]),
                Dispatch::MapClick { cell, secondary } => self.play_apply(|play, world| {
                    if secondary {
                        play.right_click(world, cell)
                    } else {
                        play.left_click(world, cell)
                    }
                }),
                Dispatch::Fault(fault) => {
                    let now = self.clock.now_ms();
                    match &mut self.net {
                        Link::Net(n) => {
                            tracing::warn!("test port: injecting {fault:?}");
                            use crate::test_port::Fault;
                            match fault {
                                Fault::DropSocket => n.handle.with_client(|c| c.drop_sockets(now)),
                                Fault::CorruptView => {
                                    n.handle.with(|core| core.debug_corrupt_view())
                                }
                                Fault::BadMessage => n.handle.with_client(|c| c.bad_message(now)),
                                Fault::Silence { ms } => {
                                    n.handle.with_client(|c| c.silence(now, u64::from(ms)));
                                }
                                Fault::KillHost => {
                                    tracing::warn!("test port: there is no host to kill")
                                }
                            }
                        }
                        Link::NetFailed(_) => {}
                    }
                }
                Dispatch::QuickStart(params) => {
                    if let Link::Net(n) = &mut self.net {
                        n.ui.quick_start(params);
                    }
                }
                Dispatch::Skip => tracing::warn!("test action not possible now"),
            }
        }
    }
}

/// The camera of the flags and banners for a surface of `size` physical pixels.
fn label_camera(
    camera: &GameCamera,
    size: (u32, u32),
    scale: f32,
    terrain: Option<&render::terrain::TerrainSnapshot>,
) -> LabelCamera {
    LabelCamera {
        view_proj: camera.view_proj(size.0 as f32 / size.1.max(1) as f32),
        eye: camera.eye(),
        viewport: glam::Vec2::new(size.0 as f32, size.1 as f32),
        scale,
        wrap_width: terrain
            .filter(|t| t.wrap_x())
            .map(|t| render::hexgrid::HEX_WIDTH * t.width() as f32),
    }
}

/// The actions of the game's own screens, the same on a local game and a server session. The rest of the
/// `UiAction`s belong to the screens of the session (sign-in, lobby, room, notices).
fn preset_of(p: QualityPreset) -> render::Preset {
    match p {
        QualityPreset::Low => render::Preset::Low,
        QualityPreset::Medium => render::Preset::Medium,
        QualityPreset::High => render::Preset::High,
    }
}

/// The panel's source this frame and whether it is the renderer's texture. The texture is shown when
/// the renderer can turn a click into a point of the world; else the stand-in picture of the view (SIMP-060).
fn minimap_view(
    builder: &mut MinimapBuilder,
    interim: &InterimUi,
    renderer: &Renderer,
    camera: &GameCamera,
    size: (u32, u32),
    world: Option<(&WorldView, u64)>,
) -> Option<(MinimapView, bool)> {
    if let Some(source) = interim.minimap_source()
        && renderer.minimap_to_world(32_768, 32_768).is_some()
    {
        let view = MinimapView {
            source,
            camera_frame: None,
        };
        return Some((view, true));
    }
    let (world, revision) = world?;
    let image = builder.update(world, revision)?.clone();
    let aspect = size.0 as f32 / size.1.max(1) as f32;
    let view = MinimapView {
        source: MinimapSource::Image(image),
        camera_frame: minimap_image::camera_frame(
            camera,
            aspect,
            world.map.width,
            world.map.height,
        ),
    };
    Some((view, false))
}

/// The minimap panel stands in the bottom left corner; the journal window goes above it.
/// AC-S4 (WO-K): on the game screen of the original UI egui draws only the B3 performance panel (and, while the
/// line to the server is down, the "No connection" strip: a diagnostic, not a game panel). Every interim game panel
/// (flags, banners, unit and city panels, top bar, journal and chat, players, end turn, minimap, settings, questions,
/// tooltip, defeat) belongs to the original UI there. The loading screens, the lobby and the room are not the game
/// screen and stay as they are.
fn original_game_screen(views: &mut Screens) {
    *views = Screens {
        perf: views.perf.take(),
        conn: views.conn.take(),
        // Opened from the performance panel (the original UI has no screen for it).
        settings: views.settings.take(),
        ..Screens::default()
    };
}

/// What stays of the egui screens of the game while a mod owns the screen: the performance panel (F3) only.
fn suppress_game_screens(views: &mut Screens) {
    views.conn = None;
    views.settings = None;
}

fn attach_minimap(views: &mut Screens, view: Option<MinimapView>) {
    if let Some(j) = views.journal.as_mut() {
        j.above_minimap = view.is_some();
    }
    views.minimap = view;
}

/// The settings window while it is open. `session`: the server address, the round trip time and
/// whether a game is running, for a server session; `None` for the local game.
fn settings_view(
    open: bool,
    renderer: &Renderer,
    prefs: crate::gfx_prefs::GfxPrefs,
    stats: bool,
    session: Option<(&str, Option<u64>, bool)>,
) -> Option<SettingsView> {
    if !open {
        return None;
    }
    let preset = render::Preset::ALL
        .into_iter()
        .find(|p| render::RenderSettings::preset(*p) == *renderer.settings());
    let quality = preset.map(|p| match p {
        render::Preset::Low => QualityPreset::Low,
        render::Preset::Medium => QualityPreset::Medium,
        render::Preset::High => QualityPreset::High,
    });
    let mut info = Vec::new();
    if let Some((url, rtt, _)) = session {
        info.push(("Server".to_owned(), url.to_owned()));
        info.push((
            "Round trip".to_owned(),
            rtt.map_or_else(|| "unknown".to_owned(), |ms| format!("{ms} ms")),
        ));
    }
    info.push((
        "Client build".to_owned(),
        env!("CARGO_PKG_VERSION").to_owned(),
    ));
    Some(SettingsView {
        quality,
        custom: preset.is_none(),
        render_scale_pct: render::render_scale::scale_to_percent(renderer.render_scale()),
        shadow_quality: renderer.shadow_quality(),
        // The preset and the multisampling are the stored choices: the samples change at the next start.
        gfx_preset: prefs.preset(),
        msaa: prefs.msaa,
        msaa_in_force: render::gfx_choice::Msaa::from_samples(renderer.msaa_in_force()),
        terrain_detail: renderer.terrain_detail(),
        stats,
        info,
        can_leave: session.is_some_and(|(_, _, playing)| playing),
        can_sign_out: session.is_some(),
    })
}

fn is_game_action(a: &UiAction) -> bool {
    matches!(
        a,
        UiAction::EndTurn { .. }
            | UiAction::ChooseBuild(_)
            | UiAction::BuyBuild { .. }
            | UiAction::CloseCity
            | UiAction::ToggleJournal
            | UiAction::JournalGoto { .. }
            | UiAction::UnitAction(_)
            | UiAction::LabelClicked { .. }
            | UiAction::CityRangeAttack(_)
            | UiAction::Confirm(_)
            | UiAction::CancelEndTurn
            | UiAction::Chat { .. }
            | UiAction::DeclareWar { .. }
            | UiAction::ProposePeace { .. }
            | UiAction::AnswerPeace { .. }
            | UiAction::Kick { .. }
            | UiAction::Pause(_)
            | UiAction::LeaveGame
            | UiAction::DefeatWatch
            | UiAction::DefeatLeave
            | UiAction::TogglePlayers
            | UiAction::LogTab(_)
            | UiAction::ToggleSettings
            | UiAction::SetQuality(_)
            | UiAction::SetRenderScale(_)
            | UiAction::SetShadowQuality(_)
            | UiAction::SetMsaa(_)
            | UiAction::SetGfxPreset(_)
            | UiAction::SetTerrainDetail(_)
            | UiAction::ToggleStats(_)
            | UiAction::SetPerfGroups(_)
            | UiAction::SendPerfReport
            | UiAction::MinimapPointer { .. }
    )
}

/// Logs `name` and hands it to `sink` unless `done` already has it.
fn report_milestone(
    done: &mut Vec<&'static str>,
    sink: Option<&crate::MilestoneSink>,
    clock: &dyn platform::Clock,
    name: &'static str,
    what: &str,
) {
    if done.contains(&name) {
        return;
    }
    done.push(name);
    let at = clock.now_ms();
    tracing::info!("{what}: {at} ms after the page start ({name})");
    if let Some(sink) = sink {
        sink(name, at as f64);
    }
}

/// The G key goes down. The `play-ui` command takes this over later.
fn is_grid_toggle(ev: &platform::InputEvent) -> bool {
    matches!(
        ev,
        platform::InputEvent::Key(k)
            if k.pressed
                && !k.repeat
                && matches!(&k.key, platform::Key::Character(c) if c.eq_ignore_ascii_case("g"))
    )
}

/// What the UI layer sees of `ev` while the map is (`dragging`) or is not being dragged: the left-button release
/// that ends a drag goes off-screen, so it clicks no control but still clears the layer's button state.
pub(crate) fn ui_event_after_drag(
    ev: &platform::InputEvent,
    dragging: bool,
) -> platform::InputEvent {
    use platform::{InputEvent, MouseButton, PointerPhase};
    match ev {
        InputEvent::Pointer(p)
            if dragging
                && p.phase == PointerPhase::Up
                && p.button.is_none_or(|b| b == MouseButton::Left) =>
        {
            let mut off = p.clone();
            off.position = platform::PhysicalPosition { x: -1.0, y: -1.0 };
            InputEvent::Pointer(off)
        }
        _ => ev.clone(),
    }
}

/// A pointer release, cancel or leave: the end of a press, which the map must see even when the UI took it.
pub(crate) fn ends_press(ev: &platform::InputEvent) -> bool {
    use platform::PointerPhase as P;
    matches!(ev, platform::InputEvent::Pointer(p) if matches!(p.phase, P::Up | P::Cancel | P::Left))
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            self.create_window(event_loop);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let Some(w) = &self.window {
            self.long_press.set_scale(w.scale_factor());
            self.hud_hold.set_scale(w.scale_factor());
        }
        if matches!(event, WindowEvent::Focused(false))
            && let Some(w) = &self.window
        {
            self.pointer.focus_lost(w);
        }
        for ev in self.input.translate(&event) {
            self.hud_hold.observe(&ev, self.clock.now_ms());
            for ev in self.long_press.filter(ev, self.clock.now_ms()) {
                self.on_input(ev);
            }
        }
        match event {
            WindowEvent::CloseRequested => {
                tracing::info!("window closed");
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let (Some(r), Some(w)) = (self.renderer.as_mut(), &self.window) {
                    r.resize((size.width, size.height), w.scale_factor() as f32);
                }
            }
            WindowEvent::RedrawRequested => self.frame(),
            _ => {}
        }
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        // The relative motion of the mouse, only while the pointer lock is engaged (never otherwise).
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event
            && let Some(ev) = self.pointer.motion(dx, dy)
        {
            self.on_input(ev);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // The cursors of the asset set, once they have arrived (custom cursors are made on the event loop).
        if let Some(set) = self.pending_cursors.take() {
            self.cursors = crate::cursor::Cursors::new(event_loop, &set);
            self.cursor_shown = None;
        }
        // The network does not wait for a redraw: a minimised window draws no frames (NC-D9, 4.8).
        if let Link::Net(n) = &self.net
            && n.started
        {
            n.handle.pump(self.clock.now_ms());
        }
        // A key press held for its modifiers goes out at the end of the batch.
        if let Some(ev) = self.input.flush() {
            self.on_input(ev);
        }
        // The pointer lock: the browser may have ended it (Esc); a change reaches the input as an event.
        if let Some(w) = &self.window
            && let Some(ev) = self.pointer.poll(w)
        {
            self.on_input(ev);
        }
        // A finger held still presses the right button; the lift releases it (touch_press.rs).
        for ev in self.long_press.poll(self.clock.now_ms()) {
            self.on_input(ev);
        }
        // Three fingers held still: the performance panel, the touch F3 (touch_hud.rs).
        if self.hud_hold.poll(self.clock.now_ms()) {
            self.hud_on = !self.hud_on;
        }
        // Continuous redraw; vsync (native) or rAF (web) paces it.
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

impl App {
    /// The game UI sees what egui did not take: the active frontend's input; what it leaves moves the camera.
    fn game_ui_input(&mut self, ev: platform::InputEvent) {
        // TEMP(T53): while a UI v2 skin runs it gets the input first and the classic layer gets none.
        let now = self.clock.now_ms_f64();
        if let Some(outcome) = self.v2.as_mut().and_then(|v| v.input(&ev, now)) {
            self.v2_input(ev, outcome);
            return;
        }
        let mut cx = crate::ui_host::InputCx {
            now_ms: now,
            camera: &mut self.game_camera,
            map_controller: &mut self.map_controller,
            picker: &mut self.picker,
            bridge: &self.bridge,
            hooks: &mut self.hooks,
            renderer: self.renderer.as_ref(),
            pointer_lock: None,
        };
        self.ui_host.input(&mut cx, ev);
        let lock = cx.pointer_lock;
        self.apply_pointer_request(lock);
    }
    /// TEMP(T53): an event a running UI v2 skin looked at first. What it passed on goes to the mods' unclaimed
    /// input, then the camera; a click the picker turns into a plot is the skin's `map.click`. A press on the skin's
    /// minimap moves the camera like the classic minimap panel.
    fn v2_input(&mut self, ev: platform::InputEvent, outcome: ui2_host::InputOutcome) {
        match outcome {
            ui2_host::InputOutcome::Consumed => {}
            ui2_host::InputOutcome::MinimapClick { u, v } => {
                let (u, v) = (
                    crate::ui_host::v2::minimap_share(u),
                    crate::ui_host::v2::minimap_share(v),
                );
                if let Some(at) = self
                    .renderer
                    .as_ref()
                    .and_then(|r| r.minimap_to_world(u, v))
                {
                    self.game_camera.center_on(at);
                }
            }
            ui2_host::InputOutcome::PassToWorld => {
                if !self.hooks.is_empty() {
                    let claim = self.hooks.unclaimed_input(&ev);
                    self.apply_pointer_request(claim.pointer_lock);
                    if claim.taken {
                        return;
                    }
                }
                self.map_controller.handle(&ev, &mut self.game_camera);
                let clicks = [
                    (self.map_controller.take_click(), ui2_host::MapButton::Left),
                    (
                        self.map_controller.take_right_click(),
                        ui2_host::MapButton::Right,
                    ),
                ];
                for (at, button) in clicks {
                    let Some(at) = at else { continue };
                    let ray = self.map_controller.ray_at(at, &self.game_camera);
                    if let Some(p) = self.picker.pick(&self.bridge, &ray)
                        && let Some(v2) = self.v2.as_mut()
                    {
                        v2.map_click(p.cell, button);
                    }
                }
            }
        }
    }

    fn original_ui_takes(&mut self, ev: &platform::InputEvent) -> bool {
        self.original_ui
            .as_mut()
            .is_some_and(|original| original.handle_input(ev))
    }

    /// A press is a user gesture: the browser lets the sound output start (U-D7).
    fn note_gesture(&mut self, ev: &platform::InputEvent) {
        let press = match ev {
            platform::InputEvent::Pointer(p) => p.phase == platform::PointerPhase::Down,
            platform::InputEvent::Key(k) => k.pressed,
            _ => false,
        };
        if press && let Some(snd) = self.ui_sound.as_mut() {
            snd.gesture(self.clock.now_ms());
        }
    }

    /// Takes or gives back the pointer as a hook asked (`HookCtl::request_pointer_lock`).
    fn apply_pointer_request(&mut self, want: Option<bool>) {
        let (Some(want), Some(w)) = (want, self.window.as_ref()) else {
            return;
        };
        if want {
            let status = self.pointer.request(w);
            tracing::info!(?status, "pointer lock requested");
        } else {
            self.pointer.release(w);
        }
    }

    /// One input event: egui first; what it does not consume goes to the map or the showcase.
    fn on_input(&mut self, ev: platform::InputEvent) {
        // The hooks of mods see the event first (client-hooks I1); a taken event goes no further. No hooks: no-op.
        if !self.hooks.is_empty() {
            let claim = self.hooks.claim_input(&ev);
            self.apply_pointer_request(claim.pointer_lock);
            if claim.taken {
                self.note_gesture(&ev);
                return;
            }
        }
        self.note_gesture(&ev);
        // F3: the performance panel.
        if let platform::InputEvent::Key(k) = &ev
            && k.pressed
            && !k.repeat
            && k.key == platform::Key::Named(platform::NamedKey::F3)
        {
            self.hud_on = !self.hud_on;
            return;
        }
        if self.ui_host.has_game_ui() && !self.showcase_mode {
            // A mod that owns the screen (client-hooks C3) has hidden the original UI: it gets no input either.
            if self.interim.handle_input(&ev) {
                if !self.hooks.ui_suppressed() {
                    self.game_ui_input(ev);
                } else if !self.hooks.is_empty() {
                    // The original UI is hidden: the keys it would have seen are the mods' to bind.
                    let claim = self.hooks.unclaimed_input(&ev);
                    self.apply_pointer_request(claim.pointer_lock);
                }
            }
            return;
        }
        if self.interim.handle_input(&ev) && !self.original_ui_takes(&ev) {
            if self.showcase_mode {
                self.showcase.handle_input(&ev);
                self.controller.handle(&ev, &mut self.camera);
            } else {
                if !self.hooks.is_empty() && self.in_game() {
                    let claim = self.hooks.unclaimed_input(&ev);
                    self.apply_pointer_request(claim.pointer_lock);
                    if claim.taken {
                        return;
                    }
                }
                if let platform::InputEvent::Key(k) = &ev {
                    let esc =
                        k.pressed && k.key == platform::Key::Named(platform::NamedKey::Escape);
                    let chat_key = k.pressed
                        && !k.repeat
                        && !k.modifiers.ctrl
                        && !k.modifiers.alt
                        && !k.modifiers.meta
                        && matches!(&k.key, platform::Key::Character(c) if c.eq_ignore_ascii_case("t"));
                    if esc
                        && matches!(
                            self.question,
                            Some(
                                Question::Disband { .. }
                                    | Question::DeclareWar { .. }
                                    | Question::Kick { .. }
                                    | Question::LeaveGame { .. }
                            )
                        )
                    {
                        // Esc answers the open question "no" first.
                        self.question = None;
                    } else if chat_key && matches!(self.net, Link::Net(_)) && self.in_game() {
                        // T: the chat tab and its input (LU-D9).
                        self.hud.focus_chat();
                    } else if self.in_game() {
                        self.play_apply(|play, world| play.key(world, k));
                    } else if is_grid_toggle(&ev)
                        && let Some(r) = self.renderer.as_mut()
                    {
                        r.set_grid_visible(!r.grid_visible());
                    }
                }
                self.map_controller.handle(&ev, &mut self.game_camera);
                if let Some(at) = self.map_controller.take_click() {
                    let ray = self.map_controller.ray_at(at, &self.game_camera);
                    let picked = self.picker.pick(&self.bridge, &ray);
                    tracing::info!(
                        "click at ({:.0}, {:.0}) picked {:?}",
                        at.x,
                        at.y,
                        picked.map(|p| p.cell)
                    );
                    if let Some(p) = picked {
                        self.play_apply(|play, world| play.left_click(world, p.cell));
                    }
                }
                if let Some(at) = self.map_controller.take_right_click() {
                    let ray = self.map_controller.ray_at(at, &self.game_camera);
                    let picked = self.picker.pick(&self.bridge, &ray);
                    tracing::info!(
                        "right click at ({:.0}, {:.0}) picked {:?}",
                        at.x,
                        at.y,
                        picked.map(|p| p.cell)
                    );
                    if let Some(p) = picked {
                        self.play_apply(|play, world| play.right_click(world, p.cell));
                    }
                }
            }
        }
    }
}

/// Runs `f` with what the mods can see of the frame: the replica of the game (when a game is on and the core is free),
/// the terrain sampler, the column grid of the cutout mask and the renderer's `ModGpu`.
fn with_mod_info<R>(
    net: &Link,
    renderer: &Renderer,
    (dt, now_ms, focus): (f32, u64, glam::Vec2),
    screen: (u32, u32, f32),
    (hover, strategic): (Option<(i32, i32)>, Option<mod_client::StrategicCamera>),
    f: impl FnOnce(&crate::mod_hooks::FrameInfo<'_>) -> R,
) -> R {
    fn build<'a>(
        renderer: &'a Renderer,
        world: Option<&'a dyn mod_client::ModWorld>,
        (dt, now_ms, focus): (f32, u64, glam::Vec2),
        screen: (u32, u32, f32),
        (hover, strategic): (Option<(i32, i32)>, Option<mod_client::StrategicCamera>),
    ) -> crate::mod_hooks::FrameInfo<'a> {
        crate::mod_hooks::FrameInfo {
            dt,
            now_ms,
            focus,
            terrain: renderer.terrain_system().sampler(),
            world,
            cutout_grid: renderer.cutout_grid(),
            gpu: Some(renderer.mod_gpu()),
            screen,
            hover,
            strategic,
        }
    }
    let time = (dt, now_ms, focus);
    let mut f = Some(f);
    let seen = match net {
        Link::Net(n) if n.playing => n.handle.peek(|core| {
            let run = f.take()?;
            Some(match core.shown() {
                Some(view) => {
                    let world = crate::mod_host::ReplicaWorld::new(view, core.revision());
                    run(&build(
                        renderer,
                        Some(&world),
                        time,
                        screen,
                        (hover, strategic),
                    ))
                }
                None => run(&build(renderer, None, time, screen, (hover, strategic))),
            })
        }),
        _ => None,
    };
    match seen.flatten() {
        Some(r) => r,
        // No game, or the core was in use: the mods see no world this frame.
        None => (f.take().expect("not consumed"))(&build(
            renderer,
            None,
            time,
            screen,
            (hover, strategic),
        )),
    }
}

/// Carries out what the mods asked of the renderer and the sound output (client-hooks, T36): render systems,
/// cutout writes, HUD textures, sounds. Textures stay in `kept`; one the cache evicted is put back.
fn apply_mod_requests(
    renderer: &mut Renderer,
    sound: Option<&mut crate::ui_sound::UiSound>,
    req: crate::mod_hooks::Requests,
    kept: &mut Vec<(draw_list::TextureId, render::UiTextureData)>,
) {
    for system in req.systems {
        if let Err(e) = renderer.add_mod_system(system) {
            tracing::warn!("a render system of a mod was refused: {e}");
        }
    }
    for cmd in req.cutouts {
        match cmd {
            crate::mod_hooks::CutoutCmd::Set(region, bytes) => {
                if let Err(e) = renderer.set_cutout_mask(region, &bytes) {
                    tracing::warn!("a cutout mask of a mod was refused: {e}");
                }
            }
            crate::mod_hooks::CutoutCmd::Clear => renderer.clear_cutout_mask(),
        }
    }
    for (id, data) in req.textures {
        if let Err(e) = renderer.ui_textures_mut().upload(id, &data) {
            tracing::warn!(texture = id.0, "a HUD texture of a mod was refused: {e}");
            continue;
        }
        kept.retain(|(k, _)| *k != id);
        kept.push((id, data));
    }
    for (id, data) in kept.iter() {
        if !renderer.ui_textures_mut().contains(*id) {
            let _ = renderer.ui_textures_mut().upload(*id, data);
        }
    }
    if let Some(snd) = sound {
        if let Some(m) = req.mute_world {
            snd.set_world_muted(m);
        }
        for s in req.sounds {
            if let Some(wav) = s.pack.get(&s.sound) {
                snd.play_mod_sound(&s.key, wav, s.volume, s.pitch);
            }
        }
    }
}

/// This client's seat as the exe API tells it (`Matchmaking.*`), once the attach named it.
fn seat_info(handle: Option<&NetHandle>) -> Option<exe_api::SeatInfo> {
    handle.and_then(|h| {
        h.peek(|c| {
            c.attach_info().map(|a| {
                // WO-C (K7): the host is the host seat of the game; SIMPLIFIED(SIMP-exe-135): every other online
                // human has the line's round trip as its ping (the server is the only peer).
                let seats = c.seats();
                let rtt = c.rtt_ms().map_or(0, |r| r as i32);
                exe_api::SeatInfo {
                    nick: c.nick().to_owned(),
                    local_id: i32::from(a.player),
                    host_id: seats
                        .iter()
                        .find(|s| s.host)
                        .map_or(i32::from(a.player), |s| i32::from(s.player)),
                    pings_ms: seats
                        .iter()
                        .filter(|s| {
                            s.player != a.player
                                && matches!(s.control, protocol::SeatControl::Human { .. })
                                && matches!(s.conn, protocol::Conn::Online)
                        })
                        .map(|s| (i32::from(s.player), rtt))
                        .collect(),
                }
            })
        })
        .flatten()
    })
}
/// The sounds of the original UI: the button events of the layer and the scripts the Lua asked for go to the engine;
/// the counters go to `__civ5stats.audio` about four times a second. `scene` is where the player is, for the music.
#[allow(clippy::too_many_arguments)]
fn ui_sound_frame(
    snd: Option<&mut crate::ui_sound::UiSound>,
    uis: (
        &crate::ui_host::UiHost,
        Option<&crate::original_ui::OriginalUi>,
    ),
    world: Option<crate::world_sound::WorldInput<'_>>,
    scene: audio::MusicScene,
    assets: Option<&mut AssetsBoot>,
    sink: Option<&crate::GroupStatsSink>,
    last_stats_ms: &mut u64,
    now: u64,
) {
    let Some(snd) = snd else {
        return;
    };
    let game = uis.0.has_game_ui().then(|| uis.0.sounds());
    // WO-G (K20), re-b15 ITEM 8: each volume knob of the options menu is the gain of its own bus.
    if let Some(Some(src)) = game {
        for (k, v) in src.take_knobs() {
            snd.set_knob(&k, v);
        }
    }
    let (buttons, scripts) = match (game, uis.1) {
        (Some(Some(src)), _) => (src.take_button_sounds(), src.take_sound_scripts()),
        // A game UI without sounds of its own: the world and the music go on.
        (Some(None), _) => (Vec::new(), Vec::new()),
        (None, Some(o)) => (
            o.layer()
                .map(|l| l.take_button_sounds())
                .unwrap_or_default(),
            Vec::new(),
        ),
        (None, None) => return,
    };
    let Some(loader) = assets.map(AssetsBoot::loader_mut) else {
        return;
    };
    snd.world(now, loader, world.as_ref());
    snd.update(now, loader, buttons, scripts);
    let src = game.flatten();
    let events = src.map(|s| s.take_music_events()).unwrap_or_default();
    snd.music(
        now,
        loader,
        scene,
        &|| src.and_then(|s| s.music_state()),
        events,
    );
    if now.saturating_sub(*last_stats_ms) >= 250
        && let Some(sink) = sink
    {
        *last_stats_ms = now;
        let mut pairs = snd.stats().as_pairs();
        pairs.push(("volume".to_owned(), f64::from(snd.volume())));
        sink("audio", &pairs);
    }
}
#[cfg(test)]
mod original_game_screen_tests {
    use super::*;
    use interim_ui::screens::{
        confirm::ConfirmView, conn::ConnView, end_turn::EndTurnView, journal::JournalView,
        perf_panel::PerfView, players::PlayersView, settings::SettingsView, tooltip::TooltipView,
        top_bar::TopBarView, unit_panel::UnitPanelView,
    };

    #[test]
    fn the_original_game_screen_keeps_only_the_perf_panel_the_settings_it_opens_and_the_connection_strip()
     {
        let mut views = Screens {
            labels: Some(Default::default()),
            unit: Some(UnitPanelView::default()),
            confirm: Some(ConfirmView::default()),
            tooltip: Some(TooltipView::default()),
            top_bar: Some(TopBarView::default()),
            journal: Some(JournalView::default()),
            players: Some(PlayersView::default()),
            end_turn: Some(EndTurnView::default()),
            settings: Some(SettingsView::default()),
            minimap: Some(Default::default()),
            defeated: Some(Default::default()),
            conn: Some(ConnView::default()),
            perf: Some(PerfView::default()),
            ..Screens::default()
        };
        original_game_screen(&mut views);
        assert_eq!(
            views,
            Screens {
                conn: Some(ConnView::default()),
                perf: Some(PerfView::default()),
                settings: Some(SettingsView::default()),
                ..Screens::default()
            }
        );
    }
}

#[cfg(test)]
mod drag_click_tests {
    use super::*;
    use platform::{
        InputEvent, Modifiers, MouseButton, PhysicalPosition, PointerEvent, PointerId, PointerKind,
        PointerPhase,
    };

    fn up(button: MouseButton) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            id: PointerId(0),
            kind: PointerKind::Mouse,
            phase: PointerPhase::Up,
            position: PhysicalPosition { x: 300.0, y: 200.0 },
            button: Some(button),
            modifiers: Modifiers::default(),
        })
    }

    #[test]
    fn release_ending_a_drag_leaves_the_screen() {
        let InputEvent::Pointer(p) = ui_event_after_drag(&up(MouseButton::Left), true) else {
            panic!("pointer");
        };
        assert_eq!(p.phase, PointerPhase::Up);
        assert!(
            p.position.x < 0.0 && p.position.y < 0.0,
            "no control is under it"
        );
    }

    #[test]
    fn plain_click_and_right_button_pass_unchanged() {
        assert_eq!(
            ui_event_after_drag(&up(MouseButton::Left), false),
            up(MouseButton::Left)
        );
        assert_eq!(
            ui_event_after_drag(&up(MouseButton::Right), true),
            up(MouseButton::Right)
        );
    }
}
