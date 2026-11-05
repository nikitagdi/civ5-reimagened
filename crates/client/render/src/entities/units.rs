//! Units: squads of static figures at the unit's cell, a marker when there is no model.
//!
//! SIMPLIFIED(SIMP-010): static pose, instant moves, military/civilian slot offsets of our own.

use std::collections::HashMap;

use asset_format::{ArtTable, LogicalId, UnitArt};
use glam::{Vec2, Vec3};

use super::formation::{self, ALONE_HALF_EXTENT, SHARED_HALF_EXTENT};
use super::labels::{LABEL_HEIGHT, LabelData, UnitFlag, WorldLabel};
use super::{
    ArtState, EntityPlan, EntitySnapshot, GLOBAL_UNIT_SCALE, ModelRef, ModelStatus, Owners,
    PlanContext, UnitEntity, fog_of, instance, srgb_tint,
};
use crate::hexgrid::cell_center;
use crate::static_model::{StaticInstance, pack_team};
use crate::terrain::{Fog, terrain_height_at};

/// X offset of a military unit that shares its cell with a civilian one, and of the civilian.
// SIMPLIFIED(SIMP-010): offsets chosen by us, not read from the original
pub const MILITARY_OFFSET: f32 = -12.0;
pub const CIVILIAN_OFFSET: f32 = 12.0;

/// Scale of a marker (the model is authored at its final size).
const MARKER_SCALE: f32 = 1.0;

/// Percent of damage of a unit: 0 at full health, 100 when dead.
pub fn damage_pct(hp: i32, max_hp: i32) -> u32 {
    if max_hp <= 0 {
        return 0;
    }
    let lost = (max_hp - hp.clamp(0, max_hp)) as i64;
    (lost * 100 / i64::from(max_hp)) as u32
}

/// Figures alive of a squad of `total` at `damage_pct` percent of damage.
// re: `UpdateFromHandle(SetHitPct)` 0x08804c25-57 (RV-7): r = (pct * n + 50) / 100;
// dead = (pct == 100 || r < n) ? r : n - 1. The figures with the larger indices die first.
pub fn alive_figures(total: usize, damage_pct: u32) -> usize {
    let pct = damage_pct.min(100) as usize;
    let r = (pct * total + 50) / 100;
    let dead = if pct == 100 || r < total {
        r
    } else {
        total.saturating_sub(1)
    };
    total - dead.min(total)
}

/// Which side of its cell a unit stands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Alone,
    Military,
    Civilian,
}

impl Slot {
    pub fn offset_x(self) -> f32 {
        match self {
            Self::Alone => 0.0,
            Self::Military => MILITARY_OFFSET,
            Self::Civilian => CIVILIAN_OFFSET,
        }
    }
}

/// The art of a unit for its owner's culture.
pub fn art_of<'a>(
    table: &'a ArtTable,
    u: &UnitEntity,
    suffix: Option<&str>,
) -> Option<&'a UnitArt> {
    table.unit(&u.type_name).map(|r| r.for_style(suffix))
}

/// One figure of a squad: model and scale, in figure order (the order the table lists them).
fn figures(art: &UnitArt) -> Vec<(&LogicalId, f32)> {
    art.figures
        .iter()
        .flat_map(|f| std::iter::repeat_n((&f.model, f.scale), f.count as usize))
        .collect()
}

/// Which kinds stand on a cell: (a military unit, a civilian one).
pub type CellKinds = (bool, bool);

/// What one unit adds to the plan: its flag, its instances, the models it waits for. WOB0 keeps
/// one per unit key, so a change of one unit recomputes only its own.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UnitPlaced {
    pub label: Option<WorldLabel>,
    pub items: Vec<(ModelRef, StaticInstance)>,
    /// Models of the squad that are still coming ([`EntityPlan::wanted`]).
    pub wanted: Vec<LogicalId>,
    /// Models that are figures of this unit ([`EntityPlan::unit_models`]).
    pub unit_models: Vec<LogicalId>,
    /// Every model whose arrival or failure changes this placement.
    pub deps: Vec<LogicalId>,
}

impl UnitPlaced {
    /// Adds this unit to `plan`.
    pub fn add_to(&self, plan: &mut EntityPlan) {
        plan.labels.extend(self.label.iter().cloned());
        plan.unit_models.extend(self.unit_models.iter().cloned());
        plan.wanted.extend(self.wanted.iter().cloned());
        for (model, instance) in &self.items {
            plan.push(model.clone(), *instance);
        }
    }
}

/// Whether the unit shares its cell with one of the other kind.
pub fn is_shared(kinds: CellKinds) -> bool {
    kinds.0 && kinds.1
}

/// Where one unit stands and what it draws: a flag on a visible cell, then its squad
/// by the formation, or a pillar. `shared` says a military and a civilian unit share the cell.
pub fn place_unit(
    u: &UnitEntity,
    shared: bool,
    owners: &Owners,
    ctx: &PlanContext<'_>,
) -> UnitPlaced {
    let mut out = UnitPlaced::default();
    if matches!(ctx.art, ArtState::Loading) {
        return out;
    }
    let Some((cell, Fog::Visible)) = fog_of(ctx.terrain, u.x, u.y) else {
        return out;
    };
    let slot = match (shared, u.civilian) {
        (false, _) => Slot::Alone,
        (true, false) => Slot::Military,
        (true, true) => Slot::Civilian,
    };
    let centre = cell_center(u.x, u.y) + Vec2::new(slot.offset_x(), 0.0);
    let ground = terrain_height_at(ctx.terrain, centre);
    out.label = Some(WorldLabel {
        cell: (u.x, u.y),
        position: Vec3::new(centre.x, ground + LABEL_HEIGHT, centre.y),
        data: LabelData::UnitFlag(UnitFlag {
            unit: u.id,
            owner: u.owner,
            type_name: u.type_name.clone(),
            hp: u.hp,
            max_hp: u.max_hp,
            needs_orders: u.needs_orders,
            civilian: u.civilian,
        }),
    });

    let model_art = match ctx.art {
        ArtState::Ready(table) => art_of(table, u, owners.culture_of(u.owner).map(|c| c.suffix()))
            .filter(|a| !a.placeholder)
            .map(|a| (table, a)),
        _ => None,
    };
    let squad = model_art.and_then(|(table, art)| {
        let list = figures(art);
        (!list.is_empty()).then_some((table, art, list))
    });
    let Some((table, art, list)) = squad else {
        marker(owners, u, centre, ground, cell, &mut out);
        return out;
    };
    out.unit_models = list.iter().map(|(m, _)| (*m).clone()).collect();
    out.deps = out.unit_models.clone();
    // Any figure whose model failed turns the whole unit into a marker; one that is still
    // coming keeps the unit out of the picture until it is here.
    let mut statuses = list.iter().map(|(m, _)| (ctx.status)(m));
    if statuses.clone().any(|s| s == ModelStatus::Failed) {
        marker(owners, u, centre, ground, cell, &mut out);
        return out;
    }
    if statuses.any(|s| s == ModelStatus::Pending) {
        out.wanted = list
            .iter()
            .filter(|(m, _)| (ctx.status)(m) == ModelStatus::Pending)
            .map(|(m, _)| (*m).clone())
            .collect();
        return out;
    }
    let params = formation::Params::of_name(art.formation.as_deref(), |n| table.formation(n));
    // The layout is made for the whole squad so the living figures stay where they were.
    let mut offsets = formation::generate(list.len(), list.len(), &params, u.id);
    formation::fit(
        &mut offsets,
        if shared {
            SHARED_HALF_EXTENT
        } else {
            ALONE_HALF_EXTENT
        },
    );
    let alive = alive_figures(list.len(), damage_pct(u.hp, u.max_hp));
    for ((model, scale), off) in list.iter().zip(&offsets).take(alive) {
        let p = centre + *off;
        let h = terrain_height_at(ctx.terrain, p);
        let mut i = instance(Vec3::new(p.x, h, p.y), scale * GLOBAL_UNIT_SCALE, cell);
        // `TeamColorR` of the ported `UnitShader`.
        i.team = pack_team(owners.team_color_of(u.owner));
        out.items.push((ModelRef::Asset((*model).clone()), i));
    }
    out
}

pub(super) fn place_units(snapshot: &EntitySnapshot, ctx: &PlanContext<'_>, plan: &mut EntityPlan) {
    if matches!(ctx.art, ArtState::Loading) {
        return;
    }
    let owners = snapshot.owners();
    // Which kinds stand on each cell.
    let mut kinds: HashMap<(i32, i32), CellKinds> = HashMap::new();
    for u in &snapshot.units {
        let k = kinds.entry((u.x, u.y)).or_default();
        if u.civilian {
            k.1 = true;
        } else {
            k.0 = true;
        }
    }
    let mut units: Vec<&UnitEntity> = snapshot.units.iter().collect();
    units.sort_by_key(|u| u.id);
    for u in units {
        place_unit(u, is_shared(kinds[&(u.x, u.y)]), &owners, ctx).add_to(plan);
    }
}

/// A pillar in the owner's colour (SIMP-011).
// SIMPLIFIED(SIMP-011): a plain pillar instead of the unit's model (no SV icon on it yet)
fn marker(
    owners: &Owners,
    u: &UnitEntity,
    centre: Vec2,
    ground: f32,
    cell: usize,
    out: &mut UnitPlaced,
) {
    let mut i = instance(Vec3::new(centre.x, ground, centre.y), MARKER_SCALE, cell);
    i.tint = srgb_tint(owners.color_of(u.owner));
    out.items.push((ModelRef::UnitMarker, i));
}

#[cfg(test)]
pub(crate) mod tests {
    use asset_format::{Figure, UnitRecord, UnitVariant};

    use super::*;
    use crate::entities::{Culture, EntityPlayer};
    use crate::hexgrid::cell_center;
    use crate::terrain::test_util::{land, map};
    use crate::terrain::{Relief, TerrainCell, TerrainSnapshot};

    pub fn art(tag: &str, count: u32, scale: f32, placeholder: bool) -> UnitArt {
        UnitArt {
            art_info: format!("ART_{tag}"),
            figures: vec![Figure {
                model: LogicalId::new(format!("model:{tag}")),
                count,
                scale,
                z_offset: 0.0,
            }],
            formation: None,
            sv_icon: None,
            placeholder,
        }
    }

    pub fn table() -> ArtTable {
        let mut t = ArtTable::new();
        t.insert_unit(
            "UNIT_WARRIOR",
            UnitRecord {
                base: art("warrior", 12, 0.14, false),
                variants: vec![UnitVariant {
                    suffix: "_ASIA".into(),
                    art: art("warrior_asia", 12, 0.14, false),
                }],
            },
        );
        t.insert_unit(
            "UNIT_SETTLER",
            UnitRecord {
                base: art("settler", 3, 0.12, false),
                variants: vec![],
            },
        );
        t.insert_unit(
            "UNIT_STUB",
            UnitRecord {
                base: art("stub", 1, 0.14, true),
                variants: vec![],
            },
        );
        t
    }

    pub fn terrain() -> TerrainSnapshot {
        map(6, 6, |x, y| match (x, y) {
            (4, 4) => TerrainCell {
                relief: Relief::Hill,
                ..land()
            },
            (5, 5) => TerrainCell {
                fog: Fog::Hidden,
                ..land()
            },
            (0, 5) => TerrainCell::default(),
            _ => land(),
        })
    }

    pub fn unit(id: i32, ty: &str, x: i32, y: i32, civilian: bool) -> UnitEntity {
        UnitEntity {
            id,
            owner: 0,
            type_name: ty.into(),
            x,
            y,
            hp: 100,
            max_hp: 100,
            civilian,
            needs_orders: false,
        }
    }

    pub fn snapshot(units: Vec<UnitEntity>) -> EntitySnapshot {
        EntitySnapshot {
            players: vec![
                EntityPlayer {
                    id: 0,
                    color: [200, 30, 30],
                    team_color: [200, 30, 30],
                    culture: None,
                    era: None,
                    industrial: false,
                    art_techs: 0,
                    era_index: None,
                },
                EntityPlayer {
                    id: 1,
                    color: [30, 30, 200],
                    team_color: [30, 30, 200],
                    culture: Some(Culture::Asia),
                    era: None,
                    industrial: false,
                    art_techs: 0,
                    era_index: None,
                },
            ],
            units,
            ..EntitySnapshot::default()
        }
    }

    pub fn plan_with(
        s: &EntitySnapshot,
        art: &ArtState,
        status: &dyn Fn(&LogicalId) -> ModelStatus,
    ) -> EntityPlan {
        crate::entities::build_plan(
            s,
            &PlanContext {
                terrain: &terrain(),
                art,
                status,
            },
        )
    }

    pub fn ready(_: &LogicalId) -> ModelStatus {
        ModelStatus::Ready
    }

    fn asset(plan: &EntityPlan, id: &str) -> usize {
        plan.batches
            .get(&ModelRef::Asset(LogicalId::new(id)))
            .map_or(0, Vec::len)
    }

    #[test]
    fn hp_to_figures_formula_on_all_values() {
        for n in [1usize, 2, 3, 5, 12] {
            assert_eq!(alive_figures(n, 0), n);
            assert_eq!(alive_figures(n, 100), 0, "dead at 100, n={n}");
            let mut last = n;
            for pct in 0..100u32 {
                let a = alive_figures(n, pct);
                assert!(a >= 1, "n={n} pct={pct}");
                assert!(a <= last, "monotonic, n={n} pct={pct}");
                last = a;
                let r = (pct as usize * n + 50) / 100;
                let dead = if r < n { r } else { n - 1 };
                assert_eq!(a, n - dead);
            }
        }
        // 12 figures: half dead at 50 percent.
        assert_eq!(alive_figures(12, 50), 6);
        assert_eq!(alive_figures(1, 99), 1);
        assert_eq!(alive_figures(0, 50), 0);
    }

    #[test]
    fn damage_follows_hp() {
        assert_eq!(damage_pct(100, 100), 0);
        assert_eq!(damage_pct(40, 100), 60);
        assert_eq!(damage_pct(0, 100), 100);
        assert_eq!(damage_pct(5, 0), 0);
        assert_eq!(damage_pct(150, 100), 0);
    }

    #[test]
    fn a_squad_has_the_table_count_inside_its_hex() {
        let s = snapshot(vec![unit(1, "UNIT_WARRIOR", 2, 2, false)]);
        let p = plan_with(&s, &ArtState::Ready(table()), &ready);
        assert_eq!(asset(&p, "model:warrior"), 12);
        let c = cell_center(2, 2);
        for i in &p.batches[&ModelRef::Asset(LogicalId::new("model:warrior"))] {
            let off = Vec2::new(i.position[0], i.position[2]) - c;
            assert!(crate::terrain::height::hex_norm(off) < 0.9, "{off:?}");
            assert_eq!(i.scale, 0.14);
            assert_eq!(i.position[1], crate::terrain::LAND_HEIGHT);
        }
    }

    #[test]
    fn wounded_unit_loses_figures_from_the_end() {
        let mut u = unit(1, "UNIT_WARRIOR", 2, 2, false);
        u.hp = 40;
        let full = plan_with(
            &snapshot(vec![unit(1, "UNIT_WARRIOR", 2, 2, false)]),
            &ArtState::Ready(table()),
            &ready,
        );
        let hurt = plan_with(&snapshot(vec![u]), &ArtState::Ready(table()), &ready);
        let key = ModelRef::Asset(LogicalId::new("model:warrior"));
        let (a, b) = (&full.batches[&key], &hurt.batches[&key]);
        assert_eq!(b.len(), alive_figures(12, 60));
        assert_eq!(
            a[..b.len()],
            b[..],
            "the first figures stay where they were"
        );
    }

    fn warrior_positions(hp: i32, id: i32) -> Vec<Vec2> {
        let mut u = unit(id, "UNIT_WARRIOR", 2, 2, false);
        u.hp = hp;
        let p = plan_with(&snapshot(vec![u]), &ArtState::Ready(table()), &ready);
        let c = cell_center(2, 2);
        p.batches
            .get(&ModelRef::Asset(LogicalId::new("model:warrior")))
            .into_iter()
            .flatten()
            .map(|i| Vec2::new(i.position[0], i.position[2]) - c)
            .collect()
    }

    #[test]
    fn dead_figures_leave_from_the_highest_index_at_every_hp() {
        let full = warrior_positions(100, 1);
        assert_eq!(full.len(), 12);
        for hp in [100, 92, 75, 50, 34, 9, 1] {
            let hurt = warrior_positions(hp, 1);
            assert_eq!(
                hurt.len(),
                alive_figures(12, damage_pct(hp, 100)),
                "hp {hp}"
            );
            assert_eq!(
                hurt[..],
                full[..hurt.len()],
                "hp {hp}: survivors do not move"
            );
        }
        assert_eq!(warrior_positions(50, 1).len(), 6);
        assert_eq!(warrior_positions(1, 1).len(), 1);
        assert_eq!(warrior_positions(0, 1).len(), 0);
    }

    #[test]
    fn positions_are_stable_between_frames_and_differ_between_units() {
        assert_eq!(warrior_positions(100, 5), warrior_positions(100, 5));
        assert_ne!(warrior_positions(100, 5), warrior_positions(100, 6));
    }

    #[test]
    fn a_single_figure_unit_and_a_shared_cell_stay_inside_the_hex() {
        let mut t = table();
        t.insert_unit(
            "UNIT_SCOUT",
            UnitRecord {
                base: art("scout", 1, 0.14, false),
                variants: vec![],
            },
        );
        let s = snapshot(vec![
            unit(1, "UNIT_SCOUT", 2, 2, false),
            unit(2, "UNIT_WARRIOR", 3, 3, false),
            unit(3, "UNIT_SETTLER", 3, 3, true),
        ]);
        let p = plan_with(&s, &ArtState::Ready(t), &ready);
        let one = &p.batches[&ModelRef::Asset(LogicalId::new("model:scout"))];
        assert_eq!(one.len(), 1);
        let off = Vec2::new(one[0].position[0], one[0].position[2]) - cell_center(2, 2);
        assert!(off.length() < 13.0, "{off:?}");
        let c = cell_center(3, 3);
        for key in ["model:warrior", "model:settler"] {
            for i in &p.batches[&ModelRef::Asset(LogicalId::new(key))] {
                let o = Vec2::new(i.position[0], i.position[2]) - c;
                assert!(crate::terrain::height::hex_norm(o) < 0.95, "{key} {o:?}");
            }
        }
    }

    #[test]
    fn military_and_civilian_on_one_cell_are_apart() {
        let s = snapshot(vec![
            unit(1, "UNIT_WARRIOR", 2, 2, false),
            unit(2, "UNIT_SETTLER", 2, 2, true),
        ]);
        let p = plan_with(&s, &ArtState::Ready(table()), &ready);
        let c = cell_center(2, 2);
        let mean_x = |id: &str| {
            let v = &p.batches[&ModelRef::Asset(LogicalId::new(id))];
            v.iter().map(|i| i.position[0] - c.x).sum::<f32>() / v.len() as f32
        };
        assert!((mean_x("model:warrior") - MILITARY_OFFSET).abs() < 0.5);
        assert!((mean_x("model:settler") - CIVILIAN_OFFSET).abs() < 0.5);
        // The flags hang over the two slots.
        let xs: Vec<f32> = p.labels.iter().map(|l| l.position.x - c.x).collect();
        assert_eq!(xs, [MILITARY_OFFSET, CIVILIAN_OFFSET]);
        // Alone, a unit stands in the middle.
        let alone = plan_with(
            &snapshot(vec![unit(2, "UNIT_SETTLER", 2, 2, true)]),
            &ArtState::Ready(table()),
            &ready,
        );
        assert!((alone.labels[0].position.x - c.x).abs() < 1e-4);
    }

    #[test]
    fn style_variant_follows_the_owner() {
        let mut other = unit(2, "UNIT_WARRIOR", 3, 3, false);
        other.owner = 1;
        let s = snapshot(vec![unit(1, "UNIT_WARRIOR", 2, 2, false), other]);
        let p = plan_with(&s, &ArtState::Ready(table()), &ready);
        assert_eq!(asset(&p, "model:warrior"), 12);
        assert_eq!(asset(&p, "model:warrior_asia"), 12);
    }

    #[test]
    fn placeholder_missing_record_and_failed_model_give_markers() {
        let s = snapshot(vec![
            unit(1, "UNIT_STUB", 1, 1, false),
            unit(2, "UNIT_UNKNOWN", 2, 2, false),
            unit(3, "UNIT_SETTLER", 3, 3, true),
        ]);
        let failed = |m: &LogicalId| {
            if m.as_str() == "model:settler" {
                ModelStatus::Failed
            } else {
                ModelStatus::Ready
            }
        };
        let p = plan_with(&s, &ArtState::Ready(table()), &failed);
        let m = &p.batches[&ModelRef::UnitMarker];
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].tint, srgb_tint([200, 30, 30]), "the owner's colour");
        assert_eq!(p.instance_count(), 3);
        // Without assets every unit is a marker.
        let n = plan_with(&s, &ArtState::Unavailable, &ready);
        assert_eq!(n.batches[&ModelRef::UnitMarker].len(), 3);
    }

    #[test]
    fn pending_models_hide_the_unit_and_are_wanted() {
        let s = snapshot(vec![unit(1, "UNIT_SETTLER", 1, 1, true)]);
        let pending = |_: &LogicalId| ModelStatus::Pending;
        let p = plan_with(&s, &ArtState::Ready(table()), &pending);
        assert_eq!(p.instance_count(), 0);
        assert_eq!(p.wanted.len(), 1);
        assert_eq!(p.labels.len(), 1, "the flag does not wait for the model");
        // Nothing before the table is there.
        let l = plan_with(&s, &ArtState::Loading, &ready);
        assert_eq!((l.instance_count(), l.labels.len()), (0, 0));
    }

    #[test]
    fn units_only_on_visible_cells_and_on_the_ground() {
        let s = snapshot(vec![
            unit(1, "UNIT_SETTLER", 5, 5, true), // hidden
            unit(2, "UNIT_SETTLER", 0, 5, true), // unrevealed
            unit(3, "UNIT_SETTLER", 4, 4, true), // hill
            unit(4, "UNIT_SETTLER", 9, 9, true), // off the map
        ]);
        let p = plan_with(&s, &ArtState::Ready(table()), &ready);
        let v = &p.batches[&ModelRef::Asset(LogicalId::new("model:settler"))];
        assert_eq!(v.len(), 3);
        assert!(v[0].position[1] > crate::terrain::LAND_HEIGHT + 5.0);
        assert_eq!(p.labels.len(), 1);
        assert_eq!(p.labels[0].cell, (4, 4));
        // The flag is 35 over the ground.
        assert!((p.labels[0].position.y - (crate::terrain::HILL_TOP + 35.0)).abs() < 0.5);
    }

    #[test]
    fn budget_of_a_busy_world_is_batches_not_instances() {
        let types = ["UNIT_WARRIOR", "UNIT_SETTLER"];
        let units: Vec<UnitEntity> = (0..100)
            .map(|i| unit(i, types[(i % 2) as usize], i % 6, (i / 6) % 6, i % 2 == 1))
            .collect();
        let s = snapshot(units);
        let p = plan_with(&s, &ArtState::Ready(table()), &ready);
        assert!(p.instance_count() > 100);
        assert!(p.batches.len() <= 40, "{} batches", p.batches.len());
    }
}
