//! `probe --kind gr2`: decode every visible .gr2 into typed models, meshes and animations,
//! sample every curve and report failures.

use std::collections::BTreeMap;
use std::panic::{self, AssertUnwindSafe};

use anyhow::{Result, bail};
use asset_pipeline::game::GameSource;
use granny::check::{AnimSummary, Checked, FileStats, check_file};
use granny::curve::format_name;
use granny::skeleton::Skeleton;
use rayon::prelude::*;

/// W2: share of files that must decode without errors.
const TARGET_RATIO: f64 = 0.995;
const EXAMPLES: usize = 3;

enum Outcome {
    Io(String),
    Panic,
    Error(String),
    Ok(Box<Checked>),
}

/// Groups messages that differ only in numbers (track indexes, times, lengths).
fn reason_key(msg: &str) -> String {
    let mut out = String::new();
    let mut in_num = false;
    for c in msg.chars() {
        if c.is_ascii_digit() || (in_num && c == '.') {
            if !in_num {
                out.push('#');
            }
            in_num = true;
        } else {
            in_num = false;
            out.push(c);
        }
    }
    out
}

/// Model whose name is the longest `_`-boundary prefix of the animation's stem.
fn model_for<'a>(
    stem: &str,
    models: &'a BTreeMap<String, Skeleton>,
) -> Option<(&'a str, &'a Skeleton)> {
    let mut end = stem.len();
    loop {
        if let Some((k, v)) = models.get_key_value(&stem[..end]) {
            return Some((k, v));
        }
        end = stem[..end].rfind('_')?;
    }
}

#[derive(Default)]
struct BindStats {
    paired: usize,
    complete: usize,
    tracks: usize,
    bound: usize,
    /// Bound tracks whose t=0 translation is within 1 unit of the bone's rest translation.
    translation_close: usize,
    incomplete_examples: Vec<String>,
}

fn bind_stats(
    anims: &[(String, AnimSummary)],
    models: &BTreeMap<String, Skeleton>,
    keep: &dyn Fn(&str, &Skeleton) -> bool,
) -> BindStats {
    let mut s = BindStats::default();
    for (name, a) in anims {
        let stem = name.strip_suffix(".gr2").unwrap_or(name);
        let Some((model, skel)) = model_for(stem, models) else {
            continue;
        };
        if !keep(model, skel) {
            continue;
        }
        s.paired += 1;
        let bones: Vec<Option<usize>> = a
            .track_ids
            .iter()
            .zip(&a.track_names)
            .map(|(id, n)| match (n, id) {
                (Some(n), _) => skel.find_bone(n),
                (None, Some(id)) => skel.find_bone_by_id(*id),
                _ => None,
            })
            .collect();
        s.tracks += bones.len();
        s.bound += bones.iter().filter(|b| b.is_some()).count();
        let mut seen = std::collections::HashSet::new();
        let complete = bones.iter().all(|b| b.is_some_and(|b| seen.insert(b)));
        if complete {
            s.complete += 1;
        } else if s.incomplete_examples.len() < EXAMPLES {
            s.incomplete_examples.push(format!(
                "{name} ({} tracks, {} unbound, {} bones)",
                bones.len(),
                bones.iter().filter(|b| b.is_none()).count(),
                skel.bones.len()
            ));
        }
        for (b, (pos, _)) in bones.iter().zip(&a.start_pose) {
            let Some(bone) = b.and_then(|b| skel.bones.get(b)) else {
                continue;
            };
            let d: f32 = (0..3)
                .map(|k| (pos[k] - bone.local.position[k]).powi(2))
                .sum::<f32>()
                .sqrt();
            s.translation_close += usize::from(d < 1.0);
        }
    }
    s
}

pub fn probe_gr2(src: &GameSource) -> Result<()> {
    let started = std::time::Instant::now();
    let entries: Vec<_> = src.iter_ext("gr2").collect();
    let results: Vec<_> = entries
        .par_iter()
        .map(|entry| {
            let outcome =
                match src.read(entry) {
                    Err(e) => Outcome::Io(e.to_string()),
                    Ok(bytes) => panic::catch_unwind(AssertUnwindSafe(|| check_file(&bytes)))
                        .map_or(Outcome::Panic, |r| match r {
                            Ok(c) => Outcome::Ok(Box::new(c)),
                            Err(e) => Outcome::Error(e.to_string()),
                        }),
                };
            (entry.name.as_str(), outcome)
        })
        .collect();

    let total = results.len();
    let mut ok = 0usize;
    let mut stats = FileStats::default();
    let mut failures: BTreeMap<String, (usize, Vec<String>)> = BTreeMap::new();
    let mut io_errors = Vec::new();
    let mut panics = Vec::new();
    let mut models: BTreeMap<String, Skeleton> = BTreeMap::new();
    let mut anims: Vec<(String, AnimSummary)> = Vec::new();
    let mut hash_collisions = 0usize;
    for (name, outcome) in results {
        match outcome {
            Outcome::Io(e) => io_errors.push(format!("{name}: {e}")),
            Outcome::Panic => panics.push(name.to_string()),
            Outcome::Error(e) => {
                let (n, ex) = failures.entry(reason_key(&e)).or_default();
                *n += 1;
                if ex.len() < EXAMPLES {
                    ex.push(name.to_string());
                }
            }
            Outcome::Ok(c) => {
                ok += 1;
                stats.add(&c.stats);
                let stem = name.strip_suffix(".gr2").unwrap_or(name).to_string();
                if let Some(s) = c.model_skeleton {
                    let mut ids: Vec<_> = s.bones.iter().filter_map(|b| b.name_id).collect();
                    ids.sort_unstable();
                    let before = ids.len();
                    ids.dedup();
                    hash_collisions += usize::from(ids.len() != before);
                    models.insert(stem, s);
                } else if let Some(a) = c.anim {
                    anims.push((name.to_string(), a));
                }
            }
        }
    }

    let ratio = ok as f64 / total.max(1) as f64;
    println!(
        "gr2: {total} files, {ok} decoded without errors ({:.3}%), {} failed, {} read errors, {} panics ({:.1}s)",
        ratio * 100.0,
        failures.values().map(|(n, _)| n).sum::<usize>(),
        io_errors.len(),
        panics.len(),
        started.elapsed().as_secs_f64()
    );
    println!(
        "contents: {} skeletons ({} bones), {} meshes ({} skinned, {} vertices, {} triangles), {} models, {} animations ({} tracks)",
        stats.skeletons,
        stats.bones,
        stats.meshes,
        stats.skinned_meshes,
        stats.vertices,
        stats.triangles,
        stats.models,
        stats.animations,
        stats.tracks
    );
    println!("curves by format (3 per track, all sampled at 7 times):");
    for (code, n) in stats.curve_formats.iter().enumerate() {
        println!("  {code:>2} {:<14} {n:>9}", format_name(code as u8));
    }
    if !stats.skipped_members.is_empty() {
        println!(
            "vertex members skipped (not decoded): {}",
            stats.skipped_members.join(", ")
        );
    }
    type Filter = fn(&str, &Skeleton) -> bool;
    let filters: [(&str, Filter); 3] = [
        ("all skeletons", |_, _| true),
        ("skeletons with <= 128 bones", |_, s| s.bones.len() <= 128),
        ("unit models (u_*)", |m, _| m.starts_with("u_")),
    ];
    for (label, keep) in filters {
        let b = bind_stats(&anims, &models, &keep);
        println!(
            "track binding by name ID, {label}: {} animation files paired with a model, {} fully bound, {}/{} tracks bound, {} of those within 1.0 of the rest translation at t=0",
            b.paired, b.complete, b.bound, b.tracks, b.translation_close
        );
        for e in &b.incomplete_examples {
            println!("  incompletely bound: {e}");
        }
    }
    println!(
        "{} animation files have no model skeleton in the pack; {hash_collisions} skeletons have duplicate bone IDs",
        anims.len() - bind_stats(&anims, &models, &|_, _| true).paired
    );
    if !failures.is_empty() {
        println!("failures by reason:");
        for (reason, (n, examples)) in &failures {
            println!("  {n:>6}  {reason}  e.g. {}", examples.join(", "));
        }
    }
    for e in &io_errors {
        println!("io error: {e}");
    }
    for p in &panics {
        println!("PANIC decoding {p}");
    }
    if !panics.is_empty() || !io_errors.is_empty() {
        bail!(
            "{} panic(s), {} read error(s)",
            panics.len(),
            io_errors.len()
        );
    }
    if ratio < TARGET_RATIO {
        bail!(
            "only {:.3}% of gr2 files decoded cleanly (target {:.1}%)",
            ratio * 100.0,
            TARGET_RATIO * 100.0
        );
    }
    Ok(())
}
