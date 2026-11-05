//! Content-addressed blob store: Cache API on the web, disk natively (architecture §6.5).

use std::fmt;

/// Blob key: content hash (32 bytes). The hash function is defined by `assets`.
pub type BlobKey = [u8; 32];

/// Pending storage operation. Yields its result once, then `None`.
pub trait BlobOp<T> {
    fn poll(&mut self) -> Option<Result<T, BlobError>>;
}

/// Cache of immutable blobs. Re-putting an existing key may be skipped.
pub trait BlobStore {
    fn get(&self, key: &BlobKey) -> Box<dyn BlobOp<Option<Vec<u8>>>>;

    /// `bytes` must already match `key` (checked by the caller in `assets`).
    fn put(&self, key: &BlobKey, bytes: Vec<u8>) -> Box<dyn BlobOp<()>>;

    /// Reads the mutable value kept under `name` (settings, not content): `None` if there is none.
    /// A store without named values says `Unavailable`.
    fn get_named(&self, _name: &str) -> Box<dyn BlobOp<Option<Vec<u8>>>> {
        Box::new(Failed::new(BlobError::Unavailable(
            "named values are not supported".into(),
        )))
    }

    /// Keeps `bytes` under `name`, replacing what was there (unlike `put`, which may skip an
    /// existing content key).
    fn put_named(&self, _name: &str, _bytes: Vec<u8>) -> Box<dyn BlobOp<()>> {
        Box::new(Failed::new(BlobError::Unavailable(
            "named values are not supported".into(),
        )))
    }
}

/// An operation that has already failed, for stores without a feature.
struct Failed(Option<BlobError>);

impl Failed {
    fn new(e: BlobError) -> Self {
        Self(Some(e))
    }
}

impl<T> BlobOp<T> for Failed {
    fn poll(&mut self) -> Option<Result<T, BlobError>> {
        self.0.take().map(Err)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlobError {
    Unavailable(String),
    Io(String),
    QuotaExceeded,
}

impl fmt::Display for BlobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlobError::Unavailable(msg) => write!(f, "blob store unavailable: {msg}"),
            BlobError::Io(msg) => write!(f, "blob store I/O error: {msg}"),
            BlobError::QuotaExceeded => f.write_str("blob store is out of space"),
        }
    }
}

impl std::error::Error for BlobError {}
