//! The root `chat`: the most recent chat lines of the session.

use std::collections::BTreeMap;
use std::sync::Arc;

use ui2_model::r#gen::{ChatRow, ChatTo};
use ui2_model::{Key, KeyedList};

use crate::build::player_key;
use crate::session::SessionRead;

/// Lines the model keeps.
pub const CHAT_ROWS: usize = 50;

/// The turn each line came in, by line number (`chat_total` counts every line ever received).
#[derive(Clone, Debug, Default)]
pub struct ChatLog {
    turns: BTreeMap<u64, i32>,
}

impl ChatLog {
    /// The last [`CHAT_ROWS`] lines of the session, oldest first, keyed by their line number. `turn_now` is the
    /// game turn of this frame: a line seen for the first time is stamped with it.
    pub fn rows(&mut self, s: &dyn SessionRead, turn_now: i32) -> KeyedList<ChatRow> {
        let lines = s.chat_lines();
        let first = s
            .chat_total()
            .saturating_sub(u64::try_from(lines.len()).unwrap_or(u64::MAX));
        let skip = lines.len().saturating_sub(CHAT_ROWS);
        let me = s.me();
        let mut out = KeyedList::new();
        for (i, e) in lines.iter().enumerate().skip(skip) {
            let seq = first + u64::try_from(i).unwrap_or(u64::MAX);
            // SIMPLIFIED(SIMP-244): a chat line carries no game turn, so it shows the turn the client was in when
            // it first saw the line (after a reload, every old line gets the current turn).
            let turn = *self.turns.entry(seq).or_insert(turn_now);
            let line = &e.line;
            // Line numbers are unique: a push never fails.
            let _ = out.try_push(ChatRow {
                seq: Key(seq),
                from: Arc::from(line.from.as_str()),
                from_player: line.from_player.map(player_key),
                to: match line.to {
                    protocol::ChatTo::All => ChatTo::All,
                    protocol::ChatTo::Player(_) => ChatTo::Player,
                },
                text: Arc::from(line.text.as_str()),
                turn,
                mine: me.is_some() && line.from_player == me,
            });
        }
        // Lines that left the list are not shown again.
        let kept = first + u64::try_from(skip).unwrap_or(u64::MAX);
        self.turns = self.turns.split_off(&kept);
        out
    }
}
