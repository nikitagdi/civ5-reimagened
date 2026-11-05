//! Blob decoding errors.

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormatError {
    #[error("blob is too short for a header")]
    Truncated,
    #[error("bad magic: expected {expected:?}, found {found:?}")]
    BadMagic { expected: [u8; 4], found: [u8; 4] },
    #[error(
        "unsupported format version {found} ({}: this build reads {min_supported}..={supported})",
        String::from_utf8_lossy(magic)
    )]
    UnsupportedVersion {
        magic: [u8; 4],
        found: u16,
        /// Newest version this build reads.
        supported: u16,
        /// Oldest version this build reads.
        min_supported: u16,
    },
    #[error("malformed payload: {0}")]
    Payload(#[from] postcard::Error),
    #[error("{0} trailing bytes after payload")]
    TrailingBytes(usize),
    #[error("invalid blob: {0}")]
    Invalid(String),
    #[error("mip {level} is {found} bytes, expected {expected}")]
    BadMipSize {
        level: usize,
        expected: usize,
        found: usize,
    },
}

impl FormatError {
    pub(crate) fn invalid(msg: impl Into<String>) -> Self {
        Self::Invalid(msg.into())
    }
}
