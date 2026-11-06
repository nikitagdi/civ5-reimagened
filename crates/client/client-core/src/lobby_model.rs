//! The lobby as the client keeps it (net-client 4.2 item 9, 5): `LobbyState` and `LobbyChoices` as the server
//! sent them, and the host-change detector (NP-D25).
//!
//! `LobbyState` is replaced whole on every message. `RoomScript` (the autostart steps) belongs to NC8.

use protocol::{LobbyChoices, LobbyState, RoomId, RoomState, SeatView};

/// The lobby: the last `LobbyState` and `Choices`.
#[derive(Debug, Clone, Default)]
pub struct LobbyModel {
    state: Option<LobbyState>,
    choices: Option<LobbyChoices>,
}

impl LobbyModel {
    pub fn state(&self) -> Option<&LobbyState> {
        self.state.as_ref()
    }

    pub fn choices(&self) -> Option<&LobbyChoices> {
        self.choices.as_ref()
    }

    /// The room the player is in.
    pub fn room(&self) -> Option<&RoomState> {
        self.state.as_ref().and_then(|s| s.room.as_ref())
    }

    pub(crate) fn set_state(&mut self, state: LobbyState) {
        self.state = Some(state);
    }

    pub(crate) fn set_choices(&mut self, choices: LobbyChoices) {
        self.choices = Some(choices);
    }

    /// A new login starts from nothing; the pick lists stay (they come once per login anyway).
    pub(crate) fn clear_state(&mut self) {
        self.state = None;
    }
}

/// A host change between two neighbouring snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostChange {
    pub from: Option<String>,
    pub to: String,
    pub me: bool,
}

/// Compares the host of two neighbouring snapshots: `SeatView.host` of `Seats` in a game and
/// `RoomState.host` of `LobbyState` in a room (4.2 item 9). The first snapshot after `Fresh` or a login only
/// sets the baseline. A snapshot with no host leaves the last known one, so a host that comes back (the
/// snapshots never changed host) gives no event.
#[derive(Debug, Clone, Default)]
pub struct HostWatch {
    seat_host: Option<String>,
    room: Option<(RoomId, String)>,
}

impl HostWatch {
    /// After `Attached { Fresh }` or a new game.
    pub fn reset_seats(&mut self) {
        self.seat_host = None;
    }

    /// After a login.
    pub fn reset_room(&mut self) {
        self.room = None;
    }

    /// The seat list of a game.
    pub fn seats(&mut self, seats: &[SeatView], my_nick: &str) -> Option<HostChange> {
        let now = seats
            .iter()
            .find(|s| s.host)
            .and_then(|s| match &s.control {
                protocol::SeatControl::Human { nick } => Some(nick.clone()),
                _ => None,
            })?;
        let prev = self.seat_host.replace(now.clone());
        change(prev, now, my_nick)
    }

    /// The room of a `LobbyState`; `None` when the player is in no room.
    pub fn room(&mut self, room: Option<&RoomState>, my_nick: &str) -> Option<HostChange> {
        let Some(room) = room else {
            self.room = None;
            return None;
        };
        let prev = match self.room.replace((room.id, room.host.clone())) {
            // Another room is a first snapshot again.
            Some((id, host)) if id == room.id => Some(host),
            _ => return None,
        };
        change(prev, room.host.clone(), my_nick)
    }
}

fn change(prev: Option<String>, now: String, my_nick: &str) -> Option<HostChange> {
    match prev {
        None => None,
        Some(p) if p == now => None,
        Some(p) => Some(HostChange {
            me: now == my_nick,
            from: Some(p),
            to: now,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{Conn, GameSettings, GameStatus, SeatControl};

    fn seat(player: i8, nick: &str, host: bool) -> SeatView {
        SeatView {
            player,
            control: SeatControl::Human { nick: nick.into() },
            conn: Conn::Online,
            host,
        }
    }

    fn room(id: RoomId, host: &str) -> RoomState {
        RoomState {
            id,
            name: "r".into(),
            host: host.into(),
            status: GameStatus::Lobby,
            settings: GameSettings {
                name: "r".into(),
                world_size: "S".into(),
                map_script: "M".into(),
                map_options: vec![],
                speed: "s".into(),
                start_era: "e".into(),
                handicap: "h".into(),
                minor_civs: 0,
                turn_mode: protocol::TurnMode::Hybrid,
                timer: protocol::TimerMode::Off,
                game_options: vec![],
                seed: None,
                map_seed: None,
                auto_choices: Default::default(),
                diplo_auto_decline: false,
                mods: Vec::new(),
            },
            slots: vec![],
            from_save: None,
            mod_plan: protocol::ModPlan::default(),
        }
    }

    #[test]
    fn seats_first_snapshot_is_a_baseline_then_changes_fire() {
        let mut w = HostWatch::default();
        let a = [seat(0, "alice", true), seat(1, "bob", false)];
        let b = [seat(0, "alice", false), seat(1, "bob", true)];
        assert_eq!(w.seats(&a, "bob"), None);
        assert_eq!(w.seats(&a, "bob"), None, "same host: nothing");
        assert_eq!(
            w.seats(&b, "bob"),
            Some(HostChange {
                from: Some("alice".into()),
                to: "bob".into(),
                me: true
            })
        );
        assert_eq!(
            w.seats(&a, "bob"),
            Some(HostChange {
                from: Some("bob".into()),
                to: "alice".into(),
                me: false
            })
        );
    }

    #[test]
    fn a_snapshot_without_a_host_keeps_the_last_one() {
        let mut w = HostWatch::default();
        let a = [seat(0, "alice", true), seat(1, "bob", false)];
        let none = [seat(0, "alice", false), seat(1, "bob", false)];
        assert_eq!(w.seats(&a, "bob"), None);
        assert_eq!(w.seats(&none, "bob"), None);
        // The former host is back as the host: the host in the snapshots did not change.
        assert_eq!(w.seats(&a, "bob"), None);
    }

    #[test]
    fn reset_makes_the_next_snapshot_a_baseline() {
        let mut w = HostWatch::default();
        let a = [seat(0, "alice", true)];
        let b = [seat(0, "bob", true)];
        assert_eq!(w.seats(&a, "x"), None);
        w.reset_seats();
        assert_eq!(w.seats(&b, "x"), None);
    }

    #[test]
    fn rooms_follow_the_same_rule_per_room() {
        let mut w = HostWatch::default();
        assert_eq!(w.room(None, "me"), None);
        assert_eq!(w.room(Some(&room(1, "alice")), "me"), None);
        assert_eq!(w.room(Some(&room(1, "alice")), "me"), None);
        assert_eq!(
            w.room(Some(&room(1, "me")), "me"),
            Some(HostChange {
                from: Some("alice".into()),
                to: "me".into(),
                me: true
            })
        );
        // Another room, or leaving and coming back: a first snapshot again.
        assert_eq!(w.room(Some(&room(2, "carol")), "me"), None);
        assert_eq!(w.room(None, "me"), None);
        assert_eq!(w.room(Some(&room(2, "dave")), "me"), None);
        w.reset_room();
        assert_eq!(w.room(Some(&room(2, "erin")), "me"), None);
    }

    #[test]
    fn lobby_state_is_replaced_whole() {
        let mut m = LobbyModel::default();
        assert!(m.state().is_none() && m.room().is_none());
        m.set_state(LobbyState {
            room: Some(room(1, "a")),
            ..Default::default()
        });
        assert_eq!(m.room().map(|r| r.id), Some(1));
        m.set_state(LobbyState::default());
        assert!(m.room().is_none() && m.state().is_some());
        m.clear_state();
        assert!(m.state().is_none());
    }
}
