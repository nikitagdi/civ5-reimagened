//! `ToolTipManager`: the control under the mouse asks for its tip; the manager shows the tip
//! control (a registered type, or the basic text one) near the mouse and keeps it on the screen.

use glam::Vec2;

use crate::flags::*;
use crate::state::{LuaArg, State};
use crate::tree::*;

impl State {
    /// `ToolTipManager::RegisterBasicControls` (the Lua of `ToolTips.lua` calls it).
    pub fn tooltips_register_basic(
        &mut self,
        root: ControlId,
        content: ControlId,
        text: ControlId,
        grid: ControlId,
    ) {
        let t = &mut self.tooltips;
        t.registered = true;
        t.root = Some(root);
        t.content = Some(content);
        t.text = Some(text);
        t.grid = Some(grid);
        t.types.insert(0, grid);
        self.build_pending_tooltip_types();
    }

    /// `ToolTipManager::RequestToolTip`: the first control asking during a move wins.
    pub fn request_tooltip(&mut self, id: ControlId) {
        if self.tree[id].has(HAS_TOOLTIP) && self.tooltips.requested.is_none() {
            self.tooltips.requested = Some(id);
        }
    }

    /// `ToolTipManager::RefreshToolTip`: the shown control changed its tip.
    pub fn refresh_tooltip(&mut self, id: ControlId) {
        if self.tooltips.current == Some(id) {
            self.setup_tooltip(id);
        }
    }

    /// `ToolTipManager::ControlClosing`.
    pub fn tooltip_control_closing(&mut self, id: ControlId) {
        let t = &mut self.tooltips;
        if t.requested == Some(id) {
            t.requested = None;
        }
        if t.current == Some(id) {
            t.current = None;
            t.shown = false;
            if let Some(r) = t.root
                && r != id
                && let Some(n) = self.tree.get_mut(r)
            {
                n.flags |= HIDDEN;
            }
        }
    }

    /// The control a tip belongs to was hidden (or removed) under the mouse: the tip goes (the exe's tip of a button
    /// that closes its notification does not stay).
    pub fn tooltip_drop_if_hidden(&mut self) {
        if let Some(c) = self.tooltips.current
            && (!self.tree.alive(c) || self.is_hidden(c))
        {
            self.tooltips.requested = None;
            if self.tree.alive(c) {
                self.tooltip_control_closing(c);
            } else {
                self.tooltips.current = None;
                self.tooltips.shown = false;
                if let Some(r) = self.tooltips.root {
                    self.tree[r].flags |= HIDDEN;
                }
            }
        }
    }

    /// `ControlBase::SetUpToolTip`: puts the tip of the control into the manager.
    pub fn setup_tooltip(&mut self, id: ControlId) {
        let n = &self.tree[id];
        if !n.has(HAS_TOOLTIP) {
            return;
        }
        let (crc, text, cb, owner) = (
            n.tt_crc,
            n.tt_string.clone(),
            n.tt_callback.clone(),
            n.owner,
        );
        if crc == 0 {
            if let Some(text) = text {
                self.tooltip_set_string(&text);
            }
        } else {
            self.tooltip_set_type(crc);
        }
        if let Some(cb) = cb {
            self.queue_call(owner, &cb, vec![LuaArg::Control(id)]);
        }
    }

    /// `ToolTipManager::SetToolTipType`: hides the old type control, shows the new one.
    pub fn tooltip_set_type(&mut self, hash: u32) {
        let t = &self.tooltips;
        if !t.registered || t.type_hash == hash || !t.types.contains_key(&hash) {
            return;
        }
        self.tooltip_set_type_internal(hash);
    }

    fn tooltip_set_type_internal(&mut self, hash: u32) {
        let old = self.tooltips.types.get(&self.tooltips.type_hash).copied();
        let new = self.tooltips.types.get(&hash).copied();
        if let Some(o) = old
            && self.tree.alive(o)
        {
            self.tree[o].flags |= HIDDEN;
        }
        if let Some(n) = new
            && self.tree.alive(n)
        {
            self.tree[n].flags &= !HIDDEN;
        }
        self.tooltips.type_hash = hash;
    }

    /// `ToolTipManager::SetToolTipString`: the basic text tip.
    pub fn tooltip_set_string(&mut self, text: &str) {
        if !self.tooltips.registered {
            return;
        }
        self.tooltip_set_type_internal(0);
        if let (Some(t), Some(g)) = (self.tooltips.text, self.tooltips.grid) {
            self.label_change(t, |l| l.text = text.to_owned());
            self.grid_do_auto_size(g);
        }
    }

    /// `ToolTipManager::ProcessMouseMove`: called after each mouse move.
    pub fn tooltips_mouse_move(&mut self) {
        if !self.tooltips.registered {
            return;
        }
        let requested = self.tooltips.requested;
        if requested != self.tooltips.current {
            self.tooltips.current = requested;
            let root = self.tooltips.root;
            match requested {
                None => {
                    if let Some(r) = root {
                        self.tree[r].flags |= HIDDEN;
                    }
                    self.tooltips.shown = false;
                }
                Some(c) => {
                    self.tooltips.shown = true;
                    self.setup_tooltip(c);
                    if let Some(r) = root {
                        self.tree[r].flags &= !HIDDEN;
                        self.branch_reset_animation(r);
                    }
                }
            }
        }
        self.tooltips.requested = None;
        if self.tooltips.shown
            && let Some(r) = self.tooltips.root
        {
            let m = self.mouse;
            self.set_offset(r, m.x, m.y);
            self.tooltip_check_edges();
        }
    }

    /// SIMPLIFIED(SIMP-exe-421): the position of a shown tip is checked again every frame, after the listeners ran. The exe checks it on each
    /// mouse move only, and a listener's `DoAutoSize` (the tip's real size) comes after that check here (the callbacks are queued), so a
    /// tip that the first check saw at its design size would hang over the bottom or right edge until the mouse moved again.
    pub fn tooltips_refresh_position(&mut self) {
        if self.tooltips.registered
            && self.tooltips.shown
            && let Some(r) = self.tooltips.root
        {
            let m = self.mouse;
            self.set_offset(r, m.x, m.y);
            self.tooltip_check_edges();
        }
    }

    /// `ToolTipManager::CheckEdgesInternal`: the tip control sits right and below the mouse
    /// and flips to the other side near the screen edge.
    fn tooltip_check_edges(&mut self) {
        let Some(root) = self.tooltips.root else {
            return;
        };
        let Some(&ty) = self.tooltips.types.get(&self.tooltips.type_hash) else {
            return;
        };
        if !self.tree.alive(ty) {
            return;
        }
        let (sw, sh) = self.screen();
        let (sw, sh) = (sw as f32, sh as f32);
        let m = self.mouse;
        let size = self.tree[ty].size;
        let n = &mut self.tree[ty];
        n.flags &= 0xffff_fe4f;
        let mut off = Vec2::new(30.0, 5.0);
        if m.x + size.x + 30.0 > sw {
            if sw - size.x >= 0.0 {
                n.flags |= H_RIGHT;
                off.x = 10.0;
            } else {
                off.x = -sw;
            }
        }
        if m.y + size.y + 5.0 > sh {
            if sh - size.y >= 0.0 {
                n.flags |= V_BOTTOM;
                off.y = -5.0;
            } else {
                off.y = -sh;
            }
        }
        self.set_offset(ty, off.x, off.y);
        self.reprocess_anchoring(root);
    }
}
