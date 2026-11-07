//! Client-server protocol (architecture §5.1, D5). One WebSocket binary message is one message.
//! Enum variant order is the wire format: add variants at the end, never reorder (see tag_stability tests).

mod account;
mod codec;
mod context;
pub mod hash;
mod ipc;
mod lobby;
mod messages;
mod mods;
mod replica;
mod session;
mod stream;
mod view;
mod world;
pub mod wv3;

pub use account::*;
#[cfg(any(feature = "json", test))]
pub use codec::Json;
pub use codec::{Codec, CodecError, Postcard};
pub use context::*;
pub use hash::xxh64;
pub use ipc::*;
pub use lobby::*;
pub use messages::{ClientMsg, GameCommand, RejectReason, ServerMsg};
pub use mods::*;
pub use replica::*;
pub use session::*;
pub use stream::*;
pub use view::*;
pub use world::*;

pub const PROTOCOL_VERSION: u32 = 14;
