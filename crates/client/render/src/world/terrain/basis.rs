//! The basis of the map: the static relief of ALL hexes, fog or not,
//! as the client has it from `MapBasis` (net-protocol NP-D19) and the `basis` section of the
//! diffs. It is the render's own type (R6: `render` does not know `protocol`);
//! `client-app/bridge/terrain.rs` converts to it by the names of the catalog.

use crate::terrain::{FeatureKind, Relief, TerrainKind, TerrainSnapshot};

/// The natural wonders that have a piece (`Wonder_*`) in the install.
// re: assets inventory 2026-10-21 (14 pieces `Wonder_*` per style)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NwPiece {
    Crater,
    Fuji,
    Geyser,
    Gibraltar,
    Kilimanjaro,
    Krakatoa,
    LakeVictoria,
    Mesa,
    MtKailash,
    MtSinai,
    Potosi,
    Reef,
    SriPada,
    Uluru,
}

impl NwPiece {
    pub const ALL: [NwPiece; 14] = [
        Self::Crater,
        Self::Fuji,
        Self::Geyser,
        Self::Gibraltar,
        Self::Kilimanjaro,
        Self::Krakatoa,
        Self::LakeVictoria,
        Self::Mesa,
        Self::MtKailash,
        Self::MtSinai,
        Self::Potosi,
        Self::Reef,
        Self::SriPada,
        Self::Uluru,
    ];

    /// The name of the piece kind: `Wonder_Crater`.
    pub fn piece_name(self) -> &'static str {
        match self {
            Self::Crater => "Wonder_Crater",
            Self::Fuji => "Wonder_Fuji",
            Self::Geyser => "Wonder_Geyser",
            Self::Gibraltar => "Wonder_Gibraltar",
            Self::Kilimanjaro => "Wonder_Kilimanjaro",
            Self::Krakatoa => "Wonder_Krakatoa",
            Self::LakeVictoria => "Wonder_Lake_Victoria",
            Self::Mesa => "Wonder_Mesa",
            Self::MtKailash => "Wonder_Mt_Kailash",
            Self::MtSinai => "Wonder_Mt_Sinai",
            Self::Potosi => "Wonder_Potosi",
            Self::Reef => "Wonder_Reef",
            Self::SriPada => "Wonder_Sri_Pada",
            Self::Uluru => "Wonder_Uluru",
        }
    }

    /// The wonder of a catalog `FEATURE_*` type, if it has a piece.
    // TODO(RW-5): the catalog names are from memory of the rules of the game; the bridge of the
    // original (`StartInitTerrainFromGameplayMap`) says which feature takes which piece.
    pub fn from_type_name(name: &str) -> Option<Self> {
        Some(match name {
            "FEATURE_CRATER" => Self::Crater,
            "FEATURE_FUJI" => Self::Fuji,
            "FEATURE_GEYSER" => Self::Geyser,
            "FEATURE_GIBRALTAR" => Self::Gibraltar,
            "FEATURE_KILIMANJARO" => Self::Kilimanjaro,
            "FEATURE_VOLCANO" => Self::Krakatoa,
            "FEATURE_LAKE_VICTORIA" => Self::LakeVictoria,
            "FEATURE_MESA" => Self::Mesa,
            "FEATURE_MT_KAILASH" => Self::MtKailash,
            "FEATURE_MT_SINAI" => Self::MtSinai,
            "FEATURE_POTOSI" => Self::Potosi,
            "FEATURE_REEF" => Self::Reef,
            "FEATURE_SRI_PADA" => Self::SriPada,
            "FEATURE_ULURU" => Self::Uluru,
            _ => return None,
        })
    }
}

/// The feature of a hex as far as the relief cares.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BasisFeature {
    #[default]
    None,
    Marsh,
    FloodPlains,
    Ice,
    NaturalWonder(NwPiece),
    /// El Dorado and the Fountain of Youth: natural wonders with no piece of their own, the cell
    /// keeps the class of its terrain.
    // re: StartInitTerrainFromGameplayMap, NaturalWonderFlags (LoadGameState.cpp:77-97), RE-B12 item 1
    WonderKeepsTerrain,
    /// Solomon's Mines: its cell takes the desert class (`0x4`).
    WonderDesert,
    /// A feature with no say in the relief (forest, jungle, oasis, fallout, a wonder with no
    /// piece).
    Other,
}

impl BasisFeature {
    /// The feature of a catalog `FEATURE_*` type name.
    pub fn from_type_name(name: &str) -> Self {
        match name {
            "FEATURE_MARSH" => Self::Marsh,
            "FEATURE_FLOOD_PLAINS" => Self::FloodPlains,
            "FEATURE_ICE" => Self::Ice,
            "FEATURE_EL_DORADO" | "FEATURE_FOUNTAIN_YOUTH" => Self::WonderKeepsTerrain,
            "FEATURE_SOLOMONS_MINES" => Self::WonderDesert,
            other => NwPiece::from_type_name(other).map_or(Self::Other, Self::NaturalWonder),
        }
    }

    /// The feature of the kind B1's snapshot has.
    pub fn from_kind(kind: FeatureKind) -> Self {
        match kind {
            FeatureKind::None => Self::None,
            FeatureKind::Marsh => Self::Marsh,
            FeatureKind::FloodPlains => Self::FloodPlains,
            FeatureKind::Ice => Self::Ice,
            FeatureKind::Forest
            | FeatureKind::Jungle
            | FeatureKind::Oasis
            | FeatureKind::Fallout
            | FeatureKind::Other => Self::Other,
        }
    }

    pub fn is_natural_wonder(self) -> bool {
        matches!(self, Self::NaturalWonder(_))
    }
}

/// Flags of a hex of the basis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct BasisFlags(pub u8);

impl BasisFlags {
    pub const LAKE: u8 = 1;
    pub const COASTAL: u8 = 2;
    pub const NATURAL_WONDER: u8 = 4;

    pub fn contains(self, bit: u8) -> bool {
        self.0 & bit == bit
    }
}

/// One hex of the basis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PlotBasis {
    pub relief: Relief,
    pub terrain: TerrainKind,
    /// `continent_art` 0..4: Generic, America, Asia, Africa, Europe.
    pub continent: u8,
    pub feature: BasisFeature,
    pub flags: BasisFlags,
    /// Six edge bits, in the order of `DirectionTypes`.
    pub rivers: u8,
    /// Flow on the edges E, SE, SW: `FlowDirectionTypes`, `-1` none.
    pub river_flow: [i8; 3],
}

impl PlotBasis {
    /// A hex with no river flow.
    pub const NO_FLOW: [i8; 3] = [-1; 3];
}

/// The basis of a map: size, wrap, the seed of the build and every hex, `y * width + x`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerrainBasis {
    pub width: u16,
    pub height: u16,
    pub wrap_x: bool,
    pub seed: u32,
    pub plots: Vec<PlotBasis>,
}

impl TerrainBasis {
    /// `None` if the number of hexes is not `width * height`.
    pub fn new(
        width: u16,
        height: u16,
        wrap_x: bool,
        seed: u32,
        plots: Vec<PlotBasis>,
    ) -> Option<Self> {
        (plots.len() == usize::from(width) * usize::from(height)).then_some(Self {
            width,
            height,
            wrap_x,
            seed,
            plots,
        })
    }

    /// A basis of one hex everywhere.
    pub fn uniform(width: u16, height: u16, wrap_x: bool, seed: u32, plot: PlotBasis) -> Self {
        Self {
            width,
            height,
            wrap_x,
            seed,
            plots: vec![plot; usize::from(width) * usize::from(height)],
        }
    }

    /// The basis of B1's snapshot, for the games that have no `MapBasis` (the fixture, the B1
    /// path): what the snapshot knows of the relief. A hex nobody revealed is flat and unknown.
    pub fn from_snapshot(snapshot: &TerrainSnapshot, seed: u32) -> Option<Self> {
        let plots = snapshot
            .cells()
            .iter()
            .map(|c| PlotBasis {
                relief: c.drawn_relief(),
                terrain: c.kind,
                continent: c.art,
                feature: BasisFeature::from_kind(c.feat),
                flags: BasisFlags(0),
                rivers: c.rivers,
                river_flow: PlotBasis::NO_FLOW,
            })
            .collect();
        Self::new(
            u16::try_from(snapshot.width()).ok()?,
            u16::try_from(snapshot.height()).ok()?,
            snapshot.wrap_x(),
            seed,
            plots,
        )
    }

    pub fn index(&self, x: i32, y: i32) -> Option<usize> {
        ((0..i32::from(self.width)).contains(&x) && (0..i32::from(self.height)).contains(&y))
            .then(|| y as usize * usize::from(self.width) + x as usize)
    }

    pub fn plot(&self, x: i32, y: i32) -> Option<&PlotBasis> {
        self.index(x, y).map(|i| &self.plots[i])
    }

    /// `(x, y)` of the hex with this index.
    pub fn xy(&self, index: u32) -> (i32, i32) {
        let w = u32::from(self.width).max(1);
        ((index % w) as i32, (index / w) as i32)
    }

    /// Applies the records of a `basis` diff; returns the indexes whose record changed, sorted.
    /// An index past the map is ignored.
    pub fn patch(&mut self, changes: &[(u32, PlotBasis)]) -> Vec<u32> {
        let mut changed = Vec::new();
        for (i, p) in changes {
            if let Some(slot) = self.plots.get_mut(*i as usize)
                && *slot != *p
            {
                *slot = *p;
                changed.push(*i);
            }
        }
        changed.sort_unstable();
        changed.dedup();
        changed
    }

    /// Whether the map has the same size and wrap as `other` (the field can stay).
    pub fn same_shape(&self, other: &TerrainBasis) -> bool {
        (self.width, self.height, self.wrap_x) == (other.width, other.height, other.wrap_x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat() -> PlotBasis {
        PlotBasis {
            relief: Relief::Flat,
            terrain: TerrainKind::Grass,
            river_flow: PlotBasis::NO_FLOW,
            ..PlotBasis::default()
        }
    }

    #[test]
    fn a_basis_needs_a_record_per_hex() {
        assert!(TerrainBasis::new(3, 2, false, 1, vec![flat(); 6]).is_some());
        assert!(TerrainBasis::new(3, 2, false, 1, vec![flat(); 5]).is_none());
        let b = TerrainBasis::uniform(4, 3, true, 9, flat());
        assert_eq!(b.plots.len(), 12);
        assert_eq!(b.index(3, 2), Some(11));
        assert_eq!(b.index(4, 0), None);
        assert_eq!(b.index(0, -1), None);
        assert_eq!(b.xy(11), (3, 2));
        assert!(b.plot(1, 1).is_some() && b.plot(9, 9).is_none());
    }

    #[test]
    fn catalog_names_give_features_and_wonders() {
        assert_eq!(
            BasisFeature::from_type_name("FEATURE_MARSH"),
            BasisFeature::Marsh
        );
        assert_eq!(
            BasisFeature::from_type_name("FEATURE_FLOOD_PLAINS"),
            BasisFeature::FloodPlains
        );
        assert_eq!(
            BasisFeature::from_type_name("FEATURE_ICE"),
            BasisFeature::Ice
        );
        assert_eq!(
            BasisFeature::from_type_name("FEATURE_FUJI"),
            BasisFeature::NaturalWonder(NwPiece::Fuji)
        );
        assert_eq!(
            BasisFeature::from_type_name("FEATURE_FOREST"),
            BasisFeature::Other
        );
        assert!(BasisFeature::from_type_name("FEATURE_REEF").is_natural_wonder());
        assert!(!BasisFeature::Marsh.is_natural_wonder());
    }

    #[test]
    fn every_wonder_has_a_distinct_piece_name_and_a_catalog_name() {
        let mut names: Vec<&str> = NwPiece::ALL.iter().map(|w| w.piece_name()).collect();
        assert!(names.iter().all(|n| n.starts_with("Wonder_")));
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 14);
        // The catalog names the bridge knows come back as the wonder they stand for.
        assert_eq!(
            NwPiece::from_type_name("FEATURE_VOLCANO"),
            Some(NwPiece::Krakatoa)
        );
        assert_eq!(NwPiece::from_type_name("FEATURE_FOREST"), None);
    }

    #[test]
    fn flags_are_bits() {
        let f = BasisFlags(BasisFlags::LAKE | BasisFlags::NATURAL_WONDER);
        assert!(f.contains(BasisFlags::LAKE) && f.contains(BasisFlags::NATURAL_WONDER));
        assert!(!f.contains(BasisFlags::COASTAL));
    }

    #[test]
    fn a_patch_changes_what_differs_and_says_which() {
        let mut b = TerrainBasis::uniform(3, 3, false, 1, flat());
        let hill = PlotBasis {
            relief: Relief::Hill,
            ..flat()
        };
        // Index 4 changes, index 5 is the same record, index 99 is off the map; 4 twice.
        let changed = b.patch(&[(4, hill), (5, flat()), (99, hill), (4, hill)]);
        assert_eq!(changed, [4]);
        assert_eq!(b.plots[4], hill);
        assert!(b.patch(&[(4, hill)]).is_empty());
    }

    #[test]
    fn a_snapshot_gives_a_basis_of_the_same_size() {
        use crate::terrain::{Fog, TerrainCell, TerrainSnapshot};
        let cells = vec![
            TerrainCell {
                relief: Relief::Hill,
                kind: TerrainKind::Plains,
                art: 4,
                fog: Fog::Visible,
                rivers: 0b101,
                feat: FeatureKind::Marsh,
                ..TerrainCell::default()
            },
            TerrainCell::default(),
            TerrainCell::default(),
            TerrainCell::default(),
        ];
        let snap = TerrainSnapshot::new(2, 2, true, cells).unwrap();
        let b = TerrainBasis::from_snapshot(&snap, 77).unwrap();
        assert_eq!((b.width, b.height, b.wrap_x, b.seed), (2, 2, true, 77));
        let p = b.plots[0];
        assert_eq!(
            (p.relief, p.terrain, p.continent, p.rivers, p.feature),
            (
                Relief::Hill,
                TerrainKind::Plains,
                4,
                0b101,
                BasisFeature::Marsh
            )
        );
        assert_eq!(p.river_flow, PlotBasis::NO_FLOW);
    }

    #[test]
    fn same_shape_ignores_the_records() {
        let a = TerrainBasis::uniform(3, 3, false, 1, flat());
        let mut b = a.clone();
        b.seed = 2;
        b.plots[0].relief = Relief::Hill;
        assert!(a.same_shape(&b));
        assert!(!a.same_shape(&TerrainBasis::uniform(3, 4, false, 1, flat())));
    }
}
