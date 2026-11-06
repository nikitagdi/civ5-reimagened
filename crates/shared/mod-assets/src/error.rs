//! Errors of the pack and lock codecs.

use crate::sha256::Sha256;

/// Everything that can be wrong with a pack, a lock or the pair.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PackError {
    /// The caller found no pack at all (not downloaded, not installed). The signal for "run the pack builder".
    #[error("the asset pack is missing")]
    Missing,
    #[error("not a mod asset pack (bad magic)")]
    BadMagic,
    #[error("the pack is truncated: {0}")]
    Truncated(&'static str),
    #[error("{0} unexpected bytes after the last file of the pack")]
    TrailingBytes(usize),
    #[error("manifest line {line}: {msg}")]
    Manifest { line: usize, msg: String },
    #[error("the manifest is too large ({len} bytes, the limit is {max})")]
    ManifestTooLarge { len: usize, max: usize },
    #[error("invalid asset id {0:?} (lowercase path segments of [a-z0-9_.-], no `..`)")]
    BadId(String),
    #[error("invalid source name {0:?}")]
    BadSourceName(String),
    #[error("duplicate asset id `{0}`")]
    DuplicateId(String),
    #[error("duplicate source `{0}`")]
    DuplicateSource(String),
    #[error("too many files: {count} (the limit is {max})")]
    TooManyFiles { count: usize, max: usize },
    #[error("file `{id}`: a {kind} must be {expected} bytes, found {actual}")]
    BadSize {
        id: String,
        kind: String,
        expected: u64,
        actual: u64,
    },
    #[error("file `{id}`: invalid texture shape {width}x{height} with {mips} mip level(s)")]
    BadTexture {
        id: String,
        width: u32,
        height: u32,
        mips: u8,
    },
    #[error("file `{id}` is corrupt: sha256 is {actual}, the manifest says {expected}")]
    Corrupt {
        id: String,
        expected: Sha256,
        actual: Sha256,
    },
    #[error("the pack hash is {actual}, expected {expected}")]
    HashMismatch { expected: Sha256, actual: Sha256 },
    #[error("assets.lock line {line}: {msg}")]
    Lock { line: usize, msg: String },
    #[error(transparent)]
    Verify(#[from] VerifyError),
}

/// One disagreement between a pack and its `assets.lock`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LockProblem {
    #[error("the lock lists source `{0}` but the pack does not record it")]
    SourceMissing(String),
    #[error("the pack records source `{0}` that the lock does not list")]
    SourceExtra(String),
    #[error("source `{name}`: the lock says {expected}, the pack says {actual}")]
    SourceMismatch {
        name: String,
        expected: Sha256,
        actual: Sha256,
    },
    #[error("the lock lists file `{0}` but the pack does not contain it")]
    FileMissing(String),
    #[error("the pack contains file `{0}` that the lock does not list")]
    FileExtra(String),
    #[error("file `{id}`: the lock says {expected}, the pack has {actual}")]
    FileMismatch {
        id: String,
        expected: Sha256,
        actual: Sha256,
    },
}

/// All problems found by [`crate::verify_against_lock`] (never empty).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyError(pub Vec<LockProblem>);

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the pack does not match assets.lock: ")?;
        for (i, p) in self.0.iter().enumerate() {
            if i > 0 {
                write!(f, "; ")?;
            }
            write!(f, "{p}")?;
        }
        Ok(())
    }
}

impl std::error::Error for VerifyError {}
