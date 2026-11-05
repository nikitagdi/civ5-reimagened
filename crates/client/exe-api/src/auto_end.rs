//! The options menu's "Multiplayer Auto End Turn": when nothing holds the seat's turn any more, the turn ends by itself.
//!
//! The original is the DLL's `CvGame::updateTestEndTurn` (Lekmod CvGame.cpp:2312-2445, every frame of the exe; RB4-MP2
//! item 1): with `ICvUserInterface2::IsMPAutoEndTurnEnabled` on (`InterfaceBuddy::IsMPAutoEndTurnEnabled` 0x086b2fe0 reads
//! the options menu's cache, i.e. the checkbox as it is now), no unit selected, not waiting for remote players, a little
//! after the last unit move (`s_unitMoveTurnSlice + 10` turn slices), `canEndTurn()` (no blocker, no
//! ready, busy unit or city) and the AI civs processed, it calls `gDLL->sendTurnComplete()` unless that was sent; a second
//! path (CvGame.cpp:2424) does the same 3 frames after the last unit move ended. Nothing latches: a player who takes the end
//! of turn back and does nothing is ended again. The replica's core answers `IsMPAutoEndTurnEnabled` false (the client
//! decides, not the core), so the exe API does it here, once per frame, through the END TURN button's handshake
//! (`Network.SendTurnComplete`). The option is the client's (`civ5.options.MultiplayerAutoEndTurn` in the page's store,
//! written by the options menu's commit), so it holds for every game of this browser.
//!
//! Both paths of the DLL are here (item 19 of the b9 list; RB4-MP2 item 1):
//! - Part A (`updateTestEndTurn`, CvGame.cpp:2317-2352): no unit has moved yet, or [`SLICES_AFTER_MOVE`] turn slices
//!   have passed since the last unit move of any unit (`CvGame::unitIsMoving`); nothing
//!   selected; the seat can end the turn (no blocker, no ready, busy unit or city) -> send.
//! - Part B (CvGame.cpp:2400-2439): the seat gave a manual order this turn (`setHasMovedUnit`), nothing blocks, a counter of
//!   [`END_COUNTER`] frames runs down (selection is not looked at) -> send, and the counter starts again.
//!
//! A turn slice is what the exe's `GameViewState::Update` 0x0883aa90 makes of it (RB10 item 1): frame time is added to an
//! accumulator, and when that exceeds `getSecsPerTurn()` = 0.1 s one slice passes and the accumulator is reset to 0, so at
//! most one slice per rendered frame (60 fps: a slice every 7 frames). The wait after a unit move is `+ 10` slices, about a
//! second: the `+ 100` branch of Lekmod's NQM macro is commented out (`_Defines.h:299`), in network games too.
//!
//! SIMPLIFIED(SIMP-exe-610): the unit move is the replica's `UnitMoved` of any unit, not the exe's `UnitIsMoving` call; the
//! manual order is the seat's `sendPushMission` / `sendAutoMission` / `sendSwapUnits`; the AI processed checks
//! (`allAICivsProcessedThisTurn`, `allUnitAIProcessed`) are the server's (it refuses an early end of turn).
//!
//! SIMPLIFIED(SIMP-exe-1020): the check runs once per frame of the page. The exe runs `updateTestEndTurn` only on frames
//! on which the unit cycle list is empty and no popup or placement gate holds (`updateIfNotPaused`, C3a), and the slice
//! counter stands still while the game is paused; here the windows hold it as SIMP-exe-701 says, and a pause is not looked at.
//!
//! SIMPLIFIED(SIMP-exe-1021): Part B's `sendAutoMoves` for automated units (workers, explorers; `hasAutoUnit` before the
//! counter runs) is not sent: the replica drops the message and the server's core moves the seat's automated units itself
//! when the end of turn comes (`CvPlayer::setEndTurn` -> `setAutoMoves(true)`), so they move after the end, not before it.
//!
//! SIMPLIFIED(SIMP-exe-1022): the city range attack holds the auto end while one of the seat's cities `CanRangeStrikeNow`
//! (the notification of `CvCity::DoNearbyEnemy` lives as long as that is true), read in Lua once per frame. The original
//! gets it from `CvNotifications::GetEndTurnBlockedType` (CvNotifications.cpp:678), a blocker only while the option is on,
//! which the server's core never reports because it reads the client's option as off; the button stays "Next turn" meanwhile.
//!
//! SIMPLIFIED(SIMP-exe-700): a deliberate deviation, the players' wish (live reports 2026-10-31). The original latches
//! nothing, so a player who clicked "Waiting for players" is ended again half a second later. Here a take-back of the
//! seat's end of turn that the player made (`Network.SendTurnUnready`, or an order that reopened the turn: the router's
//! `CancelEndTurn`) latches the auto end off until the turn changes. Any take-back counts, one after a manual end of turn
//! too (the player who ended by hand and took it back wants to act, not to be ended again). A refusal by the server does
//! not (it is not the player's), nor does a new turn.
//!
//! SIMPLIFIED(SIMP-exe-701): the auto end waits while the exe would show the player a window: the city screen, a popup up
//! or waiting or delayed (the popup queue: diplomacy and trade, the tech tree, the options and game menus, advisors), as
//! the unit cycle does ([`Core::auto_cycle_held`](crate::cycle)), though not for the seat's own orders on their way.

use lua_sys::Lua;

use crate::NetMessage;
use crate::core::Core;
use crate::options::KEY_PREFIX;

/// Turn slices after the last unit move before Part A may end the turn (`s_unitMoveTurnSlice + 10`, CvGame.cpp:2325; the
/// +100 branch of Lekmod's NQM macro is commented out in `_Defines.h:299`, RB10 item 1).
pub const SLICES_AFTER_MOVE: u64 = 10;
/// Frames Part B waits after the last order (`setEndTurnCounter(3)`).
pub const END_COUNTER: i32 = 3;
/// Microseconds of frame time that make one turn slice (`getSecsPerTurn()` = 0.1 s).
const SLICE_US: u64 = 100_000;
/// The frame time assumed when the page has no clock (tests): a sixtieth of a second.
const FRAME_US: u64 = 16_667;

/// The option's key in the store (without [`KEY_PREFIX`]).
pub const OPTION_KEY: &str = "MultiplayerAutoEndTurn";

/// The player took the end of turn back (SIMP-exe-700): the turn it was in (set on the first probe after) until it changes.
#[derive(Debug, Default)]
pub struct AutoEndLatch {
    pub(crate) on: bool,
    turn: Option<i32>,
}

/// `ok`: the seat's turn is free to end (active, no blocking type, `UI.CanEndTurn`); `selected`: a unit of the seat is selected.
const FREE_LUA: &str = "if not Game.IsNetworkMultiPlayer() then return false, false end
local me = Game.GetActivePlayer()
local p = me and me >= 0 and Players[me]
if not p or not p:IsHuman() or not p:IsAlive() or not p:IsTurnActive() then return false, false end
local none = EndTurnBlockingTypes and EndTurnBlockingTypes.NO_ENDTURN_BLOCKING_TYPE or -1
if p:GetEndTurnBlockingType() ~= none then return false, false end
if not UI.CanEndTurn() then return false, false end
if Game.IsProcessingMessages and Game.IsProcessingMessages() then return false, false end
if p.Cities then
  for c in p:Cities() do
    if c.CanRangeStrikeNow and c:CanRangeStrikeNow() then return false, false end
  end
end
local u = UI.GetHeadSelectedUnit()
return true, (u ~= nil and u:GetOwner() == me)";

impl Core {
    /// Is "Multiplayer Auto End Turn" on? The options menu's value as it is now (set, not yet accepted), else the store.
    pub fn mp_auto_end_turn(&self) -> bool {
        let pending = self
            .inner
            .options
            .pending_gen
            .borrow()
            .get(OPTION_KEY)
            .cloned();
        pending
            .or_else(|| self.host.kv().get(&format!("{KEY_PREFIX}{OPTION_KEY}")))
            .is_some_and(|v| v.trim() == "1")
    }

    /// Once per frame (`ExeApi::dispatch`), after the turn handshake is read. `reset`: the replica loaded a new state.
    pub fn track_auto_end_turn(&self, lua: &Lua, reset: bool) {
        self.advance_slice();
        // A latch is for one turn.
        {
            let mut l = self.auto_end_latch.borrow_mut();
            if l.on
                && let Ok(t) = lua
                    .load("return Game.GetGameTurn()")
                    .set_name("=exe_auto_end_turn_no")
                    .eval::<i32>()
            {
                match l.turn {
                    None => l.turn = Some(t),
                    Some(lt) if lt != t => *l = AutoEndLatch::default(),
                    _ => {}
                }
            }
        }
        if reset {
            // A new state came in (the new turn's snapshot or a resync): `setHasMovedUnit(false)` of CvGame::doTurn.
            self.has_moved_unit.set(false);
            self.end_counter.set(END_COUNTER);
            return;
        }
        let latched = self.auto_end_latch.borrow().on;
        if latched
            || self.auto_end_held()
            || !self.mp_auto_end_turn()
            || self.ui.borrow().sent_turn_complete
        {
            return;
        }
        let (ok, selected) = match lua
            .load(FREE_LUA)
            .set_name("=exe_auto_end_turn")
            .eval::<(bool, bool)>()
        {
            Ok(v) => v,
            Err(e) => {
                tracing::debug!(target: "exe.net", "the auto end turn probe failed: {e}");
                return;
            }
        };
        if !ok {
            return;
        }
        let slice = self.turn_slice();
        let moved = self.unit_move_frame.get();
        // Part A.
        let mut send = (moved == 0 || moved + SLICES_AFTER_MOVE < slice) && !selected;
        // Part B.
        if !send && self.has_moved_unit.get() {
            let c = self.end_counter.get();
            if c > 0 {
                self.end_counter.set(c - 1);
            } else {
                send = true;
            }
        }
        if !send {
            return;
        }
        self.end_counter.set(END_COUNTER);
        tracing::info!(target: "exe.net", "multiplayer auto end turn: the turn ends by itself");
        if let Some(m) = NetMessage::new("sendTurnComplete", &[]) {
            self.post_net(m);
        }
    }

    /// The exe's turn slice counter (`CvGame::getTurnSlice`).
    pub(crate) fn turn_slice(&self) -> u64 {
        self.slice.get()
    }

    /// Once per frame, as `GameViewState::Update` 0x0883aa90 does: the frame time is added to an accumulator; when it
    /// exceeds 0.1 s one slice passes and the accumulator is reset to 0 (not reduced), so a frame brings at most one slice.
    fn advance_slice(&self) {
        let dt = if self.clock.borrow().is_some() {
            let now = self.now_us();
            self.slice_last_us
                .replace(Some(now))
                .map_or(0, |last| now.saturating_sub(last))
        } else {
            FRAME_US
        };
        let acc = self.slice_acc_us.get() + dt;
        if acc > SLICE_US {
            self.slice.set(self.slice.get() + 1);
            self.slice_acc_us.set(0);
        } else {
            self.slice_acc_us.set(acc);
        }
    }

    /// A unit moved (`CvGame::unitIsMoving`).
    pub(crate) fn note_unit_moved(&self) {
        self.unit_move_frame.set(self.turn_slice());
    }

    /// The player took the end of turn back: no auto end for the rest of this turn (SIMP-exe-700).
    pub(crate) fn latch_auto_end(&self) {
        *self.auto_end_latch.borrow_mut() = AutoEndLatch {
            on: true,
            turn: None,
        };
    }

    /// A window is up (SIMP-exe-701).
    fn auto_end_held(&self) -> bool {
        {
            let b = self.buddy.borrow();
            if b.is_popup_up() || b.waiting_len() > 0 || b.delayed_len() > 0 {
                return true;
            }
        }
        self.op(crate::UiOp::IsCityScreenUp, &[]).first() == Some(&1)
    }
}
