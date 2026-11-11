//! Text fields (`<input type="text">`): the runtime draws them, the platform edits them.
//!
//! While a text field has the focus, the host's text bridge ([`platform::ui2::TextInputBridge`], a hidden
//! `<textarea>` on the web) edits its text: typing, paste, undo and input methods are the browser's. The
//! runtime asks for the bridge with [`TextInputRequest`]s in [`FrameOutput::text_input`](crate::FrameOutput)
//! (`Focus` when a field takes the focus, again when the model changes the text of the focused field or a
//! press moves the caret; `Blur` when it loses it), and takes the bridge's [`TextInputState`] with
//! [`Runtime::set_text_input`]:
//!
//! - a new value is written to the field's `data-bind-model` path (a [`StateOp`], like the controls) and
//!   runs `input`; the field's text is shaped again (no layout: a field's box does not depend on its text);
//! - a new selection or composition repaints the field;
//! - `submitted` (Enter) runs `submit`, `cancelled` (Escape) takes the focus away, `tab` moves the focus;
//! - when the focus leaves a field whose text changed since it took it, `change` runs.
//!
//! The field shows, on one line in its content box (centered vertically, scrolled so that the caret stays
//! inside), the text being edited while it has the focus, else its model value (a field without a model
//! keeps its own text), else its placeholder (`data-l10n-placeholder`, a skin text) in its color at half
//! opacity. Over the text: the selection, the composition (underlined) and the caret, which blinks every
//! [`CARET_BLINK_MS`] (only its chunk is repainted then; a frozen scene gives the time, so the caret holds
//! still in reference images).

use std::collections::HashMap;
use std::sync::Arc;

use glam::Vec2;
use platform::ui2::{TextInputRequest, TextInputState, TextSel};
use ui2_model::Value as MValue;
use ui2_model::wire::StateOp;
use ui2_package::profile::action::Event;
use ui2_package::{BindKind, NodeKind};
use ui2_text::{ShapedRun, TextMeasure};

use crate::Runtime;
use crate::arena::{Dirty, NodeId};
use crate::error::{ErrorCode, UiError};
use crate::instantiate;
use crate::text::{TextCtx, VMetrics};

/// The caret is shown, then hidden, for this long each, ms.
pub const CARET_BLINK_MS: f64 = 530.0;

/// What a field shows, shaped.
#[derive(Debug, Clone)]
pub(crate) struct FieldView {
    /// The shown text (`None` when it is empty).
    pub run: Option<ShapedRun>,
    /// The vertical metrics of the field's font and line height.
    pub vm: VMetrics,
    /// The text is the placeholder.
    pub placeholder: bool,
}

/// The focused field.
#[derive(Debug, Clone)]
pub(crate) struct Edit {
    pub node: NodeId,
    /// The text as the bridge has it.
    pub value: String,
    pub sel: TextSel,
    pub composing: Option<(u32, u32)>,
    /// The text when the field took the focus (`change` on blur when it differs).
    initial: String,
    /// The last value written to the model.
    written: Option<String>,
    /// When the caret started blinking (`None`: at the next frame).
    blink_from: Option<f64>,
    /// The caret is drawn.
    pub caret_on: bool,
}

/// The text fields of a runtime.
#[derive(Debug, Default)]
pub(crate) struct Fields {
    views: HashMap<NodeId, FieldView>,
    /// The text of fields without a model.
    own: HashMap<NodeId, String>,
    edit: Option<Edit>,
    /// The request for the host's bridge since the last frame (the last one wins).
    request: Option<TextInputRequest>,
}

impl Fields {
    /// What a field shows.
    pub fn view(&self, id: NodeId) -> Option<&FieldView> {
        self.views.get(&id)
    }

    /// The edit state when the field has the focus.
    pub fn edit_of(&self, id: NodeId) -> Option<&Edit> {
        self.edit.as_ref().filter(|e| e.node == id)
    }

    /// The request for the bridge, once.
    pub fn take_request(&mut self) -> Option<TextInputRequest> {
        self.request.take()
    }
}

/// The x of a byte offset in a shaped run (cluster boundaries), CSS px.
pub(crate) fn offset_x(run: Option<&ShapedRun>, at: u32) -> f32 {
    match run {
        Some(r) if at > 0 => r.range_width(0, at as usize),
        _ => 0.0,
    }
}

/// The horizontal scroll of a field's text: the caret stays inside the content box.
pub(crate) fn scroll_for(caret_x: f32, content_w: f32) -> f32 {
    (caret_x - content_w + 1.0).max(0.0)
}

/// The byte offset nearest to `x` (CSS px from the start of the text).
fn offset_at(run: Option<&ShapedRun>, x: f32) -> u32 {
    let Some(r) = run else {
        return 0;
    };
    let mut points: Vec<usize> = r.cluster_starts();
    points.push(r.text.len());
    let mut best = (0u32, f32::INFINITY);
    for b in points {
        let d = (offset_x(Some(r), b as u32) - x).abs();
        if d < best.1 {
            best = (b as u32, d);
        }
    }
    best.0
}

impl Runtime {
    /// Whether a node is a text field.
    pub(crate) fn is_text_field(&self, id: NodeId) -> bool {
        self.arena
            .get(id)
            .and_then(|n| instantiate::template_node(&*self.pkg, n.tmpl()))
            .is_some_and(|t| matches!(t.kind, NodeKind::Input { .. }))
    }

    /// The `state.*` path of a field's `data-bind-model`.
    fn field_path(&self, id: NodeId) -> Option<Arc<str>> {
        let tmpl = self.arena.get(id)?.tmpl();
        self.pkg
            .bindings_of(tmpl.0, tmpl.1)
            .iter()
            .find_map(|b| match b.kind {
                BindKind::Model(p) => self.pkg.str(p).map(|s| Arc::from(format!("state.{s}"))),
                _ => None,
            })
    }

    /// The model's text of a field (as its row last read it), `None` without a model.
    fn model_text(&self, id: NodeId) -> Option<String> {
        self.field_path(id)?;
        let n = self.arena.get(id)?;
        Some(self.binder.model_values(n.binds.clone()).map_or(
            String::new(),
            |(raw, text)| match raw {
                MValue::None => String::new(),
                _ => text.to_string(),
            },
        ))
    }

    /// The text a field holds when it does not have the focus.
    fn resting_text(&self, id: NodeId) -> String {
        self.model_text(id)
            .or_else(|| self.input.fields.own.get(&id).cloned())
            .unwrap_or_default()
    }

    /// A field's border box on the screen, physical px.
    fn field_rect(&self, id: NodeId) -> [f32; 4] {
        let mut g = self.geo();
        let (p, s) = (g.abs(id), g.size(id));
        let k = self.style_env.scale.k;
        [p.x * k, p.y * k, s.x * k, s.y * k]
    }

    fn focus_request(&mut self, e: &Edit) {
        let rect = self.field_rect(e.node);
        self.input.fields.request = Some(TextInputRequest::Focus {
            value: e.value.clone(),
            sel: e.sel,
            rect,
        });
    }

    /// The focus moved from `old` to `new`: a field that lost it stops editing (`change` when its text
    /// changed), a field that got it starts, with the caret at the end of its text.
    pub(crate) fn field_focus_changed(&mut self, old: Option<NodeId>, new: Option<NodeId>) {
        if let Some(e) = self.input.fields.edit.take() {
            if old == Some(e.node) || new != Some(e.node) {
                if self.field_path(e.node).is_none() {
                    self.input.fields.own.insert(e.node, e.value.clone());
                }
                self.input.fields.request = Some(TextInputRequest::Blur);
                self.arena.mark(e.node, Dirty::TEXT);
                if e.value != e.initial {
                    self.dispatch(e.node, Event::Change, false);
                }
            } else {
                self.input.fields.edit = Some(e);
            }
        }
        let Some(n) = new.filter(|&n| self.is_text_field(n)) else {
            return;
        };
        if self.input.fields.edit.is_some() {
            return;
        }
        let value = self.resting_text(n);
        let e = Edit {
            node: n,
            sel: TextSel::caret(value.len() as u32),
            composing: None,
            initial: value.clone(),
            written: None,
            value,
            blink_from: None,
            caret_on: true,
        };
        self.focus_request(&e);
        self.input.fields.edit = Some(e);
        self.arena.mark(n, Dirty::TEXT);
    }

    /// A left press on a node: on the focused field, the caret goes to the pressed place.
    pub(crate) fn field_press(&mut self, node: NodeId, local: Vec2) {
        let Some(e) = self.input.fields.edit.clone().filter(|e| e.node == node) else {
            return;
        };
        let Some(view) = self.input.fields.views.get(&node) else {
            return;
        };
        let Some(l) = self.arena.get(node).map(|n| n.layout().unrounded) else {
            return;
        };
        let content_w =
            l.size.width - l.border.left - l.border.right - l.padding.left - l.padding.right;
        let run = if view.placeholder {
            None
        } else {
            view.run.as_ref()
        };
        let scroll = scroll_for(offset_x(run, e.sel.caret), content_w);
        let x = local.x - l.border.left - l.padding.left + scroll;
        let at = offset_at(run, x);
        let mut e = e;
        e.sel = TextSel::caret(at);
        e.composing = None;
        e.blink_from = None;
        self.focus_request(&e);
        self.input.fields.edit = Some(e);
        self.arena.mark(node, Dirty::PAINT);
    }

    /// Takes the state of the host's text bridge (see the [module docs](self)). Without a focused field it
    /// is ignored.
    pub fn set_text_input(&mut self, s: TextInputState) {
        let Some(mut e) = self.input.fields.edit.clone() else {
            return;
        };
        if !self.arena.contains(e.node) {
            return;
        }
        let node = e.node;
        let sel = s.sel.clamped(&s.value);
        let composing = s.composing.and_then(|(a, b)| {
            let c = TextSel {
                anchor: a,
                caret: b,
            }
            .clamped(&s.value)
            .range();
            (c.0 < c.1).then_some(c)
        });
        let new_value = s.value != e.value;
        let moved = sel != e.sel || composing != e.composing;
        if new_value || moved {
            e.blink_from = None;
        }
        e.sel = sel;
        e.composing = composing;
        if new_value {
            e.value = s.value;
            if let Some(path) = self.field_path(node) {
                e.written = Some(e.value.clone());
                self.output.state_writes.push(StateOp {
                    path,
                    value: MValue::Str(e.value.as_str().into()),
                });
            }
        }
        self.input.fields.edit = Some(e);
        if new_value {
            self.arena.mark(node, Dirty::TEXT);
            self.dispatch(node, Event::Input, false);
        } else if moved {
            self.arena.mark(node, Dirty::PAINT);
        }
        if s.submitted {
            self.dispatch(node, Event::Submit, false);
        }
        if let Some(back) = s.tab {
            self.tab(back);
        }
        if s.cancelled && self.focused() == Some(node) {
            self.set_focus(None, false);
        }
    }

    /// Shapes what the queued fields show (a frame pass, after the text pass). The focused field takes a
    /// text the model got from elsewhere (not its own write) and asks the bridge to edit that. Returns the
    /// runs shaped and the fields looked at.
    pub(crate) fn field_pass(
        &mut self,
        tm: &mut dyn TextMeasure,
        queue: Vec<NodeId>,
    ) -> (u32, u32) {
        let (mut shaped, mut visited) = (0, 0);
        // The focused field went away with its node.
        if self
            .input
            .fields
            .edit
            .as_ref()
            .is_some_and(|e| !self.arena.contains(e.node))
        {
            self.input.fields.edit = None;
            self.input.fields.request = Some(TextInputRequest::Blur);
        }
        if queue.is_empty() {
            return (0, 0);
        }
        let arena = &self.arena;
        self.input.fields.views.retain(|id, _| arena.contains(*id));
        self.input.fields.own.retain(|id, _| arena.contains(*id));
        for id in queue {
            visited += 1;
            if !self.arena.contains(id) {
                continue;
            }
            // The model changed the focused field's text (not by its own write): edit the new text.
            if let Some(mut e) = self.input.fields.edit.clone().filter(|e| e.node == id)
                && let Some(model) = self.model_text(id)
                && model != e.value
                && e.written.as_deref() != Some(model.as_str())
            {
                e.sel = TextSel::caret(model.len() as u32);
                e.value = model;
                e.composing = None;
                e.written = None;
                e.blink_from = None;
                self.focus_request(&e);
                self.input.fields.edit = Some(e);
            }
            let text = match self.input.fields.edit_of(id) {
                Some(e) => e.value.clone(),
                None => self.resting_text(id),
            };
            let (text, placeholder) = if text.is_empty() {
                (self.placeholder(id), true)
            } else {
                (text, false)
            };
            let Some(rec) = self.style(id).cloned() else {
                continue;
            };
            let ctx = TextCtx {
                pkg: &self.pkg,
                fonts: &self.fonts,
                index: &self.style_index,
            };
            let Some((_, style, vm)) = ctx.vmetrics(&*tm, &rec) else {
                // An empty field needs no font (its caret is then not drawn).
                if text.is_empty() {
                    self.input.fields.views.insert(
                        id,
                        FieldView {
                            run: None,
                            vm: VMetrics::ZERO,
                            placeholder,
                        },
                    );
                    continue;
                }
                let path = self.path(id);
                self.errors.report(UiError::at(
                    &path,
                    ErrorCode::Font,
                    "no font face of the package matches",
                ));
                continue;
            };
            let kept = self
                .input
                .fields
                .views
                .get(&id)
                .and_then(|v| v.run.as_ref())
                .filter(|r| r.style == style && *r.text == *text)
                .cloned();
            let run = if text.is_empty() {
                None
            } else if kept.is_some() {
                kept
            } else {
                shaped += 1;
                match tm.shape(&style, &text) {
                    Ok(r) => Some(r),
                    Err(err) => {
                        let path = self.path(id);
                        self.errors.report(UiError::at(
                            &path,
                            ErrorCode::Font,
                            format!("text cannot be shaped: {err}"),
                        ));
                        None
                    }
                }
            };
            self.input.fields.views.insert(
                id,
                FieldView {
                    run,
                    vm,
                    placeholder,
                },
            );
            self.arena.mark(id, Dirty::PAINT);
        }
        (shaped, visited)
    }

    /// The placeholder of a field (its skin text), empty without one.
    fn placeholder(&self, id: NodeId) -> String {
        let key = self
            .arena
            .get(id)
            .and_then(|n| instantiate::template_node(&*self.pkg, n.tmpl()))
            .and_then(|t| match t.kind {
                NodeKind::Input { placeholder } => placeholder,
                _ => None,
            })
            .and_then(|k| self.pkg.str(k));
        match key {
            Some(k) => self
                .binder
                .loc
                .skin_text(&self.pkg, k, &[])
                .unwrap_or_else(|k| k),
            None => String::new(),
        }
    }

    /// Moves the caret's blink to the time `t` (a frame step): the focused field's chunk is repainted when the
    /// caret shows or hides.
    pub(crate) fn caret_tick(&mut self, t: f64) {
        let Some(e) = self.input.fields.edit.as_mut() else {
            return;
        };
        let from = *e.blink_from.get_or_insert(t);
        let on = t < from || ((t - from) / CARET_BLINK_MS).floor() as i64 % 2 == 0;
        if on != e.caret_on {
            e.caret_on = on;
            let node = e.node;
            self.arena.mark(node, Dirty::PAINT);
        }
    }
}
