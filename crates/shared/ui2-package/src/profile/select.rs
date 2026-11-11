//! Selectors and `@media` queries of the profile.
//!
//! The runtime never matches a selector: the converter matches the **static part** of every
//! selector (type, `*`, `.class`, `#id`, `:root`, the combinators descendant and `>`) on the
//! document itself and asks Chrome for the computed style of each combination of the **dynamic
//! conditions** (`:hover :active :focus :focus-visible :disabled` and the classes a binding
//! toggles) and of each **viewport cell** (the `@media` breakpoints split the viewport into at most
//! 16 cells). This module is the profile's half of that work:
//!
//! - [`parse`] / [`parse_list`] read selector text into a general [`SelectorAst`] (any CSS
//!   selector, so the error can say what is outside the profile);
//! - [`check`] accepts the profile subset and splits a selector into [`SelInfo`]: the static part
//!   and the dynamic conditions with the compound each belongs to;
//! - [`node_conds`] and [`check_toggled`] hold the per-node limits (at most four conditions, a
//!   toggled class is never also static on the node);
//! - [`media`] parses `@media` preludes and turns the conditions of a skin into viewport cells
//!   (at most 8 conditions and 16 cells).
//!
//! Errors carry the `E3` codes; their texts are in `profile.toml`.

use crate::sections::styles::{Cond, CondKind, MAX_CONDS};

use super::ids::Tag;

/// Error codes of this module (texts in `profile.toml`).
pub mod codes {
    /// A selector outside the profile.
    pub const SELECTOR: &str = "E3-002";
    /// A toggled class is also in the static `class` attribute of the node.
    pub const TOGGLED_STATIC: &str = "E3-003";
    /// A media query outside the profile.
    pub const MEDIA: &str = "E3-004";
    /// More than four conditions on one node.
    pub const CONDS: &str = "E3-005";
    /// More than 8 media conditions or 16 viewport cells.
    pub const CELLS: &str = "E3-006";
    /// Selector text that is not CSS at all.
    pub const SYNTAX: &str = "E3-010";

    /// Every code of this module.
    pub const ALL: &[&str] = &[SELECTOR, TOGGLED_STATIC, MEDIA, CONDS, CELLS, SYNTAX];
}

/// A selector, a node's conditions or a media query outside the profile.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct SelectError {
    /// The code (`E3-...`).
    pub code: &'static str,
    /// What is outside the profile.
    pub message: String,
}

impl SelectError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

fn syntax(message: impl Into<String>) -> SelectError {
    SelectError::new(codes::SYNTAX, message)
}

fn outside(message: impl Into<String>) -> SelectError {
    SelectError::new(codes::SELECTOR, message)
}

// ---------------------------------------------------------------------------------------------
// The general selector syntax

/// A combinator between two compounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combinator {
    /// Whitespace: any ancestor.
    Descendant,
    /// `>`: the parent.
    Child,
    /// `+`: the previous sibling.
    NextSibling,
    /// `~`: any previous sibling.
    SubsequentSibling,
}

/// One simple selector of a compound, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Simple {
    /// `*`.
    Universal,
    /// A type selector, lower case.
    Type(String),
    /// `.name`.
    Class(String),
    /// `#name`.
    Id(String),
    /// `[...]`, the text between the brackets.
    Attribute(String),
    /// `:name` or `:name(arg)`; the name lower case.
    PseudoClass {
        /// The name without the colon.
        name: String,
        /// The text between the parentheses.
        arg: Option<String>,
    },
    /// `::name` or `::name(arg)`; the name lower case.
    PseudoElement {
        /// The name without the colons.
        name: String,
        /// The text between the parentheses.
        arg: Option<String>,
    },
    /// A namespace prefix (`svg|`, `*|`, `|`), the text before the bar.
    Namespace(String),
    /// `&`.
    Nesting,
}

/// One complex selector, left to right. The first compound has no combinator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectorAst {
    /// The compounds, each with the combinator before it.
    pub parts: Vec<(Option<Combinator>, Vec<Simple>)>,
}

/// Parses a selector list (`a, b`) into its selectors.
pub fn parse_list(src: &str) -> Result<Vec<SelectorAst>, SelectError> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut start = 0;
    let mut escaped = false;
    for (i, c) in src.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, c) {
            (_, '\\') => escaped = true,
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(' | '[') => depth += 1,
            (None, ')' | ']') => depth -= 1,
            (None, ',') if depth == 0 => {
                out.push(parse(&src[start..i])?);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(parse(&src[start..])?);
    Ok(out)
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_' || !c.is_ascii()
}

struct Reader {
    chars: Vec<char>,
    i: usize,
}

impl Reader {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.i).copied()
    }

    fn peek_at(&self, k: usize) -> Option<char> {
        self.chars.get(self.i + k).copied()
    }

    /// An identifier with CSS escapes decoded (`\:` is `:`, `\31 ` is `1`).
    fn ident(&mut self) -> Result<String, SelectError> {
        let mut out = String::new();
        loop {
            match self.peek() {
                Some('\\') => {
                    self.i += 1;
                    let Some(c) = self.peek() else {
                        return Err(syntax("an escape at the end"));
                    };
                    if c.is_ascii_hexdigit() {
                        let mut hex = String::new();
                        while hex.len() < 6 && self.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
                            hex.push(self.chars[self.i]);
                            self.i += 1;
                        }
                        if self.peek() == Some(' ') {
                            self.i += 1;
                        }
                        let code = u32::from_str_radix(&hex, 16).unwrap_or(0xFFFD);
                        out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                    } else if c == '\n' {
                        return Err(syntax("an escaped line break"));
                    } else {
                        out.push(c);
                        self.i += 1;
                    }
                }
                Some(c) if is_ident_char(c) => {
                    out.push(c);
                    self.i += 1;
                }
                _ => break,
            }
        }
        Ok(out)
    }

    /// The text up to the bracket that closes the one just consumed, nesting and quotes aware.
    fn balanced(&mut self, open: char, close: char) -> Result<String, SelectError> {
        let start = self.i;
        let mut depth = 1;
        let mut quote: Option<char> = None;
        while let Some(c) = self.peek() {
            self.i += 1;
            match (quote, c) {
                (_, '\\') => self.i += 1,
                (Some(q), c) if c == q => quote = None,
                (Some(_), _) => {}
                (None, '"' | '\'') => quote = Some(c),
                (None, c) if c == open => depth += 1,
                (None, c) if c == close => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(self.chars[start..self.i - 1].iter().collect());
                    }
                }
                _ => {}
            }
        }
        Err(syntax(format!("`{open}` is not closed")))
    }
}

/// Parses one complex selector (no commas). Accepts any selector of CSS Selectors 4 well enough
/// to name the part that is outside the profile; [`check`] decides what the profile allows.
pub fn parse(src: &str) -> Result<SelectorAst, SelectError> {
    let mut r = Reader {
        chars: src.trim().chars().collect(),
        i: 0,
    };
    if r.chars.is_empty() {
        return Err(syntax("an empty selector"));
    }
    let mut parts: Vec<(Option<Combinator>, Vec<Simple>)> = Vec::new();
    let mut cur: Vec<Simple> = Vec::new();
    let mut pending: Option<Combinator> = None;
    let flush = |parts: &mut Vec<(Option<Combinator>, Vec<Simple>)>,
                 cur: &mut Vec<Simple>,
                 pending: &mut Option<Combinator>|
     -> Result<(), SelectError> {
        if cur.is_empty() {
            return Err(syntax("a combinator without a compound"));
        }
        let comb = if parts.is_empty() {
            None
        } else {
            Some(pending.take().unwrap_or(Combinator::Descendant))
        };
        parts.push((comb, std::mem::take(cur)));
        Ok(())
    };
    while let Some(c) = r.peek() {
        match c {
            ' ' | '\t' | '\n' | '\r' | '\x0c' | '>' | '+' | '~' => {
                let mut comb: Option<Combinator> = None;
                while let Some(c) = r.peek() {
                    let next = match c {
                        ' ' | '\t' | '\n' | '\r' | '\x0c' => None,
                        '>' => Some(Combinator::Child),
                        '+' => Some(Combinator::NextSibling),
                        '~' => Some(Combinator::SubsequentSibling),
                        _ => break,
                    };
                    if let Some(n) = next {
                        if comb.is_some() {
                            return Err(syntax("two combinators in a row"));
                        }
                        comb = Some(n);
                    }
                    r.i += 1;
                }
                if r.peek().is_none() {
                    if comb.is_some() {
                        return Err(syntax("a combinator at the end"));
                    }
                    break;
                }
                // A leading combinator (a relative selector) leaves `cur` empty: an error.
                flush(&mut parts, &mut cur, &mut pending)?;
                pending = Some(comb.unwrap_or(Combinator::Descendant));
            }
            '*' => {
                r.i += 1;
                if r.peek() == Some('|') && r.peek_at(1) != Some('=') {
                    r.i += 1;
                    cur.push(Simple::Namespace("*".into()));
                } else {
                    cur.push(Simple::Universal);
                }
            }
            '|' => {
                r.i += 1;
                cur.push(Simple::Namespace(String::new()));
            }
            '&' => {
                r.i += 1;
                cur.push(Simple::Nesting);
            }
            '.' | '#' => {
                r.i += 1;
                let name = r.ident()?;
                if name.is_empty() {
                    return Err(syntax(format!("`{c}` without a name")));
                }
                cur.push(if c == '.' {
                    Simple::Class(name)
                } else {
                    Simple::Id(name)
                });
            }
            '[' => {
                r.i += 1;
                let inner = r.balanced('[', ']')?;
                cur.push(Simple::Attribute(inner.trim().to_string()));
            }
            ':' => {
                let element = r.peek_at(1) == Some(':');
                r.i += if element { 2 } else { 1 };
                let name = r.ident()?.to_ascii_lowercase();
                if name.is_empty() {
                    return Err(syntax("`:` without a name"));
                }
                let arg = if r.peek() == Some('(') {
                    r.i += 1;
                    Some(r.balanced('(', ')')?.trim().to_string())
                } else {
                    None
                };
                cur.push(if element {
                    Simple::PseudoElement { name, arg }
                } else {
                    Simple::PseudoClass { name, arg }
                });
            }
            c if is_ident_char(c) || c == '\\' => {
                let name = r.ident()?.to_ascii_lowercase();
                if r.peek() == Some('|') && r.peek_at(1) != Some('=') {
                    r.i += 1;
                    cur.push(Simple::Namespace(name));
                } else {
                    if !cur.is_empty() && !matches!(cur.last(), Some(Simple::Namespace(_))) {
                        return Err(syntax(format!(
                            "the type selector `{name}` after another part"
                        )));
                    }
                    cur.push(Simple::Type(name));
                }
            }
            other => return Err(syntax(format!("`{other}`"))),
        }
    }
    flush(&mut parts, &mut cur, &mut pending)?;
    Ok(SelectorAst { parts })
}

// ---------------------------------------------------------------------------------------------
// The profile subset

/// A combinator of the profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comb {
    /// Whitespace: any ancestor.
    Descendant,
    /// `>`: the parent.
    Child,
}

/// A dynamic pseudo-class: one condition of a style variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Pseudo {
    /// `:hover`.
    Hover,
    /// `:active`.
    Active,
    /// `:focus`.
    Focus,
    /// `:focus-visible`.
    FocusVisible,
    /// `:disabled`.
    Disabled,
}

impl Pseudo {
    /// Every dynamic pseudo-class.
    pub const ALL: &'static [Pseudo] = &[
        Pseudo::Hover,
        Pseudo::Active,
        Pseudo::Focus,
        Pseudo::FocusVisible,
        Pseudo::Disabled,
    ];

    /// The name without the colon.
    pub const fn name(self) -> &'static str {
        match self {
            Pseudo::Hover => "hover",
            Pseudo::Active => "active",
            Pseudo::Focus => "focus",
            Pseudo::FocusVisible => "focus-visible",
            Pseudo::Disabled => "disabled",
        }
    }

    /// The pseudo-class of a name (lower case, without the colon).
    pub fn from_name(name: &str) -> Option<Pseudo> {
        Pseudo::ALL.iter().copied().find(|p| p.name() == name)
    }

    /// The condition kind a style set stores for it.
    pub const fn cond_kind(self) -> CondKind {
        match self {
            Pseudo::Hover => CondKind::Hover,
            Pseudo::Active => CondKind::Active,
            Pseudo::Focus => CondKind::Focus,
            Pseudo::FocusVisible => CondKind::FocusVisible,
            Pseudo::Disabled => CondKind::Disabled,
        }
    }
}

/// The static part of one compound: what the converter matches on the document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StaticCompound {
    /// The element, `None` for `*` or no type selector.
    pub tag: Option<Tag>,
    /// `#id`s.
    pub ids: Vec<String>,
    /// `.class`es. A class that a binding toggles is a condition of the node, not a static
    /// part: the converter tells them apart ([`SelInfo::dynamic_classes`]).
    pub classes: Vec<String>,
    /// `:root`.
    pub root: bool,
}

/// One dynamic pseudo-class of a selector and the compound it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelCond {
    /// The index of the compound in [`SelInfo::static_part`] (0 is the leftmost).
    pub part: usize,
    /// The pseudo-class.
    pub pseudo: Pseudo,
}

/// A selector of the profile, split into what is matched once and what varies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelInfo {
    /// The compounds left to right, each with the combinator before it.
    pub static_part: Vec<(Option<Comb>, StaticCompound)>,
    /// The dynamic pseudo-classes, left to right.
    pub conds: Vec<SelCond>,
    /// `input::placeholder`: the rule styles the placeholder text (only `color` counts).
    pub placeholder: bool,
}

impl SelInfo {
    /// The subject (rightmost) compound.
    pub fn subject(&self) -> &StaticCompound {
        // `check` never returns an empty selector.
        &self.static_part[self.static_part.len() - 1].1
    }

    /// The classes of the selector that `toggled` says a binding switches, with their compound.
    pub fn dynamic_classes<'a>(
        &'a self,
        toggled: &'a dyn Fn(&str) -> bool,
    ) -> impl Iterator<Item = (usize, &'a str)> + 'a {
        self.static_part
            .iter()
            .enumerate()
            .flat_map(move |(i, (_, c))| {
                c.classes
                    .iter()
                    .filter(move |k| toggled(k))
                    .map(move |k| (i, k.as_str()))
            })
    }

    /// Whether the selector gives its nodes conditions: a dynamic pseudo-class or a class some
    /// binding toggles.
    pub fn is_dynamic(&self, toggled: &dyn Fn(&str) -> bool) -> bool {
        !self.conds.is_empty() || self.dynamic_classes(toggled).next().is_some()
    }
}

/// The properties a `::placeholder` rule may set.
pub const PLACEHOLDER_PROPS: &[&str] = &["color"];

/// Checks a selector against the profile and splits it into its static part and conditions.
pub fn check(ast: &SelectorAst) -> Result<SelInfo, SelectError> {
    let mut static_part = Vec::with_capacity(ast.parts.len());
    let mut conds = Vec::new();
    let mut placeholder = false;
    let last = ast.parts.len().saturating_sub(1);
    for (i, (comb, simples)) in ast.parts.iter().enumerate() {
        let comb = match comb {
            None => None,
            Some(Combinator::Descendant) => Some(Comb::Descendant),
            Some(Combinator::Child) => Some(Comb::Child),
            Some(Combinator::NextSibling) => return Err(outside("the sibling combinator `+`")),
            Some(Combinator::SubsequentSibling) => {
                return Err(outside("the sibling combinator `~`"));
            }
        };
        let mut c = StaticCompound::default();
        let mut after_element = false;
        for s in simples {
            if after_element {
                return Err(outside("a selector after `::placeholder`"));
            }
            match s {
                Simple::Universal => {}
                Simple::Type(name) => match Tag::from_name(name) {
                    Some(t) => c.tag = Some(t),
                    None => {
                        return Err(outside(format!(
                            "the element `{name}` is outside the profile"
                        )));
                    }
                },
                Simple::Class(k) => c.classes.push(k.clone()),
                Simple::Id(k) => c.ids.push(k.clone()),
                Simple::Attribute(a) => {
                    return Err(outside(format!("the attribute selector `[{a}]`")));
                }
                Simple::Namespace(_) => return Err(outside("a namespace prefix")),
                Simple::Nesting => return Err(outside("the nesting selector `&`")),
                Simple::PseudoClass { name, arg: Some(_) } => {
                    return Err(outside(format!("the pseudo-class `:{name}()`")));
                }
                Simple::PseudoClass { name, arg: None } => {
                    if name == "root" {
                        c.root = true;
                    } else if let Some(p) = Pseudo::from_name(name) {
                        if !conds.iter().any(|x: &SelCond| x.part == i && x.pseudo == p) {
                            conds.push(SelCond { part: i, pseudo: p });
                        }
                    } else {
                        return Err(outside(format!("the pseudo-class `:{name}`")));
                    }
                }
                Simple::PseudoElement { name, arg } => {
                    if name != "placeholder" || arg.is_some() {
                        return Err(outside(format!("the pseudo-element `::{name}`")));
                    }
                    if i != last || c.tag != Some(Tag::Input) {
                        return Err(outside("`::placeholder` outside `input::placeholder`"));
                    }
                    placeholder = true;
                    after_element = true;
                }
            }
        }
        static_part.push((comb, c));
    }
    if static_part.is_empty() {
        return Err(syntax("an empty selector"));
    }
    if conds.len() > MAX_CONDS {
        return Err(SelectError::new(
            codes::CONDS,
            format!(
                "{} dynamic pseudo-classes in one selector, at most {MAX_CONDS}",
                conds.len()
            ),
        ));
    }
    Ok(SelInfo {
        static_part,
        conds,
        placeholder,
    })
}

/// Parses a selector list and checks every selector of it.
pub fn check_list(src: &str) -> Result<Vec<SelInfo>, SelectError> {
    parse_list(src)?.iter().map(check).collect()
}

/// The conditions of one node, from every rule that reaches it (inherited ones included):
/// duplicates removed, the first occurrence fixing the mask bit. More than four is an error.
pub fn node_conds(conds: impl IntoIterator<Item = Cond>) -> Result<Vec<Cond>, SelectError> {
    let mut out: Vec<Cond> = Vec::new();
    for c in conds {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    if out.len() > MAX_CONDS {
        return Err(SelectError::new(
            codes::CONDS,
            format!("{} conditions on one node, at most {MAX_CONDS}", out.len()),
        ));
    }
    Ok(out)
}

/// The number of style variants of a node with `conds` conditions in one cell: `2^conds`.
pub const fn variants(conds: usize) -> usize {
    1 << conds
}

/// A class that a binding toggles must not also be in the static `class` attribute of the node.
pub fn check_toggled(static_classes: &[&str], toggled: &[&str]) -> Result<(), SelectError> {
    match toggled.iter().find(|t| static_classes.contains(t)) {
        Some(t) => Err(SelectError::new(
            codes::TOGGLED_STATIC,
            format!("the class `{t}` is both static and toggled"),
        )),
        None => Ok(()),
    }
}

// ---------------------------------------------------------------------------------------------
// Media queries and viewport cells

/// `@media` queries: width and height in px, joined by `and`, and the viewport cells they make.
pub mod media {
    use super::{SelectError, codes};
    use crate::sections::styles::{Bound, MAX_CELLS};

    /// Most distinct media conditions (breakpoints) one skin may have.
    pub const MAX_MEDIA_CONDS: usize = 8;

    /// The viewport size of a skin without breakpoints, CSS px.
    pub const DEFAULT_VIEWPORT: [u32; 2] = [1920, 1080];

    /// A viewport axis.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum Axis {
        /// `width`.
        Width,
        /// `height`.
        Height,
    }

    /// A comparison, with the feature on the left (`width >= 1600px`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum Op {
        /// `<`.
        Lt,
        /// `<=` (`max-*`).
        Le,
        /// `>`.
        Gt,
        /// `>=` (`min-*`).
        Ge,
    }

    impl Op {
        fn parse(s: &str) -> Option<Op> {
            Some(match s {
                "<" => Op::Lt,
                "<=" => Op::Le,
                ">" => Op::Gt,
                ">=" => Op::Ge,
                _ => return None,
            })
        }

        /// The same comparison with the sides swapped (`1600px <= width` is `width >= 1600px`).
        pub const fn flip(self) -> Op {
            match self {
                Op::Lt => Op::Gt,
                Op::Le => Op::Ge,
                Op::Gt => Op::Lt,
                Op::Ge => Op::Le,
            }
        }

        const fn upward(self) -> bool {
            matches!(self, Op::Gt | Op::Ge)
        }
    }

    /// One condition of a media query: `axis op px`.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct MediaCond {
        /// The viewport axis.
        pub axis: Axis,
        /// The comparison.
        pub op: Op,
        /// The breakpoint, CSS px.
        pub px: f32,
    }

    impl MediaCond {
        /// The breakpoint as a style table stores it.
        pub fn bound(&self) -> Bound {
            Bound {
                px: self.px,
                // The value `px` itself falls below the breakpoint for `<=` and `>`.
                upper_inclusive: matches!(self.op, Op::Le | Op::Gt),
            }
        }

        /// Whether the condition holds for a logical viewport (CSS px).
        pub fn holds(&self, viewport: [f32; 2]) -> bool {
            let v = match self.axis {
                Axis::Width => viewport[0],
                Axis::Height => viewport[1],
            };
            match self.op {
                Op::Lt => v < self.px,
                Op::Le => v <= self.px,
                Op::Gt => v > self.px,
                Op::Ge => v >= self.px,
            }
        }
    }

    fn bad(message: impl Into<String>) -> SelectError {
        SelectError::new(codes::MEDIA, message)
    }

    fn px(s: &str) -> Result<f32, SelectError> {
        let n = s
            .strip_suffix("px")
            .ok_or_else(|| bad(format!("`{s}`: a media query takes px only")))?;
        match n.parse::<f32>() {
            Ok(v) if v.is_finite() && v > 0.0 => Ok(v),
            _ => Err(bad(format!("`{s}` is not a positive length"))),
        }
    }

    fn axis(name: &str) -> Option<Axis> {
        match name {
            "width" => Some(Axis::Width),
            "height" => Some(Axis::Height),
            _ => None,
        }
    }

    fn feature_name(name: &str) -> SelectError {
        bad(format!("the media feature `{name}`"))
    }

    /// `name:value` or a range, without the parentheses and spaces.
    fn feature(body: &str) -> Result<Vec<MediaCond>, SelectError> {
        if let Some((name, value)) = body.split_once(':') {
            let (axis, op) = match name {
                "min-width" => (Axis::Width, Op::Ge),
                "max-width" => (Axis::Width, Op::Le),
                "min-height" => (Axis::Height, Op::Ge),
                "max-height" => (Axis::Height, Op::Le),
                "width" | "height" => {
                    return Err(bad(format!(
                        "`({name}: ...)` matches one size only: write min-, max- or a range"
                    )));
                }
                other => return Err(feature_name(other)),
            };
            return Ok(vec![MediaCond {
                axis,
                op,
                px: px(value)?,
            }]);
        }
        // Split into operands and operators: "1280px<=width<1600px".
        let mut toks: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut op = String::new();
        for c in body.chars() {
            if matches!(c, '<' | '>' | '=') {
                if !cur.is_empty() {
                    toks.push(std::mem::take(&mut cur));
                }
                op.push(c);
            } else {
                if !op.is_empty() {
                    toks.push(std::mem::take(&mut op));
                }
                cur.push(c);
            }
        }
        if !cur.is_empty() {
            toks.push(cur);
        }
        if !op.is_empty() {
            toks.push(op);
        }
        let op_of = |s: &str| {
            Op::parse(s).ok_or_else(|| bad(format!("the comparison `{s}` (use < <= > >=)")))
        };
        /// The feature and the value of `a op b`, and whether the feature is on the right.
        fn side<'a>(a: &'a str, b: &'a str) -> Result<(Axis, &'a str, bool), SelectError> {
            match (axis(a), axis(b)) {
                (Some(ax), None) => Ok((ax, b, false)),
                (None, Some(ax)) => Ok((ax, a, true)),
                (Some(_), Some(_)) => Err(bad("a range between two features")),
                (None, None) => {
                    let name = if a.ends_with("px") || a.parse::<f32>().is_ok() {
                        b
                    } else {
                        a
                    };
                    Err(feature_name(name))
                }
            }
        }
        match toks.as_slice() {
            [a, o, b] => {
                let (ax, value, flipped) = side(a, b)?;
                let mut op = op_of(o)?;
                if flipped {
                    op = op.flip();
                }
                Ok(vec![MediaCond {
                    axis: ax,
                    op,
                    px: px(value)?,
                }])
            }
            [a, o1, f, o2, b] => {
                let Some(ax) = axis(f) else {
                    return Err(feature_name(f));
                };
                let (o1, o2) = (op_of(o1)?, op_of(o2)?);
                if o1.upward() != o2.upward() {
                    return Err(bad("a range must point one way (`a < width < b`)"));
                }
                Ok(vec![
                    MediaCond {
                        axis: ax,
                        op: o1.flip(),
                        px: px(a)?,
                    },
                    MediaCond {
                        axis: ax,
                        op: o2,
                        px: px(b)?,
                    },
                ])
            }
            _ => Err(bad(format!("`({body})`"))),
        }
    }

    /// Parses a media prelude (the text between `@media` and `{`) into conditions that must all
    /// hold. Types `all`, `screen` (and `only`) are accepted and mean nothing.
    pub fn parse(prelude: &str) -> Result<Vec<MediaCond>, SelectError> {
        let p = prelude.trim().to_ascii_lowercase();
        if p.is_empty() {
            return Err(bad("an empty media query"));
        }
        if p.contains(',') {
            return Err(bad("a list of queries (an `or`)"));
        }
        let word_of = |s: &str| -> String {
            s.chars()
                .take_while(|c| c.is_ascii_alphabetic() || *c == '-')
                .collect()
        };
        // An optional media type, `only` before it.
        let mut rest = p.as_str();
        let mut word = word_of(rest);
        let only = word == "only";
        if only {
            rest = rest[word.len()..].trim_start();
            word = word_of(rest);
        }
        let mut need_and = false;
        match word.as_str() {
            "all" | "screen" => {
                rest = &rest[word.len()..];
                need_and = true;
            }
            "" if !only => {}
            "not" | "or" => return Err(bad(format!("`{word}`"))),
            "" => return Err(bad("`only` without a media type")),
            other => return Err(bad(format!("the media type `{other}`"))),
        }
        let mut out = Vec::new();
        loop {
            rest = rest.trim_start();
            if rest.is_empty() {
                break;
            }
            if need_and {
                let w = word_of(rest);
                if w != "and" {
                    return Err(bad(format!(
                        "`{}`: conditions are joined by `and`",
                        if w.is_empty() { rest } else { &w }
                    )));
                }
                rest = rest[3..].trim_start();
                if rest.is_empty() {
                    return Err(bad("`and` at the end"));
                }
            }
            let Some(body) = rest.strip_prefix('(') else {
                let w = word_of(rest);
                return Err(match w.as_str() {
                    "not" | "or" | "only" | "all" | "screen" => {
                        bad(format!("`{w}` in the middle of the query"))
                    }
                    _ => bad(format!("`{rest}`")),
                });
            };
            let close = body.find(')').ok_or_else(|| bad("an unclosed condition"))?;
            let inner = &body[..close];
            if inner.contains('(') {
                return Err(bad("nested conditions"));
            }
            let compact: String = inner.chars().filter(|c| !c.is_whitespace()).collect();
            if compact.starts_with("not") {
                return Err(bad("`not` inside a condition"));
            }
            out.extend(feature(&compact)?);
            rest = &body[close + 1..];
            need_and = true;
        }
        Ok(out)
    }

    /// The viewport cells of a skin: the distinct breakpoints of every media condition split
    /// each axis; a cell is one interval of width times one of height.
    #[derive(Debug, Clone, Default, PartialEq)]
    pub struct Cells {
        /// Width breakpoints, ascending.
        pub media_w: Vec<Bound>,
        /// Height breakpoints, ascending.
        pub media_h: Vec<Bound>,
    }

    fn axis_index(bounds: &[Bound], v: f32) -> usize {
        bounds
            .iter()
            .filter(|b| {
                if b.upper_inclusive {
                    v > b.px
                } else {
                    v >= b.px
                }
            })
            .count()
    }

    /// One value of a cell axis: the lower end of the interval, else one below the upper end.
    fn axis_value(bounds: &[Bound], i: usize, default: u32) -> u32 {
        if i > 0 {
            let lo = bounds[i - 1];
            if lo.upper_inclusive {
                lo.px.floor() as u32 + 1
            } else {
                lo.px.ceil() as u32
            }
        } else if let Some(hi) = bounds.first() {
            if hi.upper_inclusive {
                (hi.px.floor() as u32).max(1)
            } else {
                (hi.px.ceil() as u32).saturating_sub(1).max(1)
            }
        } else {
            default
        }
    }

    impl Cells {
        /// The cells of every media condition of a skin. Errors: two breakpoints at the same size
        /// of one axis with `<` and `<=` (E3-004), more than 8 distinct conditions or more than
        /// 16 cells (E3-006).
        pub fn new(conds: &[MediaCond]) -> Result<Cells, SelectError> {
            let mut cells = Cells::default();
            for c in conds {
                let b = c.bound();
                let list = match c.axis {
                    Axis::Width => &mut cells.media_w,
                    Axis::Height => &mut cells.media_h,
                };
                match list.iter().find(|x| x.px == b.px) {
                    Some(x) if x.upper_inclusive != b.upper_inclusive => {
                        return Err(bad(format!(
                            "two breakpoints at {} px of the {} with and without `=`; use one form",
                            b.px,
                            match c.axis {
                                Axis::Width => "width",
                                Axis::Height => "height",
                            }
                        )));
                    }
                    Some(_) => {}
                    None => list.push(b),
                }
            }
            cells.media_w.sort_by(|a, b| a.px.total_cmp(&b.px));
            cells.media_h.sort_by(|a, b| a.px.total_cmp(&b.px));
            let (w, h) = (cells.media_w.len(), cells.media_h.len());
            if w + h > MAX_MEDIA_CONDS || cells.count() > MAX_CELLS {
                return Err(SelectError::new(
                    codes::CELLS,
                    format!(
                        "{} media conditions ({w} on width, {h} on height) make {} cells; at most \
                         {MAX_MEDIA_CONDS} conditions and {MAX_CELLS} cells",
                        w + h,
                        cells.count()
                    ),
                ));
            }
            Ok(cells)
        }

        /// The number of cells: `(width breakpoints + 1) * (height breakpoints + 1)`.
        pub fn count(&self) -> usize {
            (self.media_w.len() + 1) * (self.media_h.len() + 1)
        }

        /// The cell of a logical viewport (CSS px), as `Styles::cell` computes it.
        pub fn cell(&self, viewport: [f32; 2]) -> usize {
            axis_index(&self.media_w, viewport[0]) * (self.media_h.len() + 1)
                + axis_index(&self.media_h, viewport[1])
        }

        /// The viewport the converter reads a cell at (whole CSS px inside the cell).
        pub fn viewport(&self, cell: usize) -> [u32; 2] {
            let per = self.media_h.len() + 1;
            [
                axis_value(&self.media_w, cell / per, DEFAULT_VIEWPORT[0]),
                axis_value(&self.media_h, cell % per, DEFAULT_VIEWPORT[1]),
            ]
        }

        /// The viewport of every cell, in cell order.
        pub fn viewports(&self) -> Vec<[u32; 2]> {
            (0..self.count()).map(|c| self.viewport(c)).collect()
        }

        /// Whether a rule under `conds` applies in a cell.
        pub fn applies(&self, conds: &[MediaCond], cell: usize) -> bool {
            let [w, h] = self.viewport(cell);
            conds.iter().all(|c| c.holds([w as f32, h as f32]))
        }
    }
}
