//! `state_dir` layout and the single-gateway lock.
//!
//! Only one gateway may use a `state_dir` at a time: [`StateDir::open`] takes an exclusive lock
//! on `server.lock` and holds it until the value is dropped (or the process dies).

pub mod files;
pub mod meta;
pub mod retention;
pub mod sha256;

use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

use files::FileMode;

/// Current format of `identities.json` and the other state files this module will grow.
pub const FORMAT: u32 = 1;

/// An opened, locked `state_dir`.
#[derive(Debug)]
pub struct StateDir {
    root: PathBuf,
    /// Held for the lifetime of the gateway; the OS releases the lock when it is closed.
    _lock: File,
}

impl StateDir {
    /// Creates the directory tree if needed and takes `server.lock`. Fails at once if another
    /// gateway holds the lock.
    pub fn open(root: &Path) -> anyhow::Result<Self> {
        fs::create_dir_all(root)
            .with_context(|| format!("cannot create state_dir {}", root.display()))?;
        let lock_path = root.join("server.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("cannot open {}", lock_path.display()))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => bail!(
                "state_dir {} is in use by another civ5-server (it holds {})",
                root.display(),
                lock_path.display()
            ),
            Err(TryLockError::Error(e)) => {
                return Err(e).with_context(|| format!("cannot lock {}", lock_path.display()));
            }
        }
        for sub in ["catalog", "games", "logs"] {
            let dir = root.join(sub);
            fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
        }
        Ok(Self {
            root: root.to_path_buf(),
            _lock: lock,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `identities.json`.
    pub fn identities_path(&self) -> PathBuf {
        self.root.join("identities.json")
    }

    /// `ids.json`: next game and room ids.
    pub fn ids_path(&self) -> PathBuf {
        self.root.join("ids.json")
    }

    /// `admin.token`.
    pub fn admin_token_path(&self) -> PathBuf {
        self.root.join("admin.token")
    }

    /// `catalog/`: immutable game catalogs by hash.
    pub fn catalog_dir(&self) -> PathBuf {
        self.root.join("catalog")
    }

    /// `games/<id>/`: meta and saves of one game (created by the game actor).
    pub fn game_dir(&self, id: u64) -> PathBuf {
        self.root.join("games").join(id.to_string())
    }

    /// `logs/`.
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// Returns the admin token, creating a random 128-bit one (mode 0600) on first start.
    pub fn ensure_admin_token(&self) -> anyhow::Result<String> {
        let path = self.admin_token_path();
        read_admin_token(&path).or_else(|_| {
            let token = files::random_hex(16)?;
            files::write_atomic(&path, token.as_bytes(), FileMode::Private)?;
            Ok(token)
        })
    }
}

/// Reads `admin.token` from a state dir without locking it (the `admin` subcommand runs next
/// to a live gateway).
pub fn read_admin_token(path: &Path) -> anyhow::Result<String> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("cannot read admin token {}", path.display()))?;
    let token = text.trim();
    if token.is_empty() {
        bail!("admin token file {} is empty", path.display());
    }
    Ok(token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("civ5-state-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn second_gateway_on_same_state_dir_is_refused() {
        let dir = scratch("lock");
        let first = StateDir::open(&dir).unwrap();
        let err = StateDir::open(&dir).unwrap_err().to_string();
        assert!(err.contains("in use by another civ5-server"), "{err}");
        drop(first);
        // The lock is released with the first gateway.
        StateDir::open(&dir).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn different_state_dirs_do_not_conflict() {
        let a = scratch("a");
        let b = scratch("b");
        let _sa = StateDir::open(&a).unwrap();
        let _sb = StateDir::open(&b).unwrap();
        let _ = fs::remove_dir_all(&a);
        let _ = fs::remove_dir_all(&b);
    }

    #[test]
    fn layout_is_created() {
        let dir = scratch("layout");
        let s = StateDir::open(&dir).unwrap();
        assert!(s.catalog_dir().is_dir());
        assert!(s.logs_dir().is_dir());
        assert!(s.root().join("games").is_dir());
        assert_eq!(s.game_dir(7), dir.join("games").join("7"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn admin_token_is_created_once_and_stable() {
        let dir = scratch("token");
        let s = StateDir::open(&dir).unwrap();
        let t1 = s.ensure_admin_token().unwrap();
        let t2 = s.ensure_admin_token().unwrap();
        assert_eq!(t1.len(), 32);
        assert_eq!(t1, t2);
        assert_eq!(read_admin_token(&s.admin_token_path()).unwrap(), t1);
        let _ = fs::remove_dir_all(&dir);
    }
}
