//! The control tree, layout, input and Lua contexts of the original Civ5 UI (EUI), run as the
//! UI framework of the exe does. Output is a `draw_list::UiDrawList`.

pub mod config;
mod error;
pub mod fixture;
pub mod flags;
pub mod values;
pub mod xml;

pub use config::UiConfig;
pub use error::UiError;
pub mod host;
pub mod style;
pub mod text;

pub use host::{FileSource, MemoryFiles, StubHost, TextureInfo, UiHost};
pub use text::{LocArg, StubText, TextAlign, TextEngine, TextLayout, TextQuad, TextRequest};

/// Last path segment, lower case: how the UI file system keys names (directories do not count).
pub fn bare_lower(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .trim()
        .to_ascii_lowercase()
}
pub mod anim;
pub mod build;
pub mod layout;
pub mod state;
pub mod tooltip;
pub mod tree;

pub use state::{State, XmlWarning};
pub use tree::{ControlId, Kind};
pub mod context;
pub mod controls;
pub mod core;
pub mod debug;
pub mod draw;
pub mod input;
pub mod lua_api;
pub mod modal;
pub mod shared;

pub use core::{InputResult, UiCore, UiStats};
pub use shared::Shared;

#[cfg(not(target_arch = "wasm32"))]
pub mod dev;
