//! The golden HTML report and the triage gate.
//!
//! `build` writes `index.html` and `img/*.png` in one folder: for every parsed capture the captured
//! frame, and, when a render of ours exists (`<ours>/<capture>.dds`), the FLIP error map, metrics and the
//! worst 256x256 crops and the worst, median and best crop of every mask; the replay results
//! (`replay/report.json`). Every mask row has its threshold (`thresholds.toml`). `accept` is the
//! `--stage accept` gate (AC-0.4): it fails while a capture has no render of ours, is `reject`ed or `pending`, or
//! while a mask exceeds its threshold and `triage.toml` has no `bug` (with a task) or `simp` (with a `SIMP-NNN`)
//! verdict for it.

pub mod html;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::image::{Rgba8, read_dds_file, write_png};
use crate::metrics::{Image, Mask, MaskMetrics, PairMaps};
use crate::noise::{Metric, Thresholds};
use crate::replay::driver::Report as ReplayReport;

/// Size of the crops of the report.
pub const CROP: usize = 256;
/// Crops per capture.
pub const CROPS_PER_CAPTURE: usize = 4;

/// `triage.toml`: one table per capture, `[S1a] verdict = "accept"` (optional: `accept`, `reject` or `pending`), and
/// one table per mask that exceeds its threshold, `[S1a.masks.land] verdict = "simp" reference = "SIMP-025"`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Triage {
    #[serde(flatten)]
    pub captures: BTreeMap<String, TriageEntry>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct TriageEntry {
    /// `accept`, `reject` or `pending`; empty when the capture has only mask verdicts.
    #[serde(default)]
    pub verdict: String,
    #[serde(default)]
    pub note: String,
    /// Verdicts of the masks that exceed their threshold, by mask name (`land`, `water`, `all` ...).
    #[serde(default)]
    pub masks: BTreeMap<String, MaskVerdict>,
}

/// The verdict on one mask of one capture that exceeds its threshold.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MaskVerdict {
    /// `bug` (a known defect: `reference` names the task) or `simp` (a recorded simplification: `reference` is
    /// `SIMP-NNN`).
    pub verdict: String,
    #[serde(default)]
    pub reference: String,
    #[serde(default)]
    pub note: String,
}

fn is_simp_id(s: &str) -> bool {
    s.strip_prefix("SIMP-")
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

impl Triage {
    pub fn load(path: &Path) -> Result<Triage> {
        if !path.exists() {
            return Ok(Triage::default());
        }
        let text = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
        let t: Triage = toml::from_str(&text).with_context(|| path.display().to_string())?;
        for (c, e) in &t.captures {
            if !matches!(e.verdict.as_str(), "" | "accept" | "reject" | "pending") {
                bail!(
                    "{}: [{c}] verdict {:?} is not accept, reject or pending",
                    path.display(),
                    e.verdict
                );
            }
            for (mask, v) in &e.masks {
                let ok = match v.verdict.as_str() {
                    "bug" => !v.reference.trim().is_empty(),
                    "simp" => is_simp_id(&v.reference),
                    _ => false,
                };
                if !ok {
                    bail!(
                        "{}: [{c}.masks.{mask}] needs verdict \"bug\" with a task in reference, or \"simp\" with SIMP-NNN",
                        path.display()
                    );
                }
            }
        }
        Ok(t)
    }
}

pub struct Options {
    pub cache: PathBuf,
    /// Folder of the report.
    pub out: PathBuf,
    /// `replay/report.json`.
    pub replay: PathBuf,
    /// Renders of ours: `<dir>/<capture>.dds`.
    pub ours: PathBuf,
    pub triage: PathBuf,
    /// `thresholds.toml`; the copy in the crate when the file is not there.
    pub thresholds: PathBuf,
}

impl Options {
    fn thresholds(&self) -> Result<Thresholds> {
        match std::fs::read_to_string(&self.thresholds) {
            Ok(text) => Thresholds::parse(&text),
            Err(_) => Thresholds::parse(include_str!("../../thresholds.toml")),
        }
    }
}

/// `(scene, capture)` of every parsed capture in the cache, sorted.
pub fn discover(cache: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(scenes) = std::fs::read_dir(cache.join("golden")) else {
        return out;
    };
    for s in scenes.flatten() {
        let Ok(caps) = std::fs::read_dir(s.path()) else {
            continue;
        };
        for c in caps.flatten() {
            if c.path().join("meta.json").is_file() {
                out.push((
                    s.file_name().to_string_lossy().into_owned(),
                    c.file_name().to_string_lossy().into_owned(),
                ));
            }
        }
    }
    out.sort();
    out
}

/// The reference of the metrics: `world.dds`, the frame before the UI boundary; a capture
/// without it falls back to `final.dds`. Returns the file name with the image.
pub fn frame_of(dir: &Path) -> Option<(&'static str, Rgba8)> {
    ["world.dds", "final.dds"]
        .into_iter()
        .find_map(|n| read_dds_file(&dir.join(n)).ok().map(|img| (n, img)))
}

/// Pixels of the frame the particle draws (`FXShader`, the start-of-game "look here" beam) wrote: particles are not
/// judged. Found as the difference of `world.dds` and the saved target after the last draw before
/// the first particle draw, when only particle draws lie between them and after; widened by `MASK_ERODE_PX`.
/// `Err` with the reason when the capture has no particle draw or no such target.
pub fn particle_pixels(dir: &Path, world: &Rgba8) -> Result<Vec<bool>, String> {
    let draws = crate::captures::DrawsFile::load(dir).map_err(|e| e.to_string())?;
    let is_fx = |d: &crate::captures::Draw| {
        d.ps_entry
            .as_deref()
            .is_some_and(|e| e.starts_with("FXShader::"))
    };
    let Some(first_fx) = draws
        .draws
        .iter()
        .filter(|d| d.world && is_fx(d))
        .map(|d| d.event)
        .min()
    else {
        return Err("no particle draw".into());
    };
    if draws
        .draws
        .iter()
        .any(|d| d.world && d.event > first_fx && !is_fx(d))
    {
        return Err("world draws after the particles".into());
    }
    let before = std::fs::read_dir(dir.join("targets"))
        .map_err(|_| "no saved targets".to_string())?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_suffix("-after.dds")?.parse::<u32>().ok()
        })
        .filter(|&eid| eid < first_fx)
        .max()
        .ok_or_else(|| format!("no saved target before the particle draw {first_fx}"))?;
    if draws
        .draws
        .iter()
        .any(|d| d.event > before && d.event < first_fx && !is_fx(d))
    {
        return Err(format!(
            "draws between the saved target {before} and the particles"
        ));
    }
    let prev = read_dds_file(&dir.join("targets").join(format!("{before}-after.dds")))
        .map_err(|e| e.to_string())?;
    if (prev.width, prev.height) != (world.width, world.height) {
        return Err("saved target of another size".into());
    }
    let (w, h) = (world.width, world.height);
    let hit: Vec<bool> = prev
        .rgba
        .chunks_exact(4)
        .zip(world.rgba.chunks_exact(4))
        .map(|(a, b)| (0..3).any(|k| a[k].abs_diff(b[k]) > 1))
        .collect();
    let r = crate::metrics::MASK_ERODE_PX;
    let mut out = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            if hit[y * w + x] {
                for yy in y.saturating_sub(r)..(y + r + 1).min(h) {
                    for xx in x.saturating_sub(r)..(x + r + 1).min(w) {
                        out[yy * w + xx] = true;
                    }
                }
            }
        }
    }
    Ok(out)
}

fn crop(img: &Rgba8, x0: usize, y0: usize) -> Rgba8 {
    let w = CROP.min(img.width - x0);
    let h = CROP.min(img.height - y0);
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let at = ((y0 + y) * img.width + x0) * 4;
        rgba.extend_from_slice(&img.rgba[at..at + w * 4]);
    }
    Rgba8 {
        width: w,
        height: h,
        rgba,
    }
}

/// FLIP error (0..1) as a heat image: black, through red, to white.
fn heat(map: &[f32], w: usize, h: usize) -> Rgba8 {
    let mut rgba = Vec::with_capacity(w * h * 4);
    for &v in map {
        let t = v.clamp(0.0, 1.0);
        let r = (t * 2.0).min(1.0);
        let g = ((t - 0.5) * 2.0).clamp(0.0, 1.0);
        rgba.extend_from_slice(&[(r * 255.0) as u8, (g * 255.0) as u8, (g * 255.0) as u8, 255]);
    }
    Rgba8 {
        width: w,
        height: h,
        rgba,
    }
}

fn num(v: f64) -> Value {
    if v.is_finite() { json!(v) } else { Value::Null }
}

struct Built {
    data: Value,
    captures: Vec<String>,
    summary: Vec<String>,
}

/// The masks of the comparison (`[compare] masks` of the scenes): the class numbers and the flag bits
/// of the class mask of our frame.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Select {
    Class(u8),
    Flag(u8),
}

pub(crate) const MASKS: [(&str, Select); 9] = [
    ("land", Select::Class(1)),
    ("water", Select::Class(2)),
    ("river", Select::Class(3)),
    ("forest", Select::Class(4)),
    ("unit", Select::Class(5)),
    ("city", Select::Class(6)),
    ("landmark", Select::Class(7)),
    ("fog", Select::Flag(crate::metrics::CLASS_BIT_FOG)),
    ("overlay", Select::Flag(crate::metrics::CLASS_BIT_OVERLAY)),
];

/// A tile needs this many pixels of a mask to stand for it in the crops of that mask.
const MIN_TILE_PIXELS: usize = 4096;

/// What a row of the metrics table is judged by.
struct Judge<'a> {
    thresholds: &'a Thresholds,
    verdicts: Option<&'a BTreeMap<String, MaskVerdict>>,
}

impl Judge<'_> {
    /// One row of the table: the metrics of a mask, the thresholds, whether it exceeds them and its verdict.
    /// `gated` rows are the ones `--stage accept` needs a verdict for when they exceed.
    fn row(&self, label: &str, key: &str, m: &MaskMetrics, gated: bool) -> Value {
        let over: Vec<&str> = Metric::ALL
            .iter()
            .filter(|&&k| k.value(m) > self.thresholds.threshold(key, k))
            .map(|k| k.key())
            .collect();
        let thr = |k| self.thresholds.threshold(key, k);
        json!({
            "mask": label,
            "key": key,
            "pixels": m.flip.count,
            "flip": num(m.flip.mean),
            "de2000": num(m.de2000.mean),
            "ssim": num(1.0 - m.one_minus_ssim),
            "thr_flip": thr(Metric::FlipMean),
            "thr_ssim": thr(Metric::OneMinusSsim),
            "thr_de2000": thr(Metric::De2000Mean),
            "over": over,
            "gated": gated,
            "exceeds": gated && !over.is_empty(),
            "verdict": self.verdicts.and_then(|v| v.get(key)).map(|v| {
                if v.reference.is_empty() { v.verdict.clone() } else { format!("{} {}", v.verdict, v.reference) }
            }),
        })
    }
}

fn write_crop(
    img_dir: &Path,
    base: &str,
    reference: &Rgba8,
    ours: &Rgba8,
    heat_full: &Rgba8,
    (tx, ty): (usize, usize),
    label: String,
) -> Result<Value> {
    write_png(
        &img_dir.join(format!("{base}-ref.png")),
        &crop(reference, tx, ty),
    )?;
    write_png(
        &img_dir.join(format!("{base}-ours.png")),
        &crop(ours, tx, ty),
    )?;
    write_png(
        &img_dir.join(format!("{base}-error.png")),
        &crop(heat_full, tx, ty),
    )?;
    Ok(json!({
        "label": label,
        "ref": format!("img/{base}-ref.png"),
        "ours": format!("img/{base}-ours.png"),
        "error": format!("img/{base}-error.png"),
    }))
}

/// The worst, the median and the best tile of a mask by mean FLIP over the pixels of the mask: `(mean, tx, ty)`.
fn class_tiles(flip: &[f32], mask: &Mask) -> Vec<(f64, usize, usize)> {
    let (w, h) = (mask.width, mask.height);
    let mut tiles = Vec::new();
    for ty in (0..h).step_by(CROP) {
        for tx in (0..w).step_by(CROP) {
            let (mut sum, mut n) = (0.0f64, 0usize);
            for y in ty..(ty + CROP).min(h) {
                for x in tx..(tx + CROP).min(w) {
                    if mask.bits[y * w + x] {
                        sum += f64::from(flip[y * w + x]);
                        n += 1;
                    }
                }
            }
            if n >= MIN_TILE_PIXELS {
                tiles.push((sum / n as f64, tx, ty));
            }
        }
    }
    tiles.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut picked: Vec<(f64, usize, usize)> = Vec::new();
    for k in [tiles.len().wrapping_sub(1), tiles.len() / 2, 0] {
        if let Some(t) = tiles.get(k)
            && !picked.iter().any(|p| (p.1, p.2) == (t.1, t.2))
        {
            picked.push(*t);
        }
    }
    picked
}

fn capture_section(
    opts: &Options,
    thresholds: &Thresholds,
    scene: &str,
    capture: &str,
    triage: &Triage,
    img_dir: &Path,
    summary: &mut Vec<String>,
) -> Result<Value> {
    let dir = crate::captures::parsed_dir(&opts.cache, scene, capture);
    let mut notes: Vec<String> = Vec::new();
    let entry = triage.captures.get(capture);
    let verdict = entry.map(|e| e.verdict.clone());
    let judge = Judge {
        thresholds,
        verdicts: entry.map(|e| &e.masks),
    };
    let reference = frame_of(&dir);
    let Some((ref_name, reference)) = reference else {
        notes.push("no final.dds or world.dds in the capture folder".into());
        return Ok(json!({"id": capture, "scene": scene, "notes": notes, "verdict": verdict}));
    };
    write_png(&img_dir.join(format!("{capture}-ref.png")), &reference)?;
    let mut sec = json!({
        "id": capture,
        "scene": scene,
        "ref": format!("img/{capture}-ref.png"),
        "ours": null,
        "error": null,
        "metrics": [],
        "crops": [],
        "notes": notes,
        "verdict": verdict,
    });
    let ours_path = opts.ours.join(format!("{capture}.dds"));
    if !ours_path.is_file() {
        sec["notes"] =
            json!(["no render of ours (golden render) at ".to_string()
                + &ours_path.display().to_string()]);
        return Ok(sec);
    }
    let ours = read_dds_file(&ours_path)?;
    if (ours.width, ours.height) != (reference.width, reference.height) {
        sec["notes"] = json!([format!(
            "our render is {}x{}, the capture {}x{}: not compared",
            ours.width, ours.height, reference.width, reference.height
        )]);
        return Ok(sec);
    }
    let (w, h) = (reference.width, reference.height);
    write_png(&img_dir.join(format!("{capture}-ours.png")), &ours)?;
    let (ri, oi): (Image, Image) = (reference.to_image(), ours.to_image());
    let maps = PairMaps::new(&ri, &oi);
    write_png(
        &img_dir.join(format!("{capture}-error.png")),
        &heat(&maps.flip, w, h),
    )?;
    sec["ours"] = json!(format!("img/{capture}-ours.png"));
    sec["error"] = json!(format!("img/{capture}-error.png"));
    // The class masks come with `golden render` (`<capture>-<backend>.mask.r8`): the masks of the scene, in the
    // classes of our frame. Without them the whole frame is the one mask.
    let classes = ["native", "webgpu"]
        .iter()
        .map(|b| opts.ours.join(format!("{capture}-{b}.mask.r8")))
        .find(|p| p.is_file())
        .and_then(|p| std::fs::read(p).ok())
        .filter(|c| c.len() == w * h);
    let all_mask = match &classes {
        Some(c) => Mask::all_non_empty(w, h, c),
        None => Mask::full(w, h),
    };
    let m = maps.over(&all_mask);
    let mut rows = Vec::new();
    let mut line = format!("{capture}:");
    // The particles of the capture are not judged: shown, not gated.
    match particle_pixels(&dir, &reference) {
        Ok(fx) => {
            let mask = Mask {
                width: w,
                height: h,
                bits: fx.iter().map(|&p| !p).collect(),
            };
            let pm = maps.over(&mask);
            rows.push(judge.row(
                &format!("all but particles, against {ref_name}"),
                "all",
                &pm,
                false,
            ));
            line += &format!(
                " no particles ({:.1}% masked) SSIM {:.3} FLIP {:.3};",
                100.0 * fx.iter().filter(|&&p| p).count() as f64 / (w * h) as f64,
                1.0 - pm.one_minus_ssim,
                pm.flip.mean
            );
        }
        Err(why) => {
            line += &format!(" (particles not masked: {why})");
            sec["notes"] = json!([format!("particles not masked: {why}")]);
        }
    }
    rows.push(judge.row(&format!("all, against {ref_name}"), "all", &m, true));
    line += &format!(
        " all against {ref_name} SSIM {:.3} FLIP {:.3}",
        1.0 - m.one_minus_ssim,
        m.flip.mean
    );
    let mut crops = Vec::new();
    let heat_full = heat(&maps.flip, w, h);
    if let Some(classes) = &classes {
        for (name, select) in MASKS {
            let mask = match select {
                Select::Class(n) => Mask::from_class(w, h, classes, n),
                Select::Flag(f) => Mask::from_flag(w, h, classes, f),
            };
            let cm = maps.over(&mask);
            if cm.flip.count == 0 {
                continue;
            }
            rows.push(judge.row(
                &format!("{name} of our frame, against {ref_name}"),
                name,
                &cm,
                true,
            ));
            line += &format!(
                "; {name} ({} px) SSIM {:.3} FLIP {:.3}",
                cm.flip.count,
                1.0 - cm.one_minus_ssim,
                cm.flip.mean
            );
            // The worst, the median and the best crop of the mask by FLIP.
            for ((mean, tx, ty), rank) in class_tiles(&maps.flip, &mask.for_metrics())
                .into_iter()
                .zip(["worst", "median", "best"])
            {
                crops.push(write_crop(
                    img_dir,
                    &format!("{capture}-{name}-{rank}"),
                    &reference,
                    &ours,
                    &heat_full,
                    (tx, ty),
                    format!("{name}, {rank}: ({tx}, {ty}) mean FLIP {mean:.4}"),
                )?);
            }
        }
    }
    // The old baseline (RP29..RP31 compared against the frame with the UI): kept as a second row, so the change of
    // reference stays visible next to the new number.
    if ref_name != "final.dds"
        && let Ok(fin) = read_dds_file(&dir.join("final.dds"))
        && (fin.width, fin.height) == (w, h)
    {
        let old = PairMaps::new(&fin.to_image(), &oi).over(&all_mask);
        rows.push(judge.row(
            "all, against final.dds (with the UI: the old baseline)",
            "all",
            &old,
            false,
        ));
        line += &format!(
            "; against final.dds SSIM {:.3} FLIP {:.3}",
            1.0 - old.one_minus_ssim,
            old.flip.mean
        );
    }
    summary.push(line);
    sec["metrics"] = Value::Array(rows);
    // The worst tiles by mean FLIP error.
    let mut tiles = Vec::new();
    for ty in (0..h).step_by(CROP) {
        for tx in (0..w).step_by(CROP) {
            let (cw, ch) = (CROP.min(w - tx), CROP.min(h - ty));
            let mut sum = 0.0f64;
            for y in ty..ty + ch {
                for x in tx..tx + cw {
                    sum += f64::from(maps.flip[y * w + x]);
                }
            }
            tiles.push((sum / (cw * ch) as f64, tx, ty));
        }
    }
    tiles.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut worst = Vec::new();
    for (k, (mean, tx, ty)) in tiles.into_iter().take(CROPS_PER_CAPTURE).enumerate() {
        worst.push(write_crop(
            img_dir,
            &format!("{capture}-crop{k}"),
            &reference,
            &ours,
            &heat_full,
            (tx, ty),
            format!("({tx}, {ty}) mean FLIP {mean:.4}"),
        )?);
    }
    worst.extend(crops);
    sec["crops"] = Value::Array(worst);
    Ok(sec)
}

fn replay_section(opts: &Options, img_dir: &Path) -> Result<Value> {
    let Ok(text) = std::fs::read_to_string(&opts.replay) else {
        return Ok(Value::Null);
    };
    let rep: ReplayReport =
        serde_json::from_str(&text).with_context(|| opts.replay.display().to_string())?;
    let base = opts.replay.parent().unwrap_or(Path::new("."));
    let mut entries = Vec::new();
    for e in &rep.entries {
        let mut calls = Vec::new();
        for c in rep.calls.iter().filter(|c| {
            let bare = |n: &str| {
                n.trim_end_matches(":PS")
                    .trim_end_matches(":VS")
                    .to_string()
            };
            c.selected && (bare(&c.ps) == bare(&e.entry) || bare(&c.vs) == bare(&e.entry))
        }) {
            let mut image = Value::Null;
            if let Some(name) = &c.error_image
                && let Ok(bytes) = std::fs::read(base.join(name))
            {
                std::fs::write(img_dir.join(format!("replay-{name}")), bytes)?;
                image = json!(format!("img/replay-{name}"));
            }
            let st = c.stats.as_ref();
            let mut notes = c.notes.clone();
            notes.extend(c.error.clone());
            calls.push(json!({
                "capture": c.capture,
                "eid": c.event,
                "status": c.status,
                "compared": st.map_or(0, |s| s.compared),
                "failing": st.map_or(0, |s| s.failing),
                "max_abs": st.map_or(Value::Null, |s| num(f64::from(s.max_abs))),
                "p99_9": st.map_or(Value::Null, |s| num(f64::from(s.p99_9))),
                "image": image,
                "notes": notes,
            }));
        }
        entries.push(json!({
            "entry": e.entry,
            "status": e.status,
            "summary": e.verdict,
            "calls": calls,
        }));
    }
    Ok(json!({"gpu": rep.gpu, "entries": entries}))
}

fn build_data(opts: &Options, img_dir: &Path) -> Result<Built> {
    let triage = Triage::load(&opts.triage)?;
    let thresholds = opts.thresholds()?;
    let mut captures = Vec::new();
    let mut names = Vec::new();
    let mut summary = Vec::new();
    for (scene, capture) in discover(&opts.cache) {
        captures.push(capture_section(
            opts,
            &thresholds,
            &scene,
            &capture,
            &triage,
            img_dir,
            &mut summary,
        )?);
        names.push(capture);
    }
    Ok(Built {
        data: json!({
            "title": "Golden report",
            "generated": format!("{} captures", names.len()),
            "captures": captures,
            "replay": replay_section(opts, img_dir)?,
            "camera": [],
        }),
        captures: names,
        summary,
    })
}

/// Writes the report; returns the path of `index.html`, the capture names it covers and one metrics line per
/// compared capture.
pub fn build(opts: &Options) -> Result<(PathBuf, Vec<String>, Vec<String>)> {
    let img = opts.out.join("img");
    std::fs::create_dir_all(&img)?;
    let built = build_data(opts, &img)?;
    let path = opts.out.join("index.html");
    std::fs::write(&path, html::page(&built.data)?)?;
    Ok((path, built.captures, built.summary))
}

/// `--stage accept` (AC-0.4): the report is built, then every capture of the cache is judged: it needs a render of
/// ours, must not be `reject`ed or `pending`, and every mask that exceeds its threshold needs a verdict in
/// `triage.toml`. Returns the captures that pass; fails with the list of the problems.
pub fn accept(opts: &Options) -> Result<Vec<String>> {
    let triage = Triage::load(&opts.triage)?;
    let captures = discover(&opts.cache);
    if captures.is_empty() {
        bail!("no parsed capture in {}", opts.cache.display());
    }
    let img = opts.out.join("img");
    std::fs::create_dir_all(&img)?;
    let built = build_data(opts, &img)?;
    std::fs::write(opts.out.join("index.html"), html::page(&built.data)?)?;
    let mut accepted = Vec::new();
    let mut problems = Vec::new();
    for sec in built.data["captures"].as_array().into_iter().flatten() {
        let id = sec["id"].as_str().unwrap_or("?");
        let before = problems.len();
        match triage.captures.get(id).map(|e| e.verdict.as_str()) {
            Some("reject") => problems.push(format!("{id}: rejected")),
            Some("pending") => problems.push(format!("{id}: pending")),
            _ => {}
        }
        if sec["ours"].is_null() {
            let why = sec["notes"][0].as_str().unwrap_or("not compared");
            problems.push(format!("{id}: {why}"));
        }
        for row in sec["metrics"].as_array().into_iter().flatten() {
            if row["exceeds"].as_bool() == Some(true) && row["verdict"].is_null() {
                problems.push(format!(
                    "{id}: mask {} exceeds {} (FLIP {:.3}, SSIM {:.3}, dE {:.2}): no verdict in {}",
                    row["key"].as_str().unwrap_or("?"),
                    row["over"]
                        .as_array()
                        .map(|o| o
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("+"))
                        .unwrap_or_default(),
                    row["flip"].as_f64().unwrap_or(f64::NAN),
                    row["ssim"].as_f64().unwrap_or(f64::NAN),
                    row["de2000"].as_f64().unwrap_or(f64::NAN),
                    opts.triage.display()
                ));
            }
        }
        if problems.len() == before {
            accepted.push(id.to_owned());
        }
    }
    if !problems.is_empty() {
        bail!("stage accept failed:\n  {}", problems.join("\n  "));
    }
    Ok(accepted)
}

/// `golden noise`: the metrics of S1a against S1b (two captures of the original of one scene) over the masks of our
/// frame of S1a, as a proposal for the `[noise.*]` tables of `thresholds.toml`.
pub fn noise(opts: &Options, date: &str) -> Result<String> {
    let dir = |c: &str| crate::captures::parsed_dir(&opts.cache, "S1", c);
    let (Some((_, a)), Some((_, b))) = (frame_of(&dir("S1a")), frame_of(&dir("S1b"))) else {
        bail!(
            "noise needs the frames of S1a and S1b in {}",
            opts.cache.display()
        );
    };
    if (a.width, a.height) != (b.width, b.height) {
        bail!("S1a and S1b are not the same size");
    }
    let (w, h) = (a.width, a.height);
    let classes = ["native", "webgpu"]
        .iter()
        .map(|k| opts.ours.join(format!("S1a-{k}.mask.r8")))
        .find(|p| p.is_file())
        .and_then(|p| std::fs::read(p).ok())
        .filter(|c| c.len() == w * h)
        .with_context(|| {
            format!(
                "noise needs the class mask of our S1a frame (S1a-native.mask.r8) in {}: run `golden render S1`",
                opts.ours.display()
            )
        })?;
    let maps = PairMaps::new(&a.to_image(), &b.to_image());
    let mut by_mask = BTreeMap::new();
    by_mask.insert(
        "all".to_owned(),
        maps.over(&Mask::all_non_empty(w, h, &classes)),
    );
    for (name, select) in MASKS {
        let mask = match select {
            Select::Class(n) => Mask::from_class(w, h, &classes, n),
            Select::Flag(f) => Mask::from_flag(w, h, &classes, f),
        };
        let m = maps.over(&mask);
        if m.flip.count > 0 {
            by_mask.insert(name.to_owned(), m);
        }
    }
    Ok(crate::noise::proposal(&opts.thresholds()?, &by_mask, date))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::Rgba8;
    use crate::render::dds_bytes;

    fn opts(dir: &Path) -> Options {
        Options {
            cache: dir.join("cache"),
            out: dir.join("out"),
            replay: dir.join("none.json"),
            ours: dir.join("ours"),
            triage: dir.join("triage.toml"),
            thresholds: dir.join("thresholds.toml"),
        }
    }

    fn fake_cache(dir: &Path) {
        let c = dir.join("cache/golden/S1/S1a");
        std::fs::create_dir_all(&c).unwrap();
        std::fs::write(c.join("meta.json"), "{}").unwrap();
    }

    fn plain(rgb: [u8; 3]) -> Rgba8 {
        let (width, height) = (32, 32);
        let rgba = (0..width * height)
            .flat_map(|_| [rgb[0], rgb[1], rgb[2], 255])
            .collect();
        Rgba8 {
            width,
            height,
            rgba,
        }
    }

    /// A capture S1a whose frame is `reference` and a render of ours `ours`.
    fn frames(dir: &Path, reference: [u8; 3], ours: [u8; 3]) {
        fake_cache(dir);
        std::fs::write(
            dir.join("cache/golden/S1/S1a/final.dds"),
            dds_bytes(&plain(reference)).unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("ours")).unwrap();
        std::fs::write(dir.join("ours/S1a.dds"), dds_bytes(&plain(ours)).unwrap()).unwrap();
    }

    fn fresh(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("golden-report-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn accept_fails_without_a_render_of_ours() {
        let dir = fresh("a");
        fake_cache(&dir);
        let err = accept(&opts(&dir)).unwrap_err().to_string();
        assert!(
            err.contains("S1a:") && err.contains("no final.dds"),
            "{err}"
        );
        std::fs::write(opts(&dir).triage, "[S1a]\nverdict = \"pending\"\n").unwrap();
        assert!(
            accept(&opts(&dir))
                .unwrap_err()
                .to_string()
                .contains("S1a: pending")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn accept_needs_a_verdict_for_a_mask_over_its_threshold() {
        let dir = fresh("b");
        frames(&dir, [200, 40, 40], [30, 30, 200]);
        let o = opts(&dir);
        let err = accept(&o).unwrap_err().to_string();
        assert!(
            err.contains("S1a: mask all exceeds") && err.contains("no verdict"),
            "{err}"
        );
        // A verdict of the wrong shape is refused.
        std::fs::write(
            &o.triage,
            "[S1a.masks.all]\nverdict = \"simp\"\nreference = \"later\"\n",
        )
        .unwrap();
        assert!(accept(&o).is_err());
        std::fs::write(&o.triage, "[S1a.masks.all]\nverdict = \"bug\"\n").unwrap();
        assert!(accept(&o).is_err());
        // A `simp` with its SIMP number, or a `bug` with its task, passes.
        std::fs::write(
            &o.triage,
            "[S1a.masks.all]\nverdict = \"simp\"\nreference = \"SIMP-025\"\n",
        )
        .unwrap();
        assert_eq!(accept(&o).unwrap(), ["S1a"]);
        std::fs::write(&o.triage, "[S1a.masks.all]\nverdict = \"bug\"\nreference = \"R2.4\"\n[S1a]\nverdict = \"reject\"\n").unwrap();
        assert!(
            accept(&o)
                .unwrap_err()
                .to_string()
                .contains("S1a: rejected")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn accept_passes_for_equal_frames() {
        let dir = fresh("c");
        frames(&dir, [90, 120, 60], [90, 120, 60]);
        assert_eq!(accept(&opts(&dir)).unwrap(), ["S1a"]);
        let page = std::fs::read_to_string(opts(&dir).out.join("index.html")).unwrap();
        assert!(page.contains("thr_flip"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn report_page_is_written_without_frames() {
        let dir = fresh("d");
        fake_cache(&dir);
        let (p, caps, _) = build(&opts(&dir)).unwrap();
        assert_eq!(caps, ["S1a"]);
        let text = std::fs::read_to_string(p).unwrap();
        assert!(text.contains("id=\"data\"") && text.contains("S1a"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
