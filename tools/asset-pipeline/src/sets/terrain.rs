//! Set `b1-terrain`: diffuse cell textures per continent style from
//! `terraindef.csv`, feature decals, ice and river textures, plus the art table records
//! `terrain/<TERRAIN_*>/<0-4>`, `feature/<FEATURE_*>/<0-4>` and `river`. The three fog cloud
//! textures of the fog of war and the water bump map are in the manifest without a record
//! ([`FOG_CLOUD_TEXTURES`], [`WATER_TEXTURES`]).
//!
//! SIMPLIFIED(SIMP-005): only the diffuse texture of a cell type; heightmaps, specular maps and
//! the terrain pieces are not converted.

use std::collections::BTreeMap;
use std::path::Path;

use asset_format::{
    ArtTable, FeatureArt, PrefetchGroup, Projection, RiverArt, TerrainArt, Variant, VariantFormat,
};
use rayon::prelude::*;

use super::{TextureOptions, convert_texture, load_art_table, store_art_table};
use crate::gamedata::{Db, GAMEPLAY_DB_FILE};
use crate::models::ModelError;
use crate::models::set::texture_id;
use crate::store::{BlobStore, BuildCache, ManifestBuilder, StoreStats};

pub const SET_NAME: &str = "b1-terrain";

/// File the game reads; the DLC copy wins in the virtual file system .
pub const TERRAIN_DEF: &str = "terraindef.csv";

/// Continent styles (`Q_PLOTS.continent_art`): 0 Generic, 1 America, 2 Asia, 3 Africa, 4 Europe.
pub const STYLES: std::ops::RangeInclusive<u8> = 0..=4;

/// Extra table key for the top layer of a mountain. `TerrainArt` carries one texture, and the
/// terrain render spec (4.3) uses `Mountain_Base` and `Mountain_Top` for mountains.
pub const MOUNTAIN_TOP: &str = "TERRAIN_MOUNTAIN_TOP";

/// Decal textures of features, same for every style.
pub const FEATURE_DECALS: &[(&str, &str)] = &[
    ("FEATURE_MARSH", "feature_marsh.dds"),
    ("FEATURE_FLOOD_PLAINS", "feature_flood_plains.dds"),
    ("FEATURE_OASIS", "feature_oasis.dds"),
    ("FEATURE_ICE", "feature_ice.dds"),
    ("FEATURE_FALLOUT", "feature_fallout.dds"),
];

/// Converted for the manifest without a table record: the ice surface texture.
pub const EXTRA_TEXTURES: &[&str] = &["ice_d.dds"];

/// The three cloud layers of the fog of war (`FluidFOWSystem`, RV-10), bottom layer first:
/// converted for the client's cloud pass, without a table record (the client asks for them by id).
pub const FOG_CLOUD_TEXTURES: &[&str] = &[
    "fogcloudtile_dif_02.dds",
    "fogcloudtile_dif_03.dds",
    "fogcloudtile_dif_01.dds",
];

/// The bump map of the water surface (`TerrainTextures.fpk`, 512x512 grey height noise). The
/// client's water pass asks for it by id (no art-table record). The colour ramp of the game's
/// `waterdepthcolor.dds` is not converted: `water.wgsl` has its values.
pub const WATER_TEXTURES: &[&str] = &["waterbumps.dds"];

pub const RIVER_TEXTURES: &[&str] = &["river_diffuse.dds", "river_alpha.dds"];

/// A row of `terraindef.csv`.
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainDefRow {
    pub continent: u8,
    pub diffuse: String,
    pub heightmap: String,
    pub specmap: String,
    pub base_dominance: f32,
    pub texture_scale: f32,
    /// 0 horizontal, 1 vertical.
    pub projection: i32,
    /// The "ID (unique for continent)" column, hexadecimal in the file (`%x`).
    pub id: u32,
}

/// Parses `terraindef.csv`: a header line, then `continent,diffuse,height,spec,float,scale,
/// projection,hexid`. The original reads the same fields with `%i %s %s %s %f %f %i %x`
/// (commas turned into blanks); the fifth, `BaseDominance`, it skips.
pub fn parse_terraindef(text: &str) -> Result<Vec<TerrainDefRow>, String> {
    let mut rows = Vec::new();
    for (n, line) in text.lines().enumerate().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let at = |what: &str| format!("{TERRAIN_DEF} line {}: {what}", n + 1);
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        if f.len() != 8 {
            return Err(at(&format!("expected 8 fields, found {}", f.len())));
        }
        let num = |i: usize, what: &str| -> Result<f32, String> {
            f[i].parse::<f32>()
                .map_err(|_| at(&format!("bad {what} {:?}", f[i])))
        };
        let continent: u8 = f[0]
            .parse()
            .ok()
            .filter(|c| STYLES.contains(c))
            .ok_or_else(|| at(&format!("bad continent {:?}", f[0])))?;
        rows.push(TerrainDefRow {
            continent,
            diffuse: f[1].to_owned(),
            heightmap: f[2].to_owned(),
            specmap: f[3].to_owned(),
            base_dominance: num(4, "base dominance")?,
            texture_scale: num(5, "texture scale")?,
            projection: f[6]
                .parse()
                .map_err(|_| at(&format!("bad projection {:?}", f[6])))?,
            id: u32::from_str_radix(f[7], 16).map_err(|_| at(&format!("bad hex id {:?}", f[7])))?,
        });
    }
    if rows.is_empty() {
        return Err(format!("{TERRAIN_DEF} has no rows"));
    }
    Ok(rows)
}

/// How a terrain type finds its row.
#[derive(Debug, Clone, Copy)]
enum Rule {
    /// Diffuse file `<prefix>_<stem>_d.dds`, searched on the style's continent, then on
    /// continent 0, then on continent 4 (Europe).
    Stem(&'static str),
    /// Row of continent 0 with this `ID` (generic cell types: snow, coast, ocean).
    Generic(u32),
}

/// SIMPLIFIED(SIMP-006): `TERRAIN_HILL` has no texture of its own in `terraindef.csv` (the
/// original textures a hill by the base terrain below it, which `TERRAIN_HILL` does not
/// carry), so a hill takes the plains texture.
fn rule_for(terrain: &str) -> Option<Rule> {
    Some(match terrain {
        "TERRAIN_GRASS" => Rule::Stem("grassland"),
        "TERRAIN_PLAINS" | "TERRAIN_HILL" => Rule::Stem("plain"),
        "TERRAIN_DESERT" => Rule::Stem("desert"),
        "TERRAIN_TUNDRA" => Rule::Stem("tundra"),
        "TERRAIN_MOUNTAIN" => Rule::Stem("mountain_base"),
        MOUNTAIN_TOP => Rule::Stem("mountain_top"),
        // IDs 0x30 / 0x05 / 0x15 of continent 0: Generic_Snow, Euro_Coast, Worl_Ocean_Floor.
        "TERRAIN_SNOW" => Rule::Generic(0x30),
        "TERRAIN_COAST" => Rule::Generic(0x05),
        "TERRAIN_OCEAN" => Rule::Generic(0x15),
        _ => return None,
    })
}

fn stem_matches(diffuse: &str, stem: &str) -> bool {
    let d = diffuse.to_ascii_lowercase();
    let Some(d) = d.strip_suffix("_d.dds") else {
        return false;
    };
    d == stem || d.ends_with(&format!("_{stem}"))
}

/// The row for a rule on `style`, and whether it came from another continent than `style`.
fn select(rows: &[TerrainDefRow], rule: Rule, style: u8) -> Option<(&TerrainDefRow, bool)> {
    match rule {
        Rule::Generic(id) => rows
            .iter()
            .find(|r| r.continent == 0 && r.id == id)
            .map(|r| (r, false)),
        // SIMPLIFIED(SIMP-005): a style without its own row (Generic has almost none; Asia and
        // Africa lack a mountain top) takes the Generic row, then the Europe one.
        Rule::Stem(stem) => [style, 0, 4].into_iter().find_map(|c| {
            rows.iter()
                .find(|r| r.continent == c && stem_matches(&r.diffuse, stem))
                .map(|r| (r, c != style))
        }),
    }
}

pub(super) fn group_of_style(style: u8) -> PrefetchGroup {
    // Group 1 for Generic, 11..14 for America, Asia, Africa, Europe.
    PrefetchGroup(if style == 0 { 1 } else { 10 + u16::from(style) })
}

/// Everything `b1-terrain` puts into the table and the manifest, before any conversion.
#[derive(Debug, Default)]
pub struct TerrainPlan {
    pub terrain: Vec<(String, u8, TerrainArt)>,
    pub features: Vec<(String, u8, FeatureArt)>,
    pub river: Option<RiverArt>,
    /// Lowercase file name to prefetch group (the lowest of its users).
    pub textures: BTreeMap<String, PrefetchGroup>,
    /// "terrain style: took continent N" notes for the build log.
    pub fallbacks: Vec<String>,
}

impl TerrainPlan {
    fn use_texture(&mut self, file: &str, group: PrefetchGroup) {
        let g = self
            .textures
            .entry(file.to_ascii_lowercase())
            .or_insert(group);
        *g = (*g).min(group);
    }

    /// Adds the records to `table`. Feature records keep a tree model and placement parameters
    /// that other sets have put there.
    pub fn apply(&self, table: &mut ArtTable) {
        for (t, s, rec) in &self.terrain {
            table.insert_terrain(t, *s, rec.clone());
        }
        for (t, s, rec) in &self.features {
            let mut rec = rec.clone();
            if let Some(old) = table.feature(t, *s) {
                rec.tree_model = old.tree_model.clone();
                rec.placement = old.placement.clone();
            }
            table.insert_feature(t, *s, rec);
        }
        if let Some(r) = &self.river {
            table.insert_river(r.clone());
        }
    }
}

/// Plans the records for `terrains` (all `Terrains.Type` of the game). A terrain type this
/// module has no rule for, or without any row, is an error: nothing is skipped silently.
pub fn plan_terrain(rows: &[TerrainDefRow], terrains: &[String]) -> Result<TerrainPlan, String> {
    let mut plan = TerrainPlan::default();
    let types = terrains
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(MOUNTAIN_TOP));
    for t in types {
        let rule = rule_for(t).ok_or_else(|| {
            format!("no texture rule for {t}: add it to sets/terrain.rs (rule_for)")
        })?;
        for style in STYLES {
            let (row, fell_back) = select(rows, rule, style).ok_or_else(|| {
                format!("{TERRAIN_DEF} has no row for {t} (style {style}, rule {rule:?})")
            })?;
            if fell_back {
                plan.fallbacks.push(format!(
                    "{t} style {style}: {} (continent {})",
                    row.diffuse, row.continent
                ));
            }
            plan.use_texture(&row.diffuse, group_of_style(style));
            plan.terrain.push((
                t.to_owned(),
                style,
                TerrainArt {
                    texture: texture_id(&row.diffuse),
                    texture_scale: row.texture_scale,
                    // SIMPLIFIED(SIMP-005): vertical projection (1) is mapped to the mesh UVs;
                    // no row of the cell types above uses it.
                    projection: if row.projection == 0 {
                        Projection::WorldXz
                    } else {
                        Projection::MeshUv
                    },
                },
            ));
        }
    }
    for &(feature, file) in FEATURE_DECALS {
        plan.use_texture(file, PrefetchGroup(1));
        for style in STYLES {
            plan.features.push((
                feature.to_owned(),
                style,
                FeatureArt {
                    texture: Some(texture_id(file)),
                    tree_model: None,
                    placement: BTreeMap::new(),
                },
            ));
        }
    }
    for f in EXTRA_TEXTURES
        .iter()
        .chain(RIVER_TEXTURES)
        .chain(FOG_CLOUD_TEXTURES)
        .chain(WATER_TEXTURES)
    {
        plan.use_texture(f, PrefetchGroup(1));
    }
    plan.river = Some(RiverArt {
        textures: RIVER_TEXTURES.iter().map(|f| texture_id(f)).collect(),
    });
    Ok(plan)
}

/// `Terrains.Type` of the snapshot, in ID order.
pub fn terrain_types_from_snapshot(snapshot: &Path) -> Result<Vec<String>, ModelError> {
    let path = snapshot.join(GAMEPLAY_DB_FILE);
    let db = Db::open_read_only(&path)
        .map_err(|e| ModelError::invalid(format!("opening {}: {e}", path.display())))?;
    let list = db
        .scalar_text("select group_concat(Type, ',') from (select Type from Terrains order by ID)")
        .map_err(|e| ModelError::invalid(format!("reading Terrains: {e}")))?
        .unwrap_or_default();
    let types: Vec<String> = list
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    if types.is_empty() {
        return Err(ModelError::invalid(format!(
            "{} has no Terrains rows",
            path.display()
        )));
    }
    Ok(types)
}

/// What a build of the set produced.
#[derive(Debug)]
pub struct TerrainSummary {
    pub textures: usize,
    /// Bytes of the blobs per variant format.
    pub bytes: BTreeMap<VariantFormat, u64>,
    pub terrain_records: usize,
    pub feature_records: usize,
    pub table_entries: usize,
    pub table_bytes: u64,
    pub fallbacks: Vec<String>,
    pub stats: StoreStats,
}

impl TerrainSummary {
    pub fn total_bytes(&self) -> u64 {
        self.bytes.values().sum::<u64>() + self.table_bytes
    }
}

/// Reads a game file by flat name; `None` if the install lacks it.
pub type ReadFile<'a> = &'a (dyn Fn(&str) -> Result<Option<Vec<u8>>, ModelError> + Sync);

/// Builds the set into `out`: textures and the table records, added to the manifest already
/// there.
pub fn build_terrain(
    read: ReadFile<'_>,
    terrains: &[String],
    out: &Path,
    opts: TextureOptions,
) -> Result<TerrainSummary, ModelError> {
    let need = |name: &str| {
        read(name)?
            .ok_or_else(|| ModelError::invalid(format!("{name} is missing from the install")))
    };
    let csv = String::from_utf8_lossy(&need(TERRAIN_DEF)?).into_owned();
    let rows = parse_terraindef(&csv).map_err(ModelError::invalid)?;
    let plan = plan_terrain(&rows, terrains).map_err(ModelError::invalid)?;

    let store = BlobStore::open(out)?;
    let cache = BuildCache::open(&store)?;
    let mut manifest = ManifestBuilder::open(&store)?;

    let files: Vec<(&String, PrefetchGroup)> = plan.textures.iter().map(|(f, g)| (f, *g)).collect();
    let converted: Vec<Result<Vec<_>, ModelError>> = files
        .par_iter()
        .map(|(f, _)| convert_texture(&cache, &store, f, &need(f)?, opts))
        .collect();
    let mut bytes: BTreeMap<VariantFormat, u64> = BTreeMap::new();
    for ((file, group), result) in files.iter().zip(converted) {
        for (format, hash, size) in result? {
            *bytes.entry(format).or_default() += size;
            manifest.add(
                texture_id(file),
                Variant {
                    format,
                    hash,
                    size,
                    prefetch: *group,
                },
            );
        }
    }

    let mut table = load_art_table(&store, &manifest)?;
    plan.apply(&mut table);
    let table_bytes = store_art_table(&store, &mut manifest, &table)?;
    manifest.finish(&store)?;
    Ok(TerrainSummary {
        textures: files.len(),
        bytes,
        terrain_records: plan.terrain.len(),
        feature_records: plan.features.len(),
        table_entries: table.len(),
        table_bytes,
        fallbacks: plan.fallbacks,
        stats: store.stats(),
    })
}
