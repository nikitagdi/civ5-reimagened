//! The cache of clips in the bone texture.
//!
//! A clip of `B` bones and `F` frames takes one run of `B x F` slots of the bone texture; frame `k`
//! starts at `base + k * B`. The poses are baked once, on the 30 Hz grid (frame `k` is
//! `Clip::sample(k / 30)` of the `C5A2` pack, then the skeleton's world and skin matrices), and an
//! instance picks frames by number (`BoneRef`). Slots come from a [`BoneSlots`] (the bone texture of
//! `render-core`, `skin::BoneStore`/`BoneTexture`, or [`MemoryBones`] in tests and until the world
//! has a texture of its own). When the texture is full the clip that nobody has asked for the
//! longest is evicted; a clip that an instance asks for this frame is never evicted.
//!
//! SIMPLIFIED(SIMP-050): the poses are on the 30 Hz grid and are blended between two frames, as
//! matrices; during the cross-fade of two clips the nearest frame of each is taken. Granny samples
//! the curves at any time. Goes if the eye sees it: four references in the instance.

use std::ops::Range;
use std::sync::Arc;

use glam::Mat4;

use super::bone_ref::{BoneRef, frame_ref_at};
use super::data::{LAST_TIER, ModelAnim, bake_fps_tier, frames_tier};
use super::fx::FxHashMap;

/// One bone as the texture holds it: the three rows of the 3x4 matrix, `skin.wgsl` order.
pub type Bone = [f32; 12];

/// Bytes of a bone in the texture.
pub const BYTES_PER_BONE: usize = 48;
/// How many bytes of baked clips go to the texture in a frame (render-core 4.12: 512 KiB).
pub const UPLOAD_BUDGET_BYTES: usize = 512 * 1024;
/// Slots of the biggest bone texture: 512 bones a row, 2048 rows.
pub const MAX_SLOTS: u32 = 512 * 2048;

/// The three rows of the upper 3x4 of `m`: the layout of `skin::pack_bone`.
pub fn pack_bone(m: &Mat4) -> Bone {
    let c = m.to_cols_array_2d();
    std::array::from_fn(|k| c[k % 4][k / 4])
}

/// What the cache needs of the bone texture: runs of slots, and writing bones into them.
pub trait BoneSlots {
    /// A run of `slots` free slots, or `None` if there is none.
    fn alloc(&mut self, slots: u32) -> Option<Range<u32>>;
    /// Gives the run back. False if it was not an allocated run.
    fn free(&mut self, range: Range<u32>) -> bool;
    /// Writes `bones` from the start of `range`.
    fn write(&mut self, range: Range<u32>, bones: &[Bone]);
    /// Slots taken now.
    fn used_slots(&self) -> u32;
    /// Slots the store has in all.
    fn capacity(&self) -> u32;
}

/// A bone store in memory, first fit, with the runs joined when they are given back. It is the
/// store of the tests and the stand-in until the world's own bone texture is wired.
#[derive(Debug, Clone)]
pub struct MemoryBones {
    free: Vec<Range<u32>>,
    data: Vec<Bone>,
    used: u32,
    capacity: u32,
}

impl MemoryBones {
    pub fn new(capacity: u32) -> Self {
        Self {
            free: std::iter::once(0..capacity).collect(),
            data: Vec::new(),
            used: 0,
            capacity,
        }
    }

    /// The bone in `slot` (the identity-less zero matrix if it was never written).
    pub fn bone(&self, slot: u32) -> Bone {
        self.data.get(slot as usize).copied().unwrap_or([0.0; 12])
    }

    /// Free runs, for the tests.
    pub fn free_runs(&self) -> &[Range<u32>] {
        &self.free
    }
}

impl Default for MemoryBones {
    fn default() -> Self {
        Self::new(MAX_SLOTS)
    }
}

impl BoneSlots for MemoryBones {
    fn alloc(&mut self, slots: u32) -> Option<Range<u32>> {
        if slots == 0 {
            return None;
        }
        let i = self.free.iter().position(|r| r.len() as u32 >= slots)?;
        let start = self.free[i].start;
        if self.free[i].len() as u32 == slots {
            self.free.remove(i);
        } else {
            self.free[i].start += slots;
        }
        self.used += slots;
        Some(start..start + slots)
    }

    fn free(&mut self, range: Range<u32>) -> bool {
        if range.is_empty() {
            return false;
        }
        // Must not overlap a free run.
        if self
            .free
            .iter()
            .any(|f| f.start < range.end && range.start < f.end)
        {
            return false;
        }
        let at = self.free.partition_point(|f| f.start < range.start);
        self.free.insert(at, range.clone());
        self.used -= range.len() as u32;
        // Join the neighbours.
        if at + 1 < self.free.len() && self.free[at].end == self.free[at + 1].start {
            self.free[at].end = self.free[at + 1].end;
            self.free.remove(at + 1);
        }
        if at > 0 && self.free[at - 1].end == self.free[at].start {
            self.free[at - 1].end = self.free[at].end;
            self.free.remove(at);
        }
        true
    }

    fn write(&mut self, range: Range<u32>, bones: &[Bone]) {
        let n = bones.len().min(range.len());
        let start = range.start as usize;
        if self.data.len() < start + n {
            self.data.resize(start + n, [0.0; 12]);
        }
        self.data[start..start + n].copy_from_slice(&bones[..n]);
    }

    fn used_slots(&self) -> u32 {
        self.used
    }

    fn capacity(&self) -> u32 {
        self.capacity
    }
}

/// A clip of a model: the model's number in the service and the index in its pack.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClipKey {
    pub model: u32,
    pub clip: u16,
}

/// A baked clip.
#[derive(Debug, Clone, PartialEq)]
pub struct BakedClip {
    pub range: Range<u32>,
    /// Slots a frame takes: the bones of the skeleton.
    pub bones: u32,
    pub frames: u32,
    pub duration: f32,
    /// Frames a second it was baked at (`data::bake_fps_tier`).
    pub fps: f32,
    /// The bake tier (0 is the finest).
    pub tier: u8,
    /// The frame of the cache that last asked for the clip.
    last_used: u64,
}

impl BakedClip {
    /// The reference for the time `t` in the clip.
    pub fn bone_ref(&self, t: f32) -> BoneRef {
        frame_ref_at(self.range.start, self.bones, self.frames, t, self.fps)
    }

    /// What `bone_ref` needs, by value: the step of the service keeps it for the next instance
    /// that plays the same clip instead of looking the clip up again.
    pub fn view(&self) -> ClipView {
        ClipView {
            start: self.range.start,
            bones: self.bones,
            frames: self.frames,
            fps: self.fps,
        }
    }
}

/// The part of a [`BakedClip`] that turns a time into a reference.
#[derive(Debug, Clone, Copy)]
pub struct ClipView {
    start: u32,
    bones: u32,
    frames: u32,
    fps: f32,
}

impl ClipView {
    pub fn bone_ref(&self, t: f32) -> BoneRef {
        frame_ref_at(self.start, self.bones, self.frames, t, self.fps)
    }
}

/// The matrices of every frame of a clip, in the order of the slots: `frames` runs of `bones`
/// bones. Frame `k` is the pose at `k / 30` seconds.
pub fn bake_clip(model: &ModelAnim, clip: u16) -> Option<(Vec<Bone>, u32)> {
    bake_clip_tier(model, clip, 0)
}

/// [`bake_clip`] at the rate of `tier` (`data::BAKE_TIERS`).
pub fn bake_clip_tier(model: &ModelAnim, clip: u16, tier: u8) -> Option<(Vec<Bone>, u32)> {
    let c = model.pack.clips.get(usize::from(clip))?;
    let frames = frames_tier(c.duration, tier);
    let fps = bake_fps_tier(c.duration, tier);
    let skeleton = &model.skeleton;
    let rest = skeleton.rest_pose();
    let n = rest.len();
    let mut local = rest.clone();
    let mut world = vec![Mat4::IDENTITY; n];
    let mut skin = vec![Mat4::IDENTITY; n];
    let mut out = Vec::with_capacity(n * frames as usize);
    for k in 0..frames {
        c.sample(k as f32 / fps, &rest, &mut local);
        skeleton.world_matrices(&local, &mut world);
        skeleton.skin_matrices(&world, &mut skin);
        out.extend(skin.iter().map(pack_bone));
    }
    Some((out, frames))
}

/// What one `bake_pending` did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BakeReport {
    pub baked: u32,
    pub evicted: u32,
    pub bytes: u64,
    pub failed: u32,
}

struct Pending {
    key: ClipKey,
    model: Arc<ModelAnim>,
}

/// The cache. One per service.
pub struct ClipCache<S: BoneSlots> {
    store: S,
    clips: FxHashMap<ClipKey, BakedClip>,
    pending: Vec<Pending>,
    frame: u64,
    budget: usize,
    last: BakeReport,
    baked_total: u64,
    evicted_total: u64,
    failed_total: u64,
    /// The run of identity bones every instance can show before its clip is baked.
    identity: Option<Range<u32>>,
    /// False: always the finest tier (the tests of the allocator).
    tiering: bool,
    /// Slots taken for something else than clips (the block of the GPU clock): not in the fill.
    reserved: u32,
    /// Bumped when a clip is baked, evicted or dropped: the references of the instances may move.
    revision: u64,
}

/// Bones of the identity run: `MAX_BONES` and one more (`model.rs` keeps the same).
pub const IDENTITY_BONES: u32 = 256;

impl<S: BoneSlots + Default> Default for ClipCache<S> {
    fn default() -> Self {
        Self::new(S::default())
    }
}

impl<S: BoneSlots> ClipCache<S> {
    pub fn new(store: S) -> Self {
        Self {
            store,
            clips: FxHashMap::default(),
            pending: Vec::new(),
            frame: 1,
            budget: UPLOAD_BUDGET_BYTES,
            last: BakeReport::default(),
            baked_total: 0,
            evicted_total: 0,
            failed_total: 0,
            identity: None,
            tiering: true,
            reserved: 0,
            revision: 0,
        }
    }

    /// Off: the cache bakes at the finest tier only, whatever the fill (the tests of the sizes).
    pub fn set_tiering(&mut self, on: bool) {
        self.tiering = on;
    }

    /// Takes a run of identity bones from the store, once: a thing whose clip is not baked yet
    /// shows its model in the pose of the bind (the skin matrices of the rest pose are identity)
    /// instead of not being drawn. On a fresh store the run starts at slot 0, which is the
    /// reference of a zeroed `BoneRef`.
    pub fn reserve_identity(&mut self) {
        if self.identity.is_some() {
            return;
        }
        if let Some(r) = self.store.alloc(IDENTITY_BONES) {
            self.store.write(
                r.clone(),
                &vec![pack_bone(&Mat4::IDENTITY); IDENTITY_BONES as usize],
            );
            self.identity = Some(r);
        }
    }

    /// Takes slots from the store for something that is not a clip (the block of the GPU clock),
    /// left out of the fill the tiers are chosen by.
    pub fn reserve_block(&mut self, slots: u32) -> Option<Range<u32>> {
        let r = self.store.alloc(slots)?;
        self.reserved += slots;
        Some(r)
    }

    /// Changes when a clip is baked, evicted or dropped.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The reference of the bind pose: every bone the identity. Zeros if nothing was reserved.
    pub fn identity_ref(&self) -> BoneRef {
        let base = self.identity.as_ref().map_or(0, |r| r.start);
        BoneRef {
            base,
            next: base,
            blend: 0.0,
        }
    }

    /// The finest tier to bake at for the fill of the store now: the first 40 percent of the
    /// texture at full rate, up to 60 percent the half-rate, then the coarsest.
    fn tier_for_fill(&self) -> u8 {
        let cap = u64::from(self.store.capacity().saturating_sub(self.reserved).max(1));
        let used = u64::from(self.store.used_slots().saturating_sub(self.reserved));
        if !self.tiering || used * 5 < cap * 2 {
            0
        } else if used * 5 < cap * 3 {
            1
        } else {
            LAST_TIER
        }
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut S {
        &mut self.store
    }

    /// Bytes of baked clips written in one `bake_pending` (at least one clip is always baked).
    pub fn set_budget(&mut self, bytes: usize) {
        self.budget = bytes;
    }

    /// A new frame: what asks for clips from here on counts as using them.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        self.last = BakeReport::default();
    }

    /// A baked clip, if it is in the cache.
    pub fn get(&self, key: ClipKey) -> Option<&BakedClip> {
        self.clips.get(&key)
    }

    /// Says that this frame needs the clip. True if it is baked and can be shown; otherwise it is
    /// queued for `bake_pending`.
    pub fn request(&mut self, key: ClipKey, model: &Arc<ModelAnim>) -> bool {
        if let Some(c) = self.clips.get_mut(&key) {
            c.last_used = self.frame;
            return true;
        }
        if !self.pending.iter().any(|p| p.key == key) {
            self.pending.push(Pending {
                key,
                model: Arc::clone(model),
            });
        }
        false
    }

    /// Marks a baked clip as used this frame, without queueing anything.
    pub fn touch(&mut self, key: ClipKey) -> bool {
        match self.clips.get_mut(&key) {
            Some(c) => {
                c.last_used = self.frame;
                true
            }
            None => false,
        }
    }

    pub fn clips_cached(&self) -> usize {
        self.clips.len()
    }

    pub fn clips_pending(&self) -> usize {
        self.pending.len()
    }

    pub fn last_report(&self) -> BakeReport {
        self.last
    }

    pub fn totals(&self) -> (u64, u64, u64) {
        (self.baked_total, self.evicted_total, self.failed_total)
    }

    /// Bakes the queued clips in the order they were asked for, until the budget of the frame is
    /// spent (the first clip is always baked, so that a big clip is not starved).
    pub fn bake_pending(&mut self) -> BakeReport {
        let mut report = BakeReport::default();
        let mut i = 0;
        while i < self.pending.len() {
            if report.bytes as usize >= self.budget && report.baked > 0 {
                break;
            }
            let p = &self.pending[i];
            let (key, model) = (p.key, Arc::clone(&p.model));
            // The slots the clip takes, known before the (costly) bake: a clip that finds no room
            // is not baked, or it would be baked again every frame to be thrown away.
            let Some(c) = model.pack.clips.get(usize::from(key.clip)) else {
                // The pack has no such clip: nothing to wait for.
                self.pending.remove(i);
                continue;
            };
            let duration = c.duration;
            if model.bones() == 0 {
                self.pending.remove(i);
                continue;
            }
            // The finest tier the fill of the texture allows; if there is no room even after the
            // unused clips are evicted, a coarser one before giving up.
            let start = self.tier_for_fill();
            let found = loop {
                let last = if self.tiering { LAST_TIER } else { start };
                let hit = (start..=last).find_map(|tier| {
                    let n = model.bones() * frames_tier(duration, tier);
                    self.store.alloc(n).map(|r| (r, n, tier))
                });
                if hit.is_some() {
                    break hit;
                }
                // Room is made by the clip that nobody has asked for the longest.
                if !self.evict_one(&mut report) {
                    break None;
                }
            };
            let Some((range, n, tier)) = found else {
                // No room even with everything unused gone, at the coarsest rate: try again when
                // something is freed. The thing shows its bind pose meanwhile.
                report.failed += 1;
                self.failed_total += 1;
                i += 1;
                continue;
            };
            let Some((bones, frames)) = bake_clip_tier(&model, key.clip, tier) else {
                self.store.free(range);
                self.pending.remove(i);
                continue;
            };
            debug_assert_eq!(bones.len() as u32, n);
            self.store.write(range.clone(), &bones);
            self.clips.insert(
                key,
                BakedClip {
                    range,
                    bones: model.bones(),
                    frames,
                    duration,
                    fps: bake_fps_tier(duration, tier),
                    tier,
                    last_used: self.frame,
                },
            );
            self.pending.remove(i);
            self.revision += 1;
            report.baked += 1;
            report.bytes += u64::from(n) * BYTES_PER_BONE as u64;
        }
        if (report.baked > 0 || report.failed > 0 || report.evicted > 0)
            && self.frame.is_multiple_of(120)
        {
            tracing::info!(
                "clip cache: frame {} baked {} evicted {} failed {} (totals {} {} {}), {} cached, {} pending, {} slots used",
                self.frame,
                report.baked,
                report.evicted,
                report.failed,
                self.baked_total,
                self.evicted_total,
                self.failed_total,
                self.clips.len(),
                self.pending.len(),
                self.store.used_slots()
            );
        }
        self.baked_total += u64::from(report.baked);
        self.evicted_total += u64::from(report.evicted);
        self.last = BakeReport {
            baked: self.last.baked + report.baked,
            evicted: self.last.evicted + report.evicted,
            bytes: self.last.bytes + report.bytes,
            failed: self.last.failed + report.failed,
        };
        report
    }

    /// Takes out the clip that has been unused the longest and was not asked for this frame.
    fn evict_one(&mut self, report: &mut BakeReport) -> bool {
        let frame = self.frame;
        let victim = self
            .clips
            .iter()
            .filter(|(_, c)| c.last_used < frame)
            .min_by_key(|(k, c)| (c.last_used, **k))
            .map(|(k, _)| *k);
        let Some(key) = victim else {
            return false;
        };
        if let Some(c) = self.clips.remove(&key) {
            self.store.free(c.range);
            self.revision += 1;
            report.evicted += 1;
        }
        true
    }

    /// Drops every clip of a model (it was unloaded).
    pub fn drop_model(&mut self, model: u32) {
        let keys: Vec<ClipKey> = self
            .clips
            .keys()
            .filter(|k| k.model == model)
            .copied()
            .collect();
        for k in keys {
            if let Some(c) = self.clips.remove(&k) {
                self.store.free(c.range);
                self.revision += 1;
            }
        }
        self.pending.retain(|p| p.key.model != model);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::data::{fixtures, frames_of};
    use asset_format::Transform;

    fn model(name: &str, bones: usize) -> Arc<ModelAnim> {
        let (m, pack) = fixtures::model(name, bones);
        Arc::new(ModelAnim::new(&m, Some(pack), None).unwrap())
    }

    /// A cache with the bake tier fixed at the finest, so the sizes of the tests are the clips'.
    fn cache(capacity: u32) -> ClipCache<MemoryBones> {
        let mut c = ClipCache::new(MemoryBones::new(capacity));
        c.set_tiering(false);
        c
    }

    #[test]
    fn the_store_gives_runs_and_joins_them_back() {
        let mut s = MemoryBones::new(100);
        let a = s.alloc(30).unwrap();
        let b = s.alloc(30).unwrap();
        let c = s.alloc(30).unwrap();
        assert_eq!((a.start, b.start, c.start), (0, 30, 60));
        assert!(s.alloc(11).is_none());
        assert_eq!(s.used_slots(), 90);
        assert!(s.free(b.clone()));
        assert!(!s.free(b.clone()), "twice");
        assert!(s.free(a));
        assert_eq!(s.free_runs(), [0..60, 90..100]);
        assert!(s.free(c));
        assert_eq!(s.free_runs().len(), 1);
        assert_eq!(s.free_runs()[0], 0..100);
        assert_eq!(s.used_slots(), 0);
        assert!(s.alloc(0).is_none());
    }

    /// WAN2 test of the spec: frame `k` of a baked clip is `Clip::sample` at `k / 30` through the
    /// skeleton's matrices, to 1e-6.
    #[test]
    fn a_baked_frame_is_the_sampled_pose() {
        let m = model("warrior", 3);
        let mut c = cache(10_000);
        let key = ClipKey { model: 0, clip: 0 };
        assert!(!c.request(key, &m));
        c.begin_frame();
        let r = c.bake_pending();
        assert_eq!(r.baked, 1);
        let baked = c.get(key).unwrap().clone();
        assert_eq!(baked.bones, 3);
        assert_eq!(baked.frames, frames_of(2.0));
        assert_eq!(baked.range.len() as u32, 3 * baked.frames);

        let clip = &m.pack.clips[0];
        let rest = m.skeleton.rest_pose();
        for k in [0u32, 1, 15, 30, 59, baked.frames - 1] {
            let mut local = vec![Transform::IDENTITY; 3];
            clip.sample(k as f32 / 30.0, &rest, &mut local);
            let mut world = vec![Mat4::IDENTITY; 3];
            let mut skin = vec![Mat4::IDENTITY; 3];
            m.skeleton.world_matrices(&local, &mut world);
            m.skeleton.skin_matrices(&world, &mut skin);
            for (b, matrix) in skin.iter().enumerate() {
                let want = pack_bone(matrix);
                let got = c.store().bone(baked.range.start + k * 3 + b as u32);
                for (w, g) in want.iter().zip(got) {
                    assert!((w - g).abs() < 1e-6, "frame {k} bone {b}: {want:?} {got:?}");
                }
            }
        }
        // A pose that moves: bone 1 is not at rest at 1 s.
        let at = |k: u32| c.store().bone(baked.range.start + k * 3 + 1);
        assert_ne!(at(0), at(30));
    }

    #[test]
    fn pack_bone_is_the_rows_of_the_matrix() {
        let m = Mat4::from_translation(glam::Vec3::new(7.0, 8.0, 9.0));
        let b = pack_bone(&m);
        // Row 0: (1, 0, 0, 7), row 1: (0, 1, 0, 8), row 2: (0, 0, 1, 9).
        assert_eq!(
            b,
            [1.0, 0.0, 0.0, 7.0, 0.0, 1.0, 0.0, 8.0, 0.0, 0.0, 1.0, 9.0]
        );
    }

    #[test]
    fn addresses_cross_the_row_of_the_texture() {
        // A clip that starts 5 slots before the end of a row: frame 0 starts there, frame 1 in the
        // same row, frame 2 in the next.
        let mut c = cache(10_000);
        let m = model("w", 4);
        c.store_mut().alloc(507).unwrap();
        let key = ClipKey { model: 0, clip: 1 };
        c.request(key, &m);
        c.bake_pending();
        let b = c.get(key).unwrap();
        assert_eq!(b.range.start, 507);
        let r = b.bone_ref(1.0 / 30.0);
        assert_eq!((r.base, r.next), (507 + 4, 507 + 8));
        assert_eq!(
            (r.base / 512, r.next / 512),
            (0, 1),
            "frame 2 starts in row 1"
        );
    }

    #[test]
    fn a_clip_is_baked_once_and_asked_for_many_times() {
        let m = model("w", 3);
        let mut c = cache(10_000);
        let key = ClipKey { model: 0, clip: 0 };
        for _ in 0..5 {
            c.request(key, &m);
        }
        assert_eq!(c.clips_pending(), 1);
        c.bake_pending();
        assert_eq!(c.clips_pending(), 0);
        assert!(c.request(key, &m));
        assert_eq!(c.totals().0, 1);
    }

    #[test]
    fn eviction_takes_the_unused_and_spares_the_needed() {
        let m = model("w", 3);
        // Clip 0: 61 frames x 3 = 183 slots; clip 2: 31 x 3 = 93. The store holds 183 + 93.
        let mut c = cache(183 + 93);
        let (k0, k2, k1) = (
            ClipKey { model: 0, clip: 0 },
            ClipKey { model: 0, clip: 2 },
            ClipKey { model: 0, clip: 1 },
        );
        c.request(k0, &m);
        c.request(k2, &m);
        c.bake_pending();
        assert_eq!((c.clips_cached(), c.store().used_slots()), (2, 276));

        // A new frame asks for k2 again and for k1 (16 frames x 3 = 48 slots): k0 goes.
        c.begin_frame();
        c.request(k2, &m);
        c.request(k1, &m);
        let r = c.bake_pending();
        assert_eq!((r.baked, r.evicted), (1, 1));
        assert!(c.get(k0).is_none());
        assert!(c.get(k2).is_some() && c.get(k1).is_some());

        // Everything is needed this frame and there is no room: nothing is evicted, the clip waits.
        c.begin_frame();
        c.touch(k2);
        c.touch(k1);
        let k5 = ClipKey { model: 0, clip: 3 };
        c.request(k5, &m);
        let r = c.bake_pending();
        assert_eq!((r.baked, r.evicted, r.failed), (0, 0, 1));
        assert!(c.get(k2).is_some() && c.get(k1).is_some());
        assert_eq!(c.clips_pending(), 1);
        assert_eq!(c.totals().2, 1);

        // Next frame k2 is not asked for: it goes, and k5 (76 x 3 = 228 slots) fits beside k1
        // (48) in the 276, in the run the two evictions joined.
        c.begin_frame();
        c.touch(k1);
        let r = c.bake_pending();
        assert_eq!((r.baked, r.evicted), (1, 1));
        assert!(c.get(k2).is_none() && c.get(k5).is_some() && c.get(k1).is_some());
        assert_eq!(c.store().used_slots(), 276);
        assert_eq!(c.clips_pending(), 0);
    }

    #[test]
    fn the_budget_limits_a_frame_and_the_rest_waits() {
        let m = model("w", 3);
        let mut c = cache(10_000);
        // Clip 0 is 183 bones = 8784 bytes. A budget of one byte bakes one clip a frame.
        c.set_budget(1);
        let keys: Vec<ClipKey> = (0..4).map(|i| ClipKey { model: 0, clip: i }).collect();
        for k in &keys {
            c.request(*k, &m);
        }
        let mut frames = 0;
        while c.clips_pending() > 0 {
            c.begin_frame();
            let r = c.bake_pending();
            assert_eq!(r.baked, 1, "one a frame");
            frames += 1;
        }
        assert_eq!(frames, 4);
        // With the default budget (512 KiB) they all go in one.
        let mut c = cache(10_000);
        for k in &keys {
            c.request(*k, &m);
        }
        assert_eq!(c.bake_pending().baked, 4);
    }

    #[test]
    fn a_model_dropped_gives_its_slots_back() {
        let m = model("w", 3);
        let mut c = cache(10_000);
        c.request(ClipKey { model: 7, clip: 0 }, &m);
        c.request(ClipKey { model: 7, clip: 1 }, &m);
        c.bake_pending();
        c.request(ClipKey { model: 7, clip: 2 }, &m);
        assert!(c.store().used_slots() > 0);
        c.drop_model(7);
        assert_eq!(c.store().used_slots(), 0);
        assert_eq!((c.clips_cached(), c.clips_pending()), (0, 0));
    }

    /// Many clips in use at once all fit: the cache goes down a bake tier as the texture fills
    /// instead of leaving the last clips without room. 40 clips of 6 s on 64 bones are 40 x 181 x 64
    /// = 463k slots at 30 fps; in a store of 230k slots they fit only at the coarser tiers.
    #[test]
    fn many_clips_in_use_fit_by_baking_coarser_as_the_store_fills() {
        let (mut m, mut pack) = fixtures::model("long", 64);
        pack.clips.clear();
        m.clips.clear();
        for i in 0..40u16 {
            let mut c = fixtures::swing_clip(&format!("c{i}"), 6.0, 64, 10.0);
            c.duration = 6.0;
            pack.clips.push(c);
            m.clips.push(asset_format::ClipRef {
                name: format!("c{i}"),
                ec: 1000 + u32::from(i),
                index: i,
                events: vec![],
                legacy_blob: None,
            });
        }
        let model = Arc::new(ModelAnim::new(&m, Some(pack), None).unwrap());
        let mut c = ClipCache::new(MemoryBones::new(230_000));
        for i in 0..40u16 {
            c.request(ClipKey { model: 0, clip: i }, &model);
        }
        c.set_budget(usize::MAX);
        c.begin_frame();
        let r = c.bake_pending();
        assert_eq!((r.baked, r.failed, r.evicted), (40, 0, 0), "{r:?}");
        assert_eq!(c.clips_pending(), 0);
        assert!(c.store().used_slots() <= 230_000);
        let tiers: Vec<u8> = (0..40u16)
            .map(|i| c.get(ClipKey { model: 0, clip: i }).unwrap().tier)
            .collect();
        assert_eq!(tiers[0], 0, "the first go at the full rate");
        assert!(
            *tiers.last().unwrap() > 0,
            "the last at a coarser one: {tiers:?}"
        );
        // A coarser clip still has frames and a reference inside its run.
        let b = c.get(ClipKey { model: 0, clip: 39 }).unwrap();
        let r = b.bone_ref(3.3);
        assert!(r.base >= b.range.start && r.next < b.range.end);
        assert!(b.fps < 30.0);
    }

    #[test]
    fn an_identity_run_is_reserved_first_and_is_the_default_reference() {
        let mut c = cache(10_000);
        c.reserve_identity();
        c.reserve_identity();
        assert_eq!(c.store().used_slots(), IDENTITY_BONES);
        assert_eq!(c.identity_ref(), BoneRef::default());
        assert_eq!(c.store().bone(5), pack_bone(&Mat4::IDENTITY));
        assert_eq!(
            c.store().bone(IDENTITY_BONES - 1),
            pack_bone(&Mat4::IDENTITY)
        );
    }

    #[test]
    fn a_clip_the_pack_lacks_is_dropped_from_the_queue() {
        let m = model("w", 3);
        let mut c = cache(10_000);
        c.request(ClipKey { model: 0, clip: 99 }, &m);
        let r = c.bake_pending();
        assert_eq!((r.baked, r.failed), (0, 0));
        assert_eq!(c.clips_pending(), 0);
    }
}
