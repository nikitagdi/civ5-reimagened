//! Contexts: `LuaContext` as a control plus the Lua state it runs in. This file has the XML side
//! (`LuaContext::BuildFromXML`, `LoadXML`, layout defaults); the Lua side is in `shared.rs`.

use std::rc::Rc;

use crate::state::State;
use crate::tree::*;
use crate::xml::XmlElem;

impl State {
    /// `LuaContext::BuildFromXML`: the style, `FileName`, `DeferLoad`, then the base.
    pub fn build_context_xml(&mut self, id: ControlId, el: &Rc<XmlElem>) {
        self.build_from_style(id, el);
        if let Some(p) = self.attr(el, "FileName").map(str::to_owned)
            && let Some(c) = self.ctx_mut(id)
        {
            c.path = p;
        }
        if self.attr_bool(el, "DeferLoad") == Some(true) {
            self.tree[id].flags |= ctx_flag::DEFER_LOAD;
        }
        let owner = self.tree[id].owner;
        self.base_build(id, el);
        self.tree[id].owner = owner;
    }

    /// `LuaContext::LoadLayoutDefaults` for the root element of a file.
    pub fn load_layout_defaults(&mut self, ctx: ControlId, root: &Rc<XmlElem>) {
        // Local colour sets.
        for cs in root.children_named("ColorSet") {
            let Some(name) = cs.attr("Name", self.cfg.attr_case_insensitive) else {
                continue;
            };
            let mut set = crate::style::ColorSet::default();
            for (i, slot) in set.colors.iter_mut().enumerate() {
                if let Some(v) = cs.attr(&format!("Color{i}"), self.cfg.attr_case_insensitive)
                    && let Some(c) = self.styles.extract_color(v)
                {
                    *slot = c;
                }
            }
            let name = name.to_owned();
            if let Some(c) = self.ctx_mut(ctx) {
                c.color_sets.entry(name).or_insert(set);
            }
        }
        let ci = self.cfg.attr_case_insensitive;
        let mut set = None;
        if let Some(name) = root.attr("ColorSet", ci) {
            let local = self.ctx(ctx).and_then(|c| c.color_sets.get(name).copied());
            set = local.or_else(|| self.styles.color_set(name));
        }
        let colors: Vec<(usize, [f32; 4])> = (0..3)
            .filter_map(|i| {
                root.attr(&format!("Color{i}"), ci)
                    .and_then(|v| self.styles.extract_color(v))
                    .map(|c| (i, c))
            })
            .collect();
        let font = root.attr("Font", ci).map(str::to_owned);
        let style = root.attr("FontStyle", ci).map(str::to_owned);
        if let Some(c) = self.ctx_mut(ctx) {
            if let Some(s) = set {
                c.default_set = s;
            }
            for (i, col) in colors {
                c.default_set.colors[i] = col;
            }
            if font.is_some() {
                c.default_font = font;
            }
            if let Some(s) = style {
                c.default_style = s;
            }
        }
    }
}
