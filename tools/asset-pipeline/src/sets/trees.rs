//! Set `b1-trees`: the tree models of forests and jungles per continent
//! style, their textures, and the art table records `feature/FEATURE_FOREST/<0-4>` and
//! `feature/FEATURE_JUNGLE/<0-4>`. This set also owns the placement parameters of `forests.xml`
//! and the `formation/*` records of `UnitFormations.xml` (see [`super::formations`]).
//!
//! # Files (checked against the install)
//! - `forest_<style>.gr2` for `generic america asia africa europe`: 12 rigid models of one small
//!   mesh each (a few triangles, 6 to 7 units tall), all at the origin; they are the tree
//!   variants the placement draws from. `jungle_<style>.gr2` for `america asia africa europe`:
//!   17 variants. There is no `jungle_generic`: the Generic style takes the Europe jungle.
//! - `forest_<style>.dds` / `jungle_<style>.dds`: the tree texture (the model's only material).
//! - `forest_overlay_<style>.dds` / `jungle_overlay_<style>.dds`: the ground overlay under the
//!   trees (`overlay_alpha` of `forests.xml`); it is `FeatureArt::texture`.
//! - `forest_tile<i>.dds`, `i` in `1..=num_tiles`: L8 placement masks (RV-9).
//!
//! A tree model is the whole file: one engine mesh per variant, variant `i` is mesh `i`.
//!
//! - `forest_<style>_2.gr2` / `jungle_<style>_2.gr2` (a re-export in the DLC layer) with their
//!   `*_2.dds` textures and `*_overlay_<style>_2.dds` overlays: the set the game draws (the tree
//!   draws of the captures S1a and S2 have the vertices of `forest_asia_2.gr2`: bigger trees on a
//!   4x4 atlas, and DXT3 textures, darker than the plain DXT5 ones). It wins where it is complete;
//!   Generic has none and keeps the plain files.
//!
//! SIMPLIFIED(SIMP-009): the parameters of `forests.xml` are passed on, the placement is the
//! client's own.

use std::collections::BTreeMap;
use std::path::Path;

use asset_format::{
    ArtTable, FeatureArt, LogicalId, Material, ModelBlob, PrefetchGroup, Skeleton, VariantFormat,
};
use granny::{GrannyFile, Scene};

use super::formations;
use super::terrain::{ReadFile, STYLES, group_of_style};
use super::{
    TextureOptions, add_model, add_textures, cached_model, load_art_table, store_art_table,
};
use crate::game::GameSource;
use crate::models::set::{model_id, rgba_texture, texture_id};
use crate::models::{Basis, ModelError, ModelMaterials, convert_model, write_glb};
use crate::store::{BlobStore, BuildCache, CacheKey, ManifestBuilder, StoreStats};

pub const SET_NAME: &str = "b1-trees";

/// File names of the two parameter files, flat.
pub const UNIT_FORMATIONS_FILE: &str = "unitformations.xml";
pub const FORESTS_FILE: &str = "forests.xml";

/// Style number (`Q_PLOTS.continent_art`) to the name used in the file names.
pub const STYLE_NAMES: [&str; 5] = ["generic", "america", "asia", "africa", "europe"];

/// The style that stands in for one without files of its own.
const FALLBACK_STYLE: u8 = 4;

/// A feature that is drawn with trees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeKind {
    pub feature: &'static str,
    /// File name prefix.
    pub prefix: &'static str,
}

pub const KINDS: [TreeKind; 2] = [
    TreeKind {
        feature: "FEATURE_FOREST",
        prefix: "forest",
    },
    TreeKind {
        feature: "FEATURE_JUNGLE",
        prefix: "jungle",
    },
];

/// One `feature/<feature>/<style>` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRecord {
    pub kind: TreeKind,
    pub style: u8,
    /// File stem of the model and of its texture: `forest_europe`.
    pub stem: String,
    /// Overlay texture file: `forest_overlay_europe.dds`.
    pub overlay: String,
}

/// Everything the set puts into the table and the manifest, before any conversion.
#[derive(Debug, Default)]
pub struct TreePlan {
    pub records: Vec<TreeRecord>,
    /// Model file stem to the prefetch group (the lowest of its users).
    pub models: BTreeMap<String, PrefetchGroup>,
    /// Lowercase texture file to the prefetch group.
    pub textures: BTreeMap<String, PrefetchGroup>,
    /// "jungle style 0: took europe" notes.
    pub fallbacks: Vec<String>,
}

fn stem_of(kind: TreeKind, style: u8) -> String {
    format!("{}_{}", kind.prefix, STYLE_NAMES[usize::from(style)])
}

fn overlay_of(kind: TreeKind, style: u8) -> String {
    format!(
        "{}_overlay_{}.dds",
        kind.prefix,
        STYLE_NAMES[usize::from(style)]
    )
}

/// The files of a style: the `*_2` set when the install has all of it (the game draws those: the
/// tree draws of the captures S1a and S2 have the vertices of `forest_asia_2.gr2` and the DXT3
/// textures of `forest_asia_2.dds` and `forest_overlay_asia_2.dds`), else the plain set; `None`
/// when neither is complete. The model and its texture share the stem.
fn files_of(has: &dyn Fn(&str) -> bool, kind: TreeKind, style: u8) -> Option<(String, String)> {
    let stem = stem_of(kind, style);
    let overlay = overlay_of(kind, style);
    let overlay_2 = overlay.replace(".dds", "_2.dds");
    [(format!("{stem}_2"), overlay_2), (stem, overlay)]
        .into_iter()
        .find(|(s, o)| has(&format!("{s}.gr2")) && has(&format!("{s}.dds")) && has(o))
}

fn use_group(map: &mut BTreeMap<String, PrefetchGroup>, key: &str, group: PrefetchGroup) {
    let g = map.entry(key.to_ascii_lowercase()).or_insert(group);
    *g = (*g).min(group);
}

/// Plans the records. `has` tells whether the install has a flat file name. A style with a file
/// missing takes the Europe one; if that is missing too, it is an error. `num_tiles` is the number
/// of placement masks.
pub fn plan_trees(has: &dyn Fn(&str) -> bool, num_tiles: u32) -> Result<TreePlan, String> {
    let mut plan = TreePlan::default();
    for kind in KINDS {
        for style in STYLES {
            let (stem, overlay) = if let Some(f) = files_of(has, kind, style) {
                f
            } else if let Some(f) = files_of(has, kind, FALLBACK_STYLE) {
                plan.fallbacks.push(format!(
                    "{} style {style}: took {} (no complete {} file set)",
                    kind.prefix,
                    STYLE_NAMES[usize::from(FALLBACK_STYLE)],
                    stem_of(kind, style)
                ));
                f
            } else {
                return Err(format!(
                    "{}: neither {} nor {} has gr2, texture and overlay in the install",
                    kind.prefix,
                    stem_of(kind, style),
                    stem_of(kind, FALLBACK_STYLE)
                ));
            };
            let group = group_of_style(style);
            use_group(&mut plan.models, &stem, group);
            use_group(&mut plan.textures, &format!("{stem}.dds"), group);
            use_group(&mut plan.textures, &overlay, group);
            plan.records.push(TreeRecord {
                kind,
                style,
                stem,
                overlay,
            });
        }
    }
    for i in 1..=num_tiles {
        use_group(
            &mut plan.textures,
            &format!("forest_tile{i}.dds"),
            PrefetchGroup(1),
        );
    }
    Ok(plan)
}

impl TreePlan {
    /// Adds the feature records. The record keeps nothing from an earlier one: this set owns the
    /// tree model, the overlay and the placement of forests and jungles.
    pub fn apply(&self, table: &mut ArtTable) {
        for r in &self.records {
            table.insert_feature(
                r.kind.feature,
                r.style,
                FeatureArt {
                    texture: Some(texture_id(&r.overlay)),
                    tree_model: Some(model_id(&r.stem)),
                    placement: BTreeMap::new(),
                },
            );
        }
    }
}

/// The tree model of a gr2: every variant a mesh at the origin, the tree texture as the only
/// material, no skeleton.
fn tree_model(gr2: &[u8], what: &str, diffuse: &str) -> Result<ModelBlob, ModelError> {
    let granny_err = |source| ModelError::Granny {
        what: what.to_owned(),
        source,
    };
    let file = GrannyFile::parse(gr2).map_err(granny_err)?;
    let scene = Scene::read(&file).map_err(granny_err)?;
    let materials = ModelMaterials::single(Material {
        diffuse: Some(texture_id(diffuse)),
        specular: None,
    });
    let converted = convert_model(&scene, &Basis::CIV5_TO_ENGINE, &materials)?;
    let mut model = converted.model;
    if model.meshes.is_empty() {
        return Err(ModelError::invalid(format!("{what}: no meshes")));
    }
    model.skeleton = Skeleton::default();
    Ok(model)
}

/// What a build of the set produced.
#[derive(Debug)]
pub struct TreesSummary {
    pub models: usize,
    pub textures: usize,
    pub bytes: BTreeMap<VariantFormat, u64>,
    pub model_bytes: u64,
    pub feature_records: usize,
    pub formation_records: usize,
    pub forest_params: BTreeMap<String, f32>,
    pub table_entries: usize,
    pub table_bytes: u64,
    pub fallbacks: Vec<String>,
    pub stats: StoreStats,
}

impl TreesSummary {
    pub fn total_bytes(&self) -> u64 {
        self.bytes.values().sum::<u64>() + self.model_bytes + self.table_bytes
    }
}

fn read_text(read: ReadFile<'_>, name: &str) -> Result<String, ModelError> {
    let bytes = read(name)?
        .ok_or_else(|| ModelError::invalid(format!("{name} is missing from the install")))?;
    Ok(String::from_utf8_lossy(&bytes)
        .trim_start_matches('\u{feff}')
        .to_owned())
}

/// Builds the set into `out`: models, textures and table records, added to the manifest already
/// there.
pub fn build_trees(
    read: ReadFile<'_>,
    out: &Path,
    opts: TextureOptions,
) -> Result<TreesSummary, ModelError> {
    let formation_rows = formations::parse_unit_formations(&read_text(read, UNIT_FORMATIONS_FILE)?)
        .map_err(ModelError::invalid)?;
    let params =
        formations::parse_forests(&read_text(read, FORESTS_FILE)?).map_err(ModelError::invalid)?;
    let num_tiles = params["num_tiles"];
    if num_tiles < 0.0 || num_tiles.fract() != 0.0 {
        return Err(ModelError::invalid(format!(
            "forests.xml: num_tiles {num_tiles} is not a count"
        )));
    }
    let plan = plan_trees(&|n| matches!(read(n), Ok(Some(_))), num_tiles as u32)
        .map_err(ModelError::invalid)?;

    let store = BlobStore::open(out)?;
    let cache = BuildCache::open(&store)?;
    let mut manifest = ManifestBuilder::open(&store)?;

    let bytes = add_textures(&cache, &store, &mut manifest, read, &plan.textures, opts)?;

    let mut model_bytes = 0;
    for (stem, group) in &plan.models {
        let gr2_name = format!("{stem}.gr2");
        let gr2 = read(&gr2_name)?.ok_or_else(|| {
            ModelError::invalid(format!("{gr2_name} is missing from the install"))
        })?;
        let diffuse = format!("{stem}.dds");
        let key = CacheKey::new(
            "tree-model-b1",
            super::MODEL_STEP_VERSION,
            &[asset_format::ContentHash(*blake3::hash(&gr2).as_bytes())],
            diffuse.as_bytes(),
        );
        let blob = cached_model(&cache, &store, &key, || {
            tree_model(&gr2, &gr2_name, &diffuse)
        })?;
        model_bytes += blob.1;
        add_model(&mut manifest, model_id(stem), blob, *group);
    }

    let mut table = load_art_table(&store, &manifest)?;
    plan.apply(&mut table);
    formations::apply(&mut table, &formation_rows, &params);
    let table_bytes = store_art_table(&store, &mut manifest, &table)?;
    manifest.finish(&store)?;
    Ok(TreesSummary {
        models: plan.models.len(),
        textures: plan.textures.len(),
        bytes,
        model_bytes,
        feature_records: plan.records.len(),
        formation_records: formation_rows.len(),
        forest_params: params,
        table_entries: table.len(),
        table_bytes,
        fallbacks: plan.fallbacks,
        stats: store.stats(),
    })
}

/// GLB bytes of the tree model of a record: `kind` is `forest` or `jungle`, `style` a name of
/// [`STYLE_NAMES`]. The tree texture is embedded at full size.
pub fn export_tree_glb(game: &GameSource, kind: &str, style: &str) -> Result<Vec<u8>, ModelError> {
    let kind = KINDS
        .iter()
        .find(|k| k.prefix.eq_ignore_ascii_case(kind))
        .ok_or_else(|| ModelError::invalid(format!("unknown tree kind {kind:?}")))?;
    let style = STYLE_NAMES
        .iter()
        .position(|n| n.eq_ignore_ascii_case(style))
        .ok_or_else(|| ModelError::invalid(format!("unknown tree style {style:?}")))?
        as u8;
    let has = |n: &str| game.lookup(n).is_some();
    let plan = plan_trees(&has, 0).map_err(ModelError::invalid)?;
    let rec = plan
        .records
        .iter()
        .find(|r| r.kind == *kind && r.style == style)
        .ok_or_else(|| ModelError::invalid("no such tree record"))?;
    let gr2_name = format!("{}.gr2", rec.stem);
    let gr2 = game
        .read_by_name(&gr2_name)?
        .ok_or_else(|| ModelError::invalid(format!("{gr2_name} is missing")))?;
    let diffuse = format!("{}.dds", rec.stem);
    let model = tree_model(&gr2, &gr2_name, &diffuse)?;
    let tex: Vec<(LogicalId, _)> = vec![(texture_id(&diffuse), rgba_texture(game, &diffuse)?)];
    write_glb(&model, &[], &tex, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sets::formations::FOREST_KEYS;
    use std::collections::BTreeSet;

    fn full_install() -> Vec<String> {
        let mut v = Vec::new();
        for kind in KINDS {
            for (style, name) in STYLE_NAMES.iter().enumerate() {
                if kind.prefix == "jungle" && style == 0 {
                    continue; // no jungle_generic in the game
                }
                v.push(format!("{}_{name}.gr2", kind.prefix));
                v.push(format!("{}_{name}.dds", kind.prefix));
                v.push(format!("{}_overlay_{name}.dds", kind.prefix));
            }
        }
        v
    }

    #[test]
    fn plans_ten_records_with_the_jungle_fallback() {
        let files: BTreeSet<String> = full_install().into_iter().collect();
        let plan = plan_trees(&|n| files.contains(n), 4).unwrap();
        assert_eq!(plan.records.len(), 10);
        let jungle0 = plan
            .records
            .iter()
            .find(|r| r.kind.prefix == "jungle" && r.style == 0)
            .unwrap();
        assert_eq!(jungle0.stem, "jungle_europe");
        assert_eq!(jungle0.overlay, "jungle_overlay_europe.dds");
        assert_eq!(plan.fallbacks.len(), 1, "{:?}", plan.fallbacks);
        // forest 5 + jungle 4 distinct models; the Generic jungle shares the Europe one.
        assert_eq!(plan.models.len(), 9);
        // 9 tree textures + 9 overlays + 4 masks.
        assert_eq!(plan.textures.len(), 22);
        assert!(plan.textures.contains_key("forest_tile4.dds"));
        assert!(!plan.textures.contains_key("forest_tile5.dds"));
    }

    #[test]
    fn model_groups_follow_the_style_and_the_lowest_user() {
        let files: BTreeSet<String> = full_install().into_iter().collect();
        let plan = plan_trees(&|n| files.contains(n), 4).unwrap();
        assert_eq!(plan.models["forest_europe"], PrefetchGroup(14));
        assert_eq!(plan.models["forest_generic"], PrefetchGroup(1));
        assert_eq!(plan.models["forest_america"], PrefetchGroup(11));
        // Used by Europe (14) and by the Generic fallback (1).
        assert_eq!(plan.models["jungle_europe"], PrefetchGroup(1));
        assert_eq!(plan.textures["jungle_overlay_europe.dds"], PrefetchGroup(1));
        assert_eq!(plan.textures["forest_tile1.dds"], PrefetchGroup(1));
    }

    #[test]
    fn a_style_without_files_and_without_europe_is_an_error() {
        let mut files: BTreeSet<String> = full_install().into_iter().collect();
        files.remove("jungle_europe.gr2");
        let e = plan_trees(&|n| files.contains(n), 4).unwrap_err();
        assert!(e.contains("jungle"), "{e}");
        // A missing overlay is as bad as a missing model.
        let mut files: BTreeSet<String> = full_install().into_iter().collect();
        files.remove("forest_overlay_asia.dds");
        let plan = plan_trees(&|n| files.contains(n), 4).unwrap();
        let asia = plan
            .records
            .iter()
            .find(|r| r.kind.prefix == "forest" && r.style == 2)
            .unwrap();
        assert_eq!(asia.stem, "forest_europe");
    }

    /// The `*_2` set is taken where it is complete (the game draws it, golden S1a and S2); an
    /// incomplete one leaves the plain files.
    #[test]
    fn the_second_set_wins_where_it_is_complete() {
        let mut files: BTreeSet<String> = full_install().into_iter().collect();
        for f in [
            "forest_asia_2.gr2",
            "forest_asia_2.dds",
            "forest_overlay_asia_2.dds",
            "jungle_asia_2.gr2",
            "jungle_asia_2.dds",
        ] {
            files.insert(f.to_owned());
        }
        let plan = plan_trees(&|n| files.contains(n), 4).unwrap();
        let rec = |prefix: &str, style: u8| {
            plan.records
                .iter()
                .find(|r| r.kind.prefix == prefix && r.style == style)
                .unwrap()
        };
        assert_eq!(rec("forest", 2).stem, "forest_asia_2");
        assert_eq!(rec("forest", 2).overlay, "forest_overlay_asia_2.dds");
        assert!(plan.textures.contains_key("forest_asia_2.dds"));
        // No `jungle_overlay_asia_2.dds`: the plain jungle stays.
        assert_eq!(rec("jungle", 2).stem, "jungle_asia");
        assert_eq!(rec("forest", 4).stem, "forest_europe");
    }

    #[test]
    fn apply_writes_texture_model_and_keeps_nothing_else() {
        let files: BTreeSet<String> = full_install().into_iter().collect();
        let plan = plan_trees(&|n| files.contains(n), 4).unwrap();
        let mut table = ArtTable::new();
        plan.apply(&mut table);
        let f = table.feature("FEATURE_FOREST", 4).unwrap();
        assert_eq!(f.tree_model, Some(model_id("forest_europe")));
        assert_eq!(f.texture, Some(texture_id("forest_overlay_europe.dds")));
        let j = table.feature("FEATURE_JUNGLE", 0).unwrap();
        assert_eq!(j.tree_model, Some(model_id("jungle_europe")));
        assert_eq!(table.len(), 10);
    }

    #[test]
    fn placement_parameters_merge_on_top() {
        let files: BTreeSet<String> = full_install().into_iter().collect();
        let plan = plan_trees(&|n| files.contains(n), 4).unwrap();
        let mut table = ArtTable::new();
        plan.apply(&mut table);
        let params: BTreeMap<String, f32> = FOREST_KEYS
            .iter()
            .enumerate()
            .map(|(i, k)| (k.to_string(), i as f32))
            .collect();
        formations::apply(&mut table, &[], &params);
        let f = table.feature("FEATURE_JUNGLE", 2).unwrap();
        assert_eq!(f.placement, params);
        assert_eq!(f.tree_model, Some(model_id("jungle_asia")));
    }
}
