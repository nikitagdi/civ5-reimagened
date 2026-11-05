//! The ribbon of the plots (WOB0, world-objects 4.2): what lies on the plots (resource,
//! improvement, route) -> keyed changes of the renderer's `Landmark` subsystem, as the [`PlotsFeed`]
//! (R4.10). Like `units.rs`.

use std::collections::{HashMap, HashSet};

use client_core::ViewChange;
use protocol::{Catalog, SectionMask, WorldView};
use render::Renderer;
use render::entities::{PlotDecor, PlotKey, plot_key};
use render::world::objects::input::EntityChanges;

use super::entities::{EntityTable, flush_entity_changes};
use super::{FeedCx, ViewFeed, mask, section};

/// The plots with something on them as the renderer has them: what the ribbon sent last.
#[derive(Debug, Default)]
pub struct PlotsMirror {
    sent: HashMap<PlotKey, PlotDecor>,
}

impl PlotsMirror {
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.sent.len()
    }

    /// The plots sent so far, in row-major order.
    pub fn plots(&self) -> Vec<&PlotDecor> {
        let mut all: Vec<(&PlotKey, &PlotDecor)> = self.sent.iter().collect();
        all.sort_by_key(|(k, _)| **k);
        all.into_iter().map(|(_, p)| p).collect()
    }

    /// Sends `decor` for the plot at `key` if it differs from the copy; `None` clears the plot.
    fn send(&mut self, key: PlotKey, decor: Option<PlotDecor>, out: &mut EntityChanges) {
        match decor {
            Some(d) => {
                if self.sent.get(&key) != Some(&d) {
                    self.sent.insert(key, d.clone());
                    out.plots.push((key, Some(d)));
                }
            }
            None => {
                if self.sent.remove(&key).is_some() {
                    out.plots.push((key, None));
                }
            }
        }
    }
}

/// Adds to `out` what changed on the plots. `change.dirty_plots` lists the indices of a diff;
/// without it (a keyframe, a new catalog, a new map) every plot is compared with the copy.
pub fn plot_changes(
    world: &WorldView,
    table: &EntityTable,
    change: &ViewChange,
    mirror: &mut PlotsMirror,
    out: &mut EntityChanges,
) {
    let width = usize::from(world.map.width).max(1);
    let key_of = |i: usize| plot_key((i % width) as i32, (i / width) as i32);
    match (&change.dirty_plots, change.reset) {
        (Some(dirty), false) => {
            for &i in dirty {
                let i = i as usize;
                let decor = world
                    .map
                    .plots
                    .get(i)
                    .and_then(|p| table.plot_decor(p, i, width));
                mirror.send(key_of(i), decor, out);
            }
        }
        _ => {
            let mut seen = HashSet::new();
            for (i, p) in world.map.plots.iter().enumerate() {
                let decor = table.plot_decor(p, i, width);
                if decor.is_some() {
                    seen.insert(key_of(i));
                }
                mirror.send(key_of(i), decor, out);
            }
            let gone: Vec<PlotKey> = mirror
                .sent
                .keys()
                .filter(|k| !seen.contains(k))
                .copied()
                .collect();
            for key in gone {
                mirror.send(key, None, out);
            }
        }
    }
}

/// The feed of what lies on the plots (net-client 4.10, R4.10): reads the section `PLOTS` and sends
/// the plots whose decor changed (`dirty_plots` of the change, else every plot is compared).
#[derive(Debug, Default)]
pub struct PlotsFeed {
    table: EntityTable,
    mirror: PlotsMirror,
    /// Changes that `apply` made and `flush` has not handed over yet.
    pub(super) pending: EntityChanges,
}

impl PlotsFeed {
    /// The plots sent so far, in row-major order (tests, tools).
    pub fn plots(&self) -> Vec<&PlotDecor> {
        self.mirror.plots()
    }

    /// Changes waiting for `flush`.
    pub fn pending(&self) -> &EntityChanges {
        &self.pending
    }
}

impl ViewFeed for PlotsFeed {
    fn sections(&self) -> SectionMask {
        mask(&[section::PLOTS])
    }

    fn on_catalog(&mut self, catalog: &Catalog) {
        self.table = EntityTable::from_catalog(catalog);
    }

    fn apply(&mut self, view: &WorldView, change: &ViewChange, _cx: &mut FeedCx<'_>) {
        let mut out = EntityChanges::default();
        plot_changes(view, &self.table, change, &mut self.mirror, &mut out);
        self.pending.merge(out);
    }

    fn flush(&mut self, renderer: &mut Renderer) {
        flush_entity_changes(&mut self.pending, renderer);
    }
}

#[cfg(test)]
mod tests {
    use protocol::PlotFlags;

    use super::super::tests::fixture_world;
    use super::*;

    #[test]
    fn a_keyframe_sends_the_decorated_plots_and_a_diff_only_the_dirty_ones() {
        let (mut world, catalog) = fixture_world();
        let table = EntityTable::from_catalog(&catalog);
        let mut m = PlotsMirror::default();
        let mut out = EntityChanges::default();
        plot_changes(&world, &table, &ViewChange::all(), &mut m, &mut out);
        let n = m.len();
        assert!(n > 0 && out.plots.len() == n);

        // A road on a revealed plot that has none; the other plots are not looked at.
        let at = world
            .map
            .plots
            .iter()
            .enumerate()
            .position(|(i, p)| {
                p.flags.contains(PlotFlags::REVEALED)
                    && p.route < 0
                    && table
                        .plot_decor(p, i, usize::from(world.map.width))
                        .is_none()
            })
            .unwrap();
        let road = world.map.plots.iter().find(|p| p.route >= 0).unwrap().route;
        world.map.plots[at].route = road;
        let change = ViewChange {
            dirty_plots: Some(vec![at as u32]),
            ..ViewChange::default()
        };
        let mut out = EntityChanges::default();
        plot_changes(&world, &table, &change, &mut m, &mut out);
        assert_eq!(out.plots.len(), 1);
        assert_eq!(m.len(), n + 1);

        // The plot loses its reveal: it is cleared.
        world.map.plots[at].flags = PlotFlags(0);
        let mut out = EntityChanges::default();
        plot_changes(&world, &table, &change, &mut m, &mut out);
        assert_eq!(out.plots.len(), 1);
        assert!(out.plots[0].1.is_none());
        assert_eq!(m.len(), n);

        // A keyframe of the same world sends nothing.
        let mut out = EntityChanges::default();
        plot_changes(&world, &table, &ViewChange::all(), &mut m, &mut out);
        assert!(out.is_empty());
    }
}
