//! The ribbon of the units (WOB0, world-objects 4.2): units of the view -> keyed changes of the
//! renderer's `Unit` subsystem, as the [`UnitsFeed`] (R4.10).
//!
//! It compares the view with the copy it sent last time and gives only the units that are new,
//! changed or gone.

use std::collections::HashMap;

use client_core::ViewChange;
use protocol::{Catalog, SectionMask, UnitView, WorldView};
use render::Renderer;
use render::entities::{UnitEntity, UnitKey};
use render::world::objects::input::EntityChanges;
use render::world::unit::place::UnitVisExtra;

use super::entities::{EntityTable, flush_entity_changes};
use super::{FeedCx, ViewFeed, mask, section};

/// The units as the renderer has them: what the ribbon sent last.
#[derive(Debug, Default)]
pub struct UnitsMirror {
    sent: HashMap<UnitKey, UnitEntity>,
    /// What was sent of facing, flags and the work of each unit; a unit with the default is absent.
    sent_extras: HashMap<UnitKey, UnitVisExtra>,
}

impl UnitsMirror {
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.sent.len()
    }

    /// The units sent so far, in no order.
    pub fn units(&self) -> impl Iterator<Item = &UnitEntity> {
        self.sent.values()
    }

    fn send(&mut self, unit: UnitEntity, extra: UnitVisExtra, out: &mut EntityChanges) {
        let key = (unit.owner, unit.id);
        if self.sent.get(&key) != Some(&unit) {
            self.sent.insert(key, unit.clone());
            out.units.push(unit);
        }
        let had = self.sent_extras.get(&key).copied().unwrap_or_default();
        if had != extra {
            if extra == UnitVisExtra::default() {
                self.sent_extras.remove(&key);
            } else {
                self.sent_extras.insert(key, extra);
            }
            out.unit_extras.push((key, extra));
        }
    }

    fn drop_key(&mut self, key: UnitKey, out: &mut EntityChanges) {
        self.sent_extras.remove(&key);
        if self.sent.remove(&key).is_some() {
            out.units_removed.push(key);
        }
    }
}

/// Adds to `out` what changed in the units. `change.dirty_units` names the keys of a diff; without
/// it (a keyframe, a new catalog) the whole section is compared with the copy.
pub fn unit_changes(
    world: &WorldView,
    table: &EntityTable,
    change: &ViewChange,
    mirror: &mut UnitsMirror,
    out: &mut EntityChanges,
) {
    match (&change.dirty_units, change.reset) {
        (Some(keys), false) => {
            for key in &keys.remove {
                mirror.drop_key(*key, out);
            }
            let index: HashMap<UnitKey, &UnitView> = if keys.upsert.len() > 8 {
                world.units.iter().map(|u| ((u.owner, u.id), u)).collect()
            } else {
                HashMap::new()
            };
            for key in &keys.upsert {
                let view = if index.is_empty() {
                    world.units.iter().find(|u| (u.owner, u.id) == *key)
                } else {
                    index.get(key).copied()
                };
                match view {
                    Some(u) => mirror.send(table.unit_entity(u), table.unit_extra(u), out),
                    // Named but not in the view: it is gone.
                    None => mirror.drop_key(*key, out),
                }
            }
        }
        _ => {
            let mut seen = std::collections::HashSet::with_capacity(world.units.len());
            for u in &world.units {
                seen.insert((u.owner, u.id));
                mirror.send(table.unit_entity(u), table.unit_extra(u), out);
            }
            let gone: Vec<UnitKey> = mirror
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

/// The feed of the units (net-client 4.10, R4.10): reads the section `UNITS` and sends the units
/// that are new, changed or gone (`dirty_units` of the change, else the whole section).
#[derive(Debug, Default)]
pub struct UnitsFeed {
    table: EntityTable,
    mirror: UnitsMirror,
    /// Changes that `apply` made and `flush` has not handed over yet.
    pub(super) pending: EntityChanges,
}

impl UnitsFeed {
    /// The units sent so far (tests, tools).
    pub fn units(&self) -> impl Iterator<Item = &UnitEntity> {
        self.mirror.units()
    }

    /// Changes waiting for `flush`.
    pub fn pending(&self) -> &EntityChanges {
        &self.pending
    }
}

impl ViewFeed for UnitsFeed {
    fn sections(&self) -> SectionMask {
        mask(&[section::UNITS])
    }

    fn on_catalog(&mut self, catalog: &Catalog) {
        self.table = EntityTable::from_catalog(catalog);
    }

    fn apply(&mut self, view: &WorldView, change: &ViewChange, _cx: &mut FeedCx<'_>) {
        let mut out = EntityChanges::default();
        unit_changes(view, &self.table, change, &mut self.mirror, &mut out);
        self.pending.merge(out);
    }

    fn flush(&mut self, renderer: &mut Renderer) {
        flush_entity_changes(&mut self.pending, renderer);
    }
}

#[cfg(test)]
mod tests {
    use client_core::KeyChanges;
    use render::hexgrid::Dir;

    use super::super::tests::fixture_world;
    use super::*;

    fn all(world: &WorldView, table: &EntityTable, m: &mut UnitsMirror) -> EntityChanges {
        let mut out = EntityChanges::default();
        unit_changes(world, table, &ViewChange::all(), m, &mut out);
        out
    }

    #[test]
    fn a_keyframe_sends_every_unit_once_and_then_nothing() {
        let (world, catalog) = fixture_world();
        let table = EntityTable::from_catalog(&catalog);
        let mut m = UnitsMirror::default();
        let first = all(&world, &table, &mut m);
        assert_eq!(first.units.len(), world.units.len());
        assert!(first.units_removed.is_empty());
        assert!(
            all(&world, &table, &mut m).is_empty(),
            "same view, no change"
        );
    }

    #[test]
    fn a_diff_sends_the_named_keys_that_differ_and_the_removals() {
        let (world, catalog) = fixture_world();
        let table = EntityTable::from_catalog(&catalog);
        let mut m = UnitsMirror::default();
        all(&world, &table, &mut m);

        let mut next = world.clone();
        let gone = next.units.pop().unwrap();
        next.units[0].x += 1;
        let moved = (next.units[0].owner, next.units[0].id);
        let change = ViewChange {
            dirty_units: Some(KeyChanges {
                upsert: vec![moved],
                remove: vec![(gone.owner, gone.id)],
            }),
            ..ViewChange::default()
        };
        let mut out = EntityChanges::default();
        unit_changes(&next, &table, &change, &mut m, &mut out);
        assert_eq!(out.units.len(), 1);
        assert_eq!((out.units[0].owner, out.units[0].id), moved);
        assert_eq!(out.units_removed, [(gone.owner, gone.id)]);
        assert_eq!(m.len(), next.units.len());

        // A named key whose unit is identical sends nothing.
        let mut out = EntityChanges::default();
        unit_changes(&next, &table, &change, &mut m, &mut out);
        assert!(out.units.is_empty() && out.units_removed.is_empty());
    }

    #[test]
    fn a_keyframe_drops_the_units_that_are_gone() {
        let (world, catalog) = fixture_world();
        let table = EntityTable::from_catalog(&catalog);
        let mut m = UnitsMirror::default();
        all(&world, &table, &mut m);
        let mut next = world.clone();
        let gone = next.units.remove(0);
        let out = all(&next, &table, &mut m);
        assert!(out.units.is_empty());
        assert_eq!(out.units_removed, [(gone.owner, gone.id)]);
    }

    /// A diff naming one unit (`upsert`, or `remove` for the unit being gone).
    fn named(upsert: Vec<UnitKey>, remove: Vec<UnitKey>) -> ViewChange {
        ViewChange {
            dirty_units: Some(KeyChanges { upsert, remove }),
            ..ViewChange::default()
        }
    }

    #[test]
    fn a_changed_facing_sends_the_extra_once() {
        let (world, catalog) = fixture_world();
        let table = EntityTable::from_catalog(&catalog);
        let mut m = UnitsMirror::default();
        all(&world, &table, &mut m);

        let mut next = world.clone();
        next.units[0].facing = 2;
        next.units[0].building = 5;
        let key = (next.units[0].owner, next.units[0].id);
        let mut out = EntityChanges::default();
        unit_changes(&next, &table, &named(vec![key], vec![]), &mut m, &mut out);
        assert_eq!(out.unit_extras.len(), 1);
        let (sent_key, extra) = out.unit_extras[0];
        assert_eq!(sent_key, key);
        assert_eq!(extra.facing, Some(Dir::SouthEast));
        assert_eq!(extra.building, Some(5));

        // The same view named again: the extra is already there, nothing is sent.
        let mut out = EntityChanges::default();
        unit_changes(&next, &table, &named(vec![key], vec![]), &mut m, &mut out);
        assert!(out.unit_extras.is_empty());
    }

    #[test]
    fn a_removed_unit_forgets_its_extra() {
        let (world, catalog) = fixture_world();
        let table = EntityTable::from_catalog(&catalog);
        let mut m = UnitsMirror::default();
        all(&world, &table, &mut m);

        let mut next = world.clone();
        next.units[0].facing = 2;
        next.units[0].building = 5;
        let key = (next.units[0].owner, next.units[0].id);
        let mut out = EntityChanges::default();
        unit_changes(&next, &table, &named(vec![key], vec![]), &mut m, &mut out);
        assert_eq!(out.unit_extras.len(), 1);

        // The unit leaves the view.
        let mut without = next.clone();
        without.units.remove(0);
        let mut out = EntityChanges::default();
        unit_changes(
            &without,
            &table,
            &named(vec![], vec![key]),
            &mut m,
            &mut out,
        );
        assert_eq!(out.units_removed, [key]);

        // It comes back with the same facing: the extra is sent again.
        let mut out = EntityChanges::default();
        unit_changes(&next, &table, &named(vec![key], vec![]), &mut m, &mut out);
        assert_eq!(out.units.len(), 1);
        assert_eq!(out.unit_extras.len(), 1);
        let (sent_key, extra) = out.unit_extras[0];
        assert_eq!(sent_key, key);
        assert_eq!(extra.facing, Some(Dir::SouthEast));
        assert_eq!(extra.building, Some(5));
    }
}
