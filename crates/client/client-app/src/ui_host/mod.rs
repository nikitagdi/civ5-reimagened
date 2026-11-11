//! The host of the game's interfaces: which game UI is on the screen.
//!
//! A game UI is a [`Frontend`]: the classic original UI ([`classic::ClassicFrontend`]) or UI v2. Each is one
//! registration in the app; the host forwards the app's frame, input and render hooks to the active one only. A
//! frontend that is not active sleeps: it gets no frame and no input. The game session (`crate::session`) runs
//! before the frontends and does not depend on which one is active.
//!
//! Without a registered frontend (`?ui=interim`) the host is empty and the app's interim panels are the game screen.

// TEMP(T53): the switch between frontends is the next step; until then parts of the interface are unused.
#![allow(dead_code)]

pub mod classic;
pub mod v2;

use std::rc::Rc;

use client_core::ClientEvent;
use draw_list::UiDrawList;
use platform::{Clock, InputEvent};
use render::{GameCamera, Renderer};

use crate::bridge::WorldBridge;
use crate::map_camera::MapCameraController;
use crate::mod_hooks::ModHooks;
use crate::net::NetHandle;
use crate::picking::Picker;
use crate::session::{GameSession, SessionStep};

pub use ui2_host::ScreenId;

/// Which game UI a frontend is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiKind {
    /// The original Lua/XML UI over the exe API.
    Classic,
    /// UI v2: a skin package.
    V2,
}

/// Where a frontend's preparation is.
#[derive(Debug, Clone, PartialEq)]
pub enum Readiness {
    /// Still loading; `progress` from 0 to 1 when it is known.
    Loading { progress: Option<f32> },
    /// It can take the screen.
    Ready,
    /// It cannot run in this session.
    Failed(String),
}

/// What a frontend may look at while it prepares, wakes up or goes to sleep.
pub struct PrepareCx<'a> {
    /// The app's clock, ms.
    pub now_ms: f64,
    /// Physical size of the window and its pixel density.
    pub size: (u32, u32),
    pub dpr: f32,
    /// The game session (`None` before it exists).
    pub session: Option<&'a GameSession>,
}

/// What a frontend's frame touches. The world requests of the UI (camera, highlights, the minimap, sounds of the
/// world) are carried out by the frontend itself through these.
pub struct FrameCx<'a> {
    /// The app's clock: ms, and the same as a float.
    pub now: u64,
    pub now_ms: f64,
    /// Physical size of the window and its pixel density.
    pub size: (u32, u32),
    pub dpr: f32,
    pub net: Option<&'a NetHandle>,
    /// This client's seat in the game, once the attach named it.
    pub seat: Option<exe_api::SeatInfo>,
    pub session: &'a GameSession,
    /// What the session did this frame.
    pub step: &'a SessionStep,
    pub camera: &'a mut GameCamera,
    pub map_controller: &'a mut MapCameraController,
    pub loader: Option<&'a mut assets::AssetLoader>,
    pub clock: &'a Rc<dyn Clock>,
    pub renderer: &'a mut Renderer,
    /// `__civ5stats.<group>` (the M2 counter groups).
    pub stats: Option<&'a crate::GroupStatsSink>,
}

/// What a frontend's input touches. The events come after the mods' claims, F3 and egui (the app's chain).
pub struct InputCx<'a> {
    /// The app's clock, ms.
    pub now_ms: f64,
    pub camera: &'a mut GameCamera,
    pub map_controller: &'a mut MapCameraController,
    pub picker: &'a mut Picker,
    pub bridge: &'a WorldBridge,
    pub hooks: &'a mut ModHooks,
    pub renderer: Option<&'a Renderer>,
    /// Out: what a mod asked of the pointer lock while it looked at the event (the app carries it out).
    pub pointer_lock: Option<bool>,
}

/// The movement path a game UI shows and the plot of its head selected unit.
pub type Selection = (Vec<render::entities::PathStep>, Option<(i32, i32)>);

/// The sounds a game UI asks for: the classic layer's button sounds and Lua requests, the options menu's volume
/// knobs and the events and state the music follows.
pub trait UiSoundSource {
    /// The volume knobs that moved since the last call: `(knob, 0..=1)`.
    fn take_knobs(&self) -> Vec<(String, f32)>;
    /// The button events of the frame.
    fn take_button_sounds(&self) -> Vec<(audio::ButtonEvent, audio::ButtonFlags)>;
    /// The sound scripts asked for since the last call, oldest first.
    fn take_sound_scripts(&self) -> Vec<String>;
    /// The events the music listens to since the last call.
    fn take_music_events(&self) -> Vec<exe_api::music::MusicEvent>;
    /// The game as the music reads it.
    fn music_state(&self) -> Option<exe_api::music::MusicState>;
}

/// One game UI.
pub trait Frontend {
    fn kind(&self) -> UiKind;
    /// The UI reads the replica: the game session makes one.
    fn needs_replica(&self) -> bool;
    /// Background preparation before activation, called every frame while the host waits for it.
    fn prepare(&mut self, cx: &mut PrepareCx<'_>) -> Readiness;
    /// Takes the screen (the frames and the input are its own from now on).
    fn activate(&mut self, cx: &mut PrepareCx<'_>);
    /// Goes to sleep: no frame, no input until the next activation.
    fn deactivate(&mut self, cx: &mut PrepareCx<'_>);
    /// One frame (the active frontend only), after the game session's. Draws into `out`.
    fn frame(&mut self, cx: &mut FrameCx<'_>, out: &mut UiDrawList);
    /// One window event the app's chain left for the game UI (the active frontend only).
    fn input(&mut self, cx: &mut InputCx<'_>, ev: InputEvent);
    /// One event of the client core (every frontend, in the order the app got them).
    fn net_event(&mut self, _ev: &ClientEvent) {}
    /// The frame started on the app's clock (the active frontend only).
    fn frame_started(&mut self, _now_ms: f64) {}
    /// After the renderer drew the frame (`drawn`: it did): textures the frame asked for, frame timing.
    fn after_render(&mut self, _r: &mut Renderer, _drawn: bool) {}
    /// The game was left: what belongs to the old game goes (every frontend).
    fn new_game(&mut self);
    /// Every human has ended the turn and the server runs it (the busy cursor).
    fn turn_processing(&self) -> bool {
        false
    }
    /// Opens one of the game's screens when the skin has none (`false`: it cannot).
    fn open_screen(&mut self, _s: ScreenId) -> bool {
        false
    }
    /// The UI dispatches the core's UI events itself while it is active (the session then leaves them).
    fn dispatches_core_events(&self) -> bool {
        false
    }
    /// The row of the game's cursors the UI asks for; `None`: the world's own.
    fn cursor_row(&self) -> Option<i32> {
        None
    }
    /// The UI failed while it ran: the host gives the screen to another one.
    fn failure(&self) -> Option<String> {
        None
    }
    /// The plots of every trade route and the `TradeConnection` index of each (`None`: not now).
    fn trade_paths(&self) -> Option<crate::m2::TradePaths> {
        None
    }
    /// The trade route popup shown now.
    fn popup_route(&self) -> Option<exe_api::PopupRoute> {
        None
    }
    /// The movement path the UI shows and the plot of its head selected unit, given the cell the renderer drew the
    /// selection ring on last (`None`: the UI has no selection of its own).
    fn selection(&mut self, _ring: Option<(i32, i32)>) -> Option<Selection> {
        None
    }
    /// The UI's sounds (`None`: it has none).
    fn sounds(&self) -> Option<&dyn UiSoundSource> {
        None
    }
}

/// The registered game UIs and the one on the screen.
#[derive(Default)]
pub struct UiHost {
    fronts: Vec<Box<dyn Frontend>>,
    /// The frontend on the screen; `None`: none (interim).
    active: Option<usize>,
}

impl UiHost {
    pub fn new() -> UiHost {
        UiHost::default()
    }

    /// Registers a frontend; the first one registered takes the screen.
    pub fn register(&mut self, f: Box<dyn Frontend>) {
        self.fronts.push(f);
        if self.active.is_none() {
            self.active = Some(self.fronts.len() - 1);
        }
    }

    /// A game UI is registered (the interim game panels are off).
    pub fn has_game_ui(&self) -> bool {
        !self.fronts.is_empty()
    }

    /// A registered frontend reads the replica: the app makes the game session.
    pub fn needs_replica(&self) -> bool {
        self.fronts.iter().any(|f| f.needs_replica())
    }

    /// The kind of the frontend on the screen.
    pub fn active_kind(&self) -> Option<UiKind> {
        self.active().map(|f| f.kind())
    }

    fn active(&self) -> Option<&dyn Frontend> {
        self.active.and_then(|i| self.fronts.get(i)).map(|f| &**f)
    }

    fn active_mut(&mut self) -> Option<&mut Box<dyn Frontend>> {
        self.active.and_then(|i| self.fronts.get_mut(i))
    }

    /// The active frontend dispatches the core's UI events (the session must not drop them).
    pub fn dispatches_core_events(&self) -> bool {
        self.active().is_some_and(|f| f.dispatches_core_events())
    }

    pub fn frame_started(&mut self, now_ms: f64) {
        if let Some(f) = self.active_mut() {
            f.frame_started(now_ms);
        }
    }

    /// The active frontend's frame.
    pub fn frame(&mut self, cx: &mut FrameCx<'_>, out: &mut UiDrawList) {
        if let Some(f) = self.active_mut() {
            f.frame(cx, out);
        }
    }

    /// One window event for the active frontend.
    pub fn input(&mut self, cx: &mut InputCx<'_>, ev: InputEvent) {
        if let Some(f) = self.active_mut() {
            f.input(cx, ev);
        }
    }

    /// One event of the client core for every frontend.
    pub fn net_event(&mut self, ev: &ClientEvent) {
        for f in &mut self.fronts {
            f.net_event(ev);
        }
    }

    pub fn after_render(&mut self, r: &mut Renderer, drawn: bool) {
        if let Some(f) = self.active_mut() {
            f.after_render(r, drawn);
        }
    }

    /// The game was left: every frontend drops what belongs to it.
    pub fn new_game(&mut self) {
        for f in &mut self.fronts {
            f.new_game();
        }
    }

    pub fn turn_processing(&self) -> bool {
        self.active().is_some_and(|f| f.turn_processing())
    }

    pub fn cursor_row(&self) -> Option<i32> {
        self.active().and_then(|f| f.cursor_row())
    }

    pub fn trade_paths(&self) -> Option<crate::m2::TradePaths> {
        self.active().and_then(|f| f.trade_paths())
    }

    pub fn popup_route(&self) -> Option<exe_api::PopupRoute> {
        self.active().and_then(|f| f.popup_route())
    }

    pub fn selection(&mut self, ring: Option<(i32, i32)>) -> Option<Selection> {
        self.active_mut().and_then(|f| f.selection(ring))
    }

    /// The active frontend's sounds.
    pub fn sounds(&self) -> Option<&dyn UiSoundSource> {
        self.active().and_then(|f| f.sounds())
    }
}
