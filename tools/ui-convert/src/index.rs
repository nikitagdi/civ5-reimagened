//! The skins of a web build: the package files under `<web>/ui2/` and the index `skins.json` next to them.
//!
//! The index is a JSON array of `{id, file, hash, title_key, themes, api, default}`, the shape the application's
//! skin registry reads. Every field comes from the package itself (its manifest and its hash), so the index can
//! never disagree with the file it names.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use ui2_package::Package;

/// The folder of the skins inside a web output.
pub const UI2_DIR: &str = "ui2";

/// The index file name.
pub const INDEX_FILE: &str = "skins.json";

/// One skin of the index: the package file name (`<id>-<hash16>.ui2`) and its entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The package file name.
    pub file: String,
    /// The JSON object of the index.
    pub json: Value,
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from(DIGITS[usize::from(b >> 4)]));
        s.push(char::from(DIGITS[usize::from(b & 15)]));
    }
    s
}

/// The index entry of a package: reads the manifest and takes the full hash. `file` is the name the package is
/// stored under.
pub fn entry_of(file: &str, bytes: &[u8]) -> Result<Entry, String> {
    let pkg = Package::read(std::sync::Arc::from(bytes)).map_err(|e| format!("{file}: {e}"))?;
    let m = pkg.manifest();
    let hash = hex(&pkg.hash().0);
    let want = format!("{}-{}.ui2", m.id, &hash[..16]);
    if file != want {
        return Err(format!(
            "{file}: the name does not match the package (expected {want})"
        ));
    }
    Ok(Entry {
        file: file.to_string(),
        json: json!({
            "id": m.id,
            "file": file,
            "hash": hash,
            "title_key": m.title_key,
            "themes": m.themes,
            "api": format!("{}.{}", m.api.0, m.api.1),
            "default": m.default,
        }),
    })
}

/// The text of `skins.json` for entries sorted by id.
pub fn index_text(entries: &[Entry]) -> String {
    let mut list: Vec<&Entry> = entries.iter().collect();
    list.sort_by(|a, b| a.json["id"].as_str().cmp(&b.json["id"].as_str()));
    let v = Value::Array(list.iter().map(|e| e.json.clone()).collect());
    serde_json::to_string_pretty(&v).unwrap_or_default() + "\n"
}

/// Puts the packages into `<web>/ui2/` and writes `skins.json`: older `.ui2` files of the folder are removed, so
/// the folder holds exactly the files the index lists. Returns the entries.
pub fn publish(web: &Path, packages: &[PathBuf]) -> Result<Vec<Entry>, String> {
    let dir = web.join(UI2_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let mut entries: Vec<(Entry, Vec<u8>)> = Vec::new();
    for p in packages {
        let file = p
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("{}: not a file name", p.display()))?;
        let bytes = std::fs::read(p).map_err(|e| format!("cannot read {}: {e}", p.display()))?;
        let entry = entry_of(file, &bytes)?;
        if entries
            .iter()
            .any(|(o, _)| o.json["id"] == entry.json["id"])
        {
            return Err(format!("skin {} is built twice", entry.json["id"]));
        }
        entries.push((entry, bytes));
    }
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.ends_with(".ui2") && !entries.iter().any(|(en, _)| en.file == n) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    for (e, bytes) in &entries {
        std::fs::write(dir.join(&e.file), bytes)
            .map_err(|er| format!("cannot write {}: {er}", e.file))?;
    }
    let entries: Vec<Entry> = entries.into_iter().map(|(e, _)| e).collect();
    std::fs::write(dir.join(INDEX_FILE), index_text(&entries))
        .map_err(|e| format!("cannot write {INDEX_FILE}: {e}"))?;
    Ok(entries)
}
