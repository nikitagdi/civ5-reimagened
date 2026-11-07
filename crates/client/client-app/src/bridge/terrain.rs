//! The `terrain` feed (net-client 4.10): `WorldView.map` -> `TerrainSnapshot` -> render (terrain-render
//! spec §4.2, §5).
//!
//! The feed keeps the last snapshot and its mesh. The mesh is rebuilt only when some cell's relief changed; a
//! change of fog or material only recolours it. It reads the header (size), the plots, the players (border
//! colours) and the map basis. A change that touches none of the cells (a new turn, a unit moved) costs a
//! comparison of the map's size.
//!
//! The basis (`FeedCx::basis`) goes to the `Terrain` subsystem as a
//! `TerrainBasis`: `set_basis` when `basis_plots` is `None` (a new `MapBasis`), `patch_basis` with the
//! records of the changed plots when it is `Some`. The names of the catalog (`TERRAIN_*`, `FEATURE_*`)
//! turn the ids of the records into kinds and natural wonders. Until a game has a `MapBasis` (the B1 path,
//! the fixture) the adapter makes the basis from the snapshot it has built: what the view knows of the
//! relief, sent as a patch of the plots that changed since the last one.

use std::collections::HashMap;

use glam::Vec2;
use protocol::{
    Catalog, MapBasis, MapView, PlotBasisView, PlotFlags, PlotType, PlotView, SectionMask,
    WorldView,
};
use render::terrain::{
    FeatureKind, Fog, Relief, TerrainCell, TerrainKind, TerrainMesh, TerrainSnapshot,
};
use render::world::terrain::{BasisFeature, BasisFlags, PlotBasis, TerrainBasis};
use render::{
    MapBounds, Renderer,
    hexgrid::{GridShape, cell_center},
};

use super::fog::plot_fog;
use super::owners::owner_colors;
use super::{FeedCx, ViewFeed, mask, section};
use client_core::ViewChange;

/// Catalog terrain id -> render terrain kind, by the catalog's `TERRAIN_*` type names; catalog
/// feature id -> feature kind, by the `FEATURE_*` names.
#[derive(Debug, Clone, Default)]
pub struct TerrainTable {
    kinds: HashMap<i16, TerrainKind>,
    features: HashMap<i16, FeatureKind>,
    /// The same features as the basis of the map sees them (natural wonders by piece).
    basis_features: HashMap<i16, BasisFeature>,
}

impl TerrainTable {
    pub fn from_catalog(catalog: &Catalog) -> Self {
        let kinds = catalog
            .terrains
            .iter()
            .map(|e| (e.id, kind_of(&e.type_name)))
            .collect();
        let features = catalog
            .features
            .iter()
            .map(|e| (e.id, FeatureKind::from_type_name(&e.type_name)))
            .collect();
        let basis_features = catalog
            .features
            .iter()
            .map(|e| (e.id, BasisFeature::from_type_name(&e.type_name)))
            .collect();
        Self {
            kinds,
            features,
            basis_features,
        }
    }

    /// Feature of a basis record (`-1` for none); an id the catalog does not list is `Other`.
    pub fn basis_feature(&self, feature: i16) -> BasisFeature {
        if feature < 0 {
            return BasisFeature::None;
        }
        self.basis_features
            .get(&feature)
            .copied()
            .unwrap_or(BasisFeature::Other)
    }

    /// Kind of a plot's feature id (`-1` for none); an id the catalog does not list is `Other`.
    pub fn feature(&self, feature: i16) -> FeatureKind {
        if feature < 0 {
            return FeatureKind::None;
        }
        self.features
            .get(&feature)
            .copied()
            .unwrap_or(FeatureKind::Other)
    }

    pub fn kind(&self, terrain: i16) -> TerrainKind {
        self.kinds
            .get(&terrain)
            .copied()
            .unwrap_or(TerrainKind::Unknown)
    }
}

fn kind_of(type_name: &str) -> TerrainKind {
    match type_name {
        "TERRAIN_GRASS" => TerrainKind::Grass,
        "TERRAIN_PLAINS" => TerrainKind::Plains,
        "TERRAIN_DESERT" => TerrainKind::Desert,
        "TERRAIN_TUNDRA" => TerrainKind::Tundra,
        "TERRAIN_SNOW" => TerrainKind::Snow,
        "TERRAIN_COAST" => TerrainKind::Coast,
        "TERRAIN_OCEAN" => TerrainKind::Ocean,
        _ => TerrainKind::Unknown,
    }
}

fn cell_of(p: &PlotView, terrains: &TerrainTable) -> TerrainCell {
    if !p.flags.contains(PlotFlags::REVEALED) && *p == PlotView::default() {
        // An empty record (a view of before the whole world came, NP-D19) stays empty for the renderer.
        return TerrainCell::default();
    }
    // An unrevealed plot carries the world (NP-D19, 2026-10-22): its cell is the world's, `fog` says it is
    // unrevealed and the render hides it under the clouds.
    let relief = match p.plot_type {
        PlotType::Land => Relief::Flat,
        PlotType::Hill => Relief::Hill,
        PlotType::Mountain => Relief::Mountain,
        PlotType::Water => Relief::Water,
    };
    let mut kind = terrains.kind(p.terrain);
    if relief == Relief::Water
        && (p.flags.contains(PlotFlags::IS_LAKE)
            || !matches!(kind, TerrainKind::Coast | TerrainKind::Ocean))
    {
        // Lakes look like the coast (SIMP-008); a water plot of an unknown terrain is ocean.
        kind = if p.flags.contains(PlotFlags::IS_LAKE) {
            TerrainKind::Coast
        } else {
            TerrainKind::Ocean
        };
    }
    TerrainCell {
        relief,
        kind,
        art: p.continent_art,
        fog: plot_fog(p),
        rivers: p.rivers & 0x3F,
        owner: p.owner,
        feature: p.feature,
        feat: terrains.feature(p.feature),
    }
}

/// `MapView` -> render snapshot. `None` if the plot count does not match the size.
pub fn terrain_snapshot(map: &MapView, terrains: &TerrainTable) -> Option<TerrainSnapshot> {
    let cells = map.plots.iter().map(|p| cell_of(p, terrains)).collect();
    TerrainSnapshot::new(
        u32::from(map.width),
        u32::from(map.height),
        map.wrap_x,
        cells,
    )
}

/// A record of the map basis as the render's `PlotBasis`, by the names of the catalog. A water plot
/// is a coast or the ocean, as in `cell_of`.
pub fn plot_basis(p: &PlotBasisView, terrains: &TerrainTable) -> PlotBasis {
    let relief = match p.plot_type {
        PlotType::Land => Relief::Flat,
        PlotType::Hill => Relief::Hill,
        PlotType::Mountain => Relief::Mountain,
        PlotType::Water => Relief::Water,
    };
    let mut kind = terrains.kind(p.terrain);
    if relief == Relief::Water
        && (p.flags.contains(PlotFlags::IS_LAKE)
            || !matches!(kind, TerrainKind::Coast | TerrainKind::Ocean))
    {
        kind = if p.flags.contains(PlotFlags::IS_LAKE) {
            TerrainKind::Coast
        } else {
            TerrainKind::Ocean
        };
    }
    let mut flags = 0;
    for (view, ours) in [
        (PlotFlags::IS_LAKE, BasisFlags::LAKE),
        (PlotFlags::IS_COASTAL, BasisFlags::COASTAL),
        (PlotFlags::NATURAL_WONDER, BasisFlags::NATURAL_WONDER),
    ] {
        if p.flags.contains(view) {
            flags |= ours;
        }
    }
    PlotBasis {
        relief,
        terrain: kind,
        continent: p.continent_art,
        feature: terrains.basis_feature(p.feature),
        flags: BasisFlags(flags),
        rivers: p.rivers & 0x3F,
        river_flow: p.river_flow,
    }
}

/// `MapBasis` -> render basis. `None` if the plot count does not match the size.
pub fn terrain_basis(basis: &MapBasis, terrains: &TerrainTable) -> Option<TerrainBasis> {
    let plots = basis
        .plots
        .iter()
        .map(|p| plot_basis(p, terrains))
        .collect();
    TerrainBasis::new(
        basis.width,
        basis.height,
        basis.wrap_x,
        basis.terrain_seed,
        plots,
    )
}

/// What the feed owes the terrain subsystem of the renderer about the basis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BasisUpdate {
    /// A new basis: `Terrain::set_basis`.
    Set(TerrainBasis),
    /// The records that changed: `Terrain::patch_basis`.
    Patch(Vec<(u32, PlotBasis)>),
}

impl BasisUpdate {
    /// What is owed after this update and then `next`: a patch on a basis that was not sent yet is
    /// folded into it.
    fn then(self, next: BasisUpdate) -> BasisUpdate {
        match (self, next) {
            (_, set @ BasisUpdate::Set(_)) => set,
            (BasisUpdate::Set(mut basis), BasisUpdate::Patch(changes)) => {
                basis.patch(&changes);
                BasisUpdate::Set(basis)
            }
            (BasisUpdate::Patch(mut a), BasisUpdate::Patch(b)) => {
                a.extend(b);
                BasisUpdate::Patch(a)
            }
        }
    }
}

/// What the last `apply` of the feed did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerrainChange {
    /// Same map as before; nothing to upload.
    None,
    /// Fog or material changed; the geometry was reused, the colours need uploading.
    Recolored,
    /// Relief or size changed; the mesh was rebuilt.
    Rebuilt,
}

/// Keeps the render-side terrain in step with the view.
#[derive(Debug, Default)]
pub struct TerrainFeed {
    terrains: TerrainTable,
    terrain: Option<(TerrainSnapshot, TerrainMesh)>,
    mesh_builds: u32,
    last: Option<TerrainChange>,
    /// Something was built and not handed to the renderer yet.
    upload: bool,
    /// The catalog changed: the cells are converted again.
    stale: bool,
    /// The basis the terrain subsystem is owed and has not been given.
    basis_out: Option<BasisUpdate>,
    /// A `MapBasis` was taken: the basis comes from it, not from the snapshot.
    from_net: bool,
    /// The basis the B1 adapter made last, to send only what changed since.
    b1_basis: Option<TerrainBasis>,
    /// The cell of the start unit the first view centred on: the "look here" beam goes there on
    /// the next flush (R5.12; the original's selection of the first unit fires `GameplayFX`).
    start_beam: Option<(i32, i32)>,
}

impl TerrainFeed {
    /// The basis that waits for the next `flush` (tests, `?stats=1`).
    pub fn pending_basis(&self) -> Option<&BasisUpdate> {
        self.basis_out.as_ref()
    }

    fn owe(&mut self, update: BasisUpdate) {
        self.basis_out = Some(match self.basis_out.take() {
            Some(old) => old.then(update),
            None => update,
        });
    }

    /// The basis of the game's `MapBasis`: all of it when it is new (or the catalog changed), the
    /// records of the changed plots when a `basis` diff says which.
    fn take_net_basis(&mut self, change: &ViewChange, cx: &FeedCx<'_>) {
        let Some(map_basis) = cx.basis else {
            return;
        };
        let new = change.reset
            || change.sections.contains(section::BASIS)
            || !self.from_net
            || self.stale;
        if !new {
            return;
        }
        let patch = match &change.basis_plots {
            Some(list) if self.from_net && !change.reset && !self.stale => Some(list),
            _ => None,
        };
        match patch {
            Some(list) => {
                let changes: Vec<(u32, PlotBasis)> = list
                    .iter()
                    .filter_map(|&i| {
                        let p = map_basis.plots.get(i as usize)?;
                        Some((i, plot_basis(p, &self.terrains)))
                    })
                    .collect();
                if !changes.is_empty() {
                    self.owe(BasisUpdate::Patch(changes));
                }
            }
            None => match terrain_basis(map_basis, &self.terrains) {
                Some(basis) => {
                    self.owe(BasisUpdate::Set(basis));
                    self.from_net = true;
                }
                None => tracing::warn!(
                    "map basis has {} plots for {}x{}",
                    map_basis.plots.len(),
                    map_basis.width,
                    map_basis.height
                ),
            },
        }
    }

    /// The adapter of the B1 path: the basis of the snapshot, whole the first time and when the
    /// size changes, else the records that differ from the last one.
    fn take_b1_basis(&mut self, snapshot: &TerrainSnapshot) {
        let Some(new) = TerrainBasis::from_snapshot(snapshot, 0) else {
            return;
        };
        let update = match &self.b1_basis {
            Some(old) if old.same_shape(&new) => {
                let changes: Vec<(u32, PlotBasis)> = new
                    .plots
                    .iter()
                    .zip(&old.plots)
                    .enumerate()
                    .filter(|(_, (a, b))| a != b)
                    .map(|(i, (a, _))| (i as u32, *a))
                    .collect();
                (!changes.is_empty()).then_some(BasisUpdate::Patch(changes))
            }
            _ => Some(BasisUpdate::Set(new.clone())),
        };
        self.b1_basis = Some(new);
        if let Some(u) = update {
            self.owe(u);
        }
    }

    /// A record of the map basis as the render's, by the catalog this feed has (the terrain share of the mods).
    pub fn plot_basis(&self, p: &PlotBasisView) -> PlotBasis {
        plot_basis(p, &self.terrains)
    }

    /// The current snapshot and its mesh, to hand to `Renderer::set_terrain`.
    pub fn terrain(&self) -> Option<(&TerrainSnapshot, &TerrainMesh)> {
        self.terrain.as_ref().map(|(s, m)| (s, m))
    }

    /// How many times the mesh was generated (tests, `?stats=1`).
    pub fn mesh_builds(&self) -> u32 {
        self.mesh_builds
    }

    /// What the last `apply` did.
    pub fn last_change(&self) -> TerrainChange {
        self.last.unwrap_or(TerrainChange::None)
    }

    /// The new snapshot for `view` after `change`, or `None` if the map is nonsense.
    fn build(&self, view: &WorldView, change: &ViewChange) -> Option<TerrainSnapshot> {
        let map = &view.map;
        let colors = owner_colors(&view.players);
        // Only the listed cells are converted again when the old cells can be reused.
        let dirty = match (&change.dirty_plots, &self.terrain) {
            (Some(list), Some((old, _)))
                if !change.reset
                    && !self.stale
                    && (old.width(), old.height(), old.wrap_x())
                        == (u32::from(map.width), u32::from(map.height), map.wrap_x)
                    && map.plots.len() == old.cells().len() =>
            {
                Some((old, list))
            }
            _ => None,
        };
        let snapshot = match dirty {
            Some((old, list)) => {
                let mut cells = old.cells().to_vec();
                for &i in list {
                    let i = i as usize;
                    if let (Some(cell), Some(plot)) = (cells.get_mut(i), map.plots.get(i)) {
                        *cell = cell_of(plot, &self.terrains);
                    }
                }
                TerrainSnapshot::new(
                    u32::from(map.width),
                    u32::from(map.height),
                    map.wrap_x,
                    cells,
                )
            }
            None => terrain_snapshot(map, &self.terrains),
        };
        snapshot.map(|s| s.with_owner_colors(colors))
    }
}

impl ViewFeed for TerrainFeed {
    fn sections(&self) -> SectionMask {
        mask(&[
            section::HEADER,
            section::PLOTS,
            section::PLAYERS,
            section::BASIS,
        ])
    }

    fn on_catalog(&mut self, catalog: &Catalog) {
        self.terrains = TerrainTable::from_catalog(catalog);
        self.stale = true;
    }

    /// Takes a new view. On the first one, or when the map size changes, the camera gets the map bounds,
    /// and on the first one it centres on the first unit of the human.
    fn apply(&mut self, view: &WorldView, change: &ViewChange, cx: &mut FeedCx<'_>) {
        self.last = Some(TerrainChange::None);
        self.take_net_basis(change, cx);
        let cells_read = change.reset
            || self.stale
            || self.terrain.is_none()
            || change.sections.contains(section::PLOTS)
            || change.sections.contains(section::PLAYERS)
            || change.sections.contains(section::BASIS);
        if !cells_read {
            // A header only. A different size is a different map.
            let same = self.terrain.as_ref().is_some_and(|(s, _)| {
                (s.width(), s.height(), s.wrap_x())
                    == (
                        u32::from(view.map.width),
                        u32::from(view.map.height),
                        view.map.wrap_x,
                    )
            });
            if same {
                return;
            }
        }
        let Some(snapshot) = self.build(view, change) else {
            tracing::warn!(
                "world map has {} plots for {}x{}",
                view.map.plots.len(),
                view.map.width,
                view.map.height
            );
            return;
        };
        self.stale = false;
        let (change_kind, mesh) = match self.terrain.take() {
            Some((old, mesh)) if old == snapshot => {
                self.terrain = Some((old, mesh));
                return;
            }
            Some((old, mesh)) if old.same_geometry(&snapshot) => (TerrainChange::Recolored, mesh),
            old => {
                self.mesh_builds += 1;
                let first = old.is_none();
                let resized = old.as_ref().is_none_or(|(o, _)| {
                    (o.width(), o.height(), o.wrap_x())
                        != (snapshot.width(), snapshot.height(), snapshot.wrap_x())
                });
                if resized {
                    cx.camera
                        .set_bounds(Some(MapBounds::from_grid(&snapshot.shape())));
                }
                if first {
                    cx.camera.center_on(start_focus(view, &snapshot.shape()));
                    self.start_beam = view
                        .units
                        .iter()
                        .find(|u| u.owner == view.human)
                        .map(|u| (i32::from(u.x), i32::from(u.y)));
                    log_terrain_summary(&snapshot);
                }
                if !self.from_net {
                    self.take_b1_basis(&snapshot);
                }
                (TerrainChange::Rebuilt, TerrainMesh::build(&snapshot))
            }
        };
        self.terrain = Some((snapshot, mesh));
        self.last = Some(change_kind);
        self.upload = true;
    }

    fn flush(&mut self, renderer: &mut Renderer) {
        if std::mem::take(&mut self.upload)
            && let Some((snapshot, mesh)) = &self.terrain
        {
            renderer.set_terrain(snapshot, mesh);
            // The fog and the borders come from their own feeds (`fog.rs`, `owners.rs`).
        }
        if let Some((x, y)) = self.start_beam.take() {
            renderer.particles_mut().look_at(x, y);
        }
        match self.basis_out.take() {
            Some(BasisUpdate::Set(basis)) => renderer.set_terrain_basis(basis),
            Some(BasisUpdate::Patch(changes)) => renderer.patch_terrain_basis(&changes),
            None => {}
        }
    }
}

/// One line about the first map: what the human knows of it and what the renderer will draw.
fn log_terrain_summary(snapshot: &TerrainSnapshot) {
    let cells = snapshot.cells();
    let count = |f: &dyn Fn(&TerrainCell) -> bool| cells.iter().filter(|c| f(c)).count();
    let mut arts: Vec<u8> = cells
        .iter()
        .filter(|c| c.is_revealed())
        .map(|c| c.art)
        .collect();
    arts.sort_unstable();
    arts.dedup();
    let mut kinds: Vec<(String, usize)> = Vec::new();
    for c in cells.iter().filter(|c| c.is_revealed()) {
        let k = format!("{:?}", c.kind);
        match kinds.iter_mut().find(|(n, _)| *n == k) {
            Some((_, n)) => *n += 1,
            None => kinds.push((k, 1)),
        }
    }
    tracing::info!(
        "terrain kinds of the revealed plots: {}",
        kinds
            .iter()
            .map(|(k, n)| format!("{k} {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    tracing::info!(
        "terrain {}x{} wrap_x={}: {} revealed, {} visible, {} hills, {} mountains, {} water, \
         {} with rivers, {} with features, {} owned, {} of unknown terrain, continent arts {arts:?}",
        snapshot.width(),
        snapshot.height(),
        snapshot.wrap_x(),
        count(&|c| c.is_revealed()),
        count(&|c| c.fog == Fog::Visible),
        count(&|c| c.is_revealed() && c.relief == Relief::Hill),
        count(&|c| c.is_revealed() && c.relief == Relief::Mountain),
        count(&|c| c.is_revealed() && c.relief == Relief::Water),
        count(&|c| c.rivers != 0),
        count(&|c| c.feat != FeatureKind::None),
        count(&|c| c.owner >= 0 && c.is_revealed()),
        count(&|c| c.is_revealed() && c.kind == TerrainKind::Unknown),
    );
}

/// First unit of the human, else the middle of what the human sees (the real world has no units
/// until F9; the start area is the visible part of the map), else the middle of the map.
fn start_focus(world: &WorldView, shape: &GridShape) -> Vec2 {
    if let Some(u) = world.units.iter().find(|u| u.owner == world.human) {
        return cell_center(i32::from(u.x), i32::from(u.y));
    }
    match visible_middle(&world.map) {
        Some((x, y)) => cell_center(x, y),
        None => cell_center(shape.width / 2, shape.height / 2),
    }
}

/// The cell in the middle of the visible plots (the revealed ones if none is visible). Along a
/// wrapping X the middle is the circular mean, so an area across the seam stays together.
fn visible_middle(map: &MapView) -> Option<(i32, i32)> {
    let width = usize::from(map.width);
    if width == 0 {
        return None;
    }
    let cells = |flag: u16| {
        map.plots
            .iter()
            .enumerate()
            .filter(move |(_, p)| p.flags.contains(flag))
            .map(move |(i, _)| ((i % width) as f64, (i / width) as f64))
    };
    let mut picked: Vec<(f64, f64)> = cells(PlotFlags::VISIBLE).collect();
    if picked.is_empty() {
        picked = cells(PlotFlags::REVEALED).collect();
    }
    if picked.is_empty() {
        return None;
    }
    let n = picked.len() as f64;
    let y = picked.iter().map(|c| c.1).sum::<f64>() / n;
    let x = if map.wrap_x {
        let w = width as f64;
        let (s, c) = picked.iter().fold((0.0, 0.0), |(s, c), p| {
            let a = p.0 / w * std::f64::consts::TAU;
            (s + a.sin(), c + a.cos())
        });
        (s.atan2(c) / std::f64::consts::TAU * w).rem_euclid(w)
    } else {
        picked.iter().map(|c| c.0).sum::<f64>() / n
    };
    let w = i32::from(map.width);
    let x = (x.round() as i32).rem_euclid(w);
    Some((x, y.round() as i32))
}

#[cfg(test)]
mod tests {
    use game_host::fixture;
    use render::GameCamera;
    use render::terrain::TRIANGLE_BUDGET;

    use super::super::tests::{fixture_world, wrapped_world};
    use super::*;

    /// A feed with the fixture's catalog, and the camera it works on.
    fn feed_with_fixture() -> (TerrainFeed, WorldView, GameCamera) {
        let (world, catalog) = fixture_world();
        let mut feed = TerrainFeed::default();
        feed.on_catalog(&catalog);
        (feed, world, GameCamera::default())
    }

    fn run(
        feed: &mut TerrainFeed,
        world: &WorldView,
        change: &ViewChange,
        camera: &mut GameCamera,
    ) {
        feed.apply(
            world,
            change,
            &mut FeedCx {
                basis: None,
                camera,
            },
        );
    }

    fn plots_changed(list: Vec<u32>) -> ViewChange {
        ViewChange {
            sections: mask(&[section::PLOTS]),
            dirty_plots: Some(list),
            ..ViewChange::default()
        }
    }

    #[test]
    fn snapshot_matches_the_fixture_map() {
        let (world, catalog) = fixture_world();
        let snap = terrain_snapshot(&world.map, &TerrainTable::from_catalog(&catalog)).unwrap();
        assert_eq!((snap.width(), snap.height()), (40, 24));
        for (i, (p, c)) in world.map.plots.iter().zip(snap.cells()).enumerate() {
            let revealed = p.flags.contains(PlotFlags::REVEALED);
            assert_eq!(c.is_revealed(), revealed, "plot {i}");
            assert_eq!(c.fog == Fog::Visible, p.flags.contains(PlotFlags::VISIBLE));
            if *p == PlotView::default() {
                assert_eq!(*c, TerrainCell::default());
            } else {
                // Revealed or not: the whole world (NP-D19).
                assert_eq!(c.rivers, p.rivers);
                assert_eq!(c.owner, p.owner);
                assert!(revealed || c.owner < 0, "plot {i}");
                assert_eq!(c.art, p.continent_art);
                assert_ne!(c.kind, TerrainKind::Unknown, "plot {i}");
                assert_eq!(c.relief == Relief::Water, p.plot_type == PlotType::Water);
                assert_eq!(
                    matches!(c.kind, TerrainKind::Coast | TerrainKind::Ocean),
                    p.plot_type == PlotType::Water
                );
            }
        }
        // Every relief and every fog state is present.
        for r in [Relief::Flat, Relief::Hill, Relief::Mountain, Relief::Water] {
            assert!(snap.cells().iter().any(|c| c.relief == r), "{r:?}");
        }
        for f in [Fog::Unrevealed, Fog::Hidden, Fog::Visible] {
            assert!(snap.cells().iter().any(|c| c.fog == f), "{f:?}");
        }
    }

    #[test]
    fn fixture_mesh_has_no_cracks_and_fits_the_budget() {
        let (mut feed, world, mut camera) = feed_with_fixture();
        run(&mut feed, &world, &ViewChange::all(), &mut camera);
        assert_eq!(feed.last_change(), TerrainChange::Rebuilt);
        let (snap, mesh) = feed.terrain().unwrap();
        mesh.check_no_cracks(snap).unwrap();
        let tris = mesh.triangle_count();
        eprintln!(
            "fixture 40x24: {tris} triangles, {} vertices, land {} + water {}",
            mesh.vertices.len(),
            mesh.land.len() / 3,
            mesh.water.len() / 3
        );
        assert!(tris <= TRIANGLE_BUDGET, "{tris} triangles");
        assert!(tris <= 40 * 24 * 42, "per-cell worst case is 42 triangles");
        assert!(mesh.indices_valid());
    }

    #[test]
    fn fixture_mesh_has_no_cracks_when_wrapping() {
        let (world, catalog) = fixture_world();
        let mut map = world.map;
        map.wrap_x = true;
        let snap = terrain_snapshot(&map, &TerrainTable::from_catalog(&catalog)).unwrap();
        let mesh = TerrainMesh::build(&snap);
        mesh.check_no_cracks(&snap).unwrap();
        assert!(mesh.triangle_count() <= TRIANGLE_BUDGET);
    }

    #[test]
    fn apply_rebuilds_only_when_the_relief_changes() {
        let (mut feed, world, mut camera) = feed_with_fixture();
        run(&mut feed, &world, &ViewChange::all(), &mut camera);
        assert_eq!(feed.last_change(), TerrainChange::Rebuilt);
        run(&mut feed, &world, &ViewChange::all(), &mut camera);
        assert_eq!(feed.last_change(), TerrainChange::None);

        // Fog only: a revealed plot becomes visible.
        let mut fogged = world.clone();
        let at = fogged
            .map
            .plots
            .iter()
            .position(|p| {
                p.flags.contains(PlotFlags::REVEALED) && !p.flags.contains(PlotFlags::VISIBLE)
            })
            .unwrap();
        let p = &mut fogged.map.plots[at];
        p.flags = PlotFlags(p.flags.0 | PlotFlags::VISIBLE);
        run(
            &mut feed,
            &fogged,
            &plots_changed(vec![at as u32]),
            &mut camera,
        );
        assert_eq!(feed.last_change(), TerrainChange::Recolored);
        assert_eq!(feed.mesh_builds(), 1);

        // A hill appears.
        let mut changed = fogged.clone();
        let at = changed
            .map
            .plots
            .iter()
            .position(|p| p.plot_type == PlotType::Land && p.flags.contains(PlotFlags::REVEALED))
            .unwrap();
        changed.map.plots[at].plot_type = PlotType::Hill;
        run(
            &mut feed,
            &changed,
            &plots_changed(vec![at as u32]),
            &mut camera,
        );
        assert_eq!(feed.last_change(), TerrainChange::Rebuilt);
        assert_eq!(feed.mesh_builds(), 2);
    }

    #[test]
    fn a_dirty_list_gives_the_same_snapshot_as_a_full_conversion() {
        let (mut feed, world, mut camera) = feed_with_fixture();
        run(&mut feed, &world, &ViewChange::all(), &mut camera);
        let mut next = world.clone();
        let hill = next
            .map
            .plots
            .iter()
            .position(|p| p.plot_type == PlotType::Land && p.flags.contains(PlotFlags::REVEALED))
            .unwrap();
        next.map.plots[hill].plot_type = PlotType::Hill;
        next.map.plots[3].rivers ^= 1;
        run(
            &mut feed,
            &next,
            &plots_changed(vec![3, hill as u32]),
            &mut camera,
        );
        let (_, catalog) = fixture_world();
        let want = terrain_snapshot(&next.map, &TerrainTable::from_catalog(&catalog))
            .unwrap()
            .with_owner_colors(owner_colors(&next.players));
        assert!(feed.terrain().unwrap().0 == &want);
    }

    #[test]
    fn a_header_only_change_does_nothing_and_a_new_size_rebuilds() {
        let (mut feed, mut world, mut camera) = feed_with_fixture();
        run(&mut feed, &world, &ViewChange::all(), &mut camera);
        world.turn += 1;
        let header = ViewChange {
            sections: mask(&[section::HEADER]),
            ..ViewChange::default()
        };
        run(&mut feed, &world, &header, &mut camera);
        assert_eq!(feed.last_change(), TerrainChange::None);
        assert_eq!(feed.mesh_builds(), 1);
        // The map shrinks by a row (another game): the header says so and the mesh is built again.
        world.map.height -= 1;
        world
            .map
            .plots
            .truncate(usize::from(world.map.width) * usize::from(world.map.height));
        run(&mut feed, &world, &header, &mut camera);
        assert_eq!(feed.last_change(), TerrainChange::Rebuilt);
        assert_eq!(feed.mesh_builds(), 2);
    }

    #[test]
    fn wrapped_fixture_gives_the_camera_a_wrapping_x() {
        let (world, catalog) = wrapped_world();
        assert!(world.map.wrap_x);
        let mut feed = TerrainFeed::default();
        feed.on_catalog(&catalog);
        let mut camera = GameCamera::default();
        run(&mut feed, &world, &ViewChange::all(), &mut camera);
        let w = 64.0 * f32::from(fixture::WIDTH);
        assert_eq!(camera.bounds().unwrap().wrap_width, Some(w));
        let (snap, mesh) = feed.terrain().unwrap();
        assert!(snap.wrap_x());
        mesh.check_no_cracks(snap).unwrap();
        // Scrolling east past the last column comes out at the west edge.
        camera.center_on(glam::Vec2::new(w - 5.0, -300.0));
        camera.scroll(glam::Vec2::new(1.0, 0.0), 0.1);
        assert!(camera.focus().x < 200.0, "{}", camera.focus().x);
    }

    #[test]
    fn first_world_sets_camera_bounds_and_focus() {
        let (mut feed, world, mut camera) = feed_with_fixture();
        run(&mut feed, &world, &ViewChange::all(), &mut camera);
        let b = camera.bounds().expect("bounds set");
        assert_eq!(b.wrap_width, None);
        assert!(b.max.x > 39.0 * 64.0 && b.min.y < -22.0 * 55.0);
        let unit = world.units.iter().find(|u| u.owner == world.human).unwrap();
        let want = cell_center(i32::from(unit.x), i32::from(unit.y));
        assert!(camera.focus().distance(want) < 1.0);
    }

    #[test]
    fn without_units_the_camera_starts_on_the_visible_area() {
        let (mut feed, mut world, mut camera) = feed_with_fixture();
        world.units.clear();
        let (x, y) = visible_middle(&world.map).expect("the fixture has visible plots");
        let i = usize::try_from(y).unwrap() * usize::from(world.map.width);
        assert!(x >= 0 && x < i32::from(world.map.width) && i < world.map.plots.len());
        run(&mut feed, &world, &ViewChange::all(), &mut camera);
        assert!(camera.focus().distance(cell_center(x, y)) < 1.0);

        // Across the seam of a wrapping map: columns 0 and W-1 meet at the edge, not the middle.
        let mut map = world.map.clone();
        map.wrap_x = true;
        let w = usize::from(map.width);
        for (i, p) in map.plots.iter_mut().enumerate() {
            let col = i % w;
            p.flags = if (i / w == 3) && (col == 0 || col == w - 1) {
                PlotFlags(PlotFlags::REVEALED | PlotFlags::VISIBLE)
            } else {
                PlotFlags(0)
            };
        }
        let (x, y) = visible_middle(&map).unwrap();
        assert_eq!(y, 3);
        assert!(x == 0 || x == i32::from(map.width) - 1, "x = {x}");
        // Nothing visible: the revealed plots count; nothing at all: none.
        map.plots[w * 5 + 2].flags = PlotFlags(PlotFlags::REVEALED);
        for (i, p) in map.plots.iter_mut().enumerate() {
            if i != w * 5 + 2 {
                p.flags = PlotFlags(0);
            }
        }
        assert_eq!(visible_middle(&map), Some((2, 5)));
        map.plots[w * 5 + 2].flags = PlotFlags(0);
        assert_eq!(visible_middle(&map), None);
    }

    #[test]
    fn owner_colors_come_from_the_players() {
        let (world, _) = fixture_world();
        let colors = owner_colors(&world.players);
        assert_eq!(colors.len(), world.players.len());
        let human = world.players.iter().find(|p| p.id == world.human).unwrap();
        let [r, g, b, _] = human.color_primary.to_be_bytes();
        assert_eq!(colors[usize::try_from(human.id).unwrap()], [r, g, b]);
    }

    #[test]
    fn bad_plot_count_is_ignored() {
        let (mut feed, mut world, mut camera) = feed_with_fixture();
        world.map.plots.pop();
        run(&mut feed, &world, &ViewChange::all(), &mut camera);
        assert_eq!(feed.last_change(), TerrainChange::None);
        assert!(feed.terrain().is_none());
    }

    #[test]
    fn catalog_names_map_to_kinds() {
        let (_, catalog) = fixture_world();
        let t = TerrainTable::from_catalog(&catalog);
        assert_eq!(t.kind(0), TerrainKind::Grass);
        assert_eq!(t.kind(5), TerrainKind::Coast);
        assert_eq!(t.kind(6), TerrainKind::Ocean);
        assert_eq!(t.kind(99), TerrainKind::Unknown);
        assert_eq!(fixture::TERRAINS.len(), 7);
    }

    /// A `MapBasis` of the fixture's map: the relief of the view, with the natural wonder of
    /// the catalog on plot 0 when the catalog has one.
    fn map_basis_of(world: &WorldView, wonder: Option<i16>) -> MapBasis {
        let plots = world
            .map
            .plots
            .iter()
            .map(|p| PlotBasisView {
                plot_type: p.plot_type,
                terrain: p.terrain,
                continent_art: p.continent_art,
                feature: -1,
                flags: PlotFlags(
                    p.flags.0
                        & (PlotFlags::IS_LAKE | PlotFlags::IS_COASTAL | PlotFlags::NATURAL_WONDER),
                ),
                rivers: p.rivers,
                river_flow: [-1; 3],
            })
            .collect();
        let mut basis = MapBasis {
            game: 1,
            basis_hash: 0,
            width: world.map.width,
            height: world.map.height,
            wrap_x: world.map.wrap_x,
            wrap_y: false,
            terrain_seed: 77,
            plots,
        };
        if let Some(f) = wonder {
            basis.plots[0].feature = f;
            basis.plots[0].flags = PlotFlags(PlotFlags::NATURAL_WONDER);
        }
        basis
    }

    /// Mod API 1.2: the mods' terrain share reaches the renderer as the records of the basis (the same conversion as
    /// the `MapBasis`), and nothing of it goes into the view's feeds: the fog the fog feed hands over (the minimap's,
    /// the camera bounds') still has the shared plots unrevealed, and the snapshot's cells too.
    #[test]
    fn a_terrain_share_is_the_basis_records_and_leaves_the_fog_of_the_view() {
        use crate::bridge::{FogFeed, WorldBridge};
        let (world, catalog) = fixture_world();
        let basis = map_basis_of(&world, None);
        let dark: Vec<u32> = world
            .map
            .plots
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.flags.contains(PlotFlags::REVEALED))
            .map(|(i, _)| i as u32)
            .take(3)
            .collect();
        assert_eq!(dark.len(), 3, "the fixture has unrevealed plots");
        let mut share = client_core::SharedTerrain::default();
        share.apply(protocol::TerrainShare {
            mod_idx: 0,
            reset: false,
            plots: dark
                .iter()
                .map(|&i| (i, basis.plots[i as usize].clone()))
                .collect(),
        });
        let mut bridge = WorldBridge::new();
        bridge.set_catalog(&catalog);
        let mut camera = render::GameCamera::default();
        bridge.sync(&world, Some(&basis), &ViewChange::all(), &mut camera);
        let records = bridge.ground_share(&share);
        let whole = terrain_basis(&basis, &TerrainTable::from_catalog(&catalog)).unwrap();
        let want: Vec<(u32, PlotBasis)> =
            dark.iter().map(|&i| (i, whole.plots[i as usize])).collect();
        assert_eq!(records, want);
        // The view's fog and cells do not know of it.
        let mut fog = FogFeed::default();
        fog.apply(
            &world,
            &ViewChange::all(),
            &mut FeedCx {
                basis: Some(&basis),
                camera: &mut camera,
            },
        );
        let cells = &fog.pending().expect("a fog").cells;
        let (snapshot, _) = bridge.terrain().expect("a snapshot");
        for &i in &dark {
            assert_eq!(cells[i as usize], Fog::Unrevealed);
            assert_eq!(snapshot.cells()[i as usize].fog, Fog::Unrevealed);
        }
    }

    #[test]
    fn a_basis_record_is_converted_by_the_names_of_the_catalog() {
        let (world, catalog) = fixture_world();
        let table = TerrainTable::from_catalog(&catalog);
        // Every feature of the catalog is the one its name says.
        for e in &catalog.features {
            assert_eq!(
                table.basis_feature(e.id),
                BasisFeature::from_type_name(&e.type_name),
                "{}",
                e.type_name
            );
        }
        assert_eq!(table.basis_feature(-1), BasisFeature::None);
        assert_eq!(table.basis_feature(900), BasisFeature::Other);

        let wonder = catalog
            .features
            .iter()
            .find(|e| e.type_name == "FEATURE_FUJI")
            .map(|e| e.id);
        let basis = map_basis_of(&world, wonder);
        let tb = terrain_basis(&basis, &table).expect("the plot count matches");
        assert_eq!(
            (tb.width, tb.height, tb.wrap_x, tb.seed),
            (world.map.width, world.map.height, world.map.wrap_x, 77)
        );
        for (p, q) in world.map.plots.iter().zip(&tb.plots).skip(1) {
            let want = match p.plot_type {
                PlotType::Land => Relief::Flat,
                PlotType::Hill => Relief::Hill,
                PlotType::Mountain => Relief::Mountain,
                PlotType::Water => Relief::Water,
            };
            assert_eq!(q.relief, want);
            assert_eq!(q.continent, p.continent_art);
            assert_eq!(q.rivers, p.rivers & 0x3f);
            assert_ne!(q.terrain, TerrainKind::Unknown);
            assert_eq!(
                q.flags.contains(BasisFlags::LAKE),
                p.flags.contains(PlotFlags::IS_LAKE)
            );
        }
        if wonder.is_some() {
            assert!(tb.plots[0].feature.is_natural_wonder());
            assert!(tb.plots[0].flags.contains(BasisFlags::NATURAL_WONDER));
        }
        // A plot count that does not match the size is no basis.
        let mut bad = basis;
        bad.plots.pop();
        assert!(terrain_basis(&bad, &table).is_none());
    }

    #[test]
    fn the_b1_path_owes_the_basis_of_the_snapshot_and_then_only_what_changed() {
        let (mut feed, world, mut camera) = feed_with_fixture();
        run(&mut feed, &world, &ViewChange::all(), &mut camera);
        let Some(BasisUpdate::Set(first)) = feed.pending_basis().cloned() else {
            panic!("the first view owes a whole basis");
        };
        assert_eq!((first.width, first.height), (40, 24));
        // The renderer takes it.
        feed.basis_out = None;

        // The same view again owes nothing.
        run(&mut feed, &world, &ViewChange::all(), &mut camera);
        assert!(feed.pending_basis().is_none());

        // A hill appears: one record.
        let mut changed = world.clone();
        let at = changed
            .map
            .plots
            .iter()
            .position(|p| p.plot_type == PlotType::Land && p.flags.contains(PlotFlags::REVEALED))
            .unwrap();
        changed.map.plots[at].plot_type = PlotType::Hill;
        run(
            &mut feed,
            &changed,
            &plots_changed(vec![at as u32]),
            &mut camera,
        );
        match feed.pending_basis() {
            Some(BasisUpdate::Patch(c)) => {
                assert_eq!(c.len(), 1);
                assert_eq!(c[0].0, at as u32);
                assert_eq!(c[0].1.relief, Relief::Hill);
            }
            other => panic!("a patch of one record, got {other:?}"),
        }
        // A patch that is not taken yet and another one are one.
        let at2 = changed
            .map
            .plots
            .iter()
            .enumerate()
            .position(|(i, p)| {
                i != at && p.plot_type == PlotType::Land && p.flags.contains(PlotFlags::REVEALED)
            })
            .unwrap();
        let mut more = changed.clone();
        more.map.plots[at2].plot_type = PlotType::Mountain;
        run(
            &mut feed,
            &more,
            &plots_changed(vec![at2 as u32]),
            &mut camera,
        );
        match feed.pending_basis() {
            Some(BasisUpdate::Patch(c)) => assert_eq!(c.len(), 2),
            other => panic!("two records, got {other:?}"),
        }
    }

    #[test]
    fn a_map_basis_replaces_the_adapter_and_its_diffs_are_patches() {
        let (mut feed, world, mut camera) = feed_with_fixture();
        let (_, catalog) = fixture_world();
        let table = TerrainTable::from_catalog(&catalog);
        let basis = map_basis_of(&world, None);

        // A new `MapBasis` (basis_plots None): the whole basis, with the seed of the game.
        let new = ViewChange {
            sections: mask(&[section::BASIS, section::PLOTS, section::HEADER]),
            basis_plots: None,
            ..ViewChange::default()
        };
        feed.apply(
            &world,
            &new,
            &mut FeedCx {
                basis: Some(&basis),
                camera: &mut camera,
            },
        );
        let Some(BasisUpdate::Set(set)) = feed.pending_basis().cloned() else {
            panic!("a whole basis");
        };
        assert_eq!(set.seed, 77);
        assert_eq!(set, terrain_basis(&basis, &table).unwrap());
        // The snapshot of the view was built too; the adapter did not send a second basis.
        assert!(feed.terrain().is_some());
        feed.basis_out = None;

        // A `basis` diff names the records: a patch with the converted ones.
        let mut b2 = basis.clone();
        b2.plots[5].plot_type = PlotType::Mountain;
        b2.plots[9].rivers ^= 1;
        let diff = ViewChange {
            sections: mask(&[section::BASIS]),
            basis_plots: Some(vec![5, 9]),
            ..ViewChange::default()
        };
        feed.apply(
            &world,
            &diff,
            &mut FeedCx {
                basis: Some(&b2),
                camera: &mut camera,
            },
        );
        match feed.pending_basis() {
            Some(BasisUpdate::Patch(c)) => {
                let idx: Vec<u32> = c.iter().map(|(i, _)| *i).collect();
                assert_eq!(idx, [5, 9]);
                assert_eq!(c[0].1.relief, Relief::Mountain);
                assert_eq!(c[1].1.rivers, (world.map.plots[9].rivers ^ 1) & 0x3f);
            }
            other => panic!("a patch, got {other:?}"),
        }
        // A view change that is none of the basis owes nothing more.
        feed.basis_out = None;
        run(&mut feed, &world, &plots_changed(vec![3]), &mut camera);
        assert!(feed.pending_basis().is_none());
    }

    #[test]
    fn a_patch_folds_into_a_basis_that_was_not_taken() {
        let (_, catalog) = fixture_world();
        let table = TerrainTable::from_catalog(&catalog);
        let (world, _) = fixture_world();
        let basis = terrain_basis(&map_basis_of(&world, None), &table).unwrap();
        let mut hill = basis.plots[3];
        hill.relief = Relief::Hill;
        let owed = BasisUpdate::Set(basis.clone()).then(BasisUpdate::Patch(vec![(3, hill)]));
        match owed {
            BasisUpdate::Set(b) => assert_eq!(b.plots[3], hill),
            other => panic!("{other:?}"),
        }
        // A new basis replaces what was owed.
        let owed = BasisUpdate::Patch(vec![(3, hill)]).then(BasisUpdate::Set(basis.clone()));
        assert_eq!(owed, BasisUpdate::Set(basis));
    }
}
