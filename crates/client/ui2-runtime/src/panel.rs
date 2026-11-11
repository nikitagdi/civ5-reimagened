//! Panels: the units the UI fails and reports by.
//!
//! A panel is a top-level part of the skin (a child of a layer root, or a top-level node, with a
//! `data-part` name) or a mounted mod fragment. When a panel cannot work any more (its mod's script context
//! was switched off) the host calls [`Runtime::set_panel_failed`]: its subtree goes, with its bindings, and a
//! placeholder box of the same size stands in its place until the skin is loaded again. The neighbours are
//! not touched beyond the layout of the parent.

use std::sync::Arc;

use ui2_package::StrId;
use ui2_package::style::{
    BorderStyle, BoxSizing, ColorV, Display, Len, LenAuto, Overflow, StyleRecord, TextOverflow,
    TextWrapMode,
};

use crate::arena::{Dirty, Node, NodeClass, NodeId, PLACEHOLDER_STYLE};
use crate::error::{ErrorCode, UiError};
use crate::instantiate;
use crate::style::Overrides;
use crate::{Runtime, layout};

/// A panel of the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PanelId(pub u32);

/// How a mod panel enters its extension point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountMode {
    /// After the skin's own children of the point and the panels mounted before (by order).
    Append,
    /// In place of the point's node, which is hidden while the panel is mounted.
    Replace,
}

/// What [`Runtime::panels`] tells about one panel (for the `ui2()` console and the host's mod checks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelStatus {
    /// The panel.
    pub id: PanelId,
    /// Its name: the part name of a skin panel, the panel id of a mod panel.
    pub name: String,
    /// The mod that mounted it; `None` for a part of the skin.
    pub owner: Option<Arc<str>>,
    /// The extension point a mod panel is mounted in.
    pub slot: Option<String>,
    /// How a mod panel is mounted.
    pub mode: Option<MountMode>,
    /// Why the panel shows a placeholder, when it does.
    pub failed: Option<String>,
    /// Live nodes of the panel (the placeholder counts one).
    pub nodes: u32,
}

/// One registered panel.
#[derive(Debug, Clone)]
pub(crate) struct Panel {
    pub name: String,
    pub owner: Option<Arc<str>>,
    pub slot: Option<String>,
    pub mode: Option<MountMode>,
    /// The top node of the panel (the placeholder once it failed).
    pub root: Option<NodeId>,
    pub failed: Option<String>,
    /// The order of a mod panel inside its point (the host's mod order).
    pub order: u32,
    /// The node of the skin a `Replace` panel hides.
    pub hides: Option<NodeId>,
}

/// The panels of a runtime; a `PanelId` is the index. Unmounted panels leave a hole (ids are not reused).
#[derive(Debug, Default)]
pub(crate) struct Panels {
    pub list: Vec<Option<Panel>>,
}

impl Panels {
    pub fn add(&mut self, p: Panel) -> PanelId {
        self.list.push(Some(p));
        PanelId(self.list.len() as u32 - 1)
    }

    pub fn get_mut(&mut self, id: PanelId) -> Option<&mut Panel> {
        self.list.get_mut(id.0 as usize)?.as_mut()
    }
}

/// The smallest height of a placeholder whose panel never had a box.
const PLACEHOLDER_MIN_PX: f32 = 20.0;

/// The font size of the panel name a placeholder shows.
const PLACEHOLDER_FONT_PX: f32 = 12.0;

/// The record of a failed panel's placeholder: a translucent red box with a thin border, of the panel's last
/// size, that shows the panel's name on one line (cut with an ellipsis) in the package's first font family.
/// The look is built in: a package carries style records only for the nodes of its templates, so a skin
/// rule for placeholders has no record to take.
fn failed_record(size: taffy::Size<f32>, family: Option<StrId>) -> StyleRecord {
    let mut r = name_record(family);
    r.display = Display::Block;
    r.box_sizing = BoxSizing::BorderBox;
    if size.width > 0.0 {
        r.width = LenAuto::Len(Len::px(size.width));
    }
    r.height = LenAuto::Len(Len::px(size.height.max(PLACEHOLDER_MIN_PX)));
    r.flex_shrink = 0.0;
    r.background_color = ColorV::Lit([170, 60, 50, 72]);
    let edge = ColorV::Lit([200, 80, 60, 220]);
    r.border_top_width = Len::px(1.0);
    r.border_right_width = Len::px(1.0);
    r.border_bottom_width = Len::px(1.0);
    r.border_left_width = Len::px(1.0);
    r.border_top_style = BorderStyle::Solid;
    r.border_right_style = BorderStyle::Solid;
    r.border_bottom_style = BorderStyle::Solid;
    r.border_left_style = BorderStyle::Solid;
    r.border_top_color = edge;
    r.border_right_color = edge;
    r.border_bottom_color = edge;
    r.border_left_color = edge;
    r.padding_left = Len::px(4.0);
    r.padding_right = Len::px(4.0);
    r.overflow_x = Overflow::Hidden;
    r.overflow_y = Overflow::Hidden;
    r.text_overflow = TextOverflow::Ellipsis;
    r
}

/// The text style of a placeholder (its box's, and its name's).
fn name_record(family: Option<StrId>) -> StyleRecord {
    let mut r = StyleRecord::initial();
    r.font_family = family.into_iter().collect();
    r.font_size = Len::px(PLACEHOLDER_FONT_PX);
    r.color = ColorV::Lit([240, 214, 204, 255]);
    r.text_wrap_mode = TextWrapMode::Nowrap;
    r
}

impl Runtime {
    /// Registers the top-level parts of the skin as panels.
    pub(crate) fn register_skin_panels(&mut self) {
        let mut tops = Vec::new();
        for &r in self.arena.roots() {
            match self.arena.get(r).map(Node::class) {
                Some(NodeClass::Layer) => {
                    tops.extend(self.arena.get(r).map_or(&[][..], |n| n.children()).iter())
                }
                Some(_) => tops.push(r),
                None => {}
            }
        }
        for id in tops {
            let name = self
                .arena
                .get(id)
                .and_then(|n| instantiate::template_node(&*self.pkg, n.tmpl()))
                .and_then(|t| t.part)
                .and_then(|p| self.pkg.str(p))
                .map(str::to_string);
            if let Some(name) = name {
                self.panels.add(Panel {
                    name,
                    owner: None,
                    slot: None,
                    mode: None,
                    root: Some(id),
                    failed: None,
                    order: 0,
                    hides: None,
                });
            }
        }
    }

    /// The panels: the skin's top-level parts, then the mod panels in mount order.
    pub fn panels(&self) -> Vec<PanelStatus> {
        self.panels
            .list
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let p = p.as_ref()?;
                Some(PanelStatus {
                    id: PanelId(i as u32),
                    name: p.name.clone(),
                    owner: p.owner.clone(),
                    slot: p.slot.clone(),
                    mode: p.mode,
                    failed: p.failed.clone(),
                    nodes: p.root.map_or(0, |r| self.subtree_len(r)),
                })
            })
            .collect()
    }

    /// The number of live nodes under a node, itself included.
    fn subtree_len(&self, id: NodeId) -> u32 {
        let mut n = 0;
        let mut stack = vec![id];
        while let Some(at) = stack.pop() {
            if let Some(node) = self.arena.get(at) {
                n += 1;
                stack.extend(node.children().iter().copied());
            }
        }
        n
    }

    /// Replaces a panel by a placeholder until the skin is loaded again: its nodes and bindings go, a box of
    /// its last size (at least a line high) takes its place, and one [`ErrorCode::PanelFailed`] error with
    /// the reason is reported. A panel that already failed, an unknown id, or a panel without nodes changes
    /// nothing but the reason shown by [`panels`](Self::panels).
    pub fn set_panel_failed(&mut self, id: PanelId, reason: &str) {
        let Some(p) = self.panels.get_mut(id) else {
            return;
        };
        if p.failed.is_some() {
            return;
        }
        p.failed = Some(reason.to_string());
        let name = p.name.clone();
        let owner = p.owner.clone();
        let root = p.root.filter(|&r| self.arena.contains(r));
        if let Some(root) = root {
            let placeholder = self.stub(root, &name);
            if let Some(p) = self.panels.get_mut(id) {
                p.root = placeholder;
            }
        }
        let who = owner.map_or_else(String::new, |o| format!(" of mod {o}"));
        let e = UiError {
            path: format!("panel {name}{who}"),
            panel: Some(name),
            code: ErrorCode::PanelFailed,
            msg: format!("shows a placeholder: {reason}"),
        };
        self.errors.report(e);
    }

    /// Takes a subtree out and puts a placeholder of its last size at its place, showing the panel's name
    /// (when the package has a font to show it in). Returns the placeholder.
    fn stub(&mut self, root: NodeId, name: &str) -> Option<NodeId> {
        let family = self.pkg.skin().fonts().first().map(|f| f.family);
        let n = self.arena.get(root)?;
        let (parent, tmpl, size) = (n.parent(), n.tmpl(), n.layout().unrounded.size);
        let own_chunk = n.chunk == root.index();
        let at = self
            .arena
            .siblings(parent)
            .iter()
            .position(|&c| c == root)?;
        self.arena.remove(root);
        let mut node = Node::new(tmpl, NodeClass::Placeholder, parent);
        node.style = PLACEHOLDER_STYLE;
        node.over = Some(Box::new(overrides(failed_record(size, family))));
        let ph = self.arena.alloc(node);
        let chunk = if own_chunk {
            ph.index()
        } else {
            instantiate::chunk_of(&self.arena, ph, parent)
        };
        if let Some(n) = self.arena.get_mut(ph) {
            n.chunk = chunk;
        }
        self.arena.attach(parent, at, ph);
        self.arena.mark(ph, Dirty::LAYOUT | Dirty::PAINT);
        if family.is_some() {
            let mut text = Node::new(tmpl, NodeClass::Text, Some(ph));
            text.style = PLACEHOLDER_STYLE;
            text.text = Some(Arc::from(name));
            text.over = Some(Box::new(overrides(name_record(family))));
            text.chunk = chunk;
            let t = self.arena.alloc(text);
            self.arena.attach(Some(ph), 0, t);
            self.arena.mark(t, Dirty::TEXT | Dirty::PAINT);
        }
        if let Some(p) = parent {
            self.arena.mark(p, Dirty::LAYOUT | Dirty::PAINT);
        }
        self.arena.bump_revision();
        Some(ph)
    }
}

/// Style properties that give a node its own record.
fn overrides(record: StyleRecord) -> Overrides {
    Overrides {
        props: Vec::new(),
        taffy: layout::style::taffy_style(&record),
        record,
    }
}
