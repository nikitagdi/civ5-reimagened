//! `BlobStore` stub; the disk cache comes later.

use platform::{BlobError, BlobKey, BlobOp, BlobStore};

/// Always unavailable; `assets` downloads without caching.
#[derive(Debug, Default, Clone, Copy)]
pub struct StubBlobStore;

struct Ready<T>(Option<Result<T, BlobError>>);

impl<T> BlobOp<T> for Ready<T> {
    fn poll(&mut self) -> Option<Result<T, BlobError>> {
        self.0.take()
    }
}

fn unavailable() -> BlobError {
    BlobError::Unavailable("disk cache not implemented yet".into())
}

impl BlobStore for StubBlobStore {
    fn get(&self, _key: &BlobKey) -> Box<dyn BlobOp<Option<Vec<u8>>>> {
        Box::new(Ready(Some(Err(unavailable()))))
    }

    fn put(&self, _key: &BlobKey, _bytes: Vec<u8>) -> Box<dyn BlobOp<()>> {
        Box::new(Ready(Some(Err(unavailable()))))
    }

    fn get_named(&self, _name: &str) -> Box<dyn BlobOp<Option<Vec<u8>>>> {
        Box::new(Ready(Some(Err(unavailable()))))
    }

    fn put_named(&self, _name: &str, _bytes: Vec<u8>) -> Box<dyn BlobOp<()>> {
        Box::new(Ready(Some(Err(unavailable()))))
    }
}
