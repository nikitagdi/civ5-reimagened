//! The server half of the example mod `hello`: an echo on channel 0 and a persisted counter.
//!
//! Every `Say` a client sends is repeated to everybody (the speaker included) as an `Echo` carrying the speaker's
//! player id and the running count of echoes. The count survives restarts through `persist`/`start`, and a seat that
//! attaches is told the count. The host owns limits and delivery; the mod only returns data. Registered with one
//! function, [`register`].

#![forbid(unsafe_code)]

use hello_shared::{CH_ECHO, MOD_ID, Msg, decode_state, encode_state};
use mod_api::{LogLevel, Outgoing, Seat, ServerCtx, ServerMod, ServerRegistry};

/// Registers the mod with the host's registry (the `entry` in `mod.toml`).
pub fn register(r: &mut ServerRegistry) {
    r.add(MOD_ID, || Box::new(HelloServer::default()));
}

/// State of the mod in one game.
#[derive(Debug, Default)]
pub struct HelloServer {
    count: u32,
    dirty: bool,
}

impl HelloServer {
    /// Echoes so far.
    pub fn count(&self) -> u32 {
        self.count
    }
}

impl ServerMod for HelloServer {
    fn start(&mut self, cx: &mut ServerCtx<'_>, saved: Option<&[u8]>) {
        self.dirty = false;
        self.count = match saved {
            None => 0,
            Some(bytes) => decode_state(bytes).unwrap_or_else(|| {
                cx.log(
                    LogLevel::Warn,
                    "saved state is not understood; starting from zero",
                );
                0
            }),
        };
    }

    fn on_client(
        &mut self,
        cx: &mut ServerCtx<'_>,
        from: Seat,
        ch: u8,
        payload: &[u8],
    ) -> Vec<Outgoing> {
        if ch != CH_ECHO {
            return Vec::new();
        }
        let text = match Msg::decode(payload) {
            Ok(Msg::Say { text }) => text,
            Ok(_) => {
                cx.log(LogLevel::Warn, "client sent a server-to-client message");
                return Vec::new();
            }
            Err(e) => {
                cx.log(
                    LogLevel::Warn,
                    &format!("bad message from seat {}: {e}", from.player()),
                );
                return Vec::new();
            }
        };
        self.count = self.count.saturating_add(1);
        self.dirty = true;
        let echo = Msg::Echo {
            from: from.player(),
            count: self.count,
            text,
        }
        .encode();
        vec![Outgoing::to_all(CH_ECHO, echo)]
    }

    fn on_attach(&mut self, _cx: &mut ServerCtx<'_>, _seat: Seat) -> Vec<Outgoing> {
        vec![Outgoing::to_sender(
            CH_ECHO,
            Msg::State { count: self.count }.encode(),
        )]
    }

    fn persist(&self) -> Vec<u8> {
        encode_state(self.count)
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn mark_saved(&mut self) {
        self.dirty = false;
    }
}
