//! The mod API: `ServerMod`, the `ServerCtx` facade, capabilities, limits and `WorldFacts`. MIT-licensed; no I/O.
//!
//! Only values cross the boundary (D2): bytes, plain structs and ids, never a host type, so a later sandboxed
//! (WIT) host can offer the same surface. Spec: `specs/modding/SPEC-mod-api.md` in the workspace repository.
//!
//! A mod exposes one `register` function and talks to the host only through the context. This is a complete
//! server half, tested with the mock host:
//!
//! ```
//! use mod_api::testing::{MockCtx, MockFacts};
//! use mod_api::{Caps, Outgoing, Seat, ServerCtx, ServerMod, ServerRegistry};
//! use mod_manifest::{Channel, ModId};
//!
//! const MOD_ID: &str = "echo";
//!
//! #[derive(Default)]
//! struct EchoServer {
//!     seen: u32,
//! }
//!
//! impl ServerMod for EchoServer {
//!     fn start(&mut self, _cx: &mut ServerCtx<'_>, saved: Option<&[u8]>) {
//!         self.seen = saved.and_then(|b| b.try_into().ok()).map_or(0, u32::from_le_bytes);
//!     }
//!     fn on_client(&mut self, _cx: &mut ServerCtx<'_>, from: Seat, ch: u8, p: &[u8]) -> Vec<Outgoing> {
//!         self.seen += 1;
//!         vec![
//!             Outgoing::to_sender(ch, p.to_vec()),
//!             Outgoing::all_except(from, ch, p.to_vec()),
//!         ]
//!     }
//!     fn on_attach(&mut self, _cx: &mut ServerCtx<'_>, _seat: Seat) -> Vec<Outgoing> {
//!         vec![Outgoing::to_sender(0, self.seen.to_le_bytes().to_vec())]
//!     }
//!     fn persist(&self) -> Vec<u8> {
//!         self.seen.to_le_bytes().to_vec()
//!     }
//! }
//!
//! pub fn register(r: &mut ServerRegistry) {
//!     r.add(MOD_ID, || Box::new(EchoServer::default()));
//! }
//!
//! let mut reg = ServerRegistry::new();
//! register(&mut reg);
//! assert!(reg.problems().is_empty());
//! let mut m = reg.create(&ModId::new(MOD_ID).unwrap()).unwrap();
//!
//! let channel = Channel {
//!     id: 0,
//!     name: "echo".into(),
//!     c2s_max: 64,
//!     s2c_max: 64,
//!     rate_per_s: 10,
//!     burst: 2,
//! };
//! let mut host = MockCtx::with_channels(MockFacts::new(8, 8), Caps::of(&[mod_api::Cap::NetChannel]), &[channel]);
//! host.start(m.as_mut(), None);
//! let d = host.deliver(m.as_mut(), Seat(1), 0, b"hi");
//! assert_eq!(d.sent.len(), 2);
//! assert_eq!(d.sent[0], Outgoing::to_sender(0, b"hi".to_vec()));
//! assert_eq!(m.persist(), 1u32.to_le_bytes());
//! ```

#![forbid(unsafe_code)]

mod bodies;
mod caps;
mod facts;
mod ids;
mod limits;
mod server;
pub mod testing;

pub use bodies::{
    Body, BodyRef, Hit, HitBox, HitKind, HitQueue, HitZone, MAX_BODIES, MAX_BOXES_PER_BODY,
    MAX_HITS_PER_CALL, prepare_bodies,
};
pub use caps::{Cap, CapDenied, Caps, CapsError};
pub use facts::{MapShape, Owner, PlotClass, PlotIdx, WorldFacts};
pub use ids::{ChannelId, MOD_API_VERSION, ModId, PlayerId, Seat, Version};
pub use limits::{Dropped, Filtered, Limiter, Verdict};
pub use server::{
    Audience, HostServices, LogLevel, Outgoing, RegistryError, ServerCtx, ServerMod, ServerRegistry,
};
