//! The catalogue of UI v2 extension points and colour tokens (mod API 1.3).
//!
//! A mod attaches its panels to stable point ids, never to the classes of a skin. Every skin implements `mods`; a
//! point a skin lacks sends the panel to `mods` (or nowhere, with `fallback = "none"`). The catalogue has its own
//! version, the UI API version: a minor version only adds points and tokens, renaming or removing one is a major one.

use mod_manifest::{ManifestError, UiMode, UiSection, Version};

/// How a point takes panels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PointKind {
    /// A container: panels are appended after the skin's own children, in load order. It cannot be replaced.
    Stack,
    /// One element of the skin that a mod may replace (and so hide, with an empty panel).
    Single,
}

/// One extension point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UiPoint {
    /// The id a manifest names (`[[ui.panels]] point`).
    pub id: &'static str,
    pub kind: PointKind,
    /// The point holds buttons (a menu group, a row of actions): a panel there should be a button.
    pub buttons: bool,
    /// Every skin must implement it.
    pub required: bool,
}

/// The catalogue: points and the colour tokens a mod may read as `var(--ui-*)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiCatalog {
    /// The UI API version the catalogue belongs to.
    pub version: Version,
    pub points: &'static [UiPoint],
    /// Colour tokens every skin defines (names with the leading `--`).
    pub tokens: &'static [&'static str],
}

impl UiCatalog {
    /// The point with this id.
    pub fn point(&self, id: &str) -> Option<&'static UiPoint> {
        self.points.iter().find(|p| p.id == id)
    }

    /// Whether a mod may read this token.
    pub fn has_token(&self, name: &str) -> bool {
        self.tokens.contains(&name)
    }
}

/// The shared slot every skin has: panels whose point a skin lacks land here.
pub const MODS_POINT: &str = "mods";

const fn stack(id: &'static str, buttons: bool) -> UiPoint {
    UiPoint {
        id,
        kind: PointKind::Stack,
        buttons,
        required: false,
    }
}

const fn single(id: &'static str) -> UiPoint {
    UiPoint {
        id,
        kind: PointKind::Single,
        buttons: false,
        required: false,
    }
}

/// The catalogue of this build.
pub const UI_CATALOG: UiCatalog = UiCatalog {
    version: Version::new(1, 0, 0),
    points: &[
        UiPoint {
            id: MODS_POINT,
            kind: PointKind::Stack,
            buttons: false,
            required: true,
        },
        stack("hud.top.extra", false),
        stack("hud.menu", true),
        stack("hud.right", false),
        stack("hud.unit.actions", true),
        stack("window.production.detail", false),
        single("hud.minimap"),
        single("hud.chat"),
    ],
    tokens: &[
        "--ui-surface",
        "--ui-surface-2",
        "--ui-line",
        "--ui-text",
        "--ui-text-2",
        "--ui-text-3",
        "--ui-accent",
        "--ui-accent-ink",
        "--ui-negative",
        "--ui-y-food",
        "--ui-y-prod",
        "--ui-y-gold",
        "--ui-y-sci",
        "--ui-y-cult",
        "--ui-y-faith",
        "--ui-y-happy",
        "--ui-y-tour",
    ],
};

/// Checks the panels of a `[ui]` section against the catalogue: every point exists, and only a single point is
/// replaced. The error carries the key path (`ui.panels[0].point: unknown UI point "hud.nowhere"`), like any manifest
/// error. The build tool and the server's mod store both run it.
pub fn check_ui_points(ui: &UiSection) -> Result<(), ManifestError> {
    for (i, p) in ui.panels.iter().enumerate() {
        let path = format!("ui.panels[{i}].point");
        let Some(point) = UI_CATALOG.point(&p.point) else {
            return Err(ManifestError::Invalid {
                path,
                message: format!("unknown UI point {:?}", p.point),
            });
        };
        if p.mode == UiMode::Replace && point.kind == PointKind::Stack {
            return Err(ManifestError::Invalid {
                path,
                message: format!(
                    "UI point {:?} is a stack: panels are appended to it (mode = \"append\"), it cannot be replaced",
                    p.point
                ),
            });
        }
    }
    Ok(())
}
