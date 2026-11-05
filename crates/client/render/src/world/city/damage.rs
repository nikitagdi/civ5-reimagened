//! The damage effects of a city (world-objects WOB5, card R5.11): the smoke and fire models
//! `FXM_City_Damage_State_*` over the buildings of a hurt city.
//!
//! re: `cvCityBuildingSim::UpdateDamageState` (0x087b8412, `ra6c-5-citydamage.txt`): for each
//! population building whose tier (byte +0x19 of its 0x24-byte record) is below the damage level of
//! the city (byte +0x532, set by `OnCitySetDamage` 0x087b4062 from the event), one VEFFECT is made
//! at the building, a translation matrix and the handle `table[(level - 1 - tier) * 3 + rand % 3]`
//! of `sim+0x3fe74[9]`. The table is FIRST_01..03, then SECOND_01 three times, then THIRD_01 three
//! times (the exe never uses variants 02 and 03 of the second and third state, though the XML
//! defines them). Every VEFFECT has the transform scale 0.7 and shows on visible plots only
//! (`FOWMask` absent = 2).
//!
//! The fire (`FXP_D2_Fire_01`, `FXP_D3_Fire_01`) of the second and third state is the particle of the
//! same VEFFECT: [`Effect::fire`] names it for the particle player (`particle::bindings`), at the
//! same place as the model and held while the effect stands.
//!
//! The level is the one of `GameplayCitySetDamage` (0x0879ec40, re-b17 4.1): the first `i` with
//! `damage <= T[i]`, `T = {0, 7, 15, 20}`, else 3, on the raw damage points of the city
//! (`max_hp - hp`, not rescaled: a city of 200 hit points burns in full after 16).
//!
//! SIMPLIFIED(SIMP-068): the tier of a building is the burn rank of the layout
//! (`ResetPopBuildingsFromPlacementsInCity`, [`super::layout`]); the choice among the three is a
//! hash, not `rand() % 3`.

use glam::{Mat4, Vec2, Vec3};

use super::compose::Chosen;
use super::layout::CITY_SCALE;
use crate::world::landmark::select::mix;

/// The thresholds of the damage (floats at 0x0920b980): the level is the first one the damage
/// does not pass.
pub const STEPS: [i32; 4] = [0, 7, 15, 20];

/// The highest level (the tiers are 0..3).
pub const MAX_LEVEL: u8 = 3;

/// `sim+0x3fe74[9]`: the logical ids of the effect models, in the order of the exe table.
pub const MODELS: [&str; 9] = [
    "md:fxm_city_damage_state_first_01",
    "md:fxm_city_damage_state_first_02",
    "md:fxm_city_damage_state_first_03",
    "md:fxm_city_damage_state_second_01",
    "md:fxm_city_damage_state_second_01",
    "md:fxm_city_damage_state_second_01",
    "md:fxm_city_damage_state_third_01",
    "md:fxm_city_damage_state_third_01",
    "md:fxm_city_damage_state_third_01",
];

/// `Transform scale` of every VEFFECT of the city damage.
pub const EFFECT_SCALE: f32 = 0.7;

/// The first slot (`Placed::slot`) of the damage models of a city; one per distinct model.
pub const DAMAGE_SLOT: u16 = 40;

/// The damage level 0..=3 of a city that has lost `damage` points.
pub fn level_of(damage: i32) -> u8 {
    STEPS
        .iter()
        .position(|&t| damage <= t)
        .map_or(MAX_LEVEL, |i| (i as u8).min(MAX_LEVEL))
}

/// The tier of the `j`-th building of a city.
pub fn tier_of(j: usize) -> u8 {
    (j % 3) as u8
}

/// The model of a building of `tier` in a city of `level`, with `rand` for the choice among three;
/// `None` if the building is not hurt (`tier >= level`).
pub fn model_for(level: u8, tier: u8, rand: u32) -> Option<&'static str> {
    if tier >= level {
        return None;
    }
    let state = usize::from(level - 1 - tier);
    Some(MODELS[state * 3 + (rand % 3) as usize])
}

/// The VEFFECT whose particles are the fire of `model` (`None` for the first state, a model only).
/// The fx index holds the VEFFECT with its `.psb` only, so playing the name plays the fire alone.
pub fn fire_of(model: &str) -> Option<&'static str> {
    if model.contains("_second_") {
        Some("ART_DEF_VEFFECT_CITY_DAMAGED_SMOKE_SECOND_01")
    } else if model.contains("_third_") {
        Some("ART_DEF_VEFFECT_CITY_DAMAGED_SMOKE_THIRD_01")
    } else {
        None
    }
}

/// One effect: the model and where it stands (catalog units from the middle of the hex).
#[derive(Debug, Clone, PartialEq)]
pub struct Effect {
    /// The place of the building in the list of the lots (the key of its fire).
    pub index: usize,
    pub model: &'static str,
    pub at: Vec2,
}

impl Effect {
    /// The VEFFECT of the fire particles of this effect, if its state has them.
    pub fn fire(&self) -> Option<&'static str> {
        fire_of(self.model)
    }
}

/// The effects of a city of `level` over its buildings. `seed` is the seed of the city
/// (`layout::seed_of`); the choice among the three variants is a hash of it and of the building.
pub fn effects(level: u8, chosen: &[Chosen], seed: u32) -> Vec<Effect> {
    chosen
        .iter()
        .enumerate()
        .filter_map(|(j, c)| {
            let rand = mix(seed, 0x300 + j as u32);
            model_for(level, c.rank, rand).map(|model| Effect {
                index: j,
                model,
                at: c.at,
            })
        })
        .collect()
}

/// The matrix of an effect at `at` of a city whose middle is `at_city`: the translation of the
/// building and the scale of the VEFFECT.
pub fn matrix(at_city: Mat4, at: Vec2) -> Mat4 {
    let off = at * CITY_SCALE;
    at_city
        * Mat4::from_translation(Vec3::new(off.x, 0.0, off.y))
        * Mat4::from_scale(Vec3::splat(EFFECT_SCALE))
}

/// The slot of `model` among the damage models (the first of its place in [`MODELS`]).
pub fn slot_of(model: &str) -> u16 {
    DAMAGE_SLOT + MODELS.iter().position(|m| *m == model).unwrap_or(0) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chosen(n: usize) -> Vec<Chosen> {
        (0..n)
            .map(|i| Chosen {
                lot: i,
                slot: i,
                at: Vec2::new(10.0 * i as f32, 0.0),
                yaw: 0.0,
                ruined: false,
                rank: tier_of(i),
            })
            .collect()
    }

    #[test]
    fn the_level_is_the_first_threshold_the_raw_damage_does_not_pass() {
        let level = |d| level_of(d);
        assert_eq!(
            [level(-5), level(0), level(1), level(7), level(8)],
            [0, 0, 1, 1, 2]
        );
        assert_eq!(
            [level(15), level(16), level(20), level(21), level(200)],
            [2, 3, 3, 3, 3]
        );
    }

    #[test]
    fn a_building_shows_the_state_of_the_level_beyond_its_tier() {
        // Level 1: only tier 0, the first state.
        assert_eq!(model_for(1, 0, 0), Some(MODELS[0]));
        assert_eq!(model_for(1, 0, 4), Some(MODELS[1]));
        assert_eq!(model_for(1, 1, 0), None);
        // Level 3: tier 0 the third state, tier 1 the second, tier 2 the first.
        assert_eq!(
            model_for(3, 0, 5),
            Some("md:fxm_city_damage_state_third_01")
        );
        assert_eq!(
            model_for(3, 1, 7),
            Some("md:fxm_city_damage_state_second_01")
        );
        assert_eq!(model_for(3, 2, 2), Some(MODELS[2]));
        assert_eq!(model_for(0, 0, 0), None);
    }

    #[test]
    fn an_undamaged_city_has_no_effect_and_a_hurt_one_is_the_same_every_time() {
        let c = chosen(6);
        assert!(effects(0, &c, 9).is_empty());
        let a = effects(2, &c, 9);
        assert_eq!(a, effects(2, &c, 9));
        // Tiers 0 and 1 of the six buildings are hurt at level 2, tier 2 is not.
        assert_eq!(a.len(), 4);
        assert!(a[0].model.contains("second") && a[1].model.contains("first"));
        assert_eq!(a[0].at, c[0].at);
    }

    #[test]
    fn only_the_second_and_third_states_burn() {
        let c = chosen(6);
        let burning = |level: u8| -> Vec<(usize, &'static str)> {
            effects(level, &c, 9)
                .iter()
                .filter_map(|e| e.fire().map(|f| (e.index, f)))
                .collect()
        };
        let d2 = "ART_DEF_VEFFECT_CITY_DAMAGED_SMOKE_SECOND_01";
        let d3 = "ART_DEF_VEFFECT_CITY_DAMAGED_SMOKE_THIRD_01";
        assert!(burning(0).is_empty());
        assert!(burning(1).is_empty(), "the first state is a model only");
        // Level 2: tier 0 (buildings 0 and 3) is in the second state, tier 1 in the first.
        assert_eq!(burning(2), [(0, d2), (3, d2)]);
        // Level 3: tier 0 third, tier 1 second, tier 2 first.
        assert_eq!(burning(3), [(0, d3), (1, d2), (3, d3), (4, d2)]);
        // The index is that of the building, whatever the level.
        assert!(effects(3, &c, 9).iter().all(|e| e.at == c[e.index].at));
    }

    #[test]
    fn every_model_has_a_slot_of_its_own() {
        assert_eq!(slot_of(MODELS[0]), DAMAGE_SLOT);
        assert_ne!(slot_of(MODELS[1]), slot_of(MODELS[2]));
        assert_eq!(slot_of(MODELS[3]), slot_of(MODELS[4]));
    }
}
