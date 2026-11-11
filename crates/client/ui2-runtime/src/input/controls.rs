//! Controls: the behaviour of `data-control` nodes. A control edits the skin state path of its
//! `data-bind-model` (`state.<path>`); every change is a [`StateOp`] in the runtime's output (the actions
//! module applies it before the next frame), followed by `input` and `change` events on the control. Their
//! look is ordinary elements: class toggles on the model value, `:focus` and the like. Buttons (`<button>`,
//! `tabindex`) need nothing here: a press, Enter or Space clicks them (see [`hotkeys`](super::hotkeys)).
//!
//! - **checkbox, toggle**: a click (or Enter, Space) writes the negation of the value. `CHECKED` is set
//!   while the value is true.
//! - **radio**: a click writes its `data-value` (typed as the model path is: a number, a boolean or text);
//!   `CHECKED` while the value is its `data-value`. The arrow keys move the focus to the next or previous
//!   radio of the same path (document order, wrapping) and select it.
//! - **slider** (`data-min`, `data-max`, `data-step`): a press sets the value at the pointer's place across
//!   the slider's box and a drag follows the pointer (an `input` event per change, `change` on release); the
//!   arrow keys step it, Page Up and Page Down by ten steps, Home and End go to the ends. Values snap to the
//!   steps from the minimum and stay in the range; an integer path gets integers.
//! - **select** (`data-options`): the options are the instances of the `for` inside the select (the one over
//!   the same list as `data-options`, else the first); an option's value is its item's key (as text when the
//!   path holds text). The list is open while the select has the focus, so the skin shows it with `:focus`
//!   on the select. A click on an option writes its value and closes the list (the focus leaves); a click
//!   on the rest of the select opens it, or closes it when it was open; Esc closes it. The arrow keys pick
//!   the next or previous option (no wrapping), Home and End the first and the last.
//!   SIMPLIFIED(SIMP-202): the select is the skin's own list (a `for` template inside it), without the
//!   native one's search by typed letters.
//!
//! Until the next frame the model has not taken a write yet: a control reads its own last write of a path
//! (two clicks on a checkbox between frames write `true`, then `false`).

use std::sync::Arc;

use glam::Vec2;
use platform::{KeyEvent, NamedKey};
use ui2_model::wire::StateOp;
use ui2_model::{Key, Scalar, Type, Value as MValue};
use ui2_package::profile::action::{ControlKind, Event};
use ui2_package::profile::expr::Value;
use ui2_package::{BindKind, ExprId, NodeKind, NodeRef, Package, StrId};

use crate::Runtime;
use crate::arena::{NodeClass, NodeId};
use crate::bind::to_model;
use crate::input::InputOutcome;
use crate::instantiate;

/// The control state of the input.
#[derive(Debug, Default)]
pub(crate) struct Controls {
    /// Writes since the last frame, by path: the value a control sees until the model has it.
    pending: Vec<(Arc<str>, MValue)>,
    /// The slider being dragged.
    drag: Option<NodeId>,
    /// The drag changed the value (`change` on release).
    dragged: bool,
    /// The focused node when the current press began (a click on an open select closes it).
    focus_at_press: Option<NodeId>,
}

impl Controls {
    /// A frame starts: the model has taken the writes.
    pub fn frame(&mut self) {
        if !self.pending.is_empty() {
            self.pending.clear();
        }
    }
}

/// A control node's behaviour, from its bindings.
#[derive(Debug, Clone)]
pub(crate) struct Control {
    kind: ControlKind,
    /// `state.<path>` of `data-bind-model`.
    path: Option<Arc<str>>,
    /// `data-value` of a radio.
    value: Option<StrId>,
    /// Minimum, maximum and step of a slider.
    range: Option<[f32; 3]>,
    /// `data-options` of a select.
    options: Option<ExprId>,
}

fn control_of(pkg: &Package, tmpl: NodeRef) -> Option<Control> {
    let mut out: Option<Control> = None;
    let mut path = None;
    for b in pkg.bindings_of(tmpl.0, tmpl.1) {
        match &b.kind {
            BindKind::Control {
                kind,
                value,
                range,
                options,
            } => {
                out = Some(Control {
                    kind: *kind,
                    path: None,
                    value: *value,
                    range: *range,
                    options: *options,
                });
            }
            BindKind::Model(p) => {
                path = pkg.str(*p).map(|s| Arc::<str>::from(format!("state.{s}")));
            }
            _ => {}
        }
    }
    out.map(|mut c| {
        c.path = path;
        c
    })
}

/// A model value as text (what a radio's `data-value` is compared with).
fn value_text(v: &MValue) -> Option<String> {
    Some(match v {
        MValue::Bool(b) => b.to_string(),
        MValue::I32(i) => i.to_string(),
        MValue::U32(u) => u.to_string(),
        MValue::F32(f) => ui2_package::profile::expr::render(&Value::Float(f64::from(*f))),
        MValue::Str(s) => s.to_string(),
        MValue::Key(k) => k.to_js(),
        _ => return None,
    })
}

fn is_int(ty: Option<&Type>) -> bool {
    matches!(
        ty,
        Some(Type::Scalar(
            Scalar::I8 | Scalar::I16 | Scalar::I32 | Scalar::U32
        ))
    )
}

impl Runtime {
    /// The control of a node, when it is one.
    fn control(&self, id: NodeId) -> Option<Control> {
        control_of(&self.pkg, self.arena.get(id)?.tmpl())
    }

    /// The nearest control at or above a node.
    fn control_at(&self, id: NodeId) -> Option<(NodeId, Control)> {
        let mut at = Some(id);
        while let Some(a) = at {
            if let Some(c) = self.control(a) {
                return Some((a, c));
            }
            at = self.arena.get(a).and_then(|n| n.parent());
        }
        None
    }

    /// The value a control sees at its path: its own write since the last frame, else what its row read.
    fn control_value(&self, id: NodeId, path: &str) -> MValue {
        if let Some((_, v)) = self
            .input
            .controls
            .pending
            .iter()
            .find(|(p, _)| &**p == path)
        {
            return v.clone();
        }
        self.arena
            .get(id)
            .and_then(|n| self.binder.model_values(n.binds.clone()))
            .map_or(MValue::None, |(raw, _)| raw.clone())
    }

    /// The control's value as text.
    fn control_text(&self, id: NodeId, path: &str) -> Option<String> {
        if let Some((_, v)) = self
            .input
            .controls
            .pending
            .iter()
            .find(|(p, _)| &**p == path)
        {
            return value_text(v);
        }
        self.arena
            .get(id)
            .and_then(|n| self.binder.model_values(n.binds.clone()))
            .map(|(_, text)| text.to_string())
    }

    /// Writes a control's value; `false` when it already has it.
    fn write_control(&mut self, path: &Arc<str>, id: NodeId, value: MValue) -> bool {
        if self.control_value(id, path) == value {
            return false;
        }
        let pending = &mut self.input.controls.pending;
        pending.retain(|(p, _)| p != path);
        pending.push((Arc::clone(path), value.clone()));
        self.output.state_writes.push(StateOp {
            path: Arc::clone(path),
            value,
        });
        true
    }

    /// The events of a change: `input`, then `change` unless a drag goes on.
    fn control_changed(&mut self, id: NodeId, done: bool) {
        self.dispatch(id, Event::Input, false);
        if done {
            self.dispatch(id, Event::Change, false);
        }
    }

    /// Text (a radio's `data-value`) as a value of the path's type.
    fn typed(&self, path: &str, text: &str) -> MValue {
        let ty = self.binder.model_type(path);
        let v = match &ty {
            Some(Type::Scalar(Scalar::Bool)) => text.parse().ok().map(Value::Bool),
            t if is_int(t.as_ref()) => text.parse().ok().map(Value::Int),
            Some(Type::Scalar(Scalar::F32)) => text.parse().ok().map(Value::Float),
            _ => None,
        };
        to_model(
            &v.unwrap_or_else(|| Value::Str(text.to_string())),
            ty.as_ref(),
        )
    }

    /// An option's key as a value of the path's type.
    fn key_value(&self, path: &str, key: Key) -> MValue {
        match self.binder.model_type(path) {
            Some(Type::Scalar(Scalar::Str)) => MValue::Str(key.to_js().into()),
            ty => to_model(&Value::Int(key.0 as i64), ty.as_ref()),
        }
    }

    /// The press of a left button begins: remembers the focus (before the press moves it).
    pub(crate) fn control_press_begins(&mut self) {
        self.input.controls.focus_at_press = self.focused();
    }

    /// A click on `target`: the control at or above it acts.
    pub(crate) fn activate_control(&mut self, target: NodeId) {
        let focus_at_press = self.input.controls.focus_at_press.take();
        let Some((id, c)) = self.control_at(target) else {
            return;
        };
        let Some(path) = c.path.clone() else {
            return;
        };
        let value = match c.kind {
            ControlKind::Checkbox | ControlKind::Toggle => {
                let on = self.control_value(id, &path) == MValue::Bool(true);
                MValue::Bool(!on)
            }
            ControlKind::Radio => {
                let Some(v) = c.value.and_then(|s| self.pkg.str(s)).map(str::to_string) else {
                    return;
                };
                if self.control_text(id, &path).as_deref() == Some(v.as_str()) {
                    return;
                }
                self.typed(&path, &v)
            }
            ControlKind::Slider => return,
            ControlKind::Select => match self.option_of(id, target) {
                Some(key) => {
                    self.set_focus(None, false);
                    self.key_value(&path, key)
                }
                None => {
                    if focus_at_press == Some(id) {
                        self.set_focus(None, false);
                    }
                    return;
                }
            },
        };
        if self.write_control(&path, id, value) {
            self.control_changed(id, true);
        }
    }

    // Select.

    /// Whether a node is a `for` node.
    fn is_for(&self, id: NodeId) -> bool {
        self.arena
            .get(id)
            .filter(|n| n.class() == NodeClass::Service)
            .and_then(|n| instantiate::template_node(&*self.pkg, n.tmpl()))
            .is_some_and(|t| matches!(t.kind, NodeKind::For { .. }))
    }

    /// The option a node of a select is in: the key of the nearest instance of a `for` between the node and
    /// the select.
    fn option_of(&self, select: NodeId, target: NodeId) -> Option<Key> {
        let mut at = Some(target);
        while let Some(a) = at.filter(|&a| a != select) {
            let n = self.arena.get(a)?;
            if let (Some(owner), Some(key)) = (n.owner(), n.key())
                && self.is_for(owner)
            {
                return Some(key);
            }
            at = n.parent();
        }
        None
    }

    /// The keys of a select's options in shown order.
    fn options(&self, select: NodeId, c: &Control) -> Vec<Key> {
        let wanted = c.options.and_then(|e| self.pkg.expr(e));
        let mut first = None;
        let mut stack = vec![select];
        while let Some(id) = stack.pop() {
            let Some(n) = self.arena.get(id) else {
                continue;
            };
            if self.is_for(id) {
                let same = instantiate::template_node(&*self.pkg, n.tmpl()).is_some_and(|t| {
                    matches!(t.kind, NodeKind::For { list, .. } if wanted.is_some() && self.pkg.expr(list) == wanted)
                });
                if same {
                    first = Some(id);
                    break;
                }
                first.get_or_insert(id);
            }
            stack.extend(n.children().iter().rev().copied());
        }
        first
            .and_then(|f| self.arena.get(f))
            .map(|n| n.instances().iter().filter_map(|i| i.key).collect())
            .unwrap_or_default()
    }

    fn select_key(&mut self, id: NodeId, c: &Control, path: &Arc<str>, key: NamedKey) -> bool {
        let keys = self.options(id, c);
        let Some(last) = keys.len().checked_sub(1) else {
            return false;
        };
        let now = self.control_value(id, path);
        let at = keys.iter().position(|k| self.key_value(path, *k) == now);
        let next = match key {
            NamedKey::ArrowDown | NamedKey::ArrowRight => at.map_or(0, |i| (i + 1).min(last)),
            NamedKey::ArrowUp | NamedKey::ArrowLeft => at.map_or(0, |i| i.saturating_sub(1)),
            NamedKey::Home => 0,
            NamedKey::End => last,
            _ => return false,
        };
        let value = self.key_value(path, keys[next]);
        self.write_control(path, id, value)
    }

    // Slider.

    /// Sets a slider to a number: snapped to its steps, kept in its range, typed as its path.
    fn set_slider(&mut self, id: NodeId, c: &Control, path: &Arc<str>, v: f64) -> bool {
        let Some([lo, hi, step]) = c.range.map(|r| r.map(f64::from)) else {
            return false;
        };
        let v = v.clamp(lo, hi);
        let v = if step > 0.0 {
            (lo + ((v - lo) / step).round() * step).clamp(lo, hi)
        } else {
            v
        };
        let ty = self.binder.model_type(path);
        let value = match ty {
            Some(Type::Scalar(Scalar::U32)) => MValue::U32(v.round().max(0.0) as u32),
            ref t if is_int(t.as_ref()) => MValue::I32(v.round() as i32),
            _ => MValue::F32(v as f32),
        };
        self.write_control(path, id, value)
    }

    fn slider_number(&self, id: NodeId, c: &Control, path: &str) -> f64 {
        match self.control_value(id, path) {
            MValue::I32(i) => f64::from(i),
            MValue::U32(u) => f64::from(u),
            MValue::F32(f) => f64::from(f),
            _ => c.range.map_or(0.0, |r| f64::from(r[0])),
        }
    }

    /// Sets a slider from a point of the page (CSS px): its place across the slider's box.
    fn slider_to(&mut self, id: NodeId, at: Vec2) -> bool {
        let Some(c) = self.control(id) else {
            return false;
        };
        let (Some(path), Some([lo, hi, _])) = (c.path.clone(), c.range) else {
            return false;
        };
        let mut g = self.geo();
        let (x0, w) = (g.abs(id).x, g.size(id).x);
        let f = if w > 0.0 {
            ((at.x - x0) / w).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let v = f64::from(lo) + f64::from(f) * f64::from(hi - lo);
        self.set_slider(id, &c, &path, v)
    }

    /// A left press on `node` (not disabled): a slider at or above it takes the drag and the value.
    pub(crate) fn control_press(&mut self, node: NodeId, at: Vec2) {
        let Some((id, c)) = self.control_at(node) else {
            return;
        };
        if c.kind != ControlKind::Slider {
            return;
        }
        self.input.controls.drag = Some(id);
        self.input.controls.dragged = false;
        if self.slider_to(id, at) {
            self.input.controls.dragged = true;
            self.control_changed(id, false);
        }
    }

    /// The pointer moved: a dragged slider follows it.
    pub(crate) fn control_drag(&mut self, at: Vec2) {
        let Some(id) = self.input.controls.drag else {
            return;
        };
        if self.slider_to(id, at) {
            self.input.controls.dragged = true;
            self.control_changed(id, false);
        }
    }

    /// The press ended: a drag that changed its slider gives `change`. `cancel` drops the event.
    pub(crate) fn control_release(&mut self, cancel: bool) {
        let c = &mut self.input.controls;
        c.focus_at_press = None;
        let drag = c.drag.take();
        let dragged = std::mem::take(&mut c.dragged);
        if let Some(id) = drag
            && dragged
            && !cancel
            && self.arena.contains(id)
        {
            self.dispatch(id, Event::Change, false);
        }
    }

    // Keys.

    /// The radios of a path in document order (those that can take the focus).
    fn radio_group(&self, path: &str) -> Vec<NodeId> {
        self.arena
            .preorder()
            .into_iter()
            .filter(|&id| {
                self.control(id).is_some_and(|c| {
                    c.kind == ControlKind::Radio && c.path.as_deref() == Some(path)
                }) && self.focusable(id)
            })
            .collect()
    }

    /// A key press while a control has the focus: what the control does with it, or `None` when the key is
    /// not the control's (Enter and Space then click it).
    pub(crate) fn control_key(
        &mut self,
        k: &KeyEvent,
        named: Option<NamedKey>,
    ) -> Option<InputOutcome> {
        let id = self.focused()?;
        let c = self.control(id)?;
        let path = c.path.clone()?;
        let key = named?;
        let arrow = matches!(
            key,
            NamedKey::ArrowUp | NamedKey::ArrowDown | NamedKey::ArrowLeft | NamedKey::ArrowRight
        );
        let back = matches!(key, NamedKey::ArrowUp | NamedKey::ArrowLeft);
        match c.kind {
            ControlKind::Checkbox | ControlKind::Toggle => None,
            ControlKind::Radio => {
                if !arrow {
                    return None;
                }
                let group = self.radio_group(&path);
                let n = group.len();
                if let Some(i) = group.iter().position(|&r| r == id)
                    && n > 1
                {
                    let next = group[if back { (i + n - 1) % n } else { (i + 1) % n }];
                    self.set_focus(Some(next), true);
                    self.activate_control(next);
                }
                Some(InputOutcome::Consumed)
            }
            ControlKind::Slider => {
                let [lo, hi, step] = c.range.map(|r| r.map(f64::from))?;
                let now = self.slider_number(id, &c, &path);
                let v = match key {
                    NamedKey::ArrowUp | NamedKey::ArrowRight => now + step,
                    NamedKey::ArrowDown | NamedKey::ArrowLeft => now - step,
                    NamedKey::PageUp => now + 10.0 * step,
                    NamedKey::PageDown => now - 10.0 * step,
                    NamedKey::Home => lo,
                    NamedKey::End => hi,
                    // Enter and Space do nothing on a slider.
                    NamedKey::Enter | NamedKey::Space => return Some(InputOutcome::Consumed),
                    _ => return None,
                };
                if self.set_slider(id, &c, &path, v) {
                    self.control_changed(id, true);
                }
                Some(InputOutcome::Consumed)
            }
            ControlKind::Select => {
                match key {
                    NamedKey::Escape => {
                        if !k.repeat {
                            self.set_focus(None, false);
                        }
                    }
                    // The list stays as it is: open while focused.
                    NamedKey::Enter | NamedKey::Space => {}
                    _ => {
                        if !matches!(key, NamedKey::Home | NamedKey::End) && !arrow {
                            return None;
                        }
                        if self.select_key(id, &c, &path, key) {
                            self.control_changed(id, true);
                        }
                    }
                }
                Some(InputOutcome::Consumed)
            }
        }
    }
}
