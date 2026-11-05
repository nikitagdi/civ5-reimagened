//! `variants`: the compressed variants of every texture of an output folder
//! (option A).
//!
//! Runs on a finished pack, after the sets, and only adds variants; it never replaces one:
//! - **desktop:** an `Rgba8` colour texture gets a BC7 variant (`Bc`); the `Rgba8` stays as the
//!   `Uncompressed` fallback, and the BC1/2/3 originals stay byte for byte (AB-D2);
//! - **mobile:** every texture gets an ASTC 4x4 variant (`Astc`), encoded from the decoded pixels of
//!   the original (`Rgba8`, BC1/2/3, or the BC7 of the b1 sets). A texture whose larger side is above
//!   1024 drops its top level there (phones only, option A).
//!
//! A texture that gets neither is a written exception ([`Rules`]): the pass and `verify` share the
//! rules, and the report lists every texture a BC device or an ASTC device gets uncompressed, with
//! the rule. Each new encode is checked against its source (PSNR of level 0 and 1, a floor per class);
//! one below the floor is not added and is a problem in the report. The encodes go through the build
//! cache, so a second run only reads.
//!
//! UI textures (`ui:tx:`) are uploaded at level 0 only, so they get level 0 only; one that is not a
//! multiple of 4 is padded to whole blocks with its edge texels, and the client samples the image
//! part (`asset_format::stored_content`).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result, bail};
use asset_format::{
    ContentHash, LogicalId, Manifest, PlatformProfile, TEXTURE_MAGIC, TexDim, TextureBlob,
    TextureFormat, UI_INDEX_ID, UiIndex, Variant, VariantFormat, WORLD_SETTINGS_ID, WorldSettings,
    pad_to_block,
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::encode::encode_chain_keep_rgb;
use super::quality::{decode_slice, psnr};
use super::repack::{Exceptions, array_textures};
use super::{ColorSpace, Rgba8Image, variant_format};
use crate::store::{BlobStore, BuildCache, CacheKey, ManifestBuilder};

/// Bump when the bytes of a variant change for the same source (invalidates the build cache). 2:
/// the UI's RGB is gated as it shows, premultiplied by alpha.
pub const STEP_VERSION: u32 = 2;

/// Stack of an encoder thread.
const ENCODER_STACK: usize = 16 << 20;

/// Above this side the `Astc` variant drops its top level (option A).
pub const MOBILE_MAX_SIDE: u32 = 1024;

pub const REPORT_JSON: &str = "report-textures.json";
pub const REPORT_TXT: &str = "report-textures.txt";

/// What the texture holds, for the PSNR floor of its encodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Class {
    /// A picture of the world (sRGB-ish values the shaders gamma themselves).
    Colour,
    /// Data read as numbers: normal, specular, height and blend maps (`color_space_for` is linear).
    Data,
    /// An image of the UI.
    Ui,
}

impl Class {
    fn of(id: &str) -> Self {
        if is_ui(id) {
            return Self::Ui;
        }
        let name = id.split_once(':').map_or(id, |(_, n)| n);
        let name = name.split_once('@').map_or(name, |(n, _)| n);
        match super::color_space_for(name) {
            ColorSpace::Linear => Self::Data,
            ColorSpace::Srgb => Self::Colour,
        }
    }

    /// Lowest PSNR in dB (RGB and alpha, level 0 and 1) a new variant of `target` must reach. BC7
    /// replaces the plain texture on desktop, so its floor is near the `repack` gate; ASTC 4x4 is a
    /// second lossy step over DXT on phones (assumption 3) and gets a lower one.
    pub fn floor(self, target: TextureFormat) -> f64 {
        match (target, self) {
            (TextureFormat::Bc7, Self::Data) => 38.0,
            (TextureFormat::Bc7, _) => 36.0,
            (_, Self::Data) => 32.0,
            (_, Self::Colour) => 30.0,
            (_, Self::Ui) => 32.0,
        }
    }
}

fn is_ui(id: &str) -> bool {
    id.starts_with("ui:tx:")
}

/// The texture the variants are made from, as `verify` and the pass see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    pub format: TextureFormat,
    pub dim: TexDim,
    pub width: u32,
    pub height: u32,
}

impl Shape {
    pub fn of(blob: &TextureBlob) -> Self {
        Self {
            format: blob.format,
            dim: blob.dim,
            width: blob.width,
            height: blob.height,
        }
    }
}

/// The written exceptions: textures that keep no BC7 variant (desktop) or no ASTC variant (phones).
#[derive(Debug, Clone, Default)]
pub struct Rules {
    /// `repack-exceptions.txt`: data tables, ramps and textures the client reads on the CPU.
    listed: Exceptions,
    /// UI font atlases (glyph coverage) by manifest id.
    fonts: BTreeSet<String>,
    /// Layers of a texture array (art tables, decals): an array has one format, so the desktop
    /// keeps the format the set wrote.
    arrays: BTreeSet<String>,
}

impl Rules {
    pub fn new(listed: Exceptions, fonts: BTreeSet<String>, arrays: BTreeSet<String>) -> Self {
        Self {
            listed,
            fonts,
            arrays,
        }
    }

    /// The rules of the folder `out`: the built-in list, the fonts of the UI index and the arrays of
    /// the art tables and of the world settings' decals.
    pub fn of_folder(out: &Path, manifest: &Manifest) -> Self {
        let mut arrays = array_textures(out, manifest);
        let read = |id: &str| -> Option<Vec<u8>> {
            let v = manifest.variants(&LogicalId::new(id)).first()?;
            std::fs::read(out.join(v.hash.to_string())).ok()
        };
        if let Some(s) = read(WORLD_SETTINGS_ID).and_then(|b| WorldSettings::decode(&b).ok()) {
            for d in &s.decals {
                arrays.extend(
                    [&d.albedo, &d.heightmap]
                        .into_iter()
                        .flatten()
                        .map(|t| t.as_str().to_owned()),
                );
            }
        }
        let mut fonts = BTreeSet::new();
        if let Some(index) = read(UI_INDEX_ID).and_then(|b| UiIndex::decode(&b).ok()) {
            for f in &index.fonts {
                for t in &f.textures {
                    if let Some(info) = index.find(t) {
                        fonts.insert(info.id.clone());
                    }
                }
            }
        }
        Self::new(Exceptions::builtin(), fonts, arrays)
    }

    /// Why `id` (made from `src`) gets no variant of `target`; `None` when it should get one.
    pub fn exception(&self, id: &str, src: Shape, target: TextureFormat) -> Option<String> {
        if let Some(why) = self.listed.matches(id) {
            return Some(format!("listed: {why}"));
        }
        if self.fonts.contains(id) {
            return Some("font atlas: block compression blurs the glyph edges".into());
        }
        match src.format {
            TextureFormat::Rgba8 | TextureFormat::Bc1 | TextureFormat::Bc2 | TextureFormat::Bc3 => {
            }
            TextureFormat::Bc7 if target == TextureFormat::Astc4x4 => {}
            TextureFormat::Bc7 | TextureFormat::Astc4x4 => {
                return Some(format!("{:?} is the set's own encode", src.format));
            }
            TextureFormat::R8
            | TextureFormat::Rg8
            | TextureFormat::Bc4
            | TextureFormat::Bc5
            | TextureFormat::R16F
            | TextureFormat::Rgba16F => {
                return Some(format!(
                    "{:?}: one- or two-channel or float data, kept as the original",
                    src.format
                ));
            }
        }
        if src.dim == TexDim::D3 {
            // SIMPLIFIED(SIMP-exe-1091): a compressed volume needs the sliced-3D feature, which few
            // devices have; volumes keep the original's format.
            return Some("volume: compressed volumes need the sliced-3D feature".into());
        }
        if target == TextureFormat::Bc7 && src.format != TextureFormat::Rgba8 {
            return Some("BC original, kept byte for byte (AB-D2)".into());
        }
        let ui = is_ui(id);
        if src.format == TextureFormat::Rgba8 && src.width <= 8 && src.height <= 8 {
            return Some(format!(
                "{}x{}: too small to gain from compression",
                src.width, src.height
            ));
        }
        if !ui && (!src.width.is_multiple_of(4) || !src.height.is_multiple_of(4)) {
            // SIMPLIFIED(SIMP-exe-1092): a world texture that is not a multiple of 4 stays plain;
            // padding it would move the seam of a tiled texture.
            return Some(format!(
                "{}x{}: not a multiple of 4, and a world texture is not padded",
                src.width, src.height
            ));
        }
        if target == TextureFormat::Bc7 && self.arrays.contains(base_id(id)) {
            // SIMPLIFIED(SIMP-exe-1093): the desktop keeps a layer of a texture array (art tables,
            // decals) in the set's format, since an array has one format; phones get ASTC for all.
            return Some("layer of a texture array: one format per array on desktop".into());
        }
        None
    }
}

fn base_id(id: &str) -> &str {
    id.split_once('@').map_or(id, |(b, _)| b)
}

/// One texture variant made by the pass.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Added {
    pub id: String,
    pub format: TextureFormat,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub psnr_rgb: f64,
    pub psnr_alpha: f64,
}

/// A texture a device gets uncompressed (or as BC decoded on the CPU), and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    pub id: String,
    /// `R8 128x128` and the like.
    pub texture: String,
    /// `desktop`, `phone` or both.
    pub devices: String,
    pub reason: String,
}

/// An encode that failed or did not reach its floor: the texture keeps what it had.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Failed {
    pub id: String,
    pub format: TextureFormat,
    pub why: String,
    /// It missed the PSNR floor of its class: a written exception (`verify` takes it). An encoder
    /// error is not one.
    #[serde(default)]
    pub floor: bool,
}

impl Failed {
    /// The floor misses of the report in `out` ([`REPORT_JSON`]), for `verify`: (id, format).
    pub fn floor_misses(out: &Path) -> Vec<(String, TextureFormat)> {
        std::fs::read(out.join(REPORT_JSON))
            .ok()
            .and_then(|b| serde_json::from_slice::<VariantReport>(&b).ok())
            .map(|r| {
                r.failed
                    .into_iter()
                    .filter(|f| f.floor)
                    .map(|f| (f.id, f.format))
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VariantReport {
    pub added: Vec<Added>,
    pub kept: Vec<Kept>,
    pub failed: Vec<Failed>,
    /// Stored bytes of the texture variants by `variant/format` (`Bc/Bc1`, `Astc/Astc4x4`, ...),
    /// each blob once: (count, bytes).
    pub variant_bytes: BTreeMap<String, (u64, u64)>,
    /// What each device downloads of the textures (every blob once): BC device, ASTC device,
    /// neither.
    pub profile_bytes: BTreeMap<String, u64>,
    /// Textures (ids with a `C5TX` variant) in the manifest.
    pub textures: usize,
    pub dry_run: bool,
}

impl VariantReport {
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        let mb = |b: u64| b as f64 / 1e6;
        let _ = writeln!(
            s,
            "Texture variants: {} textures, {} variants added, {} failed{}\n",
            self.textures,
            self.added.len(),
            self.failed.len(),
            if self.dry_run {
                " (dry run: nothing written)"
            } else {
                ""
            }
        );
        let _ = writeln!(s, "== bytes per variant (each blob once) ==");
        for (k, (n, b)) in &self.variant_bytes {
            let _ = writeln!(s, "{k:<26} {n:>6} blobs {:>10.1} MB", mb(*b));
        }
        let _ = writeln!(s, "\n== download per device (textures only) ==");
        for (k, b) in &self.profile_bytes {
            let _ = writeln!(s, "{k:<26} {:>10.1} MB", mb(*b));
        }
        for (f, floor) in [
            (TextureFormat::Bc7, "BC7"),
            (TextureFormat::Astc4x4, "ASTC 4x4"),
        ] {
            let mine: Vec<&Added> = self.added.iter().filter(|a| a.format == f).collect();
            if mine.is_empty() {
                continue;
            }
            let worst = mine
                .iter()
                .map(|a| a.psnr_rgb.min(a.psnr_alpha))
                .fold(f64::INFINITY, f64::min);
            let mean = mine.iter().map(|a| a.psnr_rgb.min(99.0)).sum::<f64>() / mine.len() as f64;
            let _ = writeln!(
                s,
                "{floor}: {} added, {:.1} MB, PSNR rgb mean {mean:.1} dB, worst {worst:.1} dB",
                mine.len(),
                mb(mine.iter().map(|a| a.bytes).sum())
            );
        }
        let _ = writeln!(
            s,
            "\n== exceptions: textures a device gets uncompressed ({}) ==",
            self.kept.len()
        );
        for k in &self.kept {
            let _ = writeln!(
                s,
                "{:<52} {:<20} {:<14} {}",
                k.id, k.texture, k.devices, k.reason
            );
        }
        let _ = writeln!(s, "\n== failed encodes ({}) ==", self.failed.len());
        for f in &self.failed {
            let _ = writeln!(s, "{:<52} {:?}: {}", f.id, f.format, f.why);
        }
        s
    }

    pub fn write(&self, out: &Path) -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(out.join(REPORT_JSON), json)?;
        std::fs::write(out.join(REPORT_TXT), self.to_text())
    }
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub dry_run: bool,
}

/// What the cache keeps of one encode.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Outcome {
    Made {
        hash: ContentHash,
        size: u64,
        width: u32,
        height: u32,
        psnr_rgb: f64,
        psnr_alpha: f64,
    },
    /// `floor`: made and under the PSNR floor ([`Rejected::Floor`]); else an error.
    Failed { why: String, floor: bool },
}

struct Job {
    id: LogicalId,
    prefetch: asset_format::PrefetchGroup,
    source_hash: ContentHash,
    dim: TexDim,
    target: TextureFormat,
}

/// The texture variants of `id` and their shapes. Only the shapes stay in memory: a job reads its
/// source again (the UI set alone is near 1 GB of pixels).
type Textures = Vec<(Variant, Shape)>;

/// Runs the pass on the folder `out`; writes the new variants, the manifest and the report unless
/// `opts.dry_run`.
pub fn add_variants(out: &Path, opts: &Options) -> Result<VariantReport> {
    let store = BlobStore::open(out)?;
    let cache = BuildCache::open(&store)?;
    let mut builder = ManifestBuilder::open(&store)?;
    if builder.manifest().is_empty() {
        bail!("{} has no manifest", out.display());
    }
    let rules = Rules::of_folder(out, builder.manifest());
    let textures = read_textures(out, builder.manifest());

    let mut jobs = Vec::new();
    for (id, list) in &textures {
        let Some(&(ref src_variant, src)) = source(list) else {
            continue;
        };
        let has = |f: VariantFormat| list.iter().any(|(v, _)| v.format == f);
        for (target, family) in [
            (TextureFormat::Bc7, VariantFormat::Bc),
            (TextureFormat::Astc4x4, VariantFormat::Astc),
        ] {
            if has(family) || rules.exception(id.as_str(), src, target).is_some() {
                continue;
            }
            jobs.push(Job {
                id: id.clone(),
                prefetch: src_variant.prefetch,
                source_hash: src_variant.hash,
                dim: src.dim,
                target,
            });
        }
    }
    tracing::info!(jobs = jobs.len(), "texture variants to make");

    // The encoders (bc7enc, astcenc) need more stack than rayon's 2 MiB default when the binary
    // is not started through `cargo run` (which sets RUST_MIN_STACK, .cargo/config.toml).
    let pool = rayon::ThreadPoolBuilder::new()
        .stack_size(ENCODER_STACK)
        .build()
        .context("starting the encoder threads")?;
    let done: Vec<(Job, Result<Outcome>)> = pool.install(|| {
        jobs.into_par_iter()
            .map(|j| {
                let o = cached_encode(&cache, &store, &j, opts.dry_run);
                (j, o)
            })
            .collect()
    });

    let mut report = VariantReport {
        dry_run: opts.dry_run,
        textures: textures.len(),
        ..VariantReport::default()
    };
    let mut new_variants: BTreeMap<LogicalId, Vec<Variant>> = BTreeMap::new();
    let mut shapes: BTreeMap<ContentHash, Shape> = BTreeMap::new();
    for list in textures.values() {
        for (v, sh) in list {
            shapes.insert(v.hash, *sh);
        }
    }
    for (job, outcome) in done {
        match outcome? {
            Outcome::Made {
                hash,
                size,
                width,
                height,
                psnr_rgb,
                psnr_alpha,
            } => {
                shapes.insert(
                    hash,
                    Shape {
                        format: job.target,
                        dim: job.dim,
                        width,
                        height,
                    },
                );
                new_variants
                    .entry(job.id.clone())
                    .or_default()
                    .push(Variant {
                        format: variant_format(job.target),
                        hash,
                        size,
                        prefetch: job.prefetch,
                    });
                report.added.push(Added {
                    id: job.id.as_str().to_owned(),
                    format: job.target,
                    width,
                    height,
                    bytes: size,
                    psnr_rgb,
                    psnr_alpha,
                });
            }
            Outcome::Failed { why, floor } => report.failed.push(Failed {
                id: job.id.as_str().to_owned(),
                format: job.target,
                why,
                floor,
            }),
        }
    }
    report.added.sort_by(|a, b| a.id.cmp(&b.id));
    report.failed.sort_by(|a, b| a.id.cmp(&b.id));

    // The manifest with the new variants (a first `add` replaces what was loaded, so every old
    // variant of the id goes in again).
    let mut manifest = builder.manifest().clone();
    for (id, vs) in &new_variants {
        for v in vs {
            manifest.insert(id.clone(), v.clone());
        }
    }
    if !opts.dry_run && !new_variants.is_empty() {
        for (id, vs) in &new_variants {
            let old = builder.manifest().variants(id).to_vec();
            for v in old.into_iter().chain(vs.iter().cloned()) {
                builder.add(id.clone(), v);
            }
        }
        if !builder
            .manifest()
            .variants(&LogicalId::new(asset_format::GROUP_TABLE_ID))
            .is_empty()
        {
            crate::world::groups::publish(&store, &mut builder, Vec::new())
                .map_err(|e| anyhow::anyhow!("updating the group table: {e}"))?;
        }
        builder.finish(&store)?;
        crate::gc::record(out).context("recording the manifest")?;
    }

    // Exceptions and sizes, from the manifest as it is now.
    let ids: Vec<&LogicalId> = textures.keys().collect();
    report.kept = kept_list(&manifest, &ids, &shapes, &rules, &report.failed);
    report.variant_bytes = variant_bytes(&manifest, &ids, &shapes);
    report.profile_bytes = profile_bytes(&manifest, &ids);
    if !opts.dry_run {
        report
            .write(out)
            .with_context(|| format!("writing the texture report into {}", out.display()))?;
    }
    Ok(report)
}

/// Every id with a `C5TX` variant, and the shapes of those variants.
fn read_textures(out: &Path, manifest: &Manifest) -> BTreeMap<LogicalId, Textures> {
    let all: Vec<(&LogicalId, &Variant)> = manifest
        .iter()
        .flat_map(|(id, vs)| vs.iter().map(move |v| (id, v)))
        .filter(|(_, v)| v.format != VariantFormat::Generic)
        .collect();
    let read: Vec<Option<(LogicalId, Variant, Shape)>> = all
        .par_iter()
        .map(|(id, v)| {
            let bytes = std::fs::read(out.join(v.hash.to_string())).ok()?;
            if !bytes.starts_with(&TEXTURE_MAGIC) {
                return None;
            }
            let blob = TextureBlob::decode(&bytes).ok()?;
            Some(((*id).clone(), (*v).clone(), Shape::of(&blob)))
        })
        .collect();
    let mut map: BTreeMap<LogicalId, Textures> = BTreeMap::new();
    for (id, v, sh) in read.into_iter().flatten() {
        map.entry(id).or_default().push((v, sh));
    }
    map
}

/// The variant the others are made from: the plain one when there is one (lossless), else the
/// BC original, else whatever there is.
fn source(list: &Textures) -> Option<&(Variant, Shape)> {
    let rank = |b: &Shape| match b.format {
        TextureFormat::Rgba8 => 0,
        TextureFormat::Bc1 | TextureFormat::Bc2 | TextureFormat::Bc3 => 1,
        TextureFormat::Bc7 => 2,
        _ => 3,
    };
    list.iter().min_by_key(|(_, b)| rank(b))
}

fn cached_encode(
    cache: &BuildCache<'_>,
    store: &BlobStore,
    job: &Job,
    dry_run: bool,
) -> Result<Outcome> {
    let ui = is_ui(job.id.as_str());
    let class = Class::of(job.id.as_str());
    let params = [
        job.target as u8,
        u8::from(ui),
        class as u8,
        (MOBILE_MAX_SIDE >> 8) as u8,
    ];
    let key = CacheKey::new("texture-variant", STEP_VERSION, &[job.source_hash], &params);
    if let Some(o) = cache.get::<Outcome>(&key) {
        return Ok(o);
    }
    let path = store.blob_path(&job.source_hash);
    let source = std::fs::read(&path)
        .with_context(|| format!("reading {}", path.display()))
        .and_then(|b| TextureBlob::decode(&b).map_err(anyhow::Error::from))
        .with_context(|| format!("the source of {}", job.id.as_str()))?;
    let outcome = match make_variant(&source, job.target, ui, class) {
        Ok(m) => {
            let bytes = m.blob.encode();
            let (hash, size) = if dry_run {
                (
                    ContentHash(*blake3::hash(&bytes).as_bytes()),
                    bytes.len() as u64,
                )
            } else {
                store.put(&bytes)?
            };
            Outcome::Made {
                hash,
                size,
                width: m.blob.width,
                height: m.blob.height,
                psnr_rgb: m.psnr_rgb,
                psnr_alpha: m.psnr_alpha,
            }
        }
        Err(Rejected::Floor(why)) => Outcome::Failed { why, floor: true },
        Err(Rejected::Error(why)) => Outcome::Failed { why, floor: false },
    };
    // An encoder error (out of memory under load, say) is not kept: the next run tries again.
    let keep = !matches!(outcome, Outcome::Failed { floor: false, .. });
    if !dry_run && keep {
        let outputs = match &outcome {
            Outcome::Made { hash, .. } => vec![*hash],
            Outcome::Failed { .. } => Vec::new(),
        };
        cache.insert(&key, &outcome, &outputs)?;
    }
    Ok(outcome)
}

/// A new variant and how close it is to its source.
#[derive(Debug)]
pub struct Made {
    pub blob: TextureBlob,
    pub psnr_rgb: f64,
    pub psnr_alpha: f64,
}

/// Why a variant was not made.
#[derive(Debug, Clone, PartialEq)]
pub enum Rejected {
    /// It was made and misses the PSNR floor of its class: the texture keeps what it had (a
    /// written exception, listed with its numbers in the report).
    Floor(String),
    /// It could not be made (decoder, encoder); tried again on the next run.
    Error(String),
}

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Floor(w) | Self::Error(w) => f.write_str(w),
        }
    }
}

/// Encodes `src` as `target` (BC7 or ASTC 4x4) and checks it against the source pixels. `ui`: level
/// 0 only, padded to whole blocks. `Err` when it cannot be made or misses the floor of `class`.
pub fn make_variant(
    src: &TextureBlob,
    target: TextureFormat,
    ui: bool,
    class: Class,
) -> Result<Made, Rejected> {
    let error = |m: String| Rejected::Error(m);
    let slices = match src.dim {
        TexDim::D2 => 1,
        TexDim::Cube => 6,
        TexDim::D3 => return Err(error("volumes are not encoded".into())),
    };
    let levels = if ui { 1 } else { src.mips.len() };
    // chains[slice][level]
    let mut chains: Vec<Vec<Rgba8Image>> = vec![Vec::new(); slices];
    for level in 0..levels {
        let (w, h) = ((src.width >> level).max(1), (src.height >> level).max(1));
        let data = &src.mips[level];
        let per = data.len() / slices;
        for (s, chain) in chains.iter_mut().enumerate() {
            let img = decode_slice(src.format, src.srgb, w, h, &data[s * per..(s + 1) * per])
                .map_err(|e| error(format!("decoding the source: {e}")))?;
            chain.push(img);
        }
    }
    let (mut width, mut height) = (src.width, src.height);
    if target == TextureFormat::Astc4x4 && width.max(height) > MOBILE_MAX_SIDE {
        // Option A: phones drop the top level of a large texture.
        let (hw, hh) = (width.div_ceil(2), height.div_ceil(2));
        if ui || (hw.is_multiple_of(4) && hh.is_multiple_of(4)) {
            for chain in &mut chains {
                if chain.len() > 1 && !ui {
                    chain.remove(0);
                } else {
                    chain[0] = halve(&chain[0]);
                    chain.truncate(1);
                }
            }
            (width, height) = (chains[0][0].width, chains[0][0].height);
        }
    }
    if ui && (!width.is_multiple_of(4) || !height.is_multiple_of(4)) {
        for chain in &mut chains {
            chain[0] = pad_edges(&chain[0]);
        }
        (width, height) = (pad_to_block(width), pad_to_block(height));
    }
    let space = if src.srgb {
        ColorSpace::Srgb
    } else {
        ColorSpace::Linear
    };
    let mut encoded: Vec<TextureBlob> = Vec::with_capacity(slices);
    let (mut rgb, mut alpha) = (f64::INFINITY, f64::INFINITY);
    for chain in &chains {
        let blob = encode_chain_keep_rgb(chain, space, target).map_err(|e| error(e.to_string()))?;
        for (level, want) in chain.iter().enumerate().take(2) {
            let (w, h) = (want.width, want.height);
            let got = decode_slice(target, blob.srgb, w, h, &blob.mips[level])
                .map_err(|e| error(format!("decoding the encode: {e}")))?;
            let (r, a) = psnr_split(&want.pixels, &got.pixels, class == Class::Ui);
            rgb = rgb.min(r);
            alpha = alpha.min(a);
        }
        encoded.push(blob);
    }
    let floor = class.floor(target);
    if rgb < floor || alpha < floor {
        return Err(Rejected::Floor(format!(
            "{rgb:.1} dB rgb, {alpha:.1} dB alpha, under the {floor:.0} dB floor of {class:?}"
        )));
    }
    let mips = (0..encoded[0].mips.len())
        .map(|level| encoded.iter().flat_map(|b| b.mips[level].clone()).collect())
        .collect();
    let blob = TextureBlob {
        format: target,
        dim: src.dim,
        srgb: src.srgb,
        width,
        height,
        depth: 1,
        mips,
    };
    Ok(Made {
        blob,
        psnr_rgb: rgb,
        psnr_alpha: alpha,
    })
}

/// PSNR of RGB and of alpha. `premultiplied`: RGB is compared as it shows once blended (times
/// alpha), for the UI, whose transparent texels hold colours nobody sees; a world texture's alpha
/// is often a separate mask, so its RGB counts everywhere.
fn psnr_split(a: &[u8], b: &[u8], premultiplied: bool) -> (f64, f64) {
    let pick = |px: &[u8], c: &[usize]| -> Vec<u8> {
        px.chunks_exact(4)
            .flat_map(|p| {
                c.iter().map(move |&i| {
                    if premultiplied && i < 3 {
                        ((u32::from(p[i]) * u32::from(p[3]) + 127) / 255) as u8
                    } else {
                        p[i]
                    }
                })
            })
            .collect()
    };
    (
        psnr(&pick(a, &[0, 1, 2]), &pick(b, &[0, 1, 2])),
        psnr(&pick(a, &[3]), &pick(b, &[3])),
    )
}

/// Half size, each side rounded up, by a 2x2 box over the raw values (the edge texel repeats on an
/// odd side). The textures keep their values as the original has them (AB-D2), so no gamma here.
fn halve(src: &Rgba8Image) -> Rgba8Image {
    let (sw, sh) = (src.width as usize, src.height as usize);
    let (dw, dh) = (sw.div_ceil(2), sh.div_ceil(2));
    let mut out = vec![0u8; dw * dh * 4];
    for y in 0..dh {
        for x in 0..dw {
            for c in 0..4 {
                let mut sum = 0u32;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let sx = (2 * x + dx).min(sw - 1);
                    let sy = (2 * y + dy).min(sh - 1);
                    sum += u32::from(src.pixels[(sy * sw + sx) * 4 + c]);
                }
                out[(y * dw + x) * 4 + c] = ((sum + 2) / 4) as u8;
            }
        }
    }
    Rgba8Image::new(dw as u32, dh as u32, out)
}

/// `src` grown to whole 4x4 blocks, the new texels copies of the nearest edge texel: what a clamped
/// sampler reads past the edge anyway, so filtering at the image's border is unchanged.
fn pad_edges(src: &Rgba8Image) -> Rgba8Image {
    let (sw, sh) = (src.width as usize, src.height as usize);
    let (dw, dh) = (
        pad_to_block(src.width) as usize,
        pad_to_block(src.height) as usize,
    );
    let mut out = vec![0u8; dw * dh * 4];
    for y in 0..dh {
        let sy = y.min(sh - 1);
        for x in 0..dw {
            let sx = x.min(sw - 1);
            out[(y * dw + x) * 4..][..4].copy_from_slice(&src.pixels[(sy * sw + sx) * 4..][..4]);
        }
    }
    Rgba8Image::new(dw as u32, dh as u32, out)
}

/// The three devices.
pub fn device_profiles() -> [(&'static str, PlatformProfile); 3] {
    [
        ("BC device", PlatformProfile::bc_device()),
        ("ASTC device", PlatformProfile::astc_device()),
        ("neither", PlatformProfile::no_compression()),
    ]
}

/// Every texture a BC device gets as a plain `Rgba8`, or an ASTC device gets other than as ASTC,
/// with the rule that keeps it so (or the failed encode).
fn kept_list(
    manifest: &Manifest,
    ids: &[&LogicalId],
    shapes: &BTreeMap<ContentHash, Shape>,
    rules: &Rules,
    failed: &[Failed],
) -> Vec<Kept> {
    let mut out = Vec::new();
    for id in ids {
        let s = id.as_str();
        let pick = |p: &PlatformProfile| manifest.resolve(id, p);
        let src = manifest
            .variants(id)
            .iter()
            .filter_map(|v| shapes.get(&v.hash))
            .min_by_key(|sh| match sh.format {
                TextureFormat::Rgba8 => 0,
                TextureFormat::Bc1 | TextureFormat::Bc2 | TextureFormat::Bc3 => 1,
                _ => 2,
            });
        let Some(&src) = src else { continue };
        let mut devices = Vec::new();
        let mut reasons = Vec::new();
        let why = |target: TextureFormat| {
            rules.exception(s, src, target).or_else(|| {
                failed
                    .iter()
                    .find(|f| f.id == s && f.format == target)
                    .map(|f| {
                        let what = if f.floor {
                            "under its PSNR floor"
                        } else {
                            "failed encode"
                        };
                        format!("{what}: {}", f.why)
                    })
            })
        };
        if let Some(v) = pick(&PlatformProfile::bc_device())
            && v.format == VariantFormat::Uncompressed
        {
            devices.push("desktop");
            reasons.push(why(TextureFormat::Bc7).unwrap_or_else(|| "no rule".into()));
        }
        if let Some(v) = pick(&PlatformProfile::astc_device())
            && v.format != VariantFormat::Astc
        {
            devices.push("phone");
            let r = why(TextureFormat::Astc4x4).unwrap_or_else(|| "no rule".into());
            if !reasons.contains(&r) {
                reasons.push(r);
            }
        }
        if devices.is_empty() {
            continue;
        }
        out.push(Kept {
            id: s.to_owned(),
            texture: format!("{:?} {}x{}", src.format, src.width, src.height),
            devices: devices.join("+"),
            reason: reasons.join("; "),
        });
    }
    out
}

fn variant_bytes(
    manifest: &Manifest,
    ids: &[&LogicalId],
    shapes: &BTreeMap<ContentHash, Shape>,
) -> BTreeMap<String, (u64, u64)> {
    let mut seen = BTreeSet::new();
    let mut out: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for id in ids {
        for v in manifest.variants(id) {
            if !seen.insert(v.hash) {
                continue;
            }
            let f = shapes
                .get(&v.hash)
                .map_or("?".to_owned(), |s| format!("{:?}", s.format));
            let e = out.entry(format!("{:?}/{f}", v.format)).or_default();
            e.0 += 1;
            e.1 += v.size;
        }
    }
    out
}

/// What each device downloads of the textures, every blob once.
pub fn profile_bytes(manifest: &Manifest, ids: &[&LogicalId]) -> BTreeMap<String, u64> {
    device_profiles()
        .iter()
        .map(|(name, p)| {
            let mut seen = BTreeSet::new();
            let bytes = ids
                .iter()
                .filter_map(|id| manifest.resolve(id, p))
                .filter(|v| seen.insert(v.hash))
                .map(|v| v.size)
                .sum();
            ((*name).to_owned(), bytes)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use asset_format::PrefetchGroup;

    fn rgba(w: u32, h: u32, levels: usize, f: impl Fn(u32, u32, usize) -> [u8; 4]) -> TextureBlob {
        let mips = (0..levels)
            .map(|l| {
                let (lw, lh) = ((w >> l).max(1), (h >> l).max(1));
                (0..lh)
                    .flat_map(|y| (0..lw).map(move |x| (x, y)))
                    .flat_map(|(x, y)| f(x, y, l))
                    .collect()
            })
            .collect();
        TextureBlob::new_2d(TextureFormat::Rgba8, false, w, h, mips)
    }

    fn smooth(x: u32, y: u32, l: usize) -> [u8; 4] {
        let s = 1u32 << l;
        [
            (x * s * 3 % 256) as u8,
            (y * s * 2 % 256) as u8,
            90,
            (200 - (x * s % 64)) as u8,
        ]
    }

    fn shape(format: TextureFormat, dim: TexDim, w: u32, h: u32) -> Shape {
        Shape {
            format,
            dim,
            width: w,
            height: h,
        }
    }

    #[test]
    fn the_rules_name_why_a_texture_keeps_no_variant() {
        let rules = Rules::new(
            Exceptions::builtin(),
            ["ui:tx:twcenmt16base.dds".to_owned()].into(),
            ["tx:grass_d.dds".to_owned()].into(),
        );
        let bc7 = TextureFormat::Bc7;
        let astc = TextureFormat::Astc4x4;
        let colour = shape(TextureFormat::Rgba8, TexDim::D2, 256, 256);
        // A plain colour texture gets both.
        assert_eq!(rules.exception("tx:warrior_diff.dds", colour, bc7), None);
        assert_eq!(rules.exception("tx:warrior_diff.dds", colour, astc), None);
        // A DXT original gets ASTC only.
        let dxt = shape(TextureFormat::Bc1, TexDim::D2, 256, 256);
        assert!(rules.exception("tx:a.dds", dxt, bc7).is_some());
        assert_eq!(rules.exception("tx:a.dds", dxt, astc), None);
        // The listed data tables, the fonts, the one-channel data, volumes, tiny and odd world
        // textures keep none.
        for (id, s) in [
            ("tx:df_house.dds", colour),
            ("ui:tx:twcenmt16base.dds", colour),
            (
                "tx:forest_tile1.dds",
                shape(TextureFormat::R8, TexDim::D2, 128, 128),
            ),
            (
                "tx:lut.dds",
                shape(TextureFormat::Rgba8, TexDim::D3, 32, 32),
            ),
            ("tx:dot.dds", shape(TextureFormat::Rgba8, TexDim::D2, 8, 8)),
            (
                "tx:odd.dds",
                shape(TextureFormat::Rgba8, TexDim::D2, 30, 32),
            ),
        ] {
            for t in [bc7, astc] {
                assert!(rules.exception(id, s, t).is_some(), "{id} {t:?}");
            }
        }
        // A UI image that is not a multiple of 4 is padded instead.
        let odd_ui = shape(TextureFormat::Rgba8, TexDim::D2, 30, 17);
        assert_eq!(rules.exception("ui:tx:button.dds", odd_ui, bc7), None);
        // An array layer: ASTC on phones, the set's format on desktop.
        assert!(rules.exception("tx:grass_d.dds@low", colour, bc7).is_some());
        assert_eq!(rules.exception("tx:grass_d.dds", colour, astc), None);
        // The b1 sets' BC7 gets ASTC from its decode.
        let b1 = shape(TextureFormat::Bc7, TexDim::D2, 512, 512);
        assert_eq!(rules.exception("tex:x.dds", b1, astc), None);
    }

    #[test]
    fn a_colour_texture_becomes_bc7_and_astc_above_the_floor() {
        let src = rgba(64, 64, 3, smooth);
        for target in [TextureFormat::Bc7, TextureFormat::Astc4x4] {
            let m = make_variant(&src, target, false, Class::Colour)
                .unwrap_or_else(|e| panic!("{target:?}: {e}"));
            assert_eq!(
                (m.blob.format, m.blob.width, m.blob.mips.len()),
                (target, 64, 3)
            );
            assert!(m.psnr_rgb >= Class::Colour.floor(target), "{target:?}");
            assert!(m.psnr_alpha >= Class::Colour.floor(target), "{target:?}");
            assert!(TextureBlob::decode(&m.blob.encode()).is_ok());
        }
    }

    #[test]
    fn a_dxt_original_is_decoded_and_encoded_as_astc() {
        // A BC1 texture: encode a smooth image to BC1 first, then make the phone variant of it.
        let img = rgba(32, 32, 1, |x, y, _| [(x * 8) as u8, (y * 8) as u8, 40, 255]);
        let bc1 = super::super::encode_chain(
            &[Rgba8Image::new(32, 32, img.mips[0].clone())],
            ColorSpace::Linear,
            TextureFormat::Bc1,
        )
        .unwrap();
        let m = make_variant(&bc1, TextureFormat::Astc4x4, false, Class::Colour).unwrap();
        assert_eq!((m.blob.format, m.blob.width), (TextureFormat::Astc4x4, 32));
        // Compared with the decoded DXT, not with the image before it.
        assert!(m.psnr_rgb > 30.0, "{}", m.psnr_rgb);
    }

    #[test]
    fn a_large_texture_drops_its_top_level_on_the_phone_only() {
        let src = rgba(2048, 16, 2, |x, _, _| [(x % 256) as u8, 0, 0, 255]);
        let astc = make_variant(&src, TextureFormat::Astc4x4, false, Class::Colour).unwrap();
        assert_eq!((astc.blob.width, astc.blob.height), (1024, 8));
        assert_eq!(astc.blob.mips.len(), 1);
        let bc7 = make_variant(&src, TextureFormat::Bc7, false, Class::Colour).unwrap();
        assert_eq!((bc7.blob.width, bc7.blob.height), (2048, 16));
        assert_eq!(bc7.blob.mips.len(), 2);
    }

    #[test]
    fn a_ui_image_is_padded_to_whole_blocks_with_its_edge_texels() {
        // 30x17, red left half, blue right half, level 0 only even if the blob has more.
        let src = rgba(30, 17, 2, |x, _, _| {
            if x < 15 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 255, 255]
            }
        });
        let m = make_variant(&src, TextureFormat::Bc7, true, Class::Ui).unwrap();
        assert_eq!(
            (m.blob.width, m.blob.height, m.blob.mips.len()),
            (32, 20, 1)
        );
        assert_eq!(
            asset_format::stored_content((30, 17), (32, 20)),
            (30, 17),
            "the client finds the image part"
        );
        // The padding repeats the edge: the decoded corner is blue.
        let px = decode_slice(TextureFormat::Bc7, false, 32, 20, &m.blob.mips[0]).unwrap();
        let at = |x: usize, y: usize| &px.pixels[(y * 32 + x) * 4..][..4];
        assert!(
            at(31, 19)[2] > 240 && at(31, 19)[0] < 15,
            "{:?}",
            at(31, 19)
        );
        assert!(at(0, 19)[0] > 240, "{:?}", at(0, 19));
    }

    #[test]
    fn a_large_ui_image_is_halved_and_padded_for_the_phone() {
        let src = rgba(1030, 6, 1, |_, _, _| [10, 20, 30, 255]);
        let m = make_variant(&src, TextureFormat::Astc4x4, true, Class::Ui).unwrap();
        assert_eq!((m.blob.width, m.blob.height), (516, 4));
        assert_eq!(asset_format::stored_content((1030, 6), (516, 4)), (515, 3));
    }

    #[test]
    fn a_cube_is_encoded_face_by_face() {
        let face = rgba(16, 16, 2, smooth);
        let mut cube = face.clone();
        cube.dim = TexDim::Cube;
        cube.mips = face.mips.iter().map(|m| m.repeat(6)).collect();
        let m = make_variant(&cube, TextureFormat::Astc4x4, false, Class::Colour).unwrap();
        assert_eq!(m.blob.dim, TexDim::Cube);
        assert_eq!(
            m.blob.mips[0].len(),
            6 * TextureFormat::Astc4x4.mip_size(16, 16)
        );
        assert!(TextureBlob::decode(&m.blob.encode()).is_ok());
    }

    #[test]
    fn noise_misses_the_floor_and_is_not_added() {
        let src = rgba(32, 32, 1, |x, y, _| {
            let h =
                (x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663)).wrapping_mul(0x5bd1_e995);
            [h as u8, (h >> 8) as u8, (h >> 16) as u8, (h >> 24) as u8]
        });
        let e = make_variant(&src, TextureFormat::Astc4x4, false, Class::Data).unwrap_err();
        assert!(matches!(e, Rejected::Floor(_)), "{e}");
    }

    fn pack(dir: &Path, blobs: &[(&str, TextureBlob)]) {
        let store = BlobStore::open(dir).unwrap();
        let mut b = ManifestBuilder::new();
        for (id, blob) in blobs {
            let (hash, size) = store.put(&blob.encode()).unwrap();
            b.add(
                LogicalId::new(*id),
                Variant {
                    format: variant_format(blob.format),
                    hash,
                    size,
                    prefetch: PrefetchGroup(1),
                },
            );
        }
        b.finish(&store).unwrap();
    }

    #[test]
    fn a_pack_gets_its_variants_once_and_every_texture_resolves_for_every_device() {
        let dir = std::env::temp_dir().join(format!("variants-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dxt = super::super::encode_chain(
            &[Rgba8Image::new(
                16,
                16,
                rgba(16, 16, 1, smooth).mips[0].clone(),
            )],
            ColorSpace::Linear,
            TextureFormat::Bc1,
        )
        .unwrap();
        let r8 = TextureBlob::new_2d(TextureFormat::R8, false, 8, 8, vec![vec![7; 64]]);
        pack(
            &dir,
            &[
                ("tx:unit_diff.dds", rgba(32, 32, 2, smooth)),
                ("tx:rock.dds", dxt.clone()),
                ("tx:forest_tile1.dds", r8),
                ("tx:df_city.dds", rgba(16, 16, 1, smooth)),
            ],
        );
        let r = add_variants(&dir, &Options::default()).unwrap();
        let added: Vec<(&str, TextureFormat)> =
            r.added.iter().map(|a| (a.id.as_str(), a.format)).collect();
        assert_eq!(
            added,
            [
                ("tx:rock.dds", TextureFormat::Astc4x4),
                ("tx:unit_diff.dds", TextureFormat::Bc7),
                ("tx:unit_diff.dds", TextureFormat::Astc4x4),
            ]
        );
        let m = crate::report::budget::read_manifest(&dir).unwrap();
        // The original and the plain fallback stay.
        let rock = m.variants(&LogicalId::new("tx:rock.dds"));
        assert!(rock.iter().any(|v| v.format == VariantFormat::Bc));
        let unit = m.variants(&LogicalId::new("tx:unit_diff.dds"));
        assert!(unit.iter().any(|v| v.format == VariantFormat::Uncompressed));
        assert!(unit.iter().all(|v| v.prefetch == PrefetchGroup(1)));
        for (_, p) in device_profiles() {
            for id in [
                "tx:unit_diff.dds",
                "tx:rock.dds",
                "tx:forest_tile1.dds",
                "tx:df_city.dds",
            ] {
                assert!(m.resolve(&LogicalId::new(id), &p).is_some(), "{id}");
            }
        }
        // The kept list: the mask and the CPU-read field, each with its rule.
        let kept: Vec<&str> = r.kept.iter().map(|k| k.id.as_str()).collect();
        assert_eq!(kept, ["tx:df_city.dds", "tx:forest_tile1.dds"]);
        assert!(r.kept.iter().all(|k| k.reason != "no rule"));
        assert!(r.variant_bytes.contains_key("Astc/Astc4x4"));
        assert!(dir.join(REPORT_TXT).is_file());
        // A second run adds nothing and leaves the manifest as it is.
        let pointer = std::fs::read_to_string(dir.join("manifest")).unwrap();
        let r2 = add_variants(&dir, &Options::default()).unwrap();
        assert!(r2.added.is_empty());
        assert_eq!(
            std::fs::read_to_string(dir.join("manifest")).unwrap(),
            pointer
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
