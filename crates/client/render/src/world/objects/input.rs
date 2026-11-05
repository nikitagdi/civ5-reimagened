//! Inputs of the object subsystems by key (WO-D2): a subsystem keeps its records (a unit by
//! `(owner, id)`, a city the same, a plot by its cell) and takes `upsert` and `remove`. Only the
//! keys that changed are placed again; everything is placed again only when the ground, the art
//! or the owners change.
//!
//! The records are the types of B1's `entities` (`UnitEntity`, `CityEntity`, `PlotDecor`): WOB0
//! moves the code without changing the picture; the richer states of spec 4.2 (`facing`,
//! `UnitVisFlags`, `CityState.buildings`) come with WOB2 and WOB5.

use std::collections::{BTreeMap, BTreeSet};

use crate::entities::{CityEntity, Owners, PlotDecor, UnitEntity};
pub use crate::entities::{CityKey, PlotKey, UnitKey, plot_key};
use crate::world::unit::place::UnitVisExtra;

/// A keyed collection with the keys to place again.
///
/// `upsert` and `remove` say whether the record really changed: a record equal to the one held is
/// not a change (a feed may send a key twice).
#[derive(Debug, Clone)]
pub struct Keyed<K: Ord + Copy, V: PartialEq> {
    records: BTreeMap<K, V>,
    dirty: BTreeSet<K>,
}

impl<K: Ord + Copy, V: PartialEq> Default for Keyed<K, V> {
    fn default() -> Self {
        Self {
            records: BTreeMap::new(),
            dirty: BTreeSet::new(),
        }
    }
}

impl<K: Ord + Copy, V: PartialEq> Keyed<K, V> {
    /// Stores `value` under `key`; the old record, if there was one, and whether anything changed.
    pub fn upsert(&mut self, key: K, value: V) -> (Option<V>, bool) {
        if self.records.get(&key) == Some(&value) {
            return (None, false);
        }
        let old = self.records.insert(key, value);
        self.dirty.insert(key);
        (old, true)
    }

    /// Drops the record of `key`; it, if there was one.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        let old = self.records.remove(key);
        if old.is_some() {
            self.dirty.remove(key);
        }
        old
    }

    pub fn clear(&mut self) {
        self.records.clear();
        self.dirty.clear();
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.records.get(key)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.records.iter()
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.records.keys()
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.records.values()
    }

    /// Marks `key` to be placed again.
    pub fn touch(&mut self, key: K) {
        if self.records.contains_key(&key) {
            self.dirty.insert(key);
        }
    }

    /// Marks every record.
    pub fn touch_all(&mut self) {
        self.dirty.extend(self.records.keys().copied());
    }

    /// The keys to place again, once.
    pub fn take_dirty(&mut self) -> Vec<K> {
        std::mem::take(&mut self.dirty).into_iter().collect()
    }

    pub fn dirty_len(&self) -> usize {
        self.dirty.len()
    }
}

/// What a subsystem shows in `?stats=1`: how much it holds, how much it placed
/// and what it hands to the draw.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ObjectCounters {
    /// Records held.
    pub records: u32,
    /// Records placed in the last refresh (the others kept their placement).
    pub placed_last: u32,
    /// Records placed since the start.
    pub placed_total: u64,
    /// Instances in the last assembly.
    pub instances: u32,
    /// Models in the last assembly: one draw per model and copy of the map.
    pub batches: u32,
}

impl ObjectCounters {
    /// `records 12, placed 1 (total 40), instances 30, batches 4`.
    pub fn line(&self) -> String {
        format!(
            "records {}, placed {} (total {}), instances {}, batches {}",
            self.records, self.placed_last, self.placed_total, self.instances, self.batches
        )
    }
}

/// A batch of changes of the keyed inputs (`Renderer::apply_entity_changes`): what a feed of the
/// bridge hands over after it compared the view with its last copy.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EntityChanges {
    /// New colours, cultures or eras of the owners.
    pub owners: Option<Owners>,
    /// Drop every unit, city and plot first (a new game); the rest is applied after.
    pub clear: bool,
    /// Units that are new or changed.
    pub units: Vec<UnitEntity>,
    pub units_removed: Vec<UnitKey>,
    /// What the view says of a unit beyond its record: facing, flags, what a worker builds (WOB2).
    /// A unit not named here keeps what it had; the default value clears it.
    pub unit_extras: Vec<(UnitKey, UnitVisExtra)>,
    pub cities: Vec<CityEntity>,
    pub cities_removed: Vec<CityKey>,
    /// Plots with something on them (`Some`) and plots that were cleared (`None`).
    pub plots: Vec<(PlotKey, Option<PlotDecor>)>,
    /// The technologies of the landmark art list by type name, in the bit order of
    /// `PlayerView.art_techs` (a new catalog, WOB4).
    pub art_techs: Option<Vec<String>>,
}

impl EntityChanges {
    pub fn is_empty(&self) -> bool {
        self.art_techs.is_none()
            && self.owners.is_none()
            && !self.clear
            && self.units.is_empty()
            && self.units_removed.is_empty()
            && self.unit_extras.is_empty()
            && self.cities.is_empty()
            && self.cities_removed.is_empty()
            && self.plots.is_empty()
    }

    /// Adds `other` after `self`: a later record of a key wins.
    pub fn merge(&mut self, other: EntityChanges) {
        if other.clear {
            *self = EntityChanges {
                owners: other.owners.or(self.owners.take()),
                art_techs: other.art_techs.or(self.art_techs.take()),
                ..other
            };
            return;
        }
        self.owners = other.owners.or(self.owners.take());
        self.art_techs = other.art_techs.or(self.art_techs.take());
        for key in other.units_removed {
            self.units.retain(|u| (u.owner, u.id) != key);
            self.unit_extras.retain(|(k, _)| *k != key);
            self.units_removed.push(key);
        }
        for (key, extra) in other.unit_extras {
            self.unit_extras.retain(|(k, _)| *k != key);
            self.unit_extras.push((key, extra));
        }
        for u in other.units {
            let key = (u.owner, u.id);
            self.units.retain(|x| (x.owner, x.id) != key);
            self.units_removed.retain(|k| *k != key);
            self.units.push(u);
        }
        for key in other.cities_removed {
            self.cities.retain(|c| (c.owner, c.id) != key);
            self.cities_removed.push(key);
        }
        for c in other.cities {
            let key = (c.owner, c.id);
            self.cities.retain(|x| (x.owner, x.id) != key);
            self.cities_removed.retain(|k| *k != key);
            self.cities.push(c);
        }
        for (key, decor) in other.plots {
            self.plots.retain(|(k, _)| *k != key);
            self.plots.push((key, decor));
        }
    }
}

/// A subsystem's counters under its name, in `FrameStats::objects`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectStat {
    pub system: crate::frame::SystemId,
    pub counters: ObjectCounters,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_equal_record_is_not_a_change() {
        let mut k: Keyed<u32, i32> = Keyed::default();
        assert_eq!(k.upsert(1, 5), (None, true));
        assert_eq!(k.take_dirty(), [1]);
        assert_eq!(k.upsert(1, 5), (None, false));
        assert!(k.take_dirty().is_empty());
        assert_eq!(k.upsert(1, 6), (Some(5), true));
        assert_eq!(k.take_dirty(), [1]);
    }

    #[test]
    fn a_removed_key_is_not_placed_again() {
        let mut k: Keyed<u32, i32> = Keyed::default();
        k.upsert(1, 5);
        assert_eq!(k.remove(&1), Some(5));
        assert!(k.take_dirty().is_empty());
        assert_eq!(k.remove(&1), None);
    }

    #[test]
    fn plot_keys_sort_row_major() {
        let mut keys = vec![
            plot_key(3, 1),
            plot_key(0, 2),
            plot_key(7, 0),
            plot_key(2, 1),
        ];
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                plot_key(7, 0),
                plot_key(2, 1),
                plot_key(3, 1),
                plot_key(0, 2)
            ]
        );
    }
}
