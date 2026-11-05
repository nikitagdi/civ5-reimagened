//! Twins SM3 <-> DX11.
//!
//! A twin is the DX11 block built from the same HLSL source as an SM3 block. DX11 blocks have no
//! names, so the twins are found by features and by order:
//!
//! - The DX11 exe stores each shader as the blocks of its profiles one after another, highest
//!   first (`vs_5_0`, `vs_4_1`, `vs_4_0`); a shader built for fewer profiles has fewer blocks.
//!   These runs are the *groups* (hull and domain shaders stand alone).
//! - The groups come in the order of the SM3 blocks of the DX9 exe (the registration order), with
//!   shaders that exist only in DX11 in between (research: 493 groups for 433 SM3 blocks). So the
//!   twins are a monotone matching of the SM3 blocks to the vertex and pixel groups.
//! - The matching maximises the sum of a feature score (stage, input and output semantics,
//!   texture kinds, instruction count) minus the price of skipped groups (dynamic programming).
//!   A DX11 signature declares the whole vertex or interpolant struct while an SM3 block declares
//!   what the code reads, so the SM3 side has to be a subset of the DX11 side, not equal to it.
//! - The score and two flags are kept for each twin: `quality` (every SM3 input and output is in
//!   the DX11 signature and the texture kinds agree) and `ambiguous` (a neighbouring group scores
//!   the same, so only the order decides).
//!
//! The 11 world entries that have no SM3 block (`dynsym` slots only) get a *candidate* from the
//! groups that no SM3 block took, by the package of the neighbours in the file.
//!
//! `confirmed` comes from the draw calls of the `golden` captures (`draws.json`, schema
//! `civ5-golden-draws/1`, read by `read_draw_pairs`): a call whose
//! vertex and pixel blocks are twins of entries of one package confirms both.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::Dxbc;
use crate::catalog::{BlockInfo, Catalog};
use crate::scan::{Block, Stage};

/// Price of a DX11 group that no SM3 block takes (DX11-only shaders are common).
const GAP_GROUP: f64 = -0.05;
/// Price of an SM3 block without a twin (never wanted: every block has one in the pinned build).
const GAP_SM3: f64 = -2.0;
/// Profiles of a group, highest first.
pub const PROFILES: [&str; 3] = ["5_0", "4_1", "4_0"];

/// One DX11 shader: its blocks of the profiles `*_5_0`, `*_4_1`, `*_4_0` (any subset), in file order.
#[derive(Clone, Debug)]
pub struct Group {
    pub blocks: Vec<Block>,
}

impl Group {
    pub fn stage(&self) -> Stage {
        self.blocks[0].stage
    }

    /// Block of the profile `5_0`, `4_1` or `4_0`.
    pub fn block(&self, profile: &str) -> Option<&Block> {
        self.blocks
            .iter()
            .find(|b| b.profile.split_once('_').is_some_and(|x| x.1 == profile))
    }
}

/// Profile rank inside a group: `5_0` first, then `4_1`, then `4_0`.
fn rank(profile: &str) -> u8 {
    match profile.split_once('_').map_or("", |x| x.1) {
        "5_0" => 0,
        "4_1" => 1,
        _ => 2,
    }
}

/// Splits the blocks of the DX11 exe into groups.
pub fn group_blocks(blocks: &[Block]) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for b in blocks {
        let joins = groups.last().is_some_and(|g| {
            let last = g.blocks.last().expect("a group has a block");
            last.stage == b.stage
                && matches!(b.stage, Stage::Vs | Stage::Ps)
                && rank(&b.profile) > rank(&last.profile)
        });
        if joins {
            groups.last_mut().expect("checked").blocks.push(b.clone());
        } else {
            groups.push(Group {
                blocks: vec![b.clone()],
            });
        }
    }
    groups
}

/// Group shapes: profiles joined -> count.
pub fn shapes(groups: &[Group]) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for g in groups {
        let k = g
            .blocks
            .iter()
            .map(|b| b.profile.as_str())
            .collect::<Vec<_>>()
            .join("+");
        *m.entry(k).or_insert(0) += 1;
    }
    m
}

type Bag = BTreeMap<(String, u32), usize>;

fn bag(items: impl IntoIterator<Item = (String, u32)>) -> Bag {
    let mut m = Bag::new();
    for k in items {
        *m.entry(k).or_insert(0) += 1;
    }
    m
}

/// What a shader shows to the matcher; the same shape for both sides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Features {
    pub stage: Stage,
    /// Inputs the code needs: `(SEMANTIC, index)`; no system values.
    pub inputs: Bag,
    /// Outputs: vertex shader interpolants, or pixel shader colours `("COLOR", n)` and `("DEPTH", 0)`.
    pub outputs: Bag,
    /// Texture kinds: `2d`, `cube`, `volume`, `1d` -> count.
    pub textures: BTreeMap<String, usize>,
    pub instructions: usize,
}

impl Features {
    /// From the catalog's SM3 block.
    pub fn of_sm3(b: &BlockInfo) -> Features {
        let up = |s: &str| s.to_ascii_uppercase();
        let vs = b.stage == Stage::Vs;
        let inputs = bag(b
            .inputs
            .iter()
            .filter(|i| i.semantic != "misc")
            .map(|i| (up(&i.semantic), u32::from(i.index))));
        let outputs = bag(b
            .outputs
            .iter()
            .filter(|o| !(vs && o.semantic == "position"))
            .map(|o| (up(&o.semantic), u32::from(o.index))));
        let mut textures = BTreeMap::new();
        for s in &b.samplers {
            *textures.entry(s.kind.clone()).or_insert(0) += 1;
        }
        Features {
            stage: b.stage,
            inputs,
            outputs,
            textures,
            instructions: b.instructions,
        }
    }

    /// From a DX11 container.
    pub fn of_dxbc(d: &Dxbc) -> Features {
        let up = |s: &str| s.to_ascii_uppercase();
        let user = |e: &&super::chunks::SigElement| !e.name.to_ascii_uppercase().starts_with("SV_");
        let inputs = bag(d
            .isgn
            .elements
            .iter()
            .filter(user)
            .map(|e| (up(&e.name), e.index)));
        let outputs = if d.stage() == Stage::Ps {
            bag(d.osgn.elements.iter().filter_map(|e| {
                let n = up(&e.name);
                if n == "SV_TARGET" {
                    Some(("COLOR".to_string(), e.index))
                } else if n.starts_with("SV_DEPTH") {
                    Some(("DEPTH".to_string(), 0))
                } else {
                    None
                }
            }))
        } else {
            bag(d
                .osgn
                .elements
                .iter()
                .filter(user)
                .map(|e| (up(&e.name), e.index)))
        };
        let mut textures = BTreeMap::new();
        for r in &d.decls().resources {
            if let Some(k) = r.dim.sm3_kind() {
                *textures.entry(k.to_string()).or_insert(0) += 1;
            }
        }
        Features {
            stage: d.stage(),
            inputs,
            outputs,
            textures,
            instructions: d.decls().instructions,
        }
    }
}

/// Share of the entries of `a` that `b` has too (1 when `a` is empty).
fn cover(a: &Bag, b: &Bag) -> f64 {
    let n: usize = a.values().sum();
    if n == 0 {
        return 1.0;
    }
    let hit: usize = a
        .iter()
        .map(|(k, v)| (*v).min(b.get(k).copied().unwrap_or(0)))
        .sum();
    hit as f64 / n as f64
}

fn jaccard(a: &Bag, b: &Bag) -> f64 {
    let keys: BTreeSet<_> = a.keys().chain(b.keys()).collect();
    if keys.is_empty() {
        return 1.0;
    }
    let (mut inter, mut uni) = (0usize, 0usize);
    for k in keys {
        let (x, y) = (
            a.get(k).copied().unwrap_or(0),
            b.get(k).copied().unwrap_or(0),
        );
        inter += x.min(y);
        uni += x.max(y);
    }
    inter as f64 / uni as f64
}

/// Score of the pair `(sm3, dx)`, or `None` when the stages differ.
pub fn score(sm3: &Features, dx: &Features) -> Option<f64> {
    if sm3.stage != dx.stage {
        return None;
    }
    let tex_a: Bag = sm3
        .textures
        .iter()
        .map(|(k, v)| ((k.clone(), 0), *v))
        .collect();
    let tex_b: Bag = dx
        .textures
        .iter()
        .map(|(k, v)| ((k.clone(), 0), *v))
        .collect();
    let (a, b) = (sm3.instructions as f64, dx.instructions as f64);
    let size = 1.0 / (1.0 + (a - b).abs() / a.max(1.0));
    Some(
        1.5 * cover(&sm3.inputs, &dx.inputs)
            + 0.5 * jaccard(&sm3.inputs, &dx.inputs)
            + 1.5 * cover(&sm3.outputs, &dx.outputs)
            + 0.5 * jaccard(&sm3.outputs, &dx.outputs)
            + 2.0 * jaccard(&tex_a, &tex_b)
            + size,
    )
}

/// The best monotone matching: `result[i]` is the index into `dx` of the twin of `sm3[i]`.
pub fn align(sm3: &[Features], dx: &[Features]) -> Vec<Option<usize>> {
    let (n, m) = (sm3.len(), dx.len());
    let neg = f64::NEG_INFINITY;
    let mut dp = vec![vec![neg; m + 1]; n + 1];
    #[derive(Clone, Copy)]
    enum Step {
        None,
        SkipGroup,
        SkipSm3,
        Match,
    }
    let mut from = vec![vec![Step::None; m + 1]; n + 1];
    dp[0][0] = 0.0;
    for i in 0..=n {
        for j in 0..=m {
            let v = dp[i][j];
            if v == neg {
                continue;
            }
            if j < m && v + GAP_GROUP > dp[i][j + 1] {
                dp[i][j + 1] = v + GAP_GROUP;
                from[i][j + 1] = Step::SkipGroup;
            }
            if i < n && v + GAP_SM3 > dp[i + 1][j] {
                dp[i + 1][j] = v + GAP_SM3;
                from[i + 1][j] = Step::SkipSm3;
            }
            if i < n
                && j < m
                && let Some(s) = score(&sm3[i], &dx[j])
                && v + s > dp[i + 1][j + 1]
            {
                dp[i + 1][j + 1] = v + s;
                from[i + 1][j + 1] = Step::Match;
            }
        }
    }
    let mut out = vec![None; n];
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        match from[i][j] {
            Step::Match => {
                out[i - 1] = Some(j - 1);
                i -= 1;
                j -= 1;
            }
            Step::SkipGroup => j -= 1,
            Step::SkipSm3 => i -= 1,
            Step::None => break,
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    /// Every SM3 input and output is in the DX11 signature and the texture kinds agree.
    Strong,
    Weak,
}

fn quality(sm3: &Features, dx: &Features) -> Quality {
    if cover(&sm3.inputs, &dx.inputs) == 1.0
        && cover(&sm3.outputs, &dx.outputs) == 1.0
        && sm3.textures == dx.textures
    {
        Quality::Strong
    } else {
        Quality::Weak
    }
}

/// The twin of an SM3 block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Twin {
    /// `UnitShader::UnitPS:PS`.
    pub entry: String,
    pub world: bool,
    pub sm3_sha1: String,
    /// Index of the group in file order.
    pub group: usize,
    /// DX11 blocks by profile (`5_0`, `4_1`, `4_0`) -> sha1.
    pub blocks: BTreeMap<String, String>,
    pub score: f64,
    pub quality: Quality,
    /// A neighbouring group scores the same.
    pub ambiguous: bool,
    /// All blocks of the group have the same signatures and texture kinds.
    pub profiles_agree: bool,
    /// A draw call of a capture shows this block with the twin of another entry of its package.
    pub confirmed: bool,
}

/// A DX11-only candidate for an entry without SM3 block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub entry: String,
    /// Groups that fit by stage and by the package of their neighbours, strongest evidence only.
    pub groups: Vec<usize>,
    /// `inside` (both neighbours are of the package), `boundary` (one is), or `none`.
    pub evidence: String,
    /// The blocks of the chosen group when it is unique.
    pub blocks: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TwinReport {
    pub groups: usize,
    pub twins: Vec<Twin>,
    pub candidates: Vec<Candidate>,
    /// Groups no SM3 block took (vertex and pixel only).
    pub unmatched_groups: Vec<usize>,
}

impl TwinReport {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("report serializes") + "\n"
    }

    pub fn twin_of(&self, entry: &str) -> Option<&Twin> {
        self.twins.iter().find(|t| t.entry == entry)
    }

    pub fn candidate_of(&self, entry: &str) -> Option<&Candidate> {
        self.candidates.iter().find(|c| c.entry == entry)
    }

    /// Strings for `Entry::dx11_twins`: `<profile>:<sha1>`, highest profile first.
    pub fn catalog_strings(&self, entry: &str) -> Vec<String> {
        let Some(t) = self.twin_of(entry) else {
            return Vec::new();
        };
        PROFILES
            .iter()
            .filter_map(|p| {
                t.blocks
                    .get(*p)
                    .map(|s| format!("{}_{}:{s}", stage_prefix(t), p))
            })
            .collect()
    }
}

fn stage_prefix(t: &Twin) -> &'static str {
    // The profile keys carry no stage; the first block's sha1 does not either: the entry name does.
    if t.entry.ends_with(":VS") { "vs" } else { "ps" }
}

/// SM3 blocks in file order of `Civ5XP`: `(addr, sha1)`.
fn sm3_sequence(cat: &Catalog) -> Vec<(u64, &BlockInfo)> {
    let mut v: Vec<(u64, &BlockInfo)> = cat
        .blocks
        .iter()
        .flat_map(|b| b.linux.iter().map(move |l| (l.addr, b)))
        .collect();
    v.sort_by_key(|(a, _)| *a);
    v
}

/// Computes the twins of all entries with an SM3 block and the candidates of those without.
pub fn compute(cat: &Catalog, dx_blocks: &[Block], dx: &[Dxbc]) -> TwinReport {
    assert_eq!(dx_blocks.len(), dx.len(), "one container per block");
    let groups = group_blocks(dx_blocks);
    // Features of every group: the highest profile's container.
    let by_sha1: BTreeMap<&str, &Dxbc> = dx_blocks
        .iter()
        .zip(dx)
        .map(|(b, d)| (b.sha1.as_str(), d))
        .collect();
    let gfeat: Vec<Features> = groups
        .iter()
        .map(|g| Features::of_dxbc(by_sha1[g.blocks[0].sha1.as_str()]))
        .collect();
    // Only vertex and pixel groups take part.
    let eligible: Vec<usize> = (0..groups.len())
        .filter(|&g| matches!(groups[g].stage(), Stage::Vs | Stage::Ps))
        .collect();
    let dxf: Vec<Features> = eligible.iter().map(|&g| gfeat[g].clone()).collect();

    let seq = sm3_sequence(cat);
    let sfeat: Vec<Features> = seq.iter().map(|(_, b)| Features::of_sm3(b)).collect();
    let matching = align(&sfeat, &dxf);

    let entry_of_addr: BTreeMap<u64, &crate::catalog::Entry> = cat
        .entries
        .iter()
        .filter_map(|e| e.sm3.as_ref().map(|b| (u64::from(b.addr), e)))
        .collect();

    let mut twins = Vec::new();
    let mut taken: BTreeSet<usize> = BTreeSet::new();
    // Group (index in `groups`) -> the SM3 sequence index that took it.
    let mut owner: BTreeMap<usize, usize> = BTreeMap::new();
    for (i, m) in matching.iter().enumerate() {
        let Some(j) = *m else { continue };
        let g = eligible[j];
        taken.insert(g);
        owner.insert(g, i);
        let (addr, block) = seq[i];
        let Some(entry) = entry_of_addr.get(&addr) else {
            continue;
        };
        let s = score(&sfeat[i], &dxf[j]).expect("matched pairs have equal stages");
        // Between the twins of the neighbouring SM3 blocks every group is free: one of them that
        // scores the same could take this block's place without breaking the order.
        let lo = matching[..i]
            .iter()
            .rev()
            .find_map(|m| *m)
            .map_or(0, |p| p + 1);
        let hi = matching[i + 1..]
            .iter()
            .find_map(|m| *m)
            .unwrap_or(dxf.len());
        let ambiguous = (lo..hi)
            .filter(|&k| k != j)
            .any(|k| score(&sfeat[i], &dxf[k]).is_some_and(|s2| s2 >= s - 1e-9));
        let profiles_agree = groups[g].blocks.iter().all(|b| {
            let f = Features::of_dxbc(by_sha1[b.sha1.as_str()]);
            f.inputs == gfeat[g].inputs
                && f.outputs == gfeat[g].outputs
                && f.textures == gfeat[g].textures
        });
        let blocks = groups[g]
            .blocks
            .iter()
            .filter_map(|b| {
                let p = b.profile.split_once('_')?.1.to_string();
                Some((p, b.sha1.clone()))
            })
            .collect();
        twins.push(Twin {
            entry: entry.name.clone(),
            world: entry.world,
            sm3_sha1: block.sha1.clone(),
            group: g,
            blocks,
            score: s,
            quality: quality(&sfeat[i], &dxf[j]),
            ambiguous,
            profiles_agree,
            confirmed: false,
        });
    }
    twins.sort_by(|a, b| a.entry.cmp(&b.entry));

    // Entries without SM3 block: groups nobody took, by the package of the neighbours.
    let pkg_of_seq =
        |i: usize| -> Option<&str> { entry_of_addr.get(&seq[i].0).map(|e| e.package.as_str()) };
    // For each untaken eligible group: the SM3 index of the nearest taken group before and after.
    let mut neighbours: BTreeMap<usize, (Option<usize>, Option<usize>)> = BTreeMap::new();
    let mut prev: Option<usize> = None;
    for &g in &eligible {
        match owner.get(&g) {
            Some(&i) => prev = Some(i),
            None => {
                neighbours.insert(g, (prev, None));
            }
        }
    }
    let mut next: Option<usize> = None;
    for &g in eligible.iter().rev() {
        match owner.get(&g) {
            Some(&i) => next = Some(i),
            None => {
                neighbours.get_mut(&g).expect("inserted above").1 = next;
            }
        }
    }
    let mut candidates = Vec::new();
    for e in cat.entries.iter().filter(|e| e.world && e.sm3.is_none()) {
        let stage = if e.stage == "VS" {
            Stage::Vs
        } else {
            Stage::Ps
        };
        let mut inside = Vec::new();
        let mut boundary = Vec::new();
        for (&g, &(p, n)) in &neighbours {
            if groups[g].stage() != stage {
                continue;
            }
            let hit = |x: Option<usize>| x.and_then(pkg_of_seq) == Some(e.package.as_str());
            match (hit(p), hit(n)) {
                (true, true) => inside.push(g),
                (true, false) | (false, true) => boundary.push(g),
                _ => {}
            }
        }
        let (evidence, gs) = if !inside.is_empty() {
            ("inside", inside)
        } else if !boundary.is_empty() {
            ("boundary", boundary)
        } else {
            ("none", Vec::new())
        };
        let blocks = match gs.as_slice() {
            [g] => groups[*g]
                .blocks
                .iter()
                .filter_map(|b| Some((b.profile.split_once('_')?.1.to_string(), b.sha1.clone())))
                .collect(),
            _ => BTreeMap::new(),
        };
        candidates.push(Candidate {
            entry: e.name.clone(),
            groups: gs,
            evidence: evidence.to_string(),
            blocks,
        });
    }
    let unmatched_groups: Vec<usize> = eligible
        .iter()
        .copied()
        .filter(|g| !taken.contains(g))
        .collect();
    TwinReport {
        groups: groups.len(),
        twins,
        candidates,
        unmatched_groups,
    }
}

/// Reads the vertex and pixel block pairs of the draw calls out of a `draws.json` of `golden`.
///
/// The schema of that file belongs to `golden` ("draws.json"): `schema` is
/// `civ5-golden-draws/1` and `draws` is an array of calls, each with `vs` and `ps` = the sha1 (40 hex
/// digits) of the DXBC container of that stage, or `null` when the stage is off. A call without a
/// pixel shader (depth only) pairs nothing and is skipped. Any other schema is an error.
pub fn read_draw_pairs(path: &Path) -> Result<Vec<(String, String)>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if v["schema"].as_str() != Some(DRAWS_SCHEMA) {
        return Err(format!(
            "{}: schema is {}, expected \"{DRAWS_SCHEMA}\"",
            path.display(),
            v["schema"]
        ));
    }
    let draws = v["draws"]
        .as_array()
        .ok_or_else(|| format!("{}: no `draws` array", path.display()))?;
    let sha = |d: &serde_json::Value, k: &str| -> Result<Option<String>, String> {
        match &d[k] {
            serde_json::Value::Null => Ok(None),
            serde_json::Value::String(s) if is_sha1(s) => Ok(Some(s.to_ascii_lowercase())),
            other => Err(format!(
                "{}: draw {}: `{k}` is {other}, expected 40 hex digits or null",
                path.display(),
                d["event"]
            )),
        }
    };
    let mut out = Vec::new();
    for d in draws {
        if let (Some(vs), Some(ps)) = (sha(d, "vs")?, sha(d, "ps")?) {
            out.push((vs, ps));
        }
    }
    Ok(out)
}

/// Schema tag of `draws.json` of `golden`.
pub const DRAWS_SCHEMA: &str = "civ5-golden-draws/1";

fn is_sha1(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Marks the twins that the draw calls confirm; returns how many.
pub fn confirm(report: &mut TwinReport, pairs: &[(String, String)]) -> usize {
    // DX11 block sha1 -> index of the twin.
    let mut by_block: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, t) in report.twins.iter().enumerate() {
        for s in t.blocks.values() {
            by_block.insert(s.as_str(), i);
        }
    }
    let package = |entry: &str| entry.split("::").next().unwrap_or("").to_string();
    let mut hits = BTreeSet::new();
    for (vs, ps) in pairs {
        if let (Some(&a), Some(&b)) = (by_block.get(vs.as_str()), by_block.get(ps.as_str()))
            && package(&report.twins[a].entry) == package(&report.twins[b].entry)
        {
            hits.insert(a);
            hits.insert(b);
        }
    }
    for &i in &hits {
        report.twins[i].confirmed = true;
    }
    hits.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(
        stage: Stage,
        ins: &[(&str, u32)],
        outs: &[(&str, u32)],
        tex: &[(&str, usize)],
        n: usize,
    ) -> Features {
        Features {
            stage,
            inputs: bag(ins.iter().map(|(s, i)| (s.to_string(), *i))),
            outputs: bag(outs.iter().map(|(s, i)| (s.to_string(), *i))),
            textures: tex.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            instructions: n,
        }
    }

    fn block(profile: &str, stage: Stage, sha: &str) -> Block {
        Block {
            sha1: sha.to_string(),
            kind: crate::scan::Kind::Dxbc,
            stage,
            profile: profile.to_string(),
            offset: 0,
            addr: 0,
            size: 0,
            section: String::new(),
        }
    }

    #[test]
    fn blocks_are_grouped_by_profile_runs() {
        let bs = [
            block("vs_5_0", Stage::Vs, "a"),
            block("vs_4_1", Stage::Vs, "b"),
            block("vs_4_0", Stage::Vs, "c"),
            block("ps_5_0", Stage::Ps, "d"),
            block("ps_5_0", Stage::Ps, "e"),
            block("ps_4_1", Stage::Ps, "f"),
            block("hs_5_0", Stage::Hs, "g"),
            block("hs_5_0", Stage::Hs, "h"),
        ];
        let g = group_blocks(&bs);
        let lens: Vec<usize> = g.iter().map(|g| g.blocks.len()).collect();
        assert_eq!(lens, [3, 1, 2, 1, 1]);
        assert_eq!(g[0].block("4_1").unwrap().sha1, "b");
        assert!(g[1].block("4_1").is_none());
        let s = shapes(&g);
        assert_eq!(s["vs_5_0+vs_4_1+vs_4_0"], 1);
        assert_eq!(s["hs_5_0"], 2);
    }

    #[test]
    fn subset_signatures_score_high_and_other_stages_not_at_all() {
        let sm3 = f(
            Stage::Vs,
            &[("POSITION", 0), ("TEXCOORD", 0)],
            &[("TEXCOORD", 0)],
            &[],
            10,
        );
        let dx_full = f(
            Stage::Vs,
            &[("POSITION", 0), ("TEXCOORD", 0), ("NORMAL", 0)],
            &[("TEXCOORD", 0)],
            &[],
            11,
        );
        let dx_other = f(Stage::Vs, &[("BLENDWEIGHT", 0)], &[("COLOR", 0)], &[], 10);
        assert!(score(&sm3, &dx_full).unwrap() > score(&sm3, &dx_other).unwrap());
        assert_eq!(quality(&sm3, &dx_full), Quality::Strong);
        assert_eq!(quality(&sm3, &dx_other), Quality::Weak);
        let ps = f(Stage::Ps, &[], &[], &[], 10);
        assert!(score(&sm3, &ps).is_none());
    }

    #[test]
    fn alignment_skips_dx11_only_shaders_and_keeps_the_order() {
        let a = |n| {
            f(
                Stage::Ps,
                &[("TEXCOORD", 0)],
                &[("COLOR", 0)],
                &[("2d", 1)],
                n,
            )
        };
        let b = |n| {
            f(
                Stage::Ps,
                &[("TEXCOORD", 0), ("TEXCOORD", 1)],
                &[("COLOR", 0)],
                &[("cube", 2)],
                n,
            )
        };
        let c = |n| {
            f(
                Stage::Ps,
                &[("COLOR", 0)],
                &[("COLOR", 0), ("COLOR", 1)],
                &[],
                n,
            )
        };
        let sm3 = [a(10), b(40), c(5)];
        // The DX11 side has an extra shader before and one between.
        let dx = [
            f(
                Stage::Ps,
                &[("NORMAL", 0)],
                &[("COLOR", 0)],
                &[("volume", 1)],
                300,
            ),
            a(11),
            f(
                Stage::Ps,
                &[("NORMAL", 0)],
                &[("COLOR", 0)],
                &[("volume", 1)],
                90,
            ),
            b(41),
            c(6),
        ];
        assert_eq!(align(&sm3, &dx), [Some(1), Some(3), Some(4)]);
    }

    #[test]
    fn draw_pairs_follow_the_golden_schema() {
        let dir = std::env::temp_dir().join(format!("shader-re-draws-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("draws.json");
        let h = |c: char| c.to_string().repeat(40);
        std::fs::write(
            &p,
            format!(
                r#"{{"schema": "{DRAWS_SCHEMA}", "draws": [
                    {{"event": 5, "vs": "{}", "ps": "{}"}},
                    {{"event": 6, "vs": "{}", "ps": null}}],
                  "by_entry": {{"X::Y:PS": [5]}}}}"#,
                h('a'),
                h('B'),
                h('c')
            ),
        )
        .unwrap();
        assert_eq!(read_draw_pairs(&p).unwrap(), [(h('a'), h('b'))]);
        // Another schema, and a stage that is not a sha1, are errors.
        std::fs::write(&p, r#"{"schema": "other/1", "draws": []}"#).unwrap();
        assert!(read_draw_pairs(&p).unwrap_err().contains("expected"));
        std::fs::write(
            &p,
            format!(r#"{{"schema": "{DRAWS_SCHEMA}", "draws": [{{"event": 1, "vs": "nothex", "ps": null}}]}}"#),
        )
        .unwrap();
        assert!(read_draw_pairs(&p).unwrap_err().contains("40 hex"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The sample file of `golden` (its tests parse the same file into its own types).
    #[test]
    fn reads_the_draws_sample_of_golden() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../golden/tests/fixtures/draws.sample.json");
        let pairs = read_draw_pairs(&p).unwrap();
        // Two calls with both stages (the depth-only one is skipped); hex case is folded.
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, pairs[0].1);
    }

    #[test]
    fn confirmation_needs_one_package() {
        let t = |entry: &str, sha: &str| Twin {
            entry: entry.to_string(),
            world: true,
            sm3_sha1: String::new(),
            group: 0,
            blocks: [("5_0".to_string(), sha.to_string())].into(),
            score: 1.0,
            quality: Quality::Strong,
            ambiguous: false,
            profiles_agree: true,
            confirmed: false,
        };
        let mut r = TwinReport {
            groups: 3,
            twins: vec![t("U::VS:VS", "v"), t("U::PS:PS", "p"), t("W::PS:PS", "w")],
            candidates: Vec::new(),
            unmatched_groups: Vec::new(),
        };
        assert_eq!(confirm(&mut r, &[("v".into(), "w".into())]), 0);
        assert_eq!(confirm(&mut r, &[("v".into(), "p".into())]), 2);
        assert!(r.twins[0].confirmed && r.twins[1].confirmed && !r.twins[2].confirmed);
    }
}
