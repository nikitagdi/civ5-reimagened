//! Set `b2-light` (milestone M1): the light and colour data of the
//! original, all in the boot group.
//!
//! - light cubes of the terrain (`sunlight_*`, `shadowlight_*`, the `_2` variants of Upgrade1) and
//!   of the models (`unit_irradiance`, `unit_environment_dull/sharp`, `cubelight_generic_32`),
//! - `specscreenmap`, the specular highlight screen,
//! - the colour-key lookup tables as `D3` 32^3 volumes ([`crate::world::lut`]),
//! - `fogramp`, the distance-fog ramp, and the textures of the fog clouds,
//! - the textures of the world overlay (hex grid, contours, highlights, selection ring, arrows) and
//!   of the minimap,
//! - the world settings blob `tb:world-b2` (`C5WS`, [`crate::world::settings`]).
//!
//! Every texture is the original's bytes (AB-D2): no re-encoding, `srgb = false`. Ids are
//! `tx:<source file name>`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use asset_format::{
    ContentHash, LogicalId, PrefetchGroup, TexDim, TextureBlob, Trigger, Variant, VariantFormat,
    WORLD_SETTINGS_ID,
};
use rayon::prelude::*;

use super::terrain::ReadFile;
use super::tx_id;
use crate::game::GameSource;
use crate::models::ModelError;
use crate::store::{BlobStore, ManifestBuilder, StoreStats};
use crate::textures::{TextureError, dds_passthrough, level0_only, variant_format};
use crate::world::groups::{self, GroupSpec};
use crate::world::lut::{lut_files, lut_from_tga};
use crate::world::settings::{self, SETTINGS_FILES};

pub const SET_NAME: &str = "b2-light";

/// Group of the first loaded data: `Variant.prefetch = 1` is `Boot` in B2 (AB-D10).
pub use crate::world::groups::BOOT;

/// Continent styles of the terrain light cubes.
const STYLES: [&str; 5] = ["africa", "america", "asia", "europe", "generic"];
/// Styles that have a second shadow cube (`..._terrain_2.dds`, Upgrade1).
// measured: 2026-10-21, `shadowlight_{america,asia,europe}_terrain_2.dds` exist, no `sunlight_*_2`.
const STYLES_WITH_SECOND_SHADOW: [&str; 3] = ["america", "asia", "europe"];
/// Fog of war: the cloud tiles, the fog sprite and the flow field.
// `flowfield` stays here until the `shader-re` catalog says whether the
// sampler belongs to `FOW` or to `WorldCurrents` (then it goes to the water group).
const FOG_TEXTURES: [&str; 5] = [
    "fogcloudtile_dif_01.dds",
    "fogcloudtile_dif_02.dds",
    "fogcloudtile_dif_03.dds",
    "fx_fog_dif.dds",
    "flowfield.dds",
];
/// World overlay: hex grid, contours, highlight textures, selection ring and glow, arrows.
const OVERLAY_TEXTURES: [&str; 17] = [
    "hexgrid.dds",
    "hexgridid.dds",
    "hex_contour1.dds",
    "hex_contour2.dds",
    "spline_border_contour1.dds",
    "spline_border_contour2.dds",
    "static_texture_highlight.dds",
    "solid_contour.dds",
    "gui_unitselectring01.dds",
    "gui_unitselectringglow01.dds",
    "gui_unitselectedhexglow_01.dds",
    "arrow.dds",
    "arrow_food.dds",
    "arrow_popup.dds",
    "arrow_popup_food.dds",
    "arrow_popup_prod.dds",
    "arrow_prod.dds",
];
/// Minimap layers. Group `minimap`, not `boot`.
const MINIMAP_TEXTURES: [&str; 7] = [
    "minimapculture.dds",
    "minimapfog.dds",
    "minimapgrass.dds",
    "minimapice.dds",
    "minimapmountain.dds",
    "minimapshallowwater.dds",
    "minimapwater.dds",
];
/// Light cubes of the unit and landmark models, and the cube lights and reflection cubes of the
/// houses of the cities.
// measured: S3 (2026-10-28), the houses bind <culture>_cubelight.dds and hiconreflmap_soft/sharp.
const MODEL_CUBES: [&str; 10] = [
    "unit_irradiance.dds",
    "unit_environment_dull.dds",
    "unit_environment_sharp.dds",
    "cubelight_generic_32.dds",
    "africa_cubelight.dds",
    "americas_cubelight.dds",
    "asia_cubelight.dds",
    "europe_cubelight.dds",
    "hiconreflmap_soft.dds",
    "hiconreflmap_sharp.dds",
];

/// How a file becomes a blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// DDS, all mips of the file.
    Dds,
    /// DDS, level 0 only (small data textures, 4.2).
    DdsLevel0,
    /// Colour-key TGA to a `D3` 32^3 volume.
    Lut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Lower-case file name, also the id after `tx:`.
    pub file: String,
    pub source: Source,
    /// Shape the blob must have.
    pub dim: TexDim,
    /// In the boot group; the others get group 0, "by the group table" (AB-D10).
    pub boot: bool,
}

/// The textures of the set, sorted by file name.
pub fn plan() -> Vec<Item> {
    let item = |file: String, source, dim| Item {
        file,
        source,
        dim,
        boot: true,
    };
    let mut v = Vec::new();
    for style in STYLES {
        v.push(item(
            format!("sunlight_{style}_terrain.dds"),
            Source::Dds,
            TexDim::Cube,
        ));
        v.push(item(
            format!("shadowlight_{style}_terrain.dds"),
            Source::Dds,
            TexDim::Cube,
        ));
    }
    for style in STYLES_WITH_SECOND_SHADOW {
        v.push(item(
            format!("shadowlight_{style}_terrain_2.dds"),
            Source::Dds,
            TexDim::Cube,
        ));
    }
    for file in MODEL_CUBES {
        v.push(item(file.to_owned(), Source::Dds, TexDim::Cube));
    }
    v.push(item(
        "specscreenmap.dds".to_owned(),
        Source::Dds,
        TexDim::D2,
    ));
    // fogramp keeps all its mips (orchestrator decision R1.1, +1.4 KB).
    for file in FOG_TEXTURES.into_iter().chain(OVERLAY_TEXTURES) {
        v.push(item(file.to_owned(), Source::Dds, TexDim::D2));
    }
    v.push(item("fogramp.dds".to_owned(), Source::Dds, TexDim::D2));
    for file in MINIMAP_TEXTURES {
        v.push(Item {
            boot: false,
            ..item(file.to_owned(), Source::Dds, TexDim::D2)
        });
    }
    for file in lut_files() {
        v.push(item(file, Source::Lut, TexDim::D3));
    }
    v.sort_by(|a, b| a.file.cmp(&b.file));
    v
}

/// One file to its blob.
pub fn convert(item: &Item, bytes: &[u8]) -> Result<TextureBlob, TextureError> {
    let blob = match item.source {
        Source::Dds => dds_passthrough(bytes)?,
        Source::DdsLevel0 => level0_only(dds_passthrough(bytes)?),
        Source::Lut => lut_from_tga(bytes)?,
    };
    if blob.dim != item.dim {
        return Err(TextureError::Unsupported(format!(
            "expected a {:?} texture, the file is {:?}",
            item.dim, blob.dim
        )));
    }
    Ok(blob)
}

#[derive(Debug)]
pub struct LightSummary {
    pub textures: usize,
    /// Bytes per manifest variant format.
    pub bytes: BTreeMap<VariantFormat, u64>,
    /// Bytes of the textures of the boot group, and of the ones that are not.
    pub boot_bytes: u64,
    pub other_bytes: u64,
    /// Size of the `C5WS` blob.
    pub settings_bytes: u64,
    /// Size of the `C5GP` blob.
    pub group_table_bytes: u64,
    /// Unknown attributes and odd values in the settings files.
    pub warnings: Vec<String>,
    pub stats: StoreStats,
}

impl LightSummary {
    pub fn total_bytes(&self) -> u64 {
        self.bytes.values().sum()
    }
}

type Built = Result<(VariantFormat, ContentHash, u64), ModelError>;

/// Builds the set into `out` and adds it to the manifest there: the textures and the settings
/// blob. A file the install lacks, or one that is not the expected kind of texture, is an error
/// (no stand-ins, AB-D14).
pub fn build_light(read: ReadFile<'_>, out: &Path) -> Result<LightSummary, ModelError> {
    let store = BlobStore::open(out)?;
    let mut manifest = ManifestBuilder::open(&store)?;
    let items = plan();
    let built: Vec<Built> = items
        .par_iter()
        .map(|item| {
            let bytes = read(&item.file)?.ok_or_else(|| {
                ModelError::invalid(format!("{} is missing from the install", item.file))
            })?;
            let blob = convert(item, &bytes).map_err(|source| ModelError::Texture {
                name: item.file.clone(),
                source,
            })?;
            let (hash, size) = store.put(&blob.encode())?;
            Ok((variant_format(blob.format), hash, size))
        })
        .collect();
    let mut bytes: BTreeMap<VariantFormat, u64> = BTreeMap::new();
    let (mut boot_bytes, mut other_bytes) = (0, 0);
    for (item, result) in items.iter().zip(built) {
        let (format, hash, size) = result?;
        *bytes.entry(format).or_default() += size;
        *if item.boot {
            &mut boot_bytes
        } else {
            &mut other_bytes
        } += size;
        manifest.add(
            tx_id(&item.file),
            Variant {
                format,
                hash,
                size,
                prefetch: if item.boot { BOOT } else { PrefetchGroup(0) },
            },
        );
    }

    // The settings blob is shared with the other sets: this one replaces its own sections.
    let (light, warnings) = settings::build_settings(read)?;
    let known: BTreeSet<LogicalId> = items.iter().map(|i| tx_id(&i.file)).collect();
    settings::check_references(&light, |t| known.contains(t))?;
    let mut world = settings::load_existing(&store, &manifest)?;
    settings::apply_light(&mut world, light);
    let settings_bytes = settings::store_settings(&store, &mut manifest, &world)?;

    // Groups (AB-D10): `boot` holds the boot textures and the settings; the minimap layers wait
    // for the map.
    let boot_members: Vec<LogicalId> = items
        .iter()
        .filter(|i| i.boot)
        .map(|i| tx_id(&i.file))
        .chain([LogicalId::new(WORLD_SETTINGS_ID)])
        .collect();
    let minimap_members: Vec<LogicalId> = items
        .iter()
        .filter(|i| !i.boot)
        .map(|i| tx_id(&i.file))
        .collect();
    let group_table_bytes = groups::publish(
        &store,
        &mut manifest,
        vec![
            GroupSpec::boot(boot_members),
            GroupSpec::map("minimap", Trigger::Always, minimap_members),
        ],
    )?;

    manifest.finish(&store)?;
    Ok(LightSummary {
        textures: items.len(),
        bytes,
        boot_bytes,
        other_bytes,
        settings_bytes,
        group_table_bytes,
        warnings,
        stats: store.stats(),
    })
}

/// Files the conflict report also watches although this set does not build them yet: the terrain
/// table (RW-9, `world-terrain`).
const WATCHED_ONLY: [&str; 1] = ["terraindef.csv"];

/// Files of the set (textures and settings) that exist in more than one layer of the install with
/// different bytes (AB-D15). The index serves the highest layer; this lists what it hides.
pub fn layer_conflicts(game: &GameSource) -> Result<Vec<String>, ModelError> {
    let names: Vec<String> = plan()
        .into_iter()
        .map(|i| i.file)
        .chain(
            SETTINGS_FILES
                .iter()
                .chain(&WATCHED_ONLY)
                .map(|f| (*f).to_owned()),
        )
        .collect();
    let mut out = Vec::new();
    for hidden in game.shadowed() {
        if !names.contains(&hidden.name) {
            continue;
        }
        let Some(winner) = game.lookup(&hidden.name) else {
            continue;
        };
        if game.read(hidden)? != game.read(winner)? {
            let from = |e: &crate::game::source::Entry| {
                game.origin_path(e).map_or_else(String::new, |p| {
                    p.strip_prefix(game.root())
                        .unwrap_or(&p)
                        .display()
                        .to_string()
                })
            };
            out.push(format!(
                "{}: {:?} {} ({} B) hides {:?} {} ({} B)",
                hidden.name,
                winner.layer,
                from(winner),
                winner.size(),
                hidden.layer,
                from(hidden),
                hidden.size()
            ));
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_has_the_expected_files() {
        let p = plan();
        // 13 terrain cubes, 10 model cubes, specscreenmap, 22 colour keys, fogramp and 5 more of the
        // fog, 17 overlay textures, 7 of the minimap.
        assert_eq!(p.len(), 13 + 10 + 1 + 22 + 1 + 5 + 17 + 7);
        let names: Vec<_> = p.iter().map(|i| i.file.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted, "sorted, no repeats");
        for f in [
            "sunlight_europe_terrain.dds",
            "shadowlight_asia_terrain_2.dds",
            "unit_irradiance.dds",
            "cubelight_generic_32.dds",
            "specscreenmap.dds",
            "terrainshadow_europe_colorkey.tga",
            "fogramp.dds",
            "fogcloudtile_dif_02.dds",
            "hexgridid.dds",
            "arrow_popup_food.dds",
            "minimapshallowwater.dds",
        ] {
            assert!(names.contains(&f), "{f}");
        }
        assert_eq!(p.iter().filter(|i| i.dim == TexDim::Cube).count(), 23);
        assert_eq!(p.iter().filter(|i| i.dim == TexDim::D3).count(), 22);
        // The minimap is not in the boot group; nothing else is left out of it.
        let not_boot: Vec<_> = p.iter().filter(|i| !i.boot).map(|i| &i.file).collect();
        assert_eq!(not_boot.len(), 7);
        assert!(not_boot.iter().all(|f| f.starts_with("minimap")));
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_stand_in() {
        let dir = std::env::temp_dir().join(format!("civ5-b2-light-{}", std::process::id()));
        let read = |_: &str| -> Result<Option<Vec<u8>>, ModelError> { Ok(None) };
        let err = build_light(&read, &dir).unwrap_err().to_string();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(err.contains("missing from the install"), "{err}");
    }

    #[test]
    fn an_unreadable_file_is_an_error() {
        let item = Item {
            file: "x.dds".into(),
            source: Source::Dds,
            dim: TexDim::Cube,
            boot: true,
        };
        assert!(convert(&item, b"not a dds").is_err());
    }
}
