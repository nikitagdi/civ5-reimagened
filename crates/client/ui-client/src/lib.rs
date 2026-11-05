//! The client side of the original UI (EUI): what `ui` asks the client for, and the per-frame layer.
//!
//! - [`UiBoot`] fetches the `ui` set through the asset loader into a [`UiStore`];
//! - [`ClientText`] is the `ui::TextEngine` over `ui-text` and `loc`; [`ClientHost`] the `ui::UiHost`;
//!   [`StoreFiles`] the `ui::FileSource`;
//! - [`UiLayer`] runs the core each frame: input in, a draw list out, textures resolved by id.
//!
//! No platform code and no renderer: the app passes the loader, the input and the draw list, and uploads the
//! textures the renderer asks for.

mod boot;
mod host;
mod layer;
mod store;
mod text;

pub use boot::{BootPlan, BootState, UiBoot, ui_profile};
pub use host::{ClientHost, HostCells, Projector};
pub use layer::{
    InputResult, LayerConfig, LayerError, LayerHooks, LayerStats, ReadyTexture, UiLayer,
    load_localizer, upload_form,
};
pub use store::{LazyStats, MemoryFile, StoreFiles, UiStore};
pub use text::{ClientText, LocaleRows};
