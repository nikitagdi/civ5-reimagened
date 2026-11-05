//! `Decal` subsystem: the decals the original bakes
//! into the pages of the cells ("Decal Layer", "Decal Macro Shadow"). This is the table of their
//! types, which `decal_filenames.xml` gives (`asset_format::DecalDef`), and the subsystem that the
//! switch `[DisableSystems] Decal` turns off. The layer itself (the banks of vertices, the grid, the
//! passes) comes with WT-6; until then the subsystem has no pass of its own and B1's feature decals
//! are drawn by `frame::b1_systems::B1Decal`.

use asset_format::DecalDef;

use crate::frame::{
    DrawContext, InitContext, PassId, PassSlot, PrepareContext, RenderSystem, SystemId,
};

/// The categories of decals, in the order of the original's `TerrainDecalLayer::m_typeRanges`; the
/// name is the element of `decal_filenames.xml`.
// re: TerrainDecalLayer.cpp:21-43 (kCategoryNames)
pub const CATEGORY_NAMES: [&str; 10] = [
    "Detail_Road",
    "Main_Road",
    "Crops",
    "Tree_Shadow",
    "Floodplains",
    "Floodplains_EndCap1",
    "Floodplains_EndCap2",
    "Riverbank",
    "Riverbank_EndCap1",
    "Riverbank_EndCap2",
];

/// The priority of a decal type that has none in the file.
// re: TerrainDecalLayer.cpp:21-43 (kDefaultPriority)
pub const DEFAULT_PRIORITY: f32 = 0.1;

/// Size of the cells of the grid that sorts the decals, in world units.
// re: TerrainDecalLayer.cpp:21-43 (kGridCellSize)
pub const GRID_CELL_SIZE: f32 = 256.0;

/// A decal larger than this goes into the list of the big ones, not into the grid.
// re: TerrainDecalLayer.cpp:21-43 (kBigDecalExtent)
pub const BIG_DECAL_EXTENT: f32 = 256.0;

/// Vertices a bank of decals holds at most (the index buffer is 16 bit).
// re: TerrainDecalLayer.cpp:21-43 (kMaxBankVertices)
pub const MAX_BANK_VERTICES: u32 = 0xffff;

/// The category of a decal: an index into [`CATEGORY_NAMES`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DecalCategory(pub u8);

impl DecalCategory {
    pub fn of_name(name: &str) -> Option<Self> {
        CATEGORY_NAMES
            .iter()
            .position(|n| *n == name)
            .map(|i| Self(i as u8))
    }

    pub fn name(self) -> &'static str {
        CATEGORY_NAMES[usize::from(self.0)]
    }
}

/// One decal type: the textures (`tx:` ids) and the priority that orders the decals of an area.
#[derive(Clone, Debug, PartialEq)]
pub struct DecalType {
    pub albedo: Option<asset_format::LogicalId>,
    pub heightmap: Option<asset_format::LogicalId>,
    pub priority: f32,
    /// The element of the file (`Floodplains_01`).
    pub name: String,
}

/// The types of each category, in the order of the file (`TerrainDecalLayer::LoadTerrainDecals`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DecalTypes {
    types: Vec<DecalType>,
    /// `[begin, end)` of each category in `types`.
    ranges: [(usize, usize); 10],
}

impl DecalTypes {
    /// Builds the table from the rows of `WorldSettings::decals`. The rows of a category are
    /// contiguous in the file; a category with no row has an empty range. A row of an element the
    /// original does not know is left out.
    pub fn from_defs(defs: &[DecalDef]) -> Self {
        let mut types = Vec::new();
        let mut ranges = [(0, 0); 10];
        for (c, name) in CATEGORY_NAMES.iter().enumerate() {
            let begin = types.len();
            for d in defs.iter().filter(|d| d.kind == *name) {
                types.push(DecalType {
                    albedo: d.albedo.clone(),
                    heightmap: d.heightmap.clone(),
                    priority: d.priority.unwrap_or(DEFAULT_PRIORITY),
                    name: d.name.clone(),
                });
            }
            ranges[c] = (begin, types.len());
        }
        Self { types, ranges }
    }

    pub fn len(&self) -> usize {
        self.types.len()
    }

    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    /// The types of a category.
    pub fn of(&self, category: DecalCategory) -> &[DecalType] {
        let (b, e) = self.ranges[usize::from(category.0)];
        &self.types[b..e]
    }

    /// The type a draw of the random stream `rand` picks in `category`: `rand() % count`
    /// (`TerrainDecalLayer::CreateDecal`). `None` if the category has no type.
    // re: TerrainDecalLayer.cpp:311-318 (the variant is `rand() % number of types`)
    pub fn pick(&self, category: DecalCategory, rand: u32) -> Option<&DecalType> {
        let list = self.of(category);
        if list.is_empty() {
            return None;
        }
        list.get(rand as usize % list.len())
    }
}

/// The decals: the table of types today; the layer comes with WT-6.
#[derive(Default)]
pub struct Decal {
    types: DecalTypes,
}

impl Decal {
    /// Takes the types of `decal_filenames.xml` (`WorldSettings::decals`).
    pub fn set_types(&mut self, defs: &[DecalDef]) {
        self.types = DecalTypes::from_defs(defs);
    }

    pub fn types(&self) -> &DecalTypes {
        &self.types
    }
}

impl RenderSystem for Decal {
    fn id(&self) -> SystemId {
        SystemId::Decal
    }

    fn passes(&self) -> &'static [PassSlot] {
        &[PassSlot::Pass(PassId::TerrainBake)]
    }

    fn init(&mut self, _cx: &mut InitContext<'_>) {}

    fn prepare(&mut self, _cx: &mut PrepareContext<'_>) {}

    fn record(&self, _slot: PassSlot, _cx: &mut DrawContext<'_, '_>) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use asset_format::{Extra, LogicalId};

    fn def(kind: &str, name: &str, priority: Option<f32>) -> DecalDef {
        DecalDef {
            kind: kind.into(),
            name: name.into(),
            albedo: Some(LogicalId::new(format!("tx:{name}.dds"))),
            heightmap: None,
            priority,
            extra: Extra::default(),
        }
    }

    #[test]
    fn there_are_ten_categories_in_the_order_of_the_original() {
        assert_eq!(CATEGORY_NAMES.len(), 10);
        assert_eq!(DecalCategory::of_name("Main_Road"), Some(DecalCategory(1)));
        assert_eq!(
            DecalCategory::of_name("Floodplains_EndCap2")
                .unwrap()
                .name(),
            "Floodplains_EndCap2"
        );
        assert_eq!(DecalCategory::of_name("Nope"), None);
    }

    #[test]
    fn a_category_with_three_types_picks_by_rand_mod_three() {
        let defs = [
            def("Floodplains", "Floodplains_01", Some(0.5)),
            def("Crops", "Crops_01", None),
            def("Floodplains", "Floodplains_02", None),
            def("Floodplains", "Floodplains_03", None),
        ];
        let t = DecalTypes::from_defs(&defs);
        let f = DecalCategory::of_name("Floodplains").unwrap();
        assert_eq!(t.of(f).len(), 3);
        // The rows of a category are in the order of the file, the others in between are not its own.
        assert_eq!(t.pick(f, 0).unwrap().name, "Floodplains_01");
        assert_eq!(t.pick(f, 1).unwrap().name, "Floodplains_02");
        assert_eq!(t.pick(f, 5).unwrap().name, "Floodplains_03");
        assert_eq!(t.pick(f, 7).unwrap().name, "Floodplains_02");
        // The priority of the file, or the default.
        assert_eq!(t.of(f)[0].priority, 0.5);
        assert_eq!(t.of(f)[1].priority, DEFAULT_PRIORITY);
        // A category with no row picks nothing.
        assert!(t.pick(DecalCategory(0), 3).is_none());
    }

    #[test]
    fn the_subsystem_keeps_the_table() {
        let mut d = Decal::default();
        assert!(d.types().is_empty());
        d.set_types(&[def("Crops", "Crops_01", None)]);
        assert_eq!(d.types().len(), 1);
    }
}
