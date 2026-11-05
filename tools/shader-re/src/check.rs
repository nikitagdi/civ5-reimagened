//! `check`: status of every world entry and the guard against files
//! derived from the game executables in git (SR-D6, criterion 7).

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::status::{Status, StatusFile};

/// What the last `compare` (R1.8) said about an entry: `target/shader-re/compare.json`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct CompareResults {
    #[serde(default)]
    pub entries: BTreeMap<String, CompareResult>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CompareResult {
    pub sha1: String,
    pub pass: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Row {
    pub entry: String,
    pub status: String,
    pub problems: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    pub rows: Vec<Row>,
    /// Names in `status.toml` that are not world entries of the catalog.
    pub unknown_in_status: Vec<String>,
    /// Tracked files that look derived from the game executables.
    pub derived_in_git: Vec<String>,
    pub todo: usize,
    pub ported: usize,
    pub ported_dx11: usize,
    pub not_needed: usize,
    /// Set when `--stage accept` was asked.
    pub accept: bool,
}

impl Report {
    /// Problems that fail the check. `todo` fails only in the accept stage.
    pub fn failed(&self) -> bool {
        self.rows.iter().any(|r| !r.problems.is_empty())
            || !self.derived_in_git.is_empty()
            || !self.unknown_in_status.is_empty()
            || (self.accept && self.todo > 0)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("report serializes") + "\n"
    }

    pub fn table(&self) -> String {
        let mut s = String::new();
        for r in &self.rows {
            let mark = if !r.problems.is_empty() {
                "FAIL"
            } else if r.status == "todo" {
                "todo"
            } else {
                "ok"
            };
            s.push_str(&format!("{mark:<5} {:<12} {}\n", r.status, r.entry));
            for p in &r.problems {
                s.push_str(&format!("      {p}\n"));
            }
        }
        for n in &self.unknown_in_status {
            s.push_str(&format!(
                "FAIL  status.toml names {n}, which is not a world entry\n"
            ));
        }
        for p in &self.derived_in_git {
            s.push_str(&format!(
                "FAIL  tracked file derived from the executables: {p}\n"
            ));
        }
        s.push_str(&format!(
            "world entries {}: ported {}, ported-dx11 {}, not-needed {}, todo {}{}\n",
            self.rows.len(),
            self.ported,
            self.ported_dx11,
            self.not_needed,
            self.todo,
            if self.accept && self.todo > 0 {
                " (accept stage: todo fails)"
            } else {
                ""
            }
        ));
        s
    }
}

/// Everything `run` reads from the outside.
pub struct Input<'a> {
    /// World entry names (from the catalog) with the sha1 of their SM3 block, if any.
    pub world: &'a [(String, Option<String>)],
    pub status: &'a StatusFile,
    /// Repository root: `wgsl` paths are relative to it.
    pub repo: &'a Path,
    pub compare: &'a CompareResults,
    /// `git ls-files`.
    pub tracked: &'a [String],
    pub accept: bool,
}

/// Does a tracked path look derived from the game executables (SR-D6)?
pub fn derived_from_exe(path: &str) -> bool {
    let p = path.replace('\\', "/");
    let lower = p.to_lowercase();
    let file = lower.rsplit('/').next().unwrap_or("");
    if lower.contains("cache/shader-re/") {
        return true;
    }
    if matches!(file, "blocks.json" | "catalog.json") {
        return true;
    }
    if lower.ends_with(".exe") || lower.ends_with(".dll") || file == "civ5xp" {
        return true;
    }
    // `blocks/<sha1>.bin`
    if let Some(stem) = file.strip_suffix(".bin")
        && stem.len() == 40
        && stem.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return true;
    }
    // Disassembly, DXBC text, oracle output.
    ["/asm/", "/oracle/", "/dx11/", "/d3d/"]
        .iter()
        .any(|d| lower.contains(&format!("shader-re{d}")))
}

/// Looks for `// re: <Package>::<Entry> (sha1 <12 hex>)` in `src`.
fn has_re_comment(src: &str, name: &str, sha1: &str) -> bool {
    let want = &sha1[..sha1.len().min(12)];
    let base = name.rsplit_once(':').map_or(name, |(n, _)| n);
    src.lines().any(|l| {
        let l = l.trim();
        l.starts_with("// re:")
            && (l.contains(name) || l.contains(base))
            && l.contains(&format!("sha1 {want}"))
    })
}

fn check_ported(
    name: &str,
    block_sha1: Option<&str>,
    e: &crate::status::StatusEntry,
    inp: &Input,
    with_compare: bool,
) -> Vec<String> {
    let mut p = Vec::new();
    let Some(wgsl) = e.wgsl.as_deref().filter(|w| !w.is_empty()) else {
        return vec!["no `wgsl` path".to_string()];
    };
    let (file, point) = wgsl.split_once('#').unwrap_or((wgsl, ""));
    let Some(sha1) = e.sha1.as_deref().filter(|s| s.len() >= 12) else {
        p.push("no `sha1` (at least 12 hex digits)".to_string());
        return p;
    };
    if let Some(b) = block_sha1
        && !b.starts_with(sha1)
        && !sha1.starts_with(b)
    {
        p.push(format!("sha1 {sha1} is not the block of the entry ({b})"));
    }
    let path = inp.repo.join(file);
    let Ok(src) = std::fs::read_to_string(&path) else {
        p.push(format!("{file}: file not found"));
        return p;
    };
    if !has_re_comment(&src, name, sha1) {
        p.push(format!(
            "{file}: no `// re: {name} (sha1 {})` line",
            &sha1[..12]
        ));
    }
    if point.is_empty() {
        p.push("`wgsl` has no `#entry_point`".to_string());
    } else if !src.contains(&format!("fn {point}")) {
        p.push(format!("{file}: no entry point `{point}`"));
    }
    // The parts the entry's module has before its own file.
    let mut joined = String::new();
    for m in &e.modules {
        match std::fs::read_to_string(inp.repo.join(m)) {
            Ok(t) => {
                joined.push_str(&t);
                joined.push('\n');
            }
            Err(_) => p.push(format!("{m}: module part not found")),
        }
    }
    joined.push_str(&src);
    let report = crate::validate::validate_wgsl(file, &joined);
    if !report.ok() {
        p.push(format!("{file}: fails naga / WebGL2 limits"));
    }
    if with_compare {
        match inp.compare.entries.get(name) {
            None => p.push("no result of `compare` for this entry".to_string()),
            Some(r) if !r.sha1.starts_with(&sha1[..12]) => p.push(format!(
                "`compare` ran on sha1 {}, status says {sha1}",
                r.sha1
            )),
            Some(r) if !r.pass => p.push("last `compare` failed".to_string()),
            Some(_) => {}
        }
    }
    p
}

/// The check of the catalog.
pub fn run(inp: &Input) -> Report {
    let mut rep = Report {
        accept: inp.accept,
        ..Report::default()
    };
    for (name, block) in inp.world {
        let entry = inp.status.entry.get(name);
        let status = entry.map_or(Status::Todo, |e| e.status);
        let mut problems = Vec::new();
        match (status, entry) {
            (Status::Todo, _) => {
                rep.todo += 1;
                if entry.is_none() {
                    problems.push("missing from status.toml".to_string());
                }
            }
            (Status::Ported, Some(e)) => {
                rep.ported += 1;
                problems = check_ported(name, block.as_deref(), e, inp, true);
            }
            (Status::PortedDx11, Some(e)) => {
                rep.ported_dx11 += 1;
                problems = check_ported(name, block.as_deref(), e, inp, false);
                if e.replay.as_deref().is_none_or(str::is_empty) {
                    problems.push("no `replay` test".to_string());
                }
            }
            (Status::NotNeeded, Some(e)) => {
                rep.not_needed += 1;
                if e.reason.as_deref().is_none_or(|r| r.trim().is_empty()) {
                    problems.push("`not-needed` without a `reason`".to_string());
                }
            }
            (_, None) => unreachable!("only `todo` is the default status"),
        }
        rep.rows.push(Row {
            entry: name.clone(),
            status: status.name().to_string(),
            problems,
        });
    }
    let world: std::collections::BTreeSet<&str> =
        inp.world.iter().map(|(n, _)| n.as_str()).collect();
    rep.unknown_in_status = inp
        .status
        .entry
        .keys()
        .filter(|k| !world.contains(k.as_str()))
        .cloned()
        .collect();
    rep.derived_in_git = inp
        .tracked
        .iter()
        .filter(|p| derived_from_exe(p))
        .cloned()
        .collect();
    rep
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(text: &str) -> StatusFile {
        StatusFile::parse(text).unwrap()
    }

    fn world() -> Vec<(String, Option<String>)> {
        vec![
            (
                "P::a:PS".to_string(),
                Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()),
            ),
            ("P::b:VS".to_string(), None),
            ("P::c:PS".to_string(), None),
        ]
    }

    fn run_with(st: &StatusFile, repo: &Path, accept: bool) -> Report {
        run(&Input {
            world: &world(),
            status: st,
            repo,
            compare: &CompareResults::default(),
            tracked: &[],
            accept,
        })
    }

    #[test]
    fn todo_warns_and_fails_only_in_accept() {
        let st = status("");
        let r = run_with(&st, Path::new("."), false);
        assert_eq!(r.todo, 3);
        // Entries missing from status.toml are a problem of their own.
        assert!(r.failed());
        let st = status(
            "[entry.\"P::a:PS\"]\nstatus=\"todo\"\n[entry.\"P::b:VS\"]\nstatus=\"todo\"\n[entry.\"P::c:PS\"]\nstatus=\"todo\"\n",
        );
        let r = run_with(&st, Path::new("."), false);
        assert!(!r.failed());
        let r = run_with(&st, Path::new("."), true);
        assert!(r.failed());
    }

    #[test]
    fn not_needed_wants_a_reason() {
        let st = status(
            "[entry.\"P::a:PS\"]\nstatus=\"not-needed\"\n[entry.\"P::b:VS\"]\nstatus=\"not-needed\"\nreason=\"unused\"\n[entry.\"P::c:PS\"]\nstatus=\"todo\"\n",
        );
        let r = run_with(&st, Path::new("."), false);
        assert!(!r.rows[0].problems.is_empty());
        assert!(r.rows[1].problems.is_empty());
        assert_eq!(r.not_needed, 2);
    }

    #[test]
    fn names_outside_the_world_are_reported() {
        let st = status(
            "[entry.\"P::a:PS\"]\nstatus=\"todo\"\n[entry.\"P::b:VS\"]\nstatus=\"todo\"\n[entry.\"P::c:PS\"]\nstatus=\"todo\"\n[entry.\"Q::z:PS\"]\nstatus=\"todo\"\n",
        );
        let r = run_with(&st, Path::new("."), false);
        assert_eq!(r.unknown_in_status, ["Q::z:PS"]);
        assert!(r.failed());
    }

    #[test]
    fn ported_entries_are_checked_against_the_file() {
        let dir = std::env::temp_dir().join(format!("shader-re-check-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wgsl = "// re: P::a (sha1 aaaaaaaaaaaa)\n@fragment\nfn a_ps() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }\n";
        std::fs::write(dir.join("a.wgsl"), wgsl).unwrap();
        let text = "[entry.\"P::a:PS\"]\nstatus=\"ported\"\nsha1=\"aaaaaaaaaaaa\"\nwgsl=\"a.wgsl#a_ps\"\n\
                    [entry.\"P::b:VS\"]\nstatus=\"todo\"\n[entry.\"P::c:PS\"]\nstatus=\"todo\"\n";
        let st = status(text);
        // No `compare` result yet: ported fails.
        let r = run_with(&st, &dir, false);
        assert_eq!(
            r.rows[0].problems,
            ["no result of `compare` for this entry"]
        );
        // With a passing result for that sha1 it is clean.
        let mut cmp = CompareResults::default();
        cmp.entries.insert(
            "P::a:PS".to_string(),
            CompareResult {
                sha1: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                pass: true,
            },
        );
        let w = world();
        let ok = run(&Input {
            world: &w,
            status: &st,
            repo: &dir,
            compare: &cmp,
            tracked: &[],
            accept: false,
        });
        assert!(ok.rows[0].problems.is_empty(), "{:?}", ok.rows[0].problems);
        // A comment for another sha1 or a missing entry point is caught.
        std::fs::write(
            dir.join("a.wgsl"),
            wgsl.replace("aaaaaaaaaaaa", "bbbbbbbbbbbb"),
        )
        .unwrap();
        let bad = run(&Input {
            world: &w,
            status: &st,
            repo: &dir,
            compare: &cmp,
            tracked: &[],
            accept: false,
        });
        assert!(bad.rows[0].problems.iter().any(|p| p.contains("// re:")));
        std::fs::write(dir.join("a.wgsl"), wgsl.replace("a_ps", "other")).unwrap();
        let bad = run(&Input {
            world: &w,
            status: &st,
            repo: &dir,
            compare: &cmp,
            tracked: &[],
            accept: false,
        });
        assert!(
            bad.rows[0]
                .problems
                .iter()
                .any(|p| p.contains("entry point"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn derived_files_in_git_are_caught() {
        for bad in [
            "cache/shader-re/catalog.json",
            "tools/shader-re/blocks.json",
            "x/0123456789abcdef0123456789abcdef01234567.bin",
            "bin/CivilizationV.exe",
            "tools/shader-re/oracle/UnitPS.wgsl",
            "Civ5XP",
        ] {
            assert!(derived_from_exe(bad), "{bad}");
        }
        for good in [
            "tools/shader-re/status.toml",
            "tools/shader-re/src/catalog.rs",
            "tools/shader-re/maps/formats.toml",
            "crates/client/render/src/shaders/unit/unit.wgsl",
        ] {
            assert!(!derived_from_exe(good), "{good}");
        }
        let st = status("");
        let w: Vec<(String, Option<String>)> = Vec::new();
        let r = run(&Input {
            world: &w,
            status: &st,
            repo: Path::new("."),
            compare: &CompareResults::default(),
            tracked: &[
                "a/cache/shader-re/x.json".to_string(),
                "src/lib.rs".to_string(),
            ],
            accept: false,
        });
        assert_eq!(r.derived_in_git, ["a/cache/shader-re/x.json"]);
        assert!(r.failed());
    }
}
