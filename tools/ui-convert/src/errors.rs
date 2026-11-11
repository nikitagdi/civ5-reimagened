//! Converter errors: one record per problem, with the place, the element, the property or
//! attribute, and why. Every code has a text (`what`, `hint`) in the profile's `profile.toml`.
//!
//! ```text
//! error[E2-010]: css/hud.css:251:3: `.chat .log .t` float
//!   why: property outside the profile: `float: right`
//!   hint: make the parent display:flex ...
//!   at: parts/chat.html:9:7 <span class="t">
//! ```
//!
//! Paths are relative to the skin folder with `/` separators; nothing machine-specific is ever
//! written. Errors are sorted by (file, line, column, code) and then by the rest of the record, so
//! the output is the same on every run.

use std::fmt;

use serde::Serialize;
use ui2_package::profile::Profile;

/// The codes the converter raises itself. The texts live in `profile.toml`; the codes of the
/// profile's own parsers (`values`, `attrs`, `expr`) are reused as they come.
pub mod codes {
    /// `skin.toml` is malformed.
    pub const SKIN_TOML: &str = "E1-001";
    /// An element outside the profile.
    pub const ELEMENT: &str = "E1-002";
    /// A resource outside the skin folder (remote, absolute or missing).
    pub const RESOURCE: &str = "E1-003";
    /// An inline or module script.
    pub const INLINE_SCRIPT: &str = "E1-004";
    /// An `id` used twice.
    pub const DUP_ID: &str = "E1-005";
    /// An element without the computed value the profile requires for it.
    pub const REQUIRE: &str = "E1-006";
    /// `import` or `export` in a skin script.
    pub const JS_MODULE: &str = "E1-007";
    /// A browser API name in a skin script.
    pub const JS_NAME: &str = "E1-008";
    /// A `data-include` target that does not exist.
    pub const INCLUDE_MISSING: &str = "E1-009";

    /// A property outside the profile.
    pub const PROP: &str = "E2-010";
    /// A value Chrome computed that the style record cannot hold.
    pub const STORE: &str = "E2-011";
    /// An icon paint property on an element that is not an icon.
    pub const PAINT: &str = "E2-012";
    /// The page background is not transparent.
    pub const PAGE_BG: &str = "E2-013";

    /// An at-rule outside the profile.
    pub const AT_RULE: &str = "E3-001";
    /// A selector outside the profile.
    pub const SELECTOR: &str = "E3-002";
    /// A class a binding toggles is also written in the static `class`.
    pub const DYN_CLASS: &str = "E3-003";
    /// A media query outside the profile.
    pub const MEDIA: &str = "E3-004";
    /// More than four conditions on one node.
    pub const CONDS: &str = "E3-005";
    /// Too many breakpoints or viewport cells.
    pub const CELLS: &str = "E3-006";
    /// Transitions differ between the variants of a node.
    pub const TRANSITIONS: &str = "E3-007";
    /// `display: contents` changes with a condition or a breakpoint.
    pub const CONTENTS: &str = "E3-008";
    /// `order` changes with a condition or a breakpoint.
    pub const ORDER: &str = "E3-009";
    /// A CSS syntax error.
    pub const CSS_SYNTAX: &str = "E3-010";

    /// A sample does not fit the model schema.
    pub const SAMPLE: &str = "E4-040";
    /// A sample state value that the skin state does not declare.
    pub const SAMPLE_STATE: &str = "E4-041";
    /// A skin state value of a type the profile has no place for.
    pub const STATE_TYPE: &str = "E4-042";

    /// SVG content outside the profile.
    pub const SVG: &str = "E5-001";
    /// An icon reference outside the skin's sprite.
    pub const SVG_REF: &str = "E5-002";
    /// A font face whose file is not a skin font of skin.toml.
    pub const FONT_FILE: &str = "E5-003";
    /// A font face that Chromium did not load.
    pub const FONT_LOAD: &str = "E5-004";
    /// A picture of the skin that cannot be rasterized.
    pub const IMAGE: &str = "E5-005";

    /// A package budget is exceeded.
    pub const BUDGET: &str = "E8-001";

    /// Text that is not translated.
    pub const UNTRANSLATED: &str = "E6-001";
    /// A text key that a language does not have.
    pub const L10N_MISSING: &str = "E6-002";
    /// A malformed l10n table or template.
    pub const L10N_FILE: &str = "E6-003";
    /// A text template uses more arguments than the node passes.
    pub const L10N_ARGS: &str = "E6-004";

    /// The children of `body` are not the four layer roots.
    pub const LAYERS: &str = "E7-001";
    /// A required extension point is missing.
    pub const SLOT_MISSING: &str = "E7-002";
    /// A required color token synonym is missing.
    pub const TOKEN_MISSING: &str = "E7-003";
    /// The same hotkey twice in one layer.
    pub const HOTKEY_DUP: &str = "E7-004";
    /// The same part name twice.
    pub const PART_DUP: &str = "E7-005";
    /// A tooltip reference without its template.
    pub const TOOLTIP_REF: &str = "E7-006";
    /// A layer root is not an absolute, full-size box without pointer events.
    pub const LAYER_ROOT: &str = "E7-007";

    /// A request outside the local server (raised by the Node side).
    pub const NET: &str = "E9-001";
    /// Node or Chromium failed.
    pub const NODE: &str = "E9-002";
    /// A bad request on the Node protocol.
    pub const PROTOCOL: &str = "E9-003";
    /// The skin folder or its entry file is missing.
    pub const SKIN_DIR: &str = "E9-004";
    /// The preview script is missing: includes and bindings were not expanded.
    pub const PREVIEW: &str = "E9-005";
    /// The font subsetter is missing or of another version.
    pub const FONTTOOLS: &str = "E9-006";
    /// The package could not be written.
    pub const WRITE: &str = "E9-007";

    /// Every code of this list.
    pub const ALL: &[&str] = &[
        SKIN_TOML,
        ELEMENT,
        RESOURCE,
        INLINE_SCRIPT,
        DUP_ID,
        REQUIRE,
        JS_MODULE,
        JS_NAME,
        INCLUDE_MISSING,
        PROP,
        STORE,
        PAINT,
        PAGE_BG,
        AT_RULE,
        SELECTOR,
        DYN_CLASS,
        MEDIA,
        CONDS,
        CELLS,
        TRANSITIONS,
        CONTENTS,
        ORDER,
        CSS_SYNTAX,
        SAMPLE,
        SAMPLE_STATE,
        STATE_TYPE,
        SVG,
        SVG_REF,
        FONT_FILE,
        FONT_LOAD,
        IMAGE,
        BUDGET,
        UNTRANSLATED,
        L10N_MISSING,
        L10N_FILE,
        L10N_ARGS,
        LAYERS,
        SLOT_MISSING,
        TOKEN_MISSING,
        HOTKEY_DUP,
        PART_DUP,
        TOOLTIP_REF,
        LAYER_ROOT,
        NET,
        NODE,
        PROTOCOL,
        SKIN_DIR,
        PREVIEW,
        FONTTOOLS,
        WRITE,
    ];
}

/// A place in a source file of the skin: path relative to the skin folder, 1-based line and
/// column (0 when unknown).
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Place {
    /// The file, relative to the skin folder, `/` separators. Empty when the problem has no file.
    pub file: String,
    /// The line, 1-based; 0 when unknown.
    pub line: u32,
    /// The column, 1-based; 0 when unknown.
    pub col: u32,
}

impl Place {
    /// A file without a position.
    pub fn file(file: &str) -> Self {
        Self {
            file: file.to_string(),
            line: 0,
            col: 0,
        }
    }

    /// A position in a file.
    pub fn at(file: &str, line: u32, col: u32) -> Self {
        Self {
            file: file.to_string(),
            line,
            col,
        }
    }

    /// `"<file>:<line>:<col>"` as the server writes it into `data-ui2-src`.
    pub fn parse_src(src: &str) -> Option<Self> {
        let (rest, col) = src.rsplit_once(':')?;
        let (file, line) = rest.rsplit_once(':')?;
        Some(Self::at(file, line.parse().ok()?, col.parse().ok()?))
    }
}

impl fmt::Display for Place {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.file.is_empty(), self.line) {
            (true, _) => f.write_str("(skin)"),
            (false, 0) => f.write_str(&self.file),
            (false, _) => write!(f, "{}:{}:{}", self.file, self.line, self.col),
        }
    }
}

/// One problem found in a skin.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct ConvError {
    /// Where: the declaration, the element's start tag, the file.
    pub place: Place,
    /// The code (`E2-010`), with a text in `profile.toml`.
    pub code: String,
    /// The element (`<span class="t">`) or the selector the problem is about; may be empty.
    pub element: String,
    /// The property or attribute; may be empty.
    pub prop: String,
    /// Why, in this instance (the profile text says why in general).
    pub why: String,
    /// A second place that explains the first (the element a declaration applied to).
    pub at: Option<String>,
}

impl ConvError {
    /// A problem with a code and a reason.
    pub fn new(code: &str, place: Place, why: impl Into<String>) -> Self {
        Self {
            place,
            code: code.to_string(),
            element: String::new(),
            prop: String::new(),
            why: why.into(),
            at: None,
        }
    }

    /// Names the element or selector.
    pub fn element(mut self, element: impl Into<String>) -> Self {
        self.element = element.into();
        self
    }

    /// Names the property or attribute.
    pub fn prop(mut self, prop: impl Into<String>) -> Self {
        self.prop = prop.into();
        self
    }

    /// Adds the second place.
    pub fn with_at(mut self, at: impl Into<String>) -> Self {
        self.at = Some(at.into());
        self
    }

    /// The profile's text for the code: (what, hint).
    pub fn texts(&self) -> (&'static str, &'static str) {
        match Profile::v1().error(&self.code) {
            Some(def) => (def.what.as_str(), def.hint.as_str()),
            None => ("", ""),
        }
    }

    /// The record as `errors.json` stores it.
    pub fn to_json(&self) -> serde_json::Value {
        let (what, hint) = self.texts();
        serde_json::json!({
            "code": self.code,
            "file": self.place.file,
            "line": self.place.line,
            "col": self.place.col,
            "element": self.element,
            "prop": self.prop,
            "what": what,
            "why": self.why,
            "hint": hint,
            "at": self.at,
        })
    }
}

impl fmt::Display for ConvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (what, hint) = self.texts();
        write!(f, "error[{}]: {}", self.code, self.place)?;
        let subject = match (self.element.is_empty(), self.prop.is_empty()) {
            (true, true) => String::new(),
            (false, true) => self.element.clone(),
            (true, false) => self.prop.clone(),
            (false, false) => format!("{} {}", self.element, self.prop),
        };
        if !subject.is_empty() {
            write!(f, ": {subject}")?;
        }
        match (what.is_empty(), self.why.is_empty()) {
            (false, false) => write!(f, "\n  why: {what}: {}", self.why)?,
            (false, true) => write!(f, "\n  why: {what}")?,
            (true, false) => write!(f, "\n  why: {}", self.why)?,
            (true, true) => {}
        }
        if !hint.is_empty() {
            write!(f, "\n  hint: {hint}")?;
        }
        if let Some(at) = &self.at {
            write!(f, "\n  at: {at}")?;
        }
        Ok(())
    }
}

/// Sorts errors by (file, line, column, code, ...) and drops exact repeats.
pub fn sort(errors: &mut Vec<ConvError>) {
    errors.sort();
    errors.dedup();
}

/// `errors.json`: the sorted list.
pub fn to_json(errors: &[ConvError]) -> String {
    let list: Vec<serde_json::Value> = errors.iter().map(ConvError::to_json).collect();
    let mut s = serde_json::to_string_pretty(&list).unwrap_or_else(|_| "[]".to_string());
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_converter_code_has_a_profile_text() {
        for code in codes::ALL {
            let def = Profile::v1().error(code);
            assert!(def.is_some(), "{code} has no text in profile.toml");
        }
    }

    #[test]
    fn errors_sort_by_place_then_code() {
        let mut v = vec![
            ConvError::new("E3-002", Place::at("css/b.css", 1, 1), "x"),
            ConvError::new("E2-010", Place::at("css/a.css", 9, 3), "x"),
            ConvError::new("E1-002", Place::at("css/a.css", 9, 3), "x"),
            ConvError::new("E1-002", Place::at("css/a.css", 2, 7), "x"),
            ConvError::new("E1-002", Place::at("css/a.css", 2, 7), "x"),
        ];
        sort(&mut v);
        let got: Vec<String> = v
            .iter()
            .map(|e| format!("{} {}", e.place, e.code))
            .collect();
        assert_eq!(
            got,
            [
                "css/a.css:2:7 E1-002",
                "css/a.css:9:3 E1-002",
                "css/a.css:9:3 E2-010",
                "css/b.css:1:1 E3-002"
            ]
        );
    }

    #[test]
    fn display_names_file_element_property_and_why() {
        let e = ConvError::new(
            codes::PROP,
            Place::at("css/hud.css", 251, 3),
            "`float: right`",
        )
        .element("`.chat .log .t`")
        .prop("float")
        .with_at("parts/chat.html:9:7 <span class=\"t\">");
        let text = e.to_string();
        assert!(text.starts_with("error[E2-010]: css/hud.css:251:3: `.chat .log .t` float\n"));
        assert!(text.contains("\n  why: property outside the profile: `float: right`"));
        assert!(text.contains("\n  hint: "));
        assert!(text.ends_with("\n  at: parts/chat.html:9:7 <span class=\"t\">"));
    }

    #[test]
    fn src_attribute_parses_into_a_place() {
        assert_eq!(
            Place::parse_src("parts/top.html:6:15"),
            Some(Place::at("parts/top.html", 6, 15))
        );
        assert_eq!(Place::parse_src("index.html"), None);
    }
}
