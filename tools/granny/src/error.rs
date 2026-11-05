//! Error type for every malformed-input case; parsing never panics.

use crate::Ptr;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GrannyError {
    #[error("not a granny file (bad magic)")]
    BadMagic,
    #[error("unsupported granny variant: {0}")]
    UnsupportedVariant(&'static str),
    #[error("unsupported file format revision {0} (expected 7)")]
    UnsupportedRevision(u32),
    #[error("truncated or out-of-bounds read: {0}")]
    OutOfBounds(&'static str),
    #[error("section {section}: unsupported compression type {kind}")]
    UnsupportedCompression { section: u32, kind: u32 },
    #[error("section {section}: expanded size {size} exceeds limit")]
    SectionTooLarge { section: u32, size: u32 },
    #[error("Oodle1: {0}")]
    Oodle1(&'static str),
    #[error("section {section}: invalid fixup #{index}: {reason}")]
    BadFixup {
        section: u32,
        index: u32,
        reason: &'static str,
    },
    #[error("invalid pointer {0}")]
    BadPointer(Ptr),
    #[error("unknown member type {kind} in type at {at}")]
    UnknownMemberType { kind: u32, at: Ptr },
    #[error("type at {0} nests too deep (cyclic inline members?)")]
    TypeTooDeep(Ptr),
    #[error("type at {0} has too many members")]
    TooManyMembers(Ptr),
    #[error("size overflow in {0}")]
    SizeOverflow(&'static str),
    #[error("negative array count {0}")]
    NegativeCount(i32),
    #[error("unterminated string at {0}")]
    UnterminatedString(Ptr),
    #[error("missing type for {0}")]
    MissingType(&'static str),
    #[error("missing field `{0}`")]
    MissingField(String),
    #[error("field `{field}`: expected {expected}, found {found}")]
    TypeMismatch {
        field: String,
        expected: &'static str,
        found: &'static str,
    },
    #[error("null reference in field `{0}`")]
    NullReference(String),
    #[error("{0}")]
    Invalid(String),
}
