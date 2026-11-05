//! Behaviour of the controls that have a state of their own: sliders and pull-downs.

use glam::Vec2;

use crate::flags::*;
use crate::state::{LuaArg, State};
use crate::tree::*;

impl State {
    /// `SliderControl::SetAndCall`.
    pub fn slider_set_and_call(&mut self, id: ControlId, v: f32) {
        self.slider_set_value(id, v);
        // ScrollPanelControl::BarMoved: the bar of a panel moves its content.
        if let Some(p) = self.tree[id].parent
            && let Kind::ScrollPanel(sp) = &self.tree[p].kind
            && sp.bar == Some(id)
        {
            let vertical = self.tree[p].flags & sp_flag::VERTICAL != 0;
            let c = v.clamp(0.0, 1.0);
            if let Kind::ScrollPanel(sp) = &mut self.tree[p].kind {
                sp.value = c;
                let pos = (sp.min_offset + (sp.max_offset - sp.min_offset) * c).floor();
                if vertical {
                    sp.scroll.y = pos;
                } else {
                    sp.scroll.x = pos;
                }
            }
        }
        let owner = self.tree[id].owner;
        let Kind::Slider(s) = &self.tree[id].kind else {
            return;
        };
        if let Some(cb) = s.callback.clone() {
            let (val, v1, v2) = (s.value, s.base.void1, s.base.void2);
            self.queue_call(
                owner,
                &cb,
                vec![
                    LuaArg::Num(f64::from(val)),
                    LuaArg::Int(v1),
                    LuaArg::Int(v2),
                    LuaArg::Control(id),
                ],
            );
        }
    }

    /// `SliderControl::ProcessInput`; `None` means `ControlBase::ProcessInput` goes on.
    pub fn slider_process(
        &mut self,
        id: ControlId,
        msg: u32,
        w: u32,
        origin: Vec2,
    ) -> Option<bool> {
        let Kind::Slider(s) = &self.tree[id].kind else {
            return None;
        };
        let (track, shuttle, vertical, travel, pad, dragging, wheel, value) = (
            s.track,
            s.shuttle,
            s.vertical,
            s.travel,
            s.padding,
            s.dragging,
            s.wheel_steps,
            s.value,
        );
        let mouse = self.mouse;
        let local = self.tree[id].pos + origin;
        let track_hit = self.test_hit(track, mouse, local);
        let shuttle_hit = self.test_hit(shuttle, mouse, local);
        let f = self.tree[id].flags;
        if f & DISABLED != 0 && track_hit {
            return Some(false);
        }
        // 100 / wheel steps is the divisor of a key or wheel step.
        let step = 1.0 / (100.0 / wheel);
        if msg < 0x200 {
            if msg != 0x101 || f & MOUSE_OVER == 0 {
                return None;
            }
            let delta = match w {
                0x27 | 0x28 => step,
                0x25 | 0x26 => -step,
                _ => return None,
            };
            self.slider_set_and_call(id, value + delta);
            return Some(true);
        }
        if msg == 0x200 || msg == 0x245 {
            if !dragging {
                if (track_hit || shuttle_hit) && self.key_state & 0x13 == 0 {
                    self.request_mouse_over(id);
                }
                return None;
            }
            if travel > 0 {
                let grab = match &self.tree[id].kind {
                    Kind::Slider(s) => s.grab,
                    _ => 0.0,
                };
                let pointer = if vertical {
                    mouse.y - local.y
                } else {
                    mouse.x - local.x
                };
                self.slider_set_and_call(id, (pointer - grab - pad as f32) / travel as f32);
            }
            self.request_mouse_over(id);
            return None;
        }
        if (msg == 0x201 || msg == 0x203 || msg == 0x246)
            && ((track_hit && f & MOUSE_OVER != 0) || shuttle_hit)
        {
            if let Kind::Slider(s) = &mut self.tree[id].kind {
                s.dragging = true;
            }
            self.set_vis_state(id, 2);
            let sh = &self.tree[shuttle];
            let (axis_pos, axis_size, m, l) = if vertical {
                (sh.pos.y, sh.size.y, mouse.y, local.y)
            } else {
                (sh.pos.x, sh.size.x, mouse.x, local.x)
            };
            let grab;
            if shuttle_hit {
                grab = (m - l - axis_pos).trunc();
            } else {
                grab = (axis_size as i32 / 2) as f32;
                if travel > 0 {
                    self.slider_set_and_call(id, ((m - l) - grab - pad as f32) / travel as f32);
                }
            }
            if let Kind::Slider(s) = &mut self.tree[id].kind {
                s.grab = grab;
            }
            return Some(true);
        }
        if (msg == 0x202 || msg == 0x247) && dragging {
            if track_hit || shuttle_hit {
                self.granted_mouse_over(id);
            } else {
                self.lost_mouse_over(id);
            }
            if let Kind::Slider(s) = &mut self.tree[id].kind {
                s.dragging = false;
            }
            self.slider_set_and_call(id, value);
            return Some(true);
        }
        if msg == 0x20a && f & MOUSE_OVER != 0 {
            let up = (w as i32 >> 16) >= 1;
            self.slider_set_and_call(id, if up { value - step } else { value + step });
            return Some(true);
        }
        None
    }

    // ---- pull-down ----

    /// `PullDownControl::ToggleOpen`.
    pub fn pull_down_toggle(&mut self, id: ControlId) {
        let open = match &mut self.tree[id].kind {
            Kind::PullDown(p) => {
                p.open = !p.open;
                p.open
            }
            _ => return,
        };
        if open {
            self.push_modal(id, false);
        } else {
            self.pop_modal(id);
        }
    }

    /// `PullDownControl::SubButtonPressed`: the selection callback, then close.
    pub fn pull_down_sub_pressed(&mut self, id: ControlId, v1: i32, v2: i32) {
        let owner = self.tree[id].owner;
        if let Kind::PullDown(p) = &self.tree[id].kind
            && let Some(cb) = p.callback.clone()
        {
            self.queue_call(
                owner,
                &cb,
                vec![LuaArg::Int(v1), LuaArg::Int(v2), LuaArg::Control(id)],
            );
        }
        self.pull_down_toggle(id);
    }

    /// `PullDownControl::ProcessInput`; `None` means the generic path.
    pub fn pull_down_process(
        &mut self,
        id: ControlId,
        msg: u32,
        w: u32,
        origin: Vec2,
    ) -> Option<bool> {
        let Kind::PullDown(p) = &self.tree[id].kind else {
            return None;
        };
        if !p.open {
            return None;
        }
        // Open: the popup takes everything; Escape and the right button close it, and so does
        // a left click that starts and ends outside it.
        let (grid, main) = (p.grid, p.main);
        let local = self.tree[id].pos + origin;
        let mouse = self.mouse;
        let hit_grid = self.test_hit(grid, mouse, local);
        let hit_main = main.is_some_and(|m| self.test_hit(m, mouse, local));
        if msg == 0x205 || (msg == 0x101 && w == 0x1b) {
            self.pull_down_toggle(id);
            return Some(true);
        }
        if msg == 0x202 && !hit_grid && !hit_main {
            self.pull_down_toggle(id);
            return Some(true);
        }
        None
    }

    /// `PullDownControl::ClearEntries`.
    pub fn pull_down_clear(&mut self, id: ControlId) {
        let Kind::PullDown(p) = &mut self.tree[id].kind else {
            return;
        };
        let stack = p.stack;
        p.entries.clear();
        self.release_all_children(stack);
    }

    /// `PullDownControl::CalculateInternals`: sizes the popup to its entries.
    pub fn pull_down_calculate(&mut self, id: ControlId) {
        let Kind::PullDown(p) = &self.tree[id].kind else {
            return;
        };
        let (grid, scroll, stack, thr, main) =
            (p.grid, p.scroll, p.stack, p.scroll_threshold, p.main);
        self.stack_calculate_size(stack);
        let ss = self.tree[stack].size;
        let h = ss.y.min(thr as f32);
        self.set_size_val(scroll, ss.x, h);
        self.scroll_panel_calc_internal(scroll);
        self.grid_do_auto_size(grid);
        let below = main.map(|m| self.tree[m].size.y).unwrap_or(0.0);
        self.set_offset(grid, 0.0, below);
    }
}
