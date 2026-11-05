//! `Landmark` subsystem (world-objects WOB0): what lies on the plots, by
//! plot key (`y << 16 | x`): resource, improvement, feature, route.
//!
//! B1 draws these as icons on the ground and lines for the roads (`entities::icons`,
//! `entities::roads`, through `frame::b1_systems::B1Icons` and `B1Landmark`); the plots are kept
//! here and the renderer builds those meshes from them when a plot changes.
//!
//! WOB4 is the second placement beside it, for the models of `assets-b2` (`refresh_b2`), like the
//! units' (`world::unit`): [`select`] chooses the rows of `ArtDefine_Landmarks` for a plot (its
//! improvement or the one being built, its resource, its feature; the era, the technologies and the
//! state of the owner), [`layout`] and [`snapshot`] say how the parts of the model stand on the plot,
//! the pieces are placed on the ground (`ground`, and the water for a plot at sea), and
//! `AnimService` plays the clips of the model (`sync_anim`). The farms' fields are decals
//! ([`decals`]). The draw is a second [`UnitDraw`] that records `Main.Landmark` (the families are
//! chosen by the material of the mesh, `LandmarkShader` for the landmarks).
//!
//! With the models in, the icons of the improvements, camps and ruins go; the resource keeps its icon
//! under the switch `YieldIcons` ([`icons`]). The roads are still B1's lines (WOB6).

use std::collections::{BTreeMap, BTreeSet};

use asset_format::{LandmarkArt, LogicalId, ModelBlobV2};
use glam::{Mat4, Vec2, Vec3};

use crate::anim::landmark_tree::anim_state;
use crate::anim::{AnimHandle, AnimService, AnimState, AnimTarget, BoneRef};
use crate::entities::{PlotDecor, PlotKey, plot_key};
use crate::frame::{
    DrawContext, InitContext, MainSlot, MaskContext, PassId, PassSlot, PrepareContext,
    RenderSystem, SystemId,
};
use crate::hexgrid::cell_center;
use crate::world::objects::input::{Keyed, ObjectCounters};
use crate::world::objects::instance::ObjectInstance;
use crate::world::unit::draw::{self, PartBatches};
use crate::world::unit::place::push_to;
use crate::world::unit::{B2Context, B2Frame};

pub mod decals;
pub mod ground;
pub mod heading;
pub mod icons;
pub mod layout;
pub mod routes;
pub mod select;
pub mod smoke;
pub mod snapshot;

use decals::Farm;
use layout::{Layout, PartInfo};
use select::{LandmarkEra, Owner};

/// What the layout needs of a model, kept when it arrives.
#[derive(Debug, Clone)]
struct ModelMeta {
    parts: Vec<PartMeta>,
    /// The `Tree Cut` meshes (`LandmarkShader_Stencil`): their part and triangles, model space.
    cuts: Vec<(u16, Vec<[Vec3; 3]>)>,
    /// The ground meshes, decals of the terrain ([`ground::GroundMesh`]).
    grounds: Vec<ground::GroundMesh>,
}

#[derive(Debug, Clone)]
struct PartMeta {
    name: String,
    identity: bool,
    /// `InitialPlacement` as a matrix of our basis.
    placement: Mat4,
}

impl ModelMeta {
    fn of(blob: &ModelBlobV2) -> Self {
        Self {
            parts: blob
                .parts
                .iter()
                .map(|p| {
                    let m = p.placement.to_mat4();
                    PartMeta {
                        name: p.name.clone(),
                        identity: m.abs_diff_eq(Mat4::IDENTITY, 1e-4),
                        placement: m,
                    }
                })
                .collect(),
            cuts: blob
                .meshes
                .iter()
                .filter(|m| {
                    blob.materials
                        .get(usize::from(m.material))
                        .is_some_and(crate::material::stencil_only)
                })
                .filter_map(|m| {
                    let pos = m.positions()?;
                    let idx = m.indices.to_u32();
                    let tris = idx
                        .chunks_exact(3)
                        .filter_map(|t| {
                            let p = |i: u32| pos.get(i as usize).map(|&v| Vec3::from(v));
                            Some([p(t[0])?, p(t[1])?, p(t[2])?])
                        })
                        .collect();
                    Some((m.part, tris))
                })
                .collect(),
            grounds: ground::GroundMesh::of_model(blob),
        }
    }

    fn infos(&self) -> Vec<PartInfo<'_>> {
        self.parts
            .iter()
            .enumerate()
            .map(|(i, p)| PartInfo {
                index: i as u16,
                name: &p.name,
                identity: p.identity,
            })
            .collect()
    }
}

/// One draw of a landmark: the whole model, or one part of it, at a place.
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    /// `None`: all the meshes of the model.
    pub part: Option<u16>,
    /// Model space to our world.
    pub matrix: Mat4,
}

/// One landmark of a plot.
#[derive(Debug, Clone)]
pub struct Placed {
    /// The place of the landmark among those of the plot (the key of its animation).
    pub slot: u16,
    pub model: LogicalId,
    /// Empty while the model has not come (`pending`), or when the row has no model.
    pub pieces: Vec<Piece>,
    pub anim: AnimState,
    /// The model is not here yet: the plot is placed again when it comes.
    pub pending: bool,
    /// The plot has a farm of this landmark: its fields are decals.
    pub farm: Option<Farm>,
}

/// The farms of the plots and the plots to give the layer of decals: what changed in the fields.
pub type FarmUpdate = (Vec<Farm>, BTreeSet<(i32, i32)>);

/// The instances of a frame, the placing they were made for and the bone references they hold.
type InstCache = (
    u64,
    Vec<BoneRef>,
    (
        BTreeMap<LogicalId, Vec<ObjectInstance>>,
        PartBatches,
        draw::StateBatches,
        draw::PartStateBatches,
    ),
);

/// The triangles of the cuts that a plot's placing took out and put in (world `(X, Z)`).
pub type CutChange = (Vec<[Vec2; 3]>, Vec<[Vec2; 3]>);

/// The ground the `Tree Cut` meshes of one plot's landmarks cover, world `(X, Z)`. A mesh on a bone
/// is taken in the pose of its model.
fn cut_triangles(models: &BTreeMap<LogicalId, ModelMeta>, list: &[Placed]) -> Vec<[Vec2; 3]> {
    let mut tris = Vec::new();
    for p in list {
        let Some(meta) = models.get(&p.model) else {
            continue;
        };
        for (part, cut) in &meta.cuts {
            for piece in p
                .pieces
                .iter()
                .filter(|x| x.part.is_none_or(|q| q == *part))
            {
                tris.extend(cut.iter().map(|t| {
                    t.map(|v| {
                        let w = piece.matrix.transform_point3(v);
                        Vec2::new(w.x, w.z)
                    })
                }));
            }
        }
    }
    tris
}

/// Calls of `sync_anim` between two looks when nothing was placed.
const SYNC_EVERY: u32 = 60;

/// The draw of WOB4 and what `prepare` needs of the frame that `b2_frame` left.
struct B2State {
    draw: draw::UnitDraw,
    active: bool,
    shifts: Vec<f32>,
    eye: Vec3,
}

/// The plots with something on them.
#[derive(Default)]
pub struct Landmark {
    plots: Keyed<PlotKey, PlotDecor>,
    counters: ObjectCounters,
    /// WOB4: the plots to place again for the models of `assets-b2`.
    dirty_b2: BTreeSet<PlotKey>,
    placed: BTreeMap<PlotKey, Vec<Placed>>,
    models: BTreeMap<LogicalId, ModelMeta>,
    /// The animation of each landmark that has one, and the state it was last given.
    /// The third is the pass of `sync_anim` that last saw the landmark.
    handles: BTreeMap<(PlotKey, u16), (AnimHandle, AnimState, u32, LogicalId)>,
    anim_epoch: u32,
    /// `Catalog.art_techs` by name, in the bit order of `PlayerView.art_techs`.
    techs: Vec<String>,
    /// The plots whose farm changed since `take_farm_update` (`farm_all`: every farm), and the plots
    /// whose fields the layer holds.
    farm_dirty: BTreeSet<(i32, i32)>,
    farm_all: bool,
    farmed: BTreeSet<(i32, i32)>,
    placed_b2_last: u32,
    placed_b2_total: u64,
    /// Bumped whenever `placed` or the routes change; with the bone references it decides whether
    /// the instances of the last frame are still the instances of this one.
    place_gen: u64,
    inst_cache: Option<InstCache>,
    /// `sync_anim` has work only after a placing or a model; every [`SYNC_EVERY`] calls it looks
    /// anyway (a handle the service dropped).
    sync_dirty: bool,
    sync_calls: u32,
    instances_reused: u64,
    /// The placed landmarks changed since `take_cuts_dirty`: the trees may stand on other cuts.
    cuts_dirty: bool,
    /// The cuts that single placings changed since `take_cut_changes`.
    cut_changes: Vec<CutChange>,
    b2: Option<Box<B2State>>,
    /// WOB6: the roads, railroads and bridges of the plots.
    routes: routes::RouteLayer,
    /// The pillaged plots that are visible now (see [`Landmark::set_seen`]).
    seen: BTreeSet<PlotKey>,
    /// The ground meshes changed since `grounds_applied`, and the hexes whose ground decals the
    /// layer holds ([`ground`]).
    grounds_dirty: BTreeSet<PlotKey>,
    grounds_all: bool,
    grounded: BTreeMap<u32, Vec<crate::world::terrain::decals::MeshDecal>>,
    /// The fingerprint of the ground under each plot as it was placed, and the plots whose ground
    /// is to be compared with it (the terrain built cells).
    ground_fp: BTreeMap<PlotKey, u64>,
    height_check: BTreeSet<PlotKey>,
    height_check_all: bool,
    /// Bumped when a plot is added, changed or dropped (a cheap gate for the per-frame feeds).
    epoch: u64,
    height_checked: u64,
    deferred: u64,
    /// The layout heading of the map: where a farm model stands and how it is turned.
    heading: heading::HeadingField,
}

/// The most plots one frame places again; the rest stay queued and keep what they are drawn with.
const PLACES_PER_FRAME: usize = 512;
/// The most plots one frame compares the ground of.
const HEIGHT_CHECKS_PER_FRAME: usize = 1024;

/// A fingerprint of the ground of a plot: the heights at its middle and round it.
fn plot_ground_fp(x: i32, y: i32, ground: &dyn Fn(Vec2) -> f32) -> u64 {
    let c = cell_center(x, y);
    let mut h = u64::from(ground(c).to_bits());
    for k in 0..6 {
        let a = k as f32 * std::f32::consts::FRAC_PI_3;
        let p = c + Vec2::new(a.cos(), a.sin()) * 30.0;
        h = (h ^ u64::from(ground(p).to_bits()))
            .wrapping_mul(0x0000_0100_0000_01b3)
            .rotate_left(23);
    }
    h
}

/// The ground decals of every landmark now, and the hexes to give the layer (those with one now and
/// those that had one).
pub type GroundUpdate = (Vec<crate::world::terrain::decals::MeshDecal>, Vec<u32>);

impl Landmark {
    /// Sets the plots in `changed`: `Some` puts something on the plot (or replaces what was
    /// there), `None` clears it. True if anything changed.
    pub fn set_plots(&mut self, changed: &[(PlotKey, Option<PlotDecor>)]) -> bool {
        let mut any = false;
        for (key, decor) in changed {
            let did = match decor {
                Some(d) => self.plots.upsert(*key, d.clone()).1,
                None => self.plots.remove(key).is_some(),
            };
            if did {
                self.dirty_b2.insert(*key);
                self.routes.touch(*key);
                self.epoch += 1;
            }
            any |= did;
        }
        any
    }

    /// Sets one plot by its coordinates.
    pub fn set_plot(&mut self, decor: PlotDecor) -> bool {
        let key = plot_key(decor.x, decor.y);
        let changed = self.plots.upsert(key, decor).1;
        if changed {
            self.dirty_b2.insert(key);
            self.routes.touch(key);
            self.epoch += 1;
        }
        changed
    }

    pub fn clear(&mut self) {
        self.epoch += 1;
        self.ground_fp.clear();
        self.height_check.clear();
        self.dirty_b2.extend(self.plots.keys().copied());
        self.seen.clear();
        self.plots.clear();
        self.routes.touch_all();
    }

    pub fn len(&self) -> usize {
        self.plots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plots.is_empty()
    }

    /// The plots in row-major order, the order of the snapshot.
    pub fn plots(&self) -> impl Iterator<Item = &PlotDecor> {
        self.plots.values()
    }

    pub fn get(&self, key: &PlotKey) -> Option<&PlotDecor> {
        self.plots.get(key)
    }

    /// The renderer made `instances` icons and line parts of the plots since the last change.
    pub fn set_assembled(&mut self, placed: usize, instances: usize) {
        self.plots.take_dirty();
        self.counters.placed_last = placed as u32;
        self.counters.placed_total += placed as u64;
        self.counters.instances = instances as u32;
        self.counters.batches = u32::from(instances > 0);
    }

    pub fn counters(&self) -> ObjectCounters {
        ObjectCounters {
            records: self.plots.len() as u32,
            ..self.counters
        }
    }

    /// Plots changed since the last `set_assembled`.
    pub fn changed_plots(&self) -> usize {
        self.plots.dirty_len()
    }

    // ---- WOB4: the models of `assets-b2` ----

    /// The technologies of the landmark art list (`Catalog.art_techs`), by type name, in the bit
    /// order of `PlayerView.art_techs`. The plots are placed again.
    pub fn set_art_techs(&mut self, names: Vec<String>) {
        if self.techs != names {
            self.techs = names;
            self.invalidate_all();
        }
    }

    /// Tells which pillaged plots are visible (`visible(x, y)`, fog of war): the smoke of a pillaged
    /// improvement shows on those only. A plot whose answer changed is placed again.
    pub fn set_seen(&mut self, visible: impl Fn(i32, i32) -> bool) {
        let now: BTreeSet<PlotKey> = self
            .plots
            .iter()
            .filter(|(_, p)| smoke::smoke_of(p).is_some() && visible(p.x, p.y))
            .map(|(k, _)| *k)
            .collect();
        if now != self.seen {
            self.dirty_b2
                .extend(now.symmetric_difference(&self.seen).copied());
            self.seen = now;
        }
    }

    /// Every plot is placed again at the next refresh (new art, owners or technologies), and its
    /// farm, cuts and grounds are made again.
    pub fn invalidate_all(&mut self) {
        self.dirty_b2.extend(self.plots.keys().copied());
        self.routes.touch_all();
        self.farm_all = true;
        // The cuts follow the placings one plot at a time (`mark_changed`): the plots are placed
        // again and each one that now cuts other ground says so, so no whole rebuild of the index.
        self.grounds_all = true;
    }

    /// The terrain built cells: the plots compare the ground under them with the one they were
    /// placed on (a good many per frame) and are placed again only if it differs.
    pub fn heights_changed(&mut self) {
        self.height_check_all = true;
    }

    /// The terrain built these cells (`(x, y)` of a hex): the plots on them or next to them compare
    /// the ground under them with the one they were placed on.
    pub fn heights_dirty(&mut self, cells: &[(i32, i32)]) {
        for &(cx, cy) in cells {
            for y in cy - 2..=cy + 2 {
                for x in cx - 2..=cx + 2 {
                    let key = plot_key(x, y);
                    if self.plots.get(&key).is_some() {
                        self.height_check.insert(key);
                    }
                }
            }
        }
    }

    /// True while plots wait to be placed again or to have their ground compared.
    pub fn placing_pending(&self) -> bool {
        self.b2_active()
            && (!self.dirty_b2.is_empty() || !self.height_check.is_empty() || self.height_check_all)
    }

    /// The record epoch: changes when a plot is added, changed or dropped.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// `(ground checks, placings deferred by the budget)` since the start.
    pub fn budget_counters(&self) -> (u64, u64) {
        (self.height_checked, self.deferred)
    }

    /// Frames that reused the instances of the frame before.
    pub fn instances_reused(&self) -> u64 {
        self.instances_reused
    }

    /// A model arrived: what the layout needs of it is kept and the plots that wait for it are
    /// placed again.
    pub fn register_model(&mut self, id: LogicalId, blob: &ModelBlobV2) {
        self.sync_dirty = true;
        self.models.insert(id.clone(), ModelMeta::of(blob));
        self.routes.register(id.clone(), blob);
        self.invalidate_model(&id);
    }

    /// The plots that draw `model` are placed again (it arrived or failed).
    pub fn invalidate_model(&mut self, model: &LogicalId) {
        for (key, list) in &self.placed {
            if list.iter().any(|p| &p.model == model) {
                self.dirty_b2.insert(*key);
            }
        }
        self.routes.invalidate_model(model);
    }

    /// True if a plot is waiting for `model`: the model is a landmark's or a route's.
    pub fn wants_model(&self, model: &LogicalId) -> bool {
        self.placed
            .values()
            .flatten()
            .any(|p| p.pending && &p.model == model)
            || self.routes.wants_model(model)
    }

    /// WOB6: the shape of the map and the plots that have a city, for the links of the routes.
    pub fn set_net(&mut self, shape: crate::hexgrid::GridShape, cities: BTreeSet<(i32, i32)>) {
        self.routes.set_net(shape, cities);
    }

    /// The viewer's bridges (`GameplayBridgeChanged`); `None` = not told. True if the routes are placed again.
    pub fn set_bridges(&mut self, state: Option<routes::BridgeState>) -> bool {
        self.routes.set_bridges(state)
    }

    /// WOB6: the road decals to make, if the links changed (see [`routes::RouteLayer::decal_update`]).
    pub fn route_decals(&self) -> Option<routes::DecalWork> {
        self.routes.decal_update()
    }

    /// How the road pieces are textured (the models that came), and whether a plot has a city.
    pub fn route_skins(&self) -> &routes::RoadSkins {
        self.routes.skins()
    }

    pub fn route_city(&self, x: i32, y: i32) -> bool {
        self.routes.is_city(x, y)
    }

    /// The road decals are made again (the coastal roads of the terrain changed).
    pub fn redo_route_decals(&mut self) {
        self.routes.redo_decals();
    }

    /// The decals of the roads were made for these links.
    pub fn route_decals_applied(&mut self, links: &[routes::DrawnLink]) {
        self.routes.decals_applied(links);
    }

    /// True if the routes of the plots are models now: the lines of B1 can go.
    pub fn routes_native(&self) -> bool {
        self.b2_active() && !self.routes.is_empty() && self.routes.models_wanted().is_empty()
    }

    /// The landmarks of a plot as the last refresh placed them.
    pub fn placed_at(&self, key: &PlotKey) -> &[Placed] {
        self.placed.get(key).map_or(&[], Vec::as_slice)
    }

    /// The layout heading of the map (from its terrain frame); a new one places the farms again.
    pub fn set_heading(&mut self, heading: heading::HeadingField) {
        if self.heading == heading {
            return;
        }
        self.heading = heading;
        let farms: Vec<PlotKey> = self
            .placed
            .iter()
            .filter(|(_, list)| list.iter().any(|p| p.farm.is_some()))
            .map(|(k, _)| *k)
            .collect();
        self.dirty_b2.extend(farms);
    }

    /// Places again the plots that changed, for the models of `assets-b2`; returns how many. A plot
    /// that is gone leaves nothing.
    pub fn refresh_b2(&mut self, ctx: &B2Context<'_>) -> usize {
        let routed = self.routes.refresh(&self.plots, ctx);
        if std::mem::take(&mut self.height_check_all) {
            self.height_check.extend(self.plots.keys().copied());
        }
        for _ in 0..HEIGHT_CHECKS_PER_FRAME {
            let Some(key) = self.height_check.pop_first() else {
                break;
            };
            self.height_checked += 1;
            let Some(plot) = self.plots.get(&key) else {
                continue;
            };
            let fp = plot_ground_fp(plot.x, plot.y, ctx.ground);
            if self.ground_fp.get(&key).is_some_and(|old| *old != fp) {
                self.dirty_b2.insert(key);
                self.routes.touch(key);
            }
        }
        let mut queue = std::mem::take(&mut self.dirty_b2);
        let mut n = 0usize;
        let start = crate::perf::slow::now_ms();
        while let Some(key) = queue.pop_first() {
            if n >= PLACES_PER_FRAME || (n > 0 && crate::perf::slow::now_ms() - start >= 3.0) {
                self.dirty_b2.insert(key);
                self.deferred += 1;
                continue;
            }
            n += 1;
            let Some(plot) = self.plots.get(&key) else {
                if let Some(old) = self.placed.remove(&key) {
                    self.mark_changed(key, &old, &[]);
                }
                self.ground_fp.remove(&key);
                continue;
            };
            let seen = self.seen.contains(&key);
            let list = place_plot(
                &self.models,
                &self.techs,
                ctx,
                &self.heading,
                key,
                plot,
                seen,
            );
            self.ground_fp
                .insert(key, plot_ground_fp(plot.x, plot.y, ctx.ground));
            let old = self.placed.remove(&key).unwrap_or_default();
            self.mark_changed(key, &old, &list);
            if !list.is_empty() {
                // The ground meshes of the model are the terrain's decals, not drawn as models.
                if let Some(b2) = self.b2.as_mut() {
                    for p in &list {
                        if self
                            .models
                            .get(&p.model)
                            .is_some_and(|m| !m.grounds.is_empty())
                        {
                            b2.draw.set_ground_decals(&p.model, true);
                        }
                    }
                }
                self.placed.insert(key, list);
            }
        }
        let total = n + routed;
        if total > 0 {
            self.place_gen += 1;
            self.sync_dirty = true;
        }
        self.placed_b2_last = total as u32;
        self.placed_b2_total += total as u64;
        total
    }

    /// What a plot's placing changed for the farms, the tree cuts and the ground decals.
    fn mark_changed(&mut self, key: PlotKey, old: &[Placed], new: &[Placed]) {
        let farms = |l: &[Placed]| l.iter().filter_map(|p| p.farm).collect::<Vec<_>>();
        let (fo, fnw) = (farms(old), farms(new));
        if fo != fnw {
            self.farm_dirty
                .extend(fo.iter().chain(&fnw).map(|f| (f.x, f.y)));
        }
        let cuts = |l: &[Placed]| {
            l.iter()
                .filter(|p| {
                    self.models
                        .get(&p.model)
                        .is_some_and(|m| !m.cuts.is_empty())
                })
                .map(|p| {
                    // The cuts are triangles in world (X, Z): only the rows of the matrix that make
                    // x and z count, not the height the landmark was lifted to.
                    let pieces: Vec<(Option<u16>, [f32; 8])> = p
                        .pieces
                        .iter()
                        .map(|q| {
                            let m = &q.matrix;
                            (
                                q.part,
                                [
                                    m.x_axis.x, m.y_axis.x, m.z_axis.x, m.w_axis.x, m.x_axis.z,
                                    m.y_axis.z, m.z_axis.z, m.w_axis.z,
                                ],
                            )
                        })
                        .collect();
                    (p.model.clone(), pieces)
                })
                .collect::<Vec<_>>()
        };
        // Everything is made again anyway while `cuts_dirty` waits; else this plot's change alone.
        if !self.cuts_dirty && self.b2_active() && cuts(old) != cuts(new) {
            self.cut_changes.push((
                cut_triangles(&self.models, old),
                cut_triangles(&self.models, new),
            ));
        }
        let grounds = |l: &[Placed]| {
            l.iter()
                .filter(|p| {
                    !p.pending
                        && self
                            .models
                            .get(&p.model)
                            .is_some_and(|m| !m.grounds.is_empty())
                })
                .map(|p| {
                    (
                        p.model.clone(),
                        p.pieces.clone(),
                        desired_state(p.anim.landmark),
                    )
                })
                .collect::<Vec<_>>()
        };
        if grounds(old) != grounds(new) {
            self.grounds_dirty.insert(key);
        }
    }

    /// How many plots the last `refresh_b2` placed, and since the start.
    pub fn placed_b2(&self) -> (u32, u64) {
        (self.placed_b2_last, self.placed_b2_total)
    }

    /// The farms have changed: the fields of every farm now, and the plots to give to the layer of
    /// decals (those with a farm now and those that had one). `None` when nothing changed.
    pub fn farm_update(&mut self) -> Option<FarmUpdate> {
        self.farm_update_of(usize::MAX)
    }

    /// [`Self::farm_update`] for at most `max_plots` plots: the rest wait for the next call, so a
    /// turn that changes many farms is given to the layer over several frames.
    pub fn farm_update_of(&mut self, max_plots: usize) -> Option<FarmUpdate> {
        if std::mem::take(&mut self.farm_all) {
            // Every farm: its plot, and those that had one, are dirty.
            let now: Vec<(i32, i32)> = self
                .placed
                .values()
                .flatten()
                .filter_map(|p| p.farm)
                .map(|f| (f.x, f.y))
                .collect();
            self.farm_dirty.extend(now);
            self.farm_dirty.extend(self.farmed.iter().copied());
        }
        if self.farm_dirty.is_empty() {
            return None;
        }
        let touched: BTreeSet<(i32, i32)> =
            self.farm_dirty.iter().take(max_plots).copied().collect();
        let mut farms: Vec<Farm> = self
            .placed
            .values()
            .flatten()
            .filter_map(|p| p.farm)
            .filter(|f| touched.contains(&(f.x, f.y)))
            .collect();
        farms.sort();
        farms.dedup();
        Some((farms, touched))
    }

    /// The layer got the fields of `farms` for the plots `touched` (from [`Self::farm_update`]).
    pub fn farms_applied(&mut self, farms: &[Farm], touched: &BTreeSet<(i32, i32)>) {
        for t in touched {
            self.farmed.remove(t);
            self.farm_dirty.remove(t);
        }
        self.farmed.extend(farms.iter().map(|f| (f.x, f.y)));
    }

    /// The ground meshes of the placed landmarks have changed: the decals of the hexes whose ground
    /// is not what the layer holds (none while the models of `assets-b2` are not drawn), and those
    /// hexes; the layer is to be given them (`DecalLayer::set_mesh_decals`). `width` is the width of
    /// the map in plots. `None` when nothing changed.
    pub fn take_ground_update(&mut self, width: i32) -> Option<GroundUpdate> {
        let all = std::mem::take(&mut self.grounds_all);
        if !all && self.grounds_dirty.is_empty() {
            return None;
        }
        let dirty = std::mem::take(&mut self.grounds_dirty);
        let cell_of = |key: &PlotKey| {
            let (x, y) = ((key & 0xffff) as i32, (key >> 16) as i32);
            (y * width + x) as u32
        };
        let dirty_cells: BTreeSet<u32> = dirty.iter().map(cell_of).collect();
        let mut now: BTreeMap<u32, Vec<crate::world::terrain::decals::MeshDecal>> = BTreeMap::new();
        if self.b2_active() {
            for (key, list) in &self.placed {
                if !all && !dirty.contains(key) {
                    continue;
                }
                let Some(plot) = self.plots.get(key) else {
                    continue;
                };
                let cell = (plot.y * width + plot.x) as u32;
                for p in list.iter().filter(|p| !p.pending) {
                    let Some(meta) = self.models.get(&p.model) else {
                        continue;
                    };
                    if meta.grounds.is_empty() {
                        continue;
                    }
                    let pieces: Vec<(Option<u16>, Mat4)> =
                        p.pieces.iter().map(|x| (x.part, x.matrix)).collect();
                    let d = ground::landmark_decals(
                        &meta.grounds,
                        &pieces,
                        desired_state(p.anim.landmark),
                        cell,
                    );
                    if !d.is_empty() {
                        now.entry(cell).or_default().extend(d);
                    }
                }
            }
        }
        let changed: Vec<u32> = now
            .keys()
            .chain(self.grounded.keys())
            .copied()
            .filter(|c| all || dirty_cells.contains(c))
            .collect::<BTreeSet<u32>>()
            .into_iter()
            .filter(|c| now.get(c) != self.grounded.get(c))
            .collect();
        if changed.is_empty() {
            return None;
        }
        let decals = changed
            .iter()
            .filter_map(|c| now.get(c))
            .flatten()
            .cloned()
            .collect();
        if all {
            self.grounded = now;
        } else {
            for c in &changed {
                match now.remove(c) {
                    Some(v) => self.grounded.insert(*c, v),
                    None => self.grounded.remove(c),
                };
            }
        }
        Some((decals, changed))
    }

    /// True once after the placed landmarks changed (the trees are gated by their cuts again).
    pub fn take_cuts_dirty(&mut self) -> bool {
        let all = std::mem::take(&mut self.cuts_dirty);
        if all {
            // The cuts are taken whole, the single changes before it are in them.
            self.cut_changes.clear();
        }
        all
    }

    /// The ground the `Tree Cut` meshes of the placed landmarks cover, world `(X, Z)`; none while the
    /// models of `assets-b2` are not drawn. A mesh on a bone is taken in the pose of its model.
    // SIMPLIFIED(SIMP-065): the cut is the mesh's ground on the CPU, a bound mesh without its bone.
    pub fn tree_cuts(&self) -> crate::world::forest::batches::TreeCuts {
        if !self.b2_active() {
            return Default::default();
        }
        let mut tris = Vec::new();
        for list in self.placed.values() {
            tris.extend(cut_triangles(&self.models, list));
        }
        crate::world::forest::batches::TreeCuts::new(tris)
    }

    /// What the placed landmarks changed in the cuts since the last call, as the triangles taken out
    /// and the ones put in for each plot that was placed again, in the order of the placings. Empty
    /// after [`Landmark::take_cuts_dirty`] says everything changed.
    pub fn take_cut_changes(&mut self) -> Vec<CutChange> {
        std::mem::take(&mut self.cut_changes)
    }

    /// The animation of the landmarks: a landmark gets a handle when its model
    /// has clips, keeps it while it stands, and is asked for the state of its plot; the ones that
    /// are gone give theirs back.
    pub fn sync_anim(&mut self, anim: &mut AnimService) {
        self.sync_calls = self.sync_calls.wrapping_add(1);
        if !std::mem::take(&mut self.sync_dirty) && !self.sync_calls.is_multiple_of(SYNC_EVERY) {
            return;
        }
        // A mark in the handle says it was seen in this pass, instead of a set of every live key.
        self.anim_epoch = self.anim_epoch.wrapping_add(1);
        let epoch = self.anim_epoch;
        for (key, list) in &self.placed {
            for p in list.iter().filter(|p| !p.pending) {
                let k = (*key, p.slot);
                match self.handles.get_mut(&k) {
                    Some((h, state, seen, model)) if anim.is_live(*h) && *model == p.model => {
                        *seen = epoch;
                        if *state != p.anim {
                            anim.set_state(*h, p.anim);
                            *state = p.anim;
                        }
                    }
                    _ => {
                        // A handle of another model (the plot was built over, or it is a plot of another game).
                        if let Some((old, _, _, _)) = self.handles.remove(&k) {
                            anim.release(old);
                        }
                        let target = AnimTarget::Landmark {
                            cell: *key,
                            slot: p.slot,
                        };
                        if let Some(h) = anim.attach(&p.model, target) {
                            anim.set_state(h, p.anim);
                            self.handles.insert(k, (h, p.anim, epoch, p.model.clone()));
                        }
                    }
                }
            }
        }
        self.handles.retain(|_, (h, _, seen, _)| {
            let live = *seen == epoch;
            if !live {
                anim.release(*h);
            }
            live
        });
    }

    /// The instances of this frame: whole models by model, single parts by (model, part) (WO-D4).
    /// A landmark whose clip is not in the bone texture yet is drawn in the pose of the bind,
    /// never left out.
    ///
    /// The meshes of a model carry the node states they are drawn in (`MeshV2::states`): the
    /// desired-state byte of a landmark is the bit of its state, so a fishing boat (`0x4`) stands on
    /// an improved plot and not on a raw resource, a pillaged one shows its `0x8` parts.
    // re: FGXModelNodeStateSorter::IsInCorrectState; the bits are read from the models of the
    // landmark rows: 0x1 raw resource, 0x2 under construction, 0x4 constructed, 0x8 pillaged.
    pub fn frame_instances(
        &self,
        anim: &AnimService,
    ) -> (
        BTreeMap<LogicalId, Vec<ObjectInstance>>,
        PartBatches,
        draw::StateBatches,
        draw::PartStateBatches,
    ) {
        let mut whole: BTreeMap<LogicalId, Vec<ObjectInstance>> = BTreeMap::new();
        let mut parts: PartBatches = BTreeMap::new();
        let mut states = draw::StateBatches::new();
        let mut part_states = draw::PartStateBatches::new();
        for (key, list) in &self.placed {
            for p in list {
                let bones = match self.handles.get(&(*key, p.slot)) {
                    Some((h, _, _, _)) => anim.bone_ref(*h),
                    None => BoneRef::default(),
                };
                let desired = desired_state(p.anim.landmark);
                // The keys with the model name are made once per landmark, not once per piece.
                let mut part_key: Option<(LogicalId, u16)> = None;
                let mut state_key: Option<(LogicalId, u8)> = None;
                let mut part_state_key: Option<(LogicalId, u16, u8)> = None;
                for piece in &p.pieces {
                    let inst = instance_of(&piece.matrix, bones);
                    match (piece.part, desired == asset_format::STATE_INTACT) {
                        (None, true) => push_to(&mut whole, &p.model, inst),
                        (None, false) => {
                            let k = state_key.get_or_insert_with(|| (p.model.clone(), desired));
                            push_to(&mut states, k, inst);
                        }
                        (Some(part), true) => {
                            let k = part_key.get_or_insert_with(|| (p.model.clone(), part));
                            k.1 = part;
                            push_to(&mut parts, k, inst);
                        }
                        (Some(part), false) => {
                            let k = part_state_key
                                .get_or_insert_with(|| (p.model.clone(), part, desired));
                            k.1 = part;
                            push_to(&mut part_states, k, inst);
                        }
                    }
                }
            }
        }
        // The pieces of the roads and bridges: meshes of their models, no clips.
        let mut route_key: Option<(LogicalId, u16)> = None;
        for (model, mesh, matrix) in self.routes.pieces() {
            let k = match &mut route_key {
                Some(k) if k.0 == *model => k,
                other => other.insert((model.clone(), mesh)),
            };
            k.1 = mesh;
            push_to(&mut parts, k, instance_of(matrix, BoneRef::default()));
        }
        (whole, parts, states, part_states)
    }

    /// Makes the draw of the models of `assets-b2` (once; the formats are those of the main pass).
    pub fn b2_attach(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        colour: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        samples: u32,
    ) {
        match &mut self.b2 {
            Some(b2) => b2.draw.set_targets(colour, depth, samples),
            None => {
                let mut draw = draw::UnitDraw::new(device, queue, colour, depth, samples);
                draw.set_main_slot(MainSlot::Landmark);
                // The `BuildingShader` meshes of the landmarks (the wheat, the animals, the camps)
                // take the ported colour pass of the D3D11 build, as the buildings of the cities:
                // `BuildingPS` of SM3 has no sun (the irradiance cube alone) and reads the textures
                // raw, which made them bright and flat.
                draw.set_building_code(true);
                // Lit by the cube their material names (the wheat is Improvements_Cubelight,
                // about half as bright as the unit's cube and shaded from one side).
                draw.set_material_irradiance(true);
                self.b2 = Some(Box::new(B2State {
                    draw,
                    active: false,
                    shifts: vec![0.0],
                    eye: Vec3::ZERO,
                }));
            }
        }
    }

    /// The landmarks draw through the models of `assets-b2`. The art and the light have to be there;
    /// the renderer says so.
    pub fn set_b2_active(&mut self, on: bool) -> bool {
        let Some(b2) = &mut self.b2 else {
            return false;
        };
        if b2.active == on {
            return false;
        }
        b2.active = on;
        self.place_gen += 1;
        self.sync_dirty = true;
        self.invalidate_all();
        self.cuts_dirty = true;
        self.grounds_all = true;
        true
    }

    pub fn b2_active(&self) -> bool {
        self.b2.as_ref().is_some_and(|b| b.active)
    }

    /// The models of the landmarks that are not on the GPU yet.
    pub fn b2_models_wanted(&self) -> Vec<LogicalId> {
        let Some(b2) = &self.b2 else {
            return Vec::new();
        };
        let mut wanted: BTreeSet<LogicalId> = self
            .placed
            .values()
            .flatten()
            .filter(|p| p.pending)
            .map(|p| p.model.clone())
            .collect();
        wanted.extend(self.routes.models_wanted());
        wanted.retain(|m| !b2.draw.has_model(m));
        wanted.into_iter().collect()
    }

    /// The inputs of the frame for the ported colour pass of the `BuildingShader` meshes
    /// (`BuildingPS_CODE`): call it before [`Self::b2_frame`], which binds them.
    pub fn set_code_frame(&mut self, queue: &wgpu::Queue, f: &draw::CodeFrame<'_>) {
        if let Some(b2) = self.b2.as_mut().filter(|b| b.active) {
            b2.draw.set_code_frame(queue, f);
        }
    }

    pub fn b2_draw_mut(&mut self) -> Option<&mut draw::UnitDraw> {
        self.b2.as_mut().map(|b| &mut b.draw)
    }

    pub fn b2_draw(&self) -> Option<&draw::UnitDraw> {
        self.b2.as_ref().map(|b| &b.draw)
    }

    /// The part of the frame that needs the inputs of the renderer, before the plan: places the
    /// plots that changed, asks the animation for their states, and binds the light and the fog mask
    /// to the draw. The instances themselves are made in `prepare`, after the animation stepped.
    pub fn b2_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        anim: &mut AnimService,
        f: &B2Frame<'_>,
    ) {
        if !self.b2_active() {
            return;
        }
        self.refresh_b2(&B2Context {
            art: f.art,
            owners: f.owners,
            ground: f.ground,
            water_level: f.water_level,
        });
        self.sync_anim(anim);
        let Some(b2) = &mut self.b2 else { return };
        b2.draw
            .bind_light(device, f.lights, f.frame_globals, f.reflection_globals);
        b2.draw.bind_fow(device, queue, f.fow);
        b2.shifts = f.shifts.to_vec();
        b2.eye = f.eye;
    }

    /// The landmarks that reflect, into the reflection pass, for every copy of the map.
    pub fn record_reflection(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        stats: &mut crate::renderer::FrameStats,
    ) {
        if let Some(b2) = self.b2.as_ref().filter(|b| b.active) {
            for &shift in &b2.shifts {
                b2.draw.record_reflection(pass, shift, stats);
            }
        }
    }

    /// The casters of the landmarks into the pass of the shadow map.
    pub fn record_shadow(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        stats: &mut crate::renderer::FrameStats,
    ) {
        if let Some(b2) = self.b2.as_ref().filter(|b| b.active) {
            b2.draw.record_shadow(pass, stats);
        }
    }
}

/// The desired-state byte of a landmark in `state`: the bit its meshes' masks use.
fn desired_state(state: crate::anim::LandmarkState) -> u8 {
    use crate::anim::LandmarkState as S;
    match state {
        S::RawResource => 0x1,
        S::UnderConstruction => 0x2,
        S::Constructed => 0x4,
        S::Pillaged => 0x8,
    }
}

/// The matrix of a piece as the rows of an instance.
pub fn instance_of(m: &Mat4, bones: BoneRef) -> ObjectInstance {
    let row = |i: usize| [m.x_axis[i], m.y_axis[i], m.z_axis[i], m.w_axis[i]];
    ObjectInstance {
        world: [row(0), row(1), row(2)],
        color: [255; 4],
        bones,
    }
}

/// `T(centre) * R_y(yaw) * S(scale)`: B1's `rotate_y`, then the move.
fn base_matrix(centre: Vec2, yaw: f32, scale: f32) -> Mat4 {
    Mat4::from_translation(Vec3::new(centre.x, 0.0, centre.y))
        * Mat4::from_rotation_y(yaw)
        * Mat4::from_scale(Vec3::splat(scale))
}

/// The matrix with its origin put on the surface: the height of the ground (or the water) at the
/// point the origin stands on.
fn on_surface(mut m: Mat4, surface: &dyn Fn(Vec2) -> f32) -> Mat4 {
    m.w_axis.y += surface(Vec2::new(m.w_axis.x, m.w_axis.z));
    m
}

/// The matrix with its origin seated on the surface: the height REPLACES the one the matrix has (the
/// piece's authored placement height is dropped), as the original does for every element.
// re: FUN_0887ee5a 0x0887ee5a (RE-B12 item 1b): `M._43 = max(GetWorldHeight(M._41, M._42), field_4)`, an
// assignment; the vertex shaders add nothing, so the piece is a rigid body on its origin cell
// SIMPLIFIED(SIMP-exe-1070): the river test that hides a piece (bTerrainContour) is not ported
fn seated_on_surface(mut m: Mat4, surface: &dyn Fn(Vec2) -> f32) -> Mat4 {
    m.w_axis.y = surface(Vec2::new(m.w_axis.x, m.w_axis.z));
    m
}

/// The pieces of a natural wonder in `state`: all of them, each at its authored place, not turned.
// re: the library record of a wonder has one piece per model of the gr2, each with its
// `InitialPlacement`; `cvLandmarkSim` gives no turn (RE-B12 item 1, section 2)
fn wonder_arrangement(
    parts: &[PartInfo<'_>],
    state: crate::anim::LandmarkState,
) -> layout::Arrangement {
    layout::Arrangement {
        yaw: 0.0,
        parts: Some(
            parts
                .iter()
                .filter(|p| layout::part_in_state(p.name, state))
                .map(|p| p.index)
                .collect(),
        ),
        authored: true,
    }
}

/// The landmarks of one plot, placed.
fn place_plot(
    models: &BTreeMap<LogicalId, ModelMeta>,
    techs: &[String],
    ctx: &B2Context<'_>,
    heading: &heading::HeadingField,
    key: PlotKey,
    plot: &PlotDecor,
    seen: bool,
) -> Vec<Placed> {
    let player = plot.owner.and_then(|o| ctx.owners.player(o));
    let era = LandmarkEra::of(player.is_some_and(|p| p.industrial));
    let art_techs = player.map_or(0, |p| p.art_techs);
    let has = |name: &str| {
        techs
            .iter()
            .position(|t| t == name)
            .is_some_and(|i| i < 64 && art_techs & (1 << i) != 0)
    };
    let owner = Owner { era, techs: &has };
    let wanted = select::plan_plot(ctx.art, plot, &owner, key);
    // The scale of the improvement's row: the model whose bone carries the pillaged smoke.
    let host_scale = wanted
        .iter()
        .find(|w| w.kind == select::Kind::Improvement)
        .map_or(1.0, |w| snapshot::row_scale(w.row.scale));
    let centre = cell_center(plot.x, plot.y);
    let surface = |p: Vec2| {
        let g = (ctx.ground)(p);
        ctx.water_level.map_or(g, |w| g.max(w))
    };
    let mut out = Vec::new();
    for (slot, w) in wanted.into_iter().enumerate() {
        let Some(model) = w.row.model.clone() else {
            // The row draws nothing on purpose.
            continue;
        };
        let anim = anim_state(w.state, plot.worked);
        // re: cvFarmSimSystem::CreateFarmDecal 0x08a1d4ac (the fields of every state: half built while
        // the farm is built, the damage over them when it is pillaged)
        let farm_state = match w.state {
            crate::anim::LandmarkState::Constructed => Some(decals::FarmState::Constructed),
            crate::anim::LandmarkState::UnderConstruction => {
                Some(decals::FarmState::UnderConstruction)
            }
            crate::anim::LandmarkState::Pillaged => Some(decals::FarmState::Pillaged),
            crate::anim::LandmarkState::RawResource => None,
        };
        let farm = farm_state.filter(|_| is_farm(w.row)).map(|state| Farm {
            x: plot.x,
            y: plot.y,
            state,
            wheat: plot.resource.as_deref() == Some(decals::WHEAT),
        });
        let Some(meta) = models.get(&model) else {
            out.push(Placed {
                slot: slot as u16,
                model,
                pieces: Vec::new(),
                anim,
                pending: true,
                farm,
            });
            continue;
        };
        let handler = Layout::parse(&w.row.layout).unwrap_or_else(|| {
            tracing::warn!(
                "landmark layout `{}` is not known; the model stands whole",
                w.row.layout
            );
            Layout::Snapshot
        });
        // A natural wonder whose pieces the set keeps apart: every piece at its authored place, seated
        // on the ground at its own origin.
        let wonder = w.kind == select::Kind::Feature
            && w.row.is_natural_wonder()
            && handler == Layout::Snapshot
            && meta.parts.len() > 1;
        let arrangement = if wonder {
            wonder_arrangement(&meta.infos(), w.state)
        } else {
            layout::arrange(handler, key, &meta.infos(), w.state)
        };
        let scale = snapshot::row_scale(w.row.scale);
        let base = if handler == Layout::Farm {
            // The farm model stands on a field near the centre, turned with the crops.
            let (anchor, yaw) = decals::farm_anchor(plot.x, plot.y, heading);
            base_matrix(anchor, yaw, scale)
        } else {
            base_matrix(centre, arrangement.yaw, scale)
        };
        let seat = |m: Mat4| {
            if wonder {
                seated_on_surface(m, &surface)
            } else {
                on_surface(m, &surface)
            }
        };
        let pieces = match &arrangement.parts {
            None => vec![Piece {
                part: None,
                matrix: seat(base),
            }],
            Some(parts) => parts
                .iter()
                .map(|&p| {
                    let m = match meta.parts.get(usize::from(p)) {
                        Some(pm) if arrangement.authored => base * pm.placement,
                        _ => base,
                    };
                    Piece {
                        part: Some(p),
                        matrix: seat(m),
                    }
                })
                .collect(),
        };
        out.push(Placed {
            slot: slot as u16,
            model,
            pieces,
            anim,
            pending: false,
            farm,
        });
    }
    // The smoke of a pillaged improvement, on a visible plot only.
    if seen && let Some(s) = smoke::smoke_of(plot) {
        let model = LogicalId::new(s.model);
        let anim = anim_state(crate::anim::LandmarkState::Constructed, false);
        out.push(match models.get(&model) {
            None => Placed {
                slot: smoke::SMOKE_SLOT,
                model,
                pieces: Vec::new(),
                anim,
                pending: true,
                farm: None,
            },
            Some(_) => Placed {
                slot: smoke::SMOKE_SLOT,
                model,
                pieces: vec![Piece {
                    part: None,
                    matrix: on_surface(smoke::matrix(centre, host_scale, &s), &surface),
                }],
                anim,
                pending: false,
                farm: None,
            },
        });
    }
    out
}

fn is_farm(row: &LandmarkArt) -> bool {
    Layout::parse(&row.layout) == Some(Layout::Farm)
}

impl RenderSystem for Landmark {
    fn id(&self) -> SystemId {
        SystemId::Landmark
    }

    fn passes(&self) -> &'static [PassSlot] {
        &[
            PassSlot::Pass(PassId::ShadowMap),
            PassSlot::Pass(PassId::Reflection),
            PassSlot::Main(MainSlot::Landmark),
        ]
    }

    fn init(&mut self, _cx: &mut InitContext<'_>) {}

    /// WOB4: the instances of the frame, after the animation stepped (`PrepareServices`).
    fn prepare(&mut self, cx: &mut PrepareContext<'_>) {
        let Some((shifts, eye)) = self
            .b2
            .as_ref()
            .filter(|b| b.active)
            .map(|b| (b.shifts.clone(), b.eye))
        else {
            return;
        };
        let services = cx.services();
        let anim = &services.anim;
        let bones: Vec<BoneRef> = self
            .handles
            .values()
            .map(|(h, _, _, _)| anim.bone_ref(*h))
            .collect();
        let cached = self
            .inst_cache
            .take()
            .filter(|c| c.0 == self.place_gen && c.1 == bones);
        if cached.is_some() {
            self.instances_reused += 1;
        }
        let cache = match cached {
            Some(c) => c,
            None => (self.place_gen, bones, self.frame_instances(anim)),
        };
        let (batches, parts, states, part_states) = &cache.2;
        let animated = |id: &LogicalId| anim.model(id).is_some_and(|m| m.animated());
        let Some(b2) = &mut self.b2 else { return };
        let report = b2.draw.build(
            cx.device,
            cx.queue,
            services.bones(),
            &draw::BuildInputs {
                batches,
                parts,
                states,
                part_states,
                shifts: &shifts,
                eye,
                animated: &animated,
                remember: true,
                models_revision: anim.models_revision(),
            },
        );
        self.counters.instances = report.instances;
        self.counters.batches = report.calls;
        self.inst_cache = Some(cache);
    }

    fn record_mask(&self, slot: PassSlot, cx: &mut MaskContext<'_, '_>) {
        if let Some(b2) = self.b2.as_ref().filter(|b| b.active) {
            b2.draw.record_mask(slot, cx.pass, cx.copy_shift);
        }
    }

    fn record(&self, slot: PassSlot, cx: &mut DrawContext<'_, '_>) {
        if let Some(b2) = self.b2.as_ref().filter(|b| b.active) {
            b2.draw.record(slot, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use asset_format::{ArtTable, Part, Transform};

    use super::*;
    use crate::entities::{EntityPlayer, Owners};

    fn road(x: i32, y: i32) -> PlotDecor {
        PlotDecor {
            x,
            y,
            route: Some(crate::entities::RouteKind::Road),
            ..PlotDecor::default()
        }
    }

    #[test]
    fn plots_are_kept_by_key_in_row_major_order() {
        let mut l = Landmark::default();
        let keys = [plot_key(3, 1), plot_key(0, 0), plot_key(2, 1)];
        assert!(l.set_plots(&[
            (keys[0], Some(road(3, 1))),
            (keys[1], Some(road(0, 0))),
            (keys[2], Some(road(2, 1))),
        ]));
        let order: Vec<(i32, i32)> = l.plots().map(|p| (p.x, p.y)).collect();
        assert_eq!(order, [(0, 0), (2, 1), (3, 1)]);
        // The same plot again is no change; a cleared one is.
        assert!(!l.set_plots(&[(keys[0], Some(road(3, 1)))]));
        assert!(l.set_plots(&[(keys[0], None)]));
        assert!(!l.set_plots(&[(keys[0], None)]));
        assert_eq!(l.len(), 2);
        assert_eq!(l.counters().records, 2);
    }

    // ---- WOB4 on a synthetic table ----

    const FARM: &str = "ART_DEF_IMPROVEMENT_FARM";
    const COW: &str = "ART_DEF_RESOURCE_COW";
    const MINE: &str = "ART_DEF_IMPROVEMENT_MINE";

    fn row(imp: &str, res: &str, layout: &str, scale: f32, model: &str) -> LandmarkArt {
        LandmarkArt {
            improvement: imp.into(),
            resource: res.into(),
            era: "Any".into(),
            state: "Any".into(),
            layout: layout.into(),
            scale,
            terrain_contour: 1,
            tech: None,
            model: Some(LogicalId::new(model)),
        }
    }

    fn table() -> ArtTable {
        let mut t = ArtTable::default();
        t.insert_landmark(
            0,
            row(FARM, "ART_DEF_RESOURCE_NONE", "FARM", 1.0, "md:farm"),
        );
        t.insert_landmark(
            0,
            row(MINE, "ART_DEF_RESOURCE_NONE", "SNAPSHOT", 0.5, "md:mine"),
        );
        t.insert_landmark(
            0,
            row(
                "ART_DEF_IMPROVEMENT_PASTURE",
                COW,
                "ANIMATED",
                0.07,
                "md:cow",
            ),
        );
        t
    }

    fn part(name: &str, at: [f32; 3]) -> Part {
        Part {
            name: name.into(),
            first_mesh: 0,
            mesh_count: 1,
            placement: Transform {
                translation: at,
                ..Transform::IDENTITY
            },
        }
    }

    fn blob(parts: Vec<Part>) -> ModelBlobV2 {
        ModelBlobV2 {
            parts,
            ..ModelBlobV2::default()
        }
    }

    fn landmark_with_models() -> Landmark {
        let mut l = Landmark::default();
        l.register_model(
            LogicalId::new("md:farm"),
            &blob(vec![
                part("model0", [-5.0, 0.0, 0.0]),
                part("model1", [5.0, 0.0, 0.0]),
            ]),
        );
        l.register_model(
            LogicalId::new("md:mine"),
            &blob(vec![part("Scene", [0.0; 3])]),
        );
        l.register_model(
            LogicalId::new("md:cow"),
            &blob(vec![
                part("Dummy_CowLocation", [0.0; 3]),
                part("Pasture_Pillaged", [0.0; 3]),
                part("Pasture", [0.0; 3]),
            ]),
        );
        l
    }

    fn refresh(l: &mut Landmark, art: &ArtTable, owners: &Owners) -> usize {
        let ground = |p: Vec2| p.x * 0.01 + 10.0;
        l.refresh_b2(&B2Context {
            art,
            owners,
            ground: &ground,
            water_level: Some(2.0),
        })
    }

    fn farm_plot(x: i32, y: i32) -> PlotDecor {
        PlotDecor {
            x,
            y,
            improvement: Some(FARM.into()),
            ..PlotDecor::default()
        }
    }

    #[test]
    fn a_pillaged_farm_smokes_on_a_visible_plot_with_the_farm_smoke_model() {
        let art = table();
        let mut l = landmark_with_models();
        let mut plot = farm_plot(3, 2);
        plot.pillaged = true;
        assert!(l.set_plot(plot));
        let key = plot_key(3, 2);
        let smoke_of = |l: &Landmark| {
            l.placed_at(&key)
                .iter()
                .find(|p| p.slot == smoke::SMOKE_SLOT)
                .cloned()
        };
        // Not visible: no smoke.
        refresh(&mut l, &art, &Owners::default());
        assert!(smoke_of(&l).is_none());
        // Visible, the model not here: pending, and wanted by the renderer's list.
        l.set_seen(|x, y| (x, y) == (3, 2));
        refresh(&mut l, &art, &Owners::default());
        let s = smoke_of(&l).expect("smoke");
        assert!(s.pending);
        assert_eq!(s.model, LogicalId::new(smoke::FARM_SMOKE_MODEL));
        // The model comes: placed over the middle of the plot, on the ground.
        l.register_model(s.model.clone(), &blob(vec![part("Scene", [0.0; 3])]));
        refresh(&mut l, &art, &Owners::default());
        let s = smoke_of(&l).expect("smoke");
        assert!(!s.pending && s.pieces.len() == 1 && s.pieces[0].part.is_none());
        let c = cell_center(3, 2);
        let o = s.pieces[0].matrix.transform_point3(Vec3::ZERO);
        assert!((o.x - c.x).abs() < 1e-3 && (o.z - (c.y + 0.4)).abs() < 1e-3);
        assert!(o.y > 10.0, "on the ground");
        // The farm itself still stands in its parts beside it.
        assert!(l.placed_at(&key).iter().any(|p| p.slot == 0));
        // The fog comes back: the smoke goes; a farm that is whole has none.
        l.set_seen(|_, _| false);
        refresh(&mut l, &art, &Owners::default());
        assert!(smoke_of(&l).is_none());
        l.set_plot(farm_plot(3, 2));
        l.set_seen(|_, _| true);
        refresh(&mut l, &art, &Owners::default());
        assert!(smoke_of(&l).is_none());
    }

    #[test]
    fn a_farm_stands_in_parts_at_their_authored_places_on_the_ground() {
        let art = table();
        let mut l = landmark_with_models();
        assert!(l.set_plot(farm_plot(3, 2)));
        assert_eq!(refresh(&mut l, &art, &Owners::default()), 1);
        let placed = l.placed_at(&plot_key(3, 2));
        assert_eq!(placed.len(), 1);
        let p = &placed[0];
        assert_eq!(p.model, LogicalId::new("md:farm"));
        assert!(!p.pending);
        let parts: Vec<Option<u16>> = p.pieces.iter().map(|p| p.part).collect();
        assert_eq!(parts, [Some(0), Some(1)]);
        // The group stands on the field nearest the centre, turned by the heading there.
        let (anchor, yaw) = decals::farm_anchor(3, 2, &heading::HeadingField::default());
        assert!(anchor.distance(cell_center(3, 2)) < decals::FIELD_SIDE);
        for (piece, dx) in p.pieces.iter().zip([-5.0f32, 5.0]) {
            let o = piece.matrix.transform_point3(Vec3::ZERO);
            let want = anchor + Vec2::new(yaw.cos(), -yaw.sin()) * dx;
            assert!((o.x - want.x).abs() < 1e-3, "{o} vs {want}");
            assert!((o.z - want.y).abs() < 1e-3, "{o} vs {want}");
            assert!((o.y - (o.x * 0.01 + 10.0)).abs() < 1e-3, "on the ground");
        }
        // A farm that is built has fields as decals; the plot is a farm of the owner's era.
        assert_eq!(
            p.farm,
            Some(Farm {
                x: 3,
                y: 2,
                state: decals::FarmState::Constructed,
                wheat: false,
            })
        );
        let (farms, touched) = l.farm_update().unwrap();
        assert_eq!(farms.len(), 1);
        assert!(touched.contains(&(3, 2)));
        l.farms_applied(&farms, &touched);
        assert!(l.farm_update().is_none());
        // The farm is gone: its plot is still given to the layer, once.
        l.set_plots(&[(plot_key(3, 2), None)]);
        refresh(&mut l, &art, &Owners::default());
        let (farms, touched) = l.farm_update().unwrap();
        assert!(farms.is_empty());
        assert!(touched.contains(&(3, 2)));
    }

    #[test]
    fn a_snapshot_is_one_whole_piece_at_the_scale_of_the_row_and_a_sea_plot_floats() {
        let art = table();
        let mut l = landmark_with_models();
        l.set_plot(PlotDecor {
            x: 1,
            y: 1,
            improvement: Some(MINE.into()),
            ..PlotDecor::default()
        });
        refresh(&mut l, &art, &Owners::default());
        let p = &l.placed_at(&plot_key(1, 1))[0];
        assert_eq!(p.pieces.len(), 1);
        assert_eq!(p.pieces[0].part, None);
        let m = p.pieces[0].matrix;
        assert!(
            (m.transform_point3(Vec3::X).x - m.transform_point3(Vec3::ZERO).x - 0.5).abs() < 1e-4
        );
        // The ground there is below the water: the model stands on the water.
        let low = |_: Vec2| -50.0;
        l.invalidate_all();
        l.refresh_b2(&B2Context {
            art: &art,
            owners: &Owners::default(),
            ground: &low,
            water_level: Some(2.0),
        });
        let y = l.placed_at(&plot_key(1, 1))[0].pieces[0].matrix.w_axis.y;
        assert!((y - 2.0).abs() < 1e-4, "{y}");
    }

    const DORADO: &str = "ART_DEF_FEATURE_EL_DORADO";

    fn dorado_plot() -> PlotDecor {
        PlotDecor {
            x: 2,
            y: 2,
            feature: Some(DORADO.into()),
            ..PlotDecor::default()
        }
    }

    fn dorado_table() -> ArtTable {
        let mut t = ArtTable::default();
        t.insert_landmark(
            0,
            row(
                "ART_DEF_IMPROVEMENT_NONE",
                DORADO,
                "SNAPSHOT",
                0.9,
                "md:dorado",
            ),
        );
        t
    }

    #[test]
    fn a_natural_wonder_of_pieces_sits_on_the_ground_at_each_origin_and_drops_the_authored_height()
    {
        let art = dorado_table();
        let mut l = Landmark::default();
        // Two pieces; the first one is authored 1.7 units over the ground (El Dorado: z 1.93 * 0.9).
        l.register_model(
            LogicalId::new("md:dorado"),
            &blob(vec![
                part("model0", [-10.0, 1.9, 0.0]),
                part("model1", [30.0, 0.0, 0.0]),
            ]),
        );
        assert!(l.set_plot(dorado_plot()));
        refresh(&mut l, &art, &Owners::default());
        let placed = l.placed_at(&plot_key(2, 2));
        assert_eq!(placed.len(), 1);
        let pieces = &placed[0].pieces;
        assert_eq!(pieces.len(), 2);
        let c = cell_center(2, 2);
        for (piece, dx) in pieces.iter().zip([-10.0 * 0.9, 30.0 * 0.9]) {
            let o = piece.matrix.transform_point3(Vec3::ZERO);
            assert!((o.x - (c.x + dx)).abs() < 1e-3, "the piece keeps its place");
            // The height is the ground at the piece's own origin: not the authored 1.9, not the
            // ground at the middle of the plot.
            assert!((o.y - ((c.x + dx) * 0.01 + 10.0)).abs() < 1e-3, "{}", o.y);
        }
    }

    #[test]
    fn a_natural_wonder_in_the_water_is_seated_on_the_water() {
        let art = dorado_table();
        let mut l = Landmark::default();
        l.register_model(
            LogicalId::new("md:dorado"),
            &blob(vec![
                part("model0", [0.0, 3.0, 0.0]),
                part("model1", [5.0, 0.0, 0.0]),
            ]),
        );
        l.set_plot(dorado_plot());
        let low = |_: Vec2| -50.0;
        l.refresh_b2(&B2Context {
            art: &art,
            owners: &Owners::default(),
            ground: &low,
            water_level: Some(2.0),
        });
        for piece in &l.placed_at(&plot_key(2, 2))[0].pieces {
            assert!((piece.matrix.w_axis.y - 2.0).abs() < 1e-4);
        }
    }

    #[test]
    fn a_wonder_of_a_set_that_merged_it_stays_whole() {
        // The model of a set built before the pieces were kept apart: one part, the placement in the
        // vertices; it stands whole as before.
        let art = dorado_table();
        let mut l = Landmark::default();
        l.register_model(
            LogicalId::new("md:dorado"),
            &blob(vec![part("x", [0.0; 3])]),
        );
        l.set_plot(dorado_plot());
        refresh(&mut l, &art, &Owners::default());
        let p = &l.placed_at(&plot_key(2, 2))[0];
        assert_eq!(p.pieces.len(), 1);
        assert_eq!(p.pieces[0].part, None);
    }

    #[test]
    fn a_herd_shows_the_parts_of_its_state_and_the_animation_is_asked_for_it() {
        let art = table();
        let mut l = landmark_with_models();
        let cow = |pillaged: bool| PlotDecor {
            x: 4,
            y: 4,
            resource: Some(COW.into()),
            improvement: Some("ART_DEF_IMPROVEMENT_PASTURE".into()),
            pillaged,
            ..PlotDecor::default()
        };
        l.set_plot(cow(false));
        refresh(&mut l, &art, &Owners::default());
        let p = &l.placed_at(&plot_key(4, 4))[0];
        // Not pillaged: the pillaged part is not drawn, so the herd is not one whole piece.
        let parts: Vec<Option<u16>> = p.pieces.iter().map(|p| p.part).collect();
        assert_eq!(parts, [Some(0), Some(2)]);
        assert_eq!(p.anim.landmark, crate::anim::LandmarkState::Constructed);
        l.set_plot(cow(true));
        refresh(&mut l, &art, &Owners::default());
        let p = &l.placed_at(&plot_key(4, 4))[0];
        let parts: Vec<Option<u16>> = p.pieces.iter().map(|p| p.part).collect();
        assert_eq!(parts, [Some(0), Some(1)]);
        assert_eq!(p.anim.landmark, crate::anim::LandmarkState::Pillaged);
    }

    #[test]
    fn a_model_that_has_not_come_is_wanted_and_the_plot_is_placed_when_it_does() {
        let art = table();
        let mut l = Landmark::default();
        l.set_plot(farm_plot(0, 0));
        refresh(&mut l, &art, &Owners::default());
        let placed = l.placed_at(&plot_key(0, 0));
        assert!(placed[0].pending && placed[0].pieces.is_empty());
        assert!(l.wants_model(&LogicalId::new("md:farm")));
        assert_eq!(
            refresh(&mut l, &art, &Owners::default()),
            0,
            "nothing changed"
        );
        l.register_model(
            LogicalId::new("md:farm"),
            &blob(vec![part("model0", [0.0; 3])]),
        );
        assert_eq!(refresh(&mut l, &art, &Owners::default()), 1);
        assert!(!l.placed_at(&plot_key(0, 0))[0].pending);
        assert!(!l.wants_model(&LogicalId::new("md:farm")));
    }

    #[test]
    fn the_era_and_the_technologies_of_the_owner_reach_the_choice() {
        let mut t = ArtTable::default();
        let polder = "ART_DEF_IMPROVEMENT_POLDER";
        let none = "ART_DEF_RESOURCE_ALL";
        let mut anc = row(polder, none, "ANIMATED", 1.0, "md:anc");
        anc.era = "Ancient".into();
        let mut econ = row(polder, none, "ANIMATED", 1.0, "md:econ");
        econ.era = "Ancient".into();
        econ.tech = Some("TECH_ECONOMICS".into());
        let mut ind = row(polder, none, "ANIMATED", 1.0, "md:ind");
        ind.era = "Industrial".into();
        t.insert_landmark(0, anc);
        t.insert_landmark(1, econ);
        t.insert_landmark(2, ind);
        let mut l = Landmark::default();
        l.set_art_techs(vec!["TECH_ECONOMICS".into()]);
        let plot = |owner: Option<i8>| PlotDecor {
            x: 0,
            y: 0,
            improvement: Some(polder.into()),
            owner,
            ..PlotDecor::default()
        };
        let player = |id, industrial, art_techs| EntityPlayer {
            id,
            color: [0; 3],
            team_color: [0; 3],
            culture: None,
            era: None,
            industrial,
            art_techs,
            era_index: None,
        };
        let owners = Owners {
            players: vec![player(0, false, 0), player(1, false, 1), player(2, true, 0)],
            ..Owners::default()
        };
        let model_of = |l: &mut Landmark, owner| {
            l.set_plot(plot(owner));
            refresh(l, &t, &owners);
            l.placed_at(&plot_key(0, 0))[0].model.as_str().to_owned()
        };
        assert_eq!(model_of(&mut l, None), "md:anc");
        assert_eq!(model_of(&mut l, Some(0)), "md:anc");
        assert_eq!(model_of(&mut l, Some(1)), "md:econ");
        assert_eq!(model_of(&mut l, Some(2)), "md:ind");
    }

    // ---- the real pack: one landmark for each row of the table (AC-R3.4, world-objects 9) ----

    /// The art table and the parts of every model of a pack directory (`CIV5_ASSETS_B2`, the output
    /// of `civ5-assets b2-*`); `None` without it.
    fn real_pack() -> Option<(ArtTable, BTreeMap<LogicalId, ModelMeta>)> {
        let dir = std::env::var_os("CIV5_ASSETS_B2")?;
        let mut art = None;
        let mut models = BTreeMap::new();
        for e in std::fs::read_dir(dir).ok()?.flatten() {
            let Ok(bytes) = std::fs::read(e.path()) else {
                continue;
            };
            match bytes.get(..4) {
                Some(b"C5AT") => {
                    if let Ok(t) = ArtTable::decode(&bytes)
                        && t.landmarks().next().is_some()
                    {
                        art = Some(t);
                    }
                }
                Some(b"C5MD") => {
                    if let Ok(m) = ModelBlobV2::decode(&bytes) {
                        models.insert(
                            LogicalId::new(format!("md:{}", m.source.to_ascii_lowercase())),
                            ModelMeta::of(&m),
                        );
                    }
                }
                _ => {}
            }
        }
        Some((art?, models))
    }

    /// A plot on which the row `r` is the choice, and whether its owner is of the Industrial era.
    fn plot_of_row(r: &LandmarkArt) -> (PlotDecor, bool) {
        let mut p = PlotDecor {
            x: 5,
            y: 5,
            owner: Some(0),
            ..PlotDecor::default()
        };
        let none = r.improvement == select::IMPROVEMENT_NONE;
        let generic = r.resource == select::RESOURCE_ALL || r.resource == select::RESOURCE_NONE;
        if r.resource.starts_with("ART_DEF_FEATURE_") && none {
            p.feature = Some(r.resource.clone());
        } else if r.resource == select::RESOURCE_ALL {
            // `ALL` is the row of a resource that has none of its own.
            p.resource = Some("ART_DEF_RESOURCE_WITHOUT_A_ROW".to_owned());
        } else if !generic {
            p.resource = Some(r.resource.clone());
        }
        if !none {
            match r.state.as_str() {
                "UnderConstruction" => p.building = Some(r.improvement.clone()),
                "Pillaged" => {
                    p.improvement = Some(r.improvement.clone());
                    p.pillaged = true;
                }
                _ => p.improvement = Some(r.improvement.clone()),
            }
        }
        (p, r.era == "Industrial")
    }

    /// Every row of `ArtDefine_Landmarks` is the choice of some plot, and what it chooses has a
    /// model in the pack and stands somewhere. A row with no `Model` draws nothing by design.
    #[test]
    fn every_row_of_the_real_table_has_a_model_and_a_place() {
        let Some((art, models)) = real_pack() else {
            eprintln!("skipped: CIV5_ASSETS_B2 is not set");
            return;
        };
        let techs = vec!["TECH_ECONOMICS".to_owned()];
        let mut problems = Vec::new();
        let mut rows = 0;
        for r in art.landmarks() {
            rows += 1;
            if r.model.is_none() {
                // The row draws nothing, and an exact row beats it.
                continue;
            }
            let (plot, industrial) = plot_of_row(r);
            let reached = [0u64, 1].into_iter().any(|tech_bits| {
                let owners = Owners {
                    players: vec![EntityPlayer {
                        id: 0,
                        color: [0; 3],
                        team_color: [0; 3],
                        culture: None,
                        era: None,
                        industrial,
                        art_techs: tech_bits,
                        era_index: None,
                    }],
                    ..Owners::default()
                };
                let has = |t: &str| t == "TECH_ECONOMICS" && tech_bits == 1;
                let owner = Owner {
                    era: LandmarkEra::of(industrial),
                    techs: &has,
                };
                let hit = (0..200).any(|cell| {
                    select::plan_plot(&art, &plot, &owner, cell)
                        .iter()
                        .any(|w| std::ptr::eq(w.row, r))
                });
                // And the plot is placed: every landmark it chose has pieces.
                let ground = |_: Vec2| 0.0;
                let ctx = B2Context {
                    art: &art,
                    owners: &owners,
                    ground: &ground,
                    water_level: None,
                };
                for key in 0..20u32 {
                    let heading = heading::HeadingField::default();
                    for placed in place_plot(&models, &techs, &ctx, &heading, key, &plot, false) {
                        if placed.pending {
                            problems.push(format!("no blob for {}", placed.model.as_str()));
                        } else if placed.pieces.is_empty() {
                            problems.push(format!("no piece for {}", placed.model.as_str()));
                        }
                    }
                }
                hit
            });
            if !reached {
                problems.push(format!(
                    "row {}/{}/{}/{}/{} is the choice of no plot",
                    r.improvement, r.resource, r.era, r.state, r.layout
                ));
            }
        }
        problems.sort();
        problems.dedup();
        assert!(rows >= 600, "{rows} rows");
        assert!(
            problems.is_empty(),
            "{} problems:\n{}",
            problems.len(),
            problems.join("\n")
        );
    }
    // ---- the draw on a GPU (no adapter: the test returns) ----

    mod gpu_draw {
        use asset_format::{
            MeshArrays, ParamSet, TexDim, TexKind, TexParam, TexSource, TextureFormat,
        };
        use draw_list::UiDrawList;

        use super::*;
        use crate::entities::EntitySnapshot;
        use crate::game_camera::GameCamera;
        use crate::gpu_test::{gpu, read_texture};
        use crate::lighting::tests::{boot_blobs_with, settings};
        use crate::material::MaterialDesc;
        use crate::renderer::Renderer;
        use crate::scene::Scene;
        use crate::terrain::{Fog, TerrainCell, TerrainMesh, TerrainSnapshot};
        use crate::world::terrain::TerrainBasis;

        /// A rigid quad of 40 x 40 standing on the ground, drawn with `LandmarkShader`: the texture
        /// is the colour on the screen. `ground`: a `LandmarkShader` mesh is a decal of the terrain
        /// instead (the original's decal container).
        fn mine_model(texture: &LogicalId) -> ModelBlobV2 {
            let a = MeshArrays {
                positions: vec![
                    [-20.0, 0.0, 0.0],
                    [20.0, 0.0, 0.0],
                    [20.0, 40.0, 0.0],
                    [-20.0, 40.0, 0.0],
                ],
                normals: vec![[0.0, 0.0, 1.0]; 4],
                uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                joints: vec![],
                weights: vec![],
                indices: vec![0, 1, 2, 0, 2, 3],
            };
            ModelBlobV2 {
                parts: vec![part("Scene", [0.0; 3])],
                meshes: vec![asset_format::MeshV2::pack("quad", 0, 0, &a, false, false).unwrap()],
                materials: vec![MaterialDesc {
                    package: "LandmarkShader".into(),
                    version: 4,
                    sets: vec![ParamSet {
                        blocks: vec![],
                        textures: vec![TexParam {
                            name: "BaseTextureMap".into(),
                            kind: TexKind::Tex2D,
                            src: TexSource::File(texture.clone()),
                            semantic: 0,
                            reg: 0,
                            usage: 0,
                            flags: 0,
                            active: true,
                        }],
                        constants: vec![],
                    }],
                    ..MaterialDesc::default()
                }],
                ..ModelBlobV2::default()
            }
        }

        fn frames(
            g: &crate::gpu_test::Gpu,
            r: &mut Renderer,
            size: (u32, u32),
            n: usize,
        ) -> Vec<u8> {
            let mut camera = GameCamera::new(cell_center(3, 2));
            camera.set_level(10.0);
            let target = g.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("landmark target"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = target.create_view(&wgpu::TextureViewDescriptor::default());
            let mut last = Vec::new();
            for i in 0..n {
                let commands = r.encode_frame(
                    &view,
                    &Scene::test(i as f32 / 30.0),
                    &camera,
                    &UiDrawList::default(),
                    None,
                );
                g.queue.submit([commands]);
                r.frame_submitted();
                last = read_texture(g, &target, 0, 0, size, 4);
            }
            last
        }

        /// WOB4: the mine of a plot is asked for and comes; its `LandmarkShader` quad is laid into
        /// the layer of decals of the terrain with the texture of its material, not drawn by the
        /// landmark draw (the original's decal container).
        #[test]
        fn a_landmark_of_assets_b2_lays_its_ground_mesh_into_the_terrain() {
            let Some(gpu) = gpu(
                "a_landmark_of_assets_b2_lays_its_ground_mesh_into_the_terrain",
                wgpu::Backends::PRIMARY,
                wgpu::Features::empty(),
            ) else {
                return;
            };
            let size = (96u32, 96u32);
            let mut r = Renderer::offscreen(&gpu, size, wgpu::TextureFormat::Rgba8Unorm, 1);
            r.set_boot_required(true);
            for (file, b) in boot_blobs_with(&[], &|_, _| [200, 200, 200, 255]) {
                r.add_lighting_texture(&file, &b).unwrap();
            }
            r.set_world_settings(settings());
            let cell = TerrainCell {
                fog: Fog::Visible,
                ..TerrainCell::default()
            };
            let snapshot = TerrainSnapshot::new(6, 4, false, vec![cell; 24]).unwrap();
            r.set_terrain(&snapshot, &TerrainMesh::build(&snapshot));
            r.set_terrain_basis(TerrainBasis::from_snapshot(&snapshot, 1).unwrap());
            r.set_unit_art_b2(table());
            r.set_entities(EntitySnapshot {
                players: vec![],
                units: vec![],
                cities: vec![],
                era: Default::default(),
                plots: vec![PlotDecor {
                    x: 3,
                    y: 2,
                    improvement: Some(MINE.into()),
                    ..PlotDecor::default()
                }],
            });
            frames(&gpu, &mut r, size, 1);
            assert!(
                r.world().landmark.b2_active(),
                "the light and the art are in"
            );
            assert_eq!(r.unit_models_b2_wanted(), [LogicalId::new("md:mine")]);
            let tex = LogicalId::new("tx:mine");
            let textures = [(
                tex.clone(),
                asset_format::TextureBlob {
                    format: TextureFormat::Rgba8,
                    dim: TexDim::D2,
                    srgb: true,
                    width: 1,
                    height: 1,
                    depth: 1,
                    mips: vec![vec![0, 255, 0, 255]],
                },
            )]
            .into();
            r.add_unit_model_b2(
                LogicalId::new("md:mine"),
                &mine_model(&tex),
                None,
                None,
                &textures,
            );
            assert!(r.unit_models_b2_wanted().is_empty());
            frames(&gpu, &mut r, size, 4);
            let draw = r.world().landmark.b2_draw().unwrap();
            // The `LandmarkShader` mesh is not drawn as a model: it is a decal of the terrain with
            // the texture of its material, on the hex of the plot (`ground`).
            assert_eq!(draw.call_counts().0, 0, "{:?}", draw.report());
            assert!(draw.report().instances >= 1, "{:?}", draw.report());
            let layer = r.terrain_decals_mut();
            assert_eq!(layer.len(), 1);
            let d = layer.iter().next().unwrap();
            assert_eq!(d.albedo.as_ref(), Some(&tex));
            let c = crate::hexgrid::cell_center(3, 2);
            assert!((d.min + d.max - c * 2.0).abs().max_element() < 1.0, "{d:?}");
        }
    }
    #[test]
    fn the_matrix_becomes_the_rows_of_an_instance() {
        let m = base_matrix(Vec2::new(10.0, -5.0), 0.7, 1.5);
        let i = instance_of(&m, BoneRef::default());
        let p = [1.0, 2.0, 3.0];
        let want = m.transform_point3(Vec3::from(p));
        let got = i.transform_point(p);
        for k in 0..3 {
            assert!((got[k] - want[k]).abs() < 1e-4, "{got:?} {want:?}");
        }
        // B1's rotate_y, as `from_static` writes it.
        let mut s = crate::static_model::StaticInstance::new([10.0, 0.0, -5.0], 0.7, 1.5);
        s.alpha = 1.0;
        let o = ObjectInstance::from_static(&s);
        let q = o.transform_point(p);
        for k in 0..3 {
            assert!((got[k] - q[k]).abs() < 1e-4, "{got:?} {q:?}");
        }
    }
}
