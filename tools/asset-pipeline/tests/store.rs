use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use asset_format::{ContentHash, LogicalId, PrefetchGroup, Variant, VariantFormat};
use asset_pipeline::store::{BlobStore, BuildCache, CacheKey, ManifestBuilder};
use rayon::prelude::*;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "asset-store-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        Self(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn variant(hash: ContentHash, size: u64) -> Variant {
    Variant {
        format: VariantFormat::Generic,
        hash,
        size,
        prefetch: PrefetchGroup(0),
    }
}

#[test]
fn put_twice_writes_once() {
    let tmp = TempDir::new();
    let store = BlobStore::open(tmp.path()).unwrap();
    let (h1, n1) = store.put(b"hello").unwrap();
    let (h2, n2) = store.put(b"hello").unwrap();
    assert_eq!((h1, n1), (h2, n2));
    assert_eq!(n1, 5);
    assert_eq!(
        std::fs::read(tmp.path().join(h1.to_string())).unwrap(),
        b"hello"
    );
    let s = store.stats();
    assert_eq!(
        (s.blobs_written, s.blobs_reused, s.bytes_written),
        (1, 1, 5)
    );
}

#[test]
fn concurrent_identical_puts_are_safe() {
    let tmp = TempDir::new();
    let store = BlobStore::open(tmp.path()).unwrap();
    let data = vec![7u8; 100_000];
    let hashes: Vec<_> = (0..64)
        .into_par_iter()
        .map(|_| store.put(&data).unwrap().0)
        .collect();
    assert!(hashes.windows(2).all(|w| w[0] == w[1]));
    assert_eq!(std::fs::read(store.blob_path(&hashes[0])).unwrap(), data);
    let s = store.stats();
    assert!(s.blobs_written >= 1);
    assert_eq!(s.blobs_written + s.blobs_reused, 64);
    // No temp files left behind.
    let leftovers = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp"))
        .count();
    assert_eq!(leftovers, 0);
}

#[test]
fn manifest_rewritten_only_on_change() {
    let tmp = TempDir::new();
    let store = BlobStore::open(tmp.path()).unwrap();
    let (bh, bn) = store.put(b"blob").unwrap();
    let mut b = ManifestBuilder::new();
    b.add(LogicalId::new("a"), variant(bh, bn));
    let h1 = b.finish(&store).unwrap();
    let h2 = b.finish(&store).unwrap();
    assert_eq!(h1, h2);
    assert_eq!(store.stats().manifest_writes, 1);
    let text = std::fs::read_to_string(tmp.path().join("manifest")).unwrap();
    assert_eq!(text, format!("{h1}\n"));
    assert!(store.contains(&h1));

    b.add(LogicalId::new("b"), variant(bh, bn));
    let h3 = b.finish(&store).unwrap();
    assert_ne!(h1, h3);
    assert_eq!(store.stats().manifest_writes, 2);
}

#[test]
fn two_sequential_builds_keep_both_in_manifest() {
    let tmp = TempDir::new();
    let store = BlobStore::open(tmp.path()).unwrap();
    let (ha, na) = store.put(b"set a").unwrap();
    let (hb, nb) = store.put(b"set b").unwrap();

    let mut first = ManifestBuilder::open(&store).unwrap();
    assert!(first.manifest().is_empty());
    first.add(LogicalId::new("a"), variant(ha, na));
    first.finish(&store).unwrap();

    // A later build (even from a fresh store handle) must add to the manifest, not replace it.
    let store = BlobStore::open(tmp.path()).unwrap();
    let mut second = ManifestBuilder::open(&store).unwrap();
    second.add(LogicalId::new("b"), variant(hb, nb));
    let hash = second.finish(&store).unwrap();

    let bytes = std::fs::read(store.blob_path(&hash)).unwrap();
    let m = asset_format::Manifest::decode(&bytes).unwrap();
    assert_eq!(m.variants(&LogicalId::new("a")).len(), 1);
    assert_eq!(m.variants(&LogicalId::new("b")).len(), 1);
    assert_eq!(m.len(), 2);
}

#[test]
fn rebuilding_same_set_does_not_change_manifest() {
    let tmp = TempDir::new();
    let store = BlobStore::open(tmp.path()).unwrap();
    let (h, n) = store.put(b"x").unwrap();
    let mut b = ManifestBuilder::open(&store).unwrap();
    b.add(LogicalId::new("a"), variant(h, n));
    let h1 = b.finish(&store).unwrap();
    let mut again = ManifestBuilder::open(&store).unwrap();
    again.add(LogicalId::new("a"), variant(h, n));
    assert_eq!(again.finish(&store).unwrap(), h1);
}

#[test]
fn manifest_independent_of_add_order() {
    let tmp = TempDir::new();
    let store = BlobStore::open(tmp.path()).unwrap();
    let items: Vec<_> = (0..20u8)
        .map(|i| {
            let (h, n) = store.put(&[i; 10]).unwrap();
            (LogicalId::new(format!("id{}", i % 7)), variant(h, n))
        })
        .collect();
    let build = |order: &mut dyn Iterator<Item = &(LogicalId, Variant)>| {
        let mut b = ManifestBuilder::new();
        for (id, v) in order {
            b.add(id.clone(), v.clone());
        }
        b.finish(&store).unwrap()
    };
    let fwd = build(&mut items.iter());
    let rev = build(&mut items.iter().rev());
    assert_eq!(fwd, rev);
}

#[test]
fn cache_hit_miss_and_invalidation() {
    let tmp = TempDir::new();
    let store = BlobStore::open(tmp.path()).unwrap();
    let cache = BuildCache::open(&store).unwrap();
    let input = ContentHash(*blake3::hash(b"in").as_bytes());
    let key = CacheKey::new("tex", 1, &[input], b"p");

    assert_eq!(cache.get::<Vec<u32>>(&key), None);
    let (out, _) = store.put(b"output").unwrap();
    cache.insert(&key, &vec![1u32, 2, 3], &[out]).unwrap();
    assert_eq!(cache.get::<Vec<u32>>(&key), Some(vec![1, 2, 3]));

    std::fs::remove_file(store.blob_path(&out)).unwrap();
    assert_eq!(cache.get::<Vec<u32>>(&key), None);

    let s = store.stats();
    assert_eq!((s.cache_hits, s.cache_misses), (1, 2));
}

#[test]
fn cache_key_depends_on_every_field() {
    let a = ContentHash([1; 32]);
    let b = ContentHash([2; 32]);
    let base = CacheKey::new("s", 1, &[a], b"p");
    assert_eq!(base, CacheKey::new("s", 1, &[a], b"p"));
    assert_ne!(base, CacheKey::new("t", 1, &[a], b"p"));
    assert_ne!(base, CacheKey::new("s", 2, &[a], b"p"));
    assert_ne!(base, CacheKey::new("s", 1, &[b], b"p"));
    assert_ne!(base, CacheKey::new("s", 1, &[a], b"q"));
    assert_ne!(base, CacheKey::new("s", 1, &[a, b], b""));
}

#[test]
fn get_or_compute_runs_once() {
    let tmp = TempDir::new();
    let store = BlobStore::open(tmp.path()).unwrap();
    let cache = BuildCache::open(&store).unwrap();
    let key = CacheKey::new("x", 1, &[], b"");
    let mut calls = 0;
    for _ in 0..2 {
        let v: String = cache
            .get_or_compute::<_, asset_pipeline::store::StoreError>(
                &key,
                |_| vec![],
                || {
                    calls += 1;
                    Ok("done".to_string())
                },
            )
            .unwrap();
        assert_eq!(v, "done");
    }
    assert_eq!(calls, 1);
}
