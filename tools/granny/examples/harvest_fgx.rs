//! Dev tool: harvests `FireGrafixMaterial` strings (shader sets, parameter blocks, parameter
//! names) from gr2 files that keep their text, to grow `data/fgx-names.txt`.
//!
//! `cargo run --release -p granny --example harvest_fgx -- <install>` scans `Resource/Common/*.fpk`
//! and loose `Assets/**/*.gr2` (never mods) and prints the names, sorted, one per line. With
//! `--stats` it prints per-pack counts of stored versus stripped strings instead (`--list` adds the\n//! unresolved hashes by role).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use granny::extended::read_fgx_materials;
use granny::{GrannyFile, MAGIC_LE32};

fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(&p, ext, out);
        } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext))
            && !p.to_string_lossy().to_ascii_lowercase().contains("lekmod")
        {
            out.push(p);
        }
    }
}

/// Offsets of embedded gr2 files (packs store them uncompressed).
fn embedded(data: &[u8]) -> Vec<usize> {
    let mut v = Vec::new();
    let mut i = 0;
    while let Some(k) = data
        .get(i..)
        .and_then(|d| d.windows(16).position(|w| w == MAGIC_LE32))
    {
        v.push(i + k);
        i += k + 16;
    }
    v
}

#[derive(Default)]
struct Tally {
    files: usize,
    with_fgx: usize,
    stored: usize,
    stripped: usize,
    /// Unresolved hashes by role: shader set, block, parameter name, fallback material.
    unresolved: [BTreeSet<String>; 4],
}

fn harvest(data: &[u8], names: &mut BTreeSet<String>, tally: &mut Tally) {
    tally.files += 1;
    let Ok(file) = GrannyFile::parse(data) else {
        return;
    };
    let mats = match read_fgx_materials(&file) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("error: {e}");
            return;
        }
    };
    if mats.iter().any(Option::is_some) {
        tally.with_fgx += 1;
    }
    for m in mats.iter().flatten() {
        let mut strings = vec![(0, &m.shader_set)];
        strings.extend(m.fallback_mtl.iter().map(|s| (3, s)));
        for set in m.present_sets() {
            strings.push((1, &set.block));
            strings.extend(set.names.iter().map(|s| (2, s)));
        }
        for (role, s) in strings {
            if let Some(t) = &s.text {
                tally.stored += 1;
                names.insert(t.clone());
            } else if s.id.is_some() {
                tally.stripped += 1;
                if s.is_unresolved() {
                    tally.unresolved[role].extend(s.display());
                }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let stats = args.iter().any(|a| a == "--stats");
    let Some(root) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!("usage: harvest_fgx <install> [--stats]");
        std::process::exit(2);
    };
    let root = PathBuf::from(root);
    let mut packs = Vec::new();
    walk(&root.join("Resource").join("Common"), "fpk", &mut packs);
    let mut loose = Vec::new();
    walk(&root.join("Assets"), "gr2", &mut loose);

    let mut names = BTreeSet::new();
    for pack in &packs {
        let data = std::fs::read(pack).expect("read pack");
        let mut tally = Tally::default();
        for off in embedded(&data) {
            harvest(&data[off..], &mut names, &mut tally);
        }
        if stats {
            if args.iter().any(|a| a == "--list") {
                println!("  unresolved: {:?}", tally.unresolved);
            }
            println!(
                "{}: gr2 {} with FGX {} strings stored {} stripped {} unresolved shader/block/param/fallback {:?}",
                pack.file_name().unwrap().to_string_lossy(),
                tally.files,
                tally.with_fgx,
                tally.stored,
                tally.stripped,
                tally
                    .unresolved
                    .iter()
                    .map(BTreeSet::len)
                    .collect::<Vec<_>>()
            );
        }
    }
    let mut tally = Tally::default();
    for path in &loose {
        let data = std::fs::read(path).expect("read gr2");
        harvest(&data, &mut names, &mut tally);
    }
    if stats {
        println!(
            "loose Assets: gr2 {} with FGX {} strings stored {} stripped {}",
            tally.files, tally.with_fgx, tally.stored, tally.stripped
        );
        return;
    }
    for n in names {
        println!("{n}");
    }
}
