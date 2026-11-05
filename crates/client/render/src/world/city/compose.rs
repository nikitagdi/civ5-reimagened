//! What a city is made of (world-objects 4.4, WOB5): the models of the catalog of the owner's
//! culture and era on the lots of [`super::layout`], and how the damage and the ruins change them.
//!
//! A catalog (`Euro_Anc_City.gr2`) is twenty models, each a flat ground quad (`LandmarkShader`)
//! and one house (`BuildingShader`) whose vertices are centred on the origin; the `InitialPlacement`
//! of a model is where the artist laid it out in a row (thousands of units away) and is not applied.
//! `plan` gives each lot a model of its height class at random (`SelectBuilding`, 0x088acdc0: the
//! set is `culture * 4 + era`, `rand() % k` among its nodes of the class), turned to the heading of
//! the lot (`cvCityVisSystem::UpdateFromHandle(AddBuilding)`, 0x088568c6). One ground quad, the
//! widest, lies under the whole city.
//!
//! SIMPLIFIED(SIMP-021): the share of the houses that the damage turns into ruins is ours.

use asset_format::{ModelBlobV2, Transform};
use glam::{Mat4, Vec2, Vec3};

use super::layout;
use super::msvc::MsvcRand;

/// One model of a catalog: its ground and its building.
#[derive(Debug, Clone, PartialEq)]
pub struct Lot {
    /// The index of the ground quad among the meshes of the model (the draw's part, see
    /// [`split_by_mesh`]).
    pub ground: Option<u16>,
    /// The building.
    pub building: Option<u16>,
    /// The lot's middle (of its ground quad, else of its building) to the origin. The vertices of
    /// the real catalogs are centred already (this is about the identity); the part's
    /// `InitialPlacement` is the place in the artist's row and is not part of it.
    pub placement: Mat4,
    /// Half of the longer side of the building, catalog units.
    pub radius: f32,
    pub height: f32,
    /// The shorter side of the ground quad.
    pub base_side: f32,
    /// The sides of the building box: width (x) and depth (z).
    pub size: Vec2,
}

/// What the placement of a city needs of a model, kept when it arrives.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModelMeta {
    pub lots: Vec<Lot>,
    /// The middle of the box of the model on the ground, and the box.
    pub centre: Vec2,
    pub lo: Vec3,
    pub hi: Vec3,
}

fn is_ground(blob: &ModelBlobV2, mesh: &asset_format::MeshV2) -> bool {
    blob.materials
        .get(usize::from(mesh.material))
        .is_some_and(|m| m.package.starts_with("Landmark"))
}

/// The box of the positions of a mesh in the frame of `m`.
fn bounds_in(mesh: &asset_format::MeshV2, m: &Mat4) -> Option<(Vec3, Vec3)> {
    let pos = mesh.positions()?;
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for p in &pos {
        let v = m.transform_point3(Vec3::from(*p));
        lo = lo.min(v);
        hi = hi.max(v);
    }
    (!pos.is_empty()).then_some((lo, hi))
}

impl ModelMeta {
    /// Reads the lots of a model: a part is a lot, its meshes are the ground (a quad of the
    /// `LandmarkShader`) and the building (the rest).
    pub fn of(blob: &ModelBlobV2) -> Self {
        let mut lots = Vec::new();
        for part in &blob.parts {
            let first = usize::from(part.first_mesh);
            let meshes = first..first + usize::from(part.mesh_count);
            // The middle of the lot in the space of its vertices: the ground quad's, else the box's.
            let mut ground_box = None;
            let mut any_box = None;
            for k in meshes.clone() {
                let Some(mesh) = blob.meshes.get(k) else {
                    continue;
                };
                let Some(b) = bounds_in(mesh, &Mat4::IDENTITY) else {
                    continue;
                };
                if is_ground(blob, mesh) && ground_box.is_none() {
                    ground_box = Some(b);
                }
                any_box.get_or_insert(b);
            }
            let middle = ground_box
                .or(any_box)
                .map_or(Vec3::ZERO, |(lo, hi)| (lo + hi) * 0.5);
            let placement = Mat4::from_translation(Vec3::new(-middle.x, 0.0, -middle.z));
            let mut lot = Lot {
                ground: None,
                building: None,
                placement,
                radius: 0.0,
                height: 0.0,
                base_side: 0.0,
                size: Vec2::ZERO,
            };
            for k in meshes {
                let Some(mesh) = blob.meshes.get(k) else {
                    continue;
                };
                let Some((lo, hi)) = bounds_in(mesh, &placement) else {
                    continue;
                };
                if is_ground(blob, mesh) {
                    if lot.ground.is_none() {
                        lot.ground = Some(k as u16);
                        lot.base_side = (hi.x - lo.x).min(hi.z - lo.z);
                    }
                } else if lot.building.is_none() {
                    lot.building = Some(k as u16);
                    lot.size = Vec2::new(hi.x - lo.x, hi.z - lo.z);
                    lot.radius = 0.5 * lot.size.max_element();
                    lot.height = hi.y;
                }
            }
            if lot.ground.is_some() || lot.building.is_some() {
                lots.push(lot);
            }
        }
        let [lx, ly, lz, hx, hy, hz] = blob.bounds;
        Self {
            lots,
            centre: Vec2::new((lx + hx) * 0.5, (lz + hz) * 0.5),
            lo: Vec3::new(lx, ly, lz),
            hi: Vec3::new(hx, hy, hz),
        }
    }

    /// The lot with the widest ground quad: the ground of the whole city.
    pub fn ground_lot(&self) -> Option<usize> {
        self.lots
            .iter()
            .enumerate()
            .filter(|(_, l)| l.ground.is_some())
            .max_by(|(ia, a), (ib, b)| a.base_side.total_cmp(&b.base_side).then(ib.cmp(ia)))
            .map(|(i, _)| i)
    }
}

/// The model with every mesh a part of its own: the part index is the mesh index, so the draw can
/// take the ground and the building of a lot apart (`PartBatches` key (model, part)). The
/// placements stay with the meshes.
pub fn split_by_mesh(blob: &ModelBlobV2) -> ModelBlobV2 {
    let mut out = blob.clone();
    out.parts = Vec::with_capacity(blob.meshes.len());
    for (i, mesh) in out.meshes.iter_mut().enumerate() {
        let old = blob.parts.get(usize::from(mesh.part));
        out.parts.push(asset_format::Part {
            name: match old {
                Some(p) => format!("{}:{}", p.name, mesh.name),
                None => mesh.name.clone(),
            },
            first_mesh: i as u16,
            mesh_count: 1,
            placement: old.map_or(Transform::IDENTITY, |p| p.placement),
        });
        mesh.part = i as u16;
    }
    out
}

/// One house of a city in its place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Chosen {
    /// The index in the lots of the catalog (or of the ruins when `ruined`).
    pub lot: usize,
    /// The index of the house in the layout.
    pub slot: usize,
    /// Catalog units from the middle of the hex (ours: x east, y = Z).
    pub at: Vec2,
    /// The turn about the vertical, radians.
    pub yaw: f32,
    pub ruined: bool,
    /// The burn rank of the house (`layout::RANK_NONE`: never).
    pub rank: u8,
}

/// The height classes 0..3 of the buildings of a catalog: the quarters of the lots by height.
// SIMPLIFIED(SIMP-021): the original reads the class of each model node of the catalog
// (`cvBuildingNode::m_ucHeight`, set by `cvPopBuildingSPICreator`, not read); ours the quarters by
// the height of the building.
pub fn height_classes(catalog: &ModelMeta) -> [Vec<usize>; 4] {
    let mut by_height: Vec<usize> = (0..catalog.lots.len())
        .filter(|&i| catalog.lots[i].building.is_some())
        .collect();
    by_height.sort_by(|&a, &b| {
        catalog.lots[a]
            .height
            .total_cmp(&catalog.lots[b].height)
            .then(a.cmp(&b))
    });
    let n = by_height.len();
    std::array::from_fn(|c| by_height[c * n / 4..(c + 1) * n / 4].to_vec())
}

/// The houses of a city on the lots of its layout. `centre` is the middle of the hex in the
/// original's plane; `rand` is the C library state of the city: `SelectBuilding` draws
/// `rand() % k` among the models of the class for each house, in the order of the layout.
/// `hp_frac` is the health of the city (1 whole): the later houses of a damaged city are ruins, a
/// share of `0.6 * (1 - hp_frac)`.
pub fn plan(
    catalog: &ModelMeta,
    ruins: Option<&ModelMeta>,
    houses: &[layout::House],
    centre: Vec2,
    hp_frac: f32,
    rand: &mut MsvcRand,
) -> Vec<Chosen> {
    let classes = height_classes(catalog);
    // `m_afRandom`: 33 quarter turns `rand() % 4` drawn once (`cvCityVisSystem::Startup`), taken in
    // turn by the buildings (`m_uRandomIndex`) and added to the heading (`FUN_088587b8`).
    // SIMPLIFIED(SIMP-021): the table is drawn per city from its own state, not once per game.
    let mut table_rand = MsvcRand::new(rand.rand() ^ 0x5f3d);
    let quarters: [f32; 0x21] = std::array::from_fn(|_| (table_rand.rand() % 4) as f32);
    let mut out: Vec<Chosen> = Vec::new();
    for (k, h) in houses.iter().enumerate() {
        // An empty class takes the nearest lower one, then the nearest higher one.
        let class = usize::from(h.class.min(3));
        let Some(pool) = (0..=class)
            .rev()
            .chain(class + 1..4)
            .map(|c| &classes[c])
            .find(|p| !p.is_empty())
        else {
            break;
        };
        let lot = pool[rand.rand() as usize % pool.len()];
        let at = layout::from_orig(h.pos - centre) / layout::CITY_SCALE;
        out.push(Chosen {
            lot,
            slot: k,
            at,
            yaw: h.dir.y.atan2(h.dir.x) + quarters[k % 0x21] * std::f32::consts::FRAC_PI_2,
            ruined: false,
            rank: h.rank,
        });
    }
    // The damage: the houses that came last turn into ruins, a ruin no wider than the house.
    let room = |m: &ModelMeta, i: usize| m.lots[i].radius;
    if let Some(ruins) = ruins {
        let share = (0.6 * (1.0 - hp_frac.clamp(0.0, 1.0))).clamp(0.0, 1.0);
        let k = (out.len() as f32 * share).floor() as usize;
        let n = out.len();
        let mut by_room: Vec<usize> = (0..ruins.lots.len())
            .filter(|&i| ruins.lots[i].building.is_some())
            .collect();
        by_room.sort_by(|&a, &b| room(ruins, a).total_cmp(&room(ruins, b)));
        for (j, c) in out[n - k..].iter_mut().enumerate() {
            let limit = room(catalog, c.lot) * 1.25;
            let fitting: Vec<usize> = by_room
                .iter()
                .copied()
                .filter(|&i| room(ruins, i) <= limit)
                .collect();
            let fitting = if fitting.is_empty() {
                by_room.get(..1).unwrap_or(&[]).to_vec()
            } else {
                fitting
            };
            if fitting.is_empty() {
                continue;
            }
            let pick = fitting[crate::world::landmark::select::mix(c.slot as u32, 0x20 + j as u32)
                as usize
                % fitting.len()];
            *c = Chosen {
                lot: pick,
                ruined: true,
                ..*c
            };
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn lot(radius: f32, height: f32, base: f32) -> Lot {
        Lot {
            ground: Some(0),
            building: Some(1),
            placement: Mat4::IDENTITY,
            radius,
            height,
            base_side: base,
            size: Vec2::splat(radius * 2.0),
        }
    }

    /// Twenty lots of every size, like a real catalog.
    pub fn catalog() -> ModelMeta {
        ModelMeta {
            lots: (0..20)
                .map(|i| {
                    lot(
                        30.0 + 7.0 * (i % 10) as f32,
                        100.0 + 9.0 * i as f32,
                        100.0 + i as f32 * 10.0,
                    )
                })
                .collect(),
            ..ModelMeta::default()
        }
    }

    fn houses(n: usize) -> Vec<layout::House> {
        (0..n)
            .map(|i| layout::House {
                pos: Vec2::new(100.0 + i as f32 * 4.0, 50.0),
                dir: Vec2::new(0.0, 0.5),
                class: (i % 4) as u8,
                rank: if i < 3 { 0 } else { layout::RANK_NONE },
            })
            .collect()
    }

    #[test]
    fn each_house_takes_a_model_of_its_height_class() {
        let c = catalog();
        let classes = height_classes(&c);
        assert!(classes.iter().all(|k| k.len() == 5));
        let p = plan(
            &c,
            None,
            &houses(12),
            Vec2::new(100.0, 50.0),
            1.0,
            &mut MsvcRand::new(1),
        );
        assert_eq!(p.len(), 12);
        for (i, h) in p.iter().enumerate() {
            assert!(classes[i % 4].contains(&h.lot), "{i}");
            // The heading (0, 1) is a quarter turn, then whole quarter turns of the table.
            let q = (h.yaw / std::f32::consts::FRAC_PI_2).round();
            assert!((h.yaw - q * std::f32::consts::FRAC_PI_2).abs() < 1e-5);
            assert!((1.0..=4.0).contains(&q), "{q}");
        }
        assert_eq!(p[1].at, Vec2::new(4.0 / layout::CITY_SCALE, 0.0));
        assert_eq!(p[0].rank, 0);
        // The same state, the same picks.
        let q = plan(
            &c,
            None,
            &houses(12),
            Vec2::new(100.0, 50.0),
            1.0,
            &mut MsvcRand::new(1),
        );
        assert_eq!(p, q);
    }

    #[test]
    fn the_damage_turns_the_last_houses_into_ruins() {
        let c = catalog();
        let ruins = ModelMeta {
            lots: vec![lot(40.0, 60.0, 400.0), lot(60.0, 80.0, 400.0)],
            ..ModelMeta::default()
        };
        let at = Vec2::new(100.0, 50.0);
        let whole = plan(
            &c,
            Some(&ruins),
            &houses(20),
            at,
            1.0,
            &mut MsvcRand::new(3),
        );
        assert!(whole.iter().all(|c| !c.ruined));
        let hurt = plan(
            &c,
            Some(&ruins),
            &houses(20),
            at,
            0.2,
            &mut MsvcRand::new(3),
        );
        let ruined = hurt.iter().filter(|c| c.ruined).count();
        assert_eq!(ruined, (hurt.len() as f32 * 0.48).floor() as usize);
        assert_eq!(hurt[..hurt.len() - ruined], whole[..hurt.len() - ruined]);
    }

    #[test]
    fn the_ground_of_the_city_is_the_widest_quad() {
        let mut m = catalog();
        assert_eq!(m.ground_lot(), Some(19));
        m.lots[3].base_side = 5000.0;
        assert_eq!(m.ground_lot(), Some(3));
        assert_eq!(ModelMeta::default().ground_lot(), None);
    }

    /// The real catalogs: the vertices of a lot are centred, the part's placement is far off in the
    /// artist's row. The lot stays where its vertices are.
    #[test]
    fn the_initial_placement_of_a_catalog_part_is_not_applied() {
        use asset_format::{MaterialDesc, MeshArrays, MeshV2, Part};
        let quad = |material: u16, half: f32, height: f32| {
            let a = MeshArrays {
                positions: vec![
                    [-half, height, -half],
                    [half, height, -half],
                    [half, 0.0, half],
                    [-half, 0.0, half],
                ],
                normals: vec![[0.0, 1.0, 0.0]; 4],
                uvs: vec![[0.0, 0.0]; 4],
                joints: vec![],
                weights: vec![],
                indices: vec![0, 1, 2, 0, 2, 3],
            };
            MeshV2::pack("m", 0, material, &a, false, false).unwrap()
        };
        let blob = ModelBlobV2 {
            parts: vec![Part {
                name: "model0".into(),
                first_mesh: 0,
                mesh_count: 2,
                placement: Transform {
                    translation: [1278.0, 0.0, 258.0],
                    ..Transform::IDENTITY
                },
            }],
            meshes: vec![quad(0, 175.0, 0.0), quad(1, 55.0, 180.0)],
            materials: ["LandmarkShader", "BuildingShader"]
                .map(|p| MaterialDesc {
                    package: p.into(),
                    ..MaterialDesc::default()
                })
                .into(),
            ..ModelBlobV2::default()
        };
        let m = ModelMeta::of(&blob);
        let at = m.lots[0].placement.transform_point3(Vec3::ZERO);
        assert!(at.length() < 1e-3, "{at:?}");
        assert!((m.lots[0].radius - 55.0).abs() < 1e-3);
    }
}
