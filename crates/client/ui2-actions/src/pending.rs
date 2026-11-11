//! Orders waiting for the server's answer: the rows of `ui.pending` and the `ui.turn_pending` mark. Nothing
//! here predicts a value; the model keeps showing what the replica holds, and these marks only say that an
//! order about it is on its way.

use protocol::{CmdId, CommandOutcome, RejectCode};
use ui2_model::generated::{
    Meta, Pending, RefuseWhy, ToastKind, Turn, TurnPending, Ui, WinCity, Windows,
};
use ui2_model::{KeyedList, ModelStore};

use crate::UiActions;
use crate::ui::UiState;

/// The most orders shown as waiting at once; the oldest row goes first. The sink refuses long before this
/// (`TooMany`), so it only bounds a host that never reports answers.
pub(crate) const MAX_PENDING: usize = 64;

/// What happened on the network that the actions module has to know. The host turns the events of the network
/// client into these, in the order it got them, before it applies the intents of the frame.
#[derive(Clone, Debug, PartialEq)]
pub enum ActionEvent {
    /// The server answered an order (`ClientEvent::OrderAnswered`). `Queued` is not final: the row stays.
    Answered { id: CmdId, outcome: CommandOutcome },
    /// The order did not go through, or maybe did not, and got no answer in time (`OrderRolledBack`).
    RolledBack { id: CmdId, text: String },
    /// The end of turn of the player was taken back, by an order of a unit or city or by the player
    /// (`EndTurnCancelled`). The server does this itself; the client sends nothing.
    EndTurnCancelled { by_order: bool },
    /// A different game was attached (a new stream of another game, or the old one was left): everything
    /// that belonged to the old one goes.
    NewGame,
}

/// What the module remembers about an order it sent.
#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub id: CmdId,
    /// The server said `Applied`; the row goes when the replica has caught up.
    pub answered: bool,
    /// The `turn_pending` mark this order set, if it is `turn.end` or `turn.cancel`.
    pub turn: Option<TurnPending>,
}

/// The sequence number of the row of an order: its command id.
pub(crate) fn seq_of(id: CmdId) -> i32 {
    id as i32
}

/// The reason a toast gives for the code of a rejected order.
fn reject_why(code: RejectCode) -> RefuseWhy {
    match code {
        RejectCode::Core => RefuseWhy::Core,
        RejectCode::TurnChanged => RefuseWhy::TurnChanged,
        RejectCode::TurnEnded => RefuseWhy::TurnEnded,
        RejectCode::NotYourTurn => RefuseWhy::NotYourTurn,
        RejectCode::NoGame => RefuseWhy::NoGame,
        RejectCode::Restarting => RefuseWhy::Restarting,
        RejectCode::TooMany => RefuseWhy::TooMany,
        RejectCode::Invalid => RefuseWhy::Invalid,
        RejectCode::Expired => RefuseWhy::Expired,
        RejectCode::NotSeated => RefuseWhy::NotSeated,
    }
}

impl UiState {
    /// Show an order as waiting. `target` names what it is about (the key of the unit, item, tech or city in its
    /// `Key::to_js` form, or `turn`), which is how the source marks `unit.pending` and `Opt.pending`.
    pub(crate) fn track(
        &mut self,
        id: CmdId,
        intent: &str,
        target: String,
        turn: Option<TurnPending>,
        now_ms: u64,
    ) {
        let seq = seq_of(id);
        let mut rows = std::mem::take(&mut self.edit().pending).into_vec();
        rows.retain(|r| r.seq != seq);
        self.entries.retain(|e| e.id != id);
        rows.push(Pending {
            seq,
            intent: intent.into(),
            target: target.into(),
            since_ms: now_ms as u32,
        });
        self.entries.push(Entry {
            id,
            answered: false,
            turn,
        });
        while rows.len() > MAX_PENDING {
            let gone = rows.remove(0);
            self.entries.retain(|e| seq_of(e.id) != gone.seq);
        }
        // The sequence numbers are unique here, so this cannot fail.
        self.edit().pending = KeyedList::from_vec(rows).unwrap_or_default();
    }

    /// Drop the row of an order that is settled, and the `turn_pending` mark it set. `None` if the module does
    /// not know the order (it came from somewhere else, or was already settled).
    fn settle(&mut self, id: CmdId) -> Option<Entry> {
        let at = self.entries.iter().position(|e| e.id == id)?;
        let entry = self.entries.remove(at);
        let seq = seq_of(id);
        let rows: Vec<Pending> = std::mem::take(&mut self.edit().pending)
            .into_vec()
            .into_iter()
            .filter(|r| r.seq != seq)
            .collect();
        self.edit().pending = KeyedList::from_vec(rows).unwrap_or_default();
        if let Some(mark) = entry.turn
            && self.ui().turn_pending == mark
        {
            self.edit().turn_pending = TurnPending::None;
        }
        Some(entry)
    }

    /// Settle what the model now shows: the orders the server applied once the replica has caught up, and the
    /// `turn_pending` mark once `turn.finished` has the value the order was after.
    fn sweep(&mut self, store: &ModelStore) {
        let caught_up = store.get::<Meta>().is_none_or(|m| !m.catching_up);
        if caught_up {
            let done: Vec<CmdId> = self
                .entries
                .iter()
                .filter(|e| e.answered)
                .map(|e| e.id)
                .collect();
            for id in done {
                self.settle(id);
            }
        }
        if let Some(turn) = store.get::<Turn>() {
            let reached = match self.ui().turn_pending {
                TurnPending::None => false,
                TurnPending::Ending => turn.finished,
                TurnPending::Cancelling => !turn.finished,
            };
            if reached {
                self.edit().turn_pending = TurnPending::None;
            }
        }
    }

    /// Forget everything that belonged to the game that is gone: selection, mode, the production window,
    /// waiting orders, dismissed notes. The tab and chat choices and the toasts stay.
    fn new_game(&mut self) {
        self.entries.clear();
        self.dismissed.clear();
        self.last_chat_ms = None;
        let old = self.ui().clone();
        *self.edit() = Ui {
            production: old.production,
            chat: old.chat,
            windows: Windows {
                production: WinCity::default(),
                ..old.windows
            },
            errors: old.errors,
            ..Ui::default()
        };
    }
}

impl UiActions {
    /// Take in what happened on the network. An answer settles the row of its order: `Applied` marks it done
    /// (it goes in the next [`reconcile`](Self::reconcile) once the replica has caught up); a rejection, a loss
    /// or a rollback shows a toast and clears the row at once. Orders the module did not send are ignored.
    pub fn on_event(&mut self, ev: &ActionEvent, now_ms: u64) {
        match ev {
            ActionEvent::Answered { id, outcome } => match outcome {
                CommandOutcome::Queued => {}
                CommandOutcome::Applied => {
                    if let Some(e) = self.state.entries.iter_mut().find(|e| e.id == *id) {
                        e.answered = true;
                    }
                }
                CommandOutcome::Rejected { code, reason } => {
                    if self.state.settle(*id).is_some() {
                        let detail = (!reason.is_empty()).then_some(reason.as_str());
                        self.state
                            .toast(ToastKind::Rejected, reject_why(*code), detail, now_ms);
                    }
                }
                CommandOutcome::Lost => {
                    if self.state.settle(*id).is_some() {
                        self.state
                            .toast(ToastKind::Lost, RefuseWhy::Other, None, now_ms);
                    }
                }
            },
            ActionEvent::RolledBack { id, text } => {
                if self.state.settle(*id).is_some() {
                    let detail = (!text.is_empty()).then_some(text.as_str());
                    self.state
                        .toast(ToastKind::Lost, RefuseWhy::Other, detail, now_ms);
                }
            }
            ActionEvent::EndTurnCancelled { .. } => {
                if self.state.ui().turn_pending != TurnPending::None {
                    self.state.edit().turn_pending = TurnPending::None;
                }
            }
            ActionEvent::NewGame => self.state.new_game(),
        }
    }

    /// Settle the orders the model has caught up with (called by `reconcile`).
    pub(crate) fn sweep_pending(&mut self, store: &ModelStore) {
        self.state.sweep(store);
    }
}
