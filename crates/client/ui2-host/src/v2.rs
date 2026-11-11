//! One running UI v2: the view model, its source, and the skin (runtime, text engine, scene).
//!
//! The model store and the source outlive a skin: a later skin switch replaces only [`Skin`] and keeps the model,
//! so the new skin's first frame reads the current values. The replica is not owned here: the game session lends
//! a [`ReplicaView`] to each [`V2Ui::frame`].
//!
//! The frame runs the host's order, in the scope of the first slice (no scripts, actions, input or anchors yet):
//! runtime output → source → model frame → runtime delta → runtime frame → glyph resolve. The application then
//! hands [`V2Ui::scene_mut`] to the renderer.

use std::rc::Rc;
use std::sync::Arc;

use replica::ReplicaStep;
use ui2_draw::{Scene, SceneStats};
use ui2_model::{ModelStore, Owner, Schema};
use ui2_package::Package;
use ui2_package::style::Cursor;
use ui2_runtime::{Env, FrameStats, LoadError, NodeClass, Runtime, Services, UiError};
use ui2_source::{ReplicaView, SourceFrame, SourceStats, UiSource};
use ui2_text::TextEngine;

use crate::error::{LoadFailure, SkinLoadError};
use crate::registry::check_package;

/// The time budget of glyph rasterization per frame, ms.
pub const RESOLVE_BUDGET_MS: f32 = 1.5;

/// A monotonic clock in milliseconds that the application supplies (library code reads no time on wasm).
pub type HostClock = Rc<dyn Fn() -> f64>;

/// What a new UI v2 starts with.
#[derive(Clone, Default)]
pub struct V2Init {
    /// Window size, pixel density, UI scale and theme.
    pub env: Env,
    /// What the runtime borrows from the application (the game's localizer).
    pub services: Services,
    /// Times the load and the frame's steps; without one every time is 0.
    pub clock: Option<HostClock>,
}

/// The input of one frame, made by the application after it pumped the game session.
pub struct V2Frame<'a> {
    /// The replica as the session lends it; `None` when there is none (lobby, no game yet).
    pub replica: Option<ReplicaView<'a>>,
    /// What the session's pump did this frame (the source's change signal).
    pub steps: &'a [ReplicaStep],
    /// The frame clock, ms.
    pub now_ms: f64,
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
    /// Picture pages of the package (not uploaded by this crate; see [`V2Ui::new`]).
    pub image_pages: usize,
}

/// What one frame did.
#[derive(Debug, Clone, PartialEq)]
pub struct V2FrameReport {
    /// The source's work.
    pub source: SourceStats,
    /// Operations of the model frame handed to the runtime (0: no model frame).
    pub delta_ops: u32,
    /// The model revision after the frame.
    pub model_revision: u64,
    /// The runtime's work.
    pub runtime: FrameStats,
    /// The scene's counters (frame fields: this frame's runtime paint and glyph resolve).
    pub scene: SceneStats,
    /// The pointer cursor the UI wants.
    pub cursor: Cursor,
    /// Transitions are running: keep producing frames.
    pub animating: bool,
    /// Runtime errors reported for the first time in this frame (bindings, styles, fonts of a panel).
    pub errors: Vec<UiError>,
    /// Markup output dropped because nothing consumes it yet (intents, state writes, script events).
    pub dropped_output: u32,
    /// Source step, ms (0 without a clock).
    pub source_ms: f32,
    /// Glyph resolve step, ms (0 without a clock).
    pub resolve_ms: f32,
    /// The whole frame, ms (0 without a clock).
    pub cpu_ms: f32,
}

impl V2FrameReport {
    /// The report as named counters (`ui2.*`) for the application's stats overlay.
    pub fn counters(&self) -> Vec<(&'static str, f64)> {
        let r = &self.runtime;
        vec![
            ("ui2.cpu_ms", f64::from(self.cpu_ms)),
            ("ui2.source_ms", f64::from(self.source_ms)),
            ("ui2.runtime_ms", f64::from(r.cpu_ms)),
            ("ui2.resolve_ms", f64::from(self.resolve_ms)),
            ("ui2.source.roots", f64::from(self.source.roots_built)),
            ("ui2.source.reads", f64::from(self.source.getter_calls)),
            ("ui2.model.ops", f64::from(self.delta_ops)),
            ("ui2.binds", f64::from(r.binds_evaluated)),
            ("ui2.shaped", f64::from(r.shaped_runs)),
            ("ui2.layout_runs", f64::from(r.layout_runs)),
            ("ui2.chunks_rebuilt", f64::from(r.chunks_rebuilt)),
            ("ui2.nodes", f64::from(r.nodes)),
            ("ui2.instances", self.scene.instances as f64),
            ("ui2.raster_glyphs", f64::from(self.scene.raster_glyphs)),
            ("ui2.raster_deferred", f64::from(self.scene.raster_deferred)),
            ("ui2.errors", self.errors.len() as f64),
        ]
    }
}

/// What belongs to one skin and is replaced when the skin changes.
struct Skin {
    pkg: Arc<Package>,
    runtime: Runtime,
    text: TextEngine,
    scene: Scene,
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
        if !pkg.images().pages.is_empty() {
            // Picture pages go to the GPU with the scene's static mask pages, which the render side does not
            // take yet: until then icons draw from an empty mask layer.
            tracing::warn!(
                target: "ui2_host",
                skin = %id,
                pages = pkg.images().pages.len(),
                "picture pages are not uploaded yet; icons of this skin do not show"
            );
        }
        Ok(Skin {
            pkg,
            runtime,
            text,
            scene,
        })
    }
}

fn fatal(e: LoadError) -> String {
    match e {
        LoadError::Fatal(s) => s,
    }
}

/// One running UI v2.
pub struct V2Ui {
    store: ModelStore,
    source: UiSource,
    skin: Skin,
    clock: Option<HostClock>,
    load: LoadStats,
    frames: u64,
}

impl V2Ui {
    /// Brings up a skin: package checks against this client (model paths and intents; the hash check against
    /// the index is [`SkinRegistry::open`](crate::SkinRegistry::open)'s), fonts into the text engine, the
    /// document tree, an empty scene, a model store and a source. Any failure is a [`SkinLoadError`] for the
    /// application's fallback.
    ///
    /// The package's picture pages are not handed to the scene yet (the render side has no page upload); a
    /// package with pages loads, logs a warning and draws its icons from an empty mask layer.
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
        let load = LoadStats {
            build_ms: (now() - t0) as f32,
            fonts: pkg.fonts().len(),
            nodes: skin.runtime.arena().len(),
            image_pages: pkg.images().pages.len(),
        };
        Ok(V2Ui {
            store: ModelStore::new(Schema::v1()),
            source: UiSource::new(Schema::v1()),
            skin,
            clock: init.clock,
            load,
            frames: 0,
        })
    }

    /// Runs one frame. Afterwards the application hands [`scene_mut`](Self::scene_mut) to the renderer.
    pub fn frame(&mut self, f: V2Frame<'_>) -> V2FrameReport {
        let clock = self.clock.clone();
        let now = || clock.as_ref().map_or(0.0, |c| c());
        let t0 = now();
        let Skin {
            runtime,
            text,
            scene,
            ..
        } = &mut self.skin;
        scene.reset_frame_stats();

        // Nothing consumes markup output in this slice (no actions module, no scripts): drop it, counted.
        let out = runtime.drain_output();
        let dropped_output =
            (out.intents.len() + out.state_writes.len() + out.script_events.len()) as u32;
        if dropped_output > 0 {
            tracing::debug!(target: "ui2_host", dropped_output, "markup output has no consumer yet");
        }

        let t_source = now();
        let source = {
            let mut w = self.store.writer(Owner::Source);
            self.source.frame(
                SourceFrame {
                    replica: f.replica,
                    steps: f.steps,
                    now_ms: f.now_ms.max(0.0) as u64,
                },
                &mut w,
            )
        };
        let source_ms = (now() - t_source) as f32;

        let mut delta_ops = 0;
        if let Some(d) = self.store.take_frame() {
            delta_ops = d.ops.len() as u32;
            runtime.apply_delta(&d, &self.store);
        }
        let out = runtime.frame(f.now_ms, &self.store, text, scene);
        for e in &out.errors {
            tracing::warn!(target: "ui2_host", "{e}");
        }

        let t_resolve = now();
        scene.resolve(text, RESOLVE_BUDGET_MS);
        let resolve_ms = (now() - t_resolve) as f32;
        self.frames += 1;

        V2FrameReport {
            source,
            delta_ops,
            model_revision: self.store.revision(),
            runtime: out.stats,
            scene: scene.stats(),
            cursor: out.cursor,
            animating: out.animating,
            errors: out.errors,
            dropped_output,
            source_ms,
            resolve_ms,
            cpu_ms: (now() - t0) as f32,
        }
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
        self.skin.runtime.set_localizer(loc);
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

    /// Frames run since the skin was loaded.
    pub fn frames(&self) -> u64 {
        self.frames
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
