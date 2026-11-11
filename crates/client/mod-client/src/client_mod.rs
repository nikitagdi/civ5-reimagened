//! The client half of a mod.

use draw_list::UiDrawList;
use mod_api::{ChannelId, PlayerId};
use platform::InputEvent;

use crate::ctx::ClientCtx;
use crate::types::{AttachInfo, Claim, FrameOut, UiValue};

/// A render system a mod contributes to `MainSlot::Mod`: the engine's own `render::frame::RenderSystem` trait. Its id
/// must be `SystemId::Mod(_)`; the host renumbers it so that two mods never clash.
pub use render::frame::RenderSystem as ModRenderSystem;

/// The client half of a mod. One instance per game; all calls run on the main thread, in load order.
///
/// A panic in a client mod aborts the page (the client builds with `panic = "abort"`, plan decision 7): there is
/// no per-mod isolation in v1. Every method has an empty default, so a mod implements what it needs.
pub trait ClientMod {
    /// The game started (or the mod was enabled mid-session). Open pack requests, register hotkeys, read `kv`.
    fn attach(&mut self, _cx: &mut dyn ClientCtx, _info: &AttachInfo) {}

    /// The game ended or the mod is being turned off. Release the pointer, undo `suppress_ui`, drop cutouts.
    fn detach(&mut self, _cx: &mut dyn ClientCtx) {}

    /// A message from the mod's server half. `from` is the seat the server says caused it.
    fn on_msg(
        &mut self,
        _cx: &mut dyn ClientCtx,
        _ch: ChannelId,
        _from: Option<PlayerId>,
        _payload: &[u8],
    ) {
    }

    /// A key binding registered through `ClientCtx::hotkeys` was pressed.
    fn on_hotkey(&mut self, _cx: &mut dyn ClientCtx, _id: &str) {}

    /// Offered every input event before the game sees it (needs `input.claim`). `Consume` stops the event.
    fn claim_input(&mut self, _cx: &mut dyn ClientCtx, _ev: &InputEvent) -> Claim {
        Claim::Pass
    }

    /// Once per frame, after the game's own update. `dt` is seconds since the last frame.
    fn frame(&mut self, _cx: &mut dyn ClientCtx, _dt: f32) -> FrameOut {
        FrameOut::default()
    }

    /// Draws HUD into the list that is rendered after the game's UI (needs `hud`).
    fn hud(&mut self, _cx: &mut dyn ClientCtx, _list: &mut UiDrawList) {}

    /// Called once, when the renderer is there (after `attach`), to collect render systems (needs `render.pipeline`).
    /// Build the pipelines from `ClientCtx::gpu()`.
    fn render_systems(&mut self, _cx: &mut dyn ClientCtx) -> Vec<Box<dyn ModRenderSystem>> {
        Vec::new()
    }

    /// An intent of the mod's own panels or script (API 1.3, needs `ui`): `mods.<id>.<name>` in the markup
    /// (`data-on-click="intent mods.hello.say()"`) or the script (`ui.intent("mods.hello.say", {})`). `name` is the
    /// part after `mods.<id>.` (`say`); `args` are the intent's arguments by name. Another mod's intents never
    /// arrive here: the host refuses a prefix that is not this mod's before calling.
    fn on_ui_intent(&mut self, _cx: &mut dyn ClientCtx, _name: &str, _args: &[(String, UiValue)]) {}
}
