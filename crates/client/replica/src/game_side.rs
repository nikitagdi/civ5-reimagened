//! `GameSide`: what the exe-api (`crates/client/exe-api`, WO-4) uses of the replica, and nothing else
//! The tests of the exe-api run on a fake of it.
//!
//! The UI types (`UiEvent`, `UiEventKind`, `UiOp`, `UiValue`) belong to `gamecore` (WO-1, `gamecore_abi.h` has the
//! layout of the arguments of each kind); they are re-exported here.

pub use gamecore::replica::NetMessage;
pub use gamecore::{PathNode, ReachFlags, TradePath, UiEvent, UiEventKind, UiOp, UiValue};

/// The replica as the exe-api sees it. Errors are texts for the Lua error or the log.
pub trait GameSide {
    /// Registers the DLL's Lua API (`Players`, `Game`, `Map`, ...) in the state of the UI.
    fn attach_lua(&mut self, lua: &lua_sys::Lua) -> Result<(), String>;
    /// The game was replaced (`generation` changed): objects the Lua state holds are stale.
    fn invalidate_lua(&mut self, lua: &lua_sys::Lua);
    /// Events of the core since the last call, in order.
    fn take_events(&mut self, out: &mut Vec<UiEvent>);
    /// An operation on the interface state of the core.
    fn ui_op(&mut self, op: UiOp, args: &[i32], out: &mut Vec<i32>) -> Result<(), String>;
    /// The UI listens to `GameEvents.<name>`.
    fn subscribe_game_event(&mut self, name: &str);
    /// A `Network.Send*` of the UI (the DLL's `gDLL->send*`). The drain of the core's own box comes first, so
    /// the order of messages is the order the UI made them in.
    fn post_net(&mut self, msg: NetMessage);
    /// Grows by one on every snapshot load or resync.
    fn generation(&self) -> u64;
    /// Journal batches applied so far: a batch can free units and cities (a unit killed or consumed, a city razed)
    /// while the Lua state still holds them.
    fn applied(&self) -> u64 {
        0
    }
    /// The replica is still joining: no base followed yet, or journal batches wait in its queue. What the core announces
    /// meanwhile is the game's past, not news.
    fn catching_up(&self) -> bool {
        false
    }
    /// After a load: whether the loaded game has the seat's end of turn (the router then takes it back before the
    /// next gameplay message, as the exe's `BroadcastGameplayMessage`).
    fn set_turn_sent(&mut self, _sent: bool) {}
    /// Every human has ended the turn (the exe's NetTurnAllComplete): the end of turn can no longer be taken back, so
    /// the router sends a gameplay message without a `CancelEndTurn` ahead of it. Told once per frame.
    fn set_turn_locked(&mut self, _locked: bool) {}
    /// Players (bit = player) whose end of turn the journal applied and whose turn-complete flag the core has not set
    /// yet: it sets it in the next pump, which for the last human is the AI phase the journal does not carry.
    fn turn_ended_by_journal(&self) -> u64 {
        0
    }
    /// The router took the seat's end of turn back ahead of a gameplay message since the last call (the UI's
    /// `Network.HasSentNetTurnComplete` is false again).
    fn take_turn_reopened(&mut self) -> bool {
        false
    }
    /// UI messages of the seat are out and the journal has not brought them back yet: the game shown is behind the
    /// player's own orders (a unit just ordered still looks ready to move).
    fn own_orders_pending(&self) -> bool {
        false
    }
    /// After a load or a resync: the core announces everything alive as `rebuild` events (`CvMap::setupGraphical`).
    fn setup_graphical(&mut self) -> Result<(), String>;
    /// The interface path finder (`Q_PATH`): the steps of `player`'s unit to the plot `(x, y)`, its own plot first; a
    /// target it cannot reach is one node with `flags.unreachable()`.
    fn unit_path(
        &mut self,
        _player: i32,
        _unit: i32,
        _x: i32,
        _y: i32,
    ) -> Result<Vec<PathNode>, String> {
        Err("this game has no path finder".into())
    }
    /// The plots the unit reaches with the moves it has left this turn (`Q_REACHABLE`), not its own plot.
    fn unit_reach(&mut self, _player: i32, _unit: i32) -> Result<Vec<PathNode>, String> {
        Err("this game has no path finder".into())
    }
    /// The plots the unit can hit with a ranged attack right now (`Q_RANGE_TARGETS`): enemies in range only.
    fn unit_range_targets(&mut self, _player: i32, _unit: i32) -> Result<Vec<(i16, i16)>, String> {
        Err("this game has no range targets".into())
    }
    /// The paths of the trade routes the team `viewer` sees (`Q_TRADE_PATHS`, -1 = all): the plots the DLL's trade
    /// route path finder laid, origin city first.
    fn trade_paths(&mut self, _viewer: i32) -> Result<Vec<TradePath>, String> {
        Err("this game has no trade routes".into())
    }
}
