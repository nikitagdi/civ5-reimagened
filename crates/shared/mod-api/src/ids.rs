//! Identifiers that cross the mod boundary: plain values only (D2).

pub use mod_manifest::{ModId, Version};

/// The API version of this crate. A mod's `mod.api` requirement must admit it. Minor versions are additive
/// (new defaulted trait methods, new capabilities, new variants of `#[non_exhaustive]` enums).
///
/// 1.1: `ServerMod::{on_tick, on_detach, on_hit}`, `ServerCtx::{now_ms, bodies, set_bodies, hit}`, the
/// `world.bodies` capability, `[rust] tick_hz`; on the client, one holder of the camera and of the input at a time.
///
/// 1.2: `ServerCtx::{share_terrain, unshare_terrain}` and the `world.share_terrain` capability: the cosmetic share of
/// the ground of plots a seat has not revealed.
///
/// 1.3: the UI v2 for mods: the `ui` part and the `[ui]` manifest section, the `ui` capability, the catalogue of
/// extension points (`UI_CATALOG`), and on the client `ClientCtx::ui_model` and `ClientMod::on_ui_intent`.
pub const MOD_API_VERSION: Version = Version::new(1, 3, 0);

/// A game player id, the same value the server actor and the protocol use (`protocol::PlayerId`).
pub type PlayerId = i8;

/// A human seat of the game. Seats are keyed by the player id they play, so the mapping is the identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Seat(pub PlayerId);

impl Seat {
    pub const fn player(self) -> PlayerId {
        self.0
    }
}

/// A mod channel id, as declared in `[[channels]]` of the manifest.
pub type ChannelId = u8;
