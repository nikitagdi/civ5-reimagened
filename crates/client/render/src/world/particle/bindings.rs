//! Which visual effect plays where (answers-b update 9, RB-5). Only eight effect names are in the
//! exe; the rest come from the `.ftsxml` triggers of the models (not played here):
//!
//! * the "look here" beam: `GameplayFX` (event 143) -> `ART_DEF_VEFFECT_LOOK_AT`, one shot, FOW
//!   mask 3 ([`super::Particle::look_at`]);
//! * the fire of a damaged city: `UpdateDamageState` (0x087b8412) makes one VEFFECT per hurt
//!   population building (`city::damage`, which also draws the `FXM_City_Damage_State_*` models of
//!   it). FIRST is a model effect only (no `.psb`), SECOND and THIRD add `FXP_D2_Fire_01.psb` and
//!   `FXP_D3_Fire_01.psb`, always variant 01 (rw-a6 RA6c). The city system says which buildings
//!   burn and where ([`CityFire`]); the particles of the VEFFECT are played here, held (`Spawn`)
//!   until the set of fires of the city changes. The fx index holds the VEFFECT with its `.psb`
//!   only, so playing the name plays the fire and not the model a second time;
//! * `FARM_PILLAGED_SMOKE_01`, `PARADROP`, `XCOM_DROP`: a model effect, and the paradrop that the
//!   game here has no event for; not played here (the pillaged smoke is a model of the landmarks).

use std::collections::HashMap;

use glam::Vec3;

use super::{Lifetime, Particle, Place, key_of};
use crate::entities::CityKey;

/// One fire of a city: the `index`-th building (its place in the list of the lots), the visual
/// effect to play and where it stands (world point, our basis; the scale 0.7 is the VEFFECT's own).
#[derive(Debug, Clone, PartialEq)]
pub struct CityFire {
    pub index: usize,
    pub name: &'static str,
    pub at: Vec3,
}

/// The fires the cities show now.
#[derive(Debug, Default)]
pub struct CityFires {
    fires: HashMap<CityKey, Vec<CityFire>>,
}

/// The key of the fire of building `index` of a city.
pub fn fire_key(key: CityKey, index: usize) -> u64 {
    let city = (i64::from(key.0) << 32) | i64::from(key.1 as u32);
    key_of("city_fire", city, index as i64)
}

impl CityFires {
    /// The fires of a city as the city system has them now (empty: none). Only what changed is
    /// stopped and played again.
    pub fn set(&mut self, p: &mut Particle, key: CityKey, fires: &[CityFire]) {
        let old = self.fires.remove(&key).unwrap_or_default();
        for f in &old {
            if !fires.contains(f) {
                p.stop(fire_key(key, f.index));
            }
        }
        for f in fires {
            if !old.contains(f) {
                p.play(
                    f.name,
                    fire_key(key, f.index),
                    Place::World(f.at),
                    1.0,
                    Lifetime::Held,
                );
            }
        }
        if !fires.is_empty() {
            self.fires.insert(key, fires.to_vec());
        }
    }

    /// The city is gone (captured, razed, out of the feed).
    pub fn remove(&mut self, p: &mut Particle, key: CityKey) {
        self.set(p, key, &[]);
    }

    pub fn clear(&mut self, p: &mut Particle) {
        for key in self.fires.keys().copied().collect::<Vec<_>>() {
            self.remove(p, key);
        }
    }

    /// How many fires burn in the city.
    pub fn count(&self, key: CityKey) -> usize {
        self.fires.get(&key).map_or(0, Vec::len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D2: &str = "ART_DEF_VEFFECT_CITY_DAMAGED_SMOKE_SECOND_01";
    const D3: &str = "ART_DEF_VEFFECT_CITY_DAMAGED_SMOKE_THIRD_01";

    fn fire(index: usize, name: &'static str) -> CityFire {
        CityFire {
            index,
            name,
            at: Vec3::new(index as f32, 0.0, 0.0),
        }
    }

    #[test]
    fn keys_are_stable_and_differ_by_city_and_building() {
        assert_eq!(fire_key((0, 5), 1), fire_key((0, 5), 1));
        assert_ne!(fire_key((0, 5), 1), fire_key((0, 5), 2));
        assert_ne!(fire_key((0, 5), 1), fire_key((1, 5), 1));
        assert_ne!(fire_key((0, 5), 1), fire_key((0, 6), 1));
    }

    #[test]
    fn a_fire_burns_while_the_city_lists_it_and_stops_when_healed_or_removed() {
        let mut p = Particle::default();
        let mut d = CityFires::default();
        let key = (0, 5);
        d.set(&mut p, key, &[fire(0, D2), fire(1, D2)]);
        assert_eq!(d.count(key), 2);
        assert!(p.is_playing(fire_key(key, 0)) && p.is_playing(fire_key(key, 1)));
        // One building heals, the other worsens: only the changed ones are touched.
        d.set(&mut p, key, &[fire(1, D3)]);
        assert!(!p.is_playing(fire_key(key, 0)));
        assert!(p.is_playing(fire_key(key, 1)));
        d.set(&mut p, key, &[]);
        assert_eq!(d.count(key), 0);
        assert!(!p.is_playing(fire_key(key, 1)));
        // Removal of a burning city.
        d.set(&mut p, key, &[fire(2, D2)]);
        d.remove(&mut p, key);
        assert!(!p.is_playing(fire_key(key, 2)));
        d.set(&mut p, key, &[fire(2, D2)]);
        d.clear(&mut p);
        assert!(!p.is_playing(fire_key(key, 2)));
    }
}
