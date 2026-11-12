//! The client half of the example mod `hello`: a hotkey that says hello, the echo on the HUD, and a cube.
//!
//! - **Hotkey** `Alt+H` sends `Say` on channel 0. The server (`hello-server`) repeats it to everybody.
//! - **UI v2 panel** `echo` (`ui/echo.html` in `hud.right`) shows "Echoes N", the last echo and a **Say hello**
//!   button. The mod writes the panel's fields `mods.hello.*` (`ui/model.toml`) through `ClientCtx::ui_model`; the
//!   button sends the intent `mods.hello.say`, which reaches [`ClientMod::on_ui_intent`] as `say` and sends `Say`
//!   like the hotkey. Without the UI v2 (or the `ui` capability) the writes are refused and ignored.
//! - **HUD** (`ClientMod::hud`, drawn over whichever UI is active) shows the last echo (who said it, what, the running count) and how many times this player has
//!   said hello. What it shows survives a restart: the count comes back from the server (`State` when the game
//!   attaches), the player's own tally and the last echo from the mod's `kv` namespace.
//! - **Cube** `HelloCube` is a render system in `MainSlot::Mod`, standing on a plot the local player owns (the camera
//!   cannot leave the explored map, so a fixed hex would mostly be out of sight; `HEX` is used until the game says who we are).
//!
//! The whole mod is registered by one function, [`register`]. It depends on `mod-client` and `hello-shared` only (R11);
//! everything it draws with comes through `mod_client::gpu`.

#![forbid(unsafe_code)]

mod cube;
mod font;

use hello_shared::{CH_ECHO, MOD_ID, Msg};
use mod_client::gpu::glam::Vec2;
use mod_client::{
    AttachInfo, ClientCtx, ClientMod, ClientRegistry, FrameOut, HotkeySpec, LogLevel,
    ModRenderSystem, UiDrawList, UiValue,
};

pub use cube::{COLOR, HEX, HelloCube, Home, SHADER, SIDE};

/// The id of the hotkey.
pub const HOTKEY_SAY: &str = "say";
/// The key: `Alt+H` (the game binds a bare `H`, so the chord needs a modifier).
pub const HOTKEY_CODE: &str = "KeyH";

/// The intent of the panel's button (`mods.hello.say` in the markup).
pub const INTENT_SAY: &str = "say";

/// The fields of `ui/model.toml`.
pub const FIELD_COUNT: &str = "count";
pub const FIELD_LAST_FROM: &str = "last_from";
pub const FIELD_LAST_TEXT: &str = "last_text";

/// Keys of the mod's `kv` namespace.
pub const KV_SAID: &str = "said";
pub const KV_LAST: &str = "last";

/// Registers the mod with the host's registry (the `entry` in `mod.toml`).
pub fn register(r: &mut ClientRegistry) {
    r.add(MOD_ID, || Box::new(HelloClient::default()));
}

/// The nick of a player id, or `PLAYER <id>` when the replica does not know it.
fn nick(cx: &dyn ClientCtx, id: i8) -> String {
    cx.world()
        .ok()
        .and_then(|w| w.players().into_iter().find(|p| p.id == id))
        .map_or_else(|| format!("PLAYER {id}"), |p| p.nick)
}

/// The last echo the player saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastEcho {
    pub from: i8,
    pub count: u32,
    pub text: String,
}

/// State of the mod in one game.
#[derive(Debug, Default)]
pub struct HelloClient {
    /// Times this player said hello (kept in `kv`).
    said: u32,
    /// Echoes the server has counted, from `State` and `Echo`.
    count: u32,
    last: Option<LastEcho>,
    /// The text of the last echo of an earlier session, from `kv`.
    earlier: Option<String>,
    /// Where the cube stands: a plot of the local player (the camera cannot leave the explored map).
    home: Home,
    /// The world revision `home` was chosen for.
    home_rev: Option<u64>,
    /// The nick the panel shows for the last echo (the replica may name the player only after the echo came).
    shown_from: Option<String>,
}

impl HelloClient {
    /// Where the cube stands.
    pub fn home(&self) -> &Home {
        &self.home
    }

    pub fn said(&self) -> u32 {
        self.said
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn last(&self) -> Option<&LastEcho> {
        self.last.as_ref()
    }

    /// Puts the cube on the first plot the local player owns; re-reads when the world changes.
    fn track_home(&mut self, cx: &mut dyn ClientCtx) {
        let Ok(w) = cx.world() else { return };
        let Some(me) = w.me() else { return };
        let rev = w.revision();
        if self.home_rev == Some(rev) {
            return;
        }
        self.home_rev = Some(rev);
        let spot = (0..w.map().plot_count())
            .find(|p| w.plot_owner(*p) == Some(me))
            .map(|p| w.plot_xy(p));
        if spot.is_some() {
            self.home.set(spot);
        }
    }

    /// Says hello on channel 0 (the hotkey and the panel's button).
    fn say(&mut self, cx: &mut dyn ClientCtx) {
        self.said = self.said.saturating_add(1);
        if let Ok(kv) = cx.kv() {
            kv.set(KV_SAID, &self.said.to_string());
        }
        let msg = Msg::Say {
            text: format!("hello #{}", self.said),
        };
        if let Err(e) = cx.send(CH_ECHO, &msg.encode()) {
            cx.log(LogLevel::Warn, &format!("hello was not sent: {e}"));
        }
    }

    /// Writes what the panel shows into `mods.hello.*`. Without the UI v2 there is nothing to write to.
    fn show(&mut self, cx: &mut dyn ClientCtx) {
        let from = self.last.as_ref().map(|e| nick(cx, e.from));
        self.shown_from.clone_from(&from);
        let Ok(ui) = cx.ui_model() else { return };
        let mut writes = vec![ui.set(FIELD_COUNT, self.count.into())];
        if let (Some(e), Some(from)) = (&self.last, from) {
            writes.push(ui.set(FIELD_LAST_FROM, from.into()));
            writes.push(ui.set(FIELD_LAST_TEXT, e.text.as_str().into()));
        }
        if let Some(Err(e)) = writes.into_iter().find(Result::is_err) {
            cx.log(LogLevel::Warn, &format!("the panel was not updated: {e}"));
        }
    }

    /// The lines of the HUD, top to bottom. `nick` names a player id.
    pub fn lines(&self, nick: impl Fn(i8) -> String) -> Vec<String> {
        let mut lines = vec!["HELLO MOD - PRESS ALT+H TO SAY HELLO".to_string()];
        lines.push(format!("YOU SAID HELLO {} TIMES", self.said));
        match (&self.last, &self.earlier) {
            (Some(e), _) => lines.push(format!("{} SAID: {}", nick(e.from), e.text)),
            (None, Some(t)) => lines.push(format!("LAST TIME: {t}")),
            (None, None) => lines.push("NOBODY HAS SAID ANYTHING YET".to_string()),
        }
        lines.push(format!("ECHOES SO FAR: {}", self.count));
        lines
    }
}

impl ClientMod for HelloClient {
    fn attach(&mut self, cx: &mut dyn ClientCtx, _info: &AttachInfo) {
        if let Ok(kv) = cx.kv() {
            self.said = kv.get(KV_SAID).and_then(|v| v.parse().ok()).unwrap_or(0);
            self.earlier = kv.get(KV_LAST);
        }
        if let Ok(keys) = cx.hotkeys() {
            let mut spec = HotkeySpec::new(HOTKEY_SAY, HOTKEY_CODE, "Say hello");
            spec.alt = true;
            if let Err(e) = keys.register(spec) {
                cx.log(
                    LogLevel::Warn,
                    &format!("the hello hotkey is not bound: {e}"),
                );
            }
        }
    }

    fn on_hotkey(&mut self, cx: &mut dyn ClientCtx, id: &str) {
        if id == HOTKEY_SAY {
            self.say(cx);
        }
    }

    fn frame(&mut self, cx: &mut dyn ClientCtx, _dt: f32) -> FrameOut {
        // The panel names the speaker as soon as the replica knows the nick.
        if let Some(e) = &self.last
            && self.shown_from.as_deref() != Some(nick(cx, e.from).as_str())
        {
            self.show(cx);
        }
        FrameOut::default()
    }

    fn on_ui_intent(&mut self, cx: &mut dyn ClientCtx, name: &str, _args: &[(String, UiValue)]) {
        if name == INTENT_SAY {
            self.say(cx);
        }
    }

    fn on_msg(&mut self, cx: &mut dyn ClientCtx, ch: u8, _from: Option<i8>, payload: &[u8]) {
        if ch != CH_ECHO {
            return;
        }
        match Msg::decode(payload) {
            Ok(Msg::Echo { from, count, text }) => {
                self.count = count;
                if let Ok(kv) = cx.kv() {
                    kv.set(KV_LAST, &text);
                }
                self.last = Some(LastEcho { from, count, text });
                self.show(cx);
            }
            Ok(Msg::State { count }) => {
                self.count = count;
                self.show(cx);
            }
            Ok(Msg::Say { .. }) => cx.log(LogLevel::Warn, "the server sent a Say; ignored"),
            Err(e) => cx.log(
                LogLevel::Warn,
                &format!("a message was not understood: {e}"),
            ),
        }
    }

    fn hud(&mut self, cx: &mut dyn ClientCtx, list: &mut UiDrawList) {
        self.track_home(cx);
        let lines = self.lines(|id| nick(cx, id));
        let scale = 2.0 * cx.screen().scale.max(1.0);
        let at = Vec2::new(12.0 * scale, 90.0 * scale);
        let width = lines
            .iter()
            .map(|l| font::text_width(l, scale))
            .fold(0.0, f32::max);
        let height = font::line_height(scale) * lines.len() as f32;
        let pad = 4.0 * scale;
        list.add_filled_rect(
            at - Vec2::splat(pad),
            at + Vec2::new(width, height) + Vec2::splat(pad),
            [0, 0, 0, 160],
        );
        for (i, line) in lines.iter().enumerate() {
            let y = at.y + font::line_height(scale) * i as f32;
            font::draw_text(list, Vec2::new(at.x, y), scale, line, [255, 235, 180, 255]);
        }
    }

    fn render_systems(&mut self, cx: &mut dyn ClientCtx) -> Vec<Box<dyn ModRenderSystem>> {
        match cx.gpu() {
            Ok(gpu) => vec![Box::new(HelloCube::at(gpu, self.home.clone()))],
            Err(e) => {
                cx.log(LogLevel::Warn, &format!("no cube: {e}"));
                Vec::new()
            }
        }
    }
}
