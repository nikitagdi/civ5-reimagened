//! Version compatibility between a package and the client that loads it.
//!
//! The format version (`major.minor` of the container) is checked by `Package::read`. This module
//! checks the three versions the manifest records against the client's: the UI profile the styles
//! were baked for, the view model the bindings were built against, and the script API.

use crate::error::PackageError;
use crate::package::Package;

/// The UI profile version of this crate's `profile/profile.toml` (`major`, `minor`). A client that
/// links this crate runs this profile.
pub const PROFILE_VERSION: (u16, u16) = (1, 0);

/// The versions of the client a package is checked against, each `(major, minor)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Versions {
    /// The UI profile the runtime implements ([`PROFILE_VERSION`]).
    pub profile: (u16, u16),
    /// The view model schema of the client (`SchemaView::version`).
    pub model: (u16, u16),
    /// The script API the client's script host implements.
    pub api: (u16, u16),
}

impl Package {
    /// Checks the manifest's versions against the client's:
    ///
    /// - the profile major equals the client's (a minor only adds properties and values);
    /// - the model major equals the client's and the model minor is not newer;
    /// - the script API major equals the client's.
    ///
    /// Model paths are checked separately (`check_paths`). Any mismatch is `Incompatible`, naming
    /// what the package needs and what the client has.
    pub fn check_compat(&self, client: &Versions) -> Result<(), PackageError> {
        let m = self.manifest();
        let fail = |what: &str, need: (u16, u16), have: (u16, u16)| {
            Err(PackageError::Incompatible {
                need: format!(
                    "{what} {}.{} (the client has {}.{})",
                    need.0, need.1, have.0, have.1
                ),
            })
        };
        if m.profile.0 != client.profile.0 {
            return fail("UI profile", m.profile, client.profile);
        }
        if m.model.0 != client.model.0 || m.model.1 > client.model.1 {
            return fail("view model", m.model, client.model);
        }
        if m.api.0 != client.api.0 {
            return fail("script API", m.api, client.api);
        }
        Ok(())
    }
}
