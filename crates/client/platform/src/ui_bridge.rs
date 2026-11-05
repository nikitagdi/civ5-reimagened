//! The page's own screens (web only): sign-in, lobby list and room are plain DOM there, not egui on the canvas.
//!
//! The app hands the views of those screens over as JSON text and takes back the actions the page posted as
//! JSON text. Both sides are polled by the frame, like every service here. The native client has no bridge.

pub trait UiBridge {
    /// Shows `screen` (`"login"`, `"lobby"`, `"room"`, or `"none"`: the canvas has the screen, the page's
    /// screens are hidden) with its view as JSON. Called when the view changed, not every frame.
    fn render(&self, screen: &str, json: &str);

    /// The actions (JSON texts) the page posted since the last call, oldest first.
    fn take_actions(&self) -> Vec<String>;
}
