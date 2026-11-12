//! One running UI v2: the view model, its source, the actions module, and the skin (runtime, text engine, scene).
//!
//! The model store, the source and the actions module outlive a skin: a later skin switch replaces only the skin's
//! part (runtime, text engine, scene) and keeps the model and the UI's own state (selection, open windows,
//! waiting orders), so the new skin's first frame reads the current values. The replica and the network are not
//! owned here: the application lends them to each [`V2Ui::frame`] (see [`crate::frame`] for its order).

use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use platform::ScriptHost;
use platform::ui2::{TextInputBridge, TextInputRequest};
use replica::GameRead;
use ui2_actions::{MemorySettings, UiActions};
use ui2_draw::Scene;
use ui2_draw::{Color, Vec2};
use ui2_model::wire::{ScriptCtxId, WireIntent};
use ui2_model::{ModelStore, Schema, Value};
use ui2_package::Package;
use ui2_package::profile::action::AnchorKind;
use ui2_runtime::{AnchorId, AnchorScreen, Env, LoadError, NodeClass, Runtime, Services};
use ui2_script::{ScriptStats, ScriptStatus};
use ui2_source::{SourceStats, StaticSource, UiSource};
use ui2_text::TextEngine;

use crate::error::{LoadFailure, SkinLoadError};
use crate::registry::check_package;
use crate::scripts::Scripts;

/// The time budget of glyph rasterization per frame, ms.
pub const RESOLVE_BUDGET_MS: f32 = 1.5;

/// A monotonic clock in milliseconds that the application supplies (library code reads no time on wasm).
pub type HostClock = Rc<dyn Fn() -> f64>;

/// What a new UI v2 starts with.
#[derive(Default)]
pub struct V2Init {
    /// Window size, pixel density, UI scale and theme.
    pub env: Env,
    /// What the runtime borrows from the application (the game's localizer).
    pub services: Services,
    /// Times the load and the frame's steps; without one every time is 0.
    pub clock: Option<HostClock>,
    /// The workers the skin's scripts run in (the page's on the web). `None`, or a host that cannot start a
    /// worker (`platform::NullScriptHost`): the skin works on its markup alone.
    pub scripts: Option<Box<dyn ScriptHost>>,
}

/// What loading the skin cost.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LoadStats {
    /// Fonts, runtime tree and scene, ms (0 without a clock). Download and package checks are the caller's.
    pub build_ms: f32,
    /// Font faces registered with the text engine.
    pub fonts: usize,
    /// Nodes of the document after instantiation.
    pub nodes: usize,
    /// Picture pages of the package handed to the scene.
    pub image_pages: usize,
}

/// What belongs to one skin and is replaced when the skin changes.
pub(crate) struct Skin {
    pub(crate) pkg: Arc<Package>,
    pub(crate) runtime: Runtime,
    pub(crate) text: TextEngine,
    pub(crate) scene: Scene,
}

impl Skin {
    fn build(
        pkg: Arc<Package>,
        env: Env,
        services: Services,
        clock: Option<&HostClock>,
    ) -> Result<Skin, SkinLoadError> {
        let id = pkg.manifest().id.clone();
        let fail = |f| SkinLoadError::new(id.as_str(), f);
        let mut text = TextEngine::new();
        ui2_runtime::load_faces(&pkg, &mut text).map_err(|e| fail(LoadFailure::Fonts(fatal(e))))?;
        let mut runtime = Runtime::new(Arc::clone(&pkg), env, services)
            .map_err(|e| fail(LoadFailure::Tree(fatal(e))))?;
        // The runtime syncs the scene's scale on every frame; start at the right one so nothing is redone.
        let mut scene = Scene::new(runtime.style_env().scale.k);
        if let Some(c) = clock {
            let (a, b) = (Rc::clone(c), Rc::clone(c));
            runtime.set_clock(Box::new(move || a()));
            scene.set_clock(Box::new(move || b()));
        }
        // The package's picture pages: masks join the coverage layers, colour pages the image array.
        let pages = pkg
            .images()
            .pages
            .iter()
            .map(|p| pkg.page_data(p))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| fail(LoadFailure::Package(e)))?;
        scene
            .set_pages(pkg.images(), pages)
            .map_err(|e| fail(LoadFailure::Tree(e.to_string())))?;
        Ok(Skin {
            pkg,
            runtime,
            text,
            scene,
        })
    }
}

/// The name a sample gives an anchor's object: `city:<id>`, `unit:<id>`, `plot:<x>,<y>`.
fn anchor_name(kind: AnchorKind, key: &Value) -> Option<String> {
    let id = || match key {
        Value::Key(k) => Some(i64::from(k.0 as u32 as i32)),
        Value::I32(v) => Some(i64::from(*v)),
        Value::U32(v) => Some(i64::from(*v)),
        _ => None,
    };
    Some(match kind {
        AnchorKind::City => format!("city:{}", id()?),
        AnchorKind::Unit => format!("unit:{}", id()?),
        AnchorKind::Plot => {
            let Value::Struct(xy) = key else {
                return None;
            };
            match (xy.first(), xy.get(1)) {
                (Some(Value::I32(x)), Some(Value::I32(y))) => format!("plot:{x},{y}"),
                _ => return None,
            }
        }
    })
}

fn fatal(e: LoadError) -> String {
    match e {
        LoadError::Fatal(s) => s,
    }
}

/// One running UI v2.
pub struct V2Ui {
    pub(crate) store: ModelStore,
    pub(crate) source: UiSource,
    /// The UI's own state and the way out for orders; lives as long as the model, across skins.
    pub(crate) actions: UiActions,
    /// Intents from outside the markup (map clicks), applied with the markup's in the next frame.
    pub(crate) queued: Vec<WireIntent>,
    pub(crate) skin: Skin,
    pub(crate) clock: Option<HostClock>,
    /// The skin's scripts; `None` when the skin has none or they cannot run here (no script work in a frame).
    pub(crate) scripts: Option<Scripts>,
    load: LoadStats,
    pub(crate) frames: u64,
    /// The map anchors of the document and the object each follows (kind and key), as the runtime reported them.
    pub(crate) anchors: BTreeMap<AnchorId, (AnchorKind, Value)>,
    /// The platform's text input for the focused text field (`None`: fields take no typing).
    pub(crate) text_bridge: Option<Box<dyn TextInputBridge>>,
    /// What the runtime borrows from the application; a new skin's runtime gets it too.
    services: Services,
    /// `?ui2bg=`: the reference background a new skin's scene gets too.
    golden: Option<[u8; 3]>,
    /// The sample mode (`?ui2sample=`): a sample file writes the model instead of the replica source.
    pub(crate) sample: Option<StaticSource>,
    /// The mods' side: their model writes, intents and panels (outlives a skin, like the model).
    pub(crate) mods: crate::mods::Mods,
    /// Where the skins find the game's pictures; a new skin's runtime gets it too.
    assets: Option<ui2_runtime::AssetResolver>,
}

impl V2Ui {
    /// Brings up a skin: package checks against this client (model paths and intents; the hash check against
    /// the index is [`SkinRegistry::open`](crate::SkinRegistry::open)'s), fonts into the text engine, the
    /// document tree, an empty scene, a model store and a source. Any failure is a [`SkinLoadError`] for the
    /// application's fallback.
    ///
    /// The package's picture pages go to the scene; the renderer uploads them with the scene.
    pub fn new(pkg: Arc<Package>, init: V2Init) -> Result<V2Ui, SkinLoadError> {
        let now = || init.clock.as_ref().map_or(0.0, |c| c());
        let t0 = now();
        check_package(&pkg).map_err(|f| SkinLoadError::new(pkg.manifest().id.as_str(), f))?;
        let skin = Skin::build(
            Arc::clone(&pkg),
            init.env,
            init.services.clone(),
            init.clock.as_ref(),
        )?;
        let mut store = ModelStore::new(Schema::v1());
        // Skin settings stay in memory for the session until the device store is wired in.
        let mut actions = UiActions::new(Schema::v1(), Box::new(MemorySettings::default()));
        // The `ui` root exists before the first frame, and the skin's own state with its defaults before the
        // runtime and the scripts first look at it.
        actions.publish(&mut store);
        crate::state::mount(&pkg, &mut actions, &mut store);
        // The skin's scripts start on that model; a script that fails while loading comes back from a later frame
        // as `V2FrameReport::fatal`.
        let scripts = match init.scripts {
            Some(host) => Scripts::start(host, &pkg, &skin.runtime, &store, &mut actions)
                .map_err(|f| SkinLoadError::new(pkg.manifest().id.as_str(), f))?,
            None => None,
        };
        let load = LoadStats {
            build_ms: (now() - t0) as f32,
            fonts: pkg.fonts().len(),
            nodes: skin.runtime.arena().len(),
            image_pages: pkg.images().pages.len(),
        };
        let mut source = UiSource::new(Schema::v1());
        if let Some(c) = &init.clock {
            // The source's profile times its roots and reads with the host's clock.
            let c = Rc::clone(c);
            source.set_profile_clock(Box::new(move || c()));
        }
        Ok(V2Ui {
            store,
            source,
            actions,
            queued: Vec::new(),
            skin,
            clock: init.clock,
            scripts,
            load,
            frames: 0,
            anchors: BTreeMap::new(),
            text_bridge: None,
            services: init.services,
            golden: None,
            sample: None,
            mods: crate::mods::Mods::default(),
            assets: None,
        })
    }

    /// Where the skin finds the game's pictures (`data-bind-src` of an `icon:<atlas>:<index>`): kept for every
    /// skin that comes after this one too.
    pub fn set_asset_resolver(&mut self, resolver: ui2_runtime::AssetResolver) {
        self.skin.runtime.set_asset_resolver(Rc::clone(&resolver));
        self.assets = Some(resolver);
    }

    /// The sample mode (`?ui2sample=<state>`): the model comes from a sample file of the skin
    /// (`sample/<state>.json`: `{model, state, ui, anchors}`) instead of a game. Its roots are written in the next
    /// frame, its `ui` part becomes the UI's own state (an open window, a focus), its `state` the skin's, and
    /// [`place_sample_anchors`](Self::place_sample_anchors) puts the map anchors where it says. No replica, no
    /// network: a frame reads nothing of a game.
    ///
    /// # Errors
    /// The sample does not read against this client's model; nothing changed.
    pub fn set_sample(&mut self, text: &str) -> Result<(), String> {
        let sample = ui2_model::Sample::parse(Schema::v1(), text).map_err(|e| e.join("; "))?;
        if let Some((_, ui)) = sample.ui() {
            self.actions
                .adopt_ui(ui.clone(), &mut self.store)
                .map_err(|e| format!("the sample's ui: {e}"))?;
        }
        if let Err(errs) = sample.apply_state(&mut self.store) {
            for e in errs {
                tracing::warn!(target: "ui2_host", "sample state: {e}");
            }
        }
        self.sample = Some(StaticSource::new(sample));
        Ok(())
    }

    /// The UI runs on a sample, not on a game.
    pub fn sample_mode(&self) -> bool {
        self.sample.is_some()
    }

    /// In the sample mode: places the map anchors at the sample's points (CSS px, times `dpr`); an anchor the sample
    /// does not name is hidden. Names are `city:<id>`, `unit:<id>` and `plot:<x>,<y>`.
    pub fn place_sample_anchors(&mut self, dpr: f32) {
        let Some(points) = self.sample.as_ref().map(|s| s.anchors().clone()) else {
            return;
        };
        self.place_anchors(|kind, key| {
            let name = anchor_name(kind, key)?;
            let (x, y) = points.get(&name)?;
            Some((Vec2::new(x * dpr, y * dpr), 1.0))
        });
    }

    /// Changes the skin on the screen and keeps the rest of the UI: the model, the source, the actions module with
    /// the UI's own state (the open windows are `ui.windows.*`, the same for every skin) and the text input. The
    /// new skin's runtime, text engine and scene replace the old ones and start from a snapshot of the model; the
    /// old skin's scripts stop and the new skin's start; the old skin's `state.*` is set aside (it comes back when
    /// that skin is chosen again; for the same skin, see [`reload`](Self::reload)) and the new skin's is mounted.
    /// `scripts`: makes the workers for a skin's scripts, as in [`V2Init::scripts`] (`None`: markup only).
    ///
    /// # Errors
    /// The new skin cannot be used (package checks, fonts, tree, its scripts): the old skin runs on with its
    /// state (its scripts start again when the new skin's failed to).
    pub fn set_skin(
        &mut self,
        pkg: Arc<Package>,
        scripts: Option<&dyn Fn() -> Box<dyn ScriptHost>>,
    ) -> Result<LoadStats, SkinLoadError> {
        let clock = self.clock.clone();
        let now = || clock.as_ref().map_or(0.0, |c| c());
        let t0 = now();
        let id = pkg.manifest().id.clone();
        check_package(&pkg).map_err(|f| SkinLoadError::new(id.as_str(), f))?;
        let mut skin = Skin::build(
            Arc::clone(&pkg),
            self.skin.runtime.env(),
            self.services.clone(),
            self.clock.as_ref(),
        )?;
        // The old scripts go before the state they watch is remounted; the new ones start on the new state.
        let old_scripts = self.scripts.take();
        drop(old_scripts);
        crate::state::mount(&pkg, &mut self.actions, &mut self.store);
        // In the sample mode the sample's state goes over the new skin's defaults, as at the start.
        if let Some(s) = &self.sample {
            let _ = s.sample().apply_state(&mut self.store);
        }
        let started = match scripts {
            Some(make) => {
                Scripts::start(make(), &pkg, &skin.runtime, &self.store, &mut self.actions)
            }
            None => Ok(None),
        };
        let new_scripts = match started {
            Ok(s) => s,
            Err(f) => {
                // Back to the old skin as it was: its state comes back, its scripts start again on it.
                let old = Arc::clone(&self.skin.pkg);
                crate::state::mount(&old, &mut self.actions, &mut self.store);
                self.scripts = match scripts {
                    Some(make) => Scripts::start(
                        make(),
                        &old,
                        &self.skin.runtime,
                        &self.store,
                        &mut self.actions,
                    )
                    .unwrap_or_else(|e| {
                        tracing::warn!(target: "ui2_host", "the scripts of skin {} do not start again: {e}", old.manifest().id);
                        None
                    }),
                    None => None,
                };
                return Err(SkinLoadError::new(id.as_str(), f));
            }
        };
        if let Some(r) = &self.assets {
            skin.runtime.set_asset_resolver(Rc::clone(r));
        }
        // The new runtime has seen no frame: it takes every root of the model as it is.
        skin.runtime
            .apply_delta(&self.store.snapshot(), &self.store);
        if let Some(b) = self.text_bridge.as_mut() {
            b.request(&TextInputRequest::Blur);
        }
        self.skin = skin;
        self.scripts = new_scripts;
        self.anchors.clear();
        self.frames = 0;
        self.set_golden(self.golden);
        self.load = LoadStats {
            build_ms: (now() - t0) as f32,
            fonts: pkg.fonts().len(),
            nodes: self.skin.runtime.arena().len(),
            image_pages: pkg.images().pages.len(),
        };
        Ok(self.load)
    }

    /// A new build of the running skin (hot reload): as [`set_skin`](Self::set_skin), and the skin's `state.*`
    /// keeps every value whose path and type the new build still has. `Ok(false)`: the package is the one that
    /// runs (same hash), nothing to do.
    ///
    /// # Errors
    /// As [`set_skin`](Self::set_skin): the old build runs on.
    pub fn reload(
        &mut self,
        pkg: Arc<Package>,
        scripts: Option<&dyn Fn() -> Box<dyn ScriptHost>>,
    ) -> Result<bool, SkinLoadError> {
        if pkg.hash() == self.skin.pkg.hash() {
            return Ok(false);
        }
        self.set_skin(pkg, scripts).map(|_| true)
    }

    /// Gives the UI the platform's text input (the host owns one per page and hands it to the running UI); the
    /// previous one comes back. Each frame passes the runtime's request for the focused field to it, then the
    /// field's state it reports back to the runtime.
    pub fn set_text_bridge(
        &mut self,
        bridge: Box<dyn TextInputBridge>,
    ) -> Option<Box<dyn TextInputBridge>> {
        self.text_bridge.replace(bridge)
    }

    /// Takes the text input back (before the UI is dropped, so the next one gets it).
    pub fn take_text_bridge(&mut self) -> Option<Box<dyn TextInputBridge>> {
        self.text_bridge.take()
    }

    /// The UI leaves the screen: the platform's text input lets the focused field go.
    pub fn release_text_input(&mut self) {
        if let Some(b) = self.text_bridge.as_mut() {
            b.request(&TextInputRequest::Blur);
        }
    }

    /// Makes the core's slow first reads once, outside the frames (`UiSource::warm_up`): call it once when a game
    /// is joined or loaded and after [`reset_source`](Self::reset_source) for a new game, at a moment that may
    /// take a few ms more. Its reads count in the returned stats (`spent_units`: their cost units).
    pub fn warm_up(&mut self, read: &mut dyn GameRead) -> SourceStats {
        self.source.warm_up(read)
    }

    /// Makes the core's slow first reads after a snapshot load (`UiSource::warm_turn`: the top bar alone costs
    /// the core several ms after each load): call it in the frame whose pump loaded a snapshot, right after the
    /// pump and before [`frame`](Self::frame), as part of the load. Returns its stats and its measured time, ms
    /// (0 without a clock).
    pub fn warm_turn(&mut self, read: &mut dyn GameRead) -> (SourceStats, f64) {
        let stats = self.source.warm_turn(read, self.actions.ui());
        let us: f64 = self.source.profile().reads.iter().map(|c| c.us).sum();
        (stats, us / 1000.0)
    }

    /// Changes the window size, pixel density, UI scale or theme; the next frame redoes what depends on it.
    pub fn set_env(&mut self, env: Env) {
        if env != self.skin.runtime.env() {
            self.skin.runtime.set_env(env);
        }
    }

    /// The current environment.
    pub fn env(&self) -> Env {
        self.skin.runtime.env()
    }

    /// Changes the game's localizer: texts that depend on the language are evaluated again next frame.
    pub fn set_localizer(&mut self, loc: Rc<loc::Localizer>) {
        self.services.loc = Some(Rc::clone(&loc));
        self.skin.runtime.set_localizer(loc);
    }

    /// How many core reads the source may start model roots with in one frame (the rest wait for the next
    /// frame). Tests that look at one frame's model raise it.
    pub fn set_read_budget(&mut self, reads: u32) {
        self.source.set_read_budget(reads);
    }

    /// A new game or a new core: the next frame rebuilds every root of the model from the replica.
    pub fn reset_source(&mut self) {
        self.source.reset();
    }

    /// The scene, for the renderer's `prepare_ui2` after [`frame`](Self::frame).
    pub fn scene_mut(&mut self) -> &mut Scene {
        &mut self.skin.scene
    }

    /// The scene.
    pub fn scene(&self) -> &Scene {
        &self.skin.scene
    }

    /// The runtime (rectangle dumps, parts, nodes: debugging and test hooks).
    pub fn runtime(&self) -> &Runtime {
        &self.skin.runtime
    }

    /// The runtime, mutable: for tools and tests that rebuild a chunk's draw list
    /// ([`Runtime::chunk_items`]). Frames go through [`frame`](Self::frame) only.
    pub fn runtime_mut(&mut self) -> &mut Runtime {
        &mut self.skin.runtime
    }

    /// The view model.
    pub fn store(&self) -> &ModelStore {
        &self.store
    }

    /// The skin's own state as the model has it now: each declared path (without `state.`) and its value
    /// (playtest hooks, debugging). Paths that are not mounted are left out.
    pub fn skin_state(&self) -> Vec<(String, Value)> {
        self.skin
            .pkg
            .state_defaults()
            .filter_map(|(path, _)| {
                let f = self.store.resolve(&format!("state.{path}")).ok()?;
                let v = self.store.value(f.id, &[])?.clone();
                Some((path.to_string(), v))
            })
            .collect()
    }

    /// The actions module: the UI's own state (selection, windows, waiting orders, toasts, dismissed notes).
    pub fn actions(&self) -> &UiActions {
        &self.actions
    }

    /// An intent from outside the skin's markup (a click on the map picked by the application, a test hook):
    /// applied in the next frame, after the markup's own.
    pub fn push_intent(&mut self, intent: WireIntent) {
        self.queued.push(intent);
    }

    /// The running skin's package.
    pub fn package(&self) -> &Arc<Package> {
        &self.skin.pkg
    }

    /// The running skin's id.
    pub fn skin_id(&self) -> &str {
        &self.skin.pkg.manifest().id
    }

    /// What loading the skin cost.
    pub fn load_stats(&self) -> LoadStats {
        self.load
    }

    /// The counters of the skin's scripts; `None` when none run.
    pub fn script_stats(&self) -> Option<ScriptStats> {
        self.scripts.as_ref().map(|s| s.sup.stats())
    }

    /// Where the skin's script context is; `None` when none runs.
    pub fn script_status(&self) -> Option<ScriptStatus> {
        self.scripts
            .as_ref()
            .and_then(|s| s.sup.status(&ScriptCtxId::Skin))
    }

    /// Frames run since the skin was loaded.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// The number of map anchors the document has.
    pub fn anchor_count(&self) -> usize {
        self.anchors.len()
    }

    /// Places the map anchors after a frame, before the scene goes to the renderer: `project` gives the screen
    /// point (device px) and the scale of an anchor's object (`city`, `unit`: its key; `plot`: `{x, y}`), `None`
    /// when it is not on the screen (then nothing of the anchor is drawn or hit).
    pub fn place_anchors(&mut self, project: impl Fn(AnchorKind, &Value) -> Option<(Vec2, f32)>) {
        if self.anchors.is_empty() {
            return;
        }
        let placed: Vec<AnchorScreen> = self
            .anchors
            .iter()
            .map(|(&id, (kind, key))| match project(*kind, key) {
                Some((pos, scale)) => AnchorScreen {
                    id,
                    pos,
                    scale,
                    visible: true,
                },
                None => AnchorScreen {
                    id,
                    pos: Vec2::ZERO,
                    scale: 1.0,
                    visible: false,
                },
            })
            .collect();
        let Skin { runtime, scene, .. } = &mut self.skin;
        runtime.set_anchor_screen(&placed, scene);
    }

    /// Reference images (`?ui2bg=`): a solid background under the skin and the scene frozen at time 0, so a frame
    /// is the same each time. `None` leaves the scene as it is.
    pub fn set_golden(&mut self, background: Option<[u8; 3]>) {
        self.golden = background;
        if let Some([r, g, b]) = background {
            let scene = &mut self.skin.scene;
            scene.set_background(Some(Color::rgba(r, g, b, 255)));
            scene.freeze(Some(0.0));
        }
    }

    /// The strings of the document's text leaves in document order (for tests and the playtest hooks).
    pub fn texts(&self) -> Vec<String> {
        let arena = self.skin.runtime.arena();
        arena
            .preorder()
            .into_iter()
            .filter_map(|id| arena.get(id))
            .filter(|n| n.class() == NodeClass::Text)
            .filter_map(|n| n.text().map(str::to_string))
            .collect()
    }
}
