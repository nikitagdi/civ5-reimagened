//! Bodies and hits (`world.bodies`, API 1.1): a server half publishes the hit boxes of the things it owns (players,
//! creatures), reads the bodies of the other mods of the game, and asks the host to deliver a hit to the owner of one.
//!
//! Coordinates are world units, the same frame the client camera of a mod uses. The host fills [`Body::owner`], keeps
//! at most [`MAX_BODIES`] bodies of [`MAX_BOXES_PER_BODY`] boxes per mod (the rest is cut off and logged) and delivers
//! at most [`MAX_HITS_PER_CALL`] hits per call, one level deep (a hit queued while handling a hit is dropped).
//!
//! [`prepare_bodies`] and [`HitQueue`] are the shared halves of those rules, used by the server host and by the test
//! double, so both behave alike.

use mod_manifest::ModId;

use crate::ids::Seat;

/// The most bodies one mod may publish; the rest is cut off (and logged).
pub const MAX_BODIES: usize = 64;
/// The most boxes one body may have; the rest is cut off (and logged).
pub const MAX_BOXES_PER_BODY: usize = 16;
/// The most hits one call may queue; the rest is dropped (and logged).
pub const MAX_HITS_PER_CALL: usize = 256;

/// Which part of a body a box (or a hit) is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HitZone {
    Head,
    Chest,
    Stomach,
    Legs,
    Other,
}

/// What made a hit. The owner of the body decides what it means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HitKind {
    /// A shot (hitscan or projectile).
    Bullet,
    /// Close combat.
    Melee,
    /// Area damage (a blast).
    Explosion,
    Other,
}

/// An axis-aligned box: `center` +- `half` on each axis, in world units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitBox {
    pub center: [f32; 3],
    pub half: [f32; 3],
    pub zone: HitZone,
}

impl HitBox {
    pub fn new(center: [f32; 3], half: [f32; 3], zone: HitZone) -> Self {
        Self { center, half, zone }
    }
}

/// One body a mod owns. `id` is the mod's own name for it (unique among its bodies); `seat` is the seat it belongs
/// to (the player behind it).
#[derive(Debug, Clone, PartialEq)]
pub struct Body {
    /// The mod that published the body. Filled in by the host: whatever the mod writes here is replaced.
    pub owner: ModId,
    pub seat: Seat,
    pub id: u32,
    pub boxes: Vec<HitBox>,
    pub alive: bool,
}

impl Body {
    /// A live body; the owner is filled in by the host when it is published.
    pub fn new(seat: Seat, id: u32, boxes: Vec<HitBox>) -> Self {
        Self {
            owner: placeholder_owner(),
            seat,
            id,
            boxes,
            alive: true,
        }
    }

    /// The reference [`crate::ServerCtx::hit`] takes.
    pub fn target(&self) -> BodyRef {
        BodyRef {
            owner: self.owner.clone(),
            id: self.id,
        }
    }
}

fn placeholder_owner() -> ModId {
    ModId::new("unowned").unwrap_or_else(|_| unreachable!("`unowned` is a valid mod id"))
}

/// Names one body of the game: its owner and its id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BodyRef {
    pub owner: ModId,
    pub id: u32,
}

/// A hit on a body. `from` is the seat that dealt it; `damage` and `zone` are the hitting mod's verdict, the owner
/// of the body decides what they do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hit {
    pub from: Seat,
    pub damage: u16,
    pub zone: HitZone,
    pub kind: HitKind,
}

/// Host side of `set_bodies`: sets the owner of every body, cuts the list to [`MAX_BODIES`] and each body to
/// [`MAX_BOXES_PER_BODY`] boxes. `Some(line)` is the warning for the log when something was cut.
pub fn prepare_bodies(mut bodies: Vec<Body>, owner: &ModId) -> (Vec<Body>, Option<String>) {
    let extra_bodies = bodies.len().saturating_sub(MAX_BODIES);
    bodies.truncate(MAX_BODIES);
    let mut cut_boxes = 0usize;
    for b in &mut bodies {
        b.owner = owner.clone();
        cut_boxes += b.boxes.len().saturating_sub(MAX_BOXES_PER_BODY);
        b.boxes.truncate(MAX_BOXES_PER_BODY);
    }
    let warn = (extra_bodies > 0 || cut_boxes > 0).then(|| {
        format!(
            "set_bodies: {extra_bodies} bodies over {MAX_BODIES} and {cut_boxes} boxes over \
             {MAX_BOXES_PER_BODY} per body were cut off"
        )
    });
    (bodies, warn)
}

/// The hits one call queued (host side of `ServerCtx::hit`): at most [`MAX_HITS_PER_CALL`], none while a hit is being
/// delivered (`nested`). What was refused is counted for one warning per call.
#[derive(Debug, Clone, Default)]
pub struct HitQueue {
    hits: Vec<(BodyRef, Hit)>,
    nested: bool,
    over: usize,
    in_hit: usize,
}

impl HitQueue {
    /// A queue for a normal call.
    pub fn new() -> Self {
        Self::default()
    }

    /// A queue for an `on_hit` call: everything pushed is dropped (delivery is one level deep).
    pub fn nested() -> Self {
        Self {
            nested: true,
            ..Self::default()
        }
    }

    pub fn push(&mut self, target: BodyRef, hit: Hit) {
        if self.nested {
            self.in_hit += 1;
        } else if self.hits.len() >= MAX_HITS_PER_CALL {
            self.over += 1;
        } else {
            self.hits.push((target, hit));
        }
    }

    /// The queued hits in order, and the warning for the log if some were refused.
    pub fn finish(self) -> (Vec<(BodyRef, Hit)>, Option<String>) {
        let warn = match (self.over, self.in_hit) {
            (0, 0) => None,
            (n, 0) => Some(format!(
                "hit: {n} hits over {MAX_HITS_PER_CALL} in one call were dropped"
            )),
            (_, n) => Some(format!(
                "hit: {n} hits queued inside on_hit were dropped (delivery is one level deep)"
            )),
        };
        (self.hits, warn)
    }
}
