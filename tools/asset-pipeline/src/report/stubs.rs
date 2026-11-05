//! Stand-ins and the list that allows them (4.15 items 7, 9 "verify").
//!
//! A *stub* is any stand-in for data of the original: a marker, a flat-colour texture, a default
//! material, a skipped clip. The B2 sets have no code path that makes one. If one ever does, it
//! records a [`StubEvent`] in `stubs-b2.json` next to the manifest, and `verify` fails unless the
//! id is in `stubs.allow` with a reason, an owner and a date.
//!
//! `stubs.allow` has one entry per line: `id | reason | owner | YYYY-MM-DD`; `#` starts a comment.
//! For AC-R3.1 it has no line about a unit.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The allow list, in the crate directory (not in the output directory: it is reviewed).
pub const STUBS_ALLOW_FILE: &str = "stubs.allow";
/// The events of the last builds, in the output directory.
pub const STUBS_LOG_FILE: &str = "stubs-b2.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StubEvent {
    /// Logical id of what was replaced.
    pub id: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowEntry {
    pub id: String,
    pub reason: String,
    pub owner: String,
    pub date: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StubAllow {
    pub entries: Vec<AllowEntry>,
}

/// `stubs.allow` or the log did not read.
#[derive(Debug, thiserror::Error)]
pub enum StubError {
    #[error("{path}: {msg}")]
    File { path: String, msg: String },
}

fn file_err(path: &Path, msg: impl Into<String>) -> StubError {
    StubError::File {
        path: path.display().to_string(),
        msg: msg.into(),
    }
}

/// `YYYY-MM-DD` with plausible month and day.
fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let num = |r: std::ops::Range<usize>| s[r].parse::<u32>().ok();
    matches!(
        (num(0..4), num(5..7), num(8..10)),
        (Some(_), Some(1..=12), Some(1..=31))
    ) && b
        .iter()
        .enumerate()
        .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// Ids that name a unit: the allow list must have none (AC-R3.1). Until the unit set exists the
/// pipeline cannot tell a unit model from a landmark model by id alone, so the test is by name.
// TODO(AB-9): with `b2-units` the list of unit ids comes from `units.csv`; replace this by it.
pub fn looks_like_unit(id: &str) -> bool {
    id.to_ascii_lowercase().contains("unit")
}

impl StubAllow {
    /// The text of a `stubs.allow`; `origin` names it in the errors.
    pub fn parse(text: &str, origin: &Path) -> Result<Self, StubError> {
        let mut entries = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim_start_matches('\u{feff}').trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.split('|').map(str::trim).collect();
            let at = format!("line {}", i + 1);
            let [id, reason, owner, date] = parts.as_slice() else {
                return Err(file_err(
                    origin,
                    format!(
                        "{at}: expected `id | reason | owner | date`, found {} field(s)",
                        parts.len()
                    ),
                ));
            };
            if [id, reason, owner].iter().any(|f| f.is_empty()) {
                return Err(file_err(
                    origin,
                    format!("{at}: id, reason and owner must not be empty"),
                ));
            }
            if !is_date(date) {
                return Err(file_err(
                    origin,
                    format!("{at}: {date:?} is not YYYY-MM-DD"),
                ));
            }
            entries.push(AllowEntry {
                id: (*id).to_owned(),
                reason: (*reason).to_owned(),
                owner: (*owner).to_owned(),
                date: (*date).to_owned(),
            });
        }
        Ok(Self { entries })
    }

    pub fn load(path: &Path) -> Result<Self, StubError> {
        let text = std::fs::read_to_string(path).map_err(|e| file_err(path, e.to_string()))?;
        Self::parse(&text, path)
    }

    /// `tools/asset-pipeline/stubs.allow`, next to this crate's `Cargo.toml`.
    pub fn default_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(STUBS_ALLOW_FILE)
    }

    pub fn allows(&self, id: &str) -> bool {
        self.entries.iter().any(|e| e.id == id)
    }

    /// Entries about units; AC-R3.1 wants none.
    pub fn unit_entries(&self, is_unit: impl Fn(&str) -> bool) -> Vec<&AllowEntry> {
        self.entries.iter().filter(|e| is_unit(&e.id)).collect()
    }

    /// Problems of the allow list and of the events: an event whose id is not allowed, an entry
    /// about a unit, an id listed twice.
    pub fn check(&self, events: &[StubEvent], is_unit: impl Fn(&str) -> bool) -> Vec<String> {
        let mut problems = Vec::new();
        for e in events {
            if !self.allows(&e.id) {
                problems.push(format!(
                    "stub {}: {} (not in {STUBS_ALLOW_FILE})",
                    e.id, e.reason
                ));
            }
        }
        for e in self.unit_entries(is_unit) {
            problems.push(format!(
                "{STUBS_ALLOW_FILE} allows a stub for {}: the list must have no unit (AC-R3.1)",
                e.id
            ));
        }
        for (i, e) in self.entries.iter().enumerate() {
            if self.entries[..i].iter().any(|o| o.id == e.id) {
                problems.push(format!("{STUBS_ALLOW_FILE} lists {} twice", e.id));
            }
        }
        problems
    }
}

/// The events of the output directory; none when there is no log.
pub fn load_log(out: &Path) -> Result<Vec<StubEvent>, StubError> {
    let path = out.join(STUBS_LOG_FILE);
    match std::fs::read(&path) {
        Ok(b) => serde_json::from_slice(&b).map_err(|e| file_err(&path, e.to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(file_err(&path, e.to_string())),
    }
}

/// Adds an event to the log of `out`. No set calls this today: the pipeline has no stand-in path.
pub fn record(out: &Path, event: StubEvent) -> Result<(), StubError> {
    let mut log = load_log(out)?;
    if !log.contains(&event) {
        log.push(event);
    }
    let path = out.join(STUBS_LOG_FILE);
    let bytes = serde_json::to_vec_pretty(&log).map_err(|e| file_err(&path, e.to_string()))?;
    std::fs::write(&path, bytes).map_err(|e| file_err(&path, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "# header\n\
        \n\
        tx:flowfield.dds | no CTAB sampler yet | world-overlays | 2026-10-21\n\
        md:landmark_x | model has no gr2 in the install | assets-b2 | 2026-10-22\n";

    #[test]
    fn parses_entries_and_skips_comments() {
        let a = StubAllow::parse(TEXT, Path::new("stubs.allow")).unwrap();
        assert_eq!(a.entries.len(), 2);
        assert_eq!(a.entries[0].owner, "world-overlays");
        assert!(a.allows("md:landmark_x") && !a.allows("md:other"));
        assert!(
            StubAllow::parse("# only a comment\n", Path::new("f"))
                .unwrap()
                .entries
                .is_empty()
        );
    }

    #[test]
    fn malformed_lines_are_errors_that_name_the_line() {
        for (bad, want) in [
            ("a | b | c\n", "3 field"),
            ("a | b | c | d | e\n", "5 field"),
            ("a | | c | 2026-10-21\n", "must not be empty"),
            ("a | b | c | 21.10.2026\n", "not YYYY-MM-DD"),
            ("a | b | c | 2026-13-01\n", "not YYYY-MM-DD"),
        ] {
            let e = StubAllow::parse(bad, Path::new("f"))
                .unwrap_err()
                .to_string();
            assert!(e.contains("line 1") && e.contains(want), "{bad:?}: {e}");
        }
    }

    #[test]
    fn an_event_that_is_not_allowed_a_unit_entry_and_a_repeat_are_problems() {
        let a = StubAllow::parse(TEXT, Path::new("f")).unwrap();
        let ev = |id: &str| StubEvent {
            id: id.into(),
            reason: "r".into(),
        };
        assert!(a.check(&[ev("md:landmark_x")], looks_like_unit).is_empty());
        let p = a.check(&[ev("md:archer")], looks_like_unit);
        assert_eq!(p.len(), 1);
        assert!(p[0].contains("md:archer") && p[0].contains("not in stubs.allow"));

        let with_unit = StubAllow::parse(
            "md:unit_archer | no mesh | assets-b2 | 2026-10-21\n",
            Path::new("f"),
        )
        .unwrap();
        let p = with_unit.check(&[], looks_like_unit);
        assert!(p[0].contains("no unit"), "{p:?}");

        let twice = StubAllow::parse(
            "x | a | o | 2026-10-21\nx | b | o | 2026-10-21\n",
            Path::new("f"),
        )
        .unwrap();
        assert!(twice.check(&[], looks_like_unit)[0].contains("twice"));
    }

    #[test]
    fn the_log_is_written_without_repeats_and_read_back() {
        let dir = std::env::temp_dir().join(format!("civ5-stubs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(load_log(&dir).unwrap().is_empty());
        let e = StubEvent {
            id: "tx:x".into(),
            reason: "r".into(),
        };
        record(&dir, e.clone()).unwrap();
        record(&dir, e.clone()).unwrap();
        assert_eq!(load_log(&dir).unwrap(), [e]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_file_in_the_crate_has_no_entries() {
        // The checked-in list is empty: no stub is allowed today.
        let a = StubAllow::load(&StubAllow::default_path()).unwrap();
        assert!(a.entries.is_empty(), "{:?}", a.entries);
    }
}
