//! The exe API of the original Civ5 UI: the globals the exe gives the Lua of the original UI, over the replica of
//! the game (milestone M2).
//!
//! The client's own globals (WO-5): `OptionsManager`, `Modding`, `ContentManager`, `PreGame` (client part),
//! `Mouse`/`Keys`/`KeyEvents`/`MouseEvents`/`PopupPriority`, the hex helpers, `Steam`, `Matchmaking`,
//! `InStrategicView` and `Locale`; the AC-A1 coverage report is in [`coverage`].
//!
//! The events and the bridge to the game (WO-4):
//! - `Events`, `LuaEvents`, `GameEvents` ([`events`]), fed by the engine callbacks of the replica
//!   ([`engine_events`], [`entities`]);
//! - `UI.*` ([`ui_api`]) with the popup queue ([`buddy`]), `Network.*` ([`network`]);
//! - the bridge ([`ExeApi::bind_game`], [`ExeApi::dispatch`]): the DLL registers `Players`, `Game`, `Map`,
//!   `GameInfo` in the same Lua state; events come back through [`GameSide::take_events`] (`replica::GameSide`).
//!
//! Frame order: network -> `Replica::pump` -> [`ExeApi::dispatch`] (the FIFO, once) -> `UiCore::update` -> draw.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use lua_sys::{Lua, Runtime, Table};
use platform::KeyValueStore;

pub mod auto_end;
pub mod bridge;
pub mod buddy;
pub mod content;
mod core;
pub mod corners;
pub mod coverage;
pub mod cycle;
pub mod engine_events;
pub mod entities;
pub mod enums;
pub mod event_table;
pub mod events;
pub mod hex;
pub mod hotkeys;
pub mod locale;
pub mod m3_woa;
pub mod m4_wod;
pub mod m4_woe;
pub mod m4_wof;
pub mod m4_wog;
pub mod modding;
pub mod music;
pub mod network;
pub mod options;
pub mod path;
pub mod pregame;
mod registry;
pub mod simp;
pub mod sound_lines;
pub mod stubs;
pub mod ui_api;
pub mod yields;

/// The contexts of milestone M2 (m2-contexts.txt: K1-K3 and the EUI core), by bare name.
pub fn m2_contexts() -> Vec<String> {
    [
        include_str!("../m2-contexts.txt"),
        include_str!("../m3-wob-contexts.txt"),
        include_str!("../m4-woe-contexts.txt"),
    ]
    .into_iter()
    .flat_map(str::lines)
    .map(str::trim)
    .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('['))
    .map(str::to_owned)
    .chain(corners::CONTEXTS.iter().map(|s| (*s).to_owned()))
    .chain(
        m3_woa::K8
            .iter()
            .chain(m3_woa::K18)
            .chain(m4_wod::K12)
            .chain(m4_wof::K16)
            .chain(m4_wof::K17)
            .map(|s| (*s).to_owned()),
    )
    .chain(
        m4_wog::K19
            .iter()
            .chain(m4_wog::K20)
            .map(|s| (*s).to_owned()),
    )
    // A window two work orders list (Demographics) loads once.
    .fold(Vec::new(), |mut v: Vec<String>, c| {
        if !v.contains(&c) {
            v.push(c);
        }
        v
    })
}

pub use content::{ContentPackage, ContentType};
pub use registry::{Registry, Status};

pub use crate::buddy::PopupInfo;
use crate::core::Core;
pub use crate::core::{
    Arg, ExeSpan, Family, SOUND_QUIET_MAX_FRAMES, TapFn, TempRoute, UnitSoundRow,
    city_ambience_name,
};
pub use crate::sound_lines::{EC_ATTACK, EC_ATTACK_CITY, EC_DEATH, EC_RUN, UnitClipCue};

/// The popup route of the trade visuals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PopupRoute {
    /// A route of the game, by `TradeConnection` index (the selected or hovered trade unit's): its plots are the
    /// core's `Q_TRADE_PATHS` of that id.
    Existing(i32),
    /// The caravan's target choice: the core's temporary route (9 plots of `route.count` at most) and the plot the
    /// UI asked it for.
    Temp {
        route: TempRoute,
        dest: Option<(i32, i32)>,
    },
}
pub use replica::{
    GameSide, NetMessage, PathNode, ReachFlags, TradePath, UiEvent, UiEventKind, UiOp, UiValue,
};

/// What the client tells `ExeApi` once, at creation.
#[derive(Clone)]
pub struct ExeConfig {
    /// The text engine behind `Locale.*`. `None` (tests): `Locale` answers with the key itself
    /// (SIMPLIFIED(SIMP-exe-16)).
    pub localizer: Option<Rc<loc::Localizer>>,
    /// The packages `ContentManager` reports as installed and active.
    pub packages: Vec<ContentPackage>,
    /// Seconds between two writes of the user data (`Modding.OpenUserData`) to the store.
    pub userdata_flush_secs: f64,
    /// Popup types (`ButtonPopupTypes`) that may be open several times (`RegisterPopupType(.., true)`).
    pub stack_popup_types: Vec<i32>,
    /// "Do not show popups" mode: popups wait until `process_delayed_popups`.
    pub dont_show_popups: bool,
    /// Fire `SequenceGameInitComplete` once, after the first announcement of the game (the contexts wait for it).
    pub fire_init_complete: bool,
}

impl Default for ExeConfig {
    fn default() -> Self {
        ExeConfig {
            localizer: None,
            packages: content::default_packages(),
            userdata_flush_secs: 1.0,
            stack_popup_types: Vec::new(),
            dont_show_popups: false,
            fire_init_complete: true,
        }
    }
}

/// The seat of this client in the game, as the lobby knows it (`Network.GetPingTime`,
/// `Matchmaking.*`, `PreGame.GetNickName`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SeatInfo {
    pub nick: String,
    /// This client's player slot (`Matchmaking.GetLocalID`).
    pub local_id: i32,
    /// The slot of the host (`Matchmaking.GetHostID`).
    pub host_id: i32,
    /// Ping to each remote seat in milliseconds (`Network.GetPingTime`), by player id.
    pub pings_ms: Vec<(i32, i32)>,
}

impl SeatInfo {
    /// Is this client the host of the game (`Matchmaking.IsHost`)?
    pub fn is_host(&self) -> bool {
        self.local_id == self.host_id
    }

    /// `Network.IsPlayerConnected`: this seat and every seat we have a ping to.
    pub fn connected(&self, player: i32) -> bool {
        player == self.local_id || self.pings_ms.iter().any(|(p, _)| *p == player)
    }

    /// `Network.GetPingTime`.
    pub fn ping(&self, player: i32) -> i32 {
        self.pings_ms
            .iter()
            .find(|(p, _)| *p == player)
            .map_or(0, |(_, ms)| *ms)
    }
}

/// The `setTurnActive` of the non-human civilizations the server's core did in the AI phase of `turn` (the turn that was
/// being ended), in order: `(player, ended)`; `ended` false is the activation (`AIProcessingStartedForPlayer`), true the
/// deactivation (`AIProcessingEndedForPlayer`). Cumulative: the exe API keeps its own cursor into it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AiTurnLog {
    pub turn: i32,
    pub events: Vec<(i32, bool)>,
}

/// `CITY_ZOOM_LEVEL_1/2/3` of GlobalDefines.xml: the camera height of the city view by city-distance high-water mark.
pub const CITY_ZOOM_HEIGHTS: [f32; 3] = [500.0, 700.0, 800.0];

/// What the app gives `ExeApi`.
pub trait ExeHost {
    /// Persistent strings: options (`civ5.options.*`) and user data (`civ5.userdata.*`).
    fn kv(&self) -> &dyn KeyValueStore;
    /// `UI.LookAt`: the camera goes to the plot.
    fn look_at(&self, x: i32, y: i32);
    /// Debug (`__exe_camera_place`, the golden scenario): the camera puts plot `(x, y)` where the screen fractions
    /// `(px, py)` are (0,0 the top left corner) at zoom `level` (`<= 0`: the zoom stays).
    fn camera_place(&self, _x: i32, _y: i32, _px: f32, _py: f32, _level: f32) {}
    /// `Events.GameplayFX` reached the exe's particle system: the "look here" effect (`ART_DEF_VEFFECT_LOOK_AT`) plays
    /// once at plot `(x, y)` (grid cells).
    fn gameplay_fx(&self, _x: i32, _y: i32) {}
    /// The city screen went up or down: the camera takes the city view (zoom) or gives it back (WO-A, K8).
    fn city_screen(&self, _up: bool) {}
    fn seat_info(&self) -> SeatInfo;
    /// The AI civilizations the server started and ended in the AI phase of the last turn it processed, in order, until
    /// the next AI phase replaces it; `None` before the server said anything (SIMP-exe-820).
    fn ai_turn(&self) -> Option<AiTurnLog> {
        None
    }
    /// Size of the minimap texture in pixels (`MinimapTextureBroadcastEvent`).
    fn minimap_size(&self) -> (u32, u32) {
        (320, 190)
    }
    /// `Network.SendChat`: a line to everybody (`None`) or to one player.
    fn send_chat(&self, _to_player: Option<i32>, _text: &str) {}
    /// `Matchmaking.KickPlayer`.
    fn kick_player(&self, _player: i32) {}
    /// The hex rectangle of the camera `(x0, y0, x1, y1)` in grid cells, upper bounds exclusive and unwrapped (a column
    /// may be below 0 or past the width on a wrapping map), a hex wider than the view on each side; `None` when the
    /// host has no camera (the yield icons then cover the whole map).
    fn visible_cells(&self) -> Option<[i32; 4]> {
        None
    }
    /// `UI.SetGridVisibleMode`: the hex grid of the world is shown or hidden.
    fn set_grid_visible(&self, _on: bool) {}
    /// `UI.SaveGame(name)` (the game menu's "Save game", the host only): the app asks the server for a save of the
    /// game (WO-G, K20). The name is not used (SIMP-exe-393).
    fn save_game(&self, _name: &str) {}
    /// `Events.ExitToMainMenu` (and `UserRequestClose`, `UI.ExitGame`): the player leaves the game for the lobby
    /// (WO-G, K20).
    fn exit_to_lobby(&self) {}
    /// A volume knob of the options menu moved (`SetVolumeKnobValue`): `knob` is one of [`m4_wog::KNOBS`] ("MUSIC",
    /// "SFX", "SPEECH", "AMBIENCE"), `value` 0..=1: the gain of that bus of the UI sounds (SIMP-exe-392).
    fn set_volume(&self, _knob: &str, _value: f32) {}
    /// The value of the knobs when the player never set them (the page's `?volume=`, SIMP-exe-190), 0..=1.
    fn volume_default(&self) -> f32 {
        0.5
    }
    /// A new notification of the seat arrived: the exe plays its sound (`NotificationSounds.xml`, by the type's name,
    /// e.g. `NOTIFICATION_GOODY` -> `AS2D_INTERFACE_ANCIENT_RUINS`).
    fn notification_sound(&self, _type_name: &str) {}
    /// A popup for the seat arrived: the exe plays its sound (`ButtonPopupSounds.xml`, by the type's name, e.g.
    /// `BUTTONPOPUP_NEW_ERA` -> `AS2D_INTERFACE_NEW_ERA`).
    fn popup_sound(&self, _type_name: &str) {}
    /// The seat's own unit or city was created: the exe plays the sound of the event (`EventSoundConnections.xml`:
    /// `UnitCreated`, `CityCreated`).
    fn event_sound(&self, _name: &str) {}
    /// The spoken line of a popup (re-b15 ITEM 5 P5.2: the new era's, the tech's, the wonder's quote), a 2D script that
    /// starts `delay_ms` from now; the line of the popup before ends.
    fn voice_over(&self, _script: &str, _delay_ms: u32) {}
    /// A city-state's popup: its greeting voice (`MinorCivSounds.xml`, by the `MINOR_CIV_*` type) plays instead of the
    /// popup's sound (`PlayMinorCivSound`).
    fn minor_civ_sound(&self, _minor_type: &str) {}
    /// A unit the seat sees did something the exe animates and the port does not (a move, an attack, a death): the
    /// sound rows of the clip the exe would play are played as if it had (re-b15 ITEM 4).
    fn unit_clip_sound(&self, _cue: UnitClipCue) {}
    /// The seat's unit was selected: the exe plays the unit's selection sound (`UnitGameplay2DScripts`, a 2D sound
    /// script such as `AS2D_SELECT_ARCHER`).
    fn unit_selection_sound(&self, _script: &str) {}
    /// A 2D sound script will probably be called soon (the selection sounds of a unit type the seat has on the map): the
    /// sound engine may fetch its files ahead, at low priority.
    fn sound_preload(&self, _script: &str) {}
    /// A 2D sound script plays now (the sounds the exe's particle system plays with an effect).
    fn effect_sound(&self, _script: &str) {}
    /// The city screen opened on a city (`Some(script)`: the looping ambience of the city, a 2D script chosen by the
    /// owner's era and the city's size) or closed (`None`: it stops).
    fn city_ambience(&self, _script: Option<&str>) {}
    /// The saves of the server the save menu lists (SIMP-exe-393).
    fn saved_games(&self) -> Vec<SavedGame> {
        Vec::new()
    }
    /// The height of the relief at the centre of plot `(x, y)` (grid coordinates; the Lua world's z, up), which the
    /// exe's `GridToWorld` / `HexToWorld` give as z; `None` (z = 0) when the host has no relief.
    fn plot_height(&self, _x: i32, _y: i32) -> Option<f32> {
        None
    }
    /// `GameplayBridgeChanged`: the active team's roads over rivers have bridges (`bridge`: it knows the technology
    /// with `BridgeBuilding`) and they are iron (`iron`: its era is `LAST_BRIDGE_ART_ERA` or later). Called when the
    /// state changed, and once when it first became known (`bridge.rs`).
    fn bridge_changed(&self, _bridge: bool, _iron: bool) {}
    /// An event the music listens to ran (a leader screen up or left, the game over screen).
    fn music_event(&self, _e: music::MusicEvent) {}
}

/// A save of the server, for the save menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedGame {
    pub name: String,
    pub turn: i32,
    /// Unix time in milliseconds.
    pub saved_at_ms: u64,
}

/// What the client itself feeds into the events (mouse over a plot, keys, the window).
#[derive(Debug, Clone)]
pub enum ClientEvent {
    /// The mouse is over a plot (grid coordinates): `SerialEventMouseOverHex(hexX, hexY)`.
    MouseOverHex { x: i32, y: i32 },
    /// The mouse entered or left the world view: `WorldMouseOver`.
    WorldMouseOver(bool),
    /// A plot was clicked or deselected: `SerialEventHexSelected` / `SerialEventHexDeSelected`.
    HexSelected { x: i32, y: i32, selected: bool },
    /// The window changed: `SystemUpdateUI(type)`.
    SystemUpdateUi(i32),
    /// Key up (`KeyUpEvent(wParam)`).
    KeyUp(u32),
    /// The state of Ctrl, Shift, Alt for `UI.CtrlKeyDown` and friends.
    Modifiers { ctrl: bool, shift: bool, alt: bool },
    /// The camera moved: `CameraViewChanged`.
    CameraViewChanged,
    /// Any `Events.<name>(args)`.
    Fire { name: String, args: Vec<Arg> },
}

/// Counters for the debug panel (`exe.*`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExeStats {
    /// Events whose listeners were run.
    pub events: u64,
    /// Time of the last `dispatch`, microseconds.
    pub dispatch_us: u64,
    /// Calls of functions that are SIMP stubs (see [`simp::SIMP_ROWS`]).
    pub simp_calls: u64,
    /// The same, by name (`UI.ProposedDealExists`, `engine.UnitCombat`, `Events.<unknown>`).
    pub simp_by_name: BTreeMap<String, u64>,
    pub frames: u64,
    /// Engine callbacks with no event in K1-K3.
    pub unmapped: u64,
    pub game_events_in: u64,
    pub attach_errors: u64,
    /// Events waiting in the FIFO.
    pub queued: usize,
}

/// The state the installed Lua functions share.
pub(crate) struct Inner {
    pub cfg: ExeConfig,
    pub host: Rc<dyn ExeHost>,
    pub registry: RefCell<Registry>,
    pub simp_calls: Cell<u64>,
    pub options: options::OptionsState,
    pub userdata: modding::UserDataStore,
    pub leader_keys: RefCell<std::collections::HashMap<i32, String>>,
}

/// The sweep of [`sweep_dead_instances`]: every cached `Unit` / `City` instance whose object no player has any more
/// loses its pointer (and leaves the cache); returns how many.
const SWEEP_DEAD_LUA: &str = r#"
local live = {}
local n = (GameDefines and GameDefines.MAX_PLAYERS) or 64
for i = 0, n - 1 do
  local p = Players[i]
  if p ~= nil then
    for u in p:Units() do live[u] = true end
    for c in p:Cities() do live[c] = true end
  end
end
local dropped = 0
for _, T in ipairs({ Unit, City }) do
  local cache = type(T) == "table" and rawget(T, "__instances")
  if type(cache) == "table" then
    for k, t in pairs(cache) do
      if not live[t] then
        if type(t) == "table" then rawset(t, "__instance", nil) end
        cache[k] = nil
        dropped = dropped + 1
      end
    end
  end
end
return dropped
"#;

/// Clears the Lua instances of units and cities that are gone (live report 2026-10-31: "memory access out of
/// bounds" in `CvUnit::IsGarrisoned` from `CvLuaUnit::lIsGarrisoned`, turn 106; a Great Musician's Create Great Work
/// froze the game).
///
/// SIMPLIFIED(SIMP-exe-692): a Lua instance of the DLL (`Unit`, `City`) is a table with the object's raw pointer
/// (`__instance`, CvLuaScopedInstance), cached by pointer in `<Type>.__instances`. The replica frees a unit or city
/// while it replays a journal batch (killed, upgraded, consumed by a mission, razed), and a script that kept the table
/// called a method on freed memory: the wasm module trapped. After each frame's new batches, before any Lua runs, the
/// instances no player lists (`Players[i]:Units()`, `:Cities()`) lose their pointer, so such a call is the Lua error
/// "Not a valid instance" (as after a load, `gc_lua_invalidate_ui`). The exe's own process kept freed objects of its
/// pools mapped, so there a stale pointer read garbage instead.
fn sweep_dead_instances(lua: &lua_sys::Lua) {
    match lua
        .load(SWEEP_DEAD_LUA)
        .set_name("=exe_sweep_dead")
        .eval::<i32>()
    {
        Ok(0) => {}
        Ok(n) => tracing::debug!(target: "exe.bind", "{n} dead unit/city instances cleared"),
        Err(e) => tracing::warn!(target: "exe.bind", "the sweep of dead instances failed: {e}"),
    }
}

pub struct ExeApi {
    inner: Rc<Inner>,
    core: Rc<Core>,
}

/// SIMP-exe-66: the DLL's Lua functions the shipped UI (EUI, Lekmod) calls to change saved game state directly, with no
/// network message, made no-ops in the replica's UI state ([`ExeApi::bind_game`]). The rules' own calls (the core's Lua,
/// another state) are untouched. The writes:
/// * `Game.SetEverRightClickMoved` (WorldView.lua, every right-click move): `CvGame::m_bEverRightClickMoved`;
/// * `Game.SetAdvisorMessageHasBeenSeen` (the advisor popups): `CvGame::m_AdvisorMessagesViewed`;
/// * `Game.SetStaticTutorialActive`: `CvGame::m_bStaticTutorialActive`;
/// * `Game.SetPausePlayer` (the loading screen of a single-player game, the hot-seat bumper): `CvGame::m_ePausePlayer`;
/// * `Player:AddNotification` (EUI's CityView when the last queued item is removed, NotificationPanel on a culture
///   tile): the player's `CvNotifications`, which also hold the end-turn blocking.
const LOCAL_ONLY_MUTATORS: &str = r#"
local function none() end
local function local_only(t, name)
  if type(t) == "table" and t[name] ~= nil then t[name] = none end
end
local_only(Game, "SetEverRightClickMoved")
local_only(Game, "SetAdvisorMessageHasBeenSeen")
local_only(Game, "SetStaticTutorialActive")
local_only(Game, "SetPausePlayer")
-- The method table of the players (`Player`, made by the DLL when it pushed the first player); the metatable of a player
-- object leads to it as well.
local P = rawget(_G, "Player")
if type(P) ~= "table" and type(Players) == "table" then
  for _, p in pairs(Players) do
    local mt = getmetatable(p)
    if mt and type(mt.__index) == "table" then P = mt.__index break end
  end
end
local_only(P, "AddNotification")
-- SIMP-exe-820: an AI civilization the server's core has active in the AI phase (`__exe_ai_turn[id]`, set when the server's
-- turn queue event is handled, gone with the next load) reads as turn-active; the replica does not play the AI phase.
if type(P) == "table" and type(P.IsTurnActive) == "function" then
  local active = P.IsTurnActive
  P.IsTurnActive = function(p, ...)
    local o = rawget(_G, "__exe_ai_turn")
    if o ~= nil and p ~= nil then
      local v = o[p:GetID()]
      if v ~= nil then return v end
    end
    return active(p, ...)
  end
end
-- SIMP-exe-742: a human whose END_TURN the journal applied is done before the core's next pump marks it (Core::track_turn_complete).
if type(P) == "table" and type(P.HasReceivedNetTurnComplete) == "function" then
  local received = P.HasReceivedNetTurnComplete
  P.HasReceivedNetTurnComplete = function(p, ...)
    if received(p, ...) then return true end
    local ended = rawget(_G, "__exe_turn_ended")
    return ended ~= nil and p ~= nil and ended[p:GetID()] == true
  end
end
"#;

impl ExeApi {
    pub fn new(cfg: ExeConfig, host: Rc<dyn ExeHost>) -> ExeApi {
        let inner = Rc::new(Inner {
            userdata: modding::UserDataStore::new(cfg.userdata_flush_secs),
            cfg: cfg.clone(),
            host: host.clone(),
            registry: RefCell::new(Registry::default()),
            simp_calls: Cell::new(0),
            options: options::OptionsState::default(),
            leader_keys: RefCell::default(),
        });
        let core = Core::new(cfg, host, Rc::clone(&inner));
        ExeApi { inner, core }
    }

    /// Observes the `Events.<name>` families of `names` (exact names; empty = none): `tap(name, args)` runs for each
    /// such event before the Lua listeners, however it was fired (by Rust, or by Lua with its values converted to
    /// `Arg`). A later call replaces the tap.
    pub fn set_event_tap(&self, names: &[&str], tap: TapFn) {
        *self.core.tap.borrow_mut() = if names.is_empty() {
            None
        } else {
            Some(crate::core::EventTap {
                names: names.iter().map(|n| (*n).to_owned()).collect(),
                f: tap,
            })
        };
    }

    /// The clock for `exe.dispatch_us`.
    pub fn set_clock(&self, clock: Rc<dyn platform::Clock>) {
        *self.core.clock.borrow_mut() = Some(clock);
    }

    /// For `UiCore::new`: puts the globals of this crate into the base globals of the Lua state.
    pub fn base_setup(&self) -> impl Fn(&Lua, &Table<'_>) -> lua_sys::Result<()> + 'static {
        let inner = Rc::clone(&self.inner);
        let core = Rc::clone(&self.core);
        move |lua, g| {
            install(&inner, lua, g)?;
            events::install(&core, lua, g)?;
            ui_api::install(&core, lua, g)?;
            network::install(&core, lua, g)?;
            corners::install(&core, &inner, lua, g)?;
            m4_wog::install(&core, &inner, lua, g)?;
            Ok(())
        }
    }

    /// Lets the exe API run listeners the moment an event is fired (before the first `dispatch`). `dispatch`
    /// does this by itself; an app that fires events earlier (`post`) calls it once after `UiCore::new`.
    /// The Lua `print` of the contexts (kept in the runtime's log ring) also goes to the browser console.
    pub fn attach_runtime(&self, rt: &Runtime) {
        rt.set_log_sink(|_, line| tracing::info!(target: "m2.print", "{line}"));
        *self.core.rt.borrow_mut() = Some(rt.downgrade());
    }

    /// Puts the client functions of `PreGame` into the `PreGame` table that exists now. The game
    /// replica registers its own `PreGame` (reads of `CvPreGame`); call this after it did, so the client
    /// part (`GetNickName`, `IsMultiplayerGame`, ...) is on top of it.
    pub fn reinstall_pregame(&self, lua: &Lua) -> lua_sys::Result<()> {
        pregame::install(&self.inner, lua, &lua.globals())
    }

    /// The game is let go (the layer of the game is being dropped): the replica no longer lives through this API.
    pub fn unbind_game(&mut self) {
        self.core.game.borrow_mut().take();
    }

    /// The replica is the game: the DLL's Lua API is registered in `lua` and the game events the Lua side listens
    /// to are subscribed. The first `dispatch` announces the whole game (`setup_graphical`).
    pub fn bind_game(&mut self, game: Rc<RefCell<dyn GameSide>>, lua: &Lua) {
        let c = &self.core;
        *c.game.borrow_mut() = Some(game.clone());
        c.generation.set(None);
        match game.try_borrow_mut() {
            Ok(mut g) => {
                if let Err(e) = g.attach_lua(lua) {
                    tracing::error!(target: "exe.bind", "attach_lua: {e}");
                    c.counters
                        .attach_errors
                        .set(c.counters.attach_errors.get() + 1);
                }
                for name in c.game_subscribed.borrow().iter() {
                    g.subscribe_game_event(name);
                }
            }
            Err(_) => tracing::error!(target: "exe.bind", "the game is busy"),
        }
        // The client part of PreGame (nick, multiplayer flags) goes on top of the replica's PreGame.
        if let Err(e) = self.reinstall_pregame(lua) {
            tracing::error!(target: "exe.bind", "reinstall_pregame: {e}");
        }
        if let Err(e) = corners::install_pregame_names(&self.inner, lua) {
            tracing::error!(target: "exe.bind", "pregame names: {e}");
        }
        // SIMPLIFIED(SIMP-exe-65): the end of turn of the UI (`Game.DoControl(CONTROL_ENDTURN)`) goes out as the
        // turn handshake only; the DLL also ends the turn of the seat in the local game, and the server's journal
        // then meets a game that ended it already (the replica diverges and resyncs every turn).
        if let Err(e) = lua
            .load(
                "local g = Game
                 local f = g and g.DoControl
                 if f then
                   g.DoControl = function(c, ...)
                     if c == GameInfoTypes.CONTROL_ENDTURN then return Network.SendTurnComplete() end
                     return f(c, ...)
                   end
                 end",
            )
            .set_name("=exe_end_turn")
            .exec()
        {
            tracing::error!(target: "exe.bind", "end turn: {e}");
        }
        // The plot of the caravan's target choice (`Game.SelectedUnit_SpeculativePopupTradeRoute_Display(x, y, type,
        // domain)`): the core's event for the temporary route carries 9 plots of the path at most (SIMP-exe-921).
        if let Err(e) = lua
            .load(
                "local g = Game
                 local f = g and g.SelectedUnit_SpeculativePopupTradeRoute_Display
                 if f then
                   g.SelectedUnit_SpeculativePopupTradeRoute_Display = function(x, y, ...)
                     __exe_popup_dest(x, y)
                     return f(x, y, ...)
                   end
                 end",
            )
            .set_name("=exe_popup_dest")
            .exec()
        {
            tracing::error!(target: "exe.bind", "popup dest: {e}");
        }
        // SIMPLIFIED(SIMP-exe-66): the DLL functions the UI calls to change the saved game directly, with no network
        // message (the exe ran them on its own machine only), change nothing in the replica: the server never hears
        // of them, so they made the replica's game differ from the server's and the next checked digest forced a
        // resync (live 2026-10-30: WorldView.lua's right-click move calls Game.SetEverRightClickMoved, a saved field
        // of CvGame; m5-map-sync `record 2: digest ...`).
        if let Err(e) = lua
            .load(LOCAL_ONLY_MUTATORS)
            .set_name("=exe_local_only")
            .exec()
        {
            tracing::error!(target: "exe.bind", "local-only mutators: {e}");
        }
        // SIMPLIFIED(SIMP-exe-420): in the exe the UI contexts load before the game graphics are set up, and the contexts that fill
        // their lists at load (EUI's unit and city ribbons: `g_cities.Initialize`) meet an empty game and are told every unit and city
        // as created. Here the replica holds the game already when the contexts load, so the ribbons held every city twice (the
        // load, then the announcement). Until the announcement is done `Player:Cities()` and `:Units()` list nothing; that holds for
        // every load (a turn's snapshot is a load, SIMP-exe-64), the ribbons are filled again when `GameplaySetActivePlayer` comes.
        c.setup_done.set(false);
        if let Err(e) = lua
            .load(
                "local P
                 for i = 0, 63 do P = Players and Players[i] if P then break end end
                 local mt = P and getmetatable(P)
                 local idx = mt and mt.__index
                 if type(idx) == 'table' and rawget(idx, '__exe_gated') == nil then
                   rawset(idx, '__exe_gated', true)
                   for _, name in ipairs({'Cities', 'Units'}) do
                     local f = idx[name]
                     if f then
                       rawset(idx, name, function(self, ...)
                         if not __exe_setup_done() then return function() return nil end end
                         return f(self, ...)
                       end)
                     end
                   end
                 end",
            )
            .set_name("=exe_gate_lists")
            .exec()
        {
            tracing::error!(target: "exe.bind", "gate lists: {e}");
        }
        self.core.flush_net();
    }

    /// An event of the client (mouse over a plot, camera, keys).
    pub fn post(&mut self, ev: ClientEvent) {
        let c = &self.core;
        let hexv = |x: i32, y: i32| {
            let (hx, hy) = hex::grid_to_hex(x, y);
            (Arg::from(hx), Arg::from(hy))
        };
        match ev {
            ClientEvent::MouseOverHex { x, y } => {
                // The exe tells the scripts when the mouse comes onto another plot, not on every move (WorldView.lua
                // asks the path finder on each).
                let fire = {
                    let mut ui = c.ui.borrow_mut();
                    ui.mouse_hex = (x, y);
                    ui.mouse_hex_fired.replace((x, y)) != Some((x, y))
                };
                if fire {
                    // The scripts hand the two numbers to `Map.GetPlot` as they are (EnemyUnitPanel, PlotHelpManager,
                    // Bombardment): the exe's "hex" of this event is the plot's grid position, as `UI.GetMouseOverHex`
                    // gives. Cube-to-offset shifted them by y/2: the combat preview showed the plot of another row.
                    c.fire_events("SerialEventMouseOverHex", vec![Arg::from(x), Arg::from(y)]);
                }
            }
            ClientEvent::WorldMouseOver(over) => {
                c.fire_events("WorldMouseOver", vec![Arg::Bool(over)]);
            }
            ClientEvent::HexSelected { x, y, selected } => {
                let (hx, hy) = hexv(x, y);
                let name = if selected {
                    "SerialEventHexSelected"
                } else {
                    "SerialEventHexDeSelected"
                };
                c.fire_events(name, vec![hx, hy]);
            }
            ClientEvent::SystemUpdateUi(t) => c.fire_events("SystemUpdateUI", vec![Arg::from(t)]),
            ClientEvent::KeyUp(k) => c.fire_events("KeyUpEvent", vec![Arg::Num(f64::from(k))]),
            ClientEvent::Modifiers { ctrl, shift, alt } => {
                let mut ui = c.ui.borrow_mut();
                ui.ctrl = ctrl;
                ui.shift = shift;
                ui.alt = alt;
            }
            ClientEvent::CameraViewChanged => c.fire_events("CameraViewChanged", vec![]),
            ClientEvent::Fire { name, args } => c.fire_events(&name, args),
        }
    }

    /// The movement path the exe draws now (`Events.DisplayMovementIndicator(true)` and the last
    /// `UI.SendPathfinderUpdate`), the unit's plot first; empty when none is shown.
    pub fn movement_path(&self) -> Vec<PathNode> {
        let ui = self.core.ui.borrow();
        if ui.path_shown {
            ui.path.clone()
        } else {
            Vec::new()
        }
    }

    /// The paths of the trade routes the team `viewer` sees, from the replica core (`Q_TRADE_PATHS`): the plots the
    /// DLL's trade route path finder laid, which the caravan walks and the exe's `TradeVisSystem` draws. Empty
    /// without a game, or when the game is busy.
    pub fn trade_paths(&self, viewer: i32) -> Vec<TradePath> {
        let Some(game) = self.core.game() else {
            return Vec::new();
        };
        let Ok(mut g) = game.try_borrow_mut() else {
            return Vec::new();
        };
        g.trade_paths(viewer).unwrap_or_else(|e| {
            tracing::debug!(target: "exe.ui", "trade_paths: {e}");
            Vec::new()
        })
    }

    /// The trade route popup now shown (`TradeVisuals_ActivatePopupRoute`): what to draw as the popup route, if any.
    pub fn popup_route(&self) -> Option<PopupRoute> {
        let ui = self.core.ui.borrow();
        match ui.popup_route? {
            -1 => ui.temp_route.clone().map(|t| PopupRoute::Temp {
                dest: ui.popup_dest,
                route: t,
            }),
            id => Some(PopupRoute::Existing(id)),
        }
    }

    /// `SequenceGameInitComplete` by hand (when `ExeConfig::fire_init_complete` is off, or after the contexts
    /// of a late-loaded root exist).
    pub fn fire_init_complete(&self) {
        self.core.init_fired.set(true);
        self.core.fire_events("SequenceGameInitComplete", vec![]);
    }

    /// `RegisterPopupType(type, stack)`.
    pub fn register_popup_type(&self, ty: i32, stack: bool) {
        self.core.buddy.borrow_mut().register_type(ty, stack);
    }

    /// The "do not show popups" mode; turning it off shows the delayed popups.
    pub fn set_dont_show_popups(&self, on: bool) {
        self.core.buddy.borrow_mut().set_dont_show(on);
        if !on {
            self.process_delayed_popups();
        }
    }

    /// `ProcessDelayedPopups`.
    pub fn process_delayed_popups(&self) {
        let fires = self.core.buddy.borrow_mut().process_delayed();
        for f in fires {
            self.core.enqueue(f);
        }
    }

    /// `InterfaceBuddy::AddPopup` from the client side (not from the core).
    pub fn add_popup(&self, info: PopupInfo) {
        self.core.add_popup(info);
    }

    /// The UI slept while the game went on (another game UI had the screen and the core's events were dropped): the
    /// next [`dispatch`](Self::dispatch) takes the replica's game as a new load, the way a new snapshot is taken
    /// (`invalidate_lua`, `setup_graphical`, the units and cities announced again, the turn events of a later turn).
    /// Nothing happens before the first dispatch announced the game.
    // SIMPLIFIED(SIMP-267): the classic UI that wakes up announces every unit and city again, as after a snapshot
    // load, instead of getting the core's events while it sleeps.
    pub fn force_resync(&self) {
        let c = &self.core;
        if c.generation.get().is_some() {
            // No replica's generation is ever this one: the next dispatch sees a change.
            c.generation.set(Some(u64::MAX));
        }
    }

    /// Once per frame: the replica's changes of generation, the engine events translated into `Events.*`, the FIFO
    /// handled, the dirty bits raised.
    pub fn dispatch(&mut self, rt: &Runtime) {
        let c = self.core.clone();
        let t0 = c.now_us();
        let events_before = c.counters.events.get();
        let errors_before = rt.error_count();
        let mut turn_events_from: Option<f64> = None;
        let mut turn_started_now = false;
        self.attach_runtime(rt);
        c.counters.frames.set(c.counters.frames.get() + 1);
        if !c.error_filter_set.replace(true) {
            // SIMPLIFIED(SIMP-exe-64): a handler that touches an object of the previous load right after a load
            // raises "Not a valid instance"; those errors are logged, not counted.
            let core = Rc::clone(&c);
            rt.set_error_filter(Some(Rc::new(move |e: &lua_sys::LuaError| {
                core.counters.frames.get() <= core.stale_until.get()
                    && e.full_text().contains("Not a valid instance")
            })));
        }

        if let (Some(game), Some(ctx)) = (c.game(), c.lua_ctx()) {
            let mut rebuilt = false;
            let mut loaded = false;
            let gen_now = game.try_borrow().map(|g| g.generation()).ok();
            if let Some(now) = gen_now
                && c.generation.get() != Some(now)
                && let Ok(mut g) = game.try_borrow_mut()
            {
                let reset_at = c.now_ms_f64();
                if c.generation.get().is_some() {
                    g.invalidate_lua(ctx.lua());
                }
                // SIMPLIFIED(SIMP-exe-420): the lists of the new load are announced by the rebuild events of this batch.
                c.setup_done.set(false);
                match g.setup_graphical() {
                    Ok(()) => rebuilt = true,
                    Err(e) => tracing::error!(target: "exe.bind", "setup_graphical: {e}"),
                }
                c.generation.set(Some(now));
                c.stale_until.set(c.counters.frames.get() + 30);
                // A turn's snapshot is a new turn: the end of turn is not sent yet (the core fires no
                // `ActivePlayerTurnStart` for a loaded game).
                c.ui.borrow_mut().sent_turn_complete = false;
                // SIMPLIFIED(SIMP-exe-630): the DLL's turn sets the unit-info dirty bits (CvPlayer::doTurnUnits,
                // CvPlayer.cpp:5659-5665: UnitInfo, and SelectionButtons for the active player) and the units' new moves
                // set them for a selected unit; the replica loads the turn's snapshot instead, so every load raises them
                // once. A unit kept selected across the end of turn showed the old turn in EUI's unit panel (no moves, no
                // actions) until it was selected again (live report 2026-10-31).
                c.dirty.borrow_mut().extend([6, 19]);
                loaded = true;
                let reset_end = c.now_ms_f64();
                c.span(
                    "c.lua_reset",
                    reset_at,
                    reset_end - reset_at,
                    vec![("rebuilt", f64::from(u8::from(rebuilt)))],
                );
                turn_events_from = Some(reset_end);
            }
            // Before any Lua of this frame runs: the batches applied since the last frame may have freed units and
            // cities (a unit killed, upgraded or consumed, as a great person's Create Great Work; a city razed).
            if let Ok(g) = game.try_borrow() {
                let applied = g.applied();
                drop(g);
                if c.applied_seen.replace(applied) != applied && !loaded {
                    sweep_dead_instances(ctx.lua());
                }
            }
            // SIMPLIFIED(SIMP-exe-136): the replica loads each turn's snapshot instead of running the turn, so no
            // turn event comes from the core; a load at a later turn raises `NewGameTurn` and `ActivePlayerTurnStart`
            // here ("Your turn", the turn queue, the MP list).
            let mut turn_started = None;
            if loaded
                && let Ok(t) = ctx
                    .lua()
                    .load("return Game.GetGameTurn()")
                    .set_name("=exe_turn")
                    .eval::<i32>()
            {
                let prev = c.turn_seen.replace(Some(t));
                if prev.is_some_and(|p| t > p) {
                    turn_started = Some(t);
                }
            }
            // SIMPLIFIED(SIMP-exe-702): what the core announces while the replica joins (a reload of the page replays the
            // notifications and popups of the turn) or loads is the game's past: it makes no sound.
            let catching = game.try_borrow().map(|g| g.catching_up()).unwrap_or(false);
            let frame = c.counters.frames.get();
            // (a pump that drains the queue in the frame still announced the replayed batches: one frame of tail)
            if catching {
                let since = c.sound_quiet_since.get();
                if since == 0 {
                    c.sound_quiet_since.set(frame.max(1));
                }
                // SIMPLIFIED(SIMP-exe-973): a catch-up that lasts longer than a join does (a replica stuck out of the
                // live state, or with batches waiting for a missing one) no longer holds the sounds: the notifications
                // of the rest of the turn sounded nowhere (reload of the page, live report 2026-11-01).
                if frame.saturating_sub(since.max(1)) < crate::core::SOUND_QUIET_MAX_FRAMES {
                    c.sound_quiet_until.set(frame + 1);
                } else if frame.saturating_sub(since.max(1)) == crate::core::SOUND_QUIET_MAX_FRAMES
                {
                    tracing::warn!(
                        frames = crate::core::SOUND_QUIET_MAX_FRAMES,
                        "sounds are no longer held: the replica is still catching up"
                    );
                }
            } else {
                c.sound_quiet_since.set(0);
                if loaded {
                    c.sound_quiet_until.set(frame);
                }
            }
            let mut events = Vec::new();
            if let Ok(mut g) = game.try_borrow_mut() {
                g.take_events(&mut events);
            }
            c.translate_batch(events, rebuilt);
            turn_started_now = turn_started.is_some();
            if let Some(t) = turn_started {
                c.fire_events("NewGameTurn", vec![Arg::from(t)]);
                c.fire_events("ActivePlayerTurnStart", vec![]);
            }
            c.track_turn_complete(ctx.lua(), loaded);
            c.track_ai_turn(ctx.lua(), loaded);
            c.track_auto_cycle(ctx.lua(), loaded);
            c.track_auto_end_turn(ctx.lua(), loaded);
            // WO-G (K19): a state that is over shows the end-of-game window.
            // (also every 30th frame: the state of a snapshot can come a frame after the load)
            let tick = c.over_tick.get().wrapping_add(1);
            c.over_tick.set(tick);
            if loaded || tick.is_multiple_of(30) {
                c.check_game_over(ctx.lua());
            }
            c.sync_bridge(ctx.lua(), loaded);
        }
        c.drain();
        // After every Lua of the frame: the AI progress is the end-turn button's text while the turn is processed.
        if let Some(ctx) = c.lua_ctx() {
            c.track_ai_progress(ctx.lua());
        }
        // The resource icons of the map, once after a change of what the active team sees (yields.rs).
        c.sync_resources();
        c.sync_camera_rect();
        c.sync_yield_changes();
        // The dirty bits are raised once per frame, after everything the frame's events did.
        c.flush_dirty();
        c.drain();
        // What the listeners asked of the game in the meantime goes out in order.
        c.flush_net();
        let dt = c.now_us().saturating_sub(t0);
        c.counters.dispatch_us.set(dt);
        // The turn's events and the Lua handlers they ran, from the end of the reset to here (`c.turn_events`).
        if let Some(from) = turn_events_from {
            let now = c.now_ms_f64();
            c.span(
                "c.turn_events",
                from,
                now - from,
                vec![
                    ("events", (c.counters.events.get() - events_before) as f64),
                    (
                        "lua_errors",
                        rt.error_count().saturating_sub(errors_before) as f64,
                    ),
                    ("turn_started", f64::from(u8::from(turn_started_now))),
                ],
            );
        }
    }

    /// The timed pieces of the UI side since the last call (`c.click`, `c.lua_reset`, `c.turn_events`).
    pub fn take_spans(&self) -> Vec<ExeSpan> {
        std::mem::take(&mut *self.core.spans.borrow_mut())
    }

    /// `Network.HasSentNetTurnAllComplete` as the timeline sees it: every human has ended the turn.
    pub fn turn_all_complete(&self) -> bool {
        self.core.turn_all_complete()
    }

    /// The game as the music reads it once a second (re-b15 ITEM 3); `None` before a game is bound.
    pub fn music_state(&self) -> Option<music::MusicState> {
        self.core.music_state()
    }

    pub fn stats(&self) -> ExeStats {
        let c = &self.core;
        ExeStats {
            events: c.counters.events.get(),
            dispatch_us: c.counters.dispatch_us.get(),
            simp_calls: self.inner.simp_calls.get(),
            simp_by_name: c.simp.borrow().clone(),
            frames: c.counters.frames.get(),
            unmapped: c.counters.unmapped.get(),
            game_events_in: c.counters.game_events_in.get(),
            attach_errors: c.counters.attach_errors.get(),
            queued: c.queue_len(),
        }
    }

    /// Number of listeners of `Events.<name>` (tests, the report).
    pub fn listener_count(&self, family: Family, name: &str) -> usize {
        self.core.handler_count(family, name)
    }

    /// Units and cities the UI has been told about (tests, the report).
    pub fn entity_counts(&self) -> (usize, usize) {
        let t = self.core.entities.borrow();
        (t.units.len(), t.cities.len())
    }

    /// The plot (grid) of the head selected unit the UI was last told of (`UnitSelectionChanged`): where the
    /// engine draws the selection ring of the unit.
    pub fn selected_unit_plot(&self) -> Option<(i32, i32)> {
        let t = self.core.entities.borrow();
        let key = t.selected?;
        t.units.get(&key).map(|u| (u.x, u.y))
    }

    /// The camera height of `CAMERALOOKAT_CITY_ZOOM_IN` (exe `lookAt`, night answers 1b P4): 500 / 700 / 800 for the
    /// seat's city-distance high-water mark below 2 / equal to 2 / 3 and more (`CITY_ZOOM_LEVEL_1/2/3`); 500 without a
    /// seat. The mark (`CvPlayer::GetCityDistanceHighwaterMark`, 1 at the start, never falls) is the farthest an owned
    /// plot lies from the nearest own city.
    ///
    /// SIMPLIFIED(SIMP-exe-930): the Lua API has no `GetCityDistanceHighwaterMark`, so it is read off the borders now
    /// (the farthest owned plot within 5 hexes of an own city, from the nearest own city) and kept as a high-water mark
    /// per `ExeApi`. The DLL sets its field when a plot changes owner; a border that shrank again keeps the mark here
    /// as there.
    pub fn city_zoom_height(&self) -> f32 {
        const LUA: &str = "local me = Game and Game.GetActivePlayer and Game.GetActivePlayer()
            local p = me and me >= 0 and Players and Players[me]
            if not p then return 1 end
            local cities = {}
            for c in p:Cities() do cities[#cities + 1] = c end
            local best, seen = 1, {}
            for _, c in ipairs(cities) do
                local cx, cy = c:GetX(), c:GetY()
                for dx = -5, 5 do
                    for dy = -5, 5 do
                        local pl = Map.GetPlot(cx + dx, cy + dy)
                        if pl and pl:GetOwner() == me and not seen[pl:GetPlotIndex()] then
                            seen[pl:GetPlotIndex()] = true
                            local nearest = 99
                            for _, o in ipairs(cities) do
                                local d = Map.PlotDistance(o:GetX(), o:GetY(), pl:GetX(), pl:GetY())
                                if d < nearest then nearest = d end
                            end
                            if nearest <= 5 and nearest > best then best = nearest end
                        end
                    end
                end
            end
            return best";
        let Some(ctx) = self.core.lua_ctx() else {
            return CITY_ZOOM_HEIGHTS[0];
        };
        let now = ctx
            .lua()
            .load(LUA)
            .set_name("=exe_city_zoom")
            .eval::<i32>()
            .unwrap_or_else(|e| {
                tracing::debug!(target: "exe.camera", "the city zoom: {e}");
                1
            });
        let mark = self.core.city_highwater.get().max(now);
        self.core.city_highwater.set(mark);
        CITY_ZOOM_HEIGHTS[match mark {
            ..=1 => 0,
            2 => 1,
            _ => 2,
        }]
    }

    /// The camera is moving (the client says so each frame): `UI.IsCameraMoving`.
    pub fn set_camera_moving(&self, moving: bool) {
        self.core.camera_moving.set(moving);
    }

    /// The exe's hot keys ([`hotkeys`]): a key press (Windows virtual key and modifiers) that no Lua input handler
    /// took. Carries out the action of the key when there is one the game allows; answers its `Type`.
    pub fn hotkey(&self, vk: u32, ctrl: bool, alt: bool, shift: bool) -> Option<String> {
        let kb = hotkeys::kb_of_vk(vk)?;
        let ctx = self.core.lua_ctx()?;
        let r = ctx
            .lua()
            .load(&hotkeys::lua(
                kb,
                ctrl,
                alt,
                shift,
                self.core.turn_all_complete(),
            ))
            .set_name("=exe_hotkey")
            .eval::<Option<String>>();
        // What the action asked of the game goes out with the next dispatch, like a button click's.
        match r {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(target: "exe.hotkey", "key {vk:#x}: {e}");
                None
            }
        }
    }

    /// Writes the user data that changed to the store when `userdata_flush_secs` passed since the last
    /// write. Call once per frame with the time of the app.
    pub fn tick(&self, now_secs: f64) {
        self.inner.userdata.tick(&*self.inner.host, now_secs);
    }

    /// Writes the user data now (the page is being hidden or closed).
    pub fn flush(&self) {
        self.inner.userdata.flush(&*self.inner.host);
    }

    /// What `base_setup` registered: every name with the status of its implementation. Empty until the
    /// setup ran.
    pub fn registered(&self) -> Registry {
        self.inner.registry.borrow().clone()
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.userdata.flush(&*self.host);
    }
}

fn install(inner: &Rc<Inner>, lua: &Lua, g: &Table<'_>) -> lua_sys::Result<()> {
    enums::install(inner, lua, g)?;
    hex::install(inner, lua, g)?;
    options::install(inner, lua, g)?;
    modding::install(inner, lua, g)?;
    content::install(inner, lua, g)?;
    pregame::install(inner, lua, g)?;
    path::install(inner, lua, g)?;
    stubs::install(inner, lua, g)?;
    locale::install(inner, lua, g)?;
    Ok(())
}

#[cfg(test)]
mod sweep_tests {
    /// A fake of the DLL's cache: `Unit.__instances` keyed by "pointer", the players' unit lists.
    #[test]
    fn dead_unit_and_city_instances_lose_their_pointer() {
        let lua = lua_sys::Lua::new().unwrap();
        lua.exec(
            r#"
            local function inst(t, ptr) local o = { __instance = ptr }; t.__instances[ptr] = o; return o end
            Unit = { __instances = {} }; City = { __instances = {} }
            alive_u = inst(Unit, "u1"); dead_u = inst(Unit, "u2")
            alive_c = inst(City, "c1"); dead_c = inst(City, "c2")
            local function iter(list) local i = 0; return function() i = i + 1; return list[i] end end
            Players = { [0] = { Units = function() return iter({ alive_u }) end, Cities = function() return iter({ alive_c }) end } }
            GameDefines = { MAX_PLAYERS = 4 }
            "#,
            "=setup",
        )
        .unwrap();
        super::sweep_dead_instances(&lua);
        let check: bool = lua
            .load(
                "return alive_u.__instance == 'u1' and dead_u.__instance == nil and Unit.__instances.u2 == nil
                   and alive_c.__instance == 'c1' and dead_c.__instance == nil and City.__instances.c1 == alive_c",
            )
            .eval()
            .unwrap();
        assert!(check);
    }
}
