//! The ribbon of the cities (WOB0, world-objects 4.2): cities of the view -> keyed changes of the
//! renderer's `City` subsystem, as the [`CitiesFeed`] (R4.10). Like `units.rs`.

use std::collections::{HashMap, HashSet};

use client_core::ViewChange;
use protocol::{Catalog, CityView, SectionMask, WorldView};
use render::Renderer;
use render::entities::{CityEntity, CityKey};
use render::world::objects::input::EntityChanges;

use super::entities::{EntityTable, flush_entity_changes};
use super::{FeedCx, ViewFeed, mask, section};

/// The cities as the renderer has them: what the ribbon sent last.
#[derive(Debug, Default)]
pub struct CitiesMirror {
    sent: HashMap<CityKey, CityEntity>,
}

impl CitiesMirror {
    /// The cities sent so far, in no order.
    pub fn cities(&self) -> impl Iterator<Item = &CityEntity> {
        self.sent.values()
    }

    fn send(&mut self, city: CityEntity, out: &mut EntityChanges) {
        let key = (city.owner, city.id);
        if self.sent.get(&key) != Some(&city) {
            self.sent.insert(key, city.clone());
            out.cities.push(city);
        }
    }

    fn drop_key(&mut self, key: CityKey, out: &mut EntityChanges) {
        if self.sent.remove(&key).is_some() {
            out.cities_removed.push(key);
        }
    }
}

/// Adds to `out` what changed in the cities (see `unit_changes`).
pub fn city_changes(
    world: &WorldView,
    table: &EntityTable,
    change: &ViewChange,
    mirror: &mut CitiesMirror,
    out: &mut EntityChanges,
) {
    match (&change.dirty_cities, change.reset) {
        (Some(keys), false) => {
            for key in &keys.remove {
                mirror.drop_key(*key, out);
            }
            for key in &keys.upsert {
                let view: Option<&CityView> = world.cities.iter().find(|c| (c.owner, c.id) == *key);
                match view {
                    Some(c) => mirror.send(table.city_entity(c), out),
                    None => mirror.drop_key(*key, out),
                }
            }
        }
        _ => {
            let mut seen = HashSet::with_capacity(world.cities.len());
            for c in &world.cities {
                seen.insert((c.owner, c.id));
                mirror.send(table.city_entity(c), out);
            }
            let gone: Vec<CityKey> = mirror
                .sent
                .keys()
                .filter(|k| !seen.contains(k))
                .copied()
                .collect();
            for key in gone {
                mirror.drop_key(key, out);
            }
        }
    }
}

/// The feed of the cities (net-client 4.10, R4.10): reads the section `CITIES` and sends the cities
/// that are new, changed or gone (`dirty_cities` of the change, else the whole section).
#[derive(Debug, Default)]
pub struct CitiesFeed {
    table: EntityTable,
    mirror: CitiesMirror,
    /// Changes that `apply` made and `flush` has not handed over yet.
    pub(super) pending: EntityChanges,
}

impl CitiesFeed {
    /// The cities sent so far (tests, tools).
    pub fn cities(&self) -> impl Iterator<Item = &CityEntity> {
        self.mirror.cities()
    }

    /// Changes waiting for `flush`.
    pub fn pending(&self) -> &EntityChanges {
        &self.pending
    }
}

impl ViewFeed for CitiesFeed {
    fn sections(&self) -> SectionMask {
        mask(&[section::CITIES])
    }

    fn on_catalog(&mut self, catalog: &Catalog) {
        self.table = EntityTable::from_catalog(catalog);
    }

    fn apply(&mut self, view: &WorldView, change: &ViewChange, _cx: &mut FeedCx<'_>) {
        let mut out = EntityChanges::default();
        city_changes(view, &self.table, change, &mut self.mirror, &mut out);
        self.pending.merge(out);
    }

    fn flush(&mut self, renderer: &mut Renderer) {
        flush_entity_changes(&mut self.pending, renderer);
    }
}

#[cfg(test)]
mod tests {
    use client_core::KeyChanges;

    use super::super::tests::fixture_world;
    use super::*;

    #[test]
    fn only_the_city_that_grew_is_sent() {
        let (world, catalog) = fixture_world();
        let table = EntityTable::from_catalog(&catalog);
        let mut m = CitiesMirror::default();
        let mut out = EntityChanges::default();
        city_changes(&world, &table, &ViewChange::all(), &mut m, &mut out);
        assert_eq!(out.cities.len(), world.cities.len());

        let mut next = world.clone();
        next.cities[0].population += 1;
        let key = (next.cities[0].owner, next.cities[0].id);
        let change = ViewChange {
            dirty_cities: Some(KeyChanges {
                upsert: vec![key],
                remove: vec![],
            }),
            ..ViewChange::default()
        };
        let mut out = EntityChanges::default();
        city_changes(&next, &table, &change, &mut m, &mut out);
        assert_eq!(out.cities.len(), 1);
        assert_eq!(out.cities[0].population, next.cities[0].population as i32);

        // The whole section again: nothing differs from the copy.
        let mut out = EntityChanges::default();
        city_changes(&next, &table, &ViewChange::all(), &mut m, &mut out);
        assert!(out.is_empty());
    }
}
