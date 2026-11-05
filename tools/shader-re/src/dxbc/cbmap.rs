//! Constant map: the name of an SM3 constant (CTAB) -> where the DX11 twin reads it, `(cb, byte
//! offset, rows)`. Replay tests of `golden` fill the constants of the original
//! from the `cbuffer` bytes of a capture with it.
//!
//! The two blocks come from the same HLSL source, so the code is nearly the same instruction for
//! instruction (`mul r0.w, r0.z, c1.x` against `mul r0.w, r0.z, cb1[9].x`), but the constants are
//! numbered differently: SM3 gives every variable its own register in the order of declaration and
//! drops the unused ones; DX11 packs scalars into the free lanes of a float4 and keeps the layout
//! of the whole cbuffer. So the register of a constant cannot be computed; it is read from the code.
//! The compilers schedule the instructions differently (a `dp3` that comes first in SM3 may come
//! last in DX11), so the two programs are not aligned by position:
//!
//! 1. Every read of a constant (`c#` of SM3, `cb#[k]` of DX11) is described by the operation, the
//!    destination (`r1.x`; the temporaries keep their names between the two compilers), the other
//!    operands (by kind) and the components it uses (the lanes of the destination mask; 3 for
//!    `dp3`, 4 for `dp4`, 1 for the scalar functions).
//! 2. Reads are grouped by constant: an SM3 register, a DX11 cell `(cb, index)`. A register and a
//!    cell are as alike as the best one-to-one pairing of their reads is.
//! 3. The registers are assigned to the cells so that the total likeness is largest (the
//!    Hungarian method). Arrays and matrices are anchored by the rows that were read and
//!    extended over the rest; the component of a scalar inside its float4 comes from the pairs.
//! 4. A register that stays without a cell gets one if the order of declaration (which both
//!    compilers keep inside one cbuffer) leaves exactly one free cell for it.
//!
//! The result carries a confidence, and constants whose places break the order of declaration
//! inside a cbuffer are listed as suspect. What does not come out is written by hand to
//! `tools/shader-re/maps/<Package>.<Entry>.<STAGE>.toml` (names and offsets only; applied over the
//! automatic map). The final check is the replay test of `golden`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::asm_text::{CbIndex, Listing, TextInst};
use crate::sm3::ctab::RegisterSet;
use crate::sm3::ir::{Inst, Opcode, Reg, RegType, Shader, Stmt};

/// Points of a pair of reads: the same operation, the same destination, the same number of
/// components, the same kinds of the other operands.
const SAME_OPERATION: f64 = 3.0;
const SAME_DESTINATION: f64 = 0.5;
const SAME_COMPONENTS: f64 = 0.5;
/// Plus this when the lanes have the same shape too.
const SAME_SHAPE: f64 = 1.0;
const SAME_OPERANDS: f64 = 2.0;
/// The sampler of the nearest texture fetch before, and after, is the same (each counts).
const SAME_FETCH: f64 = 1.0;
/// Best score of one pair of reads (the scale of the confidence).
const BEST_PAIR: f64 = SAME_OPERATION
    + SAME_DESTINATION
    + SAME_COMPONENTS
    + SAME_SHAPE
    + SAME_OPERANDS
    + 2.0 * SAME_FETCH;
/// A register and a cell that are less alike than this are not assigned to each other.
const MIN_LIKENESS: f64 = 1.5;
/// Confidence of a place found by the order of declaration only.
const BY_ORDER: f64 = 0.2;
/// Below this a place is listed as suspect.
const SUSPECT_BELOW: f64 = 0.35;
/// Bytes of one float4 and of one component.
const VEC4: u32 = 16;
const LANE: u32 = 4;
/// Registers that can share one float4 of a cbuffer.
const LANES: usize = 4;

/// One read of a constant by an instruction.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Read {
    /// SM3 register, or the constant part of the DX11 index.
    index: u32,
    /// DX11 only: which `cb`.
    cb: u32,
    /// Dynamic index (`c[a0.x + 10]`, `cb0[r0.x + 4]`).
    rel: bool,
    /// Distinct components the read uses, in order of first use.
    comps: Vec<u8>,
    /// Normalised operation.
    op: String,
    /// `r1.x`, `o2.xyz`; empty when the instruction has no register destination.
    dst: String,
    /// Kinds of all the source operands, sorted: `c`, `l`, `r0`, `v`, ...
    others: Vec<String>,
    /// Sampler of the nearest texture fetch before and after the instruction.
    prev_smp: Option<u32>,
    next_smp: Option<u32>,
}

fn distinct(v: impl IntoIterator<Item = u8>) -> Vec<u8> {
    let mut out = Vec::new();
    for c in v {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// Lanes (positions 0..=3 of the swizzle) an SM3 source operand uses.
fn sm3_lanes(inst: &Inst, operand: usize) -> Vec<usize> {
    use Opcode::*;
    let n = |k: usize| (0..k).collect::<Vec<_>>();
    match inst.op {
        Dp2Add => {
            if operand < 2 {
                n(2)
            } else {
                n(1)
            }
        }
        Dp3 | Nrm => n(3),
        Dp4 => n(4),
        Rcp | Rsq | Exp | Log | ExpP | LogP | SinCos | Pow | Ifc | If | BreakC | Rep => n(1),
        TexLd | TexLdl | TexLdd | TexKill => n(4),
        _ => match &inst.dst {
            Some(d) => (0..4).filter(|k| d.mask & (1 << k) != 0).collect(),
            None => n(1),
        },
    }
}

/// SM3 operation names that stand for the same work as a DX11 one.
fn compatible(sm3: &str, dx: &str) -> bool {
    if sm3 == dx {
        return true;
    }
    let pairs: [(&str, &[&str]); 12] = [
        ("rcp", &["div"]),
        ("rsq", &["sqrt"]),
        ("cmp", &["movc"]),
        ("lrp", &["mad", "add"]),
        ("sub", &["add"]),
        ("pow", &["exp", "log", "mul"]),
        ("nrm", &["dp3", "rsq", "mul"]),
        ("dp2add", &["dp2", "mad", "add"]),
        ("slt", &["lt", "movc"]),
        ("sge", &["ge", "movc"]),
        ("mova", &["ftoi", "mov"]),
        ("abs", &["mov"]),
    ];
    pairs.iter().any(|(a, bs)| *a == sm3 && bs.contains(&dx))
        || sm3.starts_with("texld") && dx.starts_with("sample")
}

/// Kind of an SM3 register for the comparison with the DX11 text: temporaries and outputs keep
/// their numbers, the inputs do not (DX11 puts `SV_Position` in `v0`).
fn sm3_kind(r: Reg, defined: &BTreeSet<u32>) -> String {
    match r.ty {
        RegType::Temp => format!("r{}", r.num),
        RegType::Input => "v".to_string(),
        RegType::Output | RegType::ColorOut => format!("o{}", r.num),
        RegType::Const if defined.contains(&r.num) => "l".to_string(),
        RegType::Const | RegType::Const2 | RegType::Const3 | RegType::Const4 => "c".to_string(),
        RegType::DepthOut => "od".to_string(),
        _ => "?".to_string(),
    }
}

/// `r12` -> `r`: the numbers of the temporaries differ between the compilers when they allocate
/// registers differently, so the other operands are compared by kind only.
fn plain_kind(kind: &str) -> String {
    match kind.strip_prefix('r') {
        Some(n) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => "r".to_string(),
        _ => kind.to_string(),
    }
}

fn mask_letters(mask: u8) -> String {
    ["x", "y", "z", "w"]
        .iter()
        .enumerate()
        .filter(|(i, _)| mask & (1 << i) != 0)
        .map(|(_, c)| *c)
        .collect()
}

fn sm3_reads(sh: &Shader) -> Vec<Read> {
    let defined: BTreeSet<u32> = sh
        .stmts
        .iter()
        .filter_map(|s| match s {
            Stmt::DefF { reg, .. } if reg.ty == RegType::Const => Some(reg.num),
            _ => None,
        })
        .collect();
    let insts: Vec<&Inst> = sh.insts().collect();
    let fetch_smp: Vec<Option<u32>> = insts
        .iter()
        .map(|i| match i.op {
            Opcode::TexLd | Opcode::TexLdl | Opcode::TexLdd => i.src.get(1).map(|s| s.reg.num),
            _ => None,
        })
        .collect();
    let around = fetch_context(&fetch_smp);
    let mut out = Vec::new();
    for (n, inst) in insts.iter().copied().enumerate() {
        let dst = inst
            .dst
            .as_ref()
            .map(|d| format!("{}.{}", sm3_kind(d.reg, &defined), mask_letters(d.mask)))
            .unwrap_or_default();
        let mut others: Vec<String> = inst
            .src
            .iter()
            .map(|s| plain_kind(&sm3_kind(s.reg, &defined)))
            .collect();
        others.sort();
        for (k, src) in inst.src.iter().enumerate() {
            if src.reg.ty != RegType::Const {
                continue;
            }
            // A literal of the compiler: not a constant of the engine (not in the CTAB either).
            if src.rel.is_none() && defined.contains(&src.reg.num) {
                continue;
            }
            let lanes = sm3_lanes(inst, k);
            out.push(Read {
                index: src.reg.num,
                cb: 0,
                rel: src.rel.is_some(),
                comps: distinct(lanes.iter().map(|&l| src.swizzle[l])),
                op: inst.op.name().to_string(),
                dst: dst.clone(),
                others: others.clone(),
                prev_smp: around[n].0,
                next_smp: around[n].1,
            });
        }
    }
    out
}

/// For every instruction the sampler of the nearest texture fetch before it and after it (a fetch
/// is its own neighbour on neither side).
fn fetch_context(fetch: &[Option<u32>]) -> Vec<(Option<u32>, Option<u32>)> {
    let mut prev = vec![None; fetch.len()];
    let mut last = None;
    for (i, f) in fetch.iter().enumerate() {
        prev[i] = last;
        if f.is_some() {
            last = *f;
        }
    }
    let mut out = vec![(None, None); fetch.len()];
    let mut next = None;
    for i in (0..fetch.len()).rev() {
        out[i] = (prev[i], next);
        if fetch[i].is_some() {
            next = fetch[i];
        }
    }
    out
}

/// `r2.xyz` -> positions of the mask; `None` when the operand has no mask.
fn dst_lanes(operand: &str) -> Option<Vec<usize>> {
    let (_, m) = operand.rsplit_once('.')?;
    let v: Vec<usize> = m
        .bytes()
        .map_while(|b| match b {
            b'x' => Some(0),
            b'y' => Some(1),
            b'z' => Some(2),
            b'w' => Some(3),
            _ => None,
        })
        .collect();
    (!v.is_empty() && v.len() == m.len()).then_some(v)
}

fn dx_base_op(op: &str) -> String {
    op.split(['(', '_']).next().unwrap_or(op).to_string()
}

/// Kind of a DX11 operand: `r0.xyz` -> `r0`, `-v3.z` -> `v`, `l(1, 2)` -> `l`, `cb1[3].x` -> `c`.
fn dx_kind(operand: &str) -> String {
    let s = operand.trim_start_matches(['-', '|', '!']);
    let name: String = s
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("cb") {
        "c".to_string()
    } else if lower.starts_with('r') || lower.starts_with('o') && lower != "od" {
        lower
    } else if lower.starts_with('v') {
        "v".to_string()
    } else if lower.starts_with('l') {
        "l".to_string()
    } else {
        lower
    }
}

fn dx_reads(l: &Listing) -> Vec<Read> {
    let fetch: Vec<Option<u32>> = l
        .insts
        .iter()
        .map(|i| i.fetch.as_ref().map(|f| f.smp.unwrap_or(f.tex)))
        .collect();
    let around = fetch_context(&fetch);
    l.insts
        .iter()
        .enumerate()
        .flat_map(|(n, i)| dx_inst_reads(i, around[n]))
        .collect()
}

fn dx_inst_reads(inst: &TextInst, around: (Option<u32>, Option<u32>)) -> Vec<Read> {
    let op = dx_base_op(&inst.op);
    let first = inst.operands.first().map(String::as_str).unwrap_or("");
    let is_dst = first.starts_with(['r', 'o']) && !first.starts_with("ret");
    let dst_lanes_v = if is_dst { dst_lanes(first) } else { None };
    let dst = if is_dst {
        let mask = first.rsplit_once('.').map_or("", |x| x.1);
        format!("{}.{mask}", dx_kind(first))
    } else {
        String::new()
    };
    let mut others: Vec<String> = inst
        .operands
        .iter()
        .skip(usize::from(is_dst))
        .map(|o| plain_kind(&dx_kind(o)))
        .collect();
    others.sort();
    let n_lanes = match op.as_str() {
        "dp2" => Some(2),
        "dp3" => Some(3),
        "dp4" => Some(4),
        _ => None,
    };
    inst.cb_reads
        .iter()
        .map(|r| {
            // A swizzle of one letter is a scalar operand; a longer one is read at the lanes of the
            // destination (or the first lanes of a dot product).
            let comps = if r.swizzle.len() <= 1 {
                r.swizzle.clone()
            } else {
                let lanes: Vec<usize> = match (n_lanes, &dst_lanes_v) {
                    (Some(n), _) => (0..n).collect(),
                    (None, Some(d)) => d.clone(),
                    (None, None) => (0..r.swizzle.len()).collect(),
                };
                lanes
                    .iter()
                    .filter_map(|&k| r.swizzle.get(k).copied())
                    .collect()
            };
            let (index, rel) = match &r.index {
                CbIndex::Imm(n) => (*n, false),
                CbIndex::Rel { base, .. } => (*base, true),
            };
            Read {
                index,
                cb: r.cb,
                rel,
                comps: distinct(comps),
                op: op.clone(),
                dst: dst.clone(),
                others: others.clone(),
                prev_smp: around.0,
                next_smp: around.1,
            }
        })
        .collect()
}

/// The components a read uses, sorted and moved to start at 0: `yzw` and `xyz` have the same shape
/// (DX11 may pack a float3 into the last three lanes of a float4), `xyw` has another.
fn shape(comps: &[u8]) -> Vec<u8> {
    let mut v = comps.to_vec();
    v.sort_unstable();
    let first = v.first().copied().unwrap_or(0);
    v.iter().map(|c| c - first).collect()
}

fn read_score(a: &Read, b: &Read) -> f64 {
    if a.rel != b.rel {
        return 0.0;
    }
    let mut s = 0.0;
    if compatible(&a.op, &b.op) {
        s += SAME_OPERATION;
    }
    if !a.dst.is_empty() && a.dst == b.dst {
        s += SAME_DESTINATION;
    }
    if a.comps.len() == b.comps.len() {
        s += SAME_COMPONENTS;
        if shape(&a.comps) == shape(&b.comps) {
            s += SAME_SHAPE;
        }
    }
    if a.others == b.others {
        s += SAME_OPERANDS;
    }
    if a.prev_smp.is_some() && a.prev_smp == b.prev_smp {
        s += SAME_FETCH;
    }
    if a.next_smp.is_some() && a.next_smp == b.next_smp {
        s += SAME_FETCH;
    }
    s
}

/// The pairs `(read of a, read of b)` of the best one-to-one pairing (greedy by score) and their
/// total score.
fn pair_reads(a: &[&Read], b: &[&Read]) -> (Vec<(usize, usize)>, f64) {
    let mut all: Vec<(f64, usize, usize)> = Vec::new();
    for (i, x) in a.iter().enumerate() {
        for (j, y) in b.iter().enumerate() {
            let s = read_score(x, y);
            if s > 0.0 {
                all.push((s, i, j));
            }
        }
    }
    // Largest first; ties by position, so that the result is the same on every run.
    all.sort_by(|p, q| q.0.total_cmp(&p.0).then(p.1.cmp(&q.1)).then(p.2.cmp(&q.2)));
    let (mut used_a, mut used_b) = (vec![false; a.len()], vec![false; b.len()]);
    let (mut pairs, mut total) = (Vec::new(), 0.0);
    for (s, i, j) in all {
        if !used_a[i] && !used_b[j] {
            used_a[i] = true;
            used_b[j] = true;
            pairs.push((i, j));
            total += s;
        }
    }
    (pairs, total)
}

/// Assignment that maximises the total of `value[i][j]` (rows `n`, columns `m`, any shape):
/// `result[i]` is the column of row `i`. The Hungarian method on the padded square matrix.
fn assign_max(value: &[Vec<f64>], n: usize, m: usize) -> Vec<Option<usize>> {
    let k = n.max(m);
    let cost = |i: usize, j: usize| -> f64 { if i < n && j < m { -value[i][j] } else { 0.0 } };
    // Potentials method, 1-based as in the usual statement.
    let inf = f64::INFINITY;
    let (mut u, mut v) = (vec![0.0; k + 1], vec![0.0; k + 1]);
    let (mut p, mut way) = (vec![0usize; k + 1], vec![0usize; k + 1]);
    for i in 1..=k {
        p[0] = i;
        let mut j0 = 0;
        let mut minv = vec![inf; k + 1];
        let mut used = vec![false; k + 1];
        loop {
            used[j0] = true;
            let (i0, mut delta, mut j1) = (p[j0], inf, 0);
            for j in 1..=k {
                if !used[j] {
                    let cur = cost(i0 - 1, j - 1) - u[i0] - v[j];
                    if cur < minv[j] {
                        minv[j] = cur;
                        way[j] = j0;
                    }
                    if minv[j] < delta {
                        delta = minv[j];
                        j1 = j;
                    }
                }
            }
            for j in 0..=k {
                if used[j] {
                    u[p[j]] += delta;
                    v[j] -= delta;
                } else {
                    minv[j] -= delta;
                }
            }
            j0 = j1;
            if p[j0] == 0 {
                break;
            }
        }
        loop {
            let j1 = way[j0];
            p[j0] = p[j1];
            j0 = j1;
            if j0 == 0 {
                break;
            }
        }
    }
    let mut out = vec![None; n];
    for (j, &i) in p.iter().enumerate().skip(1) {
        if (1..=n).contains(&i) && j <= m {
            out[i - 1] = Some(j - 1);
        }
    }
    out
}

fn majority<K: Ord + Copy>(m: &BTreeMap<K, usize>) -> Option<K> {
    // Most votes; the smaller key on a tie, so that the result does not depend on hash order.
    m.iter()
        .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
        .map(|(k, _)| *k)
}

/// Where one CTAB constant lives in the DX11 cbuffers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConstMap {
    /// CTAB name.
    pub name: String,
    /// SM3 register of the first row.
    pub reg: u32,
    /// Registers of the constant (rows of a matrix, elements of an array).
    pub rows: u32,
    pub cb: u32,
    /// Byte offset in the cbuffer.
    pub offset: u32,
    /// `auto`, `inferred` (some rows were never read), `order` (found by the order of declaration
    /// only) or `manual`.
    pub source: String,
    /// How alike the reads of the register and of the cell are, 0..=1.
    pub confidence: f64,
    /// Reads of the register in SM3.
    pub reads: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Unmapped {
    pub name: String,
    pub reg: u32,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CbMap {
    pub entry: String,
    pub sm3_sha1: String,
    pub dx11_sha1: String,
    pub consts: Vec<ConstMap>,
    pub unmapped: Vec<Unmapped>,
    /// Reads of constants in SM3 and in DX11.
    pub sm3_reads: usize,
    pub dx11_reads: usize,
    /// Constants whose rows do not lie one after another in the cbuffer.
    pub scattered: Vec<String>,
    /// Constants with a low confidence, or whose place breaks the order of declaration inside a
    /// cbuffer: to be checked by hand or by the replay.
    pub suspect: Vec<String>,
}

impl CbMap {
    pub fn get(&self, name: &str) -> Option<&ConstMap> {
        self.consts.iter().find(|c| c.name == name)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("cbmap serializes") + "\n"
    }
}

/// An SM3 register (`rel`: the base of an array read through an index) and a DX11 cell.
type SmKey = (bool, u32);
type DxKey = (u32, bool, u32);

/// Builds the map of an SM3 block from its DX11 twin's listing.
pub fn cb_map(
    entry: &str,
    sm3_sha1: &str,
    dx11_sha1: &str,
    sh: &Shader,
    listing: &Listing,
) -> CbMap {
    let s = sm3_reads(sh);
    let d = dx_reads(listing);
    let mut out = CbMap {
        entry: entry.to_string(),
        sm3_sha1: sm3_sha1.to_string(),
        dx11_sha1: dx11_sha1.to_string(),
        sm3_reads: s.len(),
        dx11_reads: d.len(),
        ..CbMap::default()
    };
    let Some(ctab) = &sh.ctab else {
        return out;
    };

    let mut sg: BTreeMap<SmKey, Vec<&Read>> = BTreeMap::new();
    for r in &s {
        sg.entry((r.rel, r.index)).or_default().push(r);
    }
    let mut dg: BTreeMap<DxKey, Vec<&Read>> = BTreeMap::new();
    for r in &d {
        dg.entry((r.cb, r.rel, r.index)).or_default().push(r);
    }
    let skeys: Vec<SmKey> = sg.keys().copied().collect();
    let dkeys: Vec<DxKey> = dg.keys().copied().collect();
    // Likeness of every register and cell, and the pairs of reads behind it.
    // DX11 packs scalars and small vectors into one float4: up to four registers share a cell,
    // so every cell is offered `LANES` times (the later copies a little less attractive).
    let mut likeness = vec![vec![0.0; dkeys.len()]; skeys.len()];
    let mut value = vec![vec![0.0; dkeys.len() * LANES]; skeys.len()];
    for (i, sk) in skeys.iter().enumerate() {
        for (j, dk) in dkeys.iter().enumerate() {
            if sk.0 != dk.1 {
                continue;
            }
            let (a, b) = (&sg[sk], &dg[dk]);
            let (_, total) = pair_reads(a, b);
            // The share of the register's reads that the cell explains, and a little for equal
            // numbers of reads.
            let like = total / a.len() as f64 + 0.01 * total / a.len().max(b.len()) as f64;
            likeness[i][j] = like;
            for c in 0..LANES {
                value[i][j * LANES + c] = like - 0.001 * c as f64;
            }
        }
    }
    // Registers that share a cell must use different lanes of it; when two claim the same lane
    // the less alike one is sent elsewhere and the assignment is made again.
    let mut place: BTreeMap<SmKey, (DxKey, f64, u32)> = BTreeMap::new();
    for _ in 0..(skeys.len() * LANES).max(1) {
        let assignment = assign_max(&value, skeys.len(), dkeys.len() * LANES);
        place.clear();
        // Per cell: the registers in it with their lanes, `(row of the register, lanes, likeness)`.
        let mut cells: BTreeMap<usize, Vec<(usize, u8, f64)>> = BTreeMap::new();
        for (i, a) in assignment.iter().enumerate() {
            let Some(col) = *a else { continue };
            let j = col / LANES;
            if likeness[i][j] < MIN_LIKENESS {
                continue;
            }
            let (a_reads, b_reads) = (&sg[&skeys[i]], &dg[&dkeys[j]]);
            let (pairs, _) = pair_reads(a_reads, b_reads);
            let mut deltas: BTreeMap<u32, usize> = BTreeMap::new();
            let mut lanes = 0u8;
            for (x, y) in pairs {
                for c in &b_reads[y].comps {
                    lanes |= 1 << c;
                }
                if let (Some(p), Some(q)) = (a_reads[x].comps.first(), b_reads[y].comps.first())
                    && q >= p
                {
                    *deltas.entry(u32::from(q - p)).or_insert(0) += 1;
                }
            }
            place.insert(
                skeys[i],
                (
                    dkeys[j],
                    (likeness[i][j] / BEST_PAIR).min(1.0),
                    majority(&deltas).unwrap_or(0),
                ),
            );
            cells.entry(j).or_default().push((i, lanes, likeness[i][j]));
        }
        let mut changed = false;
        for (j, regs) in &cells {
            for (k, &(i, lanes, like)) in regs.iter().enumerate() {
                // The loser of a pair that overlaps: the less alike one; the earlier on a tie.
                let loses = regs.iter().enumerate().any(|(k2, &(_, l2, like2))| {
                    k2 != k && lanes & l2 != 0 && (like2, k2) > (like, k)
                });
                if loses {
                    for c in 0..LANES {
                        value[i][j * LANES + c] = 0.0;
                    }
                    likeness[i][*j] = 0.0;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    struct Found {
        cb: u32,
        start: u32,
        lane: u32,
        conf: f64,
        source: &'static str,
        reads: usize,
    }
    let mut found: BTreeMap<String, Found> = BTreeMap::new();
    let mut float_consts: Vec<(String, u32, u32)> = Vec::new();
    for c in &ctab.constants {
        let (name, reg, rows) = (c.name.clone(), u32::from(c.reg), u32::from(c.count.max(1)));
        if c.set != RegisterSet::Float4 {
            if c.set != RegisterSet::Sampler {
                out.unmapped.push(Unmapped {
                    name,
                    reg,
                    reason: format!("{:?} constants are not mapped", c.set),
                });
            }
            continue;
        }
        float_consts.push((name, reg, rows));
    }
    for (name, reg, rows) in &float_consts {
        // Anchors: the registers of the constant that have a cell, each saying where row 0 is.
        let mut starts: BTreeMap<(u32, i64), (f64, usize)> = BTreeMap::new();
        let mut lane = 0;
        let mut reads = 0;
        for r in *reg..*reg + *rows {
            for rel in [false, true] {
                let Some(&((cb, _, idx), conf, l)) = place.get(&(rel, r)) else {
                    continue;
                };
                let e = starts
                    .entry((cb, i64::from(idx) - i64::from(r - *reg)))
                    .or_insert((0.0, 0));
                e.0 += conf;
                e.1 += 1;
                if *rows == 1 {
                    lane = l;
                }
                reads += sg.get(&(rel, r)).map_or(0, Vec::len);
            }
        }
        let Some((&(cb, start), &(conf_sum, n))) = starts
            .iter()
            .max_by(|a, b| a.1.0.total_cmp(&b.1.0).then(b.0.cmp(a.0)))
        else {
            continue;
        };
        if starts.len() > 1 {
            out.scattered.push(name.clone());
        }
        if start < 0 {
            out.unmapped.push(Unmapped {
                name: name.clone(),
                reg: *reg,
                reason: format!("rows start at index {start}"),
            });
            continue;
        }
        found.insert(
            name.clone(),
            Found {
                cb,
                start: start as u32,
                lane,
                conf: conf_sum / n as f64,
                source: if (n as u32) < *rows {
                    "inferred"
                } else {
                    "auto"
                },
                reads,
            },
        );
    }

    // Registers still without a cell: the order of declaration inside a cbuffer is kept by both
    // compilers, so a free cell between the cells of the neighbours is theirs when it is the only one.
    loop {
        // Cells that no constant occupies yet (rows of the found constants included).
        let mut occupied: BTreeSet<(u32, u32)> = BTreeSet::new();
        for (name, _, rows) in &float_consts {
            if let Some(f) = found.get(name) {
                for k in 0..*rows {
                    occupied.insert((f.cb, f.start + k));
                }
            }
        }
        let free: Vec<DxKey> = dkeys
            .iter()
            .copied()
            .filter(|(cb, rel, idx)| !rel && !occupied.contains(&(*cb, *idx)))
            .collect();
        let mut progress = false;
        for (name, reg, rows) in &float_consts {
            if found.contains_key(name) || *rows != 1 || !sg.contains_key(&(false, *reg)) {
                continue;
            }
            let mut candidates: Vec<DxKey> = Vec::new();
            let cbs: BTreeSet<u32> = found.values().map(|f| f.cb).collect();
            for cb in cbs {
                let lower = float_consts
                    .iter()
                    .filter(|(n, r, _)| r < reg && found.get(n).is_some_and(|f| f.cb == cb))
                    .filter_map(|(n, _, rows)| found.get(n).map(|f| f.start + rows - 1))
                    .max();
                let upper = float_consts
                    .iter()
                    .filter(|(n, r, _)| r > reg && found.get(n).is_some_and(|f| f.cb == cb))
                    .filter_map(|(n, _, _)| found.get(n).map(|f| f.start))
                    .min();
                candidates.extend(free.iter().copied().filter(|(c, _, idx)| {
                    *c == cb && lower.is_none_or(|l| *idx > l) && upper.is_none_or(|u| *idx < u)
                }));
            }
            if let [(cb, _, idx)] = candidates.as_slice() {
                let reads = &sg[&(false, *reg)];
                let cell = &dg[&(*cb, false, *idx)];
                let lane = match (
                    reads.iter().flat_map(|r| r.comps.iter()).min(),
                    cell.iter().flat_map(|r| r.comps.iter()).min(),
                ) {
                    (Some(p), Some(q)) if q >= p => u32::from(q - p),
                    _ => 0,
                };
                found.insert(
                    name.clone(),
                    Found {
                        cb: *cb,
                        start: *idx,
                        lane,
                        conf: BY_ORDER,
                        source: "order",
                        reads: reads.len(),
                    },
                );
                progress = true;
                break;
            }
        }
        if !progress {
            break;
        }
    }

    // Both compilers keep the order of declaration inside a cbuffer. Single registers that were
    // swapped (a lower register behind a higher one) get their places back in order: the places
    // stay, the names take them in the order of the registers.
    let cbs: BTreeSet<u32> = found.values().map(|f| f.cb).collect();
    for cb in cbs {
        let mut singles: Vec<(u32, String, u32)> = float_consts
            .iter()
            .filter(|(n, _, rows)| *rows == 1 && found.get(n).is_some_and(|f| f.cb == cb))
            .map(|(n, reg, _)| {
                let f = &found[n];
                (*reg, n.clone(), f.start * VEC4 + f.lane * LANE)
            })
            .collect();
        singles.sort();
        let inverted: Vec<usize> = (0..singles.len())
            .filter(|&i| {
                (0..singles.len()).any(|j| {
                    (j < i && singles[j].2 > singles[i].2) || (j > i && singles[j].2 < singles[i].2)
                })
            })
            .collect();
        let mut offsets: Vec<u32> = inverted.iter().map(|&i| singles[i].2).collect();
        offsets.sort_unstable();
        for (&i, off) in inverted.iter().zip(offsets) {
            if let Some(f) = found.get_mut(&singles[i].1)
                && f.start * VEC4 + f.lane * LANE != off
            {
                f.start = off / VEC4;
                f.lane = (off % VEC4) / LANE;
                f.source = "order";
                f.conf = f.conf.min(BY_ORDER * 2.0);
            }
        }
    }

    for (name, reg, rows) in &float_consts {
        match found.get(name) {
            Some(f) => out.consts.push(ConstMap {
                name: name.clone(),
                reg: *reg,
                rows: *rows,
                cb: f.cb,
                offset: f.start * VEC4 + f.lane * LANE,
                source: f.source.to_string(),
                confidence: f.conf,
                reads: f.reads,
            }),
            None if out.unmapped.iter().any(|u| &u.name == name) => {}
            None => out.unmapped.push(Unmapped {
                name: name.clone(),
                reg: *reg,
                reason: "no cell is like its reads".to_string(),
            }),
        }
    }
    // Suspect: low confidence, or a place that breaks the order of declaration in its cbuffer.
    let mut by_cb: BTreeMap<u32, Vec<&ConstMap>> = BTreeMap::new();
    for c in &out.consts {
        by_cb.entry(c.cb).or_default().push(c);
    }
    let mut suspect: BTreeSet<String> = BTreeSet::new();
    for v in by_cb.values_mut() {
        v.sort_by_key(|c| c.reg);
        for w in v.windows(2) {
            // A scalar takes one lane, an array or a matrix all its rows.
            let end = w[0].offset
                + if w[0].rows > 1 {
                    w[0].rows * VEC4
                } else {
                    LANE
                };
            if w[1].offset < end {
                suspect.insert(w[0].name.clone());
                suspect.insert(w[1].name.clone());
            }
        }
    }
    for c in &out.consts {
        if c.confidence < SUSPECT_BELOW && c.source != "manual" {
            suspect.insert(c.name.clone());
        }
    }
    out.suspect = suspect.into_iter().collect();
    out
}

/// A hand-written map of one entry: `maps/<Package>.<Entry>.<STAGE>.toml`.
///
/// ```toml
/// entry = "UnitShader::UnitPS:PS"
/// sha1 = "cb0f..."        # optional: the SM3 block the offsets were read for
/// [const.g_FogConsts]
/// cb = 1
/// offset = 144            # bytes
/// rows = 1
/// ```
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualMap {
    pub entry: String,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default, rename = "const")]
    pub consts: BTreeMap<String, ManualConst>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualConst {
    pub cb: u32,
    pub offset: u32,
    #[serde(default = "one")]
    pub rows: u32,
}

fn one() -> u32 {
    1
}

/// File name of the manual map of an entry: `::` and `:` become `.`.
pub fn manual_file_name(entry: &str) -> String {
    format!("{}.toml", entry.replace("::", ".").replace(':', "."))
}

impl ManualMap {
    pub fn parse(text: &str) -> Result<ManualMap, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    pub fn load(dir: &Path, entry: &str) -> Result<Option<ManualMap>, String> {
        let p = dir.join(manual_file_name(entry));
        if !p.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let m = ManualMap::parse(&text).map_err(|e| format!("{}: {e}", p.display()))?;
        if m.entry != entry {
            return Err(format!(
                "{}: entry is {:?}, expected {entry:?}",
                p.display(),
                m.entry
            ));
        }
        Ok(Some(m))
    }

    /// Puts the hand-written places over the map; unknown names are an error (a typo would
    /// otherwise silently change nothing). Constants that were unmapped leave that list.
    pub fn apply(&self, map: &mut CbMap, sh: &Shader) -> Result<(), String> {
        if let Some(h) = &self.sha1
            && !map.sm3_sha1.starts_with(h.as_str())
        {
            return Err(format!(
                "{}: the map was written for the block {h}, the entry has {}",
                map.entry, map.sm3_sha1
            ));
        }
        let ctab = sh.ctab.as_ref().ok_or("the block has no CTAB")?;
        for (name, m) in &self.consts {
            let c = ctab
                .constants
                .iter()
                .find(|c| &c.name == name && c.set == RegisterSet::Float4)
                .ok_or_else(|| format!("{}: no float constant {name:?} in the CTAB", map.entry))?;
            let new = ConstMap {
                name: name.clone(),
                reg: u32::from(c.reg),
                rows: m.rows,
                cb: m.cb,
                offset: m.offset,
                source: "manual".to_string(),
                confidence: 1.0,
                reads: 0,
            };
            map.unmapped.retain(|u| &u.name != name);
            map.scattered.retain(|n| n != name);
            map.suspect.retain(|n| n != name);
            match map.consts.iter_mut().find(|x| &x.name == name) {
                Some(x) => *x = new,
                None => map.consts.push(new),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::asm_text::parse_listing;
    use super::*;
    use crate::sm3::ctab::{Ctab, CtabConstant, CtabType, ParamClass, ParamType};
    use crate::sm3::ir::*;

    fn konst(name: &str, reg: u16, count: u16, class: ParamClass, cols: u16) -> CtabConstant {
        CtabConstant {
            name: name.to_string(),
            set: RegisterSet::Float4,
            reg,
            count,
            ty: CtabType {
                class,
                ty: ParamType::Float,
                rows: 1,
                columns: cols,
                elements: 1,
                members: Vec::new(),
            },
            default: None,
        }
    }

    fn c(n: u32, sw: [u8; 4]) -> Src {
        Src {
            reg: Reg {
                ty: RegType::Const,
                num: n,
            },
            rel: None,
            swizzle: sw,
            modifier: SrcMod::None,
        }
    }

    fn t(n: u32) -> Src {
        Src {
            reg: Reg {
                ty: RegType::Temp,
                num: n,
            },
            rel: None,
            swizzle: [0, 1, 2, 3],
            modifier: SrcMod::None,
        }
    }

    fn inst(op: Opcode, d: u32, mask: u8, src: Vec<Src>) -> Stmt {
        Stmt::Inst(Inst {
            op,
            control: 0,
            dst: Some(Dst {
                reg: Reg {
                    ty: RegType::Temp,
                    num: d,
                },
                rel: None,
                mask,
                mods: 0,
                shift: 0,
            }),
            src,
            pred: None,
        })
    }

    /// `float4 a` at c1 (cb1[0]), `float s` at c2.x (DX11 packs it into cb1[1].x), a float3 `v` at
    /// c3 (cb1[1].yzw), a `float4x4 m` at c4..c7 (cb0[2..6]); `c0` is a literal.
    fn shader() -> Shader {
        Shader {
            version: Version {
                stage: crate::scan::Stage::Ps,
                major: 3,
                minor: 0,
            },
            ctab: Some(Ctab {
                creator: String::new(),
                version: 0,
                flags: 0,
                target: "ps_3_0".into(),
                constants: vec![
                    konst("a", 1, 1, ParamClass::Vector, 4),
                    konst("s", 2, 1, ParamClass::Scalar, 1),
                    konst("v", 3, 1, ParamClass::Vector, 3),
                    konst("m", 4, 4, ParamClass::MatrixColumns, 4),
                ],
            }),
            comments: Vec::new(),
            stmts: vec![
                Stmt::DefF {
                    reg: Reg {
                        ty: RegType::Const,
                        num: 0,
                    },
                    value: [1.0; 4],
                },
                inst(Opcode::Mul, 0, 0xF, vec![t(1), c(1, [0, 1, 2, 3])]),
                inst(Opcode::Mul, 0, 0x8, vec![t(1), c(2, [0, 0, 0, 0])]),
                inst(Opcode::Mul, 1, 0x7, vec![t(1), c(3, [0, 1, 2, 3])]),
                inst(Opcode::Dp4, 2, 0x1, vec![t(1), c(4, [0, 1, 2, 3])]),
                inst(Opcode::Dp4, 2, 0x2, vec![t(1), c(5, [0, 1, 2, 3])]),
                inst(Opcode::Add, 3, 0x7, vec![t(1), c(0, [0, 1, 2, 3])]),
            ],
        }
    }

    /// The same code with the instructions in another order, as the DX11 compiler may schedule
    /// them.
    const LISTING: &str = "ps_5_0\n\
        dcl_constantbuffer CB1[1], immediateIndexed\n\
        dcl_constantbuffer CB0[6], immediateIndexed\n\
        dp4 r2.y, r1.xyzw, cb0[3].xyzw\n\
        mul r1.xyz, r1.xyzw, cb1[1].yzwx\n\
        mul r0.xyzw, r1.xyzw, cb1[0].xyzw\n\
        dp4 r2.x, r1.xyzw, cb0[2].xyzw\n\
        mul r0.w, r1.xyzw, cb1[1].x\n\
        add r3.xyz, r1.xyzw, l(1.000000, 1.000000, 1.000000, 1.000000)\n\
        ret\n";

    #[test]
    fn constants_are_mapped_whatever_the_order_of_the_instructions() {
        let l = parse_listing(LISTING).unwrap();
        let m = cb_map("T::P:PS", "aa", "bb", &shader(), &l);
        // The literal `def c0` is not a constant of the engine.
        assert_eq!(m.sm3_reads, 5);
        assert_eq!(m.dx11_reads, 5);
        let a = m.get("a").unwrap();
        assert_eq!((a.cb, a.offset, a.rows), (1, 0, 1));
        // A packed scalar in the first lane of cb1[1], and a float3 in the three lanes behind it.
        let s = m.get("s").unwrap();
        assert_eq!((s.cb, s.offset), (1, 16));
        let v = m.get("v").unwrap();
        assert_eq!((v.cb, v.offset), (1, 16 + 4));
        // The matrix: rows 4 and 5 were read; 6 and 7 follow by contiguity.
        let mm = m.get("m").unwrap();
        assert_eq!((mm.cb, mm.offset, mm.rows), (0, 2 * 16, 4));
        assert_eq!(mm.source, "inferred");
        assert!(m.unmapped.is_empty(), "{:?}", m.unmapped);
        assert!(m.scattered.is_empty());
        assert!(m.suspect.is_empty(), "{:?}", m.suspect);
    }

    #[test]
    fn a_constant_without_a_like_read_gets_the_free_cell_of_its_place() {
        // `s` is read by an instruction that has no look-alike in DX11; cb1[2] is the only free
        // cell between `a` (cb1[1]) and `v` (cb1[3]).
        let mut sh = shader();
        sh.stmts[2] = inst(Opcode::Add, 2, 0x8, vec![c(2, [0, 0, 0, 0]), t(2)]);
        let l = parse_listing(
            "ps_5_0\n\
             mul r0.xyzw, r1.xyzw, cb1[1].xyzw\n\
             max r4.xyz, v1.xyzw, cb1[2].xyzx\n\
             mul r1.xyz, r1.xyzw, cb1[3].xyzx\n\
             dp4 r2.x, r1.xyzw, cb0[2].xyzw\n\
             dp4 r2.y, r1.xyzw, cb0[3].xyzw\n",
        )
        .unwrap();
        let m = cb_map("T::P:PS", "aa", "bb", &sh, &l);
        let s = m.get("s").unwrap();
        assert_eq!((s.cb, s.offset, s.source.as_str()), (1, 2 * 16, "order"));
        assert!(m.suspect.contains(&"s".to_string()));
    }

    #[test]
    fn manual_maps_override_and_check_names() {
        let l = parse_listing(LISTING).unwrap();
        let sh = shader();
        let mut m = cb_map("T::P:PS", "aabbcc", "bb", &sh, &l);
        let man = ManualMap::parse(
            "entry = \"T::P:PS\"\nsha1 = \"aabb\"\n[const.a]\ncb = 2\noffset = 32\nrows = 1\n",
        )
        .unwrap();
        man.apply(&mut m, &sh).unwrap();
        let a = m.get("a").unwrap();
        assert_eq!((a.cb, a.offset, a.source.as_str()), (2, 32, "manual"));
        // A typo is an error, and so is a block that is not the one the file was written for.
        let typo =
            ManualMap::parse("entry = \"T::P:PS\"\n[const.zz]\ncb = 0\noffset = 0\n").unwrap();
        assert!(typo.apply(&mut m, &sh).is_err());
        let stale = ManualMap::parse("entry = \"T::P:PS\"\nsha1 = \"ffff\"\n").unwrap();
        assert!(stale.apply(&mut m, &sh).is_err());
        assert!(ManualMap::parse("entry = \"x\"\nbogus = 1\n").is_err());
        assert_eq!(
            manual_file_name("UnitShader::UnitPS:PS"),
            "UnitShader.UnitPS.PS.toml"
        );
    }

    #[test]
    fn assignment_finds_the_best_total() {
        // Greedy would give row 0 its best column 0 (9) and row 1 the leftover (1): 10 in all;
        // the best total is 8 + 8.
        let v = vec![vec![9.0, 8.0], vec![8.0, 1.0]];
        assert_eq!(assign_max(&v, 2, 2), [Some(1), Some(0)]);
        // More rows than columns: one row stays without.
        let v = vec![vec![5.0], vec![7.0], vec![1.0]];
        assert_eq!(assign_max(&v, 3, 1), [None, Some(0), None]);
        // More columns than rows.
        let v = vec![vec![1.0, 4.0, 2.0]];
        assert_eq!(assign_max(&v, 1, 3), [Some(1)]);
    }

    #[test]
    fn dot_product_lanes_and_operand_kinds() {
        let ins = TextInst {
            op: "dp3".into(),
            operands: vec!["r0.x".into(), "r1.xyzx".into(), "cb0[1].xyzx".into()],
            cb_reads: vec![super::super::asm_text::cb_read("cb0[1].xyzx").unwrap()],
            fetch: None,
        };
        // Three lanes, whatever the destination says.
        let r = &dx_inst_reads(&ins, (None, None))[0];
        assert_eq!(r.comps, [0, 1, 2]);
        assert_eq!(r.dst, "r0.x");
        assert_eq!(r.others, ["c", "r"]);
        assert_eq!(dst_lanes("r2.yzw"), Some(vec![1, 2, 3]));
        assert_eq!(dst_lanes("r2"), None);
        assert_eq!(dx_kind("-v3.z"), "v");
        assert_eq!(dx_kind("l(1.0, 2.0)"), "l");
        assert_eq!(dx_kind("o1.xyz"), "o1");
        assert!(
            compatible("rcp", "div") && compatible("texldl", "sample") && !compatible("mul", "dp3")
        );
    }
}
