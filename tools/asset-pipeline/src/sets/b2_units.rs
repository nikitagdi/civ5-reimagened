//! Set `b2-units`: every unit model of the game as a `C5MD` v2
//! blob with FGX materials, the textures of the materials as the original's bytes, the unit
//! records of `tb:art-b2` and the `VisEffectArtInfo` rows.
//!
//! The snapshot decides what is built: every `ArtDefine_UnitMemberInfos` row (329 members) names an
//! `.fxsxml` (319 distinct); its mesh gr2 is converted by the shared model builder
//! ([`super::b2_models`], which also holds the notes on materials and SIMP-061). No model is
//! dropped and no stand-in is drawn (AB-D14): a model that cannot be built fails the build, after
//! `units.csv` and the report say why.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

use asset_format::{ArtTable, PrefetchGroup, VariantFormat};

pub use super::b2_models::{AnimSummary, CsvRow, FsmSummary, Options};
use super::b2_models::{BuiltModels, ModelJob, build_models};
use crate::anim::idle::ModelKind;
use crate::db::{self, DbError};
use crate::game::GameSource;
use crate::models::ModelError;
use crate::models::merge::MergeScope;
use crate::report::{ReportEntry, SetReport};
use crate::store::{BlobStore, ManifestBuilder, StoreStats};
use crate::world::art_table::{
    UnitsInput, embark_records, flat_fxsxml, is_vis_effects_file, load_table, merge_vis_effects,
    parse_vis_effects, store_table, unit_records,
};

pub const SET_NAME: &str = "b2-units";

/// File name of the per-unit and per-member table in the output directory.
pub const UNITS_CSV: &str = "units.csv";

/// Units, models and textures are fetched on demand (group by the group table, AB-D10).
const GROUP: PrefetchGroup = PrefetchGroup(0);

// ---------------------------------------------------------------- snapshot input

/// What the set reads from the snapshot.
#[derive(Debug, Clone, Default)]
pub struct SnapshotInput {
    pub units: UnitsInput,
    /// Flat fxsxml name to the member types that name it, sorted.
    pub models: BTreeMap<String, Vec<String>>,
}

pub fn read_input(snapshot: &Path) -> Result<SnapshotInput, DbError> {
    let d = db::open_snapshot(snapshot)?;
    let units = db::all_units(&d)?;
    let arts = db::all_art_infos(&d)?
        .into_iter()
        .map(|a| (a.art_info.clone(), a))
        .collect();
    let art_members = db::all_art_members(&d)?;
    let members: BTreeMap<String, db::MemberInfoRow> = db::all_member_infos(&d)?
        .into_iter()
        .map(|m| (m.member_type.clone(), m))
        .collect();
    let mut models: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for m in members.values() {
        models
            .entry(flat_fxsxml(&m.model))
            .or_default()
            .push(m.member_type.clone());
    }
    Ok(SnapshotInput {
        units: UnitsInput {
            units,
            arts,
            art_members,
            members,
            style_suffixes: db::style_suffixes(&d)?,
        },
        models,
    })
}
pub const CSV_HEADER: &str = "kind,id,art_info,model,bones,meshes,triangles,materials,damage0,damage1,damage2,damage3,textures,clips,anim_bytes,status";

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

pub fn csv_text(rows: &[CsvRow]) -> String {
    let mut out = String::from(CSV_HEADER);
    out.push('\n');
    for r in rows {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            r.kind,
            csv_field(&r.id),
            csv_field(&r.art_info),
            csv_field(&r.model),
            r.bones,
            r.meshes,
            r.triangles,
            r.materials,
            r.damage[0],
            r.damage[1],
            r.damage[2],
            r.damage[3],
            r.textures,
            r.clips,
            r.anim_bytes,
            csv_field(&r.status),
        ));
    }
    out
}

#[derive(Debug)]
pub struct UnitsSummary {
    pub units: usize,
    pub unit_failures: usize,
    pub members: usize,
    pub models: usize,
    pub failed_models: usize,
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
    pub vis_effects: usize,
    pub anims: AnimSummary,
    pub report: SetReport,
    pub stats: StoreStats,
    pub seconds: f64,
}

impl UnitsSummary {
    pub fn failures(&self) -> usize {
        self.failed_models + self.unit_failures
    }

    pub fn total_bytes(&self) -> u64 {
        self.model_bytes
            + self.anims.bytes
            + self.anims.fsm.bytes
            + self.texture_bytes.values().sum::<u64>()
            + self.table_bytes
    }
}

/// Builds the set into `out`: models, textures, `tb:art-b2` and `units.csv`. Failed models are
/// listed in the summary and the report; the caller turns them into an error.
pub fn build_units(
    game: &GameSource,
    snapshot: &Path,
    out: &Path,
    opts: &Options,
) -> Result<UnitsSummary, ModelError> {
    let started = Instant::now();
    let input = read_input(snapshot)
        .map_err(|e| ModelError::invalid(format!("reading {}: {e}", snapshot.display())))?;
    let store = BlobStore::open(out)?;
    let mut manifest = ManifestBuilder::open(&store)?;

    // The models: effect models (`fxm_*`) carry only the clips of code 1000.
    let jobs: Vec<ModelJob> = input
        .models
        .keys()
        .map(|f| ModelJob {
            flat: f.clone(),
            kind: if f.starts_with("fxm_") {
                ModelKind::Effect
            } else {
                ModelKind::Unit
            },
            scope: MergeScope::Part,
            xml: None,
            diffuse: None,
        })
        .collect();
    let BuiltModels {
        good,
        failed,
        rows,
        mut entries,
        texture_files: files,
        stub_ids,
        texture_bytes,
        model_bytes,
        meshes,
        meshes_before_merge: meshes_before,
        triangles,
        materials,
        damage_models,
        anims,
    } = build_models(game, &jobs, &store, &mut manifest, opts, GROUP)?;

    // 6. The art table: unit records and the visual effects.
    let (records, broken) = unit_records(&input.units);
    let mut table: ArtTable = load_table(&store, &manifest)?;
    for (unit_type, rec) in &records {
        table.insert_unit_b2(unit_type, rec.clone());
    }
    // The boats of the embarked units, keyed by their art info (no unit type starts `ART_DEF_`).
    let (embark, embark_broken) = embark_records(&input.units);
    for (art_info, rec) in &embark {
        table.insert_unit_b2(art_info, rec.clone());
    }
    for why in &embark_broken {
        entries.push(ReportEntry::new(
            "embark-art-missing",
            "embark",
            why.clone(),
        ));
    }
    let mut vis_files: Vec<(String, Vec<asset_format::VisEffectArt>)> = Vec::new();
    for e in game
        .iter_ext("xml")
        .filter(|e| is_vis_effects_file(&e.name))
    {
        let bytes = game.read(e)?;
        let text = String::from_utf8_lossy(&bytes);
        let text = text.trim_start_matches('\u{feff}');
        let rows =
            parse_vis_effects(text).map_err(|m| ModelError::invalid(format!("{}: {m}", e.name)))?;
        vis_files.push((e.name.to_ascii_lowercase(), rows));
    }
    let (vis, replaced) = merge_vis_effects(&vis_files);
    for v in &vis {
        table.insert_vis_effect(v.clone());
    }
    for n in &replaced {
        entries.push(ReportEntry::new(
            "vis-effect-replaced",
            n.clone(),
            "defined in several files; the later file wins",
        ));
    }
    let table_bytes = store_table(&store, &mut manifest, &table)?;

    // 7. Units: a unit is complete when its record exists and every model of it was built.
    let mut csv: Vec<CsvRow> = Vec::new();
    let mut unit_failures = 0;
    for u in &input.units.units {
        let mut status = "ok".to_owned();
        let mut bad_models: Vec<String> = Vec::new();
        let mut model_names: BTreeSet<String> = BTreeSet::new();
        let (mut meshes_u, mut tri_u, mut mat_u, mut bones_u) = (0, 0, 0, 0);
        let mut damage_u = [0usize; 4];
        let (mut clips_u, mut anim_bytes_u) = (0usize, 0u64);
        if let Some(rec) = records.get(&u.unit_type) {
            let arts = std::iter::once(&rec.base)
                .chain(rec.style_variants.iter().map(|v| &v.art))
                .chain(rec.era_variants.iter().map(|v| &v.art));
            for art in arts {
                for f in &art.figures {
                    let flat = format!("{}.fxsxml", f.model.as_str().trim_start_matches("md:"));
                    if !model_names.insert(flat.clone()) {
                        continue;
                    }
                    match rows.get(&flat) {
                        Some(r) if r.status == "ok" => {
                            if art.art_info == rec.base.art_info {
                                meshes_u += r.meshes;
                                tri_u += r.triangles;
                                mat_u += r.materials;
                                clips_u += r.clips;
                                anim_bytes_u += r.anim_bytes;
                                bones_u += r.bones;
                                for (d, x) in damage_u.iter_mut().zip(r.damage) {
                                    *d += x;
                                }
                            }
                        }
                        Some(r) => bad_models.push(format!("{flat}: {}", r.status)),
                        None => bad_models.push(format!("{flat}: not built")),
                    }
                }
            }
            // DamageStates = 3 is not always a hull swap: a model without a second hull (a
            // single material, the damaged texture only named by the fxsxml) is listed, not failed.
            if rec.base.damage_states == Some(3)
                && damage_u[2] == 0
                && damage_u[3] == 0
                && bad_models.is_empty()
            {
                entries.push(ReportEntry::new(
                    "damage-states-no-meshes",
                    u.unit_type.clone(),
                    format!(
                        "{}: DamageStates = 3 but the gr2 has no light or heavy damage hull",
                        rec.base.art_info
                    ),
                ));
            }
        } else if let Some(why) = broken.get(&u.unit_type) {
            bad_models.push(why.clone());
        }
        if !bad_models.is_empty() {
            status = format!("FAILED: {}", bad_models.join("; "));
            unit_failures += 1;
            entries.push(ReportEntry::new(
                "unit-failed",
                u.unit_type.clone(),
                bad_models.join("; "),
            ));
        }
        csv.push(CsvRow {
            kind: "unit",
            id: u.unit_type.clone(),
            art_info: u.art_info.clone(),
            model: model_names.iter().cloned().collect::<Vec<_>>().join(";"),
            bones: bones_u,
            meshes: meshes_u,
            triangles: tri_u,
            materials: mat_u,
            damage: damage_u,
            textures: 0,
            clips: clips_u,
            anim_bytes: anim_bytes_u,
            status,
        });
    }
    for (flat, members) in &input.models {
        let Some(mut row) = rows.get(flat).cloned() else {
            continue;
        };
        for m in members {
            let art_infos: BTreeSet<&str> = input
                .units
                .art_members
                .iter()
                .filter(|(_, ms)| ms.iter().any(|(mm, _)| mm == m))
                .map(|(a, _)| a.as_str())
                .collect();
            row.id = m.clone();
            row.art_info = art_infos.into_iter().collect::<Vec<_>>().join(";");
            csv.push(row.clone());
        }
    }
    manifest.finish(&store)?;
    std::fs::write(out.join(UNITS_CSV), csv_text(&csv)).map_err(|e| {
        ModelError::invalid(format!("writing {}: {e}", out.join(UNITS_CSV).display()))
    })?;

    let mut report = SetReport {
        entries,
        ..SetReport::default()
    };
    for (k, v) in [
        ("units", input.units.units.len()),
        ("unit failures", unit_failures),
        ("members", input.units.members.len()),
        ("models", good.len()),
        ("failed models", failed.len()),
        ("meshes", meshes as usize),
        ("meshes before merge", meshes_before as usize),
        ("triangles", triangles as usize),
        ("materials", materials as usize),
        ("textures", files.len()),
        ("stub textures", stub_ids.len()),
        ("models with damage meshes", damage_models),
        ("vis effects", vis.len()),
    ] {
        report.counts.insert(k.to_owned(), v as u64);
    }
    report.bytes.insert("models".to_owned(), model_bytes);
    for (f, b) in &texture_bytes {
        report.bytes.insert(format!("textures {f:?}"), *b);
    }
    report.bytes.insert("table".to_owned(), table_bytes);
    for (k, v) in [
        ("clip packs", anims.packs),
        ("state machines", anims.fsm.machines),
        ("state machines failed", anims.fsm.failed.len()),
        ("state machine states", anims.fsm.states),
        ("state machine transitions", anims.fsm.transitions),
        ("state machine conditions", anims.fsm.conditions),
        ("models with state clips", anims.fsm.models_with_state_clips),
        ("clips", anims.clips),
        ("clip tracks", anims.tracks),
        ("clip keys", anims.keys),
        ("clip keys rotation", anims.channel_keys[0]),
        ("clip keys translation", anims.channel_keys[1]),
        ("clip keys scale", anims.channel_keys[2]),
        ("clip frames at 30 Hz", anims.frames),
        ("models without an idle 1000 clip", anims.no_idle.len()),
    ] {
        report.counts.insert(k.to_owned(), v as u64);
    }
    for (f, n) in &anims.by_family {
        report.counts.insert(format!("clip entries {f}"), *n as u64);
    }
    for (f, n) in &anims.keys_by_family {
        report.counts.insert(format!("clip keys {f}"), *n as u64);
    }
    report.bytes.insert("clip packs".to_owned(), anims.bytes);
    report
        .bytes
        .insert("state machines".to_owned(), anims.fsm.bytes);
    for (f, n) in &anims.fsm.action_states {
        report.counts.insert(format!("fsm states {f}"), *n as u64);
    }
    Ok(UnitsSummary {
        units: input.units.units.len(),
        unit_failures,
        members: input.units.members.len(),
        models: good.len(),
        failed_models: failed.len(),
        meshes,
        meshes_before_merge: meshes_before,
        triangles,
        materials,
        textures: files.len(),
        substitutes: stub_ids,
        damage_models,
        model_bytes,
        texture_bytes,
        table_bytes,
        vis_effects: vis.len(),
        anims,
        report,
        stats: store.stats(),
        seconds: started.elapsed().as_secs_f64(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn csv_quotes_what_needs_it() {
        let row = CsvRow {
            kind: "member",
            id: "A,B".into(),
            art_info: "x".into(),
            model: "m.fxsxml".into(),
            bones: 1,
            meshes: 2,
            triangles: 3,
            materials: 4,
            damage: [5, 6, 7, 8],
            textures: 9,
            clips: 10,
            anim_bytes: 11,
            status: "FAILED: \"why\"".into(),
        };
        let t = csv_text(&[row]);
        let mut lines = t.lines();
        assert_eq!(lines.next(), Some(CSV_HEADER));
        assert_eq!(
            lines.next(),
            Some("member,\"A,B\",x,m.fxsxml,1,2,3,4,5,6,7,8,9,10,11,\"FAILED: \"\"why\"\"\"")
        );
        assert_eq!(CSV_HEADER.split(',').count(), 16);
    }
}
