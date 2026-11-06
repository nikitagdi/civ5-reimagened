//! `assets.lock`: the digests an asset import recorded, and the check of a pack against them.
//!
//! ```text
//! c5-assets-lock 1
//! source minecraft-1.21.1-client.jar <sha256>
//! file textures/dirt <sha256>
//! ```
//!
//! `source` names the external inputs (a game jar; the name carries the game version), `file` the logical id
//! and the digest of the bytes as they sit in the pack. Sorted by name / id, LF, final newline.

use std::collections::BTreeMap;

use crate::error::{LockProblem, PackError, VerifyError};
use crate::pack::{Pack, Source, valid_asset_id};
use crate::sha256::Sha256;

const LOCK_HEADER: &str = "c5-assets-lock 1";

/// A parsed `assets.lock`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AssetsLock {
    pub sources: Vec<Source>,
    pub files: Vec<(String, Sha256)>,
}

impl AssetsLock {
    /// Records what `pack` is made of: the lock an import would write for it.
    pub fn of_pack(pack: &Pack) -> AssetsLock {
        AssetsLock {
            sources: pack.manifest().sources().to_vec(),
            files: pack
                .manifest()
                .files()
                .iter()
                .map(|f| (f.id.clone(), f.sha256))
                .collect(),
        }
    }

    /// Canonical text (sources and files sorted).
    pub fn to_text(&self) -> String {
        let mut sources = self.sources.clone();
        sources.sort_by(|a, b| a.name.cmp(&b.name));
        let mut files = self.files.clone();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let mut out = format!("{LOCK_HEADER}\n");
        for s in &sources {
            out.push_str(&format!("source {} {}\n", s.name, s.sha256));
        }
        for (id, sha) in &files {
            out.push_str(&format!("file {id} {sha}\n"));
        }
        out
    }

    /// Strict parse: unknown records, bad ids or digests, duplicates, disorder and a missing final newline fail.
    pub fn parse_text(text: &str) -> Result<AssetsLock, PackError> {
        let err = |line: usize, msg: &str| PackError::Lock {
            line,
            msg: msg.to_string(),
        };
        if !text.ends_with('\n') {
            return Err(err(0, "missing final newline"));
        }
        let mut lines = text[..text.len() - 1].split('\n').enumerate();
        match lines.next() {
            Some((_, l)) if l == LOCK_HEADER => {}
            _ => return Err(err(1, "expected the header `c5-assets-lock 1`")),
        }
        let mut lock = AssetsLock::default();
        for (i, line) in lines {
            let n = i + 1;
            let parts: Vec<&str> = line.split(' ').collect();
            match parts.as_slice() {
                ["source", name, sha] => {
                    if !lock.files.is_empty() {
                        return Err(err(n, "`source` records come before `file`"));
                    }
                    let sha256 = sha.parse().map_err(|_| err(n, "bad sha256"))?;
                    if lock
                        .sources
                        .last()
                        .is_some_and(|l| l.name.as_str() >= *name)
                    {
                        return Err(err(n, "sources must be unique and sorted by name"));
                    }
                    lock.sources.push(Source {
                        name: (*name).to_string(),
                        sha256,
                    });
                }
                ["file", id, sha] => {
                    if !valid_asset_id(id) {
                        return Err(err(n, "invalid asset id"));
                    }
                    let sha = sha.parse().map_err(|_| err(n, "bad sha256"))?;
                    if lock.files.last().is_some_and(|l| l.0.as_str() >= *id) {
                        return Err(err(n, "files must be unique and sorted by id"));
                    }
                    lock.files.push(((*id).to_string(), sha));
                }
                _ => return Err(err(n, "unknown or malformed record")),
            }
        }
        Ok(lock)
    }
}

/// Checks `pack` against `lock`: the sources the pack records, and the digest of every file, must equal the
/// lock's, with no file on one side only. Reports every disagreement, not only the first.
pub fn verify_against_lock(pack: &Pack, lock: &AssetsLock) -> Result<(), VerifyError> {
    let mut problems = Vec::new();

    let locked_src: BTreeMap<&str, Sha256> = lock
        .sources
        .iter()
        .map(|s| (s.name.as_str(), s.sha256))
        .collect();
    let packed_src: BTreeMap<&str, Sha256> = pack
        .manifest()
        .sources()
        .iter()
        .map(|s| (s.name.as_str(), s.sha256))
        .collect();
    for (name, expected) in &locked_src {
        match packed_src.get(name) {
            None => problems.push(LockProblem::SourceMissing((*name).to_string())),
            Some(actual) if actual != expected => problems.push(LockProblem::SourceMismatch {
                name: (*name).to_string(),
                expected: *expected,
                actual: *actual,
            }),
            Some(_) => {}
        }
    }
    for name in packed_src.keys() {
        if !locked_src.contains_key(name) {
            problems.push(LockProblem::SourceExtra((*name).to_string()));
        }
    }

    let locked: BTreeMap<&str, Sha256> = lock.files.iter().map(|(i, s)| (i.as_str(), *s)).collect();
    for (id, expected) in &locked {
        match pack.manifest().file(id) {
            None => problems.push(LockProblem::FileMissing((*id).to_string())),
            Some(f) if f.sha256 != *expected => problems.push(LockProblem::FileMismatch {
                id: (*id).to_string(),
                expected: *expected,
                actual: f.sha256,
            }),
            Some(_) => {}
        }
    }
    for f in pack.manifest().files() {
        if !locked.contains_key(f.id.as_str()) {
            problems.push(LockProblem::FileExtra(f.id.clone()));
        }
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(VerifyError(problems))
    }
}
