//! The host of the game's interfaces: which game UI is on the screen.
//!
//! A game UI is a [`Frontend`]: the classic original UI ([`classic::ClassicFrontend`]) or UI v2. Each is one
//! registration in the app; the host forwards the app's frame, input and render hooks to the active one only. A
//! frontend that is not active sleeps: it gets no frame and no input. The game session (`crate::session`) runs
//! before the frontends and does not depend on which one is active.
//!
//! Without a registered frontend (`?ui=interim`) the host is empty and the app's interim panels are the game screen.

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
    /// The client core is in a game being played.
    pub playing: bool,
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
    /// Opens one of the game's screens when the skin has none (`false`: it cannot). The hand-off of a screen a
    /// skin lacks calls it; nothing does yet.
    #[allow(dead_code)]
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
    /// The running skin (UI v2).
    fn skin(&self) -> Option<String> {
        None
    }
}

/// The game UI chosen for this run (`?ui=`, `AppConfig::ui_choice`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiChoice {
    /// The classic original UI (the default).
    Classic,
    /// UI v2 with a skin; `None`: the index's default skin.
    V2 { skin: Option<String> },
    /// The interim egui panels; no game UI and no replica.
    Interim,
}

impl UiChoice {
    /// Reads `?ui=`: `classic`, `v2`, `v2:<skin>` or `interim`. Anything else (absent, `m1`, garbage) is no choice:
    /// the default stays. An unknown skin falls back to the default skin when the index is read.
    pub fn parse(value: &str) -> Option<UiChoice> {
        if let Some(v2) = v2::parse_ui_param(Some(value)) {
            return Some(UiChoice::V2 { skin: v2.skin });
        }
        match value.trim() {
            "classic" => Some(UiChoice::Classic),
            "interim" => Some(UiChoice::Interim),
            _ => None,
        }
    }

    /// The value as `?ui=` and the setting write it.
    pub fn value(&self) -> String {
        match self {
            UiChoice::Classic => "classic".into(),
            UiChoice::V2 { skin: None } => "v2".into(),
            UiChoice::V2 { skin: Some(s) } => format!("v2:{s}"),
            UiChoice::Interim => "interim".into(),
        }
    }

    /// The kind of frontend that shows it; `None` for the interim panels.
    pub fn kind(&self) -> Option<UiKind> {
        match self {
            UiChoice::Classic => Some(UiKind::Classic),
            UiChoice::V2 { .. } => Some(UiKind::V2),
            UiChoice::Interim => None,
        }
    }
}

/// The name of a kind in the playtest hooks.
fn kind_name(kind: UiKind) -> &'static str {
    match kind {
        UiKind::Classic => "classic",
        UiKind::V2 => "v2",
    }
}

/// What the host counts per kind of frontend (playtest hooks, the one-active rule).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UiCounters {
    /// Frames and input events each kind got: `[classic, v2]`.
    pub frames: [u64; 2],
    pub inputs: [u64; 2],
    /// Switches done, and the last one's time from the request to the target's first frame, ms.
    pub switches: u64,
    pub last_switch_ms: Option<f64>,
    /// Frontends that failed (while they prepared or ran), and why.
    pub failed: Vec<(UiKind, String)>,
}

fn slot(kind: UiKind) -> usize {
    match kind {
        UiKind::Classic => 0,
        UiKind::V2 => 1,
    }
}

/// A switch the host waits for: the target prepares while the old UI stays on the screen.
struct Switch {
    to: usize,
    since_ms: Option<f64>,
}

/// The registered game UIs and the one on the screen (R22: exactly one gets the frames and the input).
#[derive(Default)]
pub struct UiHost {
    fronts: Vec<Box<dyn Frontend>>,
    /// The frontend on the screen; `None`: none (interim).
    active: Option<usize>,
    switch: Option<Switch>,
    counters: UiCounters,
    /// The choice in force (the last one asked for); `None`: the default.
    choice: Option<UiChoice>,
}

impl UiHost {
    pub fn new() -> UiHost {
        UiHost::default()
    }

    /// Registers a frontend; the first one registered is on the screen from the start and is where the host
    /// falls back to.
    pub fn register(&mut self, f: Box<dyn Frontend>) {
        self.fronts.push(f);
        if self.active.is_none() {
            self.active = Some(self.fronts.len() - 1);
        }
    }

    /// Asks for the frontend of `kind`: it prepares in the background while the active one stays, and takes the
    /// screen in the frame it is ready. `false`: no such frontend is registered.
    pub fn request(&mut self, kind: UiKind) -> bool {
        let Some(to) = self.fronts.iter().position(|f| f.kind() == kind) else {
            return false;
        };
        self.switch = (self.active != Some(to)).then_some(Switch { to, since_ms: None });
        true
    }

    /// Asks for a choice: the frontend of its kind prepares and takes the screen once it is ready (see
    /// [`request`](Self::request)). `false`: no registered frontend shows it.
    pub fn ask(&mut self, choice: UiChoice) -> bool {
        let Some(kind) = choice.kind() else {
            return false;
        };
        if !self.request(kind) {
            return false;
        }
        self.choice = Some(choice);
        true
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

    /// A switch is under way.
    pub fn switching(&self) -> bool {
        self.switch.is_some()
    }

    pub fn counters(&self) -> &UiCounters {
        &self.counters
    }

    /// The choice in force: the last one asked for, else the frontend on the screen.
    pub fn choice(&self) -> Option<UiChoice> {
        self.choice.clone().or_else(|| match self.active_kind()? {
            UiKind::Classic => Some(UiChoice::Classic),
            UiKind::V2 => Some(UiChoice::V2 {
                skin: self.active().and_then(|f| f.skin()),
            }),
        })
    }

    /// The playtest's view of the game UIs (`state().ui`); `saved` is the player's stored choice.
    pub fn test_view(&self, saved: Option<String>) -> crate::test_port::TestUi {
        let c = &self.counters;
        crate::test_port::TestUi {
            active: self.active_kind().map(kind_name),
            choice: self.choice().map_or_else(String::new, |c| c.value()),
            saved,
            skin: self
                .fronts
                .iter()
                .find(|f| f.kind() == UiKind::V2)
                .and_then(|f| f.skin()),
            switching: self.switching(),
            switches: c.switches,
            last_switch_ms: c.last_switch_ms.map(|ms| ms.max(0.0).round() as u64),
            failed: c
                .failed
                .iter()
                .map(|(k, why)| (kind_name(*k), why.clone()))
                .collect(),
            notice: None,
            frames: c.frames,
            inputs: c.inputs,
            replica_digest: None,
        }
    }

    /// `__civ5stats.counters`: `ui.active` (0 none, 1 classic, 2 UI v2), `ui.switching`, `ui.switches`,
    /// `ui.switch_ms`, and the frames and input events each kind got (`ui.frames.classic`, `ui.inputs.v2`, ...).
    pub fn publish(&self) {
        let c = self.counters();
        let active = self.active_kind().map_or(0.0, |k| (slot(k) + 1) as f64);
        render::perf::set_counter("ui.active", active);
        render::perf::set_counter("ui.switching", f64::from(u8::from(self.switching())));
        render::perf::set_counter("ui.switches", c.switches as f64);
        render::perf::set_counter("ui.switch_ms", c.last_switch_ms.unwrap_or(-1.0));
        render::perf::set_counter("ui.frames.classic", c.frames[0] as f64);
        render::perf::set_counter("ui.frames.v2", c.frames[1] as f64);
        render::perf::set_counter("ui.inputs.classic", c.inputs[0] as f64);
        render::perf::set_counter("ui.inputs.v2", c.inputs[1] as f64);
    }

    fn active(&self) -> Option<&dyn Frontend> {
        self.active.and_then(|i| self.fronts.get(i)).map(|f| &**f)
    }

    fn active_mut(&mut self) -> Option<&mut Box<dyn Frontend>> {
        self.active.and_then(|i| self.fronts.get_mut(i))
    }

    /// Before the frame: an active frontend that failed gives the screen back to the first one; a requested one
    /// that is ready takes it, the old one goes to sleep (both in this frame, before any frame of either).
    pub fn step(&mut self, cx: &mut PrepareCx<'_>) {
        if let Some(a) = self.active
            && let Some(why) = self.fronts[a].failure()
        {
            let kind = self.fronts[a].kind();
            tracing::warn!("the {kind:?} interface failed: {why}");
            self.counters.failed.push((kind, why));
            self.fronts[a].deactivate(cx);
            self.active = None;
            if self.switch.as_ref().is_some_and(|s| s.to == a) {
                self.switch = None;
            }
            if a != 0 && !self.fronts.is_empty() {
                self.fronts[0].activate(cx);
                self.active = Some(0);
            }
        }
        let Some(sw) = self.switch.as_mut() else {
            return;
        };
        let started = *sw.since_ms.get_or_insert(cx.now_ms);
        let to = sw.to;
        match self.fronts[to].prepare(cx) {
            Readiness::Loading { .. } => {}
            Readiness::Ready => {
                if let Some(a) = self.active {
                    self.fronts[a].deactivate(cx);
                }
                self.fronts[to].activate(cx);
                self.active = Some(to);
                self.switch = None;
                self.counters.switches += 1;
                self.counters.last_switch_ms = Some(cx.now_ms - started);
                tracing::info!(
                    kind = ?self.fronts[to].kind(),
                    ms = cx.now_ms - started,
                    "the interface switched"
                );
            }
            Readiness::Failed(why) => {
                let kind = self.fronts[to].kind();
                tracing::warn!(
                    "the {kind:?} interface cannot be used, the current one stays: {why}"
                );
                self.counters.failed.push((kind, why));
                self.switch = None;
            }
        }
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

    /// Runs `f` on the active frontend as its frame (counted).
    fn on_frame(&mut self, f: impl FnOnce(&mut dyn Frontend)) {
        if let Some(a) = self.active {
            self.counters.frames[slot(self.fronts[a].kind())] += 1;
            f(&mut *self.fronts[a]);
        }
    }

    /// Runs `f` on the active frontend as an input event (counted).
    fn on_input(&mut self, f: impl FnOnce(&mut dyn Frontend)) {
        if let Some(a) = self.active {
            self.counters.inputs[slot(self.fronts[a].kind())] += 1;
            f(&mut *self.fronts[a]);
        }
    }

    /// The active frontend's frame; the sleeping ones get none.
    pub fn frame(&mut self, cx: &mut FrameCx<'_>, out: &mut UiDrawList) {
        self.on_frame(|f| f.frame(cx, out));
    }

    /// One window event for the active frontend; the sleeping ones get none.
    pub fn input(&mut self, cx: &mut InputCx<'_>, ev: InputEvent) {
        self.on_input(|f| f.input(cx, ev));
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

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// What a fake frontend saw.
    #[derive(Debug, Default)]
    struct Seen {
        frames: u32,
        inputs: u32,
        activations: u32,
        deactivations: u32,
        nets: u32,
    }

    struct Fake {
        kind: UiKind,
        ready: Rc<RefCell<Readiness>>,
        failed: Rc<RefCell<Option<String>>>,
        seen: Rc<RefCell<Seen>>,
    }

    type Handles = (
        Rc<RefCell<Readiness>>,
        Rc<RefCell<Option<String>>>,
        Rc<RefCell<Seen>>,
    );

    fn fake(kind: UiKind) -> (Box<dyn Frontend>, Handles) {
        let ready = Rc::new(RefCell::new(Readiness::Loading { progress: None }));
        let failed = Rc::new(RefCell::new(None));
        let seen = Rc::new(RefCell::new(Seen::default()));
        let f = Fake {
            kind,
            ready: Rc::clone(&ready),
            failed: Rc::clone(&failed),
            seen: Rc::clone(&seen),
        };
        (Box::new(f), (ready, failed, seen))
    }

    impl Frontend for Fake {
        fn kind(&self) -> UiKind {
            self.kind
        }
        fn needs_replica(&self) -> bool {
            true
        }
        fn prepare(&mut self, _cx: &mut PrepareCx<'_>) -> Readiness {
            self.ready.borrow().clone()
        }
        fn activate(&mut self, _cx: &mut PrepareCx<'_>) {
            self.seen.borrow_mut().activations += 1;
        }
        fn deactivate(&mut self, _cx: &mut PrepareCx<'_>) {
            self.seen.borrow_mut().deactivations += 1;
        }
        fn frame(&mut self, _cx: &mut FrameCx<'_>, _out: &mut UiDrawList) {
            self.seen.borrow_mut().frames += 1;
        }
        fn input(&mut self, _cx: &mut InputCx<'_>, _ev: InputEvent) {
            self.seen.borrow_mut().inputs += 1;
        }
        fn net_event(&mut self, _ev: &ClientEvent) {
            self.seen.borrow_mut().nets += 1;
        }
        fn new_game(&mut self) {}
        fn failure(&self) -> Option<String> {
            self.failed.borrow().clone()
        }
    }

    fn cx(now_ms: f64) -> PrepareCx<'static> {
        PrepareCx {
            now_ms,
            size: (1280, 720),
            dpr: 1.0,
            session: None,
            playing: false,
        }
    }

    /// One frame as the app runs it: the step, then a frame and an input for whoever is active (the frontends'
    /// `frame` and `input` need the app's parts; the host's routing is what is tested).
    fn frame(h: &mut UiHost, now_ms: f64) {
        h.step(&mut cx(now_ms));
        h.on_frame(|f| f.frame_started(now_ms));
        h.on_input(|f| f.frame_started(now_ms));
    }

    #[test]
    fn the_first_frontend_is_on_the_screen_and_a_ready_one_takes_it_in_one_frame() {
        let mut h = UiHost::new();
        assert!(!h.has_game_ui() && h.active_kind().is_none());
        let (classic, _) = fake(UiKind::Classic);
        let (v2, (ready, _, _)) = fake(UiKind::V2);
        h.register(classic);
        h.register(v2);
        assert!(h.has_game_ui() && h.needs_replica());
        assert_eq!(h.active_kind(), Some(UiKind::Classic));
        assert!(h.request(UiKind::V2) && h.switching());
        for t in 0..3 {
            frame(&mut h, f64::from(t) * 16.0);
        }
        assert_eq!(
            h.active_kind(),
            Some(UiKind::Classic),
            "the old one stays while the new one loads"
        );
        *ready.borrow_mut() = Readiness::Ready;
        frame(&mut h, 100.0);
        assert_eq!(h.active_kind(), Some(UiKind::V2));
        assert!(!h.switching());
        let c = h.counters();
        assert_eq!((c.switches, c.last_switch_ms), (1, Some(100.0)));
        assert_eq!(c.frames, [3, 1], "the frame of the switch is the new one's");
        assert_eq!(c.inputs, [3, 1]);
    }

    #[test]
    fn a_sleeping_frontend_gets_no_frame_and_no_input_but_the_core_events() {
        let mut h = UiHost::new();
        let (classic, (classic_ready, _, cs)) = fake(UiKind::Classic);
        let (v2, (ready, _, vs)) = fake(UiKind::V2);
        h.register(classic);
        h.register(v2);
        *ready.borrow_mut() = Readiness::Ready;
        h.request(UiKind::V2);
        frame(&mut h, 0.0);
        let ev = ClientEvent::ViewChanged;
        for t in 1..10 {
            frame(&mut h, f64::from(t));
            h.net_event(&ev);
        }
        assert_eq!((cs.borrow().deactivations, vs.borrow().activations), (1, 1));
        assert_eq!(h.counters().frames, [0, 10]);
        assert_eq!(h.counters().inputs, [0, 10]);
        assert_eq!((cs.borrow().nets, vs.borrow().nets), (9, 9));
        // Back, once the classic one is ready.
        *cs.borrow_mut() = Seen::default();
        *classic_ready.borrow_mut() = Readiness::Ready;
        h.request(UiKind::Classic);
        frame(&mut h, 20.0);
        assert_eq!(h.active_kind(), Some(UiKind::Classic));
        assert_eq!(cs.borrow().activations, 1);
        assert_eq!(vs.borrow().deactivations, 1);
    }

    #[test]
    fn a_frontend_that_fails_to_prepare_leaves_the_current_one() {
        let mut h = UiHost::new();
        let (classic, _) = fake(UiKind::Classic);
        let (v2, (ready, _, vs)) = fake(UiKind::V2);
        h.register(classic);
        h.register(v2);
        h.request(UiKind::V2);
        *ready.borrow_mut() = Readiness::Failed("no index".into());
        frame(&mut h, 0.0);
        frame(&mut h, 16.0);
        assert_eq!(h.active_kind(), Some(UiKind::Classic));
        assert!(!h.switching());
        assert_eq!(h.counters().failed, [(UiKind::V2, "no index".to_owned())]);
        assert_eq!(vs.borrow().activations, 0);
        assert_eq!(h.counters().frames, [2, 0]);
        // Asking for an unregistered kind changes nothing.
        let mut lone = UiHost::new();
        lone.register(fake(UiKind::Classic).0);
        assert!(!lone.request(UiKind::V2) && !lone.switching());
    }

    #[test]
    fn a_frontend_that_fails_while_it_runs_gives_the_screen_back_to_the_first_once() {
        let mut h = UiHost::new();
        let (classic, (_, _, cs)) = fake(UiKind::Classic);
        let (v2, (ready, failed, _)) = fake(UiKind::V2);
        h.register(classic);
        h.register(v2);
        *ready.borrow_mut() = Readiness::Ready;
        h.request(UiKind::V2);
        frame(&mut h, 0.0);
        assert_eq!(h.active_kind(), Some(UiKind::V2));
        *failed.borrow_mut() = Some("scripts failed".into());
        for t in 1..5 {
            frame(&mut h, f64::from(t));
        }
        assert_eq!(h.active_kind(), Some(UiKind::Classic));
        assert_eq!(cs.borrow().activations, 1, "no loop");
        assert_eq!(h.counters().failed.len(), 1);
    }

    #[test]
    fn the_test_view_names_the_ui_on_the_screen_and_the_choice() {
        let mut h = UiHost::new();
        let (classic, _) = fake(UiKind::Classic);
        let (v2, (ready, _, _)) = fake(UiKind::V2);
        h.register(classic);
        h.register(v2);
        let v = h.test_view(None);
        assert_eq!((v.active, v.choice.as_str()), (Some("classic"), "classic"));
        assert!(
            !h.ask(UiChoice::Interim),
            "the interim panels are no frontend"
        );
        let engraved = UiChoice::V2 {
            skin: Some("engraved".into()),
        };
        assert!(h.ask(engraved.clone()));
        frame(&mut h, 10.0);
        let v = h.test_view(None);
        assert_eq!(v.active, Some("classic"));
        assert!(v.switching);
        *ready.borrow_mut() = Readiness::Ready;
        frame(&mut h, 30.0);
        let v = h.test_view(Some("v2".into()));
        assert_eq!(v.active, Some("v2"));
        assert_eq!(v.choice, "v2:engraved");
        assert_eq!(v.saved.as_deref(), Some("v2"));
        assert_eq!((v.switches, v.last_switch_ms), (1, Some(20)));
        assert_eq!(v.frames, [1, 1]);
        assert_eq!(h.choice(), Some(engraved));
    }

    #[test]
    fn the_ui_param_is_read() {
        let v2 = |skin: Option<&str>| {
            Some(UiChoice::V2 {
                skin: skin.map(str::to_owned),
            })
        };
        assert_eq!(UiChoice::parse("classic"), Some(UiChoice::Classic));
        assert_eq!(UiChoice::parse("interim"), Some(UiChoice::Interim));
        assert_eq!(UiChoice::parse("v2"), v2(None));
        assert_eq!(UiChoice::parse("v2:engraved"), v2(Some("engraved")));
        for other in ["", "m1", "v3", "Classic", "v2;x"] {
            assert_eq!(UiChoice::parse(other), None, "{other:?}");
        }
        // The value reads back as itself.
        for value in ["classic", "interim", "v2", "v2:engraved"] {
            assert_eq!(
                UiChoice::parse(value).map(|c| c.value()).as_deref(),
                Some(value)
            );
        }
    }
}
