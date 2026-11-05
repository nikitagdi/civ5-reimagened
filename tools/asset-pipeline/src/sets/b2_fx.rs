//! Set `b2-fx`: every `.psb` of the install as
//! a typed `fx:` effect ([`asset_format::FxEffect`], read by [`crate::fx::psb`]), the textures they
//! name as `tx:` (the original's bytes), and `fx:index`: the visual effects that name particles.
//! All of it is the map group `fx`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

use asset_format::{
    FX_INDEX_ID, FxEffect, FxShader, LogicalId, PrefetchGroup, Trigger, Variant, VariantFormat,
};

use super::tx_id;
use crate::fx::index::{build_index, fx_id, is_particles_file, merge_particles, parse_particles};
use crate::fx::psb::read_psb;
use crate::game::GameSource;
use crate::models::ModelError;
use crate::report::{ReportEntry, SetReport};
use crate::store::{BlobStore, ManifestBuilder, StoreStats};
use crate::textures::{dds_passthrough, variant_format};
use crate::world::art_table::{is_vis_effects_file, merge_vis_effects, parse_vis_effects};
use crate::world::groups::{self, GroupSpec};

pub const SET_NAME: &str = "b2-fx";

/// Name of the group of the set.
pub const GROUP_ID: &str = "fx";

/// Fetched by the group table (AB-D10).
const PREFETCH: PrefetchGroup = PrefetchGroup(0);

#[derive(Debug, Default)]
pub struct FxSummary {
    pub files: usize,
    pub emitters: usize,
    /// `.psb` files that did not read: name and why.
    pub failed: Vec<(String, String)>,
    pub textures: usize,
    pub missing_textures: Vec<String>,
    pub vis_effects: usize,
    /// `ART_DEF_PEFFECT_*` names without a built file.
    pub unbound_particles: Vec<String>,
    pub fx_bytes: u64,
    pub texture_bytes: BTreeMap<VariantFormat, u64>,
    pub index_bytes: u64,
    pub group_bytes: u64,
    pub report: SetReport,
    pub stats: StoreStats,
    pub seconds: f64,
}

/// Text of an XML file of the install (BOM removed).
fn xml_text(bytes: &[u8]) -> String {
    let t = String::from_utf8_lossy(bytes);
    t.trim_start_matches('\u{feff}').to_owned()
}

/// Builds the set into `out`.
pub fn build_fx(game: &GameSource, out: &Path) -> Result<FxSummary, ModelError> {
    let started = Instant::now();
    let store = BlobStore::open(out)?;
    let mut manifest = ManifestBuilder::open(&store)?;
    let mut s = FxSummary::default();
    let mut entries = Vec::new();

    // 1. Effects: every `.psb`; texture names resolved against the install.
    let has = |f: &str| game.lookup(f).is_some();
    let resolve = |f: &str| has(f).then(|| tx_id(f));
    let mut textures: BTreeSet<String> = BTreeSet::new();
    let mut missing: BTreeSet<String> = BTreeSet::new();
    let mut built: BTreeSet<LogicalId> = BTreeSet::new();
    let mut members: Vec<LogicalId> = Vec::new();
    let psbs: Vec<_> = game.iter_ext("psb").cloned().collect();
    for e in &psbs {
        let id = fx_id(&e.name);
        let stem = id.as_str().trim_start_matches("fx:").to_owned();
        let bytes = game.read(e)?;
        let (fx, notes) = match read_psb(&stem, &bytes, &resolve) {
            Ok(v) => v,
            Err(why) => {
                s.failed.push((e.name.clone(), why));
                continue;
            }
        };
        collect_textures(&fx, &mut textures);
        missing.extend(
            notes
                .missing_textures
                .iter()
                .map(|f| f.to_ascii_lowercase()),
        );
        for r in &notes.unknown_refs {
            entries.push(ReportEntry::new(
                "fx-unknown-ref",
                e.name.clone(),
                format!("ref asset {r} names no emitter of the file"),
            ));
        }
        s.files += 1;
        s.emitters += fx.emitters.len();
        let (hash, size) = store.put(&fx.encode())?;
        s.fx_bytes += size;
        manifest.add(
            id.clone(),
            Variant {
                format: VariantFormat::Generic,
                hash,
                size,
                prefetch: PREFETCH,
            },
        );
        built.insert(id.clone());
        members.push(id);
    }

    // 2. Textures: the original's bytes.
    for f in &textures {
        let Some(bytes) = game.read_by_name(f)? else {
            missing.insert(f.clone());
            continue;
        };
        let blob = match dds_passthrough(&bytes) {
            Ok(b) => b,
            Err(e) => {
                entries.push(ReportEntry::new(
                    "fx-texture-failed",
                    f.clone(),
                    e.to_string(),
                ));
                continue;
            }
        };
        let (hash, size) = store.put(&blob.encode())?;
        let format = variant_format(blob.format);
        *s.texture_bytes.entry(format).or_default() += size;
        let id = tx_id(f);
        manifest.add(
            id.clone(),
            Variant {
                format,
                hash,
                size,
                prefetch: PREFETCH,
            },
        );
        members.push(id);
        s.textures += 1;
    }

    // 3. The index of the visual effects.
    let mut vis_files = Vec::new();
    let mut particle_files = Vec::new();
    for e in game.iter_ext("xml") {
        if is_vis_effects_file(&e.name) {
            let rows = parse_vis_effects(&xml_text(&game.read(e)?))
                .map_err(|m| ModelError::invalid(format!("{}: {m}", e.name)))?;
            vis_files.push((e.name.to_ascii_lowercase(), rows));
        } else if is_particles_file(&e.name) {
            let rows = parse_particles(&xml_text(&game.read(e)?))
                .map_err(|m| ModelError::invalid(format!("{}: {m}", e.name)))?;
            particle_files.push((e.name.to_ascii_lowercase(), rows));
        }
    }
    let (vis, _) = merge_vis_effects(&vis_files);
    let particles = merge_particles(&particle_files);
    let (index, unbound) = build_index(&vis, &particles, &built);
    s.vis_effects = index.effects.len();
    s.unbound_particles = unbound;
    let (hash, size) = store.put(&index.encode())?;
    s.index_bytes = size;
    let index_id = LogicalId::new(FX_INDEX_ID);
    manifest.add(
        index_id.clone(),
        Variant {
            format: VariantFormat::Generic,
            hash,
            size,
            prefetch: PREFETCH,
        },
    );
    members.push(index_id);

    // 4. The group.
    members.sort();
    members.dedup();
    groups::publish(
        &store,
        &mut manifest,
        vec![GroupSpec::map(GROUP_ID, Trigger::Always, members)],
    )?;
    s.group_bytes = groups::load(&store, &manifest)?
        .get(GROUP_ID)
        .map_or(0, |g| g.bytes);
    manifest.finish(&store)?;

    // 5. The report.
    s.missing_textures = missing.into_iter().collect();
    for (f, why) in &s.failed {
        entries.push(ReportEntry::new("fx-failed", f.clone(), why.clone()));
    }
    for f in &s.missing_textures {
        entries.push(ReportEntry::new(
            "fx-texture-missing",
            f.clone(),
            "named by a .psb, not in the install (the original lacks it too); the frame has no texture and the renderer skips that particle quad (SIMP-exe-980)",
        ));
    }
    for p in &s.unbound_particles {
        entries.push(ReportEntry::new(
            "fx-particle-unbound",
            p.clone(),
            "a visual effect names it, but no .psb of it was built",
        ));
    }
    let mut report = SetReport {
        entries,
        ..SetReport::default()
    };
    for (k, v) in [
        ("psb files", s.files),
        ("emitters", s.emitters),
        ("failed", s.failed.len()),
        ("textures", s.textures),
        ("missing textures", s.missing_textures.len()),
        ("vis effects", s.vis_effects),
    ] {
        report.counts.insert(k.to_owned(), v as u64);
    }
    report.bytes.insert("fx".to_owned(), s.fx_bytes);
    report.bytes.insert("index".to_owned(), s.index_bytes);
    for (f, b) in &s.texture_bytes {
        report.bytes.insert(format!("textures {f:?}"), *b);
    }
    s.report = report;
    s.stats = store.stats();
    s.seconds = started.elapsed().as_secs_f64();
    Ok(s)
}

/// The texture files an effect names: frames and FakeVolume gradients, by their `tx:` ids.
fn collect_textures(fx: &FxEffect, out: &mut BTreeSet<String>) {
    let name = |id: &LogicalId| id.as_str().trim_start_matches("tx:").to_owned();
    for e in &fx.emitters {
        out.extend(e.frames.iter().filter_map(|f| f.texture.as_ref()).map(name));
        if let FxShader::FakeVolume { gradient: Some(g) } = &e.shader {
            out.insert(name(g));
        }
    }
}
