//! `world-anim`: the idle animations of the world. Everything that moves in
//! place plays here: the squads of units, the landmarks, the wonders, the models of the path and
//! of effects. The poses are baked clips in the bone texture ([`clip_cache`]); an instance picks
//! frames by number ([`BoneRef`]); the skinning vertex shaders (`shaders/skin/`) read them.
//!
//! The service is not a subsystem (`RenderSystem`) but a part of the world that the subsystems
//! share (`WorldServices.anim`). Its `prepare` runs first in the `Prepare` pass.
//!
//! What is here: the data of a model (`data`), the program of the graph of a state (`graph`), the
//! cache of clips (`clip_cache`), the references (`bone_ref`), the counters and the report
//! (`stats`), the automaton (`fsm`, WAN4: states, transitions, path options, 30 Hz, one per
//! figure) and [`AnimService`]. A figure whose model has a machine is driven by it; everything else
//! (landmarks, features, wonders, models without a machine) plays the clip [`clip_for`] picks and a
//! change of the state cross-fades to the new one.
//!
//! No clip of attack, move, death, victory or capture is ever asked for (BUILD 5, R-D7): the
//! requested action is checked in [`clip_for`], and a code outside the three standing ones plays
//! the idle clip.

pub mod bone_ref;
pub mod clip_cache;
pub mod data;
pub mod dge;
pub mod fsm;
pub mod fx;
pub mod gpu_clock;
pub mod graph;
pub mod landmark_tree;
pub mod stats;

#[cfg(test)]
mod gpu_tests;

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::Arc;

use asset_format::LogicalId;

use fx::FxHashMap;

pub use bone_ref::BoneRef;
pub use clip_cache::{BoneSlots, ClipCache, ClipKey, MemoryBones};
pub use data::{AnimDataError, ClipEntry, ClipTable, ModelAnim};
pub use fsm::Fsm;
pub use graph::{GraphProgram, MachineDef, PathOptions};
pub use stats::{AnimStats, CoverageReport};

use bone_ref::clip_time;
use data::{
    EC_FEATURE_IDLE, EC_FEATURE_WORKED, EC_FORTIFY, EC_IDLE_A, EC_WORK_FIRST, EC_WORK_LAST,
};
use graph::CROSSFADE_S;

use crate::entities::{CityKey, UnitKey};

// ---- a small generator (WA-D3) ----

/// The generator of one animated thing (splitmix64): its own stream per target, so that the choice
/// of a clip does not depend on the order of the others.
// SIMPLIFIED(SIMP-049): the original draws from the engine's `FRandom`, whose stream depends on the
// order of every earlier call; ours has the same weights and its own stream per figure.
#[derive(Debug, Clone)]
pub struct AnimRng(u64);

impl AnimRng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// In `[0, 1)`.
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}

fn mix(parts: &[u64]) -> u64 {
    let mut h = 0xCBF2_9CE4_8422_2325u64;
    for p in parts {
        h = (h ^ p).wrapping_mul(0x0000_0100_0000_01B3);
        h ^= h >> 29;
    }
    h
}

// ---- what is animated ----

/// What an animation belongs to. The same target always has the same handle (4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AnimTarget {
    Figure { unit: UnitKey, index: u8 },
    Landmark { cell: u32, slot: u16 },
    Wonder { city: CityKey, slot: u16 },
    PathNode { index: u16 },
    Fx { id: u32 },
}

impl AnimTarget {
    /// The seed of the generator of the target (stable across runs).
    pub fn seed(&self) -> u64 {
        match *self {
            Self::Figure { unit, index } => {
                mix(&[1, unit.0 as u64, unit.1 as u64, u64::from(index)])
            }
            Self::Landmark { cell, slot } => mix(&[2, u64::from(cell), u64::from(slot)]),
            Self::Wonder { city, slot } => mix(&[3, city.0 as u64, city.1 as u64, u64::from(slot)]),
            Self::PathNode { index } => mix(&[4, u64::from(index)]),
            Self::Fx { id } => mix(&[5, u64::from(id)]),
        }
    }

    /// The seed of a squad: the same for all its figures.
    fn squad_seed(&self) -> u64 {
        match *self {
            Self::Figure { unit, .. } => mix(&[6, unit.0 as u64, unit.1 as u64]),
            _ => self.seed(),
        }
    }
}

/// The state of a landmark (the rows of the decision tree are keyed by these names).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LandmarkState {
    #[default]
    Constructed,
    RawResource,
    UnderConstruction,
    Pillaged,
}

impl LandmarkState {
    /// The name in the `.ddtxml` of the game.
    pub fn name(self) -> &'static str {
        match self {
            Self::RawResource => "RawResource",
            Self::UnderConstruction => "UnderConstruction",
            Self::Constructed => "Constructed",
            Self::Pillaged => "Pillaged",
        }
    }
}

/// What an object asks of its animation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnimState {
    /// The requested action (path option 4): 1000 idle, 1500 fortify and sleep, 1910..=1960 work.
    pub action: u32,
    pub landmark: LandmarkState,
    /// A feature on a cell that a city works (clip `ec` 101 instead of 100).
    pub worked: bool,
}

impl Default for AnimState {
    fn default() -> Self {
        Self {
            action: EC_IDLE_A,
            landmark: LandmarkState::default(),
            worked: false,
        }
    }
}

/// The three actions `world-objects` asks for (WA-D4), by name.
pub const EC_IDLE_ACTION: u32 = EC_IDLE_A;
pub const EC_FORTIFY_ACTION: u32 = EC_FORTIFY;
/// The first clip of the work of a worker; the one a build asks for is [`work_action_of_build`].
pub const EC_WORK_ACTION: u32 = EC_WORK_FIRST;

/// The action a worker asks for while it works, by the name of the build (RW-14 (k): the engine
/// posts RAKE for farms, PICKAXE for mines and quarries, SHOVEL for roads, railroads and the
/// clearing of marsh, fallout and routes, AXE for forest and jungle, HAMMER for the rest; the
/// numbers are `FSMActionValues.xml`: 1910 shovel, 1920 rake, 1930 axe, 1940 pickaxe, 1950 hammer).
pub fn work_action_of_build_name(name: &str) -> u32 {
    match name {
        "BUILD_FARM" | "BUILD_TERRACE_FARM" | "BUILD_POLDER" => 1920,
        "BUILD_MINE" | "BUILD_QUARRY" => 1940,
        "BUILD_ROAD"
        | "BUILD_RAILROAD"
        | "BUILD_SCRUB_FALLOUT"
        | "BUILD_REMOVE_MARSH"
        | "BUILD_REMOVE_ROUTE" => 1910,
        "BUILD_REMOVE_FOREST" | "BUILD_REMOVE_JUNGLE" => 1930,
        _ => 1950,
    }
}

/// The `BuildTypes` of the game: the rows of the `Builds` table of the game database (Brave New
/// World with Lekmod) in table order, which is the index the core uses (the `ID` column has gaps at
/// 45 and 46; the core counts rows). Checked against `gamedata-b2` (B2 RP23). A build the table does
/// not know asks for the hammer.
const BUILD_NAMES: [&str; 68] = [
    "BUILD_ROAD",
    "BUILD_RAILROAD",
    "BUILD_FARM",
    "BUILD_MINE",
    "BUILD_TRADING_POST",
    "BUILD_LUMBERMILL",
    "BUILD_PASTURE",
    "BUILD_CAMP",
    "BUILD_PLANTATION",
    "BUILD_QUARRY",
    "BUILD_WELL",
    "BUILD_OFFSHORE_PLATFORM",
    "BUILD_FISHING_BOATS",
    "BUILD_FORT",
    "BUILD_REMOVE_JUNGLE",
    "BUILD_REMOVE_FOREST",
    "BUILD_REMOVE_MARSH",
    "BUILD_SCRUB_FALLOUT",
    "BUILD_REPAIR",
    "BUILD_REMOVE_ROUTE",
    "BUILD_LANDMARK",
    "BUILD_ACADEMY",
    "BUILD_CUSTOMS_HOUSE",
    "BUILD_MANUFACTORY",
    "BUILD_CITADEL",
    "BUILD_TERRACE_FARM",
    "BUILD_MOAI",
    "BUILD_HOLY_SITE",
    "BUILD_POLDER",
    "BUILD_ARCHAEOLOGY_DIG",
    "BUILD_BRAZILWOOD_CAMP",
    "BUILD_KASBAH",
    "BUILD_FEITORIA",
    "BUILD_CHATEAU",
    "BUILD_FISHING_BOATS_NO_KILL",
    "BUILD_MOTTE",
    "BUILD_TEPE",
    "BUILD_SACRED_GROVE",
    "BUILD_NELA",
    "BUILD_AYER",
    "BUILD_GOTH",
    "BUILD_TIBET",
    "BUILD_PONTOON_BRIDGE",
    "BUILD_DOCK",
    "BUILD_FISHING_BOATS_AUSTRALIA_NO_KILL",
    "BUILD_MC_SCOTTISH_CLAN_CASTLE",
    "BUILD_WATER_POLDER",
    "BUILD_ROCK_HEWN",
    "BUILD_RUBBER_PLANTATION",
    "BUILD_CONCERT",
    "BUILD_MC_MAORI_PA",
    "BUILD_CHILE_FISHING_BOATS",
    "BUILD_CHILE_DOCK",
    "BUILD_CANAL",
    "BUILD_REMOVE_JUNGLE_COLOMBIA",
    "BUILD_REMOVE_FOREST_COLOMBIA",
    "BUILD_REMOVE_MARSH_COLOMBIA",
    "BUILD_FARM_COLOMBIA",
    "BUILD_MINE_COLOMBIA",
    "BUILD_PASTURE_COLOMBIA",
    "BUILD_PLANTATION_COLOMBIA",
    "BUILD_QUARRY_COLOMBIA",
    "BUILD_WELL_COLOMBIA",
    "BUILD_BRUNEI_FISHING_BOATS",
    "BUILD_BRUNEI_OFFSHORE_PLATFORM",
    "BUILD_BRUNEI_PONTOON_BRIDGE",
    "BUILD_OFFSHORE_PLATFORM_WORKER",
    "BUILD_FISHING_BOATS_WORKER",
];

/// The name of a `BuildTypes` id (`BUILD_FARM`), `None` for an id the table does not know.
pub fn build_name(build: i16) -> Option<&'static str> {
    usize::try_from(build)
        .ok()
        .and_then(|i| BUILD_NAMES.get(i))
        .copied()
}

/// The action for `UnitState.building` (a `BuildTypes` id): 1910..=1950.
pub fn work_action_of_build(build: i16) -> u32 {
    usize::try_from(build)
        .ok()
        .and_then(|i| BUILD_NAMES.get(i))
        .map_or(1950, |n| work_action_of_build_name(n))
}

/// The actions the world asks for (WA-D4): idle, fortify and sleep, the work of a worker.
pub fn action_is_standing(action: u32) -> bool {
    action == EC_IDLE_A || action == EC_FORTIFY || (EC_WORK_FIRST..=EC_WORK_LAST).contains(&action)
}

/// The clip a state plays: index in the pack, its `ec`, the duration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Choice {
    pub clip: u16,
    pub ec: u32,
    pub duration: f32,
    /// The repeats and the pause of the row of a landmark's tree (`landmark_tree`).
    pub cycle: Option<landmark_tree::Cycle>,
}

fn choice_of(c: &ClipEntry) -> Choice {
    Choice {
        clip: c.index,
        ec: c.ec,
        duration: c.duration,
        cycle: None,
    }
}

/// The clip that plays for `state` on `model`; `None` if the model has no clip at all.
///
/// The clip of a figure without a machine, and of everything but figures: one clip in a loop, the
/// decision row of a landmark; the clip of the feature (100, 101); the steady clip of the graph of
/// the state that the action enters; the idle clip.
pub fn clip_for(model: &ModelAnim, state: &AnimState) -> Option<Choice> {
    clip_for_seeded(model, state, 0)
}

/// [`clip_for`] for a thing with a seed of its own: of the rows of a landmark's tree for the state
/// the seed picks one (`landmark_tree::pick_row`).
pub fn clip_for_seeded(model: &ModelAnim, state: &AnimState, seed: u64) -> Option<Choice> {
    let clips = &model.clips;
    // A landmark with a decision tree plays the row of its state.
    if let Some((row, c)) = landmark_tree::pick_row(model, state.landmark, seed) {
        return Some(Choice {
            cycle: landmark_tree::Cycle::of(row),
            ..choice_of(c)
        });
    }
    // A feature.
    if state.worked
        && let Some(c) = clips.by_ec(EC_FEATURE_WORKED)
    {
        return Some(choice_of(c));
    }
    if state.action == EC_IDLE_A
        && !clips.has_idle()
        && let Some(c) = clips.by_ec(EC_FEATURE_IDLE)
    {
        return Some(choice_of(c));
    }
    // A standing state of a unit.
    if state.action != EC_IDLE_A && action_is_standing(state.action) {
        if let Some((_, st)) = model
            .machine
            .as_ref()
            .and_then(|m| m.state_for_action(state.action))
        {
            let p = &st.program;
            let steady = p
                .cycle_from
                .and_then(|i| p.clips.get(usize::from(i)))
                .or_else(|| p.select(&PathOptions::default()));
            if let Some(g) = steady
                && let Some(c) = clips.by_index(g.clip)
            {
                return Some(choice_of(c));
            }
        } else if let Some(c) = clips.by_ec(state.action) {
            return Some(choice_of(c));
        }
    }
    clips.idle().map(choice_of)
}

// ---- the service ----

/// A handle on an animated thing. Stale handles (released, or from before `clear`) do nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AnimHandle {
    index: u32,
    generation: u32,
}

/// One clip being played: it began at `start_s` of the service clock (a phase puts it before the
/// moment of the attach).
#[derive(Debug, Clone, Copy)]
struct Play {
    key: ClipKey,
    ec: u32,
    duration: f32,
    start_s: f32,
    /// False for a clip that plays once and holds its last frame.
    looping: bool,
    /// The repeats and the pause of a landmark's row; `None` for a loop.
    cycle: Option<landmark_tree::Cycle>,
}

#[derive(Debug, Clone, Copy)]
struct Fade {
    from: Play,
    since_s: f32,
    /// Seconds of the crossfade and its curve (the graph's `BlendTime`, `Curve3`).
    blend_s: f32,
    s_curve: bool,
}

struct Instance {
    target: AnimTarget,
    model_key: u32,
    model: Arc<ModelAnim>,
    state: AnimState,
    play: Play,
    fade: Option<Fade>,
    /// The automaton of a figure whose model has a machine (WAN4).
    fsm: Option<Fsm>,
    last_ref: BoneRef,
    /// The clips `last_ref` points into (current and the one it fades from), kept while a new one
    /// waits for its bake.
    held: [Option<ClipKey>; 2],
    ready: bool,
    /// The reference has changed since the attach (the coverage test reads it).
    moved: bool,
    /// The first tick its machine may do more than count (`Fsm::next_wake`); 0: look next frame.
    wake: u64,
    /// In the list of the instances whose reference is made again every frame.
    active: bool,
    /// `last_ref` is a clocked one ([`gpu_clock`]): it stays until the play changes.
    clocked: bool,
}

/// The clips an instance asks the cache for every frame, kept apart from the instances so that
/// the walk over them is short.
#[derive(Debug, Clone, Copy, Default)]
struct Asks {
    live: bool,
    play: ClipKey,
    fade: Option<ClipKey>,
}

impl Asks {
    fn of(i: &Instance) -> Self {
        Self {
            live: true,
            play: i.play.key,
            fade: i.fade.map(|f| f.from.key),
        }
    }
}

fn play_of(model_key: u32, spec: fsm::PlaySpec) -> Play {
    Play {
        key: ClipKey {
            model: model_key,
            clip: spec.clip,
        },
        ec: spec.ec,
        duration: spec.duration,
        start_s: spec.start_s,
        looping: spec.looping,
        cycle: None,
    }
}

impl Play {
    /// The time in the clip at the service time `now`.
    fn time_at(&self, now: f32) -> f32 {
        match self.cycle {
            Some(c) => c.time(now - self.start_s, self.duration),
            None => clip_time(now - self.start_s, self.duration, self.looping),
        }
    }
}

impl Instance {
    /// Runs the ticks of the machine up to `tick_now` and takes the clip it chose last; a gap
    /// longer than [`fsm::MAX_CATCH_UP`] ticks (or a clock that went back) jumps. Returns the
    /// ticks it ran.
    ///
    /// With `sleep`, the machine was not called on the frames it slept through: its tick is then
    /// behind, and `last_frame` (the tick of the previous `step`) is where counting every frame
    /// would have left it. The quiet ticks are skipped (`Fsm::next_wake`), which is what stepping
    /// them does; `self.wake` is set to the next tick that is not quiet.
    fn step_machine(&mut self, tick_now: u64, last_frame: Option<u64>, sleep: bool) -> u32 {
        let (Some(f), Some(def)) = (self.fsm.as_mut(), self.model.machine.as_ref()) else {
            return 0;
        };
        let clips = &self.model.clips;
        if sleep && let Some(p) = last_frame.filter(|&p| p > f.tick()) {
            f.jump_to(p);
        }
        if f.tick() == tick_now {
            if sleep {
                self.wake = f.next_wake(def, clips);
            }
            return 0;
        }
        if tick_now < f.tick() || tick_now - f.tick() > fsm::MAX_CATCH_UP {
            f.jump_to(tick_now);
        }
        let mut change = None;
        let mut ran = 0;
        let mut wake = None;
        while f.tick() < tick_now {
            if sleep {
                let w = f.next_wake(def, clips);
                if w > tick_now {
                    f.jump_to(tick_now);
                    wake = Some(w);
                    break;
                }
                f.jump_to(w - 1);
            }
            f.step(def, clips);
            ran += 1;
            if let Some(spec) = f.take_change() {
                change = Some(spec);
            }
        }
        if sleep {
            self.wake = wake.unwrap_or_else(|| f.next_wake(def, clips));
        }
        if let Some(spec) = change {
            let new = play_of(self.model_key, spec);
            if new.key != self.play.key
                || new.start_s != self.play.start_s
                || new.looping != self.play.looping
            {
                if spec.fade {
                    self.fade = Some(Fade {
                        from: self.play,
                        since_s: spec.start_s.max(self.play.start_s),
                        blend_s: spec.blend_s,
                        s_curve: spec.s_curve,
                    });
                }
                self.play = new;
            }
        }
        ran
    }
}

struct Slot {
    generation: u32,
    instance: Option<Instance>,
}

struct ModelEntry {
    key: u32,
    anim: Arc<ModelAnim>,
}

/// The animation service: models, handles, the cache of clips. `S` is where the bones go.
pub struct AnimServiceOf<S: BoneSlots> {
    cache: ClipCache<S>,
    models: FxHashMap<LogicalId, ModelEntry>,
    next_model: u32,
    slots: Vec<Slot>,
    free: Vec<u32>,
    by_target: FxHashMap<AnimTarget, AnimHandle>,
    now_s: f32,
    time_override: Option<f32>,
    /// Bumped when the set of models changes (register, clear): the owners of handles attach again
    /// only then, not every frame.
    models_revision: u64,
    /// The last `step`: microseconds (native only, 0 in the browser) and ticks of the automata.
    last_step_us: u64,
    last_ticks: u32,
    /// Parallel to `slots`: the clips each one asks for.
    asks: Vec<Asks>,
    /// Machines skip their quiet ticks (packet F; off only to compare with the old walk).
    sleep: bool,
    /// The machines by the tick they wake at: (wake, slot, generation); stale entries are skipped.
    wakes: BinaryHeap<Reverse<(u64, u32, u32)>>,
    /// The tick of the last `step`.
    last_tick: Option<u64>,
    /// The clock of the GPU, when the instances are drawn with clocked references.
    clock: Option<gpu_clock::GpuClock>,
    /// Slots whose reference is made again every frame (all of them without the clock).
    active: Vec<u32>,
    /// The revision of the cache the references were last made for.
    seen_revision: Option<u64>,
}

/// The service of the world. Its bones go to the CPU half of the bone texture (`skin::BoneStore`);
/// `WorldServices::prepare` sends them to the texture every frame.
pub type AnimService = AnimServiceOf<crate::skin::BoneStore>;

impl<S: BoneSlots + Default> Default for AnimServiceOf<S> {
    fn default() -> Self {
        Self::new(S::default())
    }
}

/// The golden ratio's fraction: figures of a squad are `0.618..` of a clip apart, so that twelve of
/// them never stand in the same frame (WA-D11).
const PHASE_STEP: f32 = 0.618_034;

impl<S: BoneSlots> AnimServiceOf<S> {
    pub fn new(store: S) -> Self {
        let mut cache = ClipCache::new(store);
        cache.reserve_identity();
        Self {
            cache,
            models: FxHashMap::default(),
            next_model: 0,
            slots: Vec::new(),
            free: Vec::new(),
            by_target: FxHashMap::default(),
            now_s: 0.0,
            time_override: None,
            models_revision: 0,
            last_step_us: 0,
            last_ticks: 0,
            asks: Vec::new(),
            sleep: true,
            wakes: BinaryHeap::new(),
            last_tick: None,
            clock: None,
            active: Vec::new(),
            seen_revision: None,
        }
    }

    /// Off: every machine walks every tick, as before packet F (the tests compare the two).
    pub fn set_fsm_sleep(&mut self, on: bool) {
        self.sleep = on;
        // Every machine looks again at the next step.
        self.last_tick = None;
    }

    /// Draws the steady instances with clocked references ([`gpu_clock`]): their reference is
    /// made once per play and the vertex shader runs the time. Takes the block of the clock from
    /// the store the first time; `false` if the store has no room for it.
    pub fn set_gpu_clock(&mut self, on: bool) -> bool {
        if on && self.clock.is_none() {
            let Some(block) = self.cache.reserve_block(gpu_clock::CLOCK_BLOCK) else {
                return false;
            };
            self.clock = Some(gpu_clock::GpuClock::new(block));
        }
        if !on {
            // The block stays taken (the slots after it keep their places); a later `on` takes
            // a new one, which is rare and only costs slots.
            self.clock = None;
        }
        // Every reference is made again at the next step.
        self.seen_revision = None;
        true
    }

    /// True when the instances are drawn with clocked references.
    pub fn gpu_clock(&self) -> bool {
        self.clock.is_some()
    }

    /// The reference of `h` as the shader sees it at the time `now` (a clocked one decoded): what
    /// the CPU walk would give. For the tests and the readers of a pose on the CPU.
    pub fn resolved_ref(&self, h: AnimHandle, now: f32) -> BoneRef {
        let r = self.bone_ref(h);
        match &self.clock {
            Some(c) => c.decode(r, now),
            None => r,
        }
    }

    /// Runs the machine of slot `index` up to `tick_now` and plans when it wakes; a new play makes
    /// the reference active. Returns the ticks it ran.
    fn run_machine(&mut self, index: u32, tick_now: u64) -> u32 {
        let s = &mut self.slots[index as usize];
        let Some(inst) = s.instance.as_mut() else {
            return 0;
        };
        let before = (
            inst.play.key,
            inst.play.start_s.to_bits(),
            inst.fade.is_some(),
        );
        let ran = inst.step_machine(tick_now, self.last_tick, self.sleep);
        let after = (
            inst.play.key,
            inst.play.start_s.to_bits(),
            inst.fade.is_some(),
        );
        if self.sleep {
            self.wakes.push(Reverse((inst.wake, index, s.generation)));
        }
        if before != after {
            self.sync_asks(index);
            self.activate(index);
        }
        ran
    }

    /// Puts slot `index` in the list of the references made every frame.
    fn activate(&mut self, index: u32) {
        if let Some(inst) = self.slots[index as usize].instance.as_mut()
            && !inst.active
        {
            inst.active = true;
            inst.clocked = false;
            self.active.push(index);
        }
    }

    /// Copies what slot `index` asks the cache for into `asks`.
    fn sync_asks(&mut self, index: u32) {
        let i = index as usize;
        if self.asks.len() <= i {
            self.asks.resize(i + 1, Asks::default());
        }
        self.asks[i] = match self.slots[i].instance.as_ref() {
            Some(inst) => Asks::of(inst),
            None => Asks::default(),
        };
    }

    /// Changes when a model is registered or the service is cleared. A thing that has no handle
    /// because its model was not there asks again when this moves.
    pub fn models_revision(&self) -> u64 {
        self.models_revision
    }

    pub fn cache(&self) -> &ClipCache<S> {
        &self.cache
    }

    pub fn cache_mut(&mut self) -> &mut ClipCache<S> {
        &mut self.cache
    }

    /// Makes a model known (its blobs arrived). A model of the same id replaces the old one.
    pub fn register_model(&mut self, id: LogicalId, anim: ModelAnim) {
        let key = self.next_model;
        self.next_model += 1;
        self.models_revision += 1;
        if let Some(old) = self.models.insert(
            id,
            ModelEntry {
                key,
                anim: Arc::new(anim),
            },
        ) {
            self.cache.drop_model(old.key);
        }
    }

    pub fn has_model(&self, id: &LogicalId) -> bool {
        self.models.contains_key(id)
    }

    pub fn model(&self, id: &LogicalId) -> Option<&Arc<ModelAnim>> {
        self.models.get(id).map(|m| &m.anim)
    }

    /// `WA-D12`: freezes the clock at `t` (`?anim=freeze`, `golden`); `None` lets it run.
    pub fn set_time_override(&mut self, t: Option<f32>) {
        self.time_override = t;
    }

    /// The clock the next `prepare` uses.
    pub fn set_time(&mut self, now_s: f32) {
        self.now_s = now_s;
    }

    fn slot(&self, h: AnimHandle) -> Option<&Instance> {
        let s = self.slots.get(h.index as usize)?;
        if s.generation != h.generation {
            return None;
        }
        s.instance.as_ref()
    }

    /// Starts the animation of `target` on `model`. `None` if the model is not known or has no
    /// clips. The same target again gives the same handle (a unit that moved, a figure that left
    /// the view and came back keeps its clip).
    pub fn attach(&mut self, model: &LogicalId, target: AnimTarget) -> Option<AnimHandle> {
        let entry = self.models.get(model)?;
        let (model_key, anim) = (entry.key, Arc::clone(&entry.anim));
        if let Some(&h) = self.by_target.get(&target) {
            if self.slot(h).is_some_and(|i| i.model_key == model_key) {
                return Some(h);
            }
            // The target changed its model: the old clip is of no use.
            self.release(h);
        }
        let state = AnimState::default();
        let choice = clip_for_seeded(&anim, &state, target.seed())?;

        // The phase (WA-D11): a random place in the clip; the figures of a squad one golden step
        // apart from a random start, so that none stands in the frame of another.
        let mut squad = AnimRng::new(target.squad_seed());
        let mut own = AnimRng::new(target.seed());
        let u = match target {
            AnimTarget::Figure { index, .. } => {
                (squad.unit() + f32::from(index) * PHASE_STEP).fract()
            }
            _ => own.unit(),
        };
        let now = self.time_override.unwrap_or(self.now_s);
        let play = Play {
            key: ClipKey {
                model: model_key,
                clip: choice.clip,
            },
            ec: choice.ec,
            duration: choice.duration,
            start_s: now - u * choice.duration,
            looping: true,
            cycle: choice.cycle,
        };
        // A figure with a machine is driven by it: it enters its default state now, with the
        // phase `u` (SIMP-049: its own stream, from the key of the figure).
        let mut play = play;
        let mut fsm = None;
        let mut wake = 0;
        if matches!(target, AnimTarget::Figure { .. })
            && let Some(def) = anim.machine.as_ref().filter(|d| !d.states.is_empty())
        {
            let mut f = Fsm::new(target.seed(), fsm::tick_of(now));
            f.start(def, &anim.clips, u);
            if let Some(spec) = f.take_change() {
                play = play_of(model_key, spec);
            }
            wake = f.next_wake(def, &anim.clips);
            fsm = Some(f);
        }
        let has_fsm = fsm.is_some();
        self.cache.request(play.key, &anim);
        let instance = Instance {
            target,
            model_key,
            model: anim,
            state,
            play,
            fade: None,
            fsm,
            last_ref: self.cache.identity_ref(),
            held: [None; 2],
            ready: false,
            moved: false,
            wake,
            active: false,
            clocked: false,
        };
        let index = match self.free.pop() {
            Some(i) => {
                self.slots[i as usize].instance = Some(instance);
                i
            }
            None => {
                self.slots.push(Slot {
                    generation: 0,
                    instance: Some(instance),
                });
                (self.slots.len() - 1) as u32
            }
        };
        let h = AnimHandle {
            index,
            generation: self.slots[index as usize].generation,
        };
        self.by_target.insert(target, h);
        if has_fsm {
            self.wakes.push(Reverse((wake, index, h.generation)));
        }
        self.sync_asks(index);
        self.activate(index);
        Some(h)
    }

    /// Changes what the thing asks of its animation. A change of clip cross-fades to the new one;
    /// the same clip goes on without a restart.
    pub fn set_state(&mut self, h: AnimHandle, state: AnimState) {
        let now = self.time_override.unwrap_or(self.now_s);
        let Some(inst) = self
            .slots
            .get_mut(h.index as usize)
            .filter(|s| s.generation == h.generation)
            .and_then(|s| s.instance.as_mut())
        else {
            return;
        };
        inst.state = state;
        if let Some(f) = inst.fsm.as_mut() {
            // The machine reacts on its next tick (WA-D4). Only the standing actions get through
            // (SIMP-010: no clips of moving or attacking); anything else asks for idle.
            f.request(if action_is_standing(state.action) {
                state.action
            } else {
                EC_IDLE_A
            });
            // It looks at the request on its next tick.
            inst.wake = 0;
            self.wakes.push(Reverse((0, h.index, h.generation)));
            return;
        }
        let Some(choice) = clip_for_seeded(&inst.model, &state, inst.target.seed()) else {
            return;
        };
        let key = ClipKey {
            model: inst.model_key,
            clip: choice.clip,
        };
        if key == inst.play.key {
            return;
        }
        inst.fade = Some(Fade {
            from: inst.play,
            since_s: now,
            blend_s: CROSSFADE_S,
            s_curve: false,
        });
        inst.play = Play {
            key,
            ec: choice.ec,
            duration: choice.duration,
            start_s: now,
            looping: true,
            cycle: choice.cycle,
        };
        let model = Arc::clone(&inst.model);
        self.cache.request(key, &model);
        self.sync_asks(h.index);
        self.activate(h.index);
    }

    /// The frames of this frame's pose. Before the clip is baked it is the pose the thing had
    /// while that clip is still in the texture, otherwise the bind pose (every bone the identity):
    /// the thing is drawn either way. See [`Self::is_ready`].
    pub fn bone_ref(&self, h: AnimHandle) -> BoneRef {
        self.slot(h).map(|i| i.last_ref).unwrap_or_default()
    }

    /// True once the clip is in the bone texture. A thing whose clip is not ready is still drawn,
    /// in the pose of the bind or the frozen last one, until the bake (it used to be left out,
    /// and a full texture hid it for good).
    pub fn is_ready(&self, h: AnimHandle) -> bool {
        self.slot(h).is_some_and(|i| i.ready)
    }

    /// True if the reference of the handle has changed since the attach.
    pub fn has_moved(&self, h: AnimHandle) -> bool {
        self.slot(h).is_some_and(|i| i.moved)
    }

    /// The `ec` of the clip the handle plays, for the tests and the report.
    pub fn playing_ec(&self, h: AnimHandle) -> Option<u32> {
        self.slot(h).map(|i| i.play.ec)
    }

    /// The `ec` of the clip the handle plays and the time in it at the service's clock (seconds from the clip's
    /// start, wrapped for a loop): the sound rows of the clip's timed triggers fire as it passes them (re-b15 ITEM 4).
    pub fn clip_time(&self, h: AnimHandle) -> Option<(u32, f32)> {
        let now = self.time_override.unwrap_or(self.now_s);
        self.slot(h).map(|i| (i.play.ec, i.play.time_at(now)))
    }

    pub fn is_live(&self, h: AnimHandle) -> bool {
        self.slot(h).is_some()
    }

    /// Ends the animation. The baked clips stay in the cache until they are evicted.
    pub fn release(&mut self, h: AnimHandle) {
        let Some(s) = self.slots.get_mut(h.index as usize) else {
            return;
        };
        if s.generation != h.generation {
            return;
        }
        if let Some(inst) = s.instance.take() {
            self.by_target.remove(&inst.target);
            s.generation = s.generation.wrapping_add(1);
            self.free.push(h.index);
            if let Some(a) = self.asks.get_mut(h.index as usize) {
                a.live = false;
            }
        }
    }

    /// Drops everything: models, handles, the cache (a new game).
    pub fn clear(&mut self) {
        self.models_revision += 1;
        let mut handles = Vec::new();
        for (i, s) in self.slots.iter().enumerate() {
            if s.instance.is_some() {
                handles.push(AnimHandle {
                    index: i as u32,
                    generation: s.generation,
                });
            }
        }
        for h in handles {
            self.release(h);
        }
        let keys: Vec<u32> = self.models.values().map(|m| m.key).collect();
        for k in keys {
            self.cache.drop_model(k);
        }
        self.models.clear();
    }

    /// The first step of a frame (4.2): asks the cache for the clips in use, bakes what is queued
    /// within the budget, and sets the references of this frame.
    pub fn step(&mut self, now_s: f32) {
        let now = self.time_override.unwrap_or(now_s);
        self.now_s = now_s;
        #[cfg(not(target_arch = "wasm32"))]
        let started = std::time::Instant::now();
        self.cache.begin_frame();
        // The automata, 30 ticks a second whatever the frame rate (WA-D6). A machine that sleeps
        // (its next ticks are quiet, `Fsm::next_wake`) is not looked at until it wakes; a frame
        // that jumps (the clock went back, or more than `MAX_CATCH_UP` ticks) looks at all.
        let tick_now = fsm::tick_of(now);
        let mut ticks = 0u32;
        let steady = self.sleep
            && self
                .last_tick
                .is_some_and(|p| tick_now >= p && tick_now - p <= fsm::MAX_CATCH_UP);
        if steady {
            while let Some(&Reverse((w, i, g))) = self.wakes.peek() {
                if w > tick_now {
                    break;
                }
                self.wakes.pop();
                let s = &self.slots[i as usize];
                if s.generation == g && s.instance.as_ref().is_some_and(|inst| inst.wake == w) {
                    ticks += self.run_machine(i, tick_now);
                }
            }
        } else {
            self.wakes.clear();
            for i in 0..self.slots.len() as u32 {
                if self.slots[i as usize]
                    .instance
                    .as_ref()
                    .is_some_and(|inst| inst.fsm.is_some())
                {
                    ticks += self.run_machine(i, tick_now);
                }
            }
        }
        self.last_tick = Some(tick_now);
        // The request of the clips in use, in the order of the slots. A request only marks the
        // clip used this frame, so the same clip as the one before it is not asked again.
        let mut last_request: Option<ClipKey> = None;
        for (i, a) in self.asks.iter().enumerate() {
            if !a.live {
                continue;
            }
            let model = |slots: &[Slot]| {
                slots[i]
                    .instance
                    .as_ref()
                    .map(|inst| Arc::clone(&inst.model))
            };
            if last_request != Some(a.play) {
                if !self.cache.touch(a.play)
                    && let Some(m) = model(&self.slots)
                {
                    self.cache.request(a.play, &m);
                }
                last_request = Some(a.play);
            }
            if let Some(from) = a.fade {
                if !self.cache.touch(from)
                    && let Some(m) = model(&self.slots)
                {
                    self.cache.request(from, &m);
                }
                last_request = None;
            }
        }
        self.cache.bake_pending();
        // The references. Without the clock, all of them every frame; with it, only the active
        // ones (waiting for a bake, in a cross-fade, a new play), unless the cache moved a clip.
        let revision = self.cache.revision();
        let full = self.clock.is_none() || self.seen_revision != Some(revision);
        if let Some(clock) = self.clock.as_mut() {
            let slot = clock.clock_slot();
            self.cache
                .store_mut()
                .write(slot..slot + 1, &[gpu_clock::GpuClock::clock_bone(now)]);
            if full {
                let cache = &self.cache;
                clock.prune(|k| {
                    cache
                        .get(k)
                        .map(|c| (c.range.start, c.bones, c.frames, c.fps.to_bits()))
                });
            }
        }
        self.seen_revision = Some(revision);
        let list: Vec<u32> = if full {
            (0..self.slots.len() as u32).collect()
        } else {
            std::mem::take(&mut self.active)
        };
        let mut still = Vec::new();
        let identity = self.cache.identity_ref();
        // The clip the previous instance played, by value: the instances of a model share a few.
        let mut memo: Option<(ClipKey, Option<clip_cache::ClipView>)> = None;
        for i in list {
            let Some(inst) = self.slots[i as usize].instance.as_mut() else {
                continue;
            };
            if !full && !inst.active {
                continue;
            }
            let cur = match memo {
                Some((k, v)) if k == inst.play.key => v,
                _ => {
                    let v = self.cache.get(inst.play.key).map(|c| c.view());
                    memo = Some((inst.play.key, v));
                    v
                }
            };
            let Some(cur) = cur else {
                inst.ready = false;
                inst.clocked = false;
                inst.active = true;
                still.push(i);
                // The clip waits for its bake. The pose of the clips that were shown stays while
                // they are in the texture (they are touched, so they are not evicted); otherwise
                // the bind pose.
                let held = inst.held;
                let alive = held.iter().flatten().all(|k| self.cache.touch(*k))
                    && held.iter().any(Option::is_some);
                if !alive {
                    inst.last_ref = identity;
                    inst.held = [None; 2];
                }
                continue;
            };
            let t = inst.play.time_at(now);
            let mut r = cur.bone_ref(t);
            let mut held = [Some(inst.play.key), None];
            if let Some(f) = inst.fade {
                let x = (now - f.since_s) / f.blend_s;
                if x >= 1.0 || !x.is_finite() {
                    inst.fade = None;
                } else if let Some(from) = self.cache.get(f.from.key) {
                    // SIMP-050: the nearest frame of each clip, and the share of the fade.
                    let ta = f.from.time_at(now);
                    let a = from.bone_ref(ta);
                    let nearest = |r: BoneRef| if r.blend < 0.5 { r.base } else { r.next };
                    r = BoneRef {
                        base: nearest(a),
                        next: nearest(r),
                        blend: if f.s_curve {
                            dge::s_curve(x)
                        } else {
                            x.max(0.0)
                        },
                    };
                    held[1] = Some(f.from.key);
                } else {
                    // The clip it fades from was evicted: no fade, the new clip at once.
                    inst.fade = None;
                }
                if inst.fade.is_none() {
                    self.asks[i as usize].fade = None;
                }
            }
            // A steady play: the reference of its header, the shader runs the time.
            let mut clocked = false;
            if inst.fade.is_none()
                && let Some(clock) = self.clock.as_mut()
                && let Some(baked) = self.cache.get(inst.play.key).cloned()
            {
                let p = &inst.play;
                let kind = gpu_clock::PlayKind::new(p.key, p.looping, p.duration, p.cycle);
                let store = self.cache.store_mut();
                if let Some(header) = clock.header_for(kind, &baked, |r, b| store.write(r, b)) {
                    r = clock.encode(header, p.start_s);
                    clocked = true;
                }
            }
            if r != inst.last_ref || clocked {
                inst.moved = inst.moved || inst.ready;
            }
            inst.last_ref = r;
            inst.held = held;
            inst.ready = true;
            inst.clocked = clocked;
            // Without the clock every instance is made every frame: none is listed.
            inst.active = !clocked;
            if !clocked && self.clock.is_some() {
                still.push(i);
            }
        }
        if !full {
            still.sort_unstable();
            still.dedup();
        }
        if full {
            self.active.clear();
        }
        self.active.extend(still);
        self.last_ticks = ticks;
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.last_step_us = started.elapsed().as_micros() as u64;
        }
    }

    /// The counters for `?stats=1`.
    pub fn stats(&self) -> AnimStats {
        let used = self.cache.store().used_slots();
        let report = self.cache.last_report();
        let (baked, evicted, failed) = self.cache.totals();
        AnimStats {
            models: self.models.len() as u32,
            attached: self.by_target.len() as u32,
            clips_cached: self.cache.clips_cached() as u32,
            clips_pending: self.cache.clips_pending() as u32,
            used_slots: used,
            rows: stats::rows_for(used),
            baked_frame: report.baked,
            evicted_frame: report.evicted,
            uploaded_frame: report.bytes,
            baked_total: baked,
            evicted_total: evicted,
            failed_total: failed,
            step_us: self.last_step_us,
            fsm_ticks: u64::from(self.last_ticks),
        }
    }

    /// Unfinished work: clips waiting for their bake (`Renderer::settled`).
    pub fn pending_work(&self) -> u32 {
        self.cache.clips_pending() as u32
    }
}

impl AnimService {
    /// The call `WorldServices::prepare` makes first in the `Prepare` pass: the CPU side. The upload
    /// of the bones to the texture is `WorldServices`'s (`BoneTexture::flush_from`).
    pub fn prepare(&mut self, _device: &wgpu::Device, _queue: &wgpu::Queue) {
        self.step(self.now_s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use data::fixtures;

    fn service_with(names: &[&str], bones: usize, fsm: bool) -> AnimServiceOf<MemoryBones> {
        let mut s = AnimServiceOf::new(MemoryBones::new(100_000));
        for n in names {
            let (m, pack) = fixtures::model(n, bones);
            let machine = fixtures::omni_machine();
            let a = ModelAnim::new(&m, Some(pack), fsm.then_some(&machine)).unwrap();
            s.register_model(LogicalId::new(*n), a);
        }
        s
    }

    fn id(n: &str) -> LogicalId {
        LogicalId::new(n)
    }

    fn figure(i: u8) -> AnimTarget {
        AnimTarget::Figure {
            unit: (0, 42),
            index: i,
        }
    }

    fn run(s: &mut AnimServiceOf<MemoryBones>, from: f32, to: f32) {
        let mut t = from;
        while t < to {
            s.step(t);
            t += 1.0 / 60.0;
        }
    }

    #[test]
    fn a_model_without_clips_or_unknown_is_not_attached() {
        let mut s = service_with(&["warrior"], 3, false);
        assert!(s.attach(&id("nobody"), figure(0)).is_none());
        let still = ModelAnim::new(
            &asset_format::ModelBlobV2 {
                source: "wall".into(),
                skeleton: fixtures::skeleton(1),
                ..Default::default()
            },
            None,
            None,
        )
        .unwrap();
        s.register_model(id("wall"), still);
        assert!(s.attach(&id("wall"), figure(1)).is_none());
        assert!(s.attach(&id("warrior"), figure(2)).is_some());
    }

    /// WAN2: an instance plays the idle clip in a loop and its reference changes.
    #[test]
    fn a_figure_loops_its_idle_clip() {
        let mut s = service_with(&["warrior"], 3, false);
        let h = s.attach(&id("warrior"), figure(0)).unwrap();
        assert!(!s.is_ready(h), "not before the first step");
        s.step(0.0);
        assert!(s.is_ready(h));
        assert_eq!(s.playing_ec(h), Some(1000));
        let first = s.bone_ref(h);
        let baked = s
            .cache()
            .get(ClipKey { model: 0, clip: 0 })
            .unwrap()
            .clone();
        let inside = |r: BoneRef| {
            (baked.range.start..baked.range.end).contains(&r.base)
                && (baked.range.start..baked.range.end).contains(&r.next)
        };
        assert!(inside(first));
        // Over two clip lengths the reference moves, stays inside the clip, and blends in [0, 1].
        let mut seen = std::collections::BTreeSet::new();
        let mut t = 0.0;
        while t < 4.0 {
            s.step(t);
            let r = s.bone_ref(h);
            assert!(inside(r), "{r:?}");
            assert!((0.0..=1.0).contains(&r.blend));
            seen.insert(r.base);
            t += 1.0 / 60.0;
        }
        assert!(
            seen.len() > 55,
            "it passes through the frames: {}",
            seen.len()
        );
        assert!(s.has_moved(h));
    }

    /// WAN2 test of the spec: the phases of twelve figures of a squad differ.
    #[test]
    fn twelve_figures_are_in_twelve_frames() {
        let mut s = service_with(&["warrior"], 3, false);
        let hs: Vec<AnimHandle> = (0..12)
            .map(|i| s.attach(&id("warrior"), figure(i)).unwrap())
            .collect();
        s.step(0.0);
        let mut bases: Vec<u32> = hs.iter().map(|&h| s.bone_ref(h).base).collect();
        bases.sort_unstable();
        bases.dedup();
        assert_eq!(bases.len(), 12, "{bases:?}");
        // And another squad has other phases.
        let other = AnimTarget::Figure {
            unit: (1, 7),
            index: 0,
        };
        let h = s.attach(&id("warrior"), other).unwrap();
        s.step(0.0);
        let _ = h;
    }

    #[test]
    fn the_same_target_has_the_same_handle_until_it_is_released() {
        let mut s = service_with(&["warrior"], 3, false);
        let a = s.attach(&id("warrior"), figure(0)).unwrap();
        s.step(0.0);
        s.run_to_move(a);
        let again = s.attach(&id("warrior"), figure(0)).unwrap();
        assert_eq!(a, again);
        assert_eq!(s.stats().attached, 1);
        s.release(a);
        assert!(!s.is_live(a));
        assert_eq!(s.bone_ref(a), BoneRef::default());
        s.set_state(a, AnimState::default());
        s.release(a);
        let b = s.attach(&id("warrior"), figure(0)).unwrap();
        assert_ne!(a, b, "a stale handle stays dead");
        assert!(s.is_live(b) && !s.is_live(a));
    }

    impl AnimServiceOf<MemoryBones> {
        fn run_to_move(&mut self, h: AnimHandle) {
            let mut t = 0.0;
            while !self.has_moved(h) && t < 3.0 {
                self.step(t);
                t += 1.0 / 60.0;
            }
        }
    }

    #[test]
    fn a_build_asks_for_its_tool() {
        assert_eq!(work_action_of_build(0), 1910, "road: shovel");
        assert_eq!(work_action_of_build(2), 1920, "farm: rake");
        assert_eq!(work_action_of_build(3), 1940, "mine: pickaxe");
        assert_eq!(work_action_of_build(15), 1930, "remove forest: axe");
        assert_eq!(work_action_of_build(5), 1950, "other: hammer");
        assert_eq!(work_action_of_build(-1), 1950);
        assert!(action_is_standing(work_action_of_build(9999)));
    }

    #[test]
    fn a_moved_unit_goes_on_with_its_clip() {
        // The unit moves to another cell: the feed attaches the same key, and the clip is not
        // started again.
        let mut s = service_with(&["warrior"], 3, false);
        let h = s.attach(&id("warrior"), figure(3)).unwrap();
        run(&mut s, 0.0, 1.0);
        let before = s.bone_ref(h);
        let h2 = s.attach(&id("warrior"), figure(3)).unwrap();
        s.step(1.0);
        assert_eq!(h, h2);
        let after = s.bone_ref(h2);
        assert!(
            after.base.abs_diff(before.base) < 3 * 4,
            "{before:?} {after:?}"
        );
    }

    #[test]
    fn a_standing_action_changes_the_clip_with_a_fade() {
        let mut s = service_with(&["warrior"], 3, true);
        let h = s.attach(&id("warrior"), figure(0)).unwrap();
        run(&mut s, 0.0, 1.0);
        // The chooser of the machine starts a figure on idle or on a fidget.
        assert!(matches!(s.playing_ec(h), Some(1000 | 1040)));
        // Fortify: the warrior of the fixture has no 1500 clip, so it stands on idle (and the
        // report of the model says so). The machine reacts on its next tick.
        s.set_state(
            h,
            AnimState {
                action: 1500,
                ..AnimState::default()
            },
        );
        run(&mut s, 1.0, 1.5);
        assert_eq!(s.playing_ec(h), Some(1000));
        // A model that has the fortify clip changes to it, with a fade in between.
        let (mut m, mut pack) = fixtures::model("fort", 3);
        // ENTER_FORTIFY crossfades into 1500 over the graph's 0.15 s S-curve.
        for (i, (ec, dur)) in [(1500u32, 1.5f32), (1520, 2.0)].into_iter().enumerate() {
            pack.clips
                .push(fixtures::swing_clip("Fortify", dur, 3, 40.0));
            m.clips.push(asset_format::ClipRef {
                name: format!("Fortify{ec}"),
                ec,
                index: 6 + i as u16,
                events: vec![],
                legacy_blob: None,
            });
        }
        let machine = fixtures::omni_machine_v2();
        s.register_model(
            id("fort"),
            ModelAnim::new(&m, Some(pack), Some(&machine)).unwrap(),
        );
        let f = s.attach(&id("fort"), figure(9)).unwrap();
        run(&mut s, 1.0, 2.0);
        s.set_state(
            f,
            AnimState {
                action: 1500,
                ..AnimState::default()
            },
        );
        s.step(2.1);
        assert_eq!(s.playing_ec(f), Some(1500));
        let r = s.bone_ref(f);
        assert!(s.is_ready(f));
        assert!(r.blend > 0.0 && r.blend < 1.0, "mid-fade: {r:?}");
        // After the fade the reference is of the new clip alone.
        run(&mut s, 2.1, 3.0);
        let k = s
            .cache()
            .get(ClipKey { model: 1, clip: 6 })
            .unwrap()
            .clone();
        let r = s.bone_ref(f);
        assert!(
            k.range.contains(&r.base) && k.range.contains(&r.next),
            "{r:?}"
        );
        // Back to idle.
        s.set_state(f, AnimState::default());
        // ENTER_FORTIFY plays its clip out first (until 3.55 s), then the held pose is left.
        run(&mut s, 3.0, 4.0);
        assert!(matches!(s.playing_ec(f), Some(1000 | 1040 | 2000 | 2040)));
    }

    /// Packet F: the machines that sleep, and the clocked references the shader runs, give the
    /// very poses of the old walk (every machine every tick, every reference every frame): over a
    /// scripted run of figures with machines of both kinds, figures and landmarks without one,
    /// requests, cross-fades, releases, frames of 1/240 s to 0.6 s and a clock that goes back.
    #[test]
    fn sleeping_machines_and_the_gpu_clock_draw_the_old_poses() {
        let make = |sleep: bool, clock: bool| {
            let mut s = AnimServiceOf::new(MemoryBones::new(200_000));
            for (n, machine) in [
                ("v1", Some(fixtures::omni_machine())),
                ("v2", Some(fixtures::omni_machine_v2())),
                ("plain", None),
            ] {
                let (mut m, mut pack) = fixtures::model(n, 3);
                for (i, ec) in [1500u32, 1520, 1540, 1930].into_iter().enumerate() {
                    pack.clips
                        .push(fixtures::swing_clip("F", 0.4 + i as f32 * 0.35, 3, 30.0));
                    m.clips.push(asset_format::ClipRef {
                        name: format!("F{ec}"),
                        ec,
                        index: 6 + i as u16,
                        events: vec![],
                        legacy_blob: None,
                    });
                }
                let a = ModelAnim::new(&m, Some(pack), machine.as_ref()).unwrap();
                s.register_model(id(n), a);
            }
            s.set_fsm_sleep(sleep);
            // The block is taken on all three, so the clips have the same slots.
            assert!(s.set_gpu_clock(true));
            s.set_gpu_clock(clock);
            s
        };
        let mut old = make(false, false);
        let mut asleep = make(true, false);
        let mut clocked = make(true, true);
        let targets: Vec<(&str, AnimTarget)> = (0..60u32)
            .map(|i| {
                let n = ["v1", "v2", "plain"][(i % 3) as usize];
                let t = if i % 7 == 6 {
                    AnimTarget::Landmark { cell: i, slot: 0 }
                } else {
                    AnimTarget::Figure {
                        unit: (0, i as i32 / 4),
                        index: (i % 4) as u8,
                    }
                };
                (n, t)
            })
            .collect();
        let mut script = AnimRng::new(99);
        let mut live: Vec<Option<AnimHandle>> = vec![None; targets.len()];
        let mut t = 0.0f32;
        let mut checked = 0u32;
        for frame in 0..4000 {
            // Attach, release, ask, between the frames: the same on the three services.
            for (k, (n, target)) in targets.iter().enumerate() {
                let roll = script.next_u64() % 1000;
                match live[k] {
                    None if roll < 30 => {
                        let a = old.attach(&id(n), *target);
                        assert_eq!(a, asleep.attach(&id(n), *target));
                        assert_eq!(a, clocked.attach(&id(n), *target));
                        live[k] = a;
                    }
                    Some(h) if roll < 2 => {
                        old.release(h);
                        asleep.release(h);
                        clocked.release(h);
                        live[k] = None;
                    }
                    Some(h) if roll < 12 => {
                        let all = [1000, 1000, 1500, 1910, 1930];
                        let state = AnimState {
                            action: all[(script.next_u64() % all.len() as u64) as usize],
                            ..AnimState::default()
                        };
                        old.set_state(h, state);
                        asleep.set_state(h, state);
                        clocked.set_state(h, state);
                    }
                    _ => {}
                }
            }
            let dt = match script.next_u64() % 200 {
                0 => 0.6,
                1 if frame > 2000 && frame < 2010 => -0.8,
                r => 1.0 / 240.0 + (r % 40) as f32 * 0.004,
            };
            t = (t + dt).max(0.0);
            old.step(t);
            asleep.step(t);
            clocked.step(t);
            for h in live.iter().flatten() {
                let want = old.bone_ref(*h);
                assert_eq!(want, asleep.bone_ref(*h), "frame {frame} t {t}");
                assert_eq!(want, clocked.resolved_ref(*h, t), "frame {frame} t {t}");
                assert_eq!(old.playing_ec(*h), clocked.playing_ec(*h));
                assert_eq!(old.is_ready(*h), clocked.is_ready(*h));
                checked += 1;
            }
        }
        assert!(checked > 50_000, "{checked}");
        // The point of it: most ticks slept, most references not made again.
        assert!(asleep.stats().fsm_ticks <= old.stats().fsm_ticks);
        assert!(clocked.clock.as_ref().unwrap().headers() > 3);
        assert!(clocked.active.len() < live.iter().flatten().count());
    }

    /// AC-R4.2 (here): no matter what is asked, the clip is never one of attack, move, death,
    /// victory, capture or combat readiness.
    #[test]
    fn forbidden_actions_play_idle() {
        let (mut m, mut pack) = fixtures::model("w", 3);
        for ec in data::EC_NEVER_SAMPLES {
            let index = pack.clips.len() as u16;
            pack.clips.push(fixtures::swing_clip("x", 1.0, 3, 10.0));
            m.clips.push(asset_format::ClipRef {
                name: format!("c{ec}"),
                ec,
                index,
                events: vec![],
                legacy_blob: None,
            });
        }
        let a = ModelAnim::new(&m, Some(pack), None).unwrap();
        for ec in data::EC_NEVER_SAMPLES {
            assert!(data::EC_NEVER.contains(&ec) || data::is_combat_ready(ec));
            let c = clip_for(
                &a,
                &AnimState {
                    action: ec,
                    ..AnimState::default()
                },
            )
            .unwrap();
            assert_eq!(c.ec, 1000, "action {ec}");
        }
        assert!(action_is_standing(1000) && action_is_standing(1500));
        assert!(action_is_standing(1910) && action_is_standing(1960));
        assert!(
            !action_is_standing(1961) && !action_is_standing(1100) && !action_is_standing(1600)
        );
    }

    #[test]
    fn a_feature_plays_100_and_101_and_a_landmark_its_row() {
        let (mut m, mut pack) = fixtures::model("oasis", 2);
        m.clips.clear();
        pack.clips.truncate(3);
        let names = [("Idle", 100u32), ("Worked", 101), ("Build", 0)];
        for (i, (n, ec)) in names.into_iter().enumerate() {
            m.clips.push(asset_format::ClipRef {
                name: n.into(),
                ec,
                index: i as u16,
                events: vec![],
                legacy_blob: None,
            });
        }
        m.state_clips = vec![asset_format::StateClip {
            state: "UnderConstruction".into(),
            clip: asset_format::ClipKey::Name("Build".into()),
            repeat: 0,
            pause: 0.0,
        }];
        let a = ModelAnim::new(&m, Some(pack), None).unwrap();
        let ec = |st: AnimState| clip_for(&a, &st).unwrap().ec;
        assert_eq!(ec(AnimState::default()), 100);
        assert_eq!(
            ec(AnimState {
                worked: true,
                ..AnimState::default()
            }),
            101
        );
        assert_eq!(
            ec(AnimState {
                landmark: LandmarkState::UnderConstruction,
                ..AnimState::default()
            }),
            0,
            "the row names the clip `Build`, which has no code"
        );
    }

    #[test]
    fn the_bake_waits_for_the_budget_and_the_figure_waits_for_the_bake() {
        let mut s = service_with(&["a", "b", "c"], 3, false);
        s.cache_mut().set_budget(1);
        let hs: Vec<_> = ["a", "b", "c"]
            .iter()
            .enumerate()
            .map(|(i, n)| s.attach(&id(n), figure(i as u8)).unwrap())
            .collect();
        s.step(0.0);
        let ready: usize = hs.iter().filter(|&&h| s.is_ready(h)).count();
        assert_eq!(ready, 1, "one clip a frame");
        assert_eq!(s.stats().clips_pending, 2);
        assert_eq!(s.pending_work(), 2);
        s.step(0.02);
        s.step(0.04);
        assert!(hs.iter().all(|&h| s.is_ready(h)));
        assert_eq!(s.pending_work(), 0);
        assert_eq!(s.stats().baked_total, 3);
    }

    /// WA-D12: with the clock frozen the frame repeats byte for byte.
    #[test]
    fn a_frozen_clock_repeats_the_frame() {
        let run_once = || {
            let mut s = service_with(&["warrior"], 3, false);
            s.set_time_override(Some(5.0));
            let h = s.attach(&id("warrior"), figure(4)).unwrap();
            s.step(0.0);
            s.step(123.0);
            s.bone_ref(h)
        };
        assert_eq!(run_once(), run_once());
        let mut s = service_with(&["warrior"], 3, false);
        s.set_time_override(Some(5.0));
        let h = s.attach(&id("warrior"), figure(4)).unwrap();
        s.step(0.0);
        let a = s.bone_ref(h);
        s.step(9.0);
        assert_eq!(a, s.bone_ref(h), "frozen means frozen");
    }

    /// AC-R4.1 in the small: every model with an idle clip, attached and run for two seconds,
    /// has a reference that moved.
    #[test]
    fn every_model_with_an_idle_clip_moves() {
        let names = ["warrior", "archer", "settler", "worker"];
        let mut s = service_with(&names, 4, true);
        let hs: Vec<_> = names
            .iter()
            .enumerate()
            .map(|(i, n)| (n, s.attach(&id(n), figure(i as u8)).unwrap()))
            .collect();
        run(&mut s, 0.0, 2.0);
        for (n, h) in hs {
            assert!(s.has_moved(h), "{n} did not move");
        }
        let st = s.stats();
        assert_eq!((st.models, st.attached), (4, 4));
        assert!(st.used_slots > 0 && st.rows >= 1);
    }

    #[test]
    fn clear_forgets_everything() {
        let mut s = service_with(&["warrior"], 3, false);
        let h = s.attach(&id("warrior"), figure(0)).unwrap();
        s.step(0.0);
        s.clear();
        assert!(!s.is_live(h));
        assert!(!s.has_model(&id("warrior")));
        assert_eq!(
            s.stats().used_slots,
            clip_cache::IDENTITY_BONES,
            "only the identity run stays"
        );
    }

    /// A landmark or a figure whose clip finds no room (or has not been baked yet) is still given a
    /// reference to draw with: the identity run, never "skip it".
    #[test]
    fn a_landmark_without_a_baked_clip_still_has_a_bind_pose_to_draw() {
        // A store that holds the identity run and nothing more.
        let mut s = AnimServiceOf::new(MemoryBones::new(clip_cache::IDENTITY_BONES + 10));
        let (m, pack) = fixtures::model("fish", 3);
        s.register_model(id("fish"), ModelAnim::new(&m, Some(pack), None).unwrap());
        let h = s
            .attach(&id("fish"), AnimTarget::Landmark { cell: 5, slot: 0 })
            .unwrap();
        let before = s.bone_ref(h);
        assert_eq!(before, s.cache().identity_ref(), "before the first step");
        for k in 0..3 {
            s.step(k as f32 * 0.03);
            assert!(!s.is_ready(h), "no room for any clip");
            assert_eq!(s.bone_ref(h), s.cache().identity_ref());
        }
        assert!(s.stats().failed_total > 0);
        assert_eq!(s.cache().identity_ref().blend, 0.0);
        // The identity run is real bones: identity matrices where the reference points.
        let r = s.bone_ref(h);
        for k in 0..3 {
            assert_eq!(
                s.cache().store().bone(r.base + k),
                clip_cache::pack_bone(&glam::Mat4::IDENTITY)
            );
        }
    }

    #[test]
    fn the_generator_is_a_stream_in_the_unit_interval() {
        let mut r = AnimRng::new(1);
        let v: Vec<f32> = (0..1000).map(|_| r.unit()).collect();
        assert!(v.iter().all(|x| (0.0..1.0).contains(x)));
        let mean = v.iter().sum::<f32>() / 1000.0;
        assert!((mean - 0.5).abs() < 0.05, "{mean}");
        let mut a = AnimRng::new(7);
        let mut b = AnimRng::new(7);
        assert_eq!(a.next_u64(), b.next_u64());
        assert_ne!(figure(0).seed(), figure(1).seed());
    }
}
