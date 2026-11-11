//! Orders waiting for the server's answer: the rows of `ui.pending` and the `ui.turn_pending` mark. Nothing
//! here predicts a value; the model keeps showing what the replica holds, and these marks only say that an
//! order about it is on its way.

use protocol::CmdId;
use ui2_model::KeyedList;
use ui2_model::generated::{Pending, TurnPending};

use crate::ui::UiState;

/// The most orders shown as waiting at once; the oldest row goes first. The sink refuses long before this
/// (`TooMany`), so it only bounds a host that never reports answers.
pub(crate) const MAX_PENDING: usize = 64;

/// What the module remembers about an order it sent.
#[derive(Clone, Debug)]
#[allow(dead_code)] // read when answers arrive
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
}
