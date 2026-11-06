//! The server half of a mod: the `ServerMod` trait, the `ServerCtx` facade, `Outgoing` and the registry.
//!
//! The mod returns data and the host sends it; nothing here exposes a host type (D2). All calls of one game run
//! serialised on the game actor in arrival order (S5). The host wraps every call in `catch_unwind`: a panic disables
//! the mod for that game.

use std::collections::BTreeMap;

use mod_manifest::ModId;
use thiserror::Error;

use crate::caps::{Cap, CapDenied, Caps};
use crate::facts::WorldFacts;
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
pub trait HostServices {
    fn facts(&self) -> &dyn WorldFacts;
    /// Spends `cost` tokens of the seat's mod-chosen bucket on `ch` (see `Limiter::allow`).
    fn allow(&mut self, seat: Seat, ch: ChannelId, cost: u32) -> bool;
    /// Writes a line to the server log, tagged with the mod id by the host.
    fn log(&mut self, level: LogLevel, msg: &str);
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
}

/// The server half of a mod. One instance per game.
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
