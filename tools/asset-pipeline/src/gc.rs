//! `civ5-assets gc`: removes the blobs that the last N manifests do not need, so the asset folder
//! of the server does not grow with every release (deployment).
//!
//! The store keeps a history of the manifests it served: `manifests.log` in the output folder, one
//! manifest hash per line, oldest first. `civ5-assets build` appends the new pointer after every
//! run and `gc` does it too, so the folder's history starts at the first build or gc that saw it.
//! A client that holds an older manifest than the last N loses blobs; that is the point of `N`.
//!
//! Only files named by a 64-digit hash are touched. The `manifest` pointer, the history, the
//! reports and `.cache` stay. Nothing is removed if one of the manifests to keep does not read.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use asset_format::{ContentHash, Manifest};

/// Name of the history file in the output folder.
pub const HISTORY_FILE: &str = "manifests.log";
/// How many manifests `gc` keeps unless told otherwise.
pub const DEFAULT_KEEP: usize = 2;

#[derive(Debug, thiserror::Error)]
pub enum GcError {
    #[error("i/o error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Invalid(String),
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> GcError + '_ {
    move |source| GcError::Io {
        path: path.to_owned(),
        source,
    }
}

/// The manifest `out/manifest` points at; `None` when there is no pointer.
fn pointer(out: &Path) -> Result<Option<ContentHash>, GcError> {
    let p = out.join("manifest");
    let text = match fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io(&p)(e)),
    };
    text.trim()
        .parse()
        .map(Some)
        .map_err(|e| GcError::Invalid(format!("{} is not a hash: {e}", p.display())))
}

/// The history, oldest first; a line that is not a hash is an error (the file is ours).
pub fn history(out: &Path) -> Result<Vec<ContentHash>, GcError> {
    let p = out.join(HISTORY_FILE);
    let text = match fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io(&p)(e)),
    };
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| {
            l.parse()
                .map_err(|e| GcError::Invalid(format!("{}: {l:?} is not a hash: {e}", p.display())))
        })
        .collect()
}

fn write_history(out: &Path, hashes: &[ContentHash]) -> Result<(), GcError> {
    let p = out.join(HISTORY_FILE);
    let text: String = hashes.iter().map(|h| format!("{h}\n")).collect();
    let tmp = out.join(format!(".{HISTORY_FILE}.tmp"));
    fs::write(&tmp, text).map_err(io(&tmp))?;
    fs::rename(&tmp, &p).map_err(io(&p))
}

/// Appends the manifest the pointer names to the history when it is not the last entry.
/// Returns it, or `None` when the folder has no manifest yet.
pub fn record(out: &Path) -> Result<Option<ContentHash>, GcError> {
    let Some(current) = pointer(out)? else {
        return Ok(None);
    };
    let mut h = history(out)?;
    if h.last() != Some(&current) {
        h.push(current);
        write_history(out, &h)?;
    }
    Ok(Some(current))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcSummary {
    /// Manifests kept, oldest first (the current one is last).
    pub kept_manifests: Vec<ContentHash>,
    /// Manifests of the history that were dropped (older than the last N).
    pub dropped_manifests: usize,
    pub blobs_kept: usize,
    pub bytes_kept: u64,
    /// Blobs removed (or, with `dry_run`, that would be), with their sizes.
    pub removed: Vec<(ContentHash, u64)>,
    /// `<hash>.br` / `<hash>.zst` sidecars removed (or that would be) with their blob; their
    /// bytes count in [`removed_bytes`](Self::removed_bytes).
    pub removed_sidecars: Vec<(ContentHash, u64)>,
    pub dry_run: bool,
}

impl GcSummary {
    pub fn removed_bytes(&self) -> u64 {
        self.removed
            .iter()
            .chain(&self.removed_sidecars)
            .map(|(_, n)| n)
            .sum()
    }
}

/// Blob hash of a file name: the name itself, or the name without `.br` / `.zst` (sidecars made
/// by `precompress`). The bool tells a sidecar.
fn blob_of(name: &str) -> Option<(ContentHash, bool)> {
    let (stem, side) = match name
        .strip_suffix(".br")
        .or_else(|| name.strip_suffix(".zst"))
    {
        Some(stem) => (stem, true),
        None => (name, false),
    };
    let hash = stem.parse::<ContentHash>().ok()?;
    (hash.to_string() == stem).then_some((hash, side))
}

fn read_manifest(out: &Path, hash: &ContentHash) -> Result<Manifest, GcError> {
    let p = out.join(hash.to_string());
    let bytes = fs::read(&p).map_err(io(&p))?;
    Manifest::decode(&bytes)
        .map_err(|e| GcError::Invalid(format!("manifest {hash} ({}): {e}", p.display())))
}

/// Keeps the blobs of the last `keep` manifests of the history (the current one counts) and
/// removes every other hash-named file of `out`. With `dry_run` nothing is removed or written.
pub fn gc(out: &Path, keep: usize, dry_run: bool) -> Result<GcSummary, GcError> {
    if keep == 0 {
        return Err(GcError::Invalid(
            "--keep must be at least 1 (the current manifest)".to_owned(),
        ));
    }
    let current = pointer(out)?.ok_or_else(|| {
        GcError::Invalid(format!(
            "{} has no manifest pointer: nothing to keep, nothing removed",
            out.display()
        ))
    })?;
    let mut hist = history(out)?;
    if hist.last() != Some(&current) {
        hist.push(current);
    }
    let dropped_manifests = hist.len().saturating_sub(keep);
    let kept: Vec<ContentHash> = hist.split_off(dropped_manifests);

    // Every manifest to keep must read, or nothing is removed: a missing blob is not a reason to
    // delete more.
    let mut referenced: BTreeSet<ContentHash> = BTreeSet::new();
    for h in &kept {
        referenced.insert(*h);
        let m = read_manifest(out, h)?;
        for (_, variants) in m.iter() {
            referenced.extend(variants.iter().map(|v| v.hash));
        }
    }

    let mut removed = Vec::new();
    let (mut blobs_kept, mut bytes_kept) = (0usize, 0u64);
    let mut removed_sidecars = Vec::new();
    let mut candidates: Vec<(ContentHash, bool, PathBuf, u64)> = Vec::new();
    for e in fs::read_dir(out).map_err(io(out))? {
        let e = e.map_err(io(out))?;
        let name = e.file_name().to_string_lossy().into_owned();
        let Some((hash, sidecar)) = blob_of(&name) else {
            continue;
        };
        if !e.file_type().map_err(io(&e.path()))?.is_file() {
            continue;
        }
        let size = e.metadata().map_err(io(&e.path()))?.len();
        if referenced.contains(&hash) {
            if !sidecar {
                blobs_kept += 1;
                bytes_kept += size;
            }
        } else {
            candidates.push((hash, sidecar, e.path(), size));
        }
    }
    candidates.sort_by_key(|c| (c.0, c.1, c.2.clone()));
    for (hash, sidecar, path, size) in candidates {
        if !dry_run {
            fs::remove_file(&path).map_err(io(&path))?;
        }
        if sidecar {
            removed_sidecars.push((hash, size));
        } else {
            removed.push((hash, size));
        }
    }
    if !dry_run {
        write_history(out, &kept)?;
    }
    Ok(GcSummary {
        kept_manifests: kept,
        dropped_manifests,
        blobs_kept,
        bytes_kept,
        removed,
        removed_sidecars,
        dry_run,
    })
}

#[cfg(test)]
mod tests {
    use asset_format::{LogicalId, PrefetchGroup, Variant, VariantFormat};

    use super::*;
    use crate::store::{BlobStore, ManifestBuilder};

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("civ5-gc-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            Self(p)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A build: sets `id` to `bytes` in the manifest of the folder and records the history, as
    /// `build` does.
    fn build(out: &Path, items: &[(&str, &[u8])]) -> ContentHash {
        let store = BlobStore::open(out).unwrap();
        let mut m = ManifestBuilder::new();
        for (id, bytes) in items {
            let (hash, size) = store.put(bytes).unwrap();
            m.add(
                LogicalId::new(*id),
                Variant {
                    format: VariantFormat::Generic,
                    hash,
                    size,
                    prefetch: PrefetchGroup(0),
                },
            );
        }
        let h = m.finish(&store).unwrap();
        assert_eq!(record(out).unwrap(), Some(h));
        h
    }

    fn blobs(out: &Path) -> usize {
        fs::read_dir(out)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .parse::<ContentHash>()
                    .is_ok()
            })
            .count()
    }

    #[test]
    fn keeps_the_blobs_of_the_last_n_manifests_and_removes_the_rest() {
        let d = Dir::new("keep");
        // Three releases: `a` changes every time, `b` stays, `c` goes after the first.
        let m1 = build(&d.0, &[("tx:a", b"a1"), ("tx:b", b"bb"), ("tx:c", b"c1")]);
        let m2 = build(&d.0, &[("tx:a", b"a2"), ("tx:b", b"bb")]);
        let m3 = build(&d.0, &[("tx:a", b"a3"), ("tx:b", b"bb")]);
        // 3 manifests + a1 a2 a3 b c1.
        assert_eq!(blobs(&d.0), 3 + 5);
        assert_eq!(history(&d.0).unwrap(), [m1, m2, m3]);

        let dry = gc(&d.0, 2, true).unwrap();
        assert_eq!(blobs(&d.0), 8, "a dry run removes nothing");
        assert_eq!(dry.kept_manifests, [m2, m3]);
        assert_eq!(dry.dropped_manifests, 1);
        // m1 and the blobs only it needs: a1 and c1.
        assert_eq!(dry.removed.len(), 3);

        let s = gc(&d.0, 2, false).unwrap();
        assert_eq!(s.removed, dry.removed);
        assert_eq!(blobs(&d.0), 5);
        assert_eq!(s.blobs_kept, 5);
        assert_eq!(history(&d.0).unwrap(), [m2, m3]);
        assert!(!d.0.join(m1.to_string()).exists());
        assert!(d.0.join(m2.to_string()).is_file());
        // The pointer and the kept manifests still resolve.
        assert_eq!(pointer(&d.0).unwrap(), Some(m3));
        read_manifest(&d.0, &m2).unwrap();

        // Running again removes nothing; keeping 1 drops m2 and a2.
        assert!(gc(&d.0, 2, false).unwrap().removed.is_empty());
        let s = gc(&d.0, 1, false).unwrap();
        assert_eq!(s.removed.len(), 2, "m2 and a2");
        assert_eq!(history(&d.0).unwrap(), [m3]);
        assert_eq!(blobs(&d.0), 3, "m3, a3 and b");
    }

    #[test]
    fn files_that_are_not_blobs_stay() {
        let d = Dir::new("others");
        build(&d.0, &[("tx:a", b"a")]);
        for name in ["report-b2.json", "notes.txt"] {
            fs::write(d.0.join(name), "x").unwrap();
        }
        fs::create_dir_all(d.0.join(".cache")).unwrap();
        fs::write(d.0.join(".cache").join("e".repeat(64)), "x").unwrap();
        // A hash-like name in upper case is not a blob name of the store.
        fs::write(d.0.join("F".repeat(64)), "x").unwrap();
        let s = gc(&d.0, 1, false).unwrap();
        assert!(s.removed.is_empty());
        assert!(d.0.join("report-b2.json").is_file() && d.0.join("notes.txt").is_file());
        assert!(d.0.join(".cache").join("e".repeat(64)).is_file());
        assert!(d.0.join("F".repeat(64)).is_file());
    }

    #[test]
    fn sidecars_go_and_stay_with_their_blob() {
        let d = Dir::new("sidecars");
        let m1 = build(&d.0, &[("tx:a", b"a1"), ("tx:b", b"bb")]);
        let m2 = build(&d.0, &[("tx:a", b"a2"), ("tx:b", b"bb")]);
        let hash_of = |bytes: &[u8]| BlobStore::open(&d.0).unwrap().put(bytes).unwrap().0;
        let (a1, a2, b) = (hash_of(b"a1"), hash_of(b"a2"), hash_of(b"bb"));
        for h in [a1, a2, b, m1, m2] {
            fs::write(d.0.join(format!("{h}.br")), "br").unwrap();
            fs::write(d.0.join(format!("{h}.zst")), "zst").unwrap();
        }
        // Not sidecars: wrong stem, upper case, a double suffix, a stray one without its blob.
        fs::write(d.0.join("notes.br"), "x").unwrap();
        fs::write(d.0.join(format!("{}.br", "F".repeat(64))), "x").unwrap();
        fs::write(d.0.join(format!("{a2}.br.zst")), "x").unwrap();
        let stray = "c".repeat(64);
        fs::write(d.0.join(format!("{stray}.zst")), "x").unwrap();

        let dry = gc(&d.0, 1, true).unwrap();
        assert!(
            d.0.join(format!("{a1}.br")).is_file(),
            "a dry run removes nothing"
        );
        assert_eq!(dry.removed.len(), 2, "m1 and a1");
        // m1 and a1 have two sidecars each, the stray one has one.
        assert_eq!(dry.removed_sidecars.len(), 5);

        let s = gc(&d.0, 1, false).unwrap();
        assert_eq!(s.removed_sidecars, dry.removed_sidecars);
        assert!(s.removed_bytes() > s.removed.iter().map(|(_, n)| n).sum::<u64>());
        for gone in [m1, a1] {
            for ext in ["br", "zst"] {
                assert!(!d.0.join(format!("{gone}.{ext}")).exists());
            }
        }
        assert!(!d.0.join(format!("{stray}.zst")).exists());
        for kept in [m2, a2, b] {
            for ext in ["br", "zst"] {
                assert!(d.0.join(format!("{kept}.{ext}")).is_file());
            }
        }
        assert_eq!(s.blobs_kept, 3, "sidecars are not blobs");
        assert!(d.0.join("notes.br").is_file());
        assert!(d.0.join(format!("{}.br", "F".repeat(64))).is_file());
        assert!(d.0.join(format!("{a2}.br.zst")).is_file());
    }

    #[test]
    fn a_folder_without_history_keeps_the_current_manifest_and_a_broken_one_removes_nothing() {
        let d = Dir::new("nohist");
        let store = BlobStore::open(&d.0).unwrap();
        let mut m = ManifestBuilder::new();
        let (hash, size) = store.put(b"x").unwrap();
        m.add(
            LogicalId::new("tx:x"),
            Variant {
                format: VariantFormat::Generic,
                hash,
                size,
                prefetch: PrefetchGroup(0),
            },
        );
        let cur = m.finish(&store).unwrap();
        store.put(b"stray").unwrap();
        // No history file: the pointer starts it; the stray blob is no manifest's.
        let s = gc(&d.0, 2, false).unwrap();
        assert_eq!(s.kept_manifests, [cur]);
        assert_eq!(s.removed.len(), 1);

        // The pointer names a blob that is not a manifest: refuse, remove nothing.
        let (junk, _) = store.put(b"not a manifest").unwrap();
        fs::write(d.0.join("manifest"), format!("{junk}\n")).unwrap();
        let before = blobs(&d.0);
        assert!(gc(&d.0, 2, false).is_err());
        assert_eq!(blobs(&d.0), before);
        assert!(gc(&d.0, 0, false).is_err());
        assert!(gc(&d.0.join("nowhere"), 1, false).is_err());
    }
}
