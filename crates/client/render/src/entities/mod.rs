//! entity-render: units and cities on the map through the static
//! pipeline (`BatchSet::Entities`), and the `WorldLabel`s the UI draws flags and banners at.
//!
//! Everything here is CPU only: [`build_plan`] turns an [`EntitySnapshot`] and the terrain into
//! instance lists per model; `Renderer` uploads them. `render` does not know `protocol` (R6), so
//! `client-app/world_bridge.rs` converts a `WorldView` into the snapshot.
//!
//! Scale: the static unit models are in Civ5 units and do not bake the member scale; the world is
//! in Civ5 units too (B5, `ENGINE_UNITS_PER_CIV5_UNIT = 1`), so an instance is scaled by the
//! figure's `scale` times [`GLOBAL_UNIT_SCALE`] and nothing else.

use std::collections::{BTreeMap, BTreeSet};

use asset_format::{ArtTable, LogicalId};
use glam::Vec3;

use crate::static_model::StaticInstance;
use crate::terrain::{Fog, TerrainSnapshot};

pub mod building_family;
pub mod cities;
pub mod formation;
pub mod highlight;
pub mod icons;
pub mod labels;
pub mod lines;
pub mod marker;
pub mod roads;
pub mod unit_family;
pub mod units;

pub use highlight::{Highlights, PathStep};
pub use labels::{
    CityBanner, LabelData, PathTurn, ScreenLabel, UnitFlag, WorldLabel, project_label,
    project_wrapped_label,
};
pub use roads::RouteKind;

/// Global scale of unit models.
// re: `cvUnitVisSystem::m_uiGlobalUnitScale` (+0x13ab0), default 100 -> 1.0, applied to the root
// node (`cvUnitVisSystem.cpp:85`, RV-12)
pub const GLOBAL_UNIT_SCALE: f32 = 1.0;

/// Direction the figures face: towards the camera (south, +Z).
pub const FACING_Y: f32 = 0.0;

/// Cultures of the art table (`Civilizations.ArtStyleSuffix` / `ArtStylePrefix`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Culture {
    Euro,
    Afri,
    Amer,
    Asia,
    Med,
}

impl Culture {
    /// Key suffix of the unit variants.
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Euro => "_EURO",
            Self::Afri => "_AFRI",
            Self::Amer => "_AMER",
            Self::Asia => "_ASIA",
            Self::Med => "_MED",
        }
    }

    /// Key of the city models (`city/<prefix>/<era>`).
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Euro => "EUROPEAN",
            Self::Afri => "AFRICAN",
            Self::Amer => "AMERICAN",
            Self::Asia => "ASIAN",
            Self::Med => "MEDITERRANEAN",
        }
    }

    /// Reads a catalog `art_style`, whether it holds the suffix (`_EURO`, `EURO`), the prefix
    /// (`EUROPEAN`) or the `ArtStyleType` the core really sends (`ARTSTYLE_ASIAN`); `None` for
    /// anything else, including empty.
    // SIMPLIFIED(SIMP-021): the core's catalog carries `Civilizations.ArtStyleType`, not the
    // `ArtStylePrefix` the city catalogs are keyed by; the type stands for the prefix 95 times in 100
    // (34 of 35 European, 26 of 28 Middle East = African; the rest differ civilization by civilization).
    // Polynesia has the prefix AMERICAN in the database.
    pub fn parse(style: &str) -> Option<Self> {
        let s = style.trim().trim_start_matches('_').to_ascii_uppercase();
        if let Some(kind) = s.strip_prefix("ARTSTYLE_") {
            return match kind {
                "EUROPEAN" | "BARBARIAN" => Some(Self::Euro),
                "ASIAN" => Some(Self::Asia),
                "MIDDLE_EAST" => Some(Self::Afri),
                "GRECO_ROMAN" => Some(Self::Med),
                "SOUTH_AMERICA" | "POLYNESIAN" => Some(Self::Amer),
                _ => None,
            };
        }
        [Self::Euro, Self::Afri, Self::Amer, Self::Asia, Self::Med]
            .into_iter()
            .find(|c| s == c.suffix().trim_start_matches('_') || s == c.prefix())
    }
}

/// Era bucket of the city models (`Ancient`, `Rennaisance` (the game files' spelling), `Industrial`
/// and `Modern`: the four `<Era>` elements of `Civ5ArtDefines_CityBuildings.xml`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EraBucket {
    #[default]
    Ancient,
    Renaissance,
    Industrial,
    Modern,
}

impl EraBucket {
    pub fn key(self) -> &'static str {
        match self {
            Self::Ancient => "Ancient",
            Self::Renaissance => "Rennaisance",
            Self::Industrial => "Industrial",
            Self::Modern => "Modern",
        }
    }

    /// The bucket B1's city models know (they stop at the Renaissance).
    pub fn b1(self) -> Self {
        match self {
            Self::Ancient => Self::Ancient,
            _ => Self::Renaissance,
        }
    }

    /// The word of the era variants of the building art (`ANCIENT HARBOR`, `MEDIEVAL HARBOR`).
    pub fn building_key(self) -> &'static str {
        match self {
            Self::Ancient => "ANCIENT",
            Self::Renaissance => "MEDIEVAL",
            Self::Industrial => "INDUSTRIAL",
            Self::Modern => "MODERN",
        }
    }
}

/// What the owner of an entity looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityPlayer {
    pub id: i8,
    /// sRGB primary colour.
    pub color: [u8; 3],
    /// sRGB colour of the units' cloth (`TeamColorR`): the player's secondary colour.
    pub team_color: [u8; 3],
    pub culture: Option<Culture>,
    /// The owner's era; `None` when unknown (then `EntitySnapshot::era` is used).
    pub era: Option<EraBucket>,
    /// WOB4: the owner's era is the Industrial or a later one (the era of the landmark rows).
    pub industrial: bool,
    /// WOB4: bit `i` is "has the technology `i` of the landmark art list" (`PlayerView.art_techs`).
    pub art_techs: u64,
    /// The owner's era as an index of `Eras` (0 Ancient .. 7 Future); `None` when unknown. The boat
    /// of an embarked unit follows it.
    pub era_index: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitEntity {
    pub id: i32,
    pub owner: i8,
    /// `UNIT_*` type name (art table key `unit/<type>`).
    pub type_name: String,
    pub x: i32,
    pub y: i32,
    pub hp: i32,
    pub max_hp: i32,
    pub civilian: bool,
    pub needs_orders: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CityEntity {
    pub id: i32,
    pub owner: i8,
    pub x: i32,
    pub y: i32,
    pub name: String,
    pub population: i32,
    pub capital: bool,
    pub hp: i32,
    pub max_hp: i32,
    /// WOB5: the art tags (`Buildings.ArtDefineTag`) of the buildings with a model in the world
    /// (`CityView.buildings`): wonders, walls, castles, harbors... Sorted, no duplicates.
    pub buildings: Vec<String>,
    /// The `Buildings.Type` of the same buildings, in the order of their ids (re-b17 5.1: the
    /// `DisplayPosition` of each type decides whether it has a model in the city).
    pub building_types: Vec<String>,
    /// The `Buildings.Type` the city is building, when it is a building (re-b17 5.3: a wonder in
    /// the making stands half built).
    pub production_building: Option<String>,
    /// WOB5: the city is being razed (it looks the same until it is gone, re-b17 4.2).
    pub razing: bool,
    /// WOB5: the player who founded the city (`CityView.founder`); the style follows the owner until
    /// RW-18 says otherwise.
    pub founder: i8,
}

/// What lies on one plot besides the terrain: resource, improvement, route. Only plots
/// with something on them are listed; the art defines are the catalog's `ArtDefineTag`s, the keys
/// of the art table's `sv/<tag>` records.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlotDecor {
    pub x: i32,
    pub y: i32,
    /// Art define of the resource the human knows of.
    pub resource: Option<String>,
    /// Art define of the (last seen) improvement.
    pub improvement: Option<String>,
    /// The improvement is a barbarian encampment or ruins: drawn in the middle of the cell.
    pub special: bool,
    pub pillaged: bool,
    pub route: Option<RouteKind>,
    /// WOB4: art define of the improvement being built here (`PlotView.building`); the landmark of
    /// the state `UnderConstruction`. `improvement` is the finished one, if any.
    pub building: Option<String>,
    /// WOB4: art define of a feature that has a model of its own (`ART_DEF_FEATURE_*`).
    pub feature: Option<String>,
    /// WOB4: the owner of the plot; its era and technologies choose the landmark (`None` unowned).
    pub owner: Option<i8>,
    /// WOB4: a city works the plot (clip 101 of a feature).
    pub worked: bool,
    /// WOB4: which model of the resource the plot asks for.
    pub variant: ResourceVariant,
    /// WOB6: the route (road, railroad) of the plot is pillaged (`PlotFlags::ROUTE_PILLAGED`).
    pub route_pillaged: bool,
    /// WOB6: six edge bits, bit `i` is a river on the edge towards `Dir` `i`; a road that crosses
    /// one has a bridge.
    pub rivers: u8,
}

/// The model of a resource that suits its plot: the art defines carry it in a suffix
/// (`ART_DEF_RESOURCE_IRON_HEAVY_MARSH`, `..._FURS_HILL`, `..._OIL_WATER`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResourceVariant {
    /// A large deposit (`_HEAVY`).
    pub heavy: bool,
    /// On a marsh (`_MARSH`).
    pub marsh: bool,
    /// On a hill (`_HILL`).
    pub hill: bool,
    /// At sea (`_WATER`).
    pub water: bool,
}

/// Units, cities and what lies on the plots, for the renderer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntitySnapshot {
    pub players: Vec<EntityPlayer>,
    pub units: Vec<UnitEntity>,
    pub cities: Vec<CityEntity>,
    pub era: EraBucket,
    pub plots: Vec<PlotDecor>,
}

impl EntitySnapshot {
    pub fn player(&self, id: i8) -> Option<&EntityPlayer> {
        self.players.iter().find(|p| p.id == id)
    }

    pub fn culture_of(&self, owner: i8) -> Option<Culture> {
        self.player(owner).and_then(|p| p.culture)
    }

    /// Era of an owner's cities: the owner's own, else the snapshot's.
    pub fn era_of(&self, owner: i8) -> EraBucket {
        self.player(owner).and_then(|p| p.era).unwrap_or(self.era)
    }

    pub fn color_of(&self, owner: i8) -> [u8; 3] {
        self.player(owner).map_or([160; 3], |p| p.color)
    }

    /// The team colour of an owner's units (see [`EntityPlayer::team_color`]).
    pub fn team_color_of(&self, owner: i8) -> [u8; 3] {
        self.player(owner).map_or([160; 3], |p| p.team_color)
    }

    /// The owners of the snapshot: what units and cities need besides their own record.
    pub fn owners(&self) -> Owners {
        Owners {
            players: self.players.clone(),
            era: self.era,
        }
    }
}

/// What the owners of the entities look like: the part of an [`EntitySnapshot`] that is neither a
/// unit, a city nor a plot (WOB0: the keyed inputs take it on its own).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Owners {
    pub players: Vec<EntityPlayer>,
    /// The era of the top bar, for an owner whose own era is unknown.
    pub era: EraBucket,
}

impl Owners {
    pub fn player(&self, id: i8) -> Option<&EntityPlayer> {
        self.players.iter().find(|p| p.id == id)
    }

    pub fn culture_of(&self, owner: i8) -> Option<Culture> {
        self.player(owner).and_then(|p| p.culture)
    }

    /// Era of an owner's cities: the owner's own, else the top bar's.
    pub fn era_of(&self, owner: i8) -> EraBucket {
        self.player(owner).and_then(|p| p.era).unwrap_or(self.era)
    }

    /// The `Eras` index of an owner (see [`EntityPlayer::era_index`]).
    pub fn era_index_of(&self, owner: i8) -> Option<u8> {
        self.player(owner).and_then(|p| p.era_index)
    }

    pub fn color_of(&self, owner: i8) -> [u8; 3] {
        self.player(owner).map_or([160; 3], |p| p.color)
    }

    /// The team colour of an owner's units (see [`EntityPlayer::team_color`]).
    pub fn team_color_of(&self, owner: i8) -> [u8; 3] {
        self.player(owner).map_or([160; 3], |p| p.team_color)
    }
}

/// Key of a plot in the keyed inputs: `y << 16 | x`, so keys sort in the row-major order the
/// snapshot lists plots in, whatever the width of the map (the renderer does not know it here).
pub type PlotKey = u32;

/// The key of the plot `(x, y)`.
pub fn plot_key(x: i32, y: i32) -> PlotKey {
    (y as u32) << 16 | (x as u32 & 0xFFFF)
}

/// The key of a unit: owner and id (`UnitKey` of `protocol`).
pub type UnitKey = (i8, i32);
/// The key of a city: owner and id (`CityKey` of `protocol`).
pub type CityKey = (i8, i32);

/// State of the art table.
#[derive(Debug, Clone, Default)]
pub enum ArtState {
    /// Not arrived yet: nothing is drawn (no marker flicker).
    #[default]
    Loading,
    /// No table (no assets): every entity is a marker.
    Unavailable,
    Ready(ArtTable),
}

/// State of one model of the art.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelStatus {
    /// Asked for, not here yet: the entity is not drawn for now.
    Pending,
    /// Will not come: the entity is drawn as a marker (SIMP-011).
    Failed,
    Ready,
}

/// Which model a batch of instances uses.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModelRef {
    Asset(LogicalId),
    UnitMarker,
    CityMarker,
}

/// Result of [`build_plan`].
#[derive(Debug, Clone, Default)]
pub struct EntityPlan {
    pub batches: BTreeMap<ModelRef, Vec<StaticInstance>>,
    pub labels: Vec<WorldLabel>,
    /// Models the plan needs that are still [`ModelStatus::Pending`].
    pub wanted: BTreeSet<LogicalId>,
    /// The models that are figures of units: the ported `UnitShader` draws these.
    pub unit_models: BTreeSet<LogicalId>,
    /// The composed city models: the ported `BuildingShader` draws these.
    pub city_models: BTreeSet<LogicalId>,
}

impl EntityPlan {
    pub fn instance_count(&self) -> usize {
        self.batches.values().map(Vec::len).sum()
    }

    pub(crate) fn push(&mut self, model: ModelRef, instance: StaticInstance) {
        self.batches.entry(model).or_default().push(instance);
    }
}

/// Everything [`build_plan`] reads besides the snapshot.
pub struct PlanContext<'a> {
    pub terrain: &'a TerrainSnapshot,
    pub art: &'a ArtState,
    pub status: &'a dyn Fn(&LogicalId) -> ModelStatus,
}

/// Instances and labels of `snapshot` on `terrain`. Units are drawn on visible cells only, cities
/// on revealed ones (the shader darkens the hidden ones).
pub fn build_plan(snapshot: &EntitySnapshot, ctx: &PlanContext<'_>) -> EntityPlan {
    let mut plan = EntityPlan::default();
    units::place_units(snapshot, ctx, &mut plan);
    cities::place_cities(snapshot, ctx, &mut plan);
    plan
}

/// sRGB byte colour as a linear tint (the surface does the encoding).
pub fn srgb_tint(c: [u8; 3]) -> [f32; 3] {
    c.map(|v| {
        let s = f32::from(v) / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    })
}

/// Cell index and fog state of cell `(x, y)`; `None` off the map.
pub(crate) fn fog_of(terrain: &TerrainSnapshot, x: i32, y: i32) -> Option<(usize, Fog)> {
    let idx = terrain.index(x, y)?;
    Some((idx, terrain.cells()[idx].fog))
}

pub(crate) fn instance(position: Vec3, scale: f32, cell: usize) -> StaticInstance {
    let mut i = StaticInstance::new(position.to_array(), FACING_Y, scale);
    i.cell = cell as u32;
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cultures_parse_from_suffix_or_prefix() {
        assert_eq!(Culture::parse("_EURO"), Some(Culture::Euro));
        assert_eq!(Culture::parse("asia"), Some(Culture::Asia));
        assert_eq!(Culture::parse("MEDITERRANEAN"), Some(Culture::Med));
        assert_eq!(Culture::parse("_AFRI").unwrap().prefix(), "AFRICAN");
        assert_eq!(Culture::parse(""), None);
        assert_eq!(Culture::parse("_XXXX"), None);
    }

    #[test]
    fn the_art_style_type_of_the_core_is_a_culture() {
        assert_eq!(Culture::parse("ARTSTYLE_ASIAN"), Some(Culture::Asia));
        assert_eq!(Culture::parse("ARTSTYLE_EUROPEAN"), Some(Culture::Euro));
        assert_eq!(Culture::parse("ARTSTYLE_MIDDLE_EAST"), Some(Culture::Afri));
        assert_eq!(Culture::parse("ARTSTYLE_GRECO_ROMAN"), Some(Culture::Med));
        assert_eq!(
            Culture::parse("ARTSTYLE_SOUTH_AMERICA"),
            Some(Culture::Amer)
        );
        assert_eq!(Culture::parse("ARTSTYLE_NOPE"), None);
    }

    #[test]
    fn tint_is_linear() {
        assert_eq!(srgb_tint([0, 0, 0]), [0.0; 3]);
        let t = srgb_tint([255, 128, 0]);
        assert!((t[0] - 1.0).abs() < 1e-5 && t[1] > 0.2 && t[1] < 0.25 && t[2] == 0.0);
    }
}
