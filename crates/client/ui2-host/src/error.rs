//! Why a skin could not be used.
//!
//! Every failure to bring a skin up is a value, never a panic: the application turns it into the fallback
//! (another skin or the classic UI) and a notice, and marks the skin broken for the session
//! ([`SkinRegistry::mark_broken`](crate::SkinRegistry::mark_broken)) so the fallback cannot loop.

use ui2_package::PackageError;

/// A skin that cannot be used, with the reason.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("skin {skin}: {failure}")]
pub struct SkinLoadError {
    /// The skin id (from the index, or the package's manifest when there is no index entry).
    pub skin: String,
    /// What went wrong.
    pub failure: LoadFailure,
}

impl SkinLoadError {
    pub(crate) fn new(skin: impl Into<String>, failure: LoadFailure) -> Self {
        Self {
            skin: skin.into(),
            failure,
        }
    }
}

/// The step that refused the skin. Later steps (download, script API, scripts failing at load) add variants.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LoadFailure {
    /// The index has no skin of this id.
    #[error("no such skin in the index")]
    UnknownSkin,
    /// The bytes are not a readable package.
    #[error("{0}")]
    Package(PackageError),
    /// The file is not the one the index names.
    #[error("the file's hash {file} does not match the index ({index})")]
    HashMismatch {
        /// The hash in the index (hex, possibly a prefix).
        index: String,
        /// The blake3 of the file (hex).
        file: String,
    },
    /// The package is another skin than the index entry says.
    #[error("the package is skin {package}, the index says {index}")]
    IdMismatch {
        /// The id in the index.
        index: String,
        /// The id in the package's manifest.
        package: String,
    },
    /// The package needs model paths, intents or versions this client does not have.
    #[error("{0}")]
    Incompatible(PackageError),
    /// A font of the package does not parse.
    #[error("fonts: {0}")]
    Fonts(String),
    /// The runtime cannot build the document.
    #[error("{0}")]
    Tree(String),
}
