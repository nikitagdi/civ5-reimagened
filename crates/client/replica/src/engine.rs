//! The game the replica drives. [`Engine`] is the seam between the state machine and `gamecore`: the real one
//! is [`CoreEngine`] over `gamecore::ReplicaGame`; the tests of the state machine use a fake.

use gamecore::{Applied, HostServices, ReplicaGame};

use crate::game_side::{NetMessage, PathNode, TradePath, UiEvent, UiOp};
use crate::read::{
    CatalogEntry, CatalogTable, City, CityBuild, GameInfo, GameRead, Notification, Player,
    ReadError, TopBar, Unit, UnitAction,
};

/// What the state machine needs of the game.
pub trait Engine {
    /// Takes a GCS1 snapshot: the first one loads the game, later ones replace it in place.
    fn load(&mut self, snapshot: &[u8]) -> Result<(), String>;
    /// Replays one GCJ1 batch. `Diverged` leaves the game as it is until the next [`load`](Self::load).
    fn apply(&mut self, batch: &[u8]) -> Result<Applied, String>;
    /// The UI's messages the core collected in its box since the last call.
    fn take_ui_messages(&mut self) -> Result<Vec<NetMessage>, String>;
    /// The replica digest of the game (`gc_state_digest(-3)`): what a server's `CHECK` carries. Costs a pass over
    /// the whole game (0.3 s on a big one in wasm).
    fn digest(&mut self) -> Result<u64, String> {
        Err("no digest".into())
    }
    /// Linear memory of the module in MiB where that is known (wasm), else 0.
    fn memory_mb(&self) -> u32 {
        0
    }
    /// The seat of the player in the next game the engine loads (a core boots once per process, so a new game
    /// reuses it).
    fn set_seat(&mut self, _seat: u8) {}
    /// The seat this engine plays, where it is known: the journal's `NET` records of the seat are the echo of the UI
    /// messages the replica sent.
    fn seat(&self) -> Option<u8> {
        None
    }

    // The calls of `GameSide` that need the core's UI side (WO-1). A fake engine says they are not there.
    fn attach_lua(&mut self, _lua: &lua_sys::Lua) -> Result<(), String> {
        Err(NOT_THERE.into())
    }
    fn invalidate_lua(&mut self, _lua: &lua_sys::Lua) {}
    fn take_events(&mut self, _out: &mut Vec<UiEvent>) {}
    fn ui_op(&mut self, _op: UiOp, _args: &[i32], _out: &mut Vec<i32>) -> Result<(), String> {
        Err(NOT_THERE.into())
    }
    fn subscribe_game_event(&mut self, _name: &str) {}
    fn setup_graphical(&mut self) -> Result<(), String> {
        Err(NOT_THERE.into())
    }
    fn unit_path(
        &mut self,
        _player: i32,
        _unit: i32,
        _x: i32,
        _y: i32,
    ) -> Result<Vec<PathNode>, String> {
        Err(NOT_THERE.into())
    }
    fn unit_reach(&mut self, _player: i32, _unit: i32) -> Result<Vec<PathNode>, String> {
        Err(NOT_THERE.into())
    }
    fn unit_range_targets(&mut self, _player: i32, _unit: i32) -> Result<Vec<(i16, i16)>, String> {
        Err(NOT_THERE.into())
    }
    fn trade_paths(&mut self, _viewer: i32) -> Result<Vec<TradePath>, String> {
        Err(NOT_THERE.into())
    }

    /// The game, read only (the UI v2 source). `None`: this engine has no core to read (the fakes of the tests).
    fn read(&mut self) -> Option<&mut dyn GameRead> {
        None
    }
}

fn player_u8(player: i32) -> Result<u8, String> {
    u8::try_from(player).map_err(|_| format!("bad player {player}"))
}

const NOT_THERE: &str = "this engine has no UI interface";

/// The real core: a `gamecore::ReplicaGame`.
pub struct CoreEngine {
    game: ReplicaGame,
    /// The seat this replica plays.
    seat: u8,
    /// The core's UI events taken out ahead of a journal batch (in order, before the core's queue).
    events: Vec<UiEvent>,
    /// The seat's team as the last `Q_GAME` of the seat said; forgotten on a load and a seat change.
    team: Option<i32>,
}

impl CoreEngine {
    /// Boots the core (game data through `host`) as the replica of `seat`. Takes seconds on a big game: the
    /// databases and the Lua contexts load here.
    pub fn new(host: impl HostServices + 'static, seat: u8) -> Result<Self, String> {
        let game = ReplicaGame::new(host, Some(seat)).map_err(|e| e.to_string())?;
        Ok(CoreEngine {
            game,
            seat,
            events: Vec::new(),
            team: None,
        })
    }

    /// The game, for tests and the exe-api's registration of the Lua API.
    pub fn game(&mut self) -> &mut ReplicaGame {
        &mut self.game
    }
}

impl Engine for CoreEngine {
    fn load(&mut self, snapshot: &[u8]) -> Result<(), String> {
        self.team = None;
        self.game.resync(snapshot).map_err(|e| e.to_string())
    }

    fn apply(&mut self, batch: &[u8]) -> Result<Applied, String> {
        if !others_only(batch, self.seat) {
            return self.game.apply(batch).map_err(|e| e.to_string());
        }
        // The events made before this batch keep their place; the batch's own lose the camera moves.
        self.game.take_ui_events(&mut self.events);
        let r = self.game.apply(batch).map_err(|e| e.to_string());
        let mut made = Vec::new();
        self.game.take_ui_events(&mut made);
        let before = made.len();
        made.retain(|e| !is_camera_move(e));
        if made.len() != before {
            tracing::debug!(
                target: "replica",
                dropped = before - made.len(),
                "camera moves of other players' orders dropped (SIMP-exe-612)"
            );
        }
        self.events.append(&mut made);
        r
    }

    fn take_ui_messages(&mut self) -> Result<Vec<NetMessage>, String> {
        self.game.take_ui_messages().map_err(|e| e.to_string())
    }

    fn digest(&mut self) -> Result<u64, String> {
        self.game
            .game()
            .state_digest(gamecore::replica::REPLICA_DIGEST)
            .map_err(|e| e.to_string())
    }

    fn memory_mb(&self) -> u32 {
        wasm_memory_mb()
    }

    fn set_seat(&mut self, seat: u8) {
        self.seat = seat;
        self.team = None;
        self.game.set_seat(Some(seat));
    }

    fn seat(&self) -> Option<u8> {
        Some(self.seat)
    }

    fn attach_lua(&mut self, lua: &lua_sys::Lua) -> Result<(), String> {
        self.game.attach_lua(lua).map_err(|e| e.to_string())
    }

    fn invalidate_lua(&mut self, lua: &lua_sys::Lua) {
        self.game.invalidate_lua(lua);
    }

    fn take_events(&mut self, out: &mut Vec<UiEvent>) {
        out.append(&mut self.events);
        self.game.take_ui_events(out);
    }

    fn ui_op(&mut self, op: UiOp, args: &[i32], out: &mut Vec<i32>) -> Result<(), String> {
        self.game.ui_op(op, args, out).map_err(|e| e.to_string())
    }

    fn subscribe_game_event(&mut self, name: &str) {
        self.game.subscribe_game_event(name);
    }

    fn setup_graphical(&mut self) -> Result<(), String> {
        self.game.setup_graphical().map_err(|e| e.to_string())
    }

    fn unit_path(
        &mut self,
        player: i32,
        unit: i32,
        x: i32,
        y: i32,
    ) -> Result<Vec<PathNode>, String> {
        self.game
            .game()
            .path(player_u8(player)?, unit, x, y)
            .map_err(|e| e.to_string())
    }

    fn unit_reach(&mut self, player: i32, unit: i32) -> Result<Vec<PathNode>, String> {
        self.game
            .game()
            .reachable(player_u8(player)?, unit)
            .map_err(|e| e.to_string())
    }

    fn unit_range_targets(&mut self, player: i32, unit: i32) -> Result<Vec<(i16, i16)>, String> {
        self.game
            .game()
            .range_targets(player_u8(player)?, gamecore::RangeShooter::Unit(unit))
            .map(|t| t.into_iter().map(|t| (t.x, t.y)).collect())
            .map_err(|e| e.to_string())
    }

    fn trade_paths(&mut self, viewer: i32) -> Result<Vec<TradePath>, String> {
        self.game
            .game()
            .trade_paths(viewer)
            .map_err(|e| e.to_string())
    }

    fn read(&mut self) -> Option<&mut dyn GameRead> {
        Some(self)
    }
}

impl CoreEngine {
    /// The viewer team of the team-wide queries: the seat's team, from a `Q_GAME` of the seat when it is not
    /// known yet (one extra query after a load; the UI source reads `game_info` first anyway).
    fn viewer_team(&mut self) -> Result<i32, ReadError> {
        match self.team {
            Some(t) => Ok(t),
            None => self.game_info().map(|i| i32::from(i.human_team)),
        }
    }
}

/// Only reads: every method is one `gc_query` of the core, none changes the game. `plots_dirty` is not here on
/// purpose: reading it clears the set the renderer consumes.
impl GameRead for CoreEngine {
    fn seat(&self) -> u8 {
        self.seat
    }

    fn team(&self) -> i32 {
        self.team.unwrap_or(-1)
    }

    fn game_info(&mut self) -> Result<GameInfo, ReadError> {
        let info = self.game.game().game_info_for(self.seat)?;
        self.team = Some(i32::from(info.human_team));
        Ok(info)
    }

    fn players(&mut self) -> Result<Vec<Player>, ReadError> {
        let team = self.viewer_team()?;
        Ok(self.game.game().players(team)?)
    }

    fn catalog(&mut self, table: CatalogTable) -> Result<Vec<CatalogEntry>, ReadError> {
        Ok(self.game.game().catalog(table)?)
    }

    fn units(&mut self) -> Result<Vec<Unit>, ReadError> {
        let team = self.viewer_team()?;
        Ok(self.game.game().units(team)?)
    }

    fn unit_actions(&mut self, unit: i32) -> Result<Vec<UnitAction>, ReadError> {
        Ok(self.game.game().unit_actions(self.seat, unit)?)
    }

    fn cities(&mut self) -> Result<Vec<City>, ReadError> {
        let team = self.viewer_team()?;
        Ok(self.game.game().cities(team)?)
    }

    fn city_builds(&mut self, city: i32) -> Result<Vec<CityBuild>, ReadError> {
        Ok(self.game.game().city_builds(self.seat, city)?)
    }

    fn top_bar(&mut self) -> Result<TopBar, ReadError> {
        Ok(self.game.game().top_bar(self.seat)?)
    }

    fn notifications(&mut self) -> Result<Vec<Notification>, ReadError> {
        Ok(self.game.game().notifications(self.seat)?)
    }
}

/// A journal batch with orders (commands, UI messages) of humans other than `seat` and none of the seat's.
///
/// SIMPLIFIED(SIMP-exe-612): the core runs a human's order with that human as its active player (`ActiveAs`, also for the
/// UI messages it delivers inside the pump), so the DLL's camera moves for "the active player" (`CvUnit::found`, the combat
/// and mission code) are raised on every replica for another player's order: another human's first city took the camera
/// into the fog (live report 2026-10-30). The exe would look only for its own player. The events do not say whose order
/// made them, so a batch of others' orders only loses its camera moves; a batch with the seat's orders keeps them.
pub fn others_only(batch: &[u8], seat: u8) -> bool {
    use gamecore::replica::{JournalRecord, decode_journal};
    let Ok(records) = decode_journal(batch) else {
        return false;
    };
    let mut others = false;
    for r in &records {
        let player = match r {
            JournalRecord::Command { player, .. } | JournalRecord::Net { player, .. } => *player,
            _ => continue,
        };
        if player == seat {
            return false;
        }
        others = true;
    }
    others
}

/// A camera move of the core (`lookAt`, `centerCamera`).
pub fn is_camera_move(e: &UiEvent) -> bool {
    matches!(
        e.kind,
        gamecore::UiEventKind::LookAt | gamecore::UiEventKind::CenterCamera
    )
}

#[cfg(target_arch = "wasm32")]
fn wasm_memory_mb() -> u32 {
    (core::arch::wasm32::memory_size(0) * 64 / 1024) as u32
}

#[cfg(not(target_arch = "wasm32"))]
fn wasm_memory_mb() -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use gamecore::NetMessage;
    use gamecore::replica::{JournalRecord, encode_journal};

    use super::others_only;

    fn net(player: u8) -> JournalRecord {
        JournalRecord::Net {
            player,
            message: NetMessage::new("sendPushMission", &[1, 2]).unwrap(),
        }
    }

    fn cmd(player: u8) -> JournalRecord {
        JournalRecord::Command {
            player,
            kind: 3,
            args: vec![1],
            text: None,
        }
    }

    const PUMP: JournalRecord = JournalRecord::Pump {
        slices: 1,
        status: 0,
        turn: 1,
    };

    /// Another human founded a city (his order and the pump that runs it): the batch's camera moves are not the seat's.
    #[test]
    fn a_batch_of_other_humans_orders_is_not_the_seats() {
        assert!(others_only(&encode_journal(&[net(1), PUMP]), 0));
        assert!(others_only(&encode_journal(&[cmd(2), net(1), PUMP]), 0));
        // The seat's own order (alone or with others') keeps them; so does a batch of no orders (the AI, the pump).
        assert!(!others_only(&encode_journal(&[net(0), PUMP]), 0));
        assert!(!others_only(&encode_journal(&[net(1), cmd(0), PUMP]), 0));
        assert!(!others_only(&encode_journal(&[PUMP]), 0));
        assert!(!others_only(b"not a journal", 0));
    }
}
