//! Dev tool: harvests member names from gr2 files whose type trees keep their strings
//! (loose DLC files), to grow `data/member-names.txt`.
//!
//! `cargo run -p granny --example harvest_names -- <file-or-dir>... [--all]`
//! prints names missing from the dictionary (or all names with `--all`), one per line, sorted.
//! Review the output: only Granny SDK identifiers belong in the dictionary.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use granny::{GrannyFile, NameSource, member_name};

fn collect(p: &Path, out: &mut Vec<PathBuf>) {
    if p.is_dir() {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for e in entries {
            collect(&e, out);
        }
    } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("gr2")) {
        out.push(p.to_path_buf());
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let all = args.iter().any(|a| a == "--all");
    let mut files = Vec::new();
    for a in args.iter().filter(|a| !a.starts_with("--")) {
        collect(Path::new(a), &mut files);
    }
    let mut names = BTreeSet::new();
    let (mut ok, mut failed) = (0, 0);
    for f in &files {
        let Ok(data) = std::fs::read(f) else { continue };
        let Ok(file) = GrannyFile::parse(&data) else {
            failed += 1;
            continue;
        };
        // Errors still leave every type parsed so far in the cache.
        if file.walk().is_ok() {
            ok += 1;
        } else {
            failed += 1;
        }
        for t in file.parsed_types() {
            for m in &t.members {
                if m.name_source == NameSource::Stored
                    && (all || member_name(m.name_hash).is_none())
                {
                    names.insert(m.name.to_string());
                }
            }
        }
    }
    eprintln!("files: {} walked, {} failed", ok, failed);
    for n in names {
        println!("{n}");
    }
}
