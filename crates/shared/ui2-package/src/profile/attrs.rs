//! The attributes of markup: bindings, localization, events and actions, slots, hotkeys, anchors,
//! layers, controls, tooltips, fitting and script islands.
//!
//! [`parse`] checks the grammar of one element's attributes and returns them typed ([`NodeAttrs`]);
//! [`check`] types the expressions and actions against the model schema and the scope the node
//! sits in, rewrites positional intent arguments into named ones and derives a loop's key.
//! Both collect every error they find, each with a code (`E1`, `E4`, `E5`, `E6` and `E7`
//! families, texts in `profile.toml`).
//!
//! ```text
//! data-on-<ev> := action ( ';' action )*
//! action       := 'intent' NAME '(' args ')' | 'set' statepath '=' expr | 'toggle' statepath
//!               | 'call' NAME '(' args ')' | 'stop'
//! args         := ( expr ( ',' expr )* )? | IDENT ':' expr ( ',' IDENT ':' expr )*
//! for          := [ IDENT ',' ] IDENT ':' '{{' expr '}}'
//! class-toggle := CLASS ':' '{{' expr '}}' ( ';' CLASS ':' '{{' expr '}}' )*
//! hotkey       := [ 'Ctrl+' ] [ 'Alt+' ] [ 'Shift+' ] KeyboardEvent.code
//! anchor       := ( 'city' | 'unit' | 'plot' ) ':' '{{' expr '}}'
//! slot, part   := [a-z][a-z0-9-]* ( '.' [a-z0-9-]+ )*
//! ```

mod actions;
mod check;

use std::collections::BTreeSet;

pub use check::{Checked, check};

use super::action::{Action, AnchorKind, ControlKind, Event};
use super::expr::{self, Expr, ExprError, Template, TplSeg};
use super::ids::{PropId, Tag};
use super::props::Profile;
use super::values::parse_value;
use crate::sections::templates::Layer;

/// Error codes of the attribute grammars; the texts are in `profile.toml` (`error.<code>`).
pub mod codes {
    /// A `title` attribute.
    pub const TITLE: &str = "E1-020";
    /// An `on<event>` attribute.
    pub const ON_ATTR: &str = "E1-021";
    /// A `data-*` attribute the profile does not have.
    pub const UNKNOWN_DATA: &str = "E1-022";
    /// A `srcset` attribute.
    pub const SRCSET: &str = "E1-023";
    /// An attribute outside the profile.
    pub const OUTSIDE: &str = "E1-024";
    /// An attribute the element may not have.
    pub const NOT_ON_ELEMENT: &str = "E1-025";
    /// An attribute written twice.
    pub const REPEATED: &str = "E1-026";
    /// A `data-include` path that is not a relative path inside the skin.
    pub const INCLUDE: &str = "E1-027";
    /// An attribute value outside the profile's list.
    pub const VALUE: &str = "E1-028";
    /// A binding that is not exactly one `{{ }}`.
    pub const ONE_EXPR: &str = "E4-020";
    /// A malformed `data-bind-for`.
    pub const FOR: &str = "E4-021";
    /// A loop without a key.
    pub const NO_KEY: &str = "E4-022";
    /// An attribute that needs another one next to it.
    pub const NEEDS: &str = "E4-023";
    /// A malformed `data-bind-class-toggle`.
    pub const CLASS_TOGGLE: &str = "E4-024";
    /// `data-bind-style-<prop>` of a property that cannot be bound.
    pub const STYLE_PROP: &str = "E4-025";
    /// A malformed action.
    pub const ACTION: &str = "E4-026";
    /// An intent that is not in the catalogue.
    pub const UNKNOWN_INTENT: &str = "E4-027";
    /// Intent arguments that do not fit the catalogue.
    pub const INTENT_ARGS: &str = "E4-028";
    /// A value of the wrong type for its target.
    pub const TYPE: &str = "E4-029";
    /// A path that markup may not write.
    pub const NOT_WRITABLE: &str = "E4-030";
    /// An intent of a mod where it is not allowed.
    pub const MOD_INTENT: &str = "E4-031";
    /// A malformed `data-bind-model`.
    pub const MODEL: &str = "E4-032";
    /// A malformed `data-control`.
    pub const CONTROL: &str = "E4-033";
    /// `data-event` next to `data-on-click`.
    pub const EVENT_CLICK: &str = "E4-034";
    /// A binding inside a script island.
    pub const ISLAND_BIND: &str = "E4-035";
    /// Two sources of text on one node, or text over child elements.
    pub const TEXT_SOURCE: &str = "E4-036";
    /// `data-bind-if` and `data-bind-for` on one node.
    pub const IF_FOR: &str = "E4-037";
    /// A malformed game asset reference.
    pub const ASSET: &str = "E5-010";
    /// A malformed localization key.
    pub const L10N_KEY: &str = "E6-010";
    /// A malformed slot or part name.
    pub const SLOT: &str = "E7-010";
    /// A malformed layer or screen.
    pub const LAYER: &str = "E7-011";
    /// A malformed or misplaced anchor.
    pub const ANCHOR: &str = "E7-012";
    /// A malformed hotkey.
    pub const HOTKEY: &str = "E7-013";
    /// A hotkey inside a loop.
    pub const HOTKEY_FOR: &str = "E7-014";
    /// A slot on a node with `if` or `for`.
    pub const SLOT_NODE: &str = "E7-015";
    /// A malformed tooltip reference.
    pub const TOOLTIP: &str = "E7-016";
    /// A malformed `data-fit`.
    pub const FIT: &str = "E7-017";
    /// A malformed script island.
    pub const ISLAND: &str = "E7-018";

    /// Every code of this module.
    pub const ALL: &[&str] = &[
        TITLE,
        ON_ATTR,
        UNKNOWN_DATA,
        SRCSET,
        OUTSIDE,
        NOT_ON_ELEMENT,
        REPEATED,
        INCLUDE,
        VALUE,
        ONE_EXPR,
        FOR,
        NO_KEY,
        NEEDS,
        CLASS_TOGGLE,
        STYLE_PROP,
        ACTION,
        UNKNOWN_INTENT,
        INTENT_ARGS,
        TYPE,
        NOT_WRITABLE,
        MOD_INTENT,
        MODEL,
        CONTROL,
        EVENT_CLICK,
        ISLAND_BIND,
        TEXT_SOURCE,
        IF_FOR,
        ASSET,
        L10N_KEY,
        SLOT,
        LAYER,
        ANCHOR,
        HOTKEY,
        HOTKEY_FOR,
        SLOT_NODE,
        TOOLTIP,
        FIT,
        ISLAND,
    ];
}

/// A problem with an attribute of one element.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {attr}: {message}")]
pub struct ProfileError {
    /// The code (`E1-...`, `E4-...`, ...).
    pub code: &'static str,
    /// The attribute, empty when the problem is not about one attribute.
    pub attr: String,
    /// What was wrong with this attribute.
    pub message: String,
    /// The byte offset in the attribute value, when the problem has a place.
    pub at: Option<usize>,
}

impl ProfileError {
    pub(crate) fn new(code: &'static str, attr: &str, message: impl Into<String>) -> Self {
        Self {
            code,
            attr: attr.to_string(),
            message: message.into(),
            at: None,
        }
    }

    pub(crate) fn from_expr(attr: &str, e: ExprError, base: usize) -> Self {
        let e = e.shifted(base);
        Self {
            code: e.code,
            attr: attr.to_string(),
            message: e.message,
            at: e.at,
        }
    }
}

/// The attributes of one element as the DOM gives them: lower-case names, values as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawAttrs {
    /// The element.
    pub tag: Tag,
    /// The attributes in document order.
    pub attrs: Vec<(String, String)>,
}

impl RawAttrs {
    /// An element with attributes.
    pub fn new(tag: Tag, attrs: &[(&str, &str)]) -> Self {
        Self {
            tag,
            attrs: attrs
                .iter()
                .map(|(n, v)| ((*n).to_string(), (*v).to_string()))
                .collect(),
        }
    }
}

/// Where the element sits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AttrCtx {
    /// The id of the mod whose fragment this is; `None` in a skin.
    pub mod_id: Option<String>,
    /// An ancestor is a loop body.
    pub in_for: bool,
    /// The layer the element is in, when known.
    pub layer: Option<Layer>,
    /// The element has child elements.
    pub has_children: bool,
    /// An ancestor is a script island.
    pub in_island: bool,
}

/// `data-bind-for`.
#[derive(Debug, Clone, PartialEq)]
pub struct ForSpec {
    /// The item variable.
    pub item: String,
    /// The index variable.
    pub index: Option<String>,
    /// The list.
    pub list: Expr,
    /// The key of an item: written, or derived by [`check`] from the schema's list key.
    pub key: Option<Expr>,
    /// `data-virtual`.
    pub virtual_list: bool,
    /// `data-bind-sort`.
    pub sort: Option<Expr>,
    /// `data-bind-sort-desc`.
    pub sort_desc: Option<Expr>,
    /// `data-bind-filter`.
    pub filter: Option<Expr>,
}

/// One entry of `data-bind-class-toggle`.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassToggle {
    /// The class.
    pub class: String,
    /// When it is on.
    pub cond: Expr,
}

/// `data-bind-style-<prop>`.
#[derive(Debug, Clone, PartialEq)]
pub struct StyleBind {
    /// The property.
    pub prop: PropId,
    /// The value, a template.
    pub tpl: Template,
}

/// `data-on-<event>`.
#[derive(Debug, Clone, PartialEq)]
pub struct EventBind {
    /// The event.
    pub ev: Event,
    /// The steps, in order.
    pub actions: Vec<Action>,
}

/// `data-control` and what goes with it.
#[derive(Debug, Clone, PartialEq)]
pub struct Control {
    /// The behaviour.
    pub kind: ControlKind,
    /// `data-value` of a radio.
    pub value: Option<String>,
    /// `data-min`, `data-max`, `data-step` of a slider.
    pub range: Option<(f64, f64, f64)>,
    /// `data-options` of a select.
    pub options: Option<Expr>,
}

/// `data-hotkey`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hotkey {
    /// Ctrl is held.
    pub ctrl: bool,
    /// Alt is held.
    pub alt: bool,
    /// Shift is held.
    pub shift: bool,
    /// The `KeyboardEvent.code`.
    pub code: String,
}

impl Hotkey {
    /// The modifier bits the package stores: ctrl 1, alt 2, shift 4.
    pub fn mods(&self) -> u8 {
        u8::from(self.ctrl) | u8::from(self.alt) << 1 | u8::from(self.shift) << 2
    }
}

/// `data-ui2-island` and its classes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Island {
    /// The island id.
    pub id: String,
    /// The classes the script may switch (`data-ui2-classes`).
    pub classes: Vec<String>,
}

/// The attributes of one element, typed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NodeAttrs {
    /// Plain HTML attributes (`id`, `class`, `style`, ...), in order, for the converter.
    pub plain: Vec<(String, String)>,
    /// `data-bind-value`
    pub value: Option<Template>,
    /// `data-bind-if`
    pub cond: Option<Expr>,
    /// `data-bind-for` with its companions
    pub for_: Option<ForSpec>,
    /// `data-bind-class-toggle`
    pub class_toggle: Vec<ClassToggle>,
    /// `data-bind-style-*`
    pub style: Vec<StyleBind>,
    /// `data-bind-src`
    pub src: Option<Expr>,
    /// `data-bind-disabled`
    pub disabled: Option<Expr>,
    /// `data-bind-model`, the path below `state.`
    pub model: Option<String>,
    /// `data-l10n`
    pub l10n: Option<String>,
    /// `data-l10n-args`
    pub l10n_args: Vec<Expr>,
    /// `data-l10n-placeholder`
    pub l10n_placeholder: Option<String>,
    /// `data-include`
    pub include: Option<String>,
    /// `data-asset`
    pub asset: Option<String>,
    /// `data-on-*` and `data-event`
    pub events: Vec<EventBind>,
    /// `data-control`
    pub control: Option<Control>,
    /// `data-layer`
    pub layer: Option<Layer>,
    /// `data-screen`
    pub screen: Option<String>,
    /// `data-slot`
    pub slot: Option<String>,
    /// `data-part`
    pub part: Option<String>,
    /// `data-anchor`
    pub anchor: Option<(AnchorKind, Expr)>,
    /// `data-hotkey`
    pub hotkey: Option<Hotkey>,
    /// `data-hotkey-label`
    pub hotkey_label: bool,
    /// `data-tooltip`
    pub tooltip: Option<String>,
    /// `data-fit`, with `data-fit-min` in px
    pub fit: Option<Option<f32>>,
    /// `data-ui2-island`
    pub island: Option<Island>,
    /// `data-ui2-key`
    pub island_key: Option<String>,
    /// `data-ui2-dev`: the element exists only for the preview
    pub dev: bool,
}

pub(crate) fn is_ident(s: &str) -> bool {
    let mut chars = s.bytes();
    chars
        .next()
        .is_some_and(|c| c == b'_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == b'_' || c.is_ascii_alphanumeric())
}

fn is_class(s: &str) -> bool {
    let mut chars = s.bytes();
    chars
        .next()
        .is_some_and(|c| c == b'_' || c == b'-' || c.is_ascii_alphabetic())
        && chars.all(|c| c == b'_' || c == b'-' || c.is_ascii_alphanumeric())
}

fn is_html_id(s: &str) -> bool {
    let mut chars = s.bytes();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c == b'_' || c == b'-' || c.is_ascii_alphanumeric())
}

/// `[a-z][a-z0-9-]*(\.[a-z0-9-]+)*`
pub(crate) fn is_slot_name(s: &str) -> bool {
    let mut parts = s.split('.');
    let head = parts.next().unwrap_or("");
    let lower = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-';
    head.bytes().next().is_some_and(|c| c.is_ascii_lowercase())
        && head.bytes().all(lower)
        && parts.all(|p| !p.is_empty() && p.bytes().all(lower))
}

/// `[a-z][a-z0-9_.-]*`: a key of the skin's text tables.
fn is_l10n_key(s: &str) -> bool {
    s.bytes().next().is_some_and(|c| c.is_ascii_lowercase())
        && s.bytes().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'.' | b'-')
        })
        && !s.ends_with('.')
        && !s.contains("..")
}

/// A `KeyboardEvent.code` value.
fn is_key_code(c: &str) -> bool {
    const NAMED: &[&str] = &[
        "Space",
        "Enter",
        "Escape",
        "Tab",
        "Backspace",
        "Delete",
        "Insert",
        "Home",
        "End",
        "PageUp",
        "PageDown",
        "ArrowUp",
        "ArrowDown",
        "ArrowLeft",
        "ArrowRight",
        "Minus",
        "Equal",
        "Comma",
        "Period",
        "Slash",
        "Backslash",
        "Semicolon",
        "Quote",
        "Backquote",
        "BracketLeft",
        "BracketRight",
        "NumpadAdd",
        "NumpadSubtract",
        "NumpadMultiply",
        "NumpadDivide",
        "NumpadEnter",
        "NumpadDecimal",
    ];
    if NAMED.contains(&c) {
        return true;
    }
    let one = |rest: &str, ok: fn(u8) -> bool| rest.len() == 1 && ok(rest.as_bytes()[0]);
    if let Some(rest) = c.strip_prefix("Key") {
        return one(rest, |b| b.is_ascii_uppercase());
    }
    if let Some(rest) = c.strip_prefix("Digit").or_else(|| c.strip_prefix("Numpad")) {
        return one(rest, |b| b.is_ascii_digit());
    }
    if let Some(n) = c.strip_prefix('F') {
        return !n.starts_with('0')
            && n.parse::<u8>().is_ok_and(|n| (1..=24).contains(&n))
            && n.bytes().all(|b| b.is_ascii_digit());
    }
    false
}

/// Splits `src` at `sep` outside string literals, parentheses and `{{ }}` inserts. Returns each
/// piece with its byte offset.
pub(crate) fn split_top(src: &str, sep: char) -> Vec<(usize, &str)> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let (mut start, mut depth, mut in_str) = (0usize, 0usize, false);
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            match c {
                b'\\' => i += 1,
                b'\'' => in_str = false,
                _ => {}
            }
        } else if src[i..].starts_with("{{") {
            if let Some(close) = expr::find_close(src, i + 2) {
                i = close + 1;
            }
        } else if c == b'\'' {
            in_str = true;
        } else if c == b'(' {
            depth += 1;
        } else if c == b')' {
            depth = depth.saturating_sub(1);
        } else if c as char == sep && depth == 0 {
            out.push((start, &src[start..i]));
            start = i + 1;
        }
        i += 1;
    }
    out.push((start, &src[start.min(src.len())..]));
    out
}

/// Exactly one `{{ expr }}` (spaces around it are fine).
pub(crate) fn single_expr(attr: &str, value: &str) -> Result<Expr, ProfileError> {
    let lead = value.len() - value.trim_start().len();
    let tpl =
        expr::parse_template(value.trim()).map_err(|e| ProfileError::from_expr(attr, e, lead))?;
    let mut segs = tpl.segs;
    if segs.len() == 1
        && let Some(TplSeg::Expr(e)) = segs.pop()
    {
        return Ok(e);
    }
    Err(ProfileError::new(
        codes::ONE_EXPR,
        attr,
        "the value must be exactly one {{ }} insert",
    ))
}

fn template(attr: &str, value: &str) -> Result<Template, ProfileError> {
    expr::parse_template(value).map_err(|e| ProfileError::from_expr(attr, e, 0))
}

const TEXTLESS: &[Tag] = &[Tag::Img, Tag::Svg, Tag::Input, Tag::Hr];

struct Parser<'a> {
    tag: Tag,
    ctx: &'a AttrCtx,
    out: NodeAttrs,
    errs: Vec<ProfileError>,
    for_head: Option<(String, Option<String>, Expr)>,
    for_virtual: bool,
    for_key: Option<Expr>,
    for_sort: Option<Expr>,
    for_sort_desc: Option<Expr>,
    for_filter: Option<Expr>,
    fit_min: Option<f32>,
    event_name: Option<String>,
    event_arg: Option<Expr>,
    control: Option<ControlKind>,
    control_value: Option<String>,
    control_min: Option<f64>,
    control_max: Option<f64>,
    control_step: Option<f64>,
    control_options: Option<Expr>,
    island_classes: Option<Vec<String>>,
}

/// Checks the grammar of one element's attributes and types them.
pub fn parse(raw: &RawAttrs, ctx: &AttrCtx) -> Result<NodeAttrs, Vec<ProfileError>> {
    let mut p = Parser {
        tag: raw.tag,
        ctx,
        out: NodeAttrs::default(),
        errs: Vec::new(),
        for_head: None,
        for_virtual: false,
        for_key: None,
        for_sort: None,
        for_sort_desc: None,
        for_filter: None,
        fit_min: None,
        event_name: None,
        event_arg: None,
        control: None,
        control_value: None,
        control_min: None,
        control_max: None,
        control_step: None,
        control_options: None,
        island_classes: None,
    };
    let mut seen = BTreeSet::new();
    for (name, value) in &raw.attrs {
        if !seen.insert(name.as_str()) {
            p.fail(codes::REPEATED, name, "the attribute is written twice");
            continue;
        }
        p.attr(name, value);
    }
    p.finish();
    if p.errs.is_empty() {
        Ok(p.out)
    } else {
        Err(p.errs)
    }
}

impl Parser<'_> {
    fn fail(&mut self, code: &'static str, attr: &str, message: impl Into<String>) {
        self.errs.push(ProfileError::new(code, attr, message));
    }

    fn take<T>(&mut self, r: Result<T, ProfileError>) -> Option<T> {
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                self.errs.push(e);
                None
            }
        }
    }

    fn only_on(&mut self, name: &str, tags: &[Tag]) -> bool {
        if tags.contains(&self.tag) {
            true
        } else {
            self.fail(
                codes::NOT_ON_ELEMENT,
                name,
                format!("`{}` elements cannot have this attribute", self.tag.name()),
            );
            false
        }
    }

    /// A binding of text: not on elements without text, not over child elements.
    fn text_target(&mut self, name: &str) -> bool {
        if TEXTLESS.contains(&self.tag) {
            self.fail(
                codes::NOT_ON_ELEMENT,
                name,
                format!("`{}` elements have no text", self.tag.name()),
            );
            return false;
        }
        true
    }

    fn bind_site(&mut self, name: &str) {
        if self.ctx.in_island {
            self.fail(
                codes::ISLAND_BIND,
                name,
                "an island is driven by script only; it cannot hold bindings",
            );
        }
    }

    fn attr(&mut self, name: &str, value: &str) {
        if name.starts_with("data-bind-") || name.starts_with("data-l10n") {
            self.bind_site(name);
        }
        match name {
            "data-bind-value" => {
                if self.text_target(name)
                    && let Some(t) = self.take(template(name, value))
                {
                    self.out.value = Some(t);
                }
            }
            "data-bind-if" => self.out.cond = self.take(single_expr(name, value)),
            "data-bind-for" => self.for_attr(name, value),
            "data-virtual" => {
                if matches!(value, "" | "true" | "data-virtual") {
                    self.for_virtual = true;
                } else {
                    self.fail(
                        codes::VALUE,
                        name,
                        "a boolean attribute: leave the value empty",
                    );
                }
            }
            "data-bind-key" => self.for_key = self.take(single_expr(name, value)),
            "data-bind-sort" => self.for_sort = self.take(single_expr(name, value)),
            "data-bind-sort-desc" => self.for_sort_desc = self.take(single_expr(name, value)),
            "data-bind-filter" => self.for_filter = self.take(single_expr(name, value)),
            "data-bind-class-toggle" => self.class_toggle(name, value),
            "data-bind-src" => {
                if self.only_on(name, &[Tag::Img]) {
                    self.out.src = self.take(single_expr(name, value));
                }
            }
            "data-bind-disabled" => self.out.disabled = self.take(single_expr(name, value)),
            "data-bind-model" => self.model(name, value),
            "data-l10n" => self.l10n(name, value),
            "data-l10n-args" => self.l10n_args(name, value),
            "data-l10n-placeholder" => {
                if self.only_on(name, &[Tag::Input]) {
                    self.out.l10n_placeholder = self.key(name, value);
                }
            }
            "data-include" => self.include(name, value),
            "data-asset" => {
                if self.only_on(name, &[Tag::Img]) {
                    self.asset(name, value);
                }
            }
            "data-event" => {
                if is_ident(value) {
                    self.event_name = Some(value.to_string());
                } else {
                    self.fail(codes::ACTION, name, "the event name must be an identifier");
                }
            }
            "data-event-arg" => self.event_arg = self.take(single_expr(name, value)),
            "data-control" => match ControlKind::from_name(value) {
                Some(kind) => self.control = Some(kind),
                None => self.fail(
                    codes::CONTROL,
                    name,
                    "the control is one of checkbox, toggle, radio, slider, select",
                ),
            },
            "data-value" => self.control_value = Some(value.to_string()),
            "data-min" => self.control_min = self.number(name, value),
            "data-max" => self.control_max = self.number(name, value),
            "data-step" => self.control_step = self.number(name, value),
            "data-options" => self.control_options = self.take(single_expr(name, value)),
            "data-layer" => match value {
                "map" => self.out.layer = Some(Layer::Map),
                "hud" => self.out.layer = Some(Layer::Hud),
                "window" => self.out.layer = Some(Layer::Window),
                "tooltip" => self.out.layer = Some(Layer::Tooltip),
                _ => self.fail(
                    codes::LAYER,
                    name,
                    "the layer is map, hud, window or tooltip",
                ),
            },
            "data-screen" => {
                if is_slot_name(value) {
                    self.out.screen = Some(value.to_string());
                } else {
                    self.fail(codes::LAYER, name, "a screen id looks like `production`");
                }
            }
            "data-slot" | "data-part" => {
                if is_slot_name(value) {
                    let id = Some(value.to_string());
                    if name == "data-slot" {
                        self.out.slot = id;
                    } else {
                        self.out.part = id;
                    }
                } else {
                    self.fail(codes::SLOT, name, "a name looks like `top.right` or `menu`");
                }
            }
            "data-anchor" => self.anchor(name, value),
            "data-hotkey" => self.hotkey(name, value),
            "data-hotkey-label" => {
                if value.is_empty() {
                    self.out.hotkey_label = true;
                } else {
                    self.fail(codes::HOTKEY, name, "the attribute takes no value");
                }
            }
            "data-tooltip" => {
                if is_html_id(value) {
                    self.out.tooltip = Some(value.to_string());
                } else {
                    self.fail(
                        codes::TOOLTIP,
                        name,
                        "the value is the id of a tooltip template",
                    );
                }
            }
            "data-fit" => {
                if value == "shrink" {
                    self.out.fit = Some(None);
                } else {
                    self.fail(codes::FIT, name, "the only fit is `shrink`");
                }
            }
            "data-fit-min" => {
                let px = value.strip_suffix("px").unwrap_or(value);
                match px.parse::<f32>() {
                    Ok(v) if v.is_finite() && v > 0.0 => self.fit_min = Some(v),
                    _ => self.fail(codes::FIT, name, "the minimum is a size in px"),
                }
            }
            "data-ui2-island" => {
                if is_slot_name(value) {
                    self.out.island = Some(Island {
                        id: value.to_string(),
                        classes: Vec::new(),
                    });
                } else {
                    self.fail(codes::ISLAND, name, "an island id looks like `clock`");
                }
            }
            "data-ui2-key" => {
                if is_class(value) {
                    self.out.island_key = Some(value.to_string());
                } else {
                    self.fail(codes::ISLAND, name, "a key is a short word");
                }
            }
            "data-ui2-classes" => {
                let classes: Vec<String> = value
                    .split([' ', ',', '\t', '\n'])
                    .filter(|c| !c.is_empty())
                    .map(str::to_string)
                    .collect();
                if classes.iter().all(|c| is_class(c)) {
                    self.island_classes = Some(classes);
                } else {
                    self.fail(codes::ISLAND, name, "the list holds class names");
                }
            }
            "data-ui2-dev" => self.out.dev = true,
            _ => self.other(name, value),
        }
    }

    fn other(&mut self, name: &str, value: &str) {
        if let Some(rest) = name.strip_prefix("data-bind-style-") {
            self.style(name, rest, value);
        } else if let Some(rest) = name.strip_prefix("data-on-") {
            self.event(name, rest, value);
        } else if name.starts_with("data-") {
            self.fail(
                codes::UNKNOWN_DATA,
                name,
                "the profile has no such data attribute; check the spelling",
            );
        } else if name == "title" {
            self.fail(codes::TITLE, name, "use data-tooltip");
        } else if name == "srcset" {
            self.fail(
                codes::SRCSET,
                name,
                "the 2x picture is found next to the 1x file",
            );
        } else if name.len() > 2
            && name.starts_with("on")
            && name.bytes().all(|c| c.is_ascii_lowercase())
        {
            self.fail(codes::ON_ATTR, name, "use data-on-<event>");
        } else {
            self.plain(name, value);
        }
    }

    fn plain(&mut self, name: &str, value: &str) {
        let tag = self.tag;
        let ok = match name {
            "id" | "class" | "style" | "hidden" | "tabindex" | "role" | "lang" => true,
            n if n.starts_with("aria-") => true,
            "disabled" => {
                let ok = self.only_on(name, &[Tag::Button, Tag::Input]);
                // A static `disabled` is a binding that is always true (a `data-bind-disabled` wins).
                if ok && self.out.disabled.is_none() {
                    self.out.disabled = Some(Expr::Bool(true));
                }
                ok
            }
            "type" => {
                let limits = Profile::v1().element(tag).map(|d| d.types.clone());
                match limits {
                    Some(types) if !types.is_empty() => {
                        if !types.iter().any(|t| t == value) {
                            self.fail(
                                codes::VALUE,
                                name,
                                format!("`{}` takes the type {}", tag.name(), types.join(", ")),
                            );
                            return;
                        }
                        true
                    }
                    _ => self.only_on(name, &[Tag::Button, Tag::Input, Tag::Script, Tag::Link]),
                }
            }
            "src" => self.only_on(name, &[Tag::Img, Tag::Script]),
            "width" | "height" => self.only_on(name, &[Tag::Img, Tag::Svg]),
            "rel" | "href" | "media" => self.only_on(name, &[Tag::Link]),
            "charset" | "content" | "name" | "http-equiv" => self.only_on(name, &[Tag::Meta]),
            _ => {
                self.fail(
                    codes::OUTSIDE,
                    name,
                    "the attribute is not part of the profile",
                );
                return;
            }
        };
        if ok {
            self.out.plain.push((name.to_string(), value.to_string()));
        }
    }

    fn number(&mut self, name: &str, value: &str) -> Option<f64> {
        match value.trim().parse::<f64>() {
            Ok(v) if v.is_finite() => Some(v),
            _ => {
                self.fail(codes::CONTROL, name, "the value is a finite number");
                None
            }
        }
    }

    fn key(&mut self, name: &str, value: &str) -> Option<String> {
        if is_l10n_key(value) {
            Some(value.to_string())
        } else {
            self.fail(
                codes::L10N_KEY,
                name,
                "a key looks like `top.turn` (lower case, digits, `_`, `-`, `.`)",
            );
            None
        }
    }

    fn l10n(&mut self, name: &str, value: &str) {
        if self.text_target(name) {
            self.out.l10n = self.key(name, value);
        }
    }

    fn l10n_args(&mut self, name: &str, value: &str) {
        let mut args = Vec::new();
        let mut rest = value;
        let mut base = 0;
        loop {
            let lead = rest.len() - rest.trim_start().len();
            let (at, body) = (base + lead, &rest[lead..]);
            if !body.starts_with("{{") {
                self.fail(
                    codes::ONE_EXPR,
                    name,
                    "the value is a list of {{ }} inserts",
                );
                return;
            }
            let Some(close) = expr::find_close(body, 2) else {
                self.fail(codes::ONE_EXPR, name, "an insert is not closed");
                return;
            };
            for (off, part) in split_top(&body[2..close], ',') {
                match expr::parse(part) {
                    Ok(e) => args.push(e),
                    Err(e) => {
                        self.errs
                            .push(ProfileError::from_expr(name, e, at + 2 + off));
                        return;
                    }
                }
            }
            let after = &body[close + 2..];
            let tail = after.trim_start();
            if tail.is_empty() {
                break;
            }
            let Some(next) = tail.strip_prefix(',') else {
                self.fail(codes::ONE_EXPR, name, "the inserts are separated by commas");
                return;
            };
            base = value.len() - next.len();
            rest = next;
        }
        self.out.l10n_args = args;
    }

    fn include(&mut self, name: &str, value: &str) {
        let bad = value.is_empty()
            || value.starts_with('/')
            || value.contains('\\')
            || value.contains(':')
            || value.split('/').any(|s| s.is_empty() || s == "..");
        if bad {
            self.fail(
                codes::INCLUDE,
                name,
                "the path is relative, uses `/` and stays inside the skin",
            );
        } else {
            self.out.include = Some(value.to_string());
        }
    }

    fn asset(&mut self, name: &str, value: &str) {
        let ok = value.split_once(':').is_some_and(|(ns, id)| {
            ns.bytes().next().is_some_and(|c| c.is_ascii_lowercase())
                && ns
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
                && !id.is_empty()
                && !id.contains(char::is_whitespace)
        });
        if ok {
            self.out.asset = Some(value.to_string());
        } else {
            self.fail(
                codes::ASSET,
                name,
                "a game asset is written `<namespace>:<id>`",
            );
        }
    }

    fn for_attr(&mut self, name: &str, value: &str) {
        let Some(open) = value.find("{{") else {
            self.fail(codes::FOR, name, "the form is `[index,] item:{{ list }}`");
            return;
        };
        let head = value[..open].trim();
        let Some(names) = head.strip_suffix(':') else {
            self.fail(codes::FOR, name, "the form is `[index,] item:{{ list }}`");
            return;
        };
        let names: Vec<&str> = names.split(',').map(str::trim).collect();
        if names.is_empty() || names.len() > 2 || !names.iter().all(|n| is_ident(n)) {
            self.fail(codes::FOR, name, "one or two variable names, then a colon");
            return;
        }
        let Some(list) = self.take(single_expr(name, &value[open..]).map_err(|mut e| {
            e.at = e.at.map(|a| a + open);
            e
        })) else {
            return;
        };
        let (index, item) = match names.as_slice() {
            [item] => (None, (*item).to_string()),
            [index, item] => (Some((*index).to_string()), (*item).to_string()),
            _ => return,
        };
        self.for_head = Some((item, index, list));
    }

    fn class_toggle(&mut self, name: &str, value: &str) {
        let mut out = Vec::new();
        for (off, part) in split_top(value, ';') {
            if part.trim().is_empty() {
                continue;
            }
            let Some((class, rest)) = part.split_once(':') else {
                self.fail(codes::CLASS_TOGGLE, name, "the form is `class:{{ cond }}`");
                return;
            };
            let class = class.trim();
            if !is_class(class) || out.iter().any(|c: &ClassToggle| c.class == class) {
                self.fail(codes::CLASS_TOGGLE, name, "a class name, once");
                return;
            }
            match single_expr(name, rest) {
                Ok(cond) => out.push(ClassToggle {
                    class: class.to_string(),
                    cond,
                }),
                Err(mut e) => {
                    e.at = e.at.map(|a| a + off + class.len() + 1);
                    self.errs.push(e);
                    return;
                }
            }
        }
        if out.is_empty() {
            self.fail(codes::CLASS_TOGGLE, name, "the list is empty");
        }
        self.out.class_toggle = out;
    }

    fn style(&mut self, name: &str, prop: &str, value: &str) {
        let profile = Profile::v1();
        let Some(id) = profile.prop(prop) else {
            self.fail(
                codes::STYLE_PROP,
                name,
                format!("`{prop}` is not a profile property"),
            );
            return;
        };
        if !profile.def(id).bind {
            self.fail(codes::STYLE_PROP, name, format!("`{prop}` cannot be bound"));
            return;
        }
        let Some(tpl) = self.take(template(name, value)) else {
            return;
        };
        if let Some(text) = tpl.as_static()
            && let Err(e) = parse_value(id, &text)
        {
            self.errs.push(ProfileError::new(e.code, name, e.message));
            return;
        }
        self.out.style.push(StyleBind { prop: id, tpl });
    }

    fn model(&mut self, name: &str, value: &str) {
        let path = value.trim();
        let ok = path
            .strip_prefix("state.")
            .is_some_and(|rest| !rest.is_empty() && rest.split('.').all(is_ident));
        if ok {
            self.out.model = Some(path["state.".len()..].to_string());
        } else {
            self.fail(codes::MODEL, name, "the value is a path `state.<name>`");
        }
    }

    fn event(&mut self, name: &str, ev: &str, value: &str) {
        let Some(event) = Event::from_name(ev) else {
            self.fail(
                codes::UNKNOWN_DATA,
                name,
                format!("`{ev}` is not an event of the profile"),
            );
            return;
        };
        if event == Event::Submit && self.tag != Tag::Input {
            self.fail(codes::NOT_ON_ELEMENT, name, "submit belongs to text inputs");
            return;
        }
        match actions::parse_actions(value) {
            Ok(list) => self.out.events.push(EventBind {
                ev: event,
                actions: list,
            }),
            Err(mut es) => {
                for e in &mut es {
                    e.attr = name.to_string();
                }
                self.errs.extend(es);
            }
        }
    }

    fn anchor(&mut self, name: &str, value: &str) {
        let Some((kind, rest)) = value.split_once(':') else {
            self.fail(codes::ANCHOR, name, "the form is `city:{{ id }}`");
            return;
        };
        let Some(kind) = AnchorKind::from_name(kind.trim()) else {
            self.fail(codes::ANCHOR, name, "the kind is city, unit or plot");
            return;
        };
        let base = kind.name().len() + 1;
        if let Some(e) = self.take(single_expr(name, rest).map_err(|mut e| {
            e.at = e.at.map(|a| a + base);
            e
        })) {
            self.out.anchor = Some((kind, e));
        }
    }

    fn hotkey(&mut self, name: &str, value: &str) {
        let mut parts: Vec<&str> = value.split('+').collect();
        let code = parts.pop().unwrap_or("");
        let order = ["Ctrl", "Alt", "Shift"];
        let mut last = None;
        let mut key = Hotkey {
            ctrl: false,
            alt: false,
            shift: false,
            code: code.to_string(),
        };
        for m in &parts {
            let Some(at) = order.iter().position(|o| o == m) else {
                self.fail(
                    codes::HOTKEY,
                    name,
                    "modifiers are Ctrl, Alt, Shift, in that order",
                );
                return;
            };
            if last.is_some_and(|l| l >= at) {
                self.fail(
                    codes::HOTKEY,
                    name,
                    "modifiers are Ctrl, Alt, Shift, in that order",
                );
                return;
            }
            last = Some(at);
            match at {
                0 => key.ctrl = true,
                1 => key.alt = true,
                _ => key.shift = true,
            }
        }
        if !is_key_code(code) {
            self.fail(
                codes::HOTKEY,
                name,
                format!("`{code}` is not a KeyboardEvent.code such as KeyB or F5"),
            );
            return;
        }
        self.out.hotkey = Some(key);
    }

    /// Rules that need several attributes at once.
    fn finish(&mut self) {
        // Loops.
        let companions = [
            ("data-bind-key", self.for_key.is_some()),
            ("data-bind-sort", self.for_sort.is_some()),
            ("data-bind-sort-desc", self.for_sort_desc.is_some()),
            ("data-bind-filter", self.for_filter.is_some()),
            ("data-virtual", self.for_virtual),
        ];
        match self.for_head.take() {
            Some((item, index, list)) => {
                self.out.for_ = Some(ForSpec {
                    item,
                    index,
                    list,
                    key: self.for_key.take(),
                    virtual_list: self.for_virtual,
                    sort: self.for_sort.take(),
                    sort_desc: self.for_sort_desc.take(),
                    filter: self.for_filter.take(),
                });
                if self.out.cond.is_some() {
                    self.fail(
                        codes::IF_FOR,
                        "data-bind-if",
                        "put the condition on a parent element",
                    );
                }
            }
            None => {
                for (name, present) in companions {
                    if present {
                        self.fail(codes::NEEDS, name, "the attribute goes with data-bind-for");
                    }
                }
            }
        }
        if self.out.l10n.is_none() && !self.out.l10n_args.is_empty() {
            self.fail(
                codes::NEEDS,
                "data-l10n-args",
                "the attribute goes with data-l10n",
            );
        }
        if self.out.value.is_some() && (self.out.l10n.is_some() || self.ctx.has_children) {
            self.fail(
                codes::TEXT_SOURCE,
                "data-bind-value",
                "an element has one source of text and then no child elements",
            );
        }
        if self.out.l10n.is_some() && self.ctx.has_children {
            self.fail(
                codes::TEXT_SOURCE,
                "data-l10n",
                "an element with a text key has no child elements",
            );
        }
        // A hotkey label's text is the name of its key: no other source, no child elements.
        if self.out.hotkey_label
            && (self.out.value.is_some() || self.out.l10n.is_some() || self.ctx.has_children)
        {
            self.fail(
                codes::TEXT_SOURCE,
                "data-hotkey-label",
                "a hotkey label shows the key's name: no other text and no child elements",
            );
        }

        // Events.
        if let Some(name) = self.event_name.take() {
            if self.out.events.iter().any(|e| e.ev == Event::Click) {
                self.fail(
                    codes::EVENT_CLICK,
                    "data-event",
                    "the node already has data-on-click",
                );
            } else {
                let args = self
                    .event_arg
                    .take()
                    .map(|value| vec![super::action::Arg { name: None, value }])
                    .unwrap_or_default();
                self.out.events.push(EventBind {
                    ev: Event::Click,
                    actions: vec![Action::Call { name, args }],
                });
            }
        } else if self.event_arg.is_some() {
            self.fail(
                codes::NEEDS,
                "data-event-arg",
                "the attribute goes with data-event",
            );
        }

        // Controls.
        self.finish_control();

        // Slots, layers, anchors, hotkeys.
        if self.out.slot.is_some() && (self.out.for_.is_some() || self.out.cond.is_some()) {
            self.fail(
                codes::SLOT_NODE,
                "data-slot",
                "a slot cannot sit on a node with if or for",
            );
        }
        if let Some((_, _)) = &self.out.anchor {
            if self.out.for_.is_none() {
                self.fail(
                    codes::ANCHOR,
                    "data-anchor",
                    "the anchor sits on the node with data-bind-for",
                );
            }
            if self.ctx.layer.is_some_and(|l| l != Layer::Map) {
                self.fail(
                    codes::ANCHOR,
                    "data-anchor",
                    "anchors live in the map layer",
                );
            }
        }
        if self.out.screen.is_some() && self.ctx.layer.is_some_and(|l| l != Layer::Window) {
            self.fail(
                codes::LAYER,
                "data-screen",
                "a screen is a child of the window layer",
            );
        }
        if self.out.hotkey.is_some() && (self.ctx.in_for || self.out.for_.is_some()) {
            self.fail(
                codes::HOTKEY_FOR,
                "data-hotkey",
                "a hotkey cannot sit inside a loop body",
            );
        }
        if let Some(min) = self.fit_min {
            match &mut self.out.fit {
                Some(fit) => *fit = Some(min),
                None => self.fail(
                    codes::FIT,
                    "data-fit-min",
                    "the attribute goes with data-fit",
                ),
            }
        }

        // Islands.
        if let Some(classes) = self.island_classes.take() {
            match &mut self.out.island {
                Some(island) => island.classes = classes,
                None => self.fail(
                    codes::ISLAND,
                    "data-ui2-classes",
                    "the attribute goes with data-ui2-island",
                ),
            }
        }
        if self.out.island_key.is_some() && !(self.ctx.in_island || self.out.island.is_some()) {
            self.fail(
                codes::ISLAND,
                "data-ui2-key",
                "a key is used inside an island",
            );
        }
        if self.out.island.is_some() {
            let binds = self.out.value.is_some()
                || self.out.cond.is_some()
                || self.out.for_.is_some()
                || !self.out.class_toggle.is_empty()
                || !self.out.style.is_empty()
                || self.out.src.is_some()
                || self
                    .out
                    .disabled
                    .as_ref()
                    .is_some_and(|d| *d != Expr::Bool(true))
                || self.out.model.is_some()
                || self.out.l10n.is_some();
            if binds {
                self.fail(
                    codes::ISLAND_BIND,
                    "data-ui2-island",
                    "an island holds no bindings",
                );
            }
        }
    }

    fn finish_control(&mut self) {
        let extras = [
            ("data-value", self.control_value.is_some()),
            ("data-min", self.control_min.is_some()),
            ("data-max", self.control_max.is_some()),
            ("data-step", self.control_step.is_some()),
            ("data-options", self.control_options.is_some()),
        ];
        let Some(kind) = self.control.take() else {
            for (name, present) in extras {
                if present {
                    self.fail(codes::CONTROL, name, "the attribute goes with data-control");
                }
            }
            return;
        };
        if self.out.model.is_none() {
            self.fail(
                codes::CONTROL,
                "data-control",
                "a control needs data-bind-model",
            );
        }
        let wants: &[&str] = match kind {
            ControlKind::Checkbox | ControlKind::Toggle => &[],
            ControlKind::Radio => &["data-value"],
            ControlKind::Slider => &["data-min", "data-max", "data-step"],
            ControlKind::Select => &["data-options"],
        };
        for (name, present) in extras {
            let needed = wants.contains(&name);
            if needed && !present {
                self.fail(
                    codes::CONTROL,
                    name,
                    format!("a {} needs this attribute", kind.name()),
                );
            } else if present && !needed {
                self.fail(
                    codes::CONTROL,
                    name,
                    format!("a {} does not use this attribute", kind.name()),
                );
            }
        }
        let range = match (self.control_min, self.control_max, self.control_step) {
            (Some(lo), Some(hi), Some(step)) => {
                if lo >= hi || step <= 0.0 {
                    self.fail(
                        codes::CONTROL,
                        "data-min",
                        "the range needs min < max and step > 0",
                    );
                }
                Some((lo, hi, step))
            }
            _ => None,
        };
        self.out.control = Some(Control {
            kind,
            value: self.control_value.take(),
            range,
            options: self.control_options.take(),
        });
    }
}
