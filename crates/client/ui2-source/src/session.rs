//! The client session as the source sees it: the connection, the seat, the turn state with its timers, the seats'
//! connections and the chat.
//!
//! [`SessionRead`] only reads (every method takes `&self`); its implementation for [`ClientCore`] forwards to the
//! core's own getters. The source never takes the core's view change set: the renderer is its one consumer.

use client_core::{ChatEntry, ClientCore};
use protocol::{PlayerId, SeatView, TurnState};
use ui2_model::r#gen::Timer;

/// The session facts the UI model shows. Read only.
pub trait SessionRead {
    /// Orders can be sent now: a game, an open socket and a line that is up.
    fn connected(&self) -> bool;
    /// The seat this client plays; `None` outside a game.
    fn me(&self) -> Option<PlayerId>;
    /// The turn state of the game (phase, mode, who ended the turn, timers); `None` before the first one.
    fn turn_state(&self) -> Option<&TurnState>;
    /// Our own "turn ended" as the screens show it; `None` without a turn state.
    fn my_finished(&self) -> Option<bool>;
    /// Time left on the timer of our seat's group, ms; `None`: no timer.
    fn timer_left_ms(&self, now_ms: u64) -> Option<u64>;
    /// The seats of the game and their connections.
    fn seats(&self) -> &[SeatView];
    /// Chat lines received so far (grows when old lines are dropped from [`chat_lines`](Self::chat_lines)).
    fn chat_total(&self) -> u64;
    /// The chat lines still kept, oldest first; the last one is line `chat_total() - 1`.
    fn chat_lines(&self) -> &[ChatEntry];
}

impl SessionRead for ClientCore {
    fn connected(&self) -> bool {
        self.orders_enabled()
    }

    fn me(&self) -> Option<PlayerId> {
        self.attach_info().map(|a| a.player)
    }

    fn turn_state(&self) -> Option<&TurnState> {
        ClientCore::turn_state(self)
    }

    fn my_finished(&self) -> Option<bool> {
        ClientCore::my_finished(self)
    }

    fn timer_left_ms(&self, now_ms: u64) -> Option<u64> {
        ClientCore::timer_left_ms(self, now_ms)
    }

    fn seats(&self) -> &[SeatView] {
        ClientCore::seats(self)
    }

    fn chat_total(&self) -> u64 {
        ClientCore::chat_total(self)
    }

    fn chat_lines(&self) -> &[ChatEntry] {
        ClientCore::chat_lines(self)
    }
}

/// The turn timer of our seat: whole seconds left (rounded down, so the value changes once a second), the
/// timer's length, and whether it stands (a pause or a lost seat).
#[must_use]
pub fn timer(s: &dyn SessionRead, now_ms: u64) -> Option<Timer> {
    let left = s.timer_left_ms(now_ms)?;
    let ts = s.turn_state()?;
    let me = s.me()?;
    let group = ts.seats.iter().find(|x| x.player == me)?.group;
    let t = ts.timers.iter().find(|t| t.group == group)?;
    Some(Timer {
        left_s: i32::try_from(left / 1000).unwrap_or(i32::MAX),
        total_s: i32::try_from(t.len_ms / 1000).unwrap_or(i32::MAX),
        stopped: t.left_ms_when_stopped.is_some() || ts.paused,
    })
}

/// A test double of the session.
pub mod testing {
    use client_core::ChatEntry;
    use protocol::{ChatLine, PlayerId, SeatView, TurnState};

    use super::SessionRead;

    /// A [`SessionRead`] over plain data. The gateway clock is the caller's clock: a timer's deadline is in the
    /// same milliseconds as `now_ms`.
    #[derive(Debug, Clone, Default)]
    pub struct FakeSession {
        pub connected: bool,
        pub me: Option<PlayerId>,
        pub turn: Option<TurnState>,
        pub seats: Vec<SeatView>,
        pub chat: Vec<ChatEntry>,
        /// Lines received in all; [`push_chat`](Self::push_chat) keeps it in step with `chat`.
        pub chat_total: u64,
    }

    impl FakeSession {
        /// A line arrives at `received_ms`.
        pub fn push_chat(&mut self, line: ChatLine, received_ms: u64) {
            self.chat.push(ChatEntry { line, received_ms });
            self.chat_total += 1;
        }
    }

    impl SessionRead for FakeSession {
        fn connected(&self) -> bool {
            self.connected
        }

        fn me(&self) -> Option<PlayerId> {
            self.me
        }

        fn turn_state(&self) -> Option<&TurnState> {
            self.turn.as_ref()
        }

        fn my_finished(&self) -> Option<bool> {
            let me = self.me?;
            self.turn
                .as_ref()?
                .seats
                .iter()
                .find(|s| s.player == me)
                .map(|s| s.finished)
        }

        fn timer_left_ms(&self, now_ms: u64) -> Option<u64> {
            let me = self.me?;
            let ts = self.turn.as_ref()?;
            let group = ts.seats.iter().find(|s| s.player == me)?.group;
            let t = ts.timers.iter().find(|t| t.group == group)?;
            if let Some(left) = t.left_ms_when_stopped {
                return Some(u64::from(left));
            }
            Some(t.deadline_ms.saturating_sub(now_ms))
        }

        fn seats(&self) -> &[SeatView] {
            &self.seats
        }

        fn chat_total(&self) -> u64 {
            self.chat_total
        }

        fn chat_lines(&self) -> &[ChatEntry] {
            &self.chat
        }
    }
}
