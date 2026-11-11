//! Routes: what each intent does. A route changes the `ui` state, asks the sink for something, or refuses with a
//! toast; it never reads the replica, only the model the player sees.

pub mod camera;
pub mod map;
pub mod unit;
pub mod window;

use protocol::{CmdId, Command};
use ui2_model::ModelStore;
use ui2_model::generated::{RefuseWhy, ToastKind, TurnPending};

use crate::sink::OrderSink;
use crate::ui::UiState;
use crate::validate;

/// What a route works with.
pub(crate) struct Ctx<'a> {
    pub ui: &'a mut UiState,
    /// The model as the player sees it (the roots the source wrote).
    pub model: &'a ModelStore,
    pub sink: &'a mut dyn OrderSink,
    pub now_ms: u64,
}

impl Ctx<'_> {
    /// Tell the player the intent was refused, and why. Nothing is sent.
    pub(crate) fn refuse(&mut self, why: RefuseWhy) {
        self.ui.toast(ToastKind::Refused, why, None, self.now_ms);
    }

    /// Send an order stamped with the turn of the model and show it as waiting. `intent` and `target` fill the
    /// `ui.pending` row (see [`UiState::track`]); `mark` is the `ui.turn_pending` value the order sets.
    /// A refusal of the sink becomes a toast and `None`.
    pub(crate) fn send(
        &mut self,
        cmd: Command,
        intent: &str,
        target: String,
        mark: Option<TurnPending>,
    ) -> Option<CmdId> {
        let Some(turn) = validate::game_turn(self.model) else {
            self.refuse(RefuseWhy::NoGame);
            return None;
        };
        match self.sink.command(cmd, turn) {
            Ok(id) => {
                self.ui.track(id, intent, target, mark, self.now_ms);
                if let Some(mark) = mark {
                    self.ui.edit().turn_pending = mark;
                }
                Some(id)
            }
            Err(refused) => {
                self.refuse(refused.why());
                None
            }
        }
    }
}
