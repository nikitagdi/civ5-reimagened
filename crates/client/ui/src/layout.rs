//! Layout as the exe does it: imperative, run when `Initialize` or an explicit call asks for it.
//! Nothing is recomputed per frame; Lua of the original relies on the stale sizes in between.

use glam::Vec2;

use crate::flags::{self, *};
use crate::state::State;
use crate::text::{TextAlign, TextRequest};
use crate::tree::*;

/// `UIManager`'s anchor scale (`s_fAnchorScale`): centres at half the free space.
const ANCHOR_SCALE: f32 = 0.5;
/// Added to the mouse-hit rectangle (`unk_0912c9f4`): the right and bottom edges are exclusive.
pub const HIT_EPSILON: f32 = -1.0;
/// Bias added to the position of quads and grid cells (`0x0921f10c`).
pub const QUAD_BIAS: f32 = -0.374;
/// `StackControl::s_AnchorDir`: down, up, left, right.
const STACK_DIR: [Vec2; 4] = [
    Vec2::new(0.0, 1.0),
    Vec2::new(0.0, -1.0),
    Vec2::new(-1.0, 0.0),
    Vec2::new(1.0, 0.0),
];

impl State {
    // ---- ControlBase ----

    /// `ControlBase::ProcessAnchoring` and the classes that extend it.
    pub fn process_anchoring(&mut self, id: ControlId) {
        self.base_process_anchoring(id);
        match &self.tree[id].kind {
            Kind::Stack(_) => self.stack_content_origin(id),
            Kind::ScrollPanel(s) => {
                // ScrollPanelControl::ProcessAnchoring: its parts are anchored to it.
                let parts = [s.up, s.down, s.bar];
                for p in parts.into_iter().flatten() {
                    self.reprocess_anchoring(p);
                }
            }
            _ => {}
        }
    }

    fn base_process_anchoring(&mut self, id: ControlId) {
        let (f, offset, size, parent) = {
            let n = &self.tree[id];
            (n.flags, n.offset, n.size, n.parent)
        };
        let mut pos = offset;
        if let Some(p) = parent {
            let pn = &self.tree[p];
            let mut anchor = pn.size;
            let mut in_scroll = false;
            // A scroll panel anchors its children to the size of its content, except the outer ones.
            if let Kind::ScrollPanel(sp) = &pn.kind
                && f & OUTER_BITS == 0
            {
                anchor = sp.inner;
                in_scroll = true;
            }
            if f & H_RIGHT != 0 {
                pos.x = if f & H_OUTER != 0 {
                    anchor.x + pos.x
                } else if in_scroll {
                    // SIMPLIFIED(SIMP-uic-9): a child of a scroll panel keeps its right margin inward whatever the sign
                    // of its Offset (the panel's content size counts |Offset|, CalculateInternalSize): the EUI
                    // notification stack (SmallStack Anchor="R,B" Offset="-6,0" in a panel as wide as its icons) went
                    // 6 px out of the panel and the screen and clipped the icons' right edge (live report 2026-10-30).
                    // The exe's own rule is not read off the exe.
                    (anchor.x - size.x) - pos.x.abs()
                } else {
                    (anchor.x - size.x) - pos.x
                };
            } else if f & H_CENTER != 0 {
                pos.x += ((anchor.x - size.x) * ANCHOR_SCALE).round();
            } else if f & H_OUTER != 0 {
                pos.x = -pos.x - size.x;
            }
            if f & V_BOTTOM != 0 {
                pos.y = if f & V_OUTER != 0 {
                    anchor.y + pos.y
                } else {
                    (anchor.y - size.y) - pos.y
                };
            } else if f & V_CENTER != 0 {
                pos.y += ((anchor.y - size.y) * ANCHOR_SCALE).round();
            } else if f & V_OUTER != 0 {
                pos.y = -pos.y - size.y;
            }
        }
        self.tree[id].pos = pos;
    }

    /// `ControlBase::CalculateVisibilityBox`: own box united with the children's; the parent
    /// follows only when the box changed.
    pub fn calc_vis_box(&mut self, id: ControlId) {
        let n = &self.tree[id];
        let old = n.vis;
        let mut b = [n.pos.x, n.pos.y, n.pos.x + n.size.x, n.pos.y + n.size.y];
        for &c in &n.children {
            let cv = self.tree[c].vis;
            b[0] = b[0].min(cv[0]);
            b[1] = b[1].min(cv[1]);
            b[2] = b[2].max(cv[2]);
            b[3] = b[3].max(cv[3]);
        }
        let parent = n.parent;
        self.tree[id].vis = b;
        if b != old
            && let Some(p) = parent
        {
            self.calc_vis_box(p);
        }
    }

    /// `ControlBase::ReprocessAnchoring`: self, box, then the children.
    pub fn reprocess_anchoring(&mut self, id: ControlId) {
        self.process_anchoring(id);
        self.calc_vis_box(id);
        for c in self.tree[id].children.clone() {
            self.reprocess_anchoring(c);
        }
    }

    /// `ControlBase::SetSizeVal` and the overrides.
    pub fn set_size_val(&mut self, id: ControlId, w: f32, h: f32) {
        match &mut self.tree[id].kind {
            Kind::Image(i) => {
                i.quad.width = w;
                i.quad.height = h;
            }
            Kind::Anim(a) => {
                if let Some(q) = &mut a.quad {
                    q.width = w;
                    q.height = h;
                }
            }
            _ => {}
        }
        if matches!(self.tree[id].kind, Kind::ScrollPanel(_)) {
            self.scroll_panel_set_size(id, w, h);
            return;
        }
        self.base_set_size(id, w, h);
        if matches!(self.tree[id].kind, Kind::Grid(_)) {
            self.grid_adjust(id);
        }
    }

    fn base_set_size(&mut self, id: ControlId, w: f32, h: f32) {
        let n = &mut self.tree[id];
        n.size = Vec2::new(w, h);
        n.full = n.offset.abs() + n.size;
        self.reprocess_anchoring(id);
        self.calc_vis_box(id);
    }

    /// `ControlBase::SetOffset`: the offset is floored.
    pub fn set_offset(&mut self, id: ControlId, x: f32, y: f32) {
        let n = &mut self.tree[id];
        if n.offset.x == x && n.offset.y == y {
            return;
        }
        n.offset = Vec2::new(x.floor(), y.floor());
        n.full = n.offset.abs() + n.size;
        self.process_anchoring(id);
        self.calc_vis_box(id);
    }

    /// `ControlBase::SetAnchor`.
    pub fn set_anchor(&mut self, id: ControlId, text: &str) {
        let n = &mut self.tree[id];
        n.flags &= !ANCHOR_BITS;
        n.flags |= crate::values::anchor_flags(text);
    }

    /// `OnSystemUpdateUI`: the sizes written as `Full` follow the screen.
    pub fn apply_screen_size(&mut self, id: ControlId) {
        let (w, h) = self.screen();
        let n = &self.tree[id];
        if n.flags & (SIZE_FULL_X | SIZE_FULL_Y) != 0 {
            let x = if n.flags & SIZE_FULL_X != 0 {
                w as f32
            } else {
                n.size.x
            };
            let y = if n.flags & SIZE_FULL_Y != 0 {
                h as f32
            } else {
                n.size.y
            };
            self.set_size_val(id, x, y);
        }
        for c in self.tree[id].children.clone() {
            self.apply_screen_size(c);
        }
    }

    /// `ControlBase::TestHit`: the point is in the box at `origin + pos`.
    pub fn test_hit(&self, id: ControlId, mouse: Vec2, origin: Vec2) -> bool {
        let n = &self.tree[id];
        let l = n.pos.x + origin.x;
        let t = n.pos.y + origin.y;
        l <= mouse.x
            && t <= mouse.y
            && mouse.x <= l + n.size.x + HIT_EPSILON
            && mouse.y <= t + n.size.y + HIT_EPSILON
    }

    // ---- initialisation in phases (the recursion is the caller's) ----

    /// `Initialize` of a subtree without contexts.
    pub fn initialize_plain(&mut self, id: ControlId) {
        self.init_pre(id);
        self.init_self(id);
        for c in self.tree[id].children.clone() {
            self.initialize_plain(c);
            self.init_union(id, c);
        }
        self.init_post(id);
    }

    /// What a class does before `ControlBase::Initialize`.
    pub fn init_pre(&mut self, id: ControlId) {
        match &self.tree[id].kind {
            Kind::Label(_) => self.label_prepare(id),
            Kind::Button(b) => {
                let text = b.text;
                let size = self.tree[id].size;
                let f = self.tree[id].flags;
                if let Kind::Button(b) = &mut self.tree[id].kind
                    && b.quad.texture.is_none()
                {
                    self.tree[id].flags |= button_flag::NO_QUAD;
                }
                let f2 = self.tree[id].flags;
                if let Kind::Button(b) = &mut self.tree[id].kind {
                    if f2 & button_flag::STATE_INC == 0 {
                        b.state_inc.y = size.y;
                    }
                    b.quad.width = size.x;
                    b.quad.height = size.y;
                    b.quad.src_w = size.x;
                    b.quad.src_h = size.y;
                    b.quad.src_x = b.tex_offset.x;
                    b.quad.src_y = b.tex_offset.y;
                    if f2 & button_flag::NO_STATE_CHANGE != 0 {
                        b.quad.src_x = b.tex_offset.x;
                        b.quad.src_y = b.tex_offset.y;
                    }
                    b.quad.normalize();
                }
                let _ = f;
                if let Some(t) = text {
                    let owner = self.tree[id].owner;
                    self.tree[t].parent = Some(id);
                    self.tree[t].owner = owner;
                    if self.tree[id].flags & button_flag::TEXT_ANCHOR == 0 {
                        self.tree[t].flags |= H_CENTER | V_CENTER;
                    }
                    self.initialize_plain(t);
                }
            }
            Kind::TextButton(t) => {
                let label = t.label;
                self.initialize_plain(label);
                let s = self.tree[label].size;
                self.tree[id].size = s;
            }
            Kind::GridButton(g) => {
                let (grid, text) = (g.grid, g.text);
                let size = self.tree[id].size;
                self.tree[grid].size = size;
                self.initialize_plain(grid);
                if let Some(t) = text {
                    let owner = self.tree[id].owner;
                    self.tree[t].parent = Some(id);
                    self.tree[t].owner = owner;
                    self.tree[t].flags |= H_CENTER | V_CENTER;
                    self.initialize_plain(t);
                }
            }
            Kind::CheckBox(c) | Kind::RadioButton(c) => {
                let (boxed, label) = (c.boxed, c.label);
                let size = self.tree[boxed].size;
                self.tree[id].size = size;
                self.initialize_plain(boxed);
                self.initialize_plain(label);
                let ms = match &self.tree[id].kind {
                    Kind::CheckBox(c) | Kind::RadioButton(c) => c.mark_size,
                    _ => Vec2::ZERO,
                };
                if let Kind::CheckBox(c) | Kind::RadioButton(c) = &mut self.tree[id].kind {
                    c.mark.width = ms.x;
                    c.mark.height = ms.y;
                    c.mark.src_w = ms.x;
                    c.mark.src_h = ms.y;
                    c.mark.normalize();
                }
            }
            Kind::Slider(s) => {
                let (track, shuttle, vertical) = (s.track, s.shuttle, s.vertical);
                self.tree[track].flags &= !0x3f0;
                self.initialize_plain(track);
                let n = &mut self.tree[shuttle];
                n.flags = if vertical {
                    (n.flags & !0x70) | H_CENTER
                } else {
                    (n.flags & !0x380) | V_CENTER
                };
                self.initialize_plain(shuttle);
                let size = self.tree[id].size;
                let ssize = self.tree[shuttle].size;
                if let Kind::Slider(s) = &mut self.tree[id].kind {
                    let pad = 2 * s.padding;
                    s.travel = if vertical {
                        (size.y - ssize.y) as i32 - pad
                    } else {
                        (size.x - ssize.x) as i32 - pad
                    };
                }
            }
            Kind::EditBox(e) => {
                let l = e.label;
                self.initialize_plain(l);
            }
            Kind::ScrollPanel(s) => {
                let (up, down, bar) = (s.up, s.down, s.bar);
                if let Some(u) = up
                    && let Some(b) = self.tree[u].kind.button_base_mut()
                {
                    b.native = Native::ScrollUp(id);
                }
                if let Some(d) = down
                    && let Some(b) = self.tree[d].kind.button_base_mut()
                {
                    b.native = Native::ScrollDown(id);
                }
                for p in [up, down, bar].into_iter().flatten() {
                    self.initialize_plain(p);
                }
            }
            Kind::PullDown(p) => {
                let (main, grid, scroll, stack) = (p.main, p.grid, p.scroll, p.stack);
                let size = self.tree[id].size;
                if let Some(m) = main {
                    self.tree[m].size = size;
                    if let Some(b) = self.tree[m].kind.button_base_mut() {
                        b.native = Native::PullDownMain(id);
                    }
                    self.initialize_plain(m);
                }
                for c in [grid, scroll, stack] {
                    self.tree[c].parent = Some(match c {
                        x if x == grid => id,
                        x if x == scroll => grid,
                        _ => scroll,
                    });
                }
                self.initialize_plain(grid);
            }
            Kind::Image(i) => {
                // ImageControl::Initialize: the quad takes the size, or `Resize` fits the stretch mode.
                let _ = i;
            }
            Kind::Anim(a) => {
                if a.variant == AnimVariant::Slide {
                    let begin = a.slide_begin;
                    if let Some(p) = self.tree[id].parent {
                        let ps = self.tree[p].size;
                        self.tree[id].size = ps;
                    }
                    self.tree[id].offset = begin;
                }
                let s = self.tree[id].size;
                if let Kind::Anim(a) = &mut self.tree[id].kind
                    && let Some(q) = &mut a.quad
                {
                    q.width = s.x;
                    q.height = s.y;
                    q.src_w = s.x;
                    q.src_h = s.y;
                    q.normalize();
                }
            }
            Kind::Bar(_) | Kind::Meter(_) | Kind::TextureBar(_) | Kind::ColorBox(_) => {}
            Kind::WorldAnchor(_) => {
                self.tree[id].flags &= !0x3f0;
            }
            _ => {}
        }
    }

    /// `ControlBase::Initialize` without the children.
    pub fn init_self(&mut self, id: ControlId) {
        {
            let n = &mut self.tree[id];
            n.flags |= INITIALIZED;
        }
        self.process_anchoring(id);
        let n = &mut self.tree[id];
        n.full = n.offset.abs() + n.size;
        n.vis = [n.pos.x, n.pos.y, n.size.x + n.pos.x, n.size.y + n.pos.y];
    }

    /// The box of a child joins the parent's (`ControlBase::Initialize` loop).
    pub fn init_union(&mut self, id: ControlId, child: ControlId) {
        let cv = self.tree[child].vis;
        let v = &mut self.tree[id].vis;
        v[0] = v[0].min(cv[0]);
        v[1] = v[1].min(cv[1]);
        v[2] = v[2].max(cv[2]);
        v[3] = v[3].max(cv[3]);
    }

    /// What a class does after `ControlBase::Initialize`.
    pub fn init_post(&mut self, id: ControlId) {
        match &self.tree[id].kind {
            Kind::Stack(_) => {
                if !self.tree[id].children.is_empty() {
                    self.stack_calculate_size(id);
                    self.reprocess_anchoring(id);
                }
                self.stack_content_origin(id);
            }
            Kind::Label(_) => self.label_rebuild(id),
            Kind::Grid(_) => self.grid_initialize(id),
            Kind::Image(_) => self.image_initialize(id),
            Kind::ScrollPanel(_) => {
                self.scroll_panel_calc_internal(id);
                self.reprocess_anchoring(id);
            }
            Kind::Button(_)
            | Kind::TextButton(_)
            | Kind::GridButton(_)
            | Kind::CheckBox(_)
            | Kind::RadioButton(_) => {
                let st = self.tree[id]
                    .kind
                    .button_base()
                    .map(|b| b.vis_state)
                    .unwrap_or(0);
                self.set_vis_state(id, st);
            }
            Kind::Anim(_) => {
                if let Kind::Anim(a) = &mut self.tree[id].kind {
                    a.pause_remaining = a.pause;
                    if a.variant == AnimVariant::Flip && a.flip_columns == 0 {
                        a.flip_columns = 1;
                    }
                }
                self.anim_update(id, 0.0);
            }
            Kind::WorldAnchor(_) => self.world_anchor_bounds(id),
            Kind::Slider(s) => {
                let v = s.value;
                self.slider_set_value(id, v);
            }
            _ => {}
        }
    }

    fn image_initialize(&mut self, id: ControlId) {
        let size = self.tree[id].size;
        let Kind::Image(i) = &mut self.tree[id].kind else {
            return;
        };
        if i.stretch == 0 {
            i.quad.width = size.x;
            i.quad.height = size.y;
            i.quad.src_w = size.x;
            i.quad.src_h = size.y;
        } else {
            self.image_resize(id, size.x, size.y);
        }
        if let Kind::Image(i) = &mut self.tree[id].kind {
            i.quad.normalize();
        }
    }

    /// `ImageControl::Resize`: the quad fits the size by the stretch mode.
    pub fn image_resize(&mut self, id: ControlId, w: f32, h: f32) {
        self.set_size_val(id, w, h);
        let size = self.tree[id].size;
        let Kind::Image(i) = &mut self.tree[id].kind else {
            return;
        };
        let (tw, th) = (i.tex_w, i.tex_h);
        let q = &mut i.quad;
        match i.stretch {
            1 => {
                let cw = size.x.min(tw);
                let ch = size.y.min(th);
                q.width = cw;
                q.height = ch;
                q.src_w = cw;
                q.src_h = ch;
            }
            2 => {
                let fit_h = size.x * th / tw;
                if fit_h <= size.y {
                    q.width = size.x;
                    q.height = fit_h;
                } else {
                    q.width = tw * size.y / th;
                    q.height = size.y;
                }
                q.src_w = tw;
                q.src_h = th;
            }
            3 => {
                q.width = size.x;
                q.height = size.y;
                q.src_w = tw;
                q.src_h = th;
            }
            4 => {
                let fit_h = size.x * th / tw;
                if size.y <= fit_h {
                    q.src_h = th * (size.y / fit_h);
                    q.src_w = tw;
                } else {
                    q.src_w = (size.x / (size.y * tw / th)) * tw;
                    q.src_h = th;
                }
                q.width = size.x;
                q.height = size.y;
            }
            _ => {}
        }
        q.normalize();
    }

    /// `ImageControl::SetTextureAndResize`.
    pub fn image_set_texture_and_resize(&mut self, id: ControlId, name: &str) -> bool {
        let Some(t) = self.texture(name) else {
            return false;
        };
        let (w, h) = (t.w, t.h);
        if let Kind::Image(i) = &mut self.tree[id].kind {
            i.quad.set_texture(Some(t), None);
            i.tex_w = w;
            i.tex_h = h;
        }
        self.set_size_val(id, w, h);
        if let Kind::Image(i) = &mut self.tree[id].kind {
            i.quad.src_w = w;
            i.quad.src_h = h;
            i.quad.normalize();
        }
        true
    }

    // ---- stack ----

    /// `StackControl::ProcessAnchoring` second half: where the flow starts.
    fn stack_content_origin(&mut self, id: ControlId) {
        let n = &self.tree[id];
        let Kind::Stack(s) = &n.kind else { return };
        let dir = STACK_DIR[s.dir.clamp(0, 3) as usize];
        let wrap = STACK_DIR[s.wrap_dir.clamp(0, 3) as usize];
        let mut content = n.pos;
        let along = dir.dot(n.size);
        if along < 0.0 {
            content += dir * along;
        }
        if s.wrap_width > 0 {
            let w = wrap.dot(n.size);
            if w < 0.0 {
                content += wrap * w;
            }
        }
        if let Kind::Stack(s) = &mut self.tree[id].kind {
            s.content = content;
        }
    }

    /// The offsets `CalculateNextStackOffset` gives each child, in order. Children that do not
    /// take part in the flow get zero.
    pub fn stack_offsets(&self, id: ControlId) -> Vec<Vec2> {
        let n = &self.tree[id];
        let Kind::Stack(s) = &n.kind else {
            return Vec::new();
        };
        let dir = STACK_DIR[s.dir.clamp(0, 3) as usize];
        let wrap = STACK_DIR[s.wrap_dir.clamp(0, 3) as usize];
        let mut cursor = Vec2::ZERO;
        let mut wrap_base = Vec2::ZERO;
        let mut wrap_extent = Vec2::ZERO;
        let mut out = Vec::with_capacity(n.children.len());
        for &c in &n.children {
            let ch = &self.tree[c];
            if cursor == Vec2::ZERO {
                wrap_base = Vec2::ZERO;
                wrap_extent = Vec2::ZERO;
            }
            if ch.flags & 0x242 != 0 || ch.size.x == 0.0 || ch.size.y == 0.0 {
                out.push(Vec2::ZERO);
                continue;
            }
            let extent = ch.pos + ch.size + s.padding;
            let along = dir.dot(extent);
            let step = along.abs();
            let stepv = dir * step;
            let mut result = cursor;
            let mut origin = cursor;
            if s.wrap_width > 0 {
                let reach = dir.dot(cursor + stepv);
                if reach.abs() > s.wrap_width as f32 {
                    origin = wrap_extent;
                    result = origin;
                    wrap_base = wrap_extent;
                }
                let wrap_dot = wrap.dot(extent);
                if wrap_dot < 0.0 {
                    let origin_along = dir.dot(origin);
                    origin = wrap_base + dir * origin_along - wrap * wrap_dot;
                    result = origin;
                }
            }
            cursor = origin + stepv;
            if along < 0.0 {
                result = cursor;
            }
            if s.wrap_width > 0 {
                let cur = if along < 0.0 { cursor } else { origin };
                let s1 = wrap.dot(s.padding + ch.full + cur);
                let s2 = wrap.dot(cur);
                let s3 = wrap.dot(wrap_extent);
                wrap_extent = wrap * s1.max(s2).max(s3);
            }
            out.push(result);
        }
        out
    }

    /// `StackControl::CalculateSize`.
    pub fn stack_calculate_size(&mut self, id: ControlId) {
        let offsets = self.stack_offsets(id);
        let n = &self.tree[id];
        let old = n.vis;
        let mut vis = [n.pos.x.abs(), n.pos.y.abs(), n.pos.x.abs(), n.pos.y.abs()];
        let mut flow_min = Vec2::ZERO;
        let mut flow_max = Vec2::ZERO;
        for (&c, &off) in n.children.iter().zip(&offsets) {
            let ch = &self.tree[c];
            if ch.flags & HIDDEN != 0 || ch.size.x == 0.0 || ch.size.y == 0.0 {
                continue;
            }
            if ch.flags & 0x240 == 0 {
                flow_min = flow_min.min(off);
                flow_max = flow_max.max(ch.full + off);
            }
            vis[0] = vis[0].min(ch.vis[0] + off.x);
            vis[1] = vis[1].min(ch.vis[1] + off.y);
            vis[2] = vis[2].max(off.x + ch.vis[2]);
            vis[3] = vis[3].max(off.y + ch.vis[3]);
        }
        let size = (flow_max - flow_min).abs();
        let full = n.offset.abs() + size;
        let new_vis = [0.0, 0.0, (vis[2] - vis[0]).abs(), (vis[3] - vis[1]).abs()];
        let parent = n.parent;
        let nn = &mut self.tree[id];
        nn.size = size;
        nn.full = full;
        nn.vis = new_vis;
        if new_vis != old
            && let Some(p) = parent
        {
            self.calc_vis_box(p);
        }
    }

    // ---- grid ----

    /// `GridControl::Initialize` after `ControlBase::Initialize`.
    fn grid_initialize(&mut self, id: ControlId) {
        if self.tree[id].flags & grid_flag::AUTO_SIZE != 0 {
            self.grid_auto_size(id);
        }
        let Kind::Grid(g) = &mut self.tree[id].kind else {
            return;
        };
        let off = g.tex_offset;
        for i in 0..9 {
            g.cell_min[i] += off;
            g.cell_max[i] = g.cell_min[i] + g.cell_size[i];
        }
        let no_state = self.tree[id].flags & grid_flag::NO_STATE_CHANGE != 0;
        let Kind::Grid(g) = &mut self.tree[id].kind else {
            return;
        };
        if no_state {
            g.cur_offset = g.tex_offset;
        }
        self.grid_adjust_inner(id);
    }

    /// The stacks under `id` (through plain containers and stacks), innermost first, are calculated and re-anchored.
    fn refresh_flow(&mut self, id: ControlId) {
        for c in self.tree[id].children.clone() {
            let n = &self.tree[c];
            if n.flags & INITIALIZED == 0 {
                continue;
            }
            match n.kind {
                Kind::Container => self.refresh_flow(c),
                Kind::Stack(_) => {
                    self.refresh_flow(c);
                    if !self.tree[c].children.is_empty() {
                        self.stack_calculate_size(c);
                        self.reprocess_anchoring(c);
                    }
                }
                _ => {}
            }
        }
    }

    /// `GridControl::CalculateAutoSize`.
    pub fn grid_auto_size(&mut self, id: ControlId) {
        // SIMPLIFIED(SIMP-uic-10): the stacks inside the grid are measured again first. A label's `SetText` does not
        // resize the stack it sits in here (the stack is only recalculated by `CalculateSize`), and EUI's
        // `UnitFlagToolTip` (and other tips) set the text and call `Box:DoAutoSize()` only: the unit tooltip frame
        // stayed at the size of its design text (live report 2026-10-30). The exe's way is not read off the exe.
        self.refresh_flow(id);
        let n = &self.tree[id];
        let Kind::Grid(g) = &n.kind else { return };
        let mut max = Vec2::ZERO;
        for &c in &n.children {
            let ch = &self.tree[c];
            if ch.flags & 0x240 == 0 {
                max = max.max(ch.full);
            }
        }
        let size = g.auto_margin * 2.0 + max;
        let nn = &mut self.tree[id];
        nn.size = size;
        nn.full = nn.offset.abs() + size;
        self.reprocess_anchoring(id);
        self.calc_vis_box(id);
    }

    /// `GridControl::DoAutoSize` (the Lua method): auto size, then the cells.
    pub fn grid_do_auto_size(&mut self, id: ControlId) {
        self.grid_auto_size(id);
        self.grid_adjust(id);
    }

    /// `GridControl::AdjustComponents`: only for initialised grids.
    pub fn grid_adjust(&mut self, id: ControlId) {
        if self.tree[id].flags & INITIALIZED == 0 {
            return;
        }
        self.grid_adjust_inner(id);
    }

    fn grid_adjust_inner(&mut self, id: ControlId) {
        let size = self.tree[id].size;
        let Kind::Grid(g) = &mut self.tree[id].kind else {
            return;
        };
        let (hk, vk) = (g.hkerning, g.vkerning);
        let cs = &mut g.cell_size;
        // C 0, UC 1, LC 2, L 3, R 4, UL 5, UR 6, LL 7, LR 8
        cs[1].x = size.x - cs[6].x + hk[0] - cs[5].x + hk[1];
        cs[3].y = size.y - cs[5].y + vk[0] - cs[7].y + vk[1];
        cs[0].x = size.x - cs[3].x - cs[4].x + hk[0] + hk[1];
        cs[0].y = size.y - cs[1].y - cs[2].y + vk[0] + vk[1];
        cs[4].y = size.y - cs[6].y + vk[0] - cs[8].y + vk[1];
        cs[2].x = size.x - cs[8].x + hk[0] - cs[7].x + hk[1];
        let co = &mut g.cell_offset;
        co[5] = Vec2::ZERO;
        co[1] = Vec2::new(cs[5].x - hk[0], 0.0);
        co[6] = Vec2::new(size.x - cs[6].x, 0.0);
        co[3] = Vec2::new(0.0, cs[5].y - vk[0]);
        co[0] = Vec2::new(cs[3].x - hk[0], cs[1].y - vk[0]);
        co[4] = Vec2::new(size.x - cs[4].x, cs[6].y - vk[0]);
        co[7] = Vec2::new(0.0, size.y - cs[7].y);
        co[2] = Vec2::new(cs[7].x - hk[0], size.y - cs[2].y);
        co[8] = Vec2::new(size.x - cs[8].x, size.y - cs[8].y);
        self.grid_normalize(id);
    }

    /// `GridControl::NormalizeTextureCoords`.
    pub fn grid_normalize(&mut self, id: ControlId) {
        let Kind::Grid(g) = &mut self.tree[id].kind else {
            return;
        };
        let Some(t) = &g.texture else { return };
        let tex = Vec2::new(t.w, t.h);
        if tex.x == 0.0 || tex.y == 0.0 {
            return;
        }
        for i in 0..9 {
            g.uv_min[i] = (g.cur_offset + g.cell_min[i]) / tex;
            g.uv_max[i] = (g.cell_max[i] - g.cell_min[i]) / tex;
        }
    }

    /// `GridControl::SetStateIndex`.
    pub fn grid_set_state(&mut self, id: ControlId, state: u32) {
        let no_state = self.tree[id].flags & grid_flag::NO_STATE_CHANGE != 0;
        let Kind::Grid(g) = &mut self.tree[id].kind else {
            return;
        };
        if no_state {
            return;
        }
        g.cur_offset = g.state_step * state as f32 + g.tex_offset;
        self.grid_normalize(id);
    }

    // ---- buttons ----

    /// `ButtonControlBase::SetVisState` and the overrides: 0 normal, 1 over, 2 pressed, 3 disabled.
    pub fn set_vis_state(&mut self, id: ControlId, state: u32) {
        let f = self.tree[id].flags;
        if let Some(b) = self.tree[id].kind.button_base_mut() {
            b.vis_state = state;
        }
        match &mut self.tree[id].kind {
            Kind::Button(b) if f & button_flag::NO_STATE_CHANGE == 0 => {
                b.quad.src_x = b.tex_offset.x + b.state_inc.x * state as f32;
                b.quad.src_y = b.tex_offset.y + b.state_inc.y * state as f32;
                b.quad.normalize();
            }
            Kind::GridButton(g) => {
                let grid = g.grid;
                self.grid_set_state(grid, state);
            }
            Kind::TextButton(t) => {
                let (label, set, style) = (
                    t.label,
                    t.sets[state.min(3) as usize],
                    t.styles[state.min(3) as usize].clone(),
                );
                self.label_mut(label, |l| {
                    l.colors = set;
                    l.color_explicit = true;
                    if !style.is_empty() {
                        l.style = Some(style);
                    }
                });
                if self.tree[label].flags & INITIALIZED != 0 {
                    self.label_rebuild(label);
                }
            }
            Kind::CheckBox(c) | Kind::RadioButton(c) => {
                let (boxed, label) = (c.boxed, c.label);
                self.set_vis_state(boxed, state);
                self.set_vis_state(label, state);
            }
            _ => {}
        }
    }

    // ---- scroll panel ----

    /// `ScrollPanelControl::SetSizeVal`.
    fn scroll_panel_set_size(&mut self, id: ControlId, w: f32, h: f32) {
        let flags = self.tree[id].flags;
        let Kind::ScrollPanel(s) = &self.tree[id].kind else {
            return;
        };
        let (up, down, bar) = (s.up, s.down, s.bar);
        {
            let n = &mut self.tree[id];
            n.size = Vec2::new(w, h);
        }
        if let Some(bar) = bar {
            let vertical = flags & sp_flag::VERTICAL != 0;
            let mut len = if vertical { h } else { w };
            for b in [up, down].into_iter().flatten() {
                len -= if vertical {
                    self.tree[b].size.y
                } else {
                    self.tree[b].size.x
                };
            }
            self.slider_set_length(bar, len);
        }
        let n = &mut self.tree[id];
        n.full = n.offset.abs() + n.size;
        self.process_anchoring(id);
        self.calc_vis_box(id);
    }

    /// `ScrollPanelControl::CalculateInternalSize`.
    pub fn scroll_panel_calc_internal(&mut self, id: ControlId) {
        let n = &self.tree[id];
        let flags = n.flags;
        let size = n.size;
        let Kind::ScrollPanel(sp) = &n.kind else {
            return;
        };
        let (up, down, bar) = (sp.up, sp.down, sp.bar);
        let mut max = size;
        let mut has_outer = false;
        let mut bounds = [0.0, size.x, 0.0, size.y];
        for &c in &n.children {
            let ch = &self.tree[c];
            if ch.flags & HIDDEN != 0 {
                continue;
            }
            if ch.flags & 0x240 != 0 {
                has_outer = true;
                continue;
            }
            max = max.max(ch.full);
            bounds[1] = max.x;
            bounds[3] = max.y;
        }
        let vertical = flags & sp_flag::VERTICAL != 0;
        let (max_offset, inner, ratio, step);
        if vertical {
            max_offset = size.y - max.y;
            inner = Vec2::new(size.x, max.y);
            ratio = if max.y != 0.0 { size.y / max.y } else { size.y };
            step = if ratio >= 0.7 {
                0.5
            } else {
                size.y * 0.1 / max.y
            };
        } else {
            max_offset = size.x - max.x;
            inner = Vec2::new(max.x, size.y);
            ratio = if max.x != 0.0 { size.x / max.x } else { size.x };
            step = if ratio >= 0.7 {
                0.5
            } else {
                size.x * 0.1 / max.x
            };
        }
        let value;
        {
            let Kind::ScrollPanel(sp) = &mut self.tree[id].kind else {
                return;
            };
            sp.min_offset = 0.0;
            sp.max_offset = max_offset;
            sp.inner = inner;
            sp.bounds = bounds;
            sp.ratio = ratio;
            sp.step = step;
            value = sp.value;
        }
        if has_outer {
            self.tree[id].flags |= sp_flag::HAS_OUTER;
        }
        if let Some(bar) = bar {
            self.slider_set_shuttle_ratio(bar, ratio);
            if let Kind::Slider(s) = &mut self.tree[bar].kind {
                s.wheel_steps = (1.0 / step).floor();
            }
        }
        let full = ratio >= 1.0 || ratio == 0.0;
        let auto = flags & sp_flag::NO_AUTO_BAR == 0;
        if auto {
            if full {
                self.tree[id].flags |= sp_flag::BAR_HIDDEN;
            } else {
                self.tree[id].flags &= !sp_flag::BAR_HIDDEN;
            }
        } else {
            for p in [up, down, bar].into_iter().flatten() {
                self.set_disabled(p, full);
            }
        }
        self.scroll_set_value(id, value);
    }

    /// `ScrollPanelControl::SetScrollValue`: clamps 0..1 and places the content with `floor`.
    pub fn scroll_set_value(&mut self, id: ControlId, v: f32) {
        let vertical = self.tree[id].flags & sp_flag::VERTICAL != 0;
        let Kind::ScrollPanel(s) = &mut self.tree[id].kind else {
            return;
        };
        let c = v.clamp(0.0, 1.0);
        s.value = c;
        let pos = (s.min_offset + (s.max_offset - s.min_offset) * c).floor();
        if vertical {
            s.scroll.y = pos;
        } else {
            s.scroll.x = pos;
        }
        let bar = s.bar;
        if let Some(bar) = bar {
            self.slider_set_value(bar, v);
        }
    }

    // ---- sliders ----

    pub fn slider_set_length(&mut self, id: ControlId, len: f32) {
        let Kind::Slider(s) = &self.tree[id].kind else {
            return;
        };
        let (vertical, track) = (s.vertical, s.track);
        let n = &mut self.tree[id];
        if vertical {
            n.size.y = len;
        } else {
            n.size.x = len;
        }
        let sz = n.size;
        self.tree[track].size = sz;
    }

    /// `SliderControl::SetShuttleRatio`: the shuttle is `ratio` of the control along the axis.
    pub fn slider_set_shuttle_ratio(&mut self, id: ControlId, ratio: f32) {
        let Kind::Slider(s) = &self.tree[id].kind else {
            return;
        };
        let (shuttle, vertical, pad, value) = (s.shuttle, s.vertical, s.padding, s.value);
        let size = self.tree[id].size;
        let sz = self.tree[shuttle].size;
        let (len, ns) = if vertical {
            (size.y, Vec2::new(sz.x, ratio * size.y))
        } else {
            (size.x, Vec2::new(ratio * size.x, sz.y))
        };
        self.set_size_val(shuttle, ns.x, ns.y);
        let shuttle_len = if vertical { ns.y } else { ns.x };
        if let Kind::Slider(s) = &mut self.tree[id].kind {
            s.travel = (len - shuttle_len - 2.0 * pad as f32) as i32;
        }
        self.slider_set_value(id, value);
    }

    /// `SliderControl::SetValue`: puts the shuttle at the value along the travel.
    pub fn slider_set_value(&mut self, id: ControlId, v: f32) {
        let Kind::Slider(s) = &mut self.tree[id].kind else {
            return;
        };
        let v = v.clamp(0.0, 1.0);
        s.value = v;
        let (shuttle, vertical, travel, pad) = (s.shuttle, s.vertical, s.travel, s.padding);
        let pos = (travel as f32 * v + pad as f32).floor();
        if vertical {
            let x = self.tree[shuttle].offset.x;
            self.set_offset(shuttle, x, pos);
        } else {
            let y = self.tree[shuttle].offset.y;
            self.set_offset(shuttle, pos, y);
        }
    }

    // ---- disabled, hidden ----

    /// `ControlBase::SetDisabled`.
    pub fn set_disabled(&mut self, id: ControlId, disabled: bool) {
        let n = &mut self.tree[id];
        let old = n.flags;
        if disabled {
            n.flags |= DISABLED;
            if old & MOUSE_OVER != 0 {
                n.flags = (old & !MOUSE_OVER) | DISABLED;
                self.hover.retain(|&h| h != id);
                self.show_mouse_over_children(id, false);
            }
        } else {
            n.flags &= !DISABLED;
        }
        if self.tree[id].kind.button_base().is_some() {
            let st = if disabled { 3 } else { 0 };
            self.set_vis_state(id, st);
        }
    }

    /// `ControlBase::ShowMouseOverChildren`.
    pub fn show_mouse_over_children(&mut self, id: ControlId, show: bool) {
        let n = &self.tree[id];
        let (shown, hidden) = (n.show_on_over.clone(), n.hide_on_over.clone());
        if show {
            for c in shown {
                self.tree[c].flags &= !MOUSE_OVER_HIDDEN;
            }
            for c in hidden {
                self.tree[c].flags |= MOUSE_OVER_HIDDEN;
            }
        } else {
            for c in shown {
                self.tree[c].flags |= MOUSE_OVER_HIDDEN;
                self.branch_reset_animation(c);
            }
            for c in hidden {
                self.tree[c].flags &= !MOUSE_OVER_HIDDEN;
                self.branch_reset_animation(c);
            }
        }
    }

    /// `ControlBase::GrantedMouseOver` and the button override.
    pub fn granted_mouse_over(&mut self, id: ControlId) {
        let old = self.tree[id].flags;
        if self.tree[id].kind.button_base().is_some() {
            if old & DISABLED == 0 {
                let bound = {
                    let b = self.tree[id].kind.button_base().expect("button");
                    let has = |slot: usize| b.callbacks[slot].is_some();
                    (old & btn::LEFT_DOWN != 0 && has(0))
                        || (old & btn::RIGHT_DOWN != 0 && has(2))
                        || (old & btn::MIDDLE_DOWN != 0 && has(3))
                };
                self.set_vis_state(id, if bound { 2 } else { 1 });
            }
            self.call_button_slot(id, 5);
        }
        let n = &mut self.tree[id];
        n.flags |= MOUSE_OVER;
        if old & DISABLED == 0 {
            self.show_mouse_over_children(id, true);
        }
    }

    /// `ControlBase::LostMouseOver` and the button override.
    pub fn lost_mouse_over(&mut self, id: ControlId) {
        if self.tree[id].kind.button_base().is_some() {
            let d = self.tree[id].flags & DISABLED != 0;
            self.set_vis_state(id, if d { 3 } else { 0 });
            self.call_button_slot(id, 6);
        }
        let n = &mut self.tree[id];
        let old = n.flags;
        n.flags &= !MOUSE_OVER;
        if old & DISABLED == 0 {
            self.show_mouse_over_children(id, false);
        }
    }

    /// `ButtonControlBase::CallDelegates` for a slot: queues the Lua callback.
    pub fn call_button_slot(&mut self, id: ControlId, slot: usize) {
        let slot = slot.min(7);
        // The exe's own handler runs first (the delegate), then the Lua one.
        if let Some(b) = self.tree[id].kind.button_base()
            && slot == 0
        {
            let (native, v1, v2) = (b.native, b.void1, b.void2);
            match native {
                Native::PullDownMain(pd) => self.pull_down_toggle(pd),
                Native::PullDownSub(pd) => self.pull_down_sub_pressed(pd, v1, v2),
                Native::ScrollUp(sp) | Native::ScrollDown(sp) => {
                    if let Kind::ScrollPanel(s) = &self.tree[sp].kind {
                        let (value, step) = (s.value, s.step);
                        let up = matches!(native, Native::ScrollUp(_));
                        self.scroll_set_value(sp, if up { value - step } else { value + step });
                    }
                }
                Native::None => {}
            }
        }
        let owner = self.tree[id].owner;
        let Some(b) = self.tree[id].kind.button_base() else {
            return;
        };
        let Some(cb) = b.callbacks[slot].clone() else {
            return;
        };
        let (v1, v2, grab) = (b.void1, b.void2, b.grab);
        self.queue_call(
            owner,
            &cb,
            vec![
                crate::state::LuaArg::Int(v1),
                crate::state::LuaArg::Int(v2),
                crate::state::LuaArg::Control(id),
                crate::state::LuaArg::Num(f64::from(grab.x)),
                crate::state::LuaArg::Num(f64::from(grab.y)),
            ],
        );
    }

    /// `ControlBase::BranchResetAnimation` (and the animation override).
    pub fn branch_reset_animation(&mut self, id: ControlId) {
        if let Kind::Anim(a) = &mut self.tree[id].kind {
            a.progress = 0.0;
            a.reversing = false;
            a.stopped = false;
            a.pause_remaining = a.pause;
            self.anim_update(id, 0.0);
            if let Kind::Anim(a) = &mut self.tree[id].kind {
                a.stopped = false;
            }
        }
        for c in self.tree[id].children.clone() {
            if self.tree[c].flags & NOT_ACTIVE == 0 {
                self.branch_reset_animation(c);
            }
        }
    }

    // ---- tree edits ----

    /// `ControlBase::ReleaseChild`: takes the child out and destroys it.
    pub fn release_child(&mut self, parent: ControlId, child: ControlId) {
        let Some(p) = self.tree.get_mut(parent) else {
            return;
        };
        if let Some(i) = p.children.iter().position(|&c| c == child) {
            p.children.remove(i);
            self.destroy(child);
        }
    }

    /// `ControlBase::ReleaseAllChildren` (`DestroyAllChildren`).
    pub fn release_all_children(&mut self, id: ControlId) {
        let kids = std::mem::take(&mut self.tree[id].children);
        for c in kids {
            self.destroy(c);
        }
    }

    /// `ControlBase::ChangeParent`.
    pub fn change_parent(&mut self, id: ControlId, new_parent: ControlId) {
        if let Some(old) = self.tree[id].parent {
            self.tree[old].children.retain(|&c| c != id);
        }
        self.add_child(new_parent, id);
    }

    /// Frees a subtree and every reference to it (the destructor of `ControlBase`).
    pub fn destroy(&mut self, id: ControlId) {
        if !self.tree.alive(id) {
            return;
        }
        let mut stack = vec![id];
        let mut doomed = Vec::new();
        while let Some(c) = stack.pop() {
            let Some(n) = self.tree.get(c) else { continue };
            stack.extend(n.children.iter().copied());
            stack.extend(
                n.show_on_over
                    .iter()
                    .copied()
                    .filter(|x| !n.children.contains(x)),
            );
            stack.extend(compound_parts(&n.kind));
            doomed.push(c);
        }
        for d in &doomed {
            self.modals.retain(|m| m != d);
            self.hover.retain(|m| m != d);
            self.hover_requests.retain(|m| m != d);
            self.roots.retain(|m| m != d);
            if self.focus == Some(*d) {
                self.focus = None;
            }
            self.tooltip_control_closing(*d);
        }
        for d in doomed {
            self.tree.free_node(d);
        }
    }

    // ---- labels ----

    /// `TextControl::Initialize` before `ControlBase::Initialize`: fonts and colours from the context.
    fn label_prepare(&mut self, id: ControlId) {
        let owner = self.tree[id].owner;
        let (font, style, set) = match owner.and_then(|o| self.ctx(o)) {
            Some(c) => (
                c.default_font.clone(),
                c.default_style.clone(),
                c.default_set,
            ),
            None => (None, String::new(), Default::default()),
        };
        if let Kind::Label(l) = &mut self.tree[id].kind {
            if l.font.is_none() {
                l.font = Some(font.unwrap_or_else(|| "TwCenMT14".to_owned()));
            }
            if l.style.is_none() && !style.is_empty() {
                l.style = Some(style);
            }
            if !l.color_explicit {
                l.colors = set;
            }
        }
    }

    /// The request for a label's current state.
    pub fn label_request(&self, id: ControlId) -> Option<TextRequest> {
        let n = &self.tree[id];
        let Kind::Label(l) = &n.kind else { return None };
        let align = if n.flags & H_CENTER != 0 {
            TextAlign::Center
        } else if n.flags & H_RIGHT != 0 {
            TextAlign::Right
        } else {
            TextAlign::Left
        };
        Some(TextRequest {
            text: l.text.clone(),
            font: l.font.clone().unwrap_or_else(|| "TwCenMT14".to_owned()),
            style: l.style.clone().unwrap_or_else(|| "Base".to_owned()),
            colors: l.colors.colors,
            wrap_width: l.wrap_width,
            truncate_width: l.truncate_width,
            leading_offset: l.leading_offset,
            align,
            obscured: l.obscured,
            parse_markup: true,
        })
    }

    /// Makes the layout when the request changed (`ParseTextIntoBlocks` .. `CalculateSize`).
    pub fn label_layout(&mut self, id: ControlId) {
        let Some(req) = self.label_request(id) else {
            return;
        };
        let Kind::Label(l) = &self.tree[id].kind else {
            return;
        };
        if l.request.as_ref() == Some(&req) && l.layout.is_some() {
            return;
        }
        let layout = self.text.layout(&req);
        if let Kind::Label(l) = &mut self.tree[id].kind {
            l.layout = Some(std::rc::Rc::new(layout));
            l.request = Some(req);
        }
    }

    /// `TextControl::RebuildText`: only for initialised labels.
    pub fn label_rebuild(&mut self, id: ControlId) {
        if self.tree[id].flags & INITIALIZED == 0 {
            // Before Initialize the request is made at Initialize.
            if !matches!(&self.tree[id].kind, Kind::Label(_)) {
                return;
            }
        }
        self.label_layout(id);
        let Kind::Label(l) = &self.tree[id].kind else {
            return;
        };
        let size = l.layout.as_ref().map(|x| x.size).unwrap_or(Vec2::ZERO);
        let n = &mut self.tree[id];
        let changed = n.size != size;
        n.size = size;
        n.full = n.offset.abs() + size;
        self.process_anchoring(id);
        self.calc_vis_box(id);
        for c in self.tree[id].children.clone() {
            self.reprocess_anchoring(c);
        }
        // SIMPLIFIED(SIMP-uic-21): a label whose text changed its size measures the stack it sits in again. The EUI
        // NotificationPanel civ list puts its status labels (score, trade items) into stacks that have no ID, so no
        // Lua can call `CalculateSize`; with the stale size (0) a stack anchored left and growing left drew its
        // children over the leader portrait (live report 2026-10-31). The exe's own way is not read off the exe.
        if changed
            && let Some(p) = self.tree[id].parent
            && self.tree[p].flags & INITIALIZED != 0
            && matches!(self.tree[p].kind, Kind::Stack(_))
        {
            self.stack_calculate_size(p);
            self.reprocess_anchoring(p);
        }
    }

    /// A label setter from Lua: changes the label and rebuilds it if it is initialised.
    pub fn label_change(&mut self, id: ControlId, f: impl FnOnce(&mut Label)) {
        self.label_mut(id, f);
        if self.tree[id].flags & INITIALIZED != 0 {
            self.label_rebuild(id);
        }
    }

    // ---- world anchor ----

    fn world_anchor_bounds(&mut self, id: ControlId) {
        let n = &self.tree[id];
        let mut b = [32766.0f32, -32766.0, 32766.0, -32766.0];
        for &c in &n.children {
            let ch = &self.tree[c];
            b[0] = b[0].min(ch.pos.x);
            b[1] = b[1].max(ch.pos.x + ch.size.x);
            b[2] = b[2].min(ch.pos.y);
            b[3] = b[3].max(ch.pos.y + ch.size.y);
        }
        if let Kind::WorldAnchor(w) = &mut self.tree[id].kind {
            w.bounds = b;
        }
    }

    /// `WorldAnchorControl::RecalcPosition`: projects the world point; hides the anchor off screen.
    pub fn world_anchor_recalc(&mut self, id: ControlId) {
        let Kind::WorldAnchor(w) = &self.tree[id].kind else {
            return;
        };
        let world = w.world;
        let b = w.bounds;
        let scaling = w.scaling;
        let (sw, sh) = self.screen();
        match self.host.project(world) {
            Some([x, y])
                if x + b[0] <= sw as f32
                    && x + b[1] >= 0.0
                    && y + b[2] <= sh as f32
                    && y + b[3] >= 0.0 =>
            {
                if scaling {
                    // UI pixels per world unit at the point: a step of one unit east, projected.
                    let east = [world[0] + 1.0, world[1], world[2]];
                    let k = self.host.project(east).map_or(1.0, |[ex, ey]| {
                        (Vec2::new(ex, ey) - Vec2::new(x, y)).length() * SCALING_WORLD_PER_PIXEL
                    });
                    if let Kind::WorldAnchor(w) = &mut self.tree[id].kind {
                        w.scale = k;
                    }
                }
                self.tree[id].flags &= !MOUSE_OVER_HIDDEN;
                self.set_offset(id, x.round(), y.round());
            }
            _ => {
                self.tree[id].flags |= MOUSE_OVER_HIDDEN;
            }
        }
    }
}

/// The parts a control owns outside its child list.
pub fn compound_parts(kind: &Kind) -> Vec<ControlId> {
    match kind {
        Kind::Button(b) => b.text.into_iter().collect(),
        Kind::TextButton(t) => vec![t.label],
        Kind::GridButton(g) => [Some(g.grid), g.text].into_iter().flatten().collect(),
        Kind::CheckBox(c) | Kind::RadioButton(c) => vec![c.boxed, c.label],
        Kind::Slider(s) => vec![s.track, s.shuttle],
        Kind::EditBox(e) => vec![e.label],
        Kind::ScrollPanel(s) => [s.up, s.down, s.bar].into_iter().flatten().collect(),
        Kind::PullDown(p) => {
            let mut v = vec![p.grid, p.scroll, p.stack];
            v.extend(p.main);
            v.extend(p.entries.iter().copied());
            v
        }
        _ => Vec::new(),
    }
}

#[allow(dead_code)]
fn _unused(_: u32) -> u32 {
    flags::INITIALIZED
}
