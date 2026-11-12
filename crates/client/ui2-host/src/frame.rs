//! One frame of UI v2, in the host's order:
//!
//! 1. the network's events of the frame → `UiActions::on_event` (answers settle waiting orders; a new game also
//!    resets the source), then the game's mod set → `UiActions::set_mods` when it changed;
//! 2. `ScriptSupervisor::drain` (what the skin's worker answered since the last frame; a script that failed while
//!    loading ends the frame's report as [`V2FrameReport::fatal`]), then `Runtime::drain_output`;
//! 3. the state writes of the markup, then of the scripts → `UiActions::state_ops`; the scripts' settings writes
//!    → `UiActions::settings_set`; the intents of the markup, of the scripts and of the host (map clicks) →
//!    `UiActions::apply`, orders leaving through the frame's [`HostNet`] sink; the mod intents it accepted are kept
//!    for the application ([`V2Ui::take_mod_intents`](crate::V2Ui::take_mod_intents));
//! 4. the writes of the mods' Rust halves (`Owner::Mod`, [`crate::mods`]), then the source writes its roots from
//!    the replica, the session, and the actions module's `ui` root and dismissed notes → `UiActions::reconcile` → `ModelStore::take_frame`;
//! 5. `Runtime::apply_delta` → the scripts' island commands (`Runtime::apply_island`; a refused one is logged and
//!    reported with the frame's errors) → the panels of mods whose scripts were switched off show their
//!    placeholder (`Runtime::set_panel_failed`) → `Runtime::frame` → the text bridge (the frame's request for the
//!    focused field, then the field's state the player typed, `Runtime::set_text_input`) → `Scene::resolve`;
//! 6. `ScriptSupervisor::post` with the frame's delta, the markup's script events and a changed environment (the
//!    supervisor sends nothing to a context with nothing new).
//!
//! `reconcile` runs in every frame, so every frame that took events settles what they decided. A script's state
//! write reaches the model in the frame that drains it, and its worker's mirror with that frame's post. The mods'
//! scripts run under a supervisor of their own ([`crate::mods`]): their answers join the skin's in step 2 and they
//! get the frame after the skin's in step 6; a skin built anew gets the mods' panels again before its first frame.
//! The map anchors the runtime reported are kept; the application
//! places them with its camera after the frame (`V2Ui::place_anchors`).

use std::sync::Arc;

use replica::ReplicaStep;
use ui2_actions::ActionEvent;
use ui2_draw::SceneStats;
use ui2_model::Owner;
use ui2_model::wire::ScriptCtxId;
use ui2_model::wire::WireIntent;
use ui2_package::style::Cursor;
use ui2_runtime::{FrameStats, Runtime, UiError};
use ui2_script::{FrameInput, ScriptStats};
use ui2_source::{ReplicaView, SourceFrame, SourceProfile, SourceStats};

use crate::error::{LoadFailure, SkinLoadError};
use crate::scripts;
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
    /// Where the source's work went: per root and per kind of read (µs with the host's clock).
    pub source_profile: SourceProfile,
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
    /// Output dropped because nothing consumes it (script events without scripts).
    pub dropped_output: u32,
    /// The skin's scripts after the frame; `None` when none run.
    pub scripts: Option<ScriptStats>,
    /// Main-thread time of the scripts' steps (drain and post), ms (0 without a clock or scripts).
    pub js_main_ms: f32,
    /// The skin's scripts failed while loading: the skin cannot be used. The application falls back as for any
    /// load failure; the scripts are already stopped.
    pub fatal: Option<SkinLoadError>,
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
        [
            ("ui2.cpu_ms", f64::from(self.cpu_ms)),
            ("ui2.source_ms", f64::from(self.source_ms)),
            ("ui2.runtime_ms", f64::from(r.cpu_ms)),
            ("ui2.resolve_ms", f64::from(self.resolve_ms)),
            ("ui2.source.roots", f64::from(self.source.roots_built)),
            ("ui2.source.reads", f64::from(self.source.getter_calls)),
            ("ui2.source.units", f64::from(self.source.spent_units)),
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
        .into_iter()
        .chain(self.source_profile.counters())
        .chain(
            self.scripts
                .clone()
                .unwrap_or_default()
                .counters(f64::from(self.js_main_ms)),
        )
        .collect()
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
        // A skin built anew gets the mods' panels again.
        self.keep_mod_panels();
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
            pkg,
            runtime,
            text,
            scene,
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
            self.mods.table.clone_from(&ids);
            self.actions.set_mods(ids);
        }

        // 2. What the scripts answered and the markup asked since the last frame.
        let t_drain = now();
        let mut js = self.scripts.as_mut().map(|s| s.sup.drain(frame_ms));
        self.mods.drain_scripts(frame_ms, &mut js);
        let fatal = self
            .scripts
            .as_mut()
            .and_then(|s| s.sup.take_fatal())
            .map(|(_, why)| {
                SkinLoadError::new(pkg.manifest().id.as_str(), LoadFailure::Scripts(why))
            });
        let mut js_main_ms = (now() - t_drain) as f32;
        let out = runtime.drain_output();

        // 3. State writes (the markup's, then the scripts'), the scripts' settings, then intents; orders leave
        //    through the sink.
        let mut refused = 0;
        // A mod panel's writes (`modstate.<id>.*`) go to that mod's state.
        for e in
            crate::mods::markup_state_ops(&mut self.actions, &out.state_writes, &mut self.store)
        {
            tracing::warn!(target: "ui2_host", "state write refused: {e}");
            refused += 1;
        }
        let mut intents: Vec<WireIntent> = out.intents;
        // A mod panel's `mods.<id>.*` intents carry the mod's origin.
        self.mods.tag_markup(&mut intents);
        if let Some(js) = js.as_mut() {
            let skin = pkg.manifest().id.as_str();
            refused += scripts::apply_writes(js, &mut self.actions, &mut self.store, skin).refused;
            intents.extend(scripts::take_intents(js));
        }
        intents.append(&mut self.queued);
        let applied = intents.len() as u32;
        self.actions
            .apply(&mut intents, &mut self.store, net.sink(), now_ms);
        // The actions module logs each form error once; the list only needs emptying.
        refused += self.actions.take_form_errors().len() as u32;
        self.mods.keep_intents(self.actions.take_mod_intents());
        let script_events = out.script_events;
        let mut dropped_output = 0;
        if self.scripts.is_none() && !self.mods.has_scripts() {
            dropped_output += script_events.len();
        }

        // 4. The mods' Rust halves' writes, the source, then the actions module judges its state by what the source
        //    wrote, then the delta.
        refused += self.mods.apply_writes(&mut self.store);
        let t_source = now();
        let source = {
            let mut w = self.store.writer(Owner::Source);
            match self.sample.as_mut() {
                // The sample mode: the sample's roots, once; nothing of a game is read.
                Some(sample) => sample.write(&mut w),
                None => self.source.frame(
                    SourceFrame {
                        replica,
                        steps,
                        session: net.session(),
                        dismissed: self.actions.dismissed(),
                        now_ms,
                    },
                    &mut w,
                    self.actions.ui(),
                ),
            }
        };
        let source_ms = (now() - t_source) as f32;
        // A sample is a fixed picture: its windows and focus are not judged against a game.
        if self.sample.is_none() {
            self.actions
                .reconcile(&mut self.store, self.source.notification_ids(), now_ms);
        }

        // 5. The runtime and the glyphs.
        let delta = self.store.take_frame();
        let delta_ops = delta.as_ref().map_or(0, |d| d.ops.len() as u32);
        if let Some(d) = &delta {
            runtime.apply_delta(d, &self.store);
        }
        // The scripts' island commands, then the panels of the mods whose scripts were switched off.
        let mut island_errors = Vec::new();
        if let Some(js) = &js {
            for (ctx, cmd) in &js.island_cmds {
                for e in runtime.apply_island(ctx, std::slice::from_ref(cmd)) {
                    tracing::warn!(target: "ui2_host", "{e}");
                    island_errors.push(e);
                }
            }
            switch_off_mod_panels(runtime, &js.failed);
        }
        if dropped_output > 0 {
            tracing::debug!(target: "ui2_host", dropped_output, "script events have no consumer");
        }
        let mut out = runtime.frame(frame_ms, &self.store, text, scene);
        // The text bridge: what the runtime asked of the focused field, then what the player typed since.
        if let Some(bridge) = self.text_bridge.as_mut() {
            if let Some(req) = &out.text_input {
                bridge.request(req);
            }
            if let Some(state) = bridge.poll() {
                runtime.set_text_input(state);
            }
        }
        out.errors.splice(0..0, island_errors);
        // The map anchors and their objects, for the application to place (`V2Ui::place_anchors`).
        for (id, kind, key) in &out.anchor_changes {
            if *key == ui2_model::Value::None {
                self.anchors.remove(id);
            } else {
                self.anchors.insert(*id, (*kind, key.clone()));
            }
        }
        for e in &out.errors {
            tracing::warn!(target: "ui2_host", "{e}");
        }

        let t_resolve = now();
        scene.resolve(text, RESOLVE_BUDGET_MS);
        let resolve_ms = (now() - t_resolve) as f32;

        // 6. The scripts get the frame: the delta they watch, their events, a new environment.
        if fatal.is_some() {
            // The skin cannot be used: its workers go now, the application falls back.
            self.scripts = None;
        }
        if let Some(s) = self.scripts.as_mut() {
            let t_post = now();
            let env = s.env_change(runtime);
            s.sup.post(
                frame_ms,
                &FrameInput {
                    delta: delta.as_ref(),
                    events: &script_events,
                    env: env.as_ref(),
                    store: Some(&self.store),
                },
            );
            js_main_ms += (now() - t_post) as f32;
        }
        self.mods.post_scripts(
            frame_ms,
            runtime,
            delta.as_ref(),
            &script_events,
            &self.store,
        );
        self.frames += 1;

        V2FrameReport {
            source,
            source_profile: if self.sample.is_none() {
                self.source.profile().clone()
            } else {
                SourceProfile::default()
            },
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
            dropped_output: dropped_output as u32,
            scripts: self.scripts.as_ref().map(|s| s.sup.stats()),
            js_main_ms: if self.scripts.is_some() {
                js_main_ms
            } else {
                0.0
            },
            fatal,
            source_ms,
            resolve_ms,
            cpu_ms: (now() - t0) as f32,
        }
    }
}

/// The panels of each mod whose scripts were switched off show their placeholder (the mod's markup cannot work
/// without its scripts), and one in place of a skin's element gives the element back; the skin's own panels and
/// other mods' panels live on. Returns the panels switched off.
pub(crate) fn switch_off_mod_panels(runtime: &mut Runtime, failed: &[ScriptCtxId]) -> usize {
    let mut off = 0;
    for ctx in failed {
        let ScriptCtxId::Mod(m) = ctx else {
            tracing::warn!(
                target: "ui2_host",
                "scripts of {} are switched off; the skin works on markup alone",
                ui2_script::ctx_name(ctx)
            );
            continue;
        };
        let reason = format!("the scripts of mod {m} were switched off");
        tracing::warn!(target: "ui2_host", "{reason}; its panels show a placeholder");
        for p in runtime.panels() {
            if p.owner.as_deref() != Some(&**m) || p.failed.is_some() {
                continue;
            }
            // A panel in place of a skin's element gives the element back; the others show a placeholder.
            if p.mode == Some(ui2_runtime::MountMode::Replace) {
                runtime.unmount(p.id);
            } else {
                runtime.set_panel_failed(p.id, &reason);
            }
            off += 1;
        }
    }
    off
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ui2_draw::Scene;
    use ui2_model::wire::ScriptCtxId;
    use ui2_model::{ModelStore, Schema};
    use ui2_package::style::{Display, Len, StyleRecord};
    use ui2_package::{Layer, Package};
    use ui2_runtime::testkit::{Kit, TNode};
    use ui2_runtime::{Env, MountMode, Runtime, Services};
    use ui2_text::TextEngine;

    use super::switch_off_mod_panels;

    fn block(kit: &mut Kit) -> ui2_package::StyleSetId {
        let mut r = StyleRecord::initial();
        r.display = Display::Block;
        r.height = ui2_package::style::LenAuto::Len(Len::px(20.0));
        let r = kit.record(r);
        kit.plain(r)
    }

    /// A HUD layer with the extension point `mods`.
    fn skin() -> Arc<Package> {
        let mut kit = Kit::new();
        let set = block(&mut kit);
        let part = kit.str("right");
        kit.template(
            "doc",
            vec![TNode::layer(Layer::Hud, set).child(TNode::el(set).part(part).slot("mods"))],
        );
        kit.finish().expect("skin")
    }

    /// A fragment of mod `id` with one empty panel `panel`.
    fn fragment(id: &str, panel: &str) -> Arc<Package> {
        let mut kit = Kit::new();
        let set = block(&mut kit);
        let part = kit.str(panel);
        kit.template("doc", vec![TNode::el(set).part(part)]);
        kit.finish_fragment(id, &["mods"]).expect("fragment")
    }

    #[test]
    fn a_mod_whose_scripts_stop_gets_placeholders_and_the_others_live_on() {
        let mut rt = Runtime::new(skin(), Env::default(), Services::default()).expect("runtime");
        let a = rt
            .mount_fragment(fragment("alpha", "a1"), "a1", "mods", MountMode::Append, 0)
            .expect("a1");
        let b = rt
            .mount_fragment(fragment("beta", "b1"), "b1", "mods", MountMode::Append, 1)
            .expect("b1");
        let (store, mut text, mut scene) = (
            ModelStore::new(Schema::v1()),
            TextEngine::new(),
            Scene::new(1.0),
        );
        rt.frame(16.0, &store, &mut text, &mut scene);
        let failed = [
            ScriptCtxId::Skin,
            ScriptCtxId::Mod(Arc::from("alpha")),
            ScriptCtxId::Mod(Arc::from("gone")),
        ];
        assert_eq!(switch_off_mod_panels(&mut rt, &failed), 1);
        let panels = rt.panels();
        let state = |id| {
            panels
                .iter()
                .find(|p| p.id == id)
                .expect("panel")
                .failed
                .clone()
        };
        assert!(
            state(a).is_some_and(|why| why.contains("alpha")),
            "{panels:?}"
        );
        assert_eq!(state(b), None, "another mod's panel lives on");
        assert!(
            panels
                .iter()
                .filter(|p| p.owner.is_none())
                .all(|p| p.failed.is_none()),
            "the skin's panels live on"
        );
        // Once: a panel already switched off is not switched off again.
        assert_eq!(switch_off_mod_panels(&mut rt, &failed), 0);
    }
}
