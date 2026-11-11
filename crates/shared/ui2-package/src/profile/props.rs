//! The profile tables: elements, properties, ignored properties, selectors, slots and tokens, read
//! from `profile/profile.toml`.
//!
//! Adding a property later is four small edits, none of which changes a type: a `PropId` variant
//! (append), a `prop.<name>` row in `profile.toml`, a `StyleRecord` field (a new package major),
//! and, only if the property needs a new kind of value, a `ValueKind` arm with its parser.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

use super::ids::{PropId, Tag};

/// How the value of a property is parsed (`values.rs`). One arm per kind, one parser per arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ValueKind {
    /// One of `values`.
    Keyword,
    /// `values[0]` alone, or any combination of the rest.
    KeywordSet,
    /// A length (`px`, `%`, `calc`), or one of `values` (`auto`, `none`, `normal`).
    Length,
    /// A color (`#hex`, `rgb()`, `rgba()`, a name, `transparent`), or one of `values`.
    Color,
    /// A float, or one of `values`.
    Number,
    /// An integer, or one of `values`.
    Integer,
    /// A corner radius: one length or two (horizontal vertical).
    Radius,
    /// `none` or a comma list of shadows.
    Shadows,
    /// `none` or a list of `translate*`, `rotate`, `scale*`.
    Transform,
    /// Two lengths (or `left`/`center`/`right`, `top`/`center`/`bottom`).
    Origin,
    /// A comma list of times (`s`, `ms`).
    Times,
    /// A comma list of timing functions.
    Easings,
    /// `none`, `all` or a comma list of animatable properties.
    Properties,
    /// A comma list of font family names.
    Families,
    /// `normal`, a length in px, or a number.
    LineHeight,
    /// `none` or a comma list of gradient layers (`background-image`).
    Images,
    /// A comma list of background sizes: `auto`, `cover`, `contain`, or one or two lengths/`auto`.
    BgSizes,
    /// A comma list of lengths (`background-position-x`, `-y`).
    Lengths,
    /// A comma list of `background-repeat` values.
    Repeats,
    /// A comma list of keywords, each one of `values`.
    Keywords,
    /// `auto` or `w / h` (`aspect-ratio`).
    Ratio,
    /// `none` or a track list with `repeat(N, ...)` (`grid-template-*`).
    Tracks,
    /// A list of track sizes (`grid-auto-*`).
    TrackSizes,
    /// `auto`, a line number or `span N` (`grid-row-start` ...).
    GridLine,
    /// `none` or `url()` of a picture (`border-image-source`).
    Url,
    /// One to four numbers or percentages and `fill` (`border-image-slice`).
    Slice,
    /// One to four px lengths or numbers (`border-image-width`).
    ImageWidths,
    /// One or two of `values` (`border-image-repeat`).
    ImageRepeat,
    /// `none` or a convex `polygon()` (`clip-path`).
    Polygon,
}

/// One property of the profile.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropDef {
    /// How the value is parsed.
    pub kind: ValueKind,
    /// Keywords (see the file header).
    #[serde(default)]
    pub values: Vec<String>,
    /// Inherited by children.
    #[serde(default)]
    pub inherited: bool,
    /// Can be the target of a transition.
    #[serde(default)]
    pub anim: bool,
    /// Can be set by `data-bind-style-<name>`.
    #[serde(default)]
    pub bind: bool,
    /// A color that may refer to a root color token.
    #[serde(default)]
    pub token: bool,
    /// `calc()` is allowed.
    #[serde(default)]
    pub calc: bool,
    /// A percentage is allowed in a length.
    #[serde(default = "yes")]
    pub pct: bool,
    /// Lower bound, inclusive.
    #[serde(default)]
    pub min: Option<f64>,
    /// Upper bound, inclusive.
    #[serde(default)]
    pub max: Option<f64>,
    /// Longest list.
    #[serde(default)]
    pub max_items: Option<usize>,
    /// The initial value, as Chrome serializes it.
    pub initial: String,
}

fn yes() -> bool {
    true
}

/// One element of the profile.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElementDef {
    /// The profile's grouping: `document`, `block`, `text`, `control`, `image`.
    pub group: String,
    /// Allowed `type` attribute values, when restricted.
    #[serde(default)]
    pub types: Vec<String>,
    /// Computed values the element must have (`appearance: none`, `list-style: none`).
    #[serde(default)]
    pub require: BTreeMap<String, String>,
}

/// A property the profile ignores, and why.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IgnoredDef {
    /// For the documentation.
    pub why: String,
}

/// The selector subset.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectorDef {
    /// Dynamic pseudo-classes, without the colon.
    pub pseudo_class: Vec<String>,
    /// Combinators.
    pub combinators: Vec<String>,
}

/// An extension point.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotDef {
    /// Every skin must define it.
    #[serde(default)]
    pub required: bool,
}

/// A color token synonym.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenDef {
    /// Every skin must define it.
    #[serde(default)]
    pub required: bool,
}

/// A converter error code with its text.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorDef {
    /// What is wrong.
    pub what: String,
    /// What to do instead.
    pub hint: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    version: String,
    element: BTreeMap<String, ElementDef>,
    prop: BTreeMap<String, PropDef>,
    #[serde(default)]
    ignored: BTreeMap<String, IgnoredDef>,
    selector: SelectorDef,
    #[serde(default)]
    slot: BTreeMap<String, SlotDef>,
    #[serde(default)]
    token: BTreeMap<String, TokenDef>,
    #[serde(default)]
    error: BTreeMap<String, ErrorDef>,
}

/// `profile.toml` could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TableError {
    /// Not valid TOML, or a field of the wrong shape.
    #[error("profile.toml: {0}")]
    Toml(String),
    /// Valid TOML that breaks a rule of the table.
    #[error("profile.toml: {0}")]
    Invalid(String),
}

/// The profile: what a skin may use.
#[derive(Debug)]
pub struct Profile {
    version: (u16, u16),
    elements: BTreeMap<Tag, ElementDef>,
    props: Vec<PropDef>,
    ignored: BTreeMap<String, IgnoredDef>,
    selector: SelectorDef,
    slots: BTreeMap<String, SlotDef>,
    tokens: BTreeMap<String, TokenDef>,
    errors: BTreeMap<String, ErrorDef>,
}

static V1: OnceLock<Profile> = OnceLock::new();

impl Profile {
    /// Profile 1.x, from the embedded `profile.toml`.
    pub fn v1() -> &'static Profile {
        V1.get_or_init(|| {
            // The file is part of the crate and the `profile_` tests load it, so this cannot fail
            // in a build that passed them.
            Profile::parse(include_str!("../../profile/profile.toml"))
                .expect("the embedded profile.toml is valid")
        })
    }

    /// Reads a profile table and checks it against the `PropId` and `Tag` lists.
    pub fn parse(text: &str) -> Result<Profile, TableError> {
        let raw: RawProfile = toml::from_str(text).map_err(|e| TableError::Toml(e.to_string()))?;
        let invalid = |m: String| Err(TableError::Invalid(m));
        let version = parse_version(&raw.version)
            .ok_or_else(|| TableError::Invalid(format!("bad version {:?}", raw.version)))?;

        let mut elements = BTreeMap::new();
        for (name, def) in raw.element {
            let Some(tag) = Tag::from_name(&name) else {
                return invalid(format!("element {name:?} has no Tag variant"));
            };
            elements.insert(tag, def);
        }
        if let Some(t) = Tag::ALL.iter().find(|t| !elements.contains_key(t)) {
            return invalid(format!("Tag::{t:?} has no element row"));
        }

        let mut by_id: BTreeMap<PropId, PropDef> = BTreeMap::new();
        for (name, def) in raw.prop {
            let Some(id) = PropId::from_name(&name) else {
                return invalid(format!("property {name:?} has no PropId variant"));
            };
            check_prop(&name, &def)?;
            by_id.insert(id, def);
        }
        let mut props = Vec::with_capacity(PropId::ALL.len());
        for &id in PropId::ALL {
            match by_id.remove(&id) {
                Some(def) => props.push(def),
                None => return invalid(format!("PropId::{id:?} has no prop row")),
            }
        }
        for name in raw.ignored.keys() {
            if PropId::from_name(name).is_some() {
                return invalid(format!("{name:?} is both a property and ignored"));
            }
        }
        if raw.selector.pseudo_class.iter().any(String::is_empty)
            || raw.selector.combinators.iter().any(String::is_empty)
        {
            return invalid("empty selector entry".to_string());
        }
        Ok(Profile {
            version,
            elements,
            props,
            ignored: raw.ignored,
            selector: raw.selector,
            slots: raw.slot,
            tokens: raw.token,
            errors: raw.error,
        })
    }

    /// The profile version `(major, minor)`.
    pub fn version(&self) -> (u16, u16) {
        self.version
    }

    /// The id of a property by its CSS name.
    pub fn prop(&self, name: &str) -> Option<PropId> {
        PropId::from_name(name)
    }

    /// The definition of a property.
    pub fn def(&self, id: PropId) -> &PropDef {
        &self.props[id.index()]
    }

    /// Every property definition, in `PropId` order.
    pub fn props(&self) -> &[PropDef] {
        &self.props
    }

    /// The definition of an element.
    pub fn element(&self, tag: Tag) -> Option<&ElementDef> {
        self.elements.get(&tag)
    }

    /// Whether the profile ignores the property (documented, without effect).
    pub fn ignored(&self, name: &str) -> Option<&IgnoredDef> {
        self.ignored.get(name)
    }

    /// Every ignored property, by name.
    pub fn ignored_props(&self) -> &BTreeMap<String, IgnoredDef> {
        &self.ignored
    }

    /// The selector subset.
    pub fn selector(&self) -> &SelectorDef {
        &self.selector
    }

    /// The extension points of the profile.
    pub fn slots(&self) -> &BTreeMap<String, SlotDef> {
        &self.slots
    }

    /// The color token synonyms.
    pub fn tokens(&self) -> &BTreeMap<String, TokenDef> {
        &self.tokens
    }

    /// The text of an error code.
    pub fn error(&self, code: &str) -> Option<&ErrorDef> {
        self.errors.get(code)
    }
}

fn parse_version(s: &str) -> Option<(u16, u16)> {
    let (major, minor) = s.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

fn check_prop(name: &str, def: &PropDef) -> Result<(), TableError> {
    let bad = |m: &str| Err(TableError::Invalid(format!("prop.{name}: {m}")));
    let keywords = matches!(
        def.kind,
        ValueKind::Keyword | ValueKind::KeywordSet | ValueKind::Keywords | ValueKind::ImageRepeat
    );
    if keywords && def.values.is_empty() {
        return bad("a keyword property needs values");
    }
    if def.initial.is_empty() {
        return bad("initial is empty");
    }
    if let (Some(lo), Some(hi)) = (def.min, def.max)
        && lo > hi
    {
        return bad("min is above max");
    }
    if def.min.is_some_and(|v| !v.is_finite()) || def.max.is_some_and(|v| !v.is_finite()) {
        return bad("min and max must be finite");
    }
    let mut seen = def.values.clone();
    seen.sort_unstable();
    if seen.windows(2).any(|w| w[0] == w[1]) {
        return bad("repeated value");
    }
    if def.token
        && !matches!(
            def.kind,
            ValueKind::Color | ValueKind::Shadows | ValueKind::Images
        )
    {
        return bad("only colors, shadows and gradients can use tokens");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_v1_loads_and_covers_every_id() {
        let p = Profile::v1();
        assert_eq!(p.version(), (1, 0));
        assert_eq!(p.props().len(), PropId::ALL.len());
        for &tag in Tag::ALL {
            assert!(p.element(tag).is_some(), "{tag:?}");
        }
        assert_eq!(p.prop("flex-grow"), Some(PropId::FlexGrow));
        assert_eq!(p.prop("float"), None);
        assert!(p.ignored("user-select").is_some());
        assert!(p.slots()["mods"].required);
        assert_eq!(p.tokens().len(), 17);
        assert_eq!(p.selector().pseudo_class.len(), 5);
    }

    #[test]
    fn profile_flags_follow_the_spec() {
        let p = Profile::v1();
        let color = p.def(PropId::Color);
        assert!(color.inherited && color.anim && color.bind && color.token);
        assert!(!p.def(PropId::BackgroundColor).inherited);
        assert!(p.def(PropId::Width).bind);
        assert!(!p.def(PropId::MinWidth).bind);
        assert_eq!(p.def(PropId::BoxShadow).max_items, Some(4));
        assert!(p.def(PropId::Transform).calc);
        assert!(!p.def(PropId::Width).calc);
        let image = p.def(PropId::BackgroundImage);
        assert!(image.token && image.calc && !image.anim && !image.bind);
        assert_eq!(image.max_items, Some(4));
        assert!(p.def(PropId::BackgroundSize).bind);
        let outline = p.def(PropId::OutlineColor);
        assert!(outline.anim && outline.token && !outline.bind);
        assert!(!p.def(PropId::OutlineWidth).pct);
    }

    #[test]
    fn profile_every_property_has_a_value_kind_and_an_initial() {
        for &id in PropId::ALL {
            let def = Profile::v1().def(id);
            assert!(!def.initial.is_empty(), "{}", id.name());
            if matches!(def.kind, ValueKind::Keyword | ValueKind::KeywordSet) {
                assert!(!def.values.is_empty(), "{}", id.name());
            }
        }
    }

    #[test]
    fn profile_a_missing_row_is_an_error() {
        let text = include_str!("../../profile/profile.toml")
            .replace("prop.display = ", "prop.displayx = ");
        assert!(matches!(Profile::parse(&text), Err(TableError::Invalid(_))));
        let text =
            include_str!("../../profile/profile.toml").replace("element.div = ", "element.divx = ");
        assert!(matches!(Profile::parse(&text), Err(TableError::Invalid(_))));
        assert!(matches!(
            Profile::parse("version = 1"),
            Err(TableError::Toml(_))
        ));
    }

    #[test]
    fn profile_unknown_fields_are_rejected() {
        let text = include_str!("../../profile/profile.toml").replace(
            "prop.display = { kind",
            "prop.display = { color = true, kind",
        );
        assert!(matches!(Profile::parse(&text), Err(TableError::Toml(_))));
    }
}
