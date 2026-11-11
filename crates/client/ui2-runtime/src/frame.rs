//! The frame: the passes in order, the output for the host, and what markup asks of the rest of the UI.
//!
//! The host calls, once per frame: [`Runtime::drain_output`] (intents, state writes and script events
//! collected since the last frame), [`Runtime::apply_delta`] when the model store had a frame (it only queues
//! the bindings that read a changed field), then [`Runtime::frame`]. Inside `frame` every pass consumes its
//! own queue and nothing else:
//!
//! 1. bindings: rows of new nodes, rows queued by deltas or a new localizer; their effects queue texts,
//!    style selection, overrides;
//! 2. styles: environment changes (viewport cell, theme, scale), then the queued nodes;
//! 3. text: queued text leaves and inline roots are shaped again (unchanged runs are kept); a root whose
//!    content takes another room is queued for layout, any other only for paint;
//! 4. layout: the roots above queued nodes are laid out; boxes that moved are queued for paint; virtual
//!    lists move their windows, `data-fit` texts shrink to their boxes;
//! 5. transitions: records that changed start them, running ones queue their chunks for paint;
//! 6. paint: removed chunks leave the scene, chunks with a queued node are built and set on the scene (whose
//!    device scale follows the environment).
//!
//! A frame without changes finds every queue empty: no evaluation, no shaping, no layout, no draw list, and
//! the scene is not touched.

use ui2_draw::Scene;
use ui2_model::ModelStore;
use ui2_model::wire::{ScriptEvent, StateOp, WireIntent};
use ui2_package::style::Cursor;

use crate::error::UiError;
use crate::stats::{FrameStats, Stopwatch};
use crate::{Runtime, TextMeasure, bind, layout, paint, style, text};

/// What markup asked of the rest of the UI since the last frame: intents for the actions module, writes to
/// the skin's state, events for scripts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UiOutput {
    /// Intents (`data-on-*="intent ..."`, hotkeys).
    pub intents: Vec<WireIntent>,
    /// Writes to `state.*` and to the `js` fields of `ui` (`set`, `toggle`, controls).
    pub state_writes: Vec<StateOp>,
    /// Events for the skin's or a mod's scripts (`call`, `data-event`).
    pub script_events: Vec<ScriptEvent>,
}

impl UiOutput {
    /// Whether nothing was asked (the host then has nothing to forward).
    pub fn is_empty(&self) -> bool {
        self.intents.is_empty() && self.state_writes.is_empty() && self.script_events.is_empty()
    }
}

/// The result of a frame.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameOutput {
    /// The pointer cursor the UI wants (`auto`: the world decides).
    pub cursor: Cursor,
    /// Errors reported for the first time during this frame (each is logged once).
    pub errors: Vec<UiError>,
    /// Transitions are running: the host should keep producing frames.
    pub animating: bool,
    /// Work counters.
    pub stats: FrameStats,
}

impl Runtime {
    /// Runs one frame (see the [module docs](crate::frame) for the order of the passes) and hands the changed
    /// draw lists to the scene. `now_ms` is the frame clock (it drives transitions); `m` is the model store
    /// the last [`apply_delta`](Self::apply_delta) came from.
    pub fn frame(
        &mut self,
        now_ms: f64,
        m: &ModelStore,
        text: &mut dyn TextMeasure,
        scene: &mut Scene,
    ) -> FrameOutput {
        // The clock is lent to the stopwatch for the frame (the passes borrow the runtime).
        let clock = self.clock.take();
        let mut watch = Stopwatch::new(clock.as_deref());
        let bctx = bind::Ctx {
            pkg: &self.pkg,
            index: &self.style_index,
            store: m,
            env: &self.style_env,
        };
        let mut binds = self.binder.pass(&mut self.arena, &bctx, &mut self.errors);
        let mut bind_ms = watch.lap();
        let change = std::mem::take(&mut self.env_change);
        let mut restyled = Vec::new();
        let mut styles = style::select::pass(
            &mut self.arena,
            self.pkg.styles(),
            &self.style_index,
            &self.style_env,
            change,
            &mut restyled,
        );
        let mut style_ms = watch.lap();
        let ctx = text::TextCtx {
            pkg: &self.pkg,
            fonts: &self.fonts,
            index: &self.style_index,
        };
        let mut texts = text::pass(
            &ctx,
            &mut self.arena,
            text,
            &mut self.errors,
            &mut self.fit_roots,
        );
        let mut text_ms = watch.lap();
        let mut layout = layout::pass(
            &mut self.arena,
            &self.layout_styles,
            text,
            self.style_env.scale.viewport_css,
        );
        // A new tooltip is placed once its size is known, and laid out again where it goes.
        if self.place_tooltip() {
            let again = layout::pass(
                &mut self.arena,
                &self.layout_styles,
                text,
                self.style_env.scale.viewport_css,
            );
            layout.runs += again.runs;
            layout.visited += again.visited;
        }
        let mut layout_ms = watch.lap();
        // Virtual lists show what their scroll containers show once these are laid out: rows that entered
        // the window are bound, styled, shaped and laid out in the same frame.
        if layout.runs > 0 {
            self.binder.windows_changed();
        }
        for _ in 0..3 {
            let bctx = bind::Ctx {
                pkg: &self.pkg,
                index: &self.style_index,
                store: m,
                env: &self.style_env,
            };
            if !self
                .binder
                .update_windows(&mut self.arena, &bctx, &mut self.errors)
            {
                break;
            }
            let b = self.binder.pass(&mut self.arena, &bctx, &mut self.errors);
            binds.evaluated += b.evaluated;
            binds.visited += b.visited;
            bind_ms += watch.lap();
            let s = style::select::pass(
                &mut self.arena,
                self.pkg.styles(),
                &self.style_index,
                &self.style_env,
                style::select::EnvChange::default(),
                // Rows a virtual list hands to other items take their records without transitions.
                &mut Vec::new(),
            );
            styles.restyled += s.restyled;
            styles.visited += s.visited;
            style_ms += watch.lap();
            let ctx = text::TextCtx {
                pkg: &self.pkg,
                fonts: &self.fonts,
                index: &self.style_index,
            };
            let t = text::pass(
                &ctx,
                &mut self.arena,
                text,
                &mut self.errors,
                &mut self.fit_roots,
            );
            texts.shaped += t.shaped;
            texts.visited += t.visited;
            text_ms += watch.lap();
            let l = layout::pass(
                &mut self.arena,
                &self.layout_styles,
                text,
                self.style_env.scale.viewport_css,
            );
            layout.runs += l.runs;
            layout.visited += l.visited;
            layout_ms += watch.lap();
            if l.runs > 0 {
                self.binder.windows_changed();
            }
        }
        // Texts that fit their box are shrunk once the box is known (only after work: an idle frame skips).
        if !self.fit_roots.is_empty() && (layout.runs > 0 || texts.visited > 0) {
            let f = text::fit_pass(&mut self.arena, text, &mut self.fit_roots);
            texts.shaped += f.shaped;
            texts.visited += f.visited;
            text_ms += watch.lap();
        }
        // Transitions: records that changed start them, running ones move to the frame time (paint only).
        let acx = crate::anim::AnimCtx {
            pkg: &self.pkg,
            index: &self.style_index,
            theme: self.style_env.theme,
        };
        if !restyled.is_empty() {
            self.anim.start(now_ms, &self.arena, &acx, restyled);
        }
        let animating = self.anim.tick(now_ms, &mut self.arena, &acx);
        if scene.scale() != self.style_env.scale.k {
            scene.set_scale(self.style_env.scale.k);
        }
        let paint = paint::pass(
            &mut self.arena,
            &paint::PaintCtx {
                pkg: &self.pkg,
                index: &self.style_index,
                env: &self.style_env,
                anim: &self.anim,
            },
            scene,
            &mut self.errors,
        );
        let paint_ms = watch.lap();
        let cpu_ms = watch.total();
        self.clock = clock;
        FrameOutput {
            cursor: self.cursor(),
            errors: self.errors.take(),
            animating,
            stats: FrameStats {
                cpu_ms,
                style_ms,
                layout_ms,
                text_ms,
                bind_ms,
                paint_ms,
                restyled: styles.restyled,
                layout_runs: layout.runs,
                shaped_runs: texts.shaped,
                binds_evaluated: binds.evaluated,
                chunks_rebuilt: paint.chunks,
                nodes: self.arena.len() as u32,
                visited: binds.visited
                    + styles.visited
                    + texts.visited
                    + layout.visited
                    + paint.visited,
            },
        }
    }

    /// What markup asked since the last call: intents, state writes and script events of the input handled
    /// since (empty when there was none: the host then forwards nothing).
    pub fn drain_output(&mut self) -> UiOutput {
        std::mem::take(&mut self.output)
    }

    /// Sets the clock the frame times its passes with (milliseconds, monotonic). Without one the times of
    /// [`FrameStats`] are zero; the runtime reads no time source of its own.
    pub fn set_clock(&mut self, clock: ui2_draw::Clock) {
        self.clock = Some(clock);
    }
}
