//! Set `b2-terrain`: the terrain pieces,
//! the terrain and decal textures, and the terrain section of the world settings.
//!
//! - **Pieces** (`pc:<style>`, `C5TP`): every piece CSV of the install with the four DDS files it
//!   names, one blob per continent style. The style is the folder of the CSV
//!   (`Assets/terrain/pieces/Grass/Africa/...`); `missing_hex` is Generic.
//! - **Terrain textures**: every file `terraindef.csv` names (diffuse, height, specular), the bytes
//!   of the original (BC blocks as they are, AB-D2); the reduced `DX9_Low` copies as
//!   `tx:<name>@low` (AB-D12, SIMP-032: terrain only).
//! - **Decals**: every texture of `DecalTextures.fpk`.
//! - **Settings**: `terrain_defs` and `decals` of `tb:world-b2`, merged into the blob there.
//! - **Groups** (4.13): `style:<style>` holds the pieces of the style and the textures only the
//!   terrain types of that continent use (with the `@low` ones); a texture several continents use
//!   is in `style:shared`, which each style group requires (section 10 change log). `decals`
//!   holds the decal textures.
//!
//! The terrain ids of the pieces are checked against the `ID` column of `terraindef.csv`
//! ([`check_ids`]); what the codes mean is RW-6, so a code the table lacks is a report line, not an
//! error.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use asset_format::{
    ContentHash, DecalDef, LogicalId, PieceStyle, PrefetchGroup, TerrainDefRow, TerrainPiece,
    TerrainPieceSet, Trigger, Variant, VariantFormat,
};
use rayon::prelude::*;

use super::tx_id;
use crate::game::GameSource;
use crate::models::ModelError;
use crate::store::{BlobStore, ManifestBuilder, StoreStats};
use crate::textures::{dds_passthrough, variant_format};
use crate::world::groups::{self, GroupSpec};
use crate::world::pieces::{PieceCsv, build_piece, parse_piece_csv, style_of_folder};
use crate::world::settings::{self, DECALS_FILE, TERRAIN_DEF_FILE};

pub const SET_NAME: &str = "b2-terrain";

/// The pack the decal textures come from.
const DECAL_PACK: &str = "DecalTextures.fpk";

/// `Continent ID` of `terraindef.csv` to the style of the textures and pieces.
// re: assets-b1 `terrain.rs` STYLES (`Q_PLOTS.continent_art`): 0 Generic, 1 America, 2 Asia,
// 3 Africa, 4 Europe; the same order as the colour keys of `ColorKeyData` (civ5-re RW-4).
pub fn style_of_continent(continent: u8) -> Option<PieceStyle> {
    Some(match continent {
        0 => PieceStyle::Generic,
        1 => PieceStyle::America,
        2 => PieceStyle::Asia,
        3 => PieceStyle::Africa,
        4 => PieceStyle::Europe,
        _ => return None,
    })
}

/// A piece found in the install: its parsed CSV and style.
#[derive(Debug, Clone)]
pub struct FoundPiece {
    pub csv: PieceCsv,
    pub style: PieceStyle,
}

/// Every piece CSV of the install, sorted by name. A CSV that does not have the shape of a piece
/// (`fogcolor.csv`, `terraindef.csv`) is skipped; one that has the shape but cannot be used (no
/// style folder, a file missing) is an error.
pub fn find_pieces(game: &GameSource) -> Result<Vec<FoundPiece>, ModelError> {
    let mut found = Vec::new();
    for e in game.iter_ext("csv") {
        let stem = e.name.strip_suffix(".csv").unwrap_or(&e.name);
        let text = String::from_utf8_lossy(&game.read(e)?).into_owned();
        // The first line of a piece is its version number alone; other CSVs begin with data.
        let first = text
            .trim_start_matches('\u{feff}')
            .lines()
            .next()
            .unwrap_or("");
        let is_piece = first
            .split(',')
            .next()
            .is_some_and(|c| matches!(c.trim(), "3" | "4"))
            && first.split(',').skip(1).all(|c| c.trim().is_empty());
        if !is_piece {
            continue;
        }
        let csv = parse_piece_csv(stem, &text).map_err(|e| ModelError::invalid(e.to_string()))?;
        let folder = game
            .origin_path(e)
            .and_then(|p| {
                p.parent()
                    .and_then(|d| d.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_default();
        let style = style_of_folder(&folder, &csv.name).ok_or_else(|| {
            ModelError::invalid(format!(
                "piece {}: the folder {folder:?} is not a continent style",
                csv.name
            ))
        })?;
        found.push(FoundPiece { csv, style });
    }
    Ok(found)
}

/// Terrain id codes of the pieces that `terraindef.csv` does not explain.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IdReport {
    /// Distinct codes over all pieces.
    pub codes: usize,
    /// Codes that no row matches, with the pieces that use them.
    pub unmatched: BTreeMap<u32, BTreeSet<String>>,
}

/// Whether a terrain id code names a row of `terraindef.csv`. // SIMPLIFIED-by-hypothesis, not a
/// decision: the codes are the file's bytes (B, G, R, A); the measured values of B (0x00, 0x02,
/// 0x05, 0x08, 0x10, 0x30, 0x40, ...) read as the digits of the decimal `ID` column (2, 5, 8, 10, 30,
/// 40, ...) written in hex, R is 0x10 plus the continent. RW-6 has the real rule; until it is
/// answered a mismatch is only reported.
// TODO(RE-6): replace by the rule of CellBlitJob when RW-6 answers what the codes mean.
fn code_matches(code: u32, defs: &[TerrainDefRow]) -> bool {
    let b = code & 0xff;
    let hex_digits_of = |id: u32| u32::from_str_radix(&id.to_string(), 16).ok();
    defs.iter().any(|r| hex_digits_of(r.id) == Some(b))
}

/// Codes that are not terrain: `ff000000` of `missing_hex`.
// measured: 2026-10-21.
const SPECIAL_CODES: [u32; 1] = [0xff00_0000];

/// Checks the palettes of `pieces` against the rows of `terraindef.csv`.
pub fn check_ids(pieces: &[TerrainPiece], defs: &[TerrainDefRow]) -> IdReport {
    let mut r = IdReport::default();
    let mut all = BTreeSet::new();
    for p in pieces {
        for &c in &p.id_palette {
            all.insert(c);
            if !SPECIAL_CODES.contains(&c) && !code_matches(c, defs) {
                r.unmatched.entry(c).or_default().insert(p.name.clone());
            }
        }
    }
    r.codes = all.len();
    r
}

/// What a texture of this set is: where it comes from and which id it gets.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TexJob {
    id: LogicalId,
    /// Lower-case file name.
    file: String,
    // SIMPLIFIED(SIMP-032): the `@low` variant is built for terrain textures only; the original has DX9_Low copies of more.
    low: bool,
}

#[derive(Debug)]
pub struct TerrainSummary {
    /// Pieces and stored bytes of the blob, per style.
    pub pieces: BTreeMap<PieceStyle, (usize, u64)>,
    pub textures: usize,
    pub low_textures: usize,
    pub decal_textures: usize,
    /// Bytes of the textures, by variant format.
    pub texture_bytes: BTreeMap<VariantFormat, u64>,
    pub settings_bytes: u64,
    pub group_table_bytes: u64,
    /// Bytes of each group the set writes.
    pub groups: BTreeMap<String, u64>,
    pub ids: IdReport,
    /// Lines for the report: the copy of `terraindef.csv`, files that differ between layers.
    pub notes: Vec<String>,
    pub stats: StoreStats,
}

type Built = Result<(VariantFormat, ContentHash, u64), ModelError>;

fn texture_job(store: &BlobStore, game: &GameSource, job: &TexJob) -> Built {
    let entry = if job.low {
        game.lookup_low(&job.file)
    } else {
        game.lookup(&job.file)
    }
    .ok_or_else(|| ModelError::invalid(format!("{} is missing from the install", job.file)))?;
    let bytes = game.read(entry)?;
    let blob = dds_passthrough(&bytes).map_err(|source| ModelError::Texture {
        name: job.file.clone(),
        source,
    })?;
    let (hash, size) = store.put(&blob.encode())?;
    Ok((variant_format(blob.format), hash, size))
}

/// The names of the files of the pack `DecalTextures.fpk`, visible or hidden by another layer.
fn decal_files(game: &GameSource) -> BTreeSet<String> {
    let in_pack = |e: &crate::game::source::Entry| {
        game.origin_path(e)
            .and_then(|p| p.file_name().map(|n| n.eq_ignore_ascii_case(DECAL_PACK)))
            .unwrap_or(false)
    };
    game.iter_all()
        .chain(game.shadowed())
        .filter(|e| e.extension() == "dds" && in_pack(e))
        .map(|e| e.name.clone())
        .collect()
}

/// Id of the group that holds the textures several continents use.
pub const SHARED_GROUP: &str = "style:shared";

/// The groups of the styles (4.13). A texture used by the terrain types of one continent is in
/// that continent's group with its pieces; a texture used by several is in [`SHARED_GROUP`], which
/// every style group requires (an owner decision: the
/// first build had such a texture in each of its continents' groups, so the groups were 46-50 MB
/// against the estimate of 33). Returns the shared group (`None` if no texture is shared) and the
/// style groups.
fn style_groups(
    piece_info: &BTreeMap<PieceStyle, (usize, u64)>,
    terrain_files: &BTreeMap<String, BTreeSet<PieceStyle>>,
    low_of: &BTreeMap<String, LogicalId>,
) -> (Option<GroupSpec>, Vec<GroupSpec>) {
    let tex_ids = |file: &String| -> Vec<LogicalId> {
        let mut v = vec![tx_id(file)];
        v.extend(low_of.get(file).cloned());
        v
    };
    let shared_members: Vec<LogicalId> = terrain_files
        .iter()
        .filter(|(_, styles)| styles.len() > 1)
        .flat_map(|(file, _)| tex_ids(file))
        .collect();
    let mut style_specs = Vec::new();
    for style in PieceStyle::ALL {
        let mut members: Vec<LogicalId> = Vec::new();
        if piece_info.contains_key(&style) {
            members.push(style.id());
        }
        for (file, styles) in terrain_files {
            if styles.len() == 1 && styles.contains(&style) {
                members.extend(tex_ids(file));
            }
        }
        if !members.is_empty() {
            let mut spec = GroupSpec::map(
                format!("style:{}", style.name()),
                Trigger::Style(style.name().to_owned()),
                members,
            );
            if !shared_members.is_empty() {
                spec.requires.push(SHARED_GROUP.to_owned());
            }
            style_specs.push(spec);
        }
    }
    let shared = (!shared_members.is_empty())
        .then(|| GroupSpec::map(SHARED_GROUP, Trigger::Always, shared_members));
    (shared, style_specs)
}

/// Builds the set into `out` and adds it to the manifest there. A file the install lacks, or a
/// piece that does not read, is an error (no stand-ins, AB-D14).
pub fn build_terrain(game: &GameSource, out: &Path) -> Result<TerrainSummary, ModelError> {
    let store = BlobStore::open(out)?;
    let mut manifest = ManifestBuilder::open(&store)?;
    let mut notes = Vec::new();

    // ---- settings sources ----
    let mut defs = settings::read_terrain_defs(game)?;
    notes.push(settings::terrain_def_note(game)?);
    // A file the table names that the install does not have is not made up: the reference is
    // dropped (the original has no such texture either) and the finding is reported.
    for r in &mut defs {
        for (what, slot) in [
            ("diffuse", &mut r.diffuse),
            ("heightmap", &mut r.heightmap),
            ("specmap", &mut r.specmap),
        ] {
            let Some(id) = slot.as_ref() else { continue };
            let file = id.as_str().trim_start_matches("tx:");
            if game.lookup(file).is_none() {
                notes.push(format!(
                    "finding: {TERRAIN_DEF_FILE} row (continent {}, id {}): {what} {file} is not in the install; the reference is dropped",
                    r.continent, r.id
                ));
                *slot = None;
            }
        }
    }
    let decal_bytes = game
        .read_by_name(DECALS_FILE)?
        .ok_or_else(|| ModelError::invalid(format!("{DECALS_FILE} is missing from the install")))?;
    let decals: Vec<DecalDef> =
        settings::parse_decals(&decal_bytes).map_err(|e| ModelError::invalid(e.to_string()))?;

    // ---- textures ----
    // Terrain textures by file, with the continents that use them.
    let mut terrain_files: BTreeMap<String, BTreeSet<PieceStyle>> = BTreeMap::new();
    for r in &defs {
        let style = style_of_continent(r.continent)
            .ok_or_else(|| ModelError::invalid(format!("terraindef: continent {}", r.continent)))?;
        for id in [&r.diffuse, &r.heightmap, &r.specmap].into_iter().flatten() {
            let file = id.as_str().trim_start_matches("tx:").to_owned();
            terrain_files.entry(file).or_default().insert(style);
        }
    }
    let missing: Vec<&str> = terrain_files
        .keys()
        .filter(|f| game.lookup(f).is_none())
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        return Err(ModelError::invalid(format!(
            "{TERRAIN_DEF_FILE} names files that are not in the install: {}",
            missing.join(", ")
        )));
    }
    let mut jobs: Vec<TexJob> = Vec::new();
    let mut low_of: BTreeMap<String, LogicalId> = BTreeMap::new();
    for file in terrain_files.keys() {
        jobs.push(TexJob {
            id: tx_id(file),
            file: file.clone(),
            low: false,
        });
        if game.lookup_low(file).is_some() {
            let id = LogicalId::new(format!("{}@low", tx_id(file).as_str()));
            low_of.insert(file.clone(), id.clone());
            jobs.push(TexJob {
                id,
                file: file.clone(),
                low: true,
            });
        }
    }
    let decal_set = decal_files(game);
    let terrain_job_count = jobs.len();
    for file in &decal_set {
        // A file that is also a terrain texture is built once, as a terrain texture.
        if !terrain_files.contains_key(file) {
            jobs.push(TexJob {
                id: tx_id(file),
                file: file.clone(),
                low: false,
            });
        }
    }
    let built: Vec<Built> = jobs
        .par_iter()
        .map(|j| texture_job(&store, game, j))
        .collect();
    let mut texture_bytes: BTreeMap<VariantFormat, u64> = BTreeMap::new();
    let mut known: BTreeSet<LogicalId> = BTreeSet::new();
    for (job, result) in jobs.iter().zip(built) {
        let (format, hash, size) = result?;
        *texture_bytes.entry(format).or_default() += size;
        known.insert(job.id.clone());
        manifest.add(
            job.id.clone(),
            Variant {
                format,
                hash,
                size,
                prefetch: PrefetchGroup(0),
            },
        );
    }

    // ---- pieces ----
    let found = find_pieces(game)?;
    let built_pieces: Vec<Result<TerrainPiece, ModelError>> = found
        .par_iter()
        .map(|f| {
            let read = |name: &str| -> Result<Vec<u8>, ModelError> {
                game.read_by_name(name)?.ok_or_else(|| {
                    ModelError::invalid(format!("piece {}: {name} is missing", f.csv.name))
                })
            };
            let files = [
                read(&f.csv.files[0])?,
                read(&f.csv.files[1])?,
                read(&f.csv.files[2])?,
                read(&f.csv.files[3])?,
            ];
            build_piece(&f.csv, [&files[0], &files[1], &files[2], &files[3]])
                .map_err(|e| ModelError::invalid(e.to_string()))
        })
        .collect();
    let mut by_style: BTreeMap<PieceStyle, Vec<TerrainPiece>> = BTreeMap::new();
    for (f, p) in found.iter().zip(built_pieces) {
        by_style.entry(f.style).or_default().push(p?);
    }
    let mut piece_info = BTreeMap::new();
    let mut all_pieces: Vec<TerrainPiece> = Vec::new();
    for (style, mut pieces) in by_style {
        pieces.sort_by(|a, b| a.name.cmp(&b.name));
        all_pieces.extend(pieces.iter().cloned());
        let count = pieces.len();
        let set = TerrainPieceSet { style, pieces };
        let (hash, size) = store.put(&set.encode())?;
        manifest.add(
            style.id(),
            Variant {
                format: VariantFormat::Generic,
                hash,
                size,
                prefetch: PrefetchGroup(0),
            },
        );
        piece_info.insert(style, (count, size));
    }
    let ids = check_ids(&all_pieces, &defs);

    // ---- settings ----
    let mut world = settings::load_existing(&store, &manifest)?;
    settings::apply_terrain(&mut world, defs.clone(), decals);
    let mut missing_refs: Vec<String> = world
        .texture_refs()
        .into_iter()
        .filter(|(what, id)| {
            (what.starts_with("terrain_defs") || what.starts_with("decals"))
                && !known.contains(id)
                && manifest.manifest().variants(id).is_empty()
        })
        .map(|(what, id)| format!("{} ({what})", id.as_str()))
        .collect();
    missing_refs.sort();
    missing_refs.dedup();
    if !missing_refs.is_empty() {
        return Err(ModelError::invalid(format!(
            "settings name textures that are not in the build: {}",
            missing_refs.join(", ")
        )));
    }
    let settings_bytes = settings::store_settings(&store, &mut manifest, &world)?;

    // ---- groups ----
    let (shared, style_specs) = style_groups(&piece_info, &terrain_files, &low_of);
    let mut specs = style_specs;
    specs.extend(shared);
    specs.push(GroupSpec::map(
        "decals",
        Trigger::Always,
        decal_set.iter().map(|f| tx_id(f)).collect(),
    ));
    let spec_ids: Vec<String> = specs.iter().map(|s| s.id.clone()).collect();
    let group_table_bytes = groups::publish(&store, &mut manifest, specs)?;
    let table = groups::load(&store, &manifest)?;
    let group_bytes = spec_ids
        .iter()
        .filter_map(|id| table.get(id).map(|g| (id.clone(), g.bytes)))
        .collect();

    manifest.finish(&store)?;
    Ok(TerrainSummary {
        pieces: piece_info,
        textures: terrain_files.len(),
        low_textures: low_of.len(),
        decal_textures: jobs.len() - terrain_job_count,
        texture_bytes,
        settings_bytes,
        group_table_bytes,
        groups: group_bytes,
        ids,
        notes,
        stats: store.stats(),
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use asset_format::{
        GROUP_TABLE_ID, GroupTable, Manifest, TextureBlob, WORLD_SETTINGS_ID, WorldSettings,
    };
    use ddsfile::{D3DFormat, Dds, NewD3dParams};

    use super::*;
    use crate::game::fpk::testutil::build_fpk;
    use crate::report::stubs::StubAllow;
    use crate::report::verify::verify;
    use crate::textures::read_dds;

    // ---- a small install -------------------------------------------------------------------

    struct Tree(PathBuf);

    impl Tree {
        fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "civ5-b2-terrain-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Tree(dir)
        }

        fn write(&self, rel: &str, bytes: &[u8]) {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn dds(format: D3DFormat, side: u32, fill: impl Fn(usize) -> u8) -> Vec<u8> {
        let mut d = Dds::new_d3d(NewD3dParams {
            height: side,
            width: side,
            depth: None,
            format,
            mipmap_levels: None,
            caps2: None,
        })
        .unwrap();
        for (i, b) in d.data.iter_mut().enumerate() {
            *b = fill(i);
        }
        let mut out = Vec::new();
        d.write(&mut out).unwrap();
        out
    }

    /// A height map (A16B16G16R16 is the fourcc 36 in the game's files) where R and B follow G
    /// except at texels 3 and 9, and A is flat.
    fn height_dds(side: u32) -> Vec<u8> {
        let n = (side * side) as usize;
        let mut out = vec![0u8; 128];
        out[..4].copy_from_slice(b"DDS ");
        let put = |out: &mut Vec<u8>, at: usize, v: u32| {
            out[at..at + 4].copy_from_slice(&v.to_le_bytes())
        };
        put(&mut out, 4, 124);
        put(&mut out, 8, 0x1007); // caps, height, width, pixel format
        put(&mut out, 12, side);
        put(&mut out, 16, side);
        put(&mut out, 76, 32); // pixel format size
        put(&mut out, 80, 4); // fourcc
        put(&mut out, 84, 36);
        put(&mut out, 108, 0x1000); // texture
        for i in 0..n {
            let g = (1000 + i * 37) as u16;
            let (r, b) = match i {
                3 => (g + 5, g - 2),
                9 => (g, g + 700),
                _ => (g, g),
            };
            for v in [r, g, b, 4242u16] {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out
    }

    fn id_dds(side: u32) -> Vec<u8> {
        // Two codes: B = 0x10, R = 0x14 and B = 0x26, R = 0x13; bytes B, G, R, A.
        dds(D3DFormat::A8R8G8B8, side, |i| {
            match (i / 4 % 3 == 0, i % 4) {
                (true, 0) => 0x26,
                (true, 2) => 0x13,
                (false, 0) => 0x10,
                (false, 2) => 0x14,
                (_, 3) => 0xff,
                _ => 0,
            }
        })
    }

    fn piece_csv(prefix: &str) -> String {
        format!(
            "3,,,\n{prefix}_H.dds,,,\n{prefix}_T1.dds,,,\n{prefix}_H_BLEND.dds,,,\n{prefix}_T_BLEND.dds,,,\n4,4,,\n0x800fffff,0x800fffff,0x800fffff,0x800fffff\n0x800fffff,0x1,0x800fffff,0x800fffff\n0x800fffff,0x800fffff,0x800fffff,0x800fffff\n0x800fffff,0x800fffff,0x800fffff,0x800fffff\n0,1,,\n"
        )
    }

    fn write_piece(t: &Tree, dir: &str, prefix: &str) {
        t.write(
            &format!("Assets/terrain/pieces/{dir}/{prefix}.csv"),
            piece_csv(prefix).as_bytes(),
        );
        t.write(
            &format!("Assets/terrain/pieces/{dir}/{prefix}_H.dds"),
            &height_dds(8),
        );
        t.write(
            &format!("Assets/terrain/pieces/{dir}/{prefix}_T1.dds"),
            &id_dds(8),
        );
        let blend = dds(D3DFormat::A8R8G8B8, 8, |i| (i * 5) as u8);
        t.write(
            &format!("Assets/terrain/pieces/{dir}/{prefix}_H_BLEND.dds"),
            &blend,
        );
        t.write(
            &format!("Assets/terrain/pieces/{dir}/{prefix}_T_BLEND.dds"),
            &blend,
        );
    }

    const DLC_DEF: &str = "Continent ID,Diffuse,Heightmap,Specmap,BaseDominance,Texture Scale,Projection,ID\n\
        4,Euro_A_D.dds,Euro_A_H.dds,NULL,0,0.1,0,10\n\
        3,Afri_A_D.dds,Euro_A_H.dds,Afri_A_S.dds,1,0.2,0,26\n\
        0,Gone_D.dds,Euro_A_H.dds,NULL,0,0.3,1,5\n";
    const PACKED_DEF: &str = "Continent ID,Diffuse,Heightmap,Specmap,BaseDominance,Texture Scale,Projection,ID\n\
        4,Euro_A_D.dds,Euro_A_H.dds,NULL,0,0.5,0,10\n\
        3,Afri_A_D.dds,Euro_A_H.dds,Afri_A_S.dds,1,0.2,0,26\n\
        0,NULL,NULL,NULL,0,0.3,1,5\n";

    fn fake_install() -> Tree {
        let t = Tree::new();
        let bc1 = |f: u8| dds(D3DFormat::DXT1, 8, move |i| f.wrapping_add(i as u8));
        t.write(
            "Resource/Common/TerrainModels.fpk",
            &build_fpk(&[("terraindef.csv", PACKED_DEF.as_bytes())]),
        );
        t.write(
            "Resource/DX9/TerrainTextures.fpk",
            &build_fpk(&[
                ("euro_a_d.dds", &bc1(1)),
                ("euro_a_h.dds", &dds(D3DFormat::DXT5, 8, |i| i as u8)),
                ("afri_a_d.dds", &bc1(2)),
                ("afri_a_s.dds", &bc1(3)),
                ("sunlight_europe_terrain.dds", &bc1(4)),
            ]),
        );
        // The fields of `complete_crops` (`Crops` names no texture in the XML) are files of the pack.
        let crops: Vec<(String, Vec<u8>)> = (1..=settings::CROPS_VARIANTS)
            .flat_map(|n| {
                [
                    (format!("crops_europe_{n:02}_d.dds"), bc1(9)),
                    (format!("crops_generic_{n:02}_h.dds"), bc1(10)),
                ]
            })
            .collect();
        let (grey, flood) = (bc1(5), bc1(6));
        let mut decal_pack: Vec<(&str, &[u8])> =
            vec![("grey.dds", &grey), ("floodplains_d.dds", &flood)];
        decal_pack.extend(crops.iter().map(|(n, d)| (n.as_str(), d.as_slice())));
        t.write("Resource/DX9/DecalTextures.fpk", &build_fpk(&decal_pack));
        t.write(
            "Resource/DX9_Low/TerrainTexturesDX9Low.fpk",
            &build_fpk(&[
                ("euro_a_d.dds", &bc1(7)),
                ("sunlight_europe_terrain.dds", &bc1(8)),
            ]),
        );
        t.write(
            "Assets/DLC/Shared/Terrain/TerrainDef.csv",
            DLC_DEF.as_bytes(),
        );
        t.write(
            "Assets/Decals/decal_filenames.xml",
            br#"<DecalTypes><Riverbank><Riverbank_01 albedo="assets/Decals/floodplains_D.dds"
               heightmap="assets/Decals/grey.dds" priority="0.4"/></Riverbank></DecalTypes>"#,
        );
        write_piece(&t, "Grass/Europe", "Euro_Grass_1_1");
        write_piece(&t, "Wonder_X/Africa", "Afri_X");
        t.write(
            "Assets/terrain/pieces/missing_hex.csv",
            piece_csv("missing_hex").as_bytes(),
        );
        for suffix in ["H", "T1", "H_BLEND", "T_BLEND"] {
            let bytes = match suffix {
                "H" => height_dds(8),
                "T1" => id_dds(8),
                _ => dds(D3DFormat::A8R8G8B8, 8, |i| i as u8),
            };
            t.write(
                &format!("Assets/terrain/pieces/missing_hex_{suffix}.dds"),
                &bytes,
            );
        }
        // Not pieces: a fog colour table and a CSV of numbers.
        t.write("Assets/Fog/fogcolor.csv", b"146,182,243\r\n");
        t
    }

    fn out_dir(tag: &str) -> Tree {
        let t = Tree::new();
        let _ = tag;
        t
    }

    fn manifest_hash(out: &std::path::Path) -> String {
        std::fs::read_to_string(out.join("manifest")).unwrap()
    }

    #[test]
    fn a_small_install_builds_closes_and_round_trips() {
        let install = fake_install();
        let game = GameSource::open(&install.0).unwrap();
        let out = out_dir("a");
        let s = build_terrain(&game, &out.0).unwrap();

        assert_eq!((s.textures, s.low_textures, s.decal_textures), (4, 1, 18));
        assert_eq!(s.pieces.len(), 3);
        assert_eq!(s.pieces[&PieceStyle::Europe].0, 1);
        assert_eq!(s.pieces[&PieceStyle::Generic].0, 1, "missing_hex");
        assert!(
            s.notes
                .iter()
                .any(|n| n.contains("finding") && n.contains("gone_d.dds")),
            "{:?}",
            s.notes
        );
        assert!(
            s.notes
                .iter()
                .any(|n| n.contains("taken Dlc") && n.contains("other copy")),
            "{:?}",
            s.notes
        );

        // The output closes: every blob reads, groups and references resolve.
        // (The settings blob is in the boot prefetch group; `b2-light`, which makes the group, was
        // not built into this directory, and that is the only thing `verify` has to say once
        // `build` has added the texture variants.)
        crate::textures::variants::add_variants(&out.0, &Default::default()).unwrap();
        let r = verify(&out.0, &StubAllow::default()).unwrap();
        assert!(
            r.problems.iter().all(|p| p.contains("no `boot` group")),
            "{:?}",
            r.problems
        );
        assert_eq!(r.problems.len(), 1);

        let store = BlobStore::open(&out.0).unwrap();
        let manifest = ManifestBuilder::open(&store).unwrap();
        let m: &Manifest = manifest.manifest();
        let blob = |id: &str| {
            std::fs::read(store.blob_path(&m.variants(&LogicalId::new(id))[0].hash)).unwrap()
        };

        // Textures are the bytes of the files: DXT1 as BC1, the @low one is the low pack's.
        let low = TextureBlob::decode(&blob("tx:euro_a_d.dds@low")).unwrap();
        let full = TextureBlob::decode(&blob("tx:euro_a_d.dds")).unwrap();
        assert_ne!(low.mips, full.mips);
        assert_eq!(full.format, asset_format::TextureFormat::Bc1);
        assert!(
            m.variants(&LogicalId::new("tx:sunlight_europe_terrain.dds@low"))
                .is_empty(),
            "@low is for terrain only"
        );
        assert!(
            m.variants(&LogicalId::new("tx:sunlight_europe_terrain.dds"))
                .is_empty(),
            "light cubes are b2-light's"
        );

        // Settings: the rows of the DLC copy, with the missing file dropped.
        let w = WorldSettings::decode(&blob(WORLD_SETTINGS_ID)).unwrap();
        assert_eq!(w.terrain_defs.len(), 3);
        assert_eq!(w.terrain_defs[0].texture_scale, 0.1, "the DLC/Shared copy");
        assert_eq!(w.terrain_defs[2].diffuse, None);
        assert_eq!(w.terrain_defs[2].heightmap, Some(tx_id("euro_a_h.dds")));
        // The river bank, then the fields `complete_crops` adds.
        assert_eq!(w.decals.len(), 1 + settings::CROPS_VARIANTS as usize);
        assert_eq!(w.decals[0].albedo, Some(tx_id("floodplains_d.dds")));
        assert_eq!(w.decals[1].albedo, Some(tx_id("crops_europe_01_d.dds")));

        // Groups.
        let t = GroupTable::decode(&blob(GROUP_TABLE_ID)).unwrap();
        let ids = |g: &str| -> Vec<String> {
            t.get(g)
                .unwrap()
                .members
                .iter()
                .map(|m| m.as_str().to_owned())
                .collect()
        };
        assert_eq!(
            ids("style:europe"),
            ["pc:europe", "tx:euro_a_d.dds", "tx:euro_a_d.dds@low"]
        );
        assert_eq!(
            ids("style:africa"),
            ["pc:africa", "tx:afri_a_d.dds", "tx:afri_a_s.dds"]
        );
        assert_eq!(ids("style:generic"), ["pc:generic"]);
        // `euro_a_h.dds` is used by three continents: once, in the shared group.
        assert_eq!(ids(SHARED_GROUP), ["tx:euro_a_h.dds"]);
        assert_eq!(t.get(SHARED_GROUP).unwrap().trigger, Trigger::Always);
        let decals = ids("decals");
        assert_eq!(decals.len(), 2 + 2 * settings::CROPS_VARIANTS as usize);
        assert!(decals.contains(&"tx:floodplains_d.dds".to_owned()));
        assert!(decals.contains(&"tx:grey.dds".to_owned()));
        // Each style group requires the shared one; there is no `boot` group in this directory yet.
        for g in ["style:europe", "style:africa", "style:generic"] {
            assert_eq!(t.get(g).unwrap().requires, [SHARED_GROUP], "{g}");
        }
        assert!(t.get(SHARED_GROUP).unwrap().requires.is_empty());

        // Pieces: lossless against the files.
        let set = TerrainPieceSet::decode(&blob("pc:europe")).unwrap();
        assert_eq!(set.pieces.len(), 1);
        let p = set.piece(0).unwrap();
        let src = read_dds(&height_dds(8)).unwrap();
        let want: Vec<[u16; 4]> = src.mips[0]
            .chunks_exact(8)
            .map(|c| [0, 2, 4, 6].map(|o| u16::from_le_bytes([c[o], c[o + 1]])))
            .collect();
        assert_eq!(p.height_rgba(), want);
        assert_eq!(p.height_diff.len(), 2);
        assert_eq!(p.id_palette.len(), 2);
        assert_eq!(set.pieces[0].flags, 0);
        assert_eq!(set.pieces[0].csv_extra, [1]);
        // id check: B = 0x10 and 0x26 are rows 10 and 26.
        assert!(s.ids.unmatched.is_empty(), "{:?}", s.ids.unmatched);
    }

    #[test]
    fn two_builds_give_the_same_manifest() {
        let install = fake_install();
        let game = GameSource::open(&install.0).unwrap();
        let (a, b) = (out_dir("a"), out_dir("b"));
        build_terrain(&game, &a.0).unwrap();
        build_terrain(&game, &b.0).unwrap();
        assert_eq!(manifest_hash(&a.0), manifest_hash(&b.0));
        // And again over the first: nothing changes.
        build_terrain(&game, &a.0).unwrap();
        assert_eq!(manifest_hash(&a.0), manifest_hash(&b.0));
    }

    #[test]
    fn the_terraindef_pick_is_one_switch_and_the_other_copy_stays_visible() {
        let install = fake_install();
        let game = GameSource::open(&install.0).unwrap();
        let dlc = settings::terrain_def_entry(&game, settings::TerrainDefCopy::DlcShared).unwrap();
        let packed =
            settings::terrain_def_entry(&game, settings::TerrainDefCopy::TerrainModelsPack)
                .unwrap();
        assert_ne!(dlc, packed);
        assert_eq!(dlc.layer, crate::game::source::Layer::Dlc);
        assert_eq!(
            settings::TERRAIN_DEF_COPY,
            settings::TerrainDefCopy::DlcShared
        );
        // The copy that is not taken is among the hidden ones.
        assert!(game.shadowed().iter().any(|e| e == packed));
        let rows = |e: &crate::game::source::Entry| {
            settings::parse_terrain_defs(&game.read(e).unwrap()).unwrap()
        };
        assert_eq!(rows(dlc)[0].texture_scale, 0.1);
        assert_eq!(rows(packed)[0].texture_scale, 0.5);
        let note = settings::terrain_def_note(&game).unwrap();
        assert!(note.contains("changed in the other: 4/10"), "{note}");
    }

    #[test]
    fn a_piece_without_its_file_or_style_is_an_error() {
        let install = fake_install();
        std::fs::remove_file(
            install
                .0
                .join("Assets/terrain/pieces/Grass/Europe/Euro_Grass_1_1_T1.dds"),
        )
        .unwrap();
        let game = GameSource::open(&install.0).unwrap();
        let out = out_dir("x");
        let e = build_terrain(&game, &out.0).unwrap_err().to_string();
        assert!(e.contains("euro_grass_1_1") && e.contains("missing"), "{e}");

        let install = fake_install();
        write_piece(&install, "Grass/Mars", "Mars_X");
        let game = GameSource::open(&install.0).unwrap();
        let e = build_terrain(&game, &out.0).unwrap_err().to_string();
        assert!(e.contains("mars_x") && e.contains("Mars"), "{e}");
    }

    #[test]
    fn a_missing_decal_file_named_by_the_xml_is_an_error() {
        let install = fake_install();
        install.write(
            "Assets/Decals/decal_filenames.xml",
            br#"<DecalTypes><Crops><Crops_01 albedo="assets/Decals/nowhere.dds"/></Crops></DecalTypes>"#,
        );
        let game = GameSource::open(&install.0).unwrap();
        let out = out_dir("d");
        let e = build_terrain(&game, &out.0).unwrap_err().to_string();
        assert!(e.contains("tx:nowhere.dds"), "{e}");
    }

    fn row(continent: u8, id: u32) -> TerrainDefRow {
        TerrainDefRow {
            continent,
            diffuse: Some(tx_id("a_d.dds")),
            heightmap: Some(tx_id("a_h.dds")),
            specmap: None,
            base_dominance: 0,
            texture_scale: 0.1,
            projection: 0,
            id,
        }
    }

    #[test]
    fn continents_map_to_styles() {
        assert_eq!(style_of_continent(0), Some(PieceStyle::Generic));
        assert_eq!(style_of_continent(3), Some(PieceStyle::Africa));
        assert_eq!(style_of_continent(4), Some(PieceStyle::Europe));
        assert_eq!(style_of_continent(5), None);
    }

    #[test]
    fn codes_are_matched_by_the_blue_byte_read_as_decimal_digits() {
        let defs = [row(4, 10), row(4, 26), row(0, 5)];
        assert!(code_matches(0x0010_0010, &defs), "ID 10 is 0x10");
        assert!(code_matches(0x0010_0026, &defs));
        assert!(code_matches(0x0011_0005, &defs));
        assert!(!code_matches(0x0010_0024, &defs));
    }
}
