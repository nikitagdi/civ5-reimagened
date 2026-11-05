//! Coverage of the manifest by the load groups and the cold start.
//!
//! The client asks for groups, not for single ids, so a B2 id that no group holds is never
//! fetched: it is listed here. The cold start is what a client downloads before the map is
//! shown: `boot` and the `Map` groups with what they require, every blob once, on the normal
//! preset (the `@low` variants left out) and on Low (a terrain texture replaced by its `@low`).

use std::collections::{BTreeMap, BTreeSet};

use asset_format::{
    BOOT_GROUP, ContentHash, GROUP_TABLE_ID, GroupTable, LogicalId, Manifest, PriorityClass,
};
use serde::{Deserialize, Serialize};

use super::budget::{is_b2_id, prefix_of};

/// A client downloads this fast: 100 Mbit/s of clean channel, in bytes per second.
pub const BYTES_PER_SECOND: f64 = 100e6 / 8.0;
/// Cold start limit of the worst case, in bytes (acceptance criterion 9).
pub const COLD_LIMIT_BYTES: u64 = 200_000_000;
/// Cold start limit of the worst case, in seconds (AC-R8.4).
pub const COLD_LIMIT_SECONDS: f64 = 60.0;
/// Ungrouped ids listed in the text report.
const LIST_LIMIT: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrefixCoverage {
    pub prefix: String,
    pub ids: usize,
    pub grouped: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    /// B2 ids of the manifest.
    pub b2_ids: usize,
    /// Of them, members of at least one group.
    pub grouped: usize,
    pub by_prefix: Vec<PrefixCoverage>,
    /// B2 ids in no group, sorted.
    pub ungrouped: Vec<String>,
    /// Groups without members.
    pub empty_groups: Vec<String>,
}

/// Which B2 ids of `manifest` some group of `table` holds. The group table itself counts as
/// covered: the client fetches it first through its prefetch flag.
pub fn coverage(manifest: &Manifest, table: &GroupTable) -> Coverage {
    let held: BTreeSet<&LogicalId> = table.groups.iter().flat_map(|g| &g.members).collect();
    let mut by_prefix: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let mut c = Coverage::default();
    for (id, _) in manifest.iter() {
        let s = id.as_str();
        if !is_b2_id(s) {
            continue;
        }
        let covered = s == GROUP_TABLE_ID || held.contains(id);
        let e = by_prefix.entry(prefix_of(s)).or_default();
        e.0 += 1;
        c.b2_ids += 1;
        if covered {
            e.1 += 1;
            c.grouped += 1;
        } else {
            c.ungrouped.push(s.to_owned());
        }
    }
    c.by_prefix = by_prefix
        .into_iter()
        .map(|(p, (ids, grouped))| PrefixCoverage {
            prefix: p.to_owned(),
            ids,
            grouped,
        })
        .collect();
    c.empty_groups = table
        .groups
        .iter()
        .filter(|g| g.members.is_empty())
        .map(|g| g.id.clone())
        .collect();
    c
}

impl Coverage {
    pub fn to_text(&self) -> String {
        use std::fmt::Write as _;
        let mut s = String::new();
        let _ = writeln!(
            s,
            "{} of {} B2 ids are in a group",
            self.grouped, self.b2_ids
        );
        for p in &self.by_prefix {
            let _ = writeln!(
                s,
                "{:<8} {:>6} ids {:>6} in a group",
                p.prefix, p.ids, p.grouped
            );
        }
        for g in &self.empty_groups {
            let _ = writeln!(s, "empty group: {g}");
        }
        if !self.ungrouped.is_empty() {
            let _ = writeln!(s, "in no group ({}):", self.ungrouped.len());
            for id in self.ungrouped.iter().take(LIST_LIMIT) {
                let _ = writeln!(s, "  {id}");
            }
            if self.ungrouped.len() > LIST_LIMIT {
                let _ = writeln!(s, "  ... {} more", self.ungrouped.len() - LIST_LIMIT);
            }
        }
        s
    }
}

/// One cold-start case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColdStart {
    pub case: String,
    /// The groups it loads, with what they require.
    pub groups: Vec<String>,
    /// Distinct blobs.
    pub blobs: usize,
    pub bytes: u64,
    /// At 100 Mbit/s.
    pub seconds: f64,
}

/// The groups `roots` need: themselves and what they require, transitively.
fn with_requires<'a>(table: &'a GroupTable, roots: &[&'a str]) -> BTreeSet<&'a str> {
    let mut out = BTreeSet::new();
    let mut todo: Vec<&str> = roots.to_vec();
    while let Some(id) = todo.pop() {
        if let Some(g) = table.get(id)
            && out.insert(g.id.as_str())
        {
            todo.extend(g.requires.iter().map(String::as_str));
        }
    }
    out
}

/// What a client of the given preset fetches of `ids`: the normal one never the `@low` variants,
/// Low a texture's `@low` in place of the texture where the manifest has it.
fn for_preset(manifest: &Manifest, ids: BTreeSet<&LogicalId>, low: bool) -> Vec<LogicalId> {
    let mut out = Vec::new();
    for id in ids {
        let s = id.as_str();
        if s.ends_with("@low") {
            if low {
                out.push(id.clone());
            }
            continue;
        }
        let variant = LogicalId::new(format!("{s}@low"));
        if low && !manifest.variants(&variant).is_empty() {
            out.push(variant);
        } else {
            out.push(id.clone());
        }
    }
    out
}

fn case(
    name: &str,
    manifest: &Manifest,
    table: &GroupTable,
    roots: &[&str],
    low: bool,
) -> ColdStart {
    let groups = with_requires(table, roots);
    let ids: BTreeSet<&LogicalId> = groups
        .iter()
        .filter_map(|g| table.get(g))
        .flat_map(|g| &g.members)
        .collect();
    let mut blobs: BTreeMap<ContentHash, u64> = BTreeMap::new();
    for id in for_preset(manifest, ids, low) {
        if let Some(v) = manifest.variants(&id).iter().max_by_key(|v| v.size) {
            blobs.insert(v.hash, v.size);
        }
    }
    let bytes: u64 = blobs.values().sum();
    ColdStart {
        case: name.to_owned(),
        groups: groups.into_iter().map(str::to_owned).collect(),
        blobs: blobs.len(),
        bytes,
        seconds: bytes as f64 / BYTES_PER_SECOND,
    }
}

/// The cold-start cases of `table`, empty when it has no `boot` and no `Map` group:
/// - worst: `boot` and every `Map` group (all styles, generic, water, decals, ...);
/// - typical: `boot`, every `Map` group that is not a continent style, and the two largest
///   continent styles (the shared texture group comes with them);
/// - the same two on Low.
///
/// Blobs count once, so a texture in two groups is one download.
pub fn cold_start(manifest: &Manifest, table: &GroupTable) -> Vec<ColdStart> {
    let map: Vec<&str> = table
        .groups
        .iter()
        .filter(|g| g.class == PriorityClass::Map)
        .map(|g| g.id.as_str())
        .collect();
    if map.is_empty() {
        return Vec::new();
    }
    const CONTINENTS: [&str; 4] = [
        "style:africa",
        "style:america",
        "style:asia",
        "style:europe",
    ];
    let mut continents: Vec<&str> = map
        .iter()
        .copied()
        .filter(|g| CONTINENTS.contains(g))
        .collect();
    // The size of a style is what its own members weigh (not the shared group it requires).
    let own_bytes = |id: &str| -> u64 {
        table.get(id).map_or(0, |g| {
            g.members
                .iter()
                .filter(|m| !m.as_str().ends_with("@low"))
                .flat_map(|m| manifest.variants(m).iter().map(|v| v.size).max())
                .sum()
        })
    };
    continents.sort_by_key(|g| std::cmp::Reverse(own_bytes(g)));
    continents.truncate(2);
    let mut worst: Vec<&str> = vec![BOOT_GROUP];
    worst.extend(&map);
    let mut typical: Vec<&str> = vec![BOOT_GROUP];
    typical.extend(map.iter().copied().filter(|g| !CONTINENTS.contains(g)));
    typical.extend(&continents);
    vec![
        case(
            "worst: boot + every Map group",
            manifest,
            table,
            &worst,
            false,
        ),
        case(
            "typical: boot + Map + the 2 largest styles",
            manifest,
            table,
            &typical,
            false,
        ),
        case("worst on Low (@low terrain)", manifest, table, &worst, true),
        case(
            "typical on Low (@low terrain)",
            manifest,
            table,
            &typical,
            true,
        ),
    ]
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

    fn group(id: &str, class: PriorityClass, members: &[&str], requires: &[&str]) -> Group {
        Group {
            id: id.into(),
            class,
            trigger: Trigger::Always,
            members: members.iter().map(|m| LogicalId::new(*m)).collect(),
            bytes: 0,
            requires: requires.iter().map(|r| (*r).to_owned()).collect(),
        }
    }

    /// boot 1 MB; shared 10 MB (+ 1 MB @low); europe 5, asia 3, america 2, africa 1; water 4.
    fn sample() -> (Manifest, GroupTable) {
        let mut m = Manifest::new();
        put(&mut m, "tx:cube.dds", 1, 1_000_000);
        put(&mut m, "tx:big_h.dds", 2, 10_000_000);
        put(&mut m, "tx:big_h.dds@low", 3, 1_000_000);
        put(&mut m, "pc:europe", 4, 5_000_000);
        put(&mut m, "pc:asia", 5, 3_000_000);
        put(&mut m, "pc:america", 6, 2_000_000);
        put(&mut m, "pc:africa", 7, 1_000_000);
        put(&mut m, "tx:waves.dds", 8, 4_000_000);
        put(&mut m, "tx:orphan.dds", 9, 100);
        put(&mut m, "tex:b1_old", 10, 7);
        put(&mut m, GROUP_TABLE_ID, 11, 300);
        let mut t = GroupTable::new();
        t.upsert(group("boot", PriorityClass::Boot, &["tx:cube.dds"], &[]));
        t.upsert(group(
            "style:shared",
            PriorityClass::Map,
            &["tx:big_h.dds", "tx:big_h.dds@low"],
            &["boot"],
        ));
        for (s, id) in [
            ("europe", "pc:europe"),
            ("asia", "pc:asia"),
            ("america", "pc:america"),
            ("africa", "pc:africa"),
        ] {
            t.upsert(group(
                &format!("style:{s}"),
                PriorityClass::Map,
                &[id],
                &["boot", "style:shared"],
            ));
        }
        t.upsert(group(
            "water",
            PriorityClass::Map,
            &["tx:waves.dds"],
            &["boot"],
        ));
        t.upsert(group("empty", PriorityClass::Near, &[], &[]));
        (m, t)
    }

    #[test]
    fn coverage_lists_b2_ids_in_no_group_and_empty_groups() {
        let (m, t) = sample();
        let c = coverage(&m, &t);
        // The B1 id is not counted; the group table counts as covered.
        assert_eq!((c.b2_ids, c.grouped), (10, 9));
        assert_eq!(c.ungrouped, ["tx:orphan.dds"]);
        assert_eq!(c.empty_groups, ["empty"]);
        let tx = c.by_prefix.iter().find(|p| p.prefix == "tx").unwrap();
        assert_eq!((tx.ids, tx.grouped), (5, 4));
        let text = c.to_text();
        assert!(
            text.contains("9 of 10") && text.contains("tx:orphan.dds"),
            "{text}"
        );
    }

    #[test]
    fn cold_start_counts_each_blob_once_leaves_out_low_and_takes_it_on_low() {
        let (m, t) = sample();
        let cs = cold_start(&m, &t);
        assert_eq!(cs.len(), 4);
        let by = |name: &str| cs.iter().find(|c| c.case.starts_with(name)).unwrap();
        // boot 1 + shared 10 (not its @low) + styles 5+3+2+1 + water 4: the shared group is
        // required by four groups and counts once.
        let worst = &cs[0];
        assert!(worst.case.starts_with("worst: "));
        assert_eq!(worst.bytes, 1_000_000 + 10_000_000 + 11_000_000 + 4_000_000);
        assert_eq!(worst.blobs, 1 + 1 + 4 + 1);
        assert!((worst.seconds - worst.bytes as f64 / 12.5e6).abs() < 1e-9);
        // typical: boot, shared, water and europe + asia (the two largest).
        assert_eq!(
            by("typical: ").bytes,
            1_000_000 + 10_000_000 + 4_000_000 + 5_000_000 + 3_000_000
        );
        // Low: the shared texture is its 1 MB variant.
        assert_eq!(by("worst on Low").bytes, worst.bytes - 9_000_000);
        assert!(
            by("typical on Low")
                .groups
                .contains(&"style:europe".to_owned())
        );
        assert!(
            !by("typical on Low")
                .groups
                .contains(&"style:africa".to_owned())
        );
    }

    #[test]
    fn a_table_without_map_groups_has_no_cold_start() {
        let (m, _) = sample();
        let mut t = GroupTable::new();
        t.upsert(group("boot", PriorityClass::Boot, &["tx:cube.dds"], &[]));
        assert!(cold_start(&m, &t).is_empty());
    }
}
