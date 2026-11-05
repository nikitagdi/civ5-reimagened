//! Blob sources and their composition.

use std::rc::Rc;

use platform::{BlobOp, BlobStore};

use crate::{AssetError, ContentHash};

/// Pending fetch. Returns the result once, then `None`. `Ok(None)` means "not in this
/// source" and is not an error: [`Layered`] moves on.
pub trait FetchOp {
    fn poll(&mut self) -> Option<Result<Option<Vec<u8>>, AssetError>>;
}

/// Source of immutable blobs by hash. Pollable rather than `async` (see crate docs).
pub trait BlobSource {
    fn get(&self, hash: &ContentHash) -> Box<dyn FetchOp>;

    /// Offers a blob found farther away, so caches can keep it. Default: no-op.
    fn put(&self, _hash: &ContentHash, _bytes: &[u8]) {}
}

/// Sources tried in order, nearest first. Failing sources are skipped. A hit is written back
/// to every nearer source. Result: `Ok(None)` if nobody has it, or the last error if some failed.
pub struct Layered {
    layers: Rc<Vec<Box<dyn BlobSource>>>,
}

impl Layered {
    pub fn new(layers: Vec<Box<dyn BlobSource>>) -> Self {
        Self {
            layers: Rc::new(layers),
        }
    }

    pub fn len(&self) -> usize {
        self.layers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }
}

impl BlobSource for Layered {
    fn get(&self, hash: &ContentHash) -> Box<dyn FetchOp> {
        Box::new(LayeredOp {
            layers: Rc::clone(&self.layers),
            hash: *hash,
            idx: 0,
            current: None,
            last_err: None,
            finished: false,
        })
    }

    fn put(&self, hash: &ContentHash, bytes: &[u8]) {
        for layer in self.layers.iter() {
            layer.put(hash, bytes);
        }
    }
}

struct LayeredOp {
    layers: Rc<Vec<Box<dyn BlobSource>>>,
    hash: ContentHash,
    idx: usize,
    current: Option<Box<dyn FetchOp>>,
    last_err: Option<AssetError>,
    finished: bool,
}

impl FetchOp for LayeredOp {
    fn poll(&mut self) -> Option<Result<Option<Vec<u8>>, AssetError>> {
        if self.finished {
            return None;
        }
        loop {
            let Some(layer) = self.layers.get(self.idx) else {
                self.finished = true;
                return Some(match self.last_err.take() {
                    Some(e) => Err(e),
                    None => Ok(None),
                });
            };
            let op = self.current.get_or_insert_with(|| layer.get(&self.hash));
            match op.poll() {
                None => return None,
                Some(Ok(Some(bytes))) => {
                    for nearer in &self.layers[..self.idx] {
                        nearer.put(&self.hash, &bytes);
                    }
                    self.finished = true;
                    return Some(Ok(Some(bytes)));
                }
                Some(Ok(None)) => {}
                Some(Err(e)) => {
                    tracing::warn!(hash = %self.hash, layer = self.idx, error = %e, "blob source returned an error");
                    self.last_err = Some(e);
                }
            }
            self.current = None;
            self.idx += 1;
        }
    }
}

/// Adapts a `platform::BlobStore` into a caching [`BlobSource`] layer.
pub struct StoreSource {
    store: Box<dyn BlobStore>,
}

impl StoreSource {
    pub fn new(store: Box<dyn BlobStore>) -> Self {
        Self { store }
    }
}

struct StoreOp(Box<dyn BlobOp<Option<Vec<u8>>>>);

impl FetchOp for StoreOp {
    fn poll(&mut self) -> Option<Result<Option<Vec<u8>>, AssetError>> {
        self.0.poll().map(|r| r.map_err(AssetError::from))
    }
}

impl BlobSource for StoreSource {
    fn get(&self, hash: &ContentHash) -> Box<dyn FetchOp> {
        Box::new(StoreOp(self.store.get(&hash.0)))
    }

    fn put(&self, hash: &ContentHash, bytes: &[u8]) {
        // Best-effort cache write.
        drop(self.store.put(&hash.0, bytes.to_vec()));
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;

    struct FakeOp {
        delay: u32,
        result: Option<Result<Option<Vec<u8>>, AssetError>>,
    }

    impl FetchOp for FakeOp {
        fn poll(&mut self) -> Option<Result<Option<Vec<u8>>, AssetError>> {
            if self.delay > 0 {
                self.delay -= 1;
                return None;
            }
            self.result.take()
        }
    }

    /// In-memory source; `delay` is in polls.
    #[derive(Clone, Default)]
    struct Fake {
        blobs: Rc<RefCell<HashMap<ContentHash, Vec<u8>>>>,
        gets: Rc<RefCell<u32>>,
        fail: bool,
        delay: u32,
    }

    impl BlobSource for Fake {
        fn get(&self, hash: &ContentHash) -> Box<dyn FetchOp> {
            *self.gets.borrow_mut() += 1;
            let result = if self.fail {
                Err(AssetError::Network("test".into()))
            } else {
                Ok(self.blobs.borrow().get(hash).cloned())
            };
            Box::new(FakeOp {
                delay: self.delay,
                result: Some(result),
            })
        }

        fn put(&self, hash: &ContentHash, bytes: &[u8]) {
            self.blobs.borrow_mut().insert(*hash, bytes.to_vec());
        }
    }

    fn h(n: u8) -> ContentHash {
        ContentHash([n; 32])
    }

    fn run(op: &mut dyn FetchOp) -> Result<Option<Vec<u8>>, AssetError> {
        for _ in 0..100 {
            if let Some(r) = op.poll() {
                return r;
            }
        }
        panic!("operation did not complete");
    }

    #[test]
    fn first_hit_wins_and_far_layers_untouched() {
        let cache = Fake::default();
        let disk = Fake::default();
        let http = Fake::default();
        cache.blobs.borrow_mut().insert(h(1), vec![1]);
        http.blobs.borrow_mut().insert(h(1), vec![9]);
        let layered = Layered::new(vec![
            Box::new(cache.clone()),
            Box::new(disk.clone()),
            Box::new(http.clone()),
        ]);
        let got = run(&mut *layered.get(&h(1))).unwrap();
        assert_eq!(got, Some(vec![1]));
        assert_eq!(*disk.gets.borrow(), 0);
        assert_eq!(*http.gets.borrow(), 0);
    }

    #[test]
    fn falls_through_and_backfills_nearer_layers() {
        let cache = Fake::default();
        let disk = Fake::default();
        let http = Fake {
            delay: 3,
            ..Fake::default()
        };
        http.blobs.borrow_mut().insert(h(2), vec![2, 2]);
        let layered = Layered::new(vec![
            Box::new(cache.clone()),
            Box::new(disk.clone()),
            Box::new(http.clone()),
        ]);
        let mut op = layered.get(&h(2));
        assert!(op.poll().is_none(), "HTTP is still in flight");
        let got = run(&mut *op).unwrap();
        assert_eq!(got, Some(vec![2, 2]));
        assert_eq!(cache.blobs.borrow().get(&h(2)), Some(&vec![2, 2]));
        assert_eq!(disk.blobs.borrow().get(&h(2)), Some(&vec![2, 2]));
        assert!(op.poll().is_none(), "result is delivered once");
    }

    #[test]
    fn failing_layer_is_skipped() {
        let bad = Fake {
            fail: true,
            ..Fake::default()
        };
        let good = Fake::default();
        good.blobs.borrow_mut().insert(h(3), vec![3]);
        let layered = Layered::new(vec![Box::new(bad), Box::new(good)]);
        assert_eq!(run(&mut *layered.get(&h(3))).unwrap(), Some(vec![3]));
    }

    #[test]
    fn missing_everywhere_is_none_or_last_error() {
        let layered = Layered::new(vec![Box::new(Fake::default()), Box::new(Fake::default())]);
        assert_eq!(run(&mut *layered.get(&h(4))).unwrap(), None);

        let bad = Fake {
            fail: true,
            ..Fake::default()
        };
        let layered = Layered::new(vec![Box::new(Fake::default()), Box::new(bad)]);
        assert!(matches!(
            run(&mut *layered.get(&h(4))),
            Err(AssetError::Network(_))
        ));
    }
}
