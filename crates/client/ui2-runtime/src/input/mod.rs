//! Input: pointer, wheel and keyboard events of the window, turned into interaction states, scrolling and
//! what markup asks for.
//!
//! The host feeds every [`platform::InputEvent`] to [`Runtime::handle_input`] between frames, in physical
//! window pixels; the runtime divides by the device scale. The answer says who takes the event:
//!
//! - [`InputOutcome::Consumed`]: the UI took it (a panel is under the pointer, a press started on a panel is
//!   still held, a modal screen blocks the layers below it, a scroll container scrolled);
//! - [`InputOutcome::PassToWorld`]: the event is the world's (the camera and the picker): the pointer is over
//!   no panel, or a press started in the world is still held (the UI then only follows the hover);
//! - [`InputOutcome::MinimapClick`]: a press on the minimap picture (`render:minimap`), or a drag that started
//!   there: the host moves the camera to `(u, v)`, the point in the picture from its top left corner, 0 to 1.
//!
//! States: `hover` is set on the node under the pointer and its ancestors (as in CSS); `active` on the
//! pressed node and its ancestors from the press to the release; a node that is disabled, or inside a
//! disabled node, never becomes active. A press captures the pointer until every button is released: the UI
//! when it started on a panel (or on what a modal screen blocks), the world otherwise.
//!
//! The wheel over a scroll container (`overflow: auto | scroll`) scrolls the nearest one and is consumed
//! (also at the end of its range); over a panel without one, or over the world, it goes to the world.
//!
//! The cursor is the `cursor` of the node under the pointer ([`Runtime::cursor`], also in
//! [`FrameOutput::cursor`](crate::FrameOutput::cursor)).
//!
//! What events ask for goes to the runtime's output ([`Runtime::drain_output`]): markup actions of
//! `data-on-*` (`events`), the focus and Tab (`focus`), the keyboard with Enter, Space, Esc and
//! `data-hotkey` (`hotkeys`), tooltips of `data-tooltip` (`tooltip`), the `data-control` nodes (`controls`:
//! a click, a key or a slider drag writes their model path). Pointer events: `pointerdown` on the
//! pressed node; on release `pointerup` on the node under the pointer, then `click` (left button) or
//! `contextmenu` (right) on the nearest node holding both the pressed and the released node, and `dblclick`
//! after a second click on one node within [`DOUBLE_CLICK_MS`]; `pointerenter`/`pointerleave` as the hover
//! chain changes. A left press moves the focus to the nearest focusable node at or above the pressed one, or
//! takes it from the UI (also a press in the world).

pub(crate) mod controls;
mod events;
mod focus;
pub(crate) mod hit;
mod hotkeys;
mod tooltip;

use std::sync::Arc;

use glam::Vec2;
use platform::{InputEvent, MouseButton, PointerEvent, PointerPhase, WheelDelta, WheelEvent};
use ui2_package::profile::action::Event;
use ui2_package::style::Cursor;
use ui2_package::{ARefId, ImageSrc, NodeKind, NodeRef, Package};

use crate::arena::{Dirty, NodeId, StateBits};
use crate::{Runtime, instantiate, style};
use hit::{Geo, Hit};

/// CSS px one wheel line scrolls.
pub const WHEEL_LINE_PX: f32 = 40.0;

/// The asset reference of the minimap picture.
pub const MINIMAP_ASSET: &str = "render:minimap";

/// The longest time between two clicks on one node that makes a `dblclick`, ms.
pub const DOUBLE_CLICK_MS: f64 = 500.0;

/// Who takes an input event (see the [module docs](crate::input)).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputOutcome {
    /// The UI took the event.
    Consumed,
    /// The event belongs to the world (camera, picker, game hotkeys).
    PassToWorld,
    /// A press or drag on the minimap: the point in the minimap picture, 0 to 1 from its top left corner.
    MinimapClick {
        /// Horizontal, left to right.
        u: f32,
        /// Vertical, top to bottom.
        v: f32,
    },
}

/// Who holds the pointer between a press and the release of every button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PointerCapture {
    /// No button is held.
    #[default]
    None,
    /// A press started on the UI: the pressed node (`None` when a modal screen blocked the press).
    Ui(Option<NodeId>),
    /// A press started in the world: the world gets the events until the release.
    World,
}

/// The input state of a runtime.
#[derive(Debug, Default)]
pub(crate) struct InputState {
    /// The node under the pointer.
    pub target: Option<NodeId>,
    pub capture: PointerCapture,
    /// Held buttons (bit per [`button_bit`]).
    buttons: u8,
    /// Nodes with `ACTIVE`: the pressed node and its ancestors.
    active: Vec<NodeId>,
    /// The press started on the minimap: moves report the minimap point.
    minimap_drag: bool,
    /// The package's screens (`data-screen`) by node, sorted, with their ids.
    pub screens: Vec<(NodeRef, Arc<str>)>,
    /// The asset reference of the minimap picture, when the package has one.
    minimap: Option<ARefId>,
    /// The node with keyboard focus.
    pub focus: Option<NodeId>,
    /// The last click: its node and time (for `dblclick`).
    last_click: Option<(NodeId, f64)>,
    /// The tooltip on screen.
    pub tooltip: Option<tooltip::Tooltip>,
    /// Controls: writes not in the model yet, the dragged slider.
    pub controls: controls::Controls,
}

impl InputState {
    pub fn new(pkg: &Package) -> Self {
        let mut screens: Vec<(NodeRef, Arc<str>)> = pkg
            .screens()
            .map(|(id, node)| (node, Arc::from(id)))
            .collect();
        screens.sort_by_key(|(n, _)| *n);
        let minimap = (0..pkg.asset_refs().count())
            .map(|i| ARefId(i as u32))
            .find(|a| pkg.asset_ref(*a) == Some(MINIMAP_ASSET));
        Self {
            screens,
            minimap,
            ..Self::default()
        }
    }
}

fn button_bit(b: Option<MouseButton>) -> u8 {
    match b {
        // Touch and pen contacts carry no button: they press like the left one.
        None | Some(MouseButton::Left) => 1,
        Some(MouseButton::Right) => 2,
        Some(MouseButton::Middle) => 4,
        Some(_) => 8,
    }
}

impl Runtime {
    fn geo(&self) -> Geo<'_> {
        Geo::new(&self.arena, &self.pkg, &self.style_index)
            .with_anchors(&self.anchors, self.style_env.scale.k)
    }

    /// Physical window pixels to page CSS px.
    fn css(&self, x: f64, y: f64) -> Vec2 {
        let k = self.style_env.scale.k;
        Vec2::new(x as f32 / k, y as f32 / k)
    }

    pub(crate) fn hit_at(&self, p: Vec2) -> Hit {
        let mut g = self.geo();
        let modal = !g.visible_screens(&self.input.screens).is_empty();
        g.hit(p, modal)
    }

    /// The node that takes input at a point of the window (physical pixels), if any.
    pub fn node_at(&self, x: f64, y: f64) -> Option<NodeId> {
        self.hit_at(self.css(x, y)).node
    }

    /// The node under the pointer (its ancestors are hovered too).
    pub fn hovered(&self) -> Option<NodeId> {
        self.input.target.filter(|&t| self.arena.contains(t))
    }

    /// Who holds the pointer.
    pub fn pointer_capture(&self) -> PointerCapture {
        self.input.capture
    }

    /// The visible screens of the `window` layer, in document order (the last is the top one). While there is
    /// one, input does not reach the layers below the `window` layer.
    pub fn visible_screens(&self) -> Vec<NodeId> {
        self.geo().visible_screens(&self.input.screens)
    }

    /// The pointer cursor the UI wants: the `cursor` of the node under the pointer; `auto` (the world decides)
    /// off the UI and while a press started in the world is held.
    pub fn cursor(&self) -> Cursor {
        if self.input.capture == PointerCapture::World {
            return Cursor::Auto;
        }
        self.hovered()
            .and_then(|t| self.style(t))
            .map_or(Cursor::Auto, |r| r.cursor)
    }

    /// Takes one input event of the window (see the [module docs](crate::input)). `now_ms` is the host's
    /// clock.
    pub fn handle_input(&mut self, ev: &InputEvent, now_ms: f64) -> InputOutcome {
        match ev {
            InputEvent::Pointer(p) => self.on_pointer(p, now_ms),
            InputEvent::Wheel(w) => self.on_wheel(w),
            InputEvent::Key(k) => self.on_key(k),
            InputEvent::Text(_) => self.on_text(),
            InputEvent::Focused(false) => {
                // The window lost the keyboard and the mouse: nothing stays pressed.
                self.release_pointer();
                InputOutcome::PassToWorld
            }
            _ => InputOutcome::PassToWorld,
        }
    }

    /// The outcome of an event outside a press: the UI's when something of it is under the pointer.
    fn idle_outcome(hit: &Hit) -> InputOutcome {
        if hit.node.is_some() || hit.blocked {
            InputOutcome::Consumed
        } else {
            InputOutcome::PassToWorld
        }
    }

    /// The outcome while a press is held.
    fn captured_outcome(&self, hit: &Hit, at: Vec2) -> InputOutcome {
        match self.input.capture {
            PointerCapture::Ui(Some(n)) if self.input.minimap_drag => {
                if hit.node == Some(n) {
                    return minimap_uv(hit.local, hit.size);
                }
                // Dragged off the minimap: the point against its box (clamped to the edge).
                let mut g = self.geo();
                let local = at - g.abs(n);
                minimap_uv(local, g.size(n))
            }
            PointerCapture::Ui(_) => InputOutcome::Consumed,
            PointerCapture::World => InputOutcome::PassToWorld,
            PointerCapture::None => Self::idle_outcome(hit),
        }
    }

    fn on_pointer(&mut self, p: &PointerEvent, now_ms: f64) -> InputOutcome {
        if p.phase == PointerPhase::Left {
            self.update_hover(None);
            return match self.input.capture {
                PointerCapture::Ui(_) => InputOutcome::Consumed,
                _ => InputOutcome::PassToWorld,
            };
        }
        let at = self.css(p.position.x, p.position.y);
        let hit = self.hit_at(at);
        self.update_hover(hit.node);
        match p.phase {
            PointerPhase::Down => self.on_down(p, &hit, at),
            PointerPhase::Up | PointerPhase::Cancel => self.on_up(p, &hit, at, now_ms),
            _ => {
                self.control_drag(at);
                self.captured_outcome(&hit, at)
            }
        }
    }

    fn on_down(&mut self, p: &PointerEvent, hit: &Hit, at: Vec2) -> InputOutcome {
        let bit = button_bit(p.button);
        let first = self.input.buttons == 0;
        self.input.buttons |= bit;
        if !first {
            // Another button while one is held: the holder keeps the pointer.
            return self.captured_outcome(hit, at);
        }
        if hit.blocked {
            self.input.capture = PointerCapture::Ui(None);
            return InputOutcome::Consumed;
        }
        let Some(node) = hit.node else {
            self.input.capture = PointerCapture::World;
            self.pointer_down_in_world();
            return InputOutcome::PassToWorld;
        };
        self.input.capture = PointerCapture::Ui(Some(node));
        if !self.disabled(node) {
            self.set_active(Some(node));
        }
        self.pointer_down_on(node, p);
        if bit == 1 && !self.disabled(node) {
            self.control_press(node, at);
        }
        if bit == 1 && self.is_minimap(node) {
            self.input.minimap_drag = true;
            return minimap_uv(hit.local, hit.size);
        }
        InputOutcome::Consumed
    }

    fn on_up(&mut self, p: &PointerEvent, hit: &Hit, at: Vec2, now_ms: f64) -> InputOutcome {
        let cancel = p.phase == PointerPhase::Cancel;
        if cancel {
            self.input.buttons = 0;
        } else {
            self.input.buttons &= !button_bit(p.button);
        }
        if self.input.buttons != 0 {
            return self.captured_outcome(hit, at);
        }
        let capture = std::mem::take(&mut self.input.capture);
        self.input.minimap_drag = false;
        self.set_active(None);
        match capture {
            PointerCapture::Ui(pressed) => {
                if !cancel && let Some(pressed) = pressed {
                    self.pointer_up_on(pressed, hit.node, p, now_ms);
                }
                self.control_release(cancel);
                InputOutcome::Consumed
            }
            PointerCapture::World => InputOutcome::PassToWorld,
            PointerCapture::None => Self::idle_outcome(hit),
        }
    }

    /// Ends any press without actions (the window lost focus).
    fn release_pointer(&mut self) {
        self.input.buttons = 0;
        self.input.capture = PointerCapture::None;
        self.input.minimap_drag = false;
        self.set_active(None);
        self.control_release(true);
    }

    fn on_wheel(&mut self, w: &WheelEvent) -> InputOutcome {
        let at = self.css(w.position.x, w.position.y);
        let hit = self.hit_at(at);
        if hit.blocked {
            return InputOutcome::Consumed;
        }
        let Some(node) = hit.node else {
            return InputOutcome::PassToWorld;
        };
        let Some(scroller) = self.geo().scroller(node) else {
            return InputOutcome::PassToWorld;
        };
        let k = self.style_env.scale.k;
        // Positive deltas scroll up (and left): the content moves down, the offset shrinks.
        let d = match w.delta {
            WheelDelta::Lines { x, y } => Vec2::new(-x, -y) * WHEEL_LINE_PX,
            WheelDelta::Pixels { x, y } => Vec2::new(-x as f32, -y as f32) / k,
        };
        self.scroll_by(scroller, d);
        InputOutcome::Consumed
    }

    /// Scrolls a scroll container by a CSS px delta, within its range; its chunk repaints when it moved.
    pub fn scroll_by(&mut self, id: NodeId, d: Vec2) {
        let range = hit::scroll_range(&self.arena, id);
        let Some(n) = self.arena.get_mut(id) else {
            return;
        };
        let old = n.scroll;
        let new = [
            (old[0] + d.x).clamp(0.0, range.x),
            (old[1] + d.y).clamp(0.0, range.y),
        ];
        if new != old {
            n.scroll = new;
            self.arena.mark(id, Dirty::PAINT);
            // A virtual list in it may show other rows now.
            self.binder.windows_changed();
        }
    }

    /// Whether a node or an ancestor is disabled.
    pub(crate) fn disabled(&self, id: NodeId) -> bool {
        let mut at = Some(id);
        while let Some(n) = at.and_then(|a| self.arena.get(a)) {
            if n.state().contains(StateBits::DISABLED) {
                return true;
            }
            at = n.parent();
        }
        false
    }

    fn is_minimap(&self, id: NodeId) -> bool {
        let Some(m) = self.input.minimap else {
            return false;
        };
        self.arena
            .get(id)
            .and_then(|n| instantiate::template_node(&*self.pkg, n.tmpl()))
            .is_some_and(|t| matches!(t.kind, NodeKind::Image(ImageSrc::Asset(a)) if a == m))
    }

    /// Moves `ACTIVE` to a node and its ancestors (or clears it).
    fn set_active(&mut self, target: Option<NodeId>) {
        let mut chain = Vec::new();
        let mut at = target.filter(|&t| self.arena.contains(t));
        while let Some(id) = at {
            chain.push(id);
            at = self.arena.get(id).and_then(|n| n.parent());
        }
        let old = std::mem::take(&mut self.input.active);
        for &id in old.iter().filter(|id| !chain.contains(id)) {
            style::select::set_state(&mut self.arena, id, StateBits::ACTIVE, false);
        }
        for &id in &chain {
            style::select::set_state(&mut self.arena, id, StateBits::ACTIVE, true);
        }
        self.input.active = chain;
    }

    /// Moves the hover to the node under the pointer: `pointerleave` on the nodes that lost it, `pointerenter`
    /// on those that got it (outermost first; neither bubbles), and the tooltip follows.
    fn update_hover(&mut self, target: Option<NodeId>) {
        if target == self.input.target && self.hover.first().copied() == target {
            return;
        }
        let old = self.hover.clone();
        self.input.target = target;
        self.set_hover(target);
        let new = self.hover.clone();
        for &id in old.iter().filter(|id| !new.contains(id)) {
            self.dispatch_here(id, Event::Pointerleave);
        }
        for &id in new.iter().rev().filter(|id| !old.contains(id)) {
            self.dispatch_here(id, Event::Pointerenter);
        }
        self.update_tooltip();
    }

    /// A press in the world takes the focus from the UI.
    fn pointer_down_in_world(&mut self) {
        self.set_focus(None, false);
    }

    /// A press on a node: the focus goes to the nearest focusable node at or above it (or leaves the UI), then
    /// `pointerdown`.
    fn pointer_down_on(&mut self, node: NodeId, p: &PointerEvent) {
        if button_bit(p.button) == 1 {
            self.control_press_begins();
            let focus = self.focus_target(node);
            self.set_focus(focus, false);
        }
        self.dispatch(node, Event::Pointerdown, false);
    }

    /// The release of a press that started on `pressed`, over `over`: `pointerup` on the node under the
    /// pointer, then `click` (left button) or `contextmenu` (right button) on the nearest node that holds both
    /// (as in browsers), and `dblclick` after a second click on the same node within [`DOUBLE_CLICK_MS`].
    fn pointer_up_on(
        &mut self,
        pressed: NodeId,
        over: Option<NodeId>,
        p: &PointerEvent,
        now_ms: f64,
    ) {
        let Some(over) = over else {
            return;
        };
        self.dispatch(over, Event::Pointerup, false);
        let Some(target) = self.common_ancestor(pressed, over) else {
            return;
        };
        match button_bit(p.button) {
            1 => {
                self.dispatch(target, Event::Click, false);
                let double = self
                    .input
                    .last_click
                    .is_some_and(|(n, t)| n == target && now_ms - t <= DOUBLE_CLICK_MS);
                if double {
                    self.input.last_click = None;
                    self.dispatch(target, Event::Dblclick, false);
                } else {
                    self.input.last_click = Some((target, now_ms));
                }
            }
            2 => self.dispatch(target, Event::Contextmenu, false),
            _ => {}
        }
    }

    /// The nearest node that is `a` or an ancestor of it and also `b` or an ancestor of `b`.
    fn common_ancestor(&self, a: NodeId, b: NodeId) -> Option<NodeId> {
        let mut chain = Vec::new();
        let mut at = Some(b);
        while let Some(id) = at.filter(|&i| self.arena.contains(i)) {
            chain.push(id);
            at = self.arena.get(id).and_then(|n| n.parent());
        }
        let mut at = Some(a);
        while let Some(id) = at.filter(|&i| self.arena.contains(i)) {
            if chain.contains(&id) {
                return Some(id);
            }
            at = self.arena.get(id).and_then(|n| n.parent());
        }
        None
    }

    /// Text typed into the window: a focused text field's (the host's text bridge edits it).
    fn on_text(&mut self) -> InputOutcome {
        if self.text_focus().is_some() {
            InputOutcome::Consumed
        } else {
            InputOutcome::PassToWorld
        }
    }
}

fn minimap_uv(local: Vec2, size: Vec2) -> InputOutcome {
    let f = |l: f32, s: f32| {
        if s > 0.0 {
            (l / s).clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    InputOutcome::MinimapClick {
        u: f(local.x, size.x),
        v: f(local.y, size.y),
    }
}
