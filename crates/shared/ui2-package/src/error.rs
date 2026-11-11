//! The error type of reading and writing packages.

use crate::container::Tag;

/// Why a package could not be read, validated or written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PackageError {
    /// A well-formed package this reader cannot use: another format major, a section the reader
    /// must understand but does not, or a mismatch with the client's versions or model paths.
    #[error("incompatible package: needs {need}")]
    Incompatible {
        /// What the package needs that the reader lacks.
        need: String,
    },
    /// The bytes are damaged: bad hash, bad table, a section that does not decode.
    #[error("corrupt package{}", fmt_at(.at))]
    Corrupt {
        /// The section the damage was found in, `None` for the file as a whole.
        at: Option<Tag>,
    },
    /// The bytes decode, but a value is out of range or inconsistent.
    #[error("invalid section {at}: {what}")]
    Invalid {
        /// The section holding the bad value.
        at: Tag,
        /// What is wrong, for a log line.
        what: String,
    },
    /// The builder was given data that cannot be written.
    #[error("cannot build package: {0}")]
    Build(String),
}

fn fmt_at(at: &Option<Tag>) -> String {
    match at {
        Some(tag) => format!(" (section {tag})"),
        None => String::new(),
    }
}

impl PackageError {
    pub(crate) fn invalid(at: Tag, what: impl Into<String>) -> Self {
        Self::Invalid {
            at,
            what: what.into(),
        }
    }

    #[cfg(feature = "write")]
    pub(crate) fn build(what: impl Into<String>) -> Self {
        Self::Build(what.into())
    }
}
