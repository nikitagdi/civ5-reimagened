//! Markup actions (`data-on-<event>`): what an event on a node asks of the rest of the UI.
//!
//! An event runs the handlers of its node, then of each ancestor (it bubbles) until a handler with `stop`.
//! `pointerenter`, `pointerleave`, `focus` and `blur` run on their node only. A node that is disabled, or
//! inside a disabled node, gets no events but `pointerenter` and `pointerleave` (which Chrome dispatches to
//! disabled controls too). The steps of a handler, in order:
//!
//! - `intent name(args)` → a [`WireIntent`] with named arguments (positional ones, of mod intents, are named
//!   by their index) and the origin [`Origin::Markup`] (the template node index), or [`Origin::Hotkey`] when a
//!   key clicked the node;
//! - `set path = value` → a [`StateOp`] with the value;
//! - `toggle path` → a [`StateOp`] with the negation of the value the path had in the last frame;
//! - `call name(args)` → a [`ScriptEvent`] for the skin's scripts: no argument is `None`, one is the value,
//!   several are a `Struct` in order;
//! - `stop` → the event does not bubble past this node.
//!
//! The values come from the binding rows of the actions (evaluated in the frames, when what they read
//! changes), so handling an event needs no model. Everything goes to the runtime's [`UiOutput`](crate::UiOutput)
//! until [`Runtime::drain_output`].

use std::sync::Arc;

use ui2_model::Value as MValue;
use ui2_model::wire::{Origin, ScriptEvent, StateOp, WireIntent};
use ui2_package::BindKind;
use ui2_package::profile::action::{Action, Event};

use crate::Runtime;
use ui2_package::profile::expr::{EvalHost, Value};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct EventData {
    pub modifiers: platform::Modifiers,
    pub dx: f32,
    pub dy: f32,
}

pub(crate) struct EventHost<'a> {
    pub event: &'a EventData,
    pub paths: &'a [(Vec<String>, Value)],
    pub pkg: &'a crate::Pkg,
    pub loc: &'a crate::loc::SkinLoc,
}

impl EvalHost for EventHost<'_> {
    fn path(&mut self, segs: &[String]) -> Value {
        if segs.first().is_some_and(|s| s == "event") {
            return match segs.get(1).map(String::as_str) {
                Some("shift") => Value::Bool(self.event.modifiers.shift),
                Some("ctrl") => Value::Bool(self.event.modifiers.ctrl),
                Some("alt") => Value::Bool(self.event.modifiers.alt),
                Some("dx") => Value::Float(f64::from(self.event.dx)),
                Some("dy") => Value::Float(f64::from(self.event.dy)),
                _ => Value::Null,
            };
        }
        self.paths
            .iter()
            .find(|(p, _)| p == segs)
            .map_or(Value::Null, |(_, v)| v.clone())
    }
    fn loc(&mut self, key: &str, args: &[Value]) -> String {
        self.loc.call(self.pkg, key, args)
    }
    fn warn(&mut self, message: &str) {
        tracing::warn!(target: "ui2.input", "{message}");
    }
}
use crate::arena::NodeId;

impl Runtime {
    pub(crate) fn has_handler(&self, id: NodeId, event: Event) -> bool {
        let mut at = Some(id);
        while let Some(n) = at.and_then(|id| self.arena.get(id)) {
            if self
                .pkg
                .bindings_of(n.tmpl().0, n.tmpl().1)
                .iter()
                .any(|b| matches!(b.kind, BindKind::Event { ev, .. } if ev == event))
            {
                return true;
            }
            at = n.parent();
        }
        false
    }
    /// Runs an event on a node and bubbles it up its ancestors. `hotkey` says that a key clicked the node.
    pub(crate) fn dispatch(&mut self, target: NodeId, ev: Event, hotkey: bool) {
        if self.disabled(target) {
            return;
        }
        // A click acts on the control it is in first (its state write comes before the handlers' steps).
        if ev == Event::Click {
            self.activate_control(target);
        }
        let mut at = Some(target);
        while let Some(id) = at {
            if self.run_handlers(id, ev, hotkey) {
                break;
            }
            at = self.arena.get(id).and_then(|n| n.parent());
        }
    }

    /// Runs an event on one node, without bubbling. `pointerenter` and `pointerleave` run on a disabled node too
    /// (as Chrome dispatches pointer events to disabled controls): a disabled row still names itself for its
    /// tooltip.
    pub(crate) fn dispatch_here(&mut self, id: NodeId, ev: Event) {
        if matches!(ev, Event::Pointerenter | Event::Pointerleave) || !self.disabled(id) {
            self.run_handlers(id, ev, false);
        }
    }

    /// Runs the handlers of one node for an event; returns whether one of them stops the bubbling.
    fn run_handlers(&mut self, id: NodeId, ev: Event, hotkey: bool) -> bool {
        let Some(n) = self.arena.get(id) else {
            return false;
        };
        let (tmpl, rows) = (n.tmpl(), n.binds.clone());
        let pkg = Arc::clone(&self.pkg);
        let mut stop = false;
        for (bi, b) in pkg.bindings_of(tmpl.0, tmpl.1).iter().enumerate() {
            let BindKind::Event { ev: on, actions } = &b.kind else {
                continue;
            };
            if *on != ev {
                continue;
            }
            for (ai, a) in actions.iter().enumerate() {
                let values = self.binder.event_values(
                    rows.clone(),
                    bi as u16,
                    ai as u16,
                    &self.input.event,
                    &pkg,
                );
                let value = |i: usize| values.get(i).cloned().unwrap_or(MValue::None);
                match a {
                    Action::Intent { name, args } => {
                        let origin = if hotkey {
                            Origin::Hotkey
                        } else {
                            Origin::Markup { node: tmpl.1 }
                        };
                        let args = args
                            .iter()
                            .enumerate()
                            .map(|(i, arg)| {
                                let name: Arc<str> = match &arg.name {
                                    Some(n) => n.as_str().into(),
                                    None => i.to_string().into(),
                                };
                                (name, value(i))
                            })
                            .collect();
                        self.output.intents.push(WireIntent {
                            name: name.as_str().into(),
                            args,
                            origin,
                        });
                    }
                    Action::Set { path, .. } => self.output.state_writes.push(StateOp {
                        path: path.as_str().into(),
                        value: value(0),
                    }),
                    Action::Toggle { path } => {
                        let now = matches!(values.first(), Some(MValue::Bool(true)));
                        self.output.state_writes.push(StateOp {
                            path: path.as_str().into(),
                            value: MValue::Bool(!now),
                        });
                    }
                    Action::Call { name, .. } => {
                        let arg = match values.len() {
                            0 => MValue::None,
                            1 => value(0),
                            _ => MValue::Struct(values.clone()),
                        };
                        let node_path = self.path(id).to_string();
                        self.output.script_events.push(ScriptEvent {
                            input: ui2_model::wire::ScriptInput {
                                shift: self.input.event.modifiers.shift,
                                ctrl: self.input.event.modifiers.ctrl,
                                alt: self.input.event.modifiers.alt,
                                dx: self.input.event.dx,
                                dy: self.input.event.dy,
                            },
                            ctx: pkg.ctx_of(tmpl.0),
                            name: name.as_str().into(),
                            arg,
                            node_path: node_path.into(),
                        });
                    }
                    Action::Stop => stop = true,
                }
            }
        }
        stop
    }
}
