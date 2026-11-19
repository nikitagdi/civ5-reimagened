//! `skin.toml`: the skin's manifest.
//!
//! ```toml
//! [skin]
//! id = "engraved"            # [a-z][a-z0-9-]*
//! version = "0.1.0"
//! title = "skin.title"       # an l10n key
//! entry = "index.html"
//! model = "1.0"              # the model schema the skin is written against
//! api = "1.0"                # the script API
//! default = true             # optional
//! extends = "engraved"       # optional: files are looked up here first, then in the base skin
//!
//! [fonts]
//! subset = ["latin", "latin-ext", "cyrillic"]
//! sans = "Fira Sans"
//! display = "Alegreya"
//! [fonts.files]              # the path a stylesheet names -> the font file, from the repository root
//! "fonts/FiraSans-Medium.ttf" = "third_party/fonts/fira-sans/FiraSans-Medium.ttf"
//!
//! [state]                    # state.* with defaults; the type follows the value
//! "windows.production" = false
//!
//! [themes]
//! default = "night"
//! dawn = { css = "themes/dawn.css" }
//!
//! [scripts]
//! files = ["scripts/hud.js"]
//! reads = ["research", "turn"]
//!
//! [verify]
//! states = ["hud"]
//! golden = [{ state = "hud", viewport = [1920, 1080], scale = 1.0 }]
//! ```
//!
//! Unknown keys are errors (they are typos more often than not).

use std::collections::BTreeMap;

use serde::Deserialize;
use ui2_package::profile::TypeSig;

use crate::errors::{ConvError, Place, codes};

/// The parsed manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct Skin {
    /// `[skin]`.
    pub skin: SkinSection,
    /// `[fonts]`.
    pub fonts: Fonts,
    /// `[state]`, flattened to dotted paths below `state.`, with their defaults.
    pub state: BTreeMap<String, StateValue>,
    /// `[themes]`: the default theme's name and the other themes' stylesheets.
    pub themes: Themes,
    /// `[scripts]`.
    pub scripts: Scripts,
    /// `[verify]`.
    pub verify: Verify,
    /// Set when the "skin" is the page the converter makes around the panels of a mod (fragment mode): the
    /// mod's `[ui]`. `skin.toml` never sets it.
    pub fragment: Option<crate::fragment::FragmentSpec>,
}

impl Skin {
    /// The sample state the converter scans and extracts with: the first state of `[verify]`, else `hud`.
    pub fn scan_state(&self) -> &str {
        self.verify.states.first().map_or("hud", String::as_str)
    }
}

/// `[skin]`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkinSection {
    /// The skin id.
    pub id: String,
    /// The skin's own version.
    pub version: String,
    /// The l10n key of the title.
    pub title: String,
    /// The entry HTML file.
    #[serde(default = "default_entry")]
    pub entry: String,
    /// The model schema version `major.minor`.
    pub model: String,
    /// The script API version `major.minor`.
    pub api: String,
    /// The default skin of the build.
    #[serde(default)]
    pub default: bool,
    /// The base skin of a derived skin.
    #[serde(default)]
    pub extends: Option<String>,
}

fn default_entry() -> String {
    "index.html".to_string()
}

/// `[fonts]`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fonts {
    /// The named character ranges every face keeps.
    #[serde(default)]
    pub subset: Vec<String>,
    /// The family of the `ui-sans` role.
    #[serde(default)]
    pub sans: Option<String>,
    /// The family of the `ui-display` role.
    #[serde(default)]
    pub display: Option<String>,
    /// The font file of each path a stylesheet names, from the repository root.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

/// A skin state default.
#[derive(Debug, Clone, PartialEq)]
pub enum StateValue {
    /// `true`/`false`.
    Bool(bool),
    /// An integer.
    Int(i64),
    /// A float.
    Float(f64),
    /// A string.
    Str(String),
}

impl StateValue {
    /// The type the expressions see.
    pub fn type_sig(&self) -> TypeSig {
        match self {
            StateValue::Bool(_) => TypeSig::Bool,
            StateValue::Int(_) => TypeSig::Int,
            StateValue::Float(_) => TypeSig::Float,
            StateValue::Str(_) => TypeSig::Str,
        }
    }
}

/// `[themes]`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Themes {
    /// The name of the default theme (the values of the root tokens as written).
    pub default: Option<String>,
    /// The other themes: name -> stylesheet with `:root { --token: color }` only.
    pub css: BTreeMap<String, String>,
}

/// `[scripts]`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scripts {
    /// Classic scripts in execution order.
    #[serde(default)]
    pub files: Vec<String>,
    /// The model roots the scripts read.
    #[serde(default)]
    pub reads: Vec<String>,
}

/// `[verify]`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verify {
    /// The sample states the layout check runs.
    #[serde(default)]
    pub states: Vec<String>,
    /// The golden frames.
    #[serde(default)]
    pub golden: Vec<Golden>,
}

/// One golden frame.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Golden {
    /// The sample state.
    pub state: String,
    /// CSS px.
    pub viewport: [u32; 2],
    /// The UI scale.
    #[serde(default = "one")]
    pub scale: f64,
}

fn one() -> f64 {
    1.0
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    skin: SkinSection,
    #[serde(default)]
    fonts: Fonts,
    #[serde(default)]
    state: toml::Table,
    #[serde(default)]
    themes: toml::Table,
    #[serde(default)]
    scripts: Scripts,
    #[serde(default)]
    verify: Verify,
}

const FILE: &str = "skin.toml";

fn bad(why: impl Into<String>) -> ConvError {
    ConvError::new(codes::SKIN_TOML, Place::file(FILE), why)
}

fn is_id(s: &str) -> bool {
    let mut b = s.bytes();
    b.next().is_some_and(|c| c.is_ascii_lowercase())
        && b.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

fn is_version(s: &str, parts: usize) -> bool {
    let v: Vec<&str> = s.split('.').collect();
    v.len() == parts
        && v.iter()
            .all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()))
}

/// A relative path inside the skin: `/` separators, no `..`, no drive or scheme.
pub fn is_skin_path(p: &str) -> bool {
    !p.is_empty()
        && !p.starts_with('/')
        && !p.contains('\\')
        && !p.contains(':')
        && p.split('/').all(|s| !s.is_empty() && s != "." && s != "..")
}

/// Decodes `%XX` escapes of a URL path; `None` for a malformed escape or bytes that are not UTF-8.
pub fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The arguments of the `url()` functions of a CSS value as written, quotes removed.
pub fn url_args(value: &str) -> Vec<String> {
    let lower = value.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find("url(") {
        let start = from + at + 4;
        let rest = value[start..].trim_start();
        let (arg, used) = match rest.chars().next() {
            Some(q @ ('"' | '\'')) => match rest[1..].find(q) {
                Some(end) => (rest[1..1 + end].to_string(), 1 + end + 1),
                None => (rest[1..].to_string(), rest.len()),
            },
            _ => {
                let end = rest.find(')').unwrap_or(rest.len());
                (rest[..end].trim().to_string(), end)
            }
        };
        out.push(arg);
        from = value.len() - rest.len() + used;
        if from >= value.len() {
            break;
        }
    }
    out
}

/// A picture URL as written in `file` (a path of the skin) resolved to a path of the skin. The error says why
/// the URL is not one: another origin, an absolute path, or `..` above the skin folder.
pub fn resolve_in_skin(file: &str, url: &str) -> Result<String, String> {
    let lower = url.to_ascii_lowercase();
    let scheme = url.split_once(':').is_some_and(|(s, _)| {
        !s.is_empty()
            && s.bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"+.-".contains(&c))
    });
    if scheme || lower.starts_with('/') || lower.starts_with('\\') {
        return Err("is not a file of the skin".to_string());
    }
    let decoded = percent_decode(url).ok_or_else(|| "is not a valid URL path".to_string())?;
    let mut parts: Vec<&str> = file.split('/').collect();
    parts.pop();
    for seg in decoded.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err("leaves the skin folder".to_string());
                }
            }
            s => parts.push(s),
        }
    }
    let path = parts.join("/");
    if is_skin_path(&path) {
        Ok(path)
    } else {
        Err("is not a file of the skin".to_string())
    }
}

/// The path of the skin a URL of the converter's local server names (Chrome writes every `url()` it
/// computed as an absolute URL on that server): `http://127.0.0.1:<port>/<path>`. `None` for anything else
/// (another origin, the server's own `__ui2/` files, a path outside the skin).
pub fn server_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("http://127.0.0.1:")?;
    let (port, path) = rest.split_once('/')?;
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if path.contains(['?', '#']) {
        return None;
    }
    let path = percent_decode(path)?;
    (is_skin_path(&path) && !path.starts_with("__ui2/")).then_some(path)
}

fn flatten_state(
    prefix: &str,
    table: &toml::Table,
    out: &mut BTreeMap<String, StateValue>,
    errs: &mut Vec<ConvError>,
) {
    for (k, v) in table {
        let path = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        // The host mounts the paths as a model fragment, whose names are lower-case snake case; a name the
        // model refuses would leave the whole skin state unmounted at run time.
        let ok_name = path.split('.').all(|seg| {
            let mut b = seg.bytes();
            b.next().is_some_and(|c| c.is_ascii_lowercase())
                && b.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        });
        if !ok_name {
            errs.push(bad(format!(
                "[state] `{path}`: a state path is lower-case snake case names (a-z, 0-9, _; the first a letter) separated by dots"
            )));
            continue;
        }
        let value = match v {
            toml::Value::Boolean(b) => StateValue::Bool(*b),
            toml::Value::Integer(i) => StateValue::Int(*i),
            toml::Value::Float(f) if f.is_finite() => StateValue::Float(*f),
            toml::Value::String(s) => StateValue::Str(s.clone()),
            toml::Value::Table(t) => {
                flatten_state(&path, t, out, errs);
                continue;
            }
            other => {
                errs.push(
                    ConvError::new(
                        codes::STATE_TYPE,
                        Place::file(FILE),
                        format!(
                            "`state.{path}` is a {}; a state value is a bool, an integer, a float or a string",
                            other.type_str()
                        ),
                    )
                    .prop(format!("state.{path}")),
                );
                continue;
            }
        };
        out.insert(path, value);
    }
}

impl Skin {
    /// Parses `skin.toml`. Every problem is reported (E1-001, E4-042).
    pub fn parse(text: &str) -> Result<Skin, Vec<ConvError>> {
        let raw: Raw = toml::from_str(text).map_err(|e| {
            let msg = e.message().to_string();
            let place = match e.span() {
                Some(span) => {
                    let before = &text[..span.start.min(text.len())];
                    let line = before.matches('\n').count() as u32 + 1;
                    let col =
                        before.rsplit('\n').next().map_or(0, |l| l.chars().count()) as u32 + 1;
                    Place::at(FILE, line, col)
                }
                None => Place::file(FILE),
            };
            vec![ConvError::new(codes::SKIN_TOML, place, msg)]
        })?;
        let mut errs = Vec::new();
        let s = &raw.skin;
        if !is_id(&s.id) {
            errs.push(bad(format!(
                "[skin] id `{}`: lower case letters, digits and -",
                s.id
            )));
        }
        if !is_version(&s.version, 3) {
            errs.push(bad(format!(
                "[skin] version `{}`: write major.minor.patch",
                s.version
            )));
        }
        for (name, v) in [("model", &s.model), ("api", &s.api)] {
            if !is_version(v, 2) {
                errs.push(bad(format!("[skin] {name} `{v}`: write major.minor")));
            }
        }
        if !is_skin_path(&s.entry) || !s.entry.ends_with(".html") {
            errs.push(bad(format!(
                "[skin] entry `{}`: an .html file inside the skin",
                s.entry
            )));
        }
        if let Some(base) = &s.extends
            && !is_id(base)
        {
            errs.push(bad(format!("[skin] extends `{base}`: a skin id")));
        }
        for name in &raw.fonts.subset {
            if !crate::SUBSETS.iter().any(|(n, _)| n == name) {
                let known: Vec<&str> = crate::SUBSETS.iter().map(|(n, _)| *n).collect();
                errs.push(bad(format!(
                    "[fonts] subset `{name}`: one of {}",
                    known.join(", ")
                )));
            }
        }
        for (k, v) in &raw.fonts.files {
            if !is_skin_path(k) || !is_skin_path(v) {
                errs.push(bad(format!(
                    "[fonts.files] `{k}` = `{v}`: relative paths with / separators"
                )));
            }
        }
        let mut state = BTreeMap::new();
        flatten_state("", &raw.state, &mut state, &mut errs);

        let mut themes = Themes::default();
        for (k, v) in &raw.themes {
            match (k.as_str(), v) {
                ("default", toml::Value::String(name)) => themes.default = Some(name.clone()),
                (_, toml::Value::Table(t)) => match (t.get("css"), t.len()) {
                    (Some(toml::Value::String(css)), 1) if is_skin_path(css) => {
                        themes.css.insert(k.clone(), css.clone());
                    }
                    _ => errs.push(bad(format!(
                        "[themes] {k}: a theme is {{ css = \"themes/<name>.css\" }}"
                    ))),
                },
                _ => errs.push(bad(format!(
                    "[themes] {k}: `default = \"<name>\"` or `<name> = {{ css = ... }}`"
                ))),
            }
        }
        for f in &raw.scripts.files {
            if !is_skin_path(f) || !f.ends_with(".js") {
                errs.push(bad(format!(
                    "[scripts] files: `{f}` is not a .js file of the skin"
                )));
            }
        }
        for g in &raw.verify.golden {
            if g.viewport.contains(&0) || !(g.scale.is_finite() && g.scale > 0.0) {
                errs.push(bad(format!(
                    "[verify] golden `{}`: viewport and scale are positive",
                    g.state
                )));
            }
        }
        if !errs.is_empty() {
            return Err(errs);
        }
        Ok(Skin {
            skin: raw.skin,
            fonts: raw.fonts,
            state,
            themes,
            scripts: raw.scripts,
            verify: raw.verify,
            fragment: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: &str = r#"
[skin]
id = "t"
version = "0.1.0"
title = "skin.title"
model = "1.0"
api = "1.0"
"#;

    #[test]
    fn picture_urls_resolve_inside_the_skin() {
        assert_eq!(
            url_args("url(\"../images/a b.png\") , url(x.svg), URL( 'q.png' )"),
            ["../images/a b.png", "x.svg", "q.png"]
        );
        assert_eq!(
            resolve_in_skin("css/a.css", "../images/frame%20x.png").as_deref(),
            Ok("images/frame x.png")
        );
        assert_eq!(
            resolve_in_skin("index.html", "./images/a.png").as_deref(),
            Ok("images/a.png")
        );
        assert!(resolve_in_skin("css/a.css", "../../a.png").is_err());
        assert!(resolve_in_skin("css/a.css", "https://example.com/a.png").is_err());
        assert!(resolve_in_skin("css/a.css", "/images/a.png").is_err());
        assert_eq!(
            server_path("http://127.0.0.1:53211/images/frame%20x.png").as_deref(),
            Some("images/frame x.png")
        );
        assert_eq!(server_path("http://127.0.0.1:53211/__ui2/preview.js"), None);
        assert_eq!(server_path("https://example.com/images/a.png"), None);
        assert_eq!(server_path("http://localhost:80/a.png"), None);
        assert_eq!(server_path("http://127.0.0.1:5/a.png?x=1"), None);
    }

    #[test]
    fn engraved_skin_toml_parses() {
        let text = include_str!("../../../skins/engraved/skin.toml");
        let skin = Skin::parse(text).expect("engraved parses");
        assert_eq!(skin.skin.id, "engraved");
        assert_eq!(skin.skin.entry, "index.html");
        assert_eq!(skin.fonts.files.len(), 3);
        assert_eq!(skin.fonts.sans.as_deref(), Some("Fira Sans"));
        assert_eq!(
            skin.state.into_iter().collect::<Vec<_>>(),
            [
                ("advisor.economic".to_string(), StateValue::Int(0)),
                ("advisor.foreign".to_string(), StateValue::Int(0)),
                ("advisor.military".to_string(), StateValue::Int(0)),
                ("advisor.science".to_string(), StateValue::Int(0)),
                ("archaeology.confirm".to_string(), StateValue::Int(0)),
                ("chat.draft".to_string(), StateValue::Str("".to_string())),
                ("chat.open".to_string(), StateValue::Bool(false)),
                ("chat.pick".to_string(), StateValue::Bool(false)),
                ("chat.unread".to_string(), StateValue::Int(0)),
                ("city.buy_tiles".to_string(), StateValue::Bool(false)),
                ("city.hover".to_string(), StateValue::Int(-1)),
                (
                    "city.hover_kind".to_string(),
                    StateValue::Str("".to_string())
                ),
                (
                    "city.hover_yield".to_string(),
                    StateValue::Str("".to_string())
                ),
                ("city.queue_mode".to_string(), StateValue::Int(0)),
                ("city.repeat_order".to_string(), StateValue::Bool(false)),
                ("city.sell".to_string(), StateValue::Int(-1)),
                ("city.tiles_open".to_string(), StateValue::Bool(false)),
                ("city_list.desc".to_string(), StateValue::Bool(true)),
                (
                    "city_list.sort".to_string(),
                    StateValue::Str("population".to_string())
                ),
                (
                    "city_rename.draft".to_string(),
                    StateValue::Str("".to_string())
                ),
                ("city_rename.touched".to_string(), StateValue::Bool(false)),
                ("city_rename.valid".to_string(), StateValue::Bool(false)),
                (
                    "city_rename.why".to_string(),
                    StateValue::Str("".to_string())
                ),
                ("civ_ribbon.hover".to_string(), StateValue::Int(-1)),
                ("civ_ribbon.left".to_string(), StateValue::Bool(false)),
                ("civ_ribbon.visible".to_string(), StateValue::Bool(true)),
                ("confirm.dont_warn".to_string(), StateValue::Bool(false)),
                ("confirm.hover".to_string(), StateValue::Int(-1)),
                (
                    "congress.choice".to_string(),
                    StateValue::Str("".to_string())
                ),
                (
                    "congress.confirm".to_string(),
                    StateValue::Str("none".to_string())
                ),
                (
                    "congress.picker".to_string(),
                    StateValue::Str("none".to_string())
                ),
                ("congress.rename".to_string(), StateValue::Bool(false)),
                (
                    "congress.rename_text".to_string(),
                    StateValue::Str("".to_string())
                ),
                ("congress.slot".to_string(), StateValue::Int(0)),
                ("congress.tip".to_string(), StateValue::Str("".to_string())),
                (
                    "cs.confirm".to_string(),
                    StateValue::Str("none".to_string())
                ),
                ("cs.menu".to_string(), StateValue::Str("main".to_string())),
                ("cs.tip".to_string(), StateValue::Str("".to_string())),
                (
                    "culture.click_mode".to_string(),
                    StateValue::Str("move".to_string())
                ),
                ("culture.offer".to_string(), StateValue::Int(-1)),
                ("culture.own_work".to_string(), StateValue::Int(-1)),
                (
                    "culture.pair_label".to_string(),
                    StateValue::Str("".to_string())
                ),
                ("culture.pair_partner".to_string(), StateValue::Int(-1)),
                ("culture.pair_work".to_string(), StateValue::Int(-1)),
                ("culture.pick".to_string(), StateValue::Int(-1)),
                (
                    "culture.reason".to_string(),
                    StateValue::Str("Pick a work from another civilization.".to_string())
                ),
                (
                    "culture.slot_type".to_string(),
                    StateValue::Str("".to_string())
                ),
                (
                    "culture.sort_cities".to_string(),
                    StateValue::Str("id".to_string())
                ),
                ("culture.source".to_string(), StateValue::Int(-1)),
                (
                    "culture.tab".to_string(),
                    StateValue::Str("yours".to_string())
                ),
                ("culture.tip".to_string(), StateValue::Str("".to_string())),
                ("culture.x".to_string(), StateValue::Int(0)),
                ("culture.y".to_string(), StateValue::Int(0)),
                ("deal.amount".to_string(), StateValue::Int(0)),
                ("deal.item".to_string(), StateValue::Int(0)),
                ("deal.kind".to_string(), StateValue::Int(-1)),
                ("deal.side".to_string(), StateValue::Int(0)),
                ("declare_war.allies".to_string(), StateValue::Bool(false)),
                ("declare_war.deals".to_string(), StateValue::Bool(false)),
                ("declare_war.diplomacy".to_string(), StateValue::Bool(false)),
                (
                    "declare_war.protectors".to_string(),
                    StateValue::Bool(false)
                ),
                ("declare_war.routes".to_string(), StateValue::Bool(false)),
                (
                    "demographics.hover".to_string(),
                    StateValue::Str("population".to_string())
                ),
                ("diplo.deal".to_string(), StateValue::Int(-1)),
                ("diplo.hover".to_string(), StateValue::Int(-1)),
                (
                    "diplo.tab".to_string(),
                    StateValue::Str("relations".to_string())
                ),
                (
                    "economy.happy_open".to_string(),
                    StateValue::Str("".to_string())
                ),
                ("economy.hover".to_string(), StateValue::Str("".to_string())),
                (
                    "economy.open_expense_buildings".to_string(),
                    StateValue::Bool(false)
                ),
                (
                    "economy.open_expense_diplomacy".to_string(),
                    StateValue::Bool(false)
                ),
                (
                    "economy.open_expense_improvements".to_string(),
                    StateValue::Bool(false)
                ),
                (
                    "economy.open_expense_units".to_string(),
                    StateValue::Bool(false)
                ),
                (
                    "economy.open_income_cities".to_string(),
                    StateValue::Bool(false)
                ),
                (
                    "economy.open_income_connections".to_string(),
                    StateValue::Bool(false)
                ),
                (
                    "economy.open_income_diplomacy".to_string(),
                    StateValue::Bool(false)
                ),
                (
                    "economy.open_income_religion".to_string(),
                    StateValue::Bool(false)
                ),
                (
                    "economy.sort".to_string(),
                    StateValue::Str("population".to_string())
                ),
                ("economy.sort_desc".to_string(), StateValue::Bool(false)),
                ("economy.tab".to_string(), StateValue::Str("".to_string())),
                (
                    "end_game.tab".to_string(),
                    StateValue::Str("summary".to_string())
                ),
                (
                    "espionage.agents_sort".to_string(),
                    StateValue::Str("name".to_string())
                ),
                (
                    "espionage.agents_sort_desc".to_string(),
                    StateValue::Bool(false)
                ),
                ("espionage.ask".to_string(), StateValue::Int(-1)),
                (
                    "espionage.confirm".to_string(),
                    StateValue::Str("".to_string())
                ),
                ("espionage.hover_agent".to_string(), StateValue::Int(-1)),
                ("espionage.hover_city".to_string(), StateValue::Int(-1)),
                (
                    "espionage.intrigue_sort".to_string(),
                    StateValue::Str("turn".to_string())
                ),
                (
                    "espionage.intrigue_sort_desc".to_string(),
                    StateValue::Bool(true)
                ),
                ("espionage.moving".to_string(), StateValue::Int(-1)),
                (
                    "espionage.other_sort".to_string(),
                    StateValue::Str("civilization".to_string())
                ),
                (
                    "espionage.other_sort_desc".to_string(),
                    StateValue::Bool(false)
                ),
                (
                    "espionage.own_sort".to_string(),
                    StateValue::Str("name".to_string())
                ),
                (
                    "espionage.own_sort_desc".to_string(),
                    StateValue::Bool(false)
                ),
                ("espionage.selected".to_string(), StateValue::Int(-1)),
                (
                    "espionage.tab".to_string(),
                    StateValue::Str("overview".to_string())
                ),
                ("free_gp.selected".to_string(), StateValue::Int(-1)),
                ("goody.sel".to_string(), StateValue::Int(-1)),
                ("ideology.confirm".to_string(), StateValue::Int(-1)),
                ("ideology.details".to_string(), StateValue::Int(-1)),
                ("ideology.folded".to_string(), StateValue::Int(0)),
                ("ideology.name".to_string(), StateValue::Str("".to_string())),
                ("leader.hover".to_string(), StateValue::Str("".to_string())),
                ("menu.options".to_string(), StateValue::Bool(false)),
                (
                    "menu.panel".to_string(),
                    StateValue::Str("details".to_string())
                ),
                (
                    "military.sort".to_string(),
                    StateValue::Str("name".to_string())
                ),
                ("military.sort_desc".to_string(), StateValue::Bool(false)),
                ("minimap.banners".to_string(), StateValue::Bool(true)),
                ("minimap.flags".to_string(), StateValue::Bool(true)),
                ("minimap.grid".to_string(), StateValue::Bool(false)),
                ("minimap.options".to_string(), StateValue::Bool(false)),
                ("minimap.resources".to_string(), StateValue::Bool(false)),
                ("minimap.routes".to_string(), StateValue::Bool(true)),
                ("minimap.streamer".to_string(), StateValue::Bool(false)),
                ("minimap.yields".to_string(), StateValue::Bool(false)),
                ("mp_vote.pending".to_string(), StateValue::Bool(false)),
                (
                    "notification.hover".to_string(),
                    StateValue::Str("".to_string())
                ),
                ("on_map.city".to_string(), StateValue::Str("".to_string())),
                ("on_map.unit".to_string(), StateValue::Str("".to_string())),
                ("opt.air_rebase_rmb".to_string(), StateValue::Bool(true)),
                ("opt.all_great_people".to_string(), StateValue::Bool(false)),
                ("opt.ambience".to_string(), StateValue::Int(70)),
                ("opt.auto_close_city".to_string(), StateValue::Bool(false)),
                ("opt.auto_close_tech".to_string(), StateValue::Bool(false)),
                ("opt.auto_end_turn".to_string(), StateValue::Bool(false)),
                ("opt.auto_end_turn_sp".to_string(), StateValue::Bool(false)),
                ("opt.auto_unit_cycle".to_string(), StateValue::Bool(true)),
                ("opt.city_advisor_recs".to_string(), StateValue::Bool(true)),
                ("opt.city_ribbon".to_string(), StateValue::Bool(true)),
                ("opt.civ_ribbon".to_string(), StateValue::Bool(true)),
                ("opt.civilian_yields".to_string(), StateValue::Bool(true)),
                ("opt.clock".to_string(), StateValue::Bool(false)),
                ("opt.effects".to_string(), StateValue::Int(80)),
                ("opt.music".to_string(), StateValue::Int(70)),
                (
                    "opt.next_unit_after_orders".to_string(),
                    StateValue::Bool(true)
                ),
                ("opt.no_basic_help".to_string(), StateValue::Bool(false)),
                ("opt.no_tile_recs".to_string(), StateValue::Bool(false)),
                ("opt.policy_info_full".to_string(), StateValue::Bool(true)),
                (
                    "opt.ribbon_side".to_string(),
                    StateValue::Str("right".to_string())
                ),
                ("opt.score_list".to_string(), StateValue::Bool(true)),
                ("opt.scroll_inertia".to_string(), StateValue::Bool(true)),
                (
                    "opt.skip_reward_popups".to_string(),
                    StateValue::Bool(false)
                ),
                ("opt.speech".to_string(), StateValue::Int(80)),
                (
                    "opt.tab".to_string(),
                    StateValue::Str("general".to_string())
                ),
                ("opt.tooltip_delay".to_string(), StateValue::Int(500)),
                ("opt.tooltip_delay_ext".to_string(), StateValue::Int(1500)),
                ("opt.ui_scale".to_string(), StateValue::Int(100)),
                ("opt.unit_ribbon".to_string(), StateValue::Bool(true)),
                ("opt.warn_unassigned".to_string(), StateValue::Bool(true)),
                (
                    "opt.workers_keep_features".to_string(),
                    StateValue::Bool(true)
                ),
                (
                    "opt.workers_keep_improvements".to_string(),
                    StateValue::Bool(true)
                ),
                ("opt.zoom_dynamic".to_string(), StateValue::Bool(true)),
                ("pantheon.confirm".to_string(), StateValue::Int(-1)),
                (
                    "pantheon.effect".to_string(),
                    StateValue::Str("".to_string())
                ),
                ("pantheon.name".to_string(), StateValue::Str("".to_string())),
                ("policies.confirm".to_string(), StateValue::Int(-1)),
                ("policies.expanded".to_string(), StateValue::Bool(false)),
                ("policies.hover".to_string(), StateValue::Int(-1)),
                (
                    "policies.kind".to_string(),
                    StateValue::Str("adopt_policy".to_string())
                ),
                ("policies.name".to_string(), StateValue::Str("".to_string())),
                ("policies.tab".to_string(), StateValue::Str("".to_string())),
                ("policies.tenet_level".to_string(), StateValue::Int(0)),
                ("policies.visible".to_string(), StateValue::Bool(false)),
                ("prod.hide_locked".to_string(), StateValue::Bool(false)),
                (
                    "prod.sort".to_string(),
                    StateValue::Str("order".to_string())
                ),
                ("religion.confirm".to_string(), StateValue::Bool(false)),
                ("religion.hover".to_string(), StateValue::Int(-1)),
                (
                    "religion.label.bonus".to_string(),
                    StateValue::Str("".to_string())
                ),
                (
                    "religion.label.enhancer".to_string(),
                    StateValue::Str("".to_string())
                ),
                (
                    "religion.label.follower".to_string(),
                    StateValue::Str("".to_string())
                ),
                (
                    "religion.label.follower2".to_string(),
                    StateValue::Str("".to_string())
                ),
                (
                    "religion.label.founder".to_string(),
                    StateValue::Str("".to_string())
                ),
                (
                    "religion.label.pantheon".to_string(),
                    StateValue::Str("".to_string())
                ),
                ("religion.list".to_string(), StateValue::Str("".to_string())),
                ("religion.name".to_string(), StateValue::Str("".to_string())),
                ("religion.pick.bonus".to_string(), StateValue::Int(65535)),
                ("religion.pick.enhancer".to_string(), StateValue::Int(65535)),
                ("religion.pick.follower".to_string(), StateValue::Int(65535)),
                (
                    "religion.pick.follower2".to_string(),
                    StateValue::Int(65535)
                ),
                ("religion.pick.founder".to_string(), StateValue::Int(65535)),
                ("religion.pick.pantheon".to_string(), StateValue::Int(65535)),
                ("religion.ready".to_string(), StateValue::Bool(false)),
                ("religion.rename".to_string(), StateValue::Bool(false)),
                ("religion.symbol".to_string(), StateValue::Int(-1)),
                (
                    "religion.symbol_name".to_string(),
                    StateValue::Str("".to_string())
                ),
                (
                    "religion_overview.beliefs_desc".to_string(),
                    StateValue::Bool(false)
                ),
                (
                    "religion_overview.beliefs_sort".to_string(),
                    StateValue::Str("religion_name".to_string())
                ),
                (
                    "religion_overview.menu".to_string(),
                    StateValue::Bool(false)
                ),
                (
                    "religion_overview.optimistic".to_string(),
                    StateValue::Str("".to_string())
                ),
                (
                    "religion_overview.tab".to_string(),
                    StateValue::Str("mine".to_string())
                ),
                (
                    "religion_overview.world_desc".to_string(),
                    StateValue::Bool(false)
                ),
                (
                    "religion_overview.world_sort".to_string(),
                    StateValue::Str("name".to_string())
                ),
                (
                    "tech_award.hover".to_string(),
                    StateValue::Str("".to_string())
                ),
                ("tech_panel.done".to_string(), StateValue::Bool(false)),
                ("tech_panel.last_id".to_string(), StateValue::Int(-1)),
                (
                    "tech_panel.last_name".to_string(),
                    StateValue::Str("".to_string())
                ),
                ("tech_panel.open".to_string(), StateValue::Bool(false)),
                ("techs.hover".to_string(), StateValue::Int(-1)),
                ("techs.unlock".to_string(), StateValue::Int(-1)),
                ("techs.x".to_string(), StateValue::Float(0.0)),
                ("top.hover".to_string(), StateValue::Str("".to_string())),
                ("top.menu".to_string(), StateValue::Bool(false)),
                ("top.resource".to_string(), StateValue::Int(-1)),
                ("top.resources".to_string(), StateValue::Bool(false)),
                (
                    "trade_routes.choose_sort".to_string(),
                    StateValue::Str("city".to_string())
                ),
                (
                    "trade_routes.collapse_city_states".to_string(),
                    StateValue::Bool(false)
                ),
                (
                    "trade_routes.collapse_majors".to_string(),
                    StateValue::Bool(false)
                ),
                (
                    "trade_routes.collapse_yours".to_string(),
                    StateValue::Bool(false)
                ),
                ("trade_routes.confirm".to_string(), StateValue::Int(-1)),
                ("trade_routes.hover".to_string(), StateValue::Int(-1)),
                ("trade_routes.overview".to_string(), StateValue::Bool(false)),
                (
                    "trade_routes.sort".to_string(),
                    StateValue::Str("from".to_string())
                ),
                (
                    "trade_routes.sort_desc".to_string(),
                    StateValue::Bool(false)
                ),
                (
                    "trade_routes.tab".to_string(),
                    StateValue::Str("".to_string())
                ),
                ("trade_routes.tip".to_string(), StateValue::Int(-1)),
                (
                    "trade_routes.tip_list".to_string(),
                    StateValue::Str("".to_string())
                ),
                (
                    "unit_lists.civilian_folded".to_string(),
                    StateValue::Bool(false)
                ),
                ("unit_lists.desc".to_string(), StateValue::Bool(false)),
                (
                    "unit_lists.military_folded".to_string(),
                    StateValue::Bool(false),
                ),
                (
                    "unit_lists.section".to_string(),
                    StateValue::Str("".to_string())
                ),
                (
                    "unit_lists.sort".to_string(),
                    StateValue::Str("name".to_string())
                ),
                (
                    "unit_panel.draft".to_string(),
                    StateValue::Str("".to_string())
                ),
                ("unit_panel.hover".to_string(), StateValue::Int(-1)),
                ("unit_panel.promo".to_string(), StateValue::Int(-1)),
                ("unit_panel.valid".to_string(), StateValue::Bool(false)),
                (
                    "victory.hover".to_string(),
                    StateValue::Str("irr".to_string())
                ),
                (
                    "victory.view".to_string(),
                    StateValue::Str("summary".to_string())
                ),
            ]
        );
        assert_eq!(
            skin.scripts.files,
            [
                "js/chat.js",
                "js/minimap.js",
                "js/unit_lists.js",
                "js/unit_panel.js",
                "js/production.js",
                "js/tech_panel.js",
                "js/techs.js",
                "js/city_rename.js",
                "js/city.js",
                "js/city_list.js",
                "js/free_great_person.js",
                "js/policies.js",
                "js/ideology.js",
                "js/pantheon.js",
                "js/religion.js",
                "js/religion_overview.js",
                "js/civ_ribbon.js",
                "js/leader.js",
                "js/economy.js",
                "js/military.js",
                "js/trade_routes.js",
                "js/espionage.js",
                "js/victory.js",
                "js/demographics.js",
                "js/culture.js",
                "js/archaeology.js",
                "js/city_state.js",
                "js/congress.js",
                "js/mp_vote.js",
                "js/end_game.js",
                "js/advisor.js",
                "js/game_menu.js",
                "js/confirm.js",
                "js/message.js",
                "js/goody_choice.js",
            ]
        );
        // The first state is the one the converter scans with.
        assert_eq!(
            skin.verify.states,
            [
                "hud",
                "top_panel",
                "end_turn",
                "unit_panel",
                "unit_rename",
                "unit_lists",
                "minimap",
                "on_map",
                "hud-timer",
                "hud-chat",
                "production",
                "production-sorted",
                "research",
                "techs",
                "techs--free",
                "techs--steal",
                "tech_award",
                "rich_tooltips",
                "tech_panel",
                "tech_panel--done",
                "city",
                "city_list",
                "city_rename",
                "city_captured",
                "free_great_person",
                "city--puppet",
                "city--sell",
                "city_rename--invalid",
                "city_rename--raze",
                "city_rename--unraze",
                "city_rename--annex",
                "city_captured--annex",
                "free_great_person--faith",
                "free_great_person--maya",
                "policies",
                "ideology",
                "pantheon",
                "religion",
                "religion_overview",
                "diplo",
                "declare_war",
                "declare_war--plunder",
                "civ_ribbon",
                "deal",
                "leader",
                "diplo--global",
                "diplo--deals",
                "diplo--empty",
                "declare_war--citystate",
                "civ_ribbon--host",
                "deal--player",
                "deal--player-incoming",
                "economy",
                "military",
                "trade_routes",
                "espionage",
                "victory",
                "demographics",
                "demographics-whos-winning",
                "economy-happiness",
                "trade_routes-choose",
                "trade_routes-home",
                "trade_routes-port",
                "culture",
                "great_work",
                "archaeology",
                "archaeology--writing",
                "city_state",
                "city_state--enter",
                "city_state--liberate",
                "congress",
                "congress--propose",
                "congress--splash",
                "congress--project",
                "congress--empty",
                "mp_vote",
                "mp_vote--result",
                "mp_vote--concede",
                "mp_vote--diplo",
                "confirm",
                "message",
                "goody_choice",
                "notification_log",
                "game_menu",
                "end_game",
                "advisor",
            ]
        );
        assert_eq!(skin.verify.golden[0].viewport, [1920, 1080]);
    }

    #[test]
    fn state_is_flattened_and_typed() {
        let text = format!(
            "{MIN}\n[state]\n\"windows.production\" = false\nprod = {{ sort = \"name\", page = 2 }}\nzoom = 1.5\n"
        );
        let skin = Skin::parse(&text).expect("parses");
        let got: Vec<(String, TypeSig)> = skin
            .state
            .iter()
            .map(|(k, v)| (k.clone(), v.type_sig()))
            .collect();
        assert_eq!(
            got,
            [
                ("prod.page".to_string(), TypeSig::Int),
                ("prod.sort".to_string(), TypeSig::Str),
                ("windows.production".to_string(), TypeSig::Bool),
                ("zoom".to_string(), TypeSig::Float),
            ]
        );
    }

    #[test]
    fn bad_manifests_are_reported() {
        let errs = Skin::parse(&format!("{MIN}\n[state]\nlist = []\n")).unwrap_err();
        assert_eq!(errs[0].code, codes::STATE_TYPE);
        for name in ["Zoom", "_zoom", "a.B", "1x", "a-b", "a..b"] {
            let errs = Skin::parse(&format!("{MIN}\n[state]\n\"{name}\" = 1\n")).unwrap_err();
            assert_eq!(errs[0].code, codes::SKIN_TOML, "{name}");
        }
        let errs = Skin::parse(&MIN.replace("id = \"t\"", "id = \"T x\"")).unwrap_err();
        assert_eq!(errs[0].code, codes::SKIN_TOML);
        let errs = Skin::parse(&format!("{MIN}\n[skin2]\n")).unwrap_err();
        assert_eq!(errs[0].code, codes::SKIN_TOML);
        assert!(errs[0].place.line > 0);
        let errs = Skin::parse(&format!("{MIN}\n[themes]\ndawn = \"x\"\n")).unwrap_err();
        assert_eq!(errs[0].code, codes::SKIN_TOML);
        let errs = Skin::parse(&format!("{MIN}\n[fonts]\nsubset = [\"greek\"]\n")).unwrap_err();
        assert_eq!(errs[0].code, codes::SKIN_TOML);
    }
}
