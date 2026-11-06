//! The client half of the example mod `hello`: a hotkey that says hello, the echo on the HUD, and a cube.
//!
//! - **Hotkey** `Alt+H` sends `Say` on channel 0. The server (`hello-server`) repeats it to everybody.
//! - **HUD** shows the last echo (who said it, what, the running count) and how many times this player has
//!   said hello. What it shows survives a restart: the count comes back from the server (`State` when the game
//!   attaches), the player's own tally and the last echo from the mod's `kv` namespace.
//! - **Cube** `HelloCube` is a render system in `MainSlot::Mod`, standing on one hex.
//!
//! The whole mod is registered by one function, [`register`]. It depends on `mod-client` and `hello-shared` only (R11);
//! everything it draws with comes through `mod_client::gpu`.

#![forbid(unsafe_code)]

mod cube;
mod font;

use hello_shared::{CH_ECHO, MOD_ID, Msg};
use mod_client::gpu::glam::Vec2;
use mod_client::{
    AttachInfo, ClientCtx, ClientMod, ClientRegistry, HotkeySpec, LogLevel, ModRenderSystem,
    UiDrawList,
};

pub use cube::{COLOR, HEX, HelloCube, SHADER, SIDE};

/// The id of the hotkey.
pub const HOTKEY_SAY: &str = "say";
/// The key: `Alt+H` (the game binds a bare `H`, so the chord needs a modifier).
pub const HOTKEY_CODE: &str = "KeyH";

/// Keys of the mod's `kv` namespace.
pub const KV_SAID: &str = "said";
pub const KV_LAST: &str = "last";

/// Registers the mod with the host's registry (the `entry` in `mod.toml`).
pub fn register(r: &mut ClientRegistry) {
    r.add(MOD_ID, || Box::new(HelloClient::default()));
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
}

impl HelloClient {
    pub fn said(&self) -> u32 {
        self.said
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn last(&self) -> Option<&LastEcho> {
        self.last.as_ref()
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
        if id != HOTKEY_SAY {
            return;
        }
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
            }
            Ok(Msg::State { count }) => self.count = count,
            Ok(Msg::Say { .. }) => cx.log(LogLevel::Warn, "the server sent a Say; ignored"),
            Err(e) => cx.log(
                LogLevel::Warn,
                &format!("a message was not understood: {e}"),
            ),
        }
    }

    fn hud(&mut self, cx: &mut dyn ClientCtx, list: &mut UiDrawList) {
        let nick = |id: i8| {
            cx.world()
                .ok()
                .and_then(|w| w.players().into_iter().find(|p| p.id == id))
                .map_or_else(|| format!("PLAYER {id}"), |p| p.nick)
        };
        let lines = self.lines(nick);
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
            Ok(gpu) => vec![Box::new(HelloCube::new(gpu))],
            Err(e) => {
                cx.log(LogLevel::Warn, &format!("no cube: {e}"));
                Vec::new()
            }
        }
    }
}
