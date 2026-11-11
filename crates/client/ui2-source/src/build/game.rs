//! The roots `meta` (the source's own state) and `game` (turn, calendar, era, phase).

use std::sync::Arc;

use replica::{GameStatus, ReadError, State};
use ui2_model::r#gen::{EraRef, Game, Meta, Phase, Source, TurnMode};
use ui2_model::{Key, Text};

use crate::CountingRead;
use crate::catalog::Catalog;
use crate::trigger::Seen;

/// `meta` from the replica's signals; reads nothing. `seen` is `None` when there is no replica.
#[must_use]
pub fn meta(schema: &Arc<str>, seen: Option<&Seen>) -> Meta {
    Meta {
        schema: schema.clone(),
        source: Source::Replica,
        live: seen.is_some_and(|s| s.state == State::Live),
        catching_up: seen.is_some_and(|s| s.catching_up),
        // The connection state comes from the session, which this source does not read yet: until the session
        // reader lands it stays at the schema's default.
        connected: false,
    }
}

/// The calendar caption: the skin's `year.bc` / `year.ad` key with the year's number.
#[must_use]
pub fn year_text(year: i32) -> Text {
    let n = year.unsigned_abs().to_string();
    if year < 0 {
        Text::key("year.bc", &[&n])
    } else {
        Text::key("year.ad", &[&n])
    }
}

/// `game` from `Q_GAME` of the seat and the era of `Q_TOP_BAR`.
///
/// Phase, turn mode and winner come from the core's game record here; the session's turn state (which the turn
/// root will read) says the same for a live replica.
pub(crate) fn game(read: &mut CountingRead<'_, '_>, catalog: &Catalog) -> Result<Game, ReadError> {
    let info = read.game_info()?;
    let (turn, year, status, core_phase, turn_mode, winner_team) = (
        info.turn,
        info.year,
        info.status,
        info.phase,
        info.turn_mode,
        info.winner,
    );
    // A core without the top bar still has a turn and a year: the era then stays unnamed.
    let era = match read.top_bar() {
        Ok(t) => {
            let id = t.era;
            EraRef {
                id,
                name: catalog.era(id).name,
            }
        }
        Err(_) => EraRef::default(),
    };
    let over = status == GameStatus::GameOver;
    let phase = if over {
        Phase::GameOver
    } else if status == GameStatus::AiRunning || core_phase == replica::Phase::Ai {
        Phase::Ai
    } else {
        Phase::Humans
    };
    let mode = match turn_mode {
        Some(replica::TurnMode::Hybrid) => TurnMode::Hybrid,
        Some(replica::TurnMode::Simultaneous) | None => TurnMode::Simultaneous,
    };
    let winner = if over && winner_team >= 0 {
        winner(read, winner_team)
    } else {
        None
    };
    Ok(Game {
        turn,
        year,
        year_text: year_text(year),
        era,
        me: Key::from_i32(i32::from(read.seat())),
        phase,
        mode,
        winner,
    })
}

/// The first player of the winning team (one read, only once the game is over).
fn winner(read: &mut CountingRead<'_, '_>, team: i8) -> Option<Key> {
    let players = read.players().ok()?;
    players
        .iter()
        .find(|p| p.team == team && !p.flags.observer())
        .map(|p| Key::from_i32(i32::from(p.id)))
}

#[cfg(test)]
mod tests {
    use super::year_text;
    use ui2_model::Text;

    #[test]
    fn year_caption_is_a_skin_key_with_the_number() {
        assert_eq!(year_text(1250), Text::key("year.ad", &["1250"]));
        assert_eq!(year_text(-4000), Text::key("year.bc", &["4000"]));
        assert_eq!(year_text(0), Text::key("year.ad", &["0"]));
    }
}
