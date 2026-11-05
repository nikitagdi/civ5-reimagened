//! Where the figures of a unit stand and what the animation is asked for
//!
//! Everything here is a function of the record of the unit: its cell, `facing`, hit points and
//! flags. A unit that changes its cell in one `upsert` is placed on the new cell the next time and
//! nowhere else (WO-D6, no animation of the move); its animation keeps running because the key of
//! the animation is the key of the unit and the number of the figure (`AnimTarget::Figure`), which
//! do not change with the cell.
//!
//! SIMPLIFIED(SIMP-044): the military and the civilian unit that share a cell stand on places of our
//! own (B1's offsets). Goes with RW-14 (h, i). The squad is not squeezed into the hex: the exe uses
//! the offsets of `GenerateUnitFormation` as they are (RE-B18 item 2 F1).

use glam::{Vec2, Vec3};

use asset_format::LogicalId;

use super::art::FigureSpec;
use crate::anim::{AnimState, EC_FORTIFY_ACTION, EC_IDLE_ACTION};
use crate::entities::formation;
use crate::entities::units::{Slot, alive_figures, damage_pct};
use crate::entities::{FACING_Y, GLOBAL_UNIT_SCALE, UnitKey};
use crate::hexgrid::{Dir, cell_center};
use crate::static_model::StaticInstance;
use crate::world::objects::instance::{BoneRef, ObjectInstance};

/// What the view says of a unit beyond its record in B1 (`UnitView.facing`, the flags, the
/// `building` of a worker; net-protocol 4.15). The bridge fills it when the protocol has the fields;
/// without it a unit faces the camera, stands, and is not embarked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UnitVisExtra {
    /// `None`: towards the camera, as B1 draws.
    pub facing: Option<Dir>,
    pub flags: UnitVisFlags,
    /// What the worker builds (`Q_UNITS.build_type`); `None` when it builds nothing.
    pub building: Option<i16>,
}

/// `UnitVisFlags` of the spec (4.2). `COMBAT_READY` is not a flag (decision of the user
/// 2026-10-21, as in the original, NP-D24).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UnitVisFlags(pub u8);

impl UnitVisFlags {
    pub const CIVILIAN: u8 = 1;
    pub const EMBARKED: u8 = 2;
    pub const FORTIFIED: u8 = 4;
    pub const SLEEPING: u8 = 8;
    /// On board a transport or a carrier.
    pub const CARGO: u8 = 16;
    pub const GARRISON: u8 = 32;

    pub fn has(self, flag: u8) -> bool {
        self.0 & flag != 0
    }

    pub fn with(self, flag: u8) -> Self {
        Self(self.0 | flag)
    }
}

/// The yaw (about Y, the `rotate_y` of B1) of a unit that faces `facing`: its front axis, +Z at
/// yaw 0 (towards the camera), turns to the direction on the map. `None` is [`FACING_Y`].
// TODO(RW-14): the front axis of the models and the rule of the engine for a unit that has not
// moved are not read.
pub fn facing_yaw(facing: Option<Dir>) -> f32 {
    let Some(d) = facing else {
        return FACING_Y;
    };
    // The direction on the map, east and north; ours is east and south (z = -north).
    let a = d.normal_angle();
    let (east, south) = (a.cos(), -a.sin());
    east.atan2(south)
}

/// `rotate_y` of B1 on the plane: `(x c + z s, -x s + z c)`.
fn rotate(v: Vec2, yaw: f32) -> Vec2 {
    let (s, c) = yaw.sin_cos();
    Vec2::new(v.x * c + v.y * s, -v.x * s + v.y * c)
}

/// One figure placed.
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    /// The number of the figure in the full squad (the dead ones keep their numbers).
    pub index: u8,
    pub model: LogicalId,
    /// In our basis.
    pub position: Vec3,
    pub yaw: f32,
    pub scale: f32,
    /// The desired-state byte of the figure's model nodes ([`damage_state`]): 1 intact, a higher
    /// bit for each damage level of a ship (R5.11).
    pub state: u8,
}

/// The desired-state byte of a living figure at `damage_pct` percent of damage, for a unit whose
/// art has `DamageStates` (`None`: the column is empty, the engine's default step of 100).
// re: cvUnitSimSystem::UpdateFromHandle(cvUnitSim_SetHitPct) 0x08804b2c: state = 1 << (pct / step),
// step = 100 / DamageStates (DamageStates <= 0 or absent: 100); a step above 100 gives 1
// (OnUnitCreated 0x088037da, GetUnitDamageStateRange 0x0892c132; answers-b update 9).
pub fn damage_state(damage_pct: u32, damage_states: Option<u8>) -> u8 {
    let step = match damage_states {
        Some(n) if n > 0 => 100 / u32::from(n),
        _ => 100,
    };
    if step == 0 || step > 100 {
        return 1;
    }
    // The engine shifts a 32-bit one and stores the low byte.
    ((1u32 << ((damage_pct.min(100) / step) & 31)) & 0xff) as u8
}

/// What `place_figures` reads of one unit.
pub struct PlaceParams<'a> {
    pub key: UnitKey,
    pub cell: (i32, i32),
    /// Alone on the cell, or on the side of the military or the civilian unit (SIMP-044).
    pub slot: Slot,
    pub extra: &'a UnitVisExtra,
    pub hp: i32,
    pub max_hp: i32,
    pub figures: &'a [FigureSpec],
    pub formation: &'a formation::Params,
    /// The level of the water (`water_level()` of `world-water`): the surface under a ship or an
    /// embarked unit. `None` before the field is there.
    pub water_level: Option<f32>,
    /// `DamageStates` of the unit's art (`ArtDefine_UnitInfos`): ships 3, galley 1.
    pub damage_states: Option<u8>,
}

/// The living figures of the squad on their cell. `ground` gives the height of the land at a point
/// (`TerrainSampler::world_height`); the figure stands at `max(ground, water)` (`surface_height`,
/// WO-D8). A unit on board (`CARGO`) has no figures (RW-14 may say otherwise).
pub fn place_figures(p: &PlaceParams<'_>, ground: &dyn Fn(Vec2) -> f32, out: &mut Vec<Placement>) {
    if p.extra.flags.has(UnitVisFlags::CARGO) || p.figures.is_empty() {
        return;
    }
    let total = p.figures.len();
    let centre = cell_center(p.cell.0, p.cell.1) + Vec2::new(p.slot.offset_x(), 0.0);
    let yaw = facing_yaw(p.extra.facing);
    // The layout is made for the whole squad so the living figures stay where they were.
    let offsets = formation::generate(total, total, p.formation, p.key.1);
    let pct = damage_pct(p.hp, p.max_hp);
    let alive = alive_figures(total, pct);
    let state = damage_state(pct, p.damage_states);
    for (i, (fig, off)) in p.figures.iter().zip(&offsets).take(alive).enumerate() {
        // The rows of `GenerateUnitFormation` go backwards from the front of the unit: its first
        // row stands on the side the unit faces. Measured in S1a (the palettes of the 21 figures of
        // ev 2095-2170 against our places): across the same, the row axis the other way.
        let at = centre + rotate(Vec2::new(off.x, -off.y), yaw);
        let mut y = ground(at);
        if let Some(w) = p.water_level {
            y = y.max(w);
        }
        out.push(Placement {
            index: i as u8,
            model: fig.model.clone(),
            position: Vec3::new(at.x, y + fig.z_offset, at.y),
            yaw,
            scale: fig.scale * GLOBAL_UNIT_SCALE,
            state,
        });
    }
}

/// The instance of `world-objects` for a placed figure: the matrix `T * R_y * S`, the colour of the
/// team, the pose of the animation.
pub fn object_instance(p: &Placement, team: [u8; 3], bones: BoneRef) -> ObjectInstance {
    let mut s = StaticInstance::new(p.position.to_array(), p.yaw, p.scale);
    s.team = crate::static_model::pack_team(team);
    let mut o = ObjectInstance::from_static(&s);
    o.bones = bones;
    o
}

/// Adds `item` to the list of `key`. `entry(key.clone())` copies the key (a model name) for every
/// call, even when the list is there; this copies it only for the first item of a list.
pub fn push_to<K: Ord + Clone, T>(
    map: &mut std::collections::BTreeMap<K, Vec<T>>,
    key: &K,
    item: T,
) {
    match map.get_mut(key) {
        Some(list) => list.push(item),
        None => {
            map.insert(key.clone(), vec![item]);
        }
    }
}

/// What the animation is asked for by the state of the unit (WA-D4): a worker that builds works,
/// a fortified unit takes its fortify pose, the rest stand. A unit that only sleeps stands: the
/// game core tells the engine of fortification alone (`GameplayUnitFortify` from
/// `CvUnit::setFortifyTurns` and `SetFortifiedThisTurn`, and a civilian is never fortifyable);
/// `SetActivityType(ACTIVITY_SLEEP)` sends nothing to the engine, so an idle or sleeping worker
/// stands in its idle. Combat readiness, attack and the others are never asked.
// TODO(RW-14 (k)): the code the engine sets for each state (`SetVisualizerAnimation`), and which of
// 1910..1960 goes with which build; until then every build is the first work clip.
pub fn anim_state_of(extra: &UnitVisExtra) -> AnimState {
    let action = if let Some(build) = extra.building {
        crate::anim::work_action_of_build(build)
    } else if extra.flags.has(UnitVisFlags::FORTIFIED) {
        EC_FORTIFY_ACTION
    } else {
        EC_IDLE_ACTION
    };
    AnimState {
        action,
        ..AnimState::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::units::alive_figures;

    fn figures(n: usize) -> Vec<FigureSpec> {
        (0..n)
            .map(|_| FigureSpec {
                model: LogicalId::new("md:warrior"),
                scale: 0.14,
                z_offset: 0.0,
            })
            .collect()
    }

    fn params<'a>(
        key: UnitKey,
        cell: (i32, i32),
        figs: &'a [FigureSpec],
        extra: &'a UnitVisExtra,
        form: &'a formation::Params,
    ) -> PlaceParams<'a> {
        PlaceParams {
            key,
            cell,
            slot: Slot::Alone,
            extra,
            hp: 100,
            max_hp: 100,
            figures: figs,
            formation: form,
            water_level: None,
            damage_states: None,
        }
    }

    fn flat(_: Vec2) -> f32 {
        5.0
    }

    fn place(p: &PlaceParams<'_>) -> Vec<Placement> {
        let mut v = Vec::new();
        place_figures(p, &flat, &mut v);
        v
    }

    #[test]
    fn the_living_figures_follow_the_formula_of_hit_points() {
        let form = formation::Params::DEFAULT;
        let extra = UnitVisExtra::default();
        let figs = figures(8);
        for hp in 0..=100 {
            let mut p = params((0, 7), (3, 3), &figs, &extra, &form);
            p.hp = hp;
            assert_eq!(
                place(&p).len(),
                alive_figures(8, damage_pct(hp, 100)),
                "hp {hp}"
            );
        }
    }

    #[test]
    fn the_survivors_do_not_move_when_others_die() {
        let form = formation::Params::DEFAULT;
        let extra = UnitVisExtra::default();
        let figs = figures(6);
        let full = place(&params((1, 9), (2, 2), &figs, &extra, &form));
        let mut hurt = params((1, 9), (2, 2), &figs, &extra, &form);
        hurt.hp = 40;
        let some = place(&hurt);
        assert!(some.len() < full.len() && !some.is_empty());
        for s in &some {
            assert_eq!(s, &full[usize::from(s.index)]);
        }
    }

    /// S1a: the first row of the formation stands on the side the unit faces, the last behind it.
    #[test]
    fn the_first_row_stands_in_front() {
        let form = formation::Params {
            unit_randomness: 0.0,
            ..formation::Params::DEFAULT_MELEE
        };
        let figs = figures(12);
        let east = UnitVisExtra {
            facing: Some(Dir::East),
            ..UnitVisExtra::default()
        };
        let placed = place(&params((0, 3), (4, 4), &figs, &east, &form));
        let ahead = |r: std::ops::Range<usize>| {
            placed[r.clone()].iter().map(|p| p.position.x).sum::<f32>() / r.len() as f32
        };
        // Facing east (+X): rows of four, the first one furthest east.
        let (first, last) = (ahead(0..4), ahead(8..12));
        assert!(first > last + 20.0, "{first} {last}");
    }

    /// WOB2: `facing` turns the formation about the middle of the cell and the figures with it.
    #[test]
    fn facing_turns_the_formation() {
        let form = formation::Params::DEFAULT;
        let figs = figures(5);
        let none = UnitVisExtra::default();
        let east = UnitVisExtra {
            facing: Some(Dir::East),
            ..UnitVisExtra::default()
        };
        let a = place(&params((0, 3), (4, 4), &figs, &none, &form));
        let b = place(&params((0, 3), (4, 4), &figs, &east, &form));
        assert_eq!(a.len(), b.len());
        let centre = cell_center(4, 4);
        for (p, q) in a.iter().zip(&b) {
            // The same distance from the middle, another place, the yaw of the direction.
            let (dp, dq) = (
                Vec2::new(p.position.x, p.position.z) - centre,
                Vec2::new(q.position.x, q.position.z) - centre,
            );
            assert!((dp.length() - dq.length()).abs() < 1e-3);
            assert_eq!(p.yaw, FACING_Y);
            assert!(
                (q.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-4,
                "east is +X"
            );
        }
        assert!(
            a.iter()
                .zip(&b)
                .any(|(p, q)| p.position.distance(q.position) > 1.0)
        );
        // The six directions are 60 degrees apart, and west is opposite east.
        let yaws: Vec<f32> = Dir::ALL.iter().map(|d| facing_yaw(Some(*d))).collect();
        for d in Dir::ALL {
            let o = d.opposite();
            let diff = (facing_yaw(Some(d)) - facing_yaw(Some(o))).abs();
            assert!((diff - std::f32::consts::PI).abs() < 1e-3, "{d:?}");
        }
        assert_eq!(yaws.len(), 6);
        // Facing south-east in our basis: x east, z south: (0.5, 0.866).
        let se = facing_yaw(Some(Dir::SouthEast));
        assert!((se - 30f32.to_radians()).abs() < 1e-4, "{se}");
    }

    #[test]
    fn cargo_is_not_drawn() {
        let form = formation::Params::DEFAULT;
        let extra = UnitVisExtra {
            flags: UnitVisFlags::default().with(UnitVisFlags::CARGO),
            ..UnitVisExtra::default()
        };
        let figs = figures(3);
        assert!(place(&params((0, 1), (1, 1), &figs, &extra, &form)).is_empty());
        assert!(
            place(&params(
                (0, 1),
                (1, 1),
                &[],
                &UnitVisExtra::default(),
                &form
            ))
            .is_empty()
        );
    }

    /// WO-D8: a figure stands at `max(ground, water)`: a ship and a landed unit on the water stand
    /// on its surface, a unit on dry land on the ground; the z offset lifts a figure.
    #[test]
    fn figures_stand_on_the_surface() {
        let form = formation::Params::DEFAULT;
        let extra = UnitVisExtra::default();
        let mut figs = figures(2);
        figs[1].z_offset = 3.0;
        let mut p = params((0, 1), (1, 1), &figs, &extra, &form);
        p.water_level = Some(10.12);
        // Land at 5 is under the water at 10.12.
        let on_water = place(&p);
        assert_eq!(on_water[0].position.y, 10.12);
        assert_eq!(on_water[1].position.y, 10.12 + 3.0);
        // Land at 20 is above it.
        let mut v = Vec::new();
        place_figures(&p, &|_| 20.0, &mut v);
        assert_eq!(v[0].position.y, 20.0);
    }

    #[test]
    fn a_shared_cell_puts_the_two_kinds_apart() {
        let form = formation::Params::DEFAULT;
        let extra = UnitVisExtra::default();
        let figs = figures(1);
        let mut mil = params((0, 1), (3, 3), &figs, &extra, &form);
        mil.slot = Slot::Military;
        let mut civ = params((0, 2), (3, 3), &figs, &extra, &form);
        civ.slot = Slot::Civilian;
        let (a, b) = (place(&mil), place(&civ));
        assert!(a[0].position.x < b[0].position.x);
    }

    /// WO-D6, the jump: the same record on another cell is placed on that cell and keeps its
    /// number of figure, which is the key of its animation.
    #[test]
    fn a_unit_that_changes_cell_stands_on_the_new_one() {
        let form = formation::Params::DEFAULT;
        let extra = UnitVisExtra::default();
        let figs = figures(4);
        let here = place(&params((2, 11), (1, 1), &figs, &extra, &form));
        let there = place(&params((2, 11), (4, 5), &figs, &extra, &form));
        let (c1, c2) = (cell_center(1, 1), cell_center(4, 5));
        for (a, b) in here.iter().zip(&there) {
            assert_eq!(a.index, b.index);
            // The same place inside the cell, shifted by the distance of the cells.
            let shift = Vec2::new(b.position.x - a.position.x, b.position.z - a.position.z);
            assert!((shift - (c2 - c1)).length() < 1e-3);
        }
    }

    #[test]
    fn the_instance_is_the_matrix_of_the_figure() {
        let p = Placement {
            index: 0,
            model: LogicalId::new("md:x"),
            position: Vec3::new(10.0, 2.0, -5.0),
            yaw: 0.7,
            scale: 1.5,
            state: 1,
        };
        let bones = BoneRef {
            base: 3,
            next: 6,
            blend: 0.5,
        };
        let o = object_instance(&p, [200, 30, 40], bones);
        assert_eq!(o.bones, bones);
        assert_eq!(o.color, [200, 30, 40, 255]);
        let q = o.transform_point([0.0, 0.0, 0.0]);
        assert_eq!(q, [10.0, 2.0, -5.0]);
    }

    /// R5.11 (answers-b update 9): `DamageStates` 3 gives the steps 0-32, 33-65, 66-98 and 99
    /// percent; the galley (1) and an empty column keep the intact state until death.
    #[test]
    fn the_damage_state_follows_the_steps_of_the_engine() {
        let s = |pct| damage_state(pct, Some(3));
        assert_eq!(
            [s(0), s(32), s(33), s(65), s(66), s(98), s(99)],
            [1, 1, 2, 2, 4, 4, 8]
        );
        assert_eq!(damage_state(99, Some(1)), 1);
        assert_eq!(damage_state(99, None), 1);
        assert_eq!(damage_state(50, Some(0)), 1);
    }

    /// WA-D4: the table from the state of the unit to what the animation is asked for.
    #[test]
    fn the_state_of_the_unit_picks_the_action() {
        let mut e = UnitVisExtra::default();
        assert_eq!(anim_state_of(&e).action, 1000);
        e.flags = e.flags.with(UnitVisFlags::FORTIFIED);
        assert_eq!(anim_state_of(&e).action, 1500);
        // Sleep is no pose of its own: the core never tells the engine of it.
        e.flags = UnitVisFlags::default().with(UnitVisFlags::SLEEPING);
        assert_eq!(anim_state_of(&e).action, 1000);
        // Garrison, embarked, civilian: no pose of their own.
        e.flags = UnitVisFlags::default()
            .with(UnitVisFlags::GARRISON)
            .with(UnitVisFlags::EMBARKED)
            .with(UnitVisFlags::CIVILIAN);
        assert_eq!(anim_state_of(&e).action, 1000);
        // A worker that builds works, even fortified.
        e.building = Some(7);
        e.flags = e.flags.with(UnitVisFlags::FORTIFIED);
        let a = anim_state_of(&e).action;
        assert!((1910..=1960).contains(&a));
        // Whatever the flags, only the three standing actions are asked for.
        for bits in 0..=63u8 {
            for building in [None, Some(0), Some(30)] {
                let st = anim_state_of(&UnitVisExtra {
                    facing: None,
                    flags: UnitVisFlags(bits),
                    building,
                });
                assert!(
                    crate::anim::action_is_standing(st.action),
                    "{bits} {building:?}"
                );
            }
        }
    }
}
