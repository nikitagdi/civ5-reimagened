//! Parses every gr2 of a Civ5 install (packs in `Resource/Common` plus loose Firaxis files under
//! `Assets`) and walks the full object graph. Runs only with `CIV5_ROOT` set; use `--release`.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use granny::{GrannyError, GrannyFile, MAGIC_LE32, NameSource, Object, Ptr, Value};

/// Third-party mods are never read.
fn is_mod(p: &Path) -> bool {
    p.to_string_lossy().to_ascii_lowercase().contains("lekmod")
}

fn walk_dir(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk_dir(&p, ext, out);
        } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)) && !is_mod(&p) {
            out.push(p);
        }
    }
}

/// Offsets of embedded gr2 files; packs store them uncompressed, so a magic scan finds them.
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
struct Report {
    total: usize,
    ok: usize,
    errors: BTreeMap<String, (usize, String)>,
    unknown: BTreeMap<String, usize>,
    names: [usize; 3],
}

/// Unknown member names outside `ExtendedData` (exporter properties, not SDK structures).
fn sdk_unknown(
    o: &Object<'_>,
    seen: &mut HashSet<(Ptr, Ptr)>,
    out: &mut BTreeSet<String>,
) -> Result<(), GrannyError> {
    for (m, v) in o.fields() {
        if &*m.name == "ExtendedData" {
            continue;
        }
        if m.name_source == NameSource::Unknown {
            out.insert(m.name.to_string());
        }
        sdk_value(v, seen, out)?;
    }
    Ok(())
}

fn sdk_value(
    v: Value<'_>,
    seen: &mut HashSet<(Ptr, Ptr)>,
    out: &mut BTreeSet<String>,
) -> Result<(), GrannyError> {
    let mut visit = |o: Object<'_>, seen: &mut HashSet<(Ptr, Ptr)>| {
        if seen.insert((o.at(), o.type_def().at)) {
            sdk_unknown(&o, seen, out)
        } else {
            Ok(())
        }
    };
    match v {
        Value::Inline(o) => visit(o, seen)?,
        Value::Reference(r) | Value::VariantReference(r) => {
            if let Some(o) = r.get()? {
                visit(o, seen)?;
            }
        }
        Value::Array(a) => {
            let d = a.resolve()?;
            let n = match d.element_type() {
                Some(t) if t.has_pointers() => d.len(),
                _ => d.len().min(1),
            };
            for o in d.iter().take(n) {
                visit(o, seen)?;
            }
        }
        Value::ArrayOfReferences(a) => {
            for o in a.iter()? {
                if let Some(o) = o? {
                    visit(o, seen)?;
                }
            }
        }
        Value::Fixed(f) => {
            for e in f.iter() {
                if let Value::Inline(o) = e {
                    visit(o, seen)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn check(report: &Mutex<Report>, label: String, data: &[u8]) {
    let result = GrannyFile::parse(data).and_then(|f| {
        f.walk()?;
        let mut unknown = BTreeSet::new();
        sdk_unknown(&f.root()?, &mut HashSet::new(), &mut unknown)?;
        let mut names = [0usize; 3];
        for t in f.parsed_types() {
            for m in &t.members {
                names[m.name_source as usize] += 1;
            }
        }
        Ok((unknown, names))
    });
    let mut rep = report.lock().unwrap();
    rep.total += 1;
    match result {
        Ok((unknown, names)) => {
            rep.ok += 1;
            for n in unknown {
                *rep.unknown.entry(n).or_default() += 1;
            }
            for (total, n) in rep.names.iter_mut().zip(names) {
                *total += n;
            }
        }
        Err(e) => {
            let e = rep.errors.entry(e.to_string()).or_insert((0, label));
            e.0 += 1;
        }
    }
}

#[test]
fn parses_all_game_gr2() {
    let Some(root) = std::env::var_os("CIV5_ROOT") else {
        println!("skipped: CIV5_ROOT not set");
        return;
    };
    let root = PathBuf::from(root);
    let mut packs = Vec::new();
    walk_dir(&root.join("Resource").join("Common"), "fpk", &mut packs);
    let mut loose = Vec::new();
    walk_dir(&root.join("Assets"), "gr2", &mut loose);
    let mut jobs: Vec<PathBuf> = packs.into_iter().chain(loose).collect();
    jobs.sort();

    let report = Mutex::new(Report::default());
    let next = Mutex::new(jobs.into_iter());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let Some(path) = next.lock().unwrap().next() else {
                        break;
                    };
                    let data = std::fs::read(&path).expect("read game file");
                    let name = path.file_name().unwrap().to_string_lossy().into_owned();
                    if path
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("gr2"))
                    {
                        check(&report, name, &data);
                    } else {
                        for off in embedded(&data) {
                            check(&report, format!("{name}@{off}"), &data[off..]);
                        }
                    }
                }
            });
        }
    });

    let rep = report.into_inner().unwrap();
    let rate = rep.ok as f64 / rep.total.max(1) as f64;
    println!(
        "gr2 files: {} parsed+walked: {} ({:.3}%)",
        rep.total,
        rep.ok,
        rate * 100.0
    );
    println!(
        "member names: stored {}, dictionary {}, unknown {} (mostly ExtendedData keys)",
        rep.names[0], rep.names[1], rep.names[2]
    );
    let mut unknown: Vec<_> = rep.unknown.iter().collect();
    unknown.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    println!(
        "distinct unknown names outside ExtendedData: {}",
        unknown.len()
    );
    // GRANNY_LIST_UNKNOWN=1 lists all of them (for growing the dictionary).
    let shown = if std::env::var_os("GRANNY_LIST_UNKNOWN").is_some() {
        usize::MAX
    } else {
        10
    };
    for (n, c) in unknown.iter().take(shown) {
        println!("  unknown name {n}: {c}");
    }
    for (e, (n, example)) in &rep.errors {
        println!("  error x{n}: {e} (e.g. {example})");
    }
    assert!(rep.total > 0, "no gr2 found under {}", root.display());
    assert!(rate >= 0.995, "success rate {rate:.4} below 99.5%");
}
