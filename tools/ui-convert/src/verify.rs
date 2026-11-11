//! `verify`: the layout of a built package against Chromium's.
//!
//! The package runs natively in `ui2-runtime` with the text engine on its own font subsets and the sample of a
//! state; the runtime's rectangle dump (`Runtime::dump_rects`) is compared with the one Chromium writes for the
//! preview's layout page (`node/rects.mjs`) at the reference viewports: the same records in the same order, every
//! edge within [`TOLERANCE`] px, text leaves included. The report names the `data-part` each record sits in and
//! the node path, and lists the records that are off.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};
use ui2_draw::Scene;
use ui2_model::{ModelStore, Sample, Schema};
use ui2_package::Package;
use ui2_runtime::{Env, Runtime, Services, load_faces};
use ui2_text::TextEngine;

use crate::errors::{ConvError, Place, codes};

/// The largest difference of any edge, CSS px.
pub const TOLERANCE: f64 = 0.5;

/// The reference viewports: CSS size and device scale.
pub const VIEWPORTS: &[([u32; 2], f32)] = &[
    ([1280, 720], 1.0),
    ([1920, 1200], 1.0),
    ([3440, 1440], 1.0),
    ([5120, 1440], 1.0),
    ([1280, 720], 1.5),
];

/// One record of a dump.
#[derive(Debug, Clone, PartialEq)]
pub struct Rect {
    /// `<template>:<index>`, `#k` for a text leaf.
    pub node: String,
    /// The instance keys.
    pub keys: String,
    /// x, y, w, h in CSS px.
    pub r: [f64; 4],
}

/// The records of a Chrome dump (`{"rects": [[node, keys, x, y, w, h], ...]}`).
pub fn rects_of(dump: &Value) -> Result<Vec<Rect>, String> {
    let rows = dump["rects"].as_array().ok_or("the dump has no rects")?;
    rows.iter()
        .map(|r| {
            let num = |i: usize| r[i].as_f64().ok_or_else(|| format!("bad record {r}"));
            Ok(Rect {
                node: r[0].as_str().unwrap_or_default().to_string(),
                keys: r[1].as_str().unwrap_or_default().to_string(),
                r: [num(2)?, num(3)?, num(4)?, num(5)?],
            })
        })
        .collect()
}

/// One compared record.
#[derive(Debug, Clone)]
pub struct Row {
    /// `<template>:<index>[#k]`.
    pub node: String,
    /// The instance keys.
    pub keys: String,
    /// The nearest `data-part` of the node or an ancestor in its template (empty when there is none).
    pub part: String,
    /// The source place of the element, `file:line` (empty when the package has none).
    pub at: String,
    /// The runtime's box.
    pub ours: [f64; 4],
    /// Chromium's box.
    pub chrome: [f64; 4],
    /// The largest edge difference.
    pub delta: f64,
}

/// The comparison at one viewport.
#[derive(Debug, Clone)]
pub struct ViewportReport {
    /// CSS size.
    pub viewport: [u32; 2],
    /// Device scale.
    pub dsf: f32,
    /// Every compared record, in dump order.
    pub rows: Vec<Row>,
    /// Set when the two dumps do not list the same records: what differs.
    pub mismatch: Option<String>,
}

impl ViewportReport {
    /// The record with the largest difference.
    pub fn worst(&self) -> Option<&Row> {
        self.rows.iter().max_by(|a, b| a.delta.total_cmp(&b.delta))
    }

    /// The records off by more than [`TOLERANCE`].
    pub fn failures(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(|r| r.delta > TOLERANCE)
    }
}

/// The result of a comparison.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// The skin id.
    pub skin: String,
    /// The sample state.
    pub state: String,
    /// One entry per viewport.
    pub viewports: Vec<ViewportReport>,
    /// What painting reported that the check does not accept (it reports what it cannot draw yet; unsupported
    /// paints named below are expected).
    pub runtime_errors: Vec<String>,
}

impl Report {
    /// Whether every viewport lists the same records and every edge is within the tolerance.
    pub fn ok(&self) -> bool {
        self.runtime_errors.is_empty()
            && self
                .viewports
                .iter()
                .all(|v| v.mismatch.is_none() && v.failures().next().is_none())
    }

    /// The worst difference of each part over all viewports, largest first.
    pub fn by_part(&self) -> Vec<(String, f64, usize)> {
        let mut m: BTreeMap<&str, (f64, usize)> = BTreeMap::new();
        for r in self.viewports.iter().flat_map(|v| &v.rows) {
            let e = m.entry(r.part.as_str()).or_insert((0.0, 0));
            e.0 = e.0.max(r.delta);
            e.1 += 1;
        }
        let mut out: Vec<(String, f64, usize)> = m
            .into_iter()
            .map(|(p, (d, n))| {
                let name = if p.is_empty() { "(no part)" } else { p };
                (name.to_string(), d, n)
            })
            .collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out
    }

    /// The report as text lines: a line per viewport, the parts, the records that are off.
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![format!(
            "verify {} --state {}: {} viewports, tolerance {TOLERANCE} px",
            self.skin,
            self.state,
            self.viewports.len()
        )];
        for v in &self.viewports {
            let worst = v.worst();
            out.push(format!(
                "  {}x{} dsf {}: {} records, worst {:.4} px{}",
                v.viewport[0],
                v.viewport[1],
                v.dsf,
                v.rows.len(),
                worst.map_or(0.0, |w| w.delta),
                worst.map_or(String::new(), |w| format!(
                    " ({} [{}] part {})",
                    w.node,
                    w.keys,
                    if w.part.is_empty() { "-" } else { &w.part }
                )),
            ));
            if let Some(m) = &v.mismatch {
                out.push(format!("    the records differ: {m}"));
            }
        }
        out.push("by data-part (worst over the viewports):".to_string());
        for (p, d, n) in self.by_part() {
            out.push(format!("  {p}: {d:.4} px over {n} records"));
        }
        for v in &self.viewports {
            for r in v.failures() {
                out.push(format!(
                    "OFF at {}x{} dsf {}: {} [{}] part {} ({}): ours {:?}, Chrome {:?}, delta {:.4}",
                    v.viewport[0],
                    v.viewport[1],
                    v.dsf,
                    r.node,
                    r.keys,
                    if r.part.is_empty() { "-" } else { &r.part },
                    if r.at.is_empty() { "no source" } else { &r.at },
                    r.ours,
                    r.chrome,
                    r.delta
                ));
            }
        }
        for e in &self.runtime_errors {
            out.push(format!("runtime: {e}"));
        }
        out.push(if self.ok() {
            "verify: layout matches Chrome".to_string()
        } else {
            "verify: the layout differs from Chrome".to_string()
        });
        out
    }
}

/// What a `verify` run produced: the report, and the package and Chrome's dumps it was made from.
#[derive(Debug)]
pub struct Outcome {
    /// The comparison.
    pub report: Report,
    /// The package bytes that ran.
    pub package: Vec<u8>,
    /// Chrome's dumps, in [`VIEWPORTS`] order.
    pub dumps: Vec<Value>,
}

fn fail(msg: impl Into<String>) -> Vec<ConvError> {
    vec![ConvError::new(codes::PROTOCOL, Place::default(), msg)]
}

/// Builds the skin (the package and its font subsets), takes Chrome's rectangles of `state` at the reference
/// viewports and compares them with the runtime's.
pub fn verify(skin_dir: &Path, state: &str) -> Result<Outcome, Vec<ConvError>> {
    let out = crate::build::build(skin_dir, true);
    if !out.run.errors.is_empty() {
        return Err(out.run.errors);
    }
    let written = out.written.ok_or_else(|| fail("no package was written"))?;
    let src = out.run.source.as_ref().ok_or_else(|| fail("no source"))?;
    let mut node = crate::node::NodeClient::start().map_err(|e| vec![e])?;
    let opts = crate::Opts {
        fonts_dir: Some(out.out_dir.join("fonts")),
    };
    let mut dumps = Vec::new();
    let got = crate::open(&mut node, src, &opts)
        .map_err(|e| vec![e])
        .and_then(|_| {
            for (vp, dsf) in VIEWPORTS {
                let d = node
                .call(
                    "rects",
                    json!({ "state": state, "viewport": vp, "dsf": dsf, "skin": src.skin.skin.id }),
                )
                .map_err(|e| vec![e])?;
                dumps.push(d);
            }
            Ok(())
        });
    node.close();
    got?;
    let sample_path = skin_dir.join("sample").join(format!("{state}.json"));
    let sample = std::fs::read_to_string(&sample_path)
        .map_err(|e| fail(format!("cannot read {}: {e}", sample_path.display())))?;
    let report =
        compare(&written.bytes, &sample, &src.skin.skin.id, state, &dumps).map_err(fail)?;
    Ok(Outcome {
        report,
        package: written.bytes,
        dumps,
    })
}

fn env(vp: [u32; 2], dsf: f32) -> Env {
    Env {
        viewport_px: ((vp[0] as f32 * dsf) as u32, (vp[1] as f32 * dsf) as u32),
        dpr: dsf,
        ui_scale: 1.0,
        theme: 0,
    }
}

/// Runs the package on the sample at the viewports of `dumps` (one dump per [`VIEWPORTS`] entry, in order) and
/// compares the rectangles.
pub fn compare(
    package: &[u8],
    sample: &str,
    skin: &str,
    state: &str,
    dumps: &[Value],
) -> Result<Report, String> {
    let pkg = Arc::new(Package::read(Arc::from(package)).map_err(|e| format!("package: {e}"))?);
    let mut engine = TextEngine::new();
    load_faces(&pkg, &mut engine).map_err(|e| format!("fonts: {e:?}"))?;
    let sample =
        Sample::parse(Schema::v1(), sample).map_err(|e| format!("sample {state}: {e:?}"))?;
    if dumps.is_empty() {
        return Err("no Chrome dump".to_string());
    }
    let mut rt = Runtime::new(
        pkg.clone(),
        env(VIEWPORTS[0].0, VIEWPORTS[0].1),
        Services::default(),
    )
    .map_err(|e| format!("runtime: {e:?}"))?;
    let mut store = ModelStore::new(Schema::v1());
    sample
        .apply(&mut store)
        .map_err(|e| format!("sample {state}: {e:?}"))?;
    let mut scene = Scene::new(1.0);
    // The runtime makes the `for` and `if` bodies itself from the model (keyed instances in model order).
    if let Some(d) = store.take_frame() {
        rt.apply_delta(&d, &store);
    }

    let mut report = Report {
        skin: skin.to_string(),
        state: state.to_string(),
        ..Report::default()
    };
    for ((vp, dsf), dump) in VIEWPORTS.iter().zip(dumps) {
        rt.set_env(env(*vp, *dsf));
        rt.frame(0.0, &store, &mut engine, &mut scene);
        let ours = rt.dump_rects(None);
        let theirs = rects_of(dump)?;
        let mut v = ViewportReport {
            viewport: *vp,
            dsf: *dsf,
            rows: Vec::new(),
            mismatch: None,
        };
        let ids = |n: usize, f: &dyn Fn(usize) -> String| (0..n).map(f).collect::<Vec<_>>();
        let a = ids(ours.len(), &|i| {
            format!("{}|{}", ours[i].node, ours[i].keys)
        });
        let b = ids(theirs.len(), &|i| {
            format!("{}|{}", theirs[i].node, theirs[i].keys)
        });
        if a != b {
            v.mismatch = Some(first_difference(&a, &b));
        }
        for (o, c) in ours.iter().zip(&theirs) {
            let ov = [o.x, o.y, o.w, o.h].map(f64::from);
            let delta = (0..4).map(|i| (ov[i] - c.r[i]).abs()).fold(0.0, f64::max);
            let (part, at) = describe(&pkg, &c.node);
            v.rows.push(Row {
                node: c.node.clone(),
                keys: c.keys.clone(),
                part,
                at,
                ours: ov,
                chrome: c.r,
                delta,
            });
        }
        report.viewports.push(v);
    }
    // Painting reports what it cannot draw yet (the gradient of the research scale, outlines); nothing else.
    for e in rt.take_errors() {
        let s = format!("{e:?}");
        if !(s.contains("gradient") || s.contains("outline") || s.contains("game asset")) {
            report.runtime_errors.push(s);
        }
    }
    Ok(report)
}

fn first_difference(ours: &[String], theirs: &[String]) -> String {
    let n = ours.len().min(theirs.len());
    match (0..n).find(|&i| ours[i] != theirs[i]) {
        Some(i) => format!(
            "record {i}: runtime {} / Chrome {} ({} vs {} records)",
            ours[i],
            theirs[i],
            ours.len(),
            theirs.len()
        ),
        None => format!(
            "the runtime has {} records, Chrome {}",
            ours.len(),
            theirs.len()
        ),
    }
}

/// The nearest part of a `<template>:<index>[#k]` record and its source place.
fn describe(pkg: &Package, node: &str) -> (String, String) {
    let base = node.split('#').next().unwrap_or(node);
    let Some((t, i)) = base.split_once(':') else {
        return Default::default();
    };
    let (Ok(t), Ok(i)) = (t.parse::<usize>(), i.parse::<usize>()) else {
        return Default::default();
    };
    let Some(tpl) = pkg.templates().get(t) else {
        return Default::default();
    };
    let at = pkg
        .source_loc(ui2_package::TemplateId(t as u32), i as u32)
        .map(|(f, l, _)| format!("{f}:{l}"))
        .unwrap_or_default();
    // Ancestors in preorder: a node j before i whose subtree reaches past i.
    let mut part = String::new();
    for j in 0..=i.min(tpl.nodes.len().saturating_sub(1)) {
        let n = &tpl.nodes[j];
        if (j == i || j + n.subtree as usize > i)
            && let Some(p) = n.part.and_then(|id| pkg.str(id))
        {
            part = p.to_string();
        }
    }
    (part, at)
}
