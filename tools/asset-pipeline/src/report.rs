//! Build report of the B1 sets: `report-b1.json` and `report-b1.txt`
//! next to the manifest in the output directory.
//!
//! Every set contributes a [`SetReport`] (entries, counts, bytes) under its own name. A build of
//! some sets only replaces those sets' sections, so the report always describes the manifest in
//! the output directory.

pub mod budget;
pub mod coverage;
pub mod stubs;
pub mod texaudit;
pub mod verify;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const REPORT_JSON: &str = "report-b1.json";
pub const REPORT_TXT: &str = "report-b1.txt";
const VERSION: u32 = 1;

/// One finding. `kind` is a short stable name (`placeholder`, `missing-icon`, `dropped-mesh`,
/// `material-ambiguous`, ...), `subject` what it is about, `reason` the explanation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportEntry {
    pub kind: String,
    pub subject: String,
    pub reason: String,
}

impl ReportEntry {
    pub fn new(kind: &str, subject: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            kind: kind.to_owned(),
            subject: subject.into(),
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetReport {
    pub entries: Vec<ReportEntry>,
    /// Named counts (`units`, `placeholders`, `textures`, ...).
    pub counts: BTreeMap<String, u64>,
    /// Named byte sizes (`models`, `textures Bc`, ...).
    pub bytes: BTreeMap<String, u64>,
}

impl SetReport {
    /// Entries per kind.
    pub fn kinds(&self) -> BTreeMap<&str, usize> {
        let mut m = BTreeMap::new();
        for e in &self.entries {
            *m.entry(e.kind.as_str()).or_default() += 1;
        }
        m
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub version: u32,
    /// By set name.
    pub sets: BTreeMap<String, SetReport>,
}

impl Default for Report {
    fn default() -> Self {
        Self {
            version: VERSION,
            sets: BTreeMap::new(),
        }
    }
}

impl Report {
    /// The report already in `out`; empty if there is none or it cannot be read.
    pub fn load(out: &Path) -> Self {
        std::fs::read(out.join(REPORT_JSON))
            .ok()
            .and_then(|b| serde_json::from_slice::<Self>(&b).ok())
            .filter(|r| r.version == VERSION)
            .unwrap_or_default()
    }

    /// Replaces the section of `set`.
    pub fn put(&mut self, set: &str, report: SetReport) {
        self.sets.insert(set.to_owned(), report);
    }

    /// Writes `report-b1.json` and `report-b1.txt` into `out`.
    pub fn write(&self, out: &Path) -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(out.join(REPORT_JSON), json)?;
        std::fs::write(out.join(REPORT_TXT), self.to_text())
    }

    /// Human-readable form: per set the counts, the sizes, the number of entries per kind, then
    /// every entry.
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        for (name, set) in &self.sets {
            let _ = writeln!(s, "== {name} ==");
            for (k, v) in &set.counts {
                let _ = writeln!(s, "{k}: {v}");
            }
            for (k, v) in &set.bytes {
                let _ = writeln!(s, "{k}: {:.2} MB ({v} B)", *v as f64 / 1e6);
            }
            let kinds = set.kinds();
            if kinds.is_empty() {
                let _ = writeln!(s, "entries: none");
            } else {
                let list: Vec<String> = kinds.iter().map(|(k, n)| format!("{k} {n}")).collect();
                let _ = writeln!(s, "entries: {}", list.join(", "));
            }
            let mut by_kind: BTreeMap<&str, Vec<&ReportEntry>> = BTreeMap::new();
            for e in &set.entries {
                by_kind.entry(e.kind.as_str()).or_default().push(e);
            }
            for (kind, entries) in by_kind {
                let _ = writeln!(s, "\n-- {kind} ({}) --", entries.len());
                for e in entries {
                    let _ = writeln!(s, "{}: {}", e.subject, e.reason);
                }
            }
            s.push('\n');
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(n: u64) -> SetReport {
        SetReport {
            entries: vec![ReportEntry::new("placeholder", "ART_X", "no model")],
            counts: BTreeMap::from([("units".to_owned(), n)]),
            bytes: BTreeMap::from([("models".to_owned(), 2_500_000)]),
        }
    }

    #[test]
    fn text_lists_counts_sizes_and_entries_by_kind() {
        let mut r = Report::default();
        r.put("b1-units", set(3));
        let t = r.to_text();
        assert!(t.contains("== b1-units =="), "{t}");
        assert!(t.contains("units: 3"), "{t}");
        assert!(t.contains("models: 2.50 MB"), "{t}");
        assert!(t.contains("entries: placeholder 1"), "{t}");
        assert!(t.contains("ART_X: no model"), "{t}");
    }

    #[test]
    fn a_set_replaces_only_its_own_section() {
        let dir = std::env::temp_dir().join(format!("civ5-report-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut r = Report::default();
        r.put("b1-units", set(1));
        r.put("b1-sv", set(7));
        r.write(&dir).unwrap();
        let mut again = Report::load(&dir);
        assert_eq!(again, r);
        again.put("b1-units", set(2));
        assert_eq!(again.sets["b1-sv"].counts["units"], 7);
        assert_eq!(again.sets["b1-units"].counts["units"], 2);
        assert!(dir.join(REPORT_TXT).is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
