//! `[ui]`: the panels, model fields and scripts a mod adds to the UI v2 (mod API 1.3).
//!
//! ```toml
//! [ui]
//! api = "^1"                    # the UI API (script API and point catalogue) the panels need
//! model = "ui/model.toml"       # fields mods.<id>.* that the Rust half writes
//! reads = ["game.turn"]         # paths of the shared model the panels and the script read
//! scripts = ["ui/hello.js"]     # the mod's script context
//!
//! [[ui.panels]]
//! id = "echo"
//! source = "ui/echo.html"
//! point = "hud.right"
//! mode = "append"               # append | replace
//! fallback = "mods"             # mods | none
//! ```
//!
//! Only the shape is checked here. Which points exist is the business of the point catalogue (`mod-api`), whether
//! the files exist that of the build tool, and whether the read paths exist that of the converter.

use std::collections::BTreeSet;
use std::fmt;

use toml::Value;

use crate::error::ManifestError;
use crate::manifest::{ModId, rel_path};
use crate::reader::{Res, Tbl, wrong_type};
use crate::semver::VersionReq;

/// The most panels one mod brings.
pub const UI_PANELS_MAX: usize = 16;
/// The most script files of one mod.
pub const UI_SCRIPTS_MAX: usize = 8;
/// The most shared model paths one mod reads.
pub const UI_READS_MAX: usize = 32;

/// How a panel joins its point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UiMode {
    /// After the skin's own children and the panels of earlier mods.
    Append,
    /// Instead of the skin's element; two mods replacing one point is a conflict of the set.
    Replace,
}

impl UiMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Append => "append",
            Self::Replace => "replace",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "append" => Some(Self::Append),
            "replace" => Some(Self::Replace),
            _ => None,
        }
    }
}

/// Where a panel goes when the active skin does not have its point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum UiFallback {
    /// Into the shared `mods` slot every skin has.
    #[default]
    Mods,
    /// Nowhere: the panel is not shown.
    None,
}

impl UiFallback {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mods => "mods",
            Self::None => "none",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "mods" => Some(Self::Mods),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

/// One `[[ui.panels]]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UiPanel {
    /// `^[a-z][a-z0-9-]{0,31}$`, unique in the mod.
    pub id: String,
    /// The fragment page, relative to the mod root.
    pub source: String,
    /// The id of an extension point (`[a-z][a-z0-9.-]{1,63}`).
    pub point: String,
    pub mode: UiMode,
    pub fallback: UiFallback,
}

/// The `[ui]` section of a manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UiSection {
    /// Requirement on the UI API version (the script API and the point catalogue), checked by the client.
    pub api: VersionReq,
    /// The model fragment (`mods.<id>.*`), relative to the mod root.
    pub model: Option<String>,
    /// Shared model paths the panels and the script read.
    pub reads: Vec<String>,
    /// The script files of the mod's script context, in order.
    pub scripts: Vec<String>,
    /// The panels, in the order they are mounted.
    pub panels: Vec<UiPanel>,
}

impl UiSection {
    /// Every file the section names, with the key path that names it: the model, the scripts, the panel sources.
    pub fn files(&self) -> Vec<(String, &str)> {
        let mut out = Vec::new();
        if let Some(m) = &self.model {
            out.push(("ui.model".to_string(), m.as_str()));
        }
        for (i, s) in self.scripts.iter().enumerate() {
            out.push((format!("ui.scripts[{i}]"), s.as_str()));
        }
        for (i, p) in self.panels.iter().enumerate() {
            out.push((format!("ui.panels[{i}].source"), p.source.as_str()));
        }
        out
    }
}

/// Two mods of a set replace the same point. `first` loads before `second`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiConflict {
    pub point: String,
    pub first: ModId,
    pub second: ModId,
}

impl fmt::Display for UiConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "mods `{}` and `{}` both replace the interface element `{}`; keep one of them",
            self.first, self.second, self.point
        )
    }
}

/// The replace conflicts of a set, given in load order: for every point replaced by more than one mod, the first
/// replacer against each later one. Appending to a point never conflicts. The result is ordered by the later mod,
/// then by point.
pub fn ui_conflicts(mods: &[(&ModId, &UiSection)]) -> Vec<UiConflict> {
    let mut first: Vec<(&str, &ModId)> = Vec::new();
    let mut out = Vec::new();
    for (id, ui) in mods {
        let mut points: Vec<&str> = ui
            .panels
            .iter()
            .filter(|p| p.mode == UiMode::Replace)
            .map(|p| p.point.as_str())
            .collect();
        points.sort_unstable();
        points.dedup();
        for point in points {
            match first.iter().find(|(p, _)| *p == point) {
                Some((_, holder)) => out.push(UiConflict {
                    point: point.to_string(),
                    first: (*holder).clone(),
                    second: (*id).clone(),
                }),
                None => first.push((point, id)),
            }
        }
    }
    out
}

/// `^[a-z][a-z0-9-]{0,31}$`.
fn valid_panel_id(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// `[a-z][a-z0-9.-]{1,63}`.
pub(crate) fn valid_point(s: &str) -> bool {
    let b = s.as_bytes();
    (2..=64).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'.' || *c == b'-')
}

/// `[a-z][a-z0-9_]*(\.[a-z0-9_*]+)*`.
fn valid_read(s: &str) -> bool {
    let mut segs = s.split('.');
    let head_ok = segs.next().is_some_and(|h| {
        let mut c = h.chars();
        c.next().is_some_and(|f| f.is_ascii_lowercase())
            && c.all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || x == '_')
    });
    head_ok
        && segs.all(|seg| {
            !seg.is_empty()
                && seg
                    .chars()
                    .all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || x == '_' || x == '*')
        })
}

/// A relative path to one file: the `rel_path` rule without glob characters and without a trailing `/`.
fn file_path(path: String, s: &str) -> Res<String> {
    let s = rel_path(path.clone(), s)?;
    if s.ends_with('/') || s.contains(['*', '?', '[', ']']) {
        return Err(ManifestError::invalid(
            path,
            "expected the path of one file (no glob characters, no trailing '/')",
        ));
    }
    Ok(s)
}

fn enum_value<T>(
    t: &mut Tbl<'_>,
    key: &'static str,
    parse: fn(&str) -> Option<T>,
    allowed: &str,
) -> Res<Option<T>> {
    let path = t.path_of(key);
    match t.str_opt(key)? {
        None => Ok(None),
        Some(s) => parse(s)
            .map(Some)
            .ok_or_else(|| ManifestError::invalid(path, format!("{s:?} is not one of {allowed}"))),
    }
}

fn parse_panel(item: &Value, path: &str) -> Res<UiPanel> {
    let mut t = Tbl::from_value(item, path)?;
    let id_path = t.path_of("id");
    let id = t.str_req("id")?;
    if !valid_panel_id(id) {
        return Err(ManifestError::invalid(
            id_path,
            "expected 1 to 32 characters: a-z first, then a-z, 0-9 or '-'",
        ));
    }
    let source_path = t.path_of("source");
    let source = file_path(source_path, t.str_req("source")?)?;
    let point_path = t.path_of("point");
    let point = t.str_req("point")?;
    if !valid_point(point) {
        return Err(ManifestError::invalid(
            point_path,
            format!(
                "{point:?} is not a point id (2 to 64 characters: a-z first, then a-z, 0-9, '.' or '-')"
            ),
        ));
    }
    let mode = enum_value(&mut t, "mode", UiMode::parse, "append, replace")?.ok_or_else(|| {
        ManifestError::Missing {
            path: format!("{path}.mode"),
        }
    })?;
    let fallback =
        enum_value(&mut t, "fallback", UiFallback::parse, "mods, none")?.unwrap_or_default();
    t.finish()?;
    Ok(UiPanel {
        id: id.to_string(),
        source,
        point: point.to_string(),
        mode,
        fallback,
    })
}

/// Parses `[ui]`; `None` when the manifest has none.
pub(crate) fn parse_ui(t: Option<Tbl<'_>>) -> Res<Option<UiSection>> {
    let Some(mut t) = t else { return Ok(None) };
    let api_path = t.path_of("api");
    let api = t
        .str_req("api")?
        .parse::<VersionReq>()
        .map_err(|e| ManifestError::invalid(api_path, e.to_string()))?;
    let model_path = t.path_of("model");
    let model = t
        .str_opt("model")?
        .map(|s| file_path(model_path, s))
        .transpose()?;

    let reads_path = t.path_of("reads");
    let reads_raw = t.strings_opt("reads")?.unwrap_or_default();
    if reads_raw.len() > UI_READS_MAX {
        return Err(ManifestError::invalid(
            reads_path,
            format!("at most {UI_READS_MAX} paths"),
        ));
    }
    let mut reads = Vec::new();
    for (path, s) in reads_raw {
        if !valid_read(s) {
            return Err(ManifestError::invalid(
                path,
                format!("{s:?} is not a model path (like \"game.turn\" or \"players.*\")"),
            ));
        }
        if reads.iter().any(|r| r == s) {
            return Err(ManifestError::invalid(
                path,
                format!("{s:?} is listed twice"),
            ));
        }
        reads.push(s.to_string());
    }

    let scripts_path = t.path_of("scripts");
    let scripts_raw = t.strings_opt("scripts")?.unwrap_or_default();
    if scripts_raw.len() > UI_SCRIPTS_MAX {
        return Err(ManifestError::invalid(
            scripts_path,
            format!("at most {UI_SCRIPTS_MAX} files"),
        ));
    }
    let mut scripts: Vec<String> = Vec::new();
    for (path, s) in scripts_raw {
        let f = file_path(path.clone(), s)?;
        if scripts.contains(&f) {
            return Err(ManifestError::invalid(
                path,
                format!("{s:?} is listed twice"),
            ));
        }
        scripts.push(f);
    }

    let panels_path = t.path_of("panels");
    let items = match t.get("panels") {
        None => {
            return Err(ManifestError::Missing { path: panels_path });
        }
        Some(Value::Array(items)) => items,
        Some(other) => {
            return Err(wrong_type(
                &panels_path,
                "array of tables ([[ui.panels]])",
                other,
            ));
        }
    };
    if items.is_empty() || items.len() > UI_PANELS_MAX {
        return Err(ManifestError::invalid(
            panels_path,
            format!("a [ui] section has 1 to {UI_PANELS_MAX} panels"),
        ));
    }
    let mut panels: Vec<UiPanel> = Vec::with_capacity(items.len());
    let mut ids = BTreeSet::new();
    for (i, item) in items.iter().enumerate() {
        let path = format!("{panels_path}[{i}]");
        let p = parse_panel(item, &path)?;
        if !ids.insert(p.id.clone()) {
            return Err(ManifestError::invalid(
                format!("{path}.id"),
                format!("panel id {:?} is used twice", p.id),
            ));
        }
        if p.mode == UiMode::Replace
            && let Some(other) = panels
                .iter()
                .find(|o| o.mode == UiMode::Replace && o.point == p.point)
        {
            return Err(ManifestError::invalid(
                format!("{path}.point"),
                format!(
                    "panel `{}` already replaces `{}`; a point is replaced once",
                    other.id, p.point
                ),
            ));
        }
        panels.push(p);
    }
    t.finish()?;
    Ok(Some(UiSection {
        api,
        model,
        reads,
        scripts,
        panels,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        assert!(
            valid_point("mods")
                && valid_point("hud.right")
                && valid_point("window.production.detail")
        );
        assert!(
            !valid_point("m")
                && !valid_point("Hud")
                && !valid_point("1a")
                && !valid_point("hud_right")
        );
        assert!(!valid_point(&format!("a{}", "b".repeat(64))));
        assert!(valid_read("game.turn") && valid_read("players.*.name") && valid_read("game"));
        assert!(
            !valid_read("Game") && !valid_read("game.") && !valid_read(".x") && !valid_read("a..b")
        );
        assert!(valid_panel_id("e") && valid_panel_id("echo-2") && !valid_panel_id("Echo"));
    }
}
