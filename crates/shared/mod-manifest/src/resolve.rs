//! The deterministic resolver (D4).
//!
//! Input: the host's picks (id + version requirement), the installed candidates (many versions per id) and the
//! host's `MOD_API_VERSION`. Output: a [`ModSet`] in load order, plus warnings.
//!
//! 1. Hard dependencies are added transitively; one version per id: the highest installed version that satisfies
//!    every requirement on that id and admits the host API. The search is depth-first over the ids in ascending
//!    order, highest version first, with backtracking (bounded by `STEP_LIMIT`). `conflicts` are checked each time
//!    a version is chosen, in both directions.
//! 2. Optional dependencies never pull a mod in. If the optional mod is present and in range it is ordered
//!    before the declarer; if it is present but out of range it is ignored and a [`Warning`] is returned.
//! 3. Load order: topological over `deps`, present `optional`, and `order.after` / `order.before` (ids that are
//!    not in the set are ignored); ties are broken by id, ascending. A cycle is an error.
//!
//! The result depends only on the *sets* of picks and installed mods, never on their order in the slices.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::error::{ConflictError, ResolveError};
use crate::manifest::{Manifest, ModId};
use crate::modset::{ContentHash, ModSet, ResolvedMod};
use crate::semver::{Version, VersionReq};

/// Upper bound of version choices the backtracking search may try.
pub const STEP_LIMIT: u32 = 100_000;

/// What the host (the lobby) asks for: a mod and the versions it accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub id: ModId,
    pub req: VersionReq,
}

/// An installed mod version: its manifest and the hash of its content bundle (computed by the caller).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub manifest: Manifest,
    pub content_hash: ContentHash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// `by` listed `optional` in range `req`, but version `found` is in the set: ordering against it is ignored.
    OptionalOutOfRange {
        by: ModId,
        by_version: Version,
        optional: ModId,
        req: VersionReq,
        found: Version,
    },
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Warning::OptionalOutOfRange {
                by,
                by_version,
                optional,
                req,
                found,
            } => write!(
                f,
                "{by} {by_version} lists optional `{optional}` {req}, but {found} is loaded; it is not ordered before {by}"
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub set: ModSet,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Source {
    Host,
    Mod(ModId, Version),
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Host => f.write_str("the host"),
            Source::Mod(id, v) => write!(f, "{id} {v}"),
        }
    }
}

#[derive(Clone, Default)]
struct State<'a> {
    chosen: BTreeMap<ModId, &'a Installed>,
    /// Every requirement seen on an id, in insertion order.
    reqs: BTreeMap<ModId, Vec<(Source, VersionReq)>>,
    /// Who asked for an id first; follows back to the host.
    parent: BTreeMap<ModId, Source>,
}

impl State<'_> {
    fn add_req(&mut self, id: &ModId, src: Source, req: VersionReq) {
        self.parent.entry(id.clone()).or_insert_with(|| src.clone());
        self.reqs.entry(id.clone()).or_default().push((src, req));
    }

    /// The ancestors of `id`, host first, not including `id` itself.
    fn ancestors(&self, id: &ModId) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = id.clone();
        while out.len() < 64 {
            match self.parent.get(&cur) {
                Some(Source::Mod(p, v)) => {
                    out.push(format!("{p} {v}"));
                    cur = p.clone();
                }
                _ => break,
            }
        }
        out.push("host".to_string());
        out.reverse();
        out
    }

    fn chain_to(&self, id: &ModId, last: String) -> String {
        let mut c = self.ancestors(id);
        c.push(last);
        c.join(" -> ")
    }

    fn constraints_text(&self, id: &ModId) -> String {
        self.reqs
            .get(id)
            .map(|rs| {
                rs.iter()
                    .map(|(s, r)| format!("{s} needs `{id}` {r}"))
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .unwrap_or_default()
    }
}

struct Solver<'a> {
    by_id: BTreeMap<ModId, Vec<&'a Installed>>,
    api: &'a Version,
    steps: u32,
    exhausted: bool,
    /// The first dead end met in the most preferred branch: the error to report if nothing works.
    failure: Option<ResolveError>,
}

impl<'a> Solver<'a> {
    fn note(&mut self, e: ResolveError) {
        if self.failure.is_none() {
            self.failure = Some(e);
        }
    }

    fn no_matching(&self, st: &State<'_>, id: &ModId) -> ResolveError {
        let mut avail: Vec<Version> = self
            .by_id
            .get(id)
            .map(|v| v.iter().map(|i| i.manifest.version).collect())
            .unwrap_or_default();
        avail.sort();
        ResolveError::NoMatchingVersion {
            id: id.clone(),
            constraints: st.constraints_text(id),
            available: avail
                .iter()
                .map(Version::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        }
    }

    fn conflict(&self, st: &State<'_>, cand: &Installed) -> Option<ResolveError> {
        let cm = &cand.manifest;
        for (oid, other) in &st.chosen {
            let om = &other.manifest;
            let forward = cm.conflicts.get(oid).filter(|r| r.matches(&om.version));
            let backward = om.conflicts.get(&cm.id).filter(|r| r.matches(&cm.version));
            let (a, b, req) = match (forward, backward) {
                (Some(r), _) => (cm, om, r),
                (None, Some(r)) => (om, cm, r),
                (None, None) => continue,
            };
            let chain = |m: &Manifest| st.chain_to(&m.id, format!("{} {}", m.id, m.version));
            return Some(ResolveError::Conflict(Box::new(ConflictError {
                a: a.id.clone(),
                a_version: a.version,
                b: b.id.clone(),
                b_version: b.version,
                req: req.clone(),
                a_chain: chain(a),
                b_chain: chain(b),
            })));
        }
        None
    }

    fn go(&mut self, st: State<'a>) -> Option<State<'a>> {
        let Some(id) = st
            .reqs
            .keys()
            .find(|k| !st.chosen.contains_key(*k))
            .cloned()
        else {
            return Some(st);
        };
        let constraints = st.reqs[&id].clone();
        let cands: Vec<&'a Installed> = self.by_id.get(&id).cloned().unwrap_or_default();
        if cands.is_empty() {
            let e = ResolveError::Missing {
                id: id.clone(),
                requirers: st.constraints_text(&id),
                chain: st.chain_to(&id, id.to_string()),
            };
            self.note(e);
            return None;
        }
        let mut api_excluded: Vec<&'a Installed> = Vec::new();
        let mut matched = false;
        for cand in cands {
            let v = cand.manifest.version;
            if !constraints.iter().all(|(_, r)| r.matches(&v)) {
                continue;
            }
            if !cand.manifest.api.matches(self.api) {
                api_excluded.push(cand);
                continue;
            }
            matched = true;
            if let Some(e) = self.conflict(&st, cand) {
                self.note(e);
                continue;
            }
            self.steps += 1;
            if self.steps > STEP_LIMIT {
                self.exhausted = true;
                self.note(ResolveError::SearchLimit { limit: STEP_LIMIT });
                return None;
            }
            let mut next = st.clone();
            next.chosen.insert(id.clone(), cand);
            let src = Source::Mod(id.clone(), v);
            let mut ok = true;
            for (dep, req) in &cand.manifest.deps {
                next.add_req(dep, src.clone(), req.clone());
                if let Some(ch) = next.chosen.get(dep)
                    && !req.matches(&ch.manifest.version)
                {
                    let e = self.no_matching(&next, dep);
                    self.note(e);
                    ok = false;
                    break;
                }
            }
            if !ok {
                continue;
            }
            if let Some(done) = self.go(next) {
                return Some(done);
            }
            if self.exhausted {
                return None;
            }
        }
        if !matched {
            let e = if api_excluded.is_empty() {
                self.no_matching(&st, &id)
            } else {
                ResolveError::ApiMismatch {
                    id: id.clone(),
                    host_api: *self.api,
                    versions: api_excluded
                        .iter()
                        .map(|i| format!("{} wants api {}", i.manifest.version, i.manifest.api))
                        .collect::<Vec<_>>()
                        .join(", "),
                    chain: st.chain_to(&id, id.to_string()),
                }
            };
            self.note(e);
        }
        None
    }
}

/// Resolves the host's picks against the installed mods. See the module documentation for the rules.
pub fn resolve(
    picks: &[Pick],
    installed: &[Installed],
    api: &Version,
) -> Result<Resolution, ResolveError> {
    // Canonical order of the inputs: the result must not depend on slice order.
    let mut inst: Vec<&Installed> = installed.iter().collect();
    inst.sort_by(|a, b| {
        (&a.manifest.id, a.manifest.version).cmp(&(&b.manifest.id, b.manifest.version))
    });
    if let Some(w) = inst.windows(2).find(|w| {
        w[0].manifest.id == w[1].manifest.id && w[0].manifest.version == w[1].manifest.version
    }) {
        return Err(ResolveError::DuplicateInstalled {
            id: w[0].manifest.id.clone(),
            version: w[0].manifest.version,
        });
    }
    let mut by_id: BTreeMap<ModId, Vec<&Installed>> = BTreeMap::new();
    for i in inst.iter().rev() {
        // Highest version first.
        by_id.entry(i.manifest.id.clone()).or_default().push(i);
    }
    let mut sorted_picks: Vec<&Pick> = picks.iter().collect();
    sorted_picks.sort_by_key(|p| (p.id.clone(), p.req.to_string()));

    let mut start = State::default();
    for p in sorted_picks {
        start.add_req(&p.id, Source::Host, p.req.clone());
    }
    let mut solver = Solver {
        by_id,
        api,
        steps: 0,
        exhausted: false,
        failure: None,
    };
    let Some(done) = solver.go(start) else {
        // `go` fails only after noting a reason.
        return Err(solver
            .failure
            .unwrap_or(ResolveError::SearchLimit { limit: STEP_LIMIT }));
    };
    order(&done.chosen)
}

fn order(chosen: &BTreeMap<ModId, &Installed>) -> Result<Resolution, ResolveError> {
    // Edge (from, to): `from` loads before `to`. The text says why, for cycle reports.
    let mut edges: BTreeMap<(ModId, ModId), String> = BTreeMap::new();
    let mut warnings = Vec::new();
    let mut edge = |from: &ModId, to: &ModId, why: String| {
        edges.entry((from.clone(), to.clone())).or_insert(why);
    };
    for (id, ins) in chosen {
        let m = &ins.manifest;
        for dep in m.deps.keys() {
            edge(dep, id, format!("`{id}` needs `{dep}`"));
        }
        for (opt, req) in &m.optional {
            if let Some(o) = chosen.get(opt) {
                if req.matches(&o.manifest.version) {
                    edge(opt, id, format!("`{id}` loads after optional `{opt}`"));
                } else {
                    warnings.push(Warning::OptionalOutOfRange {
                        by: id.clone(),
                        by_version: m.version,
                        optional: opt.clone(),
                        req: req.clone(),
                        found: o.manifest.version,
                    });
                }
            }
        }
        for a in m.order.after.iter().filter(|a| chosen.contains_key(*a)) {
            edge(a, id, format!("`{id}` is ordered after `{a}`"));
        }
        for b in m.order.before.iter().filter(|b| chosen.contains_key(*b)) {
            edge(id, b, format!("`{id}` is ordered before `{b}`"));
        }
    }

    // Kahn's algorithm; the ready set is ordered by id, which is the tie-break.
    let mut indeg: BTreeMap<&ModId, usize> = chosen.keys().map(|k| (k, 0)).collect();
    let mut succ: BTreeMap<&ModId, Vec<&ModId>> = BTreeMap::new();
    for (from, to) in edges.keys() {
        *indeg.entry(to).or_default() += 1;
        succ.entry(from).or_default().push(to);
    }
    let mut ready: BTreeSet<&ModId> = indeg
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(k, _)| *k)
        .collect();
    let mut out: Vec<&ModId> = Vec::new();
    while let Some(next) = ready.pop_first() {
        out.push(next);
        for s in succ.get(next).into_iter().flatten() {
            if let Some(d) = indeg.get_mut(s) {
                *d -= 1;
                if *d == 0 {
                    ready.insert(s);
                }
            }
        }
    }
    if out.len() < chosen.len() {
        return Err(ResolveError::Cycle {
            edges: describe_cycle(chosen, &out, &edges),
        });
    }

    let mods = out
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let ins = chosen[*id];
            ResolvedMod {
                id: (*id).clone(),
                version: ins.manifest.version,
                // `ModSet::new` rejects sets above u16::MAX before this index is used.
                load_idx: i as u16,
                sync: ins.manifest.sync,
                client: ins.manifest.client_policy(),
                parts: ins.manifest.parts.clone(),
                content_hash: ins.content_hash,
            }
        })
        .collect();
    Ok(Resolution {
        set: ModSet::new(mods)?,
        warnings,
    })
}

/// Walks predecessors among the mods Kahn could not place; every one of them has an unplaced predecessor, so the
/// walk must close a cycle.
fn describe_cycle(
    chosen: &BTreeMap<ModId, &Installed>,
    placed: &[&ModId],
    edges: &BTreeMap<(ModId, ModId), String>,
) -> String {
    let rem: BTreeSet<&ModId> = chosen.keys().filter(|k| !placed.contains(k)).collect();
    let Some(&start) = rem.first() else {
        return String::new();
    };
    let mut path: Vec<&ModId> = Vec::new();
    let mut cur = start;
    let cycle = loop {
        if let Some(pos) = path.iter().position(|x| *x == cur) {
            break path[pos..].to_vec();
        }
        path.push(cur);
        match edges
            .keys()
            .filter(|(f, t)| t == cur && rem.contains(f))
            .map(|(f, _)| f)
            .min()
        {
            Some(p) => cur = p,
            None => break path.clone(),
        }
    };
    // `cycle` runs against the edge direction; flip it to read forwards.
    let fwd: Vec<&ModId> = cycle.into_iter().rev().collect();
    let n = fwd.len();
    (0..n)
        .filter_map(|i| edges.get(&(fwd[i].clone(), fwd[(i + 1) % n].clone())))
        .cloned()
        .collect::<Vec<_>>()
        .join("; ")
}
