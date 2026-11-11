//! Input of UI v2 between frames, and clicks on the map.
//!
//! The application hands every window event to [`V2Ui::handle_input`] before anything of the world sees it. The
//! answer says where it goes next:
//!
//! - [`InputOutcome::Consumed`]: the skin took it (a panel under the pointer, a press held on a panel, a modal
//!   screen, a focused text field, a hotkey); nothing else sees it. What the markup asked for (its intents and
//!   state writes) is applied in the next [`V2Ui::frame`].
//! - [`InputOutcome::PassToWorld`]: the camera and the picker get it. A click the picker turns into a plot comes
//!   back as [`V2Ui::click_map`], a `map.click` intent applied in the next frame after the markup's own.
//! - [`InputOutcome::MinimapClick`]: the application moves the camera to that point of the minimap.

use std::sync::Arc;

use platform::InputEvent;
use ui2_model::Value;
use ui2_model::generated::MouseButton;
use ui2_model::wire::{Origin, WireIntent};
use ui2_package::style::Cursor;
use ui2_runtime::InputOutcome;

use crate::v2::V2Ui;

/// The name of the intent of a click on the map.
pub const MAP_CLICK: &str = "map.click";

/// A `map.click` intent: a click with `button` on the plot `(x, y)`, as the application's picker found it.
pub fn map_click(x: i32, y: i32, button: MouseButton) -> WireIntent {
    WireIntent {
        name: Arc::from(MAP_CLICK),
        args: vec![
            (
                Arc::from("plot"),
                Value::Struct(vec![Value::I32(x), Value::I32(y)]),
            ),
            (Arc::from("button"), Value::Enum(button as u16)),
        ],
        origin: Origin::Map,
    }
}

impl V2Ui {
    /// One window event between frames, in physical window pixels (see the [module docs](crate::input)).
    /// `now_ms` is the application's clock (double clicks).
    pub fn handle_input(&mut self, ev: &InputEvent, now_ms: f64) -> InputOutcome {
        self.skin.runtime.handle_input(ev, now_ms)
    }

    /// A click on a plot of the map that the world took: a `map.click` intent for the next frame.
    pub fn click_map(&mut self, x: i32, y: i32, button: MouseButton) {
        self.push_intent(map_click(x, y, button));
    }

    /// The pointer cursor the skin wants now (`auto`: the world decides). The frame reports the same value as
    /// [`V2FrameReport::cursor`](crate::V2FrameReport::cursor).
    pub fn cursor(&self) -> Cursor {
        self.skin.runtime.cursor()
    }
}
