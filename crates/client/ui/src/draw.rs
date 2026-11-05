//! Drawing: the controls emit quads into a `UiDrawList`, in the order of the exe's
//! `Render`/`ClipRender` walk. Coordinates are UI pixels times the UI scale.

use draw_list::{ClipRect, TextureId, UiBlend, UiDrawList, UiMaterial, UiShade, UiVertex};
use glam::Vec2;

use crate::flags::*;
use crate::state::State;
use crate::tree::*;

/// The clip box of a `ClipRender` walk: x0, y0, x1, y1 in UI pixels.
type ClipBox = [f32; 4];

struct Em<'a> {
    out: &'a mut UiDrawList,
    scale: f32,
    /// Added to every point after the scale (physical pixels): the children of a scaling `WorldAnchor` are drawn at
    /// `scale * k` about the anchor's point.
    offset: Vec2,
}

fn rgba(c: [f32; 4], alpha: f32) -> [u8; 4] {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    [q(c[0]), q(c[1]), q(c[2]), q(c[3] * alpha)]
}

impl Em<'_> {
    fn clip(&mut self, clip: Option<ClipBox>) {
        if let Some(c) = clip {
            let s = self.scale;
            self.out.push_clip(ClipRect {
                min: Vec2::new(c[0] * s, c[1] * s) + self.offset,
                max: Vec2::new(c[2] * s, c[3] * s) + self.offset,
            });
        }
    }

    fn unclip(&mut self, clip: Option<ClipBox>) {
        if clip.is_some() {
            self.out.pop_clip();
        }
    }

    /// One textured rectangle: `min`/`max` in UI pixels, UVs, colour.
    #[allow(clippy::too_many_arguments)]
    fn rect(
        &mut self,
        clip: Option<ClipBox>,
        tex: TextureId,
        min: Vec2,
        max: Vec2,
        uv_min: Vec2,
        uv_max: Vec2,
        color: [u8; 4],
        material: &UiMaterial,
    ) {
        if max.x <= min.x || max.y <= min.y {
            return;
        }
        let (s, off) = (self.scale, self.offset);
        let v = |p: Vec2, uv: Vec2| UiVertex {
            pos: (p * s + off).to_array(),
            uv: uv.to_array(),
            color,
        };
        let verts = [
            v(min, uv_min),
            v(Vec2::new(max.x, min.y), Vec2::new(uv_max.x, uv_min.y)),
            v(max, uv_max),
            v(Vec2::new(min.x, max.y), Vec2::new(uv_min.x, uv_max.y)),
        ];
        self.clip(clip);
        self.out
            .add_triangles_with(tex, material, &verts, &[0, 1, 2, 0, 2, 3]);
        self.unclip(clip);
    }

    fn quad(&mut self, clip: Option<ClipBox>, q: &Quad, pos: Vec2, alpha: f32) {
        let tex = match (&q.texture, q.handle) {
            (Some(t), _) => t.id,
            (None, Some(h)) => h,
            (None, None) => return,
        };
        let mut material = UiMaterial {
            sampler: q.sampler,
            ..UiMaterial::PLAIN
        };
        let blend = q.blend_slot == 2 && q.mask.is_some();
        if let (true, Some(m)) = (blend, &q.mask) {
            // The mask sits on the quad at its own pixel size; the renderer maps a screen
            // pixel to the mask with `uv = pos * xy + zw`.
            let s = self.scale;
            let (mw, mh) = (m.w.max(1.0) * s, m.h.max(1.0) * s);
            material.shade = UiShade::Masked;
            material.texture2 = m.id;
            let at = pos * s + self.offset;
            material.tex2_xform = [1.0 / mw, 1.0 / mh, -at.x / mw, -at.y / mh];
            self.out.push_blend(UiBlend::Slot2);
        }
        let (uv0, uv1) = if q.handle.is_some() && q.texture.is_none() {
            (Vec2::ZERO, Vec2::ONE)
        } else {
            (q.uv_offset, q.uv_offset + q.uv_scale)
        };
        self.rect(
            clip,
            tex,
            pos,
            pos + Vec2::new(q.width, q.height),
            uv0,
            uv1,
            rgba(q.color, alpha),
            &material,
        );
        if blend {
            self.out.pop_blend();
        }
    }

    fn fill(&mut self, clip: Option<ClipBox>, pos: Vec2, size: Vec2, color: [f32; 4], alpha: f32) {
        self.rect(
            clip,
            TextureId::WHITE,
            pos,
            pos + size,
            Vec2::ZERO,
            Vec2::ONE,
            rgba(color, alpha),
            &UiMaterial::PLAIN,
        );
    }
}

impl State {
    /// Everything: roots oldest first, the popup, the modals bottom up.
    pub fn draw_all(&mut self, out: &mut UiDrawList, scale: f32) {
        let (w, h) = self.screen();
        let clip = [0.0, 0.0, w as f32, h as f32];
        let mut em = Em {
            out,
            scale,
            offset: Vec2::ZERO,
        };
        // The tooltip layer is the topmost: the root that holds the tooltip's controls draws after the popup and the
        // modals (a tooltip over a popup screen such as the tech tree must not sink under it).
        let tip_root = self.tooltips.root.and_then(|t| {
            let mut c = Some(t);
            while let Some(x) = c {
                if self.roots.contains(&x) {
                    return Some(x);
                }
                c = self.tree[x].parent;
            }
            None
        });
        for r in self.roots.clone().into_iter().rev() {
            if Some(r) == tip_root {
                continue;
            }
            if self.alive(r) && self.tree[r].flags & NOT_ACTIVE == 0 {
                self.render(&mut em, r, Vec2::ZERO, 1.0, Some(clip));
            }
        }
        let mut top: Vec<ControlId> = Vec::new();
        top.extend(self.popup_current);
        top.extend(self.modals.iter().copied());
        for c in top {
            if !self.alive(c) || self.tree[c].flags & HIDDEN != 0 {
                continue;
            }
            let origin = match &self.tree[c].kind {
                Kind::PullDown(p) => p.origin,
                _ => Vec2::ZERO,
            };
            self.render_modal(&mut em, c, origin, clip);
        }
        if let Some(r) = tip_root
            && self.alive(r)
            && self.tree[r].flags & NOT_ACTIVE == 0
        {
            self.render(&mut em, r, Vec2::ZERO, 1.0, Some(clip));
        }
    }

    /// A control taken out of the tree (modal, popup) is drawn though it is flagged hidden-by-mouse-over.
    fn render_modal(&mut self, em: &mut Em<'_>, id: ControlId, origin: Vec2, clip: ClipBox) {
        let saved = self.tree[id].flags;
        self.tree[id].flags &= !MOUSE_OVER_HIDDEN;
        if let Kind::PullDown(_) = self.tree[id].kind {
            self.render_pull_down_open(em, id, origin, 1.0, Some(clip));
        } else {
            self.render(em, id, origin, 1.0, Some(clip));
        }
        self.tree[id].flags = saved;
    }

    /// Children in order, with the culling of `ControlBase::ClipRender`.
    fn render_children(
        &mut self,
        em: &mut Em<'_>,
        id: ControlId,
        origin: Vec2,
        alpha: f32,
        clip: Option<ClipBox>,
    ) {
        let n = &self.tree[id];
        let child_origin = n.pos + origin;
        let child_alpha = n.alpha * alpha;
        for c in n.children.clone() {
            let Some(ch) = self.tree.get(c) else { continue };
            if ch.flags & NOT_ACTIVE != 0 {
                continue;
            }
            if ch.flags & NO_CLIP != 0 {
                self.render(em, c, child_origin, child_alpha, None);
                continue;
            }
            if let Some(b) = clip {
                let v = ch.vis;
                let (l, t) = (child_origin.x + v[0], child_origin.y + v[1]);
                let (r, bt) = (child_origin.x + v[2], child_origin.y + v[3]);
                let inside = b[0] <= l && b[1] <= t && r <= b[2] && bt <= b[3];
                let covers = l <= b[0] && t <= b[1] && b[2] <= r && b[3] <= bt;
                let overlaps = b[0] <= r && l <= b[2] && b[1] <= bt && t <= b[3];
                if !(inside || covers || overlaps) {
                    continue;
                }
            }
            self.render(em, c, child_origin, child_alpha, clip);
        }
    }

    /// `Render`/`ClipRender` of one control. `origin` is the position of the parent.
    fn render(
        &mut self,
        em: &mut Em<'_>,
        id: ControlId,
        origin: Vec2,
        alpha: f32,
        clip: Option<ClipBox>,
    ) {
        let Some(n) = self.tree.get(id) else { return };
        let pos = n.pos + origin;
        let a = n.alpha * alpha;
        let (screen_w, screen_h) = self.screen();
        match &n.kind {
            Kind::Container | Kind::Context(_) | Kind::WorldAnchor(_) | Kind::ReplayMap(_) => {
                if matches!(n.kind, Kind::WorldAnchor(_)) && self.world_anchors_hidden {
                    return;
                }
                if let Kind::WorldAnchor(w) = &n.kind
                    && w.scaling
                    && w.scale != 1.0
                {
                    // `Scaling`: the children at `k` times their size about the anchor's point (no culling: the
                    // anchor itself is hidden off the screen).
                    let k = w.scale;
                    let saved = (em.scale, em.offset);
                    em.offset += pos * em.scale * (1.0 - k);
                    em.scale *= k;
                    self.render_children(em, id, origin, alpha, None);
                    (em.scale, em.offset) = saved;
                    return;
                }
                if let Kind::ReplayMap(m) = &n.kind
                    && !m.plots.is_empty()
                {
                    let (w, h) = (m.map_size.x, m.map_size.y);
                    let cell = Vec2::new(n.size.x / w, n.size.y / h);
                    let (sc, off) = (em.scale, em.offset);
                    let mut verts = Vec::with_capacity(m.plots.len() * 4);
                    let mut idx = Vec::with_capacity(m.plots.len() * 6);
                    for (i, c) in m.plots.iter().enumerate() {
                        if c[3] <= 0.0 {
                            continue;
                        }
                        let (x, y) = ((i as f32) % w, ((i as f32) / w).floor());
                        // The map's row 0 is the south: the replay draws it upside down, north on top.
                        let p0 = pos + Vec2::new(x * cell.x, (h - 1.0 - y) * cell.y);
                        let col = rgba(*c, a);
                        let base = verts.len() as u32;
                        for d in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
                            let p = p0 + Vec2::new(d.0 * cell.x, d.1 * cell.y);
                            verts.push(UiVertex {
                                pos: (p * sc + off).to_array(),
                                uv: [0.0, 0.0],
                                color: col,
                            });
                        }
                        idx.extend_from_slice(&[
                            base,
                            base + 1,
                            base + 2,
                            base,
                            base + 2,
                            base + 3,
                        ]);
                    }
                    if !verts.is_empty() {
                        em.clip(clip);
                        em.out.add_triangles(TextureId::WHITE, &verts, &idx);
                        em.unclip(clip);
                    }
                }
                self.render_children(em, id, origin, alpha, clip);
            }
            Kind::Image(i) => {
                em.quad(clip, &i.quad, pos, a);
                self.render_children(em, id, origin, alpha, clip);
            }
            Kind::Button(b) => {
                if n.flags & button_flag::NO_QUAD == 0 {
                    em.quad(clip, &b.quad, pos, a);
                }
                let text = (n.flags & button_flag::HAS_TEXT != 0)
                    .then_some(b.text)
                    .flatten();
                self.render_children(em, id, origin, alpha, clip);
                if let Some(t) = text {
                    self.render(em, t, pos, a, clip);
                }
            }
            Kind::GridButton(g) => {
                let (grid, text) = (g.grid, g.text);
                self.render(em, grid, pos, a, clip);
                self.render_children(em, id, origin, alpha, clip);
                if let Some(t) = text
                    && self.tree[id].flags & button_flag::HAS_TEXT != 0
                {
                    self.render(em, t, pos, a, clip);
                }
            }
            Kind::TextButton(t) => {
                let l = t.label;
                self.render(em, l, pos, a, clip);
                self.render_children(em, id, origin, alpha, clip);
            }
            Kind::CheckBox(c) | Kind::RadioButton(c) => {
                let (boxed, label, checked) = (c.boxed, c.label, c.checked);
                let (mark, off) = (c.mark.clone(), c.mark_offset);
                self.render(em, boxed, pos, a, clip);
                if checked {
                    em.quad(clip, &mark, pos + off, a);
                }
                // The label hangs below the box; its place is relative to the box.
                let box_pos = pos + self.tree[boxed].pos;
                self.render(em, label, pos, a, clip);
                let _ = box_pos;
                self.render_children(em, id, origin, alpha, clip);
            }
            Kind::Label(_) => {
                self.render_label(em, id, pos, a, clip);
                // XML nests controls inside a Label (a stack under a heading: DiploGlobalRelationships): they are drawn.
                self.render_children(em, id, origin, alpha, clip);
            }
            Kind::EditBox(e) => {
                let l = e.label;
                self.render(em, l, pos, a, clip);
                self.render_children(em, id, origin, alpha, clip);
            }
            Kind::Anim(an) => {
                if let Some(q) = &an.quad {
                    let q = q.clone();
                    em.quad(clip, &q, pos, a);
                }
                self.render_children(em, id, origin, alpha, clip);
            }
            Kind::Grid(g) => {
                if let Some(t) = &g.texture {
                    let tex = t.id;
                    let col = rgba(g.color, a);
                    let cells: Vec<(Vec2, Vec2, Vec2, Vec2)> = (0..9)
                        .map(|i| {
                            (
                                g.cell_offset[i],
                                g.cell_size[i],
                                g.uv_min[i],
                                g.uv_min[i] + g.uv_max[i],
                            )
                        })
                        .collect();
                    for (off, size, u0, u1) in cells {
                        em.rect(
                            clip,
                            tex,
                            pos + off,
                            pos + off + size,
                            u0,
                            u1,
                            col,
                            &UiMaterial::PLAIN,
                        );
                    }
                }
                self.render_children(em, id, origin, alpha, clip);
            }
            Kind::Stack(s) => {
                let offsets = self.stack_offsets(id);
                let base = s.content + origin;
                let self_pos = pos;
                let kids = n.children.clone();
                for (c, off) in kids.into_iter().zip(offsets) {
                    let Some(ch) = self.tree.get(c) else { continue };
                    let f = ch.flags;
                    if f & HIDDEN != 0
                        || ch.size.x == 0.0
                        || ch.size.y == 0.0
                        || f & MOUSE_OVER_HIDDEN != 0
                    {
                        continue;
                    }
                    if f & 0x240 != 0 {
                        self.render(em, c, self_pos, a, clip);
                        continue;
                    }
                    let child_origin = off + base;
                    let (v, o) = (ch.vis, child_origin);
                    if let Some(b) = clip {
                        let (l, t, r, bt) = (o.x + v[0], o.y + v[1], o.x + v[2], o.y + v[3]);
                        if !(b[0] <= r && l <= b[2] && b[1] <= bt && t <= b[3]) {
                            continue;
                        }
                    }
                    self.render(em, c, child_origin, a, clip);
                }
            }
            Kind::ScrollPanel(s) => {
                let (up, down, bar, scroll) = (s.up, s.down, s.bar, s.scroll);
                let f = n.flags;
                let size = n.size;
                let fixed = pos;
                let scrolled = pos + scroll;
                let bx = if f & sp_flag::FULL_CLIP != 0 {
                    [fixed.x, fixed.y, fixed.x + size.x, fixed.y + size.y]
                } else if f & sp_flag::VERTICAL != 0 {
                    [0.0, fixed.y, screen_w as f32, fixed.y + size.y]
                } else {
                    [fixed.x, 0.0, fixed.x + size.x, screen_h as f32]
                };
                let bx = match clip {
                    Some(c) => [
                        bx[0].max(c[0]),
                        bx[1].max(c[1]),
                        bx[2].min(c[2]),
                        bx[3].min(c[3]),
                    ],
                    None => bx,
                };
                for c in n.children.clone() {
                    let Some(ch) = self.tree.get(c) else { continue };
                    if ch.flags & NOT_ACTIVE != 0 {
                        continue;
                    }
                    if ch.flags & 0x240 != 0 {
                        self.render(em, c, fixed, a, clip);
                    } else {
                        self.render(em, c, scrolled, a, Some(bx));
                    }
                }
                if f & sp_flag::BAR_HIDDEN == 0 {
                    for p in [up, down, bar].into_iter().flatten() {
                        self.render(em, p, fixed, a, clip);
                    }
                }
            }
            Kind::Slider(s) => {
                let (track, shuttle) = (s.track, s.shuttle);
                self.render(em, track, pos, a, clip);
                self.render(em, shuttle, pos, a, clip);
                self.render_children(em, id, origin, alpha, clip);
            }
            Kind::ColorBox(b) => {
                em.fill(clip, pos, n.size, b.color, a);
                self.render_children(em, id, origin, alpha, clip);
            }
            Kind::ColorBoxButton(b) => {
                em.fill(clip, pos, n.size, b.1.color, a);
                self.render_children(em, id, origin, alpha, clip);
            }
            Kind::Bar(b) => {
                let size = n.size;
                if n.flags & 0x40_0000 != 0 {
                    em.fill(clip, pos, size, b.bg, a);
                }
                let p = b.percent.clamp(0.0, 1.0);
                let (fp, fs) = match b.direction {
                    2 => (
                        pos + Vec2::new(size.x * (1.0 - p), 0.0),
                        Vec2::new(size.x * p, size.y),
                    ),
                    0 => (pos, Vec2::new(size.x, size.y * p)),
                    1 => (
                        pos + Vec2::new(0.0, size.y * (1.0 - p)),
                        Vec2::new(size.x, size.y * p),
                    ),
                    _ => (pos, Vec2::new(size.x * p, size.y)),
                };
                em.fill(clip, fp, fs, b.fg, a);
                self.render_children(em, id, origin, alpha, clip);
            }
            Kind::Meter(m) => {
                let size = n.size;
                let p = m.percent.clamp(0.0, 1.0);
                let mut q = m.quad.clone();
                q.width = size.x * p;
                q.height = size.y;
                q.src_w = size.x * p;
                q.src_h = size.y;
                q.color = m.color;
                q.normalize();
                if m.has_shadow {
                    let sp = m.shadow_percent.clamp(0.0, 1.0);
                    let mut sq = q.clone();
                    sq.width = size.x * sp;
                    sq.color = [
                        m.shadow_color[0],
                        m.shadow_color[1],
                        m.shadow_color[2],
                        m.shadow_alpha.max(m.shadow_color[3]),
                    ];
                    em.quad(clip, &sq, pos, a);
                }
                em.quad(clip, &q, pos, a);
                self.render_children(em, id, origin, alpha, clip);
            }
            Kind::TextureBar(b) => {
                let size = n.size;
                let p = b.percent.clamp(0.0, 1.0);
                let mut q = b.quad.clone();
                q.src_x = b.tex_offset.x;
                q.src_y = b.tex_offset.y;
                let (w, h) = (size.x * p, size.y * p);
                // The part of the picture that is shown: from the left (Right growth) or the top (Down) it starts at the
                // top left corner; a bar that grows Up or Left shows the bottom / right part of the picture, at the
                // bottom / right of the control (a citizen meter fills from the bottom of its portrait circle).
                let mut at = pos;
                match b.direction {
                    0 | 1 => {
                        q.width = size.x;
                        q.height = h;
                        q.src_w = size.x;
                        q.src_h = h;
                        if b.direction == 1 {
                            at.y += size.y - h;
                            q.src_y += size.y - h;
                        }
                    }
                    _ => {
                        q.width = w;
                        q.height = size.y;
                        q.src_w = w;
                        q.src_h = size.y;
                        if b.direction == 2 {
                            at.x += size.x - w;
                            q.src_x += size.x - w;
                        }
                    }
                }
                q.normalize();
                em.quad(clip, &q, at, a);
                self.render_children(em, id, origin, alpha, clip);
            }
            Kind::Line(l) => {
                let (s, e) = (l.start + pos, l.end + pos);
                let d = e - s;
                let len = d.length();
                if len > 0.0 {
                    let nrm = Vec2::new(-d.y, d.x) / len * (l.width * 0.5);
                    let col = rgba(l.color, a);
                    let (sc, off) = (em.scale, em.offset);
                    let v = |p: Vec2| UiVertex {
                        pos: (p * sc + off).to_array(),
                        uv: [0.0, 0.0],
                        color: col,
                    };
                    let verts = [v(s - nrm), v(s + nrm), v(e + nrm), v(e - nrm)];
                    em.clip(clip);
                    em.out
                        .add_triangles(TextureId::WHITE, &verts, &[0, 1, 2, 0, 2, 3]);
                    em.unclip(clip);
                }
            }
            Kind::PullDown(p) => {
                let main = p.main;
                let open = p.open;
                if let Kind::PullDown(p) = &mut self.tree[id].kind {
                    p.origin = origin;
                }
                if let (Some(m), false) = (main, open) {
                    self.render(em, m, pos, a, clip);
                }
                self.render_children(em, id, origin, alpha, clip);
            }
        }
    }

    /// The open pull-down: its popup grid on top of the main button.
    fn render_pull_down_open(
        &mut self,
        em: &mut Em<'_>,
        id: ControlId,
        origin: Vec2,
        alpha: f32,
        clip: Option<ClipBox>,
    ) {
        let n = &self.tree[id];
        let Kind::PullDown(p) = &n.kind else { return };
        let (main, grid) = (p.main, p.grid);
        let pos = n.pos + origin;
        let a = n.alpha * alpha;
        if let Some(m) = main {
            self.render(em, m, pos, a, clip);
        }
        self.render(em, grid, pos, a, clip);
    }

    fn render_label(
        &mut self,
        em: &mut Em<'_>,
        id: ControlId,
        pos: Vec2,
        alpha: f32,
        clip: Option<ClipBox>,
    ) {
        self.label_layout(id);
        let Kind::Label(l) = &self.tree[id].kind else {
            return;
        };
        let Some(layout) = l.layout.clone() else {
            return;
        };
        for q in &layout.quads {
            let mut c = q.color;
            c[3] = (f32::from(c[3]) * alpha).round().clamp(0.0, 255.0) as u8;
            em.rect(
                clip,
                q.texture,
                pos + q.min,
                pos + q.max,
                q.uv_min,
                q.uv_max,
                c,
                &UiMaterial::PLAIN,
            );
        }
    }
}
