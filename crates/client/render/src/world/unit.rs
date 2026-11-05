//! `Unit` subsystem (world-objects WOB0): the units by key `(owner, id)`,
//! where each stands and what it draws.
//!
//! The record of a unit and the instances placed for it live here; B1's static pipeline still
//! draws them (`frame::b1_systems::B1Unit`, which `world-objects` replaces with WOB1/WOB2 as the
//! shaders of the families arrive). `upsert` and `remove` place again only what they touch: the
//! unit, and the other units of the two cells it leaves and enters (a military and a civilian
//! unit on one cell stand apart, so a mate depends on it). `invalidate_all` is for a new ground,
//! art or owners.
//!
//! WOB2 is the second placement beside it, for the models of `assets-b2` (`refresh_b2`): the art of
//! the owner's culture and era (`art`), the figures on the cell turned by `facing`, at the height of
//! the surface (`place`), and the animation of every figure through `AnimService` (`sync_anim`,
//! `frame_instances`). A move is a jump (WO-D6): the next placement has the new cell and nothing
//! else changes, the animation goes on because its key is the key of the unit and the figure.
//! There is no marker in it (WO-D15): a unit whose record or models are not there draws nothing and
//! writes a line.
//!
//! The draw of that path is [`draw::UnitDraw`]. It runs when the renderer has the art of
//! `assets-b2` (`tb:art-b2`) and the light data (`b2_attach`, `set_b2_active`): the renderer calls
//! [`Unit::b2_frame`] before the plan, and the system's own `prepare` and `record` do the rest. Until
//! then B1's adapter (`frame::b1_systems::B1Unit`) draws the units as it always did, the same as
//! the terrain does.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use asset_format::{ArtTable, LogicalId};
use glam::{Vec2, Vec3};

use crate::anim::fx::FxHashMap;
use crate::anim::{AnimHandle, AnimService, AnimState, AnimTarget, BoneRef};
use crate::entities::formation;
use crate::entities::units::{CellKinds, Slot, UnitPlaced, is_shared, place_unit};
use crate::entities::{EntityPlan, EraBucket, Owners, PlanContext, UnitEntity, UnitKey};
use crate::frame::{
    DrawContext, InitContext, MainSlot, MaskContext, PassId, PassSlot, PrepareContext,
    RenderSystem, SystemId,
};
use crate::world::objects::input::{Keyed, ObjectCounters};
use crate::world::objects::instance::ObjectInstance;

pub mod art;
pub mod cull;
pub mod draw;
pub mod place;

use place::{Placement, UnitVisExtra};

/// The units. Draws through B1's adapter; see the module text.
#[derive(Default)]
pub struct Unit {
    records: Keyed<UnitKey, UnitEntity>,
    /// Keys of the units on each cell.
    by_cell: HashMap<(i32, i32), BTreeSet<UnitKey>>,
    placed: BTreeMap<UnitKey, UnitPlaced>,
    counters: ObjectCounters,
    /// WOB2: what the view says of a unit beyond its record.
    extras: BTreeMap<UnitKey, UnitVisExtra>,
    /// WOB2: the keys to place again for the models of `assets-b2` (their own set, so that B1's
    /// refresh and this one do not take each other's work).
    dirty_b2: BTreeSet<UnitKey>,
    figures: BTreeMap<UnitKey, Vec<Placement>>,
    /// The figures as the frame draws them, parallel to `figures`: the matrix and the model slot
    /// are made when the unit is placed, so a frame only patches the colour and the pose.
    figs: BTreeMap<UnitKey, Vec<Fig>>,
    /// The model names of the figures as small numbers, with how many figures use each.
    models: Interner,
    /// The units placed since `sync_anim` last ran: it looks at these and no others.
    anim_dirty: BTreeSet<UnitKey>,
    /// `AnimService::models_revision` at the last `sync_anim`; a new one asks every figure again
    /// (a model that arrived lets the figures that wait for it attach).
    synced_revision: Option<u64>,
    /// The instance lists of the frame, kept from frame to frame so that they are not allocated.
    bufs: FrameBufs,
    /// The animation of each figure that has one, and the state it was last given.
    handles: BTreeMap<(UnitKey, u8), (AnimHandle, AnimState, LogicalId)>,
    /// Handles of units a `clear` dropped; the next `sync_anim` (or `release_handles`) gives them back.
    stale_handles: Vec<AnimHandle>,
    placed_b2_last: u32,
    placed_b2_total: u64,
    /// The draw of the models of `assets-b2`; made by `b2_attach`.
    b2: Option<Box<B2State>>,
    /// Instances of models that are not units' (the pieces of the path, WOV6) and the models they
    /// need: drawn in the same calls as the figures.
    extra_batches: BTreeMap<LogicalId, Vec<ObjectInstance>>,
    extra_wanted: Vec<LogicalId>,
}

/// A figure near the listener and the clip it plays: what the sound rows of its timed triggers need (re-b15 ITEM 4).
#[derive(Debug, Clone, PartialEq)]
pub struct FigureClip {
    pub unit: UnitKey,
    /// The number of the figure in the full squad.
    pub index: u8,
    pub model: LogicalId,
    /// In our basis (X east, Y up, Z south).
    pub position: Vec3,
    /// The clip's `ec` and the time in it; `None` for a figure without an animation.
    pub clip: Option<(u32, f32)>,
    /// The living figures of its unit (the tier of the sound rows).
    pub members: u8,
}

/// One figure ready to draw (see `Unit::figs`).
#[derive(Clone, Copy)]
struct Fig {
    handle: Option<AnimHandle>,
    /// The matrix and the alpha; the team colour and the pose are put in each frame.
    inst: ObjectInstance,
    /// Slot of the model in `Interner`.
    model: u32,
    state: u8,
}

/// Model names as slots: a figure carries the number, the name is looked up once, at placement.
#[derive(Default)]
struct Interner {
    ids: Vec<LogicalId>,
    slots: FxHashMap<LogicalId, u32>,
    uses: Vec<u32>,
}

impl Interner {
    fn intern(&mut self, id: &LogicalId) -> u32 {
        if let Some(&s) = self.slots.get(id) {
            return s;
        }
        let s = self.ids.len() as u32;
        self.ids.push(id.clone());
        self.uses.push(0);
        self.slots.insert(id.clone(), s);
        s
    }
}

/// The lists of the frame: by model slot, and the damaged ships by (slot, state).
#[derive(Default)]
struct FrameBufs {
    by_slot: Vec<Vec<ObjectInstance>>,
    states: BTreeMap<(u32, u8), Vec<ObjectInstance>>,
}

/// The time a frame gives to placing the units that changed.
const PLACE_BUDGET_MS: f64 = 3.0;

/// The draw of WOB2 and what `prepare` needs of the frame that `b2_frame` left.
struct B2State {
    draw: draw::UnitDraw,
    /// The units draw through it (the art of `assets-b2` is there); B1's draw is off.
    active: bool,
    owners: Owners,
    shifts: Vec<f32>,
    eye: Vec3,
}

/// What `b2_frame` reads of the frame: the art, the ground and the data of the passes.
pub struct B2Frame<'a> {
    pub art: &'a ArtTable,
    pub owners: &'a Owners,
    /// The height of the land at a point (`TerrainSampler::world_height`).
    pub ground: &'a dyn Fn(Vec2) -> f32,
    pub water_level: Option<f32>,
    pub lights: &'a crate::lighting::LightingEnv,
    /// The frame constants of the main view.
    pub frame_globals: &'a wgpu::Buffer,
    /// The frame constants of the reflection camera.
    pub reflection_globals: &'a wgpu::Buffer,
    /// The fog mask of the frame, if the pass of it runs.
    pub fow: Option<draw::FowSource<'a>>,
    /// The X shifts of the map copies that reach the screen.
    pub shifts: &'a [f32],
    pub eye: Vec3,
}

/// What the placement for the models of `assets-b2` reads besides the records.
pub struct B2Context<'a> {
    pub art: &'a ArtTable,
    pub owners: &'a Owners,
    /// The height of the land at a point (`TerrainSampler::world_height`).
    pub ground: &'a dyn Fn(Vec2) -> f32,
    /// `water_level()` of `world-water`; `None` before the field is there.
    pub water_level: Option<f32>,
}

/// The era of an owner as the index of the era variants (`art::ERA_ORDER`).
// TODO(RW-14): B1 knows two buckets; the owner's real era comes with `PlayerView.era`.
fn era_index(era: EraBucket) -> art::EraIndex {
    match era {
        EraBucket::Ancient => 0,
        // The later eras keep the Renaissance variants, as before the four buckets.
        EraBucket::Renaissance | EraBucket::Industrial | EraBucket::Modern => 3,
    }
}

impl Unit {
    /// The key of `u`.
    pub fn key_of(u: &UnitEntity) -> UnitKey {
        (u.owner, u.id)
    }

    /// Marks a unit to be placed again, for B1's placement and for the models of `assets-b2`.
    fn touch(&mut self, key: UnitKey) {
        self.records.touch(key);
        if self.records.get(&key).is_some() {
            self.dirty_b2.insert(key);
        }
    }

    /// Adds or replaces a unit. True if the record changed.
    pub fn upsert(&mut self, u: UnitEntity) -> bool {
        let key = Self::key_of(&u);
        let new_cell = (u.x, u.y);
        let (old, changed) = self.records.upsert(key, u);
        if !changed {
            return false;
        }
        self.dirty_b2.insert(key);
        if let Some(old) = old {
            let old_cell = (old.x, old.y);
            if old_cell != new_cell {
                self.leave_cell(old_cell, key);
            }
        }
        self.by_cell.entry(new_cell).or_default().insert(key);
        self.touch_mates(new_cell);
        true
    }

    /// Drops a unit. True if there was one.
    pub fn remove(&mut self, key: UnitKey) -> bool {
        let Some(old) = self.records.remove(&key) else {
            return false;
        };
        self.placed.remove(&key);
        self.extras.remove(&key);
        self.dirty_b2.insert(key);
        self.leave_cell((old.x, old.y), key);
        true
    }

    /// Drops every unit.
    pub fn clear(&mut self) {
        let gone: Vec<UnitKey> = self.records.keys().copied().collect();
        self.records.clear();
        self.by_cell.clear();
        self.placed.clear();
        self.extras.clear();
        // The figures, their model counts and animation handles belong to the game that is gone: a unit of the next
        // game with the same key must not inherit the skeleton of another model.
        self.figures.clear();
        self.figs.clear();
        self.models.uses.iter_mut().for_each(|n| *n = 0);
        self.anim_dirty.clear();
        self.stale_handles.extend(
            std::mem::take(&mut self.handles)
                .into_values()
                .map(|(h, _, _)| h),
        );
        self.dirty_b2.extend(gone);
    }

    /// Gives back the animation handles of the units a `clear` dropped (and of any unit left).
    pub fn release_handles(&mut self, anim: &mut AnimService) {
        for h in self.stale_handles.drain(..) {
            anim.release(h);
        }
    }

    /// The animation handles the units hold, and those waiting to be given back (tests).
    pub fn handle_counts(&self) -> (usize, usize) {
        (self.handles.len(), self.stale_handles.len())
    }

    fn leave_cell(&mut self, cell: (i32, i32), key: UnitKey) {
        if let Some(set) = self.by_cell.get_mut(&cell) {
            set.remove(&key);
            if set.is_empty() {
                self.by_cell.remove(&cell);
            }
        }
        self.touch_mates(cell);
    }

    /// The units on `cell` stand apart when both kinds are there: their placement depends on it.
    fn touch_mates(&mut self, cell: (i32, i32)) {
        let Some(set) = self.by_cell.get(&cell) else {
            return;
        };
        for key in set.iter().copied().collect::<Vec<_>>() {
            self.touch(key);
        }
    }

    /// Every unit is placed again at the next refresh (new ground, art or owners).
    pub fn invalidate_all(&mut self) {
        self.records.touch_all();
        self.dirty_b2.extend(self.records.keys().copied());
    }

    /// The terrain built these cells (`(x, y)` of a hex) with their final heights: the units on them and next to
    /// them are placed again on the new ground. Without it a unit placed before the pages of its hill came stayed
    /// at the height of the flat stand-in, sunk into the hill (live report 2026-10-30).
    pub fn heights_dirty(&mut self, cells: &[(i32, i32)]) {
        let hit: Vec<UnitKey> = self
            .by_cell
            .iter()
            .filter(|&(&(x, y), _)| {
                cells
                    .iter()
                    .any(|&(cx, cy)| (cx - x).abs() <= 1 && (cy - y).abs() <= 1)
            })
            .flat_map(|(_, set)| set.iter().copied())
            .collect();
        for key in hit {
            self.touch(key);
        }
    }

    /// The units whose squad uses `model` are placed again (it arrived or failed).
    pub fn invalidate_model(&mut self, model: &LogicalId) {
        let hit: Vec<UnitKey> = self
            .placed
            .iter()
            .filter(|(_, p)| p.deps.contains(model))
            .map(|(k, _)| *k)
            .collect();
        for key in hit {
            self.touch(key);
        }
        // The units of the models of `assets-b2` that use it.
        let hit: Vec<UnitKey> = self
            .figures
            .iter()
            .filter(|(_, f)| f.iter().any(|p| &p.model == model))
            .map(|(k, _)| *k)
            .collect();
        for key in hit {
            self.touch(key);
        }
    }

    pub fn get(&self, key: &UnitKey) -> Option<&UnitEntity> {
        self.records.get(key)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn units(&self) -> impl Iterator<Item = &UnitEntity> {
        self.records.values()
    }

    /// What the view says of a unit beyond its record (facing, flags, what a worker builds). True
    /// if it changed; the unit is placed again and its animation is asked for the new state.
    pub fn set_extra(&mut self, key: UnitKey, extra: UnitVisExtra) -> bool {
        let old = self.extras.get(&key).copied().unwrap_or_default();
        if old == extra {
            return false;
        }
        if extra == UnitVisExtra::default() {
            self.extras.remove(&key);
        } else {
            self.extras.insert(key, extra);
        }
        self.touch(key);
        true
    }

    pub fn extra(&self, key: &UnitKey) -> UnitVisExtra {
        self.extras.get(key).copied().unwrap_or_default()
    }

    /// The units whose figures are placed for the models of `assets-b2`, and how many figures.
    pub fn figures_of(&self, key: &UnitKey) -> &[Placement] {
        self.figures.get(key).map_or(&[], Vec::as_slice)
    }

    /// Places again the units that changed, for the models of `assets-b2` (WOB2); returns how many
    /// it placed. A unit gone leaves no figure.
    pub fn refresh_b2(&mut self, ctx: &B2Context<'_>) -> usize {
        let mut dirty = std::mem::take(&mut self.dirty_b2);
        let mut n = 0usize;
        let start = crate::perf::slow::now_ms();
        while let Some(key) = dirty.pop_first() {
            // The rest waits for the next frame: a turn that moves many units places them over a
            // few frames.
            if n > 0 && crate::perf::slow::now_ms() - start >= PLACE_BUDGET_MS {
                self.dirty_b2.insert(key);
                self.dirty_b2.extend(dirty);
                break;
            }
            n += 1;
            let Some(u) = self.records.get(&key) else {
                self.figures.remove(&key);
                self.set_figs(key, &[]);
                continue;
            };
            let extra = self.extras.get(&key).copied().unwrap_or_default();
            let slot = match (is_shared(self.kinds_on((u.x, u.y))), u.civilian) {
                (false, _) => Slot::Alone,
                (true, false) => Slot::Military,
                (true, true) => Slot::Civilian,
            };
            let mut placed = Vec::new();
            match ctx.art.unit_b2(&u.type_name) {
                None => tracing::warn!(
                    "unit `{}`: no record in the art table; nothing is drawn",
                    u.type_name
                ),
                Some(rec) => {
                    let style = ctx.owners.culture_of(u.owner).map(|c| c.suffix());
                    // The owner's own era up to the Renaissance (the city bucket puts the
                    // Medieval with the Ancient, re-b17 4.4); the later eras keep the
                    // Renaissance variants as before.
                    let era = ctx
                        .owners
                        .era_index_of(u.owner)
                        .map(|i| i.min(3))
                        .unwrap_or_else(|| era_index(ctx.owners.era_of(u.owner)));
                    let embarked = extra.flags.has(place::UnitVisFlags::EMBARKED);
                    // An embarked unit is drawn as the boat of its owner's era (its own record
                    // when the table has no boat).
                    let embark = embarked
                        .then(|| art::embark_art_info(ctx.owners.era_index_of(u.owner), None))
                        .and_then(|name| ctx.art.unit_b2(name))
                        .map(|r| &r.base);
                    let choice = art::select_art(rec, style, Some(era), embarked, embark);
                    let figures = art::figure_specs(choice.art);
                    if figures.is_empty() {
                        tracing::warn!(
                            "unit `{}`: its art has no figures; nothing is drawn",
                            u.type_name
                        );
                    }
                    let form = formation::Params::of_name(choice.art.formation.as_deref(), |n| {
                        ctx.art.formation(n)
                    });
                    place::place_figures(
                        &place::PlaceParams {
                            key,
                            cell: (u.x, u.y),
                            slot,
                            extra: &extra,
                            hp: u.hp,
                            max_hp: u.max_hp,
                            figures: &figures,
                            formation: &form,
                            water_level: ctx.water_level,
                            damage_states: choice.art.damage_states,
                        },
                        ctx.ground,
                        &mut placed,
                    );
                }
            }
            self.set_figs(key, &placed);
            self.figures.insert(key, placed);
        }
        self.placed_b2_last = n as u32;
        self.placed_b2_total += n as u64;
        n
    }

    /// Makes the records of the frame for the figures just placed for `key` (none: the unit is
    /// gone), keeping the animation handles the figures had, and says that the animation has to
    /// look at the unit.
    fn set_figs(&mut self, key: UnitKey, placed: &[Placement]) {
        if let Some(old) = self.figs.remove(&key) {
            for f in &old {
                self.models.uses[f.model as usize] -= 1;
            }
        }
        self.anim_dirty.insert(key);
        if placed.is_empty() {
            return;
        }
        let mut figs = Vec::with_capacity(placed.len());
        for p in placed {
            let model = self.models.intern(&p.model);
            self.models.uses[model as usize] += 1;
            figs.push(Fig {
                handle: self
                    .handles
                    .get(&(key, p.index))
                    .filter(|(_, _, m)| *m == p.model)
                    .map(|(h, _, _)| *h),
                inst: place::object_instance(p, [0; 3], BoneRef::default()),
                model,
                state: p.state,
            });
        }
        self.figs.insert(key, figs);
    }

    /// The figures whose cell centre is within `radius` of `near` (X, Z) and the clips they play, for the sounds of the
    /// units. Appended to `out`.
    pub fn sounding_figures(
        &self,
        anim: &AnimService,
        near: Vec2,
        radius: f32,
        out: &mut Vec<FigureClip>,
    ) {
        let r2 = radius * radius;
        for (key, placed) in &self.figures {
            let members = placed.len().min(usize::from(u8::MAX)) as u8;
            for p in placed {
                if (Vec2::new(p.position.x, p.position.z) - near).length_squared() > r2 {
                    continue;
                }
                let clip = self
                    .handles
                    .get(&(*key, p.index))
                    .and_then(|(h, _, _)| anim.clip_time(*h));
                out.push(FigureClip {
                    unit: *key,
                    index: p.index,
                    model: p.model.clone(),
                    position: p.position,
                    clip,
                    members,
                });
            }
        }
    }

    /// How many units the last `refresh_b2` placed, and since the start.
    pub fn placed_b2(&self) -> (u32, u64) {
        (self.placed_b2_last, self.placed_b2_total)
    }

    /// The animation of the figures: a figure gets a handle when its model has
    /// clips, keeps it while it stands in the plan (a move does not change the key), and is asked
    /// for the state of its unit; the figures that are gone give their handles back.
    ///
    /// Only the units placed since the last call are looked at (`anim_dirty`), unless the models of
    /// the service changed: then every figure asks again, as a model that arrived lets the figures
    /// that wait for it attach.
    pub fn sync_anim(&mut self, anim: &mut AnimService) {
        self.release_handles(anim);
        let revision = anim.models_revision();
        let mut dirty = std::mem::take(&mut self.anim_dirty);
        if self.synced_revision != Some(revision) {
            self.synced_revision = Some(revision);
            dirty.extend(self.figures.keys().copied());
            dirty.extend(self.handles.keys().map(|k| k.0));
        }
        for key in dirty {
            self.sync_unit(key, anim);
        }
    }

    /// `sync_anim` for one unit: attaches and updates the handles of its figures, gives back those
    /// of the figures that are gone, and puts the handles in the records of the frame.
    fn sync_unit(&mut self, key: UnitKey, anim: &mut AnimService) {
        let placed = self.figures.get(&key).map_or(&[][..], Vec::as_slice);
        let want = place::anim_state_of(&self.extras.get(&key).copied().unwrap_or_default());
        for (j, p) in placed.iter().enumerate() {
            let k = (key, p.index);
            let target = AnimTarget::Figure {
                unit: key,
                index: p.index,
            };
            let handle = match self.handles.get_mut(&k) {
                Some((h, state, model)) if anim.is_live(*h) && *model == p.model => {
                    if *state != want {
                        anim.set_state(*h, want);
                        *state = want;
                    }
                    Some(*h)
                }
                _ => {
                    // A handle of another model (the unit was upgraded, or the key is a unit of another game) is
                    // given back: its skeleton does not fit the figure.
                    if let Some((old, _, _)) = self.handles.remove(&k) {
                        anim.release(old);
                    }
                    match anim.attach(&p.model, target) {
                        Some(h) => {
                            anim.set_state(h, want);
                            self.handles.insert(k, (h, want, p.model.clone()));
                            Some(h)
                        }
                        None => None,
                    }
                }
            };
            if let Some(f) = self.figs.get_mut(&key).and_then(|v| v.get_mut(j)) {
                f.handle = handle;
            }
        }
        // The handles of figures the unit no longer has.
        let gone: Vec<(UnitKey, u8)> = self
            .handles
            .range((key, 0)..=(key, u8::MAX))
            .map(|(k, _)| *k)
            .filter(|(_, i)| !placed.iter().any(|p| p.index == *i))
            .collect();
        for k in gone {
            if let Some((h, _, _)) = self.handles.remove(&k) {
                anim.release(h);
            }
        }
    }

    /// The instances of this frame, by model: one call per model (WO-D4); the figures of a damaged
    /// ship go to the second map, by model and node state (R5.11). A figure whose clip is not in the
    /// bone texture yet is drawn in the pose of the bind (or its frozen last one), never left out;
    /// a rigid model has no animation and the zero pose.
    pub fn frame_instances(
        &self,
        anim: &AnimService,
        owners: &Owners,
    ) -> (BTreeMap<LogicalId, Vec<ObjectInstance>>, draw::StateBatches) {
        let mut bufs = FrameBufs::default();
        self.fill_instances(anim, owners, &mut bufs);
        self.take_batches(&mut bufs)
    }

    /// Writes the instances of the frame into `bufs` (cleared first, its allocations kept): the
    /// colour of the team and the pose go into the matrix made at the placement.
    fn fill_instances(&self, anim: &AnimService, owners: &Owners, bufs: &mut FrameBufs) {
        bufs.by_slot.resize_with(self.models.ids.len(), Vec::new);
        for v in &mut bufs.by_slot {
            v.clear();
        }
        for v in bufs.states.values_mut() {
            v.clear();
        }
        // The units are in the order of the key, so the owner changes rarely.
        let mut team = (i8::MIN, [0u8; 3]);
        let mut have_team = false;
        for (key, figs) in &self.figs {
            if !have_team || team.0 != key.0 {
                team = (key.0, owners.team_color_of(key.0));
                have_team = true;
            }
            for f in figs {
                let mut inst = f.inst;
                inst.color[..3].copy_from_slice(&team.1);
                if let Some(h) = f.handle {
                    inst.bones = anim.bone_ref(h);
                }
                if f.state == asset_format::STATE_INTACT {
                    bufs.by_slot[f.model as usize].push(inst);
                } else {
                    bufs.states
                        .entry((f.model, f.state))
                        .or_default()
                        .push(inst);
                }
            }
        }
    }

    /// The lists of `bufs` as the maps the draw takes, by name. The vectors move out; `recycle`
    /// gives them back.
    fn take_batches(
        &self,
        bufs: &mut FrameBufs,
    ) -> (BTreeMap<LogicalId, Vec<ObjectInstance>>, draw::StateBatches) {
        let mut out = BTreeMap::new();
        for (slot, v) in bufs.by_slot.iter_mut().enumerate() {
            if !v.is_empty() {
                out.insert(self.models.ids[slot].clone(), std::mem::take(v));
            }
        }
        let mut states = draw::StateBatches::new();
        for (&(slot, state), v) in &mut bufs.states {
            if !v.is_empty() {
                states.insert(
                    (self.models.ids[slot as usize].clone(), state),
                    std::mem::take(v),
                );
            }
        }
        (out, states)
    }

    /// Takes the emptied lists back from the maps `take_batches` made.
    fn recycle(&self, batches: BTreeMap<LogicalId, Vec<ObjectInstance>>, bufs: &mut FrameBufs) {
        for (id, mut v) in batches {
            if let Some(&slot) = self.models.slots.get(&id) {
                v.clear();
                if let Some(b) = bufs.by_slot.get_mut(slot as usize)
                    && b.capacity() < v.capacity()
                {
                    *b = v;
                }
            }
        }
    }

    // ---- the draw of WOB2 ----

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
                self.b2 = Some(Box::new(B2State {
                    draw: draw::UnitDraw::new(device, queue, colour, depth, samples),
                    active: false,
                    owners: Owners::default(),
                    shifts: vec![0.0],
                    eye: Vec3::ZERO,
                }));
            }
        }
    }

    /// The units draw through the models of `assets-b2` (and not B1's). The art of `assets-b2` and
    /// the light data have to be there; the renderer says so.
    pub fn set_b2_active(&mut self, on: bool) -> bool {
        let Some(b2) = &mut self.b2 else {
            return false;
        };
        if b2.active == on {
            return false;
        }
        b2.active = on;
        // The units are placed again for the other path.
        self.dirty_b2.extend(self.records.keys().copied());
        self.records.touch_all();
        true
    }

    pub fn b2_active(&self) -> bool {
        self.b2.as_ref().is_some_and(|b| b.active)
    }

    /// The instances of models that are not units' and the models they need (`MovementPath`); an
    /// empty map stops them.
    pub fn set_path_pieces(
        &mut self,
        wanted: Vec<LogicalId>,
        batches: BTreeMap<LogicalId, Vec<ObjectInstance>>,
    ) {
        self.extra_wanted = wanted;
        self.extra_batches = batches;
    }

    /// True if the model is on the draw's GPU.
    pub fn b2_has_model(&self, id: &LogicalId) -> bool {
        self.b2.as_ref().is_some_and(|b| b.draw.has_model(id))
    }

    /// The models of the figures and of the extra pieces that are not on the GPU yet.
    pub fn b2_models_wanted(&self) -> Vec<LogicalId> {
        let Some(b2) = &self.b2 else {
            return Vec::new();
        };
        let wanted: BTreeSet<&LogicalId> = self
            .models
            .ids
            .iter()
            .zip(&self.models.uses)
            .filter(|(_, n)| **n > 0)
            .map(|(m, _)| m)
            .chain(self.extra_wanted.iter())
            .filter(|m| !b2.draw.has_model(m))
            .collect();
        wanted.into_iter().cloned().collect()
    }

    /// The draw, for the renderer that gives it models.
    pub fn b2_draw_mut(&mut self) -> Option<&mut draw::UnitDraw> {
        self.b2.as_mut().map(|b| &mut b.draw)
    }

    pub fn b2_draw(&self) -> Option<&draw::UnitDraw> {
        self.b2.as_ref().map(|b| &b.draw)
    }

    /// The part of the frame that needs the inputs of the renderer, before the plan: places the units
    /// that changed, asks the animation for their states, and binds the light and the fog mask to
    /// the draw. The instances themselves are made in `prepare`, after the animation stepped.
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
        b2.owners = f.owners.clone();
        b2.shifts = f.shifts.to_vec();
        b2.eye = f.eye;
    }

    /// The casters of the units into the pass of the shadow map.
    pub fn record_shadow(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        stats: &mut crate::renderer::FrameStats,
    ) {
        if let Some(b2) = self.b2.as_ref().filter(|b| b.active) {
            b2.draw.record_shadow(pass, stats);
        }
    }

    /// The meshes of the units and the objects that reflect, into the pass of the reflection, for
    /// every copy of the map that reaches the screen.
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

    fn kinds_on(&self, cell: (i32, i32)) -> CellKinds {
        let mut kinds = (false, false);
        for key in self.by_cell.get(&cell).into_iter().flatten() {
            if let Some(u) = self.records.get(key) {
                if u.civilian {
                    kinds.1 = true;
                } else {
                    kinds.0 = true;
                }
            }
        }
        kinds
    }

    /// Places the units that changed; returns how many it placed.
    pub fn refresh(&mut self, owners: &Owners, ctx: &PlanContext<'_>) -> usize {
        let dirty = self.records.take_dirty();
        for key in &dirty {
            let Some(u) = self.records.get(key) else {
                self.placed.remove(key);
                continue;
            };
            let shared = is_shared(self.kinds_on((u.x, u.y)));
            let placed = place_unit(u, shared, owners, ctx);
            self.placed.insert(*key, placed);
        }
        self.counters.placed_last = dirty.len() as u32;
        self.counters.placed_total += dirty.len() as u64;
        dirty.len()
    }

    /// Adds every placed unit to `plan`, in the order of B1 (by id).
    pub fn assemble(&self, plan: &mut EntityPlan) {
        let mut all: Vec<(&UnitKey, &UnitPlaced)> = self.placed.iter().collect();
        all.sort_by_key(|(k, _)| (k.1, k.0));
        for (_, placed) in all {
            placed.add_to(plan);
        }
    }

    /// The counters; `instances` and `batches` are what the last assembly gave to the draw.
    pub fn counters(&self) -> ObjectCounters {
        ObjectCounters {
            records: self.records.len() as u32,
            ..self.counters
        }
    }

    /// The renderer tells what the draw got from the whole plan.
    pub fn set_assembled(&mut self, instances: usize, batches: usize) {
        self.counters.instances = instances as u32;
        self.counters.batches = batches as u32;
    }
}

impl RenderSystem for Unit {
    fn id(&self) -> SystemId {
        SystemId::Unit
    }

    fn passes(&self) -> &'static [PassSlot] {
        &[
            PassSlot::Pass(PassId::ShadowMap),
            PassSlot::Pass(PassId::Reflection),
            PassSlot::Main(MainSlot::UnitZFail),
            PassSlot::Main(MainSlot::Unit),
        ]
    }

    fn init(&mut self, _cx: &mut InitContext<'_>) {}

    /// WOB2: the instances of the frame, after the animation stepped (`PrepareServices`).
    fn prepare(&mut self, cx: &mut PrepareContext<'_>) {
        let Some((owners, shifts, eye)) = self
            .b2
            .as_ref()
            .filter(|b| b.active)
            .map(|b| (b.owners.clone(), b.shifts.clone(), b.eye))
        else {
            return;
        };
        let services = cx.services();
        let anim = &services.anim;
        let mut bufs = std::mem::take(&mut self.bufs);
        self.fill_instances(anim, &owners, &mut bufs);
        let (mut batches, states) = self.take_batches(&mut bufs);
        for (id, items) in &self.extra_batches {
            batches
                .entry(id.clone())
                .or_default()
                .extend(items.iter().copied());
        }
        let animated = |id: &LogicalId| anim.model(id).is_some_and(|m| m.animated());
        let Some(b2) = &mut self.b2 else {
            self.bufs = bufs;
            return;
        };
        let report = b2.draw.build(
            cx.device,
            cx.queue,
            services.bones(),
            &draw::BuildInputs {
                batches: &batches,
                parts: &BTreeMap::new(),
                states: &states,
                part_states: &Default::default(),
                shifts: &shifts,
                eye,
                animated: &animated,
                remember: false,
                models_revision: anim.models_revision(),
            },
        );
        self.recycle(batches, &mut bufs);
        self.bufs = bufs;
        self.counters.instances = report.instances;
        self.counters.batches = report.calls;
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
    use crate::entities::units::tests::{ready, snapshot, table, terrain, unit};
    use crate::entities::{ArtState, ModelRef, ModelStatus, build_plan};

    fn ctx<'a>(
        terrain: &'a crate::terrain::TerrainSnapshot,
        art: &'a ArtState,
        status: &'a dyn Fn(&LogicalId) -> ModelStatus,
    ) -> PlanContext<'a> {
        PlanContext {
            terrain,
            art,
            status,
        }
    }

    fn assembled(system: &Unit) -> EntityPlan {
        let mut plan = EntityPlan::default();
        system.assemble(&mut plan);
        plan
    }

    /// WOB0: the keyed units give the plan B1 builds from a snapshot, byte for byte.
    #[test]
    fn the_keyed_units_make_the_plan_of_the_snapshot() {
        let units = vec![
            unit(3, "UNIT_WARRIOR", 1, 1, false),
            unit(1, "UNIT_SETTLER", 1, 1, true),
            unit(2, "UNIT_WARRIOR", 2, 2, false),
            unit(4, "UNIT_STUB", 3, 3, false),
            unit(5, "UNIT_WARRIOR", 5, 5, false),
        ];
        let s = snapshot(units.clone());
        let (t, art) = (terrain(), ArtState::Ready(table()));
        let c = ctx(&t, &art, &ready);
        let mut system = Unit::default();
        for u in units {
            system.upsert(u);
        }
        system.refresh(&s.owners(), &c);
        let (a, b) = (assembled(&system), build_plan(&s, &c));
        assert_eq!(a.batches, b.batches);
        assert_eq!(a.labels, b.labels);
        assert_eq!(a.wanted, b.wanted);
        assert_eq!(a.unit_models, b.unit_models);
    }

    /// WOB0 test of the card: `upsert` of one unit places that unit, not the others.
    #[test]
    fn upsert_of_one_unit_does_not_place_the_others() {
        let units: Vec<UnitEntity> = (0..6)
            .map(|i| unit(i + 1, "UNIT_WARRIOR", i % 5, 1 + i / 5, false))
            .collect();
        let s = snapshot(units.clone());
        let (t, art) = (terrain(), ArtState::Ready(table()));
        let c = ctx(&t, &art, &ready);
        let mut system = Unit::default();
        for u in units.clone() {
            system.upsert(u);
        }
        assert_eq!(system.refresh(&s.owners(), &c), 6);
        let kept = system.placed.clone();

        // One unit moves to an empty cell: only it is placed again.
        let mut moved = units[2].clone();
        moved.x = 4;
        moved.y = 3;
        assert!(system.upsert(moved.clone()));
        assert_eq!(system.refresh(&s.owners(), &c), 1);
        assert_eq!(system.counters().placed_last, 1);
        assert_eq!(system.counters().placed_total, 7);
        for (key, placed) in &kept {
            if *key != (0, 3) {
                assert_eq!(
                    system.placed.get(key),
                    Some(placed),
                    "{key:?} kept its placement"
                );
            }
        }
        assert_ne!(system.placed[&(0, 3)], kept[&(0, 3)]);

        // The same record again is no change at all.
        assert!(!system.upsert(moved));
        assert_eq!(system.refresh(&s.owners(), &c), 0);

        // A removal places nothing again (the unit stood alone) and drops its plan.
        assert!(system.remove((0, 3)));
        assert_eq!(system.refresh(&s.owners(), &c), 0);
        assert!(!system.placed.contains_key(&(0, 3)));
        assert!(!system.remove((0, 3)));
    }

    /// A military and a civilian unit on one cell stand apart: the mate is placed again when the
    /// other comes or goes, and only then.
    #[test]
    fn a_mate_on_the_cell_is_placed_again_when_the_other_kind_arrives() {
        let (t, art) = (terrain(), ArtState::Ready(table()));
        let c = ctx(&t, &art, &ready);
        let s = snapshot(vec![]);
        let mut system = Unit::default();
        system.upsert(unit(1, "UNIT_WARRIOR", 2, 2, false));
        system.upsert(unit(2, "UNIT_WARRIOR", 3, 3, false));
        system.refresh(&s.owners(), &c);
        let alone = system.placed[&(0, 1)].clone();

        system.upsert(unit(3, "UNIT_SETTLER", 2, 2, true));
        assert_eq!(
            system.refresh(&s.owners(), &c),
            2,
            "the arrival and its mate"
        );
        assert_ne!(system.placed[&(0, 1)], alone, "the warrior stepped aside");

        system.remove((0, 3));
        assert_eq!(system.refresh(&s.owners(), &c), 1, "the mate only");
        assert_eq!(system.placed[&(0, 1)], alone);
    }

    #[test]
    fn a_model_arriving_places_the_units_that_wait_for_it() {
        let units = vec![
            unit(1, "UNIT_WARRIOR", 1, 1, false),
            unit(2, "UNIT_SETTLER", 2, 2, true),
        ];
        let s = snapshot(units.clone());
        let (t, art) = (terrain(), ArtState::Ready(table()));
        let pending = |_: &LogicalId| ModelStatus::Pending;
        let mut system = Unit::default();
        for u in units {
            system.upsert(u);
        }
        system.refresh(&s.owners(), &ctx(&t, &art, &pending));
        assert_eq!(assembled(&system).wanted.len(), 2);

        system.invalidate_model(&LogicalId::new("model:warrior"));
        assert_eq!(system.refresh(&s.owners(), &ctx(&t, &art, &ready)), 1);
        let plan = assembled(&system);
        let has = |id: &str| {
            plan.batches
                .contains_key(&ModelRef::Asset(LogicalId::new(id)))
        };
        assert!(has("model:warrior"));
        assert!(!has("model:settler"));
    }

    // ---- WOB2: the models of assets-b2 ----

    use crate::anim::data::{ModelAnim, fixtures as anim_fixtures};
    use crate::anim::{AnimService, EC_FORTIFY_ACTION};
    use crate::entities::{Culture, EntityPlayer};
    use crate::hexgrid::cell_center;
    use crate::world::objects::instance::BoneRef as Bones;
    use asset_format::{ArtTable, FigureB2, UnitArtB2, UnitRecordB2, UnitVariantB2};
    use place::UnitVisFlags;

    fn art_b2(tag: &str, count: u32) -> UnitArtB2 {
        UnitArtB2 {
            art_info: format!("ART_{tag}"),
            figures: vec![FigureB2 {
                member: format!("M_{tag}"),
                model: LogicalId::new(format!("md:{tag}")),
                count,
                scale: Some(0.14),
                z_offset: None,
                domain: None,
            }],
            damage_states: None,
            formation: None,
        }
    }

    fn table_b2() -> ArtTable {
        let mut t = ArtTable::new();
        t.insert_unit_b2(
            "UNIT_WARRIOR",
            UnitRecordB2 {
                base: art_b2("warrior", 3),
                style_variants: vec![UnitVariantB2 {
                    suffix: "_ASIA".into(),
                    art: art_b2("warrior_asia", 3),
                }],
                era_variation: false,
                era_variants: vec![],
            },
        );
        t.insert_unit_b2(
            "UNIT_GALLEY",
            UnitRecordB2 {
                base: art_b2("galley", 1),
                style_variants: vec![],
                era_variation: false,
                era_variants: vec![],
            },
        );
        t.insert_unit_b2(
            "UNIT_TRIREME",
            UnitRecordB2 {
                base: UnitArtB2 {
                    damage_states: Some(3),
                    ..art_b2("trireme", 1)
                },
                style_variants: vec![],
                era_variation: false,
                era_variants: vec![],
            },
        );
        t
    }

    fn owners_b2() -> Owners {
        Owners {
            players: vec![
                EntityPlayer {
                    id: 0,
                    color: [200, 30, 40],
                    team_color: [200, 30, 40],
                    culture: None,
                    era: None,
                    industrial: false,
                    art_techs: 0,
                    era_index: None,
                },
                EntityPlayer {
                    id: 1,
                    color: [10, 20, 30],
                    team_color: [10, 20, 30],
                    culture: Some(Culture::Asia),
                    era: None,
                    industrial: false,
                    art_techs: 0,
                    era_index: None,
                },
            ],
            era: Default::default(),
        }
    }

    fn flat(_: Vec2) -> f32 {
        0.0
    }

    fn b2<'a>(art: &'a ArtTable, owners: &'a Owners) -> B2Context<'a> {
        B2Context {
            art,
            owners,
            ground: &flat,
            water_level: None,
        }
    }

    fn animated(service: &mut AnimService, name: &str, fortify: bool) {
        let (mut m, mut pack) = anim_fixtures::model(name, 3);
        if fortify {
            for (i, ec) in [1500u32, 1520].into_iter().enumerate() {
                pack.clips
                    .push(anim_fixtures::swing_clip("Fortify", 1.0, 3, 40.0));
                m.clips.push(asset_format::ClipRef {
                    name: format!("Fortify{ec}"),
                    ec,
                    index: 6 + i as u16,
                    events: vec![],
                    legacy_blob: None,
                });
            }
        }
        let fsm = anim_fixtures::omni_machine_v2();
        service.register_model(
            LogicalId::new(format!("md:{name}")),
            ModelAnim::new(&m, Some(pack), Some(&fsm)).unwrap(),
        );
    }

    fn centre_of(p: &[Placement]) -> Vec2 {
        let s: Vec2 = p
            .iter()
            .map(|f| Vec2::new(f.position.x, f.position.z))
            .sum();
        s / p.len() as f32
    }

    #[test]
    fn b2_places_the_squad_on_its_cell_with_the_art_of_the_owner() {
        let (art, owners) = (table_b2(), owners_b2());
        let mut u = Unit::default();
        u.upsert(unit(1, "UNIT_WARRIOR", 3, 2, false));
        let mut asian = unit(2, "UNIT_WARRIOR", 1, 1, false);
        asian.owner = 1;
        u.upsert(asian);
        assert_eq!(u.refresh_b2(&b2(&art, &owners)), 2);
        let a = u.figures_of(&(0, 1));
        let b = u.figures_of(&(1, 2));
        assert_eq!((a.len(), b.len()), (3, 3));
        assert!(a.iter().all(|p| p.model.as_str() == "md:warrior"));
        assert!(b.iter().all(|p| p.model.as_str() == "md:warrior_asia"));
        // The squad is around the middle of its cell.
        assert!((centre_of(a) - cell_center(3, 2)).length() < 30.0);
        assert!(a.iter().all(|p| p.scale == 0.14));
        // Nothing changed, nothing placed.
        assert_eq!(u.refresh_b2(&b2(&art, &owners)), 0);
    }

    #[test]
    fn b2_draws_nothing_for_a_unit_without_a_record() {
        let (art, owners) = (table_b2(), owners_b2());
        let mut u = Unit::default();
        u.upsert(unit(1, "UNIT_GHOST", 1, 1, false));
        u.refresh_b2(&b2(&art, &owners));
        assert!(u.figures_of(&(0, 1)).is_empty());
        let mut anim = AnimService::default();
        u.sync_anim(&mut anim);
        assert!(
            u.frame_instances(&anim, &owners).0.is_empty(),
            "no marker (WO-D15)"
        );
    }

    /// WOB2 test of the spec: the jump. One `upsert` with another cell, and the next plan has every
    /// figure on the new cell, with the same animation (the same handle, the clip goes on).
    #[test]
    fn a_move_is_a_jump_and_the_animation_goes_on() {
        let (art, owners) = (table_b2(), owners_b2());
        let mut anim = AnimService::default();
        animated(&mut anim, "warrior", false);
        let mut u = Unit::default();
        let mut w = unit(1, "UNIT_WARRIOR", 1, 1, false);
        u.upsert(w.clone());
        u.refresh_b2(&b2(&art, &owners));
        u.sync_anim(&mut anim);
        let before: Vec<AnimHandle> = (0..3u8).map(|i| u.handles[&((0, 1), i)].0).collect();
        for i in 0..120 {
            anim.step(i as f32 / 60.0);
        }
        let pose = |u: &Unit, a: &AnimService| -> Vec<Bones> {
            (0..3u8)
                .map(|i| a.bone_ref(u.handles[&((0, 1), i)].0))
                .collect()
        };
        let still = pose(&u, &anim);
        let here = u.figures_of(&(0, 1)).to_vec();

        w.x = 5;
        w.y = 4;
        assert!(u.upsert(w));
        assert_eq!(u.refresh_b2(&b2(&art, &owners)), 1);
        u.sync_anim(&mut anim);
        let after: Vec<AnimHandle> = (0..3u8).map(|i| u.handles[&((0, 1), i)].0).collect();
        assert_eq!(before, after, "the same animation");
        assert_eq!(anim.stats().attached, 3);
        let there = u.figures_of(&(0, 1)).to_vec();
        let shift = cell_center(5, 4) - cell_center(1, 1);
        for (a, b) in here.iter().zip(&there) {
            assert_eq!(a.index, b.index);
            let d = Vec2::new(b.position.x - a.position.x, b.position.z - a.position.z);
            assert!((d - shift).length() < 1e-3);
        }
        // The clip was not started again: one step later the pose is where it would have been.
        anim.step(2.0);
        let moved = pose(&u, &anim);
        assert_ne!(still, moved, "time went on");
        let (inst, _) = u.frame_instances(&anim, &owners);
        let list = &inst[&LogicalId::new("md:warrior")];
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].color, [200, 30, 40, 255]);
        // A unit that dies leaves nothing and gives its animation back.
        assert!(u.remove((0, 1)));
        u.refresh_b2(&b2(&art, &owners));
        u.sync_anim(&mut anim);
        assert!(u.frame_instances(&anim, &owners).0.is_empty());
        assert_eq!(anim.stats().attached, 0);
    }

    #[test]
    fn a_figure_shows_its_bind_pose_for_its_clip_and_a_rigid_model_has_none() {
        let (art, owners) = (table_b2(), owners_b2());
        let mut anim = AnimService::default();
        animated(&mut anim, "warrior", false);
        let mut u = Unit::default();
        u.upsert(unit(1, "UNIT_WARRIOR", 1, 1, false));
        u.upsert(unit(2, "UNIT_GALLEY", 2, 2, false));
        u.refresh_b2(&b2(&art, &owners));
        u.sync_anim(&mut anim);
        // The warrior has clips that are not baked yet: its figures are there in the pose of the
        // bind (the identity bones at slot 0); the galley is rigid with the zero pose.
        let (first, _) = u.frame_instances(&anim, &owners);
        assert_eq!(first[&LogicalId::new("md:warrior")].len(), 3);
        assert!(
            first[&LogicalId::new("md:warrior")]
                .iter()
                .all(|i| i.bones == Bones::default()),
            "the bind pose is the reference at the identity run"
        );
        assert_eq!(first[&LogicalId::new("md:galley")].len(), 1);
        assert_eq!(
            first[&LogicalId::new("md:galley")][0].bones,
            Bones::default()
        );
        anim.step(0.0);
        let (then, _) = u.frame_instances(&anim, &owners);
        assert_eq!(then[&LogicalId::new("md:warrior")].len(), 3);
    }

    /// R5.11: a ship with `DamageStates` 3 draws its intact state at full HP, and the state bit of
    /// its damage level below (33..65 percent of damage: bit 1); a ship without damage states stays
    /// intact.
    #[test]
    fn a_damaged_ship_is_drawn_in_its_damage_state() {
        let (art, owners) = (table_b2(), owners_b2());
        let mut anim = AnimService::default();
        let mut u = Unit::default();
        let mut ship = unit(1, "UNIT_TRIREME", 1, 1, false);
        ship.max_hp = 100;
        ship.hp = 100;
        u.upsert(ship.clone());
        let mut galley = unit(2, "UNIT_GALLEY", 2, 2, false);
        galley.max_hp = 100;
        galley.hp = 40;
        u.upsert(galley);
        u.refresh_b2(&b2(&art, &owners));
        u.sync_anim(&mut anim);
        let (intact, states) = u.frame_instances(&anim, &owners);
        assert_eq!(intact[&LogicalId::new("md:trireme")].len(), 1);
        assert_eq!(
            intact[&LogicalId::new("md:galley")].len(),
            1,
            "DamageStates absent"
        );
        assert!(states.is_empty());
        ship.hp = 50;
        u.upsert(ship.clone());
        u.refresh_b2(&b2(&art, &owners));
        let (intact, states) = u.frame_instances(&anim, &owners);
        assert!(!intact.contains_key(&LogicalId::new("md:trireme")));
        assert_eq!(states[&(LogicalId::new("md:trireme"), 2)].len(), 1);
        ship.hp = 1;
        u.upsert(ship);
        u.refresh_b2(&b2(&art, &owners));
        let (_, states) = u.frame_instances(&anim, &owners);
        assert_eq!(
            states[&(LogicalId::new("md:trireme"), 8)].len(),
            1,
            "99 percent: bit 3"
        );
    }

    #[test]
    fn the_figures_near_the_listener_tell_their_clip_and_its_time() {
        let (art, owners) = (table_b2(), owners_b2());
        let mut anim = AnimService::default();
        animated(&mut anim, "warrior", true);
        let mut u = Unit::default();
        u.upsert(unit(1, "UNIT_WARRIOR", 1, 1, false));
        u.upsert(unit(2, "UNIT_WARRIOR", 30, 20, false));
        u.refresh_b2(&b2(&art, &owners));
        u.sync_anim(&mut anim);
        anim.step(0.5);
        let mut near = Vec::new();
        u.sounding_figures(&anim, cell_center(1, 1), 200.0, &mut near);
        assert_eq!(near.len(), 3, "the far unit is left out");
        assert!(near.iter().all(|f| f.unit == (0, 1) && f.members == 3));
        let (ec, t) = near[0].clip.unwrap();
        assert!(matches!(ec, 1000 | 1040) && t >= 0.0, "{ec} {t}");
        // The time goes on with the clock.
        anim.step(0.6);
        let mut later = Vec::new();
        u.sounding_figures(&anim, cell_center(1, 1), 200.0, &mut later);
        let h = u.handles[&((0, 1), near[0].index)].0;
        assert_eq!(later[0].clip, anim.clip_time(h));
    }

    #[test]
    fn the_state_of_the_unit_is_asked_of_the_animation() {
        let (art, owners) = (table_b2(), owners_b2());
        let mut anim = AnimService::default();
        animated(&mut anim, "warrior", true);
        let mut u = Unit::default();
        u.upsert(unit(1, "UNIT_WARRIOR", 1, 1, false));
        u.refresh_b2(&b2(&art, &owners));
        u.sync_anim(&mut anim);
        anim.step(0.0);
        let h = u.handles[&((0, 1), 0)].0;
        assert!(matches!(anim.playing_ec(h), Some(1000 | 1040)));
        let fortified = UnitVisExtra {
            flags: UnitVisFlags::default().with(UnitVisFlags::FORTIFIED),
            ..UnitVisExtra::default()
        };
        assert!(u.set_extra((0, 1), fortified));
        assert!(
            !u.set_extra((0, 1), fortified),
            "the same state is no change"
        );
        assert_eq!(u.refresh_b2(&b2(&art, &owners)), 1);
        u.sync_anim(&mut anim);
        anim.step(0.2);
        assert_eq!(anim.playing_ec(h), Some(EC_FORTIFY_ACTION));
        // Back to standing.
        u.set_extra((0, 1), UnitVisExtra::default());
        u.refresh_b2(&b2(&art, &owners));
        u.sync_anim(&mut anim);
        // ENTER_FORTIFY plays its clip out first (1 s), then the held pose is left.
        for i in 5..=20 {
            anim.step(i as f32 * 0.1);
        }
        assert!(matches!(
            anim.playing_ec(h),
            Some(1000 | 1040 | 2000 | 2040)
        ));
    }

    #[test]
    fn cargo_and_facing_come_from_the_extra() {
        let (art, owners) = (table_b2(), owners_b2());
        let mut u = Unit::default();
        u.upsert(unit(1, "UNIT_WARRIOR", 1, 1, false));
        u.refresh_b2(&b2(&art, &owners));
        let plain = u.figures_of(&(0, 1)).to_vec();
        u.set_extra(
            (0, 1),
            UnitVisExtra {
                facing: Some(crate::hexgrid::Dir::East),
                ..UnitVisExtra::default()
            },
        );
        u.refresh_b2(&b2(&art, &owners));
        let turned = u.figures_of(&(0, 1)).to_vec();
        assert_ne!(plain[0].yaw, turned[0].yaw);
        u.set_extra(
            (0, 1),
            UnitVisExtra {
                flags: UnitVisFlags::default().with(UnitVisFlags::CARGO),
                ..UnitVisExtra::default()
            },
        );
        u.refresh_b2(&b2(&art, &owners));
        assert!(u.figures_of(&(0, 1)).is_empty(), "cargo is not drawn");
    }

    /// WOB0 test of the spec for the new placement: one `upsert` on a Huge view (600 units) places
    /// that unit and no other.
    #[test]
    fn b2_places_only_the_unit_that_changed() {
        let (art, owners) = (table_b2(), owners_b2());
        let mut u = Unit::default();
        for i in 0..600 {
            u.upsert(unit(i + 1, "UNIT_WARRIOR", i % 40, i / 40, false));
        }
        assert_eq!(u.refresh_b2(&b2(&art, &owners)), 600);
        let mut w = unit(1, "UNIT_WARRIOR", 39, 14, true);
        w.x = 20;
        w.y = 20;
        assert!(u.upsert(w));
        assert_eq!(u.refresh_b2(&b2(&art, &owners)), 1);
        assert_eq!(u.placed_b2(), (1, 601));
        // A change of art re-places everything.
        u.invalidate_all();
        assert_eq!(u.refresh_b2(&b2(&art, &owners)), 600);
        // New heights of one cell: the units on it and its neighbours stand on the new ground, no others.
        u.heights_dirty(&[(5, 3)]);
        assert_eq!(u.refresh_b2(&b2(&art, &owners)), 9);
    }

    /// A figure whose model arrives after the placement attaches then, though no unit changed:
    /// `sync_anim` looks at the units placed since the last call, and at all of them when the
    /// models of the service changed.
    #[test]
    fn a_model_that_arrives_late_animates_the_figures_that_wait_for_it() {
        let (art, owners) = (table_b2(), owners_b2());
        let mut anim = AnimService::default();
        let mut u = Unit::default();
        u.upsert(unit(1, "UNIT_WARRIOR", 1, 1, false));
        u.refresh_b2(&b2(&art, &owners));
        u.sync_anim(&mut anim);
        assert!(u.handles.is_empty(), "no model yet");
        animated(&mut anim, "warrior", false);
        u.sync_anim(&mut anim);
        assert_eq!(u.handles.len(), 3);
        // The record of the frame has the handles too.
        assert!(u.figs[&(0, 1)].iter().all(|f| f.handle.is_some()));
        anim.step(0.0);
        let (inst, _) = u.frame_instances(&anim, &owners);
        let list = &inst[&LogicalId::new("md:warrior")];
        assert!(list.iter().all(|i| i.bones != Bones::default()));
        // Nothing changed: the handles are the same ones.
        let before = u.handles.clone();
        u.sync_anim(&mut anim);
        assert_eq!(before, u.handles);
    }

    /// A second game in the same page: the units of the first leave nothing behind (figures, model counts, handles),
    /// and a unit of the new game with the same key does not inherit the skeleton of another model.
    #[test]
    fn a_new_game_inherits_no_figure_or_animation_handle() {
        let (art, owners) = (table_b2(), owners_b2());
        let mut anim = AnimService::default();
        animated(&mut anim, "warrior", false);
        animated(&mut anim, "galley", false);
        let mut u = Unit::default();
        u.upsert(unit(1, "UNIT_WARRIOR", 1, 1, false));
        u.refresh_b2(&b2(&art, &owners));
        u.sync_anim(&mut anim);
        assert_eq!(u.handle_counts(), (3, 0));
        u.clear();
        assert!(u.figures.is_empty() && u.figs.is_empty() && u.anim_dirty.is_empty());
        assert!(u.models.uses.iter().all(|n| *n == 0));
        assert_eq!(
            u.handle_counts(),
            (0, 3),
            "the handles wait to be given back"
        );
        u.release_handles(&mut anim);
        assert_eq!(u.handle_counts(), (0, 0));
        // Game two: the same key is a galley.
        u.upsert(unit(1, "UNIT_GALLEY", 2, 2, false));
        u.refresh_b2(&b2(&art, &owners));
        u.sync_anim(&mut anim);
        assert_eq!(u.handle_counts(), (1, 0));
        assert_eq!(u.handles[&((0, 1), 0)].2, LogicalId::new("md:galley"));
    }

    /// The same key with another model in one game (an upgrade): the old skeleton is given back, not kept.
    #[test]
    fn a_unit_that_changes_its_model_gets_a_handle_of_the_new_one() {
        let (art, owners) = (table_b2(), owners_b2());
        let mut anim = AnimService::default();
        animated(&mut anim, "warrior", false);
        animated(&mut anim, "galley", false);
        let mut u = Unit::default();
        u.upsert(unit(1, "UNIT_WARRIOR", 1, 1, false));
        u.refresh_b2(&b2(&art, &owners));
        u.sync_anim(&mut anim);
        let old = u.handles[&((0, 1), 0)].0;
        u.upsert(unit(1, "UNIT_GALLEY", 1, 1, false));
        u.refresh_b2(&b2(&art, &owners));
        u.sync_anim(&mut anim);
        assert_eq!(u.handle_counts(), (1, 0));
        let (h, _, model) = &u.handles[&((0, 1), 0)];
        assert_eq!(*model, LogicalId::new("md:galley"));
        assert_ne!(*h, old);
        assert!(
            !anim.is_live(old),
            "the handle of the old model is given back"
        );
    }

    /// The team colour of the frame follows the owners at once, with no new placement.
    #[test]
    fn the_team_colour_is_read_each_frame() {
        let (art, mut owners) = (table_b2(), owners_b2());
        let anim = AnimService::default();
        let mut u = Unit::default();
        u.upsert(unit(1, "UNIT_WARRIOR", 1, 1, false));
        u.refresh_b2(&b2(&art, &owners));
        let (a, _) = u.frame_instances(&anim, &owners);
        assert_eq!(
            a[&LogicalId::new("md:warrior")][0].color,
            [200, 30, 40, 255]
        );
        owners.players[0].team_color = [1, 2, 3];
        let (b, _) = u.frame_instances(&anim, &owners);
        assert_eq!(b[&LogicalId::new("md:warrior")][0].color, [1, 2, 3, 255]);
    }

    /// Micro-benchmark of the per-frame bookkeeping for about 14k animated figures (the heavy
    /// fixture): `sync_anim` with nothing changed, the instance lists, `AnimService::step` at the
    /// 100 ms frames of the heavy scene. Run in release:
    /// `cargo test -p render --release bench_figure_bookkeeping -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing only"]
    fn bench_figure_bookkeeping() {
        // Packet F: the old walk (every machine every tick, every reference every frame), then
        // sleeping machines with clocked references; frames of 100 ms and of 1/144 s.
        for (label, new) in [("old", false), ("new", true)] {
            for dt in [0.1f32, 1.0 / 144.0] {
                bench_figure_bookkeeping_with(label, new, dt);
            }
        }
    }

    fn bench_figure_bookkeeping_with(label: &str, new: bool, dt: f32) {
        use std::time::Instant;
        let (art, owners) = (table_b2(), owners_b2());
        let mut anim = AnimService::default();
        anim.set_fsm_sleep(new);
        anim.set_gpu_clock(new);
        animated(&mut anim, "warrior", false);
        animated(&mut anim, "warrior_asia", false);
        let mut u = Unit::default();
        for i in 0..4600i32 {
            let mut w = unit(i + 1, "UNIT_WARRIOR", i % 200, i / 200, false);
            w.owner = (i % 2) as i8;
            u.upsert(w);
        }
        u.refresh_b2(&b2(&art, &owners));
        u.sync_anim(&mut anim);
        let mut t = 0.0f32;
        for _ in 0..20 {
            anim.step(t);
            t += 0.1;
        }
        println!("attached {}", anim.stats().attached);
        let median = |mut v: Vec<u128>| {
            v.sort_unstable();
            v[v.len() / 2]
        };
        let (mut sync, mut build, mut step) = (vec![], vec![], vec![]);
        // BENCH-BUILD-SETUP
        let mut bufs = FrameBufs::default();
        for _ in 0..200 {
            let s = Instant::now();
            u.sync_anim(&mut anim);
            sync.push(s.elapsed().as_micros());
            let s = Instant::now();
            // BENCH-BUILD
            u.fill_instances(&anim, &owners, &mut bufs);
            let (b, _s) = u.take_batches(&mut bufs);
            std::hint::black_box(&b);
            u.recycle(b, &mut bufs);
            build.push(s.elapsed().as_micros());
            let s = Instant::now();
            anim.step(t);
            step.push(s.elapsed().as_micros());
            t += dt;
        }
        println!(
            "bench {label} dt={:.1}ms figures=13800 sync_anim={}us instances={}us anim_step={}us ticks={}",
            dt * 1000.0,
            median(sync),
            median(build),
            median(step),
            anim.stats().fsm_ticks
        );
    }
}
