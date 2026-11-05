//! A small key-value store in a file (`KeyValueStore` of the desktop): the choices of the device that
//! must survive a restart and must not follow the account (the graphics choices, `civ5.gfx.*`).
//!
//! The file has one `key=value` per line, keys and values without line breaks. It is written whole on
//! every change (a few small values). A file that cannot be read is an empty store and one that cannot
//! be written keeps the values in memory for the run; the caller carries on either way.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use platform::KeyValueStore;

pub struct FileStore {
    path: PathBuf,
    items: Mutex<BTreeMap<String, String>>,
}

impl FileStore {
    /// The store in `path`; a file that is not there yet is made on the first write.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let items = std::fs::read_to_string(&path)
            .map(|text| parse(&text))
            .unwrap_or_default();
        Self {
            path,
            items: Mutex::new(items),
        }
    }

    /// The default place: `$CIV5_CONFIG_DIR`, else the config folder of the user (`%APPDATA%`,
    /// `$XDG_CONFIG_HOME` or `~/.config`), else the working folder; the file is `device-prefs.txt`
    /// in a `civ5-reimagened` folder there.
    pub fn default_path() -> PathBuf {
        let from_env = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let dir = from_env("CIV5_CONFIG_DIR")
            .or_else(|| from_env("APPDATA").map(|d| d.join("civ5-reimagened")))
            .or_else(|| from_env("XDG_CONFIG_HOME").map(|d| d.join("civ5-reimagened")))
            .or_else(|| from_env("HOME").map(|d| d.join(".config").join("civ5-reimagened")))
            .unwrap_or_default();
        dir.join("device-prefs.txt")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn save(&self, items: &BTreeMap<String, String>) {
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty())
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            tracing::warn!("cannot make {}: {e}", dir.display());
            return;
        }
        if let Err(e) = std::fs::write(&self.path, render(items)) {
            tracing::warn!("cannot write {}: {e}", self.path.display());
        }
    }
}

fn parse(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(k, _)| !k.trim().is_empty())
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect()
}

fn render(items: &BTreeMap<String, String>) -> String {
    items.iter().map(|(k, v)| format!("{k}={v}\n")).collect()
}

/// A key or value with a line break or (in a key) an `=` cannot be kept in the file.
fn storable(key: &str, value: &str) -> bool {
    !key.is_empty() && !key.contains(['=', '\n', '\r']) && !value.contains(['\n', '\r'])
}

impl KeyValueStore for FileStore {
    fn get(&self, key: &str) -> Option<String> {
        self.items.lock().ok()?.get(key).cloned()
    }

    fn set(&self, key: &str, value: &str) {
        if !storable(key, value) {
            return;
        }
        if let Ok(mut items) = self.items.lock() {
            items.insert(key.to_owned(), value.trim().to_owned());
            self.save(&items);
        }
    }

    fn remove(&self, key: &str) {
        if let Ok(mut items) = self.items.lock()
            && items.remove(key).is_some()
        {
            self.save(&items);
        }
    }

    fn keys_with_prefix(&self, prefix: &str) -> Vec<String> {
        self.items
            .lock()
            .map(|items| {
                items
                    .keys()
                    .filter(|k| k.starts_with(prefix))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("civ5-kv-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("prefs.txt")
    }

    #[test]
    fn values_survive_a_reopen_and_a_removal_too() {
        let path = temp("reopen");
        let a = FileStore::open(&path);
        assert_eq!(a.get("civ5.gfx.Msaa"), None);
        a.set("civ5.gfx.Msaa", "4");
        a.set("civ5.gfx.RenderScale", "60");
        let b = FileStore::open(&path);
        assert_eq!(b.get("civ5.gfx.Msaa").as_deref(), Some("4"));
        assert_eq!(
            b.keys_with_prefix("civ5.gfx."),
            ["civ5.gfx.Msaa", "civ5.gfx.RenderScale"]
        );
        b.remove("civ5.gfx.Msaa");
        assert_eq!(FileStore::open(&path).get("civ5.gfx.Msaa"), None);
        assert_eq!(
            FileStore::open(&path)
                .get("civ5.gfx.RenderScale")
                .as_deref(),
            Some("60")
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_missing_or_broken_file_is_an_empty_store_and_odd_keys_are_refused() {
        let path = temp("broken");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "no equals sign\n=empty key\nk=v\n").unwrap();
        let s = FileStore::open(&path);
        assert_eq!(s.get("k").as_deref(), Some("v"));
        assert_eq!(s.keys_with_prefix(""), ["k"]);
        s.set("a=b", "1");
        s.set("a", "line\nbreak");
        assert_eq!((s.get("a=b"), s.get("a")), (None, None));
        assert_eq!(FileStore::open(temp("absent")).get("k"), None);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
