//! `UIManager::BuildControl` and the `BuildFromXML` of every class: XML element to controls.
//!
//! The exe applies a style by calling the control's own `BuildFromXML` on the style element
//! (`ControlBase::BuildFromStyle`), then reads the attributes of the element itself, so the
//! element's values win. The same order is kept here.

use std::collections::HashMap;
use std::rc::Rc;

use draw_list::UiSampler;
use glam::Vec2;

use crate::flags;
use crate::state::{State, crc32};
use crate::style::ColorSet;
use crate::tree::*;
use crate::values;
use crate::xml::XmlElem;

/// Tags the parent control reads itself; `BuildControl` skips them without a message.
const SKIPPED_TAGS: [&str; 13] = [
    "ColorSet",
    "Instance",
    "Shuttle",
    "UpButton",
    "DownButton",
    "ScrollBar",
    "ButtonData",
    "StackData",
    "GridData",
    "InstanceData",
    "ScrollPanelData",
    "ShowOnMouseOver",
    "HideOnMouseOver",
];

impl State {
    /// `UIManager::BuildControl`: the control for an element, built and not yet initialised.
    /// `owner` is the context whose XML the element is in.
    pub fn build_control(
        &mut self,
        el: &Rc<XmlElem>,
        owner: Option<ControlId>,
    ) -> Option<ControlId> {
        let kind = match el.name.as_str() {
            "ScrollAnim" => Kind::Anim(Box::new(Anim::new(AnimVariant::Scroll))),
            "FlipAnim" => Kind::Anim(Box::new(Anim::new(AnimVariant::Flip))),
            "SlideAnim" => Kind::Anim(Box::new(Anim::new(AnimVariant::Slide))),
            "AlphaAnim" => Kind::Anim(Box::new(Anim::new(AnimVariant::Alpha))),
            "Button" => self.new_button_kind(),
            "TextButton" => self.make_text_button(owner),
            "Image" => Kind::Image(Box::new(Image {
                quad: Quad::default(),
                stretch: 0,
                tex_w: 0.0,
                tex_h: 0.0,
            })),
            "Label" => Kind::Label(Box::default()),
            "Stack" => Kind::Stack(Stack::default()),
            "LuaContext" => Kind::Context(Box::new(Context::new(String::new()))),
            "CheckBox" => self.make_check_box(owner, false),
            "RadioButton" => self.make_check_box(owner, true),
            "GridButton" => self.make_grid_button(owner),
            "Line" => Kind::Line(Box::new(Line {
                start: Vec2::ZERO,
                end: Vec2::ZERO,
                width: 1.0,
                color: [1.0; 4],
            })),
            "ReplayMap" => Kind::ReplayMap(Box::new(ReplayMap {
                map_size: Vec2::ZERO,
                plots: Vec::new(),
            })),
            "Grid" => Kind::Grid(Box::default()),
            "ScrollPanel" => Kind::ScrollPanel(Box::default()),
            "Meter" => Kind::Meter(Box::new(Meter {
                quad: Quad::default(),
                color: [1.0; 4],
                shadow_alpha: 0.0,
                has_shadow: false,
                percent: 1.0,
                shadow_percent: 0.0,
                shadow_color: [0.0, 0.0, 0.0, 0.5],
            })),
            "Bar" => Kind::Bar(Box::new(Bar {
                fg: [0.0, 1.0, 0.0, 1.0],
                bg: [0.0, 0.0, 0.0, 1.0],
                direction: 3,
                percent: 1.0,
            })),
            "TextureBar" => Kind::TextureBar(Box::new(TextureBar {
                quad: Quad::default(),
                direction: 3,
                has_shadow: false,
                percent: 1.0,
                shadow_percent: 0.0,
                shadow_color: [0.0, 0.0, 0.0, 0.5],
                tex_offset: Vec2::ZERO,
            })),
            "Box" => Kind::ColorBox(Box::new(ColorBox { color: [1.0; 4] })),
            "BoxButton" => Kind::ColorBoxButton(Box::new((
                ButtonBase::default(),
                ColorBox { color: [1.0; 4] },
            ))),
            "WorldAnchor" => Kind::WorldAnchor(Box::new(WorldAnchor {
                world: [0.0; 3],
                depth_adjust: 0.0,
                scaling: false,
                scale: 1.0,
                bounds: [32766.0, -32766.0, 32766.0, -32766.0],
            })),
            "Container" => Kind::Container,
            "Slider" => self.make_slider(owner),
            "EditBox" => self.make_edit_box(owner),
            "PullDown" => self.make_pull_down(owner),
            "ToolTipType" => {
                self.register_tooltip_type(el, owner);
                return None;
            }
            name if SKIPPED_TAGS.contains(&name) => return None,
            _ => {
                self.warn(el, &format!("Unknown control type {}", el.name), &el.name);
                return None;
            }
        };
        let id = self.new_node(kind, owner);
        self.link_compounds(id);
        self.stats.controls_built += 1;
        let mark = self.touch(el);
        self.build_from_xml(id, el);
        self.finish_touched(mark);
        Some(id)
    }

    fn new_button_kind(&self) -> Kind {
        Kind::Button(Box::new(Button {
            base: ButtonBase::default(),
            quad: Quad::default(),
            tex_offset: Vec2::ZERO,
            state_inc: Vec2::ZERO,
            text: None,
        }))
    }

    /// The virtual `BuildFromXML` of the control's class.
    pub fn build_from_xml(&mut self, id: ControlId, el: &Rc<XmlElem>) {
        enum K {
            Container,
            Button,
            Image,
            Label,
            Anim(AnimVariant),
            Context,
            Stack,
            Grid,
            Meter,
            Bar,
            TextureBar,
            ColorBox,
            ColorBoxButton,
            WorldAnchor,
            Line,
            ReplayMap,
            TextButton,
            CheckBox,
            GridButton,
            ScrollPanel,
            Slider,
            EditBox,
            PullDown,
        }
        let k = match &self.tree[id].kind {
            Kind::Container => K::Container,
            Kind::Button(_) => K::Button,
            Kind::Image(_) => K::Image,
            Kind::Label(_) => K::Label,
            Kind::Anim(a) => K::Anim(a.variant),
            Kind::Context(_) => K::Context,
            Kind::Stack(_) => K::Stack,
            Kind::Grid(_) => K::Grid,
            Kind::Meter(_) => K::Meter,
            Kind::Bar(_) => K::Bar,
            Kind::TextureBar(_) => K::TextureBar,
            Kind::ColorBox(_) => K::ColorBox,
            Kind::ColorBoxButton(_) => K::ColorBoxButton,
            Kind::WorldAnchor(_) => K::WorldAnchor,
            Kind::Line(_) => K::Line,
            Kind::ReplayMap(_) => K::ReplayMap,
            Kind::TextButton(_) => K::TextButton,
            Kind::CheckBox(_) | Kind::RadioButton(_) => K::CheckBox,
            Kind::GridButton(_) => K::GridButton,
            Kind::ScrollPanel(_) => K::ScrollPanel,
            Kind::Slider(_) => K::Slider,
            Kind::EditBox(_) => K::EditBox,
            Kind::PullDown(_) => K::PullDown,
        };
        match k {
            K::Container => self.base_build(id, el),
            K::Button => self.build_button(id, el),
            K::Image => self.build_image(id, el),
            K::Label => {
                self.build_from_style(id, el);
                self.build_label_core(id, el);
                self.base_build(id, el);
            }
            K::Anim(v) => self.build_anim(id, el, v),
            K::Context => self.build_context_xml(id, el),
            K::Stack => self.build_stack(id, el),
            K::Grid => {
                self.build_from_style(id, el);
                self.build_grid_core(id, el);
                self.base_build(id, el);
            }
            K::Meter => {
                self.build_from_style(id, el);
                let tex = self.attr(el, "Texture").map(str::to_owned);
                let color = self.attr(el, "Color").map(str::to_owned);
                let has_shadow = self.attr_bool(el, "HasShadow");
                let shadow_alpha = self.attr_f32(el, "ShadowAlpha");
                let t = tex.and_then(|n| self.texture(&n));
                let c = color.and_then(|c| self.styles.extract_color(&c));
                if let Kind::Meter(m) = &mut self.tree[id].kind {
                    if let Some(t) = t {
                        m.quad.set_texture(Some(t), None);
                    }
                    if let Some(c) = c {
                        m.color = c;
                    }
                    if let Some(h) = has_shadow {
                        m.has_shadow = h;
                    }
                    if let Some(a) = shadow_alpha {
                        m.shadow_alpha = a;
                    }
                }
                self.base_build(id, el);
            }
            K::Bar => {
                self.build_from_style(id, el);
                let fg = self.attr(el, "FGColor").map(str::to_owned);
                let bg = self.attr(el, "BGColor").map(str::to_owned);
                let dir = self.attr(el, "Direction").map(str::to_owned);
                let fgc = fg.and_then(|c| self.styles.extract_color(&c));
                let bgc = bg.and_then(|c| self.styles.extract_color(&c));
                let has_bg = bgc.is_some();
                if let Kind::Bar(b) = &mut self.tree[id].kind {
                    if let Some(c) = fgc {
                        b.fg = c;
                    }
                    if let Some(c) = bgc {
                        b.bg = c;
                    }
                    if let Some(d) = dir.as_deref().and_then(values::growth_dir) {
                        b.direction = d;
                    }
                }
                if has_bg {
                    // The background box is drawn when a BGColor was given.
                    self.tree[id].flags |= 0x40_0000;
                }
                self.base_build(id, el);
            }
            K::TextureBar => {
                self.build_from_style(id, el);
                let tex = self.attr(el, "Texture").map(str::to_owned);
                let off = self.attr(el, "TextureOffset").map(str::to_owned);
                let dir = self.attr(el, "Direction").map(str::to_owned);
                let shadow = self.attr(el, "ShadowColor").map(str::to_owned);
                let has_shadow = self.attr_bool(el, "HasShadow");
                let t = tex.and_then(|n| self.texture(&n));
                let scr = self.screen();
                let sc = shadow.and_then(|c| self.styles.extract_color(&c));
                if let Kind::TextureBar(b) = &mut self.tree[id].kind {
                    if let Some(t) = t {
                        b.quad.set_texture(Some(t), None);
                    }
                    if let Some(o) = off {
                        let c = values::extract_coords(&o, scr, (0.0, 0.0));
                        b.tex_offset = Vec2::new(c.x, c.y);
                    }
                    if let Some(d) = dir.as_deref().and_then(values::growth_dir) {
                        b.direction = d;
                    }
                    if let Some(h) = has_shadow {
                        b.has_shadow = h;
                    }
                    if let Some(c) = sc {
                        b.shadow_color = c;
                    }
                }
                self.base_build(id, el);
            }
            K::ColorBox => {
                self.build_from_style(id, el);
                self.base_build(id, el);
            }
            K::ColorBoxButton => {
                self.build_from_style(id, el);
                self.build_button_base(id, el);
                self.base_build(id, el);
            }
            K::WorldAnchor => {
                self.build_from_style(id, el);
                let scaling = self.attr_bool(el, "Scaling");
                let depth = self.attr_f32(el, "DepthAdjust");
                if let Kind::WorldAnchor(w) = &mut self.tree[id].kind {
                    if let Some(s) = scaling {
                        w.scaling = s;
                    }
                    if let Some(d) = depth {
                        w.depth_adjust = d;
                    }
                }
                self.base_build(id, el);
            }
            K::Line => {
                self.build_from_style(id, el);
                let scr = self.screen();
                let s = self.attr(el, "Start").map(str::to_owned);
                let e = self.attr(el, "End").map(str::to_owned);
                let w = self.attr_f32(el, "Width");
                let c = self.attr(el, "Color").map(str::to_owned);
                let col = c.and_then(|c| self.styles.extract_color(&c));
                if let Kind::Line(l) = &mut self.tree[id].kind {
                    if let Some(s) = s {
                        let c = values::extract_coords(&s, scr, (0.0, 0.0));
                        l.start = Vec2::new(c.x, c.y);
                    }
                    if let Some(e) = e {
                        let c = values::extract_coords(&e, scr, (0.0, 0.0));
                        l.end = Vec2::new(c.x, c.y);
                    }
                    if let Some(w) = w {
                        l.width = w;
                    }
                    if let Some(c) = col {
                        l.color = c;
                    }
                }
                self.base_build(id, el);
            }
            K::ReplayMap => {
                self.build_from_style(id, el);
                self.base_build(id, el);
            }
            K::TextButton => self.apply_text_button(id, el),
            K::CheckBox => self.apply_check_box(id, el),
            K::GridButton => self.apply_grid_button(id, el),
            K::ScrollPanel => self.apply_scroll_panel(id, el),
            K::Slider => self.apply_slider(id, el),
            K::EditBox => self.apply_edit_box(id, el),
            K::PullDown => self.apply_pull_down(id, el),
        }
    }

    /// Sets the parent of the parts a control owns outside its child list.
    pub fn link_compounds(&mut self, id: ControlId) {
        let parts: Vec<ControlId> = match &self.tree[id].kind {
            Kind::TextButton(t) => vec![t.label],
            Kind::GridButton(g) => vec![g.grid],
            Kind::CheckBox(c) | Kind::RadioButton(c) => {
                // The label hangs below the box.
                vec![c.boxed]
            }
            Kind::Slider(s) => vec![s.track, s.shuttle],
            Kind::EditBox(e) => vec![e.label],
            Kind::PullDown(p) => vec![p.grid, p.scroll, p.stack],
            _ => Vec::new(),
        };
        for p in parts {
            self.tree[p].parent = Some(id);
        }
    }

    // ---- ControlBase ----

    /// `ControlBase::BuildFromStyle`: the style named by `Style`, found among the local
    /// styles of the context and then in the style sheets, is built into this control first.
    pub fn build_from_style(&mut self, id: ControlId, el: &Rc<XmlElem>) {
        let Some(name) = self.attr(el, "Style").map(str::to_owned) else {
            return;
        };
        let owner = self.tree[id].owner;
        let found = owner
            .and_then(|o| self.ctx(o))
            .and_then(|c| c.styles.clone())
            .and_then(|s| s.child(&name).cloned())
            .or_else(|| self.styles.style(&name));
        match found {
            None => self.warn(el, &format!("Unidentified style: {name}"), &name),
            Some(style) => {
                // A style that itself names a style is not followed (the exe tests this).
                if style
                    .attr("Style", self.cfg.attr_case_insensitive)
                    .is_none()
                {
                    self.touch_more(&style);
                    self.build_from_xml(id, &style);
                }
            }
        }
    }

    /// `ControlBase::BuildFromXML`.
    pub fn base_build(&mut self, id: ControlId, el: &Rc<XmlElem>) {
        let scr = self.screen();
        if let Some(v) = self.attr(el, "Offset") {
            let n = &mut self.tree[id];
            let c = values::extract_coords(v, scr, (n.offset.x, n.offset.y));
            n.offset = Vec2::new(c.x, c.y);
        }
        if let Some(v) = self.attr(el, "Size") {
            let n = &mut self.tree[id];
            let c = values::extract_coords(v, scr, (n.size.x, n.size.y));
            n.size = Vec2::new(c.x, c.y);
            n.flags |= c.flags;
        }
        if let Some(v) = self.attr(el, "Anchor") {
            self.tree[id].flags |= values::anchor_flags(v);
        }
        if let Some(v) = self.attr(el, "AnchorSide") {
            self.tree[id].flags |= values::anchor_side_flags(v);
        }
        if let Some(v) = self.attr(el, "Color").map(str::to_owned) {
            if self.styles.is_unknown_name(&v) {
                self.warn(el, &format!("unknown colour {v}"), &v);
            }
            if let Some(c) = self.styles.extract_color(&v) {
                self.set_color(id, c);
            }
        }
        if let Some(v) = self.attr(el, "ID") {
            self.tree[id].id = v.to_owned();
        }
        let bools = [
            ("Hidden", flags::HIDDEN),
            ("NeedsMouseOver", flags::NEEDS_MOUSE_OVER),
            ("NoClip", flags::NO_CLIP),
            ("ConsumeMouse", flags::CONSUME_MOUSE),
            ("ConsumeMouseButton", flags::CONSUME_BUTTON),
            ("ConsumeMouseOver", flags::CONSUME_OVER),
            ("ConsumeMouseWheel", flags::CONSUME_WHEEL),
            ("Disabled", flags::DISABLED),
        ];
        for (name, bit) in bools {
            if self.attr_bool(el, name) == Some(true) {
                self.tree[id].flags |= bit;
            }
        }
        if let Some(v) = self.attr_f32(el, "BranchAlpha") {
            self.tree[id].alpha = v;
        }
        if let Some(v) = self.attr(el, "ToolTip").map(str::to_owned) {
            let text = self.localize(&v, &[]);
            self.set_tooltip_string(id, Some(text));
        }
        if let Some(v) = self.attr(el, "ToolTipType") {
            let n = &mut self.tree[id];
            n.tt_crc = crc32(v);
            n.flags |= flags::HAS_TOOLTIP;
        }
        let owner = self.tree[id].owner;
        for (tag, outer) in [("ShowOnMouseOver", false), ("HideOnMouseOver", true)] {
            if let Some(group) = el.child(tag).cloned() {
                self.touch_more(&group);
                for c in &group.children {
                    if let Some(cid) = self.build_control(c, owner) {
                        self.add_child(id, cid);
                        self.add_mouse_over_child(id, cid, outer);
                    }
                }
                self.tree[id].flags |= flags::NEEDS_MOUSE_OVER;
            }
        }
        for c in &el.children {
            if let Some(cid) = self.build_control(c, owner) {
                self.add_child(id, cid);
            }
        }
        // The control owns its children now (ParentDelete).
        self.tree[id].flags |= flags::PARENT_DELETE;
    }

    /// `ControlBase::AddMouseOverChild`.
    pub fn add_mouse_over_child(&mut self, id: ControlId, child: ControlId, alt: bool) {
        if alt {
            self.tree[id].hide_on_over.push(child);
        } else {
            self.tree[id].show_on_over.push(child);
            self.tree[child].flags |= flags::MOUSE_OVER_HIDDEN;
        }
        self.tree[id].flags |= flags::NEEDS_MOUSE_OVER;
    }

    /// `ControlBase::SetToolTipString`.
    pub fn set_tooltip_string(&mut self, id: ControlId, text: Option<String>) {
        let n = &mut self.tree[id];
        match text {
            None => {
                n.tt_string = None;
                n.flags &= !flags::HAS_TOOLTIP;
            }
            Some(t) => {
                n.tt_string = Some(t);
                let was = n.flags & flags::HAS_TOOLTIP != 0;
                n.flags |= flags::HAS_TOOLTIP;
                if !was && self.tooltips.current != Some(id) {
                    // A tip switched on under a resting mouse (SIMP-uic-7, `Shared::update`).
                    self.tooltips.recheck = true;
                }
                self.refresh_tooltip(id);
            }
        }
    }

    /// `ControlBase::AddChild` and the overrides of `StackControl` and `LuaContext`.
    pub fn add_child(&mut self, parent: ControlId, child: ControlId) {
        self.add_child_at(parent, child, None);
    }

    pub fn add_child_at(&mut self, parent: ControlId, child: ControlId, index: Option<usize>) {
        // StackControl::AddChild: a vertical stack frees the child's vertical anchor, a
        // horizontal one its horizontal anchor (the outer sides stay).
        if let Kind::Stack(s) = &self.tree[parent].kind {
            let dir = s.dir;
            let c = &mut self.tree[child];
            if dir <= 1 {
                if c.flags & flags::V_OUTER == 0 {
                    c.flags &= 0xffff_fe7f;
                }
            } else if c.flags & flags::H_OUTER == 0 {
                c.flags &= 0xffff_ffcf;
            }
        }
        let owner = if matches!(self.tree[parent].kind, Kind::Context(_)) {
            Some(parent)
        } else {
            self.tree[parent].owner
        };
        let c = &mut self.tree[child];
        c.parent = Some(parent);
        c.owner = owner;
        let kids = &mut self.tree[parent].children;
        match index {
            Some(i) if i < kids.len() => kids.insert(i, child),
            _ => kids.push(child),
        }
    }

    /// `ControlBase::SetColor` (virtual): only some classes use a colour.
    pub fn set_color(&mut self, id: ControlId, c: [f32; 4]) {
        match &mut self.tree[id].kind {
            Kind::Image(i) => i.quad.color = c,
            Kind::Button(b) => b.quad.color = c,
            Kind::Grid(g) => g.color = c,
            Kind::ColorBox(b) => b.color = c,
            Kind::ColorBoxButton(b) => b.1.color = c,
            Kind::Label(l) => {
                l.colors.colors[0] = c;
                l.color_explicit = true;
                l.layout = None;
                l.request = None;
            }
            Kind::GridButton(g) => {
                let grid = g.grid;
                self.set_color(grid, c);
            }
            Kind::Anim(a) => {
                if let Some(q) = &mut a.quad {
                    q.color = c;
                }
            }
            Kind::Meter(m) => m.color = c,
            Kind::Line(l) => l.color = c,
            _ => {}
        }
    }

    // ---- context-local lookups ----

    /// `LuaContext::ExtractColorSet`: the local sets, then the global ones, else the context default.
    pub fn extract_color_set(&mut self, owner: Option<ControlId>, name: &str) -> ColorSet {
        if let Some(c) = owner.and_then(|o| self.ctx(o))
            && let Some(s) = c.color_sets.get(name)
        {
            return *s;
        }
        if let Some(s) = self.styles.color_set(name) {
            return s;
        }
        owner
            .and_then(|o| self.ctx(o))
            .map(|c| c.default_set)
            .unwrap_or_default()
    }

    // ---- kinds ----

    fn new_label_node(&mut self, owner: Option<ControlId>) -> ControlId {
        self.new_node(Kind::Label(Box::default()), owner)
    }

    /// `TextControl::BuildCoreFromXML`.
    pub fn build_label_core(&mut self, id: ControlId, el: &Rc<XmlElem>) {
        let owner = self.tree[id].owner;
        if let Some(v) = self.attr(el, "Font").map(str::to_owned) {
            self.label_mut(id, |l| l.font = Some(v));
        }
        if let Some(v) = self.attr(el, "ColorSet").map(str::to_owned) {
            let set = self.extract_color_set(owner, &v);
            self.label_mut(id, |l| {
                l.colors = set;
                l.color_explicit = true;
            });
        }
        for (name, slot) in [("Color", 0), ("Color0", 0), ("Color1", 1), ("Color2", 2)] {
            if let Some(v) = self.attr(el, name).map(str::to_owned) {
                if self.styles.is_unknown_name(&v) {
                    self.warn(el, &format!("unknown colour {v}"), &v);
                }
                if let Some(c) = self.styles.extract_color(&v) {
                    self.label_mut(id, |l| {
                        l.colors.colors[slot] = c;
                        l.color_explicit = true;
                    });
                }
            }
        }
        let _ = self.attr(el, "Color3");
        if let Some(v) = self.attr(el, "FontStyle").map(str::to_owned) {
            self.label_mut(id, |l| l.style = Some(v));
        }
        // WrapWidth and TruncateWidth exclude each other.
        if let Some(v) = self.attr_i32(el, "WrapWidth") {
            self.label_mut(id, |l| {
                l.wrap_width = v;
                l.truncate_width = -1;
            });
        }
        if let Some(v) = self.attr_i32(el, "TruncateWidth") {
            self.label_mut(id, |l| {
                l.truncate_width = v;
                l.wrap_width = -1;
            });
        }
        let _ = self.attr(el, "ForceIME");
        let _ = self.attr(el, "ForceNonIME");
        if let Some(v) = self.attr_bool(el, "ReduceOnTruncate") {
            self.label_mut(id, |l| l.reduce_on_truncate = v);
        }
        if let Some(v) = self.attr_i32(el, "LeadingOffset") {
            self.label_mut(id, |l| l.leading_offset = v);
        }
        if let Some(v) = self.attr(el, "String").map(str::to_owned) {
            let text = if v.is_empty() {
                v
            } else {
                self.localize(&v, &[])
            };
            self.set_label_text(id, &text);
        }
    }

    pub fn label_mut(&mut self, id: ControlId, f: impl FnOnce(&mut Label)) {
        if let Kind::Label(l) = &mut self.tree[id].kind {
            f(l);
            l.layout = None;
            l.request = None;
        }
    }

    /// `TextControl::SetText`.
    pub fn set_label_text(&mut self, id: ControlId, text: &str) {
        self.label_mut(id, |l| l.text = text.to_owned());
    }

    /// `ButtonControlBase::BuildFromXML` (Void1, Void2, NoDefaultSound, Disabled), then the base.
    pub fn build_button_base(&mut self, id: ControlId, el: &Rc<XmlElem>) {
        let v1 = self.attr_i32(el, "Void1");
        let v2 = self.attr_i32(el, "Void2");
        let nds = self.attr_bool(el, "NoDefaultSound");
        let dis = self.attr_bool(el, "Disabled");
        let n = &mut self.tree[id];
        if let Some(b) = n.kind.button_base_mut() {
            if let Some(v) = v1 {
                b.void1 = v;
            }
            if let Some(v) = v2 {
                b.void2 = v;
            }
        }
        if nds == Some(true) {
            n.flags |= btn::NO_DEFAULT_SOUND;
        }
        if dis == Some(true) {
            n.flags |= flags::DISABLED;
        }
    }

    fn build_button(&mut self, id: ControlId, el: &Rc<XmlElem>) {
        self.build_from_style(id, el);
        let owner = self.tree[id].owner;
        let scr = self.screen();
        if self.attr(el, "String").is_some() {
            self.tree[id].flags |= button_flag::HAS_TEXT;
            let existing = match &self.tree[id].kind {
                Kind::Button(b) => b.text,
                _ => None,
            };
            let t = match existing {
                Some(t) => t,
                None => {
                    let t = self.new_label_node(owner);
                    self.tree[t].parent = Some(id);
                    if let Kind::Button(b) = &mut self.tree[id].kind {
                        b.text = Some(t);
                    }
                    t
                }
            };
            self.build_label_core(t, el);
            if let Some(v) = self.attr(el, "TextAnchor") {
                self.tree[id].flags |= button_flag::TEXT_ANCHOR;
                self.tree[t].flags |= values::anchor_flags(v);
            }
            if let Some(v) = self.attr(el, "TextOffset") {
                let c = values::extract_coords(v, scr, (0.0, 0.0));
                self.tree[t].offset = Vec2::new(c.x, c.y);
            }
        }
        if let Some(v) = self.attr(el, "Texture").map(str::to_owned) {
            let t = self.texture(&v);
            if let Kind::Button(b) = &mut self.tree[id].kind {
                b.quad.set_texture(t, None);
            }
        }
        if let Some(v) = self.attr(el, "TextureOffset") {
            let c = values::extract_coords(v, scr, (0.0, 0.0));
            if let Kind::Button(b) = &mut self.tree[id].kind {
                b.tex_offset = Vec2::new(c.x, c.y);
            }
        }
        if let Some(v) = self.attr(el, "Color").map(str::to_owned)
            && let Some(c) = self.styles.extract_color(&v)
            && let Kind::Button(b) = &mut self.tree[id].kind
        {
            b.quad.color = c;
        }
        if let Some(v) = self.attr(el, "StateOffsetIncrement") {
            let c = values::extract_coords(v, scr, (0.0, 0.0));
            if let Kind::Button(b) = &mut self.tree[id].kind {
                b.state_inc = Vec2::new(c.x, c.y);
            }
            self.tree[id].flags |= button_flag::STATE_INC;
        }
        if self.attr_bool(el, "NoStateChange") == Some(true) {
            self.tree[id].flags |= button_flag::NO_STATE_CHANGE;
        }
        self.build_button_base(id, el);
        self.base_build(id, el);
    }

    fn make_text_button(&mut self, owner: Option<ControlId>) -> Kind {
        let label = self.new_label_node(owner);
        Kind::TextButton(Box::new(TextButton {
            base: ButtonBase::default(),
            label,
            sets: [ColorSet::default(); 4],
            styles: Default::default(),
        }))
    }

    fn apply_text_button(&mut self, id: ControlId, el: &Rc<XmlElem>) {
        self.build_from_style(id, el);
        self.build_text_button_core(id, el);
        self.build_button_base(id, el);
        self.base_build(id, el);
    }

    /// `TextButtonControl::BuildCoreFromXML`.
    pub fn build_text_button_core(&mut self, id: ControlId, el: &Rc<XmlElem>) {
        let owner = self.tree[id].owner;
        let Kind::TextButton(tb) = &self.tree[id].kind else {
            return;
        };
        let label = tb.label;
        let default = owner
            .and_then(|o| self.ctx(o))
            .map(|c| c.default_set)
            .unwrap_or_default();
        let mut sets = [default; 4];
        let mut prev = default;
        for (i, name) in ["NormalState", "MouseOver", "ButtonDown", "Disabled"]
            .iter()
            .enumerate()
        {
            sets[i] = match self.attr(el, name).map(str::to_owned) {
                Some(n) => self.extract_color_set(owner, &n),
                // A missing state takes the normal one (the first copy stays the default).
                None => {
                    if i == 0 {
                        default
                    } else {
                        prev
                    }
                }
            };
            if i == 0 {
                prev = sets[0];
            }
        }
        // A missing state copies the NORMAL state set.
        for i in 1..4 {
            if el
                .attr(
                    ["NormalState", "MouseOver", "ButtonDown", "Disabled"][i],
                    self.cfg.attr_case_insensitive,
                )
                .is_none()
            {
                sets[i] = sets[0];
            }
        }
        self.label_mut(label, |l| {
            l.colors = sets[0];
            l.color_explicit = true;
        });
        self.build_label_core(label, el);
        let base_style = match &self.tree[label].kind {
            Kind::Label(l) => l.style.clone().unwrap_or_default(),
            _ => String::new(),
        };
        let mut styles: [String; 4] = [
            base_style.clone(),
            base_style.clone(),
            base_style.clone(),
            base_style,
        ];
        for (i, name) in [
            (1, "MouseOverStyle"),
            (2, "ButtonDownStyle"),
            (3, "DisabledStyle"),
        ] {
            if let Some(v) = self.attr(el, name) {
                styles[i] = v.to_owned();
            }
        }
        // The normal-state colours the label was given by name win over the sets.
        if let Kind::TextButton(tb) = &mut self.tree[id].kind {
            tb.sets = sets;
            tb.styles = styles;
        }
        let _ = id;
    }

    fn make_check_box(&mut self, owner: Option<ControlId>, radio: bool) -> Kind {
        let boxed = self.new_node(
            Kind::Button(Box::new(Button {
                base: ButtonBase::default(),
                quad: Quad::default(),
                tex_offset: Vec2::ZERO,
                state_inc: Vec2::new(0.0, 32.0),
                text: None,
            })),
            owner,
        );
        self.tree[boxed].size = Vec2::splat(32.0);
        self.tree[boxed].flags |= button_flag::STATE_INC;
        let ll = self.new_label_node(owner);
        let label = self.new_node(
            Kind::TextButton(Box::new(TextButton {
                base: ButtonBase::default(),
                label: ll,
                sets: [ColorSet::default(); 4],
                styles: Default::default(),
            })),
            owner,
        );
        self.tree[ll].parent = Some(label);
        self.tree[label].parent = Some(boxed);
        // CheckBoxControl::CheckBoxControl: the exe's built-in textures, used when the XML (or its style)
        // names no ButtonTexture / CheckTexture ("Assets/UI/Art/Controls/CheckBox.dds" / "CheckMark.dds").
        let box_tex = self.texture("Assets/UI/Art/Controls/CheckBox.dds");
        if let Kind::Button(b) = &mut self.tree[boxed].kind {
            b.quad.set_texture(box_tex, None);
        }
        let mut mark = Quad::default();
        let mark_tex = self.texture("Assets/UI/Art/Controls/CheckMark.dds");
        mark.set_texture(mark_tex, None);
        let cb = CheckBox {
            base: ButtonBase::default(),
            boxed,
            label,
            mark,
            mark_size: Vec2::splat(32.0),
            mark_offset: Vec2::ZERO,
            checked: false,
            handler: None,
            radio_group: None,
        };
        if radio {
            Kind::RadioButton(Box::new(cb))
        } else {
            Kind::CheckBox(Box::new(cb))
        }
    }

    fn apply_check_box(&mut self, id: ControlId, el: &Rc<XmlElem>) {
        let (boxed, label, radio) = match &self.tree[id].kind {
            Kind::CheckBox(c) => (c.boxed, c.label, false),
            Kind::RadioButton(c) => (c.boxed, c.label, true),
            _ => return,
        };
        let scr = self.screen();
        self.build_from_style(id, el);
        self.build_text_button_core(label, el);
        // The label sits to the right of the box, centred vertically, unless told otherwise.
        let anchor = self.attr(el, "TextAnchor").map(values::anchor_flags);
        self.tree[label].flags |= anchor.unwrap_or(flags::V_CENTER);
        let side = self
            .attr(el, "TextAnchorSide")
            .map(values::anchor_side_flags);
        self.tree[label].flags |= side.unwrap_or(flags::H_OUTER);
        if let Some(v) = self.attr(el, "TextOffset") {
            let c = values::extract_coords(v, scr, (0.0, 0.0));
            self.tree[label].offset = Vec2::new(c.x, c.y);
        }
        if let Some(v) = self.attr(el, "ButtonTexture").map(str::to_owned) {
            let t = self.texture(&v);
            if let Kind::Button(b) = &mut self.tree[boxed].kind {
                b.quad.set_texture(t, None);
            }
        }
        if let Some(v) = self.attr(el, "CheckTexture").map(str::to_owned) {
            let t = self.texture(&v);
            if let Some(cb) = check_data_mut(&mut self.tree[id].kind) {
                cb.mark.set_texture(t, None);
            }
        }
        if let Some(v) = self.attr_bool(el, "IsChecked")
            && let Some(cb) = check_data_mut(&mut self.tree[id].kind)
        {
            cb.checked = v;
        }
        if let Some(v) = self.attr(el, "CheckTextureOffset") {
            let c = values::extract_coords(v, scr, (0.0, 0.0));
            if let Some(cb) = check_data_mut(&mut self.tree[id].kind) {
                cb.mark.src_x = c.x;
                cb.mark.src_y = c.y;
            }
        }
        if let Some(v) = self.attr(el, "CheckOffset") {
            let c = values::extract_coords(v, scr, (0.0, 0.0));
            if let Some(cb) = check_data_mut(&mut self.tree[id].kind) {
                cb.mark_offset = Vec2::new(c.x, c.y);
            }
        }
        if let Some(v) = self.attr(el, "CheckSize") {
            let c = values::extract_coords(v, scr, (32.0, 32.0));
            if let Some(cb) = check_data_mut(&mut self.tree[id].kind) {
                cb.mark_size = Vec2::new(c.x, c.y);
            }
        }
        if let Some(v) = self.attr(el, "ButtonSize") {
            let c = values::extract_coords(v, scr, (32.0, 32.0));
            self.tree[boxed].size = Vec2::new(c.x, c.y);
        }
        if radio
            && let Some(v) = self.attr(el, "RadioGroup").map(str::to_owned)
            && let Some(cb) = check_data_mut(&mut self.tree[id].kind)
        {
            cb.radio_group = Some(v);
        }
        self.build_button_base(id, el);
        self.base_build(id, el);
    }

    fn make_grid_button(&mut self, owner: Option<ControlId>) -> Kind {
        let grid = self.new_node(Kind::Grid(Box::default()), owner);
        Kind::GridButton(Box::new(GridButton {
            base: ButtonBase::default(),
            grid,
            text: None,
        }))
    }

    fn apply_grid_button(&mut self, id: ControlId, el: &Rc<XmlElem>) {
        let Kind::GridButton(gb) = &self.tree[id].kind else {
            return;
        };
        let grid = gb.grid;
        let owner = self.tree[id].owner;
        let scr = self.screen();
        self.build_from_style(id, el);
        if self.attr(el, "String").is_some() {
            self.tree[id].flags |= button_flag::HAS_TEXT;
            let t = self.new_label_node(owner);
            self.tree[t].parent = Some(id);
            if let Kind::GridButton(g) = &mut self.tree[id].kind {
                g.text = Some(t);
            }
            self.build_label_core(t, el);
            if let Some(v) = self.attr(el, "TextOffset") {
                let c = values::extract_coords(v, scr, (0.0, 0.0));
                self.tree[t].offset = Vec2::new(c.x, c.y);
            }
        }
        if let Some(gd) = el.child("GridData").cloned() {
            self.touch_more(&gd);
            self.build_from_xml(grid, &gd);
        }
        if let Some(v) = self.attr(el, "Color").map(str::to_owned)
            && let Some(c) = self.styles.extract_color(&v)
        {
            self.set_color(grid, c);
        }
        self.build_button_base(id, el);
        self.base_build(id, el);
    }

    fn build_image(&mut self, id: ControlId, el: &Rc<XmlElem>) {
        self.build_from_style(id, el);
        let scr = self.screen();
        if let Some(v) = self.attr(el, "Texture").map(str::to_owned) {
            let t = self.texture(&v);
            if let Kind::Image(i) = &mut self.tree[id].kind {
                i.quad.texture = t;
                i.quad.handle = None;
            }
        }
        if let Some(v) = self.attr(el, "TextureOffset") {
            let c = values::extract_coords(v, scr, (0.0, 0.0));
            if let Kind::Image(i) = &mut self.tree[id].kind {
                i.quad.src_x = c.x;
                i.quad.src_y = c.y;
            }
        }
        if let Some(v) = self.attr(el, "MaskTexture").map(str::to_owned) {
            let t = self.texture(&v);
            if let Kind::Image(i) = &mut self.tree[id].kind {
                i.quad.mask = t;
                // UIQuad blend slot 2 selects the mask shader.
                i.quad.blend_slot = 2;
            }
        }
        if let Some(v) = self.attr(el, "StretchMode") {
            let m = match v.to_ascii_lowercase().as_str() {
                "none" => 1,
                "uniform" => 2,
                "fill" => 3,
                "uniformtofill" => 4,
                _ => 0,
            };
            if m != 0
                && let Kind::Image(i) = &mut self.tree[id].kind
            {
                i.stretch = m;
            }
        }
        if let Some(v) = self.attr(el, "Sampler").map(str::to_owned) {
            // ImageControl::BuildFromXML: a value that starts with the letter of "Linear"
            // sets the class bit; Initialize turns it into sampler state 5 (default 10).
            if v.starts_with(['L', 'l']) {
                self.tree[id].flags |= image_flag::SAMPLER_SET;
            }
            // SIMPLIFIED(SIMP-uic-6): the meaning of states 5 and 10 is not read from the
            // renderer; "None"/"Point" values become point sampling, everything else linear.
            if v.starts_with(['N', 'n', 'P', 'p'])
                && let Kind::Image(i) = &mut self.tree[id].kind
            {
                i.quad.sampler = UiSampler::Point;
            }
        }
        if let Kind::Image(i) = &mut self.tree[id].kind {
            if let Some(t) = &i.quad.texture {
                i.tex_w = t.w;
                i.tex_h = t.h;
            }
            i.quad.normalize();
        }
        self.base_build(id, el);
    }

    fn build_anim(&mut self, id: ControlId, el: &Rc<XmlElem>, v: AnimVariant) {
        self.build_from_style(id, el);
        let scr = self.screen();
        match v {
            AnimVariant::Slide => {
                let start = self.attr(el, "Start").map(str::to_owned);
                let begin = self.attr(el, "Begin").map(str::to_owned);
                if let Some(s) = start.or(begin) {
                    let c = values::extract_coords(&s, scr, (0.0, 0.0));
                    if let Kind::Anim(a) = &mut self.tree[id].kind {
                        a.slide_begin = Vec2::new(c.x, c.y);
                    }
                }
                if let Some(s) = self.attr(el, "End").map(str::to_owned) {
                    let c = values::extract_coords(&s, scr, (0.0, 0.0));
                    if let Kind::Anim(a) = &mut self.tree[id].kind {
                        a.slide_end = Vec2::new(c.x, c.y);
                    }
                }
            }
            AnimVariant::Alpha => {
                let s = self.attr_f32(el, "AlphaStart");
                let b = self.attr_f32(el, "AlphaBegin");
                let e = self.attr_f32(el, "AlphaEnd");
                let tex = self.attr(el, "Texture").map(str::to_owned);
                let t = tex.and_then(|n| self.texture(&n));
                let off = self.attr(el, "TextureOffset").map(str::to_owned);
                if let Kind::Anim(a) = &mut self.tree[id].kind {
                    if let Some(x) = s.or(b) {
                        a.alpha_start = x;
                    }
                    if let Some(x) = e {
                        a.alpha_end = x;
                    }
                    if let Some(t) = t {
                        let mut q = Quad::default();
                        q.set_texture(Some(t), None);
                        a.quad = Some(q);
                    }
                    if let (Some(o), Some(q)) = (off, a.quad.as_mut()) {
                        let c = values::extract_coords(&o, scr, (0.0, 0.0));
                        q.src_x = c.x;
                        q.src_y = c.y;
                    }
                }
            }
            AnimVariant::Scroll | AnimVariant::Flip => {
                if v == AnimVariant::Flip {
                    if let Some(s) = self.attr(el, "StepSize").map(str::to_owned) {
                        let c = values::extract_coords(&s, scr, (0.0, 0.0));
                        let n = self.attr_i32(el, "FrameCount");
                        if let Kind::Anim(a) = &mut self.tree[id].kind {
                            a.flip_step = Vec2::new(c.x, c.y);
                            // The exe stores FrameCount - 1.
                            a.flip_frames = n.unwrap_or(a.flip_frames + 1) - 1;
                        }
                    }
                    let sp = self.attr_f32(el, "Speed");
                    let cols = self.attr_i32(el, "Columns");
                    if let Kind::Anim(a) = &mut self.tree[id].kind {
                        if let Some(s) = sp {
                            a.flip_speed = s;
                        }
                        if let Some(c) = cols {
                            a.flip_columns = c;
                        }
                    }
                }
                let tex = self.attr(el, "Texture").map(str::to_owned);
                let t = tex.and_then(|n| self.texture(&n));
                let mask = self.attr(el, "MaskTexture").map(str::to_owned);
                let m = mask.and_then(|n| self.texture(&n));
                if let Kind::Anim(a) = &mut self.tree[id].kind {
                    let q = a.quad.get_or_insert_with(Quad::default);
                    q.set_texture(t, m);
                }
            }
        }
        // AnimControl::BuildFromXML
        let speed = self.attr_f32(el, "Speed");
        let pause = self.attr_f32(el, "Pause");
        let end_pause = self.attr_f32(el, "EndPause");
        let cycle = self.attr(el, "Cycle").map(str::to_ascii_lowercase);
        let func = self.attr(el, "Function").map(str::to_owned);
        let stopped = self.attr_bool(el, "Stopped");
        let power = self.attr_f32(el, "FunctionPower");
        if let Kind::Anim(a) = &mut self.tree[id].kind {
            if let Some(s) = speed {
                a.speed = s;
            }
            if let Some(s) = pause {
                a.pause = s;
            }
            if let Some(s) = end_pause {
                a.end_pause = s;
            }
            match cycle.as_deref() {
                Some("once") => a.cycle = 0,
                Some("bounce") => a.cycle = 2,
                Some("onebounce") => a.cycle = 3,
                _ => {}
            }
            if let Some(f) = func {
                if f.starts_with(['S', 's']) {
                    a.function = 1;
                } else if f.starts_with(['R', 'r']) {
                    a.function = 2;
                }
            }
            if stopped == Some(true) {
                a.stopped = true;
            }
            if let Some(p) = power {
                a.power = p;
            }
            // SIMPLIFIED(SIMP-uic-8): the Speed of a FlipAnim is its frames per second (every FlipAnim of the EUI XML
            // has Speed 10..20 over 8..32 frames: a run of about a second and a half), so a run takes
            // FrameCount / Speed seconds. AnimControl's speed is runs per second: reading 20 as that played the
            // 31 frames of the war icon in 50 ms (live report 2026-10-30).
            if a.variant == AnimVariant::Flip && speed.is_some() {
                a.speed = a.flip_speed / (a.flip_frames + 1).max(1) as f32;
            }
        }
        self.base_build(id, el);
    }

    fn build_stack(&mut self, id: ControlId, el: &Rc<XmlElem>) {
        self.build_from_style(id, el);
        if let Some(d) = self.attr(el, "StackGrowth").and_then(values::growth_dir)
            && let Kind::Stack(s) = &mut self.tree[id].kind
        {
            s.dir = d;
        }
        if let Some(w) = self.attr_i32(el, "WrapWidth")
            && let Kind::Stack(s) = &mut self.tree[id].kind
        {
            s.wrap_width = w as i16 as i32;
        }
        if let Some(d) = self.attr(el, "WrapGrowth").and_then(values::growth_dir)
            && let Kind::Stack(s) = &mut self.tree[id].kind
        {
            s.wrap_dir = d;
        }
        // Padding is one number, on the axis of the stack; WrapPadding is on the other axis.
        let padding = self.attr_f32(el, "Padding");
        let wrap_pad = self.attr(el, "WrapPadding").map(values::to_float);
        if let Kind::Stack(s) = &mut self.tree[id].kind {
            let horizontal = |d: i32| d >= 2;
            if let Some(p) = padding {
                if horizontal(s.dir) {
                    s.padding.x = p;
                } else {
                    s.padding.y = p;
                }
            }
            if let Some(p) = wrap_pad
                && s.wrap_dir != s.dir
            {
                if horizontal(s.wrap_dir) {
                    s.padding.x = p;
                } else {
                    s.padding.y = p;
                }
            }
        }
        self.base_build(id, el);
    }

    /// `GridControl::BuildCoreFromXML`.
    pub fn build_grid_core(&mut self, id: ControlId, el: &Rc<XmlElem>) {
        let scr = self.screen();
        let pair = |s: &mut State, name: &str| -> Option<Vec2> {
            s.attr(el, name).map(|v| {
                let c = values::extract_coords(v, scr, (0.0, 0.0));
                Vec2::new(c.x, c.y)
            })
        };
        let tex_offset = pair(self, "TextureOffset");
        let mut cells = Vec::new();
        for (i, n) in ["C", "UC", "LC", "L", "R", "UL", "UR", "LL", "LR"]
            .iter()
            .enumerate()
        {
            let size = pair(self, &format!("{n}Size"));
            let start = pair(self, &format!("{n}TexStart"));
            cells.push((i, size, start));
        }
        let hk = pair(self, "HKerning");
        let vk = pair(self, "VKerning");
        let tex = self.attr(el, "Texture").map(str::to_owned);
        let t = tex.and_then(|n| self.texture(&n));
        let step = pair(self, "StateOffsetIncrement");
        let no_state = self.attr_bool(el, "NoStateChange");
        let auto = self.attr_bool(el, "AutoSize");
        let pad = pair(self, "Padding");
        if no_state == Some(true) {
            self.tree[id].flags |= grid_flag::NO_STATE_CHANGE;
        }
        if auto == Some(true) {
            self.tree[id].flags |= grid_flag::AUTO_SIZE;
        }
        if let Kind::Grid(g) = &mut self.tree[id].kind {
            if let Some(o) = tex_offset {
                g.tex_offset = o;
            }
            for (i, size, start) in cells {
                if let Some(s) = size {
                    g.cell_size[i] = s;
                }
                if let Some(s) = start {
                    g.cell_min[i] = s;
                }
            }
            if let Some(h) = hk {
                g.hkerning = [h.x, h.y];
            }
            if let Some(v) = vk {
                g.vkerning = [v.x, v.y];
            }
            if t.is_some() {
                g.texture = t;
            }
            if let Some(s) = step {
                g.state_step = s;
            }
            if let Some(p) = pad {
                g.auto_margin = p;
            }
        }
    }

    fn apply_scroll_panel(&mut self, id: ControlId, el: &Rc<XmlElem>) {
        let owner = self.tree[id].owner;
        self.build_from_style(id, el);
        if let Some(v) = self.attr(el, "AutoScrollBar")
            && v.starts_with(['f', 'F', '0'])
        {
            self.tree[id].flags |= sp_flag::NO_AUTO_BAR;
        }
        for (name, bit) in [
            ("HideScrollBar", sp_flag::BAR_HIDDEN),
            ("Vertical", sp_flag::VERTICAL),
            ("FullClip", sp_flag::FULL_CLIP),
        ] {
            if self.attr_bool(el, name) == Some(true) {
                self.tree[id].flags |= bit;
            }
        }
        for name in ["UpButton", "DownButton"] {
            if let Some(c) = el.child(name).cloned() {
                self.touch_more(&c);
                let kind = self.new_button_kind();
                let b = self.new_node(kind, owner);
                self.build_from_xml(b, &c);
                self.tree[b].parent = Some(id);
                if let Kind::ScrollPanel(s) = &mut self.tree[id].kind {
                    if name == "UpButton" {
                        s.up = Some(b);
                    } else {
                        s.down = Some(b);
                    }
                }
            }
        }
        if let Some(c) = el.child("ScrollBar").cloned() {
            self.touch_more(&c);
            let kind = self.make_slider(owner);
            let bar = self.new_node(kind, owner);
            self.link_compounds(bar);
            self.build_from_xml(bar, &c);
            self.tree[bar].parent = Some(id);
            if let Kind::ScrollPanel(s) = &mut self.tree[id].kind {
                s.bar = Some(bar);
            }
        }
        self.base_build(id, el);
        let size = self.tree[id].size;
        if let Kind::ScrollPanel(s) = &mut self.tree[id].kind {
            s.inner = size;
        }
    }

    fn make_slider(&mut self, owner: Option<ControlId>) -> Kind {
        let track = self.new_node(Kind::Grid(Box::default()), owner);
        let shuttle = self.new_node(Kind::Grid(Box::default()), owner);
        Kind::Slider(Box::new(Slider {
            base: ButtonBase::default(),
            track,
            shuttle,
            value: -1.0,
            travel: 0,
            padding: 0,
            grab: 0.0,
            wheel_steps: 15.0,
            callback: None,
            vertical: false,
            dragging: false,
        }))
    }

    fn apply_slider(&mut self, id: ControlId, el: &Rc<XmlElem>) {
        let Kind::Slider(sl) = &self.tree[id].kind else {
            return;
        };
        let (track, shuttle) = (sl.track, sl.shuttle);
        self.build_from_style(id, el);
        let wheel = self.attr_f32(el, "WheelSteps");
        let gutter = self.attr_i32(el, "Gutter");
        if let Kind::Slider(s) = &mut self.tree[id].kind {
            if let Some(w) = wheel {
                s.wheel_steps = w;
            }
            if let Some(g) = gutter {
                s.padding = g;
            }
        }
        self.build_grid_core(track, el);
        if let Some(sh) = el.child("Shuttle").cloned() {
            self.touch_more(&sh);
            self.build_from_xml(shuttle, &sh);
        }
        if self.attr_bool(el, "Vertical") == Some(true)
            && let Kind::Slider(s) = &mut self.tree[id].kind
        {
            s.vertical = true;
        }
        self.build_button_base(id, el);
        // Length is the size along the axis; Value the start position.
        let length = self.attr_f32(el, "Length");
        let value = self.attr_f32(el, "Value");
        self.base_build(id, el);
        if let Some(l) = length {
            let vertical = matches!(&self.tree[id].kind, Kind::Slider(s) if s.vertical);
            let n = &mut self.tree[id];
            if vertical {
                n.size.y = l;
            } else {
                n.size.x = l;
            }
        }
        if let (Some(v), Kind::Slider(s)) = (value, &mut self.tree[id].kind) {
            s.value = v;
        }
        let sz = self.tree[id].size;
        self.tree[track].size = sz;
    }

    fn make_edit_box(&mut self, owner: Option<ControlId>) -> Kind {
        // SIMPLIFIED(SIMP-uic-4): no IME composition; the box takes WM_CHAR text only.
        let label = self.new_label_node(owner);
        Kind::EditBox(Box::new(EditBox {
            label,
            text: String::new(),
            max_length: 0,
            number_input: false,
            obscure: false,
            call_on_char: false,
            edit_mode: false,
            keep_focus: false,
            focus_stop: -1,
            void1: 0,
            void2: 0,
            callback: None,
            cursor: 0,
        }))
    }

    fn apply_edit_box(&mut self, id: ControlId, el: &Rc<XmlElem>) {
        let Kind::EditBox(eb) = &self.tree[id].kind else {
            return;
        };
        let label = eb.label;
        self.build_from_style(id, el);
        self.build_label_core(label, el);
        let max = self.attr_i32(el, "MaxLength");
        let num = self.attr_bool(el, "NumberInput");
        let obs = self.attr_bool(el, "Obscure");
        let coc = self.attr_bool(el, "CallOnChar");
        let em = self.attr_bool(el, "EditMode");
        let keep = self.attr_bool(el, "KeepFocus");
        let fs = self.attr_i32(el, "FocusStop");
        let v1 = self.attr_i32(el, "Void1");
        let v2 = self.attr_i32(el, "Void2");
        let _ = self.attr(el, "CursorColor");
        let _ = self.attr(el, "HighlightColor");
        if let Kind::EditBox(e) = &mut self.tree[id].kind {
            e.max_length = max.unwrap_or(e.max_length);
            e.number_input = num.unwrap_or(false);
            e.obscure = obs.unwrap_or(false);
            e.call_on_char = coc.unwrap_or(false);
            e.edit_mode = em.unwrap_or(false);
            e.keep_focus = keep.unwrap_or(false);
            e.focus_stop = fs.unwrap_or(e.focus_stop);
            e.void1 = v1.unwrap_or(0);
            e.void2 = v2.unwrap_or(0);
        }
        self.base_build(id, el);
    }

    fn make_pull_down(&mut self, owner: Option<ControlId>) -> Kind {
        let grid = self.new_node(Kind::Grid(Box::default()), owner);
        let scroll = self.new_node(Kind::ScrollPanel(Box::default()), owner);
        let stack = self.new_node(Kind::Stack(Stack::default()), owner);
        Kind::PullDown(Box::new(PullDown {
            main: None,
            grid,
            scroll,
            stack,
            scroll_threshold: 100,
            auto_size_popup: false,
            space_for_scroll: false,
            instances: HashMap::new(),
            entries: Vec::new(),
            open: false,
            callback: None,
            origin: Vec2::ZERO,
        }))
    }

    fn apply_pull_down(&mut self, id: ControlId, el: &Rc<XmlElem>) {
        let Kind::PullDown(pd) = &self.tree[id].kind else {
            return;
        };
        let (grid, scroll, stack) = (pd.grid, pd.scroll, pd.stack);
        let owner = self.tree[id].owner;
        self.build_from_style(id, el);
        let thr = self.attr_i32(el, "ScrollThreshold");
        let auto = self
            .attr_bool(el, "AutoSizePopUp")
            .or_else(|| self.attr_bool(el, "AutoSizePopup"));
        let space = self.attr_bool(el, "SpaceForScroll");
        let mut main = None;
        if let Some(bd) = el.child("ButtonData").cloned() {
            self.touch_more(&bd);
            if let Some(c) = bd.children.first() {
                main = self.build_control(c, owner);
                if let Some(m) = main {
                    self.tree[m].parent = Some(id);
                }
            }
        }
        if let Some(d) = el.child("GridData").cloned() {
            self.touch_more(&d);
            self.build_from_xml(grid, &d);
        }
        if let Some(d) = el.child("ScrollPanelData").cloned() {
            self.touch_more(&d);
            self.build_from_xml(scroll, &d);
        }
        self.add_child(grid, scroll);
        if let Some(d) = el.child("StackData").cloned() {
            self.touch_more(&d);
            self.build_from_xml(stack, &d);
        }
        self.add_child(scroll, stack);
        let mut inst = HashMap::new();
        for d in el.children_named("InstanceData") {
            self.touch_more(d);
            if let (Some(name), Some(first)) = (self.attr(d, "Name"), d.children.first()) {
                inst.insert(name.to_owned(), first.clone());
            }
        }
        if let Kind::PullDown(p) = &mut self.tree[id].kind {
            if main.is_some() {
                p.main = main;
            }
            p.scroll_threshold = thr.unwrap_or(p.scroll_threshold);
            p.auto_size_popup = auto.unwrap_or(p.auto_size_popup);
            p.space_for_scroll = space.unwrap_or(p.space_for_scroll);
            p.instances.extend(inst);
        }
        self.base_build(id, el);
    }

    /// Puts the types that waited for `RegisterBasicControls` into the content container.
    pub fn build_pending_tooltip_types(&mut self) {
        let Some(content) = self.tooltips.content else {
            return;
        };
        for c in std::mem::take(&mut self.tooltips.pending_types) {
            if self.tree.alive(c) {
                self.add_child(content, c);
                self.tooltips.init_pending.push(c);
            }
        }
    }

    /// `ToolTipManager::RegisterToolTipType`. SIMPLIFIED(SIMP-exe-73): a type read before the basic controls exist is built
    /// at once (its controls are looked up by `GetTypeControlTable` of contexts that load before ToolTips, UnitFlagManager)
    /// and put into the content container when they are registered (the exe's manager is there from the start).
    fn register_tooltip_type(&mut self, el: &Rc<XmlElem>, owner: Option<ControlId>) {
        let Some(name) = self.attr(el, "Name").map(str::to_owned) else {
            return;
        };
        let Some(first) = el.children.first().cloned() else {
            return;
        };
        let hash = crc32(&name);
        if let Some(old) = self.tooltips.types.remove(&hash) {
            if let Some(content) = self.tooltips.content {
                self.release_child(content, old);
            } else {
                self.tooltips.pending_types.retain(|&c| c != old);
            }
        }
        if let Some(c) = self.build_control(&first, owner) {
            self.tooltips.types.insert(hash, c);
            self.tree[c].flags |= flags::HIDDEN;
            if let Some(content) = self.tooltips.content {
                self.add_child(content, c);
                // SIMPLIFIED(SIMP-exe-422): the exe's manager initialises the type at once; here it happens at the next update.
                self.tooltips.init_pending.push(c);
            } else {
                self.tooltips.pending_types.push(c);
            }
        }
    }

    /// The `ID`s of a subtree, for `ExposeTree`: controls with a non-empty ID in document order,
    /// descending into compound parts the way each class's `ExposeTree` does.
    pub fn expose_list(&self, id: ControlId, out: &mut Vec<ControlId>) {
        let n = &self.tree[id];
        // ScrollPanel and PullDown expose their compound parts first.
        match &n.kind {
            Kind::ScrollPanel(s) => {
                for c in [s.bar, s.up, s.down].into_iter().flatten() {
                    self.expose_list(c, out);
                }
            }
            Kind::PullDown(p) => {
                if let Some(m) = p.main {
                    self.expose_list(m, out);
                }
                for c in [p.grid, p.scroll, p.stack] {
                    self.expose_list(c, out);
                }
            }
            _ => {}
        }
        if !n.id.is_empty() {
            out.push(id);
        }
        for &c in &n.children {
            self.expose_list(c, out);
        }
    }
}

pub(crate) fn check_data_mut(kind: &mut Kind) -> Option<&mut CheckBox> {
    match kind {
        Kind::CheckBox(c) | Kind::RadioButton(c) => Some(c),
        _ => None,
    }
}
