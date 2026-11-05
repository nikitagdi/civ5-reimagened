//! The clock of the animation on the GPU (B3 packet F): an instance that plays one clip steadily
//! (baked, no cross-fade) does not get a new [`BoneRef`] every frame. It gets, once, a reference
//! that the vertex shader turns into the frames of the moment (`bone_pose` in
//! `shaders/common/skin.wgsl`):
//!
//! * `base`: the slot of the *header* of its play, a bone of the store whose twelve floats are
//!   `first slot, bones, frames, fps, duration, mode, repeat, pause` (the clip in the texture and
//!   how its time runs: once, a loop, or a landmark's cycle);
//! * `next`: [`CLOCK_FLAG`] and the slot of the *clock*, a bone whose first float is the time of the
//!   service, written every frame;
//! * `blend`: the time the play began (`Play::start_s`).
//!
//! The shader does what [`AnimServiceOf::step`](super::AnimServiceOf::step) does on the CPU for the
//! other instances: the time in the clip ([`clip_time`] or [`Cycle::time`]), then the two frames and
//! the blend ([`frame_ref_at`]); [`GpuClock::decode`] is the same on the CPU, for the tests. The
//! header and the clock live in a block of the store taken once, so nothing about the bake (slots,
//! tiers) moves: the block is left out of the fill the tiers are chosen by.
//!
//! The slots are below 2^20 (`skin::MAX_SLOTS`), so a slot is exact as an f32, and a plain
//! `BoneRef` never has the top bit of `next` set.

use std::ops::Range;

use super::bone_ref::{BoneRef, clip_time, frame_ref_at};
use super::clip_cache::{BakedClip, Bone, ClipKey};
use super::fx::FxHashMap;
use super::landmark_tree::Cycle;

/// The top bit of `BoneRef::next`: the reference is a header and a clock, not two frames.
pub const CLOCK_FLAG: u32 = 1 << 31;

/// Bones of the block: the clock and the headers.
pub const CLOCK_BLOCK: u32 = 4096;

/// How the time of a play runs (the `mode` float of a header).
const MODE_ONCE: f32 = 0.0;
const MODE_LOOP: f32 = 1.0;
const MODE_CYCLE: f32 = 2.0;

/// What a header says, and so what two plays must share to share one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlayKind {
    pub clip: ClipKey,
    pub looping: bool,
    /// `f32::to_bits` of the duration the play runs its time by.
    pub duration: u32,
    /// The repeat and `f32::to_bits` of the pause of a cycle; `(0, 0)` for none.
    pub cycle: (u16, u32),
}

impl PlayKind {
    pub fn new(clip: ClipKey, looping: bool, duration: f32, cycle: Option<Cycle>) -> Self {
        Self {
            clip,
            looping,
            duration: duration.to_bits(),
            cycle: cycle.map_or((0, 0), |c| (c.repeat, c.pause.to_bits())),
        }
    }

    fn cycle(&self) -> Option<Cycle> {
        (self.cycle != (0, 0)).then_some(Cycle {
            repeat: self.cycle.0,
            pause: f32::from_bits(self.cycle.1),
        })
    }
}

/// A header as written, and the geometry of the clip it was written for.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Header {
    kind: PlayKind,
    first: u32,
    bones: u32,
    frames: u32,
    fps: f32,
}

impl Header {
    fn of(kind: PlayKind, clip: &BakedClip) -> Self {
        Self {
            kind,
            first: clip.range.start,
            bones: clip.bones,
            frames: clip.frames,
            fps: clip.fps,
        }
    }

    fn bone(&self) -> Bone {
        let mode = match (self.kind.cycle(), self.kind.looping) {
            (Some(_), _) => MODE_CYCLE,
            (None, true) => MODE_LOOP,
            (None, false) => MODE_ONCE,
        };
        let (repeat, pause) = self
            .kind
            .cycle()
            .map_or((0.0, 0.0), |c| (f32::from(c.repeat), c.pause));
        [
            self.first as f32,
            self.bones as f32,
            self.frames as f32,
            self.fps,
            f32::from_bits(self.kind.duration),
            mode,
            repeat,
            pause,
            0.0,
            0.0,
            0.0,
            0.0,
        ]
    }
}

/// The block of the clock and its headers.
pub struct GpuClock {
    block: Range<u32>,
    /// Header by slot offset in the block (0 is the clock).
    slots: Vec<Option<Header>>,
    free: Vec<u32>,
    by_kind: FxHashMap<PlayKind, u32>,
}

impl GpuClock {
    /// Over a block of [`CLOCK_BLOCK`] slots of the store.
    pub fn new(block: Range<u32>) -> Self {
        let n = block.len() as u32;
        Self {
            block,
            slots: vec![None; n as usize],
            free: (1..n).rev().collect(),
            by_kind: FxHashMap::default(),
        }
    }

    /// The slots of the block (left out of the fill of the store).
    pub fn block(&self) -> Range<u32> {
        self.block.clone()
    }

    /// The slot of the clock.
    pub fn clock_slot(&self) -> u32 {
        self.block.start
    }

    /// The bone of the clock at `now`.
    pub fn clock_bone(now: f32) -> Bone {
        let mut b = [0.0; 12];
        b[0] = now;
        b
    }

    /// Headers in use.
    pub fn headers(&self) -> usize {
        self.by_kind.len()
    }

    /// The slot of the header of `kind` on the baked `clip`, written through `write` when it is new
    /// or the clip moved. `None` when the block is full.
    pub fn header_for(
        &mut self,
        kind: PlayKind,
        clip: &BakedClip,
        mut write: impl FnMut(Range<u32>, &[Bone]),
    ) -> Option<u32> {
        let want = Header::of(kind, clip);
        let off = match self.by_kind.get(&kind) {
            Some(&off) if self.slots[off as usize] == Some(want) => {
                return Some(self.block.start + off);
            }
            Some(&off) => off,
            None => {
                let off = self.free.pop()?;
                self.by_kind.insert(kind, off);
                off
            }
        };
        self.slots[off as usize] = Some(want);
        let slot = self.block.start + off;
        write(slot..slot + 1, &[want.bone()]);
        Some(slot)
    }

    /// Drops the headers whose clip is gone from the cache or was baked again elsewhere.
    pub fn prune(&mut self, baked: impl Fn(ClipKey) -> Option<(u32, u32, u32, u32)>) {
        let slots = &mut self.slots;
        let free = &mut self.free;
        self.by_kind.retain(|kind, off| {
            let h = slots[*off as usize].expect("a header in use has its slot");
            let keep = baked(kind.clip) == Some((h.first, h.bones, h.frames, h.fps.to_bits()));
            if !keep {
                slots[*off as usize] = None;
                free.push(*off);
            }
            keep
        });
    }

    /// The reference an instance of the header in `header_slot`, begun at `start_s`, is drawn with.
    pub fn encode(&self, header_slot: u32, start_s: f32) -> BoneRef {
        BoneRef {
            base: header_slot,
            next: CLOCK_FLAG | self.clock_slot(),
            blend: start_s,
        }
    }

    /// What the shader makes of `r` at the time `now`: `r` itself unless it is a clocked one. The
    /// CPU's own functions, so a clocked reference decodes to the very reference the CPU would
    /// have given.
    pub fn decode(&self, r: BoneRef, now: f32) -> BoneRef {
        if r.next & CLOCK_FLAG == 0 {
            return r;
        }
        let Some(h) = r
            .base
            .checked_sub(self.block.start)
            .and_then(|off| self.slots.get(off as usize).copied().flatten())
        else {
            return BoneRef::default();
        };
        let duration = f32::from_bits(h.kind.duration);
        let elapsed = now - r.blend;
        let t = match h.kind.cycle() {
            Some(c) => c.time(elapsed, duration),
            None => clip_time(elapsed, duration, h.kind.looping),
        };
        frame_ref_at(h.first, h.bones, h.frames, t, h.fps)
    }
}
