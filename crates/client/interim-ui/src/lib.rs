//! SIMPLIFIED(SIMP-001): interim egui UI; the original UI is Lua+XML run by the exe.
//!
//! The only crate that may depend on egui (rule R9). It has three parts:
//! - [`input`]: `platform::InputEvent` to `egui::RawInput`, and the routing rule that decides
//!   which events also reach the map camera (egui gets input first);
//! - [`overlay`]: [`InterimUi`], which runs an egui frame and draws it as a `render::Overlay`
//!   through `egui-wgpu`;
//! - [`screens`]: the screens, each a plain function over `&egui::Context`;
//! - [`labels`]: unit flags and city banners painted over the map.
//!
//! No winit and no browser APIs: input arrives as `platform` events, so it runs the same on the
//! web and natively and is tested without a window.

pub mod input;
pub mod labels;
pub mod overlay;
pub mod screens;

pub use input::InputBridge;
pub use overlay::InterimUi;
