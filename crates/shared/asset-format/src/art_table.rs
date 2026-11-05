//! Art table (`C5AT`): game type string -> assets and render parameters.
//!
//! Keys:
//!
//! | Key | Record |
//! |---|---|
//! | `unit/<UNIT_TYPE>` | [`UnitArt`] |
//! | `terrain/<TERRAIN_TYPE>/<0-4>` | [`TerrainArt`] |
//! | `feature/<FEATURE_TYPE>/<0-4>` | [`FeatureArt`] |
//! | `river` | [`RiverArt`] |
//! | `city/<culture>/<era_bucket>` | [`CityArt`] |
//! | `sv/<ArtDefineTag>` | [`SvArt`] |
//! | `player_color/<PLAYERCOLOR_TYPE>` | [`PlayerColor`] |
//! | `formation/<name>` | [`Formation`] |
//!
//! Framing is the common blob header; the payload is a postcard list of `(key, record)` sorted by
//! key, so equal tables encode to equal bytes. Decoding checks that every key matches its record
//! kind and that keys are unique.
//!
//! # Version 2
//! The records of version 1 keep their fields and their place in [`ArtRecord`]; new records are
//! appended. A table that holds only version-1 records is still written as version 1 (the B1
//! sets keep their hashes); one with a B2 record is written as version 2. Both are read.
//!
//! | Key | Record |
//! |---|---|
//! | `unit/<UNIT_TYPE>` | [`ArtRecord::UnitB2`] (in a B2 table; members are `md:` models) |
//! | `vis_effect/<Name>` | [`ArtRecord::VisEffect`] (`VisEffectArtInfo` of the XML defines) |
//! | `landmark/<improvement>/<resource>/<n>` | [`ArtRecord::Landmark`] (one `ArtDefine_Landmarks` row; `n` counts the rows of the pair in table order) |
//! | `building/<ArtDefineTag>` | [`ArtRecord::Building`] |
//! | `wonder/<Type>` | [`ArtRecord::Wonder`] (`WonderArtInfo`) |
//! | `city/<culture>/<era>` | [`ArtRecord::CityB2`] (a B2 table holds these instead of [`ArtRecord::City`]) |
//! | `wall/<culture>/<era>` | [`ArtRecord::Wall`] |
//! | `route/<ROUTE_*>` | [`ArtRecord::Route`] |
//! | `overlay/<name>` | [`ArtRecord::Overlay`] |
//! | `tree/<FEATURE_*>/<style 0-4>` | [`ArtRecord::Tree`] |
//! | `civ/<CIVILIZATION_*>` | [`ArtRecord::Civ`] |

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{FormatError, LogicalId, blob};

pub const ART_TABLE_MAGIC: [u8; 4] = *b"C5AT";

/// How a terrain texture is mapped onto the ground.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Projection {
    /// `uv = (world X, world Z) * scale`.
    WorldXz,
    /// The mesh's own UVs.
    MeshUv,
}

/// One figure of a unit squad (`ArtDefine_UnitInfoMemberInfos` / `UnitMemberInfos`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Figure {
    pub model: LogicalId,
    pub count: u32,
    pub scale: f32,
    pub z_offset: f32,
}

/// Art of a unit, or of one style variant of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnitArt {
    /// `ArtDefine_UnitInfos` type.
    pub art_info: String,
    pub figures: Vec<Figure>,
    /// Key suffix of the `formation/<name>` record; the formation name from `ArtDefine_UnitInfos`.
    pub formation: Option<String>,
    pub sv_icon: Option<LogicalId>,
    /// The models are stand-ins (SIMP-011).
    pub placeholder: bool,
}

/// Variant by `ArtStyleSuffix` (`_EURO`, `_AFRI`, `_AMER`, `_ASIA`, `_MED`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnitVariant {
    pub suffix: String,
    pub art: UnitArt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnitRecord {
    pub base: UnitArt,
    /// Sorted by suffix on write.
    pub variants: Vec<UnitVariant>,
}

impl UnitRecord {
    /// Art for a style suffix; the base art when there is no such variant.
    pub fn for_style(&self, suffix: Option<&str>) -> &UnitArt {
        suffix
            .and_then(|s| self.variants.iter().find(|v| v.suffix == s))
            .map_or(&self.base, |v| &v.art)
    }
}

/// One figure kind of a unit squad in a B2 table: the DB values as they are. A NULL `Scale` or
/// `ZOffset` stays `None` (the engine default is RW-14 (д), not guessed here).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FigureB2 {
    /// `ArtDefine_UnitMemberInfos.Type`.
    pub member: String,
    /// The `md:` model of the member's `.fxsxml`.
    pub model: LogicalId,
    pub count: u32,
    pub scale: Option<f32>,
    pub z_offset: Option<f32>,
    /// `ArtDefine_UnitMemberInfos.Domain` (`Sea`, `Land`, `Air`), when set.
    pub domain: Option<String>,
}

/// B2 art of a unit, or of one variant of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnitArtB2 {
    /// `ArtDefine_UnitInfos` type.
    pub art_info: String,
    pub figures: Vec<FigureB2>,
    /// `ArtDefine_UnitInfos.DamageStates`.
    pub damage_states: Option<u8>,
    /// Name of the `formation/<name>` record.
    pub formation: Option<String>,
}

/// A variant of a unit: by `ArtStyleSuffix` (`_EURO`, ...) or by era (`_LATE`, ...).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnitVariantB2 {
    pub suffix: String,
    pub art: UnitArtB2,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnitRecordB2 {
    pub base: UnitArtB2,
    /// Sorted by suffix on write.
    pub style_variants: Vec<UnitVariantB2>,
    /// `UnitArtInfoEraVariation = 1`: the art changes with the era of the owner.
    pub era_variation: bool,
    /// The `ArtDefine_UnitInfos` types that look like era variants of the base art (the base type
    /// plus an era tag, see the pipeline). How the engine picks one is not read (RW-14); sorted by
    /// suffix on write.
    pub era_variants: Vec<UnitVariantB2>,
}

impl UnitRecordB2 {
    /// Art for a style suffix; the base art when there is no such variant.
    pub fn for_style(&self, suffix: Option<&str>) -> &UnitArtB2 {
        suffix
            .and_then(|s| self.style_variants.iter().find(|v| v.suffix == s))
            .map_or(&self.base, |v| &v.art)
    }
}

/// A `VisEffectArtInfo` of `Civ5ArtDefines_VisEffects*.xml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VisEffectArt {
    pub name: String,
    pub translation: [f32; 3],
    /// Degrees.
    pub rotation: [f32; 3],
    pub scale: f32,
    pub base_duration: f32,
    pub base_duration_scale: f32,
    pub movement_speed: Option<f32>,
    pub fow_mask: Option<u8>,
    /// `ParticleEffect` names (`ART_DEF_PEFFECT_*`).
    pub particles: Vec<String>,
    /// `ModelEffect` names (`ART_DEF_MEFFECT_*`) with the `Animation` code, when given.
    pub models: Vec<(String, Option<u32>)>,
    /// `ChildEffect`: trigger and name.
    pub children: Vec<(String, String)>,
    /// `SoundEffect` names.
    pub sounds: Vec<String>,
}

// ---------------------------------------------------------------- world objects (B2, R4)

/// One row of `ArtDefine_Landmarks` as the DB has it (4.10). The pipeline puts every row in and
/// chooses nothing: the order of matching (exact before `Any`, era, state, technology) and the
/// random variant are the client's (RW-15).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LandmarkArt {
    /// `ImprovementType`: `ART_DEF_IMPROVEMENT_*` or `ART_DEF_FEATURE_*`.
    pub improvement: String,
    /// `ResourceType`: `ART_DEF_RESOURCE_*` (`ART_DEF_RESOURCE_ALL` for any).
    pub resource: String,
    /// `Era`: `Ancient`, `Industrial` or `Any` in the snapshot.
    pub era: String,
    /// `State`: `Constructed`, `Pillaged`, `UnderConstruction` or `Any`.
    pub state: String,
    /// `LayoutHandler` as written (`SNAPSHOT`, `RANDOM`, `FARM`, `SPECIAL`, `ANIMATED`,
    /// `ANIMATED_ROTATED`, `PASTURE`, `SPRING`).
    pub layout: String,
    pub scale: f32,
    pub terrain_contour: i32,
    /// `Tech`, when the row is for a technology only.
    pub tech: Option<String>,
    /// The `md:` model; `None` when the row's `Model` is NULL (the row draws nothing).
    pub model: Option<LogicalId>,
}

/// The `ART_DEF_FEATURE_*` types of the natural wonders (the landmark rows of a wonder's model).
// re: Lekmod `Civ5ArtDefines_Landmarks*` / `Civ5ArtDefines_LandmarkTypes*` (RE-B12 item 1)
pub const NATURAL_WONDER_ART: [&str; 18] = [
    "ART_DEF_FEATURE_CRATER",
    "ART_DEF_FEATURE_EL_DORADO",
    "ART_DEF_FEATURE_EVEREST",
    "ART_DEF_FEATURE_FOUNTAIN_YOUTH",
    "ART_DEF_FEATURE_FUJI",
    "ART_DEF_FEATURE_GEYSER",
    "ART_DEF_FEATURE_GIBRALTAR",
    "ART_DEF_FEATURE_KILIMANJARO",
    "ART_DEF_FEATURE_LAKE_VICTORIA",
    "ART_DEF_FEATURE_MESA",
    "ART_DEF_FEATURE_MT_KAILASH",
    "ART_DEF_FEATURE_MT_SINAI",
    "ART_DEF_FEATURE_POTOSI",
    "ART_DEF_FEATURE_REEF",
    "ART_DEF_FEATURE_SOLOMONS_MINES",
    "ART_DEF_FEATURE_SRI_PADA",
    "ART_DEF_FEATURE_TITICACA",
    "ART_DEF_FEATURE_ULURU",
];

impl LandmarkArt {
    /// True if the row is the model of a natural wonder (`ResourceType` or `ImprovementType` is one
    /// of [`NATURAL_WONDER_ART`], plus the volcano and Victoria Falls).
    pub fn is_natural_wonder(&self) -> bool {
        is_natural_wonder_art(&self.resource) || is_natural_wonder_art(&self.improvement)
    }
}

/// True if `name` is the `ART_DEF_FEATURE_*` type of a natural wonder.
pub fn is_natural_wonder_art(name: &str) -> bool {
    NATURAL_WONDER_ART.contains(&name)
        || matches!(
            name,
            "ART_DEF_FEATURE_VOLCANO" | "ART_DEF_FEATURE_VICTORIA_FALLS"
        )
}

/// One variant of a building art: the model of a culture, an era or a random pick.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildingVariant {
    /// `culture`, `era` or `random`.
    pub kind: String,
    /// `AFRICAN` / `ANCIENT` / `ONE`: the word in front of or behind the tag in the wonder art
    /// `Type` (`AFRICAN CASTLE`, `ANCIENT HARBOR`, `STADIUM ONE`).
    pub key: String,
    /// The wonder art `Type` that holds the model (a `wonder/<Type>` record).
    pub art_type: String,
    pub model: LogicalId,
    pub scale: f32,
}

/// The art of the buildings that share one `Buildings.ArtDefineTag`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildingArt {
    pub art_tag: String,
    /// The `Buildings.Type` rows that use the tag, sorted.
    pub building_types: Vec<String>,
    /// `ArtInfoCulturalVariation`, `ArtInfoEraVariation`, `ArtInfoRandomVariation` of the rows.
    pub cultural_variation: bool,
    pub era_variation: bool,
    pub random_variation: bool,
    /// The model of the wonder art whose `Type` is the tag itself.
    pub model: Option<LogicalId>,
    pub scale: f32,
    pub variants: Vec<BuildingVariant>,
    /// Why there is no model of its own (a building without art of its own is drawn as part of
    /// the city catalog, RW-18); `None` when there is one.
    pub note: Option<String>,
}

/// One `Buildings` row as the city art needs it: its tag and `DisplayPosition` (the placement kind
/// of its model in the city, a bit value; 0 is no model of its own, even when the tag has one).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildingTypeArt {
    pub building_type: String,
    pub art_tag: String,
    pub display_position: u8,
}

/// One `WonderArtState` of a `WonderArtInfo`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WonderState {
    /// The `state` attribute (`Any` in the install; the exe reads the first state only).
    pub state: String,
    /// `fScale` as written (the exe takes 1.0 when it is not above 0).
    pub scale: f32,
    pub model: LogicalId,
}

/// A `WonderArtInfo` of `Civ5ArtDefines_*Wonders.xml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WonderArt {
    pub art_type: String,
    pub states: Vec<WonderState>,
}

/// A city catalog model of a culture and an era (`Civ5ArtDefines_CityBuildings.xml`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CityArtB2 {
    pub model: LogicalId,
    /// Parts of the catalog (gr2 `Model`s: base quads and buildings); the choice and the layout
    /// are the client's (RW-18).
    pub parts: u16,
    /// The `lighting` attribute of the `<Culture>`, when given.
    pub lighting: Option<String>,
}

/// The wall model of a culture (the install has one per culture, not per era).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WallArt {
    pub model: LogicalId,
    pub parts: u16,
}

/// A model that belongs to a route type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteModel {
    /// `road` or `bridge`: the class by the name of the file until RW-17 answers how the exe
    /// picks.
    pub class: String,
    pub model: LogicalId,
}

/// The models of a route type (`Routes.Type`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteArt {
    pub route: String,
    pub models: Vec<RouteModel>,
}

/// A model of `Assets/UI/Overlays` or a set of overlay textures.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverlayArt {
    pub name: String,
    /// `path_center`, `path_selected`, `path_connector`, `path_flank`, `ring`, `highlight`.
    pub kind: String,
    /// The variant inside the kind (`OneTurn`, `Attack`, `00`, ...); empty when there is one.
    pub variant: String,
    pub model: Option<LogicalId>,
    pub textures: Vec<LogicalId>,
}

/// The trees of one feature (forest or jungle) in one continent style.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TreeArt {
    /// `FEATURE_FOREST` or `FEATURE_JUNGLE`.
    pub feature: String,
    pub style: u8,
    /// One model, a mesh per tree variant.
    pub model: LogicalId,
    /// The ground overlay under the trees.
    pub overlay: LogicalId,
    /// `forest_tile1..N` placement masks (R8); the same for every record.
    pub masks: Vec<LogicalId>,
}

/// A civilization as the art needs it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CivArt {
    pub civ: String,
    pub art_define_tag: Option<String>,
    /// `ArtStyleType` (`ARTSTYLE_EUROPEAN`).
    pub art_style: Option<String>,
    /// `ArtStylePrefix` (`EUROPEAN`): the culture key of the `city/` records.
    pub prefix: Option<String>,
    /// `ArtStyleSuffix` (`_EURO`): the key of the unit style variants.
    pub suffix: Option<String>,
    pub default_color: Option<String>,
    /// The `city/` culture the art style type points at when it differs from the prefix (the
    /// exe's culture index is the style type, RW-18); `None` when they agree.
    pub style_culture: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainArt {
    pub texture: LogicalId,
    /// `Texture Scale` column of `terraindef.csv`.
    pub texture_scale: f32,
    pub projection: Projection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureArt {
    pub texture: Option<LogicalId>,
    pub tree_model: Option<LogicalId>,
    /// Placement parameters (`forests.xml`) by name.
    pub placement: BTreeMap<String, f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RiverArt {
    pub textures: Vec<LogicalId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CityArt {
    pub model: LogicalId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SvArt {
    pub icon: LogicalId,
}

/// RGBA8 colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerColor {
    pub primary: [u8; 4],
    pub secondary: [u8; 4],
    pub text: [u8; 4],
}

/// Parameters of `Assets/Units/UnitFormations.xml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Formation {
    pub spacing_x: f32,
    pub spacing_y: f32,
    pub total_width: f32,
    pub unit_randomness: f32,
    pub stagger: f32,
    /// Other attributes by name.
    pub extra: BTreeMap<String, f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ArtRecord {
    Unit(UnitRecord),
    Terrain(TerrainArt),
    Feature(FeatureArt),
    River(RiverArt),
    City(CityArt),
    Sv(SvArt),
    PlayerColor(PlayerColor),
    Formation(Formation),
    // Version 2 (appended: the indices above must not move).
    UnitB2(UnitRecordB2),
    VisEffect(VisEffectArt),
    Landmark(LandmarkArt),
    Building(BuildingArt),
    Wonder(WonderArt),
    CityB2(CityArtB2),
    Wall(WallArt),
    Route(RouteArt),
    Overlay(OverlayArt),
    Tree(TreeArt),
    Civ(CivArt),
    // Appended later in version 2: a table written before it still reads.
    BuildingType(BuildingTypeArt),
}

impl ArtRecord {
    /// Records that only a version-2 table can hold.
    fn is_b2(&self) -> bool {
        !matches!(
            self,
            Self::Unit(_)
                | Self::Terrain(_)
                | Self::Feature(_)
                | Self::River(_)
                | Self::City(_)
                | Self::Sv(_)
                | Self::PlayerColor(_)
                | Self::Formation(_)
        )
    }

    fn matches_key(&self, key: &str) -> bool {
        let (prefix, parts): (&str, Option<usize>) = match self {
            Self::Unit(_) | Self::UnitB2(_) => ("unit", Some(1)),
            Self::VisEffect(_) => ("vis_effect", Some(1)),
            Self::Landmark(_) => ("landmark", Some(3)),
            Self::Building(_) => ("building", Some(1)),
            Self::Wonder(_) => ("wonder", Some(1)),
            Self::CityB2(_) => ("city", Some(2)),
            Self::Wall(_) => ("wall", Some(2)),
            Self::Route(_) => ("route", Some(1)),
            Self::Overlay(_) => ("overlay", Some(1)),
            Self::Tree(_) => ("tree", Some(2)),
            Self::Civ(_) => ("civ", Some(1)),
            Self::BuildingType(_) => ("building_type", Some(1)),
            Self::Terrain(_) => ("terrain", Some(2)),
            Self::Feature(_) => ("feature", Some(2)),
            Self::River(_) => return key == "river",
            Self::City(_) => ("city", Some(2)),
            Self::Sv(_) => ("sv", Some(1)),
            Self::PlayerColor(_) => ("player_color", Some(1)),
            Self::Formation(_) => ("formation", Some(1)),
        };
        let mut it = key.split('/');
        if it.next() != Some(prefix) {
            return false;
        }
        let rest: Vec<&str> = it.collect();
        let shape_ok = rest.len() == parts.unwrap_or(0) && rest.iter().all(|s| !s.is_empty());
        let continent_ok = match self {
            Self::Terrain(_) | Self::Feature(_) | Self::Tree(_) => rest
                .get(1)
                .is_some_and(|c| matches!(*c, "0" | "1" | "2" | "3" | "4")),
            _ => true,
        };
        shape_ok && continent_ok
    }
}

/// Key -> record. Typed `insert_*` / getters build and parse the keys.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArtTable {
    entries: BTreeMap<String, ArtRecord>,
}

impl ArtTable {
    /// Newest version this build reads and writes; a table without B2 records is written as 1.
    pub const VERSION: u16 = 2;
    pub const MIN_VERSION: u16 = 1;

    pub fn new() -> Self {
        Self::default()
    }

    /// Deterministic: entries sorted by key, unit variants by suffix.
    pub fn encode(&self) -> Vec<u8> {
        let entries: Vec<(&String, ArtRecord)> = self
            .entries
            .iter()
            .map(|(k, r)| {
                let mut r = r.clone();
                match &mut r {
                    ArtRecord::Unit(u) => u.variants.sort_by(|a, b| a.suffix.cmp(&b.suffix)),
                    ArtRecord::UnitB2(u) => {
                        u.style_variants.sort_by(|a, b| a.suffix.cmp(&b.suffix));
                        u.era_variants.sort_by(|a, b| a.suffix.cmp(&b.suffix));
                    }
                    _ => {}
                }
                (k, r)
            })
            .collect();
        let version = if self.entries.values().any(ArtRecord::is_b2) {
            Self::VERSION
        } else {
            1
        };
        blob::encode_v(ART_TABLE_MAGIC, version, &entries)
    }

    /// Rejects wrong magic, unknown version, duplicate keys and keys that do not fit their record.
    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (version, entries): (u16, Vec<(String, ArtRecord)>) =
            blob::decode_range(ART_TABLE_MAGIC, Self::MIN_VERSION, Self::VERSION, bytes)?;
        let mut t = Self::new();
        for (key, rec) in entries {
            if version == 1 && rec.is_b2() {
                return Err(FormatError::invalid(format!(
                    "art table key {key:?}: a version 1 table has no B2 records"
                )));
            }
            if !rec.matches_key(&key) {
                return Err(FormatError::invalid(format!(
                    "art table key {key:?} does not match its record kind"
                )));
            }
            if t.entries.insert(key.clone(), rec).is_some() {
                return Err(FormatError::invalid(format!(
                    "duplicate art table key {key}"
                )));
            }
        }
        Ok(t)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// All entries in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &ArtRecord)> {
        self.entries.iter().map(|(k, r)| (k.as_str(), r))
    }

    pub fn get(&self, key: &str) -> Option<&ArtRecord> {
        self.entries.get(key)
    }

    /// Replaces any record under the same key.
    pub fn insert_unit(&mut self, unit_type: &str, rec: UnitRecord) {
        self.put(format!("unit/{unit_type}"), ArtRecord::Unit(rec));
    }

    pub fn insert_unit_b2(&mut self, unit_type: &str, rec: UnitRecordB2) {
        self.put(format!("unit/{unit_type}"), ArtRecord::UnitB2(rec));
    }

    pub fn insert_vis_effect(&mut self, rec: VisEffectArt) {
        self.put(
            format!("vis_effect/{}", rec.name),
            ArtRecord::VisEffect(rec),
        );
    }

    pub fn insert_landmark(&mut self, n: usize, rec: LandmarkArt) {
        self.put(
            format!("landmark/{}/{}/{n}", rec.improvement, rec.resource),
            ArtRecord::Landmark(rec),
        );
    }

    pub fn insert_building(&mut self, rec: BuildingArt) {
        self.put(
            format!("building/{}", rec.art_tag),
            ArtRecord::Building(rec),
        );
    }

    pub fn insert_wonder(&mut self, rec: WonderArt) {
        self.put(format!("wonder/{}", rec.art_type), ArtRecord::Wonder(rec));
    }

    pub fn insert_city_b2(&mut self, culture: &str, era: &str, rec: CityArtB2) {
        self.put(format!("city/{culture}/{era}"), ArtRecord::CityB2(rec));
    }

    pub fn insert_wall(&mut self, culture: &str, era: &str, rec: WallArt) {
        self.put(format!("wall/{culture}/{era}"), ArtRecord::Wall(rec));
    }

    pub fn insert_route(&mut self, rec: RouteArt) {
        self.put(format!("route/{}", rec.route), ArtRecord::Route(rec));
    }

    pub fn insert_overlay(&mut self, rec: OverlayArt) {
        self.put(format!("overlay/{}", rec.name), ArtRecord::Overlay(rec));
    }

    pub fn insert_tree(&mut self, rec: TreeArt) {
        self.put(
            format!("tree/{}/{}", rec.feature, rec.style),
            ArtRecord::Tree(rec),
        );
    }

    pub fn insert_civ(&mut self, rec: CivArt) {
        self.put(format!("civ/{}", rec.civ), ArtRecord::Civ(rec));
    }

    pub fn landmark(&self, improvement: &str, resource: &str, n: usize) -> Option<&LandmarkArt> {
        match self
            .entries
            .get(&format!("landmark/{improvement}/{resource}/{n}"))?
        {
            ArtRecord::Landmark(r) => Some(r),
            _ => None,
        }
    }

    /// Every landmark row, in key order.
    pub fn landmarks(&self) -> impl Iterator<Item = &LandmarkArt> {
        self.entries.values().filter_map(|r| match r {
            ArtRecord::Landmark(l) => Some(l),
            _ => None,
        })
    }

    pub fn insert_building_type(&mut self, rec: BuildingTypeArt) {
        self.put(
            format!("building_type/{}", rec.building_type),
            ArtRecord::BuildingType(rec),
        );
    }

    /// The `building_type/<Type>` record; `None` in a table built before they were written.
    pub fn building_type(&self, building_type: &str) -> Option<&BuildingTypeArt> {
        match self
            .entries
            .get(&format!("building_type/{building_type}"))?
        {
            ArtRecord::BuildingType(r) => Some(r),
            _ => None,
        }
    }

    /// True when the table holds the `building_type/` records (the `DisplayPosition` of each
    /// building).
    pub fn has_building_types(&self) -> bool {
        self.entries
            .range("building_type/".to_owned()..)
            .next()
            .is_some_and(|(k, _)| k.starts_with("building_type/"))
    }

    pub fn building(&self, art_tag: &str) -> Option<&BuildingArt> {
        match self.entries.get(&format!("building/{art_tag}"))? {
            ArtRecord::Building(r) => Some(r),
            _ => None,
        }
    }

    pub fn wonder(&self, art_type: &str) -> Option<&WonderArt> {
        match self.entries.get(&format!("wonder/{art_type}"))? {
            ArtRecord::Wonder(r) => Some(r),
            _ => None,
        }
    }

    pub fn city_b2(&self, culture: &str, era: &str) -> Option<&CityArtB2> {
        match self.entries.get(&format!("city/{culture}/{era}"))? {
            ArtRecord::CityB2(r) => Some(r),
            _ => None,
        }
    }

    pub fn wall(&self, culture: &str, era: &str) -> Option<&WallArt> {
        match self.entries.get(&format!("wall/{culture}/{era}"))? {
            ArtRecord::Wall(r) => Some(r),
            _ => None,
        }
    }

    pub fn route(&self, route: &str) -> Option<&RouteArt> {
        match self.entries.get(&format!("route/{route}"))? {
            ArtRecord::Route(r) => Some(r),
            _ => None,
        }
    }

    pub fn overlay(&self, name: &str) -> Option<&OverlayArt> {
        match self.entries.get(&format!("overlay/{name}"))? {
            ArtRecord::Overlay(r) => Some(r),
            _ => None,
        }
    }

    pub fn tree(&self, feature: &str, style: u8) -> Option<&TreeArt> {
        match self.entries.get(&format!("tree/{feature}/{style}"))? {
            ArtRecord::Tree(r) => Some(r),
            _ => None,
        }
    }

    pub fn civ(&self, civ: &str) -> Option<&CivArt> {
        match self.entries.get(&format!("civ/{civ}"))? {
            ArtRecord::Civ(r) => Some(r),
            _ => None,
        }
    }

    /// Every `md:` model id the records name, with the kind of record that names it (for the
    /// groups and the budget articles): `unit`, `landmark`, `building`, `wonder`, `city`, `wall`,
    /// `route`, `overlay`, `tree`. A model that several records name is listed once for each.
    pub fn models(&self) -> Vec<(&'static str, &LogicalId)> {
        let mut out: Vec<(&'static str, &LogicalId)> = Vec::new();
        for r in self.entries.values() {
            match r {
                ArtRecord::UnitB2(u) => {
                    let arts = std::iter::once(&u.base)
                        .chain(u.style_variants.iter().map(|v| &v.art))
                        .chain(u.era_variants.iter().map(|v| &v.art));
                    for a in arts {
                        out.extend(a.figures.iter().map(|f| ("unit", &f.model)));
                    }
                }
                ArtRecord::Landmark(l) => out.extend(l.model.iter().map(|m| ("landmark", m))),
                ArtRecord::Building(b) => {
                    out.extend(b.model.iter().map(|m| ("building", m)));
                    out.extend(b.variants.iter().map(|v| ("building", &v.model)));
                }
                ArtRecord::Wonder(w) => out.extend(w.states.iter().map(|s| ("wonder", &s.model))),
                ArtRecord::CityB2(c) => out.push(("city", &c.model)),
                ArtRecord::Wall(w) => out.push(("wall", &w.model)),
                ArtRecord::Route(r) => out.extend(r.models.iter().map(|m| ("route", &m.model))),
                ArtRecord::Overlay(o) => out.extend(o.model.iter().map(|m| ("overlay", m))),
                ArtRecord::Tree(t) => out.push(("tree", &t.model)),
                _ => {}
            }
        }
        out
    }

    pub fn insert_terrain(&mut self, terrain_type: &str, continent: u8, rec: TerrainArt) {
        self.put(
            format!("terrain/{terrain_type}/{continent}"),
            ArtRecord::Terrain(rec),
        );
    }

    pub fn insert_feature(&mut self, feature_type: &str, continent: u8, rec: FeatureArt) {
        self.put(
            format!("feature/{feature_type}/{continent}"),
            ArtRecord::Feature(rec),
        );
    }

    pub fn insert_river(&mut self, rec: RiverArt) {
        self.put("river".to_owned(), ArtRecord::River(rec));
    }

    pub fn insert_city(&mut self, culture: &str, era_bucket: &str, rec: CityArt) {
        self.put(format!("city/{culture}/{era_bucket}"), ArtRecord::City(rec));
    }

    pub fn insert_sv(&mut self, tag: &str, rec: SvArt) {
        self.put(format!("sv/{tag}"), ArtRecord::Sv(rec));
    }

    pub fn insert_player_color(&mut self, color_type: &str, rec: PlayerColor) {
        self.put(
            format!("player_color/{color_type}"),
            ArtRecord::PlayerColor(rec),
        );
    }

    pub fn insert_formation(&mut self, name: &str, rec: Formation) {
        self.put(format!("formation/{name}"), ArtRecord::Formation(rec));
    }

    pub fn unit(&self, unit_type: &str) -> Option<&UnitRecord> {
        match self.entries.get(&format!("unit/{unit_type}"))? {
            ArtRecord::Unit(r) => Some(r),
            _ => None,
        }
    }

    pub fn unit_b2(&self, unit_type: &str) -> Option<&UnitRecordB2> {
        match self.entries.get(&format!("unit/{unit_type}"))? {
            ArtRecord::UnitB2(r) => Some(r),
            _ => None,
        }
    }

    pub fn vis_effect(&self, name: &str) -> Option<&VisEffectArt> {
        match self.entries.get(&format!("vis_effect/{name}"))? {
            ArtRecord::VisEffect(r) => Some(r),
            _ => None,
        }
    }

    pub fn terrain(&self, terrain_type: &str, continent: u8) -> Option<&TerrainArt> {
        match self
            .entries
            .get(&format!("terrain/{terrain_type}/{continent}"))?
        {
            ArtRecord::Terrain(r) => Some(r),
            _ => None,
        }
    }

    pub fn feature(&self, feature_type: &str, continent: u8) -> Option<&FeatureArt> {
        match self
            .entries
            .get(&format!("feature/{feature_type}/{continent}"))?
        {
            ArtRecord::Feature(r) => Some(r),
            _ => None,
        }
    }

    pub fn river(&self) -> Option<&RiverArt> {
        match self.entries.get("river")? {
            ArtRecord::River(r) => Some(r),
            _ => None,
        }
    }

    pub fn city(&self, culture: &str, era_bucket: &str) -> Option<&CityArt> {
        match self.entries.get(&format!("city/{culture}/{era_bucket}"))? {
            ArtRecord::City(r) => Some(r),
            _ => None,
        }
    }

    pub fn sv(&self, tag: &str) -> Option<&SvArt> {
        match self.entries.get(&format!("sv/{tag}"))? {
            ArtRecord::Sv(r) => Some(r),
            _ => None,
        }
    }

    pub fn player_color(&self, color_type: &str) -> Option<&PlayerColor> {
        match self.entries.get(&format!("player_color/{color_type}"))? {
            ArtRecord::PlayerColor(r) => Some(r),
            _ => None,
        }
    }

    pub fn formation(&self, name: &str) -> Option<&Formation> {
        match self.entries.get(&format!("formation/{name}"))? {
            ArtRecord::Formation(r) => Some(r),
            _ => None,
        }
    }

    fn put(&mut self, key: String, rec: ArtRecord) {
        self.entries.insert(key, rec);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> LogicalId {
        LogicalId::new(s)
    }

    fn unit_art(tag: &str) -> UnitArt {
        UnitArt {
            art_info: format!("ART_DEF_UNIT_{tag}"),
            figures: vec![Figure {
                model: id(&format!("model:{tag}")),
                count: 3,
                scale: 0.14,
                z_offset: 1.5,
            }],
            formation: Some("Infantry".to_owned()),
            sv_icon: Some(id("tex:sv_warrior")),
            placeholder: false,
        }
    }

    fn full_table() -> ArtTable {
        let mut t = ArtTable::new();
        t.insert_unit(
            "UNIT_WARRIOR",
            UnitRecord {
                base: unit_art("WARRIOR"),
                variants: vec![
                    UnitVariant {
                        suffix: "_EURO".to_owned(),
                        art: unit_art("WARRIOR_EURO"),
                    },
                    UnitVariant {
                        suffix: "_AFRI".to_owned(),
                        art: unit_art("WARRIOR_AFRI"),
                    },
                ],
            },
        );
        t.insert_terrain(
            "TERRAIN_GRASS",
            2,
            TerrainArt {
                texture: id("tex:grass_c2"),
                texture_scale: 0.25,
                projection: Projection::WorldXz,
            },
        );
        t.insert_feature(
            "FEATURE_FOREST",
            0,
            FeatureArt {
                texture: Some(id("tex:forest")),
                tree_model: Some(id("model:forest_euro")),
                placement: BTreeMap::from([
                    ("density".to_owned(), 0.5),
                    ("jitter".to_owned(), 0.1),
                ]),
            },
        );
        t.insert_river(RiverArt {
            textures: vec![id("tex:river_a"), id("tex:river_b")],
        });
        t.insert_city(
            "EURO",
            "Ancient",
            CityArt {
                model: id("model:city_euro_ancient"),
            },
        );
        t.insert_sv(
            "ART_DEF_STRATEGIC_VIEW_WARRIOR",
            SvArt {
                icon: id("tex:sv_warrior"),
            },
        );
        t.insert_player_color(
            "PLAYERCOLOR_RED",
            PlayerColor {
                primary: [255, 0, 0, 255],
                secondary: [0, 0, 0, 255],
                text: [255, 255, 255, 255],
            },
        );
        t.insert_formation(
            "Infantry",
            Formation {
                spacing_x: 1.0,
                spacing_y: 2.0,
                total_width: 8.0,
                unit_randomness: 0.3,
                stagger: 0.5,
                extra: BTreeMap::from([("Foo".to_owned(), 7.0)]),
            },
        );
        t
    }

    #[test]
    fn roundtrip_every_record_kind() {
        let t = full_table();
        assert_eq!(t.len(), 8);
        let back = ArtTable::decode(&t.encode()).unwrap();
        assert_eq!(back.len(), 8);
        // Variants come back sorted by suffix; everything else is equal.
        let u = back.unit("UNIT_WARRIOR").unwrap();
        assert_eq!(u.variants[0].suffix, "_AFRI");
        assert_eq!(
            u.for_style(Some("_EURO")).art_info,
            "ART_DEF_UNIT_WARRIOR_EURO"
        );
        assert_eq!(u.for_style(Some("_ASIA")).art_info, "ART_DEF_UNIT_WARRIOR");
        assert_eq!(u.for_style(None), &u.base);
        assert_eq!(
            back.terrain("TERRAIN_GRASS", 2),
            t.terrain("TERRAIN_GRASS", 2)
        );
        assert!(back.terrain("TERRAIN_GRASS", 3).is_none());
        assert_eq!(
            back.feature("FEATURE_FOREST", 0),
            t.feature("FEATURE_FOREST", 0)
        );
        assert_eq!(back.river(), t.river());
        assert_eq!(back.city("EURO", "Ancient"), t.city("EURO", "Ancient"));
        assert_eq!(
            back.sv("ART_DEF_STRATEGIC_VIEW_WARRIOR"),
            t.sv("ART_DEF_STRATEGIC_VIEW_WARRIOR")
        );
        assert_eq!(
            back.player_color("PLAYERCOLOR_RED"),
            t.player_color("PLAYERCOLOR_RED")
        );
        assert_eq!(back.formation("Infantry"), t.formation("Infantry"));
        // Re-encoding the decoded table is stable.
        assert_eq!(back.encode(), t.encode());
        assert_eq!(
            ArtTable::decode(&ArtTable::new().encode()).unwrap(),
            ArtTable::new()
        );
    }

    #[test]
    fn encoding_is_deterministic_and_ignores_insert_order() {
        let a = full_table();
        let mut b = ArtTable::new();
        for (k, r) in a.iter().collect::<Vec<_>>().into_iter().rev() {
            b.entries.insert(k.to_owned(), r.clone());
        }
        assert_eq!(a.encode(), a.encode());
        assert_eq!(a.encode(), b.encode());
    }

    #[test]
    fn header_is_magic_then_version() {
        let bytes = full_table().encode();
        assert_eq!(&bytes[..4], b"C5AT");
        // A table of B1 records stays version 1: the B1 sets keep their hashes.
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 1);
    }

    #[test]
    fn rejects_wrong_magic_and_version() {
        let bytes = full_table().encode();
        let mut bad = bytes.clone();
        bad[..4].copy_from_slice(b"C5MF");
        assert!(matches!(
            ArtTable::decode(&bad),
            Err(FormatError::BadMagic { .. })
        ));
        let mut bad = bytes.clone();
        bad[4] = 0xff;
        let err = ArtTable::decode(&bad).unwrap_err();
        assert!(matches!(
            err,
            FormatError::UnsupportedVersion { found: 0xff, .. }
        ));
        assert!(err.to_string().contains("unsupported format version"));
        assert!(ArtTable::decode(&bytes[..bytes.len() - 3]).is_err());
        assert!(matches!(
            ArtTable::decode(&bytes[..3]),
            Err(FormatError::Truncated)
        ));
    }

    #[test]
    fn rejects_key_not_matching_record_and_duplicates() {
        let sv = ArtRecord::Sv(SvArt { icon: id("x") });
        let bad = blob::encode_v(
            ART_TABLE_MAGIC,
            ArtTable::VERSION,
            &vec![("unit/UNIT_X".to_owned(), sv.clone())],
        );
        assert!(matches!(
            ArtTable::decode(&bad),
            Err(FormatError::Invalid(_))
        ));
        let dup = blob::encode_v(
            ART_TABLE_MAGIC,
            ArtTable::VERSION,
            &vec![("sv/A".to_owned(), sv.clone()), ("sv/A".to_owned(), sv)],
        );
        assert!(matches!(
            ArtTable::decode(&dup),
            Err(FormatError::Invalid(_))
        ));
        let terrain = ArtRecord::Terrain(TerrainArt {
            texture: id("x"),
            texture_scale: 1.0,
            projection: Projection::MeshUv,
        });
        let bad_continent = blob::encode_v(
            ART_TABLE_MAGIC,
            ArtTable::VERSION,
            &vec![("terrain/TERRAIN_GRASS/9".to_owned(), terrain)],
        );
        assert!(matches!(
            ArtTable::decode(&bad_continent),
            Err(FormatError::Invalid(_))
        ));
    }

    fn figure(member: &str, model: &str) -> FigureB2 {
        FigureB2 {
            member: member.to_owned(),
            model: id(model),
            count: 2,
            scale: Some(0.14),
            z_offset: None,
            domain: Some("Sea".to_owned()),
        }
    }

    fn unit_b2() -> UnitRecordB2 {
        let art = |tag: &str| UnitArtB2 {
            art_info: format!("ART_DEF_UNIT_{tag}"),
            figures: vec![figure(&format!("MEMBER_{tag}"), &format!("md:{tag}"))],
            damage_states: Some(3),
            formation: Some("Infantry".to_owned()),
        };
        UnitRecordB2 {
            base: art("WORKER"),
            style_variants: vec![
                UnitVariantB2 {
                    suffix: "_EURO".to_owned(),
                    art: art("WORKER_EURO"),
                },
                UnitVariantB2 {
                    suffix: "_AFRI".to_owned(),
                    art: art("WORKER_AFRI"),
                },
            ],
            era_variation: true,
            era_variants: vec![UnitVariantB2 {
                suffix: "_LATE".to_owned(),
                art: art("WORKER_LATE"),
            }],
        }
    }

    fn vis_effect() -> VisEffectArt {
        VisEffectArt {
            name: "ART_DEF_VEFFECT_X".to_owned(),
            translation: [1.0, 2.0, 3.0],
            rotation: [0.0, 90.0, 90.0],
            scale: 0.22,
            base_duration: 0.1,
            base_duration_scale: 1.0,
            movement_speed: Some(3.0),
            fow_mask: None,
            particles: vec!["ART_DEF_PEFFECT_A".to_owned()],
            models: vec![("ART_DEF_MEFFECT_B".to_owned(), Some(1000))],
            children: vec![("ProjectileEnd".to_owned(), "ART_DEF_VEFFECT_Y".to_owned())],
            sounds: vec![],
        }
    }

    #[test]
    fn b2_records_roundtrip_in_a_version_2_table() {
        let mut t = full_table();
        t.insert_unit_b2("UNIT_WORKER", unit_b2());
        t.insert_vis_effect(vis_effect());
        let bytes = t.encode();
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 2);
        let back = ArtTable::decode(&bytes).unwrap();
        assert_eq!(back.vis_effect("ART_DEF_VEFFECT_X"), Some(&vis_effect()));
        let u = back.unit_b2("UNIT_WORKER").unwrap();
        // Variants come back sorted; the rest is as written.
        assert_eq!(u.style_variants[0].suffix, "_AFRI");
        assert_eq!(
            u.for_style(Some("_EURO")).art_info,
            "ART_DEF_UNIT_WORKER_EURO"
        );
        assert_eq!(u.for_style(Some("_ASIA")).art_info, "ART_DEF_UNIT_WORKER");
        assert!(u.era_variation);
        assert_eq!(u.era_variants[0].suffix, "_LATE");
        assert_eq!(u.base.figures[0].z_offset, None);
        assert_eq!(u.base.figures[0].scale, Some(0.14));
        // B1 records are untouched next to them, and the B1 getter does not see a B2 unit.
        assert_eq!(
            back.terrain("TERRAIN_GRASS", 2),
            t.terrain("TERRAIN_GRASS", 2)
        );
        assert!(back.unit("UNIT_WORKER").is_none());
        assert!(back.unit("UNIT_WARRIOR").is_some());
        assert_eq!(back.encode(), bytes);
    }

    #[test]
    fn version_1_tables_are_read_and_may_not_hold_b2_records() {
        // A version 1 payload (B1 records only) reads as before.
        let t = full_table();
        let v1 = t.encode();
        assert_eq!(u16::from_le_bytes([v1[4], v1[5]]), 1);
        assert_eq!(ArtTable::decode(&v1).unwrap().len(), t.len());
        // The same bytes under a B2 record with the version byte 1 are refused.
        let rec = ArtRecord::VisEffect(vis_effect());
        let bad = blob::encode_v(
            ART_TABLE_MAGIC,
            1,
            &vec![("vis_effect/ART_DEF_VEFFECT_X".to_owned(), rec.clone())],
        );
        assert!(matches!(
            ArtTable::decode(&bad),
            Err(FormatError::Invalid(_))
        ));
        // A key that does not fit the record is refused as for the older records.
        let wrong = blob::encode_v(
            ART_TABLE_MAGIC,
            2,
            &vec![("unit/A/B".to_owned(), ArtRecord::UnitB2(unit_b2()))],
        );
        assert!(ArtTable::decode(&wrong).is_err());
        let wrong = blob::encode_v(ART_TABLE_MAGIC, 2, &vec![("sv/A".to_owned(), rec)]);
        assert!(ArtTable::decode(&wrong).is_err());
        // Version 3 is unknown.
        let mut v3 = v1;
        v3[4] = 3;
        assert!(matches!(
            ArtTable::decode(&v3),
            Err(FormatError::UnsupportedVersion { .. })
        ));
    }

    fn world_table() -> ArtTable {
        let mut t = ArtTable::new();
        t.insert_landmark(
            0,
            LandmarkArt {
                improvement: "ART_DEF_IMPROVEMENT_FARM".into(),
                resource: "ART_DEF_RESOURCE_ALL".into(),
                era: "Any".into(),
                state: "Constructed".into(),
                layout: "FARM".into(),
                scale: 1.0,
                terrain_contour: 1,
                tech: Some("TECH_ECONOMICS".into()),
                model: Some(id("md:farm")),
            },
        );
        t.insert_landmark(
            1,
            LandmarkArt {
                improvement: "ART_DEF_IMPROVEMENT_FARM".into(),
                resource: "ART_DEF_RESOURCE_ALL".into(),
                era: "Ancient".into(),
                state: "Any".into(),
                layout: "FARM".into(),
                scale: 0.5,
                terrain_contour: 0,
                tech: None,
                model: None,
            },
        );
        t.insert_building(BuildingArt {
            art_tag: "THE COLOSSUS".into(),
            building_types: vec!["BUILDING_COLOSSUS".into()],
            cultural_variation: false,
            era_variation: false,
            random_variation: false,
            model: Some(id("md:colossus")),
            scale: 0.05,
            variants: vec![BuildingVariant {
                kind: "culture".into(),
                key: "AFRICAN".into(),
                art_type: "AFRICAN CASTLE".into(),
                model: id("md:castle_afri"),
                scale: 0.1,
            }],
            note: None,
        });
        t.insert_building_type(BuildingTypeArt {
            building_type: "BUILDING_COLOSSUS".into(),
            art_tag: "THE COLOSSUS".into(),
            display_position: 8,
        });
        t.insert_wonder(WonderArt {
            art_type: "THE COLOSSUS".into(),
            states: vec![WonderState {
                state: "Any".into(),
                scale: 0.05,
                model: id("md:colossus"),
            }],
        });
        t.insert_city_b2(
            "EUROPEAN",
            "Ancient",
            CityArtB2 {
                model: id("md:euro_anc_city"),
                parts: 40,
                lighting: None,
            },
        );
        t.insert_wall(
            "EUROPEAN",
            "Ancient",
            WallArt {
                model: id("md:euro_walls"),
                parts: 3,
            },
        );
        t.insert_route(RouteArt {
            route: "ROUTE_ROAD".into(),
            models: vec![RouteModel {
                class: "road".into(),
                model: id("md:roadsandrails"),
            }],
        });
        t.insert_overlay(OverlayArt {
            name: "CenterOneTurn".into(),
            kind: "path_center".into(),
            variant: "OneTurn".into(),
            model: Some(id("md:centeroneturn")),
            textures: vec![],
        });
        t.insert_tree(TreeArt {
            feature: "FEATURE_FOREST".into(),
            style: 4,
            model: id("md:forest_europe"),
            overlay: id("tx:forest_overlay_europe.dds"),
            masks: vec![id("tx:forest_tile1.dds")],
        });
        t.insert_civ(CivArt {
            civ: "CIVILIZATION_ROME".into(),
            art_define_tag: Some("ART_DEF_CIVILIZATION_ROME".into()),
            art_style: Some("ARTSTYLE_GRECO_ROMAN".into()),
            prefix: Some("MEDITERRANEAN".into()),
            suffix: Some("_MED".into()),
            default_color: None,
            style_culture: None,
        });
        t
    }

    #[test]
    fn world_records_roundtrip_in_a_version_2_table_and_list_their_models() {
        let t = world_table();
        let bytes = t.encode();
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 2);
        let back = ArtTable::decode(&bytes).unwrap();
        assert_eq!(back.encode(), bytes);
        assert_eq!(back.landmarks().count(), 2);
        assert_eq!(
            back.landmark("ART_DEF_IMPROVEMENT_FARM", "ART_DEF_RESOURCE_ALL", 1)
                .unwrap()
                .model,
            None
        );
        assert_eq!(back.building("THE COLOSSUS").unwrap().variants.len(), 1);
        assert_eq!(back.wonder("THE COLOSSUS").unwrap().states[0].scale, 0.05);
        assert_eq!(back.city_b2("EUROPEAN", "Ancient").unwrap().parts, 40);
        assert!(
            back.city("EUROPEAN", "Ancient").is_none(),
            "a B2 city is not a B1 city"
        );
        assert_eq!(back.wall("EUROPEAN", "Ancient").unwrap().parts, 3);
        assert_eq!(back.route("ROUTE_ROAD").unwrap().models[0].class, "road");
        assert_eq!(back.overlay("CenterOneTurn").unwrap().kind, "path_center");
        assert_eq!(back.tree("FEATURE_FOREST", 4).unwrap().masks.len(), 1);
        assert!(back.tree("FEATURE_FOREST", 5).is_none());
        assert_eq!(
            back.civ("CIVILIZATION_ROME").unwrap().suffix.as_deref(),
            Some("_MED")
        );
        assert!(back.has_building_types());
        assert_eq!(
            back.building_type("BUILDING_COLOSSUS")
                .unwrap()
                .display_position,
            8
        );
        assert!(back.building_type("BUILDING_HARBOR").is_none());
        assert!(!ArtTable::new().has_building_types());
        let kinds: Vec<&str> = back.models().iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            [
                "building", "building", "city", "landmark", "overlay", "route", "tree", "wall",
                "wonder"
            ]
        );
    }

    #[test]
    fn world_records_refuse_a_key_of_another_shape_and_a_version_1_table() {
        let rec = ArtRecord::Wonder(WonderArt {
            art_type: "X".into(),
            states: vec![],
        });
        for bad in [
            "wonder",
            "wonder/A/B",
            "landmark/X",
            "tree/FEATURE_FOREST/9",
        ] {
            let r = if bad.starts_with("landmark") {
                ArtRecord::Landmark(LandmarkArt {
                    improvement: "a".into(),
                    resource: "b".into(),
                    era: "Any".into(),
                    state: "Any".into(),
                    layout: "RANDOM".into(),
                    scale: 1.0,
                    terrain_contour: 0,
                    tech: None,
                    model: None,
                })
            } else if bad.starts_with("tree") {
                ArtRecord::Tree(TreeArt {
                    feature: "f".into(),
                    style: 9,
                    model: id("md:x"),
                    overlay: id("tx:x"),
                    masks: vec![],
                })
            } else {
                rec.clone()
            };
            let blob = blob::encode_v(ART_TABLE_MAGIC, 2, &vec![(bad.to_owned(), r)]);
            assert!(ArtTable::decode(&blob).is_err(), "{bad}");
        }
        let v1 = blob::encode_v(ART_TABLE_MAGIC, 1, &vec![("wonder/X".to_owned(), rec)]);
        assert!(ArtTable::decode(&v1).is_err());
    }
}
