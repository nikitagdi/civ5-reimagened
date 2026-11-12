//! The roots `meta` (the source's own state) and `game` (turn, calendar, era, phase).

use std::sync::Arc;

use protocol::{TurnPhase, TurnState};
use replica::{GameInfo, GameStatus, Player, ReadError, State};
use ui2_model::r#gen::{EraRef, Game, Meta, Phase, Source, TurnMode};
use ui2_model::{Key, Text};

use crate::CountingRead;
use crate::build::{Cx, player_key};
use crate::trigger::Seen;

/// `meta` from the replica's signals and the session's connection; reads nothing. `seen` is `None` when there is
/// no replica.
#[must_use]
pub fn meta(schema: &Arc<str>, seen: Option<&Seen>, connected: bool) -> Meta {
    Meta {
        schema: schema.clone(),
        source: Source::Replica,
        live: seen.is_some_and(|s| s.state == State::Live),
        catching_up: seen.is_some_and(|s| s.catching_up),
        connected,
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

/// The phase of the turn: the session's turn state when there is one (a network game), else the core's game
/// record (a game without a server turn state).
#[must_use]
pub fn phase(turn: Option<&TurnState>, info: &GameInfo) -> Phase {
    match turn.map(|t| t.phase) {
        Some(TurnPhase::Starting) => Phase::Starting,
        Some(TurnPhase::Ai { .. }) => Phase::Ai,
        Some(TurnPhase::Humans) => Phase::Humans,
        Some(TurnPhase::GameOver { .. }) => Phase::GameOver,
        None if info.status == GameStatus::GameOver => Phase::GameOver,
        None if info.status == GameStatus::AiRunning || info.phase == replica::Phase::Ai => {
            Phase::Ai
        }
        None => Phase::Humans,
    }
}

/// How the humans take their turns: the session's turn state, else the core's game record.
#[must_use]
pub fn mode(turn: Option<&TurnState>, info: &GameInfo) -> TurnMode {
    match turn.map(|t| t.mode) {
        Some(protocol::TurnMode::Hybrid) => TurnMode::Hybrid,
        Some(protocol::TurnMode::Simultaneous) => TurnMode::Simultaneous,
        None => match info.turn_mode {
            Some(replica::TurnMode::Hybrid) => TurnMode::Hybrid,
            Some(replica::TurnMode::Simultaneous) | None => TurnMode::Simultaneous,
        },
    }
}

/// The winning team once the game is over: the session's, else the core's.
fn winner_team(turn: Option<&TurnState>, info: &GameInfo) -> Option<i8> {
    match turn.map(|t| t.phase) {
        Some(TurnPhase::GameOver { winner }) => winner,
        Some(_) => None,
        None => (info.status == GameStatus::GameOver && info.winner >= 0).then_some(info.winner),
    }
}

/// `game` from `Q_GAME` of the seat, the era of `Q_TOP_BAR` and the session's turn state (phase, mode, winner).
pub(crate) fn game(read: &mut CountingRead<'_, '_>, cx: &Cx<'_>) -> Result<Game, ReadError> {
    let info = read.game_info()?;
    // A core without the top bar still has a turn and a year: the era then stays unnamed.
    let era = match read.top_bar() {
        Ok(t) => EraRef {
            id: t.era,
            name: cx.catalog.era(t.era).name,
        },
        Err(_) => EraRef::default(),
    };
    let ts = cx.session.turn_state();
    let winner = match winner_team(ts, &info) {
        // One read of the players, only once the game is over.
        Some(team) => read.players().ok().and_then(|p| first_of_team(&p, team)),
        None => None,
    };
    Ok(Game {
        // The server's turn state or a batch that ended the turn can name the new turn before its snapshot is
        // loaded: the dial shows it at once, the rest of the record follows with the snapshot.
        turn: shown_turn(info.turn, cx.journal_turn, ts),
        year: info.year,
        year_text: year_text(info.year),
        era,
        me: player_key(read.seat().cast_signed()),
        phase: phase(ts, &info),
        mode: mode(ts, &info),
        winner,
    })
}

/// The turn the dial shows: the latest of the game record's, the replica journal's and the session's turn state's.
#[must_use]
pub fn shown_turn(record: i32, journal: Option<i32>, session: Option<&TurnState>) -> i32 {
    record
        .max(journal.unwrap_or(record))
        .max(session.map_or(record, |t| t.turn))
}

/// The first player of a team, the way the winner is shown.
fn first_of_team(players: &[Player], team: i8) -> Option<Key> {
    players
        .iter()
        .find(|p| p.team == team && !p.flags.observer())
        .map(|p| player_key(p.id))
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
