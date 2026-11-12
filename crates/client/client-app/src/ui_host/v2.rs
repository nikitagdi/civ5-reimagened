//! UI v2 as a frontend of the UI host (`?ui=v2`, `?ui=v2:<skin>`).
//!
//! While it loads, the classic UI stays on the screen: the host asks [`V2Frontend`] to prepare every frame and
//! switches when the skin is up and a game is being played. From then on UI v2 is the only game UI that gets frames
//! and input; the classic one sleeps. UI v2 reads the replica and the localizer of the game session.
//!
//! The network: the host hands each of the core's events to [`V2Frontend`] (kept, as the actions module's events,
//! until its next frame), and its frame runs inside one borrow of the core ([`NetSink`]). A camera request of the
//! frame moves the camera.
//!
//! Input: every window event the app's chain leaves for the game UI goes to the skin first. What the skin takes
//! stops there; what it passes to the world goes to the mods' unclaimed input, the camera and the picker, and a
//! click the picker turns into a plot becomes `map.click` for the actions module; a press on the skin's minimap
//! moves the camera there. The cursor the last frame reported is shown with the game's cursors ([`cursor_row`]).
//!
//! Loading: the index `ui2/skins.json` and the chosen package are fetched relative to the page. Any failure (no
//! index, a bad index, a package that is missing, damaged, of another hash, or that does not build, or scripts that
//! fail while loading) is a warning and a fallback: the default skin if another one failed, else the classic UI.
//! Never a panic, never a retry loop.
//!
//! Scripts: the platform's script workers come with the frontend (`Services::ui2`, the web page's workers);
//! without them a skin's scripts do not run (`NullScriptHost`) and the skin works on its markup.
use std::cell::RefCell;
use std::rc::Rc;

use super::{FrameCx, Frontend, InputCx, PrepareCx, Readiness, UiKind, UiSoundSource};
use std::sync::Arc;

use client_core::ClientEvent;
use draw_list::UiDrawList;
use platform::ui2::{NullTextInput, TextInputBridge};
use platform::{
    HttpClient, HttpError, HttpOp, HttpResponse, InputEvent, NullScriptHost, ScriptHost,
};
use protocol::GameId;
use render::Renderer;
use ui2_host::{
    ActionEvent, Cursor, Env, Fallback, HostClock, InputOutcome, MapButton, NetSink, ReplicaView,
    Services, SkinRegistry, V2Frame, V2FrameReport, V2Init, V2Ui,
};

use crate::net::NetHandle;
use crate::session::GameSession;

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

/// Makes the platform's text input for the skins' text fields; called once, when the first skin comes up.
pub type TextInputFactory = Box<dyn FnOnce() -> Box<dyn TextInputBridge>>;

/// What UI v2 needs of the platform (`Services::ui2`).
pub struct Ui2Platform {
    /// The script workers of each skin that loads; `None`: skins run their markup only.
    pub scripts: Option<ScriptHostFactory>,
    /// Reference images (`?ui2bg=<rrggbb>`): a solid background under the skin and its scene frozen at time 0.
    pub background: Option<[u8; 3]>,
    /// The text input of the skins' text fields (web: a hidden textarea); `None`: fields take no typing.
    pub text_input: Option<TextInputFactory>,
    /// `?ui2classic=cold`: the classic UI comes back from its sleep under UI v2 with a new layer (the cold way)
    /// instead of keeping its layer (the warm way, the default).
    pub classic_cold: bool,
    /// `?ui2onmap=0`: the skin's map layer (city banners, unit flags) is off, for the cost of the layer in a bench:
    /// its anchors are never placed, so nothing of them is drawn or hit.
    pub map_off: bool,
}

/// `?ui2bg=<rrggbb>` (an optional `#`): the colour, or `None` for anything else.
pub fn parse_background(value: &str) -> Option<[u8; 3]> {
    let hex = value.trim().trim_start_matches('#');
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// Where a map anchor's object is in the world: a city's banner or a unit's flag as the renderer hangs them, or
/// the centre of a plot on the relief. `None`: not on the map (not seen, gone). A unit the renderer hangs no flag
/// for (the units' own pipeline draws them without labels) hangs over its cell (`unit_cell`: the cell of the unit
/// of an owner and an id, as the game shows it) at the height of the renderer's flags.
fn anchor_point(
    kind: ui2_host::AnchorKind,
    key: &ui2_host::ModelValue,
    labels: &[render::entities::WorldLabel],
    terrain: Option<&render::terrain::TerrainSnapshot>,
    unit_cell: &dyn Fn(i8, i32) -> Option<(i32, i32)>,
) -> Option<render::glam::Vec3> {
    use render::entities::LabelData;
    use ui2_host::{AnchorKind, ModelValue};
    // The id of a city or a unit is the low 32 bits of its key.
    let id = || match key {
        ModelValue::Key(k) => Some(k.0 as u32 as i32),
        ModelValue::I32(v) => Some(*v),
        ModelValue::U32(v) => Some(*v as i32),
        _ => None,
    };
    match kind {
        AnchorKind::Plot => {
            let ModelValue::Struct(xy) = key else {
                return None;
            };
            let (Some(ModelValue::I32(x)), Some(ModelValue::I32(y))) = (xy.first(), xy.get(1))
            else {
                return None;
            };
            let c = render::hexgrid::cell_center(*x, *y);
            let h = terrain.map_or(0.0, |t| render::terrain::terrain_height_at(t, c));
            Some(render::glam::Vec3::new(c.x, h, c.y))
        }
        AnchorKind::City => {
            let id = id()?;
            labels.iter().find_map(|l| match &l.data {
                LabelData::CityBanner(b) if b.city == id => Some(l.position),
                _ => None,
            })
        }
        AnchorKind::Unit => {
            let id = id()?;
            labels
                .iter()
                .find_map(|l| match &l.data {
                    LabelData::UnitFlag(f) if f.unit == id => Some(l.position),
                    _ => None,
                })
                .or_else(|| {
                    // The owner is the high half of the key.
                    let ModelValue::Key(k) = key else {
                        return None;
                    };
                    let owner = i8::try_from(k.0 >> 32).ok()?;
                    let (x, y) = unit_cell(owner, id)?;
                    let c = render::hexgrid::cell_center(x, y);
                    let h = terrain.map_or(0.0, |t| render::terrain::terrain_height_at(t, c));
                    Some(render::glam::Vec3::new(
                        c.x,
                        h + render::entities::labels::LABEL_HEIGHT,
                        c.y,
                    ))
                })
        }
    }
}

/// What the playtest hooks (`?test=1`) read of UI v2 after each of its frames: the loading stage and, while a
/// skin runs, the text leaves the document shows. Filled only once [`enable_probe`] was called.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct V2Probe {
    /// "loading", "running" or "classic" (UI v2 is off for the session).
    pub state: &'static str,
    /// The running skin.
    pub skin: Option<String>,
    /// From the start of the loading to the skin's first frame, ms.
    pub load_ms: Option<f64>,
    /// UI v2 frames run with the game's layer up.
    pub frames: u64,
    /// The text leaves in document order: the `data-part` name of the nearest element that has one, and the
    /// string.
    pub texts: Vec<(Option<String>, String)>,
    /// UI v2 is the game UI on the screen (it is not asleep under the classic one).
    pub active: bool,
    /// The running package's hash (hex).
    pub pkg: Option<String>,
    /// The model store's revision after the last frame.
    pub model_rev: u64,
    /// The document's panels: the skin's parts and the mods' fragments.
    pub panels: Vec<ProbePanel>,
    /// The script contexts of the skin.
    pub scripts: Vec<ProbeScript>,
    /// The answer to the last [`ask_rects`]: its number and the rectangles.
    pub rects: Option<(u64, Vec<ProbeRect>)>,
}

/// One panel as the probe shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbePanel {
    pub name: String,
    /// The mod that mounted it; `None` for the skin's own.
    pub owner: Option<String>,
    /// Why it shows a placeholder; `None`: it works.
    pub failed: Option<String>,
}

/// One script context as the probe shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeScript {
    /// `skin` or `mod:<id>`.
    pub ctx: String,
    /// `starting`, `running`, `busy`, `restarting` or `disabled`.
    pub status: &'static str,
    pub errors: u64,
    pub restarts: u64,
}

/// One rectangle of a `ui2Rects` dump (`Runtime::dump_rects`): the part it was asked for, the node and its keys,
/// the box in CSS px.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeRect {
    pub part: String,
    pub node: String,
    pub keys: String,
    pub rect: [f32; 4],
}

thread_local! {
    static PROBE: RefCell<Option<V2Probe>> = const { RefCell::new(None) };
    /// The parts a `ui2Rects` call asked for, with its number; answered by the next UI v2 frame.
    static RECTS_ASKED: RefCell<Option<(u64, Vec<String>)>> = const { RefCell::new(None) };
    static RECTS_SEQ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Asks the next UI v2 frame for the rectangles of the elements of `parts` (`data-part` names); the answer comes in
/// [`V2Probe::rects`] under the returned number.
pub fn ask_rects(parts: Vec<String>) -> u64 {
    let n = RECTS_SEQ.with(|s| {
        s.set(s.get() + 1);
        s.get()
    });
    RECTS_ASKED.with(|r| *r.borrow_mut() = Some((n, parts)));
    n
}

/// The rectangles `ask_rects` asked for, from the running skin.
fn answer_rects(ui: &V2Ui) -> Option<(u64, Vec<ProbeRect>)> {
    let (n, parts) = RECTS_ASKED.with(|r| r.borrow_mut().take())?;
    let mut out = Vec::new();
    for part in parts {
        for r in ui.runtime().dump_rects(Some(&part)) {
            out.push(ProbeRect {
                part: part.clone(),
                node: r.node,
                keys: r.keys,
                rect: [r.x, r.y, r.w, r.h],
            });
        }
    }
    Some((n, out))
}

/// The probe's panels and scripts of `ui`.
fn probe_parts(ui: &V2Ui) -> (Vec<ProbePanel>, Vec<ProbeScript>) {
    let panels = ui
        .runtime()
        .panels()
        .into_iter()
        .map(|p| ProbePanel {
            name: p.name,
            owner: p.owner.map(|o| o.to_string()),
            failed: p.failed,
        })
        .collect();
    let scripts = ui
        .script_status()
        .map(|status| {
            let stats = ui.script_stats().unwrap_or_default();
            ProbeScript {
                ctx: "skin".into(),
                status: match status {
                    ui2_host::ScriptStatus::Starting => "starting",
                    ui2_host::ScriptStatus::Running => "running",
                    ui2_host::ScriptStatus::Busy { .. } => "busy",
                    ui2_host::ScriptStatus::Restarting => "restarting",
                    ui2_host::ScriptStatus::Disabled { .. } => "disabled",
                },
                errors: stats.errors,
                restarts: stats.restarts,
            }
        })
        .into_iter()
        .collect();
    (panels, scripts)
}

/// Starts filling the probe (the test hooks call it once, at install).
pub fn enable_probe() {
    PROBE.with(|p| {
        p.borrow_mut().get_or_insert_with(|| V2Probe {
            state: "loading",
            ..V2Probe::default()
        });
    });
}

/// The probe as of the last UI v2 frame; `None` until [`enable_probe`] was called.
pub fn probe() -> Option<V2Probe> {
    PROBE.with(|p| p.borrow().clone())
}

fn probe_on() -> bool {
    PROBE.with(|p| p.borrow().is_some())
}

/// The text leaves of `ui`'s document with the part each one is in.
fn probe_texts(ui: &V2Ui) -> Vec<(Option<String>, String)> {
    let pkg = ui.package();
    let arena = ui.runtime().arena();
    let part_of = |mut id| loop {
        let node = arena.get(id)?;
        let tmpl = node.tmpl();
        let part = pkg
            .templates()
            .get(tmpl.0.index())
            .and_then(|t| t.nodes.get(tmpl.1 as usize))
            .and_then(|n| n.part)
            .and_then(|s| pkg.str(s));
        if let Some(name) = part {
            return Some(name.to_owned());
        }
        id = node.parent()?;
    };
    arena
        .preorder()
        .into_iter()
        .filter_map(|id| {
            let n = arena.get(id)?;
            // Only text leaves carry a string.
            let text = n.text()?;
            Some((part_of(id), text.to_owned()))
        })
        .collect()
}

/// Where the loading is.
enum Stage {
    /// Waiting for the index.
    Index(Box<dyn HttpOp>),
    /// Nothing asked yet: UI v2 has not been asked for (the index is fetched when it is, or for the settings).
    Idle,
    /// The index is read and no skin was asked for yet (the settings listed the skins).
    Listed,
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
    Idle,
    Listed,
    FetchingIndex,
    FetchingSkin(String),
    Running(String),
    Classic,
}

/// UI v2 over the classic layer: the loading, the fallback, the frame.
pub struct V2Frontend {
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
    /// The network's events for the actions module since the last UI v2 frame, and the game they belong to.
    events: Vec<ActionEvent>,
    game: Option<GameId>,
    /// The game's mod set, when it changed since the last UI v2 frame.
    mods: Option<Vec<Arc<str>>>,
    /// The cursor the last UI v2 frame reported.
    cursor: Cursor,
    /// The script workers of each skin that loads; `None`: scripts do not run.
    scripts: Option<ScriptHostFactory>,
    /// A game is up (its replica live while it is played); UI v2 runs no frame before.
    game_up: bool,
    /// `?ui2bg=`: reference images (see [`Ui2Platform::background`]).
    background: Option<[u8; 3]>,
    /// `?ui2onmap=0` (see [`Ui2Platform::map_off`]).
    map_off: bool,
    /// The platform's text input, made when the first skin comes up; the running skin holds it meanwhile.
    text_input: Option<TextInputFactory>,
    text_bridge: Option<Box<dyn TextInputBridge>>,
    /// The source's warm-up (`V2Ui::warm_up`) is due: a game was joined, loaded or left since the last one.
    warm_due: bool,
    /// The running skin failed (for the host, once): why.
    lost: Option<String>,
    /// Why UI v2 is off, once no skin can be used.
    off_reason: Option<String>,
    /// UI v2 has the screen (between `activate` and `deactivate`).
    on_screen: bool,
}

impl V2Frontend {
    /// UI v2 that fetches nothing until it is asked for ([`Frontend::prepare`]) or its skins are listed
    /// ([`Frontend::options`]): a page that never shows it asks for no index.
    /// `scripts`: the platform's script workers for each skin that loads (`None`: skins run their markup only).
    pub fn new(
        choice: V2Choice,
        http: Rc<dyn HttpClient>,
        clock: HostClock,
        scripts: Option<ScriptHostFactory>,
    ) -> V2Frontend {
        let started_ms = clock();
        V2Frontend {
            http,
            choice,
            registry: SkinRegistry::default(),
            stage: Stage::Idle,
            clock,
            started_ms,
            load_ms: None,
            loc: None,
            events: Vec::new(),
            game: None,
            mods: None,
            cursor: Cursor::Auto,
            scripts,
            game_up: false,
            background: None,
            map_off: false,
            text_input: None,
            text_bridge: None,
            warm_due: true,
            lost: None,
            off_reason: None,
            on_screen: false,
        }
    }

    /// Reference images: every skin that loads gets a solid background and a frozen scene.
    pub fn set_background(&mut self, background: Option<[u8; 3]>) {
        self.background = background;
    }

    /// `?ui2onmap=0`: the map layer of every skin is off (see [`Ui2Platform::map_off`]).
    pub fn set_map_off(&mut self, off: bool) {
        self.map_off = off;
    }

    /// The platform's text input for the skins' text fields (made when the first skin comes up).
    pub fn set_text_input(&mut self, make: Option<TextInputFactory>) {
        self.text_input = make;
    }

    /// The text input for a skin that comes up: the one the host has, else a new one of the platform, else none.
    fn text_bridge(&mut self) -> Box<dyn TextInputBridge> {
        if let Some(b) = self.text_bridge.take() {
            return b;
        }
        match self.text_input.take() {
            Some(make) => make(),
            None => Box::new(NullTextInput),
        }
    }

    /// The game was left: the next frame waits for the next game and rebuilds the model from it.
    pub fn new_game(&mut self) {
        self.game_up = false;
        self.warm_due = true;
        if let Some(ui) = self.ui_mut() {
            ui.reset_source();
        }
    }

    /// One window event, before the world sees it: `None` while no skin runs (the classic UI's path stays as it
    /// is), else who takes it. `now_ms` is the app's clock.
    pub fn route_input(&mut self, ev: &InputEvent, now_ms: f64) -> Option<InputOutcome> {
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
        if let ClientEvent::ModSet(set) = ev {
            self.mods = Some(mod_ids(set));
        }
        // The game is followed always; the events are kept only for a skin that is loading or loaded.
        let a = action_event(ev, &mut self.game);
        if matches!(self.stage, Stage::Classic | Stage::Idle | Stage::Listed) {
            self.events.clear();
            return;
        }
        if let Some(a) = a {
            if a == ActionEvent::NewGame {
                self.warm_due = true;
            }
            if self.events.len() >= MAX_EVENTS {
                // UI v2 sleeps under the classic UI for a long time: no line per event.
                tracing::debug!(
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
            Stage::Idle => V2State::Idle,
            Stage::Listed => V2State::Listed,
            Stage::Index(_) => V2State::FetchingIndex,
            Stage::Package { id, .. } => V2State::FetchingSkin(id.clone()),
            Stage::Running(ui) => V2State::Running(ui.skin_id().to_owned()),
            Stage::Classic => V2State::Classic,
        }
    }

    /// A skin runs.
    pub fn running(&self) -> bool {
        matches!(self.stage, Stage::Running(_))
    }

    /// The running UI v2.
    pub fn ui_mut(&mut self) -> Option<&mut V2Ui> {
        match &mut self.stage {
            Stage::Running(ui) => Some(&mut **ui),
            _ => None,
        }
    }

    /// Advances the loading of the chosen skin with what the network answered; `env` is the window the skin is
    /// built for.
    pub fn poll(&mut self, env: Env) {
        self.advance(env, true);
    }

    /// Fetches and reads the index only (the settings list the skins), without asking for a skin.
    fn poll_index(&mut self) {
        self.advance(Env::default(), false);
    }

    /// `want`: a skin is wanted (the host asks for UI v2), else the index alone.
    fn advance(&mut self, env: Env, want: bool) {
        // At most one step per kind per call: a fallback starts a new request, answered on a later frame.
        let stage = std::mem::replace(&mut self.stage, Stage::Classic);
        self.stage = match stage {
            Stage::Idle => {
                self.started_ms = (self.clock)();
                tracing::info!(url = INDEX_PATH, "UI v2: fetching the skin index");
                Stage::Index(self.http.get(INDEX_PATH))
            }
            Stage::Index(mut op) => match op.poll() {
                None => Stage::Index(op),
                Some(answer) => match self.on_index(answer) {
                    Some(stage) => stage,
                    None if want => self.start_chosen(),
                    None => Stage::Listed,
                },
            },
            Stage::Listed if want => {
                self.started_ms = (self.clock)();
                self.start_chosen()
            }
            Stage::Package { id, mut op } => match op.poll() {
                None => Stage::Package { id, op },
                Some(answer) => self.on_package(id, answer, env),
            },
            other => other,
        };
    }

    /// Reads the index; `Some`: it cannot be used (UI v2 is off).
    fn on_index(&mut self, answer: Result<HttpResponse, HttpError>) -> Option<Stage> {
        let body = match body_of(INDEX_PATH, answer) {
            Ok(b) => b,
            Err(why) => return Some(self.off(&why)),
        };
        let text = match std::str::from_utf8(&body) {
            Ok(t) => t,
            Err(_) => return Some(self.off(&format!("{INDEX_PATH} is not UTF-8"))),
        };
        match SkinRegistry::parse(text) {
            Ok(r) => self.registry = r,
            Err(e) => return Some(self.off(&e.to_string())),
        }
        None
    }

    /// Starts the chosen skin of the index (an unknown or broken one: the default; none: UI v2 is off).
    fn start_chosen(&mut self) -> Stage {
        let want = self.choice.skin.as_deref();
        let Some(entry) = self.registry.resolve(want) else {
            return self.off("the skin index has no skin to start");
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
            return self.off(&format!("skin {id} is not in the index"));
        };
        // A skin for another script API is refused before it is downloaded.
        if let Err(e) = entry.check_api() {
            let why = e.failure.to_string();
            return self.fall_back(&id, &why);
        }
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
                let mut ui = ui;
                ui.set_golden(self.background);
                let bridge = self.text_bridge();
                ui.set_text_bridge(bridge);
                // A new skin over a game already up reads its slow first calls once too.
                self.warm_due = true;
                Stage::Running(Box::new(ui))
            }
            Err(why) => self.fall_back(&id, &why),
        }
    }

    /// The host asks for UI v2 with `skin` (`None`: the index's default). A skin that is loading or running and is
    /// not the one asked for is replaced while UI v2 is not on the screen (a skin change on the screen, keeping the
    /// open windows, is not done here); after a failure, a skin of the index that did not fail is tried (the
    /// player asked: no loop), and an index that could not be read is asked for again.
    pub fn choose(&mut self, skin: Option<String>) {
        self.choice.skin = skin;
        let fits = |id: &str| self.choice.skin.as_deref().is_none_or(|w| w == id);
        let restart = match &self.stage {
            Stage::Running(ui) if !fits(ui.skin_id()) => {
                if self.on_screen {
                    tracing::warn!(
                        "UI v2: skin {} is on the screen; changing it there is not supported yet",
                        ui.skin_id()
                    );
                    false
                } else {
                    true
                }
            }
            Stage::Package { id, .. } => !fits(id),
            Stage::Classic => true,
            _ => false,
        };
        if !restart {
            return;
        }
        self.drop_skin();
        self.started_ms = (self.clock)();
        self.stage = if self.registry.entries().is_empty() {
            Stage::Idle
        } else if self.registry.resolve(self.choice.skin.as_deref()).is_some() {
            self.off_reason = None;
            self.start_chosen()
        } else {
            // Every skin failed in this session: UI v2 stays off and says why.
            Stage::Classic
        };
    }

    /// The running skin is dropped: the text input goes back to the host for the next one.
    fn drop_skin(&mut self) {
        if let Stage::Running(ui) = &mut self.stage {
            ui.release_text_input();
            if let Some(b) = ui.take_text_bridge() {
                self.text_bridge = Some(b);
            }
        }
    }

    /// A skin failed: it is not tried again; the default skin if it was another one, else the classic UI.
    fn fall_back(&mut self, id: &str, why: &str) -> Stage {
        match self.registry.fail(id) {
            Fallback::Skin(next) => {
                tracing::warn!("UI v2: skin {id} cannot be used ({why}); trying skin {next}");
                self.fetch(next)
            }
            Fallback::Classic => self.off(&format!("skin {id} cannot be used ({why})")),
        }
    }

    /// No skin can be used in this session: UI v2 is off and says why.
    fn off(&mut self, why: &str) -> Stage {
        tracing::warn!("UI v2 is off, the classic UI stays: {why}");
        self.off_reason = Some(why.to_owned());
        Stage::Classic
    }

    /// One frame after the classic layer's: advances the loading and, while a skin runs and the game's layer is up,
    /// runs UI v2 on the replica and the network (`net`: the app's handle; `None` without a network) and publishes
    /// its counters. `Some`: the scene of the returned UI goes to the renderer (`prepare_ui2`) before it draws.
    ///
    /// The network handle is borrowed for the UI v2 frame only, in the app's frame: never inside a call into page
    /// script (the frame, the timer and the page events never run inside one another).
    /// `look`: where a camera request of the frame sends the camera, set when there is one.
    pub(crate) fn run(
        &mut self,
        session: &GameSession,
        steps: &[replica::ReplicaStep],
        net: Option<&NetHandle>,
        env: Env,
        now_ms: f64,
        look: &mut Option<(i32, i32)>,
    ) -> Option<&mut V2Ui> {
        self.poll(env);
        self.probe_stage();
        // The game is up once the replica is live in a game being played; it stays up until the game is left.
        self.note_game(Some(session), playing(net));
        if !self.running() || !self.game_up {
            return None;
        }
        // The replica and the localizer are the game session's.
        let loc = session.localizer();
        let new_loc = match (&loc, &self.loc) {
            (Some(a), Some(b)) => !Rc::ptr_eq(a, b),
            (a, b) => a.is_some() != b.is_some(),
        };
        let replica = session.replica();
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
        let steps: &[replica::ReplicaStep] = if held.is_some() { steps } else { &[] };
        let events = std::mem::take(&mut self.events);
        let mods = self.mods.take();
        let (report, look_at) = match net {
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
        // The core's slow first reads, once per game (joined, loaded or a new skin), after the frame and outside its
        // budget.
        if self.warm_due
            && let Some(r) = held.as_deref_mut()
        {
            let view = ReplicaView::of(r);
            if view.state == replica::State::Live
                && !view.catching_up
                && let Some(read) = view.read
            {
                let w = ui.warm_up(read);
                self.warm_due = false;
                render::perf::set_counter("ui2.source.warm_reads", f64::from(w.getter_calls));
                render::perf::set_counter("ui2.source.warm_units", f64::from(w.spent_units));
                tracing::info!(
                    reads = w.getter_calls,
                    units = w.spent_units,
                    "UI v2: the source's first reads are warm"
                );
            }
        }
        // The camera eases to a plot the frame asked for, as the classic UI's `UI.LookAt` does.
        if look_at.is_some() {
            *look = look_at;
        }
        self.cursor = report.cursor;
        publish(&report, load_ms);
        if probe_on() {
            let texts = probe_texts(ui);
            let (panels, scripts) = probe_parts(ui);
            let rects = answer_rects(ui);
            let pkg = ui2_host::hash_hex(&ui.package().hash());
            PROBE.with(|p| {
                if let Some(p) = p.borrow_mut().as_mut() {
                    p.state = "running";
                    p.active = true;
                    p.skin = Some(ui.skin_id().to_owned());
                    p.pkg = Some(pkg);
                    p.load_ms = p.load_ms.or(load_ms);
                    p.frames += 1;
                    p.texts = texts;
                    p.model_rev = report.model_revision;
                    p.panels = panels;
                    p.scripts = scripts;
                    if rects.is_some() {
                        p.rects = rects;
                    }
                }
            });
        }
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
            self.lost = Some(format!("skin {} ({failure})", e.skin));
            self.drop_skin();
            self.stage = self.fall_back(&e.skin, &failure);
        }
        self.running()
    }
}

impl V2Frontend {
    /// The probe's loading stage while no skin runs.
    fn probe_stage(&self) {
        if probe_on() && !self.running() {
            let state = match self.stage {
                Stage::Classic => "classic",
                _ => "loading",
            };
            PROBE.with(|p| {
                if let Some(p) = p.borrow_mut().as_mut() {
                    p.state = state;
                }
            });
        }
    }

    /// The game is up once the replica is live in a game being played.
    fn note_game(&mut self, session: Option<&GameSession>, playing: bool) {
        if session.is_some_and(GameSession::live) && playing {
            self.game_up = true;
        }
    }
}

/// The client core is in a game being played.
pub(crate) fn playing(net: Option<&NetHandle>) -> bool {
    net.is_some_and(|n| {
        matches!(
            n.with_client(|c| c.phase()),
            client_core::ClientPhase::Playing
        )
    })
}

fn env_of(size: (u32, u32), dpr: f32) -> Env {
    Env {
        viewport_px: size,
        dpr,
        ..Env::default()
    }
}

impl Frontend for V2Frontend {
    fn kind(&self) -> UiKind {
        UiKind::V2
    }

    fn needs_replica(&self) -> bool {
        true
    }

    /// Ready once the skin is up and a game is being played (the classic UI stays on the screen until then);
    /// failed when no skin can be used.
    fn prepare(&mut self, cx: &mut PrepareCx<'_>) -> Readiness {
        self.poll(env_of(cx.size, cx.dpr));
        self.probe_stage();
        self.note_game(cx.session, cx.playing);
        match &self.stage {
            Stage::Classic => Readiness::Failed(
                self.off_reason
                    .clone()
                    .unwrap_or_else(|| "no UI v2 skin can be used".into()),
            ),
            Stage::Running(_) if self.game_up => Readiness::Ready,
            _ => Readiness::Loading { progress: None },
        }
    }

    fn activate(&mut self, _cx: &mut PrepareCx<'_>) {
        tracing::info!("UI v2 takes the screen; the classic UI sleeps");
        self.on_screen = true;
    }

    fn deactivate(&mut self, _cx: &mut PrepareCx<'_>) {
        self.on_screen = false;
        self.cursor = Cursor::Auto;
        // A focused text field of the skin lets the platform's input go while UI v2 sleeps.
        if let Some(ui) = self.ui_mut() {
            ui.release_text_input();
        }
        PROBE.with(|p| {
            if let Some(p) = p.borrow_mut().as_mut() {
                p.active = false;
            }
        });
    }

    /// The skin's frame; its scene goes to the renderer (`prepare_ui2`), the app's draw list gets nothing.
    fn frame(&mut self, cx: &mut FrameCx<'_>, _out: &mut UiDrawList) {
        let env = env_of(cx.size, cx.dpr);
        let mut look = None;
        let map_off = self.map_off;
        if let Some(ui) = self.run(
            cx.session,
            &cx.step.steps,
            cx.net,
            env,
            cx.now_ms,
            &mut look,
        ) {
            // The map anchors follow their objects through the camera, as the classic UI's banners do.
            if ui.anchor_count() > 0 && map_off {
                ui.place_anchors(|_, _| None);
            } else if ui.anchor_count() > 0 {
                let renderer: &Renderer = cx.renderer;
                let (w, h) = cx.size;
                let view_proj = cx.camera.view_proj(w as f32 / h.max(1) as f32);
                let eye = cx.camera.eye();
                let viewport = render::glam::Vec2::new(w as f32, h as f32);
                let terrain = renderer.terrain_snapshot();
                let wrap = terrain
                    .filter(|t| t.wrap_x())
                    .map(|t| render::hexgrid::HEX_WIDTH * t.width() as f32);
                let labels = renderer.entity_labels();
                // The cells of the units the game shows, for the flags the renderer hangs no label for.
                let units: std::collections::HashMap<(i8, i32), (i32, i32)> = cx
                    .net
                    .and_then(|n| {
                        n.with(|core| {
                            core.shown().map(|w| {
                                w.units
                                    .iter()
                                    .map(|u| ((u.owner, u.id), (i32::from(u.x), i32::from(u.y))))
                                    .collect()
                            })
                        })
                    })
                    .unwrap_or_default();
                let unit_cell = |owner: i8, id: i32| units.get(&(owner, id)).copied();
                ui.place_anchors(|kind, key| {
                    let world = anchor_point(kind, key, labels, terrain, &unit_cell)?;
                    let s = render::entities::project_wrapped_label(
                        world, wrap, &view_proj, viewport, eye, terrain,
                    );
                    s.visible.then_some((s.pos, 1.0))
                });
            }
            cx.renderer.prepare_ui2(ui.scene_mut());
        }
        if let Some((x, y)) = look {
            cx.camera.look_at(render::hexgrid::cell_center(x, y));
        }
    }

    fn input(&mut self, cx: &mut InputCx<'_>, ev: InputEvent) {
        let Some(outcome) = self.route_input(&ev, cx.now_ms) else {
            return;
        };
        match outcome {
            InputOutcome::Consumed => {}
            // A press on the skin's minimap moves the camera like the classic minimap panel.
            InputOutcome::MinimapClick { u, v } => {
                if let Some(at) = cx
                    .renderer
                    .and_then(|r| r.minimap_to_world(minimap_share(u), minimap_share(v)))
                {
                    cx.camera.center_on(at);
                }
            }
            // The rest goes to the mods' unclaimed input, then the camera; a click the picker turns into a plot is
            // the skin's `map.click`.
            InputOutcome::PassToWorld => {
                if !cx.hooks.is_empty() {
                    let claim = cx.hooks.unclaimed_input(&ev);
                    if claim.pointer_lock.is_some() {
                        cx.pointer_lock = claim.pointer_lock;
                    }
                    if claim.taken {
                        return;
                    }
                }
                cx.map_controller.handle(&ev, cx.camera);
                let clicks = [
                    (cx.map_controller.take_click(), MapButton::Left),
                    (cx.map_controller.take_right_click(), MapButton::Right),
                ];
                for (at, button) in clicks {
                    let Some(at) = at else { continue };
                    let ray = cx.map_controller.ray_at(at, cx.camera);
                    if let Some(p) = cx.picker.pick(cx.bridge, &ray) {
                        self.map_click(p.cell, button);
                    }
                }
            }
        }
    }

    fn net_event(&mut self, ev: &ClientEvent) {
        V2Frontend::net_event(self, ev);
    }

    fn new_game(&mut self) {
        V2Frontend::new_game(self);
    }

    fn cursor_row(&self) -> Option<i32> {
        V2Frontend::cursor_row(self)
    }

    fn take_failure(&mut self) -> Option<String> {
        self.lost.take()
    }

    fn recovers(&self) -> bool {
        !matches!(self.stage, Stage::Classic)
    }

    fn skin(&self) -> Option<String> {
        match &self.stage {
            Stage::Running(ui) => Some(ui.skin_id().to_owned()),
            _ => None,
        }
    }

    fn choose(&mut self, choice: &super::UiChoice) {
        if let super::UiChoice::V2 { skin } = choice {
            V2Frontend::choose(self, skin.clone());
        }
    }

    /// UI v2 with the index's default skin, and each skin of the index that did not fail; the index is fetched the
    /// first time the settings ask.
    fn options(&mut self) -> Vec<(String, String)> {
        if matches!(self.stage, Stage::Idle | Stage::Index(_)) {
            self.poll_index();
        }
        let mut out = vec![("v2".to_owned(), "UI v2".to_owned())];
        for e in self.registry.entries() {
            if !self.registry.is_broken(&e.id) {
                out.push((format!("v2:{}", e.id), format!("UI v2: {}", e.id)));
            }
        }
        out
    }

    fn sounds(&self) -> Option<&dyn UiSoundSource> {
        // SIMPLIFIED(SIMP-265): UI v2 plays no interface sounds (button clicks, the original's screen sounds); the
        // world's sounds and the music go on.
        None
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

    fn host(http: FakeHttp, choice: Option<&str>) -> (V2Frontend, Rc<FakeHttp>) {
        host_with(http, choice, None)
    }

    fn host_with(
        http: FakeHttp,
        choice: Option<&str>,
        scripts: Option<ScriptHostFactory>,
    ) -> (V2Frontend, Rc<FakeHttp>) {
        let http = Rc::new(http);
        let shared: Rc<dyn HttpClient> = http.clone();
        let host = V2Frontend::new(
            V2Choice {
                skin: choice.map(str::to_owned),
            },
            shared,
            clock(),
            scripts,
        );
        (host, http)
    }

    /// Polls until the loading settles (each poll answers one request).
    fn settle(h: &mut V2Frontend) -> V2State {
        for _ in 0..8 {
            h.poll(Env::default());
        }
        h.state()
    }

    #[test]
    fn no_index_is_the_classic_ui() {
        let (mut h, http) = host(FakeHttp::default(), Some("engraved"));
        assert_eq!(h.state(), V2State::Idle);
        assert!(
            http.asked.borrow().is_empty(),
            "nothing before it is asked for"
        );
        h.poll(Env::default());
        assert_eq!(h.state(), V2State::FetchingIndex);
        assert_eq!(settle(&mut h), V2State::Classic);
        assert!(!h.running() && h.ui_mut().is_none());
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
        // The probe names no part: the kit's text is in none.
        assert_eq!(probe_texts(ui), [(None, "Hello".to_owned())]);
    }

    #[test]
    fn the_probe_names_the_part_of_each_text() {
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
        let (gold, turn, top) = (kit.str("312"), kit.str("Turn 5"), kit.str("top.gold"));
        kit.template(
            "doc",
            vec![
                TNode::layer(Layer::Hud, cell)
                    .child(
                        TNode::el(cell)
                            .part(top)
                            .child(TNode::el(cell).child(TNode::text(gold, cell))),
                    )
                    .child(TNode::text(turn, cell)),
            ],
        );
        let pkg = kit.finish().expect("package");
        let mut ui = V2Ui::new(pkg, V2Init::default()).expect("ui");
        ui.frame(V2Frame {
            replica: None,
            steps: &[],
            net: None,
            events: &[],
            mods: None,
            now_ms: 16.0,
        });
        assert_eq!(
            probe_texts(&ui),
            [
                (Some("top.gold".to_owned()), "312".to_owned()),
                (None, "Turn 5".to_owned())
            ]
        );
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
    fn running(pkg: &Arc<ui2_package::Package>) -> V2Frontend {
        running_with(pkg, None)
    }

    fn running_with(
        pkg: &Arc<ui2_package::Package>,
        scripts: Option<ScriptHostFactory>,
    ) -> V2Frontend {
        let full = ui2_host::hash_hex(&pkg.hash());
        let idx = index(&[entry("kit", &full, true)]);
        let file = format!("ui2/kit-{}.ui2", &full[..16]);
        let (mut h, _) = host_with(
            FakeHttp::default()
                .ok(INDEX_PATH, idx)
                .ok(&file, pkg.bytes().to_vec()),
            None,
            scripts,
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

    fn frame(h: &mut V2Frontend) -> V2FrameReport {
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
        assert_eq!(h.route_input(&ev, 1.0), None, "still loading");
        settle(&mut h);
        assert_eq!(h.state(), V2State::Classic);
        assert_eq!(h.route_input(&ev, 2.0), None, "the classic UI");
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
                h.route_input(&press(phase, 50.0, 30.0), t),
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
                h.route_input(&press(phase, 640.0, 400.0), t),
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
        let mut h = running_with(
            &scripted_package(),
            Some(Rc::new(move || Box::new(w.clone()) as Box<dyn ScriptHost>)),
        );
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
        // The skin's text input came back to the host for the next skin.
        assert!(h.text_bridge.is_some());
    }

    #[test]
    fn the_text_input_is_made_once_and_handed_to_the_running_skin() {
        let made = Rc::new(Cell::new(0));
        let pkg = kit_package();
        let full = ui2_host::hash_hex(&pkg.hash());
        let idx = index(&[entry("kit", &full, true)]);
        let file = format!("ui2/kit-{}.ui2", &full[..16]);
        let (mut h, _) = host(
            FakeHttp::default()
                .ok(INDEX_PATH, idx)
                .ok(&file, pkg.bytes().to_vec()),
            None,
        );
        let m = Rc::clone(&made);
        h.set_text_input(Some(Box::new(move || {
            m.set(m.get() + 1);
            Box::new(NullTextInput) as Box<dyn TextInputBridge>
        })));
        assert_eq!(settle(&mut h), V2State::Running("kit".into()));
        assert_eq!(made.get(), 1);
        assert!(h.text_bridge.is_none(), "the skin holds it");
        assert!(h.ui_mut().expect("running").take_text_bridge().is_some());
        assert!(
            h.warm_due,
            "a new skin warms the source up once a game is read"
        );
    }

    /// A fixture package of `ui2-host` (`tests/fixtures`) by skin id, and its index entry.
    fn fixture(id: &str) -> (Vec<u8>, serde_json::Value) {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ui2-host/tests/fixtures");
        let idx: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.join("skins.json")).expect("the fixture index"),
        )
        .expect("json");
        let mut entry = idx
            .as_array()
            .and_then(|a| a.iter().find(|e| e["id"] == id))
            .cloned()
            .expect("entry");
        let bytes =
            std::fs::read(dir.join(entry["file"].as_str().expect("file"))).expect("package");
        entry["default"] = serde_json::json!(false);
        (bytes, entry)
    }

    #[test]
    fn the_settings_list_the_skins_without_loading_one_and_a_choice_loads_it() {
        let pkg = kit_package();
        let full = ui2_host::hash_hex(&pkg.hash());
        let file = format!("ui2/kit-{}.ui2", &full[..16]);
        let (mut h, http) = host(
            FakeHttp::default()
                .ok(INDEX_PATH, index(&[entry("kit", &full, true)]))
                .ok(&file, pkg.bytes().to_vec()),
            None,
        );
        let mut opts = Vec::new();
        for _ in 0..3 {
            opts = Frontend::options(&mut h);
        }
        assert_eq!(h.state(), V2State::Listed);
        assert_eq!(
            opts,
            [
                ("v2".to_owned(), "UI v2".to_owned()),
                ("v2:kit".to_owned(), "UI v2: kit".to_owned())
            ]
        );
        assert_eq!(*http.asked.borrow(), [INDEX_PATH], "no package yet");
        Frontend::choose(
            &mut h,
            &super::super::UiChoice::V2 {
                skin: Some("kit".into()),
            },
        );
        assert_eq!(settle(&mut h), V2State::Running("kit".into()));
    }

    #[test]
    fn after_every_skin_failed_a_new_choice_tries_a_skin_that_did_not() {
        let (_, api9) = fixture("api9");
        let pkg = kit_package();
        let full = ui2_host::hash_hex(&pkg.hash());
        let file = format!("ui2/kit-{}.ui2", &full[..16]);
        // The default skin's file is missing: UI v2 goes off.
        let (mut h, http) = host(
            FakeHttp::default().ok(INDEX_PATH, index(&[api9, entry("kit", &full, true)])),
            None,
        );
        assert_eq!(settle(&mut h), V2State::Classic);
        // The player asks for api9: refused by its entry, no download; still off, and it says why.
        V2Frontend::choose(&mut h, Some("api9".into()));
        assert_eq!(settle(&mut h), V2State::Classic);
        let asked = http.asked.borrow().clone();
        assert_eq!(asked, [INDEX_PATH.to_owned(), file.clone()]);
        let mut cx = PrepareCx {
            now_ms: 0.0,
            size: (1280, 720),
            dpr: 1.0,
            session: None,
            playing: false,
        };
        match Frontend::prepare(&mut h, &mut cx) {
            Readiness::Failed(why) => assert!(why.contains("api9") && why.contains("9.0"), "{why}"),
            other => panic!("{other:?}"),
        }
        // Nothing is left to try: asking again changes nothing.
        V2Frontend::choose(&mut h, None);
        assert_eq!(settle(&mut h), V2State::Classic);
        assert_eq!(http.asked.borrow().len(), 2);
    }

    #[test]
    fn a_skin_for_another_script_api_is_not_downloaded() {
        let (_, api9) = fixture("api9");
        let pkg = kit_package();
        let full = ui2_host::hash_hex(&pkg.hash());
        let idx = index(&[api9, entry("kit", &full, true)]);
        let file = format!("ui2/kit-{}.ui2", &full[..16]);
        let (mut h, http) = host(
            FakeHttp::default()
                .ok(INDEX_PATH, idx)
                .ok(&file, pkg.bytes().to_vec()),
            Some("api9"),
        );
        assert_eq!(settle(&mut h), V2State::Running("kit".into()));
        assert_eq!(*http.asked.borrow(), [INDEX_PATH.to_owned(), file]);
    }

    #[test]
    fn a_skin_that_fails_while_running_is_reported_once_and_the_default_comes_up() {
        let (bytes, scriptfail) = fixture("scriptfail");
        let file = format!("ui2/{}", scriptfail["file"].as_str().expect("file"));
        let pkg = kit_package();
        let full = ui2_host::hash_hex(&pkg.hash());
        let kit_file = format!("ui2/kit-{}.ui2", &full[..16]);
        let workers = ui2_script::testing::MockScriptHost::new();
        let w = workers.clone();
        let (mut h, _) = host_with(
            FakeHttp::default()
                .ok(INDEX_PATH, index(&[scriptfail, entry("kit", &full, true)]))
                .ok(&file, bytes)
                .ok(&kit_file, pkg.bytes().to_vec()),
            Some("scriptfail"),
            Some(Rc::new(move || Box::new(w.clone()) as Box<dyn ScriptHost>)),
        );
        assert_eq!(settle(&mut h), V2State::Running("scriptfail".into()));
        workers.reply(
            workers.live()[0],
            r#"{"t":"fail","ctx":"skin","error":{"message":"Error: boom"}}"#,
        );
        let r = frame(&mut h);
        assert!(!h.after_frame(&r));
        let why = Frontend::take_failure(&mut h).expect("the failure");
        assert!(why.contains("scriptfail") && why.contains("boom"), "{why}");
        assert_eq!(Frontend::take_failure(&mut h), None, "once");
        assert!(h.recovers(), "the default skin loads");
        assert_eq!(settle(&mut h), V2State::Running("kit".into()));
        assert!(
            h.text_bridge.is_none(),
            "the next skin holds the text input"
        );
    }

    #[test]
    fn without_a_script_host_a_skin_with_scripts_runs_on_markup() {
        // No script workers: the null host refuses them.
        let mut h = running(&scripted_package());
        let r = frame(&mut h);
        assert_eq!((&r.scripts, r.fatal.is_some()), (&None, false));
        assert!(h.after_frame(&r));
    }

    #[test]
    fn the_reference_background_is_a_hex_colour() {
        assert_eq!(parse_background("1a2b3c"), Some([0x1a, 0x2b, 0x3c]));
        assert_eq!(parse_background("#FFFFFF"), Some([255, 255, 255]));
        for bad in ["", "fff", "12345g", "1234567", "ééé"] {
            assert_eq!(parse_background(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn anchors_find_plots_cities_and_units_on_the_map() {
        use render::entities::{CityBanner, LabelData, UnitFlag, WorldLabel};
        use render::glam::Vec3;
        use ui2_host::{AnchorKind, ModelValue};
        let at = Vec3::new(1.0, 2.0, 3.0);
        let labels = vec![
            WorldLabel {
                cell: (4, 5),
                position: at,
                data: LabelData::CityBanner(CityBanner {
                    city: 7,
                    owner: 0,
                    name: "Rome".into(),
                    population: 1,
                    capital: true,
                    hp: 0,
                    max_hp: 0,
                }),
            },
            WorldLabel {
                cell: (6, 5),
                position: at * 2.0,
                data: LabelData::UnitFlag(UnitFlag {
                    unit: 9,
                    owner: 0,
                    type_name: "UNIT_WARRIOR".into(),
                    hp: 1,
                    max_hp: 1,
                    needs_orders: false,
                    civilian: false,
                }),
            },
        ];
        let key = |k: u64| ModelValue::Key(ui2_host::ModelKey(k));
        let none = |_: i8, _: i32| None;
        assert_eq!(
            anchor_point(AnchorKind::City, &key(7), &labels, None, &none),
            Some(at)
        );
        assert_eq!(
            anchor_point(AnchorKind::City, &key(8), &labels, None, &none),
            None
        );
        // The id is the low 32 bits of the key.
        assert_eq!(
            anchor_point(AnchorKind::Unit, &key((3 << 32) | 9), &labels, None, &none),
            Some(at * 2.0)
        );
        let plot = ModelValue::Struct(vec![ModelValue::I32(2), ModelValue::I32(3)]);
        let c = render::hexgrid::cell_center(2, 3);
        assert_eq!(
            anchor_point(AnchorKind::Plot, &plot, &labels, None, &none),
            Some(Vec3::new(c.x, 0.0, c.y))
        );
        assert_eq!(
            anchor_point(AnchorKind::Plot, &key(1), &labels, None, &none),
            None
        );
        // A unit without a label of the renderer hangs over its cell (owner 3, id 10 stands on 2, 3).
        let cells = |owner: i8, id: i32| (owner == 3 && id == 10).then_some((2, 3));
        assert_eq!(
            anchor_point(
                AnchorKind::Unit,
                &key((3 << 32) | 10),
                &labels,
                None,
                &cells
            ),
            Some(Vec3::new(c.x, render::entities::labels::LABEL_HEIGHT, c.y))
        );
        assert_eq!(
            anchor_point(
                AnchorKind::Unit,
                &key((2 << 32) | 10),
                &labels,
                None,
                &cells
            ),
            None
        );
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
