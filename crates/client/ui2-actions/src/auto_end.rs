//! "Multiplayer auto end turn" in UI v2: with the classic client's option on (`civ5.options.MultiplayerAutoEndTurn`,
//! the same switch as the classic options menu's), the turn ends by itself once nothing holds it, as the classic
//! client does it (`exe_api::auto_end`, after the DLL's `CvGame::updateTestEndTurn`):
//!
//! - Part A: nothing of the seat is selected and the last order of the seat is [`WAIT_MS`] old (the DLL's 10 turn
//!   slices of 0.1 s after the last unit move), or there was none this turn;
//! - Part B: the seat gave an order this turn and [`END_COUNTER`] frames went by since it was answered (the
//!   selection is not looked at);
//!
//! and in both only while the turn button says "next turn" (the seat's turn is active, nothing blocks it, it was not
//! ended yet), no order of the seat waits for its answer, no window or popup is up, and the game is connected. The
//! order is the turn button's (`turn.end`, so the route's checks and the waiting state are the button's).
//!
//! As in the classic client (SIMP-exe-700), a take-back of the seat's end of turn (the cancel button, or an order that
//! reopened the turn) latches the auto end off until the turn changes.
//!
//! SIMPLIFIED(SIMP-392): the unit move the DLL waits for is the seat's last order (any order sent through the actions
//! module, its answer included), timed on the host's clock; the city range attack and the core still processing
//! messages, which the classic client also checks, are not looked at (UI v2 has no such state in its model); every
//! skin window but the chat holds the auto end, where the classic client holds it for its popups and the city screen.

use std::sync::Arc;

use ui2_model::generated::{Game, Hud, Meta, Phase, Turn, TurnPending, TurnUi, WindowId};
use ui2_model::wire::{Origin, WireIntent};
use ui2_model::{ModelStore, Value};

use crate::UiActions;
use crate::intent::Intent;
use crate::sink::OrderSink;

/// How long after the seat's last order Part A waits, ms (`s_unitMoveTurnSlice + 10` slices of 0.1 s).
pub const WAIT_MS: u64 = 1000;
/// Frames Part B waits after an order (`setEndTurnCounter(3)`).
pub const END_COUNTER: i32 = 3;
/// The option's key in the store (after `civ5.options.`).
pub const OPTION_KEY: &str = "MultiplayerAutoEndTurn";

#[derive(Debug, Default)]
pub(crate) struct AutoEnd {
    /// The game turn this state is for.
    turn: Option<i32>,
    /// The player took the end of turn back this turn.
    latched: bool,
    /// The seat's turn was ended in the last frame.
    was_finished: bool,
    /// The seat gave an order this turn (`setHasMovedUnit`).
    ordered: bool,
    /// When the last order of the seat was seen waiting.
    last_order_ms: Option<u64>,
    counter: i32,
}

/// A window of the skin or a popup of the game is up.
fn window_up(store: &ModelStore, ui: &ui2_model::generated::Ui) -> bool {
    let w = &ui.windows;
    let open = WindowId::ALL.iter().any(|id| match id {
        WindowId::Chat => false,
        WindowId::Production => w.production.open,
        WindowId::Research => w.research.open,
        WindowId::Menu => w.menu.open,
        WindowId::NotesLog => w.notes_log.open,
        other => w.remote(other.as_str()).is_some_and(|r| r.open),
    });
    open || store
        .get::<Option<Hud>>()
        .and_then(Option::as_ref)
        .is_some_and(|h| h.popup.is_some())
}

impl UiActions {
    /// Once per frame, after the source wrote the frame's model: end the seat's turn when the multiplayer auto end
    /// turn is on and nothing holds it (see the [module docs](crate::auto_end)). Returns whether it sent the end of
    /// turn.
    pub fn auto_end_turn(
        &mut self,
        store: &mut ModelStore,
        sink: &mut dyn OrderSink,
        now_ms: u64,
    ) -> bool {
        let on = self.client.committed(OPTION_KEY) == Some(1);
        let (Some(game), Some(turn), Some(meta)) = (
            store.get::<Game>(),
            store.get::<Turn>(),
            store.get::<Meta>(),
        ) else {
            return false;
        };
        let (game_turn, finished, state, phase, connected) = (
            game.turn,
            turn.finished,
            turn.state,
            game.phase,
            meta.connected,
        );
        let a = &mut self.auto_end;
        if a.turn != Some(game_turn) {
            *a = AutoEnd {
                turn: Some(game_turn),
                was_finished: finished,
                counter: END_COUNTER,
                ..AutoEnd::default()
            };
        }
        // A take-back, by the button or by an order that reopened the turn.
        if a.was_finished && !finished {
            a.latched = true;
        }
        a.was_finished = finished;
        let ui = self.state.ui();
        if !ui.pending.is_empty() {
            a.ordered = true;
            a.last_order_ms = Some(now_ms);
            a.counter = END_COUNTER;
            return false;
        }
        if !on
            || a.latched
            || finished
            || ui.turn_pending != TurnPending::None
            || state != TurnUi::NextTurn
            || phase != Phase::Humans
            || !connected
            || window_up(store, ui)
        {
            a.counter = END_COUNTER;
            return false;
        }
        let quiet = a
            .last_order_ms
            .is_none_or(|t| now_ms >= t.saturating_add(WAIT_MS));
        let mut send = quiet && ui.selection.unit.is_none();
        if !send && a.ordered {
            if a.counter > 0 {
                a.counter -= 1;
            } else {
                send = true;
            }
        }
        if !send {
            return false;
        }
        a.counter = END_COUNTER;
        let wire = WireIntent {
            name: Arc::from("turn.end"),
            args: vec![(Arc::from("force"), Value::Bool(false))],
            // The host's own order, as a script's would be.
            origin: Origin::Script,
        };
        let Ok(intent) = Intent::from_wire(self.schema, &wire) else {
            return false;
        };
        tracing::info!(target: "ui2_actions", "multiplayer auto end turn: the turn ends by itself");
        self.run(&intent, store, sink, now_ms);
        self.publish(store);
        true
    }
}
