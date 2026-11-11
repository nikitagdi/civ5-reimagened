//! `MANI`: the manifest.
//!
//! There is no build time, no absolute path and no machine name in it: the same input gives the
//! same bytes.

use serde::{Deserialize, Serialize};

use crate::container::Tag;
use crate::error::PackageError;

/// What the package is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    /// A whole skin.
    Skin,
    /// A mod fragment that fills the named slots of a skin.
    Fragment {
        /// The extension points the fragment adds children to.
        slots: Vec<String>,
    },
}

/// The tools that built the package; they change only when the tools do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tool {
    /// The converter's name and version.
    pub converter: String,
    /// The Chrome version that baked the styles.
    pub chrome: String,
    /// blake3 (hex) of the Chrome flags.
    pub chrome_flags: String,
    /// The font subsetting tool's version.
    pub fonttools: String,
}

/// A font family name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaceFamily(pub String);

/// The skin's faces for the roles mods use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FontRoles {
    /// `ui-sans`.
    pub sans: FaceFamily,
    /// `ui-display`.
    pub display: FaceFamily,
}

/// The manifest section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Skin or mod fragment.
    pub kind: Kind,
    /// `[a-z][a-z0-9-]{0,31}`.
    pub id: String,
    /// The skin's semver.
    pub version: String,
    /// The `L10N` key of the skin's title.
    pub title_key: String,
    /// The UI profile version `(major, minor)` the styles were baked for.
    pub profile: (u16, u16),
    /// The model version `(major, minor)` the skin was built against.
    pub model: (u16, u16),
    /// The script API version `(major, minor)`.
    pub api: (u16, u16),
    /// The default skin of the build.
    pub default: bool,
    /// The tools that built the package.
    pub tool: Tool,
    /// Theme names; `[0]` is the default theme.
    pub themes: Vec<String>,
    /// Languages (`en_US`, ...); `[0]` is the fallback.
    pub languages: Vec<String>,
    /// The faces of the font roles.
    pub font_roles: FontRoles,
    /// The model schema hash at build time: information only, compatibility is decided by paths.
    pub schema_hash: u64,
}

impl Manifest {
    /// Checks what can be checked without the other sections.
    pub(crate) fn validate(&self, at: Tag) -> Result<(), PackageError> {
        if !valid_id(&self.id) {
            return Err(PackageError::invalid(
                at,
                "the id is not [a-z][a-z0-9-]{0,31}",
            ));
        }
        if self.themes.is_empty() || self.languages.is_empty() {
            return Err(PackageError::invalid(
                at,
                "a skin needs a theme and a language",
            ));
        }
        Ok(())
    }
}

/// `[a-z][a-z0-9-]{0,31}`.
pub fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

#[cfg(test)]
mod tests {
    use super::valid_id;

    #[test]
    fn ids() {
        assert!(valid_id("a"));
        assert!(valid_id("skin-a2"));
        assert!(!valid_id(""));
        assert!(!valid_id("2a"));
        assert!(!valid_id("A"));
        assert!(!valid_id("a_b"));
        assert!(valid_id(&"a".repeat(32)));
        assert!(!valid_id(&"a".repeat(33)));
    }
}
