//! The library of the terrain: the pieces of every continent style, the
//! table `TerrainDef`, the textures (diffuse, height, specular) the table points at, and the
//! groups the data is loaded in. The data comes from `assets-b2` (`pc:<style>`, `world_settings`,
//! `tx:`); this module holds it and answers the questions of the build and the cache.
//! [`TerrainLibrary::synthetic`] builds a library without the data of the game: a flat piece, a
//! hill, a mountain, a coast and a few textures, for the tests.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use asset_format::{
    LogicalId, PieceStyle, Section, TerrainDefRow, TerrainPiece, TerrainPieceSet, TextureBlob,
    TextureFormat,
};

use super::sampler::{TERRAIN_SCALE_Z, raw_from_height};

/// The group that holds the cubes, the colour keys, `TerrainDef` and the decal types.
// re: assets-b2 4.13 (the `boot` group)
pub const GROUP_BOOT: &str = "boot";
/// The group of the decals, needed before the first stage that makes one.
pub const GROUP_DECALS: &str = "decals";

/// The art style of `continent_art` (`Continent ID` of `TerrainDef.csv`): 0 Generic, 1 America,
/// 2 Asia, 3 Africa, 4 Europe.
// re: DLC/Shared/Terrain/TerrainDef.csv; g_aContinentPrefix
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

/// The inverse of [`style_of_continent`].
pub fn continent_of_style(style: PieceStyle) -> u8 {
    match style {
        PieceStyle::Generic => 0,
        PieceStyle::America => 1,
        PieceStyle::Asia => 2,
        PieceStyle::Africa => 3,
        PieceStyle::Europe => 4,
    }
}

/// The name of the group that holds the pieces of a style: `style:europe`.
pub fn style_group(style: PieceStyle) -> String {
    format!("style:{}", style.name())
}

/// The groups a map of these continent styles needs before its pieces can be placed: the group of
/// Generic (the coast, the ocean, the snow, `missing_hex`) always, the group of every style that
/// is in the map. Sorted, without repeats.
// re: (the groups `style:generic` and `style:<style>`)
pub fn needed_groups(continents: impl IntoIterator<Item = u8>) -> Vec<String> {
    let mut styles: BTreeSet<PieceStyle> = BTreeSet::new();
    styles.insert(PieceStyle::Generic);
    styles.extend(continents.into_iter().filter_map(style_of_continent));
    styles.into_iter().map(style_group).collect()
}

/// What a piece is a piece of. The classes follow the names of the pieces of the install.
// TODO(RW-6): the original takes the kind from the hex code of the piece (`g_aTTPostfix`,
// `g_pBaseTypeNames`); the names are the stand-in until the tables are answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BaseKind {
    Flat,
    Hill,
    Mountain,
    Coast,
    Ocean,
    /// A natural wonder: `Wonder_*`.
    Wonder,
    /// `missing_hex`, the spare.
    Spare,
}

impl BaseKind {
    /// The kind from the name of a piece (case does not matter).
    pub fn of_piece_name(name: &str) -> Self {
        let n = name.to_ascii_lowercase();
        if n.starts_with("missing_hex") {
            Self::Spare
        } else if n.contains("wonder_") {
            Self::Wonder
        } else if n.contains("moun") {
            Self::Mountain
        } else if n.contains("hill") {
            Self::Hill
        } else if n.contains("coast") {
            Self::Coast
        } else if n.contains("ocean") {
            Self::Ocean
        } else {
            Self::Flat
        }
    }
}

/// The table `TerrainDef`: one row per terrain type of a continent style.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerrainDefs {
    rows: Vec<TerrainDefRow>,
}

impl TerrainDefs {
    pub fn new(rows: Vec<TerrainDefRow>) -> Self {
        Self { rows }
    }

    pub fn rows(&self) -> &[TerrainDefRow] {
        &self.rows
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The rows of a continent, in the order of the file.
    pub fn of_continent(&self, continent: u8) -> impl Iterator<Item = &TerrainDefRow> {
        self.rows.iter().filter(move |r| r.continent == continent)
    }

    /// The row with this id in the continent (the id is unique in a continent).
    pub fn row(&self, continent: u8, id: u32) -> Option<&TerrainDefRow> {
        self.of_continent(continent).find(|r| r.id == id)
    }

    /// The first row of the continent whose diffuse texture has `needle` in its name (case does
    /// not matter).
    pub fn find_by_diffuse(&self, continent: u8, needle: &str) -> Option<&TerrainDefRow> {
        self.index_by_diffuse(continent, needle)
            .map(|i| &self.rows[i])
    }

    /// The index in the table of [`Self::find_by_diffuse`]: the number the field of the terrain
    /// stores for a texel (the table is the list of textures the cache blends).
    pub fn index_by_diffuse(&self, continent: u8, needle: &str) -> Option<usize> {
        let needle = needle.to_ascii_lowercase();
        self.rows.iter().position(|r| {
            r.continent == continent
                && r.diffuse
                    .as_ref()
                    .is_some_and(|d| d.as_str().to_ascii_lowercase().contains(&needle))
        })
    }

    /// The index of the first row of a continent.
    pub fn first_index(&self, continent: u8) -> Option<usize> {
        self.rows.iter().position(|r| r.continent == continent)
    }
}

/// The textures of a row of `TerrainDef`, as the library has them.
#[derive(Clone, Copy, Debug, Default)]
pub struct DefTextures<'a> {
    pub diffuse: Option<&'a TextureBlob>,
    pub height: Option<&'a TextureBlob>,
    pub specular: Option<&'a TextureBlob>,
}

/// The pieces, the table and the textures of the terrain.
#[derive(Clone, Debug, Default)]
pub struct TerrainLibrary {
    sets: BTreeMap<PieceStyle, Arc<TerrainPieceSet>>,
    defs: TerrainDefs,
    textures: HashMap<LogicalId, Arc<TextureBlob>>,
    groups: BTreeSet<String>,
}

impl TerrainLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the pieces of a style (the blob `pc:<style>`); the group `style:<style>` is loaded.
    pub fn add_piece_set(&mut self, set: TerrainPieceSet) {
        self.groups.insert(style_group(set.style));
        self.sets.insert(set.style, Arc::new(set));
    }

    /// Takes the table `TerrainDef` (`WorldSettings::terrain_defs`); the group `boot` is loaded
    /// with it.
    pub fn set_defs(&mut self, rows: Vec<TerrainDefRow>) {
        self.defs = TerrainDefs::new(rows);
        self.groups.insert(GROUP_BOOT.to_string());
    }

    pub fn add_texture(&mut self, id: LogicalId, blob: TextureBlob) {
        self.textures.insert(id, Arc::new(blob));
    }

    /// The pieces of a style, if its group is loaded.
    pub fn piece_set(&self, style: PieceStyle) -> Option<&Arc<TerrainPieceSet>> {
        self.sets.get(&style)
    }

    /// A piece by its name (lower case, as in the set).
    pub fn piece(&self, style: PieceStyle, name: &str) -> Option<&TerrainPiece> {
        self.sets
            .get(&style)?
            .pieces
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
    }

    /// The pieces of a style by kind, in the order of the set.
    pub fn pieces_of_kind(
        &self,
        style: PieceStyle,
        kind: BaseKind,
    ) -> impl Iterator<Item = &TerrainPiece> {
        self.sets
            .get(&style)
            .into_iter()
            .flat_map(|s| s.pieces.iter())
            .filter(move |p| BaseKind::of_piece_name(&p.name) == kind)
    }

    pub fn defs(&self) -> &TerrainDefs {
        &self.defs
    }

    pub fn texture(&self, id: &LogicalId) -> Option<&TextureBlob> {
        self.textures.get(id).map(|t| t.as_ref())
    }

    /// The textures a row points at; one the library does not hold is `None`.
    pub fn textures_of(&self, row: &TerrainDefRow) -> DefTextures<'_> {
        let get = |id: &Option<LogicalId>| id.as_ref().and_then(|i| self.texture(i));
        DefTextures {
            diffuse: get(&row.diffuse),
            height: get(&row.heightmap),
            specular: get(&row.specmap),
        }
    }

    /// Whether the group is loaded.
    pub fn has_group(&self, group: &str) -> bool {
        self.groups.contains(group)
    }

    /// The groups of `needed_groups(..)` that are not loaded: what the scheduler is asked for.
    pub fn missing_groups(&self, continents: impl IntoIterator<Item = u8>) -> Vec<String> {
        let mut out: Vec<String> = needed_groups(continents)
            .into_iter()
            .filter(|g| !self.has_group(g))
            .collect();
        if !self.has_group(GROUP_BOOT) {
            out.insert(0, GROUP_BOOT.to_string());
        }
        out
    }

    /// Whether every row of these continents has the diffuse and height textures it names in the library (the pages of the cache are baked from them; the specular map is not needed).
    pub fn textures_ready(&self, continents: impl IntoIterator<Item = u8>) -> bool {
        let mut seen = [false; 256];
        for c in continents {
            if std::mem::replace(&mut seen[usize::from(c)], true) {
                continue;
            }
            let mut any = false;
            for row in self.defs.of_continent(c) {
                any = true;
                // A row without a texture of its own uses a grey placeholder; one that names a texture
                // waits for it.
                let t = self.textures_of(row);
                if (row.diffuse.is_some() && t.diffuse.is_none())
                    || (row.heightmap.is_some() && t.height.is_none())
                {
                    return false;
                }
            }
            if !any {
                return false;
            }
        }
        true
    }

    /// Whether the data for a map of these continents is all here.
    pub fn is_ready_for(&self, continents: impl IntoIterator<Item = u8>) -> bool {
        self.missing_groups(continents).is_empty()
    }

    /// A library without the data of the game: the flat piece, the hill, the mountain, the coast
    /// and the ocean in every style, the rows of `TerrainDef` of every continent, and four small
    /// textures.
    pub fn synthetic() -> Self {
        let mut lib = Self::new();
        for style in PieceStyle::ALL {
            lib.add_piece_set(synthetic_piece_set(style));
        }
        let mut rows = Vec::new();
        for continent in 0..5u8 {
            for (id, name) in SYNTHETIC_ROWS.iter().enumerate() {
                rows.push(TerrainDefRow {
                    continent,
                    diffuse: Some(LogicalId::new(format!("tx:synthetic/{name}.dds"))),
                    heightmap: (*name == "mountain")
                        .then(|| LogicalId::new("tx:synthetic/mountain_h.dds")),
                    specmap: None,
                    base_dominance: u8::from(*name == "grass"),
                    texture_scale: 1.0,
                    projection: u8::from(*name == "mountain"),
                    id: id as u32,
                });
            }
        }
        lib.set_defs(rows);
        for (name, color) in [
            ("grass", [60u8, 140, 50, 255]),
            ("hill", [120, 110, 60, 255]),
            ("mountain", [130, 130, 135, 255]),
            ("coast", [190, 180, 130, 255]),
        ] {
            lib.add_texture(
                LogicalId::new(format!("tx:synthetic/{name}.dds")),
                solid_texture(color),
            );
        }
        lib.add_texture(
            LogicalId::new("tx:synthetic/mountain_h.dds"),
            solid_texture([128, 128, 128, 255]),
        );
        lib.groups.insert(GROUP_DECALS.to_string());
        lib
    }
}

/// The rows of the synthetic `TerrainDef`: the row index is the id.
pub const SYNTHETIC_ROWS: [&str; 4] = ["grass", "hill", "mountain", "coast"];

/// Side of the pieces of the synthetic library (the small pieces of the original).
const SYNTHETIC_SIDE: u16 = 128;

/// Raw height of the plain land and of the water.
// re: cvTerrainBlend.cpp:47-50; TerrainSystem.cpp:213, :936
pub const LAND_RAW: u16 = 0x3c3f;
pub const WATER_RAW: u16 = 13_000;

fn raw_of(world_height: f32) -> u16 {
    raw_from_height(world_height, TERRAIN_SCALE_Z)
}

fn solid_texture(rgba: [u8; 4]) -> TextureBlob {
    // 4 x 4, 2 x 2, 1 x 1.
    let mips = [4usize, 2, 1].iter().map(|&n| rgba.repeat(n * n)).collect();
    TextureBlob::new_2d(TextureFormat::Rgba8, false, 4, 4, mips)
}

fn u16_section(values: &[u16]) -> Section {
    Section::raw(values.iter().flat_map(|v| v.to_le_bytes()).collect())
        .expect("a piece is far below 4 GiB")
}

/// A synthetic piece: `side` x `side` texels of `height(x, y)` (in raw units), the terrain id
/// code `id_code`, blend maps that fall from full at the middle to nothing a hex away, and one
/// hex (row 1, column 1, the middle of the piece) with the class `hex_code`.
fn synthetic_piece(
    name: &str,
    flags: u32,
    hex_code: u32,
    id_code: u32,
    height: impl Fn(u32, u32) -> u16,
) -> TerrainPiece {
    let side = u32::from(SYNTHETIC_SIDE);
    let n = (side * side) as usize;
    let mut h = Vec::with_capacity(n);
    let mut blend = Vec::with_capacity(n * 4);
    for y in 0..side {
        for x in 0..side {
            h.push(height(x, y));
        }
    }
    // The weight falls with the distance from the middle (64, 64): 255 there, 0 a hex (64 texels) away.
    for _ in 0..4 {
        for y in 0..side {
            for x in 0..side {
                let d = ((x as f32 - 64.0).powi(2) + (y as f32 - 64.0).powi(2)).sqrt() / 64.0;
                blend.push((255.0 * (1.0 - d).clamp(0.0, 1.0)) as u8);
            }
        }
    }
    let mut hex_codes = vec![0x800f_ffffu32; 16];
    if hex_code != 0xffff_ffff {
        hex_codes[5] = hex_code;
    }
    TerrainPiece {
        name: name.to_string(),
        side: SYNTHETIC_SIDE,
        csv_version: 4,
        flags,
        csv_extra: Vec::new(),
        grid: (4, 4),
        hex_codes,
        height_g: u16_section(&h),
        height_a: u16_section(&h),
        height_diff: Vec::new(),
        id_palette: vec![id_code],
        id_index: Section::constant(&[0], n as u32),
        h_blend: Section::raw(blend.clone()).expect("a piece is far below 4 GiB"),
        t_blend: Section::raw(blend).expect("a piece is far below 4 GiB"),
    }
}

fn synthetic_piece_set(style: PieceStyle) -> TerrainPieceSet {
    let side = f32::from(SYNTHETIC_SIDE);
    // Distance from the middle, 0 at the centre and 1 at the middle of the edge.
    let r = move |x: u32, y: u32| {
        let (dx, dy) = (x as f32 + 0.5 - side / 2.0, y as f32 + 0.5 - side / 2.0);
        ((dx * dx + dy * dy).sqrt() / (side / 2.0)).min(1.0)
    };
    let hill = raw_of(25.0);
    let mountain = raw_of(49.0);
    let land = f32::from(LAND_RAW);
    // The id code of the row `row` of the style in the synthetic table.
    let code =
        |row: u32| crate::world::terrain::build::blit::row_code(row, continent_of_style(style));
    // The classes of the hexes: every flat type, every hill type, the mountain, the coast, the ocean.
    let (flat_class, hill_class, mountain_class) = (0x1f, 0x3e0, 0x400);
    let (coast_class, ocean_class) = (0x2000, 0x4000);
    let mut pieces = vec![
        synthetic_piece("flat_01", 1, flat_class, code(0), |_, _| LAND_RAW),
        synthetic_piece("hill_01", 1, hill_class, code(1), |x, y| {
            (land + (f32::from(hill) - land) * (1.0 - r(x, y))) as u16
        }),
        synthetic_piece("mountain_01", 1, mountain_class, code(2), |x, y| {
            (land + (f32::from(mountain) - land) * (1.0 - r(x, y))) as u16
        }),
        synthetic_piece("coast_01", 1, coast_class, code(3), |_, _| WATER_RAW),
        synthetic_piece("ocean_01", 1, ocean_class, code(3), |_, _| WATER_RAW),
        synthetic_piece("missing_hex", u32::MAX, 0xffff_ffff, code(0), |_, _| {
            LAND_RAW
        }),
    ];
    pieces.sort_by(|a, b| a.name.cmp(&b.name));
    TerrainPieceSet { style, pieces }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continents_and_styles_map_both_ways() {
        for c in 0..5u8 {
            assert_eq!(continent_of_style(style_of_continent(c).unwrap()), c);
        }
        assert_eq!(style_of_continent(1), Some(PieceStyle::America));
        assert_eq!(style_of_continent(3), Some(PieceStyle::Africa));
        assert_eq!(style_of_continent(9), None);
    }

    #[test]
    fn a_map_needs_generic_and_the_styles_it_has() {
        assert_eq!(needed_groups([]), ["style:generic"]);
        assert_eq!(
            needed_groups([4, 4, 1]),
            ["style:generic", "style:america", "style:europe"]
        );
        // An art the table does not know asks for nothing.
        assert_eq!(needed_groups([200]), ["style:generic"]);
    }

    #[test]
    fn the_kind_of_a_piece_comes_from_its_name() {
        for (n, k) in [
            ("Grass 01", BaseKind::Flat),
            ("Euro_Moun_1_1", BaseKind::Mountain),
            ("Afri_Mountain_2", BaseKind::Mountain),
            ("Afri_Desert_Hill_1_1", BaseKind::Hill),
            ("Wonder_Afri_Geyser", BaseKind::Wonder),
            ("Wonder_Reef", BaseKind::Wonder),
            ("Coast_03", BaseKind::Coast),
            ("Ocean_01", BaseKind::Ocean),
            ("missing_hex", BaseKind::Spare),
        ] {
            assert_eq!(BaseKind::of_piece_name(n), k, "{n}");
        }
    }

    #[test]
    fn the_synthetic_library_has_every_piece_in_every_style() {
        let lib = TerrainLibrary::synthetic();
        for style in PieceStyle::ALL {
            let set = lib.piece_set(style).expect("a set per style");
            assert_eq!(set.style, style);
            for kind in [
                BaseKind::Flat,
                BaseKind::Hill,
                BaseKind::Mountain,
                BaseKind::Coast,
                BaseKind::Ocean,
                BaseKind::Spare,
            ] {
                assert_eq!(
                    lib.pieces_of_kind(style, kind).count(),
                    1,
                    "{style} {kind:?}"
                );
            }
            for p in &set.pieces {
                p.validate().unwrap();
                let d = p.data().unwrap();
                assert_eq!(d.height_g.len(), 128 * 128);
            }
        }
        assert!(lib.piece(PieceStyle::Europe, "HILL_01").is_some());
        assert!(lib.piece(PieceStyle::Europe, "nope").is_none());
    }

    #[test]
    fn the_synthetic_pieces_have_the_heights_of_their_relief() {
        let lib = TerrainLibrary::synthetic();
        let height = |name: &str, x: usize, y: usize| {
            let p = lib.piece(PieceStyle::Generic, name).unwrap();
            p.data().unwrap().height_g[y * 128 + x]
        };
        assert_eq!(height("flat_01", 10, 10), LAND_RAW);
        // The hill is highest in the middle and falls to the land at the edge.
        let top = height("hill_01", 64, 64);
        assert!(top > LAND_RAW + 10_000, "{top}");
        assert!(height("hill_01", 0, 0) <= LAND_RAW + 1);
        assert!(height("mountain_01", 64, 64) > top);
        // The coast piece is the water.
        assert_eq!(height("coast_01", 0, 5), WATER_RAW);
        assert_eq!(height("ocean_01", 3, 3), WATER_RAW);
    }

    #[test]
    fn the_synthetic_table_points_at_textures_the_library_holds() {
        let lib = TerrainLibrary::synthetic();
        assert_eq!(lib.defs().len(), 5 * SYNTHETIC_ROWS.len());
        for continent in 0..5 {
            assert_eq!(lib.defs().of_continent(continent).count(), 4);
            for row in lib.defs().of_continent(continent) {
                let t = lib.textures_of(row);
                let d = t.diffuse.expect("a diffuse texture");
                assert_eq!((d.width, d.height), (4, 4));
                // Only the mountain has a height map.
                assert_eq!(t.height.is_some(), row.projection == 1);
                assert!(t.specular.is_none());
            }
        }
        let rock = lib.defs().find_by_diffuse(2, "MOUNTAIN").unwrap();
        assert_eq!((rock.continent, rock.id), (2, 2));
        assert_eq!(lib.defs().row(2, 3).unwrap().continent, 2);
        assert!(lib.defs().row(2, 9).is_none());
        assert!(lib.defs().find_by_diffuse(2, "lava").is_none());
    }

    #[test]
    fn groups_are_loaded_with_their_data() {
        let mut lib = TerrainLibrary::new();
        assert_eq!(
            lib.missing_groups([0, 4]),
            ["boot", "style:generic", "style:europe"]
        );
        assert!(!lib.is_ready_for([0]));
        lib.add_piece_set(synthetic_piece_set(PieceStyle::Generic));
        assert!(lib.has_group("style:generic"));
        lib.set_defs(Vec::new());
        assert_eq!(lib.missing_groups([0, 4]), ["style:europe"]);
        lib.add_piece_set(synthetic_piece_set(PieceStyle::Europe));
        assert!(lib.is_ready_for([0, 4]));
        // The synthetic library has everything, the decals too.
        assert!(TerrainLibrary::synthetic().is_ready_for(0..5));
        assert!(TerrainLibrary::synthetic().has_group(GROUP_DECALS));
    }
}
