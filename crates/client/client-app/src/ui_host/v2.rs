//! Temporary wiring of UI v2 into the app (`?ui=v2`, `?ui=v2:<skin>`), for the first slice only.
//!
//! The classic original UI keeps running as before (its replica pumps, it consumes the core's events), but while a
//! skin runs its draw list goes to a scratch list nobody submits and it gets no input. UI v2 reads the replica
//! through temporary getters of the classic layer; once the game session owns the replica and the frontends switch
//! through one interface, this module is replaced.
//!
//! The network reaches UI v2 the same temporary way: the app hands each of the core's events to
//! [`V2Host::net_event`] (kept, as the actions module's events, until the next UI v2 frame) and lends the network
//! handle to [`V2Host::frame`], which runs the UI v2 frame inside one borrow of the core ([`NetSink`]). Camera
//! requests of the frame go to the classic layer's look-at, which the app carries out on its next frame.
//!
//! Input: while a skin runs every window event the app's chain leaves for the game UI goes to [`V2Host::input`]
//! first. What the skin takes stops there; what it passes to the world goes to the camera and the picker, and a
//! click the picker turns into a plot comes back as [`V2Host::map_click`] (`map.click` for the actions module)
//! instead of the classic UI's click handling; a press on the skin's minimap moves the camera there. The cursor the
//! last frame reported is shown with the game's cursors ([`cursor_row`]).
//!
//! Loading: the index `ui2/skins.json` and the chosen package are fetched relative to the page. Any failure (no
//! index, a bad index, a package that is missing, damaged, of another hash, or that does not build, or scripts that
//! fail while loading) is a warning and a fallback: the default skin if another one failed, else the classic UI.
//! Never a panic, never a retry loop.
//!
//! Scripts: the platform hands its script workers over with [`set_script_host`] before `run` (the web page's
//! workers); without one a skin's scripts do not run (`NullScriptHost`) and the skin works on its markup.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use client_core::ClientEvent;
use draw_list::UiDrawList;
use platform::{
    HttpClient, HttpError, HttpOp, HttpResponse, InputEvent, NullScriptHost, ScriptHost,
};
use protocol::GameId;
use ui2_host::{
    ActionEvent, Cursor, Env, Fallback, HostClock, InputOutcome, MapButton, NetSink, ReplicaView,
    Services, SkinRegistry, V2Frame, V2FrameReport, V2Init, V2Ui,
};

use crate::net::NetHandle;

/// The skin index of the web build, relative to the page.
pub const INDEX_PATH: &str = "ui2/skins.json";

/// The most network events kept for UI v2 while it runs no frame (loading, a mod owning the screen); the oldest
/// go first.
const MAX_EVENTS: usize = 1024;

/// What the actions module must know of a core event. `game` is the game the last `Attached` named: attaching
/// to the same game again (a reconnect) keeps the UI's state; another game, or leaving the game, is a new game.
pub(crate) fn action_event(ev: &ClientEvent, game: &mut Option<GameId>) -> Option<ActionEvent> {
    match ev {
        ClientEvent::OrderAnswered { id, outcome } => Some(ActionEvent::Answered {
            id: *id,
            outcome: outcome.clone(),
        }),
        ClientEvent::OrderRolledBack { id, text } => Some(ActionEvent::RolledBack {
            id: *id,
            text: text.clone(),
        }),
        ClientEvent::EndTurnCancelled { by_order } => Some(ActionEvent::EndTurnCancelled {
            by_order: *by_order,
        }),
        ClientEvent::Attached { game: g, .. } => {
            (game.replace(*g) != Some(*g)).then_some(ActionEvent::NewGame)
        }
        ClientEvent::Detached(_) => {
            *game = None;
            Some(ActionEvent::NewGame)
        }
        _ => None,
    }
}

/// The ids of a mod set's mods in load order: the index of a mod there is the index its intents carry.
pub(crate) fn mod_ids(set: &protocol::ModSetView) -> Vec<Arc<str>> {
    let mut mods: Vec<_> = set.mods.iter().collect();
    mods.sort_by_key(|m| m.load_idx);
    mods.into_iter().map(|m| Arc::from(m.id.as_str())).collect()
}

/// The row of the game's `Cursors` table (see [`crate::cursor`]) that shows the cursor a skin asked for; `None` for
/// `auto`, where the world's own cursor stays. The game has no hand or text beam of its own: a pointer is its
/// link cursor, text its edit cursor, grabbing its grip.
pub(crate) fn cursor_row(c: Cursor) -> Option<i32> {
    match c {
        Cursor::Auto => None,
        Cursor::Default | Cursor::NotAllowed => Some(0),
        Cursor::Move => Some(12),
        Cursor::Pointer => Some(20),
        Cursor::Grab | Cursor::Grabbing => Some(21),
        Cursor::Text => Some(22),
    }
}

/// A point of the skin's minimap picture (0 to 1) as the share the renderer's `minimap_to_world` takes
/// (0 to 65535).
pub(crate) fn minimap_share(t: f32) -> u16 {
    (t.clamp(0.0, 1.0) * f32::from(u16::MAX)).round() as u16
}

/// What `?ui=` asked of UI v2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V2Choice {
    /// `v2:<skin>`; `None` for plain `v2` (the index's default skin).
    pub skin: Option<String>,
}

/// Reads the value of `?ui=`: `v2` or `v2:<skin>` choose UI v2; anything else (absent, `classic`, `interim`, `m1`,
/// garbage) is not a UI v2 choice. An empty skin (`v2:`) means the default one; an unknown skin falls back to the
/// default when the index is read.
pub fn parse_ui_param(value: Option<&str>) -> Option<V2Choice> {
    let rest = value?.trim().strip_prefix("v2")?;
    if rest.is_empty() {
        return Some(V2Choice { skin: None });
    }
    let skin = rest.strip_prefix(':')?.trim();
    Some(V2Choice {
        skin: (!skin.is_empty()).then(|| skin.to_owned()),
    })
}

/// Makes the script host for a skin; each skin that loads gets a new one.
pub type ScriptHostFactory = Rc<dyn Fn() -> Box<dyn ScriptHost>>;

thread_local! {
    static CHOICE: RefCell<Option<V2Choice>> = const { RefCell::new(None) };
    static SCRIPTS: RefCell<Option<ScriptHostFactory>> = const { RefCell::new(None) };
}

/// The platform's script workers for UI v2 skins, set before `run` (the web page's). Without it skins run their
/// markup only.
pub fn set_script_host(factory: ScriptHostFactory) {
    SCRIPTS.with(|s| *s.borrow_mut() = Some(factory));
}

/// The factory set by [`set_script_host`], taken once by the app.
fn take_script_host() -> Option<ScriptHostFactory> {
    SCRIPTS.with(|s| s.borrow_mut().take())
}

/// The page's `?ui=` value, set before `run`: a UI v2 choice makes the app load a skin over the classic layer. Any
/// other value changes nothing.
pub fn set_ui_param(value: Option<&str>) {
    let choice = parse_ui_param(value);
    if let Some(c) = &choice {
        tracing::info!(skin = ?c.skin, "UI v2 requested");
    }
    CHOICE.with(|c| *c.borrow_mut() = choice);
}

/// The choice set by [`set_ui_param`], taken once by the app.
pub(crate) fn take_choice() -> Option<V2Choice> {
    CHOICE.with(|c| c.borrow_mut().take())
}

/// Where the loading is.
enum Stage {
    /// Waiting for the index.
    Index(Box<dyn HttpOp>),
    /// Waiting for a skin's package.
    Package { id: String, op: Box<dyn HttpOp> },
    /// The skin runs.
    Running(Box<V2Ui>),
    /// UI v2 is off for this session: the classic UI draws.
    Classic,
}

/// What the loading stage is (tests; the playtest hooks will read it once they exist).
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum V2State {
    FetchingIndex,
    FetchingSkin(String),
    Running(String),
    Classic,
}

/// UI v2 over the classic layer: the loading, the fallback, the frame.
pub struct V2Host {
    http: Rc<dyn HttpClient>,
    choice: V2Choice,
    registry: SkinRegistry,
    stage: Stage,
    clock: HostClock,
    started_ms: f64,
    /// `ui2.load_ms` waits to be published (once, after the skin came up).
    load_ms: Option<f64>,
    /// The localizer handed to the running skin.
    loc: Option<Rc<loc::Localizer>>,
    /// Where the classic layer draws while a skin runs: cleared every frame, never submitted.
    scratch: UiDrawList,
    /// The network's events for the actions module since the last UI v2 frame, and the game they belong to.
    events: Vec<ActionEvent>,
    game: Option<GameId>,
    /// The game's mod set, when it changed since the last UI v2 frame.
    mods: Option<Vec<Arc<str>>>,
    /// The cursor the last UI v2 frame reported.
    cursor: Cursor,
    /// The script workers of each skin that loads; `None`: scripts do not run.
    scripts: Option<ScriptHostFactory>,
}

impl V2Host {
    /// Starts fetching the index.
    pub fn new(choice: V2Choice, http: Rc<dyn HttpClient>, clock: HostClock) -> V2Host {
        let started_ms = clock();
        tracing::info!(url = INDEX_PATH, "UI v2: fetching the skin index");
        let op = http.get(INDEX_PATH);
        V2Host {
            http,
            choice,
            registry: SkinRegistry::default(),
            stage: Stage::Index(op),
            clock,
            started_ms,
            load_ms: None,
            loc: None,
            scratch: UiDrawList::new(),
            events: Vec::new(),
            game: None,
            mods: None,
            cursor: Cursor::Auto,
            scripts: take_script_host(),
        }
    }

    /// One window event, before the world sees it: `None` while no skin runs (the classic UI's path stays as it
    /// is), else who takes it. `now_ms` is the app's clock.
    pub fn input(&mut self, ev: &InputEvent, now_ms: f64) -> Option<InputOutcome> {
        self.ui_mut().map(|ui| ui.handle_input(ev, now_ms))
    }

    /// A click the world took and the picker turned into the plot `cell`: a `map.click` for the next UI v2 frame.
    pub fn map_click(&mut self, cell: (i32, i32), button: MapButton) {
        if let Some(ui) = self.ui_mut() {
            ui.click_map(cell.0, cell.1, button);
        }
    }

    /// The row of the game's cursors the running skin asks for (its last frame's cursor); `None`: no skin runs,
    /// or the skin leaves the cursor to the world.
    pub fn cursor_row(&self) -> Option<i32> {
        if self.running() {
            cursor_row(self.cursor)
        } else {
            None
        }
    }

    /// One event of the client core, in the order the app got them: kept for the next UI v2 frame. Nothing is
    /// kept once UI v2 is off for the session.
    pub fn net_event(&mut self, ev: &ClientEvent) {
        if matches!(self.stage, Stage::Classic) {
            self.events.clear();
            return;
        }
        if let ClientEvent::ModSet(set) = ev {
            self.mods = Some(mod_ids(set));
        }
        if let Some(a) = action_event(ev, &mut self.game) {
            if self.events.len() >= MAX_EVENTS {
                tracing::warn!(
                    "UI v2: no frame for {MAX_EVENTS} network events; the oldest is dropped"
                );
                self.events.remove(0);
            }
            self.events.push(a);
        }
    }

    /// The loading stage.
    #[cfg(test)]
    pub fn state(&self) -> V2State {
        match &self.stage {
            Stage::Index(_) => V2State::FetchingIndex,
            Stage::Package { id, .. } => V2State::FetchingSkin(id.clone()),
            Stage::Running(ui) => V2State::Running(ui.skin_id().to_owned()),
            Stage::Classic => V2State::Classic,
        }
    }

    /// A skin runs: the classic layer neither draws nor gets input.
    pub fn running(&self) -> bool {
        matches!(self.stage, Stage::Running(_))
    }

    /// While a skin runs: an empty list for the classic layer to draw into, which is never submitted.
    pub fn classic_sink(&mut self) -> Option<&mut UiDrawList> {
        if !self.running() {
            return None;
        }
        self.scratch.clear();
        Some(&mut self.scratch)
    }

    /// The running UI v2.
    pub fn ui_mut(&mut self) -> Option<&mut V2Ui> {
        match &mut self.stage {
            Stage::Running(ui) => Some(&mut **ui),
            _ => None,
        }
    }

    /// Advances the loading with what the network answered; `env` is the window the skin is built for.
    pub fn poll(&mut self, env: Env) {
        // At most one step per kind per call: a fallback starts a new request, answered on a later frame.
        let stage = std::mem::replace(&mut self.stage, Stage::Classic);
        self.stage = match stage {
            Stage::Index(mut op) => match op.poll() {
                None => Stage::Index(op),
                Some(answer) => self.on_index(answer),
            },
            Stage::Package { id, mut op } => match op.poll() {
                None => Stage::Package { id, op },
                Some(answer) => self.on_package(id, answer, env),
            },
            other => other,
        };
    }

    fn on_index(&mut self, answer: Result<HttpResponse, HttpError>) -> Stage {
        let body = match body_of(INDEX_PATH, answer) {
            Ok(b) => b,
            Err(why) => return classic(&why),
        };
        let text = match std::str::from_utf8(&body) {
            Ok(t) => t,
            Err(_) => return classic(&format!("{INDEX_PATH} is not UTF-8")),
        };
        match SkinRegistry::parse(text) {
            Ok(r) => self.registry = r,
            Err(e) => return classic(&e.to_string()),
        }
        let want = self.choice.skin.as_deref();
        let Some(entry) = self.registry.resolve(want) else {
            return classic("the skin index has no skin to start");
        };
        if let Some(w) = want
            && w != entry.id
        {
            tracing::warn!(
                "UI v2: the index has no skin {w:?}; starting the default skin {}",
                entry.id
            );
        }
        let id = entry.id.clone();
        self.fetch(id)
    }

    fn fetch(&mut self, id: String) -> Stage {
        let Some(entry) = self.registry.get(&id) else {
            return classic(&format!("skin {id} is not in the index"));
        };
        let url = entry.path();
        tracing::info!(%url, "UI v2: fetching skin {id}");
        let op = self.http.get(&url);
        Stage::Package { id, op }
    }

    fn on_package(
        &mut self,
        id: String,
        answer: Result<HttpResponse, HttpError>,
        env: Env,
    ) -> Stage {
        let path = self
            .registry
            .get(&id)
            .map_or_else(|| id.clone(), ui2_host::SkinEntry::path);
        let built = body_of(&path, answer)
            .and_then(|bytes| {
                self.registry
                    .open(&id, Arc::from(bytes))
                    .map_err(|e| e.to_string())
            })
            .and_then(|pkg| {
                let scripts: Box<dyn ScriptHost> = match &self.scripts {
                    Some(make) => make(),
                    None => Box::new(NullScriptHost),
                };
                let init = V2Init {
                    env,
                    services: Services {
                        loc: self.loc.clone(),
                    },
                    clock: Some(Rc::clone(&self.clock)),
                    scripts: Some(scripts),
                };
                V2Ui::new(pkg, init).map_err(|e| e.to_string())
            });
        match built {
            Ok(ui) => {
                let ms = (self.clock)() - self.started_ms;
                let load = ui.load_stats();
                tracing::info!(
                    skin = %id,
                    load_ms = ms,
                    build_ms = load.build_ms,
                    nodes = load.nodes,
                    "UI v2: skin is up; the classic UI stops drawing"
                );
                self.load_ms = Some(ms);
                Stage::Running(Box::new(ui))
            }
            Err(why) => self.fall_back(&id, &why),
        }
    }

    /// A skin failed: it is not tried again; the default skin if it was another one, else the classic UI.
    fn fall_back(&mut self, id: &str, why: &str) -> Stage {
        match self.registry.fail(id) {
            Fallback::Skin(next) => {
                tracing::warn!("UI v2: skin {id} cannot be used ({why}); trying skin {next}");
                self.fetch(next)
            }
            Fallback::Classic => classic(&format!("skin {id} cannot be used ({why})")),
        }
    }

    /// One frame after the classic layer's: advances the loading and, while a skin runs and the game's layer is up,
    /// runs UI v2 on the replica and the network (`net`: the app's handle; `None` without a network) and publishes
    /// its counters. `Some`: the scene of the returned UI goes to the renderer (`prepare_ui2`) before it draws.
    ///
    /// The network handle is borrowed for the UI v2 frame only, in the app's frame: never inside a call into page
    /// script (the frame, the timer and the page events never run inside one another).
    pub(crate) fn frame(
        &mut self,
        m2: &mut crate::m2::M2Ui,
        net: Option<&NetHandle>,
        env: Env,
        now_ms: f64,
    ) -> Option<&mut V2Ui> {
        self.poll(env);
        if !self.running() || m2.layer().is_none() {
            return None;
        }
        // TEMP(T51): the replica and the localizer come from the classic layer.
        let loc = m2.v2_localizer();
        let new_loc = match (&loc, &self.loc) {
            (Some(a), Some(b)) => !Rc::ptr_eq(a, b),
            (a, b) => a.is_some() != b.is_some(),
        };
        let replica = m2.v2_replica();
        let steps = m2.v2_take_steps();
        let load_ms = self.load_ms.take();
        let Stage::Running(ui) = &mut self.stage else {
            return None;
        };
        let ui = &mut **ui;
        if new_loc && let Some(l) = &loc {
            ui.set_localizer(Rc::clone(l));
        }
        self.loc = loc;
        ui.set_env(env);
        let mut held = match replica.as_ref().map(|r| r.try_borrow_mut()) {
            Some(Ok(r)) => Some(r),
            // The layer holds the replica (it never does between frames): the last scene stays, the events wait.
            Some(Err(_)) => return self.ui_mut(),
            None => None,
        };
        let steps: &[replica::ReplicaStep] = if held.is_some() { &steps } else { &[] };
        let events = std::mem::take(&mut self.events);
        let mods = self.mods.take();
        let (report, look) = match net {
            Some(h) => h.with(|core| {
                let mut sink = NetSink::new(core);
                let report = ui.frame(V2Frame {
                    replica: held.as_deref_mut().map(ReplicaView::of),
                    steps,
                    net: Some(&mut sink),
                    events: &events,
                    mods,
                    now_ms,
                });
                (report, sink.look_at())
            }),
            None => {
                let report = ui.frame(V2Frame {
                    replica: held.as_deref_mut().map(ReplicaView::of),
                    steps,
                    net: None,
                    events: &events,
                    mods,
                    now_ms,
                });
                (report, None)
            }
        };
        // TEMP(T51): the camera goes where the classic UI's `UI.LookAt` would send it.
        if let Some((x, y)) = look {
            m2.v2_look_at(x, y);
        }
        self.cursor = report.cursor;
        publish(&report, load_ms);
        if !self.after_frame(&report) {
            return None;
        }
        self.ui_mut()
    }

    /// What a frame's report means for the loading: scripts that failed while loading make the skin unusable, as
    /// any load failure (the default skin, else the classic UI). `false`: no skin runs any more.
    fn after_frame(&mut self, report: &V2FrameReport) -> bool {
        if let Some(e) = &report.fatal {
            let failure = e.failure.to_string();
            self.stage = self.fall_back(&e.skin, &failure);
        }
        self.running()
    }
}

/// The counters of a frame go to `__civ5stats.counters`; the renderer adds its own `ui2.*` (draws per layer,
/// instances, uploads) after `prepare_ui2`, so the report's instance count is left to it.
fn publish(report: &V2FrameReport, load_ms: Option<f64>) {
    if let Some(ms) = load_ms {
        render::perf::set_counter("ui2.load_ms", ms);
    }
    for (name, value) in report.counters() {
        if name != "ui2.instances" {
            render::perf::set_counter(name, value);
        }
    }
}

/// The body of a successful answer, or why there is none.
fn body_of(url: &str, answer: Result<HttpResponse, HttpError>) -> Result<Vec<u8>, String> {
    match answer {
        Ok(r) if r.is_success() => Ok(r.body),
        Ok(r) => Err(format!("{url}: HTTP {}", r.status)),
        Err(e) => Err(format!("{url}: {e}")),
    }
}

fn classic(why: &str) -> Stage {
    tracing::warn!("UI v2 is off, the classic UI stays: {why}");
    Stage::Classic
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;

    use ui2_package::Layer;
    use ui2_package::style::{Display, Len, StyleRecord};
    use ui2_runtime::testkit::{Kit, TNode};

    use super::*;

    #[test]
    fn the_ui_param_chooses_v2() {
        let v2 = |skin: Option<&str>| {
            Some(V2Choice {
                skin: skin.map(str::to_owned),
            })
        };
        assert_eq!(parse_ui_param(Some("v2")), v2(None));
        assert_eq!(parse_ui_param(Some("v2:engraved")), v2(Some("engraved")));
        assert_eq!(parse_ui_param(Some(" v2:vellum ")), v2(Some("vellum")));
        // An empty skin is the default one; an unknown one is resolved against the index later.
        assert_eq!(parse_ui_param(Some("v2:")), v2(None));
        assert_eq!(parse_ui_param(Some("v2:No Such")), v2(Some("No Such")));
        // Absent, the other UIs and garbage: not UI v2.
        for other in [
            None,
            Some(""),
            Some("classic"),
            Some("interim"),
            Some("m1"),
            Some("v2x"),
            Some("v3"),
            Some("v2;engraved"),
            Some("V2"),
            Some("xv2"),
        ] {
            assert_eq!(parse_ui_param(other), None, "{other:?}");
        }
    }

    #[test]
    fn core_events_become_action_events() {
        use protocol::{AttachMode, CommandOutcome, DetachReason};
        let mut game = None;
        let mut map = |ev: ClientEvent| action_event(&ev, &mut game);
        assert_eq!(
            map(ClientEvent::OrderAnswered {
                id: 4,
                outcome: CommandOutcome::Applied
            }),
            Some(ActionEvent::Answered {
                id: 4,
                outcome: CommandOutcome::Applied
            })
        );
        assert_eq!(
            map(ClientEvent::OrderRolledBack {
                id: 5,
                text: "lost".into()
            }),
            Some(ActionEvent::RolledBack {
                id: 5,
                text: "lost".into()
            })
        );
        assert_eq!(
            map(ClientEvent::EndTurnCancelled { by_order: true }),
            Some(ActionEvent::EndTurnCancelled { by_order: true })
        );
        let attach = |game| ClientEvent::Attached {
            game,
            mode: AttachMode::Fresh,
        };
        // The first game is a new game; attaching to it again (a reconnect) is not; another game is.
        assert_eq!(map(attach(7)), Some(ActionEvent::NewGame));
        assert_eq!(map(attach(7)), None);
        assert_eq!(map(attach(8)), Some(ActionEvent::NewGame));
        // Leaving the game is a new game, and so is coming back to it.
        assert_eq!(
            map(ClientEvent::Detached(DetachReason::Left)),
            Some(ActionEvent::NewGame)
        );
        assert_eq!(map(attach(8)), Some(ActionEvent::NewGame));
        assert_eq!(map(ClientEvent::ViewChanged), None);
    }

    #[test]
    fn mod_ids_follow_the_load_order() {
        let m = |id: &str, load_idx| protocol::ModView {
            id: id.into(),
            version: "1.0".into(),
            load_idx,
            sync: protocol::ModSync::Client,
            client_required: false,
            available: true,
            pack: None,
        };
        let set = protocol::ModSetView {
            modset_hash: 1,
            mods: vec![m("b", 1), m("a", 0)],
        };
        let ids: Vec<String> = mod_ids(&set).iter().map(|s| s.to_string()).collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn the_choice_is_taken_once() {
        set_ui_param(Some("interim"));
        assert_eq!(take_choice(), None);
        set_ui_param(Some("v2:engraved"));
        assert_eq!(
            take_choice(),
            Some(V2Choice {
                skin: Some("engraved".into())
            })
        );
        assert_eq!(take_choice(), None);
    }

    /// Answers by URL; a URL without an answer is a 404. Every answer is ready at once.
    #[derive(Default)]
    struct FakeHttp {
        answers: RefCell<BTreeMap<String, Result<HttpResponse, HttpError>>>,
        asked: RefCell<Vec<String>>,
    }

    impl FakeHttp {
        fn ok(self, url: &str, body: Vec<u8>) -> Self {
            self.answers
                .borrow_mut()
                .insert(url.to_owned(), Ok(HttpResponse { status: 200, body }));
            self
        }
    }

    struct Ready(Option<Result<HttpResponse, HttpError>>);

    impl HttpOp for Ready {
        fn poll(&mut self) -> Option<Result<HttpResponse, HttpError>> {
            self.0.take()
        }
    }

    impl HttpClient for FakeHttp {
        fn get(&self, url: &str) -> Box<dyn HttpOp> {
            self.asked.borrow_mut().push(url.to_owned());
            let answer = self
                .answers
                .borrow()
                .get(url)
                .cloned()
                .unwrap_or(Ok(HttpResponse {
                    status: 404,
                    body: Vec::new(),
                }));
            Box::new(Ready(Some(answer)))
        }
    }

    fn clock() -> HostClock {
        let t = Rc::new(Cell::new(0.0f64));
        Rc::new(move || {
            t.set(t.get() + 1.0);
            t.get()
        })
    }

    fn entry(id: &str, hash: &str, default: bool) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "file": format!("{id}-{}.ui2", &hash[..16]),
            "hash": hash,
            "api": "1.0",
            "default": default,
        })
    }

    fn index(entries: &[serde_json::Value]) -> Vec<u8> {
        serde_json::Value::Array(entries.to_vec())
            .to_string()
            .into_bytes()
    }

    const H1: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
    const H2: &str = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";

    /// A package of one text on the HUD layer, with the id `kit`.
    fn kit_package() -> Arc<ui2_package::Package> {
        let font = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../third_party/fonts/fira-sans/FiraSans-Regular.ttf"),
        )
        .expect("the font of the tests");
        let mut kit = Kit::new();
        kit.font("Fira Sans", 400, false, &font);
        let mut cell = StyleRecord::initial();
        cell.display = Display::Block;
        cell.font_family = vec![kit.str("Fira Sans")];
        cell.font_size = Len::px(14.0);
        let cell = kit.record(cell);
        let cell = kit.plain(cell);
        let hello = kit.str("Hello");
        kit.template(
            "doc",
            vec![TNode::layer(Layer::Hud, cell).child(TNode::text(hello, cell))],
        );
        kit.finish().expect("package")
    }

    fn host(http: FakeHttp, choice: Option<&str>) -> (V2Host, Rc<FakeHttp>) {
        let http = Rc::new(http);
        let shared: Rc<dyn HttpClient> = http.clone();
        let host = V2Host::new(
            V2Choice {
                skin: choice.map(str::to_owned),
            },
            shared,
            clock(),
        );
        (host, http)
    }

    /// Polls until the loading settles (each poll answers one request).
    fn settle(h: &mut V2Host) -> V2State {
        for _ in 0..8 {
            h.poll(Env::default());
        }
        h.state()
    }

    #[test]
    fn no_index_is_the_classic_ui() {
        let (mut h, http) = host(FakeHttp::default(), Some("engraved"));
        assert_eq!(h.state(), V2State::FetchingIndex);
        assert_eq!(settle(&mut h), V2State::Classic);
        assert!(!h.running() && h.classic_sink().is_none() && h.ui_mut().is_none());
        assert_eq!(
            *http.asked.borrow(),
            [INDEX_PATH],
            "nothing asked after the 404"
        );
    }

    #[test]
    fn a_broken_or_empty_index_is_the_classic_ui() {
        for body in [
            b"not json".to_vec(),
            b"{}".to_vec(),
            vec![0xff, 0xfe],
            index(&[]),
        ] {
            let (mut h, _) = host(FakeHttp::default().ok(INDEX_PATH, body), None);
            assert_eq!(settle(&mut h), V2State::Classic);
        }
        // A network failure too.
        let http = FakeHttp::default();
        http.answers
            .borrow_mut()
            .insert(INDEX_PATH.into(), Err(HttpError::Network("offline".into())));
        let (mut h, _) = host(http, None);
        assert_eq!(settle(&mut h), V2State::Classic);
    }

    #[test]
    fn a_missing_package_falls_back_to_the_default_then_classic() {
        let idx = index(&[entry("engraved", H1, true), entry("vellum", H2, false)]);
        let (mut h, http) = host(FakeHttp::default().ok(INDEX_PATH, idx), Some("vellum"));
        h.poll(Env::default());
        assert_eq!(h.state(), V2State::FetchingSkin("vellum".into()));
        h.poll(Env::default());
        assert_eq!(h.state(), V2State::FetchingSkin("engraved".into()));
        assert_eq!(settle(&mut h), V2State::Classic);
        let asked = http.asked.borrow();
        assert_eq!(
            *asked,
            [
                INDEX_PATH.to_owned(),
                format!("ui2/vellum-{}.ui2", &H2[..16]),
                format!("ui2/engraved-{}.ui2", &H1[..16]),
            ],
            "each skin is tried once"
        );
    }

    #[test]
    fn a_package_of_another_hash_or_damaged_is_refused() {
        let pkg = kit_package();
        let bytes = pkg.bytes().to_vec();
        // The index names another hash.
        let idx = index(&[entry("kit", H1, true)]);
        let file = format!("ui2/kit-{}.ui2", &H1[..16]);
        let (mut h, _) = host(
            FakeHttp::default()
                .ok(INDEX_PATH, idx)
                .ok(&file, bytes.clone()),
            None,
        );
        assert_eq!(settle(&mut h), V2State::Classic);
        // The right hash, damaged bytes.
        let full = ui2_host::hash_hex(&pkg.hash());
        let idx = index(&[entry("kit", &full, true)]);
        let file = format!("ui2/kit-{}.ui2", &full[..16]);
        let mut damaged = bytes;
        let mid = damaged.len() / 2;
        damaged[mid] ^= 0x55;
        let (mut h, _) = host(
            FakeHttp::default().ok(INDEX_PATH, idx).ok(&file, damaged),
            None,
        );
        assert_eq!(settle(&mut h), V2State::Classic);
    }

    #[test]
    fn a_good_package_runs_and_silences_the_classic_list() {
        let pkg = kit_package();
        let full = ui2_host::hash_hex(&pkg.hash());
        let idx = index(&[entry("kit", &full, true)]);
        let file = format!("ui2/kit-{}.ui2", &full[..16]);
        // `v2:gone` is not in the index: the default skin starts.
        let (mut h, _) = host(
            FakeHttp::default()
                .ok(INDEX_PATH, idx)
                .ok(&file, pkg.bytes().to_vec()),
            Some("gone"),
        );
        assert_eq!(settle(&mut h), V2State::Running("kit".into()));
        assert!(h.load_ms.is_some_and(|ms| ms > 0.0));
        assert!(h.classic_sink().is_some_and(|l| l.is_empty()));
        let ui = h.ui_mut().expect("running");
        let r = ui.frame(V2Frame {
            replica: None,
            steps: &[],
            net: None,
            events: &[],
            mods: None,
            now_ms: 16.0,
        });
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(ui.texts(), ["Hello"]);
        assert!(r.scene.raster_glyphs > 0);
    }

    /// A package with the `menu` button (10, 10, 100 x 40; cursor pointer; a click toggles the menu window) on
    /// the HUD layer.
    fn button_package() -> Arc<ui2_package::Package> {
        use ui2_package::profile::action::{Action, Arg, Event};
        use ui2_package::style::{LenAuto, Position};
        let mut kit = Kit::new();
        let mut r = StyleRecord::initial();
        r.position = Position::Absolute;
        r.left = LenAuto::Len(Len::px(10.0));
        r.top = LenAuto::Len(Len::px(10.0));
        r.width = LenAuto::Len(Len::px(100.0));
        r.height = LenAuto::Len(Len::px(40.0));
        r.cursor = Cursor::Pointer;
        let button = kit.record(r);
        let button = kit.plain(button);
        let layer = kit.record(StyleRecord::initial());
        let layer = kit.plain(layer);
        let toggle = Action::Intent {
            name: "window.toggle".into(),
            args: vec![Arg {
                name: Some("window".into()),
                value: ui2_package::profile::expr::Expr::Str("menu".into()),
            }],
        };
        kit.template(
            "doc",
            vec![
                TNode::layer(Layer::Hud, layer).child(TNode::el(button).bind(
                    ui2_package::BindKind::Event {
                        ev: Event::Click,
                        actions: vec![toggle],
                    },
                )),
            ],
        );
        kit.finish().expect("package")
    }

    /// A host whose skin runs `pkg`.
    fn running(pkg: &Arc<ui2_package::Package>) -> V2Host {
        let full = ui2_host::hash_hex(&pkg.hash());
        let idx = index(&[entry("kit", &full, true)]);
        let file = format!("ui2/kit-{}.ui2", &full[..16]);
        let (mut h, _) = host(
            FakeHttp::default()
                .ok(INDEX_PATH, idx)
                .ok(&file, pkg.bytes().to_vec()),
            None,
        );
        let env = Env {
            viewport_px: (1280, 720),
            ..Env::default()
        };
        for _ in 0..8 {
            h.poll(env);
        }
        assert_eq!(h.state(), V2State::Running("kit".into()));
        h
    }

    fn frame(h: &mut V2Host) -> V2FrameReport {
        let r = h.ui_mut().expect("running").frame(V2Frame {
            replica: None,
            steps: &[],
            net: None,
            events: &[],
            mods: None,
            now_ms: 16.0,
        });
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        h.cursor = r.cursor;
        r
    }

    fn press(phase: platform::PointerPhase, x: f64, y: f64) -> InputEvent {
        use platform::{
            Modifiers, MouseButton, PhysicalPosition, PointerEvent, PointerId, PointerKind,
        };
        InputEvent::Pointer(PointerEvent {
            id: PointerId::MOUSE,
            kind: PointerKind::Mouse,
            phase,
            position: PhysicalPosition { x, y },
            button: (phase != platform::PointerPhase::Move).then_some(MouseButton::Left),
            modifiers: Modifiers::default(),
        })
    }

    #[test]
    fn without_a_running_skin_input_takes_the_classic_path() {
        let (mut h, _) = host(FakeHttp::default(), None);
        let ev = press(platform::PointerPhase::Down, 50.0, 30.0);
        assert_eq!(h.input(&ev, 1.0), None, "still loading");
        settle(&mut h);
        assert_eq!(h.state(), V2State::Classic);
        assert_eq!(h.input(&ev, 2.0), None, "the classic UI");
        assert_eq!(h.cursor_row(), None);
        // A click the world took is dropped: no skin to tell.
        h.map_click((3, 4), MapButton::Left);
    }

    #[test]
    fn a_running_skin_routes_panel_clicks_to_itself_and_the_rest_to_the_world() {
        use platform::PointerPhase::{Down, Move, Up};
        let mut h = running(&button_package());
        frame(&mut h);
        // On the button: the skin's; its intent runs in the next frame.
        for (phase, t) in [(Move, 1.0), (Down, 2.0), (Up, 3.0)] {
            assert_eq!(
                h.input(&press(phase, 50.0, 30.0), t),
                Some(InputOutcome::Consumed)
            );
        }
        let r = frame(&mut h);
        assert_eq!(r.intents, 1);
        assert!(
            h.ui_mut()
                .expect("running")
                .actions()
                .ui()
                .windows
                .menu
                .open
        );
        assert_eq!(h.cursor_row(), Some(20), "the pointer over the button");
        // Off the panels: the world's (the camera and the picker); the picked plot comes back as `map.click`.
        for (phase, t) in [(Move, 4.0), (Down, 5.0), (Up, 6.0)] {
            assert_eq!(
                h.input(&press(phase, 640.0, 400.0), t),
                Some(InputOutcome::PassToWorld)
            );
        }
        h.map_click((3, 4), MapButton::Left);
        let r = frame(&mut h);
        assert_eq!(r.intents, 1, "the map click");
        assert_eq!(h.cursor_row(), None, "the world's cursor");
    }

    /// A package of one empty HUD layer with a script, with the id `kit`.
    fn scripted_package() -> Arc<ui2_package::Package> {
        use ui2_package::{Node, NodeKind, PackageBuilder, StyleSet, Template};
        let mut b = PackageBuilder::new();
        let record = b.style_record(StyleRecord::initial());
        let style = b.style_set(StyleSet::plain(vec![record]));
        let name = b.str("doc");
        b.template(Template {
            name,
            nodes: vec![Node {
                kind: NodeKind::Layer(Layer::Hud),
                subtree: 1,
                style,
                part: None,
                flags: 0,
            }],
        });
        b.script("js/hud.js", b"x();");
        let bytes = b.finish(Kit::manifest()).expect("package");
        Arc::new(ui2_package::Package::read(Arc::from(bytes)).expect("readable"))
    }

    #[test]
    fn scripts_that_fail_to_load_fall_back_like_any_load_failure() {
        let workers = ui2_script::testing::MockScriptHost::new();
        let w = workers.clone();
        set_script_host(Rc::new(move || Box::new(w.clone()) as Box<dyn ScriptHost>));
        let mut h = running(&scripted_package());
        let worker = workers.live()[0];
        workers.reply(
            worker,
            r#"{"t":"fail","ctx":"skin","error":{"message":"x is not defined"}}"#,
        );
        let r = frame(&mut h);
        assert!(r.fatal.is_some(), "{r:?}");
        assert!(!h.after_frame(&r));
        assert_eq!(h.state(), V2State::Classic, "the only skin is broken");
        assert!(workers.live().is_empty(), "its worker is gone");
        assert!(h.classic_sink().is_none());
    }

    #[test]
    fn without_a_script_host_a_skin_with_scripts_runs_on_markup() {
        // No `set_script_host` on this thread: the null host refuses workers.
        let mut h = running(&scripted_package());
        let r = frame(&mut h);
        assert_eq!((&r.scripts, r.fatal.is_some()), (&None, false));
        assert!(h.after_frame(&r));
    }

    #[test]
    fn skin_cursors_map_to_game_cursor_rows_and_the_minimap_to_shares() {
        assert_eq!(cursor_row(Cursor::Auto), None);
        assert_eq!(cursor_row(Cursor::Default), Some(0));
        assert_eq!(cursor_row(Cursor::Pointer), Some(20));
        assert_eq!(cursor_row(Cursor::Text), Some(22));
        assert_eq!(cursor_row(Cursor::Grabbing), Some(21));
        assert_eq!(minimap_share(0.0), 0);
        assert_eq!(minimap_share(0.5), 32_768);
        assert_eq!(minimap_share(1.0), u16::MAX);
        assert_eq!(minimap_share(-1.0), 0);
        assert_eq!(minimap_share(2.0), u16::MAX);
    }
}
