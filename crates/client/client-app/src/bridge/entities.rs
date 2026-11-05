//! The entities of the bridge (net-client 4.10): the table of catalog names the ribbons of units, cities and
//! plots share, the feed of the players (colours, civilization, era; top bar era) and `entity_snapshot`.

use std::collections::{HashMap, HashSet};

use client_core::ViewChange;
use protocol::{
    Catalog, CityFlags, CityView, PlotFlags, PlotType, PlotView, SectionMask, UnitFlags, UnitView,
    WorldView,
};
use render::Renderer;
use render::anim::build_name;
use render::entities::{
    CityEntity, Culture, EntityPlayer, EntitySnapshot, EraBucket, Owners, PlotDecor,
    ResourceVariant, RouteKind, UnitEntity,
};
use render::hexgrid::Dir;
use render::world::objects::input::EntityChanges;
use render::world::unit::place::{UnitVisExtra, UnitVisFlags};

use super::{FeedCx, ViewFeed, mask, section};

/// `production_order` of a city building a building (`GC_ORDER_CONSTRUCT`).
const ORDER_CONSTRUCT: i8 = 1;

/// Improvements drawn in the middle of the cell (barbarian encampment, ruins).
const SPECIAL_IMPROVEMENTS: [&str; 2] = ["IMPROVEMENT_BARBARIAN_CAMP", "IMPROVEMENT_GOODY_HUT"];

/// Features the landmarks do not draw: the terrain, the forest and the water draw them.
const FEATURES_WITHOUT_LANDMARK: [&str; 5] = [
    "FEATURE_FOREST",
    "FEATURE_JUNGLE",
    "FEATURE_MARSH",
    "FEATURE_FLOOD_PLAINS",
    "FEATURE_ICE",
];

/// The eras before the Industrial one; every other era has the Industrial art of the landmarks.
const EARLY_ERAS: [&str; 4] = [
    "ERA_ANCIENT",
    "ERA_CLASSICAL",
    "ERA_MEDIEVAL",
    "ERA_RENAISSANCE",
];

/// The eras of the game in their order (Eras).
const ERA_ORDER: [&str; 8] = [
    "ERA_ANCIENT",
    "ERA_CLASSICAL",
    "ERA_MEDIEVAL",
    "ERA_RENAISSANCE",
    "ERA_INDUSTRIAL",
    "ERA_MODERN",
    "ERA_POSTMODERN",
    "ERA_FUTURE",
];

/// The quantity of a strategic resource from which its plot shows the large deposit (`_HEAVY`).
// TODO(RW-15): the rule of the engine for the heavy and light models of the resources.
const HEAVY_RESOURCE_QTY: i8 = 5;

/// Catalog names the entity renderer needs: unit type names, the culture of each civilization,
/// the era buckets, and the art defines of resources and improvements and the kinds of routes.
#[derive(Debug, Clone, Default)]
pub struct EntityTable {
    units: HashMap<i16, String>,
    cultures: HashMap<i16, Culture>,
    eras: HashMap<i16, EraBucket>,
    /// Resource id -> art define (`sv/<tag>` in the art table).
    resources: HashMap<i16, String>,
    /// Improvement id -> (art define, drawn in the middle of the cell).
    improvements: HashMap<i16, (String, bool)>,
    routes: HashMap<i16, RouteKind>,
    /// `BuildTypes` id -> art define of the improvement that build makes (`PlotView.building` is the
    /// build under way on the plot, WOB4); the roads and the removals make none.
    builds: HashMap<i16, String>,
    /// Feature id -> art define, for the features that have a model of their own (WOB4).
    features: HashMap<i16, String>,
    /// Building id -> `Buildings.ArtDefineTag`, for the buildings the art table may draw (WOB5).
    buildings: HashMap<i16, String>,
    /// Building id -> `Buildings.Type` of every building (re-b17 5.1, 5.3).
    building_types: HashMap<i16, String>,
    /// The id of the marsh feature: the plots with it ask for the marsh model of a resource.
    marsh: Option<i16>,
    /// Eras from the Industrial one on (the era of the landmark rows).
    industrial_eras: HashSet<i16>,
    /// Era id -> its place in [ERA_ORDER] (the boat of an embarked unit follows it).
    era_index: HashMap<i16, u8>,
    /// `Catalog.art_techs` by type name, in the bit order of `PlayerView.art_techs`.
    art_techs: Vec<String>,
}

impl EntityTable {
    pub fn from_catalog(catalog: &Catalog) -> Self {
        Self {
            units: catalog
                .units
                .iter()
                .map(|e| (e.id, e.type_name.clone()))
                .collect(),
            // The catalog's `art_style` of a civilization; empty in the fixture.
            cultures: catalog
                .civs
                .iter()
                .filter_map(|e| Some((e.id, Culture::parse(&e.art.art_style)?)))
                .collect(),
            eras: catalog
                .eras
                .iter()
                .map(|e| (e.id, era_bucket(&e.type_name)))
                .collect(),
            resources: catalog
                .resources
                .iter()
                .filter(|e| !e.art.art_define.is_empty())
                .map(|e| (e.id, e.art.art_define.clone()))
                .collect(),
            improvements: catalog
                .improvements
                .iter()
                .filter(|e| !e.art.art_define.is_empty())
                .map(|e| {
                    let special = SPECIAL_IMPROVEMENTS.contains(&e.type_name.as_str());
                    (e.id, (e.art.art_define.clone(), special))
                })
                .collect(),
            routes: catalog
                .routes
                .iter()
                .map(|e| (e.id, RouteKind::from_type_name(&e.type_name)))
                .collect(),
            buildings: catalog
                .buildings
                .iter()
                .filter(|e| !e.art.art_define.is_empty())
                .map(|e| (e.id, e.art.art_define.clone()))
                .collect(),
            building_types: catalog
                .buildings
                .iter()
                .map(|e| (e.id, e.type_name.clone()))
                .collect(),
            builds: (0..64)
                .filter_map(|id| {
                    let improvement =
                        format!("IMPROVEMENT_{}", build_name(id)?.strip_prefix("BUILD_")?);
                    let tag = catalog
                        .improvements
                        .iter()
                        .find(|e| e.type_name == improvement && !e.art.art_define.is_empty())?;
                    Some((id, tag.art.art_define.clone()))
                })
                .collect(),
            features: catalog
                .features
                .iter()
                .filter(|e| {
                    !e.art.art_define.is_empty()
                        && !FEATURES_WITHOUT_LANDMARK.contains(&e.type_name.as_str())
                })
                .map(|e| (e.id, e.art.art_define.clone()))
                .collect(),
            marsh: catalog
                .features
                .iter()
                .find(|e| e.type_name == "FEATURE_MARSH")
                .map(|e| e.id),
            industrial_eras: catalog
                .eras
                .iter()
                .filter(|e| !EARLY_ERAS.contains(&e.type_name.as_str()))
                .map(|e| e.id)
                .collect(),
            era_index: catalog
                .eras
                .iter()
                .filter_map(|e| {
                    let i = ERA_ORDER.iter().position(|n| *n == e.type_name)?;
                    Some((e.id, i as u8))
                })
                .collect(),
            art_techs: catalog
                .art_techs
                .iter()
                .map(|id| {
                    catalog
                        .techs
                        .iter()
                        .find(|t| t.id == *id)
                        .map(|t| t.type_name.clone())
                        .unwrap_or_default()
                })
                .collect(),
        }
    }

    /// `Catalog.art_techs` by type name, in the bit order of `PlayerView.art_techs` (WOB4).
    pub fn art_tech_names(&self) -> Vec<String> {
        self.art_techs.clone()
    }

    /// The unit as the renderer keeps it (`unit_changes` of `units.rs`).
    pub(super) fn unit_entity(&self, u: &UnitView) -> UnitEntity {
        UnitEntity {
            id: u.id,
            owner: u.owner,
            type_name: self.units.get(&u.unit_type).cloned().unwrap_or_default(),
            x: i32::from(u.x),
            y: i32::from(u.y),
            hp: i32::from(u.hp),
            max_hp: i32::from(u.max_hp),
            civilian: u.flags.contains(UnitFlags::CIVILIAN),
            needs_orders: u.flags.contains(UnitFlags::NEEDS_ORDERS),
        }
    }

    /// What the view says of the unit beyond the record: the direction it faces, its state flags
    /// and what a worker builds (`UnitVisExtra`, WOB2). `facing` and `building` are `-1` when none.
    pub(super) fn unit_extra(&self, u: &UnitView) -> UnitVisExtra {
        let mut flags = UnitVisFlags::default();
        for (from, to) in [
            (UnitFlags::CIVILIAN, UnitVisFlags::CIVILIAN),
            (UnitFlags::EMBARKED, UnitVisFlags::EMBARKED),
            (UnitFlags::FORTIFIED, UnitVisFlags::FORTIFIED),
            (UnitFlags::SLEEPING, UnitVisFlags::SLEEPING),
            (UnitFlags::GARRISONED, UnitVisFlags::GARRISON),
        ] {
            if u.flags.contains(from) {
                flags = flags.with(to);
            }
        }
        if u.cargo_of >= 0 {
            flags = flags.with(UnitVisFlags::CARGO);
        }
        UnitVisExtra {
            facing: usize::try_from(u.facing)
                .ok()
                .and_then(|i| Dir::ALL.get(i).copied()),
            flags,
            building: (u.building >= 0).then_some(u.building),
        }
    }

    /// The city as the renderer keeps it (`city_changes` of `cities.rs`).
    pub(super) fn city_entity(&self, c: &CityView) -> CityEntity {
        CityEntity {
            id: c.id,
            owner: c.owner,
            x: i32::from(c.x),
            y: i32::from(c.y),
            name: c.name.clone(),
            population: i32::from(c.population),
            capital: c.flags.contains(CityFlags::CAPITAL),
            hp: i32::from(c.hp),
            max_hp: i32::from(c.max_hp),
            buildings: {
                let mut tags: Vec<String> = c
                    .buildings
                    .iter()
                    .filter_map(|id| self.buildings.get(id).cloned())
                    .collect();
                tags.sort();
                tags.dedup();
                tags
            },
            building_types: {
                let mut ids: Vec<i16> = c
                    .buildings
                    .iter()
                    .copied()
                    .filter(|id| self.buildings.contains_key(id))
                    .collect();
                ids.sort_unstable();
                ids.dedup();
                ids.iter()
                    .filter_map(|id| self.building_types.get(id).cloned())
                    .collect()
            },
            production_building: (c.production_order == ORDER_CONSTRUCT)
                .then(|| self.building_types.get(&c.production_item).cloned())
                .flatten(),
            razing: c.flags.contains(CityFlags::RAZING),
            founder: c.founder,
        }
    }

    /// The owners of the units and cities: colours, cultures, eras (`apply` of the feed).
    pub(super) fn owners(&self, world: &WorldView) -> Owners {
        Owners {
            players: self.players(world),
            era: self
                .eras
                .get(&world.top_bar.era)
                .copied()
                .unwrap_or_default(),
        }
    }

    fn players(&self, world: &WorldView) -> Vec<EntityPlayer> {
        world
            .players
            .iter()
            .map(|p| {
                let [r, g, b, _] = p.color_primary.to_be_bytes();
                let [tr, tg, tb, _] = p.color_secondary.to_be_bytes();
                EntityPlayer {
                    id: p.id,
                    color: [r, g, b],
                    // re: units take `PlayerColors.SecondaryColor` as `TeamColorR`: the S2 capture
                    // (instance buffer, stride 1568) holds Rome's (0.275, 0, 0.463), the
                    // `COLOR_PLAYER_ROME_BACKGROUND` row, not the gold `..._ICON` primary.
                    team_color: [tr, tg, tb],
                    culture: self.cultures.get(&p.civ_type).copied(),
                    era: self.eras.get(&p.era).copied(),
                    industrial: self.industrial_eras.contains(&p.era),
                    art_techs: p.art_techs,
                    era_index: self.era_index.get(&p.era).copied(),
                }
            })
            .collect()
    }

    /// What lies on `plot` besides the terrain; `None` for an empty or an unrevealed plot.
    ///
    /// The resource follows the rule of the original (`CvPlot::updateLayout`): none on a plot the
    /// team has not explored (`FOGOFWARMODE_UNEXPLORED`), even though the record of such a plot carries
    /// the world's resource since the whole world goes to the client (NP-D19); on an explored one the
    /// resource of the record, which the core gives as `CvPlot::getResourceType(team)`, the resource
    /// only when the team has its `TechReveal` (`Q_PLOTS`, gc_query.cpp). The improvement of an
    /// unexplored plot is the team's revealed one, none.
    pub(super) fn plot_decor(&self, plot: &PlotView, x: usize, width: usize) -> Option<PlotDecor> {
        if !plot.flags.contains(PlotFlags::REVEALED) {
            return None;
        }
        let resource = self.resources.get(&plot.resource).cloned();
        let improvement = self.improvements.get(&plot.improvement);
        let route = self.routes.get(&plot.route).copied();
        // The improvement being built (`PlotView.building`) and the feature with a model of its own.
        let building = self.builds.get(&plot.building).cloned();
        let feature = self.features.get(&plot.feature).cloned();
        if resource.is_none()
            && improvement.is_none()
            && route.is_none()
            && building.is_none()
            && feature.is_none()
        {
            return None;
        }
        Some(PlotDecor {
            x: (x % width) as i32,
            y: (x / width) as i32,
            resource,
            improvement: improvement.map(|(tag, _)| tag.clone()),
            special: improvement.is_some_and(|(_, special)| *special),
            pillaged: plot.flags.contains(PlotFlags::PILLAGED),
            route,
            building,
            feature,
            owner: (plot.owner >= 0).then_some(plot.owner),
            worked: plot.flags.contains(PlotFlags::WORKED),
            variant: ResourceVariant {
                heavy: plot.resource_qty >= HEAVY_RESOURCE_QTY,
                marsh: self.marsh == Some(plot.feature),
                hill: plot.plot_type == PlotType::Hill,
                water: plot.plot_type == PlotType::Water,
            },
            route_pillaged: route.is_some() && plot.flags.contains(PlotFlags::ROUTE_PILLAGED),
            rivers: plot.rivers,
        })
    }

    /// The decor of every plot of the view.
    fn decor(&self, world: &WorldView) -> Vec<PlotDecor> {
        let width = usize::from(world.map.width).max(1);
        world
            .map
            .plots
            .iter()
            .enumerate()
            .filter_map(|(i, p)| self.plot_decor(p, i, width))
            .collect()
    }
}

/// City models exist for four buckets (`GameplayToEngineEra` 0x087b2a1e, re-b17 4.4): the eras up
/// to the Medieval are Ancient, the Renaissance Renaissance, the Industrial Industrial, and every
/// later one Modern.
fn era_bucket(type_name: &str) -> EraBucket {
    match type_name {
        "ERA_ANCIENT" | "ERA_CLASSICAL" | "ERA_MEDIEVAL" => EraBucket::Ancient,
        "ERA_INDUSTRIAL" => EraBucket::Industrial,
        "ERA_MODERN" | "ERA_POSTMODERN" | "ERA_FUTURE" => EraBucket::Modern,
        _ => EraBucket::Renaissance,
    }
}

/// `WorldView` -> units and cities for the renderer.
pub fn entity_snapshot(world: &WorldView, table: &EntityTable) -> EntitySnapshot {
    snapshot_with(world, table, table.decor(world))
}

fn snapshot_with(world: &WorldView, table: &EntityTable, plots: Vec<PlotDecor>) -> EntitySnapshot {
    let owners = table.owners(world);
    EntitySnapshot {
        players: owners.players,
        units: world.units.iter().map(|u| table.unit_entity(u)).collect(),
        cities: world.cities.iter().map(|c| table.city_entity(c)).collect(),
        era: owners.era,
        plots,
    }
}

/// Hands the changes a feed collected to the renderer, if there are any.
pub(super) fn flush_entity_changes(pending: &mut EntityChanges, renderer: &mut Renderer) {
    let changes = std::mem::take(pending);
    if !changes.is_empty() {
        renderer.apply_entity_changes(changes);
    }
}

/// The feed of the players' side of the entities: colours, cultures, eras and the art techs the
/// units, cities and landmarks are drawn with. The ribbons of `units.rs`, `cities.rs` and `plots.rs`
/// are feeds of their own (R4.10); this one sends the shared part, only when it changed.
#[derive(Debug, Default)]
pub struct PlayersFeed {
    table: EntityTable,
    owners: Owners,
    /// Changes that `apply` made and `flush` has not handed over yet.
    pending: EntityChanges,
}

impl PlayersFeed {
    /// Changes waiting for `flush`.
    pub fn pending(&self) -> &EntityChanges {
        &self.pending
    }

    /// The owners as sent last.
    pub fn owners(&self) -> &Owners {
        &self.owners
    }
}

impl ViewFeed for PlayersFeed {
    fn sections(&self) -> SectionMask {
        mask(&[section::PLAYERS, section::TOP_BAR])
    }

    fn on_catalog(&mut self, catalog: &Catalog) {
        // New names: the next `apply` runs whole (`WorldBridge::sync` makes it a full change).
        self.table = EntityTable::from_catalog(catalog);
        self.pending.art_techs = Some(self.table.art_tech_names());
    }

    fn apply(&mut self, view: &WorldView, _change: &ViewChange, _cx: &mut FeedCx<'_>) {
        let owners = self.table.owners(view);
        if owners != self.owners {
            self.owners = owners.clone();
            self.pending.owners = Some(owners);
        }
    }

    fn flush(&mut self, renderer: &mut Renderer) {
        flush_entity_changes(&mut self.pending, renderer);
    }
}

#[cfg(test)]
mod tests {
    use render::GameCamera;

    use super::super::tests::fixture_world;
    use super::super::{CitiesFeed, PlotsFeed, UnitsFeed};
    use super::*;

    /// The four feeds of the entities, run like `WorldBridge::sync` does: a feed runs when the
    /// change touches its sections (or is a reset).
    #[derive(Default)]
    struct Feeds {
        players: PlayersFeed,
        units: UnitsFeed,
        cities: CitiesFeed,
        plots: PlotsFeed,
    }

    impl Feeds {
        fn new(catalog: &Catalog) -> Self {
            let mut feeds = Self::default();
            feeds.players.on_catalog(catalog);
            feeds.units.on_catalog(catalog);
            feeds.cities.on_catalog(catalog);
            feeds.plots.on_catalog(catalog);
            feeds
        }

        fn each(&mut self, mut f: impl FnMut(&mut dyn ViewFeed)) {
            f(&mut self.players);
            f(&mut self.units);
            f(&mut self.cities);
            f(&mut self.plots);
        }

        /// What the feeds hold for `flush`, merged.
        fn pending(&self) -> EntityChanges {
            let mut all = self.players.pending().clone();
            all.merge(self.units.pending().clone());
            all.merge(self.cities.pending().clone());
            all.merge(self.plots.pending().clone());
            all
        }

        /// Takes what the feeds hold, as the renderer's `flush` would.
        fn take(&mut self) -> EntityChanges {
            let all = self.pending();
            self.players.pending = EntityChanges::default();
            self.units.pending = EntityChanges::default();
            self.cities.pending = EntityChanges::default();
            self.plots.pending = EntityChanges::default();
            all
        }

        /// The units, cities and plots the feeds have sent, as one snapshot.
        fn snapshot(&self) -> EntitySnapshot {
            let mut units: Vec<UnitEntity> = self.units.units().cloned().collect();
            units.sort_by_key(|u| u.id);
            let mut cities: Vec<CityEntity> = self.cities.cities().cloned().collect();
            cities.sort_by_key(|c| c.id);
            EntitySnapshot {
                players: self.players.owners().players.clone(),
                units,
                cities,
                era: self.players.owners().era,
                plots: self.plots.plots().into_iter().cloned().collect(),
            }
        }
    }

    fn apply(feeds: &mut Feeds, world: &WorldView, change: &ViewChange) {
        let mut camera = GameCamera::default();
        feeds.each(|feed| {
            if change.reset || feed.sections().0 & change.sections.0 != 0 {
                feed.apply(
                    world,
                    change,
                    &mut FeedCx {
                        basis: None,
                        camera: &mut camera,
                    },
                );
            }
        });
    }

    #[test]
    fn entities_come_from_the_world_and_the_catalog() {
        let (world, catalog) = fixture_world();
        let table = EntityTable::from_catalog(&catalog);
        let e = entity_snapshot(&world, &table);
        assert_eq!(e.units.len(), world.units.len());
        assert_eq!(e.cities.len(), world.cities.len());
        assert!(e.units.iter().all(|u| u.type_name.starts_with("UNIT_")));
        assert!(e.units.iter().any(|u| u.civilian) && e.units.iter().any(|u| !u.civilian));
        let human = e.player(world.human).unwrap();
        assert_ne!(human.color, [0, 0, 0]);
    }

    #[test]
    fn the_feed_hands_over_only_the_differences() {
        let (world, catalog) = fixture_world();
        let mut feed = Feeds::new(&catalog);
        apply(&mut feed, &world, &ViewChange::all());
        let first = feed.take();
        assert_eq!(first.units.len(), world.units.len());
        assert_eq!(first.cities.len(), world.cities.len());
        assert!(first.owners.is_some() && !first.plots.is_empty());
        // Same world: nothing to hand over.
        apply(&mut feed, &world, &ViewChange::all());
        assert!(feed.pending().is_empty());
        // One unit moves: that unit and nothing else.
        let mut moved = world.clone();
        moved.units[0].x += 1;
        apply(&mut feed, &moved, &ViewChange::all());
        let next = feed.take();
        assert_eq!(next.units.len(), 1);
        assert!(next.cities.is_empty() && next.plots.is_empty() && next.owners.is_none());
    }

    /// Two syncs before a flush hand over one change set: a later record of a key wins.
    #[test]
    fn changes_between_two_flushes_are_merged() {
        let (world, catalog) = fixture_world();
        let mut feed = Feeds::new(&catalog);
        apply(&mut feed, &world, &ViewChange::all());
        let mut a = world.clone();
        a.units[0].x += 1;
        apply(&mut feed, &a, &ViewChange::all());
        let mut b = a.clone();
        b.units[0].x += 1;
        apply(&mut feed, &b, &ViewChange::all());
        let pending = feed.pending();
        let units = &pending.units;
        let one: Vec<_> = units
            .iter()
            .filter(|u| (u.owner, u.id) == (b.units[0].owner, b.units[0].id))
            .collect();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].x, i32::from(b.units[0].x));
    }

    #[test]
    fn the_decor_of_the_plots_is_kept_until_the_plots_change() {
        let (mut world, catalog) = fixture_world();
        let table = EntityTable::from_catalog(&catalog);
        let mut feed = Feeds::new(&catalog);
        apply(&mut feed, &world, &ViewChange::all());
        let n = feed.snapshot().plots.len();
        assert!(n > 0);
        // A road appears on an unrevealed-then-revealed plot; units only: the old decor stays.
        let at = world
            .map
            .plots
            .iter()
            .position(|p| {
                p.flags.contains(PlotFlags::REVEALED)
                    && p.route < 0
                    && !table.features.contains_key(&p.feature)
                    && !table.builds.contains_key(&p.building)
            })
            .unwrap();
        let road = world.map.plots.iter().find(|p| p.route >= 0).unwrap().route;
        world.map.plots[at].route = road;
        let units_only = ViewChange {
            sections: mask(&[section::UNITS]),
            ..ViewChange::default()
        };
        apply(&mut feed, &world, &units_only);
        assert_eq!(feed.snapshot().plots.len(), n);
        let plots = ViewChange {
            sections: mask(&[section::PLOTS]),
            dirty_plots: Some(vec![at as u32]),
            ..ViewChange::default()
        };
        apply(&mut feed, &world, &plots);
        assert_eq!(feed.snapshot().plots.len(), n + 1);
    }

    /// A city carries the types of its buildings with a model, in the order of their ids, and the
    /// building it is building (re-b17 5.1, 5.3).
    #[test]
    fn a_city_carries_its_building_types_by_id_and_the_building_in_production() {
        use protocol::{ArtRefs, CatalogEntry};
        let entry = |id: i16, type_name: &str, art_define: &str| CatalogEntry {
            id,
            type_name: type_name.into(),
            art: ArtRefs {
                art_define: art_define.into(),
                ..ArtRefs::default()
            },
            ..CatalogEntry::default()
        };
        let catalog = Catalog {
            buildings: vec![
                entry(3, "BUILDING_COLOSSEUM", "COLESSEUM"),
                entry(7, "BUILDING_AMPHITHEATER", "COLESSEUM"),
                entry(9, "BUILDING_PYRAMID", "THE PYRAMIDS"),
                entry(11, "BUILDING_LIBRARY", ""),
            ],
            ..Catalog::default()
        };
        let table = EntityTable::from_catalog(&catalog);
        let mut c = CityView {
            buildings: vec![7, 11, 3, 7],
            production_order: ORDER_CONSTRUCT,
            production_item: 9,
            ..CityView::default()
        };
        let e = table.city_entity(&c);
        assert_eq!(e.buildings, ["COLESSEUM"]);
        assert_eq!(
            e.building_types,
            ["BUILDING_COLOSSEUM", "BUILDING_AMPHITHEATER"]
        );
        assert_eq!(e.production_building.as_deref(), Some("BUILDING_PYRAMID"));
        // Training a unit (item 9 is a unit then): no building.
        c.production_order = 0;
        assert_eq!(table.city_entity(&c).production_building, None);
    }

    /// WOB4: what the landmarks need of a plot besides its resource and improvement: the build
    /// under way (`PlotView.building` is a `BuildTypes` id), the feature with a model, the owner, the
    /// work of a city, the model variant of the resource, and the era of a player.
    #[test]
    fn a_plot_carries_what_the_choice_of_a_landmark_needs() {
        use protocol::{ArtRefs, CatalogEntry};
        let entry = |id: i16, type_name: &str, art_define: &str| CatalogEntry {
            id,
            type_name: type_name.into(),
            art: ArtRefs {
                art_define: art_define.into(),
                ..ArtRefs::default()
            },
            ..CatalogEntry::default()
        };
        let catalog = Catalog {
            improvements: vec![entry(4, "IMPROVEMENT_FARM", "ART_DEF_IMPROVEMENT_FARM")],
            resources: vec![entry(2, "RESOURCE_IRON", "ART_DEF_RESOURCE_IRON")],
            features: vec![
                entry(1, "FEATURE_OASIS", "ART_DEF_FEATURE_OASIS"),
                entry(2, "FEATURE_FOREST", "ART_DEF_FEATURE_FOREST"),
                entry(3, "FEATURE_MARSH", "ART_DEF_FEATURE_MARSH"),
            ],
            eras: vec![entry(0, "ERA_ANCIENT", ""), entry(5, "ERA_INDUSTRIAL", "")],
            techs: vec![
                entry(0, "TECH_AGRICULTURE", ""),
                entry(9, "TECH_ECONOMICS", ""),
            ],
            art_techs: vec![9],
            ..Catalog::default()
        };
        let table = EntityTable::from_catalog(&catalog);
        assert_eq!(table.art_tech_names(), ["TECH_ECONOMICS"]);
        let plot = PlotView {
            flags: PlotFlags(PlotFlags::REVEALED | PlotFlags::WORKED),
            // BUILD_FARM is the third build of the game.
            building: 2,
            feature: 1,
            owner: 3,
            resource: -1,
            improvement: -1,
            route: -1,
            ..PlotView::default()
        };
        let d = table.plot_decor(&plot, 5, 4).unwrap();
        assert_eq!(d.building.as_deref(), Some("ART_DEF_IMPROVEMENT_FARM"));
        assert_eq!(d.feature.as_deref(), Some("ART_DEF_FEATURE_OASIS"));
        assert_eq!((d.x, d.y, d.owner, d.worked), (1, 1, Some(3), true));
        // A road under way and a forest are no landmarks.
        let none = PlotView {
            flags: PlotFlags(PlotFlags::REVEALED),
            building: 0,
            feature: 2,
            owner: -1,
            resource: -1,
            improvement: -1,
            route: -1,
            ..PlotView::default()
        };
        assert!(table.plot_decor(&none, 0, 4).is_none());
        // A large iron deposit on a marsh hill asks for the heavy marsh model.
        let iron = PlotView {
            flags: PlotFlags(PlotFlags::REVEALED),
            resource: 2,
            resource_qty: 8,
            feature: 3,
            plot_type: PlotType::Hill,
            building: -1,
            improvement: -1,
            route: -1,
            owner: -1,
            ..PlotView::default()
        };
        let v = table.plot_decor(&iron, 0, 4).unwrap().variant;
        assert!(v.heavy && v.marsh && v.hill && !v.water);
        // The same plot under the clouds carries the world's iron (NP-D19) and shows nothing: the
        // original has no resource on an unexplored plot.
        let dark = PlotView::unrevealed_of(&iron);
        assert_eq!(dark.resource, 2);
        assert!(table.plot_decor(&dark, 0, 4).is_none());
        // The Industrial era and the later ones have the Industrial art.
        assert!(table.industrial_eras.contains(&5) && !table.industrial_eras.contains(&0));
    }

    #[test]
    fn plots_carry_resources_improvements_and_routes_of_revealed_plots() {
        let (world, catalog) = fixture_world();
        let table = EntityTable::from_catalog(&catalog);
        let e = entity_snapshot(&world, &table);
        assert!(!e.plots.is_empty());
        let width = usize::from(world.map.width);
        for d in &e.plots {
            let p = &world.map.plots[d.y as usize * width + d.x as usize];
            assert!(p.flags.contains(PlotFlags::REVEALED));
            assert_eq!(d.resource.is_some(), p.resource >= 0);
            assert_eq!(d.improvement.is_some(), p.improvement >= 0);
            assert_eq!(d.route.is_some(), p.route >= 0);
        }
        // Every plot with something on it is listed, and the tags are the catalog's art defines.
        let n = world
            .map
            .plots
            .iter()
            .filter(|p| {
                p.flags.contains(PlotFlags::REVEALED)
                    && (p.resource >= 0
                        || p.improvement >= 0
                        || p.route >= 0
                        || table.features.contains_key(&p.feature)
                        || table.builds.contains_key(&p.building))
            })
            .count();
        assert_eq!(e.plots.len(), n);
        assert!(
            e.plots
                .iter()
                .filter_map(|d| d.resource.as_deref())
                .all(|t| t.starts_with("ART_DEF_RESOURCE_"))
        );
        assert!(e.plots.iter().any(|d| d.route == Some(RouteKind::Road)));
        // The encampment and the ruins are the improvements drawn in the middle.
        assert!(
            e.plots.iter().any(|d| d.special),
            "camp or ruins in the fixture"
        );
        assert!(e.plots.iter().filter(|d| d.special).all(|d| {
            d.improvement
                .as_deref()
                .is_some_and(|t| t.ends_with("BARBARIAN_CAMP") || t.ends_with("GOODY_HUT"))
        }));
    }

    #[test]
    fn unrevealed_plots_carry_nothing_and_unknown_ids_are_skipped() {
        let (mut world, catalog) = fixture_world();
        let table = EntityTable::from_catalog(&catalog);
        let p = &mut world.map.plots[0];
        p.flags = PlotFlags(0);
        p.resource = 0;
        p.route = 0;
        p.improvement = 99; // not in the catalog
        let e = entity_snapshot(&world, &table);
        assert!(e.plots.iter().all(|d| (d.x, d.y) != (0, 0)));
        let q = &mut world.map.plots[0];
        q.flags = PlotFlags(PlotFlags::REVEALED);
        let e = entity_snapshot(&world, &table);
        let d = e.plots.iter().find(|d| (d.x, d.y) == (0, 0)).unwrap();
        assert!(d.resource.is_some() && d.route.is_some() && d.improvement.is_none());
    }

    #[test]
    fn eras_map_to_city_buckets() {
        assert_eq!(era_bucket("ERA_ANCIENT"), EraBucket::Ancient);
        assert_eq!(era_bucket("ERA_CLASSICAL"), EraBucket::Ancient);
        assert_eq!(era_bucket("ERA_MEDIEVAL"), EraBucket::Ancient);
        assert_eq!(era_bucket("ERA_RENAISSANCE"), EraBucket::Renaissance);
        assert_eq!(era_bucket("ERA_INDUSTRIAL"), EraBucket::Industrial);
        assert_eq!(era_bucket("ERA_MODERN"), EraBucket::Modern);
        assert_eq!(era_bucket("ERA_FUTURE"), EraBucket::Modern);
    }

    #[test]
    fn unit_extra_reads_facing_flags_cargo_and_work() {
        let (world, catalog) = fixture_world();
        let table = EntityTable::from_catalog(&catalog);
        let mut u: UnitView = world.units[0].clone();
        u.flags = UnitFlags(0);
        u.cargo_of = -1;
        u.building = -1;

        // Facing: index 1 is East; a negative facing is none.
        u.facing = 1;
        assert_eq!(table.unit_extra(&u).facing, Some(Dir::East));
        u.facing = -1;
        assert_eq!(table.unit_extra(&u).facing, None);

        // State flags map to the renderer's flags.
        u.flags = UnitFlags(
            UnitFlags::FORTIFIED
                | UnitFlags::SLEEPING
                | UnitFlags::EMBARKED
                | UnitFlags::GARRISONED
                | UnitFlags::CIVILIAN,
        );
        let f = table.unit_extra(&u).flags;
        assert!(f.has(UnitVisFlags::FORTIFIED));
        assert!(f.has(UnitVisFlags::SLEEPING));
        assert!(f.has(UnitVisFlags::EMBARKED));
        assert!(f.has(UnitVisFlags::GARRISON));
        assert!(f.has(UnitVisFlags::CIVILIAN));
        assert!(!f.has(UnitVisFlags::CARGO));
        u.flags = UnitFlags(0);
        assert_eq!(table.unit_extra(&u).flags, UnitVisFlags::default());

        // Cargo: a transport id sets CARGO, -1 does not.
        u.cargo_of = 7;
        assert!(table.unit_extra(&u).flags.has(UnitVisFlags::CARGO));
        u.cargo_of = -1;
        assert!(!table.unit_extra(&u).flags.has(UnitVisFlags::CARGO));

        // Work: the building type, none for -1.
        u.building = 3;
        assert_eq!(table.unit_extra(&u).building, Some(3));
        u.building = -1;
        assert_eq!(table.unit_extra(&u).building, None);
    }
}
