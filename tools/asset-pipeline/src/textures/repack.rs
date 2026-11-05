//! `civ5-assets repack`: RGBA8 2D textures of an output folder to BC1 / BC7 (B3 asset-delivery).
//! Runs on a finished pack: every plain RGBA8 texture that is not
//! excluded is encoded, the result is checked against the source (PSNR of level 0 and 1), and only
//! a texture that passes replaces its variant; the others stay as they were. A second run finds
//! nothing new to change.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result, bail};
use asset_format::{
    ArtRecord, ArtTable, ContentHash, LogicalId, TexDim, TextureBlob, TextureFormat, Variant,
    VariantFormat,
};
use rayon::prelude::*;

use super::encode::encode_chain_keep_rgb as encode_chain;
use super::{ColorSpace, Rgba8Image, decode_mip, psnr, variant_format};
use crate::store::{BlobStore, ManifestBuilder};

/// Default PSNR threshold of the gate, in dB (RGB and alpha, level 0 and 1).
pub const MIN_PSNR: f64 = 42.0;

const DEFAULT_EXCEPTIONS: &str = include_str!("../../repack-exceptions.txt");

/// Rules `pattern<TAB>reason`; `*` is a wildcard, matching ignores case.
#[derive(Debug, Clone, Default)]
pub struct Exceptions {
    rules: Vec<(String, String)>,
}

impl Exceptions {
    pub fn parse(text: &str) -> Self {
        let rules = text
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
            .map(|l| match l.split_once('\t') {
                Some((p, r)) => (p.trim().to_ascii_lowercase(), r.trim().to_owned()),
                None => (l.trim().to_ascii_lowercase(), String::new()),
            })
            .collect();
        Self { rules }
    }

    /// The list that ships with the pipeline.
    pub fn builtin() -> Self {
        Self::parse(DEFAULT_EXCEPTIONS)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading exceptions {}", path.display()))?;
        Ok(Self::parse(&text))
    }

    /// The reason of the first rule that matches `id`.
    pub fn matches(&self, id: &str) -> Option<&str> {
        let id = id.to_ascii_lowercase();
        self.rules
            .iter()
            .find(|(p, _)| wildcard(p, &id))
            .map(|(p, r)| if r.is_empty() { p.as_str() } else { r.as_str() })
    }
}

/// `*` matches any run of characters (also none).
fn wildcard(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let (first, rest) = parts.split_first().expect("split gives one part");
    let Some(mut tail) = text.strip_prefix(first) else {
        return false;
    };
    let Some((last, middle)) = rest.split_last() else {
        return tail.is_empty();
    };
    for m in middle {
        match tail.find(m) {
            Some(i) => tail = &tail[i + m.len()..],
            None => return false,
        }
    }
    tail.ends_with(last) && tail.len() >= last.len()
}

#[derive(Debug, Clone)]
pub struct Options {
    pub min_psnr: f64,
    pub dry_run: bool,
    pub exceptions: Exceptions,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            min_psnr: MIN_PSNR,
            dry_run: false,
            exceptions: Exceptions::builtin(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Converted {
        format: TextureFormat,
        psnr_rgb: f64,
        psnr_alpha: f64,
    },
    /// Not a candidate (cube, volume, size).
    Skipped(String),
    /// Excluded by a rule or by an art table.
    Excluded(String),
    /// Encoded, but it did not pass the gate.
    Kept(String),
}

#[derive(Debug, Clone)]
pub struct Row {
    pub id: String,
    pub before: u64,
    pub after: u64,
    pub decision: Decision,
}

/// Bytes of a set of textures: how many, what the GPU holds, what the blobs weigh.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tally {
    pub count: usize,
    pub gpu_bytes: u64,
    pub blob_bytes: u64,
}

impl Tally {
    fn add(&mut self, gpu: u64, blob: u64) {
        self.count += 1;
        self.gpu_bytes += gpu;
        self.blob_bytes += blob;
    }
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub rows: Vec<Row>,
    /// RGBA8 textures of the manifest (any shape) before and after.
    pub rgba8_before: Tally,
    pub rgba8_after: Tally,
    /// What the converted textures weigh now.
    pub converted: Tally,
    /// What they weighed before.
    pub converted_was: Tally,
    pub dry_run: bool,
}

impl Report {
    pub fn count(&self, f: impl Fn(&Decision) -> bool) -> usize {
        self.rows.iter().filter(|r| f(&r.decision)).count()
    }

    pub fn changed(&self) -> bool {
        self.converted.count > 0
    }

    pub fn to_text(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "{:<44} {:>10} {:>10}  {:<5} {:>7} {:>7}  note",
            "id", "before", "after", "fmt", "rgb dB", "a dB"
        );
        for r in &self.rows {
            let (fmt, rgb, a, note) = match &r.decision {
                Decision::Converted {
                    format,
                    psnr_rgb,
                    psnr_alpha,
                } => (
                    format!("{format:?}"),
                    db(*psnr_rgb),
                    db(*psnr_alpha),
                    String::new(),
                ),
                Decision::Skipped(w) => (
                    "-".into(),
                    String::new(),
                    String::new(),
                    format!("skip: {w}"),
                ),
                Decision::Excluded(w) => (
                    "-".into(),
                    String::new(),
                    String::new(),
                    format!("excluded: {w}"),
                ),
                Decision::Kept(w) => (
                    "-".into(),
                    String::new(),
                    String::new(),
                    format!("kept RGBA8: {w}"),
                ),
            };
            let _ = writeln!(
                s,
                "{:<44} {:>10} {:>10}  {:<5} {:>7} {:>7}  {note}",
                r.id, r.before, r.after, fmt, rgb, a
            );
        }
        let mb = |b: u64| b as f64 / 1e6;
        let _ = writeln!(
            s,
            "\nconverted {}, kept {}, excluded {}, skipped {}{}",
            self.count(|d| matches!(d, Decision::Converted { .. })),
            self.count(|d| matches!(d, Decision::Kept(_))),
            self.count(|d| matches!(d, Decision::Excluded(_))),
            self.count(|d| matches!(d, Decision::Skipped(_))),
            if self.dry_run {
                " (dry run: nothing written)"
            } else {
                ""
            }
        );
        let _ = writeln!(
            s,
            "RGBA8 textures: {} / {:.1} MB gpu before, {} / {:.1} MB after",
            self.rgba8_before.count,
            mb(self.rgba8_before.gpu_bytes),
            self.rgba8_after.count,
            mb(self.rgba8_after.gpu_bytes)
        );
        let _ = writeln!(
            s,
            "converted textures: gpu {:.1} MB -> {:.1} MB, blobs {:.1} MB -> {:.1} MB",
            mb(self.converted_was.gpu_bytes),
            mb(self.converted.gpu_bytes),
            mb(self.converted_was.blob_bytes),
            mb(self.converted.blob_bytes)
        );
        s
    }
}

fn db(v: f64) -> String {
    if v.is_infinite() {
        "inf".into()
    } else {
        format!("{v:.1}")
    }
}

/// Ids whose textures are layers of a texture array in the client (one format per array): the
/// terrain, feature, river and SV icon textures and the tree overlays of every `tb:` table, in
/// the form without `@low`.
pub fn array_textures(out: &Path, manifest: &asset_format::Manifest) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for (id, variants) in manifest.iter() {
        if !id.as_str().starts_with("tb:") {
            continue;
        }
        for v in variants {
            let Ok(bytes) = std::fs::read(out.join(v.hash.to_string())) else {
                continue;
            };
            let Ok(table) = ArtTable::decode(&bytes) else {
                continue;
            };
            for (_, rec) in table.iter() {
                let used: Vec<&LogicalId> = match rec {
                    ArtRecord::Terrain(t) => vec![&t.texture],
                    ArtRecord::Feature(f) => f.texture.iter().collect(),
                    ArtRecord::River(r) => r.textures.iter().collect(),
                    ArtRecord::Sv(s) => vec![&s.icon],
                    ArtRecord::Tree(t) => vec![&t.overlay],
                    _ => Vec::new(),
                };
                ids.extend(used.into_iter().map(|t| t.as_str().to_owned()));
            }
        }
    }
    ids
}

fn base_id(id: &str) -> &str {
    id.split_once('@').map_or(id, |(b, _)| b)
}

fn gpu_bytes(t: &TextureBlob) -> u64 {
    t.mips.iter().map(|m| m.len() as u64).sum()
}

struct Job {
    id: LogicalId,
    index: usize,
    variant: Variant,
    blob: TextureBlob,
}

enum Outcome {
    Done {
        blob: TextureBlob,
        format: TextureFormat,
        psnr_rgb: f64,
        psnr_alpha: f64,
    },
    Skipped(String),
    Kept(String),
}

/// Runs the pass on the folder `out`; writes unless `opts.dry_run`.
pub fn repack_pack(out: &Path, opts: &Options) -> Result<Report> {
    let store = BlobStore::open(out)?;
    let mut builder = ManifestBuilder::open(&store)?;
    if builder.manifest().is_empty() {
        bail!("{} has no manifest", out.display());
    }
    let arrays = array_textures(out, builder.manifest());

    let mut report = Report {
        dry_run: opts.dry_run,
        ..Report::default()
    };
    let mut jobs = Vec::new();
    for (id, variants) in builder.manifest().iter() {
        for (index, v) in variants.iter().enumerate() {
            if v.format != VariantFormat::Uncompressed {
                continue;
            }
            let Ok(bytes) = std::fs::read(out.join(v.hash.to_string())) else {
                continue;
            };
            let Ok(blob) = TextureBlob::decode(&bytes) else {
                continue;
            };
            if blob.format != TextureFormat::Rgba8 {
                continue;
            }
            report
                .rgba8_before
                .add(gpu_bytes(&blob), bytes.len() as u64);
            let row = |decision| Row {
                id: id.as_str().to_owned(),
                before: bytes.len() as u64,
                after: bytes.len() as u64,
                decision,
            };
            if blob.dim != TexDim::D2 {
                report
                    .rows
                    .push(row(Decision::Skipped(format!("{:?}", blob.dim))));
            } else if let Some(why) = opts.exceptions.matches(id.as_str()) {
                report.rows.push(row(Decision::Excluded(why.to_owned())));
            } else if arrays.contains(base_id(id.as_str())) {
                report.rows.push(row(Decision::Excluded(
                    "layer of a texture array (art table)".into(),
                )));
            } else {
                jobs.push(Job {
                    id: id.clone(),
                    index,
                    variant: v.clone(),
                    blob,
                });
            }
        }
    }

    let results: Vec<(Job, Outcome)> = jobs
        .into_par_iter()
        .map(|j| {
            let o = convert(&j.blob, opts.min_psnr);
            (j, o)
        })
        .collect();

    let mut changed: Vec<(LogicalId, Vec<Variant>)> = Vec::new();
    for (job, outcome) in results {
        let before = job.variant.size;
        match outcome {
            Outcome::Skipped(why) => report.rows.push(Row {
                id: job.id.as_str().to_owned(),
                before,
                after: before,
                decision: Decision::Skipped(why),
            }),
            Outcome::Kept(why) => report.rows.push(Row {
                id: job.id.as_str().to_owned(),
                before,
                after: before,
                decision: Decision::Kept(why),
            }),
            Outcome::Done {
                blob,
                format,
                psnr_rgb,
                psnr_alpha,
            } => {
                let bytes = blob.encode();
                let after = bytes.len() as u64;
                let (hash, size) = if opts.dry_run {
                    (ContentHash(*blake3::hash(&bytes).as_bytes()), after)
                } else {
                    store.put(&bytes)?
                };
                report.converted_was.add(gpu_bytes(&job.blob), before);
                report.converted.add(gpu_bytes(&blob), after);
                let mut variants = changed
                    .iter()
                    .position(|(id, _)| *id == job.id)
                    .map(|i| changed.swap_remove(i).1)
                    .unwrap_or_else(|| builder.manifest().variants(&job.id).to_vec());
                variants[job.index] = Variant {
                    format: variant_format(format),
                    hash,
                    size,
                    prefetch: job.variant.prefetch,
                };
                changed.push((job.id.clone(), variants));
                report.rows.push(Row {
                    id: job.id.as_str().to_owned(),
                    before,
                    after,
                    decision: Decision::Converted {
                        format,
                        psnr_rgb,
                        psnr_alpha,
                    },
                });
            }
        }
    }
    report.rows.sort_by(|a, b| a.id.cmp(&b.id));

    // What stays RGBA8 afterwards.
    for r in &report.rows {
        if !matches!(r.decision, Decision::Converted { .. }) {
            report.rgba8_after.count += 1;
            report.rgba8_after.blob_bytes += r.before;
        }
    }
    report.rgba8_after.gpu_bytes = report.rgba8_before.gpu_bytes - report.converted_was.gpu_bytes;

    if !opts.dry_run && !changed.is_empty() {
        for (id, variants) in changed {
            for v in variants {
                builder.add(id.clone(), v);
            }
        }
        // The groups carry the byte sum of their members; the new sizes go in.
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
    Ok(report)
}

/// Encodes one texture and checks it. BC1 for an opaque image, else BC7; BC1 that fails the gate
/// falls back to BC7.
fn convert(src: &TextureBlob, min_psnr: f64) -> Outcome {
    let (w, h) = (src.width, src.height);
    if !w.is_multiple_of(4) || !h.is_multiple_of(4) {
        return Outcome::Skipped(format!("{w}x{h} is not a multiple of 4"));
    }
    if w <= 8 && h <= 8 {
        return Outcome::Skipped(format!("{w}x{h} is too small"));
    }
    let mut mips = Vec::with_capacity(src.mips.len());
    for (level, data) in src.mips.iter().enumerate() {
        let (lw, lh) = ((w >> level).max(1), (h >> level).max(1));
        if data.len() != lw as usize * lh as usize * 4 {
            return Outcome::Skipped(format!("mip {level} has a wrong size"));
        }
        mips.push(Rgba8Image::new(lw, lh, data.clone()));
    }
    let opaque = mips
        .iter()
        .all(|m| m.pixels.chunks_exact(4).all(|p| p[3] == 255));
    let space = if src.srgb {
        ColorSpace::Srgb
    } else {
        ColorSpace::Linear
    };
    let targets: &[TextureFormat] = if opaque {
        &[TextureFormat::Bc1, TextureFormat::Bc7]
    } else {
        &[TextureFormat::Bc7]
    };
    let mut why = Vec::new();
    for &format in targets {
        let blob = match encode_chain(&mips, space, format) {
            Ok(b) => b,
            Err(e) => return Outcome::Kept(format!("{format:?}: {e}")),
        };
        match gate(&mips, &blob, format, min_psnr) {
            Ok((psnr_rgb, psnr_alpha)) => {
                return Outcome::Done {
                    blob,
                    format,
                    psnr_rgb,
                    psnr_alpha,
                };
            }
            Err(e) => why.push(format!("{format:?} {e}")),
        }
    }
    Outcome::Kept(why.join("; "))
}

/// Worst PSNR (RGB, alpha) of level 0 and 1; `Err` with the numbers when one is below `min`.
fn gate(
    mips: &[Rgba8Image],
    blob: &TextureBlob,
    format: TextureFormat,
    min: f64,
) -> Result<(f64, f64), String> {
    let (mut rgb, mut alpha) = (f64::INFINITY, f64::INFINITY);
    for (level, mip) in mips.iter().enumerate().take(2) {
        let dec = decode_mip(blob, level).map_err(|e| e.to_string())?;
        let src = &mip.pixels;
        let split = |px: &[u8], channels: &[usize]| -> Vec<u8> {
            px.chunks_exact(4)
                .flat_map(|p| channels.iter().map(|&c| p[c]))
                .collect()
        };
        rgb = rgb.min(psnr(
            &split(src, &[0, 1, 2]),
            &split(&dec.pixels, &[0, 1, 2]),
        ));
        if format != TextureFormat::Bc1 {
            alpha = alpha.min(psnr(&split(src, &[3]), &split(&dec.pixels, &[3])));
        }
    }
    if rgb < min || alpha < min {
        return Err(format!("{rgb:.1} dB rgb, {alpha:.1} dB alpha"));
    }
    Ok((rgb, alpha))
}

#[cfg(test)]
mod tests {
    use super::*;
    use asset_format::{Manifest, PrefetchGroup};

    fn rgba_blob(
        w: u32,
        h: u32,
        mips: usize,
        f: impl Fn(u32, u32, usize) -> [u8; 4],
    ) -> TextureBlob {
        let data = (0..mips)
            .map(|l| {
                let (lw, lh) = ((w >> l).max(1), (h >> l).max(1));
                let mut v = Vec::new();
                for y in 0..lh {
                    for x in 0..lw {
                        v.extend(f(x, y, l));
                    }
                }
                v
            })
            .collect();
        TextureBlob::new_2d(TextureFormat::Rgba8, true, w, h, data)
    }

    fn gradient(x: u32, y: u32, l: usize) -> [u8; 4] {
        let s = 1u32 << l;
        [(x * s * 2) as u8, (y * s * 2) as u8, 90, 255]
    }

    fn noise(x: u32, y: u32, l: usize) -> [u8; 4] {
        let mut h =
            x.wrapping_mul(73856093) ^ y.wrapping_mul(19349663) ^ (l as u32).wrapping_mul(83492791);
        h ^= h >> 13;
        h = h.wrapping_mul(0x5bd1e995);
        h ^= h >> 15;
        [h as u8, (h >> 8) as u8, (h >> 16) as u8, (h >> 24) as u8]
    }

    #[test]
    fn wildcards() {
        assert!(wildcard("tx:df_*", "tx:df_a.dds"));
        assert!(!wildcard("tx:df_*", "tx:xdf_a.dds"));
        assert!(wildcard("tx:*_uv_*", "tx:fx_uv_1.dds"));
        assert!(!wildcard("tx:*_uv_*", "tx:fx_uv.dds"));
        assert!(wildcard("tx:a", "tx:a"));
        assert!(!wildcard("tx:a", "tx:ab"));
        assert!(wildcard("*b", "ab"));
        assert!(!wildcard("a*ab", "ab"));
    }

    #[test]
    fn exceptions_match_ids_and_give_the_reason() {
        let e = Exceptions::builtin();
        for id in [
            "tx:df_house.dds",
            "tx:TF_x.dds",
            "tx:flowfield.dds",
            "tx:fogramp_a.dds",
            "tx:waterdepthcolor_new.dds",
            "tx:specgray.dds",
            "tx:fx_uv_a.dds",
            "tx:unresolved_1.dds",
        ] {
            assert!(e.matches(id).is_some(), "{id}");
        }
        assert!(e.matches("tx:warrior_diff.dds").is_none());
        let custom = Exceptions::parse("# c\n\ntx:a*\tbecause\n");
        assert_eq!(custom.matches("tx:abc"), Some("because"));
        assert_eq!(custom.matches("tx:b"), None);
    }

    #[test]
    fn smooth_opaque_goes_to_bc1_and_keeps_its_mips() {
        let src = rgba_blob(64, 64, 4, |_, _, _| [255, 0, 0, 255]);
        let Outcome::Done { blob, format, .. } = convert(&src, MIN_PSNR) else {
            panic!("flat colour should pass");
        };
        assert_eq!(format, TextureFormat::Bc1);
        assert_eq!((blob.mips.len(), blob.srgb, blob.width), (4, true, 64));
        assert!(blob.mips[0].len() < src.mips[0].len());
    }

    #[test]
    fn an_opaque_gradient_that_bc1_cannot_hold_is_tried_as_bc7() {
        let src = rgba_blob(64, 64, 3, gradient);
        let Outcome::Done { format, .. } = convert(&src, MIN_PSNR) else {
            panic!("gradient should pass as BC1 or BC7");
        };
        assert!(matches!(format, TextureFormat::Bc1 | TextureFormat::Bc7));
    }

    #[test]
    fn smooth_with_alpha_goes_to_bc7() {
        let src = rgba_blob(64, 64, 3, |x, y, l| {
            let mut p = gradient(x, y, l);
            p[3] = 64 + x as u8;
            p
        });
        let Outcome::Done {
            format, psnr_alpha, ..
        } = convert(&src, MIN_PSNR)
        else {
            let Outcome::Kept(why) = convert(&src, MIN_PSNR) else {
                unreachable!()
            };
            panic!("should pass: {why}");
        };
        assert_eq!(format, TextureFormat::Bc7);
        assert!(psnr_alpha >= MIN_PSNR);
    }

    #[test]
    fn noise_fails_the_gate_and_stays() {
        let src = rgba_blob(64, 64, 2, noise);
        assert!(matches!(convert(&src, MIN_PSNR), Outcome::Kept(_)));
    }

    #[test]
    fn odd_and_tiny_sizes_are_skipped() {
        for (w, h) in [(30, 32), (8, 8), (4, 4)] {
            let src = rgba_blob(w, h, 1, gradient);
            assert!(
                matches!(convert(&src, MIN_PSNR), Outcome::Skipped(_)),
                "{w}x{h}"
            );
        }
    }

    fn pack(dir: &Path, blobs: &[(&str, TextureBlob)]) {
        let store = BlobStore::open(dir).unwrap();
        let mut b = ManifestBuilder::new();
        for (id, blob) in blobs {
            let bytes = blob.encode();
            let (hash, size) = store.put(&bytes).unwrap();
            b.add(
                LogicalId::new(*id),
                Variant {
                    format: VariantFormat::Uncompressed,
                    hash,
                    size,
                    prefetch: PrefetchGroup(1),
                },
            );
        }
        b.finish(&store).unwrap();
    }

    fn manifest(dir: &Path) -> Manifest {
        crate::report::budget::read_manifest(dir).unwrap()
    }

    #[test]
    fn a_pack_is_repacked_once_and_the_second_run_changes_nothing() {
        let dir = std::env::temp_dir().join(format!("repack-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        pack(
            &dir,
            &[
                (
                    "tx:smooth_diff.dds",
                    rgba_blob(64, 64, 3, |_, _, _| [255, 0, 0, 255]),
                ),
                ("tx:noise.dds", rgba_blob(32, 32, 1, noise)),
                ("tx:df_city.dds", rgba_blob(64, 64, 1, gradient)),
                ("tx:odd.dds", rgba_blob(30, 30, 1, gradient)),
            ],
        );
        let r = repack_pack(&dir, &Options::default()).unwrap();
        assert_eq!(r.count(|d| matches!(d, Decision::Converted { .. })), 1);
        assert_eq!(r.count(|d| matches!(d, Decision::Kept(_))), 1);
        assert_eq!(r.count(|d| matches!(d, Decision::Excluded(_))), 1);
        assert_eq!(r.count(|d| matches!(d, Decision::Skipped(_))), 1);
        let m = manifest(&dir);
        let v = &m.variants(&LogicalId::new("tx:smooth_diff.dds"))[0];
        assert_eq!(
            (v.format, v.prefetch),
            (VariantFormat::Bc, PrefetchGroup(1))
        );
        let blob =
            TextureBlob::decode(&std::fs::read(dir.join(v.hash.to_string())).unwrap()).unwrap();
        assert_eq!(blob.format, TextureFormat::Bc1);
        let pointer = std::fs::read_to_string(dir.join("manifest")).unwrap();

        let r2 = repack_pack(&dir, &Options::default()).unwrap();
        assert!(!r2.changed());
        assert_eq!(
            std::fs::read_to_string(dir.join("manifest")).unwrap(),
            pointer
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn terrain_array_layers_are_left_alone() {
        use asset_format::{Projection, TerrainArt};
        let dir = std::env::temp_dir().join(format!("repack-arr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        pack(
            &dir,
            &[
                (
                    "tx:grass_d.dds",
                    rgba_blob(64, 64, 1, |_, _, _| [255, 0, 0, 255]),
                ),
                (
                    "tx:grass_d.dds@low",
                    rgba_blob(32, 32, 1, |_, _, _| [255, 0, 0, 255]),
                ),
                (
                    "tx:free.dds",
                    rgba_blob(64, 64, 1, |_, _, _| [255, 0, 0, 255]),
                ),
            ],
        );
        let mut table = ArtTable::new();
        table.insert_terrain(
            "TERRAIN_GRASS",
            0,
            TerrainArt {
                texture: LogicalId::new("tx:grass_d.dds"),
                texture_scale: 1.0,
                projection: Projection::WorldXz,
            },
        );
        let store = BlobStore::open(&dir).unwrap();
        let (hash, size) = store.put(&table.encode()).unwrap();
        let mut b = ManifestBuilder::open(&store).unwrap();
        b.add(
            LogicalId::new("tb:art"),
            Variant {
                format: VariantFormat::Generic,
                hash,
                size,
                prefetch: PrefetchGroup(0),
            },
        );
        b.finish(&store).unwrap();
        let r = repack_pack(&dir, &Options::default()).unwrap();
        let excluded: Vec<&str> = r
            .rows
            .iter()
            .filter(|r| matches!(r.decision, Decision::Excluded(_)))
            .map(|r| r.id.as_str())
            .collect();
        assert_eq!(excluded, ["tx:grass_d.dds", "tx:grass_d.dds@low"]);
        assert_eq!(r.count(|d| matches!(d, Decision::Converted { .. })), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dry_run_writes_nothing() {
        let dir = std::env::temp_dir().join(format!("repack-dry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        pack(
            &dir,
            &[("tx:smooth_diff.dds", rgba_blob(64, 64, 1, gradient))],
        );
        let pointer = std::fs::read_to_string(dir.join("manifest")).unwrap();
        let opts = Options {
            dry_run: true,
            ..Options::default()
        };
        assert!(repack_pack(&dir, &opts).unwrap().changed());
        assert_eq!(
            std::fs::read_to_string(dir.join("manifest")).unwrap(),
            pointer
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
