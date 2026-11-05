//! `golden terrain` (world-terrain 4.18): G-T1, the height tiles of a capture
//! against the field of our terrain build for the same map.
//!
//! The capture has the `DisplacementMap` tiles (A8, 1024 x 1024, 64 texels of border each side, so
//! 896 apart) of the cells it drew. We build the whole field of the scene's map with the pieces of
//! the install (the stages of the water included), put each tile where the layout says, look a few
//! texels around for the offset that fits best (and say what it is), and count the texels that agree.
//!
//! What the check cannot do (SIMP-035): the original draws the variant of every piece from the
//! stream of the MSVC `rand`, which D-3 could not repeat; our variant comes from our own stream, so
//! the heights agree where the pieces of one kind agree (flat land, water) and differ in the shape
//! of hills and mountains. The report therefore counts the agreement per kind of ground, and writes
//! pairs of crops (the tile, our field, the difference) for the registry (AC-0.4).
//!
//! G-T2 (pages of the cache against the pages of the capture) needs the bake of our cache on a GPU
//! and the page-to-cell table of the draws; it is not here yet. `S2-bake` is not a frame of the
//! bake (the bake ran before it), so only final pages can ever be compared.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use asset_format::{
    ContentHash, LogicalId, Manifest, PieceStyle, PlatformProfile, TerrainPieceSet,
    WORLD_SETTINGS_ID, WorldSettings,
};
use glam::Vec2;
use protocol::{MapBasis, PlotFlags, PlotType};
use render::terrain::{Relief, TerrainKind};
use render::world::terrain::library::TerrainLibrary;
use render::world::terrain::{BasisFlags, PlotBasis, Terrain, TerrainBasis};
use serde::Serialize;

use crate::image::{Rgba8, write_png};
use crate::wv3;

/// Texels between two tiles of the capture, and the border of a tile.
const TILE_STRIDE: i32 = 896;
const TILE_BORDER: i32 = 64;
const TILE_SIDE: usize = 1024;
/// How far around the nominal place the registration looks, in texels.
const SEARCH: i32 = 16;

pub struct Options {
    pub scene: String,
    pub cache: PathBuf,
    pub assets: PathBuf,
    pub out: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct TileReport {
    pub resource: String,
    pub tile: (i32, i32),
    pub flipped: bool,
    /// Offset from the nominal place, in texels (ours).
    pub offset: (i32, i32),
    /// Mean |difference| at the nominal place and at the best place (units of the A8 height).
    pub mean_error_nominal: f64,
    pub mean_error_best: f64,
    pub texels: u64,
    pub within_1: f64,
    pub within_4: f64,
    /// Texels the tile and our field both call land (>= 56) or both call water.
    pub land_water_agreement: f64,
    /// By the relief of our hex under the texel: texels, share within 1, share within 4.
    pub by_relief: BTreeMap<String, (u64, f64, f64)>,
    pub crop: String,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub scene: String,
    pub map: (u16, u16, bool),
    pub tiles: Vec<TileReport>,
    pub within_1: f64,
    pub within_4: f64,
    pub note: Vec<String>,
}

/// Reads the blob of a logical id from a directory of content-addressed blobs.
fn blob(dir: &Path, manifest: &Manifest, id: &LogicalId) -> Result<Vec<u8>> {
    let v = manifest
        .resolve(id, &PlatformProfile::uncompressed_only())
        .or_else(|| manifest.variants(id).first())
        .with_context(|| format!("{} is not in the manifest", id.as_str()))?;
    let path = dir.join(v.hash.to_string());
    std::fs::read(&path).with_context(|| format!("cannot read {}", path.display()))
}

fn read_manifest(dir: &Path) -> Result<Manifest> {
    let text = std::fs::read_to_string(dir.join("manifest"))
        .with_context(|| format!("no manifest in {}", dir.display()))?;
    let hash: ContentHash = text
        .trim()
        .parse()
        .map_err(|e| anyhow::anyhow!("manifest hash: {e:?}"))?;
    let bytes = std::fs::read(dir.join(hash.to_string()))?;
    Manifest::decode(&bytes).map_err(|e| anyhow::anyhow!("manifest: {e}"))
}

/// The terrain names of the catalog by id (the base game's order).
fn terrain_of(id: i16) -> TerrainKind {
    match id {
        0 => TerrainKind::Grass,
        1 => TerrainKind::Plains,
        2 => TerrainKind::Desert,
        3 => TerrainKind::Tundra,
        4 => TerrainKind::Snow,
        5 => TerrainKind::Coast,
        _ => TerrainKind::Ocean,
    }
}

/// `MapBasis` to the render's basis, as `client-app/bridge/terrain.rs::plot_basis` does, without
/// the catalog: the terrain ids of the base game, and a natural wonder is only a flag (the bridge of
/// the matcher gives it the class of Fuji then).
pub fn terrain_basis(basis: &MapBasis) -> Result<TerrainBasis> {
    let plots = basis
        .plots
        .iter()
        .map(|p| {
            let relief = match p.plot_type {
                PlotType::Land => Relief::Flat,
                PlotType::Hill => Relief::Hill,
                PlotType::Mountain => Relief::Mountain,
                PlotType::Water => Relief::Water,
            };
            let mut kind = terrain_of(p.terrain);
            if relief == Relief::Water {
                kind = if p.flags.contains(PlotFlags::IS_LAKE) || kind == TerrainKind::Coast {
                    TerrainKind::Coast
                } else {
                    TerrainKind::Ocean
                };
            }
            let mut flags = 0;
            for (view, ours) in [
                (PlotFlags::IS_LAKE, BasisFlags::LAKE),
                (PlotFlags::IS_COASTAL, BasisFlags::COASTAL),
                (PlotFlags::NATURAL_WONDER, BasisFlags::NATURAL_WONDER),
            ] {
                if p.flags.contains(view) {
                    flags |= ours;
                }
            }
            PlotBasis {
                relief,
                terrain: kind,
                continent: p.continent_art,
                flags: BasisFlags(flags),
                rivers: p.rivers & 0x3F,
                river_flow: p.river_flow,
                ..PlotBasis::default()
            }
        })
        .collect();
    TerrainBasis::new(
        basis.width,
        basis.height,
        basis.wrap_x,
        basis.terrain_seed,
        plots,
    )
    .context("the plot count does not match the size")
}

/// Builds the whole field of a basis with the install's pieces.
pub fn build_terrain(assets: &Path, basis: TerrainBasis) -> Result<Terrain> {
    let manifest = read_manifest(assets)?;
    let settings = WorldSettings::decode(&blob(
        assets,
        &manifest,
        &LogicalId::new(WORLD_SETTINGS_ID),
    )?)
    .map_err(|e| anyhow::anyhow!("world settings: {e}"))?;
    let mut lib = TerrainLibrary::new();
    for style in PieceStyle::ALL {
        let bytes = blob(assets, &manifest, &style.id())?;
        lib.add_piece_set(
            TerrainPieceSet::decode(&bytes).map_err(|e| anyhow::anyhow!("{style}: {e}"))?,
        );
    }
    lib.set_defs(settings.terrain_defs.clone());
    let mut terrain = Terrain::default();
    terrain.set_library(Arc::new(lib));
    render::frame::wire_build_stages(&mut terrain, &settings);
    terrain.set_basis(basis);
    terrain.build_all();
    Ok(terrain)
}

/// One tile of the capture: its index in the grid of tiles and its A8 bytes (mip 0).
struct Tile {
    resource: String,
    k: i32,
    a8: Vec<u8>,
}

fn read_tiles(cache: &Path, scene: &str, capture: &str) -> Result<Vec<Tile>> {
    let dir = cache.join("golden").join(scene).join(capture);
    let list: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(dir.join("textures.json"))?)?;
    let is_tile = |t: &serde_json::Value| {
        t["format"] == "A8_UNORM" && t["width"] == 1024 && t["height"] == 1024 && t["mips"] == 11
    };
    let mut ids: Vec<String> = list
        .iter()
        .filter(|t| is_tile(t))
        .filter_map(|t| t["resource"].as_str().map(str::to_string))
        .collect();
    ids.sort_by_key(|s| s.parse::<u64>().unwrap_or(0));
    let mut out = Vec::new();
    for (k, id) in ids.iter().enumerate() {
        let path = dir.join("textures").join(format!("{id}.dds"));
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let dds = ddsfile::Dds::read(&bytes[..]).context("a tile is not a DDS")?;
        let data = dds.get_data(0).context("a tile has no data")?;
        if data.len() < TILE_SIDE * TILE_SIDE {
            continue;
        }
        out.push(Tile {
            resource: id.clone(),
            k: k as i32,
            a8: data[..TILE_SIDE * TILE_SIDE].to_vec(),
        });
    }
    Ok(out)
}

/// Our height in the units of the A8 tile: the high byte of the raw u16 (it fits the tiles better
/// than `raw / 255`, which the height formula `a8 * scale.z` suggests).
fn ours_a8(raw: u16) -> f32 {
    f32::from(raw >> 8)
}

/// The A8 of the tile at (column, row) with the rows flipped or not.
fn tile_at(t: &Tile, flipped: bool, c: i32, r: i32) -> u8 {
    let row = if flipped { TILE_SIDE as i32 - 1 - r } else { r };
    t.a8[row as usize * TILE_SIDE + c as usize]
}

/// The byte of the original's height tiles (the `DisplacementMap` A8 of the captures of `scene`) at
/// the texel `(u, v)` of our field, at the nominal place of the tiles (rows flipped, as G-T1 finds
/// them); `None` where no tile of the captures covers it.
pub fn capture_a8_at(
    cache: &Path,
    scene: &str,
    wrap_x: bool,
    u: i32,
    v: i32,
) -> Result<Option<u8>> {
    let shift = if wrap_x { (0, 96) } else { (96, 96) };
    for c in capture_dirs(cache, scene)? {
        for t in read_tiles(cache, scene, &c)? {
            let (tx, ty) = (t.k / 3, t.k % 3);
            let (c0, r0) = (
                u - (TILE_STRIDE * tx - TILE_BORDER - shift.0),
                v - (TILE_STRIDE * ty - TILE_BORDER - shift.1),
            );
            if (0..TILE_SIDE as i32).contains(&c0) && (0..TILE_SIDE as i32).contains(&r0) {
                let a = tile_at(&t, true, c0, r0);
                if a != 0 {
                    return Ok(Some(a));
                }
            }
        }
    }
    Ok(None)
}

/// Mean |difference| over a sparse set of texels and how many were compared.
fn score(
    t: &Tile,
    field: &render::world::terrain::field::TerrainField,
    origin: (i32, i32),
    flipped: bool,
    step: usize,
) -> (f64, u64) {
    let (fw, fh) = (field.width() as i32, field.height() as i32);
    let (mut sum, mut n) = (0.0f64, 0u64);
    for r in (0..TILE_SIDE).step_by(step) {
        for c in (0..TILE_SIDE).step_by(step) {
            let (u, v) = (origin.0 + c as i32, origin.1 + r as i32);
            let a = tile_at(t, flipped, c as i32, r as i32);
            if a == 0 || v < 0 || v >= fh || u < 0 || u >= fw {
                continue;
            }
            sum += f64::from((f32::from(a) - ours_a8(field.get_height(u, v))).abs());
            n += 1;
        }
    }
    if n == 0 {
        (f64::MAX, 0)
    } else {
        (sum / n as f64, n)
    }
}

pub fn run(o: &Options) -> Result<Report> {
    let scene_dir = o.cache.join("golden").join(&o.scene);
    let basis_bytes = std::fs::read(scene_dir.join("mapbasis.wv3")).with_context(|| {
        format!(
            "no mapbasis.wv3 for {} (run `golden world {}` first)",
            o.scene, o.scene
        )
    })?;
    let mb = wv3::decode_basis(&basis_bytes)?;
    let basis = terrain_basis(&mb)?;
    let terrain = build_terrain(&o.assets, basis)?;
    let sampler = terrain.sampler().context("no field")?;
    let frame = *sampler.frame();
    let (fw, fh) = frame.texel_size();
    std::fs::create_dir_all(&o.out)?;

    let mut report = Report {
        scene: o.scene.clone(),
        map: (mb.width, mb.height, mb.wrap_x),
        tiles: Vec::new(),
        within_1: 0.0,
        within_4: 0.0,
        note: Vec::new(),
    };

    // The shift from the padded field of the original to ours.
    let shift = if mb.wrap_x { (0, 96) } else { (96, 96) };
    let captures = capture_dirs(&o.cache, &o.scene)?;
    let mut tiles = Vec::new();
    for c in &captures {
        tiles.extend(read_tiles(&o.cache, &o.scene, c)?);
    }
    if tiles.is_empty() {
        report.note.push(format!(
            "{}: no DisplacementMap tile is saved in the capture(s) {:?}; G-T1 has nothing to compare",
            o.scene, captures
        ));
        report.note.push(
            "G-T2 (cache pages) is not run: it needs our bake on a GPU and the page-to-cell table; \
             S2-bake is not a bake frame (the bake ran before it), final pages only"
                .into(),
        );
        write_report(o, &report)?;
        return Ok(report);
    }

    let (mut t1_all, mut t4_all, mut n_all) = (0u64, 0u64, 0u64);
    for t in &tiles {
        let (tx, ty) = (t.k / 3, t.k % 3);
        let nominal = (
            TILE_STRIDE * tx - TILE_BORDER - shift.0,
            TILE_STRIDE * ty - TILE_BORDER - shift.1,
        );
        let mut best = (f64::MAX, nominal, true);
        let mut nominal_err = (f64::MAX, true);
        for flipped in [true, false] {
            let (e, _) = score(t, sampler.field(), nominal, flipped, 8);
            if flipped || e < nominal_err.0 {
                nominal_err = (e, flipped);
            }
            for dv in (-SEARCH..=SEARCH).step_by(2) {
                for du in (-SEARCH..=SEARCH).step_by(2) {
                    let org = (nominal.0 + du, nominal.1 + dv);
                    let (e, n) = score(t, sampler.field(), org, flipped, 8);
                    if n > 1000 && e < best.0 {
                        best = (e, org, flipped);
                    }
                }
            }
        }
        // Refine to the texel around the best coarse place.
        let (mut refined, centre) = (best, best.1);
        for dv in -2..=2 {
            for du in -2..=2 {
                let org = (centre.0 + du, centre.1 + dv);
                let (e, n) = score(t, sampler.field(), org, best.2, 4);
                if n > 1000 && e < refined.0 {
                    refined = (e, org, best.2);
                }
            }
        }
        let (_, origin, flipped) = refined;

        // The statistics at the registered place.
        let (mut n, mut w1, mut w4, mut lw) = (0u64, 0u64, 0u64, 0u64);
        let mut by_relief: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
        let mut diff = Rgba8 {
            width: TILE_SIDE,
            height: TILE_SIDE,
            rgba: vec![0; TILE_SIDE * TILE_SIDE * 4],
        };
        let mut pair = Rgba8 {
            width: TILE_SIDE * 2,
            height: TILE_SIDE,
            rgba: vec![0; TILE_SIDE * TILE_SIDE * 8],
        };
        for r in 0..TILE_SIDE as i32 {
            for c in 0..TILE_SIDE as i32 {
                let a = tile_at(t, flipped, c, r);
                let (u, v) = (origin.0 + c, origin.1 + r);
                let inside = a != 0 && v >= 0 && v < fh as i32 && u >= 0 && u < fw as i32;
                let ours = if inside {
                    ours_a8(sampler.field().get_height(u, v))
                } else {
                    0.0
                };
                let px = |img: &mut Rgba8, x: usize, y: usize, rgb: [u8; 3]| {
                    let i = (y * img.width + x) * 4;
                    img.rgba[i..i + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
                };
                px(&mut pair, c as usize, r as usize, [a, a, a]);
                let o8 = ours.round().clamp(0.0, 255.0) as u8;
                px(&mut pair, TILE_SIDE + c as usize, r as usize, [o8, o8, o8]);
                if !inside {
                    continue;
                }
                let d = (f32::from(a) - ours).abs();
                n += 1;
                w1 += u64::from(d <= 1.0);
                w4 += u64::from(d <= 4.0);
                lw += u64::from((f32::from(a) >= 56.0) == (ours >= 56.0));
                let world = frame.tex_to_world(Vec2::new(u as f32 + 0.5, v as f32 + 0.5));
                let name = sampler
                    .hex_at(world)
                    .and_then(|(x, y)| terrain.basis().and_then(|b| b.plot(x, y)))
                    .map_or("off".to_string(), |p| format!("{:?}", p.relief));
                let e = by_relief.entry(name).or_default();
                e.0 += 1;
                e.1 += u64::from(d <= 1.0);
                e.2 += u64::from(d <= 4.0);
                let g = (d * 16.0).min(255.0) as u8;
                px(&mut diff, c as usize, r as usize, [g, 255 - g, 0]);
            }
        }
        let crop = o.out.join(format!("{}-tile-{}.png", o.scene, t.resource));
        write_png(&crop, &pair)?;
        write_png(
            &o.out
                .join(format!("{}-tile-{}-diff.png", o.scene, t.resource)),
            &diff,
        )?;
        let share = |a: u64, b: u64| if b == 0 { 0.0 } else { a as f64 / b as f64 };
        t1_all += w1;
        t4_all += w4;
        n_all += n;
        report.tiles.push(TileReport {
            resource: t.resource.clone(),
            tile: (tx, ty),
            flipped,
            offset: (origin.0 - nominal.0, origin.1 - nominal.1),
            mean_error_nominal: nominal_err.0,
            mean_error_best: refined.0,
            texels: n,
            within_1: share(w1, n),
            within_4: share(w4, n),
            land_water_agreement: share(lw, n),
            by_relief: by_relief
                .into_iter()
                .map(|(k, (a, b, c))| (k, (a, share(b, a), share(c, a))))
                .collect(),
            crop: crop.display().to_string(),
        });
    }
    report.within_1 = if n_all == 0 {
        0.0
    } else {
        t1_all as f64 / n_all as f64
    };
    report.within_4 = if n_all == 0 {
        0.0
    } else {
        t4_all as f64 / n_all as f64
    };
    report.note.push(
        "threshold G-T1: >= 99.5 % within 1, the rest within 4; SIMP-035 (piece variant from our own stream) \
         keeps hills and mountains from agreeing texel by texel, see by_relief and the crops"
            .into(),
    );
    report.note.push(
        "G-T2 (cache pages) is not run: it needs our bake on a GPU and the page-to-cell table; \
         S2-bake is not a bake frame (the bake ran before it), final pages only"
            .into(),
    );
    write_report(o, &report)?;
    if report.tiles.is_empty() {
        bail!("no tile compared");
    }
    Ok(report)
}

/// The captures of a scene that have a texture list: `S1/S1a`, `S2/S2` and `S2/S2-bake`.
fn capture_dirs(cache: &Path, scene: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(cache.join("golden").join(scene))? {
        let e = e?;
        if e.path().join("textures.json").is_file() {
            out.push(e.file_name().to_string_lossy().into_owned());
        }
    }
    out.sort();
    Ok(out)
}

fn write_report(o: &Options, r: &Report) -> Result<()> {
    let path = o.out.join(format!("{}-terrain.json", o.scene));
    std::fs::write(&path, serde_json::to_vec_pretty(r)?)?;
    Ok(())
}
