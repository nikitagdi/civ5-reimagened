//! `City` subsystem (world-objects WOB0): the cities by key
//! `(owner, id)`.
//!
//! Like `Unit`: the records and the placed instances live here. B1's static pipeline draws the one
//! composed model of a city (`frame::b1_systems::B1Unit`); `upsert` places only the city that
//! changed. The banner (`WorldLabel`) is B1's in both.
//!
//! WOB5 is the second placement beside it, for the models of `assets-b2` (`refresh_b2`), like the
//! landmarks' (`world::landmark`): [`compose`] chooses the buildings of the catalog model of the
//! owner's culture and era on the lots of [`layout`] (the
//! ported `cvCityBuildingLayoutGen` over the authored [`fields`]), [`walls`] rings the city with
//! the round wall of [`wall_gen`], [`wonders`] stands the wonders and the buildings with a model of
//! their own on the hex corners of `FindWonderPlacementSlot`, and `AnimService` plays
//! the clips of an animated wonder (`sync_anim`). The draw is a third [`UnitDraw`] that records
//! `Main.City` (`BuildingShader` for the buildings). The ground of a city (the `LandmarkShader` quad)
//! is not drawn there but laid into the page of the terrain as a decal ([`City::take_ground_update`]),
//! as the original's `cvCityVisSystem::InitMeshDecals` does: drawn as a blended quad it read no fog
//! of war and stood as a bright sand box under a city out of sight (user report fb4).
// re: rw-w8-answers.txt q2 (cvCityVisSystem::InitMeshDecals 0x088563aa feeds the meshes of the
// building set into the TerrainDecalLayer through InitializeMeshDecalsForModel<cvCityBuildingSet>)

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use asset_format::{ArtTable, LogicalId, ModelBlobV2};
use glam::{Mat4, Vec2, Vec3};

use crate::anim::landmark_tree::anim_state;
use crate::anim::{AnimHandle, AnimService, AnimState, AnimTarget, BoneRef, LandmarkState};
use crate::entities::cities::{CityPlaced, place_city};
use crate::entities::{CityEntity, CityKey, Culture, EntityPlan, EraBucket, Owners, PlanContext};
use crate::frame::{
    DrawContext, InitContext, MainSlot, MaskContext, PassId, PassSlot, PrepareContext,
    RenderSystem, SystemId,
};
use crate::hexgrid::cell_center;
use crate::world::landmark::ground::{GroundMesh, landmark_decals};
use crate::world::landmark::{GroundUpdate, instance_of};
use crate::world::objects::input::{Keyed, ObjectCounters};
use crate::world::objects::instance::ObjectInstance;
use crate::world::particle::bindings::CityFire;
use crate::world::terrain::decals::MeshDecal;
use crate::world::unit::draw::{self, PartBatches};
use crate::world::unit::place::push_to;
use crate::world::unit::{B2Context, B2Frame};

pub mod compose;
pub mod damage;
pub mod fields;
pub mod great_wall;
pub mod layout;
pub mod layout_gen;
pub mod msvc;
pub mod wall_gen;
pub mod walls;
pub mod wonders;

use compose::ModelMeta;
use fields::LayoutFields;
use layout::{CITY_SCALE, PlotEdges};

/// What the layout of a city reads of the map: per plot, water, mountain and rivers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlotGrid {
    pub width: i32,
    pub height: i32,
    pub wrap_x: bool,
    pub cells: Vec<PlotEdges>,
}

impl PlotGrid {
    /// The plot at `(col, row)`; `None` past the map.
    pub fn get(&self, col: i32, row: i32) -> Option<PlotEdges> {
        if !(0..self.height).contains(&row) {
            return None;
        }
        let col = if self.wrap_x {
            col.rem_euclid(self.width)
        } else if (0..self.width).contains(&col) {
            col
        } else {
            return None;
        };
        self.cells.get((row * self.width + col) as usize).copied()
    }
}

/// The fields of the layout when the pack has none (see [`layout::stand_in_fields`]).
fn stand_in() -> &'static LayoutFields {
    static FIELDS: std::sync::OnceLock<LayoutFields> = std::sync::OnceLock::new();
    FIELDS.get_or_init(layout::stand_in_fields)
}

/// The inputs of the layout beside the frame: the authored fields and the map.
#[derive(Clone, Copy)]
struct LayoutEnv<'a> {
    fields: &'a LayoutFields,
    plots: Option<&'a PlotGrid>,
}

/// One draw of a city model: the whole model, or one mesh of it, at a place.
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    /// `None`: all the meshes of the model; `Some`: the mesh of this index (see
    /// [`compose::split_by_mesh`], where the mesh is a part of its own).
    pub part: Option<u16>,
    /// Model space to our world.
    pub matrix: Mat4,
}

/// What one model contributes to a city.
#[derive(Debug, Clone)]
pub struct Placed {
    /// The place of the model among those of the city (the key of its animation): 0 the catalog,
    /// 1 the ruins, 2 the wall, [`SPECIAL_SLOT`] and up the wonders.
    pub slot: u16,
    pub model: LogicalId,
    /// Empty while the model has not come.
    pub pieces: Vec<Piece>,
    /// The model is not here yet: the city is placed again when it comes.
    pub pending: bool,
    /// A wonder: it may have clips.
    pub animated: bool,
}

/// The slot of the first wonder among the models of a city.
pub const SPECIAL_SLOT: u16 = 10;

/// The raw height above which the bisection of a harbor sees land (`OnWonderCreated`: 12999).
const SEA_RAW: u16 = 12_999;

/// The draw of WOB5 and what `prepare` needs of the frame that `b2_frame` left.
struct B2State {
    draw: draw::UnitDraw,
    active: bool,
    shifts: Vec<f32>,
    eye: Vec3,
}

/// The cities. Draws through B1's adapter; see the module text.
#[derive(Default)]
pub struct City {
    records: Keyed<CityKey, CityEntity>,
    placed: BTreeMap<CityKey, CityPlaced>,
    counters: ObjectCounters,
    /// WOB5: the cities to place again for the models of `assets-b2`.
    dirty_b2: BTreeSet<CityKey>,
    placed_b2: BTreeMap<CityKey, Vec<Placed>>,
    /// The fire particles over the buildings of the hurt cities as the last refresh placed them,
    /// and the cities whose fires changed since [`City::take_fire_changes`].
    fires: BTreeMap<CityKey, Vec<CityFire>>,
    fires_changed: BTreeSet<CityKey>,
    /// The cities on a visible plot (see [`City::set_seen`]).
    seen: BTreeSet<CityKey>,
    models: BTreeMap<LogicalId, ModelMeta>,
    /// The animation of each wonder that has one, and the state it was last given.
    /// The third is the pass of `sync_anim` that last saw the piece.
    handles: BTreeMap<(CityKey, u16), (AnimHandle, AnimState, u32, LogicalId)>,
    anim_epoch: u32,
    placed_b2_last: u32,
    placed_b2_total: u64,
    /// Bumped whenever `placed_b2` changes; with the bone references of the wonders it decides
    /// whether the instances of the last frame are the instances of this one.
    place_gen: u64,
    inst_cache: Option<InstCache>,
    instances_reused: u64,
    b2: Option<Box<B2State>>,
    /// R5.11: the models of the Great Wall (`HB_Great_Wall.gr2`) once they came, and the owners
    /// and the frame the wall is placed on.
    great_wall: Option<great_wall::GreatWallMeta>,
    territory: Option<great_wall::TerritoryInput>,
    /// The density and tensor fields of the layout system, once they came.
    layout_fields: Option<LayoutFields>,
    /// The water, mountains and rivers of the map (the hex-edge pattern of each city).
    plots: Option<PlotGrid>,
    /// The ground meshes of each model that has some ([`GroundMesh`], by the mesh's index: the part
    /// of [`compose::split_by_mesh`]), laid into the terrain as decals.
    grounds: BTreeMap<LogicalId, Vec<GroundMesh>>,
    /// The placed cities changed since the last [`City::take_ground_update`], and the decals the
    /// terrain holds by the plot of the city.
    grounds_dirty: BTreeSet<CityKey>,
    grounds_all: bool,
    grounded: BTreeMap<u32, Vec<MeshDecal>>,
    /// The layout of each city as the last solve made it, with the fingerprint of its inputs: a
    /// city is laid out again only when that changes (the solve is the dearest part of a placing).
    layouts: BTreeMap<CityKey, LayoutEntry>,
    /// The layouts being solved over frames after the load (a few ticks a frame).
    layout_jobs: BTreeMap<CityKey, LayoutProgress>,
    /// Bumped when the authored fields change (they are inputs of every layout).
    fields_gen: u64,
    /// The fingerprint of the ground under each city as it was placed.
    ground_fp: BTreeMap<CityKey, u64>,
    /// The cities whose ground is to be compared with the one they were placed on (the terrain
    /// built cells), and whether all of them are to be queued.
    height_check: BTreeSet<CityKey>,
    height_check_all: bool,
    /// Bumped when a record is added, changed or dropped (a cheap gate for the per-frame feeds).
    epoch: u64,
    layouts_built: u64,
    layouts_deferred: u64,
    /// Before the first full view (see `Renderer::map_loading`).
    loading: bool,
    height_checked: u64,
    /// The ground the houses of each placed city clear of trees (re-b17 3.1), the changes since
    /// [`City::take_cut_changes`], and whether they are to be taken whole.
    cuts: BTreeMap<CityKey, Vec<[Vec2; 3]>>,
    cut_changes: Vec<crate::world::landmark::CutChange>,
    cuts_all: bool,
}

/// Half the side of the square of ground a house clears of trees (`UpdateBuildingBase` 0x087b90b8:
/// a quad of `(x +- 5, y +- 5)` stamped with the tree-cut bit 0x02).
const HOUSE_CUT_HALF: f32 = 5.0;

/// The tree cuts of the houses of a layout, in our world: two triangles per house.
// SIMPLIFIED(SIMP-exe-1250): the quads go to the cuts the trees test on the CPU (as the roads',
// SIMP-exe-1161), not into the stencil; every house of the layout cuts, drawn or not.
fn house_cuts(houses: &[layout::House]) -> Vec<[Vec2; 3]> {
    houses
        .iter()
        .flat_map(|h| {
            let p = layout::from_orig(h.pos);
            let [a, b, c, d] = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
                .map(|(sx, sy)| p + Vec2::new(sx, sy) * HOUSE_CUT_HALF);
            [[a, b, c], [a, c, d]]
        })
        .collect()
}

/// The instances of a frame, the placing they were made for and the bone references they hold.
type InstCache = (
    u64,
    Vec<BoneRef>,
    (BTreeMap<LogicalId, Vec<ObjectInstance>>, PartBatches),
);

/// The most layouts one frame solves; the other cities keep the layout they are drawn with.
pub const LAYOUTS_PER_FRAME: u32 = 3;
/// The layouts a frame may solve for cities that have none yet while the map loads (every city is
/// new), and after the first full view (a new city, about a millisecond).
const COLD_LAYOUTS_PER_FRAME: u32 = 8;
const COLD_LAYOUTS_PLAYING: u32 = 1;
/// The most cities one frame places again.
const PLACES_PER_FRAME: usize = 48;
/// The same after the first full view.
const PLACES_PLAYING: usize = 6;
/// The ticks of the relaxation of the lots (a layout is 40, about half a millisecond each) that the
/// frames after the load spend in all.
const LAYOUT_TICKS_PLAYING: u32 = 3;
/// The most cities one frame compares the ground of.
const HEIGHT_CHECKS_PER_FRAME: usize = 160;

/// A solved layout and what it was solved from.
struct LayoutEntry {
    fp: u64,
    plan: Arc<layout::CityLayout>,
    /// The state of the city's random numbers after the solve (the model picks go on from it).
    rand: msvc::MsvcRand,
}

/// The layouts of the cities, and what a frame may still solve.
struct LayoutBook<'a> {
    cache: &'a mut BTreeMap<CityKey, LayoutEntry>,
    /// The layouts being solved over frames, by city.
    jobs: &'a mut BTreeMap<CityKey, LayoutProgress>,
    fields_gen: u64,
    budget: u32,
    built: u32,
    /// The ticks of the relaxation that the frame may still do (all of them while the map loads).
    ticks: u32,
}

/// A layout part way through its solve, and the fingerprint of the inputs it was begun from.
struct LayoutProgress {
    fp: u64,
    job: layout::LayoutJob,
}

fn mix64(h: u64, v: u64) -> u64 {
    (h ^ v).wrapping_mul(0x0000_0100_0000_01b3).rotate_left(23)
}

/// A fingerprint of the ground round a city: the heights on a grid over its layout rect.
fn ground_fingerprint(centre: Vec2, ground: &dyn Fn(Vec2) -> f32) -> u64 {
    const N: i32 = 9;
    let step = layout::RECT_HALF * 1.2 / (N / 2) as f32;
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for j in -(N / 2)..=(N / 2) {
        for i in -(N / 2)..=(N / 2) {
            let p = centre + Vec2::new(i as f32, j as f32) * step;
            h = mix64(h, u64::from(ground(p).to_bits()));
        }
    }
    h
}

impl City {
    /// The key of `c`.
    pub fn key_of(c: &CityEntity) -> CityKey {
        (c.owner, c.id)
    }

    /// Adds or replaces a city. True if the record changed.
    pub fn upsert(&mut self, c: CityEntity) -> bool {
        let key = Self::key_of(&c);
        let changed = self.records.upsert(key, c).1;
        if changed {
            self.dirty_b2.insert(key);
            self.epoch += 1;
        }
        changed
    }

    /// Drops a city. True if there was one.
    pub fn remove(&mut self, key: CityKey) -> bool {
        self.epoch += 1;
        self.layouts.remove(&key);
        self.ground_fp.remove(&key);
        self.height_check.remove(&key);
        self.placed.remove(&key);
        if self.placed_b2.remove(&key).is_some() {
            self.grounds_dirty.insert(key);
        }
        self.dirty_b2.remove(&key);
        self.seen.remove(&key);
        self.fires.remove(&key);
        self.fires_changed.remove(&key);
        self.set_cuts(key, Vec::new());
        self.records.remove(&key).is_some()
    }

    /// The tree cuts of a city are now `cuts`; a change is queued for the trees.
    fn set_cuts(&mut self, key: CityKey, cuts: Vec<[Vec2; 3]>) {
        let old = self.cuts.get(&key).cloned().unwrap_or_default();
        if old == cuts {
            return;
        }
        if cuts.is_empty() {
            self.cuts.remove(&key);
        } else {
            self.cuts.insert(key, cuts.clone());
        }
        if !self.cuts_all {
            self.cut_changes.push((old, cuts));
        }
    }

    /// The ground the houses of the placed cities clear of trees, world `(X, Z)` (re-b17 3.1); none
    /// while the models of `assets-b2` are not drawn.
    pub fn tree_cuts(&self) -> Vec<[Vec2; 3]> {
        if !self.b2_active() {
            return Vec::new();
        }
        self.cuts.values().flatten().copied().collect()
    }

    /// True once after the cuts changed as a whole (the trees take [`City::tree_cuts`] again).
    pub fn take_cuts_dirty(&mut self) -> bool {
        let all = std::mem::take(&mut self.cuts_all);
        if all {
            self.cut_changes.clear();
        }
        all
    }

    /// The single changes of the cuts since the last call: the triangles taken out and put in.
    pub fn take_cut_changes(&mut self) -> Vec<crate::world::landmark::CutChange> {
        std::mem::take(&mut self.cut_changes)
    }

    /// Drops every city.
    pub fn clear(&mut self) {
        self.epoch += 1;
        self.layouts.clear();
        self.ground_fp.clear();
        self.height_check.clear();
        self.records.clear();
        self.placed.clear();
        self.placed_b2.clear();
        self.grounds_all = true;
        self.dirty_b2.clear();
        self.seen.clear();
        self.fires.clear();
        self.fires_changed.clear();
        self.cuts.clear();
        self.cuts_all = true;
    }

    /// Every city is placed again at the next refresh (new ground, art or owners).
    pub fn invalidate_all(&mut self) {
        self.records.touch_all();
        self.dirty_b2.extend(self.records.keys().copied());
    }

    /// The terrain built cells: the cities compare the ground under them with the one they were
    /// placed on (a few per frame) and are placed again only if it differs.
    pub fn heights_changed(&mut self) {
        self.height_check_all = true;
    }

    /// The map is loading (before the first full view): new cities may be laid out faster.
    pub fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
    }

    /// The terrain built these cells (`(x, y)` of a hex): the cities on them or next to them
    /// compare the ground under them with the one they were placed on.
    pub fn heights_dirty(&mut self, cells: &[(i32, i32)]) {
        let near = |x: i32, y: i32| {
            cells
                .iter()
                .any(|&(cx, cy)| (cx - x).abs() <= 2 && (cy - y).abs() <= 2)
        };
        let hit: Vec<CityKey> = self
            .records
            .iter()
            .filter(|(_, c)| near(c.x, c.y))
            .map(|(k, _)| *k)
            .collect();
        self.height_check.extend(hit);
    }

    /// True while cities wait to be placed again or to have their ground compared.
    pub fn placing_pending(&self) -> bool {
        self.b2_active()
            && (!self.dirty_b2.is_empty() || !self.height_check.is_empty() || self.height_check_all)
    }

    /// The record epoch: changes when a city is added, changed or dropped.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Frames that reused the instances of the frame before.
    pub fn instances_reused(&self) -> u64 {
        self.instances_reused
    }

    /// `(layouts solved, placings deferred by the budget, ground checks)` since the start.
    pub fn layout_counters(&self) -> (u64, u64, u64) {
        (
            self.layouts_built,
            self.layouts_deferred,
            self.height_checked,
        )
    }

    /// Tells which cities stand on a visible plot (`visible(x, y)`, fog of war): the damage effects
    /// show on those only. A city whose answer changed is placed again.
    pub fn set_seen(&mut self, visible: impl Fn(i32, i32) -> bool) {
        let now: BTreeSet<CityKey> = self
            .records
            .iter()
            .filter(|(_, c)| visible(c.x, c.y))
            .map(|(k, _)| *k)
            .collect();
        if now != self.seen {
            self.dirty_b2
                .extend(now.symmetric_difference(&self.seen).copied());
            self.seen = now;
        }
    }

    /// The cities that use `model` are placed again.
    pub fn invalidate_model(&mut self, model: &LogicalId) {
        let hit: Vec<CityKey> = self
            .placed
            .iter()
            .filter(|(_, p)| p.deps.contains(model))
            .map(|(k, _)| *k)
            .collect();
        for key in hit {
            self.records.touch(key);
        }
        for (key, list) in &self.placed_b2 {
            if list.iter().any(|p| &p.model == model) {
                self.dirty_b2.insert(*key);
            }
        }
    }

    pub fn get(&self, key: &CityKey) -> Option<&CityEntity> {
        self.records.get(key)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn cities(&self) -> impl Iterator<Item = &CityEntity> {
        self.records.values()
    }

    /// Places the cities that changed; returns how many it placed. With the models of `assets-b2`
    /// in use only the banner is B1's.
    pub fn refresh(&mut self, owners: &Owners, ctx: &PlanContext<'_>) -> usize {
        let dirty = self.records.take_dirty();
        let models = !self.b2_active();
        for key in &dirty {
            match self.records.get(key) {
                Some(c) => {
                    self.placed.insert(*key, place_city(c, owners, ctx, models));
                }
                None => {
                    self.placed.remove(key);
                }
            }
        }
        self.counters.placed_last = dirty.len() as u32;
        self.counters.placed_total += dirty.len() as u64;
        dirty.len()
    }

    /// Adds every placed city to `plan`, in the order of B1 (by id).
    pub fn assemble(&self, plan: &mut EntityPlan) {
        let mut all: Vec<(&CityKey, &CityPlaced)> = self.placed.iter().collect();
        all.sort_by_key(|(k, _)| (k.1, k.0));
        for (_, placed) in all {
            placed.add_to(plan);
        }
    }

    pub fn counters(&self) -> ObjectCounters {
        ObjectCounters {
            records: self.records.len() as u32,
            ..self.counters
        }
    }

    /// The renderer tells what the draw got from the whole plan.
    pub fn set_assembled(&mut self, instances: usize, batches: usize) {
        if self.b2_active() {
            return;
        }
        self.counters.instances = instances as u32;
        self.counters.batches = batches as u32;
    }

    // ---- WOB5: the models of `assets-b2` ----

    /// A model arrived: what the placement needs of it (`blob` is the model as the file has it,
    /// before [`compose::split_by_mesh`]) is kept and the cities that wait for it are placed again.
    pub fn register_model(&mut self, id: LogicalId, blob: &ModelBlobV2) {
        if id.as_str() == great_wall::GREAT_WALL_MODEL {
            self.great_wall = Some(great_wall::GreatWallMeta::of(blob));
        }
        self.models.insert(id.clone(), ModelMeta::of(blob));
        let grounds = GroundMesh::of_model(&compose::split_by_mesh(blob));
        if grounds.is_empty() {
            self.grounds.remove(&id);
        } else {
            self.grounds.insert(id.clone(), grounds);
        }
        self.invalidate_model(&id);
    }

    /// The decals of the ground meshes of the placed cities, by the plot of the city.
    fn ground_decals(
        &self,
        width: i32,
        only: Option<&BTreeSet<CityKey>>,
    ) -> BTreeMap<u32, Vec<MeshDecal>> {
        let mut now: BTreeMap<u32, Vec<MeshDecal>> = BTreeMap::new();
        for (key, list) in &self.placed_b2 {
            if only.is_some_and(|o| !o.contains(key)) {
                continue;
            }
            let Some(city) = self.records.get(key) else {
                continue;
            };
            let cell = (city.y * width + city.x) as u32;
            for p in list.iter().filter(|p| !p.pending) {
                let Some(grounds) = self.grounds.get(&p.model) else {
                    continue;
                };
                let pieces: Vec<(Option<u16>, Mat4)> =
                    p.pieces.iter().map(|x| (x.part, x.matrix)).collect();
                let d = landmark_decals(grounds, &pieces, asset_format::STATE_INTACT, cell);
                if !d.is_empty() {
                    now.entry(cell).or_default().extend(d);
                }
            }
        }
        now
    }

    /// The ground meshes of the placed cities have changed: the decals of the plots whose ground is
    /// not what the layer holds (none while the models of `assets-b2` are not drawn), and those
    /// plots, for `DecalLayer::set_mesh_decals`. `width` is the width of the map in plots. `None`
    /// when nothing changed.
    pub fn take_ground_update(&mut self, width: i32) -> Option<GroundUpdate> {
        let all = std::mem::take(&mut self.grounds_all);
        if !all && self.grounds_dirty.is_empty() {
            return None;
        }
        let dirty = std::mem::take(&mut self.grounds_dirty);
        let dirty_cells: BTreeSet<u32> = dirty
            .iter()
            .filter_map(|k| self.records.get(k))
            .map(|c| (c.y * width + c.x) as u32)
            .collect();
        let mut now = if self.b2_active() {
            self.ground_decals(width, (!all).then_some(&dirty))
        } else {
            BTreeMap::new()
        };
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

    /// R5.11: the owner of every hex and the frame of the map, for the Great Wall. The cities with
    /// the wonder are placed again when they change; the same again does nothing.
    pub fn set_territory(
        &mut self,
        owners: Option<&crate::world::overlay::OwnersInput>,
        frame: Option<crate::world::terrain::sampler::TerrainFrame>,
    ) {
        let now = match (owners, frame) {
            (Some(o), Some(f)) if (o.width, o.height) == (f.width_hexes(), f.height_hexes()) => {
                Some((o, f))
            }
            _ => None,
        };
        let same = match (&self.territory, now) {
            (Some(t), Some((o, f))) => t.frame == f && t.owners == o.owners,
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.territory = now.map(|(o, frame)| great_wall::TerritoryInput {
            frame,
            owners: o.owners.clone(),
        });
        let walled: Vec<CityKey> = self
            .records
            .iter()
            .filter(|(_, c)| c.buildings.iter().any(|t| t == great_wall::GREAT_WALL_TAG))
            .map(|(k, _)| *k)
            .collect();
        self.dirty_b2.extend(walled);
    }

    /// The fields of the layout system (`tx:df_*`, `tx:tf_*` of `b2-city`); every city is laid out
    /// again with them.
    pub fn set_layout_fields(&mut self, fields: LayoutFields) {
        self.layout_fields = Some(fields);
        self.fields_gen += 1;
        self.invalidate_all();
    }

    /// The water, mountains and rivers of the map; the cities are laid out again when it changes
    /// (the same again does nothing).
    pub fn set_plots(&mut self, plots: Option<PlotGrid>) {
        if self.plots == plots {
            return;
        }
        self.plots = plots;
        self.invalidate_all();
    }

    /// True if a city is waiting for `model`.
    pub fn wants_model(&self, model: &LogicalId) -> bool {
        self.placed_b2
            .values()
            .flatten()
            .any(|p| p.pending && &p.model == model)
    }

    /// The models of a city as the last refresh placed them.
    pub fn placed_at(&self, key: &CityKey) -> &[Placed] {
        self.placed_b2.get(key).map_or(&[], Vec::as_slice)
    }

    /// Places again the cities that changed, for the models of `assets-b2`; returns how many. A
    /// frame solves at most [`LAYOUTS_PER_FRAME`] layouts and places [`PLACES_PER_FRAME`] cities;
    /// the others stay queued and keep what they are drawn with.
    pub fn refresh_b2(&mut self, ctx: &B2Context<'_>) -> usize {
        if std::mem::take(&mut self.height_check_all) {
            self.height_check.extend(self.records.keys().copied());
        }
        let slow_checks = crate::perf::slow::scope("city.height_checks");
        for _ in 0..HEIGHT_CHECKS_PER_FRAME {
            let Some(key) = self.height_check.pop_first() else {
                break;
            };
            self.height_checked += 1;
            let Some(city) = self.records.get(&key) else {
                continue;
            };
            let fp = ground_fingerprint(cell_center(city.x, city.y), ctx.ground);
            if self.ground_fp.get(&key).is_some_and(|old| *old != fp) {
                self.dirty_b2.insert(key);
            }
        }
        drop(slow_checks);
        let _slow_place = crate::perf::slow::scope("city.place");
        let mut queue = std::mem::take(&mut self.dirty_b2);
        let touched = !queue.is_empty();
        let mut n = 0usize;
        // After the load a frame spends little on cities: one layout, a few placings.
        let mut budget = if self.loading { LAYOUTS_PER_FRAME } else { 1 };
        let places = if self.loading {
            PLACES_PER_FRAME
        } else {
            PLACES_PLAYING
        };
        let mut cold_budget = if self.loading {
            COLD_LAYOUTS_PER_FRAME
        } else {
            COLD_LAYOUTS_PLAYING
        };
        // After the load the relaxation of the lots goes a few ticks a frame, over frames.
        let mut tick_pool = if self.loading {
            u32::MAX
        } else {
            LAYOUT_TICKS_PLAYING
        };
        // The fields and the map are taken out while the cities are placed, put back after.
        let fields = self.layout_fields.take();
        let plots = self.plots.take();
        let env = LayoutEnv {
            fields: fields.as_ref().unwrap_or_else(|| stand_in()),
            plots: plots.as_ref(),
        };
        while let Some(key) = queue.pop_first() {
            let Some(city) = self.records.get(&key) else {
                self.layout_jobs.remove(&key);
                if self.placed_b2.remove(&key).is_some() {
                    self.grounds_dirty.insert(key);
                }
                self.set_fires(key, Vec::new());
                self.set_cuts(key, Vec::new());
                continue;
            };
            if n >= places {
                self.dirty_b2.insert(key);
                continue;
            }
            let mut fires = Vec::new();
            let mut cuts = Vec::new();
            // A city that has no layout yet draws on the share for new cities.
            let cold = !self.layouts.contains_key(&key);
            let mut book = LayoutBook {
                cache: &mut self.layouts,
                jobs: &mut self.layout_jobs,
                fields_gen: self.fields_gen,
                budget: if cold { cold_budget } else { budget },
                built: 0,
                ticks: tick_pool,
            };
            let slow_one = crate::perf::slow::scope(if cold {
                "city.place_cold"
            } else {
                "city.place_one"
            });
            let placed = place_b2_fires(
                &self.models,
                ctx,
                env,
                &mut book,
                key,
                city,
                self.seen.contains(&key),
                &mut fires,
                &mut cuts,
            );
            drop(slow_one);
            tick_pool = book.ticks;
            if cold {
                cold_budget = book.budget;
            } else {
                budget = book.budget;
            }
            self.layouts_built += u64::from(book.built);
            let Some(mut list) = placed else {
                // Out of budget for a layout: keep the old placing, try again next frame. A city
                // that has none yet asks for its models at once (they are the long wait of the
                // load), as pending entries without a place.
                self.layouts_deferred += 1;
                self.dirty_b2.insert(key);
                if cold && !self.placed_b2.contains_key(&key) {
                    let wanted = wanted_models(&self.models, ctx, city);
                    if !wanted.is_empty() {
                        self.placed_b2.insert(key, wanted);
                        self.place_gen += 1;
                    }
                }
                continue;
            };
            n += 1;
            self.ground_fp.insert(
                key,
                ground_fingerprint(cell_center(city.x, city.y), ctx.ground),
            );
            if let Some(wall) = great_wall::placed(
                self.great_wall.as_ref(),
                self.territory.as_ref(),
                &city.buildings,
                (city.x, city.y),
                ctx.ground,
                layout::seed_of(key),
            ) {
                list.push(wall);
            }
            // The ground meshes of the models are the terrain's decals, not drawn as models.
            if let Some(b2) = self.b2.as_mut() {
                for p in list.iter().filter(|p| self.grounds.contains_key(&p.model)) {
                    b2.draw.set_ground_decals(&p.model, true);
                }
            }
            let grounds = &self.grounds;
            let sig = |l: &[Placed]| {
                l.iter()
                    .filter(|p| !p.pending && grounds.contains_key(&p.model))
                    .map(|p| (p.model.clone(), p.pieces.clone()))
                    .collect::<Vec<_>>()
            };
            let old = self.placed_b2.get(&key).map(|l| sig(l)).unwrap_or_default();
            if old != sig(&list) {
                self.grounds_dirty.insert(key);
            }
            if list.is_empty() {
                self.placed_b2.remove(&key);
            } else {
                self.placed_b2.insert(key, list);
            }
            self.set_fires(key, fires);
            self.set_cuts(key, cuts);
        }
        self.place_gen += u64::from(touched);
        self.layout_fields = fields;
        self.plots = plots;
        self.placed_b2_last = n as u32;
        self.placed_b2_total += n as u64;
        n
    }

    fn set_fires(&mut self, key: CityKey, fires: Vec<CityFire>) {
        if fires.is_empty() {
            if self.fires.remove(&key).is_some() {
                self.fires_changed.insert(key);
            }
        } else if self.fires.get(&key) != Some(&fires) {
            self.fires.insert(key, fires);
            self.fires_changed.insert(key);
        }
    }

    /// The cities whose fire particles changed since the last call, with their fires now (empty:
    /// none), for the particle player.
    pub fn take_fire_changes(&mut self) -> Vec<(CityKey, Vec<CityFire>)> {
        std::mem::take(&mut self.fires_changed)
            .into_iter()
            .map(|k| (k, self.fires.get(&k).cloned().unwrap_or_default()))
            .collect()
    }

    /// How many cities the last `refresh_b2` placed, and since the start.
    pub fn placed_b2(&self) -> (u32, u64) {
        (self.placed_b2_last, self.placed_b2_total)
    }

    /// The animation of the wonders: a wonder gets a handle when its model has clips, keeps it while
    /// it stands, and the ones that are gone give theirs back.
    pub fn sync_anim(&mut self, anim: &mut AnimService) {
        let state = anim_state(LandmarkState::Constructed, false);
        // A mark in the handle says it was seen in this pass, instead of a set of every live key.
        self.anim_epoch = self.anim_epoch.wrapping_add(1);
        let epoch = self.anim_epoch;
        for (key, list) in &self.placed_b2 {
            for p in list.iter().filter(|p| p.animated && !p.pending) {
                let k = (*key, p.slot);
                if let Some((h, _, seen, model)) = self.handles.get_mut(&k)
                    && anim.is_live(*h)
                    && *model == p.model
                {
                    *seen = epoch;
                    continue;
                }
                if let Some((old, _, _, _)) = self.handles.remove(&k) {
                    anim.release(old);
                }
                let target = AnimTarget::Wonder {
                    city: *key,
                    slot: p.slot,
                };
                if let Some(h) = anim.attach(&p.model, target) {
                    anim.set_state(h, state);
                    self.handles.insert(k, (h, state, epoch, p.model.clone()));
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

    /// The instances of this frame: whole models by model, single meshes by (model, mesh). A model
    /// whose clip is not in the bone texture yet is drawn in the pose of the bind.
    pub fn frame_instances(
        &self,
        anim: &AnimService,
    ) -> (BTreeMap<LogicalId, Vec<ObjectInstance>>, PartBatches) {
        let mut whole: BTreeMap<LogicalId, Vec<ObjectInstance>> = BTreeMap::new();
        let mut parts: PartBatches = BTreeMap::new();
        for (key, list) in &self.placed_b2 {
            for p in list {
                let bones = match self.handles.get(&(*key, p.slot)) {
                    Some((h, _, _, _)) => anim.bone_ref(*h),
                    None => BoneRef::default(),
                };
                // The key with the model name is made once per piece list, not once per piece.
                let mut part_key: Option<(LogicalId, u16)> = None;
                for piece in &p.pieces {
                    let inst = instance_of(&piece.matrix, bones);
                    match piece.part {
                        None => push_to(&mut whole, &p.model, inst),
                        Some(part) => {
                            let k = part_key.get_or_insert_with(|| (p.model.clone(), part));
                            k.1 = part;
                            push_to(&mut parts, k, inst);
                        }
                    }
                }
            }
        }
        (whole, parts)
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
                draw.set_main_slot(MainSlot::City);
                // The buildings take the ported colour pass of the D3D11 build.
                draw.set_building_code(true);
                self.b2 = Some(Box::new(B2State {
                    draw,
                    active: false,
                    shifts: vec![0.0],
                    eye: Vec3::ZERO,
                }));
            }
        }
    }

    /// The cities draw through the models of `assets-b2`. The art and the light have to be there;
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
        self.grounds_all = true;
        self.cuts_all = true;
        self.invalidate_all();
        true
    }

    pub fn b2_active(&self) -> bool {
        self.b2.as_ref().is_some_and(|b| b.active)
    }

    /// The models of the cities that are not on the GPU yet.
    pub fn b2_models_wanted(&self) -> Vec<LogicalId> {
        let Some(b2) = &self.b2 else {
            return Vec::new();
        };
        let wanted: BTreeSet<&LogicalId> = self
            .placed_b2
            .values()
            .flatten()
            .filter(|p| p.pending)
            .map(|p| &p.model)
            .filter(|m| !b2.draw.has_model(m))
            .collect();
        wanted.into_iter().cloned().collect()
    }

    pub fn b2_draw_mut(&mut self) -> Option<&mut draw::UnitDraw> {
        self.b2.as_mut().map(|b| &mut b.draw)
    }

    pub fn b2_draw(&self) -> Option<&draw::UnitDraw> {
        self.b2.as_ref().map(|b| &b.draw)
    }

    /// The part of the frame that needs the inputs of the renderer, before the plan: places the
    /// cities that changed, asks the animation for their states, and binds the light and the fog
    /// mask to the draw. The instances themselves are made in `prepare`.
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
        // `ColorCorrection` of each model: the colour volume of the style of the city it stands in
        // (`Buildings_<style>`), once its model is on the GPU.
        let styled: Vec<(LogicalId, usize)> = self
            .placed_b2
            .iter()
            .flat_map(|(key, list)| {
                let class = style_class(f.owners.culture_of(key.0));
                list.iter().map(move |p| (p.model.clone(), class))
            })
            .collect();
        // The houses (the catalog and its ruins) are lit by the cube light of their culture and the
        // reflection cubes of the city, not by the unit's cubes as the other buildings are.
        let houses: Vec<(LogicalId, &'static str)> = self
            .placed_b2
            .iter()
            .flat_map(|(key, list)| {
                let light = house_cube_light(f.owners.culture_of(key.0));
                list.iter()
                    .filter(|p| p.slot <= 1)
                    .map(move |p| (p.model.clone(), light))
            })
            .collect();
        let Some(b2) = &mut self.b2 else { return };
        for (model, class) in styled {
            if b2.draw.has_model(&model)
                && !b2.draw.has_color_key(&model)
                && let Some(view) = f.lights.model_color_key(class)
            {
                b2.draw.set_color_key(device, &model, view);
            }
        }
        b2.draw
            .bind_light(device, f.lights, f.frame_globals, f.reflection_globals);
        // re: S3 (measured): t3 of the houses is `hiconreflmap_soft.dds` (5244), t4
        // `hiconreflmap_sharp.dds` (5247); the code lerps from t4 to t3 by the gloss, as from
        // `SpecSoft` (binding 4) to `SpecSharp` (binding 6).
        use crate::lighting::env::{CITY_REFLECTION_SHARP, CITY_REFLECTION_SOFT};
        for (model, light) in houses {
            if b2.draw.has_model(&model)
                && !b2.draw.has_model_light(&model)
                && let (Some(irradiance), Some(t4), Some(t3)) = (
                    f.lights.model_cube_srgb(light),
                    f.lights.model_cube_srgb(CITY_REFLECTION_SHARP),
                    f.lights.model_cube_srgb(CITY_REFLECTION_SOFT),
                )
            {
                b2.draw.set_model_light(device, &model, irradiance, t4, t3);
            }
        }
        b2.draw.bind_fow(device, queue, f.fow);
        b2.shifts = f.shifts.to_vec();
        b2.eye = f.eye;
    }

    /// The inputs of the frame for the ported colour pass of the buildings (`BuildingPS_CODE`):
    /// call it before [`Self::b2_frame`], which binds them.
    pub fn set_code_frame(&mut self, queue: &wgpu::Queue, f: &draw::CodeFrame<'_>) {
        if let Some(b2) = self.b2.as_mut().filter(|b| b.active) {
            b2.draw.set_code_frame(queue, f);
        }
    }

    /// The casters of the cities into the pass of the shadow map.
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

/// The style (`lighting::STYLES`) of the colour volume of a city's buildings: its culture's, the
/// generic one for the Mediterranean (the install has no table of its own for it).
// measured: S3 binds three colour volumes (resources 1951, 1957, 1963) of the same bytes to the
// buildings, so the choice does not show in the captures.
fn style_class(culture: Option<Culture>) -> usize {
    let style = match culture.unwrap_or(crate::entities::cities::FALLBACK_CULTURE) {
        Culture::Afri => "africa",
        Culture::Amer => "america",
        Culture::Asia => "asia",
        Culture::Euro => "europe",
        Culture::Med => "generic",
    };
    crate::lighting::STYLES
        .iter()
        .position(|s| *s == style)
        .unwrap_or(crate::lighting::STYLES.len() - 1)
}

/// The cube light (`Irradiance`) of the houses of a city of `culture`.
// measured: S3 (2026-10-28), the instanced `BuildingPS_CODE_Opaque` draws of the houses bind
// t2 = `europe_cubelight.dds` (resource 5361, the European catalog) or `asia_cubelight.dds` (5291,
// the Asian one), byte for byte, with one colour volume for both: the light goes with the catalog,
// not with the continent. The Mediterranean has no cube of its own: Europe's (assumed).
fn house_cube_light(culture: Option<Culture>) -> &'static str {
    use crate::lighting::env::CITY_CUBE_LIGHTS;
    match culture.unwrap_or(crate::entities::cities::FALLBACK_CULTURE) {
        Culture::Afri => CITY_CUBE_LIGHTS[0],
        Culture::Amer => CITY_CUBE_LIGHTS[1],
        Culture::Asia => CITY_CUBE_LIGHTS[2],
        Culture::Euro | Culture::Med => CITY_CUBE_LIGHTS[3],
    }
}

/// The building types whose models stand in a city: its buildings, then the wonder it is building,
/// half built (`CvCity` sends the production wonder with state 1, re-b17 5.3; the shipped
/// `WonderArtState` is `Any` only, so it is the same model).
// SIMPLIFIED(SIMP-exe-1251): a wonder is a building whose tag has a model of its own and no
// variants (the art table does not say which buildings are wonders); a league project in the
// capital is not drawn.
fn special_types(table: &ArtTable, city: &CityEntity) -> Vec<String> {
    let mut types = city.building_types.clone();
    if let Some(p) = &city.production_building
        && !types.contains(p)
        && table
            .building_type(p)
            .and_then(|r| table.building(&r.art_tag))
            .is_some_and(|a| a.model.is_some() && a.variants.is_empty())
    {
        types.push(p.clone());
    }
    types
}

/// The era of the era variants of the buildings (the harbor, the lighthouse) of an owner: the
/// building art goes by `Eras.ArtPrefix` up to the owner's era (re-b17 5.6), where the Medieval
/// and the Renaissance take the `MEDIEVAL` art, while the city catalog takes the Ancient set for
/// the Medieval (re-b17 4.4).
fn building_era(owners: &crate::entities::Owners, owner: i8) -> EraBucket {
    match owners.era_index_of(owner) {
        Some(0 | 1) => EraBucket::Ancient,
        Some(2 | 3) => EraBucket::Renaissance,
        Some(4) => EraBucket::Industrial,
        Some(_) => EraBucket::Modern,
        None => owners.era_of(owner),
    }
}

/// The catalog model of a city: the owner's culture and era, an older era of the culture when the
/// era has none, then the same of Europe. A city being razed looks the same until it is gone
/// (re-b17 4.2: the original sends nothing new to the city vis while it burns).
fn catalog_of(table: &ArtTable, culture: Culture, era: EraBucket) -> Option<&LogicalId> {
    const ERAS: [EraBucket; 4] = [
        EraBucket::Modern,
        EraBucket::Industrial,
        EraBucket::Renaissance,
        EraBucket::Ancient,
    ];
    let from = ERAS.iter().position(|e| *e == era).unwrap_or(0);
    let older = || ERAS[from..].iter();
    [culture, Culture::Euro]
        .into_iter()
        .flat_map(|c| older().map(move |e| (c, *e)))
        .find_map(|(c, e)| table.city_b2(c.prefix(), e.key()))
        .map(|c| &c.model)
}

/// The matrix lifted by the height of the surface (the ground, or the water) at the point where
/// `anchor` of the model stands: a lot is on the ground at its own middle, not at the origin of the
/// model it belongs to.
fn on_surface(mut m: Mat4, anchor: Vec3, surface: &dyn Fn(Vec2) -> f32) -> Mat4 {
    let at = m.transform_point3(anchor);
    m.w_axis.y += surface(Vec2::new(at.x, at.z));
    m
}

/// The point of the model space that `placement` carries to the origin: the middle of a lot.
fn lot_anchor(lot: &compose::Lot) -> Vec3 {
    lot.placement.inverse().transform_point3(Vec3::ZERO)
}

/// The damage effect models (`FXM_City_Damage_State_*`) over the buildings `chosen` of a city, one
/// [`Placed`] per distinct model; a model that has not come is pending. Empty for a city that has
/// lost nothing.
fn damage_effects(
    models: &BTreeMap<LogicalId, ModelMeta>,
    city: &CityEntity,
    chosen: &[compose::Chosen],
    seed: u32,
    at_city: Mat4,
    surface: &dyn Fn(Vec2) -> f32,
) -> Vec<Placed> {
    if city.max_hp <= 0 {
        return Vec::new();
    }
    let level = damage_level_of(city);
    let mut by_model: BTreeMap<&'static str, Vec<Piece>> = BTreeMap::new();
    for e in damage::effects(level, chosen, seed) {
        let matrix = on_surface(damage::matrix(at_city, e.at), Vec3::ZERO, surface);
        by_model
            .entry(e.model)
            .or_default()
            .push(Piece { part: None, matrix });
    }
    by_model
        .into_iter()
        .map(|(model, pieces)| {
            let id = LogicalId::new(model);
            let slot = damage::slot_of(model);
            if models.contains_key(&id) {
                Placed {
                    slot,
                    model: id,
                    pieces,
                    pending: false,
                    animated: false,
                }
            } else {
                pending(slot, &id, false)
            }
        })
        .collect()
}

/// The damage level of a city (0..=3, the one source of it): the raw hit points lost, by the
/// thresholds of [`damage`] (the core sends the damage unscaled, re-b17 4.1).
fn damage_level_of(city: &CityEntity) -> u8 {
    if city.max_hp <= 0 {
        return 0;
    }
    damage::level_of(city.max_hp - city.hp)
}

/// The fire particles of the buildings of a hurt city (the second and third state): the world point
/// of each effect of [`damage::effects`], the same as its model, on a surface `surface`.
fn city_fires(
    city: &CityEntity,
    chosen: &[compose::Chosen],
    seed: u32,
    at_city: Mat4,
    surface: &dyn Fn(Vec2) -> f32,
) -> Vec<CityFire> {
    damage::effects(damage_level_of(city), chosen, seed)
        .into_iter()
        .filter_map(|e| {
            let m = on_surface(damage::matrix(at_city, e.at), Vec3::ZERO, surface);
            Some(CityFire {
                index: e.index,
                name: e.fire()?,
                at: m.transform_point3(Vec3::ZERO),
            })
        })
        .collect()
}

/// The models of one city, placed. `seen` is true while the plot of the city is visible (the damage
/// effects show on visible plots only).
#[cfg(test)]
fn place_b2(
    models: &BTreeMap<LogicalId, ModelMeta>,
    ctx: &B2Context<'_>,
    key: CityKey,
    city: &CityEntity,
    seen: bool,
) -> Vec<Placed> {
    let env = LayoutEnv {
        fields: stand_in(),
        plots: None,
    };
    let mut cache = BTreeMap::new();
    let mut jobs = BTreeMap::new();
    let mut book = LayoutBook {
        cache: &mut cache,
        jobs: &mut jobs,
        fields_gen: 0,
        budget: u32::MAX,
        built: 0,
        ticks: u32::MAX,
    };
    place_b2_fires(
        models,
        ctx,
        env,
        &mut book,
        key,
        city,
        seen,
        &mut Vec::new(),
        &mut Vec::new(),
    )
    .unwrap_or_default()
}

/// The ground box of a special whose origin stands at `at` (our world), turned by `yaw`, in the
/// original's plane: the box of its model, scaled, about the middle of the box.
fn footprint_of(meta: &ModelMeta, scale: f32, at: Vec2, yaw: f32) -> layout::Footprint {
    let half = Vec2::new(meta.hi.x - meta.lo.x, meta.hi.z - meta.lo.z) * 0.5 * scale;
    let off = Mat4::from_rotation_y(yaw)
        .transform_vector3(Vec3::new(meta.centre.x, 0.0, meta.centre.y) * scale);
    let mid = at + Vec2::new(off.x, off.z);
    let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
        .map(|(sx, sz)| layout::to_orig(mid + Vec2::new(sx * half.x, sz * half.y)));
    layout::Footprint { corners }
}

/// [`place_b2`], and the fire particles of the city in `fires` (they do not wait for the models)
/// and the tree cuts of its houses in `cuts`.
#[allow(clippy::too_many_arguments)]
fn place_b2_fires(
    models: &BTreeMap<LogicalId, ModelMeta>,
    ctx: &B2Context<'_>,
    env: LayoutEnv<'_>,
    book: &mut LayoutBook<'_>,
    key: CityKey,
    city: &CityEntity,
    seen: bool,
    fires: &mut Vec<CityFire>,
    cuts: &mut Vec<[Vec2; 3]>,
) -> Option<Vec<Placed>> {
    let culture = ctx
        .owners
        .culture_of(city.owner)
        .unwrap_or(crate::entities::cities::FALLBACK_CULTURE);
    let era = ctx.owners.era_of(city.owner);
    let seed = layout::seed_of(key);
    let centre = cell_center(city.x, city.y);
    let surface = |p: Vec2| {
        let g = (ctx.ground)(p);
        ctx.water_level.map_or(g, |w| g.max(w))
    };
    let at_city = Mat4::from_translation(Vec3::new(centre.x, 0.0, centre.y));
    let mut out = Vec::new();
    let plot = |c: i32, r: i32| env.plots.and_then(|g| g.get(c, r));

    // The wonders and the buildings with a model: the corners of `FindWonderPlacementSlot`.
    let specials = wonders::specials(
        ctx.art,
        &city.buildings,
        &special_types(ctx.art, city),
        culture,
        building_era(ctx.owners, city.owner),
        seed,
    );
    let hex = layout::hex_of(city.x, city.y);
    let usable = |slot: wonders::Slot| {
        wonders::corner_hexes(slot).iter().all(|&(hx, hy)| {
            let (c, r) = layout::plot_of(hx, hy);
            plot(c, r).is_none_or(|p| !p.water && !p.mountain)
        })
    };
    let hex_plot = |h: (i32, i32)| {
        let (c, r) = layout::plot_of(h.0, h.1);
        plot(c, r)
    };
    let land = |p: Vec2| {
        crate::world::terrain::sampler::raw_from_height(
            (ctx.ground)(p),
            crate::world::terrain::sampler::TERRAIN_SCALE_Z,
        ) > SEA_RAW
    };
    let special_at: Vec<Option<wonders::Spot>> = wonders::place(
        &specials,
        &wonders::Ground {
            hex,
            centre,
            plot: &hex_plot,
            land: &land,
        },
        &usable,
    );
    // re: re-b17 FIX 1: what a live report needs to tell the placing of a special (a harbor of
    // kind 2 is a table without the building types).
    for (s, spot) in specials.iter().zip(&special_at) {
        tracing::debug!(
            city = %city.name,
            hex = ?hex,
            tag = %s.tag,
            kind = s.kind,
            model = s.model.as_str(),
            spot = ?spot.map(|p| (p.at, p.yaw)),
            types = ?ctx.art.building(&s.tag).map(|a| &a.building_types),
            "city special placed"
        );
    }
    let footprints: Vec<layout::Footprint> = specials
        .iter()
        .zip(&special_at)
        .filter_map(|(special, spot)| {
            let meta = models.get(&special.model)?;
            let spot = spot.as_ref()?;
            Some(footprint_of(
                meta,
                special.scale,
                spot.at + centre,
                spot.yaw,
            ))
        })
        .collect();

    // The layout: the wall, the lots, the classes and the burn order.
    let wall_art = walls::wall_of(ctx.art, &city.buildings, culture, era);
    let ground = |p: Vec2| (ctx.ground)(layout::from_orig(p));
    let site = layout::Site {
        centre: layout::to_orig(centre),
        population: layout::city_size(city.population),
        edges: layout::edge_coverage(city.x, city.y, &plot),
        has_wall: wall_art.is_some(),
        ground: &ground,
        water_level: ctx.water_level,
    };
    let slow_fp = crate::perf::slow::scope("city.p_fingerprint");
    let fp = layout_fingerprint(&site, &footprints, book.fields_gen);
    drop(slow_fp);
    let (plan, mut rand) = match book.cache.get(&key).filter(|e| e.fp == fp) {
        Some(e) => (e.plan.clone(), e.rand),
        None => {
            if book.budget == 0 || book.ticks == 0 {
                return None;
            }
            // The solve goes on from where an earlier frame left it, if the inputs are the same;
            // a frame does only the ticks it has left (all of them while the map loads).
            let slow_gen = crate::perf::slow::scope("city.p_generate");
            let mut job = match book.jobs.remove(&key) {
                Some(p) if p.fp == fp => p.job,
                _ => {
                    // The start of a layout (the wall, the mask) counts as a tick.
                    book.ticks = book.ticks.saturating_sub(1);
                    layout::LayoutJob::begin(env.fields, &site, &footprints)
                }
            };
            let used = job.advance(book.ticks);
            book.ticks = book.ticks.saturating_sub(used);
            if !job.is_done() {
                book.ticks = 0;
                book.jobs.insert(key, LayoutProgress { fp, job });
                return None;
            }
            drop(slow_gen);
            book.budget -= 1;
            book.built += 1;
            let mut rand = msvc::MsvcRand::new(seed);
            let plan = Arc::new(job.finish(env.fields, &site, &mut rand));
            book.cache.insert(
                key,
                LayoutEntry {
                    fp,
                    plan: plan.clone(),
                    rand,
                },
            );
            (plan, rand)
        }
    };

    // Every house clears the trees under it (re-b17 3.1).
    cuts.extend(house_cuts(&plan.houses));

    // The catalog: the ground of the city, and the houses on the lots.
    // The original has no ruins among the houses of a living city (re-b17 4.2): the damage shows
    // as the fire models only.
    if let Some(catalog_id) = catalog_of(ctx.art, culture, era) {
        match models.get(catalog_id) {
            None => out.push(pending(0, catalog_id, false)),
            Some(meta) => {
                let hp_frac = if city.max_hp > 0 {
                    city.hp as f32 / city.max_hp as f32
                } else {
                    1.0
                };
                let slow_compose = crate::perf::slow::scope("city.p_compose");
                let chosen =
                    compose::plan(meta, None, &plan.houses, site.centre, hp_frac, &mut rand);
                drop(slow_compose);
                let slow_pieces = crate::perf::slow::scope("city.p_pieces");
                // The widest ground quad lies under the core, its edge under the wall.
                let scale = Mat4::from_scale(Vec3::splat(CITY_SCALE * layout::GROUND));
                let mut pieces = Vec::new();
                if let Some(g) = meta.ground_lot() {
                    let lot = &meta.lots[g];
                    if let Some(mesh) = lot.ground {
                        pieces.push(Piece {
                            part: Some(mesh),
                            matrix: on_surface(
                                at_city * scale * lot.placement,
                                lot_anchor(lot),
                                &surface,
                            ),
                        });
                    }
                }
                let building_scale = Mat4::from_scale(Vec3::splat(layout::BUILDING_SCALE));
                let lot_matrix = |lot: &compose::Lot, c: &compose::Chosen| {
                    let off = c.at * CITY_SCALE;
                    at_city
                        * Mat4::from_translation(Vec3::new(off.x, 0.0, off.y))
                        * Mat4::from_rotation_y(c.yaw)
                        * building_scale
                        * lot.placement
                };
                for c in &chosen {
                    if let Some(mesh) = meta.lots[c.lot].building {
                        pieces.push(Piece {
                            part: Some(mesh),
                            matrix: on_surface(
                                lot_matrix(&meta.lots[c.lot], c),
                                lot_anchor(&meta.lots[c.lot]),
                                &surface,
                            ),
                        });
                    }
                }
                drop(slow_pieces);
                out.push(Placed {
                    slot: 0,
                    model: catalog_id.clone(),
                    pieces,
                    pending: false,
                    animated: false,
                });
                // The smoke and fire over the burning houses of a hurt city, on visible plots only.
                let _slow_damage = crate::perf::slow::scope("city.p_damage");
                if seen {
                    out.extend(damage_effects(
                        models, city, &chosen, seed, at_city, &surface,
                    ));
                    if city.max_hp > 0 {
                        fires.extend(city_fires(city, &chosen, seed, at_city, &surface));
                    }
                }
            }
        }
    }

    // The wall.
    let _slow_wall = crate::perf::slow::scope("city.p_wall_wonders");
    if let Some(wall) = wall_art {
        match models.get(&wall.model) {
            None => out.push(pending(2, &wall.model, false)),
            Some(meta) => out.push(Placed {
                slot: 2,
                model: wall.model.clone(),
                pieces: plan
                    .wall
                    .as_ref()
                    .map(|w| walls::ring(meta, w, site.centre))
                    .unwrap_or_default()
                    .into_iter()
                    .map(|p| Piece {
                        part: Some(p.mesh),
                        matrix: on_surface(at_city * p.matrix, Vec3::ZERO, &surface),
                    })
                    .collect(),
                pending: false,
                animated: false,
            }),
        }
    }

    // The wonders: the origin of the model on the terrain under its point, not on the water plane
    // (`AddWonderData` takes `GetWorldHeight`: the bases of the lighthouses and harbours are made to
    // stand under the water, re-b17 1.3b).
    for (i, (special, at)) in specials.iter().zip(&special_at).enumerate() {
        let n = SPECIAL_SLOT + i as u16;
        let Some(spot) = at else { continue };
        match models.get(&special.model) {
            None => out.push(pending(n, &special.model, true)),
            Some(_) => {
                out.push(Placed {
                    slot: n,
                    model: special.model.clone(),
                    pieces: vec![Piece {
                        part: None,
                        matrix: on_surface(
                            at_city
                                * wonders::matrix(spot.at / CITY_SCALE, special.scale, spot.yaw),
                            Vec3::ZERO,
                            ctx.ground,
                        ),
                    }],
                    pending: false,
                    animated: true,
                });
            }
        }
    }
    Some(out)
}

/// A fingerprint of everything the solve of a layout reads: the site, the wonder boxes, the
/// authored fields and what the ground round the city is for the layout: water, the land band and
/// flat enough, on a grid over the rect. A height that moves without crossing one of those leaves
/// the layout as it is (the models are still put on the new heights, see [`ground_fingerprint`]);
/// the solve is the dearest part of a placing.
fn layout_fingerprint(
    site: &layout::Site<'_>,
    footprints: &[layout::Footprint],
    fields_gen: u64,
) -> u64 {
    const N: i32 = 9;
    let step = layout::RECT_HALF * 1.2 / (N / 2) as f32;
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut bits = 0u64;
    let mut count = 0;
    for j in -(N / 2)..=(N / 2) {
        for i in -(N / 2)..=(N / 2) {
            let p = site.centre + Vec2::new(i as f32, j as f32) * step;
            let class = u64::from(site.is_water(p))
                | u64::from(site.in_land_band(p)) << 1
                | u64::from(site.normal_z(p) >= layout::MIN_NORMAL_Z) << 2;
            bits = bits << 3 | class;
            count += 1;
            if count == 20 {
                h = mix64(h, bits);
                bits = 0;
                count = 0;
            }
        }
    }
    h = mix64(h, bits);
    h = mix64(h, fields_gen);
    let pair = |v: Vec2| u64::from(v.x.to_bits()) << 32 | u64::from(v.y.to_bits());
    h = mix64(h, pair(site.centre));
    h = mix64(h, site.population as u64);
    h = mix64(h, u64::from(site.edges) << 1 | u64::from(site.has_wall));
    h = mix64(
        h,
        site.water_level
            .map_or(u64::MAX, |w| u64::from(w.to_bits())),
    );
    for f in footprints {
        for c in f.corners {
            h = mix64(h, pair(c));
        }
    }
    h
}

/// The models a city has not got yet, as pending entries with no place: what its placing will ask
/// for, found without the layout.
fn wanted_models(
    models: &BTreeMap<LogicalId, ModelMeta>,
    ctx: &B2Context<'_>,
    city: &CityEntity,
) -> Vec<Placed> {
    let culture = ctx
        .owners
        .culture_of(city.owner)
        .unwrap_or(crate::entities::cities::FALLBACK_CULTURE);
    let era = ctx.owners.era_of(city.owner);
    let seed = layout::seed_of((city.owner, city.id));
    let mut out = Vec::new();
    if let Some(id) = catalog_of(ctx.art, culture, era)
        && !models.contains_key(id)
    {
        out.push(pending(0, id, false));
    }
    if let Some(wall) = walls::wall_of(ctx.art, &city.buildings, culture, era)
        && !models.contains_key(&wall.model)
    {
        out.push(pending(2, &wall.model, false));
    }
    let specials = wonders::specials(
        ctx.art,
        &city.buildings,
        &special_types(ctx.art, city),
        culture,
        building_era(ctx.owners, city.owner),
        seed,
    );
    for (i, special) in specials.iter().enumerate() {
        if !models.contains_key(&special.model) {
            out.push(pending(SPECIAL_SLOT + i as u16, &special.model, true));
        }
    }
    out
}

fn pending(slot: u16, model: &LogicalId, animated: bool) -> Placed {
    Placed {
        slot,
        model: model.clone(),
        pieces: Vec::new(),
        pending: true,
        animated,
    }
}

impl RenderSystem for City {
    fn id(&self) -> SystemId {
        SystemId::City
    }

    fn passes(&self) -> &'static [PassSlot] {
        &[
            PassSlot::Pass(PassId::ShadowMap),
            PassSlot::Pass(PassId::Reflection),
            PassSlot::Main(MainSlot::City),
        ]
    }

    fn init(&mut self, _cx: &mut InitContext<'_>) {}

    /// WOB5: the instances of the frame, after the animation stepped (`PrepareServices`).
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
        let (batches, parts) = &cache.2;
        let animated = |id: &LogicalId| anim.model(id).is_some_and(|m| m.animated());
        let Some(b2) = &mut self.b2 else { return };
        let report = b2.draw.build(
            cx.device,
            cx.queue,
            services.bones(),
            &draw::BuildInputs {
                batches,
                parts,
                states: &Default::default(),
                part_states: &Default::default(),
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
    use super::*;
    use crate::entities::units::tests::{ready, snapshot, terrain};
    use crate::entities::{ArtState, EntityPlayer, build_plan};
    use asset_format::{
        BuildingArt, CityArt, CityArtB2, MaterialDesc, MeshArrays, MeshV2, Part, Transform, WallArt,
    };

    fn city(id: i32, owner: i8, x: i32, y: i32) -> CityEntity {
        CityEntity {
            id,
            owner,
            x,
            y,
            name: format!("City {id}"),
            population: 3,
            capital: id == 1,
            hp: 200,
            max_hp: 200,
            ..CityEntity::default()
        }
    }

    fn table() -> ArtTable {
        let mut t = ArtTable::new();
        t.insert_city(
            "EUROPEAN",
            "Ancient",
            CityArt {
                model: LogicalId::new("model:city"),
            },
        );
        t
    }

    #[test]
    fn upsert_of_one_city_does_not_place_the_others() {
        let cities = [city(1, 0, 1, 1), city(2, 0, 2, 2), city(3, 1, 3, 3)];
        let mut s = snapshot(vec![]);
        s.cities = cities.to_vec();
        let (t, art) = (terrain(), ArtState::Ready(table()));
        let ctx = PlanContext {
            terrain: &t,
            art: &art,
            status: &ready,
        };
        let mut system = City::default();
        for c in &cities {
            system.upsert(c.clone());
        }
        assert_eq!(system.refresh(&s.owners(), &ctx), 3);
        let mut plan = EntityPlan::default();
        system.assemble(&mut plan);
        let reference = build_plan(&s, &ctx);
        assert_eq!(plan.batches, reference.batches);
        assert_eq!(plan.labels, reference.labels);

        let mut grown = cities[1].clone();
        grown.population = 4;
        assert!(system.upsert(grown.clone()));
        assert!(!system.upsert(grown));
        assert_eq!(system.refresh(&s.owners(), &ctx), 1);
        assert_eq!(system.counters().placed_total, 4);
        assert!(system.remove((1, 3)));
        assert_eq!(system.refresh(&s.owners(), &ctx), 0);
        assert_eq!(system.len(), 2);
    }

    // ---- WOB5: the models of `assets-b2` ----

    /// A flat quad of half side `half` centred on `(cx, cz)` at `height`.
    fn quad(name: &str, material: u16, cx: f32, cz: f32, half: f32, height: f32) -> MeshV2 {
        let a = MeshArrays {
            positions: vec![
                [cx - half, height, cz - half],
                [cx + half, height, cz - half],
                [cx + half, 0.0, cz + half],
                [cx - half, 0.0, cz + half],
            ],
            normals: vec![[0.0, 1.0, 0.0]; 4],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            joints: vec![],
            weights: vec![],
            indices: vec![0, 1, 2, 0, 2, 3],
        };
        MeshV2::pack(name, 0, material, &a, false, false).unwrap()
    }

    fn materials() -> Vec<MaterialDesc> {
        ["LandmarkShader", "BuildingShader"]
            .map(|package| MaterialDesc {
                package: package.into(),
                ..MaterialDesc::default()
            })
            .into()
    }

    /// A catalog of `n` lots, each a ground quad of 400 and a building of half side `half`; the
    /// vertices carry the opposite of the placement, like the real files.
    fn catalog_blob(n: u16, half: f32) -> ModelBlobV2 {
        let mut meshes = Vec::new();
        let mut parts = Vec::new();
        for i in 0..n {
            let dx = 1000.0 + 500.0 * f32::from(i);
            meshes.push(quad(&format!("ground{i}"), 0, -dx, 0.0, 200.0, 0.0));
            meshes.push(quad(
                &format!("house{i}"),
                1,
                -dx,
                0.0,
                half,
                100.0 + f32::from(i),
            ));
            parts.push(Part {
                name: format!("model{i}"),
                first_mesh: 2 * i,
                mesh_count: 2,
                placement: Transform {
                    translation: [dx, 0.0, 0.0],
                    ..Transform::IDENTITY
                },
            });
        }
        ModelBlobV2 {
            parts,
            meshes,
            materials: materials(),
            ..ModelBlobV2::default()
        }
    }

    fn wall_blob() -> ModelBlobV2 {
        let mut meshes = Vec::new();
        let mut parts = Vec::new();
        for (i, half) in [60.0, 40.0, 45.0].into_iter().enumerate() {
            meshes.push(quad(&format!("wall{i}"), 1, 0.0, 0.0, half, 120.0));
            parts.push(Part {
                name: format!("model{i}"),
                first_mesh: i as u16,
                mesh_count: 1,
                placement: Transform::IDENTITY,
            });
        }
        ModelBlobV2 {
            parts,
            meshes,
            materials: materials(),
            ..ModelBlobV2::default()
        }
    }

    fn art_b2() -> ArtTable {
        let mut t = ArtTable::new();
        for (culture, era) in [
            ("EUROPEAN", "Ancient"),
            ("ASIAN", "Ancient"),
            ("ASIAN", "Industrial"),
        ] {
            t.insert_city_b2(
                culture,
                era,
                CityArtB2 {
                    model: LogicalId::new(format!("md:{culture}_{era}")),
                    parts: 20,
                    lighting: None,
                },
            );
        }
        t.insert_city_b2(
            "RUINS",
            "Ancient",
            CityArtB2 {
                model: LogicalId::new("md:ruins"),
                parts: 12,
                lighting: None,
            },
        );
        t.insert_wall(
            "EUROPEAN",
            "Ancient",
            WallArt {
                model: LogicalId::new("md:walls"),
                parts: 3,
            },
        );
        let building = |tag: &str, model: Option<&str>| BuildingArt {
            art_tag: tag.into(),
            building_types: vec![],
            cultural_variation: false,
            era_variation: false,
            random_variation: false,
            model: model.map(LogicalId::new),
            scale: 0.1,
            variants: vec![],
            note: model.is_none().then(|| "part of the catalog".into()),
        };
        t.insert_building(building("THE PYRAMIDS", Some("md:pyramids")));
        t.insert_building(building(walls::WALLS_TAG, None));
        t
    }

    fn owners(culture: Option<Culture>, era: EraBucket) -> Owners {
        Owners {
            players: vec![EntityPlayer {
                id: 0,
                color: [0; 3],
                team_color: [0; 3],
                culture,
                era: Some(era),
                industrial: false,
                art_techs: 0,
                era_index: None,
            }],
            era,
        }
    }

    fn placed(
        models: &BTreeMap<LogicalId, ModelMeta>,
        art: &ArtTable,
        owners: &Owners,
        c: &CityEntity,
    ) -> Vec<Placed> {
        let ground = |p: Vec2| p.x * 0.01 + 10.0;
        let ctx = B2Context {
            art,
            owners,
            ground: &ground,
            water_level: None,
        };
        place_b2(models, &ctx, (c.owner, c.id), c, true)
    }

    fn test_wall_plan() -> wall_gen::WallPlan {
        let ring: Vec<Vec2> = (0..16)
            .map(|k| {
                let a = k as f32 / 16.0 * std::f32::consts::TAU;
                Vec2::new(a.cos(), a.sin()) * 30.0
            })
            .collect();
        wall_gen::plan(ring)
    }

    fn test_env() -> LayoutEnv<'static> {
        LayoutEnv {
            fields: stand_in(),
            plots: None,
        }
    }

    fn all_models() -> BTreeMap<LogicalId, ModelMeta> {
        let mut m = BTreeMap::new();
        for id in [
            "md:EUROPEAN_Ancient",
            "md:ASIAN_Ancient",
            "md:ASIAN_Industrial",
        ] {
            m.insert(LogicalId::new(id), ModelMeta::of(&catalog_blob(20, 50.0)));
        }
        m.insert(
            LogicalId::new("md:ruins"),
            ModelMeta::of(&catalog_blob(12, 40.0)),
        );
        m.insert(LogicalId::new("md:walls"), ModelMeta::of(&wall_blob()));
        m.insert(
            LogicalId::new("md:pyramids"),
            ModelMeta::of(&ModelBlobV2 {
                bounds: [-100.0, 0.0, -100.0, 100.0, 500.0, 100.0],
                ..ModelBlobV2::default()
            }),
        );
        m
    }

    #[test]
    fn a_model_is_a_lot_per_part_with_the_ground_and_the_building_apart() {
        let meta = ModelMeta::of(&catalog_blob(3, 50.0));
        assert_eq!(meta.lots.len(), 3);
        let lot = &meta.lots[1];
        assert_eq!((lot.ground, lot.building), (Some(2), Some(3)));
        assert!((lot.base_side - 400.0).abs() < 1e-3, "{}", lot.base_side);
        assert!((lot.radius - 50.0).abs() < 1e-3, "{}", lot.radius);
        let split = compose::split_by_mesh(&catalog_blob(3, 50.0));
        assert_eq!(split.parts.len(), 6);
        assert!(
            split
                .meshes
                .iter()
                .enumerate()
                .all(|(i, m)| usize::from(m.part) == i && split.parts[i].first_mesh as usize == i)
        );
    }

    #[test]
    fn a_city_is_the_ground_and_a_building_more_than_its_people() {
        let (models, art) = (all_models(), art_b2());
        let o = owners(Some(Culture::Euro), EraBucket::Ancient);
        let list = placed(&models, &art, &o, &city(1, 0, 4, 4));
        let catalog = list.iter().find(|p| p.slot == 0).unwrap();
        assert_eq!(catalog.model.as_str(), "md:EUROPEAN_Ancient");
        // The ground and the houses of three people.
        assert!(catalog.pieces.len() > 10, "{}", catalog.pieces.len());
        let c = cell_center(4, 4);
        let g = &catalog.pieces[0];
        // The vertices of the first lot are centred on -1000.
        let at = g.matrix.transform_point3(Vec3::new(-1000.0, 0.0, 0.0));
        // The middle of the ground quad is on the middle of the hex, on the ground.
        assert!(
            (at.x - c.x).abs() < 1.0 && (at.z - c.y).abs() < 1.0,
            "{at:?}"
        );
        assert!((at.y - (c.x * 0.01 + 10.0)).abs() < 0.5);
        let mut bigger = city(1, 0, 4, 4);
        bigger.population = 8;
        let more = placed(&models, &art, &o, &bigger);
        // The layout is made again: more houses (the stand-in fields grow with the population).
        assert!(more[0].pieces.len() > catalog.pieces.len());
        assert_eq!(more[0].pieces[0], catalog.pieces[0], "the same ground");
    }

    #[test]
    fn the_culture_and_the_era_of_the_owner_choose_the_catalog() {
        let (models, art) = (all_models(), art_b2());
        let c = city(1, 0, 4, 4);
        let model = |culture, era| {
            let o = owners(culture, era);
            placed(&models, &art, &o, &c)[0].model.as_str().to_owned()
        };
        assert_eq!(
            model(Some(Culture::Asia), EraBucket::Ancient),
            "md:ASIAN_Ancient"
        );
        assert_eq!(
            model(Some(Culture::Asia), EraBucket::Industrial),
            "md:ASIAN_Industrial"
        );
        // The Renaissance has none: the older era of the culture.
        assert_eq!(
            model(Some(Culture::Asia), EraBucket::Renaissance),
            "md:ASIAN_Ancient"
        );
        // A culture with none: Europe.
        assert_eq!(
            model(Some(Culture::Afri), EraBucket::Ancient),
            "md:EUROPEAN_Ancient"
        );
        assert_eq!(model(None, EraBucket::Modern), "md:EUROPEAN_Ancient");
    }

    #[test]
    fn walls_and_wonders_come_with_the_buildings_of_the_city() {
        let (models, art) = (all_models(), art_b2());
        let o = owners(Some(Culture::Euro), EraBucket::Ancient);
        let mut c = city(1, 0, 4, 4);
        assert!(placed(&models, &art, &o, &c).iter().all(|p| p.slot == 0));
        c.buildings = vec!["THE PYRAMIDS".into(), walls::WALLS_TAG.into()];
        let list = placed(&models, &art, &o, &c);
        let wall = list.iter().find(|p| p.slot == 2).unwrap();
        assert_eq!(wall.model.as_str(), "md:walls");
        // Small pieces: the stretches end to end round the houses.
        assert!(wall.pieces.len() > 20, "{}", wall.pieces.len());
        let wonder = list.iter().find(|p| p.slot == SPECIAL_SLOT).unwrap();
        assert_eq!(wonder.model.as_str(), "md:pyramids");
        assert!(wonder.animated && wonder.pieces[0].part.is_none());
    }

    #[test]
    fn the_wonder_a_city_is_building_stands_half_built_and_a_plain_building_does_not() {
        use asset_format::{BuildingArt, BuildingTypeArt, BuildingVariant};
        let mut t = ArtTable::new();
        let art = |tag: &str, model: Option<&str>, variants: Vec<BuildingVariant>| BuildingArt {
            art_tag: tag.into(),
            building_types: vec![],
            cultural_variation: false,
            era_variation: false,
            random_variation: false,
            model: model.map(LogicalId::new),
            scale: 0.05,
            variants,
            note: None,
        };
        t.insert_building(art("THE PYRAMIDS", Some("md:pyramids"), vec![]));
        t.insert_building(art(
            "HARBOR",
            None,
            vec![BuildingVariant {
                kind: "era".into(),
                key: "ANCIENT".into(),
                art_type: "ANCIENT HARBOR".into(),
                model: LogicalId::new("md:harbor_anc"),
                scale: 0.05,
            }],
        ));
        for (ty, tag, d) in [
            ("BUILDING_PYRAMID", "THE PYRAMIDS", 4),
            ("BUILDING_HARBOR", "HARBOR", 16),
        ] {
            t.insert_building_type(BuildingTypeArt {
                building_type: ty.into(),
                art_tag: tag.into(),
                display_position: d,
            });
        }
        let mut c = city(1, 0, 4, 4);
        c.building_types = vec!["BUILDING_HARBOR".into()];
        assert_eq!(special_types(&t, &c), ["BUILDING_HARBOR"]);
        c.production_building = Some("BUILDING_PYRAMID".into());
        assert_eq!(
            special_types(&t, &c),
            ["BUILDING_HARBOR", "BUILDING_PYRAMID"]
        );
        // A harbour in the making is not drawn; a built wonder is not drawn twice.
        c.building_types = vec![];
        c.production_building = Some("BUILDING_HARBOR".into());
        assert!(special_types(&t, &c).is_empty());
        c.building_types = vec!["BUILDING_PYRAMID".into()];
        c.production_building = Some("BUILDING_PYRAMID".into());
        assert_eq!(special_types(&t, &c), ["BUILDING_PYRAMID"]);
    }

    #[test]
    fn a_medieval_owner_has_the_ancient_city_but_the_medieval_harbor() {
        let mut o = owners(Some(Culture::Euro), EraBucket::Ancient);
        o.players[0].era_index = Some(2);
        assert_eq!(o.era_of(0), EraBucket::Ancient);
        assert_eq!(building_era(&o, 0), EraBucket::Renaissance);
        o.players[0].era_index = Some(1);
        assert_eq!(building_era(&o, 0), EraBucket::Ancient);
        o.players[0].era_index = Some(6);
        assert_eq!(building_era(&o, 0), EraBucket::Modern);
        o.players[0].era_index = None;
        assert_eq!(building_era(&o, 0), EraBucket::Ancient);
    }

    #[test]
    fn a_city_being_razed_or_hurt_keeps_its_houses_wall_and_wonders_and_shows_no_ruins() {
        let (models, art) = (all_models(), art_b2());
        let o = owners(Some(Culture::Euro), EraBucket::Ancient);
        let mut c = city(1, 0, 4, 4);
        c.population = 10;
        c.buildings = vec![walls::WALLS_TAG.into(), "THE PYRAMIDS".into()];
        let whole = placed(&models, &art, &o, &c);
        c.razing = true;
        let razing = placed(&models, &art, &o, &c);
        assert_eq!(razing[0].model.as_str(), "md:EUROPEAN_Ancient");
        assert!(razing.iter().any(|p| p.slot == 2), "the wall stands");
        assert!(razing.iter().any(|p| p.slot == SPECIAL_SLOT), "the wonder");
        assert_eq!(
            razing.iter().map(|p| p.pieces.len()).collect::<Vec<_>>(),
            whole.iter().map(|p| p.pieces.len()).collect::<Vec<_>>()
        );
        // A hurt city: its houses, no ruins among them.
        c.razing = false;
        c.hp = 40;
        let hurt = placed(&models, &art, &o, &c);
        assert!(
            hurt.iter()
                .all(|p| p.slot != 1 && p.model.as_str() != "md:ruins")
        );
        assert_eq!(hurt[0].pieces.len(), whole[0].pieces.len());
    }

    #[test]
    fn a_hurt_visible_city_gets_the_damage_models_and_the_others_get_none() {
        let (mut models, art) = (all_models(), art_b2());
        let o = owners(Some(Culture::Euro), EraBucket::Ancient);
        let fx = |p: &Placed| p.slot >= damage::DAMAGE_SLOT;
        let mut c = city(1, 0, 4, 4);
        c.population = 8;
        // Whole: nothing.
        assert!(!placed(&models, &art, &o, &c).iter().any(fx));
        // Hurt (10 points: level 2), the models not there yet: pending, and wanted.
        c.hp = 190;
        let list = placed(&models, &art, &o, &c);
        assert!(list.iter().filter(|p| fx(p)).all(|p| p.pending));
        assert!(list.iter().any(fx));
        for id in damage::MODELS {
            models.insert(LogicalId::new(id), ModelMeta::default());
        }
        let list = placed(&models, &art, &o, &c);
        let effects: Vec<&Placed> = list.iter().filter(|p| fx(p)).collect();
        assert!(!effects.is_empty() && effects.iter().all(|p| !p.pending));
        // 10 points lost: level 2, so the 4 houses of burn ranks 0 and 1 smoke.
        let n: usize = effects.iter().map(|p| p.pieces.len()).sum();
        assert_eq!(n, 4);
        assert!(
            effects
                .iter()
                .all(|p| p.pieces.iter().all(|q| q.part.is_none()))
        );
        // A city nobody sees shows none.
        let ground = |_: Vec2| 0.0;
        let ctx = B2Context {
            art: &art,
            owners: &o,
            ground: &ground,
            water_level: None,
        };
        let unseen = place_b2(&models, &ctx, (0, 1), &c, false);
        assert!(!unseen.iter().any(fx));
    }

    #[test]
    fn a_layout_is_solved_once_and_again_only_when_its_inputs_change() {
        let (models, art) = (all_models(), art_b2());
        let o = owners(Some(Culture::Euro), EraBucket::Ancient);
        let flat = |_: Vec2| 12.0;
        let hill = |p: Vec2| 12.0 + p.x * 0.001;
        let mut c = city(1, 0, 4, 4);
        c.population = 8;
        let mut cache = BTreeMap::new();
        let mut jobs = BTreeMap::new();
        let mut place = |ground: &dyn Fn(Vec2) -> f32, c: &CityEntity, budget: u32| {
            let ctx = B2Context {
                art: &art,
                owners: &o,
                ground,
                water_level: None,
            };
            let mut book = LayoutBook {
                cache: &mut cache,
                jobs: &mut jobs,
                fields_gen: 0,
                budget,
                built: 0,
                ticks: u32::MAX,
            };
            let out = place_b2_fires(
                &models,
                &ctx,
                test_env(),
                &mut book,
                (0, 1),
                c,
                false,
                &mut Vec::new(),
                &mut Vec::new(),
            );
            (out, book.built)
        };
        // No budget and nothing cached: not placed.
        assert!(place(&flat, &c, 0).0.is_none());
        let (first, built) = place(&flat, &c, 1);
        assert_eq!(built, 1);
        // The same inputs (a hit point changes nothing): the cache, whatever the budget.
        c.hp = 150;
        let (again, built) = place(&flat, &c, 0);
        assert_eq!(built, 0);
        assert!(first.is_some() && again.is_some());
        // Heights that move without leaving the land band or the slope limit: the layout stays.
        assert!(place(&hill, &c, 0).0.is_some());
        // Ground out of the land band (high mountains): solved again, and not without budget.
        let peak = |_: Vec2| 90.0;
        assert!(place(&peak, &c, 0).0.is_none());
        assert_eq!(place(&peak, &c, 1).1, 1);
        // Another size: solved again.
        c.population = 1;
        assert_eq!(place(&peak, &c, 1).1, 1);
    }

    #[test]
    fn the_fires_follow_the_damage_models_and_are_reported_once_per_change() {
        let (models, art) = (all_models(), art_b2());
        let o = owners(Some(Culture::Euro), EraBucket::Ancient);
        // Land in the band of the houses (raw 15300).
        let ground = |_: Vec2| 12.0;
        let ctx = B2Context {
            art: &art,
            owners: &o,
            ground: &ground,
            water_level: None,
        };
        let mut c = city(1, 0, 4, 4);
        c.population = 8;
        let fires = |c: &CityEntity, seen: bool| {
            let mut out = Vec::new();
            let mut cache = BTreeMap::new();
            let mut jobs = BTreeMap::new();
            let mut book = LayoutBook {
                cache: &mut cache,
                jobs: &mut jobs,
                fields_gen: 0,
                budget: u32::MAX,
                built: 0,
                ticks: u32::MAX,
            };
            place_b2_fires(
                &models,
                &ctx,
                test_env(),
                &mut book,
                (0, 1),
                c,
                seen,
                &mut out,
                &mut Vec::new(),
            );
            out
        };
        // Level 0 and 1 (the first state is a model only): no fire; a city nobody sees: none.
        assert!(fires(&c, true).is_empty());
        c.hp = 193;
        assert!(fires(&c, true).is_empty());
        c.hp = 185;
        assert!(fires(&c, false).is_empty());
        // Level 2: the houses of rank 0 burn (the first three of the burn order), at the place of
        // their model.
        let l2 = fires(&c, true);
        assert_eq!(l2.len(), 3);
        assert!(l2.iter().all(|f| f.name.ends_with("SECOND_01")));
        assert_eq!(l2, fires(&c, true), "the same every time");
        // Level 3: rank 0 third, rank 1 second (rank 2 is the first state, a model only).
        c.hp = 20;
        let l3 = fires(&c, true);
        assert_eq!(l3.len(), 4);
        assert_eq!(
            l3.iter().filter(|f| f.name.ends_with("THIRD_01")).count(),
            3
        );
        for f in &l2 {
            assert!(l3.iter().any(|g| g.index == f.index && g.at == f.at));
        }

        // The system reports a change once, and the removal of the city as an empty list.
        let mut system = City {
            models: all_models(),
            loading: true,
            ..City::default()
        };
        let mut c = city(1, 0, 4, 4);
        c.population = 8;
        c.hp = 190;
        system.upsert(c.clone());
        system.set_seen(|_, _| true);
        system.refresh_b2(&ctx);
        let changes = system.take_fire_changes();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].1.len(), 3);
        assert!(system.take_fire_changes().is_empty());
        system.upsert({
            let mut h = c.clone();
            h.hp = 200;
            h
        });
        system.refresh_b2(&ctx);
        assert_eq!(system.take_fire_changes(), [((0, 1), Vec::new())]);
        system.upsert(c);
        system.refresh_b2(&ctx);
        assert!(system.remove((0, 1)));
        assert!(system.take_fire_changes().is_empty());
    }

    #[test]
    fn the_fog_of_the_plot_changes_the_effects_of_the_city() {
        let (art, o) = (art_b2(), owners(Some(Culture::Euro), EraBucket::Ancient));
        let ground = |_: Vec2| 0.0;
        let ctx = B2Context {
            art: &art,
            owners: &o,
            ground: &ground,
            water_level: None,
        };
        let mut system = City {
            loading: true,
            ..City::default()
        };
        system.upsert(city(1, 0, 4, 4));
        system.refresh_b2(&ctx);
        assert!(system.dirty_b2.is_empty());
        // Seen: the city is placed again; the same answer again: nothing to do.
        system.set_seen(|x, y| (x, y) == (4, 4));
        assert_eq!(system.dirty_b2.len(), 1);
        system.refresh_b2(&ctx);
        system.set_seen(|x, y| (x, y) == (4, 4));
        assert!(system.dirty_b2.is_empty());
        system.set_seen(|_, _| false);
        assert_eq!(system.dirty_b2.len(), 1);
    }

    #[test]
    fn a_model_that_has_not_come_is_wanted_and_the_city_is_placed_when_it_does() {
        let art = art_b2();
        let o = owners(Some(Culture::Euro), EraBucket::Ancient);
        let c = city(1, 0, 4, 4);
        let mut system = City {
            loading: true,
            ..City::default()
        };
        system.upsert(c);
        let ground = |_: Vec2| 0.0;
        let ctx = B2Context {
            art: &art,
            owners: &o,
            ground: &ground,
            water_level: None,
        };
        assert_eq!(system.refresh_b2(&ctx), 1);
        let id = LogicalId::new("md:EUROPEAN_Ancient");
        assert!(system.placed_at(&(0, 1))[0].pending);
        assert!(system.wants_model(&id));
        system.register_model(id.clone(), &catalog_blob(20, 50.0));
        assert_eq!(system.refresh_b2(&ctx), 1);
        assert!(!system.placed_at(&(0, 1))[0].pending);
        assert!(!system.placed_at(&(0, 1))[0].pieces.is_empty());
        assert!(!system.wants_model(&id));
        // A city that goes leaves nothing.
        assert!(system.remove((0, 1)));
        assert!(system.placed_at(&(0, 1)).is_empty());
    }

    #[test]
    fn every_house_of_a_city_clears_a_square_of_ten_of_trees_and_gives_it_back_when_it_goes() {
        let art = art_b2();
        let o = owners(Some(Culture::Euro), EraBucket::Ancient);
        let mut system = City {
            models: all_models(),
            loading: true,
            ..City::default()
        };
        let mut c = city(1, 0, 4, 4);
        c.population = 8;
        system.upsert(c);
        // Land in the band of the houses.
        let ground = |_: Vec2| 12.0;
        let ctx = B2Context {
            art: &art,
            owners: &o,
            ground: &ground,
            water_level: None,
        };
        system.refresh_b2(&ctx);
        let houses = system.layouts[&(0, 1)].plan.houses.clone();
        assert!(!houses.is_empty());
        let changes = system.take_cut_changes();
        assert_eq!(changes.len(), 1);
        let (out, put) = &changes[0];
        assert!(out.is_empty());
        assert_eq!(put.len(), houses.len() * 2);
        // The two triangles of the first house are the square of 10 round it.
        let p = layout::from_orig(houses[0].pos);
        let pts: Vec<Vec2> = put[..2].iter().flatten().copied().collect();
        let lo = pts.iter().fold(Vec2::MAX, |a, b| a.min(*b));
        let hi = pts.iter().fold(Vec2::MIN, |a, b| a.max(*b));
        assert!((lo - (p - Vec2::splat(5.0))).length() < 1e-4);
        assert!((hi - (p + Vec2::splat(5.0))).length() < 1e-4);
        // Placed again the same: no change; gone: the ground is given back.
        system.invalidate_all();
        system.refresh_b2(&ctx);
        assert!(system.take_cut_changes().is_empty());
        assert!(system.remove((0, 1)));
        assert_eq!(system.take_cut_changes(), [(put.clone(), Vec::new())]);
    }

    #[test]
    fn a_city_waiting_for_its_layout_asks_for_its_models_at_once() {
        let art = art_b2();
        let o = owners(Some(Culture::Euro), EraBucket::Ancient);
        let mut system = City::default();
        system.set_loading(true);
        let n = (COLD_LAYOUTS_PER_FRAME + 5) as i32;
        for i in 0..n {
            system.upsert(city(i + 1, 0, 4 + i, 4));
        }
        let ground = |_: Vec2| 0.0;
        let ctx = B2Context {
            art: &art,
            owners: &o,
            ground: &ground,
            water_level: None,
        };
        system.refresh_b2(&ctx);
        let (built, deferred, _) = system.layout_counters();
        assert_eq!(built, u64::from(COLD_LAYOUTS_PER_FRAME));
        assert_eq!(deferred, 5);
        // The five without a layout hold their model as pending, so the loader gets it now.
        let id = LogicalId::new("md:EUROPEAN_Ancient");
        assert!(system.wants_model(&id));
        for i in 0..n {
            assert!(
                system
                    .placed_at(&(0, i))
                    .iter()
                    .all(|p| p.pending || !p.pieces.is_empty())
            );
            assert!(!system.placed_at(&(0, i + 1)).is_empty(), "city {i}");
        }
    }

    #[test]
    fn the_ground_quad_of_a_city_is_a_decal_of_the_terrain_under_the_city() {
        use asset_format::{ParamSet, TexKind, TexParam, TexSource};
        let art = art_b2();
        let o = owners(Some(Culture::Euro), EraBucket::Ancient);
        let ground = |_: Vec2| 0.0;
        let ctx = B2Context {
            art: &art,
            owners: &o,
            ground: &ground,
            water_level: None,
        };
        // The catalog with a texture on its ground material, as the real ones.
        let mut blob = catalog_blob(20, 50.0);
        blob.materials[0].sets = vec![ParamSet {
            blocks: vec![],
            constants: vec![],
            textures: vec![TexParam {
                name: "BaseTextureMap".into(),
                kind: TexKind::Tex2D,
                src: TexSource::File(LogicalId::new("tx:city_ground.dds")),
                semantic: 0,
                reg: 0,
                usage: 0,
                flags: 0,
                active: true,
            }],
        }];
        let mut system = City {
            loading: true,
            ..City::default()
        };
        system.upsert(city(1, 0, 4, 4));
        let id = LogicalId::new("md:EUROPEAN_Ancient");
        system.register_model(id.clone(), &blob);
        system.refresh_b2(&ctx);
        assert_eq!(system.grounds.get(&id).map(Vec::len), Some(20));
        let width = 10;
        let decals = system.ground_decals(width, None);
        let cell = (4 * width + 4) as u32;
        assert_eq!(decals.keys().copied().collect::<Vec<_>>(), [cell]);
        // One quad, the widest, under the middle of the city.
        let d = &decals[&cell];
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].albedo, LogicalId::new("tx:city_ground.dds"));
        let mid = d[0].verts.iter().map(|v| v.pos).sum::<Vec2>() / d[0].verts.len() as f32;
        let at = cell_center(4, 4);
        assert!(mid.distance(at) < 1.0, "{mid} {at}");
        // Without the draw of `assets-b2` the terrain holds none; a city that goes takes it away.
        assert!(system.take_ground_update(width).is_none());
        system.remove((0, 1));
        assert!(system.ground_decals(width, None).is_empty());
    }

    // ---- the real pack: every catalog, wall and building of the table (AC-R3.2, AC-R3.3) ----

    /// The art table and the lots of every model of a pack directory (`CIV5_ASSETS_B2`, the output
    /// of `civ5-assets b2-*`); `None` without it.
    fn real_pack() -> Option<(Vec<ArtTable>, BTreeMap<LogicalId, ModelMeta>)> {
        let dir = std::env::var_os("CIV5_ASSETS_B2")?;
        let mut tables = Vec::new();
        let mut models = BTreeMap::new();
        for e in std::fs::read_dir(dir).ok()?.flatten() {
            let Ok(bytes) = std::fs::read(e.path()) else {
                continue;
            };
            match bytes.get(..4) {
                Some(b"C5AT") => {
                    if let Ok(t) = ArtTable::decode(&bytes) {
                        tables.push(t);
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
        (!tables.is_empty()).then_some((tables, models))
    }

    #[test]
    fn every_catalog_wall_and_building_of_the_real_table_has_a_model_and_a_place() {
        let Some((tables, models)) = real_pack() else {
            eprintln!("skipped: CIV5_ASSETS_B2 is not set");
            return;
        };
        let mut problems = Vec::new();
        let mut catalogs = 0;
        let mut walls_seen = 0;
        for (key, rec) in tables.iter().flat_map(ArtTable::iter) {
            match rec {
                asset_format::ArtRecord::CityB2(c) => {
                    catalogs += 1;
                    let Some(meta) = models.get(&c.model) else {
                        problems.push(format!("{key}: no blob for {}", c.model.as_str()));
                        continue;
                    };
                    if meta.ground_lot().is_none() {
                        problems.push(format!("{key}: no ground quad"));
                    }
                    // Every height class has a model.
                    if compose::height_classes(meta).iter().any(Vec::is_empty) {
                        problems.push(format!("{key}: a height class without a model"));
                    }
                }
                asset_format::ArtRecord::Wall(w) => {
                    walls_seen += 1;
                    match models.get(&w.model) {
                        None => problems.push(format!("{key}: no blob for {}", w.model.as_str())),
                        Some(meta)
                            if walls::ring(meta, &test_wall_plan(), Vec2::ZERO).is_empty() =>
                        {
                            problems.push(format!("{key}: no ring"));
                        }
                        Some(_) => {}
                    }
                }
                asset_format::ArtRecord::Building(b) => {
                    for culture in [Culture::Euro, Culture::Asia, Culture::Afri] {
                        for era in [EraBucket::Ancient, EraBucket::Modern] {
                            if let Some((m, _)) = wonders::choose(b, culture, era, 4)
                                && !models.contains_key(m)
                            {
                                problems.push(format!("{key}: no blob for {}", m.as_str()));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        problems.sort();
        problems.dedup();
        assert!(
            catalogs >= 20 && walls_seen >= 20,
            "{catalogs} {walls_seen}"
        );
        assert!(
            problems.is_empty(),
            "{} problems:
{}",
            problems.len(),
            problems.join(
                "
"
            )
        );
    }

    #[test]
    fn the_banner_stays_when_the_models_of_assets_b2_draw_the_city() {
        let c = city(1, 0, 1, 1);
        let mut s = snapshot(vec![]);
        s.cities = vec![c.clone()];
        let (t, art) = (terrain(), ArtState::Ready(table()));
        let ctx = PlanContext {
            terrain: &t,
            art: &art,
            status: &ready,
        };
        let p = place_city(&c, &s.owners(), &ctx, false);
        assert!(p.label.is_some());
        assert!(p.items.is_empty() && p.wanted.is_empty() && p.deps.is_empty());
    }
}
