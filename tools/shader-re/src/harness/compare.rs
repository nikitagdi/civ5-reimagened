//! Comparison of a hand-written module with the oracle.
//!
//! The metric (SR-D10): for every component of every output, `abs(a - b) <= 1e-4 * max(1,
//! abs(a), abs(b))`; NaN matches NaN, an infinity matches the same infinity.

use std::collections::BTreeMap;
use std::fmt::Write;

use serde::Serialize;

use super::formats::Formats;
use super::random;
use super::subject::{ConstSource, Subject};
use super::{EntryCtx, GRID, Gpu, HarnessError, PIXELS, RunOutput, SEEDS, make_scene, run_subject};
use crate::oracle::{self, Oracle};
use crate::scan::Stage;
use crate::sm3::Shader;
use crate::sm3::ctab::RegisterSet;
use crate::status::DataMove;

/// Relative (and, below 1, absolute) tolerance of the metric.
pub const TOLERANCE: f32 = 1e-4;

/// `abs(a - b) / (TOLERANCE * max(1, abs(a), abs(b)))`: at most 1 means the values match.
/// Values that cannot be subtracted (NaN against a number, opposite infinities) give infinity;
/// equal NaNs and equal infinities give 0.
pub fn error_ratio(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        return if a.is_nan() && b.is_nan() {
            0.0
        } else {
            f32::INFINITY
        };
    }
    if a.is_infinite() || b.is_infinite() {
        return if a == b { 0.0 } else { f32::INFINITY };
    }
    (a - b).abs() / (TOLERANCE * 1f32.max(a.abs()).max(b.abs()))
}

/// The metric of SR-D10 for one component.
pub fn components_match(a: f32, b: f32) -> bool {
    error_ratio(a, b) <= 1.0
}

/// The worst value of a comparison, with what is needed to reproduce it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Worst {
    /// `seed 2, linear` / `seed 2, point`.
    pub run: String,
    pub pixel: (u32, u32),
    pub output: String,
    pub component: usize,
    pub oracle: [f32; 4],
    pub subject: [f32; 4],
    pub ratio: f32,
    /// The inputs of the pixel (`v0`, ..., `vface`).
    pub inputs: Vec<(String, [f32; 4])>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    pub entry: String,
    pub sha1: String,
    pub runs: usize,
    pub values: usize,
    /// Reference values that are finite and not zero: a run where this is 0 compared nothing.
    pub live: usize,
    pub mismatches: usize,
    pub worst: Option<Worst>,
    /// Uniform members no CTAB constant claimed, and similar remarks.
    pub notes: Vec<String>,
}

impl Report {
    pub fn pass(&self) -> bool {
        self.mismatches == 0
    }

    pub fn summary(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "{}: {} ({} runs, {} values, {} finite and not zero, {} mismatches)",
            self.entry,
            if self.pass() { "ok" } else { "FAIL" },
            self.runs,
            self.values,
            self.live,
            self.mismatches
        );
        if let Some(w) = &self.worst {
            let _ = writeln!(
                s,
                "  worst: {}, pixel ({}, {}), output {}.{}: oracle {} subject {} (error x{:.3})",
                w.run,
                w.pixel.0,
                w.pixel.1,
                w.output,
                "xyzw".chars().nth(w.component).unwrap_or('?'),
                w.oracle[w.component],
                w.subject[w.component],
                w.ratio
            );
            let _ = writeln!(s, "  oracle  {:?}\n  subject {:?}", w.oracle, w.subject);
            for (n, v) in &w.inputs {
                let _ = writeln!(s, "  input {n} = {v:?}");
            }
        }
        for n in &self.notes {
            let _ = writeln!(s, "  note: {n}");
        }
        s
    }
}

/// What `compare` takes besides the two sides.
pub struct CompareOptions<'a> {
    pub formats: &'a Formats,
    /// `status.toml`: constants that are zero on both sides.
    pub zero_in_compare: &'a [String],
    pub data_moves: &'a [DataMove],
    /// `status.toml` `input_ranges`: input name to `(min, max)`.
    pub input_ranges: BTreeMap<String, (i32, i32)>,
    pub seeds: &'a [u64],
}

impl<'a> CompareOptions<'a> {
    pub fn new(formats: &'a Formats) -> CompareOptions<'a> {
        CompareOptions {
            formats,
            zero_in_compare: &[],
            data_moves: &[],
            input_ranges: BTreeMap::new(),
            seeds: &SEEDS,
        }
    }
}

/// The oracle for `shader` with the format views the map asks for.
pub fn oracle_for(
    shader: &Shader,
    name: &str,
    sha1: &str,
    formats: &Formats,
) -> Result<Oracle, HarnessError> {
    // First pass to learn the sampler names, second with the views.
    let plain = oracle::translate(shader, name, sha1, &oracle::Options::default())
        .map_err(|e| HarnessError::Wgsl(e.to_string()))?;
    let mut opts = oracle::Options::default();
    for s in &plain.samplers {
        if let Some(def) = formats.for_sampler(name, &s.ctab_name) {
            opts.formats.insert(s.ctab_name.clone(), def.oracle_view());
        }
    }
    if opts.formats.is_empty() {
        return Ok(plain);
    }
    oracle::translate(shader, name, sha1, &opts).map_err(|e| HarnessError::Wgsl(e.to_string()))
}

fn pixel_inputs(
    o: &Oracle,
    seed: u64,
    p: usize,
    ranges: &BTreeMap<String, (i32, i32)>,
) -> Vec<(String, [f32; 4])> {
    let mut v = Vec::new();
    for i in o.inputs.iter().filter(|i| i.usage.is_some()) {
        let name = format!("v{}", i.reg);
        let q = super::input_values(seed, &name, ranges)[p];
        v.push((name, q));
    }
    if o.inputs.iter().any(|i| i.usage.is_none() && i.reg == 1) {
        v.push(("vface".into(), random::face_slot(seed, PIXELS)[p]));
    }
    v
}

/// Compares the outputs of two runs of the same scene; `oracle` is the reference.
fn compare_runs(
    o: &Oracle,
    seed: u64,
    run_name: &str,
    reference: &RunOutput,
    subject: &RunOutput,
    ranges: &BTreeMap<String, (i32, i32)>,
    report: &mut Report,
) -> Result<(), HarnessError> {
    let is_ps = o.stage == Stage::Ps;
    // A module without a `kill` member never discards: the oracle declares it for every pixel
    // shader, so a missing one counts as "never".
    let never = vec![[0.0f32; 4]; PIXELS];
    let kill_ref = reference.get("kill").unwrap_or(&never);
    let kill_sub = subject.get("kill").unwrap_or(&never);
    for (name, ref_vals) in &reference.fields {
        let sub_vals = match subject.get(name) {
            Some(v) => v,
            None if name == "kill" => &never,
            None => {
                return Err(HarnessError::Contract(format!(
                    "the module under test has no output `{name}`, the oracle has"
                )));
            }
        };
        let comps = if name == "kill" {
            1
        } else {
            *subject.comps.get(name).unwrap_or(&4)
        };
        for p in 0..PIXELS {
            // A pixel that both sides discard has no outputs to compare.
            let killed = |k: &Vec<[f32; 4]>| k.get(p).is_some_and(|q| q[0] != 0.0);
            if is_ps && name != "kill" && killed(kill_ref) && killed(kill_sub) {
                continue;
            }
            for c in 0..comps {
                report.values += 1;
                if ref_vals[p][c].is_finite() && ref_vals[p][c] != 0.0 {
                    report.live += 1;
                }
                let ratio = error_ratio(ref_vals[p][c], sub_vals[p][c]);
                if ratio > 1.0 {
                    report.mismatches += 1;
                }
                let worse = report.worst.as_ref().is_none_or(|w| ratio > w.ratio);
                if ratio > 1.0 && worse {
                    report.worst = Some(Worst {
                        run: run_name.to_string(),
                        pixel: ((p as u32) % GRID, (p as u32) / GRID),
                        output: name.clone(),
                        component: c,
                        oracle: ref_vals[p],
                        subject: sub_vals[p],
                        ratio,
                        inputs: pixel_inputs(o, seed, p, ranges),
                    });
                }
            }
        }
    }
    Ok(())
}

/// Runs the oracle of `shader` and `subject` on the same random scenes and compares every
/// output. The oracle is made here, with the views of `opts.formats`.
pub fn compare_entry(
    gpu: &Gpu,
    shader: &Shader,
    name: &str,
    sha1: &str,
    subject: &Subject,
    opts: &CompareOptions<'_>,
) -> Result<Report, HarnessError> {
    let o = oracle_for(shader, name, sha1, opts.formats)?;
    compare_oracle(gpu, &o, subject, opts)
}

/// As [`compare_entry`], with the oracle already made.
pub fn compare_oracle(
    gpu: &Gpu,
    o: &Oracle,
    subject: &Subject,
    opts: &CompareOptions<'_>,
) -> Result<Report, HarnessError> {
    if subject.stage != o.stage {
        return Err(HarnessError::Contract(format!(
            "{} is a {:?} module, the oracle {:?}",
            subject.label, subject.stage, o.stage
        )));
    }
    let reference = Subject::from_oracle(o)?;
    let ctx = EntryCtx {
        name: &o.name,
        ctab: o.ctab.as_ref(),
        samplers: &o.samplers,
    };
    // Registers up to the end of the CTAB: the hand-written side may read constants the oracle
    // got from `def` or never reads.
    let ctab_end = o
        .ctab
        .iter()
        .flat_map(|c| &c.constants)
        .filter(|c| c.set == RegisterSet::Float4)
        .map(|c| c.reg as usize + c.count as usize)
        .max()
        .unwrap_or(0);
    let float_regs = (o.consts.map_or(0, |c| c.float_regs) as usize).max(ctab_end);
    let mut report = Report {
        entry: o.name.clone(),
        sha1: o.sha1.clone(),
        ..Default::default()
    };
    let mut notes: BTreeMap<String, ()> = BTreeMap::new();
    for &seed in opts.seeds {
        for linear in [true, false] {
            let mut scene = make_scene(
                &ctx,
                float_regs,
                seed,
                linear,
                opts.formats,
                opts.zero_in_compare,
                opts.data_moves,
            )?;
            scene.input_ranges = opts.input_ranges.clone();
            let run_name = format!("seed {seed}, {}", if linear { "linear" } else { "point" });
            let a = run_subject(gpu, &reference, &ctx, &scene)?;
            let b = run_subject(gpu, subject, &ctx, &scene)?;
            compare_runs(o, seed, &run_name, &a, &b, &opts.input_ranges, &mut report)?;
            report.runs += 1;
            if matches!(subject.consts, ConstSource::ByCtab) {
                for n in &b.unfilled {
                    notes.insert(
                        format!("uniform member `{n}` has no CTAB constant: left zero"),
                        (),
                    );
                }
            }
        }
    }
    report.notes = notes.into_keys().collect();
    Ok(report)
}
