//! Atomic, versioned files in `state_dir` (GS-D10).
//!
//! A write goes to `<name>.tmp`, is flushed to disk, then renamed over the target, so a crash
//! leaves either the old or the new file. JSON files carry a `format` number next to their data;
//! a file newer than the code understands is a startup error, not silently misread.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Who may read the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileMode {
    /// Default permissions.
    Normal,
    /// Owner only (0600 on Unix). Tokens and identities.
    Private,
}

/// Replaces `path` with `bytes` atomically (`.tmp`, `fsync`, rename).
pub fn write_atomic(path: &Path, bytes: &[u8], mode: FileMode) -> anyhow::Result<()> {
    let tmp = tmp_path(path);
    let result = (|| -> anyhow::Result<()> {
        let mut file =
            create_tmp(&tmp, mode).with_context(|| format!("cannot create {}", tmp.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("cannot flush {}", tmp.display()))?;
        drop(file);
        fs::rename(&tmp, path).with_context(|| format!("cannot replace {}", path.display()))?;
        sync_parent(path);
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".tmp");
    PathBuf::from(name)
}

fn create_tmp(tmp: &Path, mode: FileMode) -> std::io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if mode == FileMode::Private {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = mode;
    opts.open(tmp)
}

/// Makes the rename durable. Best effort: not every platform can fsync a directory.
fn sync_parent(path: &Path) {
    #[cfg(unix)]
    if let Some(parent) = path.parent()
        && let Ok(dir) = File::open(parent)
    {
        let _ = dir.sync_all();
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Writes `value` (which must serialize to a JSON object) with a `format` field added.
pub fn write_versioned<T: Serialize>(
    path: &Path,
    format: u32,
    value: &T,
    mode: FileMode,
) -> anyhow::Result<()> {
    let mut json = serde_json::to_value(value).context("cannot serialize state file")?;
    let Some(obj) = json.as_object_mut() else {
        bail!("{}: a versioned file must be a JSON object", path.display());
    };
    if obj.contains_key("format") {
        bail!("{}: the field name `format` is reserved", path.display());
    }
    obj.insert("format".to_string(), format.into());
    let mut bytes = serde_json::to_vec_pretty(&json).context("cannot encode state file")?;
    bytes.push(b'\n');
    write_atomic(path, &bytes, mode)
}

/// Reads a file written by [`write_versioned`]. `Ok(None)` if it does not exist. A `format`
/// above `known` (written by a newer build) is an error with a clear text.
pub fn read_versioned<T: DeserializeOwned>(path: &Path, known: u32) -> anyhow::Result<Option<T>> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    let mut json: serde_json::Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("{} is not valid JSON", path.display()))?;
    let format = json
        .as_object_mut()
        .and_then(|o| o.remove("format"))
        .and_then(|f| f.as_u64())
        .with_context(|| format!("{} has no numeric `format` field", path.display()))?;
    if format > u64::from(known) {
        bail!(
            "{} has format {format}, but this build understands up to {known}; \
             it was written by a newer civ5-server",
            path.display()
        );
    }
    let value = serde_json::from_value(json)
        .with_context(|| format!("{} does not match format {format}", path.display()))?;
    Ok(Some(value))
}

/// `bytes` random bytes as lowercase hex (tokens, admin token).
pub fn random_hex(bytes: usize) -> anyhow::Result<String> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|e| anyhow::anyhow!("no system randomness: {e}"))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Doc {
        records: Vec<String>,
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("civ5-files-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn roundtrip_and_replace() {
        let dir = scratch("rt");
        let path = dir.join("doc.json");
        assert_eq!(read_versioned::<Doc>(&path, 1).unwrap(), None);
        let a = Doc {
            records: vec!["a".into()],
        };
        write_versioned(&path, 1, &a, FileMode::Normal).unwrap();
        assert_eq!(read_versioned::<Doc>(&path, 1).unwrap(), Some(a));
        let b = Doc {
            records: vec!["b".into(), "c".into()],
        };
        write_versioned(&path, 1, &b, FileMode::Private).unwrap();
        assert_eq!(read_versioned::<Doc>(&path, 1).unwrap(), Some(b));
        assert!(!tmp_path(&path).exists(), "tmp file must not remain");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn newer_format_is_refused() {
        let dir = scratch("newer");
        let path = dir.join("doc.json");
        write_versioned(&path, 2, &Doc { records: vec![] }, FileMode::Normal).unwrap();
        let err = read_versioned::<Doc>(&path, 1).unwrap_err().to_string();
        assert!(err.contains("format 2") && err.contains("up to 1"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn damaged_files_are_errors() {
        let dir = scratch("bad");
        let path = dir.join("doc.json");
        fs::write(&path, b"{not json").unwrap();
        assert!(read_versioned::<Doc>(&path, 1).is_err());
        fs::write(&path, br#"{"records":[]}"#).unwrap();
        assert!(read_versioned::<Doc>(&path, 1).is_err(), "missing format");
        fs::write(&path, br#"{"format":1,"records":5}"#).unwrap();
        assert!(read_versioned::<Doc>(&path, 1).is_err(), "wrong shape");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_write_leaves_old_file() {
        let dir = scratch("fail");
        let path = dir.join("doc.json");
        let a = Doc {
            records: vec!["keep".into()],
        };
        write_versioned(&path, 1, &a, FileMode::Normal).unwrap();
        // The target is a directory now: the rename must fail and the tmp file must go.
        let blocked = dir.join("blocked.json");
        fs::create_dir(&blocked).unwrap();
        assert!(write_atomic(&blocked, b"x", FileMode::Normal).is_err());
        assert!(!tmp_path(&blocked).exists());
        assert_eq!(read_versioned::<Doc>(&path, 1).unwrap(), Some(a));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reserved_and_non_object_are_refused() {
        let dir = scratch("shape");
        let path = dir.join("doc.json");
        assert!(write_versioned(&path, 1, &vec![1, 2], FileMode::Normal).is_err());
        assert!(!path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn random_hex_has_requested_length_and_differs() {
        let a = random_hex(16).unwrap();
        let b = random_hex(16).unwrap();
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
    }

    #[cfg(unix)]
    #[test]
    fn private_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("mode");
        let path = dir.join("secret");
        write_atomic(&path, b"x", FileMode::Private).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
