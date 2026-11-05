//! Size report of the B2 build (`report-b2.json` and `report-b2.txt`).
//!
//! The report reads the manifest and the group table of an output directory and prints three
//! tables: by load group (4.13), by id prefix and by budget article (the table of 4.14), then
//! the cold-start downloads and the coverage of the manifest by the groups ([`super::coverage`]).
//! Shared blobs count once in the total and in every group that holds them. The previous report
//! in the directory gives the change of each group; growth over 10 % is a warning, a total over
//! 600 MB is an error, and so is `boot` over 30 MB and a worst-case cold start over 200 MB or
//! 60 s (acceptance criterion 9). A B2 id that no group holds is a warning.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use asset_format::{ContentHash, GROUP_TABLE_ID, GroupTable, LogicalId, Manifest, PriorityClass};
use serde::{Deserialize, Serialize};

use super::coverage::{self, ColdStart, Coverage};
use super::stubs::{StubEvent, load_log};

pub const REPORT_B2_JSON: &str = "report-b2.json";
pub const REPORT_B2_TXT: &str = "report-b2.txt";
const VERSION: u32 = 1;

/// Total size limit of the build, in bytes (decimal megabytes, as the source sizes of the spec).
// re: a total size over 600 MB is a build error.
pub const TOTAL_LIMIT_BYTES: u64 = 600_000_000;
/// Above this size the owner of the acceptance criterion must agree (section 10 "Бюджет").
pub const TOTAL_ASK_BYTES: u64 = 500_000_000;
/// `boot` must stay under this (acceptance criterion 9).
pub const BOOT_LIMIT_BYTES: u64 = 30_000_000;
/// A group grows by more than this since the last report: warning.
const GROWTH_WARN: f64 = 0.10;
/// A group above this multiple of its estimate needs the agreement of the owner (section 10).
const OVER_ESTIMATE: f64 = 1.3;

/// Id prefixes of the B2 blobs (AB-D11); anything else in the manifest is a B1 id.
pub const B2_PREFIXES: [&str; 7] = ["tx", "md", "an", "fsm", "pc", "fx", "tb"];

/// The prefix of an id (`tx:a.dds` is `tx`); empty when there is none.
pub fn prefix_of(id: &str) -> &str {
    id.split_once(':').map_or("", |(p, _)| p)
}

/// Whether `id` is one of the B2 ids: a B2 prefix, and for `tb:` one of the B2 tables.
pub fn is_b2_id(id: &str) -> bool {
    let p = prefix_of(id);
    B2_PREFIXES.contains(&p) && (p != "tb" || id.ends_with("-b2"))
}

/// Estimate of a group in megabytes: `(estimate, hard limit)`.
// re: the size table of the load groups.
fn group_estimate(id: &str) -> Option<(f64, Option<f64>)> {
    Some(match id {
        "boot" => (25.0, Some(BOOT_LIMIT_BYTES as f64 / 1e6)),
        // Owner decision of the R4 packet (section 10 change log): the textures several continents
        // use are in `style:shared` (required by every style group), so a style group is its pieces
        // and its own textures. Measured at the split: shared 40.6, styles 12.5-16.9, generic 14.4.
        "style:shared" => (41.0, None),
        "style:generic" => (14.5, None),
        "decals" => (10.0, None),
        "water" => (20.0, None),
        "minimap" => (2.5, None),
        "trees" => (2.0, None),
        "roads" => (1.0, None),
        "fx" => (8.0, None),
        // The game data of the client's replica (B4): both databases and the gameplay Lua.
        "gc:replica" => (12.0, None),
        _ if id.starts_with("style:") => (17.0, None),
        _ if id.starts_with("civ:") => (15.0, None),
        _ if id.starts_with("era:") => (40.0, None),
        _ if id.starts_with("obj:") => (1.0, None),
        _ => return None,
    })
}

/// Estimate of a budget article in megabytes.
// re: the first size table.
const ARTICLES: [(&str, f64); 16] = [
    ("terrain-pieces", 20.0),
    ("terrain-tex", 93.0),
    ("terrain-tex@low", 12.5),
    ("decals", 10.0),
    ("boot-light", 5.5),
    ("fow-overlay", 15.0),
    ("minimap", 2.5),
    ("water", 20.0),
    ("settings", 0.5),
    ("units", 84.0),
    ("landmarks", 95.0),
    ("city-bldg-wonder", 45.0),
    ("trees", 2.0),
    ("roads", 1.0),
    ("fx", 8.0),
    ("sv-icons", 15.0),
];

/// The groups of the table an id belongs to, to place it in an article.
struct Placement<'a> {
    in_style: BTreeSet<&'a LogicalId>,
    in_decals: BTreeSet<&'a LogicalId>,
    in_water: BTreeSet<&'a LogicalId>,
    /// Models, clips, state machines and textures of the world objects, by article.
    owners: &'a BTreeMap<LogicalId, &'static str>,
}

impl<'a> Placement<'a> {
    fn of(table: &'a GroupTable, owners: &'a BTreeMap<LogicalId, &'static str>) -> Self {
        let mut p = Placement {
            in_style: BTreeSet::new(),
            in_decals: BTreeSet::new(),
            in_water: BTreeSet::new(),
            owners,
        };
        for g in &table.groups {
            let set = if g.id.starts_with("style:") {
                &mut p.in_style
            } else if g.id == "decals" {
                &mut p.in_decals
            } else if g.id == "water" {
                &mut p.in_water
            } else {
                continue;
            };
            set.extend(g.members.iter());
        }
        p
    }

    /// The budget article of an id; `None` for an id no article covers (a B1 id, a model set whose
    /// card has not been built yet).
    fn article(&self, id: &LogicalId) -> Option<&'static str> {
        let s = id.as_str();
        match prefix_of(s) {
            "pc" => return Some("terrain-pieces"),
            "tb" => return Some("settings"),
            "fx" => return Some("fx"),
            "tx" => {}
            // Models, clip packs and state machines: the object that shows them.
            _ => return self.owners.get(id).copied(),
        }
        let name = &s[3..];
        if name.ends_with("@low") {
            return Some("terrain-tex@low");
        }
        if name.starts_with("minimap") {
            return Some("minimap");
        }
        if self.in_style.contains(id) {
            return Some("terrain-tex");
        }
        if self.in_water.contains(id) && !self.in_decals.contains(id) {
            return Some("water");
        }
        if self.in_decals.contains(id) {
            return Some("decals");
        }
        const LIGHT: [&str; 5] = [
            "sunlight_",
            "shadowlight_",
            "unit_",
            "cubelight_",
            "specscreenmap",
        ];
        if LIGHT.iter().any(|p| name.starts_with(p))
            || name == "fogramp.dds"
            || name.ends_with("_colorkey.tga")
        {
            return Some("boot-light");
        }
        if let Some(a) = self.owners.get(id) {
            return Some(a);
        }
        // Fog clouds, hex grid, contours, highlights, selection ring, arrows.
        Some("fow-overlay")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupRow {
    pub id: String,
    pub class: String,
    pub ids: usize,
    pub bytes: u64,
    /// Share of the 600 MB limit, in percent.
    pub share_pct: f64,
    pub estimate_mb: Option<f64>,
    pub limit_mb: Option<f64>,
    /// Bytes in the previous report and the change since, in percent.
    pub previous_bytes: Option<u64>,
    pub delta_pct: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrefixRow {
    pub prefix: String,
    pub ids: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArticleRow {
    pub article: String,
    pub ids: usize,
    pub bytes: u64,
    pub estimate_mb: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetReport {
    pub version: u32,
    /// Distinct blobs of the manifest, each once (what the server stores).
    pub total_bytes: u64,
    /// What one device downloads, every blob once: the variant its profile picks for each id
    /// (BC device, ASTC device, neither).
    #[serde(default)]
    pub devices: BTreeMap<String, u64>,
    pub total_ids: usize,
    pub limit_bytes: u64,
    pub groups: Vec<GroupRow>,
    pub prefixes: Vec<PrefixRow>,
    pub articles: Vec<ArticleRow>,
    /// Ids of the manifest that no article covers (B1 ids, models that are not built yet).
    pub unplaced_ids: usize,
    /// What the groups hold of the manifest (`None`: no group table).
    #[serde(default)]
    pub coverage: Option<Coverage>,
    /// Cold-start downloads (4.13); empty when the table has no `Map` group.
    #[serde(default)]
    pub cold_start: Vec<ColdStart>,
    pub stubs: Vec<StubEvent>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    #[error("{0}")]
    Read(String),
}

fn read_err(what: &str, e: impl std::fmt::Display) -> ReportError {
    ReportError::Read(format!("{what}: {e}"))
}

/// The manifest `out/manifest` points at. A directory without one is an error here: there is
/// nothing to report or verify.
pub fn read_manifest(out: &Path) -> Result<Manifest, ReportError> {
    let pointer = out.join("manifest");
    let text = std::fs::read_to_string(&pointer)
        .map_err(|e| read_err(&format!("reading {}", pointer.display()), e))?;
    let hash: ContentHash = text
        .trim()
        .parse()
        .map_err(|e| read_err(&format!("{} is not a hash", pointer.display()), e))?;
    let path = out.join(hash.to_string());
    let bytes =
        std::fs::read(&path).map_err(|e| read_err(&format!("reading {}", path.display()), e))?;
    Manifest::decode(&bytes).map_err(|e| read_err(&format!("manifest {}", path.display()), e))
}

/// The group table of the manifest; `None` when the manifest has none.
pub fn read_groups(out: &Path, manifest: &Manifest) -> Result<Option<GroupTable>, ReportError> {
    let Some(v) = manifest.variants(&LogicalId::new(GROUP_TABLE_ID)).first() else {
        return Ok(None);
    };
    let path = out.join(v.hash.to_string());
    let bytes =
        std::fs::read(&path).map_err(|e| read_err(&format!("reading {}", path.display()), e))?;
    GroupTable::decode(&bytes)
        .map(Some)
        .map_err(|e| read_err(&format!("group table {}", path.display()), e))
}

/// The report of the previous run in `out`; `None` if there is none or it cannot be read.
pub fn load_previous(out: &Path) -> Option<BudgetReport> {
    let bytes = std::fs::read(out.join(REPORT_B2_JSON)).ok()?;
    serde_json::from_slice::<BudgetReport>(&bytes)
        .ok()
        .filter(|r| r.version == VERSION)
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1e6
}

/// What each device downloads of the whole manifest, every blob once.
fn device_bytes(manifest: &Manifest) -> BTreeMap<String, u64> {
    let ids: Vec<&LogicalId> = manifest.iter().map(|(id, _)| id).collect();
    crate::textures::variants::profile_bytes(manifest, &ids)
}

/// Largest variant of an id, as in the group table.
fn size_of(manifest: &Manifest, id: &LogicalId) -> u64 {
    manifest
        .variants(id)
        .iter()
        .map(|v| v.size)
        .max()
        .unwrap_or(0)
}

/// Builds the report of `manifest` and `table` (`None`: no group table). `previous` gives the change
/// of each group.
pub fn build(
    manifest: &Manifest,
    table: Option<&GroupTable>,
    previous: Option<&BudgetReport>,
    stubs: Vec<StubEvent>,
) -> BudgetReport {
    build_owned(manifest, table, previous, stubs, &BTreeMap::new())
}

/// [`build`] with the articles of the ids the world objects own ([`object_groups::owners`]).
pub fn build_owned(
    manifest: &Manifest,
    table: Option<&GroupTable>,
    previous: Option<&BudgetReport>,
    stubs: Vec<StubEvent>,
    owners: &BTreeMap<LogicalId, &'static str>,
) -> BudgetReport {
    let mut warnings = Vec::new();
    let mut errors = Vec::new();

    // Total: every distinct blob once.
    let mut blobs: BTreeMap<ContentHash, u64> = BTreeMap::new();
    for (_, variants) in manifest.iter() {
        for v in variants {
            blobs.insert(v.hash, v.size);
        }
    }
    let total_bytes: u64 = blobs.values().sum();
    // A device downloads one variant of a texture (the
    // server holds more, the player does not), so the limit is on the largest device's share.
    let devices = device_bytes(manifest);
    let worst = devices.values().copied().max().unwrap_or(total_bytes);
    if worst > TOTAL_LIMIT_BYTES {
        errors.push(format!(
            "the build is {:.1} MB for one device, over the {:.0} MB limit",
            mb(worst),
            mb(TOTAL_LIMIT_BYTES)
        ));
    } else if worst > TOTAL_ASK_BYTES {
        warnings.push(format!(
            "the build is {:.1} MB for one device, over {:.0} MB: the owner of the criterion must agree (section 10)",
            mb(worst),
            mb(TOTAL_ASK_BYTES)
        ));
    }

    // Groups.
    let mut groups = Vec::new();
    if let Some(t) = table {
        let before: BTreeMap<&str, u64> = previous
            .map(|p| p.groups.iter().map(|g| (g.id.as_str(), g.bytes)).collect())
            .unwrap_or_default();
        for g in &t.groups {
            let bytes: u64 = g.members.iter().map(|m| size_of(manifest, m)).sum();
            if bytes != g.bytes {
                warnings.push(format!(
                    "group {}: the table says {} B, the manifest sums to {bytes} B",
                    g.id, g.bytes
                ));
            }
            let est = group_estimate(&g.id);
            if let Some((estimate, limit)) = est {
                if let Some(limit) = limit
                    && mb(bytes) > limit
                {
                    errors.push(format!(
                        "group {} is {:.2} MB, over its limit of {limit:.0} MB",
                        g.id,
                        mb(bytes)
                    ));
                }
                if mb(bytes) > estimate * OVER_ESTIMATE {
                    warnings.push(format!(
                        "group {} is {:.2} MB, over {OVER_ESTIMATE}x its estimate of {estimate} MB: the owner must agree (section 10)",
                        g.id,
                        mb(bytes)
                    ));
                }
            }
            let previous_bytes = before.get(g.id.as_str()).copied();
            let delta_pct = previous_bytes
                .filter(|p| *p > 0)
                .map(|p| (bytes as f64 - p as f64) / p as f64 * 100.0);
            if let Some(d) = delta_pct
                && d > GROWTH_WARN * 100.0
            {
                warnings.push(format!(
                    "group {} grew {d:.1} % since the last report ({} B to {bytes} B)",
                    g.id,
                    previous_bytes.unwrap_or(0)
                ));
            }
            groups.push(GroupRow {
                id: g.id.clone(),
                class: format!("{:?}", g.class),
                ids: g.members.len(),
                bytes,
                share_pct: bytes as f64 / TOTAL_LIMIT_BYTES as f64 * 100.0,
                estimate_mb: est.map(|e| e.0),
                limit_mb: est.and_then(|e| e.1),
                previous_bytes,
                delta_pct,
            });
        }
        // `boot` is the class the loading screen waits for.
        if !t.groups.iter().any(|g| g.class == PriorityClass::Boot) {
            warnings.push("the group table has no group of the Boot class".to_owned());
        }
    }

    // Prefixes.
    let mut by_prefix: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    for (id, _) in manifest.iter() {
        let p = prefix_of(id.as_str());
        let key = if p.is_empty() { "(none)" } else { p };
        let e = by_prefix.entry(key.to_owned()).or_default();
        e.0 += 1;
        e.1 += size_of(manifest, id);
    }
    let prefixes = by_prefix
        .into_iter()
        .map(|(prefix, (ids, bytes))| PrefixRow { prefix, ids, bytes })
        .collect();

    // Articles.
    let empty = GroupTable::new();
    let placement = Placement::of(table.unwrap_or(&empty), owners);
    let mut by_article: BTreeMap<&str, (usize, u64)> = BTreeMap::new();
    let mut unplaced_ids = 0;
    for (id, _) in manifest.iter() {
        match placement.article(id) {
            Some(a) => {
                let e = by_article.entry(a).or_default();
                e.0 += 1;
                e.1 += size_of(manifest, id);
            }
            None => unplaced_ids += 1,
        }
    }
    let articles = ARTICLES
        .iter()
        .map(|(name, estimate)| {
            let (ids, bytes) = by_article.get(name).copied().unwrap_or_default();
            ArticleRow {
                article: (*name).to_owned(),
                ids,
                bytes,
                estimate_mb: *estimate,
            }
        })
        .collect();

    // Coverage and cold start (4.13).
    let coverage = table.map(|t| coverage::coverage(manifest, t));
    if let Some(c) = &coverage {
        if !c.ungrouped.is_empty() {
            warnings.push(format!(
                "{} B2 id(s) are in no group, so the client never asks for them: {}{}",
                c.ungrouped.len(),
                c.ungrouped
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
                if c.ungrouped.len() > 3 { ", ..." } else { "" }
            ));
        }
        for g in &c.empty_groups {
            warnings.push(format!("group {g} has no members"));
        }
    }
    let cold_start = table.map_or_else(Vec::new, |t| coverage::cold_start(manifest, t));
    if let Some(worst) = cold_start.first() {
        if worst.bytes > coverage::COLD_LIMIT_BYTES {
            errors.push(format!(
                "cold start (boot + every Map group) is {:.1} MB, over the {:.0} MB limit",
                mb(worst.bytes),
                mb(coverage::COLD_LIMIT_BYTES)
            ));
        }
        if worst.seconds > coverage::COLD_LIMIT_SECONDS {
            errors.push(format!(
                "cold start (boot + every Map group) takes {:.0} s at 100 Mbit/s, over {:.0} s",
                worst.seconds,
                coverage::COLD_LIMIT_SECONDS
            ));
        }
    }

    if !stubs.is_empty() {
        warnings.push(format!("{} stub event(s) in the log", stubs.len()));
    }
    BudgetReport {
        version: VERSION,
        total_bytes,
        devices,
        total_ids: manifest.len(),
        limit_bytes: TOTAL_LIMIT_BYTES,
        groups,
        prefixes,
        articles,
        unplaced_ids,
        coverage,
        cold_start,
        stubs,
        warnings,
        errors,
    }
}

impl BudgetReport {
    /// The text form: the three tables, the stubs, the warnings and errors.
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "B2 build: {} ids, {:.2} MB of {:.0} MB ({:.1} %)\n",
            self.total_ids,
            mb(self.total_bytes),
            mb(self.limit_bytes),
            self.total_bytes as f64 / self.limit_bytes as f64 * 100.0
        );
        if !self.devices.is_empty() {
            let _ = writeln!(s, "== per device (the variant each one downloads) ==");
            for (d, b) in &self.devices {
                let _ = writeln!(s, "{d:<18} {:>10.2} MB", mb(*b));
            }
            let _ = writeln!(s);
        }
        let _ = writeln!(s, "== by load group ==");
        let _ = writeln!(
            s,
            "{:<18} {:<5} {:>6} {:>10} {:>7} {:>9} {:>9}",
            "group", "class", "ids", "MB", "% of 600", "estimate", "change"
        );
        for g in &self.groups {
            let est = g.estimate_mb.map_or("-".to_owned(), |e| format!("{e:.1}"));
            let delta = g.delta_pct.map_or("-".to_owned(), |d| format!("{d:+.1} %"));
            let _ = writeln!(
                s,
                "{:<18} {:<5} {:>6} {:>10.3} {:>7.2} {:>9} {:>9}",
                g.id,
                g.class,
                g.ids,
                mb(g.bytes),
                g.share_pct,
                est,
                delta
            );
        }
        let _ = writeln!(s, "\n== by id prefix ==");
        for p in &self.prefixes {
            let _ = writeln!(
                s,
                "{:<8} {:>6} ids {:>10.3} MB",
                p.prefix,
                p.ids,
                mb(p.bytes)
            );
        }
        let _ = writeln!(s, "\n== by article (4.14) ==");
        let _ = writeln!(
            s,
            "{:<18} {:>6} {:>10} {:>9}",
            "article", "ids", "MB", "estimate"
        );
        for a in &self.articles {
            let note = if a.ids == 0 {
                "  (not built)"
            } else if mb(a.bytes) > a.estimate_mb * OVER_ESTIMATE {
                "  (over 1.3x the estimate)"
            } else if mb(a.bytes) < a.estimate_mb * 0.7 {
                "  (under 0.7x the estimate)"
            } else {
                ""
            };
            let _ = writeln!(
                s,
                "{:<18} {:>6} {:>10.3} {:>9.1}{note}",
                a.article,
                a.ids,
                mb(a.bytes),
                a.estimate_mb
            );
        }
        if self.unplaced_ids > 0 {
            let _ = writeln!(s, "({} id(s) are in no article)", self.unplaced_ids);
        }
        if !self.cold_start.is_empty() {
            let _ = writeln!(
                s,
                "\n== cold start (4.13; every blob once, 100 Mbit/s; limits: worst case {:.0} MB and {:.0} s) ==",
                mb(coverage::COLD_LIMIT_BYTES),
                coverage::COLD_LIMIT_SECONDS
            );
            let _ = writeln!(
                s,
                "{:<46} {:>6} {:>6} {:>10} {:>8}",
                "case", "groups", "blobs", "MB", "seconds"
            );
            for c in &self.cold_start {
                let _ = writeln!(
                    s,
                    "{:<46} {:>6} {:>6} {:>10.3} {:>8.1}",
                    c.case,
                    c.groups.len(),
                    c.blobs,
                    mb(c.bytes),
                    c.seconds
                );
            }
        }
        if let Some(c) = &self.coverage {
            let _ = writeln!(s, "\n== coverage (ids held by a group) ==");
            s.push_str(&c.to_text());
        }
        let _ = writeln!(s, "\n== stubs ==");
        if self.stubs.is_empty() {
            let _ = writeln!(s, "none");
        }
        for e in &self.stubs {
            let _ = writeln!(s, "{}: {}", e.id, e.reason);
        }
        for (title, list) in [("warnings", &self.warnings), ("errors", &self.errors)] {
            let _ = writeln!(s, "\n== {title} ==");
            if list.is_empty() {
                let _ = writeln!(s, "none");
            }
            for w in list {
                let _ = writeln!(s, "{w}");
            }
        }
        s
    }

    pub fn write(&self, out: &Path) -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(out.join(REPORT_B2_JSON), json)?;
        std::fs::write(out.join(REPORT_B2_TXT), self.to_text())
    }
}

/// Reads `out`, builds the report against the previous one and writes both files. The report is
/// written even when it has errors, so it shows what is wrong.
pub fn run(out: &Path) -> Result<BudgetReport, ReportError> {
    let manifest = read_manifest(out)?;
    let table = read_groups(out, &manifest)?;
    let previous = load_previous(out);
    let stubs = load_log(out).map_err(|e| ReportError::Read(e.to_string()))?;
    // The articles of the models need the art table; without it they stay unplaced.
    let owners = crate::world::object_groups::owners(out, &manifest).unwrap_or_default();
    let report = build_owned(&manifest, table.as_ref(), previous.as_ref(), stubs, &owners);
    report
        .write(out)
        .map_err(|e| read_err(&format!("writing the report into {}", out.display()), e))?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use asset_format::{Group, PrefetchGroup, Trigger, Variant, VariantFormat};

    use super::*;

    fn put(m: &mut Manifest, id: &str, n: u8, size: u64) {
        m.insert(
            LogicalId::new(id),
            Variant {
                format: VariantFormat::Generic,
                hash: ContentHash([n; 32]),
                size,
                prefetch: PrefetchGroup(0),
            },
        );
    }

    fn group(id: &str, class: PriorityClass, members: &[&str], bytes: u64) -> Group {
        Group {
            id: id.into(),
            class,
            trigger: Trigger::Always,
            members: members.iter().map(|m| LogicalId::new(*m)).collect(),
            bytes,
            requires: vec![],
        }
    }

    fn sample() -> (Manifest, GroupTable) {
        let mut m = Manifest::new();
        put(&mut m, "tx:sunlight_europe_terrain.dds", 1, 1_000_000);
        put(&mut m, "tx:hexgrid.dds", 2, 2_000_000);
        put(&mut m, "tx:grass_d.dds", 3, 5_000_000);
        put(&mut m, "tx:grass_d.dds@low", 4, 500_000);
        put(&mut m, "tx:floodplains_d.dds", 5, 700_000);
        put(&mut m, "pc:europe", 6, 3_000_000);
        put(&mut m, "tb:world-b2", 7, 100_000);
        put(&mut m, "tex:old_b1", 8, 10);
        let mut t = GroupTable::new();
        t.upsert(group(
            "boot",
            PriorityClass::Boot,
            &[
                "tx:sunlight_europe_terrain.dds",
                "tx:hexgrid.dds",
                "tb:world-b2",
            ],
            3_100_000,
        ));
        t.upsert(group(
            "style:europe",
            PriorityClass::Map,
            &["pc:europe", "tx:grass_d.dds", "tx:grass_d.dds@low"],
            8_500_000,
        ));
        t.upsert(group(
            "decals",
            PriorityClass::Map,
            &["tx:floodplains_d.dds"],
            700_000,
        ));
        (m, t)
    }

    #[test]
    fn tables_count_shared_blobs_once_and_place_ids_in_articles() {
        let (m, t) = sample();
        let r = build(&m, Some(&t), None, vec![]);
        assert_eq!(r.total_ids, 8);
        assert_eq!(
            r.total_bytes,
            1_000_000 + 2_000_000 + 5_000_000 + 500_000 + 700_000 + 3_000_000 + 100_000 + 10
        );
        assert!(
            r.errors.is_empty() && r.warnings.is_empty(),
            "{:?} {:?}",
            r.errors,
            r.warnings
        );
        let art = |n: &str| r.articles.iter().find(|a| a.article == n).unwrap();
        assert_eq!(art("boot-light").bytes, 1_000_000);
        assert_eq!(art("fow-overlay").bytes, 2_000_000);
        assert_eq!(art("terrain-tex").bytes, 5_000_000);
        assert_eq!(art("terrain-tex@low").bytes, 500_000);
        assert_eq!(art("decals").bytes, 700_000);
        assert_eq!(art("terrain-pieces").bytes, 3_000_000);
        assert_eq!(art("settings").bytes, 100_000);
        assert_eq!(art("water").ids, 0);
        assert_eq!(r.unplaced_ids, 1, "the B1 id");
        let g = r.groups.iter().find(|g| g.id == "boot").unwrap();
        assert_eq!((g.ids, g.bytes), (3, 3_100_000));
        assert!((g.share_pct - 0.5166).abs() < 0.01);
        assert!(r.to_text().contains("== by article"));
    }

    #[test]
    fn models_clips_and_their_textures_go_to_the_article_of_the_object_that_owns_them() {
        let (mut m, t) = sample();
        put(&mut m, "md:archer", 40, 1_000);
        put(&mut m, "an:archer", 41, 500);
        put(&mut m, "tx:archer_diff.dds", 42, 10_000);
        put(&mut m, "md:farm", 43, 2_000);
        put(&mut m, "md:fxm_damage", 44, 300);
        put(&mut m, "tx:forest_tile1.dds", 45, 70);
        let owners: BTreeMap<LogicalId, &'static str> = [
            ("md:archer", "units"),
            ("an:archer", "units"),
            ("tx:archer_diff.dds", "units"),
            ("md:farm", "landmarks"),
            ("tx:forest_tile1.dds", "trees"),
        ]
        .iter()
        .map(|(i, a)| (LogicalId::new(*i), *a))
        .collect();
        let r = build_owned(&m, Some(&t), None, vec![], &owners);
        let art = |n: &str| r.articles.iter().find(|a| a.article == n).unwrap();
        assert_eq!((art("units").ids, art("units").bytes), (3, 11_500));
        assert_eq!(art("landmarks").bytes, 2_000);
        assert_eq!(art("trees").bytes, 70);
        // A model nobody owns is in no article; without owners nothing changes.
        assert_eq!(r.unplaced_ids, 2, "the B1 id and the model nobody owns");
        let plain = build(&m, Some(&t), None, vec![]);
        assert_eq!(
            plain
                .articles
                .iter()
                .find(|a| a.article == "units")
                .unwrap()
                .ids,
            0
        );
    }

    #[test]
    fn the_shared_style_group_has_its_own_estimate_and_its_textures_are_terrain_textures() {
        assert_eq!(group_estimate("style:shared"), Some((41.0, None)));
        assert_eq!(group_estimate("style:europe").map(|e| e.0), Some(17.0));
        assert_eq!(group_estimate("style:generic").map(|e| e.0), Some(14.5));

        let (mut m, mut t) = sample();
        put(&mut m, "tx:rock_d.dds", 30, 4_000_000);
        t.upsert(group(
            "style:shared",
            PriorityClass::Map,
            &["tx:rock_d.dds"],
            4_000_000,
        ));
        let r = build(&m, Some(&t), None, vec![]);
        let art = |n: &str| r.articles.iter().find(|a| a.article == n).unwrap();
        assert_eq!(art("terrain-tex").bytes, 5_000_000 + 4_000_000);
        let g = r.groups.iter().find(|g| g.id == "style:shared").unwrap();
        assert_eq!((g.ids, g.bytes, g.estimate_mb), (1, 4_000_000, Some(41.0)));
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    #[test]
    fn total_boot_and_group_limits_are_errors_and_growth_a_warning() {
        let (mut m, t) = sample();
        let r0 = build(&m, Some(&t), None, vec![]);
        // `boot` grows from 3.1 to 35 MB (over its 30 MB limit) and over 1.3x its estimate.
        put(&mut m, "tx:hexgrid.dds", 20, 33_900_000);
        let r = build(&m, Some(&t), Some(&r0), vec![]);
        assert!(
            r.errors
                .iter()
                .any(|e| e.contains("group boot") && e.contains("limit")),
            "{:?}",
            r.errors
        );
        assert!(
            r.warnings.iter().any(|w| w.contains("grew")),
            "{:?}",
            r.warnings
        );
        assert!(
            r.warnings.iter().any(|w| w.contains("1.3x")),
            "{:?}",
            r.warnings
        );

        // 700 MB of blobs is over the total limit.
        let mut big = Manifest::new();
        for i in 0..7u8 {
            put(&mut big, &format!("tx:big{i}.dds"), i, 100_000_000);
        }
        let r = build(&big, None, None, vec![]);
        assert!(
            r.errors.iter().any(|e| e.contains("600 MB")),
            "{:?}",
            r.errors
        );
        // 550 MB is a warning only.
        let mut mid = Manifest::new();
        for i in 0..11u8 {
            put(&mut mid, &format!("tx:big{i}.dds"), i, 50_000_000);
        }
        let r = build(&mid, None, None, vec![]);
        assert!(r.errors.is_empty());
        assert!(r.warnings.iter().any(|w| w.contains("500 MB")));
    }

    #[test]
    fn a_stale_group_sum_and_stubs_are_reported() {
        let (m, mut t) = sample();
        t.groups[0].bytes += 1;
        let r = build(
            &m,
            Some(&t),
            None,
            vec![StubEvent {
                id: "tx:x".into(),
                reason: "r".into(),
            }],
        );
        assert!(r.warnings.iter().any(|w| w.contains("the table says")));
        assert!(r.warnings.iter().any(|w| w.contains("stub event")));
        assert!(r.to_text().contains("tx:x: r"));
    }

    #[test]
    fn b2_ids_by_prefix() {
        assert!(is_b2_id("tx:a.dds") && is_b2_id("pc:europe") && is_b2_id("tb:world-b2"));
        assert!(!is_b2_id("tex:a") && !is_b2_id("table:art-b1") && !is_b2_id("tb:art-b1"));
        assert_eq!(prefix_of("tx:a"), "tx");
        assert_eq!(prefix_of("plain"), "");
    }
}
