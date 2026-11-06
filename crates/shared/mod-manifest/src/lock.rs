//! `mods.lock`: a TOML-shaped text file that freezes a `ModSet`. Serialization is byte-stable (fixed key order,
//! LF line endings, trailing newline); parsing is strict and verifies `modset_hash`.
//!
//! ```text
//! # mods.lock - written by `cargo xtask mods build`; do not edit by hand.
//! version = 1
//! modset_hash = "<32 hex>"
//!
//! [[mod]]
//! id = "a"
//! version = "1.0.0"
//! load_idx = 0
//! sync = "gameplay"
//! client = "required"
//! parts = ["data", "server-rs"]
//! content_hash = "<64 hex>"
//! ```

use std::collections::BTreeSet;
use std::fmt::Write;

use toml::Value;

use crate::error::{LockError, ManifestError};
use crate::manifest::{ClientPolicy, ModId, Part, SyncClass};
use crate::modset::{ContentHash, ModSet, ModsetHash, ResolvedMod};
use crate::reader::{Res, Tbl, wrong_type};
use crate::semver::Version;

pub const LOCK_VERSION: i64 = 1;
const HEADER: &str = "# mods.lock - written by `cargo xtask mods build`; do not edit by hand.\n";

/// Serializes the set. The same set always gives the same bytes.
pub fn write_lock(set: &ModSet) -> String {
    let mut s = String::new();
    s.push_str(HEADER);
    // Writing to a String cannot fail.
    let _ = writeln!(s, "version = {LOCK_VERSION}");
    let _ = writeln!(s, "modset_hash = \"{}\"", set.modset_hash());
    for m in set.mods() {
        s.push('\n');
        s.push_str("[[mod]]\n");
        let _ = writeln!(s, "id = \"{}\"", m.id);
        let _ = writeln!(s, "version = \"{}\"", m.version);
        let _ = writeln!(s, "load_idx = {}", m.load_idx);
        let _ = writeln!(s, "sync = \"{}\"", m.sync.as_str());
        let _ = writeln!(s, "client = \"{}\"", m.client.as_str());
        let parts: Vec<String> = m
            .parts
            .iter()
            .map(|p| format!("\"{}\"", p.as_str()))
            .collect();
        let _ = writeln!(s, "parts = [{}]", parts.join(", "));
        let _ = writeln!(s, "content_hash = \"{}\"", m.content_hash);
    }
    s
}

/// Parses a lock file and checks that `modset_hash` matches the mods listed in it.
pub fn parse_lock(src: &str) -> Result<ModSet, LockError> {
    let table: toml::Table = src
        .parse()
        .map_err(|e: toml::de::Error| ManifestError::Syntax(e.to_string()))?;
    let mut root = Tbl::new(&table, "");
    let version = root
        .int_opt("version", i64::MIN, i64::MAX)?
        .ok_or_else(|| ManifestError::Missing {
            path: "version".to_string(),
        })?;
    if version != LOCK_VERSION {
        return Err(LockError::Version(version));
    }
    let hash_s = root.str_req("modset_hash")?;
    let stated = ModsetHash::from_hex(hash_s)
        .ok_or_else(|| ManifestError::invalid("modset_hash", "expected 32 lowercase hex digits"))?;
    let mut mods = Vec::new();
    if let Some(v) = root.get("mod") {
        let Value::Array(items) = v else {
            return Err(wrong_type("mod", "array of tables ([[mod]])", v).into());
        };
        for (i, item) in items.iter().enumerate() {
            mods.push(parse_mod(item, &format!("mod[{i}]"))?);
        }
    }
    root.finish()?;
    let set = ModSet::new(mods)?;
    let actual = set.modset_hash();
    if actual != stated {
        return Err(LockError::HashMismatch {
            expected: actual.to_string(),
            found: stated.to_string(),
        });
    }
    Ok(set)
}

fn parse_mod(v: &Value, path: &str) -> Res<ResolvedMod> {
    let mut t = Tbl::from_value(v, path)?;
    let id =
        ModId::new(t.str_req("id")?).map_err(|m| ManifestError::invalid(t.path_of("id"), m))?;
    let version = t
        .str_req("version")?
        .parse::<Version>()
        .map_err(|e| ManifestError::invalid(t.path_of("version"), e.to_string()))?;
    let load_idx = t
        .int_opt("load_idx", 0, i64::from(u16::MAX))?
        .ok_or_else(|| ManifestError::Missing {
            path: t.path_of("load_idx"),
        })?;
    let sync_s = t.str_req("sync")?;
    let sync = SyncClass::parse(sync_s).ok_or_else(|| {
        ManifestError::invalid(t.path_of("sync"), format!("unknown sync class {sync_s:?}"))
    })?;
    let client_s = t.str_req("client")?;
    let client = ClientPolicy::parse(client_s).ok_or_else(|| {
        ManifestError::invalid(
            t.path_of("client"),
            format!("unknown client policy {client_s:?}"),
        )
    })?;
    let mut parts = BTreeSet::new();
    let items = t
        .strings_opt("parts")?
        .ok_or_else(|| ManifestError::Missing {
            path: t.path_of("parts"),
        })?;
    for (p, s) in items {
        let part = Part::parse(s)
            .ok_or_else(|| ManifestError::invalid(p.clone(), format!("unknown part {s:?}")))?;
        if !parts.insert(part) {
            return Err(ManifestError::invalid(
                p,
                format!("part {s:?} is listed twice"),
            ));
        }
    }
    let content_hash = ContentHash::from_hex(t.str_req("content_hash")?).ok_or_else(|| {
        ManifestError::invalid(
            t.path_of("content_hash"),
            "expected 64 lowercase hex digits",
        )
    })?;
    t.finish()?;
    Ok(ResolvedMod {
        id,
        version,
        // int_opt bounded the value to the u16 range.
        load_idx: load_idx as u16,
        sync,
        client,
        parts,
        content_hash,
    })
}
