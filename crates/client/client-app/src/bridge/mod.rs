//! The bridge from the client's view to the renderer (net-client 4.10, NC-D12): a set of feeds.
//!
//! `render` does not know `protocol` (R6), so the conversion lives here. [`ViewChange`] says which sections
//! and cells changed; a feed reads only its own sections and is not called when they did not change. A
//! subsystem of track R adds a file `bridge/<name>.rs` with a [`ViewFeed`] and one line to
//! [`WorldBridge::each_feed`]; nothing else changes (not `client-core`, not the other feeds).
//!
//! The frame is two steps: [`WorldBridge::sync`] converts on the CPU (the feeds' `apply`) and
//! [`WorldBridge::flush`] hands the results to the renderer (the feeds' `flush`), for the feeds that
//! were applied.

mod cities;
mod entities;
mod fog;
mod owners;
mod plots;
mod terrain;
mod trade;
mod units;

use client_core::ViewChange;
use protocol::{Catalog, MapBasis, SectionMask, WorldView};
use render::{GameCamera, Renderer};

pub use cities::CitiesFeed;
pub use entities::{EntityTable, PlayersFeed, entity_snapshot};
pub use fog::{FogFeed, fog_input, plot_fog};
pub use owners::{OwnersFeed, owner_colors, owners_input};
pub use plots::PlotsFeed;
pub use terrain::{TerrainChange, TerrainFeed, TerrainTable, terrain_snapshot};
pub use trade::{TradeFeed, trade_input};
pub use units::UnitsFeed;

/// Section numbers of `ViewDiff::touched` (net-protocol 4.6), for `ViewFeed::sections`.
pub mod section {
    pub const HEADER: u8 = 0;
    pub const PLOTS: u8 = 1;
    pub const UNITS: u8 = 2;
    pub const CITIES: u8 = 3;
    pub const PLAYERS: u8 = 4;
    pub const TOP_BAR: u8 = 5;
    pub const NOTIFICATIONS: u8 = 6;
    pub const PEACE_OFFERS: u8 = 7;
    pub const BASIS: u8 = 8;
    pub const TRADE_ROUTES: u8 = 9;
}

/// A mask of the given section numbers.
pub const fn mask(sections: &[u8]) -> SectionMask {
    let mut bits = 0;
    let mut i = 0;
    while i < sections.len() {
        bits |= 1 << sections[i];
        i += 1;
    }
    SectionMask(bits)
}

/// What a feed gets besides the view and the change.
pub struct FeedCx<'a> {
    /// The map basis in page memory (`ClientCore::basis`): static relief, rivers, natural wonders. `None`
    /// until it came (and on the B1 path, which has none).
    pub basis: Option<&'a MapBasis>,
    pub camera: &'a mut GameCamera,
}

/// One feed: a converter from some sections of the view to one subsystem of the renderer.
pub trait ViewFeed {
    /// The sections the feed reads. `apply` is called only when the change touches one of them (or is a
    /// reset).
    fn sections(&self) -> SectionMask;
    /// Names of the game's catalog; arrives before the first view. The feed converts everything again on
    /// its next `apply`.
    fn on_catalog(&mut self, catalog: &Catalog);
    /// CPU part: convert. `change` says what to look at; a feed that cannot use `dirty_*` compares its
    /// sections whole.
    fn apply(&mut self, view: &WorldView, change: &ViewChange, cx: &mut FeedCx<'_>);
    /// Renderer part: hand over what `apply` produced. Does nothing if there is nothing new.
    fn flush(&mut self, renderer: &mut Renderer);
}

/// Numbers of the feeds in [`WorldBridge::each_feed`] (bits of `applied`).
const FEED_TERRAIN: usize = 0;
const FEED_FOG: usize = 1;
const FEED_OWNERS: usize = 2;
const FEED_PLAYERS: usize = 3;
const FEED_UNITS: usize = 4;
const FEED_CITIES: usize = 5;
const FEED_PLOTS: usize = 6;
const FEED_TRADE: usize = 7;
/// Every feed.
#[cfg(test)]
const ALL_FEEDS: u32 = 0b1111_1111;

/// Keeps the renderer's world in step with the client's view.
#[derive(Debug, Default)]
pub struct WorldBridge {
    terrain: TerrainFeed,
    fog: FogFeed,
    owners: OwnersFeed,
    players: PlayersFeed,
    units: UnitsFeed,
    cities: CitiesFeed,
    plots: PlotsFeed,
    trade: TradeFeed,
    /// Bit n: feed n was applied and awaits `flush`.
    applied: u32,
    /// The catalog changed: the next `sync` runs every feed whole.
    catalog_changed: bool,
}

impl WorldBridge {
    pub fn new() -> Self {
        Self::default()
    }

    /// THE LIST OF FEEDS. Order is the order of `apply` and `flush`: the units stand on the terrain,
    /// and the fog and the borders go after the terrain that seeds them.
    fn each_feed(&mut self, mut f: impl FnMut(usize, &mut dyn ViewFeed)) {
        f(FEED_TERRAIN, &mut self.terrain);
        f(FEED_FOG, &mut self.fog);
        f(FEED_OWNERS, &mut self.owners);
        f(FEED_PLAYERS, &mut self.players);
        f(FEED_UNITS, &mut self.units);
        f(FEED_CITIES, &mut self.cities);
        f(FEED_PLOTS, &mut self.plots);
        f(FEED_TRADE, &mut self.trade);
    }

    /// Names come with the catalog (`CatalogReady`), before the first view.
    pub fn set_catalog(&mut self, catalog: &Catalog) {
        self.each_feed(|_, feed| feed.on_catalog(catalog));
        self.catalog_changed = true;
    }

    /// CPU step: runs the feeds whose sections the change touches. `basis` is `ClientCore::basis()`.
    pub fn sync(
        &mut self,
        view: &WorldView,
        basis: Option<&MapBasis>,
        change: &ViewChange,
        camera: &mut GameCamera,
    ) {
        let all;
        let change = if std::mem::take(&mut self.catalog_changed) {
            all = ViewChange {
                basis_plots: None,
                ..ViewChange::all()
            };
            &all
        } else {
            change
        };
        let mut applied = 0;
        let mut cx = FeedCx { basis, camera };
        self.each_feed(|n, feed| {
            if change.reset || feed.sections().0 & change.sections.0 != 0 {
                feed.apply(view, change, &mut cx);
                applied |= 1 << n;
            }
        });
        self.applied |= applied;
    }

    /// Renderer step: the feeds that were applied since the last flush hand over their data.
    pub fn flush(&mut self, renderer: &mut Renderer) {
        let applied = std::mem::take(&mut self.applied);
        self.each_feed(|n, feed| {
            if applied & (1 << n) != 0 {
                feed.flush(renderer);
            }
        });
    }

    /// The trade route paths of the replica core (`Q_TRADE_PATHS`): the routes of the view are laid on them
    /// at the next `flush` when they changed.
    pub fn set_trade_paths(&mut self, paths: Vec<trade::CorePath>, ids: Vec<i32>) {
        if self.trade.set_core_paths(paths) | self.trade.set_core_ids(ids) {
            self.applied |= 1 << FEED_TRADE;
        }
    }

    /// The popup route of the trade visuals (the caravan's target choice, the trade unit selected or hovered).
    pub fn set_trade_popup(&mut self, sel: Option<exe_api::PopupRoute>) {
        if self.trade.set_popup(sel) {
            self.applied |= 1 << FEED_TRADE;
        }
    }

    /// Routes of the view the core has given no path for yet (drawn on a hex line, SIMP-069).
    pub fn trade_routes_without_path(&self) -> usize {
        self.trade.routes_without_path()
    }

    /// The current terrain snapshot and its mesh (picking, labels).
    pub fn terrain(
        &self,
    ) -> Option<(
        &render::terrain::TerrainSnapshot,
        &render::terrain::TerrainMesh,
    )> {
        self.terrain.terrain()
    }

    /// How many times the terrain mesh was generated (tests, `?stats=1`, the pick index).
    pub fn mesh_builds(&self) -> u32 {
        self.terrain.mesh_builds()
    }

    /// The terrain feed, for tests and tools.
    pub fn terrain_feed(&self) -> &TerrainFeed {
        &self.terrain
    }

    /// The unit and city models the renderer was given, as `[owner, id, x, y]` sorted (the test port: the playtest
    /// compares them with the replica, live report 2026-10-30).
    pub fn drawn(&self) -> (Vec<[i32; 4]>, Vec<[i32; 4]>) {
        let mut units: Vec<[i32; 4]> = self
            .units
            .units()
            .map(|u| [i32::from(u.owner), u.id, u.x, u.y])
            .collect();
        let mut cities: Vec<[i32; 4]> = self
            .cities
            .cities()
            .map(|c| [i32::from(c.owner), c.id, c.x, c.y])
            .collect();
        units.sort_unstable();
        cities.sort_unstable();
        (units, cities)
    }
}

#[cfg(test)]
mod tests {
    use crate::test_world;

    use super::*;

    pub(crate) fn fixture_world() -> (WorldView, Catalog) {
        test_world::world_and_catalog()
    }

    /// The same world on the map that wraps along X.
    pub(crate) fn wrapped_world() -> (WorldView, Catalog) {
        (test_world::world_wrapped(), test_world::catalog())
    }
    fn changed(sections: &[u8]) -> ViewChange {
        ViewChange {
            sections: mask(sections),
            ..ViewChange::default()
        }
    }

    #[test]
    fn mask_collects_section_numbers() {
        assert_eq!(mask(&[0, 2, 8]), SectionMask(1 | 4 | 256));
        assert_eq!(mask(&[]), SectionMask(0));
    }

    #[test]
    fn a_feed_runs_only_when_its_sections_changed() {
        let (world, catalog) = fixture_world();
        let mut bridge = WorldBridge::new();
        bridge.set_catalog(&catalog);
        let mut camera = GameCamera::default();
        bridge.sync(&world, None, &ViewChange::all(), &mut camera);
        assert_eq!(bridge.applied, ALL_FEEDS);
        bridge.applied = 0;
        // Notifications are read by no feed.
        bridge.sync(
            &world,
            None,
            &changed(&[section::NOTIFICATIONS]),
            &mut camera,
        );
        assert_eq!(bridge.applied, 0);
        // Units: the units feed alone, not the terrain, the fog, the borders, the cities or the plots.
        bridge.sync(&world, None, &changed(&[section::UNITS]), &mut camera);
        assert_eq!(bridge.applied, 1 << FEED_UNITS);
        bridge.applied = 0;
        // Cities: theirs and the trade routes' (the type reads the city at the end of a route).
        bridge.sync(&world, None, &changed(&[section::CITIES]), &mut camera);
        assert_eq!(bridge.applied, (1 << FEED_CITIES) | (1 << FEED_TRADE));
        bridge.applied = 0;
        // The header (every turn changes it): the terrain reads it, nobody else (not the trade routes).
        bridge.sync(&world, None, &changed(&[section::HEADER]), &mut camera);
        assert_eq!(bridge.applied, 1 << FEED_TERRAIN);
        bridge.applied = 0;
        // Plots: the terrain, the fog, the borders and the landmarks.
        bridge.sync(&world, None, &changed(&[section::PLOTS]), &mut camera);
        assert_eq!(
            bridge.applied,
            (1 << FEED_TERRAIN) | (1 << FEED_FOG) | (1 << FEED_OWNERS) | (1 << FEED_PLOTS)
        );
        bridge.applied = 0;
        // Players: the terrain (border colours), the borders and the colours of the entities.
        bridge.sync(&world, None, &changed(&[section::PLAYERS]), &mut camera);
        assert_eq!(
            bridge.applied,
            (1 << FEED_TERRAIN) | (1 << FEED_OWNERS) | (1 << FEED_PLAYERS) | (1 << FEED_TRADE)
        );
        bridge.applied = 0;
        // The routes themselves: the trade feed alone.
        bridge.sync(
            &world,
            None,
            &changed(&[section::TRADE_ROUTES]),
            &mut camera,
        );
        assert_eq!(bridge.applied, 1 << FEED_TRADE);
    }

    #[test]
    fn units_change_does_not_rebuild_the_terrain_mesh() {
        let (world, catalog) = fixture_world();
        let mut bridge = WorldBridge::new();
        bridge.set_catalog(&catalog);
        let mut camera = GameCamera::default();
        bridge.sync(&world, None, &ViewChange::all(), &mut camera);
        assert_eq!(bridge.mesh_builds(), 1);
        let mut moved = world.clone();
        moved.units[0].x += 1;
        moved.turn += 1;
        let change = changed(&[section::HEADER, section::UNITS, section::CITIES]);
        bridge.sync(&moved, None, &change, &mut camera);
        // Even a change of the header (every turn changes it) leaves the terrain alone.
        assert_eq!(bridge.mesh_builds(), 1);
        assert_eq!(bridge.terrain_feed().last_change(), TerrainChange::None);
    }

    #[test]
    fn drawn_lists_the_models_after_a_move_and_a_new_city() {
        let (world, catalog) = fixture_world();
        let mut bridge = WorldBridge::new();
        bridge.set_catalog(&catalog);
        let mut camera = GameCamera::default();
        bridge.sync(&world, None, &ViewChange::all(), &mut camera);
        let (units, cities) = bridge.drawn();
        assert_eq!(units.len(), world.units.len());
        assert_eq!(cities.len(), world.cities.len());

        let mut next = world.clone();
        next.units[0].x += 1;
        let u = &next.units[0];
        let (key, x, y) = ((u.owner, u.id), i32::from(u.x), i32::from(u.y));
        let mut city = next.cities[0].clone();
        city.id += 1000;
        next.cities.push(city.clone());
        let change = ViewChange {
            sections: mask(&[section::UNITS, section::CITIES]),
            dirty_units: Some(client_core::KeyChanges {
                upsert: vec![key],
                remove: vec![],
            }),
            dirty_cities: Some(client_core::KeyChanges {
                upsert: vec![(city.owner, city.id)],
                remove: vec![],
            }),
            ..ViewChange::default()
        };
        bridge.sync(&next, None, &change, &mut camera);
        let (units, cities) = bridge.drawn();
        assert!(
            units.contains(&[i32::from(key.0), key.1, x, y]),
            "the moved unit"
        );
        assert!(
            cities.iter().any(|c| c[1] == city.id),
            "the new city: {cities:?}"
        );
    }

    #[test]
    fn the_fog_and_the_borders_have_feeds_of_their_own() {
        let (world, catalog) = fixture_world();
        let mut bridge = WorldBridge::new();
        bridge.set_catalog(&catalog);
        let mut camera = GameCamera::default();
        bridge.sync(&world, None, &ViewChange::all(), &mut camera);
        assert_eq!(bridge.fog.pending(), fog_input(&world.map).as_ref());
        assert_eq!(
            bridge.owners.pending(),
            owners_input(&world.map, &world.players).as_ref()
        );
    }

    /// The acceptance condition of the bridge (net-client 4.10): an order that changes only units
    /// does not rebuild the terrain, nor does it touch the fog, the borders or the landmarks.
    #[test]
    fn a_units_only_order_rebuilds_nothing_but_the_units() {
        let (world, catalog) = fixture_world();
        let mut bridge = WorldBridge::new();
        bridge.set_catalog(&catalog);
        let mut camera = GameCamera::default();
        bridge.sync(&world, None, &ViewChange::all(), &mut camera);
        bridge.applied = 0;
        // As if the first frame had been flushed.
        bridge.fog = FogFeed::default();
        bridge.owners = OwnersFeed::default();
        std::mem::take(&mut bridge.units.pending);
        std::mem::take(&mut bridge.cities.pending);
        std::mem::take(&mut bridge.plots.pending);
        let builds = bridge.mesh_builds();
        let mut moved = world.clone();
        moved.units[0].x += 1;
        let key = (moved.units[0].owner, moved.units[0].id);
        // What `ClientCore::take_change` says of a diff with one moved unit.
        let change = ViewChange {
            sections: mask(&[section::UNITS]),
            dirty_units: Some(client_core::KeyChanges {
                upsert: vec![key],
                remove: vec![],
            }),
            ..ViewChange::default()
        };
        bridge.sync(&moved, None, &change, &mut camera);
        assert_eq!(bridge.applied, 1 << FEED_UNITS, "only the units feed ran");
        assert_eq!(bridge.mesh_builds(), builds, "the terrain mesh was rebuilt");
        assert!(bridge.fog.pending().is_none() && bridge.owners.pending().is_none());
        assert!(bridge.plots.pending().is_empty() && bridge.cities.pending().is_empty());
        assert_eq!(bridge.units.pending().units.len(), 1, "one unit changed");
    }

    #[test]
    fn a_new_catalog_converts_everything_again() {
        let (world, catalog) = fixture_world();
        let mut bridge = WorldBridge::new();
        bridge.set_catalog(&catalog);
        let mut camera = GameCamera::default();
        bridge.sync(&world, None, &ViewChange::all(), &mut camera);
        bridge.applied = 0;
        bridge.set_catalog(&catalog);
        bridge.sync(&world, None, &ViewChange::default(), &mut camera);
        assert_eq!(
            bridge.applied, ALL_FEEDS,
            "every feed ran although nothing changed"
        );
    }

    #[test]
    fn sync_of_a_diff_on_a_huge_map_is_fast() {
        let (mut world, catalog) = fixture_world();
        // Huge: 128 x 80, tiled from the fixture's plots.
        let (w, h) = (128u16, 80u16);
        let tile = world.map.plots.clone();
        world.map.width = w;
        world.map.height = h;
        world.map.plots = (0..usize::from(w) * usize::from(h))
            .map(|i| tile[i % tile.len()].clone())
            .collect();
        let mut bridge = WorldBridge::new();
        bridge.set_catalog(&catalog);
        let mut camera = GameCamera::default();
        bridge.sync(&world, None, &ViewChange::all(), &mut camera);
        let builds = bridge.mesh_builds();

        // A diff: 50 plots change fog, 5 units move.
        let mut next = world.clone();
        let mut dirty = Vec::new();
        for i in (0..50).map(|k| k * 37) {
            let flags = &mut next.map.plots[i].flags;
            *flags = protocol::PlotFlags(flags.0 ^ protocol::PlotFlags::VISIBLE);
            dirty.push(i as u32);
        }
        let mut keys = Vec::new();
        for u in next.units.iter_mut().take(5) {
            u.x += 1;
            keys.push((u.owner, u.id));
        }
        let change = ViewChange {
            sections: mask(&[section::PLOTS, section::UNITS]),
            dirty_plots: Some(dirty),
            dirty_units: Some(client_core::KeyChanges {
                upsert: keys,
                remove: vec![],
            }),
            ..ViewChange::default()
        };
        let started = std::time::Instant::now();
        bridge.sync(&next, None, &change, &mut camera);
        let took = started.elapsed();
        eprintln!("Huge sync of 50 plots and 5 units: {took:?}");
        assert_eq!(bridge.mesh_builds(), builds, "fog only recolours");
        // 2 ms in a release build (net-client 4.10, provisional); a debug build is far slower.
        let limit = if cfg!(debug_assertions) { 40 } else { 2 };
        assert!(took.as_millis() <= limit, "{took:?}");
    }
}
