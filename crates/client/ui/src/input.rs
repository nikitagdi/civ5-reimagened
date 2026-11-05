//! Input: Win32 messages, as the exe and the Lua of the original see them
//! (`UIManager::ProcessNormalInput`). The traversal is in `Shared` because a context's
//! `InputHandler` is Lua and its answer decides whether the event goes on.

use audio::{ButtonEvent, ButtonFlags, ControlKind};
use glam::Vec2;

use crate::flags::*;
use crate::shared::Shared;
use crate::state::{LuaArg, State};
use crate::tree::*;

pub const WM_MOUSEMOVE: u32 = 0x200;
pub const WM_LBUTTONDOWN: u32 = 0x201;
pub const WM_LBUTTONUP: u32 = 0x202;
pub const WM_LBUTTONDBLCLK: u32 = 0x203;
pub const WM_RBUTTONDOWN: u32 = 0x204;
pub const WM_RBUTTONUP: u32 = 0x205;
pub const WM_RBUTTONDBLCLK: u32 = 0x206;
pub const WM_MBUTTONDOWN: u32 = 0x207;
pub const WM_MBUTTONUP: u32 = 0x208;
pub const WM_MBUTTONDBLCLK: u32 = 0x209;
pub const WM_MOUSEWHEEL: u32 = 0x20a;
pub const WM_KEYDOWN: u32 = 0x100;
pub const WM_KEYUP: u32 = 0x101;
pub const WM_CHAR: u32 = 0x102;

fn is_mouse(msg: u32) -> bool {
    (0x200..0x20f).contains(&msg)
}

fn is_pointer(msg: u32) -> bool {
    (0x241..0x250).contains(&msg)
}

impl State {
    /// `ControlBase::DoConsumeMouse`.
    pub fn consume_mouse(&self, id: ControlId, msg: u32, origin: Vec2) -> bool {
        let f = self.tree[id].flags;
        if f & CONSUME_MOUSE == 0 {
            return false;
        }
        let hit = (f & CONSUME_BUTTON != 0
            && ((0x201..0x20a).contains(&msg) || (msg & !1) == 0x246))
            || (f & CONSUME_OVER != 0 && ((0x245..0x248).contains(&msg) || msg == WM_MOUSEMOVE))
            || (f & CONSUME_WHEEL != 0 && msg == WM_MOUSEWHEEL);
        hit && self.test_hit_virtual(id, self.mouse, origin)
    }

    /// `TestHit`, virtual: a check box is hit by its box or its label.
    pub fn test_hit_virtual(&self, id: ControlId, mouse: Vec2, origin: Vec2) -> bool {
        if let Kind::CheckBox(c) | Kind::RadioButton(c) = &self.tree[id].kind {
            let o = self.tree[id].pos + origin;
            return self.test_hit(c.boxed, mouse, o) || self.test_hit(c.label, mouse, o);
        }
        self.test_hit(id, mouse, origin)
    }

    /// `UIManager::RequestMouseOver`.
    pub fn request_mouse_over(&mut self, id: ControlId) {
        if self.tree[id].has(NEEDS_MOUSE_OVER) && !self.hover_requests.contains(&id) {
            self.hover_requests.push(id);
        }
    }

    /// `ButtonControlBase::ForcedButtonUp`-less half of `ProcessMouse`.
    pub fn button_process_mouse(
        &mut self,
        id: ControlId,
        msg: u32,
        inside: bool,
        origin: Vec2,
    ) -> bool {
        let f = self.tree[id].flags;
        if inside && f & DISABLED != 0 && msg != WM_MOUSEWHEEL && msg != WM_MOUSEMOVE {
            return false;
        }
        if matches!(self.tree[id].kind, Kind::CheckBox(_) | Kind::RadioButton(_))
            && matches!(msg, WM_LBUTTONDOWN | WM_LBUTTONDBLCLK)
        {
            // CheckBoxControl::ProcessMouse
            if f & MOUSE_OVER != 0 && inside {
                self.tree[id].flags |= btn::LEFT_DOWN;
                self.set_vis_state(id, 2);
                return true;
            }
            return false;
        }
        let has = |s: &State, slot: usize| {
            s.tree[id]
                .kind
                .button_base()
                .is_some_and(|b| b.callbacks[slot].is_some())
        };
        // (down message, up message, flag, slot)
        let table: [(u32, u32, u32, usize); 3] = [
            (WM_LBUTTONDOWN, WM_LBUTTONUP, btn::LEFT_DOWN, 0),
            (WM_RBUTTONDOWN, WM_RBUTTONUP, btn::RIGHT_DOWN, 2),
            (WM_MBUTTONDOWN, WM_MBUTTONUP, btn::MIDDLE_DOWN, 3),
        ];
        for (down, up, bit, slot) in table {
            let is_down = msg == down || msg == down + 2;
            if is_down {
                // The left button needs no handler to be taken; the others do.
                if f & MOUSE_OVER == 0 || !inside {
                    return false;
                }
                if slot != 0 && !has(self, slot) {
                    return false;
                }
                self.tree[id].flags |= bit;
                if slot == 0 && !has(self, 0) {
                    return true;
                }
                self.set_vis_state(id, 2);
                return true;
            }
            if msg == up || (slot == 0 && msg == 0x247) {
                if f & bit == 0 {
                    return false;
                }
                if inside {
                    let mouse = self.mouse;
                    let pos = self.tree[id].pos;
                    if let Some(b) = self.tree[id].kind.button_base_mut() {
                        b.grab = mouse - origin - pos;
                    }
                    self.tree[id].flags &= !bit;
                    self.granted_mouse_over(id);
                    self.do_click(id, slot);
                    return true;
                }
                let before = self.tree[id].flags;
                self.tree[id].flags &= !bit;
                let others = btn::ANY_DOWN & !bit;
                if before & others == 0 {
                    self.lost_mouse_over(id);
                }
                return false;
            }
        }
        false
    }

    /// `ButtonControlBase::DoClick`: the handler first, then the default sound when the slot
    /// has a handler and the control does not mute it.
    pub fn do_click(&mut self, id: ControlId, slot: usize) {
        if matches!(self.tree[id].kind, Kind::CheckBox(_) | Kind::RadioButton(_)) && slot == 0 {
            self.check_box_click(id);
        }
        self.call_button_slot(id, slot);
        let n = &self.tree[id];
        if n.flags & btn::NO_DEFAULT_SOUND != 0 {
            return;
        }
        let has_handler = n
            .kind
            .button_base()
            .is_some_and(|b| b.callbacks[slot.min(7)].is_some())
            || matches!(&n.kind, Kind::CheckBox(c) | Kind::RadioButton(c) if c.handler.is_some());
        if has_handler {
            let flags = ButtonFlags {
                no_default_sound: false,
                disabled: n.flags & DISABLED != 0,
                kind: match n.kind {
                    Kind::Button(_) => ControlKind::Button,
                    Kind::GridButton(_) => ControlKind::GridButton,
                    Kind::TextButton(_) => ControlKind::TextButton,
                    Kind::CheckBox(_) => ControlKind::CheckBox,
                    Kind::RadioButton(_) => ControlKind::RadioButton,
                    _ => ControlKind::Other,
                },
            };
            self.host.button_sound(ButtonEvent::Click, flags);
        }
    }

    /// `CheckBoxControl::DoClick` (slot 0): flips the check and calls the handler.
    fn check_box_click(&mut self, id: ControlId) {
        let radio = matches!(self.tree[id].kind, Kind::RadioButton(_));
        let checked = match &self.tree[id].kind {
            Kind::CheckBox(c) | Kind::RadioButton(c) => c.checked,
            _ => return,
        };
        if radio {
            self.radio_select(id);
        } else {
            self.check_set_and_call(id, !checked);
        }
    }

    /// `CheckBoxControl::SetAndCall`.
    pub fn check_set_and_call(&mut self, id: ControlId, check: bool) {
        let owner = self.tree[id].owner;
        let (handler, v1, v2) = match &mut self.tree[id].kind {
            Kind::CheckBox(c) | Kind::RadioButton(c) => {
                c.checked = check;
                (c.handler.clone(), c.base.void1, c.base.void2)
            }
            _ => return,
        };
        if let Some(h) = handler {
            self.queue_call(
                owner,
                &h,
                vec![
                    LuaArg::Bool(check),
                    LuaArg::Int(v1),
                    LuaArg::Int(v2),
                    LuaArg::Control(id),
                ],
            );
        }
    }

    /// `RadioButtonControl::DoClick`: this one is on, the others of the group are off.
    pub fn radio_select(&mut self, id: ControlId) {
        let group = match &self.tree[id].kind {
            Kind::RadioButton(c) => c.radio_group.clone(),
            _ => None,
        };
        if let Some(g) = &group {
            let all: Vec<ControlId> = self.tree.ids().collect();
            for o in all {
                if o != id
                    && let Kind::RadioButton(c) = &mut self.tree[o].kind
                    && c.radio_group.as_ref() == Some(g)
                {
                    c.checked = false;
                }
            }
        }
        self.check_set_and_call(id, true);
    }

    /// The hover bookkeeping at the end of a mouse move: leave, then enter.
    pub fn finish_mouse_move(&mut self) -> (Vec<ControlId>, Vec<ControlId>) {
        let prev = std::mem::take(&mut self.hover);
        let req = std::mem::take(&mut self.hover_requests);
        let lost: Vec<ControlId> = prev.iter().copied().filter(|p| !req.contains(p)).collect();
        let gained: Vec<ControlId> = req.iter().copied().filter(|r| !prev.contains(r)).collect();
        self.hover = req;
        (lost, gained)
    }
}

impl Shared {
    /// `UIManager::ProcessNormalInput`: true when the UI took the message.
    pub fn process_message(&self, msg: u32, w: u32, l: i32) -> bool {
        if msg == 7 {
            let mut st = self.st.borrow_mut();
            st.shift = false;
            st.alt = false;
            st.ctrl = false;
            return true;
        }
        let mouse = is_mouse(msg);
        let key = (0x100..0x10a).contains(&msg);
        if !mouse && !key {
            return false;
        }
        let mut moved = false;
        {
            let mut st = self.st.borrow_mut();
            if mouse {
                st.key_state = w;
                if msg == WM_MOUSEMOVE {
                    let p = Vec2::new(f32::from(l as i16), f32::from((l >> 16) as i16));
                    st.mouse_delta = st.mouse - p;
                    st.mouse = p;
                    moved = true;
                }
            }
            if msg & !4 == 0x101 {
                match w {
                    0x10 => st.shift = false,
                    0x12 => st.alt = false,
                    0x11 => st.ctrl = false,
                    _ => {}
                }
            } else if msg & !4 == 0x100 {
                match w {
                    0x10 => st.shift = true,
                    0x12 => st.alt = true,
                    0x11 => st.ctrl = true,
                    _ => {}
                }
            }
        }
        // Keyboard focus (an edit box) takes the keys first.
        let focus = self
            .st
            .borrow()
            .focus
            .filter(|&f| self.st.borrow().alive(f));
        let mut route_then_consume = false;
        if let Some(f) = focus
            && key
        {
            if msg != WM_KEYDOWN {
                if msg == WM_CHAR {
                    self.edit_char(f, w);
                }
                self.flush();
                return true;
            }
            if (0x21..0x2f).contains(&w) {
                self.edit_control_key(f, w);
                self.flush();
                return true;
            }
            if w == 0x1b || w == 0xd {
                route_then_consume = true;
            } else if w == 9 {
                self.focus_next();
                return true;
            } else {
                return true;
            }
        }
        if msg == WM_LBUTTONUP
            && let Some(f) = focus
        {
            // EditBoxControl::TestLostFocus
            self.test_lost_focus(f);
        }
        // Modal, popup, roots.
        let mut handled = false;
        let mut blocked = false;
        let top = self.st.borrow().modals.last().copied();
        if let Some(m) = top
            && self.st.borrow().alive(m)
            && self.st.borrow().node(m).has(MODAL)
        {
            handled = self.input_node(m, msg, w, l, Vec2::ZERO);
            blocked = !self.st.borrow().node(m).has(MODAL_PASS);
        }
        if !handled && !blocked {
            let popup = self.st.borrow().popup_current;
            if let Some(p) = popup
                && self.st.borrow().alive(p)
            {
                handled = self.input_node(p, msg, w, l, Vec2::ZERO);
            }
            if !handled {
                let roots = self.st.borrow().roots.clone();
                for r in roots {
                    let ok = {
                        let st = self.st.borrow();
                        st.alive(r) && !st.node(r).has(HIDDEN)
                    };
                    if ok && self.input_node(r, msg, w, l, Vec2::ZERO) {
                        handled = true;
                        break;
                    }
                }
            }
        }
        if blocked && !handled {
            // SIMPLIFIED(SIMP-uic-2): the exe lets the message through to the world when a
            // blocking modal does not use it; the world code may test `IsModal` itself. The
            // client's world must not react under a popup, so the message is taken here.
            handled = true;
        }
        if route_then_consume {
            self.flush();
            return true;
        }
        if msg != WM_LBUTTONDOWN || handled {
            if moved {
                let (lost, gained) = self.st.borrow_mut().finish_mouse_move();
                for c in lost {
                    self.run(|st| {
                        if st.alive(c) {
                            st.lost_mouse_over(c);
                        }
                    });
                }
                for c in gained {
                    self.run(|st| {
                        if st.alive(c) {
                            st.granted_mouse_over(c);
                        }
                    });
                }
                self.run(|st| st.tooltips_mouse_move());
            }
        } else {
            let m = self.st.borrow().mouse;
            self.st.borrow_mut().mouse_down = m;
        }
        self.flush();
        handled
    }

    /// `ProcessInput` of a control by its class.
    pub fn input_node(&self, id: ControlId, msg: u32, w: u32, l: i32, origin: Vec2) -> bool {
        enum Mode {
            Base,
            Stack,
            Scroll,
            Context,
            Button,
            Slider,
            PullDown,
            EditBox,
            Anchor,
        }
        let mode = {
            let st = self.st.borrow();
            let Some(n) = st.tree.get(id) else {
                return false;
            };
            match &n.kind {
                Kind::Stack(_) => Mode::Stack,
                Kind::ScrollPanel(_) => Mode::Scroll,
                Kind::Context(_) => Mode::Context,
                Kind::Button(_)
                | Kind::TextButton(_)
                | Kind::GridButton(_)
                | Kind::CheckBox(_)
                | Kind::RadioButton(_)
                | Kind::ColorBoxButton(_) => Mode::Button,
                Kind::Slider(_) => Mode::Slider,
                Kind::PullDown(_) => Mode::PullDown,
                Kind::EditBox(_) => Mode::EditBox,
                Kind::WorldAnchor(_) => Mode::Anchor,
                _ => Mode::Base,
            }
        };
        match mode {
            Mode::Base => {
                self.base_input(id, msg, w, l, origin)
                    || self.run(|st| st.consume_mouse(id, msg, origin))
            }
            Mode::Anchor => {
                if self.st.borrow().node(id).has(MOUSE_OVER_HIDDEN) {
                    return false;
                }
                self.base_input(id, msg, w, l, origin)
                    || self.run(|st| st.consume_mouse(id, msg, origin))
            }
            Mode::Stack => self.stack_input(id, msg, w, l, origin),
            Mode::Scroll => self.scroll_input(id, msg, w, l, origin),
            Mode::Context => {
                if self.base_input(id, msg, w, l, origin)
                    || self.run(|st| st.consume_mouse(id, msg, origin))
                {
                    return true;
                }
                self.context_handler(id, msg, w, l)
            }
            Mode::Button => {
                if self.base_input(id, msg, w, l, origin) {
                    return true;
                }
                if is_mouse(msg) || is_pointer(msg) {
                    let r = self.run(|st| {
                        let inside = st.test_hit_virtual(id, st.mouse, origin);
                        st.button_process_mouse(id, msg, inside, origin)
                    });
                    if r {
                        return true;
                    }
                }
                self.run(|st| st.consume_mouse(id, msg, origin))
            }
            Mode::Slider => self.slider_input(id, msg, w, l, origin),
            Mode::PullDown => self.pull_down_input(id, msg, w, l, origin),
            Mode::EditBox => {
                let r = self.run(|st| {
                    let f = st.tree[id].flags;
                    if f & (DISABLED | HIDDEN) == 0
                        && (msg == WM_LBUTTONDOWN || msg == 0x246)
                        && st.test_hit(id, st.mouse, origin)
                    {
                        st.focus = Some(id);
                        true
                    } else {
                        false
                    }
                });
                r || self.base_input(id, msg, w, l, origin)
                    || self.run(|st| st.consume_mouse(id, msg, origin))
            }
        }
    }

    /// `ControlBase::BaseProcessInput`: the children from the last, then hover and tip requests.
    pub fn base_input(&self, id: ControlId, msg: u32, w: u32, l: i32, origin: Vec2) -> bool {
        let (pos, kids) = {
            let st = self.st.borrow();
            let n = st.node(id);
            (n.pos, n.children.clone())
        };
        let o2 = pos + origin;
        for c in kids.into_iter().rev() {
            let ok = {
                let st = self.st.borrow();
                st.alive(c) && !st.node(c).has(NOT_ACTIVE)
            };
            if ok && self.input_node(c, msg, w, l, o2) {
                return true;
            }
        }
        if msg == WM_MOUSEMOVE || msg == 0x245 {
            let mut st = self.st.borrow_mut();
            let f = st.node(id).flags;
            if f & (NEEDS_MOUSE_OVER | HAS_TOOLTIP) != 0
                && st.test_hit_virtual(id, st.mouse, origin)
            {
                if f & NEEDS_MOUSE_OVER != 0 {
                    st.request_mouse_over(id);
                }
                if f & HAS_TOOLTIP != 0 {
                    st.request_tooltip(id);
                }
            }
        }
        false
    }

    /// `StackControl::ProcessInput`: the children in order, at their flow positions.
    fn stack_input(&self, id: ControlId, msg: u32, w: u32, l: i32, origin: Vec2) -> bool {
        let (kids, offsets, content, pos) = {
            let st = self.st.borrow();
            let n = st.node(id);
            let Kind::Stack(s) = &n.kind else {
                return false;
            };
            (n.children.clone(), st.stack_offsets(id), s.content, n.pos)
        };
        let base = content + origin;
        let self_pos = origin + pos;
        for (c, off) in kids.into_iter().zip(offsets) {
            let (f, sz, alive) = {
                let st = self.st.borrow();
                match st.tree.get(c) {
                    Some(n) => (n.flags, n.size, true),
                    None => (0, Vec2::ZERO, false),
                }
            };
            if !alive || f & HIDDEN != 0 || sz.x == 0.0 || sz.y == 0.0 || f & MOUSE_OVER_HIDDEN != 0
            {
                continue;
            }
            let o = if f & 0x240 != 0 { self_pos } else { off + base };
            if self.input_node(c, msg, w, l, o) {
                return true;
            }
        }
        false
    }

    /// `ScrollPanelControl::ProcessInput`.
    fn scroll_input(&self, id: ControlId, msg: u32, w: u32, l: i32, origin: Vec2) -> bool {
        let (pos, scroll, parts, flags, kids, step, value) = {
            let st = self.st.borrow();
            let n = st.node(id);
            let Kind::ScrollPanel(s) = &n.kind else {
                return false;
            };
            (
                n.pos,
                s.scroll,
                [s.bar, s.up, s.down],
                n.flags,
                n.children.clone(),
                s.step,
                s.value,
            )
        };
        let child_origin = pos + origin;
        let mouse = self.st.borrow().mouse;
        let mut handled = false;
        if flags & DISABLED == 0 {
            let mut wheel_target = false;
            if msg == WM_MOUSEWHEEL {
                let st = self.st.borrow();
                wheel_target = st.test_hit(id, mouse, origin)
                    || parts
                        .iter()
                        .flatten()
                        .any(|&p| st.test_hit(p, mouse, child_origin));
            }
            if wheel_target {
                let up = (w as i32 >> 16) > 0;
                self.run(|st| {
                    st.scroll_set_value(id, if up { value - step } else { value + step })
                });
                handled = true;
            } else if flags & sp_flag::BAR_HIDDEN == 0 {
                for p in parts.into_iter().flatten() {
                    if self.input_node(p, msg, w, l, child_origin) {
                        handled = true;
                        break;
                    }
                }
            }
        }
        if flags & sp_flag::HAS_OUTER != 0 {
            for c in kids.iter().rev() {
                let ok = {
                    let st = self.st.borrow();
                    st.alive(*c) && st.node(*c).has(0x240) && !st.node(*c).has(NOT_ACTIVE)
                };
                if ok && self.input_node(*c, msg, w, l, child_origin) {
                    return true;
                }
            }
        }
        if !handled && self.st.borrow().test_hit(id, mouse, origin) {
            let o = scroll + origin;
            handled =
                self.base_input(id, msg, w, l, o) || self.run(|st| st.consume_mouse(id, msg, o));
        }
        handled
    }

    /// `LuaContext::ProcessInput` after the children: the Lua handler.
    fn context_handler(&self, id: ControlId, msg: u32, w: u32, l: i32) -> bool {
        let key = {
            let st = self.st.borrow();
            let n = st.node(id);
            if n.flags & ctx_flag::HAS_INPUT == 0 {
                return false;
            }
            match &n.kind {
                Kind::Context(c) => c.input_handler.clone(),
                _ => None,
            }
        };
        let in_range = (0x100..0x10a).contains(&msg)
            || (0x241..0x250).contains(&msg)
            || (0x200..0x20f).contains(&msg);
        let Some(key) = key else { return false };
        if !in_range {
            return false;
        }
        // The release that ends a camera drag comes off-screen (client-app `ui_event_after_drag`): it clears the
        // buttons' state but is no click, and the exe never hands it to Lua (ProcessCameraDrag drops it), or
        // WorldView.lua would select or order at the hex under the cursor.
        let mouse = self.st.borrow().mouse;
        if (msg == WM_LBUTTONUP || msg == 0x247) && (mouse.x < 0.0 || mouse.y < 0.0) {
            return false;
        }
        let lc = self.st.borrow().ctx(id).and_then(|c| c.lua.clone());
        let Some(lc) = lc else { return false };
        match lc.call_key::<_, lua_sys::LuaValue<'_>>(&key, (msg, w, l)) {
            Ok(v) => v.is_truthy(),
            Err(_) => false,
        }
    }

    /// `SliderControl::ProcessInput`.
    fn slider_input(&self, id: ControlId, msg: u32, w: u32, l: i32, origin: Vec2) -> bool {
        let r = self.run(|st| st.slider_process(id, msg, w, origin));
        match r {
            Some(v) => v,
            None => {
                self.base_input(id, msg, w, l, origin)
                    || self.run(|st| st.consume_mouse(id, msg, origin))
            }
        }
    }

    /// `PullDownControl::ProcessInput`: the main button first; open, the popup takes everything.
    fn pull_down_input(&self, id: ControlId, msg: u32, w: u32, l: i32, origin: Vec2) -> bool {
        let (main, grid, open, pos, cached) = {
            let st = self.st.borrow();
            let n = st.node(id);
            let Kind::PullDown(p) = &n.kind else {
                return false;
            };
            (p.main, p.grid, p.open, n.pos, p.origin)
        };
        let o = if open { cached } else { origin };
        if !open && let Kind::PullDown(p) = &mut self.st.borrow_mut().tree[id].kind {
            p.origin = origin;
        }
        let local = pos + o;
        if let Some(m) = main
            && self.input_node(m, msg, w, l, local)
        {
            return true;
        }
        if !open {
            return self.base_input(id, msg, w, l, origin)
                || self.run(|st| st.consume_mouse(id, msg, origin));
        }
        if self.input_node(grid, msg, w, l, local) {
            return true;
        }
        self.run(|st| st.pull_down_process(id, msg, w, o));
        true
    }

    // ---- keyboard focus and edit boxes ----

    fn edit_char(&self, id: ControlId, ch: u32) {
        self.run(|st| st.edit_char(id, ch));
    }

    fn edit_control_key(&self, id: ControlId, vk: u32) {
        self.run(|st| st.edit_control_key(id, vk));
    }

    fn test_lost_focus(&self, id: ControlId) {
        self.run(|st| {
            let origin = Vec2::ZERO;
            let hit = st.tree.get(id).is_some_and(|n| {
                let o = st.absolute_origin(id);
                let _ = n;
                st.test_hit(id, st.mouse, o)
            });
            let _ = origin;
            if !hit {
                st.edit_invoke_callback(id, true);
                st.focus = None;
            }
        });
    }

    /// Tab: the next control with a `FocusStop` (`ControlBase::FindNextFocus`).
    fn focus_next(&self) {
        self.run(|st| {
            let Some(f) = st.focus else { return };
            let Kind::EditBox(e) = &st.tree[f].kind else {
                return;
            };
            let stop = e.focus_stop;
            if stop < 0 {
                return;
            }
            let forward = !st.shift;
            // The search starts at the root of the focused control's tree.
            let mut top = f;
            while let Some(p) = st.tree[top].parent {
                top = p;
            }
            if let Some(next) = st.find_next_focus(top, stop, forward) {
                st.focus = Some(next);
            }
        });
    }
}

impl State {
    /// Where a control's own position is added to give its screen position: the sum of the
    /// parents' positions (valid while no scroll panel or stack moves the child).
    pub fn absolute_origin(&self, id: ControlId) -> Vec2 {
        let mut o = Vec2::ZERO;
        let mut cur = self.tree[id].parent;
        while let Some(p) = cur {
            o += self.tree[p].pos;
            cur = self.tree[p].parent;
        }
        o
    }

    /// Screen rectangle of a control in UI pixels (left, top, width, height) and whether it is shown (no hidden
    /// control on the path from its root). It follows the origins `input_node` hands to the children (stacks and
    /// scroll panels included), so a click at its centre lands on the control. For the playtests and the dumps.
    pub fn screen_rect(&self, id: ControlId) -> Option<([f32; 4], bool)> {
        self.screen_rect_in(id, None)
    }

    /// `screen_rect` for a control inside a popup or modal `top` (taken out of the tree and flagged
    /// hidden-by-mouse-over while it is up): that one node counts as shown unless it is plainly hidden.
    pub fn screen_rect_in(
        &self,
        id: ControlId,
        top: Option<ControlId>,
    ) -> Option<([f32; 4], bool)> {
        let off = |c: ControlId, n: &Node| {
            if Some(c) == top {
                n.has(HIDDEN)
            } else {
                n.has(NOT_ACTIVE)
            }
        };
        let mut chain = vec![id];
        let mut cur = self.tree.get(id)?.parent;
        while let Some(p) = cur {
            chain.push(p);
            cur = self.tree.get(p)?.parent;
        }
        chain.reverse();
        let mut origin = Vec2::ZERO;
        let mut shown = true;
        // A popup that sits inside another context (DiploTrade in LeaderHeadRoot, a game menu hidden behind its own
        // options menu) is drawn on its own: the parents above the popup do not count.
        let top_at = top.and_then(|t| chain.iter().position(|&c| c == t));
        for (i, w) in chain.windows(2).enumerate() {
            let (p, c) = (w[0], w[1]);
            let pn = self.tree.get(p)?;
            let cn = self.tree.get(c)?;
            if top_at.is_none_or(|t| i >= t) {
                shown &= !off(p, pn);
            }
            origin = match &pn.kind {
                Kind::Stack(s) => {
                    if cn.flags & 0x240 != 0 {
                        origin + pn.pos
                    } else {
                        let i = pn.children.iter().position(|&k| k == c)?;
                        self.stack_offsets(p).get(i).copied().unwrap_or(Vec2::ZERO)
                            + s.content
                            + origin
                    }
                }
                Kind::ScrollPanel(s) => {
                    let part = [s.bar, s.up, s.down].contains(&Some(c));
                    if part || cn.flags & 0x240 != 0 {
                        origin + pn.pos
                    } else {
                        s.scroll + origin + pn.pos
                    }
                }
                _ => origin + pn.pos,
            };
        }
        let n = self.tree.get(id)?;
        if Some(id) == top {
            shown = !off(id, n);
        } else {
            shown &= !off(id, n);
        }
        let o = origin + n.pos;
        Some(([o.x, o.y, n.size.x, n.size.y], shown))
    }

    fn focus_stop_of(&self, id: ControlId) -> i32 {
        match &self.tree[id].kind {
            Kind::EditBox(e) => e.focus_stop,
            _ => -1,
        }
    }

    /// `ControlBase::FindNextFocus(int, bool)`: the nearest stop in the direction, wrapping once.
    pub fn find_next_focus(&self, root: ControlId, start: i32, forward: bool) -> Option<ControlId> {
        fn walk(
            st: &State,
            id: ControlId,
            limit: i32,
            forward: bool,
            best: &mut Option<ControlId>,
        ) {
            let kids = st.tree[id].children.clone();
            for &c in &kids {
                let n = &st.tree[c];
                if n.flags & (HIDDEN | DISABLED) != 0 {
                    continue;
                }
                let stop = st.focus_stop_of(c);
                if stop < 0 {
                    continue;
                }
                let better = |b: &Option<ControlId>, f: &dyn Fn(i32, i32) -> bool| match b {
                    None => true,
                    Some(bb) => f(stop, st.focus_stop_of(*bb)),
                };
                if forward {
                    if stop < limit && better(best, &|s, b| b < s) {
                        *best = Some(c);
                    }
                } else if stop > limit && better(best, &|s, b| s < b) {
                    *best = Some(c);
                }
            }
            for &c in &kids {
                walk(st, c, limit, forward, best);
            }
        }
        let mut best = None;
        walk(self, root, start, forward, &mut best);
        if best.is_none() {
            let limit = if forward { i32::MAX } else { -1 };
            walk(self, root, limit, forward, &mut best);
        }
        best
    }

    /// `EditBoxControl::ProcessChar`.
    pub fn edit_char(&mut self, id: ControlId, ch: u32) {
        let Kind::EditBox(e) = &mut self.tree[id].kind else {
            return;
        };
        let label = e.label;
        match ch {
            8 => {
                if e.cursor > 0 {
                    let mut chars: Vec<char> = e.text.chars().collect();
                    let at = e.cursor.min(chars.len());
                    if at > 0 {
                        chars.remove(at - 1);
                        e.cursor = at - 1;
                        e.text = chars.into_iter().collect();
                    }
                }
            }
            13 | 27 => {
                let enter = ch == 13;
                self.edit_invoke_callback(id, enter);
                if !enter {
                    // Escape leaves the focus without the callback value.
                }
                self.focus = None;
                return;
            }
            c if c >= 0x20 => {
                let Some(chr) = char::from_u32(c) else { return };
                if e.number_input && !chr.is_ascii_digit() {
                    return;
                }
                if e.max_length > 0 && e.text.chars().count() as i32 >= e.max_length {
                    return;
                }
                let mut chars: Vec<char> = e.text.chars().collect();
                let at = e.cursor.min(chars.len());
                chars.insert(at, chr);
                e.cursor = at + 1;
                e.text = chars.into_iter().collect();
            }
            _ => return,
        }
        let shown = {
            let Kind::EditBox(e) = &self.tree[id].kind else {
                return;
            };
            if e.obscure {
                "*".repeat(e.text.chars().count())
            } else {
                e.text.clone()
            }
        };
        self.label_change(label, |l| l.text = shown);
        let call = matches!(&self.tree[id].kind, Kind::EditBox(e) if e.call_on_char);
        if call {
            self.edit_invoke_callback(id, false);
        }
    }

    /// `EditBoxControl::ProcessControlChar`: the cursor keys and Delete.
    pub fn edit_control_key(&mut self, id: ControlId, vk: u32) {
        let Kind::EditBox(e) = &mut self.tree[id].kind else {
            return;
        };
        let n = e.text.chars().count();
        match vk {
            0x25 => e.cursor = e.cursor.saturating_sub(1),
            0x27 => e.cursor = (e.cursor + 1).min(n),
            0x24 => e.cursor = 0,
            0x23 => e.cursor = n,
            0x2e if e.cursor < n => {
                let mut chars: Vec<char> = e.text.chars().collect();
                chars.remove(e.cursor);
                e.text = chars.into_iter().collect();
                let (label, shown) = (e.label, e.text.clone());
                self.label_change(label, |l| l.text = shown);
            }
            _ => {}
        }
    }

    /// `EditBoxControl::InvokeCallback(bIsEnter)`: `(text, control, bIsEnter)`.
    pub fn edit_invoke_callback(&mut self, id: ControlId, enter: bool) {
        let owner = self.tree[id].owner;
        let Kind::EditBox(e) = &self.tree[id].kind else {
            return;
        };
        if let Some(cb) = e.callback.clone() {
            let text = e.text.clone();
            self.queue_call(
                owner,
                &cb,
                vec![LuaArg::Str(text), LuaArg::Control(id), LuaArg::Bool(enter)],
            );
        }
    }
}
