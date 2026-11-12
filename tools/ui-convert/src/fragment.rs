//! Fragment mode: the panels of a mod (`mod.toml [ui]`) as one package of kind `Fragment`.
//!
//! ```text
//! ui2-convert fragment <mod dir> --out <dir>
//!   mod.toml [ui] ─► the page around the panels (virtual files, served next to the panel pages)
//!     ─► the skin pipeline (scan, checks, plan, extract, styles, bindings) with the rules of a fragment
//!     ─► pictures, scripts ─► <id>-<version>-<hash16>.ui2 in <dir>; stdout: `ui2-fragment <file> <blake3>`
//! ```
//!
//! **The page.** The converter writes a page of its own into the panels' folder (virtual: the converter's
//! server serves it, nothing is written to the mod): every panel page is a `data-include` of its body, in the
//! order of `[[ui.panels]]`, and a stylesheet before them defines the color tokens of the catalogue (`--ui-*`)
//! with reference values and gives the page the skin's text color and the `ui-sans` font role. A panel page
//! links its own stylesheets beside it; because the page sits in the same folder, Chrome and the converter
//! read the same files. All panel pages of a mod are in one folder.
//!
//! **What the package holds.** Template 0 has one top-level node per panel: the root element of its page,
//! named (`data-part`) by the panel id, which is how the runtime finds a panel. Colors that come from the
//! tokens are token references by name, so the runtime paints them with the active skin's palette; the
//! families `ui-sans` and `ui-display` become the skin's faces. There are no fonts and no text tables: a mod
//! panel writes its text in the markup or binds it. The mod's scripts (`[ui] scripts`) and `[ui] reads` go
//! into `SCRP`, and so does the model fragment (`[ui] model`) under [`MODEL_FILE`] (the client mounts the
//! mod's fields from it). Expressions read the shared model, the mod's own fields `mods.<id>.*`, and send the
//! intents of the catalogue and `mods.<id>.*`.
//!
//! **The rules of a fragment** (on top of the profile): one root element per panel page (E7-008); no
//! layers, screens or extension points; classes start with `m-<id>-` (E7-009); `--ui-*` tokens are read, not
//! defined, and only those of the catalogue (E7-008); text uses the font roles and no `@font-face` (E5-009);
//! at most 1 MB per fragment and 256 KiB of pixels per picture (E8-002). Two things build but do not show as
//! written yet, so they are warnings: the runtime does not draw a fragment's pictures, and it uses a
//! fragment's breakpoints only when the skin has the same ones (otherwise the first viewport cell).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use mod_api::UI_CATALOG;
use mod_manifest::{Manifest, UiStateValue, Version};
use ui2_model::{Fragment, ModelStore, Namespace, Schema};
use ui2_package::profile::{SchemaView, TypeSig};

use crate::check::Statics;
use crate::errors::{self, ConvError, Place, codes};
use crate::ir::{CssItem, Dom, Element, Ir, Ix};
use crate::skin::{Fonts, Scripts, Skin, SkinSection, StateValue, Themes, Verify, is_skin_path};
use crate::styles::Compiled;
use crate::write::{Pictures, Written};
use crate::{Input, Opts, Run, Source, check_source};

/// The page the converter makes around the panels, in the panels' folder.
pub const ENTRY_NAME: &str = "__ui2-fragment.html";
/// The stylesheet of the page: the color tokens and the defaults of a panel.
pub const TOKENS_NAME: &str = "__ui2-tokens.css";
/// The most bytes of a fragment package.
pub const MAX_BYTES: u64 = 1_000_000;
/// The most bytes of one picture's pixels, at both scales together.
pub const MAX_PICTURE_BYTES: u64 = 256 * 1024;

/// The reference values of the catalogue's color tokens: what the converter's page shows. In the game every
/// token is the active skin's color of the same name.
pub const REFERENCE_TOKENS: &[(&str, &str)] = &[
    ("--ui-surface", "#1b1815"),
    ("--ui-surface-2", "#26221d"),
    ("--ui-line", "#4d4032"),
    ("--ui-text", "#ebe2d2"),
    ("--ui-text-2", "#b2a794"),
    ("--ui-text-3", "#8a8072"),
    ("--ui-accent", "#c9a36a"),
    ("--ui-accent-ink", "#1b1815"),
    ("--ui-negative", "#cf6f5c"),
    ("--ui-y-food", "#86b061"),
    ("--ui-y-prod", "#c78a52"),
    ("--ui-y-gold", "#dcc057"),
    ("--ui-y-sci", "#78a6d6"),
    ("--ui-y-cult", "#a98ccc"),
    ("--ui-y-faith", "#cfcbc1"),
    ("--ui-y-happy", "#b9c662"),
    ("--ui-y-tour", "#cc86b4"),
];

/// The name the mod's model fragment (`[ui] model`) is stored under among the fragment's script files: no script
/// file can have it (a `:` is not in a relative path), and the client mounts it at `mods.<id>` instead of running it
/// (`ui2_host::mods::MODEL_FILE`).
pub const MODEL_FILE: &str = "ui2:model.toml";

/// The font roles a mod panel uses.
pub const FONT_ROLES: &[&str] = &["ui-sans", "ui-display"];

/// One `[[ui.panels]]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelSpec {
    /// The panel id (the `data-part` of its root in the package).
    pub id: String,
    /// The panel page, relative to the mod folder.
    pub source: String,
    /// The extension point.
    pub point: String,
}

/// The `[ui]` of a mod, as the converter uses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentSpec {
    /// The mod id (the package id).
    pub mod_id: String,
    /// The mod version.
    pub version: String,
    /// `[ui] api` as written.
    pub api: String,
    /// `[ui] model`, relative to the mod folder.
    pub model: Option<String>,
    /// The panels in order.
    pub panels: Vec<PanelSpec>,
    /// The folder of the panel pages, relative to the mod folder (empty for the mod folder itself).
    pub folder: String,
}

fn in_folder(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_string()
    } else {
        format!("{folder}/{name}")
    }
}

impl FragmentSpec {
    /// The page around the panels.
    pub fn entry(&self) -> String {
        in_folder(&self.folder, ENTRY_NAME)
    }

    /// The stylesheet of the page.
    pub fn tokens_file(&self) -> String {
        in_folder(&self.folder, TOKENS_NAME)
    }

    /// The extension points of the panels, each once, in panel order.
    pub fn points(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for p in &self.panels {
            if !out.contains(&p.point) {
                out.push(p.point.clone());
            }
        }
        out
    }

    /// The prefix of the mod's classes.
    pub fn class_prefix(&self) -> String {
        format!("m-{}-", self.mod_id)
    }

    /// The files the converter's server serves for the fragment: the page and its stylesheet.
    pub fn virtual_files(&self) -> BTreeMap<String, String> {
        let mut page =
            String::from("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
        page.push_str(&format!(
            "<link rel=\"stylesheet\" href=\"{TOKENS_NAME}\">\n</head>\n<body>\n"
        ));
        for p in &self.panels {
            page.push_str(&format!("<div data-include=\"{}\"></div>\n", p.source));
        }
        page.push_str("</body>\n</html>\n");
        let mut css = String::from(
            "/* The color tokens a mod reads, with reference values; in the game they are the active skin's. */\n:root {\n",
        );
        for (name, value) in REFERENCE_TOKENS {
            css.push_str(&format!("  {name}: {value};\n"));
        }
        css.push_str("}\n\n/* A panel starts with the skin's text color and sans face. */\nhtml {\n  color: var(--ui-text);\n  font-family: ui-sans;\n}\n");
        BTreeMap::from([(self.entry(), page), (self.tokens_file(), css)])
    }
}

fn manifest_error(why: impl Into<String>) -> ConvError {
    ConvError::new(codes::SKIN_TOML, Place::file("mod.toml"), why)
}

fn fragment_error(place: Place, why: impl Into<String>) -> ConvError {
    ConvError::new(codes::FRAGMENT, place, why)
}

/// `[a-z][a-z0-9-]{0,31}`: the ids a package may have.
fn package_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// The lowest `major.minor` that `[ui] api` accepts (the manifest's API version of the fragment).
fn api_pair(req: &mod_manifest::VersionReq) -> (u16, u16) {
    for major in 0..64u16 {
        for minor in 0..64u16 {
            if req.matches(&Version::new(major.into(), minor.into(), 0)) {
                return (major, minor);
            }
        }
    }
    (0, 0)
}

/// Reads `mod.toml` of a mod folder and makes the source of its fragment.
pub fn load(dir: &Path) -> Result<Source, Vec<ConvError>> {
    let text = std::fs::read_to_string(dir.join("mod.toml")).map_err(|_| {
        vec![ConvError::new(
            codes::SKIN_DIR,
            Place::file("mod.toml"),
            "the folder has no mod.toml",
        )]
    })?;
    let m = Manifest::parse(&text.replace("\r\n", "\n"))
        .map_err(|e| vec![manifest_error(e.to_string())])?;
    let Some(ui) = &m.ui else {
        return Err(vec![ConvError::new(
            codes::SKIN_DIR,
            Place::file("mod.toml"),
            "the mod has no [ui] section",
        )]);
    };
    let mut errs = Vec::new();
    let id = m.id.as_str().to_string();
    if !package_id(&id) {
        errs.push(manifest_error(format!(
            "the mod id `{id}` cannot name a package: lower case letters, digits and -, at most 32"
        )));
    }
    if let Err(e) = mod_api::check_ui_points(ui) {
        errs.push(fragment_error(Place::file("mod.toml"), e.to_string()));
    }
    let mut folders = BTreeSet::new();
    for (i, p) in ui.panels.iter().enumerate() {
        if !is_skin_path(&p.source) || !p.source.ends_with(".html") {
            errs.push(fragment_error(
                Place::file("mod.toml"),
                format!(
                    "ui.panels[{i}].source `{}` is not an .html file of the mod",
                    p.source
                ),
            ));
            continue;
        }
        if !dir.join(&p.source).is_file() {
            errs.push(ConvError::new(
                codes::INCLUDE_MISSING,
                Place::file("mod.toml"),
                format!("ui.panels[{i}].source: `{}` does not exist", p.source),
            ));
        }
        folders.insert(p.source.rsplit_once('/').map_or("", |(f, _)| f).to_string());
    }
    if folders.len() > 1 {
        let list: Vec<&str> = folders.iter().map(String::as_str).collect();
        errs.push(fragment_error(
            Place::file("mod.toml"),
            format!(
                "the panel pages are in the folders {}; keep them in one",
                list.join(", ")
            ),
        ));
    }
    if !errs.is_empty() {
        errors::sort(&mut errs);
        return Err(errs);
    }
    let spec = FragmentSpec {
        mod_id: id.clone(),
        version: m.version.to_string(),
        api: ui.api.to_string(),
        model: ui.model.clone(),
        panels: ui
            .panels
            .iter()
            .map(|p| PanelSpec {
                id: p.id.clone(),
                source: p.source.clone(),
                point: p.point.clone(),
            })
            .collect(),
        folder: folders.into_iter().next().unwrap_or_default(),
    };
    let model = Schema::v1().version();
    let api = api_pair(&ui.api);
    let skin = Skin {
        skin: SkinSection {
            id,
            version: spec.version.clone(),
            title: String::new(),
            entry: spec.entry(),
            model: format!("{}.{}", model.major, model.minor),
            api: format!("{}.{}", api.0, api.1),
            default: false,
            extends: None,
        },
        fonts: Fonts::default(),
        // `[ui] state`: the mod's own state, `state.*` in its panels (`modstate.<id>.*` in the game).
        state: ui
            .state
            .iter()
            .map(|(path, v)| {
                let v = match v {
                    UiStateValue::Bool(b) => StateValue::Bool(*b),
                    UiStateValue::Int(i) => StateValue::Int(*i),
                    UiStateValue::Str(s) => StateValue::Str(s.clone()),
                };
                (path.clone(), v)
            })
            .collect(),
        themes: Themes::default(),
        scripts: Scripts {
            files: ui.scripts.clone(),
            reads: ui.reads.clone(),
        },
        verify: Verify::default(),
        fragment: Some(spec),
    };
    Ok(Source {
        dir: dir.to_path_buf(),
        skin,
    })
}

/// The top-level elements of the page body: the panel roots (everything that is a node).
fn tops(dom: &Dom) -> Vec<Ix> {
    let Some(body) = dom.body() else {
        return Vec::new();
    };
    (0..dom.elems.len())
        .filter(|&ix| dom.parent[ix] == Some(body) && !crate::numbering::skipped(dom.elems[ix]))
        .collect()
}

/// The root element of each panel, with the panel id, when the page has one root per panel in panel order.
pub fn panel_roots(dom: &Dom, f: &FragmentSpec) -> Vec<(Ix, String)> {
    let t = tops(dom);
    let matches = t.len() == f.panels.len()
        && t.iter()
            .zip(&f.panels)
            .all(|(&ix, p)| dom.elems[ix].place().file == p.source);
    if !matches {
        return Vec::new();
    }
    t.into_iter()
        .zip(&f.panels)
        .map(|(ix, p)| (ix, p.id.clone()))
        .collect()
}

fn at_element(dom: &Dom, ix: Ix, code: &str, prop: &str, why: String) -> ConvError {
    let e: &Element = dom.elems[ix];
    ConvError::new(code, e.place(), why)
        .element(e.describe())
        .prop(prop)
}

/// The names of the custom properties a value reads (`var(--a)`, `var(--b, ...)`).
fn var_names(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = value;
    while let Some(at) = rest.find("var(") {
        let after = rest[at + 4..].trim_start();
        let end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .unwrap_or(after.len());
        if after.starts_with("--") {
            out.push(after[..end].to_string());
        }
        rest = &after[end..];
    }
    out
}

/// The `@font-face` rules and the declarations of the mod's own stylesheets (not the page's).
fn mod_sheet_items<'a>(ir: &'a Ir, f: &FragmentSpec) -> Vec<(String, &'a CssItem)> {
    fn walk<'a>(file: &str, list: &'a [CssItem], out: &mut Vec<(String, &'a CssItem)>) {
        for item in list {
            out.push((file.to_string(), item));
            match item {
                CssItem::Rule { items, .. } => walk(file, items, out),
                CssItem::Atrule {
                    items: Some(items), ..
                } => walk(file, items, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    for sheet in &ir.sheets {
        let file = sheet.source_file();
        if file == f.tokens_file() {
            continue;
        }
        walk(&file, &sheet.rules, &mut out);
    }
    out
}

/// The rules of a fragment that need only the scan (see the module docs).
pub fn static_checks(f: &FragmentSpec, dom: &Dom, statics: &Statics, ir: &Ir) -> Vec<ConvError> {
    let mut errs = Vec::new();

    // One root element per panel page, in panel order, named by the panel when it carries a name.
    let t = tops(dom);
    if panel_roots(dom, f).is_empty() {
        for p in &f.panels {
            let n = t
                .iter()
                .filter(|&&ix| dom.elems[ix].place().file == p.source)
                .count();
            let same = f.panels.iter().filter(|q| q.source == p.source).count();
            if n != same {
                errs.push(fragment_error(
                    Place::file(&p.source),
                    format!(
                        "panel `{}`: the page has {} root element(s); a panel page is one element (and the <link> of its stylesheets)",
                        p.id,
                        n / same.max(1)
                    ),
                ));
            }
        }
        if errs.is_empty() {
            errs.push(fragment_error(
                Place::file(&f.entry()),
                "the panel pages do not give one root element per panel".to_string(),
            ));
        }
    }
    for &ix in &t {
        let file = dom.elems[ix].place().file;
        let mut of = f.panels.iter().filter(|p| p.source == file);
        let (Some(p), None) = (of.next(), of.next()) else {
            continue;
        };
        let panel = &p.id;
        if let Some(part) = dom.elems[ix].attr("data-part")
            && part != panel
        {
            errs.push(at_element(
                dom,
                ix,
                codes::FRAGMENT,
                "data-part",
                format!(
                    "the root of panel `{panel}` is named `{part}`; name it `{panel}` or not at all"
                ),
            ));
        }
    }

    // No layers, screens or extension points; classes with the mod's prefix.
    let prefix = f.class_prefix();
    for &ix in &statics.ui {
        let e = dom.elems[ix];
        for attr in ["data-layer", "data-screen", "data-slot"] {
            if e.attr(attr).is_some() {
                errs.push(at_element(
                    dom,
                    ix,
                    codes::FRAGMENT,
                    attr,
                    format!("a mod panel has no `{attr}`"),
                ));
            }
        }
        for class in e.classes() {
            if !class.starts_with(&prefix) {
                errs.push(at_element(
                    dom,
                    ix,
                    codes::MOD_CLASS,
                    "class",
                    format!("`{class}` does not start with `{prefix}`"),
                ));
            }
        }
    }
    for (class, owners) in &statics.dyn_classes {
        if !class.starts_with(&prefix)
            && let Some(&ix) = owners.iter().next()
        {
            errs.push(at_element(
                dom,
                ix,
                codes::MOD_CLASS,
                "data-bind-class-toggle",
                format!("`{class}` does not start with `{prefix}`"),
            ));
        }
    }
    for rule in &statics.rules {
        if rule.place.file == f.tokens_file() {
            continue;
        }
        let mut bad: BTreeSet<&str> = BTreeSet::new();
        for sel in &rule.selectors {
            for (_, compound) in &sel.static_part {
                for class in &compound.classes {
                    if !class.starts_with(&prefix) {
                        bad.insert(class);
                    }
                }
            }
        }
        for class in bad {
            errs.push(
                ConvError::new(
                    codes::MOD_CLASS,
                    rule.place.clone(),
                    format!("`.{class}` does not start with `{prefix}`"),
                )
                .element(format!("`{}`", rule.text)),
            );
        }
    }

    // The color tokens are read, not defined; only those of the catalogue.
    let own: BTreeSet<&str> = statics
        .root_vars
        .iter()
        .filter(|v| v.place.file != f.tokens_file())
        .map(|v| v.name.as_str())
        .collect();
    for v in &statics.root_vars {
        if v.place.file != f.tokens_file() && v.name.starts_with("--ui-") {
            errs.push(
                ConvError::new(
                    codes::FRAGMENT,
                    v.place.clone(),
                    format!(
                        "`{}` is a color of the skin; a mod reads it, it does not define it",
                        v.name
                    ),
                )
                .prop(v.name.clone()),
            );
        }
    }
    for (file, item) in mod_sheet_items(ir, f) {
        match item {
            CssItem::Atrule {
                name, line, col, ..
            } if name.eq_ignore_ascii_case("font-face") => errs.push(
                ConvError::new(
                    codes::MOD_FONT,
                    Place::at(&file, *line, *col),
                    "a mod brings no fonts: its text uses the skin's faces by role",
                )
                .element("`@font-face`"),
            ),
            CssItem::Decl {
                prop,
                value,
                line,
                col,
                ..
            } => {
                for name in var_names(value) {
                    if name.starts_with("--ui-")
                        && !UI_CATALOG.has_token(&name)
                        && !own.contains(name.as_str())
                    {
                        errs.push(
                            ConvError::new(
                                codes::FRAGMENT,
                                Place::at(&file, *line, *col),
                                format!("`{name}` is not a color token of the catalogue"),
                            )
                            .prop(prop.clone()),
                        );
                    }
                }
            }
            _ => {}
        }
    }
    errs
}

/// Whether an element shows text: a text child, a bound value or a text key, or a text field.
fn shows_text(e: &Element) -> bool {
    e.tag == "input"
        || e.attr("data-bind-value").is_some()
        || e.attr("data-l10n").is_some()
        || e.children
            .iter()
            .any(|c| matches!(c, crate::ir::Node::Text(t) if !t.ws))
}

/// The rules of a fragment on the compiled styles: text uses a font role (E5-009). Warnings: breakpoints.
pub fn style_checks(f: &FragmentSpec, dom: &Dom, c: &Compiled) -> (Vec<ConvError>, Vec<String>) {
    let mut errs = Vec::new();
    for (&id, &set) in &c.node_set {
        let Some(&ix) = dom.by_id.get(&id) else {
            continue;
        };
        if !shows_text(dom.elems[ix]) {
            continue;
        }
        let Some(s) = c.sets.get(set) else { continue };
        let bad: BTreeSet<String> = s
            .table
            .iter()
            .filter_map(|r| c.records.get(r.index()))
            .filter_map(|r| {
                let first = r.font_family.first().map(|s| c.strs.get(*s)).unwrap_or("");
                (!FONT_ROLES.contains(&first)).then(|| first.to_string())
            })
            .collect();
        if let Some(family) = bad.into_iter().next() {
            errs.push(at_element(
                dom,
                ix,
                codes::MOD_FONT,
                "font-family",
                format!(
                    "the text is set in `{family}`; a mod panel uses `ui-sans` or `ui-display`"
                ),
            ));
        }
    }
    let mut warnings = Vec::new();
    if !c.media_w.is_empty() || !c.media_h.is_empty() {
        warnings.push(format!(
            "mod {}: the panels have breakpoints; the runtime uses them only with a skin that has the same ones, and the first viewport cell with any other",
            f.mod_id
        ));
    }
    (errs, warnings)
}

/// The fragment budget (E8-002): the package and every picture.
pub fn budget(w: &Written, pictures: &Pictures) -> Vec<ConvError> {
    let mut errs = Vec::new();
    let size = w.bytes.len() as u64;
    if size > MAX_BYTES {
        errs.push(ConvError::new(
            codes::FRAGMENT_BUDGET,
            Place::default(),
            format!("the fragment is {size} bytes, at most {MAX_BYTES}"),
        ));
    }
    for p in &pictures.imgs {
        let format_bytes = |page: u16| -> u64 {
            match pictures
                .pages
                .get(usize::from(page))
                .map(|(pg, _)| pg.format)
            {
                Some(ui2_package::sections::blobs::PageFormat::A8) => 1,
                _ => 4,
            }
        };
        let area = |s: &ui2_package::Sprite| {
            u64::from(s.rect[2]) * u64::from(s.rect[3]) * format_bytes(s.page)
        };
        let bytes = area(&p.x1) + p.x2.as_ref().map_or(0, area);
        if bytes > MAX_PICTURE_BYTES {
            errs.push(
                ConvError::new(
                    codes::FRAGMENT_BUDGET,
                    Place::file(&p.path),
                    format!("the picture is {bytes} bytes of pixels at both scales, at most {MAX_PICTURE_BYTES}"),
                )
                .element(p.path.clone()),
            );
        }
    }
    errs
}

/// The model the markup of a mod reads: the shared model, and the mod's own fields under `mods.<id>`.
pub struct ModView {
    base: &'static Schema,
    id: String,
    own: Option<&'static Schema>,
}

/// The type name `mods` stands for in a mod's expressions.
const MODS_ROOT: &str = "mods";

impl ModView {
    /// The view of a mod: its `[ui] model` (a model fragment) mounted at `mods.<id>`.
    pub fn new(dir: &Path, f: &FragmentSpec) -> Result<ModView, ConvError> {
        let base = Schema::v1();
        let own = match &f.model {
            None => None,
            Some(rel) => {
                let text = std::fs::read_to_string(dir.join(rel)).map_err(|e| {
                    fragment_error(
                        Place::file(rel),
                        format!("cannot read the model fragment: {e}"),
                    )
                })?;
                let frag = Fragment::parse(&text.replace("\r\n", "\n")).map_err(|e| {
                    fragment_error(Place::file(rel), format!("the model fragment: {e}"))
                })?;
                let ns = Namespace::Mod(f.mod_id.as_str().into());
                let mut store = ModelStore::new(base);
                store.mount(ns.clone(), &frag).map_err(|e| {
                    fragment_error(Place::file(rel), format!("the model fragment: {e}"))
                })?;
                store.mount_schema(&ns)
            }
        };
        Ok(ModView {
            base,
            id: f.mod_id.clone(),
            own,
        })
    }

    fn own_root(&self) -> String {
        format!("{MODS_ROOT}.{}", self.id)
    }
}

impl SchemaView for ModView {
    fn root(&self, name: &str) -> Option<TypeSig> {
        if name == MODS_ROOT && self.own.is_some() {
            return Some(TypeSig::Struct(MODS_ROOT.to_string()));
        }
        SchemaView::root(self.base, name)
    }

    fn field(&self, of: &TypeSig, name: &str) -> Option<TypeSig> {
        if matches!(of, TypeSig::Struct(s) if s == MODS_ROOT) {
            // Only the mod's own fields: another mod's are not in its view.
            return if name == self.id {
                SchemaView::root(self.own?, &self.own_root())
            } else {
                None
            };
        }
        self.base
            .field(of, name)
            .or_else(|| self.own?.field(of, name))
    }

    fn list_key(&self, list: &TypeSig) -> Option<&str> {
        self.base
            .list_key(list)
            .or_else(|| self.own.and_then(|o| o.list_key(list)))
    }

    fn intent(&self, name: &str) -> Option<Vec<(String, TypeSig)>> {
        self.base.intent(name)
    }

    fn js_writable(&self, path: &str) -> bool {
        self.base.js_writable(path)
    }

    fn version(&self) -> (u16, u16) {
        SchemaView::version(self.base)
    }
}

/// What a fragment build gave.
#[derive(Debug, Default)]
pub struct FragmentOut {
    /// The run, with every error.
    pub run: Run,
    /// The package, when it was written.
    pub written: Option<Written>,
    /// The package file, when it was written.
    pub path: Option<PathBuf>,
}

/// Builds the fragment of the mod in `dir` and writes `<id>-<version>-<hash16>.ui2` into `out`. The scan and the
/// errors of the run go to `target/ui2/mods/<id>/` (`ir.json`, `errors.json`).
pub fn build(dir: &Path, out: &Path) -> FragmentOut {
    let src = match load(dir) {
        Ok(s) => s,
        Err(errors) => {
            return FragmentOut {
                run: Run {
                    errors,
                    ..Run::default()
                },
                ..FragmentOut::default()
            };
        }
    };
    let id = src.skin.skin.id.clone();
    let mut run = check_source(src, Input::Chrome, &Opts::default());
    let (mut scr, e) = crate::build::scripts(dir, &run);
    run.errors.extend(e);
    // The model fragment travels with the package: the client mounts its fields from it.
    if let Some(model) = run
        .source
        .as_ref()
        .and_then(|s| s.skin.fragment.as_ref())
        .and_then(|f| f.model.clone())
    {
        match std::fs::read_to_string(dir.join(&model)) {
            Ok(text) => scr.push((
                MODEL_FILE.to_string(),
                text.replace("\r\n", "\n").into_bytes(),
            )),
            Err(e) => run.errors.push(fragment_error(
                Place::file(&model),
                format!("cannot read the model fragment: {e}"),
            )),
        }
    }
    let pictures = if run.errors.is_empty() {
        match crate::build::pictures_of(&run, None) {
            Ok(p) => p,
            Err(e) => {
                run.errors.extend(e);
                Pictures::default()
            }
        }
    } else {
        Pictures::default()
    };
    if !pictures.imgs.is_empty() {
        run.warnings.push(format!(
            "mod {id}: {} picture(s) are in the fragment, but the runtime does not draw a fragment's pictures yet",
            pictures.imgs.len()
        ));
    }
    let mut written = None;
    if run.errors.is_empty() {
        match crate::build::package_of(&run, &[], &pictures, &scr) {
            Ok(w) => {
                let e = budget(&w, &pictures);
                if e.is_empty() {
                    written = Some(w);
                } else {
                    run.errors.extend(e);
                }
            }
            Err(e) => run.errors.extend(e),
        }
    }
    errors::sort(&mut run.errors);
    let mut path = None;
    let logs = crate::build::target_dir()
        .join("ui2")
        .join("mods")
        .join(&id);
    let saved = std::fs::create_dir_all(&logs)
        .and_then(|()| std::fs::write(logs.join("errors.json"), errors::to_json(&run.errors)))
        .and_then(|()| match run.dump() {
            Some(d) => std::fs::write(logs.join("ir.json"), d),
            None => Ok(()),
        });
    if let Err(e) = saved {
        run.errors.push(ConvError::new(
            codes::WRITE,
            Place::default(),
            format!("cannot write the build notes: {e}"),
        ));
    }
    if let Some(w) = &written {
        let p = out.join(&w.file_name);
        match std::fs::create_dir_all(out).and_then(|()| std::fs::write(&p, &w.bytes)) {
            Ok(()) => path = Some(p),
            Err(e) => run.errors.push(ConvError::new(
                codes::WRITE,
                Place::default(),
                format!("cannot write the fragment: {e}"),
            )),
        }
    }
    FragmentOut {
        run,
        written: written.filter(|_| path.is_some()),
        path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_tokens_are_the_catalogue() {
        let names: Vec<&str> = REFERENCE_TOKENS.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, UI_CATALOG.tokens);
    }

    #[test]
    fn var_names_are_found() {
        assert_eq!(
            var_names("1px solid var(--ui-line) , var( --x-y,red)"),
            ["--ui-line", "--x-y"]
        );
        assert!(var_names("red").is_empty());
    }

    #[test]
    fn api_requirements_give_their_lowest_version() {
        let req = |s: &str| s.parse::<mod_manifest::VersionReq>().expect("req");
        assert_eq!(api_pair(&req("^1")), (1, 0));
        assert_eq!(api_pair(&req("^1.2")), (1, 2));
        assert_eq!(api_pair(&req(">=2.1, <3")), (2, 1));
    }
}
