//! The decals of the farms (world-objects WO-D7, 4.5, WOB4): the fields under a `FARM` landmark are
//! not geometry but `Crops` decals that the terrain bakes into the page of the cell
//! (`DecalLayer::set_object_decals`, source `Farms`).
//!
//! The port of `cvFarmSimSystem`: `LoadFarmDecalTextures` reads `farm_types.xml` (`MiscData.fpk`),
//! eight groups of field textures ([`FARM_GROUPS`]); `CreateFarmDecal` makes one decal per plot
//! polygon of the farm with a type drawn from the group of the farm: the group of the continent art
//! of the hex (America, Asia, Africa, Europe; none for 0), `Crops_Half_Built` while the farm is being
//! built, `Crops_Wheat` / `Crops_Wheat_HB` on wheat (only `ART_DEF_RESOURCE_WHEAT`, any other
//! resource, the maize of Lekmod too, takes the crops of the continent), and over a pillaged farm a
//! second decal of `Crops_Damage`. The texture is laid in the world, not stretched over the polygon:
//! `uv = rotate((p - centroid) / 25.6 + 0.5, angle)`, the angle the direction of the layout field at
//! the centroid ([`super::heading`]), a quarter turn more for one draw in three.
//!
//! The plot polygons of the original are the faces of the road network that `cvRoadNetworkManager`
//! traces along the heading field, each given to the hex its centroid lies in
//! (`cvPlotPolygonManager::ProcessPlots`); ours are cells of the same spacing in the frame of the
//! heading ([`field_cells`], SIMP-exe-1210).

use std::collections::BTreeSet;

use asset_format::{DecalDef, LogicalId};
use glam::Vec2;

use super::heading::HeadingField;
use super::select::mix;
use crate::hexgrid::{HEX_WIDTH, cell_center, nearest_cell};
use crate::world::decal::{DecalCategory, DecalType};
use crate::world::terrain::decals::{DecalLayer, DecalShape, DecalSource, ObjectDecal};

/// What the landmark of a farm is in: the texture group depends on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FarmState {
    UnderConstruction,
    Constructed,
    Pillaged,
}

/// A farm whose fields are decals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Farm {
    pub x: i32,
    pub y: i32,
    pub state: FarmState,
    /// The plot has `ART_DEF_RESOURCE_WHEAT` (the yellow fields).
    pub wheat: bool,
}

/// The art define whose farms get the wheat fields.
// re: cvFarmSimSystem::StartupGame 0x08a1c82c (FindLandmarkType "ART_DEF_RESOURCE_WHEAT" -> +0x4a8),
// OnImprovementCreated 0x08a1a894 (flag 0x200 = the resource of the event is that one)
pub const WHEAT: &str = "ART_DEF_RESOURCE_WHEAT";

/// One field texture of `farm_types.xml`: the albedo and the height (`tx:` file names).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FarmTexture {
    pub albedo: &'static str,
    pub height: Option<&'static str>,
    pub priority: f32,
}

/// A group of `farm_types.xml`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FarmGroup {
    pub name: &'static str,
    pub textures: &'static [FarmTexture],
}

const fn crop(albedo: &'static str, height: &'static str) -> FarmTexture {
    FarmTexture {
        albedo,
        height: Some(height),
        priority: 0.5,
    }
}

/// The eight crops of a style: the heights of the entries are 01, 02, 02, 04, 05, 05, 02, 02.
macro_rules! crops {
    ($style:literal) => {
        &[
            crop(
                concat!("crops_", $style, "_01_d.dds"),
                "crops_generic_01_h.dds",
            ),
            crop(
                concat!("crops_", $style, "_02_d.dds"),
                "crops_generic_02_h.dds",
            ),
            crop(
                concat!("crops_", $style, "_03_d.dds"),
                "crops_generic_02_h.dds",
            ),
            crop(
                concat!("crops_", $style, "_04_d.dds"),
                "crops_generic_04_h.dds",
            ),
            crop(
                concat!("crops_", $style, "_05_d.dds"),
                "crops_generic_05_h.dds",
            ),
            crop(
                concat!("crops_", $style, "_06_d.dds"),
                "crops_generic_05_h.dds",
            ),
            crop(
                concat!("crops_", $style, "_07_d.dds"),
                "crops_generic_02_h.dds",
            ),
            crop(
                concat!("crops_", $style, "_08_d.dds"),
                "crops_generic_02_h.dds",
            ),
        ]
    };
}

/// `farm_types.xml` in the order `LoadFarmDecalTextures` reads its groups (the name table at
/// "Crops_Half_Built", stride 0x20; the ranges at `this+0x24 + 4 * group`).
// re: cvFarmSimSystem::LoadFarmDecalTextures 0x08a1bcf6, Startup 0x08a1bc12 ("farm_types.xml");
// the file of MiscData.fpk (Crops_Damage: heightmap "blank.dds", which the install does not have)
pub const FARM_GROUPS: [FarmGroup; 8] = [
    FarmGroup {
        name: "Crops_Half_Built",
        textures: crops!("half_built"),
    },
    FarmGroup {
        name: "Crops_Africa",
        textures: crops!("africa"),
    },
    FarmGroup {
        name: "Crops_America",
        textures: crops!("america"),
    },
    FarmGroup {
        name: "Crops_Asia",
        textures: crops!("asia"),
    },
    FarmGroup {
        name: "Crops_Europe",
        textures: crops!("europe"),
    },
    FarmGroup {
        name: "Crops_Wheat",
        textures: &[crop("wheat_farm_d.dds", "wheat_farm_h.dds")],
    },
    FarmGroup {
        name: "Crops_Wheat_HB",
        textures: &[crop(
            "wheat_farm_half_built_d.dds",
            "wheat_farm_half_built_h.dds",
        )],
    },
    FarmGroup {
        name: "Crops_Damage",
        textures: &[FarmTexture {
            albedo: "crops_destroyed_01_d.dds",
            height: None,
            priority: 0.51,
        }],
    },
];

const HALF_BUILT: usize = 0;
const WHEAT_GROUP: usize = 5;
const WHEAT_HB: usize = 6;
const DAMAGE: usize = 7;

/// The group of the fields of a farm, `None` when it has none: a hex of the continent art 0 gets no
/// field.
// re: cvFarmSimSystem::CreateFarmDecal 0x08a1d4ac (switch on (flags >> 4) & 0xf: 1 -> +0x2c America,
// 2 -> +0x30 Asia, 3 -> +0x28 Africa, 4 -> +0x34 Europe, else return; then state 2 -> +0x24, wheat
// (flag 0x200) -> +0x38 or, state 2, +0x3c)
pub fn farm_group(farm: &Farm, continent: u8) -> Option<usize> {
    let style = match continent {
        1 => 2,
        2 => 3,
        3 => 1,
        4 => 4,
        _ => return None,
    };
    let building = farm.state == FarmState::UnderConstruction;
    Some(match (farm.wheat, building) {
        (true, true) => WHEAT_HB,
        (true, false) => WHEAT_GROUP,
        (false, true) => HALF_BUILT,
        (false, false) => style,
    })
}

fn tx(name: &str) -> LogicalId {
    LogicalId::new(format!("tx:{name}"))
}

/// `defs` with the `Crops` category replaced by the textures of `farm_types.xml`: the group of
/// `decal_filenames.xml` names no file (the farms load their own), so the layer gets one type per
/// field texture, named `<group>/<NN>`, and the farms pick theirs by the albedo.
pub fn with_field_types(defs: &[DecalDef]) -> Vec<DecalDef> {
    let mut out: Vec<DecalDef> = defs.iter().filter(|d| d.kind != "Crops").cloned().collect();
    for g in &FARM_GROUPS {
        for (n, t) in g.textures.iter().enumerate() {
            out.push(DecalDef {
                kind: "Crops".into(),
                name: format!("{}/{:02}", g.name, n + 1),
                albedo: Some(tx(t.albedo)),
                heightmap: t.height.map(tx),
                priority: Some(t.priority),
                extra: Default::default(),
            });
        }
    }
    out
}

/// The texture of the field laid in the world: one repeat in this many units.
// re: CreateFarmDecal 0x08a1d4ac (uv = (p - c) / 25.6f @0x09224508 + 0.5f @0x0912c9e8)
pub const TEXTURE_PERIOD: f32 = 25.6;

/// The side of a field: the spacing of the streamlines of the two families that bound the plots.
// re: cvRoadNetworkManager ctor 0x087e2558 (seed spacing 14.4; side seeds every 14.4 of arc,
// ManagerQueueSideSeeds 0x087e559c)
pub const FIELD_SIDE: f32 = 14.4;

/// The smallest plot the farm model stands on.
// re: FillLandmarkSimInfo 0x08a1c214 (|area| >= 100.0)
pub const ANCHOR_MIN_AREA: f32 = 100.0;

/// A field: its corners (round it) and a number of its own (for the texture).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FieldCell {
    pub corners: [Vec2; 4],
    pub key: u32,
}

impl FieldCell {
    pub fn centroid(&self) -> Vec2 {
        (self.corners[0] + self.corners[2]) * 0.5
    }

    pub fn area(&self) -> f32 {
        (self.corners[1] - self.corners[0])
            .perp_dot(self.corners[3] - self.corners[0])
            .abs()
    }
}

/// A number of hex `(x, y)`.
fn hex_key(x: i32, y: i32) -> u32 {
    (x as u32).wrapping_mul(0x9e37_79b1) ^ (y as u32).wrapping_mul(0x85eb_ca6b)
}

/// The fields whose centroid lies in hex `(x, y)`: cells of [`FIELD_SIDE`] in the frame of the
/// heading at the hex centre, the lattice shifted by a fraction of a cell from the plot.
///
/// SIMPLIFIED(SIMP-exe-1210): the original's plots are the faces of streamlines of two families
/// traced through the heading field (`cvRoadNetworkManager`, the tracer and the face walk are not
/// read); ours are straight cells of the same spacing turned by the heading of the hex, so a field
/// does not curve and the fields of two farms side by side may overlap or leave a sliver at the
/// border of their hexes.
pub fn field_cells(x: i32, y: i32, heading: &HeadingField) -> Vec<FieldCell> {
    let c = cell_center(x, y);
    let h = heading.angle(c);
    // Along the heading and across it, in our (X, Z): the original's y is our -Z.
    let e1 = Vec2::new(h.cos(), -h.sin()) * FIELD_SIDE;
    let e2 = Vec2::new(-h.sin(), -h.cos()) * FIELD_SIDE;
    let k = hex_key(x, y);
    let frac = |salt| (mix(k, salt) % 1000) as f32 / 1000.0;
    let origin = c + e1 * frac(0xf1a5) + e2 * frac(0xf1a6);
    let n = (HEX_WIDTH / FIELD_SIDE).ceil() as i32 + 1;
    let mut out = Vec::new();
    for i in -n..=n {
        for j in -n..=n {
            let p = origin + e1 * i as f32 + e2 * j as f32;
            let cell = FieldCell {
                corners: [p, p + e1, p + e1 + e2, p + e2],
                key: mix(k ^ ((i + 64) as u32) << 8 ^ (j + 64) as u32, 0xf1e1),
            };
            if nearest_cell(cell.centroid()) == (x, y) {
                out.push(cell);
            }
        }
    }
    out
}

/// The texture coordinates of the corners of a field: the world point from the centroid in repeats
/// of the texture (the original's world, y north), turned by the heading at the centroid, a quarter
/// turn more for one field in three.
// re: CreateFarmDecal 0x08a1d4ac (D3DXMatrixRotationZ(table[rand % 3 == 0] - atan2(field)), table
// {-0.0, pi/2} @0x0922450c; the vertex (u, v, 0, 1) times the matrix)
pub fn field_uvs(cell: &FieldCell, heading: &HeadingField) -> [Vec2; 4] {
    let c = cell.centroid();
    let field_angle = heading.angle(c);
    let turn = if mix(cell.key, 0x7a6e).is_multiple_of(3) {
        std::f32::consts::FRAC_PI_2
    } else {
        -0.0
    };
    let (s, co) = (turn - field_angle).sin_cos();
    cell.corners.map(|p| {
        let d = p - c;
        let q = Vec2::new(d.x, -d.y) / TEXTURE_PERIOD + Vec2::splat(0.5);
        // D3DXMatrixRotationZ, row vector times the matrix: (x c - y s, x s + y c).
        Vec2::new(q.x * co - q.y * s, q.x * s + q.y * co)
    })
}

/// Where the farm model of hex `(x, y)` stands and its turn: on the centroid of the field nearest
/// the hex centre (of those of [`ANCHOR_MIN_AREA`] or more; the centre if none), turned by the
/// heading there and a whole number of quarter turns.
// re: FillLandmarkSimInfo 0x08a1c214 (anchor; yaw = atan2(LayoutHeading(anchor)) + (rand() % 4) * pi/2,
// every piece the same matrix RotZ(yaw) + T(anchor))
pub fn farm_anchor(x: i32, y: i32, heading: &HeadingField) -> (Vec2, f32) {
    let c = cell_center(x, y);
    let anchor = field_cells(x, y, heading)
        .iter()
        .filter(|f| f.area() >= ANCHOR_MIN_AREA)
        .map(FieldCell::centroid)
        .min_by(|a, b| a.distance_squared(c).total_cmp(&b.distance_squared(c)))
        .unwrap_or(c);
    let k = mix(hex_key(x, y), 0xfa47) % 4;
    let yaw = heading.angle(anchor) + k as f32 * std::f32::consts::FRAC_PI_2;
    (anchor, yaw)
}
/// The albedo of a field of `group`: `rand() % count` of the group.
// re: CreateFarmDecal 0x08a1d4ac (rand_wincompatible() % count + first)
fn pick(group: usize, key: u32) -> &'static FarmTexture {
    let list = FARM_GROUPS[group].textures;
    &list[mix(key, 0x9a7e) as usize % list.len()]
}

/// The decals of the farms. `width` is the width of the map in hexes, `heading` the layout field of
/// the map and `continent(x, y)` the continent art of a hex.
pub fn farm_decals(
    farms: &[Farm],
    width: i32,
    heading: &HeadingField,
    continent: &dyn Fn(i32, i32) -> u8,
) -> Vec<ObjectDecal> {
    let Some(category) = DecalCategory::of_name("Crops") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for f in farms {
        let Some(group) = farm_group(f, continent(f.x, f.y)) else {
            continue;
        };
        let cell = (f.y * width + f.x) as u32;
        for field in field_cells(f.x, f.y, heading) {
            let uvs = field_uvs(&field, heading);
            let shape = DecalShape::Mesh {
                verts: field.corners.iter().copied().zip(uvs).collect(),
                tris: vec![[0, 1, 2], [0, 2, 3]],
            };
            let mut layers = vec![pick(group, field.key)];
            if f.state == FarmState::Pillaged {
                // re: CreateFarmDecal (state 8: a second decal of the group at +0x40 over the first)
                layers.push(pick(DAMAGE, field.key));
            }
            for t in layers {
                out.push(ObjectDecal {
                    category,
                    texture: Some(tx(t.albedo)),
                    shape: shape.clone(),
                    cell,
                    casts_shadow: false,
                });
            }
        }
    }
    out
}

/// The fields of the farms in the layer: replaces the decals of the plots in `touched` (every plot
/// that has a farm now or had one) with the ones for `farms`. Returns the plots that have fields.
pub fn apply(
    layer: &mut DecalLayer,
    farms: &[Farm],
    touched: &BTreeSet<(i32, i32)>,
    width: i32,
    heading: &HeadingField,
    continent: &dyn Fn(i32, i32) -> u8,
) -> BTreeSet<(i32, i32)> {
    let has_types =
        DecalCategory::of_name("Crops").is_some_and(|c| !layer.types().of(c).is_empty());
    let decals = if has_types {
        farm_decals(farms, width, heading, continent)
    } else {
        Vec::new()
    };
    let cells: Vec<u32> = touched
        .iter()
        .map(|&(x, y)| (y * width + x) as u32)
        .collect();
    let made: BTreeSet<u32> = decals.iter().map(|d| d.cell).collect();
    layer.set_object_decals(DecalSource::Farms, &cells, decals);
    farms
        .iter()
        .map(|f| (f.x, f.y))
        .filter(|&(x, y)| made.contains(&((y * width + x) as u32)))
        .collect()
}

/// The types of the `Crops` category whose name starts with a group of `farm_types.xml`.
pub fn group_types<'a>(types: &'a [DecalType], group: &str) -> Vec<&'a DecalType> {
    types
        .iter()
        .filter(|t| t.name.split('/').next() == Some(group))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::terrain::decals::DecalShape;

    fn farm(x: i32, y: i32, state: FarmState, wheat: bool) -> Farm {
        Farm { x, y, state, wheat }
    }

    fn layer() -> DecalLayer {
        let mut l = DecalLayer::new();
        l.set_types(&with_field_types(&[DecalDef {
            kind: "Crops".into(),
            name: "Crops_01".into(),
            albedo: None,
            heightmap: None,
            priority: Some(0.5),
            extra: Default::default(),
        }]));
        l
    }

    #[test]
    fn the_crops_category_gets_every_texture_of_farm_types() {
        let l = layer();
        let types = l.types().of(DecalCategory::of_name("Crops").unwrap());
        assert_eq!(types.len(), 8 * 5 + 3);
        let wheat = group_types(types, "Crops_Wheat");
        assert_eq!(wheat.len(), 1);
        assert_eq!(
            wheat[0].albedo.as_ref().unwrap().as_str(),
            "tx:wheat_farm_d.dds"
        );
        assert_eq!(
            wheat[0].heightmap.as_ref().unwrap().as_str(),
            "tx:wheat_farm_h.dds"
        );
        let africa = group_types(types, "Crops_Africa");
        assert_eq!(
            africa[2].heightmap.as_ref().unwrap().as_str(),
            "tx:crops_generic_02_h.dds"
        );
        let damage = group_types(types, "Crops_Damage");
        assert_eq!(damage[0].priority, 0.51);
        assert!(damage[0].heightmap.is_none());
    }

    #[test]
    fn the_group_comes_from_the_continent_the_state_and_the_wheat() {
        let built = farm(0, 0, FarmState::Constructed, false);
        assert_eq!(farm_group(&built, 1), Some(2)); // America
        assert_eq!(farm_group(&built, 2), Some(3)); // Asia
        assert_eq!(farm_group(&built, 3), Some(1)); // Africa
        assert_eq!(farm_group(&built, 4), Some(4)); // Europe
        assert_eq!(farm_group(&built, 0), None);
        let hb = farm(0, 0, FarmState::UnderConstruction, false);
        assert_eq!(farm_group(&hb, 4), Some(HALF_BUILT));
        let wheat = farm(0, 0, FarmState::Constructed, true);
        assert_eq!(farm_group(&wheat, 3), Some(WHEAT_GROUP));
        let wheat_hb = farm(0, 0, FarmState::UnderConstruction, true);
        assert_eq!(farm_group(&wheat_hb, 3), Some(WHEAT_HB));
        // Wheat on a hex of no continent art has no field either.
        assert_eq!(farm_group(&wheat, 0), None);
    }

    fn field() -> HeadingField {
        HeadingField::new(&crate::world::terrain::TerrainFrame::new(10, 8, false))
    }

    #[test]
    fn the_fields_of_a_hex_are_cells_of_the_streamline_spacing_that_cover_it() {
        let h = field();
        let cells = field_cells(5, 4, &h);
        assert!(cells.len() >= 12, "{} fields", cells.len());
        for c in &cells {
            assert!((c.area() - FIELD_SIDE * FIELD_SIDE).abs() < 1e-2);
            assert_eq!(nearest_cell(c.centroid()), (5, 4));
        }
        let area: f32 = cells.iter().map(FieldCell::area).sum();
        // The area of the hex: 64 * 55.43.
        let hex = HEX_WIDTH * crate::hexgrid::ROW_STEP;
        assert!((area / hex - 1.0).abs() < 0.3, "area {area} of {hex}");
    }

    #[test]
    fn the_fields_run_along_the_heading_of_their_hex() {
        let h = field();
        for (x, y) in [(1, 1), (5, 4), (8, 6)] {
            let a = h.angle(cell_center(x, y));
            let edge = field_cells(x, y, &h)[0].corners;
            let along = edge[1] - edge[0];
            // Our Z is the original's -y.
            let got = (-along.y).atan2(along.x);
            assert!((got - a).abs() < 1e-4, "{got} vs {a}");
        }
        // The heading is not one direction over the map.
        let a = h.angle(cell_center(0, 0));
        assert!((0..10).any(|x| (h.angle(cell_center(x, 7)) - a).abs() > 0.05));
    }

    #[test]
    fn the_texture_is_laid_in_the_world_at_its_period_along_the_heading() {
        let h = field();
        let cells = field_cells(2, 2, &h);
        let mut turned = 0;
        for c in &cells {
            let uv = field_uvs(c, &h);
            let world = (c.corners[1] - c.corners[0]).length();
            let d = uv[1] - uv[0];
            assert!((d.length() - world / TEXTURE_PERIOD).abs() < 1e-4);
            // The edge along the heading of the hex is along u, or along v for a field turned a
            // quarter, up to the turn of the heading between the hex centre and the field.
            let off = (h.angle(c.centroid()) - h.angle(cell_center(2, 2))).abs() + 1e-3;
            if d.y.abs() > d.x.abs() {
                turned += 1;
                assert!((d.x / d.y).atan().abs() < off);
            } else {
                assert!((d.y / d.x).atan().abs() < off);
            }
        }
        assert!(
            turned > 0 && turned < cells.len(),
            "{turned} of {}",
            cells.len()
        );
    }

    #[test]
    fn the_farm_model_stands_on_the_field_nearest_the_centre_turned_by_the_heading() {
        let h = field();
        let mut quarters = std::collections::BTreeSet::new();
        for x in 0..8 {
            let (anchor, yaw) = farm_anchor(x, 3, &h);
            let c = cell_center(x, 3);
            let fields = field_cells(x, 3, &h);
            assert!(fields.iter().any(|f| f.centroid() == anchor));
            let best = fields
                .iter()
                .map(|f| f.centroid().distance(c))
                .fold(f32::MAX, f32::min);
            assert!((anchor.distance(c) - best).abs() < 1e-4);
            let k = (yaw - h.angle(anchor)) / std::f32::consts::FRAC_PI_2;
            assert!((k - k.round()).abs() < 1e-4 && (0.0..4.0).contains(&k.round()));
            quarters.insert(k.round() as i32);
        }
        assert!(quarters.len() > 1);
    }
    #[test]
    fn a_pillaged_farm_has_the_damage_over_its_fields_and_wheat_is_yellow() {
        let europe = |_: i32, _: i32| 4u8;
        let d = farm_decals(
            &[farm(3, 2, FarmState::Constructed, true)],
            10,
            &field(),
            &europe,
        );
        assert!(!d.is_empty());
        assert!(
            d.iter()
                .all(|x| x.cell == 23
                    && x.texture.as_ref().unwrap().as_str() == "tx:wheat_farm_d.dds")
        );
        let p = farm_decals(
            &[farm(3, 2, FarmState::Pillaged, false)],
            10,
            &field(),
            &europe,
        );
        let damaged = p
            .iter()
            .filter(|x| x.texture.as_ref().unwrap().as_str() == "tx:crops_destroyed_01_d.dds")
            .count();
        assert_eq!(damaged * 2, p.len());
        assert!(matches!(p[0].shape, DecalShape::Mesh { .. }));
    }

    #[test]
    fn the_layer_gets_the_fields_with_the_type_of_their_texture_and_loses_a_gone_farm() {
        let mut l = layer();
        let asia = |_: i32, _: i32| 2u8;
        let mut touched: BTreeSet<(i32, i32)> = [(1, 1), (2, 1)].into();
        let farms = [
            farm(1, 1, FarmState::Constructed, false),
            farm(2, 1, FarmState::Constructed, false),
        ];
        let now = apply(&mut l, &farms, &touched, 8, &field(), &asia);
        assert_eq!(now.len(), 2);
        let n = l.len();
        assert!(n >= 10);
        for d in l.iter() {
            let a = d.albedo.as_ref().unwrap().as_str();
            assert!(a.starts_with("tx:crops_asia_"), "{a}");
            assert!(
                d.height
                    .as_ref()
                    .unwrap()
                    .as_str()
                    .starts_with("tx:crops_generic_")
            );
            assert_eq!(d.priority, 0.5);
        }
        touched.insert((1, 1));
        let now = apply(&mut l, &farms[..1], &touched, 8, &field(), &asia);
        assert_eq!(now.len(), 1);
        assert!(l.len() < n);
        // A hex of no continent art: no field.
        let none = |_: i32, _: i32| 0u8;
        assert!(apply(&mut l, &farms, &touched, 8, &field(), &none).is_empty());
        assert!(l.is_empty());
    }
}
