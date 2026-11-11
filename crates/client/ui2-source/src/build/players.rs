//! The root `players`: the civilizations of the game as the seat's team sees them.

use std::sync::Arc;

use replica::{Player, ReadError};
use ui2_model::KeyedList;
use ui2_model::r#gen::PlayerRow;

use crate::CountingRead;
use crate::build::player_key;

/// A player the list shows: a major civilization (not a city-state, not the barbarians, not the server's
/// observer seat).
#[must_use]
pub fn listed(p: &Player) -> bool {
    !(p.flags.minor() || p.flags.barbarian() || p.flags.observer())
}

/// One row of the list. `seat` is the replica's player.
#[must_use]
pub fn row(p: &Player, seat: u8) -> PlayerRow {
    PlayerRow {
        id: player_key(p.id),
        leader: Arc::from(p.name.as_str()),
        civ: Arc::from(p.civ_name.as_str()),
        nick: p.nick.as_deref().map(Arc::from),
        color: p.color_primary,
        color2: p.color_secondary,
        human: p.flags.human(),
        alive: p.flags.alive(),
        met: p.flags.met(),
        at_war: p.flags.at_war_with_viewer(),
        me: i16::from(p.id) == i16::from(seat),
    }
}

/// The rows in player order (the seats of the game).
#[must_use]
pub fn rows(players: &[Player], seat: u8) -> KeyedList<PlayerRow> {
    let mut list: Vec<&Player> = players.iter().filter(|p| listed(p)).collect();
    list.sort_by_key(|p| p.id);
    let mut out = KeyedList::new();
    for p in list {
        // The core's ids are unique; a repeated one keeps its first row.
        let _ = out.try_push(row(p, seat));
    }
    out
}

/// `players` from `Q_PLAYERS`.
pub(crate) fn players(read: &mut CountingRead<'_, '_>) -> Result<KeyedList<PlayerRow>, ReadError> {
    let seat = read.seat();
    let players = read.players()?;
    Ok(rows(&players, seat))
}

/// The player with an id, from a `Q_PLAYERS` reply.
#[must_use]
pub fn find(players: &[Player], id: i8) -> Option<&Player> {
    players.iter().find(|p| p.id == id)
}
