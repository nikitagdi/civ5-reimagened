//! M2 of the original UI (B4): EUI's `InGame` on the replica of a real networked game (the default; `?ui=interim` turns it off).
//!
//! `M2Ui` owns the pieces and runs them in a fixed order:
//! network (replica messages, bodies) -> `Replica::pump` -> `ExeApi::dispatch` -> `UiLayer::frame`.
//!
//! Boot: the `ui` set and the `gc:replica` game data are fetched through the asset loader; when the game data is in
//! the core is made (`gc_create`, seconds on the main thread); the replica joins on the server's snapshot;
//! when it is `Live` the UI core is made over the exe API, the game is bound (the DLL registers `Players`, `Game`,
//! ... into the UI's Lua) and only then the root `InGame` loads, since EUI reads the game while its scripts load.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use assets::{AssetHandle, AssetLoader, AssetPoll, LogicalId};
use draw_list::UiDrawList;
use exe_api::{ExeApi, ExeConfig, ExeHost, SavedGame, SeatInfo};
use platform::{Clock, InputEvent, KeyValueStore};
use replica::{
    GameDataHost, GameFiles, Outgoing, Replica, ReplicaMsg, ReplicaStep, State as ReplicaState,
};
use ui_client::{
    BootPlan, BootState, LayerConfig, LayerHooks, Projector, UiBoot, UiLayer, ui_profile,
};

use crate::m2_highlights::{HexHighlights, TAPPED_EVENTS};
use crate::net::NetHandle;
use crate::timeline::{FrameInfo, Timeline};

/// Frame budget for `Replica::pump` (ms).
const PUMP_BUDGET_MS: u32 = 8;
/// A replica that is not `Live` after this long is reported (ms).
const JOIN_WARN_MS: u64 = 120_000;

/// The camera events `WorldView.lua` fires for its keys (the hot keys of `CONTROL_CAMERA_MOVE_*`), as the key of the
/// map controller that does the same and whether it goes down. SIMPLIFIED(SIMP-exe-560): the exe's camera moves on
/// these events; here the map controller's own arrow keys and zoom steps do (its speed, not the exe's).
const CAMERA_EVENTS: [(&str, platform::NamedKey, bool); 10] = {
    use platform::NamedKey as K;
    [
        ("SerialEventCameraStartMovingLeft", K::ArrowLeft, true),
        ("SerialEventCameraStopMovingLeft", K::ArrowLeft, false),
        ("SerialEventCameraStartMovingRight", K::ArrowRight, true),
        ("SerialEventCameraStopMovingRight", K::ArrowRight, false),
        ("SerialEventCameraStartMovingForward", K::ArrowUp, true),
        ("SerialEventCameraStopMovingForward", K::ArrowUp, false),
        ("SerialEventCameraStartMovingBack", K::ArrowDown, true),
        ("SerialEventCameraStopMovingBack", K::ArrowDown, false),
        ("SerialEventCameraIn", K::PageUp, true),
        ("SerialEventCameraOut", K::PageDown, true),
    ]
};

fn camera_key(key: platform::NamedKey, pressed: bool) -> InputEvent {
    InputEvent::Key(platform::KeyEvent {
        key: platform::Key::Named(key),
        code: None,
        pressed,
        repeat: false,
        modifiers: platform::Modifiers::default(),
    })
}

/// The grid cells the camera sees, `(x0, y0, x1, y1)` with upper bounds exclusive, unwrapped, a cell wider than the view
/// on each side (the exe's visible rectangle, `GameViewState::CalcVisibleRegion`: where the edges of the view frustum meet
/// the ground planes at heights 0 and 127.5). `None` when the view reaches no plane.
pub fn visible_cells(view_proj: &glam::Mat4) -> Option<[i32; 4]> {
    use render::hexgrid::{HEX_WIDTH, ROW_STEP};
    let inverse = view_proj.inverse();
    if !inverse.is_finite() {
        return None;
    }
    let (mut min, mut max) = (
        glam::Vec2::splat(f32::INFINITY),
        glam::Vec2::splat(f32::NEG_INFINITY),
    );
    for (cx, cy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let near = inverse.project_point3(glam::Vec3::new(cx, cy, 0.0));
        let far = inverse.project_point3(glam::Vec3::new(cx, cy, 1.0));
        let dir = far - near;
        for height in [0.0_f32, 127.5] {
            if dir.y.abs() < 1e-6 {
                continue;
            }
            let t = (height - near.y) / dir.y;
            if t <= 0.0 {
                continue;
            }
            let p = near + dir * t;
            if !p.is_finite() {
                continue;
            }
            // The ground is (X, Z).
            min = min.min(glam::Vec2::new(p.x, p.z));
            max = max.max(glam::Vec2::new(p.x, p.z));
        }
    }
    if min.x > max.x {
        return None;
    }
    // Cell (x, y) is centred at (HEX_WIDTH * (x + 0.5 * (y & 1)), -ROW_STEP * y).
    let x0 = (min.x / HEX_WIDTH - 0.5).floor() as i32 - 1;
    let x1 = (max.x / HEX_WIDTH).floor() as i32 + 3;
    let y0 = (-max.y / ROW_STEP).floor() as i32 - 1;
    let y1 = (-min.y / ROW_STEP).ceil() as i32 + 2;
    Some([x0, y0, x1, y1])
}

/// Plot (x, y), screen fractions (px, py), zoom level.
pub type CameraPlace = (i32, i32, f32, f32, f32);

/// What `ExeApi` asks of the app.
/// The plots of every trade route path and the `TradeConnection` index of each.
pub type TradePaths = (Vec<Vec<(i32, i32)>>, Vec<i32>);

pub struct M2Host {
    kv: Rc<dyn KeyValueStore>,
    /// The page's `?volume=` (0..=1): where the volume knobs of the options menu start (SIMP-exe-190).
    volume_default: f32,
    /// The volume knobs that moved (`MUSIC`, `SFX`, `SPEECH`, `AMBIENCE`: each the gain of its bus), until the app
    /// applied them.
    knobs: RefCell<Vec<(String, f32)>>,
    /// The events the music listens to (a leader screen, the game over screen), until the app took them.
    music_events: RefCell<Vec<exe_api::music::MusicEvent>>,
    /// `UI.SaveGame`: the host asks for a save of the game (WO-G, K20).
    save_req: Cell<bool>,
    /// The saves of the server (the lobby state), for the save menu.
    saves: RefCell<Vec<SavedGame>>,
    /// `Events.ExitToMainMenu`: the player leaves for the lobby (WO-G, K20).
    exit_req: Cell<bool>,
    look: Cell<Option<(i32, i32)>>,
    /// `__exe_camera_place` (debug, the golden scenario): plot, screen fractions, zoom.
    place: Cell<Option<CameraPlace>>,
    /// The city screen went up (true) or down (false) since the app last looked (the camera takes the city view).
    city_view: Cell<Option<bool>>,
    seat: RefCell<SeatInfo>,
    /// `M2Env::ai_turn` of the frame.
    ai_turn: RefCell<Option<exe_api::AiTurnLog>>,
    /// Size of the renderer's minimap texture (`MinimapTextureBroadcastEvent`).
    minimap: Cell<(u32, u32)>,
    /// The relief's height at the centre of every plot (width, height, row by row) for `GridToWorld`, and the
    /// renderer's revision of the heights it was made at.
    heights: RefCell<(u32, u32, Vec<f32>)>,
    heights_rev: Cell<Option<u64>>,
    /// A click on the minimap panel, in pixels of the control (`Events.MinimapClickedEvent`).
    minimap_click: Cell<Option<(f32, f32)>>,
    /// `Network.SendChat`: lines the layer wants to send (to a player or to all).
    chat_out: RefCell<Vec<(Option<i32>, String)>>,
    /// `Matchmaking.KickPlayer`.
    kicks: RefCell<Vec<i32>>,
    /// `UI.SetGridVisibleMode`: the grid state the layer asked for, until the app applied it.
    grid: Cell<Option<bool>>,
    /// The camera's hex rectangle of this frame ([`visible_cells`]).
    visible: Cell<Option<[i32; 4]>>,
    /// Types of new notifications whose sound the exe plays (`NotificationSounds.xml`), until the app played them.
    notification_sounds: RefCell<Vec<String>>,
    /// The camera events of `WorldView.lua`'s keys (`SerialEventCameraStartMovingLeft`, `SerialEventCameraIn`, ...) as the
    /// keys of the map controller, until the app took them.
    camera_keys: RefCell<Vec<InputEvent>>,
    /// The viewer's bridges over rivers (`GameplayBridgeChanged`: has them, iron), until the app took them.
    bridge: Cell<Option<(bool, bool)>>,
    /// Unit selection and city ambience requests of the exe, as queue entries (scripts or prefixed), until played.
    exe_sounds: RefCell<Vec<String>>,
    /// The plots of `Events.GameplayFX` (the "look here" effect), until the app took them.
    gameplay_fx: RefCell<Vec<(i32, i32)>>,
}

/// A notification sound in the queue of [`M2Ui::take_sound_scripts`]: the type name after this prefix (the rest are scripts).
pub const NOTIFICATION_SOUND_PREFIX: &str = "notification:";
/// A popup sound in the queue: the popup type name (`BUTTONPOPUP_NEW_ERA`) after this prefix (`ButtonPopupSounds.xml`).
pub const POPUP_SOUND_PREFIX: &str = "popup:";
/// An event sound in the queue: the event name (`CityCreated`) after this prefix (`EventSoundConnections.xml`).
pub const EVENT_SOUND_PREFIX: &str = "event:";
/// A script whose sound files are fetched ahead (nothing plays): the script after this prefix.
pub const PRELOAD_PREFIX: &str = "preload:";
/// A looping city ambience in the queue: the script after this prefix starts (and ends the one before).
pub const AMBIENCE_START_PREFIX: &str = "ambience:";
/// The city ambience in the queue ends.
pub const AMBIENCE_STOP: &str = "ambience-stop:";
/// A popup's spoken line in the queue: `<delay ms>:<script>` after this prefix.
pub const VOICE_PREFIX: &str = "voice:";
/// A city-state's greeting in the queue: its `MINOR_CIV_*` type after this prefix (`MinorCivSounds.xml`).
pub const MINOR_CIV_PREFIX: &str = "minorciv:";
/// A unit's action the port does not animate, for its sounds: [`crate::unit_sound::UnitCue::format`] after this prefix.
pub const UNIT_CUE_PREFIX: &str = "unit-cue:";
/// The unit cues one frame keeps (a turn of many moves must not flood the queue).
const MAX_UNIT_CUES: usize = 64;

impl ExeHost for M2Host {
    fn bridge_changed(&self, bridge: bool, iron: bool) {
        self.bridge.set(Some((bridge, iron)));
    }

    fn unit_selection_sound(&self, script: &str) {
        let mut q = self.exe_sounds.borrow_mut();
        if q.len() < 16 {
            q.push(script.to_owned());
        }
    }

    fn effect_sound(&self, script: &str) {
        let mut q = self.exe_sounds.borrow_mut();
        if q.len() < 16 {
            q.push(script.to_owned());
        }
    }

    fn sound_preload(&self, script: &str) {
        self.exe_sounds
            .borrow_mut()
            .push(format!("{PRELOAD_PREFIX}{script}"));
    }

    fn city_ambience(&self, script: Option<&str>) {
        self.exe_sounds.borrow_mut().push(match script {
            Some(s) => format!("{AMBIENCE_START_PREFIX}{s}"),
            None => AMBIENCE_STOP.to_owned(),
        });
    }

    fn notification_sound(&self, type_name: &str) {
        let mut q = self.notification_sounds.borrow_mut();
        if q.len() < 16 {
            q.push(type_name.to_owned());
        }
    }

    fn popup_sound(&self, type_name: &str) {
        let mut q = self.exe_sounds.borrow_mut();
        if q.len() < 16 {
            q.push(format!("{POPUP_SOUND_PREFIX}{type_name}"));
        }
    }

    fn event_sound(&self, name: &str) {
        let mut q = self.exe_sounds.borrow_mut();
        if q.len() < 16 {
            q.push(format!("{EVENT_SOUND_PREFIX}{name}"));
        }
    }

    fn voice_over(&self, script: &str, delay_ms: u32) {
        self.exe_sounds
            .borrow_mut()
            .push(format!("{VOICE_PREFIX}{delay_ms}:{script}"));
    }

    fn minor_civ_sound(&self, minor_type: &str) {
        self.exe_sounds
            .borrow_mut()
            .push(format!("{MINOR_CIV_PREFIX}{minor_type}"));
    }

    fn unit_clip_sound(&self, cue: exe_api::UnitClipCue) {
        let mut q = self.exe_sounds.borrow_mut();
        if q.iter().filter(|s| s.starts_with(UNIT_CUE_PREFIX)).count() < MAX_UNIT_CUES {
            let cue = crate::unit_sound::UnitCue {
                owner: cue.owner,
                id: cue.id,
                ec: cue.ec,
                x: cue.x,
                y: cue.y,
                opponent: cue.opponent,
            };
            q.push(format!("{UNIT_CUE_PREFIX}{}", cue.format()));
        }
    }

    fn kv(&self) -> &dyn KeyValueStore {
        // The options and user data of the original UI live in the page's store (`localStorage`): they stick between
        // sessions of this client (WO-G; SIMP-exe-60 held them in memory for the page's life).
        &*self.kv
    }

    fn look_at(&self, x: i32, y: i32) {
        self.look.set(Some((x, y)));
    }

    fn gameplay_fx(&self, x: i32, y: i32) {
        let mut q = self.gameplay_fx.borrow_mut();
        if q.len() < 16 {
            q.push((x, y));
        }
    }

    fn camera_place(&self, x: i32, y: i32, px: f32, py: f32, level: f32) {
        self.place.set(Some((x, y, px, py, level)));
    }

    fn city_screen(&self, up: bool) {
        self.city_view.set(Some(up));
    }

    fn seat_info(&self) -> SeatInfo {
        self.seat.borrow().clone()
    }

    fn ai_turn(&self) -> Option<exe_api::AiTurnLog> {
        self.ai_turn.borrow().clone()
    }

    fn minimap_size(&self) -> (u32, u32) {
        self.minimap.get()
    }

    fn plot_height(&self, x: i32, y: i32) -> Option<f32> {
        let h = self.heights.borrow();
        let (w, rows) = (h.0 as i32, h.1 as i32);
        if w <= 0 || !(0..rows).contains(&y) {
            return None;
        }
        // A wrapping map: the column wraps (a plot past the edge is the one on the other side).
        h.2.get((y * w + x.rem_euclid(w)) as usize).copied()
    }

    fn send_chat(&self, to_player: Option<i32>, text: &str) {
        self.chat_out
            .borrow_mut()
            .push((to_player, text.to_owned()));
    }

    fn kick_player(&self, player: i32) {
        self.kicks.borrow_mut().push(player);
    }

    fn set_grid_visible(&self, on: bool) {
        self.grid.set(Some(on));
    }

    fn visible_cells(&self) -> Option<[i32; 4]> {
        self.visible.get()
    }

    fn save_game(&self, _name: &str) {
        self.save_req.set(true);
    }

    fn exit_to_lobby(&self) {
        self.exit_req.set(true);
    }

    fn set_volume(&self, knob: &str, value: f32) {
        let mut k = self.knobs.borrow_mut();
        k.retain(|(n, _)| n != knob);
        k.push((knob.to_owned(), value));
    }

    fn music_event(&self, e: exe_api::music::MusicEvent) {
        let mut q = self.music_events.borrow_mut();
        if q.len() < 16 {
            q.push(e);
        }
    }

    fn volume_default(&self) -> f32 {
        self.volume_default
    }

    fn saved_games(&self) -> Vec<SavedGame> {
        self.saves.borrow().clone()
    }
}

/// `Box<dyn Clock>` over the app's clock.
struct SharedClock(Rc<dyn Clock>);

impl Clock for SharedClock {
    fn now_ms(&self) -> u64 {
        self.0.now_ms()
    }

    fn now_ms_f64(&self) -> f64 {
        self.0.now_ms_f64()
    }
}

enum DataStage {
    Table(AssetHandle),
    Files {
        queue: Vec<LogicalId>,
        pending: Vec<(LogicalId, AssetHandle)>,
        total: usize,
        done: usize,
    },
    Ready,
    Failed(String),
}

/// Fetches the `gc:replica` members (the group table, the index first, the databases, the Lua of the add-ins).
struct DataBoot {
    stage: Option<DataStage>,
    files: Arc<Mutex<GameFiles>>,
    have_index: bool,
    /// The files of the core, filled when the index is in.
    started_ms: Option<u64>,
}

impl DataBoot {
    fn new() -> DataBoot {
        DataBoot {
            stage: None,
            files: Arc::new(Mutex::new(GameFiles::default())),
            have_index: false,
            started_ms: None,
        }
    }

    fn ready(&self) -> bool {
        matches!(self.stage, Some(DataStage::Ready))
    }

    fn failed(&self) -> Option<&str> {
        match &self.stage {
            Some(DataStage::Failed(m)) => Some(m),
            _ => None,
        }
    }

    fn progress(&self) -> (usize, usize) {
        match &self.stage {
            Some(DataStage::Files { done, total, .. }) => (*done, *total),
            Some(DataStage::Ready) => (1, 1),
            _ => (0, 0),
        }
    }

    fn update(&mut self, loader: &mut AssetLoader, now_ms: u64) {
        let profile = ui_profile();
        let stage = match self.stage.take() {
            None => {
                self.started_ms = Some(now_ms);
                DataStage::Table(loader.request(LogicalId::new(assets::GROUP_TABLE_ID), &profile))
            }
            Some(s) => s,
        };
        self.stage = Some(match stage {
            DataStage::Table(h) => match loader.poll(h) {
                AssetPoll::Pending => DataStage::Table(h),
                AssetPoll::Failed(e) => DataStage::Failed(format!("group table: {e}")),
                AssetPoll::Ready(bytes) => match replica::group_members(&bytes) {
                    Ok(ids) => {
                        let queue: Vec<LogicalId> =
                            ids.into_iter().rev().map(LogicalId::new).collect();
                        let total = queue.len();
                        DataStage::Files {
                            queue,
                            pending: Vec::new(),
                            total,
                            done: 0,
                        }
                    }
                    Err(e) => DataStage::Failed(e.to_string()),
                },
            },
            DataStage::Files {
                mut queue,
                mut pending,
                total,
                mut done,
            } => {
                let mut failed = None;
                let files = Arc::clone(&self.files);
                let have_index = &mut self.have_index;
                // The index first: nothing else can be put before `GameFiles::from_index`.
                pending.retain(|(id, h)| match loader.poll(*h) {
                    AssetPoll::Pending => true,
                    AssetPoll::Ready(bytes) => {
                        done += 1;
                        let Ok(mut g) = files.lock() else {
                            return false;
                        };
                        if id.as_str() == replica::VFS_INDEX_ID {
                            match GameFiles::from_index(bytes) {
                                Ok(f) => {
                                    *g = f;
                                    *have_index = true;
                                }
                                Err(e) => {
                                    failed.get_or_insert(format!("vfs-index: {e}"));
                                }
                            }
                        } else {
                            g.put(id.as_str(), bytes);
                        }
                        false
                    }
                    AssetPoll::Failed(e) => {
                        failed.get_or_insert(format!("{}: {e}", id.as_str()));
                        false
                    }
                });
                if let Some(m) = failed {
                    DataStage::Failed(m)
                } else {
                    // Until the index is in, only the index is in flight.
                    let limit = if *have_index { 24 } else { 1 };
                    while pending.len() < limit {
                        let Some(id) = queue.pop() else { break };
                        let h = loader.request(id.clone(), &profile);
                        pending.push((id, h));
                    }
                    if pending.is_empty() && queue.is_empty() {
                        tracing::info!(files = total, "the replica game data is loaded");
                        DataStage::Ready
                    } else {
                        DataStage::Files {
                            queue,
                            pending,
                            total,
                            done,
                        }
                    }
                }
            }
            s @ (DataStage::Ready | DataStage::Failed(_)) => s,
        });
    }
}

/// Where M2 is, for the loading text and the logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum M2Phase {
    /// The `ui` set and the game data are being fetched.
    Loading,
    /// The core is made and the replica waits for its snapshot or loads it.
    Joining,
    /// The UI runs.
    Running,
    Failed,
}

/// What the app gives a frame.
pub struct M2Env<'a> {
    pub now_ms: u64,
    pub loader: Option<&'a mut AssetLoader>,
    /// Physical size of the window.
    pub size: (u32, u32),
    pub net: Option<&'a NetHandle>,
    pub clock: &'a Rc<dyn Clock>,
    pub seat: SeatInfo,
    /// The AI civilizations the server started and ended in the last AI phase it ran (SIMP-exe-820, SIMP-exe-790).
    pub ai_turn: Option<exe_api::AiTurnLog>,
    /// The game's seat of this client is known (the attach came): the replica's active player is that seat, so the
    /// core is not made before.
    pub seat_known: bool,
    /// `view_proj` of the game camera, for the projection of world anchors, and the scale of the UI.
    pub projection: Option<ProjectionInfo>,
}

/// The camera as `WorldAnchor` needs it.
#[derive(Debug, Clone, Copy)]
pub struct ProjectionInfo {
    pub view_proj: glam::Mat4,
    /// Physical pixels per UI pixel.
    pub ui_scale: f32,
    /// The ground point (X, Z) in the middle of the screen and the zoom level (for the stats: the playtest checks that the
    /// city screen took the camera).
    pub focus: [f32; 2],
    pub level: f32,
}

/// Numbers for `__civ5stats.{ui,replica,exe}.*`.
#[derive(Debug, Clone, Default)]
pub struct M2Stats {
    pub groups: Vec<(&'static str, Vec<(String, f64)>)>,
}

pub struct M2Ui {
    phase: M2Phase,
    failed: Option<String>,
    ui_boot: UiBoot,
    data: DataBoot,
    replica: Option<Rc<RefCell<Replica>>>,
    backlog: Vec<ReplicaMsg>,
    exe: Option<Rc<RefCell<ExeApi>>>,
    host: Rc<M2Host>,
    layer: Option<UiLayer>,
    hl: Rc<RefCell<HexHighlights>>,
    /// Scripts the Lua asked for with `Events.AudioPlay2DSound`, not yet taken (`take_sound_scripts`).
    sounds: Rc<RefCell<Vec<String>>>,
    contexts: Vec<String>,
    allowed: Rc<BTreeSet<String>>,
    projector: Rc<RefCell<Option<ProjectionInfo>>>,
    window: Rc<Cell<(u32, u32)>>,
    size: (u32, u32),
    logged_errors: u64,
    /// Number of missing textures the log has named (`m2.tex`).
    logged_missing: usize,
    clk: Option<Rc<dyn Clock>>,
    started_ms: Option<u64>,
    live_ms: Option<u64>,
    /// Dev (`?luadbg=--dump`): log the control tree once, 20 s after the first frame.
    dump_armed: bool,
    dump_at: Option<u64>,
    warned: bool,
    last_frame_us: Cell<u64>,
    last_stats_ms: u64,
    stats: M2Stats,
    unresolved: BTreeSet<u64>,
    /// The zoom level before the city view took the camera.
    saved_level: Option<f32>,
    blob_wanted: BTreeSet<u64>,
    resync_flush: Vec<Outgoing>,
    /// Numbers of the `EndTurn` orders the UI's `sendTurnComplete` sent that have no final answer yet (the last few).
    end_turn_orders: Vec<protocol::CmdId>,
    /// Chat lines of the stream handed to the layer so far (`ClientCore::chat_total`), and the counters of the report.
    chat_seen: u64,
    chat_in: u64,
    chat_sent: u64,
    kicks_sent: u64,
    /// WO-G (K20): saves the host asked for, and the exit to the lobby went out.
    saves_sent: u64,
    left: bool,
    /// The layer of the last game is gone and the next one is not playing yet (`new_game`).
    awaiting_game: bool,
    minimap_clicks: u64,
    minimap_map: bool,
    minimap_seen: (u32, u32),
    /// Hot keys the exe carried out (`ExeApi::hotkey`), and the cell the selection ring was last drawn on.
    hotkeys: u64,
    ring: Option<(i32, i32)>,
    /// Milliseconds from the first frame of M2 to the ui set being in the store (the boot fetches done), and to M2 running.
    ui_ready_ms: Option<u64>,
    up_ms: Option<u64>,
    /// Time spent in `gc_create` and in the root load (ms), for the report.
    pub create_ms: f64,
    pub root_ms: f64,
    /// The per-turn timeline (`__civ5stats.turn`, the log ring's `span` lines).
    timeline: Timeline,
    /// The last Lua error number logged (`lua.error`).
    lua_error_seq: u64,
    /// The last slow frame the first-frame line has seen.
    slow_seq: u64,
    frame_t0: f64,
    /// TEMP(T51): what the replica's pump did in the last frame, and the game's localizer, for UI v2 to read until
    /// the game session owns the replica.
    v2_steps: Vec<ReplicaStep>,
    v2_loc: Option<Rc<loc::Localizer>>,
}

impl M2Ui {
    /// The variants the device takes, for the UI textures fetched from now on (the boot and the
    /// background fetches); a phone gets their `Astc` variants.
    pub fn set_texture_profile(&mut self, profile: &assets::PlatformProfile) {
        if self.ui_boot.profile() != profile {
            self.ui_boot.set_profile(profile.clone());
        }
    }

    /// `kv`: the page's store for the options and the user data; `ui_volume`: the page's `?volume=` (0..=1).
    pub fn new(kv: Rc<dyn KeyValueStore>, ui_volume: f32) -> M2Ui {
        let contexts = exe_api::m2_contexts();
        let allowed: BTreeSet<String> = contexts.iter().map(|c| c.to_ascii_lowercase()).collect();
        let (level, wall_offset) = crate::trace_settings();
        let mut timeline = Timeline::new(level);
        timeline.set_wall_offset(wall_offset);
        M2Ui {
            phase: M2Phase::Loading,
            failed: None,
            ui_boot: UiBoot::new(BootPlan::m2(contexts.clone())),
            data: DataBoot::new(),
            replica: None,
            backlog: Vec::new(),
            exe: None,
            host: Rc::new(M2Host {
                kv,
                volume_default: ui_volume.clamp(0.0, 1.0),
                knobs: RefCell::new(Vec::new()),
                music_events: RefCell::new(Vec::new()),
                save_req: Cell::new(false),
                saves: RefCell::new(Vec::new()),
                exit_req: Cell::new(false),
                look: Cell::new(None),
                place: Cell::new(None),
                city_view: Cell::new(None),
                seat: RefCell::new(SeatInfo::default()),
                ai_turn: RefCell::new(None),
                minimap: Cell::new((320, 190)),
                heights: RefCell::new((0, 0, Vec::new())),
                heights_rev: Cell::new(None),
                minimap_click: Cell::new(None),
                chat_out: RefCell::new(Vec::new()),
                kicks: RefCell::new(Vec::new()),
                grid: Cell::new(None),
                visible: Cell::new(None),
                notification_sounds: RefCell::new(Vec::new()),
                exe_sounds: RefCell::new(Vec::new()),
                gameplay_fx: RefCell::new(Vec::new()),
                camera_keys: RefCell::new(Vec::new()),
                bridge: Cell::new(None),
            }),
            layer: None,
            hl: Rc::new(RefCell::new(HexHighlights::new())),
            sounds: Rc::new(RefCell::new(Vec::new())),
            contexts,
            allowed: Rc::new(allowed),
            projector: Rc::new(RefCell::new(None)),
            window: Rc::new(Cell::new((1, 1))),
            size: (0, 0),
            logged_errors: 0,
            logged_missing: 0,
            clk: None,
            started_ms: None,
            live_ms: None,
            dump_armed: false,
            dump_at: None,
            warned: false,
            last_frame_us: Cell::new(0),
            last_stats_ms: 0,
            stats: M2Stats::default(),
            unresolved: BTreeSet::new(),
            saved_level: None,
            blob_wanted: BTreeSet::new(),
            resync_flush: Vec::new(),
            end_turn_orders: Vec::new(),
            chat_seen: 0,
            chat_in: 0,
            chat_sent: 0,
            kicks_sent: 0,
            saves_sent: 0,
            left: false,
            awaiting_game: false,
            minimap_clicks: 0,
            minimap_map: false,
            minimap_seen: (0, 0),
            hotkeys: 0,
            ring: None,
            ui_ready_ms: None,
            up_ms: None,
            create_ms: 0.0,
            root_ms: 0.0,
            timeline,
            lua_error_seq: 0,
            slow_seq: 0,
            frame_t0: 0.0,
            v2_steps: Vec::new(),
            v2_loc: None,
        }
    }

    /// TEMP(T51): the replica, for UI v2 to read after this layer's frame (the game session will own it).
    pub fn v2_replica(&self) -> Option<Rc<RefCell<Replica>>> {
        self.replica.clone()
    }

    /// TEMP(T51): what the replica's pump did in the last frame, taken once.
    pub fn v2_take_steps(&mut self) -> Vec<ReplicaStep> {
        std::mem::take(&mut self.v2_steps)
    }

    /// TEMP(T51): the game's localizer, once the layer loaded it.
    pub fn v2_localizer(&self) -> Option<Rc<loc::Localizer>> {
        self.v2_loc.clone()
    }

    /// TEMP(T51): UI v2 asks the camera to go to a plot, as the layer's `UI.LookAt` does (carried out next frame).
    pub fn v2_look_at(&self, x: i32, y: i32) {
        self.host.look.set(Some((x, y)));
    }

    pub fn phase(&self) -> M2Phase {
        self.phase
    }

    pub fn failure(&self) -> Option<&str> {
        self.failed.as_deref()
    }

    pub fn layer(&self) -> Option<&UiLayer> {
        self.layer.as_ref()
    }

    pub fn contexts(&self) -> &[String] {
        &self.contexts
    }

    /// The camera goes to the plot `UI.LookAt` named, once.
    pub fn take_look_at(&self) -> Option<(i32, i32)> {
        self.host.look.take()
    }

    /// The plots `Events.GameplayFX` played the "look here" effect on since the last call, oldest first.
    pub fn take_gameplay_fx(&self) -> Vec<(i32, i32)> {
        std::mem::take(&mut *self.host.gameplay_fx.borrow_mut())
    }

    /// `__exe_camera_place` (debug): plot, screen fractions, zoom level, once.
    pub fn take_camera_place(&self) -> Option<CameraPlace> {
        self.host.place.take()
    }

    /// The 2D sound scripts the Lua asked for since the last call (Events.AudioPlay2DSound), oldest first.
    /// Notification sounds come as [`NOTIFICATION_SOUND_PREFIX`] and the type name.
    pub fn take_sound_scripts(&self) -> Vec<String> {
        let mut out = std::mem::take(&mut *self.sounds.borrow_mut());
        out.extend(self.host.exe_sounds.borrow_mut().drain(..));
        out.extend(
            self.host
                .notification_sounds
                .borrow_mut()
                .drain(..)
                .map(|t| format!("{NOTIFICATION_SOUND_PREFIX}{t}")),
        );
        out
    }

    /// The camera height of the city view (500 / 700 / 800 by the city-distance high-water mark) as a zoom level;
    /// 500 without a game.
    fn city_view_level(&self) -> f32 {
        let height = self
            .exe
            .as_ref()
            .and_then(|e| e.try_borrow().ok().map(|e| e.city_zoom_height()))
            .unwrap_or(exe_api::CITY_ZOOM_HEIGHTS[0]);
        render::camera_params::level_for_height(height)
    }

    /// The camera takes the city view's zoom (`up`) or gives the player's zoom back.
    /// The exe's `CITY_ZOOM_IN` height is 500 / 700 / 800 by the city-distance high-water mark (night answers 1b P4),
    /// `CITY_ZOOM_OUT` gives the remembered height back. The pan offset (y - 20) and the easing are not taken (SIMP-exe-750).
    pub fn city_camera(&mut self, up: bool, camera: &mut render::GameCamera) {
        if up {
            self.saved_level.get_or_insert(camera.level());
            camera.set_level(self.city_view_level());
        } else if let Some(l) = self.saved_level.take() {
            camera.set_level(l);
        }
    }

    /// Every human has ended the turn: the server runs it (the globe cursor).
    pub fn turn_processing(&self) -> bool {
        self.exe
            .as_ref()
            .is_some_and(|e| e.try_borrow().is_ok_and(|e| e.turn_all_complete()))
    }

    /// `UI.IsCameraMoving`: the camera is still easing to a target or gliding.
    pub fn set_camera_moving(&self, moving: bool) {
        if let Some(Ok(e)) = self.exe.as_ref().map(|e| e.try_borrow()) {
            e.set_camera_moving(moving);
        }
    }

    /// The city screen went up or down since the last call (the camera takes or leaves the city view).
    pub fn take_city_view(&self) -> Option<bool> {
        self.host.city_view.take()
    }

    /// The viewer's bridges changed since the last call: `(has bridges, iron)` (`Renderer::set_bridges`).
    pub fn take_bridge(&self) -> Option<(bool, bool)> {
        self.host.bridge.take()
    }

    /// The highlights changed since the last call (draw them with `Renderer::set_highlight_set`).
    pub fn take_highlights(&self) -> Option<render::world::overlay::highlights::HighlightSet> {
        self.hl.borrow_mut().take_changed()
    }

    /// The movement path the original UI shows (a right drag of a unit, the move-to mode), for the renderer's path.
    /// The core counts the turns from 0 (this turn); the exe's path, the renderer's looks and `UIPathFinderUpdate`
    /// count from 1, so turn 1 is blue, 2 yellow, 3 and later grey-green (live report 2026-10-30: one turn late).
    pub fn movement_path(&self) -> Vec<render::entities::PathStep> {
        let Some(exe) = &self.exe else {
            return Vec::new();
        };
        let Ok(e) = exe.try_borrow() else {
            return Vec::new();
        };
        e.movement_path()
            .iter()
            .map(|n| render::entities::PathStep {
                x: i32::from(n.x),
                y: i32::from(n.y),
                turn: i32::from(n.turn) + 1,
                flags: n.flags.0,
            })
            .collect()
    }

    /// The paths of every trade route from the replica core (`Q_TRADE_PATHS`): the plots the DLL's trade route path
    /// finder laid, origin city first, which the caravans walk and the exe's `TradeVisSystem` draws. `None` while the
    /// exe API is busy (try again later).
    pub fn trade_paths(&self) -> Option<TradePaths> {
        let exe = self.exe.as_ref()?;
        let e = exe.try_borrow().ok()?;
        let paths = e.trade_paths(-1);
        let ids = paths.iter().map(|p| p.id).collect();
        let plots = paths
            .into_iter()
            .map(|p| {
                p.plots
                    .into_iter()
                    .map(|(x, y)| (i32::from(x), i32::from(y)))
                    .collect()
            })
            .collect();
        Some((plots, ids))
    }

    /// The trade route popup the exe shows now (`TradeVisuals_ActivatePopupRoute`).
    pub fn popup_route(&self) -> Option<exe_api::PopupRoute> {
        self.exe.as_ref()?.try_borrow().ok()?.popup_route()
    }

    /// The mouse is over a plot of the map (grid coordinates).
    pub fn mouse_over_hex(&self, x: i32, y: i32) {
        if let Some(exe) = &self.exe
            && let Ok(mut e) = exe.try_borrow_mut()
        {
            e.post(exe_api::ClientEvent::MouseOverHex { x, y });
        }
    }

    /// The player did something: idle checks of the replica wait.
    pub fn note_input(&self) {
        if let Some(r) = &self.replica
            && let Ok(mut r) = r.try_borrow_mut()
        {
            r.note_input();
        }
    }

    /// Mouse, wheel, keys, text. `true`: the UI used the event.
    ///
    /// A key goes to the Lua input handlers first (an edit box with the focus takes every key); a press none of
    /// them took is the exe's hot key (`ExeApi::hotkey`: unit orders, END TURN, ...). The modifiers and the key
    /// release reach the exe API as well (`UI.ShiftKeyDown`, `Events.KeyUpEvent`).
    pub fn handle_input(&mut self, ev: &InputEvent) -> bool {
        self.note_input();
        let Some(l) = &mut self.layer else {
            return false;
        };
        if let Some((ctrl, shift, alt)) = held_modifiers(ev)
            && let Some(exe) = &self.exe
            && let Ok(mut e) = exe.try_borrow_mut()
        {
            e.post(exe_api::ClientEvent::Modifiers { ctrl, shift, alt });
        }
        // `UIManager::ProcessInput` raises `Events.KeyUpEvent` BEFORE the Lua dispatch, so a key release the Lua
        // handler takes (WorldView's arrows) still reaches the listeners that stop the camera scroll when a popup
        // took the focus while the key was held.
        if let InputEvent::Key(k) = ev
            && !k.pressed
            && let (Some(vk), Some(exe)) = (ui::core::key_vk(k), &self.exe)
            && let Ok(mut e) = exe.try_borrow_mut()
        {
            e.post(exe_api::ClientEvent::KeyUp(vk));
        }
        if l.handle_input(ev) == ui_client::InputResult::Consumed {
            return true;
        }
        let InputEvent::Key(k) = ev else {
            return false;
        };
        if !k.pressed {
            return false;
        }
        let (Some(vk), Some(exe)) = (ui::core::key_vk(k), &self.exe) else {
            return false;
        };
        let Ok(e) = exe.try_borrow_mut() else {
            return false;
        };
        if k.repeat {
            return false;
        }
        match e.hotkey(vk, k.modifiers.ctrl, k.modifiers.alt, k.modifiers.shift) {
            Some(action) => {
                tracing::info!(target: "m2.hotkey", "key {vk:#x}: {action}");
                self.hotkeys += 1;
                true
            }
            None => false,
        }
    }

    /// What the layer asked of the camera since the last call (`SerialEventCameraStartMovingLeft` and friends from
    /// `WorldView.lua`'s keys), as the keys the map controller knows.
    pub fn take_camera_keys(&self) -> Vec<InputEvent> {
        std::mem::take(&mut *self.host.camera_keys.borrow_mut())
    }

    /// The plot of the head selected unit (the engine's selection ring is drawn there).
    pub fn selected_unit_plot(&self) -> Option<(i32, i32)> {
        let exe = self.exe.as_ref()?;
        exe.try_borrow().ok()?.selected_unit_plot()
    }

    /// The cell the renderer drew the selection ring on (for the stats).
    pub fn note_ring(&mut self, cell: Option<(i32, i32)>) {
        self.ring = cell;
    }

    fn fail(&mut self, why: String) {
        tracing::error!("the original UI (M2) is off: {why}");
        self.failed = Some(why);
        self.phase = M2Phase::Failed;
    }

    /// The spans of the replica and of the UI side since the last frame go to the turn timeline in time order; the
    /// timeline's lines go to the log (`civ5::span`, kept out of the console); `c.processing` is watched.
    fn collect_timeline(&mut self) {
        let Some(clk) = self.clk.clone() else { return };
        let now = clk.now_ms_f64();
        let (mut rep_spans, mut exe_spans) = (Vec::new(), Vec::new());
        let mut turn_now = 0;
        if let Some(rep) = &self.replica
            && let Ok(mut r) = rep.try_borrow_mut()
        {
            turn_now = r.turn();
            rep_spans = r.take_spans();
        }
        let mut all_complete = false;
        if let Some(exe) = &self.exe
            && let Ok(e) = exe.try_borrow()
        {
            exe_spans = e.take_spans();
            all_complete = e.turn_all_complete();
        }
        self.timeline
            .ingest(rep_spans, exe_spans, turn_now, all_complete, now);
        for line in self.timeline.take_lines() {
            tracing::info!(target: "civ5::span", json = %line);
        }
    }

    /// After the frame was drawn (called by the app): the settle of the render after a turn's load. `pending`: cells in
    /// view without a page; `view_ready`: none.
    pub fn frame_done(&mut self, view_ready: bool, pending: usize) {
        if !self.timeline.settling() {
            return;
        }
        let Some(clk) = self.clk.clone() else { return };
        let now = clk.now_ms_f64();
        let frame_ms = (now - self.frame_t0).max(0.0);
        // The names of the biggest scopes of the frame, once, for the first frame after the load.
        let slow = render::perf::slow::ring();
        let top = slow.last().filter(|f| f.seq > self.slow_seq).map(|f| {
            f.scopes
                .iter()
                .take(3)
                .map(|(n, _)| *n)
                .collect::<Vec<_>>()
                .join(",")
        });
        if let Some(f) = slow.last() {
            self.slow_seq = self.slow_seq.max(f.seq);
        }
        self.timeline.frame(
            FrameInfo {
                now_ms: now,
                frame_ms,
                view_ready,
                pages_pending: pending,
            },
            top.as_deref(),
        );
        for line in self.timeline.take_lines() {
            tracing::info!(target: "civ5::span", json = %line);
        }
    }

    /// The start of the frame on the clock (the app sets it before the work), for [`Self::frame_done`].
    pub fn frame_started(&mut self, now_ms: f64) {
        // UTC unix time of the clock now: the timeline's stamps stay on the real calendar (SIMP-uic-51).
        if let Some(off) = crate::wall_offset_now() {
            self.timeline.set_wall_offset(off);
        }
        self.frame_t0 = now_ms;
    }

    /// New errors of the Lua runtime go to the log with the file, the line and the traceback, as `lua.error` (at most
    /// 2000 characters each, 8 a frame).
    fn log_lua_errors(last_seq: &mut u64, layer: &UiLayer) {
        let rt = layer.core().runtime();
        if rt.error_count() == *last_seq {
            return;
        }
        let mut fresh: Vec<_> = rt
            .recent_errors()
            .into_iter()
            .filter(|e| e.seq > *last_seq)
            .collect();
        let total = rt.error_count();
        let missed = total - *last_seq - fresh.len() as u64;
        *last_seq = total;
        let skipped = fresh.len().saturating_sub(8);
        fresh.truncate(8);
        for e in fresh {
            let text = e.error.full_text();
            let text: String = text.chars().take(2000).collect();
            tracing::error!(target: "lua.error", seq = e.seq, context = %e.context, "{text}");
        }
        if missed + skipped as u64 > 0 {
            tracing::error!(
                target: "lua.error",
                "{} more Lua errors in this frame were not logged",
                missed + skipped as u64
            );
        }
    }

    /// One frame. Fills `out` and returns whether there is anything to draw.
    pub fn begin_frame(&mut self, env: M2Env<'_>, out: &mut UiDrawList) -> bool {
        if self.timeline.enabled()
            && let Some(n) = env.net
        {
            let game = n.peek(|c| c.attached_game()).flatten();
            self.timeline
                .set_context(game.map_or(-1, i64::from), self.host.seat.borrow().local_id);
        }
        let drawn = self.begin_frame_inner(env, out);
        self.collect_timeline();
        drawn
    }

    fn begin_frame_inner(&mut self, env: M2Env<'_>, out: &mut UiDrawList) -> bool {
        let M2Env {
            now_ms,
            loader,
            size,
            net,
            clock,
            seat,
            ai_turn,
            seat_known,
            projection,
        } = env;
        if self.phase == M2Phase::Failed || size.0 == 0 || size.1 == 0 {
            return false;
        }
        let Some(loader) = loader else {
            self.fail("the original UI needs the assets; there are none".into());
            return false;
        };
        self.started_ms.get_or_insert(now_ms);
        self.clk = Some(Rc::clone(clock));
        *self.host.seat.borrow_mut() = seat;
        *self.host.ai_turn.borrow_mut() = ai_turn;
        self.window.set(size);
        self.host.visible.set(
            projection
                .as_ref()
                .and_then(|p| visible_cells(&p.view_proj)),
        );
        *self.projector.borrow_mut() = projection;

        // 1. fetches
        match self.ui_boot.update(loader) {
            BootState::Failed(m) => {
                self.fail(format!("the ui set: {m}"));
                return false;
            }
            BootState::Ready if self.ui_ready_ms.is_none() => {
                self.ui_ready_ms = Some(now_ms.saturating_sub(self.started_ms.unwrap_or(now_ms)));
            }
            _ => {}
        }
        self.data.update(loader, now_ms);
        if let Some(m) = self.data.failed() {
            let m = m.to_owned();
            self.fail(format!("the game data: {m}"));
            return false;
        }
        // 2. the network: replica messages and bodies in, the UI's messages out
        self.network(net, now_ms);
        if self.awaiting_game
            && net.is_some_and(|n| {
                matches!(
                    n.with_client(|c| c.phase()),
                    client_core::ClientPhase::Playing
                )
            })
        {
            self.awaiting_game = false;
        }
        // 3. the core
        if self.replica.is_none() && self.data.ready() && seat_known {
            self.make_replica(clock);
            if self.phase == M2Phase::Failed {
                return false;
            }
        }
        let live = self.pump_replica(now_ms);
        // 4. the UI layer, once the game is there
        if self.layer.is_none() {
            let store_ready = matches!(self.ui_boot.update(loader), BootState::Ready);
            if live && store_ready && self.replica.is_some() && !self.awaiting_game {
                self.make_layer(size);
                if self.phase == M2Phase::Failed {
                    return false;
                }
                self.up_ms = Some(now_ms.saturating_sub(self.started_ms.unwrap_or(now_ms)));
            } else {
                if !self.warned
                    && now_ms.saturating_sub(self.started_ms.unwrap_or(now_ms)) > JOIN_WARN_MS
                {
                    self.warned = true;
                    tracing::warn!(
                        live,
                        store_ready,
                        data = ?self.data.progress(),
                        "M2 is still not up after {} s",
                        JOIN_WARN_MS / 1000
                    );
                }
                return false;
            }
        }
        self.run_layer(now_ms, size, out)
    }

    fn make_replica(&mut self, clock: &Rc<dyn Clock>) {
        let t0 = clock.now_ms_f64();
        let host = GameDataHost::new(Arc::clone(&self.data.files));
        match Replica::new(host, self.replica_seat()) {
            Ok(mut r) => {
                r.set_clock(Box::new(SharedClock(Rc::clone(clock))));
                self.create_ms = clock.now_ms_f64() - t0;
                tracing::info!(ms = self.create_ms, "replica core created");
                self.replica = Some(Rc::new(RefCell::new(r)));
                self.phase = M2Phase::Joining;
            }
            Err(e) => self.fail(format!("the replica core: {e}")),
        }
    }

    /// The seat of this client's player in the game; the core takes it as the active player.
    fn replica_seat(&self) -> u8 {
        self.host.seat.borrow().local_id.clamp(0, 63) as u8
    }

    fn network(&mut self, net: Option<&NetHandle>, now_ms: u64) {
        let _slow = render::perf::slow::scope("m2.network");
        let Some(net) = net else { return };
        let msgs = net.with_client(|c| c.take_replica_msgs());
        if !msgs.is_empty() {
            match self.replica.as_ref().map(|r| r.try_borrow_mut()) {
                Some(Ok(mut r)) => {
                    r.engine_mut().set_seat(self.replica_seat());
                    for m in msgs {
                        r.on_msg(m);
                    }
                }
                // No replica yet, or it is busy: the messages wait (a dropped journal batch is a gap and a resync).
                _ => self.backlog.extend(msgs),
            }
        }
        self.chat_and_kicks(net);
        let Some(rep) = self.replica.clone() else {
            return;
        };
        let Ok(mut r) = rep.try_borrow_mut() else {
            return;
        };
        if !self.backlog.is_empty() {
            for m in std::mem::take(&mut self.backlog) {
                r.on_msg(m);
            }
        }
        for b in r.wanted_blobs() {
            self.blob_wanted.insert(b.hash);
            net.with_client(|c| c.fetch_replica_blob(b.hash));
        }
        for (hash, body) in net.with_client(|c| c.poll_replica_blobs()) {
            self.blob_wanted.remove(&hash);
            match body {
                Some(bytes) => r.blob(hash, bytes),
                None => r.blob_failed(hash),
            }
        }
        // The UI's messages go out; what cannot be sent now stays (AC-G2).
        let mut out = std::mem::take(&mut self.resync_flush);
        out.extend(r.take_outgoing());
        let mut order_marks: Vec<protocol::CmdId> = Vec::new();
        let turn_now = r.turn();
        let mut stale_dropped = 0u32;
        if !out.is_empty() {
            let mut back = Vec::new();
            for o in out {
                let sent = net.with(|core| match &o {
                    Outgoing::GcNet(bytes) => {
                        match core.send_gc_net_at(bytes.clone(), turn_now) {
                            Ok(()) => true,
                            Err(client_core::OrderRefused::TurnChanged) => {
                                // Given on a turn the server has left: it does nothing, it is not kept (B1).
                                stale_dropped += 1;
                                true
                            }
                            Err(_) => false,
                        }
                    }
                    Outgoing::EndTurn => {
                        match core
                            .order_at(protocol::Command::EndTurn { force: false }, Some(turn_now))
                        {
                            Ok(id) => {
                                if self.end_turn_orders.len() >= 8 {
                                    self.end_turn_orders.remove(0);
                                }
                                self.end_turn_orders.push(id);
                                order_marks.push(id);
                                true
                            }
                            Err(_) => false,
                        }
                    }
                    Outgoing::CancelEndTurn => core
                        .order_at(protocol::Command::CancelEndTurn, Some(turn_now))
                        .is_ok(),
                    Outgoing::Resync {
                        epoch,
                        have_raw_hash,
                        at_jseq,
                        reason,
                    } => core
                        .send_gc_resync(*epoch, *have_raw_hash, *at_jseq, *reason)
                        .is_ok(),
                });
                if !sent {
                    back.push(o);
                }
            }
            if !back.is_empty() {
                r.unsend(back);
            }
        }
        if stale_dropped > 0 {
            tracing::info!(
                target: "m2",
                "{stale_dropped} UI message(s) given on turn {turn_now} dropped: the server is in a later turn"
            );
        }
        if !order_marks.is_empty() && self.timeline.enabled() {
            let at = self.clk.as_ref().map_or(0.0, |c| c.now_ms_f64());
            for id in order_marks {
                tracing::debug!(id, turn = turn_now, "cmd.send: END_TURN sent");
                self.timeline.record(
                    turn_now,
                    "c.order_sent",
                    at,
                    -1.0,
                    vec![("cmd", f64::from(id))],
                );
            }
        }
        let _ = now_ms;
    }

    /// Applies what is due; `true` when the replica is `Live`.
    fn pump_replica(&mut self, _now_ms: u64) -> bool {
        let _slow = render::perf::slow::scope("m2.pump_replica");
        self.v2_steps.clear();
        let Some(rep) = self.replica.clone() else {
            return false;
        };
        let Ok(mut r) = rep.try_borrow_mut() else {
            return self.phase == M2Phase::Running;
        };
        // A turn change is one step of seconds on a big game: the pump does at most one stage per frame.
        for _ in 0..16 {
            let step = r.pump(PUMP_BUDGET_MS);
            self.v2_steps.push(step);
            if matches!(
                step,
                ReplicaStep::Loaded { .. } | ReplicaStep::TurnSwitched { .. }
            ) {
                tracing::info!(?step, "replica loaded a game");
            }
            if !matches!(step, ReplicaStep::Working) {
                break;
            }
        }
        r.state() == ReplicaState::Live
    }

    fn make_layer(&mut self, size: (u32, u32)) {
        let t0 = self.clk.as_ref().map_or(0.0, |c| c.now_ms_f64());
        let Some(store) = self.ui_boot.store() else {
            return;
        };
        let Some(rep) = self.replica.clone() else {
            return;
        };
        let lang = LayerConfig::m2().lang;
        let localizer = match ui_client::load_localizer(&store, &lang) {
            Ok(l) => Some(l),
            Err(e) => {
                tracing::warn!("the texts for Locale are not loaded: {e}");
                None
            }
        };
        self.v2_loc.clone_from(&localizer);
        let cfg = ExeConfig {
            localizer: localizer.clone(),
            ..ExeConfig::default()
        };
        let exe = Rc::new(RefCell::new(ExeApi::new(cfg, self.host.clone())));
        if let Some(c) = &self.clk {
            exe.borrow().set_clock(Rc::clone(c));
        }
        let setup = exe.borrow().base_setup();
        // SIMPLIFIED(SIMP-exe-61): the camera's projection for WorldAnchor, on the ground plane.
        let window = Rc::clone(&self.window);
        let proj = Rc::clone(&self.projector);
        let projector: Projector = Rc::new(move |w: [f32; 3]| {
            let p = (*proj.borrow())?;
            // Lua world: x east, y north, z up; the renderer: x east, y up, -z north.
            let v = p.view_proj * glam::Vec4::new(w[0], w[2], -w[1], 1.0);
            if v.w <= 0.0 {
                return None;
            }
            let (ndc_x, ndc_y) = (v.x / v.w, v.y / v.w);
            let (pw, ph) = window.get();
            let px = (ndc_x * 0.5 + 0.5) * pw as f32;
            let py = (1.0 - (ndc_y * 0.5 + 0.5)) * ph as f32;
            Some([px / p.ui_scale, py / p.ui_scale])
        });
        let allowed = Rc::clone(&self.allowed);
        let exe_bind = Rc::clone(&exe);
        let exe_after = Rc::clone(&exe);
        let game: Rc<RefCell<dyn replica::GameSide>> = rep;
        let hl = Rc::clone(&self.hl);
        let tap_host = Rc::clone(&self.host);
        let sounds = Rc::clone(&self.sounds);
        let hooks = LayerHooks {
            localizer,
            base_setup: Some(Box::new(setup)),
            context_filter: Some(Rc::new(move |name: &str| {
                allowed.contains(&name.to_ascii_lowercase())
            })),
            before_root: Some(Box::new(move |core| {
                let mut e = exe_bind.borrow_mut();
                e.attach_runtime(core.runtime());
                match core.runtime().create_context("exe-api-bind", |_, _| Ok(())) {
                    Ok(ctx) => {
                        let tap_hl = Rc::clone(&hl);
                        let tap_host = Rc::clone(&tap_host);
                        let mut tapped = TAPPED_EVENTS.to_vec();
                        tapped.push("AudioPlay2DSound");
                        tapped.push("MinimapClickedEvent");
                        tapped.extend(CAMERA_EVENTS.iter().map(|(n, _, _)| *n));
                        let sounds = Rc::clone(&sounds);
                        e.set_event_tap(
                            &tapped,
                            Rc::new(move |n: &str, a: &[exe_api::Arg]| {
                                if let Some(&(_, key, pressed)) =
                                    CAMERA_EVENTS.iter().find(|(e, _, _)| *e == n)
                                {
                                    let mut q = tap_host.camera_keys.borrow_mut();
                                    if q.len() < 64 {
                                        q.push(camera_key(key, pressed));
                                    }
                                } else if n == "MinimapClickedEvent" {
                                    // K6: the click, in pixels of the minimap control.
                                    if let (
                                        Some(exe_api::Arg::Num(x)),
                                        Some(exe_api::Arg::Num(y)),
                                    ) = (a.first(), a.get(1))
                                    {
                                        tap_host.minimap_click.set(Some((*x as f32, *y as f32)));
                                    }
                                } else if n == "AudioPlay2DSound" {
                                    if let Some(exe_api::Arg::Str(s)) = a.first() {
                                        let mut q = sounds.borrow_mut();
                                        if q.len() < 64 {
                                            q.push(s.clone());
                                        }
                                    }
                                } else {
                                    tap_hl.borrow_mut().on_event(n, a)
                                }
                            }),
                        );
                        e.bind_game(game, ctx.lua());
                    }
                    Err(err) => tracing::error!("exe-api bind: {err}"),
                }
            })),
            after_root: Some(Box::new(move |core| {
                if let Some(code) = crate::m2_debug_lua() {
                    match core.runtime().create_context("m2-debug", |_, _| Ok(())) {
                        Ok(ctx) => {
                            if let Err(e) = ctx.lua().load(&code).set_name("=m2_debug").exec() {
                                tracing::warn!("luadbg: {e}");
                            }
                        }
                        Err(e) => tracing::warn!("luadbg context: {e}"),
                    }
                }
                // The first dispatch announces the game and fires `SequenceGameInitComplete`.
                exe_after.borrow_mut().dispatch(core.runtime());
            })),
        };
        match UiLayer::with_hooks(store, &LayerConfig::m2(), size, hooks) {
            Ok(layer) => {
                layer.set_projector(Some(Rc::new({
                    let proj = Rc::clone(&self.projector);
                    let window = Rc::clone(&self.window);
                    move |w: [f32; 3]| {
                        let p = (*proj.borrow())?;
                        let v = p.view_proj * glam::Vec4::new(w[0], w[2], -w[1], 1.0);
                        if v.w <= 0.0 {
                            return None;
                        }
                        let (pw, ph) = window.get();
                        Some([
                            (v.x / v.w * 0.5 + 0.5) * pw as f32 / p.ui_scale,
                            (1.0 - (v.y / v.w * 0.5 + 0.5)) * ph as f32 / p.ui_scale,
                        ])
                    }
                })));
                let _ = projector;
                self.root_ms = self.clk.as_ref().map_or(0.0, |c| c.now_ms_f64()) - t0;
                tracing::info!(
                    root_ms = self.root_ms,
                    refused = ?layer.core().filtered_contexts().len(),
                    "the original UI (M2) is running"
                );
                self.exe = Some(exe);
                self.layer = Some(layer);
                self.live_ms = Some(0);
                self.dump_armed = crate::m2_debug_lua().is_some_and(|c| c.starts_with("--dump"));
                self.size = size;
                self.phase = M2Phase::Running;
            }
            Err(e) => self.fail(e.to_string()),
        }
    }

    fn run_layer(&mut self, now_ms: u64, size: (u32, u32), out: &mut UiDrawList) -> bool {
        let (Some(layer), Some(exe)) = (self.layer.as_mut(), self.exe.clone()) else {
            return false;
        };
        let t0 = self.clk.as_ref().map_or(0.0, |c| c.now_ms_f64());
        for code in crate::take_m2_lua() {
            match layer
                .core()
                .runtime()
                .create_context("m2-lua", |_, _| Ok(()))
            {
                Ok(ctx) => {
                    if let Err(e) = ctx.lua().load(&code).set_name("=m2_lua").exec() {
                        tracing::warn!("m2 lua: {e}");
                    }
                }
                Err(e) => tracing::warn!("m2 lua context: {e}"),
            }
        }
        if let Ok(mut e) = exe.try_borrow_mut() {
            let _slow = render::perf::slow::scope("m2.dispatch");
            e.dispatch(layer.core().runtime());
            e.tick(now_ms as f64 / 1000.0);
        }
        if self.size != size {
            self.size = size;
            layer.resize(size.0, size.1);
        }
        layer.frame(now_ms, out);
        Self::log_lua_errors(&mut self.lua_error_seq, layer);
        // Dev (`?luadbg=--dump...`): the control tree once, 20 s after the first frame.
        if std::mem::take(&mut self.dump_armed) {
            // `--dump=NN` dumps NN s after the first frame (default 20).
            let secs = crate::m2_debug_lua()
                .and_then(|c| {
                    c.strip_prefix("--dump=").map(|r| {
                        r.chars()
                            .take_while(char::is_ascii_digit)
                            .collect::<String>()
                    })
                })
                .and_then(|d| d.parse::<u64>().ok())
                .unwrap_or(20);
            self.dump_at = Some(now_ms + secs * 1000);
        }
        if self.dump_at.is_some_and(|t| now_ms >= t) {
            self.dump_at = None;
            tracing::info!(target: "m2.tree", "{}", layer.core().dump_tree());
        }
        let t1 = self.clk.as_ref().map_or(0.0, |c| c.now_ms_f64());
        self.last_frame_us.set(((t1 - t0) * 1000.0).max(0.0) as u64);
        true
    }

    /// After the renderer drew: uploads what the frame asked for.
    pub fn resolve_textures(
        &mut self,
        wanted: &[draw_list::TextureId],
    ) -> Vec<ui_client::ReadyTexture> {
        match &self.layer {
            Some(l) => {
                let r = l.resolve_textures(wanted);
                for id in wanted
                    .iter()
                    .filter(|w| r.iter().all(|x| x.id != **w) && !l.store().is_loading(**w))
                {
                    if self.unresolved.insert(id.0) {
                        tracing::warn!(
                            texture = id.0,
                            "the renderer wants a ui texture the set did not give"
                        );
                    }
                }
                r
            }
            None => Vec::new(),
        }
    }

    /// `__civ5stats.{ui,replica,exe}`: refreshed about once a second.
    pub fn stats(&mut self, now_ms: u64) -> Option<&M2Stats> {
        if now_ms.saturating_sub(self.last_stats_ms) < 1000 && !self.stats.groups.is_empty() {
            return None;
        }
        self.last_stats_ms = now_ms;
        let mut groups: Vec<(&'static str, Vec<(String, f64)>)> = Vec::new();
        let n = |k: &str, v: f64| (k.to_owned(), v);
        groups.push((
            "m2",
            vec![
                n(
                    "phase",
                    match self.phase {
                        M2Phase::Loading => 0.0,
                        M2Phase::Joining => 1.0,
                        M2Phase::Running => 2.0,
                        M2Phase::Failed => -1.0,
                    },
                ),
                n("create_ms", self.create_ms),
                n("root_ms", self.root_ms),
                n("ui_ready_ms", self.ui_ready_ms.map_or(-1.0, |v| v as f64)),
                n("up_ms", self.up_ms.map_or(-1.0, |v| v as f64)),
                n("data_done", self.data.progress().0 as f64),
                n("data_total", self.data.progress().1 as f64),
                n("chat_in", self.chat_in as f64),
                n("chat_sent", self.chat_sent as f64),
                n("kicks_sent", self.kicks_sent as f64),
                n("saves_sent", self.saves_sent as f64),
                n("left", f64::from(u8::from(self.left))),
                n("minimap_clicks", self.minimap_clicks as f64),
                n(
                    "turn_complete",
                    if self
                        .exe
                        .as_ref()
                        .is_some_and(|e| e.try_borrow().is_ok_and(|e| e.turn_complete_sent()))
                    {
                        1.0
                    } else {
                        0.0
                    },
                ),
                n("minimap_w", f64::from(self.minimap_seen.0)),
                n("minimap_h", f64::from(self.minimap_seen.1)),
                n("minimap_has_map", if self.minimap_map { 1.0 } else { 0.0 }),
                n("hotkeys", self.hotkeys as f64),
                n("ring_x", self.ring.map_or(-1.0, |c| f64::from(c.0))),
                n("ring_y", self.ring.map_or(-1.0, |c| f64::from(c.1))),
            ],
        ));
        if let Some(l) = &self.layer {
            let s = l.stats();
            self.logged_errors = s.core.lua_errors;
            if s.tex_missing != self.logged_missing {
                self.logged_missing = s.tex_missing;
                tracing::warn!(
                    target: "m2.tex",
                    "{} textures the UI asked for are not in the set or have no size in its index: {}",
                    s.tex_missing,
                    l.store().missing_textures().join(", ")
                );
            }
            let (refused, tex_missing) = (
                l.core().filtered_contexts().len() as f64,
                s.tex_missing as f64,
            );
            groups.push((
                "ui",
                vec![
                    n("lua_errors", s.core.lua_errors as f64),
                    n("xml_warnings", s.core.xml_warnings as f64),
                    n("controls", s.core.controls as f64),
                    n("layout_us", s.core.layout_us as f64),
                    n(
                        "lua_us",
                        self.exe_stats().map_or(0.0, |e| e.dispatch_us as f64),
                    ),
                    n("frame_us", self.last_frame_us.get() as f64),
                    n("draw_cmds", s.draw_cmds as f64),
                    n("vertices", s.vertices as f64),
                    n("filtered_contexts", refused),
                    n("tex_missing", tex_missing),
                    n("tex_lazy_requested", s.tex_lazy.requested as f64),
                    n("tex_lazy_loaded", s.tex_lazy.loaded as f64),
                    n("tex_lazy_failed", s.tex_lazy.failed as f64),
                    n("tex_lazy_pending", s.tex_lazy.pending as f64),
                ],
            ));
        }
        if let Some(r) = &self.replica
            && let Ok(r) = r.try_borrow()
        {
            let s = r.stats();
            groups.push((
                "replica",
                vec![
                    n("applied", s.applied as f64),
                    n("diverged", s.diverged as f64),
                    n("resyncs", s.resyncs as f64),
                    n("checks_skipped", s.checks_skipped as f64),
                    n("checks_run", s.checks_run as f64),
                    n("net_sent", s.net_sent as f64),
                    n("net_rejected", s.net_rejected as f64),
                    n("turn_reopened", s.turn_reopened as f64),
                    n("loads", s.loads as f64),
                    n("turn_switch_ms", s.turn_switch_ms),
                    n("turn_wait_ms", s.turn_wait_ms),
                    n("turn_patch", s.turn_patch as f64),
                    n("turn_snapshot", s.turn_snapshot as f64),
                    n("patch_fallbacks", s.patch_fallbacks as f64),
                    n("last_body_bytes", s.last_body_bytes as f64),
                    n("join_ms", s.join_ms),
                    n("memory_mb", f64::from(s.memory_mb)),
                    n("queued", f64::from(s.queued)),
                    n(
                        "live",
                        if r.state() == ReplicaState::Live {
                            1.0
                        } else {
                            0.0
                        },
                    ),
                    n("turn", f64::from(r.turn())),
                ],
            ));
        }
        if let Some(e) = self.exe_stats() {
            let mut v = vec![
                n("events", e.events as f64),
                n("dispatch_us", e.dispatch_us as f64),
                n("simp_calls", e.simp_calls as f64),
                n("unmapped", e.unmapped as f64),
                n("game_events_in", e.game_events_in as f64),
                n("attach_errors", e.attach_errors as f64),
                n("queued", e.queued as f64),
                n("entity_units", self.entity_counts().0 as f64),
                n("entity_cities", self.entity_counts().1 as f64),
            ];
            for (name, calls) in &e.simp_by_name {
                v.push((format!("simp.{name}"), *calls as f64));
            }
            groups.push(("exe", v));
        }
        // `__civ5stats.turn`: the last turns' timelines, `"<turn>.<field>"` (the page nests them by turn).
        {
            let mut v: Vec<(String, f64)> = vec![
                n("level", f64::from(self.timeline.level() as u8)),
                n(
                    "open_turn",
                    self.timeline.open_turn().map_or(-1.0, f64::from),
                ),
            ];
            for (turn, fields) in self.timeline.stat_rows() {
                for (k, val) in fields {
                    v.push((format!("{turn}.{k}"), val));
                }
            }
            groups.push(("turn", v));
        }
        // The camera, so a scripted player can aim the mouse at a plot (the playtest).
        if let Some(p) = *self.projector.borrow() {
            let mut cam: Vec<(String, f64)> = p
                .view_proj
                .to_cols_array()
                .iter()
                .enumerate()
                .map(|(i, v)| (format!("m{i}"), f64::from(*v)))
                .collect();
            let (w, h) = self.window.get();
            cam.push(n("width", f64::from(w)));
            cam.push(n("height", f64::from(h)));
            cam.push(n("ui_scale", f64::from(p.ui_scale)));
            cam.push(n("focus_x", f64::from(p.focus[0])));
            cam.push(n("focus_z", f64::from(p.focus[1])));
            cam.push(n("level", f64::from(p.level)));
            groups.push(("camera", cam));
        }
        self.stats = M2Stats { groups };
        Some(&self.stats)
    }

    fn entity_counts(&self) -> (usize, usize) {
        self.exe
            .as_ref()
            .and_then(|e| e.try_borrow().ok().map(|e| e.entity_counts()))
            .unwrap_or_default()
    }

    fn exe_stats(&self) -> Option<exe_api::ExeStats> {
        self.exe.as_ref()?.try_borrow().ok().map(|e| e.stats())
    }

    /// The names of the `SIMP` stubs the K1-K3 scenario called.
    pub fn simp_calls(&self) -> Vec<(String, u64)> {
        self.exe_stats()
            .map(|e| e.simp_by_name.into_iter().collect())
            .unwrap_or_default()
    }
}

// ---- WO-G (M4): the game menu (save, leave) and the volume of the options menu ----

impl M2Ui {
    /// The volume knobs of the options menu that moved since the last call: `(knob, 0..=1)`, each the gain of its bus.
    pub fn take_knobs(&self) -> Vec<(String, f32)> {
        std::mem::take(&mut *self.host.knobs.borrow_mut())
    }

    /// The events the music listens to since the last call, oldest first.
    pub fn take_music_events(&self) -> Vec<exe_api::music::MusicEvent> {
        std::mem::take(&mut *self.host.music_events.borrow_mut())
    }

    /// The game as the music reads it (`None` before the exe API runs a game).
    pub fn music_state(&self) -> Option<exe_api::music::MusicState> {
        self.exe.as_ref()?.try_borrow().ok()?.music_state()
    }

    /// The player confirmed "Exit to main menu" (`Events.ExitToMainMenu`) and the game must be left (once). The
    /// request is taken when it goes out (`chat_and_kicks`); this tells the app to drop the game's UI afterwards.
    pub fn left_game(&self) -> bool {
        self.left
    }

    /// The game of this layer is over and the player may join another. The core of the replica boots once per process,
    /// so it stays and takes the next game's snapshot in place (with the seat of the new game); the Lua UI of the old
    /// game goes (its popups, notifications and flags are of that game) and a new one is made over the same core once
    /// the next game is playing.
    pub fn new_game(&mut self) {
        self.left = false;
        self.backlog.clear();
        self.resync_flush.clear();
        self.blob_wanted.clear();
        self.chat_seen = 0;
        self.host.exit_req.set(false);
        self.host.save_req.set(false);
        self.clear_game_leftovers();
        if let Some(exe) = self.exe.take()
            && let Ok(mut e) = exe.try_borrow_mut()
        {
            // The API no longer holds the replica (the Lua state of the old UI holds the API).
            e.unbind_game();
        }
        self.layer = None;
        self.awaiting_game = true;
        if self.phase == M2Phase::Running {
            self.phase = M2Phase::Joining;
        }
    }

    /// Everything of the last game that lives outside the Lua layer and the replica: the highlights the Lua set, the
    /// requests and sounds the layer queued for the app, the orders waiting for an answer, the heights of its map. The
    /// next game starts with none of it (a second game in the page is the same as the first after a fresh load).
    fn clear_game_leftovers(&mut self) {
        self.hl.borrow_mut().clear();
        self.sounds.borrow_mut().clear();
        let h = &self.host;
        h.look.set(None);
        h.place.set(None);
        h.city_view.set(None);
        h.ai_turn.borrow_mut().take();
        *h.heights.borrow_mut() = (0, 0, Vec::new());
        h.heights_rev.set(None);
        h.minimap_click.set(None);
        h.chat_out.borrow_mut().clear();
        h.kicks.borrow_mut().clear();
        h.grid.set(None);
        h.visible.set(None);
        h.notification_sounds.borrow_mut().clear();
        h.camera_keys.borrow_mut().clear();
        h.bridge.set(None);
        h.gameplay_fx.borrow_mut().clear();
        // The city ambience of the last game must not go on in the lobby.
        let mut q = h.exe_sounds.borrow_mut();
        q.clear();
        q.push(AMBIENCE_STOP.to_owned());
        drop(q);
        self.end_turn_orders.clear();
        self.saved_level = None;
        self.ring = None;
        self.unresolved.clear();
        self.minimap_seen = (0, 0);
        self.minimap_map = false;
    }

    /// K20: the host's "Save game" and the exit to the lobby go to the server.
    fn menu_requests(&mut self, net: &NetHandle) {
        if let Some(list) = net
            .peek(|c| {
                c.lobby_state().map(|l| {
                    l.saves
                        .iter()
                        .map(|s| SavedGame {
                            name: s.name.clone(),
                            turn: s.turn,
                            saved_at_ms: s.saved_at_ms,
                        })
                        .collect::<Vec<_>>()
                })
            })
            .flatten()
            && *self.host.saves.borrow() != list
        {
            *self.host.saves.borrow_mut() = list;
        }
        if self.host.save_req.replace(false)
            && net
                .with(|core| core.game_ctl(protocol::GameCtl::Save))
                .is_ok()
        {
            self.saves_sent += 1;
        }
        if self.host.exit_req.replace(false)
            && net
                .with(|core| core.game_ctl(protocol::GameCtl::Leave))
                .is_ok()
        {
            self.left = true;
        }
    }
}

// ---- WO-C (M3): chat, kicks and the minimap of the corners ----

impl M2Ui {
    /// The final fate of an order of this seat (`ClientEvent::OrderAnswered` / `OrderRolledBack`): an end of turn
    /// the server refused or lost leaves the turn running, so the UI takes its "turn complete" back and draws the
    /// end-turn button again instead of "Waiting for players" (live game 931, 2026-10-30). `refused`: the order
    /// was not applied.
    pub fn on_order_done(&mut self, id: protocol::CmdId, refused: bool) {
        let Some(at) = self.end_turn_orders.iter().position(|&x| x == id) else {
            return;
        };
        self.end_turn_orders.remove(at);
        if !refused {
            return;
        }
        let taken_back = self
            .exe
            .as_ref()
            .and_then(|e| e.try_borrow().ok().map(|e| e.turn_complete_refused()));
        tracing::info!(target: "m2", "the server refused the end of turn (order {id}): the turn goes on (flag was set: {taken_back:?})");
    }

    /// K5 chat: the lines the server delivered since the last frame go to `Events.GameMessageChat`; the lines the
    /// layer wants to send (`Network.SendChat`) and the kicks it asks for (`Matchmaking.KickPlayer`) go out.
    fn chat_and_kicks(&mut self, net: &NetHandle) {
        // Out: chat and kicks (kept in the host until the line is up).
        let out = std::mem::take(&mut *self.host.chat_out.borrow_mut());
        for (to, text) in out {
            let to = match to {
                Some(p) if (0..=255).contains(&p) => {
                    protocol::ChatTo::Player(p as protocol::PlayerId)
                }
                _ => protocol::ChatTo::All,
            };
            if net.with(|core| core.chat(to, &text)).is_ok() {
                self.chat_sent += 1;
            }
        }
        self.menu_requests(net);
        let kicks = std::mem::take(&mut *self.host.kicks.borrow_mut());
        for p in kicks {
            if (0..=255).contains(&p)
                && net
                    .with(|core| {
                        core.game_ctl(protocol::GameCtl::Kick {
                            player: p as protocol::PlayerId,
                        })
                    })
                    .is_ok()
            {
                self.kicks_sent += 1;
            }
        }
        // In: new lines, once the layer has its `Events`.
        let Some(exe) = self.exe.clone() else { return };
        let turn = self
            .replica
            .as_ref()
            .and_then(|r| r.try_borrow().ok().map(|r| r.turn()))
            .unwrap_or(0);
        let (total, fresh) = net
            .peek(|c| {
                let total = c.chat_total();
                let lines = c.chat_lines();
                let new = ((total - self.chat_seen.min(total)) as usize).min(lines.len());
                (total, lines[lines.len() - new..].to_vec())
            })
            .unwrap_or((self.chat_seen, Vec::new()));
        if fresh.is_empty() {
            self.chat_seen = total;
            return;
        }
        let Ok(e) = exe.try_borrow() else { return };
        self.chat_seen = total;
        for entry in fresh {
            let l = &entry.line;
            let from = l.from_player.map_or(-1, i32::from);
            let to = match l.to {
                protocol::ChatTo::All => None,
                protocol::ChatTo::Player(p) => Some(i32::from(p)),
            };
            tracing::info!(target: "m2.chat", "chat from {} ({from}) to {:?}: {}", l.from, to, l.text);
            e.chat_received(from, to, &l.text, turn, (l.server_ms / 1000) as i32);
            self.chat_in += 1;
        }
    }

    /// K6: the renderer's minimap texture changed (size): the panel is told again.
    pub fn set_minimap_size(&mut self, size: (u32, u32), has_map: bool) {
        self.minimap_map = has_map;
        self.minimap_seen = size;
        if size.0 == 0 || size.1 == 0 || self.host.minimap.get() == size {
            return;
        }
        self.host.minimap.set(size);
        if let Some(e) = &self.exe
            && let Ok(e) = e.try_borrow()
        {
            e.broadcast_minimap();
        }
    }

    /// The revision of the relief's heights `GridToWorld` was given last (`None`: none yet).
    pub fn plot_heights_revision(&self) -> Option<u64> {
        self.host.heights_rev.get()
    }

    /// The relief's height at the centre of every plot (row by row), made at the renderer's revision `rev`.
    pub fn set_plot_heights(&self, rev: Option<u64>, width: u32, height: u32, heights: Vec<f32>) {
        self.host.heights_rev.set(rev);
        *self.host.heights.borrow_mut() = (width, height, heights);
    }

    /// `UI.SetGridVisibleMode` since the last call.
    pub fn take_grid(&self) -> Option<bool> {
        self.host.grid.take()
    }

    /// K6: a click on the minimap panel since the last call, as the share of the control (0..=65535 each way).
    pub fn take_minimap_click(&mut self) -> Option<(u16, u16)> {
        let (x, y) = self.host.minimap_click.take()?;
        let (w, h) = self.host.minimap.get();
        self.minimap_clicks += 1;
        let u = (x / w.max(1) as f32).clamp(0.0, 1.0) * 65535.0;
        let v = (y / h.max(1) as f32).clamp(0.0, 1.0) * 65535.0;
        Some((u as u16, v as u16))
    }
}

/// The modifier keys held (ctrl, shift, alt) as an input event tells them. Every key, mouse and wheel event carries them, so
/// a key release the window never saw (Alt+Tab away with Alt down) does not leave `UI.AltKeyDown` true until the next key;
/// that made every order of the city screen a repeating one (the Alt flag of `Game.CityPushOrder` is "repeat").
/// Losing the focus releases all of them.
fn held_modifiers(ev: &InputEvent) -> Option<(bool, bool, bool)> {
    let m = match ev {
        InputEvent::Key(k) => k.modifiers,
        InputEvent::Pointer(p) => p.modifiers,
        InputEvent::Wheel(w) => w.modifiers,
        InputEvent::Focused(false) => return Some((false, false, false)),
        _ => return None,
    };
    Some((m.ctrl, m.shift, m.alt))
}

#[cfg(test)]
mod modifier_tests {
    use super::*;
    use platform::{
        Key, KeyEvent, Modifiers, PhysicalPosition, PointerEvent, PointerId, PointerKind,
        PointerPhase,
    };

    #[test]
    fn a_click_or_a_lost_focus_refreshes_the_held_modifiers() {
        let alt = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        let key = InputEvent::Key(KeyEvent {
            key: Key::Unidentified,
            code: None,
            pressed: true,
            repeat: false,
            modifiers: alt,
        });
        assert_eq!(held_modifiers(&key), Some((false, false, true)));
        let click = |modifiers| {
            InputEvent::Pointer(PointerEvent {
                id: PointerId::MOUSE,
                kind: PointerKind::Mouse,
                phase: PointerPhase::Down,
                position: PhysicalPosition::default(),
                button: None,
                modifiers,
            })
        };
        assert_eq!(
            held_modifiers(&click(Modifiers::default())),
            Some((false, false, false))
        );
        assert_eq!(held_modifiers(&click(alt)), Some((false, false, true)));
        assert_eq!(
            held_modifiers(&InputEvent::Focused(false)),
            Some((false, false, false))
        );
        assert_eq!(held_modifiers(&InputEvent::Focused(true)), None);
        assert_eq!(held_modifiers(&InputEvent::Text("a".into())), None);
    }
}

#[cfg(test)]
mod visible_cells_tests {
    use super::visible_cells;

    #[test]
    fn the_rectangle_holds_the_plot_the_camera_looks_at_and_follows_it() {
        let at = |x: i32, y: i32| {
            let cam = render::GameCamera::new(render::hexgrid::cell_center(x, y));
            visible_cells(&cam.view_proj(16.0 / 9.0)).expect("a rectangle")
        };
        let r = at(40, 25);
        assert!(r[0] < 40 && 40 < r[2] && r[1] < 25 && 25 < r[3], "{r:?}");
        // A view of a few dozen cells, not the map.
        assert!(r[2] - r[0] < 120 && r[3] - r[1] < 120, "{r:?}");
        let east = at(60, 25);
        assert_eq!(east[0] - r[0], 20, "{east:?} {r:?}");
        assert_eq!(east[1], r[1]);
    }
}

#[cfg(test)]
mod new_game_tests {
    use super::*;

    /// A second game in the same page starts with nothing of the first one outside the Lua layer and the replica:
    /// the queues the layer filled for the app, the pending orders, the sounds (the city ambience is told to stop).
    #[test]
    fn a_new_game_drops_what_the_last_one_left_for_the_app() {
        let mut m2 = M2Ui::new(Rc::new(platform::MemoryStore::default()), 1.0);
        let h = Rc::clone(&m2.host);
        h.look.set(Some((3, 4)));
        h.city_view.set(Some(true));
        h.minimap_click.set(Some((1.0, 2.0)));
        h.chat_out.borrow_mut().push((None, "hi".into()));
        h.kicks.borrow_mut().push(2);
        h.notification_sounds
            .borrow_mut()
            .push("NOTIFICATION_X".into());
        h.gameplay_fx.borrow_mut().push((5, 6));
        h.camera_keys
            .borrow_mut()
            .push(camera_key(platform::NamedKey::Escape, true));
        h.exe_sounds
            .borrow_mut()
            .push("ambience:AS2D_AMBIENCE_CITY".into());
        h.heights_rev.set(Some(7));
        *h.heights.borrow_mut() = (2, 1, vec![1.0, 2.0]);
        m2.sounds.borrow_mut().push("AS2D_X".into());
        m2.end_turn_orders.push(9);
        m2.saved_level = Some(3.0);
        m2.ring = Some((1, 1));
        m2.new_game();
        assert_eq!(h.look.get(), None);
        assert_eq!(h.city_view.get(), None);
        assert_eq!(h.minimap_click.get(), None);
        assert!(h.chat_out.borrow().is_empty() && h.kicks.borrow().is_empty());
        assert!(h.notification_sounds.borrow().is_empty() && h.gameplay_fx.borrow().is_empty());
        assert!(h.camera_keys.borrow().is_empty() && m2.sounds.borrow().is_empty());
        assert_eq!(h.heights_rev.get(), None);
        assert_eq!(*h.heights.borrow(), (0, 0, Vec::new()));
        assert_eq!(*h.exe_sounds.borrow(), vec![AMBIENCE_STOP.to_owned()]);
        assert!(m2.end_turn_orders.is_empty());
        assert_eq!((m2.saved_level, m2.ring), (None, None));
        assert!(m2.awaiting_game);
    }
}
