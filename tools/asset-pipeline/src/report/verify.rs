//! `civ5-assets verify`: the manifest of a B2 output directory is closed and has no stand-in
//! (9 "verify").
//!
//! It reads every blob of the manifest and checks:
//! - the blob file exists, has the size the manifest says and the hash it is named by;
//! - a B2 blob reads with its own type (`tx:` as `C5TX`, `tb:world-b2` as `C5WS`, `tb:groups-b2`
//!   as `C5GP`, `pc:` as `C5TP`); a B2 prefix without a reader here is a problem, not a skip;
//! - the texture ids the settings name are in the manifest;
//! - every member of every group is in the manifest, the byte sum of a group is what the table says,
//!   and the group the client loads first (`boot`) holds exactly the B2 ids with `prefetch = Boot`;
//! - the groups close: a model in a group has its textures and clips in the group or in a group it
//!   requires, a texture is in one style group at most (several: `style:shared`), every texture a
//!   terrain row names is in its continent's group or in `style:shared`;
//! - no stub event in `stubs-b2.json` that `stubs.allow` does not allow, and no unit in that list;
//! - every texture resolves for a BC device, an ASTC device and one with neither, each variant is
//!   filed under its format's family with a level 0 of whole blocks, and a device that gets a
//!   texture uncompressed has a written exception for it.
//!
//! The problems are collected, not thrown at the first one.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use asset_format::{
    BOOT_GROUP, ContentHash, GROUP_TABLE_ID, Group, GroupTable, LogicalId, Manifest, TEXTURE_MAGIC,
    TerrainPieceSet, TextureBlob, TextureFormat, Variant, VariantFormat, WORLD_SETTINGS_ID,
    WorldSettings,
};
use rayon::prelude::*;

use super::budget::{ReportError, is_b2_id, prefix_of, read_groups, read_manifest};
use super::stubs::{StubAllow, load_log, looks_like_unit};
use crate::sets::b2_terrain::{SHARED_GROUP, style_of_continent};
use crate::textures::variant_format;
use crate::textures::variants::{Failed, Rules, Shape, device_profiles};
use crate::world::groups::{BOOT, closure_gaps};

#[derive(Debug, Default)]
pub struct VerifyReport {
    /// Ids and blobs of the manifest.
    pub ids: usize,
    pub blobs: usize,
    /// B2 blobs that were read with their own type.
    pub decoded: usize,
    pub groups: usize,
    pub problems: Vec<String>,
}

impl VerifyReport {
    pub fn is_ok(&self) -> bool {
        self.problems.is_empty()
    }
}

/// What one blob gave.
#[derive(Default)]
struct Checked {
    problems: Vec<String>,
    decoded: bool,
    settings: Option<WorldSettings>,
    /// A `C5TX` blob (any id): its shape, for the device checks.
    texture: Option<(ContentHash, Shape)>,
}

fn check_blob(out: &Path, id: &LogicalId, v: &Variant) -> Checked {
    let mut c = Checked::default();
    let path = out.join(v.hash.to_string());
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            c.problems
                .push(format!("{}: blob {} is missing ({e})", id.as_str(), v.hash));
            return c;
        }
    };
    if bytes.len() as u64 != v.size {
        c.problems.push(format!(
            "{}: blob {} is {} B, the manifest says {} B",
            id.as_str(),
            v.hash,
            bytes.len(),
            v.size
        ));
    }
    if blake3::hash(&bytes).as_bytes() != v.hash.as_bytes() {
        c.problems.push(format!(
            "{}: the content of blob {} does not match its hash",
            id.as_str(),
            v.hash
        ));
        return c;
    }
    let s = id.as_str();
    if bytes.starts_with(&TEXTURE_MAGIC) {
        match TextureBlob::decode(&bytes) {
            Ok(t) => {
                c.problems.extend(texture_shape_problems(s, v, &t));
                c.texture = Some((v.hash, Shape::of(&t)));
            }
            Err(e) if !is_b2_id(s) => c.problems.push(format!("{s}: not a valid C5TX blob: {e}")),
            // A B2 id is read below with its own type.
            Err(_) => {}
        }
    }
    if !is_b2_id(s) {
        return c;
    }
    let read = |what: &str, r: Result<(), asset_format::FormatError>, c: &mut Checked| match r {
        Ok(()) => c.decoded = true,
        Err(e) => c
            .problems
            .push(format!("{s}: not a valid {what} blob: {e}")),
    };
    match prefix_of(s) {
        "tx" => read("C5TX", TextureBlob::decode(&bytes).map(drop), &mut c),
        "pc" => read("C5TP", TerrainPieceSet::decode(&bytes).map(drop), &mut c),
        "tb" if s == WORLD_SETTINGS_ID => match WorldSettings::decode(&bytes) {
            Ok(w) => {
                c.decoded = true;
                c.settings = Some(w);
            }
            Err(e) => c.problems.push(format!("{s}: not a valid C5WS blob: {e}")),
        },
        "tb" if s == GROUP_TABLE_ID => read("C5GP", GroupTable::decode(&bytes).map(drop), &mut c),
        "md" => read(
            "C5MD",
            asset_format::ModelBlobV2::decode(&bytes).map(drop),
            &mut c,
        ),
        "an" => read(
            "C5A2",
            asset_format::AnimPack::decode(&bytes).map(drop),
            &mut c,
        ),
        "fsm" => read(
            "C5FM",
            asset_format::IdleFsm::decode(&bytes).map(drop),
            &mut c,
        ),
        "tb" if s == "tb:art-b2" => read(
            "C5AT",
            asset_format::ArtTable::decode(&bytes).map(drop),
            &mut c,
        ),
        "fx" if s == asset_format::FX_INDEX_ID => read(
            "C5FI",
            asset_format::FxIndex::decode(&bytes).map(drop),
            &mut c,
        ),
        "fx" => read(
            "C5FX",
            asset_format::FxEffect::decode(&bytes).map(drop),
            &mut c,
        ),
        other => c.problems.push(format!(
            "{s}: verify has no reader for `{other}:` blobs yet"
        )),
    }
    c
}

/// A texture variant is filed under the family of its format, and a block-compressed one has a
/// level 0 of whole blocks (WebGPU creates it only so).
fn texture_shape_problems(id: &str, v: &Variant, t: &TextureBlob) -> Vec<String> {
    let mut p = Vec::new();
    let want = variant_format(t.format);
    if v.format != want {
        p.push(format!(
            "{id}: a {:?} texture is filed as {:?}, not {want:?}",
            t.format, v.format
        ));
    }
    let b = t.format.block_dim();
    // A BC1-BC5 original that is not whole blocks (a few UI DXT files) stays byte for byte; the
    // client decodes it on the CPU (render `upload_blob`). A variant the pipeline made must fit.
    let cpu_decoded = matches!(
        t.format,
        TextureFormat::Bc1
            | TextureFormat::Bc2
            | TextureFormat::Bc3
            | TextureFormat::Bc4
            | TextureFormat::Bc5
    );
    if b > 1 && !cpu_decoded && (!t.width.is_multiple_of(b) || !t.height.is_multiple_of(b)) {
        p.push(format!(
            "{id}: {:?} level 0 is {}x{}, not a multiple of the {b}x{b} block",
            t.format, t.width, t.height
        ));
    }
    p
}

/// Every texture resolves for the three devices: a BC device, an ASTC device and one with neither. A device must be able to show what it
/// gets: BC7 and ASTC have no CPU decoder in the client. A BC device that gets a plain texture, and
/// an ASTC device that gets anything but ASTC, must have a written exception ([`Rules`]).
fn texture_device_checks(
    out: &Path,
    manifest: &Manifest,
    shapes: &BTreeMap<ContentHash, Shape>,
    problems: &mut Vec<String>,
) {
    let rules = Rules::of_folder(out, manifest);
    // An encode under the PSNR floor of its class is the last written exception: the pass lists
    // each one with its numbers in its report.
    let floor_misses = Failed::floor_misses(out);
    for (id, variants) in manifest.iter() {
        let s = id.as_str();
        let mine: Vec<(&Variant, Shape)> = variants
            .iter()
            .filter_map(|v| shapes.get(&v.hash).map(|sh| (v, *sh)))
            .collect();
        // The source as the pass sees it: the plain one, else the BC original, else any.
        let Some(&(_, src)) = mine.iter().min_by_key(|(_, sh)| match sh.format {
            TextureFormat::Rgba8 => 0,
            TextureFormat::Bc1 | TextureFormat::Bc2 | TextureFormat::Bc3 => 1,
            _ => 2,
        }) else {
            continue;
        };
        for (device, profile) in device_profiles() {
            let Some(v) = manifest.resolve(id, &profile) else {
                problems.push(format!("{s}: no variant for a {device}"));
                continue;
            };
            let Some(got) = shapes.get(&v.hash) else {
                continue;
            };
            let shown = match got.format {
                TextureFormat::Bc7 => profile.texture_formats.first() == Some(&VariantFormat::Bc),
                TextureFormat::Astc4x4 => {
                    profile.texture_formats.first() == Some(&VariantFormat::Astc)
                }
                _ => true,
            };
            // SIMP-022: the b1 sets have BC7 and ASTC only; a device with neither shows flat colours.
            let b1 = s.starts_with("tex:") && device == "neither";
            if !shown && !b1 {
                problems.push(format!(
                    "{s}: a {device} gets {:?}, which it cannot show",
                    got.format
                ));
            }
            let (target, missed) = match device {
                "BC device" => (TextureFormat::Bc7, v.format == VariantFormat::Uncompressed),
                "ASTC device" => (TextureFormat::Astc4x4, v.format != VariantFormat::Astc),
                _ => continue,
            };
            if missed
                && rules.exception(s, src, target).is_none()
                && !floor_misses.contains(&(s.to_owned(), target))
            {
                problems.push(format!(
                    "{s}: a {device} gets {:?} {}x{} and no exception rule covers it (see {})",
                    got.format,
                    got.width,
                    got.height,
                    crate::textures::variants::REPORT_TXT
                ));
            }
        }
    }
}

fn settings_closure(settings: &WorldSettings, manifest: &Manifest, problems: &mut Vec<String>) {
    for (what, id) in settings.texture_refs() {
        if manifest.variants(&id).is_empty() {
            problems.push(format!(
                "{WORLD_SETTINGS_ID}: {what} names {}, which is not in the manifest",
                id.as_str()
            ));
        }
    }
}

/// The `.lean` list of the settings agrees with the textures it names: the
/// slopes are `Rgba16F`, the cross moment `R16F`, both with the size and the mip count of the
/// header. (Whether the ids are in the manifest is [`settings_closure`]'s.)
fn lean_checks(
    settings: &WorldSettings,
    out: &Path,
    manifest: &Manifest,
    problems: &mut Vec<String>,
) {
    let read = |id: &LogicalId| -> Option<TextureBlob> {
        let v = manifest.variants(id).first()?;
        TextureBlob::decode(&std::fs::read(out.join(v.hash.to_string())).ok()?).ok()
    };
    for l in &settings.lean {
        for (id, format, what) in [
            (&l.slopes, TextureFormat::Rgba16F, "slopes"),
            (&l.cross, TextureFormat::R16F, "cross"),
        ] {
            // A texture that is missing or does not read is reported by the checks above.
            let Some(t) = read(id) else { continue };
            if (t.format, t.width, t.height, t.mips.len())
                != (format, l.width, l.height, l.mips as usize)
            {
                problems.push(format!(
                    "{WORLD_SETTINGS_ID}: lean[{}] {what} {} is {:?} {}x{} with {} mips, the header says {format:?} {}x{} with {}",
                    l.file,
                    id.as_str(),
                    t.format,
                    t.width,
                    t.height,
                    t.mips.len(),
                    l.width,
                    l.height,
                    l.mips
                ));
            }
        }
    }
}

/// A group of the table implies the settings section its set writes: a settings blob left from
/// an older layout (or from a set that was built into an empty blob) is found here.
fn settings_complete(settings: &WorldSettings, table: &GroupTable, problems: &mut Vec<String>) {
    let mut need = |section: &str, empty: bool, because: &str| {
        if empty {
            problems.push(format!(
                "{WORLD_SETTINGS_ID}: `{section}` is empty but the table has {because}: build that set again"
            ));
        }
    };
    if table.get(BOOT_GROUP).is_some() {
        need(
            "fog_colors",
            settings.fog_colors.is_empty(),
            "the boot group",
        );
        need(
            "highlights",
            settings.highlights.is_empty(),
            "the boot group",
        );
        need(
            "color_keys",
            settings.color_keys.is_empty(),
            "the boot group",
        );
    }
    if table.groups.iter().any(|g| g.id.starts_with("style:")) {
        need(
            "terrain_defs",
            settings.terrain_defs.is_empty(),
            "style groups",
        );
    }
    if table.get("decals").is_some() {
        need("decals", settings.decals.is_empty(), "the decals group");
    }
    if table.get("water").is_some() {
        need("coast", settings.coast.styles.is_empty(), "the water group");
        need(
            "waves",
            settings.waves.crest_pages_in_use == 0,
            "the water group",
        );
        need(
            "ice",
            settings.ice.full.far_samples == 0.0,
            "the water group",
        );
        need("lean", settings.lean.is_empty(), "the water group");
    }
}

/// The style groups close (4.13; the shared group is the owner decision of the R4 packet, section 10
/// change log): a texture is in one style group at most and never in that group and `style:shared`;
/// each style group requires `style:shared` when it exists; every texture a terrain row names (and
/// its `@low`) is in the group of the row's continent or in `style:shared`.
fn style_checks(
    settings: &WorldSettings,
    table: &GroupTable,
    manifest: &Manifest,
    problems: &mut Vec<String>,
) {
    let styles: Vec<&Group> = table
        .groups
        .iter()
        .filter(|g| g.id.starts_with("style:") && g.id != SHARED_GROUP)
        .collect();
    if styles.is_empty() {
        return;
    }
    let shared = table.get(SHARED_GROUP);
    let in_shared: BTreeSet<&LogicalId> = shared.iter().flat_map(|g| g.members.iter()).collect();
    let mut owners: BTreeMap<&LogicalId, Vec<&str>> = BTreeMap::new();
    for g in &styles {
        if shared.is_some() && !g.requires.iter().any(|r| r == SHARED_GROUP) {
            problems.push(format!("group {} does not require {SHARED_GROUP}", g.id));
        }
        for m in g.members.iter().filter(|m| prefix_of(m.as_str()) == "tx") {
            owners.entry(m).or_default().push(&g.id);
            if in_shared.contains(m) {
                problems.push(format!(
                    "{} is in {} and in {SHARED_GROUP}",
                    m.as_str(),
                    g.id
                ));
            }
        }
    }
    for (id, groups) in &owners {
        if groups.len() > 1 {
            problems.push(format!(
                "{} is in the groups {}: a texture of several styles belongs in {SHARED_GROUP}",
                id.as_str(),
                groups.join(", ")
            ));
        }
    }
    for row in &settings.terrain_defs {
        let Some(style) = style_of_continent(row.continent) else {
            problems.push(format!(
                "{WORLD_SETTINGS_ID}: terrain row {} names the unknown continent {}",
                row.id, row.continent
            ));
            continue;
        };
        let group_id = format!("style:{}", style.name());
        let Some(group) = table.get(&group_id) else {
            problems.push(format!(
                "{WORLD_SETTINGS_ID}: terrain row {} (continent {}) needs the group {group_id}, which the table lacks",
                row.id, row.continent
            ));
            continue;
        };
        let held: BTreeSet<&LogicalId> = group.members.iter().collect();
        for id in [&row.diffuse, &row.heightmap, &row.specmap]
            .into_iter()
            .flatten()
        {
            let low = LogicalId::new(format!("{}@low", id.as_str()));
            let ids =
                std::iter::once(id).chain((!manifest.variants(&low).is_empty()).then_some(&low));
            for id in ids {
                if !held.contains(id) && !in_shared.contains(id) {
                    problems.push(format!(
                        "{} is named by terrain row {} (continent {}) but is in neither {group_id} nor {SHARED_GROUP}",
                        id.as_str(),
                        row.id,
                        row.continent
                    ));
                }
            }
        }
    }
}

fn group_checks(
    table: &GroupTable,
    manifest: &Manifest,
    boot_ids: &BTreeSet<&LogicalId>,
    problems: &mut Vec<String>,
) {
    if let Err(e) = table.check() {
        problems.push(format!("{GROUP_TABLE_ID}: {e}"));
    }
    for g in &table.groups {
        let mut sum = 0u64;
        for m in &g.members {
            let sizes: Vec<u64> = manifest.variants(m).iter().map(|v| v.size).collect();
            match sizes.iter().max() {
                Some(n) => sum += n,
                None => problems.push(format!(
                    "group {}: member {} is not in the manifest",
                    g.id,
                    m.as_str()
                )),
            }
        }
        if sum != g.bytes {
            problems.push(format!(
                "group {}: the table says {} B, the members sum to {sum} B",
                g.id, g.bytes
            ));
        }
    }
    // The boot group is the B2 ids with prefetch = Boot, and nothing else (the table itself is
    // fetched first through its own prefetch flag, but is not a member).
    match table.get(BOOT_GROUP) {
        None => {
            if !boot_ids.is_empty() {
                problems.push(format!(
                    "the manifest has {} id(s) with prefetch = Boot but the table has no `{BOOT_GROUP}` group",
                    boot_ids.len()
                ));
            }
        }
        Some(boot) => {
            let members: BTreeSet<&LogicalId> = boot.members.iter().collect();
            for id in boot_ids.difference(&members) {
                problems.push(format!(
                    "{} has prefetch = Boot but is not a member of `{BOOT_GROUP}`",
                    id.as_str()
                ));
            }
            for id in members.difference(boot_ids) {
                problems.push(format!(
                    "`{BOOT_GROUP}` holds {}, whose variant has no prefetch = Boot",
                    id.as_str()
                ));
            }
        }
    }
}

/// Checks the output directory `out` against `allow`.
pub fn verify(out: &Path, allow: &StubAllow) -> Result<VerifyReport, ReportError> {
    let manifest = read_manifest(out)?;
    let mut r = VerifyReport {
        ids: manifest.len(),
        ..VerifyReport::default()
    };
    let jobs: Vec<(&LogicalId, &Variant)> = manifest
        .iter()
        .flat_map(|(id, vs)| vs.iter().map(move |v| (id, v)))
        .collect();
    r.blobs = jobs.len();
    let checked: Vec<Checked> = jobs
        .par_iter()
        .map(|(id, v)| check_blob(out, id, v))
        .collect();
    let mut settings = None;
    let mut shapes = BTreeMap::new();
    for c in checked {
        r.problems.extend(c.problems);
        r.decoded += usize::from(c.decoded);
        if c.settings.is_some() {
            settings = c.settings;
        }
        shapes.extend(c.texture);
    }
    texture_device_checks(out, &manifest, &shapes, &mut r.problems);
    if let Some(s) = &settings {
        settings_closure(s, &manifest, &mut r.problems);
        lean_checks(s, out, &manifest, &mut r.problems);
    }

    let b2_ids = manifest
        .iter()
        .filter(|(id, _)| is_b2_id(id.as_str()))
        .count();
    match read_groups(out, &manifest) {
        Ok(Some(table)) => {
            r.groups = table.groups.len();
            let boot_ids: BTreeSet<&LogicalId> = manifest
                .iter()
                .filter(|(id, vs)| {
                    is_b2_id(id.as_str())
                        && id.as_str() != GROUP_TABLE_ID
                        && vs.iter().any(|v| v.prefetch == BOOT)
                })
                .map(|(id, _)| id)
                .collect();
            group_checks(&table, &manifest, &boot_ids, &mut r.problems);
            if let Some(s) = &settings {
                settings_complete(s, &table, &mut r.problems);
                style_checks(s, &table, &manifest, &mut r.problems);
            }
            // A model in a group brings its textures and clips with it (AB-D10).
            let read = |id: &LogicalId| -> Option<Vec<u8>> {
                let v = manifest.variants(id).first()?;
                std::fs::read(out.join(v.hash.to_string())).ok()
            };
            r.problems.extend(closure_gaps(&table, &read));
        }
        Ok(None) if b2_ids > 0 => r.problems.push(format!(
            "the manifest has {b2_ids} B2 id(s) but no {GROUP_TABLE_ID}"
        )),
        Ok(None) => {}
        Err(e) => r.problems.push(e.to_string()),
    }

    // Stand-ins.
    match load_log(out) {
        Ok(events) => r.problems.extend(allow.check(&events, looks_like_unit)),
        Err(e) => r.problems.push(e.to_string()),
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use asset_format::{PrefetchGroup, TextureFormat, Trigger, VariantFormat};

    use super::*;
    use crate::report::stubs::{StubEvent, record};
    use crate::store::{BlobStore, ManifestBuilder};
    use crate::world::groups::{GroupSpec, publish};

    struct Build {
        dir: std::path::PathBuf,
        store: BlobStore,
    }

    impl Drop for Build {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Settings with the sections a build with a boot group must have.
    fn settings() -> WorldSettings {
        WorldSettings {
            fog_colors: vec![[1, 2, 3]],
            highlights: vec![asset_format::HighlightStyle {
                name: "Fill".into(),
                kind: asset_format::HighlightKind::FilledHex,
                width: None,
                texture: Some(LogicalId::new("tx:cube.dds")),
                model: None,
                color: vec![0, 0, 0, 128],
                extra: Default::default(),
            }],
            color_keys: vec![asset_format::ColorKey {
                system: "terrain".into(),
                kind: "europe".into(),
                file: LogicalId::new("tx:cube.dds"),
            }],
            terrain_defs: vec![asset_format::TerrainDefRow {
                continent: 4,
                diffuse: Some(LogicalId::new("tx:grass.dds")),
                heightmap: None,
                specmap: None,
                base_dominance: 0,
                texture_scale: 0.1,
                projection: 0,
                id: 2,
            }],
            ..WorldSettings::default()
        }
    }

    fn tex(fill: u8) -> Vec<u8> {
        TextureBlob::new_2d(TextureFormat::Rgba8, false, 2, 2, vec![vec![fill; 16]]).encode()
    }

    /// A small normal build: two textures (one in boot), the settings, the group table.
    fn normal_build(tag: &str) -> Build {
        let dir = std::env::temp_dir().join(format!("civ5-verify-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = BlobStore::open(&dir).unwrap();
        let mut m = ManifestBuilder::new();
        let add = |id: &str, bytes: Vec<u8>, prefetch: PrefetchGroup, m: &mut ManifestBuilder| {
            let (hash, size) = store.put(&bytes).unwrap();
            m.add(
                LogicalId::new(id),
                Variant {
                    format: VariantFormat::Uncompressed,
                    hash,
                    size,
                    prefetch,
                },
            );
        };
        add("tx:cube.dds", tex(1), BOOT, &mut m);
        add("tx:grass.dds", tex(2), PrefetchGroup(0), &mut m);
        add(WORLD_SETTINGS_ID, settings().encode(), BOOT, &mut m);
        publish(
            &store,
            &mut m,
            vec![
                GroupSpec::boot(vec![
                    LogicalId::new("tx:cube.dds"),
                    LogicalId::new(WORLD_SETTINGS_ID),
                ]),
                GroupSpec::map(
                    "style:europe",
                    Trigger::Style("europe".into()),
                    vec![LogicalId::new("tx:grass.dds")],
                ),
            ],
        )
        .unwrap();
        m.finish(&store).unwrap();
        Build { dir, store }
    }

    fn problems(b: &Build) -> Vec<String> {
        verify(&b.dir, &StubAllow::default()).unwrap().problems
    }

    #[test]
    fn a_normal_build_passes() {
        let b = normal_build("ok");
        let r = verify(&b.dir, &StubAllow::default()).unwrap();
        assert!(r.is_ok(), "{:?}", r.problems);
        assert_eq!((r.ids, r.groups), (4, 2));
        assert_eq!(r.decoded, 4);
        let _ = &b.store;
    }

    #[test]
    fn a_texture_without_its_compressed_variants_fails_until_the_pass_adds_them() {
        let b = normal_build("variants");
        let colour: Vec<u8> = (0..64u32 * 64)
            .flat_map(|i| [(i % 64 * 4) as u8, (i / 64 * 4) as u8, 90, 255])
            .collect();
        let blob = TextureBlob::new_2d(TextureFormat::Rgba8, false, 64, 64, vec![colour]);
        let (hash, size) = b.store.put(&blob.encode()).unwrap();
        let mut m = ManifestBuilder::open(&b.store).unwrap();
        m.add(
            LogicalId::new("tx:wall_diff.dds"),
            Variant {
                format: VariantFormat::Uncompressed,
                hash,
                size,
                prefetch: PrefetchGroup(0),
            },
        );
        m.finish(&b.store).unwrap();
        let p = problems(&b);
        assert!(
            p.iter()
                .any(|p| p.contains("tx:wall_diff.dds: a BC device gets Rgba8")),
            "{p:?}"
        );
        assert!(
            p.iter()
                .any(|p| p.contains("tx:wall_diff.dds: a ASTC device")),
            "{p:?}"
        );
        crate::textures::variants::add_variants(&b.dir, &Default::default()).unwrap();
        assert!(problems(&b).is_empty(), "{:?}", problems(&b));
    }

    #[test]
    fn a_compressed_variant_that_is_not_whole_blocks_or_misfiled_is_a_problem() {
        let id = LogicalId::new("tx:odd.dds");
        let v = |format| Variant {
            format,
            hash: asset_format::ContentHash([1; 32]),
            size: 1,
            prefetch: PrefetchGroup(0),
        };
        let astc = TextureBlob::new_2d(
            TextureFormat::Astc4x4,
            false,
            6,
            8,
            vec![vec![0; TextureFormat::Astc4x4.mip_size(6, 8)]],
        );
        let p = texture_shape_problems(id.as_str(), &v(VariantFormat::Bc), &astc);
        assert_eq!(p.len(), 2, "{p:?}");
        // A DXT original of 6x8 stays; the client decodes it.
        let dxt = TextureBlob::new_2d(
            TextureFormat::Bc1,
            false,
            6,
            8,
            vec![vec![0; TextureFormat::Bc1.mip_size(6, 8)]],
        );
        assert!(texture_shape_problems(id.as_str(), &v(VariantFormat::Bc), &dxt).is_empty());
    }

    #[test]
    fn a_deleted_blob_fails() {
        let b = normal_build("deleted");
        let m = read_manifest(&b.dir).unwrap();
        let v = &m.variants(&LogicalId::new("tx:grass.dds"))[0];
        std::fs::remove_file(b.dir.join(v.hash.to_string())).unwrap();
        let p = problems(&b);
        assert!(
            p.iter()
                .any(|p| p.contains("tx:grass.dds") && p.contains("missing")),
            "{p:?}"
        );
    }

    #[test]
    fn a_forged_blob_fails_on_hash_and_on_type() {
        let b = normal_build("forged");
        let m = read_manifest(&b.dir).unwrap();
        let v = &m.variants(&LogicalId::new("tx:grass.dds"))[0];
        std::fs::write(b.dir.join(v.hash.to_string()), b"something else").unwrap();
        let p = problems(&b);
        assert!(
            p.iter()
                .any(|p| p.contains("hash") || p.contains("manifest says")),
            "{p:?}"
        );

        // A blob under the right hash that is not a texture.
        let b = normal_build("junk");
        let mut m = ManifestBuilder::open(&b.store).unwrap();
        let (hash, size) = b.store.put(b"not a texture at all").unwrap();
        m.add(
            LogicalId::new("tx:junk.dds"),
            Variant {
                format: VariantFormat::Uncompressed,
                hash,
                size,
                prefetch: PrefetchGroup(0),
            },
        );
        m.finish(&b.store).unwrap();
        let p = problems(&b);
        assert!(
            p.iter()
                .any(|p| p.contains("tx:junk.dds") && p.contains("C5TX")),
            "{p:?}"
        );
    }

    #[test]
    fn a_group_member_that_is_not_in_the_manifest_fails() {
        let b = normal_build("member");
        // Remove the id from the manifest but keep the table.
        let mut m = read_manifest(&b.dir).unwrap();
        assert!(m.remove(&LogicalId::new("tx:grass.dds")));
        let (hash, _) = b.store.put(&m.encode()).unwrap();
        std::fs::write(b.dir.join("manifest"), format!("{hash}\n")).unwrap();
        let p = problems(&b);
        assert!(
            p.iter().any(|p| p.contains("style:europe")
                && p.contains("tx:grass.dds")
                && p.contains("not in the manifest")),
            "{p:?}"
        );
    }

    #[test]
    fn the_boot_group_and_the_boot_flag_must_agree() {
        let b = normal_build("boot");
        // Another B2 id gets the boot flag without being in `boot`.
        let mut m = ManifestBuilder::open(&b.store).unwrap();
        let (hash, size) = b.store.put(&tex(9)).unwrap();
        m.add(
            LogicalId::new("tx:late.dds"),
            Variant {
                format: VariantFormat::Uncompressed,
                hash,
                size,
                prefetch: BOOT,
            },
        );
        m.finish(&b.store).unwrap();
        let p = problems(&b);
        assert!(
            p.iter()
                .any(|p| p.contains("tx:late.dds") && p.contains("not a member")),
            "{p:?}"
        );
    }

    /// Publishes `specs` into the build `b` again.
    fn republish(b: &Build, specs: Vec<GroupSpec>) {
        let mut m = ManifestBuilder::open(&b.store).unwrap();
        publish(&b.store, &mut m, specs).unwrap();
        m.finish(&b.store).unwrap();
    }

    #[test]
    fn a_texture_of_two_styles_belongs_in_the_shared_group() {
        let b = normal_build("two-styles");
        republish(
            &b,
            vec![GroupSpec::map(
                "style:africa",
                Trigger::Style("africa".into()),
                vec![LogicalId::new("tx:grass.dds")],
            )],
        );
        let p = problems(&b);
        assert!(
            p.iter().any(|p| p.contains("tx:grass.dds")
                && p.contains("style:africa, style:europe")
                && p.contains("style:shared")),
            "{p:?}"
        );

        // Moved to the shared group, with the requirements, the build is whole again.
        let mut europe = GroupSpec::map(
            "style:europe",
            Trigger::Style("europe".into()),
            vec![LogicalId::new("tx:cube.dds")],
        );
        europe.requires.push("style:shared".into());
        let mut m = ManifestBuilder::open(&b.store).unwrap();
        let (hash, size) = b.store.put(&tex(7)).unwrap();
        m.add(
            LogicalId::new("tx:rock.dds"),
            Variant {
                format: VariantFormat::Uncompressed,
                hash,
                size,
                prefetch: PrefetchGroup(0),
            },
        );
        m.finish(&b.store).unwrap();
        let mut africa = GroupSpec::map(
            "style:africa",
            Trigger::Style("africa".into()),
            vec![LogicalId::new("tx:rock.dds")],
        );
        africa.requires.push("style:shared".into());
        republish(
            &b,
            vec![
                europe,
                africa,
                GroupSpec::map(
                    "style:shared",
                    Trigger::Always,
                    vec![LogicalId::new("tx:grass.dds")],
                ),
            ],
        );
        let p = problems(&b);
        assert!(p.is_empty(), "{p:?}");
    }

    #[test]
    fn a_style_group_must_require_the_shared_group_and_hold_what_its_rows_name() {
        let b = normal_build("shared-req");
        // The shared group exists, `style:europe` does not require it.
        republish(
            &b,
            vec![GroupSpec::map(
                "style:shared",
                Trigger::Always,
                vec![LogicalId::new("tx:cube.dds")],
            )],
        );
        let p = problems(&b);
        assert!(
            p.iter()
                .any(|p| p.contains("style:europe") && p.contains("does not require")),
            "{p:?}"
        );

        // A row names `grass.dds`, which is in neither group of its continent.
        let b = normal_build("row-outside");
        republish(
            &b,
            vec![GroupSpec::map(
                "style:europe",
                Trigger::Style("europe".into()),
                vec![LogicalId::new("tx:cube.dds")],
            )],
        );
        let p = problems(&b);
        assert!(
            p.iter().any(|p| p.contains("tx:grass.dds")
                && p.contains("neither style:europe nor style:shared")),
            "{p:?}"
        );
    }

    #[test]
    fn a_stub_event_fails_unless_it_is_allowed_and_a_unit_entry_fails_always() {
        let b = normal_build("stub");
        record(
            &b.dir,
            StubEvent {
                id: "tx:grass.dds".into(),
                reason: "flat colour".into(),
            },
        )
        .unwrap();
        let p = problems(&b);
        assert!(p.iter().any(|p| p.contains("stub tx:grass.dds")), "{p:?}");

        let allow = StubAllow::parse(
            "tx:grass.dds | flat colour for the test | assets-b2 | 2026-10-21\n",
            Path::new("f"),
        )
        .unwrap();
        assert!(verify(&b.dir, &allow).unwrap().is_ok());

        let allow = StubAllow::parse(
            "tx:grass.dds | a | o | 2026-10-21\nmd:unit_archer | no mesh | o | 2026-10-21\n",
            Path::new("f"),
        )
        .unwrap();
        let p = verify(&b.dir, &allow).unwrap().problems;
        assert!(p.iter().any(|p| p.contains("no unit")), "{p:?}");
    }

    #[test]
    fn settings_that_name_a_missing_texture_fail() {
        let b = normal_build("refs");
        let mut m = ManifestBuilder::open(&b.store).unwrap();
        let mut w = settings();
        w.color_keys.push(asset_format::ColorKey {
            system: "terrain".into(),
            kind: "europe".into(),
            file: LogicalId::new("tx:gone.tga"),
        });
        let (hash, size) = b.store.put(&w.encode()).unwrap();
        m.add(
            LogicalId::new(WORLD_SETTINGS_ID),
            Variant {
                format: VariantFormat::Generic,
                hash,
                size,
                prefetch: BOOT,
            },
        );
        m.finish(&b.store).unwrap();
        let p = problems(&b);
        assert!(
            p.iter()
                .any(|p| p.contains("tx:gone.tga") && p.contains("color_keys")),
            "{p:?}"
        );
    }

    #[test]
    fn settings_that_lack_a_section_the_groups_imply_fail() {
        let b = normal_build("empty-settings");
        let mut m = ManifestBuilder::open(&b.store).unwrap();
        let (hash, size) = b.store.put(&WorldSettings::default().encode()).unwrap();
        m.add(
            LogicalId::new(WORLD_SETTINGS_ID),
            Variant {
                format: VariantFormat::Generic,
                hash,
                size,
                prefetch: BOOT,
            },
        );
        m.finish(&b.store).unwrap();
        let p = problems(&b);
        for section in ["fog_colors", "highlights", "color_keys"] {
            assert!(
                p.iter().any(|p| p.contains(section) && p.contains("empty")),
                "{section}: {p:?}"
            );
        }
        // The style group of the table implies terrain definitions.
        let b = normal_build("no-defs");
        let mut m = ManifestBuilder::open(&b.store).unwrap();
        let none = WorldSettings {
            terrain_defs: vec![],
            ..settings()
        };
        let (hash, size) = b.store.put(&none.encode()).unwrap();
        m.add(
            LogicalId::new(WORLD_SETTINGS_ID),
            Variant {
                format: VariantFormat::Generic,
                hash,
                size,
                prefetch: BOOT,
            },
        );
        m.finish(&b.store).unwrap();
        let p = problems(&b);
        assert!(p.iter().any(|p| p.contains("terrain_defs")), "{p:?}");
    }

    #[test]
    fn an_empty_directory_is_an_error_a_bad_model_blob_is_one_and_an_unknown_table_has_no_reader() {
        let dir = std::env::temp_dir().join(format!("civ5-verify-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(verify(&dir, &StubAllow::default()).is_err());
        let _ = std::fs::remove_dir_all(&dir);

        let b = normal_build("model");
        let mut m = ManifestBuilder::open(&b.store).unwrap();
        let (hash, size) = b.store.put(b"model bytes").unwrap();
        m.add(
            LogicalId::new("md:archer"),
            Variant {
                format: VariantFormat::Generic,
                hash,
                size,
                prefetch: PrefetchGroup(0),
            },
        );
        m.add(
            LogicalId::new("tb:other-b2"),
            Variant {
                format: VariantFormat::Generic,
                hash,
                size,
                prefetch: PrefetchGroup(0),
            },
        );
        m.finish(&b.store).unwrap();
        let p = problems(&b);
        assert!(
            p.iter()
                .any(|p| p.contains("md:archer") && p.contains("not a valid C5MD")),
            "{p:?}"
        );
        assert!(
            p.iter()
                .any(|p| p.contains("tb:other-b2") && p.contains("no reader")),
            "{p:?}"
        );
    }
}
