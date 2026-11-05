//! blob-store: content-addressed blobs, manifest and incremental cache.
//!
//! Layout under `<out>`: `<64-hex>` blobs (what the server gateway serves from `--assets-root`),
//! `manifest` (hex hash of the manifest blob + newline) and `.cache/<hex>` build-cache entries.
//! Old blobs are never garbage-collected in this slice; rebuilding into a fresh dir is the cleanup.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use asset_format::{ContentHash, LogicalId, Manifest, Variant};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::world::groups::BOOT;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("i/o error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cache entry encoding failed: {0}")]
    Encode(postcard::Error),
}

pub type Result<T, E = StoreError> = std::result::Result<T, E>;

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> StoreError + '_ {
    move |source| StoreError::Io {
        path: path.to_path_buf(),
        source,
    }
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Write via a temp file in the same dir + rename, so readers never see partial files.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    // Leading dot + non-hex name: never collides with a servable blob name.
    let tmp = dir.join(format!(".tmp-{}-{n}", std::process::id()));
    fs::write(&tmp, bytes).map_err(io_err(&tmp))?;
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        // Windows can refuse to replace a file another thread just placed; same content is fine.
        let same = fs::metadata(path).is_ok_and(|m| m.len() == bytes.len() as u64);
        if !same {
            return Err(io_err(path)(e));
        }
    }
    Ok(())
}

/// Counters for the CLI summary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StoreStats {
    pub blobs_written: u64,
    pub blobs_reused: u64,
    pub bytes_written: u64,
    pub manifest_writes: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
}

#[derive(Default)]
struct Counters {
    written: AtomicU64,
    reused: AtomicU64,
    bytes: AtomicU64,
    manifest_writes: AtomicU64,
    hits: AtomicU64,
    misses: AtomicU64,
}

/// Content-addressed blob directory. `&self` API; safe to share across rayon threads.
pub struct BlobStore {
    dir: PathBuf,
    counters: Counters,
}

impl BlobStore {
    pub fn open(out_dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = out_dir.into();
        fs::create_dir_all(&dir).map_err(io_err(&dir))?;
        Ok(Self {
            dir,
            counters: Counters::default(),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn blob_path(&self, hash: &ContentHash) -> PathBuf {
        self.dir.join(hash.to_string())
    }

    pub fn contains(&self, hash: &ContentHash) -> bool {
        self.blob_path(hash).is_file()
    }

    /// Stores `bytes` under their blake3 hash; returns `(hash, len)`.
    pub fn put(&self, bytes: &[u8]) -> Result<(ContentHash, u64)> {
        let hash = ContentHash(*blake3::hash(bytes).as_bytes());
        let len = bytes.len() as u64;
        let path = self.blob_path(&hash);
        // Size check is a cheap guard against truncated files from an interrupted older run.
        if fs::metadata(&path).is_ok_and(|m| m.len() == len) {
            self.counters.reused.fetch_add(1, Ordering::Relaxed);
        } else {
            write_atomic(&path, bytes)?;
            self.counters.written.fetch_add(1, Ordering::Relaxed);
            self.counters.bytes.fetch_add(len, Ordering::Relaxed);
        }
        Ok((hash, len))
    }

    pub fn stats(&self) -> StoreStats {
        let c = &self.counters;
        StoreStats {
            blobs_written: c.written.load(Ordering::Relaxed),
            blobs_reused: c.reused.load(Ordering::Relaxed),
            bytes_written: c.bytes.load(Ordering::Relaxed),
            manifest_writes: c.manifest_writes.load(Ordering::Relaxed),
            cache_hits: c.hits.load(Ordering::Relaxed),
            cache_misses: c.misses.load(Ordering::Relaxed),
        }
    }
}

/// Collects (id, variant) pairs; order of `add` calls does not affect the result.
#[derive(Default)]
pub struct ManifestBuilder {
    manifest: Manifest,
    /// Ids added in this session; a first `add` to any other id replaces what was loaded.
    fresh: std::collections::HashSet<LogicalId>,
}

impl ManifestBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts from the manifest `<out>/manifest` points at (empty if there is none), so a build
    /// of one set adds to what earlier builds put there instead of replacing it.
    pub fn open(store: &BlobStore) -> Result<Self> {
        let Ok(text) = fs::read_to_string(store.dir.join("manifest")) else {
            return Ok(Self::new());
        };
        let Ok(hash) = text.trim().parse::<ContentHash>() else {
            return Ok(Self::new());
        };
        let manifest = fs::read(store.blob_path(&hash))
            .ok()
            .and_then(|bytes| Manifest::decode(&bytes).ok())
            .unwrap_or_default();
        Ok(Self {
            manifest,
            ..Self::default()
        })
    }

    pub fn add(&mut self, id: LogicalId, mut variant: Variant) {
        // The boot flag is merged, not replaced: an id that one set put in `boot` (the selection
        // ring, named by `b2-light` and `b2-misc`) stays in it when another set adds the same
        // texture with "by the group table". So the result does not depend on the order of the sets.
        if self
            .manifest
            .variants(&id)
            .iter()
            .any(|v| v.prefetch == BOOT)
        {
            variant.prefetch = BOOT;
        }
        // An id rebuilt in this session supersedes the one loaded from an earlier build.
        if !self.fresh.contains(&id) {
            self.manifest.remove(&id);
            self.fresh.insert(id.clone());
        }
        self.manifest.insert(id, variant);
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Stores the manifest blob and points `<out>/manifest` at it (rewritten only on change).
    pub fn finish(&self, store: &BlobStore) -> Result<ContentHash> {
        let (hash, _) = store.put(&self.manifest.encode())?;
        let path = store.dir.join("manifest");
        let text = format!("{hash}\n");
        if fs::read(&path).ok().as_deref() != Some(text.as_bytes()) {
            write_atomic(&path, text.as_bytes())?;
            store
                .counters
                .manifest_writes
                .fetch_add(1, Ordering::Relaxed);
        }
        Ok(hash)
    }
}

/// Cache key: blake3 over (step name, step version, input hashes, params).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CacheKey(pub ContentHash);

impl CacheKey {
    /// Fields are length-prefixed so different splits can't collide. Bump `version` whenever
    /// the step's output changes for the same inputs.
    pub fn new(step: &str, version: u32, inputs: &[ContentHash], params: &[u8]) -> Self {
        let mut h = blake3::Hasher::new();
        h.update(&(step.len() as u64).to_le_bytes());
        h.update(step.as_bytes());
        h.update(&version.to_le_bytes());
        h.update(&(inputs.len() as u64).to_le_bytes());
        for i in inputs {
            h.update(i.as_bytes());
        }
        h.update(&(params.len() as u64).to_le_bytes());
        h.update(params);
        Self(ContentHash(*h.finalize().as_bytes()))
    }
}

#[derive(Serialize, Deserialize)]
struct CacheEntry {
    /// Blobs the value refers to; entry is stale if any is gone.
    outputs: Vec<ContentHash>,
    value: Vec<u8>,
}

/// Step-level incremental cache: key -> small postcard value + the blobs it refers to.
pub struct BuildCache<'a> {
    store: &'a BlobStore,
    dir: PathBuf,
}

impl<'a> BuildCache<'a> {
    pub fn open(store: &'a BlobStore) -> Result<Self> {
        let dir = store.dir.join(".cache");
        fs::create_dir_all(&dir).map_err(io_err(&dir))?;
        Ok(Self { store, dir })
    }

    /// `None` (a miss) if absent, undecodable, or any referenced blob was deleted.
    pub fn get<T: DeserializeOwned>(&self, key: &CacheKey) -> Option<T> {
        let found = self.lookup(key);
        let counter = if found.is_some() {
            &self.store.counters.hits
        } else {
            &self.store.counters.misses
        };
        counter.fetch_add(1, Ordering::Relaxed);
        found
    }

    fn lookup<T: DeserializeOwned>(&self, key: &CacheKey) -> Option<T> {
        let bytes = fs::read(self.dir.join(key.0.to_string())).ok()?;
        let entry: CacheEntry = postcard::from_bytes(&bytes).ok()?;
        if !entry.outputs.iter().all(|h| self.store.contains(h)) {
            return None;
        }
        postcard::from_bytes(&entry.value).ok()
    }

    pub fn insert<T: Serialize>(
        &self,
        key: &CacheKey,
        value: &T,
        outputs: &[ContentHash],
    ) -> Result<()> {
        let entry = CacheEntry {
            outputs: outputs.to_vec(),
            value: postcard::to_stdvec(value).map_err(StoreError::Encode)?,
        };
        let bytes = postcard::to_stdvec(&entry).map_err(StoreError::Encode)?;
        write_atomic(&self.dir.join(key.0.to_string()), &bytes)
    }

    /// Returns the cached value or computes, caches and returns it. `outputs_of` lists the
    /// blobs the value refers to.
    pub fn get_or_compute<T, E>(
        &self,
        key: &CacheKey,
        outputs_of: impl FnOnce(&T) -> Vec<ContentHash>,
        compute: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E>
    where
        T: Serialize + DeserializeOwned,
        E: From<StoreError>,
    {
        if let Some(v) = self.get(key) {
            return Ok(v);
        }
        let v = compute()?;
        self.insert(key, &v, &outputs_of(&v))?;
        Ok(v)
    }
}
