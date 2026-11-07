//! The server half of a mod: the `ServerMod` trait, the `ServerCtx` facade, `Outgoing` and the registry.
//!
//! The mod returns data and the host sends it; nothing here exposes a host type (D2). All calls of one game run
//! serialised on the game actor in arrival order (S5). The host wraps every call in `catch_unwind`: a panic disables
//! the mod for that game.

use std::collections::BTreeMap;

use mod_manifest::ModId;
use thiserror::Error;

use crate::bodies::{Body, BodyRef, Hit};
use crate::caps::{Cap, CapDenied, Caps};
use crate::facts::{PlotIdx, WorldFacts};
use crate::ids::{ChannelId, Seat};

/// Who receives an [`Outgoing`] message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Audience {
    /// The seat whose message is being handled (for `on_attach`, the attaching seat).
    Sender,
    /// Every human seat.
    All,
    /// Every human seat except one.
    AllExcept(Seat),
    /// The listed seats.
    Seats(Vec<Seat>),
}

/// A message a mod wants sent to the other half on a channel. Delivery order per seat is the order returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub to: Audience,
    pub ch: ChannelId,
    pub payload: Vec<u8>,
}

impl Outgoing {
    pub fn new(to: Audience, ch: ChannelId, payload: Vec<u8>) -> Self {
        Self { to, ch, payload }
    }
    pub fn to_sender(ch: ChannelId, payload: Vec<u8>) -> Self {
        Self::new(Audience::Sender, ch, payload)
    }
    pub fn to_all(ch: ChannelId, payload: Vec<u8>) -> Self {
        Self::new(Audience::All, ch, payload)
    }
    pub fn all_except(seat: Seat, ch: ChannelId, payload: Vec<u8>) -> Self {
        Self::new(Audience::AllExcept(seat), ch, payload)
    }
    pub fn to_seats(seats: Vec<Seat>, ch: ChannelId, payload: Vec<u8>) -> Self {
        Self::new(Audience::Seats(seats), ch, payload)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

/// What the host provides for one mod call. Implemented by the server host and by the test double.
///
/// The methods added in API 1.1 and 1.2 have defaults (no clock, no bodies, no terrain share), so an older
/// implementation still builds.
pub trait HostServices {
    fn facts(&self) -> &dyn WorldFacts;
    /// Spends `cost` tokens of the seat's mod-chosen bucket on `ch` (see `Limiter::allow`).
    fn allow(&mut self, seat: Seat, ch: ChannelId, cost: u32) -> bool;
    /// Writes a line to the server log, tagged with the mod id by the host.
    fn log(&mut self, level: LogLevel, msg: &str);
    /// The host's monotonic millisecond clock at the moment of the call.
    fn now_ms(&self) -> u64 {
        0
    }
    /// The bodies of the other mods of the game, as they were when the call began.
    fn bodies(&self) -> &[Body] {
        &[]
    }
    /// Replaces the calling mod's bodies (the host fills the owner and applies the limits, see [`prepare_bodies`]).
    ///
    /// [`prepare_bodies`]: crate::prepare_bodies
    fn set_bodies(&mut self, _bodies: Vec<Body>) {}
    /// Queues a hit; the host delivers it to the owner after the call (see [`HitQueue`]).
    ///
    /// [`HitQueue`]: crate::HitQueue
    fn hit(&mut self, _target: BodyRef, _hit: Hit) {}
    /// Queues a terrain share for the seat; the host applies it after the call (see [`ShareQueue`], API 1.2).
    ///
    /// [`ShareQueue`]: crate::ShareQueue
    fn share_terrain(&mut self, _seat: Seat, _plots: &[PlotIdx]) {}
    /// Queues the end of this mod's terrain share for the seat (API 1.2).
    fn unshare_terrain(&mut self, _seat: Seat) {}
}

/// The facade a server mod talks to the host through. Created by the host per call.
pub struct ServerCtx<'a> {
    caps: &'a Caps,
    host: &'a mut dyn HostServices,
}

impl<'a> ServerCtx<'a> {
    pub fn new(caps: &'a Caps, host: &'a mut dyn HostServices) -> Self {
        Self { caps, host }
    }

    /// Whole-truth map facts. Needs `world.read`.
    pub fn facts(&self) -> Result<&dyn WorldFacts, CapDenied> {
        self.caps.require(Cap::WorldRead)?;
        Ok(self.host.facts())
    }

    /// Spends `cost` tokens from the seat's cost bucket on the channel (rate and burst from the manifest).
    pub fn allow(&mut self, seat: Seat, ch: ChannelId, cost: u32) -> bool {
        self.host.allow(seat, ch, cost)
    }

    pub fn log(&mut self, level: LogLevel, msg: &str) {
        self.host.log(level, msg);
    }

    pub fn has(&self, cap: Cap) -> bool {
        self.caps.has(cap)
    }

    /// The host's monotonic clock in milliseconds (the clock of the token buckets; the `now_ms` of `on_tick`).
    pub fn now_ms(&self) -> u64 {
        self.host.now_ms()
    }

    /// The bodies the **other** mods of this game published, as they were when this call began. Needs
    /// `world.bodies`.
    pub fn bodies(&self) -> Result<&[Body], CapDenied> {
        self.caps.require(Cap::WorldBodies)?;
        Ok(self.host.bodies())
    }

    /// Replaces this mod's bodies. The host sets [`Body::owner`] and keeps at most [`MAX_BODIES`] bodies of
    /// [`MAX_BOXES_PER_BODY`] boxes (the rest is cut off with a warning in the log). The other mods see the new list
    /// from their next call on. Needs `world.bodies`.
    ///
    /// [`MAX_BODIES`]: crate::MAX_BODIES
    /// [`MAX_BOXES_PER_BODY`]: crate::MAX_BOXES_PER_BODY
    pub fn set_bodies(&mut self, bodies: Vec<Body>) -> Result<(), CapDenied> {
        self.caps.require(Cap::WorldBodies)?;
        self.host.set_bodies(bodies);
        Ok(())
    }

    /// Queues a hit on a body of the game. After this call returns, the host delivers each queued hit, in order, to
    /// the owner's [`ServerMod::on_hit`]. At most [`MAX_HITS_PER_CALL`] per call; a hit queued inside `on_hit`, on an
    /// unknown body or for a mod that is off is dropped with a line in the log. Needs `world.bodies`.
    ///
    /// [`MAX_HITS_PER_CALL`]: crate::MAX_HITS_PER_CALL
    pub fn hit(&mut self, target: BodyRef, hit: Hit) -> Result<(), CapDenied> {
        self.caps.require(Cap::WorldBodies)?;
        self.host.hit(target, hit);
        Ok(())
    }

    /// Lets `seat` draw the ground of `plots` although it has not revealed them (an arena in another player's land): a
    /// first-person camera there stands on the relief every client sees. Cosmetic: the host sends the seat the
    /// fog-free basis records of the plots (relief, terrain, rivers, lakes, relief features), never an owner, a unit,
    /// a city, an improvement, a route or a resource, and the plots stay unrevealed to the game (strategic map,
    /// minimap, tooltips). The client draws them like fogged revealed land.
    ///
    /// After this call returns the host sends only what is new: duplicates, plots off the map and plots this mod
    /// already shared to the seat are dropped. At most [`MAX_SHARE_PLOTS_PER_CALL`] plots per call into the mod (all
    /// its `share_terrain` calls together), the rest is dropped with a warning in the log. A seat that does not have
    /// this mod gets nothing. The share lasts until [`unshare_terrain`](Self::unshare_terrain), until the seat's
    /// client goes away (`on_detach`: share again in the next `on_attach`) or the game ends. Needs
    /// `world.share_terrain`. API 1.2.
    ///
    /// [`MAX_SHARE_PLOTS_PER_CALL`]: crate::MAX_SHARE_PLOTS_PER_CALL
    pub fn share_terrain(&mut self, seat: Seat, plots: &[PlotIdx]) -> Result<(), CapDenied> {
        self.caps.require(Cap::WorldShareTerrain)?;
        self.host.share_terrain(seat, plots);
        Ok(())
    }

    /// Ends this mod's terrain share for `seat` (the other mods' shares stay): the seat's client stops drawing the
    /// plots it has not revealed. Nothing is sent when the mod shared nothing to the seat. Needs
    /// `world.share_terrain`. API 1.2.
    pub fn unshare_terrain(&mut self, seat: Seat) -> Result<(), CapDenied> {
        self.caps.require(Cap::WorldShareTerrain)?;
        self.host.unshare_terrain(seat);
        Ok(())
    }
}

/// The server half of a mod. One instance per game. Every call (including `on_tick` and `on_hit`) runs serialised on
/// the game actor under `catch_unwind`, and what it returns goes through the same capability and size filter.
pub trait ServerMod: Send {
    /// A new game (`saved` is `None`) or a loaded one (the bytes the last `persist` returned).
    fn start(&mut self, cx: &mut ServerCtx<'_>, saved: Option<&[u8]>);

    /// A message from a client. The host has already checked channel, size and rate. Validate against server facts,
    /// never against the payload. Return the acknowledgement to the sender and any broadcast in one call.
    fn on_client(
        &mut self,
        cx: &mut ServerCtx<'_>,
        from: Seat,
        ch: ChannelId,
        payload: &[u8],
    ) -> Vec<Outgoing>;

    /// A seat attached (Fresh attach or reconnect): return the snapshot pages for it, usually `to_sender`.
    fn on_attach(&mut self, cx: &mut ServerCtx<'_>, seat: Seat) -> Vec<Outgoing>;

    /// A new turn began.
    fn on_turn(&mut self, _cx: &mut ServerCtx<'_>, _turn: i32) {}

    /// The game actor's tick, `[rust] tick_hz` times a second (never when it is 0 or absent). `now_ms` is the
    /// host's monotonic clock (also `cx.now_ms()`). A late tick is not repeated: after a stall the next one comes
    /// one period later. There is no sender: `Audience::Sender` reaches nobody. API 1.1.
    fn on_tick(&mut self, _cx: &mut ServerCtx<'_>, _now_ms: u64) -> Vec<Outgoing> {
        Vec::new()
    }

    /// The client of a seat went away (connection lost, left, or kicked). It may come back: then `on_attach` comes
    /// again. `Audience::Sender` reaches nobody. API 1.1.
    fn on_detach(&mut self, _cx: &mut ServerCtx<'_>, _seat: Seat) -> Vec<Outgoing> {
        Vec::new()
    }

    /// A hit another mod (or this one) queued with [`ServerCtx::hit`] on this mod's body `target_id`. Hits queued
    /// here are dropped (one level deep). `Audience::Sender` reaches nobody. API 1.1, cap `world.bodies`.
    fn on_hit(
        &mut self,
        _cx: &mut ServerCtx<'_>,
        _from_mod: &ModId,
        _target_id: u32,
        _hit: &Hit,
    ) -> Vec<Outgoing> {
        Vec::new()
    }

    /// The state for the save (`MODS` section), at most `state_max` bytes.
    fn persist(&self) -> Vec<u8>;

    /// False lets the host skip `persist` on an autosave when nothing changed. The default always saves.
    fn is_dirty(&self) -> bool {
        true
    }

    /// The host saved the state `persist` returned. A mod with a dirty flag clears it here.
    fn mark_saved(&mut self) {}
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RegistryError {
    #[error("invalid mod id `{id}`: {reason}")]
    BadId { id: String, reason: &'static str },
    #[error("mod `{0}` is registered twice")]
    Duplicate(ModId),
}

type Factory = Box<dyn Fn() -> Box<dyn ServerMod> + Send + Sync>;

/// Maps mod ids to factories. A mod crate exposes `pub fn register(r: &mut ServerRegistry)`.
#[derive(Default)]
pub struct ServerRegistry {
    factories: BTreeMap<ModId, Factory>,
    problems: Vec<RegistryError>,
}

impl ServerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a factory. A bad or duplicate id is recorded in [`problems`](Self::problems) (the first
    /// registration wins) so the `register` convention stays infallible.
    pub fn add<F>(&mut self, id: &str, factory: F) -> &mut Self
    where
        F: Fn() -> Box<dyn ServerMod> + Send + Sync + 'static,
    {
        match ModId::new(id) {
            Err(reason) => self.problems.push(RegistryError::BadId {
                id: id.to_string(),
                reason,
            }),
            Ok(id) if self.factories.contains_key(&id) => {
                self.problems.push(RegistryError::Duplicate(id));
            }
            Ok(id) => {
                self.factories.insert(id, Box::new(factory));
            }
        }
        self
    }

    /// A fresh instance of the mod, if registered.
    pub fn create(&self, id: &ModId) -> Option<Box<dyn ServerMod>> {
        self.factories.get(id).map(|f| f())
    }

    pub fn ids(&self) -> impl Iterator<Item = &ModId> {
        self.factories.keys()
    }

    pub fn problems(&self) -> &[RegistryError] {
        &self.problems
    }
}
