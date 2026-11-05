//! Set `b2-landmarks` (4.15 item 3): every row of
//! `ArtDefine_Landmarks` (631 in the snapshot) as a `landmark/<improvement>/<resource>/<n>` record
//! of `tb:art-b2`, and the models they name (482 `.fxsxml`) as `C5MD` v2 blobs with their FGX
//! materials, textures and clips, by the shared builder ([`super::b2_models`]).
//!
//! The pipeline puts every row in and chooses nothing (RW-15): which row draws for a plot (exact
//! match before `Any`, era, state, technology, the random pick) is the client's.
//!
//! # Merging (4.8)
//! A model that only `SNAPSHOT` rows use merges over the whole gr2 (one mesh per material; this is
//! almost all of the saving: 8 603 Granny meshes in the install). A model that any other layout
//! handler (`RANDOM`, `FARM`, `SPECIAL`, `ANIMATED`, `ANIMATED_ROTATED`, `PASTURE`, `SPRING`) uses
//! keeps its parts, and its meshes merge only inside one part, because the handler places the
//! parts one by one (the order of `world-objects`, 12.2 item 6 (a)). The count of models that two
//! kinds of handler share is in the report.
//!
//! # The smoke of a pillaged improvement
//! The two effect models [`PILLAGED_SMOKE_MODELS`] (`FXM_Pillaged_Smoke_02`, `FXM_Pillaged_Farm_Smoke_02`)
//! are built with the set although no row names them: the `.ftsxml` triggers of the pillaged models
//! play them as visual effects (`ART_DEF_VEFFECT_*PILLAGED_SMOKE*`), and the client places them
//! over the plot (R5.13). They are the models `civ5artdefines_viseffectmodels_shared.xml` gives
//! `ART_DEF_MEFFECT_PILLAGED_SMOKE_NEW_2` and `..._FARM_SMOKE_NEW_2`, which the shared
//! `civ5artdefines_viseffects_shared.xml` uses for those visual effects instead of the base
//! `_01` models of the base file. No record names them: the group step gives each an `obj:` group.
//!
//! # `landmarks.csv`
//! One line per row of the table: the model, what was built (meshes, triangles, materials,
//! textures, clips) and `ok` or the reason (a row whose `Model` is NULL draws nothing and says so;
//! an `ANIMATED` row whose model has no clip says so too). The client test places one landmark of
//! each line (4.15 item 9).
//!
//! SIMPLIFIED(SIMP-012): narrowed to the data: the table holds the real rows; the choice among
//! them and the layout are the client's.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

use asset_format::{ArtTable, LandmarkArt, PrefetchGroup, VariantFormat, is_natural_wonder_art};

use super::b2_models::{AnimSummary, BuiltModels, CsvRow, ModelJob, Options, build_models};
use crate::anim::idle::ModelKind;
use crate::db::{self, DbError, LandmarkRow};
use crate::game::GameSource;
use crate::models::ModelError;
use crate::models::merge::MergeScope;
use crate::report::{ReportEntry, SetReport};
use crate::store::{BlobStore, ManifestBuilder, StoreStats};
use crate::world::art_table::{flat_fxsxml, load_table, md_id, store_table};

pub const SET_NAME: &str = "b2-landmarks";

/// Name of the per-row table in the output directory.
pub const LANDMARKS_CSV: &str = "landmarks.csv";

/// Fetched on demand, by the group table (AB-D10).
const GROUP: PrefetchGroup = PrefetchGroup(0);

/// The flat `.fxsxml` names of the pillaged smoke effect models: the smoke of an improvement and of
/// a farm.
pub const PILLAGED_SMOKE_MODELS: [&str; 2] = [
    "fxm_pillaged_smoke_02.fxsxml",
    "fxm_pillaged_farm_smoke_02.fxsxml",
];

pub fn read_input(snapshot: &Path) -> Result<Vec<LandmarkRow>, DbError> {
    let d = db::open_snapshot(snapshot)?;
    db::all_landmarks(&d)
}

/// The records in table order with their `n` (the position among the rows of the same
/// improvement and resource).
pub fn landmark_records(rows: &[LandmarkRow]) -> Vec<(usize, LandmarkArt)> {
    let mut count: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    rows.iter()
        .map(|r| {
            let n = count
                .entry((r.improvement.as_str(), r.resource.as_str()))
                .or_default();
            let i = *n;
            *n += 1;
            (
                i,
                LandmarkArt {
                    improvement: r.improvement.clone(),
                    resource: r.resource.clone(),
                    era: r.era.clone(),
                    state: r.state.clone(),
                    layout: r.layout.clone(),
                    scale: r.scale,
                    terrain_contour: r.terrain_contour,
                    tech: r.tech.clone(),
                    model: r.model.as_deref().map(md_id),
                },
            )
        })
        .collect()
}

/// The models to build: one job per distinct `.fxsxml`. The scope is the whole model when every
/// row that names it is a `SNAPSHOT` row, else the part. Also returns the models that rows of
/// both kinds use.
// SIMPLIFIED(SIMP-exe-1071): a natural wonder's pieces stay apart; a set built earlier needs a rebuild
pub fn model_jobs(rows: &[LandmarkRow]) -> (Vec<ModelJob>, Vec<String>) {
    let mut layouts: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
    // The models of the natural wonders: their pieces stay apart (each is seated on the ground at its
    // own origin, RE-B12 item 1b), whatever the layout of the row says.
    let mut wonders: BTreeSet<String> = BTreeSet::new();
    for r in rows {
        if let Some(m) = &r.model {
            layouts
                .entry(flat_fxsxml(m))
                .or_default()
                .insert(r.layout.as_str());
            if is_natural_wonder_art(&r.resource) || is_natural_wonder_art(&r.improvement) {
                wonders.insert(flat_fxsxml(m));
            }
        }
    }
    let mut shared = Vec::new();
    let jobs = layouts
        .into_iter()
        .map(|(flat, l)| {
            let snapshot_only = l.len() == 1 && l.contains("SNAPSHOT") && !wonders.contains(&flat);
            if l.contains("SNAPSHOT") && l.len() > 1 {
                shared.push(flat.clone());
            }
            ModelJob {
                flat,
                kind: ModelKind::Feature,
                scope: if snapshot_only {
                    MergeScope::Model
                } else {
                    MergeScope::Part
                },
                xml: None,
                diffuse: None,
            }
        })
        .collect();
    (jobs, shared)
}

/// The jobs of the pillaged smoke models that `exists` finds in the install: plain effect models.
pub fn effect_jobs(exists: impl Fn(&str) -> bool) -> Vec<ModelJob> {
    PILLAGED_SMOKE_MODELS
        .into_iter()
        .filter(|f| exists(f))
        .map(|f| ModelJob {
            flat: f.to_owned(),
            kind: ModelKind::Effect,
            scope: MergeScope::Model,
            xml: None,
            diffuse: None,
        })
        .collect()
}

pub const CSV_HEADER: &str = "row,improvement,resource,era,state,layout,tech,model,parts,meshes,triangles,materials,textures,clips,anim_bytes,status";

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

/// One line of `landmarks.csv`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandmarkCsvRow {
    pub row: usize,
    pub improvement: String,
    pub resource: String,
    pub era: String,
    pub state: String,
    pub layout: String,
    pub tech: String,
    pub model: String,
    pub parts: usize,
    pub meshes: usize,
    pub triangles: usize,
    pub materials: usize,
    pub textures: usize,
    pub clips: usize,
    pub anim_bytes: u64,
    pub status: String,
}

pub fn csv_text(rows: &[LandmarkCsvRow]) -> String {
    let mut out = String::from(CSV_HEADER);
    out.push('\n');
    for r in rows {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            r.row,
            csv_field(&r.improvement),
            csv_field(&r.resource),
            r.era,
            r.state,
            r.layout,
            csv_field(&r.tech),
            csv_field(&r.model),
            r.parts,
            r.meshes,
            r.triangles,
            r.materials,
            r.textures,
            r.clips,
            r.anim_bytes,
            csv_field(&r.status),
        ));
    }
    out
}

/// The status of a row given what was built for its model (`None`: the model is not built).
pub fn row_status(row: &LandmarkRow, built: Option<&CsvRow>) -> String {
    let Some(model) = &row.model else {
        return "ok: the row has no Model (it draws nothing)".to_owned();
    };
    match built {
        None => format!("FAILED: {model} was not built"),
        Some(b) if b.status != "ok" => b.status.clone(),
        Some(b) if b.meshes == 0 => format!("FAILED: {model} has no mesh"),
        Some(b) if row.layout == "ANIMATED" && b.clips == 0 => {
            "FAILED: an ANIMATED row whose model has no clip".to_owned()
        }
        Some(_) => "ok".to_owned(),
    }
}

#[derive(Debug)]
pub struct LandmarksSummary {
    pub rows: usize,
    pub row_failures: usize,
    pub rows_without_model: usize,
    pub models: usize,
    pub failed_models: usize,
    /// Models that SNAPSHOT rows and rows of another handler both use (built with parts).
    pub shared_models: usize,
    pub whole_models: usize,
    pub part_models: usize,
    pub meshes: u64,
    pub meshes_before_merge: u64,
    pub triangles: u64,
    pub materials: u64,
    pub textures: usize,
    pub substitutes: Vec<String>,
    pub damage_models: usize,
    pub model_bytes: u64,
    pub texture_bytes: BTreeMap<VariantFormat, u64>,
    pub table_bytes: u64,
    pub anims: AnimSummary,
    pub report: SetReport,
    pub stats: StoreStats,
    pub seconds: f64,
}

impl LandmarksSummary {
    pub fn failures(&self) -> usize {
        self.failed_models + self.row_failures
    }

    pub fn total_bytes(&self) -> u64 {
        self.model_bytes
            + self.anims.bytes
            + self.anims.fsm.bytes
            + self.texture_bytes.values().sum::<u64>()
            + self.table_bytes
    }
}

/// Builds the set into `out`: the models, their textures and clips, the `landmark/` records of
/// `tb:art-b2` and `landmarks.csv`. Failures are listed in the summary and the report; the caller
/// turns them into an error.
pub fn build_landmarks(
    game: &GameSource,
    snapshot: &Path,
    out: &Path,
    opts: &Options,
) -> Result<LandmarksSummary, ModelError> {
    let started = Instant::now();
    let rows = read_input(snapshot)
        .map_err(|e| ModelError::invalid(format!("reading {}: {e}", snapshot.display())))?;
    let store = BlobStore::open(out)?;
    let mut manifest = ManifestBuilder::open(&store)?;
    let (mut jobs, shared) = model_jobs(&rows);
    jobs.extend(effect_jobs(|f| game.lookup(f).is_some()));
    let BuiltModels {
        good,
        failed,
        rows: model_rows,
        mut entries,
        texture_files,
        stub_ids,
        texture_bytes,
        model_bytes,
        meshes,
        meshes_before_merge,
        triangles,
        materials,
        damage_models,
        anims,
    } = build_models(game, &jobs, &store, &mut manifest, opts, GROUP)?;

    // The table: every row, in order.
    let mut table: ArtTable = load_table(&store, &manifest)?;
    for (n, rec) in landmark_records(&rows) {
        table.insert_landmark(n, rec);
    }
    let table_bytes = store_table(&store, &mut manifest, &table)?;

    // landmarks.csv and the row status.
    let mut csv: Vec<LandmarkCsvRow> = Vec::new();
    let mut row_failures = 0;
    let mut without_model = 0;
    for (i, r) in rows.iter().enumerate() {
        let flat = r.model.as_deref().map(flat_fxsxml);
        let built = flat.as_ref().and_then(|f| model_rows.get(f));
        let status = row_status(r, built);
        if status.starts_with("FAILED") {
            row_failures += 1;
            entries.push(ReportEntry::new(
                "landmark-failed",
                format!("{}/{}", r.improvement, r.resource),
                status.clone(),
            ));
        }
        if r.model.is_none() {
            without_model += 1;
            entries.push(ReportEntry::new(
                "landmark-no-model",
                format!("{}/{}", r.improvement, r.resource),
                "the row has no Model (it draws nothing)",
            ));
        }
        let parts = flat
            .as_ref()
            .and_then(|f| good.iter().find(|(l, _)| &l.flat == f))
            .map_or(0, |(_, b)| b.model.parts.len());
        let b = built.filter(|b| b.status == "ok");
        csv.push(LandmarkCsvRow {
            row: i,
            improvement: r.improvement.clone(),
            resource: r.resource.clone(),
            era: r.era.clone(),
            state: r.state.clone(),
            layout: r.layout.clone(),
            tech: r.tech.clone().unwrap_or_default(),
            model: r.model.clone().unwrap_or_default(),
            parts,
            meshes: b.map_or(0, |b| b.meshes),
            triangles: b.map_or(0, |b| b.triangles),
            materials: b.map_or(0, |b| b.materials),
            textures: b.map_or(0, |b| b.textures),
            clips: b.map_or(0, |b| b.clips),
            anim_bytes: b.map_or(0, |b| b.anim_bytes),
            status,
        });
    }
    for f in &shared {
        entries.push(ReportEntry::new(
            "landmark-model-shared",
            f.clone(),
            "used by SNAPSHOT rows and by rows of another layout handler: built with parts",
        ));
    }
    manifest.finish(&store)?;
    std::fs::write(out.join(LANDMARKS_CSV), csv_text(&csv)).map_err(|e| {
        ModelError::invalid(format!(
            "writing {}: {e}",
            out.join(LANDMARKS_CSV).display()
        ))
    })?;

    let whole = jobs.iter().filter(|j| j.scope == MergeScope::Model).count();
    let mut report = SetReport {
        entries,
        ..SetReport::default()
    };
    for (k, v) in [
        ("rows", rows.len()),
        ("row failures", row_failures),
        ("rows without a model", without_model),
        ("models", good.len()),
        ("failed models", failed.len()),
        ("models merged whole", whole),
        ("models with parts", jobs.len() - whole),
        ("models shared by SNAPSHOT and other handlers", shared.len()),
        ("meshes", meshes as usize),
        ("meshes before merge", meshes_before_merge as usize),
        ("triangles", triangles as usize),
        ("materials", materials as usize),
        ("textures", texture_files.len()),
        ("stub textures", stub_ids.len()),
        ("models with damage meshes", damage_models),
        ("clip packs", anims.packs),
        ("clips", anims.clips),
        ("clip keys", anims.keys),
        ("clip frames at 30 Hz", anims.frames),
        ("state machines", anims.fsm.machines),
        ("models with state clips", anims.fsm.models_with_state_clips),
        ("models without an idle 1000 clip", anims.no_idle.len()),
    ] {
        report.counts.insert(k.to_owned(), v as u64);
    }
    for (f, n) in &anims.by_family {
        report.counts.insert(format!("clip entries {f}"), *n as u64);
    }
    report.bytes.insert("models".to_owned(), model_bytes);
    for (f, b) in &texture_bytes {
        report.bytes.insert(format!("textures {f:?}"), *b);
    }
    report.bytes.insert("table".to_owned(), table_bytes);
    report.bytes.insert("clip packs".to_owned(), anims.bytes);
    report
        .bytes
        .insert("state machines".to_owned(), anims.fsm.bytes);
    Ok(LandmarksSummary {
        rows: rows.len(),
        row_failures,
        rows_without_model: without_model,
        models: good.len(),
        failed_models: failed.len(),
        shared_models: shared.len(),
        whole_models: whole,
        part_models: jobs.len() - whole,
        meshes,
        meshes_before_merge,
        triangles,
        materials,
        textures: texture_files.len(),
        substitutes: stub_ids,
        damage_models,
        model_bytes,
        texture_bytes,
        table_bytes,
        anims,
        report,
        stats: store.stats(),
        seconds: started.elapsed().as_secs_f64(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(improvement: &str, resource: &str, layout: &str, model: Option<&str>) -> LandmarkRow {
        LandmarkRow {
            era: "Any".into(),
            state: "Any".into(),
            scale: 1.0,
            improvement: improvement.into(),
            layout: layout.into(),
            resource: resource.into(),
            model: model.map(str::to_owned),
            terrain_contour: 1,
            tech: None,
        }
    }

    #[test]
    fn every_row_is_a_record_and_n_counts_per_pair() {
        let rows = vec![
            row("A", "R1", "RANDOM", Some("Assets/x/A1.fxsxml")),
            row("A", "R1", "RANDOM", Some("A2.fxsxml")),
            row("A", "R2", "RANDOM", None),
            row("B", "R1", "SNAPSHOT", Some("B.fxsxml")),
        ];
        let recs = landmark_records(&rows);
        let ns: Vec<usize> = recs.iter().map(|(n, _)| *n).collect();
        assert_eq!(ns, [0, 1, 0, 0]);
        assert_eq!(recs[0].1.model.as_ref().unwrap().as_str(), "md:a1");
        assert_eq!(recs[2].1.model, None);
        let mut t = ArtTable::new();
        for (n, r) in recs {
            t.insert_landmark(n, r);
        }
        assert_eq!(t.landmarks().count(), 4, "no row is lost to a key clash");
    }

    #[test]
    fn snapshot_only_models_merge_whole_and_the_rest_keep_their_parts() {
        let rows = vec![
            row("A", "R", "SNAPSHOT", Some("snap.fxsxml")),
            row("B", "R", "SNAPSHOT", Some("snap.fxsxml")),
            row("C", "R", "RANDOM", Some("rand.fxsxml")),
            row("D", "R", "SNAPSHOT", Some("both.fxsxml")),
            row("E", "R", "ANIMATED", Some("both.fxsxml")),
            row("F", "R", "SNAPSHOT", None),
        ];
        let (jobs, shared) = model_jobs(&rows);
        let scope = |n: &str| jobs.iter().find(|j| j.flat == n).unwrap().scope;
        assert_eq!(jobs.len(), 3);
        assert_eq!(scope("snap.fxsxml"), MergeScope::Model);
        assert_eq!(scope("rand.fxsxml"), MergeScope::Part);
        assert_eq!(scope("both.fxsxml"), MergeScope::Part);
        assert_eq!(shared, ["both.fxsxml"]);
        assert!(jobs.iter().all(|j| j.kind == ModelKind::Feature));
    }

    #[test]
    fn the_models_of_natural_wonders_keep_their_pieces_apart_even_as_snapshots() {
        let rows = vec![
            row(
                "ART_DEF_IMPROVEMENT_NONE",
                "ART_DEF_FEATURE_EL_DORADO",
                "SNAPSHOT",
                Some("el_dorado.fxsxml"),
            ),
            row(
                "ART_DEF_IMPROVEMENT_NONE",
                "ART_DEF_FEATURE_LAKE_VICTORIA",
                "SNAPSHOT",
                Some("lake.fxsxml"),
            ),
            row(
                "ART_DEF_IMPROVEMENT_NONE",
                "ART_DEF_RESOURCE_WHEAT",
                "SNAPSHOT",
                Some("wheat.fxsxml"),
            ),
        ];
        let (jobs, _) = model_jobs(&rows);
        let scope = |n: &str| jobs.iter().find(|j| j.flat == n).unwrap().scope;
        assert_eq!(scope("el_dorado.fxsxml"), MergeScope::Part);
        assert_eq!(scope("lake.fxsxml"), MergeScope::Part);
        assert_eq!(scope("wheat.fxsxml"), MergeScope::Model);
    }

    #[test]
    fn the_pillaged_smoke_models_are_effects_and_only_those_the_install_has() {
        let jobs = effect_jobs(|_| true);
        assert_eq!(jobs.len(), 2);
        assert!(jobs.iter().all(|j| j.kind == ModelKind::Effect));
        assert!(jobs.iter().all(|j| j.scope == MergeScope::Model));
        let ids: Vec<String> = jobs
            .iter()
            .map(|j| md_id(&j.flat).as_str().to_owned())
            .collect();
        assert_eq!(
            ids,
            ["md:fxm_pillaged_smoke_02", "md:fxm_pillaged_farm_smoke_02"]
        );
        let only_farm = effect_jobs(|f| f.contains("farm"));
        assert_eq!(only_farm.len(), 1);
        assert_eq!(only_farm[0].flat, "fxm_pillaged_farm_smoke_02.fxsxml");
        assert!(effect_jobs(|_| false).is_empty());
        // No row names them, so the group step makes them `obj:` groups of their own.
        assert!(PILLAGED_SMOKE_MODELS.iter().all(|f| f.ends_with(".fxsxml")));
    }

    fn built(status: &str, meshes: usize, clips: usize) -> CsvRow {
        CsvRow {
            kind: "model",
            id: String::new(),
            art_info: String::new(),
            model: "m.fxsxml".into(),
            bones: 1,
            meshes,
            triangles: 10,
            materials: 1,
            damage: [0; 4],
            textures: 1,
            clips,
            anim_bytes: 0,
            status: status.into(),
        }
    }

    #[test]
    fn a_row_is_ok_or_says_why_not() {
        let r = |layout: &str, model: Option<&str>| row("A", "R", layout, model);
        assert!(row_status(&r("RANDOM", None), None).starts_with("ok: the row has no Model"));
        assert_eq!(
            row_status(&r("RANDOM", Some("m")), Some(&built("ok", 2, 0))),
            "ok"
        );
        assert!(row_status(&r("RANDOM", Some("m")), None).starts_with("FAILED"));
        assert!(
            row_status(&r("RANDOM", Some("m")), Some(&built("FAILED: x", 0, 0))) == "FAILED: x"
        );
        assert!(row_status(&r("RANDOM", Some("m")), Some(&built("ok", 0, 0))).contains("no mesh"));
        // Only an ANIMATED row needs a clip.
        assert!(
            row_status(&r("ANIMATED", Some("m")), Some(&built("ok", 1, 0))).contains("no clip")
        );
        assert_eq!(
            row_status(&r("ANIMATED", Some("m")), Some(&built("ok", 1, 2))),
            "ok"
        );
        assert_eq!(
            row_status(&r("SPECIAL", Some("m")), Some(&built("ok", 1, 0))),
            "ok"
        );
    }

    #[test]
    fn the_csv_has_one_line_per_row_and_quotes() {
        let line = LandmarkCsvRow {
            row: 3,
            improvement: "A,B".into(),
            resource: "R".into(),
            era: "Any".into(),
            state: "Any".into(),
            layout: "RANDOM".into(),
            tech: String::new(),
            model: "m.fxsxml".into(),
            parts: 2,
            meshes: 3,
            triangles: 4,
            materials: 5,
            textures: 6,
            clips: 7,
            anim_bytes: 8,
            status: "ok".into(),
        };
        let t = csv_text(&[line]);
        let mut lines = t.lines();
        assert_eq!(lines.next(), Some(CSV_HEADER));
        assert_eq!(
            lines.next(),
            Some("3,\"A,B\",R,Any,Any,RANDOM,,m.fxsxml,2,3,4,5,6,7,8,ok")
        );
        assert_eq!(CSV_HEADER.split(',').count(), 16);
    }
}
