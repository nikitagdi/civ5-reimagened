//! Set `b1-sv`: strategic-view icons of every `ArtDefine_StrategicView`
//! row (units, resources, improvements, features, terrains; the barbarian camp and the ruins are
//! improvements of the snapshot), the records `sv/<ArtDefineTag>`, and the records
//! `player_color/<PLAYERCOLOR_TYPE>`.
//!
//! SIMPLIFIED(SIMP-012): improvements, resources, camps and ruins are shown by their SV icon
//! instead of a model.
//! SIMPLIFIED(SIMP-014): the flag icon of a unit is its SV icon instead of a UI atlas entry.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use asset_format::{PrefetchGroup, SvArt, Variant, VariantFormat};
use rayon::prelude::*;

use super::terrain::ReadFile;
use super::{TextureOptions, convert_texture, load_art_table, store_art_table};
use crate::db::{
    DbError, PlayerColorRow, SnapDb, StrategicViewRow, open_snapshot, player_colors,
    strategic_view_rows,
};
use crate::models::ModelError;
use crate::models::set::texture_id;
use crate::store::{BlobStore, BuildCache, ManifestBuilder, StoreStats};

pub const SET_NAME: &str = "b1-sv";

/// An entry for the build report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportEntry {
    /// `sv-file-missing` or `sv-tag-missing`.
    pub kind: &'static str,
    pub subject: String,
    pub reason: String,
}

impl std::fmt::Display for ReportEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}: {}", self.kind, self.subject, self.reason)
    }
}

/// A game type that must have an SV icon (a resource or an improvement).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Needed {
    /// `Resources` or `Improvements`.
    pub table: &'static str,
    pub game_type: String,
    pub art_tag: Option<String>,
}

/// Everything the set reads from the snapshot.
#[derive(Debug, Clone, Default)]
pub struct SvInput {
    pub rows: Vec<StrategicViewRow>,
    pub needed: Vec<Needed>,
    pub colors: Vec<PlayerColorRow>,
}

fn needed_from(db: &SnapDb, table: &'static str) -> Result<Vec<Needed>, DbError> {
    Ok(db
        .query(
            &format!("SELECT Type, ArtDefineTag FROM {table} ORDER BY ID"),
            2,
        )?
        .into_iter()
        .filter_map(|r| {
            Some(Needed {
                table,
                game_type: r[0].clone()?,
                art_tag: r[1].clone().filter(|t| !t.is_empty()),
            })
        })
        .collect())
}

/// Reads the SV rows, the resources and improvements with their art tags, and the player colors.
pub fn load_input(snapshot: &Path) -> Result<SvInput, ModelError> {
    let wrap = |e: DbError| ModelError::invalid(format!("reading {}: {e}", snapshot.display()));
    let db = open_snapshot(snapshot).map_err(wrap)?;
    let mut needed = needed_from(&db, "Resources").map_err(wrap)?;
    needed.extend(needed_from(&db, "Improvements").map_err(wrap)?);
    Ok(SvInput {
        rows: strategic_view_rows(&db).map_err(wrap)?,
        needed,
        colors: player_colors(&db).map_err(wrap)?,
    })
}

/// Records and textures of the set, before any conversion.
#[derive(Debug, Default)]
pub struct SvPlan {
    /// Tag to lowercase icon file.
    pub icons: BTreeMap<String, String>,
    /// Lowercase icon files to convert.
    pub files: BTreeSet<String>,
}

pub fn plan_sv(input: &SvInput) -> SvPlan {
    let mut plan = SvPlan::default();
    for r in &input.rows {
        let file = r.asset.to_ascii_lowercase();
        // Tags are unique in the snapshot; if one repeats, the first tile type wins.
        plan.icons
            .entry(r.tag.clone())
            .or_insert_with(|| file.clone());
        plan.files.insert(file);
    }
    plan
}

/// Report entries for what the table cannot name: needed tags without an
/// `ArtDefine_StrategicView` row, and tags whose icon file is not in the install (`missing`,
/// lowercase file names).
pub fn report(input: &SvInput, plan: &SvPlan, missing: &BTreeSet<String>) -> Vec<ReportEntry> {
    let mut out = Vec::new();
    for n in &input.needed {
        let subject = format!("{} {}", n.table, n.game_type);
        match &n.art_tag {
            None => out.push(ReportEntry {
                kind: "sv-tag-missing",
                subject,
                reason: "no ArtDefineTag".into(),
            }),
            Some(tag) => match plan.icons.get(tag) {
                None => out.push(ReportEntry {
                    kind: "sv-tag-missing",
                    subject,
                    reason: format!("{tag} has no ArtDefine_StrategicView row"),
                }),
                Some(file) if missing.contains(file) => out.push(ReportEntry {
                    kind: "sv-file-missing",
                    subject,
                    reason: format!("{tag}: {file} is not in the install"),
                }),
                Some(_) => {}
            },
        }
    }
    let needed_tags: BTreeSet<&str> = input
        .needed
        .iter()
        .filter_map(|n| n.art_tag.as_deref())
        .collect();
    for (tag, file) in &plan.icons {
        if missing.contains(file) && !needed_tags.contains(tag.as_str()) {
            out.push(ReportEntry {
                kind: "sv-file-missing",
                subject: format!("tag {tag}"),
                reason: format!("{file} is not in the install"),
            });
        }
    }
    out
}

/// What a build of the set produced.
#[derive(Debug)]
pub struct SvSummary {
    pub textures: usize,
    pub bytes: BTreeMap<VariantFormat, u64>,
    pub sv_records: usize,
    pub color_records: usize,
    pub table_entries: usize,
    pub table_bytes: u64,
    pub report: Vec<ReportEntry>,
    pub stats: StoreStats,
}

impl SvSummary {
    pub fn total_bytes(&self) -> u64 {
        self.bytes.values().sum::<u64>() + self.table_bytes
    }
}

/// Builds the set into `out`: icons and table records, added to the manifest already there.
/// An icon file the install lacks is not an error: its tags get no record and a report entry.
pub fn build_sv(
    read: ReadFile<'_>,
    input: &SvInput,
    out: &Path,
    opts: TextureOptions,
) -> Result<SvSummary, ModelError> {
    let plan = plan_sv(input);
    let store = BlobStore::open(out)?;
    let cache = BuildCache::open(&store)?;
    let mut manifest = ManifestBuilder::open(&store)?;

    let files: Vec<&String> = plan.files.iter().collect();
    let converted: Vec<Result<Option<Vec<_>>, ModelError>> = files
        .par_iter()
        .map(|f| {
            read(f)?
                .map(|bytes| convert_texture(&cache, &store, f, &bytes, opts))
                .transpose()
        })
        .collect();
    let mut bytes: BTreeMap<VariantFormat, u64> = BTreeMap::new();
    let mut missing = BTreeSet::new();
    for (file, result) in files.iter().zip(converted) {
        let Some(variants) = result? else {
            missing.insert((*file).clone());
            continue;
        };
        for (format, hash, size) in variants {
            *bytes.entry(format).or_default() += size;
            manifest.add(
                texture_id(file),
                Variant {
                    format,
                    hash,
                    size,
                    prefetch: PrefetchGroup(1),
                },
            );
        }
    }

    let mut table = load_art_table(&store, &manifest)?;
    let mut sv_records = 0;
    for (tag, file) in &plan.icons {
        if !missing.contains(file) {
            table.insert_sv(
                tag,
                SvArt {
                    icon: texture_id(file),
                },
            );
            sv_records += 1;
        }
    }
    for c in &input.colors {
        table.insert_player_color(&c.color_type, c.color);
    }
    let table_bytes = store_art_table(&store, &mut manifest, &table)?;
    manifest.finish(&store)?;
    Ok(SvSummary {
        textures: files.len() - missing.len(),
        bytes,
        sv_records,
        color_records: input.colors.len(),
        table_entries: table.len(),
        table_bytes,
        report: report(input, &plan, &missing),
        stats: store.stats(),
    })
}
