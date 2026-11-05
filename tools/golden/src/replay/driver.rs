//! The replay over the parsed captures: picks the draws of the ported entries,
//! builds the `DrawSpec` of each from the capture, the catalog and `status.toml`, runs it, compares
//! and writes `replay/report.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use render::shaders::{ModuleDesc, catalog as module_catalog};
use serde::{Deserialize, Serialize};
use shader_re::catalog::{BlockInfo, Catalog};
use shader_re::harness::{Registers, Subject};
use shader_re::scan::Stage;
use shader_re::sm3::ctab::RegisterSet;
use shader_re::status::{Status, StatusFile};

use super::compare::{self, Stats};
use super::inputs::{CBuffer, Call, CaptureData, Dds, Kind, View, fmt_by_name};
use super::{BoundTexture, DrawSpec, IoReg, ReplayGpu, SamplerDecl, ShaderSide};
use crate::captures::{Draw, DrawsFile, parsed_dir};
use crate::image::write_png;
use crate::scene::{SCENE_IDS, Scene};

pub const REPORT_SCHEMA: &str = "civ5-golden-replay/1";
/// Calls per entry and capture that count (the ones with the largest coverage).
pub const CALLS_PER_PAIR: usize = 3;

pub struct Options {
    pub cache: PathBuf,
    /// `Package::Entry` or `Package::Entry:STAGE`.
    pub entry: Option<String>,
    /// Capture names (`S1a`); empty: every parsed capture.
    pub captures: Vec<String>,
    /// `target/golden/replay`.
    pub out_dir: PathBuf,
    pub status: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CallReport {
    pub capture: String,
    pub event: u32,
    pub vs: String,
    pub ps: String,
    /// One of the `CALLS_PER_PAIR` largest of its pair in the capture: only these count.
    pub selected: bool,
    /// `pass`, `fail`, `error` (the replay cannot run it: the reason is in `error`).
    pub status: String,
    pub error: Option<String>,
    pub stats: Option<Stats>,
    pub notes: Vec<String>,
    pub error_image: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EntryReport {
    pub entry: String,
    pub status: String,
    /// Calls of the entry in each capture (all, replayable or not).
    pub calls_in_captures: BTreeMap<String, usize>,
    pub replayed: usize,
    pub passed: usize,
    pub failed: usize,
    pub errors: usize,
    pub verdict: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    pub schema: String,
    pub gpu: String,
    pub captures: Vec<String>,
    pub entries: Vec<EntryReport>,
    pub calls: Vec<CallReport>,
    /// No selected call failed.
    pub passed: bool,
}

fn strip_stage(name: &str) -> &str {
    name.strip_suffix(":PS")
        .or_else(|| name.strip_suffix(":VS"))
        .unwrap_or(name)
}

/// The catalog module that has the entry `Package::Entry`, and the entry in it.
fn module_of(entry: &str) -> Option<(&'static ModuleDesc, &'static render::shaders::EntryDesc)> {
    let want = strip_stage(entry);
    module_catalog().iter().find_map(|m| {
        m.entries
            .iter()
            .find(|e| e.re.is_some_and(|r| r.input == want))
            .map(|e| (m, e))
    })
}

fn block_bytes(cache: &Path, sha1: &str) -> Result<Vec<u8>> {
    let root = cache.join("shader-re");
    for e in std::fs::read_dir(&root).with_context(|| root.display().to_string())? {
        let p = e?.path().join("blocks").join(format!("{sha1}.bin"));
        if p.is_file() {
            return std::fs::read(&p).with_context(|| p.display().to_string());
        }
    }
    bail!(
        "block {sha1} is not in {}: run `shader-re extract`",
        root.display()
    )
}

#[derive(Deserialize)]
struct CbMap {
    consts: Vec<CbConst>,
}

#[derive(Deserialize)]
struct CbConst {
    name: String,
    #[serde(default)]
    reg: u32,
    cb: u32,
    offset: u32,
    rows: u32,
}

fn load_cbmap(cache: &Path, entry: &str) -> Result<CbMap> {
    let file = format!("{}.json", entry.replace("::", ".").replace(':', "."));
    for d in crate::camera::cbmap_dirs(cache) {
        let p = d.join(&file);
        if let Ok(text) = std::fs::read_to_string(&p) {
            return serde_json::from_str(&text).with_context(|| p.display().to_string());
        }
    }
    bail!("{entry} has no constant map ({file}): run `shader-re dx11 cbmap`")
}

/// The constant registers of a stage: the CTAB constants the map finds in the captured constant
/// buffers, register by register.
fn registers(
    entry: &str,
    cache: &Path,
    ctab: &shader_re::sm3::Ctab,
    cbuffers: &[CBuffer],
    notes: &mut Vec<String>,
) -> Result<Registers> {
    let map = load_cbmap(cache, entry)?;
    let end = ctab
        .constants
        .iter()
        .filter(|c| c.set == RegisterSet::Float4)
        .map(|c| c.reg as usize + c.count as usize)
        .max()
        .unwrap_or(0);
    let mut regs = Registers {
        c: vec![[0.0; 4]; end.max(64)],
        i: Default::default(),
        b: 0,
    };
    for c in &map.consts {
        let Some(cb) = cbuffers.iter().find(|b| b.slot == c.cb) else {
            notes.push(format!(
                "{entry}: constant buffer {} (`{}`) was not saved",
                c.cb, c.name
            ));
            continue;
        };
        for r in 0..c.rows as usize {
            let at = c.offset as usize / 4 + 4 * r;
            let Some(q) = cb.floats.get(at..at + 4) else {
                notes.push(format!(
                    "{entry}: `{}` is past the end of constant buffer {}",
                    c.name, c.cb
                ));
                break;
            };
            let reg = c.reg as usize + r;
            if reg >= regs.c.len() {
                regs.c.resize(reg + 1, [0.0; 4]);
            }
            regs.c[reg] = [q[0], q[1], q[2], q[3]];
        }
    }
    Ok(regs)
}

#[allow(clippy::too_many_arguments)]
fn side(
    cache: &Path,
    cat: &Catalog,
    status: &StatusFile,
    entry: &str,
    module: &ModuleDesc,
    desc: &render::shaders::EntryDesc,
    cbuffers: &[CBuffer],
    notes: &mut Vec<String>,
) -> Result<ShaderSide> {
    let e = cat
        .find_entry(entry)
        .with_context(|| format!("{entry} is not in the catalog"))?;
    let sm3 = e
        .sm3
        .as_ref()
        .with_context(|| format!("{entry} has no SM3 block"))?;
    let block: &BlockInfo = cat
        .block(&sm3.sha1)
        .with_context(|| format!("block {} is not in the catalog", sm3.sha1))?;
    let shader = shader_re::sm3::parse(&block_bytes(cache, &sm3.sha1)?)
        .with_context(|| format!("parsing the block of {entry}"))?;
    let ctab = shader
        .ctab
        .with_context(|| format!("{entry} has no constant table"))?;
    let stage = if e.stage == "VS" {
        Stage::Vs
    } else {
        Stage::Ps
    };
    let body = format!("{}_body", desc.name);
    let subject = Subject::hand(
        &format!("{}::{}", module.name, desc.name),
        &module.source(),
        stage,
        &body,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    let io = |l: &[shader_re::catalog::Io]| {
        l.iter()
            .map(|i| IoReg {
                reg: i.reg.clone(),
                semantic: i.semantic.to_ascii_lowercase(),
                index: i.index,
            })
            .collect::<Vec<_>>()
    };
    let regs = registers(&e.name, cache, &ctab, cbuffers, notes)?;
    Ok(ShaderSide {
        entry: e.name.clone(),
        subject,
        inputs: io(&block.inputs),
        outputs: io(&block.outputs),
        ctab,
        samplers: block
            .samplers
            .iter()
            .map(|s| SamplerDecl {
                name: s.name.clone(),
                register: s.register,
            })
            .collect(),
        regs,
        moves: status
            .entry
            .get(&e.name)
            .map(|s| s.data_moves.clone())
            .unwrap_or_default(),
    })
}

/// Mips 1.. of an 8-bit RGBA image by a 2x2 box filter (render targets are saved at mip 0 only).
fn box_mips(base: Dds, count: u32) -> Dds {
    if base.fmt.kind != Kind::Unorm8 || base.fmt.channels != 4 || base.mips != 1 || base.layers != 1
    {
        return base;
    }
    let mut images = vec![base.images[0][0].clone()];
    let (mut w, mut h) = (base.width as usize, base.height as usize);
    for _ in 1..count {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let prev = images.last().expect("mip 0").clone();
        let mut next = vec![0u8; nw * nh * 4];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let at = |xx: usize, yy: usize| {
                        u32::from(prev[(yy.min(h - 1) * w + xx.min(w - 1)) * 4 + c])
                    };
                    let s = at(2 * x, 2 * y)
                        + at(2 * x + 1, 2 * y)
                        + at(2 * x, 2 * y + 1)
                        + at(2 * x + 1, 2 * y + 1);
                    next[(y * nw + x) * 4 + c] = ((s + 2) / 4) as u8;
                }
            }
        }
        images.push(next);
        (w, h) = (nw, nh);
        if w == 1 && h == 1 {
            break;
        }
    }
    Dds {
        mips: images.len() as u32,
        images: vec![images],
        ..base
    }
}

fn load_textures(cap: &CaptureData, call: &Call) -> Result<BTreeMap<(bool, u32), BoundTexture>> {
    let mut out = BTreeMap::new();
    for (is_ps, views) in [(false, &call.textures.vs), (true, &call.textures.ps)] {
        for v in views.iter().filter(|v| !v.buffer) {
            let Some(view_fmt) = fmt_by_name(&v.view_format) else {
                continue;
            };
            let tag = if is_ps { "ps" } else { "vs" };
            let in_file = cap.target_file(call.eid, &format!("in-{tag}{}", v.slot));
            let dds = load_one(cap, v, &in_file).with_context(|| {
                format!(
                    "texture {} in register {} ({})",
                    v.resource, v.slot, v.view_format
                )
            })?;
            out.insert(
                (is_ps, v.slot),
                BoundTexture {
                    format: view_fmt.wgpu,
                    dds,
                },
            );
        }
    }
    Ok(out)
}

fn load_one(cap: &CaptureData, v: &View, in_file: &Path) -> Result<Dds> {
    // A render target changes during the frame: the copy saved for this very draw, with the mips
    // made here. Anything else is the file of the resource, with its own mips.
    let skip = std::env::var("GOLDEN_REPLAY_DEBUG").is_ok_and(|d| d.contains("rtfile"));
    if !skip && v.name.contains("Render Target") && in_file.is_file() {
        let base = Dds::load(in_file)?;
        return Ok(box_mips(base, v.num_mips.max(1)));
    }
    let p = cap.texture_file(&v.resource);
    if p.is_file() {
        return Dds::load(&p);
    }
    if in_file.is_file() {
        return Ok(box_mips(Dds::load(in_file)?, v.num_mips.max(1)));
    }
    bail!("neither {} nor {} exists", p.display(), in_file.display())
}

struct Ctx<'a> {
    gpu: &'a ReplayGpu,
    cache: &'a Path,
    cat: &'a Catalog,
    status: &'a StatusFile,
    out_dir: &'a Path,
}

fn replay_call(
    cx: &Ctx,
    cap: &CaptureData,
    draw: &Draw,
    capture: &str,
) -> Result<(Stats, Vec<String>, Option<String>)> {
    let call = cap
        .calls
        .get(&draw.event)
        .with_context(|| format!("event {} is not in events.json", draw.event))?;
    if !call.offsets_known() {
        bail!(
            "events.json of {capture} predates the draw offsets (start_index): run `golden parse {capture} --targets ...` again"
        );
    }
    let vs_name = draw.vs_entry.as_deref().context("no vertex entry")?;
    let ps_name = draw.ps_entry.as_deref().context("no pixel entry")?;
    let (vm, vd) = module_of(vs_name)
        .with_context(|| format!("{vs_name} has no module in the shader catalog"))?;
    let (pm, pd) = module_of(ps_name)
        .with_context(|| format!("{ps_name} has no module in the shader catalog"))?;
    if vm.name != pm.name {
        bail!(
            "{vs_name} is in module {} and {ps_name} in module {}: they cannot be one pipeline",
            vm.name,
            pm.name
        );
    }
    let mut notes = Vec::new();
    let vs = side(
        cx.cache,
        cx.cat,
        cx.status,
        vs_name,
        vm,
        vd,
        &call.cbuffers.vs,
        &mut notes,
    )?;
    let ps = side(
        cx.cache,
        cx.cat,
        cx.status,
        ps_name,
        pm,
        pd,
        &call.cbuffers.ps,
        &mut notes,
    )?;
    let textures = load_textures(cap, call)?;
    let before = Dds::load(&cap.target_file(draw.event, "before"))?;
    let after = Dds::load(&cap.target_file(draw.event, "after"))?;
    let before_depth = {
        let p = cap.target_file(draw.event, "before-depth");
        if p.is_file() {
            Some(Dds::load(&p)?.images.remove(0).remove(0))
        } else {
            None
        }
    };
    let t0 = call.targets.first().context("no target")?;
    if before.width != t0.width || before.height != t0.height || after.width != t0.width {
        bail!(
            "the saved target is {}x{}, the draw's {}x{}",
            before.width,
            before.height,
            t0.width,
            t0.height
        );
    }
    let spec = DrawSpec {
        label: format!("{vs_name} + {ps_name} @ {capture}:{}", draw.event),
        wgsl: vm.source(),
        vs,
        ps,
        call,
        buffers: cap,
        textures,
        before: before.images[0][0].clone(),
        before_depth,
    };
    let result = super::run_draw(cx.gpu, &spec)?;
    notes.extend(result.notes.iter().cloned());
    let reference = super::inputs::decode_pixels(&after.fmt, &after.images[0][0])?;
    let (w, h) = (result.width as usize, result.height as usize);
    if std::env::var("GOLDEN_REPLAY_DEBUG").is_ok_and(|d| d.contains("png")) {
        std::fs::create_dir_all(cx.out_dir)?;
        let before_px = super::inputs::decode_pixels(&before.fmt, &before.images[0][0])?;
        for (tag, px) in [
            ("ours", &result.ours),
            ("ref", &reference),
            ("before", &before_px),
        ] {
            let rgba = px
                .iter()
                .flat_map(|q| (0..4).map(|c| (q[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8))
                .collect();
            let name = format!("{capture}-{}-{tag}.png", draw.event);
            write_png(
                &cx.out_dir.join(name),
                &crate::image::Rgba8 {
                    width: w,
                    height: h,
                    rgba,
                },
            )?;
        }
    }
    let (stats, ratio) = compare::compare(
        &result.ours,
        &reference,
        &result.cover,
        w,
        result.fmt.channels as usize,
        result.fmt.unorm,
    );
    let mut image = None;
    if !stats.passed {
        std::fs::create_dir_all(cx.out_dir)?;
        let name = format!("{capture}-{}-err.png", draw.event);
        write_png(
            &cx.out_dir.join(&name),
            &compare::error_image(&ratio, &result.cover, w, h),
        )?;
        image = Some(name);
    }
    Ok((stats, notes, image))
}

/// The scene a capture belongs to.
fn scene_of(scenes_dir: &Path, capture: &str) -> Result<String> {
    for id in SCENE_IDS {
        let s = Scene::load_by_id(scenes_dir, id)?;
        if s.scene.captures.iter().any(|c| c == capture) {
            return Ok(id.to_string());
        }
    }
    bail!("unknown capture {capture:?}")
}

fn matches_filter(name: &Option<String>, filter: &str) -> bool {
    name.as_deref()
        .is_some_and(|n| strip_stage(n) == strip_stage(filter))
}

pub fn run(opts: &Options, scenes_dir: &Path) -> Result<Report> {
    let gpu = ReplayGpu::new().map_err(|e| anyhow::anyhow!("replay needs a GPU adapter: {e}"))?;
    let cat = Catalog::load(&opts.cache.join("shader-re").join("catalog.json"))
        .map_err(|e| anyhow::anyhow!("{e}: run `shader-re extract` first"))?;
    let status = StatusFile::load(&opts.status).map_err(|e| anyhow::anyhow!("{e}"))?;
    let cx = Ctx {
        gpu: &gpu,
        cache: &opts.cache,
        cat: &cat,
        status: &status,
        out_dir: &opts.out_dir,
    };

    // The captures to read.
    let mut names = opts.captures.clone();
    if names.is_empty() {
        for id in SCENE_IDS {
            let s = Scene::load_by_id(scenes_dir, id)?;
            for c in &s.scene.captures {
                if parsed_dir(&opts.cache, id, c).join("draws.json").is_file() {
                    names.push(c.clone());
                }
            }
        }
    }
    if names.is_empty() {
        bail!(
            "no parsed captures in {}: run `golden parse`",
            opts.cache.join("golden").display()
        );
    }

    let ported: Vec<&String> = status
        .entry
        .iter()
        .filter(|(_, e)| matches!(e.status, Status::Ported | Status::PortedDx11))
        .map(|(n, _)| n)
        .filter(|n| {
            opts.entry
                .as_ref()
                .is_none_or(|f| strip_stage(n) == strip_stage(f))
        })
        .collect();
    if let Some(f) = &opts.entry
        && ported.is_empty()
    {
        bail!("{f} is not a ported entry in {}", opts.status.display());
    }

    let mut calls: Vec<CallReport> = Vec::new();
    let mut counts: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for capture in &names {
        let scene = scene_of(scenes_dir, capture)?;
        let dir = parsed_dir(&opts.cache, &scene, capture);
        let draws =
            DrawsFile::load(&dir).with_context(|| format!("capture {capture} is not parsed"))?;
        let cap = CaptureData::load(&dir)?;
        println!("{capture}: {} draws", draws.draws.len());
        // Calls per ported entry (all draws).
        for d in draws.draws.iter().filter(|d| d.world) {
            for n in [&d.vs_entry, &d.ps_entry].into_iter().flatten() {
                if ported.iter().any(|p| strip_stage(p) == strip_stage(n)) {
                    *counts
                        .entry(strip_stage(n).to_string())
                        .or_default()
                        .entry(capture.clone())
                        .or_default() += 1;
                }
            }
        }
        // Candidates: world draws of two ported stages, with their targets saved.
        let mut groups: BTreeMap<(String, String), Vec<&Draw>> = BTreeMap::new();
        for d in draws.draws.iter().filter(|d| d.world) {
            let (Some(v), Some(p)) = (&d.vs_entry, &d.ps_entry) else {
                continue;
            };
            let wanted = opts
                .entry
                .as_ref()
                .is_none_or(|f| matches_filter(&d.vs_entry, f) || matches_filter(&d.ps_entry, f));
            let both_ported = [v, p].iter().all(|n| {
                status.status_of(n) == Status::Ported || status.status_of(n) == Status::PortedDx11
            });
            if wanted && both_ported {
                groups.entry((v.clone(), p.clone())).or_default().push(d);
            }
        }
        for ((v, p), ds) in groups {
            let mut done: Vec<(CallReport, usize)> = Vec::new();
            for d in ds {
                if !cap.target_file(d.event, "before").is_file()
                    || !cap.target_file(d.event, "after").is_file()
                {
                    continue;
                }
                let mut r = CallReport {
                    capture: capture.clone(),
                    event: d.event,
                    vs: v.clone(),
                    ps: p.clone(),
                    selected: false,
                    status: "error".into(),
                    error: None,
                    stats: None,
                    notes: Vec::new(),
                    error_image: None,
                };
                let covered = match replay_call(&cx, &cap, d, capture) {
                    Ok((stats, notes, image)) => {
                        // A draw that covers no pixel proves nothing: `empty`, not `pass`.
                        r.status = if stats.compared == 0 {
                            "empty"
                        } else if stats.passed {
                            "pass"
                        } else {
                            "fail"
                        }
                        .into();
                        let c = stats.compared;
                        r.stats = Some(stats);
                        r.notes = notes;
                        r.error_image = image;
                        c
                    }
                    Err(e) => {
                        r.error = Some(format!("{e:#}"));
                        0
                    }
                };
                println!(
                    "  {capture}:{} {v} + {p}: {}{}",
                    d.event,
                    r.status,
                    r.stats.as_ref().map_or(String::new(), |s| format!(
                        " (compared {}, max {:.5}, p99.9 {:.5}, limit ratio {:.2})",
                        s.compared, s.max_abs, s.p99_9, s.max_ratio
                    ))
                );
                if let Some(e) = &r.error {
                    println!("    {e}");
                }
                done.push((r, covered));
            }
            // The calls with the largest coverage count.
            let mut order: Vec<usize> = (0..done.len()).collect();
            order.sort_by_key(|&i| std::cmp::Reverse(done[i].1));
            for &i in order.iter().take(CALLS_PER_PAIR) {
                done[i].0.selected = true;
            }
            calls.extend(done.into_iter().map(|(r, _)| r));
        }
    }

    // One row per ported entry.
    let mut entries = Vec::new();
    for name in ported {
        let key = strip_stage(name).to_string();
        let mine: Vec<&CallReport> = calls
            .iter()
            .filter(|c| c.selected && (strip_stage(&c.vs) == key || strip_stage(&c.ps) == key))
            .collect();
        let passed = mine.iter().filter(|c| c.status == "pass").count();
        let failed = mine.iter().filter(|c| c.status == "fail").count();
        // Calls the replay could not run, and calls that covered no pixel (nothing was proved).
        let errors = mine
            .iter()
            .filter(|c| c.status == "error" || c.status == "empty")
            .count();
        let in_caps = counts.get(&key).cloned().unwrap_or_default();
        let verdict = if in_caps.is_empty() {
            format!("no calls in {}", names.join(", "))
        } else if mine.is_empty() {
            "calls exist, none with saved targets or none replayable".to_string()
        } else if failed > 0 {
            "fail".to_string()
        } else if passed > 0 {
            "pass".to_string()
        } else {
            "no usable call (cannot replay, or no pixel covered)".to_string()
        };
        entries.push(EntryReport {
            entry: name.clone(),
            status: status.status_of(name).name().to_string(),
            calls_in_captures: in_caps,
            replayed: mine.len(),
            passed,
            failed,
            errors,
            verdict,
        });
    }
    let passed = !calls.iter().any(|c| c.selected && c.status == "fail");
    Ok(Report {
        schema: REPORT_SCHEMA.into(),
        gpu: format!(
            "{} ({:?}), features {:?}",
            gpu.info.name, gpu.info.backend, gpu.features
        ),
        captures: names,
        entries,
        calls,
        passed,
    })
}

pub fn write_report(report: &Report, out_dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(out_dir)?;
    let p = out_dir.join("report.json");
    std::fs::write(&p, serde_json::to_string_pretty(report)? + "\n")?;
    Ok(p)
}
