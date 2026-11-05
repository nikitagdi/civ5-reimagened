//! Terrain texture layers: which texture a vertex shows, and how the textures of the
//! art table become the layers of one texture array. CPU only, so it is tested without a device.
//!
//! A vertex carries a *slot*: `layer type x continent style`. The slot is fixed by the cell, so
//! the vertex buffer does not depend on the assets. The material (see `terrain_pipeline.rs`) maps
//! each slot to an array layer and a texture scale, or to a flat colour when the art table or the
//! texture is missing ("coloured placeholder layers when assets are missing").

use std::collections::HashMap;

use asset_format::{ArtTable, LogicalId, TextureBlob};
use bytemuck::{Pod, Zeroable};

use super::mesh::VertexPart;
use super::water;
use super::{Relief, TerrainKind, TerrainMesh, TerrainSnapshot};
use crate::blend::srgb_to_linear;

/// Continent styles in the art table (0 Generic, 1 America, 2 Asia, 3 Africa, 4 Europe).
pub const STYLES: usize = 5;
/// Pseudo terrain type of the mountain top layer in the art table (`b1-terrain` set).
pub const MOUNTAIN_TOP_TYPE: &str = "TERRAIN_MOUNTAIN_TOP";
/// A texture array has fewer layers than this (WebGL2 limit).
pub const MAX_ARRAY_LAYERS: usize = 256;

/// What a surface is made of; each has a table type and a placeholder colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayerType {
    Grass,
    Plains,
    Desert,
    Tundra,
    Snow,
    Coast,
    Ocean,
    Hill,
    Mountain,
    MountainTop,
    Unknown,
}

impl LayerType {
    pub const ALL: [LayerType; 11] = [
        Self::Grass,
        Self::Plains,
        Self::Desert,
        Self::Tundra,
        Self::Snow,
        Self::Coast,
        Self::Ocean,
        Self::Hill,
        Self::Mountain,
        Self::MountainTop,
        Self::Unknown,
    ];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    /// Type string in `table:art-b1` (`terrain/<type>/<style>`); `Unknown` has no art.
    pub fn art_name(self) -> Option<&'static str> {
        Some(match self {
            Self::Grass => "TERRAIN_GRASS",
            Self::Plains => "TERRAIN_PLAINS",
            Self::Desert => "TERRAIN_DESERT",
            Self::Tundra => "TERRAIN_TUNDRA",
            Self::Snow => "TERRAIN_SNOW",
            Self::Coast => "TERRAIN_COAST",
            Self::Ocean => "TERRAIN_OCEAN",
            Self::Hill => "TERRAIN_HILL",
            Self::Mountain => "TERRAIN_MOUNTAIN",
            Self::MountainTop => MOUNTAIN_TOP_TYPE,
            Self::Unknown => return None,
        })
    }

    /// Flat colour (sRGB) of the layer when there is no texture.
    pub fn fallback_srgb(self) -> [u8; 3] {
        match self {
            Self::Grass => [86, 140, 52],
            Self::Plains => [170, 160, 78],
            Self::Desert => [222, 196, 120],
            Self::Tundra => [140, 150, 120],
            Self::Snow => [236, 240, 245],
            Self::Coast => [70, 150, 190],
            Self::Ocean => [24, 70, 140],
            Self::Hill => [128, 120, 58],
            Self::Mountain => [120, 115, 110],
            Self::MountainTop => [235, 235, 240],
            Self::Unknown => [200, 60, 200],
        }
    }

    fn is_land(self) -> bool {
        matches!(
            self,
            Self::Grass | Self::Plains | Self::Desert | Self::Tundra | Self::Snow
        )
    }
}

fn kind_type(kind: TerrainKind) -> LayerType {
    match kind {
        TerrainKind::Grass => LayerType::Grass,
        TerrainKind::Plains => LayerType::Plains,
        TerrainKind::Desert => LayerType::Desert,
        TerrainKind::Tundra => LayerType::Tundra,
        TerrainKind::Snow => LayerType::Snow,
        TerrainKind::Coast => LayerType::Coast,
        TerrainKind::Ocean => LayerType::Ocean,
        TerrainKind::Unknown => LayerType::Unknown,
    }
}

/// Layer of a vertex: a hill shows the texture of its base terrain (the catalog's own
/// `TERRAIN_HILL`, which the bridge cannot map, takes the hill layer); a mountain shows
/// `Mountain_Base` with `Mountain_Top` on the peak; skirts take the texture of their cell.
pub fn layer_type(kind: TerrainKind, relief: Relief, part: VertexPart) -> LayerType {
    if part == VertexPart::MountainTop {
        return LayerType::MountainTop;
    }
    match relief {
        Relief::Mountain => LayerType::Mountain,
        Relief::Hill => match kind_type(kind) {
            t if t.is_land() => t,
            _ => LayerType::Hill,
        },
        Relief::Flat | Relief::Water => kind_type(kind),
    }
}

/// Continent style of a cell; values outside 0..=4 take Generic.
pub fn art_style(art: u8) -> usize {
    if usize::from(art) < STYLES {
        usize::from(art)
    } else {
        0
    }
}

/// Number of slots: every layer type in every style.
pub const SLOT_COUNT: usize = LayerType::ALL.len() * STYLES;

/// Slot of a layer type in a continent style.
pub fn slot(layer: LayerType, art: u8) -> u32 {
    (layer.index() * STYLES + art_style(art)) as u32
}

/// Vertex of the terrain pipeline: position (0), normal (12), cell index into the fog-state
/// texture (24), slot (28), water depth (32), shore (36); stride 40.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct TerrainGpuVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub cell: u32,
    pub slot: u32,
    /// Depth of the water for the water pass (`terrain::water`), 0 on land.
    pub water: f32,
    /// 1 at a water corner that touches land, 0 elsewhere (foam of the water pass).
    pub shore: f32,
}

/// The mesh's vertices with their slots. Rebuild it when any cell's material changes, not only
/// the relief.
pub fn gpu_vertices(snapshot: &TerrainSnapshot, mesh: &TerrainMesh) -> Vec<TerrainGpuVertex> {
    mesh.vertices
        .iter()
        .map(|v| {
            let cell = &snapshot.cells()[v.cell as usize];
            let layer = layer_type(cell.kind, cell.drawn_relief(), v.part());
            let (water, shore) = water::vertex_depth(snapshot, v);
            TerrainGpuVertex {
                position: v.position,
                normal: v.normal,
                cell: v.cell,
                slot: slot(layer, cell.art),
                water,
                shore,
            }
        })
        .collect()
}

/// Art of one slot from the table.
#[derive(Debug, Clone, PartialEq)]
pub struct SlotArt {
    pub texture: Option<LogicalId>,
    /// `uv = (world X, world Z) * scale`.
    pub scale: f32,
}

/// What `table:art-b1` says about every slot.
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainArtSlots {
    slots: Vec<SlotArt>,
}

impl TerrainArtSlots {
    /// No art at all: every slot is a flat colour.
    pub fn none() -> Self {
        Self {
            slots: vec![
                SlotArt {
                    texture: None,
                    scale: 0.0,
                };
                SLOT_COUNT
            ],
        }
    }

    /// `terrain/<type>/<style>` for each slot; a slot without a record stays a flat colour.
    pub fn from_table(table: &ArtTable) -> Self {
        let mut out = Self::none();
        for ty in LayerType::ALL {
            let Some(name) = ty.art_name() else { continue };
            for style in 0..STYLES {
                let Some(rec) = table.terrain(name, style as u8) else {
                    continue;
                };
                // SIMPLIFIED(SIMP-005): a `MeshUv` row is drawn with the world projection too; no
                // row of the cell types uses it.
                out.slots[ty.index() * STYLES + style] = SlotArt {
                    texture: Some(rec.texture.clone()),
                    scale: rec.texture_scale,
                };
            }
        }
        out
    }

    /// One slot per texture, scale 1 (not terrain slots: the decal array uses the same layer
    /// planning). [`LayerPlan::build`] then has one slot per texture, in this order.
    pub fn from_textures(textures: &[LogicalId]) -> Self {
        Self {
            slots: textures
                .iter()
                .map(|id| SlotArt {
                    texture: Some(id.clone()),
                    scale: 1.0,
                })
                .collect(),
        }
    }

    pub fn slot(&self, index: usize) -> &SlotArt {
        &self.slots[index]
    }

    /// Distinct textures in slot order: what to fetch.
    pub fn textures(&self) -> Vec<LogicalId> {
        let mut out: Vec<LogicalId> = Vec::new();
        for id in self.slots.iter().filter_map(|s| s.texture.as_ref()) {
            if !out.contains(id) {
                out.push(id.clone());
            }
        }
        out
    }
}

/// How one slot is drawn: an array layer with a scale, or the flat colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlotInfo {
    pub layer: Option<u32>,
    pub scale: f32,
    /// Linear colour used when `layer` is `None`.
    pub color: [f32; 3],
}

/// The texture array to build and the slot table that goes with it.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerPlan {
    /// Array layers in order.
    pub layers: Vec<LogicalId>,
    /// Per layer: how many times its texture repeats along each side to fill the layer (1 when
    /// the texture has the array's size).
    pub tiles: Vec<u32>,
    /// `SLOT_COUNT` entries.
    pub slots: Vec<SlotInfo>,
    /// Size of every layer, `None` without layers.
    pub size: Option<(u32, u32)>,
    /// Mip levels the array has: the fewest any layer can fill; 0 without layers.
    pub mip_count: u32,
    /// Textures left out, with the reason (format, size that does not tile).
    pub rejected: Vec<(LogicalId, String)>,
}

fn slot_color(index: usize) -> [f32; 3] {
    srgb_to_linear(LayerType::ALL[index / STYLES].fallback_srgb())
}

impl LayerPlan {
    /// Every slot a flat colour (no assets).
    pub fn fallback() -> Self {
        Self {
            layers: Vec::new(),
            tiles: Vec::new(),
            slots: (0..SLOT_COUNT)
                .map(|i| SlotInfo {
                    layer: None,
                    scale: 0.0,
                    color: slot_color(i),
                })
                .collect(),
            size: None,
            mip_count: 0,
            rejected: Vec::new(),
        }
    }

    /// Layers from the textures that arrived. All layers of an array share format and size: the
    /// first texture in slot order sets the format, the largest texture of that format the size.
    /// A smaller texture whose sides divide the layer by the same power of two (the original has
    /// 128 px and 256 px terrain textures next to the 512 px ones) is repeated to fill its
    /// layer, and its slots get the scale divided by that factor, so it looks the same. Any other
    /// texture is left out and its slots stay flat colours, like the slots whose texture is
    /// missing.
    pub fn build(art: &TerrainArtSlots, blobs: &HashMap<LogicalId, TextureBlob>) -> Self {
        let mut plan = Self::fallback();
        if art.slots.len() != plan.slots.len() {
            // Not the terrain slots (decals): one slot per art slot, no flat colour.
            plan.slots = vec![
                SlotInfo {
                    layer: None,
                    scale: 0.0,
                    color: [1.0; 3],
                };
                art.slots.len()
            ];
        }
        let present: Vec<(usize, &LogicalId, &TextureBlob)> = art
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let id = s.texture.as_ref()?;
                Some((i, id, blobs.get(id)?))
            })
            .collect();
        let Some(&(_, ref_id, reference)) = present.first() else {
            return plan;
        };
        let (format, srgb) = (reference.format, reference.srgb);
        let (tw, th) = present
            .iter()
            .map(|(_, _, b)| b)
            .filter(|b| (b.format, b.srgb) == (format, srgb))
            .max_by_key(|b| u64::from(b.width) * u64::from(b.height))
            .map_or((reference.width, reference.height), |b| (b.width, b.height));
        let mut tile_of: HashMap<&LogicalId, Result<u32, String>> = HashMap::new();
        for &(i, id, blob) in &present {
            let tile = tile_of
                .entry(id)
                .or_insert_with(|| tile_factor(blob, (format, srgb, ref_id), (tw, th)));
            let Ok(k) = *tile else { continue };
            let layer = match plan.layers.iter().position(|l| l == id) {
                Some(l) => l,
                None if plan.layers.len() < MAX_ARRAY_LAYERS - 1 => {
                    plan.layers.push(id.clone());
                    plan.tiles.push(k);
                    plan.layers.len() - 1
                }
                None => {
                    tile_of.insert(id, Err("texture array is full".to_owned()));
                    continue;
                }
            };
            plan.slots[i].layer = Some(layer as u32);
            plan.slots[i].scale = art.slots[i].scale / k as f32;
        }
        for &(_, id, _) in &present {
            if let Some(Err(why)) = tile_of.get(id)
                && !plan.rejected.iter().any(|(r, _)| r == id)
            {
                plan.rejected.push((id.clone(), why.clone()));
            }
        }
        if !plan.layers.is_empty() {
            plan.size = Some((tw, th));
            plan.mip_count = plan
                .layers
                .iter()
                .zip(&plan.tiles)
                .filter_map(|(id, k)| Some(blobs.get(id)?.mips.len() as u32 + k.ilog2()))
                .min()
                .unwrap_or(0);
        }
        plan
    }
}

/// How often `blob` repeats along each side to fill a layer of `size`, or why it cannot.
fn tile_factor(
    blob: &TextureBlob,
    (format, srgb, ref_id): (asset_format::TextureFormat, bool, &LogicalId),
    (tw, th): (u32, u32),
) -> Result<u32, String> {
    if blob.format != format || blob.srgb != srgb {
        return Err(format!(
            "{:?} (srgb {}) differs from {:?} (srgb {}) of {}",
            blob.format,
            blob.srgb,
            format,
            srgb,
            ref_id.as_str()
        ));
    }
    let (w, h) = (blob.width, blob.height);
    let k = tw / w.max(1);
    let fits = w * k == tw && h * k == th && k.is_power_of_two();
    let blocks_ok = format == asset_format::TextureFormat::Rgba8 || (w % 4 == 0 && h % 4 == 0);
    if fits && blocks_ok {
        Ok(k)
    } else {
        Err(format!("{w}x{h} does not tile into the {tw}x{th} layers"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::TerrainCell;
    use crate::terrain::mesh::VertexPart::{Base, MountainTop, Skirt};
    use crate::terrain::test_util::{land, map, water};
    use asset_format::{Projection, TerrainArt, TextureFormat};

    fn art(file: &str, scale: f32) -> TerrainArt {
        TerrainArt {
            texture: LogicalId::new(format!("tex:{file}")),
            texture_scale: scale,
            projection: Projection::WorldXz,
        }
    }

    /// A table like `b1-terrain` writes: a texture per type and style, shared between styles of
    /// the same continent file.
    fn full_table() -> ArtTable {
        let mut t = ArtTable::new();
        for ty in LayerType::ALL {
            let Some(name) = ty.art_name() else { continue };
            for style in 0..STYLES as u8 {
                // Generic and Europe share a file, the rest have their own.
                let file = if style == 0 || style == 4 {
                    format!("{}_euro", name.to_lowercase())
                } else {
                    format!("{}_{style}", name.to_lowercase())
                };
                t.insert_terrain(name, style, art(&file, 0.01 + f32::from(style) * 0.001));
            }
        }
        t
    }

    fn blob(w: u32, h: u32, format: TextureFormat, mips: usize) -> TextureBlob {
        TextureBlob {
            format,
            dim: asset_format::TexDim::D2,
            srgb: true,
            width: w,
            height: h,
            depth: 1,
            mips: (0..mips)
                .map(|l| vec![0; format.mip_size((w >> l).max(1), (h >> l).max(1))])
                .collect(),
        }
    }

    fn blobs_for(art: &TerrainArtSlots, w: u32, mips: usize) -> HashMap<LogicalId, TextureBlob> {
        art.textures()
            .into_iter()
            .map(|id| (id, blob(w, w, TextureFormat::Rgba8, mips)))
            .collect()
    }

    #[test]
    fn layer_selection_table() {
        use LayerType as L;
        use Relief::{Flat, Hill, Mountain, Water};
        use TerrainKind as K;
        let cases = [
            (K::Grass, Flat, Base, L::Grass),
            (K::Plains, Flat, Base, L::Plains),
            (K::Desert, Flat, Base, L::Desert),
            (K::Tundra, Flat, Base, L::Tundra),
            (K::Snow, Flat, Base, L::Snow),
            (K::Coast, Water, Base, L::Coast),
            (K::Ocean, Water, Base, L::Ocean),
            (K::Unknown, Flat, Base, L::Unknown),
            // A hill shows the base terrain; the catalog's TERRAIN_HILL takes the hill layer.
            (K::Desert, Hill, Base, L::Desert),
            (K::Unknown, Hill, Base, L::Hill),
            (K::Ocean, Hill, Base, L::Hill),
            // A mountain is Mountain_Base, its peak Mountain_Top, whatever the terrain.
            (K::Grass, Mountain, Base, L::Mountain),
            (K::Unknown, Mountain, Base, L::Mountain),
            (K::Grass, Mountain, MountainTop, L::MountainTop),
            // Skirts take the texture of their cell.
            (K::Plains, Flat, Skirt, L::Plains),
            (K::Plains, Hill, Skirt, L::Plains),
            (K::Plains, Mountain, Skirt, L::Mountain),
        ];
        for (kind, relief, part, want) in cases {
            assert_eq!(
                layer_type(kind, relief, part),
                want,
                "{kind:?} {relief:?} {part:?}"
            );
        }
    }

    #[test]
    fn slots_are_unique_per_type_and_style_and_in_range() {
        let mut seen = std::collections::HashSet::new();
        for ty in LayerType::ALL {
            for style in 0..STYLES as u8 {
                let s = slot(ty, style);
                assert!((s as usize) < SLOT_COUNT);
                assert!(seen.insert(s), "{ty:?} {style}");
            }
        }
        assert_eq!(seen.len(), SLOT_COUNT);
        // Out-of-range continent art is Generic.
        assert_eq!(slot(LayerType::Grass, 9), slot(LayerType::Grass, 0));
    }

    #[test]
    fn table_picks_texture_by_type_and_style() {
        let art = TerrainArtSlots::from_table(&full_table());
        let grass_style2 = art.slot(LayerType::Grass.index() * STYLES + 2);
        assert_eq!(
            grass_style2.texture,
            Some(LogicalId::new("tex:terrain_grass_2"))
        );
        assert!((grass_style2.scale - 0.012).abs() < 1e-6);
        let top = art.slot(LayerType::MountainTop.index() * STYLES + 3);
        assert_eq!(
            top.texture,
            Some(LogicalId::new("tex:terrain_mountain_top_3"))
        );
        // The unknown type never has art.
        assert!(
            art.slot(LayerType::Unknown.index() * STYLES)
                .texture
                .is_none()
        );
        // Distinct textures only: Generic and Europe share a file per type.
        let want = (LayerType::ALL.len() - 1) * 4;
        assert_eq!(art.textures().len(), want);
    }

    #[test]
    fn plan_gives_each_slot_its_layer_and_scale() {
        let art = TerrainArtSlots::from_table(&full_table());
        let plan = LayerPlan::build(&art, &blobs_for(&art, 8, 4));
        assert_eq!(plan.layers.len(), 40);
        assert!(plan.layers.len() < MAX_ARRAY_LAYERS);
        assert_eq!(plan.mip_count, 4);
        assert!(plan.rejected.is_empty());
        let s = plan.slots[LayerType::Ocean.index() * STYLES + 1];
        let layer = s.layer.expect("ocean has a layer") as usize;
        assert_eq!(plan.layers[layer].as_str(), "tex:terrain_ocean_1");
        assert!((s.scale - 0.011).abs() < 1e-6);
        // Generic and Europe share a layer.
        assert_eq!(
            plan.slots[LayerType::Ocean.index() * STYLES].layer,
            plan.slots[LayerType::Ocean.index() * STYLES + 4].layer
        );
        // No art for the unknown type: still a flat colour.
        assert_eq!(plan.slots[LayerType::Unknown.index() * STYLES].layer, None);
    }

    #[test]
    fn plan_without_assets_is_flat_colours() {
        for plan in [
            LayerPlan::fallback(),
            LayerPlan::build(&TerrainArtSlots::none(), &HashMap::new()),
            LayerPlan::build(
                &TerrainArtSlots::from_table(&ArtTable::new()),
                &HashMap::new(),
            ),
        ] {
            assert!(plan.layers.is_empty());
            assert_eq!(plan.mip_count, 0);
            assert_eq!(plan.slots.len(), SLOT_COUNT);
            assert!(plan.slots.iter().all(|s| s.layer.is_none()));
            // Terrain kinds stay told apart by colour.
            let c = |t: LayerType| plan.slots[t.index() * STYLES].color;
            assert_ne!(c(LayerType::Grass), c(LayerType::Ocean));
            assert_ne!(c(LayerType::Mountain), c(LayerType::MountainTop));
            assert!(c(LayerType::Grass).iter().all(|v| (0.0..=1.0).contains(v)));
        }
    }

    #[test]
    fn table_without_textures_loaded_keeps_colours_and_partial_loads_mix() {
        let art = TerrainArtSlots::from_table(&full_table());
        // The table is there, but no texture arrived.
        let none = LayerPlan::build(&art, &HashMap::new());
        assert!(none.layers.is_empty() && none.slots.iter().all(|s| s.layer.is_none()));
        // Only grass arrived: grass is textured, the rest flat.
        let mut blobs = blobs_for(&art, 8, 3);
        blobs.retain(|id, _| id.as_str().starts_with("tex:terrain_grass"));
        let plan = LayerPlan::build(&art, &blobs);
        assert!(
            plan.slots[LayerType::Grass.index() * STYLES]
                .layer
                .is_some()
        );
        assert!(
            plan.slots[LayerType::Plains.index() * STYLES]
                .layer
                .is_none()
        );
        assert_eq!(plan.layers.len(), 4);
    }

    #[test]
    fn smaller_textures_tile_into_the_largest_size() {
        let art = TerrainArtSlots::from_table(&full_table());
        let mut blobs = blobs_for(&art, 8, 4);
        let id = |s: &str| LogicalId::new(s);
        // The ocean floor of the original is 128 px between 512 px textures: here 2 px next to 8.
        blobs.insert(
            id("tex:terrain_ocean_euro"),
            blob(2, 2, TextureFormat::Rgba8, 2),
        );
        let plan = LayerPlan::build(&art, &blobs);
        assert!(plan.rejected.is_empty());
        assert_eq!(plan.size, Some((8, 8)));
        let ocean = plan.slots[LayerType::Ocean.index() * STYLES];
        let layer = ocean.layer.expect("ocean is textured") as usize;
        assert_eq!(plan.tiles[layer], 4);
        // The texture repeats 4 times per layer, so the scale drops by 4 to keep its look.
        assert!((ocean.scale - 0.01 / 4.0).abs() < 1e-6);
        // Other layers are untouched.
        let grass = plan.slots[LayerType::Grass.index() * STYLES + 2];
        assert_eq!(plan.tiles[grass.layer.unwrap() as usize], 1);
        assert!((grass.scale - 0.012).abs() < 1e-6);
        // 2 mips plus the 2 levels the repeating gives: 4, same as the others.
        assert_eq!(plan.mip_count, 4);
    }

    #[test]
    fn odd_textures_are_left_out_and_their_slots_stay_flat() {
        let art = TerrainArtSlots::from_table(&full_table());
        let mut blobs = blobs_for(&art, 8, 4);
        let id = |s: &str| LogicalId::new(s);
        // Does not divide, other format, two mips only (that only lowers the array's count).
        blobs.insert(
            id("tex:terrain_desert_euro"),
            blob(6, 6, TextureFormat::Rgba8, 3),
        );
        blobs.insert(id("tex:terrain_snow_2"), blob(8, 8, TextureFormat::Bc7, 2));
        blobs.insert(
            id("tex:terrain_coast_1"),
            blob(8, 8, TextureFormat::Rgba8, 2),
        );
        let plan = LayerPlan::build(&art, &blobs);
        assert_eq!(plan.rejected.len(), 2);
        let desert = plan.slots[LayerType::Desert.index() * STYLES];
        assert_eq!(desert.layer, None);
        let snow2 = plan.slots[LayerType::Snow.index() * STYLES + 2];
        assert_eq!(snow2.layer, None);
        // A texture used by two styles is left out for both.
        assert_eq!(
            plan.slots[LayerType::Desert.index() * STYLES + 4].layer,
            None
        );
        // Accepted, but it has two mips only: the array drops to two levels.
        assert!(
            plan.slots[LayerType::Coast.index() * STYLES + 1]
                .layer
                .is_some()
        );
        assert_eq!(plan.mip_count, 2);
    }

    #[test]
    fn gpu_vertices_carry_cell_and_slot() {
        let snap = map(3, 3, |x, y| match (x, y) {
            (0, 0) => TerrainCell {
                kind: TerrainKind::Desert,
                art: 3,
                ..land()
            },
            (1, 1) => TerrainCell {
                relief: Relief::Mountain,
                kind: TerrainKind::Grass,
                art: 1,
                ..land()
            },
            (2, 2) => water(),
            (2, 0) => TerrainCell::default(),
            _ => land(),
        });
        let mesh = TerrainMesh::build(&snap);
        let verts = gpu_vertices(&snap, &mesh);
        assert_eq!(verts.len(), mesh.vertices.len());
        let desert = slot(LayerType::Desert, 3);
        let base = slot(LayerType::Mountain, 1);
        let top = slot(LayerType::MountainTop, 1);
        let ocean = slot(LayerType::Ocean, 0);
        let slots_of = |cell: u32| -> std::collections::HashSet<u32> {
            verts
                .iter()
                .filter(|v| v.cell == cell)
                .map(|v| v.slot)
                .collect()
        };
        assert_eq!(slots_of(0), [desert].into());
        assert!(slots_of(4).is_superset(&[base, top].into()));
        assert!(slots_of(4).iter().all(|s| [base, top].contains(s)));
        assert_eq!(slots_of(8), [ocean].into());
        // An unrevealed cell is flat and has no terrain; the fog hides it.
        assert_eq!(slots_of(2), [slot(LayerType::Unknown, 0)].into());
        // Mountain-top vertices exist only on the mountain.
        for v in &mesh.vertices {
            if v.part() == MountainTop {
                assert_eq!(v.cell, 4);
            }
        }
        assert!(verts.iter().all(|v| (v.cell as usize) < snap.cells().len()));
    }

    #[test]
    fn vertex_layout() {
        assert_eq!(std::mem::size_of::<TerrainGpuVertex>(), 40);
    }
}
