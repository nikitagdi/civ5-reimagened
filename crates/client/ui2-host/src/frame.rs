//! One frame of UI v2, in the host's order:
//!
//! 1. the network's events of the frame → `UiActions::on_event` (answers settle waiting orders; a new game also
//!    resets the source), then the game's mod set → `UiActions::set_mods` when it changed;
//! 2. `Runtime::drain_output`;
//! 3. the markup's state writes → `UiActions::state_ops`, then the intents of the markup and of the host (map
//!    clicks) → `UiActions::apply`, orders leaving through the frame's [`HostNet`] sink; mod intents are taken
//!    (no dispatcher consumes them yet);
//! 4. the source writes its roots from the replica, the session, and the actions module's `ui` root and
//!    dismissed notes → `UiActions::reconcile` → `ModelStore::take_frame`;
//! 5. `Runtime::apply_delta` → `Runtime::frame` → `Scene::resolve`.
//!
//! `reconcile` runs in every frame, so every frame that took events settles what they decided. Scripts, mod
//! panels, anchors and settings writes join the order when their modules arrive.

use std::sync::Arc;

use replica::ReplicaStep;
use ui2_actions::{ActionEvent, StateNs};
use ui2_draw::SceneStats;
use ui2_model::Owner;
use ui2_model::wire::WireIntent;
use ui2_package::style::Cursor;
use ui2_runtime::{FrameStats, UiError};
use ui2_source::{ReplicaView, SourceFrame, SourceStats};

use crate::sink::{HostNet, Offline};
use crate::v2::{RESOLVE_BUDGET_MS, Skin, V2Ui};

/// The input of one frame, made by the application after it pumped the game session and the network.
pub struct V2Frame<'a> {
    /// The replica as the session lends it; `None` when there is none (lobby, no game yet).
    pub replica: Option<ReplicaView<'a>>,
    /// What the session's pump did this frame (the source's change signal).
    pub steps: &'a [ReplicaStep],
    /// The network for this frame (in the app: a [`NetSink`](crate::NetSink) over the client core); `None`:
    /// offline, every order is refused and the session is empty.
    pub net: Option<&'a mut dyn HostNet>,
    /// What the network reported since the last frame, in the order it came.
    pub events: &'a [ActionEvent],
    /// The ids of the game's mods in index order, when the mod set became known or changed.
    pub mods: Option<Vec<Arc<str>>>,
    /// The frame clock, ms.
    pub now_ms: f64,
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
    /// Network events taken in.
    pub events: u32,
    /// Intents applied (markup and host).
    pub intents: u32,
    /// Intents and state writes refused for their form (a mistake of the skin; each is logged).
    pub refused: u32,
    /// Output dropped because nothing consumes it yet (script events, intents for mods).
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
            ("ui2.intents", f64::from(self.intents)),
            ("ui2.errors", self.errors.len() as f64),
        ]
    }
}

impl V2Ui {
    /// Runs one frame in the host's order (see the [module docs](crate::frame)). Afterwards the application
    /// hands [`scene_mut`](Self::scene_mut) to the renderer.
    pub fn frame(&mut self, f: V2Frame<'_>) -> V2FrameReport {
        let clock = self.clock.clone();
        let now = || clock.as_ref().map_or(0.0, |c| c());
        let t0 = now();
        let now_ms = f.now_ms.max(0.0) as u64;
        let V2Frame {
            replica,
            steps,
            net,
            events,
            mods,
            now_ms: frame_ms,
        } = f;
        let mut offline = Offline;
        let net: &mut dyn HostNet = match net {
            Some(n) => n,
            None => &mut offline,
        };
        let Skin {
            runtime,
            text,
            scene,
            ..
        } = &mut self.skin;
        scene.reset_frame_stats();

        // 1. The network's events, then the mod set.
        for ev in events {
            if *ev == ActionEvent::NewGame {
                // Another game: every root is rebuilt from it, its catalog read again.
                self.source.reset();
            }
            self.actions.on_event(ev, now_ms);
        }
        if let Some(ids) = mods {
            self.actions.set_mods(ids);
        }

        // 2. What the markup asked since the last frame.
        let out = runtime.drain_output();

        // 3. State writes, then intents; orders leave through the sink. Settings writes come from scripts only.
        let mut refused = 0;
        for e in self
            .actions
            .state_ops(StateNs::Skin, &out.state_writes, &mut self.store)
        {
            tracing::warn!(target: "ui2_host", "state write refused: {e}");
            refused += 1;
        }
        let mut intents: Vec<WireIntent> = out.intents;
        intents.append(&mut self.queued);
        let applied = intents.len() as u32;
        self.actions
            .apply(&mut intents, &mut self.store, net.sink(), now_ms);
        // The actions module logs each form error once; the list only needs emptying.
        refused += self.actions.take_form_errors().len() as u32;
        let mod_intents = self.actions.take_mod_intents();
        let dropped_output = (out.script_events.len() + mod_intents.len()) as u32;
        if dropped_output > 0 {
            tracing::debug!(target: "ui2_host", dropped_output, "script events and mod intents have no consumer yet");
        }

        // 4. The source, then the actions module judges its state by what the source wrote, then the delta.
        let t_source = now();
        let source = {
            let mut w = self.store.writer(Owner::Source);
            self.source.frame(
                SourceFrame {
                    replica,
                    steps,
                    session: net.session(),
                    dismissed: self.actions.dismissed(),
                    now_ms,
                },
                &mut w,
                self.actions.ui(),
            )
        };
        let source_ms = (now() - t_source) as f32;
        self.actions
            .reconcile(&mut self.store, self.source.notification_ids(), now_ms);

        // 5. The runtime and the glyphs.
        let mut delta_ops = 0;
        if let Some(d) = self.store.take_frame() {
            delta_ops = d.ops.len() as u32;
            runtime.apply_delta(&d, &self.store);
        }
        let out = runtime.frame(frame_ms, &self.store, text, scene);
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
            events: events.len() as u32,
            intents: applied,
            refused,
            dropped_output,
            source_ms,
            resolve_ms,
            cpu_ms: (now() - t0) as f32,
        }
    }
}
