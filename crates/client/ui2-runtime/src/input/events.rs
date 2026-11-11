//! Markup actions (`data-on-<event>`): what an event on a node asks of the rest of the UI.
//!
//! An event runs the handlers of its node, then of each ancestor (it bubbles) until a handler with `stop`.
//! `pointerenter`, `pointerleave`, `focus` and `blur` run on their node only. A node that is disabled, or
//! inside a disabled node, gets no events. The steps of a handler, in order:
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
use ui2_model::wire::{Origin, ScriptCtxId, ScriptEvent, StateOp, WireIntent};
use ui2_package::BindKind;
use ui2_package::profile::action::{Action, Event};

use crate::Runtime;
use crate::arena::NodeId;

impl Runtime {
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

    /// Runs an event on one node, without bubbling.
    pub(crate) fn dispatch_here(&mut self, id: NodeId, ev: Event) {
        if !self.disabled(id) {
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
                let values = self
                    .binder
                    .action_values(rows.clone(), bi as u16, ai as u16)
                    .map(<[MValue]>::to_vec)
                    .unwrap_or_default();
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
                            ctx: ScriptCtxId::Skin,
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
