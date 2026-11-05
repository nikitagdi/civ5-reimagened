//! The state of the UI core: the tree, the style book, the services, and the queues that the
//! Lua side drains. Everything here is plain Rust; calls into Lua are queued as [`Pending`]
//! and made by the owner of the state after it let go of its borrow.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use glam::Vec2;

use crate::config::UiConfig;
use crate::flags;
use crate::host::{TextureInfo, UiHost};
use crate::style::StyleBook;
use crate::text::{LocArg, TextEngine};
use crate::tree::{Callback, ControlId, Kind, Node, TexRef, Tree};
use crate::xml::XmlElem;

/// An argument for a queued Lua call.
#[derive(Debug, Clone)]
pub enum LuaArg {
    Nil,
    Bool(bool),
    Int(i32),
    Num(f64),
    Str(String),
    Control(ControlId),
}

/// A Lua call the tree layer wants made.
pub enum Pending {
    /// A callback of a control: `owner` is the context whose Lua state runs it.
    Call {
        owner: Option<ControlId>,
        key: Callback,
        args: Vec<LuaArg>,
    },
    /// `ShowHideHandler(bIsHide, false)` of a context.
    ShowHide { ctx: ControlId, hide: bool },
    /// `SetHide` of a context (show/hide handler, deferred load).
    SetHide { ctx: ControlId, hide: bool },
    /// A deferred context is shown for the first time: load it.
    InitContext { ctx: ControlId },
    /// `UIManager:QueuePopup` / `PushModal` ended up hiding or showing a context.
    Shutdown { ctx: ControlId },
}

/// A problem in a UI file (AC-F1). One per file and name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlWarning {
    pub file: String,
    pub line: u32,
    pub tag: String,
    pub what: String,
}

/// Counters for the debug panel.
#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub xml_warnings: u64,
    pub controls_built: u64,
    pub texture_misses: u64,
}

/// `ToolTipManager` state.
#[derive(Default)]
pub struct ToolTips {
    /// A control switched its tip on (`EnableToolTip(true)`) while the mouse rests: the next update asks again which control
    /// under the mouse has a tip (PlotHelpManager.lua enables the full-screen box's tip after the mouse stood still).
    pub recheck: bool,
    pub requested: Option<ControlId>,
    pub current: Option<ControlId>,
    pub root: Option<ControlId>,
    pub content: Option<ControlId>,
    pub text: Option<ControlId>,
    pub grid: Option<ControlId>,
    pub type_hash: u32,
    /// Basic controls are registered (flag 2) and the tooltip is shown (flag 1).
    pub registered: bool,
    pub shown: bool,
    pub types: HashMap<u32, ControlId>,
    /// Types that were read before the basic controls were registered (EUI_context loads before ToolTips): built at
    /// once, so `GetTypeControlTable` of a context that loads before ToolTips finds their controls, and put into the
    /// content container when the basic controls are registered.
    pub pending_types: Vec<ControlId>,
    /// Types built after the tooltip context was initialised: they are initialised at the next `update` (a label that is not
    /// initialised keeps size 0 after `SetText`, and the tip's `Box:DoAutoSize()` made a 16x16 box).
    pub init_pending: Vec<ControlId>,
}

pub struct State {
    pub tree: Tree,
    pub cfg: UiConfig,
    pub styles: StyleBook,
    pub host: Box<dyn UiHost>,
    pub text: Box<dyn TextEngine>,
    pub roots: Vec<ControlId>,
    pub warnings: Vec<XmlWarning>,
    warned: HashSet<(String, String)>,
    consumed: HashSet<(usize, usize)>,
    touched: Vec<Rc<XmlElem>>,
    pub stats: Stats,
    pub pending: Vec<Pending>,
    /// Mouse position in UI pixels and the change since the last move.
    pub mouse: Vec2,
    pub mouse_delta: Vec2,
    pub mouse_down: Vec2,
    pub key_state: u32,
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    /// Controls the mouse is over, and the ones requested during the current move.
    pub hover: Vec<ControlId>,
    pub hover_requests: Vec<ControlId>,
    pub tooltips: ToolTips,
    /// Modal stack, bottom first.
    pub modals: Vec<ControlId>,
    pub focus: Option<ControlId>,
    /// `UIManager:QueuePopup`: contexts by priority, the head first; the head is shown.
    pub popups: Vec<(ControlId, u32)>,
    pub popup_current: Option<ControlId>,
    pub world_anchors_hidden: bool,
    /// Contexts by their `ID`, in the order they were added as roots.
    pub elapsed: f64,
}

impl State {
    pub fn new(
        cfg: UiConfig,
        styles: StyleBook,
        host: Box<dyn UiHost>,
        text: Box<dyn TextEngine>,
    ) -> State {
        State {
            tree: Tree::default(),
            cfg,
            styles,
            host,
            text,
            roots: Vec::new(),
            warnings: Vec::new(),
            warned: HashSet::new(),
            consumed: HashSet::new(),
            touched: Vec::new(),
            stats: Stats::default(),
            pending: Vec::new(),
            mouse: Vec2::ZERO,
            mouse_delta: Vec2::ZERO,
            mouse_down: Vec2::ZERO,
            key_state: 0,
            shift: false,
            alt: false,
            ctrl: false,
            hover: Vec::new(),
            hover_requests: Vec::new(),
            tooltips: ToolTips::default(),
            modals: Vec::new(),
            focus: None,
            popups: Vec::new(),
            popup_current: None,
            world_anchors_hidden: false,
            elapsed: 0.0,
        }
    }

    pub fn screen(&self) -> (i32, i32) {
        (self.cfg.screen_width, self.cfg.screen_height)
    }

    // ---- attribute reading with bookkeeping ----

    /// `GetFirstAttributeVal`, remembering that the attribute was read.
    pub fn attr<'e>(&mut self, el: &'e XmlElem, name: &str) -> Option<&'e str> {
        let i = el.attr_index(name, self.cfg.attr_case_insensitive)?;
        self.consumed.insert((el as *const XmlElem as usize, i));
        Some(el.attrs[i].1.as_str())
    }

    pub fn attr_f32(&mut self, el: &XmlElem, name: &str) -> Option<f32> {
        self.attr(el, name).map(crate::values::to_float)
    }

    pub fn attr_i32(&mut self, el: &XmlElem, name: &str) -> Option<i32> {
        self.attr(el, name).map(crate::values::to_int)
    }

    pub fn attr_bool(&mut self, el: &XmlElem, name: &str) -> Option<bool> {
        self.attr(el, name).map(crate::values::parse_bool)
    }

    /// Starts watching an element for attributes nobody reads.
    pub fn touch(&mut self, el: &Rc<XmlElem>) -> usize {
        let mark = self.touched.len();
        // An element already watched by an outer call is reported there, once.
        if !self.touched.iter().any(|t| Rc::ptr_eq(t, el)) {
            self.touched.push(el.clone());
        }
        mark
    }

    /// Adds a style element to the elements of the control being built.
    pub fn touch_more(&mut self, el: &Rc<XmlElem>) {
        self.touched.push(el.clone());
    }

    /// Warns about the attributes of the elements touched since `mark` that were not read.
    pub fn finish_touched(&mut self, mark: usize) {
        let els: Vec<Rc<XmlElem>> = self.touched.drain(mark..).collect();
        for el in els {
            for (i, (name, _)) in el.attrs.iter().enumerate() {
                let key = (Rc::as_ptr(&el) as usize, i);
                if !self.consumed.remove(&key) {
                    // A name that repeats is read through its first copy only.
                    let first = el.attrs.iter().position(|(k, _)| k == name);
                    if first != Some(i) {
                        continue;
                    }
                    let what = format!("unknown attribute {name}");
                    self.warn(&el, &what, name);
                }
            }
        }
    }

    /// One warning per file and subject (`ui.xml`).
    pub fn warn(&mut self, el: &XmlElem, what: &str, subject: &str) {
        if self
            .warned
            .insert((el.file.to_string(), format!("{}:{}", el.name, subject)))
        {
            tracing::warn!(target: "ui.xml", "{}:{}: <{}> {}", el.file, el.line, el.name, what);
            self.stats.xml_warnings += 1;
            self.warnings.push(XmlWarning {
                file: el.file.to_string(),
                line: el.line,
                tag: el.name.clone(),
                what: what.to_owned(),
            });
        }
    }

    // ---- services ----

    /// `UIManager::GetTexture`.
    pub fn texture(&mut self, name: &str) -> Option<TexRef> {
        match self.host.texture(name) {
            Some(TextureInfo { id, width, height }) => Some(TexRef {
                name: name.to_owned(),
                id,
                w: width as f32,
                h: height as f32,
            }),
            None => {
                self.stats.texture_misses += 1;
                None
            }
        }
    }

    /// `Localization::Lookup(key).toUTF8()`.
    pub fn localize(&mut self, key: &str, args: &[LocArg]) -> String {
        self.text.localize(key, args)
    }

    // ---- nodes ----

    pub fn new_node(&mut self, kind: Kind, owner: Option<ControlId>) -> ControlId {
        let mut n = Node::new(kind);
        n.owner = owner;
        n.flags = default_flags(&n.kind);
        self.tree.alloc(n)
    }

    pub fn node(&self, id: ControlId) -> &Node {
        &self.tree[id]
    }

    pub fn node_mut(&mut self, id: ControlId) -> &mut Node {
        &mut self.tree[id]
    }

    pub fn alive(&self, id: ControlId) -> bool {
        self.tree.alive(id)
    }

    pub fn is_hidden(&self, id: ControlId) -> bool {
        self.tree.get(id).is_some_and(|n| n.has(flags::NOT_ACTIVE))
    }

    /// Queues a Lua call for a control's callback.
    pub fn queue_call(&mut self, owner: Option<ControlId>, key: &Callback, args: Vec<LuaArg>) {
        self.pending.push(Pending::Call {
            owner,
            key: key.clone(),
            args,
        });
    }

    /// The context that owns a control: the control itself for a context's own children
    /// is the context; for a context node, the node itself.
    pub fn context_of(&self, id: ControlId) -> Option<ControlId> {
        let n = self.tree.get(id)?;
        if matches!(n.kind, Kind::Context(_)) {
            return Some(id);
        }
        n.owner
    }

    pub fn ctx(&self, id: ControlId) -> Option<&crate::tree::Context> {
        match &self.tree.get(id)?.kind {
            Kind::Context(c) => Some(c),
            _ => None,
        }
    }

    pub fn ctx_mut(&mut self, id: ControlId) -> Option<&mut crate::tree::Context> {
        match &mut self.tree.get_mut(id)?.kind {
            Kind::Context(c) => Some(c),
            _ => None,
        }
    }
}

/// Flags a control of this kind starts with (what the constructors set).
pub fn default_flags(kind: &Kind) -> u32 {
    match kind {
        // ButtonControlBase::ButtonControlBase
        Kind::Button(_)
        | Kind::TextButton(_)
        | Kind::GridButton(_)
        | Kind::CheckBox(_)
        | Kind::RadioButton(_)
        | Kind::ColorBoxButton(_)
        | Kind::Slider(_) => flags::NEEDS_MOUSE_OVER,
        // TextControl::TextControl: (m_Flags & 0xf67fffff) | 0x9000000
        Kind::Label(_) => 0x0900_0000,
        _ => 0,
    }
}

/// CRC-32 of a name, the key of tooltip types (the exe uses `FCRC::Calc`).
pub fn crc32(s: &str) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in s.as_bytes() {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_is_stable_and_distinguishes_names() {
        assert_eq!(crc32("a"), crc32("a"));
        assert_ne!(crc32("EUI_UnitTooltip"), crc32("EUI_CityBannerTooltip"));
        assert_ne!(crc32(""), 0);
    }
}
