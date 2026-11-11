//! Keyboard focus: which node gets Enter and Space (and, for a text field, the typing), how it moves with Tab
//! and with presses.
//!
//! A node can take focus when its template node is focusable (`tabindex`, buttons, controls, text fields),
//! it is shown (no `display: none` or `visibility: hidden` at or above it) and it is not disabled. A press
//! focuses the nearest such node at or above the pressed one, or takes the focus from the UI; Tab and
//! Shift+Tab move it through the focusable nodes in document order, inside the top visible screen when there
//! is one (and never into the tooltip layer), wrapping at the ends. `FOCUS` is set on the focused node,
//! `FOCUS_VISIBLE` too when the keyboard moved the focus there; `blur` and `focus` run on the nodes that lose
//! and get it.

use ui2_package::profile::action::Event;
use ui2_package::{Layer, NodeKind, node_flags};

use crate::Runtime;
use crate::arena::{NodeId, StateBits};
use crate::input::hit;
use crate::{instantiate, style};

impl Runtime {
    /// The node with keyboard focus.
    pub fn focused(&self) -> Option<NodeId> {
        self.input.focus.filter(|&f| self.arena.contains(f))
    }

    /// Whether a node can take the focus now.
    pub fn focusable(&self, id: NodeId) -> bool {
        let Some(n) = self.arena.get(id) else {
            return false;
        };
        let flagged = instantiate::template_node(&*self.pkg, n.tmpl())
            .is_some_and(|t| t.flags & node_flags::FOCUSABLE != 0);
        flagged && !self.disabled(id) && self.geo().shown(id)
    }

    /// The focused node when it is a text field (it takes the keyboard but Esc and Tab).
    pub(crate) fn text_focus(&self) -> Option<NodeId> {
        self.focused().filter(|&f| {
            self.arena
                .get(f)
                .and_then(|n| instantiate::template_node(&*self.pkg, n.tmpl()))
                .is_some_and(|t| matches!(t.kind, NodeKind::Input { .. }))
        })
    }

    /// The nearest focusable node at or above a node.
    pub(crate) fn focus_target(&self, id: NodeId) -> Option<NodeId> {
        let mut at = Some(id);
        while let Some(a) = at {
            if self.focusable(a) {
                return Some(a);
            }
            at = self.arena.get(a).and_then(|n| n.parent());
        }
        None
    }

    /// Moves the focus to a node (or takes it from the UI). `keyboard` says that the keyboard moved it (the
    /// node then also gets `FOCUS_VISIBLE`). A node that cannot take the focus takes nothing.
    pub fn set_focus(&mut self, id: Option<NodeId>, keyboard: bool) {
        let id = id.filter(|&i| self.focusable(i));
        let old = self.focused();
        if old == id {
            if let Some(i) = id {
                style::select::set_state(&mut self.arena, i, StateBits::FOCUS_VISIBLE, keyboard);
            }
            return;
        }
        if let Some(o) = old {
            style::select::set_state(
                &mut self.arena,
                o,
                StateBits::FOCUS | StateBits::FOCUS_VISIBLE,
                false,
            );
            self.dispatch_here(o, Event::Blur);
        }
        self.input.focus = id;
        if let Some(i) = id {
            style::select::set_state(&mut self.arena, i, StateBits::FOCUS, true);
            style::select::set_state(&mut self.arena, i, StateBits::FOCUS_VISIBLE, keyboard);
            self.dispatch_here(i, Event::Focus);
        }
    }

    /// The nodes Tab moves through, in document order: inside the top visible screen, or the whole document
    /// but the tooltip layer.
    fn tab_order(&self) -> Vec<NodeId> {
        let scope: Vec<NodeId> = match self.visible_screens().last() {
            Some(&screen) => vec![screen],
            None => hit::layer_roots(&self.pkg, &self.arena)
                .into_iter()
                .filter(|(l, _, _)| *l != Layer::Tooltip)
                .map(|(_, r, _)| r)
                .collect(),
        };
        let mut out = Vec::new();
        for root in scope {
            let mut stack = vec![root];
            while let Some(id) = stack.pop() {
                let Some(n) = self.arena.get(id) else {
                    continue;
                };
                if self.focusable(id) {
                    out.push(id);
                }
                stack.extend(n.children().iter().rev().copied());
            }
        }
        out
    }

    /// Tab (or Shift+Tab, `back`): moves the focus to the next (previous) focusable node. Returns whether
    /// there is any.
    pub(crate) fn tab(&mut self, back: bool) -> bool {
        let order = self.tab_order();
        if order.is_empty() {
            return false;
        }
        let at = self
            .focused()
            .and_then(|f| order.iter().position(|&n| n == f));
        let next = match (at, back) {
            (None, false) => 0,
            (None, true) => order.len() - 1,
            (Some(i), false) => (i + 1) % order.len(),
            (Some(i), true) => (i + order.len() - 1) % order.len(),
        };
        self.set_focus(Some(order[next]), true);
        true
    }
}
