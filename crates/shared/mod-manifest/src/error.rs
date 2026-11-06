//! Error types of the crate. Every message is meant to be shown to an operator as is.

use thiserror::Error;

use crate::manifest::ModId;
use crate::semver::{Version, VersionReq};

/// A malformed version or version requirement.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SemverError {
    #[error("invalid version {input:?}: {reason}")]
    Version { input: String, reason: &'static str },
    #[error("invalid version requirement {input:?}: {reason}")]
    Req { input: String, reason: &'static str },
}

/// A problem in `mod.toml` (or in the TOML-shaped `mods.lock`). Every variant except `Syntax` carries the key
/// path (`channels[1].rate`, `deps.some-lib`), so the author can find the line.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ManifestError {
    #[error("invalid TOML: {0}")]
    Syntax(String),
    #[error("{path}: unknown key")]
    UnknownKey { path: String },
    #[error("{path}: missing required key")]
    Missing { path: String },
    #[error("{path}: expected {expected}, found {found}")]
    WrongType {
        path: String,
        expected: &'static str,
        found: &'static str,
    },
    #[error("{path}: {message}")]
    Invalid { path: String, message: String },
}

impl ManifestError {
    /// The key path the error is about; `None` for a syntax error of the whole document.
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::Syntax(_) => None,
            Self::UnknownKey { path }
            | Self::Missing { path }
            | Self::WrongType { path, .. }
            | Self::Invalid { path, .. } => Some(path),
        }
    }

    pub(crate) fn invalid(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Invalid {
            path: path.into(),
            message: message.into(),
        }
    }
}

/// The dependency resolver refused the request. The text names the chain that led to the problem.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ResolveError {
    #[error("installed mod {id} {version} appears more than once")]
    DuplicateInstalled { id: ModId, version: Version },
    #[error("mod `{id}` is not installed (needed by: {requirers}; chain: {chain})")]
    Missing {
        id: ModId,
        /// "the host needs `x` ^1; b 1.0.0 needs `x` *".
        requirers: String,
        chain: String,
    },
    #[error(
        "no installed version of `{id}` satisfies every requirement: {constraints}; installed versions: {available}"
    )]
    NoMatchingVersion {
        id: ModId,
        /// "a 1.0.0 needs `b` ^2; c 1.2.0 needs `b` =1.4.0".
        constraints: String,
        available: String,
    },
    #[error(
        "mod API mismatch: this host provides API {host_api}, but `{id}` does not admit it ({versions}); chain: {chain}"
    )]
    ApiMismatch {
        id: ModId,
        host_api: Version,
        /// "1.0.0 wants ^2, 0.9.0 wants ^0.1".
        versions: String,
        chain: String,
    },
    #[error("{0}")]
    Conflict(Box<ConflictError>),
    #[error("load order cycle: {edges}")]
    Cycle { edges: String },
    #[error("resolution gave up after more than {limit} steps (too many alternatives)")]
    SearchLimit { limit: u32 },
    #[error("mod set is invalid: {0}")]
    Set(#[from] ModSetError),
}

/// Two chosen mods that must not be loaded together. `a` declared the conflict with `b`.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error(
    "{a} {a_version} conflicts with {b} {b_version} (`{a}` declares conflicts.{b} = \"{req}\"); chain of {a}: {a_chain}; chain of {b}: {b_chain}"
)]
pub struct ConflictError {
    pub a: ModId,
    pub a_version: Version,
    pub b: ModId,
    pub b_version: Version,
    pub req: VersionReq,
    pub a_chain: String,
    pub b_chain: String,
}

/// A `ModSet` that breaks its invariants.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ModSetError {
    #[error("mod `{0}` appears twice in the mod set")]
    DuplicateId(ModId),
    #[error("mod `{id}` has load_idx {found}, expected {expected}")]
    BadLoadIdx {
        id: ModId,
        expected: u16,
        found: u16,
    },
    #[error("too many mods ({0}); load_idx is a u16")]
    TooMany(usize),
}

/// A `mods.lock` that cannot be read.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LockError {
    #[error("mods.lock: {0}")]
    Field(#[from] ManifestError),
    #[error("mods.lock: unsupported lock version {0}")]
    Version(i64),
    #[error(
        "mods.lock: modset_hash {found} does not match the listed mods ({expected}); the file was edited or is corrupt"
    )]
    HashMismatch { expected: String, found: String },
    #[error("mods.lock: {0}")]
    Set(#[from] ModSetError),
}
