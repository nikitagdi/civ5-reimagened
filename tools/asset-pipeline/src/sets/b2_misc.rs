//! Set `b2-misc`: the trees, the roads, rails and bridges,
//! the path models and the selection ring and highlight textures, as `tb:art-b2` records, `C5MD`
//! v2 models and `tx:` textures.
//!
//! * **Trees** (`tree/<feature>/<style>`, [`TreeArt`]): the `forest_<style>_2.gr2` and
//!   `jungle_<style>_2.gr2` of the B1 set `b1-trees` (the `*_2` re-exports the game draws, the
//!   plain files where a style has no complete `*_2` set), each as one model with a mesh per tree
//!   variant (variant `i` is mesh `i`; the model keeps its parts). The ground overlay under the
//!   trees is `forest_overlay_<style>[_2].dds`; the placement masks
//!   `forest_tile1..4` are `R8` textures (L8 of the original) read on the CPU by the client. A style
//!   without files takes the Europe ones (the jungle has no Generic style), as in B1. The models have
//!   no `.fxsxml`; the asset list is made up from the file names (`<Mesh>` and `<Texture>`).
//!   The simplification SIMP-009 narrows with the client: the masks are read on the CPU now.
//! * **Routes** (`route/<ROUTE>`, [`RouteArt`]): the road models are `roadsandrails`, `roads_rails`
//!   (parts `Road_*` and `RR_*`) and `bridges` (`ANC_Bridge_*`, `IND_Bridge_*`). Which part serves
//!   which link is RW-17's; until it answers every road model is in the record of both routes (the
//!   class is `road` or `bridge` by the file name) and the report lists each model under
//!   `route-unassigned` with the kinds of its parts.
//! * **Path models** (`overlay/<name>`, [`OverlayArt`]): the 24 `Assets/UI/Overlays/*.fxsxml`, with
//!   their textures and the 12 clips they carry; kind `path_center`, `path_connector`, `path_flank`
//!   or `path_selected` and the variant (`OneTurn`, `Attack`, `00`, ...).
//! * **Ring and highlights** (`overlay/ring`, `overlay/highlight`): textures only. The original has
//!   no ring model.
//! * **God ray** ([`GOD_RAY_MODEL`], `md:fxm_god_ray`): the effect model of the beam over the selected unit
//!   (`FXM_God_Ray.fxsxml` of `EffectData.fpk`, the exe's `cvMovementPathVisSystem`), a plain effect model
//!   like the pillage smoke; it has no record. An install without it builds the set without the beam.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

use asset_format::{
    ArtTable, LogicalId, OverlayArt, PrefetchGroup, RouteArt, RouteModel, TreeArt, Variant,
    VariantFormat,
};

use super::b2_models::{AnimSummary, BuiltModels, ModelJob, Options, build_models};
use super::trees::plan_trees;
use super::tx_id;
use crate::anim::idle::ModelKind;
use crate::db::{self, DbError};
use crate::game::GameSource;
use crate::models::ModelError;
use crate::models::merge::MergeScope;
use crate::report::{ReportEntry, SetReport};
use crate::store::{BlobStore, ManifestBuilder, StoreStats};
use crate::textures::{dds_passthrough, variant_format};
use crate::world::art_table::{load_table, md_id, store_table};

pub const SET_NAME: &str = "b2-misc";

/// Fetched on demand, by the group table (AB-D10).
const GROUP: PrefetchGroup = PrefetchGroup(0);

/// Number of placement masks (`forest_tile1..4`, `num_tiles` of `forests.xml`).
pub const MASKS: u32 = 4;

/// The road models of the install, by flat name, and the class of each.
pub const ROUTE_MODELS: [(&str, &str); 3] = [
    ("roadsandrails.fxsxml", "road"),
    ("roads_rails.fxsxml", "road"),
    ("bridges.fxsxml", "bridge"),
];

/// The road model the original lays (`FUN_088402e6` loads `RoadsandRails_new.gr2` of `Dlc/Shared/Decals`
/// with the road material `RoadsandRails_new_D/H/SREF.dds`): it has no `.fxsxml`, so its asset list is
/// made up from the file names. It comes before the others in the records, so the client takes its
/// pieces (`Road_reg`, `RR_reg`) first; an install without it keeps the older models.
// re: re-b13-answers.txt 1d (road material of RoadsandRails_new.gr2, albedo _D.dds, height _H.dds)
pub const NEW_ROUTE_MODEL: (&str, &str, &str) =
    ("roadsandrails_new.fxsxml", "road", "roadsandrails_new.gr2");

/// The asset list of [`NEW_ROUTE_MODEL`].
pub fn new_route_asset_xml(gr2: &str) -> String {
    format!("<Asset><Mesh file=\"{gr2}\"/></Asset>")
}

/// The path models: flat name, record kind and variant.
pub const PATH_MODELS: [(&str, &str, &str); 24] = [
    ("centerattack.fxsxml", "path_center", "Attack"),
    ("centerembark.fxsxml", "path_center", "Embark"),
    ("centerinaccessible.fxsxml", "path_center", "Inaccessible"),
    ("centeroneturn.fxsxml", "path_center", "OneTurn"),
    ("centerthreeturns.fxsxml", "path_center", "ThreeTurns"),
    ("centertwoturns.fxsxml", "path_center", "TwoTurns"),
    ("connectorattack.fxsxml", "path_connector", "Attack"),
    ("connectorembark.fxsxml", "path_connector", "Embark"),
    (
        "connectorinaccessible.fxsxml",
        "path_connector",
        "Inaccessible",
    ),
    ("connectoroneturn.fxsxml", "path_connector", "OneTurn"),
    ("connectorthreeturns.fxsxml", "path_connector", "ThreeTurns"),
    ("connectortwoturns.fxsxml", "path_connector", "TwoTurns"),
    ("flanked00.fxsxml", "path_flank", "00"),
    ("flanked01.fxsxml", "path_flank", "01"),
    ("flanked02.fxsxml", "path_flank", "02"),
    ("flanked03.fxsxml", "path_flank", "03"),
    ("flanked04.fxsxml", "path_flank", "04"),
    ("flanked05.fxsxml", "path_flank", "05"),
    ("selectedaniattack.fxsxml", "path_selected", "Attack"),
    ("selectedaniembark.fxsxml", "path_selected", "Embark"),
    (
        "selectedaniinaccessible.fxsxml",
        "path_selected",
        "Inaccessible",
    ),
    ("selectedanioneturn.fxsxml", "path_selected", "OneTurn"),
    (
        "selectedanithreeturns.fxsxml",
        "path_selected",
        "ThreeTurns",
    ),
    ("selectedanitwoturns.fxsxml", "path_selected", "TwoTurns"),
];

// SIMPLIFIED(SIMP-exe-1034): the six textures of the beam come with the model (base_01/02, UV_01/02/03, alpha_01).
/// The beam over the selected unit (`assets/Effects/Level_Up/FXM_God_Ray.fxsxml`), by flat name.
pub const GOD_RAY_MODEL: &str = "fxm_god_ray.fxsxml";

/// The ring and highlight textures: record name, kind and files.
pub const TEXTURE_OVERLAYS: [(&str, &str, &[&str]); 2] = [
    (
        "ring",
        "ring",
        &["gui_unitselectring01.dds", "gui_unitselectringglow01.dds"],
    ),
    (
        "highlight",
        "highlight",
        &[
            "gui_unitselectedhexglow_01.dds",
            "static_texture_highlight.dds",
        ],
    ),
];

// ---------------------------------------------------------------- the plan

/// A tree record before the models are known to exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRecord {
    pub feature: &'static str,
    pub style: u8,
    /// File stem of the model: `forest_europe`.
    pub stem: String,
    /// Overlay texture file: `forest_overlay_europe.dds`.
    pub overlay: String,
}

/// Everything the set builds, decided from the install.
#[derive(Debug, Default)]
pub struct Plan {
    pub trees: Vec<TreeRecord>,
    /// Styles that took the Europe files.
    pub fallbacks: Vec<String>,
    /// Road models found in the install: flat name and class.
    pub routes: Vec<(String, &'static str)>,
    /// Path models found in the install.
    pub paths: Vec<(String, &'static str, &'static str)>,
    /// Textures of the ring and the highlights found in the install.
    pub texture_overlays: Vec<(&'static str, &'static str, Vec<String>)>,
    /// The install has the effect model of the god ray ([`GOD_RAY_MODEL`]); it is not required.
    pub god_ray: bool,
    /// What was expected and is not in the install; a name here fails the set.
    pub missing: Vec<String>,
}

/// The asset list of a tree model.
pub fn tree_asset_xml(stem: &str) -> String {
    format!("<Asset><Mesh file=\"{stem}.gr2\"/><Texture file=\"{stem}.dds\"/></Asset>")
}

/// Plans the set. `has` tells whether the install has a flat file name.
pub fn plan(has: &dyn Fn(&str) -> bool) -> Result<Plan, String> {
    let mut p = Plan::default();
    let trees = plan_trees(has, MASKS)?;
    p.fallbacks = trees.fallbacks;
    for r in trees.records {
        p.trees.push(TreeRecord {
            feature: r.kind.feature,
            style: r.style,
            stem: r.stem,
            overlay: r.overlay,
        });
    }
    for i in 1..=MASKS {
        let f = format!("forest_tile{i}.dds");
        if !has(&f) {
            p.missing.push(f);
        }
    }
    // Optional: an install without the file keeps the older road models.
    if has(NEW_ROUTE_MODEL.2) {
        p.routes
            .push((NEW_ROUTE_MODEL.0.to_owned(), NEW_ROUTE_MODEL.1));
    }
    for (flat, class) in ROUTE_MODELS {
        if has(flat) {
            p.routes.push((flat.to_owned(), class));
        } else {
            p.missing.push(flat.to_owned());
        }
    }
    for (flat, kind, variant) in PATH_MODELS {
        if has(flat) {
            p.paths.push((flat.to_owned(), kind, variant));
        } else {
            p.missing.push(flat.to_owned());
        }
    }
    p.god_ray = has(GOD_RAY_MODEL);
    for (name, kind, files) in TEXTURE_OVERLAYS {
        for f in files {
            if !has(f) {
                p.missing.push((*f).to_owned());
            }
        }
        p.texture_overlays
            .push((name, kind, files.iter().map(|f| (*f).to_owned()).collect()));
    }
    Ok(p)
}

/// The tree models to build, each once; a tree model keeps its parts (a variant per part).
pub fn model_jobs(p: &Plan) -> Vec<ModelJob> {
    let mut jobs: BTreeMap<String, ModelJob> = BTreeMap::new();
    for t in &p.trees {
        let flat = format!("{}.fxsxml", t.stem);
        jobs.entry(flat.clone()).or_insert(ModelJob {
            flat,
            kind: ModelKind::Feature,
            scope: MergeScope::Part,
            xml: Some(tree_asset_xml(&t.stem)),
            diffuse: Some(format!("{}.dds", t.stem)),
        });
    }
    for (flat, _) in &p.routes {
        let xml = (flat == NEW_ROUTE_MODEL.0).then(|| new_route_asset_xml(NEW_ROUTE_MODEL.2));
        jobs.insert(
            flat.clone(),
            ModelJob {
                flat: flat.clone(),
                kind: ModelKind::Feature,
                scope: MergeScope::Part,
                xml,
                diffuse: None,
            },
        );
    }
    for (flat, _, _) in &p.paths {
        jobs.insert(
            flat.clone(),
            ModelJob {
                flat: flat.clone(),
                kind: ModelKind::Effect,
                scope: MergeScope::Model,
                xml: None,
                diffuse: None,
            },
        );
    }
    if p.god_ray {
        jobs.insert(
            GOD_RAY_MODEL.to_owned(),
            ModelJob {
                flat: GOD_RAY_MODEL.to_owned(),
                kind: ModelKind::Effect,
                scope: MergeScope::Model,
                xml: None,
                diffuse: None,
            },
        );
    }
    jobs.into_values().collect()
}

/// Texture files the set adds on its own: masks, overlays and the ring and highlight textures.
pub fn texture_files(p: &Plan) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = (1..=MASKS).map(|i| format!("forest_tile{i}.dds")).collect();
    out.extend(p.trees.iter().map(|t| t.overlay.clone()));
    for (_, _, files) in &p.texture_overlays {
        out.extend(files.iter().cloned());
    }
    out
}

/// The `Routes.Type` the road models belong to (all of them until RW-17 answers).
pub fn route_records(routes: &[String], models: &[(String, &'static str)]) -> Vec<RouteArt> {
    routes
        .iter()
        .map(|r| RouteArt {
            route: r.clone(),
            models: models
                .iter()
                .map(|(flat, class)| RouteModel {
                    class: (*class).to_owned(),
                    model: md_id(flat),
                })
                .collect(),
        })
        .collect()
}

/// The records of the set; a model that was not built leaves its record out (named in the result).
pub fn apply(
    p: &Plan,
    routes: &[String],
    built: &BTreeSet<String>,
    table: &mut ArtTable,
) -> Vec<String> {
    let mut missing = Vec::new();
    let masks: Vec<LogicalId> = (1..=MASKS)
        .map(|i| tx_id(&format!("forest_tile{i}.dds")))
        .collect();
    for t in &p.trees {
        let flat = format!("{}.fxsxml", t.stem);
        if !built.contains(&flat) {
            missing.push(format!(
                "tree {}/{}: {flat} was not built",
                t.feature, t.style
            ));
            continue;
        }
        table.insert_tree(TreeArt {
            feature: t.feature.to_owned(),
            style: t.style,
            model: md_id(&flat),
            overlay: tx_id(&t.overlay),
            masks: masks.clone(),
        });
    }
    let have: Vec<(String, &'static str)> = p
        .routes
        .iter()
        .filter(|(f, _)| built.contains(f))
        .cloned()
        .collect();
    for (f, _) in p.routes.iter().filter(|(f, _)| !built.contains(f)) {
        missing.push(format!("routes: {f} was not built"));
    }
    for r in route_records(routes, &have) {
        table.insert_route(r);
    }
    for (flat, kind, variant) in &p.paths {
        if !built.contains(flat) {
            missing.push(format!("path model {flat} was not built"));
            continue;
        }
        let name = flat.trim_end_matches(".fxsxml").to_owned();
        table.insert_overlay(OverlayArt {
            name,
            kind: (*kind).to_owned(),
            variant: (*variant).to_owned(),
            model: Some(md_id(flat)),
            textures: Vec::new(),
        });
    }
    for (name, kind, files) in &p.texture_overlays {
        table.insert_overlay(OverlayArt {
            name: (*name).to_owned(),
            kind: (*kind).to_owned(),
            variant: String::new(),
            model: None,
            textures: files.iter().map(|f| tx_id(f)).collect(),
        });
    }
    missing
}

/// The kinds of the parts of a road model, for the `route-unassigned` report: `Road_*`, `RR_*`
/// and bridge parts by the first letters of their names.
pub fn part_kinds(names: &[String]) -> String {
    let mut n: BTreeMap<&str, usize> = BTreeMap::new();
    for name in names {
        let l = name.to_ascii_lowercase();
        let k = if l.starts_with("rr_") {
            "rail"
        } else if l.starts_with("road_") {
            "road"
        } else if l.contains("bridge") {
            "bridge"
        } else {
            "other"
        };
        *n.entry(k).or_default() += 1;
    }
    n.iter()
        .map(|(k, v)| format!("{v} {k}"))
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------- the build

#[derive(Debug)]
pub struct MiscSummary {
    pub tree_records: usize,
    pub tree_models: usize,
    pub route_records: usize,
    pub route_models: usize,
    pub path_models: usize,
    pub overlay_records: usize,
    pub models: usize,
    pub failed_models: usize,
    /// Records left out because their model was not built, and expected files the install lacks.
    pub missing: Vec<String>,
    pub textures: usize,
    pub substitutes: Vec<String>,
    pub model_bytes: u64,
    pub texture_bytes: BTreeMap<VariantFormat, u64>,
    pub table_bytes: u64,
    pub meshes: u64,
    pub triangles: u64,
    pub anims: AnimSummary,
    pub report: SetReport,
    pub stats: StoreStats,
    pub seconds: f64,
}

impl MiscSummary {
    pub fn failures(&self) -> usize {
        self.failed_models + self.missing.len()
    }

    pub fn total_bytes(&self) -> u64 {
        self.model_bytes
            + self.anims.bytes
            + self.anims.fsm.bytes
            + self.texture_bytes.values().sum::<u64>()
            + self.table_bytes
    }
}

/// The `Routes.Type` of the snapshot.
pub fn read_routes(snapshot: &Path) -> Result<Vec<String>, DbError> {
    let d = db::open_snapshot(snapshot)?;
    db::routes(&d)
}

/// Builds the set into `out`.
pub fn build_misc(
    game: &GameSource,
    snapshot: &Path,
    out: &Path,
    opts: &Options,
) -> Result<MiscSummary, ModelError> {
    let started = Instant::now();
    let routes = read_routes(snapshot)
        .map_err(|e| ModelError::invalid(format!("reading {}: {e}", snapshot.display())))?;
    let has = |n: &str| game.lookup(n).is_some();
    let p = plan(&has).map_err(ModelError::invalid)?;

    let store = BlobStore::open(out)?;
    let mut manifest = ManifestBuilder::open(&store)?;
    let jobs = model_jobs(&p);
    let BuiltModels {
        good,
        failed,
        mut entries,
        texture_files: model_textures,
        stub_ids,
        mut texture_bytes,
        model_bytes,
        meshes,
        triangles,
        anims,
        ..
    } = build_models(game, &jobs, &store, &mut manifest, opts, GROUP)?;

    // The textures of the set itself: the original's bytes (L8 masks as R8).
    let mut own_missing: Vec<String> = p.missing.clone();
    let own = texture_files(&p);
    for f in &own {
        if model_textures.contains(f) {
            continue;
        }
        let Some(bytes) = game.read_by_name(f)? else {
            own_missing.push(format!("{f} is missing from the install"));
            continue;
        };
        let blob = dds_passthrough(&bytes).map_err(|e| ModelError::invalid(format!("{f}: {e}")))?;
        let (hash, size) = store.put(&blob.encode())?;
        let format = variant_format(blob.format);
        *texture_bytes.entry(format).or_default() += size;
        manifest.add(
            tx_id(f),
            Variant {
                format,
                hash,
                size,
                prefetch: GROUP,
            },
        );
    }

    let built: BTreeSet<String> = good.iter().map(|(l, _)| l.flat.clone()).collect();
    let mut table = load_table(&store, &manifest)?;
    let mut missing = apply(&p, &routes, &built, &mut table);
    for r in &routes {
        if table.route(r).is_none() {
            missing.push(format!("{r}: no record"));
        }
    }
    missing.extend(own_missing);
    let table_bytes = store_table(&store, &mut manifest, &table)?;
    manifest.finish(&store)?;

    // The report.
    for (f, why) in &failed {
        entries.push(ReportEntry::new(
            "misc-model-failed",
            f.clone(),
            why.clone(),
        ));
    }
    for m in &missing {
        entries.push(ReportEntry::new("record-left-out", SET_NAME, m.clone()));
    }
    for fb in &p.fallbacks {
        entries.push(ReportEntry::new("tree-fallback", SET_NAME, fb.clone()));
    }
    for (l, b) in &good {
        if p.routes.iter().any(|(f, _)| *f == l.flat) {
            let names: Vec<String> = b.model.parts.iter().map(|x| x.name.clone()).collect();
            entries.push(ReportEntry::new(
                "route-unassigned",
                l.flat.clone(),
                format!(
                    "{} part(s) ({}): which link uses which is RW-17's; in the record of every route",
                    names.len(),
                    part_kinds(&names)
                ),
            ));
        }
    }
    let mut report = SetReport {
        entries,
        ..SetReport::default()
    };
    let tree_models: BTreeSet<&str> = p.trees.iter().map(|t| t.stem.as_str()).collect();
    for (k, v) in [
        ("tree records", p.trees.len()),
        ("tree models", tree_models.len()),
        ("route records", routes.len()),
        ("route models", p.routes.len()),
        ("path models", p.paths.len()),
        ("overlay records", p.paths.len() + p.texture_overlays.len()),
        ("models", good.len()),
        ("failed models", failed.len()),
        ("meshes", meshes as usize),
        ("triangles", triangles as usize),
        ("textures", own.len() + model_textures.len()),
        ("clip packs", anims.packs),
        ("clips", anims.clips),
    ] {
        report.counts.insert(k.to_owned(), v as u64);
    }
    report.bytes.insert("models".to_owned(), model_bytes);
    for (f, b) in &texture_bytes {
        report.bytes.insert(format!("textures {f:?}"), *b);
    }
    report.bytes.insert("table".to_owned(), table_bytes);
    report.bytes.insert("clip packs".to_owned(), anims.bytes);
    Ok(MiscSummary {
        tree_records: p.trees.len(),
        tree_models: tree_models.len(),
        route_records: routes.len(),
        route_models: p.routes.len(),
        path_models: p.paths.len(),
        overlay_records: p.paths.len() + p.texture_overlays.len(),
        models: good.len(),
        failed_models: failed.len(),
        missing,
        textures: own.len() + model_textures.len(),
        substitutes: stub_ids,
        model_bytes,
        texture_bytes,
        table_bytes,
        meshes,
        triangles,
        anims,
        report,
        stats: store.stats(),
        seconds: started.elapsed().as_secs_f64(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::trees::{KINDS, STYLE_NAMES};
    use super::*;

    fn all(_: &str) -> bool {
        true
    }

    /// An install without the `*_2` tree files.
    fn plain(n: &str) -> bool {
        !n.contains("_2.")
    }

    #[test]
    fn the_plan_has_a_tree_record_per_feature_and_style_and_every_path_model() {
        let p = plan(&plain).unwrap();
        assert_eq!(p.trees.len(), KINDS.len() * STYLE_NAMES.len());
        assert_eq!(p.paths.len(), 24);
        assert_eq!(p.routes.len(), 4);
        assert!(p.missing.is_empty(), "{:?}", p.missing);
        assert_eq!(p.texture_overlays.len(), 2);
        let jobs = model_jobs(&p);
        // 5 forest + 5 jungle styles share files: the Generic jungle is not a model of its own only
        // when its files are missing; with every file present all ten are models.
        assert_eq!(jobs.len(), 10 + 4 + 24 + 1);
        let tree = jobs
            .iter()
            .find(|j| j.flat == "forest_europe.fxsxml")
            .unwrap();
        assert_eq!(tree.scope, MergeScope::Part);
        assert!(tree.xml.as_deref().unwrap().contains("forest_europe.gr2"));
        let path = jobs.iter().find(|j| j.flat == "flanked00.fxsxml").unwrap();
        assert_eq!(
            (path.kind, path.scope),
            (ModelKind::Effect, MergeScope::Model)
        );
    }

    /// With the `*_2` files the trees are those, and the diffuse of each is its own texture (the
    /// gr2 of the re-export names another style's).
    #[test]
    fn the_second_tree_files_are_built_with_their_own_texture() {
        let p = plan(&all).unwrap();
        let jobs = model_jobs(&p);
        let tree = jobs
            .iter()
            .find(|j| j.flat == "forest_asia_2.fxsxml")
            .unwrap();
        assert!(tree.xml.as_deref().unwrap().contains("forest_asia_2.gr2"));
        assert_eq!(tree.diffuse.as_deref(), Some("forest_asia_2.dds"));
        assert!(jobs.iter().all(|j| j.flat != "forest_asia.fxsxml"));
        let route = jobs.iter().find(|j| j.flat == "bridges.fxsxml").unwrap();
        assert_eq!(route.diffuse, None);
    }

    /// The road model of the original has no `.fxsxml`: it is built from a made-up asset list, comes
    /// first in the records, and an install without it is not an error.
    #[test]
    fn the_new_road_model_comes_first_and_an_install_without_it_keeps_the_others() {
        let p = plan(&plain).unwrap();
        assert_eq!(p.routes[0].0, "roadsandrails_new.fxsxml");
        let job = model_jobs(&p)
            .into_iter()
            .find(|j| j.flat == "roadsandrails_new.fxsxml")
            .unwrap();
        assert_eq!(job.scope, MergeScope::Part);
        assert!(
            job.xml
                .as_deref()
                .unwrap()
                .contains("roadsandrails_new.gr2")
        );
        let without = |n: &str| plain(n) && n != "roadsandrails_new.gr2";
        let p = plan(&without).unwrap();
        assert_eq!(p.routes.len(), 3);
        assert!(p.missing.is_empty(), "{:?}", p.missing);
        let records = route_records(&["ROUTE_ROAD".to_owned()], &plan(&plain).unwrap().routes);
        assert_eq!(
            records[0].models[0].model,
            md_id("roadsandrails_new.fxsxml")
        );
    }

    #[test]
    fn a_style_without_files_takes_europe_and_a_missing_file_is_named() {
        let has = |n: &str| plain(n) && !n.contains("jungle_generic") && n != "bridges.fxsxml";
        let p = plan(&has).unwrap();
        let g = p
            .trees
            .iter()
            .find(|t| t.feature == "FEATURE_JUNGLE" && t.style == 0)
            .unwrap();
        assert_eq!(g.stem, "jungle_europe");
        assert_eq!(p.fallbacks.len(), 1);
        assert_eq!(p.missing, ["bridges.fxsxml"]);
        // Two records share one model: it is built once.
        let jobs = model_jobs(&p);
        assert_eq!(
            jobs.iter()
                .filter(|j| j.flat == "jungle_europe.fxsxml")
                .count(),
            1
        );
    }

    #[test]
    fn records_are_written_for_built_models_only() {
        let p = plan(&plain).unwrap();
        let mut built: BTreeSet<String> = model_jobs(&p).into_iter().map(|j| j.flat).collect();
        built.remove("forest_asia.fxsxml");
        built.remove("centerattack.fxsxml");
        let mut t = ArtTable::new();
        let routes = vec!["ROUTE_ROAD".to_owned(), "ROUTE_RAILROAD".to_owned()];
        let missing = apply(&p, &routes, &built, &mut t);
        assert_eq!(missing.len(), 2, "{missing:?}");
        assert!(t.tree("FEATURE_FOREST", 2).is_none());
        let e = t.tree("FEATURE_FOREST", 4).unwrap();
        assert_eq!(e.model, md_id("forest_europe.fxsxml"));
        assert_eq!(e.masks.len(), 4);
        assert_eq!(e.overlay.as_str(), "tx:forest_overlay_europe.dds");
        // Every route lists every road model with its class.
        for r in &routes {
            let rec = t.route(r).unwrap();
            assert_eq!(rec.models.len(), 4);
            assert_eq!(rec.models[0].class, "road");
            assert_eq!(rec.models[3].class, "bridge");
        }
        assert!(t.overlay("centerattack").is_none());
        let o = t.overlay("centeroneturn").unwrap();
        assert_eq!(
            (o.kind.as_str(), o.variant.as_str()),
            ("path_center", "OneTurn")
        );
        let ring = t.overlay("ring").unwrap();
        assert!(ring.model.is_none() && ring.textures.len() == 2);
    }

    #[test]
    fn the_texture_list_has_masks_overlays_and_the_ring() {
        let p = plan(&plain).unwrap();
        let f = texture_files(&p);
        for need in [
            "forest_tile1.dds",
            "forest_tile4.dds",
            "forest_overlay_generic.dds",
            "jungle_overlay_europe.dds",
            "gui_unitselectring01.dds",
            "static_texture_highlight.dds",
        ] {
            assert!(f.contains(need), "{need}");
        }
    }

    #[test]
    fn the_god_ray_is_an_optional_effect_model_without_a_record() {
        let p = plan(&plain).unwrap();
        assert!(p.god_ray);
        let jobs = model_jobs(&p);
        let ray = jobs.iter().find(|j| j.flat == GOD_RAY_MODEL).unwrap();
        assert_eq!(
            (ray.kind, ray.scope),
            (ModelKind::Effect, MergeScope::Model)
        );
        assert!(ray.xml.is_none() && ray.diffuse.is_none());
        // An install without the file builds the set without the beam, and the set still plans.
        let without = |n: &str| plain(n) && n != GOD_RAY_MODEL;
        let p = plan(&without).unwrap();
        assert!(!p.god_ray && p.missing.is_empty(), "{:?}", p.missing);
        assert!(model_jobs(&p).iter().all(|j| j.flat != GOD_RAY_MODEL));
        // Its model needs no record: `apply` writes the same records with and without it.
        let built: BTreeSet<String> = model_jobs(&p).into_iter().map(|j| j.flat).collect();
        let mut t = ArtTable::new();
        assert!(apply(&p, &["ROUTE_ROAD".to_owned()], &built, &mut t).is_empty());
    }

    #[test]
    fn part_kinds_count_roads_rails_and_bridges() {
        let names: Vec<String> = ["Road_PL1", "Road_HB1", "RR_reg", "ANC_Bridge_Mid", "Box"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert_eq!(part_kinds(&names), "1 bridge, 1 other, 1 rail, 2 road");
    }
}
