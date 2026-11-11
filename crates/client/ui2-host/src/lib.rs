//! UI v2 host side: the skins of the build and one running UI v2.
//!
//! - [`SkinRegistry`]: the index `ui2/skins.json`, the checks of a downloaded package against it (hash, id, model
//!   paths), and the fallback after a failure (the default skin, else the classic UI; a failed skin is not tried
//!   again in the session).
//! - [`V2Ui`]: the view model store, the replica source, the actions module, and the skin's runtime, text engine
//!   and scene, with the frame in the host's order ([`frame`]). It does not own the replica or the network (the
//!   application lends them per frame) and touches no GPU: the application gives [`V2Ui::scene_mut`] to the
//!   renderer after each frame.
//! - [`NetSink`]: the actions module's order sink over the network client, for one frame ([`HostNet`]).
//! - [`input`]: window events between frames ([`V2Ui::handle_input`]) and clicks on the map
//!   ([`V2Ui::click_map`]).
//! - The skin's scripts: a worker per skin started with it ([`V2Init::scripts`]), drained and fed in the frame.
//!
//! Load failures are values ([`SkinLoadError`]), never panics, so the application can fall back.

#![forbid(unsafe_code)]

pub mod error;
pub mod frame;
pub mod input;
pub mod mods;
pub mod registry;
mod scripts;
pub mod sink;
pub mod v2;

pub use error::{LoadFailure, SkinLoadError};
pub use frame::{V2Frame, V2FrameReport};
pub use input::{MAP_CLICK, map_click};
pub use mods::{ModModelCheck, ModValue, ModWrite};
pub use registry::{
    ApiVersion, Fallback, RegistryError, SCRIPT_API, SkinEntry, SkinRegistry, hash_hex,
};
pub use scripts::SCRIPT_LANG;
pub use sink::{HostNet, NetSink, Offline, refused};
pub use ui2_actions::ActionEvent;
/// The mouse button of a `map.click` (the model's enum, not the platform's).
pub use ui2_model::generated::MouseButton as MapButton;
/// The game's screens a skin may open (or ask the host for).
pub use ui2_model::wire::ScreenId;
/// The intents on the wire, and those of the mods' panels and scripts for their Rust halves.
pub use ui2_model::wire::{ModIdx, ModIntent, Origin, WireIntent};
/// What a map anchor follows, and the model values that name its object.
pub use ui2_model::{Key as ModelKey, Text as ModelText, Value as ModelValue};
pub use ui2_package::profile::action::AnchorKind;
pub use ui2_package::style::Cursor;
pub use ui2_runtime::{Env, InputOutcome, Services};
pub use ui2_script::{ScriptStats, ScriptStatus};
pub use ui2_source::ReplicaView;
pub use v2::{HostClock, LoadStats, RESOLVE_BUDGET_MS, V2Init, V2Ui};
