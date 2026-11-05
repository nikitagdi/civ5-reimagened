//! B1 asset sets that are not unit models. Each set adds its blobs to
//! the shared manifest and its records to the shared art table `table:art-b1`.

pub mod b2_city;
pub mod b2_fx;
pub mod b2_landmarks;
pub mod b2_light;
pub mod b2_misc;
pub mod b2_models;
pub mod b2_terrain;
pub mod b2_units;
pub mod b2_water;
pub mod cities;
pub mod client_data;
pub mod formations;
pub mod sv;
pub mod table;
pub mod terrain;
pub mod trees;
pub mod units;
pub mod web_lobby;

use asset_format::{
    ArtTable, ContentHash, LogicalId, ModelBlob, PrefetchGroup, TextureFormat, Variant,
    VariantFormat,
};
use serde::{Deserialize, Serialize};

use crate::models::ModelError;
use crate::store::{BlobStore, BuildCache, CacheKey, ManifestBuilder};
use crate::textures::{ConvertOptions, color_space_for, convert_dds_with};

/// Id of a B2 world texture: `tx:` and the lower-case name of the source file (AB-D11).
pub fn tx_id(file: &str) -> LogicalId {
    LogicalId::new(format!("tx:{}", file.to_ascii_lowercase()))
}

/// Id of the clip pack of a B2 model: `an:` and the file name without extension, lower case.
pub fn an_id(model: &str) -> LogicalId {
    LogicalId::new(format!("an:{}", model.to_ascii_lowercase()))
}

/// Id of an idle state machine: `fsm:` and the `.fsmxml` name without extension, lower case.
pub fn fsm_id(name: &str) -> LogicalId {
    LogicalId::new(format!("fsm:{}", name.to_ascii_lowercase()))
}

/// Manifest id of the art table (C5AT); every set of the composite `b1` adds its keys to it.
pub const ART_TABLE_ID: &str = "table:art-b1";

/// Bump when the converted bytes change for the same inputs (invalidates the build cache). 3: ASTC
/// on by default.
pub const TEXTURE_STEP_VERSION: u32 = 3;

/// Texture conversion settings of the B1 sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureOptions {
    /// Largest side of a converted texture; `None` keeps the source size.
    pub max_side: Option<u32>,
    /// Also write an ASTC 4x4 variant, for phones (on by default).
    pub astc: bool,
}

impl Default for TextureOptions {
    fn default() -> Self {
        Self {
            max_side: Some(512),
            astc: true,
        }
    }
}

impl TextureOptions {
    fn formats(self) -> Vec<(TextureFormat, VariantFormat)> {
        // SIMPLIFIED(SIMP-022): the b1-* sets write textures as BC7 and ASTC 4x4 (phones),
        // no RGBA8 variant (operator decision 2026-10-20); a device
        // with neither BC nor ASTC gets no texture and the client falls back to flat colours. The
        // sides are capped at 512, so the phones' top-level drop of option A never applies here.
        // `--no-astc` writes BC7 only.
        let mut f = vec![(TextureFormat::Bc7, VariantFormat::Bc)];
        if self.astc {
            f.push((TextureFormat::Astc4x4, VariantFormat::Astc));
        }
        f
    }
}

#[derive(Serialize, Deserialize)]
struct TextureStepOut {
    variants: Vec<(VariantFormat, ContentHash, u64)>,
}

/// One DDS file to blobs, through the build cache. Returns (variant, hash, size) per format.
pub(crate) fn convert_texture(
    cache: &BuildCache<'_>,
    store: &BlobStore,
    file: &str,
    bytes: &[u8],
    opts: TextureOptions,
) -> Result<Vec<(VariantFormat, ContentHash, u64)>, ModelError> {
    let space = color_space_for(file);
    let formats = opts.formats();
    let params = [
        u8::from(matches!(space, crate::textures::ColorSpace::Srgb)),
        u8::from(opts.astc),
    ]
    .into_iter()
    .chain(opts.max_side.unwrap_or(0).to_le_bytes())
    .collect::<Vec<u8>>();
    let key = CacheKey::new(
        "texture-b1",
        TEXTURE_STEP_VERSION,
        &[ContentHash(*blake3::hash(bytes).as_bytes())],
        &params,
    );
    let out: TextureStepOut = cache.get_or_compute(
        &key,
        |o: &TextureStepOut| o.variants.iter().map(|v| v.1).collect(),
        || {
            let blobs = convert_dds_with(
                bytes,
                ConvertOptions {
                    color_space: space,
                    generate_mips: true,
                    max_side: opts.max_side,
                },
                &formats.iter().map(|f| f.0).collect::<Vec<_>>(),
            )
            .map_err(|source| ModelError::Texture {
                name: file.to_owned(),
                source,
            })?;
            let mut variants = Vec::new();
            for (blob, (_, variant)) in blobs.iter().zip(&formats) {
                let (hash, size) = store.put(&blob.encode())?;
                variants.push((*variant, hash, size));
            }
            Ok::<_, ModelError>(TextureStepOut { variants })
        },
    )?;
    Ok(out.variants)
}

/// The art table already in the manifest (empty if there is none).
pub(crate) fn load_art_table(
    store: &BlobStore,
    manifest: &ManifestBuilder,
) -> Result<ArtTable, ModelError> {
    let id = LogicalId::new(ART_TABLE_ID);
    let Some(v) = manifest.manifest().variants(&id).first() else {
        return Ok(ArtTable::new());
    };
    let bytes = std::fs::read(store.blob_path(&v.hash)).map_err(|e| {
        ModelError::invalid(format!(
            "reading the art table {}: {e}",
            store.blob_path(&v.hash).display()
        ))
    })?;
    ArtTable::decode(&bytes)
        .map_err(|e| ModelError::invalid(format!("art table in the manifest: {e}")))
}

/// Stores `table` as `table:art-b1` (prefetch group 1). Returns the blob size.
pub(crate) fn store_art_table(
    store: &BlobStore,
    manifest: &mut ManifestBuilder,
    table: &ArtTable,
) -> Result<u64, ModelError> {
    let (hash, size) = store.put(&table.encode())?;
    manifest.add(
        LogicalId::new(ART_TABLE_ID),
        Variant {
            format: VariantFormat::Generic,
            hash,
            size,
            prefetch: PrefetchGroup(1),
        },
    );
    Ok(size)
}

/// Bump when the bytes of a model blob of the tree and city sets change for the same inputs.
pub const MODEL_STEP_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct ModelStepOut {
    model: (ContentHash, u64),
}

/// One model blob through the build cache; `compute` builds the blob on a miss. Returns the hash
/// and size of the stored blob.
pub(crate) fn cached_model(
    cache: &BuildCache<'_>,
    store: &BlobStore,
    key: &CacheKey,
    compute: impl FnOnce() -> Result<ModelBlob, ModelError>,
) -> Result<(ContentHash, u64), ModelError> {
    let out: ModelStepOut = cache.get_or_compute(
        key,
        |o: &ModelStepOut| vec![o.model.0],
        || {
            let model = store.put(&compute()?.encode())?;
            Ok::<_, ModelError>(ModelStepOut { model })
        },
    )?;
    Ok(out.model)
}

/// Adds a model blob to the manifest as a generic variant.
pub(crate) fn add_model(
    manifest: &mut ManifestBuilder,
    id: LogicalId,
    blob: (ContentHash, u64),
    prefetch: PrefetchGroup,
) {
    manifest.add(
        id,
        Variant {
            format: VariantFormat::Generic,
            hash: blob.0,
            size: blob.1,
            prefetch,
        },
    );
}

/// Converts every texture of `files` (lowercase name to prefetch group) through the cache and
/// adds the variants to the manifest. A file the install lacks is an error. Returns the bytes per
/// variant format.
pub(crate) fn add_textures(
    cache: &BuildCache<'_>,
    store: &BlobStore,
    manifest: &mut ManifestBuilder,
    read: terrain::ReadFile<'_>,
    files: &std::collections::BTreeMap<String, PrefetchGroup>,
    opts: TextureOptions,
) -> Result<std::collections::BTreeMap<VariantFormat, u64>, ModelError> {
    use rayon::prelude::*;
    let list: Vec<(&String, PrefetchGroup)> = files.iter().map(|(f, g)| (f, *g)).collect();
    let converted: Vec<Result<Vec<_>, ModelError>> = list
        .par_iter()
        .map(|(f, _)| {
            let bytes = read(f)?
                .ok_or_else(|| ModelError::invalid(format!("{f} is missing from the install")))?;
            convert_texture(cache, store, f, &bytes, opts)
        })
        .collect();
    let mut bytes = std::collections::BTreeMap::new();
    for ((file, group), result) in list.iter().zip(converted) {
        for (format, hash, size) in result? {
            *bytes.entry(format).or_default() += size;
            manifest.add(
                crate::models::set::texture_id(file),
                Variant {
                    format,
                    hash,
                    size,
                    prefetch: *group,
                },
            );
        }
    }
    Ok(bytes)
}
